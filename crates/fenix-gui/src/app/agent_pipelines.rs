//! Pipelines in the editor (`SPC a P`): each step of a
//! `fenix_acp::pipeline::Run` is a thread of its own, shown in the dock
//! as it runs. Its prompt waits until its session is up, goes out in the
//! step's mode with its toolsets, and its turn's end moves the run on.
//! What the steps change goes into one ledger -- the first thread's -- so
//! you review the outcome once (`SPC a r`). A read-only step's changes
//! are put back. A gate is a question in the step's thread: `y` goes on,
//! `n` stops.

use fenix_acp::hub::ThreadId;
use fenix_acp::pipeline::{Next, PipelineSpec, Run, StepEnd};
use fenix_acp::protocol::ContentBlock;
use serde_json::json;

use super::agents::Pick;
use super::*;

/// A pipeline under way.
pub(crate) struct Running {
    pub run: Run,
    pub root: PathBuf,
    /// Each step's thread, in the order they ran.
    pub threads: Vec<ThreadId>,
    /// The thread whose ledger holds every step's changes.
    pub ledger: Option<ThreadId>,
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
}

impl App {
    /// The pipelines there are: Fenix's `review`, then yours
    /// (`pipelines/*.toml` in the settings folder), then the project's
    /// (`.fenix/pipelines/*.toml`), a later one replacing an earlier of
    /// the same name.
    pub(super) fn pipeline_specs(&self) -> (Vec<PipelineSpec>, Vec<String>) {
        let mut specs = vec![PipelineSpec::review()];
        let mut problems = Vec::new();
        let mut dirs: Vec<PathBuf> = fenix_storage::paths::Roots::current()
            .map(|r| r.settings_file().with_file_name("pipelines"))
            .into_iter()
            .collect();
        dirs.push(self.agents_root().join(".fenix").join("pipelines"));
        for dir in dirs {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            let mut files: Vec<PathBuf> = entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x == "toml"))
                .collect();
            files.sort();
            for file in files {
                let name = file
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                match std::fs::read_to_string(&file)
                    .map_err(|e| e.to_string())
                    .and_then(|text| PipelineSpec::parse(&name, &text))
                {
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
        let candidates = specs
            .into_iter()
            .map(|s| {
                fenix_picker::Candidate::new(
                    format!(
                        "{}   {}",
                        s.title,
                        s.steps
                            .iter()
                            .map(|st| st.name.as_str())
                            .collect::<Vec<_>>()
                            .join(" → ")
                    ),
                    Pick::Pipeline(s),
                )
            })
            .collect();
        self.enter_picker(ActivePicker::Agents(fenix_picker::PickerState::new(
            candidates,
        )));
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
        self.set_message(format!(
            "{title}: write what it's to do, and send it to start"
        ));
    }

    /// Starts the armed pipeline on `input`, when one is: true when it
    /// took the prompt.
    pub(super) fn pipeline_take_prompt(&mut self, input: &str) -> bool {
        let Some(spec) = self.pipelines.armed.take() else {
            return false;
        };
        let (run, next) = Run::start(spec, input);
        self.pipelines.next += 1;
        let id = self.pipelines.next;
        let root = self.agents_root();
        self.pipelines.runs.insert(
            id,
            Running {
                run,
                root,
                threads: Vec::new(),
                ledger: None,
            },
        );
        self.pipeline_do(id, next);
        true
    }

    fn pipeline_do(&mut self, id: u64, next: Next) {
        match next {
            Next::Start { step, prompt } => self.pipeline_start_step(id, step, prompt),
            Next::Gate { step, passing } => {
                let Some(thread) = self
                    .pipelines
                    .runs
                    .get(&id)
                    .and_then(|r| r.threads.last().cloned())
                else {
                    return;
                };
                let name = self.pipelines.runs[&id].run.spec.steps[step].name.clone();
                let next = self.pipelines.runs[&id]
                    .run
                    .spec
                    .steps
                    .get(step + 1)
                    .map(|s| s.name.clone())
                    .unwrap_or_default();
                let key = format!("pipeline-gate-{id}-{step}");
                let answers = [
                    (
                        "allow_once",
                        "Go on",
                        fenix_acp::protocol::PermissionKind::AllowOnce,
                    ),
                    (
                        "edit",
                        "Edit what's passed on, then go on",
                        fenix_acp::protocol::PermissionKind::Other,
                    ),
                    (
                        "reject_once",
                        "Stop",
                        fenix_acp::protocol::PermissionKind::RejectOnce,
                    ),
                ];
                let fx = self.agents.hub.ask_fenix_with(
                    &thread,
                    &key,
                    &format!("Pipeline: pass what {name} gave on to {next}?"),
                    fenix_acp::protocol::ToolKind::Other,
                    json!({ "passing": passing }),
                    &answers,
                );
                self.pipelines.passing.insert(id, passing);
                self.pipelines.gates.insert(key, id);
                self.agents_run(fx);
            }
            Next::Done { summary } => self.pipeline_finished(id, &format!("done: {summary}")),
            Next::Stopped { why } => self.pipeline_finished(id, &format!("stopped: {why}")),
        }
    }

    fn pipeline_finished(&mut self, id: u64, how: &str) {
        let Some(running) = self.pipelines.runs.remove(&id) else {
            return;
        };
        self.pipelines.passing.remove(&id);
        self.pipelines.ended.push(running.threads.clone());
        for thread in &running.threads {
            self.pipelines.steps.remove(thread);
            self.pipelines.held.remove(thread);
        }
        let review = if running.ledger.is_some() {
            " -- SPC a r reviews what it changed"
        } else {
            ""
        };
        self.set_message(format!("{}: {how}{review}", running.run.spec.title));
        if let Some(thread) = running
            .ledger
            .clone()
            .or_else(|| running.threads.last().cloned())
        {
            if let Some(t) = self.agents.hub.thread_mut(&thread) {
                t.notice(
                    fenix_acp::thread::NoticeKind::Info,
                    format!("Pipeline {}: {how}", running.run.spec.title),
                );
            }
            self.agents_run(vec![fenix_acp::hub::Effect::Changed(thread)]);
        }
    }

    fn pipeline_start_step(&mut self, id: u64, step: usize, prompt: String) {
        let Some(running) = self.pipelines.runs.get(&id) else {
            return;
        };
        let spec = running.run.spec.steps[step].clone();
        let root = running.root.clone();
        let progress = running.run.progress();
        let entries = self.agents_configure();
        let agent = spec
            .agent
            .clone()
            .filter(|a| !a.is_empty())
            .map(|a| fenix_acp::catalog::slug(&a))
            .or_else(|| self.agents_default_of(&entries));
        let Some(agent) = agent else {
            self.agents_none_found(&entries);
            return self.pipeline_finished(id, "stopped: no agent to run it");
        };
        let thread = self.agents.hub.new_thread(&agent, &root);
        if let Some(t) = self.agents.hub.thread_mut(&thread) {
            t.notice(
                fenix_acp::thread::NoticeKind::Info,
                format!(
                    "Pipeline {}: {progress}",
                    self.pipelines.runs[&id].run.spec.title
                ),
            );
        }
        // The step's toolsets, and no others, for its session.
        if !spec.toolsets.is_empty() {
            let caller = fenix_control::AgentCaller {
                client: "fenix-mcp".into(),
                thread: Some(thread.0.clone()),
            };
            self.agent_set_session_toolsets(&caller, &spec.toolsets);
        }
        if let Some(r) = self.pipelines.runs.get_mut(&id) {
            r.threads.push(thread.clone());
        }
        self.pipelines.steps.insert(thread.clone(), id);
        self.pipelines
            .held
            .insert(thread.clone(), Held { run: id, prompt });
        self.agents_show(thread.clone());
        self.pipeline_changed(&thread);
    }

    /// A thread changed: a step's held prompt goes once its session is up,
    /// in the step's mode.
    pub(super) fn pipeline_changed(&mut self, thread: &ThreadId) {
        let Some(view) = self.agents.hub.view(thread) else {
            return;
        };
        if !matches!(view.link, fenix_acp::hub::Link::Live { .. }) {
            if let fenix_acp::hub::Link::ReadOnly(why) = view.link {
                let why = why.clone();
                if let Some(held) = self.pipelines.held.remove(thread) {
                    self.pipeline_finished(
                        held.run,
                        &format!("stopped: the agent can't take it ({why})"),
                    );
                }
            }
            return;
        }
        let Some(held) = self.pipelines.held.remove(thread) else {
            return;
        };
        let Some(running) = self.pipelines.runs.get(&held.run) else {
            return;
        };
        let step = running
            .run
            .step()
            .map(|i| running.run.spec.steps[i].clone());
        let mut fx = Vec::new();
        if let (Some(wanted), Some(modes)) = (
            step.as_ref().and_then(|s| s.mode.clone()),
            view.thread.modes.as_ref(),
        ) {
            let wanted = wanted.to_lowercase();
            let found = modes
                .available
                .iter()
                .find(|m| {
                    m.id.to_lowercase() == wanted
                        || m.name.to_lowercase().contains(&wanted)
                        || m.id.to_lowercase().contains(&wanted)
                })
                .map(|m| m.id.clone());
            if let Some(mode) = found.filter(|m| *m != modes.current) {
                fx.extend(self.agents.hub.set_mode(thread, &mode));
            }
        }
        fx.extend(
            self.agents
                .hub
                .prompt(thread, vec![ContentBlock::text(held.prompt)]),
        );
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
        let run = self
            .pipelines
            .steps
            .get(thread)
            .and_then(|id| self.pipelines.runs.get(id))?;
        Some(format!("{}: {}", run.run.spec.title, run.run.progress()))
    }

    /// `]s` / `[s`: the pipeline's next or previous step's thread.
    pub(crate) fn pipeline_other_step(&mut self, thread: &ThreadId, forward: bool) {
        let steps = self
            .pipelines
            .runs
            .values()
            .map(|r| r.threads.clone())
            .chain(self.pipelines.ended.iter().cloned())
            .find(|t| t.contains(thread));
        let Some(steps) = steps else {
            return self.set_message("this thread isn't a pipeline's step");
        };
        let at = steps.iter().position(|t| t == thread).unwrap_or(0);
        let to = if forward { at + 1 } else { at.wrapping_sub(1) };
        match steps.get(to) {
            Some(t) => self.agents_show(t.clone()),
            None => self.set_message(if forward {
                "the last step"
            } else {
                "the first step"
            }),
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
        let Some(&id) = self.pipelines.steps.get(thread) else {
            return;
        };
        let Some(view) = self.agents.hub.view(thread) else {
            return;
        };
        let answer = view.thread.last_answer();
        let transcript = view.thread.transcript();
        // The plan the agent put up for approval, else its plan entries.
        let plan = view.thread.proposed_plan().or_else(|| {
            (!view.thread.plan.is_empty()).then(|| {
                view.thread
                    .plan
                    .iter()
                    .enumerate()
                    .map(|(i, e)| format!("{}. {}", i + 1, e.content))
                    .collect::<Vec<_>>()
                    .join("\n")
            })
        });
        let Some(running) = self.pipelines.runs.get(&id) else {
            return;
        };
        let read_only = running
            .run
            .step()
            .is_some_and(|i| running.run.spec.steps[i].is_read_only());
        // What git saw changed, too, though no tool call said so.
        self.agents_turn_ended(thread);
        if read_only {
            let reverted = self.agents_ask_revert(thread);
            if !reverted.is_empty() {
                if let Some(t) = self.agents.hub.thread_mut(thread) {
                    t.notice(
                        fenix_acp::thread::NoticeKind::Warning,
                        format!(
                            "A read-only step changed {}; put back.",
                            reverted.join(", ")
                        ),
                    );
                }
            }
        } else if let Some(mine) = self.agents.baselines.remove(thread) {
            // Into the pipeline's one ledger, the earliest text kept.
            let ledger = self
                .pipelines
                .runs
                .get_mut(&id)
                .map(|r| r.ledger.get_or_insert_with(|| thread.clone()).clone())
                .unwrap();
            let into = self.agents.baselines.entry(ledger.clone()).or_default();
            for (path, before) in mine {
                into.entry(path).or_insert(before);
            }
            self.agents.dirty.insert(ledger);
            self.agents_refresh_marks();
        }
        let changes = self
            .pipelines
            .runs
            .get(&id)
            .and_then(|r| r.ledger.clone())
            .map(|l| self.pipeline_changes(&l))
            .unwrap_or_default();
        let next = match self.pipelines.runs.get_mut(&id) {
            Some(r) => r.run.step_ended(StepEnd {
                stop: stop.to_string(),
                answer,
                plan,
                changes,
                transcript,
            }),
            None => return,
        };
        // A review's findings go beside the hunks they're about.
        if let Some(r) = self.pipelines.runs.get(&id) {
            if let Some(ledger) = r.ledger.clone() {
                if r.run.findings.is_empty() {
                    self.pipelines.findings.remove(&ledger);
                } else {
                    self.pipelines
                        .findings
                        .insert(ledger, r.run.findings.clone());
                }
            }
        }
        self.pipeline_do(id, next);
    }

    /// The gate's question answered: true when it was a gate's.
    pub(super) fn pipeline_gate_answered(&mut self, key: &str, option: Option<&str>) -> bool {
        let Some(id) = self.pipelines.gates.remove(key) else {
            return false;
        };
        // `e`: what's passed on, in a compose pane, sent on when it's sent.
        if option == Some("edit") {
            let passing = self.pipelines.passing.get(&id).cloned().unwrap_or_default();
            self.open_compose_seeded(ComposePurpose::PipelineGate { run: id }, &passing);
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
        app.config.set_value(
            "agents.servers",
            Some(fenix_config::Value::Map(vec![(
                "claude".into(),
                "fenix-test-no-such-agent".into(),
            )])),
        );
        app
    }

    #[test]
    fn the_built_in_pipeline_is_offered_and_a_project_one_too() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".fenix/pipelines")).unwrap();
        std::fs::write(
            dir.path().join(".fenix/pipelines/quick.toml"),
            "title = \"Quick\"\n[[steps]]\nname = \"do\"\nprompt = \"{input}\"\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join(".fenix/pipelines/broken.toml"),
            "steps = []\n",
        )
        .unwrap();
        let mut app = App::with_file(None);
        app.project_root = Some(dir.path().to_path_buf());
        let (specs, problems) = app.pipeline_specs();
        let names: Vec<&str> = specs.iter().map(|s| s.name.as_str()).collect();
        assert!(
            names.contains(&"review") && names.contains(&"quick"),
            "{names:?}"
        );
        assert_eq!(problems.len(), 1, "{problems:?}");
    }

    #[test]
    fn an_armed_pipeline_takes_the_next_prompt_and_a_step_waits_for_its_session() {
        let mut app = app();
        assert!(!app.pipeline_take_prompt("nothing armed"));
        app.pipelines.armed = Some(
            PipelineSpec::parse(
                "one",
                "[[steps]]\nname = \"do\"\nagent = \"claude\"\nprompt = \"Do: {input}\"\n",
            )
            .unwrap(),
        );
        assert!(app.pipeline_take_prompt("the thing"));
        let running = app.pipelines.runs.values().next().unwrap();
        assert_eq!(running.threads.len(), 1);
        let thread = running.threads[0].clone();
        assert_eq!(
            app.pipelines.held.get(&thread).map(|h| h.prompt.as_str()),
            Some("Do: the thing"),
            "held until the session is up"
        );
        // The step ends: the run is done.
        app.pipeline_turn_ended(&thread, "end_turn");
        assert!(app.pipelines.runs.is_empty());
    }

    #[test]
    fn a_gate_is_a_question_and_no_stops_the_run() {
        let mut app = app();
        app.pipelines.armed = Some(PipelineSpec::parse("g", "[[steps]]\nname = \"a\"\nagent = \"claude\"\nprompt = \"{input}\"\ngate = true\n[[steps]]\nname = \"b\"\nagent = \"claude\"\nprompt = \"{previous}\"\n").unwrap());
        app.pipeline_take_prompt("x");
        let thread = app.pipelines.runs.values().next().unwrap().threads[0].clone();
        app.pipelines.held.clear();
        app.pipeline_turn_ended(&thread, "end_turn");
        let key = app
            .pipelines
            .gates
            .keys()
            .next()
            .cloned()
            .expect("a gate is up");
        assert_eq!(app.agents.hub.status().waiting, 1);
        assert!(app.pipeline_gate_answered(&key, Some("reject_once")));
        assert!(app.pipelines.runs.is_empty(), "stopped");
        assert!(!app.pipeline_gate_answered("someone-elses", None));
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
        let card = app
            .agents
            .hub
            .view(&first)
            .unwrap()
            .thread
            .entries
            .iter()
            .rev()
            .find_map(|e| {
                if let fenix_acp::thread::Entry::Permission(c) = e {
                    Some(c.clone())
                } else {
                    None
                }
            })
            .unwrap();
        assert!(card.options.iter().any(|o| o.id == "edit"), "e edits");
        assert_eq!(
            crate::agent_page::answer_for(
                app.agents.hub.view(&first).unwrap().thread,
                crate::page::Key::Char('e')
            ),
            Some(crate::agent_page::Action::Permission {
                key: key.clone(),
                option: "edit".into()
            })
        );
        assert!(app.pipeline_gate_answered(&key, Some("edit")));
        let compose = app
            .compose
            .as_ref()
            .map(|c| c.buffer)
            .expect("the passing text, to edit");
        app.replace_buffer_from_disk(compose, "the edited plan");
        app.compose_submit();
        let run = app.pipelines.runs.values().next().unwrap();
        assert_eq!(run.threads.len(), 2, "the next step started");
        let second = run.threads[1].clone();
        assert_eq!(
            app.pipelines.held.get(&second).map(|h| h.prompt.as_str()),
            Some("Do: the edited plan")
        );
        assert_eq!(
            app.pipeline_strip(&second).as_deref(),
            Some("Two: a ✓ → b ●")
        );
        // [s from the second step shows the first.
        app.pipeline_other_step(&second, false);
        assert_eq!(app.agents.shown(&app.pages), Some(first));
    }
}
