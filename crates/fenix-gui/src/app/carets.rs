//! More cursors, with standard keys: Alt+click adds one (or takes it
//! away), Alt+Shift+drag makes a column of them, Escape goes back to one.
//! A pane's carets are its cursor -- the primary, which everything that
//! knows one cursor reads -- and the extras here. Typing, deleting,
//! Enter, Tab, pasting and moving happen at every caret, the last in the
//! buffer first, as one undo step. Any other key or command, or an edit
//! made elsewhere (a language server's, an agent's, a reload), leaves the
//! primary alone.

use std::collections::HashMap;

use fenix_core::Cursor;
use fenix_keymap::{KeyCode, KeyPress, NamedKey};
use fenix_vim::{Mode, VisualKind};

use super::{App, BufferId};

/// The most carets a column drag makes.
pub(crate) const MAX_CARETS: usize = 10_000;

/// One extra caret: where it is, the column it keeps going up and down,
/// and where its selection starts when it has one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Caret {
    pub(crate) at: usize,
    pub(crate) sticky: usize,
    pub(crate) anchor: Option<usize>,
}

/// A pane's extra carets, in the buffer they were made in, as of an edit.
#[derive(Debug, Clone, Default)]
pub(crate) struct Carets {
    pub(crate) buffer: Option<BufferId>,
    pub(crate) extra: Vec<Caret>,
    /// The buffer's edit count when the carets were last right: an edit
    /// since, made elsewhere, drops them.
    pub(crate) edits: u64,
}

pub(crate) type PaneCarets = HashMap<fenix_window::WindowId, Carets>;

impl App {
    /// The focused pane's extra carets, if they still hold.
    pub(crate) fn extra_carets(&self) -> &[Caret] {
        let pane = self.focused_pane_id();
        let id = self.focused_buffer_id();
        let Some(carets) = self.carets.get(&pane) else { return &[] };
        let edits = self.buffers.get(id).map(|ob| ob.buffer.edit_count() as u64);
        if carets.buffer != Some(id) || edits != Some(carets.edits) {
            return &[];
        }
        &carets.extra
    }

    /// Back to the one cursor.
    pub(crate) fn clear_carets(&mut self) {
        let pane = self.focused_pane_id();
        self.carets.remove(&pane);
    }

    /// `extra` as the focused pane's extra carets, now.
    fn set_extra_carets(&mut self, mut extra: Vec<Caret>) {
        let pane = self.focused_pane_id();
        let id = self.focused_buffer_id();
        let primary = self.cursor().char_idx;
        extra.retain(|c| c.at != primary);
        extra.sort_by_key(|c| c.at);
        extra.dedup_by_key(|c| c.at);
        if extra.is_empty() {
            self.carets.remove(&pane);
            return;
        }
        let edits = self.buffers.get(id).map_or(0, |ob| ob.buffer.edit_count() as u64);
        self.carets.insert(pane, Carets { buffer: Some(id), extra, edits });
    }

    /// Alt+click (or Ctrl+click, `editor.multi_cursor_modifier`): a caret at
    /// `at`, or, on one, that one taken away.
    pub(crate) fn toggle_caret_at(&mut self, at: usize) {
        let mut extra = self.extra_carets().to_vec();
        let primary = self.cursor().char_idx;
        if let Some(i) = extra.iter().position(|c| c.at == at) {
            extra.remove(i);
        } else if at == primary {
            // The primary itself: the nearest extra takes its place.
            if let Some(i) = extra.iter().enumerate().min_by_key(|(_, c)| c.at.abs_diff(at)).map(|(i, _)| i) {
                let next = extra.remove(i);
                let pane = self.focused_pane_id();
                self.put_cursor(pane, next.at);
            }
        } else {
            let sticky = self.col_of(at);
            extra.push(Caret { at, sticky, anchor: None });
        }
        self.set_extra_carets(extra);
        self.say_carets();
    }

    /// Alt+Shift+drag: a caret on every line from `from_line` to the
    /// pointer's, at `col` (or the line's end, when it's shorter); the
    /// primary on the pointer's line.
    pub(crate) fn column_carets(&mut self, from_line: usize, to_line: usize, col: usize) {
        let (lo, hi) = (from_line.min(to_line), from_line.max(to_line));
        let lines = hi - lo + 1;
        if lines > MAX_CARETS {
            self.set_error(format!("{lines} lines is more cursors than {MAX_CARETS} -- a shorter column"));
            return;
        }
        let at_col = |app: &App, line: usize| {
            let buffer = &app.open().buffer;
            buffer.line_start_char(line) + col.min(buffer.line_len(line))
        };
        let pane = self.focused_pane_id();
        let primary = at_col(self, to_line);
        self.put_cursor(pane, primary);
        if let Some(state) = self.workspaces.active_pane_states_mut().get_mut(&pane) {
            state.cursor.sticky_col = col;
        }
        let extra: Vec<Caret> = (lo..=hi).filter(|&l| l != to_line).map(|l| Caret { at: at_col(self, l), sticky: col, anchor: None }).collect();
        self.set_extra_carets(extra);
        self.say_carets();
    }

    /// The modeline says how many there are.
    fn say_carets(&mut self) {
        let n = self.extra_carets().len();
        if n > 0 {
            self.set_message(format!("{} cursors -- Esc goes back to one", n + 1));
        }
    }

    fn col_of(&self, at: usize) -> usize {
        let c = Cursor { char_idx: at, sticky_col: 0 };
        self.open().buffer.line_col(&c).1
    }

    /// A key, at every caret, when there's more than one: `true` when it's
    /// been done. Escape, and a key that isn't typing or moving, go back to
    /// one caret and are left to what comes next.
    pub(crate) fn multi_caret_key(&mut self, key: KeyPress) -> bool {
        if self.extra_carets().is_empty() {
            if self.carets.contains_key(&self.focused_pane_id()) {
                self.clear_carets();
            }
            return false;
        }
        if self.vim.policy() != fenix_vim::EditPolicy::Standard || !self.focused_takes_typing() || !takes_every_caret(key) {
            self.clear_carets();
            return false;
        }
        self.at_every_caret(|app, buffer_key| app.vim_key_here(buffer_key), key);
        true
    }

    /// `edit.paste` with more than one caret: the clipboard at each.
    pub(crate) fn paste_at_every_caret(&mut self, text: &str) -> bool {
        if self.extra_carets().is_empty() {
            return false;
        }
        let pieces: Vec<&str> = text.trim_end_matches('\n').split('\n').collect();
        let carets = self.extra_carets().len() + 1;
        let one_each = pieces.len() == carets;
        let mut i = carets;
        self.at_every_caret(
            |app, _| {
                // The last caret first: the last piece first.
                i -= 1;
                let piece = if one_each { pieces[i] } else { text };
                let selected = app.vim.visual_selection_range(&app.open().buffer, &app.cursor()).map(|(r, _)| r);
                if let Some(range) = selected {
                    let cursor = app.cursor();
                    app.vim.exit_visual_mode(&cursor);
                    let (buffer, cursor) = app.focused_buffer_and_cursor_mut();
                    buffer.delete_range(cursor, range.start, range.end);
                    cursor.char_idx = range.start;
                    app.enter_insert_in_place();
                }
                let (buffer, cursor) = app.focused_buffer_and_cursor_mut();
                buffer.insert_str(cursor, piece);
            },
            KeyPress::char('v').with_ctrl(),
        );
        true
    }

    /// `edit.copy` with more than one caret: each caret's selection (or
    /// line), one to a line.
    pub(crate) fn copy_every_caret(&mut self) -> Option<String> {
        if self.extra_carets().is_empty() {
            return None;
        }
        let mut carets: Vec<Caret> = self.extra_carets().to_vec();
        let primary_anchor = (self.vim.mode() == Mode::Visual).then(|| self.vim.visual_anchor());
        carets.push(Caret { at: self.cursor().char_idx, sticky: 0, anchor: primary_anchor });
        carets.sort_by_key(|c| c.at);
        let buffer = &self.open().buffer;
        let pieces: Vec<String> = carets
            .iter()
            .map(|c| match c.anchor {
                Some(a) => buffer.rope().slice(a.min(c.at)..a.max(c.at)).to_string(),
                None => {
                    let line = buffer.rope().char_to_line(c.at.min(buffer.len_chars()));
                    buffer.rope().line(line).to_string().trim_end_matches('\n').to_string()
                }
            })
            .collect();
        Some(pieces.join("\n"))
    }

    /// The key Vim takes, at the focused pane's cursor as it is now.
    fn vim_key_here(&mut self, key: KeyPress) {
        let (id, pane) = (self.focused_buffer_id(), self.focused_pane_id());
        if let (Some(ob), Some(state)) = (self.buffers.get_mut(id), self.workspaces.active_pane_states_mut().get_mut(&pane)) {
            self.vim.handle_key(&mut ob.buffer, &mut state.cursor, key);
        }
    }

    /// `act` done at every caret, the last in the buffer first, each as if
    /// it were the cursor (with its selection, when it has one), the
    /// others moved by what it changed: one undo step.
    fn at_every_caret(&mut self, mut act: impl FnMut(&mut App, KeyPress), key: KeyPress) {
        let pane = self.focused_pane_id();
        let primary_cursor = self.cursor();
        let primary_anchor = (self.vim.mode() == Mode::Visual).then(|| self.vim.visual_anchor());
        // (caret, is the primary)
        let mut carets: Vec<(Caret, bool)> = self.extra_carets().iter().map(|c| (*c, false)).collect();
        carets.push((Caret { at: primary_cursor.char_idx, sticky: primary_cursor.sticky_col, anchor: primary_anchor }, true));
        carets.sort_by_key(|(c, _)| std::cmp::Reverse(c.at));
        self.focused_buffer_and_cursor_mut().0.begin_undo_group();
        for i in 0..carets.len() {
            let (caret, _) = carets[i];
            let len_before = self.open().buffer.len_chars();
            // The caret as the cursor, in its mode.
            if let Some(state) = self.workspaces.active_pane_states_mut().get_mut(&pane) {
                state.cursor = Cursor { char_idx: caret.at.min(len_before), sticky_col: caret.sticky };
            }
            if self.vim.mode() == Mode::Visual {
                let cursor = self.cursor();
                self.vim.exit_visual_mode(&cursor);
            }
            match caret.anchor {
                Some(anchor) => {
                    if self.vim.is_inserting() {
                        self.leave_insert_in_place();
                    }
                    self.vim.select(VisualKind::Char, anchor.min(len_before));
                }
                None => self.enter_insert_in_place(),
            }
            act(self, key);
            let len_after = self.open().buffer.len_chars();
            let cursor = self.cursor();
            let anchor = (self.vim.mode() == Mode::Visual).then(|| self.vim.visual_anchor());
            let delta = len_after as isize - len_before as isize;
            // Those done already are later in the buffer: moved by what
            // this one added or took away.
            let edit_at = caret.at.min(cursor.char_idx).min(anchor.unwrap_or(usize::MAX));
            for (done, _) in carets.iter_mut().take(i) {
                if done.at >= edit_at {
                    done.at = (done.at as isize + delta).max(0) as usize;
                    done.anchor = done.anchor.map(|a| (a as isize + delta).max(0) as usize);
                }
            }
            carets[i].0 = Caret { at: cursor.char_idx, sticky: cursor.sticky_col, anchor };
        }
        {
            let buffer = self.focused_buffer_and_cursor_mut().0;
            buffer.break_undo_run();
            buffer.end_undo_group();
        }
        // The primary back as the cursor, in its mode.
        let primary = carets.iter().find(|(_, p)| *p).map(|(c, _)| *c).unwrap_or(Caret { at: 0, sticky: 0, anchor: None });
        if let Some(state) = self.workspaces.active_pane_states_mut().get_mut(&pane) {
            state.cursor = Cursor { char_idx: primary.at, sticky_col: primary.sticky };
        }
        if self.vim.mode() == Mode::Visual {
            let cursor = self.cursor();
            self.vim.exit_visual_mode(&cursor);
        }
        match primary.anchor {
            Some(anchor) => {
                if self.vim.is_inserting() {
                    self.leave_insert_in_place();
                }
                self.vim.select(VisualKind::Char, anchor);
            }
            None => self.enter_insert_in_place(),
        }
        // Carets that met are one.
        let mut extra: Vec<Caret> = carets.into_iter().filter(|(_, p)| !*p).map(|(c, _)| c).collect();
        extra.retain(|c| c.at != primary.at);
        self.set_extra_carets(extra);
    }
}

/// Whether a key happens at every caret: typing, deleting, Enter, Tab and
/// moving.
fn takes_every_caret(key: KeyPress) -> bool {
    let m = key.mods;
    if m.alt || m.super_ {
        return false;
    }
    match key.code {
        KeyCode::Char(_) => !m.ctrl,
        KeyCode::Named(NamedKey::Enter | NamedKey::Tab) => !m.ctrl,
        KeyCode::Named(NamedKey::Backspace | NamedKey::Delete) => true,
        KeyCode::Named(NamedKey::Left | NamedKey::Right | NamedKey::Up | NamedKey::Down | NamedKey::Home | NamedKey::End) => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_keys::app_on_file;
    use super::*;

    fn standard(app: &mut App) {
        app.config.set_value("editor.keys", Some(fenix_config::Value::Text("standard".into())));
        app.sync_key_policy();
    }

    #[test]
    fn typing_happens_at_every_caret_and_one_undo_takes_it_back() {
        let (mut app, _f) = app_on_file("a.txt", "a\nb\nc\n");
        standard(&mut app);
        app.toggle_caret_at(2);
        app.toggle_caret_at(4);
        assert_eq!(app.extra_carets().len(), 2);
        app.type_text("x");
        assert_eq!(app.text(), "xa\nxb\nxc\n");
        app.press("Backspace");
        assert_eq!(app.text(), "a\nb\nc\n", "Backspace at each");
        app.type_text("yz");
        assert_eq!(app.text(), "yza\nyzb\nyzc\n");
        app.press("C-z");
        assert_eq!(app.text(), "ya\nyb\nyc\n", "one keystroke, at every caret");
        assert!(app.extra_carets().is_empty(), "undo is another edit: back to one");
    }

    #[test]
    fn enter_and_moving_happen_at_every_caret_and_carets_that_meet_are_one() {
        let (mut app, _f) = app_on_file("a.txt", "ab\ncd\n");
        standard(&mut app);
        app.toggle_caret_at(4);
        app.press("End");
        app.type_text("!");
        assert_eq!(app.text(), "ab!\ncd!\n");
        app.press("Enter");
        assert_eq!(app.text(), "ab!\n\ncd!\n\n");
        app.press("C-Home");
        assert!(app.extra_carets().is_empty(), "both at the start: one");
    }

    #[test]
    fn escape_goes_back_to_one_and_a_click_on_a_caret_takes_it_away() {
        let (mut app, _f) = app_on_file("a.txt", "a\nb\n");
        standard(&mut app);
        app.toggle_caret_at(2);
        assert_eq!(app.extra_carets().len(), 1);
        app.toggle_caret_at(2);
        assert!(app.extra_carets().is_empty());
        app.toggle_caret_at(2);
        app.press("Esc");
        assert!(app.extra_carets().is_empty());
        assert_eq!(app.vim.mode(), Mode::Insert);
    }

    #[test]
    fn a_column_of_carets_types_on_every_line_and_shift_selects_at_each() {
        let (mut app, _f) = app_on_file("a.txt", "one\ntwo\nthree\nfour\n");
        standard(&mut app);
        app.column_carets(0, 2, 1);
        assert_eq!(app.extra_carets().len(), 2);
        app.press("S-Right");
        app.type_text("_");
        assert_eq!(app.text(), "o_e\nt_o\nt_ree\nfour\n");
        let lines = app.open().buffer.line_count() + MAX_CARETS;
        app.column_carets(0, lines, 0);
        assert!(app.extra_carets().len() <= 2, "too many: refused");
    }

    #[test]
    fn paste_puts_a_piece_at_each_caret_when_they_match() {
        let (mut app, _f) = app_on_file("a.txt", "a\nb\n");
        standard(&mut app);
        app.toggle_caret_at(2);
        assert!(app.paste_at_every_caret("1\n2\n"));
        assert_eq!(app.text(), "1a\n2b\n");
        assert!(app.paste_at_every_caret("+"));
        assert_eq!(app.text(), "1+a\n2+b\n");
        assert_eq!(app.copy_every_caret().as_deref(), Some("1+a\n2+b"));
    }

    #[test]
    fn an_edit_made_elsewhere_drops_the_extras() {
        let (mut app, _f) = app_on_file("a.txt", "a\nb\n");
        standard(&mut app);
        app.toggle_caret_at(2);
        let (buffer, cursor) = app.focused_buffer_and_cursor_mut();
        let mut c = *cursor;
        buffer.insert_char(&mut c, 'z');
        assert!(app.extra_carets().is_empty());
    }

    #[test]
    fn vim_keys_never_have_extra_carets() {
        let (mut app, _f) = app_on_file("a.txt", "a\nb\n");
        app.toggle_caret_at(2);
        app.press("x");
        assert!(app.extra_carets().is_empty());
        assert_eq!(app.text(), "\nb\n", "x is Vim's");
    }
}
