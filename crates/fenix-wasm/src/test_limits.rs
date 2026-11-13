//! Test providers' limits (API 1.6, `docs/plans/test-explorer.md` §6.5),
//! both ways, and what the host does at the boundary with an answer over
//! one: a string is cut, a list's rest left out, and what was cut is said
//! once on the provider's output page (`Cuts`). The rest -- a step whose
//! command line is over its limit or whose `needs` aren't there, what
//! Fenix keeps of results, the asks and batches Fenix makes -- is the
//! test explorer's, which reads its numbers here.

use std::collections::BTreeSet;

use crate::host::{FoundTest, LaunchConfig, TestEnded, TestLaunch, TestLocation, TestPlan, TestReport, TestStep};

/// A call's answer, in all: past it, the rest of its list is left out
/// from where it went over.
pub const MAX_ANSWER: usize = 16 << 20;
/// `tests-scan`'s answer.
pub const MAX_SCANNED: usize = 10_000;
/// A `found-test`'s `id` and `parent`, and any id a report or a step's
/// `covers` names: one over it is cut the same way wherever it comes
/// (`cut_id`), so a long id still matches, and two long ids stay two.
pub const MAX_ID: usize = 1 << 10;
/// What the page shows of a test (`found-test.label`) or a step
/// (`test-step.label`), in characters.
pub const MAX_LABEL: usize = 256;
/// A path: `found-test.path`, `test-location.path`, one of a step's
/// `needs` (one longer isn't cut but fails its step: cut, it would name
/// another file).
pub const MAX_PATH: usize = 4 << 10;
/// `found-test.skip`.
pub const MAX_SKIP: usize = 1 << 10;
/// `found-test.data`: one over it isn't kept, rather than given back cut.
pub const MAX_DATA: usize = 4 << 10;
/// The `data` Fenix keeps of a provider's tests, in all; past it a
/// node's isn't kept, and isn't given back.
pub const MAX_DATA_KEPT: usize = 16 << 20;
/// A `run` ask's records (Fenix's): past it, a selection is asked as
/// its parents.
pub const MAX_ASK: usize = 10_000;
/// A plan's steps.
pub const MAX_STEPS: usize = 256;
/// The ids in all a plan's steps' `covers`.
pub const MAX_COVERS: usize = 10_000;
/// A step's `needs`.
pub const MAX_NEEDS: usize = 32;
/// The files a plan's steps need, in all, in bytes.
pub const MAX_NEEDS_SIZE: u64 = 16 << 20;
/// A step's command line as Windows quotes it (the program, then each
/// argument quoted as `CommandLineToArgvW` reads it, joined by spaces),
/// in characters, on every platform: `CreateProcessW`'s 32,767 with its
/// null.
pub const MAX_COMMAND_LINE: usize = 32_766;
/// The same for a program that resolves to a `.cmd` or `.bat` on
/// Windows, which runs through `cmd.exe`.
pub const MAX_BATCH_COMMAND_LINE: usize = 8_191;
/// A step's `env`, names and values in all, in bytes: the rest is left
/// out.
pub const MAX_ENV: usize = 64 << 10;
/// A `tests-read`'s lines (Fenix's).
pub const MAX_READ_LINES: usize = 512;
/// A `tests-read`'s text, in bytes (Fenix's).
pub const MAX_READ_BYTES: usize = 256 << 10;
/// The longest line Fenix gives whole: past it, the rest is the next
/// line, this one `cut`.
pub const MAX_LINE: usize = 64 << 10;
/// Reports a call.
pub const MAX_REPORTS: usize = 10_000;
/// A report's `message`; what Fenix keeps of one.
pub const MAX_MESSAGE: usize = 4 << 10;
/// A report's `text`, and an `output`'s: their ends. What Fenix keeps of
/// a failure's text and of what a test printed.
pub const MAX_TEXT: usize = 64 << 10;
/// `tests-plan`'s and `tests-launch`'s error, and `failed`'s reason.
pub const MAX_ERROR: usize = 4 << 10;
/// `tests-launch`'s `launch-config`, its attributes included, in bytes.
pub const MAX_LAUNCH: usize = 1 << 20;
/// `test-provider-set`'s `runner`, in characters.
pub const MAX_RUNNER: usize = 256;
/// `test-provider-set`'s `problem`.
pub const MAX_PROBLEM: usize = 4 << 10;
/// The text of a file Fenix scans (Fenix's): a larger file's tests come
/// from the listing.
pub const MAX_SCAN_TEXT: usize = 1 << 20;
/// A run's output Fenix keeps: its end.
pub const MAX_OUTPUT_KEPT: usize = 16 << 20;
/// The tests Fenix keeps of a provider in a project.
pub const MAX_TESTS_KEPT: usize = 100_000;

/// What an answer had over a limit: each said once on the provider's
/// output page.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Cuts(pub BTreeSet<&'static str>);

impl Cuts {
    fn note(&mut self, cut: bool, what: &'static str) {
        if cut {
            self.0.insert(what);
        }
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// `s` cut to at most `max` bytes, at a character's start: whether it was.
pub fn cut(s: &mut String, max: usize) -> bool {
    if s.len() <= max {
        return false;
    }
    let mut at = max;
    while !s.is_char_boundary(at) {
        at -= 1;
    }
    s.truncate(at);
    true
}

/// `s` cut to its last `max` bytes, at a character's start: whether it
/// was.
pub fn cut_end(s: &mut String, max: usize) -> bool {
    if s.len() <= max {
        return false;
    }
    let mut at = s.len() - max;
    while !s.is_char_boundary(at) {
        at += 1;
    }
    s.drain(..at);
    true
}

/// `s` cut to at most `max` characters: whether it was.
pub fn cut_chars(s: &mut String, max: usize) -> bool {
    match s.char_indices().nth(max) {
        Some((at, _)) => {
            s.truncate(at);
            true
        }
        None => false,
    }
}

/// An id held to 1 KiB as the test explorer cuts it (`fenix_tests::cut_id`,
/// the plan's "Review of steps 1 and 2"): one of at most 1,024 bytes as
/// it is; a longer one its start up to 1,007 bytes, back to a
/// character's start, then `#` and the FNV-1a 64-bit hash of the whole
/// id in 16 lowercase hex digits. Whether it was cut.
pub fn cut_id(s: &mut String) -> bool {
    if s.len() <= MAX_ID {
        return false;
    }
    *s = fenix_tests::cut_id(s).into_owned();
    true
}

fn cut_id_opt(s: &mut Option<String>) -> bool {
    s.as_mut().is_some_and(cut_id)
}

fn cut_opt(s: &mut Option<String>, max: usize) -> bool {
    s.as_mut().is_some_and(|s| cut(s, max))
}

fn len(s: &Option<String>) -> usize {
    s.as_ref().map_or(0, String::len)
}

const ID: &str = "an id over 1 KiB was cut to 1 KiB, ending in a hash of the whole";
const LABEL: &str = "a label over 256 characters was cut";
const PATH: &str = "a path over 4 KiB was cut";
const SKIP: &str = "a skip's reason over 1 KiB was cut";
const DATA: &str = "a test's data over 4 KiB wasn't kept";
const MESSAGE: &str = "a message over 4 KiB was cut";
const TEXT: &str = "a text over 64 KiB was cut to its end";
const ANSWER: &str = "an answer over 16 MiB: what came after was left out";

/// A `found-test` held to its limits.
pub fn found(f: &mut FoundTest, cuts: &mut Cuts) {
    cuts.note(cut_id(&mut f.id) | cut_id_opt(&mut f.parent), ID);
    cuts.note(cut_chars(&mut f.label, MAX_LABEL), LABEL);
    cuts.note(cut_opt(&mut f.path, MAX_PATH), PATH);
    cuts.note(cut_opt(&mut f.skip, MAX_SKIP), SKIP);
    if len(&f.data) > MAX_DATA {
        f.data = None;
        cuts.note(true, DATA);
    }
}

fn found_size(f: &FoundTest) -> usize {
    f.id.len() + len(&f.parent) + f.label.len() + len(&f.path) + len(&f.skip) + len(&f.data)
}

/// `items` up to `most`, and up to `MAX_ANSWER` of `size` in all.
fn within<T>(items: &mut Vec<T>, most: usize, too_many: &'static str, size: impl Fn(&T) -> usize, cuts: &mut Cuts) {
    cuts.note(items.len() > most, too_many);
    items.truncate(most);
    let mut total = 0;
    if let Some(over) = items.iter().position(|item| {
        total += size(item);
        total > MAX_ANSWER
    }) {
        items.truncate(over);
        cuts.note(true, ANSWER);
    }
}

/// `tests-scan`'s answer held to its limits.
pub fn scanned(mut found_tests: Vec<FoundTest>) -> (Vec<FoundTest>, Cuts) {
    let mut cuts = Cuts::default();
    found_tests.truncate(found_tests.len().min(MAX_SCANNED + 1));
    for f in &mut found_tests {
        found(f, &mut cuts);
    }
    within(&mut found_tests, MAX_SCANNED, "a scan of over 10,000 tests: the rest were left out", found_size, &mut cuts);
    (found_tests, cuts)
}

fn step_size(s: &TestStep) -> usize {
    let c = &s.command;
    c.program.len() + c.args.iter().map(String::len).sum::<usize>() + len(&c.cwd) + c.env.iter().map(|(k, v)| k.len() + v.len()).sum::<usize>() + s.label.len() + s.covers.iter().map(String::len).sum::<usize>() + s.needs.iter().map(String::len).sum::<usize>()
}

/// A plan held to its limits. A step's command line isn't: one over it
/// fails when it would start.
pub fn planned(mut plan: TestPlan) -> (TestPlan, Cuts) {
    let mut cuts = Cuts::default();
    cuts.note(plan.steps.len() > MAX_STEPS, "a plan of over 256 steps: the rest were left out");
    plan.steps.truncate(MAX_STEPS);
    let mut covers = 0;
    for step in &mut plan.steps {
        cuts.note(cut_chars(&mut step.label, MAX_LABEL), LABEL);
        let room = MAX_COVERS - covers;
        cuts.note(step.covers.len() > room, "a plan covering over 10,000 ids: the rest were left out");
        step.covers.truncate(room);
        covers += step.covers.len();
        for id in &mut step.covers {
            cuts.note(cut_id(id), ID);
        }
        cuts.note(step.needs.len() > MAX_NEEDS, "a step needing over 32 files: the rest were left out");
        step.needs.truncate(MAX_NEEDS);
        // A need isn't cut: it fails its step when it would start
        // (`need`).
        let mut env = 0;
        let before = step.command.env.len();
        step.command.env.retain(|(k, v)| {
            env += k.len() + v.len();
            env <= MAX_ENV
        });
        cuts.note(step.command.env.len() < before, "a step's env over 64 KiB: the rest was left out");
    }
    within(&mut plan.steps, MAX_STEPS, "a plan of over 256 steps: the rest were left out", step_size, &mut cuts);
    (plan, cuts)
}

fn location(at: &mut Option<TestLocation>, cuts: &mut Cuts) {
    if let Some(at) = at {
        cuts.note(cut(&mut at.path, MAX_PATH), PATH);
    }
}

fn report_size(r: &TestReport) -> usize {
    let at = |a: &Option<TestLocation>| a.as_ref().map_or(0, |a| a.path.len());
    match r {
        TestReport::Found(f) => found_size(f),
        TestReport::Started(id) => id.len(),
        TestReport::Ended(e) => e.id.len() + len(&e.message) + at(&e.at) + len(&e.text),
        TestReport::Output(o) => len(&o.id) + o.text.len(),
        TestReport::Broken(b) => len(&b.id) + b.message.len() + at(&b.at),
        TestReport::Crashed(c) => len(&c.id) + c.message.len(),
    }
}

/// A report held to its limits.
pub fn report(r: &mut TestReport, cuts: &mut Cuts) {
    match r {
        TestReport::Found(f) => found(f, cuts),
        TestReport::Started(id) => cuts.note(cut_id(id), ID),
        TestReport::Ended(TestEnded { id, duration_ms, message, at, text, .. }) => {
            cuts.note(cut_id(id), ID);
            // A time is a time: not NaN, endless or before it started.
            *duration_ms = duration_ms.filter(|ms| ms.is_finite() && *ms >= 0.0);
            cuts.note(cut_opt(message, MAX_MESSAGE), MESSAGE);
            location(at, cuts);
            cuts.note(text.as_mut().is_some_and(|t| cut_end(t, MAX_TEXT)), TEXT);
        }
        TestReport::Output(o) => {
            cuts.note(cut_id_opt(&mut o.id), ID);
            cuts.note(cut_end(&mut o.text, MAX_TEXT), TEXT);
        }
        TestReport::Broken(b) => {
            cuts.note(cut_id_opt(&mut b.id), ID);
            cuts.note(cut(&mut b.message, MAX_MESSAGE), MESSAGE);
            location(&mut b.at, cuts);
        }
        TestReport::Crashed(c) => {
            cuts.note(cut_id_opt(&mut c.id), ID);
            cuts.note(cut(&mut c.message, MAX_MESSAGE), MESSAGE);
        }
    }
}

/// `tests-read`'s or `tests-step-ended`'s answer held to its limits.
pub fn reports(mut reports: Vec<TestReport>) -> (Vec<TestReport>, Cuts) {
    let mut cuts = Cuts::default();
    reports.truncate(reports.len().min(MAX_REPORTS + 1));
    for r in &mut reports {
        report(r, &mut cuts);
    }
    within(&mut reports, MAX_REPORTS, "over 10,000 reports from one call: the rest were left out", report_size, &mut cuts);
    (reports, cuts)
}

/// A launch config's size, in bytes, as its limit counts it.
pub fn launch_size(l: &LaunchConfig) -> usize {
    l.name.len() + l.languages.iter().map(String::len).sum::<usize>() + len(&l.program) + l.args.iter().map(String::len).sum::<usize>() + len(&l.cwd) + l.env.iter().map(|(k, v)| k.len() + v.len()).sum::<usize>() + l.attributes.len() + len(&l.pre_launch_task)
}

/// `tests-launch`'s answer held to its limits: a launch over 1 MiB can't
/// be cut and still be one, so it's refused.
pub fn launch(launch: TestLaunch) -> Result<(TestLaunch, Cuts), String> {
    match launch {
        TestLaunch::Launch(l) if launch_size(&l) > MAX_LAUNCH => Err(format!("its launch is {} KiB: a test's launch is at most 1 MiB", launch_size(&l) >> 10)),
        TestLaunch::Launch(l) => Ok((TestLaunch::Launch(l), Cuts::default())),
        TestLaunch::BuildFirst(plan) => {
            let (plan, cuts) = planned(plan);
            Ok((TestLaunch::BuildFirst(plan), cuts))
        }
    }
}

/// One of a step's `needs` as it's checked before Fenix maps it (§6.3
/// `test-step.needs`, §6.5): a path under `/data`, with `/`, of at most
/// 4 KiB, naming a file below it. Not cut and not mended: one that
/// isn't so fails its step when it would start, with why. A `\` isn't
/// taken for a `/` here, though other calls take it: the API promises
/// `needs` only as `/data/...` paths, and a remote project's copy keeps
/// the path as given.
pub fn need(path: &str) -> Result<&str, String> {
    if path.len() > MAX_PATH {
        return Err(format!("needs a path of {} bytes; a need is at most 4 KiB", path.len()));
    }
    if path.contains('\\') {
        return Err(format!("needs {path}, which isn't a /data path with /"));
    }
    match path.strip_prefix("/data/") {
        Some(rest) if !rest.is_empty() && !rest.split('/').any(|part| part.is_empty() || part == "." || part == "..") => Ok(path),
        _ => Err(format!("needs {path}, which isn't a file's path under /data")),
    }
}

/// An error a provider gave (`tests-plan`'s, `tests-launch`'s), held to
/// its limit.
pub fn error(mut e: String) -> String {
    cut(&mut e, MAX_ERROR);
    e
}

/// `test-provider-set`'s runner and problem held to their limits.
pub fn provider(runner: &mut String, problem: &mut Option<String>) -> Cuts {
    let mut cuts = Cuts::default();
    cuts.note(cut_chars(runner, MAX_RUNNER), "a runner over 256 characters was cut");
    cuts.note(cut_opt(problem, MAX_PROBLEM), "a problem over 4 KiB was cut");
    cuts
}

#[cfg(test)]
mod checks {
    use super::*;
    use crate::host::{Command, TestCrash, TestKind, TestOutcome, TestOutput};

    fn test(id: &str) -> FoundTest {
        FoundTest { id: id.into(), parent: None, label: id.into(), kind: TestKind::Test, path: None, line: None, end_line: None, skip: None, template: false, data: None }
    }

    #[test]
    fn strings_are_cut_at_a_characters_start() {
        let mut s = "aé".repeat(3);
        assert!(cut(&mut s, 2) && s == "a", "é is two bytes: not half of it");
        let mut s = "aébc".to_string();
        assert!(cut_end(&mut s, 3) && s == "bc");
        let mut s = "ééé".to_string();
        assert!(cut_chars(&mut s, 2) && s == "éé");
        assert!(!cut_chars(&mut s, 2) && !cut(&mut s, 4) && !cut_end(&mut s, 4));
    }

    #[test]
    fn a_found_test_at_its_limits_is_kept_whole_and_over_them_cut() {
        let mut at = FoundTest { id: "i".repeat(MAX_ID), parent: Some("p".repeat(MAX_ID)), label: "é".repeat(MAX_LABEL), path: Some("f".repeat(MAX_PATH)), skip: Some("s".repeat(MAX_SKIP)), data: Some("d".repeat(MAX_DATA)), ..test("x") };
        let whole = at.clone();
        let mut cuts = Cuts::default();
        found(&mut at, &mut cuts);
        assert_eq!((at, cuts.is_empty()), (whole, true));
        let mut over = FoundTest { id: "i".repeat(MAX_ID + 1), label: "é".repeat(MAX_LABEL + 1), data: Some("d".repeat(MAX_DATA + 1)), ..test("x") };
        found(&mut over, &mut cuts);
        assert_eq!((over.id.len(), over.label.chars().count(), over.data), (MAX_ID, MAX_LABEL, None));
        assert_eq!(cuts.0.len(), 3, "{cuts:?}");
    }

    /// The id cut's test vectors, the same as `fenix-tests`' (its
    /// `tests::limits::ID_CUT_VECTORS`) and `WRITING_EXTENSIONS.md`'s: an
    /// id at most 1 KiB is as it was; a longer one its start, then `#` and
    /// the FNV-1a 64-bit hash of the whole in 16 lowercase hex digits.
    #[test]
    fn a_long_id_is_cut_to_a_prefix_and_a_hash_as_the_explorer_cuts_it() {
        let shared = format!("tests/test_x.py::test_cases[{}", "a".repeat(1_100));
        let vectors: [(String, usize, &str); 7] = [
            ("a".repeat(1_023), 1_023, ""),
            ("a".repeat(1_024), 1_024, ""),
            ("a".repeat(1_025), 1_024, "#6a424025ce84288c"),
            ("é".repeat(600), 1_023, "#0e3ffe7c6421a045"),
            (format!("a{}", "😀".repeat(300)), 1_022, "#2056db1222d21dec"),
            (format!("{shared}-one]"), 1_024, "#589a673077e12368"),
            (format!("{shared}-two]"), 1_024, "#a9074c8a277a3a36"),
        ];
        for (id, len, end) in vectors {
            let mut f = FoundTest { label: "t".into(), parent: Some(id.clone()), ..test(&id) };
            let mut cuts = Cuts::default();
            found(&mut f, &mut cuts);
            assert_eq!((f.id.len(), f.id.ends_with(end), f.parent.as_deref() == Some(f.id.as_str())), (len, true, true), "{id:.40}");
            assert_eq!(cuts.is_empty(), end.is_empty());
            if !end.is_empty() {
                assert!(id.starts_with(&f.id[..len - 17]));
            }
        }
    }

    /// A step's `needs` aren't cut or mended: one over 4 KiB, with a `\`,
    /// or not a file under `/data` fails its step, with why.
    #[test]
    fn a_need_is_a_data_path_or_its_step_fails() {
        assert_eq!(need("/data/lang-kit/tests/fenix_tests.py"), Ok("/data/lang-kit/tests/fenix_tests.py"));
        let long = format!("/data/{}", "x".repeat(MAX_PATH));
        assert!(need(&long).unwrap_err().contains("at most 4 KiB"));
        assert!(need("\\data\\x.py").unwrap_err().contains("with /"));
        assert!(need("/data/a\\b.py").is_err());
        for bad in ["/data/", "/data", "/data/../x", "/data/a//b", "/project/x", "x.py"] {
            assert!(need(bad).unwrap_err().contains("isn't a file's path under /data"), "{bad}");
        }
        let (plan, _) = planned(TestPlan { steps: vec![TestStep { command: Command { program: "x".into(), args: Vec::new(), cwd: None, env: Vec::new() }, label: "l".into(), covers: Vec::new(), needs: vec![long.clone()] }], list_after: false });
        assert_eq!(plan.steps[0].needs, [long], "not cut: it fails its step");
    }

    #[test]
    fn a_scan_keeps_ten_thousand_and_sixteen_mib() {
        let (kept, cuts) = scanned((0..MAX_SCANNED + 5).map(|i| test(&i.to_string())).collect());
        assert_eq!((kept.len(), cuts.0.len()), (MAX_SCANNED, 1));
        // 4,000 tests of 4 KiB of data and 1 KiB of id: about 20 MiB.
        let big = |i: usize| FoundTest { id: format!("{i:0>1024}"), data: Some("d".repeat(MAX_DATA)), ..test("x") };
        let (kept, cuts) = scanned((0..4000).map(big).collect());
        assert!(kept.len() < 4000 && kept.iter().map(found_size).sum::<usize>() <= MAX_ANSWER, "{}", kept.len());
        assert!(cuts.0.contains(ANSWER));
    }

    #[test]
    fn a_plan_is_held_to_its_steps_covers_needs_and_env() {
        let step = |covers: usize| TestStep {
            command: Command { program: "cargo".into(), args: vec!["test".into()], cwd: None, env: vec![("A".into(), "a".repeat(MAX_ENV)), ("B".into(), "b".into())] },
            label: "l".repeat(MAX_LABEL + 1),
            covers: (0..covers).map(|i| i.to_string()).collect(),
            needs: (0..MAX_NEEDS + 1).map(|i| format!("/data/{i}")).collect(),
        };
        let (plan, cuts) = planned(TestPlan { steps: vec![step(6000), step(6000), step(1)], list_after: true });
        assert_eq!(plan.steps.iter().map(|s| s.covers.len()).collect::<Vec<_>>(), [6000, 4000, 0]);
        assert!(plan.steps.iter().all(|s| s.needs.len() == MAX_NEEDS && s.label.len() == MAX_LABEL && s.command.env.is_empty()), "A's value alone is 64 KiB: it and what follows go");
        assert!(plan.list_after);
        assert_eq!(cuts.0.len(), 4, "{cuts:?}");
        let (plan, _) = planned(TestPlan { steps: (0..MAX_STEPS + 1).map(|_| step(0)).collect(), list_after: false });
        assert_eq!(plan.steps.len(), MAX_STEPS);
    }

    #[test]
    fn reports_are_held_to_their_limits_text_by_its_end() {
        let ended = TestReport::Ended(TestEnded {
            id: "t".into(),
            outcome: TestOutcome::Failed,
            duration_ms: Some(f64::NAN),
            message: Some("m".repeat(MAX_MESSAGE + 1)),
            at: Some(TestLocation { path: "p".repeat(MAX_PATH + 1), line: 0, column: None }),
            text: Some(format!("start{}", "e".repeat(MAX_TEXT))),
        });
        let (kept, cuts) = reports(vec![ended, TestReport::Output(TestOutput { id: None, text: "o".repeat(MAX_TEXT) }), TestReport::Crashed(TestCrash { id: Some("i".repeat(MAX_ID + 1)), message: "x".into() })]);
        let TestReport::Ended(e) = &kept[0] else { panic!() };
        assert_eq!((e.message.as_ref().unwrap().len(), e.at.as_ref().unwrap().path.len()), (MAX_MESSAGE, MAX_PATH));
        assert_eq!(e.duration_ms, None, "NaN is no time");
        assert_eq!(e.text.as_deref(), Some("e".repeat(MAX_TEXT).as_str()), "its end is kept");
        assert!(matches!(&kept[1], TestReport::Output(o) if o.text.len() == MAX_TEXT), "at its limit, whole");
        assert!(matches!(&kept[2], TestReport::Crashed(c) if c.id.as_ref().unwrap().len() == MAX_ID));
        assert_eq!(cuts.0.len(), 4, "{cuts:?}");
        let (kept, _) = reports((0..MAX_REPORTS + 1).map(|i| TestReport::Started(i.to_string())).collect());
        assert_eq!(kept.len(), MAX_REPORTS);
    }

    #[test]
    fn a_launch_over_a_mebibyte_is_refused_and_a_runner_cut() {
        let config = |attributes: String| LaunchConfig { name: "t".into(), languages: vec!["rust".into()], request: crate::host::LaunchRequest::Launch, program: None, args: Vec::new(), cwd: None, env: Vec::new(), attributes, pre_launch_task: None };
        assert!(launch(TestLaunch::Launch(config("{}".into()))).is_ok());
        assert!(launch(TestLaunch::Launch(config(format!("{{\"x\":\"{}\"}}", "a".repeat(MAX_LAUNCH))))).unwrap_err().contains("at most 1 MiB"));
        let (mut runner, mut problem) = ("r".repeat(MAX_RUNNER + 1), Some("p".repeat(MAX_PROBLEM + 1)));
        assert_eq!(provider(&mut runner, &mut problem).0.len(), 2);
        assert_eq!((runner.len(), problem.unwrap().len()), (MAX_RUNNER, MAX_PROBLEM));
        assert_eq!(error("e".repeat(MAX_ERROR * 2)).len(), MAX_ERROR);
    }
}
