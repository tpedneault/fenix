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
    assert_eq!(cut_id(&format!("a{s}")).len(), 1023);
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
