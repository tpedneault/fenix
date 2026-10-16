//! The editor's answers to the control channel (`crate::ipc`): every
//! request the protocol itself can't answer arrives here, on the UI
//! thread, as a `FenixUserEvent::Control`. The command line ends
//! up here too when this launch is the editor, so `fenix x.rs:12` means
//! the same whether or not Fenix was already running.

use fenix_control::protocol::OpenProjectParams;
use fenix_control::{
    AgendaTask, AgendaTimer, Call, CaptureParams, CaptureResult, CaptureTarget, CommandInfo,
    CommandsResult, RpcError, StateResult, TaskStatus, TasksParams, TasksResult,
};
use fenix_tasks::PathPosition;
use serde_json::{json, Value};

use super::notebook_capture::CaptureTo;
use super::*;
use crate::commands::{CommandRegistry, ExternalError};

/// The agenda's status as the protocol writes it.
fn task_status(status: fenix_agenda::Status) -> TaskStatus {
    match status {
        fenix_agenda::Status::Todo => TaskStatus::Todo,
        fenix_agenda::Status::InProgress => TaskStatus::InProgress,
        fenix_agenda::Status::Blocked => TaskStatus::Blocked,
        fenix_agenda::Status::Done => TaskStatus::Done,
    }
}

fn to_value(value: impl serde::Serialize) -> Value {
    serde_json::to_value(value).expect("plain data serializes")
}

impl App {
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
            Call::Commands => Ok(to_value(control_commands())),
            Call::RunCommand(params) => {
                match CommandRegistry::with_builtins().run_external(self, &params.id) {
                    Ok(()) => {
                        self.raise_window();
                        self.wake_caret();
                        Ok(json!({}))
                    }
                    Err(ExternalError::Unknown) => {
                        Err(RpcError::invalid_params(format!("no command {}", params.id)))
                    }
                    Err(ExternalError::NotExternal) => Err(RpcError::invalid_params(format!(
                        "{} can't be run from outside Fenix: it acts on what's in front of you there",
                        params.id
                    ))),
                }
            }
            Call::NotebookCapture(params) => self.control_capture(&params).map(to_value),
            Call::AgendaTasks(params) => Ok(to_value(self.control_tasks(&params))),
            Call::AgendaClockIn(params) => {
                let id = TaskId(params.task_id);
                if !self
                    .features
                    .agenda()
                    .store
                    .task(id)
                    .is_some_and(|t| !t.archived)
                {
                    return Err(RpcError::invalid_params(format!(
                        "no agenda task {}",
                        params.task_id
                    )));
                }
                self.agenda_clock_pick(super::agenda_host::ClockPick::Start(id));
                Ok(json!({}))
            }
            Call::AgendaClockOut => {
                if self.features.agenda().store.active_timer.is_some() {
                    self.agenda_clock_pick(super::agenda_host::ClockPick::Stop);
                }
                Ok(json!({}))
            }
        }
    }

    /// `fenix/notebook.capture`: the line filed as `SPC n c` files it,
    /// without a link back (there's no "here" for a caller outside) and
    /// without taking the focus from whatever the user is in.
    fn control_capture(&mut self, params: &CaptureParams) -> Result<CaptureResult, RpcError> {
        let to = match params.target {
            CaptureTarget::Inbox => CaptureTo::Inbox,
            CaptureTarget::Journal => CaptureTo::Journal,
        };
        let path = self
            .notebook_capture_into(to, false, None, &params.text)
            .map_err(RpcError::internal)?;
        self.set_message(format!("captured to {}", to.label()));
        Ok(CaptureResult {
            entry: fenix_lsp::normalize(path).to_string_lossy().into_owned(),
        })
    }

    /// `fenix/agenda.tasks`: the tasks not archived, in board order
    /// (by status, then as arranged within it), only those in
    /// `params.status` when given.
    fn control_tasks(&self, params: &TasksParams) -> TasksResult {
        let store = &self.features.agenda().store;
        let mut tasks: Vec<&fenix_agenda::Task> = store
            .tasks
            .iter()
            .filter(|t| !t.archived)
            .filter(|t| {
                params
                    .status
                    .as_ref()
                    .is_none_or(|wanted| wanted.contains(&task_status(t.status)))
            })
            .collect();
        tasks.sort_by_key(|t| {
            (
                fenix_agenda::Status::ALL
                    .iter()
                    .position(|s| *s == t.status),
                t.order,
            )
        });
        TasksResult {
            tasks: tasks
                .into_iter()
                .map(|t| AgendaTask {
                    id: t.id.0,
                    title: t.title.clone(),
                    status: task_status(t.status),
                    jira_key: t.jira_key().map(str::to_string),
                    elapsed_minutes: store.elapsed_on(t.id).num_minutes(),
                })
                .collect(),
        }
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
                // A PDF or a hex view has no cursor line to move to.
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
            agenda_timer: self
                .features
                .agenda()
                .store
                .active_timer
                .as_ref()
                .map(|timer| AgendaTimer {
                    task_id: timer.task_id.0,
                    title: self
                        .features
                        .agenda()
                        .store
                        .task(timer.task_id)
                        .map(|t| t.title.clone())
                        .unwrap_or_default(),
                    since: timer.started_at.to_rfc3339(),
                }),
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
pub(crate) fn control_commands() -> CommandsResult {
    let keys = crate::keymap::leader_keys();
    CommandsResult {
        commands: CommandRegistry::with_builtins()
            .iter()
            .map(|c| CommandInfo {
                id: c.id.to_string(),
                description: c.description.to_string(),
                keys: keys.get(c.id).cloned(),
                external: c.external(),
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fenix_control::{OpenParams, OpenPath};

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
        let commands = control_commands().commands;
        let get = |id: &str| {
            commands
                .iter()
                .find(|c| c.id == id)
                .unwrap_or_else(|| panic!("no {id}"))
        };
        let agenda = get("agenda.open");
        assert!(agenda.external);
        assert_eq!(agenda.keys.as_deref(), Some("SPC a a"));
        assert!(!agenda.description.is_empty());
        let save = get("file.save");
        assert!(!save.external);
        assert_eq!(save.keys.as_deref(), Some("SPC f s"));
        // Bound twice: the shorter sequence is the one listed.
        assert_eq!(get("project.find_file").keys.as_deref(), Some("SPC SPC"));
        // Run by a Ctrl chord, not the leader: no keys.
        assert_eq!(get("nav.jump_back").keys, None);
        for id in [
            "project.switch_project",
            "agenda.open",
            "notebook.journal",
            "agenda.toggle_clock",
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

    /// An app whose agenda and notebook live in `dir`, never the ones of
    /// the Fenix you use, with one task to clock in on.
    fn isolated(dir: &Path, file: Option<&Path>) -> App {
        let mut app = App::with_file(file.map(|f| f.display().to_string()));
        app.features.agenda_mut().path = dir.join("agenda.json");
        app.features.agenda_mut().store = fenix_agenda::AgendaStore::default();
        app.features.agenda_mut().store.create_task(
            "Write the report".into(),
            String::new(),
            fenix_agenda::Priority::Medium,
            None,
        );
        app.config.notebook_folder = Some(dir.join("nb"));
        app
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
        let registry = CommandRegistry::with_builtins();
        let external: Vec<&str> = registry
            .iter()
            .filter(|c| c.external())
            .map(|c| c.id)
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
                    let bytes = dir.path().join("frame.bin");
                    std::fs::write(&bytes, [0x1a, 0xcf, 0xfc, 0x1d]).unwrap();
                    app.open_hex_view(bytes);
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
        assert_eq!(run(&mut app, "agenda.open").unwrap(), json!({}));
        assert!(matches!(
            app.pages.get(&app.focused_buffer_id()).map(|p| &p.model),
            Some(super::super::pages::PageModel::Agenda(_))
        ));
        let err = run(&mut app, "file.save").unwrap_err();
        assert_eq!(err.code, fenix_control::protocol::code::INVALID_PARAMS);
        assert!(err.message.contains("outside"), "{}", err.message);
        let err = run(&mut app, "no.such.command").unwrap_err();
        assert_eq!(err.code, fenix_control::protocol::code::INVALID_PARAMS);
    }

    // -- Notebook -----------------------------------------------------------

    fn capture(app: &mut App, text: &str, target: CaptureTarget) -> PathBuf {
        let reply = app
            .apply_control(Call::NotebookCapture(CaptureParams {
                text: text.into(),
                target,
            }))
            .unwrap();
        PathBuf::from(reply["entry"].as_str().unwrap())
    }

    #[test]
    fn capture_writes_the_inbox_or_journal_through_the_notebook() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = isolated(dir.path(), None);
        let inbox = capture(&mut app, "call OBSW", CaptureTarget::Inbox);
        let text = std::fs::read_to_string(&inbox).unwrap();
        assert!(
            text.starts_with("# Inbox") && text.trim_end().ends_with("call OBSW"),
            "{text}"
        );
        assert!(!inbox.to_string_lossy().starts_with(r"\\?\"));
        let again = capture(&mut app, "and the ground segment", CaptureTarget::Inbox);
        assert_eq!(again, inbox);
        let journal = capture(&mut app, "retry is 3", CaptureTarget::Journal);
        assert_ne!(journal, inbox);
        assert!(std::fs::read_to_string(&journal)
            .unwrap()
            .contains("retry is 3"));
        // The index agrees with the files: a fresh look at the notebook
        // finds both, as they are on disk.
        let fresh = fenix_notebook::Notebook::open(&dir.path().join("nb")).unwrap();
        let entry = fresh.by_name("Inbox").unwrap();
        assert!(entry.text.contains("call OBSW") && entry.text.contains("and the ground segment"));
        assert!(fresh
            .day(chrono::Local::now().date_naive())
            .unwrap()
            .text
            .contains("retry is 3"));
        // Nothing was opened or focused for it.
        assert_eq!(app.open().kind, BufferKind::Dashboard);
    }

    #[test]
    fn capture_into_an_open_buffer_of_the_entry_keeps_it_in_step() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = isolated(dir.path(), None);
        let path = capture(&mut app, "one", CaptureTarget::Inbox);
        app.open_startup_file(&path);
        capture(&mut app, "two", CaptureTarget::Inbox);
        let on_disk = std::fs::read_to_string(&path).unwrap();
        assert!(
            on_disk.contains("one") && on_disk.contains("two"),
            "{on_disk}"
        );
        assert_eq!(app.open().buffer.text(), on_disk);
        assert!(!app.open().buffer.is_dirty());
    }

    // -- Agenda -------------------------------------------------------------

    fn tasks(app: &mut App, status: Option<Vec<TaskStatus>>) -> Vec<AgendaTask> {
        let reply = app
            .apply_control(Call::AgendaTasks(TasksParams { status }))
            .unwrap();
        serde_json::from_value::<TasksResult>(reply).unwrap().tasks
    }

    fn clock_in(app: &mut App, task_id: u32) -> Result<Value, RpcError> {
        app.apply_control(Call::AgendaClockIn(fenix_control::ClockInParams {
            task_id,
        }))
    }

    #[test]
    fn tasks_come_in_board_order_filtered_by_status() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = isolated(dir.path(), None);
        let low = fenix_agenda::Priority::Low;
        let store = &mut app.features.agenda_mut().store;
        let done = store.create_task("Old".into(), String::new(), low, None);
        store.set_status(done, fenix_agenda::Status::Done);
        let doing = store.create_task("Doing".into(), String::new(), low, None);
        store.set_status(doing, fenix_agenda::Status::InProgress);
        store.log_manual_time(doing, chrono::Duration::minutes(90));
        let gone = store.create_task("Gone".into(), String::new(), low, None);
        store.archive(gone);
        let all = tasks(&mut app, None);
        let ids: Vec<u32> = all.iter().map(|t| t.id).collect();
        assert_eq!(ids, vec![0, doing.0, done.0]);
        assert_eq!(all[1].elapsed_minutes, 90);
        assert_eq!(all[1].status, TaskStatus::InProgress);
        assert_eq!(all[0].jira_key, None);
        let open = tasks(
            &mut app,
            Some(vec![TaskStatus::Todo, TaskStatus::InProgress]),
        );
        assert_eq!(
            open.iter().map(|t| t.title.as_str()).collect::<Vec<_>>(),
            ["Write the report", "Doing"]
        );
    }

    #[test]
    fn clock_in_and_out_round_trip_through_agenda_json() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = isolated(dir.path(), None);
        let id = app.features.agenda().store.tasks[0].id;
        clock_in(&mut app, id.0).unwrap();
        let saved = fenix_agenda::load(&app.features.agenda().path);
        assert_eq!(saved.active_timer.as_ref().map(|t| t.task_id), Some(id));
        let timer = app.control_state().agenda_timer.expect("the clock runs");
        assert_eq!(
            (timer.task_id, timer.title.as_str()),
            (id.0, "Write the report")
        );
        let since = chrono::DateTime::parse_from_rfc3339(&timer.since).unwrap();
        assert_eq!(since, saved.active_timer.unwrap().started_at);
        // Backdated, so stopping records a span.
        app.features
            .agenda_mut()
            .store
            .active_timer
            .as_mut()
            .unwrap()
            .started_at -= chrono::Duration::minutes(5);
        app.apply_control(Call::AgendaClockOut).unwrap();
        let saved = fenix_agenda::load(&app.features.agenda().path);
        assert!(saved.active_timer.is_none());
        assert_eq!(saved.task(id).unwrap().time_entries.len(), 1);
        assert_eq!(app.control_state().agenda_timer, None);
        // Stopping a clock that isn't running is fine, and does nothing.
        app.apply_control(Call::AgendaClockOut).unwrap();
        let saved = fenix_agenda::load(&app.features.agenda().path);
        assert_eq!(saved.task(id).unwrap().time_entries.len(), 1);
    }

    #[test]
    fn clock_in_refuses_a_task_that_isnt_there() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = isolated(dir.path(), None);
        let gone = app.features.agenda_mut().store.create_task(
            "Gone".into(),
            String::new(),
            fenix_agenda::Priority::Low,
            None,
        );
        app.features.agenda_mut().store.archive(gone);
        for task_id in [99, gone.0] {
            let err = clock_in(&mut app, task_id).unwrap_err();
            assert_eq!(err.code, fenix_control::protocol::code::INVALID_PARAMS);
        }
        assert!(app.features.agenda().store.active_timer.is_none());
        assert!(!app.features.agenda().path.exists(), "nothing to save");
    }

    /// Sets `path`'s modification time well away from now, so a write
    /// within the same tick of a coarse clock still reads as a change.
    fn backdate(path: &Path) {
        let file = std::fs::File::options().write(true).open(path).unwrap();
        file.set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(60))
            .unwrap();
    }

    #[test]
    fn agenda_json_written_elsewhere_is_read_again() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = isolated(dir.path(), None);
        app.save_agenda();
        // Fenix's own save isn't a change.
        app.reload_agenda_if_changed();
        assert_eq!(app.features.agenda().store.tasks.len(), 1);
        let mut theirs = fenix_agenda::load(&app.features.agenda().path);
        theirs.create_task(
            "From the laptop".into(),
            String::new(),
            fenix_agenda::Priority::High,
            None,
        );
        fenix_agenda::save(&app.features.agenda().path, &theirs).unwrap();
        backdate(&app.features.agenda().path);
        app.reload_agenda_if_changed();
        assert_eq!(app.features.agenda().store.tasks.len(), 2);
        // And Fenix's next save keeps their task.
        let first = app.features.agenda().store.tasks[0].id.0;
        clock_in(&mut app, first).unwrap();
        assert_eq!(
            fenix_agenda::load(&app.features.agenda().path).tasks.len(),
            2
        );
    }

    #[test]
    fn an_unreadable_agenda_json_keeps_the_agenda_in_memory() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = isolated(dir.path(), None);
        app.save_agenda();
        std::fs::write(&app.features.agenda().path, b"{ half written").unwrap();
        backdate(&app.features.agenda().path);
        app.reload_agenda_if_changed();
        assert_eq!(app.features.agenda().store.tasks.len(), 1);
        assert!(app
            .status_message
            .as_ref()
            .is_some_and(|m| m.is_error && m.text.contains("agenda.json")));
        // Said once, not on every tick.
        app.status_message = None;
        app.reload_agenda_if_changed();
        assert!(app.status_message.is_none());
    }
}
