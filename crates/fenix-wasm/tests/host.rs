//! The host against a real extension: `guests/hello.wasm`, built from
//! `guests/hello` (`guests/build.ps1`).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{mpsc, Arc, RwLock};
use std::time::Duration;

use fenix_extensions::manifest::Permissions;
use fenix_wasm::host::{
    load, Call, CellStyle, Changed, DecorationLook, Event, Focus, FocusedPage, Key, KeyMods, KeyPress, Level, Line, MouseAction, MouseButton, MouseEvent, MousePress, NamedKey, Notice, Reply, Request, Shade, Source, Span, Spec,
    SurfaceKey, VerticalRule, Worker,
};
use fenix_wasm::{engine, Options, Ticker};

fn hello() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/guests/hello.wasm")
}

struct Running {
    worker: Worker,
    events: mpsc::Receiver<Event>,
    dir: tempfile::TempDir,
    _ticker: Ticker,
}

/// The test extension, started with `permissions` and `settings`; it may
/// read the secret `hello.token`, which is "s3cret".
fn start_with(permissions: Permissions, settings: &[(&str, &str)]) -> Running {
    start_tuned(permissions, settings, |_| {})
}

/// `start_with`, and `tune` has the last say on its spec.
fn start_tuned(permissions: Permissions, settings: &[(&str, &str)], tune: impl FnOnce(&mut Spec)) -> Running {
    let engine = engine(&Options::default()).unwrap();
    let ticker = Ticker::start(&engine);
    let dir = tempfile::tempdir().unwrap();
    let mut spec = Spec::new("test.hello", hello(), dir.path());
    spec.permissions = permissions;
    spec.settings = Arc::new(RwLock::new(settings.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect::<HashMap<_, _>>()));
    spec.secrets = Arc::new(|key| (key == "hello.token" || key == "other.token").then(|| "s3cret".to_string()));
    tune(&mut spec);
    let (tx, events) = mpsc::channel();
    let worker = Worker::start(&engine, spec, move |e| {
        let _ = tx.send(e);
    });
    let running = Running { worker, events, dir, _ticker: ticker };
    running.until(|e| *e == Event::Ready);
    running
}

fn start() -> Running {
    start_with(Permissions::default(), &[])
}

impl Running {
    fn next(&self) -> Event {
        // Generous: a loaded CI runner compiles and calls an extension slowly.
        self.events.recv_timeout(Duration::from_secs(120)).expect("an event")
    }

    /// Events up to and including the first `done` says is the one.
    fn until(&self, done: impl Fn(&Event) -> bool) -> Vec<Event> {
        let mut seen = Vec::new();
        loop {
            let e = self.next();
            let last = done(&e);
            seen.push(e);
            if last {
                return seen;
            }
        }
    }

    fn call(&mut self, what: Call) -> (Result<Reply, String>, Vec<Event>) {
        let call = self.worker.call(what, 0);
        let seen = self.until(|e| matches!(e, Event::Finished { call: c, .. } if *c == call));
        let Some(Event::Finished { result, .. }) = seen.last().cloned() else { unreachable!() };
        (result, seen)
    }

    fn run(&mut self, command: &str) -> (Result<(), String>, Vec<Event>) {
        let (result, seen) = self.call(Call::RunCommand(command.into()));
        (result.map(|r| assert_eq!(r, Reply::Done)), seen)
    }

    /// What it logged, up to and including a line `done` says is the one.
    fn logged_until(&self, done: impl Fn(&str) -> bool) -> Vec<String> {
        self.until(|e| matches!(e, Event::Said(_, s) if done(s))).into_iter().filter_map(|e| if let Event::Said(_, s) = e { Some(s) } else { None }).collect()
    }
}

fn said(seen: &[Event], level: Level, text: &str) -> bool {
    seen.iter().any(|e| matches!(e, Event::Said(Source::Log(l), s) if *l == level && s == text))
}

/// A page as it was laid out (`page-render`).
fn laid_out(reply: Result<Reply, String>) -> fenix_wasm::host::Page {
    match reply {
        Ok(Reply::Page(page)) => *page,
        other => panic!("{other:?}"),
    }
}

fn warned(seen: &[Event], part: &str) -> bool {
    seen.iter().any(|e| matches!(e, Event::Said(Source::Log(Level::Warn), s) if s.contains(part)))
}

#[test]
fn it_activates_and_what_it_says_and_asks_comes_back() {
    let engine = engine(&Options::default()).unwrap();
    let _ticker = Ticker::start(&engine);
    let dir = tempfile::tempdir().unwrap();
    let (tx, rx) = mpsc::channel();
    let _worker = Worker::start(&engine, Spec::new("test.hello", hello(), dir.path()), move |e| {
        let _ = tx.send(e);
    });
    let mut first: Vec<Event> = (0..3).map(|_| rx.recv_timeout(Duration::from_secs(120)).unwrap()).collect();
    first.sort_by_key(|e| format!("{e:?}"));
    assert!(first.contains(&Event::Ready), "{first:?}");
    assert!(first.contains(&Event::Said(Source::Log(Level::Info), "hello is up".into())), "{first:?}");
    assert!(first.contains(&Event::Said(Source::Stdout, "and printing to stdout".into())), "{first:?}");

    let mut ext = start();
    let (result, seen) = ext.run("hello.greet");
    assert_eq!(result, Ok(()));
    assert!(said(&seen, Level::Info, "hello, Fenix"), "{seen:?}");
    assert!(seen.contains(&Event::Asked(Request::Notify(Level::Info, "Hello says hello".into()))), "{seen:?}");
    // An error it returns is an answer, not a crash: it keeps running.
    assert_eq!(ext.run("hello.fail").0, Err("asked to fail".into()));
    assert_eq!(ext.run("nope").0, Err("no command nope".into()));
    assert_eq!(ext.run("hello.greet").0, Ok(()));
}

/// A trap stops the extension, says why, and refuses what follows.
/// Returns how long the call took, from after the extension was up.
fn stops_on(command: &str, why: &str) -> Duration {
    let mut ext = start();
    let started = std::time::Instant::now();
    let (result, seen) = ext.run(command);
    let took = started.elapsed();
    let err = result.unwrap_err();
    assert!(err.contains(why), "{command}: {err}");
    let stopped = ext.next();
    assert!(matches!(&stopped, Event::Stopped(s) if s.starts_with("test.hello stopped: ") && s.contains(why)), "{stopped:?} after {seen:?}");
    let (after, _) = ext.run("hello.greet");
    assert!(after.unwrap_err().contains("test.hello stopped"), "later calls are refused");
    took
}

/// Timed from after it's up: compiling it on a busy CI runner is slow,
/// and isn't what's measured.
#[test]
fn a_call_that_runs_forever_is_stopped_at_its_deadline() {
    let took = stops_on("hello.spin", "took too long");
    assert!(took < Duration::from_secs(20), "{took:?}");
}

#[test]
fn a_panic_stops_it_and_says_so() {
    stops_on("hello.panic", "crashed");
}

#[test]
fn endless_recursion_stops_it() {
    stops_on("hello.recurse", "stack");
}

#[test]
fn memory_past_its_limit_stops_it() {
    stops_on("hello.hog", "ran out of memory");
}

/// Nothing's granted but its own folder: no files of the machine's, no
/// environment; `/data` is its, and kept.
#[test]
fn it_has_its_own_folder_and_nothing_else() {
    let mut ext = start();
    let (result, seen) = ext.run("hello.read-file");
    assert_eq!(result, Ok(()), "{seen:?}");
    assert!(warned(&seen, "no file"), "{seen:?}");
    let (_, seen) = ext.run("hello.env");
    assert!(said(&seen, Level::Info, "0 environment variables"), "{seen:?}");
    let (_, seen) = ext.run("hello.data");
    assert!(said(&seen, Level::Info, "note: x"), "{seen:?}");
    let (_, seen) = ext.run("hello.data");
    assert!(said(&seen, Level::Info, "note: xx"), "{seen:?}");
    assert_eq!(std::fs::read_to_string(ext.dir.path().join("data").join("note.txt")).unwrap(), "xx", "on disk, in its own folder");
}

#[test]
fn a_timer_goes_off_as_an_event() {
    let mut ext = start();
    assert_eq!(ext.run("hello.timer").0, Ok(()));
    let lines = ext.logged_until(|s| s.ends_with("went off"));
    assert_eq!(lines.last().map(String::as_str), Some("timer 1 went off"), "{lines:?}");
}

/// A program it's allowed runs, its output and end coming as events;
/// another, or one given as a path, doesn't.
#[test]
fn programs_run_only_as_its_manifest_allows() {
    let mut ext = start_with(Permissions { process: vec!["cmd".into()], ..Permissions::default() }, &[]);
    let (result, _) = ext.run("hello.spawn");
    assert_eq!(result, Ok(()));
    let lines = ext.logged_until(|s| s.starts_with("process 1 ended"));
    assert!(lines.contains(&"process 1 said: hi from cmd".to_string()), "{lines:?}");
    assert_eq!(lines.last().map(String::as_str), Some("process 1 ended: Some(0)"));
    let (_, seen) = ext.run("hello.spawn-denied");
    assert!(warned(&seen, "powershell isn't a program this extension may run"), "{seen:?}");
    let (_, seen) = ext.run("hello.spawn-path");
    assert!(warned(&seen, "a path runs only when one of its settings holds it"), "{seen:?}");
}

/// A path runs when a setting it lists holds it, or when it's under its
/// own folder where its manifest says.
#[test]
fn programs_by_path_run_only_where_the_manifest_says() {
    let cmd = std::env::var("ComSpec").unwrap_or_else(|_| r"C:\Windows\System32\cmd.exe".into());
    let permissions = Permissions { process: vec!["${setting:hello.cli}".into(), "/data/tools/*".into()], ..Permissions::default() };
    let mut ext = start_with(permissions, &[("hello.cli", &cmd)]);
    let (result, _) = ext.run("hello.spawn-setting");
    assert_eq!(result, Ok(()));
    let lines = ext.logged_until(|l| l.starts_with("process 1 ended"));
    assert!(lines.iter().any(|l| l == "process 1 said: by path"), "{lines:?}");
    let (_, seen) = ext.run("hello.spawn-data");
    // Listed: it's allowed, and fails only because there's nothing there.
    assert!(seen.iter().any(|e| matches!(e, Event::Said(Source::Log(Level::Warn), s) if s.starts_with("/data/tools/missing.exe:") && !s.contains("permissions"))), "{seen:?}");
    assert!(warned(&seen, "/data/elsewhere/x.exe: /data/elsewhere/x.exe isn't a program this extension may run"), "{seen:?}");
}

/// `find-program` finds what its manifest names, and the path it gives
/// may run; it won't look for anything else.
#[test]
fn a_program_it_finds_may_run_by_that_path() {
    let mut ext = start_with(Permissions { process: vec!["cmd".into()], ..Permissions::default() }, &[]);
    let (result, seen) = ext.run("hello.find");
    assert_eq!(result, Ok(()));
    assert!(seen.iter().any(|e| matches!(e, Event::Asked(Request::Found { name, path }) if name == "cmd" && path.is_file())), "{seen:?}");
    assert!(warned(&seen, "find-program: powershell isn't a program this extension may run"), "{seen:?}");
    assert!(seen.iter().any(|e| matches!(e, Event::Said(_, s) if s.eq_ignore_ascii_case("found cmd.exe"))), "{seen:?}");
    let lines = ext.logged_until(|l| l.starts_with("process 1 ended"));
    assert!(lines.iter().any(|l| l == "process 1 said: found it"), "{lines:?}");
}

/// Drawing over the editor takes `draw = "over"`; a sheet is a file of
/// its own folder, and the layer's items come to Fenix as they're set.
#[test]
fn it_draws_only_with_leave_and_only_its_own_sheets() {
    let mut ext = start();
    let (result, _) = ext.run("hello.layer");
    assert!(result.unwrap_err().contains("may not draw over the editor"));
    let mut ext = start_with(Permissions { draw: fenix_extensions::manifest::Draw::Over, ..Permissions::default() }, &[]);
    let (result, seen) = ext.run("hello.layer");
    assert_eq!(result, Ok(()));
    let asked: Vec<&Request> = seen.iter().filter_map(|e| if let Event::Asked(r) = e { Some(r) } else { None }).collect();
    let sheet = std::fs::read(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/guests/hello-sheet.png")).unwrap();
    assert!(matches!(asked[0], Request::SheetLoad { bytes, frame: (2, 2), .. } if *bytes == sheet), "read as it was checked: {asked:?}");
    assert!(matches!(asked[1], Request::LayerOpen { level: fenix_wasm::host::LayerLevel::AboveText, .. }));
    assert!(matches!(asked[2], Request::LayerSet { items, .. } if items.len() == 1 && items[0].glide.is_some() && items[0].clickable));
    assert!(warned(&seen, "../escape.png: a sheet is a file in its own folder"), "{seen:?}");
}

/// A model is a `.glb` of its own folder, read on the worker: its clips
/// come back to the extension, and the model to Fenix with its id.
#[test]
fn a_model_is_read_and_its_clips_told() {
    let mut ext = start();
    assert!(ext.run("hello.model").0.unwrap_err().contains("may not draw over the editor"));
    let mut ext = start_with(Permissions { draw: fenix_extensions::manifest::Draw::Over, ..Permissions::default() }, &[]);
    let (result, seen) = ext.run("hello.model");
    assert_eq!(result, Ok(()));
    let asked: Vec<&Request> = seen.iter().filter_map(|e| if let Event::Asked(r) = e { Some(r) } else { None }).collect();
    let Request::ModelLoad { id, model } = asked[0] else { panic!("{asked:?}") };
    assert_eq!(model.0.clip("bend"), Some(0));
    assert!(said(&seen, Level::Info, &format!("model {id} clips bend 1")), "{seen:?}");
    assert!(matches!(asked[2], Request::LayerSet { items, .. } if items[0].pose.as_ref().is_some_and(|p| p.model == *id && p.parts == ["hat"])));
    assert!(warned(&seen, "missing.glb"), "{seen:?}");
}

/// A terminal it owns: under the focused pane, stopped and started again;
/// and a task, both asked of Fenix with their ids.
#[test]
fn terminals_and_tasks_are_asked_for_with_ids() {
    let mut ext = start_with(Permissions { process: vec!["cmd".into()], ..Permissions::default() }, &[]);
    let (result, seen) = ext.run("hello.terminal");
    assert_eq!(result, Ok(()));
    let asked: Vec<&Request> = seen.iter().filter_map(|e| if let Event::Asked(r) = e { Some(r) } else { None }).collect();
    let Some(Request::Terminal { id, title, below: true, .. }) = asked.first() else { panic!("{asked:?}") };
    assert_eq!(title, "Hello monitor");
    assert_eq!(asked[1], &Request::TerminalStop(*id));
    assert!(matches!(asked[2], Request::TerminalRestart { id: i, program: Some((_, args, _)), title: Some(t) } if i == id && args[2] == "again" && t == "Hello again"), "{asked:?}");
    let (_, seen) = ext.run("hello.task");
    assert!(seen.iter().any(|e| matches!(e, Event::Asked(Request::Task { name, args, .. }) if name == "hello build" && args[0] == "/c")), "{seen:?}");
    // What Fenix tells it back.
    ext.worker.call(Call::Deliver(Notice::TaskFinished((2, true))), 0);
    ext.logged_until(|l| l == "task 2 finished: true");
    ext.worker.call(Call::Deliver(Notice::FocusChanged(Some("/project/src/main.cpp".into()))), 0);
    ext.logged_until(|l| l == "in Some(\"/project/src/main.cpp\")");
}

/// HTTP only to the hosts it names -- here through a setting -- with the
/// answer as an event.
#[test]
fn http_reaches_only_the_hosts_it_names() {
    let server = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = server.local_addr().unwrap().port();
    std::thread::spawn(move || {
        if let Ok((mut stream, _)) = server.accept() {
            use std::io::{Read, Write};
            let mut buf = [0u8; 1024];
            let _ = stream.read(&mut buf);
            let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello");
        }
    });
    let url = format!("http://127.0.0.1:{port}/x");
    let mut ext = start_with(Permissions { network: vec!["${setting:hello.url}".into()], ..Permissions::default() }, &[("hello.url", &url)]);
    assert_eq!(ext.run("hello.http").0, Ok(()));
    let lines = ext.logged_until(|s| s.starts_with("response"));
    assert_eq!(lines.last().map(String::as_str), Some("response 1: 200 hello"), "{lines:?}");
    let (_, seen) = ext.run("hello.http-denied");
    assert!(warned(&seen, "example.org isn't a host this extension may reach"), "{seen:?}");
}

/// Secrets only as its manifest names them, settings only its own.
#[test]
fn secrets_and_settings_are_only_its_own() {
    let mut ext = start_with(Permissions { secrets: vec!["hello.token".into()], ..Permissions::default() }, &[("other.url", "x")]);
    let (_, seen) = ext.run("hello.secret");
    assert!(said(&seen, Level::Info, "token: Some(\"s3cret\")"), "{seen:?}");
    assert!(said(&seen, Level::Info, "other: None"), "{seen:?}");
    assert!(said(&seen, Level::Info, "their setting: None"), "{seen:?}");
}

/// A setting it contributes under a name that isn't its own -- a 1.x
/// feature's, as the PDF reader's `reader.zoom` -- it reads as its own.
#[test]
fn a_setting_it_contributes_under_a_namespace_is_its_own() {
    let mut ext = start_tuned(Permissions::default(), &[("other.url", "x")], |spec| spec.own_settings = vec!["other.url".into()]);
    let (_, seen) = ext.run("hello.secret");
    assert!(said(&seen, Level::Info, "their setting: Some(\"x\")"), "{seen:?}");
}

/// Another's settings it may read (`permissions.settings`, as the MIB
/// reads the CCSDS extension's `ccsds.*`) it reads as its own.
#[test]
fn another_namespace_it_may_read_is_read() {
    let mut ext = start_with(Permissions { settings: vec!["other".into()], ..Permissions::default() }, &[("other.url", "x")]);
    let (_, seen) = ext.run("hello.secret");
    assert!(said(&seen, Level::Info, "their setting: Some(\"x\")"), "{seen:?}");
}

/// What only Fenix can do is asked of it, and the answer delivered.
#[test]
fn a_picker_is_asked_for_and_its_choice_delivered() {
    let mut ext = start();
    let (_, seen) = ext.run("hello.pick");
    let Some(Request::Picker { id, title, items }) = seen.iter().find_map(|e| if let Event::Asked(r) = e { Some(r.clone()) } else { None }) else { panic!("{seen:?}") };
    assert_eq!((title.as_str(), items.len(), items[1].value.as_str()), ("A colour", 2, "green"));
    let (delivered, seen) = ext.call(Call::Deliver(Notice::Picked((id, Some("green".into())))));
    assert_eq!(delivered, Ok(Reply::Done));
    assert!(said(&seen, Level::Info, &format!("picked {id}: Some(\"green\")")), "{seen:?}");
    let (_, seen) = ext.run("hello.service");
    assert!(seen.iter().any(|e| matches!(e, Event::Asked(Request::Service { name, payload, .. }) if name == "echo" && payload == "ping")), "{seen:?}");
}

/// Its page: opened by asking, laid out when Fenix asks, keys sent to it
/// (and it asks to be laid out again), kept and restored by its state.
#[test]
fn its_page_is_rendered_keyed_and_restored() {
    let mut ext = start();
    let (_, seen) = ext.run("hello.page");
    let Some(page) = seen.iter().find_map(|e| if let Event::Asked(Request::PageOpen { page, kind, title }) = e { (kind == "counter" && title == "Hello").then_some(*page) } else { None }) else { panic!("{seen:?}") };
    let render = |ext: &mut Running, page: u64| laid_out(ext.call(Call::Render { page, width: 80, height: 24 }).0);
    let first = render(&mut ext, page);
    assert_eq!(first.lines[0].spans[1], Span { text: "  80 wide".into(), role: fenix_wasm::host::Role::Muted });
    assert_eq!(first.lines[1], Line { spans: Vec::new(), rule: true });
    assert_eq!(first.focus, Some(2));
    let (_, seen) = ext.call(Call::Key { page, key: Key::Char('j') });
    assert!(seen.contains(&Event::Asked(Request::PageRefresh(page))), "{seen:?}");
    assert_eq!(render(&mut ext, page).lines[2].spans[0].text, "counted 1");
    assert_eq!(ext.call(Call::PageState(page)).0, Ok(Reply::State(Some("1".into()))));
    assert_eq!(ext.call(Call::PageRestore { page: 99, kind: "counter".into(), state: "7".into() }).0, Ok(Reply::Restored(true)));
    assert_eq!(render(&mut ext, 99).lines[2].spans[0].text, "counted 7");
    assert_eq!(ext.call(Call::PageRestore { page: 100, kind: "other".into(), state: "1".into() }).0, Ok(Reply::Restored(false)));
}

/// 0.2's calls: a read-only text, the clipboard, its modeline item and
/// a URL -- http(s) only -- each asked of Fenix.
#[test]
fn text_urls_the_clipboard_and_its_status_are_asked_for() {
    let mut ext = start();
    let (result, seen) = ext.run("hello.extras");
    assert_eq!(result, Ok(()), "{seen:?}");
    let asked: Vec<&Request> = seen.iter().filter_map(|e| if let Event::Asked(r) = e { Some(r) } else { None }).collect();
    assert!(asked.contains(&&Request::OpenText { title: "Hello JSON".into(), text: "{\"hello\": 1}".into(), language: Some("json".into()), beside: None }), "{asked:?}");
    assert!(asked.contains(&&Request::Clipboard("copied by hello".into())));
    assert!(asked.contains(&&Request::Status(Some("hello · ok".into()))));
    assert!(asked.contains(&&Request::OpenUrl("https://example.com/".into())));
    assert!(!asked.iter().any(|r| matches!(r, Request::OpenUrl(u) if u.starts_with("file:"))), "{asked:?}");
    assert!(warned(&seen, "only http(s) URLs open"), "{seen:?}");
}

/// A page's popup, and being told when it's shown.
#[test]
fn a_page_has_a_popup_and_hears_when_its_shown() {
    let mut ext = start();
    let (_, seen) = ext.run("hello.page");
    let Some(page) = seen.iter().find_map(|e| if let Event::Asked(Request::PageOpen { page, .. }) = e { Some(*page) } else { None }) else { panic!("{seen:?}") };
    let render = |ext: &mut Running| laid_out(ext.call(Call::Render { page, width: 80, height: 24 }).0);
    assert_eq!(render(&mut ext).popup, None);
    for _ in 0..2 {
        assert_eq!(ext.call(Call::Key { page, key: Key::Char('j') }).0, Ok(Reply::Done));
    }
    let popup = render(&mut ext).popup.expect("a popup past two counts");
    assert_eq!((popup.line, popup.rows[0][1].text.as_str()), (2, "  reset"));
    let (_, seen) = ext.call(Call::Deliver(Notice::PageShown((page, false))));
    assert!(said(&seen, Level::Info, &format!("page {page} shown: false")), "{seen:?}");
}

#[test]
fn its_services_answer_now_or_later() {
    let mut ext = start();
    assert_eq!(ext.call(Call::Serve { call: 1, name: "hello.upper".into(), payload: "abc".into() }).0, Ok(Reply::Served(Some(Ok("ABC".into())))));
    assert_eq!(ext.call(Call::Serve { call: 2, name: "nope".into(), payload: String::new() }).0, Ok(Reply::Served(Some(Err("no service nope".into())))));
    // Later: none now, then its answer to Fenix's call 7.
    assert_eq!(ext.call(Call::Serve { call: 7, name: "hello.later".into(), payload: "x".into() }).0, Ok(Reply::Served(None)));
    let seen = ext.until(|e| matches!(e, Event::Asked(Request::ServiceAnswer { .. })));
    assert_eq!(seen.last(), Some(&Event::Asked(Request::ServiceAnswer { call: 7, answer: Ok("later: x".into()) })));
}

#[test]
fn a_canvas_page_is_laid_out_cell_by_cell_and_takes_pastes() {
    let mut ext = start();
    let (_, seen) = ext.run("hello.canvas");
    let Some(page) = seen.iter().find_map(|e| if let Event::Asked(Request::PageOpen { page, kind, .. }) = e { (kind == "canvas").then_some(*page) } else { None }) else { panic!("{seen:?}") };
    let laid = laid_out(ext.call(Call::Render { page, width: 80, height: 24 }).0);
    let canvas = laid.canvas.expect("a canvas");
    assert!(canvas.text.contains("Canvas 80"), "{}", canvas.text);
    assert_eq!(canvas.spans[0].role, fenix_wasm::host::Role::Syntax("keyword".into()));
    assert!((laid.space, laid.typing) == (true, false));
    assert_eq!(ext.call(Call::Paste { page, text: "abc".into() }).0, Ok(Reply::Done));
    let laid = laid_out(ext.call(Call::Render { page, width: 80, height: 24 }).0);
    assert!(laid.canvas.unwrap().text.contains("counted 3") && laid.typing, "a paste counted, and odd: typing");
}

#[test]
fn the_new_requests_are_asked_for_and_held_to_the_manifest() {
    let mut ext = start_with(Permissions::default(), &[("hello.url", "https://x.example/")]);
    std::fs::create_dir_all(ext.dir.path().join("data")).unwrap();
    // A 1x1 PNG, in its folder.
    let png: &[u8] = &[
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15,
        0xc4, 0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0xf8, 0xcf, 0xc0, 0xf0, 0x1f, 0x00, 0x05, 0x00, 0x01, 0xff, 0x89, 0x99, 0x3d, 0x1d, 0x00, 0x00, 0x00, 0x00,
        0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
    ];
    std::fs::write(ext.dir.path().join("data/dot.png"), png).unwrap();
    let (result, seen) = ext.run("hello.v6");
    // Not one of its settings: refused, so its own is the only change.
    assert_eq!(result, Err("hello.url isn't one of its settings (contributes.settings)".into()), "{seen:?}");
}

#[test]
fn the_new_requests_are_asked_for() {
    let engine = engine(&Options::default()).unwrap();
    let ticker = Ticker::start(&engine);
    let dir = tempfile::tempdir().unwrap();
    let mut spec = Spec::new("test.hello", hello(), dir.path());
    spec.own_settings = vec!["hello.url".into()];
    spec.settings = Arc::new(RwLock::new(HashMap::from([("hello.url".to_string(), "https://x.example/".to_string())])));
    let settings = spec.settings.clone();
    let (tx, events) = mpsc::channel();
    let worker = Worker::start(&engine, spec, move |e| {
        let _ = tx.send(e);
    });
    let mut ext = Running { worker, events, dir, _ticker: ticker };
    ext.until(|e| *e == Event::Ready);
    let png: &[u8] = &[
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15,
        0xc4, 0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0xf8, 0xcf, 0xc0, 0xf0, 0x1f, 0x00, 0x05, 0x00, 0x01, 0xff, 0x89, 0x99, 0x3d, 0x1d, 0x00, 0x00, 0x00, 0x00,
        0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
    ];
    std::fs::write(ext.dir.path().join("data/dot.png"), png).unwrap();
    let (result, seen) = ext.run("hello.v6");
    assert_eq!(result, Ok(()), "{seen:?}");
    assert!(said(&seen, Level::Info, "image 1x1"), "{seen:?}");
    assert!(seen.iter().any(|e| matches!(e, Event::Asked(Request::ImageLoad { .. }))), "{seen:?}");
    assert!(seen.contains(&Event::Asked(Request::SettingSet { key: "hello.url".into(), value: Some("https://changed.example/".into()) })), "{seen:?}");
    assert_eq!(settings.read().unwrap().get("hello.url").map(String::as_str), Some("https://changed.example/"), "its map has it at once");
    assert!(said(&seen, Level::Info, "set: other.key isn't one of its settings (contributes.settings)"), "{seen:?}");
    assert!(seen.iter().any(|e| matches!(e, Event::Asked(Request::Home(Some(s))) if s.rows[0].value == "row-1")), "{seen:?}");
    assert!(seen.iter().any(|e| matches!(e, Event::Asked(Request::PickerMany { items, .. }) if items.len() == 2)), "{seen:?}");
    let (_, seen) = ext.run("hello.compose");
    assert!(seen.iter().any(|e| matches!(e, Event::Asked(Request::Compose { verb, initial, allow_empty: false, .. }) if verb == "save" && initial == "seed")), "{seen:?}");
    // Where you are needs `editor = "read"`: without it, nowhere.
    let (_, seen) = ext.run("hello.here");
    assert!(said(&seen, Level::Info, "here: nowhere"), "{seen:?}");
}

/// A language server, through its port: framed messages in, framed
/// answers out, as over a server's stdin and stdout.
#[test]
fn it_answers_as_a_language_server_through_its_port() {
    let ext = start();
    let port = ext.worker.lsp_port();
    let (from_ext, to_fenix) = std::io::pipe().unwrap();
    port.open(7, to_fenix);
    port.send(7, r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#.into());
    port.send(7, r#"{"jsonrpc":"2.0","id":2,"method":"textDocument/hover","params":{}}"#.into());
    port.send(7, r#"{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///a.md","languageId":"markdown","version":0,"text":"x"}}}"#.into());
    let mut reader = std::io::BufReader::new(from_ext);
    let mut frame = || {
        use std::io::{BufRead, Read};
        let mut header = String::new();
        reader.read_line(&mut header).unwrap();
        let length: usize = header.trim().strip_prefix("Content-Length: ").unwrap().parse().unwrap();
        let mut blank = String::new();
        reader.read_line(&mut blank).unwrap();
        let mut body = vec![0; length];
        reader.read_exact(&mut body).unwrap();
        String::from_utf8(body).unwrap()
    };
    assert_eq!(frame(), r#"{"jsonrpc":"2.0","id":1,"result":{"capabilities":{"hoverProvider":true}}}"#);
    assert_eq!(frame(), r#"{"jsonrpc":"2.0","id":2,"result":{"contents":"hello from the guest"}}"#);
    assert!(frame().contains("a guest's problem"));
    // Closed: what it says goes nowhere, and nothing breaks.
    port.close(7);
    port.send(7, r#"{"jsonrpc":"2.0","id":3,"method":"textDocument/hover","params":{}}"#.into());
}

#[test]
fn where_you_are_is_read_with_editor_read() {
    let engine = engine(&Options::default()).unwrap();
    let ticker = Ticker::start(&engine);
    let dir = tempfile::tempdir().unwrap();
    let mut spec = Spec::new("test.hello", hello(), dir.path());
    spec.permissions.editor = fenix_extensions::manifest::Editor::Read;
    *spec.here.write().unwrap() = Some(fenix_wasm::host::Here { path: Some("C:/w/a.rs".into()), line: 3, text: "let x = 1;".into(), language: "rs".into(), project: Some(fenix_wasm::host::ProjectInfo { name: "w".into(), root: "C:/w".into(), meta: Vec::new() }) });
    let (tx, events) = mpsc::channel();
    let worker = Worker::start(&engine, spec, move |e| {
        let _ = tx.send(e);
    });
    let mut ext = Running { worker, events, dir, _ticker: ticker };
    ext.until(|e| *e == Event::Ready);
    let (_, seen) = ext.run("hello.here");
    assert!(said(&seen, Level::Info, "here: Some(\"C:/w/a.rs\") line 3 \"let x = 1;\" in Some(\"w\")"), "{seen:?}");
}

/// An extension written with `fenix-extension-api`, as an author would:
/// its state a struct kept between calls, its page built, its timer's
/// event handled, its page restored and its service answered.
#[test]
fn an_extension_written_with_the_api_crate_runs() {
    let engine = engine(&Options::default()).unwrap();
    let _ticker = Ticker::start(&engine);
    let dir = tempfile::tempdir().unwrap();
    let wasm = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/guests/counter.wasm");
    let (tx, events) = mpsc::channel();
    let worker = Worker::start(&engine, Spec::new("test.counter", wasm, dir.path()), move |e| {
        let _ = tx.send(e);
    });
    let mut ext = Running { worker, events, dir, _ticker: Ticker::start(&engine) };
    let started = ext.until(|e| *e == Event::Ready);
    assert!(said(&started, Level::Info, "counter is up"), "{started:?}");
    let (_, seen) = ext.run("counter.open");
    let Some(page) = seen.iter().find_map(|e| if let Event::Asked(Request::PageOpen { page, .. }) = e { Some(*page) } else { None }) else { panic!("{seen:?}") };
    for _ in 0..3 {
        let (_, seen) = ext.call(Call::Key { page, key: Key::Char('j') });
        assert!(seen.contains(&Event::Asked(Request::PageRefresh(page))), "{seen:?}");
    }
    let Ok(Reply::Page(laid)) = ext.call(Call::Render { page, width: 60, height: 24 }).0 else { panic!() };
    assert_eq!(laid.lines[0].spans[1].text, "  60 wide");
    assert_eq!(laid.lines[2].spans.iter().map(|s| s.text.as_str()).collect::<String>(), "count 3", "its count kept between calls");
    assert_eq!(laid.keys[0].what, "count");
    assert_eq!(ext.call(Call::Serve { call: 1, name: "counter.count".into(), payload: String::new() }).0, Ok(Reply::Served(Some(Ok("3".into())))));
    assert_eq!(ext.call(Call::PageState(page)).0, Ok(Reply::State(Some("3".into()))));
    assert_eq!(ext.call(Call::PageRestore { page, kind: "counter".into(), state: "10".into() }).0, Ok(Reply::Restored(true)));
    assert_eq!(ext.call(Call::Serve { call: 2, name: "counter.count".into(), payload: String::new() }).0, Ok(Reply::Served(Some(Ok("10".into())))));
    assert_eq!(ext.run("counter.tick").0, Ok(()));
    assert_eq!(ext.logged_until(|s| s.starts_with("ticked")).last().map(String::as_str), Some("ticked 1"));
    assert_eq!(ext.run("nope").0, Err("no command nope".into()), "the trait's default answer");
}

/// Compiled once, then loaded from the cache; other bytes, another file.
#[test]
fn compiled_code_is_kept_and_reused() {
    let engine = engine(&Options::default()).unwrap();
    let cache = tempfile::tempdir().unwrap();
    load(&engine, &hello(), cache.path()).unwrap();
    let kept: Vec<_> = std::fs::read_dir(cache.path()).unwrap().flatten().map(|e| e.path()).collect();
    assert_eq!(kept.len(), 1, "{kept:?}");
    assert!(kept[0].extension().is_some_and(|x| x == "cwasm"));
    let modified = std::fs::metadata(&kept[0]).unwrap().modified().unwrap();
    let started = std::time::Instant::now();
    load(&engine, &hello(), cache.path()).unwrap();
    assert!(started.elapsed() < Duration::from_millis(500), "loaded, not compiled: {:?}", started.elapsed());
    assert_eq!(std::fs::metadata(&kept[0]).unwrap().modified().unwrap(), modified, "the same file, untouched");
    // A file that won't load is compiled again over.
    std::fs::write(&kept[0], b"not compiled code").unwrap();
    load(&engine, &hello(), cache.path()).unwrap();
    assert!(std::fs::read(&kept[0]).unwrap().len() > 1000);
}

#[test]
fn an_extension_that_cant_load_says_why() {
    let engine = engine(&Options::default()).unwrap();
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("extension.wasm"), b"\0asm garbage").unwrap();
    let spec = Spec::new("test.broken", dir.path().join("extension.wasm"), dir.path());
    let (tx, rx) = mpsc::channel();
    let mut worker = Worker::start(&engine, spec, move |e| {
        let _ = tx.send(e);
    });
    let first = rx.recv_timeout(Duration::from_secs(10)).unwrap();
    assert!(matches!(&first, Event::Stopped(s) if s.starts_with("test.broken couldn't start")), "{first:?}");
    let call = worker.call(Call::RunCommand("x".into()), 10);
    assert!(matches!(rx.recv_timeout(Duration::from_secs(10)).unwrap(), Event::Finished { call: c, result: Err(_) } if c == call));
}

/// a TCP socket to an echo server, held to `permissions.sockets` by
/// way of a setting: connected, written before it was, read, closed.
#[test]
fn a_socket_echoes_through_a_local_listener() {
    let server = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = server.local_addr().unwrap().port();
    std::thread::spawn(move || {
        if let Ok((mut stream, _)) = server.accept() {
            use std::io::{Read, Write};
            let mut buf = [0u8; 64];
            if let Ok(n) = stream.read(&mut buf) {
                let _ = stream.write_all(&buf[..n]);
            }
        }
    });
    let address = format!("127.0.0.1:{port}");
    let mut ext = start_with(Permissions { sockets: vec!["${setting:hello.echo}".into()], ..Permissions::default() }, &[("hello.echo", &address)]);
    let (result, seen) = ext.run("hello.socket");
    assert_eq!(result, Ok(()), "{seen:?}");
    assert!(warned(&seen, &format!("127.0.0.1:{} isn't somewhere this extension may open a TCP socket to", port.wrapping_add(1).max(1))), "{seen:?}");
    let lines = ext.logged_until(|l| l.starts_with("socket 1 said"));
    assert!(lines.contains(&"socket 1 connected: Ok(())".to_string()), "{lines:?}");
    assert_eq!(lines.last().map(String::as_str), Some("socket 1 said: ping"));
    // Without the permission, nothing.
    let mut none = start_with(Permissions::default(), &[("hello.echo", &address)]);
    assert!(none.run("hello.socket").0.unwrap_err().contains("permissions.sockets"));
}

/// A UDP socket on any free port sends a datagram and hears the answer
/// from where it sent; a port of this machine's needs its own entry.
#[test]
fn a_udp_socket_sends_and_hears_back() {
    let echo = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let port = echo.local_addr().unwrap().port();
    std::thread::spawn(move || {
        let mut buf = [0u8; 64];
        if let Ok((n, from)) = echo.recv_from(&mut buf) {
            let _ = echo.send_to(&buf[..n], from);
        }
    });
    let address = format!("127.0.0.1:{port}");
    let mut ext = start_with(Permissions { sockets: vec![format!("udp://127.0.0.1:{port}")], ..Permissions::default() }, &[("hello.echo", &address)]);
    let (result, seen) = ext.run("hello.udp");
    assert_eq!(result, Ok(()), "{seen:?}");
    assert!(warned(&seen, "port 9 isn't one this extension may take"), "{seen:?}");
    let lines = ext.logged_until(|l| l.starts_with("datagram"));
    assert_eq!(lines.last().map(String::as_str), Some("datagram on 1: ping"));
}

/// A surface: its pixels written on the worker into the framebuffer Fenix
/// draws from, shown at `surface-present`, a key and a scroll back to it.
#[test]
fn a_surface_is_fed_presented_and_keyed() {
    use fenix_wasm::host::{KeyEvent, Modifiers, SurfaceInput, SurfaceKey, SurfaceScale};
    let mut ext = start();
    let (result, seen) = ext.run("hello.surface");
    assert_eq!(result, Ok(()), "{seen:?}");
    assert!(warned(&seen, "isn't inside the surface's 4 x 2"), "{seen:?}");
    let Some(Request::SurfaceOpen { page, kind, options, surface, .. }) = seen.iter().find_map(|e| if let Event::Asked(r @ Request::SurfaceOpen { .. }) = e { Some(r.clone()) } else { None }) else { panic!("{seen:?}") };
    assert_eq!((kind.as_str(), options.capture, options.scale), ("screen", true, SurfaceScale::Stretch));
    assert_eq!(seen.iter().filter(|e| matches!(e, Event::Asked(Request::SurfaceChanged(p)) if *p == page)).count(), 1, "told once until it looks: {seen:?}");
    {
        let mut fb = surface.lock();
        assert_eq!(fb.size(), (4, 2));
        assert_eq!(&fb.pixels()[0..4], &[0, 0, 255, 255], "red, as BGRA");
        assert_eq!(&fb.pixels()[(4 + 3) * 4..(4 + 3) * 4 + 4], &[255, 255, 255, 255], "the white pixel");
        assert_eq!(&fb.pixels()[2 * 4..2 * 4 + 4], &[0, 0, 0, 255], "black where nothing was written");
        assert_eq!(fb.take_dirty(), Some((0, 0, 4, 2)), "all of it, the first time");
        assert_eq!(fb.take_dirty(), None);
        assert_eq!(fb.take_cursor().flatten().map(|c| (c.width, c.height)), Some((1, 1)));
        assert_eq!(fb.marks().len(), 1);
    }
    let key = |c| SurfaceInput::Key(KeyEvent { key: SurfaceKey::Char(c), down: true, mods: Modifiers { ctrl: true, alt: false, meta: false } });
    let (_, seen) = ext.call(Call::SurfaceInput { surface: page, input: key('c') });
    assert!(said(&seen, Level::Info, "key SurfaceKey::Char('c') down ctrl") && said(&seen, Level::Info, "copied: Ok(())"), "{seen:?}");
    let mut fb = surface.lock();
    assert_eq!(&fb.pixels()[2 * 4..2 * 4 + 4], &[0, 0, 255, 255], "the red half moved right");
    assert_eq!(fb.take_dirty(), Some((2, 0, 2, 2)), "only what changed");
}

/// `page-focus` asked of Fenix; `page-closed` told to the extension; a surface opened while its page is restored put in
/// that page's place.
#[test]
fn a_page_is_brought_here_told_when_its_closed_and_a_surface_restored_in_place() {
    let opened = |seen: &[Event]| seen.iter().find_map(|e| if let Event::Asked(Request::SurfaceOpen { page, replaces, .. }) = e { Some((*page, *replaces)) } else { None });
    let mut ext = start();
    let (_, seen) = ext.run("hello.surface");
    let Some((page, None)) = opened(&seen) else { panic!("a surface of its own: {seen:?}") };
    let (_, seen) = ext.run("hello.focus");
    assert!(seen.contains(&Event::Asked(Request::PageFocus(page))), "{seen:?}");
    let (closed, seen) = ext.call(Call::PageClosed(page));
    assert_eq!(closed, Ok(Reply::Done));
    assert!(said(&seen, Level::Info, &format!("you closed {page}")), "{seen:?}");
    let (restored, seen) = ext.call(Call::PageRestore { page: 77, kind: "screen".into(), state: String::new() });
    assert_eq!(restored, Ok(Reply::Restored(true)));
    let Some((again, Some(77))) = opened(&seen) else { panic!("in page 77's place: {seen:?}") };
    assert!(said(&seen, Level::Info, &format!("restored 77 as surface {again}")), "{seen:?}");
    let (_, seen) = ext.run("hello.surface");
    assert!(matches!(opened(&seen), Some((_, None))), "only while it's restoring: {seen:?}");
}

/// An extension written with `fenix-extension-api` that implements only
/// what it needs: what it leaves out is taken by the crate's defaults, and
/// it carries on.
#[test]
fn what_an_extension_leaves_out_is_taken_by_its_defaults() {
    let engine = engine(&Options::default()).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let wasm = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/guests/counter.wasm");
    let (tx, events) = mpsc::channel();
    let worker = Worker::start(&engine, Spec::new("test.counter", wasm, dir.path()), move |e| {
        let _ = tx.send(e);
    });
    let mut ext = Running { worker, events, dir, _ticker: Ticker::start(&engine) };
    ext.until(|e| *e == Event::Ready);
    let (_, seen) = ext.run("counter.open");
    let Some(page) = seen.iter().find_map(|e| if let Event::Asked(Request::PageOpen { page, .. }) = e { Some(*page) } else { None }) else { panic!("{seen:?}") };
    assert_eq!(ext.call(Call::PageClosed(page)).0, Ok(Reply::Done));
    // Nor events it doesn't listen for.
    assert_eq!(ext.call(Call::BufferChanged(Changed::Scratch(1))).0, Ok(Reply::Done));
    assert_eq!(ext.call(Call::ScratchClosed(1)).0, Ok(Reply::Done));
    assert_eq!(ext.call(Call::FocusMoved(Focus::None)).0, Ok(Reply::Done));
    assert_eq!(ext.run("counter.tick").0, Ok(()), "still running");
}

/// `spawn-raw`: stdout as bytes. `file-read`: only where it may read, or
/// a path Fenix gave it. The clipboard's text only with its permission.
#[test]
fn raw_output_files_given_and_the_clipboard() {
    let me = std::env::current_exe().unwrap().display().to_string();
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("given.bin");
    std::fs::write(&file, [1u8, 2, 3]).unwrap();
    let file_text = file.display().to_string();
    let settings = [("hello.cli", me.as_str()), ("hello.args", "--list"), ("hello.file", file_text.as_str())];
    let mut ext = start_with(Permissions { process: vec!["${setting:hello.cli}".into()], ..Permissions::default() }, &settings);
    assert_eq!(ext.run("hello.raw").0, Ok(()));
    let lines = ext.logged_until(|l| l.starts_with("process 1 ended"));
    assert!(lines.contains(&"raw 1 wrote bytes".to_string()), "{lines:?}");
    let (_, seen) = ext.run("hello.read");
    assert!(warned(&seen, "not somewhere it may read") && warned(&seen, "may not read the clipboard"), "{seen:?}");
    assert!(said(&seen, Level::Info, "clipboard: None"), "{seen:?}");
    assert!(seen.contains(&Event::Asked(Request::PageRetitle { page: 1, title: "Renamed".into() })) && seen.contains(&Event::Asked(Request::GitRefresh)), "{seen:?}");
    assert_eq!(ext.call(Call::Grant(dir.path().to_path_buf())).0, Ok(Reply::Done));
    let (_, seen) = ext.run("hello.read");
    assert!(said(&seen, Level::Info, "read 3 bytes"), "{seen:?}");
    // A file it opens itself, given with the event.
    let (_, seen) = ext.call(Call::Deliver(Notice::Opened(file_text.clone())));
    assert!(said(&seen, Level::Info, "opened given.bin: Ok(3)"), "{seen:?}");
}

/// A file it may not reach, with `editor = "write"`: `open-file` opens it
/// sealed, for you; `buffer-text` and `buffer-edit` reach only a buffer
/// you have open on it, not one it opened; `image-file` refuses it. Given
/// to it, or in its project, it's its to use.
#[test]
fn a_file_it_may_not_reach_is_opened_for_you_and_kept_from_it() {
    let outside = tempfile::tempdir().unwrap();
    let file = outside.path().join("id_rsa.png");
    std::fs::copy(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/guests/hello-sheet.png"), &file).unwrap();
    let file_text = file.display().to_string();
    let editor = Permissions { editor: fenix_extensions::manifest::Editor::Write, ..Permissions::default() };
    let mut ext = start_with(editor.clone(), &[("hello.file", file_text.as_str())]);
    let asked = std::sync::Mutex::new(Vec::new());
    let call = ext.worker.call(Call::RunCommand("hello.escape".into()), 0);
    let seen = ext.until(|e| {
        if let Event::Asked(Request::BufferText { open_only, answer, .. }) = e {
            asked.lock().unwrap().push(*open_only);
            answer.send(None);
        }
        matches!(e, Event::Finished { call: c, .. } if *c == call)
    });
    assert!(seen.contains(&Event::Asked(Request::OpenFile { path: file.clone(), line: None, sealed: true, text: None })), "{seen:?}");
    assert_eq!(*asked.lock().unwrap(), [true], "only a buffer you have open");
    assert!(seen.iter().any(|e| matches!(e, Event::Asked(Request::BufferEdit { open_only: true, .. }))), "{seen:?}");
    assert!(said(&seen, Level::Info, "escape text: None"), "{seen:?}");
    assert!(seen.iter().any(|e| matches!(e, Event::Said(_, s) if s.starts_with("escape image: Err(") && s.contains("not somewhere it may read"))), "{seen:?}");

    // Given to it (`opened`, a forge's repository): its to read and edit.
    assert_eq!(ext.call(Call::Grant(outside.path().to_path_buf())).0, Ok(Reply::Done));
    let call = ext.worker.call(Call::RunCommand("hello.escape".into()), 0);
    let seen = ext.until(|e| {
        if let Event::Asked(Request::BufferText { open_only, answer, .. }) = e {
            asked.lock().unwrap().push(*open_only);
            answer.send(Some("x".into()));
        }
        matches!(e, Event::Finished { call: c, .. } if *c == call)
    });
    assert!(seen.contains(&Event::Asked(Request::OpenFile { path: file.clone(), line: None, sealed: false, text: None })), "{seen:?}");
    assert_eq!(*asked.lock().unwrap(), [true, false]);
    assert!(said(&seen, Level::Info, "escape image: Ok(4)"), "{seen:?}");

    // Without `editor = "read"`, not even opened.
    let mut ext = start_with(Permissions::default(), &[("hello.file", file_text.as_str())]);
    let (_, seen) = ext.run("hello.escape");
    assert!(!seen.iter().any(|e| matches!(e, Event::Asked(Request::OpenFile { .. }))) && warned(&seen, "isn't somewhere it may open"), "{seen:?}");

    // The project's own files, spelled as full paths, are its.
    let project = Permissions { filesystem: fenix_extensions::manifest::Filesystem::ProjectRead, ..editor };
    let mut ext = start_tuned(project, &[("hello.file", file_text.as_str())], |spec| spec.project = Some(outside.path().to_path_buf()));
    let call = ext.worker.call(Call::RunCommand("hello.escape".into()), 0);
    let seen = ext.until(|e| {
        if let Event::Asked(Request::BufferText { answer, .. }) = e {
            answer.send(None);
        }
        matches!(e, Event::Finished { call: c, .. } if *c == call)
    });
    assert!(seen.contains(&Event::Asked(Request::OpenFile { path: file.clone(), line: None, sealed: false, text: None })), "{seen:?}");
    assert!(said(&seen, Level::Info, "escape image: Ok(4)"), "{seen:?}");
}

/// where the cursor and the selection were, with `editor = "read"`.
#[test]
fn the_cursor_and_the_selection_are_read_with_editor_read() {
    let cursor = fenix_wasm::host::CursorInfo { at: (2, 4), selection: Some(((2, 0), (2, 4))), selected: true };
    let editor = Permissions { editor: fenix_extensions::manifest::Editor::Read, ..Permissions::default() };
    let mut ext = start_tuned(editor, &[], |spec| *spec.cursor.write().unwrap() = Some(cursor));
    let (_, seen) = ext.run("hello.cursor");
    assert!(said(&seen, Level::Info, "cursor: Some(CursorInfo { at: (2, 4), selection: Some(((2, 0), (2, 4))), selected: true })"), "{seen:?}");
    let mut ext = start_tuned(Permissions::default(), &[], |spec| *spec.cursor.write().unwrap() = Some(cursor));
    let (_, seen) = ext.run("hello.cursor");
    assert!(said(&seen, Level::Info, "cursor: None") && warned(&seen, "may not read the editor"), "{seen:?}");
}

/// a scratch buffer of its own, opened, set and read back; its
/// changes and its closing told to it, and what has the focus.
#[test]
fn a_scratch_buffer_is_its_own_and_its_changes_are_told() {
    let mut ext = start();
    let call = ext.worker.call(Call::RunCommand("hello.scratch".into()), 0);
    let seen = ext.until(|e| {
        if let Event::Asked(Request::ScratchText { answer, .. }) = e {
            answer.send(Some("two\n".into()));
        }
        matches!(e, Event::Finished { call: c, .. } if *c == call)
    });
    let Some(id) = seen.iter().find_map(|e| if let Event::Asked(Request::ScratchOpen { id, title, text, language, beside: None }) = e { (title == "Notes" && text == "one\n" && language.as_deref() == Some("md")).then_some(*id) } else { None }) else { panic!("{seen:?}") };
    assert!(seen.contains(&Event::Asked(Request::ScratchSet { id, text: "two\n".into() })), "{seen:?}");
    assert!(said(&seen, Level::Info, "scratch text: Some(\"two\\n\")"), "{seen:?}");
    assert!(seen.iter().any(|e| matches!(e, Event::Said(_, s) if s.starts_with("not its own: Err("))), "{seen:?}");
    let (_, seen) = ext.call(Call::BufferChanged(Changed::Scratch(id)));
    assert!(said(&seen, Level::Info, &format!("changed Changed::Scratch({id})")), "{seen:?}");
    let (_, seen) = ext.call(Call::ScratchClosed(id));
    assert!(said(&seen, Level::Info, &format!("scratch {id} closed")), "{seen:?}");
    let (_, seen) = ext.call(Call::FocusMoved(Focus::Page(FocusedPage { kind: "git".into(), mine: None })));
    assert!(said(&seen, Level::Info, "focus Focus::Page(FocusedPage { kind: \"git\", mine: None })"), "{seen:?}");
}

/// the folder of a file it was given lists the files it opens
/// itself, which it may then read; a folder it was given, all of it;
/// anywhere else, nothing.
#[test]
fn the_folder_of_a_file_it_was_given_is_listed() {
    let dir = tempfile::tempdir().unwrap();
    for (name, bytes) in [("a.png", 3), ("b.png", 5), ("notes.txt", 2)] {
        std::fs::write(dir.path().join(name), vec![0u8; bytes]).unwrap();
    }
    std::fs::create_dir(dir.path().join("sub.png")).unwrap();
    let (dir_text, next) = (dir.path().display().to_string(), dir.path().join("b.png").display().to_string());
    let mut ext = start_tuned(Permissions::default(), &[("hello.dir", dir_text.as_str()), ("hello.next", next.as_str())], |spec| spec.opens = vec!["*.png".into()]);
    let (_, seen) = ext.run("hello.list");
    assert!(seen.iter().any(|e| matches!(e, Event::Said(_, s) if s.starts_with("list: ") && s.contains("not a folder Fenix gave it"))), "{seen:?}");
    assert!(seen.iter().any(|e| matches!(e, Event::Said(_, s) if s.starts_with("next read: Err("))), "{seen:?}");
    let (_, seen) = ext.call(Call::Deliver(Notice::Opened(dir.path().join("a.png").display().to_string())));
    assert!(said(&seen, Level::Info, "opened a.png: Ok(3)"), "{seen:?}");
    let (_, seen) = ext.run("hello.list");
    assert!(said(&seen, Level::Info, "listed: a.png 3, b.png 5") && said(&seen, Level::Info, "next read: Ok(5)"), "{seen:?}");
    // A folder it was given: all its files.
    let mut ext = start_with(Permissions::default(), &[("hello.dir", dir_text.as_str()), ("hello.next", next.as_str())]);
    assert_eq!(ext.call(Call::Grant(dir.path().to_path_buf())).0, Ok(Reply::Done));
    let (_, seen) = ext.run("hello.list");
    assert!(said(&seen, Level::Info, "listed: a.png 3, b.png 5, notes.txt 2"), "{seen:?}");
}

/// a WAV of its own played, with `sound = true`; nothing else.
#[test]
fn a_sound_is_played_with_its_permission() {
    let mut ext = start();
    let (_, seen) = ext.run("hello.sound");
    assert!(seen.iter().any(|e| matches!(e, Event::Said(_, s) if s.starts_with("sound: Err(") && s.contains("permissions.sound"))), "{seen:?}");
    let mut ext = start_with(Permissions { sound: true, ..Permissions::default() }, &[]);
    let (_, seen) = ext.run("hello.sound");
    assert!(said(&seen, Level::Info, "sound: Ok(())"), "{seen:?}");
    assert!(seen.iter().any(|e| matches!(e, Event::Asked(Request::PlaySound(wav)) if wav.starts_with(b"RIFF") && wav.len() == 44)), "its bytes, read as they were checked: {seen:?}");
    assert!(seen.iter().any(|e| matches!(e, Event::Said(_, s) if s.starts_with("not a wav: Err(") && s.contains("WAV"))), "{seen:?}");
    assert!(seen.iter().any(|e| matches!(e, Event::Said(_, s) if s.starts_with("not its own: Err("))), "{seen:?}");
}

/// A link in its folders that leads out of them (WASI lets it make one
/// with `..`; Fenix's own reads would follow it) isn't followed:
/// `file-read` and `image-file` refuse it, and `open-file` opens it for
/// you, sealed, as any file it may not reach.
#[cfg(unix)]
#[test]
fn a_link_out_of_its_folders_is_not_followed() {
    let outside = tempfile::tempdir().unwrap();
    std::fs::copy(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/guests/hello-sheet.png"), outside.path().join("secret.png")).unwrap();
    let reads = Permissions { editor: fenix_extensions::manifest::Editor::Read, ..Permissions::default() };
    let mut settings = None;
    let mut ext = start_tuned(reads, &[("hello.file", "/data/out/secret.png")], |spec| settings = Some(spec.settings.clone()));
    let data = ext.dir.path().join("data");
    std::os::unix::fs::symlink(outside.path(), data.join("out")).unwrap();
    std::fs::copy(outside.path().join("secret.png"), data.join("mine.png")).unwrap();
    std::os::unix::fs::symlink("mine.png", data.join("near.png")).unwrap();
    let (_, seen) = ext.run("hello.read");
    assert!(warned(&seen, "not somewhere it may read"), "{seen:?}");
    let set = |key: &str, value: String| settings.as_ref().unwrap().write().unwrap().insert(key.to_string(), value);
    set("hello.file", data.join("out/secret.png").display().to_string());
    let (_, seen) = ext.run("hello.escape");
    assert!(seen.iter().any(|e| matches!(e, Event::Said(_, s) if s.starts_with("escape image: Err("))), "{seen:?}");
    assert!(seen.iter().any(|e| matches!(e, Event::Asked(Request::OpenFile { sealed: true, .. }))), "{seen:?}");
    // A link that stays in its folder is followed.
    set("hello.file", "/data/near.png".into());
    let (_, seen) = ext.run("hello.read");
    assert!(seen.iter().any(|e| matches!(e, Event::Said(_, s) if s.starts_with("read ") && s.ends_with(" bytes"))), "{seen:?}");
}


/// a page laid out for the pane's height too, with what its canvas
/// has besides; more keys and the mouse come to it.
#[test]
fn a_styled_page_is_laid_out_for_its_pane_and_hears_more_keys_and_the_mouse() {
    let mut ext = start();
    let (_, seen) = ext.run("hello.styled");
    let Some(page) = seen.iter().find_map(|e| if let Event::Asked(Request::PageOpen { page, kind, .. }) = e { (kind == "styled").then_some(*page) } else { None }) else { panic!("{seen:?}") };
    let styled = laid_out(ext.call(Call::Render { page, width: 90, height: 33 }).0);
    assert!(styled.canvas.as_ref().unwrap().text.starts_with("日本 ok 90x33"), "{:?}", styled.canvas);
    let extra = &styled;
    assert!(extra.more_keys && extra.mouse);
    assert_eq!(extra.styles[0], CellStyle { cells: fenix_wasm::host::Cells { line: 0, start: 3, end: 5 }, background: Some(Shade::Selection), underline: Some(fenix_wasm::host::Role::Accent) });
    assert_eq!(extra.vertical_rules, [VerticalRule { column: 9, top: 0, bottom: 2 }]);
    assert_eq!(extra.focuses.len(), 1);
    let mods = KeyMods { shift: true, ctrl: false, alt: false, meta: false };
    let (_, seen) = ext.call(Call::KeyPress { page, key: KeyPress { key: SurfaceKey::Named(NamedKey::Left), mods } });
    assert!(seen.iter().any(|e| matches!(e, Event::Said(_, s) if s.starts_with(&format!("key press on {page}:")) && s.contains("Left") && s.contains("shift: true"))), "{seen:?}");
    let event = MouseEvent { line: 1, column: 2, cell: 2, action: MouseAction::Press(MousePress { button: MouseButton::Left, clicks: 2 }), mods };
    let (_, seen) = ext.call(Call::Mouse { page, event });
    assert!(seen.iter().any(|e| matches!(e, Event::Said(_, s) if s.starts_with(&format!("mouse on {page}:")) && s.contains("clicks: 2"))), "{seen:?}");
    // Another page of its: as `page-render` lays it out, nothing besides.
    let (_, seen) = ext.run("hello.page");
    let Some(plain) = seen.iter().find_map(|e| if let Event::Asked(Request::PageOpen { page, .. }) = e { Some(*page) } else { None }) else { panic!() };
    let styled = laid_out(ext.call(Call::Render { page: plain, width: 80, height: 24 }).0);
    assert!(styled.styles.is_empty() && !styled.mouse && styled.lines[0].spans[1].text == "  80 wide");
}

/// decorations of a file's buffer need `editor = "read"`; of its
/// own scratch buffer, nothing; of another's, and too many, are refused.
#[test]
fn decorations_are_set_on_a_buffer_it_may_reach() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("a.rs");
    std::fs::write(&file, "let x = 1;\n").unwrap();
    let file_text = file.display().to_string();
    let mut ext = start_with(Permissions::default(), &[("hello.file", file_text.as_str())]);
    let (_, seen) = ext.run("hello.decorate");
    assert!(seen.iter().any(|e| matches!(e, Event::Said(_, s) if s.starts_with("decorate: Err(") && s.contains("permissions.editor"))), "{seen:?}");
    assert!(said(&seen, Level::Info, "decorate scratch: Ok(())"), "{seen:?}");
    assert!(seen.iter().any(|e| matches!(e, Event::Asked(Request::ScratchDecorations { decorations, .. }) if decorations.len() == 1)), "{seen:?}");
    assert!(seen.iter().any(|e| matches!(e, Event::Said(_, s) if s.starts_with("not its own: Err("))), "{seen:?}");
    assert!(seen.iter().any(|e| matches!(e, Event::Said(_, s) if s.starts_with("too many: Err(") && s.contains("10000"))), "{seen:?}");
    let reads = Permissions { editor: fenix_extensions::manifest::Editor::Read, ..Permissions::default() };
    let mut ext = start_with(reads, &[("hello.file", file_text.as_str())]);
    let (_, seen) = ext.run("hello.decorate");
    assert!(said(&seen, Level::Info, "decorate: Ok(())"), "{seen:?}");
    let Some((path, open_only, decorations)) = seen.iter().find_map(|e| if let Event::Asked(Request::FileDecorations { path, open_only, decorations }) = e { Some((path.clone(), *open_only, decorations.clone())) } else { None }) else { panic!("{seen:?}") };
    assert_eq!((path, open_only, decorations.len()), (file.clone(), true, 5), "a full path not its own: only a buffer you have open");
    assert_eq!((&decorations[2].look, decorations[2].id), (&DecorationLook::Hide("••••".into()), None));
}

/// a buffer's change undone and done again, with `editor =
/// "write"`; and the theme's palette.
#[test]
fn a_buffer_is_undone_and_the_palette_read() {
    let file = std::env::temp_dir().join("fenix-undo.txt").display().to_string();
    let mut ext = start_with(Permissions::default(), &[("hello.file", file.as_str())]);
    let (_, seen) = ext.run("hello.undo");
    assert!(seen.iter().any(|e| matches!(e, Event::Said(_, s) if s.starts_with("undo: Err(") && s.contains("permissions.editor"))), "{seen:?}");
    let writes = Permissions { editor: fenix_extensions::manifest::Editor::Write, ..Permissions::default() };
    let mut ext = start_with(writes, &[("hello.file", file.as_str())]);
    let (_, seen) = ext.run("hello.undo");
    assert!(said(&seen, Level::Info, "undo: Ok(())") && said(&seen, Level::Info, "redo: Ok(())"), "{seen:?}");
    assert!(seen.iter().any(|e| matches!(e, Event::Asked(Request::BufferUndo { redo: false, .. }))) && seen.iter().any(|e| matches!(e, Event::Asked(Request::BufferUndo { redo: true, .. }))), "{seen:?}");
    let mut ext = start_tuned(Permissions::default(), &[], |spec| *spec.palette.write().unwrap() = vec![(1, 2, 3); 8]);
    let (_, seen) = ext.run("hello.palette");
    assert!(said(&seen, Level::Info, "palette: 8"), "{seen:?}");
}


/// a language pack's debug adapter and formatter, held to its
/// manifest as its language server is; a linter's problems in a file it
/// may reach.
#[test]
fn a_packs_adapter_formatter_and_problems_are_asked_for() {
    let mut ext = start_with(Permissions { process: vec!["sort".into()], ..Permissions::default() }, &[]);
    let (_, seen) = ext.run("hello.pack");
    assert!(said(&seen, Level::Info, "adapter: Ok(())") && said(&seen, Level::Info, "formatter: Ok(())") && said(&seen, Level::Info, "no formatter: Ok(())"), "{seen:?}");
    assert!(seen.iter().any(|e| matches!(e, Event::Asked(Request::DebugAdapter { languages, program: Some(((p, args, _), _)) }) if languages == &["python"] && p == "sort" && args == &["-r"])), "{seen:?}");
    assert!(seen.iter().any(|e| matches!(e, Event::Asked(Request::Formatter { program: Some((_, args, _)), .. }) if args == &["${file}"])), "Fenix fills in ${{file}}: {seen:?}");
    assert!(seen.iter().any(|e| matches!(e, Event::Asked(Request::Formatter { program: None, .. }))), "{seen:?}");
    assert!(seen.iter().any(|e| matches!(e, Event::Said(_, s) if s.starts_with("not its own: Err(") && s.contains("powershell"))), "{seen:?}");

    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("a.py");
    std::fs::write(&file, "x = 1\n").unwrap();
    let file_text = file.display().to_string();
    let mut ext = start_with(Permissions::default(), &[("hello.file", file_text.as_str())]);
    let (_, seen) = ext.run("hello.problems");
    assert!(seen.iter().any(|e| matches!(e, Event::Said(_, s) if s.starts_with("problems: Err(") && s.contains("editor"))), "a full path not its own needs editor = read: {seen:?}");
    assert!(seen.iter().any(|e| matches!(e, Event::Said(_, s) if s.starts_with("in its project: Err("))), "no project here: {seen:?}");
    let reads = Permissions { editor: fenix_extensions::manifest::Editor::Read, filesystem: fenix_extensions::manifest::Filesystem::ProjectRead, ..Permissions::default() };
    let mut ext = start_tuned(reads, &[("hello.file", file_text.as_str())], |spec| spec.project = Some(dir.path().to_path_buf()));
    let (_, seen) = ext.run("hello.problems");
    assert!(said(&seen, Level::Info, "problems: Ok(())") && said(&seen, Level::Info, "in its project: Ok(())"), "{seen:?}");
    let Some((path, open_only, problems)) = seen.iter().find_map(|e| if let Event::Asked(Request::Diagnostics { path, open_only, problems, .. }) = e { (!problems.is_empty()).then(|| (path.clone(), *open_only, problems.clone())) } else { None }) else { panic!("{seen:?}") };
    assert_eq!((path, open_only, problems[0].code.as_deref()), (file.clone(), false, Some("W1")), "in its project: any file of it");
    assert!(seen.iter().any(|e| matches!(e, Event::Asked(Request::Diagnostics { path, problems, .. }) if path == &dir.path().join("a.py") && problems.is_empty())), "{seen:?}");
    assert!(seen.iter().any(|e| matches!(e, Event::Said(_, s) if s.starts_with("too many: Err(") && s.contains("10000"))), "{seen:?}");
}

/// a program's stdin closed, it reads to its end -- `sort` writes
/// only then.
#[test]
fn a_programs_stdin_is_closed_so_it_reads_to_its_end() {
    let mut ext = start_with(Permissions { process: vec!["sort".into()], ..Permissions::default() }, &[]);
    let (result, seen) = ext.run("hello.stdin");
    assert_eq!(result, Ok(()), "{seen:?}");
    assert!(seen.iter().any(|e| matches!(e, Event::Said(_, s) if s.starts_with("after close: Err("))), "{seen:?}");
    let lines = ext.logged_until(|l| l.starts_with("process 1 ended"));
    let said: Vec<&String> = lines.iter().filter(|l| l.starts_with("process 1 said: ")).collect();
    assert_eq!(said, ["process 1 said: apple", "process 1 said: pear"], "{lines:?}");
    assert_eq!(lines.last().map(String::as_str), Some("process 1 ended: Some(0)"));
}

/// a scratch buffer and text opened beside, its width kept
/// sensible.
#[test]
fn scratch_buffers_and_text_open_beside() {
    let mut ext = start();
    let (_, seen) = ext.run("hello.beside");
    assert!(seen.iter().any(|e| matches!(e, Event::Asked(Request::ScratchOpen { title, beside: Some(w), .. }) if title == "Result" && (*w - 0.4).abs() < 1e-6)), "{seen:?}");
    assert!(seen.iter().any(|e| matches!(e, Event::Asked(Request::OpenText { title, beside: Some(w), .. }) if title == "Raw" && *w == 0.9)), "{seen:?}");
}

/// its tools folder, when its manifest names a program there: what
/// it writes there lands in Fenix's tools folder, is made to run, found
/// and run by its path. Downloads go there or to `/data` only, to hosts it
/// may reach.
#[test]
fn its_tools_folder_holds_the_programs_it_installs() {
    let tools = tempfile::tempdir().unwrap();
    let permissions = Permissions { process: vec!["/tools/*".into(), "hello-tool".into()], network: vec!["x.example".into()], ..Permissions::default() };
    let mut ext = start_tuned(permissions, &[], |spec| spec.tools_dir = Some(tools.path().to_path_buf()));
    let (result, seen) = ext.run("hello.tools");
    assert_eq!(result, Ok(()), "{seen:?}");
    let own = tools.path().join("test.hello");
    assert!(own.join("hello-tool/bin/hello-tool").is_file(), "under <tools>/<id>");
    assert!(said(&seen, Level::Info, "executable: Ok(())"), "{seen:?}");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(own.join("hello-tool/bin/hello-tool")).unwrap().permissions().mode();
        assert_eq!(mode & 0o100, 0o100, "{mode:o}");
    }
    assert!(seen.iter().any(|e| matches!(e, Event::Said(_, s) if s.starts_with("not in tools: Err("))), "{seen:?}");
    assert!(said(&seen, Level::Info, "found in tools: true"), "{seen:?}");
    assert!(said(&seen, Level::Info, "adapter by path: Ok(())"), "{seen:?}");
    let Some((program, args)) = seen.iter().find_map(|e| if let Event::Asked(Request::DebugAdapter { program: Some(((p, a, _), _)), .. }) = e { Some((p.clone(), a.clone())) } else { None }) else { panic!("{seen:?}") };
    assert_eq!(PathBuf::from(program), own.join("hello-tool/bin/hello-tool"));
    assert_eq!(args, [format!("{}/x", own.display())], "${{tools}} filled in");
    assert!(seen.iter().any(|e| matches!(e, Event::Said(_, s) if s.starts_with("download outside: Err(") && s.contains("/tools or /data"))), "{seen:?}");
    assert!(seen.iter().any(|e| matches!(e, Event::Said(_, s) if s.starts_with("download bad sum: Err(") && s.contains("64 hex"))), "{seen:?}");
    assert!(seen.iter().any(|e| matches!(e, Event::Said(_, s) if s.starts_with("download elsewhere: Err(") && s.contains("permissions.network"))), "{seen:?}");
    // Without a program there in its manifest, there's no /tools.
    let mut ext = start_tuned(Permissions { process: vec!["hello-tool".into()], ..Permissions::default() }, &[], |spec| spec.tools_dir = Some(tools.path().join("other")));
    let (result, _) = ext.run("hello.tools");
    assert!(result.is_err(), "{result:?}");
    assert!(!tools.path().join("other").exists());
}


/// a link it makes through WASI (`..` is let through) leads out
/// of its folder, but neither WASI nor Fenix's own reads follow it.
#[cfg(unix)]
#[test]
fn a_link_it_makes_out_of_its_folder_is_followed_by_nothing() {
    let mut ext = start();
    std::fs::write(ext.dir.path().join("secret.txt"), "s3cret").unwrap();
    let (_, seen) = ext.run("hello.link");
    assert!(said(&seen, Level::Info, "link made: true"), "WASI lets it make one: {seen:?}");
    assert!(ext.dir.path().join("data/up").symlink_metadata().is_ok_and(|m| m.file_type().is_symlink()));
    assert!(seen.iter().any(|e| matches!(e, Event::Said(_, s) if s.starts_with("through it: Err(") && s.contains("not somewhere it may read"))), "{seen:?}");
    assert!(seen.iter().any(|e| matches!(e, Event::Said(_, s) if s.starts_with("by wasi: Err("))), "{seen:?}");
}

/// A server on this machine that gives each connection, in turn, the
/// next of `answers` (a whole HTTP response).
fn serve(answers: Vec<String>) -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        use std::io::{Read, Write};
        for answer in answers {
            let Ok((mut stream, _)) = listener.accept() else { return };
            let mut buf = [0u8; 4096];
            let _ = stream.read(&mut buf);
            let _ = stream.write_all(answer.as_bytes());
        }
    });
    port
}

fn ok(body: &str) -> String {
    format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())
}

/// `http-send`: its answer, where it ended up and how long it
/// took, to `http-event`; redirects followed only to hosts it may reach;
/// streamed, its body in parts and its end; given up, nothing more.
#[test]
fn http_send_answers_with_its_timing_and_streams() {
    let port = serve(vec!["HTTP/1.1 302 Found\r\nLocation: /final\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string(), ok("hello")]);
    let url = format!("http://127.0.0.1:{port}/first");
    let permissions = || Permissions { network: vec!["127.0.0.1".into()], ..Permissions::default() };
    let mut ext = start_with(permissions(), &[("hello.url", &url)]);
    let (result, seen) = ext.run("hello.send");
    assert_eq!(result, Ok(()), "{seen:?}");
    assert!(seen.iter().any(|e| matches!(e, Event::Said(_, s) if s.starts_with("elsewhere: Err(") && s.contains("permissions.network"))), "{seen:?}");
    assert!(seen.iter().any(|e| matches!(e, Event::Said(_, s) if s.starts_with("by a proxy elsewhere: Err(") && s.contains("permissions.network"))), "{seen:?}");
    let lines = ext.logged_until(|l| l.starts_with("http 1"));
    assert_eq!(lines.last().map(String::as_str), Some("http 1: 200 final hello dns true connect false tls false total true"), "followed, timed: {lines:?}");

    // A redirect to a host it may not reach isn't followed.
    let port = serve(vec!["HTTP/1.1 301 Moved\r\nLocation: http://elsewhere.example/x\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string()]);
    let url = format!("http://127.0.0.1:{port}/first");
    let mut ext = start_with(permissions(), &[("hello.url", &url)]);
    let _ = ext.run("hello.send");
    let lines = ext.logged_until(|l| l.starts_with("http 1"));
    assert!(lines.last().is_some_and(|l| l.starts_with("http 1 failed: redirected to http://elsewhere.example/x") && l.contains("permissions.network")), "{lines:?}");

    // Not followed, when it says so.
    let port = serve(vec!["HTTP/1.1 302 Found\r\nLocation: /final\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string()]);
    let url = format!("http://127.0.0.1:{port}/first");
    let mut ext = start_with(permissions(), &[("hello.url", &url), ("hello.stay", "yes")]);
    let _ = ext.run("hello.send");
    let lines = ext.logged_until(|l| l.starts_with("http 1"));
    assert!(lines.last().is_some_and(|l| l.starts_with("http 1: 302 first ")), "{lines:?}");

    // Streamed.
    let port = serve(vec![ok("a streamed body")]);
    let url = format!("http://127.0.0.1:{port}/events");
    let mut ext = start_with(Permissions { network: vec!["*".into()], ..Permissions::default() }, &[("hello.url", &url), ("hello.stream", "yes")]);
    let _ = ext.run("hello.send");
    let lines = ext.logged_until(|l| l.starts_with("http 1 end"));
    assert!(lines.contains(&"http 1: 200 events  dns true connect false tls false total true".to_string()), "its head, no body: {lines:?}");
    let body: String = lines.iter().filter_map(|l| l.strip_prefix("http 1 chunk: ")).collect();
    assert_eq!(body, "a streamed body");
    assert_eq!(lines.last().map(String::as_str), Some("http 1 end: None"));

    // Given up: nothing comes.
    let port = serve(vec![ok("late")]);
    let url = format!("http://127.0.0.1:{port}/x");
    let mut ext = start_with(permissions(), &[("hello.url", &url)]);
    let (_, seen) = ext.run("hello.cancel");
    assert!(said(&seen, Level::Info, "cancelled 1"), "{seen:?}");
    let (_, seen) = ext.run("hello.greet");
    std::thread::sleep(Duration::from_millis(300));
    let (_, later) = ext.run("hello.greet");
    assert!(!seen.iter().chain(&later).any(|e| matches!(e, Event::Said(_, s) if s.starts_with("http 1"))), "{later:?}");
}

/// A TLS server on this machine with `guests/localhost.pem`: it echoes
/// four bytes. With `starttls`, it first takes `STARTTLS\n` and answers
/// `OK\n` in plain text, as PostgreSQL's does in its own words.
fn tls_server(starttls: bool) -> u16 {
    use std::io::{BufRead, Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let config = tls_config();
    std::thread::spawn(move || {
        let Ok((mut tcp, _)) = listener.accept() else { return };
        if starttls {
            let mut line = String::new();
            let mut reading = std::io::BufReader::new(tcp.try_clone().unwrap());
            if reading.read_line(&mut line).is_err() || line != "STARTTLS\n" {
                return;
            }
            let _ = tcp.write_all(b"OK\n");
        }
        let conn = rustls::ServerConnection::new(Arc::new(config)).unwrap();
        let mut tls = rustls::StreamOwned::new(conn, tcp);
        let mut buf = [0u8; 4];
        if tls.read_exact(&mut buf).is_ok() {
            let _ = tls.write_all(&buf).and_then(|_| tls.flush());
        }
        std::thread::sleep(Duration::from_millis(500));
    });
    port
}

/// `http-send` over https times its connection and its TLS too;
/// a certificate that doesn't check out is taken only when it says.
#[test]
fn http_send_over_tls_times_each_part() {
    let port = https_server();
    let url = format!("https://127.0.0.1:{port}/secure");
    let permissions = || Permissions { network: vec!["127.0.0.1".into()], ..Permissions::default() };
    let mut ext = start_with(permissions(), &[("hello.url", &url), ("hello.any", "yes")]);
    let _ = ext.run("hello.send");
    let lines = ext.logged_until(|l| l.starts_with("http 1"));
    assert_eq!(lines.last().map(String::as_str), Some("http 1: 200 secure tls dns true connect true tls true total true"), "{lines:?}");
    let port = https_server();
    let url = format!("https://127.0.0.1:{port}/secure");
    let mut ext = start_with(permissions(), &[("hello.url", &url)]);
    let _ = ext.run("hello.send");
    let lines = ext.logged_until(|l| l.starts_with("http 1"));
    assert!(lines.last().is_some_and(|l| l.starts_with("http 1 failed: ")), "self-signed, not taken: {lines:?}");
}

/// The server `tls_server` is, answering one HTTP request with `tls`.
fn https_server() -> u16 {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let config = tls_config();
    std::thread::spawn(move || {
        let Ok((tcp, _)) = listener.accept() else { return };
        let mut tls = rustls::StreamOwned::new(rustls::ServerConnection::new(Arc::new(config)).unwrap(), tcp);
        let mut buf = [0u8; 4096];
        if tls.read(&mut buf).is_ok() {
            let _ = tls.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\nConnection: close\r\n\r\ntls").and_then(|_| tls.flush());
            tls.conn.send_close_notify();
            let _ = tls.flush();
        }
    });
    port
}

/// A TLS server's set-up, with `guests/localhost.pem`.
fn tls_config() -> rustls::ServerConfig {
    let guests = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/guests");
    let certs = rustls_pemfile::certs(&mut std::fs::read(guests.join("localhost.pem")).unwrap().as_slice()).collect::<Result<Vec<_>, _>>().unwrap();
    let key = rustls_pemfile::private_key(&mut std::fs::read(guests.join("localhost.key")).unwrap().as_slice()).unwrap().unwrap();
    rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider())).with_safe_default_protocol_versions().unwrap().with_no_client_auth().with_single_cert(certs, key).unwrap()
}

/// TLS on a socket -- connected with it, or started on a socket
/// already talking (STARTTLS) -- with a certificate pinned, or any taken;
/// one the system doesn't vouch for is refused otherwise.
#[test]
fn a_socket_talks_tls_from_the_start_or_from_starttls() {
    let cert = std::fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/guests/localhost.pem")).unwrap();
    let sockets = || Permissions { sockets: vec!["${setting:hello.echo}".into()], ..Permissions::default() };
    let address = format!("127.0.0.1:{}", tls_server(false));
    let mut ext = start_with(sockets(), &[("hello.echo", &address), ("hello.cert", &cert)]);
    assert_eq!(ext.run("hello.tls").0, Ok(()));
    let lines = ext.logged_until(|l| l.starts_with("socket 1 said") || l.starts_with("socket 1 closed"));
    assert!(lines.contains(&"socket 1 connected: Ok(())".to_string()), "{lines:?}");
    assert_eq!(lines.last().map(String::as_str), Some("socket 1 said: ping"), "through TLS");

    let address = format!("127.0.0.1:{}", tls_server(false));
    let mut ext = start_with(sockets(), &[("hello.echo", &address)]);
    let _ = ext.run("hello.tls");
    let lines = ext.logged_until(|l| l.starts_with("socket 1 connected"));
    assert!(lines.last().is_some_and(|l| l.starts_with("socket 1 connected: Err(") && l.contains("TLS")), "not one the system vouches for: {lines:?}");

    let address = format!("127.0.0.1:{}", tls_server(false));
    let mut ext = start_with(sockets(), &[("hello.echo", &address), ("hello.any", "yes")]);
    let _ = ext.run("hello.tls");
    let lines = ext.logged_until(|l| l.starts_with("socket 1 said") || l.starts_with("socket 1 closed"));
    assert_eq!(lines.last().map(String::as_str), Some("socket 1 said: ping"), "any taken: {lines:?}");

    let address = format!("127.0.0.1:{}", tls_server(true));
    let mut ext = start_with(sockets(), &[("hello.echo", &address), ("hello.cert", &cert)]);
    let (result, seen) = ext.run("hello.starttls");
    assert_eq!(result, Ok(()), "{seen:?}");
    assert!(seen.iter().any(|e| matches!(e, Event::Said(_, s) if s.starts_with("not tcp: Err("))), "{seen:?}");
    let lines = ext.logged_until(|l| l.starts_with("socket 1 said: ping") || l.starts_with("socket 1 closed"));
    assert!(lines.contains(&"socket 1 said: OK\n".to_string()) && lines.contains(&"start tls: Ok(())".to_string()), "{lines:?}");
    assert_eq!(lines.iter().filter(|l| *l == "socket 1 connected: Ok(())").count(), 2, "connected, then with TLS: {lines:?}");
    assert_eq!(lines.last().map(String::as_str), Some("socket 1 said: ping"));
}

/// a Unix socket by a name its manifest lists, a socket as a TCP
/// one is; one it doesn't list is refused.
#[cfg(unix)]
#[test]
fn a_local_socket_is_reached_by_a_name_it_lists() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("discord-ipc-0");
    let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
    std::thread::spawn(move || {
        use std::io::{Read, Write};
        if let Ok((mut stream, _)) = listener.accept() {
            let mut buf = [0u8; 64];
            if let Ok(n) = stream.read(&mut buf) {
                let _ = stream.write_all(&buf[..n]);
            }
        }
    });
    let name = path.display().to_string();
    let mut ext = start_with(Permissions { ipc: vec!["${setting:hello.ipc}".into()], ..Permissions::default() }, &[("hello.ipc", &name)]);
    let (result, seen) = ext.run("hello.ipc");
    assert_eq!(result, Ok(()), "{seen:?}");
    assert!(seen.iter().any(|e| matches!(e, Event::Said(_, s) if s.starts_with("not listed: Err(") && s.contains("permissions.ipc"))), "{seen:?}");
    let lines = ext.logged_until(|l| l.starts_with("socket 1 said"));
    assert!(lines.contains(&"socket 1 connected: Ok(())".to_string()), "{lines:?}");
    assert_eq!(lines.last().map(String::as_str), Some("socket 1 said: ping"));
    let mut none = start_with(Permissions::default(), &[("hello.ipc", &name)]);
    assert!(none.run("hello.ipc").0.unwrap_err().contains("permissions.ipc"));
}

/// an adapter over a socket, started (its port Fenix's to pick)
/// or already running at a port it may reach; rules for decorating files,
/// checked; the file a page shows; the machine; a file saved, by its
/// path.
#[test]
fn adapters_on_a_port_rules_a_pages_file_and_saves_are_asked_for() {
    let permissions = Permissions { process: vec!["sort".into()], sockets: vec!["localhost:4711".into()], editor: fenix_extensions::manifest::Editor::Read, ..Permissions::default() };
    let mut ext = start_with(permissions, &[("hello.file", "/data/a.png")]);
    let (_, seen) = ext.run("hello.adapter-port");
    assert!(said(&seen, Level::Info, "spawned: Ok(())") && said(&seen, Level::Info, "running: Ok(())"), "{seen:?}");
    assert!(seen.iter().any(|e| matches!(e, Event::Said(_, s) if s.starts_with("nowhere: Err("))), "{seen:?}");
    assert!(seen.iter().any(|e| matches!(e, Event::Asked(Request::DebugAdapterSocket { program: Some(((p, args, _), _)), port: 0, .. }) if p == "sort" && args == &["--port", "${port}"])), "{seen:?}");
    assert!(seen.iter().any(|e| matches!(e, Event::Asked(Request::DebugAdapterSocket { program: None, port: 4711, .. }))), "{seen:?}");
    let (_, seen) = ext.run("hello.rules");
    assert!(said(&seen, Level::Info, "rules: Ok(())"), "{seen:?}");
    assert!(seen.iter().any(|e| matches!(e, Event::Said(_, s) if s.starts_with("bad pattern: Err("))) && seen.iter().any(|e| matches!(e, Event::Said(_, s) if s.starts_with("no such group: Err("))), "{seen:?}");
    assert!(seen.iter().any(|e| matches!(e, Event::Asked(Request::DecorationRules(rules)) if rules.len() == 1 && rules[0].group == 1)), "{seen:?}");
    let (_, seen) = ext.run("hello.page-file");
    assert!(said(&seen, Level::Info, "page file: Ok(())") && said(&seen, Level::Info, "none: Ok(())"), "{seen:?}");
    assert!(seen.iter().any(|e| matches!(e, Event::Said(_, s) if s.starts_with("not its own: Err("))), "no project here: {seen:?}");
    assert!(seen.iter().any(|e| matches!(e, Event::Asked(Request::PageFile { path: Some(p), .. }) if p.ends_with("a.png"))), "{seen:?}");
    let (_, seen) = ext.run("hello.platform");
    let env = if cfg!(target_env = "msvc") { "msvc" } else if cfg!(target_env = "musl") { "musl" } else if cfg!(target_env = "gnu") { "gnu" } else { "" };
    assert!(said(&seen, Level::Info, &format!("platform: {} {} {env}", std::env::consts::OS, std::env::consts::ARCH)), "{seen:?}");
    let (_, seen) = ext.call(Call::FileSaved("/home/me/a.rs".into()));
    assert!(said(&seen, Level::Info, "file saved: /home/me/a.rs"), "{seen:?}");
    // Without the permissions they need.
    let mut ext = start_with(Permissions { process: vec!["sort".into()], ..Permissions::default() }, &[]);
    let (_, seen) = ext.run("hello.adapter-port");
    assert!(seen.iter().any(|e| matches!(e, Event::Said(_, s) if s.starts_with("running: Err(") && s.contains("permissions.sockets"))), "{seen:?}");
    let (_, seen) = ext.run("hello.rules");
    assert!(seen.iter().any(|e| matches!(e, Event::Said(_, s) if s.starts_with("rules: Err(") && s.contains("permissions.editor"))), "{seen:?}");
}


/// a buffer's language set -- its own scratch buffer's, or with
/// `editor = "read"` a file's; tagged decorations; and what Fenix tells it
/// of views and clicks, and its page's colours.
#[test]
fn a_buffer_is_given_a_language_and_clickable_decorations() {
    let mut ext = start();
    let (_, seen) = ext.run("hello.language");
    assert!(said(&seen, Level::Info, "language: Ok(())") && said(&seen, Level::Info, "by its name again: Ok(())"), "{seen:?}");
    assert!(seen.iter().any(|e| matches!(e, Event::Asked(Request::ScratchLanguage { language: Some(l), .. }) if l == "json")), "{seen:?}");
    assert!(seen.iter().any(|e| matches!(e, Event::Asked(Request::ScratchLanguage { language: None, .. }))), "{seen:?}");
    assert!(seen.iter().any(|e| matches!(e, Event::Said(_, s) if s.starts_with("not its own: Err("))), "{seen:?}");
    assert!(seen.iter().any(|e| matches!(e, Event::Said(_, s) if s.starts_with("a file: Err(") && s.contains("permissions.editor"))), "{seen:?}");
    let (_, seen) = ext.run("hello.tagged");
    assert!(said(&seen, Level::Info, "tagged: Ok(())"), "{seen:?}");
    assert!(seen.iter().any(|e| matches!(e, Event::Asked(Request::ScratchDecorations { decorations, .. }) if decorations.len() == 1 && decorations[0].id == Some(5))), "{seen:?}");
    let scratch = fenix_wasm::host::TextBuffer::Scratch(1);
    let (_, seen) = ext.call(Call::ViewChanged(vec![fenix_wasm::host::TextView { buffer: scratch.clone(), pane: 2, first: 0, last: 9 }]));
    assert!(said(&seen, Level::Info, "view TextBuffer::Scratch(1) 0..=9"), "{seen:?}");
    let (_, seen) = ext.call(Call::DecorationClicked { buffer: scratch, id: 5 });
    assert!(said(&seen, Level::Info, "clicked 5 in TextBuffer::Scratch(1)"), "{seen:?}");
    let (_, seen) = ext.run("hello.styled");
    let Some(page) = seen.iter().find_map(|e| if let Event::Asked(Request::PageOpen { page, kind, .. }) = e { (kind == "styled").then_some(*page) } else { None }) else { panic!("{seen:?}") };
    let page = laid_out(ext.call(Call::Render { page, width: 90, height: 33 }).0);
    assert_eq!(page.colors, [fenix_wasm::host::CellColor { cells: fenix_wasm::host::Cells { line: 1, start: 0, end: 3 }, color: (200, 100, 50) }]);
    assert!(page.mouse, "the rest of the page with them");
}

/// tasks beside the task pane, at most 4 at once; another once one
/// has ended; cancelled and shown by id.
#[test]
fn tasks_run_beside_the_task_pane_four_at_once() {
    let mut ext = start_with(Permissions { process: vec!["cmd".into()], ..Permissions::default() }, &[]);
    let (_, seen) = ext.run("hello.background");
    let asked = seen.iter().filter(|e| matches!(e, Event::Asked(Request::BackgroundTask { .. }))).count();
    assert_eq!(asked, 4, "{seen:?}");
    assert!(seen.iter().any(|e| matches!(e, Event::Said(_, s) if s.starts_with("background 4: Err(") && s.contains("at most 4"))), "{seen:?}");
    assert!(seen.iter().any(|e| matches!(e, Event::Said(_, s) if s.starts_with("denied: Err(") && s.contains("permissions.process"))), "{seen:?}");
    assert!(seen.contains(&Event::Asked(Request::TaskCancel(1))) && seen.contains(&Event::Asked(Request::TaskShow(1))), "{seen:?}");
    assert!(seen.iter().any(|e| matches!(e, Event::Said(_, s) if s.starts_with("show none: Err("))), "{seen:?}");
    // One ends: there's room for one more.
    assert_eq!(ext.call(Call::Deliver(Notice::TaskFinished((1, true)))).0, Ok(Reply::Done));
    let (_, seen) = ext.run("hello.background");
    assert!(seen.iter().any(|e| matches!(e, Event::Said(_, s) if s.starts_with("background 0: Ok("))), "{seen:?}");
    assert!(seen.iter().any(|e| matches!(e, Event::Said(_, s) if s.starts_with("background 1: Err("))), "{seen:?}");
}

/// a language server added beside the others, by name, held to
/// `permissions.process`; and a program a setting of its holds found
/// first.
#[test]
fn a_language_server_is_added_by_name_and_a_program_found_by_setting() {
    let me = std::env::current_exe().unwrap();
    let me_text = me.display().to_string();
    let mut ext = start_with(Permissions { process: vec!["cmd".into()], ..Permissions::default() }, &[("hello.cmd", me_text.as_str())]);
    let (_, seen) = ext.run("hello.add-server");
    assert!(said(&seen, Level::Info, "add: Ok(())") && said(&seen, Level::Info, "gone: Ok(())"), "{seen:?}");
    assert!(seen.iter().any(|e| matches!(e, Event::Asked(Request::LanguageServerAdd { name, languages, program: Some(((program, args, _), _)) }) if name == "ruff" && languages == &["python"] && program == "cmd" && args == &["/c", "ruff"])), "{seen:?}");
    assert!(seen.iter().any(|e| matches!(e, Event::Asked(Request::LanguageServerAdd { program: None, .. }))), "{seen:?}");
    assert!(seen.iter().any(|e| matches!(e, Event::Said(_, s) if s.starts_with("bad name: Err("))), "{seen:?}");
    assert!(seen.iter().any(|e| matches!(e, Event::Said(_, s) if s.starts_with("denied: Err(") && s.contains("permissions.process"))), "{seen:?}");
    let (_, seen) = ext.run("hello.find-setting");
    let name = me.file_name().unwrap().to_string_lossy().into_owned();
    assert!(said(&seen, Level::Info, &format!("found by setting: Some({name:?})")), "{seen:?}");
}

/// a file of its project it opens, or edits when it isn't open, is
/// read as it's checked, and its buffer made from that.
#[test]
fn a_file_it_opens_is_read_as_it_is_checked() {
    let project = tempfile::tempdir().unwrap();
    let file = project.path().join("a.txt");
    std::fs::write(&file, "hello").unwrap();
    let file_text = file.display().to_string();
    let perms = Permissions { filesystem: fenix_extensions::manifest::Filesystem::ProjectRead, editor: fenix_extensions::manifest::Editor::Write, ..Permissions::default() };
    let mut ext = start_tuned(perms, &[("hello.file", file_text.as_str())], |spec| spec.project = Some(project.path().to_path_buf()));
    let call = ext.worker.call(Call::RunCommand("hello.escape".into()), 0);
    let seen = ext.until(|e| {
        if let Event::Asked(Request::BufferText { answer, .. }) = e {
            answer.send(None);
        }
        matches!(e, Event::Finished { call: c, .. } if *c == call)
    });
    assert!(seen.contains(&Event::Asked(Request::OpenFile { path: file.clone(), line: None, sealed: false, text: Some("hello".into()) })), "{seen:?}");
    assert!(seen.iter().any(|e| matches!(e, Event::Asked(Request::BufferEdit { open_only: false, text: Some(t), .. }) if t == "hello")), "{seen:?}");
}


/// Fenix's process id; a server's settings, a JSON object; problems
/// with their fixes; and what Fenix tells it of tasks and debug sessions.
#[test]
fn settings_fixes_and_fenixs_process_id_are_asked_for() {
    let project = tempfile::tempdir().unwrap();
    let file = project.path().join("a.py");
    std::fs::write(&file, "import os\n").unwrap();
    let file_text = file.display().to_string();
    let perms = Permissions { filesystem: fenix_extensions::manifest::Filesystem::ProjectRead, ..Permissions::default() };
    let mut ext = start_tuned(perms, &[("hello.file", file_text.as_str())], |spec| spec.project = Some(project.path().to_path_buf()));
    let (_, seen) = ext.run("hello.pid");
    assert!(said(&seen, Level::Info, &format!("pid: {}", std::process::id())), "{seen:?}");
    let (_, seen) = ext.run("hello.settings");
    assert!(said(&seen, Level::Info, "settings: Ok(())") && said(&seen, Level::Info, "gone: Ok(())"), "{seen:?}");
    assert!(seen.iter().any(|e| matches!(e, Event::Asked(Request::LanguageServerSettings { name, settings: Some(s) }) if name == "typescript" && s["typescript"]["format"]["semicolons"] == "remove")), "{seen:?}");
    assert!(seen.iter().any(|e| matches!(e, Event::Asked(Request::LanguageServerSettings { settings: None, .. }))), "{seen:?}");
    for bad in ["not json: Err(", "not an object: Err(", "bad name: Err("] {
        assert!(seen.iter().any(|e| matches!(e, Event::Said(_, s) if s.starts_with(bad))), "{bad} {seen:?}");
    }
    let (_, seen) = ext.run("hello.fixes");
    assert!(said(&seen, Level::Info, "fixes: Ok(())"), "{seen:?}");
    assert!(seen.iter().any(|e| matches!(e, Event::Said(_, s) if s.starts_with("untitled: Err("))), "{seen:?}");
    assert!(seen.iter().any(|e| matches!(e, Event::Asked(Request::Diagnostics { problems, .. }) if problems.len() == 2 && problems[0].fixes[0].title == "Remove the import" && problems[1].fixes.is_empty())), "{seen:?}");
    let (_, seen) = ext.call(Call::TaskStarted("build".into()));
    assert!(said(&seen, Level::Info, "task started: build"), "{seen:?}");
    let session = fenix_wasm::host::DebugSession { id: 7, name: "main.py".into(), language: "python".into(), parent: Some(3) };
    let (_, seen) = ext.call(Call::DebugChanged(fenix_wasm::host::DebugState::Started(session)));
    assert!(said(&seen, Level::Info, "debug started: 7 main.py python Some(3)"), "{seen:?}");
    let (_, seen) = ext.call(Call::DebugChanged(fenix_wasm::host::DebugState::Paused((7, "breakpoint".into()))));
    assert!(said(&seen, Level::Info, "debug paused: 7 breakpoint"), "{seen:?}");
}

/// launches it gives as it runs, checked, its own paths made
/// Fenix's; its launch variables, never one of Fenix's own; and an
/// adapter's and a server's environment kept.
#[test]
fn launches_and_their_variables_are_given_and_adapters_keep_their_environment() {
    let tools = tempfile::tempdir().unwrap();
    let mut ext = start_tuned(Permissions { process: vec!["sort".into(), "/tools/*".into()], ..Permissions::default() }, &[], |spec| spec.tools_dir = Some(tools.path().to_path_buf()));
    let (_, seen) = ext.run("hello.launches");
    assert!(said(&seen, Level::Info, "launches: Ok(())") && said(&seen, Level::Info, "variable: Ok(())"), "{seen:?}");
    let Some(launches) = seen.iter().find_map(|e| if let Event::Asked(Request::Launches(l)) = e { Some(l.clone()) } else { None }) else { panic!("{seen:?}") };
    assert_eq!(launches.len(), 2);
    let program = std::path::PathBuf::from(launches[0].program.clone().unwrap());
    assert!(program.starts_with(ext.dir.path()) && program.ends_with("app"), "/data/app, as Fenix has it: {program:?}");
    assert!(!launches[0].args[0].contains("${tools}"), "{:?}", launches[0].args);
    assert_eq!(launches[0].pre_launch_task.as_deref(), Some("build app"));
    for bad in ["bad attributes: Err(", "no languages: Err(", "bad variable: Err("] {
        assert!(seen.iter().any(|e| matches!(e, Event::Said(_, s) if s.starts_with(bad))), "{bad} {seen:?}");
    }
    assert!(seen.iter().any(|e| matches!(e, Event::Said(_, s) if s.starts_with("fenix's own: Err(") && s.contains("Fenix's own"))), "{seen:?}");
    assert!(seen.iter().any(|e| matches!(e, Event::Asked(Request::LaunchVariable { name, choices }) if name == "target" && choices[0].value == "build/app")), "{seen:?}");
    let (_, seen) = ext.run("hello.adapter-env");
    assert!(said(&seen, Level::Info, "adapter env: Ok(())") && said(&seen, Level::Info, "server env: Ok(())"), "{seen:?}");
    let env_of = |e: &Event| match e {
        Event::Asked(Request::DebugAdapter { program: Some((_, env)), .. }) | Event::Asked(Request::LanguageServer { program: Some((_, env)), .. }) => Some(env.clone()),
        _ => None,
    };
    let envs: Vec<_> = seen.iter().filter_map(env_of).collect();
    assert_eq!(envs.len(), 2, "{seen:?}");
    assert!(envs.iter().all(|env| env[0].0 == "WHERE" && !env[0].1.contains("${tools}") && env[0].1.ends_with("here")), "{envs:?}");
}


/// secrets by pattern, kept and taken away -- never Fenix's own,
/// nor one its manifest doesn't name -- and a masked prompt.
#[test]
fn secrets_are_read_by_pattern_and_kept() {
    let perms = Permissions { secrets: vec!["hello.secret.*".into(), "gitlab.token".into()], ..Permissions::default() };
    let mut ext = start_tuned(perms, &[], |spec| spec.secrets = Arc::new(|key| (key == "hello.secret.api").then(|| "k3y".to_string())));
    let (_, seen) = ext.run("hello.secrets");
    assert!(said(&seen, Level::Info, "by pattern: Some(\"k3y\")"), "{seen:?}");
    assert!(said(&seen, Level::Info, "kept: Ok(())") && said(&seen, Level::Info, "read back: Some(\"hunter2\")"), "{seen:?}");
    assert!(said(&seen, Level::Info, "gone: Ok(()) None"), "{seen:?}");
    assert!(seen.iter().any(|e| matches!(e, Event::Said(_, s) if s.starts_with("not its: Err(") && s.contains("permissions.secrets"))), "{seen:?}");
    assert!(seen.iter().any(|e| matches!(e, Event::Said(_, s) if s.starts_with("fenix's: Err(") && s.contains("Fenix's own"))), "{seen:?}");
    let kept: Vec<&Request> = seen.iter().filter_map(|e| if let Event::Asked(r @ Request::SecretSet { .. }) = e { Some(r) } else { None }).collect();
    assert_eq!(kept, [&Request::SecretSet { key: "hello.secret.pw".into(), value: Some(fenix_wasm::host::Hidden("hunter2".into())) }, &Request::SecretSet { key: "hello.secret.pw".into(), value: None }]);
    assert!(!format!("{kept:?}").contains("hunter2"), "a secret isn't printed: {kept:?}");
    let (_, seen) = ext.run("hello.prompt-secret");
    assert!(seen.iter().any(|e| matches!(e, Event::Asked(Request::Prompt { label, masked: true, initial, .. }) if label == "Password" && initial.is_empty())), "{seen:?}");
}

/// a port on the loopback listened on, its connections sockets of
/// the extension's own; and only a port its manifest names.
#[test]
fn a_port_is_listened_on_and_its_connections_are_sockets() {
    use std::io::{Read, Write};
    let mut ext = start_with(Permissions { listen: vec!["127.0.0.1:0".into()], ..Permissions::default() }, &[]);
    let (_, seen) = ext.run("hello.listen");
    let listening = seen.iter().find_map(|e| if let Event::Said(_, s) = e { s.strip_prefix("listening: ").map(str::to_string) } else { None }).unwrap_or_else(|| panic!("{seen:?}"));
    let (listener, port) = listening.split_once(' ').unwrap();
    assert!(seen.iter().any(|e| matches!(e, Event::Said(_, s) if s.starts_with("taken port: Err(") && s.contains("permissions.listen"))), "{seen:?}");
    let mut client = std::net::TcpStream::connect(("127.0.0.1", port.parse::<u16>().unwrap())).unwrap();
    client.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    let mut hello = [0u8; 6];
    client.read_exact(&mut hello).unwrap();
    assert_eq!(&hello, b"hello\n", "it wrote to the socket it was given");
    client.write_all(b"GET /callback?code=abc").unwrap();
    let logged = ext.logged_until(|s| s.starts_with("socket") && s.ends_with("said: GET /callback?code=abc"));
    assert!(logged.iter().any(|s| s.starts_with(&format!("accepted on {listener}: ")) && s.ends_with("from 127.0.0.1")), "{logged:?}");
    let mut rest = Vec::new();
    let _ = client.read_to_end(&mut rest);
    assert!(rest.is_empty(), "closed by it");
    let mut none = start_with(Permissions::default(), &[]);
    let (_, seen) = none.run("hello.listen");
    assert!(seen.iter().any(|e| matches!(e, Event::Said(_, s) if s.starts_with("listen: Err(") && s.contains("no port"))), "{seen:?}");
}

/// Fenix's environment variables, those its manifest names, never
/// Fenix's own secrets'; `${data}` in a command; a formatter whose output
/// is kept when it fails; and a project's wrapper (`./gradlew`) found.
#[test]
fn environment_variables_data_a_lenient_formatter_and_a_wrapper() {
    std::env::set_var("HELLO_TEST_VAR", "from Fenix");
    std::env::set_var("HELLO_TEST_OTHER", "by prefix");
    std::env::set_var("FENIX_HELLO_TEST", "a secret");
    let project = tempfile::tempdir().unwrap();
    let wrapper = if cfg!(windows) { "gradlew.bat" } else { "gradlew" };
    std::fs::write(project.path().join(wrapper), "").unwrap();
    let perms = Permissions {
        env: vec!["HELLO_TEST_VAR".into(), "HELLO_TEST_O*".into(), "FENIX_HELLO_TEST".into()],
        process: vec!["sort".into(), "gradlew".into()],
        filesystem: fenix_extensions::manifest::Filesystem::ProjectRead,
        ..Permissions::default()
    };
    let mut ext = start_tuned(perms, &[], |spec| spec.project = Some(project.path().to_path_buf()));
    let (_, seen) = ext.run("hello.env-var");
    assert!(said(&seen, Level::Info, "env: Some(\"from Fenix\")"), "FENIX_ is Fenix's own: {seen:?}");
    assert!(said(&seen, Level::Info, "by prefix: Some(\"by prefix\")") && said(&seen, Level::Info, "not its: None"), "{seen:?}");
    assert!(warned(&seen, "PATH isn't a variable this extension may read"), "{seen:?}");
    let (_, seen) = ext.run("hello.formatter-ex");
    assert!(said(&seen, Level::Info, "formatter: Ok(())"), "{seen:?}");
    let Some((args, keep)) = seen.iter().find_map(|e| if let Event::Asked(Request::Formatter { program: Some((_, args, _)), keep_output, .. }) = e { Some((args.clone(), *keep_output)) } else { None }) else { panic!("{seen:?}") };
    assert!(keep && args[0].starts_with(&ext.dir.path().to_string_lossy().into_owned()) && args[0].ends_with("x"), "${{data}} is its folder: {args:?}");
    let (_, seen) = ext.run("hello.wrapper");
    assert!(said(&seen, Level::Info, &format!("wrapper: Some({wrapper:?})")), "{seen:?}");
}


/// an agent it brings, held to `permissions.process`; Fenix's
/// `agents.ask` asked as a service; and a turn that ended, told.
#[test]
fn an_agent_is_brought_asked_and_heard() {
    let mut ext = start_with(Permissions { process: vec!["cmd".into()], agents: true, ..Permissions::default() }, &[]);
    let (_, seen) = ext.run("hello.agent");
    assert!(said(&seen, Level::Info, "provide: Ok(())") && said(&seen, Level::Info, "gone: Ok(())"), "{seen:?}");
    assert!(seen.iter().any(|e| matches!(e, Event::Asked(Request::AgentProvide { id, program: Some(((program, args, _), _)) }) if id == "hello" && program == "cmd" && args == &["/c", "agent", "--acp"])), "{seen:?}");
    assert!(seen.iter().any(|e| matches!(e, Event::Asked(Request::AgentProvide { program: None, .. }))), "{seen:?}");
    assert!(seen.iter().any(|e| matches!(e, Event::Said(_, s) if s.starts_with("no id: Err("))), "{seen:?}");
    assert!(seen.iter().any(|e| matches!(e, Event::Said(_, s) if s.starts_with("denied: Err(") && s.contains("permissions.process"))), "{seen:?}");
    assert!(seen.iter().any(|e| matches!(e, Event::Asked(Request::Service { name, payload, .. }) if name == "agents.ask" && payload.contains("\"prompt\""))), "{seen:?}");
    let turn = fenix_wasm::host::AgentTurn { agent: "claude".into(), thread: "t1".into(), title: "T".into(), root: "/p".into(), stop_reason: "end_turn".into(), text: "Done.".into() };
    let (_, seen) = ext.call(Call::AgentTurnEnded(turn));
    assert!(said(&seen, Level::Info, "turn ended: claude t1 end_turn Done."), "{seen:?}");
}
