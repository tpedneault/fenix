//! What a provider is asked (§4 "What the provider is asked", F2, R1).

use super::*;

fn ask_for(m: &mut Model, selection: Selection) -> Ask {
    assert_ne!(m.request(selection, Mode::Queue), Requested::Nothing);
    let Some(Action::Plan { run, ask, .. }) = m.next(0) else { panic!("a plan asked") };
    m.planned(run, Err("not run".into()));
    while m.next(0).is_some() {}
    ask
}

fn ids(ask: &Ask) -> Vec<&str> {
    match ask {
        Ask::Run(records) => records.iter().map(|r| r.id.as_str()).collect(),
        other => panic!("{other:?}"),
    }
}

/// §4, F2: after each `file` or `suite` asked come the tests and cases
/// the model has under it, so a provider whose runner filters by
/// substring can name them exactly.
#[test]
fn an_asked_suite_is_followed_by_its_tests() {
    let mut m = orbit();
    let ask = ask_for(&mut m, Selection::Nodes(vec![key(RUST, "orbit/lib tests")]));
    assert_eq!(ids(&ask), ["orbit/lib tests", "orbit/lib tests::adds", "orbit/lib tests::case_negative", "orbit/lib tests::fails_on_purpose"]);
}

/// §4, F2: never under a `group`: a target or a package is named to its
/// runner as itself.
#[test]
fn an_asked_group_is_asked_alone() {
    let mut m = orbit();
    let ask = ask_for(&mut m, Selection::Nodes(vec![key(RUST, LIB)]));
    assert_eq!(ids(&ask), [LIB]);
}

/// §4, F6: each record is the node as the model has it now, the merged
/// fields and its `data`; a test asked with its suite is asked once.
#[test]
fn the_ask_holds_the_merged_node_and_its_data() {
    let mut m = orbit();
    let mut found = lib_rs();
    found[2].data = Some("tests::adds".into());
    m.scan(RUST, "src/lib.rs", found);
    let ask = ask_for(&mut m, Selection::Nodes(vec![key(RUST, "orbit/lib tests::adds"), key(RUST, "orbit/lib geo::tests::unwraps_none")]));
    let Ask::Run(records) = ask else { panic!() };
    assert_eq!(records[1].data.as_deref(), Some("tests::adds"));
    assert_eq!(records[1].line, Some(12));
    assert_eq!(records[1].path.as_deref(), Some("src/lib.rs"));
    let ask = ask_for(&mut m, Selection::Nodes(vec![key(RUST, "orbit/lib tests::adds"), key(RUST, "orbit/lib tests")]));
    assert_eq!(ids(&ask).iter().filter(|&&id| id == "orbit/lib tests::adds").count(), 1);
}

/// §4: the whole project is asked as `run-all`.
#[test]
fn all_is_asked_as_run_all() {
    let mut m = orbit();
    assert_eq!(ask_for(&mut m, Selection::All), Ask::RunAll);
}

/// §4 "What the provider is asked", R1: an ask holds at most 10,000
/// records; past that the tests under an asked file are left out, a
/// file's whole or not at all (Decisions 20).
#[test]
fn past_10000_records_the_tests_under_a_file_are_left_out() {
    let mut m = Model::new();
    m.add_provider(PY, "pytest");
    for (file, count) in [("a", 6_000u32), ("b", 5_000)] {
        let path = format!("tests/test_{file}.py");
        let mut found = vec![Found::new(path.clone(), None, path.clone(), Kind::File)];
        found.extend((0..count).map(|i| Found::new(format!("{path}::test_{i}"), Some(&path), format!("test_{i}"), Kind::Test).at(&path, i, None)));
        m.scan(PY, &path, found);
    }
    let ask = ask_for(&mut m, Selection::Nodes(vec![key(PY, "tests/test_a.py"), key(PY, "tests/test_b.py")]));
    let ids = ids(&ask);
    assert_eq!(ids.len(), 2 + 6_000);
    assert_eq!(ids[0], "tests/test_a.py");
    assert_eq!(ids[6_001], "tests/test_b.py");
}

/// §4, R1: a selection of more than 10,000 nodes is asked as their
/// parents, coarser but never fewer; past the top, as `run-all`.
#[test]
fn a_selection_past_10000_is_asked_as_its_parents() {
    let mut m = Model::new();
    m.add_provider(PY, "pytest");
    let mut keys = Vec::new();
    for f in 0..30 {
        let path = format!("tests/test_{f}.py");
        let mut found = vec![Found::new(path.clone(), None, path.clone(), Kind::File)];
        for i in 0..400 {
            let id = format!("{path}::test_{i}");
            keys.push(key(PY, &id));
            found.push(Found::new(id, Some(&path), format!("test_{i}"), Kind::Test).at(&path, i, None));
        }
        m.scan(PY, &path, found);
    }
    let ask = ask_for(&mut m, Selection::Nodes(keys.clone()));
    let ids = ids(&ask);
    assert_eq!(ids.len(), 30 + 24 * 400, "30 files, and all the tests of the 24 that fit");
    assert!(ids.contains(&"tests/test_0.py"));

    // 10,001 files, each at the top: no parent is left, so `run-all`.
    let mut m = Model::new();
    m.add_provider(PY, "pytest");
    for (path, range) in [("tests/test_a.py", 0..5_000), ("tests/test_b.py", 5_000..10_001)] {
        m.scan(PY, path, range.map(|i| Found::new(format!("t{i}"), None, format!("t{i}"), Kind::Test)).collect());
    }
    let keys = (0..10_001).map(|i| key(PY, &format!("t{i}"))).collect();
    assert_eq!(ask_for(&mut m, Selection::Nodes(keys)), Ask::RunAll);
}

/// §8.4, Decisions 27: a file's tests, from every provider that has
/// them, each asked as its topmost nodes in the file.
#[test]
fn a_files_tests_are_every_providers_in_it() {
    let mut m = orbit();
    m.add_provider("rust.nextest", "cargo nextest");
    m.scan("rust.nextest", "src/lib.rs", lib_rs());
    m.request(Selection::File("src/lib.rs".into()), Mode::Queue);
    let mut asked = Vec::new();
    drive(&mut m, |p, ask| {
        asked.push((p.to_string(), ids(ask).into_iter().map(str::to_string).collect::<Vec<_>>()));
        Ok(plan(&[]))
    }, |_, _, _| unreachable!());
    assert_eq!(asked.len(), 2);
    assert_eq!(asked[0].1[0], "orbit/lib tests");
}
