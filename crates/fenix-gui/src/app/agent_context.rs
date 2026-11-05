//! MCP resources and prompts, for the bridges (`fenix/agent.resources`,
//! `read`, `subscribe`, `prompts`, `prompt`).
//!
//! - **Resources** are what Fenix's read tools give, by URI:
//!   `fenix://buffer/{path}`, `fenix://diagnostics[/{path}]`,
//!   `fenix://selection`, `fenix://debug/state`, `fenix://git/status`,
//!   `fenix://task/output`, and the extensions' (`agent_resources`). A
//!   read is the tool's call, under the same rules.
//! - **Subscriptions** are looked at once a second; a resource whose
//!   answer changed is announced (`notifications/resources/updated`).
//! - **Prompts** are workflows the agent's client offers as commands:
//!   Fenix's (`fix_diagnostic`, `explain`, `review_changes`,
//!   `write_tests`) and the extensions' (`agent_prompts`).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::mpsc::Sender;

use fenix_control::{AgentCaller, RpcError};
use serde_json::{json, Value};

use super::*;
use crate::ipc::Reply;

/// How often subscribed resources are looked at.
const LOOK_EVERY: Duration = Duration::from_secs(1);

/// The resources whose answers a caller asked to hear about, and what
/// each answered last.
#[derive(Default)]
pub(crate) struct Subscriptions {
    by_caller: BTreeMap<String, BTreeMap<String, u64>>,
    next_look: Option<Instant>,
}

fn invalid(message: impl Into<String>) -> RpcError {
    RpcError::invalid_params(message)
}

/// The tool a Fenix resource is read by, and its arguments.
fn resource_tool(uri: &str) -> Option<(&'static str, Value)> {
    let rest = uri.strip_prefix("fenix://")?;
    Some(match rest {
        "diagnostics" => ("diagnostics", json!({})),
        "selection" => ("selection", json!({})),
        "debug/state" => ("debug_state", json!({})),
        "git/status" => ("git_status", json!({})),
        "task/output" => ("task_output", json!({})),
        "open_files" => ("open_files", json!({})),
        _ => {
            if let Some(path) = rest.strip_prefix("buffer/") {
                ("read_buffer", json!({ "path": decode(path) }))
            } else {
                ("diagnostics", json!({ "path": decode(rest.strip_prefix("diagnostics/")?) }))
            }
        }
    })
}

/// `%xx` escapes undone: a path in a URI.
fn decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(b) = u8::from_str_radix(&text[i + 1..i + 3], 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    let path = String::from_utf8_lossy(&out).into_owned();
    // `fenix://buffer//home/me/a.rs` and `fenix://buffer/C:/x.rs` alike.
    if path.starts_with('/') || path.chars().nth(1) == Some(':') { path } else { format!("/{path}") }
}

/// A path as a resource URI's tail.
fn encode(path: &str) -> String {
    path.chars().map(|c| match c {
        ' ' => "%20".to_string(),
        '%' => "%25".to_string(),
        '#' => "%23".to_string(),
        '?' => "%3F".to_string(),
        '\\' => "/".to_string(),
        c => c.to_string(),
    }).collect::<String>().trim_start_matches('/').to_string()
}

/// `template`'s parameters as `uri` gives them, when it matches.
pub(super) fn match_template(template: &str, uri: &str) -> Option<BTreeMap<String, String>> {
    let mut params = BTreeMap::new();
    let (mut t, mut u) = (template, uri);
    loop {
        match t.find('{') {
            None => return (t == u).then_some(params),
            Some(open) => {
                let (literal, after) = t.split_at(open);
                u = u.strip_prefix(literal)?;
                let close = after.find('}')?;
                let name = &after[1..close];
                t = &after[close + 1..];
                // A parameter runs to the next literal, or to the end.
                let next = t.find('{').map(|i| &t[..i]).unwrap_or(t);
                let end = if next.is_empty() { u.len() } else { u.find(next)? };
                params.insert(name.to_string(), u[..end].to_string());
                u = &u[end..];
            }
        }
    }
}

/// A fingerprint of an answer, to tell when it changed.
fn fingerprint(value: &Value) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    value.to_string().hash(&mut hash);
    hash.finish()
}

/// Fenix's prompts: name, what it's for, its arguments (name, required).
const PROMPTS: &[(&str, &str, &[(&str, bool)])] = &[
    ("fix_diagnostic", "Fix the problem the language server reports on a line, as small a change as will do", &[("path", true), ("line", true)]),
    ("explain", "Explain what some lines of code do, and why", &[("path", true), ("startLine", true), ("endLine", false)]),
    ("review_changes", "Review the working tree's changes, or a branch's against base, with findings by file and line", &[("base", false)]),
    ("write_tests", "Write tests for the function at a line, in the project's own style", &[("path", true), ("line", true)]),
];

impl App {
    fn agent_extension_resources(&self) -> Vec<(String, fenix_extensions::manifest::AgentResourceContribution)> {
        self.wasm.installed.iter().flat_map(|e| e.manifest.contributes.agent_resources.iter().map(move |r| (e.id.clone(), r.clone()))).collect()
    }

    fn agent_extension_prompts(&self) -> Vec<(String, fenix_extensions::manifest::AgentPromptContribution)> {
        self.wasm.installed.iter().flat_map(|e| e.manifest.contributes.agent_prompts.iter().map(move |p| (e.id.clone(), p.clone()))).collect()
    }

    /// `fenix/agent.resources`, `read`, `subscribe`, `prompts`, `prompt`.
    pub(super) fn agent_context(&mut self, method: &str, caller: AgentCaller, params: Value, reply: Sender<Reply>) {
        match method {
            "resources" if params.get("templates").and_then(Value::as_bool) == Some(true) => {
                let mut templates = vec![
                    json!({ "uriTemplate": "fenix://buffer/{path}", "name": "An open file as it stands", "mimeType": "text/plain" }),
                    json!({ "uriTemplate": "fenix://diagnostics/{path}", "name": "A file's problems", "mimeType": "application/json" }),
                ];
                templates.extend(self.agent_extension_resources().into_iter().map(|(_, r)| json!({ "uriTemplate": r.uri_template, "name": r.name, "description": r.description, "mimeType": r.mime_type })));
                let _ = reply.send(Ok(json!({ "resourceTemplates": templates })));
            }
            "resources" => {
                let mut resources = vec![
                    json!({ "uri": "fenix://diagnostics", "name": "Problems in the open files", "mimeType": "application/json" }),
                    json!({ "uri": "fenix://selection", "name": "The user's selection", "mimeType": "application/json" }),
                    json!({ "uri": "fenix://open_files", "name": "The open files, and which are unsaved", "mimeType": "application/json" }),
                    json!({ "uri": "fenix://debug/state", "name": "Where the debugger is stopped", "mimeType": "application/json" }),
                    json!({ "uri": "fenix://git/status", "name": "The repository's state", "mimeType": "application/json" }),
                    json!({ "uri": "fenix://task/output", "name": "The last task's output", "mimeType": "application/json" }),
                ];
                for id in self.buffers.ids_sorted_by_path() {
                    if let Some(path) = self.buffers.get(id).filter(|ob| ob.kind == BufferKind::Text).and_then(|ob| ob.buffer.path()) {
                        let path = path.to_string_lossy();
                        resources.push(json!({ "uri": format!("fenix://buffer/{}", encode(&path)), "name": path, "mimeType": "text/plain" }));
                    }
                }
                let _ = reply.send(Ok(json!({ "resources": resources })));
            }
            "read" => {
                let uri = params.get("uri").and_then(Value::as_str).unwrap_or_default().to_string();
                self.agent_read_resource(caller, uri, reply);
            }
            "subscribe" => {
                let uri = params.get("uri").and_then(Value::as_str).unwrap_or_default().to_string();
                let on = params.get("on").and_then(Value::as_bool).unwrap_or(true);
                let key = super::agent_calls::caller_key(&caller);
                let subs = self.agent_subscriptions.by_caller.entry(key.clone()).or_default();
                if on {
                    subs.insert(uri, 0);
                } else {
                    subs.remove(&uri);
                }
                if subs.is_empty() {
                    self.agent_subscriptions.by_caller.remove(&key);
                }
                let _ = reply.send(Ok(json!({})));
            }
            "prompts" => {
                let mut prompts: Vec<Value> = PROMPTS.iter().map(|(name, description, args)| json!({ "name": name, "description": description, "arguments": args.iter().map(|(n, r)| json!({ "name": n, "required": r })).collect::<Vec<_>>() })).collect();
                prompts.extend(self.agent_extension_prompts().into_iter().map(|(_, p)| json!({ "name": p.name, "description": p.description, "arguments": p.arguments.iter().map(|a| json!({ "name": a.name, "description": a.description, "required": a.required })).collect::<Vec<_>>() })));
                let _ = reply.send(Ok(json!({ "prompts": prompts })));
            }
            "prompt" => {
                let name = params.get("name").and_then(Value::as_str).unwrap_or_default().to_string();
                let arguments = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
                self.agent_prompt(&name, &arguments, reply);
            }
            _ => {
                let _ = reply.send(Err(invalid(format!("no method fenix/agent.{method}"))));
            }
        }
    }

    /// A resource read, as MCP has it: `{ contents: [{ uri, mimeType, text }] }`.
    fn agent_read_resource(&mut self, caller: AgentCaller, uri: String, reply: Sender<Reply>) {
        let (inner, answered) = std::sync::mpsc::channel::<Reply>();
        let text_uri = uri.clone();
        std::thread::spawn(move || {
            if let Ok(answer) = answered.recv() {
                let _ = reply.send(answer.map(|a| {
                    let body = a.get("answer").cloned().unwrap_or(a);
                    let (mime, text) = match body.get("text").and_then(Value::as_str) {
                        Some(text) if text_uri.starts_with("fenix://buffer/") => ("text/plain", text.to_string()),
                        _ => match &body {
                            Value::String(s) => ("text/plain", s.clone()),
                            other => ("application/json", serde_json::to_string_pretty(other).unwrap_or_default()),
                        },
                    };
                    json!({ "contents": [{ "uri": text_uri, "mimeType": mime, "text": text }] })
                }));
            }
        });
        if let Some((tool, arguments)) = resource_tool(&uri) {
            self.control_agent("call", caller, json!({ "call": "", "tool": tool, "arguments": arguments }), inner);
            return;
        }
        // An extension's, by its template.
        for (extension, resource) in self.agent_extension_resources() {
            if let Some(params) = match_template(&resource.uri_template, &uri) {
                let then = move |_: &mut App, said: Result<String, String>| {
                    let _ = inner.send(said.map_err(invalid).map(|text| serde_json::from_str::<Value>(&text).unwrap_or(Value::String(text))));
                };
                self.code_serve(&extension, &resource.service, json!(params).to_string(), super::wasm_host::Answer::Fenix(Box::new(then)));
                return;
            }
        }
        let _ = inner.send(Err(invalid(format!("Fenix has no resource {uri}"))));
    }

    /// Subscribed resources looked at again; those that changed announced.
    pub(super) fn agent_subscriptions_tick(&mut self, now: Instant) -> Option<Instant> {
        if self.agent_subscriptions.by_caller.is_empty() {
            return None;
        }
        if self.agent_subscriptions.next_look.is_some_and(|t| now < t) {
            return self.agent_subscriptions.next_look;
        }
        self.agent_subscriptions.next_look = Some(now + LOOK_EVERY);
        let wanted: BTreeSet<String> = self.agent_subscriptions.by_caller.values().flat_map(|m| m.keys().cloned()).collect();
        let mut now_print: BTreeMap<String, u64> = BTreeMap::new();
        for uri in wanted {
            // What Fenix knows at once; Git and extensions are left out of
            // the look, being slow: a client reads them again when it wants.
            let print = match resource_tool(&uri) {
                Some(("diagnostics", args)) => fingerprint(&self.apply_control(fenix_control::Call::Diagnostics(args)).unwrap_or_default()),
                Some(("selection", _)) => fingerprint(&self.apply_control(fenix_control::Call::Selection).unwrap_or_default()),
                Some(("open_files", _)) => fingerprint(&self.apply_control(fenix_control::Call::Buffers).unwrap_or_default()),
                Some(("debug_state", _)) => fingerprint(&self.control_debug_state()),
                Some(("read_buffer", args)) => fingerprint(&self.control_buffer_read(&args).unwrap_or_default()),
                Some(("task_output", _)) => self.task_session.as_ref().and_then(|s| self.buffers.get(s.buffer)).map(|ob| ob.buffer.edit_count()).unwrap_or(0),
                _ => continue,
            };
            now_print.insert(uri, print);
        }
        let mut news = Vec::new();
        for (caller, subs) in self.agent_subscriptions.by_caller.iter_mut() {
            for (uri, last) in subs.iter_mut() {
                let Some(print) = now_print.get(uri) else { continue };
                if *last != 0 && *last != *print {
                    news.push((caller.clone(), uri.clone()));
                }
                *last = *print;
            }
        }
        for (caller, uri) in news {
            self.agent_news(Some(caller), json!({ "kind": "resource", "uri": uri }));
        }
        self.agent_subscriptions.next_look
    }

    /// `prompts/get`: the messages a prompt is.
    fn agent_prompt(&mut self, name: &str, arguments: &Value, reply: Sender<Reply>) {
        if let Some((extension, prompt)) = self.agent_extension_prompts().into_iter().find(|(_, p)| p.name == name) {
            let then = move |_: &mut App, said: Result<String, String>| {
                let answer = said.map_err(invalid).map(|text| match serde_json::from_str::<Value>(&text) {
                    // An answer of messages, or of text that is the prompt.
                    Ok(v) if v.get("messages").is_some() => v,
                    _ => json!({ "messages": [{ "role": "user", "content": { "type": "text", "text": text } }] }),
                });
                let _ = reply.send(answer);
            };
            self.code_serve(&extension, &prompt.service, arguments.to_string(), super::wasm_host::Answer::Fenix(Box::new(then)));
            return;
        }
        let _ = reply.send(self.agent_fenix_prompt(name, arguments).map(|text| json!({ "description": PROMPTS.iter().find(|p| p.0 == name).map(|p| p.1), "messages": [{ "role": "user", "content": { "type": "text", "text": text } }] })));
    }

    fn agent_lines(&self, arguments: &Value, start_key: &str, end_key: &str) -> Result<(PathBuf, usize, String), RpcError> {
        let path = self.control_path(arguments)?;
        let text = self.agent_text_of(&path).map_err(invalid)?;
        let start = arguments.get(start_key).and_then(|v| v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse().ok()))).unwrap_or(1).max(1) as usize;
        let end = arguments.get(end_key).and_then(|v| v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse().ok()))).map(|e| e as usize).unwrap_or(start).max(start);
        let lines: Vec<&str> = text.lines().collect();
        let picked = lines.iter().skip(start - 1).take(end + 1 - start).copied().collect::<Vec<_>>().join("\n");
        Ok((path, start, picked))
    }

    /// The text of one of Fenix's own prompts.
    fn agent_fenix_prompt(&self, name: &str, arguments: &Value) -> Result<String, RpcError> {
        match name {
            "fix_diagnostic" => {
                let (path, line, code) = self.agent_lines(arguments, "line", "line")?;
                let canonical = fenix_lsp::normalize(std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone()));
                let problems: Vec<String> = self.all_diagnostics(&canonical).into_iter().filter(|d| (d.range.start.line as usize) < line && d.range.end.line as usize + 1 >= line).map(|d| format!("- {}{}", d.message, d.source.map(|s| format!(" ({s})")).unwrap_or_default())).collect();
                let problems = if problems.is_empty() { "- (the language server reports nothing on that line now)".to_string() } else { problems.join("\n") };
                Ok(format!("Fix the problem on line {line} of {}.\n\nWhat the language server says:\n{problems}\n\nThe line:\n```\n{code}\n```\n\nMake the smallest change that fixes it; `code_actions` lists the language server's own fixes for that line. Don't change anything else.", path.display()))
            }
            "explain" => {
                let (path, start, code) = self.agent_lines(arguments, "startLine", "endLine")?;
                Ok(format!("Explain what this code from {} (from line {start}) does, and why it's written this way. Use `hover` and `definition` on the names you need to understand.\n\n```\n{code}\n```", path.display()))
            }
            "review_changes" => {
                let root = self.integration_root();
                let repo = crate::agent_work::repo_of(&root).map_err(invalid)?;
                let diff = match arguments.get("base").and_then(Value::as_str).filter(|b| !b.is_empty()) {
                    Some(base) => fenix_git::diff_refs(&repo, base, "HEAD", true),
                    None => fenix_git::file_diff(&repo, ".", false),
                }
                .map_err(invalid)?;
                let diff = crate::agent_work::cap(&diff, self.agent_answer_max(), "git_diff with a path reads one file at a time");
                Ok(format!("Review these changes. For each finding give the file, the line, how serious it is, and what to do. Check the callers of what changed with `references`. Don't change any files.\n\n```diff\n{}\n```", diff.as_str().unwrap_or_default()))
            }
            "write_tests" => {
                let (path, line, _) = self.agent_lines(arguments, "line", "line")?;
                Ok(format!("Write tests for the function at line {line} of {}. First read it (`syntax_node` gives its extent), its callers (`references`) and the project's existing tests, and follow their style and place. Then run them (`run_task`).", path.display()))
            }
            other => Err(invalid(format!("no prompt {other}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fenix_resources_are_its_read_tools() {
        assert_eq!(resource_tool("fenix://selection"), Some(("selection", json!({}))));
        assert_eq!(resource_tool("fenix://buffer/home/me/a%20b.rs"), Some(("read_buffer", json!({ "path": "/home/me/a b.rs" }))));
        assert_eq!(resource_tool("fenix://buffer/C:/x.rs"), Some(("read_buffer", json!({ "path": "C:/x.rs" }))));
        assert_eq!(resource_tool("fenix://diagnostics/p/a.rs"), Some(("diagnostics", json!({ "path": "/p/a.rs" }))));
        assert_eq!(resource_tool("https://x"), None);
        assert_eq!(encode("/home/me/a b.rs"), "home/me/a%20b.rs");
    }

    #[test]
    fn a_template_gives_its_parameters() {
        assert_eq!(match_template("fenix://ext/jira/issue/{key}", "fenix://ext/jira/issue/ABC-1"), Some(BTreeMap::from([("key".into(), "ABC-1".into())])));
        assert_eq!(match_template("db://{name}/table/{table}", "db://main/table/users"), Some(BTreeMap::from([("name".into(), "main".into()), ("table".into(), "users".into())])));
        assert_eq!(match_template("fenix://ext/jira/issue/{key}", "fenix://ext/other/1"), None);
        assert_eq!(match_template("fenix://fixed", "fenix://fixed"), Some(BTreeMap::new()));
    }

    fn ask(app: &mut App, method: &str, params: Value) -> Reply {
        let (reply, answer) = std::sync::mpsc::channel();
        app.agent_context(method, AgentCaller { client: "c".into(), thread: None }, params, reply);
        answer.recv_timeout(Duration::from_secs(10)).unwrap()
    }

    #[test]
    fn resources_list_and_read_and_a_subscription_hears_of_a_change() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.rs");
        std::fs::write(&file, "fn a() {}\n").unwrap();
        let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
        let listed = ask(&mut app, "resources", json!({})).unwrap();
        let uris: Vec<&str> = listed["resources"].as_array().unwrap().iter().map(|r| r["uri"].as_str().unwrap()).collect();
        let buffer = uris.iter().find(|u| u.starts_with("fenix://buffer/")).expect("the open file").to_string();
        let read = ask(&mut app, "read", json!({ "uri": buffer })).unwrap();
        assert_eq!(read["contents"][0]["text"], "fn a() {}\n", "{read}");
        assert!(ask(&mut app, "read", json!({ "uri": "fenix://nothing" })).is_err());
        ask(&mut app, "subscribe", json!({ "uri": "fenix://selection" })).unwrap();
        let start = Instant::now();
        app.agent_subscriptions_tick(start);
        app.agent_calls_tick(start);
        // Nothing changed: no news.
        app.agent_subscriptions_tick(start + LOOK_EVERY);
        assert!(!app.agent_calls_news_has("resource"));
    }

    #[test]
    fn prompts_are_listed_and_written() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.rs");
        std::fs::write(&file, "fn a() {}\nfn b() { a() }\n").unwrap();
        let mut app = App::with_file(None);
        let listed = ask(&mut app, "prompts", json!({})).unwrap();
        assert_eq!(listed["prompts"].as_array().unwrap().len(), PROMPTS.len());
        let explain = ask(&mut app, "prompt", json!({ "name": "explain", "arguments": { "path": file.to_string_lossy(), "startLine": "2" } })).unwrap();
        let text = explain["messages"][0]["content"]["text"].as_str().unwrap();
        assert!(text.contains("fn b() { a() }") && !text.contains("fn a() {}"), "{text}");
        assert!(ask(&mut app, "prompt", json!({ "name": "nope" })).is_err());
    }
}
