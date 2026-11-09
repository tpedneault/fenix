//! The key scheme in use, and the chords (`[[keys]]`) looked up for it.

use fenix_keymap::{KeyCode, KeyPress};

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
