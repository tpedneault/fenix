//! One connection to an agent: its process (and everything it starts),
//! JSON-RPC over its stdio a line at a time, and a log of both.
//!
//! In the shape of `fenix_lsp::LspClient`: the client writes and reads,
//! and nothing else. Which request an answer belongs to, and what it
//! means, is the hub's (`hub.rs`); the client hands every message up as
//! an `AcpEvent`, from a reader thread, through the `send` callback the
//! caller gives it (the GUI forwards them to its event loop). Request ids
//! are the caller's too, so the hub, which is pure, can choose them.

use crate::protocol::code;
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The longest line read from an agent: 16 MiB, since agents put whole
/// files in a diff or a replayed conversation.
pub const MAX_LINE: usize = 16 * 1024 * 1024;

/// How many log lines a connection keeps (`SPC h a`).
const LOG_LINES: usize = 4000;

/// The program name that runs the mock agent inside Fenix
/// (`AcpClient::start`).
pub const MOCK_PROGRAM: &str = "fenix:mock";

/// A JSON-RPC error, either side's.
#[derive(Clone, Debug, PartialEq)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    pub data: Option<Value>,
}

impl RpcError {
    pub fn new(code: i64, message: impl Into<String>) -> Self {
        Self { code, message: message.into(), data: None }
    }

    fn parse(v: &Value) -> Self {
        Self {
            code: v.get("code").and_then(Value::as_i64).unwrap_or(code::INTERNAL_ERROR),
            message: v.get("message").and_then(Value::as_str).unwrap_or("unknown error").to_string(),
            data: v.get("data").cloned(),
        }
    }

    fn to_value(&self) -> Value {
        let mut v = json!({ "code": self.code, "message": self.message });
        if let Some(data) = &self.data {
            v["data"] = data.clone();
        }
        v
    }
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.message, self.code)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum AcpEvent {
    /// The answer to a request Fenix sent, by the id it gave.
    Response { id: i64, result: Result<Value, RpcError> },
    Notification { method: String, params: Value },
    /// A request from the agent; answered with `respond(id, ..)`.
    Request { id: Value, method: String, params: Value },
    /// The agent is gone: its output closed, or what it sent can't be
    /// read as messages. Why, with its exit status when there is one.
    /// Always the last event.
    Disconnected(String),
}

/// What to run for an agent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Launch {
    pub program: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub cwd: PathBuf,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogSide {
    /// Fenix sent it.
    Out,
    /// The agent sent it.
    In,
    /// The agent's stderr.
    Stderr,
    /// Fenix's own note: started, stopped, a line it couldn't read.
    Note,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogLine {
    pub side: LogSide,
    pub text: String,
}

#[derive(Default)]
pub struct Log {
    lines: VecDeque<LogLine>,
}

impl Log {
    fn push(&mut self, side: LogSide, text: &str) {
        if self.lines.len() == LOG_LINES {
            self.lines.pop_front();
        }
        // A replayed conversation is one line of megabytes: the log keeps
        // its start.
        let text = if text.len() > 2000 { format!("{}… ({} bytes)", &text[..floor_char_boundary(text, 2000)], text.len()) } else { text.to_string() };
        self.lines.push_back(LogLine { side, text: redact(&text) });
    }

    pub fn lines(&self) -> impl Iterator<Item = &LogLine> {
        self.lines.iter()
    }
}

fn floor_char_boundary(s: &str, mut i: usize) -> usize {
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Hides what looks like a credential: API keys (`sk-…`, `sk-ant-…`,
/// `ghp_…`, `xox…`), and what follows `Bearer `, `token`, `apiKey`,
/// `password`, `secret`. What's left is enough to debug a conversation.
pub fn redact(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    'outer: while !rest.is_empty() {
        for prefix in ["sk-ant-", "sk-", "ghp_", "gho_", "github_pat_", "xoxb-", "xoxp-", "AKIA"] {
            if rest.starts_with(prefix) && !out.ends_with(|c: char| c.is_ascii_alphanumeric()) {
                let len = rest.find(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_')).unwrap_or(rest.len());
                if len >= prefix.len() + 12 {
                    out.push_str(prefix);
                    out.push_str("••••");
                    rest = &rest[len..];
                    continue 'outer;
                }
            }
        }
        if let Some(after) = rest.strip_prefix("Bearer ") {
            let len = after.find(|c: char| c.is_whitespace() || c == '"').unwrap_or(after.len());
            out.push_str("Bearer ••••");
            rest = &after[len..];
            continue;
        }
        let lower: String = rest.chars().take(12).collect::<String>().to_ascii_lowercase();
        for key in ["\"token\"", "\"apikey\"", "\"api_key\"", "\"password\"", "\"secret\""] {
            if lower.starts_with(key) {
                // "token": "value" -> "token": "••••"
                let key_len = key.len();
                let after = &rest[key_len..];
                if let Some(colon) = after.find(':').filter(|&i| after[..i].trim().is_empty()) {
                    let value = &after[colon + 1..];
                    let trimmed = value.trim_start();
                    if let Some(body) = trimmed.strip_prefix('"') {
                        if let Some(end) = body.find('"') {
                            out.push_str(&rest[..key_len + colon + 1]);
                            out.push_str(&value[..value.len() - trimmed.len()]);
                            out.push_str("\"••••\"");
                            rest = &body[end + 1..];
                            continue 'outer;
                        }
                    }
                }
            }
        }
        let c = rest.chars().next().unwrap();
        out.push(c);
        rest = &rest[c.len_utf8()..];
    }
    out
}

pub struct AcpClient {
    writer: Mutex<Box<dyn Write + Send>>,
    tree: Arc<Mutex<Option<fenix_tasks::ProcessTree>>>,
    log: Arc<Mutex<Log>>,
}

impl AcpClient {
    /// Starts the agent and its reader threads. `send` gets every event
    /// and returns false once nobody wants them any more; the reader then
    /// stops. The process is killed, whole, when the client is dropped.
    pub fn spawn(launch: &Launch, send: impl FnMut(AcpEvent) -> bool + Send + 'static) -> io::Result<Self> {
        let mut command = Command::new(fenix_rpc::resolve_command(&launch.program));
        command.args(&launch.args).current_dir(&launch.cwd).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        for (k, v) in &launch.env {
            command.env(k, v);
        }
        let mut tree = fenix_tasks::ProcessTree::spawn(command)?;
        let stdin = tree.take_stdin().ok_or_else(|| io::Error::other("the agent has no stdin"))?;
        let stdout = tree.take_stdout().ok_or_else(|| io::Error::other("the agent has no stdout"))?;
        let stderr = tree.take_stderr().ok_or_else(|| io::Error::other("the agent has no stderr"))?;
        let log = Arc::new(Mutex::new(Log::default()));
        log.lock().unwrap().push(LogSide::Note, &format!("started {} {} (pid {}) in {}", launch.program, launch.args.join(" "), tree.id(), launch.cwd.display()));
        let tree = Arc::new(Mutex::new(Some(tree)));
        let stderr_log = log.clone();
        std::thread::Builder::new().name("fenix-acp-stderr".into()).spawn(move || {
            for line in BufReader::new(stderr).lines() {
                let Ok(line) = line else { break };
                stderr_log.lock().unwrap().push(LogSide::Stderr, &line);
            }
        })?;
        let client = Self { writer: Mutex::new(Box::new(stdin)), tree: tree.clone(), log: log.clone() };
        start_reader(BufReader::new(stdout), log, Some(tree), send)?;
        Ok(client)
    }

    /// Starts an agent: the program `launch` names, or, for
    /// [`MOCK_PROGRAM`], the mock in this process (`launch.args` are its
    /// flags) -- for tests, and for trying Fenix's agent UI with no agent
    /// installed (`agents.servers`: `mock = "fenix:mock --profile claude"`).
    pub fn start(launch: &Launch, send: impl FnMut(AcpEvent) -> bool + Send + 'static) -> io::Result<Self> {
        if launch.program != MOCK_PROGRAM {
            return Self::spawn(launch, send);
        }
        let options = crate::mock::MockOptions::from_args(launch.args.iter().cloned()).map_err(io::Error::other)?;
        let (to_agent_read, to_agent_write) = io::pipe()?;
        let (from_agent_read, from_agent_write) = io::pipe()?;
        let client = Self::connect(from_agent_read, to_agent_write, send)?;
        client.note(&format!("started the mock agent in Fenix ({})", launch.args.join(" ")));
        std::thread::Builder::new().name("fenix-acp-mock".into()).spawn(move || {
            // Its error (a crash cue) closes its output, which the reader
            // reports as the agent going away.
            let _ = crate::mock::run(BufReader::new(to_agent_read), from_agent_write, options);
        })?;
        Ok(client)
    }

    /// A client over streams already open: an agent running in this
    /// process (the mock, in tests).
    pub fn connect<R: Read + Send + 'static, W: Write + Send + 'static>(reader: R, writer: W, send: impl FnMut(AcpEvent) -> bool + Send + 'static) -> io::Result<Self> {
        let log = Arc::new(Mutex::new(Log::default()));
        let client = Self { writer: Mutex::new(Box::new(writer)), tree: Arc::new(Mutex::new(None)), log: log.clone() };
        start_reader(BufReader::new(reader), log, None, send)?;
        Ok(client)
    }

    fn write(&self, message: &Value) -> io::Result<()> {
        let body = serde_json::to_vec(message)?;
        self.log.lock().unwrap().push(LogSide::Out, &String::from_utf8_lossy(&body));
        fenix_rpc::write_line_message(&mut *self.writer.lock().unwrap(), &body)
    }

    pub fn request(&self, id: i64, method: &str, params: Value) -> io::Result<()> {
        self.write(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }))
    }

    pub fn notify(&self, method: &str, params: Value) -> io::Result<()> {
        self.write(&json!({ "jsonrpc": "2.0", "method": method, "params": params }))
    }

    pub fn respond(&self, id: Value, result: Result<Value, RpcError>) -> io::Result<()> {
        match result {
            Ok(result) => self.write(&json!({ "jsonrpc": "2.0", "id": id, "result": result })),
            Err(error) => self.write(&json!({ "jsonrpc": "2.0", "id": id, "error": error.to_value() })),
        }
    }

    /// `$/cancel_request`: Fenix no longer wants the answer to `id`.
    pub fn cancel_request(&self, id: i64) -> io::Result<()> {
        self.notify("$/cancel_request", json!({ "requestId": id }))
    }

    /// Notes something in the connection's log.
    pub fn note(&self, text: &str) {
        self.log.lock().unwrap().push(LogSide::Note, text);
    }

    pub fn log(&self) -> Arc<Mutex<Log>> {
        self.log.clone()
    }

    /// Stops the agent, and everything it started.
    pub fn kill(&self) {
        if let Some(mut tree) = self.tree.lock().unwrap().take() {
            tree.kill();
        }
    }
}

impl Drop for AcpClient {
    fn drop(&mut self) {
        self.kill();
    }
}

fn start_reader<R: BufRead + Send + 'static>(
    mut reader: R,
    log: Arc<Mutex<Log>>,
    tree: Option<Arc<Mutex<Option<fenix_tasks::ProcessTree>>>>,
    mut send: impl FnMut(AcpEvent) -> bool + Send + 'static,
) -> io::Result<()> {
    std::thread::Builder::new().name("fenix-acp-reader".into()).spawn(move || {
        let why = loop {
            let body = match fenix_rpc::read_line_message_limited(&mut reader, MAX_LINE) {
                Ok(Some(body)) => body,
                Ok(None) => break "the agent closed its output".to_string(),
                Err(e) => break format!("couldn't read from the agent: {e}"),
            };
            log.lock().unwrap().push(LogSide::In, &String::from_utf8_lossy(&body));
            let Ok(message) = serde_json::from_slice::<Value>(&body) else {
                // Not JSON: something printed to stdout that shouldn't
                // have been. Logged, and the conversation goes on.
                log.lock().unwrap().push(LogSide::Note, "a line that isn't JSON was skipped");
                continue;
            };
            let Some(event) = classify(message) else {
                log.lock().unwrap().push(LogSide::Note, "a message that isn't JSON-RPC was skipped");
                continue;
            };
            if !send(event) {
                return;
            }
        };
        let status = tree.and_then(|tree| exit_status(&tree));
        let why = match status {
            Some(status) => format!("{why} ({status})"),
            None => why,
        };
        log.lock().unwrap().push(LogSide::Note, &why);
        send(AcpEvent::Disconnected(why));
    })?;
    Ok(())
}

/// The agent's exit status, waiting a moment for it to be reaped.
fn exit_status(tree: &Mutex<Option<fenix_tasks::ProcessTree>>) -> Option<String> {
    let deadline = Instant::now() + Duration::from_millis(500);
    loop {
        let status = tree.lock().unwrap().as_mut().and_then(|t| t.try_wait().ok().flatten());
        if let Some(status) = status {
            return Some(match status.code() {
                Some(code) => format!("exit code {code}"),
                None => status.to_string(),
            });
        }
        if Instant::now() > deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn classify(message: Value) -> Option<AcpEvent> {
    let method = message.get("method").and_then(Value::as_str).map(str::to_string);
    let id = message.get("id").filter(|id| !id.is_null()).cloned();
    match (method, id) {
        (Some(method), Some(id)) => Some(AcpEvent::Request { id, method, params: message.get("params").cloned().unwrap_or(Value::Null) }),
        (Some(method), None) => Some(AcpEvent::Notification { method, params: message.get("params").cloned().unwrap_or(Value::Null) }),
        (None, Some(id)) => {
            // Fenix only sends numeric ids; a string one is from a confused
            // agent and is matched as well as it can be.
            let id = id.as_i64().or_else(|| id.as_str().and_then(|s| s.parse().ok()))?;
            let result = match message.get("error") {
                Some(error) if !error.is_null() => Err(RpcError::parse(error)),
                _ => Ok(message.get("result").cloned().unwrap_or(Value::Null)),
            };
            Some(AcpEvent::Response { id, result })
        }
        (None, None) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_are_told_apart_by_method_and_id() {
        assert_eq!(classify(json!({ "jsonrpc": "2.0", "id": 3, "result": { "ok": true } })), Some(AcpEvent::Response { id: 3, result: Ok(json!({ "ok": true })) }));
        assert_eq!(
            classify(json!({ "jsonrpc": "2.0", "id": 4, "error": { "code": -32000, "message": "Authentication required" } })),
            Some(AcpEvent::Response { id: 4, result: Err(RpcError::new(-32000, "Authentication required")) })
        );
        assert!(matches!(classify(json!({ "jsonrpc": "2.0", "id": "a", "method": "fs/read_text_file", "params": {} })), Some(AcpEvent::Request { .. })));
        assert!(matches!(classify(json!({ "jsonrpc": "2.0", "method": "session/update", "params": {} })), Some(AcpEvent::Notification { .. })));
        assert_eq!(classify(json!({ "jsonrpc": "2.0" })), None);
    }

    #[test]
    fn credentials_are_hidden_in_the_log() {
        assert_eq!(redact("key sk-ant-api03-abcdefghijklmnopqrstuvwxyz end"), "key sk-ant-•••• end");
        assert_eq!(redact("Authorization: Bearer abc.def.ghi rest"), "Authorization: Bearer •••• rest");
        assert_eq!(redact(r#"{"token": "hunter2", "x": 1}"#), r#"{"token": "••••", "x": 1}"#);
        assert_eq!(redact("task-list and sk-short stay"), "task-list and sk-short stay");
        assert_eq!(redact("é sk-"), "é sk-");
    }

    #[test]
    fn a_long_line_is_logged_by_its_start() {
        let mut log = Log::default();
        log.push(LogSide::In, &"é".repeat(3000));
        let line = log.lines().next().unwrap();
        assert!(line.text.ends_with("(6000 bytes)"));
    }
}
