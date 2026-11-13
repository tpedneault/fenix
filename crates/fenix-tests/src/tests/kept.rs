//! What's kept across a restart (§4 "What's kept across a restart").

use super::*;

const FAILS: &str = "orbit/lib tests::fails_on_purpose";

/// §4: results, not tests, round trip through the results file: state,
/// duration, when, message, place and failure text.
#[test]
fn results_round_trip_through_the_results_file() {
    let mut m = orbit();
    let mut failed = Ended::new(FAILS, Outcome::Failed);
    failed.duration_ms = Some(1.5);
    failed.message = Some("assertion failed".into());
    failed.at = Some(Location { path: "src/lib.rs".into(), line: 48, column: Some(5) });
    failed.text = Some("thread 'tests::fails_on_purpose' panicked".into());
    run(&mut m, Selection::Nodes(vec![key(RUST, FAILS)]), plan(&[("one", &[])]), vec![Report::Output { id: Some(FAILS.into()), text: "printed\n".into() }, Report::Ended(failed)], StepEnd::Exited(Some(101)));
    let before = m.node(id(&m, RUST, FAILS)).unwrap().result().unwrap().clone();
    assert_eq!(before.printed, "printed\n");

    let mut after = orbit();
    assert_eq!(after.restore_results(&m.results_file()), Ok(1));
    let got = after.node(id(&after, RUST, FAILS)).unwrap().result().unwrap().clone();
    assert_eq!((got.outcome, got.duration_ms, got.when_ms, &got.message, &got.at, &got.text), (before.outcome, before.duration_ms, before.when_ms, &before.message, &before.at, &before.text));
    assert!(got.restored);
    assert_eq!(got.printed, "", "output isn't kept");
}

/// §4: the file is read when the first provider starts, before its scans
/// come: a result waits for its test, and is written again meanwhile.
#[test]
fn a_result_waits_for_its_test_to_be_found() {
    let mut m = orbit();
    run(&mut m, Selection::All, plan(&[("all", &[])]), vec![ended(FAILS, Outcome::Failed)], StepEnd::Exited(Some(101)));
    let file = m.results_file();
    let mut after = Model::new();
    after.restore_results(&file).unwrap();
    assert!(after.results_file().contains("fails_on_purpose"), "kept until found");
    after.add_provider(RUST, "cargo test");
    after.scan(RUST, "src/lib.rs", lib_rs());
    assert_eq!(state(&after, RUST, FAILS), State::Failed);
    assert!(after.node(id(&after, RUST, FAILS)).unwrap().stale());
}

/// §4: a failure's text is kept cut to 4 KiB, its end.
#[test]
fn a_failures_text_is_kept_by_its_last_4_kib() {
    let mut m = orbit();
    let mut failed = Ended::new(FAILS, Outcome::Failed);
    failed.text = Some(format!("{}the end", "x".repeat(10_000)));
    run(&mut m, Selection::Nodes(vec![key(RUST, FAILS)]), plan(&[("one", &[])]), vec![Report::Ended(failed)], StepEnd::Exited(Some(101)));
    let mut after = orbit();
    after.restore_results(&m.results_file()).unwrap();
    let text = after.node(id(&after, RUST, FAILS)).unwrap().result().unwrap().text.clone().unwrap();
    assert_eq!(text.len(), 4096);
    assert!(text.ends_with("the end"));
}

/// §4: the file is at most 8 MiB; past it the oldest results are left out.
#[test]
fn the_results_file_stays_within_8_mib() {
    let mut m = Model::new();
    m.add_provider(PY, "pytest");
    let found: Vec<Found> = (0..3_000).map(|i| Found::new(format!("t{i}"), None, format!("t{i}"), Kind::Test)).collect();
    m.scan(PY, "tests/test_big.py", found);
    let reports = (0..3_000)
        .map(|i| {
            let mut e = Ended::new(&format!("t{i}"), Outcome::Failed);
            e.text = Some("y".repeat(4_000));
            Report::Ended(e)
        })
        .collect();
    run(&mut m, Selection::All, plan(&[("all", &[])]), reports, StepEnd::Exited(Some(1)));
    let file = m.results_file();
    assert!(file.len() <= RESULTS_FILE_BYTES);
    let mut after = Model::new();
    let read = after.restore_results(&file).unwrap();
    assert!(read > 1_000 && read < 3_000, "{read}");
}

/// §4: a file that isn't a results file, or is too large, is refused and
/// changes nothing.
#[test]
fn a_bad_results_file_is_refused() {
    let mut m = orbit();
    assert!(matches!(m.restore_results("{\"not\": 1}"), Err(ResultsFileError::Unreadable(_))));
    assert!(matches!(m.restore_results("{\"version\":9,\"results\":[]}"), Err(ResultsFileError::Unreadable(_))));
    let big = " ".repeat(RESULTS_FILE_BYTES + 1);
    assert_eq!(m.restore_results(&big), Err(ResultsFileError::TooLarge(RESULTS_FILE_BYTES + 1)));
    assert_eq!(state(&m, RUST, FAILS), State::Unknown);
}

/// §4: a test that has a result already keeps it over the file's.
#[test]
fn a_restored_result_never_replaces_a_newer_one() {
    let mut m = orbit();
    run(&mut m, Selection::All, plan(&[("all", &[])]), vec![ended(FAILS, Outcome::Failed)], StepEnd::Exited(Some(101)));
    let file = m.results_file();
    run(&mut m, Selection::Nodes(vec![key(RUST, FAILS)]), plan(&[("one", &[])]), vec![ended(FAILS, Outcome::Passed)], StepEnd::Exited(Some(0)));
    m.restore_results(&file).unwrap();
    assert_eq!(state(&m, RUST, FAILS), State::Passed);
    assert!(!m.node(id(&m, RUST, FAILS)).unwrap().stale());
}

/// "Review of steps 1 and 2", Step 3 items: the file keys a result by
/// its provider's extension too (version 2); a file of version 1, which
/// didn't, is refused, not misread.
#[test]
fn a_results_file_of_version_1_is_refused() {
    let mut m = orbit();
    let old = "{\"version\":1,\"results\":[{\"provider\":\"rust\",\"id\":\"orbit/lib tests::fails_on_purpose\",\"outcome\":\"failed\",\"when_ms\":1}]}";
    assert_eq!(m.restore_results(old), Err(ResultsFileError::Unreadable("its version is 1".into())));
    assert_eq!(state(&m, RUST, FAILS), State::Unknown);
    run(&mut m, Selection::All, plan(&[("all", &[])]), vec![ended(FAILS, Outcome::Failed)], StepEnd::Exited(Some(101)));
    let file = m.results_file();
    assert!(file.starts_with("{\"version\":2,") && file.contains("\"extension\":\"fenix.rust\""), "{file}");
}
