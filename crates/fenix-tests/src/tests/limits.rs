//! §6.5's sizes: a provider's data is untrusted and bounded, cut the
//! same way wherever it comes, and the cut said once.

use super::*;

/// §4, R9: an id over 1 KiB is cut the same way in a scan, a listing and
/// a report, so the same long id still matches; the cut is said once.
#[test]
fn an_id_over_1_kib_is_cut_the_same_everywhere() {
    let long = format!("tests/test_long.py::test_cases[{}]", "x".repeat(2_000));
    let mut m = Model::new();
    m.add_provider(PY, "pytest");
    m.scan(PY, "tests/test_long.py", vec![Found::new(long.clone(), None, "long", Kind::Test).at("tests/test_long.py", 3, None)]);
    m.list(Some(PY));
    let listed = Found::new(long.clone(), None, "long", Kind::Test);
    drive(&mut m, |_, _| Ok(plan(&[("list", &[])])), |_, _, _| (vec![Report::Found(listed.clone())], StepEnd::Exited(Some(0))));
    run(&mut m, Selection::All, plan(&[("all", &[&long])]), vec![ended(&long, Outcome::Passed)], StepEnd::Exited(Some(0)));
    let node = m.find(&key(PY, &long)).unwrap();
    assert_eq!(m.node(node).unwrap().found().id.len(), ID_BYTES);
    assert_eq!(m.node(node).unwrap().state(), State::Passed);
    assert_eq!(m.summary().counts.tests(), 1);
    let said: Vec<_> = m.take_notices().into_iter().filter(|n| n.text.contains("1 KiB")).collect();
    assert_eq!(said.len(), 1);
}

/// §6.5: cuts never split a character.
#[test]
fn a_cut_never_splits_a_character() {
    let s = "é".repeat(600);
    assert_eq!(cut_start(&s, 1023).len(), 1022);
    assert_eq!(cut_end(&s, 1023).len(), 1022);
}

/// The test vectors of the id cut ("Review of steps 1 and 2", finding 9;
/// `WRITING_EXTENSIONS.md`, "Test providers"): a pack's kit that cuts
/// ids itself must give these same strings. `fenix_wasm::test_limits`
/// cuts with the same function, and its test holds it to the same
/// vectors.
pub(crate) const ID_CUT_VECTORS: &[(&str, &str)] = &[
    // (what the id is, the 17 bytes a cut one ends with, or "" when it isn't cut)
    ("a × 1,023", ""),
    ("a × 1,024", ""),
    ("a × 1,025", "#6a424025ce84288c"),
    ("é × 600 (1,200 bytes: the prefix cut back to 1,006)", "#0e3ffe7c6421a045"),
    ("a then 😀 × 300 (1,201 bytes: the prefix cut back to 1,005)", "#2056db1222d21dec"),
    ("tests/test_x.py::test_cases[ then a × 1,100 then -one]", "#589a673077e12368"),
    ("tests/test_x.py::test_cases[ then a × 1,100 then -two]", "#a9074c8a277a3a36"),
];

pub(crate) fn id_cut_inputs() -> Vec<String> {
    let shared = format!("tests/test_x.py::test_cases[{}", "a".repeat(1_100));
    vec!["a".repeat(1_023), "a".repeat(1_024), "a".repeat(1_025), "é".repeat(600), format!("a{}", "😀".repeat(300)), format!("{shared}-one]"), format!("{shared}-two]")]
}

/// §4, §6.5, finding 9: an id over 1 KiB is its first 1,007 bytes or
/// fewer, to a character's start, then `#` and the FNV-1a 64-bit hash of
/// the whole id in 16 lowercase hex digits; at most 1,024 is as it was.
#[test]
fn a_long_id_is_cut_to_a_prefix_and_a_hash_of_the_whole() {
    assert_eq!(fnv1a_64(b""), 0xcbf2_9ce4_8422_2325);
    assert_eq!(fnv1a_64(b"a"), 0xaf63_dc4c_8601_ec8c);
    let inputs = id_cut_inputs();
    let lens = [1_023, 1_024, 1_024, 1_023, 1_022, 1_024, 1_024];
    for ((id, (what, end)), len) in inputs.iter().zip(ID_CUT_VECTORS).zip(lens) {
        let cut = cut_id(id);
        assert_eq!(cut.len(), len, "{what}");
        if end.is_empty() {
            assert_eq!(cut, id.as_str(), "{what}: kept as it is");
            assert!(!is_cut_id(&cut), "{what}");
        } else {
            assert_eq!(&cut[cut.len() - 17..], *end, "{what}");
            assert_eq!(&cut[..cut.len() - 17], &id[..cut.len() - 17], "{what}: its start");
            assert!(is_cut_id(&cut), "{what}");
            assert_eq!(cut_id(&cut), cut, "{what}: cutting it again changes nothing");
        }
    }
}

/// Finding 9: two ids that share their first 1,100 bytes stay two tests,
/// each with its own `data`.
#[test]
fn two_long_ids_that_share_a_kib_stay_two() {
    let inputs = id_cut_inputs();
    let (a, b) = (&inputs[5], &inputs[6]);
    let mut m = Model::new();
    m.add_provider(PY, "pytest");
    let mut fa = Found::new(a.clone(), None, "one", Kind::Test).at("tests/test_x.py", 1, Some(2));
    fa.data = Some("one".into());
    let mut fb = Found::new(b.clone(), None, "two", Kind::Test).at("tests/test_x.py", 5, Some(6));
    fb.data = Some("two".into());
    m.scan(PY, "tests/test_x.py", vec![fa, fb]);
    assert_ne!(id(&m, PY, a), id(&m, PY, b));
    assert_eq!(m.summary().counts.tests(), 2);
    assert_eq!(m.node(id(&m, PY, b)).unwrap().found().data.as_deref(), Some("two"));
}

/// §6.5, Decisions 26: a test's `data` over 4 KiB isn't kept -- not
/// cut, which wouldn't give it back "as it was" -- and that's said once.
#[test]
fn data_over_4_kib_isnt_kept_rather_than_cut() {
    let mut m = Model::new();
    m.add_provider(PY, "pytest");
    let mut f = Found::new("t", None, "t", Kind::Test);
    f.data = Some("d".repeat(DATA_BYTES + 1));
    let mut g = Found::new("u", None, "u", Kind::Test);
    g.data = Some("d".repeat(DATA_BYTES));
    m.scan(PY, "tests/test_x.py", vec![f, g]);
    assert_eq!(m.node(id(&m, PY, "t")).unwrap().found().data, None);
    assert_eq!(m.node(id(&m, PY, "u")).unwrap().found().data.as_ref().map(String::len), Some(DATA_BYTES), "at its limit, whole");
    assert!(m.take_notices().iter().any(|n| n.text == "a test's data over 4 KiB wasn't kept"));
}

/// §6.5: a label over 256 characters, a path over 4 KiB, a scan's answer
/// over 10,000 are cut or left out, never refused.
#[test]
fn what_a_scan_gives_is_bounded() {
    let mut m = Model::new();
    m.add_provider(PY, "pytest");
    let mut found: Vec<Found> = (0..10_050).map(|i| Found::new(format!("t{i}"), None, "ü".repeat(300), Kind::Test)).collect();
    found[0].path = Some("p".repeat(5_000));
    found[1].line = Some(9);
    found[1].end_line = Some(3);
    m.scan(PY, "tests/test_x.py", found);
    assert_eq!(m.summary().counts.tests(), 10_000);
    let first = m.node(id(&m, PY, "t0")).unwrap();
    assert_eq!(first.found().label.chars().count(), 256);
    assert_eq!(first.found().path.as_ref().unwrap().len(), 4096);
    assert_eq!(m.node(id(&m, PY, "t1")).unwrap().found().end_line, None);
    let notices = m.take_notices();
    assert_eq!(notices.len(), 3, "{notices:?}");
}

/// §6.5: 16 MiB of `data` kept a provider; past it a node's isn't kept.
#[test]
fn a_providers_data_past_16_mib_isnt_kept() {
    let mut m = Model::new();
    m.add_provider(PY, "pytest");
    let found: Vec<Found> = (0..4_200)
        .map(|i| {
            let mut f = Found::new(format!("t{i}"), None, "t", Kind::Test);
            f.data = Some("d".repeat(4_096));
            f
        })
        .collect();
    m.scan(PY, "tests/test_x.py", found);
    assert!(m.node(id(&m, PY, "t0")).unwrap().found().data.is_some());
    assert_eq!(m.node(id(&m, PY, "t4199")).unwrap().found().data, None);
    // A rescan replaces its own records: the budget is freed as it goes.
    m.scan(PY, "tests/test_x.py", Vec::new());
    m.scan(PY, "tests/test_y.py", vec![{
        let mut f = Found::new("y", None, "y", Kind::Test);
        f.data = Some("d".into());
        f
    }]);
    assert_eq!(m.node(id(&m, PY, "y")).unwrap().found().data.as_deref(), Some("d"));
}

/// §6.5: at most 100,000 tests a provider a project; what's past it
/// isn't kept, and that's said.
#[test]
fn past_100000_tests_a_provider_keeps_no_more() {
    let mut m = Model::new();
    m.add_provider(PY, "pytest");
    for f in 0..11 {
        let found = (0..10_000).map(|i| Found::new(format!("f{f}::t{i}"), None, "t", Kind::Test)).collect();
        m.scan(PY, &format!("tests/test_{f}.py"), found);
    }
    assert_eq!(m.summary().counts.tests(), TESTS_PER_PROVIDER as u32);
    assert!(m.take_notices().iter().any(|n| n.text.contains("100,000")));
}

/// §6.5: a plan's steps past 256 and its covers past 10,000 ids are left
/// out; a report's message is cut; a duration that isn't a number isn't
/// kept.
#[test]
fn a_plan_and_its_reports_are_bounded() {
    let mut m = orbit();
    let mut big = Plan::default();
    for i in 0..300 {
        big.steps.push(Step { label: format!("step {i}"), covers: (0..100).map(|j| format!("x{i}-{j}")).collect() });
    }
    m.request(Selection::All, Mode::Queue);
    let mut bad = Ended::new("orbit/lib tests::adds", Outcome::Failed);
    bad.duration_ms = Some(f64::NAN);
    bad.message = Some("m".repeat(10_000));
    let mut bad = Some(bad);
    let seen = drive(&mut m, |_, _| Ok(big.clone()), |_, _, _| (bad.take().map(Report::Ended).into_iter().collect(), StepEnd::Exited(Some(0))));
    assert_eq!(seen.steps.len(), 256);
    let result = m.node(id(&m, RUST, "orbit/lib tests::adds")).unwrap().result().unwrap().clone();
    assert_eq!(result.duration_ms, None);
    assert_eq!(result.message.unwrap().len(), MESSAGE_BYTES);
    let notices = m.take_notices();
    assert!(notices.iter().any(|n| n.text.contains("256")));
    assert!(notices.iter().any(|n| n.text.contains("covers")));
}
