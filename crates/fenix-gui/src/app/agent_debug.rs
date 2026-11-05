//! The `debug` toolset: an agent drives the debugger as you would --
//! launches, breakpoints, starting, stepping, running to a line. A call
//! that moves the program answers once it next stops (or ends, or a
//! while passes), with the same state `debug_state` gives, so an agent's
//! loop is one call per step.

use std::sync::mpsc::Sender;

use fenix_control::RpcError;
use serde_json::{json, Value};

use super::*;
use crate::ipc::Reply;

/// How long a step waits for the program to stop again.
const STEP_WAIT: Duration = Duration::from_secs(10);

/// How long a start waits for its first stop.
const START_WAIT: Duration = Duration::from_secs(30);

/// After a stop, how long the variables get to arrive.
const SETTLE: Duration = Duration::from_millis(500);

/// An agent waiting for the program to stop.
pub(super) struct DebugWait {
    reply: Sender<Reply>,
    until: Instant,
    /// `debug_stops` when it began: a later stop is the one it waits for.
    stops_at: u64,
}

fn invalid(message: impl Into<String>) -> RpcError {
    RpcError::invalid_params(message)
}

impl App {
    /// A stop happened (`Event::Stopped`): for the waits.
    pub(super) fn agent_debug_stopped(&mut self) {
        self.agent_calls.debug_stops += 1;
        self.agent_calls.debug_stopped_at = Some(Instant::now());
    }

    fn agent_debug_wait(&mut self, reply: Sender<Reply>, wait: Duration) {
        let stops_at = self.agent_calls.debug_stops;
        self.agent_calls.debug_waits.push(DebugWait {
            reply,
            until: Instant::now() + wait,
            stops_at,
        });
    }

    /// Waits whose stop has come (its stack read), whose session ended, or
    /// whose time is up, are answered.
    pub(super) fn agent_debug_tick(&mut self, now: Instant) {
        if self.agent_calls.debug_waits.is_empty() {
            return;
        }
        let ended = self.debug_session.as_ref().is_none_or(|s| s.ended);
        let settled = self.debug_session.as_ref().is_some_and(|s| {
            s.stopped_thread.is_some()
                && !s.call_stack.is_empty()
                && (!s.variables.is_empty()
                    || self
                        .agent_calls
                        .debug_stopped_at
                        .is_some_and(|at| now.duration_since(at) >= SETTLE))
        });
        let stops = self.agent_calls.debug_stops;
        let waits = std::mem::take(&mut self.agent_calls.debug_waits);
        for wait in waits {
            let stopped = stops > wait.stops_at && settled;
            if stopped || ended || now >= wait.until {
                let mut state = self.control_debug_state();
                if !stopped && !ended {
                    state["note"] =
                        json!("still running: debug_pause stops it, debug_state looks again");
                }
                let _ = wait.reply.send(Ok(state));
            } else {
                self.agent_calls.debug_waits.push(wait);
            }
        }
    }

    /// The soonest a wait needs looking at.
    pub(super) fn agent_debug_next(&self) -> Option<Instant> {
        if self.agent_calls.debug_waits.is_empty() {
            None
        } else {
            // Polled while it waits: a stop's stack arrives on its own time.
            Some(Instant::now() + Duration::from_millis(100))
        }
    }

    fn agent_breakpoint_path(&self, arguments: &Value) -> Result<(PathBuf, usize), RpcError> {
        let path = self.control_path(arguments)?;
        let line = arguments
            .get("line")
            .and_then(Value::as_u64)
            .filter(|l| *l > 0)
            .ok_or_else(|| invalid("line: which line, from 1"))? as usize;
        Ok((
            fenix_lsp::normalize(std::fs::canonicalize(&path).unwrap_or(path)),
            line,
        ))
    }

    /// The launches there are for the language of `path` (or the file in
    /// front): the extensions' templates and the project's own.
    fn agent_launches(
        &self,
        arguments: &Value,
    ) -> Result<(Vec<crate::dap::Template>, bool), RpcError> {
        let language = match arguments.get("path").and_then(Value::as_str) {
            Some(path) => fenix_syntax::detect_language_from_path(Path::new(path)),
            None => self.focused_language(),
        }
        .ok_or_else(|| invalid("no language to debug: give path, a file of the program"))?;
        let root = self.integration_root();
        let name = crate::lsp::language_config_name(language);
        let (mut templates, _) =
            crate::dap::templates_for(&crate::installed::data_dirs("launch"), &name);
        templates.extend(
            self.wasm
                .launches(&root)
                .into_iter()
                .filter(|t| t.languages.contains(&name)),
        );
        let project =
            fenix_project::tools::ProjectTools::read(&root).is_ok_and(|t| !t.launch.is_empty());
        Ok((templates, project))
    }

    /// The `debug` toolset: answers `reply`, or gives it back for a tool
    /// that isn't one of its.
    pub(super) fn agent_debug_call(
        &mut self,
        name: &str,
        arguments: Value,
        reply: Sender<Reply>,
    ) -> Result<(), Sender<Reply>> {
        let stopped = self
            .debug_session
            .as_ref()
            .is_some_and(|s| !s.ended && s.stopped_thread.is_some());
        let running = self.debug_session.as_ref().is_some_and(|s| !s.ended);
        let answer: Option<Result<Value, RpcError>> = match name {
            "debug_launches" => Some(self.agent_launches(&arguments).map(|(templates, project)| {
                let mut names: Vec<Value> = templates.iter().map(|t| json!({ "name": t.name })).collect();
                if project {
                    names.insert(0, json!({ "name": "project", "note": "the project's own launch (.fenix/tools.json)" }));
                }
                json!({ "launches": names })
            })),
            "debug_start" => {
                if running {
                    Some(Err(invalid("a debug session is running: debug_stop ends it first")))
                } else {
                    // The program's file in front: launches are by its language.
                    if let Some(path) = arguments.get("path").and_then(Value::as_str) {
                        self.open_positions(&[fenix_tasks::PathPosition { path: PathBuf::from(path), line: None, column: None }], true);
                    }
                    let launch = arguments.get("launch").and_then(Value::as_str).filter(|l| !l.is_empty() && *l != "project");
                    match launch {
                        Some(launch) => match self.agent_launches(&arguments) {
                            Ok((templates, _)) => match templates.into_iter().find(|t| t.name == launch) {
                                Some(template) => {
                                    self.debug_start_from(template);
                                    None
                                }
                                None => Some(Err(invalid(format!("no launch {launch}: debug_launches lists them")))),
                            },
                            Err(e) => Some(Err(e)),
                        },
                        None => {
                            self.debug_start_or_continue();
                            None
                        }
                    }
                }
                .or_else(|| {
                    self.agent_debug_wait(reply.clone(), START_WAIT);
                    Some(Ok(Value::Null))
                })
                .filter(|a| !matches!(a, Ok(Value::Null)))
            }
            "breakpoints" => {
                let list: Vec<Value> = self
                    .breakpoints
                    .iter()
                    .flat_map(|(path, lines)| lines.iter().map(move |(line, b)| json!({ "path": path.to_string_lossy(), "line": line, "condition": b.condition, "hits": b.hits, "log": b.log, "off": b.off })))
                    .collect();
                Some(Ok(json!({ "breakpoints": list })))
            }
            "set_breakpoint" => Some(self.agent_breakpoint_path(&arguments).map(|(path, line)| {
                let breakpoint = self.breakpoints.entry(path.clone()).or_default().entry(line).or_default();
                breakpoint.off = false;
                breakpoint.condition = arguments.get("condition").and_then(Value::as_str).map(str::to_string).filter(|c| !c.is_empty());
                breakpoint.log = arguments.get("log").and_then(Value::as_str).map(str::to_string).filter(|c| !c.is_empty());
                self.agent_calls.agent_breakpoints.insert((path.clone(), line));
                self.send_set_breakpoints_for(&path);
                self.refresh_debug_panes();
                json!({ "set": { "path": path.to_string_lossy(), "line": line } })
            })),
            "clear_breakpoint" => Some(self.agent_breakpoint_path(&arguments).and_then(|(path, line)| {
                let removed = self.breakpoints.get_mut(&path).and_then(|lines| lines.remove(&line)).is_some();
                self.agent_calls.agent_breakpoints.remove(&(path.clone(), line));
                if !removed {
                    return Err(invalid("no breakpoint there"));
                }
                self.send_set_breakpoints_for(&path);
                self.refresh_debug_panes();
                Ok(json!({ "cleared": { "path": path.to_string_lossy(), "line": line } }))
            })),
            "debug_continue" | "debug_step" if !stopped => Some(Err(invalid(if running { "the program is running, not stopped: debug_pause stops it" } else { "no debug session: debug_start starts one" }))),
            "debug_continue" => {
                self.debug_start_or_continue();
                self.agent_debug_wait(reply.clone(), STEP_WAIT);
                None
            }
            "debug_step" => {
                match arguments.get("kind").and_then(Value::as_str).unwrap_or("over") {
                    "into" => self.debug_step_into(),
                    "out" => self.debug_step_out(),
                    _ => self.debug_step_over(),
                }
                self.agent_debug_wait(reply.clone(), STEP_WAIT);
                None
            }
            "debug_run_to" => {
                if !stopped {
                    Some(Err(invalid("the program isn't stopped: run to a line from a stop")))
                } else {
                    match self.agent_breakpoint_path(&arguments) {
                        Ok((path, line)) => {
                            if let Some((old, _)) = self.debug_run_to.replace((path.clone(), line)) {
                                if old != path {
                                    self.send_set_breakpoints_for(&old);
                                }
                            }
                            self.send_set_breakpoints_for(&path);
                            self.debug_start_or_continue();
                            self.agent_debug_wait(reply.clone(), STEP_WAIT);
                            None
                        }
                        Err(e) => Some(Err(e)),
                    }
                }
            }
            "debug_pause" => {
                if !running || stopped {
                    Some(Err(invalid(if stopped { "it's stopped already" } else { "no debug session running" })))
                } else {
                    self.debug_pause();
                    self.agent_debug_wait(reply.clone(), STEP_WAIT);
                    None
                }
            }
            "debug_stop" => {
                if !running {
                    Some(Err(invalid("no debug session running")))
                } else {
                    self.debug_stop();
                    Some(Ok(json!({ "stopped": true })))
                }
            }
            _ => return Err(reply),
        };
        if let Some(answer) = answer {
            let _ = reply.send(answer);
        }
        Ok(())
    }

    /// The breakpoints agents set, cleared (`agents.clear_breakpoints`):
    /// yours stay.
    pub(crate) fn agent_clear_breakpoints(&mut self) {
        let set: Vec<(PathBuf, usize)> = self.agent_calls.agent_breakpoints.drain().collect();
        let mut paths = Vec::new();
        for (path, line) in set {
            if let Some(lines) = self.breakpoints.get_mut(&path) {
                lines.remove(&line);
            }
            paths.push(path);
        }
        paths.sort();
        paths.dedup();
        for path in &paths {
            self.send_set_breakpoints_for(path);
        }
        self.refresh_debug_panes();
        self.set_message(format!(
            "cleared the breakpoints agents set in {} file(s)",
            paths.len()
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(app: &mut App, tool: &str, arguments: Value) -> Option<Reply> {
        let (reply, answer) = std::sync::mpsc::channel();
        if app.agent_debug_call(tool, arguments, reply).is_err() {
            return None;
        }
        answer.try_recv().ok()
    }

    #[test]
    fn breakpoints_are_set_listed_and_cleared_by_path() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.py");
        std::fs::write(&file, "x = 1\ny = 2\n").unwrap();
        let mut app = App::with_file(None);
        let path = file.to_string_lossy().into_owned();
        call(
            &mut app,
            "set_breakpoint",
            json!({ "path": path, "line": 2, "condition": "y > 1" }),
        )
        .unwrap()
        .unwrap();
        let listed = call(&mut app, "breakpoints", json!({})).unwrap().unwrap();
        assert_eq!(listed["breakpoints"][0]["line"], 2);
        assert_eq!(listed["breakpoints"][0]["condition"], "y > 1");
        call(
            &mut app,
            "clear_breakpoint",
            json!({ "path": path, "line": 2 }),
        )
        .unwrap()
        .unwrap();
        assert!(call(
            &mut app,
            "clear_breakpoint",
            json!({ "path": path, "line": 2 })
        )
        .unwrap()
        .is_err());
        call(
            &mut app,
            "set_breakpoint",
            json!({ "path": path, "line": 1 }),
        )
        .unwrap()
        .unwrap();
        app.agent_clear_breakpoints();
        assert!(
            app.breakpoints.values().all(|lines| lines.is_empty()),
            "the agents' are gone"
        );
    }

    #[test]
    fn an_agents_breakpoint_has_a_shade_of_its_own() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.py");
        std::fs::write(&file, "x = 1\ny = 2\n").unwrap();
        let path = file.to_string_lossy().into_owned();
        let mut app = App::with_file(Some(path.clone()));
        let buffer = app.focused_buffer_id();
        call(
            &mut app,
            "set_breakpoint",
            json!({ "path": path, "line": 2 }),
        )
        .unwrap()
        .unwrap();
        let plain = super::super::debug_breakpoints::BreakpointLook::Plain;
        assert_eq!(
            app.debug_marks(buffer, &[0, 1], 2),
            vec![(
                1,
                super::super::debug_host::DebugMark::AgentBreakpoint(plain)
            )]
        );
        // Kept with the session, its shade too.
        let saved = app.capture_debugging();
        let mut again = App::with_file(Some(path.clone()));
        again.restore_debugging(saved);
        assert_eq!(
            again.debug_marks(again.focused_buffer_id(), &[0, 1], 2),
            vec![(
                1,
                super::super::debug_host::DebugMark::AgentBreakpoint(plain)
            )]
        );
        app.agent_clear_breakpoints();
        assert!(app.debug_marks(buffer, &[0, 1], 2).is_empty());
    }

    #[test]
    fn moving_without_a_session_says_what_to_do() {
        let mut app = App::with_file(None);
        for tool in [
            "debug_continue",
            "debug_step",
            "debug_run_to",
            "debug_pause",
            "debug_stop",
        ] {
            let answer = call(&mut app, tool, json!({ "path": "/x.py", "line": 1 })).unwrap();
            assert!(answer.is_err(), "{tool}");
        }
        assert!(
            call(&mut app, "nope", json!({})).is_none(),
            "not this toolset's"
        );
    }

    #[test]
    fn a_wait_ends_at_a_new_stop_or_when_its_time_is_up() {
        let mut app = App::with_file(None);
        let (reply, answer) = std::sync::mpsc::channel();
        app.agent_debug_wait(reply, STEP_WAIT);
        app.agent_debug_tick(Instant::now());
        // No session at all: it's over, and the wait says so.
        assert_eq!(answer.try_recv().unwrap().unwrap()["session"], false);
        assert!(app.agent_calls.debug_waits.is_empty());
        app.agent_debug_stopped();
        assert_eq!(app.agent_calls.debug_stops, 1);
    }
}
