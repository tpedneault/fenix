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
