//! Standard keys: the editing policy of a traditional IDE on top of the
//! same engine. A buffer you type in rests in Insert; Escape never leaves
//! it; Shift with a movement selects (Visual underneath, between the
//! anchor and the cursor, the cursor's character not in it); typing,
//! Backspace or Enter replace the selection; Ctrl with an arrow goes a
//! word at a time. Undo steps break where a new word starts and at Enter,
//! so one undo doesn't take back all that was typed since the last move.
//!
//! `handle_standard_key` takes the keys this policy means differently and
//! leaves the rest (a plain character typed, a key Vim's Insert already
//! does as an IDE would) to the ordinary handlers.

use fenix_core::{Buffer, Cursor};
use fenix_keymap::{KeyCode, KeyPress, NamedKey};

use super::{Mode, VimEvent, VimState, VisualKind};
use crate::indent;
use crate::motion::{self, Motion};

/// How keys edit text: Vim's modes and keys, or standard keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EditPolicy {
    #[default]
    Vim,
    Standard,
}

/// Where a movement key takes the cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Move {
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    WordLeft,
    WordRight,
    Top,
    Bottom,
}

impl VimState {
    /// The editing policy in use.
    pub fn policy(&self) -> EditPolicy {
        self.policy
    }

    /// Switches the editing policy. Going to standard keys leaves a pending
    /// sequence and Visual behind; the host puts the cursor back in Insert
    /// (`enter_insert_here`) where a buffer can be typed in.
    pub fn set_policy(&mut self, policy: EditPolicy) {
        if self.policy == policy {
            return;
        }
        self.policy = policy;
        self.cancel_pending();
        if self.mode == Mode::Visual {
            self.mode = Mode::Normal;
        }
    }

    /// Standard keys, with a selection made (Visual underneath).
    pub fn is_selecting(&self) -> bool {
        self.policy == EditPolicy::Standard && self.mode == Mode::Visual
    }

    /// Into Insert where the cursor is, as `i` would -- standard keys'
    /// resting mode, for the host to put a buffer you type in back into.
    pub fn enter_insert_here(&mut self, buffer: &Buffer, cursor: &mut Cursor) {
        self.cancel_pending();
        self.one_shot_from = None;
        self.mode = Mode::Insert;
        let (_, col) = buffer.line_col(cursor);
        cursor.sticky_col = col;
    }

    /// Out of Insert where the cursor is: Escape's ending of the typing,
    /// without its step back a character.
    pub fn leave_insert_here(&mut self, buffer: &mut Buffer) {
        if self.is_inserting() {
            self.mode = Mode::Normal;
            self.replay_block_insert(buffer);
        }
    }

    /// A key, as standard keys mean it -- `None` for one the ordinary
    /// handlers do as an IDE would.
    pub(super) fn handle_standard_key(&mut self, buffer: &mut Buffer, cursor: &mut Cursor, key: KeyPress) -> Option<VimEvent> {
        let selecting = self.mode == Mode::Visual;
        if !(self.is_inserting() || selecting) {
            return None;
        }
        let mods = key.mods;
        let shift = mods.shift;
        let movement = movement(key);
        if let Some(mv) = movement {
            if shift {
                if !selecting {
                    if self.is_inserting() {
                        self.leave_insert_here(buffer);
                    }
                    self.select(VisualKind::Char, cursor.char_idx);
                }
                self.move_cursor(buffer, cursor, mv);
            } else {
                if selecting {
                    let (range, _) = self.visual_range(buffer, cursor);
                    self.end_selection(buffer, cursor);
                    // Left and Right collapse the selection to its edge;
                    // anything else moves from where the cursor is.
                    match mv {
                        Move::Left => cursor.char_idx = range.start,
                        Move::Right => cursor.char_idx = range.end,
                        _ => self.move_cursor(buffer, cursor, mv),
                    }
                    let (_, col) = buffer.line_col(cursor);
                    cursor.sticky_col = col;
                } else {
                    buffer.break_undo_run();
                    self.move_cursor(buffer, cursor, mv);
                }
            }
            return Some(VimEvent::None);
        }
        match key.code {
            KeyCode::Named(NamedKey::Escape) => {
                if selecting {
                    self.end_selection(buffer, cursor);
                }
                Some(VimEvent::None)
            }
            // What's selected goes, and the key is typed in its place.
            KeyCode::Named(NamedKey::Backspace | NamedKey::Delete) if selecting => {
                self.delete_selection(buffer, cursor);
                Some(VimEvent::None)
            }
            KeyCode::Named(NamedKey::Tab) if selecting => {
                self.indent_selection(buffer, cursor, !shift);
                Some(VimEvent::None)
            }
            KeyCode::Named(NamedKey::Tab) if shift => {
                let (line, _) = buffer.line_col(cursor);
                indent::dedent_line(buffer, cursor, line, self.indent_width);
                Some(VimEvent::None)
            }
            KeyCode::Named(NamedKey::Enter) | KeyCode::Char(_) if selecting && !mods.ctrl && !mods.alt && !mods.super_ => {
                self.delete_selection(buffer, cursor);
                Some(self.handle_insert_key(buffer, cursor, key.legacy(), false))
            }
            KeyCode::Named(NamedKey::Backspace) if mods.ctrl => {
                let to = motion::target(buffer, cursor, Motion::WordBackward, &self.iskeyword_extra);
                if to < cursor.char_idx {
                    buffer.delete_range(cursor, to, cursor.char_idx);
                    cursor.char_idx = to;
                }
                Some(VimEvent::None)
            }
            KeyCode::Named(NamedKey::Delete) if mods.ctrl => {
                let to = word_right(buffer, cursor.char_idx, &self.iskeyword_extra);
                if to > cursor.char_idx {
                    let at = cursor.char_idx;
                    buffer.delete_range(cursor, at, to);
                    cursor.char_idx = at;
                }
                Some(VimEvent::None)
            }
            KeyCode::Named(NamedKey::Enter) if !mods.ctrl && !mods.alt => {
                // Enter is an undo step of its own: before and after.
                buffer.break_undo_run();
                let event = self.handle_insert_key(buffer, cursor, key.legacy(), false);
                buffer.break_undo_run();
                Some(event)
            }
            KeyCode::Char(c) if !mods.ctrl && !mods.alt && !mods.super_ => {
                // A new word starts a new undo step.
                let before = cursor.char_idx.checked_sub(1).and_then(|i| buffer.char_at(i));
                if !c.is_whitespace() && before.is_some_and(char::is_whitespace) {
                    buffer.break_undo_run();
                }
                None
            }
            // Insert's own Vim chords (Ctrl-O, Ctrl-W, Ctrl-R...) don't
            // apply: a chord nothing took does nothing.
            KeyCode::Char(_) => Some(VimEvent::None),
            _ if selecting => {
                // Keys with nothing to do to a selection leave it be.
                Some(VimEvent::None)
            }
            _ => None,
        }
    }

    /// The selection over, typing again where the cursor is.
    fn end_selection(&mut self, buffer: &Buffer, cursor: &mut Cursor) {
        self.exit_visual_mode(cursor);
        self.enter_insert_here(buffer, cursor);
    }

    /// What's selected deleted (not yanked: the clipboard keeps what was
    /// copied), typing again where it was.
    fn delete_selection(&mut self, buffer: &mut Buffer, cursor: &mut Cursor) {
        let (range, _) = self.visual_range(buffer, cursor);
        self.exit_visual_mode(cursor);
        if !range.is_empty() {
            buffer.delete_range(cursor, range.start, range.end);
        }
        cursor.char_idx = range.start.min(buffer.len_chars());
        self.enter_insert_here(buffer, cursor);
    }

    /// Tab and Shift+Tab over a selection: its lines indented or
    /// dedented, the selection kept on them.
    fn indent_selection(&mut self, buffer: &mut Buffer, cursor: &mut Cursor, indent: bool) {
        let anchor = Cursor { char_idx: self.visual_anchor, sticky_col: 0 };
        let (a, _) = buffer.line_col(&anchor);
        let (b, _) = buffer.line_col(cursor);
        let (lo, hi) = (a.min(b), a.max(b));
        let (anchor_line, anchor_col) = buffer.line_col(&anchor);
        let (cursor_line, cursor_col) = buffer.line_col(cursor);
        let before: Vec<usize> = (lo..=hi).map(|l| buffer.line_len(l)).collect();
        for line in lo..=hi {
            let mut c = Cursor { char_idx: buffer.line_start_char(line), sticky_col: 0 };
            if indent {
                indent::indent_line(buffer, &mut c, line, self.indent_width);
            } else {
                indent::dedent_line(buffer, &mut c, line, self.indent_width);
            }
        }
        // Each end stays on its character, moved as its line was.
        let shifted = |line: usize, col: usize, buffer: &Buffer| {
            let delta = buffer.line_len(line) as isize - before[line - lo] as isize;
            let col = (col as isize + delta).max(0) as usize;
            buffer.line_start_char(line) + col.min(buffer.line_len(line))
        };
        self.visual_anchor = shifted(anchor_line, anchor_col, buffer);
        cursor.char_idx = shifted(cursor_line, cursor_col, buffer);
    }

    /// The cursor moved as `mv` says, across lines as an IDE's does.
    fn move_cursor(&mut self, buffer: &mut Buffer, cursor: &mut Cursor, mv: Move) {
        let len = buffer.len_chars();
        let to = match mv {
            Move::Left => cursor.char_idx.saturating_sub(1),
            Move::Right => (cursor.char_idx + 1).min(len),
            Move::Up => {
                let mut c = *cursor;
                buffer.move_up(&mut c);
                *cursor = c;
                return;
            }
            Move::Down => {
                let mut c = *cursor;
                buffer.move_down(&mut c);
                *cursor = c;
                return;
            }
            Move::Home => {
                // The line's first character that isn't a space, then its
                // very start.
                let (line, _) = buffer.line_col(cursor);
                let first = motion::line_first_non_blank(buffer, line);
                let start = buffer.line_start_char(line);
                if cursor.char_idx == first || first >= start + buffer.line_len(line) {
                    start
                } else {
                    first
                }
            }
            Move::End => {
                let (line, _) = buffer.line_col(cursor);
                buffer.line_start_char(line) + buffer.line_len(line)
            }
            Move::WordLeft => motion::target(buffer, cursor, Motion::WordBackward, &self.iskeyword_extra),
            Move::WordRight => word_right(buffer, cursor.char_idx, &self.iskeyword_extra),
            Move::Top => 0,
            Move::Bottom => len,
        };
        cursor.char_idx = to;
        let (_, col) = buffer.line_col(cursor);
        cursor.sticky_col = col;
    }
}

/// The movement a key is, if it's one.
fn movement(key: KeyPress) -> Option<Move> {
    if key.mods.alt || key.mods.super_ {
        return None;
    }
    let ctrl = key.mods.ctrl;
    let KeyCode::Named(n) = key.code else { return None };
    Some(match (n, ctrl) {
        (NamedKey::Left, false) => Move::Left,
        (NamedKey::Right, false) => Move::Right,
        (NamedKey::Up, false) => Move::Up,
        (NamedKey::Down, false) => Move::Down,
        (NamedKey::Home, false) => Move::Home,
        (NamedKey::End, false) => Move::End,
        (NamedKey::Left, true) => Move::WordLeft,
        (NamedKey::Right, true) => Move::WordRight,
        (NamedKey::Home, true) => Move::Top,
        (NamedKey::End, true) => Move::Bottom,
        _ => return None,
    })
}

/// Ctrl+Right's stop: the end of the word the cursor's in or before, as an
/// IDE goes -- past the spaces, then past the word.
fn word_right(buffer: &Buffer, from: usize, iskeyword_extra: &[char]) -> usize {
    let len = buffer.len_chars();
    let mut i = from;
    // Past spaces (but not past a line's end on the first step).
    while i < len && buffer.char_at(i).is_some_and(|c| c == ' ' || c == '\t') {
        i += 1;
    }
    if i < len && buffer.char_at(i) == Some('\n') {
        return if i == from { i + 1 } else { i };
    }
    let class = |c: char| crate::charclass::classify(c, iskeyword_extra);
    let Some(first) = buffer.char_at(i).map(class) else { return i };
    while i < len && buffer.char_at(i).is_some_and(|c| c != '\n' && class(c) == first) {
        i += 1;
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup(text: &str) -> (VimState, Buffer, Cursor) {
        let mut vim = VimState::new();
        vim.set_policy(EditPolicy::Standard);
        let buffer = Buffer::from_text(text);
        let mut cursor = Cursor::at_start();
        vim.enter_insert_here(&buffer, &mut cursor);
        (vim, buffer, cursor)
    }

    fn named(n: NamedKey) -> KeyPress {
        KeyPress::named(n)
    }

    fn press(vim: &mut VimState, b: &mut Buffer, c: &mut Cursor, key: KeyPress) {
        vim.handle_key(b, c, key);
    }

    fn typed(vim: &mut VimState, b: &mut Buffer, c: &mut Cursor, text: &str) {
        for ch in text.chars() {
            let key = if ch == '\n' { named(NamedKey::Enter) } else { KeyPress::char(ch) };
            press(vim, b, c, key);
        }
    }

    #[test]
    fn escape_never_leaves_insert_and_arrows_cross_lines() {
        let (mut vim, mut b, mut c) = setup("ab\ncd");
        press(&mut vim, &mut b, &mut c, named(NamedKey::Escape));
        assert_eq!(vim.mode(), Mode::Insert);
        press(&mut vim, &mut b, &mut c, named(NamedKey::End));
        assert_eq!(c.char_idx, 2, "past the line's last character, as an IDE's End");
        press(&mut vim, &mut b, &mut c, named(NamedKey::Right));
        assert_eq!(c.char_idx, 3, "onto the next line");
        press(&mut vim, &mut b, &mut c, named(NamedKey::Left));
        press(&mut vim, &mut b, &mut c, named(NamedKey::Left));
        assert_eq!(c.char_idx, 1);
        typed(&mut vim, &mut b, &mut c, "X");
        assert_eq!(b.rope().to_string(), "aXb\ncd");
    }

    #[test]
    fn shift_selects_between_the_anchor_and_the_cursor_and_typing_replaces_it() {
        let (mut vim, mut b, mut c) = setup("hello world");
        for _ in 0..5 {
            press(&mut vim, &mut b, &mut c, named(NamedKey::Right).with_shift());
        }
        assert!(vim.is_selecting());
        assert_eq!(vim.visual_selection_range(&b, &c), Some((0..5, false)), "the cursor's character isn't in it");
        typed(&mut vim, &mut b, &mut c, "bye");
        assert_eq!(b.rope().to_string(), "bye world");
        assert_eq!(vim.mode(), Mode::Insert);
        assert_eq!(c.char_idx, 3);
        assert_eq!(vim.register().0, "", "replacing what's selected doesn't copy it");
    }

    #[test]
    fn an_arrow_collapses_the_selection_to_its_edge_and_escape_drops_it() {
        let (mut vim, mut b, mut c) = setup("hello world");
        c.char_idx = 2;
        press(&mut vim, &mut b, &mut c, named(NamedKey::Right).with_shift().with_ctrl());
        assert_eq!(vim.visual_selection_range(&b, &c), Some((2..5, false)));
        press(&mut vim, &mut b, &mut c, named(NamedKey::Left));
        assert_eq!((vim.mode(), c.char_idx), (Mode::Insert, 2));
        press(&mut vim, &mut b, &mut c, named(NamedKey::Right).with_shift());
        press(&mut vim, &mut b, &mut c, named(NamedKey::Escape));
        assert_eq!((vim.mode(), c.char_idx), (Mode::Insert, 3), "Esc drops it where the cursor is");
    }

    #[test]
    fn backspace_deletes_the_selection_and_enter_replaces_it() {
        let (mut vim, mut b, mut c) = setup("one two three");
        c.char_idx = 4;
        press(&mut vim, &mut b, &mut c, named(NamedKey::End).with_shift());
        press(&mut vim, &mut b, &mut c, named(NamedKey::Backspace));
        assert_eq!(b.rope().to_string(), "one ");
        press(&mut vim, &mut b, &mut c, named(NamedKey::Home).with_shift());
        press(&mut vim, &mut b, &mut c, named(NamedKey::Enter));
        assert_eq!(b.rope().to_string(), "\n");
    }

    #[test]
    fn ctrl_moves_and_deletes_a_word_at_a_time() {
        let (mut vim, mut b, mut c) = setup("let value = 42;");
        press(&mut vim, &mut b, &mut c, named(NamedKey::Right).with_ctrl());
        assert_eq!(c.char_idx, 3, "the end of `let`");
        press(&mut vim, &mut b, &mut c, named(NamedKey::Right).with_ctrl());
        assert_eq!(c.char_idx, 9, "past the space, then `value`");
        press(&mut vim, &mut b, &mut c, named(NamedKey::Backspace).with_ctrl());
        assert_eq!(b.rope().to_string(), "let  = 42;");
        press(&mut vim, &mut b, &mut c, named(NamedKey::Delete).with_ctrl());
        assert_eq!(b.rope().to_string(), "let  42;", "the space and the next word");
        press(&mut vim, &mut b, &mut c, named(NamedKey::End).with_ctrl());
        assert_eq!(c.char_idx, b.len_chars());
        press(&mut vim, &mut b, &mut c, named(NamedKey::Home).with_ctrl());
        assert_eq!(c.char_idx, 0);
    }

    #[test]
    fn home_goes_to_the_first_character_then_the_line_s_start() {
        let (mut vim, mut b, mut c) = setup("    code");
        c.char_idx = 6;
        press(&mut vim, &mut b, &mut c, named(NamedKey::Home));
        assert_eq!(c.char_idx, 4);
        press(&mut vim, &mut b, &mut c, named(NamedKey::Home));
        assert_eq!(c.char_idx, 0);
        press(&mut vim, &mut b, &mut c, named(NamedKey::Home));
        assert_eq!(c.char_idx, 4);
    }

    #[test]
    fn undo_takes_back_a_word_or_a_line_not_everything_typed() {
        let (mut vim, mut b, mut c) = setup("");
        typed(&mut vim, &mut b, &mut c, "one two\nthree");
        b.undo(&mut c);
        assert_eq!(b.rope().to_string(), "one two\n", "the last word");
        b.undo(&mut c);
        assert_eq!(b.rope().to_string(), "one two", "Enter");
        b.undo(&mut c);
        assert_eq!(b.rope().to_string(), "one ", "`two`");
    }

    #[test]
    fn tab_indents_the_selected_lines_and_keeps_them_selected() {
        let (mut vim, mut b, mut c) = setup("a\nb\nc");
        press(&mut vim, &mut b, &mut c, named(NamedKey::Down).with_shift());
        press(&mut vim, &mut b, &mut c, named(NamedKey::Tab));
        assert_eq!(b.rope().to_string(), "    a\n    b\nc");
        assert!(vim.is_selecting());
        press(&mut vim, &mut b, &mut c, named(NamedKey::Tab).with_shift());
        assert_eq!(b.rope().to_string(), "a\nb\nc");
    }

    #[test]
    fn insert_s_vim_chords_do_nothing_and_vim_keys_are_text() {
        let (mut vim, mut b, mut c) = setup("");
        press(&mut vim, &mut b, &mut c, KeyPress::char('o').with_ctrl());
        assert_eq!(vim.mode(), Mode::Insert, "Ctrl-O isn't one Normal command");
        typed(&mut vim, &mut b, &mut c, "dd:wq");
        assert_eq!(b.rope().to_string(), "dd:wq");
    }

    #[test]
    fn vim_keys_are_untouched_under_vim_s_policy() {
        let mut vim = VimState::new();
        let mut b = Buffer::from_text("hello");
        let mut c = Cursor::at_start();
        vim.handle_key(&mut b, &mut c, KeyPress::char('v'));
        vim.handle_key(&mut b, &mut c, KeyPress::char('l'));
        assert_eq!(vim.visual_selection_range(&b, &c), Some((0..2, false)), "Vim's selection has the cursor's character");
    }
}
