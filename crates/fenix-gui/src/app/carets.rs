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

/// The carets around an edit made at several: the primary, then the
/// extras, before it and after it.
#[derive(Debug, Clone)]
pub(crate) struct CaretStep {
    before: (Caret, Vec<Caret>),
    after: (Caret, Vec<Caret>),
}

/// Edits made at several carets, by their undo step's tag.
#[derive(Debug, Default)]
pub(crate) struct CaretSteps {
    steps: HashMap<u64, CaretStep>,
    next: u64,
}

/// Where the cursors were, for `edit.cursor_undo` (Ctrl+U) to go back
/// to: in a buffer and pane, as of an edit count (an edit since makes the
/// positions stale).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CaretMark {
    buffer: BufferId,
    pane: fenix_window::WindowId,
    edits: u64,
    primary: Caret,
    extra: Vec<Caret>,
}

/// The cursor moves `edit.cursor_undo` keeps.
const KEPT_CARET_MARKS: usize = 100;

/// How many such edits keep their carets for undo: past it, the oldest
/// undo to one cursor, as any other edit does.
const KEPT_CARET_STEPS: u64 = 1_000;

impl App {
    /// The focused pane's extra carets, if they still hold.
    pub(crate) fn extra_carets(&self) -> &[Caret] {
        let pane = self.focused_pane_id();
        let id = self.focused_buffer_id();
        let Some(carets) = self.carets.get(&pane) else { return &[] };
        let edits = self.buffers.get(id).map(|ob| ob.buffer.edit_count());
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
        let edits = self.buffers.get(id).map_or(0, |ob| ob.buffer.edit_count());
        self.carets.insert(pane, Carets { buffer: Some(id), extra, edits });
    }

    /// Alt+click (or Ctrl+click, `editor.multi_cursor_modifier`): a caret at
    /// `at`, or, on one, that one taken away.
    pub(crate) fn toggle_caret_at(&mut self, at: usize) {
        let before = self.caret_mark();
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
        self.note_caret_move(before);
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
        let before = (carets.last().map(|(c, _)| *c).unwrap_or(Caret { at: 0, sticky: 0, anchor: None }), self.extra_carets().to_vec());
        carets.sort_by_key(|(c, _)| std::cmp::Reverse(c.at));
        let edits_before = self.open().buffer.edit_count();
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
        let primary = carets.iter().find(|(_, p)| *p).map(|(c, _)| *c).unwrap_or(Caret { at: 0, sticky: 0, anchor: None });
        let extra: Vec<Caret> = carets.into_iter().filter(|(_, p)| !*p).map(|(c, _)| c).collect();
        self.put_carets(primary, extra);
        // An edit: its undo step remembers the carets around it.
        if self.open().buffer.edit_count() != edits_before {
            let after = (primary, self.extra_carets().to_vec());
            let steps = &mut self.caret_steps;
            steps.next += 1;
            let tag = steps.next;
            if self.focused_buffer_and_cursor_mut().0.tag_last_undo_step(tag) {
                let steps = &mut self.caret_steps;
                steps.steps.insert(tag, CaretStep { before, after });
                steps.steps.remove(&tag.saturating_sub(KEPT_CARET_STEPS));
            }
        }
    }

    /// `primary` as the cursor, in its mode, and `extra` around it; carets
    /// that met are one.
    fn put_carets(&mut self, primary: Caret, mut extra: Vec<Caret>) {
        let pane = self.focused_pane_id();
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
        extra.retain(|c| c.at != primary.at);
        self.set_extra_carets(extra);
    }

    /// After an undo (or a redo) of the step tagged `tag`: when it was
    /// made at several carets, those carets come back -- as they were
    /// before it for an undo, after it for a redo.
    pub(crate) fn carets_after_undo(&mut self, tag: u64, undo: bool) {
        let Some(step) = self.caret_steps.steps.get(&tag).cloned() else { return };
        let (primary, extra) = if undo { step.before } else { step.after };
        let len = self.open().buffer.len_chars();
        let fit = |c: Caret| Caret { at: c.at.min(len), anchor: c.anchor.map(|a| a.min(len)), ..c };
        self.put_carets(fit(primary), extra.into_iter().map(fit).collect());
    }

    /// Where the cursors are now, for `edit.cursor_undo`.
    pub(crate) fn caret_mark(&self) -> Option<CaretMark> {
        let buffer = self.focused_buffer_id();
        let edits = self.buffers.get(buffer)?.buffer.edit_count();
        Some(CaretMark { buffer, pane: self.focused_pane_id(), edits, primary: self.primary_caret(), extra: self.extra_carets().to_vec() })
    }

    /// After a key (or a click) that may have moved the cursors: where
    /// they were, kept for Ctrl+U when they moved and nothing was edited.
    /// An edit makes what's kept for that buffer stale, so it goes.
    pub(crate) fn note_caret_move(&mut self, before: Option<CaretMark>) {
        if std::mem::take(&mut self.caret_undoing) {
            return;
        }
        let (Some(before), Some(now)) = (before, self.caret_mark()) else { return };
        if before.edits != now.edits {
            self.caret_marks.retain(|m| m.buffer != now.buffer);
            return;
        }
        if before.buffer != now.buffer || before.pane != now.pane || (before.primary, &before.extra) == (now.primary, &now.extra) {
            return;
        }
        self.caret_marks.push(before);
        if self.caret_marks.len() > KEPT_CARET_MARKS {
            self.caret_marks.remove(0);
        }
    }

    /// `edit.cursor_undo` (Ctrl+U): the cursors back where they were
    /// before the last move, or before the last cursor was added.
    pub(crate) fn cmd_cursor_undo(&mut self) {
        let Some(now) = self.caret_mark() else { return };
        while let Some(mark) = self.caret_marks.pop() {
            if mark.buffer != now.buffer || mark.pane != now.pane || mark.edits != now.edits || (mark.primary, &mark.extra) == (now.primary, &now.extra) {
                continue;
            }
            self.caret_undoing = true;
            let len = self.open().buffer.len_chars();
            let fit = |c: Caret| Caret { at: c.at.min(len), anchor: c.anchor.map(|a| a.min(len)), ..c };
            if self.vim.policy() == fenix_vim::EditPolicy::Standard {
                self.put_carets(fit(mark.primary), mark.extra.into_iter().map(fit).collect());
            } else {
                let pane = self.focused_pane_id();
                self.put_cursor(pane, fit(mark.primary).at);
            }
            self.say_carets();
            return;
        }
        self.set_message("no cursor move to undo");
    }

    /// The primary as a caret: where the cursor is, and where its
    /// selection starts when it has one.
    fn primary_caret(&self) -> Caret {
        let cursor = self.cursor();
        let anchor = (self.vim.mode() == Mode::Visual).then(|| self.vim.visual_anchor());
        Caret { at: cursor.char_idx, sticky: cursor.sticky_col, anchor }
    }

    /// Whether more cursors can be added here: standard keys, in a buffer
    /// you type in. With Vim's keys, says so.
    fn carets_can_be_added(&mut self) -> bool {
        if !self.standard_keys() {
            self.set_message("more cursors are standard keys' -- SPC t k switches to them");
            return false;
        }
        self.focused_takes_typing()
    }

    /// The primary's selection, else the word under the cursor (`true`
    /// when it's the word: then only whole words are its occurrences).
    fn selection_or_word(&self) -> Option<(std::ops::Range<usize>, bool)> {
        let buffer = &self.open().buffer;
        if self.vim.mode() == Mode::Visual {
            let (range, _) = self.vim.visual_selection_range(buffer, &self.cursor())?;
            if !range.is_empty() {
                return Some((range, false));
            }
        }
        fenix_vim::word_to_select(buffer, self.cursor().char_idx, self.vim.iskeyword_extra()).map(|r| (r, true))
    }

    /// `edit.add_next_occurrence` (Ctrl+D): with nothing selected, the
    /// word under the cursor selected; then, each time, a cursor on the
    /// next occurrence of the selection, which it selects. The newest is
    /// the primary, so the view goes with it.
    pub(crate) fn cmd_add_next_occurrence(&mut self) {
        if !self.carets_can_be_added() {
            return;
        }
        // Selected: what the selection really holds (a whole line for a
        // triple-click), not where its ends are.
        let selected = matches!(self.selection_or_word(), Some((_, false)));
        if !selected {
            let Some((word, _)) = self.selection_or_word() else {
                self.set_message("no word under the cursor to select");
                return;
            };
            let extra = self.extra_carets().to_vec();
            let sticky = self.col_of(word.end);
            self.put_carets(Caret { at: word.end, sticky, anchor: Some(word.start) }, extra);
            return;
        }
        let Some((range, _)) = self.selection_or_word() else { return };
        let mut extra = self.extra_carets().to_vec();
        if extra.len() + 1 >= MAX_CARETS {
            self.set_error(format!("{MAX_CARETS} cursors is as many as there can be"));
            return;
        }
        let buffer = &self.open().buffer;
        let needle = buffer.text_range(range.start, range.end);
        let extra_kw = self.vim.iskeyword_extra();
        let whole = fenix_vim::is_whole_word(buffer, &range, extra_kw);
        let mut taken: Vec<std::ops::Range<usize>> = extra.iter().filter_map(|c| c.anchor.map(|a| a.min(c.at)..a.max(c.at))).collect();
        taken.push(range.clone());
        let Some(next) = fenix_vim::next_occurrence(buffer, &needle, whole, extra_kw, range.end, &taken) else {
            self.set_message(format!("no other {}", one_line(&needle)));
            return;
        };
        // The old primary as the extra it becomes, its selection as it
        // was (a whole line's, a word's) -- not its raw ends.
        extra.push(Caret { at: range.end, sticky: self.col_of(range.end), anchor: Some(range.start) });
        let sticky = self.col_of(next.end);
        self.put_carets(Caret { at: next.end, sticky, anchor: Some(next.start) }, extra);
        self.say_carets();
    }

    /// `edit.select_all_occurrences` (Ctrl+Shift+L): a cursor on every
    /// occurrence of the selection (or of the word under the cursor),
    /// each selecting it.
    pub(crate) fn cmd_select_all_occurrences(&mut self) {
        if !self.carets_can_be_added() {
            return;
        }
        let Some((range, from_word)) = self.selection_or_word() else {
            self.set_message("no word under the cursor to select");
            return;
        };
        let buffer = &self.open().buffer;
        let needle = buffer.text_range(range.start, range.end);
        let extra_kw = self.vim.iskeyword_extra();
        let whole = from_word || fenix_vim::is_whole_word(buffer, &range, extra_kw);
        let found = match fenix_vim::occurrences_around(buffer, &needle, whole, extra_kw, MAX_CARETS, Some(&range)) {
            Ok(found) => found,
            Err(n) => {
                self.set_error(format!("{n} of {} is more cursors than {MAX_CARETS}", one_line(&needle)));
                return;
            }
        };
        let Some(own) = found.iter().position(|r| r.start == range.start).or((!found.is_empty()).then_some(0)) else { return };
        let carets: Vec<Caret> = found.iter().map(|r| Caret { at: r.end, sticky: self.col_of(r.end), anchor: Some(r.start) }).collect();
        let primary = carets[own];
        let extra = carets.into_iter().enumerate().filter(|&(i, _)| i != own).map(|(_, c)| c).collect();
        self.put_carets(primary, extra);
        self.say_carets();
    }

    /// `edit.add_cursor_above` / `_below` (Ctrl+Alt+Up/Down): a cursor on
    /// the line above the topmost (or below the bottommost), at its
    /// column. The new one is the primary.
    pub(crate) fn cmd_add_cursor_vertically(&mut self, down: bool) {
        if !self.carets_can_be_added() {
            return;
        }
        let mut extra = self.extra_carets().to_vec();
        if extra.len() + 1 >= MAX_CARETS {
            self.set_error(format!("{MAX_CARETS} cursors is as many as there can be"));
            return;
        }
        let primary = self.primary_caret();
        let buffer = &self.open().buffer;
        let line_of = |c: &Caret| buffer.rope().char_to_line(c.at.min(buffer.len_chars()));
        let all = extra.iter().chain(std::iter::once(&primary));
        let edge = if down { all.max_by_key(|c| (line_of(c), c.at)) } else { all.min_by_key(|c| (line_of(c), c.at)) };
        let Some(edge) = edge.copied() else { return };
        let line = line_of(&edge);
        let last = buffer.line_count().saturating_sub(1);
        let target = match (down, line) {
            (true, l) if l < last => l + 1,
            (false, l) if l > 0 => l - 1,
            _ => return,
        };
        let at = fenix_vim::at_column(buffer, target, edge.sticky);
        // The old primary with its selection as it was, a linewise one
        // whole.
        let primary = match self.selection_or_word().filter(|(_, word)| !*word) {
            Some((range, _)) => Caret { at: range.end, sticky: primary.sticky, anchor: Some(range.start) },
            None => primary,
        };
        extra.push(primary);
        self.put_carets(Caret { at, sticky: edge.sticky, anchor: None }, extra);
        self.say_carets();
    }
}

/// Whether command `id`, run from a chord, keeps the extra carets: the
/// clipboard's (at every caret) and those that add carets.
pub(crate) fn keeps_carets(id: &str) -> bool {
    matches!(id, "edit.copy" | "edit.paste" | "edit.add_next_occurrence" | "edit.select_all_occurrences" | "edit.add_cursor_above" | "edit.add_cursor_below" | "edit.cursor_undo")
}

/// `text` on one line, short, for a message.
fn one_line(text: &str) -> String {
    let line = text.lines().next().unwrap_or("");
    let short: String = line.chars().take(40).collect();
    if short.len() < text.len() {
        format!("\"{short}…\"")
    } else {
        format!("\"{short}\"")
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
        assert_eq!(app.extra_carets().len(), 2, "undo puts the carets back");
        app.type_text("w");
        assert_eq!(app.text(), "ywa\nywb\nywc\n", "and they still type at each");
        app.press_all("C-z C-z");
        assert_eq!(app.text(), "a\nb\nc\n", "a keystroke at a time");
        assert_eq!(app.extra_carets().len(), 2);
        app.press_all("C-y C-y");
        assert_eq!(app.text(), "ywa\nywb\nywc\n", "redo, at every caret");
        assert_eq!(app.extra_carets().len(), 2, "with its carets");
        assert_eq!(app.line_col(), (0, 2), "the primary where it was after");
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
    fn ctrl_d_selects_the_word_then_adds_its_next_occurrences() {
        let (mut app, _f) = app_on_file("a.txt", "foo bar foobar foo\nfoo");
        standard(&mut app);
        app.press("C-d");
        assert!(app.extra_carets().is_empty(), "the first press only selects");
        assert_eq!(app.vim.visual_selection_range(&app.open().buffer, &app.cursor()).map(|(r, _)| r), Some(0..3));
        app.press("C-d");
        assert_eq!(app.extra_carets().len(), 1);
        assert_eq!(app.cursor().char_idx, 18, "the newest is the primary; foobar isn't the word");
        app.press_all("C-d C-d");
        assert_eq!(app.extra_carets().len(), 2, "all three taken: nothing more");
        app.type_text("x");
        assert_eq!(app.text(), "x bar foobar x\nx", "typing replaces each selection");
        app.press("C-z");
        assert_eq!(app.text(), "foo bar foobar foo\nfoo", "one undo");
    }

    #[test]
    fn ctrl_d_on_a_selection_finds_it_anywhere_and_in_multi_byte_text() {
        let (mut app, _f) = app_on_file("a.txt", "été xété été");
        standard(&mut app);
        app.press_all("Right S-Right S-Right");
        app.press_all("C-d C-d");
        assert_eq!(app.extra_carets().len(), 2, "té, not a word, is found inside words");
        app.type_text("e");
        assert_eq!(app.text(), "ée xée ée");
    }

    #[test]
    fn ctrl_shift_l_puts_a_cursor_on_every_occurrence() {
        let (mut app, _f) = app_on_file("a.txt", "ab x ab\nab abc");
        standard(&mut app);
        app.press("C-S-l");
        assert_eq!(app.extra_carets().len(), 2, "the word under the cursor, whole");
        app.type_text("Z");
        assert_eq!(app.text(), "Z x Z\nZ abc");
        assert_eq!(app.line_col(), (0, 1), "the primary is the one it started on");
    }

    #[test]
    fn ctrl_alt_arrows_add_a_cursor_above_and_below_at_the_column() {
        let (mut app, _f) = app_on_file("a.txt", "abc\nd\nefg");
        standard(&mut app);
        app.press_all("Right Right");
        app.press_all("C-M-Down C-M-Down C-M-Down");
        assert_eq!(app.extra_carets().len(), 2, "the last line is as far as it goes");
        app.type_text("!");
        assert_eq!(app.text(), "ab!c\nd!\nef!g", "a short line's end; the last line without a newline");
        app.press("Esc");
        app.press("C-M-Up");
        assert_eq!(app.extra_carets().len(), 1);
        app.type_text("?");
        assert_eq!(app.text(), "ab!c\nd!?\nef!?g");
    }

    #[test]
    fn the_cursor_commands_are_standard_keys_and_say_so_with_vim_s() {
        let (mut app, _f) = app_on_file("a.txt", "foo foo\n");
        app.run_cmd("edit.add_next_occurrence");
        app.run_cmd("edit.add_cursor_below");
        assert!(app.extra_carets().is_empty());
        assert!(app.modeline_text().contains("standard keys"), "{}", app.modeline_text());
        app.press("C-d");
        assert!(app.extra_carets().is_empty(), "Ctrl+D is still Vim's");
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
