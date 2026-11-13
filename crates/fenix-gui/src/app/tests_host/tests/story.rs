//! Step 7's tests (plan `docs/plans/test-explorer.md` §2, §10, §20 "Step
//! 7"): each door routed through the explorer where a provider serves the
//! file, today's way where none does, and as in 1.5 with the explorer off;
//! `last_task`, `task-started`/`task-ended` and the notification once; the
//! agents' tools, their shapes, budgets and refusals; the control
//! channel's methods.

use super::*;
use crate::agent_tools::TESTS_TOOLS;
use crate::app::tests::{answer, capabilities, install_server, only_pending};
use fenix_control::AgentCaller;
use serde_json::{json, Value};

fn off(p: &mut Project) {
    p.app.config.set_value("tests.enabled", Some(fenix_config::Value::Bool(false)));
}

fn at_line(p: &mut Project, line: usize) {
    let at = p.app.open().buffer.line_start_char(line);
    p.app.test_set_cursor(Cursor { char_idx: at, sticky_col: 0 });
}

fn test_runs(p: &Project) -> usize {
    p.app.tests.projects.get(&p.root).map_or(0, |t| t.ended_runs.iter().filter(|e| matches!(e.kind, fenix_tests::RunKind::Tests(_))).count())
}

/// The notifications from test runs, in the history: (summary, body,
/// actions' commands), each as many times as it came.
fn notified(p: &Project) -> Vec<(String, String, Vec<String>)> {
    p.app.notifications.history().iter().filter(|n| n.source == "tests").flat_map(|n| std::iter::repeat_n((n.summary.clone(), n.body.clone(), n.actions.iter().map(|a| a.command.clone()).collect()), n.count as usize)).collect()
}

/// A project that names a program, not allowed yet: not trusted (§12).
fn untrusted(p: &mut Project, state: &Path) {
    std::fs::write(p.root.join(".fenix").join("tools.json"), r#"{"tasks": {"build": {"executable": "make"}}}"#).unwrap();
    p.app.known_projects = fenix_project::KnownProjects::load_or_default(state.join("projects.txt"));
    p.app.project_trust_begin(Some(state.join("trusted-projects.json")));
    assert!(!p.app.project_is_trusted(&p.root));
}

// SPC p r and SPC p T.

/// `SPC p r` in a file a provider serves runs the test the cursor is in
/// through the explorer (no Task Output), says why on a line in no test,
/// and `SPC p T` runs that test again; in a file no provider serves it's
/// today's task, and `SPC p T` then runs that task. Off, both are today's.
#[test]
fn spc_p_r_and_spc_p_t_go_through_the_explorer_where_a_provider_serves_the_file() {
    let state = tempfile::tempdir().unwrap();
    let a = "\n# test adds\n#! out PASS a.fake::adds 2ms\n";
    let b = "def test_totals():\n    assert 1 == 1\n";
    let files = [("a.fake", a), ("test_b.py", b), ("pyproject.toml", "[project]\nname = \"b\"\n")];
    let mut p = Project::new(&files, "a.fake", state.path());
    p.scanned("a.fake::adds");
    // Above its first test: the explorer's word, not the finder's.
    p.app.run_test_at_cursor();
    assert_eq!(p.said(), "line 1 of a.fake isn't in a test -- tests.run_file runs this file's");
    at_line(&mut p, 1);
    p.app.run_cmd("task.run_test_at_cursor");
    p.until("SPC p r's run", |p| test_runs(p) == 1);
    assert_eq!(p.state("a.fake::adds"), State::Passed);
    assert!(p.app.task_session.is_none(), "not the task pane");
    p.app.run_cmd("task.rerun_last");
    p.until("SPC p T's run", |p| test_runs(p) == 2);
    assert!(p.app.task_session.is_none());
    // A file no provider found tests in (the guest scans *.py, and finds
    // none in it): the finder's test, in Task Output.
    let path = p.root.join("test_b.py");
    assert_eq!(p.app.run_test_at(&path, 1).unwrap(), "test test_totals");
    let session = p.app.task_session.as_ref().expect("Task Output opened");
    assert!(p.app.buffers.get(session.buffer).unwrap().buffer.text().starts_with("$ pytest test_b.py::test_totals"));
    // SPC p T: what SPC p r ran last, that task.
    p.app.rerun_last_task();
    assert_eq!(test_runs(&p), 2, "no test run");
    assert!(p.app.buffers.get(p.app.task_session.as_ref().unwrap().buffer).unwrap().buffer.text().starts_with("$ pytest test_b.py::test_totals"));

    // Off: today's for both files, nothing asked of a provider.
    let mut q = Project::new(&files, "a.fake", state.path());
    off(&mut q);
    let a_path = q.root.join("a.fake");
    let finder = fenix_tasks::test_at(&a_path, a, 2, &tool_sessions::root_for_path(&a_path)).unwrap_err();
    assert_eq!(q.app.run_test_at(&a_path, 2).unwrap_err(), finder, "the finder's own word");
    assert_eq!(q.app.run_test_at(&q.root.join("test_b.py"), 1).unwrap(), "test test_totals");
    q.app.rerun_last_task();
    assert!(q.app.tests.projects.is_empty() && q.app.tests.calls.is_empty() && q.app.tests.last_door.is_empty(), "nothing of the explorer's");
}

// The lenses.

/// The own lenses of `id` (the file at `path`), laid out: by line, each
/// row's text.
fn lens_rows(app: &mut App, id: BufferId) -> Vec<(usize, String)> {
    app.lay_out_lenses(id);
    app.lens_rows.get(&id).map(|rows| rows.iter().map(|(line, row)| (*line, row.text.clone())).collect()).unwrap_or_default()
}

/// A buffer with code lens on and no language server: its own lenses only.
fn lensed_buffer(app: &mut App, path: &Path) -> BufferId {
    app.config.set_value("lsp.code_lens", Some(fenix_config::Value::Bool(true)));
    let id = app.buffers.id_for_path(path).expect("open");
    app.lens_of.insert(id, (Vec::new(), fenix_lsp::normalize(std::fs::canonicalize(path).unwrap())));
    id
}

/// Fenix's own lens over a file a provider serves (§9.5): "▶ Run test |
/// Debug" over each test, "▶ Run test" over a group and where its
/// extension has no `tests-launch`; *Run* runs it through the explorer,
/// *Debug* debugs it -- only in a trusted project. The right click offers
/// *Debug test* only where it then debugs. A file no provider serves
/// keeps today's lens, and off, every file does.
#[test]
fn fenix_s_own_lens_runs_and_debugs_through_the_explorer_where_a_provider_serves_the_file() {
    let state = tempfile::tempdir().unwrap();
    let a = "\n# suite s\n# test adds\n# each launched\n#! out PASS a.py::adds 1ms\n#! out PASS a.py::launched 1ms\n";
    let t = "#[test]\nfn adds() {}\n";
    let (mut p, launches) = Project::debugging(&[("a.py", a), ("t.rs", t)], "a.py", state.path());
    p.scanned("a.py::launched");
    let path = p.root.join("a.py");
    let id = lensed_buffer(&mut p.app, &path);
    assert_eq!(lens_rows(&mut p.app, id), vec![(1, "▶ Run test".to_string()), (2, "▶ Run test | Debug".into()), (3, "▶ Run test | Debug".into())]);
    // The right click: Debug test on a test, not on a group.
    let labels = |p: &Project| p.app.text_menu_items().into_iter().map(|i| i.label).collect::<Vec<_>>();
    at_line(&mut p, 1);
    assert!(labels(&p).contains(&"Run test".to_string()) && !labels(&p).contains(&"Debug test".to_string()), "a suite: {:?}", labels(&p));
    at_line(&mut p, 2);
    assert!(labels(&p).contains(&"Debug test".to_string()));
    // Run, on adds' line: through the explorer.
    let canonical = p.app.lens_of[&id].1.clone();
    let pick = |p: &Project, index: usize| p.app.lens_pick(id, &None, &canonical, index).unwrap();
    let run = pick(&p, 1);
    p.app.run_lens(&run);
    p.until("the lens's run", |p| test_runs(p) == 1);
    assert_eq!(p.state("a.py::adds"), State::Passed);
    assert!(p.app.task_session.is_none());
    // Debug, on launched's: its launch started.
    let debug = pick(&p, 4);
    p.app.run_lens(&debug);
    let launch = p.launched(&launches);
    assert_eq!(launch["args"][0], "a.py::launched");
    p.app.debug_stop();
    // An extension with no tests-launch: no Debug, on the lens or the menu.
    p.app.tests.no_launch.insert(EXT.to_string());
    p.app.tests_lenses_due(None);
    assert_eq!(lens_rows(&mut p.app, id), vec![(1, "▶ Run test".to_string()), (2, "▶ Run test".into()), (3, "▶ Run test".into())]);
    at_line(&mut p, 2);
    assert!(!labels(&p).contains(&"Debug test".to_string()), "{:?}", labels(&p));
    p.app.tests.no_launch.clear();
    // A file no provider serves: the finder's lens, no Debug.
    p.open("t.rs");
    let t_id = lensed_buffer(&mut p.app, &p.root.join("t.rs"));
    let today = lens_rows(&mut p.app, t_id);
    assert!(!today.is_empty() && today.iter().all(|(_, text)| text == "▶ Run test"), "{today:?}");
    // Off: as before, here and there.
    off(&mut p);
    p.app.tests_tick(Instant::now());
    p.app.tests_views_due(Instant::now());
    assert_eq!(lens_rows(&mut p.app, t_id), today);
    assert!(lens_rows(&mut p.app, id).is_empty(), "a.py has no test the finder knows");
}

/// The lens's *Debug* keeps the lens's rule (§9.5): in a project that
/// isn't trusted it starts nothing -- not even its provider is asked --
/// where `SPC T d`, a key, still debugs (§12).
#[test]
fn the_lens_s_debug_starts_nothing_in_a_project_that_isnt_trusted() {
    let state = tempfile::tempdir().unwrap();
    let a = "# each launched\n";
    let (mut p, _launches) = Project::debugging(&[("a.fake", a)], "a.fake", state.path());
    p.scanned("a.fake::launched");
    untrusted(&mut p, state.path());
    let path = p.root.join("a.fake");
    let id = lensed_buffer(&mut p.app, &path);
    assert_eq!(lens_rows(&mut p.app, id), vec![(0, "▶ Run test | Debug".to_string())]);
    let canonical = p.app.lens_of[&id].1.clone();
    for index in [0, 1] {
        let pick = p.app.lens_pick(id, &None, &canonical, index).unwrap();
        p.app.run_lens(&pick);
        assert!(p.said().contains("only in a trusted project (SPC p y)"), "{}", p.said());
        assert!(p.tests().debug.is_none() && p.tests().run.is_none() && p.model().summary().waiting.is_none(), "nothing asked");
    }
    p.app.tests_debug_here();
    assert!(p.tests().debug.is_some(), "a key debugs");
}

/// rust-analyzer's run and debug lenses on a line the explorer has a test
/// at go through it; a binary's run (not `cargo test`) stays a task; off,
/// both are as before.
#[test]
fn rust_analyzer_s_lenses_on_an_explorer_s_test_go_through_it() {
    let state = tempfile::tempdir().unwrap();
    let lib = "// a library\n# test adds\n#! out PASS lib.rs::adds 1ms\n";
    let mut p = Project::new(&[("lib.rs", lib)], "lib.rs", state.path());
    // The guest, scanning Rust files too.
    p.app.wasm.installed[0].manifest.contributes.test_providers[0].scan.push("*.rs".into());
    p.scanned("lib.rs::adds");
    let path = p.root.join("lib.rs");
    p.app.config.set_value("lsp.code_lens", Some(fenix_config::Value::Bool(true)));
    let key = install_server(&mut p.app, &path, LspServer::Primary(fenix_syntax::LanguageId::Rust), "rust-analyzer", &["rust"], capabilities(json!({"textDocumentSync": 2, "codeLensProvider": {}})));
    p.app.sync_lsp_for_focused_buffer();
    p.app.ask_due_lenses();
    let (request, _) = only_pending(&p.app, &key);
    let runnable = |args: &[&str]| json!({ "label": "x", "kind": "cargo", "args": { "cargoArgs": args } });
    let lens = |line: u32, title: &str, command: &str, args: &[&str]| json!({ "range": { "start": { "line": line, "character": 0 }, "end": { "line": line, "character": 2 } }, "command": { "title": title, "command": command, "arguments": [runnable(args)] } });
    answer(&mut p.app, &key, request, json!([lens(0, "▶ Run", "rust-analyzer.runSingle", &["--version"]), lens(1, "▶ Run Test", "rust-analyzer.runSingle", &["test", "--lib", "--", "adds", "--exact"]), lens(1, "Debug", "rust-analyzer.debugSingle", &["test", "--lib", "--", "adds", "--exact"])]));
    let id = p.app.focused_buffer_id();
    let canonical = p.app.lens_of[&id].1.clone();
    let pick = |p: &Project, index: usize| p.app.lens_pick(id, &Some(key.clone()), &canonical, index).unwrap();
    let run = pick(&p, 1);
    p.app.run_lens(&run);
    p.until("the lens's run", |p| test_runs(p) == 1);
    assert_eq!(p.state("lib.rs::adds"), State::Passed);
    assert!(p.app.task_session.is_none(), "not cargo in Task Output");
    let debug = pick(&p, 2);
    p.app.run_lens(&debug);
    assert_eq!(p.said(), "asking provider how to debug adds");
    p.until("its answer", |p| p.tests().debug.is_none());
    // A binary's run: cargo, in Task Output, as before.
    let binary = pick(&p, 0);
    p.app.run_lens(&binary);
    assert!(p.app.task_session.is_some(), "{}", p.said());
    // Off: the test's run lens is cargo's, its debug lens as before.
    let mut q = Project::new(&[("lib.rs", lib)], "lib.rs", state.path());
    off(&mut q);
    q.app.config.set_value("lsp.code_lens", Some(fenix_config::Value::Bool(true)));
    let key = install_server(&mut q.app, &q.root.join("lib.rs"), LspServer::Primary(fenix_syntax::LanguageId::Rust), "rust-analyzer", &["rust"], capabilities(json!({"textDocumentSync": 2, "codeLensProvider": {}})));
    q.app.sync_lsp_for_focused_buffer();
    q.app.ask_due_lenses();
    let (request, _) = only_pending(&q.app, &key);
    answer(&mut q.app, &key, request, json!([lens(1, "Debug", "rust-analyzer.debugSingle", &["test", "--lib", "--", "adds", "--exact"])]));
    let id = q.app.focused_buffer_id();
    let canonical = q.app.lens_of[&id].1.clone();
    let debug = q.app.lens_pick(id, &Some(key.clone()), &canonical, 0).unwrap();
    q.app.run_lens(&debug);
    assert!(q.said().contains("can't debug from a lens yet"), "{}", q.said());
    assert!(q.app.tests.projects.is_empty());
}

// What a test run tells the rest of Fenix.

/// A run of tests is `last_task` when it ends, with the model's counts;
/// extensions hear `task-started` and `task-ended` once each (Q1); one
/// notification comes, as `notifications.tasks` says, its action showing
/// the failures. A listing is none of these; a stopped run tells its end
/// but leaves `last_task` and notifies nothing, as a cancelled task.
#[test]
fn a_test_run_is_the_last_task_tells_extensions_once_and_notifies_once() {
    let state = tempfile::tempdir().unwrap();
    let a = "# test adds\n# test fails\n#! out PASS a.fake::adds 3ms\n#! err FAIL a.fake::fails a.fake:1 two and two\n";
    let slow = "# test slow\n#! wait\n";
    let fine = "# test fine\n#! out PASS ok.fake::fine 1ms\n";
    let mut p = Project::new(&[("a.fake", a), ("slow.fake", slow), ("ok.fake", fine)], "a.fake", state.path());
    p.app.config.set_value("notifications.tasks", Some(fenix_config::Value::Text("all".into())));
    p.scanned("a.fake::fails");
    p.app.tests_run_file();
    p.until("the run", |p| test_runs(p) == 1);
    for _ in 0..5 {
        p.app.tests_tick(Instant::now());
    }
    assert_eq!(p.app.tests.told, ["started tests: a.fake", "ended false"]);
    assert_eq!(p.app.wasm.last_task, Some(fenix_wasm::host::TaskResult { name: "tests: a.fake".into(), ok: false, tests: Some((1, 1)) }));
    assert_eq!(notified(&p), vec![("1 test failed".to_string(), "fails · 1 passed".to_string(), vec!["tests.open".to_string(), "tests.run_failed".to_string()])]);
    // The notification's action shows the failures: the tests page.
    p.app.run_cmd(&notified(&p)[0].2[0]);
    assert!(p.app.find_page(|m| matches!(m, PageModel::Tests(_))).is_some());
    // A listing: none of it.
    let listed = p.listings();
    p.app.tests_list(&p.root.clone());
    p.until("the listing", |p| p.listings() > listed);
    assert_eq!(p.app.tests.told.len(), 2);
    assert_eq!(notified(&p).len(), 1);
    // `failed`: a run that passes isn't notified, but is the last task.
    p.app.config.set_value("notifications.tasks", Some(fenix_config::Value::Text("failed".into())));
    p.open("ok.fake");
    p.scanned("ok.fake::fine");
    let root = p.root.clone();
    p.app.tests_request(&root, vec![Selection::Nodes(vec![TestKey::new(&provider(), "ok.fake::fine")])], "fine").unwrap();
    p.until("fine's run", |p| test_runs(p) == 2);
    assert_eq!(p.app.tests.told[2..], ["started tests: fine", "ended true"]);
    assert_eq!(p.app.wasm.last_task.as_ref().map(|t| (t.name.as_str(), t.ok, t.tests)), Some(("tests: fine", true, Some((1, 0)))));
    assert_eq!(notified(&p).len(), 1);
    // Stopped: its end told, nothing else.
    p.open("slow.fake");
    p.scanned("slow.fake::slow");
    p.app.tests_run_file();
    p.until("the slow step", Project::step_running);
    p.app.run_cmd("tests.stop");
    p.until("the stopped run", |p| test_runs(p) == 3);
    assert_eq!(p.app.tests.told[4..], ["started tests: slow.fake", "ended false"]);
    assert_eq!(p.app.wasm.last_task.as_ref().map(|t| t.name.as_str()), Some("tests: fine"), "left as it was");
    assert_eq!(notified(&p).len(), 1);
}

/// A target that didn't build is said as that (§10.1), its action the
/// output; under `long`, a quick run says nothing.
#[test]
fn a_run_that_didnt_build_notifies_as_that_and_a_quick_one_under_long_doesnt() {
    let state = tempfile::tempdir().unwrap();
    let bad = "# test x\n#! out BROKEN bad.fake mismatched types\n";
    let mut p = Project::new(&[("bad.fake", bad)], "bad.fake", state.path());
    p.scanned("bad.fake::x");
    p.app.tests_run_file();
    p.until("the run", |p| test_runs(p) == 1);
    assert!(notified(&p).is_empty(), "long, the default: a quick run isn't notified");
    p.app.config.set_value("notifications.tasks", Some(fenix_config::Value::Text("failed".into())));
    p.app.tests_run_file();
    p.until("the second run", |p| test_runs(p) == 2);
    assert_eq!(notified(&p), vec![("The tests didn't build".to_string(), "mismatched types".to_string(), vec!["tests.output".to_string()])]);
    assert_eq!(p.app.wasm.last_task.as_ref().map(|t| t.ok), Some(false));
}

/// `SPC T k` stops the run going and lets go the debug waiting behind it,
/// one key for two: it says both (step 6's review).
#[test]
fn one_stop_says_the_run_and_the_debug_behind_it() {
    let state = tempfile::tempdir().unwrap();
    let slow = "# test slow\n#! wait\n";
    let b = "# test build_x\n";
    let mut p = Project::new(&[("slow.fake", slow), ("b.fake", b)], "slow.fake", state.path());
    p.scanned("slow.fake::slow");
    p.app.tests_run_file();
    p.until("the slow step", Project::step_running);
    p.open("b.fake");
    p.scanned("b.fake::build_x");
    p.app.tests_debug_here();
    p.until("the build waiting", |p| p.tests().debug.as_ref().is_some_and(|d| d.building));
    p.app.run_cmd("tests.stop");
    assert_eq!(p.said(), "stopping the test run, and the debug of build_x waiting behind it");
    p.until("the stopped run", |p| test_runs(p) == 1);
    assert!(p.tests().debug.is_none() && p.app.debug_session.is_none());
    assert!(p.tests().ended_runs.iter().all(|e| !matches!(e.kind, fenix_tests::RunKind::Build { .. })), "the build never ran");
    p.app.run_cmd("tests.stop");
    assert_eq!(p.said(), "no test run to stop");
}

// The agents' tools.

/// An agent's call, as a client Fenix didn't start (allowed to run by
/// `client:run`), pumped until it's answered.
fn agent(p: &mut Project, tool: &str, arguments: Value) -> Result<Value, fenix_control::RpcError> {
    let (reply, answered) = mpsc::channel();
    let caller = AgentCaller { client: "fenix-mcp".into(), thread: None };
    p.app.control_agent("call", caller, json!({ "call": "1", "tool": tool, "arguments": arguments }), reply);
    let started = Instant::now();
    loop {
        if let Ok(answer) = answered.try_recv() {
            return answer.map(|a| a["answer"].clone());
        }
        assert!(started.elapsed() < Duration::from_secs(120), "{tool}: no answer\n{}", p.output());
        std::thread::sleep(Duration::from_millis(5));
        p.app.pump_wasm();
        p.app.tests_tick(Instant::now());
        p.app.agent_calls_tick(Instant::now());
        let events: Vec<_> = p.app.dap_unread.iter().flat_map(|(generation, rx)| rx.try_iter().map(|e| (*generation, e)).collect::<Vec<_>>()).collect();
        for (generation, event) in events {
            p.app.handle_user_event(FenixUserEvent::Dap { generation, event });
        }
    }
}

fn allow_runs(p: &mut Project) {
    p.app.config.set_value("agents.mcp.allow", Some(fenix_config::Value::Map(vec![("client:run".into(), "allow".into())])));
}

fn listed(p: &mut Project) -> Vec<String> {
    let (reply, answered) = mpsc::channel();
    p.app.control_agent("tools", AgentCaller { client: "fenix-mcp".into(), thread: None }, json!({}), reply);
    answered.recv_timeout(Duration::from_secs(5)).unwrap().unwrap()["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap().to_string()).collect()
}

/// `tests`, `test_failures`, `run_tests` and `run_test`: their answers'
/// shapes, as data; the list and the failures held to their budgets
/// (500 tests, 4 KiB a failure's text, `agents.mcp.max_answer_kib` in
/// all), saying how many were left out.
#[test]
fn the_agents_tools_answer_the_explorers_model_as_data_within_their_budget() {
    let state = tempfile::tempdir().unwrap();
    let long = "w".repeat(6000);
    let a = format!("# test adds\n# test fails\n#! out PASS a.fake::adds 3ms\n#! err FAIL a.fake::fails a.fake:1 two and two {long}\n");
    let mut p = Project::new(&[("a.fake", a.as_str()), ("many.fake", "#many\n")], "a.fake", state.path());
    allow_runs(&mut p);
    p.scanned("a.fake::fails");
    let names = listed(&mut p);
    for tool in ["tests", "test_failures", "run_tests", "run_test"] {
        assert!(names.contains(&tool.to_string()), "{tool}: {names:?}");
    }
    // tests: each with its place and state, lines from 1.
    let tests = agent(&mut p, "tests", json!({ "path": "a.fake" })).unwrap();
    assert_eq!(tests["tests"], json!([
        { "provider": "provider", "id": "a.fake::adds", "path": "a.fake", "line": 1, "state": "not_run", "durationMs": null, "stale": false },
        { "provider": "provider", "id": "a.fake::fails", "path": "a.fake", "line": 2, "state": "not_run", "durationMs": null, "stale": false },
    ]));
    // run_tests: waits for the run, then its counts and failures.
    let ran = agent(&mut p, "run_tests", json!({ "path": "a.fake" })).unwrap();
    assert_eq!((ran["passed"].as_u64(), ran["failed"].as_u64(), ran["errored"].as_u64(), ran["halted"].clone()), (Some(1), Some(1), Some(0), Value::Null), "{ran}");
    let failure = &ran["failures"][0];
    assert_eq!((failure["id"].as_str(), failure["message"].as_str().map(|m| m.starts_with("two and two")), failure["at"].clone()), (Some("a.fake::fails"), Some(true), json!({ "path": "a.fake", "line": 2, "column": null })));
    let text = failure["text"].as_str().unwrap();
    assert!(text.len() < 4 * 1024 + 64 && text.ends_with("bytes more]"), "a failure's text is cut at 4 KiB: {} bytes", text.len());
    // test_failures: the same, and by state in tests.
    let failures = agent(&mut p, "test_failures", json!({})).unwrap();
    assert_eq!(failures["failures"].as_array().unwrap().len(), 1);
    let failed = agent(&mut p, "tests", json!({ "state": "failed" })).unwrap();
    assert_eq!(failed["tests"][0]["id"], "a.fake::fails");
    assert_eq!(agent(&mut p, "tests", json!({ "state": "red" })).unwrap_err().message, "state red: passed, failed, errored, skipped, not_run, queued or running");
    // The answer's whole held to max_answer_kib: the rest counted.
    p.app.config.set_value("agents.mcp.max_answer_kib", Some(fenix_config::Value::Int(4)));
    let held = agent(&mut p, "test_failures", json!({})).unwrap();
    assert_eq!((held["failures"].as_array().unwrap().len(), held["more"].as_u64()), (0, Some(1)), "{held}");
    // 10,000 tests (the scan's cap): 500, and how many more.
    p.open("many.fake");
    p.scanned("many.fake::t0");
    let many = agent(&mut p, "tests", json!({ "path": "many.fake" })).unwrap();
    assert_eq!((many["tests"].as_array().unwrap().len(), many["total"].as_u64(), many["more"].as_u64()), (500, Some(10_000), Some(9_500)));
    assert!(many.to_string().len() < 64 * 1024, "{} bytes", many.to_string().len());
    // run_test at a line of a served file: its result.
    let one = { let arguments = json!({ "path": p.root.join("a.fake").to_string_lossy().into_owned(), "line": 1 }); agent(&mut p, "run_test", arguments) }.unwrap();
    assert_eq!((one["test"].clone(), one["state"].as_str(), one["durationMs"].as_f64()), (json!({ "provider": "provider", "id": "a.fake::adds" }), Some("passed"), Some(3.0)), "{one}");
    let output = one["output"].as_str().unwrap();
    assert!(output.len() <= 4 * 1024 + 32 && output.starts_with('[') && output.ends_with("--\n"), "the run's output's end, 4 KiB: {output}");
    // run_tests with a test by its id, and one that isn't there.
    let by_id = agent(&mut p, "run_tests", json!({ "tests": [{ "provider": "provider", "id": "a.fake::adds" }] })).unwrap();
    // (The made-up runner prints its file's every line: fails' too.)
    assert_eq!((by_id["passed"].as_u64(), by_id["run"].as_u64().is_some()), (Some(1), true), "{by_id}");
    assert!(agent(&mut p, "run_tests", json!({ "tests": [{ "provider": "provider", "id": "nope" }] })).unwrap_err().message.contains("no test nope"));
    assert!(agent(&mut p, "run_tests", json!({})).unwrap_err().message.contains("say what to run"));
}

/// Past its time, `run_tests` says it's still running, with what's in.
#[test]
fn run_tests_past_its_time_says_still_running() {
    let state = tempfile::tempdir().unwrap();
    let mut p = Project::new(&[("slow.fake", "# test slow\n#! wait\n")], "slow.fake", state.path());
    allow_runs(&mut p);
    p.scanned("slow.fake::slow");
    let going = agent(&mut p, "run_tests", json!({ "path": "slow.fake", "timeout": 1 })).unwrap();
    assert_eq!((going["running"].clone(), going["note"].as_str().map(|n| n.starts_with("still running"))), (json!(true), Some(true)), "{going}");
    p.app.run_cmd("tests.stop");
    p.until("the stopped run", |p| test_runs(p) == 1);
}

/// The run tools are asked about by default and refused to a client Fenix
/// didn't start; allowed, they still run a project's tests only where a
/// lens would (a trusted project). Off, the explorer's tools aren't there
/// and `run_test` is as before.
#[test]
fn the_agents_run_tools_keep_the_rules_and_trust() {
    let state = tempfile::tempdir().unwrap();
    let a = "# test adds\n#! out PASS a.fake::adds 1ms\n";
    let mut p = Project::new(&[("a.fake", a)], "a.fake", state.path());
    p.scanned("a.fake::adds");
    let path = p.root.join("a.fake").to_string_lossy().into_owned();
    // A client Fenix didn't start may read, not run.
    assert!(agent(&mut p, "tests", json!({})).is_ok());
    for (tool, arguments) in [("run_tests", json!({ "all": true })), ("run_test", json!({ "path": path, "line": 1 })), ("debug_test", json!({ "path": path, "line": 1 }))] {
        let refused = agent(&mut p, tool, arguments).unwrap_err().message;
        assert!(refused.contains("refused by your rules"), "{tool}: {refused}");
    }
    // Allowed by the rules, in a project that isn't trusted: not run.
    allow_runs(&mut p);
    untrusted(&mut p, state.path());
    for (tool, arguments) in [("run_tests", json!({ "all": true })), ("run_test", json!({ "path": path, "line": 1 })), ("debug_test", json!({ "path": path, "line": 1 }))] {
        let refused = agent(&mut p, tool, arguments).unwrap_err().message;
        assert!(refused.contains("only in a trusted project (SPC p y)"), "{tool}: {refused}");
    }
    assert!(p.tests().run.is_none() && p.tests().debug.is_none() && test_runs(&p) == 0, "nothing ran");
    // Off: no such tools, and run_test is today's.
    off(&mut p);
    let names = listed(&mut p);
    assert!(TESTS_TOOLS.iter().all(|t| !names.contains(&t.to_string())), "{names:?}");
    assert!(agent(&mut p, "tests", json!({})).unwrap_err().message.contains("Fenix has no tool tests"));
    let today = agent(&mut p, "run_test", json!({ "path": path, "line": 1 })).unwrap_err().message;
    let finder = fenix_tasks::test_at(Path::new(&path), a, 1, &tool_sessions::root_for_path(Path::new(&path))).unwrap_err();
    assert_eq!(today, finder);
}

/// `debug_test` at a test's line: built first, its session started, and
/// answered at the first stop as `debug_start` is (here, the session's
/// end); a test its provider won't debug says why. The build isn't a test
/// run: nothing's told of it.
#[test]
fn debug_test_debugs_a_test_through_the_explorer_and_says_why_not() {
    let state = tempfile::tempdir().unwrap();
    let ok = "# test build_ok\n#! out BUILT /project/target/fake-bin\n";
    let plain = "# test plain\n";
    let (mut p, launches) = Project::debugging(&[("ok.py", ok), ("plain.py", plain)], "ok.py", state.path());
    allow_runs(&mut p);
    p.scanned("ok.py::build_ok");
    let path = p.root.join("ok.py").to_string_lossy().into_owned();
    let (reply, answered) = mpsc::channel();
    p.app.control_agent("call", AgentCaller { client: "fenix-mcp".into(), thread: None }, json!({ "call": "1", "tool": "debug_test", "arguments": { "path": path, "line": 1 } }), reply);
    let launch = p.launched(&launches);
    assert_eq!(launch["args"], json!(["ok.py::build_ok", "--exact"]));
    p.until("the wait for its first stop", |p| p.app.tests.debug_waits.is_empty());
    assert_eq!(p.app.agent_calls.debug_waits.len(), 1, "waits as debug_start does");
    assert!(p.app.tests.told.is_empty(), "a debug's build is no test run");
    p.app.debug_stop();
    let started = Instant::now();
    let answer = loop {
        if let Ok(answer) = answered.try_recv() {
            break answer.unwrap();
        }
        assert!(started.elapsed() < Duration::from_secs(60));
        std::thread::sleep(Duration::from_millis(5));
        p.app.agent_calls_tick(Instant::now());
    };
    assert!(answer["answer"].get("session").is_some(), "the debugger's state: {answer}");
    // Its provider won't: why.
    p.open("plain.py");
    p.scanned("plain.py::plain");
    let refused = { let arguments = json!({ "path": p.root.join("plain.py").to_string_lossy().into_owned(), "line": 1 }); agent(&mut p, "debug_test", arguments) }.unwrap_err().message;
    assert_eq!(refused, "plain can't be debugged: plain.py::plain can't be debugged");
    let refused = { let arguments = json!({ "path": p.root.join("nowhere.rs").to_string_lossy().into_owned(), "line": 1 }); agent(&mut p, "debug_test", arguments) }.unwrap_err().message;
    assert!(refused.starts_with("no test provider serves"), "{refused}");
}

// The control channel.

/// `fenix/tests`, `fenix/tests.failures` and `fenix/tests.run`: each
/// test with its label and kind; the failures as `test_failures`; a run
/// started and its id said at once, only with `agents.mcp.fenix_write`
/// on, in a trusted project. Off, none of them is there.
#[test]
fn the_control_channel_lists_runs_and_reads_failures() {
    use fenix_control::{Call, TestRef, TestsParams, TestsRunParams};
    let state = tempfile::tempdir().unwrap();
    let a = "# test adds\n# test fails\n#! out PASS a.fake::adds 3ms\n#! err FAIL a.fake::fails a.fake:1 two and two\n";
    let mut p = Project::new(&[("a.fake", a)], "a.fake", state.path());
    p.scanned("a.fake::fails");
    let listed = p.app.apply_control(Call::Tests(TestsParams { path: Some("a.fake".into()), state: None })).unwrap();
    assert_eq!(listed["tests"][0], json!({ "provider": "provider", "id": "a.fake::adds", "label": "adds", "kind": "test", "path": "a.fake", "line": 1, "state": "not_run", "durationMs": null, "stale": false }));
    let run = TestsRunParams { tests: Some(vec![TestRef { provider: "provider".into(), id: "a.fake::fails".into() }]), path: None, failed: false, all: false };
    p.app.config.set_value("agents.mcp.fenix_write", Some(fenix_config::Value::Bool(false)));
    assert!(p.app.apply_control(Call::TestsRun(run.clone())).unwrap_err().message.contains("fenix_write"));
    p.app.config.set_value("agents.mcp.fenix_write", Some(fenix_config::Value::Bool(true)));
    let started = p.app.apply_control(Call::TestsRun(run.clone())).unwrap();
    assert_eq!(started["requested"], "starts");
    let id = started["run"].as_u64().expect("its id");
    p.until("the run", |p| test_runs(p) == 1);
    assert_eq!(p.last().run.0, id);
    let failures = p.app.apply_control(Call::TestsFailures(TestsParams::default())).unwrap();
    assert_eq!(failures["failures"][0]["id"], "a.fake::fails");
    untrusted(&mut p, state.path());
    assert!(p.app.apply_control(Call::TestsRun(run)).unwrap_err().message.contains("only in a trusted project"));
    off(&mut p);
    let refused = p.app.apply_control(Call::Tests(TestsParams::default())).unwrap_err();
    assert_eq!((refused.code, refused.message.as_str()), (fenix_control::protocol::code::METHOD_NOT_FOUND, "the test explorer is off (tests.enabled)"));
}
