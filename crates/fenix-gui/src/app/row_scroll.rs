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

    /// How many rows display line `index` has, if no more than `cap`;
    /// `cap + 1` when it has more.
    fn count_capped(&mut self, index: usize, cap: usize) -> usize {
        self.rows_upto(index, cap.saturating_add(1)).len().max(1)
    }

    fn count(&mut self, index: usize) -> usize {
        self.breaks(index).len().max(1)
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

    /// The row of display line `index` that visual column `col` is on.
    pub(super) fn row_of(&mut self, index: usize, col: usize) -> usize {
        let mut k = 8;
        loop {
            let rows = self.rows_upto(index, k);
            let found = rows.iter().position(|r| col < r.end);
            let all = rows.len() < k;
            if let Some(row) = found {
                return row;
            }
            if all {
                return rows.len().saturating_sub(1);
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

impl App {
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
        let held = if cursor < top {
            Some(top)
        } else if walk.distance(top, cursor, visible - 1).is_none() {
            Some(walk.step(top, visible as isize - 1))
        } else {
            None
        };
        let Some((index, row)) = held else { return true };
        let line = walk.lines.get(index).copied().unwrap_or(0);
        // Only as far as that row.
        let vcol = walk.rows_upto(index, row + 1).get(row).map_or(0, |r| r.start);
        drop(walk);
        let Some(ob) = self.buffers.get(buffer) else { return true };
        // The first char that starts on the row: a tab or a decoration
        // the row begins inside starts on the row above.
        let col_map = self.line_col_map(buffer, line, line);
        let col = col_map[..col_map.len().saturating_sub(1)].partition_point(|&v| v < vcol);
        let at = ob.buffer.line_start_char(line) + col;
        self.pane_state_mut(pane).cursor = Cursor { char_idx: at, sticky_col: col };
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
        let start = ob.buffer.line_start_char(line);
        let len = ob.buffer.line_len(line);
        let line_text = ob.buffer.text_range(start, start + len);
        let tab_stops = self.tab_stops_for(buffer);
        let decorations: Vec<&crate::decorations::Decoration> = self.code_decorations(buffer).into_iter().filter(|d| d.start <= start + len && d.end >= start).collect();
        let display = if !decorations.is_empty() {
            crate::decorations::layout_line(&line_text, start, &decorations, &tab_stops, line == cursor_line).display
        } else if line_text.contains('\t') {
            tabstops::expand_line(&line_text, &tab_stops).0
        } else {
            line_text
        };
        let indent = crate::wrap::continuation_indent(&display, width);
        crate::wrap::break_line_rows(&display, width, indent, max_rows)
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_keys::app_on_file;
    use super::*;

    /// `lines` lines of `aaaa bbbb ` / `cccc dddd ` / `eeee`: three rows
    /// each at ten columns.
    fn three_row_lines(lines: usize) -> (App, super::super::test_keys::TestFile) {
        let text: String = (0..lines).map(|_| "aaaa bbbb cccc dddd eeee\n").collect();
        let (mut app, file) = app_on_file("a.txt", &text);
        app.run_cmd("view.toggle_wrap");
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
        app.run_cmd("view.toggle_wrap");
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
}
