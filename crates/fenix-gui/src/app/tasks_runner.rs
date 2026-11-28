//! Tasks: picking one, running it and its output.
//!
//! Methods of `App` moved here from `app.rs` as they were.

use super::*;

impl App {
    /// Where task `task` of the project at `root` comes from, as the task
    /// picker says: the project's own `tools.json` (`own`), the extension
    /// that gave it, or Fenix's defaults for the project's kind (#35).
    pub(super) fn task_source(&self, root: &Path, task: &fenix_tasks::TaskDef, own: bool) -> String {
        if own {
            return "tools.json".to_string();
        }
        self.wasm.task_owner(root, task).unwrap_or_else(|| "Fenix".to_string())
    }

    /// `SPC t r` -- reruns `last_task` without going back through the
    /// picker. A no-op (with a message) if nothing has been run yet this
    /// session.
    pub(crate) fn rerun_last_task(&mut self) {
        let root = self.integration_root();
        // `SPC p r`'s test run, when it went through the test explorer.
        if self.tests_rerun_door(&root) {
            return;
        }
        let Some(task) = self.project_last_tasks.get(&root).cloned() else {
            self.set_error("no task has been run yet for this project".to_string());
            return;
        };
        self.run_task(task, root);
    }

    /// `SPC t k` -- ends whatever task is currently running. A no-op
    /// (with a message) if no task is running right now -- either no
    /// panel is open at all, or one is but its last run already finished
    /// (`finish_task_run` already cleared `runner`).
    pub(crate) fn kill_running_task(&mut self) {
        let Some(session) = &self.task_session else {
            self.set_error("no task is running".to_string());
            return;
        };
        if session.root != self.integration_root() { self.set_error("task belongs to another project; switch to its task panel to cancel it"); return; }
        let Some(runner) = &session.runner else {
            self.set_error("no task is running".to_string());
            return;
        };
        runner.cancel();
        let buffer = session.buffer;
        self.append_task_output_text(buffer, "-- cancellation requested --");
    }

    /// Runs `task` from `root`, opening the single-pane Task Output
    /// panel if this is the first task run this session, or reusing
    /// (and clearing) the existing one otherwise -- replacing any
    /// previous run's live `runner` first, so there's never two tasks
    /// running from this app at once (same "one at a time" posture
    /// `DockerHost::view_logs_selected` already established for the Docker
    /// log follower). Also records `last_task` for `SPC t r`, and clears
    /// the quickfix list before anything about this run's own output has
    /// arrived -- a stale previous run's diagnostics have no business
    /// surviving into a fresh one.
    pub(super) fn run_task(&mut self, task: fenix_tasks::TaskDef, root: PathBuf) {
        self.run_task_as(task, root, None);
    }

    /// `run_task`, with how an extension gave it (`given`: its
    /// environment) when the project doesn't name the task itself. False
    /// when it didn't start.
    pub(super) fn run_task_as(&mut self, mut task: fenix_tasks::TaskDef, root: PathBuf, given: Option<fenix_project::tools::CommandSpec>) -> bool {
        let root = fenix_lsp::normalize(refactor::identity(&root));
        if self.task_session.as_ref().is_some_and(|s| s.runner.is_some() && s.root != root) {
            self.set_error("a task is running in another project; cancel it from its task panel first"); return false;
        }
        let tools = match self.project_tools(&root) { Ok(tools) => tools, Err(error) => { self.set_error(error); return false; } };
        // The project's own first, then an extension's as it gave it (with
        // its environment), then the task as it is.
        let given = given.or_else(|| self.wasm.task_spec(&root, &task.name).filter(|s| s.executable == task.command && s.args == task.args));
        let spec = tools.tasks.get(&task.name).cloned().or(given).unwrap_or_else(|| fenix_project::tools::CommandSpec::new(task.command.clone(), task.args.clone()));
        task.command = spec.executable.clone();
        task.args = spec.args.clone();
        self.project_last_tasks.insert(root.clone(), task.clone());
        // A debug session's `preLaunchTask` isn't what `SPC p t` or `SPC p
        // r` ran: `SPC p T` keeps a door's test run (step 7's review, S3).
        if self.debug_after_task.as_ref().is_none_or(|(name, _)| *name != task.name) {
            self.tests_task_ran(&root);
        }
        self.quickfix.clear();
        self.search.quickfix_page = None;
        self.quickfix_index = None;
        let header = format!("$ {} {}\n", task.command, task.args.join(" "));

        let buffer = if let Some(session) = &self.task_session {
            let (workspace_index, pane, buffer) = (session.workspace_index, session.pane, session.buffer);
            self.task_session.as_mut().expect("just matched Some above").runner = None; // tear down any previous runner first
            self.workspaces.switch_to_index(workspace_index);
            self.windows_mut().focus(pane);
            self.reset_task_output_buffer(buffer, &header);
            buffer
        } else {
            let buffer = self.buffers.open_task_output(&header);
            let cursor = Cursor::at_start();
            self.workspaces.new_workspace(buffer, cursor);
            let workspace_index = self.workspaces.active_index();
            let pane = self.focused_pane_id();
            self.pane_titles.insert(pane, "Task Output".to_string());
            self.task_session = Some(TaskSession { workspace_index, pane, buffer, cwd: root.clone(), root: root.clone(), runner: None, run_id: None, started: None });
            buffer
        };

        if let Some(session) = self.task_session.as_mut() { session.root = root.clone(); }
        // The extensions that asked are told before it can end.
        self.wasm.task_name = Some(task.name.clone());
        self.editor_call("tasks", fenix_wasm::host::Call::TaskStarted(task.name.clone()));
        self.start_task_process(task, spec, root, buffer);
        self.wake_caret();
        true
    }

    /// `Enter` in Task Output: the place the cursor's line names, opened
    /// as `SPC p n` would, relative to where the task ran. False, and
    /// nothing done, when the line names none.
    pub(crate) fn task_output_open_location_at_cursor(&mut self) -> bool {
        let (line, _) = self.open().buffer.line_col(&self.cursor());
        let text = self.open().buffer.line(line).to_string();
        let Some(found) = fenix_tasks::find_location(text.trim_end()) else { return false };
        let buffer = self.focused_buffer_id();
        let base = self
            .task_session
            .as_ref()
            .filter(|s| s.buffer == buffer)
            .map(|s| s.cwd.clone())
            // A test run's *Test output*: the folder of the step the line
            // is in.
            .or_else(|| self.tests_output_base(buffer, line))
            .unwrap_or_else(|| self.integration_root());
        let path = base.join(&found.path);
        if !path.is_file() {
            self.set_error(format!("{} isn't a file", readable_path(&path)));
            return true;
        }
        let from = JumpEntry { buffer, char_idx: self.cursor().char_idx };
        let m = fenix_project::GrepMatch { path, line: found.line, col: found.col, text: found.message };
        self.open_file_as_preview(&m.path);
        self.jump_to_grep_match(&m);
        self.record_jump(from);
        true
    }

    /// Closes Task Output (`q`, `SPC b d`): a task still running is
    /// cancelled, as `SPC p k` does; its buffer, and the workspace it
    /// opened in, go. The next task opens it again (#29).
    pub(super) fn task_output_close(&mut self) {
        let Some(session) = self.task_session.take() else { return };
        if let Some(runner) = &session.runner {
            runner.cancel();
        }
        self.close_view_workspace(session.workspace_index, session.pane, session.buffer);
    }
}
