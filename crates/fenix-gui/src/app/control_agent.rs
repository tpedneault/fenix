//! What the coding agents can ask of Fenix beyond the open files: an open
//! file's text as it stands, the language servers' answers about any file,
//! the debugger's stop, and the tools extensions offer them. `fenix --mcp`
//! makes tools of these (`crate::mcp`); like the rest of the control
//! channel they're there for any other client too.
//!
//! A language server's answer and an adapter's come later, so those are
//! answered on the connection's `reply` once they do, as an extension's
//! service is (`App::control_by_extension`).

use std::hash::{Hash, Hasher};
use std::sync::mpsc::Sender;

use fenix_control::RpcError;
use serde_json::{json, Value};

use super::*;
use crate::ipc::Reply;

/// The most of a file `fenix/buffer.read` answers with, in characters.
const MAX_READ_CHARS: usize = 1 << 20;
/// The most locations an answer lists.
const MAX_LOCATIONS: usize = 200;
/// How long a language server gets: a project-wide question can take some
/// time on a large project, and nobody's waiting at a keystroke.
const LSP_CONTROL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);
/// How much of the debuggee's output `fenix/debug.state` carries, in lines.
const CONSOLE_LINES: usize = 40;

/// What's asked of the language servers (`fenix/lsp`'s `kind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LspQuestion {
    Definition,
    References,
    Hover,
    DocumentSymbols,
    WorkspaceSymbols,
}

impl LspQuestion {
    fn parse(kind: &str) -> Option<LspQuestion> {
        Some(match kind {
            "definition" => LspQuestion::Definition,
            "references" => LspQuestion::References,
            "hover" => LspQuestion::Hover,
            "document_symbols" => LspQuestion::DocumentSymbols,
            "workspace_symbols" => LspQuestion::WorkspaceSymbols,
            _ => return None,
        })
    }
}

/// The zero-based character in `line` that `params` means: `column`
/// (from 1), or where `symbol` starts as a whole word, or the line's
/// first non-blank.
pub(super) fn column_in(line: &str, params: &Value) -> Result<usize, RpcError> {
    if let Some(symbol) = params.get("symbol").and_then(Value::as_str) {
        return word_at(line, symbol).ok_or_else(|| RpcError::invalid_params(format!("{symbol} isn't on that line: {}", line.trim_end())));
    }
    if let Some(column) = params.get("column").and_then(Value::as_u64) {
        return Ok((column as usize).saturating_sub(1));
    }
    Ok(line.chars().take_while(|c| c.is_whitespace()).count())
}

/// The character where `word` starts in `line`, standing alone.
fn word_at(line: &str, word: &str) -> Option<usize> {
    if word.is_empty() {
        return None;
    }
    let part_of_word = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_');
    line.match_indices(word)
        .find(|(at, _)| !part_of_word(line[..*at].chars().next_back()) && !part_of_word(line[at + word.len()..].chars().next()))
        .map(|(at, _)| line[..at].chars().count())
}

/// `path` from a server's URI, written as Fenix writes it: on Windows a
/// URI's drive letter usually comes back in lower case.
fn path_text(path: &Path) -> String {
    let text = path.to_string_lossy();
    let mut chars = text.chars();
    match (chars.next(), chars.next()) {
        (Some(drive), Some(':')) if cfg!(windows) => format!("{}{}", drive.to_ascii_uppercase(), &text[1..]),
        _ => text.into_owned(),
    }
}

fn symbol_kind(kind: lsp_types::SymbolKind) -> String {
    format!("{kind:?}").trim_start_matches("SymbolKind(").trim_end_matches(')').to_lowercase()
}

/// A nested outline, flattened: `depth` says how far in each is.
fn flat_symbols(symbols: Vec<lsp_types::DocumentSymbol>, depth: usize, out: &mut Vec<Value>) {
    for symbol in symbols {
        out.push(json!({ "name": symbol.name, "kind": symbol_kind(symbol.kind), "line": symbol.range.start.line + 1, "endLine": symbol.range.end.line + 1, "depth": depth, "detail": symbol.detail }));
        flat_symbols(symbol.children.unwrap_or_default(), depth + 1, out);
    }
}

/// The servers' answers to `question`, as tools read them. `line_of`
/// gives a file's line, for the locations to show what's there.
pub(super) fn lsp_answer(question: LspQuestion, answers: Vec<Value>, line_of: &mut dyn FnMut(&Path, u32) -> Option<String>) -> Value {
    let located = |locations: Vec<lsp_types::Location>, line_of: &mut dyn FnMut(&Path, u32) -> Option<String>| {
        let total = locations.len();
        let list: Vec<Value> = locations
            .into_iter()
            .take(MAX_LOCATIONS)
            .filter_map(|l| {
                let path = fenix_lsp::uri_to_path(&l.uri)?;
                let text = line_of(&path, l.range.start.line).map(|t| t.trim().to_string());
                Some(json!({ "path": path_text(&path), "line": l.range.start.line + 1, "column": l.range.start.character + 1, "text": text }))
            })
            .collect();
        json!({ "locations": list, "total": total, "truncated": total > MAX_LOCATIONS })
    };
    match question {
        LspQuestion::Definition => {
            let each = answers.into_iter().map(|v| serde_json::from_value::<Option<lsp_types::GotoDefinitionResponse>>(v).ok().flatten().map(fenix_lsp::fanout::locations_of).unwrap_or_default());
            located(fenix_lsp::fanout::merge_locations(each), line_of)
        }
        LspQuestion::References => {
            let each = answers.into_iter().map(|v| serde_json::from_value::<Option<Vec<lsp_types::Location>>>(v).ok().flatten().unwrap_or_default());
            located(fenix_lsp::fanout::merge_locations(each), line_of)
        }
        LspQuestion::Hover => {
            let each = answers.into_iter().map(|v| serde_json::from_value::<Option<lsp_types::Hover>>(v).ok().flatten().map(|h| crate::lsp::hover_contents_text(h.contents)));
            json!({ "hover": fenix_lsp::fanout::join_hovers(each) })
        }
        LspQuestion::DocumentSymbols => {
            let mut symbols = Vec::new();
            for answer in answers {
                match serde_json::from_value::<Option<lsp_types::DocumentSymbolResponse>>(answer).ok().flatten() {
                    Some(lsp_types::DocumentSymbolResponse::Nested(nested)) => flat_symbols(nested, 0, &mut symbols),
                    Some(lsp_types::DocumentSymbolResponse::Flat(flat)) => symbols.extend(flat.into_iter().map(|s| json!({ "name": s.name, "kind": symbol_kind(s.kind), "line": s.location.range.start.line + 1, "endLine": s.location.range.end.line + 1, "depth": 0, "container": s.container_name }))),
                    None => {}
                }
            }
            json!({ "symbols": symbols })
        }
        LspQuestion::WorkspaceSymbols => {
            let mut symbols = Vec::new();
            for answer in answers {
                match serde_json::from_value::<Option<lsp_types::WorkspaceSymbolResponse>>(answer).ok().flatten() {
                    Some(lsp_types::WorkspaceSymbolResponse::Flat(flat)) => symbols.extend(flat.into_iter().filter_map(|s| {
                        let path = fenix_lsp::uri_to_path(&s.location.uri)?;
                        Some(json!({ "name": s.name, "kind": symbol_kind(s.kind), "path": path_text(&path), "line": s.location.range.start.line + 1, "container": s.container_name }))
                    })),
                    Some(lsp_types::WorkspaceSymbolResponse::Nested(nested)) => symbols.extend(nested.into_iter().filter_map(|s| {
                        let (uri, line) = match s.location {
                            lsp_types::OneOf::Left(l) => (l.uri, Some(l.range.start.line + 1)),
                            lsp_types::OneOf::Right(l) => (l.uri, None),
                        };
                        let path = fenix_lsp::uri_to_path(&uri)?;
                        Some(json!({ "name": s.name, "kind": symbol_kind(s.kind), "path": path_text(&path), "line": line, "container": s.container_name }))
                    })),
                    None => {}
                }
            }
            let total = symbols.len();
            symbols.truncate(MAX_LOCATIONS);
            json!({ "symbols": symbols, "total": total, "truncated": total > MAX_LOCATIONS })
        }
    }
}

/// A variable and what's been fetched inside it.
fn var_value(var: &debug_drawer::DebugVar) -> Value {
    let mut out = json!({ "name": var.name, "value": var.value });
    if let Some(children) = &var.children {
        out["children"] = children.iter().map(var_value).collect();
    }
    out
}

impl App {
    /// `path` from `params`, against the project when relative.
    pub(super) fn control_path(&self, params: &Value) -> Result<PathBuf, RpcError> {
        let path = params.get("path").and_then(Value::as_str).ok_or_else(|| RpcError::invalid_params("path: which file"))?;
        let path = PathBuf::from(path);
        Ok(if path.is_absolute() { path } else { self.integration_root().join(path) })
    }

    /// `fenix/buffer.read`: an open file as it stands in Fenix, unsaved
    /// edits and all, whole or `startLine` to `endLine` (from 1).
    pub(super) fn control_buffer_read(&self, params: &Value) -> Result<Value, RpcError> {
        let path = self.control_path(params)?;
        let ob = self.buffers.id_for_path(&path).and_then(|id| self.buffers.get(id)).ok_or_else(|| RpcError::invalid_params(format!("{} isn't open in Fenix: read it from disk", path.display())))?;
        let lines = ob.buffer.line_count();
        let line_param = |name: &str| params.get(name).and_then(Value::as_u64).map(|n| n as usize);
        let first = line_param("startLine").unwrap_or(1).clamp(1, lines.max(1));
        let last = line_param("endLine").unwrap_or(lines).clamp(first, lines.max(1));
        let start = ob.buffer.line_start_char(first - 1);
        let end = ob.buffer.line_start_char(last - 1) + ob.buffer.line_len(last - 1);
        let truncated = end - start > MAX_READ_CHARS;
        let text = ob.buffer.text_range(start, end.min(start + MAX_READ_CHARS));
        Ok(json!({ "path": path.to_string_lossy(), "unsaved": ob.buffer.is_dirty(), "lines": lines, "startLine": first, "endLine": last, "text": text, "truncated": truncated }))
    }

    /// `fenix/lsp`: answered on `reply` once the servers have.
    pub(super) fn control_lsp(&mut self, params: Value, reply: Sender<Reply>) {
        if let Err(error) = self.control_lsp_ask(&params, &reply) {
            let _ = reply.send(Err(error));
        }
    }

    fn control_lsp_ask(&mut self, params: &Value, reply: &Sender<Reply>) -> Result<(), RpcError> {
        let kind = params.get("kind").and_then(Value::as_str).unwrap_or("");
        let question = LspQuestion::parse(kind).ok_or_else(|| RpcError::invalid_params(format!("no such question: {kind}")))?;
        if question == LspQuestion::WorkspaceSymbols {
            return self.control_workspace_symbols(params, reply);
        }
        let path = self.control_path(params)?;
        let (text, version) = match self.buffers.id_for_path(&path).and_then(|id| self.buffers.get(id)) {
            Some(ob) => (ob.buffer.text(), ob.buffer.edit_count()),
            None => {
                let text = std::fs::read_to_string(&path).map_err(|e| RpcError::invalid_params(format!("{}: {e}", path.display())))?;
                // What's on disk has no edit count: its text stands in, so
                // a file an agent just changed reaches the server.
                let mut hash = std::collections::hash_map::DefaultHasher::new();
                text.hash(&mut hash);
                let version = hash.finish();
                (text, version)
            }
        };
        self.sync_lsp_for(&path, &text, version);
        let canonical = fenix_lsp::normalize(std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone()));
        let all = self.cached_lsp_keys(&path);
        if all.is_empty() {
            return Err(RpcError::invalid_params(format!("no language server for {}", path.display())));
        }
        let keys: Vec<LspKey> = all.into_iter().filter(|key| self.lsp_sessions.get(key).is_some_and(|s| s.capabilities.is_some() && s.open_documents.contains_key(&canonical))).collect();
        if keys.is_empty() {
            let starting = self.cached_lsp_keys(&path).iter().any(|key| self.lsp_sessions.get(key).is_none_or(|s| s.capabilities.is_none()));
            return Err(RpcError::invalid_params(if starting { "its language server may still be starting: ask again in a moment" } else { "no language server here takes this file" }));
        }
        let uri = fenix_lsp::path_to_uri(&canonical).ok_or_else(|| RpcError::invalid_params("that path has no URI"))?;
        let text_document = lsp_types::TextDocumentIdentifier { uri };
        let targets = match question {
            LspQuestion::Definition => self.lsp_route(&keys, fenix_lsp::fanout::Feature::Definition),
            LspQuestion::References => self.lsp_route(&keys, fenix_lsp::fanout::Feature::References),
            LspQuestion::Hover => self.lsp_route(&keys, fenix_lsp::fanout::Feature::Hover),
            _ => keys.iter().filter(|key| self.lsp_sessions[*key].capabilities.as_ref().is_some_and(|c| c.document_symbol_provider.is_some())).cloned().collect(),
        };
        if targets.is_empty() {
            return Err(RpcError::invalid_params(format!("no language server here answers {kind}")));
        }
        if question == LspQuestion::DocumentSymbols {
            let fanout = self.start_control_lsp(question, targets.clone(), reply.clone());
            for key in targets {
                let params = lsp_types::DocumentSymbolParams { text_document: text_document.clone(), work_done_progress_params: Default::default(), partial_result_params: Default::default() };
                self.send_lsp_fanout_request::<lsp_types::request::DocumentSymbolRequest>(&key, params, PendingLspRequest::Control { fanout });
            }
            return Ok(());
        }
        let line = params.get("line").and_then(Value::as_u64).ok_or_else(|| RpcError::invalid_params("line: which line, from 1"))? as usize;
        let buffer = fenix_core::Buffer::from_text(&text);
        if line == 0 || line > buffer.line_count() {
            return Err(RpcError::invalid_params(format!("{} has {} lines", path.display(), buffer.line_count())));
        }
        let start = buffer.line_start_char(line - 1);
        let line_text = buffer.text_range(start, start + buffer.line_len(line - 1));
        let column = column_in(&line_text, params)?;
        let position = fenix_lsp::char_offset_to_position(buffer.rope(), start + column.min(buffer.line_len(line - 1)));
        let at = lsp_types::TextDocumentPositionParams { text_document, position };
        let fanout = self.start_control_lsp(question, targets.clone(), reply.clone());
        for key in targets {
            match question {
                LspQuestion::Definition => {
                    let params = lsp_types::GotoDefinitionParams { text_document_position_params: at.clone(), work_done_progress_params: Default::default(), partial_result_params: Default::default() };
                    self.send_lsp_fanout_request::<lsp_types::request::GotoDefinition>(&key, params, PendingLspRequest::Control { fanout });
                }
                LspQuestion::References => {
                    let params = lsp_types::ReferenceParams { text_document_position: at.clone(), work_done_progress_params: Default::default(), partial_result_params: Default::default(), context: lsp_types::ReferenceContext { include_declaration: true } };
                    self.send_lsp_fanout_request::<lsp_types::request::References>(&key, params, PendingLspRequest::Control { fanout });
                }
                _ => {
                    let params = lsp_types::HoverParams { text_document_position_params: at.clone(), work_done_progress_params: Default::default() };
                    self.send_lsp_fanout_request::<lsp_types::request::HoverRequest>(&key, params, PendingLspRequest::Control { fanout });
                }
            }
        }
        Ok(())
    }

    /// `workspace_symbols`: of every running server in the project.
    fn control_workspace_symbols(&mut self, params: &Value, reply: &Sender<Reply>) -> Result<(), RpcError> {
        let query = params.get("query").and_then(Value::as_str).ok_or_else(|| RpcError::invalid_params("query: what to look for"))?.to_string();
        let root = self.integration_root();
        let targets: Vec<LspKey> = self.lsp_sessions.iter().filter(|(key, s)| key.root == root && s.capabilities.as_ref().is_some_and(|c| c.workspace_symbol_provider.is_some())).map(|(key, _)| key.clone()).collect();
        if targets.is_empty() {
            return Err(RpcError::invalid_params("no language server is running in this project yet: open one of its files"));
        }
        let fanout = self.start_control_lsp(LspQuestion::WorkspaceSymbols, targets.clone(), reply.clone());
        for key in targets {
            let params = lsp_types::WorkspaceSymbolParams { query: query.clone(), work_done_progress_params: Default::default(), partial_result_params: Default::default() };
            self.send_lsp_fanout_request::<lsp_types::request::WorkspaceSymbolRequest>(&key, params, PendingLspRequest::Control { fanout });
        }
        Ok(())
    }

    /// Gathers `targets`' answers to `question`, with a wake-up at the
    /// timeout for a server that never gives one.
    fn start_control_lsp(&mut self, question: LspQuestion, targets: Vec<LspKey>, reply: Sender<Reply>) -> u64 {
        let deadline = std::time::Instant::now() + LSP_CONTROL_TIMEOUT;
        let id = self.next_lsp_fanout;
        self.next_lsp_fanout += 1;
        self.lsp_fanouts.insert(id, LspFanout::Control { question, gather: fenix_lsp::fanout::Gather::new(targets, deadline), reply });
        if let Some(proxy) = self.event_proxy.clone() {
            std::thread::spawn(move || {
                std::thread::sleep(LSP_CONTROL_TIMEOUT);
                let _ = proxy.send_event(FenixUserEvent::LspDeadline(id));
            });
        }
        id
    }

    /// What the servers gave, to the client that asked.
    pub(super) fn control_lsp_answered(&self, question: LspQuestion, answers: Vec<Value>, reply: Sender<Reply>) {
        let mut files: HashMap<PathBuf, Option<Vec<String>>> = HashMap::new();
        let mut line_of = |path: &Path, line: u32| -> Option<String> {
            if let Some(ob) = self.buffers.id_for_path(path).and_then(|id| self.buffers.get(id)) {
                let line = line as usize;
                if line >= ob.buffer.line_count() {
                    return None;
                }
                let start = ob.buffer.line_start_char(line);
                return Some(ob.buffer.text_range(start, start + ob.buffer.line_len(line)));
            }
            let lines = files.entry(path.to_path_buf()).or_insert_with(|| std::fs::read_to_string(path).ok().map(|text| text.lines().map(str::to_string).collect()));
            lines.as_ref()?.get(line as usize).cloned()
        };
        let _ = reply.send(Ok(lsp_answer(question, answers, &mut line_of)));
    }

    /// `fenix/debug.state`: where the debugger is stopped and what it can
    /// see there.
    pub(super) fn control_debug_state(&self) -> Value {
        let Some(session) = &self.debug_session else { return json!({ "session": false }) };
        let frames: Vec<Value> = session.call_stack.iter().map(|(function, path, line)| json!({ "function": function, "path": path.to_string_lossy(), "line": line })).collect();
        let scopes: Vec<Value> = session.variables.iter().filter(|s| s.vars.is_some()).map(|s| json!({ "scope": s.name, "variables": s.vars.iter().flatten().map(var_value).collect::<Vec<_>>() })).collect();
        let console: Vec<&String> = session.console.iter().rev().take(CONSOLE_LINES).rev().collect();
        json!({ "session": true, "name": session.name, "ended": session.ended, "stopped": session.stopped_thread.is_some(), "selectedFrame": session.selected_frame, "frames": frames, "scopes": scopes, "watches": session.watch_values, "console": console })
    }

    /// `fenix/debug.evaluate`: an expression in the selected frame, which
    /// may run code in the program, so only with `agents.mcp.fenix_write`.
    pub(super) fn control_debug_evaluate(&mut self, params: Value, reply: Sender<Reply>) {
        if let Err(error) = self.control_writes_allowed() {
            let _ = reply.send(Err(error));
            return;
        }
        let Some(expression) = params.get("expression").and_then(Value::as_str).map(str::to_string) else {
            let _ = reply.send(Err(RpcError::invalid_params("expression: what to evaluate")));
            return;
        };
        let Some(session) = self.debug_session.as_mut().filter(|s| s.stopped_thread.is_some()) else {
            let _ = reply.send(Err(RpcError::invalid_params("the debugger isn't stopped")));
            return;
        };
        let on = session.stopped_on;
        let args = fenix_dap::requests::EvaluateRequestArguments::builder().expression(expression).frame_id(session.current_frame_id).context(Some(fenix_dap::requests::EvaluateRequestContext::REPL)).build();
        let pending = PendingDapRequest::Control { reply: reply.clone() };
        if !session.send(on, fenix_dap::requests::Request::Evaluate(args), pending) {
            let _ = reply.send(Err(RpcError::internal("the adapter isn't there")));
        }
    }

    /// The tools the extensions that are on offer agents: those installed
    /// and switched on (`wasm.installed` holds only those), whatever
    /// their state -- one stopped is listed, marked so.
    fn extension_tools(&self) -> Vec<(String, String, fenix_extensions::manifest::AgentToolContribution)> {
        let mut tools = Vec::new();
        for extension in &self.wasm.installed {
            let toolset = fenix_extensions::manifest::toolset_of(&extension.id);
            for tool in &extension.manifest.contributes.agent_tools {
                tools.push((format!("{toolset}_{}", tool.name), extension.id.clone(), tool.clone()));
            }
        }
        tools
    }

    /// `fenix/tools`: what `fenix/tools.call` can run. Each tool has its
    /// class (`read`, `show`, `edit`, `run`, `external`), its toolset (the
    /// extension's short name, its name's prefix), whether it's offered by
    /// default, its deadline in milliseconds, and `stopped` while its
    /// extension is stopped (with why).
    pub(super) fn control_tools(&self) -> Value {
        let tools: Vec<Value> = self
            .extension_tools()
            .into_iter()
            .map(|(name, extension, tool)| {
                let schema = tool.input.as_deref().and_then(|text| serde_json::from_str::<Value>(text).ok()).unwrap_or_else(|| json!({ "type": "object" }));
                let mut listed = json!({
                    "name": name,
                    "description": tool.description,
                    "inputSchema": schema,
                    "extension": extension,
                    "writes": tool.writes,
                    "class": tool.effective_class().as_str(),
                    "toolset": fenix_extensions::manifest::toolset_of(&extension),
                    "default": tool.default,
                    "timeout_ms": tool.timeout_ms(),
                });
                if let Some(super::wasm_host::Status::Stopped(why)) = self.wasm.status.get(&extension) {
                    listed["stopped"] = json!(true);
                    listed["stoppedWhy"] = json!(why);
                }
                listed
            })
            .collect();
        json!({ "tools": tools })
    }

    /// `fenix/tools.call`: answered on `reply` by the extension's service.
    pub(super) fn control_tools_call(&mut self, params: Value, reply: Sender<Reply>) {
        self.control_tools_call_with(params, reply, None);
    }

    /// `fenix/tools.call`, its progress (`service-progress`) given to
    /// `progress` as it comes. The call's deadline is its tool's
    /// `timeout_ms`, an answer given later included. The id is what
    /// `code_serve_cancel` takes to give it up -- none when it was
    /// answered at once (an unknown tool, one held back).
    pub(crate) fn control_tools_call_with(&mut self, params: Value, reply: Sender<Reply>, progress: Option<super::wasm_host::OnProgress>) -> Option<super::wasm_host::ServeId> {
        let name = params.get("name").and_then(Value::as_str).unwrap_or("").to_string();
        let Some((_, extension, tool)) = self.extension_tools().into_iter().find(|(n, _, _)| *n == name) else {
            let _ = reply.send(Err(RpcError::invalid_params(format!("no tool {name}"))));
            return None;
        };
        if tool.writes {
            if let Err(error) = self.control_writes_allowed() {
                let _ = reply.send(Err(error));
                return None;
            }
        }
        let payload = params.get("arguments").cloned().unwrap_or_else(|| json!({})).to_string();
        let then = move |_: &mut App, said: Result<String, String>| {
            let answer = said.map_err(RpcError::invalid_params).map(|text| serde_json::from_str::<Value>(&text).unwrap_or(Value::String(text)));
            let _ = reply.send(answer);
        };
        let options = super::wasm_host::ServeOptions { timeout: Some(std::time::Duration::from_millis(u64::from(tool.timeout_ms()))), progress };
        Some(self.code_serve_with(&extension, &tool.service, payload, super::wasm_host::Answer::Fenix(Box::new(then)), options))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_symbol_is_found_as_a_whole_word() {
        assert_eq!(word_at("    let total = totals(x);", "total"), Some(8));
        assert_eq!(word_at("totals and total", "total"), Some(11));
        assert_eq!(word_at("é total", "total"), Some(2), "counted in characters");
        assert_eq!(word_at("subtotal", "total"), None);
    }

    #[cfg(windows)]
    #[test]
    fn a_servers_path_is_written_as_fenixs_are() {
        assert_eq!(path_text(Path::new(r"c:\Users\a.rs")), r"C:\Users\a.rs");
    }

    #[test]
    fn a_position_is_a_column_a_symbol_or_the_first_word() {
        assert_eq!(column_in("    foo(bar)", &json!({ "symbol": "bar" })).unwrap(), 8);
        assert_eq!(column_in("    foo(bar)", &json!({ "column": 5 })).unwrap(), 4);
        assert_eq!(column_in("    foo(bar)", &json!({})).unwrap(), 4);
        assert!(column_in("foo", &json!({ "symbol": "baz" })).is_err());
    }

    fn uri(path: &str) -> String {
        fenix_lsp::path_to_uri(&std::env::temp_dir().join(path)).unwrap().to_string()
    }

    #[test]
    fn locations_read_from_one_and_show_their_line() {
        let answer = json!([{ "uri": uri("a.rs"), "range": { "start": { "line": 4, "character": 2 }, "end": { "line": 4, "character": 5 } } }]);
        let out = lsp_answer(LspQuestion::References, vec![answer], &mut |_, line| Some(format!("  line {line}  ")));
        assert_eq!(out["total"], 1);
        assert_eq!(out["locations"][0]["line"], 5);
        assert_eq!(out["locations"][0]["column"], 3);
        assert_eq!(out["locations"][0]["text"], "line 4");
    }

    #[test]
    fn several_servers_answers_are_merged() {
        let one = json!({ "uri": uri("a.rs"), "range": { "start": { "line": 1, "character": 0 }, "end": { "line": 1, "character": 1 } } });
        let out = lsp_answer(LspQuestion::Definition, vec![one.clone(), one, Value::Null], &mut |_, _| None);
        assert_eq!(out["total"], 1, "the same place once");
    }

    #[test]
    fn a_long_list_says_it_was_cut() {
        let many: Vec<Value> = (0..MAX_LOCATIONS + 5).map(|i| json!({ "uri": uri("a.rs"), "range": { "start": { "line": i, "character": 0 }, "end": { "line": i, "character": 1 } } })).collect();
        let out = lsp_answer(LspQuestion::References, vec![Value::Array(many)], &mut |_, _| None);
        assert_eq!(out["locations"].as_array().unwrap().len(), MAX_LOCATIONS);
        assert_eq!(out["truncated"], true);
        assert_eq!(out["total"], MAX_LOCATIONS + 5);
    }

    #[test]
    fn hover_and_outlines_read_plainly() {
        let out = lsp_answer(LspQuestion::Hover, vec![json!({ "contents": "fn main()" })], &mut |_, _| None);
        assert_eq!(out["hover"], "fn main()");
        let range = json!({ "start": { "line": 0, "character": 0 }, "end": { "line": 3, "character": 0 } });
        let outline = json!([{ "name": "Foo", "kind": 5, "range": range, "selectionRange": range, "children": [{ "name": "bar", "kind": 6, "range": range, "selectionRange": range }] }]);
        let out = lsp_answer(LspQuestion::DocumentSymbols, vec![outline], &mut |_, _| None);
        assert_eq!(out["symbols"][0]["name"], "Foo");
        assert_eq!(out["symbols"][1]["depth"], 1);
        assert_eq!(out["symbols"][0]["endLine"], 4);
    }
}
