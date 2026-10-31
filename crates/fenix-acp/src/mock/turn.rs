//! One turn: the prompt's script played as Claude's adapter would send it,
//! on a worker thread of its own. Tool calls go `pending` (with a partial
//! `rawInput`), then the full input, then permission if the mode asks,
//! then `in_progress`, then `completed` or `failed`. A cancel ends the
//! turn at the next step, chunk or wait: a pending permission is withdrawn
//! (`$/cancel_request`) and counts as a denial, a running tool ends
//! `failed`, and the answer is `cancelled`.

use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};

use super::config;
use super::script::{self, Step};
use super::{lock, Agent, CallError, TurnCtl};

/// The context window the usage updates report.
const CONTEXT_WINDOW: u64 = 200_000;

pub(super) fn run(
    agent: Arc<Agent>,
    session: String,
    ctl: Arc<TurnCtl>,
    prompt: Vec<Value>,
    queue: Arc<Mutex<()>>,
) {
    let _turn = lock(&queue);
    let cwd = agent.sessions().get(&session).map(|s| s.record.cwd.clone());
    let _ = match cwd {
        None => agent.answer_ended(&ctl.request),
        Some(cwd) => {
            let mut turn = Turn {
                agent: agent.clone(),
                session: session.clone(),
                ctl: ctl.clone(),
                cwd,
                record: Vec::new(),
                output: 0,
            };
            turn.play(&prompt)
                .and_then(|stop| turn.finish(&prompt, stop))
        }
    };
    agent.turn_ended(&session, &ctl);
}

/// How a step leaves the turn.
enum Flow {
    Next,
    Stop(&'static str),
}

/// A permission request's outcome.
enum Answer {
    Chose(String),
    /// The client chose a reject option, or answered `cancelled` on its own.
    Refused,
    /// The turn was cancelled while asking.
    Cancelled,
}

/// What the session's mode says about a tool.
enum Gate {
    Allow,
    Ask,
    /// Plan mode: edits are refused until the plan is approved.
    Refuse,
}

struct Turn {
    agent: Arc<Agent>,
    session: String,
    ctl: Arc<TurnCtl>,
    cwd: PathBuf,
    /// The updates sent that a load replays.
    record: Vec<Value>,
    /// Characters of agent text, for the usage figures.
    output: usize,
}

impl Turn {
    fn play(&mut self, prompt: &[Value]) -> io::Result<&'static str> {
        let text = prompt
            .iter()
            .find(|block| block["type"] == "text")
            .and_then(|block| block["text"].as_str())
            .unwrap_or("");
        let first = self
            .agent
            .sessions()
            .get(&self.session)
            .is_some_and(|s| s.record.history.is_empty() && s.record.title.is_none());
        if first {
            let title = title_of(text);
            if let Some(session) = self.agent.sessions().get_mut(&self.session) {
                session.record.title = Some(title.clone());
            }
            let stamp = self.agent.stamp();
            self.send(json!({"sessionUpdate": "session_info_update", "title": title, "updatedAt": super::store::iso(stamp)}))?;
        }
        for step in script::parse(text) {
            if self.cancelled() {
                return Ok("cancelled");
            }
            if let Flow::Stop(reason) = self.step(step)? {
                return Ok(reason);
            }
        }
        Ok(if self.cancelled() {
            "cancelled"
        } else {
            "end_turn"
        })
    }

    /// Reports usage, stores the turn, and answers the prompt.
    fn finish(&mut self, prompt: &[Value], stop: &'static str) -> io::Result<()> {
        let prompt_chars: usize = prompt.iter().map(|block| block.to_string().len()).sum();
        let input = (prompt_chars / 4 + 1200) as u64;
        let output = (self.output / 4 + 20) as u64;
        let mut user: Vec<Value> = Vec::new();
        let message_id = self.agent.uuid();
        for block in prompt {
            user.push(json!({"sessionUpdate": "user_message_chunk", "content": block, "messageId": message_id}));
        }
        let stored = {
            let mut sessions = self.agent.sessions();
            sessions.get_mut(&self.session).map(|session| {
                let record = &mut session.record;
                record.history.append(&mut user);
                record.history.append(&mut self.record);
                record.input_tokens += input;
                record.output_tokens += output;
                record.cost += (input as f64 * 3.0 + output as f64 * 15.0) / 1e6;
                let stamp = self.agent.stamp();
                record.updated_nanos = stamp;
                record.updated_at = super::store::iso(stamp);
                record.clone()
            })
        };
        if let Some(record) = &stored {
            self.agent.store().save(record);
            let used = (record.input_tokens + record.output_tokens).min(CONTEXT_WINDOW);
            self.agent.update(
                &self.session,
                json!({"sessionUpdate": "usage_update", "used": used, "size": CONTEXT_WINDOW,
                       "cost": {"amount": (record.cost * 1e6).round() / 1e6, "currency": "USD"}}),
            )?;
        }
        self.agent.answer(
            &self.ctl.request,
            json!({"stopReason": stop, "usage": {
                "inputTokens": input, "outputTokens": output, "cachedReadTokens": 0, "cachedWriteTokens": 0,
                "totalTokens": input + output,
            }}),
        )
    }

    fn step(&mut self, step: Step) -> io::Result<Flow> {
        match step {
            Step::Hello => {
                self.think(&[
                    "The user is greeting me.",
                    " I'll say hello back and offer help.",
                ])?;
                self.say(&["Hello", "! How can I", " help you", " today?"])?;
                Ok(Flow::Next)
            }
            Step::Echo(text) => {
                self.say(&["You said: ", &text])?;
                Ok(Flow::Next)
            }
            Step::Read(path) => self.read(&path),
            Step::Edit { path, old, new } => self.edit(&path, &old, &new),
            Step::Write { path, content } => self.write(&path, &content),
            Step::Run(command) => self.bash(&command),
            Step::Todo => {
                let entries = json!([
                    {"content": "Read the code", "status": "completed", "priority": "medium"},
                    {"content": "Making the change", "status": "in_progress", "priority": "medium"},
                    {"content": "Run the tests", "status": "pending", "priority": "medium"},
                ]);
                self.send(json!({"sessionUpdate": "plan", "entries": entries}))?;
                self.say(&["I've made a plan."])?;
                Ok(Flow::Next)
            }
            Step::Plan => self.exit_plan(),
            Step::Slow => {
                self.think(&["This will take a while."])?;
                let id = self.agent.uuid();
                for i in 0..60 {
                    if !self.pause(50) {
                        return Ok(Flow::Stop("cancelled"));
                    }
                    self.chunk("agent_message_chunk", &id, &format!("word{i} "))?;
                }
                Ok(Flow::Next)
            }
            Step::Fail => {
                let id = self.tool_id();
                let path = self.resolve("does-not-exist.txt");
                let title = format!("Read {}", self.display(&path));
                let locations = json!([{"path": path, "line": 1}]);
                self.tool_call(
                    &id,
                    "Read",
                    &title,
                    "read",
                    json!({"file_path": path}),
                    json!([]),
                    locations,
                )?;
                self.fail(
                    &id,
                    "Read",
                    "<tool_use_error>File does not exist.</tool_use_error>",
                )?;
                self.say(&["That file doesn't exist."])?;
                Ok(Flow::Next)
            }
            Step::Refuse => {
                self.say(&["I can't help with that."])?;
                Ok(Flow::Stop("refusal"))
            }
            Step::Long => {
                for _ in 0..5 {
                    self.say(&[
                        "This answer goes on and on, ",
                        "until it runs out of tokens. ",
                    ])?;
                }
                Ok(Flow::Stop("max_tokens"))
            }
        }
    }

    // --- Tools -----------------------------------------------------------

    fn read(&mut self, path: &str) -> io::Result<Flow> {
        let id = self.tool_id();
        let abs = self.resolve(path);
        let title = format!("Read {}", self.display(&abs));
        let locations = json!([{"path": abs, "line": 1}]);
        self.tool_call(
            &id,
            "Read",
            &title,
            "read",
            json!({"file_path": abs}),
            json!([]),
            locations.clone(),
        )?;
        self.tool_update(&id, "Read", json!({"rawInput": {"file_path": abs}, "title": title, "kind": "read", "locations": locations}))?;
        self.tool_update(&id, "Read", json!({"status": "in_progress"}))?;
        match self.read_file(&abs)? {
            Err(Interrupted) => self.interrupted(&id, "Read"),
            Ok(Err(why)) => {
                self.fail(&id, "Read", &why)?;
                Ok(Flow::Next)
            }
            Ok(Ok(text)) => {
                let numbered: Vec<String> = text
                    .lines()
                    .enumerate()
                    .map(|(i, line)| format!("{}\t{line}", i + 1))
                    .collect();
                let content = json!([{"type": "content", "content": {"type": "text", "text": markdown_escape(&numbered.join("\n"))}}]);
                self.tool_update(
                    &id,
                    "Read",
                    json!({"status": "completed", "content": content}),
                )?;
                Ok(Flow::Next)
            }
        }
    }

    fn edit(&mut self, path: &str, old: &str, new: &str) -> io::Result<Flow> {
        let id = self.tool_id();
        let abs = self.resolve(path);
        let title = format!("Edit {}", self.display(&abs));
        let locations = json!([{"path": abs}]);
        self.tool_call(
            &id,
            "Edit",
            &title,
            "edit",
            json!({"file_path": abs}),
            json!([]),
            locations.clone(),
        )?;
        let input =
            json!({"file_path": abs, "old_string": old, "new_string": new, "replace_all": false});
        let diff = json!([{"type": "diff", "path": abs, "oldText": if old.is_empty() { Value::Null } else { json!(old) }, "newText": new}]);
        self.tool_update(&id, "Edit", json!({"rawInput": input, "title": title, "kind": "edit", "content": diff, "locations": locations}))?;
        if let Some(flow) = self.permit(&id, "Edit", &title, "edit", &input, &diff, &locations)? {
            return Ok(flow);
        }
        self.tool_update(&id, "Edit", json!({"status": "in_progress"}))?;
        let before = match self.read_file(&abs)? {
            Err(Interrupted) => return self.interrupted(&id, "Edit"),
            Ok(Err(why)) => return self.fail(&id, "Edit", &why).map(|_| Flow::Next),
            Ok(Ok(text)) => text,
        };
        if !before.contains(old) || old.is_empty() {
            let why = format!("<tool_use_error>String to replace not found in file.\nString: {old}</tool_use_error>");
            self.fail(&id, "Edit", &why)?;
            return Ok(Flow::Next);
        }
        let after = before.replacen(old, new, 1);
        match self.write_file(&abs, &after)? {
            Err(Interrupted) => return self.interrupted(&id, "Edit"),
            Ok(Err(why)) => return self.fail(&id, "Edit", &why).map(|_| Flow::Next),
            Ok(Ok(())) => {}
        }
        if self.agent.options().unannounced_write {
            let sneaky = self.cwd.join("UNANNOUNCED.txt");
            let mut text = std::fs::read_to_string(&sneaky).unwrap_or_default();
            text.push_str("written by the mock, unannounced\n");
            let _ = std::fs::write(&sneaky, text);
        }
        let (content, locations) = hunks(&abs, &before, &after);
        self.tool_update(
            &id,
            "Edit",
            json!({"status": "completed", "content": content, "locations": locations}),
        )?;
        Ok(Flow::Next)
    }

    fn write(&mut self, path: &str, text: &str) -> io::Result<Flow> {
        let id = self.tool_id();
        let abs = self.resolve(path);
        let title = format!("Write {}", self.display(&abs));
        let locations = json!([{"path": abs}]);
        self.tool_call(
            &id,
            "Write",
            &title,
            "edit",
            json!({"file_path": abs}),
            json!([]),
            locations.clone(),
        )?;
        let input = json!({"file_path": abs, "content": text});
        let diff = json!([{"type": "diff", "path": abs, "oldText": null, "newText": text}]);
        self.tool_update(&id, "Write", json!({"rawInput": input, "title": title, "kind": "edit", "content": diff, "locations": locations}))?;
        if let Some(flow) = self.permit(&id, "Write", &title, "edit", &input, &diff, &locations)? {
            return Ok(flow);
        }
        self.tool_update(&id, "Write", json!({"status": "in_progress"}))?;
        let before = std::fs::read_to_string(&abs).ok();
        match self.write_file(&abs, text)? {
            Err(Interrupted) => return self.interrupted(&id, "Write"),
            Ok(Err(why)) => return self.fail(&id, "Write", &why).map(|_| Flow::Next),
            Ok(Ok(())) => {}
        }
        // An update shows the real diff; a creation keeps the announced one.
        let fields = match before {
            Some(before) => {
                let (content, locations) = hunks(&abs, &before, text);
                json!({"status": "completed", "content": content, "locations": locations})
            }
            None => json!({"status": "completed"}),
        };
        self.tool_update(&id, "Write", fields)?;
        Ok(Flow::Next)
    }

    fn bash(&mut self, command: &str) -> io::Result<Flow> {
        let id = self.tool_id();
        self.tool_call(
            &id,
            "Bash",
            command,
            "execute",
            json!({"command": command}),
            json!([]),
            json!([]),
        )?;
        let input = json!({"command": command, "description": format!("Run {command}")});
        self.tool_update(
            &id,
            "Bash",
            json!({"rawInput": input, "title": command, "kind": "execute", "content": []}),
        )?;
        if let Some(flow) = self.permit(
            &id,
            "Bash",
            command,
            "execute",
            &input,
            &json!([]),
            &json!([]),
        )? {
            return Ok(flow);
        }
        self.tool_update(&id, "Bash", json!({"status": "in_progress"}))?;
        if self.agent.options().terminal {
            return self.bash_in_terminal(&id, command);
        }
        let (code, output) = match self.run_local(command) {
            Err(Interrupted) => return self.interrupted(&id, "Bash"),
            Ok(Err(why)) => return self.fail(&id, "Bash", &why).map(|_| Flow::Next),
            Ok(Ok(ran)) => ran,
        };
        let output = match code {
            Some(0) => output,
            Some(code) => format!("Exit code {code}\n{output}"),
            None => format!("Exit code 1\n{output}"),
        };
        let content = if output.trim().is_empty() {
            json!([])
        } else {
            json!([{"type": "content", "content": {"type": "text", "text": format!("```console\n{}\n```", output.trim_end())}}])
        };
        let status = if code == Some(0) {
            "completed"
        } else {
            "failed"
        };
        self.tool_update(&id, "Bash", json!({"status": status, "content": content}))?;
        Ok(Flow::Next)
    }

    /// `--terminal`: the client runs it (`terminal/create`, `wait_for_exit`,
    /// `output`, `release`), and the tool call shows the terminal.
    fn bash_in_terminal(&mut self, id: &str, command: &str) -> io::Result<Flow> {
        let (program, args) = shell_words(command);
        let create = json!({"sessionId": self.session, "command": program, "args": args, "cwd": self.cwd, "outputByteLimit": 1_048_576});
        let terminal = match self.call("terminal/create", create)? {
            Err(Interrupted) => return self.interrupted(id, "Bash"),
            Ok(Err(why)) => return self.fail(id, "Bash", &why).map(|_| Flow::Next),
            Ok(Ok(result)) => result["terminalId"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        };
        let shown = json!([{"type": "terminal", "terminalId": terminal}]);
        self.tool_update(id, "Bash", json!({"content": shown}))?;
        let which = json!({"sessionId": self.session, "terminalId": terminal});
        let exit = match self.call("terminal/wait_for_exit", which.clone())? {
            Err(Interrupted) => {
                self.agent.call_and_forget("terminal/kill", which.clone())?;
                self.agent.call_and_forget("terminal/release", which)?;
                return self.interrupted(id, "Bash");
            }
            Ok(Err(why)) => {
                self.agent.call_and_forget("terminal/release", which)?;
                return self.fail(id, "Bash", &why).map(|_| Flow::Next);
            }
            Ok(Ok(exit)) => exit,
        };
        // The output is read before the release, which frees it.
        let _ = self.call("terminal/output", which.clone())?;
        let _ = self.call("terminal/release", which)?;
        let status = if exit["exitCode"] == 0 {
            "completed"
        } else {
            "failed"
        };
        self.tool_update(id, "Bash", json!({"status": status, "content": shown}))?;
        Ok(Flow::Next)
    }

    fn exit_plan(&mut self) -> io::Result<Flow> {
        let id = self.tool_id();
        let plan = "## Plan\n\n1. Read the code\n2. Make the change\n3. Run the tests";
        let input = json!({"plan": plan});
        let content = json!([{"type": "content", "content": {"type": "text", "text": plan}}]);
        self.tool_call(
            &id,
            "ExitPlanMode",
            "Approve Plan",
            "switch_mode",
            input.clone(),
            content.clone(),
            json!([]),
        )?;
        let tool_call = json!({"toolCallId": id, "name": "ExitPlanMode", "status": "pending", "rawInput": input,
                               "title": "Approve Plan", "kind": "switch_mode", "content": content});
        match self.ask(
            tool_call,
            config::exit_plan_options(self.agent.options().bypass),
        )? {
            Answer::Cancelled => self.interrupted(&id, "ExitPlanMode"),
            Answer::Chose(option) if config::exit_plan_mode(&option).is_some() => {
                let mode = config::exit_plan_mode(&option).unwrap_or("default");
                self.agent.switch_mode(&self.session, mode)?;
                self.tool_update(
                    &id,
                    "ExitPlanMode",
                    json!({"status": "completed", "title": "Exited Plan Mode"}),
                )?;
                self.say(&["The plan is approved. Starting."])?;
                Ok(Flow::Next)
            }
            // "No, keep planning" ends the turn `cancelled`, as the adapter does.
            Answer::Chose(_) | Answer::Refused => {
                self.fail(
                    &id,
                    "ExitPlanMode",
                    "The user doesn't want to proceed with this plan.",
                )?;
                Ok(Flow::Stop("cancelled"))
            }
        }
    }

    /// Asks permission when the mode wants it. `None` to go ahead, or how
    /// the turn goes on when the tool won't run.
    #[allow(clippy::too_many_arguments)]
    fn permit(
        &mut self,
        id: &str,
        name: &str,
        title: &str,
        kind: &str,
        input: &Value,
        content: &Value,
        locations: &Value,
    ) -> io::Result<Option<Flow>> {
        let prefix = input["command"]
            .as_str()
            .and_then(|c| c.split_whitespace().next())
            .unwrap_or_default()
            .to_string();
        match self.gate(kind, &prefix) {
            Gate::Allow => Ok(None),
            Gate::Refuse => {
                self.fail(
                    id,
                    name,
                    "Plan mode is active: no changes until the plan is approved.",
                )?;
                Ok(Some(Flow::Next))
            }
            Gate::Ask => {
                let tool_call = json!({"toolCallId": id, "name": name, "status": "pending", "rawInput": input,
                                       "title": title, "kind": kind, "content": content, "locations": locations});
                let options = if kind == "execute" {
                    config::bash_options(&prefix)
                } else {
                    config::edit_options()
                };
                match self.ask(tool_call, options)? {
                    Answer::Cancelled => self.interrupted(id, name).map(Some),
                    Answer::Chose(option) if option == "allow-once" => Ok(None),
                    Answer::Chose(option) if option == "allow-with-updates" => {
                        if kind == "execute" {
                            if let Some(session) = self.agent.sessions().get_mut(&self.session) {
                                session.allowed_commands.insert(prefix);
                            }
                        } else {
                            self.agent.switch_mode(&self.session, "acceptEdits")?;
                        }
                        Ok(None)
                    }
                    Answer::Chose(_) | Answer::Refused => {
                        self.fail(id, name, "The user doesn't want to proceed with this tool use. The tool use was rejected.")?;
                        Ok(Some(Flow::Stop("end_turn")))
                    }
                }
            }
        }
    }

    /// `default` asks for edits and commands, `acceptEdits` for commands,
    /// `auto` and `bypassPermissions` for nothing; `plan` refuses edits.
    fn gate(&self, kind: &str, prefix: &str) -> Gate {
        let sessions = self.agent.sessions();
        let Some(session) = sessions.get(&self.session) else {
            return Gate::Allow;
        };
        let mode = session.settings.mode.as_str();
        match (kind, mode) {
            (_, "auto" | "bypassPermissions") => Gate::Allow,
            ("edit", "plan") => Gate::Refuse,
            ("edit", "acceptEdits") => Gate::Allow,
            ("execute", _) if session.allowed_commands.contains(prefix) => Gate::Allow,
            _ => Gate::Ask,
        }
    }

    fn ask(&mut self, tool_call: Value, options: Value) -> io::Result<Answer> {
        let params = json!({"sessionId": self.session, "toolCall": tool_call, "options": options});
        Ok(
            match self
                .agent
                .call("session/request_permission", params, &self.ctl.cancel)
            {
                Ok(result) => match result["outcome"]["outcome"].as_str() {
                    Some("selected") => Answer::Chose(
                        result["outcome"]["optionId"]
                            .as_str()
                            .unwrap_or_default()
                            .to_string(),
                    ),
                    _ if self.cancelled() => Answer::Cancelled,
                    _ => Answer::Refused,
                },
                Err(CallError::Cancelled(request)) => {
                    // Withdrawn, as the adapter's abort signal does.
                    self.agent
                        .notify("$/cancel_request", json!({"requestId": request}))?;
                    Answer::Cancelled
                }
                Err(CallError::Failed(_)) => Answer::Refused,
                Err(CallError::Gone) => return Err(io::ErrorKind::BrokenPipe.into()),
            },
        )
    }

    // --- Files and processes ----------------------------------------------

    fn read_file(
        &mut self,
        path: &Path,
    ) -> io::Result<Result<Result<String, String>, Interrupted>> {
        if !self.agent.options().fs {
            return Ok(Ok(std::fs::read_to_string(path).map_err(|_| {
                "<tool_use_error>File does not exist.</tool_use_error>".to_string()
            })));
        }
        let read = self.call(
            "fs/read_text_file",
            json!({"sessionId": self.session, "path": path}),
        )?;
        Ok(
            read.map(|result| {
                result.map(|r| r["content"].as_str().unwrap_or_default().to_string())
            }),
        )
    }

    fn write_file(
        &mut self,
        path: &Path,
        text: &str,
    ) -> io::Result<Result<Result<(), String>, Interrupted>> {
        if !self.agent.options().fs {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            return Ok(Ok(
                std::fs::write(path, text).map_err(|error| error.to_string())
            ));
        }
        let written = self.call(
            "fs/write_text_file",
            json!({"sessionId": self.session, "path": path, "content": text}),
        )?;
        Ok(written.map(|result| result.map(|_| ())))
    }

    /// A request to the client: `Err(Interrupted)` if the turn was
    /// cancelled first, `Ok(Err)` with the client's error message.
    fn call(
        &mut self,
        method: &str,
        params: Value,
    ) -> io::Result<Result<Result<Value, String>, Interrupted>> {
        match self.agent.call(method, params, &self.ctl.cancel) {
            Ok(result) => Ok(Ok(Ok(result))),
            Err(CallError::Failed(error)) => Ok(Ok(Err(error["message"]
                .as_str()
                .unwrap_or("failed")
                .to_string()))),
            Err(CallError::Cancelled(_)) => Ok(Err(Interrupted)),
            Err(CallError::Gone) => Err(io::ErrorKind::BrokenPipe.into()),
        }
    }

    /// Runs a command with the system shell in the session's cwd, killing it
    /// on cancel. Its exit code and its output, stdout then stderr.
    fn run_local(
        &self,
        command: &str,
    ) -> Result<Result<(Option<i32>, String), String>, Interrupted> {
        let (program, args) = shell_words(command);
        let child = Command::new(program)
            .args(&args)
            .current_dir(&self.cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn();
        let mut child = match child {
            Ok(child) => child,
            Err(error) => return Ok(Err(error.to_string())),
        };
        let drain = |pipe: Option<Box<dyn Read + Send>>| {
            std::thread::spawn(move || {
                let mut text = Vec::new();
                if let Some(mut pipe) = pipe {
                    let _ = pipe.read_to_end(&mut text);
                }
                String::from_utf8_lossy(&text).into_owned()
            })
        };
        let stdout = drain(
            child
                .stdout
                .take()
                .map(|p| Box::new(p) as Box<dyn Read + Send>),
        );
        let stderr = drain(
            child
                .stderr
                .take()
                .map(|p| Box::new(p) as Box<dyn Read + Send>),
        );
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if self.cancelled() => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(Interrupted);
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(10)),
                Err(error) => return Ok(Err(error.to_string())),
            }
        };
        let parts = [
            stdout.join().unwrap_or_default(),
            stderr.join().unwrap_or_default(),
        ];
        let output: Vec<&str> = parts
            .iter()
            .map(|part| part.trim_end_matches('\n'))
            .filter(|part| !part.is_empty())
            .collect();
        Ok(Ok((status.code(), output.join("\n"))))
    }

    // --- Updates -----------------------------------------------------------

    /// Sends an update, keeping the ones a load replays.
    fn send(&mut self, update: Value) -> io::Result<()> {
        let kept = matches!(
            update["sessionUpdate"].as_str(),
            Some(
                "agent_message_chunk"
                    | "agent_thought_chunk"
                    | "tool_call"
                    | "tool_call_update"
                    | "plan"
            )
        );
        self.agent.update(&self.session, update.clone())?;
        if kept {
            self.record.push(update);
        }
        Ok(())
    }

    fn chunk(&mut self, kind: &str, message_id: &str, text: &str) -> io::Result<()> {
        self.output += text.len();
        self.send(json!({"sessionUpdate": kind, "content": {"type": "text", "text": text}, "messageId": message_id}))
    }

    fn say(&mut self, chunks: &[&str]) -> io::Result<()> {
        self.stream("agent_message_chunk", chunks)
    }

    fn think(&mut self, chunks: &[&str]) -> io::Result<()> {
        self.stream("agent_thought_chunk", chunks)
    }

    fn stream(&mut self, kind: &str, chunks: &[&str]) -> io::Result<()> {
        let id = self.agent.uuid();
        for text in chunks {
            if self.cancelled() {
                break;
            }
            self.chunk(kind, &id, text)?;
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn tool_call(
        &mut self,
        id: &str,
        name: &str,
        title: &str,
        kind: &str,
        input: Value,
        content: Value,
        locations: Value,
    ) -> io::Result<()> {
        self.send(json!({
            "sessionUpdate": "tool_call",
            "_meta": {"claudeCode": {"toolName": name}},
            "toolCallId": id,
            "name": name,
            "rawInput": input,
            "status": "pending",
            "title": title,
            "kind": kind,
            "content": content,
            "locations": locations,
        }))
    }

    fn tool_update(&mut self, id: &str, name: &str, fields: Value) -> io::Result<()> {
        let mut update = json!({"sessionUpdate": "tool_call_update", "_meta": {"claudeCode": {"toolName": name}}, "toolCallId": id});
        if let (Some(update), Value::Object(fields)) = (update.as_object_mut(), fields) {
            update.extend(fields);
        }
        self.send(update)
    }

    fn fail(&mut self, id: &str, name: &str, why: &str) -> io::Result<()> {
        let content = json!([{"type": "content", "content": {"type": "text", "text": why}}]);
        self.tool_update(id, name, json!({"status": "failed", "content": content}))
    }

    /// A tool cut short by a cancel: it ends `failed`, and so does the turn.
    fn interrupted(&mut self, id: &str, name: &str) -> io::Result<Flow> {
        self.tool_update(id, name, json!({"status": "failed"}))?;
        Ok(Flow::Stop("cancelled"))
    }

    // --- Helpers -------------------------------------------------------------

    fn cancelled(&self) -> bool {
        self.ctl.cancel.load(Ordering::SeqCst)
    }

    /// Waits `ms`, or less if cancelled: whether it waited it all.
    fn pause(&self, ms: u64) -> bool {
        for _ in 0..ms.div_ceil(10) {
            if self.cancelled() {
                return false;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        !self.cancelled()
    }

    fn tool_id(&self) -> String {
        format!("toolu_{}", self.agent.uuid().replace('-', ""))
    }

    fn resolve(&self, path: &str) -> PathBuf {
        let path = Path::new(path);
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.cwd.join(path)
        }
    }

    /// Relative to the cwd when inside it, as the adapter's titles are.
    fn display(&self, path: &Path) -> String {
        match path.strip_prefix(&self.cwd) {
            Ok(relative) if !relative.as_os_str().is_empty() => {
                relative.to_string_lossy().into_owned()
            }
            _ => path.to_string_lossy().into_owned(),
        }
    }
}

/// The turn was cancelled while waiting.
struct Interrupted;

fn shell_words(command: &str) -> (&'static str, Vec<String>) {
    if cfg!(windows) {
        ("cmd", vec!["/C".into(), command.into()])
    } else {
        ("sh", vec!["-c".into(), command.into()])
    }
}

/// A title from the first line of the first prompt.
fn title_of(text: &str) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("New session");
    let mut title: String = line.chars().take(50).collect();
    if line.chars().count() > 50 {
        title.push('…');
    }
    title
}

/// Fences text so no fence inside it closes it (`markdownEscape`).
fn markdown_escape(text: &str) -> String {
    let mut fence = String::from("```");
    for line in text.lines() {
        let ticks = line.chars().take_while(|&c| c == '`').count();
        while ticks >= 3 && ticks >= fence.len() {
            fence.push('`');
        }
    }
    let newline = if text.ends_with('\n') { "" } else { "\n" };
    format!("{fence}\n{text}{newline}{fence}")
}

/// The change as Claude's PostToolUse hook reports it: a diff per hunk,
/// with three lines of context, and where each starts.
fn hunks(path: &Path, before: &str, after: &str) -> (Value, Value) {
    let old: Vec<&str> = before.lines().collect();
    let new: Vec<&str> = after.lines().collect();
    let prefix = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
    let room = old.len().min(new.len()) - prefix;
    let suffix = old
        .iter()
        .rev()
        .zip(new.iter().rev())
        .take(room)
        .take_while(|(a, b)| a == b)
        .count();
    if prefix == old.len() && prefix == new.len() {
        return (json!([]), json!([]));
    }
    let start = prefix.saturating_sub(3);
    let old_end = (old.len() - suffix + 3).min(old.len());
    let new_end = (new.len() - suffix + 3).min(new.len());
    let old_text = old[start..old_end].join("\n");
    let line = start + 1;
    (
        json!([{"type": "diff", "path": path, "oldText": if old_text.is_empty() { Value::Null } else { json!(old_text) },
                "newText": new[start..new_end].join("\n")}]),
        json!([{"path": path, "line": line}]),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hunk_with_context() {
        let before = "a\nb\nc\nd\ne\nf\ng\nh\n";
        let after = "a\nb\nc\nd\nE\nf\ng\nh\n";
        let (content, locations) = hunks(Path::new("/p"), before, after);
        assert_eq!(content[0]["oldText"], "b\nc\nd\ne\nf\ng\nh");
        assert_eq!(content[0]["newText"], "b\nc\nd\nE\nf\ng\nh");
        assert_eq!(locations[0]["line"], 2);
    }

    #[test]
    fn fences_outlast_the_text() {
        assert_eq!(markdown_escape("x"), "```\nx\n```");
        assert_eq!(markdown_escape("```\nx"), "````\n```\nx\n````");
    }
}
