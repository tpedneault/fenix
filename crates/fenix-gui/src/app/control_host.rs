//! The editor's answers to the control channel (`crate::ipc`): every
//! `fenix/open`, `fenix/openProject` and `fenix/state` arrives here, on
//! the UI thread, as a `FenixUserEvent::Control`. The command line ends
//! up here too when this launch is the editor, so `fenix x.rs:12` means
//! the same whether or not Fenix was already running.

use fenix_control::protocol::OpenProjectParams;
use fenix_control::{Call, RpcError, StateResult};
use fenix_tasks::PathPosition;
use serde_json::{json, Value};

use super::*;

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
            Call::State => {
                Ok(serde_json::to_value(self.control_state()).expect("plain data serializes"))
            }
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
}
