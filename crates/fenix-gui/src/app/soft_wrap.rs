//! Soft wrap: a pane whose long lines go on over more screen rows rather
//! than off its right edge. The agent's prompt and a compose pane always
//! wrap; any other buffer by `editor.soft_wrap` (off, prose, all) or
//! `SPC t w`. The text is never changed: a wrapped pane's display rows
//! (`pane_display_rows`) are segments of its lines, worked out from the
//! pane's width each frame (`crate::wrap::soft_wrap`), so resizing it
//! wraps again.
//!
//! What maps rows to positions and back -- the caret, the overlays,
//! scrolling, the mouse -- goes through the rows when there are some;
//! a pane that doesn't wrap has none, and is drawn as it always was.

use super::*;
use crate::wrap::{self, Row};
use std::rc::Rc;

/// Past this many chars, a buffer's rows are kept from frame to frame
/// rather than worked out again each time they're asked for.
const CACHE_FROM_CHARS: usize = 50_000;

/// The width a pane wraps at before it's first drawn, in cells.
const DEFAULT_WRAP_COLS: usize = 80;

/// What a large buffer's rows were made from: its edits, length, the
/// lines shown (folds), and its tabs.
type WrapKey = (u64, usize, Vec<usize>, TabStops);

/// Soft wrap's state, kept on `App`.
#[derive(Default)]
pub(super) struct SoftWrap {
    /// Each pane's text width in cells, as it was last drawn.
    pub(super) cols: HashMap<fenix_window::WindowId, usize>,
    /// `SPC t w`'s choice for a buffer, over what the setting says.
    toggled: HashMap<BufferId, bool>,
    /// A large buffer's rows at a width.
    cache: std::cell::RefCell<HashMap<(BufferId, usize), (WrapKey, Rc<Vec<Row>>)>>,
    /// The screen column `gj`/`gk` keep to while they're what moved the
    /// cursor: (buffer, where they left it, the column).
    want: Option<(BufferId, usize, usize)>,
}

/// Whether `ob` is prose for `editor.soft_wrap = "prose"`: Markdown, or
/// plain text -- a `.txt` file, a file with no extension (`README`), or
/// a buffer with no file.
fn is_prose(ob: &OpenBuffer) -> bool {
    if ob.syntax.as_ref().is_some_and(|s| s.language() == fenix_syntax::LanguageId::Markdown) {
        return true;
    }
    let Some(path) = ob.buffer.path() else { return ob.syntax.is_none() };
    match path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase) {
        None => ob.syntax.is_none(),
        Some(ext) => matches!(ext.as_str(), "md" | "markdown" | "mdx" | "txt" | "text"),
    }
}

/// A char column -> visual column map for `text`, its tabs expanded.
fn col_map_of(text: &str, tab_stops: &TabStops) -> Vec<usize> {
    tabstops::expand_line(text, tab_stops).1
}

impl App {
    /// Whether `buffer` is a box written in prose: the agent's prompt, or
    /// a compose pane's (a commit message, a PR description).
    pub(super) fn is_prose_box(&self, buffer: BufferId) -> bool {
        self.buffers.get(buffer).is_some_and(|ob| ob.kind == BufferKind::Prompt) || self.compose.as_ref().is_some_and(|c| c.buffer == buffer)
    }

    /// Whether `buffer`'s lines wrap: a prose box's always, unless `SPC t
    /// w` turned it off; a file's by `editor.soft_wrap`, or `SPC t w`.
    pub(super) fn soft_wraps(&self, buffer: BufferId) -> bool {
        let Some(ob) = self.buffers.get(buffer) else { return false };
        if !matches!(ob.kind, BufferKind::Text | BufferKind::Prompt) || self.table_views.contains_key(&buffer) {
            return false;
        }
        if let Some(&on) = self.soft_wrap.toggled.get(&buffer) {
            return on;
        }
        if self.is_prose_box(buffer) {
            return true;
        }
        match self.config.text_value("editor.soft_wrap").unwrap_or("off") {
            "all" => true,
            "prose" => is_prose(ob),
            _ => false,
        }
    }

    /// Whether `pane` wraps `buffer`'s lines -- not in a side-by-side,
    /// whose rows line up with the other side's.
    fn pane_soft_wraps(&self, pane: fenix_window::WindowId, buffer: BufferId) -> bool {
        self.soft_wraps(buffer) && self.side_by_side_at(pane).is_none()
    }

    /// The cells `pane` wraps at: its text width less one, kept for the
    /// caret past a line's end.
    fn pane_wrap_cols(&self, pane: fenix_window::WindowId) -> usize {
        self.soft_wrap.cols.get(&pane).copied().unwrap_or(DEFAULT_WRAP_COLS).saturating_sub(1).max(1)
    }

    /// Notes how many cells across `pane`'s text is, for its wrap.
    pub(super) fn note_wrap_cols(&mut self, pane: fenix_window::WindowId, cols: usize) {
        self.soft_wrap.cols.insert(pane, cols);
    }

    /// `pane`'s display rows when it wraps `buffer`, top to bottom: each
    /// shown line (folds applied) broken where it wraps. A closed fold's
    /// header wraps like any line, its `⋯` after its last row. `None`
    /// when the pane doesn't wrap: each row is then one line.
    pub(super) fn pane_display_rows(&self, pane: fenix_window::WindowId, buffer: BufferId) -> Option<Rc<Vec<Row>>> {
        if !self.pane_soft_wraps(pane, buffer) {
            return None;
        }
        let ob = self.buffers.get(buffer)?;
        let lines = self.folded_display_lines(buffer);
        let width = self.pane_wrap_cols(pane);
        let tab_stops = self.tab_stops_for(buffer);
        let cached = ob.buffer.len_chars() > CACHE_FROM_CHARS;
        let key = (ob.buffer.edit_count(), ob.buffer.len_chars(), lines, tab_stops);
        if cached {
            if let Some((at, rows)) = self.soft_wrap.cache.borrow().get(&(buffer, width)) {
                if *at == key {
                    return Some(rows.clone());
                }
            }
        }
        let visual_lines = ob.buffer.visual_line_count();
        let mut rows = Vec::with_capacity(key.2.len());
        for &line in &key.2 {
            if line >= visual_lines {
                rows.push(Row::whole(line, 0));
                continue;
            }
            let start = ob.buffer.line_start_char(line);
            let text = ob.buffer.text_range(start, start + ob.buffer.line_len(line));
            let segments = wrap::soft_wrap(&text, width, &key.3);
            let count = segments.len();
            rows.extend(segments.into_iter().enumerate().map(|(i, r)| Row { line, start: r.start, end: r.end, last: i + 1 == count }));
        }
        let rows = Rc::new(rows);
        if cached {
            self.soft_wrap.cache.borrow_mut().insert((buffer, width), (key, rows.clone()));
        }
        Some(rows)
    }

    /// The display row of `pane` that char `char_idx` of `buffer` is on.
    pub(super) fn display_row_of(&self, pane: fenix_window::WindowId, buffer: BufferId, char_idx: usize) -> usize {
        let Some(ob) = self.buffers.get(buffer) else { return 0 };
        let (line, col) = ob.buffer.line_col(&Cursor { char_idx, sticky_col: 0 });
        match self.pane_display_rows(pane, buffer) {
            Some(rows) => wrap::row_holding(&rows, line, col).unwrap_or(line),
            None => self.pane_display_lines(pane, buffer).iter().position(|&l| l == line).unwrap_or(line),
        }
    }

    /// The visual column map of `buffer`'s line `line`, tabs expanded.
    fn line_col_map(&self, buffer: BufferId, line: usize) -> Vec<usize> {
        let Some(ob) = self.buffers.get(buffer) else { return vec![0] };
        if line >= ob.buffer.line_count() {
            return vec![0];
        }
        let start = ob.buffer.line_start_char(line);
        let text = ob.buffer.text_range(start, start + ob.buffer.line_len(line));
        col_map_of(&text, &self.tab_stops_for(buffer))
    }

    /// Whether `j`/`k` go by screen row in the focused pane: a prose box
    /// that wraps.
    pub(super) fn jk_by_display_row(&self) -> bool {
        let buffer = self.focused_buffer_id();
        self.is_prose_box(buffer) && self.pane_soft_wraps(self.focused_pane_id(), buffer)
    }

    /// `gj`/`gk` (and `j`/`k` in a prose box): the cursor `count` screen
    /// rows down or up, at the same screen column -- kept through a
    /// short row, as Vim keeps it. In a pane that doesn't wrap, a row is
    /// a line.
    pub(crate) fn move_display_rows(&mut self, down: bool, count: u32) {
        let (pane, buffer) = (self.focused_pane_id(), self.focused_buffer_id());
        let Some(ob) = self.buffers.get(buffer) else { return };
        let rows: Vec<Row> = match self.pane_display_rows(pane, buffer) {
            Some(rows) => rows.to_vec(),
            None => self
                .pane_display_lines(pane, buffer)
                .into_iter()
                .filter(|&l| l != side_by_side::FILLER && l < ob.buffer.line_count())
                .map(|l| Row::whole(l, ob.buffer.line_len(l)))
                .collect(),
        };
        let cursor = self.pane_state(pane).cursor;
        let (line, col) = ob.buffer.line_col(&cursor);
        let Some(here) = wrap::row_holding(&rows, line, col) else { return };
        let map = self.line_col_map(buffer, line);
        let at = |map: &[usize], col: usize| map.get(col).copied().unwrap_or_else(|| map.last().copied().unwrap_or(0) + col.saturating_sub(map.len() - 1));
        let column = at(&map, col).saturating_sub(at(&map, rows[here].start));
        let want = match self.soft_wrap.want {
            Some((b, idx, want)) if b == buffer && idx == cursor.char_idx => want,
            _ => column,
        };
        let count = count.max(1) as usize;
        let target = if down { (here + count).min(rows.len().saturating_sub(1)) } else { here.saturating_sub(count) };
        if target == here {
            return;
        }
        let row = rows[target];
        let map = self.line_col_map(buffer, row.line);
        let (c, _) = crate::mouse::char_col_at(&map, at(&map, row.start) + want);
        let Some(ob) = self.buffers.get(buffer) else { return };
        let len = ob.buffer.line_len(row.line);
        let last = if row.last { len.saturating_sub(1) } else { row.end.saturating_sub(1) };
        let c = c.min(last).max(row.start.min(last));
        let char_idx = ob.buffer.line_start_char(row.line) + c;
        let state = self.pane_state_mut(pane);
        state.cursor = Cursor { char_idx, sticky_col: c };
        self.soft_wrap.want = Some((buffer, char_idx, want));
        self.wake_caret();
    }

    /// `SPC t w`: the focused buffer's lines wrapped, or not.
    pub(crate) fn toggle_soft_wrap(&mut self) {
        let buffer = self.focused_buffer_id();
        let wrappable = self.buffers.get(buffer).is_some_and(|ob| matches!(ob.kind, BufferKind::Text | BufferKind::Prompt)) && !self.table_views.contains_key(&buffer);
        if !wrappable {
            self.set_message("Nothing here wraps");
            return;
        }
        let on = !self.soft_wraps(buffer);
        self.soft_wrap.toggled.insert(buffer, on);
        let pane = self.focused_pane_id();
        self.pane_state_mut(pane).scroll_col = 0;
        self.set_message(if on { "Soft wrap on" } else { "Soft wrap off" });
        self.wake_caret();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LONG: &str = "Please explain how the build pipeline decides which crates to rebuild after a change, and what it caches between runs.";

    /// An App with the agent's prompt focused in a pane `cols` wide,
    /// holding `text`, in Insert mode with the cursor at its end.
    fn prompt_app(text: &str, cols: usize) -> (App, BufferId, fenix_window::WindowId) {
        let mut app = App::with_file(None);
        let id = app.buffers.open_prompt();
        let pane = app.focused_pane_id();
        app.set_pane_content(pane, id);
        app.note_wrap_cols(pane, cols);
        app.test_route_key(KeyPress::char('i'));
        for c in text.chars() {
            app.test_route_key(KeyPress::char(c));
        }
        (app, id, pane)
    }

    /// An App on an ordinary text buffer.
    fn text_app() -> App {
        let mut app = App::with_file(None);
        let (pane, id) = (app.focused_pane_id(), app.buffers.open_scratch());
        app.set_pane_content(pane, id);
        app
    }

    fn rows_text(app: &App, pane: fenix_window::WindowId, id: BufferId) -> Vec<String> {
        let ob = app.buffers.get(id).unwrap();
        app.pane_display_rows(pane, id)
            .unwrap()
            .iter()
            .map(|r| {
                let start = ob.buffer.line_start_char(r.line);
                ob.buffer.text_range(start + r.start, start + r.end)
            })
            .collect()
    }

    #[test]
    fn a_long_prompt_wraps_into_several_rows_at_word_boundaries() {
        let (app, id, pane) = prompt_app(LONG, 31);
        let rows = rows_text(&app, pane, id);
        assert!(rows.len() >= 4, "{rows:?}");
        assert!(rows.iter().all(|r| r.chars().count() <= 30), "{rows:?}");
        assert_eq!(rows.concat(), LONG, "nothing is lost");
        assert!(rows[..rows.len() - 1].iter().all(|r| r.ends_with(' ')), "breaks after whitespace: {rows:?}");
        assert_eq!(app.pane_display_lines(pane, id), vec![0; rows.len()], "every row is the one line");
    }

    #[test]
    fn each_row_draws_its_stretch_of_the_line_with_an_arrow_for_a_number() {
        let (mut app, id, pane) = prompt_app("one two three four five six", 11);
        app.line_number_mode = LineNumberMode::Absolute;
        let rows = app.pane_display_rows(pane, id).unwrap();
        let lines: Vec<usize> = rows.iter().map(|r| r.line).collect();
        let ob = app.buffers.get(id).unwrap();
        let gutter = app.gutter_chars(ob);
        let spans = app.folded_content_spans(ob, &lines, rows.len(), gutter, &[], 0, &TabStops::Fixed(4), 0, &HashSet::new(), &HashMap::new(), Some(&rows));
        let text: String = spans.iter().map(|(s, _)| s.as_str()).collect();
        assert_eq!(text, "1 one two \n↪ three \n↪ four five \n↪ six", "ten cells a row, one kept for the caret");
    }

    #[test]
    fn the_caret_at_the_end_is_on_the_last_row_and_typing_keeps_it_in_view() {
        let (mut app, id, pane) = prompt_app(LONG, 31);
        let rows = app.pane_display_rows(pane, id).unwrap().len();
        let end = app.cursor().char_idx;
        assert_eq!(app.display_row_of(pane, id, end), rows - 1);
        // A box three rows high: the caret's row is scrolled to.
        app.ensure_cursor_visible(3);
        let scroll = app.pane_state(pane).scroll_line;
        assert!(scroll < rows && rows - 1 < scroll + 3, "row {} with scroll {scroll}", rows - 1);
        for c in " And how do I clear it?".chars() {
            app.test_route_key(KeyPress::char(c));
            app.ensure_cursor_visible(3);
            let row = app.display_row_of(pane, id, app.cursor().char_idx);
            let scroll = app.pane_state(pane).scroll_line;
            assert!(scroll <= row && row < scroll + 3, "typing {c:?}: row {row}, scroll {scroll}");
        }
        assert_eq!(app.display_row_of(pane, id, app.cursor().char_idx), app.pane_display_rows(pane, id).unwrap().len() - 1);
    }

    #[test]
    fn a_narrower_pane_wraps_again_into_more_rows() {
        let (mut app, id, pane) = prompt_app(LONG, 61);
        let wide = app.pane_display_rows(pane, id).unwrap().len();
        app.note_wrap_cols(pane, 21);
        let narrow = app.pane_display_rows(pane, id).unwrap().len();
        assert!(narrow > wide, "{wide} rows at 60, {narrow} at 20");
    }

    #[test]
    fn j_and_k_in_the_prompt_move_by_screen_row() {
        let (mut app, id, pane) = prompt_app(LONG, 31);
        app.test_route_key(KeyPress::named(FenixNamedKey::Escape));
        app.test_route_key(KeyPress::char('0'));
        assert_eq!(app.display_row_of(pane, id, app.cursor().char_idx), 0);
        app.test_route_key(KeyPress::char('l'));
        app.test_route_key(KeyPress::char('l'));
        app.test_route_key(KeyPress::char('j'));
        let rows = app.pane_display_rows(pane, id).unwrap();
        assert_eq!(app.display_row_of(pane, id, app.cursor().char_idx), 1, "j: the next row, not the next line");
        assert_eq!(app.cursor().char_idx, rows[1].start + 2, "at the same screen column");
        app.test_route_key(KeyPress::char('2'));
        app.test_route_key(KeyPress::char('j'));
        assert_eq!(app.display_row_of(pane, id, app.cursor().char_idx), 3);
        app.test_route_key(KeyPress::char('k'));
        assert_eq!(app.display_row_of(pane, id, app.cursor().char_idx), 2);
        assert_eq!(app.cursor().char_idx, rows[2].start + 2);
    }

    #[test]
    fn a_compose_box_wraps_and_its_j_moves_by_screen_row() {
        let mut app = text_app();
        app.open_compose_seeded(ComposePurpose::CommitMessage { repo_root: PathBuf::from("/nowhere") }, LONG);
        let (pane, id) = (app.focused_pane_id(), app.focused_buffer_id());
        app.note_wrap_cols(pane, 31);
        assert!(app.pane_display_rows(pane, id).unwrap().len() > 3);
        app.test_set_cursor(Cursor::at_start());
        app.test_route_key(KeyPress::char('j'));
        assert_eq!(app.display_row_of(pane, id, app.cursor().char_idx), 1);
        assert_eq!(app.open().buffer.line_col(&app.cursor()).0, 0, "still the first line");
    }

    #[test]
    fn gj_moves_by_screen_row_in_any_wrapped_buffer_and_j_by_line() {
        let mut app = text_app();
        app.config.set_value("editor.soft_wrap", Some(fenix_config::Value::Text("all".into())));
        let pane = app.focused_pane_id();
        app.note_wrap_cols(pane, 21);
        app.test_insert_str(&format!("{LONG}\nshort\n"));
        app.test_set_cursor(Cursor::at_start());
        let id = app.focused_buffer_id();
        assert!(app.pane_display_rows(pane, id).unwrap().len() > 3);
        app.test_route_key(KeyPress::char('g'));
        app.test_route_key(KeyPress::char('j'));
        assert_eq!((app.open().buffer.line_col(&app.cursor()).0, app.display_row_of(pane, id, app.cursor().char_idx)), (0, 1), "gj: down a row, the same line");
        app.test_route_key(KeyPress::char('j'));
        assert_eq!(app.open().buffer.line_col(&app.cursor()).0, 1, "j: the next line");
    }

    #[test]
    fn code_is_unchanged_with_the_setting_off() {
        let mut app = text_app();
        let pane = app.focused_pane_id();
        app.note_wrap_cols(pane, 11);
        app.test_insert_str(&format!("{LONG}\nfn main() {{}}\n"));
        let id = app.focused_buffer_id();
        assert!(app.pane_display_rows(pane, id).is_none());
        assert_eq!(app.pane_display_lines(pane, id), vec![0, 1, 2]);
        app.config.set_value("editor.soft_wrap", Some(fenix_config::Value::Text("prose".into())));
        let ob = app.buffers.get_mut(id).unwrap();
        ob.syntax = Some(fenix_syntax::SyntaxState::new(fenix_syntax::LanguageId::Rust, &ob.buffer.text()));
        assert!(app.pane_display_rows(pane, id).is_none(), "prose: not code");
        app.config.set_value("editor.soft_wrap", Some(fenix_config::Value::Text("all".into())));
        assert!(app.pane_display_rows(pane, id).is_some());
    }

    #[test]
    fn prose_wraps_markdown_and_spc_t_w_toggles_a_buffer() {
        let mut app = text_app();
        app.config.set_value("editor.soft_wrap", Some(fenix_config::Value::Text("prose".into())));
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notes.md");
        std::fs::write(&path, format!("# Notes\n\n{LONG}\n")).unwrap();
        let id = app.focused_buffer_id();
        app.buffers.get_mut(id).unwrap().buffer = Buffer::from_path(&path).unwrap();
        assert!(app.soft_wraps(id));
        let code = dir.path().join("main.rs");
        std::fs::write(&code, "fn main() {}\n").unwrap();
        app.buffers.get_mut(id).unwrap().buffer = Buffer::from_path(&code).unwrap();
        assert!(!app.soft_wraps(id), "a .rs file isn't prose");
        app.toggle_soft_wrap();
        assert!(app.soft_wraps(id));
        app.toggle_soft_wrap();
        assert!(!app.soft_wraps(id));
    }

    #[test]
    fn folds_still_work_in_a_wrapped_buffer() {
        let mut app = text_app();
        app.config.set_value("editor.soft_wrap", Some(fenix_config::Value::Text("all".into())));
        let id = app.focused_buffer_id();
        let pane = app.focused_pane_id();
        app.note_wrap_cols(pane, 21);
        let source = format!("fn outer() {{\n    // {LONG}\n}}\nlet visible = 2;\n");
        let ob = app.buffers.get_mut(id).unwrap();
        ob.buffer = Buffer::from_text(&source);
        ob.syntax = Some(fenix_syntax::SyntaxState::new(fenix_syntax::LanguageId::Rust, &source));
        let open = app.pane_display_rows(pane, id).unwrap().len();
        app.test_set_cursor(Cursor { char_idx: source.find("explain").unwrap(), sticky_col: 0 });
        app.toggle_code_fold();
        let folded = app.pane_display_lines(pane, id);
        assert_eq!(folded, vec![0, 3, 4], "the wrapped body is hidden with its fold");
        assert!(folded.len() < open);
        assert_eq!(app.display_row_of(pane, id, app.cursor().char_idx), 0, "the cursor is on the header");
        app.toggle_code_fold();
        assert_eq!(app.pane_display_rows(pane, id).unwrap().len(), open);
    }
}
