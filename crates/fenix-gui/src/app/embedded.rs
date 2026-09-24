//! Microcontroller projects (`SPC m` in a sketch): build, flash, a serial monitor,
//! debugging where the board allows it, board/port/library management,
//! and the right language server for the sketch.
//!
//! Everything toolchain-specific comes from a `fenix_embedded::Platform`
//! (Arduino today); this module only turns what it returns into the
//! editor's existing machinery -- the task runner for builds and uploads
//! (so errors land in the quickfix list), a terminal pane for the
//! monitor and the debugger console, pickers for choices. Queries that
//! run the toolchain happen off the UI thread and come back as one
//! `EmbeddedEvent`.

use super::*;

use fenix_embedded::{Board, BoardOption, Debugging, Library, Package, Platform, Port};

/// What to do once a port is known.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortAction {
    Upload,
    Monitor,
    Debug,
}

/// What to do when the task this module started finishes.
#[derive(Debug, Clone)]
pub(super) enum AfterTask {
    /// An upload; the monitor it stopped (if any) comes back either way.
    Upload { root: PathBuf, reopen_monitor: bool },
    /// A debug build was flashed; open the debugger console.
    Debugger { root: PathBuf, session: fenix_embedded::Command },
    /// Something the language server should see (a new library).
    RestartLanguageServer { root: PathBuf },
}

/// A serial monitor: which terminal buffer, on which port.
#[derive(Debug, Clone)]
pub(super) struct Monitor {
    pub(super) buffer: BufferId,
    pub(super) port: String,
}

#[derive(Default)]
pub(super) struct EmbeddedState {
    pub(super) picker: Option<EmbeddedPickerCtx>,
    pub(super) prompt: Option<EmbeddedPrompt>,
    /// Serial monitors by project root -- one per sketch.
    pub(super) monitors: HashMap<PathBuf, Monitor>,
    pub(super) after_task: Option<AfterTask>,
    /// The library index, loaded once (it's ~10,000 entries).
    pub(super) libraries: Option<Vec<Library>>,
    /// The focused project's board and port, for the modeline.
    pub(super) indicator: Option<String>,
}

pub(super) struct EmbeddedPickerCtx {
    pub(super) label: String,
    pub(super) root: PathBuf,
    /// For a port picker: what the port is for.
    pub(super) then: Option<PortAction>,
    /// For an option picker: the board the options belong to.
    pub(super) board: Option<String>,
}

pub(super) struct EmbeddedPrompt {
    pub(super) parent: PathBuf,
    pub(super) input: String,
}

/// One row of an `ActivePicker::Embedded` picker.
#[derive(Debug, Clone)]
pub(crate) enum EmbeddedPick {
    Board(String),
    MoreBoards,
    BoardOption { option: String, value: String },
    Port(String),
    Baud(u32),
    Library(String),
    Package(String),
}

/// A toolchain query's answer.
#[derive(Debug)]
pub enum EmbeddedEvent {
    Ports { root: PathBuf, then: Option<PortAction>, result: Result<Vec<Port>, String> },
    Boards { root: PathBuf, result: Result<Vec<Board>, String> },
    BoardOptions { root: PathBuf, board: String, result: Result<Vec<BoardOption>, String> },
    Libraries { root: PathBuf, result: Result<Vec<Library>, String> },
    Packages { root: PathBuf, result: Result<Vec<Package>, String> },
    Debugging { root: PathBuf, port: Option<String>, result: Result<Debugging, String> },
}

/// `arduino:avr:nano:cpu=atmega328old` -> `nano`.
fn short_board(board: &str) -> &str {
    board.split(':').nth(2).unwrap_or(board)
}

impl App {
    pub(super) fn embedded_tools(&self) -> fenix_embedded::Tools {
        fenix_embedded::Tools::discover(&fenix_embedded::ToolOverrides {
            arduino_cli: self.config.embedded_arduino_cli.clone(),
            clangd: self.config.embedded_clangd.clone(),
            arduino_language_server: self.config.embedded_arduino_language_server.clone(),
        })
    }

    /// The embedded project rooted at `root`, if it is one.
    pub(super) fn embedded_project_at(&self, root: &Path) -> Option<Box<dyn Platform>> {
        fenix_embedded::detect(root, &self.embedded_tools())
    }

    /// The embedded project the focused buffer belongs to, or an error
    /// saying there isn't one.
    fn embedded_project(&mut self) -> Option<Box<dyn Platform>> {
        let root = self.integration_root();
        let project = fenix_embedded::project_root_of(&root).and_then(|root| self.embedded_project_at(&root));
        if project.is_none() {
            self.set_error("not in an Arduino sketch -- open a sketch's .ino file, or SPC f n to create one");
        }
        project
    }

    /// Runs `job` off the UI thread (inline without an event loop).
    pub(super) fn embedded_spawn(&mut self, job: impl FnOnce() -> EmbeddedEvent + Send + 'static) {
        match self.event_proxy.clone() {
            Some(proxy) => {
                std::thread::spawn(move || {
                    let _ = proxy.send_event(FenixUserEvent::Embedded(job()));
                });
            }
            None => {
                let event = job();
                self.apply_embedded_event(event);
            }
        }
    }

    fn embedded_run(&mut self, name: &str, command: fenix_embedded::Command, root: &Path) {
        self.run_task(fenix_tasks::TaskDef { name: name.to_string(), command: command.program, args: command.args }, root.to_path_buf());
    }

    /// The modeline's `uno · COM4`, recomputed on focus changes and after
    /// the board or port changes -- not per frame, since it reads files.
    pub(super) fn refresh_embedded_indicator(&mut self) {
        let root = self.open().buffer.path().and_then(fenix_embedded::project_root_of);
        self.embedded.indicator = root.and_then(|root| self.embedded_project_at(&root)).map(|p| {
            let (board, chosen) = p.board();
            let board = if chosen { short_board(&board).to_string() } else { format!("{} (default)", short_board(&board)) };
            format!("   {board} · {}", p.port().unwrap_or_else(|| "no port".to_string()))
        });
    }

    /// The language server command for C/C++ files under `root`, when
    /// `root` is an embedded project -- `None` otherwise, so the usual
    /// server (clangd) takes over.
    pub(super) fn embedded_language_server(&self, root: &Path) -> Option<Result<fenix_project::tools::CommandSpec, String>> {
        let root = fenix_embedded::project_root_of(root)?;
        let project = self.embedded_project_at(&root)?;
        Some(project.language_server().map(|command| fenix_project::tools::CommandSpec::new(command.program, command.args)))
    }

    /// Embedded build tasks for the task picker (`SPC t t`).
    pub(super) fn embedded_tasks(&self, root: &Path) -> Vec<fenix_tasks::TaskDef> {
        let Some(project) = fenix_embedded::project_root_of(root).and_then(|root| self.embedded_project_at(&root)) else { return Vec::new() };
        let name = format!("{}: build", project.family());
        project.build().map(|c| fenix_tasks::TaskDef { name, command: c.program, args: c.args }).into_iter().collect()
    }

    // -- Commands -----------------------------------------------------------

    /// `SPC m b`: compile.
    pub(crate) fn cmd_embedded_build(&mut self) {
        let Some(project) = self.embedded_project() else { return };
        let root = project.root().to_path_buf();
        let (board, chosen) = project.board();
        match project.build() {
            Ok(command) => {
                self.embedded_run(&format!("{}: build", project.family()), command, &root);
                if !chosen {
                    self.set_message(format!("building for {board}, the default -- SPC m s picks your board"));
                }
            }
            Err(err) => self.set_error(err),
        }
    }

    /// `SPC m u`: build and flash.
    pub(crate) fn cmd_embedded_upload(&mut self) {
        self.embedded_with_port(PortAction::Upload);
    }

    /// `SPC m m`: the serial monitor -- opened, or brought back.
    pub(crate) fn cmd_embedded_monitor(&mut self) {
        self.embedded_with_port(PortAction::Monitor);
    }

    /// `SPC m d`: debug, if the board can be.
    pub(crate) fn cmd_embedded_debug(&mut self) {
        let Some(project) = self.embedded_project() else { return };
        let root = project.root().to_path_buf();
        let port = project.port();
        self.set_message("checking whether this board can be debugged...");
        self.embedded_spawn(move || {
            let result = project.debugging(port.as_deref());
            EmbeddedEvent::Debugging { root, port, result }
        });
    }

    /// `SPC m p`: choose the port.
    pub(crate) fn cmd_embedded_port(&mut self) {
        let Some(project) = self.embedded_project() else { return };
        let root = project.root().to_path_buf();
        self.set_message("looking for boards...");
        self.embedded_spawn(move || EmbeddedEvent::Ports { root, then: None, result: project.ports() });
    }

    /// `SPC m s`: choose the board.
    pub(crate) fn cmd_embedded_board(&mut self) {
        let Some(project) = self.embedded_project() else { return };
        let root = project.root().to_path_buf();
        self.embedded_spawn(move || EmbeddedEvent::Boards { root, result: project.boards() });
    }

    /// `SPC m o`: the board's options (processor, clock...).
    pub(crate) fn cmd_embedded_board_options(&mut self) {
        let Some(project) = self.embedded_project() else { return };
        let root = project.root().to_path_buf();
        let (board, _) = project.board();
        self.embedded_spawn(move || {
            let result = project.board_options(&board);
            EmbeddedEvent::BoardOptions { root, board, result }
        });
    }

    /// `SPC m B`: the monitor's speed.
    pub(crate) fn cmd_embedded_baud(&mut self) {
        let Some(project) = self.embedded_project() else { return };
        let current = project.baud_rate();
        let candidates = fenix_embedded::arduino::BAUD_RATES
            .iter()
            .map(|&baud| {
                let label = if baud == current { format!("{baud} baud  (current)") } else { format!("{baud} baud") };
                fenix_picker::Candidate::new(label, EmbeddedPick::Baud(baud))
            })
            .collect();
        let root = project.root().to_path_buf();
        self.embedded_enter_picker(EmbeddedPickerCtx { label: "SERIAL SPEED".to_string(), root, then: None, board: None }, candidates);
    }

    /// `SPC m l`: install a library.
    pub(crate) fn cmd_embedded_library(&mut self) {
        let Some(project) = self.embedded_project() else { return };
        let root = project.root().to_path_buf();
        if let Some(libraries) = self.embedded.libraries.clone() {
            self.embedded_library_picker(root, libraries);
            return;
        }
        self.set_message("loading the library index...");
        self.embedded_spawn(move || EmbeddedEvent::Libraries { root, result: project.search_libraries("") });
    }

    /// `SPC m c`: install board support (a core).
    pub(crate) fn cmd_embedded_package(&mut self) {
        let Some(project) = self.embedded_project() else { return };
        let root = project.root().to_path_buf();
        self.set_message("loading the board package index...");
        self.embedded_spawn(move || EmbeddedEvent::Packages { root, result: project.search_packages("") });
    }

    /// `SPC m n`: a new sketch, next to the current one (or in the
    /// project/working directory when not in one).
    pub(crate) fn cmd_embedded_new_sketch(&mut self) {
        let root = self.integration_root();
        let parent = match fenix_embedded::project_root_of(&root) {
            Some(sketch) => sketch.parent().map(Path::to_path_buf).unwrap_or(root),
            None => root,
        };
        self.embedded.prompt = Some(EmbeddedPrompt { parent, input: String::new() });
    }

    /// `SPC m i`: what Fenix knows about this project and its tools.
    pub(crate) fn cmd_embedded_info(&mut self) {
        let Some(project) = self.embedded_project() else { return };
        let (board, chosen) = project.board();
        let board = if chosen { board } else { format!("{board} (default -- SPC m s to choose)") };
        let port = project.port().unwrap_or_else(|| "none yet -- SPC m p".to_string());
        let completion = match project.language_server() {
            Ok(_) => "completion ready".to_string(),
            Err(err) => err,
        };
        self.set_message(format!("{} sketch · board {board} · port {port} · {} baud · {completion}", project.family(), project.baud_rate()));
    }

    // -- Ports and the actions that need one --------------------------------

    /// Runs `action` on the project's port, finding one first if none is
    /// set: a single port with a recognized board is used (and saved);
    /// otherwise you pick.
    fn embedded_with_port(&mut self, action: PortAction) {
        let Some(project) = self.embedded_project() else { return };
        let root = project.root().to_path_buf();
        if let Some(port) = project.port() {
            self.embedded_do(action, &root, &port);
            return;
        }
        self.set_message("looking for your board...");
        self.embedded_spawn(move || EmbeddedEvent::Ports { root, then: Some(action), result: project.ports() });
    }

    fn embedded_do(&mut self, action: PortAction, root: &Path, port: &str) {
        match action {
            PortAction::Upload => self.embedded_upload(root, port),
            PortAction::Monitor => self.embedded_open_monitor(root, port),
            PortAction::Debug => self.cmd_embedded_debug(),
        }
    }

    fn embedded_upload(&mut self, root: &Path, port: &str) {
        let Some(project) = self.embedded_project_at(root) else { return };
        let command = match project.upload(port) {
            Ok(command) => command,
            Err(err) => {
                self.set_error(err);
                return;
            }
        };
        // Only one program can hold a serial port: the monitor steps
        // aside for the upload and comes back after it.
        let reopen_monitor = self.embedded_stop_monitor(root);
        self.embedded.after_task = Some(AfterTask::Upload { root: root.to_path_buf(), reopen_monitor });
        self.embedded_run(&format!("{}: upload to {port}", project.family()), command, root);
    }

    // -- The serial monitor ---------------------------------------------------

    /// Opens the monitor for `root` on `port` in a pane under the current
    /// one -- or restarts it there if it's already open but stopped, or
    /// on a different port.
    fn embedded_open_monitor(&mut self, root: &Path, port: &str) {
        let Some(project) = self.embedded_project_at(root) else { return };
        let command = match project.monitor(port) {
            Ok(command) => command,
            Err(err) => {
                self.set_error(err);
                return;
            }
        };
        let label = format!("*serial {port} {} baud*", project.baud_rate());
        let previous_port = self.embedded.monitors.get(root).map(|m| m.port.clone());
        let existing = self.embedded.monitors.get(root).map(|m| m.buffer).filter(|id| self.buffers.get(*id).is_some());
        let buffer = match existing {
            Some(buffer) => buffer,
            None => {
                let buffer = self.buffers.open_terminal();
                self.terminal_buffer_cwds.insert(buffer, root.to_path_buf());
                buffer
            }
        };
        self.terminal_buffer_labels.insert(buffer, label.clone());
        self.terminal_buffer_programs.insert(buffer, (command.program, command.args));
        self.embedded.monitors.insert(root.to_path_buf(), Monitor { buffer, port: port.to_string() });

        let running = existing.is_some()
            && previous_port.as_deref() == Some(port)
            && self.terminal_buffers.get_mut(&buffer).is_some_and(|t| t.session.is_alive());
        let shown = self.windows().windows().into_iter().find(|&p| self.windows().content(p) == Some(&buffer));
        match shown {
            Some(pane) => {
                self.windows_mut().focus(pane);
            }
            None => {
                let pane = self.windows_mut().split(SplitKind::Horizontal, buffer);
                self.workspaces.active_pane_states_mut().insert(pane, PaneState::seeded_at(Cursor::at_start()));
                self.windows_mut().resize_focused(0.35);
                self.windows_mut().focus(pane);
            }
        }
        if let Some(pane) = self.windows().windows().into_iter().find(|&p| self.windows().content(p) == Some(&buffer)) {
            self.pane_titles.insert(pane, format!("Serial monitor  --  {port} at {} baud  --  Ctrl-\\ to leave, SPC m B for speed", project.baud_rate()));
        }
        if running {
            self.focus_terminal_buffer(buffer);
        } else {
            self.terminal_buffers.remove(&buffer);
            self.spawn_terminal_for(buffer);
        }
        self.wake_caret();
    }

    /// Stops `root`'s monitor, keeping its pane and what it printed.
    /// Returns whether one was running.
    fn embedded_stop_monitor(&mut self, root: &Path) -> bool {
        let Some(monitor) = self.embedded.monitors.get(root) else { return false };
        let buffer = monitor.buffer;
        let was_running = self.terminal_buffers.get_mut(&buffer).is_some_and(|t| t.session.is_alive());
        self.terminal_buffers.remove(&buffer);
        if self.terminal_buffer_focused == Some(buffer) {
            self.terminal_buffer_focused = None;
        }
        was_running
    }

    /// Restarts `root`'s monitor with its current settings, if it has one
    /// open -- after an upload, or a speed change.
    fn embedded_restart_monitor(&mut self, root: &Path) {
        let Some(monitor) = self.embedded.monitors.get(root).cloned() else { return };
        if self.buffers.get(monitor.buffer).is_none() {
            self.embedded.monitors.remove(root);
            return;
        }
        let Some(project) = self.embedded_project_at(root) else { return };
        if let Ok(command) = project.monitor(&monitor.port) {
            self.terminal_buffer_labels.insert(monitor.buffer, format!("*serial {} {} baud*", monitor.port, project.baud_rate()));
            self.terminal_buffer_programs.insert(monitor.buffer, (command.program, command.args));
        }
        self.terminal_buffers.remove(&monitor.buffer);
        self.spawn_terminal_for(monitor.buffer);
    }

    /// A terminal buffer went away: forget it if it was a monitor.
    pub(super) fn embedded_forget_terminal(&mut self, id: BufferId) {
        self.embedded.monitors.retain(|_, m| m.buffer != id);
    }

    /// Opens an interactive program (the debugger console) in a pane.
    fn embedded_open_console(&mut self, root: &Path, label: &str, command: fenix_embedded::Command) {
        let buffer = self.buffers.open_terminal();
        self.terminal_buffer_cwds.insert(buffer, root.to_path_buf());
        self.terminal_buffer_labels.insert(buffer, format!("*{label}*"));
        self.terminal_buffer_programs.insert(buffer, (command.program, command.args));
        let pane = self.windows_mut().split(SplitKind::Horizontal, buffer);
        self.workspaces.active_pane_states_mut().insert(pane, PaneState::seeded_at(Cursor::at_start()));
        self.windows_mut().resize_focused(0.45);
        self.windows_mut().focus(pane);
        self.pane_titles.insert(pane, format!("{label}  --  Ctrl-\\ to leave"));
        self.spawn_terminal_for(buffer);
    }

    // -- Finished tasks -----------------------------------------------------

    /// Called when a task finishes, to follow up on one this module ran.
    pub(super) fn embedded_task_finished(&mut self, root: &Path, success: bool) {
        let matches = |after: &AfterTask| match after {
            AfterTask::Upload { root: r, .. } | AfterTask::Debugger { root: r, .. } | AfterTask::RestartLanguageServer { root: r } => r == root,
        };
        if !self.embedded.after_task.as_ref().is_some_and(matches) {
            return;
        }
        match self.embedded.after_task.take() {
            Some(AfterTask::Upload { root, reopen_monitor }) => {
                if reopen_monitor {
                    self.embedded_restart_monitor(&root);
                }
                if success {
                    self.set_message("uploaded");
                } else {
                    self.set_error("upload failed -- is the board plugged in, and is it the right port? (SPC m p)");
                }
            }
            Some(AfterTask::Debugger { root, session }) => {
                if success {
                    self.embedded_open_console(&root, "debugger", session);
                } else {
                    self.set_error("the debug build didn't flash, so there's nothing to attach to -- see the task output");
                }
            }
            Some(AfterTask::RestartLanguageServer { root }) => {
                self.embedded.libraries = None;
                if success {
                    self.embedded_restart_language_server(&root);
                }
            }
            None => {}
        }
    }

    fn embedded_restart_language_server(&mut self, root: &Path) {
        let root = fenix_lsp::normalize(refactor::identity(root));
        self.lsp_unavailable.retain(|key| key.root != root);
        let removed: Vec<_> = self.lsp_sessions.keys().filter(|key| key.root == root).cloned().collect();
        for key in removed {
            if let Some(session) = self.lsp_sessions.remove(&key) {
                for path in session.open_documents.keys() {
                    self.diagnostics.remove(path);
                }
            }
        }
        self.sync_lsp_for_focused_buffer();
    }

    // -- Pickers ------------------------------------------------------------

    fn embedded_enter_picker(&mut self, ctx: EmbeddedPickerCtx, candidates: Vec<fenix_picker::Candidate<EmbeddedPick>>) {
        self.embedded.picker = Some(ctx);
        self.enter_picker(ActivePicker::Embedded(fenix_picker::PickerState::new(candidates)));
    }

    fn embedded_library_picker(&mut self, root: PathBuf, libraries: Vec<Library>) {
        let candidates = libraries
            .into_iter()
            .map(|lib| {
                let installed = match &lib.installed {
                    Some(v) if *v == lib.latest => "  [installed]".to_string(),
                    Some(v) => format!("  [installed {v}, update available]"),
                    None => String::new(),
                };
                let label = format!("{}  {}{installed}  --  {}", lib.name, lib.latest, lib.summary);
                fenix_picker::Candidate::new(label, EmbeddedPick::Library(lib.name))
            })
            .collect();
        self.embedded_enter_picker(EmbeddedPickerCtx { label: "INSTALL LIBRARY".to_string(), root, then: None, board: None }, candidates);
    }

    pub(super) fn embedded_picker_cancel(&mut self) {
        self.embedded.picker = None;
    }

    pub(super) fn embedded_picker_confirm(&mut self, pick: EmbeddedPick) {
        let Some(ctx) = self.embedded.picker.take() else { return };
        let Some(project) = self.embedded_project_at(&ctx.root) else { return };
        let saved = |app: &mut App, result: Result<(), String>| match result {
            Ok(()) => {
                app.refresh_embedded_indicator();
                true
            }
            Err(err) => {
                app.set_error(err);
                false
            }
        };
        match pick {
            EmbeddedPick::Board(board) => {
                if saved(self, project.set_board(&board)) {
                    self.set_message(format!("board: {board} -- SPC m o for its options"));
                    self.embedded_restart_language_server(&ctx.root);
                }
            }
            EmbeddedPick::MoreBoards => {
                self.set_message("loading the board package index...");
                let root = ctx.root.clone();
                self.embedded_spawn(move || EmbeddedEvent::Packages { root, result: project.search_packages("") });
            }
            EmbeddedPick::BoardOption { option, value } => {
                let board = ctx.board.unwrap_or_else(|| project.board().0);
                let board = project.board_with_option(&board, &option, &value);
                if saved(self, project.set_board(&board)) {
                    self.set_message(format!("board: {board}"));
                    self.embedded_restart_language_server(&ctx.root);
                }
            }
            EmbeddedPick::Port(port) => {
                if saved(self, project.set_port(&port)) {
                    match ctx.then {
                        Some(action) => self.embedded_do(action, &ctx.root, &port),
                        None => self.set_message(format!("port: {port}")),
                    }
                }
            }
            EmbeddedPick::Baud(baud) => {
                if saved(self, project.set_baud_rate(baud)) {
                    self.set_message(format!("serial monitor speed: {baud} baud -- match your Serial.begin({baud})"));
                    self.embedded_restart_monitor(&ctx.root);
                }
            }
            EmbeddedPick::Library(name) => match project.install_library(&name) {
                Ok(command) => {
                    self.embedded.after_task = Some(AfterTask::RestartLanguageServer { root: ctx.root.clone() });
                    self.embedded_run(&format!("install library {name}"), command, &ctx.root);
                }
                Err(err) => self.set_error(err),
            },
            EmbeddedPick::Package(id) => match project.install_package(&id) {
                Ok(command) => {
                    self.embedded.after_task = Some(AfterTask::RestartLanguageServer { root: ctx.root.clone() });
                    self.embedded_run(&format!("install {id}"), command, &ctx.root);
                }
                Err(err) => self.set_error(err),
            },
        }
    }

    // -- Background results -------------------------------------------------

    pub(super) fn apply_embedded_event(&mut self, event: EmbeddedEvent) {
        match event {
            EmbeddedEvent::Ports { root, then, result } => self.apply_embedded_ports(root, then, result),
            EmbeddedEvent::Boards { root, result } => match result {
                Ok(boards) => {
                    let current = self.embedded_project_at(&root).map(|p| p.board().0).unwrap_or_default();
                    let mut candidates: Vec<fenix_picker::Candidate<EmbeddedPick>> = boards
                        .into_iter()
                        .map(|b| {
                            let mark = if current.split(':').take(3).eq(b.id.split(':')) { "  (current)" } else { "" };
                            fenix_picker::Candidate::new(format!("{}  {}{mark}", b.name, b.id), EmbeddedPick::Board(b.id))
                        })
                        .collect();
                    candidates.push(fenix_picker::Candidate::new("Install more boards...", EmbeddedPick::MoreBoards));
                    self.embedded_enter_picker(EmbeddedPickerCtx { label: "BOARD".to_string(), root, then: None, board: None }, candidates);
                }
                Err(err) => self.set_error(format!("couldn't list boards: {err}")),
            },
            EmbeddedEvent::BoardOptions { root, board, result } => match result {
                Ok(options) if options.is_empty() => self.set_message(format!("{board} has no options to choose")),
                Ok(options) => {
                    let candidates = options
                        .into_iter()
                        .flat_map(|option| {
                            option.values.into_iter().map(move |value| {
                                let mark = if value.selected { "  (current)" } else { "" };
                                fenix_picker::Candidate::new(
                                    format!("{}: {}{mark}", option.label, value.label),
                                    EmbeddedPick::BoardOption { option: option.id.clone(), value: value.value },
                                )
                            })
                        })
                        .collect();
                    self.embedded_enter_picker(EmbeddedPickerCtx { label: "BOARD OPTIONS".to_string(), root, then: None, board: Some(board) }, candidates);
                }
                Err(err) => self.set_error(format!("couldn't read the board's options: {err}")),
            },
            EmbeddedEvent::Libraries { root, result } => match result {
                Ok(libraries) => {
                    self.embedded.libraries = Some(libraries.clone());
                    self.embedded_library_picker(root, libraries);
                }
                Err(err) => self.set_error(format!("couldn't load the library index: {err}")),
            },
            EmbeddedEvent::Packages { root, result } => match result {
                Ok(packages) => {
                    let candidates = packages
                        .into_iter()
                        .map(|p| {
                            let installed = match &p.installed {
                                Some(v) if *v == p.latest => "  [installed]".to_string(),
                                Some(v) => format!("  [installed {v}, update available]"),
                                None => String::new(),
                            };
                            fenix_picker::Candidate::new(format!("{}  {}  {}{installed}", p.name, p.id, p.latest), EmbeddedPick::Package(p.id))
                        })
                        .collect();
                    self.embedded_enter_picker(EmbeddedPickerCtx { label: "INSTALL BOARD PACKAGE".to_string(), root, then: None, board: None }, candidates);
                }
                Err(err) => self.set_error(format!("couldn't load the board package index: {err}")),
            },
            EmbeddedEvent::Debugging { root, port, result } => match result {
                Ok(Debugging::Unsupported(reason)) => self.set_error(reason),
                Ok(Debugging::Supported { prepare, session }) => {
                    self.embedded.after_task = Some(AfterTask::Debugger { root: root.clone(), session });
                    self.embedded_run("debug build", prepare, &root);
                }
                // A board that can be debugged, but no port chosen yet.
                Err(_) if port.is_none() => self.embedded_with_port(PortAction::Debug),
                Err(err) => self.set_error(format!("couldn't start debugging: {err}")),
            },
        }
        self.wake_caret();
    }

    fn apply_embedded_ports(&mut self, root: PathBuf, then: Option<PortAction>, result: Result<Vec<Port>, String>) {
        let mut ports = match result {
            Ok(ports) => ports,
            Err(err) => {
                self.set_error(format!("couldn't list ports: {err}"));
                return;
            }
        };
        if ports.is_empty() {
            self.set_error("no serial ports found -- plug the board in (and check its USB cable carries data)");
            return;
        }
        // Ports with a recognized board first.
        ports.sort_by_key(|p| p.boards.is_empty());
        let recognized: Vec<&Port> = ports.iter().filter(|p| !p.boards.is_empty()).collect();
        if let (Some(action), [only]) = (then, recognized.as_slice()) {
            let port = only.address.clone();
            let board = only.boards[0].name.clone();
            if let Some(project) = self.embedded_project_at(&root) {
                if let Err(err) = project.set_port(&port) {
                    self.set_error(err);
                    return;
                }
            }
            self.refresh_embedded_indicator();
            self.set_message(format!("using {port} ({board})"));
            self.embedded_do(action, &root, &port);
            return;
        }
        let current = self.embedded_project_at(&root).and_then(|p| p.port());
        let candidates = ports
            .into_iter()
            .map(|p| {
                let board = p.boards.first().map(|b| format!("  {}", b.name)).unwrap_or_default();
                let mark = if current.as_deref() == Some(p.address.as_str()) { "  (current)" } else { "" };
                fenix_picker::Candidate::new(format!("{}{board}  ({}){mark}", p.address, p.protocol), EmbeddedPick::Port(p.address))
            })
            .collect();
        self.embedded_enter_picker(EmbeddedPickerCtx { label: "PORT".to_string(), root, then, board: None }, candidates);
    }

    // -- The new-sketch prompt -------------------------------------------------

    pub(super) fn embedded_prompt_text(&self) -> Option<String> {
        let prompt = self.embedded.prompt.as_ref()?;
        Some(format!("New sketch in {}: {}", prompt.parent.display(), prompt.input))
    }

    pub(super) fn embedded_prompt_key(&mut self, keypress: KeyPress) {
        let Some(prompt) = &mut self.embedded.prompt else { return };
        match keypress.code {
            KeyCode::Named(FenixNamedKey::Escape) => self.embedded.prompt = None,
            KeyCode::Named(FenixNamedKey::Enter) => {
                let EmbeddedPrompt { parent, input } = self.embedded.prompt.take().expect("matched Some above");
                let name = input.trim();
                if name.is_empty() {
                    return;
                }
                match fenix_embedded::arduino::create_sketch(&parent, name, fenix_embedded::arduino::DEFAULT_BOARD) {
                    Ok(ino) => {
                        self.open_file_from_picker(&ino);
                        self.set_message(format!(
                            "created {} for {} -- SPC m s to change the board",
                            ino.display(),
                            fenix_embedded::arduino::DEFAULT_BOARD
                        ));
                    }
                    Err(err) => self.set_error(err),
                }
            }
            KeyCode::Named(FenixNamedKey::Backspace) => {
                prompt.input.pop();
            }
            KeyCode::Char(c) if !keypress.mods.ctrl => prompt.input.push(c),
            _ => {}
        }
        self.wake_caret();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An `App` whose tools are stand-in files in `dir`, so nothing here
    /// depends on (or runs) a real arduino-cli.
    fn app_in(dir: &Path) -> App {
        let mut app = App::with_file(None);
        app.config = fenix_config::Config::load_or_default(dir.join("config.ini"));
        let fake = |name: &str| {
            let path = dir.join("tools").join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, "").unwrap();
            Some(path)
        };
        app.config.embedded_arduino_cli = fake("arduino-cli");
        app.config.embedded_clangd = fake("clangd");
        app.config.embedded_arduino_language_server = fake("arduino-language-server");
        app
    }

    fn temp(name: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("fenix-gui-embedded-{name}-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sketch(dir: &Path, name: &str) -> PathBuf {
        fenix_embedded::arduino::create_sketch(dir, name, "arduino:avr:uno").unwrap()
    }

    fn port(address: &str, board: Option<&str>) -> Port {
        Port {
            address: address.to_string(),
            label: address.to_string(),
            protocol: "Serial Port (USB)".to_string(),
            boards: board.map(|b| vec![Board { id: "arduino:avr:uno".to_string(), name: b.to_string(), package: "arduino:avr".to_string() }]).unwrap_or_default(),
        }
    }

    fn labels(app: &App) -> Vec<String> {
        match &app.active_picker {
            Some(ActivePicker::Embedded(state)) => state.visible_rows(0, 100).map(|(_, c)| c.label.clone()).collect(),
            _ => panic!("expected the embedded picker"),
        }
    }

    fn confirm(app: &mut App, contains: &str) {
        let Some(ActivePicker::Embedded(state)) = &mut app.active_picker else { panic!("no embedded picker") };
        for c in contains.chars() {
            state.push_char(c);
        }
        app.picker_confirm();
    }

    #[test]
    fn outside_a_sketch_every_command_says_how_to_get_one() {
        let dir = temp("outside");
        let mut app = app_in(&dir);
        app.cmd_embedded_build();
        let message = app.status_message.as_ref().unwrap();
        assert!(message.is_error && message.text.contains("SPC f n"), "{}", message.text);
    }

    #[test]
    fn the_new_sketch_prompt_creates_and_opens_it() {
        let dir = temp("new");
        let mut app = app_in(&dir);
        app.embedded.prompt = Some(EmbeddedPrompt { parent: dir.clone(), input: String::new() });
        for c in "Lab2".chars() {
            app.embedded_prompt_key(KeyPress::char(c));
        }
        assert!(app.embedded_prompt_text().unwrap().ends_with("Lab2"));
        app.embedded_prompt_key(KeyPress::named(FenixNamedKey::Enter));

        let ino = dir.join("Lab2").join("Lab2.ino");
        assert!(ino.is_file());
        assert_eq!(app.open().buffer.path(), Some(ino.as_path()));
        assert!(app.embedded.prompt.is_none());
    }

    #[test]
    fn a_single_recognized_board_is_used_without_asking() {
        let dir = temp("auto_port");
        let ino = sketch(&dir, "Blink");
        let root = ino.parent().unwrap().to_path_buf();
        let mut app = app_in(&dir);
        app.apply_embedded_event(EmbeddedEvent::Ports {
            root: root.clone(),
            then: Some(PortAction::Monitor),
            result: Ok(vec![port("COM1", None), port("COM4", Some("Arduino UNO"))]),
        });

        assert_eq!(app.embedded_project_at(&root).unwrap().port().as_deref(), Some("COM4"), "the choice is saved to sketch.yaml");
        let monitor = app.embedded.monitors.get(&root).expect("the monitor was opened");
        assert_eq!(monitor.port, "COM4");
        let (program, args) = &app.terminal_buffer_programs[&monitor.buffer];
        assert!(program.ends_with("arduino-cli"), "{program}");
        assert_eq!(args[..3], ["monitor", "-p", "COM4"]);
        assert!(args.contains(&"baudrate=9600".to_string()));
    }

    #[test]
    fn unrecognized_ports_are_offered_as_a_picker_with_boards_first() {
        let dir = temp("pick_port");
        let ino = sketch(&dir, "Blink");
        let root = ino.parent().unwrap().to_path_buf();
        let mut app = app_in(&dir);
        app.apply_embedded_event(EmbeddedEvent::Ports {
            root: root.clone(),
            then: None,
            result: Ok(vec![port("COM1", None), port("COM7", None)]),
        });
        assert_eq!(labels(&app), ["COM1  (Serial Port (USB))", "COM7  (Serial Port (USB))"]);
        confirm(&mut app, "COM7");
        assert_eq!(app.embedded_project_at(&root).unwrap().port().as_deref(), Some("COM7"));
        assert!(app.embedded.indicator.is_none() || app.embedded.indicator.as_deref().is_some_and(|i| i.contains("COM7")));
    }

    #[test]
    fn no_ports_at_all_explains_what_to_check() {
        let dir = temp("no_ports");
        let ino = sketch(&dir, "Blink");
        let mut app = app_in(&dir);
        app.apply_embedded_event(EmbeddedEvent::Ports { root: ino.parent().unwrap().to_path_buf(), then: Some(PortAction::Upload), result: Ok(vec![]) });
        assert!(app.status_message.as_ref().unwrap().text.contains("plug the board in"));
    }

    #[test]
    fn choosing_a_board_option_rewrites_the_board_id() {
        let dir = temp("options");
        let ino = sketch(&dir, "Blink");
        let root = ino.parent().unwrap().to_path_buf();
        let mut app = app_in(&dir);
        app.apply_embedded_event(EmbeddedEvent::BoardOptions {
            root: root.clone(),
            board: "arduino:avr:nano".to_string(),
            result: Ok(vec![BoardOption {
                id: "cpu".to_string(),
                label: "Processor".to_string(),
                values: vec![
                    fenix_embedded::OptionValue { value: "atmega328".to_string(), label: "ATmega328P".to_string(), selected: true },
                    fenix_embedded::OptionValue { value: "atmega328old".to_string(), label: "ATmega328P (Old Bootloader)".to_string(), selected: false },
                ],
            }]),
        });
        assert_eq!(labels(&app), ["Processor: ATmega328P  (current)", "Processor: ATmega328P (Old Bootloader)"]);
        confirm(&mut app, "Old");
        assert_eq!(app.embedded_project_at(&root).unwrap().board(), ("arduino:avr:nano:cpu=atmega328old".to_string(), true));
    }

    #[test]
    fn an_unsupported_board_says_why_debugging_isnt_possible() {
        let dir = temp("debug");
        let ino = sketch(&dir, "Blink");
        let mut app = app_in(&dir);
        app.apply_embedded_event(EmbeddedEvent::Debugging {
            root: ino.parent().unwrap().to_path_buf(),
            port: None,
            result: Ok(Debugging::Unsupported("arduino:avr:uno can't be debugged with breakpoints".to_string())),
        });
        let message = app.status_message.as_ref().unwrap();
        assert!(message.is_error && message.text.contains("can't be debugged"));
    }

    #[test]
    fn a_failed_upload_restores_the_monitor_it_stopped() {
        let dir = temp("upload_monitor");
        let ino = sketch(&dir, "Blink");
        let root = ino.parent().unwrap().to_path_buf();
        let mut app = app_in(&dir);
        let buffer = app.buffers.open_terminal();
        app.embedded.monitors.insert(root.clone(), Monitor { buffer, port: "COM4".to_string() });
        app.embedded.after_task = Some(AfterTask::Upload { root: root.clone(), reopen_monitor: true });

        app.embedded_task_finished(&root, false);

        assert!(app.embedded.after_task.is_none());
        assert!(app.terminal_buffer_programs.contains_key(&buffer), "the monitor was restarted with its command");
        assert!(app.status_message.as_ref().unwrap().text.contains("right port"));
    }

    #[test]
    fn a_finished_task_in_another_project_leaves_the_follow_up_alone() {
        let dir = temp("other_root");
        let mut app = app_in(&dir);
        app.embedded.after_task = Some(AfterTask::RestartLanguageServer { root: dir.join("A") });
        app.embedded_task_finished(&dir.join("B"), true);
        assert!(app.embedded.after_task.is_some());
    }

    #[test]
    fn the_library_picker_marks_installed_and_outdated_libraries() {
        let dir = temp("libs");
        let ino = sketch(&dir, "Blink");
        let mut app = app_in(&dir);
        let lib = |name: &str, installed: Option<&str>| Library {
            name: name.to_string(),
            latest: "1.3.0".to_string(),
            author: String::new(),
            summary: format!("{name} does things"),
            installed: installed.map(str::to_string),
        };
        app.apply_embedded_event(EmbeddedEvent::Libraries {
            root: ino.parent().unwrap().to_path_buf(),
            result: Ok(vec![lib("Servo", Some("1.2.2")), lib("Stepper", Some("1.3.0")), lib("DHT", None)]),
        });
        assert_eq!(
            labels(&app),
            [
                "Servo  1.3.0  [installed 1.2.2, update available]  --  Servo does things",
                "Stepper  1.3.0  [installed]  --  Stepper does things",
                "DHT  1.3.0  --  DHT does things",
            ]
        );
        assert!(app.embedded.libraries.is_some(), "the index is kept for next time");
    }

    #[test]
    fn short_board_names_drop_the_package_and_options() {
        assert_eq!(short_board("arduino:avr:nano:cpu=atmega328old"), "nano");
        assert_eq!(short_board("arduino:avr:uno"), "uno");
    }
}
