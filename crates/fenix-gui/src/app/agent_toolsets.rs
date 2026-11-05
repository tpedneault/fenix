//! The work of Fenix's toolsets beyond `core`: Git, search, the syntax
//! tree, tasks. `agent_calls` has already decided a call may run; these
//! do it. Edits go into the caller's thread's change ledger, exactly as
//! an agent's own writes through Fenix do (`agents_write`), so Agent
//! changes (`SPC a r`) reviews them.

use std::collections::HashMap;
use std::sync::mpsc::Sender;

use fenix_acp::hub::ThreadId;
use fenix_control::{AgentCaller, RpcError};
use serde_json::{json, Value};

use super::*;
use crate::agent_work;
use crate::ipc::Reply;

/// How long `run_task` waits for its task, unless told otherwise.
const TASK_WAIT: Duration = Duration::from_secs(10 * 60);

/// The lines of a task's output `run_task` answers with.
const TASK_TAIL: usize = 200;

/// An agent waiting for a task to end.
pub(super) struct TaskWait {
    pub reply: Sender<Reply>,
    pub until: Instant,
    pub name: String,
}

fn invalid(message: impl Into<String>) -> RpcError {
    RpcError::invalid_params(message)
}

impl App {
    /// The most a tool answers, in bytes (`agents.mcp.max_answer_kib`).
    pub(super) fn agent_answer_max(&self) -> usize {
        self.config.int_value("agents.mcp.max_answer_kib").unwrap_or(64).clamp(4, 1024) as usize * 1024
    }

    /// The open files with unsaved edits, as their text stands.
    pub(super) fn agent_unsaved(&self) -> HashMap<PathBuf, String> {
        self.buffers
            .ids_sorted_by_path()
            .into_iter()
            .filter_map(|b| self.buffers.get(b))
            .filter(|ob| ob.kind == BufferKind::Text && ob.buffer.is_dirty())
            .filter_map(|ob| ob.buffer.path().map(|p| (p.to_path_buf(), ob.buffer.text())))
            .collect()
    }

    /// A file's text as Fenix has it: the buffer's when it's open.
    pub(super) fn agent_text_of(&self, path: &Path) -> Result<String, String> {
        match self.buffers.id_for_path(path).and_then(|id| self.buffers.get(id)) {
            Some(ob) => Ok(ob.buffer.text()),
            None => std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display())),
        }
    }

    /// Writes `content` to `path` for the caller's thread, through its
    /// change ledger: the text before is kept so the user can reject the
    /// change hunk by hunk. A buffer with the user's unsaved edits gets
    /// the change unsaved, so nothing of theirs is saved for them.
    pub(super) fn agent_edit_file(&mut self, caller: &AgentCaller, path: &Path, content: &str) -> Result<(), String> {
        let Some(thread) = caller.thread.clone().map(ThreadId).filter(|t| self.agents.hub.view(t).is_some()) else {
            return Err("an edit goes to an agent thread's review, and this call has no thread".into());
        };
        let dirty = self.buffers.id_for_path(path).and_then(|id| self.buffers.get(id)).is_some_and(|ob| ob.buffer.is_dirty());
        if !dirty {
            return self.agents_write(&thread, path, content).map(|_| ()).map_err(|e| e.message);
        }
        let id = self.buffers.id_for_path(path).unwrap();
        let before = self.buffers.get(id).map(|ob| ob.buffer.text());
        self.agents.baselines.entry(thread.clone()).or_default().entry(path.to_path_buf()).or_insert(before);
        self.replace_buffer_from_disk(id, content);
        if let Some(ob) = self.buffers.get_mut(id) {
            ob.buffer.mark_unsaved();
        }
        self.refresh_gutter_hunks(id);
        self.agents_refresh_marks();
        Ok(())
    }

    /// Runs `work` off the UI thread and answers `reply` with it.
    fn agent_off_thread(reply: Sender<Reply>, work: impl FnOnce() -> Result<Value, String> + Send + 'static) {
        std::thread::spawn(move || {
            let _ = reply.send(work().map_err(invalid));
        });
    }

    /// A path argument, absolute: relative ones are the project's.
    fn agent_path(&self, arguments: &Value) -> Result<PathBuf, RpcError> {
        let path = arguments.get("path").and_then(Value::as_str).filter(|p| !p.is_empty()).ok_or_else(|| invalid("path: which file"))?;
        let path = PathBuf::from(path);
        Ok(if path.is_absolute() { path } else { self.integration_root().join(path) })
    }

    /// Fenix's toolsets beyond `core`: answers `reply` and gives `true`,
    /// or gives the reply back for a tool this doesn't know.
    pub(super) fn agent_toolset_call(&mut self, caller: &AgentCaller, name: &str, arguments: Value, reply: Sender<Reply>) -> Result<(), Sender<Reply>> {
        let root = self.integration_root();
        let max = self.agent_answer_max();
        let answer = |reply: Sender<Reply>, answer: Result<Value, RpcError>| {
            let _ = reply.send(answer);
        };
        match name {
            // -- git --
            "git_status" | "git_diff" | "git_log" | "git_show" => {
                let repo = match agent_work::repo_of(&root) {
                    Ok(repo) => repo,
                    Err(why) => {
                        answer(reply, Err(invalid(why)));
                        return Ok(());
                    }
                };
                let name = name.to_string();
                Self::agent_off_thread(reply, move || match name.as_str() {
                    "git_status" => Ok(agent_work::git_status(&repo)),
                    "git_diff" => agent_work::git_diff(&repo, &arguments, max),
                    "git_log" => agent_work::git_log(&repo, &arguments),
                    _ => agent_work::git_show(&repo, &arguments, max),
                });
            }
            "git_blame" => {
                let result = (|| {
                    let path = self.agent_path(&arguments)?;
                    let repo = agent_work::repo_of(&root).map_err(invalid)?;
                    let contents = self.buffers.id_for_path(&path).and_then(|id| self.buffers.get(id)).filter(|ob| ob.buffer.is_dirty()).map(|ob| ob.buffer.text());
                    let mut arguments = arguments.clone();
                    arguments["path"] = json!(path.to_string_lossy());
                    Ok::<_, RpcError>((repo, arguments, contents))
                })();
                match result {
                    Ok((repo, arguments, contents)) => Self::agent_off_thread(reply, move || agent_work::git_blame(&repo, &arguments, contents.as_deref())),
                    Err(e) => answer(reply, Err(e)),
                }
            }
            "git_stage" => {
                let result = (|| {
                    let path = self.agent_path(&arguments)?;
                    let repo = agent_work::repo_of(&root).map_err(invalid)?;
                    let rel = agent_work::repo_relative(&repo, &path.to_string_lossy());
                    fenix_git::stage_file(&repo, &rel).map_err(invalid)?;
                    Ok(json!({ "staged": rel }))
                })();
                answer(reply, result);
            }
            // -- search --
            "search" => {
                let unsaved = self.agent_unsaved();
                Self::agent_off_thread(reply, move || agent_work::search(&root, &unsaved, &arguments));
            }
            "replace" => {
                let unsaved = self.agent_unsaved();
                let result = agent_work::replacements(&root, &unsaved, &arguments).map_err(invalid).and_then(|changes| {
                    let mut files = Vec::new();
                    for (path, text, count) in changes {
                        self.agent_edit_file(caller, &path, &text).map_err(invalid)?;
                        files.push(json!({ "path": path.to_string_lossy(), "replaced": count }));
                    }
                    Ok(json!({ "files": files, "note": "the user reviews these in Agent changes" }))
                });
                answer(reply, result);
            }
            // -- syntax --
            "syntax_node" => answer(reply, self.agent_syntax_node(&arguments)),
            "syntax_query" => answer(reply, self.agent_syntax_query(&arguments)),
            // -- tasks --
            "run_task" => self.agent_run_task(arguments, reply),
            "run_test" => {
                let started = (|| {
                    let path = self.agent_path(&arguments)?;
                    let line = arguments.get("line").and_then(Value::as_u64).filter(|l| *l > 0).ok_or_else(|| invalid("line: which line, from 1"))? as usize;
                    self.run_test_at(&path, line).map_err(invalid)
                })();
                match started {
                    Ok(name) => {
                        let secs = arguments.get("timeout").and_then(Value::as_u64).unwrap_or(TASK_WAIT.as_secs()).clamp(1, 3600);
                        self.agent_calls.task_waits.push(TaskWait { reply, until: Instant::now() + Duration::from_secs(secs), name });
                    }
                    Err(e) => answer(reply, Err(e)),
                }
            }
            "task_output" => answer(reply, self.agent_task_output(&arguments, max)),
            other => return self.agent_lsp_call(caller, other, arguments, reply),
        }
        Ok(())
    }

    fn agent_syntax_state(&self, path: &Path) -> Result<(fenix_syntax::SyntaxState, String), RpcError> {
        let text = self.agent_text_of(path).map_err(invalid)?;
        let language = self.buffers.id_for_path(path).and_then(|id| self.buffers.get(id)).and_then(|ob| ob.syntax.as_ref().map(|s| s.language())).or_else(|| fenix_syntax::detect_language_from_path(path)).ok_or_else(|| invalid(format!("no grammar for {}", path.display())))?;
        Ok((fenix_syntax::SyntaxState::new(language, &text), text))
    }

    fn node_json(node: &fenix_syntax::NodeInfo) -> Value {
        json!({ "kind": node.kind, "line": node.start.0 + 1, "column": node.start.1 + 1, "endLine": node.end.0 + 1, "field": node.field, "text": node.first_line })
    }

    /// `syntax_node`: the node at a place and its parents.
    fn agent_syntax_node(&self, arguments: &Value) -> Result<Value, RpcError> {
        let path = self.agent_path(arguments)?;
        let (state, text) = self.agent_syntax_state(&path)?;
        let line = arguments.get("line").and_then(Value::as_u64).unwrap_or(1).max(1) as usize - 1;
        let column = arguments.get("column").and_then(Value::as_u64).unwrap_or(1).max(1) as usize - 1;
        let line_start: usize = text.split_inclusive('\n').take(line).map(str::len).sum();
        let line_text = text[line_start.min(text.len())..].lines().next().unwrap_or("");
        // Columns count characters; the tree counts bytes.
        let byte = line_start + line_text.char_indices().nth(column).map(|(i, _)| i).unwrap_or(line_text.len());
        let chain = state.node_chain(&text, byte);
        if chain.is_empty() {
            return Err(invalid(format!("{} has no syntax tree", path.display())));
        }
        Ok(json!({ "nodes": chain.iter().map(Self::node_json).collect::<Vec<_>>() }))
    }

    /// `syntax_query`: a tree-sitter query's captures in a file.
    fn agent_syntax_query(&self, arguments: &Value) -> Result<Value, RpcError> {
        let path = self.agent_path(arguments)?;
        let query = arguments.get("query").and_then(Value::as_str).ok_or_else(|| invalid("query: a tree-sitter query, with @captures"))?;
        let max = arguments.get("max").and_then(Value::as_u64).unwrap_or(100).clamp(1, 1000) as usize;
        let (state, text) = self.agent_syntax_state(&path)?;
        let found = state.query(&text, query, max).map_err(invalid)?;
        Ok(json!({ "captures": found.iter().map(|(capture, node)| { let mut n = Self::node_json(node); n["capture"] = json!(capture); n }).collect::<Vec<_>>(), "cut": found.len() >= max }))
    }

    /// `run_task`: starts it; with `wait` (the default) answers when it
    /// ends, with how it went.
    fn agent_run_task(&mut self, arguments: Value, reply: Sender<Reply>) {
        let wait = arguments.get("wait").and_then(Value::as_bool).unwrap_or(true);
        let name = arguments.get("name").and_then(Value::as_str).unwrap_or_default().to_string();
        let started = self.apply_control(fenix_control::Call::TasksRun(json!({ "name": name })));
        match started {
            Ok(value) if wait => {
                let secs = arguments.get("timeout").and_then(Value::as_u64).unwrap_or(TASK_WAIT.as_secs()).clamp(1, 3600);
                // A task already waited on is answered when it's replaced.
                self.agent_task_ended();
                let _ = value;
                self.agent_calls.task_waits.push(TaskWait { reply, until: Instant::now() + Duration::from_secs(secs), name });
            }
            other => {
                let _ = reply.send(other);
            }
        }
    }

    /// How the last task went, for agents: its exit, the problems found
    /// in its output, its tests, and the end of its output.
    fn agent_task_report(&self) -> Value {
        let Some(session) = &self.task_session else { return json!({ "error": "no task has run" }) };
        let output = self.buffers.get(session.buffer).map(|ob| ob.buffer.text()).unwrap_or_default();
        let lines: Vec<&str> = output.lines().collect();
        let succeeded = lines.iter().rev().find_map(|l| match *l {
            "-- task succeeded --" => Some(json!(true)),
            "-- task failed --" => Some(json!(false)),
            l if l.starts_with("-- task cancelled") || l.starts_with("-- task error") => Some(Value::Null),
            _ => None,
        });
        let problems: Vec<Value> = self
            .quickfix
            .iter()
            .filter_map(|q| match q {
                QuickfixEntry::Task(m) => Some(json!({ "path": m.path.to_string_lossy(), "line": m.line, "column": m.col, "message": m.text })),
                _ => None,
            })
            .take(100)
            .collect();
        let tests = fenix_tasks::test_counts(&output).map(|(passed, failed)| json!({ "passed": passed, "failed": failed }));
        let tail = lines[lines.len().saturating_sub(TASK_TAIL)..].join("\n");
        json!({ "running": session.runner.is_some(), "succeeded": succeeded, "problems": problems, "tests": tests, "lines": lines.len(), "output": agent_work::cap(&tail, self.agent_answer_max(), "task_output reads the rest") })
    }

    /// The task ended (or another took its place): agents waiting hear.
    pub(super) fn agent_task_ended(&mut self) {
        if self.agent_calls.task_waits.is_empty() {
            return;
        }
        let report = self.agent_task_report();
        for wait in std::mem::take(&mut self.agent_calls.task_waits) {
            let mut report = report.clone();
            report["task"] = json!(wait.name);
            let _ = wait.reply.send(Ok(report));
        }
    }

    /// Waits past their time are answered: still running.
    pub(super) fn agent_task_waits_tick(&mut self, now: Instant) {
        if !self.agent_calls.task_waits.iter().any(|w| now >= w.until) {
            return;
        }
        let report = self.agent_task_report();
        self.agent_calls.task_waits.retain(|w| {
            if now >= w.until {
                let mut report = report.clone();
                report["task"] = json!(w.name);
                report["note"] = json!("still running: task_output follows it");
                let _ = w.reply.send(Ok(report));
                false
            } else {
                true
            }
        });
    }

    /// `task_output`: the last task's output, or part of it.
    fn agent_task_output(&self, arguments: &Value, max: usize) -> Result<Value, RpcError> {
        let session = self.task_session.as_ref().ok_or_else(|| invalid("no task has run yet"))?;
        let output = self.buffers.get(session.buffer).map(|ob| ob.buffer.text()).unwrap_or_default();
        let lines: Vec<&str> = output.lines().collect();
        let start = arguments.get("startLine").and_then(Value::as_u64).unwrap_or(1).max(1) as usize;
        let end = arguments.get("endLine").and_then(Value::as_u64).map(|e| e as usize).unwrap_or(lines.len()).min(lines.len());
        let text = if start > end { String::new() } else { lines[start - 1..end].join("\n") };
        Ok(json!({ "lines": lines.len(), "running": session.runner.is_some(), "output": agent_work::cap(&text, max, "ask for a smaller startLine..endLine") }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(app: &mut App, thread: Option<&str>, tool: &str, arguments: Value) -> Reply {
        let (reply, answer) = std::sync::mpsc::channel();
        let caller = AgentCaller { client: "fenix-mcp".into(), thread: thread.map(str::to_string) };
        app.control_agent("call", caller, json!({ "call": "1", "tool": tool, "arguments": arguments }), reply);
        answer.recv_timeout(Duration::from_secs(20)).expect("an answer").map(|a| a["answer"].clone())
    }

    fn project() -> (tempfile::TempDir, App) {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("main.rs");
        std::fs::write(&file, "fn old_name() {}\nfn main() { old_name(); }\n").unwrap();
        let app = App::with_file(Some(file.to_string_lossy().into_owned()));
        (dir, app)
    }

    #[test]
    fn search_and_syntax_answer_from_the_project() {
        let (dir, mut app) = project();
        let found = call(&mut app, None, "search", json!({ "pattern": "old_name" })).unwrap();
        assert_eq!(found["matches"].as_array().unwrap().len(), 2, "{found}");
        let path = dir.path().join("main.rs").to_string_lossy().into_owned();
        let node = call(&mut app, None, "syntax_node", json!({ "path": path, "line": 2, "column": 14 })).unwrap();
        let kinds: Vec<&str> = node["nodes"].as_array().unwrap().iter().map(|n| n["kind"].as_str().unwrap()).collect();
        assert!(kinds.contains(&"call_expression") && kinds.contains(&"function_item"), "{kinds:?}");
        let names = call(&mut app, None, "syntax_query", json!({ "path": path, "query": "(function_item name: (identifier) @name)" })).unwrap();
        assert_eq!(names["captures"].as_array().unwrap().len(), 2);
        assert!(call(&mut app, None, "git_status", json!({})).unwrap_err().message.contains("Git repository"));
    }

    #[test]
    fn a_replace_goes_to_the_threads_review_and_needs_a_thread() {
        let (dir, mut app) = project();
        app.config.set_value("agents.mcp.allow", Some(fenix_config::Value::Map(vec![("edit".into(), "allow".into())])));
        let refused = call(&mut app, None, "replace", json!({ "pattern": "old_name", "replacement": "new_name" })).unwrap_err();
        assert!(refused.message.contains("refused"), "a client Fenix didn't start can't edit: {}", refused.message);
        let thread = app.agents.hub.new_thread("claude", dir.path());
        let done = call(&mut app, Some(&thread.0), "replace", json!({ "pattern": "old_name", "replacement": "new_name" })).unwrap();
        assert_eq!(done["files"][0]["replaced"], 2, "{done}");
        let file = dir.path().join("main.rs");
        assert_eq!(app.agent_text_of(&file).unwrap(), "fn new_name() {}\nfn main() { new_name(); }\n");
        let baselines = &app.agents.baselines[&thread];
        assert_eq!(baselines.values().next().unwrap().as_deref(), Some("fn old_name() {}\nfn main() { old_name(); }\n"), "the text before is kept for review");
    }

    #[test]
    fn task_output_says_when_no_task_ran() {
        let (_dir, mut app) = project();
        assert!(call(&mut app, None, "task_output", json!({})).unwrap_err().message.contains("no task"));
    }
}
