//! `fenix --mcp`: Fenix as an MCP server, for the coding agents it starts
//! (and any other MCP client). It speaks MCP -- JSON-RPC, one message a
//! line, over stdio -- on one side, and Fenix's control channel on the
//! other: each tool is a control call to the Fenix that's running. So an
//! agent can ask the editor what only the editor knows -- the language
//! servers' problems and answers, the open files as they stand, what you
//! have selected, where the debugger is stopped -- show you a file, and,
//! as the user's rules (`agents.mcp.allow`) let it, edit through the
//! language servers, run tasks and drive the debugger. Extensions add
//! tools of their own (`contributes.agent_tools`).
//!
//! Fenix hands it to every agent session it opens
//! (`agents.mcp.fenix`); `--thread <id>` names the thread it speaks for,
//! so Fenix applies the rules for agents it started, asks on that
//! thread and records its edits there. Without one it's a client of
//! yours, with the stricter `client:` rules (`agent_tools::decide`).
//!
//! The bridge itself decides nothing: every tool, resource and prompt
//! is a `fenix/agent.*` call, and the editor checks the caller's rules
//! (`app/agent_calls.rs`).

use serde_json::{json, Value};
use std::io::{BufRead, Write};

/// What an agent Fenix starts is told before its first prompt
/// (`agents.mcp.fenix_brief`).
pub const BRIEFING: &str = "You are running inside Fenix, the user's editor, which gives you the `fenix` MCP tools. They know what your own tools can't, so prefer them:\n\
- Before searching text for a symbol, use `definition` and `references`, and `hover` for its type and docs: they come from the language servers and are exact. `document_symbols` outlines a file, and `workspace_symbols` finds a name across the project.\n\
- `open_files` says which files have unsaved edits. Read those with `read_buffer`, because the disk has an older version.\n\
- `diagnostics` shows problems without a build, including in unsaved text.\n\
- `selection` is what the user has selected, and `show_file` points them at a place.\n\
- More toolsets (git, refactor, tasks, debug, search, syntax, and the extensions') may be off: `fenix_toolsets` lists them and turns one on. Edits made through Fenix's tools go to the user's review like your own.\n\
Fall back to your own tools when these have no answer. Don't mention this note.";

/// The MCP protocol revisions this server speaks, newest first. A client
/// asking for one of them gets it; any other gets the newest.
pub const PROTOCOLS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

/// `--mcp` among the arguments.
pub fn requested(args: &[String]) -> bool {
    args.iter().any(|a| a == "--mcp")
}

/// The value after `flag` among the arguments.
fn flag_value(args: &[String], flag: &str) -> Option<String> {
    args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).filter(|v| !v.starts_with("--")).cloned()
}

/// The revision to answer `asked` with.
pub fn negotiate(asked: Option<&str>) -> &'static str {
    asked.and_then(|a| PROTOCOLS.iter().find(|p| **p == a)).copied().unwrap_or(PROTOCOLS[0])
}

/// What a session needs to remember between messages.
#[derive(Default)]
pub struct Session {
    /// The revision `initialize` settled on.
    pub protocol: Option<&'static str>,
}

/// A tool's answer, as MCP has it. `structured`: the revision has
/// `structuredContent` (2025-06-18 on).
fn tool_result(answer: Result<Value, String>, structured: bool) -> Value {
    match answer {
        Ok(answer) => {
            // An answer that's text already goes as it is.
            let text = answer.as_str().map(str::to_string).unwrap_or_else(|| serde_json::to_string_pretty(&answer).unwrap_or_default());
            let mut result = json!({ "content": [{ "type": "text", "text": text }] });
            if structured && answer.is_object() {
                result["structuredContent"] = answer;
            }
            result
        }
        Err(why) => json!({ "content": [{ "type": "text", "text": why }], "isError": true }),
    }
}

/// The core tools, for a `tools/list` with no Fenix to ask.
fn fallback_tools() -> Vec<Value> {
    crate::agent_tools::TOOLS.iter().filter(|t| t.toolset == "core").map(crate::agent_tools::listed).collect()
}

/// What one MCP message comes to: the reply (none for a notification),
/// and notifications to send after it.
#[derive(Debug, Default, PartialEq)]
pub struct Output {
    pub reply: Option<Value>,
    pub after: Vec<Value>,
}

fn notification(method: &str, params: Value) -> Value {
    json!({ "jsonrpc": "2.0", "method": method, "params": params })
}

/// The request's id as a call id Fenix can cancel by.
pub fn call_id(id: &Value) -> String {
    id.to_string()
}

/// One MCP message, answered: `call` makes a control request.
pub fn handle(session: &mut Session, message: &Value, call: &mut dyn FnMut(&str, Value) -> Result<Value, String>) -> Output {
    let method = message.get("method").and_then(Value::as_str).unwrap_or("");
    let params = message.get("params").cloned().unwrap_or(Value::Null);
    let Some(id) = message.get("id").cloned().filter(|i| !i.is_null()) else {
        // Notifications. A cancelled request is cancelled in Fenix too.
        if method == "notifications/cancelled" {
            if let Some(request) = params.get("requestId") {
                let _ = call(fenix_control::protocol::method::AGENT_CANCEL, json!({ "call": call_id(request) }));
            }
        }
        return Output::default();
    };
    let mut after = Vec::new();
    let structured = session.protocol.is_none_or(|p| p >= "2025-06-18");
    let forward = |call: &mut dyn FnMut(&str, Value) -> Result<Value, String>, method: &str, params: Value| call(method, params).map_err(|e| (-32603, e));
    let result: Result<Value, (i64, String)> = match method {
        "initialize" => {
            let protocol = negotiate(params.get("protocolVersion").and_then(Value::as_str));
            session.protocol = Some(protocol);
            Ok(json!({
                "protocolVersion": protocol,
                "capabilities": {
                    "tools": { "listChanged": true },
                    "resources": { "subscribe": true, "listChanged": true },
                    "prompts": { "listChanged": true },
                    "logging": {},
                },
                "serverInfo": { "name": "fenix", "title": "Fenix", "version": env!("CARGO_PKG_VERSION") },
                "instructions": "Fenix is the editor the user is working in. Prefer these tools to searching text: definition, references and hover come from its language servers and are exact. read_buffer has the unsaved edits the disk doesn't, diagnostics the problems without a build. fenix_toolsets lists the toolsets that are off (git, refactor, tasks, debug...) and turns one on.",
            }))
        }
        "ping" => Ok(json!({})),
        "logging/setLevel" => Ok(json!({})),
        "tools/list" => match call(fenix_control::protocol::method::AGENT_TOOLS, json!({})) {
            Ok(offered) => Ok(json!({ "tools": offered["tools"] })),
            // No Fenix to ask: the core tools, which will say so when called.
            Err(_) => Ok(json!({ "tools": fallback_tools() })),
        },
        "tools/call" => {
            let mut request = json!({ "call": call_id(&id), "tool": params.get("name").cloned().unwrap_or(Value::Null), "arguments": params.get("arguments").cloned().unwrap_or_else(|| json!({})) });
            if let Some(token) = params.get("_meta").and_then(|m| m.get("progressToken")) {
                request["progressToken"] = token.clone();
            }
            let answer = call(fenix_control::protocol::method::AGENT_CALL, request);
            // A toolset turned on or off: the list is different now.
            if answer.as_ref().is_ok_and(|a| a.get("toolsChanged").and_then(Value::as_bool) == Some(true)) {
                after.push(notification("notifications/tools/list_changed", json!({})));
            }
            Ok(tool_result(answer.map(|mut a| a.get_mut("answer").map(Value::take).unwrap_or(a)), structured))
        }
        "resources/list" => forward(call, fenix_control::protocol::method::AGENT_RESOURCES, json!({})),
        "resources/templates/list" => forward(call, fenix_control::protocol::method::AGENT_RESOURCES, json!({ "templates": true })),
        "resources/read" => forward(call, fenix_control::protocol::method::AGENT_READ, params),
        "resources/subscribe" => forward(call, fenix_control::protocol::method::AGENT_SUBSCRIBE, json!({ "uri": params.get("uri"), "on": true })),
        "resources/unsubscribe" => forward(call, fenix_control::protocol::method::AGENT_SUBSCRIBE, json!({ "uri": params.get("uri"), "on": false })),
        "prompts/list" => forward(call, fenix_control::protocol::method::AGENT_PROMPTS, json!({})),
        "prompts/get" => forward(call, fenix_control::protocol::method::AGENT_PROMPT, params),
        _ => Err((-32601, format!("no method {method}"))),
    };
    Output {
        reply: Some(match result {
            Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            Err((code, message)) => json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }),
        }),
        after,
    }
}

/// What Fenix's `fenix/agent.wait` events become for the MCP client.
pub fn notifications_for(events: &[Value]) -> Vec<Value> {
    let mut out = Vec::new();
    for event in events {
        match event.get("kind").and_then(Value::as_str) {
            Some("tools") => out.push(notification("notifications/tools/list_changed", json!({}))),
            Some("resources") => out.push(notification("notifications/resources/list_changed", json!({}))),
            Some("prompts") => out.push(notification("notifications/prompts/list_changed", json!({}))),
            Some("resource") => out.push(notification("notifications/resources/updated", json!({ "uri": event["uri"] }))),
            Some("progress") => {
                let mut params = json!({ "progressToken": event["token"], "progress": event.get("progress").cloned().unwrap_or(json!(0)) });
                for key in ["total", "message"] {
                    if let Some(v) = event.get(key).filter(|v| !v.is_null()) {
                        params[key] = v.clone();
                    }
                }
                out.push(notification("notifications/progress", params));
            }
            Some("log") => out.push(notification("notifications/message", json!({ "level": event.get("level").cloned().unwrap_or(json!("info")), "logger": "fenix", "data": event["data"] }))),
            _ => {}
        }
    }
    out
}

/// A connection to the running Fenix, introduced as the bridge for
/// `thread`.
type Control = fenix_control::Client<fenix_control::Stream, fenix_control::Stream>;

fn connect(thread: Option<&str>) -> Result<Control, String> {
    let mut c = fenix_control::Client::connect(&fenix_control::Endpoint::current()).map_err(|e| e.to_string())?.ok_or("Fenix isn't running")?;
    c.hello_for("fenix-mcp", env!("CARGO_PKG_VERSION"), thread).map_err(|e| e.to_string())?;
    Ok(c)
}

/// Connections kept for the next call, so each doesn't say hello again.
struct Pool {
    thread: Option<String>,
    idle: std::sync::Mutex<Vec<Control>>,
}

impl Pool {
    fn call(&self, method: &str, params: Value) -> Result<Value, String> {
        let taken = self.idle.lock().unwrap().pop();
        let mut client = match taken {
            Some(c) => c,
            None => connect(self.thread.as_deref())?,
        };
        let result = client.request::<Value>(method, params).map_err(|e| e.to_string());
        // A connection that failed is dropped, and tried afresh next time.
        if result.is_ok() {
            self.idle.lock().unwrap().push(client);
        }
        result
    }
}

/// Serves MCP on stdio until it closes, then exits. Each tool call runs
/// on a thread of its own, so a long one (a question to the user, a task
/// it waits for) doesn't hold up the rest, and a cancellation can reach
/// Fenix while it runs. A watcher waits on `fenix/agent.wait` and passes
/// Fenix's news on as notifications.
pub fn run(args: &[String]) -> ! {
    use std::sync::{Arc, Mutex};
    let pool = Arc::new(Pool { thread: flag_value(args, "--thread"), idle: Mutex::new(Vec::new()) });
    let out = Arc::new(Mutex::new(std::io::stdout()));
    let send = |out: &Mutex<std::io::Stdout>, messages: &[Value]| {
        let mut out = out.lock().unwrap();
        for m in messages {
            let _ = writeln!(out, "{}", serde_json::to_string(m).unwrap_or_default());
        }
        let _ = out.flush();
    };
    let session = Arc::new(Mutex::new(Session::default()));
    let mut watching = false;
    let stdin = std::io::stdin();
    let mut reader = stdin.lock();
    loop {
        let body = match fenix_rpc_line(&mut reader) {
            Some(body) => body,
            None => std::process::exit(0),
        };
        let Ok(message) = serde_json::from_slice::<Value>(&body) else { continue };
        let slow = message.get("method").and_then(Value::as_str) == Some("tools/call");
        if !watching && message.get("method").and_then(Value::as_str) == Some("notifications/initialized") {
            watching = true;
            let (pool, out) = (Arc::clone(&pool), Arc::clone(&out));
            std::thread::spawn(move || {
                let mut since = 0u64;
                loop {
                    match pool.call(fenix_control::protocol::method::AGENT_WAIT, json!({ "since": since })) {
                        Ok(news) => {
                            since = news.get("revision").and_then(Value::as_u64).unwrap_or(since);
                            let events = news.get("events").and_then(Value::as_array).cloned().unwrap_or_default();
                            send(&out, &notifications_for(&events));
                        }
                        Err(_) => std::thread::sleep(std::time::Duration::from_secs(2)),
                    }
                }
            });
        }
        let (pool, out, session) = (Arc::clone(&pool), Arc::clone(&out), Arc::clone(&session));
        let work = move || {
            let mut call = |method: &str, params: Value| pool.call(method, params);
            // The session is only written by `initialize`, which isn't slow.
            let output = if slow {
                let mut copy = Session { protocol: session.lock().unwrap().protocol };
                handle(&mut copy, &message, &mut call)
            } else {
                handle(&mut session.lock().unwrap(), &message, &mut call)
            };
            send(&out, &output.reply.into_iter().chain(output.after).collect::<Vec<_>>());
        };
        if slow {
            std::thread::spawn(work);
        } else {
            work();
        }
    }
}

/// One line of input, at most 4 MiB; `None` at the end.
fn fenix_rpc_line(reader: &mut impl BufRead) -> Option<Vec<u8>> {
    loop {
        let mut line = Vec::new();
        let n = std::io::Read::take(&mut *reader, 4 * 1024 * 1024).read_until(b'\n', &mut line).ok()?;
        if n == 0 {
            return None;
        }
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        return Some(line);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ask(message: Value, answers: &mut Vec<(String, Value)>) -> Output {
        let mut call = |method: &str, params: Value| {
            answers.push((method.to_string(), params.clone()));
            match params.get("tool").and_then(Value::as_str) {
                Some("run_task") => Err("refused: you said no".to_string()),
                Some("fenix_toolsets") => Ok(json!({ "answer": { "enabled": "git" }, "toolsChanged": true })),
                _ => Ok(json!({ "answer": { "ok": method } })),
            }
        };
        handle(&mut Session::default(), &message, &mut call)
    }

    #[test]
    fn it_negotiates_a_revision_it_knows() {
        assert_eq!(negotiate(Some("2025-03-26")), "2025-03-26");
        assert_eq!(negotiate(Some("2030-01-01")), PROTOCOLS[0]);
        assert_eq!(negotiate(None), PROTOCOLS[0]);
        let mut session = Session::default();
        let init = handle(&mut session, &json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { "protocolVersion": "2024-11-05" } }), &mut |_, _| unreachable!());
        let result = &init.reply.unwrap()["result"];
        assert_eq!(result["protocolVersion"], "2024-11-05");
        assert_eq!(result["capabilities"]["tools"]["listChanged"], true);
        assert_eq!(session.protocol, Some("2024-11-05"));
    }

    #[test]
    fn tools_come_from_fenix_or_the_core_without_it() {
        let mut call = |method: &str, _: Value| -> Result<Value, String> {
            assert_eq!(method, fenix_control::protocol::method::AGENT_TOOLS);
            Ok(json!({ "tools": [{ "name": "jira_issue" }] }))
        };
        let list = handle(&mut Session::default(), &json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }), &mut call);
        assert_eq!(list.reply.unwrap()["result"]["tools"][0]["name"], "jira_issue");
        let mut gone = |_: &str, _: Value| -> Result<Value, String> { Err("Fenix isn't running".into()) };
        let list = handle(&mut Session::default(), &json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }), &mut gone);
        let names: Vec<String> = list.reply.unwrap()["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap().to_string()).collect();
        assert!(names.contains(&"diagnostics".to_string()) && names.contains(&"fenix_toolsets".to_string()));
    }

    #[test]
    fn a_tool_is_an_agent_call_with_its_id_and_a_refusal_is_an_error_result() {
        let mut calls = Vec::new();
        let answer = ask(json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": { "name": "diagnostics", "arguments": { "path": "/p/a.rs" }, "_meta": { "progressToken": "p1" } } }), &mut calls);
        assert_eq!(calls[0], ("fenix/agent.call".to_string(), json!({ "call": "3", "tool": "diagnostics", "arguments": { "path": "/p/a.rs" }, "progressToken": "p1" })));
        assert_eq!(answer.reply.unwrap()["result"]["structuredContent"], json!({ "ok": "fenix/agent.call" }));
        let refused = ask(json!({ "jsonrpc": "2.0", "id": 5, "method": "tools/call", "params": { "name": "run_task", "arguments": { "name": "test" } } }), &mut calls);
        assert_eq!(refused.reply.unwrap()["result"]["isError"], json!(true));
    }

    #[test]
    fn turning_on_a_toolset_says_the_list_changed() {
        let mut calls = Vec::new();
        let out = ask(json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": "fenix_toolsets", "arguments": { "enable": "git" } } }), &mut calls);
        assert_eq!(out.after, vec![notification("notifications/tools/list_changed", json!({}))]);
        assert_eq!(out.reply.unwrap()["result"]["structuredContent"], json!({ "enabled": "git" }));
    }

    #[test]
    fn a_cancelled_request_is_cancelled_in_fenix() {
        let mut calls = Vec::new();
        let out = ask(json!({ "jsonrpc": "2.0", "method": "notifications/cancelled", "params": { "requestId": 9 } }), &mut calls);
        assert_eq!(out, Output::default());
        assert_eq!(calls, vec![("fenix/agent.cancel".to_string(), json!({ "call": "9" }))]);
        assert_eq!(ask(json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }), &mut calls), Output::default());
    }

    #[test]
    fn an_older_revision_gets_no_structured_content() {
        let mut session = Session { protocol: Some("2025-03-26") };
        let out = handle(&mut session, &json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": "x" } }), &mut |_, _| Ok(json!({ "answer": { "a": 1 } })));
        let result = &out.reply.unwrap()["result"];
        assert!(result.get("structuredContent").is_none());
        assert!(result["content"][0]["text"].as_str().unwrap().contains("\"a\": 1"));
    }

    #[test]
    fn fenixs_news_become_notifications() {
        let out = notifications_for(&[json!({ "kind": "tools" }), json!({ "kind": "resource", "uri": "fenix://diagnostics" }), json!({ "kind": "progress", "token": "p1", "progress": 2, "total": 5, "message": "querying" }), json!({ "kind": "unknown" })]);
        let methods: Vec<&str> = out.iter().map(|n| n["method"].as_str().unwrap()).collect();
        assert_eq!(methods, ["notifications/tools/list_changed", "notifications/resources/updated", "notifications/progress"]);
        assert_eq!(out[2]["params"], json!({ "progressToken": "p1", "progress": 2, "total": 5, "message": "querying" }));
    }

    #[test]
    fn resources_and_prompts_are_asked_of_fenix() {
        let mut calls = Vec::new();
        ask(json!({ "jsonrpc": "2.0", "id": 1, "method": "resources/subscribe", "params": { "uri": "fenix://selection" } }), &mut calls);
        ask(json!({ "jsonrpc": "2.0", "id": 2, "method": "prompts/get", "params": { "name": "explain", "arguments": {} } }), &mut calls);
        assert_eq!(calls[0], ("fenix/agent.subscribe".to_string(), json!({ "uri": "fenix://selection", "on": true })));
        assert_eq!(calls[1].0, "fenix/agent.prompt");
    }

    #[test]
    fn the_thread_comes_from_the_arguments() {
        let args: Vec<String> = ["fenix", "--mcp", "--thread", "t-1"].iter().map(|s| s.to_string()).collect();
        assert_eq!(flag_value(&args, "--thread").as_deref(), Some("t-1"));
        assert_eq!(flag_value(&args[..2], "--thread"), None);
    }
}
