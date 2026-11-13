//! What a provider gives the model and is given by it: the model's own
//! copies of the extension API's test records (plan §6.3, and §6.6 for
//! their Rust names), field for field, so the host turns one into the
//! other without a choice to make. The model never sees the API's
//! types; every string in them is the provider's, so untrusted, and is
//! held to §6.5's sizes where it comes in.

use serde::{Deserialize, Serialize};

/// A test, or a group of them, as a provider found it: from a file's
/// text (`tests-scan`), its runner's listing, or a run
/// (`test-report.found`). The API's `found-test` (§6.3), `tests::Found`
/// (§6.6). The model also gives nodes back as these, merged (§4), in a
/// `run` ask.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    /// The provider's own name for it, the same in what it scans, lists
    /// and reads from a run: `orbit/lib tests::adds`,
    /// `tests/test_basic.py::test_cases[negative]`. Over 1 KiB, cut as
    /// `cut_id` cuts it (Decisions, 33).
    pub id: String,
    /// The node it's in, by id: none for one at its provider's top.
    pub parent: Option<String>,
    /// What the page calls it. Cut at 256 characters.
    pub label: String,
    pub kind: Kind,
    /// Its file, as the host has mapped it: relative to the project,
    /// with `/` (Decisions, 1). In a scan's answer, none is the file
    /// scanned, for anything but a group.
    pub path: Option<String>,
    /// Its first line, from 0, and its last when known.
    pub line: Option<u32>,
    pub end_line: Option<u32>,
    /// Left out by its source: why, or "" when it doesn't say.
    pub skip: Option<String>,
    /// Its cases are found only by running it.
    pub template: bool,
    /// The provider's own, never shown or matched on, given back as it
    /// was in a `run` ask (§6.3, F6). Over 4 KiB, not kept (Decisions,
    /// 26); none, too, past the provider's 16 MiB or when no source that
    /// has the node now gave it (Decisions, 32).
    pub data: Option<String>,
}

impl Found {
    /// A node with only what every node has; the rest left out.
    pub fn new(id: impl Into<String>, parent: Option<&str>, label: impl Into<String>, kind: Kind) -> Self {
        Found { id: id.into(), parent: parent.map(str::to_string), label: label.into(), kind, path: None, line: None, end_line: None, skip: None, template: false, data: None }
    }

    pub fn at(mut self, path: &str, line: u32, end_line: Option<u32>) -> Self {
        self.path = Some(path.to_string());
        self.line = Some(line);
        self.end_line = end_line;
        self
    }
}

/// What a node of the tree is (`test-kind`, §4's table).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Kind {
    /// A Cargo target, a Go package, a Jest project: runs all under it.
    Group,
    /// A file: runs everything in it.
    File,
    /// A Rust `mod tests`, a pytest class, a `describe`.
    Suite,
    Test,
    /// A case of a template: `test_cases[negative]`, `TestTable/negative_one`.
    Case,
}

impl Kind {
    /// A test or a case: what's run, and what's counted.
    pub fn is_test(self) -> bool {
        matches!(self, Kind::Test | Kind::Case)
    }
}

/// What the model asks a provider for (`test-ask`, §6.3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ask {
    /// Its runner's listing of every test it can find in the project.
    Discover,
    /// These nodes as the model has them now, each `file` or `suite`
    /// followed by every node under it while the ask stays within 10,000
    /// records (§4, "What the provider is asked"; Decisions, 20 and 32).
    Run(Vec<Found>),
    /// Every test.
    RunAll,
}

/// What runs, in order (`test-plan`, §6.3), as far as the model needs
/// it. Each step's command, environment and `needs` stay with the host,
/// which runs them; the model needs only what a step claims.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Plan {
    pub steps: Vec<Step>,
    /// Its listing costs little once this plan has run, so the model
    /// asks for one after it (§7.1, F7).
    pub list_after: bool,
}

/// One step of a plan (`test-step`, §6.3), without its command and
/// `needs`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Step {
    /// What the page says while it runs. Cut at 256 characters.
    pub label: String,
    /// The ids it runs: a group's, a file's or a suite's stands for
    /// everything under it. Empty: it claims nothing of its own (§8.5, F1).
    pub covers: Vec<String>,
}

impl Step {
    pub fn new(label: &str, covers: &[&str]) -> Self {
        Step { label: label.to_string(), covers: covers.iter().map(|c| c.to_string()).collect() }
    }
}

/// How a step ended (`test-step-end`, §6.3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StepEnd {
    /// Its program ended: its exit code, none for a signal.
    Exited(Option<i32>),
    /// Stopped, or never started because the run was stopped before it.
    Stopped,
    /// It ran past `tests.timeout_minutes`, and was stopped.
    TimedOut,
    /// It couldn't start, or its output couldn't be read: why.
    Failed(String),
}

/// How a test ended (`test-outcome`, §6.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
    Passed,
    Failed,
    Skipped,
    Errored,
}

/// A place in a file (`test-location`, §6.3): from 0, columns in
/// characters. Its path as the host has mapped it; a place outside the
/// project keeps the machine's full path, and the host tells the two
/// apart (§6.4, "Paths").
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Location {
    pub path: String,
    pub line: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column: Option<u32>,
}

/// A test that ended (`test-ended`, §6.3).
#[derive(Clone, Debug, PartialEq)]
pub struct Ended {
    pub id: String,
    pub outcome: Outcome,
    pub duration_ms: Option<f64>,
    /// One line: a failure's message, a skip's reason, why it errored.
    pub message: Option<String>,
    /// Where it failed.
    pub at: Option<Location>,
    /// All of it: the diff, the traceback.
    pub text: Option<String>,
}

impl Ended {
    pub fn new(id: &str, outcome: Outcome) -> Self {
        Ended { id: id.to_string(), outcome, duration_ms: None, message: None, at: None, text: None }
    }
}

/// What a provider read in its runner's output (`test-report`, §6.3),
/// applied in order.
#[derive(Clone, Debug, PartialEq)]
pub enum Report {
    /// A test or group: from a listing, or found in a run.
    Found(Found),
    /// A test started, by id.
    Started(String),
    Ended(Ended),
    /// What a test printed, or the run did (`id` none).
    Output { id: Option<String>, text: String },
    /// What didn't build or load (`test-broken`): none for all the step covers.
    Broken { id: Option<String>, message: String, at: Option<Location> },
    /// A process died (`test-crash`): the group whose process it was,
    /// none for all the step covers.
    Crashed { id: Option<String>, message: String },
}
