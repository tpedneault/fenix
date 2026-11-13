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
    m.add_provider(NEXTEST, "cargo nextest");
    m.scan(NEXTEST, "src/lib.rs", lib_rs());
    m.request(Selection::File("src/lib.rs".into()), Mode::Queue);
    let mut asked = Vec::new();
    drive(&mut m, |p, ask| {
        asked.push((p.to_string(), ids(ask).into_iter().map(str::to_string).collect::<Vec<_>>()));
        Ok(plan(&[]))
    }, |_, _, _| unreachable!());
    assert_eq!(asked.len(), 2);
    assert_eq!(asked[0].1[0], "orbit/lib tests");
}

/// "Review of steps 1 and 2", finding 3 (§4 "What the provider is
/// asked"): a fill-in holds every node under the file or suite asked,
/// the suites between included, so each of its records' `parent` is in
/// the ask; a node asked for itself has a `parent` that isn't. The
/// provider tells the two apart without reading its ids.
#[test]
fn a_fill_in_holds_the_suites_between_so_every_parent_is_in_the_ask() {
    let mut m = Model::new();
    m.add_provider(PY, "pytest");
    let path = "tests/test_a.py";
    let class = format!("{path}::TestMath");
    let other = "tests/test_b.py";
    m.scan(PY, path, vec![
        Found::new(path, None, path, Kind::File),
        Found::new(class.clone(), Some(path), "TestMath", Kind::Suite).at(path, 2, Some(20)),
        Found::new(format!("{class}::test_adds"), Some(&class), "test_adds", Kind::Test).at(path, 3, Some(5)),
        Found::new(format!("{path}::test_top"), Some(path), "test_top", Kind::Test).at(path, 30, Some(32)),
    ]);
    m.scan(PY, other, vec![Found::new(other, None, other, Kind::File), Found::new(format!("{other}::test_b"), Some(other), "test_b", Kind::Test).at(other, 1, Some(2))]);
    let ask = ask_for(&mut m, Selection::Nodes(vec![key(PY, path), key(PY, &format!("{other}::test_b"))]));
    let Ask::Run(records) = &ask else { panic!() };
    let asked: Vec<&str> = records.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(asked, [path, class.as_str(), &format!("{class}::test_adds"), &format!("{path}::test_top"), &format!("{other}::test_b")]);
    let in_ask = |id: &Option<String>| id.as_deref().is_some_and(|id| asked.contains(&id));
    let selected: Vec<&str> = records.iter().filter(|r| !in_ask(&r.parent)).map(|r| r.id.as_str()).collect();
    assert_eq!(selected, [path, &format!("{other}::test_b")], "test_b's parent, its file, wasn't asked");
}

/// "Review of steps 1 and 2", finding 3 (§4): with the fill-in left
/// out past 10,000 records, a test selected beside its file's sibling is
/// still told apart: its parent isn't in the ask.
#[test]
fn past_the_cap_a_selected_test_is_still_told_from_a_fill_in() {
    let mut m = Model::new();
    m.add_provider(PY, "pytest");
    let big = "tests/test_big.py";
    let mut found = vec![Found::new(big, None, big, Kind::File)];
    found.extend((0..10_000).map(|i| Found::new(format!("{big}::t{i}"), Some(big), format!("t{i}"), Kind::Test).at(big, i, None)));
    m.scan(PY, big, found);
    let small = "tests/test_small.py";
    m.scan(PY, small, vec![Found::new(small, None, small, Kind::File), Found::new(format!("{small}::u"), Some(small), "u", Kind::Test).at(small, 1, None)]);
    let ask = ask_for(&mut m, Selection::Nodes(vec![key(PY, big), key(PY, &format!("{small}::u"))]));
    assert_eq!(ids(&ask), [big, &format!("{small}::u")], "the big file's fill-in doesn't fit: left out whole");
    let Ask::Run(records) = &ask else { panic!() };
    assert_eq!(records[1].parent.as_deref(), Some(small), "not in the ask: asked for itself");
}

/// "Review of steps 1 and 2", finding 2 (§4, §6.5; Decisions 32): a
/// node whose id was cut and whose `data` isn't kept can't be named to
/// its runner, so it's asked as its parent; a fill-in holding one is
/// left out whole; with data, it's asked as itself.
#[test]
fn a_cut_id_without_its_data_is_asked_as_its_parent() {
    let mut m = Model::new();
    m.add_provider(PY, "pytest");
    let path = "tests/test_long.py";
    let long = format!("{path}::test_cases[{}]", "x".repeat(2_000));
    let mut with_data = Found::new(format!("{long}-kept"), Some(path), "kept", Kind::Case).at(path, 9, None);
    with_data.data = Some("its own name".into());
    m.scan(PY, path, vec![Found::new(path, None, path, Kind::File), Found::new(long.clone(), Some(path), "long", Kind::Case).at(path, 3, None), with_data, Found::new(format!("{path}::short"), Some(path), "short", Kind::Test).at(path, 20, None)]);
    let cut = cut_id(&long).into_owned();
    assert!(!m.node(id(&m, PY, &long)).unwrap().can_be_named());
    let ask = ask_for(&mut m, Selection::Nodes(vec![key(PY, &long)]));
    assert_eq!(ids(&ask), [path], "its file, without a fill-in that holds it");
    let ask = ask_for(&mut m, Selection::Nodes(vec![key(PY, &format!("{long}-kept"))]));
    let Ask::Run(records) = &ask else { panic!() };
    assert_eq!((records.len(), records[0].data.as_deref()), (1, Some("its own name")));
    assert!(m.node(id(&m, PY, &format!("{long}-kept"))).unwrap().can_be_named());
    assert_ne!(cut, records[0].id);
}

/// "Review of steps 1 and 2", finding 3: a record's `parent` is the one
/// Fenix put it under -- none for one at its provider's top because
/// its parent was never found (Decisions 6).
#[test]
fn a_records_parent_is_where_fenix_put_it() {
    let mut m = Model::new();
    m.add_provider(PY, "pytest");
    m.scan(PY, "tests/test_a.py", vec![Found::new("tests/test_a.py::t", Some("tests/test_a.py::Missing"), "t", Kind::Test).at("tests/test_a.py", 1, None)]);
    let ask = ask_for(&mut m, Selection::Nodes(vec![key(PY, "tests/test_a.py::t")]));
    let Ask::Run(records) = &ask else { panic!() };
    assert_eq!(records[0].parent, None);
    let node = m.node(id(&m, PY, "tests/test_a.py::t")).unwrap();
    assert_eq!(node.found().parent.as_deref(), Some("tests/test_a.py::Missing"), "the provider's own is kept");
    assert_eq!(node.given_back().parent, None);
}
