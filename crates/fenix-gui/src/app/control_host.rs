//! The editor's answers to the control channel (`crate::ipc`): every
//! request the protocol itself can't answer arrives here, on the UI
//! thread, as a `FenixUserEvent::Control`. The command line ends
//! up here too when this launch is the editor, so `fenix x.rs:12` means
//! the same whether or not Fenix was already running.

use fenix_control::protocol::OpenProjectParams;
use fenix_control::{
    Call, CaptureResult, CaptureTarget, CommandInfo, CommandsResult, RpcError,
    StateResult,
};
use fenix_tasks::PathPosition;
use serde_json::{json, Value};

use super::*;
use crate::commands::ExternalError;

fn to_value(value: impl serde::Serialize) -> Value {
    serde_json::to_value(value).expect("plain data serializes")
}

/// A control request a code extension answers: the service, what it's
/// given, and the protocol's answer made of what it says.
type ByExtension = (&'static str, Value, fn(String) -> Result<Value, RpcError>);

/// What a service's answer is to a caller that wants `{}` back.
fn done(_: String) -> Result<Value, RpcError> {
    Ok(json!({}))
}

impl App {
    /// `call`, sent to the code extension that answers it, its reply
    /// going back when it does. `Err` gives both back: Fenix answers it.
    pub(super) fn control_by_extension(&mut self, call: Call, reply: std::sync::mpsc::Sender<crate::ipc::Reply>) -> Result<(), (Call, std::sync::mpsc::Sender<crate::ipc::Reply>)> {
        let asked: Option<ByExtension> = match &call {
            Call::NotebookCapture(params) => Some((
                "notebook.capture",
                json!({ "to": match params.target { CaptureTarget::Inbox => "inbox", CaptureTarget::Journal => "journal" }, "text": params.text }),
                |entry| Ok(to_value(CaptureResult { entry })),
            )),
            Call::AgendaTasks(params) => Some(("agenda.tasks", to_value(params), |tasks| serde_json::from_str(&tasks).map_err(|e| RpcError::internal(e.to_string())))),
            Call::AgendaClockIn(params) => Some(("agenda.clock_in", json!({ "task_id": params.task_id }), done)),
            Call::AgendaClockOut => Some(("agenda.clock_out", Value::Null, done)),
            _ => None,
        };
        // `fenix/state`: Fenix's, with the agenda's clock asked for.
        if matches!(call, Call::State) {
            let Some(server) = self.code_server("agenda.timer", None) else { return Err((call, reply)) };
            let state = self.control_state();
            let then = move |_: &mut App, timer: Result<String, String>| {
                let mut state = state;
                state.agenda_timer = timer.ok().and_then(|t| serde_json::from_str(&t).ok());
                let _ = reply.send(Ok(to_value(state)));
            };
            self.code_serve(&server, "agenda.timer", "null".into(), super::wasm_host::Answer::Fenix(Box::new(then)));
            return Ok(());
        }
        let Some((name, payload, answer)) = asked else { return Err((call, reply)) };
        let Some(server) = self.code_server(name, None) else { return Err((call, reply)) };
        let then = move |_: &mut App, said: Result<String, String>| {
            let _ = reply.send(said.map_err(RpcError::invalid_params).and_then(answer));
        };
        self.code_serve(&server, name, payload.to_string(), super::wasm_host::Answer::Fenix(Box::new(then)));
        Ok(())
    }

    /// Answers one control request.
    pub(super) fn apply_control(&mut self, call: Call) -> Result<Value, RpcError> {
        match call {
            Call::Open(params) => {
                let files = crate::ipc::positions(&params.paths);
                if params.new_window {
                    // Opened by `about_to_wait`, which has the event loop
                    // a window needs; the answer doesn't wait for it.
                    self.pending_frames.push((files, params.focus));
                } else {
                    self.open_positions(&files, params.focus);
                }
                Ok(json!({}))
            }
            Call::OpenProject(params) => {
                self.open_project_from_outside(&params)?;
                self.raise_window();
                Ok(json!({}))
            }
            Call::State => Ok(to_value(self.control_state())),
            Call::Commands => Ok(to_value(control_commands(self))),
            Call::RunCommand(params) => match self.run_external_command(&params.id) {
                Ok(()) => {
                    self.raise_window();
                    self.wake_caret();
                    Ok(json!({}))
                }
                Err(ExternalError::Unknown) => Err(RpcError::invalid_params(format!(
                    "no command {}",
                    params.id
                ))),
                Err(ExternalError::NotExternal) => Err(RpcError::invalid_params(format!(
                    "{} can't be run from outside Fenix: it acts on what's in front of you there",
                    params.id
                ))),
            },
            // Answered by the notebook extension (`control_by_extension`)
            // when it's installed.
            Call::NotebookCapture(_) => Err(RpcError::invalid_params("the notebook is an extension (fenix.notebook): SPC h x installs it")),
            // Answered by the agenda extension too.
            Call::AgendaTasks(_) | Call::AgendaClockIn(_) | Call::AgendaClockOut => Err(RpcError::invalid_params("the agenda is an extension (fenix.agenda): SPC h x installs it")),
            Call::Diagnostics(params) => Ok(self.control_diagnostics(params.get("path").and_then(Value::as_str).map(PathBuf::from))),
            Call::Buffers => Ok(self.control_buffers()),
            Call::Selection => Ok(self.control_selection()),
            Call::Tasks => {
                let root = self.integration_root();
                let tasks = self.tasks_for(&root).map_err(RpcError::internal)?;
                Ok(json!({ "root": root.to_string_lossy(), "tasks": tasks.iter().map(|t| json!({ "name": t.name, "command": t.command, "args": t.args })).collect::<Vec<_>>() }))
            }
            Call::TasksRun(params) => {
                self.control_writes_allowed()?;
                let name = params.get("name").and_then(Value::as_str).ok_or_else(|| RpcError::invalid_params("name: which task"))?;
                let root = self.integration_root();
                let task = self.tasks_for(&root).map_err(RpcError::internal)?.into_iter().find(|t| t.name == name).ok_or_else(|| RpcError::invalid_params(format!("no task {name}")))?;
                self.run_task(task, root);
                Ok(json!({ "started": name, "note": "its output is in Fenix's task pane; problems go to the quickfix list" }))
            }
            Call::AgentCommand(params) => {
                self.control_writes_allowed()?;
                let id = params.get("id").and_then(Value::as_str).ok_or_else(|| RpcError::invalid_params("id: which command"))?;
                match self.run_external_command(id) {
                    Ok(()) => Ok(json!({})),
                    Err(ExternalError::Unknown) => Err(RpcError::invalid_params(format!("no command {id}"))),
                    Err(ExternalError::NotExternal) => Err(RpcError::invalid_params(format!("{id} acts on what's in front of you in Fenix: not from outside"))),
                }
            }
        }
    }

    /// Whether agents may run things through Fenix's MCP server.
    fn control_writes_allowed(&self) -> Result<(), RpcError> {
        if self.config.bool_value("agents.mcp.fenix_write").unwrap_or(false) {
            Ok(())
        } else {
            Err(RpcError::invalid_params("running things through Fenix is off: agents.mcp.fenix_write"))
        }
    }

    /// The language servers' problems: in `path`, or in every open file.
    fn control_diagnostics(&self, path: Option<PathBuf>) -> Value {
        let paths: Vec<PathBuf> = match path {
            Some(p) => vec![p],
            None => self.buffers.ids_sorted_by_path().into_iter().filter_map(|id| self.buffers.get(id).and_then(|b| b.buffer.path()).map(Path::to_path_buf)).collect(),
        };
        let mut out = Vec::new();
        for path in paths {
            let canonical = fenix_lsp::normalize(std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone()));
            for d in self.all_diagnostics(&canonical) {
                let severity = match d.severity {
                    Some(lsp_types::DiagnosticSeverity::ERROR) => "error",
                    Some(lsp_types::DiagnosticSeverity::WARNING) => "warning",
                    Some(lsp_types::DiagnosticSeverity::INFORMATION) => "information",
                    _ => "hint",
                };
                out.push(json!({ "path": canonical.to_string_lossy(), "line": d.range.start.line + 1, "column": d.range.start.character + 1, "endLine": d.range.end.line + 1, "severity": severity, "message": d.message, "source": d.source }));
            }
        }
        json!({ "diagnostics": out })
    }

    /// The open files.
    fn control_buffers(&self) -> Value {
        let focused = self.focused_buffer_id();
        let buffers: Vec<Value> = self
            .buffers
            .ids_sorted_by_path()
            .into_iter()
            .filter_map(|id| {
                let ob = self.buffers.get(id)?;
                let path = ob.buffer.path()?;
                Some(json!({ "path": fenix_lsp::normalize(path.to_path_buf()).to_string_lossy(), "unsaved": ob.buffer.is_dirty(), "focused": id == focused }))
            })
            .collect();
        json!({ "buffers": buffers })
    }

    /// What's selected: the editor's, or what was last sent to an agent
    /// (`SPC a e`) when the agent's panel has the focus.
    fn control_selection(&self) -> Value {
        if let Some(s) = self.agents_selection() {
            return json!({ "path": s.0, "line": s.1, "text": s.2 });
        }
        let (here, info) = self.editor_where();
        json!({ "path": here.path, "line": here.line, "text": here.text, "selected": info.is_some_and(|i| i.selected) })
    }

    /// Opens each file the way picking it in `SPC f f` does, moving the
    /// cursor to its line and column when given (the view follows the
    /// cursor on the next frame), then brings the window forward if
    /// `focus`. The last file ends up focused. An empty list only
    /// brings the window forward: a bare relaunch.
    pub(crate) fn open_positions(&mut self, files: &[PathPosition], focus: bool) {
        for file in files {
            self.open_startup_file(&file.path);
            if let Some(line) = file.line {
                // A PDF (an extension's) or a hex view has no cursor line
                // to move to.
                if !Self::looks_like_pdf(&file.path) && !Self::looks_binary(&file.path) {
                    self.jump_to_grep_match(&fenix_project::GrepMatch {
                        path: file.path.clone(),
                        line: line.max(1),
                        col: file.column.unwrap_or(1).max(1),
                        text: String::new(),
                    });
                }
            }
        }
        if focus {
            self.raise_window();
        }
        self.wake_caret();
    }

    /// `fenix/openProject`: what choosing `root` in `SPC p p` does --
    /// it becomes the project, joins the known projects, and the file
    /// finder opens on it.
    fn open_project_from_outside(&mut self, params: &OpenProjectParams) -> Result<(), RpcError> {
        let root = Path::new(&params.root);
        if !root.is_dir() {
            return Err(RpcError::invalid_params(format!(
                "{} isn't a folder",
                params.root
            )));
        }
        let root = std::fs::canonicalize(root)
            .map(fenix_lsp::normalize)
            .unwrap_or_else(|_| root.to_path_buf());
        self.switch_to_project(root);
        Ok(())
    }

    /// `fenix/state`: the project, the file in focus and the cursor's
    /// line, in plain form (no `\\?\`). A page or tool in focus has no
    /// file to report.
    pub(super) fn control_state(&self) -> StateResult {
        let plain = |p: &Path| {
            fenix_lsp::normalize(p.to_path_buf())
                .to_string_lossy()
                .into_owned()
        };
        let mut state = StateResult {
            project: self.project_root.as_deref().map(plain),
            ..StateResult::default()
        };
        if self.main_view == MainView::Editor {
            if let Some(ob) = self.buffers.get(self.focused_buffer_id()) {
                if let Some(path) = ob.buffer.path() {
                    state.file = Some(plain(path));
                    state.line = Some(ob.buffer.line_col(&self.cursor()).0 + 1);
                }
            }
        }
        state
    }

    /// Brings this Fenix's window to the front, un-minimizing it first:
    /// winit only raises a window that's visible and not minimized. On
    /// Windows this works when the launch that asked passed on its
    /// foreground right (`ipc::allow_foreground`); otherwise Windows
    /// flashes the taskbar button instead.
    pub(super) fn raise_window(&self) {
        if let Some(window) = &self.window {
            if window.is_minimized() == Some(true) {
                window.set_minimized(false);
            }
            window.focus_window();
            window.request_redraw();
        }
    }

    /// The command line, when this launch is the editor: the project,
    /// then the files after the first (`App::new` opened that one),
    /// each at its position. With files to show, the project's file
    /// finder is closed again rather than left over them.
    pub(crate) fn apply_startup(&mut self, files: &[PathPosition], project: Option<&Path>) {
        if let Some(project) = project {
            if let Err(e) = self.open_project_from_outside(&OpenProjectParams {
                root: project.to_string_lossy().into_owned(),
            }) {
                eprintln!("fenix: --project: {}", e.message);
            } else if !files.is_empty() {
                self.active_picker = None;
                self.main_view = MainView::Editor;
            }
        }
        if let Some(first) = files.first() {
            if project.is_some() {
                self.open_positions(std::slice::from_ref(first), false);
            } else if let Some(line) = first.line {
                if !Self::looks_like_pdf(&first.path) && !Self::looks_binary(&first.path) {
                    self.jump_to_grep_match(&fenix_project::GrepMatch {
                        path: first.path.clone(),
                        line: line.max(1),
                        col: first.column.unwrap_or(1).max(1),
                        text: String::new(),
                    });
                }
            }
        }
        if files.len() > 1 {
            self.open_positions(&files[1..], false);
        }
    }
}

/// `fenix/commands`: every command in the registry, in its order, with
/// the leader keys that run it and whether it may run from outside.
pub(crate) fn control_commands(app: &App) -> CommandsResult {
    let keys = crate::keymap::leader_keys();
    CommandsResult {
        commands: app
            .commands()
            .iter()
            .map(|c| CommandInfo {
                id: c.id.to_string(),
                description: c.description.to_string(),
                keys: keys.get(c.id.as_ref()).cloned(),
                external: c.external(),
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fenix_control::{AgendaTask, OpenParams, OpenPath, TaskStatus, TasksParams, TasksResult};

    struct TempDir(tempfile::TempDir);
    impl TempDir {
        fn new(_name: &str) -> Self {
            TempDir(tempfile::tempdir().unwrap())
        }
        fn path(&self) -> &Path {
            self.0.path()
        }
        fn write(&self, name: &str, contents: &str) -> PathBuf {
            let path = self.0.path().join(name);
            std::fs::write(&path, contents).unwrap();
            path
        }
    }

    fn at(path: &Path, line: Option<usize>, column: Option<usize>) -> PathPosition {
        PathPosition {
            path: path.to_path_buf(),
            line,
            column,
        }
    }

    fn cursor_line_col(app: &App) -> (usize, usize) {
        app.open().buffer.line_col(&app.cursor())
    }

    #[test]
    fn open_moves_the_cursor_to_the_line_and_column() {
        let dir = TempDir::new("control-open");
        let file = dir.write("x.rs", "one\ntwo\nthree three\nfour\n");
        let mut app = App::with_file(None);
        let params = OpenParams {
            paths: vec![OpenPath {
                path: file.display().to_string(),
                line: Some(3),
                column: Some(7),
            }],
            new_window: false,
            focus: true,
        };
        assert_eq!(app.apply_control(Call::Open(params)).unwrap(), json!({}));
        assert_eq!(app.open().buffer.text(), "one\ntwo\nthree three\nfour\n");
        // 1-based in, 0-based inside.
        assert_eq!(cursor_line_col(&app), (2, 6));
    }

    #[test]
    fn a_position_past_the_end_lands_on_the_last_line() {
        let dir = TempDir::new("control-past-end");
        let file = dir.write("x.rs", "a\nbb\n");
        let mut app = App::with_file(None);
        app.open_positions(&[at(&file, Some(99), Some(99))], false);
        let (line, _) = cursor_line_col(&app);
        assert!(line <= 2, "{line}");
    }

    #[test]
    fn several_files_end_on_the_last_one_each_at_its_line() {
        let dir = TempDir::new("control-several");
        let a = dir.write("a.txt", "a1\na2\na3\n");
        let b = dir.write("b.txt", "b1\nb2\n");
        let mut app = App::with_file(None);
        app.open_positions(&[at(&a, Some(3), None), at(&b, Some(2), None)], true);
        assert_eq!(app.main_view, MainView::Editor);
        assert_eq!(app.open().buffer.text(), "b1\nb2\n");
        assert_eq!(cursor_line_col(&app), (1, 0));
        let state = app.control_state();
        assert_eq!(state.line, Some(2));
        assert!(state.file.unwrap().ends_with("b.txt"));
    }

    #[test]
    fn an_empty_open_only_brings_fenix_forward() {
        // Headless, there's no window to raise: this only has to not
        // panic, and not open anything.
        let mut app = App::with_file(None);
        let before = app.focused_buffer_id();
        app.apply_control(Call::Open(OpenParams {
            paths: vec![],
            new_window: false,
            focus: true,
        }))
        .unwrap();
        assert_eq!(app.focused_buffer_id(), before);
    }

    #[test]
    fn open_in_a_new_window_waits_for_the_event_loop() {
        let mut app = App::with_file(None);
        let params = OpenParams {
            paths: vec![OpenPath {
                path: "C:/x.rs".into(),
                line: Some(2),
                column: None,
            }],
            new_window: true,
            focus: false,
        };
        app.apply_control(Call::Open(params)).unwrap();
        assert_eq!(
            app.pending_frames,
            vec![(vec![at(Path::new("C:/x.rs"), Some(2), None)], false)]
        );
    }

    #[test]
    fn open_project_is_what_spc_p_p_does() {
        let known_dir = TempDir::new("control-known");
        let project = TempDir::new("control-project");
        let mut app = App::with_file(None);
        app.known_projects =
            fenix_project::KnownProjects::load_or_default(known_dir.path().join("projects.txt"));
        let root = project.path().display().to_string();
        assert_eq!(
            app.apply_control(Call::OpenProject(OpenProjectParams { root }))
                .unwrap(),
            json!({})
        );
        let canonical = fenix_lsp::normalize(std::fs::canonicalize(project.path()).unwrap());
        assert_eq!(app.project_root.as_deref(), Some(canonical.as_path()));
        assert_eq!(app.known_projects.roots(), std::slice::from_ref(&canonical));
        assert!(matches!(app.active_picker, Some(ActivePicker::FindFile(_))));
        let state = app.control_state();
        assert_eq!(
            state.project.as_deref(),
            Some(canonical.to_string_lossy().as_ref())
        );
        assert!(!state.project.unwrap().starts_with(r"\\?\"));
    }

    #[test]
    fn open_project_refuses_what_isnt_a_folder() {
        let dir = TempDir::new("control-not-folder");
        let file = dir.write("f.txt", "x");
        let mut app = App::with_file(None);
        let err = app
            .apply_control(Call::OpenProject(OpenProjectParams {
                root: file.display().to_string(),
            }))
            .unwrap_err();
        assert_eq!(err.code, fenix_control::protocol::code::INVALID_PARAMS);
        assert_eq!(app.project_root, None);
    }

    #[test]
    fn state_reports_no_file_when_nothing_is_on_disk() {
        let app = App::with_file(None);
        let state = app.control_state();
        assert_eq!(state.file, None);
        assert_eq!(state.line, None);
    }

    #[test]
    fn startup_positions_the_first_file_too() {
        let dir = TempDir::new("control-startup");
        let a = dir.write("a.txt", "a1\na2\na3\n");
        let mut app = App::with_file(Some(a.display().to_string()));
        app.apply_startup(&[at(&a, Some(2), Some(2))], None);
        assert_eq!(cursor_line_col(&app), (1, 1));
    }

    #[test]
    fn startup_with_a_project_and_files_shows_the_files() {
        let dir = TempDir::new("control-startup-project");
        let known_dir = TempDir::new("control-startup-known");
        // A real project, so opening its files keeps it the project.
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        let a = dir.write("a.txt", "a1\na2\n");
        let b = dir.write("b.txt", "b1\nb2\n");
        let mut app = App::with_file(Some(a.display().to_string()));
        app.known_projects =
            fenix_project::KnownProjects::load_or_default(known_dir.path().join("projects.txt"));
        app.apply_startup(
            &[at(&a, Some(2), None), at(&b, None, None)],
            Some(dir.path()),
        );
        assert!(app.project_root.is_some());
        assert!(app.active_picker.is_none());
        assert_eq!(app.main_view, MainView::Editor);
        assert_eq!(app.open().buffer.text(), "b1\nb2\n");
    }

    // -- Commands -----------------------------------------------------------

    #[test]
    fn commands_carry_their_keys_and_whether_they_run_from_outside() {
        let commands = control_commands(&App::with_file(None)).commands;
        let get = |id: &str| {
            commands
                .iter()
                .find(|c| c.id == id)
                .unwrap_or_else(|| panic!("no {id}"))
        };
        let switch = get("project.switch_project");
        assert!(switch.external);
        assert!(switch.keys.is_some());
        assert!(!switch.description.is_empty());
        let save = get("file.save");
        assert!(!save.external);
        assert_eq!(save.keys.as_deref(), Some("SPC f s"));
        // Bound twice: the shorter sequence is the one listed.
        assert_eq!(get("project.find_file").keys.as_deref(), Some("SPC SPC"));
        // Run by a Ctrl chord, not the leader: no keys.
        assert_eq!(get("nav.jump_back").keys, None);
        for id in [
            "project.switch_project",
            "snippets.open",
        ] {
            assert!(get(id).external, "{id}");
        }
        for id in ["edit.undo", "git.hunk_stage", "app.quit", "frame.new"] {
            assert!(!get(id).external, "{id}");
        }
        let mut ids: Vec<&str> = commands.iter().map(|c| c.id.as_str()).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(
            ids.len(),
            commands.len(),
            "a command id is registered twice"
        );
    }

    /// An app of its own, the file it opens under `dir`.
    fn isolated(_dir: &Path, file: Option<&Path>) -> App {
        App::with_file(file.map(|f| f.display().to_string()))
    }

    /// What a command visibly did: what's in front, and what it said.
    #[derive(Debug, PartialEq)]
    struct Outcome {
        view: MainView,
        picker: Option<std::mem::Discriminant<ActivePicker>>,
        prompt: Option<String>,
        status: Option<(String, bool)>,
        /// What the focus moved to, when it moved.
        focused: Option<String>,
    }

    fn outcome(app: &App, before: BufferId) -> Outcome {
        let now = app.focused_buffer_id();
        Outcome {
            view: app.main_view,
            picker: app.active_picker.as_ref().map(std::mem::discriminant),
            prompt: app.active_prompt_text(),
            status: app
                .status_message
                .as_ref()
                .map(|m| (m.text.clone(), m.is_error)),
            focused: (now != before).then(|| app.buffer_display_name(now)),
        }
    }

    /// The guard on `external`: each external command is run once with
    /// a text buffer in focus and once with a page in focus (a hex view,
    /// which no external command opens, the same text buffer still open
    /// behind it), and must do the same thing both times without
    /// touching that buffer. A command that acts on "the buffer in front
    /// of you" -- saving it, undoing in it, staging its hunk -- does
    /// something else, or nothing, with a page in front, and fails here.
    #[test]
    fn external_commands_dont_assume_a_focused_text_buffer() {
        let registry = crate::commands::CommandRegistry::with_builtins();
        let external: Vec<&str> = registry
            .iter()
            .filter(|c| c.external())
            .map(|c| c.id.as_ref())
            .collect();
        assert!(external.len() > 20, "{external:?}");
        let mut differ = Vec::new();
        for id in external {
            let mut outcomes = Vec::new();
            for page in [false, true] {
                let dir = tempfile::tempdir().unwrap();
                let file = dir.path().join("notes.txt");
                std::fs::write(&file, "one\ntwo\n").unwrap();
                let mut app = isolated(dir.path(), Some(&file));
                let text_buffer = app.focused_buffer_id();
                // An edit, saved: something to undo, nothing unsaved.
                let (buffer, cursor) = app.focused_buffer_and_cursor_mut();
                buffer.insert_str(cursor, "zero\n");
                buffer.save().unwrap();
                if page {
                    // A page no external command opens itself: a
                    // Markdown file's reading view.
                    let readme = dir.path().join("README.md");
                    std::fs::write(&readme, "# Read me
").unwrap();
                    app.open_file_from_picker(&readme);
                    app.open_reading_view(true);
                    assert_ne!(app.focused_buffer_id(), text_buffer);
                    assert_ne!(app.open().kind, BufferKind::Text);
                }
                app.status_message = None;
                let before = app.focused_buffer_id();
                registry.run_external(&mut app, id).unwrap();
                outcomes.push(outcome(&app, before));
                let ob = app
                    .buffers
                    .get(text_buffer)
                    .expect("the text buffer is still open");
                assert_eq!(
                    ob.buffer.text(),
                    "zero\none\ntwo\n",
                    "{id} changed the text buffer"
                );
                assert!(!ob.buffer.is_dirty(), "{id} changed the text buffer");
            }
            if outcomes[0] != outcomes[1] {
                differ.push(format!(
                    "{id}
  text buffer in front: {:?}
  page in front:        {:?}",
                    outcomes[0], outcomes[1]
                ));
            }
        }
        assert!(
            differ.is_empty(),
            "external commands that do something else when no text buffer is in focus:
{}",
            differ.join(
                "
"
            )
        );
    }

    #[test]
    fn run_command_runs_external_commands_only() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = isolated(dir.path(), None);
        let run = |app: &mut App, id: &str| {
            app.apply_control(Call::RunCommand(fenix_control::RunCommandParams {
                id: id.into(),
            }))
        };
        assert_eq!(run(&mut app, "snippets.open").unwrap(), json!({}));
        let err = run(&mut app, "file.save").unwrap_err();
        assert_eq!(err.code, fenix_control::protocol::code::INVALID_PARAMS);
        assert!(err.message.contains("outside"), "{}", err.message);
        let err = run(&mut app, "no.such.command").unwrap_err();
        assert_eq!(err.code, fenix_control::protocol::code::INVALID_PARAMS);
    }

    // -- Notebook -----------------------------------------------------------

    /// The notebook is an extension: `fenix/notebook.capture` waits for
    /// its answer, the file the line went into. Without it, it says so.
    #[test]
    fn a_capture_is_answered_by_the_notebook_extension() {
        let mut app = App::with_file(None);
        let params = || fenix_control::CaptureParams { text: "call OBSW".into(), target: CaptureTarget::Inbox };
        let err = app.apply_control(Call::NotebookCapture(params())).unwrap_err();
        assert!(err.message.contains("fenix.notebook"), "{}", err.message);
        let (reply, answer) = std::sync::mpsc::channel();
        assert!(app.control_by_extension(Call::NotebookCapture(params()), reply.clone()).is_err(), "nothing serves it");

        let mut notebook = super::super::wasm_host::tests::hello("");
        notebook.manifest = fenix_extensions::Manifest::parse(
            "[extension]
id = \"test.notebook\"
name = \"Notebook\"
version = \"1.0.0\"
runtime = \"wasm\"

[engine]
api = \"0.7\"
fenix = \">=1.0, <2\"

[contributes]
services = [\"notebook.capture\"]
",
        )
        .unwrap();
        notebook.id = "test.notebook".into();
        app.wasm.installed = vec![notebook];
        assert!(app.control_by_extension(Call::NotebookCapture(params()), reply).is_ok());
        let started = std::time::Instant::now();
        let said = loop {
            app.pump_wasm();
            if let Ok(said) = answer.try_recv() {
                break said;
            }
            assert!(started.elapsed() < std::time::Duration::from_secs(60), "{}", app.extension_output_text());
            std::thread::sleep(std::time::Duration::from_millis(20));
        };
        let result: CaptureResult = serde_json::from_value(said.unwrap()).unwrap();
        assert_eq!(result.entry, "/nb/notes/inbox.md");
    }

    // -- Agenda -------------------------------------------------------------

    /// The agenda's methods are its extension's: the reply waits for its
    /// answer. Without one, they say so.
    #[test]
    fn an_agenda_call_is_answered_by_the_extension_that_serves_it() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = isolated(dir.path(), None);
        let (reply, answer) = std::sync::mpsc::channel();
        assert!(app.control_by_extension(Call::AgendaTasks(TasksParams { status: None }), reply.clone()).is_err(), "nothing serves it");
        assert!(app.apply_control(Call::AgendaTasks(TasksParams { status: None })).is_err());

        // The test guest, as an agenda: its services are named for it.
        let mut agenda = super::super::wasm_host::tests::hello("");
        agenda.manifest = fenix_extensions::Manifest::parse(
            "[extension]\nid = \"test.agenda\"\nname = \"Agenda\"\nversion = \"1.0.0\"\nruntime = \"wasm\"\n\n[engine]\napi = \"0.6\"\nfenix = \">=1.0, <2\"\n\n[contributes]\nservices = [\"agenda.tasks\", \"agenda.timer\"]\n",
        )
        .unwrap();
        agenda.id = "test.agenda".into();
        app.wasm.installed = vec![agenda];
        assert!(app.control_by_extension(Call::AgendaTasks(TasksParams { status: None }), reply.clone()).is_ok());
        let started = std::time::Instant::now();
        let said = loop {
            app.pump_wasm();
            if let Ok(said) = answer.try_recv() {
                break said;
            }
            assert!(started.elapsed() < std::time::Duration::from_secs(60), "{}", app.extension_output_text());
            std::thread::sleep(std::time::Duration::from_millis(20));
        };
        let tasks: TasksResult = serde_json::from_value(said.unwrap()).unwrap();
        assert_eq!(tasks.tasks, [AgendaTask { id: 7, title: "From the guest".into(), status: TaskStatus::InProgress, jira_key: Some("FEN-1".into()), elapsed_minutes: 30 }]);

        // `fenix/state`: Fenix's, with the extension's clock.
        assert!(app.control_by_extension(Call::State, reply).is_ok());
        let started = std::time::Instant::now();
        let said = loop {
            app.pump_wasm();
            if let Ok(said) = answer.try_recv() {
                break said;
            }
            assert!(started.elapsed() < std::time::Duration::from_secs(60), "{}", app.extension_output_text());
            std::thread::sleep(std::time::Duration::from_millis(20));
        };
        let state: StateResult = serde_json::from_value(said.unwrap()).unwrap();
        assert_eq!(state.agenda_timer.map(|t| (t.task_id, t.title)), Some((7, "From the guest".to_string())));
    }

    #[test]
    fn the_app_owns_one_registry_and_runs_commands_from_it_by_id() {
        let mut app = App::with_file(None);
        assert_eq!(
            app.commands().iter().count(),
            crate::commands::CommandRegistry::with_builtins()
                .iter()
                .count()
        );
        assert!(app
            .commands()
            .iter()
            .all(|c| !c.id.is_empty() && !c.description.is_empty()));
        assert_eq!(app.run_external_command("snippets.open"), Ok(()));
        assert_eq!(
            app.run_external_command("file.save"),
            Err(ExternalError::NotExternal),
            "acts on what's in front of you"
        );
        assert_eq!(
            app.run_external_command("no.such.command"),
            Err(ExternalError::Unknown)
        );
    }
}

#[cfg(test)]
mod agent_tool_tests {
    use super::*;

    #[test]
    fn open_files_say_which_have_unsaved_edits() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.rs");
        std::fs::write(&file, "fn a() {}\n").unwrap();
        let mut app = App::with_file(None);
        app.test_open_path(&file);
        let id = app.buffers.id_for_path(&file).unwrap();
        app.buffers.get_mut(id).unwrap().buffer.mark_unsaved();
        let answer = app.apply_control(Call::Buffers).unwrap();
        let row = answer["buffers"].as_array().unwrap().iter().find(|b| b["path"].as_str().unwrap().ends_with("a.rs")).unwrap().clone();
        assert_eq!((row["unsaved"].clone(), row["focused"].clone()), (json!(true), json!(true)));
        assert_eq!(app.apply_control(Call::Diagnostics(json!({}))).unwrap(), json!({ "diagnostics": [] }));
    }

    #[test]
    fn running_things_needs_the_setting() {
        let mut app = App::with_file(None);
        app.config.set_value("agents.mcp.fenix_write", Some(fenix_config::Value::Bool(false)));
        assert!(app.apply_control(Call::TasksRun(json!({ "name": "build" }))).is_err());
        assert!(app.apply_control(Call::AgentCommand(json!({ "id": "view.toggle_animations" }))).is_err());
        app.config.set_value("agents.mcp.fenix_write", Some(fenix_config::Value::Bool(true)));
        assert!(app.apply_control(Call::AgentCommand(json!({ "id": "no.such" }))).unwrap_err().message.contains("no command"));
    }
}
