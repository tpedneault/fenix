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

use super::{tabstops, App, BufferId, Cursor, LineSegments, Segments};
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
    /// Each line shown and its first row, by line.
    firsts: Vec<(usize, usize)>,
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
        RowMap { rows, scroll_col, firsts }
    }

    pub(crate) fn row(&self, row: usize) -> Option<&Row> {
        self.rows.get(row)
    }

    pub(crate) fn rows(&self) -> &[Row] {
        &self.rows
    }

    /// The row `line` starts on, if it's shown.
    pub(crate) fn first_row(&self, line: usize) -> Option<usize> {
        let i = self.firsts.binary_search_by_key(&line, |&(l, _)| l).ok()?;
        Some(self.firsts[i].1)
    }

    /// The rows `line` is on (empty when it isn't shown).
    pub(crate) fn rows_of(&self, line: usize) -> Range<usize> {
        let Some(first) = self.first_row(line) else { return 0..0 };
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
    /// `None` when the line isn't shown. A column at the end of a row
    /// that isn't the line's last is the next row's start.
    pub(crate) fn locate(&self, line: usize, col: usize) -> Option<(usize, usize)> {
        let rows = self.rows_of(line);
        let at = rows.clone().find(|&i| col < self.rows[i].cols.end || self.rows[i].last).or(rows.clone().last())?;
        Some((at, self.x(&self.rows[at], col)))
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
                out.push((i, self.x(row, lo), self.x(row, hi)));
            }
        }
        out
    }

    /// What's under cell `x` of `row`: its line and visual column, `None`
    /// on a row of no line.
    pub(crate) fn at(&self, row: usize, x: usize) -> Option<(usize, usize)> {
        let r = self.rows.get(row).filter(|r| r.kind == RowKind::Text)?;
        let indent = if r.first { 0 } else { r.indent };
        Some((r.line, r.cols.start + (x + self.scroll_col).saturating_sub(indent)))
    }
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

impl App {
    /// `count` rows of `pane`, from display row `top`, as it shows
    /// `buffer` at its scroll.
    pub(super) fn pane_rows(&self, pane: fenix_window::WindowId, buffer: BufferId, top: usize, count: usize) -> RowMap {
        let lines = self.pane_display_lines(pane, buffer);
        let from = top.min(lines.len());
        RowMap::lines(&lines[from..from.saturating_add(count).min(lines.len())], self.pane_state(pane).scroll_col)
    }

    /// `buffer`'s text on the rows of `map`: each line's columns, its
    /// decorations laid out (the cursor's on `cursor_line`), and the
    /// indent guides (none on Home or a page, `dashboard`). Only the
    /// lines on the rows are read.
    pub(super) fn pane_layout(&self, buffer: BufferId, map: RowMap, cursor_line: usize, dashboard: bool) -> PaneLayout {
        let mut layout = PaneLayout { map, col_maps: HashMap::new(), decorated: HashMap::new(), decoration_rows: Vec::new(), indent_guides: Vec::new() };
        let Some(ob) = self.buffers.get(buffer) else { return layout };
        let tab_stops = self.tab_stops_for(buffer);
        // The file's own step (three spaces a level is three), not the
        // setting's alone: a guide on every column otherwise, or none.
        let indent_width = self.guide_indent_width(buffer);
        let decorations = if dashboard { Vec::new() } else { self.code_decorations(buffer) };
        let visual_lines = ob.buffer.visual_line_count();
        for (r, row) in layout.map.rows().iter().enumerate() {
            let line = row.line;
            if row.kind != RowKind::Text || line >= visual_lines {
                continue;
            }
            let start = ob.buffer.line_start_char(line);
            let len = ob.buffer.line_len(line);
            let line_text = ob.buffer.text_range(start, start + len);
            let here: Vec<&crate::decorations::Decoration> = decorations.iter().copied().filter(|d| d.start <= start + len && d.end >= start).collect();
            // Its leading whitespace in display columns, for the guides.
            let leading_columns = if !here.is_empty() {
                let laid = crate::decorations::layout_line(&line_text, start, &here, &tab_stops, line == cursor_line);
                layout.col_maps.insert(line, laid.col_map.clone());
                layout.decoration_rows.push((line, laid.clone()));
                layout.decorated.insert(line, laid);
                line_text.chars().take_while(|&c| c == ' ').count()
            } else if line_text.contains('\t') {
                let (display, col_map) = tabstops::expand_line(&line_text, &tab_stops);
                layout.col_maps.insert(line, col_map);
                display.chars().take_while(|&c| c == ' ').count()
            } else {
                line_text.chars().take_while(|&c| c == ' ').count()
            };
            // Home's leading spaces are layout, not indentation.
            if !dashboard {
                layout.indent_guides.extend(super::indent_guide_columns(leading_columns, indent_width).map(|col| (r, col)));
            }
        }
        layout
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
            brackets: layout.to_rows(self.bracket_match_segments(span)),
            hlsearch: by_runs(&|run| self.hlsearch_segments(run)),
            caret: self.focused_caret(line, col).and_then(|(line, col)| layout.locate(line, col)),
        }
    }

    /// The focused pane's layout as the next frame draws it, `rows` rows
    /// of it: for tests, which have no frame.
    #[cfg(test)]
    pub(super) fn focused_layout(&self, rows: usize) -> PaneLayout {
        let (pane, buffer) = (self.focused_pane_id(), self.focused_buffer_id());
        let top = self.pane_state(pane).rendered_scroll.floor().max(0.0) as usize;
        let line = self.open().buffer.line_col(&self.cursor()).0;
        self.pane_layout(buffer, self.pane_rows(pane, buffer, top, rows), line, false)
    }

    /// The focused pane's overlays as the next frame draws them.
    #[cfg(test)]
    pub(super) fn focused_overlays_now(&self, rows: usize) -> FocusedOverlays {
        let (line, col) = self.open().buffer.line_col(&self.cursor());
        self.focused_overlays(&self.focused_layout(rows), line, col)
    }
}

#[cfg(test)]
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
        assert_eq!(map.runs(), vec![4..7]);
        assert_eq!(map.span(), 4..7);
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

    #[test]
    fn the_gutter_marks_go_beside_a_line_s_first_row_only() {
        let (app, _f) = folded(FOLDED);
        let layout = app.focused_layout(10);
        assert_eq!(layout.map.gutter_lines(), vec![0, 3, 4, 5, 6]);
        assert_eq!(layout.map.runs(), vec![0..1, 3..7]);
    }
}
