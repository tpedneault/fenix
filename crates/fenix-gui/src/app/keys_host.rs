//! The key scheme in use, and the chords (`[[keys]]`) looked up for it.

use fenix_keymap::{KeyCode, KeyPress};

use fenix_buffers::BufferKind;

use super::App;
use crate::keymap::{self, ChordContext, Scheme};

impl App {
    /// The key scheme in use: `editor.keys`.
    pub(crate) fn key_scheme(&self) -> Scheme {
        match self.config.text_value("editor.keys") {
            Some("standard") => Scheme::Standard,
            _ => Scheme::Vim,
        }
    }

    /// Whether standard keys are in use.
    pub(crate) fn standard_keys(&self) -> bool {
        self.key_scheme() == Scheme::Standard
    }

    /// What's in front of you, for a chord's `when`.
    pub(crate) fn chord_context(&self) -> ChordContext {
        let page = self.pages.contains_key(&self.focused_buffer_id());
        let in_editor = self.main_view == super::MainView::Editor && !self.sidebar_focused;
        let text = in_editor && !page;
        ChordContext {
            scheme: self.key_scheme(),
            terminal: self.terminal_focused || self.terminal_buffer_focused.is_some_and(|id| self.windows().content(self.focused_pane_id()) == Some(&id)),
            text,
            typing: text && self.vim.is_inserting(),
            page: in_editor && page,
        }
    }

    /// The command a chord runs here, if any. Only a key with a modifier,
    /// a named key or an F-key is looked up: a plain character is text,
    /// or Vim's.
    pub(crate) fn chord_command(&self, key: KeyPress) -> Option<&'static str> {
        if matches!(key.code, KeyCode::Char(_)) && !(key.mods.ctrl || key.mods.alt || key.mods.super_) {
            return None;
        }
        keymap::chord_map().lookup(key, &self.chord_context())
    }
}

impl App {
    /// `fenix-vim`'s policy kept to `editor.keys`. Going to standard keys
    /// puts a buffer you type in into Insert at once; going back to Vim's,
    /// into Normal.
    pub(crate) fn sync_key_policy(&mut self) {
        let want = if self.standard_keys() { fenix_vim::EditPolicy::Standard } else { fenix_vim::EditPolicy::Vim };
        if self.vim.policy() == want {
            return;
        }
        self.vim.set_policy(want);
        if want == fenix_vim::EditPolicy::Vim && self.vim.is_inserting() {
            self.leave_insert_in_place();
        }
        self.settle_keys();
    }

    /// Whether the focused buffer is one you type in: a file's text (or a
    /// table, or names being edited), not a page, a listing, a diff or
    /// anything Fenix writes itself.
    pub(crate) fn focused_takes_typing(&self) -> bool {
        let id = self.focused_buffer_id();
        if self.pages.contains_key(&id) || self.terminal_buffers.contains_key(&id) {
            return false;
        }
        let Some(ob) = self.buffers.get(id) else { return false };
        let editable_kind = matches!(ob.kind, BufferKind::Text | BufferKind::Table) || (ob.kind == BufferKind::Explorer && self.rename_mode.contains_key(&id));
        editable_kind && !self.buffer_edits_are_reverted(id, ob.kind)
    }

    /// Standard keys rest in Insert in a buffer you type in, and browse
    /// (Normal underneath, its own keys working) one you don't: the mode
    /// put right after a key, a click or a command moved you.
    pub(crate) fn settle_keys(&mut self) {
        if self.vim.policy() != fenix_vim::EditPolicy::Standard {
            return;
        }
        let in_editor = self.main_view == super::MainView::Editor && !self.sidebar_focused && !self.terminal_focused;
        if !in_editor {
            return;
        }
        if self.focused_takes_typing() {
            if self.vim.mode() == fenix_vim::Mode::Normal && self.vim.is_idle() {
                let pane = self.focused_pane_id();
                let id = self.focused_buffer_id();
                if let (Some(ob), Some(state)) = (self.buffers.get(id), self.workspaces.active_pane_states_mut().get_mut(&pane)) {
                    self.vim.enter_insert_here(&ob.buffer, &mut state.cursor);
                }
            }
        } else if self.vim.is_inserting() {
            let id = self.focused_buffer_id();
            if let Some(ob) = self.buffers.get_mut(id) {
                self.vim.leave_insert_here(&mut ob.buffer);
            }
        }
    }

    /// Whether `key` opens the leader's menu here: `SPC` in Vim's Normal
    /// and Visual; with standard keys, the leader chord anywhere a key
    /// reaches it, and `SPC` where you're browsing.
    pub(crate) fn leader_starts(&self, key: KeyPress) -> bool {
        let mode = self.vim.mode();
        if self.standard_keys() {
            return key.chord() == self.leader_chord().chord() || (mode == fenix_vim::Mode::Normal && key == KeyPress::char(' '));
        }
        matches!(mode, fenix_vim::Mode::Normal | fenix_vim::Mode::Visual) && key == KeyPress::char(' ')
    }

    /// Standard keys: a pause in typing longer than `editor.undo_pause_ms`
    /// starts a new undo step.
    pub(crate) fn note_typing_pause(&mut self, now: std::time::Instant) {
        let last = self.last_key_at.replace(now);
        if !self.standard_keys() || !self.vim.is_inserting() {
            return;
        }
        let pause = self.config.int_value("editor.undo_pause_ms").unwrap_or(1000);
        if pause <= 0 {
            return;
        }
        if last.is_some_and(|at| now.duration_since(at).as_millis() as i64 >= pause) {
            let id = self.focused_buffer_id();
            if let Some(ob) = self.buffers.get_mut(id) {
                ob.buffer.break_undo_run();
            }
        }
    }

    /// The modeline's badge with standard keys: none while typing or
    /// browsing, `SELECT` with a selection.
    pub(crate) fn standard_badge(&self) -> Option<&'static str> {
        if !self.standard_keys() {
            return None;
        }
        Some(match self.vim.mode() {
            fenix_vim::Mode::Visual => "SELECT",
            fenix_vim::Mode::Command | fenix_vim::Mode::Search => return None,
            _ => "",
        })
    }
}

impl App {
    /// The key scheme was switched: what reads it hears (the extensions,
    /// with API 1.5).
    pub(crate) fn key_scheme_changed(&mut self) {}
}
