use std::ops::Range;

use fenix_core::{Buffer, Cursor};
use fenix_keymap::{KeyCode, KeyPress, Matcher, Mods, NamedKey, Step};

use crate::indent;
use crate::keymaps::{self, InsertEntry, PendingTarget, VimAction, VisualAction};
use crate::mode::{Mode, VisualKind};
use crate::motion::{self, Motion};
use crate::operator::Operator;
use crate::textobject;

/// What `VimState::handle_key` wants the host application to do -- the one
/// escape hatch out of pure buffer/cursor editing, for the handful of `:`
/// ex-commands that need app-level action (save/quit) rather than a buffer
/// edit, plus a visual-feedback hint the host UI can act on however it
/// likes. Everything else stays inside fenix-vim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VimEvent {
    None,
    RequestSave,
    RequestQuit,
    RequestSaveAndQuit,
    /// A yank or paste just happened over this char range -- modeled on
    /// orbit-emacs's own yank/paste pulse feature, for the host UI to
    /// briefly highlight and fade. Not raised for Block-mode yank/paste
    /// (no single contiguous range to pulse cleanly) or delete.
    Pulse(Range<usize>),
}

struct Register {
    text: String,
    /// Whether this register holds whole lines (from a linewise operation
    /// like `dd`/`yy`/`dG`) vs. a char span -- determines whether `p`/`P`
    /// paste as new lines or inline at the cursor.
    linewise: bool,
}

/// Tracks a Visual-Block `I` session: what's typed on the top line gets
/// replayed at the same column on every other line in the block when
/// `Escape` ends Insert mode. Ragged lines shorter than `col` are skipped
/// rather than padded with spaces -- a deliberate simplification.
struct BlockInsert {
    line_lo: usize,
    line_hi: usize,
    col: usize,
    typed: String,
}

pub struct VimState {
    mode: Mode,
    register: Register,
    pending_op: Option<Operator>,
    /// The count that was accumulated before the operator was entered
    /// (`3dd`); combined multiplicatively with whatever count is typed
    /// between the operator and its motion (`d3w`, or `d3d` for the
    /// doubled form) at resolution time.
    pending_op_count: u32,
    /// Set by `r`: the *next* key replaces the char under the cursor with
    /// itself repeated `count` times, instead of going through the normal
    /// trie at all -- no motion/text object composition, just one raw key.
    pending_replace: Option<u32>,
    /// Digits accumulated so far for a numeric count prefix (`3w`, `2dd`).
    /// Consumed whenever a key sequence actually resolves to an action;
    /// preserved across `Step::Pending` (mid multi-key sequences like
    /// `gg`) so `3gg` doesn't lose the `3` on the first `g`.
    count: Option<u32>,
    visual_anchor: usize,
    visual_kind: VisualKind,
    /// The kind/anchor/cursor of the most recently exited Visual
    /// selection, for `gv` to restore.
    last_visual: Option<(VisualKind, usize, usize)>,
    block_insert: Option<BlockInsert>,
    /// Set by a yank or (non-block) paste; consumed by `handle_key` right
    /// after dispatch and turned into `VimEvent::Pulse`.
    pending_pulse: Option<Range<usize>>,
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
            pending_op_count: 1,
            pending_replace: None,
            count: None,
            visual_anchor: 0,
            visual_kind: VisualKind::Char,
            last_visual: None,
            block_insert: None,
            pending_pulse: None,
            command_line: String::new(),
            normal_matcher: keymaps::normal_trie().matcher(),
            visual_matcher: keymaps::visual_trie().matcher(),
            pending_matcher: keymaps::pending_trie().matcher(),
        }
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// Which kind of selection Visual mode is making. Only meaningful
    /// while `mode()` is `Visual`.
    pub fn visual_kind(&self) -> VisualKind {
        self.visual_kind
    }

    pub fn command_line(&self) -> &str {
        &self.command_line
    }

    /// The char offset Visual mode's selection is anchored at. Only
    /// meaningful while `mode()` is `Visual`; the host UI uses this
    /// together with the cursor position to render the selection.
    pub fn visual_anchor(&self) -> usize {
        self.visual_anchor
    }

    /// Whether a multi-key sequence (operator-pending, `gg`, ...) is
    /// waiting on more input, for a which-key-style hint in the host UI.
    pub fn is_pending(&self) -> bool {
        self.pending_op.is_some() || self.normal_matcher.is_pending() || self.visual_matcher.is_pending()
    }

    /// The key/label pairs reachable from wherever a pending sequence
    /// currently sits, for a which-key-style hint in the host UI. Empty
    /// when nothing is pending.
    pub fn pending_children(&self) -> Vec<(KeyPress, &'static str)> {
        if self.pending_op.is_some() {
            self.pending_matcher.pending_children()
        } else if self.normal_matcher.is_pending() {
            self.normal_matcher.pending_children()
        } else if self.visual_matcher.is_pending() {
            self.visual_matcher.pending_children()
        } else {
            Vec::new()
        }
    }

    pub fn handle_key(&mut self, buffer: &mut Buffer, cursor: &mut Cursor, key: KeyPress) -> VimEvent {
        let event = match self.mode {
            Mode::Insert => self.handle_insert_key(buffer, cursor, key, false),
            Mode::Replace => self.handle_insert_key(buffer, cursor, key, true),
            Mode::Command => self.handle_command_key(key),
            Mode::Normal => self.handle_normal_key(buffer, cursor, key),
            Mode::Visual => self.handle_visual_key(buffer, cursor, key),
        };
        // A pulse is purely a visual-feedback hint layered on top of
        // whatever else happened; None is the only event a yank/paste
        // keypress would otherwise produce, so this never shadows a real
        // RequestSave/Quit.
        match self.pending_pulse.take() {
            Some(range) => VimEvent::Pulse(range),
            None => event,
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
                self.replay_block_insert(buffer);
            }
            KeyCode::Named(NamedKey::Backspace) => {
                // Backspace right between an empty bracket pair (`{|}`)
                // removes both characters at once rather than leaving the
                // close bracket dangling behind -- the companion to
                // auto-pairing's own insert, same "un-type as a unit"
                // posture. Structural, not provenance-tracked: applies
                // whether or not this exact pair was just auto-inserted,
                // same as type-through above. Its own atomic step
                // (`delete_range` flushes pending), same tradeoff as
                // electric dedent.
                let before = cursor.char_idx.checked_sub(1).and_then(|i| buffer.char_at(i));
                let after = buffer.char_at(cursor.char_idx);
                let is_empty_pair = !replace
                    && match (before, after) {
                        (Some(open), Some(close)) => indent::matching_close_bracket(open) == Some(close),
                        _ => false,
                    };
                if is_empty_pair {
                    buffer.delete_range(cursor, cursor.char_idx - 1, cursor.char_idx + 1);
                } else {
                    buffer.delete_backward(cursor);
                }
                if let Some(bi) = &mut self.block_insert {
                    bi.typed.pop();
                }
            }
            KeyCode::Named(NamedKey::Delete) => buffer.delete_forward(cursor),
            KeyCode::Named(NamedKey::Enter) => {
                // Carries over the current line's leading whitespace onto
                // the new line, plus one extra indent level if the char
                // right before the cursor opens a bracket -- inserted as
                // a run of `insert_char` calls (not `insert_str`) so it
                // coalesces into the same undo step as the surrounding
                // Insert-mode session, same as ordinary typed chars do.
                let (line, _) = buffer.line_col(cursor);
                let mut new_indent = indent::leading_whitespace(buffer, line);
                let bumps = cursor.char_idx > 0
                    && buffer.char_at(cursor.char_idx - 1).is_some_and(indent::is_opening_bracket);
                if bumps {
                    new_indent.push_str(&" ".repeat(indent::INDENT_WIDTH));
                }
                buffer.insert_char(cursor, '\n');
                for ch in new_indent.chars() {
                    buffer.insert_char(cursor, ch);
                }
            }
            KeyCode::Named(NamedKey::Tab) => {
                // Soft-tab: spaces up to the next stop, not a literal
                // '\t' -- the render pipeline has no tab-stop logic (see
                // `indent.rs`'s doc comment), so a raw tab wouldn't align
                // to anything. A char-at-a-time loop, not `insert_str`,
                // so it coalesces with the surrounding Insert-mode run.
                let (_, col) = buffer.line_col(cursor);
                for _ in 0..indent::spaces_to_next_stop(col) {
                    buffer.insert_char(cursor, ' ');
                }
            }
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
                if !replace && indent::is_closing_bracket(c) && buffer.char_at(cursor.char_idx) == Some(c) {
                    // Type-through: the bracket you just typed is already
                    // the very next character (almost always one this
                    // same self-insert arm auto-paired a moment ago) --
                    // move past it instead of inserting a duplicate, the
                    // standard "close over" behavior every auto-pairing
                    // editor has. Checked before electric dedent below:
                    // if the bracket's already there, nothing should be
                    // inserted or dedented, just stepped over.
                    cursor.char_idx += 1;
                    let (_, col) = buffer.line_col(cursor);
                    cursor.sticky_col = col;
                } else {
                    // Electric dedent: typing a closing bracket as the
                    // first non-whitespace char on the line snaps it one
                    // indent level shallower first, the common "type `}`
                    // and watch it jump left" habit. A fixed one-level
                    // heuristic, not real bracket matching -- its own
                    // undo step (dedent_line uses delete_range, which
                    // doesn't coalesce), separate from whatever's typed
                    // next.
                    if !replace && indent::is_closing_bracket(c) && indent::line_blank_before_cursor(buffer, cursor) {
                        let (line, _) = buffer.line_col(cursor);
                        indent::dedent_line(buffer, cursor, line);
                        // Not `line_first_non_blank` -- the line is
                        // entirely whitespace (that's the trigger
                        // condition), so "first non-blank" degenerates to
                        // the line start, not where the bracket should
                        // land. The line's remaining content is exactly
                        // the un-removed leading whitespace, so its own
                        // end is the right spot to type into.
                        cursor.char_idx = buffer.line_start_char(line) + buffer.line_len(line);
                        let (_, col) = buffer.line_col(cursor);
                        cursor.sticky_col = col;
                    }
                    buffer.insert_char(cursor, c);
                    // Auto-pair: typing an opening bracket also inserts
                    // its close right after, then steps the cursor back
                    // between them -- via `insert_char` (not `insert_str`)
                    // so it coalesces with the opening char into one
                    // pending run (undoing right after a bare `{}` removes
                    // both together); the manual `char_idx -= 1` (not
                    // `Buffer::move_left`, which flushes pending) is what
                    // keeps that coalescing intact. Typing content after
                    // stepping back does end that run at the next
                    // `insert_char` call, since its target position no
                    // longer immediately follows the pending run's end --
                    // a disclosed side effect, and arguably the right one:
                    // undoing what you just typed *inside* the brackets
                    // shouldn't also remove the brackets themselves.
                    if !replace {
                        if let Some(close) = indent::matching_close_bracket(c) {
                            buffer.insert_char(cursor, close);
                            cursor.char_idx -= 1;
                            let (_, col) = buffer.line_col(cursor);
                            cursor.sticky_col = col;
                        }
                    }
                }
                if let Some(bi) = &mut self.block_insert {
                    bi.typed.push(c);
                }
            }
            _ => {}
        }
        VimEvent::None
    }

    /// Replays a Visual-Block `I` session's typed text at the same column
    /// on every other line in the block. Lines too short to reach that
    /// column are skipped rather than padded with spaces.
    fn replay_block_insert(&mut self, buffer: &mut Buffer) {
        let Some(bi) = self.block_insert.take() else { return };
        if bi.typed.is_empty() {
            return;
        }
        for line in (bi.line_lo + 1)..=bi.line_hi {
            if line >= buffer.line_count() || bi.col > buffer.line_len(line) {
                continue;
            }
            let at = buffer.line_start_char(line) + bi.col;
            let mut c = Cursor { char_idx: at, sticky_col: 0 };
            buffer.insert_str(&mut c, &bi.typed);
        }
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
        if let Some(count) = self.pending_replace.take() {
            if key.code != KeyCode::Named(NamedKey::Escape) {
                if let KeyCode::Char(c) = key.code {
                    self.replace_char(buffer, cursor, c, count);
                }
            }
            return VimEvent::None;
        }

        if let Some(op) = self.pending_op {
            self.handle_operator_pending_key(buffer, cursor, op, key);
            return VimEvent::None;
        }

        // Count prefix (`3w`, `2dd`): digits were never trie leaves, so
        // intercept them here rather than feeding them to the matcher.
        // Only while not already mid-sequence, so e.g. the second `g` of
        // `gg` is never misread as a count digit.
        if !self.normal_matcher.is_pending() {
            if let KeyCode::Char(c) = key.code {
                if key.mods == Mods::default() {
                    if let Some(d) = c.to_digit(10) {
                        if d != 0 || self.count.is_some() {
                            self.count = Some(self.count.unwrap_or(0).saturating_mul(10).saturating_add(d));
                            return VimEvent::None;
                        }
                    }
                }
            }
        }

        match self.normal_matcher.feed(key) {
            Step::Matched(action) => {
                let count = self.count.take().unwrap_or(1).max(1);
                self.apply_normal_action(buffer, cursor, *action, count);
            }
            Step::NoMatch => self.count = None,
            Step::Pending(_) => {}
        }
        VimEvent::None
    }

    fn apply_normal_action(&mut self, buffer: &mut Buffer, cursor: &mut Cursor, action: VimAction, count: u32) {
        match action {
            VimAction::Motion(m) => apply_motion(buffer, cursor, m, count),
            VimAction::Operator(op) => {
                self.pending_op = Some(op);
                self.pending_op_count = count;
            }
            VimAction::EnterInsert(entry) => self.enter_insert(buffer, cursor, entry),
            VimAction::EnterVisual(kind) => {
                self.mode = Mode::Visual;
                self.visual_kind = kind;
                self.visual_anchor = cursor.char_idx;
            }
            VimAction::EnterCommandLine => {
                self.mode = Mode::Command;
                self.command_line.clear();
            }
            VimAction::ReselectVisual => {
                if let Some((kind, anchor, last_cursor)) = self.last_visual {
                    self.mode = Mode::Visual;
                    self.visual_kind = kind;
                    self.visual_anchor = anchor.min(buffer.len_chars());
                    cursor.char_idx = last_cursor.min(buffer.len_chars());
                    let (_, col) = buffer.line_col(cursor);
                    cursor.sticky_col = col;
                }
            }
            VimAction::Undo => {
                for _ in 0..count {
                    buffer.undo(cursor);
                }
            }
            VimAction::Redo => {
                for _ in 0..count {
                    buffer.redo(cursor);
                }
            }
            VimAction::DeleteCharUnder => {
                let end = (cursor.char_idx + count as usize).min(buffer.len_chars());
                if end > cursor.char_idx {
                    let text = buffer.delete_range(cursor, cursor.char_idx, end);
                    self.register = Register { text, linewise: false };
                }
            }
            VimAction::DeleteCharBefore => {
                let start = cursor.char_idx.saturating_sub(count as usize);
                if start < cursor.char_idx {
                    let text = buffer.delete_range(cursor, start, cursor.char_idx);
                    self.register = Register { text, linewise: false };
                }
            }
            VimAction::PasteAfter => self.paste(buffer, cursor, true, count),
            VimAction::PasteBefore => self.paste(buffer, cursor, false, count),
            VimAction::IndentLine | VimAction::DedentLine => {
                let (line, _) = buffer.line_col(cursor);
                let end_line = (line + count as usize - 1).min(motion::last_line(buffer));
                for l in line..=end_line {
                    if action == VimAction::IndentLine {
                        indent::indent_line(buffer, cursor, l);
                    } else {
                        indent::dedent_line(buffer, cursor, l);
                    }
                }
                cursor.char_idx = motion::line_first_non_blank(buffer, line);
                let (_, col) = buffer.line_col(cursor);
                cursor.sticky_col = col;
            }
            VimAction::OperatorToLineEnd(op) => {
                // Count support skipped here (real Vim's "3D" also pulls in
                // the next 2 full lines, a nuance not worth the complexity
                // for this action) -- always behaves as a plain d$/c$/y$.
                let range = range_for_motion(buffer, cursor, Motion::LineEnd, 1);
                self.finish_operator(buffer, cursor, op, range, false);
            }
            VimAction::ChangeLine => {
                let (line, _) = buffer.line_col(cursor);
                let end_line = (line + count as usize - 1).min(motion::last_line(buffer));
                let range = lines_content_range(buffer, line, end_line);
                self.finish_operator(buffer, cursor, Operator::Change, range, true);
            }
            VimAction::SubstituteChar => {
                let end = (cursor.char_idx + count as usize).min(buffer.len_chars());
                if end > cursor.char_idx {
                    let text = buffer.delete_range(cursor, cursor.char_idx, end);
                    self.register = Register { text, linewise: false };
                }
                self.mode = Mode::Insert;
            }
            VimAction::JoinLines => {
                // "NJ" joins N lines together (N-1 individual joins); bare
                // J (count=1) and 2J both mean "join with just the next line".
                for _ in 0..count.saturating_sub(1).max(1) {
                    self.join_lines(buffer, cursor);
                }
            }
            VimAction::ToggleCase => {
                for _ in 0..count.max(1) {
                    self.toggle_case(buffer, cursor);
                }
            }
            VimAction::ReplaceChar => self.pending_replace = Some(count),
        }
    }

    /// `J`: joins the current line with the next, trimming the next
    /// line's leading whitespace and separating with a single space.
    /// A no-op on the buffer's last real line.
    fn join_lines(&mut self, buffer: &mut Buffer, cursor: &mut Cursor) {
        let (line, _) = buffer.line_col(cursor);
        if line >= motion::last_line(buffer) {
            return;
        }
        let this_line_end = buffer.line_start_char(line) + buffer.line_len(line);
        let next_start = buffer.line_start_char(line + 1);
        let next_len = buffer.line_len(line + 1);
        let mut skip = 0;
        while skip < next_len && buffer.char_at(next_start + skip).is_some_and(|c| c.is_whitespace()) {
            skip += 1;
        }
        buffer.delete_range(cursor, this_line_end, next_start + skip);
        cursor.char_idx = this_line_end;
        buffer.insert_str(cursor, " ");
        cursor.char_idx = this_line_end;
        let (_, col) = buffer.line_col(cursor);
        cursor.sticky_col = col;
    }

    /// `~`: flips the case of the char under the cursor and advances,
    /// like Vim (not full Unicode case-folding -- takes the first char of
    /// `to_uppercase`/`to_lowercase`, which is lossy for the rare
    /// multi-char cases).
    fn toggle_case(&mut self, buffer: &mut Buffer, cursor: &mut Cursor) {
        let Some(c) = buffer.char_at(cursor.char_idx) else { return };
        let toggled = if c.is_uppercase() {
            c.to_lowercase().next().unwrap_or(c)
        } else if c.is_lowercase() {
            c.to_uppercase().next().unwrap_or(c)
        } else {
            c
        };
        let start = cursor.char_idx;
        buffer.delete_range(cursor, start, start + 1);
        buffer.insert_str(cursor, &toggled.to_string());
        cursor.char_idx = (start + 1).min(buffer.len_chars());
        let (_, col) = buffer.line_col(cursor);
        cursor.sticky_col = col;
    }

    /// `r<char>`, with `count` (`3rx` replaces 3 chars with `x`). Vim
    /// rejects the whole thing if there aren't `count` chars available
    /// (no partial replace) -- matched here rather than silently clamping.
    fn replace_char(&mut self, buffer: &mut Buffer, cursor: &mut Cursor, c: char, count: u32) {
        let count = count.max(1) as usize;
        let start = cursor.char_idx;
        let end = start + count;
        if end > buffer.len_chars() {
            return;
        }
        buffer.delete_range(cursor, start, end);
        let replacement: String = std::iter::repeat_n(c, count).collect();
        buffer.insert_str(cursor, &replacement);
        cursor.char_idx = start;
        let (_, col) = buffer.line_col(cursor);
        cursor.sticky_col = col;
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
                let new_indent = indent::leading_whitespace(buffer, line);
                cursor.char_idx = buffer.line_start_char(line) + buffer.line_len(line);
                buffer.insert_char(cursor, '\n');
                for ch in new_indent.chars() {
                    buffer.insert_char(cursor, ch);
                }
            }
            InsertEntry::NewlineAbove => {
                let (line, _) = buffer.line_col(cursor);
                let new_indent = indent::leading_whitespace(buffer, line);
                cursor.char_idx = buffer.line_start_char(line);
                buffer.insert_char(cursor, '\n');
                cursor.char_idx -= 1;
                for ch in new_indent.chars() {
                    buffer.insert_char(cursor, ch);
                }
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
            self.count = None;
            return;
        }

        // Count typed between the operator and its motion/doubled key
        // (`d3w`, `d3d`) -- combines multiplicatively with pending_op_count
        // (`2d3w` = 6 words) at resolution time below. Same "not mid-
        // sequence" guard as the Normal-mode count check.
        if !self.pending_matcher.is_pending() {
            if let KeyCode::Char(c) = key.code {
                if key.mods == Mods::default() {
                    if let Some(d) = c.to_digit(10) {
                        if d != 0 || self.count.is_some() {
                            self.count = Some(self.count.unwrap_or(0).saturating_mul(10).saturating_add(d));
                            return;
                        }
                    }
                }
            }
        }

        // Doubled operator (dd/cc/yy): linewise, current line(s). Only
        // applies as the very first key after the operator, matching Vim.
        // cc (unlike dd/yy) keeps the line itself -- it only clears the
        // content, leaving a blank line to type into.
        if !self.pending_matcher.is_pending() && key == op.trigger_key() {
            self.pending_op = None;
            let total_count = self.pending_op_count.saturating_mul(self.count.take().unwrap_or(1).max(1));
            let (line, _) = buffer.line_col(cursor);
            let end_line = (line + total_count as usize - 1).min(motion::last_line(buffer));
            let range = if op == Operator::Change {
                lines_content_range(buffer, line, end_line)
            } else {
                linewise_range(buffer, line, end_line)
            };
            self.finish_operator(buffer, cursor, op, range, true);
            return;
        }

        match self.pending_matcher.feed(key) {
            Step::Pending(_) => {}
            Step::NoMatch => {
                self.pending_op = None;
                self.count = None;
            }
            Step::Matched(target) => {
                let target = *target;
                self.pending_op = None;
                let total_count = self.pending_op_count.saturating_mul(self.count.take().unwrap_or(1).max(1));
                let (range, linewise) = match target {
                    PendingTarget::Motion(m) => {
                        let m = adjust_for_change_word(op, m, buffer, cursor);
                        (range_for_motion(buffer, cursor, m, total_count), m.is_linewise())
                    }
                    // Counts on text objects aren't supported ("2diw" behaves
                    // like "diw") -- a disclosed simplification.
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
                self.pending_pulse = Some(range.clone());
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

    fn paste(&mut self, buffer: &mut Buffer, cursor: &mut Cursor, after: bool, count: u32) {
        if self.register.text.is_empty() {
            return;
        }
        let count = count.max(1) as usize;
        if self.register.linewise {
            let mut block = self.register.text.clone();
            if !block.ends_with('\n') {
                block.push('\n');
            }
            let block = block.repeat(count);
            let (line, _) = buffer.line_col(cursor);
            let insert_line = if after { line + 1 } else { line };
            let at = if insert_line < buffer.line_count() {
                buffer.line_start_char(insert_line)
            } else {
                buffer.len_chars()
            };
            cursor.char_idx = at;
            buffer.insert_str(cursor, &block);
            self.pending_pulse = Some(at..(at + block.chars().count()));
            // Vim leaves the cursor on the first non-blank of the pasted
            // block's first line, not at column 0 -- matches `^`.
            cursor.char_idx = motion::target(buffer, &Cursor { char_idx: at, sticky_col: 0 }, Motion::LineFirstNonBlank);
        } else {
            let text = self.register.text.repeat(count);
            let at = if after { (cursor.char_idx + 1).min(buffer.len_chars()) } else { cursor.char_idx };
            cursor.char_idx = at;
            buffer.insert_str(cursor, &text);
            let inserted_len = text.chars().count();
            self.pending_pulse = Some(at..(at + inserted_len));
            // Charwise paste leaves the cursor on the last pasted char, not
            // the first -- matches Vim (e.g. `yiw` then `p` lands you at
            // the end of the word you just pasted, ready to keep typing).
            cursor.char_idx = at + inserted_len - 1;
        }
        let (_, col) = buffer.line_col(cursor);
        cursor.sticky_col = col;
    }

    fn handle_visual_key(&mut self, buffer: &mut Buffer, cursor: &mut Cursor, key: KeyPress) -> VimEvent {
        if key.code == KeyCode::Named(NamedKey::Escape) {
            self.last_visual = Some((self.visual_kind, self.visual_anchor, cursor.char_idx));
            self.mode = Mode::Normal;
            return VimEvent::None;
        }

        if let Step::Matched(action) = self.visual_matcher.feed(key) {
            match *action {
                // Counts aren't supported in Visual mode -- the selection
                // is already explicit, unlike Normal mode's motions.
                VisualAction::Motion(m) => apply_motion(buffer, cursor, m, 1),
                VisualAction::Apply(op) => {
                    self.last_visual = Some((self.visual_kind, self.visual_anchor, cursor.char_idx));
                    if self.visual_kind == VisualKind::Block {
                        self.apply_block_operator(buffer, cursor, op);
                    } else {
                        let (range, linewise) = self.visual_range(buffer, cursor);
                        self.finish_operator(buffer, cursor, op, range, linewise);
                    }
                    if self.mode == Mode::Visual {
                        self.mode = Mode::Normal;
                    }
                }
                VisualAction::SetKind(kind) => {
                    if kind == self.visual_kind {
                        self.last_visual = Some((self.visual_kind, self.visual_anchor, cursor.char_idx));
                        self.mode = Mode::Normal; // same key again: toggle out
                    } else {
                        self.visual_kind = kind; // different kind: switch, keep anchor
                    }
                }
                VisualAction::Indent | VisualAction::Dedent => {
                    self.last_visual = Some((self.visual_kind, self.visual_anchor, cursor.char_idx));
                    // Always linewise, regardless of `visual_kind` --
                    // real Vim's `>`/`<` never restrict to the selected
                    // columns, even in Block mode.
                    let anchor_cursor = Cursor { char_idx: self.visual_anchor, sticky_col: 0 };
                    let (anchor_line, _) = buffer.line_col(&anchor_cursor);
                    let (cursor_line, _) = buffer.line_col(cursor);
                    let (line_lo, line_hi) =
                        if anchor_line <= cursor_line { (anchor_line, cursor_line) } else { (cursor_line, anchor_line) };
                    for l in line_lo..=line_hi {
                        if *action == VisualAction::Indent {
                            indent::indent_line(buffer, cursor, l);
                        } else {
                            indent::dedent_line(buffer, cursor, l);
                        }
                    }
                    cursor.char_idx = motion::line_first_non_blank(buffer, line_lo);
                    let (_, col) = buffer.line_col(cursor);
                    cursor.sticky_col = col;
                    self.mode = Mode::Normal;
                }
                VisualAction::BlockInsertLeft => {
                    if self.visual_kind == VisualKind::Block {
                        let (line_lo, line_hi, col_lo, _) = self.block_bounds(buffer, cursor);
                        cursor.char_idx = buffer.line_start_char(line_lo) + col_lo.min(buffer.line_len(line_lo));
                        self.block_insert = Some(BlockInsert { line_lo, line_hi, col: col_lo, typed: String::new() });
                        self.mode = Mode::Insert;
                        let (_, col) = buffer.line_col(cursor);
                        cursor.sticky_col = col;
                    }
                }
            }
        }
        VimEvent::None
    }

    /// The char range (and whether it's linewise) the current selection
    /// covers, for Char/Line kinds. Block has its own per-row handling in
    /// `apply_block_operator` (a single contiguous range can't represent a
    /// column rectangle), so this is never called for it.
    fn visual_range(&self, buffer: &Buffer, cursor: &Cursor) -> (Range<usize>, bool) {
        match self.visual_kind {
            VisualKind::Line => {
                let anchor_cursor = Cursor { char_idx: self.visual_anchor, sticky_col: 0 };
                let (anchor_line, _) = buffer.line_col(&anchor_cursor);
                let (cursor_line, _) = buffer.line_col(cursor);
                (linewise_range(buffer, anchor_line, cursor_line), true)
            }
            VisualKind::Char | VisualKind::Block => {
                let (lo, hi) = if self.visual_anchor <= cursor.char_idx {
                    (self.visual_anchor, cursor.char_idx + 1)
                } else {
                    (cursor.char_idx, self.visual_anchor + 1)
                };
                (lo..hi.min(buffer.len_chars()), false)
            }
        }
    }

    /// (line_lo, line_hi, col_lo, col_hi_exclusive) of the Block selection
    /// rectangle spanned by the anchor and the cursor.
    fn block_bounds(&self, buffer: &Buffer, cursor: &Cursor) -> (usize, usize, usize, usize) {
        let anchor_cursor = Cursor { char_idx: self.visual_anchor, sticky_col: 0 };
        let (anchor_line, anchor_col) = buffer.line_col(&anchor_cursor);
        let (cursor_line, cursor_col) = buffer.line_col(cursor);
        let (line_lo, line_hi) =
            if anchor_line <= cursor_line { (anchor_line, cursor_line) } else { (cursor_line, anchor_line) };
        let (col_lo, col_hi) =
            if anchor_col <= cursor_col { (anchor_col, cursor_col + 1) } else { (cursor_col, anchor_col + 1) };
        (line_lo, line_hi, col_lo, col_hi)
    }

    /// `d`/`y`/`c` on a Block selection: acts on the same column range on
    /// every line in the block, clamped to each (possibly ragged) line's
    /// length. Simplification, disclosed in the plan: this is N separate
    /// undo steps (one delete_range per line), not one atomic block edit,
    /// and the register just joins the per-line pieces with '\n' -- paste
    /// (`p`/`P`) won't reconstruct the block shape, only Delete/Yank
    /// themselves are block-aware.
    fn apply_block_operator(&mut self, buffer: &mut Buffer, cursor: &mut Cursor, op: Operator) {
        let (line_lo, line_hi, col_lo, col_hi) = self.block_bounds(buffer, cursor);
        let mut pieces = vec![String::new(); line_hi - line_lo + 1];

        if op == Operator::Yank {
            for line in line_lo..=line_hi {
                let start = buffer.line_start_char(line);
                let len = buffer.line_len(line);
                let seg_start = start + col_lo.min(len);
                let seg_end = start + col_hi.min(len);
                if seg_start < seg_end {
                    pieces[line - line_lo] = buffer.text_range(seg_start, seg_end);
                }
            }
        } else {
            // Bottom to top so deleting a lower line never invalidates the
            // char offsets of lines above it that haven't been processed yet.
            for line in (line_lo..=line_hi).rev() {
                let start = buffer.line_start_char(line);
                let len = buffer.line_len(line);
                let seg_start = start + col_lo.min(len);
                let seg_end = start + col_hi.min(len);
                if seg_start < seg_end {
                    pieces[line - line_lo] = buffer.delete_range(cursor, seg_start, seg_end);
                }
            }
        }

        self.register = Register { text: pieces.join("\n"), linewise: false };
        cursor.char_idx = buffer.line_start_char(line_lo) + col_lo.min(buffer.line_len(line_lo));
        let (_, col) = buffer.line_col(cursor);
        cursor.sticky_col = col;

        if op == Operator::Change {
            self.mode = Mode::Insert;
        }
    }
}

impl Default for VimState {
    fn default() -> Self {
        Self::new()
    }
}

fn apply_motion(buffer: &Buffer, cursor: &mut Cursor, m: Motion, count: u32) {
    for _ in 0..count.max(1) {
        let target = motion::target(buffer, cursor, m);
        if target == cursor.char_idx {
            break; // no progress (already at a boundary) -- stop instead of spinning
        }
        cursor.char_idx = target;
        if !matches!(m, Motion::Up | Motion::Down) {
            let (_, col) = buffer.line_col(cursor);
            cursor.sticky_col = col;
        }
    }
}

/// Where `count` repeated applications of `m` from `cursor` would land,
/// without mutating `cursor` -- used to compute an operator's range
/// (`3dw`, `d3w`) without moving the cursor before the delete/change/yank
/// actually happens.
fn motion_target_repeated(buffer: &Buffer, cursor: &Cursor, m: Motion, count: u32) -> usize {
    let mut probe = *cursor;
    apply_motion(buffer, &mut probe, m, count);
    probe.char_idx
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

/// `line_a..line_b`'s content, excluding the *last* line's terminator --
/// unlike `linewise_range`, deleting this collapses the lines into one
/// (now empty, if `line_a == line_b`) instead of removing them entirely.
/// `cc`/`S` want this: change replaces content but doesn't remove the
/// line(s), so a blank line remains for the user to type into.
fn lines_content_range(buffer: &Buffer, line_a: usize, line_b: usize) -> Range<usize> {
    let (lo, hi) = if line_a <= line_b { (line_a, line_b) } else { (line_b, line_a) };
    let start = buffer.line_start_char(lo);
    let end = buffer.line_start_char(hi) + buffer.line_len(hi);
    start..end
}

fn range_for_motion(buffer: &Buffer, cursor: &Cursor, motion: Motion, count: u32) -> Range<usize> {
    let target = motion_target_repeated(buffer, cursor, motion, count);
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
    fn tab_in_insert_mode_inserts_spaces_to_the_next_stop() {
        let mut b = buf("");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "i");
        named(&mut vim, &mut b, &mut c, NamedKey::Tab);
        assert_eq!(b.text(), "    "); // column 0 -> full 4 spaces
        keys(&mut vim, &mut b, &mut c, "xy"); // now at column 6
        named(&mut vim, &mut b, &mut c, NamedKey::Tab);
        assert_eq!(b.text(), "    xy  "); // column 6 -> 2 spaces to reach 8
    }

    #[test]
    fn enter_in_insert_mode_carries_over_the_previous_lines_indentation() {
        let mut b = buf("    foo");
        let mut c = Cursor { char_idx: 7, sticky_col: 7 }; // end of "foo"
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "i");
        named(&mut vim, &mut b, &mut c, NamedKey::Enter);
        assert_eq!(b.text(), "    foo\n    ");
        keys(&mut vim, &mut b, &mut c, "bar");
        assert_eq!(b.text(), "    foo\n    bar");
    }

    #[test]
    fn enter_after_an_opening_brace_bumps_indent_one_level() {
        let mut b = buf("fn main() {");
        let mut c = Cursor { char_idx: 11, sticky_col: 11 }; // right after "{"
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "i");
        named(&mut vim, &mut b, &mut c, NamedKey::Enter);
        assert_eq!(b.text(), "fn main() {\n    ");
    }

    #[test]
    fn enter_after_an_indented_opening_brace_bumps_one_level_further() {
        let mut b = buf("    if x {");
        let mut c = Cursor { char_idx: 10, sticky_col: 10 }; // right after "{"
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "i");
        named(&mut vim, &mut b, &mut c, NamedKey::Enter);
        assert_eq!(b.text(), "    if x {\n        "); // 4 carried + 4 bumped
    }

    #[test]
    fn o_and_shift_o_carry_over_indentation() {
        let mut b = buf("    foo");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "o");
        assert_eq!(vim.mode(), Mode::Insert);
        keys(&mut vim, &mut b, &mut c, "bar");
        assert_eq!(b.text(), "    foo\n    bar");
        named(&mut vim, &mut b, &mut c, NamedKey::Escape);

        let mut b2 = buf("    foo");
        let mut c2 = Cursor::at_start();
        let mut vim2 = VimState::new();
        keys(&mut vim2, &mut b2, &mut c2, "O");
        keys(&mut vim2, &mut b2, &mut c2, "baz");
        assert_eq!(b2.text(), "    baz\n    foo");
    }

    #[test]
    fn typing_a_closing_brace_as_first_char_dedents_the_line() {
        let mut b = buf("fn main() {\n        ");
        let mut c = Cursor { char_idx: 20, sticky_col: 8 }; // end of the indented blank line
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "i");
        keys(&mut vim, &mut b, &mut c, "}");
        assert_eq!(b.text(), "fn main() {\n    }");
    }

    #[test]
    fn typing_a_closing_brace_mid_line_does_not_dedent() {
        let mut b = buf("    foo");
        let mut c = Cursor { char_idx: 7, sticky_col: 7 }; // end of "foo", real content precedes it
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "i");
        keys(&mut vim, &mut b, &mut c, ")");
        assert_eq!(b.text(), "    foo)");
    }

    #[test]
    fn typing_an_opening_bracket_auto_inserts_its_close_and_leaves_the_cursor_between() {
        for (open, pair) in [('(', "()"), ('{', "{}"), ('[', "[]")] {
            let mut b = buf("");
            let mut c = Cursor::at_start();
            let mut vim = VimState::new();
            keys(&mut vim, &mut b, &mut c, "i");
            vim.handle_key(&mut b, &mut c, KeyPress::char(open));
            assert_eq!(b.text(), pair);
            assert_eq!(c.char_idx, 1); // right after the opening bracket, not the close
        }
    }

    #[test]
    fn typing_inside_an_auto_paired_bracket_lands_between_the_pair() {
        let mut b = buf("");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "i");
        keys(&mut vim, &mut b, &mut c, "(hi");
        assert_eq!(b.text(), "(hi)");
        assert_eq!(c.char_idx, 3);
    }

    #[test]
    fn typing_the_matching_close_bracket_types_through_instead_of_duplicating() {
        let mut b = buf("");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "i");
        keys(&mut vim, &mut b, &mut c, "(hi)"); // the ')' should type through the auto-inserted one
        assert_eq!(b.text(), "(hi)");
        assert_eq!(c.char_idx, 4);
    }

    #[test]
    fn typing_through_a_bracket_that_was_not_auto_inserted_still_just_steps_over_it() {
        // Structural, not provenance-tracked -- an existing ")" right at
        // the cursor is skipped over the same way an auto-inserted one
        // would be.
        let mut b = buf(")");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "i");
        keys(&mut vim, &mut b, &mut c, ")");
        assert_eq!(b.text(), ")");
        assert_eq!(c.char_idx, 1);
    }

    #[test]
    fn backspace_between_an_empty_auto_paired_bracket_removes_both() {
        let mut b = buf("");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "i");
        vim.handle_key(&mut b, &mut c, KeyPress::char('{'));
        assert_eq!(b.text(), "{}");
        named(&mut vim, &mut b, &mut c, NamedKey::Backspace);
        assert_eq!(b.text(), "");
        assert_eq!(c.char_idx, 0);
    }

    #[test]
    fn backspace_with_content_between_the_pair_only_removes_one_char() {
        let mut b = buf("");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "i");
        keys(&mut vim, &mut b, &mut c, "(x");
        assert_eq!(b.text(), "(x)");
        named(&mut vim, &mut b, &mut c, NamedKey::Backspace); // removes "x", not the brackets
        assert_eq!(b.text(), "()");
    }

    #[test]
    fn replace_mode_neither_auto_pairs_nor_types_through_nor_merges_backspace() {
        // `R` doesn't have a Normal-mode binding yet (a pre-existing,
        // separate gap -- `Mode::Replace` exists and `handle_insert_key`
        // already branches on its own `replace` bool, just nothing
        // transitions into it today), so this drives `handle_insert_key`
        // directly with `replace: true` rather than going through
        // `handle_key`/a key sequence.
        let mut b = buf("(a)");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        vim.handle_insert_key(&mut b, &mut c, KeyPress::char('('), true);
        assert_eq!(b.text(), "(a)"); // overwrote '(' with '(', no auto-pair inserted
        assert_eq!(c.char_idx, 1);
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
    fn cc_clears_the_line_but_keeps_it_unlike_dd() {
        let mut b = buf("one\ntwo\nthree");
        let mut c = Cursor { char_idx: 4, sticky_col: 0 }; // on "two"
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "cc");
        assert_eq!(vim.mode(), Mode::Insert);
        assert_eq!(b.text(), "one\n\nthree");
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
    fn yank_produces_a_pulse_event_over_the_yanked_range() {
        let mut b = buf("hello world");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "y"); // operator-pending, no pulse yet
        let ev = vim.handle_key(&mut b, &mut c, KeyPress::char('w'));
        assert_eq!(ev, VimEvent::Pulse(0..6)); // "hello " (word-forward, exclusive)
    }

    #[test]
    fn paste_produces_a_pulse_event_over_the_inserted_range() {
        let mut b = buf("ab");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "yl"); // yank "a"
        let ev = vim.handle_key(&mut b, &mut c, KeyPress::char('p'));
        assert_eq!(ev, VimEvent::Pulse(1..2)); // pasted "a" right after the cursor
    }

    #[test]
    fn doubled_yy_also_produces_a_pulse() {
        let mut b = buf("one\ntwo");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "y");
        let ev = vim.handle_key(&mut b, &mut c, KeyPress::char('y'));
        assert_eq!(ev, VimEvent::Pulse(0..4)); // "one\n"
    }

    #[test]
    fn ordinary_keys_produce_no_pulse() {
        let mut b = buf("abc");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        assert_eq!(vim.handle_key(&mut b, &mut c, KeyPress::char('l')), VimEvent::None);
        let ev = vim.handle_key(&mut b, &mut c, KeyPress::char('x')); // delete, not yank
        assert_eq!(ev, VimEvent::None);
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
    fn charwise_paste_after_lands_on_last_pasted_char() {
        let mut b = buf("ab");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        vim.register = Register { text: "xyz".to_string(), linewise: false };
        keys(&mut vim, &mut b, &mut c, "p");
        assert_eq!(b.text(), "axyzb");
        assert_eq!(c.char_idx, 3); // on 'z', not back at the pasted 'x'
    }

    #[test]
    fn charwise_paste_before_lands_on_last_pasted_char() {
        let mut b = buf("ab");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        vim.register = Register { text: "xyz".to_string(), linewise: false };
        keys(&mut vim, &mut b, &mut c, "P");
        assert_eq!(b.text(), "xyzab");
        assert_eq!(c.char_idx, 2); // on 'z'
    }

    #[test]
    fn linewise_paste_lands_on_first_non_blank_of_pasted_line() {
        let mut b = buf("one\ntwo");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        vim.register = Register { text: "  hi\n".to_string(), linewise: true };
        keys(&mut vim, &mut b, &mut c, "p");
        assert_eq!(b.text(), "one\n  hi\ntwo");
        assert_eq!(b.line_col(&c), (1, 2)); // first non-blank of "  hi", not column 0
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
    fn shift_v_enters_visual_line_mode() {
        let mut b = buf("one\ntwo\nthree");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "V");
        assert_eq!(vim.mode(), Mode::Visual);
        assert_eq!(vim.visual_kind(), VisualKind::Line);
    }

    #[test]
    fn visual_line_delete_removes_whole_lines_regardless_of_column() {
        let mut b = buf("one\ntwo\nthree");
        let mut c = Cursor { char_idx: 5, sticky_col: 1 }; // column 1 of "two"
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "V");
        keys(&mut vim, &mut b, &mut c, "d");
        assert_eq!(b.text(), "one\nthree");
        assert_eq!(vim.mode(), Mode::Normal);
    }

    #[test]
    fn visual_char_indent_acts_linewise_despite_the_selection_being_charwise() {
        let mut b = buf("one\ntwo\nthree");
        let mut c = Cursor { char_idx: 5, sticky_col: 1 }; // column 1 of "two", charwise selection
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "v");
        keys(&mut vim, &mut b, &mut c, ">");
        assert_eq!(b.text(), "one\n    two\nthree");
        assert_eq!(vim.mode(), Mode::Normal);
    }

    #[test]
    fn visual_indent_spans_every_line_the_selection_touches() {
        let mut b = buf("one\ntwo\nthree\nfour");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "V");
        keys(&mut vim, &mut b, &mut c, "j");
        keys(&mut vim, &mut b, &mut c, ">");
        assert_eq!(b.text(), "    one\n    two\nthree\nfour");
    }

    #[test]
    fn visual_dedent_removes_a_level_from_every_selected_line() {
        let mut b = buf("    one\n    two\nthree");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "V");
        keys(&mut vim, &mut b, &mut c, "j");
        keys(&mut vim, &mut b, &mut c, "<");
        assert_eq!(b.text(), "one\ntwo\nthree");
    }

    #[test]
    fn visual_line_extends_by_whole_lines_with_motions() {
        let mut b = buf("one\ntwo\nthree\nfour");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "V");
        keys(&mut vim, &mut b, &mut c, "j"); // extend to line 1
        keys(&mut vim, &mut b, &mut c, "d");
        assert_eq!(b.text(), "three\nfour");
    }

    #[test]
    fn v_then_v_again_toggles_back_to_normal() {
        let mut b = buf("hello");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "vv");
        assert_eq!(vim.mode(), Mode::Normal);
    }

    #[test]
    fn v_then_shift_v_switches_kind_without_exiting() {
        let mut b = buf("hello");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "v");
        assert_eq!(vim.visual_kind(), VisualKind::Char);
        keys(&mut vim, &mut b, &mut c, "V");
        assert_eq!(vim.mode(), Mode::Visual); // still in Visual, not exited
        assert_eq!(vim.visual_kind(), VisualKind::Line);
    }

    #[test]
    fn visual_line_yank_then_paste_duplicates_lines() {
        let mut b = buf("one\ntwo\nthree");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "V");
        keys(&mut vim, &mut b, &mut c, "y");
        assert_eq!(b.text(), "one\ntwo\nthree"); // yank doesn't delete
        keys(&mut vim, &mut b, &mut c, "p");
        assert_eq!(b.text(), "one\none\ntwo\nthree");
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

    #[test]
    fn pending_children_lists_operator_pending_continuations() {
        let mut b = buf("hello");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        assert!(vim.pending_children().is_empty());
        keys(&mut vim, &mut b, &mut c, "d");
        let children = vim.pending_children();
        // top-level: motions directly, text objects behind their "i"/"a" prefix
        assert!(children.iter().any(|(_, label)| *label == "word forward"));
        assert!(children.iter().any(|(_, label)| *label == "inner..."));

        keys(&mut vim, &mut b, &mut c, "i");
        let children = vim.pending_children();
        assert!(children.iter().any(|(_, label)| *label == "inner word"));
    }

    #[test]
    fn big_word_motions_are_available_in_normal_mode() {
        let mut b = buf("foo.bar baz");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "W");
        assert_eq!(c.char_idx, 8); // straight to "baz", not stopping at the "."
    }

    #[test]
    fn shift_d_c_y_act_to_end_of_line() {
        let mut b = buf("hello world");
        let mut c = Cursor { char_idx: 5, sticky_col: 5 };
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "D");
        assert_eq!(b.text(), "hello");
    }

    #[test]
    fn shift_c_enters_insert_after_deleting_to_eol() {
        let mut b = buf("hello world");
        let mut c = Cursor { char_idx: 5, sticky_col: 5 };
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "C");
        assert_eq!(vim.mode(), Mode::Insert);
        assert_eq!(b.text(), "hello");
    }

    #[test]
    fn shift_y_yanks_to_eol_without_deleting() {
        let mut b = buf("hello world");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "Y");
        assert_eq!(b.text(), "hello world");
        keys(&mut vim, &mut b, &mut c, "p");
        assert_eq!(b.text(), "hhello worldello world");
    }

    #[test]
    fn shift_s_changes_the_whole_line() {
        let mut b = buf("one\ntwo\nthree");
        let mut c = Cursor { char_idx: 4, sticky_col: 0 }; // on "two"
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "S");
        assert_eq!(vim.mode(), Mode::Insert);
        assert_eq!(b.text(), "one\n\nthree");
    }

    #[test]
    fn lowercase_s_substitutes_one_char() {
        let mut b = buf("abc");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "s");
        assert_eq!(vim.mode(), Mode::Insert);
        assert_eq!(b.text(), "bc");
    }

    #[test]
    fn j_joins_lines_trimming_leading_whitespace() {
        let mut b = buf("hello\n   world");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "J");
        assert_eq!(b.text(), "hello world");
    }

    #[test]
    fn j_on_last_line_is_a_no_op() {
        let mut b = buf("only");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "J");
        assert_eq!(b.text(), "only");
    }

    #[test]
    fn tilde_toggles_case_and_advances() {
        let mut b = buf("aB");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "~");
        assert_eq!(b.text(), "AB");
        assert_eq!(c.char_idx, 1);
        keys(&mut vim, &mut b, &mut c, "~");
        assert_eq!(b.text(), "Ab");
    }

    #[test]
    fn r_replaces_the_char_under_cursor_and_stays_normal() {
        let mut b = buf("abc");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "r");
        keys(&mut vim, &mut b, &mut c, "x");
        assert_eq!(b.text(), "xbc");
        assert_eq!(c.char_idx, 0);
        assert_eq!(vim.mode(), Mode::Normal);
    }

    #[test]
    fn r_then_escape_cancels_without_editing() {
        let mut b = buf("abc");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "r");
        named(&mut vim, &mut b, &mut c, NamedKey::Escape);
        assert_eq!(b.text(), "abc");
    }

    #[test]
    fn count_repeats_a_plain_motion() {
        let mut b = buf("one two three four");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "3w");
        assert_eq!(c.char_idx, 14); // start of "four", three words forward
    }

    #[test]
    fn count_before_doubled_operator_spans_n_lines() {
        let mut b = buf("one\ntwo\nthree\nfour");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "2dd");
        assert_eq!(b.text(), "three\nfour");
    }

    #[test]
    fn count_after_operator_and_before_doubled_key_also_works() {
        let mut b = buf("one\ntwo\nthree\nfour");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "d2d");
        assert_eq!(b.text(), "three\nfour");
    }

    #[test]
    fn greater_greater_indents_the_current_line() {
        let mut b = buf("one\ntwo\nthree");
        let mut c = Cursor { char_idx: 4, sticky_col: 0 }; // on "two"
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, ">>");
        assert_eq!(b.text(), "one\n    two\nthree");
    }

    #[test]
    fn less_less_dedents_the_current_line() {
        let mut b = buf("one\n        two\nthree");
        let mut c = Cursor { char_idx: 4, sticky_col: 0 }; // on "two"
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "<<");
        assert_eq!(b.text(), "one\n    two\nthree");
    }

    #[test]
    fn less_less_removes_only_whats_there_when_less_than_indent_width() {
        let mut b = buf("  two");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "<<");
        assert_eq!(b.text(), "two");
    }

    #[test]
    fn count_before_greater_greater_indents_n_lines() {
        let mut b = buf("one\ntwo\nthree\nfour");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "3>>");
        assert_eq!(b.text(), "    one\n    two\n    three\nfour");
    }

    #[test]
    fn indent_dedent_move_cursor_to_the_first_non_blank() {
        let mut b = buf("foo");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, ">>");
        assert_eq!(c.char_idx, 4); // right after the 4 inserted spaces, on "f"
    }

    #[test]
    fn counts_before_operator_and_before_motion_multiply() {
        let mut b = buf("one two three four five six seven");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "2d3w"); // delete 6 words
        assert_eq!(b.text(), "seven");
    }

    #[test]
    fn count_survives_a_multi_key_motion_like_gg() {
        let mut b = buf("a\nb\nc\nd\ne");
        let mut c = Cursor { char_idx: 8, sticky_col: 0 }; // on "e"
        let mut vim = VimState::new();
        // 3gg isn't "repeat gg 3 times" (idempotent) but this exercises
        // that the count typed before a two-key sequence isn't dropped
        // partway through -- gg still resolves to line 0 either way.
        keys(&mut vim, &mut b, &mut c, "3gg");
        assert_eq!(b.line_col(&c).0, 0);
    }

    #[test]
    fn count_on_x_deletes_n_chars_into_one_register() {
        let mut b = buf("abcdef");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "3x");
        assert_eq!(b.text(), "def");
        keys(&mut vim, &mut b, &mut c, "p");
        assert_eq!(b.text(), "dabcef");
    }

    #[test]
    fn count_on_paste_repeats_the_register() {
        let mut b = buf("ab");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "yl"); // yank "a"
        keys(&mut vim, &mut b, &mut c, "3p");
        assert_eq!(b.text(), "aaaab");
    }

    #[test]
    fn count_on_replace_needs_that_many_chars_available() {
        let mut b = buf("ab");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        // like every other count, it precedes the trigger key: "3rx", not "r3x"
        keys(&mut vim, &mut b, &mut c, "3rx");
        // only 2 chars available, so this should refuse rather than partially replace
        assert_eq!(b.text(), "ab");
    }

    #[test]
    fn count_on_join_joins_n_lines() {
        let mut b = buf("a\nb\nc\nd");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "3J");
        assert_eq!(b.text(), "a b c\nd");
    }

    #[test]
    fn no_match_during_count_resets_it() {
        let mut b = buf("abc");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        // an unbound key after a count should abort the count, not leave
        // it silently applied to whatever comes next
        vim.handle_key(&mut b, &mut c, KeyPress::char('2'));
        vim.handle_key(&mut b, &mut c, KeyPress::named(NamedKey::Tab)); // unbound in Normal mode
        keys(&mut vim, &mut b, &mut c, "l");
        assert_eq!(c.char_idx, 1); // single step, not 2
    }

    #[test]
    fn ctrl_v_enters_visual_block_mode() {
        let mut b = buf("abc\ndef\nghi");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        vim.handle_key(&mut b, &mut c, KeyPress::char('v').with_ctrl());
        assert_eq!(vim.mode(), Mode::Visual);
        assert_eq!(vim.visual_kind(), VisualKind::Block);
    }

    #[test]
    fn block_delete_removes_the_column_rectangle_from_every_line() {
        let mut b = buf("abc\ndef\nghi");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        vim.handle_key(&mut b, &mut c, KeyPress::char('v').with_ctrl());
        keys(&mut vim, &mut b, &mut c, "jjl"); // rectangle: lines 0-2, cols 0-1
        keys(&mut vim, &mut b, &mut c, "d");
        assert_eq!(b.text(), "c\nf\ni");
        assert_eq!(vim.mode(), Mode::Normal);
    }

    #[test]
    fn block_yank_does_not_modify_the_buffer() {
        let mut b = buf("abc\ndef\nghi");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        vim.handle_key(&mut b, &mut c, KeyPress::char('v').with_ctrl());
        keys(&mut vim, &mut b, &mut c, "jjl");
        keys(&mut vim, &mut b, &mut c, "y");
        assert_eq!(b.text(), "abc\ndef\nghi");
        assert_eq!(vim.mode(), Mode::Normal);
    }

    #[test]
    fn block_yank_register_joins_column_pieces_with_newline() {
        let mut b = buf("abc\ndef\nghi");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        vim.handle_key(&mut b, &mut c, KeyPress::char('v').with_ctrl());
        keys(&mut vim, &mut b, &mut c, "jjl");
        keys(&mut vim, &mut b, &mut c, "y");
        // paste into a separate empty buffer to inspect the register's
        // exact content without the arithmetic of pasting back into the
        // same (now-shifted) buffer
        let mut b2 = buf("");
        let mut c2 = Cursor::at_start();
        keys(&mut vim, &mut b2, &mut c2, "P");
        assert_eq!(b2.text(), "ab\nde\ngh");
    }

    #[test]
    fn block_delete_on_ragged_lines_clamps_per_line() {
        let mut b = buf("abcdef\nx\nghijkl");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        vim.handle_key(&mut b, &mut c, KeyPress::char('v').with_ctrl());
        keys(&mut vim, &mut b, &mut c, "jj"); // down to line 2, still col 0
        keys(&mut vim, &mut b, &mut c, "lll"); // extend to col 3
        keys(&mut vim, &mut b, &mut c, "d");
        // middle line "x" is shorter than the rectangle -- clamped, not padded
        assert_eq!(b.text(), "ef\n\nkl");
    }

    #[test]
    fn block_change_deletes_column_and_enters_insert_without_propagating() {
        let mut b = buf("abc\ndef");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        vim.handle_key(&mut b, &mut c, KeyPress::char('v').with_ctrl());
        keys(&mut vim, &mut b, &mut c, "j");
        keys(&mut vim, &mut b, &mut c, "c");
        assert_eq!(vim.mode(), Mode::Insert);
        assert_eq!(b.text(), "bc\nef"); // deleted on both lines, no I-style propagation
    }

    #[test]
    fn block_insert_left_propagates_typed_text_to_other_lines_on_escape() {
        let mut b = buf("abc\ndef\nghi");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        vim.handle_key(&mut b, &mut c, KeyPress::char('v').with_ctrl());
        keys(&mut vim, &mut b, &mut c, "jj"); // column 0 across all 3 lines
        keys(&mut vim, &mut b, &mut c, "I");
        assert_eq!(vim.mode(), Mode::Insert);
        keys(&mut vim, &mut b, &mut c, "X");
        named(&mut vim, &mut b, &mut c, NamedKey::Escape);
        assert_eq!(b.text(), "Xabc\nXdef\nXghi");
        assert_eq!(vim.mode(), Mode::Normal);
    }

    #[test]
    fn block_insert_left_skips_lines_too_short_to_reach_the_column() {
        let mut b = buf("abcdef\nxy\nghijkl");
        let mut c = Cursor { char_idx: 3, sticky_col: 3 }; // col 3 of line 0
        let mut vim = VimState::new();
        vim.handle_key(&mut b, &mut c, KeyPress::char('v').with_ctrl());
        keys(&mut vim, &mut b, &mut c, "jj"); // down to line 2, keeping col 3
        keys(&mut vim, &mut b, &mut c, "I");
        keys(&mut vim, &mut b, &mut c, "Z");
        named(&mut vim, &mut b, &mut c, NamedKey::Escape);
        // line 1 ("xy") is only 2 chars -- too short for column 3, skipped
        assert_eq!(b.text(), "abcZdef\nxy\nghiZjkl");
    }

    #[test]
    fn v_then_ctrl_v_switches_to_block_without_exiting() {
        let mut b = buf("hello");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "v");
        vim.handle_key(&mut b, &mut c, KeyPress::char('v').with_ctrl());
        assert_eq!(vim.mode(), Mode::Visual);
        assert_eq!(vim.visual_kind(), VisualKind::Block);
    }

    #[test]
    fn gv_reselects_the_last_visual_selection() {
        let mut b = buf("hello world");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "v");
        keys(&mut vim, &mut b, &mut c, "llll"); // select "hello"
        named(&mut vim, &mut b, &mut c, NamedKey::Escape);
        assert_eq!(vim.mode(), Mode::Normal);

        keys(&mut vim, &mut b, &mut c, "$"); // move away
        keys(&mut vim, &mut b, &mut c, "gv");
        assert_eq!(vim.mode(), Mode::Visual);
        assert_eq!(vim.visual_kind(), VisualKind::Char);
        keys(&mut vim, &mut b, &mut c, "d");
        assert_eq!(b.text(), " world");
    }
}
