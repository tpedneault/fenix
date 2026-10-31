//! The hub's flows (plan §3.5), each as inputs in and effects out.

use super::*;
use crate::thread::Entry;
use serde_json::json;

fn claude() -> AgentSpec {
    AgentSpec { id: "claude".into(), name: "Claude Code".into(), program: "claude-agent-acp".into(), args: vec![], env: vec![] }
}

fn root() -> PathBuf {
    PathBuf::from("/work/fenix")
}

/// What claude-agent-acp answers `initialize` with (trimmed).
fn claude_init() -> Value {
    json!({
        "protocolVersion": 1,
        "agentCapabilities": { "loadSession": true, "promptCapabilities": { "image": true, "embeddedContext": true }, "sessionCapabilities": { "list": {}, "resume": {}, "close": {}, "delete": {} }, "auth": { "logout": {} } },
        "agentInfo": { "name": "@agentclientprotocol/claude-agent-acp", "title": "Claude Agent", "version": "0.99.1" },
        "authMethods": [{ "id": "claude-ai-login", "name": "Claude Subscription", "type": "terminal", "args": ["--cli", "auth", "login", "--claudeai"] }]
    })
}

fn setup() -> Value {
    json!({
        "sessionId": "s1",
        "modes": { "currentModeId": "default", "availableModes": [{ "id": "default", "name": "Manual" }, { "id": "plan", "name": "Plan" }] },
        "configOptions": [{ "id": "model", "name": "Model", "category": "model", "type": "select", "currentValue": "default", "options": [{ "value": "default", "name": "Default" }, { "value": "opus", "name": "Opus" }] }]
    })
}

struct Harness {
    hub: Hub,
    now: Instant,
}

impl Harness {
    fn new() -> Self {
        let mut hub = Hub::new("t", "1.0.0");
        hub.set_agents(vec![claude()]);
        Self { hub, now: Instant::now() }
    }

    fn later(&mut self, secs: u64) -> Instant {
        self.now += Duration::from_secs(secs);
        self.now
    }

    fn generation(&self, conn: ConnId) -> u32 {
        self.hub.connection(conn).unwrap().generation
    }

    fn event(&mut self, conn: ConnId, event: AcpEvent) -> Vec<Effect> {
        let generation = self.generation(conn);
        self.hub.on_event(conn, generation, event, self.now)
    }

    fn answer(&mut self, conn: ConnId, id: i64, result: Value) -> Vec<Effect> {
        self.event(conn, AcpEvent::Response { id, result: Ok(result) })
    }

    fn fail(&mut self, conn: ConnId, id: i64, code: i64, message: &str) -> Vec<Effect> {
        self.event(conn, AcpEvent::Response { id, result: Err(RpcError::new(code, message)) })
    }

    fn update(&mut self, conn: ConnId, session: &str, update: Value) -> Vec<Effect> {
        self.event(conn, AcpEvent::Notification { method: "session/update".into(), params: json!({ "sessionId": session, "update": update }) })
    }

    /// Starts the agent for a fresh, focused thread up to a live session:
    /// spawn, initialize, session/new.
    fn live_thread(&mut self) -> (ThreadId, ConnId) {
        let id = self.hub.new_thread("claude", &root());
        let fx = self.hub.focus(&id);
        let conn = spawned(&fx).expect("focusing a new thread starts its agent");
        let fx = self.hub.on_spawned(conn, 1, Ok(()), self.now);
        let init = request(&fx, "initialize").unwrap();
        let fx = self.answer(conn, init.0, claude_init());
        let new = request(&fx, "session/new").expect("ready: the deferred attach goes out");
        assert_eq!(new.1["cwd"], json!("/work/fenix"));
        let fx = self.answer(conn, new.0, setup());
        assert!(fx.contains(&Effect::Save(id.clone())));
        assert_eq!(self.hub.view(&id).unwrap().link, &Link::Live { conn, session: "s1".into() });
        (id, conn)
    }
}

fn spawned(fx: &[Effect]) -> Option<ConnId> {
    fx.iter().find_map(|e| match e {
        Effect::Spawn { conn, .. } => Some(*conn),
        _ => None,
    })
}

fn request(fx: &[Effect], method: &str) -> Option<(i64, Value)> {
    fx.iter().find_map(|e| match e {
        Effect::Request { id, method: m, params, .. } if *m == method => Some((*id, params.clone())),
        _ => None,
    })
}

fn requests(fx: &[Effect]) -> Vec<&'static str> {
    fx.iter()
        .filter_map(|e| match e {
            Effect::Request { method, .. } => Some(*method),
            Effect::Notify { method, .. } => Some(*method),
            _ => None,
        })
        .collect()
}

fn texts(blocks: &[&str]) -> Vec<ContentBlock> {
    blocks.iter().map(|t| ContentBlock::text(*t)).collect()
}

fn notices(hub: &Hub, id: &ThreadId) -> Vec<(NoticeKind, String)> {
    hub.view(id).unwrap().thread.entries.iter().filter_map(|e| if let Entry::Notice(n) = e { Some((n.kind, n.text.clone())) } else { None }).collect()
}

#[test]
fn a_new_thread_starts_nothing_until_it_is_looked_at() {
    let mut h = Harness::new();
    let id = h.hub.new_thread("claude", &root());
    assert_eq!(h.hub.view(&id).unwrap().link, &Link::Detached);
    assert_eq!(h.hub.connections().count(), 0);
}

#[test]
fn starting_a_thread_spawns_initializes_and_opens_a_session() {
    let mut h = Harness::new();
    let (id, conn) = h.live_thread();
    let view = h.hub.view(&id).unwrap();
    assert_eq!(view.thread.modes.as_ref().unwrap().current, "default");
    assert_eq!(view.thread.config_options[0].id, "model");
    assert_eq!(view.connection.unwrap().info.as_ref().unwrap().display_name(), Some("Claude Agent"));
    assert_eq!(h.hub.route(conn, "s1"), Some(&id));
}

#[test]
fn a_prompt_written_before_the_session_exists_waits_then_goes() {
    let mut h = Harness::new();
    let id = h.hub.new_thread("claude", &root());
    let fx = h.hub.prompt(&id, texts(&["fix the bug"]));
    let conn = spawned(&fx).unwrap();
    assert!(request(&fx, "session/prompt").is_none());
    assert_eq!(h.hub.view(&id).unwrap().queued, 1);
    let fx = h.hub.on_spawned(conn, 1, Ok(()), h.now);
    let fx = h.answer(conn, request(&fx, "initialize").unwrap().0, claude_init());
    let fx = h.answer(conn, request(&fx, "session/new").unwrap().0, setup());
    let (_, params) = request(&fx, "session/prompt").expect("the queued prompt goes out once live");
    assert_eq!(params["prompt"][0]["text"], json!("fix the bug"));
    assert!(matches!(h.hub.view(&id).unwrap().turn, Turn::Prompting { .. }));
    assert!(matches!(&h.hub.view(&id).unwrap().thread.entries[0], Entry::User { queued: false, .. }));
}

#[test]
fn a_prompt_during_a_turn_is_queued_and_goes_when_it_ends() {
    let mut h = Harness::new();
    let (id, conn) = h.live_thread();
    let first = request(&h.hub.prompt(&id, texts(&["one"])), "session/prompt").unwrap();
    let fx = h.hub.prompt(&id, texts(&["two"]));
    assert!(request(&fx, "session/prompt").is_none(), "one turn at a time");
    assert_eq!(h.hub.view(&id).unwrap().queued, 1);
    let fx = h.answer(conn, first.0, json!({ "stopReason": "end_turn" }));
    assert!(fx.contains(&Effect::Attention { thread: id.clone(), why: Attention::TurnEnded }));
    assert!(fx.contains(&Effect::TurnEnded { thread: id.clone(), stop: "end_turn".into() }));
    let (_, params) = request(&fx, "session/prompt").unwrap();
    assert_eq!(params["prompt"][0]["text"], json!("two"));
}

#[test]
fn cancel_withdraws_permissions_and_cancel_with_no_turn_takes_back_the_queued_prompt() {
    let mut h = Harness::new();
    let (id, conn) = h.live_thread();
    let prompt = request(&h.hub.prompt(&id, texts(&["edit"])), "session/prompt").unwrap();
    let fx = h.event(
        conn,
        AcpEvent::Request {
            id: json!(7),
            method: "session/request_permission".into(),
            params: json!({ "sessionId": "s1", "toolCall": { "toolCallId": "t1", "title": "Edit a.rs", "kind": "edit" }, "options": [{ "optionId": "allow-once", "name": "Yes", "kind": "allow_once" }, { "optionId": "reject", "name": "No", "kind": "reject_once" }] }),
        },
    );
    assert!(fx.contains(&Effect::Attention { thread: id.clone(), why: Attention::Permission }));
    assert_eq!(h.hub.status(), Status { working: 0, waiting: 1 });
    let fx = h.hub.cancel(&id, h.now);
    assert_eq!(requests(&fx), vec!["session/cancel"]);
    assert!(fx.contains(&Effect::Respond { conn, id: json!(7), result: Ok(json!({ "outcome": { "outcome": "cancelled" } })) }));
    let fx = h.answer(conn, prompt.0, json!({ "stopReason": "cancelled" }));
    assert!(fx.contains(&Effect::Save(id.clone())));
    assert!(fx.contains(&Effect::TurnEnded { thread: id.clone(), stop: "cancelled".into() }));
    assert_eq!(notices(&h.hub, &id).last().unwrap().0, NoticeKind::Interrupted);
    // Idle now: a queued prompt can be taken back.
    h.hub.prompt(&id, texts(&["a"]));
    h.hub.prompt(&id, texts(&["b"]));
    assert_eq!(h.hub.view(&id).unwrap().queued, 1, "`a` went out at once");
    h.hub.cancel(&id, h.now);
    assert_eq!(h.hub.view(&id).unwrap().queued, 1, "a turn is running: cancel cancels it, not the queue");
}

#[test]
fn a_cancel_the_agent_never_confirms_ends_after_the_grace() {
    let mut h = Harness::new();
    let (id, conn) = h.live_thread();
    let prompt = request(&h.hub.prompt(&id, texts(&["slow"])), "session/prompt").unwrap();
    h.hub.cancel(&id, h.now);
    let now = h.later(5);
    assert!(h.hub.tick(now).is_empty());
    let now = h.later(6);
    let fx = h.hub.tick(now);
    assert!(fx.contains(&Effect::Changed(id.clone())));
    assert!(fx.contains(&Effect::TurnEnded { thread: id.clone(), stop: "cancelled".into() }));
    assert_eq!(h.hub.view(&id).unwrap().turn, &Turn::Idle);
    // The late answer is ignored.
    assert!(h.answer(conn, prompt.0, json!({ "stopReason": "cancelled" })).is_empty());
}

#[test]
fn permission_answers_go_back_by_request_id() {
    let mut h = Harness::new();
    let (id, conn) = h.live_thread();
    h.hub.prompt(&id, texts(&["edit"]));
    h.update(conn, "s1", json!({ "sessionUpdate": "tool_call", "toolCallId": "t1", "title": "Edit a.rs", "kind": "edit", "status": "pending", "content": [{ "type": "diff", "path": "/work/fenix/a.rs", "oldText": "a", "newText": "b" }] }));
    h.event(conn, AcpEvent::Request { id: json!("p-1"), method: "session/request_permission".into(), params: json!({ "sessionId": "s1", "toolCall": { "toolCallId": "t1" }, "options": [{ "optionId": "allow-once", "name": "Yes", "kind": "allow_once" }] }) });
    let card = h.hub.view(&id).unwrap().thread.entries.iter().find_map(|e| if let Entry::Permission(c) = e { Some(c.clone()) } else { None }).unwrap();
    assert_eq!(card.tool_call.title.as_deref(), Some("Edit a.rs"), "the card knows the call it's about");
    assert_eq!(card.tool_call.content.as_ref().unwrap().len(), 1);
    let fx = h.hub.answer_permission(&id, &card.key, Some("allow-once"));
    assert!(fx.contains(&Effect::Respond { conn, id: json!("p-1"), result: Ok(json!({ "outcome": { "outcome": "selected", "optionId": "allow-once" } })) }));
    assert!(h.hub.answer_permission(&id, &card.key, Some("allow-once")).is_empty(), "answered once");
}

#[test]
fn an_edit_announced_asks_for_its_files_to_be_snapshotted_once() {
    let mut h = Harness::new();
    let (id, conn) = h.live_thread();
    h.hub.prompt(&id, texts(&["edit"]));
    let fx = h.update(conn, "s1", json!({ "sessionUpdate": "tool_call", "toolCallId": "t1", "title": "Edit", "kind": "edit", "status": "pending", "rawInput": {} }));
    assert!(!fx.iter().any(|e| matches!(e, Effect::Announced { .. })), "no path yet");
    let fx = h.update(conn, "s1", json!({ "sessionUpdate": "tool_call_update", "toolCallId": "t1", "rawInput": { "file_path": "/work/fenix/a.rs" } }));
    assert!(fx.contains(&Effect::Announced { thread: id.clone(), tool_call: "t1".into(), paths: vec!["/work/fenix/a.rs".into()] }), "the kind is known from the call");
    let fx = h.update(conn, "s1", json!({ "sessionUpdate": "tool_call_update", "toolCallId": "t1", "status": "completed", "locations": [{ "path": "/work/fenix/a.rs", "line": 3 }] }));
    assert!(!fx.iter().any(|e| matches!(e, Effect::Announced { .. })));
    let fx = h.update(conn, "s1", json!({ "sessionUpdate": "tool_call", "toolCallId": "t2", "title": "Read b.rs", "kind": "read", "locations": [{ "path": "/work/fenix/b.rs" }] }));
    assert!(!fx.iter().any(|e| matches!(e, Effect::Announced { .. })), "reads aren't snapshotted");
}

#[test]
fn fs_and_terminal_requests_are_served_and_answered_unless_withdrawn() {
    let mut h = Harness::new();
    let (id, conn) = h.live_thread();
    let fx = h.event(conn, AcpEvent::Request { id: json!(3), method: "fs/read_text_file".into(), params: json!({ "sessionId": "s1", "path": "/work/fenix/a.rs" }) });
    assert!(matches!(&fx[0], Effect::Serve { request: ClientRequest::ReadTextFile { .. }, .. }));
    let fx = h.hub.on_served(conn, &id, json!(3), Ok(json!({ "content": "x" })));
    assert_eq!(fx, vec![Effect::Respond { conn, id: json!(3), result: Ok(json!({ "content": "x" })) }]);
    // Unknown sessions and methods are refused.
    let fx = h.event(conn, AcpEvent::Request { id: json!(4), method: "fs/read_text_file".into(), params: json!({ "sessionId": "nope", "path": "/a" }) });
    assert!(matches!(&fx[0], Effect::Respond { result: Err(e), .. } if e.code == code::INVALID_PARAMS));
    let fx = h.event(conn, AcpEvent::Request { id: json!(5), method: "_claude/thing".into(), params: json!({}) });
    assert!(matches!(&fx[0], Effect::Respond { result: Err(e), .. } if e.code == code::METHOD_NOT_FOUND));
    // A request served across a crash is never answered.
    h.event(conn, AcpEvent::Request { id: json!(6), method: "terminal/create".into(), params: json!({ "sessionId": "s1", "command": "cargo" }) });
    h.event(conn, AcpEvent::Disconnected("killed".into()));
    assert!(h.hub.on_served(conn, &id, json!(6), Ok(json!({ "terminalId": "1" }))).is_empty());
}

#[test]
fn a_crash_mid_turn_interrupts_restarts_with_backoff_and_resumes() {
    let mut h = Harness::new();
    let (id, conn) = h.live_thread();
    h.hub.prompt(&id, texts(&["work"]));
    h.update(conn, "s1", json!({ "sessionUpdate": "tool_call", "toolCallId": "t1", "title": "Run", "kind": "execute", "status": "in_progress" }));
    let fx = h.event(conn, AcpEvent::Disconnected("the agent closed its output (exit code 1)".into()));
    assert!(fx.contains(&Effect::Withdraw { thread: id.clone() }));
    assert!(fx.contains(&Effect::TurnEnded { thread: id.clone(), stop: "error".into() }), "a turn cut off is done with");
    let view = h.hub.view(&id).unwrap();
    assert_eq!(view.turn, &Turn::Idle);
    assert!(matches!(view.link, Link::Attaching { how: Attach::Resume, .. }), "focused: it comes back with the agent");
    assert_eq!(view.thread.tool("t1").unwrap().status, crate::protocol::ToolStatus::Failed);
    assert_eq!(notices(&h.hub, &id).last().unwrap().0, NoticeKind::Interrupted);
    let now = h.later(1);
    let fx = h.hub.tick(now);
    assert!(matches!(fx[0], Effect::Spawn { generation: 2, .. }));
    // Events from the process that died are dropped.
    let stale = AcpEvent::Notification { method: "session/update".into(), params: json!({ "sessionId": "s1", "update": { "sessionUpdate": "agent_message_chunk", "content": { "type": "text", "text": "ghost" } } }) };
    assert!(h.hub.on_event(conn, 1, stale, h.now).is_empty());
    let fx = h.hub.on_spawned(conn, 2, Ok(()), h.now);
    let fx = h.answer(conn, request(&fx, "initialize").unwrap().0, claude_init());
    let (resume, params) = request(&fx, "session/resume").expect("resume, since the agent offers it");
    assert_eq!(params["sessionId"], json!("s1"));
    h.answer(conn, resume, setup());
    assert_eq!(h.hub.view(&id).unwrap().link, &Link::Live { conn, session: "s1".into() });
}

#[test]
fn three_crashes_in_a_minute_and_the_agent_stays_down_until_restarted() {
    let mut h = Harness::new();
    let (id, conn) = h.live_thread();
    for n in 0..3u32 {
        h.event(conn, AcpEvent::Disconnected("boom".into()));
        if n < 2 {
            let now = h.later(1 << n);
            let fx = h.hub.tick(now);
            assert!(spawned(&fx).is_some(), "crash {n} restarts");
            let generation = h.generation(conn);
            let fx = h.hub.on_spawned(conn, generation, Ok(()), h.now);
            h.answer(conn, request(&fx, "initialize").unwrap().0, claude_init());
        }
    }
    assert!(matches!(h.hub.connection(conn).unwrap().state, ConnState::Dead { retry_at: None, .. }));
    let now = h.later(30);
    assert!(spawned(&h.hub.tick(now)).is_none());
    let fx = h.hub.restart(conn, h.now);
    assert!(spawned(&fx).is_some());
    let _ = id;
}

#[test]
fn an_unfocused_thread_comes_back_only_when_looked_at() {
    let mut h = Harness::new();
    let (id, conn) = h.live_thread();
    h.hub.unfocus();
    h.event(conn, AcpEvent::Disconnected("bye".into()));
    assert_eq!(h.hub.view(&id).unwrap().link, &Link::Detached);
    assert!(matches!(h.hub.connection(conn).unwrap().state, ConnState::Dead { retry_at: None, .. }), "nobody wants it: no restart");
    let fx = h.hub.focus(&id);
    assert!(spawned(&fx).is_some(), "looking at it starts the agent again");
}

#[test]
fn load_rebuilds_the_thread_from_the_replay_without_duplicates() {
    let mut h = Harness::new();
    let id = h.hub.new_thread("claude", &root());
    {
        let slot = h.hub.threads.get_mut(&id).unwrap();
        slot.meta.session_id = Some("old".into());
        slot.thread.push_user(texts(&["stored prompt"]), false);
        slot.thread.notice(NoticeKind::Info, "stored notice");
    }
    let conn = spawned(&h.hub.focus(&id)).unwrap();
    let fx = h.hub.on_spawned(conn, 1, Ok(()), h.now);
    let mut init = claude_init();
    init["agentCapabilities"]["sessionCapabilities"] = json!({});
    let fx = h.answer(conn, request(&fx, "initialize").unwrap().0, init);
    let (load, _) = request(&fx, "session/load").expect("no resume: load");
    h.update(conn, "old", json!({ "sessionUpdate": "user_message_chunk", "content": { "type": "text", "text": "stored prompt" } }));
    h.update(conn, "old", json!({ "sessionUpdate": "agent_message_chunk", "content": { "type": "text", "text": "the answer" } }));
    assert_eq!(h.hub.view(&id).unwrap().thread.entries.len(), 2, "the stored thread stays on screen during the replay");
    h.answer(conn, load, json!({}));
    let entries = &h.hub.view(&id).unwrap().thread.entries;
    assert_eq!(entries.len(), 2);
    assert!(matches!(&entries[1], Entry::Agent { text, .. } if text == "the answer"));
}

#[test]
fn load_without_a_replay_keeps_what_was_stored() {
    let mut h = Harness::new();
    let id = h.hub.new_thread("claude", &root());
    {
        let slot = h.hub.threads.get_mut(&id).unwrap();
        slot.meta.session_id = Some("old".into());
        slot.thread.push_user(texts(&["stored"]), false);
    }
    let conn = spawned(&h.hub.focus(&id)).unwrap();
    let fx = h.hub.on_spawned(conn, 1, Ok(()), h.now);
    let mut init = claude_init();
    init["agentCapabilities"]["sessionCapabilities"] = json!({});
    let fx = h.answer(conn, request(&fx, "initialize").unwrap().0, init);
    h.answer(conn, request(&fx, "session/load").unwrap().0, json!({}));
    assert_eq!(h.hub.view(&id).unwrap().thread.entries.len(), 1);
    assert!(matches!(h.hub.view(&id).unwrap().link, Link::Live { .. }));
}

#[test]
fn a_session_the_agent_forgot_makes_the_thread_read_only_and_it_can_continue_anew() {
    let mut h = Harness::new();
    let id = h.hub.new_thread("claude", &root());
    h.hub.threads.get_mut(&id).unwrap().meta.session_id = Some("gone".into());
    h.hub.threads.get_mut(&id).unwrap().thread.push_user(texts(&["old question"]), false);
    let conn = spawned(&h.hub.focus(&id)).unwrap();
    let fx = h.hub.on_spawned(conn, 1, Ok(()), h.now);
    let fx = h.answer(conn, request(&fx, "initialize").unwrap().0, claude_init());
    h.fail(conn, request(&fx, "session/resume").unwrap().0, code::SESSION_NOT_FOUND, "Session not found");
    assert!(matches!(h.hub.view(&id).unwrap().link, Link::ReadOnly(_)));
    let fx = h.hub.prompt(&id, texts(&["more"]));
    assert!(request(&fx, "session/prompt").is_none());
    h.hub.continue_in_new_session(&id, 64 * 1024);
    let fx = h.hub.prompt(&id, texts(&["more"]));
    let fx = [fx, h.hub.focus(&id)].concat();
    let (new, _) = request(&fx, "session/new").expect("a new session, on the running agent");
    let fx = h.answer(conn, new, json!({ "sessionId": "s2" }));
    let (_, params) = request(&fx, "session/prompt").unwrap();
    assert_eq!(params["prompt"][1]["type"], json!("resource"), "the transcript goes with the prompt");
    assert!(params["prompt"][1]["resource"]["text"].as_str().unwrap().contains("old question"));
}

#[test]
fn a_resume_that_fails_otherwise_falls_back_to_load() {
    let mut h = Harness::new();
    let id = h.hub.new_thread("claude", &root());
    h.hub.threads.get_mut(&id).unwrap().meta.session_id = Some("s1".into());
    let conn = spawned(&h.hub.focus(&id)).unwrap();
    let fx = h.hub.on_spawned(conn, 1, Ok(()), h.now);
    let fx = h.answer(conn, request(&fx, "initialize").unwrap().0, claude_init());
    let fx = h.fail(conn, request(&fx, "session/resume").unwrap().0, code::INTERNAL_ERROR, "busy");
    let (load, _) = request(&fx, "session/load").unwrap();
    h.answer(conn, load, json!({}));
    assert!(matches!(h.hub.view(&id).unwrap().link, Link::Live { .. }));
}

#[test]
fn auth_required_mid_turn_asks_for_a_login_and_the_prompt_goes_again_after() {
    let mut h = Harness::new();
    let (id, conn) = h.live_thread();
    let prompt = request(&h.hub.prompt(&id, texts(&["hello"])), "session/prompt").unwrap();
    let fx = h.fail(conn, prompt.0, code::AUTH_REQUIRED, "Authentication required");
    assert!(fx.contains(&Effect::Attention { thread: id.clone(), why: Attention::Login }));
    assert_eq!(h.hub.connection(conn).unwrap().state, ConnState::NeedsAuth);
    assert_eq!(h.hub.status().waiting, 1);
    let fx = h.hub.authenticate(conn, "claude-ai-login");
    assert!(fx.iter().any(|e| matches!(e, Effect::TerminalLogin { args, .. } if args[3] == "--claudeai")));
    let fx = h.hub.terminal_login_finished(conn, true);
    let (_, params) = request(&fx, "session/prompt").expect("the prompt goes again");
    assert_eq!(params["prompt"][0]["text"], json!("hello"));
    let users = h.hub.view(&id).unwrap().thread.entries.iter().filter(|e| matches!(e, Entry::User { .. })).count();
    assert_eq!(users, 1, "shown once");
}

#[test]
fn an_agent_login_goes_through_authenticate() {
    let mut h = Harness::new();
    let id = h.hub.new_thread("claude", &root());
    let conn = spawned(&h.hub.focus(&id)).unwrap();
    let fx = h.hub.on_spawned(conn, 1, Ok(()), h.now);
    let mut init = claude_init();
    init["authMethods"] = json!([{ "id": "mock-login", "name": "Log in" }]);
    let fx = h.answer(conn, request(&fx, "initialize").unwrap().0, init);
    let fx = h.fail(conn, request(&fx, "session/new").unwrap().0, code::AUTH_REQUIRED, "login first");
    assert!(fx.contains(&Effect::Attention { thread: id.clone(), why: Attention::Login }));
    let fx = h.hub.authenticate(conn, "mock-login");
    let fx = h.answer(conn, request(&fx, "authenticate").unwrap().0, json!({}));
    let (new, _) = request(&fx, "session/new").expect("the attach goes again once logged in");
    h.answer(conn, new, setup());
    assert!(matches!(h.hub.view(&id).unwrap().link, Link::Live { .. }));
}

#[test]
fn modes_and_options_change_at_once_and_go_back_when_refused() {
    let mut h = Harness::new();
    let (id, conn) = h.live_thread();
    let fx = h.hub.set_mode(&id, "plan");
    assert_eq!(h.hub.view(&id).unwrap().thread.modes.as_ref().unwrap().current, "plan");
    h.fail(conn, request(&fx, "session/set_mode").unwrap().0, code::INVALID_PARAMS, "Invalid Mode: plan");
    assert_eq!(h.hub.view(&id).unwrap().thread.modes.as_ref().unwrap().current, "default");
    let fx = h.hub.set_config_option(&id, "model", ConfigValue::Select { current: "opus".into(), choices: vec![] });
    assert_eq!(h.hub.view(&id).unwrap().thread.config_options[0].current_label(), "Opus");
    let (req, params) = request(&fx, "session/set_config_option").unwrap();
    assert_eq!(params, json!({ "sessionId": "s1", "configId": "model", "value": "opus" }));
    h.fail(conn, req, code::INVALID_PARAMS, "no");
    assert_eq!(h.hub.view(&id).unwrap().thread.config_options[0].current_label(), "Default");
    // The agent switching mode itself wins.
    h.update(conn, "s1", json!({ "sessionUpdate": "current_mode_update", "currentModeId": "acceptEdits" }));
    assert_eq!(h.hub.view(&id).unwrap().thread.modes.as_ref().unwrap().current, "acceptEdits");
}

#[test]
fn switching_threads_leaves_sessions_running() {
    let mut h = Harness::new();
    let (a, conn) = h.live_thread();
    h.hub.prompt(&a, texts(&["long job"]));
    let b = h.hub.new_thread("claude", &root());
    let fx = h.hub.focus(&b);
    assert!(spawned(&fx).is_none(), "the same agent and project: one process");
    let (new, _) = request(&fx, "session/new").unwrap();
    h.answer(conn, new, json!({ "sessionId": "s2" }));
    assert_eq!(h.hub.focused(), Some(&b));
    assert!(matches!(h.hub.view(&a).unwrap().turn, Turn::Prompting { .. }), "a keeps going in the background");
    h.update(conn, "s1", json!({ "sessionUpdate": "agent_message_chunk", "content": { "type": "text", "text": "for a" } }));
    h.update(conn, "s2", json!({ "sessionUpdate": "agent_message_chunk", "content": { "type": "text", "text": "for b" } }));
    assert!(matches!(&h.hub.view(&a).unwrap().thread.entries[1], Entry::Agent { text, .. } if text == "for a"));
    assert!(matches!(&h.hub.view(&b).unwrap().thread.entries[0], Entry::Agent { text, .. } if text == "for b"));
    assert_eq!(h.hub.status(), Status { working: 1, waiting: 0 });
}

#[test]
fn a_connection_with_no_live_session_stops_after_its_idle_time() {
    let mut h = Harness::new();
    h.hub.set_idle_timeout(Duration::from_secs(600));
    let (id, conn) = h.live_thread();
    let fx = h.hub.close(&id, h.now);
    assert!(requests(&fx).contains(&"session/close"));
    assert_eq!(h.hub.view(&id).unwrap().link, &Link::Detached);
    let now = h.later(599);
    assert!(h.hub.tick(now).is_empty());
    let now = h.later(2);
    assert!(h.hub.tick(now).contains(&Effect::Stop { conn }));
    assert!(h.hub.connection(conn).is_none());
}

#[test]
fn deleting_a_thread_deletes_its_session_and_its_file() {
    let mut h = Harness::new();
    let (id, _) = h.live_thread();
    let fx = h.hub.delete(&id, h.now);
    assert!(requests(&fx).contains(&"session/delete"));
    assert!(fx.contains(&Effect::Forget(id.clone())));
    assert!(h.hub.view(&id).is_none());
}

#[test]
fn listing_waits_for_the_agent_and_import_makes_a_thread_once() {
    let mut h = Harness::new();
    let fx = h.hub.list_sessions("claude", &root());
    let conn = spawned(&fx).unwrap();
    let fx = h.hub.on_spawned(conn, 1, Ok(()), h.now);
    let fx = h.answer(conn, request(&fx, "initialize").unwrap().0, claude_init());
    let (list, params) = request(&fx, "session/list").unwrap();
    assert_eq!(params["cwd"], json!("/work/fenix"));
    let fx = h.answer(conn, list, json!({ "sessions": [{ "sessionId": "x1", "cwd": "/work/fenix", "title": "From the CLI" }, { "sessionId": "x2", "cwd": "/elsewhere" }] }));
    let Some(Effect::Listed { sessions, .. }) = fx.into_iter().find(|e| matches!(e, Effect::Listed { .. })) else { panic!() };
    assert_eq!(sessions.len(), 1, "only this project's");
    let a = h.hub.import("claude", &root(), &sessions[0]);
    let b = h.hub.import("claude", &root(), &sessions[0]);
    assert_eq!(a, b);
    assert_eq!(h.hub.view(&a).unwrap().thread.display_title(), "From the CLI");
    assert_eq!(h.hub.view(&a).unwrap().meta.session_id.as_deref(), Some("x1"));
}

#[test]
fn an_agent_that_wont_start_says_why_on_the_thread() {
    let mut h = Harness::new();
    let id = h.hub.new_thread("claude", &root());
    let fx = h.hub.prompt(&id, texts(&["hi"]));
    let conn = spawned(&fx).unwrap();
    h.hub.on_spawned(conn, 1, Err("program not found".into()), h.now);
    assert!(notices(&h.hub, &id).iter().any(|(k, t)| *k == NoticeKind::Error && t.contains("program not found")));
    assert_eq!(h.hub.view(&id).unwrap().queued, 0);
    assert_eq!(h.hub.view(&id).unwrap().link, &Link::Detached);
}

#[test]
fn an_agent_that_is_not_set_up_leaves_the_thread_read_only() {
    let mut h = Harness::new();
    let id = h.hub.new_thread("gone", &root());
    h.hub.focus(&id);
    assert!(matches!(h.hub.view(&id).unwrap().link, Link::ReadOnly(_)));
}

#[test]
fn hand_off_carries_the_transcript_to_another_agent() {
    let mut h = Harness::new();
    let mut codex = claude();
    codex.id = "codex".into();
    h.hub.set_agents(vec![claude(), codex]);
    let (a, conn) = h.live_thread();
    let prompt = request(&h.hub.prompt(&a, texts(&["design the parser"])), "session/prompt").unwrap();
    h.update(conn, "s1", json!({ "sessionUpdate": "agent_message_chunk", "content": { "type": "text", "text": "Use a Pratt parser." } }));
    h.answer(conn, prompt.0, json!({ "stopReason": "end_turn" }));
    let b = h.hub.hand_off(&a, "codex", 64 * 1024).unwrap();
    assert_eq!(h.hub.meta(&b).unwrap().handed_off_from.as_ref(), Some(&a));
    let fx = h.hub.prompt(&b, texts(&["implement it"]));
    let codex_conn = spawned(&fx).unwrap();
    let fx = h.hub.on_spawned(codex_conn, 1, Ok(()), h.now);
    let fx = h.answer(codex_conn, request(&fx, "initialize").unwrap().0, claude_init());
    let fx = h.answer(codex_conn, request(&fx, "session/new").unwrap().0, json!({ "sessionId": "c1" }));
    let (_, params) = request(&fx, "session/prompt").unwrap();
    let transcript = params["prompt"][1]["resource"]["text"].as_str().unwrap();
    assert!(transcript.contains("design the parser") && transcript.contains("Pratt"));
    assert_eq!(params["prompt"][2]["text"], json!("implement it"));
}

#[test]
fn mcp_servers_name_their_thread_and_http_ones_need_the_agent_to_take_them() {
    let mut h = Harness::new();
    h.hub.set_mcp_servers(vec![
        McpServer::Stdio { name: "fenix".into(), command: "/opt/fenix".into(), args: vec!["--mcp".into(), "--thread".into(), "{thread}".into()], env: vec![] },
        McpServer::Http { name: "web".into(), url: "http://localhost:1".into(), headers: vec![] },
    ]);
    let id = h.hub.new_thread("claude", &root());
    let conn = spawned(&h.hub.focus(&id)).unwrap();
    let fx = h.hub.on_spawned(conn, 1, Ok(()), h.now);
    let fx = h.answer(conn, request(&fx, "initialize").unwrap().0, claude_init());
    let (_, params) = request(&fx, "session/new").unwrap();
    assert_eq!(params["mcpServers"].as_array().unwrap().len(), 1, "claude_init offers no http");
    assert_eq!(params["mcpServers"][0]["args"][2], json!(id.0));
}

#[test]
fn trimming_keeps_the_end_at_a_line() {
    let text = "first line\nsecond line\nthird line\n";
    let trimmed = trim_front(text, 15);
    assert!(trimmed.ends_with("third line\n") && !trimmed.contains("first"));
    assert_eq!(trim_front("short", 100), "short");
}
