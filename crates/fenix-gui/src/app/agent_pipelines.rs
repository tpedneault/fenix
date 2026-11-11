//! Pipelines in the editor (`SPC a P`): each step of a
//! `fenix_acp::pipeline::Run` is a thread of its own, shown in the dock
//! as it runs. Its prompt waits until its session is up, goes out in the
//! step's mode with its toolsets, and its turn's end moves the run on.
//! What the steps change goes into one ledger -- the first thread's -- so
//! you review the outcome once (`SPC a r`). A read-only step's changes
//! are put back. A gate is a question in the step's thread: `y` goes on,
//! `r` runs the step again, `n` stops, and writing to the step is another
//! round of it, the question taken back until that's done. A step that
//! ends badly (refused, cancelled, cut off) asks at its gate too. A step
//! with a command runs in the project on `fenix-tasks`' runner, off the
//! event loop, and says how it went on the thread before it.
//!
//! A run that stops is kept, in memory and beside its threads in the
//! store (`store::SavedRun`), written each time it moves: `SPC a g`
//! (`agents.pipeline_go`) picks it up at the step it stopped at, in this
//! Fenix or after a restart. A run that's done is forgotten.

use fenix_acp::hub::ThreadId;
use fenix_acp::pipeline::{Next, PipelineSpec, Run, State, StepEnd};
use fenix_acp::protocol::ContentBlock;
use fenix_tasks::{OutputLine, TaskEvent, TaskOutcome, TaskRunner};
use serde_json::json;

use super::*;
use super::agents::Pick;

/// A pipeline under way.
pub(crate) struct Running {
    pub run: Run,
    pub root: PathBuf,
    /// Each step's thread, in the order they ran.
    pub threads: Vec<ThreadId>,
    /// The thread whose ledger holds every step's changes.
    pub ledger: Option<ThreadId>,
    /// The mode a step's session was in before it was put in the step's:
    /// where a later step with no mode of its own goes back to.
    pub usual_mode: Option<String>,
    /// What each step's thread is told about the run it belongs to, kept
    /// with it (the thread list groups by it).
    pub tag: fenix_acp::hub::RunTag,
}

/// What a command step's output is kept to, from its end.
const COMMAND_KEPT: usize = 256 * 1024;
/// Lines of a failed command shown on the thread.
const COMMAND_SHOWN: usize = 12;

/// A command step running.
struct Command {
    line: String,
    runner: TaskRunner,
    state: Arc<Mutex<CommandState>>,
}

#[derive(Default)]
struct CommandState {
    output: String,
    /// It ended: its exit code, `None` when it was stopped.
    ended: Option<Option<i32>>,
}

/// A step's thread, waiting for its session to send its prompt.
struct Held {
    run: u64,
    prompt: String,
}

#[derive(Default)]
pub(crate) struct Pipelines {
    pub runs: HashMap<u64, Running>,
    next: u64,
    /// Thread -> the run it's a step of.
    steps: HashMap<ThreadId, u64>,
    held: HashMap<ThreadId, Held>,
    /// The gate questions up, by card key.
    gates: HashMap<String, u64>,
    /// `SPC a P` picked one: the next prompt sent starts it.
    pub armed: Option<PipelineSpec>,
    /// The last review's findings, by the thread whose ledger they're
    /// about: shown beside their hunks in Agent changes.
    pub findings: HashMap<ThreadId, Vec<fenix_acp::pipeline::Finding>>,
    /// What a gate passes on, by run, for `e` to edit.
    passing: HashMap<u64, String>,
    /// The steps of runs that ended, for `[s` / `]s` still.
    ended: Vec<Vec<ThreadId>>,
    /// Commands running, by run.
    commands: HashMap<u64, Command>,
}

fn gate_key(run: u64, step: usize) -> String {
    format!("pipeline-gate-{run}-{step}")
}

/// How a run that stopped for `why` says so: `stopped: ...`, once.
fn stopped_how(why: &str) -> String {
    if why.starts_with("stopped") { why.to_string() } else { format!("stopped: {why}") }
}

impl App {
    /// The pipelines there are: Fenix's own, then yours
    /// (`pipelines/*.toml` in the settings folder), then the project's
    /// (`.fenix/pipelines/*.toml`), a later one replacing an earlier of
    /// the same name.
    pub(super) fn pipeline_specs(&self) -> (Vec<PipelineSpec>, Vec<String>) {
        let mut specs = PipelineSpec::built_in();
        let mut problems = Vec::new();
        let mut dirs: Vec<PathBuf> = fenix_storage::paths::Roots::current().map(|r| r.settings_file().with_file_name("pipelines")).into_iter().collect();
        dirs.push(self.agents_root().join(".fenix").join("pipelines"));
        for dir in dirs {
            let Ok(entries) = std::fs::read_dir(&dir) else { continue };
            let mut files: Vec<PathBuf> = entries.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "toml")).collect();
            files.sort();
            for file in files {
                let name = file.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                match std::fs::read_to_string(&file).map_err(|e| e.to_string()).and_then(|text| PipelineSpec::parse(&name, &text)) {
                    Ok(spec) => {
                        specs.retain(|s| s.name != spec.name);
                        specs.push(spec);
                    }
                    Err(why) => problems.push(why),
                }
            }
        }
        (specs, problems)
    }

    /// `SPC a P`: picks a pipeline; the next prompt you send starts it.
    pub(crate) fn pipeline_pick(&mut self) {
        let (specs, problems) = self.pipeline_specs();
        if let Some(problem) = problems.first() {
            self.set_error(problem.clone());
        }
        let candidates = specs.into_iter().map(|s| fenix_picker::Candidate::new(format!("{}   {}", s.title, s.steps.iter().map(|st| st.run.as_ref().map_or_else(|| st.name.clone(), |run| format!("{} ({run})", st.name))).collect::<Vec<_>>().join(" → ")), Pick::Pipeline(s))).collect();
        self.enter_picker(ActivePicker::Agents(fenix_picker::PickerState::new(candidates)));
    }

    /// A pipeline picked: armed, the prompt ready for what it's to do.
    pub(super) fn pipeline_armed(&mut self, spec: PipelineSpec) {
        let title = spec.title.clone();
        self.pipelines.armed = Some(spec);
        if self.agents.shown(&self.pages).is_none() {
            self.agents_new_thread();
        } else {
            self.agents_focus_composer();
        }
        self.set_message(format!("{title}: write what it's to do, and send it to start"));
    }

    /// Starts the armed pipeline on `input`, when one is: true when it
    /// took the prompt.
    pub(super) fn pipeline_take_prompt(&mut self, input: &str) -> bool {
        let Some(spec) = self.pipelines.armed.take() else { return false };
        let pipeline = spec.title.clone();
        let (run, next) = Run::start(spec, input);
        self.pipelines.next += 1;
        let id = self.pipelines.next;
        let root = self.agents_root();
        let started = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        // Unique across restarts: when, and which of this run of Fenix's.
        let tag = fenix_acp::hub::RunTag { id: format!("run-{started}-{}-{id}", std::process::id()), pipeline, request: input.to_string(), started, ..Default::default() };
        self.pipelines.runs.insert(id, Running { run, root, threads: Vec::new(), ledger: None, usual_mode: None, tag });
        self.pipeline_do(id, next);
        true
    }

    fn pipeline_do(&mut self, id: u64, next: Next) {
        match next {
            Next::Start { step, prompt } => self.pipeline_start_step(id, step, prompt),
            Next::Run { command, .. } => self.pipeline_run_command(id, command),
            Next::Gate { step, passing, trouble } => self.pipeline_ask_at_gate(id, step, passing, trouble),
            Next::Done { summary } => return self.pipeline_finished(id, &format!("done: {summary}")),
            Next::Stopped { why } => return self.pipeline_finished(id, &stopped_how(&why)),
        }
        self.pipeline_keep(id);
    }

    /// The gate's question, on the step's thread (a command's: the thread
    /// before it): go on with what it passes, edit it first, run the step
    /// again, or stop. With nothing to pass, only the last two.
    fn pipeline_ask_at_gate(&mut self, id: u64, step: usize, passing: String, trouble: Option<String>) {
        let Some(thread) = self.pipelines.runs.get(&id).and_then(|r| r.threads.last().cloned()) else { return };
        let steps = &self.pipelines.runs[&id].run.spec.steps;
        let name = steps[step].name.clone();
        let command = steps[step].run.is_some();
        let next = steps.get(step + 1).map(|s| s.name.clone()).unwrap_or_default();
        let onward = if next.is_empty() { "Finish".to_string() } else { format!("Go on to {next}") };
        // A step that ended well passes what it has, even nothing; one
        // that didn't, only what it got to.
        let has = trouble.is_none() || !passing.trim().is_empty();
        let key = gate_key(id, step);
        use fenix_acp::protocol::PermissionKind;
        let again = format!("Run {name} again");
        let go = if trouble.is_some() && command { format!("{onward} anyway") } else { onward.clone() };
        let mut answers: Vec<(&str, &str, PermissionKind)> = Vec::new();
        if has {
            answers.push(("allow_once", &go, PermissionKind::AllowOnce));
            answers.push(("edit", "Edit what's passed on, then go on", PermissionKind::Other));
        }
        answers.push(("retry", &again, PermissionKind::Other));
        answers.push(("reject_once", "Stop", PermissionKind::RejectOnce));
        let question = match (&trouble, has) {
            (Some(why), true) if command => format!("Pipeline: {why}. {go}, or run it again"),
            (Some(why), true) => format!("Pipeline: {why}. {onward} with what it has, or write to it for another round"),
            (Some(why), false) if command => format!("Pipeline: {why}. Run it again, or stop"),
            (Some(why), false) => format!("Pipeline: {why}. Run {name} again, or write to it for another round"),
            (None, _) if passing == fenix_acp::pipeline::NO_CHANGES => format!("Pipeline: {name} changed nothing. {onward} anyway, or write to it"),
            (None, _) => format!("Pipeline: {name} is done. {onward}, or write to it for another round"),
        };
        let fx = self.agents.hub.ask_fenix_with(&thread, &key, &question, fenix_acp::protocol::ToolKind::Other, json!({ "passing": passing }), &answers);
        self.pipelines.passing.insert(id, passing);
        self.pipelines.gates.insert(key, id);
        self.agents_run(fx);
    }

    /// The run kept as it is now, beside its threads.
    fn pipeline_keep(&mut self, id: u64) {
        let (Some(store), Some(r)) = (self.agents.store.as_ref(), self.pipelines.runs.get(&id)) else { return };
        let saved = fenix_acp::store::SavedRun { run: r.run.clone(), root: r.root.clone(), threads: r.threads.clone(), ledger: r.ledger.clone(), usual_mode: r.usual_mode.clone(), tag: r.tag.clone() };
        if let Err(e) = store.save_run(&saved) {
            eprintln!("fenix: couldn't keep pipeline run {}: {e}", r.tag.id);
        }
    }

    /// Run `id` stops where it is, for `why`: kept, to be picked up.
    fn pipeline_stop(&mut self, id: u64, why: &str) {
        if let Some(r) = self.pipelines.runs.get_mut(&id) {
            r.run.stop(why);
        }
        self.pipeline_finished(id, &stopped_how(why));
    }

    /// A run done with (`how`): one that stopped is kept, its steps told
    /// how it ended and how to go on; one that's done is forgotten.
    fn pipeline_finished(&mut self, id: u64, how: &str) {
        if self.pipelines.runs.get(&id).is_some_and(|r| r.run.stopped_at().is_some()) {
            return self.pipeline_stopped(id, how);
        }
        let Some(running) = self.pipelines.runs.remove(&id) else { return };
        if let Some(store) = &self.agents.store {
            let _ = store.forget_run(&running.tag.id);
        }
        self.pipelines.passing.remove(&id);
        if let Some(command) = self.pipelines.commands.remove(&id) {
            command.runner.cancel();
        }
        self.pipelines.ended.push(running.threads.clone());
        self.pipelines.gates.retain(|_, run| *run != id);
        for thread in &running.threads {
            self.pipelines.steps.remove(thread);
            self.pipelines.held.remove(thread);
            // Each step is told how the run ended, and where its changes
            // are, and kept so.
            if let Some(tag) = self.agents.hub.meta_mut(thread).and_then(|m| m.run.as_mut()) {
                tag.ended = Some(how.to_string());
                tag.ledger = running.ledger.clone();
            }
            self.agents_keep_thread(thread);
        }
        let review = if running.ledger.is_some() { " -- SPC a r reviews what it changed" } else { "" };
        self.set_message(format!("{}: {how}{review}", running.run.spec.title));
        // Left on a step's thread, you'd review nothing: the changes are
        // on the ledger's.
        if let Some(ledger) = running.ledger.clone().filter(|ledger| self.agents.shown(&self.pages).is_some_and(|shown| shown != *ledger && running.threads.contains(&shown))) {
            let pane = self.focused_pane_id();
            self.agents_show(ledger);
            self.windows_mut().focus(pane);
        }
        if let Some(thread) = running.ledger.clone().or_else(|| running.threads.last().cloned()) {
            if let Some(t) = self.agents.hub.thread_mut(&thread) {
                t.notice(fenix_acp::thread::NoticeKind::Info, format!("Pipeline {}: {how}", running.run.spec.title));
            }
            self.agents_run(vec![fenix_acp::hub::Effect::Changed(thread)]);
        }
    }

    /// Run `id` stopped: kept as it is -- its run, its threads, its
    /// ledger -- and each step told how it ended, the notice saying how
    /// it's picked up.
    fn pipeline_stopped(&mut self, id: u64, how: &str) {
        self.pipelines.passing.remove(&id);
        self.pipelines.gates.retain(|_, run| *run != id);
        if let Some(command) = self.pipelines.commands.remove(&id) {
            command.runner.cancel();
        }
        let Some(running) = self.pipelines.runs.get(&id) else { return };
        let (title, threads, ledger) = (running.run.spec.title.clone(), running.threads.clone(), running.ledger.clone());
        for thread in &threads {
            self.pipelines.held.remove(thread);
            if let Some(tag) = self.agents.hub.meta_mut(thread).and_then(|m| m.run.as_mut()) {
                tag.ended = Some(how.to_string());
                tag.ledger = ledger.clone();
            }
            self.agents_keep_thread(thread);
        }
        self.pipeline_keep(id);
        let said = self.keys_text(&format!("Pipeline {title}: {how} -- SPC a g picks it up"));
        self.set_message(said.trim_start_matches("Pipeline ").to_string());
        match threads.last() {
            Some(thread) => {
                if let Some(t) = self.agents.hub.thread_mut(thread) {
                    t.notice(fenix_acp::thread::NoticeKind::Info, said);
                }
                self.agents_run(vec![fenix_acp::hub::Effect::Changed(thread.clone())]);
            }
            // Stopped before its first step had a thread: nothing to pick
            // up from.
            None => {
                if let Some(r) = self.pipelines.runs.remove(&id) {
                    if let Some(store) = &self.agents.store {
                        let _ = store.forget_run(&r.tag.id);
                    }
                }
            }
        }
    }

    /// `SPC a g` (`agents.pipeline_go`) on a pipeline's step: a stopped
    /// run's gate back, at the step it stopped at; at a gate, `y`.
    pub(crate) fn pipeline_go(&mut self) {
        let Some(thread) = self.agents.shown(&self.pages) else {
            return self.set_message(self.keys_text("no agent thread open: SPC a g on a pipeline's step, or Alt+G on its run in SPC a t"));
        };
        let run = self.pipelines.steps.get(&thread).and_then(|id| self.pipelines.runs.get(id)).map(|r| r.tag.id.clone()).or_else(|| self.agents.hub.meta(&thread).and_then(|m| m.run.as_ref().map(|r| r.id.clone())));
        match run {
            Some(run) => self.pipeline_go_run(&run),
            None => self.set_message("this thread isn't a pipeline's step"),
        }
    }

    /// Run `run` (its tag's id) picked up: read back when it isn't here.
    pub(crate) fn pipeline_go_run(&mut self, run: &str) {
        let found = self.pipelines.runs.iter().find(|(_, r)| r.tag.id == run).map(|(id, _)| *id);
        let Some(id) = found.or_else(|| self.pipeline_read_back(run)) else {
            return self.set_message("that run can't be picked up: it's done, or an older Fenix ran it");
        };
        let running = &self.pipelines.runs[&id];
        let title = running.run.spec.title.clone();
        let step_name = |i: usize| running.run.spec.steps[i].name.clone();
        match running.run.state.clone() {
            State::Running(i) => self.set_message(self.keys_text(&format!("{title}: {} is at work -- SPC a k stops it first", step_name(i)))),
            State::AtGate(i) => {
                let key = gate_key(id, i);
                let thread = running.threads.last().cloned();
                let can_go = running.run.gate.as_ref().is_some_and(|g| g.trouble.is_none() || !g.passing.trim().is_empty());
                match thread.filter(|_| can_go && self.pipelines.gates.contains_key(&key)) {
                    Some(thread) => {
                        let fx = self.agents.hub.answer_permission(&thread, &key, Some("allow_once"));
                        self.agents_run(fx);
                    }
                    None => self.set_message(format!("{title}: {} has nothing to pass on -- r on its question runs it again, n stops", step_name(i))),
                }
            }
            State::Stopped { .. } => {
                let threads = running.threads.clone();
                for thread in &threads {
                    if let Some(tag) = self.agents.hub.meta_mut(thread).and_then(|m| m.run.as_mut()) {
                        tag.ended = None;
                    }
                    self.agents_keep_thread(thread);
                }
                if let Some(last) = threads.last() {
                    self.agents_show(last.clone());
                }
                let Some(next) = self.pipelines.runs.get_mut(&id).and_then(|r| r.run.resume()) else { return };
                self.pipeline_do(id, next);
            }
            State::Done => self.set_message(format!("{title} is done")),
        }
    }

    /// A run kept in the store, read back and made one of this Fenix's,
    /// its threads read in: its local id. `None` when there's none.
    fn pipeline_read_back(&mut self, run: &str) -> Option<u64> {
        let saved = self.agents.store.as_ref()?.load_run(run).ok()?;
        for thread in &saved.threads {
            self.agents.load_one(thread);
        }
        self.pipelines.next += 1;
        let id = self.pipelines.next;
        for thread in &saved.threads {
            self.pipelines.steps.insert(thread.clone(), id);
        }
        self.pipelines.runs.insert(id, Running { run: saved.run, root: saved.root, threads: saved.threads, ledger: saved.ledger, usual_mode: saved.usual_mode, tag: saved.tag });
        Some(id)
    }

    /// Run `run` (its tag's id) archived or deleted: no longer kept to be
    /// picked up.
    pub(crate) fn pipeline_forget(&mut self, run: &str) {
        let ids: Vec<u64> = self.pipelines.runs.iter().filter(|(_, r)| r.tag.id == run && !r.run.is_live()).map(|(id, _)| *id).collect();
        for id in ids {
            if let Some(r) = self.pipelines.runs.remove(&id) {
                for thread in &r.threads {
                    self.pipelines.steps.remove(thread);
                }
                self.pipelines.ended.push(r.threads);
            }
        }
        if let Some(store) = &self.agents.store {
            let _ = store.forget_run(run);
        }
    }

    /// Whether run `run` (its tag's id) is under way here: a step at work
    /// or at its gate. A stopped one isn't.
    pub(crate) fn pipeline_is_live(&self, run: &str) -> bool {
        self.pipelines.runs.values().any(|r| r.tag.id == run && r.run.is_live())
    }

    /// A word on the run's latest thread, or the status line before it
    /// has one.
    fn pipeline_say(&mut self, id: u64, kind: fenix_acp::thread::NoticeKind, text: String) {
        match self.pipelines.runs.get(&id).and_then(|r| r.threads.last().cloned()) {
            Some(thread) => {
                if let Some(t) = self.agents.hub.thread_mut(&thread) {
                    t.notice(kind, text);
                }
                self.agents_run(vec![fenix_acp::hub::Effect::Changed(thread)]);
            }
            None => self.set_message(text),
        }
    }

    fn pipeline_run_command(&mut self, id: u64, line: String) {
        let Some(running) = self.pipelines.runs.get(&id) else { return };
        let root = running.root.clone();
        let progress = running.run.progress();
        let mut process = super::agents::agent_command(&line, &[]);
        process.current_dir(&root);
        let state = Arc::new(Mutex::new(CommandState::default()));
        let sink = state.clone();
        let runner = TaskRunner::spawn_command(process, move |event| {
            let mut s = sink.lock().unwrap();
            match event {
                TaskEvent::Output(lines) => {
                    for OutputLine { text, .. } in lines {
                        s.output.push_str(&text);
                        s.output.push('\n');
                    }
                    if s.output.len() > COMMAND_KEPT {
                        let mut start = s.output.len() - COMMAND_KEPT;
                        while !s.output.is_char_boundary(start) {
                            start += 1;
                        }
                        s.output.drain(..start);
                    }
                }
                TaskEvent::Finished(outcome) => {
                    s.ended = Some(match outcome {
                        TaskOutcome::Exited { success: true, .. } => Some(0),
                        TaskOutcome::Exited { code, .. } => Some(code.unwrap_or(-1)),
                        TaskOutcome::Cancelled => None,
                        TaskOutcome::Failed(why) => {
                            s.output.push_str(&format!("{why}\n"));
                            Some(127)
                        }
                    });
                }
            }
            true
        });
        match runner {
            Ok(runner) => {
                self.pipelines.commands.insert(id, Command { line: line.clone(), runner, state });
                self.pipeline_say(id, fenix_acp::thread::NoticeKind::Info, format!("Pipeline ({progress}): running `{line}`"));
            }
            Err(e) => self.pipeline_stop(id, &format!("couldn't run `{line}`: {e}")),
        }
    }

    /// Commands that ended move their runs on. Whether any is running.
    pub(super) fn pipeline_commands_tick(&mut self) -> bool {
        let ended: Vec<u64> = self.pipelines.commands.iter().filter(|(_, c)| c.state.lock().unwrap().ended.is_some()).map(|(id, _)| *id).collect();
        for id in ended {
            let Some(command) = self.pipelines.commands.remove(&id) else { continue };
            let (exit, output) = {
                let s = command.state.lock().unwrap();
                (s.ended.flatten(), s.output.clone())
            };
            use fenix_acp::thread::NoticeKind;
            match exit {
                Some(0) => self.pipeline_say(id, NoticeKind::Info, format!("`{}` passed", command.line)),
                Some(code) => {
                    let lines: Vec<&str> = output.lines().collect();
                    let tail = lines[lines.len().saturating_sub(COMMAND_SHOWN)..].join("\n");
                    self.pipeline_say(id, NoticeKind::Warning, format!("`{}` failed (exit {code}):\n{tail}", command.line));
                }
                None => {}
            }
            let Some(next) = self.pipelines.runs.get_mut(&id).map(|r| r.run.command_ended(exit, &output)) else { continue };
            self.pipeline_do(id, next);
        }
        !self.pipelines.commands.is_empty()
    }

    /// `SPC a k` while a command step runs on the thread in front: true
    /// when there was one to stop, which brings up its gate.
    pub(super) fn pipeline_cancel_command(&mut self) -> bool {
        let shown = self.agents.shown(&self.pages);
        let found = self.pipelines.commands.iter().find(|(id, _)| self.pipelines.runs.get(*id).is_some_and(|r| r.threads.is_empty() || shown.as_ref().is_some_and(|t| r.threads.contains(t))));
        let Some((_, command)) = found else { return false };
        command.runner.cancel();
        self.set_message(format!("stopping `{}`", command.line));
        true
    }

    fn pipeline_start_step(&mut self, id: u64, step: usize, prompt: String) {
        let Some(running) = self.pipelines.runs.get(&id) else { return };
        let spec = running.run.spec.steps[step].clone();
        let root = running.root.clone();
        let progress = running.run.progress();
        let entries = self.agents_configure();
        let agent = spec.agent.clone().filter(|a| !a.is_empty()).map(|a| fenix_acp::catalog::slug(&a)).or_else(|| self.agents_default_of(&entries));
        let Some(agent) = agent else {
            self.agents_none_found(&entries);
            return self.pipeline_stop(id, "no agent to run it");
        };
        let thread = self.agents.hub.new_thread(&agent, &root);
        let seq = self.pipelines.runs[&id].threads.len();
        let tag = fenix_acp::hub::RunTag { step, seq, step_name: spec.name.clone(), ..self.pipelines.runs[&id].tag.clone() };
        if let Some(meta) = self.agents.hub.meta_mut(&thread) {
            meta.run = Some(tag);
        }
        if let Some(t) = self.agents.hub.thread_mut(&thread) {
            t.notice(fenix_acp::thread::NoticeKind::Info, format!("Pipeline {}: {progress}", self.pipelines.runs[&id].run.spec.title));
        }
        // A read-only step ends at its plan: the gate decides, not a card.
        if spec.is_read_only() {
            self.agents.hub.stop_at_plans(&thread);
        }
        // The step's toolsets, and no others, for its session.
        if !spec.toolsets.is_empty() {
            let caller = fenix_control::AgentCaller { client: "fenix-mcp".into(), thread: Some(thread.0.clone()) };
            self.agent_set_session_toolsets(&caller, &spec.toolsets);
        }
        if let Some(r) = self.pipelines.runs.get_mut(&id) {
            r.threads.push(thread.clone());
        }
        self.pipelines.steps.insert(thread.clone(), id);
        self.pipelines.held.insert(thread.clone(), Held { run: id, prompt });
        self.agents_show(thread.clone());
        self.pipeline_changed(&thread);
    }

    /// Written to at its gate, the step is working again: the question
    /// waits for it, and what it comes up with is what's passed on.
    fn pipeline_reopen_gate(&mut self, thread: &ThreadId) {
        let Some(&id) = self.pipelines.steps.get(thread) else { return };
        // A turn under way, not only its agent starting: a step's thread
        // shown again after a restart attaches, and that's no round.
        if !self.agents.hub.view(thread).is_some_and(|v| !matches!(v.turn, fenix_acp::hub::Turn::Idle)) {
            return;
        }
        let Some(running) = self.pipelines.runs.get_mut(&id).filter(|r| r.threads.last() == Some(thread)) else { return };
        let State::AtGate(step) = running.run.state else { return };
        // A command's gate is on the step before it: writing there isn't
        // a round of the command.
        if running.run.spec.steps[step].run.is_some() {
            return;
        }
        let key = gate_key(id, step);
        if self.pipelines.gates.remove(&key).is_none() {
            return;
        }
        running.run.gate_reopened();
        self.pipelines.passing.remove(&id);
        if let Some(t) = self.agents.hub.thread_mut(thread) {
            t.taken_plan = None;
        }
        let fx = self.agents.hub.withdraw_fenix(thread, &key);
        self.agents_run(fx);
        self.pipeline_keep(id);
    }

    /// Whether `thread` is a step at its gate, its question up.
    pub(super) fn pipeline_at_gate(&self, thread: &ThreadId) -> bool {
        let Some(id) = self.pipelines.steps.get(thread) else { return false };
        self.pipelines.gates.values().any(|g| g == id) && self.pipelines.runs.get(id).is_some_and(|r| r.threads.last() == Some(thread))
    }

    /// A thread changed: a step's held prompt goes once its session is up,
    /// in the step's mode.
    pub(super) fn pipeline_changed(&mut self, thread: &ThreadId) {
        self.pipeline_reopen_gate(thread);
        let Some(view) = self.agents.hub.view(thread) else { return };
        if !matches!(view.link, fenix_acp::hub::Link::Live { .. }) {
            if let fenix_acp::hub::Link::ReadOnly(why) = view.link {
                let why = why.clone();
                if let Some(held) = self.pipelines.held.remove(thread) {
                    self.pipeline_stop(held.run, &format!("the agent can't take it ({why})"));
                }
            }
            return;
        }
        let Some(held) = self.pipelines.held.remove(thread) else { return };
        let Some(running) = self.pipelines.runs.get(&held.run) else { return };
        let step = running.run.step().map(|i| running.run.spec.steps[i].clone());
        let mut fx = Vec::new();
        let usual = running.usual_mode.clone();
        let is_plan = |id: &str| id.to_lowercase().contains("plan");
        if let (Some(wanted), Some(modes)) = (step.as_ref().and_then(|s| s.mode.clone()), view.thread.modes.as_ref()) {
            let wanted = wanted.to_lowercase();
            let found = modes.available.iter().find(|m| m.id.to_lowercase() == wanted || m.name.to_lowercase().contains(&wanted) || m.id.to_lowercase().contains(&wanted)).map(|m| m.id.clone());
            if let Some(mode) = found.filter(|m| *m != modes.current) {
                let current = modes.current.clone();
                fx.extend(self.agents.hub.set_mode(thread, &mode));
                if let Some(r) = self.pipelines.runs.get_mut(&held.run).filter(|r| r.usual_mode.is_none()) {
                    r.usual_mode = Some(current);
                }
            }
        } else if let (Some(modes), true) = (self.agents.hub.view(thread).and_then(|v| v.thread.modes.clone()), step.as_ref().is_some_and(|s| s.mode.is_none())) {
            // A step that's to do the work, in a session some agents start
            // in the mode the last one left off in: planning it would only
            // plan.
            if is_plan(&modes.current) {
                let back = usual.filter(|m| !is_plan(m)).or_else(|| modes.available.iter().map(|m| m.id.clone()).find(|m| !is_plan(m)));
                if let Some(mode) = back {
                    fx.extend(self.agents.hub.set_mode(thread, &mode));
                }
            }
        }
        fx.extend(self.agents.hub.prompt(thread, vec![ContentBlock::text(held.prompt)]));
        self.agents_run(fx);
    }

    /// The gate's text, edited: passed on, and the run goes on. `None`
    /// (the pane discarded): the run stops.
    pub(crate) fn pipeline_gate_edited(&mut self, run: u64, text: Option<String>) {
        let next = match self.pipelines.runs.get_mut(&run) {
            Some(r) => r.run.gate_answered(text.is_some(), text),
            None => return,
        };
        self.pipeline_do(run, next);
    }

    /// A step's line for the dock's title (`plan ✓ → build ● → review`),
    /// when `thread` is one.
    pub(crate) fn pipeline_strip(&self, thread: &ThreadId) -> Option<String> {
        let run = self.pipelines.steps.get(thread).and_then(|id| self.pipelines.runs.get(id))?;
        let stopped = if run.run.is_live() { "" } else { " (stopped)" };
        Some(format!("{}: {}{stopped}", run.run.spec.title, run.run.progress()))
    }

    /// `]s` / `[s`: the pipeline's next or previous step's thread.
    pub(crate) fn pipeline_other_step(&mut self, thread: &ThreadId, forward: bool) {
        let steps = self.pipelines.runs.values().map(|r| r.threads.clone()).chain(self.pipelines.ended.iter().cloned()).find(|t| t.contains(thread));
        let Some(steps) = steps else { return self.set_message("this thread isn't a pipeline's step") };
        let at = steps.iter().position(|t| t == thread).unwrap_or(0);
        let to = if forward { at + 1 } else { at.wrapping_sub(1) };
        match steps.get(to) {
            Some(t) => self.agents_show(t.clone()),
            None => self.set_message(if forward { "the last step" } else { "the first step" }),
        }
    }

    /// The diff of what a step's thread changed, from its ledger.
    fn pipeline_changes(&self, thread: &ThreadId) -> String {
        let mut out = String::new();
        for (path, before) in self.agents.baselines.get(thread).into_iter().flatten() {
            let now = self.agents_current(path).unwrap_or_default();
            let before = before.clone().unwrap_or_default();
            if now == before {
                continue;
            }
            out.push_str(&format!("--- {}\n", path.display()));
            for hunk in crate::agent_review::hunks(&before, &now) {
                out.push_str(&format!("@@ line {} @@\n", hunk.new.start + 1));
                for l in &hunk.removed {
                    out.push_str(&format!("-{l}\n"));
                }
                for l in &hunk.added {
                    out.push_str(&format!("+{l}\n"));
                }
            }
        }
        out
    }

    /// A step's turn ended: the run is told, and does what's next.
    pub(super) fn pipeline_turn_ended(&mut self, thread: &ThreadId, stop: &str) {
        let Some(&id) = self.pipelines.steps.get(thread) else { return };
        // An earlier step's thread, written to: the run has moved on. So
        // has it past the last one, while a command runs. A stopped run's
        // step, written to, is a thread like any other until it's picked up.
        if self.pipelines.runs.get(&id).is_some_and(|r| r.threads.last() != Some(thread) || r.run.at_command() || !matches!(r.run.state, State::Running(_))) {
            return;
        }
        let Some(view) = self.agents.hub.view(thread) else { return };
        // A plan the step offered is what it came up with: its last words
        // only lead up to offering it.
        let taken = view.thread.taken_plan.clone();
        let answer = taken.clone().unwrap_or_else(|| view.thread.last_answer());
        let transcript = view.thread.transcript();
        // The plan the agent put up for approval, else its plan entries.
        let plan = taken.or_else(|| view.thread.proposed_plan()).or_else(|| (!view.thread.plan.is_empty()).then(|| view.thread.plan.iter().enumerate().map(|(i, e)| format!("{}. {}", i + 1, e.content)).collect::<Vec<_>>().join("\n")));
        let Some(running) = self.pipelines.runs.get(&id) else { return };
        let read_only = running.run.step().is_some_and(|i| running.run.spec.steps[i].is_read_only());
        // What git saw changed, too, though no tool call said so.
        self.agents_turn_ended(thread);
        if read_only {
            let reverted = self.agents_ask_revert(thread);
            if !reverted.is_empty() {
                if let Some(t) = self.agents.hub.thread_mut(thread) {
                    t.notice(fenix_acp::thread::NoticeKind::Warning, format!("A read-only step changed {}; put back.", reverted.join(", ")));
                }
            }
        } else if let Some(mine) = self.agents.baselines.remove(thread) {
            // Into the pipeline's one ledger, the earliest text kept.
            let ledger = self.pipelines.runs.get_mut(&id).map(|r| r.ledger.get_or_insert_with(|| thread.clone()).clone()).unwrap();
            let into = self.agents.baselines.entry(ledger.clone()).or_default();
            for (path, before) in mine {
                into.entry(path).or_insert(before);
            }
            self.agents.dirty.insert(ledger);
            self.agents_refresh_marks();
        }
        let changes = self.pipelines.runs.get(&id).and_then(|r| r.ledger.clone()).map(|l| self.pipeline_changes(&l)).unwrap_or_default();
        let next = match self.pipelines.runs.get_mut(&id) {
            Some(r) => r.run.step_ended(StepEnd { stop: stop.to_string(), answer, plan, changes, transcript }),
            None => return,
        };
        // A review's findings go beside the hunks they're about.
        if let Some(r) = self.pipelines.runs.get(&id) {
            if let Some(ledger) = r.ledger.clone() {
                if r.run.findings.is_empty() {
                    self.pipelines.findings.remove(&ledger);
                } else {
                    self.pipelines.findings.insert(ledger, r.run.findings.clone());
                }
            }
        }
        self.pipeline_do(id, next);
    }

    /// The gate's question answered: true when it was a gate's.
    pub(super) fn pipeline_gate_answered(&mut self, key: &str, option: Option<&str>) -> bool {
        let Some(id) = self.pipelines.gates.remove(key) else { return false };
        // `e`: what's passed on, in a compose pane, sent on when it's sent.
        if option == Some("edit") {
            let passing = self.pipelines.passing.get(&id).cloned().unwrap_or_default();
            self.open_compose_seeded(ComposePurpose::PipelineGate { run: id }, &passing);
            return true;
        }
        // `r`: the step again, on a thread of its own, as it was asked.
        if option == Some("retry") {
            let Some(next) = self.pipelines.runs.get_mut(&id).map(|r| r.run.retry()) else { return true };
            self.pipeline_do(id, next);
            return true;
        }
        let go = matches!(option, Some("allow_once" | "allow_thread"));
        let next = match self.pipelines.runs.get_mut(&id) {
            Some(r) => r.run.gate_answered(go, None),
            None => return true,
        };
        self.pipeline_do(id, next);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An editor whose agents can't start: a step shown in the dock tries
    /// to start its agent, and a real one (`npx`, downloading) must never
    /// run in a test.
    fn app() -> App {
        let mut app = App::with_file(None);
        app.config.set_value("agents.servers", Some(fenix_config::Value::Map(vec![("claude".into(), "fenix-test-no-such-agent".into())])));
        app
    }

    #[test]
    fn the_built_in_pipeline_is_offered_and_a_project_one_too() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".fenix/pipelines")).unwrap();
        std::fs::write(dir.path().join(".fenix/pipelines/quick.toml"), "title = \"Quick\"\n[[steps]]\nname = \"do\"\nprompt = \"{input}\"\n").unwrap();
        std::fs::write(dir.path().join(".fenix/pipelines/broken.toml"), "steps = []\n").unwrap();
        let mut app = App::with_file(None);
        app.project_root = Some(dir.path().to_path_buf());
        let (specs, problems) = app.pipeline_specs();
        let names: Vec<&str> = specs.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["review", "fix", "tdd", "refactor", "audit", "quick"]);
        assert_eq!(problems.len(), 1, "{problems:?}");
    }

    #[test]
    fn an_armed_pipeline_takes_the_next_prompt_and_a_step_waits_for_its_session() {
        let mut app = app();
        assert!(!app.pipeline_take_prompt("nothing armed"));
        app.pipelines.armed = Some(PipelineSpec::parse("one", "[[steps]]\nname = \"do\"\nagent = \"claude\"\nprompt = \"Do: {input}\"\n").unwrap());
        assert!(app.pipeline_take_prompt("the thing"));
        let running = app.pipelines.runs.values().next().unwrap();
        assert_eq!(running.threads.len(), 1);
        let thread = running.threads[0].clone();
        assert_eq!(app.pipelines.held.get(&thread).map(|h| h.prompt.as_str()), Some("Do: the thing"), "held until the session is up");
        // The step ends: the run is done.
        app.pipeline_turn_ended(&thread, "end_turn");
        assert!(app.pipelines.runs.is_empty());
    }

    #[test]
    fn the_plan_a_step_offers_is_what_passes_on() {
        let mut app = app();
        app.pipelines.armed = Some(PipelineSpec::parse("p", "[[steps]]\nname = \"plan\"\nagent = \"claude\"\nmode = \"plan\"\nprompt = \"{input}\"\npasses = \"plan\"\ngate = true\n[[steps]]\nname = \"build\"\nagent = \"claude\"\nprompt = \"Do: {previous}\"\n").unwrap());
        app.pipeline_take_prompt("x");
        let thread = app.pipelines.runs.values().next().unwrap().threads[0].clone();
        app.pipelines.held.clear();
        app.agents.hub.thread_mut(&thread).unwrap().taken_plan = Some("1. Change a.rs".into());
        app.pipeline_turn_ended(&thread, "end_turn");
        let key = app.pipelines.gates.keys().next().cloned().expect("the gate is up");
        assert!(app.pipeline_gate_answered(&key, Some("allow_once")));
        let build = app.pipelines.runs.values().next().unwrap().threads[1].clone();
        assert_eq!(app.pipelines.held.get(&build).map(|h| h.prompt.as_str()), Some("Do: 1. Change a.rs"));
    }

    #[test]
    fn a_run_ends_on_the_thread_with_its_changes() {
        let mut app = app();
        app.pipelines.armed = Some(PipelineSpec::parse("p", "[[steps]]\nname = \"build\"\nagent = \"claude\"\nprompt = \"{input}\"\n[[steps]]\nname = \"review\"\nagent = \"claude\"\nmode = \"plan\"\nprompt = \"{previous}\"\n").unwrap());
        app.pipeline_take_prompt("x");
        let build = app.pipelines.runs.values().next().unwrap().threads[0].clone();
        app.pipelines.held.clear();
        app.agents.baselines.entry(build.clone()).or_default().insert(PathBuf::from("a.rs"), Some("before".into()));
        app.pipeline_turn_ended(&build, "end_turn");
        let review = app.pipelines.runs.values().next().unwrap().threads[1].clone();
        assert_eq!(app.agents.shown(&app.pages), Some(review.clone()));
        app.pipeline_turn_ended(&review, "end_turn");
        assert!(app.pipelines.runs.is_empty());
        assert_eq!(app.agents.shown(&app.pages), Some(build));
    }

    #[test]
    fn a_turn_on_an_earlier_steps_thread_leaves_the_run_alone() {
        let mut app = app();
        app.pipelines.armed = Some(PipelineSpec::parse("p", "[[steps]]\nname = \"one\"\nagent = \"claude\"\nprompt = \"{input}\"\n[[steps]]\nname = \"two\"\nagent = \"claude\"\nprompt = \"{previous}\"\n").unwrap());
        app.pipeline_take_prompt("x");
        let one = app.pipelines.runs.values().next().unwrap().threads[0].clone();
        app.pipelines.held.clear();
        app.pipeline_turn_ended(&one, "end_turn");
        app.pipeline_turn_ended(&one, "end_turn");
        assert_eq!(app.pipelines.runs.values().next().unwrap().run.state, fenix_acp::pipeline::State::Running(1));
    }

    #[test]
    fn a_build_that_changed_nothing_asks_before_review() {
        let mut app = app();
        let spec = "[[steps]]\nname = \"build\"\nagent = \"claude\"\nprompt = \"{input}\"\npasses = \"changes\"\n[[steps]]\nname = \"review\"\nagent = \"claude\"\nprompt = \"{previous}\"\n";
        app.pipelines.armed = Some(PipelineSpec::parse("p", spec).unwrap());
        app.pipeline_take_prompt("x");
        let build = app.pipelines.runs.values().next().unwrap().threads[0].clone();
        app.pipelines.held.clear();
        app.pipeline_turn_ended(&build, "end_turn");
        let run = &app.pipelines.runs.values().next().unwrap().run;
        assert_eq!(run.state, fenix_acp::pipeline::State::AtGate(0));
        assert_eq!(app.pipelines.runs.values().next().unwrap().threads.len(), 1, "review didn't start");
        let view = app.agents.hub.view(&build).unwrap();
        assert!(view.thread.entries.iter().any(|e| matches!(e, fenix_acp::thread::Entry::Permission(c) if c.tool_call.title.as_deref().is_some_and(|t| t.contains("build changed nothing")))));
    }

    #[test]
    fn a_gate_is_a_question_and_no_stops_the_run() {
        let mut app = app();
        app.pipelines.armed = Some(PipelineSpec::parse("g", "[[steps]]\nname = \"a\"\nagent = \"claude\"\nprompt = \"{input}\"\ngate = true\n[[steps]]\nname = \"b\"\nagent = \"claude\"\nprompt = \"{previous}\"\n").unwrap());
        app.pipeline_take_prompt("x");
        let thread = app.pipelines.runs.values().next().unwrap().threads[0].clone();
        app.pipelines.held.clear();
        app.pipeline_turn_ended(&thread, "end_turn");
        let key = app.pipelines.gates.keys().next().cloned().expect("a gate is up");
        assert_eq!(app.agents.hub.status().waiting, 1);
        assert!(app.pipeline_gate_answered(&key, Some("reject_once")));
        assert!(!app.pipeline_is_live(&app.pipelines.runs.values().next().unwrap().tag.id), "stopped, and kept");
        assert!(app.pipelines.gates.is_empty());
        assert!(app.pipeline_strip(&thread).is_some_and(|s| s.contains("a ■") && s.ends_with("(stopped)")));
        let notice = format!("{:?}", app.agents.hub.view(&thread).unwrap().thread.entries);
        assert!(notice.contains("stopped at a's gate -- SPC a g picks it up"), "{notice}");
        assert!(!app.pipeline_gate_answered("someone-elses", None));
        // SPC a g: the gate back, as it was; y goes on.
        app.agents_show(thread.clone());
        app.pipeline_go();
        let key = app.pipelines.gates.keys().next().cloned().expect("the gate is back");
        assert!(app.pipeline_at_gate(&thread));
        assert!(app.pipeline_gate_answered(&key, Some("allow_once")));
        assert_eq!(app.pipelines.runs.values().next().unwrap().threads.len(), 2, "on to b");
    }

    /// Refusing a command the plan step asked for ends its turn
    /// `cancelled`: the run is still there, its gate up, and `y` starts
    /// the next step with the plan.
    #[test]
    fn a_step_that_ends_cancelled_asks_at_its_gate() {
        let mut app = app();
        app.pipelines.armed = Some(PipelineSpec::parse("p", "title = \"P\"\n[[steps]]\nname = \"plan\"\nagent = \"claude\"\nmode = \"plan\"\nprompt = \"{input}\"\npasses = \"plan\"\ngate = true\n[[steps]]\nname = \"build\"\nagent = \"claude\"\nprompt = \"Do: {previous}\"\n").unwrap());
        app.pipeline_take_prompt("x");
        let thread = app.pipelines.runs.values().next().unwrap().threads[0].clone();
        app.pipelines.held.clear();
        app.agents.hub.thread_mut(&thread).unwrap().taken_plan = Some("1. Change a.rs".into());
        app.pipeline_turn_ended(&thread, "cancelled");
        assert_eq!(app.pipelines.runs.len(), 1, "the run is still there");
        let key = app.pipelines.gates.keys().next().cloned().expect("a gate is up");
        assert!(app.pipeline_at_gate(&thread));
        assert_eq!(app.pipeline_strip(&thread).as_deref(), Some("P: plan ● → build"));
        let card = app.agents.hub.view(&thread).unwrap().thread.entries.iter().rev().find_map(|e| if let fenix_acp::thread::Entry::Permission(c) = e { Some(c.clone()) } else { None }).unwrap();
        assert!(card.tool_call.title.as_deref().is_some_and(|t| t.contains("plan stopped early (cancelled)")), "{:?}", card.tool_call.title);
        assert!(app.pipeline_gate_answered(&key, Some("allow_once")));
        let build = app.pipelines.runs.values().next().unwrap().threads[1].clone();
        assert_eq!(app.pipelines.held.get(&build).map(|h| h.prompt.as_str()), Some("Do: 1. Change a.rs"));
    }

    /// `SPC a k` on a step before it said anything: its gate asks, with
    /// nothing to pass on, so only `r` and `n`; `r` runs the step again on
    /// a thread of its own with the prompt it had.
    #[test]
    fn a_step_stopped_with_nothing_to_pass_runs_again_with_r() {
        let mut app = app();
        app.pipelines.armed = Some(PipelineSpec::parse("p", "[[steps]]\nname = \"plan\"\nagent = \"claude\"\nprompt = \"Plan: {input}\"\n[[steps]]\nname = \"build\"\nagent = \"claude\"\nprompt = \"{previous}\"\n").unwrap());
        app.pipeline_take_prompt("x");
        let first = app.pipelines.runs.values().next().unwrap().threads[0].clone();
        app.pipelines.held.clear();
        app.pipeline_turn_ended(&first, "cancelled");
        let key = app.pipelines.gates.keys().next().cloned().expect("a gate is up");
        let card = app.agents.hub.view(&first).unwrap().thread.entries.iter().rev().find_map(|e| if let fenix_acp::thread::Entry::Permission(c) = e { Some(c.clone()) } else { None }).unwrap();
        let ids: Vec<&str> = card.options.iter().map(|o| o.id.as_str()).collect();
        assert_eq!(ids, ["retry", "reject_once"], "nothing to go on with");
        assert_eq!(crate::agent_page::answer_for(app.agents.hub.view(&first).unwrap().thread, crate::page::Key::Char('r')), Some(crate::agent_page::Action::Permission { key: key.clone(), option: "retry".into() }));
        // SPC a g says why it can't go on.
        app.agents_show(first.clone());
        app.pipeline_go();
        assert!(app.status_message.as_ref().is_some_and(|m| m.text.contains("nothing to pass on")));
        assert!(app.pipeline_gate_answered(&key, Some("retry")));
        let run = app.pipelines.runs.values().next().unwrap();
        assert_eq!(run.threads.len(), 2, "a thread of its own");
        assert_eq!(app.pipelines.held.get(&run.threads[1]).map(|h| h.prompt.as_str()), Some("Plan: x"));
        assert_eq!(app.agents.hub.view(&run.threads[1]).unwrap().meta.run.as_ref().map(|t| (t.step, t.seq)), Some((0, 1)), "the same step, its second round");
    }

    #[test]
    fn e_at_a_gate_edits_what_goes_on_and_the_strip_says_where_it_is() {
        let mut app = app();
        app.pipelines.armed = Some(PipelineSpec::parse("g", "title = \"Two\"\n[[steps]]\nname = \"a\"\nagent = \"claude\"\nprompt = \"{input}\"\ngate = true\n[[steps]]\nname = \"b\"\nagent = \"claude\"\nprompt = \"Do: {previous}\"\n").unwrap());
        app.pipeline_take_prompt("x");
        let first = app.pipelines.runs.values().next().unwrap().threads[0].clone();
        assert_eq!(app.pipeline_strip(&first).as_deref(), Some("Two: a ● → b"));
        app.pipelines.held.clear();
        app.pipeline_turn_ended(&first, "end_turn");
        let key = app.pipelines.gates.keys().next().cloned().unwrap();
        let card = app.agents.hub.view(&first).unwrap().thread.entries.iter().rev().find_map(|e| if let fenix_acp::thread::Entry::Permission(c) = e { Some(c.clone()) } else { None }).unwrap();
        assert!(card.options.iter().any(|o| o.id == "edit"), "e edits");
        assert_eq!(crate::agent_page::answer_for(app.agents.hub.view(&first).unwrap().thread, crate::page::Key::Char('e')), Some(crate::agent_page::Action::Permission { key: key.clone(), option: "edit".into() }));
        assert!(app.pipeline_gate_answered(&key, Some("edit")));
        let compose = app.compose.as_ref().map(|c| c.buffer).expect("the passing text, to edit");
        app.replace_buffer_from_disk(compose, "the edited plan");
        app.compose_submit();
        let run = app.pipelines.runs.values().next().unwrap();
        assert_eq!(run.threads.len(), 2, "the next step started");
        let second = run.threads[1].clone();
        assert_eq!(app.pipelines.held.get(&second).map(|h| h.prompt.as_str()), Some("Do: the edited plan"));
        assert_eq!(app.pipeline_strip(&second).as_deref(), Some("Two: a ✓ → b ●"));
        // [s from the second step shows the first.
        app.pipeline_other_step(&second, false);
        assert_eq!(app.agents.shown(&app.pages), Some(first));
    }
}
