//! The test explorer's host: its plain functions on their own, then Fenix
//! running the provider guest (`fenix-wasm/tests/guests/provider.wasm`)
//! and `fake-test-runner` (`fenix_tasks::fake_runner`, run from this test
//! harness: `fake_test_runner` below) headless, through each rule of plan
//! §6.4 to §8.5. Every wait is bounded.

use super::*;
use fenix_tests::{Kind, Outcome, State, TestKey};

// The plain functions.

#[test]
fn a_providers_paths_are_the_projects_relative_ones_and_go_back_as_project_paths() {
    let root = Path::new(if cfg!(windows) { r"C:\src\orbit" } else { "/src/orbit" });
    let sub = root.join("crates").join("geo");
    assert_eq!(model_path("/project/src/lib.rs", root, root), "src/lib.rs");
    assert_eq!(model_path("/project/./src/../src/lib.rs", root, root), "src/lib.rs");
    assert_eq!(model_path("src/lib.rs", root, root), "src/lib.rs", "a scan's: relative to the project");
    assert_eq!(model_path("src/lib.rs", root, &sub), "crates/geo/src/lib.rs", "a report's: relative to its step's folder");
    assert_eq!(model_path("../../README.md", root, &sub), "README.md");
    assert_eq!(model_path(r"src\lib.rs", root, root), "src/lib.rs", "a \\ or a /");
    let full = root.join("src").join("lib.rs");
    assert_eq!(model_path(&full.to_string_lossy(), root, root), "src/lib.rs", "a full path of the project's");
    let outside = if cfg!(windows) { r"C:\rust\lib\core.rs" } else { "/rust/lib/core.rs" };
    assert_eq!(model_path(outside, root, root), outside, "a place outside the project keeps its full path");
    assert_eq!(model_path("/project/../x.rs", root, root), "/project/../x.rs", "out of the project: as given, never mended");
    if cfg!(windows) {
        assert_eq!(model_path("c:/SRC/orbit/src/lib.rs", root, root), "src/lib.rs", "a drive in any case");
    }
    assert_eq!(guest_path("src/lib.rs"), "/project/src/lib.rs");
    assert_eq!(guest_path(outside), outside);
    let found = api_found(model_found(FoundTest { id: "t".into(), parent: None, label: "t".into(), kind: TestKind::Test, path: Some(full.to_string_lossy().into_owned()), line: Some(3), end_line: None, skip: None, template: false, data: Some("d".into()) }, &[root.to_path_buf()], root));
    assert_eq!((found.path.as_deref(), found.data.as_deref(), found.kind), (Some("/project/src/lib.rs"), Some("d"), TestKind::Test), "given back as /project/<path>, whatever form it came in");
}

/// The command line as Rust's standard library builds it for
/// `CreateProcessW` (the review of steps 1 and 2's K2).
#[test]
fn a_command_line_is_counted_as_windows_quotes_it() {
    let program = r"C:\tools\cargo.exe";
    let args = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    // "C:\tools\cargo.exe" test "a b" "" q\"t tr\ "a b\\"
    let line = format!("\"{program}\" test \"a b\" \"\" q\\\"t tr\\ \"a b\\\\\"");
    assert_eq!(command_line(program, &args(&["test", "a b", "", "q\"t", "tr\\", "a b\\"])), Ok((line.encode_utf16().count(), 32_766)));
    // A `\` before a `"` doubled, and one more for the `"`.
    assert_eq!(command_line("p", &args(&["a\\\"b"])), Ok((3 + 1 + "a\\\\\\\"b".len(), 32_766)));
    assert_eq!(command_line("p", &args(&["é😀"])), Ok((3 + 1 + 3, 32_766)), "UTF-16 units");
    let long = vec!["a".repeat(32_766 - 4)];
    assert_eq!(command_line("p", &long).map(|(n, _)| n), Ok(32_766), "at its limit");
    assert_eq!(command_line_refused("p", &long), None);
    let over = vec!["a".repeat(41_203 - 4)];
    assert_eq!(command_line_refused("p", &over).as_deref(), Some("its command line is 41,203 characters; Windows takes 32,766"));
    // Through cmd.exe: `cmd.exe /e:ON /v:OFF /d /c ""x.cmd" a "b&c" "%%cd:~,%""`.
    let bat = "cmd.exe /e:ON /v:OFF /d /c \"\"x.cmd\" a \"b&c\" \"%%cd:~,%\"\"";
    assert_eq!(command_line("x.cmd", &args(&["a", "b&c", "%"])), Ok((bat.encode_utf16().count(), 8_191)));
    assert!(command_line_refused("x.BAT", &[ "a".repeat(8_200)]).unwrap().contains("through cmd.exe"));
    assert!(command_line_refused("x.cmd", &args(&["a\nb"])).unwrap().contains("line break"));
}

#[test]
fn escape_codes_are_taken_out_of_a_line() {
    assert_eq!(without_escapes("\u{1b}[1;31mred\u{1b}[0m plain"), "red plain");
    assert_eq!(without_escapes("\u{1b}]0;a title\u{7}after \u{1b}]8;;x\u{1b}\\link"), "after link");
    assert!(matches!(without_escapes("plain"), std::borrow::Cow::Borrowed(_)));
}

/// What a step's process printed, as its provider is given it: each
/// line's stream and `cut` as read; and as *Test output* shows it.
#[test]
fn a_steps_lines_keep_their_stream_and_cut() {
    use fenix_tasks::OutputLine;
    let line = |text: &str, stream, cut| OutputLine { stream, text: text.into(), cut };
    let (lines, text) = test_lines(vec![line("\u{1b}[32mhalf ", OutputStream::Stdout, true), line("err", OutputStream::Stderr, false), line("rest", OutputStream::Stdout, false)]);
    assert_eq!(lines, [TestLine { text: "half ".into(), stderr: false, cut: true }, TestLine { text: "err".into(), stderr: true, cut: false }, TestLine { text: "rest".into(), stderr: false, cut: false }]);
    assert_eq!(text, "half err\nrest\n");
}

/// A batch is at most 512 lines and 256 KiB (§6.4 "Batches").
#[test]
fn lines_go_to_a_provider_512_at_a_time_and_256_kib() {
    let mut batch = Batch::default();
    for i in 0..1_000 {
        batch.push(TestLine { text: format!("line {i:>4}"), stderr: false, cut: false });
    }
    assert_eq!((batch.take().len(), batch.take().len(), batch.take().len()), (512, 488, 0));
    for _ in 0..5 {
        batch.push(TestLine { text: "x".repeat(64 << 10), stderr: false, cut: true });
    }
    assert_eq!((batch.take().len(), batch.take().len(), batch.bytes), (4, 1, 0), "256 KiB: four of 64 KiB");
}

fn record(id: &str, parent: Option<&str>, kind: Kind, bytes: usize) -> fenix_tests::Found {
    fenix_tests::Found { data: Some("d".repeat(bytes)), ..fenix_tests::Found::new(id, parent, id, kind) }
}

/// An ask is held to its bytes as the model holds it to 10,000 records:
/// whole fill-ins left out, the last first; then `run-all`.
#[test]
fn an_ask_over_its_bytes_leaves_out_fill_ins_then_runs_all() {
    let ask = || {
        fenix_tests::Ask::Run(vec![
            record("a", None, Kind::File, 10),
            record("a::1", Some("a"), Kind::Test, 100),
            record("a::2", Some("a"), Kind::Test, 100),
            record("t", None, Kind::Test, 10),
            record("b", None, Kind::File, 10),
            record("b::1", Some("b"), Kind::Test, 100),
        ])
    };
    let ids = |ask: fenix_tests::Ask| match ask {
        fenix_tests::Ask::Run(r) => r.into_iter().map(|r| r.id).collect::<Vec<_>>(),
        other => panic!("{other:?}"),
    };
    assert_eq!(ids(capped_ask(ask(), 10_000)).len(), 6, "under its bytes, as it is");
    assert_eq!(ids(capped_ask(ask(), 300)), ["a", "a::1", "a::2", "t", "b"], "b's fill-in goes first");
    assert_eq!(ids(capped_ask(ask(), 100)), ["a", "t", "b"]);
    assert_eq!(capped_ask(ask(), 20), fenix_tests::Ask::RunAll, "coarser, never fewer");
    assert_eq!(capped_ask(fenix_tests::Ask::Discover, 0), fenix_tests::Ask::Discover);
}

/// How a step ended: its exit code as the process's 32 bits signed (a
/// Windows crash's NTSTATUS negative), a signal none, and a stop or a
/// timeout what Fenix did, whatever the process did meanwhile.
#[test]
fn a_steps_end_is_how_its_process_ended_or_why_fenix_stopped_it() {
    use fenix_tests::StepEnd;
    let exited = |code| TaskOutcome::Exited { success: false, code };
    assert_eq!(step_end(&exited(Some(0xC000_0005_u32 as i32)), None), StepEnd::Exited(Some(-1_073_741_819)));
    assert_eq!(step_end(&exited(None), None), StepEnd::Exited(None));
    assert_eq!(step_end(&TaskOutcome::Cancelled, None), StepEnd::Stopped);
    assert_eq!(step_end(&TaskOutcome::Cancelled, Some(&StepEnd::TimedOut)), StepEnd::TimedOut);
    assert_eq!(step_end(&exited(Some(0)), Some(&StepEnd::Stopped)), StepEnd::Stopped);
    assert!(matches!(step_end(&TaskOutcome::Failed("pipe".into()), None), StepEnd::Failed(why) if why.contains("pipe")));
    assert_eq!(api_step_end(&StepEnd::Exited(Some(-1))), TestStepEnd::Exited(Some(-1)));
}

#[test]
fn numbers_are_grouped_and_a_results_file_named_by_its_root() {
    assert_eq!((grouped(7), grouped(41_203), grouped(1_000_000)), ("7".into(), "41,203".into(), "1,000,000".into()));
    let name = results_name(Path::new("/src/orbit"));
    assert!(name.len() == 21 && name.ends_with(".json") && name == results_name(Path::new("/src/orbit")) && name != results_name(Path::new("/src/other")));
}

// Fenix running the provider guest.

/// `fake-test-runner`, when a test starts this harness as it (the
/// `program_for` below): what follows `--` is its arguments.
#[test]
#[ignore = "the made-up test runner, which the tests below start as a process"]
fn fake_test_runner() {
    if std::env::var_os("FENIX_FAKE_TEST_RUNNER").is_none() {
        return;
    }
    let all: Vec<String> = std::env::args().skip(1).collect();
    let at = all.iter().position(|a| a == "--").map_or(all.len(), |i| i + 1);
    let (prefix, args) = (all[..at].to_vec(), all[at..].to_vec());
    fenix_tasks::fake_runner::main(&args, &move || {
        let mut again = std::process::Command::new(std::env::current_exe().unwrap());
        again.args(&prefix).env("FENIX_FAKE_TEST_RUNNER", "1");
        again
    });
}

const EXT: &str = "test.provider";

fn provider() -> ProviderId {
    ProviderId::new(EXT, "provider")
}

/// The provider guest as extension `id` (its provider the id's last
/// part, `provider`), allowed to run `fake-test-runner`, serving a
/// project with `fake.toml` at its top, scanning its `*.fake` files.
fn guest(id: &str) -> crate::installed::CodeExtension {
    let guests = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../fenix-wasm/tests/guests");
    let staged = tempfile::tempdir().unwrap().keep();
    std::fs::copy(guests.join("provider.wasm"), staged.join("extension.wasm")).unwrap();
    let short = id.rsplit('.').next().unwrap();
    let manifest = fenix_extensions::Manifest::parse(&format!(
        "[extension]\nid = \"{id}\"\nname = \"Provider\"\nversion = \"1.0.0\"\nruntime = \"wasm\"\n\n[engine]\napi = \"1.6\"\nfenix = \">=1.5, <2\"\n\n[permissions]\nprocess = [\"fake-test-runner\"]\nfilesystem = \"project-read\"\n\n[[contributes.test_providers]]\nid = \"{short}\"\nname = \"fake\"\nmarkers = [\"fake.toml\"]\nscan = [\"*.fake\", \"*.py\"]\n"
    ))
    .unwrap();
    crate::installed::CodeExtension { id: id.into(), name: "Provider".into(), dir: staged, manifest }
}

struct Project {
    app: App,
    root: PathBuf,
    _dir: tempfile::TempDir,
}

impl Project {
    /// A project of `files`, `focused` open, the explorer on, the guest
    /// installed, `fake-test-runner` this harness, results kept in
    /// `state`.
    fn new(files: &[(&str, &str)], focused: &str, state: &Path) -> Project {
        let dir = tempfile::tempdir().unwrap();
        let root = refactor::identity(dir.path());
        std::fs::write(root.join("fake.toml"), "").unwrap();
        std::fs::create_dir_all(root.join(".fenix")).unwrap();
        std::fs::write(root.join(".fenix/project.ini"), "").unwrap();
        for (name, text) in files {
            let path = root.join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        let mut app = App::with_file(Some(root.join(focused).to_string_lossy().into_owned()));
        app.project_root = Some(root.clone());
        app.config.set_value("tests.enabled", Some(fenix_config::Value::Bool(true)));
        app.wasm.installed = vec![guest(EXT)];
        app.tests.state_dir = Some(state.to_path_buf());
        let exe = std::env::current_exe().unwrap().to_string_lossy().into_owned();
        app.tests.program_for = Some(Box::new(move |program| {
            let prefix = ["--ignored", "--exact", "app::tests_host::tests::fake_test_runner", "--nocapture", "--"].map(String::from).to_vec();
            (program == "fake-test-runner").then(|| (exe.clone(), prefix, vec![("FENIX_FAKE_TEST_RUNNER".to_string(), "1".to_string())]))
        }));
        Project { app, root, _dir: dir }
    }

    /// Pumps the extensions and the host until `done`; at most 120 s (a
    /// loaded CI runner compiles and calls an extension slowly).
    fn until(&mut self, what: &str, done: impl Fn(&Project) -> bool) {
        let started = Instant::now();
        while !done(self) {
            assert!(started.elapsed() < Duration::from_secs(120), "{what}: never\n{}\n--- Test output ---\n{}", self.app.extension_output_text(), self.output());
            std::thread::sleep(Duration::from_millis(5));
            self.app.pump_wasm();
            self.app.tests_tick(Instant::now());
            // A debug session's adapter, as the event loop hands it on.
            let events: Vec<_> = self.app.dap_unread.iter().flat_map(|(generation, rx)| rx.try_iter().map(|e| (*generation, e)).collect::<Vec<_>>()).collect();
            for (generation, event) in events {
                self.app.handle_user_event(FenixUserEvent::Dap { generation, event });
            }
        }
    }

    fn tests(&self) -> &ProjectTests {
        &self.app.tests.projects[&self.root]
    }

    fn model(&self) -> &Model {
        &self.tests().model
    }

    fn has(&self, id: &str) -> bool {
        self.app.tests.projects.get(&self.root).is_some_and(|p| p.model.find(&TestKey::new(&provider(), id)).is_some())
    }

    fn state(&self, id: &str) -> State {
        let node = self.model().find(&TestKey::new(&provider(), id)).unwrap_or_else(|| panic!("no {id}"));
        self.model().node(node).unwrap().state()
    }

    fn result(&self, id: &str) -> fenix_tests::TestResult {
        let node = self.model().find(&TestKey::new(&provider(), id)).unwrap();
        self.model().node(node).unwrap().result().cloned().unwrap_or_else(|| panic!("{id} has no result"))
    }

    fn output(&self) -> String {
        self.app.tests.projects.get(&self.root).and_then(|p| p.output).and_then(|b| self.app.buffers.get(b)).map(|ob| ob.buffer.text()).unwrap_or_default()
    }

    fn ended(&self) -> bool {
        self.model().last_run().is_some()
    }

    fn last(&self) -> fenix_tests::RunEnd {
        self.model().last_run().cloned().unwrap()
    }

    /// The scan of the file you're in, done.
    fn scanned(&mut self, id: &str) {
        let id = id.to_string();
        self.until("the scan", |p| p.has(&id));
    }

    fn open(&mut self, name: &str) {
        let id = self.app.buffers.open_path(&self.root.join(name));
        self.app.open_buffer_in_focused_pane(id);
    }

    /// The pid `fake-test-runner`'s `child` said, once *Test output* has it.
    fn child(&mut self) -> u32 {
        self.until("the child", |p| p.output().contains("CHILD "));
        let output = self.output();
        output.lines().find_map(|l| l.strip_prefix("CHILD ")).unwrap().trim().parse().unwrap()
    }

    fn step_running(&self) -> bool {
        self.tests().step.as_ref().is_some_and(|s| s.runner.is_some())
    }
}

/// A run from end to end: the file scanned when it's opened, the run
/// planned from its ask, the step run through `fake-test-runner`, its
/// output read in batches, each test's result in the model; the run's
/// output in *Test output*; the results file written.
#[test]
fn a_file_is_scanned_its_tests_run_and_their_results_kept() {
    let state = tempfile::tempdir().unwrap();
    let a = "# test adds\n# test fails\n# skip slow not now\n#! out PASS a.fake::adds 3ms\n#! err FAIL a.fake::fails a.fake:2 two and two\n#! out SKIP a.fake::slow not now\n#! out \u{1b}[31mcoloured\u{1b}[0m\n";
    let mut p = Project::new(&[("a.fake", a)], "a.fake", state.path());
    p.scanned("a.fake::fails");
    assert_eq!(p.model().node(p.model().find(&TestKey::new(&provider(), "a.fake::slow")).unwrap()).unwrap().found().skip.as_deref(), Some("not now"));
    p.app.tests_run_file();
    p.until("the run", Project::ended);
    let end = p.last();
    assert_eq!((end.passed, end.failed, end.skipped, end.errored, end.halted), (1, 1, 1, 0, None), "{}", p.output());
    assert_eq!(p.state("a.fake::adds"), State::Passed);
    assert_eq!(p.result("a.fake::adds").duration_ms, Some(3.0));
    let failed = p.result("a.fake::fails");
    assert_eq!((failed.outcome, failed.message.as_deref()), (Outcome::Failed, Some("two and two")), "read from stderr as from stdout");
    assert_eq!(failed.at.map(|at| (at.path, at.line)), Some(("a.fake".to_string(), 2)), "relative to its step's folder: the project's own path");
    let output = p.output();
    assert!(output.contains("$ ") && output.contains("a.fake::adds") && output.contains("(a.fake)"), "its command line and label head its step: {output}");
    assert!(output.contains("\ncoloured\n") && !output.contains('\u{1b}'), "escape codes taken out: {output}");
    assert!(output.contains("1 passed, 1 failed, 0 errored, 1 skipped"), "{output}");
    let file = state.path().join(results_name(&p.root));
    p.until("the results written", |_| std::fs::read_to_string(&file).is_ok_and(|t| t.contains("a.fake::fails")));
    let kept = std::fs::read_to_string(&file).expect("the results file");
    assert!(kept.contains("\"a.fake::fails\"") && kept.contains("\"extension\":\"test.provider\""), "{kept}");
    assert!(!p.app.extension_output_text().contains("error "), "{}", p.app.extension_output_text());

    // Fenix starts again: the results come back stale, until they run.
    let mut q = Project { app: App::with_file(Some(p.root.join("a.fake").to_string_lossy().into_owned())), root: p.root.clone(), _dir: tempfile::tempdir().unwrap() };
    q.app.project_root = Some(q.root.clone());
    q.app.config.set_value("tests.enabled", Some(fenix_config::Value::Bool(true)));
    q.app.wasm.installed = vec![guest(EXT)];
    q.app.tests.state_dir = Some(state.path().to_path_buf());
    q.scanned("a.fake::fails");
    q.until("the results read", |q| q.tests().restoring.is_none());
    let node = q.model().find(&TestKey::new(&provider(), "a.fake::fails")).unwrap();
    let node = q.model().node(node).unwrap();
    assert_eq!((node.state(), node.stale(), node.result().map(|r| r.restored)), (State::Failed, true, Some(true)), "restored, stale");
}

/// With `tests.enabled` off no call reaches a provider: a file of its
/// isn't scanned, its extension isn't started for it, the commands say
/// so, and a save does nothing.
#[test]
fn with_the_explorer_off_no_call_reaches_a_provider() {
    let state = tempfile::tempdir().unwrap();
    let mut p = Project::new(&[("a.fake", "# test adds\n")], "a.fake", state.path());
    p.app.config.set_value("tests.enabled", Some(fenix_config::Value::Bool(false)));
    for _ in 0..20 {
        p.app.pump_wasm();
        p.app.tests_tick(Instant::now());
    }
    p.app.tests_file_saved(&p.root.join("a.fake"));
    p.app.tests_run_file();
    assert!(p.app.status_message.as_ref().is_some_and(|m| m.text.contains("tests.enabled")));
    assert!(p.app.tests.projects.is_empty() && p.app.tests.calls.is_empty(), "nothing asked");
    assert!(!p.app.wasm.status.contains_key(EXT), "its extension wasn't started for it");
}

/// A plan of several steps: one whose command line is over Windows' limit
/// fails before it starts, its tests errored with why, and the plan goes
/// on to the next; one that crashes errors what it held; one that didn't
/// build errors its file's tests, not as failures.
#[test]
fn a_plan_goes_on_past_a_step_that_failed_crashed_or_didnt_build() {
    let state = tempfile::tempdir().unwrap();
    let long: String = (0..40).map(|i| format!("# test {}{i}\n", "n".repeat(900))).collect();
    let a = "# test one\n#! out PASS a.fake::one 1ms\n";
    let c = "# test two\n# test three\n#! out PASS c.fake::two 1ms\n#! out CRASH c.fake its binary crashed: SIGABRT\n#! crash\n";
    let d = "# test four\n#! out BROKEN d.fake d.fake didn't build: mismatched types\n#! exit 101\n";
    let mut p = Project::new(&[("a.fake", a), ("b.fake", &long), ("c.fake", c), ("d.fake", d)], "a.fake", state.path());
    for (name, id) in [("b.fake", format!("b.fake::{}0", "n".repeat(900))), ("c.fake", "c.fake::three".into()), ("d.fake", "d.fake::four".into()), ("a.fake", "a.fake::one".into())] {
        p.open(name);
        p.scanned(&id);
    }
    let files: Vec<TestKey> = ["a.fake", "b.fake", "c.fake", "d.fake"].iter().map(|f| TestKey::new(&provider(), *f)).collect();
    p.app.tests_request(&p.root.clone(), vec![Selection::Nodes(files)], "four files");
    p.until("the run", Project::ended);
    assert_eq!(p.state("a.fake::one"), State::Passed);
    let long_test = p.result(&format!("b.fake::{}7", "n".repeat(900)));
    assert_eq!(long_test.outcome, Outcome::Errored);
    assert!(long_test.message.as_deref().is_some_and(|m| m.starts_with("its command line is ") && m.ends_with("characters; Windows takes 32,766")), "{:?}", long_test.message);
    assert!(p.output().contains("couldn't start: its command line is "), "it never ran: {}", p.output());
    assert_eq!(p.state("c.fake::two"), State::Passed, "what ended before the crash keeps its result");
    assert_eq!(p.result("c.fake::three").message.as_deref(), Some("its binary crashed: SIGABRT"));
    let four = p.result("d.fake::four");
    assert_eq!((four.outcome, four.message.as_deref()), (Outcome::Errored, Some("d.fake didn't build: mismatched types")), "errored, not failed");
    assert_eq!(p.last().first_broken.as_deref(), Some("d.fake didn't build: mismatched types"));
    let output = p.output();
    assert!(output.contains(if cfg!(windows) { "-- exited -" } else { "-- ended by a signal --" }), "a crash: {output}");
    assert!(output.contains("-- exited 101 --"));
}

/// `run-all`: a step whose `needs` aren't there fails before it starts,
/// with why, and its tests are errored so.
#[test]
fn a_step_whose_need_isnt_there_fails_with_why() {
    let state = tempfile::tempdir().unwrap();
    let mut p = Project::new(&[("a.fake", "# test one\n# test two\n"), ("fake-unit.txt", "out PASS a.fake::one 1ms\n")], "a.fake", state.path());
    p.scanned("a.fake::two");
    p.app.tests_run_all();
    p.until("the run", Project::ended);
    assert_eq!(p.state("a.fake::one"), State::Passed, "the unit step ran, its reporter there");
    assert_eq!(p.result("a.fake::two").message.as_deref(), Some("needs /data/missing.txt, which isn't there"));
    assert!(p.output().contains("integration couldn't start: needs /data/missing.txt"), "{}", p.output());
}

/// A stop while a step runs kills its whole tree, the child it left
/// holding the pipes too; its tests go back to what they were, nothing
/// marked failed. On Windows the job object, on Unix the process group.
#[test]
fn a_stop_kills_the_steps_tree_and_puts_its_tests_back() {
    let state = tempfile::tempdir().unwrap();
    let a = "# test one\n# test two\n#! out PASS a.fake::one 1ms\n#! child\n#! wait\n";
    let mut p = Project::new(&[("a.fake", a)], "a.fake", state.path());
    p.scanned("a.fake::two");
    p.app.tests_run_file();
    let child = p.child();
    assert!(fenix_tasks::fake_runner::alive(child));
    p.until("one's result", |p| p.state("a.fake::one") == State::Passed);
    p.app.tests_stop();
    p.until("the run", Project::ended);
    assert!(matches!(p.last().halted, Some(fenix_tests::Halt::Stopped(_))));
    assert_eq!(p.state("a.fake::two"), State::Unknown, "back to what it was");
    assert_eq!(p.state("a.fake::one"), State::Passed, "what ended before the stop stands");
    assert!(fenix_tasks::fake_runner::gone_within(child, Duration::from_secs(10)), "the child outlived its step");
    assert!(p.tests().step.is_none() && p.tests().call.is_none() && p.app.tests.calls.is_empty());
}

/// A stop while a `tests-read` is out: the call finishes, its reports
/// stand, then the step's end `stopped` (§6.4 "Stopping", R5).
#[test]
fn a_stop_mid_call_lets_the_call_finish_and_its_reports_stand() {
    let state = tempfile::tempdir().unwrap();
    let a = "# test one\n# test two\n#! out PASS a.fake::one 1ms\n#! wait\n";
    let mut p = Project::new(&[("a.fake", a)], "a.fake", state.path());
    p.scanned("a.fake::two");
    p.app.tests_run_file();
    // Until the batch with its `PASS` is out, and not yet answered: the
    // extension's answers are taken only before the host sends.
    let started = Instant::now();
    loop {
        assert!(started.elapsed() < Duration::from_secs(60), "{}", p.output());
        std::thread::sleep(Duration::from_millis(5));
        p.app.pump_wasm();
        p.app.tests_tick(Instant::now());
        let out = p.tests().step.as_ref().is_some_and(|s| s.reading && s.lines.is_empty());
        if out && p.output().contains("PASS a.fake::one") {
            break;
        }
    }
    p.app.tests_stop();
    assert!(p.tests().step.as_ref().is_some_and(|s| s.reading && s.halt.is_some()), "the tree killed, the call still out");
    p.until("the run", Project::ended);
    assert_eq!(p.state("a.fake::one"), State::Passed, "the call's reports stand");
    assert_eq!(p.state("a.fake::two"), State::Unknown);
}

/// `tests.timeout_minutes` stops a run that has gone on that long: its
/// step `timed-out`, its unfinished tests errored. Fenix's clock moved,
/// not waited for.
#[test]
fn a_run_past_its_timeout_is_stopped_and_its_tests_errored() {
    let state = tempfile::tempdir().unwrap();
    let mut p = Project::new(&[("a.fake", "# test one\n#! wait\n")], "a.fake", state.path());
    p.app.config.set_value("tests.timeout_minutes", Some(fenix_config::Value::Int(1)));
    p.scanned("a.fake::one");
    p.app.tests_run_file();
    p.until("the step", Project::step_running);
    p.app.tests.skew_ms = 61_000;
    p.until("the run", Project::ended);
    assert_eq!(p.last().halted, Some(fenix_tests::Halt::TimedOut));
    assert_eq!(p.result("a.fake::one").message.as_deref(), Some("timed out after 1 min"));
}

/// A run that replaces the one going stops its step at once and goes next.
#[test]
fn a_replacing_request_stops_the_run_going() {
    let state = tempfile::tempdir().unwrap();
    let mut p = Project::new(&[("a.fake", "# test one\n#! wait\n"), ("b.fake", "# test two\n#! out PASS b.fake::two 1ms\n")], "b.fake", state.path());
    p.scanned("b.fake::two");
    p.open("a.fake");
    p.scanned("a.fake::one");
    p.app.tests_run_file();
    p.until("the step", Project::step_running);
    let root = p.root.clone();
    assert_eq!(p.app.tests_replace(&root, Selection::File("b.fake".into())), fenix_tests::Requested::Replaces);
    p.until("the second run", |p| p.state("b.fake::two") == State::Passed && p.ended() && p.tests().run.is_none());
    assert_eq!(p.state("a.fake::one"), State::Unknown, "the replaced run's test back to what it was");
}

/// A line over 64 KiB comes in pieces, each but the last `cut`, and the
/// provider joins them: the test's failure text is the line's end.
/// 100,000 lines go through the provider in batches, timed.
#[test]
fn a_long_line_is_cut_and_joined_and_100000_lines_go_through() {
    let state = tempfile::tempdir().unwrap();
    let a = "# test long\n#! long 100000 FAIL a.fake::long a.fake:0 \n";
    let b = "# test many\n#! lines 100000 OUT line\n#! out PASS b.fake::many 1ms\n";
    let mut p = Project::new(&[("a.fake", a), ("b.fake", b)], "b.fake", state.path());
    p.scanned("b.fake::many");
    p.open("a.fake");
    p.scanned("a.fake::long");
    p.app.tests_run_file();
    p.until("the run", Project::ended);
    let text = p.result("a.fake::long").text.unwrap();
    assert!(!text.starts_with("FAIL") && text.len() <= 64 << 10 && text.ends_with('é'), "joined, its end kept: {} bytes", text.len());
    p.open("b.fake");
    let started = Instant::now();
    p.app.tests_run_file();
    p.until("the second run", |p| p.state("b.fake::many") == State::Passed && p.tests().run.is_none());
    let took = started.elapsed();
    eprintln!("100,000 lines through the provider guest in {took:?}");
    assert!(took < Duration::from_secs(90), "{took:?}");
    assert!(p.output().contains("line 99999"));
}

/// The extension started again while a run's step is under way (another
/// project, a setting): the run stops as after a trap, its tests back to
/// what they were, its tree killed; the new instance isn't asked about it.
#[test]
fn the_extension_starting_again_stops_its_run() {
    let state = tempfile::tempdir().unwrap();
    let mut p = Project::new(&[("a.fake", "# test one\n#! child\n#! wait\n")], "a.fake", state.path());
    p.scanned("a.fake::one");
    p.app.tests_run_file();
    let child = p.child();
    p.app.restart_code_extension(EXT);
    p.until("the run", Project::ended);
    match p.last().halted {
        Some(fenix_tests::Halt::Stopped(why)) => assert!(why.contains("started again"), "{why}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(p.state("a.fake::one"), State::Unknown);
    assert!(fenix_tasks::fake_runner::gone_within(child, Duration::from_secs(10)));
    assert!(p.app.tests.calls.values().all(|(_, c)| matches!(c, CallFor::Scan { .. })), "nothing asked of the new instance about the run: {:?}", p.app.tests.calls);
}

/// A provider that traps: its run stops as after a trap, its tests back
/// to what they were, and nothing more is asked of it.
#[test]
fn a_provider_that_traps_stops_its_run() {
    let state = tempfile::tempdir().unwrap();
    let mut p = Project::new(&[("a.fake", "# test one\n")], "a.fake", state.path());
    p.app.config.set_value("tests.enabled", Some(fenix_config::Value::Bool(false)));
    p.app.wasm.installed = vec![guest("test.trap")];
    let trap = ProviderId::new("test.trap", "trap");
    let root = p.root.clone();
    // Its tests from elsewhere: its scan would trap first.
    p.app.config.set_value("tests.enabled", Some(fenix_config::Value::Bool(true)));
    p.app.tests_project(&root).model.add_provider(&trap, "trap");
    p.app.tests_project(&root).model.scan(&trap, "a.fake", vec![fenix_tests::Found::new("a.fake::one", None, "one", Kind::Test)]);
    p.app.tests.seen.clear();
    p.app.tests_request(&root, vec![Selection::All], "all");
    p.until("the run", |p| p.model().last_run().is_some());
    assert!(matches!(p.last().halted, Some(fenix_tests::Halt::Stopped(_))), "{:?}", p.last());
    let node = p.model().find(&TestKey::new(&trap, "a.fake::one")).unwrap();
    assert_eq!(p.model().node(node).unwrap().state(), State::Unknown);
    p.until("its stop", |p| matches!(p.app.wasm.status.get("test.trap"), Some(wasm_host::Status::Stopped(_))));
}

/// A listing, asked for: its step runs in `/data`, what it found is in
/// the model as listed. What a provider said it is (`test-provider-set`)
/// is kept: once the page has opened a change lists it, a repeat changes
/// nothing, and `""` hides it.
#[test]
fn a_listing_runs_and_what_a_provider_says_it_is_is_kept() {
    let state = tempfile::tempdir().unwrap();
    let mut p = Project::new(&[("a.fake", "# test one\n")], "a.fake", state.path());
    std::fs::create_dir_all(p.app.wasm.installed[0].dir.join("data")).unwrap();
    std::fs::write(p.app.wasm.installed[0].dir.join("data/fake-list.txt"), "out FOUND a.fake::listed a.fake\n").unwrap();
    p.scanned("a.fake::one");
    let root = p.root.clone();
    p.app.tests_list(&root);
    p.until("the listing", |p| p.listings() == 1);
    assert!(p.has("a.fake::listed"), "{}", p.output());
    assert!(p.output().contains("--list"), "{}", p.output());
    // The page opens: its first opening lists, as an automatic listing.
    p.app.tests_page_opened(&root);
    p.until("the page's listing", |p| p.listings() == 2);
    // A change lists it again.
    p.app.run_extension_command(EXT, "provider.set");
    p.until("its runner", |p| p.model().providers().first().is_some_and(|v| v.name == "fake-test-runner 2.0"));
    p.until("the listing after", |p| p.listings() == 3);
    // The same again changes nothing.
    p.app.run_extension_command(EXT, "provider.set");
    p.until("said again", |p| p.app.extension_output_text().matches("provider.set: Ok").count() == 2);
    for _ in 0..20 {
        p.app.pump_wasm();
        p.app.tests_tick(Instant::now());
    }
    assert!(p.listings() == 3 && p.tests().run.is_none(), "a repeat changes nothing");
    // Nothing to run here: not shown, not asked.
    p.app.run_extension_command(EXT, "provider.hide");
    p.until("hidden", |p| p.model().providers().is_empty());
    // Kept across its extension starting again.
    p.app.restart_code_extension(EXT);
    p.until("ready again", |p| p.app.code_ready(EXT));
    assert!(p.model().providers().is_empty(), "still hidden");
}

impl Project {
    /// How many listings have ended in the project.
    fn listings(&self) -> usize {
        self.app.tests.projects.get(&self.root).map_or(0, |t| t.ended_runs.iter().filter(|e| matches!(e.kind, fenix_tests::RunKind::Listing { .. })).count())
    }
}

/// A project's first scan (§6.7, §7.1): its files the provider's `scan`
/// names, walked through the project's ignores -- hidden folders and
/// what `.gitignore` leaves out aren't -- off the UI thread, into the
/// model in batches.
#[test]
fn a_projects_first_scan_walks_it_through_its_ignores() {
    let state = tempfile::tempdir().unwrap();
    let files = [("a.fake", "# test one\n"), ("sub/b.fake", "# test two\n"), ("target/c.fake", "# test three\n"), (".hidden/d.fake", "# test four\n"), (".gitignore", "target/\n")];
    let mut p = Project::new(&files, "a.fake", state.path());
    p.until("the walk", |p| p.has("sub/b.fake::two") && p.tests().walk.is_none() && p.tests().walks.is_empty());
    assert!(p.has("a.fake::one"));
    assert!(!p.has("target/c.fake::three") && !p.has(".hidden/d.fake::four"), "ignored, hidden");
    assert_eq!(p.tests().scanned_of.get(&provider()), Some(&(2, 2)), "a.fake is open: scanned from its buffer, and counted");
}

/// Run on save (§8.4): a second after the last save, the saved file's
/// tests, which the save made stale; only in a trusted project.
#[test]
fn a_save_runs_the_saved_files_tests_a_second_later() {
    let state = tempfile::tempdir().unwrap();
    let mut p = Project::new(&[("a.fake", "# test one\n#! out PASS a.fake::one 1ms\n")], "a.fake", state.path());
    p.app.config.set_value("tests.on_save", Some(fenix_config::Value::Text("file".into())));
    p.scanned("a.fake::one");
    p.app.tests_run_file();
    p.until("the run", Project::ended);
    let path = p.root.join("a.fake");
    p.app.tests_file_saved(&path);
    let node = p.model().find(&TestKey::new(&provider(), "a.fake::one")).unwrap();
    assert!(p.model().node(node).unwrap().stale(), "saved since its result");
    p.app.tests_tick(Instant::now());
    assert_eq!(p.tests().ended_runs.len(), 1, "not yet: a second after the save");
    p.app.tests_tick(Instant::now() + Duration::from_secs(2));
    p.until("the run on save", |p| p.tests().ended_runs.len() == 2);
    assert!(!p.model().node(node).unwrap().stale());
}

// After the review of step 3.

/// A step whose runner floods empty or short lines while its provider
/// is slow falls behind by what the lines cost, not their text alone:
/// 2,000,000 empty lines waiting used to count as nothing (M1).
#[test]
fn empty_lines_count_toward_falling_behind() {
    let dir = tempfile::tempdir().unwrap();
    let root = refactor::identity(dir.path());
    let mut app = App::with_file(None);
    app.config.set_value("tests.enabled", Some(fenix_config::Value::Bool(true)));
    let project = app.tests_project(&root);
    // A `tests-read` out: nothing drains what waits.
    project.step = Some(StepRun { tag: 7, run: RunId(1), provider: provider(), step: 0, cwd: root.clone(), runner: None, lines: Batch::default(), reading: true, ending: false, ended: None, failed: None, halt: None, behind: false });
    let empty = || fenix_tasks::OutputLine { stream: OutputStream::Stdout, text: String::new(), cut: false };
    for _ in 0..200 {
        app.tests_apply_event(7, TaskEvent::Output((0..10_000).map(|_| empty()).collect()));
    }
    let step = app.tests.projects[&root].step.as_ref().unwrap();
    assert!(step.behind, "{} lines waiting", step.lines.len());
    assert_eq!(step.lines.len(), 0, "and given no more");
    let mut batch = Batch::default();
    for _ in 0..BEHIND_LINES + 1 {
        batch.push(TestLine { text: String::new(), stderr: false, cut: false });
    }
    assert!(batch.too_much() && batch.bytes == 0);
}

/// An extension started again makes its first scans again: a file
/// written since is found (M2).
#[test]
fn a_restarted_extension_scans_the_project_again() {
    let state = tempfile::tempdir().unwrap();
    let mut p = Project::new(&[("a.fake", "# test one\n"), ("sub/b.fake", "# test two\n")], "a.fake", state.path());
    p.until("the walk", |p| p.has("sub/b.fake::two") && p.tests().walk.is_none());
    std::fs::write(p.root.join("sub/c.fake"), "# test three\n").unwrap();
    p.app.restart_code_extension(EXT);
    p.until("the second walk", |p| p.has("sub/c.fake::three"));
}

/// A switched-off extension isn't asked to scan, and nothing piles up
/// for it as you type (S1).
#[test]
fn nothing_is_asked_of_an_extension_switched_off() {
    let state = tempfile::tempdir().unwrap();
    let mut p = Project::new(&[("a.fake", "# test one\n")], "a.fake", state.path());
    p.scanned("a.fake::one");
    p.until("calm", |p| p.app.tests.calls.is_empty());
    p.app.code_extensions_now(Vec::new());
    let id = p.app.focused_buffer_id();
    for i in 0..10 {
        let ob = p.app.buffers.get_mut(id).unwrap();
        let mut end = Cursor { char_idx: ob.buffer.len_chars(), sticky_col: 0 };
        ob.buffer.insert_str(&mut end, &format!("# test more{i}\n"));
        p.app.tests_tick(Instant::now() + Duration::from_secs(i + 1));
    }
    assert!(p.app.tests.calls.is_empty(), "{:?}", p.app.tests.calls);
    assert!(p.app.tests.seen.values().all(|s| s.providers.is_empty()), "its open files forgot it");
    let root = p.root.clone();
    assert!(matches!(p.app.tests_reach(EXT, &root, true), Reach::No(_)), "and it isn't reached, whoever asks");
}

/// A project reached through a symlink: its files are its own, by the
/// canonical root, from a buffer opened through the link (S2).
#[cfg(unix)]
#[test]
fn a_project_reached_through_a_symlink_has_its_files() {
    let state = tempfile::tempdir().unwrap();
    let mut p = Project::new(&[("a.fake", "# test one\n#! out PASS a.fake::one 1ms\n")], "a.fake", state.path());
    let links = tempfile::tempdir().unwrap();
    let link = links.path().join("link");
    std::os::unix::fs::symlink(&p.root, &link).unwrap();
    let id = p.app.buffers.open_path(&link.join("a.fake"));
    p.app.open_buffer_in_focused_pane(id);
    p.app.project_root = Some(link.clone());
    p.scanned("a.fake::one");
    assert!(p.app.tests.seen.values().all(|s| s.rel == "a.fake"), "{:?}", p.app.tests.seen.values().map(|s| &s.rel).collect::<Vec<_>>());
    p.app.tests_run_file();
    p.until("the run", Project::ended);
    assert_eq!(p.state("a.fake::one"), State::Passed);
    assert_eq!(model_path_in(&link.join("x/y.rs").to_string_lossy(), &p.app.tests_roots(&p.root), &p.root), "x/y.rs", "the link's spelling of a full path");
}

/// Fenix exits: a step's tree is killed before it returns, not left to
/// a thread the exit outruns (S3).
#[test]
fn fenix_exiting_kills_a_steps_tree() {
    let state = tempfile::tempdir().unwrap();
    let mut p = Project::new(&[("a.fake", "# test one\n#! child\n#! wait\n")], "a.fake", state.path());
    p.scanned("a.fake::one");
    p.app.tests_run_file();
    let child = p.child();
    p.app.tests_shutdown();
    assert!(fenix_tasks::fake_runner::gone_within(child, Duration::from_millis(500)), "the child outlived Fenix's exit");
}

/// What runs without a key -- a listing on its own, a run on save -- runs
/// only where `project_is_trusted` says, as a lens's program does: a
/// project that names programs (`.fenix/tools.json`) nobody allowed, no;
/// one that names none, yes (§12, and §16's Q9). A model is made with
/// its listings shut, never open until the first tick (S4, S5).
#[test]
fn what_runs_without_a_key_runs_only_in_a_trusted_project() {
    let state = tempfile::tempdir().unwrap();
    for (names_programs, trusted) in [(true, false), (false, true)] {
        let files: &[(&str, &str)] = if names_programs { &[("a.fake", "# test one\n"), (".fenix/tools.json", "{\"tasks\":{}}")] } else { &[("a.fake", "# test one\n")] };
        let mut p = Project::new(files, "a.fake", state.path());
        p.app.trust.enforced = true;
        p.app.config.set_value("tests.on_save", Some(fenix_config::Value::Text("file".into())));
        let root = p.root.clone();
        // Made between ticks: no listing until it's looked at, then only
        // where trusted.
        p.app.tests_project(&root).model.page_opened();
        let listing = p.app.tests_project(&root).model.next(0).is_some();
        assert_eq!(listing, trusted, "a listing on its own, names programs: {names_programs}");
        p.app.tests_file_saved(&root.join("a.fake"));
        assert_eq!(p.tests().save_run.is_some(), trusted, "a run on save, names programs: {names_programs}");
    }
}

/// Its trust is looked at once a while, not at every wake: a change is
/// seen when a file of its `.fenix` is saved (S6).
#[test]
fn a_projects_trust_is_looked_at_once_a_while() {
    let state = tempfile::tempdir().unwrap();
    let mut p = Project::new(&[("a.fake", "# test one\n")], "a.fake", state.path());
    p.app.trust.enforced = true;
    let root = p.root.clone();
    p.app.tests_project(&root);
    assert!(p.app.tests_trusted(&root));
    std::fs::write(root.join(".fenix/tools.json"), "{\"tasks\":{}}").unwrap();
    assert!(p.app.tests_trusted(&root), "kept: not read again at once");
    p.app.tests_file_saved(&root.join(".fenix/tools.json"));
    assert!(!p.app.tests_trusted(&root), "a save of its .fenix: read again");
}

/// The explorer turned off while a run goes: the run ends, stopped, its
/// steps told, and nothing goes on at each wake (S7).
#[test]
fn turning_the_explorer_off_ends_its_run() {
    let state = tempfile::tempdir().unwrap();
    let mut p = Project::new(&[("a.fake", "# test one\n#! wait\n")], "a.fake", state.path());
    p.scanned("a.fake::one");
    p.app.tests_run_file();
    p.until("the step", Project::step_running);
    p.app.config.set_value("tests.enabled", Some(fenix_config::Value::Bool(false)));
    p.until("the run's end", |p| p.ended() && p.tests().run.is_none() && p.tests().step.is_none());
    assert!(matches!(&p.last().halted, Some(fenix_tests::Halt::Stopped(why)) if why.contains("turned off")));
    assert!(p.app.tests.calls.is_empty());
}

/// The first scan's cap counts the files scanned, those open (from their
/// buffers) among them; and a walk waiting on its thread asks to be
/// woken (S8).
#[test]
fn a_first_scans_cap_counts_what_was_scanned() {
    let state = tempfile::tempdir().unwrap();
    let files: Vec<(String, String)> = (0..5).map(|i| (format!("sub/f{i}.fake"), format!("# test t{i}\n"))).collect();
    let mut all: Vec<(&str, &str)> = files.iter().map(|(n, t)| (n.as_str(), t.as_str())).collect();
    all.push(("a.fake", "# test one\n"));
    let mut p = Project::new(&all, "a.fake", state.path());
    p.app.tests.scan_cap = Some(3);
    p.until("the walk", |p| p.app.tests.projects.get(&p.root).is_some_and(|t| t.scanned_of.contains_key(&provider())));
    assert_eq!(p.tests().scanned_of[&provider()], (3, 6), "a.fake and two others, of six");
    assert!(p.app.extension_output_text().contains("3 of 6 files of provider scanned"));
}

/// A walk let go stops its thread mid-tree (S8).
#[test]
fn a_walk_let_go_stops() {
    let dir = tempfile::tempdir().unwrap();
    for i in 0..50 {
        std::fs::write(dir.path().join(format!("f{i}.fake")), "").unwrap();
    }
    let cancel = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let rx = walk_files(dir.path(), vec!["*.fake".into()], HashSet::new(), 2_000, cancel);
    let mut got = Vec::new();
    while let Ok(walked) = rx.recv_timeout(Duration::from_secs(10)) {
        got.push(matches!(walked, Walked::Done { .. }));
    }
    assert!(got.is_empty(), "nothing sent, not even its end: {got:?}");
}

/// A walk waiting on its thread has the tick wake for it (S8).
#[test]
fn a_walk_waiting_wakes_the_loop() {
    let state = tempfile::tempdir().unwrap();
    let mut p = Project::new(&[("a.fake", "# test one\n"), ("b.fake", "# test two\n")], "a.fake", state.path());
    let root = p.root.clone();
    let spec = p.app.tests_specs().into_iter().next().unwrap().1;
    p.app.tests_project(&root).walks.push_back((spec.id, EXT.into()));
    let wake = p.app.tests_tick(Instant::now());
    assert!(wake.is_some_and(|w| w <= Instant::now() + WALK_POLL), "{wake:?}");
}

// The page (step 4).

impl Project {
    /// The tests page, made if it isn't, laid out at `cols` by `rows` as a
    /// frame would, its head counted from the model as it is now.
    fn page_at(&mut self, cols: usize, rows: usize) -> String {
        let id = self.app.find_page(|m| matches!(m, PageModel::Tests(_))).expect("the tests page");
        let pane = self.app.pane_showing(id).unwrap_or_else(|| self.app.focused_pane_id());
        // Past the tenth of a second the head waits for.
        self.app.tests_views_due(Instant::now() + Duration::from_secs(1));
        self.app.note_page_rows(id, rows);
        self.app.restale_page(id);
        self.app.ensure_page_layout(id, pane, cols);
        self.app.pages[&id].page.text.clone()
    }

    fn page(&mut self) -> String {
        self.page_at(100, 30)
    }

    /// The tests page's buffer, and the pane showing it, focused.
    fn focus_page(&mut self) -> BufferId {
        let id = self.app.find_page(|m| matches!(m, PageModel::Tests(_))).expect("the tests page");
        let pane = self.app.pane_showing(id).expect("shown");
        self.app.windows_mut().focus(pane);
        id
    }

    /// Keys typed on the page, a character each.
    fn keys(&mut self, keys: &str) {
        self.focus_page();
        for c in keys.chars() {
            let key = match c {
                '\n' => KeyPress::named(fenix_keymap::NamedKey::Enter),
                '\t' => KeyPress::named(fenix_keymap::NamedKey::Tab),
                '\u{1b}' => KeyPress::named(fenix_keymap::NamedKey::Escape),
                c => KeyPress::char(c),
            };
            assert!(self.app.page_key(key), "{c:?} is the page's");
        }
    }

    /// `j` until the focused row's text is `label`.
    fn focus_row(&mut self, label: &str) {
        self.keys("gg");
        for _ in 0..40 {
            if self.focused_is(label) {
                return;
            }
            self.keys("j");
        }
        panic!("no row {label}:\n{}", self.page());
    }

    fn focused_is(&self, label: &str) -> bool {
        let Some(id) = self.app.find_page(|m| matches!(m, PageModel::Tests(_))) else { return false };
        let Some(PageModel::Tests(page)) = self.app.pages.get(&id).map(|s| &s.model) else { return false };
        match page.focus {
            Some(fenix_tests::Item::Node(n)) => self.model().node(n).is_some_and(|n| n.found().label == label),
            _ => false,
        }
    }
}

/// The page end to end, with the provider guest and `fake-test-runner`:
/// `SPC T T` opens it on the project's tests; `r` runs the focused file,
/// results come into the tree and the head; a group holding a failure
/// opens; `h` and `l` fold; the filter narrows; `Enter` goes to a test's
/// line beside the page; `o` opens *Test output* beside it; `x` stops a
/// run, the head saying so; closed and opened again, it's as it was.
#[test]
fn the_page_runs_folds_filters_goes_to_a_test_stops_and_shows_the_output() {
    let state = tempfile::tempdir().unwrap();
    let a = "# test adds\n# test fails\n#! out PASS a.fake::adds 3ms\n#! err FAIL a.fake::fails a.fake:2 two and two\n";
    let b = "# test slow\n#! wait\n";
    let mut p = Project::new(&[("a.fake", a), ("b.fake", b)], "a.fake", state.path());
    p.until("the first scan", |p| p.has("a.fake::fails") && p.has("b.fake::slow"));
    let file = p.app.focused_buffer_id();
    p.app.run_cmd("tests.open");
    let page = p.app.focused_buffer_id();
    assert!(matches!(p.app.pages.get(&page).map(|s| &s.model), Some(PageModel::Tests(_))), "the page, in the focused pane");
    assert!(p.app.page_title(page).starts_with("*tests: "));
    p.until("the page's first listing", |p| p.tests().run.is_none() && p.listings() == 1);
    let text = p.page();
    assert!(text.starts_with(" Tests  ") && text.contains("? 3") && text.contains("▸ a.fake") && text.contains("▸ b.fake"), "two files, closed, never run:\n{text}");
    assert!(!p.app.modeline_text().contains("tests "), "nothing to say yet");

    // `r` on a.fake.
    p.focus_row("a.fake");
    p.keys("r");
    p.until("the run", |p| p.model().last_run().is_some_and(|e| matches!(e.kind, fenix_tests::RunKind::Tests(_))));
    let text = p.page();
    assert!(text.contains("✓ 1  ✗ 1  ? 1") && text.contains("▾ a.fake") && text.contains("fails") && text.contains("✓ 3.0 ms"), "the results, the failure's file opened:\n{text}");
    p.app.status_message = None;
    assert!(p.app.modeline_text().contains("✗ 1 test"), "the modeline's item: {}", p.app.modeline_text());
    p.focus_row("fails");
    let text = p.page();
    assert!(text.contains("✗ failed") && text.contains("two and two") && text.contains("at a.fake:3"), "its details:\n{text}");
    p.keys("y");
    assert_eq!(p.app.tests.copied.as_deref(), Some("fails\na.fake:3\ntwo and two"));

    // Folding: `h` to the file, `h` closes it, `l` opens it.
    p.keys("h");
    assert!(p.focused_is("a.fake"));
    p.keys("h");
    assert!(p.page().contains("▸ a.fake"));
    p.keys("l");
    assert!(p.page().contains("▾ a.fake"));

    // The filter.
    p.keys("/add\n");
    let text = p.page();
    assert!(text.contains("adds") && !text.contains("fails") && !text.contains("b.fake"), "{text}");
    p.keys("\u{1b}");
    assert!(p.page().contains("b.fake"), "Esc clears it");

    // `Enter` on a test: its line, beside the page.
    p.focus_row("fails");
    p.keys("\n");
    assert_eq!(p.app.windows().window_count(), 2, "beside the page");
    assert_eq!(p.app.focused_buffer_id(), file);
    assert_eq!(p.app.open().buffer.line_col(&p.app.cursor()).0, 1, "on the test's line");
    assert!(p.app.pane_showing(page).is_some(), "the page stays");

    // `o`: Test output, beside the page, in the pane there.
    p.keys("o");
    let output = p.tests().output.unwrap();
    assert_eq!(p.app.focused_buffer_id(), output);
    assert_eq!(p.app.windows().window_count(), 2);
    assert!(p.app.pane_showing(page).is_some());

    // `x` stops a run going; the head says so.
    p.focus_row("b.fake");
    p.keys("r");
    p.until("the step", Project::step_running);
    let text = p.page();
    assert!(text.contains("running 0 of 1") && text.contains("x stops it"), "{text}");
    p.app.status_message = None;
    assert!(p.app.modeline_text().contains("tests 0/1"), "{}", p.app.modeline_text());
    p.keys("x");
    p.until("the stop", |p| p.tests().run.is_none());
    assert!(p.page().contains("stopped "), "{}", p.page());

    // Closed, and opened again: as it was.
    p.keys("2");
    p.keys("q");
    assert!(p.app.find_page(|m| matches!(m, PageModel::Tests(_))).is_none());
    p.app.run_cmd("tests.open");
    let text = p.page();
    assert!(text.contains("fails") && !text.contains("b.fake"), "on the Failed tab still:\n{text}");
}

/// A place on a line of *Test output* is relative to the folder of the
/// step that printed it: a listing's step runs in the provider's `/data`,
/// so `Enter` on its `notes.txt:2` opens the extension's file, not one of
/// the project's (§20, left for step 4). Lines trimmed past 16 MiB are
/// counted as the buffer's lines, a cut line's pieces as one.
#[test]
fn enter_on_test_output_goes_to_a_place_in_its_steps_folder() {
    let state = tempfile::tempdir().unwrap();
    let mut p = Project::new(&[("a.fake", "# test one\n"), ("notes.txt", "the project's\n")], "a.fake", state.path());
    let data = p.app.wasm.installed[0].dir.join("data");
    std::fs::create_dir_all(&data).unwrap();
    std::fs::write(data.join("fake-list.txt"), "out see notes.txt:2:1\nout FOUND a.fake::listed a.fake\n").unwrap();
    std::fs::write(data.join("notes.txt"), "one\nthe provider's\n").unwrap();
    p.scanned("a.fake::one");
    let root = p.root.clone();
    p.app.tests_list(&root);
    p.until("the listing", |p| p.listings() == 1);
    p.app.run_cmd("tests.output");
    let output = p.tests().output.unwrap();
    assert_eq!(p.app.focused_buffer_id(), output);
    let line = p.output().lines().position(|l| l.contains("see notes.txt:2:1")).unwrap_or_else(|| panic!("{}", p.output()));
    let at = p.app.open().buffer.line_start_char(line);
    p.app.focused_buffer_and_cursor_mut().1.char_idx = at;
    assert!(p.app.task_output_open_location_at_cursor());
    let opened = p.app.open().buffer.path().map(refactor::identity).unwrap();
    assert_eq!(opened, refactor::identity(&data.join("notes.txt")), "the step's folder's");
    assert_eq!(p.app.open().buffer.line_col(&p.app.cursor()).0, 1);
    // The header's line (no step's) is the project's.
    assert_eq!(p.app.tests_output_base(output, 0), Some(root.clone()));

    // Pieces of one line are one line of the count.
    p.app.tests_output_reset(&root, "-- head --\n");
    p.app.tests_output_text(&root, "a piece, ");
    p.app.tests_output_text(&root, "its rest\nnext\n");
    assert_eq!(p.tests().output_lines.len(), 3, "{:?}", p.tests().output_lines);
    assert_eq!(p.output().lines().count(), 3);
}

/// With `tests.enabled` off nothing of the page exists: `SPC T` isn't
/// offered and does nothing, `tests.open` says the explorer is off, the
/// palette and `SPC h b` don't list the explorer's commands, the
/// modeline has no item; turned off with the page open, the page goes.
/// On, `SPC T` is offered with its keys, with Vim's leader and
/// standard keys' `Ctrl+K` alike.
#[test]
fn with_the_explorer_off_nothing_of_the_page_exists() {
    let state = tempfile::tempdir().unwrap();
    let mut p = Project::new(&[("a.fake", "# test one\n")], "a.fake", state.path());
    p.app.config.set_value("tests.enabled", Some(fenix_config::Value::Bool(false)));
    let t = KeyPress::char('T');
    let hints = |app: &App| app.which_key_pending().map(|w| w.hints.iter().map(|h| h.key).collect::<Vec<_>>()).unwrap_or_default();
    p.app.press("SPC");
    assert!(!hints(&p.app).contains(&t), "SPC T isn't offered");
    p.app.press("T");
    assert!(!p.app.leader_matcher.is_pending(), "and isn't a group");
    p.app.run_cmd("tests.open");
    assert!(p.app.status_message.as_ref().is_some_and(|m| m.text.contains("tests.enabled")));
    assert!(p.app.find_page(|m| matches!(m, PageModel::Tests(_))).is_none() && p.app.tests.projects.is_empty());
    assert!(!p.app.command_candidates().iter().any(|c| c.payload.starts_with("tests.")), "not in the palette");
    p.app.open_bindings_picker();
    let listed = match &p.app.active_picker {
        Some(ActivePicker::Bindings(s)) => s.visible_rows(0, s.len()).any(|(_, c)| c.payload.starts_with("tests.")),
        _ => panic!("the bindings picker"),
    };
    assert!(!listed, "nor in SPC h b");
    p.app.picker_cancel();
    p.app.status_message = None;
    assert!(!p.app.modeline_text().contains("tests"), "{}", p.app.modeline_text());
    assert_eq!(p.app.tests_views_due(Instant::now()), (false, None), "nothing to do per wake");

    // On: SPC T and its keys.
    p.app.config.set_value("tests.enabled", Some(fenix_config::Value::Bool(true)));
    p.app.press("SPC");
    assert!(hints(&p.app).contains(&t));
    p.app.press("T");
    assert!(p.app.leader_matcher.is_pending());
    let under: Vec<char> = hints(&p.app).into_iter().filter_map(|k| match k.code { fenix_keymap::KeyCode::Char(c) => Some(c), _ => None }).collect();
    for key in ['T', 't', 'f', 'a', 'F', 'l', 'k', 'o'] {
        assert!(under.contains(&key), "SPC T {key}: {under:?}");
    }
    p.app.press("T");
    assert!(p.app.find_page(|m| matches!(m, PageModel::Tests(_))).is_some(), "SPC T T opens it");
    assert!(p.app.command_candidates().iter().any(|c| c.payload == "tests.open" && c.label.starts_with("Tests ›")), "the palette files them under Tests");
    // Standard keys: Ctrl+K T T.
    p.app.config.set_value("editor.keys", Some(fenix_config::Value::Text("standard".into())));
    p.app.sync_key_policy();
    p.app.press("C-k");
    p.app.press("T");
    assert!(p.app.leader_matcher.is_pending() && p.app.leader_matcher.label() == Some("tests"), "Ctrl+K T is the tests group");
    p.app.press("Esc");

    // Turned off with the page open: it goes.
    p.app.config.set_value("tests.enabled", Some(fenix_config::Value::Bool(false)));
    assert!(p.app.tests_views_due(Instant::now()).0);
    assert!(p.app.find_page(|m| matches!(m, PageModel::Tests(_))).is_none(), "closed");
    p.app.status_message = None;
    assert!(!p.app.modeline_text().contains("tests"), "{}", p.app.modeline_text());
}

/// The page's colours in a dark and a light theme: a pass, a failure and
/// what's muted are each their own, and read on the page.
#[test]
fn the_pages_marks_read_in_a_dark_and_a_light_theme() {
    let state = tempfile::tempdir().unwrap();
    let a = "# test adds\n# test fails\n#! out PASS a.fake::adds 3ms\n#! err FAIL a.fake::fails a.fake:2 two and two\n";
    let mut p = Project::new(&[("a.fake", a)], "a.fake", state.path());
    p.scanned("a.fake::fails");
    p.app.tests_run_file();
    p.until("the run", Project::ended);
    p.app.run_cmd("tests.open");
    let is_light = |t: &crate::theme::Theme| crate::theme::luminance_of(t.bg) > 0.5;
    let dark = crate::theme::all().iter().find(|t| !is_light(t) && t.name != "TempleOS").unwrap();
    let light = crate::theme::all().iter().find(|t| is_light(t) && t.name != "TempleOS").unwrap();
    for theme in [*dark, *light] {
        p.app.theme = theme;
        let text = p.page();
        let id = p.focus_page();
        let ranges = p.app.page_highlights(id, 0, 30);
        let buffer = p.app.buffers.get(id).unwrap().buffer.text();
        let color_of = |needle: &str| {
            let at = buffer.find(needle).unwrap_or_else(|| panic!("{needle} in\n{text}"));
            ranges.iter().find(|(r, _)| r.start <= at && at < r.end).map(|(_, c)| *c).unwrap_or_else(|| panic!("{needle} uncoloured"))
        };
        let (pass, fail) = (color_of("✓ 3.0 ms"), color_of("✗ failed"));
        assert_ne!(pass, fail, "{}", theme.name);
        for c in [pass, fail] {
            let ratio = crate::theme::contrast_of(glyphon_to_rgba(c), theme.bg);
            assert!(ratio >= 3.0, "{}: {c:?} on its page: {ratio:.2}:1", theme.name);
        }
    }
}

// The tests in the file (step 5).

#[test]
fn a_place_is_a_file_of_the_project_or_a_full_path_never_one_that_climbed_out() {
    let root = Path::new(if cfg!(windows) { r"C:\src\orbit" } else { "/src/orbit" });
    assert_eq!(place_path(root, "src/lib.rs"), Some(root.join("src/lib.rs")));
    assert_eq!(place_path(root, r"src\lib.rs"), Some(root.join("src/lib.rs")));
    assert_eq!(place_path(root, "/project/../x.rs"), None, "it climbed out of the project: no place, never opened");
    assert_eq!(place_path(root, "/project"), None);
    assert_eq!(place_path(root, ""), None);
    let outside = if cfg!(windows) { r"C:\rust\lib\core.rs" } else { "/rust/lib/core.rs" };
    assert_eq!(place_path(root, outside), Some(PathBuf::from(outside)));
    // A verbatim path is the plain one it names, under the project or not.
    let drive = Path::new("C:/src/orbit");
    assert_eq!(model_path(r"\\?\C:\src\orbit\src\lib.rs", drive, drive), "src/lib.rs");
    let share = Path::new("//server/share/orbit");
    assert_eq!(model_path(r"\\?\UNC\server\share\orbit\a.rs", share, share), "a.rs");
    assert_eq!(file::short_message("assertion failed:\n  two\tand two"), "assertion failed: two and two");
    let cut = file::short_message(&"x".repeat(60));
    assert!(cut.chars().count() == 48 && cut.ends_with('…'), "{cut}");
}

/// A file of `lines` lines, open and focused, in a project of its own,
/// the explorer on; the model given `tests` (id, line) as a provider's
/// scan of it, with no extension, and the buffer seen.
fn served(lines: usize, tests: &[(&str, u32)]) -> (App, PathBuf, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let root = refactor::identity(dir.path());
    std::fs::create_dir_all(root.join(".fenix")).unwrap();
    std::fs::write(root.join(".fenix/project.ini"), "").unwrap();
    let text: String = (0..lines).map(|i| format!("line {i}\n")).collect();
    std::fs::write(root.join("a.fake"), text).unwrap();
    let mut app = App::with_file(Some(root.join("a.fake").to_string_lossy().into_owned()));
    app.project_root = Some(root.clone());
    app.line_number_mode = LineNumberMode::Absolute;
    app.config.set_value("tests.enabled", Some(fenix_config::Value::Bool(true)));
    let model = &mut app.tests_project(&root).model;
    model.add_provider(&provider(), "fake");
    model.scan(&provider(), "a.fake", tests.iter().map(|(id, line)| fenix_tests::Found::new(*id, None, *id, Kind::Test).at("a.fake", *line, None)).collect());
    app.tests_tick(Instant::now());
    (app, root, dir)
}

/// The model's tests all run, each given its report: as a host would,
/// with one step and no runner.
fn ran(model: &mut Model, reports: Vec<fenix_tests::Report>) {
    assert_ne!(model.request(Selection::All, fenix_tests::Mode::Queue), fenix_tests::Requested::Nothing);
    let mut reports = Some(reports);
    let mut now = 1_000;
    for _ in 0..100 {
        now += 10;
        match model.next(now) {
            None => return,
            Some(fenix_tests::Action::Plan { run, .. }) => model.planned(run, Ok(fenix_tests::Plan { steps: vec![fenix_tests::Step::new("all", &[])], list_after: false })),
            Some(fenix_tests::Action::Step { run, step, .. }) => model.step_ended(run, step, fenix_tests::StepEnd::Exited(Some(0)), reports.take().unwrap_or_default()),
            Some(_) => {}
        }
    }
    panic!("the run never ended");
}

fn rows_of(app: &App, rows: usize) -> Vec<String> {
    let gutter = app.gutter_chars(app.open());
    let spans = app.content_spans(app.open(), 0, rows, gutter, None, &[], 0, &TabStops::Fixed(4), 0);
    spans.iter().map(|(s, _)| s.as_str()).collect::<String>().split('\n').map(str::to_string).collect()
}

/// The test column's width and its cell beside the blame, the numbers,
/// the fold column and a breakpoint, in every combination the gutter's
/// own tests cover: one cell more, left of the numbers, only in a file
/// with tests, only with line numbers, the explorer and `tests.gutter`
/// on; a click on a mark runs it, a click beside it doesn't; a
/// breakpoint drawn over the cell wins its line.
#[test]
fn the_test_column_is_one_cell_left_of_the_numbers_in_every_combination() {
    #[derive(Clone, Copy, Debug, PartialEq)]
    enum Tests {
        Served,
        Without,
        ExplorerOff,
        GutterOff,
    }
    for numbers in [LineNumberMode::Absolute, LineNumberMode::Relative, LineNumberMode::Off] {
        for fold in [true, false] {
            for blame in [false, true] {
                for tests in [Tests::Served, Tests::Without, Tests::ExplorerOff, Tests::GutterOff] {
                    for breakpoint in [false, true] {
                        let what = format!("{numbers:?}, folds {fold}, blame {blame}, {tests:?}, breakpoint {breakpoint}");
                        let (mut app, root, _dir) = served(12, if tests == Tests::Without { &[] } else { &[("a.fake::t", 4)] });
                        app.line_number_mode = numbers;
                        app.config.set_value("editor.fold_markers", Some(fenix_config::Value::Bool(fold)));
                        let path = app.open().buffer.path().unwrap().to_path_buf();
                        if blame {
                            app.test_blame(&path, 12);
                        }
                        match tests {
                            Tests::ExplorerOff => app.config.set_value("tests.enabled", Some(fenix_config::Value::Bool(false))),
                            Tests::GutterOff => app.config.set_value("tests.gutter", Some(fenix_config::Value::Bool(false))),
                            _ => {}
                        }
                        if breakpoint {
                            let at = app.open().buffer.line_start_char(4);
                            app.test_set_cursor(Cursor { char_idx: at, sticky_col: 0 });
                            app.debug_toggle_breakpoint();
                            app.test_set_cursor(Cursor { char_idx: 0, sticky_col: 0 });
                        }
                        let blame_w = if blame { git_editor::BLAME_WIDTH } else { 0 };
                        let on = numbers != LineNumberMode::Off;
                        let base = if on { 2 + 1 + blame_w + usize::from(fold) } else { blame_w };
                        let column = on && tests == Tests::Served;
                        let gutter = app.gutter_chars(app.open());
                        assert_eq!(gutter, base + usize::from(column), "{what}");
                        let row: Vec<char> = rows_of(&app, 12)[4].chars().collect();
                        assert_eq!(row[gutter..].iter().collect::<String>(), "line 4", "{what}: the text starts where it did");
                        let drawn = column && !(breakpoint && !blame);
                        if column {
                            assert_eq!(row[blame_w], if drawn { '?' } else { ' ' }, "{what}");
                        }
                        if on {
                            let number: String = row[blame_w + usize::from(column)..gutter - 1 - usize::from(fold)].iter().collect();
                            assert_eq!(number.trim_start(), if numbers == LineNumberMode::Relative { "4" } else { "5" }, "{what}: {number:?}");
                            assert_eq!(number.len(), 2, "{what}: the numbers keep their width");
                        }
                        let pane = app.focused_pane_id();
                        assert!(!app.tests_mark_click(pane, 4, blame_w + 1), "{what}: the number isn't the mark");
                        assert!(!app.tests_mark_click(pane, 1, blame_w), "{what}: a line without a mark");
                        assert_eq!(app.tests_mark_click(pane, 4, blame_w), drawn, "{what}");
                        let asked = app.tests.projects.get(&root).and_then(|p| p.model.last_selection()).map(<[Selection]>::to_vec);
                        assert_eq!(asked.is_some(), drawn, "{what}: {asked:?}");
                        if drawn {
                            assert_eq!(asked.unwrap(), [Selection::Nodes(vec![TestKey::new(&provider(), "a.fake::t")])]);
                        }
                    }
                }
            }
        }
    }
}

/// Marks: the worst state of what starts on a line, in its colour, dimmed
/// when stale; and the right click's items on a test's lines only.
#[test]
fn a_marks_state_colour_and_the_right_clicks_items() {
    let (mut app, root, _dir) = served(12, &[("a.fake::ok", 1), ("a.fake::bad", 4)]);
    let model = &mut app.tests.projects.get_mut(&root).unwrap().model;
    let failed = fenix_tests::Ended { message: Some("two and two".into()), ..fenix_tests::Ended::new("a.fake::bad", Outcome::Failed) };
    ran(model, vec![fenix_tests::Report::Ended(fenix_tests::Ended::new("a.fake::ok", Outcome::Passed)), fenix_tests::Report::Ended(failed)]);
    let cells = app.tests_cells(app.open(), 0..=11).unwrap();
    let role = |r| crate::app::pages::role_color(app.theme, r);
    assert_eq!(cells.at(1).map(|c| (c.0, c.1)), Some(('✓', role(crate::page::Role::Good))));
    assert_eq!(cells.at(4).map(|c| (c.0, c.1)), Some(('✗', role(crate::page::Role::Bad))));
    assert!(cells.at(0).is_none() && cells.at(5).is_none());
    let labels = |app: &App| app.text_menu_items().into_iter().map(|i| i.label).collect::<Vec<_>>();
    assert!(!labels(&app).contains(&"Run test".to_string()), "line 1 of a.fake is in no test");
    let at = app.open().buffer.line_start_char(4);
    app.test_set_cursor(Cursor { char_idx: at, sticky_col: 0 });
    let items = labels(&app);
    let after = items.iter().position(|l| l == "Toggle breakpoint").unwrap();
    assert_eq!(&items[after + 1..after + 4], ["Run test", "Debug test", "Show in Tests"], "beside Toggle breakpoint, Debug test after Run test: {items:?}");
    // Saved: stale, dimmed, its glyph kept.
    let path = app.open().buffer.path().unwrap().to_path_buf();
    app.tests_file_saved(&path);
    let cells = app.tests_cells(app.open(), 0..=11).unwrap();
    assert_eq!(cells.at(4).map(|c| (c.0, c.1)), Some(('✗', role(crate::page::Role::Muted))));
    // Show in Tests: the page, on the test.
    app.run_menu_action(crate::app::mouse::MenuAction::ShowTest);
    let id = app.find_page(|m| matches!(m, PageModel::Tests(_))).expect("the tests page");
    let Some(PageModel::Tests(page)) = app.pages.get(&id).map(|s| &s.model) else { panic!() };
    let bad = app.tests.projects[&root].model.find(&TestKey::new(&provider(), "a.fake::bad"));
    assert_eq!(page.focus, bad.map(fenix_tests::Item::Node));
}

/// With `tests.enabled` off the file shows nothing of the explorer: no
/// column and no width, no marks, no results after the lines, no
/// problems, no menu items, a click on the cell does nothing; and what
/// was shown goes at the next wake. Never turned on, nothing is kept to
/// look at as a frame is drawn.
#[test]
fn with_the_explorer_off_the_file_shows_nothing_of_it() {
    let mut never = App::with_file(None);
    assert!(never.tests.by_path.is_empty() && never.tests.projects.is_empty());
    assert!(never.tests_cells(never.open(), 0..=10).is_none());
    assert_eq!(never.tests_views_due(Instant::now()), (false, None));

    let (mut app, root, _dir) = served(12, &[("a.fake::bad", 4)]);
    let failed = fenix_tests::Ended { message: Some("two and two".into()), ..fenix_tests::Ended::new("a.fake::bad", Outcome::Failed) };
    ran(&mut app.tests.projects.get_mut(&root).unwrap().model, vec![fenix_tests::Report::Ended(failed)]);
    app.tests_views_due(Instant::now() + Duration::from_secs(1));
    let path = app.open().buffer.path().unwrap().to_path_buf();
    let canonical = refactor::identity(&path);
    let ours = |app: &App| app.diagnostics.for_path(&canonical).len();
    let inline = |app: &App| app.wasm.decorations.get(&app.focused_buffer_id()).is_some_and(|sets| sets.iter().any(|(who, _)| who == file::TESTS_OWNER));
    assert_eq!(ours(&app), 1);
    assert!(inline(&app));
    let with = app.gutter_chars(app.open());

    app.config.set_value("tests.enabled", Some(fenix_config::Value::Bool(false)));
    assert_eq!(app.gutter_chars(app.open()), with - 1, "no column, no width");
    assert!(app.tests_cells(app.open(), 0..=11).is_none());
    assert!(!rows_of(&app, 12)[4].contains('✗'));
    let at = app.open().buffer.line_start_char(4);
    app.test_set_cursor(Cursor { char_idx: at, sticky_col: 0 });
    assert!(app.tests_test_here().is_none());
    assert!(!app.text_menu_items().iter().any(|i| i.label == "Run test" || i.label == "Debug test" || i.label == "Show in Tests"));
    assert!(!app.tests_mark_click(app.focused_pane_id(), 4, 0));
    assert!(app.tests_views_due(Instant::now()).0, "a frame to take it away");
    assert_eq!(ours(&app), 0, "no problems");
    assert!(!inline(&app), "no results after the lines");
    assert_eq!(app.tests_views_due(Instant::now()), (false, None), "then nothing, each wake");
}

/// What a frame costs with 10,000 tests in the project and 5,000 in the
/// open file, a fifth of them failed: the column's width, its marks and
/// the rows drawn, the test the cursor is in at each move; and the wake's
/// work when the model changes (problems, the results after the lines).
/// The marks cost the lines drawn: a frame at the file's end costs what
/// one at its start does.
#[test]
fn a_frames_cost_with_5000_tests_in_the_file_is_the_lines_it_draws() {
    let lines = 10_000usize;
    let ids: Vec<String> = (0..5_000).map(|i| format!("a.fake::t{i}")).collect();
    let tests: Vec<(&str, u32)> = ids.iter().enumerate().map(|(i, id)| (id.as_str(), (i * 2) as u32)).collect();
    let (mut app, root, _dir) = served(lines, &tests);
    {
        let model = &mut app.tests.projects.get_mut(&root).unwrap().model;
        model.scan(&provider(), "b.fake", (0..5_000).map(|i| fenix_tests::Found::new(format!("b.fake::t{i}"), None, "t", Kind::Test).at("b.fake", i, None)).collect());
        let reports = ids
            .iter()
            .enumerate()
            .map(|(i, id)| {
                let outcome = if i % 5 == 0 { Outcome::Failed } else { Outcome::Passed };
                fenix_tests::Report::Ended(fenix_tests::Ended { message: (outcome == Outcome::Failed).then(|| format!("assertion failed: {i}")), ..fenix_tests::Ended::new(id, outcome) })
            })
            .collect();
        ran(model, reports);
        assert_eq!(model.summary().counts.tests(), 10_000);
    }
    let started = Instant::now();
    assert!(app.tests_in_files(std::slice::from_ref(&root)));
    let wake = started.elapsed();

    let frame = |app: &App, top: usize| {
        let ob = app.open();
        let gutter = app.gutter_chars(ob);
        let spans = app.content_spans(ob, top, 60, gutter, None, &[], top, &TabStops::Fixed(4), 0);
        assert_eq!(spans.iter().any(|(s, _)| s == "✗"), app.tests_enabled());
    };
    let time = |app: &App, top: usize| {
        let started = Instant::now();
        for _ in 0..100 {
            frame(app, top);
        }
        started.elapsed() / 100
    };
    let (start, end) = (time(&app, 0), time(&app, lines - 70));
    // The cursor moving: the test it's in, at every line of a screen.
    let model = &app.tests.projects[&root].model;
    let started = Instant::now();
    for line in 5_000..5_060 {
        assert!(model.test_at("a.fake", line).is_some());
    }
    let moves = started.elapsed() / 60;
    app.config.set_value("tests.enabled", Some(fenix_config::Value::Bool(false)));
    let off = time(&app, 0);
    eprintln!("10,000 tests, 5,000 in the file: a frame's rows {start:?} at its start, {end:?} at its end, {off:?} with the explorer off; a cursor move's test_at {moves:?}; the wake after a run {wake:?}");
    assert!(start < Duration::from_millis(20) && end < Duration::from_millis(20), "{start:?} {end:?}");
    assert!(end < start * 4 + Duration::from_millis(1), "the file's end costs what its start does: {start:?} {end:?}");
    assert!(moves < Duration::from_millis(5), "{moves:?}");
    assert!(wake < Duration::from_secs(2), "{wake:?}");
}

impl Project {
    /// A wake past the tenth of a second the views wait for.
    fn wake(&mut self) {
        std::thread::sleep(Duration::from_millis(120));
        self.app.tests_views_due(Instant::now());
    }

    fn path(&self, name: &str) -> PathBuf {
        refactor::identity(&self.root.join(name))
    }

    /// The problems on file `name`: (source, line, message).
    fn problems(&self, name: &str) -> Vec<(String, u32, String)> {
        let mut out: Vec<(String, u32, String)> = self.app.diagnostics.for_path(&self.path(name)).into_iter().map(|d| (d.source.unwrap_or_default(), d.range.start.line, d.message)).collect();
        out.sort();
        out
    }

    /// The results after the lines of the focused buffer: (line, text, role).
    fn inline(&self) -> Vec<(usize, String, crate::page::Role)> {
        let ob = self.app.open();
        let sets = self.app.wasm.decorations.get(&self.app.focused_buffer_id());
        sets.into_iter()
            .flatten()
            .filter(|(who, _)| who == file::TESTS_OWNER)
            .flat_map(|(_, set)| set.iter())
            .filter_map(|d| match &d.look {
                crate::decorations::Look::Text { text, role, .. } => Some((ob.buffer.line_col(&Cursor { char_idx: d.start, sticky_col: 0 }).0, text.clone(), *role)),
                _ => None,
            })
            .collect()
    }

    fn glyph(&self, line: usize) -> Option<char> {
        self.app.tests_cells(self.app.open(), line..=line).and_then(|c| c.at(line)).map(|c| c.0)
    }
}

/// The file end to end, with the provider guest and `fake-test-runner`: a
/// click on a test's mark runs it; its failure is shown after its line,
/// cleaned and cut, and is a problem at its place beside an extension's
/// own for the same file, neither replacing the other; confirming it in
/// the problems' list goes to its line; a run that passes clears it; an
/// edit moves the marks with the file's next scan and leaves them as they
/// were, and a save makes them stale: dimmed, the problem gone.
#[test]
fn a_failure_from_the_gutter_is_shown_at_its_line_and_as_a_problem_beside_an_extensions() {
    let state = tempfile::tempdir().unwrap();
    let long = "assertion failed: left == right, left: 2, right: 4, at the second try";
    let failing = format!("# test adds\n# test fails\n#! out PASS a.fake::adds 3ms\n#! err FAIL a.fake::fails a.fake:1 {long}\n");
    let passing = "# test adds\n# test fails\n#! out PASS a.fake::adds 3ms\n#! out PASS a.fake::fails 1ms\n";
    let mut p = Project::new(&[("a.fake", &failing)], "a.fake", state.path());
    // The jump below opens the file again, which moves an old
    // `.fenix/project.ini` away: the project keeps a marker after it.
    std::fs::write(p.root.join(".fenix/settings.toml"), "").unwrap();
    p.scanned("a.fake::fails");
    p.app.tests_tick(Instant::now());
    assert!(p.app.tests_column(p.app.open()), "a file with tests has the column");
    assert_eq!(p.glyph(1), Some('?'));
    // An extension's own problem on the same file.
    let canonical = p.path("a.fake");
    let theirs = |message: &str| lsp_types::Diagnostic { range: lsp_types::Range::default(), severity: Some(lsp_types::DiagnosticSeverity::WARNING), source: Some("lint".into()), message: message.into(), ..Default::default() };
    let key = LspKey { server: LspServer::Additional(super::super::wasm_host::problems_key("test.lint")), root: p.root.clone() };
    p.app.publish_diagnostics(canonical.clone(), key.clone(), "test.lint", vec![theirs("unused")]);

    // A click on its mark runs it.
    let pane = p.app.focused_pane_id();
    assert!(p.app.tests_mark_click(pane, 1, 0));
    p.until("the run", Project::ended);
    assert_eq!(p.state("a.fake::fails"), State::Failed);
    p.wake();
    assert_eq!(p.glyph(1), Some('✗'));
    assert_eq!(p.glyph(0), Some('✓'));
    let shown = format!(" ✗ {}…", &long[..47]);
    assert_eq!(p.inline(), [(1, shown.clone(), crate::page::Role::Bad)], "after its line, cut at 48 characters");
    let problems = p.problems("a.fake");
    assert_eq!(problems.iter().map(|(_, l, m)| (*l, m.as_str())).collect::<Vec<_>>(), [(1, long), (0, "unused")], "both, neither in the other's place: {problems:?}");
    let ours = p.app.diagnostics.for_path(&canonical).into_iter().find(|d| d.source.as_deref() != Some("lint")).unwrap();
    let runner = crate::tests_page::clean(p.model().providers()[0].name);
    assert_eq!((ours.severity, ours.code.clone(), ours.source.clone()), (Some(lsp_types::DiagnosticSeverity::ERROR), Some(lsp_types::NumberOrString::String("fails".into())), Some(runner)), "an error, its code the test's label, its source the runner");
    // The extension sets its problems again: ours stay; and the other way.
    p.app.publish_diagnostics(canonical.clone(), key.clone(), "test.lint", vec![theirs("unused again")]);
    assert_eq!(p.problems("a.fake").len(), 2);

    // The problems' list goes to it: ours first, an error.
    p.app.picker_problems();
    let Some(ActivePicker::Problems(list)) = &p.app.active_picker else { panic!("the problems' list") };
    let rows: Vec<String> = list.visible_rows(0, list.len()).map(|(_, c)| c.label.clone()).collect();
    assert!(rows.len() == 2 && rows[0].starts_with("error") && rows[0].contains("a.fake:2") && rows[0].contains("assertion failed") && rows[1].contains("unused again"), "{rows:?}");
    let end = p.app.open().buffer.line_start_char(3);
    p.app.test_set_cursor(Cursor { char_idx: end, sticky_col: 0 });
    p.app.picker_confirm();
    assert_eq!(p.app.open().buffer.line_col(&p.app.cursor()).0, 1, "at the failure's line");

    // A run that passes clears it; theirs stays.
    // The runner reads the file; the buffer keeps its text. Noted as
    // Fenix's own write, so the save below isn't refused as a change made
    // elsewhere.
    std::fs::write(p.root.join("a.fake"), passing).unwrap();
    let id = p.app.focused_buffer_id();
    p.app.note_disk_state(id);
    p.app.tests_run_file();
    p.until("the passing run", |p| p.state("a.fake::fails") == State::Passed && p.tests().run.is_none());
    p.wake();
    assert_eq!(p.problems("a.fake"), [("lint".to_string(), 0, "unused again".to_string())]);
    assert_eq!(p.inline(), [], "a passed test says nothing with tests.inline = failed");
    assert_eq!(p.glyph(1), Some('✓'));
    p.app.config.set_value("tests.inline", Some(fenix_config::Value::Text("all".into())));
    p.wake();
    assert_eq!(p.inline().iter().map(|(l, t, _)| (*l, t.clone())).collect::<Vec<_>>(), [(0, " ✓ 3.0 ms".to_string()), (1, " ✓ 1.0 ms".to_string())]);
    p.app.config.set_value("tests.inline", Some(fenix_config::Value::Text("failed".into())));

    // Failing again; then an edit above it: the marks move with the next
    // scan, as they were, not stale.
    std::fs::write(p.root.join("a.fake"), &failing).unwrap();
    p.app.note_disk_state(id);
    p.app.tests_run_file();
    p.until("the failing run", |p| p.state("a.fake::fails") == State::Failed && p.tests().run.is_none());
    p.wake();
    assert_eq!(p.problems("a.fake").len(), 2);
    p.app.test_set_cursor(Cursor { char_idx: 0, sticky_col: 0 });
    let (buffer, cursor) = p.app.focused_buffer_and_cursor_mut();
    buffer.insert_char(cursor, '\n');
    p.until("the edit's scan", |p| p.model().node(p.model().find(&TestKey::new(&provider(), "a.fake::fails")).unwrap()).unwrap().found().line == Some(2));
    p.wake();
    assert_eq!((p.glyph(1), p.glyph(2)), (Some('✓'), Some('✗')), "moved with the file");
    let fails = p.model().node(p.model().find(&TestKey::new(&provider(), "a.fake::fails")).unwrap()).unwrap();
    assert!(!fails.stale(), "an edit doesn't make a result stale: the runner reads the file as saved");
    assert_eq!(p.inline(), [(2, shown.clone(), crate::page::Role::Bad)]);

    // Saved: stale, dimmed; the problem goes, the result stays after its
    // line, muted; the extension's problem stays.
    p.app.save();
    p.wake();
    let fails = p.model().node(p.model().find(&TestKey::new(&provider(), "a.fake::fails")).unwrap()).unwrap();
    assert!(fails.stale());
    assert_eq!(p.app.tests_cells(p.app.open(), 2..=2).and_then(|c| c.at(2)).map(|c| c.1), Some(crate::app::pages::role_color(p.app.theme, crate::page::Role::Muted)));
    assert_eq!(p.problems("a.fake"), [("lint".to_string(), 0, "unused again".to_string())]);
    assert_eq!(p.inline(), [(2, shown, crate::page::Role::Muted)]);

    // An edit not saved, then the buffer closed without saving: the file
    // has no unsaved edits any more (§8.4).
    let (buffer, cursor) = p.app.focused_buffer_and_cursor_mut();
    buffer.insert_char(cursor, 'x');
    p.until("the unsaved edit seen", |p| p.model().is_unsaved("a.fake"));
    p.app.buffers.close(id);
    p.app.tests_tick(Instant::now());
    assert!(!p.model().is_unsaved("a.fake"));
    assert!(p.app.tests.by_path.is_empty() && p.app.tests.inline.is_empty());
}

// Debugging a test (step 6, `debug`).

/// A debug adapter already listening, as a launch's `debugServer` names
/// one (a recorded exchange's shape: `initialize`, `launch`, then
/// `initialized` and `configurationDone`): it hands on `launch`'s
/// arguments and answers what follows until Fenix lets it go.
fn fake_adapter() -> (u16, mpsc::Receiver<serde_json::Value>) {
    use std::io::Write;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let Ok((stream, _)) = listener.accept() else { return };
        let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
        let write = |message: serde_json::Value| {
            let body = message.to_string();
            let _ = (&stream).write_all(format!("Content-Length: {}\r\n\r\n{body}", body.len()).as_bytes());
        };
        let mut seq = 1;
        while let Some(frame) = read_lsp_frame(&mut reader) {
            let asked: serde_json::Value = serde_json::from_str(&frame).unwrap();
            if asked["type"] != "request" {
                continue;
            }
            let body = if asked["command"] == "initialize" { serde_json::json!({"supportsConfigurationDoneRequest": true}) } else { serde_json::json!({}) };
            seq += 1;
            write(serde_json::json!({"seq": seq, "type": "response", "request_seq": asked["seq"], "success": true, "command": asked["command"], "body": body}));
            if asked["command"] == "launch" {
                let _ = tx.send(asked["arguments"].clone());
                seq += 1;
                write(serde_json::json!({"seq": seq, "type": "event", "event": "initialized"}));
            }
        }
    });
    (port, rx)
}

impl Project {
    /// A project of `files` whose provider's launches name an adapter
    /// listening (`fake-debug.json`, which the guest's launches take).
    fn debugging(files: &[(&str, &str)], focused: &str, state: &Path) -> (Project, mpsc::Receiver<serde_json::Value>) {
        let (port, launches) = fake_adapter();
        let json = format!("{{\"debugServer\": {port}}}");
        let mut all: Vec<(&str, &str)> = files.to_vec();
        all.push(("fake-debug.json", &json));
        // A debug session's start moves an empty `.fenix/project.ini` into
        // `settings.toml`, and leaves no marker if it's empty: one stays.
        all.push((".fenix/settings.toml", "# a project\n"));
        (Project::new(&all, focused, state), launches)
    }

    /// `launch`'s arguments, once the adapter has them.
    fn launched(&mut self, launches: &mpsc::Receiver<serde_json::Value>) -> serde_json::Value {
        let started = Instant::now();
        loop {
            if let Ok(arguments) = launches.try_recv() {
                return arguments;
            }
            assert!(started.elapsed() < Duration::from_secs(120), "no launch: {:?}\n--- Test output ---\n{}", self.app.status_message.as_ref().map(|m| m.text.clone()), self.output());
            std::thread::sleep(Duration::from_millis(5));
            self.app.pump_wasm();
            self.app.tests_tick(Instant::now());
            let events: Vec<_> = self.app.dap_unread.iter().flat_map(|(generation, rx)| rx.try_iter().map(|e| (*generation, e)).collect::<Vec<_>>()).collect();
            for (generation, event) in events {
                self.app.handle_user_event(FenixUserEvent::Dap { generation, event });
            }
        }
    }

    fn said(&self) -> String {
        self.app.status_message.as_ref().map(|m| m.text.clone()).unwrap_or_default()
    }

    fn debug_done(&self) -> bool {
        self.tests().debug.is_none()
    }

    fn builds(&self) -> Vec<fenix_tests::RunEnd> {
        self.tests().ended_runs.iter().filter(|e| matches!(e.kind, fenix_tests::RunKind::Build { .. })).cloned().collect()
    }
}

/// `d` on the page, a test whose provider launches it now: the session
/// starts from its launch, its paths and `${data}` Fenix's, its file open
/// at its line beside the page, nothing built, and it isn't the project's
/// last launch (`SPC u u` doesn't start it again). One its provider
/// refuses says why and stops nothing: the provider runs tests after.
#[test]
fn a_test_is_debugged_from_the_launch_its_provider_gives() {
    let state = tempfile::tempdir().unwrap();
    let a = "\n\n# each launched\n# test plain\n#! out PASS a.py::plain 1ms\n";
    let (mut p, launches) = Project::debugging(&[("a.py", a)], "a.py", state.path());
    p.scanned("a.py::plain");
    let file = p.app.focused_buffer_id();
    p.app.run_cmd("tests.open");
    p.until("the page's first listing", |p| p.tests().run.is_none() && p.listings() == 1);
    assert!(p.page().contains("d debug"), "on the key strip:\n{}", p.page());
    p.focus_row("launched");
    p.keys("d");
    let run = p.tests().debug.as_ref().expect("asked").run;
    let launch = p.launched(&launches);
    let data = p.app.wasm.installed[0].dir.join("data");
    assert_eq!(launch["program"], serde_json::json!(data.join("fake-debugger")), "{launch}");
    assert_eq!(launch["args"], serde_json::json!(["a.py::launched", format!("run {}", run.0)]));
    assert_eq!(launch["env"]["AT"], serde_json::json!(data.to_string_lossy()), "${{data}} filled in");
    assert_eq!(launch["data"], serde_json::json!("{\"template\":\"launched\"}"), "its attributes, its data given back");
    assert!(launch.get("debugServer").is_none(), "not sent to the adapter");
    assert!(p.app.debug_session.is_some());
    assert_eq!(p.app.focused_buffer_id(), file, "its file, beside the page");
    assert_eq!(p.app.open().buffer.line_col(&p.app.cursor()).0, 2, "at its line");
    assert!(p.app.debug_last_launch.is_empty(), "not remembered: {:?}", p.app.debug_last_launch);
    assert!(p.builds().is_empty() && p.debug_done());
    p.app.debug_stop();
    p.app.debug_session = None;

    // A refusal: said, nothing stopped.
    p.open("a.py");
    let at = p.app.open().buffer.line_start_char(3);
    p.app.test_set_cursor(Cursor { char_idx: at, sticky_col: 0 });
    p.app.run_cmd("tests.debug_here");
    assert!(p.tests().debug.is_some(), "{}", p.said());
    p.until("the refusal", Project::debug_done);
    assert_eq!(p.said(), "plain can't be debugged: a.py::plain can't be debugged");
    assert!(p.app.debug_session.is_none());
    p.app.tests_run_file();
    p.until("a run after", |p| p.model().last_run().is_some());
    assert_eq!(p.state("a.py::plain"), State::Passed, "its provider still runs");
}

/// `SPC T d` on a test its provider builds first: the plan runs as the
/// run the first `tests-launch` was asked with, its output in *Test
/// output*, read by the provider; then it's asked again with the same
/// run, and the launch it gives names the binary its build said, as a
/// `/project/...` path mapped to the project's. A second debug while one
/// is building is refused; a build asked while a run goes waits for it.
#[test]
fn a_test_built_first_is_debugged_with_what_its_build_said() {
    let state = tempfile::tempdir().unwrap();
    let ok = "# test build_ok\n#! sleep 300\n#! out BUILT /project/target/fake-bin\n";
    let slow = "# test slow\n#! sleep 1500\n#! out PASS slow.py::slow 1ms\n";
    let (mut p, launches) = Project::debugging(&[("ok.py", ok), ("slow.py", slow)], "slow.py", state.path());
    p.scanned("slow.py::slow");
    p.open("ok.py");
    p.scanned("ok.py::build_ok");
    p.app.tests_request(&p.root.clone(), vec![Selection::File("slow.py".into())], "slow.py");
    p.until("the run", Project::step_running);
    p.app.tests_debug_here();
    let run = p.tests().debug.as_ref().expect("asked").run;
    p.until("the build waiting", |p| p.tests().debug.as_ref().is_some_and(|d| d.building));
    assert!(p.said().contains("waits for the run going"), "{}", p.said());
    p.app.tests_debug_here();
    assert_eq!(p.said(), "already building build_ok -- SPC T k lets it go");
    let launch = p.launched(&launches);
    assert_eq!(p.state("slow.py::slow"), State::Passed, "the run going went first");
    // As a string: a provider's `/project/...` comes with this machine's
    // separators throughout, as a path written here would.
    assert_eq!(launch["program"], serde_json::json!(p.root.join("target").join("fake-bin")), "{launch}");
    assert_eq!(launch["cwd"].as_str().map(Path::new), Some(p.root.as_path()), "/project is the project");
    assert_eq!(launch["args"], serde_json::json!(["ok.py::build_ok", "--exact"]));
    let builds = p.builds();
    assert_eq!(builds.len(), 1);
    assert!(builds[0].build_ok && builds[0].run == run, "as the launch's run: {builds:?}");
    let output = p.output();
    assert!(output.contains(&format!("-- a build to debug build_ok, run {} --", run.0)) && output.contains("--build ok.py::build_ok") && output.contains("-- built, in "), "{output}");
    assert!(p.app.debug_last_launch.is_empty());
    assert!(p.debug_done() && p.tests().plan.is_none());
    p.app.debug_stop();
}

/// A build that fails starts no session and says why, on the modeline
/// and the page's head: the first `broken`'s message, else the step's
/// end. A provider that asks to build twice is refused.
#[test]
fn a_build_that_fails_says_why_and_starts_nothing() {
    let state = tempfile::tempdir().unwrap();
    let bad = "# test build_bad\n#! out BROKEN bad.py bad.py didn't build: mismatched types\n#! exit 101\n";
    let exits = "# test build_exits\n#! exit 3\n";
    let twice = "# test build_twice\n";
    let (mut p, _launches) = Project::debugging(&[("bad.py", bad), ("exits.py", exits), ("twice.py", twice)], "bad.py", state.path());
    for (file, id) in [("exits.py", "exits.py::build_exits"), ("twice.py", "twice.py::build_twice"), ("bad.py", "bad.py::build_bad")] {
        p.open(file);
        p.scanned(id);
    }
    p.app.tests_debug_here();
    p.until("the build", |p| p.builds().len() == 1 && p.debug_done());
    assert_eq!(p.said(), "bad.py didn't build: mismatched types · o shows it");
    assert!(!p.builds()[0].build_ok && p.app.debug_session.is_none());
    assert!(p.output().contains("-- didn't build, in "), "{}", p.output());
    let notes = p.app.tests_page_notes(&p.root.clone());
    assert_eq!(notes.first().map(|n| n.0.as_str()), Some("bad.py didn't build: mismatched types · o shows it"), "the page's head");
    assert_eq!(p.state("bad.py::build_bad"), State::Unknown, "a build has no results");

    p.open("exits.py");
    p.app.tests_debug_here();
    p.until("the build", |p| p.builds().len() == 2 && p.debug_done());
    assert_eq!(p.said(), "build_exits didn't build: its build exited 3 · o shows it");

    p.open("twice.py");
    p.app.tests_debug_here();
    p.until("the second ask", |p| p.builds().len() == 3 && p.debug_done());
    assert_eq!(p.said(), "build_twice can't be debugged: provider asked to build it twice");
    assert!(p.app.debug_session.is_none());
}

/// A stop while a test's build runs kills its tree and starts nothing;
/// one while its build waits for a run lets it go with the run. Turned
/// off, nothing of it is left.
#[test]
fn a_stop_during_a_debugs_build_starts_nothing() {
    let state = tempfile::tempdir().unwrap();
    let slow = "# test build_slow\n#! child\n#! wait\n";
    let (mut p, _launches) = Project::debugging(&[("slow.py", slow)], "slow.py", state.path());
    p.scanned("slow.py::build_slow");
    p.app.tests_debug_here();
    let child = p.child();
    assert!(p.model().summary().running.is_some_and(|r| r.build), "the head says building to debug…");
    let notes = p.app.tests_page_notes(&p.root.clone());
    assert_eq!(notes.first().map(|n| n.0.as_str()), Some("building build_slow to debug it -- o shows it, x stops it"));
    p.app.run_cmd("tests.stop");
    p.until("the build's end", |p| p.builds().len() == 1);
    assert!(matches!(p.builds()[0].halted, Some(fenix_tests::Halt::Stopped(_))));
    assert_eq!(p.said(), "build_slow isn't debugged: its build was stopped (stopped)");
    assert!(p.debug_done() && p.app.debug_session.is_none());
    assert!(fenix_tasks::fake_runner::gone_within(child, Duration::from_secs(10)), "the child outlived its build");

    // The explorer turned off mid-build: stopped, then nothing.
    p.app.tests_debug_here();
    p.until("the second build", Project::step_running);
    let child = p.child();
    p.app.config.set_value("tests.enabled", Some(fenix_config::Value::Bool(false)));
    p.until("the build's end", |p| p.builds().len() == 2);
    assert!(p.debug_done() && p.app.debug_session.is_none());
    assert!(fenix_tasks::fake_runner::gone_within(child, Duration::from_secs(10)));
    p.app.tests_debug_here();
    assert_eq!(p.said(), "the test explorer is off (tests.enabled)");
    assert!(p.app.command_hidden("tests.debug_here"));
    assert!(!p.app.text_menu_items().iter().any(|i| i.label == "Debug test"));
}

/// An extension whose component has no `tests-launch` is said once on
/// its output page, and its tests aren't asked about again: they still
/// run. (The provider guest has every export, so the worker's answer is
/// given to the host as it would come.)
#[test]
fn a_provider_with_no_tests_launch_is_said_once_and_not_asked_again() {
    let state = tempfile::tempdir().unwrap();
    let (mut p, _launches) = Project::debugging(&[("a.py", "# test build_x\n#! out PASS a.py::build_x 1ms\n")], "a.py", state.path());
    p.scanned("a.py::build_x");
    p.app.tests_debug_here();
    let token = p.tests().debug.as_ref().and_then(|d| d.asking).expect("asked");
    let why = "it has no tests-launch: it was built for an extension API before 1.6, which had no test providers";
    p.app.tests_reply(EXT, token, Err(why.into()));
    assert!(p.debug_done());
    assert_eq!(p.said(), format!("build_x can't be debugged: {why}"));
    assert_eq!(p.app.extension_output_text().matches("its tests aren't offered to debug").count(), 1);
    let calls = p.app.tests.calls.len();
    p.app.tests_debug_here();
    assert_eq!(p.said(), "build_x can't be debugged: provider has no tests-launch (said on its output page)");
    assert_eq!(p.app.tests.calls.len(), calls, "not asked");
    assert!(p.has("a.py::build_x"), "its tests stay");
    p.app.tests_run_file();
    p.until("a run", |p| p.model().last_run().is_some());
    assert_eq!(p.state("a.py::build_x"), State::Passed);
    assert_eq!(p.app.extension_output_text().matches("its tests aren't offered to debug").count(), 1, "said once");
}

// Off is off, and leaves what isn't the explorer's (the review of steps
// 4 to 6, M1 and S1).

/// With the explorer off, what decides whether `SPC T` and its keys are
/// there is what's bound under it in the leader's trie: the explorer's
/// keys alone, it's off; a key of yours or an extension's under it keeps
/// the group, yours runs, and the explorer's keys there stay off (each
/// of them, and the group's count). Not one key per command: a key of
/// yours that runs a command bound elsewhere too (`SPC :`) counts.
#[test]
fn off_spc_t_is_decided_by_whats_bound_under_it_and_yours_stay() {
    let mut app = App::with_file(None);
    app.config.set_value("tests.enabled", Some(fenix_config::Value::Bool(false)));
    let (spc, t) = (KeyPress::char(' '), KeyPress::char('T'));
    let fenix: Vec<(Vec<KeyPress>, &&'static str)> = crate::keymap::leader_trie().bindings();
    assert!(app.leader_hidden_with(&fenix, &[spc], t), "the explorer's keys alone: off");
    let yours: &'static &'static str = Box::leak(Box::new("commands.palette"));
    let mut with_yours = fenix.clone();
    with_yours.push((vec![spc, t, KeyPress::char('z')], yours));
    for _ in 0..20 {
        assert!(!app.leader_hidden_with(&with_yours, &[spc], t), "yours keeps SPC T, every time");
    }
    assert!(!app.leader_hidden_with(&with_yours, &[spc, t], KeyPress::char('z')), "and runs");
    assert!(app.leader_hidden_with(&with_yours, &[spc, t], KeyPress::char('t')), "the explorer's stay off");
    let mut m = crate::keymap::leader_trie().matcher();
    m.feed(spc);
    let mut hints = m.pending_hints();
    let explorer = hints.iter().find(|h| h.key == t).expect("SPC T in Fenix's leader").count;
    assert_eq!(explorer, super::COMMANDS.len());
    hints.iter_mut().filter(|h| h.key == t).for_each(|h| h.count += 1);
    let offered = app.leader_offered_with(&with_yours, &[spc], hints);
    assert_eq!(offered.iter().find(|h| h.key == t).map(|h| h.count), Some(1), "the group counts yours alone");
    let hint = |c: char| fenix_keymap::Hint { key: KeyPress::char(c), label: "x", group: false, count: 0, section: None };
    let under = vec![hint('t'), hint('z')];
    let offered: Vec<KeyPress> = app.leader_offered_with(&with_yours, &[spc, t], under).into_iter().map(|h| h.key).collect();
    assert_eq!(offered, [KeyPress::char('z')], "which-key under SPC T shows yours alone");
    // On: nothing is hidden.
    app.config.set_value("tests.enabled", Some(fenix_config::Value::Bool(true)));
    assert!(!app.leader_hidden_with(&fenix, &[spc], t));
}

/// Off, the explorer is in none of the places that offer commands or
/// keys: the keys' page, `SPC h k`, `fenix/commands` (and, as before, the
/// palette, `SPC h b`, which-key). Only its own commands are: another
/// `tests.*` is someone else's.
#[test]
fn off_the_explorer_is_offered_nowhere() {
    let mut app = App::with_file(None);
    app.config.set_value("tests.enabled", Some(fenix_config::Value::Bool(false)));
    let sheet = app.keys_sheet_text();
    assert!(!sheet.contains("`SPC T`") && !sheet.contains("| tests ("), "the keys' page:\n{sheet}");
    app.describe_key_start();
    for key in ["SPC", "T"] {
        app.press(key);
    }
    let said = app.status_message.as_ref().map(|m| m.text.clone()).unwrap_or_default();
    assert_eq!(said, "SPC T isn't bound", "SPC h k");
    let listed = crate::app::control_host::control_commands(&app);
    assert!(!listed.commands.iter().any(|c| c.id.starts_with("tests.")), "fenix/commands");
    assert!(!app.command_hidden("tests.coverage"), "an extension's own tests.* isn't the explorer's");
    for id in super::COMMANDS {
        assert!(app.command_hidden(id));
        assert!(app.commands().get(id).is_some(), "{id} is a command of Fenix's");
    }
    let registered: Vec<String> = app.commands().iter().map(|c| c.id.to_string()).filter(|id| id.starts_with("tests.")).collect();
    assert_eq!(registered.len(), super::COMMANDS.len(), "every one of the explorer's commands is in COMMANDS: {registered:?}");

    app.config.set_value("tests.enabled", Some(fenix_config::Value::Bool(true)));
    assert!(app.keys_sheet_text().contains("tests ("));
    assert!(crate::app::control_host::control_commands(&app).commands.iter().any(|c| c.id == "tests.debug_here"));
}

/// A click runs a test only on its mark's own cell: one left of the
/// text's padding -- on the Git bar, where a breakpoint's square is --
/// puts the cursor there as before (the review of steps 4 to 6, S5).
#[test]
fn only_a_click_on_the_marks_own_cell_runs_its_test() {
    let (mut app, root, _dir) = served(12, &[("a.fake::bad", 4)]);
    let asked = |app: &App| app.tests.projects[&root].model.summary().running.is_some() || app.tests.projects[&root].model.last_run().is_some() || !app.tests.projects[&root].ended_runs.is_empty();
    let (sidebar_px, terminal_h, modeline_top) = app.frame_metrics(600.0);
    let geometry = app.frame_geometry(800.0, sidebar_px, terminal_h, modeline_top);
    let pane = app.focused_pane_id();
    let rect = geometry.panes.iter().find(|(id, _)| *id == pane).unwrap().1;
    let line_height = app.text.as_ref().map(|t| t.line_height()).unwrap_or(text::LINE_HEIGHT);
    let char_width = app.text.as_ref().map(|t| t.char_width()).unwrap_or(text::CHAR_WIDTH);
    let content = pane_content_rect(rect, line_height, app.pane_has_breadcrumb(pane));
    let y = content.y + text::PAD_TOP + line_height * 4.5;
    app.handle_click_at((content.x + 2.0, y), 800.0, 600.0);
    assert!(!asked(&app), "the Git bar's place runs nothing");
    assert_eq!(app.open().buffer.line_col(&app.cursor()).0, 4, "the cursor goes there, as before");
    app.handle_click_at((content.x + text::PAD_LEFT + char_width * 0.5, y), 800.0, 600.0);
    assert!(asked(&app), "the mark's cell runs it");
}
