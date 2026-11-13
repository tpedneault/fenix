//! What the page and the file views ask: states and counts, staleness,
//! rows, the filter, tabs, gutter marks, the test at a line, problems
//! (§4 "States", §7.2, §8.4, §9).

use super::*;

const ADDS: &str = "orbit/lib tests::adds";
const NEG: &str = "orbit/lib tests::case_negative";
const FAILS: &str = "orbit/lib tests::fails_on_purpose";

fn results(m: &mut Model) {
    run(m, Selection::All, plan(&[("all", &[])]), vec![ended(ADDS, Outcome::Passed), ended(NEG, Outcome::Passed), ended(FAILS, Outcome::Failed), ended("orbit/lib geo::tests::dist_is_symmetric", Outcome::Passed), ended("orbit/lib geo::tests::unwraps_none", Outcome::Skipped)], StepEnd::Exited(Some(101)));
}

fn row_labels(m: &Model, rows: &[Row]) -> Vec<String> {
    rows.iter()
        .map(|r| match r.item {
            Item::Provider(p) => format!("[{}]", m.provider(p).unwrap().id),
            Item::Node(n) => format!("{}{}", "  ".repeat(r.depth as usize), m.node(n).unwrap().found().label),
        })
        .collect()
}

/// §4 "A group's state": worked out from its children: failed if any
/// is, passed if all that ran passed, else unknown; counts kept per group
/// as results land, the same as counted afresh.
#[test]
fn a_groups_state_comes_from_its_children() {
    let mut m = orbit();
    results(&mut m);
    assert_eq!(state(&m, RUST, "orbit/lib tests"), State::Failed);
    assert_eq!(state(&m, RUST, "orbit/lib geo::tests"), State::Passed);
    let lib = m.node(id(&m, RUST, LIB)).unwrap();
    assert_eq!((lib.counts().passed, lib.counts().failed, lib.counts().skipped), (3, 1, 1));
    // Kept as results land: the same as a model that counts afresh.
    let mut fresh = orbit();
    fresh.restore_results(&m.results_file()).unwrap();
    m.file_saved("nothing.rs");
    assert_eq!(m.node(id(&m, RUST, LIB)).unwrap().counts().tests(), 5);
    assert_eq!(*m.node(id(&m, RUST, LIB)).unwrap().counts(), Counts { passed: 3, failed: 1, skipped: 1, ..Counts::default() });
}

/// §4 "States", §8.4: saving a file marks its tests stale, dimmed, their
/// marks kept; a save elsewhere marks nothing; the test running again
/// clears it.
#[test]
fn saving_a_file_marks_its_tests_stale() {
    let mut m = orbit();
    results(&mut m);
    m.file_saved("src/lib.rs");
    let adds = m.node(id(&m, RUST, ADDS)).unwrap();
    assert!(adds.stale());
    assert_eq!(adds.state(), State::Passed);
    assert!(!m.node(id(&m, RUST, "orbit/lib geo::tests::dist_is_symmetric")).unwrap().stale());
    assert!(m.node(id(&m, RUST, "orbit/lib tests")).unwrap().stale());
    assert!(!m.node(id(&m, RUST, LIB)).unwrap().stale());
    assert_eq!(m.summary().saved_since, 1);
    run(&mut m, Selection::Nodes(vec![key(RUST, ADDS)]), plan(&[("one", &[])]), vec![ended(ADDS, Outcome::Passed)], StepEnd::Exited(Some(0)));
    assert!(!m.node(id(&m, RUST, ADDS)).unwrap().stale());
}

/// §4 "What's kept", "States": results restored after a restart are
/// shown stale until the test runs again.
#[test]
fn results_restored_after_a_restart_are_stale() {
    let mut m = orbit();
    results(&mut m);
    let file = m.results_file();
    let mut after = orbit();
    after.restore_results(&file).unwrap();
    let fails = after.node(id(&after, RUST, FAILS)).unwrap();
    assert_eq!(fails.state(), State::Failed);
    assert!(fails.stale());
    assert!(after.problems().is_empty(), "a stale failure isn't a problem");
    assert_eq!(after.failures(), [id(&after, RUST, FAILS)]);
}

/// §4 "Two providers in one file", R4: a line's mark is the worst state
/// of the tests that start there, every provider's.
#[test]
fn two_providers_on_one_line_show_the_worst() {
    let mut m = orbit();
    m.add_provider("rust.nextest", "cargo nextest");
    m.scan("rust.nextest", "src/lib.rs", lib_rs());
    run(&mut m, Selection::Nodes(vec![key("rust.nextest", ADDS)]), plan(&[("one", &[])]), vec![ended(ADDS, Outcome::Failed)], StepEnd::Exited(Some(101)));
    run(&mut m, Selection::Nodes(vec![key(RUST, ADDS)]), plan(&[("one", &[])]), vec![ended(ADDS, Outcome::Passed)], StepEnd::Exited(Some(0)));
    let marks = m.marks("src/lib.rs", 12..=12);
    assert_eq!(marks.len(), 1);
    assert_eq!(marks[0].state, State::Failed);
    assert_eq!(marks[0].node, id(&m, "rust.nextest", ADDS));
}

/// §9.1, §7.5: marks are worked out for the lines asked, where tests and
/// groups start; a file no provider serves has none.
#[test]
fn marks_are_for_the_lines_in_view() {
    let mut m = orbit();
    results(&mut m);
    let marks: Vec<(u32, char)> = m.marks("src/lib.rs", 11..=47).iter().map(|k| (k.line, k.state.glyph())).collect();
    assert_eq!(marks, [(12, '✓'), (40, '✓'), (47, '✗')]);
    assert_eq!(m.marks("src/lib.rs", 0..=100)[0], Mark { line: 10, state: State::Failed, stale: false, node: id(&m, RUST, "orbit/lib tests") });
    assert!(m.serves("src/lib.rs"));
    assert!(!m.serves("src/main.rs"));
}

/// §9.4: the innermost test whose lines hold the cursor; on a group's
/// first line, the group; between tests, none.
#[test]
fn the_test_the_cursor_is_in() {
    let m = orbit();
    assert_eq!(m.test_at("src/lib.rs", 13), Some(id(&m, RUST, ADDS)));
    assert_eq!(m.test_at("src/lib.rs", 10), Some(id(&m, RUST, "orbit/lib tests")));
    assert_eq!(m.test_at("src/lib.rs", 20), None);
    assert_eq!(m.test_at("src/lib.rs", 70), None);
    assert_eq!(m.test_at("src/other.rs", 13), None);
}

/// §9.4: with no last line, the last test that starts at or above the
/// cursor within its parent's lines.
#[test]
fn without_a_last_line_the_last_test_above_the_cursor() {
    let mut m = Model::new();
    m.add_provider("web", "vitest");
    let path = "src/orbit.test.js";
    let describe = Found::new("src/orbit.test.js > math", None, "math", Kind::Suite).at(path, 2, Some(20));
    let a = Found::new("src/orbit.test.js > math > adds", Some("src/orbit.test.js > math"), "adds", Kind::Test).at(path, 3, None);
    let b = Found::new("src/orbit.test.js > math > subtracts", Some("src/orbit.test.js > math"), "subtracts", Kind::Test).at(path, 8, None);
    m.scan("web", path, vec![describe, a, b]);
    assert_eq!(m.test_at(path, 5), Some(id(&m, "web", "src/orbit.test.js > math > adds")));
    assert_eq!(m.test_at(path, 12), Some(id(&m, "web", "src/orbit.test.js > math > subtracts")));
    assert_eq!(m.test_at(path, 25), None, "past its parent's lines");
}

/// §4, §9.4, R4: with two providers in a file, the innermost of all their
/// tests; on a tie, the provider whose id sorts first.
#[test]
fn two_providers_tests_on_one_line_go_to_the_first_provider() {
    let mut m = orbit();
    m.add_provider("rust.nextest", "cargo nextest");
    m.scan("rust.nextest", "src/lib.rs", lib_rs());
    assert_eq!(m.test_at("src/lib.rs", 13), Some(id(&m, RUST, ADDS)));
    // An inner test of the second provider wins over the first's outer one.
    let mut inner = lib_rs();
    inner.push(test("orbit/lib tests::adds::inner", ADDS, "inner", 13, Some(14)));
    m.scan("rust.nextest", "src/lib.rs", inner);
    assert_eq!(m.test_at("src/lib.rs", 13), Some(id(&m, "rust.nextest", "orbit/lib tests::adds::inner")));
}

/// §7.2, §7.5: only open groups' rows are laid out; with one provider
/// there's no provider row; with two, one each.
#[test]
fn only_open_groups_rows_are_laid_out() {
    let mut m = orbit();
    let closed = m.rows(&RowQuery { tab: Tab::All, filter: "", open: &|_| false });
    assert_eq!(row_labels(&m, &closed), ["orbit (lib)"]);
    let lib = id(&m, RUST, LIB);
    let rows = m.rows(&RowQuery { tab: Tab::All, filter: "", open: &|item| item == Item::Node(lib) });
    assert_eq!(row_labels(&m, &rows), ["orbit (lib)", "  geo::tests", "  tests"]);
    m.add_provider(PY, "pytest");
    m.scan(PY, "tools/tests/test_basic.py", vec![Found::new("tests/test_basic.py::test_adds", None, "test_adds", Kind::Test)]);
    let rows = m.rows(&RowQuery { tab: Tab::All, filter: "", open: &|item| matches!(item, Item::Provider(_)) });
    assert_eq!(row_labels(&m, &rows), ["[python]", "  test_adds", "[rust]", "  orbit (lib)"]);
}

/// §7.2 "The filter": labels and ids, case-insensitive, words in any
/// order; matches show with their groups opened.
#[test]
fn the_filter_opens_the_groups_of_what_matches() {
    let m = orbit();
    let rows = m.rows(&RowQuery { tab: Tab::All, filter: "NEGATIVE tests::", open: &|_| false });
    assert_eq!(row_labels(&m, &rows), ["orbit (lib)", "  tests", "    case_negative"]);
    let rows = m.rows(&RowQuery { tab: Tab::All, filter: "geo unwraps", open: &|_| false });
    assert_eq!(row_labels(&m, &rows), ["orbit (lib)", "  geo::tests", "    unwraps_none"]);
    assert!(m.rows(&RowQuery { tab: Tab::All, filter: "nothing like it", open: &|_| true }).is_empty());
}

/// §7.2 "The tabs": Failed shows failed and errored with their groups;
/// Not run shows unknown and skipped.
#[test]
fn the_tabs_show_failed_or_not_run_with_their_groups() {
    let mut m = orbit();
    results(&mut m);
    let rows = m.rows(&RowQuery { tab: Tab::Failed, filter: "", open: &|_| true });
    assert_eq!(row_labels(&m, &rows), ["orbit (lib)", "  tests", "    fails_on_purpose"]);
    let rows = m.rows(&RowQuery { tab: Tab::NotRun, filter: "", open: &|_| true });
    assert_eq!(row_labels(&m, &rows), ["orbit (lib)", "  geo::tests", "    unwraps_none"]);
}

/// §9.3: a failed test's failure is a problem at its `at`, its code the
/// test's label; an errored test's isn't; cleared when its file is
/// saved, or while it runs again.
#[test]
fn failures_are_problems_until_saved_or_run_again() {
    let mut m = orbit();
    let mut failed = Ended::new(FAILS, Outcome::Failed);
    failed.message = Some("assertion `left == right` failed".into());
    failed.at = Some(Location { path: "src/lib.rs".into(), line: 48, column: Some(5) });
    run(&mut m, Selection::All, plan(&[("all", &[])]), vec![Report::Ended(failed), ended(ADDS, Outcome::Errored)], StepEnd::Exited(Some(101)));
    let problems = m.problems();
    assert_eq!(problems.len(), 1);
    assert_eq!(problems[0].at.as_ref().unwrap().line, 48);
    assert_eq!((problems[0].file.as_str(), problems[0].line, problems[0].label.as_str()), ("src/lib.rs", 47, "fails_on_purpose"));
    m.request(Selection::Nodes(vec![key(RUST, FAILS)]), Mode::Queue);
    m.next(0);
    assert!(m.problems().is_empty(), "running again");
    let run_id = m.summary().running.unwrap().run;
    m.stop("stopped");
    m.planned(run_id, Ok(plan(&[])));
    while m.next(0).is_some() {}
    assert_eq!(m.problems().len(), 1);
    m.file_saved("src/lib.rs");
    assert!(m.problems().is_empty());
}

/// §7.2 "The head": the counts, the run under way and its progress, the
/// one waiting, the last run.
#[test]
fn the_head_says_counts_progress_and_the_last_run() {
    let mut m = orbit();
    results(&mut m);
    let summary = m.summary();
    assert_eq!((summary.counts.passed, summary.counts.failed, summary.counts.skipped), (3, 1, 1));
    let last = summary.last.unwrap();
    assert_eq!((last.end.passed, last.end.failed, last.end.skipped), (3, 1, 1));
    assert!(last.end.ended_ms > last.end.started_ms);
    assert!(summary.running.is_none());
    assert_eq!(m.last_selection(), Some(&[Selection::All][..]));
}
