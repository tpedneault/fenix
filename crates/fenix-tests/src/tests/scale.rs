//! §7.5, §14: 10,000 tests, timed. The bars here are loose enough for a
//! debug build on a slow machine; the times are printed
//! (`cargo test -p fenix-tests --release ten_thousand -- --nocapture`).

use super::*;
use std::time::{Duration, Instant};

fn timed<T>(what: &str, bar: Duration, f: impl FnOnce() -> T) -> T {
    let start = Instant::now();
    let out = f();
    let took = start.elapsed();
    println!("{what}: {took:?}");
    assert!(took < bar, "{what} took {took:?}, over {bar:?}");
    out
}

/// §7.5: 10,000 tests in 500 files are scanned, run with results in
/// batches of 512, filtered, laid out and marked, each well within a
/// frame's budget where it's per frame.
#[test]
fn ten_thousand_tests_scanned_run_filtered_and_marked() {
    let mut m = Model::new();
    m.add_provider(RUST, "cargo test");
    let files: Vec<(String, Vec<Found>)> = (0..500)
        .map(|f| {
            let path = format!("src/m{f}.rs");
            let suite = format!("orbit/lib m{f}::tests");
            let mut found = vec![group(LIB, "orbit (lib)"), suite_of(&suite, &path)];
            found.extend((0..20).map(|t| test(&format!("{suite}::case_{t}"), &suite, &format!("case_{t}"), 10 + t * 5, Some(13 + t * 5))));
            (path, found)
        })
        .collect();
    timed("scan 500 files, 50 a batch", Duration::from_secs(2), || {
        for batch in files.chunks(50) {
            m.scan_files(RUST, batch.to_vec());
        }
    });
    assert_eq!(m.summary().counts.tests(), 10_000);
    timed("rescan one file as it was", Duration::from_millis(50), || m.scan(RUST, &files[7].0, files[7].1.clone()));
    let mut typed = files[7].1.clone();
    typed.push(test("orbit/lib m7::tests::typed", "orbit/lib m7::tests", "typed", 150, Some(152)));
    timed("rescan one file with a new test", Duration::from_millis(200), || m.scan(RUST, &files[7].0, typed));
    m.scan(RUST, &files[7].0, files[7].1.clone());

    m.request(Selection::All, Mode::Queue);
    let Some(Action::Plan { run, .. }) = m.next(0) else { panic!() };
    m.planned(run, Ok(plan(&[("all", &[])])));
    m.next(0);
    let reports: Vec<Report> = files.iter().flat_map(|(_, found)| found[2..].iter().map(|f| ended(&f.id, if f.id.ends_with("_7") { Outcome::Failed } else { Outcome::Passed }))).collect();
    timed("10,000 results in batches of 512", Duration::from_secs(5), || {
        for batch in reports.chunks(512) {
            m.reports(run, 0, batch.to_vec());
        }
        m.step_ended(run, 0, StepEnd::Exited(Some(101)), Vec::new());
        while m.next(0).is_some() {}
    });
    assert_eq!(m.summary().counts.failed, 500);

    let lib = id(&m, RUST, LIB);
    timed("lay out the open lib", Duration::from_millis(100), || m.rows(&RowQuery { tab: Tab::All, filter: "", open: &|i| i == Item::Node(lib) }));
    let rows = timed("filter 10,000", Duration::from_millis(200), || m.rows(&RowQuery { tab: Tab::All, filter: "m42:: case_1", open: &|_| false }));
    assert_eq!(rows.len(), 2 + 11, "the group, the suite, case_1 and case_10..19");
    timed("the failed tab", Duration::from_millis(200), || m.rows(&RowQuery { tab: Tab::Failed, filter: "", open: &|_| true }));
    let marks = timed("marks for 60 lines", Duration::from_millis(5), || m.marks("src/m250.rs", 20..=80));
    assert_eq!(marks.len(), 13);
    timed("the test at a line", Duration::from_millis(5), || m.test_at("src/m250.rs", 47));
    timed("the head", Duration::from_millis(50), || m.summary());
    let file = timed("the results file", Duration::from_secs(2), || m.results_file());
    timed("read it back", Duration::from_secs(2), || Model::new().restore_results(&file).unwrap());
}

fn suite_of(id: &str, path: &str) -> Found {
    suite(id, LIB, "tests", path, 1, 200)
}

/// "Review of steps 1 and 2", finding 5 (§4 "Ids", 3; §7.5): 10,000
/// results for ids no source gave (macro-made tests), in batches of 512,
/// with and without `at`, cost a build a batch, not one a result, and
/// "the deepest node of the file" is found in the provider's index of
/// files, not by a walk of every id.
#[test]
fn ten_thousand_unknown_ids_cost_a_build_a_batch() {
    let mut m = Model::new();
    m.add_provider(RUST, "cargo test");
    let files: Vec<(String, Vec<Found>)> = (0..500)
        .map(|f| {
            let path = format!("src/m{f}.rs");
            let suite = format!("orbit/lib m{f}::tests");
            let mut found = vec![group(LIB, "orbit (lib)"), suite_of(&suite, &path)];
            found.extend((0..20).map(|t| test(&format!("{suite}::case_{t}"), &suite, &format!("case_{t}"), 10 + t * 5, Some(13 + t * 5))));
            (path, found)
        })
        .collect();
    for batch in files.chunks(50) {
        m.scan_files(RUST, batch.to_vec());
    }
    m.request(Selection::All, Mode::Queue);
    let Some(Action::Plan { run, .. }) = m.next(0) else { panic!() };
    m.planned(run, Ok(plan(&[("all", &[])])));
    m.next(0);
    let plain: Vec<Report> = (0..10_000).map(|i| ended(&format!("orbit/lib macro_{i}"), Outcome::Passed)).collect();
    timed("10,000 unknown ids, batches of 512", Duration::from_secs(5), || {
        for batch in plain.chunks(512) {
            m.reports(run, 0, batch.to_vec());
        }
    });
    let placed: Vec<Report> = (0..10_000)
        .map(|i| {
            let mut e = Ended::new(&format!("orbit/lib m{}::tests::made_{i}", i % 500), Outcome::Passed);
            e.at = Some(Location { path: format!("src/m{}.rs", i % 500), line: 50, column: None });
            Report::Ended(e)
        })
        .collect();
    timed("10,000 more, each with `at`", Duration::from_secs(5), || {
        for batch in placed.chunks(512) {
            m.reports(run, 0, batch.to_vec());
        }
    });
    m.step_ended(run, 0, StepEnd::Exited(Some(0)), Vec::new());
    while m.next(0).is_some() {}
    let made = m.node(id(&m, RUST, "orbit/lib m7::tests::made_7")).unwrap();
    assert_eq!(made.parent(), Some(id(&m, RUST, "orbit/lib m7::tests")));
    assert_eq!(m.summary().counts.passed, 20_000);
}

/// "Review of steps 1 and 2", finding 6 (§9.4): the test the cursor is
/// in, among 5,000 tests of one file with no end line, costs one pass
/// over them, on every cursor move.
#[test]
fn the_test_at_a_line_among_5000_without_end_lines() {
    let mut m = Model::new();
    m.add_provider(RUST, "cargo test");
    let mut found = vec![group(LIB, "orbit (lib)")];
    found.extend((0..5_000).map(|i| test(&format!("orbit/lib t{i}"), LIB, &format!("t{i}"), i * 3, None)));
    m.scan(RUST, "src/big.rs", found);
    let at = timed("test_at, 5,000 tests with no end line", Duration::from_millis(20), || m.test_at("src/big.rs", 14_998));
    assert_eq!(at, Some(id(&m, RUST, "orbit/lib t4999")));
    assert_eq!(m.test_at("src/big.rs", 7), Some(id(&m, RUST, "orbit/lib t2")));
}

/// "Review of steps 1 and 2", Step 3 items (§8.5): 10,000 `broken`
/// reports in one call, each naming a file's suite, cost what's under
/// each, not the run each.
#[test]
fn ten_thousand_broken_reports_in_one_call() {
    let mut m = Model::new();
    m.add_provider(RUST, "cargo test");
    let files: Vec<(String, Vec<Found>)> = (0..500)
        .map(|f| {
            let path = format!("src/m{f}.rs");
            let suite = format!("orbit/lib m{f}::tests");
            let mut found = vec![group(LIB, "orbit (lib)"), suite_of(&suite, &path)];
            found.extend((0..20).map(|t| test(&format!("{suite}::case_{t}"), &suite, &format!("case_{t}"), 10 + t * 5, Some(13 + t * 5))));
            (path, found)
        })
        .collect();
    m.scan_files(RUST, files);
    m.request(Selection::All, Mode::Queue);
    let Some(Action::Plan { run, .. }) = m.next(0) else { panic!() };
    m.planned(run, Ok(plan(&[("all", &[])])));
    m.next(0);
    let broken: Vec<Report> = (0..10_000).map(|i| Report::Broken { id: if i % 2 == 0 { Some(format!("orbit/lib m{}::tests", i % 500)) } else { None }, message: "didn't build".into(), at: None }).collect();
    timed("10,000 broken in one call", Duration::from_secs(2), || m.reports(run, 0, broken));
    assert_eq!(m.summary().counts.errored, 10_000);
}
