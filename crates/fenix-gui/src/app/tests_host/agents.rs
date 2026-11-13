//! The agents' tools and the control channel's methods (plan
//! `docs/plans/test-explorer.md` §10.3, §10.4, step 7): `tests`,
//! `test_failures`, `run_tests`, `run_test` where a provider serves the
//! file, `debug_test`; `fenix/tests`, `fenix/tests.failures`,
//! `fenix/tests.run`.
//!
//! **What they answer** is the explorer's model, as data: each test's
//! provider, id, place, state and time; a failure's message, place and
//! text (cut at 4 KiB each, the whole held to `agents.mcp.max_answer_kib`,
//! saying how many were left out); a run's counts and its failures, once
//! it ends. Nothing but the project's tests and what they printed.
//!
//! **What they may do** (§12): `run_tests`, `run_test` and `debug_test`
//! are `run` tools, asked about by default as every one is; and they run a
//! project's code only in a trusted project (`SPC p y`), the rule a lens
//! keeps (§9.5) -- so an agent never runs tests where your own click on a
//! lens would have been refused. `fenix/tests.run` keeps both rules too:
//! `agents.mcp.fenix_write`, as `fenix/tasks.run` does, and trust.
//!
//! **Waiting**: a run asked for is waited for (10 minutes by default, as
//! `run_task`), and answered when it ends; past its time, "still running"
//! with what's in. The run is the next of tests to end after the run going
//! when it was asked, or that run itself when it was the same.

use super::*;
use crate::ipc::Reply;
use fenix_tests::State;
use fenix_control::RpcError;
use serde_json::{json, Value};
use std::sync::mpsc::Sender;

/// How long `run_tests` and `run_test` wait for their run, unless told.
const RUN_WAIT: Duration = Duration::from_secs(10 * 60);
/// How long `debug_test` waits for its session to start (a build first,
/// maybe), before it waits for the first stop as `debug_start` does.
const DEBUG_WAIT: Duration = Duration::from_secs(10 * 60);
/// The most tests `tests` lists, and `fenix/tests`.
pub(crate) const AGENT_TESTS: usize = 500;
pub(crate) const CONTROL_TESTS: usize = 5_000;
/// A failure's text, and the end of a run's output, as they answer it.
pub(crate) const TEXT_BYTES: usize = 4 * 1024;

/// An agent waiting for a run of tests to end.
pub(crate) struct RunWait {
    root: PathBuf,
    /// The run going when it was asked, which isn't the one: none when it
    /// was the run going itself.
    after: Option<RunId>,
    reply: Sender<Reply>,
    until: Instant,
    /// `run_test`'s one test: its result is the answer.
    single: Option<fenix_tests::TestKey>,
}

/// An agent waiting for a test it asked to debug to start (`debug_test`).
pub(crate) struct DebugWait {
    root: PathBuf,
    run: RunId,
    reply: Sender<Reply>,
    until: Instant,
}

fn invalid(message: impl Into<String>) -> RpcError {
    RpcError::invalid_params(message)
}

/// A state as the answers say it.
fn state_name(state: State) -> &'static str {
    match state {
        State::Unknown => "not_run",
        State::Queued => "queued",
        State::Running => "running",
        State::Passed => "passed",
        State::Failed => "failed",
        State::Skipped => "skipped",
        State::Errored => "errored",
    }
}

fn kind_name(kind: fenix_tests::Kind) -> &'static str {
    match kind {
        fenix_tests::Kind::Group => "group",
        fenix_tests::Kind::File => "file",
        fenix_tests::Kind::Suite => "suite",
        fenix_tests::Kind::Test => "test",
        fenix_tests::Kind::Case => "case",
    }
}

/// `text`'s first `max` bytes, back to a character's start, saying how
/// many more there were.
pub(crate) fn cut_text(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n[{} bytes more]", &text[..end], text.len() - end)
}

/// `text`'s last `max` bytes, saying how many were before.
fn last_text(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let mut start = text.len() - max;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    format!("[{start} bytes before]\n{}", &text[start..])
}


/// How the answers name providers: by their id, `extension:provider` when
/// two extensions have one of that id.
fn provider_names(model: &Model) -> HashMap<ProviderId, String> {
    let providers = model.providers();
    providers
        .iter()
        .map(|p| {
            let shared = providers.iter().filter(|q| q.id.provider == p.id.provider).count() > 1;
            (p.id.clone(), if shared { format!("{}:{}", p.id.extension, p.id.provider) } else { p.id.provider.to_string() })
        })
        .collect()
}

/// The provider an answer's name stands for.
fn provider_named(model: &Model, name: &str) -> Result<ProviderId, String> {
    let all: Vec<ProviderId> = model.providers().iter().map(|p| p.id.clone()).collect();
    let found: Vec<&ProviderId> = all.iter().filter(|p| p.provider == name || format!("{}:{}", p.extension, p.provider) == name).collect();
    match found.as_slice() {
        [one] => Ok((*one).clone()),
        [] => Err(format!("no test provider {name} in this project")),
        _ => Err(format!("two extensions have a provider {name}: name it extension:{name}")),
    }
}

impl App {
    /// The project the agent's call is about: `path`'s, else the one
    /// you're in; with the explorer on and not remote.
    fn tests_agent_root(&mut self, path: Option<&Path>) -> Result<PathBuf, String> {
        if !self.tests_enabled() {
            return Err("the test explorer is off (tests.enabled)".into());
        }
        let root = match path {
            Some(path) => self.tests_root_of(path).ok_or_else(|| format!("{} isn't in a project", path.display()))?,
            None => {
                let raw = self.integration_root();
                let root = self.tests_canon(&raw);
                self.tests_alias(&root, &raw);
                root
            }
        };
        if fenix_vfs::is_remote(&root) {
            return Err("tests aren't available on a remote project yet".into());
        }
        self.tests_project(&root);
        Ok(root)
    }

    /// An argument's path, absolute: a relative one is the project's.
    fn tests_agent_path(&self, arguments: &Value) -> Option<PathBuf> {
        let path = arguments.get("path").and_then(Value::as_str).filter(|p| !p.is_empty())?;
        let path = PathBuf::from(path);
        Some(if path.is_absolute() { path } else { self.integration_root().join(path) })
    }

    /// Whether an agent may run the project at `root`'s code: only in a
    /// trusted project, as a lens (§9.5, §12); why not.
    fn tests_agent_may_run(&self, root: &Path) -> Result<(), String> {
        if self.project_is_trusted(root) {
            Ok(())
        } else {
            Err("not run: Fenix runs a project's tests for an agent only in a trusted project (SPC p y), as a lens does".into())
        }
    }

    /// The tests of the project at `root`, in display order: in `rel` when
    /// given, in `state` when given, matching every word of `filter`; at
    /// most `max`, saying how many more. `full`: the control channel's,
    /// with each one's label and kind.
    pub(in crate::app) fn tests_listed(&self, root: &Path, rel: Option<&str>, state: Option<&str>, filter: Option<&str>, max: usize, full: bool) -> Result<Value, String> {
        if let Some(state) = state.filter(|s| !["passed", "failed", "errored", "skipped", "not_run", "queued", "running"].contains(s)) {
            return Err(format!("state {state}: passed, failed, errored, skipped, not_run, queued or running"));
        }
        let Some(project) = self.tests.projects.get(root) else { return Ok(json!({ "root": root.to_string_lossy(), "tests": [], "total": 0 })) };
        let model = &project.model;
        let names = provider_names(model);
        let words: Vec<String> = filter.unwrap_or_default().split_whitespace().map(str::to_lowercase).collect();
        let mut tests = Vec::new();
        let mut total = 0usize;
        let mut stack: Vec<fenix_tests::NodeId> = Vec::new();
        for provider in model.providers() {
            stack.extend(model.roots(provider.provider).iter().rev());
            while let Some(id) = stack.pop() {
                let Some(n) = model.node(id) else { continue };
                stack.extend(n.children().iter().rev());
                let found = n.found();
                if !found.kind.is_test() || rel.is_some_and(|rel| n.file() != Some(rel)) || state.is_some_and(|s| s != state_name(n.state())) {
                    continue;
                }
                if !words.is_empty() {
                    let text = format!("{} {}", found.label, found.id).to_lowercase();
                    if !words.iter().all(|w| text.contains(w.as_str())) {
                        continue;
                    }
                }
                total += 1;
                if tests.len() >= max {
                    continue;
                }
                let mut test = json!({
                    "provider": names.get(&n.key().provider).cloned().unwrap_or_default(),
                    "id": found.id,
                    "path": n.file(),
                    "line": found.line.map(|l| l + 1),
                    "state": state_name(n.state()),
                    "durationMs": n.result().and_then(|r| r.duration_ms),
                    "stale": n.stale(),
                });
                if full {
                    test["label"] = json!(found.label);
                    test["kind"] = json!(kind_name(found.kind));
                }
                tests.push(test);
            }
        }
        let mut answer = json!({ "root": root.to_string_lossy(), "tests": tests, "total": total });
        if total > max {
            answer["more"] = json!(total - max);
            answer["note"] = json!(format!("{} more left out: path, state or filter narrows it", total - max));
        }
        Ok(answer)
    }

    /// One failed or errored test as the answers give it.
    fn tests_failure_json(model: &Model, names: &HashMap<ProviderId, String>, id: fenix_tests::NodeId) -> Option<Value> {
        let n = model.node(id)?;
        let result = n.result()?;
        let found = n.found();
        Some(json!({
            "provider": names.get(&n.key().provider).cloned().unwrap_or_default(),
            "id": found.id,
            "label": found.label,
            "state": state_name(n.state()),
            "path": n.file(),
            "line": found.line.map(|l| l + 1),
            "message": result.message,
            "at": result.at.as_ref().map(|at| json!({ "path": at.path, "line": at.line + 1, "column": at.column.map(|c| c + 1) })),
            "text": result.text.as_deref().map(|t| cut_text(&crate::tests_page::without_escapes(t), TEXT_BYTES)),
            "stale": n.stale(),
        }))
    }

    /// The failed and errored tests of the project at `root` -- in `rel`,
    /// or of the run that started at `since` (ms) -- held to `max` bytes in
    /// all, saying how many were left out.
    pub(in crate::app) fn tests_failures(&self, root: &Path, rel: Option<&str>, since: Option<u64>, max: usize) -> Value {
        let Some(project) = self.tests.projects.get(root) else { return json!({ "failures": [] }) };
        let model = &project.model;
        let names = provider_names(model);
        let mut failures = Vec::new();
        let mut bytes = 0usize;
        let mut left = 0usize;
        for id in model.failures() {
            let Some(n) = model.node(id) else { continue };
            if rel.is_some_and(|rel| n.file() != Some(rel)) || since.is_some_and(|since| n.result().is_none_or(|r| r.restored || r.when_ms < since)) {
                continue;
            }
            let Some(failure) = Self::tests_failure_json(model, &names, id) else { continue };
            let size = failure.to_string().len();
            if bytes + size > max {
                left += 1;
                continue;
            }
            bytes += size;
            failures.push(failure);
        }
        let mut answer = json!({ "failures": failures });
        if left > 0 {
            answer["more"] = json!(left);
            answer["note"] = json!(format!("{left} more left out (agents.mcp.max_answer_kib): test_failures with a path gives one file's"));
        }
        answer
    }

    // The agents' tools.

    /// `tests`: the project's tests, as the explorer has them.
    pub(in crate::app) fn tests_agent_tests(&mut self, arguments: &Value) -> Result<Value, RpcError> {
        let path = self.tests_agent_path(arguments);
        let root = self.tests_agent_root(path.as_deref()).map_err(invalid)?;
        let rel = path.as_deref().map(|p| self.tests_rel(&root, p));
        let state = arguments.get("state").and_then(Value::as_str).filter(|s| !s.is_empty());
        let filter = arguments.get("filter").and_then(Value::as_str);
        self.tests_listed(&root, rel.as_deref(), state, filter, AGENT_TESTS, false).map_err(invalid)
    }

    /// `test_failures`: the failed and errored tests, as the last runs left
    /// them.
    pub(in crate::app) fn tests_agent_failures(&mut self, arguments: &Value) -> Result<Value, RpcError> {
        let path = self.tests_agent_path(arguments);
        let root = self.tests_agent_root(path.as_deref()).map_err(invalid)?;
        let rel = path.as_deref().map(|p| self.tests_rel(&root, p));
        Ok(self.tests_failures(&root, rel.as_deref(), None, self.agent_answer_max()))
    }

    /// What `run_tests` and `fenix/tests.run` ask: their selections, and
    /// what they're called.
    fn tests_selections(&mut self, root: &Path, run: &fenix_control::TestsRunParams) -> Result<(Vec<Selection>, String), String> {
        let mut selections = Vec::new();
        let mut what = Vec::new();
        if run.all {
            selections.push(Selection::All);
            what.push("all the tests".to_string());
        }
        if run.failed {
            selections.push(Selection::Failed);
            what.push("the failed".to_string());
        }
        if let Some(path) = run.path.as_deref().filter(|p| !p.is_empty()) {
            let path = PathBuf::from(path);
            let path = if path.is_absolute() { path } else { root.join(path) };
            let rel = self.tests_rel(root, &path);
            what.push(rel.clone());
            selections.push(Selection::File(rel));
        }
        if let Some(tests) = run.tests.as_ref().filter(|t| !t.is_empty()) {
            let model = &self.tests.projects.get(root).ok_or("no test provider serves this project")?.model;
            let mut keys = Vec::new();
            for test in tests {
                let provider = provider_named(model, &test.provider)?;
                let key = fenix_tests::TestKey::new(&provider, &test.id);
                if model.find(&key).is_none() {
                    return Err(format!("no test {} of {}: tests lists them", test.id, test.provider));
                }
                keys.push(key);
            }
            what.push(if keys.len() == 1 { keys[0].id.clone() } else { format!("{} tests", keys.len()) });
            selections.push(Selection::Nodes(keys));
        }
        if selections.is_empty() {
            return Err("say what to run: tests, path, failed or all".into());
        }
        Ok((selections, what.join(", ")))
    }

    /// Asks the project at `root` for a run of `selections` for an agent:
    /// the run going then, which isn't its own unless it's the same, and
    /// whether it is.
    fn tests_agent_request(&mut self, root: &Path, selections: Vec<Selection>, what: &str) -> Result<(Option<RunId>, fenix_tests::Requested), String> {
        let going = self.tests.projects.get(root).and_then(|p| p.model.summary().running).map(|p| p.run);
        let requested = self.tests_request(root, selections, what)?;
        let after = if requested == fenix_tests::Requested::AlreadyGoing { None } else { going };
        Ok((after, requested))
    }

    /// `run_tests`: runs what it names through the explorer, and answers
    /// when the run ends (its counts and its failures), or "still running"
    /// past its time.
    pub(in crate::app) fn tests_agent_run(&mut self, arguments: &Value, reply: Sender<Reply>) {
        let started = (|| {
            let run: fenix_control::TestsRunParams = serde_json::from_value(json!({
                "tests": arguments.get("tests"),
                "path": arguments.get("path"),
                "failed": arguments.get("failed").and_then(Value::as_bool).unwrap_or(false),
                "all": arguments.get("all").and_then(Value::as_bool).unwrap_or(false),
            }))
            .map_err(|e| e.to_string())?;
            let root = self.tests_agent_root(None)?;
            self.tests_agent_may_run(&root)?;
            let (selections, what) = self.tests_selections(&root, &run)?;
            let (after, _) = self.tests_agent_request(&root, selections, &what)?;
            Ok::<_, String>((root, after))
        })();
        match started {
            Ok((root, after)) => {
                let until = Instant::now() + Self::tests_wait_for(arguments);
                self.tests.run_waits.push(RunWait { root, after, reply, until, single: None });
            }
            Err(why) => {
                let _ = reply.send(Err(invalid(why)));
            }
        }
    }

    fn tests_wait_for(arguments: &Value) -> Duration {
        Duration::from_secs(arguments.get("timeout").and_then(Value::as_u64).unwrap_or(RUN_WAIT.as_secs()).clamp(1, 3600))
    }

    /// `run_test` at `path`'s line `line` (from 1), where a provider serves
    /// the file: the test there through the explorer, answered with its
    /// result. `Err(reply)`: no provider serves it, and today's `run_test`
    /// answers.
    pub(in crate::app) fn tests_agent_run_test(&mut self, path: &Path, line: usize, arguments: &Value, reply: Sender<Reply>) -> Result<(), Sender<Reply>> {
        let (root, node) = match self.tests_door(path, line.saturating_sub(1) as u32) {
            Door::Fallback => return Err(reply),
            Door::NotATest(why) => {
                let _ = reply.send(Err(invalid(why)));
                return Ok(());
            }
            Door::Test { root, node } => (root, node),
        };
        let started = (|| {
            self.tests_agent_may_run(&root)?;
            let key = self.tests.projects.get(&root).and_then(|p| p.model.node(node)).map(|n| n.key()).ok_or("that test is gone")?;
            let going = self.tests.projects.get(&root).and_then(|p| p.model.summary().running).map(|p| p.run);
            // Asked as `SPC T t` asks: queued behind a run going.
            let (_, requested) = self.tests_door_run(path, &root, node)?;
            Ok::<_, String>((key, if requested == fenix_tests::Requested::AlreadyGoing { None } else { going }))
        })();
        match started {
            Ok((key, after)) => {
                let until = Instant::now() + Self::tests_wait_for(arguments);
                self.tests.run_waits.push(RunWait { root, after, reply, until, single: Some(key) });
            }
            Err(why) => {
                let _ = reply.send(Err(invalid(why)));
            }
        }
        Ok(())
    }

    /// `debug_test`: the test at a line debugged through the explorer
    /// (§11); answered at the first stop, as `debug_start` is, or with why
    /// it didn't start.
    pub(in crate::app) fn tests_agent_debug(&mut self, arguments: &Value, reply: Sender<Reply>) {
        let started = (|| {
            let path = self.tests_agent_path(arguments).ok_or("path: which file")?;
            let line = arguments.get("line").and_then(Value::as_u64).filter(|l| *l > 0).ok_or("line: which line, from 1")? as u32;
            if !self.tests_enabled() {
                return Err("the test explorer is off (tests.enabled)".to_string());
            }
            let (root, node) = match self.tests_door(&path, line - 1) {
                Door::Test { root, node } => (root, node),
                Door::NotATest(why) => return Err(why),
                Door::Fallback => return Err(format!("no test provider serves {}: debug_start debugs a program", path.display())),
            };
            self.tests_agent_may_run(&root)?;
            let run = self.tests_debug(&root, node, None)?;
            Ok((root, run))
        })();
        match started {
            Ok((root, run)) => self.tests.debug_waits.push(DebugWait { root, run, reply, until: Instant::now() + DEBUG_WAIT }),
            Err(why) => {
                let _ = reply.send(Err(invalid(why)));
            }
        }
    }

    // How a wait ends.

    /// How run `end` went, for an agent: its counts and its failures, or
    /// (`single`) that test's result.
    fn tests_run_answer(&self, root: &Path, end: &fenix_tests::RunEnd, single: Option<&fenix_tests::TestKey>) -> Value {
        let output = self.tests.projects.get(root).and_then(|p| p.output).and_then(|b| self.buffers.get(b)).map(|ob| ob.buffer.text()).unwrap_or_default();
        let halted = match &end.halted {
            Some(fenix_tests::Halt::Stopped(why)) => Some(format!("stopped: {why}")),
            Some(fenix_tests::Halt::TimedOut) => Some("timed out (tests.timeout_minutes)".to_string()),
            None => None,
        };
        if let Some(key) = single {
            let model = self.tests.projects.get(root).map(|p| &p.model);
            let node = model.and_then(|m| m.find(key)).and_then(|id| model?.node(id));
            let result = node.as_ref().and_then(|n| n.result()).filter(|r| !r.restored && r.when_ms >= end.started_ms);
            let names = model.map(provider_names).unwrap_or_default();
            return json!({
                "test": { "provider": names.get(&key.provider).cloned().unwrap_or_else(|| key.provider.provider.to_string()), "id": key.id },
                "state": node.as_ref().map_or("not_run", |n| state_name(n.state())),
                "durationMs": result.and_then(|r| r.duration_ms),
                "message": result.and_then(|r| r.message.clone()),
                "at": result.and_then(|r| r.at.as_ref()).map(|at| json!({ "path": at.path, "line": at.line + 1, "column": at.column.map(|c| c + 1) })),
                "text": result.and_then(|r| r.text.as_deref()).map(|t| cut_text(&crate::tests_page::without_escapes(t), TEXT_BYTES)),
                "halted": halted,
                "didntBuild": end.first_broken,
                "output": last_text(&output, TEXT_BYTES),
            });
        }
        let mut answer = self.tests_failures(root, None, Some(end.started_ms), self.agent_answer_max().saturating_sub(TEXT_BYTES * 2));
        answer["run"] = json!(end.run.0);
        answer["passed"] = json!(end.passed);
        answer["failed"] = json!(end.failed);
        answer["errored"] = json!(end.errored);
        answer["skipped"] = json!(end.skipped);
        answer["halted"] = json!(halted);
        answer["didntBuild"] = json!(end.first_broken);
        if end.first_broken.is_some() {
            answer["output"] = json!(last_text(&output, TEXT_BYTES));
        }
        answer
    }

    /// A run of tests ended in the project at `root`: the agents waiting
    /// for it hear how it went.
    pub(super) fn tests_agents_run_ended(&mut self, root: &Path, end: &fenix_tests::RunEnd) {
        if self.tests.run_waits.is_empty() {
            return;
        }
        let (done, waiting): (Vec<RunWait>, Vec<RunWait>) = std::mem::take(&mut self.tests.run_waits).into_iter().partition(|w| w.root == root && w.after != Some(end.run));
        self.tests.run_waits = waiting;
        for wait in done {
            let answer = self.tests_run_answer(root, end, wait.single.as_ref());
            let _ = wait.reply.send(Ok(answer));
        }
    }

    /// A debug of the project at `root` ended: the agents waiting for one
    /// of its that's over hear. Started, they wait on for its first stop,
    /// as `debug_start` does.
    pub(super) fn tests_agents_debug_done(&mut self, root: &Path, outcome: Result<(), String>) {
        if self.tests.debug_waits.is_empty() {
            return;
        }
        let current = self.tests.projects.get(root).and_then(|p| p.debug.as_ref()).map(|d| d.run);
        let (done, waiting): (Vec<DebugWait>, Vec<DebugWait>) = std::mem::take(&mut self.tests.debug_waits).into_iter().partition(|w| w.root == root && Some(w.run) != current);
        self.tests.debug_waits = waiting;
        for wait in done {
            match &outcome {
                Ok(()) => self.agent_debug_wait_start(wait.reply),
                Err(why) => {
                    let _ = wait.reply.send(Err(invalid(why.clone())));
                }
            }
        }
    }

    /// Waits past their time, answered: a run still going says what's in;
    /// a debug still being got ready says so. When the next is due.
    pub(in crate::app) fn tests_agent_waits_tick(&mut self, now: Instant) -> Option<Instant> {
        let late: Vec<RunWait> = {
            let (late, waiting) = std::mem::take(&mut self.tests.run_waits).into_iter().partition(|w| now >= w.until);
            self.tests.run_waits = waiting;
            late
        };
        for wait in late {
            let project = self.tests.projects.get(&wait.root);
            let running = project.and_then(|p| p.model.summary().running);
            let since = project.and_then(|p| p.run).map(|(_, started)| started);
            let mut answer = match since {
                Some(since) => self.tests_failures(&wait.root, None, Some(since), self.agent_answer_max()),
                None => json!({ "failures": [] }),
            };
            answer["running"] = json!(true);
            if let Some(running) = running.filter(|r| r.tests) {
                answer["done"] = json!(running.done);
                answer["total"] = json!(running.total);
            }
            answer["note"] = json!("still running: tests and test_failures follow it");
            let _ = wait.reply.send(Ok(answer));
        }
        let late: Vec<DebugWait> = {
            let (late, waiting) = std::mem::take(&mut self.tests.debug_waits).into_iter().partition(|w| now >= w.until);
            self.tests.debug_waits = waiting;
            late
        };
        for wait in late {
            let _ = wait.reply.send(Ok(json!({ "session": false, "note": "still getting the test ready to debug (its build, maybe): debug_state looks again" })));
        }
        self.tests.run_waits.iter().map(|w| w.until).chain(self.tests.debug_waits.iter().map(|w| w.until)).min()
    }

    // The control channel (§10.4).

    /// `fenix/tests`, `fenix/tests.failures`, `fenix/tests.run`.
    pub(in crate::app) fn tests_control(&mut self, call: fenix_control::Call) -> Result<Value, RpcError> {
        use fenix_control::Call;
        if !self.tests_enabled() {
            return Err(RpcError::new(fenix_control::protocol::code::METHOD_NOT_FOUND, "the test explorer is off (tests.enabled)"));
        }
        let path_of = |app: &App, path: &Option<String>| path.as_deref().filter(|p| !p.is_empty()).map(|p| if Path::new(p).is_absolute() { PathBuf::from(p) } else { app.integration_root().join(p) });
        match call {
            Call::Tests(params) => {
                let path = path_of(self, &params.path);
                let root = self.tests_agent_root(path.as_deref()).map_err(invalid)?;
                let rel = path.as_deref().map(|p| self.tests_rel(&root, p));
                self.tests_listed(&root, rel.as_deref(), params.state.as_deref(), None, CONTROL_TESTS, true).map_err(invalid)
            }
            Call::TestsFailures(params) => {
                let path = path_of(self, &params.path);
                let root = self.tests_agent_root(path.as_deref()).map_err(invalid)?;
                let rel = path.as_deref().map(|p| self.tests_rel(&root, p));
                Ok(self.tests_failures(&root, rel.as_deref(), None, self.agent_answer_max()))
            }
            Call::TestsRun(params) => {
                self.control_writes_allowed()?;
                let root = self.tests_agent_root(None).map_err(invalid)?;
                self.tests_agent_may_run(&root).map_err(invalid)?;
                let (selections, what) = self.tests_selections(&root, &params).map_err(invalid)?;
                let (_, requested) = self.tests_agent_request(&root, selections, &what).map_err(invalid)?;
                // Started now, its id is the run going; one that waits has
                // none yet.
                let run = match requested {
                    fenix_tests::Requested::Starts | fenix_tests::Requested::AlreadyGoing => self.tests.projects.get(&root).and_then(|p| p.model.summary().running).filter(|r| r.tests).map(|r| r.run.0),
                    _ => None,
                };
                let requested = match requested {
                    fenix_tests::Requested::Starts => "starts",
                    fenix_tests::Requested::AlreadyGoing => "already-going",
                    fenix_tests::Requested::Waits | fenix_tests::Requested::Joined => "waits",
                    fenix_tests::Requested::Replaces => "replaces",
                    fenix_tests::Requested::Nothing => "nothing",
                };
                Ok(json!({ "run": run, "requested": requested }))
            }
            _ => Err(RpcError::internal("not a test explorer's method")),
        }
    }
}
