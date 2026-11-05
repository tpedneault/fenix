//! The `lsp` and `refactor` toolsets: what an agent asks the language
//! servers by path and position (implementations, call and type
//! hierarchies, signature help, code actions), and the edits it has them
//! make (rename, a code action, format). Requests go to one server, the
//! primary when it can answer; the answer comes back through
//! `apply_lsp_response` as `PendingLspRequest::Agent`. Edits go into the
//! caller's thread's change ledger (`agent_edit_file`), so the user
//! reviews them in Agent changes like the agent's own.

use std::sync::mpsc::Sender;

use fenix_control::{AgentCaller, RpcError};
use serde_json::{json, Value};

use super::*;
use crate::ipc::Reply;

/// How long a server gets to answer an agent.
const AGENT_LSP_TIMEOUT: Duration = Duration::from_secs(20);

/// The most items an answer lists.
const MAX_ITEMS: usize = 200;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Hierarchy {
    CallsIn,
    CallsOut,
    Supertypes,
    Subtypes,
}

/// What an answer is for, and so what's done with it.
pub(super) enum Step {
    Implementations,
    /// The prepare request's items: ask `direction` of the first.
    Prepare(Hierarchy),
    Hierarchy(Hierarchy),
    Signature,
    CodeActions,
    Resolve,
    Rename,
    Format { path: PathBuf, before: String },
}

pub(super) struct Waiting {
    key: LspKey,
    step: Step,
    reply: Sender<Reply>,
    caller: AgentCaller,
    until: Instant,
}

/// The servers' code actions an agent was shown, to apply by id.
#[derive(Default)]
pub(crate) struct AgentLsp {
    waiting: HashMap<u64, Waiting>,
    next: u64,
    actions: HashMap<String, (LspKey, lsp_types::CodeActionOrCommand)>,
    next_action: u64,
}

fn invalid(message: impl Into<String>) -> RpcError {
    RpcError::invalid_params(message)
}

/// Where a location is, as agents are told.
fn place(uri: &lsp_types::Uri, range: &lsp_types::Range) -> Value {
    let path = fenix_lsp::uri_to_path(uri).map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|| uri.as_str().to_string());
    json!({ "path": path, "line": range.start.line + 1, "column": range.start.character + 1 })
}

fn locations(value: Value) -> Vec<Value> {
    let mut out = Vec::new();
    match serde_json::from_value::<Option<lsp_types::GotoDefinitionResponse>>(value).ok().flatten() {
        Some(lsp_types::GotoDefinitionResponse::Scalar(l)) => out.push(place(&l.uri, &l.range)),
        Some(lsp_types::GotoDefinitionResponse::Array(ls)) => out.extend(ls.iter().map(|l| place(&l.uri, &l.range))),
        Some(lsp_types::GotoDefinitionResponse::Link(ls)) => out.extend(ls.iter().map(|l| place(&l.target_uri, &l.target_selection_range))),
        None => {}
    }
    out.truncate(MAX_ITEMS);
    out
}

fn kind_name(kind: lsp_types::SymbolKind) -> String {
    format!("{kind:?}").to_lowercase()
}

/// What `apply_code_action` and friends say they changed.
fn changed(files: &[PathBuf]) -> Value {
    json!({ "changed": files.iter().map(|p| p.to_string_lossy()).collect::<Vec<_>>(), "note": "the user reviews these in Agent changes" })
}

impl App {
    /// The server for `path` that answers `capable`, the document synced
    /// to it, and the position `arguments` means (`line` from 1, then
    /// `symbol` or `column`).
    fn agent_lsp_target(&mut self, arguments: &Value, capable: impl Fn(&lsp_types::ServerCapabilities) -> bool, what: &str) -> Result<(LspKey, lsp_types::TextDocumentIdentifier, Option<lsp_types::Position>, PathBuf, String), RpcError> {
        let path = self.control_path(arguments)?;
        let (text, version) = match self.buffers.id_for_path(&path).and_then(|id| self.buffers.get(id)) {
            Some(ob) => (ob.buffer.text(), ob.buffer.edit_count()),
            None => {
                let text = std::fs::read_to_string(&path).map_err(|e| invalid(format!("{}: {e}", path.display())))?;
                use std::hash::{Hash, Hasher};
                let mut hash = std::collections::hash_map::DefaultHasher::new();
                text.hash(&mut hash);
                (text, hash.finish())
            }
        };
        self.sync_lsp_for(&path, &text, version);
        let canonical = fenix_lsp::normalize(std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone()));
        let keys = self.cached_lsp_keys(&path);
        if keys.is_empty() {
            return Err(invalid(format!("no language server for {}", path.display())));
        }
        let mut ready: Vec<LspKey> = keys.into_iter().filter(|key| self.lsp_sessions.get(key).is_some_and(|s| s.open_documents.contains_key(&canonical) && s.capabilities.as_ref().is_some_and(&capable))).collect();
        if ready.is_empty() {
            let starting = self.cached_lsp_keys(&path).iter().any(|key| self.lsp_sessions.get(key).is_none_or(|s| s.capabilities.is_none()));
            return Err(invalid(if starting { "its language server may still be starting: ask again in a moment".to_string() } else { format!("no language server here answers {what}") }));
        }
        ready.sort_by_key(|k| !k.is_primary());
        let key = ready.remove(0);
        let uri = fenix_lsp::path_to_uri(&canonical).ok_or_else(|| invalid("that path has no URI"))?;
        let position = match arguments.get("line").and_then(Value::as_u64) {
            Some(line) => {
                let buffer = fenix_core::Buffer::from_text(&text);
                let line = line as usize;
                if line == 0 || line > buffer.line_count() {
                    return Err(invalid(format!("{} has {} lines", path.display(), buffer.line_count())));
                }
                let start = buffer.line_start_char(line - 1);
                let line_text = buffer.text_range(start, start + buffer.line_len(line - 1));
                let column = control_agent::column_in(&line_text, arguments)?;
                Some(fenix_lsp::char_offset_to_position(buffer.rope(), start + column.min(buffer.line_len(line - 1))))
            }
            None => None,
        };
        Ok((key, lsp_types::TextDocumentIdentifier { uri }, position, path, text))
    }

    fn agent_lsp_send<R: lsp_types::request::Request>(&mut self, key: LspKey, params: R::Params, step: Step, caller: &AgentCaller, reply: Sender<Reply>) {
        self.agent_lsp.next += 1;
        let id = self.agent_lsp.next;
        self.agent_lsp.waiting.insert(id, Waiting { key: key.clone(), step, reply, caller: caller.clone(), until: Instant::now() + AGENT_LSP_TIMEOUT });
        self.send_lsp_fanout_request::<R>(&key, params, PendingLspRequest::Agent { id });
    }

    /// The `lsp` and `refactor` toolsets: answers `reply`, or gives it
    /// back for a tool that isn't theirs.
    pub(super) fn agent_lsp_call(&mut self, caller: &AgentCaller, name: &str, arguments: Value, reply: Sender<Reply>) -> Result<(), Sender<Reply>> {
        let position_needed = |p: Option<lsp_types::Position>| p.ok_or_else(|| invalid("line: which line, from 1"));
        let result: Result<(), RpcError> = (|| {
            match name {
                "implementations" => {
                    let (key, doc, at, _, _) = self.agent_lsp_target(&arguments, |c| c.implementation_provider.is_some(), "implementations")?;
                    let params = lsp_types::request::GotoImplementationParams { text_document_position_params: lsp_types::TextDocumentPositionParams { text_document: doc, position: position_needed(at)? }, work_done_progress_params: Default::default(), partial_result_params: Default::default() };
                    self.agent_lsp_send::<lsp_types::request::GotoImplementation>(key, params, Step::Implementations, caller, reply.clone());
                }
                "call_hierarchy" => {
                    let direction = if arguments.get("direction").and_then(Value::as_str) == Some("out") { Hierarchy::CallsOut } else { Hierarchy::CallsIn };
                    let (key, doc, at, _, _) = self.agent_lsp_target(&arguments, |c| c.call_hierarchy_provider.is_some(), "call hierarchies")?;
                    let params = lsp_types::CallHierarchyPrepareParams { text_document_position_params: lsp_types::TextDocumentPositionParams { text_document: doc, position: position_needed(at)? }, work_done_progress_params: Default::default() };
                    self.agent_lsp_send::<lsp_types::request::CallHierarchyPrepare>(key, params, Step::Prepare(direction), caller, reply.clone());
                }
                "type_hierarchy" => {
                    let direction = if arguments.get("direction").and_then(Value::as_str) == Some("sub") { Hierarchy::Subtypes } else { Hierarchy::Supertypes };
                    let (key, doc, at, _, _) = self.agent_lsp_target(&arguments, |_| true, "type hierarchies")?;
                    let params = lsp_types::TypeHierarchyPrepareParams { text_document_position_params: lsp_types::TextDocumentPositionParams { text_document: doc, position: position_needed(at)? }, work_done_progress_params: Default::default() };
                    self.agent_lsp_send::<lsp_types::request::TypeHierarchyPrepare>(key, params, Step::Prepare(direction), caller, reply.clone());
                }
                "signature_help" => {
                    let (key, doc, at, _, _) = self.agent_lsp_target(&arguments, |c| c.signature_help_provider.is_some(), "signature help")?;
                    let params = lsp_types::SignatureHelpParams { context: None, text_document_position_params: lsp_types::TextDocumentPositionParams { text_document: doc, position: position_needed(at)? }, work_done_progress_params: Default::default() };
                    self.agent_lsp_send::<lsp_types::request::SignatureHelpRequest>(key, params, Step::Signature, caller, reply.clone());
                }
                "code_actions" => {
                    let (key, doc, _, path, text) = self.agent_lsp_target(&arguments, |c| fenix_lsp::fanout::provides(c, fenix_lsp::fanout::Feature::CodeAction), "code actions")?;
                    let first = arguments.get("line").and_then(Value::as_u64).ok_or_else(|| invalid("line: which line, from 1"))?.max(1) as u32 - 1;
                    let last = arguments.get("endLine").and_then(Value::as_u64).map(|l| l.max(1) as u32 - 1).unwrap_or(first).max(first);
                    let end_char = text.lines().nth(last as usize).map(|l| l.encode_utf16().count() as u32).unwrap_or(0);
                    let range = lsp_types::Range { start: lsp_types::Position { line: first, character: 0 }, end: lsp_types::Position { line: last, character: end_char } };
                    let canonical = fenix_lsp::normalize(std::fs::canonicalize(&path).unwrap_or(path));
                    let diagnostics: Vec<lsp_types::Diagnostic> = self.all_diagnostics(&canonical).into_iter().filter(|d| d.range.start.line <= last && d.range.end.line >= first).collect();
                    let params = lsp_types::CodeActionParams { text_document: doc, range, context: lsp_types::CodeActionContext { diagnostics, only: None, trigger_kind: Some(lsp_types::CodeActionTriggerKind::INVOKED) }, work_done_progress_params: Default::default(), partial_result_params: Default::default() };
                    self.agent_lsp_send::<lsp_types::request::CodeActionRequest>(key, params, Step::CodeActions, caller, reply.clone());
                }
                "apply_code_action" => {
                    let id = arguments.get("id").and_then(Value::as_str).ok_or_else(|| invalid("id: which action, from code_actions"))?;
                    let (key, action) = self.agent_lsp.actions.get(id).cloned().ok_or_else(|| invalid(format!("no code action {id}: code_actions lists them (they last until the next list)")))?;
                    match action {
                        lsp_types::CodeActionOrCommand::CodeAction(action) if action.edit.is_some() => {
                            let answer = self.agent_apply_workspace_edit(caller, action.edit.unwrap());
                            let _ = reply.send(answer);
                        }
                        lsp_types::CodeActionOrCommand::CodeAction(action) if action.data.is_some() => {
                            // Its edit is worked out when it's asked for.
                            self.agent_lsp_send::<lsp_types::request::CodeActionResolveRequest>(key, action, Step::Resolve, caller, reply.clone());
                        }
                        _ => return Err(invalid("this action is a command for the language server to run, which Fenix doesn't apply for agents; make the change another way")),
                    }
                }
                "rename" => {
                    let new_name = arguments.get("new_name").and_then(Value::as_str).filter(|n| !n.is_empty()).ok_or_else(|| invalid("new_name: what to call it"))?.to_string();
                    let (key, doc, at, _, _) = self.agent_lsp_target(&arguments, |c| fenix_lsp::fanout::provides(c, fenix_lsp::fanout::Feature::Rename), "rename")?;
                    let params = lsp_types::RenameParams { text_document_position: lsp_types::TextDocumentPositionParams { text_document: doc, position: position_needed(at)? }, new_name, work_done_progress_params: Default::default() };
                    self.agent_lsp_send::<lsp_types::request::Rename>(key, params, Step::Rename, caller, reply.clone());
                }
                "format" => {
                    let ranged = arguments.get("startLine").and_then(Value::as_u64);
                    let capable = move |c: &lsp_types::ServerCapabilities| if ranged.is_some() { c.document_range_formatting_provider.is_some() } else { fenix_lsp::fanout::provides(c, fenix_lsp::fanout::Feature::Formatting) };
                    let (key, doc, _, path, text) = self.agent_lsp_target(&arguments, capable, "formatting")?;
                    let options = lsp_types::FormattingOptions { tab_size: self.config.int_value("editor.indent_width").unwrap_or(4) as u32, insert_spaces: true, ..Default::default() };
                    match ranged {
                        Some(start) => {
                            let end = arguments.get("endLine").and_then(Value::as_u64).unwrap_or(start).max(start) as u32;
                            let end_char = text.lines().nth(end as usize - 1).map(|l| l.encode_utf16().count() as u32).unwrap_or(0);
                            let range = lsp_types::Range { start: lsp_types::Position { line: start.max(1) as u32 - 1, character: 0 }, end: lsp_types::Position { line: end - 1, character: end_char } };
                            let params = lsp_types::DocumentRangeFormattingParams { text_document: doc, range, options, work_done_progress_params: Default::default() };
                            self.agent_lsp_send::<lsp_types::request::RangeFormatting>(key, params, Step::Format { path, before: text }, caller, reply.clone());
                        }
                        None => {
                            let params = lsp_types::DocumentFormattingParams { text_document: doc, options, work_done_progress_params: Default::default() };
                            self.agent_lsp_send::<lsp_types::request::Formatting>(key, params, Step::Format { path, before: text }, caller, reply.clone());
                        }
                    }
                }
                _ => return Err(invalid("\u{0}")),
            }
            Ok(())
        })();
        match result {
            Ok(()) => Ok(()),
            Err(e) if e.message == "\u{0}" => Err(reply),
            Err(e) => {
                let _ = reply.send(Err(e));
                Ok(())
            }
        }
    }

    /// Applies a workspace edit for the caller, file by file, through its
    /// thread's change ledger.
    fn agent_apply_workspace_edit(&mut self, caller: &AgentCaller, edit: lsp_types::WorkspaceEdit) -> Reply {
        let planned = fenix_lsp::workspace_edit::prepare(edit, |path| {
            let text = self.agent_text_of(path)?;
            Ok(fenix_lsp::workspace_edit::Document { path: path.to_path_buf(), text, version: None })
        })
        .map_err(invalid)?;
        if planned.is_empty() {
            return Ok(json!({ "changed": [], "note": "the language server's edit changes nothing" }));
        }
        let mut files = Vec::new();
        for doc in planned {
            self.agent_edit_file(caller, &doc.path, &doc.after).map_err(invalid)?;
            files.push(doc.path);
        }
        Ok(changed(&files))
    }

    /// A server answered an agent's request (`PendingLspRequest::Agent`).
    pub(super) fn agent_lsp_answered(&mut self, id: u64, result: Result<Value, String>) {
        let Some(waiting) = self.agent_lsp.waiting.remove(&id) else { return };
        let Waiting { key, step, reply, caller, .. } = waiting;
        let value = match result {
            Ok(value) => value,
            Err(why) => {
                let _ = reply.send(Err(invalid(format!("the language server said: {why}"))));
                return;
            }
        };
        let answer: Reply = match step {
            Step::Implementations => Ok(json!({ "implementations": locations(value) })),
            Step::Prepare(direction) => {
                match direction {
                    Hierarchy::CallsIn | Hierarchy::CallsOut => {
                        let items: Vec<lsp_types::CallHierarchyItem> = serde_json::from_value::<Option<Vec<_>>>(value).ok().flatten().unwrap_or_default();
                        let Some(item) = items.into_iter().next() else {
                            let _ = reply.send(Ok(json!({ "calls": [], "note": "nothing callable there" })));
                            return;
                        };
                        if direction == Hierarchy::CallsIn {
                            let params = lsp_types::CallHierarchyIncomingCallsParams { item, work_done_progress_params: Default::default(), partial_result_params: Default::default() };
                            self.agent_lsp_send::<lsp_types::request::CallHierarchyIncomingCalls>(key, params, Step::Hierarchy(direction), &caller, reply);
                        } else {
                            let params = lsp_types::CallHierarchyOutgoingCallsParams { item, work_done_progress_params: Default::default(), partial_result_params: Default::default() };
                            self.agent_lsp_send::<lsp_types::request::CallHierarchyOutgoingCalls>(key, params, Step::Hierarchy(direction), &caller, reply);
                        }
                    }
                    Hierarchy::Supertypes | Hierarchy::Subtypes => {
                        let items: Vec<lsp_types::TypeHierarchyItem> = serde_json::from_value::<Option<Vec<_>>>(value).ok().flatten().unwrap_or_default();
                        let Some(item) = items.into_iter().next() else {
                            let _ = reply.send(Ok(json!({ "types": [], "note": "no type there" })));
                            return;
                        };
                        if direction == Hierarchy::Supertypes {
                            let params = lsp_types::TypeHierarchySupertypesParams { item, work_done_progress_params: Default::default(), partial_result_params: Default::default() };
                            self.agent_lsp_send::<lsp_types::request::TypeHierarchySupertypes>(key, params, Step::Hierarchy(direction), &caller, reply);
                        } else {
                            let params = lsp_types::TypeHierarchySubtypesParams { item, work_done_progress_params: Default::default(), partial_result_params: Default::default() };
                            self.agent_lsp_send::<lsp_types::request::TypeHierarchySubtypes>(key, params, Step::Hierarchy(direction), &caller, reply);
                        }
                    }
                }
                return;
            }
            Step::Hierarchy(direction) => Ok(match direction {
                Hierarchy::CallsIn => {
                    let calls: Vec<lsp_types::CallHierarchyIncomingCall> = serde_json::from_value::<Option<Vec<_>>>(value).ok().flatten().unwrap_or_default();
                    json!({ "callers": calls.iter().take(MAX_ITEMS).map(|c| { let mut p = place(&c.from.uri, &c.from.selection_range); p["name"] = json!(c.from.name); p["kind"] = json!(kind_name(c.from.kind)); p["calls"] = json!(c.from_ranges.len()); p }).collect::<Vec<_>>() })
                }
                Hierarchy::CallsOut => {
                    let calls: Vec<lsp_types::CallHierarchyOutgoingCall> = serde_json::from_value::<Option<Vec<_>>>(value).ok().flatten().unwrap_or_default();
                    json!({ "callees": calls.iter().take(MAX_ITEMS).map(|c| { let mut p = place(&c.to.uri, &c.to.selection_range); p["name"] = json!(c.to.name); p["kind"] = json!(kind_name(c.to.kind)); p }).collect::<Vec<_>>() })
                }
                Hierarchy::Supertypes | Hierarchy::Subtypes => {
                    let types: Vec<lsp_types::TypeHierarchyItem> = serde_json::from_value::<Option<Vec<_>>>(value).ok().flatten().unwrap_or_default();
                    json!({ "types": types.iter().take(MAX_ITEMS).map(|t| { let mut p = place(&t.uri, &t.selection_range); p["name"] = json!(t.name); p["kind"] = json!(kind_name(t.kind)); p }).collect::<Vec<_>>() })
                }
            }),
            Step::Signature => {
                let help = serde_json::from_value::<Option<lsp_types::SignatureHelp>>(value).ok().flatten();
                Ok(match help {
                    Some(help) => json!({
                        "signatures": help.signatures.iter().map(|s| json!({ "label": s.label, "documentation": s.documentation.as_ref().map(|d| match d { lsp_types::Documentation::String(s) => s.clone(), lsp_types::Documentation::MarkupContent(m) => m.value.clone() }) })).collect::<Vec<_>>(),
                        "active": help.active_signature,
                        "parameter": help.active_parameter,
                    }),
                    None => json!({ "signatures": [] }),
                })
            }
            Step::CodeActions => {
                let actions: Vec<lsp_types::CodeActionOrCommand> = serde_json::from_value::<Option<Vec<_>>>(value).ok().flatten().unwrap_or_default();
                // A new list replaces the last: ids are for this one.
                self.agent_lsp.actions.clear();
                let mut listed = Vec::new();
                for action in actions.into_iter().take(MAX_ITEMS) {
                    self.agent_lsp.next_action += 1;
                    let id = format!("a{}", self.agent_lsp.next_action);
                    let (title, kind, preferred, applies) = match &action {
                        lsp_types::CodeActionOrCommand::CodeAction(a) => (a.title.clone(), a.kind.as_ref().map(|k| k.as_str().to_string()), a.is_preferred.unwrap_or(false), a.edit.is_some() || a.data.is_some()),
                        lsp_types::CodeActionOrCommand::Command(c) => (c.title.clone(), None, false, false),
                    };
                    listed.push(json!({ "id": id, "title": title, "kind": kind, "preferred": preferred, "applies": applies }));
                    self.agent_lsp.actions.insert(id, (key.clone(), action));
                }
                Ok(json!({ "actions": listed }))
            }
            Step::Resolve => match serde_json::from_value::<lsp_types::CodeAction>(value) {
                Ok(action) => match action.edit {
                    Some(edit) => self.agent_apply_workspace_edit(&caller, edit),
                    None => Err(invalid("the language server gave this action no edit")),
                },
                Err(e) => Err(invalid(format!("the language server's action didn't read: {e}"))),
            },
            Step::Rename => match serde_json::from_value::<Option<lsp_types::WorkspaceEdit>>(value) {
                Ok(Some(edit)) => self.agent_apply_workspace_edit(&caller, edit),
                Ok(None) => Err(invalid("the language server can't rename that")),
                Err(e) => Err(invalid(format!("the language server's rename didn't read: {e}"))),
            },
            Step::Format { path, before } => {
                let edits: Vec<lsp_types::TextEdit> = serde_json::from_value::<Option<Vec<_>>>(value).ok().flatten().unwrap_or_default();
                match fenix_lsp::workspace_edit::apply_text_edits(&before, &edits) {
                    Ok(after) if after == before => Ok(json!({ "changed": [], "note": "already formatted" })),
                    Ok(after) => {
                        // Formatted against the text it was given: a buffer
                        // changed meanwhile isn't overwritten.
                        if self.agent_text_of(&path).ok().as_deref() != Some(before.as_str()) {
                            Err(invalid(format!("{} changed while it was being formatted: ask again", path.display())))
                        } else {
                            self.agent_edit_file(&caller, &path, &after).map(|_| changed(&[path])).map_err(invalid)
                        }
                    }
                    Err(e) => Err(invalid(format!("the formatter's edits didn't apply: {e}"))),
                }
            }
        };
        let _ = reply.send(answer);
    }

    /// The soonest a request runs out of time.
    pub(super) fn agent_lsp_next(&self) -> Option<Instant> {
        self.agent_lsp.waiting.values().map(|w| w.until).min()
    }

    /// Requests past their time are answered.
    pub(super) fn agent_lsp_tick(&mut self, now: Instant) {
        let late: Vec<u64> = self.agent_lsp.waiting.iter().filter(|(_, w)| now >= w.until).map(|(id, _)| *id).collect();
        for id in late {
            if let Some(w) = self.agent_lsp.waiting.remove(&id) {
                let _ = w.reply.send(Err(invalid(format!("{} didn't answer in time", self.lsp_server_name(&w.key)))));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A request as if it had been sent: its answer is fed in by hand.
    fn waiting(app: &mut App, step: Step, caller: &AgentCaller) -> (u64, std::sync::mpsc::Receiver<Reply>) {
        let (reply, answer) = std::sync::mpsc::channel();
        app.agent_lsp.next += 1;
        let id = app.agent_lsp.next;
        let key = LspKey::primary(fenix_syntax::LanguageId::Rust, std::env::temp_dir());
        app.agent_lsp.waiting.insert(id, Waiting { key, step, reply, caller: caller.clone(), until: Instant::now() + AGENT_LSP_TIMEOUT });
        (id, answer)
    }

    fn project() -> (tempfile::TempDir, PathBuf, App, AgentCaller) {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.rs");
        std::fs::write(&file, "fn old() {}\nfn main() { old(); }\n").unwrap();
        let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
        let thread = app.agents.hub.new_thread("claude", dir.path());
        let caller = AgentCaller { client: "fenix-mcp".into(), thread: Some(thread.0) };
        (dir, file, app, caller)
    }

    fn edit_of(file: &Path, edits: Value) -> Value {
        let uri = fenix_lsp::path_to_uri(&fenix_lsp::normalize(std::fs::canonicalize(file).unwrap())).unwrap();
        json!({ "changes": { uri.as_str(): edits } })
    }

    fn range(line: u32, start: u32, end: u32) -> Value {
        json!({ "start": { "line": line, "character": start }, "end": { "line": line, "character": end } })
    }

    #[test]
    fn a_rename_goes_into_the_threads_review() {
        let (_dir, file, mut app, caller) = project();
        let (id, answer) = waiting(&mut app, Step::Rename, &caller);
        let edit = edit_of(&file, json!([{ "range": range(0, 3, 6), "newText": "new" }, { "range": range(1, 12, 15), "newText": "new" }]));
        app.agent_lsp_answered(id, Ok(edit));
        let done = answer.try_recv().unwrap().unwrap();
        assert_eq!(done["changed"].as_array().unwrap().len(), 1, "{done}");
        assert_eq!(app.agent_text_of(&file).unwrap(), "fn new() {}\nfn main() { new(); }\n");
        let thread = fenix_acp::hub::ThreadId(caller.thread.clone().unwrap());
        assert_eq!(app.agents.baselines[&thread].values().next().unwrap().as_deref(), Some("fn old() {}\nfn main() { old(); }\n"));
    }

    #[test]
    fn code_actions_get_ids_and_one_with_an_edit_applies() {
        let (_dir, file, mut app, caller) = project();
        let (id, answer) = waiting(&mut app, Step::CodeActions, &caller);
        let edit = edit_of(&file, json!([{ "range": range(0, 3, 6), "newText": "renamed" }]));
        app.agent_lsp_answered(id, Ok(json!([{ "title": "Rename it", "kind": "refactor", "edit": edit }, { "title": "Run a command", "command": "x" }])));
        let listed = answer.try_recv().unwrap().unwrap();
        let actions = listed["actions"].as_array().unwrap();
        assert_eq!(actions.len(), 2);
        assert_eq!((actions[0]["applies"].as_bool(), actions[1]["applies"].as_bool()), (Some(true), Some(false)));
        let (reply, answer) = std::sync::mpsc::channel();
        let first = actions[0]["id"].as_str().unwrap().to_string();
        app.agent_lsp_call(&caller, "apply_code_action", json!({ "id": first }), reply).unwrap();
        assert!(answer.try_recv().unwrap().is_ok());
        assert!(app.agent_text_of(&file).unwrap().starts_with("fn renamed()"));
        let (reply, answer) = std::sync::mpsc::channel();
        app.agent_lsp_call(&caller, "apply_code_action", json!({ "id": actions[1]["id"] }), reply).unwrap();
        assert!(answer.try_recv().unwrap().unwrap_err().message.contains("command"));
    }

    #[test]
    fn formatting_applies_against_the_text_it_was_given() {
        let (_dir, file, mut app, caller) = project();
        let before = app.agent_text_of(&file).unwrap();
        let (id, answer) = waiting(&mut app, Step::Format { path: file.clone(), before: before.clone() }, &caller);
        app.agent_lsp_answered(id, Ok(json!([{ "range": range(0, 0, 0), "newText": "// formatted\n" }])));
        assert!(answer.try_recv().unwrap().is_ok());
        assert!(app.agent_text_of(&file).unwrap().starts_with("// formatted\n"));
        let (id, answer) = waiting(&mut app, Step::Format { path: file.clone(), before }, &caller);
        app.agent_lsp_answered(id, Ok(json!([{ "range": range(0, 0, 0), "newText": "x" }])));
        assert!(answer.try_recv().unwrap().unwrap_err().message.contains("changed while"), "the buffer moved on: nothing overwritten");
    }

    #[test]
    fn a_servers_error_and_silence_are_said_plainly() {
        let (_dir, _file, mut app, caller) = project();
        let (id, answer) = waiting(&mut app, Step::Implementations, &caller);
        app.agent_lsp_answered(id, Err("method not found".into()));
        assert!(answer.try_recv().unwrap().unwrap_err().message.contains("method not found"));
        let (_, answer) = waiting(&mut app, Step::Signature, &caller);
        app.agent_lsp_tick(Instant::now() + AGENT_LSP_TIMEOUT + Duration::from_secs(1));
        assert!(answer.try_recv().unwrap().unwrap_err().message.contains("in time"));
        let (reply, answer) = std::sync::mpsc::channel();
        app.agent_lsp_call(&caller, "rename", json!({ "path": "/nowhere/x.rs", "line": 1, "new_name": "y" }), reply).unwrap();
        assert!(answer.try_recv().unwrap().is_err());
    }

    #[test]
    fn locations_read_every_shape() {
        let uri = "file:///p/a.rs";
        let range = json!({ "start": { "line": 2, "character": 4 }, "end": { "line": 2, "character": 9 } });
        assert_eq!(locations(json!({ "uri": uri, "range": range }))[0]["line"], 3);
        assert_eq!(locations(json!([{ "uri": uri, "range": range }, { "uri": uri, "range": range }])).len(), 2);
        assert_eq!(locations(json!([{ "targetUri": uri, "targetRange": range, "targetSelectionRange": range }]))[0]["column"], 5);
        assert!(locations(Value::Null).is_empty());
    }

    #[test]
    fn a_change_says_where_it_goes() {
        assert_eq!(changed(&[PathBuf::from("/p/a.rs")])["changed"], json!(["/p/a.rs"]));
    }
}
