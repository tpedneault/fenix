//! The tree from scans and listings: merging per source, dropping per
//! source, parents (§4 "Tests and groups", "A `found` for an id Fenix
//! has", "What's dropped").

use super::*;

fn labels(m: &Model, ids: &[NodeId]) -> Vec<String> {
    ids.iter().map(|&n| m.node(n).unwrap().found().label.clone()).collect()
}

fn listing(m: &mut Model, provider: &str, found: Vec<Found>, end: StepEnd) {
    m.list(Some(provider));
    let mut found = Some(found);
    drive(m, |_, _| Ok(plan(&[("list", &[])])), |_, _, _| (found.take().unwrap_or_default().into_iter().map(Report::Found).collect(), end.clone()));
}

/// §4 "Tests and groups": a provider's scans make its tree, groups
/// holding files' suites and their tests, in display order (files by
/// path, tests by line).
#[test]
fn scans_make_the_tree_in_display_order() {
    let m = orbit();
    let rust = m.providers()[0].provider;
    let roots = m.roots(rust);
    assert_eq!(labels(&m, roots), ["orbit (lib)"]);
    let lib = m.node(roots[0]).unwrap();
    assert_eq!(labels(&m, lib.children()), ["geo::tests", "tests"]);
    let tests = m.node(lib.children()[1]).unwrap();
    assert_eq!(labels(&m, tests.children()), ["adds", "case_negative", "fails_on_purpose"]);
    assert_eq!(lib.counts().tests(), 5);
    assert_eq!(lib.state(), State::Unknown);
}

/// §6.3 `found-test.path`, Decisions 2: in a scan's answer, a test with
/// no path is in the file scanned; a group isn't given one.
#[test]
fn a_scanned_test_without_a_path_is_in_the_file_scanned() {
    let m = orbit();
    let adds = m.node(id(&m, RUST, "orbit/lib tests::adds")).unwrap();
    assert_eq!(adds.found().path.as_deref(), Some("src/lib.rs"));
    assert_eq!(adds.file(), Some("src/lib.rs"));
    assert_eq!(m.node(id(&m, RUST, LIB)).unwrap().found().path, None);
}

/// §4, F3: cargo's `--list` gives no line and can't tell an inline
/// `mod tests` from `src/tests.rs`; its listing never takes a scanned
/// test's line or file away.
#[test]
fn a_listing_that_knows_no_lines_keeps_the_scans_lines() {
    let mut m = orbit();
    let mut listed = Found::new("orbit/lib tests::adds", Some("orbit/lib tests"), "tests::adds", Kind::Test);
    listed.path = None;
    listing(&mut m, RUST, vec![group(LIB, "orbit"), listed], StepEnd::Exited(Some(0)));
    let adds = m.node(id(&m, RUST, "orbit/lib tests::adds")).unwrap();
    assert_eq!(adds.found().line, Some(12));
    assert_eq!(adds.found().end_line, Some(15));
    assert_eq!(adds.found().path.as_deref(), Some("src/lib.rs"));
    // The scan's label too: it has the node.
    assert_eq!(adds.found().label, "adds");
    assert_eq!(m.marks("src/lib.rs", 0..=100).len(), 4);
}

/// §4: a field the scan leaves out falls through to the listing's, then
/// the run's: a test the scan can't see a line for gets the listing's.
#[test]
fn a_field_the_scan_leaves_out_falls_through_to_the_listing() {
    let mut m = orbit();
    let mut listed = Found::new("orbit/lib tests::adds", Some("orbit/lib tests"), "adds", Kind::Test);
    listed.data = Some("adds --exact".into());
    listing(&mut m, RUST, vec![listed], StepEnd::Exited(Some(0)));
    assert_eq!(m.node(id(&m, RUST, "orbit/lib tests::adds")).unwrap().found().data.as_deref(), Some("adds --exact"));
}

/// §4: `skip` is the scan's as it is where a scan has the node. A
/// listing that doesn't mark ignored tests (cargo's) leaves the scan's
/// `#[ignore]`; removing the `#[ignore]` isn't undone by an older
/// listing that marked it (nextest's).
#[test]
fn skip_is_the_scans_wherever_a_scan_has_the_node() {
    let mut m = orbit();
    let mut scanned = lib_rs();
    scanned[2].skip = Some("slow".into());
    m.scan(RUST, "src/lib.rs", scanned);
    listing(&mut m, RUST, vec![Found::new("orbit/lib tests::adds", Some("orbit/lib tests"), "adds", Kind::Test)], StepEnd::Exited(Some(0)));
    assert_eq!(m.node(id(&m, RUST, "orbit/lib tests::adds")).unwrap().found().skip.as_deref(), Some("slow"));

    let mut nextest = Found::new("orbit/lib tests::adds", Some("orbit/lib tests"), "adds", Kind::Test);
    nextest.skip = Some(String::new());
    listing(&mut m, RUST, vec![nextest], StepEnd::Exited(Some(0)));
    m.scan(RUST, "src/lib.rs", lib_rs());
    assert_eq!(m.node(id(&m, RUST, "orbit/lib tests::adds")).unwrap().found().skip, None);
}

/// §4: `label`, `kind` and `parent` come from the first source that has
/// the node; a test only listed takes the listing's.
#[test]
fn label_and_parent_come_from_the_first_source_that_has_the_node() {
    let mut m = orbit();
    listing(&mut m, RUST, vec![Found::new("orbit/lib tests::listed_only", Some("orbit/lib tests"), "listed_only", Kind::Test)], StepEnd::Exited(Some(0)));
    let listed = m.node(id(&m, RUST, "orbit/lib tests::listed_only")).unwrap();
    assert_eq!(listed.found().label, "listed_only");
    assert_eq!(listed.parent(), Some(id(&m, RUST, "orbit/lib tests")));
    // Inherits its file from the suite it's in.
    assert_eq!(listed.file(), Some("src/lib.rs"));
}

/// §4 "What's dropped": a group several files' scans give is kept while
/// any of their last scans gives it.
#[test]
fn a_group_several_files_give_stays_while_any_scan_gives_it() {
    let mut m = orbit();
    m.scan(RUST, "src/geo/mod.rs", Vec::new());
    assert!(has(&m, RUST, LIB));
    assert!(!has(&m, RUST, "orbit/lib geo::tests::unwraps_none"));
    m.scan(RUST, "src/lib.rs", Vec::new());
    assert!(!has(&m, RUST, LIB));
}

/// §4 "What's dropped": a scan's record goes when a later scan of its
/// file doesn't give it; the node goes when no source has it, and a
/// listing's record keeps it.
#[test]
fn a_node_goes_when_no_source_has_it() {
    let mut m = orbit();
    listing(&mut m, RUST, vec![Found::new("orbit/lib tests::adds", Some("orbit/lib tests"), "adds", Kind::Test)], StepEnd::Exited(Some(0)));
    let mut without = lib_rs();
    without.retain(|f| !f.id.ends_with("adds") && !f.id.ends_with("case_negative"));
    m.scan(RUST, "src/lib.rs", without);
    assert!(!has(&m, RUST, "orbit/lib tests::case_negative"));
    assert!(has(&m, RUST, "orbit/lib tests::adds"), "the listing still has it");
    assert_eq!(m.node(id(&m, RUST, "orbit/lib tests::adds")).unwrap().found().line, None);
}

/// §4, F14: a listing's record goes when the next complete listing
/// doesn't give it.
#[test]
fn a_complete_listing_drops_what_it_no_longer_gives() {
    let mut m = Model::new();
    m.add_provider(PY, "pytest");
    let a = Found::new("tests/test_basic.py::test_adds", None, "test_adds", Kind::Test);
    let b = Found::new("tests/test_basic.py::test_gone", None, "test_gone", Kind::Test);
    listing(&mut m, PY, vec![a.clone(), b], StepEnd::Exited(Some(0)));
    listing(&mut m, PY, vec![a], StepEnd::Exited(Some(1)));
    assert!(has(&m, PY, "tests/test_basic.py::test_adds"));
    assert!(!has(&m, PY, "tests/test_basic.py::test_gone"));
}

/// §4: pytest's exit code 5 when it collected nothing is a complete,
/// empty listing: what it gave before goes.
#[test]
fn pytests_exit_5_is_a_complete_empty_listing() {
    let mut m = Model::new();
    m.add_provider(PY, "pytest");
    listing(&mut m, PY, vec![Found::new("tests/test_basic.py::test_adds", None, "test_adds", Kind::Test)], StepEnd::Exited(Some(0)));
    listing(&mut m, PY, Vec::new(), StepEnd::Exited(Some(5)));
    assert!(!has(&m, PY, "tests/test_basic.py::test_adds"));
}

/// §4, F14: a listing with a `broken`, a `crashed`, or a step that
/// didn't exit adds and updates, and drops nothing.
#[test]
fn a_listing_that_isnt_complete_drops_nothing() {
    let mut m = Model::new();
    m.add_provider(PY, "pytest");
    listing(&mut m, PY, vec![Found::new("tests/test_basic.py::test_adds", None, "test_adds", Kind::Test)], StepEnd::Exited(Some(0)));
    listing(&mut m, PY, Vec::new(), StepEnd::Stopped);
    assert!(has(&m, PY, "tests/test_basic.py::test_adds"));
    m.list(Some(PY));
    drive(&mut m, |_, _| Ok(plan(&[("list", &[])])), |_, _, _| (vec![Report::Broken { id: None, message: "ERROR collecting tests/test_cli.py".into(), at: None }], StepEnd::Exited(Some(2))));
    assert!(has(&m, PY, "tests/test_basic.py::test_adds"));
}

/// §4: a listing doesn't drop a test of a file saved since it began:
/// the scan is newer than the list.
#[test]
fn a_complete_listing_keeps_tests_of_a_file_saved_since_it_began() {
    let mut m = Model::new();
    m.add_provider(PY, "pytest");
    let mut listed = Found::new("tests/test_new.py::test_typed", None, "test_typed", Kind::Test);
    listed.path = Some("tests/test_new.py".into());
    listing(&mut m, PY, vec![listed], StepEnd::Exited(Some(0)));
    m.list(Some(PY));
    drive(
        &mut m,
        |_, _| Ok(plan(&[("list", &[])])),
        |m, _, _| {
            m.file_saved("tests/test_new.py");
            (Vec::new(), StepEnd::Exited(Some(0)))
        },
    );
    assert!(has(&m, PY, "tests/test_new.py::test_typed"));
}

/// Decisions 6: a node whose parent hasn't come is at its provider's
/// top, and moves under it when it comes.
#[test]
fn a_node_waits_at_the_top_for_its_parent() {
    let mut m = Model::new();
    m.add_provider(GO, "go test");
    let mut sub = Found::new("example.com/orbit TestTable/negative_one", Some("example.com/orbit TestTable"), "negative_one", Kind::Case);
    sub.path = Some("orbit_test.go".into());
    m.scan(GO, "orbit_test.go", vec![sub]);
    let go = m.providers()[0].provider;
    assert_eq!(m.roots(go).len(), 1);
    let mut table = Found::new("example.com/orbit TestTable", None, "TestTable", Kind::Test).at("orbit_test.go", 20, Some(40));
    table.template = true;
    m.scan(GO, "other_test.go", vec![table]);
    assert_eq!(m.roots(go).len(), 1);
    assert_eq!(m.node(id(&m, GO, "example.com/orbit TestTable/negative_one")).unwrap().parent(), Some(id(&m, GO, "example.com/orbit TestTable")));
}

/// Decisions 6: parents that make a loop don't hang the model; the tree
/// stays a tree.
#[test]
fn parents_in_a_loop_put_a_node_at_the_top() {
    let mut m = Model::new();
    m.add_provider(RUST, "cargo test");
    m.scan(RUST, "src/lib.rs", vec![Found::new("a", Some("b"), "a", Kind::Suite), Found::new("b", Some("a"), "b", Kind::Suite), Found::new("c", Some("c"), "c", Kind::Test)]);
    let rust = m.providers()[0].provider;
    assert_eq!(m.roots(rust).len(), 2, "a or b at the top, c at the top");
    let rows = m.rows(&RowQuery { tab: Tab::All, filter: "", open: &|_| true });
    assert_eq!(rows.len(), 3);
}

/// Decisions 3: tests with the same label and distinct ids are two
/// tests; the same id twice in one scan is one, the later winning.
#[test]
fn repeated_names_are_tests_while_their_ids_differ() {
    let mut m = Model::new();
    m.add_provider("web", "vitest");
    let file = Found::new("src/orbit.test.js", None, "src/orbit.test.js", Kind::File);
    let a = Found::new("src/orbit.test.js > geo > adds", Some("src/orbit.test.js"), "adds", Kind::Test).at("src/orbit.test.js", 3, None);
    let b = Found::new("src/orbit.test.js > math > adds", Some("src/orbit.test.js"), "adds", Kind::Test).at("src/orbit.test.js", 9, None);
    let twice = Found::new("src/orbit.test.js > math > adds", Some("src/orbit.test.js"), "adds", Kind::Test).at("src/orbit.test.js", 12, None);
    m.scan("web", "src/orbit.test.js", vec![file, a, b, twice]);
    let file = m.node(id(&m, "web", "src/orbit.test.js")).unwrap();
    assert_eq!(file.children().len(), 2);
    assert_eq!(m.node(id(&m, "web", "src/orbit.test.js > math > adds")).unwrap().found().line, Some(12));
}

/// §4 "Two providers in one file": both are kept, each under its own
/// root; the two never share a node.
#[test]
fn two_providers_in_one_file_never_share_a_node() {
    let mut m = orbit();
    m.add_provider("rust.nextest", "cargo nextest");
    m.scan("rust.nextest", "src/lib.rs", lib_rs());
    assert_ne!(id(&m, RUST, "orbit/lib tests::adds"), id(&m, "rust.nextest", "orbit/lib tests::adds"));
    assert_eq!(m.providers().len(), 2);
}

/// §6.3 `test-provider-set`, F8: `runner` "" with no problem hides the
/// provider: no row, no marks, never asked. A call that says what the
/// model has changes nothing.
#[test]
fn a_provider_with_nothing_to_run_isnt_shown() {
    let mut m = orbit();
    assert!(m.provider_set(RUST, "cargo test", None));
    assert!(!m.provider_set(RUST, "cargo test", None));
    assert!(m.provider_set(RUST, "", None));
    assert!(m.providers().is_empty());
    assert!(m.marks("src/lib.rs", 0..=100).is_empty());
    assert_eq!(m.request(Selection::All, Mode::Queue), Requested::Nothing);
    assert!(m.provider_set(RUST, "", Some("cargo isn't installed")));
    assert_eq!(m.providers()[0].problem, Some("cargo isn't installed"));
}

/// §7.1, F8: a `test-provider-set` that changed what the model had lists
/// the project again, but only once the page has opened in the session.
#[test]
fn a_changed_provider_is_listed_once_the_page_has_opened() {
    let mut m = orbit();
    m.provider_set(RUST, "cargo test", None);
    assert_eq!(m.next(0), None);
    m.page_opened();
    let Some(Action::Plan { run, ask: Ask::Discover, .. }) = m.next(0) else { panic!("a listing") };
    m.planned(run, Ok(plan(&[])));
    drive(&mut m, |_, _| Ok(plan(&[])), |_, _, _| (Vec::new(), StepEnd::Exited(Some(0))));
    m.provider_set(RUST, "cargo test", None);
    assert_eq!(m.next(0), None, "the same answer lists nothing");
    m.provider_set(RUST, "cargo nextest", None);
    assert!(matches!(m.next(0), Some(Action::Plan { ask: Ask::Discover, .. })));
}

/// §7.1, §12: automatic listings run only in a trusted project with
/// `tests.list_automatically` on; `u` lists whatever they say.
#[test]
fn automatic_listings_wait_for_trust_but_u_doesnt() {
    let mut m = orbit();
    m.set_automatic_listings(false);
    m.page_opened();
    assert_eq!(m.next(0), None);
    m.list(None);
    assert!(matches!(m.next(0), Some(Action::Plan { ask: Ask::Discover, .. })));
}

/// §4 "Tests and groups": a provider that serves several roots of one
/// project shows them as its own groups.
#[test]
fn a_provider_with_several_roots_shows_them_as_groups() {
    let mut m = orbit();
    m.scan(RUST, "tests/it.rs", vec![group(IT, "orbit (test it)"), Found::new("orbit/it adds_up", Some(IT), "adds_up", Kind::Test).at("tests/it.rs", 3, Some(6))]);
    let rust = m.providers()[0].provider;
    assert_eq!(labels(&m, m.roots(rust)), ["orbit (lib)", "orbit (test it)"]);
}
