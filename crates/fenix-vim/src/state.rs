use std::ops::Range;

use fenix_core::{Buffer, Cursor};
use fenix_keymap::{KeyCode, KeyPress, Matcher, Mods, NamedKey, Step};

use crate::keymaps::{self, InsertEntry, PendingTarget, VimAction, VisualAction};
use crate::mode::Mode;
use crate::motion::{self, Motion};
use crate::operator::Operator;
use crate::textobject;

/// What `VimState::handle_key` wants the host application to do -- the one
/// escape hatch out of pure buffer/cursor editing, for the handful of `:`
/// ex-commands that need app-level action (save/quit) rather than a buffer
/// edit. Everything else stays inside fenix-vim.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VimEvent {
    None,
    RequestSave,
    RequestQuit,
    RequestSaveAndQuit,
}

struct Register {
    text: String,
    /// Whether this register holds whole lines (from a linewise operation
    /// like `dd`/`yy`/`dG`) vs. a char span -- determines whether `p`/`P`
    /// paste as new lines or inline at the cursor.
    linewise: bool,
}

pub struct VimState {
    mode: Mode,
    register: Register,
    pending_op: Option<Operator>,
    visual_anchor: usize,
    command_line: String,

    normal_matcher: Matcher<'static, VimAction>,
    visual_matcher: Matcher<'static, VisualAction>,
    pending_matcher: Matcher<'static, PendingTarget>,
}

impl VimState {
    pub fn new() -> Self {
        Self {
            mode: Mode::Normal,
            register: Register { text: String::new(), linewise: false },
            pending_op: None,
            visual_anchor: 0,
            command_line: String::new(),
            normal_matcher: keymaps::normal_trie().matcher(),
            visual_matcher: keymaps::visual_trie().matcher(),
            pending_matcher: keymaps::pending_trie().matcher(),
        }
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn command_line(&self) -> &str {
        &self.command_line
    }

    /// Whether a multi-key sequence (operator-pending, `gg`, ...) is
    /// waiting on more input, for a which-key-style hint in the host UI.
    pub fn is_pending(&self) -> bool {
        self.pending_op.is_some() || self.normal_matcher.is_pending() || self.visual_matcher.is_pending()
    }

    pub fn handle_key(&mut self, buffer: &mut Buffer, cursor: &mut Cursor, key: KeyPress) -> VimEvent {
        match self.mode {
            Mode::Insert => self.handle_insert_key(buffer, cursor, key, false),
            Mode::Replace => self.handle_insert_key(buffer, cursor, key, true),
            Mode::Command => self.handle_command_key(key),
            Mode::Normal => self.handle_normal_key(buffer, cursor, key),
            Mode::Visual => self.handle_visual_key(buffer, cursor, key),
        }
    }

    fn handle_insert_key(
        &mut self,
        buffer: &mut Buffer,
        cursor: &mut Cursor,
        key: KeyPress,
        replace: bool,
    ) -> VimEvent {
        match key.code {
            KeyCode::Named(NamedKey::Escape) => {
                self.mode = Mode::Normal;
                let (_, col) = buffer.line_col(cursor);
                if col > 0 {
                    buffer.move_left(cursor);
                }
            }
            KeyCode::Named(NamedKey::Backspace) => buffer.delete_backward(cursor),
            KeyCode::Named(NamedKey::Delete) => buffer.delete_forward(cursor),
            KeyCode::Named(NamedKey::Enter) => buffer.insert_char(cursor, '\n'),
            KeyCode::Named(NamedKey::Tab) => buffer.insert_char(cursor, '\t'),
            KeyCode::Named(NamedKey::Left) => buffer.move_left(cursor),
            KeyCode::Named(NamedKey::Right) => buffer.move_right(cursor),
            KeyCode::Named(NamedKey::Up) => buffer.move_up(cursor),
            KeyCode::Named(NamedKey::Down) => buffer.move_down(cursor),
            KeyCode::Named(NamedKey::Home) => buffer.move_home(cursor),
            KeyCode::Named(NamedKey::End) => buffer.move_end(cursor),
            KeyCode::Char(c) if key.mods == Mods::default() => {
                if replace && buffer.char_at(cursor.char_idx).is_some() {
                    buffer.delete_forward(cursor);
                }
                buffer.insert_char(cursor, c);
            }
            _ => {}
        }
        VimEvent::None
    }

    fn handle_command_key(&mut self, key: KeyPress) -> VimEvent {
        match key.code {
            KeyCode::Named(NamedKey::Escape) => {
                self.command_line.clear();
                self.mode = Mode::Normal;
            }
            KeyCode::Named(NamedKey::Enter) => {
                let cmd = std::mem::take(&mut self.command_line);
                self.mode = Mode::Normal;
                return run_ex_command(&cmd);
            }
            KeyCode::Named(NamedKey::Backspace) => {
                self.command_line.pop();
            }
            KeyCode::Char(c) if key.mods == Mods::default() => {
                self.command_line.push(c);
            }
            _ => {}
        }
        VimEvent::None
    }

    fn handle_normal_key(&mut self, buffer: &mut Buffer, cursor: &mut Cursor, key: KeyPress) -> VimEvent {
        if let Some(op) = self.pending_op {
            self.handle_operator_pending_key(buffer, cursor, op, key);
            return VimEvent::None;
        }

        if let Step::Matched(action) = self.normal_matcher.feed(key) {
            self.apply_normal_action(buffer, cursor, *action);
        }
        VimEvent::None
    }

    fn apply_normal_action(&mut self, buffer: &mut Buffer, cursor: &mut Cursor, action: VimAction) {
        match action {
            VimAction::Motion(m) => apply_motion(buffer, cursor, m),
            VimAction::Operator(op) => self.pending_op = Some(op),
            VimAction::EnterInsert(entry) => self.enter_insert(buffer, cursor, entry),
            VimAction::EnterVisual => {
                self.mode = Mode::Visual;
                self.visual_anchor = cursor.char_idx;
            }
            VimAction::EnterCommandLine => {
                self.mode = Mode::Command;
                self.command_line.clear();
            }
            VimAction::Undo => {
                buffer.undo(cursor);
            }
            VimAction::Redo => {
                buffer.redo(cursor);
            }
            VimAction::DeleteCharUnder => {
                let end = (cursor.char_idx + 1).min(buffer.len_chars());
                if end > cursor.char_idx {
                    let text = buffer.delete_range(cursor, cursor.char_idx, end);
                    self.register = Register { text, linewise: false };
                }
            }
            VimAction::DeleteCharBefore => {
                if cursor.char_idx > 0 {
                    let start = cursor.char_idx - 1;
                    let text = buffer.delete_range(cursor, start, cursor.char_idx);
                    self.register = Register { text, linewise: false };
                }
            }
            VimAction::PasteAfter => self.paste(buffer, cursor, true),
            VimAction::PasteBefore => self.paste(buffer, cursor, false),
        }
    }

    fn enter_insert(&mut self, buffer: &mut Buffer, cursor: &mut Cursor, entry: InsertEntry) {
        match entry {
            InsertEntry::Before => {}
            InsertEntry::After => {
                if buffer.char_at(cursor.char_idx).is_some() {
                    cursor.char_idx += 1;
                }
            }
            InsertEntry::LineStart => cursor.char_idx = motion::target(buffer, cursor, Motion::LineFirstNonBlank),
            InsertEntry::LineEnd => {
                let (line, _) = buffer.line_col(cursor);
                cursor.char_idx = buffer.line_start_char(line) + buffer.line_len(line);
            }
            InsertEntry::NewlineBelow => {
                let (line, _) = buffer.line_col(cursor);
                cursor.char_idx = buffer.line_start_char(line) + buffer.line_len(line);
                buffer.insert_char(cursor, '\n');
            }
            InsertEntry::NewlineAbove => {
                let (line, _) = buffer.line_col(cursor);
                cursor.char_idx = buffer.line_start_char(line);
                buffer.insert_char(cursor, '\n');
                cursor.char_idx -= 1;
            }
        }
        self.mode = Mode::Insert;
        let (_, col) = buffer.line_col(cursor);
        cursor.sticky_col = col;
    }

    fn handle_operator_pending_key(
        &mut self,
        buffer: &mut Buffer,
        cursor: &mut Cursor,
        op: Operator,
        key: KeyPress,
    ) {
        if key.code == KeyCode::Named(NamedKey::Escape) {
            self.pending_op = None;
            self.pending_matcher.cancel();
            return;
        }

        // Doubled operator (dd/cc/yy): linewise, current line. Only applies
        // as the very first key after the operator, matching Vim.
        if !self.pending_matcher.is_pending() && key == op.trigger_key() {
            self.pending_op = None;
            let (line, _) = buffer.line_col(cursor);
            let range = linewise_range(buffer, line, line);
            self.finish_operator(buffer, cursor, op, range, true);
            return;
        }

        match self.pending_matcher.feed(key) {
            Step::Pending(_) => {}
            Step::NoMatch => self.pending_op = None,
            Step::Matched(target) => {
                let target = *target;
                self.pending_op = None;
                let (range, linewise) = match target {
                    PendingTarget::Motion(m) => {
                        let m = adjust_for_change_word(op, m, buffer, cursor);
                        (range_for_motion(buffer, cursor, m), m.is_linewise())
                    }
                    PendingTarget::TextObject(obj) => (textobject::span(buffer, cursor, obj), false),
                };
                self.finish_operator(buffer, cursor, op, range, linewise);
            }
        }
    }

    fn finish_operator(
        &mut self,
        buffer: &mut Buffer,
        cursor: &mut Cursor,
        op: Operator,
        range: Range<usize>,
        linewise: bool,
    ) {
        match op {
            Operator::Yank => {
                let text = buffer.text_range(range.start, range.end);
                self.register = Register { text, linewise };
                cursor.char_idx = range.start.min(buffer.len_chars());
                let (_, col) = buffer.line_col(cursor);
                cursor.sticky_col = col;
            }
            Operator::Delete | Operator::Change => {
                let text = buffer.delete_range(cursor, range.start, range.end);
                self.register = Register { text, linewise };
                if op == Operator::Change {
                    self.mode = Mode::Insert;
                }
            }
        }
    }

    fn paste(&mut self, buffer: &mut Buffer, cursor: &mut Cursor, after: bool) {
        if self.register.text.is_empty() {
            return;
        }
        if self.register.linewise {
            let (line, _) = buffer.line_col(cursor);
            let insert_line = if after { line + 1 } else { line };
            let at = if insert_line < buffer.line_count() {
                buffer.line_start_char(insert_line)
            } else {
                buffer.len_chars()
            };
            cursor.char_idx = at;
            let mut block = self.register.text.clone();
            if !block.ends_with('\n') {
                block.push('\n');
            }
            buffer.insert_str(cursor, &block);
            cursor.char_idx = at;
        } else {
            let at = if after { (cursor.char_idx + 1).min(buffer.len_chars()) } else { cursor.char_idx };
            cursor.char_idx = at;
            buffer.insert_str(cursor, &self.register.text);
            cursor.char_idx = at;
        }
        let (_, col) = buffer.line_col(cursor);
        cursor.sticky_col = col;
    }

    fn handle_visual_key(&mut self, buffer: &mut Buffer, cursor: &mut Cursor, key: KeyPress) -> VimEvent {
        if key.code == KeyCode::Named(NamedKey::Escape) {
            self.mode = Mode::Normal;
            return VimEvent::None;
        }

        if let Step::Matched(action) = self.visual_matcher.feed(key) {
            match *action {
                VisualAction::Motion(m) => apply_motion(buffer, cursor, m),
                VisualAction::Apply(op) => {
                    let (lo, hi) = if self.visual_anchor <= cursor.char_idx {
                        (self.visual_anchor, cursor.char_idx + 1)
                    } else {
                        (cursor.char_idx, self.visual_anchor + 1)
                    };
                    let hi = hi.min(buffer.len_chars());
                    self.finish_operator(buffer, cursor, op, lo..hi, false);
                    if self.mode == Mode::Visual {
                        self.mode = Mode::Normal;
                    }
                }
                VisualAction::Exit => self.mode = Mode::Normal,
            }
        }
        VimEvent::None
    }
}

impl Default for VimState {
    fn default() -> Self {
        Self::new()
    }
}

fn apply_motion(buffer: &Buffer, cursor: &mut Cursor, m: Motion) {
    cursor.char_idx = motion::target(buffer, cursor, m);
    if !matches!(m, Motion::Up | Motion::Down) {
        let (_, col) = buffer.line_col(cursor);
        cursor.sticky_col = col;
    }
}

/// Vim's well-known `cw`-behaves-like-`ce` rule: changing a word from a
/// non-blank char shouldn't also swallow the trailing whitespace the way
/// plain `dw` would, since you're about to type a replacement right after
/// the word.
fn adjust_for_change_word(op: Operator, motion: Motion, buffer: &Buffer, cursor: &Cursor) -> Motion {
    if op == Operator::Change && motion == Motion::WordForward && motion::is_non_blank_at(buffer, cursor.char_idx) {
        Motion::WordEndForward
    } else {
        motion
    }
}

fn linewise_range(buffer: &Buffer, line_a: usize, line_b: usize) -> Range<usize> {
    let (lo, hi) = if line_a <= line_b { (line_a, line_b) } else { (line_b, line_a) };
    let start = buffer.line_start_char(lo);
    let end =
        if hi + 1 < buffer.line_count() { buffer.line_start_char(hi + 1) } else { buffer.len_chars() };
    start..end
}

fn range_for_motion(buffer: &Buffer, cursor: &Cursor, motion: Motion) -> Range<usize> {
    let target = motion::target(buffer, cursor, motion);
    if motion.is_linewise() {
        let (cur_line, _) = buffer.line_col(cursor);
        let (tgt_line, _) = buffer.line_col(&Cursor { char_idx: target, sticky_col: 0 });
        return linewise_range(buffer, cur_line, tgt_line);
    }
    let cur = cursor.char_idx;
    let (lo, hi) = if cur <= target { (cur, target) } else { (target, cur) };
    let hi = match motion.inclusivity() {
        motion::Inclusivity::Inclusive => (hi + 1).min(buffer.len_chars()),
        motion::Inclusivity::Exclusive => hi,
    };
    lo..hi
}

fn run_ex_command(cmd: &str) -> VimEvent {
    match cmd.trim() {
        "w" => VimEvent::RequestSave,
        "q" | "q!" => VimEvent::RequestQuit,
        "wq" | "x" => VimEvent::RequestSaveAndQuit,
        _ => VimEvent::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::buf;

    fn keys(s: &mut VimState, b: &mut Buffer, c: &mut Cursor, seq: &str) {
        for ch in seq.chars() {
            s.handle_key(b, c, KeyPress::char(ch));
        }
    }

    fn named(s: &mut VimState, b: &mut Buffer, c: &mut Cursor, n: NamedKey) -> VimEvent {
        s.handle_key(b, c, KeyPress::named(n))
    }

    #[test]
    fn starts_in_normal_mode() {
        let vim = VimState::new();
        assert_eq!(vim.mode(), Mode::Normal);
    }

    #[test]
    fn hjkl_move_the_cursor() {
        let mut b = buf("ab\ncd");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "l");
        assert_eq!(c.char_idx, 1);
        keys(&mut vim, &mut b, &mut c, "j");
        assert_eq!(b.line_col(&c), (1, 1));
        keys(&mut vim, &mut b, &mut c, "h");
        assert_eq!(b.line_col(&c), (1, 0));
        keys(&mut vim, &mut b, &mut c, "k");
        assert_eq!(b.line_col(&c), (0, 0));
    }

    #[test]
    fn i_enters_insert_mode_and_typing_inserts() {
        let mut b = buf("bc");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "i");
        assert_eq!(vim.mode(), Mode::Insert);
        keys(&mut vim, &mut b, &mut c, "a");
        assert_eq!(b.text(), "abc");
        named(&mut vim, &mut b, &mut c, NamedKey::Escape);
        assert_eq!(vim.mode(), Mode::Normal);
    }

    #[test]
    fn escape_from_insert_moves_cursor_back_one_column() {
        let mut b = buf("");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "i");
        keys(&mut vim, &mut b, &mut c, "hi");
        assert_eq!(c.char_idx, 2);
        named(&mut vim, &mut b, &mut c, NamedKey::Escape);
        assert_eq!(c.char_idx, 1); // on the 'i', not past it
    }

    #[test]
    fn dw_deletes_word_forward() {
        let mut b = buf("foo bar baz");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "dw");
        assert_eq!(b.text(), "bar baz");
        assert_eq!(c.char_idx, 0);
    }

    #[test]
    fn dd_deletes_the_whole_line_including_newline() {
        let mut b = buf("one\ntwo\nthree");
        let mut c = Cursor { char_idx: 4, sticky_col: 0 }; // on "two"
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "dd");
        assert_eq!(b.text(), "one\nthree");
    }

    #[test]
    fn diw_deletes_inner_word() {
        let mut b = buf("foo bar baz");
        let mut c = Cursor { char_idx: 5, sticky_col: 5 }; // inside "bar"
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "diw");
        assert_eq!(b.text(), "foo  baz");
    }

    #[test]
    fn daw_deletes_a_word_with_trailing_space() {
        let mut b = buf("foo bar baz");
        let mut c = Cursor { char_idx: 5, sticky_col: 5 };
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "daw");
        assert_eq!(b.text(), "foo baz");
    }

    #[test]
    fn cw_on_a_word_behaves_like_ce_not_swallowing_trailing_space() {
        let mut b = buf("foo bar");
        let mut c = Cursor::at_start(); // on "foo"
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "cw");
        assert_eq!(vim.mode(), Mode::Insert);
        assert_eq!(b.text(), " bar"); // "foo" gone, the space survives
        keys(&mut vim, &mut b, &mut c, "X");
        assert_eq!(b.text(), "X bar");
    }

    #[test]
    fn yy_then_p_duplicates_the_line_below() {
        let mut b = buf("one\ntwo");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "yy");
        assert_eq!(b.text(), "one\ntwo"); // yank doesn't delete
        keys(&mut vim, &mut b, &mut c, "p");
        assert_eq!(b.text(), "one\none\ntwo");
    }

    #[test]
    fn x_deletes_char_under_cursor_into_register_and_p_pastes_it() {
        let mut b = buf("abc");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "x");
        assert_eq!(b.text(), "bc");
        keys(&mut vim, &mut b, &mut c, "p");
        assert_eq!(b.text(), "bac");
    }

    #[test]
    fn undo_and_redo_via_u_and_ctrl_r() {
        let mut b = buf("");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "i");
        keys(&mut vim, &mut b, &mut c, "hi");
        named(&mut vim, &mut b, &mut c, NamedKey::Escape);
        assert_eq!(b.text(), "hi");

        keys(&mut vim, &mut b, &mut c, "u");
        assert_eq!(b.text(), "");

        vim.handle_key(&mut b, &mut c, KeyPress::char('r').with_ctrl());
        assert_eq!(b.text(), "hi");
    }

    #[test]
    fn visual_mode_delete_removes_the_selection() {
        let mut b = buf("hello world");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "v");
        assert_eq!(vim.mode(), Mode::Visual);
        keys(&mut vim, &mut b, &mut c, "llll"); // select "hello"
        keys(&mut vim, &mut b, &mut c, "d");
        assert_eq!(b.text(), " world");
        assert_eq!(vim.mode(), Mode::Normal);
    }

    #[test]
    fn visual_escape_cancels_without_editing() {
        let mut b = buf("hello");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "v");
        named(&mut vim, &mut b, &mut c, NamedKey::Escape);
        assert_eq!(vim.mode(), Mode::Normal);
        assert_eq!(b.text(), "hello");
    }

    #[test]
    fn command_line_w_and_q_produce_events() {
        let mut b = buf("x");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, ":");
        assert_eq!(vim.mode(), Mode::Command);
        keys(&mut vim, &mut b, &mut c, "w");
        let ev = named(&mut vim, &mut b, &mut c, NamedKey::Enter);
        assert_eq!(ev, VimEvent::RequestSave);
        assert_eq!(vim.mode(), Mode::Normal);

        keys(&mut vim, &mut b, &mut c, ":wq");
        let ev = named(&mut vim, &mut b, &mut c, NamedKey::Enter);
        assert_eq!(ev, VimEvent::RequestSaveAndQuit);
    }

    #[test]
    fn command_line_escape_cancels() {
        let mut b = buf("x");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, ":q");
        named(&mut vim, &mut b, &mut c, NamedKey::Escape);
        assert_eq!(vim.mode(), Mode::Normal);
        assert_eq!(vim.command_line(), "");
    }

    #[test]
    fn operator_pending_escape_cancels_without_editing() {
        let mut b = buf("hello world");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "d");
        named(&mut vim, &mut b, &mut c, NamedKey::Escape);
        keys(&mut vim, &mut b, &mut c, "l");
        assert_eq!(c.char_idx, 1); // 'l' was treated as a fresh motion, not consumed by 'd'
        assert_eq!(b.text(), "hello world");
    }

    #[test]
    fn o_and_shift_o_open_lines_and_enter_insert() {
        let mut b = buf("mid");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "o");
        assert_eq!(vim.mode(), Mode::Insert);
        keys(&mut vim, &mut b, &mut c, "below");
        assert_eq!(b.text(), "mid\nbelow");

        named(&mut vim, &mut b, &mut c, NamedKey::Escape);
        keys(&mut vim, &mut b, &mut c, "gg"); // back to line 0
        keys(&mut vim, &mut b, &mut c, "O");
        keys(&mut vim, &mut b, &mut c, "above");
        assert_eq!(b.text(), "above\nmid\nbelow");
    }

    #[test]
    fn is_pending_reflects_partial_sequences() {
        let mut b = buf("hello");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        assert!(!vim.is_pending());
        keys(&mut vim, &mut b, &mut c, "d");
        assert!(vim.is_pending());
        keys(&mut vim, &mut b, &mut c, "w");
        assert!(!vim.is_pending());
    }
}
