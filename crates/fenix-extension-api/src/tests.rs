//! Being a test provider (API 1.6): the tests page, the gutter's marks,
//! failures as problems and the debugger's hand-off are Fenix's; a
//! provider says how its runner finds, runs and reports tests.
//! `[[contributes.test_providers]]` names it, and Fenix calls five
//! methods of [`Extension`](crate::Extension) for it, by its id:
//!
//! | Method | When | Its answer |
//! |---|---|---|
//! | `tests_scan` | a file of its languages is opened, typed in (paused), saved; its `scan` files when it starts | the tests in the file's text: nothing is run |
//! | `tests_plan` | a listing ([`Ask::Discover`]) or a run ([`Ask::Run`], [`Ask::RunAll`]) | the commands to run, in order: a [`Plan`] |
//! | `tests_read` | each batch of a step's output, in order | what it read: [`Report`]s |
//! | `tests_step_ended` | once per step, after its last line | what its reader still had |
//! | `tests_launch` | a test is to be debugged | a launch, or a build to run first |
//!
//! Fenix runs every step itself, kills its whole tree when it's stopped,
//! and gives the provider its output a batch of lines at a time: a call
//! holds a batch, never a whole run, and returns in milliseconds.
//!
//! ```no_run
//! use fenix_extension_api::tests::{Ask, Found, Kind, Line, Outcome, Plan, Report, Step, StepEnd};
//! use fenix_extension_api::{Command, Extension};
//!
//! #[derive(Default)]
//! struct Pack;
//!
//! impl Extension for Pack {
//!     fn tests_scan(&mut self, _provider: &str, path: &str, text: &str) -> Vec<Found> {
//!         // `# test adds` on a line is a test called `adds`.
//!         let file = Found { id: path.into(), label: path.rsplit('/').next().unwrap_or(path).into(), kind: Kind::File, path: Some(path.into()), ..Found::default() };
//!         let tests = text.lines().enumerate().filter_map(|(n, l)| l.strip_prefix("# test ").map(|name| Found {
//!             id: format!("{path}::{name}"),
//!             parent: Some(path.into()),
//!             label: name.into(),
//!             line: Some(n as u32),
//!             ..Found::default()
//!         }));
//!         std::iter::once(file).chain(tests).collect()
//!     }
//!
//!     fn tests_plan(&mut self, _provider: &str, _run: u64, ask: Ask) -> Result<Plan, String> {
//!         let mut command = Command::new("fake-test-runner");
//!         if let Ask::Run(found) = &ask {
//!             command = command.args(found.iter().map(|f| f.id.clone()));
//!         }
//!         Ok(Plan { steps: vec![Step { command, label: "tests".into(), covers: Vec::new(), needs: Vec::new() }], list_after: false })
//!     }
//!
//!     fn tests_read(&mut self, _provider: &str, _run: u64, _step: u32, lines: Vec<Line>) -> Vec<Report> {
//!         // `PASS <id>` and `FAIL <id> <message>`, one a line.
//!         lines.iter().filter_map(|l| {
//!             let (word, rest) = l.text.split_once(' ')?;
//!             let (id, message) = rest.split_once(' ').map_or((rest, None), |(i, m)| (i, Some(m.to_string())));
//!             let outcome = match word { "PASS" => Outcome::Passed, "FAIL" => Outcome::Failed, _ => return None };
//!             Some(Report::Ended(fenix_extension_api::tests::Ended { id: id.into(), outcome, duration_ms: None, message, at: None, text: None }))
//!         }).collect()
//!     }
//!
//!     fn tests_step_ended(&mut self, _provider: &str, _run: u64, _step: u32, _end: StepEnd) -> Vec<Report> {
//!         Vec::new()
//!     }
//! }
//! ```
//!
//! Every extension built on this crate exports the five functions, with
//! the defaults below for those it doesn't write; only its manifest makes
//! it a provider. One that keeps `engine.api` below 1.6 still runs on an
//! older Fenix, which never calls them.

use crate::bindings::fenix::extension::types as wit;
use crate::{host, Command, LaunchConfig};

/// A test, or a group of them, as a provider found it: from a file's text
/// (`tests_scan`), its runner's listing, or a run ([`Report::Found`]). A
/// `Found` for an id Fenix has already is merged with it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Found {
    /// The provider's own name for it, the same in what it scans, lists
    /// and reads from a run: `orbit/lib tests::adds`,
    /// `tests/test_basic.py::test_cases[negative]`. At most 1 KiB, unique
    /// among its provider's.
    pub id: String,
    /// The group it's in, by id: `None` for one at its provider's top.
    pub parent: Option<String>,
    /// What the page calls it: `adds`, `negative`, `orbit (lib)`. At most
    /// 256 characters.
    pub label: String,
    pub kind: Kind,
    /// Its file: `/project/<path>`; or relative, to the project in a
    /// scan's answer and to its step's folder in a run's or a listing's
    /// report; or a full path of the project's machine. In a scan's
    /// answer, `None` for a test or case is the file scanned.
    pub path: Option<String>,
    /// Its first line, and its last when it knows: from 0.
    pub line: Option<u32>,
    pub end_line: Option<u32>,
    /// Left out by its source (`#[ignore = "slow"]`, `@skip`,
    /// `test.skip`): why, or "" when it doesn't say.
    pub skip: Option<String>,
    /// Its cases are found only by running it: a `test.each`, a
    /// `parametrize`, a Go test that calls `t.Run`.
    pub template: bool,
    /// Its own, at most 4 KiB: never shown, given back as it was wherever
    /// Fenix gives this node back ([`Ask::Run`], `tests_launch`) -- what
    /// its runner calls it when the id can't say. Not kept across a
    /// restart.
    pub data: Option<String>,
}

/// What a node of the tree is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Kind {
    /// A Cargo target, a Go package, a Jest project: runs everything under it.
    Group,
    File,
    /// A Rust `mod tests`, a pytest class, a `describe`.
    Suite,
    #[default]
    Test,
    /// One case of a template (`test_cases[negative]`).
    Case,
}

/// What Fenix asks a provider for (`tests_plan`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    /// Its runner's listing of every test it can find in the project.
    Discover,
    /// These, as Fenix has them now: tests, cases, and groups (all under
    /// them). After each `File` or `Suite` come the tests and cases Fenix
    /// has under it, while the ask stays within 10,000 records; one with
    /// nothing after it is planned by its runner's own filter for it.
    Run(Vec<Found>),
    /// Every test.
    RunAll,
}

/// What runs, in order.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Plan {
    pub steps: Vec<Step>,
    /// Its listing costs little once this plan has run (it built what the
    /// listing builds), so Fenix lists after it. Ignored in a listing's
    /// own plan.
    pub list_after: bool,
}

/// One command of a plan.
#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    /// A program its manifest lists (`permissions.process`), run as it is,
    /// with no shell between; `${data}` and `${tools}` in its args and env
    /// are filled in. Its command line, as Windows quotes it, at most
    /// 32,766 characters.
    pub command: Command,
    /// What the page says while it runs: `orbit (lib)`. At most 256
    /// characters.
    pub label: String,
    /// The ids it runs, when it knows them: a group's, a file's or a
    /// suite's stands for everything under it. Empty: it claims nothing
    /// of its own, and what of the ask no step names is judged when the
    /// plan's last step has ended. Ignored in a listing.
    pub covers: Vec<String>,
    /// Files under `/data` its command reads (a reporter, a file of ids):
    /// each must be there when the step starts.
    pub needs: Vec<String>,
}

/// A line of a step's output.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Line {
    /// As UTF-8, what wasn't replaced by U+FFFD; colour codes taken out.
    pub text: String,
    pub stderr: bool,
    /// The line went on past 64 KiB: the rest is the next line given.
    pub cut: bool,
}

/// How a step ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepEnd {
    /// Its program ended: its exit code, `None` for a signal (Linux). On
    /// Windows a crash's NTSTATUS (`0xC0000005`) is negative.
    Exited(Option<i32>),
    /// Stopped: by you, by a run that replaced it, by its provider starting
    /// again, or never started because the run was stopped before it.
    Stopped,
    /// It ran past `tests.timeout_minutes`, and was stopped.
    TimedOut,
    /// It couldn't start, or its output couldn't be read: why.
    Failed(String),
}

/// How a test ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Outcome {
    Passed,
    Failed,
    Skipped,
    /// It couldn't run to a result: its setup or fixture failed.
    Errored,
}

/// A place in a file: from 0, columns in characters.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Location {
    /// `/project/<path>`; relative, to the step's folder; or a full path
    /// of the project's machine.
    pub path: String,
    pub line: u32,
    pub column: Option<u32>,
}

/// A test that ended. The last `Ended` for an id in a run is its result.
#[derive(Debug, Clone, PartialEq)]
pub struct Ended {
    pub id: String,
    pub outcome: Outcome,
    /// How long it took, when its runner says.
    pub duration_ms: Option<f64>,
    /// One line: a failure's message, a skip's reason, why it errored.
    pub message: Option<String>,
    /// Where it failed: the assertion, the panic, the deepest frame in the
    /// project.
    pub at: Option<Location>,
    /// All of it: the diff, the traceback, what it printed.
    pub text: Option<String>,
}

/// What a provider read in its runner's output. Fenix applies them in
/// order.
#[derive(Debug, Clone, PartialEq)]
pub enum Report {
    /// A test or group: from a listing, or found in a run.
    Found(Found),
    /// A test started, by id.
    Started(String),
    Ended(Ended),
    /// What a test printed, or the run did (`id` `None`).
    Output { id: Option<String>, text: String },
    /// What didn't build or load, never a test's failure: the group or
    /// file that didn't, `None` for all the step covers.
    Broken { id: Option<String>, message: String, at: Option<Location> },
    /// A process died: the group whose process it was (what's under it and
    /// hasn't ended in this step is errored with it), `None` for all the
    /// step covers that hasn't ended.
    Crashed { id: Option<String>, message: String },
}

/// How to debug a test.
#[derive(Debug, Clone, PartialEq)]
pub enum Launch {
    /// This launch, now.
    Launch(LaunchConfig),
    /// These first -- a build that says where the test's binary is --
    /// read as a run's are; then `tests_launch` is asked again, once, if
    /// every step exited 0 and nothing was `Broken` or `Crashed`.
    BuildFirst(Plan),
}

/// What its test provider `provider` is in its project now: the runner it
/// uses (`cargo nextest`, at most 256 characters), and why it can't run
/// tests there when it can't (`pytest isn't installed in .venv`, at most 4
/// KiB). `runner` "" and no `problem` is "nothing to run in this project":
/// it isn't shown or asked there until a later call says otherwise. A call
/// that changes what Fenix had lists the project's tests again. API 1.6.
pub fn provider(provider: &str, runner: &str, problem: Option<&str>) -> Result<(), String> {
    host::test_provider_set(provider, runner, problem)
}

impl From<wit::TestKind> for Kind {
    fn from(kind: wit::TestKind) -> Kind {
        match kind {
            wit::TestKind::Group => Kind::Group,
            wit::TestKind::File => Kind::File,
            wit::TestKind::Suite => Kind::Suite,
            wit::TestKind::Test => Kind::Test,
            wit::TestKind::Case => Kind::Case,
        }
    }
}

impl From<Kind> for wit::TestKind {
    fn from(kind: Kind) -> wit::TestKind {
        match kind {
            Kind::Group => wit::TestKind::Group,
            Kind::File => wit::TestKind::File,
            Kind::Suite => wit::TestKind::Suite,
            Kind::Test => wit::TestKind::Test,
            Kind::Case => wit::TestKind::Case,
        }
    }
}

impl From<wit::FoundTest> for Found {
    fn from(f: wit::FoundTest) -> Found {
        Found { id: f.id, parent: f.parent, label: f.label, kind: f.kind.into(), path: f.path, line: f.line, end_line: f.end_line, skip: f.skip, template: f.template, data: f.data }
    }
}

impl From<Found> for wit::FoundTest {
    fn from(f: Found) -> wit::FoundTest {
        wit::FoundTest { id: f.id, parent: f.parent, label: f.label, kind: f.kind.into(), path: f.path, line: f.line, end_line: f.end_line, skip: f.skip, template: f.template, data: f.data }
    }
}

impl From<wit::TestAsk> for Ask {
    fn from(ask: wit::TestAsk) -> Ask {
        match ask {
            wit::TestAsk::Discover => Ask::Discover,
            wit::TestAsk::Run(found) => Ask::Run(found.into_iter().map(Found::from).collect()),
            wit::TestAsk::RunAll => Ask::RunAll,
        }
    }
}

impl From<Step> for wit::TestStep {
    fn from(s: Step) -> wit::TestStep {
        wit::TestStep { command: s.command, label: s.label, covers: s.covers, needs: s.needs }
    }
}

impl From<Plan> for wit::TestPlan {
    fn from(p: Plan) -> wit::TestPlan {
        wit::TestPlan { steps: p.steps.into_iter().map(wit::TestStep::from).collect(), list_after: p.list_after }
    }
}

impl From<wit::TestLine> for Line {
    fn from(l: wit::TestLine) -> Line {
        Line { text: l.text, stderr: l.stderr, cut: l.cut }
    }
}

impl From<wit::TestStepEnd> for StepEnd {
    fn from(end: wit::TestStepEnd) -> StepEnd {
        match end {
            wit::TestStepEnd::Exited(code) => StepEnd::Exited(code),
            wit::TestStepEnd::Stopped => StepEnd::Stopped,
            wit::TestStepEnd::TimedOut => StepEnd::TimedOut,
            wit::TestStepEnd::Failed(why) => StepEnd::Failed(why),
        }
    }
}

impl From<Outcome> for wit::TestOutcome {
    fn from(o: Outcome) -> wit::TestOutcome {
        match o {
            Outcome::Passed => wit::TestOutcome::Passed,
            Outcome::Failed => wit::TestOutcome::Failed,
            Outcome::Skipped => wit::TestOutcome::Skipped,
            Outcome::Errored => wit::TestOutcome::Errored,
        }
    }
}

impl From<Location> for wit::TestLocation {
    fn from(l: Location) -> wit::TestLocation {
        wit::TestLocation { path: l.path, line: l.line, column: l.column }
    }
}

impl From<Ended> for wit::TestEnded {
    fn from(e: Ended) -> wit::TestEnded {
        wit::TestEnded { id: e.id, outcome: e.outcome.into(), duration_ms: e.duration_ms, message: e.message, at: e.at.map(Into::into), text: e.text }
    }
}

impl From<Report> for wit::TestReport {
    fn from(r: Report) -> wit::TestReport {
        match r {
            Report::Found(f) => wit::TestReport::Found(f.into()),
            Report::Started(id) => wit::TestReport::Started(id),
            Report::Ended(e) => wit::TestReport::Ended(e.into()),
            Report::Output { id, text } => wit::TestReport::Output(wit::TestOutput { id, text }),
            Report::Broken { id, message, at } => wit::TestReport::Broken(wit::TestBroken { id, message, at: at.map(Into::into) }),
            Report::Crashed { id, message } => wit::TestReport::Crashed(wit::TestCrash { id, message }),
        }
    }
}

impl From<Launch> for wit::TestLaunch {
    fn from(l: Launch) -> wit::TestLaunch {
        match l {
            Launch::Launch(config) => wit::TestLaunch::Launch(config),
            Launch::BuildFirst(plan) => wit::TestLaunch::BuildFirst(plan.into()),
        }
    }
}

/// The five exports, as [`export!`](crate::export) writes them: each takes
/// what Fenix gives as the extension's types and gives back its answer as
/// the API's.
#[doc(hidden)]
pub mod exports {
    use super::*;

    pub fn found(found: Vec<Found>) -> Vec<wit::FoundTest> {
        found.into_iter().map(Into::into).collect()
    }

    pub fn plan(plan: Result<Plan, String>) -> Result<wit::TestPlan, String> {
        plan.map(Into::into)
    }

    pub fn lines(lines: Vec<wit::TestLine>) -> Vec<Line> {
        lines.into_iter().map(Into::into).collect()
    }

    pub fn reports(reports: Vec<Report>) -> Vec<wit::TestReport> {
        reports.into_iter().map(Into::into).collect()
    }

    pub fn launch(launch: Result<Launch, String>) -> Result<wit::TestLaunch, String> {
        launch.map(Into::into)
    }
}

#[cfg(test)]
mod checks {
    use super::*;

    #[test]
    fn what_a_provider_gives_is_the_apis_field_for_field() {
        let found = Found { id: "a/lib t::x".into(), parent: Some("a/lib".into()), label: "x".into(), kind: Kind::Case, path: Some("src/lib.rs".into()), line: Some(3), end_line: Some(9), skip: Some(String::new()), template: true, data: Some("{\"n\":1}".into()) };
        let wit: wit::FoundTest = found.clone().into();
        assert_eq!((wit.kind, wit.end_line, wit.template, wit.data.as_deref()), (wit::TestKind::Case, Some(9), true, Some("{\"n\":1}")));
        assert_eq!(Found::from(wit), found, "back as it was");
        assert_eq!(Found::default().kind, Kind::Test);
        let ask: Ask = wit::TestAsk::Run(vec![found.clone().into()]).into();
        assert_eq!(ask, Ask::Run(vec![found]));
        assert_eq!(Ask::from(wit::TestAsk::RunAll), Ask::RunAll);
        let plan = Plan { steps: vec![Step { command: Command::new("cargo").arg("test"), label: "orbit".into(), covers: vec!["orbit".into()], needs: vec!["/data/r.py".into()] }], list_after: true };
        let wit: wit::TestPlan = plan.into();
        assert_eq!((wit.steps.len(), wit.list_after, wit.steps[0].command.program.as_str(), wit.steps[0].needs[0].as_str()), (1, true, "cargo", "/data/r.py"));
        assert_eq!(StepEnd::from(wit::TestStepEnd::Exited(Some(-1_073_741_819))), StepEnd::Exited(Some(-1_073_741_819)));
        assert_eq!(Line::from(wit::TestLine { text: "x".into(), stderr: true, cut: true }), Line { text: "x".into(), stderr: true, cut: true });
    }

    #[test]
    fn each_report_is_its_case() {
        let at = Location { path: "src/lib.rs".into(), line: 4, column: Some(5) };
        let reports = exports::reports(vec![
            Report::Started("t".into()),
            Report::Ended(Ended { id: "t".into(), outcome: Outcome::Errored, duration_ms: Some(0.5), message: Some("m".into()), at: Some(at.clone()), text: None }),
            Report::Output { id: None, text: "o".into() },
            Report::Broken { id: Some("g".into()), message: "b".into(), at: Some(at) },
            Report::Crashed { id: Some("g".into()), message: "SIGABRT".into() },
        ]);
        assert!(matches!(&reports[0], wit::TestReport::Started(id) if id == "t"));
        assert!(matches!(&reports[1], wit::TestReport::Ended(e) if e.outcome == wit::TestOutcome::Errored && e.at.as_ref().is_some_and(|a| a.column == Some(5))));
        assert!(matches!(&reports[2], wit::TestReport::Output(o) if o.id.is_none() && o.text == "o"));
        assert!(matches!(&reports[3], wit::TestReport::Broken(b) if b.id.as_deref() == Some("g")));
        assert!(matches!(&reports[4], wit::TestReport::Crashed(c) if c.id.as_deref() == Some("g") && c.message == "SIGABRT"));
        let launch = exports::launch(Ok(Launch::BuildFirst(Plan::default()))).unwrap();
        assert!(matches!(launch, wit::TestLaunch::BuildFirst(p) if p.steps.is_empty()));
        assert!(matches!(exports::launch(Ok(Launch::Launch(LaunchConfig::launch("t", &["rust"])))).unwrap(), wit::TestLaunch::Launch(c) if c.name == "t"));
    }
}
