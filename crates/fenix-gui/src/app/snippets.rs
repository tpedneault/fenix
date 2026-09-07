//! Insert-mode adapter. The engine owns parsing/ranges; the app owns focus,
//! editability, completion priority and the existing selection overlay.
use super::*;
use fenix_snippets::{Catalog, Context, Edit, Session};

pub(super) struct ActiveSnippet {
    buffer: BufferId,
    pub(super) pane: fenix_window::WindowId,
    pub(super) session: Session,
}

#[derive(Clone)]
pub(super) struct SnippetChoice {
    snippet: fenix_snippets::Snippet,
    buffer: BufferId,
    pane: fenix_window::WindowId,
    cursor: usize,
    revision: u64,
}

impl App {
    pub(super) fn snippet_catalog(&self) -> Catalog {
        Catalog::load(
            &self
                .config
                .path()
                .parent()
                .unwrap_or(Path::new("."))
                .join("snippets"),
        )
    }

    pub(super) fn snippet_scope(&self) -> String {
        self.open()
            .buffer
            .path()
            .and_then(fenix_syntax::detect_language_from_path)
            .map(crate::lsp::language_config_name)
            .unwrap_or_else(|| "text".into())
    }

    pub(crate) fn start_snippet_picker(&mut self) {
        if !self.open().kind.tracks_unsaved_changes()
            || self.buffer_edits_are_reverted(self.focused_buffer_id(), self.open().kind)
        {
            self.set_error("snippets can only be inserted in editable documents");
            return;
        }
        let catalog = self.snippet_catalog();
        if let Some(error) = catalog.errors.first() {
            self.set_error(format!("snippets: {error}"));
        }
        let candidates = catalog
            .available(&self.snippet_scope())
            .into_iter()
            .map(|snippet| {
                let preview = snippet
                    .template
                    .render(
                        &Default::default(),
                        &Context::current(self.open().buffer.path()),
                        "",
                    )
                    .text;
                let label = format!(
                    "{}  — {}  · {}",
                    snippet.trigger,
                    snippet.name,
                    completion::clipped_line(&preview, 65)
                );
                fenix_picker::Candidate::new(
                    label,
                    SnippetChoice {
                        snippet: snippet.clone(),
                        buffer: self.focused_buffer_id(),
                        pane: self.focused_pane_id(),
                        cursor: self.cursor().char_idx,
                        revision: self.open().buffer.edit_count(),
                    },
                )
            })
            .collect();
        self.completion = None;
        self.enter_picker(ActivePicker::Snippet(fenix_picker::PickerState::new(
            candidates,
        )));
    }

    pub(super) fn insert_picked_snippet(&mut self, choice: SnippetChoice) {
        if choice.buffer != self.focused_buffer_id()
            || choice.pane != self.focused_pane_id()
            || choice.cursor != self.cursor().char_idx
            || choice.revision != self.open().buffer.edit_count()
        {
            self.set_error("document changed while choosing a snippet; open the picker again");
            return;
        }
        let cursor = self.cursor();
        self.vim.exit_visual_mode(&cursor);
        if self.vim.mode() != Mode::Insert {
            let ob = self.buffers.get_mut(choice.buffer).unwrap();
            let state = self
                .workspaces
                .active_pane_states_mut()
                .get_mut(&choice.pane)
                .unwrap();
            self.vim
                .handle_key(&mut ob.buffer, &mut state.cursor, KeyPress::char('i'));
        }
        self.expand_snippet_template(choice.cursor, choice.snippet.template);
    }

    pub(super) fn expand_snippet_template(
        &mut self,
        start: usize,
        template: fenix_snippets::Template,
    ) {
        if !self.open().kind.tracks_unsaved_changes()
            || self.buffer_edits_are_reverted(self.focused_buffer_id(), self.open().kind)
        {
            self.set_error("snippets can only be inserted in editable documents");
            return;
        }
        let context = Context::current(self.open().buffer.path());
        let id = self.focused_buffer_id();
        let pane = self.focused_pane_id();
        let (buffer, cursor) = self.focused_buffer_and_cursor_mut();
        self.snippet = Session::expand(buffer, cursor, start, template, context).map(|session| {
            ActiveSnippet {
                buffer: id,
                pane,
                session,
            }
        });
        self.completion = None;
        if !self.replaying_change {
            self.change_capture_dirty = true;
        }
        self.marks.insert(
            '.',
            JumpEntry {
                buffer: id,
                char_idx: self.cursor().char_idx,
            },
        );
    }
    pub(super) fn snippet_is_valid(&self) -> bool {
        self.vim.mode() == Mode::Insert
            && self.snippet.as_ref().is_some_and(|s| {
                s.buffer == self.focused_buffer_id()
                    && s.pane == self.focused_pane_id()
                    && s.session.valid(&self.open().buffer, &self.cursor())
            })
    }

    pub(super) fn snippet_key(&mut self, key: KeyPress) -> bool {
        if !self.snippet_is_valid() {
            self.snippet = None;
        }
        if self.vim.mode() != Mode::Insert
            || !self.open().kind.tracks_unsaved_changes()
            || self.buffer_edits_are_reverted(self.focused_buffer_id(), self.open().kind)
        {
            return false;
        }
        let next =
            key == KeyPress::named(FenixNamedKey::Tab) || key == KeyPress::char('j').with_ctrl();
        let previous = key == KeyPress::char('k').with_ctrl();
        if let Some(mut active) = self.snippet.take() {
            self.completion = None;
            if next || previous {
                let (buffer, cursor) = self.focused_buffer_and_cursor_mut();
                let continuing = active.session.advance(cursor, previous);
                cursor.sticky_col = buffer.line_col(cursor).1;
                if continuing {
                    self.snippet = Some(active);
                }
                return true;
            }
            let typed;
            let edit = match key.code {
                KeyCode::Char(ch) if key.mods == Mods::default() => {
                    typed = ch.to_string();
                    Some(Edit::Insert(&typed))
                }
                KeyCode::Named(FenixNamedKey::Enter) if key.mods == Mods::default() => {
                    Some(Edit::Insert("\n"))
                }
                KeyCode::Named(FenixNamedKey::Backspace) if key.mods == Mods::default() => {
                    Some(Edit::Backspace)
                }
                KeyCode::Named(FenixNamedKey::Delete) if key.mods == Mods::default() => {
                    Some(Edit::Delete)
                }
                _ => None,
            };
            if let Some(edit) = edit {
                let (buffer, cursor) = self.focused_buffer_and_cursor_mut();
                active.session.edit(buffer, cursor, edit);
                if !self.replaying_change {
                    self.change_capture_dirty = true;
                }
                self.marks.insert(
                    '.',
                    JumpEntry {
                        buffer: active.buffer,
                        char_idx: self.cursor().char_idx,
                    },
                );
                self.snippet = Some(active);
                return true;
            }
            // Escape reaches Vim on this same press; movement/other commands
            // end the session and continue with their ordinary behavior.
            return false;
        }
        if !next {
            return false;
        }
        if self.completion_selected && self.completion.is_some() {
            return false;
        }
        let directory = self
            .config
            .path()
            .parent()
            .unwrap_or(Path::new("."))
            .join("snippets");
        let catalog = Catalog::load(&directory);
        if let Some(error) = catalog.errors.first() {
            self.set_error(format!("snippets: {error}"));
        }
        let cursor = self.cursor();
        let buffer = &self.open().buffer;
        let scope = buffer
            .path()
            .and_then(fenix_syntax::detect_language_from_path)
            .map(|language| format!("{language:?}").to_lowercase())
            .unwrap_or_else(|| "text".into());
        let before = buffer.text_range(
            buffer.line_start_char(buffer.line_col(&cursor).0),
            cursor.char_idx,
        );
        let Some(snippet) = catalog.matching(&before, &scope) else {
            return false;
        };
        let context = Context::current(buffer.path());
        let start = cursor.char_idx - snippet.trigger.chars().count();
        let id = self.focused_buffer_id();
        let pane = self.focused_pane_id();
        let (buffer, cursor) = self.focused_buffer_and_cursor_mut();
        self.snippet = Session::expand(buffer, cursor, start, snippet.template.clone(), context)
            .map(|session| ActiveSnippet {
                buffer: id,
                pane,
                session,
            });
        self.completion = None;
        if !self.replaying_change {
            self.change_capture_dirty = true;
        }
        self.marks.insert(
            '.',
            JumpEntry {
                buffer: id,
                char_idx: self.cursor().char_idx,
            },
        );
        if catalog.errors.is_empty() {
            self.set_message(format!(
                "{} — Tab: next field, Ctrl-K: previous, Esc: finish",
                snippet.name
            ));
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(text: &str) -> (tempfile::TempDir, App) {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("test.tcl");
        std::fs::write(&file, text).unwrap();
        let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
        app.config = fenix_config::Config::load_or_default(dir.path().join("config.ini"));
        app.test_vim_key(KeyPress::char('i'));
        app.focused_buffer_and_cursor_mut().1.char_idx = text.chars().count();
        (dir, app)
    }

    fn key(app: &mut App, key: KeyPress) {
        if !app.snippet_key(key) && !app.completion_key(key) {
            app.test_vim_key(key);
            app.sync_completion();
        }
    }

    #[test]
    fn snippet_tab_wins_over_completion_and_fields_are_highlighted() {
        let (_dir, mut app) = app("proc");
        app.sync_completion();
        key(&mut app, KeyPress::named(FenixNamedKey::Tab));
        assert!(app.open().buffer.text().starts_with("proc name {args}"));
        assert!(app.completion.is_none());
        assert!(app.snippet_is_valid());
        assert_eq!(app.visual_selection_segments(20), vec![(0, 5, 9)]);
        for ch in "greet".chars() {
            key(&mut app, KeyPress::char(ch));
        }
        key(&mut app, KeyPress::named(FenixNamedKey::Tab));
        key(&mut app, KeyPress::char('x'));
        key(&mut app, KeyPress::char('k').with_ctrl());
        key(&mut app, KeyPress::char('f'));
        assert!(app.open().buffer.text().starts_with("proc f {x}"));
        assert_eq!(app.vim.mode(), Mode::Insert);
        key(&mut app, KeyPress::named(FenixNamedKey::Tab));
        key(&mut app, KeyPress::named(FenixNamedKey::Tab));
        key(&mut app, KeyPress::named(FenixNamedKey::Tab));
        assert!(app.snippet.is_none());
    }

    #[test]
    fn escape_leaves_insert_on_the_same_press_and_arrows_cancel() {
        let (_dir, mut app) = app("proc");
        key(&mut app, KeyPress::named(FenixNamedKey::Tab));
        key(&mut app, KeyPress::named(FenixNamedKey::Escape));
        assert!(app.snippet.is_none());
        assert_eq!(app.vim.mode(), Mode::Normal);
        app.test_vim_key(KeyPress::char('u'));
        assert_eq!(app.open().buffer.text(), "proc");

        let (_dir, mut app) = self::app("proc");
        key(&mut app, KeyPress::named(FenixNamedKey::Tab));
        key(&mut app, KeyPress::named(FenixNamedKey::Right));
        assert!(app.snippet.is_none());
        assert_eq!(app.cursor().char_idx, 6);
    }

    #[test]
    fn tab_without_a_trigger_retains_indentation_behavior() {
        let (_dir, mut app) = app("");
        assert!(!app.snippet_key(KeyPress::named(FenixNamedKey::Tab)));
        key(&mut app, KeyPress::named(FenixNamedKey::Tab));
        assert!(!app.open().buffer.text().is_empty());
        assert!(app.open().buffer.text().chars().all(|c| c == ' '));
    }

    #[test]
    fn user_files_reload_and_transformed_mirrors_update_through_keys() {
        let (dir, mut app) = app("proc");
        let snippets = dir.path().join("snippets");
        std::fs::create_dir(&snippets).unwrap();
        std::fs::write(
            snippets.join("proc.snippet"),
            "# key: proc\n# scope: tcl\n# --\n${1:title} [${1|remove_whitespace|center:6}]$0",
        )
        .unwrap();
        key(&mut app, KeyPress::named(FenixNamedKey::Tab));
        for ch in "a b".chars() {
            key(&mut app, KeyPress::char(ch));
        }
        assert_eq!(app.open().buffer.text(), "a b [  ab  ]");
        key(&mut app, KeyPress::named(FenixNamedKey::Tab));
        assert_eq!(app.cursor().char_idx, 12);
        assert!(app.snippet.is_none());
    }

    #[test]
    fn unexpected_buffer_changes_cancel_before_next_edit() {
        let (_dir, mut app) = app("proc");
        key(&mut app, KeyPress::named(FenixNamedKey::Tab));
        let (buffer, cursor) = app.focused_buffer_and_cursor_mut();
        buffer.replace_range(cursor, 0, buffer.len_chars(), "external");
        key(&mut app, KeyPress::char('!'));
        assert_eq!(app.open().buffer.text(), "external!");
        assert!(app.snippet.is_none());
    }

    #[test]
    fn read_only_buffers_cannot_expand_and_pane_changes_end_sessions() {
        let (_dir, mut app) = app("proc");
        let id = app.focused_buffer_id();
        for kind in [
            BufferKind::Dashboard,
            BufferKind::Git,
            BufferKind::WorkspaceEdit,
            BufferKind::TaskOutput,
        ] {
            app.buffers.get_mut(id).unwrap().kind = kind;
            assert!(
                !app.snippet_key(KeyPress::named(FenixNamedKey::Tab)),
                "{kind:?}"
            );
            assert_eq!(app.open().buffer.text(), "proc");
        }

        let (_dir, mut app) = self::app("proc");
        key(&mut app, KeyPress::named(FenixNamedKey::Tab));
        assert!(app.snippet_is_valid());
        app.set_pane_content(app.focused_pane_id(), app.focused_buffer_id());
        assert!(app.snippet.is_none());
    }
    #[test]
    fn snippet_picker_filters_inserts_and_cancels_without_editing() {
        let (_dir, mut app) = app("");
        app.test_vim_key(KeyPress::named(FenixNamedKey::Escape));
        app.start_snippet_picker();
        let Some(ActivePicker::Snippet(state)) = app.active_picker.as_mut() else {
            panic!("snippet picker missing")
        };
        state.set_query("proc");
        assert_eq!(state.len(), 1);
        app.picker_confirm();
        assert!(app.open().buffer.text().starts_with("proc name {args}"));
        assert!(app.snippet_is_valid());
        assert_eq!(app.vim.mode(), Mode::Insert);
        let before = app.open().buffer.text();
        app.start_snippet_picker();
        app.picker_cancel();
        assert_eq!(app.open().buffer.text(), before);
    }

    #[test]
    fn completion_can_expand_a_partial_snippet_trigger() {
        let (_dir, mut app) = app("pr");
        app.force_open_completion();
        let state = app.completion.as_mut().unwrap();
        let row = state
            .picker
            .visible_rows(0, state.picker.len())
            .position(|(_, c)| {
                c.payload.source == completion::Source::Snippet && c.payload.label == "proc"
            })
            .unwrap();
        state.picker.move_selection(row as isize);
        app.accept_completion();
        assert!(app.open().buffer.text().starts_with("proc name {args}"));
        assert!(app.snippet_is_valid());
    }
    #[test]
    fn late_lsp_results_preserve_explicit_selection_and_stale_accept_is_rejected() {
        let (_dir, mut app) = app("pr");
        app.force_open_completion();
        let state = app.completion.as_mut().unwrap();
        let row = state
            .picker
            .visible_rows(0, state.picker.len())
            .position(|(_, c)| {
                c.payload.source == completion::Source::Snippet && c.payload.label == "proc"
            })
            .unwrap();
        state.picker.move_selection(row as isize);
        app.completion_selected = true;
        app.apply_lsp_completion(
            app.focused_buffer_id(),
            0,
            serde_json::json!([{"label":"print","detail":"print(value)"}]),
        );
        assert_eq!(
            app.completion
                .as_ref()
                .unwrap()
                .picker
                .selected()
                .unwrap()
                .payload
                .source,
            completion::Source::Snippet
        );
        app.focused_buffer_and_cursor_mut().1.char_idx = 0;
        app.accept_completion();
        assert_eq!(app.open().buffer.text(), "pr");
    }

    #[test]
    fn completion_popup_keeps_last_result_visible_with_details() {
        let (_dir, mut app) = app("p");
        let candidates = (0..30)
            .map(|i| {
                let mut item =
                    completion::Item::text(format!("print{i:02}"), completion::Source::Lsp);
                item.detail = "function(value)".into();
                fenix_picker::Candidate::new(item.label.clone(), item)
            })
            .collect();
        let mut picker = fenix_picker::PickerState::new(candidates);
        picker.move_selection(29);
        app.completion = Some(CompletionState {
            prefix_start: 0,
            picker,
        });
        let (_, spans, selected) = app
            .completion_popup(
                800.0,
                600.0,
                fenix_window::Rect {
                    x: 0.0,
                    y: 0.0,
                    w: 800.0,
                    h: 600.0,
                },
                Some((0, 1)),
                0.0,
                0.0,
            )
            .unwrap();
        assert!(selected.is_some());
        assert!(spans.iter().any(|(s, _, _)| s == "print29"));
        assert!(spans.iter().any(|(s, _, _)| s.contains("function(value)")));
    }
}
