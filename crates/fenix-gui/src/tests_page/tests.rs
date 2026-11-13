//! The tests page laid out as text (§7.2's two pictures, at 100×30 and
//! 80×24), its keys, what it cleans, and what it costs at 10,000 tests.

use super::*;
use fenix_tests::{Action as Next, Ended, Found, Kind, Location, Mode, Outcome, Plan, ProviderId, Report, Requested, Selection, Step, StepEnd};

fn rust() -> ProviderId {
    ProviderId::new("fenix.rust", "rust")
}

fn python() -> ProviderId {
    ProviderId::new("fenix.python", "python")
}

const LIB: &str = "orbit/lib";
const NEG: &str = "orbit/lib tests::case_negative";
const ADDS: &str = "orbit/lib tests::adds";

fn test(id: &str, parent: &str, label: &str, path: &str, line: u32) -> Found {
    Found::new(id, Some(parent), label, Kind::Test).at(path, line, Some(line + 3))
}

/// A Rust workspace with a Python tool beside it (§7.2's picture).
fn orbit() -> Model {
    let mut m = Model::new();
    m.add_provider(&rust(), "cargo test");
    m.add_provider(&python(), "pytest");
    m.scan(
        &rust(),
        "src/lib.rs",
        vec![
            Found::new(LIB, None, "orbit (lib)", Kind::Group),
            Found::new("orbit/lib tests", Some(LIB), "tests", Kind::Suite).at("src/lib.rs", 10, Some(60)),
            test(ADDS, "orbit/lib tests", "adds", "src/lib.rs", 12),
            test(NEG, "orbit/lib tests", "case_negative", "src/lib.rs", 40),
            test("orbit/lib tests::ignored_one", "orbit/lib tests", "ignored_one", "src/lib.rs", 50),
        ],
    );
    m.scan(
        &python(),
        "tools/tests/test_basic.py",
        vec![
            Found::new("tools/tests/test_basic.py", None, "tools/tests/test_basic.py", Kind::File).at("tools/tests/test_basic.py", 0, None),
            test("tools/tests/test_basic.py::test_adds", "tools/tests/test_basic.py", "test_adds", "tools/tests/test_basic.py", 3),
            test("tools/tests/test_basic.py::test_fixture", "tools/tests/test_basic.py", "test_fixture", "tools/tests/test_basic.py", 9),
        ],
    );
    m
}

/// Drives the model through a run as the host does, every step ended
/// with `reports`.
fn run(m: &mut Model, selection: Selection, reports: impl Fn(&ProviderId) -> Vec<Report>) {
    assert_ne!(m.request(selection, Mode::Queue), Requested::Nothing);
    let mut now = 50_000_000;
    loop {
        now += 700;
        match m.next(now) {
            None => return,
            Some(Next::Plan { run, provider, .. }) => {
                let label = provider.provider.to_string();
                m.planned(run, Ok(Plan { steps: vec![Step::new(&label, &[])], list_after: false }));
            }
            Some(Next::Step { run, provider, step, .. }) => {
                let said = reports(&provider);
                m.step_ended(run, step, StepEnd::Exited(Some(1)), said);
            }
            Some(_) => {}
        }
    }
}

fn after_a_run() -> Model {
    let mut m = orbit();
    run(&mut m, Selection::All, |provider| {
        if provider.provider.as_ref() == "rust" {
            let mut neg = Ended::new(NEG, Outcome::Failed);
            neg.message = Some("assertion `left == right` failed\n  left: -2\n right: -3".into());
            neg.at = Some(Location { path: "src/lib.rs".into(), line: 42, column: Some(5) });
            neg.text = Some("---- tests::case_negative stdout ----\n\u{1b}[31mthread panicked\u{1b}[0m at src/lib.rs:43:5:".into());
            neg.duration_ms = Some(0.3);
            let mut skip = Ended::new("orbit/lib tests::ignored_one", Outcome::Skipped);
            skip.message = Some("slow".into());
            vec![Report::Ended(Ended::new(ADDS, Outcome::Passed)), Report::Ended(neg), Report::Ended(skip)]
        } else {
            let mut fixture = Ended::new("tools/tests/test_basic.py::test_fixture", Outcome::Errored);
            fixture.message = Some("setup".into());
            vec![Report::Ended(Ended::new("tools/tests/test_basic.py::test_adds", Outcome::Passed)), Report::Ended(fixture)]
        }
    });
    m
}

fn clock(ms: u64) -> String {
    format!("{:02}:{:02}", (ms / 3_600_000) % 24, (ms / 60_000) % 60)
}

fn lay_out(page: &mut TestsPage, m: &Model, cols: usize, rows: usize) -> Page {
    let summary = m.summary();
    layout(page, &View { model: m, summary: &summary, notes: Vec::new(), clock: &clock }, cols, Some(rows))
}

fn page() -> TestsPage {
    TestsPage::new(PathBuf::from("/orbit"), "orbit".into())
}

/// The focused row's label (a provider's as `provider <name>`).
fn label(p: &TestsPage, m: &Model) -> String {
    match p.focus {
        Some(Item::Node(id)) => m.node(id).unwrap().found().label.clone(),
        Some(Item::Provider(pr)) => format!("provider {}", m.provider(pr).unwrap().name),
        None => String::new(),
    }
}

/// `j` until the row labelled `name` has the focus.
fn focus_on(p: &mut TestsPage, m: &Model, name: &str) {
    p.key(Key::Char('g'), Some(m));
    p.key(Key::Char('g'), Some(m));
    for _ in 0..p.rows().len() {
        if label(p, m) == name {
            return;
        }
        p.key(Key::Char('j'), Some(m));
    }
    assert_eq!(label(p, m), name);
}

fn lines(p: &Page) -> Vec<String> {
    p.text.split('\n').map(str::to_string).collect()
}

/// §7.2's first picture: at 100×30, after a run with failures, the head
/// has the counts and when it ran; the tabs their counts; the tree its
/// providers, the groups holding a failure open, the focused test's
/// details beside it with its message and text, escape codes taken out.
#[test]
fn at_100_by_30_after_a_run_the_head_tree_and_details_show_whats_wrong() {
    let m = after_a_run();
    let mut p = page();
    let out = lay_out(&mut p, &m, 100, 30);
    let text = out.text.clone();
    let l = lines(&out);
    assert_eq!(l.len(), 30, "laid out to the pane's height:\n{text}");
    assert!(l[0].starts_with(" Tests  orbit") && l[0].contains("✓ 2") && l[0].contains("✗ 1") && l[0].contains("! 1") && l[0].contains("○ 1"), "{}", l[0]);
    assert!(l[0].contains("ran ") && l[0].contains(" for "), "when it ran: {}", l[0]);
    assert!(l[2].contains("All 5") && l[2].contains("Failed 2") && l[2].contains("Not run 1") && l[2].trim_end().ends_with("/ to filter"), "{}", l[2]);
    let tree = text.find("▾ cargo test").expect("the providers' rows, two of them");
    assert!(text[tree..].contains("▾ orbit (lib)") && text.contains("▾ tests") && text.contains("▾ pytest"), "a group holding a failure starts open:\n{text}");
    assert!(text.contains("case_negative") && text.contains("○ slow") && text.contains("! setup"), "{text}");
    // Down to the failure: its details beside the tree.
    focus_on(&mut p, &m, "case_negative");
    let out = lay_out(&mut p, &m, 100, 30);
    let text = out.text.clone();
    let row = lines(&out).into_iter().find(|l| l.contains("› case_negative")).unwrap_or_else(|| panic!("the focused row:\n{text}"));
    assert!( row.contains('✗') && row.contains("0.3 ms"), "{row}");
    assert!(text.contains("✗ failed") && text.contains("src/lib.rs:41 · cargo test · ran "), "its place, provider and when:\n{text}");
    assert!(text.contains("left: -2") && text.contains("thread panicked at src/lib.rs:43:5:") && !text.contains('\u{1b}') && !text.contains("[31m"), "its message and text, cleaned:\n{text}");
    assert_eq!(out.vertical_rules.len(), 1, "the tree and the details apart");
    assert!(out.focus.is_some(), "the focused row drawn as a background");
    assert!(lines(&out)[29].contains("r run") && lines(&out)[29].contains("Enter go to it"), "the keys");
    assert!(crate::page::overflow(&out, 100).is_empty(), "{:?}", crate::page::overflow(&out, 100));
}

/// §7.2's second picture: at 80×24, while a run goes, the head says how
/// far it is and that `x` stops it, then what waits; the details are
/// under the tree.
#[test]
fn at_80_by_24_while_a_run_goes_the_head_says_how_far_and_the_details_are_under() {
    let mut m = orbit();
    assert_ne!(m.request(Selection::Nodes(vec![fenix_tests::TestKey::new(&rust(), LIB)]), Mode::Queue), Requested::Nothing);
    let mut now = 1_000;
    let mut going = None;
    while going.is_none() {
        now += 10;
        match m.next(now).expect("a run") {
            Next::Plan { run, provider, .. } => m.planned(run, Ok(Plan { steps: vec![Step::new(provider.provider.as_ref(), &[])], list_after: false })),
            Next::Step { run, step, .. } => going = Some((run, step)),
            _ => {}
        }
    }
    let (run, step) = going.unwrap();
    m.reports(run, step, vec![Report::Started(ADDS.into()), Report::Ended(Ended::new(NEG, Outcome::Failed))]);
    m.request(Selection::File("src/lib.rs".into()), Mode::Queue);
    let mut p = page();
    let out = lay_out(&mut p, &m, 80, 24);
    let l = lines(&out);
    assert_eq!(l.len(), 24);
    assert!(l[0].contains("running 1 of 3 · ✗ 1 · then: 3 tests") && l[0].trim_end().ends_with("x stops it"), "{}", l[0]);
    assert_eq!(out.vertical_rules.len(), 0, "narrow: nothing beside the tree");
    assert!(out.text.contains("● running") || out.text.contains("● "), "what runs is marked:\n{}", out.text);
    // The details are under the tree, above the keys.
    let rule = out.rules.iter().filter(|(line, _)| *line > 3 && *line < 21).count();
    assert_eq!(rule, 1, "a hairline over the details:\n{}", out.text);
    assert!(l[23].contains("Enter go") && l[23].contains("x stop"), "{}", l[23]);
    for cols in [40, 50, 60, 72, 80, 89, 90, 100, 140] {
        for rows in [10, 24, 30, 50] {
            let out = lay_out(&mut p, &m, cols, rows);
            let over = crate::page::overflow(&out, cols);
            assert!(over.is_empty(), "{cols}×{rows}: {over:#?}\n{}", out.text);
            assert_eq!(out.text.split('\n').count(), rows, "{cols}×{rows}");
        }
    }
}

/// What a provider gives is drawn on one row: escape codes, newlines and
/// tabs in a label or a message never reach the page (§19).
#[test]
fn labels_and_messages_are_cleaned_of_newlines_and_escape_codes() {
    let mut m = Model::new();
    m.add_provider(&rust(), "cargo\ntest");
    m.scan(&rust(), "a.rs", vec![Found::new("g", None, "a\u{1b}[1mgroup\u{1b}[0m\nwith a newline", Kind::Group), test("g t", "g", "the\ttest\r\nlabel", "a.rs", 1)]);
    run(&mut m, Selection::All, |_| {
        let mut e = Ended::new("g t", Outcome::Errored);
        e.message = Some("why\u{1b}]8;;http://x\u{7}it\u{1b}]8;;\u{7}\nfailed".into());
        vec![Report::Ended(e)]
    });
    let mut p = page();
    p.key(Key::Char('j'), Some(&m));
    let out = lay_out(&mut p, &m, 100, 20);
    assert!(out.text.contains("agroup with a newline") && out.text.contains("the test label"), "{}", out.text);
    assert!(out.text.contains("! whyit failed"), "the row's why, on one line: {}", out.text);
    assert!(!out.text.contains('\u{1b}') && !out.text.contains('\t') && !out.text.contains('\r') && !out.text.contains("8;;"), "{:?}", out.text);
    assert_eq!(clean("a\n\nb\u{1b}[0m c"), "a b c");
}

/// The keys of §7.3: moving, opening and closing, `Enter` going to a test
/// and folding a group, the runs, the tabs, the filter, the failures.
#[test]
fn the_keys_move_fold_run_filter_and_find_the_failures() {
    let m = after_a_run();
    let mut p = page();
    lay_out(&mut p, &m, 100, 30);
    let label = |p: &TestsPage| label(p, &m);
    assert_eq!(label(&p), "provider pytest", "the first row: the providers by id");
    assert_eq!(p.key(Key::Char('R'), Some(&m)), Action::RunAll);
    assert_eq!(p.key(Key::Char('f'), Some(&m)), Action::RunFailed);
    assert_eq!(p.key(Key::Char('.'), Some(&m)), Action::RunLast);
    assert_eq!(p.key(Key::Char('x'), Some(&m)), Action::Stop);
    assert_eq!(p.key(Key::Char('o'), Some(&m)), Action::Output);
    assert_eq!(p.key(Key::Char('u'), Some(&m)), Action::List);
    assert_eq!(p.key(Key::Char('q'), Some(&m)), Action::Close);
    // `h` on an open row closes it, `l` opens it again.
    focus_on(&mut p, &m, "orbit (lib)");
    let rows = p.rows().len();
    p.key(Key::Char('h'), Some(&m));
    assert!(p.rows().len() < rows, "closed");
    p.key(Key::Char('l'), Some(&m));
    assert_eq!(p.rows().len(), rows, "open again");
    // `Enter` on a group folds it; on a test goes to it.
    p.key(Key::Enter, Some(&m));
    assert!(p.rows().len() < rows);
    p.key(Key::Tab, Some(&m));
    assert_eq!(p.rows().len(), rows, "Tab opens it again");
    focus_on(&mut p, &m, "adds");
    let adds = m.find(&fenix_tests::TestKey::new(&rust(), ADDS)).unwrap();
    assert_eq!(p.key(Key::Enter, Some(&m)), Action::GoTo(adds));
    assert_eq!(p.key(Key::Char('r'), Some(&m)), Action::Run(Item::Node(adds)));
    assert_eq!(p.key(Key::Char('d'), Some(&m)), Action::Debug(adds));
    assert!(matches!(p.key(Key::Char('y'), Some(&m)), Action::Say(_)), "a test that passed has no failure to copy");
    // `h` on a test goes to its group.
    p.key(Key::Char('h'), Some(&m));
    assert_eq!(label(&p), "tests");
    // `n`, `N`: the failures, in display order, wrapping.
    p.key(Key::Char('n'), Some(&m));
    assert_eq!(label(&p), "case_negative");
    let neg = m.find(&fenix_tests::TestKey::new(&rust(), NEG)).unwrap();
    assert_eq!(p.key(Key::Char('y'), Some(&m)), Action::Copy(neg));
    p.key(Key::Char('n'), Some(&m));
    assert_eq!(label(&p), "test_fixture");
    p.key(Key::Char('n'), Some(&m));
    assert_eq!(label(&p), "case_negative", "round again");
    p.key(Key::Char('N'), Some(&m));
    assert_eq!(label(&p), "test_fixture");
    // `G`, `gg`.
    p.key(Key::Char('G'), Some(&m));
    let last = p.rows().len() - 1;
    p.key(Key::Char('g'), Some(&m));
    p.key(Key::Char('g'), Some(&m));
    assert_eq!(label(&p), "provider pytest", "gg from row {last}");
    // `zM` folds all, `zR` opens all.
    p.key(Key::Char('z'), Some(&m));
    p.key(Key::Char('M'), Some(&m));
    assert_eq!(p.rows().len(), 2, "the providers' rows alone");
    p.key(Key::Char('z'), Some(&m));
    p.key(Key::Char('R'), Some(&m));
    assert_eq!(p.rows().len(), 2 + 1 + 1 + 3 + 1 + 2, "every row");
    // The tabs.
    p.key(Key::Char('2'), Some(&m));
    let out = lay_out(&mut p, &m, 100, 30);
    assert!(out.text.contains("case_negative") && out.text.contains("test_fixture") && !out.text.contains("  adds"), "Failed: failed and errored, with their groups:\n{}", out.text);
    p.key(Key::Char('3'), Some(&m));
    let out = lay_out(&mut p, &m, 100, 30);
    assert!(out.text.contains("ignored_one") && !out.text.contains("case_negative"), "Not run: skipped and unknown:\n{}", out.text);
    p.key(Key::Char('1'), Some(&m));
    // The filter: typed, kept with Enter, cleared with Esc.
    p.key(Key::Char('/'), Some(&m));
    assert!(p.typing());
    for c in "FIXTURE".chars() {
        p.key(Key::Char(c), Some(&m));
    }
    let out = lay_out(&mut p, &m, 100, 30);
    assert!(out.text.contains("test_fixture") && !out.text.contains("case_negative"), "matches, case-insensitive, with their groups:\n{}", out.text);
    assert_eq!(p.key(Key::Char('q'), Some(&m)), Action::None, "while typing, q is a letter");
    p.key(Key::Backspace, Some(&m));
    p.key(Key::Enter, Some(&m));
    assert!(!p.typing() && !p.filter.is_empty());
    p.key(Key::Escape, Some(&m));
    assert!(p.filter.is_empty(), "Esc clears it");
}

/// With no provider the page says so and offers nothing to run; a filter
/// that matches nothing, or a tab with nothing in it, says that.
#[test]
fn an_empty_page_says_why() {
    let m = Model::new();
    let mut p = page();
    let out = lay_out(&mut p, &m, 100, 20);
    assert!(out.text.contains("no test provider here") && out.text.contains("No test provider serves this project"), "{}", out.text);
    assert_eq!(p.key(Key::Char('r'), Some(&m)), Action::None);
    let m = orbit();
    let out = lay_out(&mut p, &m, 100, 20);
    assert!(out.text.contains("? 5"), "never run: {}", out.text);
    p.key(Key::Char('2'), Some(&m));
    assert!(lay_out(&mut p, &m, 100, 20).text.contains("Nothing failed."));
    p.key(Key::Char('1'), Some(&m));
    p.key(Key::Char('/'), Some(&m));
    p.key(Key::Char('Q'), Some(&m));
    assert!(lay_out(&mut p, &m, 100, 20).text.contains("Nothing matches the filter."));
}

/// The details scroll with `J` and `K`, and say there's more.
#[test]
fn the_details_scroll() {
    let mut m = Model::new();
    m.add_provider(&rust(), "cargo test");
    m.scan(&rust(), "a.rs", vec![test("t", "g", "t", "a.rs", 1)]);
    run(&mut m, Selection::All, |_| {
        let mut e = Ended::new("t", Outcome::Failed);
        e.text = Some((0..60).map(|i| format!("line {i}\n")).collect());
        vec![Report::Ended(e)]
    });
    let mut p = page();
    let out = lay_out(&mut p, &m, 100, 20);
    assert!(out.text.contains("line 0") && out.text.contains("J shows more") && !out.text.contains("line 40"), "{}", out.text);
    for _ in 0..5 {
        p.key(Key::Char('J'), Some(&m));
    }
    let out = lay_out(&mut p, &m, 100, 20);
    assert!(!out.text.contains("line 2\n") && out.text.contains("line 5"), "{}", out.text);
    p.key(Key::Char('K'), Some(&m));
    let text = lay_out(&mut p, &m, 100, 20).text;
    assert!(text.contains("line 4") && !text.contains("line 3\n"), "{text}");
}

/// §7.5: at 10,000 tests, a layout writes only what's in view; laying out
/// again with nothing changed works out no rows again; opening all of
/// them still lays out a pane's worth.
#[test]
fn ten_thousand_tests_cost_what_is_in_view() {
    let mut m = Model::new();
    m.add_provider(&rust(), "cargo test");
    for f in 0..100 {
        let path = format!("src/f{f}.rs");
        let mut found = vec![Found::new(format!("f{f}"), None, path.clone(), Kind::File).at(&path, 0, None)];
        found.extend((0..100).map(|t| test(&format!("f{f}::t{t}"), &format!("f{f}"), &format!("t{t}"), &path, t * 5 + 1)));
        m.scan(&rust(), &path, found);
    }
    assert_eq!(m.summary().counts.tests(), 10_000);
    let mut p = page();
    let started = std::time::Instant::now();
    let out = lay_out(&mut p, &m, 100, 30);
    assert_eq!(p.rows().len(), 100, "groups start closed: one row a file");
    assert_eq!(out.text.split('\n').count(), 30);
    p.key(Key::Char('z'), Some(&m));
    p.key(Key::Char('R'), Some(&m));
    assert_eq!(p.rows().len(), 10_100, "all open");
    let worked = p.rows_key.clone();
    let summary = m.summary();
    for _ in 0..200 {
        let out = layout(&mut p, &View { model: &m, summary: &summary, notes: Vec::new(), clock: &clock }, 100, Some(30));
        assert_eq!(out.spans.iter().map(|s| s.line).max().unwrap_or(0), 29, "nothing past the pane");
    }
    assert_eq!(p.rows_key, worked, "the rows weren't worked out again");
    // Moving to the last row scrolls the tree, not the page.
    p.key(Key::Char('G'), Some(&m));
    let out = lay_out(&mut p, &m, 100, 30);
    assert!(out.text.contains("t99") && out.focus.is_some_and(|(line, _)| line < 30), "{}", out.text);
    // A filter is one pass over the labels, on the key.
    p.key(Key::Char('/'), Some(&m));
    for c in "f42 t7".chars() {
        p.key(if c == ' ' { Key::Space } else { Key::Char(c) }, Some(&m));
    }
    let out = lay_out(&mut p, &m, 100, 30);
    assert!(out.text.contains("src/f42.rs") && out.text.contains("t77") && !out.text.contains("src/f41.rs"), "{}", out.text);
    assert!(started.elapsed() < std::time::Duration::from_secs(20), "{:?}", started.elapsed());
}


/// A debug launch's build (step 6) is said as one in the head, not as a
/// listing, and `d debug` is on the key strip at both sizes.
#[test]
fn a_build_to_debug_a_test_is_said_as_one() {
    let mut m = orbit();
    let run = m.run_id();
    assert_eq!(m.request_build(run, &rust(), ADDS, Plan { steps: vec![Step::new("build", &[])], list_after: false }), Requested::Starts);
    assert!(matches!(m.next(1_000), Some(Next::Step { .. })));
    let mut p = page();
    for (cols, rows) in [(100, 30), (80, 24)] {
        let out = lay_out(&mut p, &m, cols, rows);
        let l = lines(&out);
        assert!(l[0].contains("building to debug…") && !l[0].contains("listing"), "{}", l[0]);
        assert!(l[rows - 1].contains("d debug"), "{}", l[rows - 1]);
    }
}
