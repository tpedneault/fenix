//! The mouse in a pane's text and between the panes. A click puts the
//! cursor on the glyph under it; a drag selects -- by characters, or by
//! words after a double click, lines after a triple, a block with Alt
//! held -- and Shift-click extends what's selected. A divider dragged
//! resizes its split, a tab dragged onto another pane moves there (or,
//! let go near the pane's edge, splits it), a right click opens a menu of
//! what can be done there, and the pointer resting on a symbol shows its
//! hover. Pages, listings, terminals and surfaces keep the mouse they
//! had: all of this is for text.

use super::*;
use crate::mouse::{char_col_at, drop_zone, ClickCounter, DropZone, Menu, MenuItem};
use fenix_window::Divider;

/// How far from a divider, in pixels, the pointer still grabs it.
const DIVIDER_SLOP: f32 = 3.0;

/// A click that moves the cursor this many lines goes on the jumplist,
/// as a jump would; a nearer one doesn't.
pub(super) const CLICK_JUMP_LINES: usize = 10;

/// How often a drag held past a pane's top or bottom scrolls it on.
const DRAG_SCROLL_EVERY: Duration = Duration::from_millis(40);

/// `editor.hover_delay_ms`'s default.
const HOVER_DELAY_MS: i64 = 500;

/// What the mouse is in the middle of.
#[derive(Default)]
pub(super) struct MouseState {
    clicks: ClickCounter,
    /// The left button, held down in a pane's text.
    drag: Option<TextDrag>,
    /// The divider being dragged.
    split: Option<Divider>,
    /// The right click's menu, while it's open.
    pub(super) menu: Option<Menu<MenuAction>>,
    /// Where the pointer rests, for the hover.
    rest: Option<Rest>,
    /// The hover the pointer opened: where, and the problems there. The
    /// language server's answer is `App::lsp_hover`, as `K`'s is.
    pub(super) hover: Option<MouseHover>,
    /// When a drag past the pane's edge last scrolled it.
    scrolled_at: Option<Instant>,
    /// Alt+Shift held down in a pane's text (standard keys): the pane and
    /// the line a column of cursors starts on.
    column: Option<(fenix_window::WindowId, usize)>,
}

/// A selection the mouse is making in `pane`.
#[derive(Debug, Clone, Copy)]
struct TextDrag {
    pane: fenix_window::WindowId,
    unit: Unit,
    /// Where it started: a char, or the word or line double or triple
    /// clicked.
    anchor: usize,
    /// The double-clicked word's end (exclusive).
    anchor_end: usize,
    /// Whether it's selecting yet: a plain click only does once the
    /// pointer moves off the char it went down on.
    started: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Unit {
    Char,
    Word,
    Line,
    Block,
}

/// The pointer still over char `at`, since `since`.
#[derive(Debug, Clone, Copy)]
struct Rest {
    at: usize,
    buffer: BufferId,
    since: Instant,
    /// The glyph's bottom-left corner, which the hover opens under.
    anchor: (f32, f32),
    asked: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct MouseHover {
    pub(super) anchor: (f32, f32),
    /// Each line of the problems at the glyph, and whether it's an error's.
    pub(super) problems: Vec<(String, bool)>,
}

/// What a context menu's row does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MenuAction {
    Definition,
    References,
    Rename,
    CodeActions,
    Hover,
    Cut,
    Copy,
    Paste,
    SelectAll,
    ToggleFold,
    Breakpoint,
    /// The test the cursor is in run, or shown on the tests page (§9.1).
    RunTest,
    DebugTest,
    ShowTest,
    AskAgent,
    Tab(TabAction, fenix_window::WindowId, BufferId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TabAction {
    Close,
    CloseOthers,
    CloseRight,
    SplitRight,
    SplitDown,
    CopyPath,
    Reveal,
}

/// Where the pointer is in a pane's text.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct TextHit {
    pub(super) buffer: BufferId,
    pub(super) line: usize,
    /// The char column, at most the line's length.
    pub(super) col: usize,
    /// Past the line's last char, or under the buffer's last line.
    pub(super) past_end: bool,
    /// Over the gutter.
    pub(super) gutter: bool,
    /// The glyph's bottom-left corner, in window pixels.
    pub(super) anchor: (f32, f32),
}

impl App {
    pub(super) fn cell_size(&self) -> (f32, f32) {
        match &self.text {
            Some(t) => (t.char_width(), t.line_height()),
            None => (text::CHAR_WIDTH, text::LINE_HEIGHT),
        }
    }

    fn geometry_at(&self, window_width: f32, window_height: f32) -> FrameGeometry {
        let (sidebar_px, terminal_h, modeline_top) = self.frame_metrics(window_height);
        self.frame_geometry(window_width, sidebar_px, terminal_h, modeline_top)
    }

    /// The rows of text `pane` has room for, as the window is now.
    pub(super) fn pane_text_rows(&self, pane: fenix_window::WindowId) -> Option<usize> {
        let (w, h) = self.window_size()?;
        let rect = self.geometry_at(w, h).panes.into_iter().find(|(p, _)| *p == pane)?.1;
        let (_, line_height) = self.cell_size();
        Some(text::lines_that_fit(pane_content_rect(rect, line_height, self.pane_has_breadcrumb(pane)).h, line_height))
    }

    fn window_size(&self) -> Option<(f32, f32)> {
        self.gpu.as_ref().map(|gpu| (gpu.size.width as f32, gpu.size.height as f32))
    }

    /// The buffer `pane` shows, when it's text the mouse places the cursor
    /// in: a file, a table, a task's output -- not a page, Home, a
    /// listing, a terminal or a surface, which have their own mouse.
    pub(super) fn mouse_text_buffer(&self, pane: fenix_window::WindowId) -> Option<BufferId> {
        let buffer = *self.windows().content(pane)?;
        let ob = self.buffers.get(buffer)?;
        let text = matches!(ob.kind, BufferKind::Text | BufferKind::Table | BufferKind::TaskOutput | BufferKind::WorkspaceEdit | BufferKind::Explorer | BufferKind::Prompt);
        let own_mouse = self.is_page_buffer(buffer) || self.terminal_buffers.contains_key(&buffer) || self.code_surface(buffer).is_some() || self.explorer_in_pane(pane).is_some();
        (text && !own_mouse && self.main_view == MainView::Editor).then_some(buffer)
    }

    /// Where `pos` is in the text of `pane`, laid out at `rect` (its whole
    /// rect, chrome and all): through the scroll (a row at a time, or
    /// part of one mid-ease), the folds, a side-by-side's filler rows, the
    /// sticky headers, the gutter, tabs and the decorations' columns. A
    /// point above or below the text is the line it would be on, held to
    /// the buffer's -- which is how a drag past the edge scrolls.
    pub(super) fn text_hit(&self, pane: fenix_window::WindowId, rect: fenix_window::Rect, pos: (f32, f32)) -> Option<TextHit> {
        let buffer = self.mouse_text_buffer(pane)?;
        let ob = self.buffers.get(buffer)?;
        let (char_width, line_height) = self.cell_size();
        let content = pane_content_rect(rect, line_height, self.pane_has_breadcrumb(pane));
        let state = *self.pane_state(pane);
        let lines = self.pane_display_lines(pane, buffer);
        if lines.is_empty() {
            return None;
        }
        let base = state.rendered_scroll.floor().max(0.0) as usize;
        let frac = state.rendered_scroll - base as f32;
        let row = ((pos.1 - content.y - text::PAD_TOP) / line_height + frac).floor() as isize;
        let gutter_px = self.gutter_chars(ob) as f32 * char_width;
        let x = pos.0 - content.x - text::PAD_LEFT - gutter_px;
        let cursor_line = ob.buffer.line_col(&state.cursor).0;
        if self.row_cols(pane, buffer).is_some() {
            return self.wrapped_text_hit(pane, buffer, content, row, x);
        }
        // Sticky scroll's headers sit over the top rows, unscrolled.
        let folds_active = lines.len() != ob.buffer.line_count();
        let sticky = if row >= 0 && !folds_active && ob.kind == BufferKind::Text && ob.syntax.is_some() && self.row_cols(pane, buffer).is_none() {
            let cursor_row = lines.iter().position(|&l| l == cursor_line).and_then(|r| r.checked_sub(base));
            self.sticky_headers(buffer, base, cursor_row)
        } else {
            Vec::new()
        };
        let index = base as isize + row;
        let (line, scroll_col) = match usize::try_from(row).ok().and_then(|r| sticky.get(r)) {
            Some(&line) => (line, 0),
            None => {
                let at = index.clamp(0, lines.len() as isize - 1) as usize;
                // A filler row has no line: the nearest real one below it,
                // else above.
                let line = lines[at..].iter().chain(lines[..at].iter().rev()).copied().find(|&l| l != side_by_side::FILLER)?;
                (line, state.scroll_col)
            }
        };
        let (col_map, bidi) = self.line_col_map_and_order(buffer, line, cursor_line, scroll_col);
        let gutter = x < 0.0;
        // The row it's on, through the pane's map of them; in right-to-
        // left text, the character drawn in the cell clicked.
        let map = super::rows::RowMap::lines(&[line], scroll_col);
        let cell = (x / char_width).floor().max(0.0) as usize;
        let (_, vcol) = map.at(0, bidi.as_ref().map_or(cell, |b| b.char_at(cell)))?;
        let (col, past_end) = char_col_at(&col_map, vcol);
        let shown_row = row.clamp(0, text::lines_that_fit(content.h, line_height) as isize);
        let glyph_col = map.locate(line, tabstops::visual_col(&col_map, col)).map_or(0, |(_, x)| bidi.as_ref().map_or(x, |b| b.caret(x, true)));
        let glyph_x = content.x + text::PAD_LEFT + gutter_px + glyph_col as f32 * char_width;
        let glyph_bottom = content.y + text::PAD_TOP + (shown_row as f32 + 1.0 - frac) * line_height;
        Some(TextHit { buffer, line, col, past_end: past_end || index >= lines.len() as isize, gutter, anchor: (glyph_x, glyph_bottom) })
    }

    /// Char column -> visual column for `buffer`'s `line` as it's drawn:
    /// its tabs expanded, its decorations laid out (as on the cursor's
    /// line, `cursor_line`).
    pub(super) fn line_col_map(&self, buffer: BufferId, line: usize, cursor_line: usize) -> Vec<usize> {
        self.line_drawn(buffer, line, cursor_line).1
    }

    /// `line_col_map`, and for a line with right-to-left text where each
    /// character of it is drawn, scrolled sideways to `scroll_col` (#17).
    fn line_col_map_and_order(&self, buffer: BufferId, line: usize, cursor_line: usize, scroll_col: usize) -> (Vec<usize>, Option<crate::bidi::VisualOrder>) {
        let (display, col_map) = self.line_drawn(buffer, line, cursor_line);
        let order = if crate::bidi::has_rtl(&display) {
            crate::bidi::visual_order(&display.chars().skip(scroll_col).take(super::MAX_ROW_CHARS).collect::<String>())
        } else {
            None
        };
        (col_map, order)
    }

    /// `line` as drawn, and its char column -> visual column map.
    fn line_drawn(&self, buffer: BufferId, line: usize, cursor_line: usize) -> (String, Vec<usize>) {
        let Some(ob) = self.buffers.get(buffer) else { return (String::new(), Vec::new()) };
        let start = ob.buffer.line_start_char(line);
        let len = ob.buffer.line_len(line);
        let line_text = ob.buffer.text_range(start, start + len);
        let tab_stops = self.tab_stops_for(buffer);
        let decorations: Vec<&crate::decorations::Decoration> = self.code_decorations(buffer).into_iter().filter(|d| d.start <= start + len && d.end >= start).collect();
        if decorations.is_empty() {
            tabstops::expand_line(&line_text, &tab_stops)
        } else {
            let laid = crate::decorations::layout_line(&line_text, start, &decorations, &tab_stops, line == cursor_line);
            (laid.display, laid.col_map)
        }
    }

    /// `text_hit` in a pane shown by rows (wrapped, or with rows that
    /// aren't text): `row` (from the top of `content`, negative above it)
    /// and `x` (pixels from the text's left edge) through the pane's rows.
    /// Right of a row that goes on below is that row's last char. Above or
    /// below the rows shown: wrapped, the line before the first or after
    /// the last; not wrapped, as a row a line has it, a line on for each
    /// row past the edge, through the sideways scroll. A row that isn't
    /// text (a code lens's) is no place in the text.
    fn wrapped_text_hit(&self, pane: fenix_window::WindowId, buffer: BufferId, content: fenix_window::Rect, row: isize, x: f32) -> Option<TextHit> {
        let ob = self.buffers.get(buffer)?;
        let (char_width, line_height) = self.cell_size();
        let state = *self.pane_state(pane);
        let fit = text::lines_that_fit(content.h, line_height);
        let top = state.rendered_scroll.floor().max(0.0) as usize;
        let map = self.pane_rows(pane, buffer, top, state.scroll_row, fit + 1);
        let last = map.len().checked_sub(1)?;
        let cell = (x / char_width).floor().max(0.0) as usize;
        let display = self.pane_display_lines(pane, buffer);
        // Off the rows: the display line `rows` on from `edge`'s line.
        let off = |edge: usize, rows: isize| -> Option<(usize, bool)> {
            let line = map.row(edge)?.line;
            let index = display.partition_point(|&l| l < line) as isize + rows;
            let at = index.clamp(0, display.len() as isize - 1) as usize;
            Some((display[at], index >= display.len() as isize))
        };
        // Unwrapped, the columns go through the sideways scroll.
        let wrapped = self.row_cols(pane, buffer).is_some_and(|w| w != super::rows::UNBOUNDED);
        let scroll_col = if wrapped { 0 } else { state.scroll_col };
        let (line, vcol, past) = match usize::try_from(row).ok().filter(|&r| r <= last) {
            Some(r) => match map.row(r).map(|row| row.kind) {
                Some(super::rows::RowKind::Virtual(_)) => return None,
                // Past a row's end, on a line that goes on below: its
                // last char (`RowMap::at` holds it there).
                _ => {
                    let (line, vcol) = map.at(r, self.row_logical_cell(&map, r, buffer, ob, state.cursor, cell))?;
                    (line, vcol, false)
                }
            },
            // A pane that doesn't wrap: as a row a line has it.
            None if !wrapped => {
                let (line, past) = if row < 0 { off(0, row)? } else { off(last, row - last as isize)? };
                (line, scroll_col + cell, past)
            }
            // Wrapped: the line before the first shown, or after the last.
            None => {
                let (line, past) = if row < 0 { off(0, -1)? } else { off(last, 1)? };
                (line, cell, past)
            }
        };
        let cursor_line = ob.buffer.line_col(&state.cursor).0;
        let col_map = self.line_col_map(buffer, line, cursor_line);
        let (col, past_end) = char_col_at(&col_map, vcol);
        // Under the glyph it lands on, for a hover's card.
        let (glyph_row, glyph_col) = map.locate(line, tabstops::visual_col(&col_map, col)).unwrap_or((row.clamp(0, last as isize) as usize, cell));
        let glyph_x = content.x + text::PAD_LEFT + self.gutter_chars(ob) as f32 * char_width + glyph_col as f32 * char_width;
        let glyph_bottom = content.y + text::PAD_TOP + (glyph_row as f32 + 1.0) * line_height;
        Some(TextHit { buffer, line, col, past_end: past_end || past, gutter: x < 0.0, anchor: (glyph_x, glyph_bottom) })
    }

    /// Cell `cell` of row `r` of `map` as its line's own order counts
    /// it: in right-to-left text, the character drawn there (#17).
    fn row_logical_cell(&self, map: &super::rows::RowMap, r: usize, buffer: BufferId, ob: &super::OpenBuffer, cursor: Cursor, cell: usize) -> usize {
        let Some(line) = map.row(r).map(|row| row.line) else { return cell };
        let (display, _) = self.line_drawn(buffer, line, ob.buffer.line_col(&cursor).0);
        if !crate::bidi::has_rtl(&display) {
            return cell;
        }
        let lines = HashMap::from([(line, display)]);
        match super::rows::bidi_rows(map, &lines).get(&r) {
            Some((first, order)) if cell >= *first => first + order.char_at(cell - first),
            _ => cell,
        }
    }

    /// The char `hit` puts the cursor on: past the line's end is its last
    /// char, or after it in Insert.
    fn hit_char(&self, hit: &TextHit) -> usize {
        let Some(ob) = self.buffers.get(hit.buffer) else { return 0 };
        let len = ob.buffer.line_len(hit.line);
        let last = if self.vim.is_inserting() { len } else { len.saturating_sub(1) };
        ob.buffer.line_start_char(hit.line) + hit.col.min(last)
    }

    /// Puts `pane`'s cursor on char `at`, its column remembered for `j`/`k`.
    pub(super) fn put_cursor(&mut self, pane: fenix_window::WindowId, at: usize) {
        let Some(&buffer) = self.windows().content(pane) else { return };
        let Some(ob) = self.buffers.get(buffer) else { return };
        let at = at.min(ob.buffer.len_chars());
        let mut cursor = Cursor { char_idx: at, sticky_col: 0 };
        cursor.sticky_col = ob.buffer.line_col(&cursor).1;
        self.pane_state_mut(pane).cursor = cursor;
        self.vim.forget_row_goals();
    }

    /// Leaves Insert (or Replace) the way Escape does, before a selection
    /// starts.
    pub(super) fn mouse_leave_insert(&mut self) {
        // Standard keys don't leave Insert on Escape: out, where it is.
        if self.vim.policy() == fenix_vim::EditPolicy::Standard {
            let id = self.focused_buffer_id();
            if let Some(ob) = self.buffers.get_mut(id) {
                self.vim.leave_insert_here(&mut ob.buffer);
            }
            return;
        }
        if self.vim.is_inserting() {
            let (id, pane) = (self.focused_buffer_id(), self.focused_pane_id());
            if let (Some(ob), Some(state)) = (self.buffers.get_mut(id), self.workspaces.active_pane_states_mut().get_mut(&pane)) {
                self.vim.handle_key(&mut ob.buffer, &mut state.cursor, KeyPress::named(FenixNamedKey::Escape));
            }
        }
    }

    /// The left button down in `pane`'s text (`pane` already focused):
    /// the cursor to the glyph, a word on a double click, a line on a
    /// triple, the selection stretched to it with Shift.
    pub(super) fn text_press(&mut self, pane: fenix_window::WindowId, rect: fenix_window::Rect, pos: (f32, f32), now: Instant) -> bool {
        // On a code lens's title: that lens run.
        if let Some((pick, _, _)) = self.lens_at(pane, rect, pos) {
            // Once a click: not again on its double or triple.
            if self.mouse.clicks.press(now, pos) == 1 {
                self.run_lens(&pick);
            }
            return true;
        }
        let Some(hit) = self.text_hit(pane, rect, pos) else { return false };
        let clicks = self.mouse.clicks.press(now, pos);
        let (shift, alt) = (self.modifiers.shift_key(), self.modifiers.alt_key());
        self.completion = None;
        self.close_mouse_hover();
        let cursor = self.pane_state(pane).cursor;
        // Standard keys: Alt+click (or Ctrl+click) adds a cursor, Alt+Shift
        // starts a column of them, and the other one goes to a definition.
        if self.standard_keys() && clicks == 1 {
            let ctrl = self.modifiers.control_key();
            let by_ctrl = self.config.text_value("editor.multi_cursor_modifier") == Some("ctrl");
            let (adds, defines) = if by_ctrl { (ctrl, alt) } else { (alt, ctrl) };
            let at = self.hit_char(&hit);
            if alt && shift {
                if self.vim.mode() == Mode::Visual {
                    self.vim.exit_visual_mode(&cursor);
                }
                self.mouse.column = Some((pane, hit.line));
                self.column_carets(hit.line, hit.line, hit.col);
                return true;
            }
            if adds && !shift {
                if self.vim.mode() == Mode::Visual {
                    self.vim.exit_visual_mode(&cursor);
                }
                self.toggle_caret_at(at);
                return true;
            }
            if defines && !shift {
                // A file's name under it opens the file, as `gf` does;
                // anything else goes to its definition.
                self.clear_carets();
                self.put_cursor(pane, at);
                if self.link_under_cursor() {
                    self.follow_link_under_cursor();
                } else {
                    self.request_goto_definition();
                }
                return true;
            }
        }
        // A plain click: back to one cursor.
        if !shift {
            self.clear_carets();
        }
        if shift && clicks == 1 {
            let (kind, anchor) = if self.vim.mode() == Mode::Visual { (self.vim.visual_kind(), self.vim.visual_anchor()) } else { (VisualKind::Char, cursor.char_idx) };
            self.mouse_leave_insert();
            let at = self.hit_char(&hit);
            self.vim.select(kind, anchor);
            self.put_cursor(pane, at);
            let unit = match kind {
                VisualKind::Char => Unit::Char,
                VisualKind::Line => Unit::Line,
                VisualKind::Block => Unit::Block,
            };
            self.mouse.drag = Some(TextDrag { pane, unit, anchor, anchor_end: anchor, started: true });
            return true;
        }
        match clicks {
            1 => {
                if self.vim.mode() == Mode::Visual {
                    self.vim.exit_visual_mode(&cursor);
                }
                self.vim.cancel_pending();
                let at = self.hit_char(&hit);
                let from = self.buffers.get(hit.buffer).map_or(hit.line, |ob| ob.buffer.line_col(&cursor).0);
                if from.abs_diff(hit.line) >= CLICK_JUMP_LINES {
                    self.record_jump(JumpEntry { buffer: hit.buffer, char_idx: cursor.char_idx });
                }
                self.put_cursor(pane, at);
                let unit = if alt { Unit::Block } else { Unit::Char };
                self.mouse.drag = Some(TextDrag { pane, unit, anchor: at, anchor_end: at, started: false });
            }
            2 => {
                self.mouse_leave_insert();
                let at = self.hit_char(&hit);
                let word = self.buffers.get(hit.buffer).map_or(at..at, |ob| fenix_vim::word_at(&ob.buffer, at, self.vim.iskeyword_extra()));
                let (start, end) = if word.is_empty() { (at, at + 1) } else { (word.start, word.end) };
                self.vim.select(VisualKind::Char, start);
                // Standard keys' selection ends before the cursor; Vim's
                // takes the cursor's character.
                let last = if self.vim.policy() == fenix_vim::EditPolicy::Standard { end } else { end - 1 };
                self.put_cursor(pane, last);
                self.mouse.drag = Some(TextDrag { pane, unit: Unit::Word, anchor: start, anchor_end: end, started: true });
            }
            _ => {
                self.mouse_leave_insert();
                let start = self.buffers.get(hit.buffer).map_or(0, |ob| ob.buffer.line_start_char(hit.line));
                self.vim.select(VisualKind::Line, start);
                let at = self.hit_char(&hit);
                self.put_cursor(pane, at);
                self.mouse.drag = Some(TextDrag { pane, unit: Unit::Line, anchor: start, anchor_end: start, started: true });
            }
        }
        true
    }

    /// The pointer moved to `pos` with the left button held: the divider
    /// or the selection follows it.
    pub(super) fn mouse_drag_to(&mut self, pos: (f32, f32), window_width: f32, window_height: f32) {
        if self.terminal_mouse_drag(pos) {
            return;
        }
        if let Some(divider) = self.mouse.split {
            if self.windows_mut().set_split_ratio(divider.index, divider.ratio_at(pos.0, pos.1)) {
                self.wake_caret();
            }
            return;
        }
        if let Some((pane, from_line)) = self.mouse.column {
            let geometry = self.geometry_at(window_width, window_height);
            if let Some(&(_, rect)) = geometry.panes.iter().find(|(p, _)| *p == pane) {
                if let Some(hit) = self.text_hit(pane, rect, pos) {
                    self.column_carets(from_line, hit.line, hit.col);
                    self.wake_caret();
                }
            }
            return;
        }
        let Some(mut drag) = self.mouse.drag else { return };
        let geometry = self.geometry_at(window_width, window_height);
        let Some(&(_, rect)) = geometry.panes.iter().find(|(p, _)| *p == drag.pane) else {
            self.mouse.drag = None;
            return;
        };
        if self.focused_pane_id() != drag.pane {
            self.mouse.drag = None;
            return;
        }
        let Some(hit) = self.text_hit(drag.pane, rect, pos) else { return };
        if !drag.started {
            if self.hit_char(&hit) == drag.anchor {
                return;
            }
            self.mouse_leave_insert();
            let kind = if drag.unit == Unit::Block { VisualKind::Block } else { VisualKind::Char };
            self.vim.select(kind, drag.anchor);
            drag.started = true;
            self.mouse.drag = Some(drag);
        }
        if self.vim.mode() != Mode::Visual {
            // Something else ended the selection: a key, a command.
            self.mouse.drag = None;
            return;
        }
        let at = self.hit_char(&hit);
        match drag.unit {
            Unit::Char | Unit::Block | Unit::Line => self.put_cursor(drag.pane, at),
            Unit::Word => {
                let word = self.buffers.get(hit.buffer).map_or(at..at + 1, |ob| fenix_vim::word_at(&ob.buffer, at, self.vim.iskeyword_extra()));
                let (start, end) = if word.is_empty() { (at, at + 1) } else { (word.start, word.end) };
                // Standard keys' selection doesn't take the cursor's
                // character: its ends are one further out.
                let inclusive = usize::from(self.vim.policy() != fenix_vim::EditPolicy::Standard);
                if start < drag.anchor {
                    self.vim.select(VisualKind::Char, drag.anchor_end - inclusive);
                    self.put_cursor(drag.pane, start);
                } else {
                    self.vim.select(VisualKind::Char, drag.anchor);
                    self.put_cursor(drag.pane, end.max(drag.anchor + 1) - inclusive);
                }
            }
        }
        self.wake_caret();
    }

    /// The left button let go.
    pub(super) fn mouse_release(&mut self) {
        self.mouse.column = None;
        // Standard keys: typing where a click put you in a file, browsing
        // where it put you in a listing.
        self.settle_keys();
        self.terminal_mouse_release();
        self.mouse.drag = None;
        self.mouse.scrolled_at = None;
        if self.mouse.split.take().is_some() {
            self.resync_terminal_cols();
            self.wake_caret();
        }
    }

    /// A drag held still above or below its pane keeps scrolling it, a
    /// line every `DRAG_SCROLL_EVERY`. When to look again, while it is.
    pub(super) fn drag_scroll_tick(&mut self, now: Instant) -> Option<Instant> {
        let drag = self.mouse.drag.filter(|d| d.started)?;
        let pos = self.cursor_pos?;
        let (window_width, window_height) = self.window_size()?;
        let (_, line_height) = self.cell_size();
        let geometry = self.geometry_at(window_width, window_height);
        let &(_, rect) = geometry.panes.iter().find(|(p, _)| *p == drag.pane)?;
        let content = pane_content_rect(rect, line_height, self.pane_has_breadcrumb(drag.pane));
        if pos.1 >= content.y && pos.1 < content.y + content.h {
            self.mouse.scrolled_at = None;
            return None;
        }
        let due = self.mouse.scrolled_at.map_or(now, |at| at + DRAG_SCROLL_EVERY);
        if now >= due {
            self.mouse.scrolled_at = Some(now);
            self.mouse_drag_to(pos, window_width, window_height);
            return Some(now + DRAG_SCROLL_EVERY);
        }
        Some(due)
    }

    /// The divider under `pos`, if one is.
    fn divider_at(&self, geometry: &FrameGeometry, pos: (f32, f32)) -> Option<Divider> {
        if self.main_view != MainView::Editor || self.windows().window_count() < 2 {
            return None;
        }
        self.windows().dividers(geometry.pane_area).into_iter().find(|d| d.near(pos.0, pos.1, DIVIDER_SLOP))
    }

    /// Whether a divider is being dragged.
    pub(super) fn mouse_resizing(&self) -> bool {
        self.mouse.split.is_some()
    }

    /// The left button down on a divider: dragging it resizes its split.
    pub(super) fn divider_press_at(&mut self, pos: (f32, f32), window_width: f32, window_height: f32) -> bool {
        let geometry = self.geometry_at(window_width, window_height);
        // A tab, or the sidebar, is in front of a divider's slop.
        if geometry.tabs.iter().any(|(_, tab)| tab.body.contains_point(pos.0, pos.1)) || geometry.sidebar_rect.is_some_and(|r| r.contains_point(pos.0, pos.1)) {
            return false;
        }
        self.mouse.split = self.divider_at(&geometry, pos);
        self.mouse.split.is_some()
    }

    /// The pointer's shape over a divider, or while dragging one.
    pub(super) fn divider_icon(&self, geometry: &FrameGeometry, pos: (f32, f32)) -> Option<winit::window::CursorIcon> {
        let divider = self.mouse.split.or_else(|| {
            if geometry.tabs.iter().any(|(_, tab)| tab.body.contains_point(pos.0, pos.1)) {
                return None;
            }
            self.divider_at(geometry, pos)
        })?;
        Some(match divider.kind {
            SplitKind::Vertical => winit::window::CursorIcon::ColResize,
            SplitKind::Horizontal => winit::window::CursorIcon::RowResize,
        })
    }

    /// A tab dragged from `from`'s strip let go at `pos`: onto another
    /// pane's strip or text, it moves there; near a pane's edge, it splits
    /// that pane and goes in the new half. The agents' dock takes no tabs.
    pub(super) fn tab_dropped_at(&mut self, from: fenix_window::WindowId, id: BufferId, pos: (f32, f32), window_width: f32, window_height: f32) -> bool {
        let geometry = self.geometry_at(window_width, window_height);
        if let Some((over, onto)) = geometry.tabs.iter().find(|(_, tab)| tab.body.contains_point(pos.0, pos.1)).map(|(p, t)| (*p, t.buffer)) {
            if over == from || self.agents_is_dock_pane(over) {
                return false;
            }
            self.move_tab_to_pane(from, id, over);
            if Some(onto) != self.workspaces.active_workspace().home {
                self.move_tab_onto(over, id, onto);
            }
            return true;
        }
        let Some(ScrollTarget::Pane(over)) = hit_test(&geometry, pos) else { return false };
        let Some(&(_, rect)) = geometry.panes.iter().find(|(p, _)| *p == over) else { return false };
        if self.agents_is_dock_pane(over) || self.pane_titles.contains_key(&over) {
            return false;
        }
        match drop_zone(rect, pos) {
            // Its own pane's top edge is just under the strip it left: too
            // near to mean a split.
            DropZone::Middle | DropZone::Top if over == from => false,
            DropZone::Middle => {
                self.move_tab_to_pane(from, id, over);
                true
            }
            zone => {
                // Its only tab, split off beside itself, would leave Home.
                if over == from && self.pane_tab_order(from).iter().filter(|&&b| Some(b) != self.workspaces.active_workspace().home).count() < 2 {
                    return false;
                }
                self.split_pane_with_tab(from, id, over, zone);
                true
            }
        }
    }

    /// `id`'s tab leaves `from` and opens in `to`, in front.
    pub(super) fn move_tab_to_pane(&mut self, from: fenix_window::WindowId, id: BufferId, to: fenix_window::WindowId) {
        self.tab_leaves(from, id);
        self.windows_mut().focus(to);
        self.sidebar_focused = false;
        self.terminal_focused = false;
        self.set_pane_content(to, id);
        self.refresh_project_root();
        self.wake_caret();
    }

    /// `id`'s tab goes from `pane`'s strip, its neighbour (or Home) shown
    /// if it was in front -- moved off, not closed: `SPC b u` doesn't
    /// bring it back.
    fn tab_leaves(&mut self, pane: fenix_window::WindowId, id: BufferId) {
        let order = self.pane_tab_order(pane);
        self.close_pane_tab(pane, id);
        if let Some(closed) = self.workspaces.active_workspace_mut().closed_tabs.get_mut(&pane) {
            if closed.last().is_some_and(|(b, _)| *b == id) {
                closed.pop();
            }
        }
        if self.windows().content(pane) == Some(&id) {
            let next = tabs::tab_after_closing(&order, id).filter(|&b| b != id).unwrap_or_else(|| self.ensure_workspace_home());
            self.set_pane_content(pane, next);
        }
    }

    /// Splits `to` on the `zone` side, `id`'s tab moving from `from` into
    /// the new pane.
    fn split_pane_with_tab(&mut self, from: fenix_window::WindowId, id: BufferId, to: fenix_window::WindowId, zone: DropZone) {
        let cursor = if self.windows().content(from) == Some(&id) { self.pane_state(from).cursor } else { self.buffers.get(id).map_or(Cursor::at_start(), |ob| ob.cursor) };
        self.tab_leaves(from, id);
        self.windows_mut().focus(to);
        let kind = if matches!(zone, DropZone::Left | DropZone::Right) { SplitKind::Vertical } else { SplitKind::Horizontal };
        let new_pane = if matches!(zone, DropZone::Right | DropZone::Bottom) { self.windows_mut().split(kind, id) } else { self.windows_mut().split_before(kind, id) };
        self.workspaces.active_pane_states_mut().insert(new_pane, PaneState::seeded_at(cursor));
        self.workspaces.active_pane_tabs_mut().insert(new_pane, vec![id]);
        self.sidebar_focused = false;
        self.terminal_focused = false;
        self.refresh_project_root();
        self.wake_caret();
    }

    /// The right button down at `pos`: a menu for the text or the tab
    /// under it.
    pub(super) fn right_click_at(&mut self, pos: (f32, f32), window_width: f32, window_height: f32) -> bool {
        let geometry = self.geometry_at(window_width, window_height);
        match hit_test(&geometry, pos) {
            Some(ScrollTarget::SwitchTab(pane, buffer) | ScrollTarget::CloseTab(pane, buffer)) => {
                self.close_mouse_hover();
                self.mouse.menu = Some(Menu::new(self.tab_menu_items(pane, buffer), pos));
                self.wake_caret();
                true
            }
            Some(ScrollTarget::Pane(pane)) => {
                let Some(&(_, rect)) = geometry.panes.iter().find(|(p, _)| *p == pane) else { return false };
                let (_, line_height) = self.cell_size();
                if pos.1 < pane_content_rect(rect, line_height, self.pane_has_breadcrumb(pane)).y {
                    return false;
                }
                let Some(hit) = self.text_hit(pane, rect, pos) else { return false };
                self.windows_mut().focus(pane);
                self.sidebar_focused = false;
                self.terminal_focused = false;
                self.unfocus_terminal_buffer();
                self.completion = None;
                self.close_mouse_hover();
                let at = self.hit_char(&hit);
                let cursor = self.pane_state(pane).cursor;
                let inside = self.vim.mode() == Mode::Visual
                    && self.buffers.get(hit.buffer).and_then(|ob| self.vim.visual_selection_range(&ob.buffer, &cursor)).is_some_and(|(range, _)| range.contains(&at));
                if !inside {
                    if self.vim.mode() == Mode::Visual {
                        self.vim.exit_visual_mode(&cursor);
                    }
                    self.vim.cancel_pending();
                    self.put_cursor(pane, at);
                }
                self.mouse.menu = Some(Menu::new(self.text_menu_items(), pos));
                self.wake_caret();
                true
            }
            _ => false,
        }
    }

    /// The text menu, for the focused pane where its cursor is.
    pub(super) fn text_menu_items(&self) -> Vec<MenuItem<MenuAction>> {
        // The keys in use, as the scheme says them.
        let key = |id: &str| self.key_for(id).unwrap_or_default();
        let item = |label: &str, key: String, action| MenuItem { label: label.to_string(), key, action };
        let ob = self.open();
        let selecting = self.vim.mode() == Mode::Visual;
        let editable = ob.kind.tracks_unsaved_changes() || ob.kind == BufferKind::Prompt;
        let mut items = vec![
            item("Go to definition", "gd".into(), MenuAction::Definition),
            item("Find references", "gr".into(), MenuAction::References),
            item("Rename", key("code.lsp_rename"), MenuAction::Rename),
            item("Code actions…", key("code.lsp_code_action"), MenuAction::CodeActions),
            item("Hover", "K".into(), MenuAction::Hover),
        ];
        if selecting && editable {
            items.push(item("Cut", "d".into(), MenuAction::Cut));
        }
        if selecting {
            items.push(item("Copy", "y".into(), MenuAction::Copy));
        }
        if editable {
            items.push(item("Paste", if selecting { "p" } else { "P" }.into(), MenuAction::Paste));
        }
        items.push(item("Select all", "ggVG".into(), MenuAction::SelectAll));
        let id = self.focused_buffer_id();
        let line = ob.buffer.line_col(&self.cursor()).0;
        let closed = self.code_folds.get(&id).cloned().unwrap_or_default();
        if crate::folds::target(&self.fold_ranges(id), &closed, line).is_some() {
            items.push(item("Toggle fold", "za".into(), MenuAction::ToggleFold));
        }
        if ob.buffer.path().is_some() {
            items.push(item("Toggle breakpoint", key("debug.toggle_breakpoint"), MenuAction::Breakpoint));
        }
        // On a test's lines, beside it (§9.1).
        if self.tests_test_here().is_some() {
            items.push(item("Run test", key("tests.run_here"), MenuAction::RunTest));
            items.push(item("Debug test", key("tests.debug_here"), MenuAction::DebugTest));
            items.push(item("Show in Tests", key("tests.open"), MenuAction::ShowTest));
        }
        items.push(item("Ask the agent about this", key("agents.ask_selection"), MenuAction::AskAgent));
        items
    }

    /// The menu for `buffer`'s tab in `pane`.
    fn tab_menu_items(&self, pane: fenix_window::WindowId, buffer: BufferId) -> Vec<MenuItem<MenuAction>> {
        // The keys in use, as the scheme says them.
        let key = |id: &str| self.key_for(id).unwrap_or_default();
        let item = |label: &str, key: String, action| MenuItem { label: label.to_string(), key, action: MenuAction::Tab(action, pane, buffer) };
        let mut items = Vec::new();
        if self.workspaces.active_workspace().home != Some(buffer) {
            items.push(item("Close", key("tab.close"), TabAction::Close));
        }
        items.push(item("Close others", key("tab.close_others"), TabAction::CloseOthers));
        items.push(item("Close to the right", String::new(), TabAction::CloseRight));
        items.push(item("Split right with it", key("window.split_vertical"), TabAction::SplitRight));
        items.push(item("Split down with it", key("window.split_horizontal"), TabAction::SplitDown));
        if self.buffers.get(buffer).is_some_and(|ob| ob.buffer.path().is_some()) {
            items.push(item("Copy path", key("file.yank_path"), TabAction::CopyPath));
            items.push(item("Reveal in explorer", key("explorer.reveal_current"), TabAction::Reveal));
        }
        items
    }

    /// The open menu's box, in a window this size.
    fn menu_rect(&self, window_width: f32, window_height: f32) -> Option<fenix_window::Rect> {
        let menu = self.mouse.menu.as_ref()?;
        let (char_width, line_height) = self.cell_size();
        let (_, _, modeline_top) = self.frame_metrics(window_height);
        Some(menu.rect(char_width, line_height, text::PAD_LEFT, window_width, modeline_top))
    }

    /// The menu, as a popup: its box, rows and the highlighted one.
    pub(super) fn context_menu_popup(&self, window_width: f32, window_height: f32) -> Option<(fenix_window::Rect, RowSpans, Option<usize>)> {
        let menu = self.mouse.menu.as_ref()?;
        let rect = self.menu_rect(window_width, window_height)?;
        let theme = self.theme;
        let mut spans: RowSpans = Vec::new();
        for (i, (label, key)) in menu.rows().into_iter().enumerate() {
            if i > 0 {
                spans.push(("\n".to_string(), theme.fg_modeline, false));
            }
            spans.push((label, theme.fg_modeline, false));
            spans.push((key, theme.gutter_fg, false));
        }
        Some((rect, spans, menu.selected))
    }

    /// A button down while the menu is open. `Some` when the menu took
    /// the click: the row's action to run, if it was on a row. A click
    /// outside closes it and goes on to whatever's there.
    pub(super) fn menu_press_at(&mut self, pos: (f32, f32), window_width: f32, window_height: f32) -> Option<Option<MenuAction>> {
        let rect = self.menu_rect(window_width, window_height)?;
        let (_, line_height) = self.cell_size();
        let menu = self.mouse.menu.as_ref()?;
        if !rect.contains_point(pos.0, pos.1) {
            self.mouse.menu = None;
            self.wake_caret();
            return None;
        }
        let action = menu.row_at(rect, line_height, pos).and_then(|row| menu.items.get(row)).map(|item| item.action);
        if action.is_some() {
            self.mouse.menu = None;
            self.wake_caret();
        }
        Some(action)
    }

    /// The pointer over the open menu highlights the row it's on.
    pub(super) fn menu_hover_at(&mut self, pos: (f32, f32), window_width: f32, window_height: f32) {
        let Some(rect) = self.menu_rect(window_width, window_height) else { return };
        let (_, line_height) = self.cell_size();
        let Some(menu) = self.mouse.menu.as_mut() else { return };
        if let Some(row) = menu.row_at(rect, line_height, pos) {
            if menu.selected != Some(row) {
                menu.selected = Some(row);
                self.wake_caret();
            }
        }
    }

    /// A key while the menu is open: it has them all. j/k and the arrows
    /// move, Enter runs the row, anything else closes it. `Some` with
    /// the action to run.
    pub(super) fn menu_key(&mut self, key: KeyPress) -> Option<MenuAction> {
        let menu = self.mouse.menu.as_mut()?;
        let plain = key.mods == Mods::default();
        let action = match key.code {
            KeyCode::Char('j') if plain => {
                menu.step(1);
                None
            }
            KeyCode::Char('k') if plain => {
                menu.step(-1);
                None
            }
            KeyCode::Named(FenixNamedKey::Down) => {
                menu.step(1);
                None
            }
            KeyCode::Named(FenixNamedKey::Up) => {
                menu.step(-1);
                None
            }
            KeyCode::Named(FenixNamedKey::Enter) => {
                let chosen = menu.chosen();
                self.mouse.menu = None;
                chosen
            }
            _ => {
                self.mouse.menu = None;
                None
            }
        };
        self.wake_caret();
        action
    }

    /// Does what a menu's row says, as its key or command would.
    pub(super) fn run_menu_action(&mut self, action: MenuAction) {
        let keys = |s: &str| s.chars().map(KeyPress::char).collect::<Vec<_>>();
        match action {
            MenuAction::Definition => self.request_goto_definition(),
            MenuAction::References => self.request_references(),
            MenuAction::Rename => self.open_lsp_rename_prompt(),
            MenuAction::CodeActions => self.request_code_action(),
            MenuAction::Hover => self.request_hover(),
            MenuAction::Cut => self.mouse_vim_keys(&keys("d")),
            MenuAction::Copy => self.mouse_vim_keys(&keys("y")),
            MenuAction::Paste if self.vim.is_inserting() => {
                if let Some(text) = self.clipboard_text().or_else(|| Some(self.vim.register().0.to_string())).filter(|t| !t.is_empty()) {
                    let (buffer, cursor) = self.focused_buffer_and_cursor_mut();
                    for c in text.chars() {
                        buffer.insert_char(cursor, c);
                    }
                }
            }
            MenuAction::Paste => self.mouse_vim_keys(&keys(if self.vim.mode() == Mode::Visual { "p" } else { "P" })),
            MenuAction::SelectAll => {
                self.mouse_leave_insert();
                let pane = self.focused_pane_id();
                let last = self.open().buffer.line_count().saturating_sub(1);
                let at = self.open().buffer.line_start_char(last);
                self.vim.select(VisualKind::Line, 0);
                self.put_cursor(pane, at);
            }
            MenuAction::ToggleFold => self.toggle_code_fold(),
            MenuAction::Breakpoint => self.debug_toggle_breakpoint(),
            MenuAction::RunTest => self.tests_run_here(),
            MenuAction::DebugTest => self.tests_debug_here(),
            MenuAction::ShowTest => self.tests_show_here(),
            MenuAction::AskAgent => self.agents_ask_selection(),
            MenuAction::Tab(what, pane, buffer) => {
                self.windows_mut().focus(pane);
                self.sidebar_focused = false;
                self.terminal_focused = false;
                if self.windows().content(pane) != Some(&buffer) {
                    self.set_pane_content(pane, buffer);
                }
                match what {
                    TabAction::Close => self.close_focused_tab(),
                    TabAction::CloseOthers => self.close_other_tabs(),
                    TabAction::CloseRight => self.close_tabs_to_the_right(pane, buffer),
                    TabAction::SplitRight => self.split_vertical(),
                    TabAction::SplitDown => self.split_horizontal(),
                    TabAction::CopyPath => self.yank_file_path(),
                    TabAction::Reveal => self.sidebar_reveal_current(),
                }
                self.refresh_project_root();
            }
        }
        self.wake_caret();
    }

    /// Every tab right of `buffer`'s in `pane`'s strip, closed.
    fn close_tabs_to_the_right(&mut self, pane: fenix_window::WindowId, buffer: BufferId) {
        let order = self.pane_tab_order(pane);
        let Some(at) = order.iter().position(|&b| b == buffer) else { return };
        for id in order[at + 1..].iter().rev() {
            self.close_pane_tab(pane, *id);
        }
    }

    /// Vim keys typed for a menu's row, the clipboard kept in step as
    /// typing them would.
    pub(super) fn mouse_vim_keys(&mut self, keys: &[KeyPress]) {
        for &key in keys {
            self.pull_clipboard_before_paste(key);
            let (id, pane) = (self.focused_buffer_id(), self.focused_pane_id());
            if let (Some(ob), Some(state)) = (self.buffers.get_mut(id), self.workspaces.active_pane_states_mut().get_mut(&pane)) {
                self.vim.handle_key(&mut ob.buffer, &mut state.cursor, key);
            }
            self.push_clipboard_after_edit();
        }
    }

    /// `editor.mouse_hover` on, and `editor.hover_delay_ms` not 0: how
    /// long the pointer rests before the hover.
    fn hover_delay(&self) -> Option<Duration> {
        if self.config.bool_value("editor.mouse_hover") == Some(false) {
            return None;
        }
        let ms = self.config.int_value("editor.hover_delay_ms").unwrap_or(HOVER_DELAY_MS);
        (ms > 0).then(|| Duration::from_millis(ms as u64))
    }

    /// The pointer moved to `pos`: over a new glyph of the focused pane's
    /// text, the wait for its hover starts again; off it, the hover the
    /// pointer opened closes.
    pub(super) fn pointer_rest_at(&mut self, pos: (f32, f32), now: Instant, window_width: f32, window_height: f32) {
        if self.hover_delay().is_none() {
            self.mouse.rest = None;
            return;
        }
        let pane = self.focused_pane_id();
        let geometry = self.geometry_at(window_width, window_height);
        let (_, line_height) = self.cell_size();
        let over = match (hit_test(&geometry, pos), geometry.panes.iter().find(|(p, _)| *p == pane)) {
            (Some(ScrollTarget::Pane(p)), Some(&(_, rect))) if p == pane && pos.1 >= pane_content_rect(rect, line_height, self.pane_has_breadcrumb(pane)).y => self.text_hit(pane, rect, pos),
            _ => None,
        };
        let over = over.filter(|hit| !hit.past_end && !hit.gutter);
        let at = over.map(|hit| (hit.buffer, self.buffers.get(hit.buffer).map_or(0, |ob| ob.buffer.line_start_char(hit.line)) + hit.col, hit.anchor));
        let same = matches!((self.mouse.rest, at), (Some(rest), Some((buffer, at, _))) if rest.buffer == buffer && rest.at == at);
        if same {
            return;
        }
        self.close_mouse_hover();
        self.mouse.rest = at.map(|(buffer, at, anchor)| Rest { at, buffer, since: now, anchor, asked: false });
    }

    /// Closes the hover the pointer opened, if it's open.
    pub(super) fn close_mouse_hover(&mut self) {
        if self.mouse.hover.take().is_some() {
            self.lsp_hover = None;
            self.lsp_hover_fanout = None;
            self.wake_caret();
        }
        if let Some(rest) = &mut self.mouse.rest {
            rest.asked = true;
        }
    }

    /// The pointer rested long enough: its glyph's problems, and the
    /// language server asked about it. When to look again, while waiting.
    pub(super) fn hover_tick(&mut self, now: Instant) -> Option<Instant> {
        let delay = self.hover_delay()?;
        let rest = self.mouse.rest.filter(|r| !r.asked)?;
        if self.mouse.drag.is_some() || self.mouse.split.is_some() || self.mouse.menu.is_some() || self.main_view != MainView::Editor || self.focused_buffer_id() != rest.buffer {
            return None;
        }
        let due = rest.since + delay;
        if now < due {
            return Some(due);
        }
        if let Some(r) = &mut self.mouse.rest {
            r.asked = true;
        }
        let problems = self.problems_at(rest.buffer, rest.at);
        // `K`'s own hover, if one's open, gives way.
        self.lsp_hover = None;
        self.lsp_hover_fanout = None;
        self.mouse.hover = Some(MouseHover { anchor: rest.anchor, problems });
        self.request_hover_at(rest.at);
        self.wake_caret();
        None
    }

    /// The problems a language server (or the MIB) reported over char
    /// `at`, a line of text each, errors first.
    fn problems_at(&self, buffer: BufferId, at: usize) -> Vec<(String, bool)> {
        let Some(ob) = self.buffers.get(buffer) else { return Vec::new() };
        let Some(path) = ob.buffer.path() else { return Vec::new() };
        let canonical = fenix_lsp::normalize(std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()));
        let rope = ob.buffer.rope();
        let mut found: Vec<&lsp_types::Diagnostic> = Vec::new();
        let all = self.all_diagnostics(&canonical);
        for d in &all {
            let start = fenix_lsp::position_to_char_offset(rope, d.range.start);
            let end = fenix_lsp::position_to_char_offset(rope, d.range.end);
            if (start..end.max(start + 1)).contains(&at) {
                found.push(d);
            }
        }
        found.sort_by_key(|d| d.severity != Some(lsp_types::DiagnosticSeverity::ERROR));
        found
            .into_iter()
            .flat_map(|d| {
                let error = d.severity == Some(lsp_types::DiagnosticSeverity::ERROR);
                let source = d.source.as_deref().map(|s| format!("{s}: ")).unwrap_or_default();
                format!("{source}{}", d.message).lines().map(|l| (l.to_string(), error)).collect::<Vec<_>>()
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: f32 = 800.0;
    const H: f32 = 600.0;

    /// A file's app, with `text` in it and the cursor at the start.
    fn app_with(text: &str) -> App {
        let mut app = App::with_file(None);
        let (pane, id) = (app.focused_pane_id(), app.buffers.open_scratch());
        app.set_pane_content(pane, id);
        app.test_insert_str(text);
        app.test_set_cursor(Cursor::at_start());
        app
    }

    /// The pixel over char column `col` of row `row` of the focused
    /// pane's text.
    fn at(app: &App, row: usize, col: usize) -> (f32, f32) {
        let geometry = app.geometry_at(W, H);
        let pane = app.focused_pane_id();
        let rect = geometry.panes.iter().find(|(p, _)| *p == pane).unwrap().1;
        let content = pane_content_rect(rect, text::LINE_HEIGHT, app.pane_has_breadcrumb(pane));
        let gutter = app.gutter_chars(app.open()) as f32 * text::CHAR_WIDTH;
        (content.x + text::PAD_LEFT + gutter + (col as f32 + 0.5) * text::CHAR_WIDTH, content.y + text::PAD_TOP + (row as f32 + 0.5) * text::LINE_HEIGHT)
    }

    fn click(app: &mut App, pos: (f32, f32)) {
        app.handle_click_at(pos, W, H);
        app.mouse_release();
    }

    fn cursor_at(app: &App) -> (usize, usize) {
        app.open().buffer.line_col(&app.cursor())
    }

    fn selection(app: &App) -> Option<String> {
        let (range, _) = app.vim.visual_selection_range(&app.open().buffer, &app.cursor())?;
        Some(app.open().buffer.text_range(range.start, range.end))
    }

    #[test]
    fn a_click_puts_the_cursor_on_the_glyph_clicked() {
        let mut app = app_with("hello world\nsecond line\nthird");
        let at_pos = at(&app, 1, 3);
        click(&mut app, at_pos);
        assert_eq!(cursor_at(&app), (1, 3));
        let at_pos = at(&app, 0, 40);
        click(&mut app, at_pos);
        assert_eq!(cursor_at(&app), (0, 10), "past the end is the line's last char");
        let at_pos = at(&app, 9, 2);
        click(&mut app, at_pos);
        assert_eq!(cursor_at(&app), (2, 2), "under the last line is the last line");
    }

    #[test]
    fn a_click_lands_through_tabs_wide_chars_and_horizontal_scroll() {
        let mut app = app_with("\tab\n日本語x");
        app.config.tab_width = Some(4);
        let at_pos = at(&app, 0, 2);
        click(&mut app, at_pos);
        assert_eq!(cursor_at(&app), (0, 0), "on the tab's spaces is the tab");
        let at_pos = at(&app, 0, 5);
        click(&mut app, at_pos);
        assert_eq!(cursor_at(&app), (0, 2));
        let at_pos = at(&app, 1, 3);
        click(&mut app, at_pos);
        assert_eq!(cursor_at(&app), (1, 3), "a char a column, as the caret's drawn");
        let pane = app.focused_pane_id();
        app.pane_state_mut(pane).scroll_col = 2;
        let at_pos = at(&app, 1, 0);
        click(&mut app, at_pos);
        assert_eq!(cursor_at(&app), (1, 2), "scrolled two columns along");
    }

    #[test]
    fn a_click_counts_the_rows_scrolled_past() {
        let text: String = (0..40).map(|i| format!("line {i}\n")).collect();
        let mut app = app_with(&text);
        let pane = app.focused_pane_id();
        app.pane_state_mut(pane).scroll_line = 10;
        app.pane_state_mut(pane).rendered_scroll = 10.0;
        let at_pos = at(&app, 2, 5);
        click(&mut app, at_pos);
        assert_eq!(cursor_at(&app), (12, 5));
    }

    #[test]
    fn a_click_under_a_closed_fold_lands_on_the_line_drawn_there() {
        let mut app = app_with("");
        let id = app.focused_buffer_id();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.conf");
        std::fs::write(&path, "a:\n  b: 1\n  c: 2\nd: 3\nafter\n").unwrap();
        app.buffers.get_mut(id).unwrap().buffer = Buffer::from_path(&path).unwrap();
        app.code_folds.entry(id).or_default().insert(0);
        assert_eq!(app.pane_display_lines(app.focused_pane_id(), id)[..2], [0, 3], "the fold's body is hidden");
        let at_pos = at(&app, 2, 2);
        click(&mut app, at_pos);
        assert_eq!(cursor_at(&app), (4, 2));
    }

    #[test]
    fn a_click_in_insert_stays_in_insert_and_can_land_after_the_last_char() {
        let mut app = app_with("abc");
        app.test_vim_key(KeyPress::char('i'));
        let at_pos = at(&app, 0, 9);
        click(&mut app, at_pos);
        assert_eq!(app.vim.mode(), Mode::Insert);
        assert_eq!(cursor_at(&app), (0, 3));
    }

    #[test]
    fn a_click_leaves_visual_and_drops_a_pending_operator() {
        let mut app = app_with("abc def");
        app.test_vim_key(KeyPress::char('v'));
        app.test_vim_key(KeyPress::char('l'));
        let at_pos = at(&app, 0, 5);
        click(&mut app, at_pos);
        assert_eq!(app.vim.mode(), Mode::Normal);
        app.test_vim_key(KeyPress::char('d'));
        let at_pos = at(&app, 0, 1);
        click(&mut app, at_pos);
        assert!(app.vim.is_idle());
    }

    #[test]
    fn dragging_selects_from_where_the_button_went_down() {
        let mut app = app_with("hello world");
        let at_pos = at(&app, 0, 1);
        app.handle_click_at(at_pos, W, H);
        assert_eq!(app.vim.mode(), Mode::Normal, "no selection until the pointer moves off the char");
        let at_pos = at(&app, 0, 4);
        app.mouse_drag_to(at_pos, W, H);
        assert_eq!((app.vim.mode(), app.vim.visual_kind()), (Mode::Visual, VisualKind::Char));
        assert_eq!(selection(&app).as_deref(), Some("ello"));
        let at_pos = at(&app, 0, 0);
        app.mouse_drag_to(at_pos, W, H);
        assert_eq!(selection(&app).as_deref(), Some("he"), "back past the start");
        app.mouse_release();
        let at_pos = at(&app, 0, 8);
        app.mouse_drag_to(at_pos, W, H);
        assert_eq!(selection(&app).as_deref(), Some("he"), "let go, the pointer selects nothing more");
    }

    #[test]
    fn a_drag_from_insert_selects_in_visual() {
        let mut app = app_with("hello world");
        app.test_vim_key(KeyPress::char('i'));
        let at_pos = at(&app, 0, 0);
        app.handle_click_at(at_pos, W, H);
        let at_pos = at(&app, 0, 4);
        app.mouse_drag_to(at_pos, W, H);
        assert_eq!(app.vim.mode(), Mode::Visual);
        assert_eq!(selection(&app).as_deref(), Some("hello"));
    }

    #[test]
    fn a_drag_below_the_pane_selects_down_into_what_it_scrolls_to() {
        let text: String = (0..100).map(|i| format!("line {i}\n")).collect();
        let mut app = app_with(&text);
        let at_pos = at(&app, 0, 0);
        app.handle_click_at(at_pos, W, H);
        let at_pos = at(&app, 0, 0);
        app.mouse_drag_to((at_pos.0, H + 200.0), W, H);
        let (line, _) = cursor_at(&app);
        assert!(line > 25, "past what the pane shows: {line}");
        assert_eq!(app.vim.mode(), Mode::Visual);
    }

    #[test]
    fn a_double_click_selects_a_word_and_a_triple_its_line() {
        let mut app = app_with("let foo_bar = 1;\nnext");
        let pos = at(&app, 0, 6);
        click(&mut app, pos);
        click(&mut app, pos);
        assert_eq!(selection(&app).as_deref(), Some("foo_bar"));
        click(&mut app, pos);
        assert_eq!(app.vim.visual_kind(), VisualKind::Line);
        assert_eq!(selection(&app).as_deref(), Some("let foo_bar = 1;\n"));
    }

    #[test]
    fn dragging_after_a_double_click_selects_whole_words() {
        let mut app = app_with("one two three four");
        let pos = at(&app, 0, 9);
        click(&mut app, pos);
        app.handle_click_at(pos, W, H);
        let at_pos = at(&app, 0, 15);
        app.mouse_drag_to(at_pos, W, H);
        assert_eq!(selection(&app).as_deref(), Some("three four"));
        let at_pos = at(&app, 0, 5);
        app.mouse_drag_to(at_pos, W, H);
        assert_eq!(selection(&app).as_deref(), Some("two three"));
    }

    #[test]
    fn shift_click_stretches_the_selection_from_the_cursor() {
        let mut app = app_with("abcdefgh");
        let at_pos = at(&app, 0, 2);
        click(&mut app, at_pos);
        app.modifiers = ModifiersState::SHIFT;
        let at_pos = at(&app, 0, 5);
        click(&mut app, at_pos);
        assert_eq!(selection(&app).as_deref(), Some("cdef"));
        let at_pos = at(&app, 0, 7);
        click(&mut app, at_pos);
        assert_eq!(selection(&app).as_deref(), Some("cdefgh"), "and again, from the same anchor");
    }

    #[test]
    fn alt_drag_selects_a_block() {
        let mut app = app_with("abcd\nefgh\nijkl");
        app.modifiers = ModifiersState::ALT;
        let at_pos = at(&app, 0, 1);
        app.handle_click_at(at_pos, W, H);
        let at_pos = at(&app, 2, 2);
        app.mouse_drag_to(at_pos, W, H);
        assert_eq!((app.vim.mode(), app.vim.visual_kind()), (Mode::Visual, VisualKind::Block));
        assert_eq!(cursor_at(&app), (2, 2));
    }

    #[test]
    fn only_a_far_click_goes_on_the_jumplist() {
        let text: String = (0..30).map(|i| format!("line {i}\n")).collect();
        let mut app = app_with(&text);
        let at_pos = at(&app, 3, 0);
        click(&mut app, at_pos);
        assert!(app.jump_back_stack.is_empty());
        let at_pos = at(&app, 3 + CLICK_JUMP_LINES, 0);
        click(&mut app, at_pos);
        assert_eq!(app.jump_back_stack.len(), 1);
    }

    #[test]
    fn dragging_a_divider_resizes_the_split() {
        let mut app = app_with("text");
        app.split_vertical();
        let geometry = app.geometry_at(W, H);
        let divider = app.windows().dividers(geometry.pane_area)[0];
        let y = 300.0;
        assert_eq!(app.divider_icon(&geometry, (divider.at + 1.0, y)), Some(winit::window::CursorIcon::ColResize));
        assert!(app.divider_press_at((divider.at + 1.0, y), W, H));
        let target = geometry.pane_area.x + geometry.pane_area.w * 0.3;
        app.mouse_drag_to((target, y), W, H);
        app.mouse_release();
        let left = app.windows().layout(geometry.pane_area)[0].1;
        assert!((left.w - geometry.pane_area.w * 0.3).abs() < 1.0, "{left:?}");
        assert!(!app.divider_press_at((target + 50.0, y), W, H), "away from it is the text");
    }

    #[test]
    fn a_right_click_puts_the_cursor_there_and_opens_the_menu() {
        let mut app = app_with("alpha beta");
        let at_pos = at(&app, 0, 7);
        assert!(app.right_click_at(at_pos, W, H));
        assert_eq!(cursor_at(&app), (0, 7));
        let menu = app.mouse.menu.as_ref().unwrap();
        let labels: Vec<&str> = menu.items.iter().map(|i| i.label.as_str()).collect();
        assert!(labels.starts_with(&["Go to definition", "Find references", "Rename"]), "{labels:?}");
        assert!(!labels.contains(&"Copy"), "nothing selected to copy");
        let rename = menu.items.iter().find(|i| i.action == MenuAction::Rename).unwrap();
        assert_eq!(rename.key, "SPC c r", "the key beside it");
        let (rect, spans, _) = app.context_menu_popup(W, H).unwrap();
        assert_eq!((rect.x, rect.y), at(&app, 0, 7));
        assert!(spans.iter().any(|(s, _, _)| s.starts_with("Go to definition")));
    }

    #[test]
    fn a_right_click_inside_the_selection_keeps_it() {
        let mut app = app_with("alpha beta");
        app.test_vim_key(KeyPress::char('v'));
        for _ in 0..4 {
            app.test_vim_key(KeyPress::char('l'));
        }
        let at_pos = at(&app, 0, 2);
        assert!(app.right_click_at(at_pos, W, H));
        assert_eq!(selection(&app).as_deref(), Some("alpha"));
        assert!(app.mouse.menu.as_ref().unwrap().items.iter().any(|i| i.action == MenuAction::Copy));
        app.mouse.menu = None;
        let at_pos = at(&app, 0, 8);
        assert!(app.right_click_at(at_pos, W, H));
        assert_eq!(app.vim.mode(), Mode::Normal, "outside it, it goes");
    }

    #[test]
    fn the_menu_is_driven_by_the_pointer_and_the_keys() {
        let mut app = app_with("alpha beta");
        let at_pos = at(&app, 0, 0);
        app.right_click_at(at_pos, W, H);
        let rect = app.menu_rect(W, H).unwrap();
        let row = |i: usize| (rect.x + 10.0, rect.y + crate::mouse::MENU_PADDING / 2.0 + (i as f32 + 0.5) * text::LINE_HEIGHT);
        app.menu_hover_at(row(1), W, H);
        assert_eq!(app.mouse.menu.as_ref().unwrap().selected, Some(1));
        assert_eq!(app.menu_key(KeyPress::char('j')), None);
        assert_eq!(app.menu_key(KeyPress::named(FenixNamedKey::Enter)), Some(MenuAction::Rename));
        assert!(app.mouse.menu.is_none());

        let at_pos = at(&app, 0, 0);
        app.right_click_at(at_pos, W, H);
        assert_eq!(app.menu_press_at(row(0), W, H), Some(Some(MenuAction::Definition)));
        let at_pos = at(&app, 0, 0);
        app.right_click_at(at_pos, W, H);
        assert_eq!(app.menu_press_at((1.0, 1.0), W, H), None, "outside: closed, and the click goes on");
        assert!(app.mouse.menu.is_none());
        let at_pos = at(&app, 0, 0);
        app.right_click_at(at_pos, W, H);
        assert_eq!(app.menu_key(KeyPress::named(FenixNamedKey::Escape)), None);
        assert!(app.mouse.menu.is_none());
    }

    #[test]
    fn the_menus_copy_and_select_all_do_what_their_keys_do() {
        let mut app = app_with("one\ntwo\nthree");
        let pos = at(&app, 1, 1);
        click(&mut app, pos);
        click(&mut app, pos);
        app.run_menu_action(MenuAction::Copy);
        assert_eq!(app.vim.register().0, "two");
        assert_eq!(app.vim.mode(), Mode::Normal);
        app.run_menu_action(MenuAction::SelectAll);
        assert_eq!(selection(&app).as_deref(), Some("one\ntwo\nthree"));
    }

    #[test]
    fn a_tab_dragged_onto_another_pane_moves_there() {
        let mut app = app_with("a");
        let first = app.focused_pane_id();
        let moved = app.focused_buffer_id();
        app.split_vertical();
        let second = app.focused_pane_id();
        let other = app.buffers.open_scratch();
        app.set_pane_content(second, other);
        let geometry = app.geometry_at(W, H);
        let rect = geometry.panes.iter().find(|(p, _)| *p == second).unwrap().1;
        let middle = (rect.x + rect.w / 2.0, rect.y + rect.h / 2.0);
        assert!(app.tab_dropped_at(first, moved, middle, W, H));
        assert_eq!(app.windows().content(second), Some(&moved));
        assert!(app.pane_tab_order(second).contains(&moved));
        assert!(!app.pane_tab_order(first).contains(&moved), "it left the first pane's strip");
        assert_eq!(app.focused_pane_id(), second);
    }

    #[test]
    fn a_tab_let_go_near_a_panes_edge_splits_it() {
        let mut app = app_with("a");
        let pane = app.focused_pane_id();
        let id = app.focused_buffer_id();
        let other = app.buffers.open_scratch();
        app.set_pane_content(pane, other);
        let geometry = app.geometry_at(W, H);
        let rect = geometry.panes[0].1;
        let right_edge = (rect.x + rect.w - 5.0, rect.y + rect.h / 2.0);
        assert!(app.tab_dropped_at(pane, id, right_edge, W, H));
        assert_eq!(app.windows().window_count(), 2);
        let new = app.focused_pane_id();
        assert_ne!(new, pane);
        assert_eq!(app.windows().content(new), Some(&id));
        let layout = app.windows().layout(geometry.pane_area);
        assert!(layout.iter().find(|(p, _)| *p == new).unwrap().1.x > layout.iter().find(|(p, _)| *p == pane).unwrap().1.x, "on the right");
    }

    #[test]
    fn the_pointer_resting_on_a_symbol_asks_for_its_hover_and_shows_its_problems() {
        let mut app = app_with("fn main() {}");
        let now = Instant::now();
        let at_pos = at(&app, 0, 4);
        app.pointer_rest_at(at_pos, now, W, H);
        assert_eq!(app.hover_tick(now), Some(now + Duration::from_millis(500)), "waiting");
        assert!(app.mouse.hover.is_none());
        assert_eq!(app.hover_tick(now + Duration::from_millis(600)), None);
        let hover = app.mouse.hover.clone().expect("asked");
        assert_eq!(hover.anchor.1, at(&app, 0, 4).1 + text::LINE_HEIGHT / 2.0, "under the glyph's row");
        assert!(hover.problems.is_empty(), "a scratch buffer has none");
        let at_pos = at(&app, 0, 5);
        app.pointer_rest_at(at_pos, now, W, H);
        assert!(app.mouse.hover.is_none(), "moving off the glyph closes it");
        app.config.set_value("editor.hover_delay_ms", Some(fenix_config::Value::Int(0)));
        let at_pos = at(&app, 0, 1);
        app.pointer_rest_at(at_pos, now, W, H);
        assert_eq!(app.hover_tick(now + Duration::from_secs(5)), None);
        assert!(app.mouse.hover.is_none(), "0 turns it off");
    }

    #[test]
    fn the_hover_popup_opens_under_the_pointers_glyph_problems_first() {
        let mut app = app_with("fn main() {}");
        app.mouse.hover = Some(MouseHover { anchor: (120.0, 60.0), problems: vec![("expected `;`".into(), true)] });
        app.lsp_hover = Some("fn main()".into());
        let rect = fenix_window::Rect { x: 0.0, y: 0.0, w: W, h: H };
        let (popup, spans) = app.hover_popup(W, 580.0, rect, None, 0.0, 0.0).unwrap();
        assert_eq!((popup.x, popup.y), (120.0, 60.0));
        let text: Vec<&str> = spans.iter().map(|(s, _, _)| s.as_str()).filter(|s| *s != "\n").collect();
        assert_eq!(text, ["expected `;`", "fn main()"]);
    }

    fn standard(app: &mut App) {
        app.config.set_value("editor.keys", Some(fenix_config::Value::Text("standard".into())));
        app.sync_key_policy();
    }

    #[test]
    fn standard_keys_a_double_click_selects_the_whole_word_and_ctrl_d_finds_whole_words() {
        let mut app = app_with("foo for foobar foo");
        standard(&mut app);
        let at_pos = at(&app, 0, 1);
        click(&mut app, at_pos);
        click(&mut app, at_pos);
        assert_eq!(selection(&app).as_deref(), Some("foo"), "the whole word, as standard keys select");
        assert_eq!(app.visual_selection_segments(0..10), vec![(0, 0, 3)], "drawn as it is");
        app.press("C-c");
        assert_eq!(app.clipboard_text().unwrap_or_else(|| app.vim.register().0.to_string()), "foo", "copied whole");
        app.press("C-d");
        app.type_text("x");
        assert_eq!(app.text(), "x for foobar x", "the next foo, whole: not for's fo nor foobar's");
    }

    #[test]
    fn standard_keys_a_triple_click_then_ctrl_d_keeps_the_first_line_whole() {
        let mut app = app_with("  }\nx\n  }\n");
        standard(&mut app);
        let at_pos = at(&app, 0, 2);
        click(&mut app, at_pos);
        click(&mut app, at_pos);
        click(&mut app, at_pos);
        app.press("C-d");
        assert_eq!(app.extra_carets().len(), 1);
        let extra = app.extra_carets()[0];
        assert_eq!((extra.anchor, extra.at), (Some(0), 4), "line 1 and its break, as it was selected");
        app.type_text("y");
        assert_eq!(app.text(), "yx\ny", "both lines replaced whole");
    }

    #[test]
    fn standard_keys_alt_click_adds_a_cursor_and_a_plain_click_goes_back_to_one() {
        let mut app = app_with("ab\ncd\nef");
        standard(&mut app);
        app.modifiers = ModifiersState::ALT;
        let at_pos = at(&app, 1, 1);
        click(&mut app, at_pos);
        assert_eq!(app.extra_carets().len(), 1);
        assert_eq!(cursor_at(&app), (0, 0), "the primary stays");
        app.modifiers = ModifiersState::empty();
        app.type_text("x");
        assert_eq!(app.text(), "xab\ncxd\nef");
        let at_pos = at(&app, 2, 0);
        click(&mut app, at_pos);
        assert!(app.extra_carets().is_empty());
        assert_eq!(app.vim.mode(), Mode::Insert, "a click keeps typing");
    }

    #[test]
    fn standard_keys_alt_shift_drag_makes_a_column_of_cursors() {
        let mut app = app_with("abcd\nefgh\nijkl");
        standard(&mut app);
        app.modifiers = ModifiersState::ALT | ModifiersState::SHIFT;
        let at_pos = at(&app, 0, 2);
        app.handle_click_at(at_pos, W, H);
        let at_pos = at(&app, 2, 2);
        app.mouse_drag_to(at_pos, W, H);
        app.mouse_release();
        assert_eq!(app.extra_carets().len(), 2);
        app.modifiers = ModifiersState::empty();
        app.type_text("|");
        assert_eq!(app.text(), "ab|cd\nef|gh\nij|kl");
    }

    #[test]
    fn standard_keys_ctrl_as_the_modifier_moves_definition_to_alt() {
        let mut app = app_with("ab\ncd");
        standard(&mut app);
        app.config.set_value("editor.multi_cursor_modifier", Some(fenix_config::Value::Text("ctrl".into())));
        app.modifiers = ModifiersState::CONTROL;
        let at_pos = at(&app, 1, 1);
        click(&mut app, at_pos);
        assert_eq!(app.extra_carets().len(), 1, "Ctrl+click adds a cursor");
        app.modifiers = ModifiersState::ALT;
        let at_pos = at(&app, 1, 0);
        click(&mut app, at_pos);
        assert!(app.extra_carets().is_empty(), "Alt+click goes to a definition, with one cursor");
        assert_eq!(cursor_at(&app), (1, 0));
    }

    #[test]
    fn standard_keys_drag_selects_between_the_press_and_the_pointer() {
        let mut app = app_with("hello world");
        standard(&mut app);
        let at_pos = at(&app, 0, 0);
        app.handle_click_at(at_pos, W, H);
        let at_pos = at(&app, 0, 5);
        app.mouse_drag_to(at_pos, W, H);
        app.mouse_release();
        assert_eq!(selection(&app).as_deref(), Some("hello"), "the pointer's character isn't in it");
        app.type_text("bye");
        assert_eq!(app.text(), "bye world");
    }

    /// Where a cell is, rows from the text's top (negative above it).
    fn at_row(app: &App, row: f32, col: usize) -> (f32, f32) {
        let (x, y) = at(app, 0, col);
        (x, y + row * text::LINE_HEIGHT)
    }

    #[test]
    fn off_the_rows_by_rows_is_where_a_row_a_line_has_it_scrolled_sideways_too() {
        let text = format!("{0}\n{0}\n{0}\n", "x".repeat(200));
        let mut runs = Vec::new();
        for forced in [false, true] {
            let mut app = app_with(&text);
            app.force_row_path = forced;
            let pane = app.focused_pane_id();
            app.pane_state_mut(pane).scroll_col = 50;
            let rect = app.geometry_at(W, H).panes.into_iter().find(|(p, _)| *p == pane).unwrap().1;
            let mut seen = Vec::new();
            // Above the text, below it, and right of a line's end.
            for (row, col) in [(-1.0, 16), (-3.0, 16), (20.0, 16), (3.0, 16), (3.0, 40)] {
                let hit = app.text_hit(pane, rect, at_row(&app, row, col));
                seen.push(hit.map(|h| (h.line, h.col, h.past_end)));
            }
            // A drag past each edge, from inside.
            for row in [-2.0, 30.0] {
                let from = at_row(&app, 1.0, 5);
                app.handle_click_at(from, W, H);
                app.mouse_drag_to(at_row(&app, row, 16), W, H);
                app.mouse_release();
                seen.push(Some((cursor_at(&app).0, cursor_at(&app).1, app.pane_state(pane).scroll_col == 50)));
                app.press("Escape");
                app.pane_state_mut(pane).scroll_col = 50;
            }
            runs.push(seen);
        }
        assert_eq!(runs[0], runs[1]);
        assert_eq!(runs[0][0], Some((0, 66, false)), "through the sideways scroll");
    }

    #[test]
    fn a_lens_row_is_no_place_in_the_text() {
        let mut app = app_with("fn a() {}\nfn b() {}\nfn c() {}\n");
        let (pane, buffer) = (app.focused_pane_id(), app.focused_buffer_id());
        app.set_virtual_rows(buffer, [(1, vec!["▶ Run".to_string()])].into());
        app.test_set_cursor(Cursor { char_idx: 3, sticky_col: 3 });
        app.pane_state_mut(pane).scroll_col = 2;
        let rect = app.geometry_at(W, H).panes.into_iter().find(|(p, _)| *p == pane).unwrap().1;
        // Row 1 is line 1's lens, past its title.
        let pos = at(&app, 1, 20);
        assert!(app.text_hit(pane, rect, pos).is_none(), "nothing to hover or put a cursor on");
        click(&mut app, pos);
        click(&mut app, pos);
        app.mouse_drag_to(pos, W, H);
        app.mouse_release();
        assert_eq!((cursor_at(&app), app.pane_state(pane).scroll_col), ((0, 3), 2), "the cursor and the scroll as they were");
        assert_eq!(app.vim.mode(), Mode::Normal, "nothing selected");
    }

    /// `text`, its lines wrapped at `cols` columns.
    fn wrapped_app(text: &str, cols: usize) -> App {
        let mut app = app_with(text);
        app.run_cmd("view.toggle_wrap");
        let pane = app.focused_pane_id();
        app.set_pane_text_cols(pane, cols);
        app
    }

    #[test]
    fn a_click_on_a_continuation_row_is_on_that_part_of_the_line() {
        // `the quick ` / `brown fox ` / `jumps`, then `next`.
        let mut app = wrapped_app("the quick brown fox jumps\nnext", 10);
        let pos = at(&app, 1, 2);
        click(&mut app, pos);
        assert_eq!(cursor_at(&app), (0, 12));
        let pos = at(&app, 3, 1);
        click(&mut app, pos);
        assert_eq!(cursor_at(&app), (1, 1), "the row after a line's last is the next line");
        let pos = at(&app, 0, 30);
        click(&mut app, pos);
        assert_eq!(cursor_at(&app), (0, 9), "right of a row the line goes on from: that row's last char");
        let pos = at(&app, 2, 30);
        click(&mut app, pos);
        assert_eq!(cursor_at(&app), (0, 24), "right of its last row: the line's last char");
        let pos = at(&app, 9, 0);
        click(&mut app, pos);
        assert_eq!(cursor_at(&app), (1, 0), "below the text: the last line");
    }

    #[test]
    fn a_click_in_a_continuation_row_s_indent_is_its_first_char() {
        // `    let total = ` / `    first + ` / `    second;`
        let mut app = wrapped_app("    let total = first + second;", 16);
        let pos = at(&app, 1, 1);
        click(&mut app, pos);
        assert_eq!(cursor_at(&app), (0, 16));
        let pos = at(&app, 1, 6);
        click(&mut app, pos);
        assert_eq!(cursor_at(&app), (0, 18));
    }

    #[test]
    fn a_drag_across_a_wrapped_line_s_rows_selects_what_is_between() {
        let mut app = wrapped_app("the quick brown fox jumps", 10);
        let (from, to) = (at(&app, 0, 4), at(&app, 1, 8));
        app.handle_click_at(from, W, H);
        app.mouse_drag_to(to, W, H);
        assert_eq!(selection(&app).as_deref(), Some("quick brown fox"));
        app.mouse_release();
    }

    #[test]
    fn a_click_on_a_row_above_a_line_that_isn_t_text_moves_nothing() {
        let mut app = wrapped_app("one\ntwo words\n", 20);
        let buffer = app.focused_buffer_id();
        app.virtual_rows.entry(buffer).or_default().insert(1, vec!["a lens".into()]);
        let pos = at(&app, 1, 4);
        click(&mut app, pos);
        assert_eq!(cursor_at(&app), (0, 0));
        let pos = at(&app, 2, 4);
        click(&mut app, pos);
        assert_eq!(cursor_at(&app), (1, 4), "the text's row below it");
    }

    #[test]
    fn a_wide_char_s_row_is_hit_a_cell_a_char() {
        let mut app = wrapped_app("日本語の文章です", 6);
        // `日本語の文` / `章です`: five a row, the sixth column the caret's.
        let pos = at(&app, 1, 1);
        click(&mut app, pos);
        assert_eq!(cursor_at(&app), (0, 6));
    }

    #[test]
    fn a_click_and_a_drag_on_a_wrapped_tab_indented_line_go_by_its_columns() {
        // `\tlet total = first;` at twelve columns: the tab is eight, so
        // `        let ` and then, six in, `total ` / `= ` / `first` / `;`.
        let mut app = wrapped_app("\tlet total = first;", 12);
        let pos = at(&app, 1, 6);
        click(&mut app, pos);
        assert_eq!(cursor_at(&app), (0, 5), "`total`'s `t`");
        // To right of `first`'s row: its last char.
        let (from, to) = (at(&app, 0, 8), at(&app, 3, 10));
        app.handle_click_at(from, W, H);
        app.mouse_drag_to(to, W, H);
        assert_eq!(selection(&app).as_deref(), Some("let total = first"));
        app.mouse_release();
    }

    #[test]
    fn a_hover_s_card_goes_under_the_glyph_on_a_continuation_row() {
        let app = wrapped_app("the quick brown fox jumps", 10);
        let pane = app.focused_pane_id();
        let rect = app.geometry_at(W, H).panes.into_iter().find(|(p, _)| *p == pane).unwrap().1;
        let hit = app.text_hit(pane, rect, at(&app, 1, 2)).unwrap();
        assert_eq!((hit.line, hit.col), (0, 12));
        let below_row_1 = at(&app, 1, 0).1 + text::LINE_HEIGHT / 2.0;
        assert_eq!(hit.anchor.1, below_row_1, "under row 1, not under the line's first row");
    }
}
