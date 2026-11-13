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
    let found = api_found(model_found(FoundTest { id: "t".into(), parent: None, label: "t".into(), kind: TestKind::Test, path: Some(full.to_string_lossy().into_owned()), line: Some(3), end_line: None, skip: None, template: false, data: Some("d".into()) }, root, root));
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
        "[extension]\nid = \"{id}\"\nname = \"Provider\"\nversion = \"1.0.0\"\nruntime = \"wasm\"\n\n[engine]\napi = \"1.6\"\nfenix = \">=1.5, <2\"\n\n[permissions]\nprocess = [\"fake-test-runner\"]\nfilesystem = \"project-read\"\n\n[[contributes.test_providers]]\nid = \"{short}\"\nname = \"fake\"\nmarkers = [\"fake.toml\"]\nscan = [\"*.fake\"]\n"
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
