use super::*;
use crate::folds;
use fenix_vim::FoldCommand;

/// The longest file folded by its indentation, in lines.
const INDENT_FOLD_LINES: usize = 20_000;

#[derive(Clone)]
enum BreadcrumbTarget { Directory(PathBuf), Symbols, Line(usize) }

impl App {
    /// What buffer `ob` can fold, `(header, last line)`: the syntax tree's
    /// scopes, or -- with no tree, or none in it -- its indentation, for
    /// a file of up to `INDENT_FOLD_LINES` lines.
    pub(super) fn fold_ranges_of(&self, ob: &OpenBuffer) -> Vec<folds::Range> {
        if ob.kind != BufferKind::Text {
            return Vec::new();
        }
        let scopes = ob.syntax.as_ref().map(|syntax| syntax.scope_ranges()).unwrap_or_default();
        if !scopes.is_empty() {
            return folds::normalize(scopes);
        }
        if ob.buffer.path().is_none() || ob.buffer.line_count() > INDENT_FOLD_LINES {
            return Vec::new();
        }
        let text = ob.buffer.text();
        folds::by_indent(text.lines(), self.vim.indent_width().max(1))
    }

    pub(super) fn fold_ranges(&self, id: BufferId) -> Vec<folds::Range> {
        self.buffers.get(id).map(|ob| self.fold_ranges_of(ob)).unwrap_or_default()
    }

    /// The actual document rows that occupy display rows after the current
    /// buffer's structural folds have been applied. A collapsed header stays
    /// visible; its body does not. Keeping this as a mapping rather than
    /// mutating buffer text is what makes folds safe to undo, edit and share
    /// across split panes.
    pub(super) fn folded_display_lines(&self, id: BufferId) -> Vec<usize> {
        let Some(ob) = self.buffers.get(id) else { return Vec::new() };
        let collapsed = self.code_folds.get(&id);
        if collapsed.is_none_or(HashSet::is_empty) {
            return (0..ob.buffer.line_count()).collect();
        }
        let ranges = self.fold_ranges_of(ob);
        folds::shown_lines(&ranges, collapsed.expect("checked"), ob.buffer.line_count())
    }

    /// `SPC c z`, `za`: the fold the cursor's in, open or closed.
    pub(crate) fn toggle_code_fold(&mut self) {
        self.fold_command(FoldCommand::Toggle);
    }

    /// Vim's `z` fold commands, on the focused buffer.
    pub(crate) fn fold_command(&mut self, command: FoldCommand) {
        let id = self.focused_buffer_id();
        // Force an incremental parse before consulting scope boundaries.
        self.syntax_highlights_for_visible_range(id, 0, 1);
        let line = self.open().buffer.line_col(&self.cursor()).0;
        let ranges = self.fold_ranges(id);
        if ranges.is_empty() {
            self.set_message("Nothing here folds");
            return;
        }
        let closed = self.code_folds.get(&id).cloned().unwrap_or_default();
        let here = folds::target(&ranges, &closed, line);
        let mut now = closed.clone();
        let mut opened: Option<folds::Range> = None;
        let mut moved_to: Option<usize> = None;
        let mut message = None;
        match command {
            FoldCommand::Toggle => {
                let Some(r) = here else {
                    self.set_message("No foldable scope at the cursor");
                    return;
                };
                if now.remove(&r.0) {
                    opened = Some(r);
                    message = Some("Scope expanded");
                } else {
                    now.insert(r.0);
                    message = Some("Scope folded");
                }
            }
            FoldCommand::Open { recursive } => {
                let Some(r) = here else { return self.set_message("No fold at the cursor") };
                if recursive {
                    for inner in folds::within(&ranges, r) {
                        now.remove(&inner.0);
                    }
                } else {
                    now.remove(&r.0);
                }
                opened = Some(r);
            }
            FoldCommand::Close { recursive } => {
                let Some(r) = folds::innermost(&ranges, line) else { return self.set_message("No fold at the cursor") };
                // Closing what's closed closes the one around it, as Vim does.
                let r = if now.contains(&r.0) && !recursive { ranges.iter().copied().filter(|&(s, e)| s < r.0 && e >= r.1).max_by_key(|r| r.0).unwrap_or(r) } else { r };
                if recursive {
                    now.extend(folds::within(&ranges, r).map(|r| r.0));
                } else {
                    now.insert(r.0);
                }
            }
            FoldCommand::OpenAll | FoldCommand::Reset => {
                now.clear();
                self.fold_levels.insert(id, folds::max_depth(&ranges));
            }
            FoldCommand::CloseAll => {
                now = folds::closed_at_level(&ranges, 0);
                self.fold_levels.insert(id, 0);
            }
            FoldCommand::OpenLevel | FoldCommand::CloseLevel => {
                let max = folds::max_depth(&ranges);
                let level = self.fold_levels.get(&id).copied().unwrap_or(max).min(max);
                let level = if command == FoldCommand::OpenLevel { (level + 1).min(max) } else { level.saturating_sub(1) };
                self.fold_levels.insert(id, level);
                now = folds::closed_at_level(&ranges, level);
                message = Some(if level >= max { "Every fold open" } else if level == 0 { "Every fold closed" } else { "Fold level changed" });
            }
            FoldCommand::Next => moved_to = folds::next_start(&ranges, line),
            FoldCommand::Previous => moved_to = folds::previous_end(&ranges, line),
        }
        if now.is_empty() {
            self.code_folds.remove(&id);
        } else {
            self.code_folds.insert(id, now.clone());
        }
        if let Some(target) = moved_to {
            let (buffer, cursor) = self.focused_buffer_and_cursor_mut();
            cursor.char_idx = buffer.line_start_char(target);
            cursor.sticky_col = 0;
        }
        // The cursor inside a fold that closed goes to its header.
        if let Some(header) = now.iter().copied().filter(|h| !closed.contains(h)).filter_map(|h| ranges.iter().find(|r| r.0 == h)).filter(|&&(s, e)| s < line && line <= e).map(|r| r.0).min() {
            let (buffer, cursor) = self.focused_buffer_and_cursor_mut();
            cursor.char_idx = buffer.line_start_char(header);
            cursor.sticky_col = 0;
        }
        if let Some((header, end)) = opened {
            // Unfolded: the body opens downwards from under its header.
            let first = self.folded_display_lines(id).iter().position(|&l| l == header + 1).unwrap_or(header + 1);
            self.reveal_lines(id, first, end.saturating_sub(header));
        }
        if let Some(message) = message {
            self.set_message(message);
        }
        self.wake_caret();
    }

    /// A click on the fold marker in the gutter of `pane`'s row `row`
    /// (from the top of what it shows): that fold, open or closed. True
    /// when there was one.
    pub(super) fn fold_marker_click(&mut self, pane: fenix_window::WindowId, row: usize, col: usize) -> bool {
        let Some(&buffer) = self.windows().content(pane) else { return false };
        let Some(ob) = self.buffers.get(buffer) else { return false };
        if !self.fold_column(ob) {
            return false;
        }
        // The marker is the gutter's last cell but one, a space after it.
        let gutter = self.gutter_chars(ob);
        if col + 2 != gutter && col + 1 != gutter {
            return false;
        }
        let scroll = self.pane_state(pane).scroll_line;
        let Some(&line) = self.pane_display_lines(pane, buffer).get(scroll + row) else { return false };
        let ranges = self.fold_ranges_of(ob);
        if !ranges.iter().any(|r| r.0 == line) {
            return false;
        }
        let collapsed = self.code_folds.entry(buffer).or_default();
        if !collapsed.remove(&line) {
            collapsed.insert(line);
        }
        if collapsed.is_empty() {
            self.code_folds.remove(&buffer);
        }
        self.wake_caret();
        true
    }

    /// Whether `ob`'s gutter has a column for fold markers: a file's, with
    /// line numbers on, and `editor.fold_markers` not off.
    pub(super) fn fold_column(&self, ob: &OpenBuffer) -> bool {
        ob.kind == BufferKind::Text
            && (ob.syntax.is_some() || ob.buffer.path().is_some())
            && self.line_number_mode != LineNumberMode::Off
            && self.config.bool_value("editor.fold_markers").unwrap_or(true)
    }

    /// For `ob`'s gutter: the lines that start a fold, and those closed.
    pub(super) fn fold_marks(&self, ob: &OpenBuffer) -> Option<(HashMap<usize, usize>, HashSet<usize>)> {
        if !self.fold_column(ob) {
            return None;
        }
        let id = self.buffers.mru().iter().copied().find(|id| self.buffers.get(*id).is_some_and(|b| std::ptr::eq(b, ob)));
        let closed = id.and_then(|id| self.code_folds.get(&id).cloned()).unwrap_or_default();
        Some((self.fold_ranges_of(ob).into_iter().collect(), closed))
    }

    /// Keeps a cursor from becoming invisible after a scope that contains it
    /// was collapsed in another split. The buffer position remains valid; we
    /// simply present the scope header, which is the only visible row for it.
    pub(super) fn normalize_cursor_for_folds(&mut self, id: BufferId, pane: fenix_window::WindowId) {
        let Some(headers) = self.code_folds.get(&id) else { return };
        let headers = headers.clone();
        let line = self.buffers.get(id).expect("open pane buffer").buffer.line_col(&self.pane_state(pane).cursor).0;
        let ranges = self.fold_ranges(id);
        let header = ranges.into_iter()
            .filter(|(start, end)| headers.contains(start) && *start < line && line <= *end)
            .min_by_key(|(start, _)| *start)
            .map(|(start, _)| start);
        if let Some(header) = header {
            let (buffer, cursor) = self.focused_buffer_and_cursor_mut();
            cursor.char_idx = buffer.line_start_char(header);
            cursor.sticky_col = 0;
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn folded_content_spans(
        &self,
        ob: &OpenBuffer,
        display_lines: &[usize],
        rows: usize,
        gutter_chars: usize,
        syntax_highlights: &[(std::ops::Range<usize>, glyphon::Color)],
        cursor_line: usize,
        tab_stops: &TabStops,
        scroll_col: usize,
        collapsed_headers: &HashSet<usize>,
        decorated: &HashMap<usize, crate::decorations::LineLayout>,
    ) -> Vec<(String, glyphon::Color)> {
        let mut spans = Vec::new();
        let fold_ends: HashMap<usize, usize> = if collapsed_headers.is_empty() { HashMap::new() } else { self.fold_ranges_of(ob).into_iter().collect() };
        for row in 0..rows {
            if display_lines.get(row) == Some(&super::side_by_side::FILLER) {
                // A side-by-side's filler row: blank, gutter and all.
                spans.push((" ".repeat(gutter_chars), self.theme.gutter_fg));
            } else if let Some(&line) = display_lines.get(row) {
                spans.extend(self.content_spans_decorated(ob, line, 1, gutter_chars, None, syntax_highlights, cursor_line, tab_stops, scroll_col, decorated));
                if collapsed_headers.contains(&line) {
                    let hidden = fold_ends.get(&line).map_or(0, |end| end - line);
                    spans.push((format!("  ⋯ {hidden} line{}", if hidden == 1 { "" } else { "s" }), self.theme.gutter_fg));
                }
            } else {
                spans.extend(self.content_spans_decorated(ob, ob.buffer.line_count(), 1, gutter_chars, None, syntax_highlights, cursor_line, tab_stops, scroll_col, decorated));
            }
            if row + 1 < rows { spans.push(("\n".to_string(), self.theme.fg)); }
        }
        spans
    }

    pub(crate) fn picker_document_scopes(&mut self) {
        let id = self.focused_buffer_id();
        // Bring the tree current once when invoked, never per picker row.
        self.syntax_highlights_for_visible_range(id, 0, 1);
        let ob = self.open();
        let Some(file) = ob.buffer.path().map(Path::to_path_buf) else {
            self.set_message("Save the buffer to navigate document symbols"); return;
        };
        let rows = ob.syntax.as_ref().map(|syntax| syntax.scope_lines()).unwrap_or_default();
        let candidates = rows.into_iter().filter(|&row| row < ob.buffer.line_count()).map(|row| {
            let start = ob.buffer.line_start_char(row);
            let name = ob.buffer.text_range(start, start + ob.buffer.line_len(row)).trim().to_string();
            let tag = fenix_completion::ctags::TagEntry { name: name.clone(), file: file.clone(), line: row + 1 };
            fenix_picker::Candidate::new(format!("{}  ·  line {}", name, row + 1), tag)
        }).collect();
        self.enter_picker(ActivePicker::Symbol(fenix_picker::PickerState::new(candidates)));
    }
    pub(crate) fn jump_scope_parent(&mut self) {
        let ob = self.open();
        let line = ob.buffer.line_col(&self.cursor()).0;
        let target = ob.syntax.as_ref().and_then(|syntax| {
            syntax.enclosing_scopes(ob.buffer.char_to_byte(self.cursor().char_idx))
                .into_iter().rev().find(|&(start, _)| start < line).map(|(start, _)| start)
        });
        if let Some(line) = target {
            let (buffer, cursor) = self.focused_buffer_and_cursor_mut();
            cursor.char_idx = buffer.line_start_char(line);
            cursor.sticky_col = 0;
            self.wake_caret();
        }
    }

    pub(super) fn editor_status_details(&self) -> String {
        let ob = self.open();
        if ob.kind != BufferKind::Text { return String::new() }
        let language = ob.syntax.as_ref().map(|s| s.language()).or_else(|| ob.buffer.path().and_then(fenix_syntax::detect_language_from_path));
        let language_label = language.map(|lang| if lang.is_grammar() { lang.name() } else { format!("{lang:?}") }).unwrap_or_else(|| "Plain text".into());
        let ending = if ob.buffer.rope().len_lines() > 1 {
            let first = ob.buffer.rope().line(0);
            if first.len_chars() >= 2 && first.char(first.len_chars() - 2) == '\r' { "CRLF" } else { "LF" }
        } else { "No EOL" };
        format!("{language_label}   Indent {}   {}   {ending}", self.vim.indent_width(), ob.buffer.encoding().label())
    }

    #[cfg(test)]
    pub(super) fn scope_breadcrumbs(&self, id: BufferId, char_idx: usize) -> String {
        self.breadcrumb_parts(id, char_idx).into_iter().map(|(text, _)| text).collect::<Vec<_>>().join(" › ")
    }

    /// Presentation-ready breadcrumbs for the two-line VS-style title area.
    /// Icons make the hierarchy scannable while the muted separators avoid
    /// competing with the document's actual syntax colors.
    pub(super) fn breadcrumb_spans(&self, id: BufferId, char_idx: usize) -> Vec<(String, glyphon::Color, bool)> {
        let mut spans = Vec::new();
        // The tab's own content has a leading cell before its icon. Give the
        // breadcrumb the identical inset so the two navigation rows share a
        // clean left edge and icon overhang never touches the window edge.
        spans.push((" ".to_string(), self.theme.fg_modeline, false));
        for (index, (label, target)) in self.breadcrumb_parts(id, char_idx).into_iter().enumerate() {
            if index != 0 {
                spans.push(("  ›  ".to_string(), self.theme.gutter_fg, false));
            }
            match target {
                BreadcrumbTarget::Directory(_) => {
                    spans.push((format!("{} ", icon::icon_for("folder", true, false)), self.theme.icon_folder, true));
                    spans.push((label, self.theme.fg_modeline, false));
                }
                BreadcrumbTarget::Symbols => {
                    spans.push((format!("{} ", icon::navigation_icon_for(&label)), self.theme.icon_file, true));
                    spans.push((label, self.theme.fg_modeline, false));
                }
                BreadcrumbTarget::Line(_) => {
                    // `ƒ` is deliberately body-font text rather than a
                    // font-specific glyph: it remains a clear function
                    // marker even when a system lacks Nerd Fonts.
                    spans.push(("ƒ ".to_string(), self.theme.syntax_function, false));
                    spans.push((label, self.theme.fg_modeline, false));
                }
            }
        }
        spans
    }

    fn breadcrumb_parts(&self, id: BufferId, char_idx: usize) -> Vec<(String, BreadcrumbTarget)> {
        let Some(ob) = self.buffers.get(id) else { return Vec::new() };
        let mut parts = Vec::new();
        if let Some(path) = ob.buffer.path() {
            let root = self.project_root.as_deref().filter(|root| path.starts_with(root));
            let relative = root.and_then(|root| path.strip_prefix(root).ok()).unwrap_or(path);
            let mut target = root.map(Path::to_path_buf).unwrap_or_default();
            for component in relative.components() {
                target.push(component);
                let action = if target == path { BreadcrumbTarget::Symbols } else { BreadcrumbTarget::Directory(target.clone()) };
                parts.push((component.as_os_str().to_string_lossy().into_owned(), action));
            }
        } else {
            parts.push((self.buffer_display_name(id), BreadcrumbTarget::Symbols));
        }
        if let Some(syntax) = &ob.syntax {
            for (line, _) in syntax.enclosing_scopes(ob.buffer.char_to_byte(char_idx.min(ob.buffer.rope().len_chars()))) {
                if line >= ob.buffer.rope().len_lines() { continue }
                let start = ob.buffer.line_start_char(line);
                let label = ob.buffer.text_range(start, start + ob.buffer.line_len(line));
                parts.push((truncate_tab_name(label.trim(), 48), BreadcrumbTarget::Line(line)));
            }
        }
        parts
    }

    pub(super) fn click_breadcrumb(&mut self, geometry: &FrameGeometry, pos: (f32, f32)) -> bool {
        if self.tab_style().is_none() { return false }
        let height = self.text.as_ref().map(|t| t.line_height()).unwrap_or(text::LINE_HEIGHT);
        // Breadcrumb text renders at `text::TITLE_FONT_SCALE` of the body
        // font (same as the tab strip above it), so hit-testing its glyph
        // positions needs that narrower measurement, not the body `char_
        // width` -- see `title_char_width`'s own doc comment.
        let width = self.text.as_ref().map(|t| t.title_char_width()).unwrap_or(text::CHAR_WIDTH * text::TITLE_FONT_SCALE);
        let tab_h = tab_strip_height(height);
        let total_h = title_reserved_height(height);
        for &(pane, rect) in &geometry.panes {
            if !rect.contains_point(pos.0, pos.1) || pos.1 < rect.y + tab_h || pos.1 >= rect.y + total_h
                || self.pane_titles.contains_key(&pane) || !geometry.tabs.iter().any(|(id, _)| *id == pane)
                || (pane == self.focused_pane_id() && (self.main_view != MainView::Editor || self.debug_ribbon_shown())) { continue }
            let Some(&id) = self.windows().content(pane) else { continue };
            let parts = self.breadcrumb_parts(id, self.pane_state(pane).cursor.char_idx);
            // Matches the leading inset emitted by `breadcrumb_spans`.
            let mut x = rect.x + text::PAD_LEFT + width;
            for (index, (label, action)) in parts.into_iter().enumerate() {
                // Keep hit targets aligned with the visible icon and muted
                // separator chrome emitted by `breadcrumb_spans`: two cells
                // for each item's marker, and five for each inter-item gap.
                if index != 0 { x += 5.0 * width; }
                x += 2.0 * width;
                let end = x + label.chars().count() as f32 * width;
                if pos.0 >= x && pos.0 < end {
                    self.windows_mut().focus(pane);
                    self.sidebar_focused = false;
                    self.terminal_focused = false;
                    match action {
                        BreadcrumbTarget::Directory(path) => self.open_dired_at(&path),
                        BreadcrumbTarget::Symbols => self.picker_document_scopes(),
                        BreadcrumbTarget::Line(line) => {
                            let from = JumpEntry { buffer: id, char_idx: self.cursor().char_idx };
                            let (buffer, cursor) = self.focused_buffer_and_cursor_mut();
                            cursor.char_idx = buffer.line_start_char(line);
                            cursor.sticky_col = 0;
                            self.record_jump(from);
                        }
                    }
                    self.wake_caret();
                    return true;
                }
                x = end + 3.0 * width;
            }
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn breadcrumbs_follow_nested_rust_scopes_without_changing_text() {
        let mut app = App::with_file(None);
        let id = app.focused_buffer_id();
        app.test_insert_str("mod outer {\n    fn inner() {\n        let value = 1;\n    }\n}\n");
        let source = app.open().buffer.text();
        app.buffers.get_mut(id).unwrap().syntax = Some(fenix_syntax::SyntaxState::new(fenix_syntax::LanguageId::Rust, &source));
        let offset = source.find("let value").unwrap();
        let parts = app.breadcrumb_parts(id, offset);
        assert_eq!(parts.iter().filter_map(|(_, action)| match action { BreadcrumbTarget::Line(line) => Some(*line), _ => None }).collect::<Vec<_>>(), vec![0, 1]);
        assert!(app.scope_breadcrumbs(id, offset).contains("fn inner()"));
        let rendered = app.breadcrumb_spans(id, offset);
        assert!(rendered.iter().any(|(text, _, _)| text == "ƒ "), "scope breadcrumbs carry a stable function marker");
        assert!(rendered.iter().any(|(text, _, _)| text.contains('›')), "hierarchy separators remain visible between breadcrumb items");
        assert_eq!(app.open().buffer.text(), source);
    }

    #[test]
    fn status_details_distinguish_crlf_from_lf() {
        let mut app = App::with_file(None);
        let id = app.focused_buffer_id();
        let ob = app.buffers.get_mut(id).unwrap();
        ob.kind = BufferKind::Text;
        ob.buffer = Buffer::from_text("hello\r\nworld");
        assert!(app.editor_status_details().ends_with("CRLF"));
        app.buffers.get_mut(id).unwrap().buffer = Buffer::from_text("hello\nworld");
        assert!(app.editor_status_details().ends_with("LF"));
        assert!(!app.editor_status_details().ends_with("CRLF"));
    }

    #[test]
    fn folding_a_scope_removes_its_body_from_display_rows_and_expands_cleanly() {
        let mut app = App::with_file(None);
        let id = app.focused_buffer_id();
        let source = "fn outer() {\n    let hidden = 1;\n}\nlet visible = 2;\n".to_string();
        let ob = app.buffers.get_mut(id).unwrap();
        ob.kind = BufferKind::Text;
        ob.buffer = Buffer::from_text(&source);
        app.buffers.get_mut(id).unwrap().syntax = Some(fenix_syntax::SyntaxState::new(fenix_syntax::LanguageId::Rust, &source));
        app.test_set_cursor(Cursor { char_idx: source.find("hidden").unwrap(), sticky_col: 0 });

        app.toggle_code_fold();
        assert_eq!(app.folded_display_lines(id), vec![0, 3, 4]);
        assert_eq!(app.open().buffer.line_col(&app.cursor()).0, 0, "a collapsed scope must leave a visible cursor");

        app.toggle_code_fold();
        assert_eq!(app.folded_display_lines(id), vec![0, 1, 2, 3, 4]);
    }

    fn rust_app(source: &str) -> (App, BufferId) {
        let mut app = App::with_file(None);
        let id = app.focused_buffer_id();
        let ob = app.buffers.get_mut(id).unwrap();
        ob.kind = BufferKind::Text;
        ob.buffer = Buffer::from_text(source);
        app.buffers.get_mut(id).unwrap().syntax = Some(fenix_syntax::SyntaxState::new(fenix_syntax::LanguageId::Rust, source));
        app.line_number_mode = LineNumberMode::Absolute;
        (app, id)
    }

    const NESTED: &str = "impl Outer {\n    fn inner() {\n        y();\n    }\n}\nfn other() {\n    z();\n}\n";

    #[test]
    fn vims_fold_keys_close_open_and_go_level_by_level() {
        let (mut app, id) = rust_app(NESTED);
        app.test_set_cursor(Cursor { char_idx: NESTED.find("y()").unwrap(), sticky_col: 0 });
        app.fold_command(FoldCommand::Close { recursive: false });
        assert_eq!(app.folded_display_lines(id), vec![0, 1, 4, 5, 6, 7, 8], "the innermost: the method");
        app.fold_command(FoldCommand::Close { recursive: false });
        assert_eq!(app.folded_display_lines(id), vec![0, 5, 6, 7, 8], "zc again closes the one around it");
        app.fold_command(FoldCommand::OpenAll);
        assert_eq!(app.folded_display_lines(id).len(), 9);
        app.fold_command(FoldCommand::CloseAll);
        assert_eq!(app.folded_display_lines(id), vec![0, 5, 8]);
        app.fold_command(FoldCommand::OpenLevel);
        assert_eq!(app.folded_display_lines(id), vec![0, 1, 4, 5, 6, 7, 8], "zr: the outer ones open, the method still closed");
        app.fold_command(FoldCommand::OpenLevel);
        assert_eq!(app.folded_display_lines(id).len(), 9);
        app.test_set_cursor(Cursor { char_idx: 0, sticky_col: 0 });
        app.fold_command(FoldCommand::Next);
        assert_eq!(app.open().buffer.line_col(&app.cursor()).0, 1, "zj: the next fold's start");
        app.fold_command(FoldCommand::Previous);
        assert_eq!(app.open().buffer.line_col(&app.cursor()).0, 1, "zk from the method's start: nothing ends before it, so it stays");
        app.test_set_cursor(Cursor { char_idx: NESTED.find("z()").unwrap(), sticky_col: 0 });
        app.fold_command(FoldCommand::Previous);
        assert_eq!(app.open().buffer.line_col(&app.cursor()).0, 4, "zk: the end of the impl");
    }

    #[test]
    fn the_gutter_marks_folds_and_a_click_on_one_toggles_it() {
        let (mut app, id) = rust_app(NESTED);
        let gutter = app.gutter_chars(app.open());
        assert_eq!(gutter, 3, "a digit, the marker, a space");
        let spans = app.content_spans(app.open(), 0, 3, gutter, None, &[], 0, &TabStops::Fixed(DEFAULT_TAB_WIDTH), 0);
        let text: String = spans.iter().map(|(s, _)| s.as_str()).collect();
        assert!(text.starts_with("1▾ impl Outer {\n2▾     fn inner() {\n3  "), "{text:?}");
        let pane = app.focused_pane_id();
        assert!(app.fold_marker_click(pane, 0, 1));
        assert_eq!(app.folded_display_lines(id), vec![0, 5, 6, 7, 8]);
        assert!(!app.fold_marker_click(pane, 2, 1), "line 6 starts no fold");
        let display = app.folded_display_lines(id);
        let spans = app.folded_content_spans(app.open(), &display, 2, gutter, &[], 0, &TabStops::Fixed(DEFAULT_TAB_WIDTH), 0, app.code_folds.get(&id).unwrap(), &HashMap::new());
        let text: String = spans.iter().map(|(s, _)| s.as_str()).collect();
        assert!(text.starts_with("1▸ impl Outer {  ⋯ 4 lines"), "{text:?}");
        app.config.set_value("editor.fold_markers", Some(fenix_config::Value::Bool(false)));
        assert_eq!(app.gutter_chars(app.open()), 2);
    }

    #[test]
    fn a_file_with_no_syntax_folds_by_its_indentation() {
        let mut app = App::with_file(None);
        let id = app.focused_buffer_id();
        let ob = app.buffers.get_mut(id).unwrap();
        ob.kind = BufferKind::Text;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.conf");
        std::fs::write(&path, "a:\n  b: 1\n  c: 2\nd: 3\n").unwrap();
        ob.buffer = Buffer::from_path(&path).unwrap();
        assert_eq!(app.fold_ranges(id), vec![(0, 2)]);
        app.fold_command(FoldCommand::Toggle);
        assert_eq!(app.folded_display_lines(id), vec![0, 3, 4]);
    }

}
