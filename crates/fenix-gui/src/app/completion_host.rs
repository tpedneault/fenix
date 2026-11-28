//! Completion: asking for it, its popup, and its keys.
//!
//! Methods of `App` moved here from `app.rs` as they were.

use super::*;

impl App {
    /// Re-derives the completion popup's state from whatever's current --
    /// called once right after every keystroke that reaches Vim while
    /// Insert-relevant (`handle_key`'s Vim-fallthrough tier). Closes the
    /// popup (via `None`) the instant any of its preconditions stop
    /// holding: not in Insert mode, the cursor's inside a comment or
    /// string (`cursor_in_comment_or_string`), not a Tcl buffer, no
    /// identifier prefix at the cursor, or the prefix no longer matches
    /// anything.
    pub(super) fn sync_completion(&mut self) {
        // The popup opens on an edit (a character typed, one deleted) and
        // only follows the cursor once open: an arrow or a click that
        // lands after a word is not asking for it -- in standard keys,
        // where a buffer rests in Insert, it would follow every move.
        let seen = self.buffers.get(self.focused_buffer_id()).map(|b| (self.focused_buffer_id(), b.buffer.edit_count()));
        let edited = std::mem::replace(&mut self.completion_edit_seen, seen) != seen;
        if self.completion.is_none() && !edited {
            return;
        }
        if self.snippet_is_valid() {
            self.completion = None;
            return;
        }
        if self.vim.mode() != Mode::Insert {
            self.completion = None;
            return;
        }
        // A prompt for an agent is prose to it: words offered as you type
        // only get in the way (its @ and / open pickers of their own, and
        // Ctrl-Space still asks).
        if self.buffers.get(self.focused_buffer_id()).is_some_and(|b| b.kind == BufferKind::Prompt) {
            self.completion = None;
            return;
        }
        let cursor = self.cursor();
        let lsp_attached = self.focused_lsp_context().is_some();
        // In a comment or a string, local words and keywords would only
        // get in the way -- but a language server may know exactly what
        // goes there (a requirement ID in `// implements: REQ-014`), so
        // attached servers are still asked.
        let in_comment = self.cursor_in_comment_or_string();
        if in_comment && !lsp_attached {
            self.completion = None;
            return;
        }
        // A character a server declared a trigger (`.`, `::`'s `:`, `-`)
        // asks those servers even with no word typed after it yet.
        let trigger = lsp_attached.then(|| self.completion_trigger_before(cursor.char_idx)).flatten();
        let found = if in_comment {
            completion::prefix_at_cursor(&self.open().buffer, &cursor).map(|(start, prefix)| (start, prefix, Vec::new()))
        } else {
            self.completion_at_cursor()
        };
        let Some((start, prefix, candidates)) = found.or_else(|| trigger.map(|_| (cursor.char_idx, String::new(), Vec::new()))) else {
            self.completion = None;
            return;
        };

        self.completion_context = Some((self.focused_buffer_id(), self.focused_pane_id(), cursor.char_idx, self.open().buffer.edit_count()));
        self.completion_selected = false;
        self.completion_asked = false;
        self.completion_local = !in_comment;
        let mut picker = fenix_picker::PickerState::new(candidates);
        picker.set_query(&prefix);
        self.completion = Some(CompletionState { prefix_start: start, picker });
        // A server attached to this buffer may still fill the popup
        // once its (async) response lands (`apply_lsp_completion`),
        // even if the locally-known candidates are empty right now --
        // so an empty popup isn't closed out from under that pending
        // request the way it is when there's no server to wait on.
        if lsp_attached {
            self.request_lsp_completion(start, if start == cursor.char_idx { trigger } else { None });
        }
        if !lsp_attached && self.completion.as_ref().is_some_and(|state| state.picker.is_empty()) {
            self.completion = None;
        }
        self.completion_scroll = 0;
    }

    /// The completion popup's own keys, claimed before Vim ever sees
    /// them. Returns whether `keypress` was claimed.
    ///
    /// The popup only ever exists in Insert mode. `Ctrl-Space`
    /// force-opens it whether or not it is already open (unclaimed
    /// otherwise -- not a global Ctrl chord, not handled by
    /// `handle_insert_key`). While it is open, its navigation/accept/
    /// dismiss keys are claimed here; every other key (plain typing,
    /// Backspace, arrows, Escape) falls through to the ordinary Vim
    /// dispatch, after which `sync_completion` re-derives the popup's
    /// state fresh -- there is no `VimEvent` for "a character was
    /// typed", so this recomputes rather than tracking incrementally.
    pub(super) fn completion_key(&mut self, keypress: KeyPress) -> bool {
        if self.vim.mode() != Mode::Insert {
            return false;
        }
        if keypress == KeyPress::char(' ').with_ctrl() {
            self.force_open_completion();
            return true;
        }
        if self.completion.is_none() {
            return false;
        }
        // With standard keys Escape never leaves Insert, so it is free to
        // do what it does in every other editor: close the popup.
        if keypress == KeyPress::named(FenixNamedKey::Escape) && self.standard_keys() {
            self.completion = None;
            return true;
        }
        if self.completion.as_ref().is_some_and(|s| s.picker.is_empty()) { return false; }
        if keypress.code == KeyCode::Named(FenixNamedKey::Enter) && keypress.mods == Mods::default() && self.enter_is_a_new_line() {
            self.completion = None;
            return false;
        }
        self.completion_selected = true;
        match keypress.code {
            KeyCode::Named(FenixNamedKey::PageDown) => self.completion.as_mut().unwrap().picker.move_selection(COMPLETION_MAX_ROWS as isize),
            KeyCode::Named(FenixNamedKey::PageUp) => self.completion.as_mut().unwrap().picker.move_selection(-(COMPLETION_MAX_ROWS as isize)),
            KeyCode::Named(FenixNamedKey::Down) => self.completion.as_mut().unwrap().picker.move_selection(1),
            KeyCode::Named(FenixNamedKey::Up) => self.completion.as_mut().unwrap().picker.move_selection(-1),
            KeyCode::Char('n') if keypress.mods.ctrl => self.completion.as_mut().unwrap().picker.move_selection(1),
            KeyCode::Char('p') if keypress.mods.ctrl => self.completion.as_mut().unwrap().picker.move_selection(-1),
            KeyCode::Named(FenixNamedKey::Tab) | KeyCode::Named(FenixNamedKey::Enter) => self.accept_completion(),
            // Vim's own accept key, beside the Tab/Enter every other
            // editor uses.
            KeyCode::Char('y') if keypress.mods.ctrl => self.accept_completion(),
            // Vim's own dismiss key: closes the popup and leaves you
            // typing. This is the behaviour Escape used to have, and
            // moving it here is the whole point -- see below.
            KeyCode::Char('e') if keypress.mods.ctrl => self.completion = None,
            // Escape is deliberately not claimed. It used to be, closing
            // the popup and leaving you in Insert -- so getting back to
            // Normal took two presses, and which one you needed depended
            // on whether a popup you may not even have been looking at
            // happened to be open. That is the VS Code reflex, and it
            // does not transfer: there, Escape has nothing to do *but*
            // dismiss things, so spending it on the popup costs nothing.
            // In a modal editor it costs the one key the whole grammar
            // rests on.
            //
            // Real Vim agrees, and `:h popupmenu-keys` is explicit: it
            // lists every key with a special meaning while the menu is
            // up -- CTRL-Y, CTRL-E, the arrows, Tab, Space -- and Escape
            // is not among them. It keeps its ordinary Insert-mode
            // meaning, and the popup closing is a consequence of leaving
            // Insert rather than something Escape was spent on. Falling
            // through is all that takes here too: `sync_completion` runs
            // after every key that reaches Vim and drops the popup the
            // moment the mode stops being Insert.
            _ => return false, // plain typing/Backspace/arrows/Escape
        }
        true
    }

    /// Force-opens the completion popup at the current (possibly empty)
    /// prefix, bypassing the normal ">=1 char" auto-trigger threshold --
    /// `Ctrl-Space`'s effect, the near-universal manual-trigger
    /// convention.
    /// The character just before `at`, when an attached server declared
    /// it a completion trigger character.
    pub(super) fn completion_trigger_before(&self, at: usize) -> Option<char> {
        let c = at.checked_sub(1).and_then(|i| self.open().buffer.char_at(i))?;
        let (keys, _, _) = self.focused_lsp_context()?;
        keys.iter()
            .any(|key| self.lsp_sessions.get(key).and_then(|s| s.capabilities.as_ref()).is_some_and(|caps| fenix_lsp::fanout::declares_trigger(caps, c)))
            .then_some(c)
    }

    pub(super) fn completion_context_valid(&self) -> bool {
        self.completion_context.is_none_or(|context| context ==
            (self.focused_buffer_id(), self.focused_pane_id(), self.cursor().char_idx, self.open().buffer.edit_count()))
    }

    pub(super) fn force_open_completion(&mut self) {
        self.completion_selected = false;
        self.completion_asked = true;
        self.completion_context = Some((self.focused_buffer_id(), self.focused_pane_id(), self.cursor().char_idx, self.open().buffer.edit_count()));
        let (prefix_start, prefix, candidates) = match self.completion_at_cursor() {
            Some(found) => found,
            None => (self.cursor().char_idx, String::new(), self.completion_candidates()),
        };
        let mut picker = fenix_picker::PickerState::new(candidates);
        picker.set_query(&prefix);
        let lsp_attached = self.focused_lsp_context().is_some();
        self.completion_local = true;
        self.completion =
            if picker.is_empty() && !lsp_attached { None } else { Some(CompletionState { prefix_start, picker }) };
        if lsp_attached {
            self.request_lsp_completion(prefix_start, None);
        }
        self.completion_scroll = 0;
    }

    /// What the popup would complete at the cursor right now: the
    /// prefix's start, the prefix itself, and the pool to filter with
    /// it. A feature's own completion at the cursor first (the
    /// notebook's names after `[[`), else the identifier prefix and the
    /// ordinary `completion_candidates` pool. `None` when there's
    /// nothing to complete -- `sync_completion` closes the popup then,
    /// while `force_open_completion` (`Ctrl-Space`) falls back to the
    /// whole pool on an empty prefix. A language server's own items
    /// (Tcl's options and subcommands, from the Tcl extension) join
    /// these when they come.
    pub(super) fn completion_at_cursor(&mut self) -> Option<(usize, String, Vec<fenix_picker::Candidate<completion::Item>>)> {
        let cursor = self.cursor();
        let (start, prefix) = completion::prefix_at_cursor(&self.open().buffer, &cursor)?;
        Some((start, prefix, self.completion_candidates()))
    }

    /// The completion candidate pool for whatever buffer is focused right
    /// now: buffer-word candidates (`completion::buffer_words`), the
    /// words of your `completion.symbols_file`, the snippets, and the
    /// features' own (the MIB's names) -- the same in any language.
    /// Shared by `sync_completion`/`force_open_completion` so the two
    /// ways of opening the popup never drift out of sync with each
    /// other.
    pub(super) fn completion_candidates(&mut self) -> Vec<fenix_picker::Candidate<completion::Item>> {
        let mut candidates = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut words: Vec<_> = completion::buffer_words_besides_cursor(&self.open().buffer, &self.cursor()).into_iter().collect();
        words.sort();
        for word in words {
            if seen.insert(word.clone()) {
                let item = completion::Item::text(word.clone(), completion::Source::Buffer);
                candidates.push(fenix_picker::Candidate::new(word, item));
            }
        }
        if let Some(file) = &self.config.completion_symbols_file {
            for item in completion::symbols_file(file) {
                if seen.insert(item.label.clone()) {
                    candidates.push(fenix_picker::Candidate::new(item.label.clone(), completion::Item::from(item)));
                }
            }
        }
        let catalog = self.snippet_catalog();
        for snippet in catalog.available(&self.snippet_scope()) {
            candidates.push(fenix_picker::Candidate::new(snippet.trigger.clone(), completion::Item::snippet(snippet)));
        }
        candidates
    }

    /// Replaces the typed prefix with the selected candidate's full
    /// label -- one atomic undo step via `Buffer::replace_range`.
    pub(super) fn accept_completion(&mut self) {
        if !self.completion_context_valid() { self.completion = None; return; }
        let Some(state) = self.completion.take() else { return };
        let Some(item) = state.picker.selected().map(|c| c.payload.clone()) else { return };
        match item.insertion {
            completion::Insertion::Snippet(template) => self.expand_snippet_template(state.prefix_start, template),
            completion::Insertion::Text(text) => {
                let (buffer, cursor) = self.focused_buffer_and_cursor_mut();
                buffer.replace_range(cursor, state.prefix_start, cursor.char_idx, &text);
            }
            completion::Insertion::Lsp(item) => {
                let (buffer, cursor) = self.focused_buffer_and_cursor_mut();
                if let Err(error) = completion::apply_lsp(buffer, cursor, state.prefix_start..cursor.char_idx, &item) { self.set_error(error); }
            }
        }
    }

    pub(super) fn completion_popup(
        &self,
        window_width: f32,
        modeline_top: f32,
        focused_rect: fenix_window::Rect,
        focused_caret: Option<(usize, usize)>,
        gutter_px: f32,
        content_frac: f32,
    ) -> Option<(fenix_window::Rect, RowSpans, Option<usize>)> {
        let state = self.completion.as_ref()?;
        let (row, col) = focused_caret?;
        let (char_width, line_height) = match &self.text {
            Some(text) => (text.char_width(), text.line_height()),
            None => (text::CHAR_WIDTH, text::LINE_HEIGHT),
        };
        let (caret_x, caret_y) = caret_pixel_pos(focused_rect, row, col, gutter_px, content_frac, char_width, line_height);

        let available = popup::max_rows(modeline_top, COMPLETION_MARGIN, line_height, COMPLETION_PADDING);
        if available == 0 || state.picker.is_empty() { return None; }
        let selected = state.picker.selected()?;
        // Keep the list and the selected item's explanation visually
        // distinct. A completion popup is navigated as a list first; raw
        // LSP detail/documentation appended as anonymous rows made it look
        // like extra candidates and pushed useful choices off screen.
        let mut extra: Vec<(String, glyphon::Color)> = Vec::new();
        if !selected.payload.detail.is_empty() {
            extra.push((format!("DETAIL  {}", completion::clipped_line(&selected.payload.detail, 72)), self.theme.caret_text));
        }
        if !selected.payload.documentation.is_empty() {
            for (index, line) in selected.payload.documentation.lines().filter(|line| !line.trim().is_empty()).take(2).enumerate() {
                let prefix = if index == 0 { "DOCS    " } else { "        " };
                extra.push((format!("{prefix}{}", completion::clipped_line(line, 72)), self.theme.fg_modeline));
            }
        }
        let close = if self.standard_keys() { "Esc" } else { "C-e" };
        extra.push((format!("{}/{}  ↑↓ choose · Tab/Enter accept · {close} close", state.picker.selected_row() + 1, state.picker.len()), self.theme.gutter_fg));
        extra.truncate(available.saturating_sub(1).min(4));
        let shown_rows = (available - extra.len()).min(COMPLETION_MAX_ROWS);
        let offset = completion::scroll_offset(state.picker.selected_row(), self.completion_scroll, shown_rows);
        let rows: Vec<_> = state.picker.visible_rows(offset, shown_rows).collect();
        // Sized to its text up to the window, not to `WHICH_KEY_MAX_
        // WIDTH` -- same reasoning as `prompt_popup`: that cap is tuned
        // for short which-key labels, while the DETAIL row here carries
        // a full usage line (`tcl::signature`, up to 72 columns) that
        // would otherwise be cut off at any font size above the default.
        let max_width = (window_width - 2.0 * COMPLETION_MARGIN).max(1.0);
        let longest = rows.iter().map(|(_, c)| c.label.chars().count() + 10)
            .chain(extra.iter().map(|(s, _)| s.chars().count().min(72))).max().unwrap_or(20);
        let width = (longest as f32 * char_width + COMPLETION_PADDING).max(text::WHICH_KEY_MIN_WIDTH).min(max_width);
        let columns = ((width - COMPLETION_PADDING).max(0.0) / char_width) as usize;
        let label_columns = columns.saturating_sub(10);
        let theme = self.theme;
        let mut spans = Vec::new();
        let mut selected_row = None;
        for (i, (is_selected, candidate)) in rows.iter().enumerate() {
            if i > 0 { spans.push(("\n".into(), theme.fg_modeline, false)); }
            if *is_selected { selected_row = Some(i); }
            let color = match candidate.payload.source {
                completion::Source::Keyword | completion::Source::Snippet => theme.caret_text,
                _ => theme.fg_modeline,
            };
            let label = completion::clipped_line(&candidate.payload.label, label_columns);
            let padding = label_columns.saturating_sub(label.chars().count()) + 2;
            spans.push((label, color, false));
            spans.push((format!("{}{}", " ".repeat(padding), completion::clipped_line(candidate.payload.source_label(), columns.saturating_sub(label_columns + 2))), theme.fg_modeline, false));
        }
        for (line, color) in &extra {
            spans.push(("\n".into(), theme.fg_modeline, false));
            spans.push((completion::clipped_line(line, columns), *color, false));
        }
        let height = (rows.len() + extra.len()) as f32 * line_height + COMPLETION_PADDING;
        let rect = popup::resolve(popup::Anchor::BelowPoint { x: caret_x, y: caret_y + line_height }, width, height, window_width, modeline_top);
        Some((rect, spans, selected_row))
    }
}
