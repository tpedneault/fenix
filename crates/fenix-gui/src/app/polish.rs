//! The static polish drawn into a code pane: problems underlined in the
//! text with their message at the end of the line, brackets coloured by
//! depth, whitespace shown inside a selection, the overview ruler down
//! the right edge, and sticky scroll's pinned scope headers. Each is
//! worked out while the pane's buffer is at hand, as rows of spans and
//! marks for the frame to draw.

use super::*;
use super::motion_host::{Ruler, RulerMark};

/// What `polish_pane` needs to know about a pane besides its buffer.
pub(super) struct PaneView<'a> {
    pub buffer: BufferId,
    /// What is on each screen row.
    pub map: &'a super::rows::RowMap,
    pub rows: usize,
    /// Whether its lines wrap: the ruler's band is then the lines shown.
    pub wrapped: bool,
    pub gutter_chars: usize,
    pub scroll_col: usize,
    pub rendered_scroll: f32,
    pub cursor_line: usize,
    /// Lines holding a tab or a decoration: their columns don't map one
    /// to one.
    pub tab_lines: &'a HashSet<usize>,
    /// The Visual selection, as `(row, start_col, end_col)`.
    pub selection: &'a Segments,
    /// A line's char column -> its visual column.
    pub vcol: &'a dyn Fn(usize, usize) -> usize,
    /// The pane's extra cursors (standard keys), for the ruler.
    pub carets: &'a [usize],
    /// The pane it is, for what its ruler's click snaps to.
    pub pane: fenix_window::WindowId,
}

/// What `polish_pane` hands back to draw.
#[derive(Default)]
pub(super) struct PanePolish {
    pub squiggles: Vec<(usize, usize, usize, [f32; 4])>,
    pub ruler: Option<Ruler>,
}

/// How serious a problem is, most serious first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Severity {
    Error,
    Warning,
    Info,
}

fn severity(d: &lsp_types::Diagnostic) -> Severity {
    match d.severity {
        Some(lsp_types::DiagnosticSeverity::WARNING) => Severity::Warning,
        Some(lsp_types::DiagnosticSeverity::INFORMATION) | Some(lsp_types::DiagnosticSeverity::HINT) => Severity::Info,
        _ => Severity::Error,
    }
}

impl App {
    fn severity_color(&self, s: Severity) -> glyphon::Color {
        match s {
            Severity::Error => self.theme.git_conflicted,
            Severity::Warning => self.theme.git_modified,
            Severity::Info => self.theme.gutter_fg,
        }
    }

    /// The problems drawn for `buffer`: its language server's latest,
    /// except while you're typing into it, when the last ones drawn stay
    /// until you pause -- so a half-typed word isn't underlined letter
    /// by letter.
    fn shown_diagnostics(&mut self, buffer: BufferId, now: Instant) -> Vec<lsp_types::Diagnostic> {
        let Some(path) = self.buffers.get(buffer).and_then(|ob| ob.buffer.path()).map(|p| fenix_lsp::normalize(std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf()))) else {
            return Vec::new();
        };
        let current = self.all_diagnostics(&path);
        let delay = Duration::from_millis(self.config.polish.diagnostics_delay_ms.unwrap_or(400));
        let typing = self.vim.mode() == Mode::Insert
            && self.focused_buffer_id() == buffer
            && self.motion.last_typed.is_some_and(|t| now.saturating_duration_since(t) < delay);
        if typing {
            if let Some(shown) = self.motion.shown_diagnostics.get(&path) {
                return shown.clone();
            }
        }
        self.motion.shown_diagnostics.insert(path, current.clone());
        current
    }

    /// When a problem held back while typing is due to be drawn.
    pub(super) fn diagnostics_wake(&self) -> Option<Instant> {
        let delay = Duration::from_millis(self.config.polish.diagnostics_delay_ms.unwrap_or(400));
        self.motion.last_typed.map(|t| t + delay)
    }

    /// Works out a code pane's polish and writes what belongs in its text
    /// (problem colours and messages, bracket colours, selection
    /// whitespace) into `spans`.
    pub(super) fn polish_pane(&mut self, view: &PaneView, spans: &mut RowSpans, now: Instant) -> PanePolish {
        let mut out = PanePolish::default();
        let polish = self.config.polish.clone();
        let mut rows = split_rows(std::mem::take(spans));

        // Problems.
        let inline = polish.diagnostics_inline.as_deref().unwrap_or("all");
        let messages = polish.diagnostics_message.as_deref().unwrap_or("cursor");
        let diagnostics = if inline == "off" && messages == "off" && polish.overview_ruler == Some(false) {
            Vec::new()
        } else {
            self.shown_diagnostics(view.buffer, now)
        };
        let mut problem_lines: Vec<(usize, Severity)> = Vec::new();
        if let Some(ob) = self.buffers.get(view.buffer) {
            let rope = ob.buffer.rope();
            let mut worst: HashMap<usize, (Severity, String)> = HashMap::new();
            for d in &diagnostics {
                let sev = severity(d);
                let start = fenix_lsp::position_to_char_offset(rope, d.range.start).min(ob.buffer.len_chars());
                let end = fenix_lsp::position_to_char_offset(rope, d.range.end).min(ob.buffer.len_chars()).max(start);
                let (start_line, start_col) = ob.buffer.line_col(&Cursor { char_idx: start, sticky_col: 0 });
                let (end_line, end_col) = ob.buffer.line_col(&Cursor { char_idx: end, sticky_col: 0 });
                problem_lines.push((start_line, sev));
                let entry = worst.entry(start_line).or_insert((sev, String::new()));
                if sev <= entry.0 {
                    *entry = (sev, d.message.lines().next().unwrap_or("").to_string());
                }
                let shown = match inline {
                    "off" => false,
                    "errors" => sev == Severity::Error,
                    _ => true,
                };
                if !shown {
                    continue;
                }
                let [r, g, b, _] = glyphon_to_rgba(self.severity_color(sev));
                let alpha = if sev == Severity::Info { 0.6 } else { 0.95 };
                for line in start_line..=end_line {
                    let s = if line == start_line { start_col } else { 0 };
                    let e = if line == end_line { end_col } else { ob.buffer.line_len(line) };
                    let raw_end = e;
                    for (row, s, e) in view.map.segments(line, (view.vcol)(line, s)..(view.vcol)(line, e)) {
                        // All of it left of a pane scrolled sideways: nothing to mark.
                        if view.scroll_col > 0 && e == 0 && raw_end > 0 {
                            continue;
                        }
                        out.squiggles.push((row, s, e.max(s + 1), [r, g, b, alpha]));
                    }
                }
            }
            for (line, (sev, message)) in &worst {
                // The number is on the line's first row, the message at
                // the end of its last.
                let on = view.map.rows_of(*line);
                if on.is_empty() {
                    continue;
                }
                let color = self.severity_color(*sev);
                if view.gutter_chars > 0 && inline != "off" {
                    // The line number takes the problem's colour.
                    if let Some(first) = rows.get_mut(on.start).and_then(|cells| cells.first_mut()) {
                        first.1 = color;
                    }
                }
                let Some(cells) = rows.get_mut(on.end - 1) else { continue };
                let show_message = match messages {
                    "all" => true,
                    "cursor" => *line == view.cursor_line,
                    _ => false,
                };
                if show_message && !message.is_empty() {
                    let faded = glyphon::Color::rgba(color.r(), color.g(), color.b(), 200);
                    cells.push((format!("    ■ {message}"), faded, false));
                }
            }
        }

        // Brackets by depth.
        if polish.rainbow_brackets == Some(true) {
            let depths = self.bracket_depths(view.buffer);
            let palette = self.theme.brackets.clone();
            if let Some(ob) = self.buffers.get(view.buffer) {
                for (row, cells) in rows.iter_mut().enumerate().take(view.rows) {
                    let Some(shown) = view.map.row(row).filter(|r| r.kind == super::rows::RowKind::Text) else { continue };
                    let line = shown.line;
                    if view.tab_lines.contains(&line) || line >= ob.buffer.line_count() {
                        continue;
                    }
                    // The chars on this row (a column a char on a line
                    // without tabs), and how far in it's drawn.
                    let line_start = ob.buffer.line_start_char(line);
                    let start = line_start + shown.cols.start.min(ob.buffer.line_len(line)) + view.scroll_col;
                    let end = line_start + shown.cols.end.min(ob.buffer.line_len(line));
                    let indent = if shown.first { 0 } else { shown.indent };
                    let from = depths.partition_point(|(at, _)| *at < start);
                    let here: HashMap<usize, usize> = depths[from..].iter().take_while(|(at, _)| *at < end).map(|&(at, d)| (at - start + indent, d)).collect();
                    if here.is_empty() {
                        continue;
                    }
                    edit_row_chars(cells, view.gutter_chars, |col, ch, color| match here.get(&col) {
                        // Only a bracket: with the tree behind the text
                        // (conflict markers stop it being read) a stale
                        // place isn't coloured.
                        Some(depth) if !palette.is_empty() && "()[]{}".contains(ch) => (ch, palette[depth % palette.len()]),
                        _ => (ch, color),
                    });
                }
            }
        }

        // Whitespace inside the selection.
        if polish.selection_whitespace != Some(false) && !view.selection.is_empty() {
            let dot = self.theme.gutter_fg;
            for &(row, s, e) in view.selection {
                let Some(cells) = rows.get_mut(row) else { continue };
                edit_row_chars(cells, view.gutter_chars, |col, ch, color| {
                    if (s..e).contains(&col) && ch == ' ' {
                        ('·', dot)
                    } else {
                        (ch, color)
                    }
                });
            }
        }
        *spans = join_rows(rows);

        // The overview ruler.
        if polish.overview_ruler != Some(false) {
            out.ruler = self.ruler_for(view, &problem_lines);
        }
        out
    }

    /// Every bracket in `buffer` with its nesting depth, by char index --
    /// from the syntax tree, so a bracket in a string or a comment isn't
    /// one; by the text in a file of no language. Worked out again only
    /// when the buffer (or its tree) changes, and not at all for a very
    /// large file.
    fn bracket_depths(&mut self, buffer: BufferId) -> std::sync::Arc<Vec<(usize, usize)>> {
        let Some(ob) = self.buffers.get(buffer) else { return Default::default() };
        let at = (ob.buffer.edit_count(), ob.syntax.as_ref().map(|s| s.revision()));
        if let Some((seen, depths)) = self.motion.brackets.get(&buffer) {
            if *seen == at {
                return depths.clone();
            }
        }
        let depths = std::sync::Arc::new(if ob.buffer.len_chars() > 2_000_000 {
            Vec::new()
        } else {
            match ob.syntax.as_ref().and_then(|s| s.bracket_depths()) {
                Some(by_byte) => by_byte.into_iter().map(|(byte, depth)| (ob.buffer.byte_to_char(byte), depth)).collect(),
                None => text_bracket_depths(ob.buffer.rope().chars()),
            }
        });
        self.motion.brackets.insert(buffer, (at, depths.clone()));
        depths
    }

    fn ruler_for(&mut self, view: &PaneView, problems: &[(usize, Severity)]) -> Option<Ruler> {
        let ob = self.buffers.get(view.buffer)?;
        let lines = ob.buffer.line_count().max(1);
        let mut marks: Vec<(usize, RulerMark)> = Vec::new();
        for &(line, sev) in problems {
            marks.push((line, match sev {
                Severity::Error => RulerMark::Error,
                Severity::Warning => RulerMark::Warning,
                Severity::Info => RulerMark::Info,
            }));
        }
        if let Some(hunks) = self.gutter_hunks.get(&view.buffer) {
            for mark in hunks {
                let kind = match mark.kind {
                    GutterMarkKind::Added => RulerMark::Added,
                    GutterMarkKind::Modified => RulerMark::Modified,
                    GutterMarkKind::Deleted => RulerMark::Deleted,
                };
                marks.push((mark.line.saturating_sub(1), kind));
            }
        }
        // Search matches across the whole file, kept until the file or
        // the search changes.
        if let Some(pattern) = self.vim.hlsearch_active().then(|| self.vim.last_search_pattern()).flatten().map(str::to_string) {
            let edits = ob.buffer.edit_count();
            let fresh = self.motion.ruler_search.get(&view.buffer).is_some_and(|(e, p, _)| *e == edits && *p == pattern);
            if !fresh {
                let found = if ob.buffer.len_chars() <= 5_000_000 {
                    let end = ob.buffer.char_to_byte(ob.buffer.len_chars());
                    let mut lines: Vec<usize> =
                        self.vim.hlsearch_matches(&ob.buffer, 0..end).iter().map(|m| ob.buffer.rope().byte_to_line(m.start)).collect();
                    lines.dedup();
                    lines
                } else {
                    Vec::new()
                };
                self.motion.ruler_search.insert(view.buffer, (edits, pattern, found));
            }
            if let Some((_, _, found)) = self.motion.ruler_search.get(&view.buffer) {
                marks.extend(found.iter().map(|&l| (l, RulerMark::Search)));
            }
        }
        // Extra cursors, by line.
        let mut caret_lines: Vec<usize> = view.carets.iter().map(|&at| ob.buffer.rope().char_to_line(at.min(ob.buffer.len_chars()))).collect();
        caret_lines.dedup();
        marks.extend(caret_lines.into_iter().map(|l| (l, RulerMark::Caret)));
        // Where a click on the ruler snaps to: every mark's line.
        let mut targets: Vec<usize> = marks.iter().map(|&(l, _)| l).collect();
        targets.sort_unstable();
        targets.dedup();
        self.motion.ruler_targets.insert(view.pane, targets);
        let visible = if view.wrapped { view.map.text_lines().count() as f32 } else { view.rows.saturating_sub(1) as f32 };
        Some(Ruler { lines, first: view.rendered_scroll, visible, cursor: view.cursor_line, marks })
    }

    /// How pane tabs are drawn, or `None` for no tab strip:
    /// `appearance.tabs`, which by default leaves it to the theme.
    pub(super) fn tab_style(&self) -> Option<crate::theme::TabStyle> {
        use crate::theme::TabStyle;
        match self.config.polish.tabs.as_deref() {
            Some("off") => None,
            Some("block") => Some(TabStyle::Block),
            Some("underline") => Some(TabStyle::Underline),
            _ => Some(self.theme.tabs),
        }
    }

    /// A click on a code pane's overview ruler jumps to the matching
    /// part of the file; says whether the click was on the ruler.
    pub(super) fn ruler_click(&mut self, pane: fenix_window::WindowId, rect: fenix_window::Rect, pos: (f32, f32)) -> bool {
        if self.config.polish.overview_ruler == Some(false) || pos.0 < rect.x + rect.w - 10.0 {
            return false;
        }
        let Some(&buffer) = self.windows().content(pane) else { return false };
        if !self.buffers.get(buffer).is_some_and(|ob| ob.kind == BufferKind::Text) {
            return false;
        }
        let line_height = self.text.as_ref().map(|t| t.line_height()).unwrap_or(text::LINE_HEIGHT);
        let content = pane_content_rect(rect, line_height, self.pane_has_breadcrumb(pane));
        if pos.1 < content.y || content.h <= 0.0 {
            return false;
        }
        let Some(ob) = self.buffers.get(buffer) else { return false };
        let lines = ob.buffer.line_count().max(1);
        let line = (((pos.1 - content.y) / content.h) * lines as f32).floor().clamp(0.0, (lines - 1) as f32) as usize;
        // On a mark (a few pixels from where it's drawn): its line,
        // exactly; anywhere else, the line the click fell on.
        let y_of = |line: usize| content.y + (line as f32 / lines as f32) * content.h;
        let line = self.motion.ruler_targets.get(&pane).and_then(|t| ruler_snap(t, pos.1, y_of, RULER_SNAP_PX)).unwrap_or(line);
        // A click there is a click: back to one cursor.
        self.carets.remove(&pane);
        let char_idx = ob.buffer.line_start_char(line);
        let from = JumpEntry { buffer, char_idx: self.pane_state(pane).cursor.char_idx };
        self.pane_state_mut(pane).cursor = Cursor { char_idx, sticky_col: 0 };
        self.record_jump(from);
        let prefs = self.motion();
        if prefs.beacon {
            self.start_beacon(buffer, line, Instant::now(), prefs.beacon_d);
        }
        true
    }

    /// `SPC t d`: problems in the text for all, errors only, or none.
    pub(crate) fn cycle_inline_diagnostics(&mut self) {
        let next = match self.config.polish.diagnostics_inline.as_deref().unwrap_or("all") {
            "all" => "errors",
            "errors" => "off",
            _ => "all",
        };
        self.config.polish.diagnostics_inline = Some(next.to_string());
        if let Err(err) = self.config.save() {
            self.set_error(format!("couldn't save settings.toml: {err}"));
        } else {
            self.set_message(match next {
                "all" => "Inline problems: all",
                "errors" => "Inline problems: errors only",
                _ => "Inline problems off",
            });
        }
    }

    /// The lines `sticky_rows` shows, outermost first -- what a click on
    /// one of its rows goes to.
    pub(super) fn sticky_headers(&self, buffer: BufferId, base_line: usize, cursor_row: Option<usize>) -> Vec<usize> {
        let polish = &self.config.polish;
        if polish.sticky_scroll == Some(false) || base_line == 0 {
            return Vec::new();
        }
        let max = polish.sticky_lines.unwrap_or(3).clamp(1, 6);
        let Some(ob) = self.buffers.get(buffer) else { return Vec::new() };
        let Some(syntax) = ob.syntax.as_ref() else { return Vec::new() };
        let lines = ob.buffer.line_count();
        let mut headers: Vec<usize> = Vec::new();
        for _ in 0..=max {
            let top = base_line + headers.len();
            if top >= lines {
                break;
            }
            let byte = ob.buffer.char_to_byte(ob.buffer.line_start_char(top));
            let enclosing: Vec<usize> = syntax.enclosing_scopes(byte).into_iter().filter(|&(start, end)| start < top && end > top).map(|(start, _)| start).collect();
            let want: Vec<usize> = enclosing.iter().rev().take(max).rev().copied().collect();
            if want.len() <= headers.len() {
                break;
            }
            headers = want;
        }
        if headers.is_empty() || cursor_row.is_some_and(|row| row < headers.len()) {
            return Vec::new();
        }
        headers
    }

    /// Sticky scroll: the headers of the scopes the pane's top line is
    /// inside, as one row of spans each, outermost first -- at most
    /// `appearance.sticky_lines`, the innermost kept when there are more.
    /// Empty when the cursor would be under them, so it's never hidden.
    pub(super) fn sticky_rows(&mut self, buffer: BufferId, base_line: usize, cursor_row: Option<usize>, gutter_chars: usize, tab_stops: &TabStops, cursor_line: usize) -> Vec<RowSpans> {
        let headers = self.sticky_headers(buffer, base_line, cursor_row);
        let mut out = Vec::with_capacity(headers.len());
        for line in headers {
            let highlights = self.syntax_highlights_for_visible_range(buffer, line, 1);
            let Some(ob) = self.buffers.get(buffer) else { break };
            let row = self.content_spans(ob, line, 1, gutter_chars, None, &highlights, cursor_line, tab_stops, 0);
            out.push(row.into_iter().map(|(s, c)| (s, c, false)).collect());
        }
        out
    }
}

/// `spans` cut into screen rows at their `"\n"` spans.
pub(super) fn split_rows(spans: RowSpans) -> Vec<RowSpans> {
    let mut rows = vec![Vec::new()];
    for span in spans {
        if span.0 == "\n" {
            rows.push(Vec::new());
        } else {
            rows.last_mut().expect("never empty").push(span);
        }
    }
    rows
}

pub(super) fn join_rows(rows: Vec<RowSpans>) -> RowSpans {
    let mut spans = Vec::new();
    let count = rows.len();
    for (i, row) in rows.into_iter().enumerate() {
        spans.extend(row);
        if i + 1 < count {
            spans.push(("\n".to_string(), glyphon::Color::rgb(0, 0, 0), false));
        }
    }
    spans
}

/// Rewrites one row's characters from `from` on: `f` gets each one's
/// column (counted from `from`), the character and its colour, and says
/// what to draw instead. Icon spans are left alone.
pub(super) fn edit_row_chars(row: &mut RowSpans, from: usize, mut f: impl FnMut(usize, char, glyphon::Color) -> (char, glyphon::Color)) {
    let mut out: RowSpans = Vec::with_capacity(row.len());
    let mut index = 0usize;
    for (text, color, icon) in row.drain(..) {
        if icon {
            index += text.chars().count();
            out.push((text, color, icon));
            continue;
        }
        for ch in text.chars() {
            let (ch, color) = if index >= from { f(index - from, ch, color) } else { (ch, color) };
            index += 1;
            match out.last_mut() {
                Some((t, c, false)) if *c == color => t.push(ch),
                _ => out.push((ch.to_string(), color, false)),
            }
        }
    }
    *row = out;
}

/// Each row of a selection's corner radii -- top-left, top-right,
/// bottom-right, bottom-left -- rounded only where the rows above and
/// below don't carry the selection on past that corner.
pub(super) fn selection_corners(segments: &Segments, radius: f32) -> Vec<[f32; 4]> {
    let find = |row: usize| segments.iter().find(|(r, _, _)| *r == row).map(|&(_, s, e)| (s, e));
    segments
        .iter()
        .map(|&(row, s, e)| {
            let above = row.checked_sub(1).and_then(find);
            let below = find(row + 1);
            let tl = above.is_none_or(|(ps, _)| ps > s);
            let tr = above.is_none_or(|(_, pe)| pe < e);
            let br = below.is_none_or(|(_, ne)| ne < e);
            let bl = below.is_none_or(|(ns, _)| ns > s);
            [tl, tr, br, bl].map(|round| if round { radius } else { 0.0 })
        })
        .collect()
}

/// How near a mark, in pixels, a click on the ruler goes to it.
const RULER_SNAP_PX: f32 = 3.0;

/// The mark whose line is drawn nearest the click at `y`, if one is
/// within `within` pixels of it; `y_of` says where a line is drawn.
fn ruler_snap(targets: &[usize], y: f32, y_of: impl Fn(usize) -> f32, within: f32) -> Option<usize> {
    targets.iter().copied().map(|t| (t, (y_of(t) - y).abs())).filter(|(_, d)| *d <= within).min_by(|a, b| a.1.total_cmp(&b.1)).map(|(t, _)| t)
}

/// Brackets by depth from the text alone, for a file of no language:
/// `(char index, depth)`.
fn text_bracket_depths(chars: impl Iterator<Item = char>) -> Vec<(usize, usize)> {
    let mut depths = Vec::new();
    let mut depth: usize = 0;
    for (i, ch) in chars.enumerate() {
        match ch {
            '(' | '[' | '{' => {
                depths.push((i, depth));
                depth += 1;
            }
            ')' | ']' | '}' => {
                depth = depth.saturating_sub(1);
                depths.push((i, depth));
            }
            _ => {}
        }
    }
    depths
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brackets_go_by_the_syntax_tree_and_by_the_text_with_no_language() {
        let path = temp_file("a.rs", "fn f() { let s = \"(\"; }\n");
        let mut app = App::with_file(Some(path.to_string_lossy().into_owned()));
        let id = app.focused_buffer_id();
        assert!(app.open().syntax.is_some(), "a Rust file has a tree");
        {
            let depths = app.bracket_depths(id);
            let text = app.open().buffer.text();
            let found: Vec<char> = depths.iter().map(|&(at, _)| text.chars().nth(at).unwrap()).collect();
            assert_eq!(found, ['(', ')', '{', '}'], "the string's ( isn't one");
            assert_eq!(depths.iter().map(|&(_, d)| d).collect::<Vec<_>>(), [0, 0, 0, 0]);
            // A frame drawn between: the tree is looked at again, and
            // nothing having changed, the depths are kept.
            app.syntax_highlights_for_visible_range(id, 0, 10);
            assert!(std::sync::Arc::ptr_eq(&depths, &app.bracket_depths(id)), "kept across frames until it changes");
        }
        assert_eq!(text_bracket_depths("a(b[c]) \"(\"".chars()), [(1, 0), (3, 1), (5, 1), (6, 0), (9, 0)], "plain text: every one");
    }

    fn span(t: &str) -> (String, glyphon::Color, bool) {
        (t.to_string(), glyphon::Color::rgb(1, 2, 3), false)
    }

    fn temp_file(name: &str, contents: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!("fenix-polish-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, contents).unwrap();
        path
    }

    fn polish(app: &mut App, spans: &mut RowSpans) -> PanePolish {
        let buffer = app.focused_buffer_id();
        let lines: Vec<usize> = (0..app.open().buffer.line_count()).collect();
        let map = super::super::rows::RowMap::lines(&lines, 0);
        let tab_lines = HashSet::new();
        let selection = Segments::new();
        let vcol = |_line: usize, col: usize| col;
        let carets: Vec<usize> = app.extra_carets().iter().map(|c| c.at).collect();
        let pane = app.focused_pane_id();
        let view = PaneView {
            buffer,
            map: &map,
            rows: lines.len(),
            wrapped: false,
            gutter_chars: 0,
            scroll_col: 0,
            rendered_scroll: 0.0,
            cursor_line: 1,
            tab_lines: &tab_lines,
            selection: &selection,
            vcol: &vcol,
            carets: &carets,
            pane,
        };
        app.polish_pane(&view, spans, Instant::now())
    }

    fn with_problem(text: &str, line: u32, start: u32, end: u32) -> App {
        let path = temp_file("bugs.py", text);
        let mut app = App::with_file(Some(path.display().to_string()));
        let key = fenix_lsp::normalize(std::fs::canonicalize(&path).unwrap());
        let range = lsp_types::Range { start: lsp_types::Position { line, character: start }, end: lsp_types::Position { line, character: end } };
        let diag = lsp_types::Diagnostic { range, message: "name is not defined".into(), severity: Some(lsp_types::DiagnosticSeverity::ERROR), ..Default::default() };
        let server = app.lsp_key(fenix_syntax::LanguageId::Python);
        app.diagnostics.publish(key, server, "pyright", vec![diag]);
        app
    }

    fn row_spans(app: &App) -> RowSpans {
        let lines: Vec<String> = (0..app.open().buffer.line_count())
            .map(|l| {
                let b = &app.open().buffer;
                let s = b.line_start_char(l);
                b.text_range(s, s + b.line_len(l))
            })
            .collect();
        join_rows(lines.into_iter().map(|t| vec![(t, glyphon::Color::rgb(9, 9, 9), false)]).collect())
    }

    #[test]
    fn a_problem_is_underlined_and_its_message_ends_the_cursors_line() {
        let mut app = with_problem("x = 1
y = zed
", 1, 4, 7);
        let mut spans = row_spans(&app);
        let out = polish(&mut app, &mut spans);
        assert_eq!(out.squiggles.len(), 1);
        assert_eq!((out.squiggles[0].0, out.squiggles[0].1, out.squiggles[0].2), (1, 4, 7));
        let text: String = spans.iter().map(|s| s.0.as_str()).collect();
        assert!(text.contains("y = zed    ■ name is not defined"), "{text}");
    }

    #[test]
    fn in_a_wrapped_pane_a_problem_is_underlined_on_each_row_and_its_message_ends_the_last() {
        let mut app = with_problem("x = 1\ny = zed + other_name_here\n", 1, 4, 25);
        app.line_number_mode = LineNumberMode::Off;
        app.run_cmd("view.toggle_wrap");
        let (pane, buffer) = (app.focused_pane_id(), app.focused_buffer_id());
        app.set_pane_text_cols(pane, 12);
        app.press("j");
        let layout = app.focused_layout(6);
        let ob = app.open();
        let mut spans: RowSpans = app
            .row_content_spans(ob, &layout.map, 6, 0, &[], 1, &app.tab_stops_for(buffer), &HashSet::new(), &layout.decorated)
            .into_iter()
            .map(|(t, c)| (t, c, false))
            .collect();
        let tab_lines = HashSet::new();
        let selection = Segments::new();
        let vcol = |line: usize, col: usize| layout.vcol(line, col);
        let view = PaneView { buffer, map: &layout.map, rows: 6, wrapped: true, gutter_chars: 0, scroll_col: 0, rendered_scroll: 0.0, cursor_line: 1, tab_lines: &tab_lines, selection: &selection, vcol: &vcol, carets: &[], pane };
        let out = app.polish_pane(&view, &mut spans, Instant::now());
        // `y = zed + ` / `other_name_h` / `ere`.
        let squiggles: Vec<(usize, usize, usize)> = out.squiggles.iter().map(|&(r, s, e, _)| (r, s, e)).collect();
        assert_eq!(squiggles, vec![(1, 4, 10), (2, 0, 12), (3, 0, 3)]);
        let rows: Vec<String> = spans.iter().map(|s| s.0.as_str()).collect::<String>().split('\n').map(str::to_string).collect();
        assert_eq!(rows[3], "ere    ■ name is not defined", "{rows:?}");
        assert_eq!(rows[1], "y = zed + ");
    }

    #[test]
    fn inline_problems_can_be_turned_off() {
        let mut app = with_problem("x = 1
y = zed
", 1, 4, 7);
        app.config.polish.diagnostics_inline = Some("off".into());
        app.config.polish.diagnostics_message = Some("off".into());
        let mut spans = row_spans(&app);
        let out = polish(&mut app, &mut spans);
        assert!(out.squiggles.is_empty());
        assert!(!spans.iter().any(|s| s.0.contains("not defined")));
    }

    #[test]
    fn the_ruler_marks_extra_cursors_and_a_click_near_a_mark_goes_to_it() {
        let mut app = with_problem("a\nb\nc\nd = zed\n", 3, 4, 7);
        app.config.set_value("editor.keys", Some(fenix_config::Value::Text("standard".into())));
        app.sync_key_policy();
        app.toggle_caret_at(4);
        let mut spans = row_spans(&app);
        let ruler = polish(&mut app, &mut spans).ruler.expect("on");
        assert!(ruler.marks.contains(&(2, RulerMark::Caret)), "{:?}", ruler.marks);
        assert!(ruler.marks.contains(&(3, RulerMark::Error)));
        let pane = app.focused_pane_id();
        assert_eq!(app.motion.ruler_targets.get(&pane).cloned(), Some(vec![2, 3]));
        let y = |line: usize| line as f32 * 100.0;
        assert_eq!(ruler_snap(&[2, 3, 40], 302.0, y, RULER_SNAP_PX), Some(3), "the nearest within reach");
        assert_eq!(ruler_snap(&[2, 3, 40], 250.0, y, RULER_SNAP_PX), None, "a line away but 50 px: where it was clicked");
        assert_eq!(ruler_snap(&[], 0.0, y, RULER_SNAP_PX), None);
        // Through the click itself, on a tall pane: a click far from any
        // mark goes where it fell, and back to one cursor.
        let rect = fenix_window::Rect { x: 0.0, y: 0.0, w: 800.0, h: 600.0 };
        assert!(app.ruler_click(pane, rect, (795.0, 590.0)));
        assert_eq!(app.open().buffer.line_col(&app.cursor()).0, 4, "the last line, where it fell");
        assert!(app.extra_carets().is_empty(), "a click is a click");
    }

    #[test]
    fn the_ruler_marks_problems_and_the_cursor() {
        let mut app = with_problem("a
b
c
d = zed
", 3, 4, 7);
        let mut spans = row_spans(&app);
        let ruler = polish(&mut app, &mut spans).ruler.expect("on by default");
        assert!(ruler.marks.contains(&(3, RulerMark::Error)));
        assert_eq!(ruler.cursor, 1);
        app.config.polish.overview_ruler = Some(false);
        assert!(polish(&mut app, &mut spans).ruler.is_none());
    }

    #[test]
    fn sticky_scroll_pins_the_scope_the_top_line_is_inside() {
        let mut body = String::from("def outer():
    x = 1
    def inner():
");
        for i in 0..30 {
            body.push_str(&format!("        y{i} = {i}
"));
        }
        let path = temp_file("nested.py", &body);
        let mut app = App::with_file(Some(path.display().to_string()));
        let id = app.focused_buffer_id();
        app.syntax_highlights_for_visible_range(id, 0, 1);
        let rows = app.sticky_rows(id, 10, None, 0, &TabStops::Fixed(8), 20);
        let text: Vec<String> = rows.iter().map(|r| r.iter().map(|s| s.0.as_str()).collect()).collect();
        assert_eq!(text, vec!["def outer():".to_string(), "    def inner():".to_string()]);
        // The cursor under them hides them rather than itself.
        assert!(app.sticky_rows(id, 10, Some(0), 0, &TabStops::Fixed(8), 10).is_empty());
        app.config.polish.sticky_scroll = Some(false);
        assert!(app.sticky_rows(id, 10, None, 0, &TabStops::Fixed(8), 20).is_empty());
    }

    #[test]
    fn every_theme_has_tabs_in_its_own_look_unless_turned_off() {
        use crate::theme::TabStyle;
        let mut app = App::with_file(None);
        app.new_scratch_buffer();
        app.theme = crate::theme::by_name("Nord").unwrap();
        assert_eq!(app.tab_style(), Some(TabStyle::Underline));
        assert!(app.pane_has_breadcrumb(app.focused_pane_id()), "text has a breadcrumb row");
        app.go_home();
        assert!(!app.pane_has_breadcrumb(app.focused_pane_id()), "a page starts under its tabs");
        app.new_scratch_buffer();
        app.theme = crate::theme::by_name("Visual Studio Dark").unwrap();
        assert_eq!(app.tab_style(), Some(TabStyle::Block));
        app.config.polish.tabs = Some("underline".into());
        assert_eq!(app.tab_style(), Some(TabStyle::Underline));
        app.config.polish.tabs = Some("off".into());
        assert_eq!(app.tab_style(), None);
        assert!(!app.pane_has_breadcrumb(app.focused_pane_id()));
    }

    #[test]
    fn rows_split_and_join_back_the_same() {
        let spans = vec![span(" 1 "), span("abc"), span("\n"), span(" 2 "), span("d")];
        let rows = split_rows(spans.clone());
        assert_eq!(rows.len(), 2);
        let joined = join_rows(rows);
        let text: String = joined.iter().map(|s| s.0.as_str()).collect();
        assert_eq!(text, " 1 abc\n 2 d");
    }

    #[test]
    fn a_rows_characters_are_rewritten_past_the_gutter_only() {
        let mut row = vec![span(" 1 "), span("a b")];
        edit_row_chars(&mut row, 3, |_, ch, color| if ch == ' ' { ('·', color) } else { (ch, color) });
        let text: String = row.iter().map(|s| s.0.as_str()).collect();
        assert_eq!(text, " 1 a·b");
    }

    #[test]
    fn a_selections_corners_round_where_nothing_continues_it() {
        // Rows 0..2: a line-wise-looking block whose middle row is widest.
        let segments = vec![(0, 4, 10), (1, 0, 12), (2, 0, 6)];
        let radii = selection_corners(&segments, 3.0);
        assert_eq!(radii[0], [3.0, 3.0, 0.0, 0.0], "top row: its top corners round");
        assert_eq!(radii[1], [3.0, 3.0, 3.0, 0.0], "wider than both neighbours on the right");
        assert_eq!(radii[2], [0.0, 0.0, 3.0, 3.0], "bottom row: its bottom corners round");
    }
}
