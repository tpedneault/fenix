//! The mock agent on the wire: `mock::run` on a thread, spoken to over
//! in-memory pipes with raw JSON, so these tests check exactly what Fenix's
//! client will read, and nothing of fenix-acp's own client is involved.

use std::io::{self, BufRead, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;
use std::time::Duration;

use fenix_acp::mock::{run, MockOptions};
use serde_json::{json, Value};

/// The read end of an in-memory pipe.
struct PipeReader {
    rx: Receiver<Vec<u8>>,
    buf: Vec<u8>,
    pos: usize,
}

impl Read for PipeReader {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        let available = self.fill_buf()?;
        let n = available.len().min(out.len());
        out[..n].copy_from_slice(&available[..n]);
        self.consume(n);
        Ok(n)
    }
}

impl BufRead for PipeReader {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        if self.pos == self.buf.len() {
            // A closed writer is the end of the stream.
            self.buf = self.rx.recv().unwrap_or_default();
            self.pos = 0;
        }
        Ok(&self.buf[self.pos..])
    }

    fn consume(&mut self, n: usize) {
        self.pos += n;
    }
}

/// The write end of an in-memory pipe.
struct PipeWriter(Sender<Vec<u8>>);

impl Write for PipeWriter {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.0
            .send(data.to_vec())
            .map_err(|_| io::Error::from(io::ErrorKind::BrokenPipe))?;
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn pipe() -> (PipeWriter, PipeReader) {
    let (tx, rx) = mpsc::channel();
    (
        PipeWriter(tx),
        PipeReader {
            rx,
            buf: Vec::new(),
            pos: 0,
        },
    )
}

/// A client: writes requests, reads every message the mock sends, and
/// keeps what it read in `log`.
struct Client {
    input: Option<PipeWriter>,
    messages: Receiver<Value>,
    agent: Option<JoinHandle<io::Result<()>>>,
    next_id: u64,
    log: Vec<Value>,
}

const TIMEOUT: Duration = Duration::from_secs(20);

impl Client {
    fn start(args: &[&str]) -> Client {
        let options =
            MockOptions::from_args(args.iter().map(|arg| arg.to_string())).expect("flags");
        let (input, agent_input) = pipe();
        let (agent_output, mut output) = pipe();
        let agent = std::thread::spawn(move || run(agent_input, agent_output, options));
        let (tx, messages) = mpsc::channel();
        std::thread::spawn(move || {
            while let Ok(Some(line)) = fenix_rpc::read_line_message_limited(&mut output, 1 << 26) {
                let message: Value = serde_json::from_slice(&line).expect("the mock writes JSON");
                if tx.send(message).is_err() {
                    return;
                }
            }
        });
        Client {
            input: Some(input),
            messages,
            agent: Some(agent),
            next_id: 1000,
            log: Vec::new(),
        }
    }

    fn send(&mut self, message: Value) {
        let body = serde_json::to_vec(&message).unwrap();
        fenix_rpc::write_line_message(self.input.as_mut().unwrap(), &body).unwrap();
    }

    fn request(&mut self, method: &str, params: Value) -> u64 {
        self.next_id += 1;
        let id = self.next_id;
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        id
    }

    fn notify(&mut self, method: &str, params: Value) {
        self.send(json!({"jsonrpc": "2.0", "method": method, "params": params}));
    }

    /// Answers a request from the mock.
    fn answer(&mut self, request: &Value, result: Value) {
        self.send(json!({"jsonrpc": "2.0", "id": request["id"], "result": result}));
    }

    /// Reads until a message matches, logging every one read.
    fn wait(&mut self, what: &str, matches: impl Fn(&Value) -> bool) -> Value {
        loop {
            let message = self
                .messages
                .recv_timeout(TIMEOUT)
                .unwrap_or_else(|_| panic!("no {what}; read: {:#?}", self.log));
            self.log.push(message.clone());
            if matches(&message) {
                return message;
            }
        }
    }

    fn response(&mut self, id: u64) -> Value {
        self.wait(&format!("response {id}"), |m| {
            m["id"] == id && m.get("method").is_none()
        })
    }

    fn agent_request(&mut self, method: &str) -> Value {
        self.wait(method, |m| m["method"] == method && m.get("id").is_some())
    }

    fn call(&mut self, method: &str, params: Value) -> Value {
        let id = self.request(method, params);
        self.response(id)
    }

    fn result(&mut self, method: &str, params: Value) -> Value {
        let response = self.call(method, params);
        assert!(
            response.get("error").is_none(),
            "{method} failed: {response}"
        );
        response["result"].clone()
    }

    fn initialize(&mut self, capabilities: Value) -> Value {
        self.result("initialize", json!({"protocolVersion": 1, "clientCapabilities": capabilities, "clientInfo": {"name": "test", "version": "0"}}))
    }

    fn new_session(&mut self, cwd: &Path) -> String {
        self.initialize(json!({}));
        let result = self.result("session/new", json!({"cwd": cwd, "mcpServers": []}));
        result["sessionId"].as_str().unwrap().to_string()
    }

    /// `session/update`s logged since `from` for `session`.
    fn updates_since(&self, from: usize, session: &str) -> Vec<Value> {
        self.log[from..]
            .iter()
            .filter(|m| m["method"] == "session/update" && m["params"]["sessionId"] == session)
            .map(|m| m["params"]["update"].clone())
            .collect()
    }

    /// Closes the mock's input and returns how `run` ended.
    fn finish(mut self) -> io::Result<()> {
        self.input = None;
        self.agent.take().unwrap().join().unwrap()
    }

    /// How `run` ended, without closing its input (after a crash cue).
    fn ended(&mut self) -> io::Result<()> {
        self.agent.take().unwrap().join().unwrap()
    }
}

/// A fresh folder for one test.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Scratch {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, Ordering::SeqCst);
        let dir =
            std::env::temp_dir().join(format!("fenix-mock-wire-{}-{name}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Scratch(dir.canonicalize().unwrap())
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn file(&self, name: &str, text: &str) -> PathBuf {
        let path = self.0.join(name);
        std::fs::write(&path, text).unwrap();
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn prompt(client: &mut Client, session: &str, text: &str) -> u64 {
    client.request(
        "session/prompt",
        json!({"sessionId": session, "prompt": [{"type": "text", "text": text}]}),
    )
}

fn kinds(updates: &[Value]) -> Vec<&str> {
    updates
        .iter()
        .map(|u| u["sessionUpdate"].as_str().unwrap())
        .collect()
}

#[test]
fn handshake_is_claudes() {
    let mut client = Client::start(&[]);
    let result = client.result(
        "initialize",
        json!({"protocolVersion": 3, "clientCapabilities": {}}),
    );
    assert_eq!(result["protocolVersion"], 1);
    assert_eq!(
        result["agentInfo"],
        json!({"name": "@agentclientprotocol/claude-agent-acp", "title": "Claude Agent", "version": "0.99.1"})
    );
    let caps = &result["agentCapabilities"];
    assert_eq!(caps["loadSession"], true);
    assert_eq!(
        caps["promptCapabilities"],
        json!({"image": true, "embeddedContext": true})
    );
    assert_eq!(caps["mcpCapabilities"], json!({"http": true, "sse": true}));
    assert_eq!(caps["auth"], json!({"logout": {}}));
    assert_eq!(caps["_meta"]["claudeCode"]["promptQueueing"], true);
    for capability in [
        "list",
        "resume",
        "close",
        "delete",
        "fork",
        "additionalDirectories",
    ] {
        assert!(
            caps["sessionCapabilities"][capability].is_object(),
            "{capability}"
        );
    }
    assert_eq!(result["authMethods"], json!([]));
    let status = client.wait("auth status", |m| m["method"] == "_auth/status_update");
    assert_eq!(status["params"]["authStatus"]["kind"], "account");

    // Unknown methods, extension requests, and extension notifications.
    assert_eq!(
        client.call("nope/nothing", json!({}))["error"]["code"],
        -32601
    );
    client.notify("_fenix/whatever", json!({}));
    assert_eq!(
        client.call("_fenix/ask", json!({}))["error"]["code"],
        -32601
    );
    client.finish().unwrap();
}

#[test]
fn auth_methods_only_for_terminal_auth() {
    let mut client = Client::start(&[]);
    let result = client.initialize(json!({"auth": {"terminal": true}}));
    let methods = result["authMethods"].as_array().unwrap();
    let ids: Vec<&str> = methods.iter().map(|m| m["id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["claude-ai-login", "console-login"]);
    assert!(methods.iter().all(|m| m["type"] == "terminal"));
    assert_eq!(methods[0]["name"], "Claude Subscription");
    assert_eq!(
        methods[0]["args"],
        json!(["--cli", "auth", "login", "--claudeai"])
    );
    assert_eq!(
        methods[1]["args"],
        json!(["--cli", "auth", "login", "--console"])
    );
    let failed = client.call("authenticate", json!({"methodId": "claude-ai-login"}));
    assert_eq!(failed["error"]["message"], "Method not implemented.");
    client.finish().unwrap();

    let mut client = Client::start(&["--agent-auth"]);
    let result = client.initialize(json!({}));
    assert_eq!(
        result["authMethods"],
        json!([{"id": "mock-login", "name": "Mock login", "description": "Log in to the mock agent"}])
    );
    client.finish().unwrap();
}

#[test]
fn new_session_and_hello_streams() {
    let dir = Scratch::new("hello");
    let mut client = Client::start(&[]);
    client.initialize(json!({}));
    let result = client.result("session/new", json!({"cwd": dir.path(), "mcpServers": []}));
    let session = result["sessionId"].as_str().unwrap().to_string();
    assert_eq!(result["modes"]["currentModeId"], "default");
    let modes: Vec<&str> = result["modes"]["availableModes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["id"].as_str().unwrap())
        .collect();
    assert_eq!(modes, ["default", "acceptEdits", "plan", "auto"]);
    let options = result["configOptions"].as_array().unwrap();
    let ids: Vec<(&str, &str)> = options
        .iter()
        .map(|o| (o["id"].as_str().unwrap(), o["category"].as_str().unwrap()))
        .collect();
    assert_eq!(
        ids,
        [
            ("mode", "mode"),
            ("model", "model"),
            ("effort", "thought_level"),
            ("fast", "model_config")
        ]
    );
    assert_eq!(options[1]["options"][0]["value"], "default");
    assert_eq!(options[3]["type"], "select");
    let commands = client.wait("commands", |m| {
        m["params"]["update"]["sessionUpdate"] == "available_commands_update"
    });
    let names: Vec<&str> = commands["params"]["update"]["availableCommands"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert!(
        names.contains(&"compact") && names.contains(&"mcp"),
        "{names:?}"
    );

    let from = client.log.len();
    let id = prompt(&mut client, &session, "hello there");
    let response = client.response(id);
    assert_eq!(response["result"]["stopReason"], "end_turn");
    let usage = &response["result"]["usage"];
    assert_eq!(
        usage["totalTokens"],
        usage["inputTokens"].as_u64().unwrap() + usage["outputTokens"].as_u64().unwrap()
    );
    let updates = client.updates_since(from, &session);
    assert_eq!(updates[0]["sessionUpdate"], "session_info_update");
    assert_eq!(updates[0]["title"], "hello there");
    assert_eq!(updates[1]["sessionUpdate"], "agent_thought_chunk");
    let answer: Vec<&Value> = updates
        .iter()
        .filter(|u| u["sessionUpdate"] == "agent_message_chunk")
        .collect();
    assert!(answer.len() >= 3);
    assert!(answer
        .iter()
        .all(|u| u["messageId"] == answer[0]["messageId"] && u["messageId"].is_string()));
    let text: String = answer
        .iter()
        .map(|u| u["content"]["text"].as_str().unwrap())
        .collect();
    assert_eq!(text, "Hello! How can I help you today?");
    let usage = updates.last().unwrap();
    assert_eq!(usage["sessionUpdate"], "usage_update");
    assert_eq!(usage["cost"]["currency"], "USD");
    assert!(usage["used"].as_u64().unwrap() > 0 && usage["size"].as_u64().unwrap() > 0);

    // A second turn has no new title; unknown text is echoed.
    let from = client.log.len();
    let id = prompt(&mut client, &session, "what now?");
    client.response(id);
    let updates = client.updates_since(from, &session);
    assert_eq!(
        kinds(&updates),
        ["agent_message_chunk", "agent_message_chunk", "usage_update"]
    );
    assert_eq!(updates[1]["content"]["text"], "what now?");

    // Unknown sessions; closing an unknown one is fine.
    let missing = client.call("session/prompt", json!({"sessionId": "nope", "prompt": []}));
    assert_eq!(missing["error"]["code"], -32002);
    assert_eq!(
        client.result("session/close", json!({"sessionId": "nope"})),
        json!({})
    );
    assert_eq!(
        client.result("session/close", json!({"sessionId": session})),
        json!({})
    );
    let ended = client.call(
        "session/prompt",
        json!({"sessionId": session, "prompt": []}),
    );
    assert_eq!(ended["error"]["code"], -32603);
    client.finish().unwrap();
}

#[test]
fn edit_asks_then_writes_to_disk() {
    let dir = Scratch::new("edit");
    let file = dir.file("a.rs", "fn main() {\n    foo();\n}\n");
    let mut client = Client::start(&[]);
    let session = client.new_session(dir.path());
    let from = client.log.len();
    let id = prompt(&mut client, &session, "edit a.rs foo=>bar");
    let ask = client.agent_request("session/request_permission");
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "fn main() {\n    foo();\n}\n",
        "not before permission"
    );
    assert_eq!(ask["params"]["sessionId"], session.as_str());
    let options: Vec<&str> = ask["params"]["options"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["optionId"].as_str().unwrap())
        .collect();
    assert_eq!(options, ["allow-once", "allow-with-updates", "reject"]);
    assert_eq!(ask["params"]["toolCall"]["title"], "Edit a.rs");
    client.answer(
        &ask,
        json!({"outcome": {"outcome": "selected", "optionId": "allow-once"}}),
    );
    let response = client.response(id);
    assert_eq!(response["result"]["stopReason"], "end_turn");
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "fn main() {\n    bar();\n}\n"
    );

    let updates = client.updates_since(from, &session);
    let call = updates
        .iter()
        .find(|u| u["sessionUpdate"] == "tool_call")
        .unwrap();
    assert_eq!(call["status"], "pending");
    assert_eq!(call["kind"], "edit");
    assert_eq!(call["name"], "Edit");
    assert_eq!(call["_meta"]["claudeCode"]["toolName"], "Edit");
    assert_eq!(call["rawInput"], json!({"file_path": file}));
    let full = updates
        .iter()
        .find(|u| u["sessionUpdate"] == "tool_call_update" && u["rawInput"]["old_string"] == "foo")
        .unwrap();
    assert_eq!(
        full["content"][0],
        json!({"type": "diff", "path": file, "oldText": "foo", "newText": "bar"})
    );
    let done = updates.iter().find(|u| u["status"] == "completed").unwrap();
    assert_eq!(done["content"][0]["oldText"], "fn main() {\n    foo();\n}");
    assert_eq!(done["content"][0]["newText"], "fn main() {\n    bar();\n}");
    assert_eq!(done["locations"], json!([{"path": file, "line": 1}]));

    // "For the session" switches to acceptEdits; then edits don't ask.
    let id = prompt(&mut client, &session, "edit a.rs bar=>baz");
    let ask = client.agent_request("session/request_permission");
    client.answer(
        &ask,
        json!({"outcome": {"outcome": "selected", "optionId": "allow-with-updates"}}),
    );
    client.wait("mode", |m| {
        m["params"]["update"]["currentModeId"] == "acceptEdits"
    });
    client.response(id);
    let from = client.log.len();
    let id = prompt(&mut client, &session, "write sub/new.txt one\\ntwo");
    client.response(id);
    assert_eq!(
        std::fs::read_to_string(dir.path().join("sub/new.txt")).unwrap(),
        "one\ntwo"
    );
    assert!(!client.log[from..]
        .iter()
        .any(|m| m["method"] == "session/request_permission"));
    let updates = client.updates_since(from, &session);
    let call = updates
        .iter()
        .find(|u| u["sessionUpdate"] == "tool_call_update" && u["content"].is_array())
        .unwrap();
    assert_eq!(call["content"][0]["oldText"], Value::Null);
    // The path as the platform writes it, as Claude's adapter does.
    let shown = format!("Write {}", std::path::Path::new("sub").join("new.txt").display());
    assert_eq!(call["title"], shown.as_str());

    // A refusal fails the tool and ends the turn without writing.
    client.result(
        "session/set_mode",
        json!({"sessionId": session, "modeId": "default"}),
    );
    let id = prompt(&mut client, &session, "edit a.rs baz=>nope");
    let ask = client.agent_request("session/request_permission");
    client.answer(
        &ask,
        json!({"outcome": {"outcome": "selected", "optionId": "reject"}}),
    );
    assert_eq!(client.response(id)["result"]["stopReason"], "end_turn");
    assert!(std::fs::read_to_string(&file).unwrap().contains("baz();"));
    client.finish().unwrap();
}

#[test]
fn unannounced_write_and_plan_mode() {
    let dir = Scratch::new("sneaky");
    dir.file("a.txt", "x\n");
    let mut client = Client::start(&["--unannounced-write"]);
    let session = client.new_session(dir.path());
    client.result(
        "session/set_mode",
        json!({"sessionId": session, "modeId": "plan"}),
    );
    // Plan mode refuses edits.
    let from = client.log.len();
    let id = prompt(&mut client, &session, "edit a.txt x=>y");
    client.response(id);
    assert!(client
        .updates_since(from, &session)
        .iter()
        .any(|u| u["status"] == "failed"));
    assert_eq!(
        std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
        "x\n"
    );
    // Approving the plan switches the mode, and says so.
    let id = prompt(&mut client, &session, "plan");
    let ask = client.agent_request("session/request_permission");
    assert_eq!(ask["params"]["toolCall"]["title"], "Approve Plan");
    assert_eq!(ask["params"]["toolCall"]["kind"], "switch_mode");
    let options: Vec<&str> = ask["params"]["options"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["optionId"].as_str().unwrap())
        .collect();
    assert_eq!(
        options,
        [
            "exit-plan-auto",
            "exit-plan-accept-edits",
            "exit-plan-default",
            "reject"
        ]
    );
    client.answer(
        &ask,
        json!({"outcome": {"outcome": "selected", "optionId": "exit-plan-accept-edits"}}),
    );
    client.wait("mode", |m| {
        m["params"]["update"]["currentModeId"] == "acceptEdits"
    });
    let options = client.wait("options", |m| {
        m["params"]["update"]["sessionUpdate"] == "config_option_update"
    });
    assert_eq!(
        options["params"]["update"]["configOptions"][0]["currentValue"],
        "acceptEdits"
    );
    client.response(id);
    // Now edits go through, and the sneaky file appears with them.
    let id = prompt(&mut client, &session, "edit a.txt x=>y");
    client.response(id);
    assert_eq!(
        std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
        "y\n"
    );
    assert!(dir.path().join("UNANNOUNCED.txt").exists());
    client.finish().unwrap();
}

#[test]
fn fs_cue_reads_and_writes_through_the_client() {
    let dir = Scratch::new("fs");
    let file = dir.file("a.txt", "on disk\n");
    let mut client = Client::start(&["--fs"]);
    let session = client.new_session(dir.path());
    client.result(
        "session/set_mode",
        json!({"sessionId": session, "modeId": "acceptEdits"}),
    );
    let id = prompt(
        &mut client,
        &session,
        "read a.txt; edit a.txt buffer=>edited",
    );
    for _ in 0..2 {
        let read = client.agent_request("fs/read_text_file");
        assert_eq!(read["params"]["path"], json!(file));
        client.answer(&read, json!({"content": "in the buffer\n"}));
    }
    let write = client.agent_request("fs/write_text_file");
    assert_eq!(
        write["params"],
        json!({"sessionId": session, "path": file, "content": "in the edited\n"})
    );
    client.answer(&write, json!(null));
    assert_eq!(client.response(id)["result"]["stopReason"], "end_turn");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "on disk\n");
    let read = client
        .log
        .iter()
        .find(|m| m["params"]["update"]["status"] == "completed")
        .unwrap();
    assert_eq!(
        read["params"]["update"]["content"][0]["content"]["text"],
        "```\n1\tin the buffer\n```"
    );
    client.finish().unwrap();
}

#[test]
fn run_locally_and_through_terminals() {
    let dir = Scratch::new("run");
    let mut client = Client::start(&[]);
    let session = client.new_session(dir.path());
    let from = client.log.len();
    let id = prompt(&mut client, &session, "run echo hi");
    let ask = client.agent_request("session/request_permission");
    assert_eq!(ask["params"]["toolCall"]["title"], "echo hi");
    client.answer(
        &ask,
        json!({"outcome": {"outcome": "selected", "optionId": "allow-once"}}),
    );
    client.response(id);
    let updates = client.updates_since(from, &session);
    let call = updates
        .iter()
        .find(|u| u["sessionUpdate"] == "tool_call")
        .unwrap();
    assert_eq!(
        (call["kind"].as_str(), call["title"].as_str()),
        (Some("execute"), Some("echo hi"))
    );
    let done = updates.iter().find(|u| u["status"] == "completed").unwrap();
    assert_eq!(done["content"][0]["content"]["text"], "```console\nhi\n```");
    client.finish().unwrap();

    let mut client = Client::start(&["--terminal"]);
    let session = client.new_session(dir.path());
    client.result(
        "session/set_mode",
        json!({"sessionId": session, "modeId": "auto"}),
    );
    let from = client.log.len();
    let id = prompt(&mut client, &session, "run cargo test");
    let create = client.agent_request("terminal/create");
    assert_eq!(
        create["params"]["args"].as_array().unwrap().last().unwrap(),
        "cargo test"
    );
    assert_eq!(create["params"]["cwd"], json!(dir.path()));
    client.answer(&create, json!({"terminalId": "term-1"}));
    let wait = client.agent_request("terminal/wait_for_exit");
    assert_eq!(wait["params"]["terminalId"], "term-1");
    client.answer(&wait, json!({"exitCode": 0, "signal": null}));
    let output = client.agent_request("terminal/output");
    client.answer(
        &output,
        json!({"output": "ok\n", "truncated": false, "exitStatus": {"exitCode": 0}}),
    );
    let release = client.agent_request("terminal/release");
    client.answer(&release, json!({}));
    assert_eq!(client.response(id)["result"]["stopReason"], "end_turn");
    let updates = client.updates_since(from, &session);
    let done = updates.iter().find(|u| u["status"] == "completed").unwrap();
    assert_eq!(
        done["content"],
        json!([{"type": "terminal", "terminalId": "term-1"}])
    );
    client.finish().unwrap();
}

#[test]
fn cancel_ends_a_slow_turn() {
    let dir = Scratch::new("slow");
    let mut client = Client::start(&[]);
    let session = client.new_session(dir.path());
    let id = prompt(&mut client, &session, "slow");
    client.wait("a chunk", |m| {
        m["params"]["update"]["sessionUpdate"] == "agent_message_chunk"
    });
    // The mock still answers other requests during the turn.
    assert_eq!(
        client.result(
            "session/set_mode",
            json!({"sessionId": session, "modeId": "auto"})
        ),
        json!({})
    );
    client.notify("session/cancel", json!({"sessionId": session}));
    assert_eq!(client.response(id)["result"]["stopReason"], "cancelled");
    let chunks = client
        .log
        .iter()
        .filter(|m| m["params"]["update"]["sessionUpdate"] == "agent_message_chunk")
        .count();
    assert!(chunks < 60, "{chunks} chunks: the turn ran to its end");
    client.finish().unwrap();
}

#[test]
fn cancel_withdraws_a_pending_permission() {
    let dir = Scratch::new("cancel");
    let file = dir.file("a.txt", "x\n");
    let mut client = Client::start(&[]);
    let session = client.new_session(dir.path());
    let id = prompt(&mut client, &session, "edit a.txt x=>y; hello");
    let ask = client.agent_request("session/request_permission");
    client.notify("session/cancel", json!({"sessionId": session}));
    let withdrawn = client.wait("withdrawal", |m| m["method"] == "$/cancel_request");
    assert_eq!(withdrawn["params"]["requestId"], ask["id"]);
    let response = client.response(id);
    assert_eq!(response["result"]["stopReason"], "cancelled");
    // The client's own "cancelled" answer, late, is dropped harmlessly.
    client.answer(&ask, json!({"outcome": {"outcome": "cancelled"}}));
    let failed = client
        .log
        .iter()
        .find(|m| m["params"]["update"]["status"] == "failed");
    assert!(failed.is_some());
    assert!(!client
        .log
        .iter()
        .any(|m| m["params"]["update"]["sessionUpdate"] == "agent_message_chunk"));
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "x\n");
    client.finish().unwrap();
}

#[test]
fn load_replays_across_restarts_and_resume_does_not() {
    let dir = Scratch::new("load");
    let store = Scratch::new("store");
    dir.file("a.txt", "one\ntwo\n");
    let store_arg = store.path().to_str().unwrap().to_string();

    let mut client = Client::start(&["--store", &store_arg]);
    let session = client.new_session(dir.path());
    let from = client.log.len();
    let id = prompt(&mut client, &session, "hello");
    client.response(id);
    let id = prompt(&mut client, &session, "read a.txt; todo");
    client.response(id);
    let replayable = [
        "agent_message_chunk",
        "agent_thought_chunk",
        "tool_call",
        "tool_call_update",
        "plan",
    ];
    let live: Vec<Value> = client
        .updates_since(from, &session)
        .into_iter()
        .filter(|u| replayable.contains(&u["sessionUpdate"].as_str().unwrap()))
        .collect();
    client.finish().unwrap();

    let mut client = Client::start(&["--store", &store_arg]);
    client.initialize(json!({}));
    let list = client.result("session/list", json!({"cwd": dir.path()}));
    assert_eq!(list["sessions"][0]["sessionId"], session.as_str());
    assert_eq!(list["sessions"][0]["title"], "hello");
    assert!(list["sessions"][0]["updatedAt"]
        .as_str()
        .unwrap()
        .ends_with('Z'));
    let from = client.log.len();
    let id = client.request(
        "session/load",
        json!({"sessionId": session, "cwd": dir.path(), "mcpServers": []}),
    );
    let response = client.response(id);
    assert_eq!(response["result"]["sessionId"], session.as_str());
    let replay = client.updates_since(from, &session);
    // Each turn: the user's message, then what the agent sent, in order.
    assert_eq!(replay[0]["sessionUpdate"], "user_message_chunk");
    assert_eq!(replay[0]["content"]["text"], "hello");
    let second = replay
        .iter()
        .position(|u| {
            u["sessionUpdate"] == "user_message_chunk" && u["content"]["text"] == "read a.txt; todo"
        })
        .unwrap();
    let agent: Vec<Value> = replay
        .iter()
        .filter(|u| u["sessionUpdate"] != "user_message_chunk")
        .cloned()
        .collect();
    assert_eq!(agent, live);
    assert!(replay[1..second]
        .iter()
        .all(|u| u["sessionUpdate"] != "user_message_chunk"));
    assert!(replay
        .iter()
        .filter(|u| u["sessionUpdate"].as_str().unwrap().ends_with("chunk"))
        .all(|u| u["messageId"].is_string()));
    // Commands come after the answer.
    client.wait("commands", |m| {
        m["params"]["update"]["sessionUpdate"] == "available_commands_update"
    });
    // The loaded session takes prompts.
    let id = prompt(&mut client, &session, "again");
    assert_eq!(client.response(id)["result"]["stopReason"], "end_turn");
    client.finish().unwrap();

    // Resume answers without replaying.
    let mut client = Client::start(&["--store", &store_arg]);
    client.initialize(json!({}));
    let from = client.log.len();
    let id = client.request(
        "session/resume",
        json!({"sessionId": session, "cwd": dir.path()}),
    );
    client.response(id);
    assert!(client.updates_since(from, &session).is_empty());
    assert_eq!(
        client.call(
            "session/resume",
            json!({"sessionId": "gone", "cwd": dir.path()})
        )["error"]["code"],
        -32002
    );
    client.finish().unwrap();

    // --no-replay loads silently; --forget-sessions knows nothing.
    let mut client = Client::start(&["--store", &store_arg, "--no-replay"]);
    client.initialize(json!({}));
    let from = client.log.len();
    let id = client.request(
        "session/load",
        json!({"sessionId": session, "cwd": dir.path(), "mcpServers": []}),
    );
    client.response(id);
    assert!(client.updates_since(from, &session).is_empty());
    client.finish().unwrap();
    let mut client = Client::start(&["--store", &store_arg, "--forget-sessions"]);
    client.initialize(json!({}));
    let failed = client.call(
        "session/load",
        json!({"sessionId": session, "cwd": dir.path(), "mcpServers": []}),
    );
    assert_eq!(failed["error"]["code"], -32002);
    assert_eq!(failed["error"]["message"], "Session not found");
    client.finish().unwrap();
}

#[test]
fn list_filters_orders_and_pages() {
    let a = Scratch::new("list-a");
    let b = Scratch::new("list-b");
    let mut client = Client::start(&[]);
    client.initialize(json!({}));
    let mut ids = Vec::new();
    for (cwd, text) in [
        (a.path(), "first"),
        (b.path(), "elsewhere"),
        (a.path(), "second"),
        (a.path(), "third"),
    ] {
        let session = client.result("session/new", json!({"cwd": cwd, "mcpServers": []}))
            ["sessionId"]
            .as_str()
            .unwrap()
            .to_string();
        let id = prompt(&mut client, &session, text);
        client.response(id);
        ids.push(session);
    }
    let list = client.result("session/list", json!({"cwd": a.path()}));
    let titles: Vec<&str> = list["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["title"].as_str().unwrap())
        .collect();
    assert_eq!(titles, ["third", "second", "first"]);
    assert_eq!(list["sessions"][0]["cwd"], json!(a.path()));
    assert!(list.get("nextCursor").is_none());
    let page = client.result(
        "session/list",
        json!({"cwd": a.path(), "cursor": "offset:1"}),
    );
    assert_eq!(page["sessions"].as_array().unwrap().len(), 2);
    assert_eq!(page["sessions"][0]["sessionId"], ids[2].as_str());
    assert_eq!(
        client.result("session/list", json!({}))["sessions"]
            .as_array()
            .unwrap()
            .len(),
        4
    );
    assert_eq!(
        client.call("session/list", json!({"cursor": "page 2"}))["error"]["code"],
        -32602
    );
    // Deleting removes it from the list.
    assert_eq!(
        client.result("session/delete", json!({"sessionId": ids[3]})),
        json!({})
    );
    assert_eq!(
        client.result("session/list", json!({"cwd": a.path()}))["sessions"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    client.finish().unwrap();
}

#[test]
fn modes_and_config_options() {
    let dir = Scratch::new("modes");
    let mut client = Client::start(&["--bypass"]);
    client.initialize(json!({"session": {"configOptions": {"boolean": {}}}}));
    let result = client.result("session/new", json!({"cwd": dir.path(), "mcpServers": []}));
    let session = result["sessionId"].as_str().unwrap().to_string();
    assert_eq!(
        result["modes"]["availableModes"][4]["id"],
        "bypassPermissions"
    );
    assert_eq!(
        result["configOptions"][3],
        json!({"id": "fast", "name": "Fast mode", "description": "Faster responses on supported models",
        "category": "model_config", "type": "boolean", "currentValue": false})
    );

    let bad = client.call(
        "session/set_mode",
        json!({"sessionId": session, "modeId": "nope"}),
    );
    assert_eq!(bad["error"]["code"], -32602);
    assert_eq!(bad["error"]["message"], "Invalid Mode: nope");
    assert_eq!(
        client.result(
            "session/set_mode",
            json!({"sessionId": session, "modeId": "plan"})
        ),
        json!({})
    );
    client.wait("mode", |m| m["params"]["update"]["currentModeId"] == "plan");
    let options = client.wait("options", |m| {
        m["params"]["update"]["sessionUpdate"] == "config_option_update"
    });
    assert_eq!(
        options["params"]["update"]["configOptions"][0]["currentValue"],
        "plan"
    );

    let set = client.result(
        "session/set_config_option",
        json!({"sessionId": session, "configId": "model", "value": "sonnet"}),
    );
    assert_eq!(set["configOptions"][1]["currentValue"], "sonnet");
    let set = client.result(
        "session/set_config_option",
        json!({"sessionId": session, "configId": "fast", "type": "boolean", "value": true}),
    );
    assert_eq!(set["configOptions"][3]["currentValue"], true);
    let bad = client.call(
        "session/set_config_option",
        json!({"sessionId": session, "configId": "model", "value": "gpt"}),
    );
    assert_eq!(bad["error"]["code"], -32602);
    let bad = client.call(
        "session/set_config_option",
        json!({"sessionId": session, "configId": "colour", "value": "red"}),
    );
    assert_eq!(bad["error"]["code"], -32602);
    assert_eq!(
        client.call(
            "session/set_mode",
            json!({"sessionId": "nope", "modeId": "plan"})
        )["error"]["code"],
        -32002
    );
    client.finish().unwrap();
}

#[test]
fn logged_out_until_authenticated() {
    let dir = Scratch::new("auth");
    let mut client = Client::start(&["--logged-out"]);
    let session = client.new_session(dir.path());
    let status = client
        .log
        .iter()
        .find(|m| m["method"] == "_auth/status_update")
        .unwrap();
    assert_eq!(status["params"]["authStatus"]["kind"], "none");
    let id = prompt(&mut client, &session, "hello");
    let failed = client.response(id);
    assert_eq!(failed["error"]["code"], -32000);
    assert_eq!(failed["error"]["message"], "Authentication required");
    client.finish().unwrap();

    let mut client = Client::start(&["--logged-out", "--agent-auth"]);
    let session = client.new_session(dir.path());
    let id = prompt(&mut client, &session, "hello");
    assert_eq!(client.response(id)["error"]["code"], -32000);
    assert_eq!(
        client.result("authenticate", json!({"methodId": "mock-login"})),
        json!({})
    );
    let id = prompt(&mut client, &session, "hello");
    assert_eq!(client.response(id)["result"]["stopReason"], "end_turn");
    client.finish().unwrap();
}

#[test]
fn stop_reasons_and_failures() {
    let dir = Scratch::new("stops");
    let mut client = Client::start(&[]);
    let session = client.new_session(dir.path());
    for (text, stop) in [
        ("refuse", "refusal"),
        ("long", "max_tokens"),
        ("fail", "end_turn"),
    ] {
        let id = prompt(&mut client, &session, text);
        assert_eq!(client.response(id)["result"]["stopReason"], stop, "{text}");
    }
    assert!(client
        .log
        .iter()
        .any(|m| m["params"]["update"]["status"] == "failed"));
    client.finish().unwrap();
}

#[test]
fn crash_cues() {
    let dir = Scratch::new("crash");
    let mut client = Client::start(&["--crash-on", "session/prompt"]);
    let session = client.new_session(dir.path());
    client.wait("commands", |m| {
        m["params"]["update"]["sessionUpdate"] == "available_commands_update"
    });
    let id = prompt(&mut client, &session, "hello");
    assert!(client.ended().is_err());
    assert!(!client.log.iter().any(|m| m["id"] == id));
    assert!(client
        .messages
        .recv_timeout(Duration::from_millis(200))
        .is_err());

    let mut client = Client::start(&["--crash-after", "3"]);
    let session = client.new_session(dir.path()); // available_commands_update is the first
    prompt(&mut client, &session, "hello");
    assert!(client.ended().is_err());
    while let Ok(message) = client.messages.recv_timeout(Duration::from_millis(200)) {
        client.log.push(message);
    }
    assert_eq!(
        client
            .log
            .iter()
            .filter(|m| m["method"] == "session/update")
            .count(),
        3
    );
}

#[test]
fn flags_parse() {
    let options = MockOptions::from_args(
        [
            "--profile",
            "claude",
            "--store",
            "/tmp/s",
            "--crash-after",
            "4",
            "--slow",
            "5",
            "--fs",
            "--terminal",
            "--bypass",
        ]
        .map(String::from),
    )
    .unwrap();
    assert_eq!(options.store.as_deref(), Some(Path::new("/tmp/s")));
    assert_eq!(
        (
            options.crash_after,
            options.slow_ms,
            options.fs,
            options.terminal,
            options.bypass
        ),
        (Some(4), 5, true, true, true)
    );
    assert!(MockOptions::from_args(["--profile", "codex"].map(String::from)).is_err());
    assert!(MockOptions::from_args(["--what"].map(String::from)).is_err());
    assert!(MockOptions::from_args(["--slow"].map(String::from)).is_err());
}
