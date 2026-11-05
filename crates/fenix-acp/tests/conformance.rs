//! The conformance suite (plan §8): one set of scenarios, run unchanged
//! against each agent through the hub and a headless host.
//!
//! Against the mock (playing Claude Code's adapter) they always run.
//! Against real agents they run when asked, on a machine where the agent
//! is installed and logged in:
//!
//! ```text
//! FENIX_ACP_LIVE=claude,codex,devin cargo test -p fenix-acp --test conformance -- --ignored --nocapture
//! ```
//!
//! Each agent's command is the catalog's (found on `PATH`), or
//! `FENIX_ACP_<ID>` (`FENIX_ACP_DEVIN="/opt/devin/bin/devin acp"`). A
//! real agent works in a temporary folder, on its own small files, and
//! is told exactly what to do; permission requests are allowed once.
//! The results are what ADR 0002's table is filled from.

use fenix_acp::catalog;
use fenix_acp::driver::{self, Driver};
use fenix_acp::hub::{AgentSpec, ConnState, Hub, Link, ThreadId, ThreadMeta, Turn};
use fenix_acp::protocol::{ContentBlock, ToolKind, ToolStatus};
use fenix_acp::thread::{Entry, NoticeKind};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[derive(Clone)]
struct Target {
    spec: AgentSpec,
    live: bool,
}

impl Target {
    fn mock() -> Self {
        // Its sessions kept on disk, as Claude keeps its own (~/.claude),
        // so they outlive the agent's process.
        let store = std::env::temp_dir().join(format!("fenix-acp-conf-store-{}", std::process::id()));
        let args = ["--profile", "claude", "--store", &store.to_string_lossy()].map(String::from).to_vec();
        Target { spec: AgentSpec { id: "mock".into(), name: "Mock (Claude profile)".into(), program: fenix_acp::client::MOCK_PROGRAM.into(), args, env: vec![] }, live: false }
    }

    fn turn_timeout(&self) -> Duration {
        if self.live {
            Duration::from_secs(300)
        } else {
            Duration::from_secs(20)
        }
    }

    /// The prompt for a scenario: the mock's script, or plain words for a
    /// real agent.
    fn prompt(&self, mock: &str, live: &str) -> Vec<ContentBlock> {
        vec![ContentBlock::text(if self.live { live } else { mock })]
    }
}

fn workdir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("fenix-acp-conf-{}-{name}-{}", std::process::id(), Instant::now().elapsed().as_nanos()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // Some agents insist on a repository.
    let _ = std::process::Command::new("git").arg("init").arg("-q").current_dir(&dir).status();
    dir
}

fn driver_for(target: &Target, root: &Path) -> (Driver, ThreadId) {
    let mut hub = Hub::new("conf", "test");
    hub.set_agents(vec![target.spec.clone()]);
    let id = hub.new_thread(&target.spec.id, root);
    (Driver::new(hub), id)
}

fn idle(id: &ThreadId) -> impl Fn(&Hub) -> bool + '_ {
    move |hub| hub.view(id).is_some_and(|v| matches!(v.link, Link::Live { .. } | Link::ReadOnly(_) | Link::Detached) && v.turn == &Turn::Idle && v.queued == 0)
}

fn live(id: &ThreadId) -> impl Fn(&Hub) -> bool + '_ {
    move |hub| hub.view(id).is_some_and(|v| matches!(v.link, Link::Live { .. }))
}

fn needs_login(id: &ThreadId) -> impl Fn(&Hub) -> bool + '_ {
    move |hub| hub.view(id).and_then(|v| v.connection).is_some_and(|c| c.state == ConnState::NeedsAuth)
}

/// Prompts and waits for the turn to end -- or for the agent to say it
/// wants a login, which the suite can't give it (the prompt waits for
/// one, so the turn would never end).
fn turn(d: &mut Driver, id: &ThreadId, target: &Target, prompt: Vec<ContentBlock>) {
    let fx = d.hub.prompt(id, prompt);
    d.apply(fx);
    let out = d.wait(target.turn_timeout(), |h| needs_login(id)(h) || h.view(id).is_some_and(|v| v.queued == 0 && !matches!(v.turn, Turn::Idle)));
    assert!(!needs_login(id)(&d.hub), "the agent wants a login: log in to it in a terminal first");
    assert!(out || d.wait(Duration::ZERO, idle(id)), "the prompt never went out");
    let ended = d.wait(target.turn_timeout(), |h| needs_login(id)(h) || idle(id)(h));
    assert!(!needs_login(id)(&d.hub), "the agent wants a login: log in to it in a terminal first");
    assert!(ended, "the turn never ended");
}

fn answer(d: &Driver, id: &ThreadId) -> String {
    d.hub.view(id).unwrap().thread.entries.iter().filter_map(|e| if let Entry::Agent { text, .. } = e { Some(text.as_str()) } else { None }).collect::<Vec<_>>().join("\n")
}

fn errors(d: &Driver, id: &ThreadId) -> Vec<String> {
    d.hub.view(id).unwrap().thread.entries.iter().filter_map(|e| if let Entry::Notice(n) = e { matches!(n.kind, NoticeKind::Error | NoticeKind::Login).then(|| n.text.clone()) } else { None }).collect()
}

// ---------------------------------------------------------------------
// The scenarios

/// 1. Handshake, then a session (a login when the agent wants one is
///    reported, not done: it needs you).
fn handshake(target: &Target) {
    let root = workdir("handshake");
    let (mut d, id) = driver_for(target, &root);
    let fx = d.hub.focus(&id);
    d.apply(fx);
    let ok = d.wait(Duration::from_secs(60), |h| h.view(&id).is_some_and(|v| matches!(v.link, Link::Live { .. }) || v.connection.is_some_and(|c| matches!(c.state, ConnState::NeedsAuth | ConnState::Dead { .. }))));
    assert!(ok, "no session and no answer");
    let view = d.hub.view(&id).unwrap();
    let conn = view.connection.expect("a connection");
    assert!(conn.info.is_some(), "initialize answered: {:?}", conn.state);
    assert!(matches!(view.link, Link::Live { .. }), "no session: {:?} {:?}", conn.state, errors(&d, &id));
    eprintln!("[{}] agent: {:?} {:?}", target.spec.id, conn.info.as_ref().unwrap().display_name(), conn.info.as_ref().unwrap().version);
    d.shutdown();
}

/// 2. A prompt, and a streamed answer.
fn prompt_and_answer(target: &Target) {
    let root = workdir("hello");
    let (mut d, id) = driver_for(target, &root);
    turn(&mut d, &id, target, target.prompt("hello", "Reply with exactly the word: ready"));
    assert!(errors(&d, &id).is_empty(), "{:?}", errors(&d, &id));
    assert!(!answer(&d, &id).trim().is_empty(), "no answer");
    d.shutdown();
}

/// 3. What's in a file is what the agent sees (Fenix saves before a
///    prompt, or serves reads from its buffers).
fn reads_the_file(target: &Target) {
    let root = workdir("read");
    std::fs::write(root.join("notes.txt"), "The secret word is pelican.\n").unwrap();
    let (mut d, id) = driver_for(target, &root);
    turn(&mut d, &id, target, target.prompt("read notes.txt", "Read the file notes.txt in the current directory and tell me the secret word in it."));
    let view = d.hub.view(&id).unwrap();
    let read = view.thread.entries.iter().any(|e| matches!(e, Entry::Tool(c) if c.kind == ToolKind::Read && c.status == ToolStatus::Completed));
    assert!(read || answer(&d, &id).to_lowercase().contains("pelican"), "the file was never read");
    d.shutdown();
}

/// 4. An edit: announced (Fenix snapshots the file) and on disk.
fn edits_a_file(target: &Target) {
    let root = workdir("edit");
    let file = root.join("fruit.txt");
    std::fs::write(&file, "apple\n").unwrap();
    let (mut d, id) = driver_for(target, &root);
    turn(&mut d, &id, target, target.prompt("edit fruit.txt apple=>pear", "In the file fruit.txt, replace the word apple with pear. Change nothing else."));
    assert_eq!(std::fs::read_to_string(&file).unwrap().trim(), "pear");
    let named = d.announced.iter().flat_map(|(_, p)| p).any(|p| p.ends_with("fruit.txt"));
    assert!(named, "the edit was never announced before it was made: {:?}", d.announced);
    d.shutdown();
}

/// 5. A new file.
fn creates_a_file(target: &Target) {
    let root = workdir("create");
    let (mut d, id) = driver_for(target, &root);
    turn(&mut d, &id, target, target.prompt("write hello.txt hi there", "Create a file named hello.txt containing exactly: hi there"));
    assert_eq!(std::fs::read_to_string(root.join("hello.txt")).unwrap().trim(), "hi there");
    d.shutdown();
}

/// 6. A command, after a permission request (allowed once).
fn runs_a_command(target: &Target) {
    let root = workdir("run");
    let (mut d, id) = driver_for(target, &root);
    turn(&mut d, &id, target, target.prompt("run echo conformance-ok", "Run the shell command `echo conformance-ok` and tell me what it printed."));
    let view = d.hub.view(&id).unwrap();
    let ran = view.thread.entries.iter().any(|e| matches!(e, Entry::Tool(c) if c.kind == ToolKind::Execute && c.status == ToolStatus::Completed));
    assert!(ran, "no command ran");
    assert!(view.thread.entries.iter().any(|e| matches!(e, Entry::Permission(_))) || !target.live, "the mock asks first");
    d.shutdown();
}

/// 7. Cancel mid-turn.
fn cancels(target: &Target) {
    let root = workdir("cancel");
    let (mut d, id) = driver_for(target, &root);
    let fx = d.hub.prompt(&id, target.prompt("slow", "Count from 1 to 300, one number per line, with no other text."));
    d.apply(fx);
    assert!(d.wait(target.turn_timeout(), |h| h.view(&id).is_some_and(|v| v.thread.entries.iter().any(|e| matches!(e, Entry::Agent { .. } | Entry::Thought { .. } | Entry::Tool(_))))), "nothing came");
    let fx = d.hub.cancel(&id, Instant::now());
    d.apply(fx);
    assert!(d.wait(Duration::from_secs(30), idle(&id)), "the turn didn't end");
    let interrupted = d.hub.view(&id).unwrap().thread.entries.iter().any(|e| matches!(e, Entry::Notice(n) if n.kind == NoticeKind::Interrupted));
    assert!(interrupted, "not marked cancelled");
    d.shutdown();
}

/// 8. The session taken back after the agent restarts (resume or load).
fn resumes(target: &Target) {
    let root = workdir("resume");
    let (mut d, id) = driver_for(target, &root);
    turn(&mut d, &id, target, target.prompt("hello", "Remember the number 4242. Reply with just: ok"));
    let session = d.hub.view(&id).unwrap().meta.session_id.clone().expect("a session");
    let thread = d.hub.view(&id).unwrap().thread.clone();
    d.shutdown();
    drop(d);
    // A Fenix started again: the thread read back, detached.
    let mut hub = Hub::new("conf2", "test");
    hub.set_agents(vec![target.spec.clone()]);
    hub.restore(ThreadMeta { id: id.clone(), agent: target.spec.id.clone(), root: root.clone(), session_id: Some(session), handed_off_from: None }, thread);
    let mut d = Driver::new(hub);
    let fx = d.hub.focus(&id);
    d.apply(fx);
    assert!(d.wait(Duration::from_secs(60), |h| h.view(&id).is_some_and(|v| matches!(v.link, Link::Live { .. } | Link::ReadOnly(_)))), "never attached");
    assert!(matches!(d.hub.view(&id).unwrap().link, Link::Live { .. }), "the agent couldn't take the session back: {:?}", d.hub.view(&id).unwrap().link);
    turn(&mut d, &id, target, target.prompt("hello", "What number did I ask you to remember? Reply with just the number."));
    if target.live {
        assert!(answer(&d, &id).contains("4242"), "the session came back without its history");
    }
    d.shutdown();
}

/// 9. Switching mode, or another config option.
fn switches_mode(target: &Target) {
    let root = workdir("mode");
    let (mut d, id) = driver_for(target, &root);
    let fx = d.hub.focus(&id);
    d.apply(fx);
    assert!(d.wait(Duration::from_secs(60), live(&id)));
    let modes = d.hub.view(&id).unwrap().thread.modes.clone();
    match modes {
        Some(modes) if modes.available.len() > 1 => {
            let other = modes.available.iter().find(|m| m.id != modes.current).unwrap().id.clone();
            let fx = d.hub.set_mode(&id, &other);
            d.apply(fx);
            d.wait(Duration::from_secs(5), |_| false);
            assert_eq!(d.hub.view(&id).unwrap().thread.modes.as_ref().unwrap().current, other, "the agent refused: {:?}", d.hub.view(&id).unwrap().thread.entries);
        }
        _ => eprintln!("[{}] offers no modes", target.spec.id),
    }
    d.shutdown();
}

fn all(target: &Target) {
    type Scenario = fn(&Target);
    let scenarios: [(&str, Scenario); 9] = [
        ("handshake, login", handshake),
        ("prompt, streamed answer", prompt_and_answer),
        ("reads the file", reads_the_file),
        ("edit a file", edits_a_file),
        ("create a file", creates_a_file),
        ("run a command", runs_a_command),
        ("cancel mid-turn", cancels),
        ("load or resume", resumes),
        ("switch mode", switches_mode),
    ];
    let mut failed = Vec::new();
    for (name, scenario) in scenarios {
        let result = std::panic::catch_unwind(|| scenario(target));
        let mark = if result.is_ok() { "pass" } else { "FAIL" };
        eprintln!("[{}] {mark}: {name}", target.spec.id);
        if result.is_err() {
            failed.push(name);
        }
    }
    assert!(failed.is_empty(), "{} failed: {failed:?}", target.spec.id);
}

#[test]
fn the_mock_conforms() {
    all(&Target::mock());
}

#[test]
#[ignore = "needs real agents: FENIX_ACP_LIVE=claude,codex,devin"]
fn live_agents_conform() {
    let wanted = std::env::var("FENIX_ACP_LIVE").unwrap_or_default();
    let entries = catalog::catalog(&[], &[], &[], &[], &catalog::find_on_path);
    let mut failed = Vec::new();
    for id in wanted.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        let spec = match std::env::var(format!("FENIX_ACP_{}", id.to_uppercase())) {
            Ok(line) => {
                let words = catalog::split_command_line(&line);
                AgentSpec { id: id.into(), name: id.into(), program: words[0].clone(), args: words[1..].to_vec(), env: vec![] }
            }
            Err(_) => match entries.iter().find(|e| e.spec.id == id && e.found) {
                Some(e) => e.spec.clone(),
                None => {
                    eprintln!("[{id}] not found on PATH; set FENIX_ACP_{}", id.to_uppercase());
                    failed.push(id.to_string());
                    continue;
                }
            },
        };
        if std::panic::catch_unwind(|| all(&Target { spec, live: true })).is_err() {
            failed.push(id.to_string());
        }
    }
    assert!(failed.is_empty(), "not conforming: {failed:?}");
    let _ = driver::allow_once;
}
