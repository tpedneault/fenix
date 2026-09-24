//! Projects in the chrome -- the kind tag and name the modeline, the
//! window title and Home put in front of a project, all in one colour
//! per kind so the tag reads as the same mark everywhere -- and the
//! host half of the new-project wizard (`SPC p c`, see `project_
//! wizard`): its page buffer, its keys, and the steps that touch the
//! disk and run commands.

use super::*;
use crate::project_wizard::{self, Action, Key, Page, Role as PageRole, RunWork, Status, Wizard};
use fenix_project::ProjectKind;
use std::collections::BTreeMap;
use std::io::BufRead;

/// A kind's colour, taken from the theme's own syntax roles rather than
/// fixed values, so a tag stays readable on a light theme too.
pub(super) fn kind_color(kind: ProjectKind, theme: &Theme) -> glyphon::Color {
    match kind {
        ProjectKind::Python => theme.syntax_type,
        ProjectKind::Arduino => rgba_to_glyphon(theme.mode_insert),
        ProjectKind::Mib => theme.syntax_constant,
        ProjectKind::Rust => theme.syntax_string,
        ProjectKind::Tcl => theme.syntax_function,
        ProjectKind::Cpp => theme.syntax_keyword,
        ProjectKind::Node => theme.syntax_number,
        ProjectKind::Go => theme.syntax_type,
        ProjectKind::Other => theme.gutter_fg,
    }
}

/// `PY orbit-tools · ` -- the modeline's project segment, drawn between
/// the mode label and the file name.
pub(super) fn modeline_project_spans(kind: ProjectKind, name: &str, theme: &Theme) -> Vec<(String, glyphon::Color)> {
    vec![
        (format!("{} ", kind.tag()), kind_color(kind, theme)),
        (name.to_string(), theme.fg_modeline),
        (" · ".to_string(), theme.gutter_fg),
    ]
}

/// "orbit-tools — Fenix", or plain "Fenix" outside a project -- so the
/// taskbar tells two Fenix windows apart.
pub(super) fn window_title(project: Option<&str>) -> String {
    match project {
        Some(name) => format!("{name} \u{2014} Fenix"),
        None => "Fenix".to_string(),
    }
}

/// The wizard's page buffer and the wizard itself -- at most one at a
/// time, like the other single-instance panels.
pub(super) struct ProjectWizardState {
    pub(super) buffer: BufferId,
    pub(super) wizard: Wizard,
    page: Page,
    /// The pane width `page` was laid out for, and whether the wizard
    /// has changed since.
    cols: usize,
    stale: bool,
    /// Bumped for every step started, so output from a step that's since
    /// been retried is recognised as stale and dropped.
    generation: u64,
    step_started: Option<Instant>,
}

/// A running step's output, or its end -- sent from the thread that
/// runs it.
#[derive(Debug)]
pub enum ProjectCreateEvent {
    Output { generation: u64, line: String },
    Finished { generation: u64, result: Result<(), String> },
}

/// Sends every line `stream` prints. Progress bars rewrite a line with
/// `\r`; only the last state of each is worth a line in the log.
fn forward_lines(stream: impl std::io::Read, generation: u64, send: &dyn Fn(ProjectCreateEvent)) {
    for line in std::io::BufReader::new(stream).lines().map_while(Result::ok) {
        let line = line.rsplit('\r').find(|part| !part.trim().is_empty()).unwrap_or_default().trim_end().to_string();
        if !line.is_empty() {
            send(ProjectCreateEvent::Output { generation, line });
        }
    }
}

/// Runs `command` to completion, sending each line it prints (both
/// streams) and then how it ended.
fn run_step(mut command: std::process::Command, generation: u64, send: impl Fn(ProjectCreateEvent) + Send + Sync + Clone + 'static) {
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(e) => {
            send(ProjectCreateEvent::Finished { generation, result: Err(format!("couldn't start: {e}")) });
            return;
        }
    };
    let stderr = child.stderr.take().map(|stream| {
        let send = send.clone();
        std::thread::spawn(move || forward_lines(stream, generation, &send))
    });
    if let Some(stdout) = child.stdout.take() {
        forward_lines(stdout, generation, &send);
    }
    if let Some(thread) = stderr {
        let _ = thread.join();
    }
    let result = match child.wait() {
        Ok(status) if status.success() => Ok(()),
        Ok(status) => Err(match status.code() {
            Some(code) => format!("exited with code {code}"),
            None => "was stopped".to_string(),
        }),
        Err(e) => Err(e.to_string()),
    };
    send(ProjectCreateEvent::Finished { generation, result });
}

impl App {
    /// Keeps the OS window's title on the focused buffer's project.
    /// Compared first: setting a title is a system call and a repaint
    /// of the title bar, and this runs every frame.
    pub(super) fn sync_window_title(&self) {
        let Some(window) = &self.window else { return };
        let name = self.project_root.as_deref().and_then(|root| root.file_name()).map(|n| n.to_string_lossy().into_owned());
        let title = window_title(name.as_deref());
        if window.title() != title {
            window.set_title(&title);
        }
    }

    /// Where a program the wizard runs is: `arduino-cli` through the same
    /// search the Arduino integration uses (its tools folder, a
    /// configured path, the usual install places), anything else on PATH.
    fn locate_program(&self, program: &str) -> Option<PathBuf> {
        if program == fenix_embedded::Tool::ArduinoCli.binary() {
            return self.embedded_tools().arduino_cli;
        }
        crate::tool_status::is_on_path(program).then(|| PathBuf::from(program))
    }

    /// `SPC p c`: the new-project wizard, in the focused pane. A second
    /// press brings back the one already open, answers and all.
    pub(crate) fn cmd_project_new(&mut self) {
        if let Some(buffer) = self.project_wizard.as_ref().map(|state| state.buffer).filter(|id| self.buffers.get(*id).is_some()) {
            self.open_buffer_in_focused_pane(buffer);
            self.main_view = MainView::Editor;
            self.wake_caret();
            return;
        }
        let user_dir = fenix_project::template::user_templates_dir();
        let (templates, errors) = fenix_project::template::available_templates(user_dir.as_deref(), self.project_root.as_deref());
        let mut found = BTreeMap::new();
        for program in templates.iter().flat_map(fenix_project::template::programs_needed).chain(["git".to_string()]) {
            if let std::collections::btree_map::Entry::Vacant(entry) = found.entry(program) {
                let located = self.locate_program(entry.key()).is_some();
                entry.insert(located);
            }
        }
        // New projects land beside the current one, else in the home
        // folder.
        let parent = self.project_root.as_deref().and_then(Path::parent).map(Path::to_path_buf).or_else(dirs::home_dir).unwrap_or_default();
        let wizard = Wizard::new(templates, errors, found, &parent);
        let buffer = self.buffers.open_page("");
        self.project_wizard =
            Some(ProjectWizardState { buffer, wizard, page: Page::default(), cols: 0, stale: true, generation: 0, step_started: None });
        self.open_buffer_in_focused_pane(buffer);
        self.main_view = MainView::Editor;
        self.refresh_project_root();
        self.wake_caret();
    }

    /// Whether `id` is the wizard's page.
    pub(super) fn is_project_wizard_buffer(&self, id: BufferId) -> bool {
        self.project_wizard.as_ref().is_some_and(|s| s.buffer == id)
    }

    /// Lays the wizard's page out for a pane `cols` cells wide, if it
    /// changed or the pane did, and puts the cursor on the focused row.
    pub(super) fn ensure_page_layout(&mut self, id: BufferId, pane: fenix_window::WindowId, cols: usize) {
        let Some(state) = self.project_wizard.as_mut().filter(|s| s.buffer == id) else { return };
        if !state.stale && state.cols == cols {
            return;
        }
        state.page = project_wizard::layout(&state.wizard, cols);
        state.cols = cols;
        state.stale = false;
        let text = state.page.text.clone();
        let (line, col) = state.page.cursor();
        let Some(ob) = self.buffers.get_mut(id) else { return };
        if ob.buffer.text() != text {
            let end = ob.buffer.len_chars();
            let mut scratch = Cursor::at_start();
            ob.buffer.replace_range(&mut scratch, 0, end, &text);
            ob.buffer.mark_saved();
            ob.buffer.drain_edits();
        }
        self.home_place_cursor(id, pane, line, col);
    }

    /// Keys the wizard claims while its page is focused. When a field is
    /// being typed into, that's every key; otherwise the leader and `:`
    /// still reach Vim, so `SPC w` and friends work from the page.
    pub(super) fn page_key(&mut self, keypress: KeyPress) -> bool {
        let id = self.focused_buffer_id();
        let Some(state) = self.project_wizard.as_ref().filter(|s| s.buffer == id) else { return false };
        // A leader sequence under way is the leader's, not the page's.
        if !self.vim.is_idle() || self.leader_matcher.is_pending() {
            return false;
        }
        let editing = state.wizard.editing.is_some();
        let claims_space = editing || state.wizard.claims_space();
        if keypress == KeyPress::char('v').with_ctrl() && editing {
            if let Some(text) = self.clipboard_text() {
                if let Some(state) = self.project_wizard.as_mut() {
                    state.wizard.type_text(&text);
                    state.stale = true;
                }
            }
            return true;
        }
        let shift = self.modifiers.shift_key();
        let key = match (keypress.code, keypress.mods.ctrl) {
            (KeyCode::Char('c'), true) => Key::CtrlC,
            (_, true) => return false,
            (KeyCode::Char(' '), false) if claims_space => Key::Space,
            // The leader and the command line, while nothing's typed.
            (KeyCode::Char(' ' | ':'), false) => return false,
            (KeyCode::Char(c), false) => Key::Char(c),
            (KeyCode::Named(FenixNamedKey::Enter), _) => Key::Enter,
            (KeyCode::Named(FenixNamedKey::Tab), _) if shift => Key::BackTab,
            (KeyCode::Named(FenixNamedKey::Tab), _) => Key::Tab,
            (KeyCode::Named(FenixNamedKey::Escape), _) => Key::Escape,
            (KeyCode::Named(FenixNamedKey::Backspace), _) => Key::Backspace,
            (KeyCode::Named(FenixNamedKey::Up), _) => Key::Up,
            (KeyCode::Named(FenixNamedKey::Down), _) => Key::Down,
            (KeyCode::Named(FenixNamedKey::Left), _) => Key::Left,
            (KeyCode::Named(FenixNamedKey::Right), _) => Key::Right,
            _ => return false,
        };
        let Some(state) = self.project_wizard.as_mut() else { return false };
        let action = state.wizard.key(key);
        state.stale = true;
        match action {
            Action::None => {}
            Action::Close => self.project_wizard_close(),
            Action::Create => self.project_run_next(),
            Action::Retry => {
                if let Some(run) = self.project_wizard.as_mut().and_then(|s| s.wizard.run.as_mut()) {
                    if let Some(i) = run.failed() {
                        run.steps[i].status = Status::Pending;
                        run.steps[i].output.clear();
                    }
                }
                self.project_run_next();
            }
            Action::Skip => {
                if let Some(run) = self.project_wizard.as_mut().and_then(|s| s.wizard.run.as_mut()) {
                    if let Some(i) = run.failed() {
                        run.steps[i].status = Status::Skipped;
                    }
                }
                self.project_run_next();
            }
            Action::Finish => self.project_finish(),
        }
        self.wake_caret();
        true
    }

    /// Closes the wizard -- refused while a command is running, since
    /// the process would carry on with nobody watching it.
    fn project_wizard_close(&mut self) {
        if self.project_wizard.as_ref().and_then(|s| s.wizard.run.as_ref()).is_some_and(|run| run.running().is_some()) {
            self.set_error("a command is still running -- C-c stops after it");
            return;
        }
        let Some(state) = self.project_wizard.take() else { return };
        self.close_page_buffer(state.buffer);
    }

    fn close_page_buffer(&mut self, id: BufferId) {
        self.buffers.close(id);
        let fallback = self.buffers.mru().first().copied().unwrap_or_else(|| self.buffers.open_scratch());
        self.repoint_panes_showing(id, fallback);
        self.refresh_project_root();
    }

    /// Starts the next pending step, if the run should go on: writing
    /// the files happens here and now, a command on its own thread.
    /// Once nothing's left and nothing failed, finishes.
    fn project_run_next(&mut self) {
        let Some(state) = self.project_wizard.as_mut() else { return };
        state.stale = true;
        let Some(run) = state.wizard.run.as_mut() else { return };
        let Some(i) = run.next_pending() else {
            if run.finished() && !run.cancel_requested && run.failed().is_none() {
                self.project_finish();
            }
            return;
        };
        run.steps[i].status = Status::Running;
        state.generation += 1;
        state.step_started = Some(Instant::now());
        let generation = state.generation;
        let work = run.steps[i].work.clone();
        let Some(plan) = state.wizard.plan.clone() else { return };
        let mut step = match work {
            RunWork::WriteFiles => {
                let result = plan.write_files().map(|_| ());
                self.apply_project_create_event(ProjectCreateEvent::Finished { generation, result });
                return;
            }
            RunWork::Command(step) => step,
        };
        if let Some(located) = self.locate_program(&step.spec.executable) {
            step.spec.executable = located.display().to_string();
        }
        let command = match step.command(&plan.dir) {
            Ok(command) => command,
            Err(e) => {
                self.apply_project_create_event(ProjectCreateEvent::Finished { generation, result: Err(e.to_string()) });
                return;
            }
        };
        match self.event_proxy.clone() {
            Some(proxy) => {
                std::thread::spawn(move || {
                    run_step(command, generation, move |event| {
                        let _ = proxy.send_event(FenixUserEvent::ProjectCreate(event));
                    })
                });
            }
            // No event loop (tests): run it here, then replay what it
            // said -- which goes on to the next step.
            None => {
                let events = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
                let sink = events.clone();
                run_step(command, generation, move |event| sink.lock().unwrap().push(event));
                let events = std::mem::take(&mut *events.lock().unwrap());
                for event in events {
                    self.apply_project_create_event(event);
                }
            }
        }
    }

    /// A step's output or its end. A step that succeeded starts the next.
    pub(super) fn apply_project_create_event(&mut self, event: ProjectCreateEvent) {
        let Some(state) = self.project_wizard.as_mut() else { return };
        let generation = match &event {
            ProjectCreateEvent::Output { generation, .. } | ProjectCreateEvent::Finished { generation, .. } => *generation,
        };
        if generation != state.generation {
            return;
        }
        let elapsed = state.step_started.map(|t| t.elapsed()).unwrap_or_default();
        state.stale = true;
        let Some(run) = state.wizard.run.as_mut() else { return };
        let Some(i) = run.running() else { return };
        let go_on = match event {
            ProjectCreateEvent::Output { line, .. } => {
                run.push_output(i, line);
                false
            }
            ProjectCreateEvent::Finished { result, .. } => {
                run.steps[i].status = match result {
                    Ok(()) => Status::Done(elapsed),
                    Err(e) => Status::Failed(e),
                };
                run.failed().is_none()
            }
        };
        if go_on {
            self.project_run_next();
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    /// The run is over (or `o` on a failure): add the project, do the
    /// template's editor-side hooks, open its main file in the wizard's
    /// place, and close the wizard.
    fn project_finish(&mut self) {
        let Some(state) = self.project_wizard.take() else { return };
        let wizard = state.wizard;
        let Some(plan) = wizard.plan else {
            self.close_page_buffer(state.buffer);
            return;
        };
        let dir = std::fs::canonicalize(&plan.dir).map(fenix_lsp::normalize).unwrap_or_else(|_| plan.dir.clone());
        self.project_kinds.borrow_mut().clear();
        if wizard.register {
            self.known_projects.add(dir.clone());
            if let Err(err) = self.known_projects.save() {
                eprintln!("fenix: couldn't save project history: {err}");
            }
        }
        for hook in &plan.hooks {
            match hook {
                fenix_project::template::Hook::MibRoot { path, label } => self.add_mib_root(dir.join(path), label.clone()),
            }
        }
        let steps = wizard.run.as_ref().map(|r| r.steps.len()).unwrap_or(0);
        match plan.file_to_open() {
            Some(file) => self.open_file_from_picker(&file),
            None => self.switch_to_project(dir.clone()),
        }
        self.close_page_buffer(state.buffer);
        self.refresh_home_data(false);
        self.set_message(format!("created {} -- {steps} step{}", dir.display(), if steps == 1 { "" } else { "s" }));
    }

    /// The page's colours for the visible lines, in the shape syntax
    /// highlighting hands the renderer.
    pub(super) fn page_highlights(&self, id: BufferId, first_line: usize, rows: usize) -> Vec<(std::ops::Range<usize>, glyphon::Color)> {
        let (Some(state), Some(ob)) = (self.project_wizard.as_ref().filter(|s| s.buffer == id), self.buffers.get(id)) else { return Vec::new() };
        let theme = self.theme;
        let color = |role: PageRole| match role {
            PageRole::Title | PageRole::Text => theme.fg,
            PageRole::Muted => theme.gutter_fg,
            PageRole::Accent => rgba_to_glyphon(theme.caret),
            PageRole::Good => theme.git_staged,
            PageRole::Warn => theme.git_modified,
            PageRole::Bad => theme.git_conflicted,
            PageRole::Kind(kind) => kind_color(kind, theme),
        };
        let last = first_line + rows;
        let mut ranges: Vec<(std::ops::Range<usize>, glyphon::Color)> = state
            .page
            .spans
            .iter()
            .filter(|s| s.line >= first_line && s.line <= last && s.line < ob.buffer.line_count())
            .map(|s| {
                let start = ob.buffer.line_start_char(s.line);
                let len = ob.buffer.line_len(s.line);
                let a = ob.buffer.char_to_byte(start + s.cols.start.min(len));
                let b = ob.buffer.char_to_byte(start + s.cols.end.min(len));
                (a..b, color(s.role))
            })
            .filter(|(r, _)| r.start < r.end)
            .collect();
        ranges.sort_by_key(|(r, _)| r.start);
        ranges
    }

    /// The page's backgrounds -- the focused row's tint and rail, the
    /// field being typed into -- and its hairlines, in Home's shapes.
    pub(super) fn page_backgrounds(&self, id: BufferId, first_line: usize, rows: usize) -> (home::BgSegments, home::HomeOverlay) {
        let Some(state) = self.project_wizard.as_ref().filter(|s| s.buffer == id) else { return Default::default() };
        let theme = self.theme;
        let visible = |line: usize| (line >= first_line && line <= first_line + rows).then(|| line - first_line);
        let mut segments = Vec::new();
        let mut overlay = home::HomeOverlay::default();
        if let Some((line, cols)) = &state.page.focus {
            if let Some(row) = visible(*line) {
                segments.push((row, cols.start, cols.end, theme.hl_line));
                overlay.rail = Some((row, cols.start, 1));
            }
        }
        let panel = if theme.sidebar_bg != theme.bg { theme.sidebar_bg } else { theme.bg_modeline };
        for (line, cols) in &state.page.panels {
            if let Some(row) = visible(*line) {
                segments.push((row, cols.start, cols.end, panel));
            }
        }
        overlay.rules = state.page.rules.iter().filter_map(|(line, cols)| visible(*line).map(|row| (row, cols.start, cols.end))).collect();
        overlay.rule_color = home::home_rule(theme);
        (segments, overlay)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_modeline_segment_is_the_tag_then_the_name() {
        let spans = modeline_project_spans(ProjectKind::Arduino, "blink-lab", &theme::VISUAL_STUDIO_DARK);
        let text: String = spans.iter().map(|(t, _)| t.as_str()).collect();
        assert_eq!(text, "INO blink-lab · ");
        assert_eq!(spans[0].1, rgba_to_glyphon(theme::VISUAL_STUDIO_DARK.mode_insert));
    }

    #[test]
    fn the_window_is_named_after_its_project() {
        assert_eq!(window_title(Some("orbit-tools")), "orbit-tools \u{2014} Fenix");
        assert_eq!(window_title(None), "Fenix");
    }
    /// A fresh folder under the temp dir, removed when dropped.
    struct Scratch(PathBuf);
    impl Scratch {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("fenix-wizard-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Scratch(dir)
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// The wizard open on `template`, pointed at `parent`, with nothing
    /// that writes outside it (no registration, which saves the user's
    /// project list).
    fn wizard_app(template: &str, parent: &Path, git: bool) -> App {
        let mut app = App::with_file(None);
        app.cmd_project_new();
        let state = app.project_wizard.as_mut().unwrap();
        let w = &mut state.wizard;
        w.parent = parent.display().to_string();
        (w.register, w.git_init, w.git_commit) = (false, git, false);
        let i = w.templates.iter().position(|t| t.id == template).unwrap();
        w.focus = w.fields().iter().position(|f| *f == project_wizard::Field::Template(i)).unwrap();
        app
    }

    fn press(app: &mut App, keys: &str) {
        for c in keys.chars() {
            let key = if c == '\n' { KeyPress::named(FenixNamedKey::Enter) } else { KeyPress::char(c) };
            assert!(app.page_key(key), "the page claims {c:?}");
        }
    }

    #[test]
    fn spc_p_c_opens_the_wizard_as_a_page_and_lays_it_out() {
        let mut app = App::with_file(None);
        app.cmd_project_new();
        assert_eq!(app.open().kind, BufferKind::Page);
        assert_eq!(app.buffer_display_name(app.focused_buffer_id()), "*new project*");
        let (id, pane) = (app.focused_buffer_id(), app.focused_pane_id());
        app.ensure_page_layout(id, pane, 110);
        let text = app.open().buffer.text();
        assert!(text.contains("New project") && text.contains("Python · uv"), "{text}");
        assert!(!app.page_highlights(id, 0, 40).is_empty());
        assert!(!app.page_key(KeyPress::char(' ')), "the leader still works from the page");
        app.leader_matcher.feed(KeyPress::char(' '));
        assert!(!app.page_key(KeyPress::char('G')), "a pending leader sequence gets its keys");
        app.leader_matcher.cancel();
        app.cmd_project_new();
        assert_eq!(app.focused_buffer_id(), id, "a second SPC p c brings back the same wizard");
    }

    #[test]
    fn a_space_typed_into_a_field_is_a_space_not_the_leader() {
        let dir = Scratch::new("space");
        let mut app = wizard_app("empty", &dir.0, false);
        press(&mut app, "\n");
        assert!(app.page_key(KeyPress::char(' ')));
        assert_eq!(app.project_wizard.as_ref().unwrap().wizard.editing.as_deref(), Some(" "));
    }

    #[test]
    fn creating_an_empty_project_writes_it_runs_git_and_opens_its_readme() {
        let dir = Scratch::new("create");
        let mut app = wizard_app("empty", &dir.0, true);
        // Template, name, then Continue (G) and Review's Create.
        press(&mut app, "\nfresh\nG\n");
        assert_eq!(app.project_wizard.as_ref().unwrap().wizard.step, project_wizard::Step::Review);
        press(&mut app, "\n");
        let project = dir.0.join("fresh");
        assert_eq!(std::fs::read_to_string(project.join("README.md")).unwrap(), "# fresh\n");
        assert!(project.join(".git").is_dir(), "git init ran");
        assert!(app.project_wizard.is_none(), "the wizard closed itself");
        assert!(app.open().buffer.path().is_some_and(|p| p.ends_with("README.md")), "and opened the README");
        assert!(app.buffers.mru().iter().all(|id| app.buffers.get(*id).is_some_and(|ob| ob.kind != BufferKind::Page)));
        let unnamed = app.buffers.mru().iter().filter(|id| app.buffers.get(**id).is_some_and(|ob| ob.kind == BufferKind::Text && ob.buffer.path().is_none())).count();
        assert_eq!(unnamed, 0, "closing the page leaves no scratch buffer behind");
    }

    #[test]
    fn a_failing_step_stops_the_run_and_skip_goes_on() {
        let dir = Scratch::new("fail");
        let mut app = wizard_app("empty", &dir.0, false);
        press(&mut app, "\nfresh\nG\n");
        let state = app.project_wizard.as_mut().unwrap();
        state.wizard.plan.as_mut().unwrap().steps.push(fenix_project::template::Step::new("definitely-not-a-program-xyz", &[]));
        press(&mut app, "\n");
        let run = app.project_wizard.as_ref().unwrap().wizard.run.as_ref().unwrap();
        assert!(matches!(run.steps.last().unwrap().status, Status::Failed(_)), "{:?}", run.steps.last().unwrap().status);
        assert!(dir.0.join("fresh/README.md").is_file(), "what was done stays");
        press(&mut app, "s");
        assert!(app.project_wizard.is_none(), "skipping the last step finishes");
    }
}

