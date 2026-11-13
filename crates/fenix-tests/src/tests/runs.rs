//! Runs: matching reports, the last `ended`, what a step covers, crashes,
//! stop, timeout, unsaved files, the queue, listings and builds (§4
//! "Ids, and matching", "A result", "Runs"; §8; §11).

use super::*;

const ADDS: &str = "orbit/lib tests::adds";
const NEG: &str = "orbit/lib tests::case_negative";
const FAILS: &str = "orbit/lib tests::fails_on_purpose";
const DIST: &str = "orbit/lib geo::tests::dist_is_symmetric";
const NONE: &str = "orbit/lib geo::tests::unwraps_none";

fn one_step() -> Plan {
    plan(&[("orbit (lib)", &[])])
}

/// §4 "Ids", 1: a report for an id the model has is that node's result.
#[test]
fn an_ended_for_a_known_id_is_its_result() {
    let mut m = orbit();
    let mut failed = Ended::new(FAILS, Outcome::Failed);
    failed.message = Some("assertion `left == right` failed".into());
    failed.at = Some(Location { path: "src/lib.rs".into(), line: 48, column: Some(5) });
    failed.duration_ms = Some(0.3);
    let seen = run(&mut m, Selection::All, one_step(), vec![ended(ADDS, Outcome::Passed), Report::Ended(failed)], StepEnd::Exited(Some(101)));
    assert_eq!(state(&m, RUST, ADDS), State::Passed);
    assert_eq!(state(&m, RUST, FAILS), State::Failed);
    let end = &seen.ends[0];
    assert_eq!((end.passed, end.failed), (2 - 1, 1));
    assert_eq!(m.node(id(&m, RUST, FAILS)).unwrap().result().unwrap().duration_ms, Some(0.3));
}

/// §4 "Ids", 2: Go's subtests exist only once run; a `found` with its
/// test as parent comes before each result, and the model keeps it.
#[test]
fn go_subtests_found_in_a_run_go_under_their_test() {
    let mut m = Model::new();
    m.add_provider(GO, "go test");
    let mut table = Found::new("example.com/orbit TestTable", Some("\u{1f}example.com/orbit"), "TestTable", Kind::Test).at("orbit_test.go", 20, Some(40));
    table.template = true;
    m.scan(GO, "orbit_test.go", vec![group("\u{1f}example.com/orbit", "example.com/orbit"), table]);
    let sub = Found::new("example.com/orbit TestTable/negative_one", Some("example.com/orbit TestTable"), "negative_one", Kind::Case);
    let seen = run(
        &mut m,
        Selection::Nodes(vec![key(GO, "example.com/orbit TestTable")]),
        plan(&[("example.com/orbit", &["\u{1f}example.com/orbit"])]),
        vec![Report::Found(sub), ended("example.com/orbit TestTable/negative_one", Outcome::Failed), ended("example.com/orbit TestTable", Outcome::Failed)],
        StepEnd::Exited(Some(1)),
    );
    let case = m.node(id(&m, GO, "example.com/orbit TestTable/negative_one")).unwrap();
    assert!(case.found_in_run());
    assert_eq!(case.file(), Some("orbit_test.go"));
    assert_eq!(case.state(), State::Failed);
    assert_eq!(state(&m, GO, "example.com/orbit TestTable"), State::Failed);
    // The template's cases are counted, not the template.
    assert_eq!(m.summary().counts.tests(), 1);
    assert_eq!(seen.ends[0].failed, 2);
}

/// §4 "Ids", 3: an `ended` for an id no source gave (a macro-made Rust
/// test) adds a test labelled by its id, found in a run, with no line,
/// under the deepest node of the file its `at` names.
#[test]
fn a_macro_made_test_is_found_in_its_run_with_no_line() {
    let mut m = orbit();
    let mut made = Ended::new("orbit/lib tests::made_by_macro", Outcome::Failed);
    made.at = Some(Location { path: "src/lib.rs".into(), line: 58, column: None });
    run(&mut m, Selection::All, one_step(), vec![Report::Ended(made), ended("orbit/lib from_nowhere", Outcome::Passed)], StepEnd::Exited(Some(101)));
    let node = m.node(id(&m, RUST, "orbit/lib tests::made_by_macro")).unwrap();
    assert!(node.found_in_run());
    assert_eq!(node.found().label, "orbit/lib tests::made_by_macro");
    assert_eq!(node.found().line, None);
    assert_eq!(node.parent(), Some(id(&m, RUST, "orbit/lib tests")));
    let rust = m.providers()[0].provider;
    assert!(m.roots(rust).contains(&id(&m, RUST, "orbit/lib from_nowhere")), "no place: at the provider's top");
}

/// §4 "Ids", 3, R8: a `started` or an `output` for an id the model
/// doesn't know adds nothing.
#[test]
fn started_or_output_for_an_unknown_id_adds_nothing() {
    let mut m = orbit();
    run(&mut m, Selection::All, one_step(), vec![Report::Started("orbit/lib nobody".into()), Report::Output { id: Some("orbit/lib nobody".into()), text: "hi".into() }], StepEnd::Exited(Some(0)));
    assert!(!has(&m, RUST, "orbit/lib nobody"));
}

/// §4 "Ids": Fenix never guesses by name. A case of a template is the
/// provider's to recognise, reported as a `found` with the template as
/// parent before its result.
#[test]
fn parametrised_cases_go_under_the_template_their_provider_names() {
    let mut m = Model::new();
    m.add_provider(PY, "pytest");
    let mut template = Found::new("tests/test_basic.py::test_cases", None, "test_cases", Kind::Test).at("tests/test_basic.py", 24, Some(27));
    template.template = true;
    m.scan(PY, "tests/test_basic.py", vec![template]);
    let case = |name: &str| Found::new(format!("tests/test_basic.py::test_cases[{name}]"), Some("tests/test_basic.py::test_cases"), name, Kind::Case);
    run(
        &mut m,
        Selection::Nodes(vec![key(PY, "tests/test_basic.py::test_cases")]),
        plan(&[("pytest", &[])]),
        vec![
            Report::Found(case("small")),
            ended("tests/test_basic.py::test_cases[small]", Outcome::Passed),
            Report::Found(case("strings [odd] id")),
            ended("tests/test_basic.py::test_cases[strings [odd] id]", Outcome::Passed),
            // Not recognised by the provider: a test of its own, not a case.
            ended("tests/test_basic.py::test_cases[negative]", Outcome::Failed),
        ],
        StepEnd::Exited(Some(1)),
    );
    let template = m.node(id(&m, PY, "tests/test_basic.py::test_cases")).unwrap();
    assert_eq!(template.children().len(), 2);
    assert_eq!(template.state(), State::Passed);
    assert!(!template.children().contains(&id(&m, PY, "tests/test_basic.py::test_cases[negative]")));
}

/// §4 "What's dropped": a run's record goes when its template runs
/// again and the run ends complete without naming it; a run that wasn't
/// complete keeps it.
#[test]
fn a_case_goes_when_its_template_runs_again_complete_without_it() {
    let mut m = Model::new();
    m.add_provider(GO, "go test");
    let mut table = Found::new("example.com/orbit TestTable", None, "TestTable", Kind::Test).at("orbit_test.go", 20, Some(40));
    table.template = true;
    m.scan(GO, "orbit_test.go", vec![table]);
    let case = |n: &str| Report::Found(Found::new(format!("example.com/orbit TestTable/{n}"), Some("example.com/orbit TestTable"), n, Kind::Case));
    let sel = || Selection::Nodes(vec![key(GO, "example.com/orbit TestTable")]);
    run(&mut m, sel(), plan(&[("go", &[])]), vec![case("one"), case("two")], StepEnd::Exited(Some(0)));
    assert!(has(&m, GO, "example.com/orbit TestTable/two"));
    run(&mut m, sel(), plan(&[("go", &[])]), vec![case("one")], StepEnd::Stopped);
    assert!(has(&m, GO, "example.com/orbit TestTable/two"), "a stopped run drops nothing");
    run(&mut m, sel(), plan(&[("go", &[])]), vec![case("one"), Report::Crashed { id: None, message: "panic".into() }], StepEnd::Exited(Some(2)));
    assert!(has(&m, GO, "example.com/orbit TestTable/two"), "a crashed run drops nothing");
    run(&mut m, sel(), plan(&[("go", &[])]), vec![case("one")], StepEnd::Exited(Some(0)));
    assert!(!has(&m, GO, "example.com/orbit TestTable/two"));
    assert!(has(&m, GO, "example.com/orbit TestTable/one"));
}

/// §4 "A result", F5: the last `ended` for an id in a run is its result,
/// in the same step or a later one; the counts follow it. pytest's call
/// passed, then its teardown failed.
#[test]
fn the_last_ended_for_an_id_wins() {
    let mut m = orbit();
    let mut teardown = Ended::new(ADDS, Outcome::Errored);
    teardown.message = Some("error in teardown".into());
    run(&mut m, Selection::All, one_step(), vec![ended(ADDS, Outcome::Passed), Report::Output { id: Some(ADDS.into()), text: "tearing down\n".into() }, Report::Ended(teardown)], StepEnd::Exited(Some(1)));
    assert_eq!(state(&m, RUST, ADDS), State::Errored);
    let counts = m.summary().counts;
    assert_eq!((counts.passed, counts.errored), (0, 5));
    assert_eq!(m.node(id(&m, RUST, ADDS)).unwrap().result().unwrap().printed, "tearing down\n");

    // A later step's `ended` replaces a crash's errored.
    let p = plan(&[("one", &[LIB]), ("two", &[])]);
    m.request(Selection::All, Mode::Queue);
    drive(
        &mut m,
        |_, _| Ok(p.clone()),
        |_, _, step| match step {
            0 => (vec![Report::Crashed { id: None, message: "SIGABRT".into() }], StepEnd::Exited(None)),
            _ => (vec![ended(DIST, Outcome::Passed)], StepEnd::Exited(Some(0))),
        },
    );
    assert_eq!(state(&m, RUST, DIST), State::Passed);
    assert_eq!(state(&m, RUST, NONE), State::Errored);
    assert_eq!(message(&m, RUST, NONE).as_deref(), Some("SIGABRT"));
}

fn workspace() -> Model {
    // `cargo test --workspace --no-fail-fast`: two binaries, one step.
    let mut m = orbit();
    m.scan(RUST, "tests/it.rs", vec![group(IT, "orbit (test it)"), Found::new("orbit/it adds_up", Some(IT), "adds_up", Kind::Test).at("tests/it.rs", 3, Some(6))]);
    m
}

/// §8.5, F4: a crash scoped to its binary errors what's under it that
/// hasn't ended in the step; the next binary's tests keep their own
/// results.
#[test]
fn a_crash_scoped_to_its_binary_spares_the_next_binary() {
    let mut m = workspace();
    run(
        &mut m,
        Selection::All,
        plan(&[("cargo test --workspace", &[])]),
        vec![ended(ADDS, Outcome::Passed), Report::Crashed { id: Some(LIB.into()), message: "its binary crashed: SIGABRT".into() }, ended("orbit/it adds_up", Outcome::Passed)],
        StepEnd::Exited(Some(101)),
    );
    assert_eq!(state(&m, RUST, ADDS), State::Passed, "ended before the crash");
    assert_eq!(state(&m, RUST, NEG), State::Errored);
    assert_eq!(message(&m, RUST, NEG).as_deref(), Some("its binary crashed: SIGABRT"));
    assert_eq!(state(&m, RUST, "orbit/it adds_up"), State::Passed);
}

/// §8.5, F4: a crash with no id errors all the step covers that hasn't
/// ended; with an empty `covers`, the plan's rest (Decisions 11).
#[test]
fn an_unscoped_crash_errors_all_the_step_covers() {
    let mut m = workspace();
    run(&mut m, Selection::All, plan(&[("all", &[])]), vec![ended(ADDS, Outcome::Passed), Report::Crashed { id: None, message: "SIGSEGV".into() }], StepEnd::Exited(None));
    assert_eq!(state(&m, RUST, ADDS), State::Passed);
    assert_eq!(state(&m, RUST, "orbit/it adds_up"), State::Errored);
    assert_eq!(message(&m, RUST, "orbit/it adds_up").as_deref(), Some("SIGSEGV"));
}

/// §4 "Ids", 3, R8: a `crashed` naming an id the provider never found is
/// taken as naming none, and said on its output page.
#[test]
fn a_crash_naming_an_unknown_id_is_taken_as_none() {
    let mut m = workspace();
    m.take_notices();
    run(&mut m, Selection::All, plan(&[("all", &[])]), vec![Report::Crashed { id: Some("\u{1f}orbit\u{1f}nope".into()), message: "SIGABRT".into() }], StepEnd::Exited(None));
    assert_eq!(state(&m, RUST, "orbit/it adds_up"), State::Errored);
    assert!(m.take_notices().iter().any(|n| n.provider == RUST && n.text.contains("taken as all the step covers")));
}

/// §8.5: what didn't build is never a test failure: its tests are
/// errored with the message, and the run's head says the first.
#[test]
fn a_target_that_didnt_build_errors_its_tests() {
    let mut m = workspace();
    let broken = Report::Broken { id: Some(LIB.into()), message: "orbit (lib) didn't build: src/lib.rs:19 mismatched types".into(), at: Some(Location { path: "src/lib.rs".into(), line: 18, column: Some(13) }) };
    let seen = run(&mut m, Selection::All, plan(&[("all", &[])]), vec![broken, ended("orbit/it adds_up", Outcome::Passed)], StepEnd::Exited(Some(101)));
    assert_eq!(state(&m, RUST, ADDS), State::Errored);
    assert_eq!(state(&m, RUST, "orbit/it adds_up"), State::Passed);
    assert_eq!(seen.ends[0].first_broken.as_deref(), Some("orbit (lib) didn't build: src/lib.rs:19 mismatched types"));
    assert!(m.problems().is_empty(), "no problems for a broken build");
}

/// §8.5, F1: a group's id in `covers` stands for everything under it: a
/// plan of one step per target judges each target as its step ends.
#[test]
fn a_group_in_covers_judges_its_tests_at_its_steps_end() {
    let mut m = workspace();
    m.request(Selection::All, Mode::Queue);
    let p = plan(&[("orbit (lib)", &[LIB]), ("orbit (test it)", &[IT])]);
    drive(
        &mut m,
        |_, _| Ok(p.clone()),
        |m, _, step| {
            if step == 1 {
                // The lib's step ended: its tests without a result are judged.
                assert_eq!(state(m, RUST, NONE), State::Errored);
                assert_eq!(state(m, RUST, "orbit/it adds_up"), State::Running);
            }
            match step {
                0 => (vec![ended(ADDS, Outcome::Passed)], StepEnd::Exited(Some(0))),
                _ => (vec![ended("orbit/it adds_up", Outcome::Passed)], StepEnd::Exited(Some(0))),
            }
        },
    );
    assert_eq!(message(&m, RUST, NONE).as_deref(), Some("the run ended without a result for it"));
}

/// §8.5, F1: an empty `covers` claims nothing of its own. A plan whose
/// first step names nothing doesn't judge the ask at that step's end;
/// what no step names is judged when the plan's last step ends.
#[test]
fn a_first_step_with_empty_covers_judges_nothing() {
    let mut m = workspace();
    m.request(Selection::All, Mode::Queue);
    let p = plan(&[("build", &[]), ("run it", &[IT]), ("run the rest", &[])]);
    drive(
        &mut m,
        |_, _| Ok(p.clone()),
        |m, _, step| {
            if step == 1 {
                assert_eq!(state(m, RUST, ADDS), State::Running, "the rest isn't judged after the first step");
            }
            match step {
                0 => (Vec::new(), StepEnd::Exited(Some(0))),
                1 => (vec![ended("orbit/it adds_up", Outcome::Passed)], StepEnd::Exited(Some(0))),
                _ => (vec![ended(ADDS, Outcome::Passed)], StepEnd::Exited(Some(0))),
            }
        },
    );
    assert_eq!(state(&m, RUST, ADDS), State::Passed);
    assert_eq!(state(&m, RUST, NEG), State::Errored);
}

/// Decisions 10: `covers` never widen a run: a step naming the whole
/// target for a run of one test judges only that test.
#[test]
fn covers_dont_widen_the_run() {
    let mut m = orbit();
    run(&mut m, Selection::Nodes(vec![key(RUST, ADDS)]), plan(&[("orbit (lib)", &[LIB])]), Vec::new(), StepEnd::Exited(Some(0)));
    assert_eq!(state(&m, RUST, ADDS), State::Errored);
    assert_eq!(state(&m, RUST, NEG), State::Unknown);
}

/// §4 "States", §8.2: a run stopped mid-way puts its queued and running
/// tests back to what they were; what ended stands; the steps not
/// started are told `stopped`; nothing is marked failed for it.
#[test]
fn a_run_stopped_mid_way_puts_its_tests_back() {
    let mut m = workspace();
    run(&mut m, Selection::All, plan(&[("all", &[])]), vec![ended(NEG, Outcome::Failed)], StepEnd::Exited(Some(101)));
    m.request(Selection::All, Mode::Queue);
    let p = plan(&[("orbit (lib)", &[LIB]), ("orbit (test it)", &[IT])]);
    let seen = drive(
        &mut m,
        |_, _| Ok(p.clone()),
        |m, _, _| {
            assert!(m.stop("stopped"));
            // The call under way finishes and its reports stand (R5).
            (vec![ended(ADDS, Outcome::Passed)], StepEnd::Stopped)
        },
    );
    assert_eq!(seen.steps.len(), 1);
    assert_eq!(seen.not_started, vec![(RUST.to_string(), 1..2)]);
    assert_eq!(seen.ends[0].halted, Some(Halt::Stopped("stopped".into())));
    assert_eq!(state(&m, RUST, ADDS), State::Passed);
    assert_eq!(state(&m, RUST, NEG), State::Failed, "back to what it was");
    assert_eq!(state(&m, RUST, "orbit/it adds_up"), State::Errored, "back to what it was");
    assert_eq!(m.summary().counts.queued + m.summary().counts.running, 0);
}

/// §8.3, R6: a timed-out step errors its unfinished tests "timed out
/// after 30 min"; the steps after it never start, are told `stopped`,
/// and their tests are errored too (Decisions 14).
#[test]
fn a_timed_out_step_then_steps_never_started() {
    let mut m = workspace();
    m.set_timeout_minutes(30);
    m.request(Selection::All, Mode::Queue);
    let p = plan(&[("orbit (lib)", &[LIB]), ("orbit (test it)", &[IT])]);
    let seen = drive(
        &mut m,
        |_, _| Ok(p.clone()),
        |m, _, _| {
            m.time_out();
            (vec![ended(ADDS, Outcome::Passed)], StepEnd::TimedOut)
        },
    );
    assert_eq!(seen.not_started, vec![(RUST.to_string(), 1..2)]);
    assert_eq!(seen.ends[0].halted, Some(Halt::TimedOut));
    assert_eq!(state(&m, RUST, ADDS), State::Passed);
    assert_eq!(message(&m, RUST, NEG).as_deref(), Some("timed out after 30 min"));
    assert_eq!(message(&m, RUST, "orbit/it adds_up").as_deref(), Some("timed out after 30 min"));
}

/// §8.4, F18: a test of a file with unsaved edits that the run ended
/// without a result for goes back to what it was, not errored; the run
/// says which files.
#[test]
fn a_test_of_an_unsaved_file_without_a_result_goes_back() {
    let mut m = orbit();
    m.set_unsaved("src/lib.rs", true);
    let seen = run(&mut m, Selection::All, one_step(), vec![ended(DIST, Outcome::Passed)], StepEnd::Exited(Some(0)));
    assert_eq!(state(&m, RUST, ADDS), State::Unknown);
    assert_eq!(state(&m, RUST, NONE), State::Errored);
    assert_eq!(seen.ends[0].unsaved, ["src/lib.rs"]);
}

/// §6.5: a step that couldn't start errors its tests with why.
#[test]
fn a_step_that_couldnt_start_errors_its_tests_with_why() {
    let mut m = orbit();
    let why = "its command line is 41,203 characters; Windows takes 32,766";
    run(&mut m, Selection::All, one_step(), Vec::new(), StepEnd::Failed(why.into()));
    assert_eq!(message(&m, RUST, ADDS).as_deref(), Some(why));
}

/// Decisions 12: a plan that's an error errors the provider's tests in
/// the run with its message, and nothing starts.
#[test]
fn a_plan_error_errors_the_providers_tests() {
    let mut m = orbit();
    m.request(Selection::All, Mode::Queue);
    let seen = drive(&mut m, |_, _| Err("cargo isn't installed".into()), |_, _, _| unreachable!());
    assert!(seen.steps.is_empty());
    assert_eq!(message(&m, RUST, ADDS).as_deref(), Some("cargo isn't installed"));
}

/// §4 "Runs", R12: an `ended` in a listing is ignored.
#[test]
fn an_ended_in_a_listing_is_ignored() {
    let mut m = orbit();
    m.list(Some(RUST));
    drive(&mut m, |_, _| Ok(one_step()), |_, _, _| (vec![ended(ADDS, Outcome::Failed)], StepEnd::Exited(Some(0))));
    assert_eq!(state(&m, RUST, ADDS), State::Unknown);
    assert!(m.last_run().is_none(), "a listing isn't the last run");
}

/// §11, F19: a `build-first` plan runs as a run's steps; an `ended` in it
/// is ignored; it may be asked again only when every step exited 0 and
/// nothing was `broken` or `crashed`.
#[test]
fn a_build_first_plan_says_whether_it_built() {
    let mut m = orbit();
    assert_eq!(m.request_build(RUST, ADDS, plan(&[("cargo test --no-run", &[])])), Requested::Starts);
    let seen = drive(&mut m, |_, _| unreachable!(), |_, _, _| (vec![ended(ADDS, Outcome::Failed)], StepEnd::Exited(Some(0))));
    assert!(seen.ends[0].build_ok);
    assert_eq!(seen.ends[0].kind, RunKind::Build { provider: RUST.into(), test: ADDS.into() });
    assert_eq!(state(&m, RUST, ADDS), State::Unknown);

    m.request_build(RUST, ADDS, plan(&[("cargo test --no-run", &[])]));
    let seen = drive(&mut m, |_, _| unreachable!(), |_, _, _| (vec![Report::Broken { id: None, message: "didn't build".into(), at: None }], StepEnd::Exited(Some(101))));
    assert!(!seen.ends[0].build_ok);
    assert_eq!(seen.ends[0].first_broken.as_deref(), Some("didn't build"));
}

/// §8.1: a request while a run goes waits; another joins the one
/// waiting, as their union; the same run asked again while it goes does
/// nothing.
#[test]
fn a_request_while_a_run_goes_waits_and_the_next_joins_it() {
    let mut m = workspace();
    assert_eq!(m.request(Selection::File("src/lib.rs".into()), Mode::Queue), Requested::Starts);
    assert!(matches!(m.next(0), Some(Action::Plan { .. })));
    assert_eq!(m.request(Selection::File("src/lib.rs".into()), Mode::Queue), Requested::AlreadyGoing);
    assert_eq!(m.request(Selection::Nodes(vec![key(RUST, "orbit/it adds_up")]), Mode::Queue), Requested::Waits);
    assert_eq!(m.request(Selection::Nodes(vec![key(RUST, DIST)]), Mode::Queue), Requested::Joined);
    assert_eq!(m.summary().waiting, Some(2));
    assert_eq!(m.request(Selection::Nodes(vec![key(RUST, "orbit/lib nobody")]), Mode::Queue), Requested::Nothing);
    // The run going ends; the waiting one is the union.
    let RunId(first) = match m.summary().running { Some(p) => p.run, None => unreachable!() };
    m.planned(RunId(first), Ok(plan(&[])));
    let seen = drive(&mut m, |_, _| Ok(plan(&[("one", &[])])), |_, _, _| (Vec::new(), StepEnd::Exited(Some(0))));
    assert_eq!(seen.ends.len(), 2);
    match &seen.asks[0].1 {
        Ask::Run(records) => assert_eq!(records.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), [DIST, "orbit/it adds_up"]),
        other => panic!("{other:?}"),
    }
}

/// §8.4: a save's run replaces a waiting one rather than joining it.
#[test]
fn run_on_save_replaces_the_waiting_run() {
    let mut m = workspace();
    m.request(Selection::All, Mode::Queue);
    m.next(0);
    m.request(Selection::Nodes(vec![key(RUST, ADDS)]), Mode::Queue);
    assert_eq!(m.request(Selection::File("tests/it.rs".into()), Mode::ReplaceWaiting), Requested::Waits);
    assert_eq!(m.summary().waiting, Some(1));
}

/// §4 "States": a run that replaces the one going stops it, puts its
/// tests back, and goes next.
#[test]
fn a_replacing_run_stops_the_one_going() {
    let mut m = orbit();
    m.request(Selection::All, Mode::Queue);
    let Some(Action::Plan { run, .. }) = m.next(0) else { panic!() };
    m.planned(run, Ok(one_step()));
    let Some(Action::Step { step, .. }) = m.next(0) else { panic!() };
    assert_eq!(state(&m, RUST, ADDS), State::Running);
    assert_eq!(m.request(Selection::Nodes(vec![key(RUST, NEG)]), Mode::Replace), Requested::Replaces);
    m.step_ended(run, step, StepEnd::Stopped, Vec::new());
    let Some(Action::Ended(end)) = m.next(0) else { panic!() };
    assert_eq!(end.halted, Some(Halt::Stopped("replaced by another run".into())));
    assert_eq!(state(&m, RUST, ADDS), State::Unknown);
    assert!(matches!(m.next(0), Some(Action::Plan { .. })));
    assert_eq!(state(&m, RUST, NEG), State::Queued);
}

/// §7.1: a listing never runs while a run goes: it waits for it.
#[test]
fn a_listing_waits_for_the_run_going() {
    let mut m = orbit();
    m.request(Selection::All, Mode::Queue);
    assert!(matches!(m.next(0), Some(Action::Plan { ask: Ask::RunAll, .. })));
    m.list(Some(RUST));
    assert_eq!(m.next(0), None, "the plan is under way");
    let run = m.summary().running.unwrap().run;
    m.planned(run, Ok(plan(&[])));
    assert!(matches!(m.next(0), Some(Action::Ended(_))));
    assert!(matches!(m.next(0), Some(Action::Plan { ask: Ask::Discover, .. })));
}

/// §7.1, F7: after a run whose plan said `list-after`, once its steps all
/// exited and nothing was broken, the provider is listed.
#[test]
fn list_after_lists_once_the_plan_ran_whole() {
    let mut m = orbit();
    let mut p = one_step();
    p.list_after = true;
    let seen = run(&mut m, Selection::All, p.clone(), Vec::new(), StepEnd::Exited(Some(101)));
    assert!(seen.asks.iter().any(|(_, ask)| *ask == Ask::Discover));
    let seen = run(&mut m, Selection::All, p, vec![Report::Broken { id: None, message: "didn't build".into(), at: None }], StepEnd::Exited(Some(101)));
    assert!(!seen.asks.iter().any(|(_, ask)| *ask == Ask::Discover));
}

/// §4 "States", Decisions 9: a step's tests are running when it starts;
/// once its runner says when each starts, the others go back to queued.
#[test]
fn running_follows_the_runner_once_it_says() {
    let mut m = orbit();
    m.request(Selection::All, Mode::Queue);
    let Some(Action::Plan { run, .. }) = m.next(0) else { panic!() };
    assert_eq!(state(&m, RUST, ADDS), State::Queued);
    m.planned(run, Ok(one_step()));
    m.next(0);
    assert_eq!(state(&m, RUST, ADDS), State::Running);
    m.reports(run, 0, vec![Report::Started(NEG.into())]);
    assert_eq!(state(&m, RUST, ADDS), State::Queued);
    assert_eq!(state(&m, RUST, NEG), State::Running);
    assert_eq!(m.summary().counts.running, 1);
    let progress = m.summary().running.unwrap();
    assert_eq!((progress.done, progress.total), (0, 5));
}

/// §6.4, F12: a provider that stops mid-run (a trap, started again in
/// another project) stops the run: no call goes to it after, and its
/// tests go back to what they were.
#[test]
fn a_provider_that_stops_stops_its_run() {
    let mut m = orbit();
    m.request(Selection::All, Mode::Queue);
    let Some(Action::Plan { run, .. }) = m.next(0) else { panic!() };
    m.planned(run, Ok(plan(&[("a", &[]), ("b", &[])])));
    m.next(0);
    m.provider_stopped(RUST, "stopped: cargo test started again in orbit-cli");
    m.reports(run, 0, vec![ended(ADDS, Outcome::Passed)]);
    let Some(Action::Ended(end)) = m.next(0) else { panic!("no call goes to it") };
    assert_eq!(end.halted, Some(Halt::Stopped("stopped: cargo test started again in orbit-cli".into())));
    assert_eq!(state(&m, RUST, ADDS), State::Unknown);
}

/// §4 "Runs", §8.1: a run's providers go in turn, each plan whole.
#[test]
fn a_runs_providers_go_in_turn() {
    let mut m = orbit();
    m.add_provider(PY, "pytest");
    m.scan(PY, "tools/tests/test_basic.py", vec![Found::new("tests/test_basic.py::test_adds", None, "test_adds", Kind::Test).at("tools/tests/test_basic.py", 3, Some(5))]);
    let seen = run(&mut m, Selection::All, plan(&[("a", &[]), ("b", &[])]), Vec::new(), StepEnd::Exited(Some(0)));
    let order: Vec<(&str, u32)> = seen.steps.iter().map(|(p, s)| (p.as_str(), *s)).collect();
    assert_eq!(order, [(PY, 0), (PY, 1), (RUST, 0), (RUST, 1)]);
    assert_eq!(seen.asks.iter().map(|(p, _)| p.as_str()).collect::<Vec<_>>(), [PY, RUST]);
}

/// §8.4 "The failed again": the tests whose last result is failed or
/// errored, as one selection; a case asked as itself.
#[test]
fn the_failed_again_are_the_failed_and_errored() {
    let mut m = orbit();
    let mut table = Found::new("orbit/lib tests::table", Some("orbit/lib tests"), "table", Kind::Test).at("src/lib.rs", 52, Some(58));
    table.template = true;
    let mut found = lib_rs();
    found.push(table);
    m.scan(RUST, "src/lib.rs", found);
    let case = Found::new("orbit/lib tests::table::neg", Some("orbit/lib tests::table"), "neg", Kind::Case);
    run(&mut m, Selection::All, one_step(), vec![ended(ADDS, Outcome::Passed), Report::Found(case), ended("orbit/lib tests::table::neg", Outcome::Failed), ended(FAILS, Outcome::Failed), ended(NEG, Outcome::Errored)], StepEnd::Exited(Some(101)));
    let seen = run(&mut m, Selection::Failed, one_step(), Vec::new(), StepEnd::Stopped);
    let Ask::Run(records) = &seen.asks[0].1 else { panic!() };
    // The geo tests errored: the run ended without a result for them.
    assert_eq!(records.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), [DIST, NONE, NEG, FAILS, "orbit/lib tests::table::neg"]);
}

/// §8.6 / §6.7: a provider that fell behind its step's output gets the
/// step's unreported tests errored with why.
#[test]
fn a_provider_that_fell_behind_errors_its_steps_unreported_tests() {
    let mut m = orbit();
    m.request(Selection::All, Mode::Queue);
    let Some(Action::Plan { run, .. }) = m.next(0) else { panic!() };
    m.planned(run, Ok(one_step()));
    m.next(0);
    m.reports(run, 0, vec![ended(ADDS, Outcome::Passed)]);
    m.step_unread(run, 0, "its output was more than cargo test could read");
    assert_eq!(state(&m, RUST, ADDS), State::Passed);
    assert_eq!(message(&m, RUST, NEG).as_deref(), Some("its output was more than cargo test could read"));
}

/// §6.4: reports for a run that isn't going, or a step not under way,
/// are dropped.
#[test]
fn reports_for_another_run_or_step_are_dropped() {
    let mut m = orbit();
    m.request(Selection::All, Mode::Queue);
    let Some(Action::Plan { run, .. }) = m.next(0) else { panic!() };
    m.planned(run, Ok(one_step()));
    m.next(0);
    m.reports(RunId(run.0 + 7), 0, vec![ended(ADDS, Outcome::Passed)]);
    m.reports(run, 3, vec![ended(ADDS, Outcome::Passed)]);
    assert_eq!(state(&m, RUST, ADDS), State::Running);
}
