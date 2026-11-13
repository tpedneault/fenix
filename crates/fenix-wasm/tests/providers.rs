//! Test providers (API 1.6) against real guests: `guests/provider.wasm`,
//! a provider of a made-up runner through every export and the import;
//! `guests/plain16.wasm`, built on the 1.6 crate with no provider; and
//! `guests/keys.wasm`, built on the 1.5 crate, before there were any.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use fenix_extensions::manifest::Permissions;
use fenix_wasm::host::{
    Call, Event, FoundTest, Launching, Level, Reply, Request, Source, Spec, TestAsk, TestKind, TestLine, TestLocation, TestOutcome, TestReport, TestStepEnd, Worker,
};
use fenix_wasm::test_limits::{MAX_ERROR, MAX_ID, MAX_LABEL, MAX_PATH, MAX_PROBLEM, MAX_REPORTS, MAX_RUNNER, MAX_SCANNED, MAX_SKIP, MAX_TEXT};
use fenix_wasm::{engine, Options, Ticker};

fn guest(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/guests").join(name)
}

struct Running {
    worker: Worker,
    events: mpsc::Receiver<Event>,
    dir: tempfile::TempDir,
    _ticker: Ticker,
}

/// `wasm` started as `test.provider`, its providers `provider` and
/// `provider.other`, allowed to run `fake-test-runner`; and what it said
/// as it started.
fn start(wasm: &str) -> (Running, Vec<Event>) {
    let engine = engine(&Options::default()).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut spec = Spec::new("test.provider", guest(wasm), dir.path());
    spec.permissions = Permissions { process: vec!["fake-test-runner".into()], ..Permissions::default() };
    spec.test_providers = vec!["provider".into(), "provider.other".into()];
    let (tx, events) = mpsc::channel();
    let worker = Worker::start(&engine, spec, move |e| {
        let _ = tx.send(e);
    });
    let running = Running { worker, events, dir, _ticker: Ticker::start(&engine) };
    let seen = running.until(|e| matches!(e, Event::Ready | Event::Stopped(_)));
    (running, seen)
}

impl Running {
    fn until(&self, done: impl Fn(&Event) -> bool) -> Vec<Event> {
        let mut seen = Vec::new();
        loop {
            // Generous: a loaded CI runner compiles and calls an extension slowly.
            let e = self.events.recv_timeout(Duration::from_secs(120)).expect("an event");
            let last = done(&e);
            seen.push(e);
            if last {
                return seen;
            }
        }
    }

    fn call_in(&mut self, what: Call, ticks: u64) -> (Result<Reply, String>, Vec<Event>) {
        let call = self.worker.call(what, ticks);
        let seen = self.until(|e| matches!(e, Event::Finished { call: c, .. } if *c == call));
        let Some(Event::Finished { result, .. }) = seen.last().cloned() else { unreachable!() };
        (result, seen)
    }

    fn call(&mut self, what: Call) -> (Result<Reply, String>, Vec<Event>) {
        self.call_in(what, 0)
    }

    fn data(&self) -> PathBuf {
        self.dir.path().join("data")
    }
}

fn warnings(seen: &[Event]) -> Vec<String> {
    seen.iter().filter_map(|e| if let Event::Said(Source::Log(Level::Warn), s) = e { Some(s.clone()) } else { None }).collect()
}

fn said(seen: &[Event]) -> Vec<String> {
    seen.iter().filter_map(|e| if let Event::Said(_, s) = e { Some(s.clone()) } else { None }).collect()
}

fn scan(provider: &str, path: &str, text: &str) -> Call {
    Call::TestsScan { provider: provider.into(), path: path.into(), text: text.into() }
}

fn scanned(reply: Result<Reply, String>) -> Vec<FoundTest> {
    match reply {
        Ok(Reply::Scanned(found)) => found,
        other => panic!("{other:?}"),
    }
}

fn reports(reply: Result<Reply, String>) -> Vec<TestReport> {
    match reply {
        Ok(Reply::Reports(reports)) => reports,
        other => panic!("{other:?}"),
    }
}

fn line(text: &str) -> TestLine {
    TestLine { text: text.into(), stderr: false, cut: false }
}

fn test(id: &str, kind: TestKind, data: Option<&str>) -> FoundTest {
    FoundTest { id: id.into(), parent: None, label: id.rsplit("::").next().unwrap_or(id).into(), kind, path: None, line: None, end_line: None, skip: None, template: false, data: data.map(Into::into) }
}

fn path(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// A run through every export: a file scanned, a listing and a run
/// planned (held to its permissions, `${data}` filled in, its needs
/// found), output read in batches with a cut line joined, each step's
/// end, and a test's launch -- now, or after a build.
#[test]
fn a_provider_scans_plans_reads_and_launches() {
    let (mut p, seen) = start("provider.wasm");
    assert!(warnings(&seen).is_empty(), "{seen:?}");

    let text = "fn x() {}\n# suite geo\n# test adds\n# skip slow it's slow\n# each cases\n";
    let found = scanned(p.call(scan("provider", "/project/tests/a.fake", text)).0);
    let ids: Vec<(&str, Option<&str>, TestKind, Option<u32>)> = found.iter().map(|f| (f.id.as_str(), f.parent.as_deref(), f.kind, f.line)).collect();
    assert_eq!(
        ids,
        [
            ("tests/a.fake", None, TestKind::File, None),
            ("tests/a.fake::geo", Some("tests/a.fake"), TestKind::Suite, Some(1)),
            ("tests/a.fake::adds", Some("tests/a.fake::geo"), TestKind::Test, Some(2)),
            ("tests/a.fake::slow", Some("tests/a.fake::geo"), TestKind::Test, Some(3)),
            ("tests/a.fake::cases", Some("tests/a.fake::geo"), TestKind::Test, Some(4)),
        ]
    );
    assert_eq!((found[0].path.as_deref(), found[0].label.as_str()), (Some("/project/tests/a.fake"), "a.fake"));
    assert_eq!(found[3].skip.as_deref(), Some("it's slow"));
    assert!(found[4].template && found[4].data.as_deref() == Some("{\"template\":\"cases\"}"));

    // A listing: one step, in /data.
    let Ok(Reply::Planned(Ok(listing))) = p.call(Call::TestsPlan { provider: "provider".into(), run: 1, ask: TestAsk::Discover }).0 else { panic!() };
    assert_eq!(listing.steps.len(), 1);
    let ((program, args, cwd), _) = listing.steps[0].command.clone().unwrap();
    assert_eq!((program.as_str(), args, cwd), ("fake-test-runner", vec!["--list".to_string()], Some(p.data())));

    // All: two steps, the first's `${data}` filled in and its reporter
    // found; the second needs one that isn't there and one outside `/data`.
    let Ok(Reply::Planned(Ok(all))) = p.call(Call::TestsPlan { provider: "provider".into(), run: 2, ask: TestAsk::RunAll }).0 else { panic!() };
    assert!(all.list_after);
    let ((_, args, _), env) = all.steps[0].command.clone().unwrap();
    assert_eq!(args, ["--unit".to_string(), format!("{}/ids.txt", path(&p.data()))]);
    assert_eq!(env, [("FAKE".to_string(), format!("{}/x", path(&p.data())))]);
    assert_eq!((all.steps[0].step.label.as_str(), all.steps[0].step.covers.as_slice()), ("unit", &["unit".to_string()][..]));
    assert_eq!(all.steps[0].needs, [Ok(p.data().join("reporter.txt"))]);
    assert!(all.steps[0].needs[0].as_ref().unwrap().is_file());
    assert_eq!(all.steps[1].needs.len(), 2);
    assert!(all.steps[1].needs[0].as_ref().is_ok_and(|n| !n.exists()), "{:?}", all.steps[1].needs);
    assert_eq!(all.steps[1].needs[1], Err("needs elsewhere/x, which isn't a file's path under /data".into()));

    // A run of a file and what's under it, and a test of another file: its
    // data given back as it was.
    let ask = TestAsk::Run(vec![test("tests/a.fake", TestKind::File, None), FoundTest { parent: Some("tests/a.fake".into()), ..test("tests/a.fake::adds", TestKind::Test, Some("{\"n\":1}")) }, test("b.fake::x", TestKind::Test, None)]);
    let (planned, seen) = p.call(Call::TestsPlan { provider: "provider".into(), run: 7, ask });
    assert!(said(&seen).contains(&"run 7 asked 3: tests/a.fake, tests/a.fake::adds={\"n\":1}, b.fake::x".to_string()), "{seen:?}");
    let Ok(Reply::Planned(Ok(run))) = planned else { panic!() };
    let args: Vec<Vec<String>> = run.steps.iter().map(|s| s.command.clone().unwrap().0 .1).collect();
    assert_eq!(args, [vec!["tests/a.fake".to_string(), "tests/a.fake::adds".into()], vec!["b.fake::x".into()]]);
    assert_eq!((run.steps[0].step.covers.as_slice(), run.steps[1].step.covers.is_empty()), (&["tests/a.fake".to_string()][..], true));

    // Its output in two batches, a line cut at 64 KiB in the first.
    let batch = vec![
        line("START tests/a.fake::adds"),
        line("PASS tests/a.fake::adds 3ms"),
        TestLine { text: "FAIL tests/a.fake::slow sr".into(), stderr: false, cut: true },
        line("c/a.rs:4 two and two"),
        line("OUT building"),
    ];
    let first = reports(p.call(Call::TestsRead { provider: "provider".into(), run: 7, step: 0, lines: batch }).0);
    assert_eq!(first.len(), 4);
    assert!(matches!(&first[0], TestReport::Started(id) if id == "tests/a.fake::adds"));
    assert!(matches!(&first[1], TestReport::Ended(e) if e.outcome == TestOutcome::Passed && e.duration_ms == Some(3.0)));
    let TestReport::Ended(failed) = &first[2] else { panic!("{first:?}") };
    assert_eq!((failed.outcome, failed.message.as_deref(), failed.at.clone()), (TestOutcome::Failed, Some("two and two"), Some(TestLocation { path: "src/a.rs".into(), line: 4, column: None })));
    assert!(matches!(&first[3], TestReport::Output(o) if o.id.is_none() && o.text == "building"));
    let second = reports(p.call(Call::TestsRead { provider: "provider".into(), run: 7, step: 0, lines: vec![line("BROKEN orbit/lib mismatched types"), line("CRASH orbit/lib SIGABRT"), line("FOUND tests/a.fake::cases/neg tests/a.fake::cases")] }).0);
    assert!(matches!(&second[0], TestReport::Broken(b) if b.id.as_deref() == Some("orbit/lib") && b.message == "mismatched types"));
    assert!(matches!(&second[1], TestReport::Crashed(c) if c.id.as_deref() == Some("orbit/lib") && c.message == "SIGABRT"));
    assert!(matches!(&second[2], TestReport::Found(f) if f.kind == TestKind::Case && f.parent.as_deref() == Some("tests/a.fake::cases")));

    // Each step's end: the first after its 8 lines, a Windows crash's code
    // as it is; the second never started.
    let end = reports(p.call(Call::TestsStepEnded { provider: "provider".into(), run: 7, step: 0, end: TestStepEnd::Exited(Some(-1_073_741_819)) }).0);
    assert!(matches!(&end[..], [TestReport::Output(o)] if o.text == "step 0 of run 7 ended Exited(Some(-1073741819)) after 8 lines"), "{end:?}");
    let end = reports(p.call(Call::TestsStepEnded { provider: "provider".into(), run: 7, step: 1, end: TestStepEnd::Stopped }).0);
    assert!(matches!(&end[..], [TestReport::Output(o)] if o.text == "step 1 of run 7 ended Stopped after 0 lines"), "{end:?}");
    let end = reports(p.call(Call::TestsStepEnded { provider: "provider".into(), run: 2, step: 0, end: TestStepEnd::Failed("needs /data/missing.txt, which isn't there".into()) }).0);
    assert!(matches!(&end[..], [TestReport::Output(o)] if o.text.contains("Failed(\"needs /data/missing.txt")), "{end:?}");

    // Debugging a test: a launch now, its paths and `${data}` as Fenix's;
    // a case after a build; one it can't.
    let launch = |p: &mut Running, test: FoundTest| match p.call(Call::TestsLaunch { provider: "provider".into(), run: 9, test }).0 {
        Ok(Reply::Launched(launched)) => launched,
        other => panic!("{other:?}"),
    };
    let Ok(Launching::Launch(config)) = launch(&mut p, test("tests/a.fake::adds", TestKind::Test, Some("{\"n\":1}"))) else { panic!() };
    assert_eq!((config.name.as_str(), config.program.clone(), config.args.clone()), ("adds", Some(path(&p.data().join("fake-debugger"))), vec!["tests/a.fake::adds".to_string(), "run 9".into()]));
    assert_eq!((config.env.clone(), config.attributes.as_str()), (vec![("AT".to_string(), path(&p.data()))], r#"{"data":"{\"n\":1}"}"#));
    let Ok(Launching::BuildFirst(build)) = launch(&mut p, test("tests/a.fake::cases/neg", TestKind::Case, None)) else { panic!() };
    assert_eq!(build.steps[0].command.clone().unwrap().0 .1, ["--build", "tests/a.fake::cases/neg"]);
    assert_eq!(launch(&mut p, test("x", TestKind::Test, None)), Err("x can't be debugged".into()));
}

/// `test-provider-set`: for its own providers only, held to its limits.
#[test]
fn the_import_says_what_a_provider_is_and_refuses_another() {
    let (mut p, _) = start("provider.wasm");
    let asked = |seen: &[Event]| seen.iter().find_map(|e| if let Event::Asked(r) = e { Some(r.clone()) } else { None });
    let (ran, seen) = p.call(Call::RunCommand("provider.set".into()));
    assert_eq!(ran, Ok(Reply::Done));
    assert_eq!(asked(&seen), Some(Request::TestProviderSet { provider: "provider".into(), runner: "fake-test-runner 2.0".into(), problem: None }));
    assert!(said(&seen).contains(&"provider.set: Ok(())".to_string()));
    let (_, seen) = p.call(Call::RunCommand("provider.hide".into()));
    assert_eq!(asked(&seen), Some(Request::TestProviderSet { provider: "provider".into(), runner: String::new(), problem: None }), "nothing to run here");
    let (_, seen) = p.call(Call::RunCommand("provider.problem".into()));
    assert_eq!(asked(&seen), Some(Request::TestProviderSet { provider: "provider.other".into(), runner: "fake-test-runner".into(), problem: Some("it isn't installed".into()) }));
    let (_, seen) = p.call(Call::RunCommand("provider.unknown".into()));
    assert_eq!(asked(&seen), None);
    assert!(said(&seen).iter().any(|s| s.contains("\\\"python\\\" isn't one of its test providers")), "{seen:?}");
    let (_, seen) = p.call(Call::RunCommand("provider.long".into()));
    let Some(Request::TestProviderSet { runner, problem, .. }) = asked(&seen) else { panic!() };
    assert_eq!((runner.chars().count(), problem.map(|p| p.len())), (MAX_RUNNER, Some(MAX_PROBLEM)));
    assert_eq!(warnings(&seen).len(), 2, "{seen:?}");
    let (_, seen) = p.call(Call::RunCommand("provider.long".into()));
    assert!(warnings(&seen).is_empty(), "said once: {seen:?}");
}

/// What a provider answers is held to the limits as it comes in: at
/// them, it's kept as it was; over them, cut or left out, and said once.
#[test]
fn answers_are_held_to_the_limits_at_the_boundary() {
    let (mut p, _) = start("provider.wasm");
    let at = scanned(p.call(scan("provider", "/project/a.fake", "#limits")).0);
    let f = &at[1];
    assert_eq!((f.id.len(), f.parent.as_ref().map(String::len), f.label.chars().count(), f.path.as_ref().map(String::len), f.skip.as_ref().map(String::len)), (MAX_ID, Some(MAX_ID), MAX_LABEL, Some(MAX_PATH), Some(MAX_SKIP)));
    assert_eq!((f.data.as_ref().map(String::len), f.line, f.end_line, f.template, f.kind), (Some(4096), Some(u32::MAX), Some(u32::MAX), true, TestKind::Case), "whole, through the boundary");

    let (over, seen) = p.call(scan("provider", "/project/a.fake", "#over"));
    let f = &scanned(over)[1];
    assert_eq!((f.id.len(), f.parent.as_ref().map(String::len), f.label.chars().count(), f.path.as_ref().map(String::len), f.skip.as_ref().map(String::len)), (MAX_ID, Some(MAX_ID), MAX_LABEL, Some(MAX_PATH), Some(MAX_SKIP)));
    assert_eq!(f.data, None, "data over 4 KiB isn't given back cut");
    assert_eq!(warnings(&seen).len(), 5, "{seen:?}");
    assert!(warnings(&p.call(scan("provider", "/project/a.fake", "#over")).1).is_empty(), "each said once");

    assert_eq!(scanned(p.call(scan("provider", "/project/a.fake", "#many")).0).len(), MAX_SCANNED);
    let many = reports(p.call(Call::TestsRead { provider: "provider".into(), run: 1, step: 0, lines: vec![line("MANY 10005")] }).0);
    assert_eq!(many.len(), MAX_REPORTS);
    let huge = reports(p.call(Call::TestsRead { provider: "provider".into(), run: 1, step: 0, lines: vec![line("HUGE")] }).0);
    assert!(matches!(&huge[..], [TestReport::Output(o)] if o.text.len() == MAX_TEXT && !o.text.starts_with("start")), "its end is kept");

    let plan = |p: &mut Running, provider: &str| match p.call(Call::TestsPlan { provider: provider.into(), run: 3, ask: TestAsk::RunAll }).0 {
        Ok(Reply::Planned(planned)) => planned,
        other => panic!("{other:?}"),
    };
    assert_eq!(plan(&mut p, "noplan").unwrap_err().len(), MAX_ERROR);
    let denied = plan(&mut p, "denied").unwrap();
    assert!(denied.steps[0].command.as_ref().unwrap_err().contains("rm isn't a program this extension may run (permissions.process)"));
    let big = match p.call(Call::TestsLaunch { provider: "biglaunch".into(), run: 3, test: test("x", TestKind::Test, None) }).0 {
        Ok(Reply::Launched(launched)) => launched,
        other => panic!("{other:?}"),
    };
    assert!(big.unwrap_err().contains("a test's launch is at most 1 MiB"));
}

/// A provider that traps in any of its exports stops, as after any trap:
/// that call ends with why, and the next is refused.
#[test]
fn a_trap_in_each_export_stops_the_provider() {
    let calls = [
        scan("trap", "/project/a.fake", ""),
        Call::TestsPlan { provider: "trap".into(), run: 1, ask: TestAsk::Discover },
        Call::TestsRead { provider: "trap".into(), run: 1, step: 0, lines: vec![line("x")] },
        Call::TestsStepEnded { provider: "trap".into(), run: 1, step: 0, end: TestStepEnd::TimedOut },
        Call::TestsLaunch { provider: "trap".into(), run: 1, test: test("x", TestKind::Test, None) },
    ];
    for call in calls {
        let (mut p, _) = start("provider.wasm");
        let (result, seen) = p.call(call.clone());
        assert_eq!(result, Err("it crashed (a panic, or unreachable code)".into()), "{call:?}");
        let more = p.until(|e| matches!(e, Event::Stopped(_)));
        assert!(seen.iter().chain(&more).any(|e| matches!(e, Event::Stopped(why) if why.contains("test.provider stopped"))));
        assert!(p.call(scan("provider", "/project/a.fake", "")).0.unwrap_err().contains("stopped"));
    }
}

/// A call that runs past its deadline is stopped there.
#[test]
fn a_provider_past_its_deadline_is_stopped() {
    let (mut p, _) = start("provider.wasm");
    let (result, _) = p.call_in(Call::TestsRead { provider: "spin".into(), run: 1, step: 0, lines: Vec::new() }, 20);
    assert_eq!(result, Err("it took too long and was stopped".into()));
}

/// Built on the 1.5 crate, before there were providers: it loads as it
/// did, a manifest naming providers is told it has none, and each call
/// says so.
#[test]
fn a_guest_built_before_1_6_has_no_provider_and_says_so() {
    let (mut p, seen) = start("keys.wasm");
    assert!(seen.contains(&Event::Ready));
    assert!(warnings(&seen).iter().any(|w| w.contains("names test providers (provider, provider.other), but it has no tests-scan and tests-plan")), "{seen:?}");
    let calls = [
        scan("provider", "/project/a.fake", ""),
        Call::TestsPlan { provider: "provider".into(), run: 1, ask: TestAsk::RunAll },
        Call::TestsRead { provider: "provider".into(), run: 1, step: 0, lines: Vec::new() },
        Call::TestsStepEnded { provider: "provider".into(), run: 1, step: 0, end: TestStepEnd::Exited(None) },
        Call::TestsLaunch { provider: "provider".into(), run: 1, test: test("x", TestKind::Test, None) },
    ];
    for (call, export) in calls.into_iter().zip(["tests-scan", "tests-plan", "tests-read", "tests-step-ended", "tests-launch"]) {
        let err = p.call(call).0.unwrap_err();
        assert!(err.starts_with(&format!("it has no {export}: it was built for an extension API before 1.6")), "{err}");
    }
    assert_eq!(p.call(Call::RunCommand("keys.say".into())).0, Ok(Reply::Done), "and runs as it did");
}

/// Rebuilt on the 1.6 crate with no provider: it has the exports, and
/// they answer the crate's defaults.
#[test]
fn a_guest_rebuilt_on_1_6_without_a_provider_answers_the_defaults() {
    let (mut p, seen) = start("plain16.wasm");
    assert!(seen.contains(&Event::Ready) && warnings(&seen).is_empty(), "{seen:?}");
    assert_eq!(scanned(p.call(scan("provider", "/project/a.fake", "# test x")).0), []);
    assert_eq!(p.call(Call::TestsPlan { provider: "provider".into(), run: 1, ask: TestAsk::RunAll }).0, Ok(Reply::Planned(Err("no tests here".into()))));
    assert_eq!(reports(p.call(Call::TestsRead { provider: "provider".into(), run: 1, step: 0, lines: vec![line("PASS x")] }).0), []);
    assert_eq!(reports(p.call(Call::TestsStepEnded { provider: "provider".into(), run: 1, step: 0, end: TestStepEnd::Stopped }).0), []);
    assert_eq!(p.call(Call::TestsLaunch { provider: "provider".into(), run: 1, test: test("x", TestKind::Test, None) }).0, Ok(Reply::Launched(Err("no debugging for tests here".into()))));
    assert_eq!(p.call(Call::RunCommand("plain.say".into())).0, Ok(Reply::Done));
}
