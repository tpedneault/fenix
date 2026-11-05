//! `fenix --mcp`: Fenix as an MCP server, for the coding agents it starts
//! (and any other MCP client). It speaks MCP -- JSON-RPC, one message a
//! line, over stdio -- on one side, and Fenix's control channel on the
//! other: each tool is a control call to the Fenix that's running. So an
//! agent can ask the editor what only the editor knows -- the language
//! servers' problems and answers, the open files as they stand, what you
//! have selected, where the debugger is stopped -- show you a file, and,
//! when `agents.mcp.fenix_write` lets it, run the project's tasks and
//! Fenix's commands and evaluate in the debugger. Extensions add tools of
//! their own (`contributes.agent_tools`).
//!
//! Fenix hands it to every agent session it opens
//! (`agents.mcp.fenix`); `--thread <id>` names the thread, for the log.

use serde_json::{json, Value};
use std::io::{BufRead, Write};

/// What an agent Fenix starts is told before its first prompt
/// (`agents.mcp.fenix_brief`).
pub const BRIEFING: &str = "You are running inside Fenix, the user's editor, which gives you the `fenix` MCP tools. They know what your own tools can't, so prefer them:\n\
- Before searching text for a symbol, use `definition` and `references`, and `hover` for its type and docs: they come from the language servers and are exact. `document_symbols` outlines a file, and `workspace_symbols` finds a name across the project.\n\
- `open_files` says which files have unsaved edits. Read those with `read_buffer`, because the disk has an older version.\n\
- `diagnostics` shows problems without a build, including in unsaved text.\n\
- When debugging, `debug_state` shows where the program is stopped, with its stack and variables.\n\
- `selection` is what the user has selected, and `show_file` points them at a place.\n\
Fall back to your own tools when these have no answer. Don't mention this note.";

/// The tools that only read, which agents needn't ask to use
/// (`agents.mcp.fenix_auto_read`). `show_file` moves what's on your
/// screen, and the rest change things.
pub const READ_ONLY_TOOLS: &[&str] = &["diagnostics", "open_files", "selection", "tasks", "read_buffer", "definition", "references", "hover", "document_symbols", "workspace_symbols", "debug_state"];

/// The MCP protocol version answered when the client asks for one this
/// doesn't know.
const PROTOCOL: &str = "2025-06-18";

/// `--mcp` among the arguments.
pub fn requested(args: &[String]) -> bool {
    args.iter().any(|a| a == "--mcp")
}

struct Tool {
    name: &'static str,
    description: &'static str,
    method: &'static str,
    schema: fn() -> Value,
}

fn no_arguments() -> Value {
    json!({ "type": "object", "properties": {} })
}

const TOOLS: &[Tool] = &[
    Tool {
        name: "diagnostics",
        description: "The problems Fenix's language servers found: in one file (path), or in every file open in Fenix. Fresher than a build: they follow unsaved edits too.",
        method: fenix_control::protocol::method::DIAGNOSTICS,
        schema: || json!({ "type": "object", "properties": { "path": { "type": "string", "description": "An absolute path; leave out for every open file" } } }),
    },
    Tool {
        name: "open_files",
        description: "The files open in Fenix: which has the focus, and which have unsaved edits the user hasn't saved yet (the disk doesn't have them).",
        method: fenix_control::protocol::method::BUFFERS,
        schema: no_arguments,
    },
    Tool {
        name: "selection",
        description: "What the user has selected in Fenix (or the line the cursor is on): its file, line and text.",
        method: fenix_control::protocol::method::SELECTION,
        schema: no_arguments,
    },
    Tool {
        name: "show_file",
        description: "Shows the user a file in Fenix, at a line: to point at what you mean.",
        method: fenix_control::protocol::method::OPEN,
        schema: || json!({ "type": "object", "properties": { "path": { "type": "string" }, "line": { "type": "integer", "minimum": 1 } }, "required": ["path"] }),
    },
    Tool {
        name: "tasks",
        description: "The project's tasks as Fenix knows them (build, test, lint...), by name.",
        method: fenix_control::protocol::method::TASKS,
        schema: no_arguments,
    },
    Tool {
        name: "run_task",
        description: "Runs one of the project's tasks in Fenix's task pane, where the user sees it. Only when the user allows it (agents.mcp.fenix_write).",
        method: fenix_control::protocol::method::TASKS_RUN,
        schema: || json!({ "type": "object", "properties": { "name": { "type": "string" } }, "required": ["name"] }),
    },
    Tool {
        name: "run_command",
        description: "Runs a Fenix command by its id (fenix/commands lists them). Only when the user allows it (agents.mcp.fenix_write).",
        method: fenix_control::protocol::method::AGENT_COMMAND,
        schema: || json!({ "type": "object", "properties": { "id": { "type": "string" } }, "required": ["id"] }),
    },
    Tool {
        name: "read_buffer",
        description: "A file open in Fenix as it stands now, with the user's unsaved edits, which the disk doesn't have. Whole, or startLine to endLine (from 1). open_files says which are open and unsaved.",
        method: fenix_control::protocol::method::BUFFER_READ,
        schema: || json!({ "type": "object", "properties": { "path": { "type": "string" }, "startLine": { "type": "integer" }, "endLine": { "type": "integer" } }, "required": ["path"] }),
    },
    Tool {
        name: "definition",
        description: "Where the symbol at a place in a file is defined, from the language servers: more exact than searching. Give path, line (from 1), and symbol, the name on that line, or column (from 1).",
        method: fenix_control::protocol::method::LSP,
        schema: position_schema,
    },
    Tool {
        name: "references",
        description: "Every use of the symbol at a place in a file, across the project, from the language servers, each with its line: what a change to it would touch. Give path, line (from 1), and symbol or column.",
        method: fenix_control::protocol::method::LSP,
        schema: position_schema,
    },
    Tool {
        name: "hover",
        description: "The type and documentation of the symbol at a place in a file, from the language servers. Give path, line (from 1), and symbol or column.",
        method: fenix_control::protocol::method::LSP,
        schema: position_schema,
    },
    Tool {
        name: "document_symbols",
        description: "A file's outline from the language servers: its types, functions and so on, with their lines.",
        method: fenix_control::protocol::method::LSP,
        schema: || json!({ "type": "object", "properties": { "path": { "type": "string" } }, "required": ["path"] }),
    },
    Tool {
        name: "workspace_symbols",
        description: "Finds types, functions and so on by name across the project, from the language servers already running there.",
        method: fenix_control::protocol::method::LSP,
        schema: || json!({ "type": "object", "properties": { "query": { "type": "string" } }, "required": ["query"] }),
    },
    Tool {
        name: "debug_state",
        description: "Where Fenix's debugger is stopped: the call stack, the variables in the selected frame, watches, and the program's latest output.",
        method: fenix_control::protocol::method::DEBUG_STATE,
        schema: || json!({ "type": "object", "properties": {} }),
    },
    Tool {
        name: "debug_evaluate",
        description: "Evaluates an expression where the debugger is stopped. It may run code in the program. Only when the user allows it (agents.mcp.fenix_write).",
        method: fenix_control::protocol::method::DEBUG_EVALUATE,
        schema: || json!({ "type": "object", "properties": { "expression": { "type": "string" } }, "required": ["expression"] }),
    },
];

/// A place in a file: what the language tools take.
fn position_schema() -> Value {
    json!({ "type": "object", "properties": { "path": { "type": "string" }, "line": { "type": "integer" }, "symbol": { "type": "string", "description": "The name on that line" }, "column": { "type": "integer" } }, "required": ["path", "line"] })
}

/// A tool's answer, as MCP has it.
fn tool_result(answer: Result<Value, String>) -> Value {
    match answer {
        Ok(answer) => json!({ "content": [{ "type": "text", "text": serde_json::to_string_pretty(&answer).unwrap_or_default() }], "structuredContent": answer }),
        Err(why) => json!({ "content": [{ "type": "text", "text": why }], "isError": true }),
    }
}

/// One MCP message, answered: `call` makes a control request. `None`
/// for a notification.
pub fn handle(message: &Value, call: &mut dyn FnMut(&str, Value) -> Result<Value, String>) -> Option<Value> {
    let id = message.get("id").cloned().filter(|i| !i.is_null())?;
    let method = message.get("method").and_then(Value::as_str).unwrap_or("");
    let params = message.get("params").cloned().unwrap_or(Value::Null);
    let result = match method {
        "initialize" => Ok(json!({
            "protocolVersion": params.get("protocolVersion").and_then(Value::as_str).unwrap_or(PROTOCOL),
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "fenix", "title": "Fenix", "version": env!("CARGO_PKG_VERSION") },
            "instructions": "Fenix is the editor the user is working in. Prefer these tools to searching text: definition, references and hover come from its language servers and are exact. read_buffer has the unsaved edits the disk doesn't, diagnostics the problems without a build, and debug_state where the debugger is stopped.",
        })),
        "ping" => Ok(json!({})),
        "tools/list" => {
            let mut tools: Vec<Value> = TOOLS.iter().map(|t| json!({ "name": t.name, "description": t.description, "inputSchema": (t.schema)() })).collect();
            // The extensions', when there's a Fenix to ask.
            if let Ok(offered) = call(fenix_control::protocol::method::TOOLS, json!({})) {
                tools.extend(offered["tools"].as_array().into_iter().flatten().map(|t| json!({ "name": t["name"], "description": t["description"], "inputSchema": t["inputSchema"] })));
            }
            Ok(json!({ "tools": tools }))
        }
        "tools/call" => {
            let name = params.get("name").and_then(Value::as_str).unwrap_or("");
            let arguments = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
            match TOOLS.iter().find(|t| t.name == name) {
                // An extension's, which Fenix knows.
                None => Ok(tool_result(call(fenix_control::protocol::method::TOOLS_CALL, json!({ "name": name, "arguments": arguments })))),
                Some(tool) => {
                    let request = match tool.name {
                        // `fenix/open` takes a list of positions.
                        "show_file" => json!({ "paths": [{ "path": arguments.get("path"), "line": arguments.get("line") }], "focus": false }),
                        // `fenix/lsp` is every question, by kind.
                        "definition" | "references" | "hover" | "document_symbols" | "workspace_symbols" => {
                            let mut request = arguments;
                            request["kind"] = json!(tool.name);
                            request
                        }
                        _ => arguments,
                    };
                    Ok(tool_result(call(tool.method, request)))
                }
            }
        }
        _ => Err((-32601, format!("no method {method}"))),
    };
    Some(match result {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err((code, message)) => json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }),
    })
}

/// Serves MCP on stdio until it closes, then exits.
pub fn run() -> ! {
    let mut client: Option<fenix_control::Client<fenix_control::Stream, fenix_control::Stream>> = None;
    let mut call = |method: &str, params: Value| -> Result<Value, String> {
        if client.is_none() {
            let mut c = fenix_control::Client::connect(&fenix_control::Endpoint::current()).map_err(|e| e.to_string())?.ok_or("Fenix isn't running")?;
            c.hello("fenix-mcp", env!("CARGO_PKG_VERSION")).map_err(|e| e.to_string())?;
            client = Some(c);
        }
        let result = client.as_mut().unwrap().request::<Value>(method, params);
        if result.is_err() {
            // A broken connection is tried afresh next time.
            client = None;
        }
        result.map_err(|e| e.to_string())
    };
    let stdin = std::io::stdin();
    let mut reader = stdin.lock();
    let mut out = std::io::stdout();
    loop {
        let body = match fenix_rpc_line(&mut reader) {
            Some(body) => body,
            None => std::process::exit(0),
        };
        let Ok(message) = serde_json::from_slice::<Value>(&body) else { continue };
        if let Some(reply) = handle(&message, &mut call) {
            let _ = writeln!(out, "{}", serde_json::to_string(&reply).unwrap_or_default());
            let _ = out.flush();
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

    fn ask(message: Value, answers: &mut Vec<(String, Value)>) -> Value {
        let mut call = |method: &str, params: Value| {
            answers.push((method.to_string(), params.clone()));
            if method == fenix_control::protocol::method::TASKS_RUN {
                Err("running things through Fenix is off: agents.mcp.fenix_write".to_string())
            } else {
                Ok(json!({ "ok": method }))
            }
        };
        handle(&message, &mut call).unwrap()
    }

    #[test]
    fn it_initializes_and_lists_its_tools() {
        let mut calls = Vec::new();
        let init = ask(json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { "protocolVersion": "2025-03-26" } }), &mut calls);
        assert_eq!(init["result"]["protocolVersion"], json!("2025-03-26"));
        assert_eq!(init["result"]["serverInfo"]["name"], json!("fenix"));
        let tools = ask(json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }), &mut calls);
        let names: Vec<&str> = tools["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(names, TOOLS.iter().map(|t| t.name).collect::<Vec<_>>());
        for tool in ["diagnostics", "read_buffer", "definition", "references", "hover", "document_symbols", "workspace_symbols", "debug_state", "debug_evaluate", "run_task"] {
            assert!(names.contains(&tool), "{tool}");
        }
        assert_eq!(calls.len(), 1, "Fenix is asked only for the extensions' tools");
        assert!(handle(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }), &mut |_, _| unreachable!()).is_none());
    }

    #[test]
    fn a_tool_is_a_control_call_and_a_refusal_is_an_error_result() {
        let mut calls = Vec::new();
        let answer = ask(json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": { "name": "diagnostics", "arguments": { "path": "/p/a.rs" } } }), &mut calls);
        assert_eq!(calls[0], ("fenix/diagnostics".to_string(), json!({ "path": "/p/a.rs" })));
        assert_eq!(answer["result"]["structuredContent"], json!({ "ok": "fenix/diagnostics" }));
        let shown = ask(json!({ "jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": { "name": "show_file", "arguments": { "path": "/p/a.rs", "line": 3 } } }), &mut calls);
        assert!(shown.get("result").is_some());
        assert_eq!(calls[1].1, json!({ "paths": [{ "path": "/p/a.rs", "line": 3 }], "focus": false }));
        let refused = ask(json!({ "jsonrpc": "2.0", "id": 5, "method": "tools/call", "params": { "name": "run_task", "arguments": { "name": "test" } } }), &mut calls);
        assert_eq!(refused["result"]["isError"], json!(true));
    }

    #[test]
    fn every_read_only_tool_exists_and_none_of_them_changes_anything() {
        for tool in READ_ONLY_TOOLS {
            assert!(TOOLS.iter().any(|t| t.name == *tool), "{tool}");
        }
        for changes in ["run_task", "run_command", "debug_evaluate", "show_file"] {
            assert!(!READ_ONLY_TOOLS.contains(&changes), "{changes}");
        }
    }

    #[test]
    fn the_language_tools_ask_by_kind() {
        let mut calls = Vec::new();
        ask(json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": "references", "arguments": { "path": "a.rs", "line": 3, "symbol": "total" } } }), &mut calls);
        assert_eq!(calls[0], (fenix_control::protocol::method::LSP.to_string(), json!({ "kind": "references", "path": "a.rs", "line": 3, "symbol": "total" })));
    }

    #[test]
    fn a_name_fenix_doesnt_have_goes_to_the_extensions() {
        let mut calls = Vec::new();
        ask(json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": "jira_issue", "arguments": { "key": "A-1" } } }), &mut calls);
        assert_eq!(calls[0], (fenix_control::protocol::method::TOOLS_CALL.to_string(), json!({ "name": "jira_issue", "arguments": { "key": "A-1" } })));
    }

    #[test]
    fn the_extensions_tools_are_listed_after_fenixs() {
        let mut call = |method: &str, _: Value| -> Result<Value, String> {
            assert_eq!(method, fenix_control::protocol::method::TOOLS);
            Ok(json!({ "tools": [{ "name": "jira_issue", "description": "An issue", "inputSchema": { "type": "object" }, "writes": false }] }))
        };
        let list = handle(&json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }), &mut call).unwrap();
        assert_eq!(list["result"]["tools"].as_array().unwrap().last().unwrap()["name"], "jira_issue");
        let mut gone = |_: &str, _: Value| -> Result<Value, String> { Err("Fenix isn't running".into()) };
        let list = handle(&json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }), &mut gone).unwrap();
        assert_eq!(list["result"]["tools"].as_array().unwrap().len(), TOOLS.len(), "Fenix's own without one");
    }
}
