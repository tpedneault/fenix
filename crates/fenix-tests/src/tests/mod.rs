//! The model's rules, each a test named for what it does and citing the
//! plan's section. The ids and shapes are the packs' (fenix-extensions
//! `plans/test-explorer-fixtures/`): Rust's `<package>/<target> <path>`
//! with groups' ids preceded by U+001F, pytest's node ids, Go's
//! `<import path> <name>`.

use crate::*;

mod asks;
mod kept;
mod limits;
mod runs;
mod scale;
mod tree;
mod views;

pub(super) const RUST: &ProviderId = &ProviderId::fixed("fenix.rust", "rust");
pub(super) const PY: &ProviderId = &ProviderId::fixed("fenix.python", "python");
pub(super) const GO: &ProviderId = &ProviderId::fixed("fenix.go", "go");
pub(super) const WEB: &ProviderId = &ProviderId::fixed("fenix.web", "web");
pub(super) const NEXTEST: &ProviderId = &ProviderId::fixed("fenix.rust", "rust.nextest");

/// Cargo's lib target of `orbit`, as lang-kit names a group.
pub(super) const LIB: &str = "\u{1f}orbit\u{1f}orbit (lib)";
pub(super) const IT: &str = "\u{1f}orbit\u{1f}orbit (test it)";

pub(super) fn group(id: &str, label: &str) -> Found {
    Found::new(id, None, label, Kind::Group)
}

pub(super) fn suite(id: &str, parent: &str, label: &str, path: &str, line: u32, end: u32) -> Found {
    Found::new(id, Some(parent), label, Kind::Suite).at(path, line, Some(end))
}

pub(super) fn test(id: &str, parent: &str, label: &str, line: u32, end: Option<u32>) -> Found {
    let mut found = Found::new(id, Some(parent), label, Kind::Test);
    found.line = Some(line);
    found.end_line = end;
    found
}

/// `src/lib.rs` as the Rust pack scans it.
pub(super) fn lib_rs() -> Vec<Found> {
    vec![
        group(LIB, "orbit (lib)"),
        suite("orbit/lib tests", LIB, "tests", "src/lib.rs", 10, 60),
        test("orbit/lib tests::adds", "orbit/lib tests", "adds", 12, Some(15)),
        test("orbit/lib tests::case_negative", "orbit/lib tests", "case_negative", 40, Some(45)),
        test("orbit/lib tests::fails_on_purpose", "orbit/lib tests", "fails_on_purpose", 47, Some(50)),
    ]
}

/// `src/geo/mod.rs` as the Rust pack scans it.
pub(super) fn geo_rs() -> Vec<Found> {
    vec![
        group(LIB, "orbit (lib)"),
        suite("orbit/lib geo::tests", LIB, "geo::tests", "src/geo/mod.rs", 20, 40),
        test("orbit/lib geo::tests::dist_is_symmetric", "orbit/lib geo::tests", "dist_is_symmetric", 22, Some(26)),
        test("orbit/lib geo::tests::unwraps_none", "orbit/lib geo::tests", "unwraps_none", 28, Some(31)),
    ]
}

/// A Rust project of two files' scans.
pub(super) fn orbit() -> Model {
    let mut m = Model::new();
    m.add_provider(RUST, "cargo test");
    m.scan(RUST, "src/lib.rs", lib_rs());
    m.scan(RUST, "src/geo/mod.rs", geo_rs());
    m
}

pub(super) fn key(provider: &ProviderId, id: &str) -> TestKey {
    TestKey::new(provider, id)
}

pub(super) fn id(m: &Model, provider: &ProviderId, test: &str) -> NodeId {
    m.find(&key(provider, test)).unwrap_or_else(|| panic!("{provider} has {test}"))
}

pub(super) fn state(m: &Model, provider: &ProviderId, test: &str) -> State {
    m.node(id(m, provider, test)).unwrap().state()
}

pub(super) fn has(m: &Model, provider: &ProviderId, test: &str) -> bool {
    m.find(&key(provider, test)).is_some()
}

pub(super) fn message(m: &Model, provider: &ProviderId, test: &str) -> Option<String> {
    m.node(id(m, provider, test)).unwrap().result().and_then(|r| r.message.clone())
}

pub(super) fn ended(id: &str, outcome: Outcome) -> Report {
    Report::Ended(Ended::new(id, outcome))
}

pub(super) fn plan(steps: &[(&str, &[&str])]) -> Plan {
    Plan { steps: steps.iter().map(|(label, covers)| Step::new(label, covers)).collect(), list_after: false }
}

/// What a scripted host saw of a run.
#[derive(Default)]
pub(super) struct Seen {
    pub asks: Vec<(ProviderId, Ask)>,
    pub steps: Vec<(ProviderId, u32)>,
    pub not_started: Vec<(ProviderId, std::ops::Range<u32>)>,
    pub ends: Vec<RunEnd>,
}

/// Drives the model as the host does until nothing is left to do: each
/// provider's plan from `plans`, each step's reports and end from `steps`.
pub(super) fn drive(m: &mut Model, mut plans: impl FnMut(&ProviderId, &Ask) -> Result<Plan, String>, mut steps: impl FnMut(&mut Model, &ProviderId, u32) -> (Vec<Report>, StepEnd)) -> Seen {
    let mut seen = Seen::default();
    let mut now = 1_000;
    for _ in 0..10_000 {
        now += 10;
        match m.next(now) {
            None => return seen,
            Some(Action::Plan { run, provider, ask }) => {
                let plan = plans(&provider, &ask);
                seen.asks.push((provider, ask));
                m.planned(run, plan);
            }
            Some(Action::Step { run, provider, step, .. }) => {
                seen.steps.push((provider.clone(), step));
                let (reports, end) = steps(m, &provider, step);
                m.step_ended(run, step, end, reports);
            }
            Some(Action::NotStarted { provider, steps, .. }) => seen.not_started.push((provider, steps)),
            Some(Action::Ended(end)) => seen.ends.push(end),
        }
    }
    panic!("the run never ended");
}

/// Runs `selection` with one plan for every provider and one answer for
/// every step.
pub(super) fn run(m: &mut Model, selection: Selection, the_plan: Plan, reports: Vec<Report>, end: StepEnd) -> Seen {
    assert_ne!(m.request(selection, Mode::Queue), Requested::Nothing);
    let mut reports = Some(reports);
    drive(m, |_, _| Ok(the_plan.clone()), |_, _, _| (reports.take().unwrap_or_default(), end.clone()))
}
