//! The terminal panel and pane terminals: their shells, output and size.
//!
//! Methods of `App` moved here from `app.rs` as they were.

use super::*;

impl App {
    /// `SPC o t`: toggles the terminal panel's visibility. Hiding it is
    /// purely a layout/rendering concern (`terminal_open = false`,
    /// unfocus) -- deliberately does *not* touch `self.terminal` itself,
    /// unlike `toggle_sidebar`'s own `self.sidebar = None`, since the
    /// whole point is that the shell (and anything running in it) keeps
    /// going while hidden.
    ///
    /// Opening (for the first time, or after the previous shell exited
    /// -- `Terminal::is_alive` catches both) spawns a fresh shell on a
    /// **background thread**, reported back via `FenixUserEvent::
    /// TerminalSpawned` (`apply_terminal_spawned`) -- deliberately *not*
    /// a direct, synchronous `fenix_terminal::Terminal::spawn` call on
    /// this (the main UI) thread the way an earlier version of this
    /// function did. `Terminal::spawn` itself (the PTY/process-creation
    /// call) tends to return quickly even here; the actual reported bug
    /// ("nothing shows up in the panel, and SPC is stuck until I click
    /// something") traces to what happens *after*: the previous version
    /// focused the terminal immediately, right when the (fast) spawn
    /// call returned, before the shell had produced a single byte of
    /// output -- and a freshly spawned interactive shell can take a genuinely
    /// long time to print its first prompt on this class of environment
    /// (60-90+ seconds under antivirus/EDR-style scanning of the child
    /// process, confirmed via `fenix-terminal`'s own `spawn_write_and_
    /// read_a_real_shell_round_trip` test). With nothing rendered and no
    /// indication a terminal now owns every keystroke, `SPC` (and
    /// everything else) silently vanishes into that empty shell for as
    /// long as it stays silent -- indistinguishable from the app being
    /// broken. Backgrounding the spawn is cheap insurance against a
    /// slow `Terminal::spawn` call too, but the real fix is pairing the
    /// focus-grab with immediate, honest feedback: the panel opens (and
    /// shows a "Starting shell..." placeholder -- `terminal_panel_
    /// render`) right away, and only *focuses* once `apply_terminal_
    /// spawned` confirms the shell process actually exists -- so `SPC`
    /// still worked as the leader key for the (usually brief) gap
    /// before that, and once focus does land, the placeholder makes it
    /// obvious a terminal now owns the keyboard, however long it stays
    /// quiet after that (real Vim/Neovim's own `:terminal` convention --
    /// `Ctrl-\` -- gets the user back out).
    pub(crate) fn toggle_terminal(&mut self) {
        if self.terminal_open {
            self.terminal_open = false;
            self.terminal_focused = false;
            self.wake_caret();
            return;
        }
        self.terminal_open = true;
        self.terminal_for_this_project();
        let needs_spawn = self.terminal.as_mut().is_none_or(|t| !t.session.is_alive());
        if needs_spawn {
            self.terminal_focused = false;
            self.terminal_spawning = true;
            let rows = text::TERMINAL_ROWS as u16;
            let cols = self.terminal_cols();
            let cwd = self.terminal_start_dir();
            self.terminal_root = self.project_root.clone();
            let shell = self.terminal_shell_to_spawn();
            let spawn = move || spawn_shell(rows, cols, cwd.as_deref(), shell.as_ref(), false);
            match self.event_proxy.clone() {
                Some(proxy) => {
                    std::thread::spawn(move || {
                        let result = spawn();
                        let _ = proxy.send_event(FenixUserEvent::TerminalSpawned(TerminalTarget::Panel, TerminalSpawnResult(result)));
                    });
                }
                None => {
                    // No event loop to report back through (every test)
                    // -- run synchronously and apply the result directly,
                    // same "no fake event loop needed" posture `git_
                    // refresh_session`'s own `None` branch already has.
                    let result = spawn();
                    self.apply_terminal_spawned(result);
                }
            }
        } else {
            self.terminal_focused = true;
        }
        self.wake_caret();
    }

    /// The panel, opened in another project than its shell's: that
    /// shell is kept running for when you're back there, and this
    /// project's comes back -- or, none yet, one is started here.
    pub(super) fn terminal_for_this_project(&mut self) {
        let Some(here) = self.project_root.clone() else { return };
        let key = place_key(&here);
        if self.terminal_root.as_deref().is_none_or(|root| place_key(root) == key) {
            return;
        }
        if let (Some(mut shell), Some(root)) = (self.terminal.take(), self.terminal_root.take()) {
            if shell.session.is_alive() {
                self.parked_terminals.push((self.terminal_shell, root, shell));
            }
        }
        self.parked_terminals.retain_mut(|(_, _, t)| t.session.is_alive());
        if let Some(i) = self.parked_terminals.iter().position(|(_, root, _)| place_key(root) == key) {
            let (shell, root, state) = self.parked_terminals.remove(i);
            self.terminal_shell = shell;
            self.terminal_root = Some(root);
            self.terminal = Some(state);
        }
    }

    /// Starts the thread that reads a freshly spawned shell's output.
    ///
    /// Without an event loop to forward to (every test) the output is
    /// read and thrown away rather than left in the pipe. Until
    /// Windows 11 24H2, `ClosePseudoConsole` blocks until the session's
    /// output has been drained, so a shell nobody reads from hangs
    /// whoever drops it -- here, the test, forever.
    pub(super) fn spawn_terminal_reader(&self, target: TerminalTarget, reader: Box<dyn std::io::Read + Send>) -> TerminalReader {
        match self.event_proxy.clone() {
            Some(proxy) => TerminalReader::spawn(target, reader, move |event| proxy.send_event(event).is_ok()),
            None => TerminalReader::spawn(target, reader, |_| true),
        }
    }

    /// `FenixUserEvent::TerminalSpawned` handling: wires the freshly
    /// spawned shell's reader thread and, if the panel's still open
    /// (the user could have closed it again while the spawn was in
    /// flight -- the shell keeps running regardless, same "runs on
    /// while hidden" posture `toggle_terminal`'s own doc comment
    /// already describes), focuses it now that it's actually ready. A
    /// spawn failure closes the panel back up and surfaces the error
    /// via the status-message system rather than leaving an inert,
    /// permanently-empty panel open.
    pub(super) fn apply_terminal_spawned(&mut self, result: std::io::Result<(fenix_terminal::Terminal, Box<dyn std::io::Read + Send>)>) {
        self.terminal_spawning = false;
        match result {
            Ok((session, reader)) => {
                let shell = self.parked_terminals.iter().map(|(id, _, _)| *id).chain([self.terminal_shell]).max().unwrap_or(0) + 1;
                self.terminal_shell = shell;
                let terminal_reader = self.spawn_terminal_reader(TerminalTarget::PanelShell(shell), reader);
                self.terminal = Some(TerminalState::new(session, Some(terminal_reader)));
                if self.terminal_open {
                    self.terminal_focused = true;
                }
            }
            Err(err) => {
                self.terminal_open = false;
                self.set_error(format!("couldn't start a terminal: {err}"));
            }
        }
        self.wake_caret();
    }

    /// Terminal columns that fit the current window width -- rows are
    /// always the fixed `text::TERMINAL_ROWS`, so only this needs
    /// recomputing on resize/font-size change. Falls back to a plain
    /// default when there's no real GPU/text pipeline yet (every test,
    /// and a `redraw()` before the GPU is ready) -- exact column count
    /// doesn't matter there, nothing asserts on real terminal line-
    /// wrapping in a headless test.
    pub(super) fn terminal_cols(&self) -> u16 {
        let (width, char_width) = match (&self.gpu, &self.text) {
            (Some(gpu), Some(text)) => (gpu.size.width as f32, text.char_width()),
            _ => (800.0, text::CHAR_WIDTH),
        };
        (((width - (text::PAD_LEFT + text::STRIP_INSET) * 2.0) / char_width).floor().max(1.0)) as u16
    }

    /// `FenixUserEvent::TerminalOutput` handling: feeds the chunk into
    /// the live session's `vt100` parser and requests a redraw. A no-op
    /// if the session's since been replaced/closed (a stray chunk from
    /// an old, already-superseded shell -- there's no request-id here
    /// the way `GitMainReady`/`GitRefreshReady` need one, since output
    /// only ever comes from whichever session is *currently* `self.
    /// terminal`, never a concurrently-in-flight alternate request).
    pub(super) fn apply_terminal_output(&mut self, target: TerminalTarget, bytes: Vec<u8>) {
        let state = match target {
            TerminalTarget::Panel => self.terminal.as_mut(),
            TerminalTarget::PanelShell(id) if id == self.terminal_shell => self.terminal.as_mut(),
            TerminalTarget::PanelShell(id) => self.parked_terminals.iter_mut().find(|(shell, _, _)| *shell == id).map(|(_, _, t)| t),
            TerminalTarget::Buffer(id) => self.terminal_buffers.get_mut(&id),
        };
        let Some(state) = state else { return };
        state.session.process(&bytes);
        self.wake_caret();
    }

    // -- Pane-resident terminals ---------------------------------------

    /// `SPC o T`: a shell in the focused pane, staying in this
    /// workspace -- see `TerminalTarget` for how this differs from the
    /// popup panel, and why both exist.
    ///
    /// Pressing it while looking at a terminal you already opened takes
    /// the keyboard back rather than making a second one. That is the
    /// common case by far: `SPC o T` is both "give me a shell" and "let
    /// me type in it again after `Ctrl-\`", and a key that quietly
    /// accumulated shells every time you meant the second thing would
    /// leave processes running behind panes you never look at.
    pub(crate) fn open_terminal_buffer(&mut self) {
        let cwd = self.terminal_start_dir();
        self.open_terminal_buffer_in(cwd);
    }

    /// Where a shell starts unless asked to start somewhere: the project
    /// you're in, else the focused file's folder -- not Fenix's own
    /// working folder (your home, started from the Start menu).
    pub(super) fn terminal_start_dir(&self) -> Option<PathBuf> {
        let folder_of = |id: BufferId| self.buffers.get(id).and_then(|ob| ob.buffer.path()).and_then(Path::parent).filter(|p| !p.as_os_str().is_empty()).map(Path::to_path_buf);
        // A remote project's folder is one on its machine, where its
        // terminal then is.
        let is_dir = |p: &PathBuf| if fenix_vfs::is_remote(p) { fenix_vfs::metadata(p).is_ok_and(|meta| meta.is_dir()) } else { p.is_dir() };
        self.project_root
            .clone()
            .filter(is_dir)
            .or_else(|| folder_of(self.focused_buffer_id()).filter(is_dir))
            // From Home or a page, with no project: where the file used
            // last is, rather than wherever Fenix itself was started.
            .or_else(|| self.buffers.mru().iter().filter_map(|&id| folder_of(id)).find(is_dir))
    }

    /// `open_terminal_buffer`, starting the shell somewhere in
    /// particular -- `SPC e T`'s "a shell *here*".
    pub(crate) fn open_terminal_buffer_in(&mut self, cwd: Option<PathBuf>) {
        let focused_buffer = self.focused_buffer_id();
        if self.terminal_buffers_spawning.contains(&focused_buffer) {
            self.focus_terminal_buffer(focused_buffer);
            self.wake_caret();
            return;
        }
        if let Some(state) = self.terminal_buffers.get_mut(&focused_buffer) {
            if state.session.is_alive() {
                self.focus_terminal_buffer(focused_buffer);
                self.wake_caret();
                return;
            }
            // The shell exited (`exit`, or something killed it) and its
            // last screen is still sitting there. Start a fresh one in
            // the same buffer rather than making a new one: the pane,
            // the workspace and the name are all still the ones the user
            // arranged, and only the process behind them is gone.
            self.terminal_buffers.remove(&focused_buffer);
            self.spawn_terminal_for(focused_buffer);
            self.wake_caret();
            return;
        }
        let id = self.buffers.open_terminal();
        self.terminal_buffers_opened += 1;
        self.terminal_buffer_labels.insert(id, format!("*terminal {}*", self.terminal_buffers_opened));
        if let Some(cwd) = &cwd {
            self.terminal_buffer_cwds.insert(id, cwd.clone());
        }
        self.open_buffer_in_focused_pane(id);
        self.spawn_terminal_for(id);
        self.wake_caret();
    }

    /// The shell `terminal.shell` names, as a program and its arguments;
    /// none when it's empty (or can't be read), for the platform's own
    /// (`fenix_terminal::default_shell`).
    pub(crate) fn terminal_shell(&self) -> Option<(String, Vec<String>)> {
        let line = self.config.text_value("terminal.shell")?.trim();
        let mut words = fenix_project::tools::split_command_line(line).ok()?.into_iter();
        let program = words.next().filter(|p| !p.is_empty())?;
        Some((program, words.collect()))
    }

    /// `terminal_shell` as it's started: a PowerShell it names reports its
    /// folder at each prompt, as the platform's own does
    /// (`fenix_terminal::shell_args`).
    pub(super) fn terminal_shell_to_spawn(&self) -> Option<(String, Vec<String>)> {
        self.terminal_shell().map(|(program, args)| {
            let args = fenix_terminal::shell_args(&program, args);
            (program, args)
        })
    }

    /// Starts a shell for one terminal buffer, sized to the pane
    /// currently showing it. Backgrounded for the same reason the
    /// panel's spawn is (see `toggle_terminal`): process creation is not
    /// something to block a redraw on.
    pub(super) fn spawn_terminal_for(&mut self, id: BufferId) {
        let pane = self.windows().windows().into_iter().find(|&p| self.windows().content(p) == Some(&id)).unwrap_or(self.focused_pane_id());
        let (rows, cols) = self.terminal_pane_size(pane);
        // Remembered rather than passed once, so a shell respawned after
        // `exit` comes back where it was rather than somewhere else.
        let cwd = self.terminal_buffer_cwds.get(&id).cloned();
        let named = self.terminal_buffer_programs.get(&id).cloned();
        let one_tool = named.is_some();
        let program = named.or_else(|| self.terminal_shell_to_spawn());
        let spawn = move || spawn_shell(rows, cols, cwd.as_deref(), program.as_ref(), one_tool);
        self.terminal_buffers_spawning.insert(id);
        match self.event_proxy.clone() {
            Some(proxy) => {
                std::thread::spawn(move || {
                    let result = spawn();
                    let _ = proxy.send_event(FenixUserEvent::TerminalSpawned(TerminalTarget::Buffer(id), TerminalSpawnResult(result)));
                });
            }
            None => {
                // No event loop to report back through (every test) --
                // run synchronously, same posture as `toggle_terminal`.
                let result = spawn();
                self.apply_terminal_buffer_spawned(id, result);
            }
        }
    }

    /// `FenixUserEvent::TerminalSpawned` for a pane-resident shell.
    /// Grabs the keyboard only if the buffer is still open -- the user
    /// can close a pane while its shell is still starting, and a
    /// terminal that captured keys on the way out would swallow
    /// everything typed next with nothing on screen to explain it.
    pub(super) fn apply_terminal_buffer_spawned(
        &mut self,
        id: BufferId,
        result: std::io::Result<(fenix_terminal::Terminal, Box<dyn std::io::Read + Send>)>,
    ) {
        self.terminal_buffers_spawning.remove(&id);
        if self.buffers.get(id).is_none() {
            // Closed while the spawn was in flight. `session`/`reader`
            // drop here, which kills the shell -- there is nothing left
            // that could ever show it.
            return;
        }
        match result {
            Ok((session, reader)) => {
                let terminal_reader = self.spawn_terminal_reader(TerminalTarget::Buffer(id), reader);
                self.terminal_buffers.insert(id, TerminalState::new(session, Some(terminal_reader)));
                self.focus_terminal_buffer(id);
            }
            Err(err) => {
                self.terminal_buffer_labels.remove(&id);
                self.set_error(format!("couldn't start a terminal: {err}"));
            }
        }
        self.wake_caret();
    }

    /// Hands the keyboard to one pane-resident terminal, and says so --
    /// `Ctrl-\` is the only way back out, and nothing else on screen
    /// would tell you that.
    pub(super) fn focus_terminal_buffer(&mut self, id: BufferId) {
        self.terminal_buffer_focused = Some(id);
        // A terminal owns every key while it has them, so no other
        // capturing surface may hold them at the same time.
        self.terminal_focused = false;
        self.sidebar_focused = false;
        let name = self.buffer_display_name(id);
        self.set_message(format!("{name} -- Ctrl+\\ to leave the terminal"));
    }

    /// Releases keyboard capture without closing anything -- the pane
    /// stays where it is and the shell keeps running, exactly as
    /// `Ctrl-\` does for the popup panel and for a VNC pane.
    pub(super) fn unfocus_terminal_buffer(&mut self) {
        self.terminal_buffer_focused = None;
    }

    /// The pane-resident terminal that should receive a keystroke right
    /// now -- `Some` only when one has capture, the focused pane is
    /// still showing it, and its shell is still running.
    ///
    /// The pane check matters because capture survives a pane switch
    /// (see `terminal_buffer_focused`): without it, moving to another
    /// pane and typing would go on feeding the shell you walked away
    /// from. The liveness check is what gets you *out* of a terminal
    /// whose shell you just exited -- capture is released here, on the
    /// next keystroke, so the keyboard goes back to Vim on its own
    /// rather than disappearing into a dead process until you remember
    /// the release chord.
    pub(super) fn active_terminal_buffer(&mut self) -> Option<BufferId> {
        let id = self.terminal_buffer_focused?;
        if self.windows().content(self.focused_pane_id()) != Some(&id) {
            return None;
        }
        if self.terminal_buffers.get_mut(&id).is_none_or(|state| !state.session.is_alive()) {
            self.unfocus_terminal_buffer();
            if self.terminal_buffer_programs.contains_key(&id) {
                self.set_message("it stopped (unplugged? port busy?) -- SPC m m starts the serial monitor again");
            } else {
                self.set_message("the shell exited -- SPC o T starts a new one");
            }
            return None;
        }
        Some(id)
    }

    /// Writes one keypress to a pane-resident terminal, encoded exactly
    /// as the panel encodes its own (`terminal_keys::encode`) -- a key
    /// with no terminal meaning is dropped rather than guessed at.
    pub(super) fn write_terminal_buffer_input(&mut self, id: BufferId, keypress: KeyPress) {
        let shift = self.modifiers.shift_key();
        let Some(state) = self.terminal_buffers.get_mut(&id) else { return };
        let Some(bytes) = terminal_keys::encode(keypress, shift, state.session.screen().application_cursor(), state.session.win32_input()) else { return };
        let _ = state.session.write_input(&bytes);
    }

    /// F1 to F12 while a terminal has the keyboard: its program's, unless
    /// a debug session is running, whose F5, F10 and F11 step it from
    /// anywhere. True when the key went to the terminal.
    pub(super) fn terminal_function_key(&mut self, key: &NamedKey) -> bool {
        if self.debug_session.is_some() {
            return false;
        }
        let n = match key {
            NamedKey::F1 => 1,
            NamedKey::F2 => 2,
            NamedKey::F3 => 3,
            NamedKey::F4 => 4,
            NamedKey::F5 => 5,
            NamedKey::F6 => 6,
            NamedKey::F7 => 7,
            NamedKey::F8 => 8,
            NamedKey::F9 => 9,
            NamedKey::F10 => 10,
            NamedKey::F11 => 11,
            NamedKey::F12 => 12,
            _ => return false,
        };
        let (shift, alt, ctrl) = (self.modifiers.shift_key(), self.modifiers.alt_key(), self.modifiers.control_key());
        let Some(bytes) = terminal_keys::encode_function(n, shift, alt, ctrl) else { return false };
        let session = if self.terminal_focused {
            self.terminal.as_mut().map(|t| &mut *t.session)
        } else {
            match self.active_terminal_buffer() {
                Some(id) => self.terminal_buffers.get_mut(&id).map(|t| &mut *t.session),
                None => None,
            }
        };
        match session {
            Some(session) => {
                let _ = session.write_input(&bytes);
                true
            }
            None => false,
        }
    }

    /// How many rows and columns of shell fit in `pane`, from the same
    /// geometry the render loop lays that pane out with. Falls back to a
    /// plain default with no GPU/text pipeline (every headless test),
    /// same posture as `terminal_cols`.
    pub(super) fn terminal_pane_size(&self, pane: fenix_window::WindowId) -> (u16, u16) {
        let cols = (self.pane_cols_or(pane, 80) as u16).max(1);
        let (Some(gpu), Some(text)) = (&self.gpu, &self.text) else {
            return (text::TERMINAL_ROWS as u16, cols);
        };
        let (window_width, window_height) = (gpu.size.width as f32, gpu.size.height as f32);
        let line_height = text.line_height();
        let (sidebar_px, terminal_h, modeline_top) = self.frame_metrics(window_height);
        let geometry = self.frame_geometry(window_width, sidebar_px, terminal_h, modeline_top);
        let Some((_, rect)) = geometry.panes.iter().find(|(id, _)| *id == pane) else {
            return (text::TERMINAL_ROWS as u16, cols);
        };
        // Every pane reserves its top line for a title bar, so the shell
        // gets what is left -- measured the same way `redraw` does, or
        // the last row would render underneath the pane below it.
        let content = pane_content_rect(*rect, line_height, self.pane_has_breadcrumb(pane));
        let rows = (text::lines_that_fit(content.h, line_height).max(1)) as u16;
        (rows, cols)
    }

    /// Drops one pane-resident terminal and kills its shell. Called
    /// when its buffer is closed -- the one gesture that says the shell
    /// has no owner left, since pointing a pane elsewhere deliberately
    /// leaves it running (see `TerminalState`).
    pub(super) fn close_terminal_buffer(&mut self, id: BufferId) {
        // `TerminalState::drop` kills the shell, which is also what
        // unblocks its reader thread's blocking read.
        self.terminal_buffers.remove(&id);
        self.terminal_buffers_spawning.remove(&id);
        self.terminal_buffer_labels.remove(&id);
        self.terminal_buffer_cwds.remove(&id);
        self.terminal_buffer_programs.remove(&id);
        self.tell_extensions(Happened::TerminalClosed(id));
        if self.terminal_buffer_focused == Some(id) {
            self.terminal_buffer_focused = None;
        }
    }

    /// Encodes `keypress` and writes it to the live terminal session, if
    /// any (a no-op if the session's since exited or never started --
    /// shouldn't happen while `terminal_focused`, but no reason to panic
    /// if it does). Errors (the shell's pipe closed, e.g. it just
    /// exited) are swallowed -- the next `SPC o t` respawns.
    pub(super) fn write_terminal_input(&mut self, keypress: KeyPress) {
        let shift = self.modifiers.shift_key();
        let Some(state) = self.terminal.as_mut() else { return };
        let Some(bytes) = terminal_keys::encode(keypress, shift, state.session.screen().application_cursor(), state.session.win32_input()) else { return };
        let _ = state.session.write_input(&bytes);
    }

    /// Recomputes terminal columns for the current window width/font
    /// size and pushes it to the live session (if any) -- rows never
    /// change (`text::TERMINAL_ROWS` is fixed), so this is the one
    /// dimension that ever needs resyncing after a window resize or a
    /// font-size change.
    pub(super) fn resync_terminal_cols(&mut self) {
        if self.terminal.is_none() {
            return;
        }
        let cols = self.terminal_cols();
        if let Some(state) = self.terminal.as_mut() {
            if let Err(err) = state.session.resize(text::TERMINAL_ROWS as u16, cols) {
                eprintln!("fenix: couldn't resize the terminal: {err}");
            }
        }
    }

    /// The modeline's mode badge: "CAPTURE" while a surface has the
    /// keyboard (a remote desktop: `WasmState::captured`), overriding the
    /// vim mode label -- there's no Normal/Insert/Visual distinction worth
    /// showing while every key goes straight to it instead of vim's own
    /// dispatch.
    /// Whether a terminal has the keyboard: the panel (`SPC o t`), or the
    /// one in the focused pane. `Ctrl-\` gives it back.
    pub(super) fn terminal_has_keyboard(&self) -> bool {
        self.terminal_focused || self.terminal_buffer_focused.is_some_and(|id| self.windows().content(self.focused_pane_id()) == Some(&id))
    }


    /// Builds the terminal panel's visible rows as rich-text spans (one
    /// coalesced foreground-color run per contiguous same-color stretch,
    /// same "not literally cell-per-cell" shape every pane's spans
    /// already use) plus a separate list of
    /// background segments for any cell whose background isn't the
    /// theme's own default -- `RowSpans` alone has no notion of a per-
    /// span background, so a non-default one is painted as its own
    /// backdrop rect first, the exact same mechanism `hl_line`/selection
    /// already use (see `App::redraw`'s own `row_y`-keyed segment loop).
    /// What the terminal panel should render this frame -- `None` while
    /// it's closed, the live shell's own screen content once `self.
    /// terminal` exists, or (the case that motivated pulling this out
    /// of `redraw` into its own directly-testable method) a "starting
    /// shell..." placeholder for the window between `toggle_terminal`
    /// kicking off a background spawn and `apply_terminal_spawned`
    /// hearing back -- see `toggle_terminal`'s own doc comment for why
    /// that gap can be long enough to matter, particularly on Windows.
    pub(super) fn terminal_panel_render(&self) -> Option<terminal_view::TermView> {
        if !self.terminal_open {
            return None;
        }
        if let Some(t) = &self.terminal {
            return Some(self.terminal_row_spans(t.session.screen()));
        }
        if self.terminal_spawning {
            // The "Ctrl-\ to unfocus" reminder rides along here since
            // this placeholder is the first thing shown right as the
            // panel grabs focus -- there's no other UI surface that
            // mentions the chord at all, and this is exactly the moment
            // a user unfamiliar with it needs to see it.
            return Some(terminal_view::TermView { spans: vec![("Starting shell... (Ctrl+\\ to unfocus)".to_string(), self.theme.fg, false)], ..Default::default() });
        }
        None
    }

    /// A terminal's screen as it's drawn: the theme's ANSI colours, and
    /// bold, dim, italic, underline and inverse (`terminal_view`).
    pub(super) fn terminal_row_spans(&self, screen: &vt100::Screen) -> terminal_view::TermView {
        terminal_view::view(self.theme, screen)
    }
}
