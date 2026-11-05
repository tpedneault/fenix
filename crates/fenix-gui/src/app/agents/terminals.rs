//! Commands an agent runs through Fenix (`terminal/*`), and logins it
//! asks you to do in a terminal.
//!
//! An agent's command runs on `fenix-tasks`' `TaskRunner` -- pipes, the
//! whole process tree killed when it's stopped -- its output kept up to
//! the agent's limit, cut from the front. The thread's tool card shows
//! its last lines as they come. Only an agent that asks gets this; one
//! that runs its commands itself (Claude Code's adapter) never does.
//!
//! A terminal login (`authMethods` of `type: "terminal"`) runs the
//! agent's own program with the method's arguments in a terminal pane,
//! for you to log in there; when it ends the agent is asked again.

use super::*;
use fenix_acp::protocol::{self, ExitStatus};
use fenix_tasks::{OutputLine, TaskEvent, TaskOutcome, TaskRunner};

/// What a terminal's output keeps when the agent gives no limit.
const DEFAULT_LIMIT: usize = 1024 * 1024;

/// A command an agent runs through Fenix.
pub(crate) struct AgentTerminal {
    pub(crate) thread: ThreadId,
    /// The command as asked for, for the tool card.
    pub(crate) line: String,
    runner: Option<TaskRunner>,
    state: Arc<Mutex<TermState>>,
    /// `terminal/wait_for_exit` requests waiting for it to end.
    waiters: Vec<(ConnId, Value)>,
}

#[derive(Default)]
struct TermState {
    output: String,
    /// Bytes cut from the front to keep to the limit.
    truncated: bool,
    exit: Option<ExitStatus>,
    /// Changed since the page was last told.
    fresh: bool,
}

impl AgentTerminal {
    fn output(&self) -> (String, bool, Option<ExitStatus>) {
        let s = self.state.lock().unwrap();
        (s.output.clone(), s.truncated, s.exit.clone())
    }

    /// Its last few lines, and how it ended, for the tool card.
    pub(crate) fn tail(&self, lines: usize) -> (String, Option<ExitStatus>) {
        let s = self.state.lock().unwrap();
        let all: Vec<&str> = s.output.lines().collect();
        (all[all.len().saturating_sub(lines)..].join("\n"), s.exit.clone())
    }
}

/// How an agent's command is started. A program (found on `PATH`, or a
/// path to one) runs with its arguments as given. Anything else is a
/// command line -- `pwd && rg foo`, `Get-ChildItem .\`, a shell builtin --
/// and runs in the platform's shell, as other ACP clients do: Devin sends
/// those, and starting one as a program failed ("Failed to create
/// terminal").
fn agent_command(command: &str, args: &[String]) -> std::process::Command {
    if let Some(program) = catalog::find_on_path(command) {
        let mut process = std::process::Command::new(program);
        process.args(args);
        return process;
    }
    let line = std::iter::once(command.to_string()).chain(args.iter().map(|a| shell_quote(a))).collect::<Vec<_>>().join(" ");
    let (shell, flags): (PathBuf, &[&str]) = if cfg!(windows) {
        match catalog::find_on_path("pwsh") {
            Some(pwsh) => (pwsh, &["-NoLogo", "-NoProfile", "-NonInteractive", "-Command"]),
            None => (PathBuf::from("powershell.exe"), &["-NoLogo", "-NoProfile", "-NonInteractive", "-Command"]),
        }
    } else {
        (PathBuf::from("sh"), &["-c"])
    };
    let mut process = std::process::Command::new(shell);
    process.args(flags).arg(line);
    process
}

/// An argument as the shell reads it back: left alone when it's plain,
/// in single quotes otherwise (PowerShell doubles a quote inside them,
/// `sh` closes and reopens around it).
fn shell_quote(arg: &str) -> String {
    if !arg.is_empty() && arg.chars().all(|c| c.is_ascii_alphanumeric() || "-_./\\:=+,@".contains(c)) {
        return arg.to_string();
    }
    if cfg!(windows) {
        format!("'{}'", arg.replace('\'', "''"))
    } else {
        format!("'{}'", arg.replace('\'', r"'\''"))
    }
}

fn keep_limit(state: &mut TermState, limit: usize) {
    if state.output.len() > limit {
        let mut start = state.output.len() - limit;
        while !state.output.is_char_boundary(start) {
            start += 1;
        }
        state.output.drain(..start);
        state.truncated = true;
    }
}

impl App {
    /// `terminal/create`: starts the command, and answers with its id.
    pub(super) fn agents_terminal_create(&mut self, thread: &ThreadId, command: String, args: Vec<String>, env: Vec<(String, String)>, cwd: Option<String>, limit: Option<u64>) -> Result<Value, RpcError> {
        let root = self.agents.hub.meta(thread).map(|m| m.root.clone()).ok_or_else(|| RpcError::new(code::INVALID_PARAMS, "no such thread"))?;
        let cwd = cwd.map(PathBuf::from).unwrap_or_else(|| root.clone());
        if !within(&root, &cwd) {
            return Err(RpcError::new(code::INVALID_PARAMS, format!("{} is outside the project", cwd.display())));
        }
        let configured = self.config.int_value("agents.terminal.output_limit").map(|k| k.max(16) as usize * 1024).unwrap_or(DEFAULT_LIMIT);
        let limit = limit.map(|l| (l as usize).min(configured)).unwrap_or(configured);
        let line = std::iter::once(command.clone()).chain(args.iter().map(|a| shell_quote(a))).collect::<Vec<_>>().join(" ");
        let mut process = agent_command(&command, &args);
        process.current_dir(&cwd).envs(env);
        let state = Arc::new(Mutex::new(TermState::default()));
        let sink = state.clone();
        let runner = TaskRunner::spawn_command(process, move |event| {
            let mut s = sink.lock().unwrap();
            match event {
                TaskEvent::Output(lines) => {
                    for OutputLine { text, .. } in lines {
                        s.output.push_str(&text);
                        s.output.push('\n');
                    }
                    keep_limit(&mut s, limit);
                }
                TaskEvent::Finished(outcome) => {
                    s.exit = Some(match outcome {
                        TaskOutcome::Exited { code, .. } => ExitStatus { code: code.map(|c| c as u32), signal: None },
                        TaskOutcome::Cancelled => ExitStatus { code: None, signal: Some("SIGKILL".into()) },
                        TaskOutcome::Failed(why) => {
                            s.output.push_str(&format!("{why}\n"));
                            ExitStatus { code: Some(127), signal: None }
                        }
                    });
                }
            }
            s.fresh = true;
            true
        })
        .map_err(|e| RpcError::new(code::INTERNAL_ERROR, format!("couldn't run {command}: {e}")))?;
        let id = format!("term-{}", self.agents.next_terminal);
        self.agents.next_terminal += 1;
        self.agents.terminals.insert(id.clone(), AgentTerminal { thread: thread.clone(), line, runner: Some(runner), state, waiters: Vec::new() });
        Ok(json!({ "terminalId": id }))
    }

    /// The rest of `terminal/*`. `None`: answered later (`wait_for_exit`).
    pub(super) fn agents_terminal_request(&mut self, conn: ConnId, thread: &ThreadId, id: Value, request: ClientRequest) -> Option<Result<Value, RpcError>> {
        let missing = |t: &str| Some(Err(RpcError::new(code::INVALID_PARAMS, format!("no terminal {t}"))));
        match request {
            ClientRequest::TerminalOutput { terminal_id, .. } => {
                let Some(t) = self.agents.terminals.get(&terminal_id).filter(|t| &t.thread == thread) else { return missing(&terminal_id) };
                let (output, truncated, exit) = t.output();
                Some(Ok(protocol::terminal_output_result(&output, truncated, exit.as_ref())))
            }
            ClientRequest::WaitForTerminalExit { terminal_id, .. } => {
                let Some(t) = self.agents.terminals.get_mut(&terminal_id).filter(|t| &t.thread == thread) else { return missing(&terminal_id) };
                if let Some(exit) = t.state.lock().unwrap().exit.clone() {
                    return Some(Ok(exit.to_value()));
                }
                t.waiters.push((conn, id));
                None
            }
            ClientRequest::KillTerminal { terminal_id, .. } => {
                let Some(t) = self.agents.terminals.get_mut(&terminal_id).filter(|t| &t.thread == thread) else { return missing(&terminal_id) };
                if let Some(runner) = &t.runner {
                    runner.cancel();
                }
                Some(Ok(Value::Null))
            }
            ClientRequest::ReleaseTerminal { terminal_id, .. } => {
                // Its output stays on the card; the process goes.
                if let Some(t) = self.agents.terminals.get_mut(&terminal_id).filter(|t| &t.thread == thread) {
                    if let Some(runner) = t.runner.take() {
                        runner.cancel();
                    }
                    let mut s = t.state.lock().unwrap();
                    if s.exit.is_none() {
                        s.exit = Some(ExitStatus { code: None, signal: Some("released".into()) });
                    }
                }
                Some(Ok(Value::Null))
            }
            _ => None,
        }
    }

    /// Terminals that ended answer their waiters; ones with new output
    /// redraw the thread. Whether any is still running.
    pub(super) fn agents_terminals_tick(&mut self) -> bool {
        let mut answers = Vec::new();
        let mut fresh = Vec::new();
        let mut running = false;
        for t in self.agents.terminals.values_mut() {
            let mut s = t.state.lock().unwrap();
            if s.fresh {
                s.fresh = false;
                fresh.push(t.thread.clone());
            }
            match &s.exit {
                Some(exit) => {
                    for (conn, id) in t.waiters.drain(..) {
                        answers.push((conn, t.thread.clone(), id, exit.clone()));
                    }
                }
                None => running = true,
            }
        }
        let mut fx = Vec::new();
        for (conn, thread, id, exit) in answers {
            fx.extend(self.agents.hub.on_served(conn, &thread, id, Ok(exit.to_value())));
        }
        fx.extend(fresh.into_iter().map(Effect::Changed));
        if !fx.is_empty() {
            self.agents_run(fx);
        }
        running
    }

    /// The thread's turn was cut short or its agent went away: its
    /// terminals are stopped (their output stays on the cards).
    pub(super) fn agents_kill_terminals(&mut self, thread: &ThreadId) {
        for t in self.agents.terminals.values_mut().filter(|t| &t.thread == thread) {
            if let Some(runner) = t.runner.take() {
                runner.cancel();
            }
            t.waiters.clear();
        }
    }

    /// A terminal login: the agent's own program, with the method's
    /// arguments, in a terminal pane under the panel. When it ends the
    /// agent is asked again.
    pub(super) fn agents_start_terminal_login(&mut self, conn: ConnId, args: &[String]) {
        let Some(c) = self.agents.hub.connection(conn) else { return };
        let program = c.agent.program.clone();
        // The agent's own arguments, less the one that makes it speak
        // ACP: what's left starts it as you'd run it yourself.
        let mut all: Vec<String> = c.agent.args.iter().filter(|a| !matches!(a.as_str(), "acp" | "--acp" | "--experimental-acp")).cloned().collect();
        all.extend(args.iter().cloned());
        let root = c.root.clone();
        let buffer = self.buffers.open_terminal();
        self.terminal_buffer_cwds.insert(buffer, root);
        self.terminal_buffer_labels.insert(buffer, format!("*log in to {}*", c.agent.name));
        self.terminal_buffer_programs.insert(buffer, (program, all));
        if let Some(pane) = self.agents.code_pane.filter(|p| self.windows().windows().contains(p)) {
            self.windows_mut().focus(pane);
        }
        let pane = self.windows_mut().split(SplitKind::Horizontal, buffer);
        self.workspaces.active_pane_states_mut().insert(pane, PaneState::seeded_at(Cursor::at_start()));
        self.workspaces.active_pane_tabs_mut().insert(pane, vec![buffer]);
        self.windows_mut().focus(pane);
        self.spawn_terminal_for(buffer);
        self.agents.logins.push((conn, buffer));
        self.set_message("log in in the terminal; when it ends, Fenix asks the agent again");
        self.wake_caret();
    }

    /// Terminal logins that ended: the agent is asked again.
    pub(super) fn agents_logins_tick(&mut self) {
        let mut ended = Vec::new();
        self.agents.logins.retain(|(conn, buffer)| {
            let spawning = self.terminal_buffers_spawning.contains(buffer);
            let alive = self.terminal_buffers.get_mut(buffer).is_some_and(|t| t.session.is_alive());
            if spawning || alive {
                true
            } else {
                ended.push(*conn);
                false
            }
        });
        for conn in ended {
            // The exit status isn't known: the agent says whether it
            // worked, by asking again or not.
            let fx = self.agents.hub.terminal_login_finished(conn, true);
            self.agents_run(fx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_program_runs_as_itself_and_a_command_line_in_the_shell() {
        let git = agent_command("git", &["--version".into()]);
        assert!(git.get_program().to_string_lossy().to_lowercase().contains("git"), "{:?}", git.get_program());
        assert_eq!(git.get_args().collect::<Vec<_>>(), ["--version"]);
        let line = agent_command("pwd && rg foo", &[]);
        let shell = line.get_program().to_string_lossy().to_lowercase();
        assert!(if cfg!(windows) { shell.contains("pwsh") || shell.contains("powershell") } else { shell == "sh" }, "{shell}");
        assert_eq!(line.get_args().last().map(|a| a.to_string_lossy().into_owned()).as_deref(), Some("pwd && rg foo"));
    }

    #[test]
    fn arguments_are_quoted_for_the_shell_only_when_they_need_it() {
        assert_eq!(shell_quote("src/main.rs"), "src/main.rs");
        assert_eq!(shell_quote("two words"), "'two words'");
        assert_eq!(shell_quote(""), "''");
        let quoted = shell_quote("it's");
        assert!(quoted == "'it''s'" || quoted == r"'it'\''s'", "{quoted}");
    }

    #[test]
    fn output_is_kept_to_the_limit_from_the_end() {
        let mut s = TermState { output: "aé".repeat(10), ..TermState::default() };
        keep_limit(&mut s, 5);
        assert!(s.truncated && s.output.len() <= 5 && s.output.ends_with('é'));
    }
}
