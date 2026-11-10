//! A pane's rows: what is on each screen row of a code pane, and the one
//! translation between places in the text and cells on the screen.
//!
//! A row is part of a buffer line (`RowKind::Text`), a side-by-side
//! pane's filler (`RowKind::Filler`, no line), or a row that isn't text
//! at all and is drawn above its line (`RowKind::Virtual`, for code lens
//! later). Its `cols` are the visual columns of the line's display text
//! it shows (tabs expanded, decorations laid out); with wrap off, a
//! line has one row and `cols` is all of it. Everything that draws over
//! the text or hit-tests it goes through a `RowMap` rather than working
//! out `line - first line shown` and `col - scroll_col` itself.

use std::collections::HashMap;
use std::ops::Range;

use super::{tabstops, App, BufferId, BufferKind, Cursor, LineSegments, Segments};
use crate::decorations::LineLayout;

/// No line: a filler row's.
pub(crate) const NO_LINE: usize = usize::MAX;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RowKind {
    Text,
    /// Keeps a side-by-side pane lined up with the other side.
    Filler,
    /// A row of something that isn't the text, above its line: which.
    #[allow(dead_code)]
    Virtual(u32),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Row {
    /// The buffer line, `NO_LINE` for a filler.
    pub line: usize,
    pub kind: RowKind,
    /// The visual columns of the line shown on this row; the last row's
    /// end is where the line ends, though what lies past it is drawn
    /// there too (the caret after the last char, a selection's sliver).
    pub cols: Range<usize>,
    /// How far in a continuation row starts: the line's indent.
    pub indent: usize,
    /// Whether this is the line's first row, and its last.
    pub first: bool,
    pub last: bool,
}

/// The rows of a pane for one frame, from its top row down.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct RowMap {
    rows: Vec<Row>,
    /// The first visual column shown: the pane's sideways scroll.
    scroll_col: usize,
    /// Each line shown and the first of its rows shown, by line.
    firsts: Vec<(usize, usize)>,
    /// The columns a row has when the lines wrap: the caret is never
    /// drawn past its last.
    width: Option<usize>,
}

impl RowMap {
    /// A row for each of `lines` (display lines from the pane's top, a
    /// filler as `NO_LINE`), the pane scrolled `scroll_col` sideways.
    pub(crate) fn lines(lines: &[usize], scroll_col: usize) -> RowMap {
        let rows = lines
            .iter()
            .map(|&line| Row {
                line,
                kind: if line == NO_LINE { RowKind::Filler } else { RowKind::Text },
                cols: 0..usize::MAX,
                indent: 0,
                first: true,
                last: true,
            })
            .collect();
        RowMap::from_rows(rows, scroll_col)
    }

    /// The map of `rows`, their lines in order.
    pub(crate) fn from_rows(rows: Vec<Row>, scroll_col: usize) -> RowMap {
        let mut firsts: Vec<(usize, usize)> = Vec::new();
        for (i, row) in rows.iter().enumerate() {
            if row.kind == RowKind::Text && firsts.last().is_none_or(|&(line, _)| line != row.line) {
                firsts.push((row.line, i));
            }
        }
        debug_assert!(firsts.windows(2).all(|w| w[0].0 < w[1].0), "a pane's lines go down in order");
        RowMap { rows, scroll_col, firsts, width: None }
    }

    pub(crate) fn len(&self) -> usize {
        self.rows.len()
    }

    /// The map, its rows `width` columns wide: the lines wrap.
    pub(crate) fn wrapped_at(mut self, width: usize) -> RowMap {
        self.width = Some(width.max(1));
        self
    }

    /// The columns a row has, when the lines wrap.
    pub(crate) fn width(&self) -> Option<usize> {
        self.width
    }


    pub(crate) fn row(&self, row: usize) -> Option<&Row> {
        self.rows.get(row)
    }

    /// The first of `line`'s rows shown, which may be one after its
    /// first when the pane's top is inside the line.
    fn first_shown(&self, line: usize) -> Option<usize> {
        let i = self.firsts.binary_search_by_key(&line, |&(l, _)| l).ok()?;
        Some(self.firsts[i].1)
    }

    /// The row `line` starts on, if that row is shown: where its number
    /// and gutter marks go.
    pub(crate) fn first_row(&self, line: usize) -> Option<usize> {
        self.first_shown(line).filter(|&row| self.rows[row].first)
    }

    /// The rows of `line` shown (empty when it isn't).
    pub(crate) fn rows_of(&self, line: usize) -> Range<usize> {
        let Some(first) = self.first_shown(line) else { return 0..0 };
        let end = self.rows[first..].iter().position(|r| r.line != line || r.kind != RowKind::Text).map_or(self.rows.len(), |n| first + n);
        first..end
    }

    /// The lines shown, in order.
    pub(crate) fn text_lines(&self) -> impl Iterator<Item = usize> + '_ {
        self.firsts.iter().map(|&(line, _)| line)
    }

    /// The lines shown as runs of consecutive lines: one run, unless a
    /// fold is closed in view.
    pub(crate) fn runs(&self) -> Vec<Range<usize>> {
        let mut runs: Vec<Range<usize>> = Vec::new();
        for line in self.text_lines() {
            match runs.last_mut() {
                Some(run) if run.end == line => run.end = line + 1,
                _ => runs.push(line..line + 1),
            }
        }
        runs
    }

    /// From the first line shown to the last, those between included.
    pub(crate) fn span(&self) -> Range<usize> {
        match (self.firsts.first(), self.firsts.last()) {
            (Some(&(first, _)), Some(&(last, _))) => first..last + 1,
            _ => 0..0,
        }
    }

    /// The line on each row, `NO_LINE` on any but a line's first: what
    /// the gutter marks go beside.
    pub(crate) fn gutter_lines(&self) -> Vec<usize> {
        self.rows.iter().map(|r| if r.kind == RowKind::Text && r.first { r.line } else { NO_LINE }).collect()
    }

    /// The line on each row, `NO_LINE` on a row of none.
    pub(crate) fn lines_by_row(&self) -> Vec<usize> {
        self.rows.iter().map(|r| if r.kind == RowKind::Text { r.line } else { NO_LINE }).collect()
    }

    /// Where on `row` visual column `col` of its line is drawn: cells
    /// from the text's left edge.
    fn x(&self, row: &Row, col: usize) -> usize {
        let indent = if row.first { 0 } else { row.indent };
        (col.saturating_sub(row.cols.start) + indent).saturating_sub(self.scroll_col)
    }

    /// Where visual column `col` of `line` is drawn: its row and cell,
    /// `None` when the line isn't shown, or that part of it is above the
    /// top or below the bottom. A column at the end of a row that isn't
    /// the line's last is the next row's start. A wrapped row is never
    /// wider than the pane, and a line's last is narrower, so even the
    /// place after its last char has a cell; the hold to the last column
    /// is a guard.
    pub(crate) fn locate(&self, line: usize, col: usize) -> Option<(usize, usize)> {
        let rows = self.rows_of(line);
        if rows.is_empty() || col < self.rows[rows.start].cols.start {
            return None;
        }
        let at = rows.clone().find(|&i| col < self.rows[i].cols.end || self.rows[i].last)?;
        let x = self.x(&self.rows[at], col);
        Some((at, self.width.map_or(x, |w| x.min(w - 1))))
    }

    /// Visual columns `cols` of `line`, as the pieces drawn on each of
    /// its rows: `(row, from, to)` in cells. An empty range is a point,
    /// on the row `locate` puts it.
    pub(crate) fn segments(&self, line: usize, cols: Range<usize>) -> Vec<(usize, usize, usize)> {
        if cols.start >= cols.end {
            return self.locate(line, cols.start).map(|(row, x)| vec![(row, x, x)]).unwrap_or_default();
        }
        let mut out = Vec::new();
        for i in self.rows_of(line) {
            let row = &self.rows[i];
            let lo = cols.start.max(row.cols.start);
            let hi = if row.last { cols.end } else { cols.end.min(row.cols.end) };
            if lo < hi {
                // Never past a wrapped pane's edge.
                let edge = |x: usize| self.width.map_or(x, |w| x.min(w));
                out.push((i, edge(self.x(row, lo)), edge(self.x(row, hi))));
            }
        }
        out
    }

    /// What's under cell `x` of `row`: its line and visual column, `None`
    /// on a row of no line. Right of a row the line goes on from is that
    /// row's last column, not the next row's first.
    pub(crate) fn at(&self, row: usize, x: usize) -> Option<(usize, usize)> {
        let r = self.rows.get(row).filter(|r| r.kind == RowKind::Text)?;
        let indent = if r.first { 0 } else { r.indent };
        let col = r.cols.start + (x + self.scroll_col).saturating_sub(indent);
        Some((r.line, if r.last { col } else { col.min(r.cols.end.saturating_sub(1).max(r.cols.start)) }))
    }
}

/// The char columns of `line` its rows shown cover, `col_map` taking
/// their visual columns to chars (none: one to one): the first shown
/// row's start to the last's end, one char on to take a char a break is
/// inside. A row that runs to the end of its line (`cols.end` is
/// `usize::MAX`, as a line past the buffer's is) runs to the end here.
pub(super) fn shown_char_columns(map: &RowMap, line: usize, col_map: Option<&Vec<usize>>) -> Option<(usize, usize)> {
    let shown = map.rows_of(line);
    let (a, b) = (map.row(shown.start)?, map.row(shown.end.checked_sub(1)?)?);
    let char_of = |v: usize| col_map.map_or(v, |m| crate::mouse::char_col_at(m, v).0);
    Some((char_of(a.cols.start), char_of(b.cols.end).saturating_add(1)))
}

/// A pane's text as a frame lays it out: its rows, and the lines whose
/// columns aren't one a char (a tab, a decoration).
pub(super) struct PaneLayout {
    pub map: RowMap,
    /// Char column -> visual column, for each line shown that has a tab
    /// or a decoration; a line without is one to one.
    pub col_maps: HashMap<usize, Vec<usize>>,
    /// The extensions' decorations of the lines shown, laid out.
    pub decorated: HashMap<usize, LineLayout>,
    pub decoration_rows: Vec<(usize, LineLayout)>,
    /// Indentation guides: (row, column).
    pub indent_guides: Vec<(usize, usize)>,
}

impl PaneLayout {
    /// `line`'s char column `col` as a visual column.
    pub(super) fn vcol(&self, line: usize, col: usize) -> usize {
        self.col_maps.get(&line).map(|m| tabstops::visual_col(m, col)).unwrap_or(col)
    }

    /// Where char column `col` of `line` is drawn: (row, cell).
    pub(super) fn locate(&self, line: usize, col: usize) -> Option<(usize, usize)> {
        self.map.locate(line, self.vcol(line, col))
    }

    /// Segments in the text, as `(line, from, to)` char columns, on the
    /// rows: `(row, from, to)` cells.
    pub(super) fn to_rows(&self, segments: LineSegments) -> Segments {
        segments.into_iter().flat_map(|(line, s, e)| self.map.segments(line, self.vcol(line, s)..self.vcol(line, e))).collect()
    }
}

/// What the focused pane draws over its text, on its rows.
#[derive(Debug, Default, PartialEq)]
pub(super) struct FocusedOverlays {
    /// Standard keys' extra cursors: (row, cell).
    pub extra_carets: Vec<(usize, usize)>,
    /// The selection, the extra cursors' among it.
    pub selection: Segments,
    pub pulse: Option<(Segments, f32)>,
    pub brackets: Segments,
    pub hlsearch: Segments,
    pub caret: Option<(usize, usize)>,
}

/// Which rows of a pane a frame wants.
pub(super) struct RowsWanted<'a> {
    /// Display lines from the pane's top (a filler as `NO_LINE`), as
    /// many as there may be rows.
    pub lines: &'a [usize],
    /// Rows of the first line above the top (`PaneState::scroll_row`).
    pub skip: usize,
    pub count: usize,
    /// The sideways scroll: 0 when the lines wrap.
    pub scroll_col: usize,
    /// The columns the lines wrap at, if they do.
    pub wrap: Option<usize>,
}

impl App {
    /// Whether `buffer`'s lines wrap where a pane lets them: as it was
    /// turned, else the default for it.
    pub(super) fn buffer_wraps(&self, buffer: BufferId) -> bool {
        self.wrap_overrides.get(&buffer).copied().unwrap_or(false)
    }

    /// Whether `pane` can wrap what it shows of `buffer`: ordinary text
    /// (a file, an agent's prompt), not a table, a terminal, a page or an
    /// extension's surface, and not one side of a side by side, whose
    /// filler rows need a row a line.
    pub(super) fn pane_can_wrap(&self, pane: fenix_window::WindowId, buffer: BufferId) -> bool {
        let Some(ob) = self.buffers.get(buffer) else { return false };
        matches!(ob.kind, BufferKind::Text | BufferKind::Prompt)
            && !self.table_views.contains_key(&buffer)
            && !self.terminal_buffers.contains_key(&buffer)
            && !self.is_page_buffer(buffer)
            && self.code_surface(buffer).is_none()
            && !self.is_side_by_side_pane(pane)
    }

    /// The columns `pane`'s lines of `buffer` wrap at: `None` when they
    /// don't, and every path is as it was without wrap.
    pub(super) fn wrap_cols(&self, pane: fenix_window::WindowId, buffer: BufferId) -> Option<usize> {
        (self.buffer_wraps(buffer) && self.pane_can_wrap(pane, buffer)).then(|| self.pane_text_cols.get(&pane).copied().unwrap_or(80).max(1))
    }

    /// `pane` has room for `cols` columns of text: what a frame found,
    /// and what a test says.
    pub(super) fn set_pane_text_cols(&mut self, pane: fenix_window::WindowId, cols: usize) {
        self.pane_text_cols.insert(pane, cols);
    }

    /// `view.toggle_wrap`: the focused buffer's lines wrapped, or not.
    pub(crate) fn toggle_wrap(&mut self) {
        let (buffer, pane) = (self.focused_buffer_id(), self.focused_pane_id());
        let on = !self.buffer_wraps(buffer);
        self.wrap_overrides.insert(buffer, on);
        // Every pane showing it starts its lines' rows afresh.
        let showing: Vec<fenix_window::WindowId> = self.windows().windows().into_iter().filter(|&p| self.windows().content(p) == Some(&buffer)).collect();
        for p in showing.into_iter().chain(std::iter::once(pane)) {
            let state = self.pane_state_mut(p);
            state.scroll_col = 0;
            state.scroll_row = 0;
        }
        let shown = if !on { "Wrap off" } else if self.pane_can_wrap(pane, buffer) { "Wrap on" } else { "Wrap on (not in this pane)" };
        self.set_message(shown);
        self.wake_caret();
    }

    /// `count` rows of `pane` from display row `top`, `skip` rows of its
    /// line above the top, as it shows `buffer` at its scroll.
    pub(super) fn pane_rows(&self, pane: fenix_window::WindowId, buffer: BufferId, top: usize, skip: usize, count: usize) -> RowMap {
        let lines = self.pane_display_lines(pane, buffer);
        let state = *self.pane_state(pane);
        let line = self.buffers.get(buffer).map_or(0, |ob| ob.buffer.line_col(&state.cursor).0);
        self.frame_layout(pane, buffer, &lines, top, skip, count, state.scroll_col, line, false).map
    }

    /// What a frame lays out of `pane` showing `buffer`: `count` rows
    /// from display line `top` of `display` (`skip` rows into it when the
    /// lines wrap), scrolled `scroll_col` sideways unless they wrap, the
    /// cursor on `cursor_line`; Home and a page (`dashboard`) never wrap.
    /// What `redraw`, the hit-tests and the tests all build rows by.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn frame_layout(&self, pane: fenix_window::WindowId, buffer: BufferId, display: &[usize], top: usize, skip: usize, count: usize, scroll_col: usize, cursor_line: usize, dashboard: bool) -> PaneLayout {
        let wrap = if dashboard { None } else { self.wrap_cols(pane, buffer) };
        let from = top.min(display.len());
        let want = RowsWanted {
            lines: &display[from..from.saturating_add(count).min(display.len())],
            skip: if wrap.is_some() { skip } else { 0 },
            count,
            scroll_col: if wrap.is_some() { 0 } else { scroll_col },
            wrap,
        };
        self.pane_layout(buffer, want, cursor_line, dashboard)
    }

    /// `buffer`'s text on the rows `want` asks for: the rows, each line's
    /// columns, its decorations laid out (the cursor's on `cursor_line`),
    /// and the indent guides (none on Home or a page, `dashboard`). Only
    /// the lines on the rows are read.
    pub(super) fn pane_layout(&self, buffer: BufferId, want: RowsWanted, cursor_line: usize, dashboard: bool) -> PaneLayout {
        let mut layout = PaneLayout { map: RowMap::default(), col_maps: HashMap::new(), decorated: HashMap::new(), decoration_rows: Vec::new(), indent_guides: Vec::new() };
        let Some(ob) = self.buffers.get(buffer) else { return layout };
        let tab_stops = self.tab_stops_for(buffer);
        // The file's own step (three spaces a level is three), not the
        // setting's alone: a guide on every column otherwise, or none.
        let indent_width = self.guide_indent_width(buffer);
        let decorations = if dashboard { Vec::new() } else { self.code_decorations(buffer) };
        let visual_lines = ob.buffer.visual_line_count();
        let mut rows: Vec<Row> = Vec::new();
        for &line in want.lines {
            if rows.len() >= want.count {
                break;
            }
            if line == NO_LINE || line >= visual_lines {
                rows.push(Row { line, kind: if line == NO_LINE { RowKind::Filler } else { RowKind::Text }, cols: 0..usize::MAX, indent: 0, first: true, last: true });
                continue;
            }
            let start = ob.buffer.line_start_char(line);
            let len = ob.buffer.line_len(line);
            let line_text = ob.buffer.text_range(start, start + len);
            let here: Vec<&crate::decorations::Decoration> = decorations.iter().copied().filter(|d| d.start <= start + len && d.end >= start).collect();
            // The line as drawn, when it differs from its text, and its
            // leading whitespace in display columns, for the guides.
            let (display, leading_columns) = if !here.is_empty() {
                let laid = crate::decorations::layout_line(&line_text, start, &here, &tab_stops, line == cursor_line);
                layout.col_maps.insert(line, laid.col_map.clone());
                let display = want.wrap.map(|_| laid.display.clone());
                layout.decoration_rows.push((line, laid.clone()));
                layout.decorated.insert(line, laid);
                (display, line_text.chars().take_while(|&c| c == ' ').count())
            } else if line_text.contains('\t') {
                let (display, col_map) = tabstops::expand_line(&line_text, &tab_stops);
                layout.col_maps.insert(line, col_map);
                let leading = display.chars().take_while(|&c| c == ' ').count();
                (Some(display), leading)
            } else {
                (None, line_text.chars().take_while(|&c| c == ' ').count())
            };
            let first_row = rows.len();
            match want.wrap {
                Some(width) => {
                    let display = display.as_deref().unwrap_or(&line_text);
                    let indent = crate::wrap::continuation_indent(display, width);
                    // Only as many rows as are wanted are found: a line
                    // far longer than the pane costs what it shows.
                    let skip = if first_row == 0 { want.skip } else { 0 };
                    let (breaks, whole) = crate::wrap::break_line_rows(display, width, indent, skip.saturating_add(want.count - rows.len()));
                    let n = breaks.len();
                    // Past the line's last row: its last.
                    let skip = skip.min(n - 1);
                    for (i, cols) in breaks.into_iter().enumerate().skip(skip) {
                        if rows.len() >= want.count {
                            break;
                        }
                        rows.push(Row { line, kind: RowKind::Text, cols, indent, first: i == 0, last: whole && i + 1 == n });
                    }
                }
                None => rows.push(Row { line, kind: RowKind::Text, cols: 0..usize::MAX, indent: 0, first: true, last: true }),
            }
            // Home's leading spaces are layout, not indentation.
            if !dashboard {
                for (r, row) in rows.iter().enumerate().skip(first_row) {
                    // A guide where the row has the line's indent: on a
                    // first row the columns it shows of it, on a
                    // continuation row its own indent and any of the
                    // line's it goes on showing.
                    let shown = |g: usize| if row.first { (g < row.cols.end).then_some(g) } else if g < row.indent { Some(g) } else { (g >= row.cols.start && g < row.cols.end).then(|| g - row.cols.start + row.indent).filter(|&x| x >= row.indent) };
                    layout.indent_guides.extend(super::indent_guide_columns(leading_columns, indent_width).filter_map(shown).map(|col| (r, col)));
                }
            }
        }
        layout.map = RowMap::from_rows(rows, want.scroll_col);
        if let Some(width) = want.wrap {
            layout.map = layout.map.wrapped_at(width);
        }
        layout
    }

    /// A wrapped pane's text, `rows` rows of it, as `map` lays them out:
    /// the gutter on a line's first row and blank on the rest, a
    /// continuation row drawn its indent in, a closed fold's `⋯ N lines`
    /// at the end of its last row, `~` past the end of the buffer.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn row_content_spans(
        &self,
        ob: &super::OpenBuffer,
        map: &RowMap,
        rows: usize,
        gutter_chars: usize,
        syntax_highlights: &[(Range<usize>, glyphon::Color)],
        cursor_line: usize,
        tab_stops: &super::TabStops,
        collapsed_headers: &std::collections::HashSet<usize>,
        decorated: &HashMap<usize, LineLayout>,
    ) -> Vec<(String, glyphon::Color)> {
        let theme = self.theme;
        let mut spans = Vec::new();
        let fold_marks = if gutter_chars > 0 { self.fold_marks(ob) } else { None };
        let fold_ends: HashMap<usize, usize> = if collapsed_headers.is_empty() { HashMap::new() } else { self.fold_ranges_of(ob).into_iter().collect() };
        let visual_lines = ob.buffer.visual_line_count();
        // The line being drawn, once for all its rows, and how far into
        // its text the rows so far have read: (chars, bytes).
        let mut drawn: Option<(usize, super::DrawnLine, usize, usize)> = None;
        for r in 0..rows {
            match map.row(r) {
                Some(row) if row.kind == RowKind::Filler => spans.push((" ".repeat(gutter_chars), theme.gutter_fg)),
                Some(row) if row.kind == RowKind::Text && row.line < visual_lines => {
                    if row.first {
                        self.push_gutter(ob, row.line, true, gutter_chars, None, cursor_line, fold_marks.as_ref(), &mut spans);
                    } else {
                        let blank = gutter_chars + row.indent;
                        if blank > 0 {
                            spans.push((" ".repeat(blank), theme.gutter_fg));
                        }
                    }
                    if drawn.as_ref().is_none_or(|d| d.0 != row.line) {
                        drawn = Some((row.line, self.drawn_line(ob, row.line, syntax_highlights, tab_stops, decorated), 0, 0));
                    }
                    if let Some((_, line, at_char, at_byte)) = drawn.as_mut() {
                        // Its rows come in order: read on from the last.
                        let advance = |to: usize, at_char: &mut usize, at_byte: &mut usize| {
                            for ch in line.text[*at_byte..].chars() {
                                if *at_char >= to {
                                    break;
                                }
                                *at_byte += ch.len_utf8();
                                *at_char += 1;
                            }
                        };
                        advance(row.cols.start, at_char, at_byte);
                        let from = *at_byte;
                        advance(row.cols.end, at_char, at_byte);
                        self.push_drawn(line, from..*at_byte, &mut spans);
                    }
                    if row.last && collapsed_headers.contains(&row.line) {
                        let hidden = fold_ends.get(&row.line).map_or(0, |end| end - row.line);
                        let used = if row.first { 0 } else { row.indent } + row.cols.end.saturating_sub(row.cols.start);
                        let room = map.width().unwrap_or(usize::MAX).saturating_sub(used);
                        let suffix = format!("  ⋯ {hidden} line{}", if hidden == 1 { "" } else { "s" });
                        // Cut to what's left of the row.
                        spans.push((suffix.chars().take(room).collect(), theme.gutter_fg));
                    }
                }
                _ => self.push_gutter(ob, visual_lines, false, gutter_chars, None, cursor_line, fold_marks.as_ref(), &mut spans),
            }
            if r + 1 < rows {
                spans.push(("\n".to_string(), theme.fg));
            }
        }
        spans
    }

    /// The focused pane's overlays on `layout`, the cursor at `line`,
    /// `col`: the caret and the extra ones, the selections, the pulse,
    /// the bracket pair and the search's matches. Worked out on each run
    /// of lines shown, so the lines in a closed fold aren't searched.
    pub(super) fn focused_overlays(&self, layout: &PaneLayout, line: usize, col: usize) -> FocusedOverlays {
        let span = layout.map.span();
        let by_runs = |f: &dyn Fn(Range<usize>) -> LineSegments| -> Segments { layout.map.runs().into_iter().flat_map(|run| layout.to_rows(f(run))).collect() };
        let buffer = &self.open().buffer;
        let mut extra_carets = Vec::new();
        let mut extra_selections = Segments::new();
        for c in self.extra_carets() {
            let (l, col) = buffer.line_col(&Cursor { char_idx: c.at, sticky_col: 0 });
            extra_carets.extend(layout.locate(l, col));
            if let Some(a) = c.anchor {
                extra_selections.extend(layout.to_rows(self.range_to_segments(a.min(c.at)..a.max(c.at), span.clone())));
            }
        }
        FocusedOverlays {
            extra_carets,
            selection: by_runs(&|run| self.visual_selection_segments(run)).into_iter().chain(extra_selections).collect(),
            pulse: self.pulse_overlay(span.clone()).map(|(segs, a)| (layout.to_rows(segs), a)),
            brackets: layout.to_rows(self.bracket_match_segments(span.clone())),
            // The search per run of lines, the `:s` preview once.
            hlsearch: by_runs(&|run| self.search_match_segments(run)).into_iter().chain(layout.to_rows(self.substitute_segments(span.clone()))).collect(),
            caret: self.focused_caret(line, col).and_then(|(line, col)| layout.locate(line, col)),
        }
    }

    /// The focused pane's layout as the next frame draws it, `rows` rows
    /// of it: for tests, which have no frame.
    #[cfg(test)]
    pub(super) fn focused_layout(&self, rows: usize) -> PaneLayout {
        let (pane, buffer) = (self.focused_pane_id(), self.focused_buffer_id());
        let state = *self.pane_state(pane);
        let top = state.rendered_scroll.floor().max(0.0) as usize;
        let line = self.open().buffer.line_col(&self.cursor()).0;
        self.frame_layout(pane, buffer, &self.pane_display_lines(pane, buffer), top, state.scroll_row, rows, state.scroll_col, line, false)
    }

    /// The focused pane's overlays as the next frame draws them.
    #[cfg(test)]
    pub(super) fn focused_overlays_now(&self, rows: usize) -> FocusedOverlays {
        let (line, col) = self.open().buffer.line_col(&self.cursor());
        self.focused_overlays(&self.focused_layout(rows), line, col)
    }
}

#[cfg(test)]
// A line of one row is a list of one range.
#[allow(clippy::single_range_in_vec_init)]
mod tests {
    use super::*;

    fn wrapped(line: usize, breaks: &[Range<usize>], indent: usize) -> Vec<Row> {
        let n = breaks.len();
        breaks.iter().enumerate().map(|(i, cols)| Row { line, kind: RowKind::Text, cols: cols.clone(), indent, first: i == 0, last: i + 1 == n }).collect()
    }

    #[test]
    fn a_row_a_line_is_the_line_less_the_top_and_the_column_less_the_scroll() {
        let map = RowMap::lines(&[4, 5, 6], 2);
        assert_eq!(map.first_row(5), Some(1));
        assert_eq!(map.first_row(3), None);
        assert_eq!(map.locate(6, 7), Some((2, 5)));
        // Left of the scroll: held to the pane's edge.
        assert_eq!(map.locate(4, 1), Some((0, 0)));
        assert_eq!(map.segments(5, 1..9), vec![(1, 0, 7)]);
        assert_eq!(map.segments(5, 3..3), vec![(1, 1, 1)]);
        assert_eq!(map.at(2, 3), Some((6, 5)));
        assert_eq!(map.at(3, 0), None);
        assert_eq!(map.runs(), [4..7]);
        assert_eq!(map.span(), 4..7);
    }

    #[test]
    fn right_of_a_short_row_the_line_goes_on_from_is_that_row_s_last_column() {
        let mut rows = wrapped(3, &[0..8, 8..14], 2);
        rows[0].cols = 0..6;
        rows[1].cols = 6..14;
        let map = RowMap::from_rows(rows, 0).wrapped_at(10);
        assert_eq!(map.at(0, 9), Some((3, 5)), "not column 9, which is the next row's");
        assert_eq!(map.at(1, 30), Some((3, 34)), "past the last row: past the line's end");
        // A caret past a row's last column is held to it.
        assert_eq!(map.locate(3, 30), Some((1, 9)));
    }

    #[test]
    fn the_columns_a_line_s_rows_show_never_overflow() {
        // A row running to its line's end, as past the buffer's last line.
        let map = RowMap::lines(&[0], 0).wrapped_at(10);
        assert_eq!(shown_char_columns(&map, 0, None), Some((0, usize::MAX)));
        let map = RowMap::from_rows(wrapped(3, &[0..8, 8..14], 0), 0);
        assert_eq!(shown_char_columns(&map, 3, None), Some((0, 15)));
        // Visual columns to chars: a tab at 0 is columns 0 to 7.
        let col_map: Vec<usize> = std::iter::once(0).chain(8..=20).collect();
        assert_eq!(shown_char_columns(&map, 3, Some(&col_map)), Some((0, 8)));
        assert_eq!(shown_char_columns(&map, 4, None), None);
    }

    #[test]
    fn a_closed_fold_splits_the_runs_and_its_lines_have_no_rows() {
        let map = RowMap::lines(&[0, 1, 9, 10], 0);
        assert_eq!(map.runs(), vec![0..2, 9..11]);
        assert_eq!(map.span(), 0..11);
        assert_eq!(map.first_row(9), Some(2));
        assert_eq!(map.locate(5, 0), None);
        assert!(map.segments(5, 0..3).is_empty());
    }

    #[test]
    fn filler_rows_have_no_line() {
        let map = RowMap::lines(&[0, NO_LINE, NO_LINE, 1], 0);
        assert_eq!(map.row(1).map(|r| r.kind), Some(RowKind::Filler));
        assert_eq!(map.first_row(1), Some(3));
        assert_eq!(map.at(2, 0), None);
        assert_eq!(map.lines_by_row(), vec![0, NO_LINE, NO_LINE, 1]);
        assert_eq!(map.text_lines().collect::<Vec<_>>(), vec![0, 1]);
    }

    #[test]
    fn a_wrapped_line_splits_a_range_across_its_rows_and_indents_the_rest() {
        // Line 3 is 0..8, 8..14, 14..20, continued two columns in.
        let mut rows = vec![Row { line: 2, kind: RowKind::Text, cols: 0..usize::MAX, indent: 0, first: true, last: true }];
        rows.extend(wrapped(3, &[0..8, 8..14, 14..20], 2));
        let map = RowMap::from_rows(rows, 0);
        assert_eq!(map.rows_of(3), 1..4);
        assert_eq!(map.locate(3, 7), Some((1, 7)));
        // A row's end is the next row's start ...
        assert_eq!(map.locate(3, 8), Some((2, 2)));
        // ... but past the last row's end is still the last row.
        assert_eq!(map.locate(3, 20), Some((3, 8)));
        assert_eq!(map.segments(3, 6..16), vec![(1, 6, 8), (2, 2, 8), (3, 2, 4)]);
        // A selection's sliver past the end of the line.
        assert_eq!(map.segments(3, 19..21), vec![(3, 7, 9)]);
        assert_eq!(map.at(2, 2), Some((3, 8)));
        assert_eq!(map.at(2, 0), Some((3, 8)), "the indent is the row's first column");
        assert_eq!(map.gutter_lines(), vec![2, 3, NO_LINE, NO_LINE]);
    }
}

#[cfg(test)]
mod app_tests {
    use super::super::test_keys::app_on_file;
    use super::super::*;

    fn standard(app: &mut App) {
        app.config.set_value("editor.keys", Some(fenix_config::Value::Text("standard".into())));
        app.sync_key_policy();
    }

    /// A Rust file with `fn a`'s body (lines 1 and 2) folded away: rows
    /// show lines 0, 3, 4, 5, 6.
    fn folded(text: &str) -> (App, super::super::test_keys::TestFile) {
        let (mut app, file) = app_on_file("a.rs", text);
        app.press("j");
        app.run_cmd("code.toggle_fold");
        let id = app.focused_buffer_id();
        assert_eq!(app.folded_display_lines(id)[..3], [0, 3, 4]);
        (app, file)
    }

    const FOLDED: &str = "fn a() {\n    x;\n}\nab cd\nef gh\nx\n";

    #[test]
    fn with_a_fold_closed_the_caret_and_the_search_s_matches_are_on_their_lines_rows() {
        let (mut app, _f) = folded(FOLDED);
        app.press_all("5 G / x Enter");
        let o = app.focused_overlays_now(10);
        // Line 1's `x` is folded away; line 5's is row 3.
        assert_eq!(o.hlsearch, vec![(3, 0, 1)]);
        assert_eq!(o.caret, Some((3, 0)));
        app.press_all("4 G V j");
        let o = app.focused_overlays_now(10);
        assert_eq!(o.selection, vec![(1, 0, 5), (2, 0, 5)], "lines 3 and 4, below the fold");
    }

    #[test]
    fn a_yank_s_pulse_is_drawn_below_a_closed_fold() {
        let (mut app, _f) = folded(FOLDED);
        app.press_all("4 G y y");
        let (segments, _) = app.focused_overlays_now(10).pulse.expect("a pulse");
        assert_eq!(segments, vec![(1, 0, 5)]);
    }

    #[test]
    fn extra_cursors_and_their_selections_are_drawn_with_a_fold_closed() {
        let (mut app, _f) = folded(FOLDED);
        app.press_all("4 G");
        standard(&mut app);
        assert_eq!(app.line_col().0, 3);
        app.press("C-M-Down");
        app.press_all("S-Right S-Right");
        let o = app.focused_overlays_now(10);
        // The cursor added is the primary: line 4's, on row 2; line 3's
        // is the extra one, on row 1.
        assert_eq!(o.caret, Some((2, 2)));
        assert_eq!(o.extra_carets, vec![(1, 2)]);
        assert_eq!(o.selection, vec![(2, 0, 2), (1, 0, 2)]);
    }

    #[test]
    fn a_tab_widens_the_columns_after_it_and_a_sideways_scroll_takes_them_off() {
        let (mut app, _f) = app_on_file("a.txt", "\tabc def\n");
        app.press_all("w w v e");
        let o = app.focused_overlays_now(10);
        // A tab is eight columns: `def` is chars 5..8, columns 12..15.
        assert_eq!(app.line_col(), (0, 7));
        assert_eq!(o.selection, vec![(0, 12, 15)]);
        assert_eq!(o.caret, Some((0, 14)));
        let pane = app.focused_pane_id();
        app.pane_state_mut(pane).scroll_col = 10;
        let o = app.focused_overlays_now(10);
        assert_eq!(o.selection, vec![(0, 2, 5)]);
        assert_eq!(o.caret, Some((0, 4)));
    }

    #[test]
    fn rows_below_the_top_of_a_scrolled_pane_are_its_lines_less_the_top() {
        let text: String = (0..60).map(|i| format!("line {i}\n")).collect();
        let (mut app, _f) = app_on_file("a.txt", &text);
        app.press_all("4 0 G v e");
        let pane = app.focused_pane_id();
        app.pane_state_mut(pane).rendered_scroll = 30.0;
        let o = app.focused_overlays_now(20);
        assert_eq!(o.selection, vec![(9, 0, 4)]);
        assert_eq!(o.caret, Some((9, 3)));
        // Off the rows shown: nothing drawn.
        app.pane_state_mut(pane).rendered_scroll = 0.0;
        let o = app.focused_overlays_now(20);
        assert!(o.selection.is_empty());
        assert_eq!(o.caret, None);
    }

    /// `text` in a file whose lines wrap at `cols` columns.
    fn wrapped(name: &str, text: &str, cols: usize) -> (App, super::super::test_keys::TestFile) {
        let (mut app, file) = app_on_file(name, text);
        app.run_cmd("view.toggle_wrap");
        let pane = app.focused_pane_id();
        app.set_pane_text_cols(pane, cols);
        (app, file)
    }

    /// What the focused pane's first `rows` rows draw, gutter and all.
    fn drawn(app: &App, rows: usize) -> Vec<String> {
        let layout = app.focused_layout(rows);
        let ob = app.open();
        let line = ob.buffer.line_col(&app.cursor()).0;
        let folds = app.code_folds.get(&app.focused_buffer_id()).cloned().unwrap_or_default();
        let spans = app.row_content_spans(ob, &layout.map, rows, app.gutter_chars(ob), &[], line, &app.tab_stops_for(app.focused_buffer_id()), &folds, &layout.decorated);
        spans.into_iter().map(|(t, _)| t).collect::<String>().split('\n').map(str::to_string).collect()
    }

    #[test]
    fn a_long_line_goes_on_under_itself_with_its_number_on_the_first_row_only() {
        let (mut app, _f) = wrapped("a.txt", "the quick brown fox jumps\nshort\n", 10);
        app.line_number_mode = LineNumberMode::Absolute;
        let rows = drawn(&app, 5);
        let gutter = app.gutter_chars(app.open());
        let text: Vec<&str> = rows.iter().map(|r| &r[gutter..]).collect();
        assert_eq!(text, vec!["the quick ", "brown fox ", "jumps", "short", ""]);
        assert!(rows[0].trim_start().starts_with('1'), "{rows:?}");
        assert!(rows[1][..gutter].trim().is_empty() && rows[2][..gutter].trim().is_empty(), "{rows:?}");
        assert!(rows[3].trim_start().starts_with('2'), "{rows:?}");
    }

    #[test]
    fn a_continuation_row_keeps_the_line_s_indent_and_the_guides_go_down_it() {
        let (mut app, _f) = wrapped("a.txt", "        let total = first + second;\n", 20);
        app.line_number_mode = LineNumberMode::Off;
        assert_eq!(drawn(&app, 3), vec!["        let total = ", "        first + ", "        second;"]);
        let guides = app.focused_layout(3).indent_guides;
        // The file's step is its one indent, eight: a guide at column 0.
        assert_eq!(guides, vec![(0, 0), (1, 0), (2, 0)]);
    }

    #[test]
    fn a_word_wider_than_the_pane_tabs_wide_chars_and_marks_are_broken_by_columns() {
        let (mut app, _f) = wrapped("a.txt", "abcdefghijklmnop\n\tab cd\n日本語の文章です\ne\u{301}e\u{301}e\u{301}\n", 6);
        app.line_number_mode = LineNumberMode::Off;
        // Five columns of text a row, the sixth the caret's.
        assert_eq!(
            drawn(&app, 12),
            vec!["abcde", "fghij", "klmno", "p", "     ", "   ab ", "cd", "日本語の文", "章です", "e\u{301}e\u{301}", "e\u{301}", "~"],
            "a tab is eight columns, wider than the pane: broken inside, and no indent after it",
        );
    }

    #[test]
    fn an_empty_line_a_line_one_short_of_the_pane_and_the_last_line_without_a_newline() {
        let (mut app, _f) = wrapped("a.txt", "\r\nabcde\r\nabcdef\r\nabcdefg", 6);
        app.line_number_mode = LineNumberMode::Off;
        assert_eq!(drawn(&app, 7), vec!["", "abcde", "abcde", "f", "abcde", "fg", "~"], "CRLF's \\r isn't a column; a full row is one short of the pane");
        // The caret after a full row's last char is in the pane's last column.
        app.press_all("j A");
        assert_eq!(app.focused_overlays_now(7).caret, Some((1, 5)));
        app.press_all("Esc j A");
        assert_eq!(app.focused_overlays_now(7).caret, Some((3, 1)));
    }

    #[test]
    fn a_closed_fold_s_count_goes_at_the_end_of_its_header_s_last_row() {
        let (mut app, _f) = folded("fn a_long_function_name(first: u32, second: u32) {\n    x;\n}\nfn b() {}\n");
        app.run_cmd("view.toggle_wrap");
        let pane = app.focused_pane_id();
        app.set_pane_text_cols(pane, 31);
        app.line_number_mode = LineNumberMode::Off;
        let rows = drawn(&app, 3);
        // `first:` ends at the text's edge, its blank hanging in the last column.
        assert_eq!(rows, ["fn a_long_function_name(first: ", "u32, second: u32) {  ⋯ 2 lines", "fn b() {}"]);
        // In a narrower pane the count is cut to what's left of the row.
        app.set_pane_text_cols(pane, 20);
        let rows = drawn(&app, 5);
        assert_eq!(rows[..4], ["fn ", "a_long_function_nam", "e(first: u32, ", "second: u32) {  ⋯ 2 "], "fourteen columns of text, six left for the count");
    }

    #[test]
    fn a_selection_and_the_caret_split_across_a_line_s_rows() {
        let (mut app, _f) = wrapped("a.txt", "the quick brown fox jumps\n", 10);
        // From `quick` to `fox`: the end of row 0, all of row 1.
        app.press_all("w v 3 e");
        let o = app.focused_overlays_now(5);
        assert_eq!(o.selection, vec![(0, 4, 10), (1, 0, 9)]);
        assert_eq!(o.caret, Some((1, 8)));
        // Each row's own: the line's row with the caret is highlighted.
        app.press("Esc");
        app.press("$");
        assert_eq!(app.focused_overlays_now(5).caret, Some((2, 4)));
    }

    #[test]
    fn a_block_selection_across_wrapped_lines_and_extra_cursors_on_continuation_rows() {
        let (mut app, _f) = wrapped("a.txt", "abcdefgh\nijklmnop\n", 5);
        // Columns 3 to 6 of both lines: across each line's break.
        app.press_all("3 l C-v j 3 l");
        let o = app.focused_overlays_now(6);
        // `abcd` / `efgh`: columns 3 to 6 are `d` and `efg`.
        assert_eq!(o.selection, vec![(0, 3, 4), (1, 0, 3), (2, 3, 4), (3, 0, 3)]);
        app.press_all("Esc g g");
        standard(&mut app);
        app.press_all("Home C-M-Down");
        app.press_all("End");
        let o = app.focused_overlays_now(6);
        // Both ends of the lines, on their second rows.
        assert_eq!(o.caret, Some((3, 4)));
        assert_eq!(o.extra_carets, vec![(1, 4)]);
    }

    #[test]
    fn a_line_taller_than_the_pane_shows_the_rows_below_its_scroll_row() {
        let line = "word ".repeat(40);
        let (mut app, _f) = wrapped("a.txt", &format!("{line}\nnext\n"), 10);
        app.line_number_mode = LineNumberMode::Off;
        let pane = app.focused_pane_id();
        assert_eq!(drawn(&app, 3), vec!["word word ", "word word ", "word word "]);
        // Its last blank has a row of its own (the caret after it needs a
        // cell): twenty-one rows.
        app.pane_state_mut(pane).scroll_row = 19;
        assert_eq!(drawn(&app, 3), vec!["word word", " ", "next"]);
        // Rows above the top hold nothing a caret could be drawn on.
        assert_eq!(app.focused_overlays_now(3).caret, None);
    }

    #[test]
    fn a_resize_or_a_split_wraps_again_at_each_pane_s_width() {
        let (mut app, _f) = wrapped("a.txt", "aaaa bbbb cccc\n", 10);
        app.line_number_mode = LineNumberMode::Off;
        assert_eq!(drawn(&app, 2), vec!["aaaa bbbb ", "cccc"]);
        let pane = app.focused_pane_id();
        app.set_pane_text_cols(pane, 5);
        assert_eq!(drawn(&app, 3), vec!["aaaa ", "bbbb ", "cccc"]);
        app.set_pane_text_cols(pane, 1);
        assert_eq!(drawn(&app, 3), vec!["a", "a", "a"], "a pane a column wide is a char a row");
        app.run_cmd("window.split_vertical");
        let other = app.focused_pane_id();
        assert_ne!(other, pane);
        app.set_pane_text_cols(other, 20);
        assert_eq!(drawn(&app, 2), vec!["aaaa bbbb cccc", "~"], "the buffer wraps in each pane at its own width");
    }

    #[test]
    fn with_wrap_off_again_a_line_is_a_row_and_scrolls_sideways() {
        let (mut app, _f) = wrapped("a.txt", "the quick brown fox jumps\n", 10);
        app.run_cmd("view.toggle_wrap");
        app.line_number_mode = LineNumberMode::Off;
        assert_eq!(drawn(&app, 2), vec!["the quick brown fox jumps", "~"]);
        assert_eq!(app.wrap_cols(app.focused_pane_id(), app.focused_buffer_id()), None);
    }

    #[test]
    fn a_diff_or_a_table_never_wraps() {
        let (mut app, _f) = wrapped("a.txt", "x\n", 10);
        let (pane, buffer) = (app.focused_pane_id(), app.focused_buffer_id());
        assert_eq!(app.wrap_cols(pane, buffer), Some(10));
        for kind in [BufferKind::Diff, BufferKind::Table, BufferKind::Git, BufferKind::Merge, BufferKind::Graph, BufferKind::Terminal, BufferKind::Dashboard] {
            app.buffers.get_mut(buffer).unwrap().kind = kind;
            assert_eq!(app.wrap_cols(pane, buffer), None, "{kind:?}");
        }
    }

    #[test]
    fn a_fold_marker_is_on_its_line_s_first_row_only() {
        let (mut app, _f) = app_on_file("a.rs", "fn a_long_function_name(first: u32, second: u32) {\n    x;\n}\n");
        app.line_number_mode = LineNumberMode::Absolute;
        app.run_cmd("view.toggle_wrap");
        let pane = app.focused_pane_id();
        app.set_pane_text_cols(pane, 30);
        let marker = app.gutter_chars(app.open()) - 2;
        let buffer = app.focused_buffer_id();
        assert!(!app.fold_marker_click(pane, 1, marker), "row 1 is the header's second row");
        assert!(!app.code_folds.contains_key(&buffer));
        assert!(app.fold_marker_click(pane, 0, marker));
        assert_eq!(app.folded_display_lines(buffer)[..2], [0, 3]);
    }

    #[test]
    fn an_extension_s_place_in_the_text_is_on_its_row_in_a_wrapped_pane() {
        let (app, _f) = wrapped("a.txt", "the quick brown fox jumps\nnext\n", 10);
        let map = app.focused_layout(10).map;
        let pane = super::super::layers::GeoPane {
            id: 1,
            rect: fenix_window::Rect { x: 0.0, y: 0.0, w: 400.0, h: 400.0 },
            text_x: 0.0,
            text_y: 0.0,
            base_line: 0,
            scroll_col: 0,
            rows: 10,
            focused: true,
            map: Some(map),
        };
        let geometry = super::super::layers::Geometry { cell: (10.0, 20.0), panes: vec![pane], ..Default::default() };
        let at = |line: u32, col: u32| geometry.resolve(&fenix_wasm::host::Place { anchor: fenix_wasm::host::Anchor::Text((1, line, col)), x: 0.0, y: 0.0 });
        assert_eq!(at(0, 12), Some((20.0, 20.0)), "`brown`'s `o`: row 1, cell 2");
        assert_eq!(at(1, 0), Some((0.0, 60.0)), "line 1 is row 3");
    }

    #[test]
    fn a_cursor_below_the_pane_s_bottom_draws_no_caret_and_no_line_highlight() {
        let line = "word ".repeat(40);
        let (mut app, _f) = wrapped("a.txt", &format!("{line}\nnext\n"), 10);
        app.press("$");
        let layout = app.focused_layout(3);
        assert_eq!(layout.locate(0, 199), None, "row 19 of the line, the pane showing 0 to 2");
        assert_eq!(app.focused_overlays_now(3).caret, None);
        assert!(layout.map.segments(0, 199..199).is_empty());
    }

    #[test]
    fn a_scroll_row_past_the_line_s_rows_shows_its_last_and_its_number_isn_t_drawn() {
        let line = "word ".repeat(40);
        let (mut app, _f) = wrapped("a.txt", &format!("{line}\nnext\n"), 10);
        let pane = app.focused_pane_id();
        app.pane_state_mut(pane).scroll_row = 100;
        let layout = app.focused_layout(3);
        assert_eq!(layout.map.row(0).map(|r| r.last), Some(true));
        // The top inside line 0: its first row isn't shown, and neither
        // its number nor its gutter marks are.
        assert_eq!(layout.map.first_row(0), None);
        assert_eq!(layout.map.gutter_lines(), vec![super::NO_LINE, 1, 2]);
        assert_eq!(layout.map.first_row(1), Some(1));
        assert_eq!(layout.map.rows_of(0), 0..1, "its rows shown are still its own");
    }

    #[test]
    fn a_continuation_row_s_number_and_marks_aren_t_drawn_with_relative_numbers_either() {
        let (mut app, _f) = wrapped("a.txt", "x\nthe quick brown fox\n", 10);
        app.line_number_mode = LineNumberMode::Relative;
        app.press("j");
        let rows = drawn(&app, 3);
        let gutter = app.gutter_chars(app.open());
        assert!(rows[1][..gutter].trim().starts_with('0'), "{rows:?}");
        assert!(rows[2][..gutter].trim().is_empty(), "{rows:?}");
    }

    #[test]
    fn indent_guides_are_on_the_columns_each_row_shows_of_the_indent() {
        // Twenty spaces at six columns: rows `     ` five times... the
        // indent wider than the pane, so no continuation indent.
        let (app, _f) = wrapped("a.txt", &format!("{}x\n    y\n", " ".repeat(20)), 6);
        let guides = app.focused_layout(6).indent_guides;
        let on = |row: usize| guides.iter().filter(|(r, _)| *r == row).map(|&(_, c)| c).collect::<Vec<_>>();
        // The file's step is four: guides at 0, 4, 8, 12, 16 of the line.
        assert_eq!(on(0), vec![0, 4], "row 0 shows columns 0 to 5");
        assert_eq!(on(1), vec![3], "row 1 shows 5 to 9: the guide at 8");
        assert_eq!(on(2), vec![2], "row 2 shows 10 to 14: 12");
        assert_eq!(on(3), vec![1], "row 3 shows 15 to 19: 16");
    }

    #[test]
    fn a_syntax_colour_across_a_break_goes_on_both_rows_with_a_tab_too() {
        let red = glyphon::Color::rgb(200, 0, 0);
        let colours = |app: &App, highlights: &[(std::ops::Range<usize>, glyphon::Color)]| -> Vec<(String, bool)> {
            let layout = app.focused_layout(3);
            let ob = app.open();
            app.row_content_spans(ob, &layout.map, 3, 0, highlights, 0, &app.tab_stops_for(app.focused_buffer_id()), &Default::default(), &layout.decorated)
                .into_iter()
                .filter(|(t, _)| !t.is_empty() && t != "\n" && t != "~")
                .map(|(t, c)| (t, c == red))
                .collect()
        };
        let (mut app, _f) = wrapped("a.txt", "the quick brown\n", 10);
        app.line_number_mode = LineNumberMode::Off;
        // `quick brown`, bytes 4 to 15.
        assert_eq!(colours(&app, &[(4..15, red)]), vec![("the ".into(), false), ("quick ".into(), true), ("brown".into(), true)]);
        let (mut app, _f) = wrapped("a.txt", "\tquick brown\n", 12);
        app.line_number_mode = LineNumberMode::Off;
        // The tab is eight columns, and `quick` goes whole to the next
        // row, six columns in.
        let indent = ("      ".to_string(), false);
        assert_eq!(colours(&app, &[(1..12, red)]), vec![("        ".into(), false), indent.clone(), ("quick ".into(), true), indent, ("brown".into(), true)]);
    }

    #[test]
    fn a_line_selection_a_search_match_and_a_bracket_split_across_rows() {
        let (mut app, _f) = wrapped("a.txt", "f(the quick) brown\n", 10);
        // `f(the ` / `quick) ` / `brown`.
        app.press("V");
        assert_eq!(app.focused_overlays_now(3).selection, vec![(0, 0, 6), (1, 0, 7), (2, 0, 5)]);
        app.press_all("Esc /");
        app.type_text("e q");
        app.press("Enter");
        assert_eq!(app.focused_overlays_now(3).hlsearch, vec![(0, 4, 6), (1, 0, 1)], "`e q` across the break");
        app.press_all("0 l");
        assert_eq!(app.focused_overlays_now(3).brackets, vec![(0, 1, 2), (1, 5, 6)]);
    }

    #[test]
    fn a_blank_run_longer_than_the_pane_goes_on_to_the_next_rows_and_every_cell_is_its_own() {
        // `ab` + 12 blanks + `cd` at five columns: blanks hang up to the
        // kept column, the rest of the run goes on, `cd` after it.
        let (mut app, _f) = wrapped("a.txt", &format!("ab{}cd\n", " ".repeat(12)), 5);
        app.line_number_mode = LineNumberMode::Off;
        assert_eq!(drawn(&app, 5), vec!["ab   ", "    ", "    ", " cd", "~"]);
        let layout = app.focused_layout(6);
        let map = &layout.map;
        for col in 0..16 {
            let (row, x) = map.locate(0, col).unwrap();
            assert!(x < 5, "column {col} at cell {x}");
            assert_eq!(map.at(row, x), Some((0, col)), "column {col} and its cell are each other's");
        }
        // A selection of the whole line never runs past the pane.
        assert!(map.segments(0, 0..16).iter().all(|&(_, _, e)| e <= 5), "{:?}", map.segments(0, 0..16));
    }

    #[test]
    fn a_fold_s_count_after_a_row_its_blanks_fill_has_no_room_and_isn_t_drawn() {
        let (mut app, _f) = folded("fn a() {    \n    x;\n}\nfn b() {}\n");
        app.run_cmd("view.toggle_wrap");
        let pane = app.focused_pane_id();
        app.set_pane_text_cols(pane, 9);
        app.line_number_mode = LineNumberMode::Off;
        let rows = drawn(&app, 4);
        assert_eq!(rows[0], "fn a() { ", "{rows:?}");
        assert!(rows[1].chars().count() <= 9, "{rows:?}");
    }

    #[test]
    fn a_prompt_wraps_a_pane_of_no_width_is_one_column_and_an_undrawn_pane_is_eighty() {
        let (mut app, _f) = wrapped("a.txt", "x\n", 10);
        let (pane, buffer) = (app.focused_pane_id(), app.focused_buffer_id());
        app.buffers.get_mut(buffer).unwrap().kind = BufferKind::Prompt;
        assert_eq!(app.wrap_cols(pane, buffer), Some(10));
        app.set_pane_text_cols(pane, 0);
        assert_eq!(app.wrap_cols(pane, buffer), Some(1));
        app.pane_text_cols.clear();
        assert_eq!(app.wrap_cols(pane, buffer), Some(80));
    }

    #[test]
    fn the_gutter_marks_go_beside_a_line_s_first_row_only() {
        let (app, _f) = folded(FOLDED);
        let layout = app.focused_layout(10);
        assert_eq!(layout.map.gutter_lines(), vec![0, 3, 4, 5, 6]);
        assert_eq!(layout.map.runs(), vec![0..1, 3..7]);
    }
}
