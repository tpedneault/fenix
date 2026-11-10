//! Scrolling a pane whose lines wrap. Its view is a display line and a
//! row of it -- `PaneState::scroll_line` and `scroll_row` -- never a row
//! counted from the top of the file, which would need the whole file
//! broken into rows and would move at every resize. Everything here
//! walks rows from a known place a bounded number of steps (`RowWalk`),
//! breaking only the lines it passes. A wrapped pane's scroll snaps: no
//! easing, a row at a time.

use std::collections::HashMap;

use super::*;

/// A place among a pane's rows: a display line (an index into
/// `pane_display_lines`) and a row of it.
pub(super) type RowPos = (usize, usize);

/// A pane's rows, walked from one place to another, each line broken
/// once.
pub(super) struct RowWalk<'a> {
    app: &'a App,
    buffer: BufferId,
    pub lines: Vec<usize>,
    width: usize,
    cursor_line: usize,
    /// Each line's rows found so far, and whether they're all of them.
    rows: HashMap<usize, (Vec<std::ops::Range<usize>>, bool)>,
}

impl RowWalk<'_> {
    /// Display line `index`'s first `k` rows (all of them, if fewer):
    /// their visual columns. Only as much of the line is broken as that.
    fn rows_upto(&mut self, index: usize, k: usize) -> &[std::ops::Range<usize>] {
        let (app, buffer, width, cursor_line) = (self.app, self.buffer, self.width, self.cursor_line);
        let line = self.lines.get(index).copied().unwrap_or(rows::NO_LINE);
        let known = self.rows.get(&index).is_some_and(|(r, whole)| *whole || r.len() >= k);
        if !known {
            // Asked for more than was found: twice as many, so a walk
            // down a long line breaks it a few times, not a row at a time.
            let had = self.rows.get(&index).map_or(0, |(r, _)| r.len());
            let want = k.max(had.saturating_mul(2)).max(8);
            let found = app.line_breaks(buffer, line, width, cursor_line, want);
            self.rows.insert(index, found);
        }
        let (rows, _) = &self.rows[&index];
        &rows[..k.min(rows.len())]
    }

    /// Display line `index`'s rows, all of them.
    pub(super) fn breaks(&mut self, index: usize) -> &[std::ops::Range<usize>] {
        self.rows_upto(index, usize::MAX)
    }

    /// The rows above display line `index` that aren't text (a code
    /// lens's): its first rows, before its text's. A row `r` of the line
    /// is its text's row `r - above`.
    pub(super) fn above(&self, index: usize) -> usize {
        self.lines.get(index).map_or(0, |&line| self.app.virtual_rows_above(self.buffer, line).len())
    }

    /// How many rows display line `index` has, if no more than `cap`;
    /// `cap + 1` when it has more.
    fn count_capped(&mut self, index: usize, cap: usize) -> usize {
        let above = self.above(index);
        above + self.rows_upto(index, cap.saturating_sub(above).saturating_add(1)).len().max(1)
    }

    fn count(&mut self, index: usize) -> usize {
        self.above(index) + self.breaks(index).len().max(1)
    }

    /// `pos` held to the rows there are: a top a resize, an edit or a
    /// fold left past its line's rows, or past the last line, is the
    /// last of them.
    pub(super) fn clamp(&mut self, pos: RowPos) -> RowPos {
        let index = pos.0.min(self.lines.len().saturating_sub(1));
        let rows = self.count_capped(index, pos.1);
        (index, pos.1.min(rows - 1))
    }

    /// The last row of all.
    #[cfg(test)]
    pub(super) fn end(&mut self) -> RowPos {
        let last = self.lines.len().saturating_sub(1);
        (last, self.count(last) - 1)
    }

    /// How many rows there are from `pos` to the very last, if no more
    /// than `cap`.
    pub(super) fn rows_to_end(&mut self, pos: RowPos, cap: usize) -> Option<usize> {
        let mut n = 0usize;
        let mut at = pos;
        loop {
            let rows = self.count_capped(at.0, at.1.saturating_add(cap - n.min(cap)));
            n += rows.saturating_sub(at.1 + 1);
            if n > cap {
                return None;
            }
            if at.0 + 1 >= self.lines.len() {
                return Some(n);
            }
            n += 1;
            at = (at.0 + 1, 0);
        }
    }

    /// `delta` rows on from `pos` (back, when negative), held to the
    /// first row and the last.
    pub(super) fn step(&mut self, pos: RowPos, delta: isize) -> RowPos {
        let (mut index, mut row) = self.clamp(pos);
        let mut left = delta.unsigned_abs();
        if delta >= 0 {
            while left > 0 {
                let here = self.count_capped(index, row + left) - 1 - row;
                if left <= here {
                    return (index, row + left);
                }
                if index + 1 >= self.lines.len() {
                    return (index, row + here);
                }
                left -= here + 1;
                index += 1;
                row = 0;
            }
        } else {
            while left > 0 {
                if left <= row {
                    return (index, row - left);
                }
                if index == 0 {
                    return (0, 0);
                }
                left -= row + 1;
                index -= 1;
                // Back into a line from its end: all its rows.
                row = self.count(index) - 1;
            }
        }
        (index, row)
    }

    /// How many rows `to` is below `from`, if it's no more than `cap`.
    pub(super) fn distance(&mut self, from: RowPos, to: RowPos, cap: usize) -> Option<usize> {
        let (from, to) = (self.clamp(from), self.clamp(to));
        if to < from {
            return None;
        }
        let mut n = 0;
        let mut at = from;
        while at.0 < to.0 {
            n += self.count_capped(at.0, at.1 + cap) - at.1;
            if n > cap {
                return None;
            }
            at = (at.0 + 1, 0);
        }
        n += to.1 - at.1;
        (n <= cap).then_some(n)
    }

    /// The row of display line `index` that visual column `col` is on,
    /// its rows above it that aren't text counted.
    pub(super) fn row_of(&mut self, index: usize, col: usize) -> usize {
        let above = self.above(index);
        let mut k = 8;
        loop {
            let rows = self.rows_upto(index, k);
            let found = rows.iter().position(|r| col < r.end);
            let all = rows.len() < k;
            if let Some(row) = found {
                return above + row;
            }
            if all {
                return above + rows.len().saturating_sub(1);
            }
            k = k.saturating_mul(2);
        }
    }
}

/// Where the view's top goes for the cursor at `cursor`, the top at
/// `top`, `visible` rows shown -- as `scroll_for_cursor` does it with
/// lines: a cursor more than half a window out of view, or one that
/// jumped, is put in the middle; one that only stepped past an edge
/// scrolls just far enough. Never so far that the last row is above the
/// bottom.
pub(super) fn follow(walk: &mut RowWalk, top: RowPos, cursor: RowPos, visible: usize, jumped: bool) -> RowPos {
    let visible = visible.max(1);
    let centred = |walk: &mut RowWalk| walk.step(cursor, -((visible / 2) as isize));
    let target = if cursor < top {
        match walk.distance(cursor, top, visible / 2) {
            Some(_) if !jumped => cursor,
            _ => centred(walk),
        }
    } else {
        // Past the bottom by no more than half a pane: just far enough,
        // as above the top.
        match walk.distance(top, cursor, visible - 1 + visible / 2) {
            Some(d) if d < visible => return top,
            Some(_) if !jumped => walk.step(cursor, -((visible - 1) as isize)),
            _ => centred(walk),
        }
    };
    // Not past the point where the last row is at the bottom.
    match walk.rows_to_end(target, visible) {
        Some(d) if d + 1 < visible => walk.step(target, -((visible - 1 - d) as isize)),
        _ => target,
    }
}

/// The focused pane's rows as Vim's motions by row see them: its width,
/// tab stops and decorations, taken when a key comes, so they don't
/// borrow the app.
pub(super) struct PaneRows {
    width: usize,
    tab_stops: TabStops,
    /// The buffer's decorations, shared with the next key's while they
    /// haven't changed.
    decorations: std::sync::Arc<Vec<crate::decorations::Decoration>>,
    cursor_line: usize,
}

/// A line as `PaneRows` lays it out: char column -> visual column, the
/// line as drawn, the continuation rows' indent, and the rows found so
/// far (all of them, `whole`).
struct LaidLine {
    col_map: Vec<usize>,
    display: String,
    indent: usize,
    breaks: Vec<std::ops::Range<usize>>,
    whole: bool,
}

impl LaidLine {
    /// At least `k` rows found, or all there are.
    fn upto(&mut self, k: usize, width: usize) -> &[std::ops::Range<usize>] {
        if !self.whole && self.breaks.len() < k {
            let want = k.max(self.breaks.len().saturating_mul(2)).max(8);
            (self.breaks, self.whole) = crate::wrap::break_line_rows(&self.display, width, self.indent, want);
        }
        &self.breaks
    }
}

impl PaneRows {
    fn lay(&self, buffer: &fenix_core::Buffer, line: usize) -> LaidLine {
        let start = buffer.line_start_char(line);
        let len = buffer.line_len(line);
        let text = buffer.text_range(start, start + len);
        let here: Vec<&crate::decorations::Decoration> = self.decorations.iter().filter(|d| d.start <= start + len && d.end >= start).collect();
        let (display, col_map) = if here.is_empty() {
            tabstops::expand_line(&text, &self.tab_stops)
        } else {
            let laid = crate::decorations::layout_line(&text, start, &here, &self.tab_stops, line == self.cursor_line);
            (laid.display, laid.col_map)
        };
        let indent = crate::wrap::continuation_indent(&display, self.width);
        LaidLine { col_map, display, indent, breaks: Vec::new(), whole: false }
    }

    /// `at`'s line, row of it and cell on that row, and the line laid out
    /// as far as that row.
    fn place(&self, buffer: &fenix_core::Buffer, at: usize) -> (usize, usize, usize, LaidLine) {
        let cursor = Cursor { char_idx: at.min(buffer.len_chars()), sticky_col: 0 };
        let (line, col) = buffer.line_col(&cursor);
        let mut laid = self.lay(buffer, line);
        let vcol = tabstops::visual_col(&laid.col_map, col);
        let mut k = 8;
        let row = loop {
            let rows = laid.upto(k, self.width);
            if let Some(row) = rows.iter().position(|r| vcol < r.end) {
                break row;
            }
            if laid.whole {
                break laid.breaks.len() - 1;
            }
            k = k.saturating_mul(2);
        };
        let x = vcol - laid.breaks[row].start.min(vcol) + if row > 0 { laid.indent } else { 0 };
        (line, row, x, laid)
    }
}

impl fenix_vim::ScreenRows for PaneRows {
    fn row_move(&self, buffer: &fenix_core::Buffer, at: usize, delta: isize, x: Option<usize>) -> Option<(usize, usize)> {
        let (mut line, mut row, here, mut laid) = self.place(buffer, at);
        let goal = x.unwrap_or(here);
        // As far as the last line, the empty one after a last newline too.
        let last = buffer.rope().len_lines().saturating_sub(1);
        let width = self.width;
        for _ in 0..delta.unsigned_abs() {
            if delta > 0 {
                if row + 1 < laid.upto(row + 2, width).len() {
                    row += 1;
                } else if line < last {
                    line += 1;
                    laid = self.lay(buffer, line);
                    row = 0;
                } else {
                    break;
                }
            } else if row > 0 {
                row -= 1;
            } else if line > 0 {
                line -= 1;
                laid = self.lay(buffer, line);
                // Into a line from its end: all its rows.
                row = laid.upto(usize::MAX, width).len() - 1;
            } else {
                break;
            }
        }
        // Whether this row is the line's last.
        let is_last = laid.upto(row + 2, width).len() == row + 1 && laid.whole;
        let cols = laid.breaks[row].clone();
        let indent = if row > 0 { laid.indent } else { 0 };
        let vcol = cols.start + goal.saturating_sub(indent);
        // On the row: its last char, or past the line's end on its last.
        let vcol = if !is_last { vcol.min(cols.end.saturating_sub(1)) } else { vcol.min(cols.end) };
        let (col, _) = crate::mouse::char_col_at(&laid.col_map, vcol);
        Some((buffer.line_start_char(line) + col, goal))
    }

    fn row_bounds(&self, buffer: &fenix_core::Buffer, at: usize) -> Option<std::ops::Range<usize>> {
        let (line, row, _, laid) = self.place(buffer, at);
        let start = buffer.line_start_char(line);
        let char_at = |vcol: usize| start + crate::mouse::char_col_at(&laid.col_map, vcol).0;
        let cols = &laid.breaks[row];
        Some(char_at(cols.start)..char_at(cols.end))
    }
}

impl App {
    /// The rows Vim's motions by row go by for the key being routed: the
    /// focused pane's, when it's text and wraps, else a row a line.
    pub(super) fn sync_screen_rows(&mut self) {
        let (pane, buffer) = (self.focused_pane_id(), self.focused_buffer_id());
        let wrap = (self.main_view == MainView::Editor && !self.terminal_focused && !self.sidebar_focused).then(|| self.wrap_cols(pane, buffer)).flatten();
        let rows: std::sync::Arc<dyn fenix_vim::ScreenRows> = match wrap {
            Some(width) => {
                let cursor_line = self.buffers.get(buffer).map_or(0, |ob| ob.buffer.line_col(&self.pane_state(pane).cursor).0);
                // Copied once while they're unchanged, not at every key.
                let rev = self.wasm.decorations_rev;
                let decorations = match &self.screen_decorations {
                    Some((b, r, d)) if *b == buffer && *r == rev => d.clone(),
                    _ => {
                        let d = std::sync::Arc::new(self.code_decorations(buffer).into_iter().cloned().collect::<Vec<_>>());
                        self.screen_decorations = Some((buffer, rev, d.clone()));
                        d
                    }
                };
                std::sync::Arc::new(PaneRows { width, tab_stops: self.tab_stops_for(buffer), decorations, cursor_line })
            }
            None => std::sync::Arc::new(fenix_vim::LineRows),
        };
        self.vim.set_screen_rows(rows);
    }

    /// `pane`'s rows to walk, when it shows `buffer` wrapped.
    pub(super) fn row_walk(&self, pane: fenix_window::WindowId, buffer: BufferId) -> Option<RowWalk<'_>> {
        let width = self.wrap_cols(pane, buffer)?;
        let ob = self.buffers.get(buffer)?;
        let cursor_line = ob.buffer.line_col(&self.pane_state(pane).cursor).0;
        Some(RowWalk { app: self, buffer, lines: self.pane_display_lines(pane, buffer), width, cursor_line, rows: HashMap::new() })
    }

    /// Where `pane`'s cursor is among its rows.
    pub(super) fn cursor_row_pos(&self, walk: &mut RowWalk, pane: fenix_window::WindowId, buffer: BufferId) -> RowPos {
        let Some(ob) = self.buffers.get(buffer) else { return (0, 0) };
        let (line, col) = ob.buffer.line_col(&self.pane_state(pane).cursor);
        // A line in a closed fold is its header's row, the last line
        // shown at or before it.
        let index = walk.lines.partition_point(|&l| l <= line).saturating_sub(1);
        let vcol = tabstops::visual_col(&self.line_col_map(buffer, line, line), col);
        (index, walk.row_of(index, vcol))
    }

    /// Puts `pane`'s view's top at `top`, at once.
    pub(super) fn set_row_top(&mut self, pane: fenix_window::WindowId, top: RowPos) {
        self.workspaces.active_scroll_anims_mut().remove(&pane);
        let state = self.pane_state_mut(pane);
        state.scroll_line = top.0;
        state.scroll_row = top.1;
        state.rendered_scroll = top.0 as f32;
    }

    /// In a wrapped pane, its view brought to its cursor, `visible` rows
    /// shown (centred when it `jumped` out of view): false, doing
    /// nothing, when the pane doesn't wrap.
    pub(super) fn follow_cursor_by_rows(&mut self, pane: fenix_window::WindowId, buffer: BufferId, visible: usize, jumped: bool) -> bool {
        let Some(mut walk) = self.row_walk(pane, buffer) else { return false };
        let state = *self.pane_state(pane);
        let cursor = self.cursor_row_pos(&mut walk, pane, buffer);
        // The top as it is now: held to its line's rows.
        let top = walk.clamp((state.scroll_line, state.scroll_row));
        let top = follow(&mut walk, top, cursor, visible, jumped);
        drop(walk);
        if top != (state.scroll_line, state.scroll_row) || state.rendered_scroll != state.scroll_line as f32 {
            self.set_row_top(pane, top);
        }
        true
    }

    /// `zt`, `zz`, `zb` in a wrapped pane: the cursor's row at the top,
    /// the middle or the bottom of `visible` rows. False when it doesn't
    /// wrap.
    pub(super) fn scroll_rows_to_cursor(&mut self, pane: fenix_window::WindowId, buffer: BufferId, target: VimScrollTarget, visible: usize) -> bool {
        let Some(mut walk) = self.row_walk(pane, buffer) else { return false };
        let cursor = self.cursor_row_pos(&mut walk, pane, buffer);
        let up = match target {
            VimScrollTarget::Top => 0,
            VimScrollTarget::Center => visible / 2,
            VimScrollTarget::Bottom => visible.saturating_sub(1),
        };
        let top = walk.step(cursor, -(up as isize));
        drop(walk);
        self.set_row_top(pane, top);
        true
    }

    /// The wheel on a wrapped pane without the focus: its view `rows`
    /// rows on (back, when negative), held so the last row can't go
    /// above the bottom of `visible`. False when it doesn't wrap.
    pub(super) fn scroll_by_rows(&mut self, pane: fenix_window::WindowId, buffer: BufferId, rows: isize, visible: usize) -> bool {
        let Some(mut walk) = self.row_walk(pane, buffer) else { return false };
        let state = *self.pane_state(pane);
        let was = walk.clamp((state.scroll_line, state.scroll_row));
        let mut top = walk.step(was, rows);
        if rows > 0 {
            // No further than the last row at the bottom.
            let visible = visible.max(1);
            if let Some(d) = walk.rows_to_end(top, visible) {
                if d + 1 < visible {
                    top = walk.step(top, -((visible - 1 - d) as isize)).max(was);
                }
            }
        }
        drop(walk);
        self.set_row_top(pane, top);
        true
    }

    /// The wheel on a wrapped pane with the focus: its view `rows` rows
    /// on, as without it, and the cursor moved by rows only as far as
    /// keeps it in view, onto the first char of the row it's held to.
    /// False when it doesn't wrap.
    pub(super) fn scroll_focused_by_rows(&mut self, pane: fenix_window::WindowId, buffer: BufferId, rows: isize, visible: usize) -> bool {
        if !self.scroll_by_rows(pane, buffer, rows, visible) {
            return false;
        }
        let Some(mut walk) = self.row_walk(pane, buffer) else { return true };
        let state = *self.pane_state(pane);
        let top = walk.clamp((state.scroll_line, state.scroll_row));
        let cursor = self.cursor_row_pos(&mut walk, pane, buffer);
        let visible = visible.max(1);
        // Held to the top row: the first char starting on it (a tab or a
        // decoration it begins inside starts on the row above), else the
        // next line's first. Held to the bottom row: the last char
        // starting on it or before (one it ends inside started there).
        let (index, row, top_side) = if cursor < top {
            (top.0, top.1, true)
        } else if walk.distance(top, cursor, visible - 1).is_none() {
            let bottom = walk.step(top, visible as isize - 1);
            (bottom.0, bottom.1, false)
        } else {
            return true;
        };
        // A row above the line that isn't text: as its first text row.
        let row = row.saturating_sub(walk.above(index));
        let cols = walk.rows_upto(index, row + 1).get(row).cloned().unwrap_or(0..0);
        let next_line = walk.lines.get(index + 1).copied();
        let line = walk.lines.get(index).copied().unwrap_or(0);
        drop(walk);
        let Some(ob) = self.buffers.get(buffer) else { return true };
        let col_map = self.line_col_map(buffer, line, line);
        let len = col_map.len().saturating_sub(1);
        let (line, col) = if top_side {
            let col = col_map[..len].partition_point(|&v| v < cols.start);
            match next_line {
                // No char of this line starts on the row: the next line's first.
                Some(next) if col >= len && len > 0 => (next, 0),
                _ => (line, col),
            }
        } else {
            (line, crate::mouse::char_col_at(&col_map, cols.end.saturating_sub(1).max(cols.start)).0)
        };
        // Where a click would put it: on a char, past the last only in Insert.
        let len = ob.buffer.line_len(line);
        let col = if self.vim.is_inserting() { col.min(len) } else { col.min(len.saturating_sub(1)) };
        let at = ob.buffer.line_start_char(line) + col;
        self.pane_state_mut(pane).cursor = Cursor { char_idx: at, sticky_col: col };
        self.vim.forget_row_goals();
        true
    }

    /// The visual columns of `buffer`'s `line` on each of its rows,
    /// wrapped at `width` (`rows::NO_LINE`, a filler, is one row).
    /// Its first `max_rows` of them, with whether they're all.
    pub(super) fn line_breaks(&self, buffer: BufferId, line: usize, width: usize, cursor_line: usize, max_rows: usize) -> (Vec<std::ops::Range<usize>>, bool) {
        let Some(ob) = self.buffers.get(buffer) else { return (std::iter::once(0..0).collect(), true) };
        if line == rows::NO_LINE || line >= ob.buffer.visual_line_count() {
            return (std::iter::once(0..usize::MAX).collect(), true);
        }
        // Found before, for the text as it is: from the cache, from frame
        // to frame and key to key until an edit, a resize, a change of
        // tab width or of decorations (or, for a decorated line, the
        // cursor going on or off it).
        let tab_stops = self.tab_stops_for(buffer);
        let decorated = self.wasm.decorations.contains_key(&buffer);
        let stamp = BreakStamp { edits: ob.buffer.edit_count(), width, tabs: format!("{tab_stops:?}"), decorations: self.wasm.decorations_rev, cursor_on: decorated && line == cursor_line };
        if let Some((was, rows, whole)) = self.break_cache.borrow().get(&(buffer, line)) {
            if *was == stamp && (*whole || rows.len() >= max_rows) {
                return (rows[..max_rows.min(rows.len())].to_vec(), *whole && max_rows >= rows.len());
            }
        }
        let start = ob.buffer.line_start_char(line);
        let len = ob.buffer.line_len(line);
        let line_text = ob.buffer.text_range(start, start + len);
        let decorations: Vec<&crate::decorations::Decoration> = self.code_decorations(buffer).into_iter().filter(|d| d.start <= start + len && d.end >= start).collect();
        let display = if !decorations.is_empty() {
            crate::decorations::layout_line(&line_text, start, &decorations, &tab_stops, line == cursor_line).display
        } else if line_text.contains('\t') {
            tabstops::expand_line(&line_text, &tab_stops).0
        } else {
            line_text
        };
        let indent = crate::wrap::continuation_indent(&display, width);
        let found = crate::wrap::break_line_rows(&display, width, indent, max_rows);
        let mut cache = self.break_cache.borrow_mut();
        // Bounded: a few panes' worth of lines, or it starts again.
        if cache.len() > 4096 {
            cache.clear();
        }
        cache.insert((buffer, line), (stamp, found.0.clone(), found.1));
        found
    }
}

/// What a line's rows were found for: they hold while it's the same.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct BreakStamp {
    edits: u64,
    width: usize,
    tabs: String,
    decorations: u64,
    cursor_on: bool,
}

/// Lines' rows found, by buffer and line.
pub(super) type BreakCache = HashMap<(BufferId, usize), (BreakStamp, Vec<std::ops::Range<usize>>, bool)>;

#[cfg(test)]
mod tests {
    use super::super::test_keys::app_on_file;
    use super::*;

    /// `lines` lines of `aaaa bbbb ` / `cccc dddd ` / `eeee`: three rows
    /// each at ten columns.
    fn three_row_lines(lines: usize) -> (App, super::super::test_keys::TestFile) {
        let text: String = (0..lines).map(|_| "aaaa bbbb cccc dddd eeee\n").collect();
        let (mut app, file) = app_on_file("a.txt", &text);
        // A plain text file wraps unless turned off.
        assert!(app.buffer_wraps(app.focused_buffer_id()));
        let pane = app.focused_pane_id();
        app.set_pane_text_cols(pane, 10);
        (app, file)
    }

    fn top(app: &App) -> RowPos {
        let state = app.pane_state(app.focused_pane_id());
        (state.scroll_line, state.scroll_row)
    }

    #[test]
    fn the_view_follows_the_cursor_down_and_up_by_rows() {
        let (mut app, _f) = three_row_lines(20);
        app.ensure_cursor_visible(5);
        assert_eq!(top(&app), (0, 0));
        // Line 2's first row is row 6 of all: the bottom of five is row 2.
        app.press_all("2 j");
        app.ensure_cursor_visible(5);
        assert_eq!(top(&app), (0, 2));
        // Its last row, row 8: one more than a line.
        app.press("$");
        app.ensure_cursor_visible(5);
        assert_eq!(top(&app), (1, 1));
        assert_eq!(app.focused_overlays_now(5).caret, Some((4, 3)), "the caret on the bottom row, on `eeee`'s last");
        // Back up a line and to its start: row 3, at the top.
        app.press_all("k 0");
        app.ensure_cursor_visible(5);
        assert_eq!(top(&app), (1, 0));
        assert_eq!(app.pane_state(app.focused_pane_id()).rendered_scroll, 1.0, "snapped, no easing");
    }

    #[test]
    fn a_line_taller_than_the_pane_is_followed_inside() {
        let line = "word ".repeat(40);
        let (mut app, _f) = app_on_file("a.txt", &format!("{line}\nnext\n"));
        // A plain text file wraps unless turned off.
        assert!(app.buffer_wraps(app.focused_buffer_id()));
        let pane = app.focused_pane_id();
        app.set_pane_text_cols(pane, 10);
        app.press("$");
        app.ensure_cursor_visible(5);
        // The line's twenty-one rows end at row 20, its last blank on a
        // row of its own (`$` is on it), `next` is row 21 and the empty
        // line after it row 22: centred, which the end leaves there.
        assert_eq!(top(&app), (0, 18));
        assert_eq!(app.focused_overlays_now(5).caret, Some((2, 0)));
        app.press("0");
        app.ensure_cursor_visible(5);
        assert_eq!(top(&app), (0, 0), "a jump of more than half the pane back: centred, held to the top");
    }

    #[test]
    fn a_far_jump_is_centred_and_the_end_stays_at_the_bottom() {
        let (mut app, _f) = three_row_lines(20);
        app.press_all("1 0 G");
        app.ensure_cursor_visible(5);
        // Line 9's first row is row 27; two above it.
        assert_eq!(top(&app), (8, 1));
        app.press("G");
        app.ensure_cursor_visible(5);
        // Line 19's first row is row 57: two above it, with the rows
        // left below it (to the empty line after, row 60) filling the pane.
        assert_eq!(top(&app), (18, 1));
        app.press("$");
        app.ensure_cursor_visible(5);
        assert_eq!(top(&app), (18, 1), "its last row, 59, is still in view");
    }

    #[test]
    fn typing_past_the_bottom_row_brings_it_into_view() {
        let (mut app, _f) = three_row_lines(2);
        app.press_all("j $");
        app.ensure_cursor_visible(4);
        assert_eq!(top(&app), (0, 2));
        app.press("a");
        app.type_text(" ffff gggg");
        app.ensure_cursor_visible(4);
        // Line 1 is four rows now, its last row 7.
        assert_eq!(top(&app), (1, 0));
    }

    #[test]
    fn zt_zz_and_zb_put_the_cursor_s_row_there() {
        let (mut app, _f) = three_row_lines(40);
        app.press_all("2 0 G $");
        let pane = app.focused_pane_id();
        let visible = {
            let (sidebar_px, terminal_h, modeline_top) = app.frame_metrics(600.0);
            let geometry = app.frame_geometry(800.0, sidebar_px, terminal_h, modeline_top);
            let rect = geometry.panes.iter().find(|(id, _)| *id == pane).unwrap().1;
            text::lines_that_fit(pane_content_rect(rect, text::LINE_HEIGHT, app.pane_has_breadcrumb(pane)).h, text::LINE_HEIGHT)
        };
        // Line 19's last row: row 59 of all.
        app.scroll_window_at(VimScrollTarget::Top, 800.0, 600.0);
        assert_eq!(top(&app), (19, 2));
        let (bottom, middle) = {
            let mut walk = app.row_walk(pane, app.focused_buffer_id()).unwrap();
            (walk.step((19, 2), -(visible as isize - 1)), walk.step((19, 2), -((visible / 2) as isize)))
        };
        // Three rows a line: the steps back land a row in.
        assert_eq!(bottom.0 * 3 + bottom.1, 59 - (visible - 1));
        app.scroll_window_at(VimScrollTarget::Bottom, 800.0, 600.0);
        assert_eq!(top(&app), bottom);
        app.scroll_window_at(VimScrollTarget::Center, 800.0, 600.0);
        assert_eq!(top(&app), middle);
    }

    #[test]
    fn the_wheel_on_a_wrapped_pane_without_the_focus_scrolls_it_by_rows() {
        let (mut app, _f) = three_row_lines(4);
        let pane = app.focused_pane_id();
        app.run_cmd("window.split_vertical");
        let other = app.focused_pane_id();
        app.set_pane_text_cols(other, 10);
        app.windows_mut().focus(other);
        app.set_pane_text_cols(pane, 10);
        let h = text::LINE_HEIGHT * 6.0;
        let visible = text::lines_that_fit(h, text::LINE_HEIGHT);
        let row_at_top = |app: &App| {
            let state = app.pane_state(pane);
            state.scroll_line * 3 + state.scroll_row
        };
        app.scroll_unfocused_pane(pane, -2, h, text::LINE_HEIGHT);
        assert_eq!(row_at_top(&app), 2);
        app.scroll_unfocused_pane(pane, -20, h, text::LINE_HEIGHT);
        // Thirteen rows (the empty last line's one): the last at the bottom.
        assert_eq!(row_at_top(&app), 13 - visible);
        app.scroll_unfocused_pane(pane, 4, h, text::LINE_HEIGHT);
        assert_eq!(row_at_top(&app), 13 - visible - 4);
    }

    #[test]
    fn a_top_left_past_its_line_s_rows_by_a_resize_is_held_to_them() {
        let line = "word ".repeat(40);
        let (mut app, _f) = app_on_file("a.txt", &format!("{line}\nnext\n"));
        if !app.buffer_wraps(app.focused_buffer_id()) {
            app.run_cmd("view.toggle_wrap");
        }
        let pane = app.focused_pane_id();
        app.set_pane_text_cols(pane, 10);
        app.press("$");
        app.ensure_cursor_visible(5);
        app.press("j");
        app.ensure_cursor_visible(5);
        assert_eq!(top(&app), (0, 18));
        // Four times as wide: line 0 is six rows now, the top's row 18 gone.
        app.set_pane_text_cols(pane, 40);
        app.ensure_cursor_visible(5);
        let (index, row) = top(&app);
        assert!(index == 0 && row < 6 || index == 1, "{:?}", (index, row));
        assert_eq!(app.focused_overlays_now(5).caret.map(|(r, _)| r < 5), Some(true), "the cursor on `next` shown");
    }

    #[test]
    fn turning_wrap_off_and_on_starts_every_pane_s_rows_afresh() {
        let (mut app, _f) = three_row_lines(10);
        let first = app.focused_pane_id();
        app.pane_state_mut(first).scroll_row = 2;
        app.run_cmd("window.split_vertical");
        let second = app.focused_pane_id();
        app.pane_state_mut(second).scroll_row = 1;
        app.run_cmd("view.toggle_wrap");
        assert_eq!((app.pane_state(first).scroll_row, app.pane_state(second).scroll_row), (0, 0));
    }

    #[test]
    fn the_wheel_on_a_focused_wrapped_pane_scrolls_its_rows_and_keeps_the_cursor_in_view() {
        // One line of five thousand chars: without rows it couldn't scroll.
        let (mut app, _f) = app_on_file("a.txt", &"word ".repeat(1000));
        if !app.buffer_wraps(app.focused_buffer_id()) {
            app.run_cmd("view.toggle_wrap");
        }
        let pane = app.focused_pane_id();
        app.set_pane_text_cols(pane, 10);
        let (sidebar_px, terminal_h, modeline_top) = app.frame_metrics(600.0);
        let rect = app.frame_geometry(800.0, sidebar_px, terminal_h, modeline_top).panes.into_iter().find(|(id, _)| *id == pane).unwrap().1;
        let pos = (rect.x + 50.0, rect.y + 50.0);
        let notch = |app: &mut App| app.handle_wheel_at(pos, MouseScrollDelta::LineDelta(0.0, -1.0), 800.0, 600.0);
        notch(&mut app);
        let (_, row) = top(&app);
        assert!(row > 0, "scrolled by rows");
        assert_eq!(app.line_col(), (0, row * 10), "the cursor onto the top row's first char");
        for _ in 0..3 {
            notch(&mut app);
        }
        assert_eq!(app.line_col().1, top(&app).1 * 10);
    }

    #[test]
    fn the_wheel_holds_the_cursor_on_the_top_row_when_that_row_starts_inside_a_tab() {
        // A tab at eight is columns 8 to 15 at the default stop: row 1
        // (from column 9) starts inside it; `x` is the first char on it.
        let text = format!("abcdefgh\tx yyyy\n{}", "more\n".repeat(40));
        let (mut app, _f) = app_on_file("a.txt", &text);
        if !app.buffer_wraps(app.focused_buffer_id()) {
            app.run_cmd("view.toggle_wrap");
        }
        let (pane, buffer) = (app.focused_pane_id(), app.focused_buffer_id());
        app.set_pane_text_cols(pane, 10);
        assert!(app.scroll_focused_by_rows(pane, buffer, 1, 5));
        assert_eq!(top(&app), (0, 1));
        assert_eq!(app.line_col(), (0, 9), "on `x`, not the tab above");
        app.ensure_cursor_visible(5);
        assert_eq!(top(&app), (0, 1), "and the view stays");
    }

    /// After the wheel's hold, the cursor's row is in view: following it
    /// moves nothing.
    fn held_in_view(app: &mut App, visible: usize) {
        let before = top(app);
        app.ensure_cursor_visible(visible);
        assert_eq!(top(app), before, "the held cursor is in view");
    }

    #[test]
    fn the_wheel_s_hold_on_a_row_inside_a_tab_stays_in_view_and_on_a_char() {
        // `abcdefgh` and a tab to sixteen: row 1 is all tab, no char
        // starts on it; the hold goes to the next line's first char.
        let text = format!("abcdefgh\t\n{}", "more\n".repeat(40));
        let (mut app, _f) = app_on_file("a.txt", &text);
        if !app.buffer_wraps(app.focused_buffer_id()) {
            app.run_cmd("view.toggle_wrap");
        }
        let (pane, buffer) = (app.focused_pane_id(), app.focused_buffer_id());
        app.set_pane_text_cols(pane, 10);
        assert!(app.scroll_focused_by_rows(pane, buffer, 1, 5));
        assert_eq!(top(&app), (0, 1));
        assert_eq!(app.line_col(), (1, 0), "a char, not the newline");
        held_in_view(&mut app, 5);
        // Up a row at a time from below a row inside a tab: never stuck.
        let text = format!("{}a\tb\n{}", "x\n".repeat(4), "m\n".repeat(40));
        let (mut app, _f) = app_on_file("a.txt", &text);
        if !app.buffer_wraps(app.focused_buffer_id()) {
            app.run_cmd("view.toggle_wrap");
        }
        let (pane, buffer) = (app.focused_pane_id(), app.focused_buffer_id());
        app.set_pane_text_cols(pane, 4);
        app.press_all("1 5 G");
        app.ensure_cursor_visible(5);
        let mut tops = Vec::new();
        for _ in 0..20 {
            assert!(app.scroll_focused_by_rows(pane, buffer, -1, 5));
            held_in_view(&mut app, 5);
            tops.push(top(&app));
        }
        assert_eq!(tops.last(), Some(&(0, 0)), "up to the top: {tops:?}");
    }

    #[test]
    fn a_cursor_in_a_closed_fold_is_its_header_s_row_not_the_file_s_top() {
        let text = format!("fn a() {{\n{}\n}}\nend\n", "    x;\n".repeat(40));
        let (mut app, _f) = app_on_file("a.rs", &text);
        app.run_cmd("view.toggle_wrap");
        let pane = app.focused_pane_id();
        app.set_pane_text_cols(pane, 10);
        app.press("j");
        app.run_cmd("code.toggle_fold");
        // The cursor on a hidden line: as on the header.
        app.press_all("2 0 G");
        let buffer = app.focused_buffer_id();
        let mut walk = app.row_walk(pane, buffer).unwrap();
        assert_eq!(app.cursor_row_pos(&mut walk, pane, buffer).0, 0);
    }

    #[test]
    fn a_line_s_rows_are_found_once_until_its_text_or_the_width_changes() {
        let (mut app, _f) = three_row_lines(3);
        let (pane, buffer) = (app.focused_pane_id(), app.focused_buffer_id());
        assert_eq!(app.line_breaks(buffer, 1, 10, 0, usize::MAX), (vec![0..10, 10..20, 20..24], true));
        assert!(app.break_cache.borrow().contains_key(&(buffer, 1)));
        // Asked for fewer: the first of those found, not all.
        let (rows, whole) = app.line_breaks(buffer, 1, 10, 0, 1);
        assert_eq!((rows.len(), rows.first().cloned(), whole), (1, Some(0..10), false));
        // An edit: found again for the new text.
        app.press_all("j A");
        app.type_text(" ffff");
        assert_eq!(app.line_breaks(buffer, 1, 10, 0, usize::MAX).0.len(), 3);
        app.set_pane_text_cols(pane, 30);
        assert_eq!(app.line_breaks(buffer, 1, 30, 0, usize::MAX).0.len(), 1);
    }

    #[test]
    fn a_walk_steps_and_measures_rows_across_lines() {
        let (app, _f) = three_row_lines(3);
        let mut walk = app.row_walk(app.focused_pane_id(), app.focused_buffer_id()).unwrap();
        assert_eq!(walk.clamp((0, 9)), (0, 2), "a row past the line's held to its last");
        assert_eq!(walk.distance((0, 9), (1, 0), 10), Some(1));
        assert_eq!(walk.step((0, 1), 4), (1, 2));
        assert_eq!(walk.step((1, 2), -4), (0, 1));
        assert_eq!(walk.step((0, 0), -3), (0, 0));
        assert_eq!(walk.step((0, 0), 100), walk.end());
        assert_eq!(walk.distance((0, 1), (2, 0), 10), Some(5));
        assert_eq!(walk.distance((0, 1), (2, 0), 4), None);
        assert_eq!(walk.distance((2, 0), (0, 1), 10), None);
    }

    fn wrapped(text: &str, cols: usize) -> (App, super::super::test_keys::TestFile) {
        let (mut app, file) = app_on_file("a.txt", text);
        // A plain text file wraps unless turned off.
        assert!(app.buffer_wraps(app.focused_buffer_id()));
        let pane = app.focused_pane_id();
        app.set_pane_text_cols(pane, cols);
        (app, file)
    }

    fn standard(app: &mut App) {
        app.config.set_value("editor.keys", Some(fenix_config::Value::Text("standard".into())));
        app.sync_key_policy();
    }

    // `the quick ` / `brown fox ` / `jumps`, at ten columns.
    const FOX: &str = "the quick brown fox jumps\nover the lazy dog\n";

    #[test]
    fn gj_and_gk_go_by_rows_keeping_the_cell_and_j_and_k_stay_by_lines() {
        let (mut app, _f) = wrapped(FOX, 10);
        app.press("w");
        assert_eq!(app.line_col(), (0, 4));
        app.press_all("g j");
        assert_eq!(app.line_col(), (0, 14), "`brown`'s `n`, under `q`");
        app.press_all("g j");
        assert_eq!(app.line_col(), (0, 24), "`jumps` is shorter: its last char");
        app.press_all("g j");
        assert_eq!(app.line_col(), (1, 4), "the next line's first row, back at the cell kept");
        app.press_all("g k g k");
        assert_eq!(app.line_col(), (0, 14));
        app.press_all("2 g k");
        assert_eq!(app.line_col(), (0, 4), "a count");
        app.press("j");
        assert_eq!(app.line_col(), (1, 4), "j is by line");
        // From `k`, the ninth cell: through `jumps`, too short for it.
        app.press_all("g g 0 8 l g j g j");
        assert_eq!(app.line_col(), (0, 24));
        app.press_all("g j");
        assert_eq!(app.line_col(), (1, 8), "the ninth cell kept through the short row");
    }

    #[test]
    fn gj_with_an_operator_and_in_visual_is_charwise() {
        let (mut app, _f) = wrapped(FOX, 10);
        app.press_all("w d g j");
        assert_eq!(app.text(), "the n fox jumps\nover the lazy dog\n", "exclusive, as Vim's");
        app.press("u");
        app.press_all("0 w v g j y");
        assert_eq!(app.vim.register().0, "quick brown");
    }

    #[test]
    fn without_wrap_gj_is_j() {
        let (mut app, _f) = app_on_file("a.txt", FOX);
        app.press_all("w g j");
        assert_eq!(app.line_col(), (1, 4));
    }

    #[test]
    fn standard_keys_up_and_down_go_by_rows_and_select_by_them_with_shift() {
        let (mut app, _f) = wrapped(FOX, 10);
        standard(&mut app);
        app.press_all("Right Right Right Right Down");
        assert_eq!(app.line_col(), (0, 14));
        app.press("Down");
        assert_eq!(app.line_col(), (0, 24), "`jumps`'s `s`, under `n`");
        app.press("Down");
        assert_eq!(app.line_col(), (1, 4));
        app.press_all("Up Up");
        assert_eq!(app.line_col(), (0, 14), "the cell kept");
        app.press("S-Up");
        assert_eq!(app.line_col(), (0, 4));
        let o = app.focused_overlays_now(6);
        assert_eq!(o.selection, vec![(0, 4, 10), (1, 0, 4)], "from `q` to `n`, across the break");
    }

    #[test]
    fn home_and_end_go_to_the_row_s_ends_first_then_the_line_s() {
        let (mut app, _f) = wrapped(FOX, 10);
        standard(&mut app);
        app.press_all("Down Right Right");
        assert_eq!(app.line_col(), (0, 12));
        app.press("Home");
        assert_eq!(app.line_col(), (0, 10), "the row's start");
        app.press("Home");
        assert_eq!(app.line_col(), (0, 0), "then the line's");
        app.press_all("Down End");
        assert_eq!(app.line_col(), (0, 19), "the row's last char, before its break");
        app.press("End");
        assert_eq!(app.line_col(), (0, 25), "then the line's end");
    }

    #[test]
    fn insert_s_arrows_go_by_rows_too() {
        let (mut app, _f) = wrapped(FOX, 10);
        app.press_all("w i Down");
        assert_eq!(app.line_col(), (0, 14));
        app.press("End");
        assert_eq!(app.line_col(), (0, 19));
        app.type_text("!");
        assert_eq!(app.text(), "the quick brown fox! jumps\nover the lazy dog\n");
        app.press("Esc");
        app.press("u");
        assert_eq!(app.text(), FOX, "one undo step");
    }

    #[test]
    fn extra_cursors_are_added_by_line_and_each_moves_by_its_own_rows() {
        let (mut app, _f) = wrapped("aaaa bbbb cccc\ndddd eeee ffff\n", 10);
        standard(&mut app);
        app.press("C-M-Down");
        assert_eq!(app.line_col(), (1, 0), "added a line down, not a row");
        app.press("Down");
        assert_eq!(app.line_col(), (1, 10));
        app.type_text("X");
        assert_eq!(app.text(), "aaaa bbbb Xcccc\ndddd eeee Xffff\n");
    }

    #[test]
    fn a_macro_that_edits_then_moves_by_row_goes_by_the_rows_its_edit_made() {
        let (mut app, _f) = wrapped("aaaa\naaaa\nend\n", 10);
        app.press_all("q a A");
        app.type_text(" bbbb cccc");
        app.press_all("Esc 0 g j q");
        assert_eq!(app.line_col(), (0, 10), "the append made a second row");
        // One key, `@a`: the append, then gj on the rows it made.
        app.press_all("j @ a");
        assert_eq!(app.line_col(), (1, 10));
    }

    #[test]
    fn end_on_a_row_ending_in_a_blank_goes_after_its_last_word_and_on_a_broken_word_to_its_last_char() {
        // `the quick ` ends in a blank: End goes after `quick`, where
        // typing goes on with it; Shift+End selects to there.
        let (mut app, _f) = wrapped(FOX, 10);
        standard(&mut app);
        app.press("End");
        assert_eq!(app.line_col(), (0, 9));
        app.press("Home");
        app.press("S-End");
        assert_eq!(app.vim.visual_selection_range(&app.open().buffer, &app.cursor()).map(|(r, _)| r), Some(0..9));
        // A word broken by the row: its row's last char.
        let (mut app, _f) = wrapped("abcdefghijklmnop\n", 5);
        standard(&mut app);
        app.press("End");
        assert_eq!(app.line_col(), (0, 3));
        app.press("End");
        assert_eq!(app.line_col(), (0, 16));
        // And in Insert, with Vim keys.
        let (mut app, _f) = wrapped(FOX, 10);
        app.press_all("i End");
        assert_eq!(app.line_col(), (0, 9));
    }

    #[test]
    fn gj_on_the_last_row_stays_and_a_count_stops_there() {
        let (mut app, _f) = wrapped(FOX, 10);
        // `over the ` / `lazy dog`: `l` of `lazy` is on the last row.
        app.press_all("j 0 9 l");
        assert_eq!(app.line_col(), (1, 9));
        app.press_all("g j");
        assert_eq!(app.line_col(), (1, 9), "the last row: nowhere to go, as with j");
        app.press_all("d g j");
        assert_eq!(app.text(), FOX, "nothing deleted");
        app.press_all("g g 0 4 l 9 g j");
        assert_eq!(app.line_col(), (1, 13), "as far as the last row");
    }

    #[test]
    fn each_cursor_keeps_its_own_cell_through_short_rows() {
        let (mut app, _f) = wrapped("012345678901\n012345678901\n--\n--\n012345678901\n012345678901\n", 20);
        standard(&mut app);
        app.press_all("End Left Left C-M-Down");
        assert_eq!(app.line_col(), (1, 10));
        for _ in 0..4 {
            app.press("Down");
        }
        let mut at: Vec<(usize, usize)> = app.extra_carets().iter().map(|c| app.open().buffer.line_col(&Cursor { char_idx: c.at, sticky_col: 0 })).collect();
        at.push(app.line_col());
        at.sort();
        assert_eq!(at, vec![(4, 10), (5, 10)], "both back at column 10");
    }

    #[test]
    fn a_cursor_moving_onto_another_s_char_doesn_t_take_its_cell() {
        // Row 0 long, row 1 three chars, row 2 long: caret A on row 0's
        // tenth cell, caret B at row 1's end.
        let (mut app, _f) = wrapped("0123456789012\n012\n0123456789012\n", 20);
        standard(&mut app);
        app.press_all("Right Right Right Right Right Right Right Right Right Right C-M-Down");
        let carets = |app: &App| {
            let mut at: Vec<(usize, usize)> = app.extra_carets().iter().map(|c| app.open().buffer.line_col(&Cursor { char_idx: c.at, sticky_col: 0 })).collect();
            at.push(app.line_col());
            at.sort();
            at
        };
        assert_eq!(carets(&app), vec![(0, 10), (1, 3)]);
        app.press("Down");
        assert_eq!(carets(&app), vec![(1, 3), (2, 3)]);
        app.press("Up");
        assert_eq!(carets(&app), vec![(0, 10), (1, 3)], "each back at its own cell");
    }

    #[test]
    fn a_click_forgets_the_cell_kept() {
        let (mut app, _f) = wrapped(FOX, 10);
        app.press_all("0 8 l g j g j");
        assert_eq!(app.line_col(), (0, 24));
        // Clicked onto the same char: a fresh start, its own cell.
        let pane = app.focused_pane_id();
        app.put_cursor(pane, 24);
        app.press_all("g j");
        assert_eq!(app.line_col(), (1, 4));
    }

    #[test]
    fn another_move_forgets_the_cell_kept() {
        let (mut app, _f) = wrapped(FOX, 10);
        app.press_all("0 8 l g j g j");
        assert_eq!(app.line_col(), (0, 24), "kept the ninth cell through the short row");
        app.press_all("h l g j");
        assert_eq!(app.line_col(), (1, 4), "from `s`'s own cell, the fifth");
    }

    #[test]
    fn a_macro_into_another_pane_moves_by_that_pane_s_rows() {
        let (mut app, _f) = wrapped(&format!("{}\nend\n", "word ".repeat(10)), 10);
        let wrapped_pane = app.focused_pane_id();
        app.run_cmd("window.split_vertical");
        let other_buffer = app.buffers.open_scratch();
        for pane in app.windows().windows() {
            if pane != wrapped_pane {
                app.set_pane_content(pane, other_buffer);
            }
        }
        let other = app.focused_pane_id();
        app.test_insert_str(&format!("{}\nend\n", "word ".repeat(10)));
        app.test_set_cursor(Cursor::at_start());
        assert_eq!(app.wrap_cols(other, other_buffer), None);
        // Recorded from the wrapped pane: to the other, then Down in Insert.
        app.windows_mut().focus(wrapped_pane);
        app.press_all("q a SPC w w i Down Esc q");
        assert_ne!(app.focused_pane_id(), wrapped_pane);
        assert_eq!(app.focused_buffer_id(), other_buffer);
        assert_eq!(app.line_col().0, 1, "Down by line in the pane that doesn't wrap");
        app.test_set_cursor(Cursor::at_start());
        app.windows_mut().focus(wrapped_pane);
        app.press_all("@ a");
        assert_eq!(app.focused_buffer_id(), other_buffer);
        assert_eq!(app.line_col().0, 1, "played back, still by line there");
    }

    #[test]
    fn without_wrap_dgj_and_a_count_are_dj_and_a_count_of_j() {
        let (mut app, _f) = app_on_file("a.rs", "one\ntwo\nthree\nfour\n");
        app.press_all("2 g j");
        assert_eq!(app.line_col(), (2, 0));
        app.press_all("g g d g j");
        assert_eq!(app.text(), "two\nthree\nfour\n", "charwise, as dj is here");
    }

    #[test]
    fn page_down_in_a_wrapped_pane_goes_by_rows() {
        let line = "word ".repeat(60);
        let (mut app, _f) = wrapped(&format!("{line}\nnext\n"), 10);
        app.press("PageDown");
        // Without a window the page is twenty rows: nineteen down.
        assert_eq!(app.line_col(), (0, 190));
        app.press("PageUp");
        assert_eq!(app.line_col(), (0, 0));
    }
}


#[cfg(test)]
mod large {
    use super::*;

    /// What a frame and a few keys cost on a million lines, wrap on and off.
    #[test]
    #[ignore]
    fn measure_a_million_lines() {
        let text: String = (0..1_000_000).map(|i| format!("line {i} with some words to wrap at a narrow width\n")).collect();
        for wrap in [false, true] {
            let mut app = App::with_file(None);
            let (pane, id) = (app.focused_pane_id(), app.buffers.open_scratch());
            app.set_pane_content(pane, id);
            let ob = app.buffers.get_mut(id).unwrap();
            ob.buffer = fenix_core::Buffer::from_text(&text);
            ob.kind = BufferKind::Text;
            app.test_set_cursor(Cursor::at_start());
            app.wrap_overrides.insert(id, wrap);
            app.set_pane_text_cols(pane, 20);
            let t = std::time::Instant::now();
            for _ in 0..10 {
                app.ensure_cursor_visible(40);
                let _ = app.focused_overlays_now(41);
            }
            let frame = t.elapsed() / 10;
            let t = std::time::Instant::now();
            app.press("G");
            app.ensure_cursor_visible(40);
            for _ in 0..10 {
                app.press_all("g k");
                app.ensure_cursor_visible(40);
            }
            let keys = t.elapsed();
            eprintln!("wrap {wrap}: a frame's rows {frame:?}; G and ten gk {keys:?}");
        }
    }
}
