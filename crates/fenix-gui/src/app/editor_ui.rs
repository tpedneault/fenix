use super::*;

#[derive(Clone)]
enum BreadcrumbTarget { Directory(PathBuf), Symbols, Line(usize) }

impl App {
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
        let ranges = ob.syntax.as_ref().map(|syntax| syntax.scope_ranges()).unwrap_or_default();
        let mut end_by_header = HashMap::new();
        for (start, end) in ranges {
            if collapsed.is_some_and(|headers| headers.contains(&start)) {
                end_by_header.insert(start, end);
            }
        }
        let mut lines = Vec::new();
        let mut line = 0;
        while line < ob.buffer.line_count() {
            lines.push(line);
            line = end_by_header.get(&line).copied().map_or(line + 1, |end| end + 1);
        }
        lines
    }

    pub(crate) fn toggle_code_fold(&mut self) {
        let id = self.focused_buffer_id();
        // Force an incremental parse before consulting scope boundaries.
        self.syntax_highlights_for_visible_range(id, 0, 1);
        let (line, ranges) = {
            let ob = self.open();
            let line = ob.buffer.line_col(&self.cursor()).0;
            let ranges = ob.syntax.as_ref().map(|syntax| syntax.scope_ranges()).unwrap_or_default();
            (line, ranges)
        };
        let target = ranges
            .iter()
            .filter(|&&(start, end)| start <= line && line <= end)
            .min_by_key(|&&(start, end)| end - start)
            .copied();
        let Some((header, _)) = target else {
            self.set_message("No foldable scope at the cursor");
            return;
        };
        let collapsed = self.code_folds.entry(id).or_default();
        let now_collapsed = if !collapsed.insert(header) {
            collapsed.remove(&header);
            false
        } else {
            true
        };
        if now_collapsed {
            let (buffer, cursor) = self.focused_buffer_and_cursor_mut();
            cursor.char_idx = buffer.line_start_char(header);
            cursor.sticky_col = 0;
        }
        self.set_message(if now_collapsed { "Scope folded" } else { "Scope expanded" });
        self.wake_caret();
    }

    /// Keeps a cursor from becoming invisible after a scope that contains it
    /// was collapsed in another split. The buffer position remains valid; we
    /// simply present the scope header, which is the only visible row for it.
    pub(super) fn normalize_cursor_for_folds(&mut self, id: BufferId, pane: fenix_window::WindowId) {
        let Some(headers) = self.code_folds.get(&id) else { return };
        let (line, ranges) = {
            let ob = self.buffers.get(id).expect("open pane buffer");
            (ob.buffer.line_col(&self.pane_state(pane).cursor).0, ob.syntax.as_ref().map(|syntax| syntax.scope_ranges()).unwrap_or_default())
        };
        let header = ranges.into_iter()
            .filter(|(start, end)| headers.contains(start) && *start < line && line <= *end)
            .max_by_key(|(start, _)| *start)
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
    ) -> Vec<(String, glyphon::Color)> {
        let mut spans = Vec::new();
        for row in 0..rows {
            if let Some(&line) = display_lines.get(row) {
                spans.extend(self.content_spans(ob, line, 1, gutter_chars, None, syntax_highlights, cursor_line, tab_stops, scroll_col));
                if collapsed_headers.contains(&line) {
                    spans.push(("  …".to_string(), self.theme.gutter_fg));
                }
            } else {
                spans.extend(self.content_spans(ob, ob.buffer.line_count(), 1, gutter_chars, None, syntax_highlights, cursor_line, tab_stops, scroll_col));
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
        let language = ob.buffer.path().and_then(fenix_syntax::detect_language_from_path);
        let language_label = language.map(|lang| format!("{lang:?}")).unwrap_or_else(|| "Plain text".into());
        let ending = if ob.buffer.rope().len_lines() > 1 {
            let first = ob.buffer.rope().line(0);
            if first.len_chars() >= 2 && first.char(first.len_chars() - 2) == '\r' { "CRLF" } else { "LF" }
        } else { "No EOL" };
        format!("{language_label}   Indent {}   UTF-8   {ending}", self.vim.indent_width())
    }

    pub(super) fn scope_breadcrumbs(&self, id: BufferId, char_idx: usize) -> String {
        self.breadcrumb_parts(id, char_idx).into_iter().map(|(text, _)| text).collect::<Vec<_>>().join(" › ")
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
        if !self.theme.show_tabs { return false }
        let height = self.text.as_ref().map(|t| t.line_height()).unwrap_or(text::LINE_HEIGHT);
        let width = self.text.as_ref().map(|t| t.char_width()).unwrap_or(text::CHAR_WIDTH);
        for &(pane, rect) in &geometry.panes {
            if !rect.contains_point(pos.0, pos.1) || pos.1 < rect.y + height || pos.1 >= rect.y + 2.0 * height
                || self.pane_titles.contains_key(&pane) || !geometry.tabs.iter().any(|(id, _)| *id == pane)
                || (pane == self.focused_pane_id() && self.main_view != MainView::Editor) { continue }
            let Some(&id) = self.windows().content(pane) else { continue };
            let parts = self.breadcrumb_parts(id, self.pane_state(pane).cursor.char_idx);
            let mut x = rect.x + text::PAD_LEFT;
            for (label, action) in parts {
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
}
