//! Microcontroller projects (`SPC m` in a sketch), as a `Feature`: build,
//! flash, a serial monitor, debugging where the board allows it,
//! board/port/library management, and the right language server for the
//! sketch.
//!
//! Everything toolchain-specific comes from a `fenix_embedded::Platform`
//! (Arduino today); this module only turns what it returns into the
//! editor's existing machinery -- the task runner for builds and uploads
//! (so errors land in the quickfix list), a terminal pane for the
//! monitor and the debugger console, pickers for choices. Queries that
//! run the toolchain happen off the UI thread and come back as one
//! `EmbeddedEvent`. Everything it keeps between calls lives on
//! `EmbeddedHost`; `App` reaches it through the `Feature` hooks and the
//! commands in `SPEC`.

use std::any::Any;

use super::feature::{Feature, FeatureCommand, FeatureEvent, FeaturePick, FeatureSpec, LeaderEntry, Notice};
use super::*;

use fenix_embedded::{Board, BoardOption, Debugging, Library, Package, Platform, Port};

/// Runs `f` on the Arduino feature: what every command below does.
fn on(app: &mut App, f: fn(&mut EmbeddedHost, &mut App)) {
    app.with_feature::<EmbeddedHost, _>(f);
}

/// Arduino's commands, its `SPC f n`, and its `SPC m` menu.
pub(crate) static SPEC: FeatureSpec = FeatureSpec {
    id: "fenix.arduino",
    commands: &[
        FeatureCommand { id: "embedded.build", description: "Compile the sketch for its board", external: false, run: |app| on(app, EmbeddedHost::build) },
        FeatureCommand { id: "embedded.upload", description: "Compile and flash the sketch to the board", external: false, run: |app| on(app, EmbeddedHost::upload_command) },
        FeatureCommand { id: "embedded.monitor", description: "Open the serial monitor for the sketch's port", external: false, run: |app| on(app, EmbeddedHost::monitor_command) },
        FeatureCommand { id: "embedded.baud", description: "Choose the serial monitor's speed", external: false, run: |app| on(app, EmbeddedHost::baud) },
        FeatureCommand { id: "embedded.port", description: "Choose the port the board is on", external: false, run: |app| on(app, EmbeddedHost::choose_port) },
        FeatureCommand { id: "embedded.board", description: "Choose the board the sketch is built for", external: false, run: |app| on(app, EmbeddedHost::choose_board) },
        FeatureCommand { id: "embedded.board_options", description: "Choose the board's options (processor, clock...)", external: false, run: |app| on(app, EmbeddedHost::board_options) },
        FeatureCommand { id: "embedded.library", description: "Install a library from the library index", external: false, run: |app| on(app, EmbeddedHost::library) },
        FeatureCommand { id: "embedded.package", description: "Install board support (a core)", external: false, run: |app| on(app, EmbeddedHost::package) },
        FeatureCommand { id: "embedded.debug", description: "Debug the sketch on the board, if the board supports it", external: false, run: |app| on(app, EmbeddedHost::debug) },
        FeatureCommand { id: "embedded.new_sketch", description: "Create a new sketch", external: false, run: |app| on(app, EmbeddedHost::new_sketch) },
        FeatureCommand { id: "embedded.info", description: "Show the sketch's board, port, speed and tools", external: false, run: |app| on(app, EmbeddedHost::info) },
    ],
    leader: &[LeaderEntry::Bind(&['f', 'n'], "new Arduino sketch", "embedded.new_sketch")],
    local_menu: &[
        LeaderEntry::Bind(&['b'], "build (verify)", "embedded.build"),
        LeaderEntry::Bind(&['u'], "upload to board", "embedded.upload"),
        LeaderEntry::Bind(&['m'], "serial monitor", "embedded.monitor"),
        LeaderEntry::Bind(&['B'], "serial monitor speed", "embedded.baud"),
        LeaderEntry::Bind(&['p'], "choose port", "embedded.port"),
        LeaderEntry::Bind(&['s'], "choose board", "embedded.board"),
        LeaderEntry::Bind(&['o'], "board options", "embedded.board_options"),
        LeaderEntry::Bind(&['l'], "install library", "embedded.library"),
        LeaderEntry::Bind(&['c'], "install board package", "embedded.package"),
        LeaderEntry::Bind(&['d'], "debug", "embedded.debug"),
        LeaderEntry::Bind(&['n'], "new sketch", "embedded.new_sketch"),
        LeaderEntry::Bind(&['i'], "project info", "embedded.info"),
    ],
    local_context: Some(local_context),
    language_server: Some(language_server),
    tasks: Some(tasks),
    hover_fallback: None,
    completion_at_cursor: None,
    completion_pool_at: None,
};

/// Anything inside an Arduino sketch gets its `SPC m` menu.
fn local_context(app: &App) -> Option<crate::keymap::LocalContext> {
    fenix_embedded::project_root_of(&app.integration_root()).map(|_| crate::keymap::LocalContext::Arduino)
}

/// For C/C++ files in a microcontroller project, the platform's server
/// (plain clangd can't make sense of an Arduino sketch); `None`
/// otherwise, so the usual server (clangd) takes over.
fn language_server(app: &App, root: &Path, language: fenix_syntax::LanguageId) -> Option<Result<fenix_project::tools::CommandSpec, String>> {
    if !matches!(language, fenix_syntax::LanguageId::C | fenix_syntax::LanguageId::Cpp) {
        return None;
    }
    let root = fenix_embedded::project_root_of(root)?;
    let project = project_at(app, &root)?;
    Some(project.language_server().map(|command| fenix_project::tools::CommandSpec::new(command.program, command.args)))
}

/// The sketch's build, in the task picker (`SPC t t`).
fn tasks(app: &App, root: &Path) -> Vec<fenix_tasks::TaskDef> {
    let Some(project) = fenix_embedded::project_root_of(root).and_then(|root| project_at(app, &root)) else { return Vec::new() };
    let name = format!("{}: build", project.family());
    project.build().map(|c| fenix_tasks::TaskDef { name, command: c.program, args: c.args }).into_iter().collect()
}

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

/// Everything Arduino keeps between calls.
#[derive(Default)]
pub(crate) struct EmbeddedHost {
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

/// One row of Arduino's picker.
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

impl EmbeddedEvent {
    fn into_user_event(self) -> FenixUserEvent {
        // The label the hang log knew it by as a `FenixUserEvent` variant.
        FenixUserEvent::Feature(FeatureEvent { feature: SPEC.id, label: "Embedded", payload: Box::new(self) })
    }
}

/// `arduino:avr:nano:cpu=atmega328old` -> `nano`.
fn short_board(board: &str) -> &str {
    board.split(':').nth(2).unwrap_or(board)
}

/// The Arduino tools, as the settings and a search of the usual places
/// find them. Also what the project doctor uses to locate them.
pub(super) fn tools(app: &App) -> fenix_embedded::Tools {
    fenix_embedded::Tools::discover(&fenix_embedded::ToolOverrides {
        arduino_cli: app.config.embedded_arduino_cli.clone(),
        clangd: app.config.embedded_clangd.clone(),
        arduino_language_server: app.config.embedded_arduino_language_server.clone(),
    })
}

/// The embedded project rooted at `root`, if it is one.
pub(super) fn project_at(app: &App, root: &Path) -> Option<Box<dyn Platform>> {
    fenix_embedded::detect(root, &tools(app))
}

/// The embedded project the focused buffer belongs to, or an error
/// saying there isn't one.
fn project(app: &mut App) -> Option<Box<dyn Platform>> {
    let root = app.integration_root();
    let project = fenix_embedded::project_root_of(&root).and_then(|root| project_at(app, &root));
    if project.is_none() {
        app.set_error("not in an Arduino sketch -- open a sketch's .ino file, or SPC f n to create one");
    }
    project
}

fn run(app: &mut App, name: &str, command: fenix_embedded::Command, root: &Path) {
    app.run_task(fenix_tasks::TaskDef { name: name.to_string(), command: command.program, args: command.args }, root.to_path_buf());
}

fn restart_language_server(app: &mut App, root: &Path) {
    let root = fenix_lsp::normalize(refactor::identity(root));
    app.lsp_unavailable.retain(|key| key.root != root);
    let removed: Vec<_> = app.lsp_sessions.keys().filter(|key| key.root == root).cloned().collect();
    for key in removed {
        app.stop_lsp_session(&key);
    }
    app.sync_lsp_for_focused_buffer();
}

/// Opens an interactive program (the debugger console) in a pane.
fn open_console(app: &mut App, root: &Path, label: &str, command: fenix_embedded::Command) {
    let buffer = app.buffers.open_terminal();
    app.terminal_buffer_cwds.insert(buffer, root.to_path_buf());
    app.terminal_buffer_labels.insert(buffer, format!("*{label}*"));
    app.terminal_buffer_programs.insert(buffer, (command.program, command.args));
    let pane = app.windows_mut().split(SplitKind::Horizontal, buffer);
    app.workspaces.active_pane_states_mut().insert(pane, PaneState::seeded_at(Cursor::at_start()));
    app.windows_mut().resize_focused(0.45);
    app.windows_mut().focus(pane);
    app.pane_titles.insert(pane, format!("{label}  --  Ctrl-\\ to leave"));
    app.spawn_terminal_for(buffer);
}

impl EmbeddedHost {
    /// Runs `job` off the UI thread (inline without an event loop).
    pub(super) fn spawn(&mut self, app: &mut App, job: impl FnOnce() -> EmbeddedEvent + Send + 'static) {
        match app.event_proxy.clone() {
            Some(proxy) => {
                std::thread::spawn(move || {
                    let _ = proxy.send_event(job().into_user_event());
                });
            }
            None => {
                let event = job();
                self.apply_event(app, event);
            }
        }
    }

    /// The modeline's `uno · COM4`, recomputed on focus changes and after
    /// the board or port changes -- not per frame, since it reads files.
    pub(super) fn refresh_indicator(&mut self, app: &App) {
        let root = app.open().buffer.path().and_then(fenix_embedded::project_root_of);
        self.indicator = root.and_then(|root| project_at(app, &root)).map(|p| {
            let (board, chosen) = p.board();
            let board = if chosen { short_board(&board).to_string() } else { format!("{} (default)", short_board(&board)) };
            format!("   {board} · {}", p.port().unwrap_or_else(|| "no port".to_string()))
        });
    }

    // -- Commands -----------------------------------------------------------

    /// `SPC m b`: compile.
    pub(super) fn build(&mut self, app: &mut App) {
        let Some(project) = project(app) else { return };
        let root = project.root().to_path_buf();
        let (board, chosen) = project.board();
        match project.build() {
            Ok(command) => {
                run(app, &format!("{}: build", project.family()), command, &root);
                if !chosen {
                    app.set_message(format!("building for {board}, the default -- SPC m s picks your board"));
                }
            }
            Err(err) => app.set_error(err),
        }
    }

    /// `SPC m u`: build and flash.
    fn upload_command(&mut self, app: &mut App) {
        self.with_port(app, PortAction::Upload);
    }

    /// `SPC m m`: the serial monitor -- opened, or brought back.
    fn monitor_command(&mut self, app: &mut App) {
        self.with_port(app, PortAction::Monitor);
    }

    /// `SPC m d`: debug, if the board can be.
    fn debug(&mut self, app: &mut App) {
        let Some(project) = project(app) else { return };
        let root = project.root().to_path_buf();
        let port = project.port();
        app.set_message("checking whether this board can be debugged...");
        self.spawn(app, move || {
            let result = project.debugging(port.as_deref());
            EmbeddedEvent::Debugging { root, port, result }
        });
    }

    /// `SPC m p`: choose the port.
    fn choose_port(&mut self, app: &mut App) {
        let Some(project) = project(app) else { return };
        let root = project.root().to_path_buf();
        app.set_message("looking for boards...");
        self.spawn(app, move || EmbeddedEvent::Ports { root, then: None, result: project.ports() });
    }

    /// `SPC m s`: choose the board.
    fn choose_board(&mut self, app: &mut App) {
        let Some(project) = project(app) else { return };
        let root = project.root().to_path_buf();
        self.spawn(app, move || EmbeddedEvent::Boards { root, result: project.boards() });
    }

    /// The project doctor's "pick a port" fix, for the sketch at `root`.
    pub(super) fn doctor_pick_port(&mut self, app: &mut App, project: Box<dyn Platform>) {
        let root = project.root().to_path_buf();
        app.set_message("looking for boards...");
        self.spawn(app, move || EmbeddedEvent::Ports { root, then: None, result: project.ports() });
    }

    /// The project doctor's "pick a board" fix, for the sketch at `root`.
    pub(super) fn doctor_pick_board(&mut self, app: &mut App, project: Box<dyn Platform>) {
        let root = project.root().to_path_buf();
        self.spawn(app, move || EmbeddedEvent::Boards { root, result: project.boards() });
    }

    /// `SPC m o`: the board's options (processor, clock...).
    fn board_options(&mut self, app: &mut App) {
        let Some(project) = project(app) else { return };
        let root = project.root().to_path_buf();
        let (board, _) = project.board();
        self.spawn(app, move || {
            let result = project.board_options(&board);
            EmbeddedEvent::BoardOptions { root, board, result }
        });
    }

    /// `SPC m B`: the monitor's speed.
    fn baud(&mut self, app: &mut App) {
        let Some(project) = project(app) else { return };
        let current = project.baud_rate();
        let candidates = fenix_embedded::arduino::BAUD_RATES
            .iter()
            .map(|&baud| {
                let label = if baud == current { format!("{baud} baud  (current)") } else { format!("{baud} baud") };
                FeaturePick::candidate(label, EmbeddedPick::Baud(baud))
            })
            .collect();
        let root = project.root().to_path_buf();
        self.enter_picker(app, EmbeddedPickerCtx { label: "SERIAL SPEED".to_string(), root, then: None, board: None }, candidates);
    }

    /// `SPC m l`: install a library.
    fn library(&mut self, app: &mut App) {
        let Some(project) = project(app) else { return };
        let root = project.root().to_path_buf();
        if let Some(libraries) = self.libraries.clone() {
            self.library_picker(app, root, libraries);
            return;
        }
        app.set_message("loading the library index...");
        self.spawn(app, move || EmbeddedEvent::Libraries { root, result: project.search_libraries("") });
    }

    /// `SPC m c`: install board support (a core).
    fn package(&mut self, app: &mut App) {
        let Some(project) = project(app) else { return };
        let root = project.root().to_path_buf();
        app.set_message("loading the board package index...");
        self.spawn(app, move || EmbeddedEvent::Packages { root, result: project.search_packages("") });
    }

    /// `SPC m n`: a new sketch, next to the current one (or in the
    /// project/working directory when not in one).
    fn new_sketch(&mut self, app: &mut App) {
        let root = app.integration_root();
        let parent = match fenix_embedded::project_root_of(&root) {
            Some(sketch) => sketch.parent().map(Path::to_path_buf).unwrap_or(root),
            None => root,
        };
        self.prompt = Some(EmbeddedPrompt { parent, input: String::new() });
    }

    /// `SPC m i`: what Fenix knows about this project and its tools.
    fn info(&mut self, app: &mut App) {
        let Some(project) = project(app) else { return };
        let (board, chosen) = project.board();
        let board = if chosen { board } else { format!("{board} (default -- SPC m s to choose)") };
        let port = project.port().unwrap_or_else(|| "none yet -- SPC m p".to_string());
        let completion = match project.language_server() {
            Ok(_) => "completion ready".to_string(),
            Err(err) => err,
        };
        app.set_message(format!("{} sketch · board {board} · port {port} · {} baud · {completion}", project.family(), project.baud_rate()));
    }

    // -- Ports and the actions that need one --------------------------------

    /// Runs `action` on the project's port, finding one first if none is
    /// set: a single port with a recognized board is used (and saved);
    /// otherwise you pick.
    fn with_port(&mut self, app: &mut App, action: PortAction) {
        let Some(project) = project(app) else { return };
        let root = project.root().to_path_buf();
        if let Some(port) = project.port() {
            self.do_action(app, action, &root, &port);
            return;
        }
        app.set_message("looking for your board...");
        self.spawn(app, move || EmbeddedEvent::Ports { root, then: Some(action), result: project.ports() });
    }

    fn do_action(&mut self, app: &mut App, action: PortAction, root: &Path, port: &str) {
        match action {
            PortAction::Upload => self.upload(app, root, port),
            PortAction::Monitor => self.open_monitor(app, root, port),
            PortAction::Debug => self.debug(app),
        }
    }

    fn upload(&mut self, app: &mut App, root: &Path, port: &str) {
        let Some(project) = project_at(app, root) else { return };
        let command = match project.upload(port) {
            Ok(command) => command,
            Err(err) => {
                app.set_error(err);
                return;
            }
        };
        // Only one program can hold a serial port: the monitor steps
        // aside for the upload and comes back after it.
        let reopen_monitor = self.stop_monitor(app, root);
        self.after_task = Some(AfterTask::Upload { root: root.to_path_buf(), reopen_monitor });
        run(app, &format!("{}: upload to {port}", project.family()), command, root);
    }

    // -- The serial monitor ---------------------------------------------------

    /// Opens the monitor for `root` on `port` in a pane under the current
    /// one -- or restarts it there if it's already open but stopped, or
    /// on a different port.
    fn open_monitor(&mut self, app: &mut App, root: &Path, port: &str) {
        let Some(project) = project_at(app, root) else { return };
        let command = match project.monitor(port) {
            Ok(command) => command,
            Err(err) => {
                app.set_error(err);
                return;
            }
        };
        let label = format!("*serial {port} {} baud*", project.baud_rate());
        let previous_port = self.monitors.get(root).map(|m| m.port.clone());
        let existing = self.monitors.get(root).map(|m| m.buffer).filter(|id| app.buffers.get(*id).is_some());
        let buffer = match existing {
            Some(buffer) => buffer,
            None => {
                let buffer = app.buffers.open_terminal();
                app.terminal_buffer_cwds.insert(buffer, root.to_path_buf());
                buffer
            }
        };
        app.terminal_buffer_labels.insert(buffer, label.clone());
        app.terminal_buffer_programs.insert(buffer, (command.program, command.args));
        self.monitors.insert(root.to_path_buf(), Monitor { buffer, port: port.to_string() });

        let running = existing.is_some()
            && previous_port.as_deref() == Some(port)
            && app.terminal_buffers.get_mut(&buffer).is_some_and(|t| t.session.is_alive());
        let shown = app.windows().windows().into_iter().find(|&p| app.windows().content(p) == Some(&buffer));
        match shown {
            Some(pane) => {
                app.windows_mut().focus(pane);
            }
            None => {
                let pane = app.windows_mut().split(SplitKind::Horizontal, buffer);
                app.workspaces.active_pane_states_mut().insert(pane, PaneState::seeded_at(Cursor::at_start()));
                app.windows_mut().resize_focused(0.35);
                app.windows_mut().focus(pane);
            }
        }
        if let Some(pane) = app.windows().windows().into_iter().find(|&p| app.windows().content(p) == Some(&buffer)) {
            app.pane_titles.insert(pane, format!("Serial monitor  --  {port} at {} baud  --  Ctrl-\\ to leave, SPC m B for speed", project.baud_rate()));
        }
        if running {
            app.focus_terminal_buffer(buffer);
        } else {
            app.terminal_buffers.remove(&buffer);
            app.spawn_terminal_for(buffer);
        }
        app.wake_caret();
    }

    /// Stops `root`'s monitor, keeping its pane and what it printed.
    /// Returns whether one was running.
    fn stop_monitor(&mut self, app: &mut App, root: &Path) -> bool {
        let Some(monitor) = self.monitors.get(root) else { return false };
        let buffer = monitor.buffer;
        let was_running = app.terminal_buffers.get_mut(&buffer).is_some_and(|t| t.session.is_alive());
        app.terminal_buffers.remove(&buffer);
        if app.terminal_buffer_focused == Some(buffer) {
            app.terminal_buffer_focused = None;
        }
        was_running
    }

    /// Restarts `root`'s monitor with its current settings, if it has one
    /// open -- after an upload, or a speed change.
    fn restart_monitor(&mut self, app: &mut App, root: &Path) {
        let Some(monitor) = self.monitors.get(root).cloned() else { return };
        if app.buffers.get(monitor.buffer).is_none() {
            self.monitors.remove(root);
            return;
        }
        let Some(project) = project_at(app, root) else { return };
        if let Ok(command) = project.monitor(&monitor.port) {
            app.terminal_buffer_labels.insert(monitor.buffer, format!("*serial {} {} baud*", monitor.port, project.baud_rate()));
            app.terminal_buffer_programs.insert(monitor.buffer, (command.program, command.args));
        }
        app.terminal_buffers.remove(&monitor.buffer);
        app.spawn_terminal_for(monitor.buffer);
    }

    // -- Finished tasks -----------------------------------------------------

    /// Called when a task finishes, to follow up on one this module ran.
    pub(super) fn after_task_finished(&mut self, app: &mut App, root: &Path, success: bool) {
        let matches = |after: &AfterTask| match after {
            AfterTask::Upload { root: r, .. } | AfterTask::Debugger { root: r, .. } | AfterTask::RestartLanguageServer { root: r } => r == root,
        };
        if !self.after_task.as_ref().is_some_and(matches) {
            return;
        }
        match self.after_task.take() {
            Some(AfterTask::Upload { root, reopen_monitor }) => {
                if reopen_monitor {
                    self.restart_monitor(app, &root);
                }
                if success {
                    app.set_message("uploaded");
                } else {
                    app.set_error("upload failed -- is the board plugged in, and is it the right port? (SPC m p)");
                }
            }
            Some(AfterTask::Debugger { root, session }) => {
                if success {
                    open_console(app, &root, "debugger", session);
                } else {
                    app.set_error("the debug build didn't flash, so there's nothing to attach to -- see the task output");
                }
            }
            Some(AfterTask::RestartLanguageServer { root }) => {
                self.libraries = None;
                if success {
                    restart_language_server(app, &root);
                }
            }
            None => {}
        }
    }

    // -- Pickers ------------------------------------------------------------

    fn enter_picker(&mut self, app: &mut App, ctx: EmbeddedPickerCtx, candidates: Vec<fenix_picker::Candidate<FeaturePick>>) {
        self.picker = Some(ctx);
        app.enter_picker(ActivePicker::Feature { feature: SPEC.id, badge: "EMBEDDED", state: fenix_picker::PickerState::new(candidates) });
    }

    fn library_picker(&mut self, app: &mut App, root: PathBuf, libraries: Vec<Library>) {
        let candidates = libraries
            .into_iter()
            .map(|lib| {
                let installed = match &lib.installed {
                    Some(v) if *v == lib.latest => "  [installed]".to_string(),
                    Some(v) => format!("  [installed {v}, update available]"),
                    None => String::new(),
                };
                let label = format!("{}  {}{installed}  --  {}", lib.name, lib.latest, lib.summary);
                FeaturePick::candidate(label, EmbeddedPick::Library(lib.name))
            })
            .collect();
        self.enter_picker(app, EmbeddedPickerCtx { label: "INSTALL LIBRARY".to_string(), root, then: None, board: None }, candidates);
    }

    /// Saves a choice to the sketch; `false` (with the error shown) if
    /// that failed.
    fn saved(&mut self, app: &mut App, result: Result<(), String>) -> bool {
        match result {
            Ok(()) => {
                self.refresh_indicator(app);
                true
            }
            Err(err) => {
                app.set_error(err);
                false
            }
        }
    }

    pub(super) fn confirm_pick(&mut self, app: &mut App, pick: EmbeddedPick) {
        let Some(ctx) = self.picker.take() else { return };
        let Some(project) = project_at(app, &ctx.root) else { return };
        match pick {
            EmbeddedPick::Board(board) => {
                if self.saved(app, project.set_board(&board)) {
                    app.set_message(format!("board: {board} -- SPC m o for its options"));
                    restart_language_server(app, &ctx.root);
                }
            }
            EmbeddedPick::MoreBoards => {
                app.set_message("loading the board package index...");
                let root = ctx.root.clone();
                self.spawn(app, move || EmbeddedEvent::Packages { root, result: project.search_packages("") });
            }
            EmbeddedPick::BoardOption { option, value } => {
                let board = ctx.board.unwrap_or_else(|| project.board().0);
                let board = project.board_with_option(&board, &option, &value);
                if self.saved(app, project.set_board(&board)) {
                    app.set_message(format!("board: {board}"));
                    restart_language_server(app, &ctx.root);
                }
            }
            EmbeddedPick::Port(port) => {
                if self.saved(app, project.set_port(&port)) {
                    match ctx.then {
                        Some(action) => self.do_action(app, action, &ctx.root, &port),
                        None => app.set_message(format!("port: {port}")),
                    }
                }
            }
            EmbeddedPick::Baud(baud) => {
                if self.saved(app, project.set_baud_rate(baud)) {
                    app.set_message(format!("serial monitor speed: {baud} baud -- match your Serial.begin({baud})"));
                    self.restart_monitor(app, &ctx.root);
                }
            }
            EmbeddedPick::Library(name) => match project.install_library(&name) {
                Ok(command) => {
                    self.after_task = Some(AfterTask::RestartLanguageServer { root: ctx.root.clone() });
                    run(app, &format!("install library {name}"), command, &ctx.root);
                }
                Err(err) => app.set_error(err),
            },
            EmbeddedPick::Package(id) => match project.install_package(&id) {
                Ok(command) => {
                    self.after_task = Some(AfterTask::RestartLanguageServer { root: ctx.root.clone() });
                    run(app, &format!("install {id}"), command, &ctx.root);
                }
                Err(err) => app.set_error(err),
            },
        }
    }

    // -- Background results -------------------------------------------------

    pub(super) fn apply_event(&mut self, app: &mut App, event: EmbeddedEvent) {
        match event {
            EmbeddedEvent::Ports { root, then, result } => self.apply_ports(app, root, then, result),
            EmbeddedEvent::Boards { root, result } => match result {
                Ok(boards) => {
                    let current = project_at(app, &root).map(|p| p.board().0).unwrap_or_default();
                    let mut candidates: Vec<fenix_picker::Candidate<FeaturePick>> = boards
                        .into_iter()
                        .map(|b| {
                            let mark = if current.split(':').take(3).eq(b.id.split(':')) { "  (current)" } else { "" };
                            FeaturePick::candidate(format!("{}  {}{mark}", b.name, b.id), EmbeddedPick::Board(b.id))
                        })
                        .collect();
                    candidates.push(FeaturePick::candidate("Install more boards...", EmbeddedPick::MoreBoards));
                    self.enter_picker(app, EmbeddedPickerCtx { label: "BOARD".to_string(), root, then: None, board: None }, candidates);
                }
                Err(err) => app.set_error(format!("couldn't list boards: {err}")),
            },
            EmbeddedEvent::BoardOptions { root, board, result } => match result {
                Ok(options) if options.is_empty() => app.set_message(format!("{board} has no options to choose")),
                Ok(options) => {
                    let candidates = options
                        .into_iter()
                        .flat_map(|option| {
                            option.values.into_iter().map(move |value| {
                                let mark = if value.selected { "  (current)" } else { "" };
                                FeaturePick::candidate(
                                    format!("{}: {}{mark}", option.label, value.label),
                                    EmbeddedPick::BoardOption { option: option.id.clone(), value: value.value },
                                )
                            })
                        })
                        .collect();
                    self.enter_picker(app, EmbeddedPickerCtx { label: "BOARD OPTIONS".to_string(), root, then: None, board: Some(board) }, candidates);
                }
                Err(err) => app.set_error(format!("couldn't read the board's options: {err}")),
            },
            EmbeddedEvent::Libraries { root, result } => match result {
                Ok(libraries) => {
                    self.libraries = Some(libraries.clone());
                    self.library_picker(app, root, libraries);
                }
                Err(err) => app.set_error(format!("couldn't load the library index: {err}")),
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
                            FeaturePick::candidate(format!("{}  {}  {}{installed}", p.name, p.id, p.latest), EmbeddedPick::Package(p.id))
                        })
                        .collect();
                    self.enter_picker(app, EmbeddedPickerCtx { label: "INSTALL BOARD PACKAGE".to_string(), root, then: None, board: None }, candidates);
                }
                Err(err) => app.set_error(format!("couldn't load the board package index: {err}")),
            },
            EmbeddedEvent::Debugging { root, port, result } => match result {
                Ok(Debugging::Unsupported(reason)) => app.set_error(reason),
                Ok(Debugging::Supported { prepare, session }) => {
                    self.after_task = Some(AfterTask::Debugger { root: root.clone(), session });
                    run(app, "debug build", prepare, &root);
                }
                // A board that can be debugged, but no port chosen yet.
                Err(_) if port.is_none() => self.with_port(app, PortAction::Debug),
                Err(err) => app.set_error(format!("couldn't start debugging: {err}")),
            },
        }
        app.wake_caret();
    }

    fn apply_ports(&mut self, app: &mut App, root: PathBuf, then: Option<PortAction>, result: Result<Vec<Port>, String>) {
        let mut ports = match result {
            Ok(ports) => ports,
            Err(err) => {
                app.set_error(format!("couldn't list ports: {err}"));
                return;
            }
        };
        if ports.is_empty() {
            app.set_error("no serial ports found -- plug the board in (and check its USB cable carries data)");
            return;
        }
        // Ports with a recognized board first.
        ports.sort_by_key(|p| p.boards.is_empty());
        let recognized: Vec<&Port> = ports.iter().filter(|p| !p.boards.is_empty()).collect();
        if let (Some(action), [only]) = (then, recognized.as_slice()) {
            let port = only.address.clone();
            let board = only.boards[0].name.clone();
            if let Some(project) = project_at(app, &root) {
                if let Err(err) = project.set_port(&port) {
                    app.set_error(err);
                    return;
                }
            }
            self.refresh_indicator(app);
            app.set_message(format!("using {port} ({board})"));
            self.do_action(app, action, &root, &port);
            return;
        }
        let current = project_at(app, &root).and_then(|p| p.port());
        let candidates = ports
            .into_iter()
            .map(|p| {
                let board = p.boards.first().map(|b| format!("  {}", b.name)).unwrap_or_default();
                let mark = if current.as_deref() == Some(p.address.as_str()) { "  (current)" } else { "" };
                FeaturePick::candidate(format!("{}{board}  ({}){mark}", p.address, p.protocol), EmbeddedPick::Port(p.address))
            })
            .collect();
        self.enter_picker(app, EmbeddedPickerCtx { label: "PORT".to_string(), root, then, board: None }, candidates);
    }

    // -- The new-sketch prompt -------------------------------------------------

    pub(super) fn sketch_prompt_text(&self) -> Option<String> {
        let prompt = self.prompt.as_ref()?;
        Some(format!("New sketch in {}: {}", prompt.parent.display(), prompt.input))
    }

    pub(super) fn sketch_prompt_key(&mut self, app: &mut App, keypress: KeyPress) {
        let Some(prompt) = &mut self.prompt else { return };
        match keypress.code {
            KeyCode::Named(FenixNamedKey::Escape) => self.prompt = None,
            KeyCode::Named(FenixNamedKey::Enter) => {
                let EmbeddedPrompt { parent, input } = self.prompt.take().expect("matched Some above");
                let name = input.trim();
                if name.is_empty() {
                    return;
                }
                match fenix_embedded::arduino::create_sketch(&parent, name, fenix_embedded::arduino::DEFAULT_BOARD) {
                    Ok(ino) => {
                        app.open_file_from_picker(&ino);
                        app.set_message(format!(
                            "created {} for {} -- SPC m s to change the board",
                            ino.display(),
                            fenix_embedded::arduino::DEFAULT_BOARD
                        ));
                    }
                    Err(err) => app.set_error(err),
                }
            }
            KeyCode::Named(FenixNamedKey::Backspace) => {
                prompt.input.pop();
            }
            KeyCode::Char(c) if !keypress.mods.ctrl => prompt.input.push(c),
            _ => {}
        }
        app.wake_caret();
    }
}

impl Feature for EmbeddedHost {
    fn spec(&self) -> &'static FeatureSpec {
        &SPEC
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn on_event(&mut self, app: &mut App, payload: Box<dyn Any + Send>) {
        let Ok(event) = payload.downcast::<EmbeddedEvent>() else { return };
        self.apply_event(app, *event);
    }

    /// The new-sketch prompt takes every key while it's open.
    fn capture_key(&mut self, app: &mut App, key: KeyPress) -> bool {
        if self.prompt.is_none() {
            return false;
        }
        self.sketch_prompt_key(app, key);
        true
    }

    fn prompt_text(&self) -> Option<String> {
        self.sketch_prompt_text()
    }

    fn modeline_indicator(&self) -> Option<&str> {
        self.indicator.as_deref()
    }

    fn notice(&mut self, app: &mut App, notice: &Notice) {
        match notice {
            Notice::FocusChanged => self.refresh_indicator(app),
            Notice::TaskFinished { root, success } => self.after_task_finished(app, root, *success),
            // Forget a closed terminal if it was a monitor.
            Notice::TerminalClosed(buffer) => self.monitors.retain(|_, m| m.buffer != *buffer),
            Notice::Tick => {}
        }
    }

    fn picker_label(&self) -> Option<&str> {
        self.picker.as_ref().map(|ctx| ctx.label.as_str())
    }

    fn picker_confirm(&mut self, app: &mut App, pick: &dyn Any) {
        if let Some(pick) = pick.downcast_ref::<EmbeddedPick>() {
            self.confirm_pick(app, pick.clone());
        }
    }

    fn picker_cancel(&mut self) {
        self.picker = None;
    }
}

/// Test-only ways in, under the names the tests have always used.
#[cfg(test)]
impl App {
    pub(super) fn embedded(&self) -> &EmbeddedHost {
        self.features.get::<EmbeddedHost>().expect("Arduino is a built-in feature")
    }

    pub(super) fn embedded_mut(&mut self) -> &mut EmbeddedHost {
        self.features.get_mut::<EmbeddedHost>().expect("Arduino is a built-in feature")
    }

    fn with_embedded<R>(&mut self, f: impl FnOnce(&mut EmbeddedHost, &mut App) -> R) -> R {
        self.with_feature::<EmbeddedHost, _>(f).expect("Arduino is a built-in feature")
    }

    fn cmd_embedded_build(&mut self) {
        self.with_embedded(|host, app| host.build(app));
    }

    fn embedded_prompt_key(&mut self, keypress: KeyPress) {
        self.with_embedded(|host, app| host.sketch_prompt_key(app, keypress));
    }

    fn embedded_prompt_text(&self) -> Option<String> {
        self.embedded().sketch_prompt_text()
    }

    fn apply_embedded_event(&mut self, event: EmbeddedEvent) {
        self.with_embedded(|host, app| host.apply_event(app, event));
    }

    fn embedded_project_at(&self, root: &Path) -> Option<Box<dyn Platform>> {
        project_at(self, root)
    }

    fn embedded_task_finished(&mut self, root: &Path, success: bool) {
        self.with_embedded(|host, app| host.after_task_finished(app, root, success));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An `App` whose tools are stand-in files in `dir`, so nothing here
    /// depends on (or runs) a real arduino-cli.
    fn app_in(dir: &Path) -> App {
        let mut app = App::with_file(None);
        app.config = fenix_config::Config::load_or_default(dir.join("settings.toml"));
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
        // Process ids come round again: start from nothing, not from what
        // an earlier run with the same id left.
        let _ = std::fs::remove_dir_all(&dir);
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
            Some(ActivePicker::Feature { state, .. }) => state.visible_rows(0, 100).map(|(_, c)| c.label.clone()).collect(),
            _ => panic!("expected the embedded picker"),
        }
    }

    fn confirm(app: &mut App, contains: &str) {
        let Some(ActivePicker::Feature { state, .. }) = &mut app.active_picker else { panic!("no embedded picker") };
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
        app.embedded_mut().prompt = Some(EmbeddedPrompt { parent: dir.clone(), input: String::new() });
        for c in "Lab2".chars() {
            app.embedded_prompt_key(KeyPress::char(c));
        }
        assert!(app.embedded_prompt_text().unwrap().ends_with("Lab2"));
        app.embedded_prompt_key(KeyPress::named(FenixNamedKey::Enter));

        let ino = dir.join("Lab2").join("Lab2.ino");
        assert!(ino.is_file());
        assert_eq!(app.open().buffer.path(), Some(ino.as_path()));
        assert!(app.embedded().prompt.is_none());
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
        let monitor = app.embedded().monitors.get(&root).expect("the monitor was opened");
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
        assert!(app.embedded().indicator.is_none() || app.embedded().indicator.as_deref().is_some_and(|i| i.contains("COM7")));
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
        app.embedded_mut().monitors.insert(root.clone(), Monitor { buffer, port: "COM4".to_string() });
        app.embedded_mut().after_task = Some(AfterTask::Upload { root: root.clone(), reopen_monitor: true });

        app.embedded_task_finished(&root, false);

        assert!(app.embedded().after_task.is_none());
        assert!(app.terminal_buffer_programs.contains_key(&buffer), "the monitor was restarted with its command");
        assert!(app.status_message.as_ref().unwrap().text.contains("right port"));
    }

    #[test]
    fn a_finished_task_in_another_project_leaves_the_follow_up_alone() {
        let dir = temp("other_root");
        let mut app = app_in(&dir);
        app.embedded_mut().after_task = Some(AfterTask::RestartLanguageServer { root: dir.join("A") });
        app.embedded_task_finished(&dir.join("B"), true);
        assert!(app.embedded().after_task.is_some());
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
        assert!(app.embedded().libraries.is_some(), "the index is kept for next time");
    }

    #[test]
    fn short_board_names_drop_the_package_and_options() {
        assert_eq!(short_board("arduino:avr:nano:cpu=atmega328old"), "nano");
        assert_eq!(short_board("arduino:avr:uno"), "uno");
    }

    // Through the `Feature` hooks, the way `App` reaches Arduino now.

    #[test]
    fn arduino_contributes_its_commands_and_keys_from_its_spec() {
        let registry = CommandRegistry::with_builtins();
        let ids: Vec<&str> = registry.iter().filter(|c| c.id.starts_with("embedded.")).map(|c| c.id).collect();
        assert_eq!(ids.len(), 12, "{ids:?}");
        assert!(registry.iter().filter(|c| c.id.starts_with("embedded.")).all(|c| !c.external()));
        assert_eq!(keymap::leader_keys().get("embedded.new_sketch").map(String::as_str), Some("SPC f n"));
        let menu = keymap::local_trie(&[crate::keymap::LocalContext::Arduino]);
        assert!(matches!(menu.matcher().feed(KeyPress::char('B')), fenix_keymap::Step::Matched(&"embedded.baud")));
    }

    #[test]
    fn a_toolchain_answer_reaches_the_host_through_the_event_loop_path() {
        let dir = temp("event_path");
        let ino = sketch(&dir, "Blink");
        let mut app = app_in(&dir);
        app.handle_user_event(EmbeddedEvent::Ports { root: ino.parent().unwrap().to_path_buf(), then: None, result: Ok(vec![]) }.into_user_event());
        assert!(app.status_message.as_ref().unwrap().text.contains("plug the board in"));
    }

    #[test]
    fn its_picker_shows_its_label_and_cancelling_forgets_what_it_was_for() {
        let dir = temp("picker_cancel");
        let ino = sketch(&dir, "Blink");
        let mut app = app_in(&dir);
        app.apply_embedded_event(EmbeddedEvent::Ports { root: ino.parent().unwrap().to_path_buf(), then: None, result: Ok(vec![port("COM1", None)]) });
        assert!(matches!(app.active_picker, Some(ActivePicker::Feature { feature: "fenix.arduino", badge: "EMBEDDED", .. })));
        assert_eq!(app.features.by_id("fenix.arduino").and_then(|f| f.picker_label()), Some("PORT"));
        app.picker_cancel();
        assert!(app.active_picker.is_none());
        assert!(app.embedded().picker.is_none());
    }

    #[test]
    fn the_sketch_prompt_takes_keys_only_while_it_is_open() {
        let dir = temp("prompt_hook");
        let mut app = app_in(&dir);
        assert!(!app.feature_claims(|feature, app| feature.capture_key(app, KeyPress::char('x'))));
        app.embedded_mut().prompt = Some(EmbeddedPrompt { parent: dir.clone(), input: String::new() });
        assert!(app.feature_claims(|feature, app| feature.capture_key(app, KeyPress::char('x'))));
        assert!(app.features.iter().find_map(|feature| feature.prompt_text()).is_some_and(|text| text.ends_with(": x")));
        app.feature_claims(|feature, app| feature.capture_key(app, KeyPress::named(FenixNamedKey::Escape)));
        assert!(app.embedded().prompt.is_none());
    }

    #[test]
    fn focusing_a_sketch_puts_its_board_and_port_in_the_modeline() {
        let dir = temp("indicator");
        let ino = sketch(&dir, "Blink");
        let mut app = app_in(&dir);
        app.test_open_path(&ino);
        app.notify_features(Notice::FocusChanged);
        let indicator = app.features.iter().find_map(|feature| feature.modeline_indicator()).map(str::to_string);
        assert!(indicator.as_deref().is_some_and(|i| i.contains("uno") && i.contains("no port")), "{indicator:?}");
    }

    #[test]
    fn its_language_server_and_tasks_are_only_for_sketches() {
        let dir = temp("server_tasks");
        let ino = sketch(&dir, "Blink");
        let root = ino.parent().unwrap().to_path_buf();
        let app = app_in(&dir);
        let (server, tasks) = (SPEC.language_server.unwrap(), SPEC.tasks.unwrap());
        assert!(server(&app, &root, fenix_syntax::LanguageId::Cpp).is_some());
        assert!(server(&app, &root, fenix_syntax::LanguageId::Rust).is_none());
        assert!(server(&app, &dir, fenix_syntax::LanguageId::Cpp).is_none(), "not a sketch");
        assert_eq!(tasks(&app, &root).len(), 1);
        assert!(tasks(&app, &dir).is_empty());
    }

    #[test]
    fn a_closed_terminal_is_forgotten_as_a_monitor() {
        let dir = temp("terminal_closed");
        let mut app = app_in(&dir);
        let buffer = app.buffers.open_terminal();
        app.embedded_mut().monitors.insert(dir.clone(), Monitor { buffer, port: "COM4".to_string() });
        app.notify_features(Notice::TerminalClosed(buffer));
        assert!(app.embedded().monitors.is_empty());
    }

    #[test]
    fn its_spc_m_menu_applies_inside_a_sketch_only() {
        let dir = temp("local_context");
        let ino = sketch(&dir, "Blink");
        let mut app = app_in(&dir);
        assert_eq!(app.local_contexts(), Vec::<crate::keymap::LocalContext>::new());
        app.test_open_path(&ino);
        assert_eq!(app.local_contexts(), [crate::keymap::LocalContext::Arduino]);
    }

    #[test]
    fn a_sketch_made_from_the_prompt_shows_its_board_in_the_modeline() {
        // The prompt opens the new sketch from inside Arduino's own key
        // hook, while it's out of `App`: the focus change it causes has to
        // reach it once it's back, or the modeline stays blank.
        let dir = temp("prompt_indicator");
        let mut app = app_in(&dir);
        app.embedded_mut().prompt = Some(EmbeddedPrompt { parent: dir.clone(), input: "Lab3".to_string() });
        assert!(app.feature_claims(|feature, app| feature.capture_key(app, KeyPress::named(FenixNamedKey::Enter))));
        assert_eq!(app.open().buffer.path(), Some(dir.join("Lab3").join("Lab3.ino").as_path()));
        let indicator = app.features.iter().find_map(|feature| feature.modeline_indicator()).map(str::to_string);
        assert!(indicator.as_deref().is_some_and(|i| i.contains("uno")), "{indicator:?}");
    }

    #[test]
    fn a_notice_missed_while_out_is_delivered_once_back() {
        let dir = temp("missed_notice");
        let mut app = app_in(&dir);
        let buffer = app.buffers.open_terminal();
        app.embedded_mut().monitors.insert(dir.clone(), Monitor { buffer, port: "COM4".to_string() });
        app.with_embedded(|host, app| {
            app.notify_features(Notice::TerminalClosed(buffer));
            assert_eq!(host.monitors.len(), 1, "not yet: the host is out");
        });
        assert!(app.embedded().monitors.is_empty());
    }
}
