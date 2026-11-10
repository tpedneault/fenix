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
    rows: HashMap<usize, Vec<std::ops::Range<usize>>>,
}

impl RowWalk<'_> {
    /// Display line `index`'s rows: their visual columns.
    pub(super) fn breaks(&mut self, index: usize) -> &[std::ops::Range<usize>] {
        let (app, buffer, width, cursor_line) = (self.app, self.buffer, self.width, self.cursor_line);
        let line = self.lines.get(index).copied().unwrap_or(rows::NO_LINE);
        self.rows.entry(index).or_insert_with(|| app.line_breaks(buffer, line, width, cursor_line))
    }

    fn count(&mut self, index: usize) -> usize {
        self.breaks(index).len().max(1)
    }

    /// The last row of all.
    pub(super) fn end(&mut self) -> RowPos {
        let last = self.lines.len().saturating_sub(1);
        (last, self.count(last) - 1)
    }

    /// `delta` rows on from `pos` (back, when negative), held to the
    /// first row and the last.
    pub(super) fn step(&mut self, pos: RowPos, delta: isize) -> RowPos {
        let (mut index, mut row) = (pos.0.min(self.lines.len().saturating_sub(1)), pos.1);
        row = row.min(self.count(index) - 1);
        let mut left = delta.unsigned_abs();
        if delta >= 0 {
            while left > 0 {
                let here = self.count(index) - 1 - row;
                if left <= here {
                    return (index, row + left);
                }
                if index + 1 >= self.lines.len() {
                    return (index, self.count(index) - 1);
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
                row = self.count(index) - 1;
            }
        }
        (index, row)
    }

    /// How many rows `to` is below `from`, if it's no more than `cap`.
    pub(super) fn distance(&mut self, from: RowPos, to: RowPos, cap: usize) -> Option<usize> {
        if to < from {
            return None;
        }
        let mut n = 0;
        let mut at = from;
        while at.0 < to.0 {
            n += self.count(at.0) - at.1;
            if n > cap {
                return None;
            }
            at = (at.0 + 1, 0);
        }
        n += to.1.saturating_sub(at.1);
        (n <= cap).then_some(n)
    }

    /// The row of display line `index` that visual column `col` is on.
    pub(super) fn row_of(&mut self, index: usize, col: usize) -> usize {
        let breaks = self.breaks(index);
        breaks.iter().position(|r| col < r.end).unwrap_or(breaks.len().saturating_sub(1))
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
        match walk.distance(top, cursor, visible + visible / 2) {
            Some(d) if d < visible => return top,
            Some(_) if !jumped => walk.step(cursor, -((visible - 1) as isize)),
            _ => centred(walk),
        }
    };
    // Not past the point where the last row is at the bottom.
    let end = walk.end();
    match walk.distance(target, end, visible) {
        Some(d) if d + 1 < visible => {
            let last_top = walk.step(end, -((visible - 1) as isize));
            last_top.min(target)
        }
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
        let index = walk.lines.iter().position(|&l| l == line).unwrap_or(0);
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
        let top = follow(&mut walk, (state.scroll_line, state.scroll_row), cursor, visible, jumped);
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
        let mut top = walk.step((state.scroll_line, state.scroll_row), rows);
        if rows > 0 {
            let end = walk.end();
            let last_top = walk.step(end, -(visible.max(1) as isize - 1));
            top = top.min(last_top.max((state.scroll_line, state.scroll_row)));
        }
        drop(walk);
        self.set_row_top(pane, top);
        true
    }

    /// The visual columns of `buffer`'s `line` on each of its rows,
    /// wrapped at `width` (`rows::NO_LINE`, a filler, is one row).
    pub(super) fn line_breaks(&self, buffer: BufferId, line: usize, width: usize, cursor_line: usize) -> Vec<std::ops::Range<usize>> {
        let Some(ob) = self.buffers.get(buffer) else { return std::iter::once(0..0).collect() };
        if line == rows::NO_LINE || line >= ob.buffer.visual_line_count() {
            return std::iter::once(0..usize::MAX).collect();
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
        crate::wrap::break_line(&display, width, indent)
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
        // The line's twenty rows end at row 19 (`$` is on its last
        // blank), `next` is row 20 and the empty line after it row 21:
        // centred, which the end leaves where it is.
        assert_eq!(top(&app), (0, 17));
        assert_eq!(app.focused_overlays_now(5).caret, Some((2, 9)));
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
    fn a_walk_steps_and_measures_rows_across_lines() {
        let (app, _f) = three_row_lines(3);
        let mut walk = app.row_walk(app.focused_pane_id(), app.focused_buffer_id()).unwrap();
        assert_eq!(walk.step((0, 1), 4), (1, 2));
        assert_eq!(walk.step((1, 2), -4), (0, 1));
        assert_eq!(walk.step((0, 0), -3), (0, 0));
        assert_eq!(walk.step((0, 0), 100), walk.end());
        assert_eq!(walk.distance((0, 1), (2, 0), 10), Some(5));
        assert_eq!(walk.distance((0, 1), (2, 0), 4), None);
        assert_eq!(walk.distance((2, 0), (0, 1), 10), None);
    }
}
