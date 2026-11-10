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

/// The focused pane's rows as Vim's motions by row see them: its width,
/// tab stops and decorations, taken when a key comes, so they don't
/// borrow the app.
pub(super) struct PaneRows {
    width: usize,
    tab_stops: TabStops,
    decorations: Vec<crate::decorations::Decoration>,
    cursor_line: usize,
}

/// A line as `PaneRows` lays it out: char column -> visual column, the
/// rows' visual columns, and the continuation rows' indent.
struct LaidLine {
    col_map: Vec<usize>,
    breaks: Vec<std::ops::Range<usize>>,
    indent: usize,
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
        LaidLine { breaks: crate::wrap::break_line(&display, self.width, indent), col_map, indent }
    }

    /// `at`'s line, row of it and cell on that row, and the line laid out.
    fn place(&self, buffer: &fenix_core::Buffer, at: usize) -> (usize, usize, usize, LaidLine) {
        let cursor = Cursor { char_idx: at.min(buffer.len_chars()), sticky_col: 0 };
        let (line, col) = buffer.line_col(&cursor);
        let laid = self.lay(buffer, line);
        let vcol = tabstops::visual_col(&laid.col_map, col);
        let row = laid.breaks.iter().position(|r| vcol < r.end).unwrap_or(laid.breaks.len() - 1);
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
        for _ in 0..delta.unsigned_abs() {
            if delta > 0 {
                if row + 1 < laid.breaks.len() {
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
                row = laid.breaks.len() - 1;
            } else {
                break;
            }
        }
        let cols = &laid.breaks[row];
        let indent = if row > 0 { laid.indent } else { 0 };
        let vcol = cols.start + goal.saturating_sub(indent);
        // On the row: its last char, or past the line's end on its last.
        let vcol = if row + 1 < laid.breaks.len() { vcol.min(cols.end.saturating_sub(1)) } else { vcol.min(cols.end) };
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
    /// The rows Vim's motions by row go by for the next key: the focused
    /// pane's, when it wraps, else a row a line.
    pub(super) fn sync_screen_rows(&mut self) {
        let (pane, buffer) = (self.focused_pane_id(), self.focused_buffer_id());
        let rows: std::sync::Arc<dyn fenix_vim::ScreenRows> = match self.wrap_cols(pane, buffer) {
            Some(width) => {
                let cursor_line = self.buffers.get(buffer).map_or(0, |ob| ob.buffer.line_col(&self.pane_state(pane).cursor).0);
                std::sync::Arc::new(PaneRows { width, tab_stops: self.tab_stops_for(buffer), decorations: self.code_decorations(buffer).into_iter().cloned().collect(), cursor_line })
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

    fn wrapped(text: &str, cols: usize) -> (App, super::super::test_keys::TestFile) {
        let (mut app, file) = app_on_file("a.txt", text);
        app.run_cmd("view.toggle_wrap");
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
