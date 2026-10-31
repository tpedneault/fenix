//! The dock: the agents' column at the right of the window, the thread's
//! page above and its prompt under it. There's one. It's beside every
//! pane rather than beside the one you were in, so opening it never
//! splits your code; its two panes have titles instead of tab strips;
//! and it's found again, not built again, after a restart.

use super::*;
use fenix_window::WindowId;

/// How much of the dock's height the thread gets; the prompt has the rest.
const THREAD_SHARE: f32 = 0.78;

impl App {
    /// Every pane showing `buffer`, in the window tree's order.
    pub(super) fn agents_panes_showing(&self, buffer: BufferId) -> Vec<WindowId> {
        let windows = self.windows();
        windows.windows().into_iter().filter(|w| windows.content(*w) == Some(&buffer)).collect()
    }

    /// The dock's panes: the thread's, the prompt's.
    pub(super) fn agents_dock_panes(&self) -> (Option<WindowId>, Option<WindowId>) {
        let page = self.agents.page.and_then(|p| self.agents_panes_showing(p).first().copied());
        let composer = self.agents.composer.and_then(|c| self.agents_panes_showing(c).first().copied());
        (page, composer)
    }

    /// Whether `pane` is one of the dock's.
    pub(super) fn agents_is_dock_pane(&self, pane: WindowId) -> bool {
        let (page, composer) = self.agents_dock_panes();
        Some(pane) == page || Some(pane) == composer
    }

    /// A new pane's own state: at the start, with `buffer` its one tab.
    fn agents_seed_pane(&mut self, pane: WindowId, buffer: BufferId) {
        self.workspaces.active_pane_states_mut().insert(pane, PaneState::seeded_at(Cursor::at_start()));
        self.workspaces.active_pane_tabs_mut().insert(pane, vec![buffer]);
    }

    /// Closes `pane` and forgets what was kept for it.
    pub(super) fn agents_close_pane(&mut self, pane: WindowId) {
        self.pane_titles.remove(&pane);
        if !self.windows_mut().focus(pane) || !self.windows_mut().close_focused() {
            return;
        }
        self.workspaces.active_pane_states_mut().remove(&pane);
        self.workspaces.active_scroll_anims_mut().remove(&pane);
        self.workspaces.active_pane_tabs_mut().remove(&pane);
    }

    /// Puts the dock up showing `page`, or finds it: the thread above,
    /// the prompt under it, at the right of the window. A second pane
    /// showing either (an older session's) is closed. Returns the
    /// thread's pane; the focus is left on it.
    pub(super) fn agents_dock_ensure(&mut self, page: BufferId) -> WindowId {
        let from = self.focused_pane_id();
        let composer = match self.agents.composer.filter(|c| self.buffers.get(*c).is_some()) {
            Some(c) => c,
            None => self.buffers.open_prompt(),
        };
        self.agents.composer = Some(composer);
        for buffer in [page, composer] {
            for extra in self.agents_panes_showing(buffer).into_iter().skip(1) {
                self.agents_close_pane(extra);
            }
        }
        let mut page_pane = self.agents_panes_showing(page).first().copied();
        let mut composer_pane = self.agents_panes_showing(composer).first().copied();
        if page_pane.is_none() {
            let pane = match composer_pane {
                // The prompt is up on its own: the thread goes above it.
                Some(c) => {
                    self.windows_mut().focus(c);
                    self.windows_mut().split_before(SplitKind::Horizontal, page)
                }
                None => {
                    let share = self.config.int_value("agents.panel_width").unwrap_or(38).clamp(20, 80) as f32 / 100.0;
                    self.windows_mut().split_root(SplitKind::Vertical, page, 1.0 - share)
                }
            };
            self.agents_seed_pane(pane, page);
            page_pane = Some(pane);
        }
        let page_pane = page_pane.expect("the thread's pane was just found or made");
        if composer_pane.is_none() {
            self.windows_mut().focus(page_pane);
            let pane = self.windows_mut().split(SplitKind::Horizontal, composer);
            self.agents_seed_pane(pane, composer);
            composer_pane = Some(pane);
        }
        self.windows_mut().set_split_near(page_pane, THREAD_SHARE);
        // Files the thread opens go to the pane you came from, unless that
        // was the dock itself.
        if from != page_pane && Some(from) != composer_pane && self.windows().windows().contains(&from) {
            self.agents.code_pane = Some(from);
        }
        self.windows_mut().focus(page_pane);
        self.agents_dock_titles();
        page_pane
    }

    /// The dock's two title rows, from the shown thread: who it is and
    /// how it's doing above, where the prompt goes and how to send it
    /// below.
    pub(super) fn agents_dock_titles(&mut self) {
        let (page_pane, composer_pane) = self.agents_dock_panes();
        let thread = self.agents.shown(&self.pages);
        let view = thread.as_ref().and_then(|t| self.agents.hub.view(t));
        let title = view.as_ref().map(|v| v.thread.display_title()).unwrap_or_else(|| "New thread".into());
        let agent = thread.as_ref().map(|t| self.agents.status_of(t).agent).unwrap_or_else(|| "the agent".into());
        let mode = view
            .as_ref()
            .and_then(|v| v.thread.modes.as_ref())
            .and_then(|m| m.available.iter().find(|a| a.id == m.current).map(|a| a.name.clone()));
        let waiting = view.as_ref().is_some_and(|v| v.thread.pending_permissions() > 0);
        // A pane that was the dock's and shows something else now is an
        // ordinary pane again, tabs and all.
        for pane in std::mem::take(&mut self.agents.titled) {
            if Some(pane) != page_pane && Some(pane) != composer_pane {
                self.pane_titles.remove(&pane);
            }
        }
        self.agents.titled = page_pane.into_iter().chain(composer_pane).collect();
        if let Some(pane) = page_pane {
            self.pane_titles.insert(pane, format!("Agent · {title}"));
        }
        if let Some(pane) = composer_pane {
            let to = match &mode {
                Some(mode) => format!("to {agent} · {mode}"),
                None => format!("to {agent}"),
            };
            let send = if waiting { "waiting on you: y or n answers, here or in the thread" } else { "Ctrl-Enter sends (Enter in Normal), ↑ the last one" };
            self.pane_titles.insert(pane, format!("{to}   ·   {send}"));
        }
    }

    /// Before a frame is drawn: a pane that left the dock (a file opened
    /// in it) loses the dock's title, cheaply -- no titles are made here.
    pub(crate) fn agents_dock_sync(&mut self) {
        if self.agents.titled.is_empty() {
            return;
        }
        // The prompt left while writing in it (a click on the thread, on
        // a card that opens a file, on your code): Normal mode where you
        // land, so the next keys aren't typed into a file or a page by
        // surprise -- the prompt is where SPC a a puts you in Insert.
        let focused = self.focused_pane_id();
        let in_prompt = self.agents_dock_panes().1 == Some(focused);
        let was_in_prompt = std::mem::replace(&mut self.agents.in_prompt, in_prompt);
        if was_in_prompt && !in_prompt && self.vim.mode() == Mode::Insert {
            let id = self.focused_buffer_id();
            if let (Some(ob), Some(state)) = (self.buffers.get_mut(id), self.workspaces.active_pane_states_mut().get_mut(&focused)) {
                self.vim.handle_key(&mut ob.buffer, &mut state.cursor, KeyPress::named(FenixNamedKey::Escape));
            }
        }
        // And come into however -- a click, SPC w, a session put back
        // with the prompt focused -- it's to write in: Insert, at the end,
        // so the first letters aren't taken for Vim's commands.
        if !was_in_prompt && in_prompt && self.vim.is_idle() {
            self.agents_focus_composer();
        }
        let (page, composer) = (self.agents.page, self.agents.composer);
        let windows = self.windows();
        let left: Vec<WindowId> = self
            .agents
            .titled
            .iter()
            .copied()
            .filter(|p| {
                let shown = windows.content(*p).copied();
                shown.is_none() || (shown != page && shown != composer)
            })
            .collect();
        for pane in left {
            self.pane_titles.remove(&pane);
            self.agents.titled.retain(|p| *p != pane);
        }
    }

    /// After a restart: the dock the session put back is taken on as
    /// the dock -- titles, and a thread above a prompt that came back on
    /// its own -- rather than left as two ordinary panes.
    pub(crate) fn agents_adopt_dock(&mut self) {
        let (page_pane, composer_pane) = self.agents_dock_panes();
        let page = self.agents.page.filter(|p| self.pages.contains_key(p));
        match (page, page_pane, composer_pane) {
            (Some(page), _, Some(_)) | (Some(page), Some(_), None) => {
                let focused = self.focused_pane_id();
                // Built again in its place -- the full height, at the right
                // -- whatever shape an older session left it in.
                for buffer in [Some(page), self.agents.composer].into_iter().flatten() {
                    for pane in self.agents_panes_showing(buffer) {
                        if self.windows().window_count() > 1 {
                            self.agents_close_pane(pane);
                        }
                    }
                }
                self.agents_dock_ensure(page);
                // Back where you were -- or, that pane gone (a second
                // prompt), the code: a file opened next (a file named on
                // the command line) mustn't land in the dock.
                if self.windows().windows().contains(&focused) && !self.agents_is_dock_pane(focused) {
                    self.windows_mut().focus(focused);
                } else {
                    self.agents_focus_code();
                }
            }
            // A prompt with no thread to go with: nothing to send it to.
            (None, _, Some(pane)) => self.agents_close_pane(pane),
            _ => {}
        }
    }

    /// `SPC a A`: the dock shown or put away, wherever the focus is --
    /// `SPC a a` from the code goes to the prompt instead.
    pub(crate) fn agents_toggle_dock(&mut self) {
        if let (Some(_), Some(_)) = self.agents_dock_panes() {
            self.agents_close_panel();
        } else {
            self.agents_open_panel();
        }
    }

    /// Focus to the code: the pane the dock was opened from, else the
    /// first pane that isn't the dock's. For what the agents open (files,
    /// their changes, the log), which never goes in the dock.
    pub(super) fn agents_focus_code(&mut self) {
        let panes = self.windows().windows();
        let pane = self
            .agents
            .code_pane
            .filter(|p| panes.contains(p) && !self.agents_is_dock_pane(*p))
            .or_else(|| panes.iter().copied().find(|p| !self.agents_is_dock_pane(*p)));
        if let Some(pane) = pane {
            self.windows_mut().focus(pane);
            self.agents.code_pane = Some(pane);
        }
    }

    /// The prompt, focused to write in: Insert mode, at the end.
    pub(super) fn agents_focus_composer(&mut self) {
        let (_, Some(pane)) = self.agents_dock_panes() else { return };
        self.windows_mut().focus(pane);
        let cursor = self.cursor();
        self.vim.exit_visual_mode(&cursor);
        if self.vim.mode() != Mode::Insert {
            let id = self.focused_buffer_id();
            if let (Some(ob), Some(state)) = (self.buffers.get_mut(id), self.workspaces.active_pane_states_mut().get_mut(&pane)) {
                state.cursor.char_idx = ob.buffer.len_chars();
                self.vim.handle_key(&mut ob.buffer, &mut state.cursor, KeyPress::char('A'));
            }
        }
        self.agents_dock_titles();
        self.wake_caret();
    }
}
