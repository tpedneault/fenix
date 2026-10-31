//! A mock ACP agent, for tests: Fenix's own, the GUI's, and the cloud
//! sessions where no real agent can run (`docs/plans/agents-acp.md` §7).
//!
//! Its one profile, `claude`, plays what Claude Code's adapter,
//! claude-agent-acp 0.99.1, does on the wire: the handshake and its
//! capabilities, sessions with Claude's modes and config options, turns
//! that think, stream, call tools, ask permission, edit files on disk and
//! run commands. Each prompt picks what the turn does ([`script`]). Cues
//! ([`MockOptions`]) make it crash, log out, go slow, forget sessions, or
//! work the ways other agents do (`--fs`, `--terminal`, `--agent-auth`).
//!
//! [`run`] serves one connection, on any reader and writer: the
//! `fake-agent` binary gives it stdio, tests give it in-memory pipes. It
//! keeps nothing global, so several can run in one process.

mod config;
mod script;
mod store;
mod turn;

use std::collections::{HashMap, HashSet};
use std::io::{self, BufRead, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::Duration;

use serde_json::{json, Value};

use config::Settings;
use store::{Record, Store};

/// The longest line the mock reads: prompts can embed whole files.
const MAX_LINE: usize = 64 << 20;

/// The cues. Every field is a flag of `fake-agent`
/// (see [`MockOptions::from_args`]).
#[derive(Clone, Debug, Default)]
pub struct MockOptions {
    /// `--store DIR`: keep sessions as files there, across restarts.
    pub store: Option<PathBuf>,
    /// `--crash-on METHOD`: die on receiving METHOD, without answering.
    pub crash_on: Option<String>,
    /// `--crash-after N`: die after sending N `session/update`s.
    pub crash_after: Option<usize>,
    /// `--logged-out`: prompts fail `-32000` until `authenticate`.
    pub logged_out: bool,
    /// `--slow MS`: wait MS before each `session/update`.
    pub slow_ms: u64,
    /// `--no-replay`: `session/load` answers without replaying.
    pub no_replay: bool,
    /// `--forget-sessions`: `session/load` and `/resume` fail `-32002`.
    pub forget_sessions: bool,
    /// `--unannounced-write`: an edit also changes `UNANNOUNCED.txt`,
    /// with no tool call saying so.
    pub unannounced_write: bool,
    /// `--bypass`: offer the `bypassPermissions` mode.
    pub bypass: bool,
    /// `--fs`: read and write files through `fs/*`.
    pub fs: bool,
    /// `--terminal`: run commands through `terminal/*`.
    pub terminal: bool,
    /// `--agent-auth`: offer an `authenticate`-style login, `mock-login`.
    pub agent_auth: bool,
}

impl MockOptions {
    /// Reads `fake-agent`'s flags (without the program name).
    pub fn from_args<I: IntoIterator<Item = String>>(args: I) -> Result<Self, String> {
        let mut options = MockOptions::default();
        let mut args = args.into_iter();
        while let Some(flag) = args.next() {
            let mut value = || args.next().ok_or_else(|| format!("{flag} needs a value"));
            match flag.as_str() {
                "--profile" => {
                    let profile = value()?;
                    if profile != "claude" {
                        return Err(format!(
                            "unknown profile `{profile}` (there is only `claude`)"
                        ));
                    }
                }
                "--store" => options.store = Some(PathBuf::from(value()?)),
                "--crash-on" => options.crash_on = Some(value()?),
                "--crash-after" => {
                    let n = value()?;
                    options.crash_after = Some(
                        n.parse()
                            .map_err(|_| format!("--crash-after: not a number: {n}"))?,
                    );
                }
                "--slow" => {
                    let ms = value()?;
                    options.slow_ms = ms
                        .parse()
                        .map_err(|_| format!("--slow: not a number: {ms}"))?;
                }
                "--logged-out" => options.logged_out = true,
                "--no-replay" => options.no_replay = true,
                "--forget-sessions" => options.forget_sessions = true,
                "--unannounced-write" => options.unannounced_write = true,
                "--bypass" => options.bypass = true,
                "--fs" => options.fs = true,
                "--terminal" => options.terminal = true,
                "--agent-auth" => options.agent_auth = true,
                other => return Err(format!("unknown flag `{other}`")),
            }
        }
        Ok(options)
    }
}

/// Serves one client on `input`/`output` until the input closes (`Ok`) or
/// a crash cue fires (`Err`, having stopped writing at once, as a dead
/// process would).
pub fn run<R: BufRead + Send + 'static, W: Write + Send + 'static>(
    input: R,
    output: W,
    options: MockOptions,
) -> io::Result<()> {
    let (events, inbox) = mpsc::channel();
    let agent = Arc::new(Agent::new(options, Box::new(output), events.clone()));
    // Reading on a thread of its own, so that a crash cue raised while a
    // turn runs ends `run` without waiting for the client's next line.
    std::thread::spawn(move || read_loop(input, events));
    let result = agent.serve(inbox);
    agent.wind_down(result.is_ok());
    result
}

enum Event {
    Message(Value),
    Garbled,
    Closed,
    Failed(io::Error),
    Crash(String),
}

fn read_loop<R: BufRead>(mut input: R, events: mpsc::Sender<Event>) {
    loop {
        let event = match fenix_rpc::read_line_message_limited(&mut input, MAX_LINE) {
            Ok(Some(line)) => match serde_json::from_slice(&line) {
                Ok(message) => Event::Message(message),
                Err(_) => Event::Garbled,
            },
            Ok(None) => Event::Closed,
            Err(error) => Event::Failed(error),
        };
        let last = matches!(event, Event::Closed | Event::Failed(_));
        if events.send(event).is_err() || last {
            return;
        }
    }
}

/// A JSON-RPC error to answer with.
struct RpcError {
    code: i64,
    message: String,
    data: Option<Value>,
}

impl RpcError {
    fn new(code: i64, message: impl Into<String>) -> Self {
        RpcError {
            code,
            message: message.into(),
            data: None,
        }
    }

    fn invalid_params(message: impl Into<String>) -> Self {
        Self::new(-32602, message)
    }

    fn session_not_found(id: &str) -> Self {
        RpcError {
            data: Some(json!({"sessionId": id})),
            ..Self::new(-32002, "Session not found")
        }
    }

    fn session_ended() -> Self {
        Self::new(
            -32603,
            "The Claude Agent session has ended. Please start a new session.",
        )
    }

    fn to_json(&self) -> Value {
        let mut error = json!({"code": self.code, "message": self.message});
        if let Some(data) = &self.data {
            error["data"] = data.clone();
        }
        error
    }
}

/// A live session: its record, and what only this process knows.
struct Session {
    record: Record,
    settings: Settings,
    /// Commands the client allowed "for the session", by first word.
    allowed_commands: HashSet<String>,
    /// Turns take this in order: a prompt sent during a turn waits for it,
    /// as Claude's prompt queueing does.
    queue: Arc<Mutex<()>>,
    /// Turns running or queued, for `session/cancel`.
    turns: Vec<Arc<TurnCtl>>,
}

/// A turn's handle, shared between its worker and the dispatcher.
struct TurnCtl {
    cancel: AtomicBool,
    request: Value,
}

/// Why a request the mock sent got no result.
enum CallError {
    /// The turn was cancelled first; the request was withdrawn.
    Cancelled(u64),
    /// The client answered with an error.
    Failed(Value),
    /// The connection is gone.
    Gone,
}

struct Agent {
    options: MockOptions,
    out: Mutex<Box<dyn Write + Send>>,
    events: Mutex<mpsc::Sender<Event>>,
    /// Set by a crash cue, and on closing: nothing is written after it.
    dead: AtomicBool,
    updates_sent: AtomicUsize,
    next_request: AtomicU64,
    /// Requests sent to the client, waiting for its answer.
    pending: Mutex<HashMap<u64, mpsc::Sender<Result<Value, Value>>>>,
    sessions: Mutex<HashMap<String, Session>>,
    /// Sessions closed in this process: a prompt to one has "ended".
    closed: Mutex<HashSet<String>>,
    store: Store,
    client_capabilities: Mutex<Value>,
    logged_out: AtomicBool,
    ids: AtomicU64,
    last_stamp: AtomicU64,
    workers: Mutex<Vec<JoinHandle<()>>>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl Agent {
    fn new(options: MockOptions, out: Box<dyn Write + Send>, events: mpsc::Sender<Event>) -> Self {
        Agent {
            store: Store::new(options.store.clone()),
            logged_out: AtomicBool::new(options.logged_out),
            options,
            out: Mutex::new(out),
            events: Mutex::new(events),
            dead: AtomicBool::new(false),
            updates_sent: AtomicUsize::new(0),
            next_request: AtomicU64::new(0),
            pending: Mutex::new(HashMap::new()),
            sessions: Mutex::new(HashMap::new()),
            closed: Mutex::new(HashSet::new()),
            client_capabilities: Mutex::new(Value::Null),
            ids: AtomicU64::new(0),
            last_stamp: AtomicU64::new(0),
            workers: Mutex::new(Vec::new()),
        }
    }

    fn serve(self: &Arc<Self>, inbox: mpsc::Receiver<Event>) -> io::Result<()> {
        for event in inbox {
            match event {
                Event::Message(message) => self.dispatch(message)?,
                Event::Garbled => {
                    self.send(json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": "Parse error"}}))?
                }
                Event::Closed => return Ok(()),
                Event::Failed(error) => return Err(error),
                Event::Crash(why) => return Err(io::Error::other(why)),
            }
        }
        Ok(())
    }

    /// Ends every turn: on a clean close, waits for them (so what they
    /// stored is on disk when `run` returns); on a crash, leaves them to
    /// fail on their next write.
    fn wind_down(&self, clean: bool) {
        if !clean {
            self.dead.store(true, Ordering::SeqCst);
        }
        for session in lock(&self.sessions).values() {
            for turn in &session.turns {
                turn.cancel.store(true, Ordering::SeqCst);
            }
        }
        // Dropping the waiting senders wakes every worker blocked on a reply.
        lock(&self.pending).clear();
        if clean {
            let workers = std::mem::take(&mut *lock(&self.workers));
            for worker in workers {
                let _ = worker.join();
            }
        }
    }

    fn crash(&self, why: String) {
        self.dead.store(true, Ordering::SeqCst);
        let _ = lock(&self.events).send(Event::Crash(why));
    }

    fn options(&self) -> &MockOptions {
        &self.options
    }

    fn send(&self, message: Value) -> io::Result<()> {
        if self.dead.load(Ordering::SeqCst) {
            return Err(io::ErrorKind::BrokenPipe.into());
        }
        let body = serde_json::to_vec(&message).map_err(io::Error::other)?;
        fenix_rpc::write_line_message(&mut *lock(&self.out), &body)
    }

    fn reply(&self, id: &Value, result: Result<Value, RpcError>) -> io::Result<()> {
        match result {
            Ok(result) => self.send(json!({"jsonrpc": "2.0", "id": id, "result": result})),
            Err(error) => self.send(json!({"jsonrpc": "2.0", "id": id, "error": error.to_json()})),
        }
    }

    fn notify(&self, method: &str, params: Value) -> io::Result<()> {
        self.send(json!({"jsonrpc": "2.0", "method": method, "params": params}))
    }

    /// Sends one `session/update`, after the `--slow` pause, and counts it
    /// for `--crash-after`.
    fn update(&self, session: &str, update: Value) -> io::Result<()> {
        if self.options.slow_ms > 0 {
            std::thread::sleep(Duration::from_millis(self.options.slow_ms));
        }
        let limit = self.options.crash_after;
        if limit == Some(0) {
            self.crash("crash cue: --crash-after 0".into());
            return Err(io::ErrorKind::BrokenPipe.into());
        }
        self.notify(
            "session/update",
            json!({"sessionId": session, "update": update}),
        )?;
        let sent = self.updates_sent.fetch_add(1, Ordering::SeqCst) + 1;
        if limit == Some(sent) {
            self.crash(format!("crash cue: --crash-after {sent}"));
        }
        Ok(())
    }

    /// Sends a request to the client and waits for its answer, giving up
    /// (and leaving the answer to be dropped) if `cancel` is set first.
    fn call(&self, method: &str, params: Value, cancel: &AtomicBool) -> Result<Value, CallError> {
        let id = self.next_request.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = mpsc::channel();
        lock(&self.pending).insert(id, tx);
        if self
            .send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
            .is_err()
        {
            lock(&self.pending).remove(&id);
            return Err(CallError::Gone);
        }
        loop {
            match rx.recv_timeout(Duration::from_millis(10)) {
                Ok(Ok(result)) => return Ok(result),
                Ok(Err(error)) => return Err(CallError::Failed(error)),
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if cancel.load(Ordering::SeqCst) {
                        lock(&self.pending).remove(&id);
                        return Err(CallError::Cancelled(id));
                    }
                    if self.dead.load(Ordering::SeqCst) {
                        return Err(CallError::Gone);
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => return Err(CallError::Gone),
            }
        }
    }

    /// Sends a request whose answer nobody waits for (a kill or a release
    /// after a cancel).
    fn call_and_forget(&self, method: &str, params: Value) -> io::Result<()> {
        let id = self.next_request.fetch_add(1, Ordering::SeqCst);
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
    }

    /// A fresh id shaped like a UUID: session and message ids.
    fn uuid(&self) -> String {
        let n = self.ids.fetch_add(1, Ordering::SeqCst);
        let seed = store::now_nanos()
            ^ (u64::from(std::process::id()) << 40)
            ^ n.wrapping_mul(0x9e37_79b9_7f4a_7c15);
        let (a, b) = (mix(seed), mix(seed ^ 0xa5a5_a5a5_a5a5_a5a5));
        format!(
            "{:08x}-{:04x}-4{:03x}-{:04x}-{:012x}",
            a >> 32,
            (a >> 16) & 0xffff,
            a & 0xfff,
            0x8000 | (b >> 48) & 0x3fff,
            b & 0xffff_ffff_ffff
        )
    }

    /// Now, strictly after every earlier stamp, so "newest first" is
    /// well defined even for sessions made in the same instant.
    fn stamp(&self) -> u64 {
        let now = store::now_nanos();
        let mut last = self.last_stamp.load(Ordering::SeqCst);
        loop {
            let next = now.max(last + 1);
            match self
                .last_stamp
                .compare_exchange(last, next, Ordering::SeqCst, Ordering::SeqCst)
            {
                Ok(_) => return next,
                Err(seen) => last = seen,
            }
        }
    }

    fn boolean_options(&self) -> bool {
        !lock(&self.client_capabilities)["session"]["configOptions"]["boolean"].is_null()
    }

    fn config_options(&self, settings: &Settings) -> Value {
        config::config_options(settings, self.options.bypass, self.boolean_options())
    }

    fn sessions(&self) -> MutexGuard<'_, HashMap<String, Session>> {
        lock(&self.sessions)
    }

    fn store(&self) -> &Store {
        &self.store
    }

    fn dispatch(self: &Arc<Self>, message: Value) -> io::Result<()> {
        let Some(method) = message["method"].as_str().map(str::to_string) else {
            // An answer to one of the mock's own requests.
            if let Some(waiting) = message["id"]
                .as_u64()
                .and_then(|id| lock(&self.pending).remove(&id))
            {
                let answer = match message.get("error") {
                    Some(error) => Err(error.clone()),
                    None => Ok(message.get("result").cloned().unwrap_or(Value::Null)),
                };
                let _ = waiting.send(answer);
            }
            return Ok(());
        };
        if self.options.crash_on.as_deref() == Some(method.as_str()) {
            self.dead.store(true, Ordering::SeqCst);
            return Err(io::Error::other(format!("crash cue: --crash-on {method}")));
        }
        let params = message.get("params").cloned().unwrap_or(Value::Null);
        let Some(id) = message.get("id").filter(|id| !id.is_null()).cloned() else {
            self.notification(&method, &params);
            return Ok(());
        };
        let result = match method.as_str() {
            "initialize" => return self.initialize(&id, &params),
            "authenticate" => self.authenticate(&params),
            "logout" => {
                self.logged_out.store(true, Ordering::SeqCst);
                self.reply(&id, Ok(json!({})))?;
                return self.auth_status();
            }
            "session/new" => return self.new_session(&id, &params, None),
            "session/fork" => return self.fork_session(&id, &params),
            "session/load" => return self.load_session(&id, &params, true),
            "session/resume" => return self.load_session(&id, &params, false),
            "session/list" => self.list_sessions(&params),
            "session/close" => Ok(self.close_session(&params, false)),
            "session/delete" => Ok(self.close_session(&params, true)),
            "session/set_mode" => return self.set_mode(&id, &params),
            "session/set_config_option" => return self.set_config_option(&id, &params),
            "session/prompt" => return self.prompt(id, &params),
            _ => Err(RpcError::new(-32601, format!("Method not found: {method}"))),
        };
        self.reply(&id, result)
    }

    fn notification(&self, method: &str, params: &Value) {
        let cancel = |matches: &dyn Fn(&TurnCtl) -> bool| {
            for session in self.sessions().values() {
                for turn in session.turns.iter().filter(|turn| matches(turn)) {
                    turn.cancel.store(true, Ordering::SeqCst);
                }
            }
        };
        match method {
            "session/cancel" => {
                let id = params["sessionId"].as_str().unwrap_or_default().to_string();
                if let Some(session) = self.sessions().get(&id) {
                    for turn in &session.turns {
                        turn.cancel.store(true, Ordering::SeqCst);
                    }
                }
            }
            // The client no longer wants a prompt's answer: a cancel too.
            "$/cancel_request" => cancel(&|turn| turn.request == params["requestId"]),
            // Extension notifications (`_…`) and anything else: ignored.
            _ => {}
        }
    }

    fn initialize(&self, id: &Value, params: &Value) -> io::Result<()> {
        let capabilities = params["clientCapabilities"].clone();
        *lock(&self.client_capabilities) = capabilities.clone();
        let auth_methods = if self.options.agent_auth {
            json!([{"id": "mock-login", "name": "Mock login", "description": "Log in to the mock agent"}])
        } else if capabilities["auth"]["terminal"] == true {
            json!([
                {"id": "claude-ai-login", "name": "Claude Subscription", "description": "Use Claude subscription ",
                 "type": "terminal", "args": ["--cli", "auth", "login", "--claudeai"]},
                {"id": "console-login", "name": "Anthropic Console", "description": "Use Anthropic Console (API usage billing)",
                 "type": "terminal", "args": ["--cli", "auth", "login", "--console"]},
            ])
        } else {
            json!([])
        };
        let result = json!({
            "protocolVersion": 1,
            "agentCapabilities": {
                "_meta": {"claudeCode": {"promptQueueing": true}, "authStatus": {}},
                "promptCapabilities": {"image": true, "embeddedContext": true},
                "mcpCapabilities": {"http": true, "sse": true},
                "auth": {"logout": {}},
                "loadSession": true,
                "sessionCapabilities": {
                    "additionalDirectories": {}, "close": {}, "delete": {}, "fork": {}, "list": {}, "resume": {},
                },
            },
            "agentInfo": {"name": "@agentclientprotocol/claude-agent-acp", "title": "Claude Agent", "version": "0.99.1"},
            "authMethods": auth_methods,
        });
        self.reply(id, Ok(result))?;
        // The adapter probes who is logged in after answering, and pushes it.
        self.auth_status()
    }

    fn auth_status(&self) -> io::Result<()> {
        let status = if self.logged_out.load(Ordering::SeqCst) {
            json!({"kind": "none", "label": "Not logged in"})
        } else {
            json!({"kind": "account", "label": "Claude Max", "account": {"email": "mock@example.com"}})
        };
        self.notify("_auth/status_update", json!({"authStatus": status}))
    }

    fn authenticate(&self, params: &Value) -> Result<Value, RpcError> {
        match params["methodId"].as_str() {
            Some("mock-login") if self.options.agent_auth => {
                self.logged_out.store(false, Ordering::SeqCst);
                let _ = self.auth_status();
                Ok(json!({}))
            }
            // Claude's terminal logins happen in the terminal, not here.
            Some("claude-ai-login" | "console-login") => {
                Err(RpcError::new(-32603, "Method not implemented."))
            }
            other => Err(RpcError::invalid_params(format!(
                "Unknown auth method: {}",
                other.unwrap_or_default()
            ))),
        }
    }

    /// Makes a session live from `record` (fresh when `None`) and answers
    /// with its modes and options, then its commands.
    fn new_session(&self, id: &Value, params: &Value, from: Option<Record>) -> io::Result<()> {
        let Some(cwd) = params["cwd"].as_str() else {
            return self.reply(id, Err(RpcError::invalid_params("cwd is required")));
        };
        let mut record = from.unwrap_or_default();
        record.session_id = self.uuid();
        record.cwd = PathBuf::from(cwd);
        let stamp = self.stamp();
        record.updated_nanos = stamp;
        record.updated_at = store::iso(stamp);
        let session_id = record.session_id.clone();
        self.go_live(record);
        self.answer_session(id, &session_id)
    }

    fn fork_session(&self, id: &Value, params: &Value) -> io::Result<()> {
        let source = params["sessionId"].as_str().unwrap_or_default();
        let record = match self.sessions().get(source) {
            Some(session) => Some(session.record.clone()),
            None => self.store.get(source),
        };
        match record {
            Some(record) => self.new_session(id, params, Some(record)),
            None => self.reply(id, Err(RpcError::session_not_found(source))),
        }
    }

    fn go_live(&self, record: Record) {
        let mut settings = record.settings();
        if !config::is_mode(&settings.mode, self.options.bypass) {
            settings.mode = "default".into();
        }
        lock(&self.closed).remove(&record.session_id);
        let session = Session {
            settings,
            allowed_commands: HashSet::new(),
            queue: Arc::new(Mutex::new(())),
            turns: Vec::new(),
            record,
        };
        self.sessions()
            .insert(session.record.session_id.clone(), session);
    }

    fn answer_session(&self, id: &Value, session_id: &str) -> io::Result<()> {
        let settings = match self.sessions().get(session_id) {
            Some(session) => session.settings.clone(),
            None => return Ok(()),
        };
        self.reply(
            id,
            Ok(json!({
                "sessionId": session_id,
                "modes": config::mode_state(&settings, self.options.bypass),
                "configOptions": self.config_options(&settings),
            })),
        )?;
        self.update(session_id, json!({"sessionUpdate": "available_commands_update", "availableCommands": config::commands()}))
    }

    /// `session/load` (replaying) and `session/resume` (not).
    fn load_session(&self, id: &Value, params: &Value, replay: bool) -> io::Result<()> {
        let session_id = params["sessionId"].as_str().unwrap_or_default().to_string();
        if self.options.forget_sessions {
            return self.reply(id, Err(RpcError::session_not_found(&session_id)));
        }
        let live = self
            .sessions()
            .get(&session_id)
            .map(|session| session.record.clone());
        let Some(mut record) = live.or_else(|| self.store.get(&session_id)) else {
            return self.reply(id, Err(RpcError::session_not_found(&session_id)));
        };
        if let Some(cwd) = params["cwd"].as_str() {
            record.cwd = PathBuf::from(cwd);
        }
        let history = record.history.clone();
        if !self.sessions().contains_key(&session_id) {
            self.go_live(record);
        }
        if replay && !self.options.no_replay {
            for update in history {
                self.update(&session_id, update)?;
            }
        }
        self.answer_session(id, &session_id)
    }

    fn list_sessions(&self, params: &Value) -> Result<Value, RpcError> {
        const PAGE: usize = 1000;
        let offset = match params["cursor"].as_str() {
            None => 0,
            Some(cursor) => cursor
                .strip_prefix("offset:")
                .and_then(|n| n.parse::<usize>().ok())
                .ok_or_else(|| RpcError::invalid_params(format!("Invalid cursor: {cursor}")))?,
        };
        let cwd = params["cwd"].as_str().map(PathBuf::from);
        let matching: Vec<Record> = self
            .store
            .all()
            .into_iter()
            .filter(|record| cwd.as_ref().is_none_or(|cwd| &record.cwd == cwd))
            .collect();
        let sessions: Vec<Value> = matching
            .iter()
            .skip(offset)
            .take(PAGE)
            .map(|record| {
                json!({"sessionId": record.session_id, "cwd": record.cwd, "title": record.title, "updatedAt": record.updated_at})
            })
            .collect();
        let mut result = json!({"sessions": sessions});
        if matching.len() > offset + PAGE {
            result["nextCursor"] = json!(format!("offset:{}", offset + PAGE));
        }
        Ok(result)
    }

    /// `session/close` and `/delete`: `{}` whether known or not.
    fn close_session(&self, params: &Value, delete: bool) -> Value {
        let session_id = params["sessionId"].as_str().unwrap_or_default().to_string();
        if let Some(session) = self.sessions().remove(&session_id) {
            for turn in &session.turns {
                turn.cancel.store(true, Ordering::SeqCst);
            }
            lock(&self.closed).insert(session_id.clone());
        }
        if delete {
            self.store.delete(&session_id);
        }
        json!({})
    }

    fn set_mode(&self, id: &Value, params: &Value) -> io::Result<()> {
        let session_id = params["sessionId"].as_str().unwrap_or_default();
        let mode = params["modeId"].as_str().unwrap_or_default();
        if !self.sessions().contains_key(session_id) {
            return self.reply(id, Err(self.missing(session_id)));
        }
        if !config::is_mode(mode, self.options.bypass) {
            return self.reply(
                id,
                Err(RpcError::invalid_params(format!("Invalid Mode: {mode}"))),
            );
        }
        self.reply(id, Ok(json!({})))?;
        self.switch_mode(session_id, mode)
    }

    /// Puts a session in `mode` and says so, as the adapter does on
    /// `set_mode` and on leaving plan mode.
    fn switch_mode(&self, session_id: &str, mode: &str) -> io::Result<()> {
        let settings = {
            let mut sessions = self.sessions();
            let Some(session) = sessions.get_mut(session_id) else {
                return Ok(());
            };
            session.settings.mode = mode.to_string();
            session.record.set_settings(&session.settings);
            session.settings.clone()
        };
        self.update(
            session_id,
            json!({"sessionUpdate": "current_mode_update", "currentModeId": mode}),
        )?;
        self.update(
            session_id,
            json!({"sessionUpdate": "config_option_update", "configOptions": self.config_options(&settings)}),
        )
    }

    fn set_config_option(&self, id: &Value, params: &Value) -> io::Result<()> {
        let session_id = params["sessionId"].as_str().unwrap_or_default();
        let option = params["configId"].as_str().unwrap_or_default();
        let outcome = {
            let mut sessions = self.sessions();
            match sessions.get_mut(session_id) {
                None => Err(self.missing(session_id)),
                Some(session) => {
                    let mut settings = session.settings.clone();
                    match config::set_option(
                        &mut settings,
                        option,
                        &params["value"],
                        self.options.bypass,
                    ) {
                        Err(why) => Err(RpcError::invalid_params(why)),
                        Ok(()) => {
                            let mode_changed = settings.mode != session.settings.mode;
                            session.settings = settings.clone();
                            session.record.set_settings(&settings);
                            Ok((settings, mode_changed))
                        }
                    }
                }
            }
        };
        match outcome {
            Err(error) => self.reply(id, Err(error)),
            Ok((settings, mode_changed)) => {
                self.reply(
                    id,
                    Ok(json!({"configOptions": self.config_options(&settings)})),
                )?;
                if mode_changed {
                    self.update(session_id, json!({"sessionUpdate": "current_mode_update", "currentModeId": settings.mode}))?;
                }
                Ok(())
            }
        }
    }

    fn missing(&self, session_id: &str) -> RpcError {
        if lock(&self.closed).contains(session_id) {
            RpcError::session_ended()
        } else {
            RpcError::session_not_found(session_id)
        }
    }

    /// Starts a turn on a worker thread, which answers when it ends; the
    /// dispatcher goes on reading (cancels, permission answers, other
    /// sessions) meanwhile.
    fn prompt(self: &Arc<Self>, id: Value, params: &Value) -> io::Result<()> {
        let session_id = params["sessionId"].as_str().unwrap_or_default().to_string();
        let turn = Arc::new(TurnCtl {
            cancel: AtomicBool::new(false),
            request: id.clone(),
        });
        let queue = {
            let mut sessions = self.sessions();
            match sessions.get_mut(&session_id) {
                None => None,
                Some(session) => {
                    session.turns.push(turn.clone());
                    Some(session.queue.clone())
                }
            }
        };
        let Some(queue) = queue else {
            return self.reply(&id, Err(self.missing(&session_id)));
        };
        if self.logged_out.load(Ordering::SeqCst) {
            self.turn_ended(&session_id, &turn);
            return self.reply(&id, Err(RpcError::new(-32000, "Authentication required")));
        }
        let prompt = params["prompt"].as_array().cloned().unwrap_or_default();
        let agent = self.clone();
        let worker = std::thread::spawn(move || turn::run(agent, session_id, turn, prompt, queue));
        let mut workers = lock(&self.workers);
        workers.retain(|worker| !worker.is_finished());
        workers.push(worker);
        Ok(())
    }

    fn turn_ended(&self, session_id: &str, turn: &Arc<TurnCtl>) {
        if let Some(session) = self.sessions().get_mut(session_id) {
            session.turns.retain(|other| !Arc::ptr_eq(other, turn));
        }
    }

    fn answer(&self, id: &Value, result: Value) -> io::Result<()> {
        self.reply(id, Ok(result))
    }

    fn answer_ended(&self, id: &Value) -> io::Result<()> {
        self.reply(id, Err(RpcError::session_ended()))
    }
}

/// splitmix64's finalizer: enough to make ids look random.
fn mix(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}
