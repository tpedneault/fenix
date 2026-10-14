//! A tiny language server for tests: Fenix's own tests start it to have
//! real processes speaking LSP on stdio, several at once on one document.
//! Not part of a release (those build `fenix-gui` alone).
//!
//! `fake-language-server NAME [--crash-on METHOD] [--triggers CHARS]`:
//! - hover answers `NAME hover`;
//! - completion answers one item, `NAME-item`, in an incomplete list;
//! - definition answers line 0 of the document asked about;
//! - every `didOpen`/`didChange` publishes one diagnostic, `NAME saw it`;
//! - a request for METHOD makes it exit at once, as a crash would.

use std::io::{BufReader, Write};

use serde_json::{json, Value};

fn main() {
    let mut args = std::env::args().skip(1);
    let name = args.next().unwrap_or_else(|| "fake".to_string());
    let mut crash_on = None;
    let mut triggers: Vec<String> = Vec::new();
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--crash-on" => crash_on = args.next(),
            "--triggers" => {
                triggers = args
                    .next()
                    .unwrap_or_default()
                    .chars()
                    .map(String::from)
                    .collect()
            }
            _ => {}
        }
    }

    let mut input = BufReader::new(std::io::stdin().lock());
    let mut output = std::io::stdout().lock();
    while let Ok(Some(body)) = fenix_rpc::read_message(&mut input) {
        let Ok(message) = serde_json::from_slice::<Value>(&body) else {
            return;
        };
        let method = message["method"].as_str().unwrap_or_default().to_string();
        let id = message.get("id").cloned();
        if crash_on.as_deref() == Some(method.as_str()) {
            std::process::exit(3);
        }
        let params = &message["params"];
        let uri = params["textDocument"]["uri"].clone();
        let result = match method.as_str() {
            "initialize" => json!({"capabilities": {
                "textDocumentSync": 1,
                "hoverProvider": true,
                "definitionProvider": true,
                "completionProvider": {"triggerCharacters": triggers},
            }, "serverInfo": {"name": name}}),
            "textDocument/hover" => {
                json!({"contents": {"kind": "plaintext", "value": format!("{name} hover")}})
            }
            "textDocument/completion" => {
                json!({"isIncomplete": true, "items": [{"label": format!("{name}-item")}]})
            }
            "textDocument/definition" => {
                json!([{"uri": uri, "range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}}}])
            }
            "textDocument/didOpen" | "textDocument/didChange" => {
                let diagnostic = json!({"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}}, "message": format!("{name} saw it")});
                send(
                    &mut output,
                    json!({"jsonrpc": "2.0", "method": "textDocument/publishDiagnostics", "params": {"uri": uri, "diagnostics": [diagnostic]}}),
                );
                continue;
            }
            "exit" => return,
            _ => Value::Null,
        };
        if let Some(id) = id {
            send(
                &mut output,
                json!({"jsonrpc": "2.0", "id": id, "result": result}),
            );
        }
    }
}

fn send(output: &mut impl Write, message: Value) {
    let body = serde_json::to_vec(&message).expect("a JSON value serializes");
    let _ = fenix_rpc::write_message(output, &body);
    let _ = output.flush();
}
