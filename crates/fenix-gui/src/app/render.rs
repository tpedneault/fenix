//! Drawing a frame: the panes, the modeline, the popups and the tab strip.
//!
//! Methods of `App` moved here from `app.rs` as they were.

use super::*;

impl App {
    /// (mode label, rest-of-modeline suffix) -- always returns real
    /// content now, even while a capturing prompt (`:command`, `/`
    /// search, a git commit prompt, ...) is active: that text lives in
    /// its own floating popup now (`App::prompt_popup`), so the
    /// modeline itself just keeps showing its ordinary badge/filename/
    /// position underneath it, the same "additive, doesn't replace the
    /// real indicator" posture `recording_indicator`/`workspace_
    /// indicator` already have below.
    /// The modeline's badge and text, its keys said in the scheme in use
    /// (a Git operation's `SPC g x c continue`, an explorer job's stop).
    pub(super) fn modeline_pieces(&self) -> (&'static str, String) {
        let (badge, text) = self.modeline_pieces_as_written();
        (badge, self.keys_text(&text))
    }

    pub(super) fn modeline_pieces_as_written(&self) -> (&'static str, String) {
        // A live status/error message pre-empts even the Explorer/Picker
        // badge -- it's telling the user something just happened,
        // regardless of what view they're in. Uses the plain mode badge
        // (Normal/Insert/...) rather than Explorer's/Picker's own,
        // matching the "additive, doesn't replace the real mode
        // indicator" posture `recording_indicator`/`workspace_indicator`
        // already have below.
        if let Some(msg) = &self.status_message {
            if msg.set_at.elapsed() < MESSAGE_DURATION {
                return (self.mode_badge_label(), format!("{} ", msg.text));
            }
        }
        if let Some(target) = self.active_explorer_target() {
            // Only the picker has a purpose; a pane's listing and the
            // sidebar are browsing.
            let purpose = if target == ExplorerTarget::Overlay { self.explorer_purpose } else { ExplorerPurpose::Browse };
            let suffix = match self.explorer_state(target) {
                Some(explorer) => {
                    let marked =
                        if explorer.marks.is_empty() { String::new() } else { format!(" [{} marked]", explorer.marks.len()) };
                    match purpose {
                        ExplorerPurpose::Browse => {
                            format!("{}{marked}   {} items ", fenix_vfs::display(&explorer.cwd), explorer.entries.len())
                        }
                        ExplorerPurpose::PickProjectDir => {
                            format!("{}   S to add as a project, q to cancel ", fenix_vfs::display(&explorer.cwd))
                        }
                        ExplorerPurpose::PickWizardParent => {
                            format!("{}   S to create the project here, q to go back ", fenix_vfs::display(&explorer.cwd))
                        }
                        ExplorerPurpose::PickSettingPath { folder: true } => {
                            format!("{}   S to pick this folder, q to go back ", fenix_vfs::display(&explorer.cwd))
                        }
                        ExplorerPurpose::PickSettingPath { folder: false } => {
                            format!("{}   Enter to pick a file, S this folder, q to go back ", fenix_vfs::display(&explorer.cwd))
                        }
                        ExplorerPurpose::FindFrom => {
                            format!("{}{marked}   Enter to open, S to search here, q to cancel ", fenix_vfs::display(&explorer.cwd))
                        }
                        ExplorerPurpose::PickRemoteProject => {
                            format!("{}   S to open this folder as a project, q to cancel ", fenix_vfs::display(&explorer.cwd))
                        }
                    }
                }
                None => String::new(),
            };
            let badge = match purpose {
                ExplorerPurpose::Browse => "EXPLORE",
                ExplorerPurpose::PickProjectDir => "ADDPROJ",
                ExplorerPurpose::PickWizardParent => "NEWIN",
                ExplorerPurpose::PickSettingPath { .. } => "PICK",
                ExplorerPurpose::FindFrom => "FINDFROM",
                ExplorerPurpose::PickRemoteProject => "REMOTE",
            };
            return (badge, suffix);
        }
        if self.main_view == MainView::Picker {
            let (label, count) = match &self.active_picker {
                Some(picker @ ActivePicker::FindFile(_)) => ("FINDFILE", picker_len(picker)),
                Some(picker @ (ActivePicker::BufferTodos(_) | ActivePicker::ProjectTodos(_))) => ("TODOS", picker_len(picker)),
                Some(picker @ ActivePicker::Problems(_)) => ("PROBLEMS", picker_len(picker)),
                Some(picker @ ActivePicker::Lenses(_)) => ("LENSES", picker_len(picker)),
                Some(picker @ ActivePicker::ProjectSymbols(_)) => ("SYMBOLS", picker_len(picker)),
                Some(picker @ ActivePicker::SwitchProject(_)) => ("SWPROJ", picker_len(picker)),
                Some(picker @ ActivePicker::SwitchBuffer(_)) => ("SWBUF", picker_len(picker)),
                Some(picker @ ActivePicker::Recovery(_)) => ("RECOVER", picker_len(picker)),
                Some(picker @ ActivePicker::Places(_)) => ("PLACES", picker_len(picker)),
                Some(picker @ ActivePicker::Agents(_)) => ("AGENTS", picker_len(picker)),
                Some(picker @ ActivePicker::DeleteProject(_)) => ("DELPROJ", picker_len(picker)),
                Some(picker @ ActivePicker::RemoteHost(_)) => ("REMOTE", picker_len(picker)),
                Some(picker @ ActivePicker::Theme(_)) => ("THEME", picker_len(picker)),
                Some(picker @ ActivePicker::Encoding(_)) => ("ENCODING", picker_len(picker)),
                Some(picker @ ActivePicker::Commands(_)) => (if self.palette_describes { "DESCRIBE" } else { "COMMAND" }, picker_len(picker)),
                Some(picker @ ActivePicker::Bindings(_)) => ("KEYS", picker_len(picker)),
                Some(picker @ ActivePicker::SettingChoice { .. }) => ("SETTING", picker_len(picker)),
                Some(picker @ ActivePicker::Snippet(_)) => ("SNIPPET", picker_len(picker)),
                Some(picker @ ActivePicker::Symbol(_)) => ("SYMBOL", picker_len(picker)),
                Some(picker @ ActivePicker::TableColumn(_)) => ("COLUMN", picker_len(picker)),
                Some(picker @ ActivePicker::BufferSearch(_)) => ("SEARCH", picker_len(picker)),
                Some(picker @ ActivePicker::SwitchWorkspace(_)) => ("SWWS", picker_len(picker)),
                Some(picker @ ActivePicker::WorkspaceLauncher(_)) => ("WORKSPACE", picker_len(picker)),
                Some(picker @ ActivePicker::Outline(_)) => ("OUTLINE", picker_len(picker)),
                Some(picker @ ActivePicker::Task(_)) => ("TASK", picker_len(picker)),
                Some(picker @ ActivePicker::Launch(_)) => ("LAUNCH", picker_len(picker)),
                Some(picker @ ActivePicker::CompareTarget(_)) => ("COMPARE", picker_len(picker)),
                Some(picker @ ActivePicker::RebaseOnto(_)) => ("REBASE ONTO", picker_len(picker)),
                Some(picker @ ActivePicker::MergeFrom(_)) => ("MERGE", picker_len(picker)),
                Some(picker @ ActivePicker::SwitchBranch(_)) => ("SWITCH", picker_len(picker)),
                Some(picker @ ActivePicker::CompareOwn { .. }) => ("COMPARE", picker_len(picker)),
                Some(picker @ ActivePicker::Versus { .. }) => ("BESIDE", picker_len(picker)),
                Some(picker @ ActivePicker::Code { badge, .. }) => (*badge, picker_len(picker)),
                None => ("PICKER", 0),
            };
            // Which half of a two-step comparison you're on, spelled
            // out. "base" and "head" alone don't say which direction the
            // result reads in, and picking them the wrong way round
            // silently answers the opposite question.
            let suffix = match &self.active_picker {
                Some(ActivePicker::CompareTarget(_)) => format!("{count} refs   compare HEAD with… "),
                Some(ActivePicker::CompareOwn { target, .. }) => format!("{count} refs   {target}...?   pick your side "),
                Some(ActivePicker::Versus { path, .. }) => {
                    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    format!("{count} refs   set {name} beside its version on… ")
                }
                Some(ActivePicker::FindFile(_)) => match self.file_listing_note() {
                    Some(note) => format!("{count} matches   {note} "),
                    None => format!("{count} matches "),
                },
                _ => format!("{count} matches "),
            };
            return (label, suffix);
        }
        let ob = self.open();
        // A page (Home, the Log, Settings...) by its tab's plain name, and
        // with no line and column: it has no caret to have them.
        let id = self.focused_buffer_id();
        let on_page = ob.kind == BufferKind::Dashboard || self.is_page_buffer(id);
        let filename = if on_page { self.tab_label(id) } else { tabs::plain_name(&self.buffer_display_name(id)) };
        let modified = if ob.kind.tracks_unsaved_changes() && ob.buffer.is_dirty() { " [+]" } else { "" };
        // A file that moved under an unsaved buffer is worth seeing
        // without pressing anything: the message that announced it
        // scrolls away, and the next `:w` refusing out of nowhere is a
        // worse way to find out.
        let diverged = if self.externally_changed.contains(&self.focused_buffer_id()) { " [disk]" } else { "" };
        let (line, col) = ob.buffer.line_col(&self.cursor());
        let recovery = if self.recovery_failures.is_empty() { "" } else { " [recovery failed]" };
        let session = if self.session.error.is_some() { " [session failed]" } else { "" };
        let modified = format!("{modified}{diverged}{recovery}{session}");
        let mode_label = self.mode_badge_label();
        // Only shown once there's more than one workspace to distinguish --
        // stays out of the way for the common single-workspace case.
        let workspace_indicator = if self.workspaces.len() > 1 {
            format!("   [{}/{} {}]", self.workspaces.active_index() + 1, self.workspaces.len(), self.workspaces.active_name())
        } else {
            String::new()
        };
        // Additive, not a full modeline override like a capturing prompt
        // (`explorer_prompt_text`, ...) -- recording doesn't block
        // ordinary editing/redraws, so the rest of the modeline should
        // keep showing real information, same posture as `workspace_
        // indicator`.
        let recording_indicator =
            match self.vim.recording_register() {
                _ if !self.extra_carets().is_empty() => format!("   {} cursors", self.extra_carets().len() + 1),
                // Standard keys: which chord opens the leader, until it's
                // been used a few times (`keys.leader_hint`).
                None if self.standard_keys() && self.leader_uses < 3 && self.config.bool_value("keys.leader_hint") != Some(false) => format!("   {} commands", self.leader_chord_text()),
                Some(reg) => format!("   recording @{reg}"),
                None => String::new(),
            };
        // Same additive posture as `recording_indicator` -- the agenda's
        // running timer is easy to forget about once you've actually gone
        // and started editing the code the task is about, which is
        // exactly when this needs to stay visible rather than only
        // showing while the agenda buffer itself is focused.


        // Error/warning counts for whatever the focused buffer's own
        // language server last published for its path, if any -- the
        // simplest possible "is anything actually wrong here" signal;
        // inline squiggle-equivalent markup and a real diagnostics list
        // (`SPC c d`) are a follow-up, this is deliberately the smallest
        // slice that's still genuinely useful on its own. Canonicalizes
        // the path on every call (matching how `apply_lsp_diagnostics`
        // keys the map) rather than caching -- a plain `stat`, not worth
        // the extra bookkeeping unless it actually shows up as slow.
        let diagnostics_indicator = ob
            .buffer
            .path()
            .map(|p| fenix_lsp::normalize(std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())))
            .map(|p| self.all_diagnostics(&p))
            .filter(|diags| !diags.is_empty())
            .map(|diags| {
                let errors = diags.iter().filter(|d| d.severity == Some(lsp_types::DiagnosticSeverity::ERROR)).count();
                let warnings = diags.iter().filter(|d| d.severity == Some(lsp_types::DiagnosticSeverity::WARNING)).count();
                format!("   {errors}E {warnings}W")
            })
            .unwrap_or_default();
        let details = self.editor_status_details();
        // The sketch's board and port -- `refresh_embedded_indicator`
        // keeps this current for whatever buffer is focused.
        let feature_indicators: String = self.code_statuses().concat();
        let feature_indicators = feature_indicators + &self.agents_status_segment() + &self.agents_usage_segment() + &self.tests_segment() + &self.notifications_segment();
        let git_indicator = self.chrome_git_segment();
        let remote_indicator = self.remote_indicator();
        let search_indicator = match self.search_count() {
            Some((_, 0)) | None => String::new(),
            Some((i, n)) => format!("   [{i}/{n}]"),
        };
        let debug_indicator = self.debug_modeline_segment();
        // How to get the keyboard back, while a shell has it (#42).
        let terminal_hint = match self.terminal_focused.then(|| self.terminal_title(terminal_mouse::TermTarget::Panel)).flatten() {
            // The panel has no title of its own: what it runs is here.
            Some(what) => format!("   {what}   C-\\ back to the editor"),
            None if self.terminal_has_keyboard() => "   C-\\ back to the editor".to_string(),
            None => String::new(),
        };
        let place = if on_page { String::new() } else { format!("   Ln {}, Col {}", line + 1, col + 1) };
        let suffix = format!(
            "{filename}{modified}{terminal_hint}{workspace_indicator}{remote_indicator}{git_indicator}{recording_indicator}{feature_indicators}{diagnostics_indicator}{search_indicator}{debug_indicator}{place}   {details} "
        );
        (mode_label, suffix)
    }

    /// Test-only: the modeline as one plain string, for assertions --
    /// `redraw` renders it as separately-colored spans instead (see
    /// `modeline_pieces`/`mode_colors`), so nothing else needs this.
    /// Always the modeline's own ordinary badge/filename/position now --
    /// a capturing prompt's own text lives in `prompt_popup`/`active_
    /// prompt_text` instead, asserted on separately.
    #[cfg(test)]
    pub(super) fn modeline_text(&self) -> String {
        let (mode_label, suffix) = self.modeline_pieces();
        format!("{}{suffix}", modeline_mode_segment(mode_label))
    }

    /// Drains buffer `id`'s edits since the last frame, feeds them through
    /// its active language's incremental parser (if any), and returns
    /// resolved syntax-highlight colors for the visible byte range only --
    /// same windowing discipline `Buffer::visible_text` already uses.
    /// Always drains, even with no active language, so the buffer's edit
    /// log doesn't grow unbounded for files nothing is consuming it for.
    /// Takes an explicit `id` (rather than always the focused buffer) so
    /// every visible pane's buffer gets reparsed, not just the focused
    /// one's -- `self.buffers.get_mut(id)` is a direct field projection,
    /// so this can still read `self.theme` first in the same call without
    /// the opaque-method-call borrow conflict `open_mut()` would risk.
    pub(super) fn syntax_highlights_for_visible_range(
        &mut self,
        id: BufferId,
        render_base_line: usize,
        rows: usize,
    ) -> Vec<(std::ops::Range<usize>, glyphon::Color)> {
        self.syntax_highlights_from_column(id, render_base_line, rows, 0)
    }

    /// `syntax_highlights_for_visible_range`, for rows scrolled sideways to
    /// `scroll_col`: on a very long line, only what those rows will show
    /// is highlighted. A line with tabs as well may lose its colors far
    /// along it, `scroll_col` being counted in visual columns.
    pub(super) fn syntax_highlights_from_column(
        &mut self,
        id: BufferId,
        render_base_line: usize,
        rows: usize,
        scroll_col: usize,
    ) -> Vec<(std::ops::Range<usize>, glyphon::Color)> {
        self.syntax_highlights_in_columns(id, render_base_line, rows, scroll_col, None, false)
    }

    /// `syntax_highlights_from_column`, with the columns of each line a
    /// wrapped pane's rows show (`columns`, by line: from, to) in place of
    /// the sideways scroll's for a very long line. `viewport`: what a pane
    /// shows, drawn (not a header or a line looked up on its own) -- what
    /// its underlines are worked out against.
    pub(super) fn syntax_highlights_in_columns(
        &mut self,
        id: BufferId,
        render_base_line: usize,
        rows: usize,
        scroll_col: usize,
        columns: Option<&HashMap<usize, (usize, usize)>>,
        viewport: bool,
    ) -> Vec<(std::ops::Range<usize>, glyphon::Color)> {
        let _profile = crate::profile::Scope::new("syntax highlights");
        let theme = self.theme;

        // Which words in command position are real calls, where the
        // grammar captures every one (Tcl).
        self.ask_workspace_names(id);
        let known_commands = self.known_commands(id);

        self.todo_spans.remove(&id);
        // Whether a language server's tokens are laid over the grammar's:
        // nothing's spent on them when not.
        let semantic = self.semantic_to_draw(id);
        let Some(ob) = self.buffers.get_mut(id) else { return Vec::new() };
        let deltas = ob.buffer.drain_edits();

        if ob.kind == BufferKind::Dashboard {
            return self.home_highlights(id, render_base_line, rows);
        }
        if ob.kind == BufferKind::Page {
            return self.page_highlights(id, render_base_line, rows);
        }
        if ob.kind == BufferKind::Git {
            return git_highlights_for_visible_range(ob, self.git_lines.get(&id).map(Vec::as_slice), render_base_line, rows, theme);
        }
        if ob.kind == BufferKind::Diff {
            return diff_highlights_for_visible_range(ob, self.diff_lines.get(&id).map(Vec::as_slice), render_base_line, rows, theme);
        }
        if ob.kind == BufferKind::Graph {
            return graph_highlights_for_visible_range(ob, self.graph_lines.get(&id).map(Vec::as_slice), render_base_line, rows, theme);
        }
        if ob.kind == BufferKind::Merge {
            return merge_highlights_for_visible_range(ob, self.merge_lines.get(&id).map(Vec::as_slice), render_base_line, rows, theme);
        }

        // Conflict markers, in an ordinary file. A conflicted file opens
        // as plain text -- the markers are just lines, and tree-sitter
        // sees them as broken syntax, so the three regions it matters
        // most to tell apart render identically to everything else.
        // Colored ahead of syntax rather than merged with it because the
        // two are answering different questions, and while a file has
        // `<<<<<<<` in it the conflict is the only one worth answering.
        let conflict_ranges = conflict_marker_highlights(ob, render_base_line, rows, theme);
        if !conflict_ranges.is_empty() {
            return conflict_ranges;
        }

        let Some(syntax) = &mut ob.syntax else {
            // No grammar: a language server's tokens are all the colour
            // there is.
            if !semantic {
                return Vec::new();
            }
            return match highlight_windows(&ob.buffer, render_base_line, rows, scroll_col, columns) {
                Some(windows) => self.with_semantic_tokens(id, Vec::new(), &windows, &[]),
                None => Vec::new(),
            };
        };
        let windows = highlight_windows(&ob.buffer, render_base_line, rows, scroll_col, columns);

        let edits: Vec<fenix_syntax::RawEdit> = deltas
            .into_iter()
            .map(|d| fenix_syntax::RawEdit { start_char: d.start_char, new_end_char: d.new_end_char, removed: d.removed })
            .collect();
        let source = ob.buffer.text();
        syntax.apply_edits(&source, &edits);

        let Some(byte_ranges) = windows else { return Vec::new() };
        let syntax = &*syntax;
        // Kept for a language server's tokens, when there are some to lay.
        let windows = if semantic { byte_ranges.clone() } else { Vec::new() };
        let highlights: Vec<(std::ops::Range<usize>, &str)> = byte_ranges
            .into_iter()
            .flat_map(|window| {
                let spans = syntax.highlights_in_range(&source, window.clone());
                spans.into_iter().map(move |(range, name)| (range.start.max(window.start)..range.end.min(window.end), name))
            })
            .filter(|(range, name)| {
                // Only the generic, unvalidated "function" capture (any
                // bare word in command position) needs cross-referencing
                // -- every other Tcl capture (specific builtins, "proc",
                // keywords, the switch/unset/variable special cases, ...)
                // is already validated by the query's own static
                // `#any-of?` lists, so it's left alone here.
                let Some(known) = &known_commands else { return true };
                if *name != "function" {
                    return true;
                }
                let text = &source[range.clone()];
                known.contains(text.strip_prefix("::").unwrap_or(text))
            })
            .collect();
        let todos: Vec<_> = highlights
            .iter()
            .filter_map(|(range, name)| fenix_syntax::TodoKind::from_capture_name(name).map(|kind| (range.clone(), kind)))
            .collect();
        // What the grammar says is a comment or a string: a language
        // server's tokens aren't laid there.
        let protected: Vec<std::ops::Range<usize>> = if semantic {
            highlights.iter().filter(|(_, name)| matches!(name.split('.').next(), Some("comment" | "string" | "spell"))).map(|(range, _)| range.clone()).collect()
        } else {
            Vec::new()
        };
        let colored = highlights.into_iter().map(|(range, name)| (range, theme.syntax_color(name))).collect();
        if !todos.is_empty() {
            self.todo_spans.insert(id, todos);
        }
        if !semantic {
            return colored;
        }
        let laid = self.with_semantic_tokens(id, colored, &windows, &protected);
        // Kept for the underlines drawn over this pane's text, right after
        // (`polish_pane`): none where a token's colour isn't laid either.
        if viewport {
            self.semantic_protected.insert(id, protected);
        }
        laid
    }

    pub(super) fn prompt_popup(&self, window_width: f32, modeline_top: f32) -> Option<(fenix_window::Rect, RowSpans)> {
        let content = self.active_prompt_text()?;

        let (char_width, line_height) = match &self.text {
            Some(text) => (text.char_width(), text.line_height()),
            None => (text::CHAR_WIDTH, text::LINE_HEIGHT),
        };

        let content_chars = content.chars().count();
        let max_width = (window_width - 2.0 * text::WHICH_KEY_MARGIN).max(text::WHICH_KEY_MIN_WIDTH);
        let width = (content_chars as f32 * char_width + WHICH_KEY_PADDING).clamp(text::WHICH_KEY_MIN_WIDTH, max_width);
        let height = line_height + WHICH_KEY_PADDING;

        let theme = self.theme;
        let spans = vec![(content, theme.fg_modeline, false)];
        let rect = popup::resolve(popup::Anchor::BottomLeft { margin: text::WHICH_KEY_MARGIN }, width, height, window_width, modeline_top);
        Some((rect, spans))
    }
    /// Builds the `K` hover popup -- same `BelowPoint`-anchored-under-
    /// the-caret shape as `completion_popup`, just rendering `self.lsp_
    /// hover`'s plain-text lines instead of a candidate list (no
    /// selection, no scrolling: hover text is short enough in practice
    /// that truncating to `COMPLETION_MAX_ROWS` is enough, matching
    /// `hover_contents_text`'s own "legible enough, not actually
    /// rendered markdown" posture).
    /// The focused page's popup -- a Git page's menu, field or question
    /// -- in the same box every other popup here uses: on the panel
    /// colour, with a border and a shadow. It opens under the row it's
    /// about with its text lined up with the page's, and over the row
    /// instead when there isn't room below, kept inside the pane.
    pub(super) fn page_popup(&self, at: Option<PagePopupAt>, window_width: f32, modeline_top: f32) -> Option<(fenix_window::Rect, RowSpans)> {
        let at = at?;
        let popup = self.pages.get(&at.buffer)?.page.popup.as_ref()?;
        let (char_width, line_height) = match &self.text {
            Some(t) => (t.char_width(), t.line_height()),
            None => (text::CHAR_WIDTH, text::LINE_HEIGHT),
        };
        let theme = self.theme;
        let color = |role: crate::page::Role| match role {
            crate::page::Role::Title | crate::page::Role::Text => theme.fg_modeline,
            crate::page::Role::Muted => theme.gutter_fg,
            crate::page::Role::Accent => theme.caret_text,
            crate::page::Role::Good => theme.git_staged,
            crate::page::Role::Warn => theme.git_modified,
            crate::page::Role::Bad => theme.git_conflicted,
            crate::page::Role::Kind(kind) => projects::kind_color(kind, theme),
            crate::page::Role::Syntax(name) => theme.syntax_color(name),
            crate::page::Role::Rgb(r, g, b) => glyphon::Color::rgb(r, g, b),
            crate::page::Role::Git(status) => theme.git_status_color(status),
        };
        let bottom = modeline_top.min(at.pane.y + at.pane.h);
        let shown = popup::max_rows(bottom - at.pane.y, 0.0, line_height, WHICH_KEY_PADDING).min(popup.rows.len());
        let mut spans: RowSpans = Vec::new();
        for (i, row) in popup.rows[..shown].iter().enumerate() {
            if i > 0 {
                spans.push(("\n".to_string(), theme.fg_modeline, false));
            }
            if row.is_empty() {
                spans.push((" ".to_string(), theme.fg_modeline, false));
            }
            for (piece, role) in row {
                spans.push((piece.clone(), color(*role), false));
            }
        }
        let width = (popup.cols() as f32 * char_width + 2.0 * text::PAD_LEFT).min(window_width);
        let height = shown as f32 * line_height + WHICH_KEY_PADDING;
        popup_beside_row(at, popup.line, popup.col, width, height, bottom, window_width, char_width, line_height).map(|rect| (rect, spans))
    }

    pub(super) fn hover_popup(
        &self,
        window_width: f32,
        modeline_top: f32,
        focused_rect: fenix_window::Rect,
        focused_caret: Option<(usize, usize)>,
        gutter_px: f32,
        content_frac: f32,
    ) -> Option<(fenix_window::Rect, RowSpans)> {
        let (char_width, line_height) = match &self.text {
            Some(t) => (t.char_width(), t.line_height()),
            None => (text::CHAR_WIDTH, text::LINE_HEIGHT),
        };
        // The pointer's: under the glyph it rests on, its problems first.
        let (problems, (caret_x, caret_y)) = match &self.mouse.hover {
            Some(hover) => (hover.problems.as_slice(), (hover.anchor.0, hover.anchor.1 - line_height)),
            None => {
                let (row, col) = focused_caret?;
                (&[][..], caret_pixel_pos(focused_rect, row, col, gutter_px, content_frac, char_width, line_height))
            }
        };
        if self.lsp_hover.is_none() && problems.is_empty() {
            return None;
        }

        let shown_rows = popup::max_rows(modeline_top, COMPLETION_MARGIN, line_height, COMPLETION_PADDING).min(COMPLETION_MAX_ROWS);
        let theme = self.theme;
        let mut lines: Vec<(&str, glyphon::Color)> = problems.iter().map(|(p, error)| (p.as_str(), if *error { theme.git_conflicted } else { theme.git_modified })).collect();
        if let Some(text) = &self.lsp_hover {
            lines.extend(text.lines().map(|l| (l, theme.fg_modeline)));
        }
        lines.truncate(shown_rows);
        if lines.is_empty() {
            return None;
        }

        let mut spans = Vec::new();
        for (i, (line, color)) in lines.iter().enumerate() {
            if i > 0 {
                spans.push(("\n".to_string(), theme.fg_modeline, false));
            }
            spans.push((line.to_string(), *color, false));
        }

        // Sized to its longest line up to the window, not to `WHICH_KEY_
        // MAX_WIDTH` -- see `prompt_popup` for why that cap doesn't apply
        // to free text: a usage line (`tcl::hover_text` wraps its
        // subcommand list at 72 columns) was being cut mid-word.
        let longest = lines.iter().map(|(l, _)| l.chars().count()).max().unwrap_or(0);
        let max_width = (window_width - 2.0 * COMPLETION_MARGIN).max(text::WHICH_KEY_MIN_WIDTH);
        let width = (longest as f32 * char_width + COMPLETION_PADDING).clamp(text::WHICH_KEY_MIN_WIDTH, max_width);
        let height = lines.len() as f32 * line_height + COMPLETION_PADDING;
        let rect =
            popup::resolve(popup::Anchor::BelowPoint { x: caret_x, y: caret_y + line_height }, width, height, window_width, modeline_top);
        Some((rect, spans))
    }

    /// The sidebar/terminal/pane rects a mouse click or wheel notch
    /// needs to hit-test against -- `sidebar_px`/`terminal_h`/
    /// `modeline_top` are handed in already resolved (`redraw` and the
    /// mouse handlers each derive them the same trivial way from
    /// `self.sidebar_open`/`self.main_view`/`self.terminal_open`), so
    /// this only needs to own the one part that actually matters for
    /// staying in sync with what's on screen: `WindowTree::layout`'s
    /// own pane rects, computed from the exact same `pane_area` both
    /// callers would otherwise have to build by hand.
    /// Whether `pane` renders a breadcrumb bar under its title row, and
    /// so has to reserve room for one. True only for an ordinary
    /// file-editing pane on a theme that shows tabs -- exactly the
    /// condition `redraw` builds tab/breadcrumb spans under, kept here
    /// so the geometry and the content can't disagree about it.
    ///
    /// A pane in `pane_titles` is a fixed-purpose panel (a Git or Docker
    /// dashboard section, a terminal, a table view): it has a
    /// descriptive title rather than tabs, and no cursor position to
    /// describe a path to, so a breadcrumb row there would only ever be
    /// blank.
    /// A pane's tab strip, when the theme draws one and the pane isn't a
    /// fixed-purpose panel with a title of its own: the tabs' rects,
    /// which is active, the strip's text, the breadcrumb (for the buffer
    /// and position in `crumbs_at`), each tab's text range and which are
    /// in italics. Computed while `self` is still whole, since it needs
    /// the buffers and workspaces.
    #[allow(clippy::type_complexity)]
    pub(super) fn pane_tab_strip(
        &self,
        pane: fenix_window::WindowId,
        rect: fenix_window::Rect,
        line_height: f32,
        is_focused: bool,
        theme: &Theme,
        crumbs_at: Option<(BufferId, usize)>,
    ) -> (Vec<TabRect>, Vec<bool>, Vec<(String, glyphon::Color, bool)>, Vec<(String, glyphon::Color, bool)>, Vec<(usize, usize, usize, usize)>, Vec<usize>) {
        if self.tab_style().is_none() || self.pane_titles.contains_key(&pane) {
            return (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
        }
            // Tab-strip text renders at `text::TITLE_FONT_SCALE` of the
            // body font (see `TextPipeline::title_metrics`), so its
            // real glyphs are narrower per cell than the body `char_
            // width` used elsewhere in this loop -- laying tabs out
            // with that wider body measurement drifted the background/
            // hit-test geometry further right of the actually-smaller
            // rendered text with every tab.
            let title_char_width =
                self.text.as_ref().map(|t| t.title_char_width()).unwrap_or(text::CHAR_WIDTH * text::TITLE_FONT_SCALE);
            let strip_rect = tab_strip_rect(fenix_window::Rect {
                x: rect.x,
                y: rect.y - pane_chrome_height(line_height, self.pane_has_breadcrumb(pane)),
                w: rect.w,
                h: tab_strip_height(line_height),
            });
            // A pane's list is its complete navigation history, not its
            // visible tabs. Lazily resolve names so repeated redraws
            // while moving do work proportional to tabs that fit, not
            // all files ever visited in that pane.
            let active_buffer = self.windows().content(pane).copied();
            // From the first tab that keeps the active one in view: with
            // more tabs than fit, the strip scrolls to it rather than
            // leaving it past the right edge (#50).
            let order = self.pane_tab_order(pane);
            let start = match active_buffer.and_then(|b| order.iter().position(|&id| id == b)) {
                Some(active) => first_tab_shown(active, strip_rect.w, |i| tab_width(self.tab_label(order[i]).chars().count(), title_char_width)),
                None => 0,
            };
            let tabs = order[start..].iter().map(|&id| (id, self.tab_label(id).chars().count()));
            let layout = pane_tab_layout_iter(strip_rect, title_char_width, tabs);
            let mut spans: Vec<(String, glyphon::Color, bool)> = Vec::new();
            let mut active_flags: Vec<bool> = Vec::new();
            let mut ranges: Vec<(usize, usize, usize, usize)> = Vec::new();
            let mut italic: Vec<usize> = Vec::new();
            let mut byte = 0usize;
            for tab in &layout {
                let is_active = Some(tab.buffer) == active_buffer;
                active_flags.push(is_active);
                let name_color =
                    if is_active { if is_focused { theme.caret_text } else { theme.fg_modeline } } else { theme.gutter_fg };
                // Home: the house, the workspace's name, and no ×
                // -- blanks where it would be, so it measures and
                // lines up like every other tab.
                let is_home = self.workspaces.active_workspace().home == Some(tab.buffer);
                let name = self.tab_label(tab.buffer);
                let icon_ch = if is_home { icon::HOME } else { self.page_tab_icon(tab.buffer).unwrap_or_else(|| icon::navigation_icon_for(&name)) };
                let dirty = self
                    .buffers
                    .get(tab.buffer)
                    .is_some_and(|ob| ob.kind.tracks_unsaved_changes() && ob.buffer.is_dirty());
                // Truncated against `TAB_MAX_NAME_CHARS` directly, and
                // padded up to `TAB_MIN_NAME_CHARS`, rather than
                // derived back out of `tab.body.w`. That round trip
                // (width -> budget -> text) was one half of why the
                // drawn tab never matched its own glyphs: it assumed
                // every rendered cell is `title_char_width` wide, which
                // the icon span in particular is not.
                let truncated = truncate_tab_name(&name, TAB_MAX_NAME_CHARS);
                let label = if truncated.chars().count() < TAB_MIN_NAME_CHARS {
                    format!("{truncated:<width$}", width = TAB_MIN_NAME_CHARS)
                } else {
                    truncated
                };
                // Byte ranges into the concatenated strip string, so
                // `TextPipeline::title_span_bounds` can report where
                // this tab and its close glyph actually landed once
                // shaped. The title buffer holds only the tab row (the
                // breadcrumb bar is its own buffer), so these offsets
                // start at 0 and stay in that string's own space.
                let icon_span = format!(" {icon_ch} ");
                let name_span = format!("{label}{}", if dirty { "*" } else { "" });
                let close_span = if is_home { "   " } else { " × " }.to_string();
                let tab_start = byte;
                byte += icon_span.len() + name_span.len();
                let close_start = byte;
                byte += close_span.len();
                ranges.push((tab_start, byte, close_start, byte));
                spans.push((icon_span, theme.icon_file, true));
                if self.is_preview_tab(pane, tab.buffer) {
                    italic.push(spans.len());
                }
                spans.push((name_span, name_color, false));
                spans.push((close_span, theme.gutter_fg, false));
            }
            // The split button, right after the tabs.
            if !layout.is_empty() {
                spans.push((format!(" {} ", icon::WINDOW), theme.gutter_fg, true));
            }
            // The focused pane's breadcrumb is the Ember Trail's ribbon while
            // there's one.
            let ribbon = if is_focused { self.debug_ribbon_spans() } else { None };
            let crumbs = ribbon.or_else(|| crumbs_at.map(|(buffer, at)| self.breadcrumb_spans(buffer, at))).unwrap_or_default();
            (layout, active_flags, spans, crumbs, ranges, italic)
    }

    pub(super) fn redraw(&mut self) {
        self.follow_outside_edits();
        // Every pane's strip starts with the workspace's Home.
        self.ensure_workspace_home();
        // A pane that left the agents' dock gets its tabs back.
        self.agents_dock_sync();
        // The extensions' decorations, moved with the edits since.
        self.shift_code_decorations();
        // A closed buffer's wrap, turned, goes with it.
        let buffers = &self.buffers;
        self.wrap_overrides.retain(|id, _| buffers.get(*id).is_some());
        // And theirs by rule, from a file's first frame.
        self.apply_decoration_rules();
        // An extension's grammar that didn't load, or its query, said once.
        let problems = fenix_syntax::take_grammar_problems();
        if !problems.is_empty() {
            self.set_error(problems.join("; "));
        }
        let _profile = crate::profile::Scope::new("redraw");
        self.wasm.views_drawn.clear();
        let now = Instant::now();
        let prefs = self.motion();
        let tab_style = self.tab_style();
        // A theme change fades from the old theme's picture: draw that
        // once more, only to copy it, then carry on in the new colours.
        if let Some(old) = self.motion.theme_from.take() {
            if self.gpu.as_ref().is_some_and(|gpu| gpu.can_copy_frames() && gpu.surface.is_some()) {
                let new = self.theme;
                self.theme = old;
                self.motion.capture_frame = true;
                self.redraw();
                self.motion.capture_frame = false;
                self.theme = new;
                self.motion.theme_fade = Some(crate::motion::Anim::new(now, prefs.theme_d));
            }
        }
        // Cheap per-frame check (an integer comparison against `Buffer::
        // edit_count()`, same "cheap no-op most frames" shape as the PDF
        // resize-tracking and VNC resize-debounce loops just below in
        // this same function) for whether the focused buffer's language
        // server needs a `textDocument/didChange` -- editing doesn't
        // otherwise have a dedicated "something changed" hook the way
        // focus changes do (`refresh_project_root`), so this is checked
        // here instead, once per rendered frame -- and only when the
        // buffer, its edits or the servers have moved since the last, as
        // the sync itself copies the text and looks at the disk (#74).
        self.sync_lsp_if_changed();
        self.ask_due_semantic_tokens();
        self.ask_due_lenses();
        self.ask_due_inlay_hints();
        self.sync_window_title();
        // Coalesces however many `WindowEvent::Resized` events landed
        // since the last frame into at most one real swapchain
        // reconfigure -- see `GpuState::pending_resize`'s own doc
        // comment for why that matters (a synchronous, GPU-idle-waiting
        // call on every intermediate size during a drag-resize).
        if let Some(gpu) = &mut self.gpu {
            gpu.apply_pending_resize();
        }
        let Some((window_width, window_height)) = self.gpu.as_ref().map(|gpu| (gpu.size.width as f32, gpu.size.height as f32))
        else {
            return;
        };

        // Resolved once, up front: the active theme's font (so
        // `char_width` below reflects it), the real measured advance
        // width for that font (used for every per-column pixel
        // computation below instead of the fixed-ratio `text::
        // CHAR_WIDTH` constant, which breaks for any font whose advance
        // isn't the constant's assumed ~0.6 em), and the live
        // `line_height`/`modeline_height` (same reasoning: fixed once,
        // now adjustable at runtime via `SPC t =`/`-`/`0`, so every
        // consumer needs the *current* value, not the `text::LINE_HEIGHT`/
        // `MODELINE_HEIGHT` constants those used to be).
        let theme = self.theme;
        // A `config.ini` `font_family` always wins over whatever the
        // active theme names; an unset config falls through to the
        // theme's own choice (`None` for the shipped themes, which
        // `TextPipeline::set_font_family` resolves to the fast
        // concrete-name fallback rather than the slow generic one).
        let font_family = self.config.font_family.as_deref().or(theme.font_family);
        let (char_width, line_height, modeline_height, modeline_char_width) = match &mut self.text {
            Some(text) => {
                text.set_font_family(font_family);
                (text.char_width(), text.line_height(), text.modeline_height(), text.modeline_char_width())
            }
            None => (
                text::CHAR_WIDTH,
                text::LINE_HEIGHT,
                text::LINE_HEIGHT,
                text::CHAR_WIDTH * text::MODELINE_FONT_SCALE,
            ),
        };
        let caret_is_block = self.caret_is_block();
        // Resize the git dashboard's sections to suit whichever one is
        // focused, before anything reads the layout. Done here, once a
        // frame, rather than at each of the many places that can move
        // focus (number keys, Tab, `SPC w` navigation, a mouse click):
        // it's a cheap idempotent no-op whenever focus hasn't actually
        // moved, and this way no focus path can forget to trigger it.
        self.apply_git_accordion();

        // Whether this frame is the one with editor focus, and so the
        // one that draws the single-instance UI: the which-key popup,
        // an open picker, the file explorer, a prompt, a completion or
        // menu popup. All of that lives on `App` rather than per frame
        // (only one window can be typed into, so only one can have
        // raised it), and painting it unconditionally would show the
        // same picker in every open window. A background frame draws
        // its panes, its own sidebar and terminal, and its modeline --
        // nothing else.
        let overlays_here = self.active_frame == self.focused_frame;
        let main_view = if overlays_here { self.main_view } else { MainView::Editor };

        // The sidebar is independent of window splits, and shown only in
        // Editor mode (a file listing beside the picker would just be
        // confusing). It's a single frame-level strip, not per-pane --
        // matches Doom's own treemacs sidebar, which is frame-local too
        // -- drawn from the explorer page laid out at its width.
        self.sidebar_follow();
        self.notice_explorer_growth(now);
        let show_sidebar = self.sidebar_open;
        let sidebar_width = self.sidebar_width;
        let sidebar_px = if show_sidebar { sidebar_width } else { 0.0 };
        if let Some(text) = &mut self.text {
            text.set_sidebar_width(sidebar_width, window_height);
        }

        // The terminal panel: a frame-level strip like the sidebar, but
        // shrinking height instead of width, and -- deliberately unlike
        // the sidebar -- shown regardless of `main_view`. The sidebar's
        // own Editor-only gating exists because a file listing next to
        // *another* file listing (Explorer/Picker's own full-buffer
        // view) is confusing; that reasoning doesn't apply to a
        // terminal, and the user's own request ("under all my existing
        // splits") named no such exception.
        let show_terminal = self.terminal_open;
        let terminal_h = if show_terminal { text::terminal_height(line_height) } else { 0.0 };
        let terminal_render = self.terminal_panel_render();
        let terminal_selection = self.terminal_selection_for(terminal_mouse::TermTarget::Panel);
        let terminal_link = self.terminal_hover_for(terminal_mouse::TermTarget::Panel);
        let dock_h = self.debug_dock_px();
        // The sidebar's rows: as many as fit its panel, above the terminal
        // and the debug dock, which run under it.
        let sidebar_lines = text::lines_that_fit(window_height - modeline_height - terminal_h - dock_h - text::STRIP_INSET * 2.0, line_height);
        let sidebar_render = if show_sidebar { self.sidebar_rows(text::cols_that_fit(sidebar_width - text::STRIP_INSET * 2.0, char_width), sidebar_lines) } else { None };
        let dock_cols = debug_host::debug_dock_cols(window_width, char_width);
        let (debug_dock_render, dock_decor) = match self.debug_dock_shown().then(|| self.debug_dock_layout(dock_cols)) {
            Some((spans, decor)) => (Some(spans), decor),
            None => (None, debug_drawer::DockDecor::default()),
        };
        let debug_drawer_highlight = self.debug_drawer_highlight(dock_cols);

        let modeline_top = window_height - modeline_height;
        let geometry = self.frame_geometry(window_width, sidebar_px, terminal_h, modeline_top);
        let pane_area = geometry.pane_area;
        let layout = self.motion_layout(geometry.panes, pane_area, now);
        let focused_pane = self.windows().focused_id();
        // The agents' prompt is drawn as a box you type in.
        let composer_pane = self.agents_composer_pane();
        let look = self.panel_look();
        let panel_radius = look.radius();
        let keyboard_panel = panels::keyboard_panel(focused_pane, self.terminal_focused && self.terminal_open, self.sidebar_focused, self.debug_drawer_focused());

        // One entry per visible window pane -- built fresh every frame
        // from whatever `WindowTree::layout` currently reports (splits/
        // closes/resizes take effect immediately, no separate relayout
        // step needed). Only the *focused* pane can show the explorer/
        // picker overlay (`main_view`/`active_picker`/`explorer` are a
        // single global "what's the current editing context" concept,
        // same as before splits existed) or Vim-mode decoration
        // (selection highlight, pulse, blinking caret) -- every other
        // pane always renders its own buffer's plain content, still with
        // its own gutter/syntax highlighting/current-line highlight, so
        // splitting to see two files is genuinely useful, just without
        // the "actively being edited" polish on the one you're not in.
        struct PaneRender {
            pane: fenix_window::WindowId,
            rect: fenix_window::Rect,
            /// This pane's title-bar text -- its buffer's display name,
            /// or the Docker panel's own descriptive override.
            title: String,
            spans: RowSpans,
            hl_row: Option<usize>,
            /// Whether `hl_row` should render with `theme.selection`
            /// (real standalone contrast) instead of `theme.hl_line`
            /// (a deliberately subtle tint, per `hl_line`'s own doc
            /// comment, that's fine to be understated because a
            /// blinking caret on the same row already draws the eye).
            /// Explorer/Picker have no caret at all (`caret: None`
            /// below) -- their `hl_row` *is* the only indicator of
            /// where you are, so it needs `selection`'s contrast, the
            /// same reasoning the sidebar's own selected-row highlight
            /// already uses `theme.selection` for. The dashboard also
            /// gets the strong styling: it's Vim-navigable text with a
            /// real caret, but functions as a menu you skim rather than
            /// actively type into, so the same "needs to be seen at a
            /// glance, not just backed up by the caret" reasoning
            /// applies.
            hl_row_strong: bool,
            marked_rows: Vec<usize>,
            /// Whole rows tinted across the pane: a side-by-side's
            /// added, removed and changed lines and its filler rows.
            row_tints: Vec<(usize, [f32; 4])>,
            /// Per-cell background colours, as `(row, start_col,
            /// end_col, rgba)` runs. Empty for every pane but a
            /// terminal's: ordinary buffers get their backgrounds from
            /// the theme, while a shell paints its own, and `RowSpans`
            /// carries only foreground colour.
            colored_bg_segments: Vec<(usize, usize, usize, [f32; 4])>,
            selection_segments: Segments,
            pulse_overlay: Option<(Segments, f32)>,
            bracket_match_segments: Segments,
            /// Standard keys' extra cursors: (row, visual column) each.
            extra_carets: Vec<(usize, usize)>,
            hlsearch_segments: Segments,
            caret: Option<(usize, usize)>,
            content_frac: f32,
            gutter_px: f32,
            /// This pane's own tab strip, when it has one -- empty for
            /// every pane but an ordinary file-editing one on a `show_
            /// tabs` theme (terminal/VNC/PDF/overlay/table panes always
            /// keep the plain single `title` above; tabs make no sense
            /// for a fixed-purpose panel). Precomputed here, not in the
            /// later GPU-borrow-split rendering block, since building it
            /// needs `self.buffer_display_name`/`self.buffers`/`self.
            /// workspaces`, all off-limits once `text`/`bg_rect` hold
            /// exclusive borrows of other `self` fields down there.
            tabs_layout: Vec<TabRect>,
            /// Parallel to `tabs_layout` -- whether each tab is the
            /// pane's currently-active buffer.
            tab_active: Vec<bool>,
            /// The tab strip's own rich-text spans, in the exact left-to-
            /// right order `tabs_layout` lays the tabs out in -- handed
            /// straight to `set_pane_title_rich`.
            tab_spans: Vec<(String, glyphon::Color, bool)>,
            /// Whether this pane renders a breadcrumb bar beneath its
            /// title row, and so reserved room for one
            /// (`App::pane_has_breadcrumb`). Carried on the render
            /// rather than recomputed in the drawing block, both
            /// because `self.pane_titles` is off-limits once `text`/`bg_
            /// rect` hold their borrows, and so the background fill and
            /// the content rect can't disagree about where the chrome
            /// ends.
            has_breadcrumb: bool,
            /// Per tab, `(body_start, body_end, close_start, close_end)`
            /// byte offsets into the concatenated `tab_spans` string.
            /// Parallel to `tabs_layout`. These are what turn the strip's
            /// *shaped* glyphs back into per-tab pixel bounds after
            /// `set_pane_title_rich` runs -- `tabs_layout`'s own rects are
            /// only an allocator's estimate (see `pane_tab_layout_iter`),
            /// and an estimate built from character counts cannot match a
            /// row that mixes the body font with the much wider icon font.
            tab_ranges: Vec<(usize, usize, usize, usize)>,
            /// Which of `tab_spans` are in italics: the preview tab's name.
            tab_italic: Vec<usize>,
            /// The breadcrumb bar's rich-text spans, directly below the
            /// tab strip -- kept as its own field (not appended to `tab_
            /// spans` with a newline) since the two rows now render as
            /// separate `GlyphBuffer`s at separate heights (see `tab_
            /// strip_height`/`breadcrumb_strip_height`), not two lines of
            /// one shared buffer. Empty whenever `tab_spans` is.
            breadcrumb_spans: Vec<(String, glyphon::Color, bool)>,
            /// Inline git gutter marks (`App::gutter_hunks`) currently
            /// visible in this pane -- `(row, kind)`, `row` already
            /// relative to this frame's own `render_base_line` like
            /// `hl_row`, so the later drawing loop needs no buffer/scroll
            /// lookups of its own. Empty for every pane but an ordinary
            /// text buffer's.
            gutter_marks: Vec<(usize, GutterMarkKind)>,
            /// Breakpoints and the debugger's stop beside the rows in view
            /// (`debug_host::DebugMark`), rows as `gutter_marks`'.
            debug_marks: Vec<(usize, debug_host::DebugMark)>,
            /// The Ember Trail's stripe: rows and colours.
            trail_marks: Vec<(usize, [f32; 4])>,
            /// Indentation guide `(row, column)` pairs -- `row` relative
            /// to `render_base_line` like `gutter_marks`, `column` a
            /// display column within the row's own text (0 = the pane's
            /// own content start). Empty for every pane but an ordinary
            /// text buffer's.
            indent_guides: Vec<(usize, usize)>,
            /// A thin left-margin accent bar marking where a Jira comment
            /// or Agenda note starts -- `(row, color)`, `row` relative to
            /// `render_base_line` like `gutter_marks`. Same rendering
            /// role as `gutter_marks`, just not git-diff-shaped: every
            /// row here gets the *same* color (one accent per buffer,
            /// not a per-hunk Added/Modified/Deleted choice), so a plain
            /// color is enough rather than reusing `GutterMarkKind`.
            /// Empty for every other pane kind.
            row_accents: Vec<(usize, [f32; 4])>,
            /// Home only: its focus rail and logo.
            home: home::HomeOverlay,
            /// The animations' marks and the polish -- see `PaneExtras`.
            extra: motion_host::PaneExtras,
        }

        let mut panes_render: Vec<PaneRender> = Vec::with_capacity(layout.len());
        // Every pane this frame showing a code extension's surface: its page
        // buffer, and where it's drawn -- as a textured quad once `gpu`/`surfaces` are available.
        let mut code_surface_panes: Vec<(BufferId, fenix_window::Rect)> = Vec::new();
        // The boxes surfaces tint over themselves (a selection, matches).
        let mut surface_marks: Vec<((f32, f32, f32, f32), [f32; 4])> = Vec::new();
        // Where the focused page is scrolled to, for its popup.
        let mut page_popup_at: Option<PagePopupAt> = None;
        self.side_by_side_frame();
        for (pane, rect) in &layout {
            let (pane, rect) = (*pane, *rect);
            // Looked up early (not just where it's needed for content
            // below) since every pane's title bar needs it too, even a
            // pane currently showing the Explorer/Picker overlay --
            // those overlays never change what buffer the pane maps to,
            // just what's rendered in its content area this frame.
            let buffer_id = *self.windows().content(pane).expect("every pane has a buffer");
            // Recomputed every frame from the buffer's *current* text
            // (cheap: a no-op unless `edit_count()` moved since last
            // time, or this buffer isn't `BufferKind::Table` at all) so
            // the elastic column layout an edit changed shows correctly
            // this same redraw, not one frame late.
            self.sync_table_view(buffer_id);
            // The Docker panel's fixed layout gives its panes their own
            // descriptive titles ("Containers", "Logs", ...) via `pane_
            // titles`, consulted first, then a table view's own column-
            // name header; every other pane is titled with its buffer's
            // own display name, so a split showing two different files
            // can tell them apart at a glance.
            let pane_title = self
                .pane_titles
                .get(&pane)
                .cloned()
                .or_else(|| self.table_view_header(buffer_id))
                // A page by its tab's name (`Settings`, not `*settings*`):
                // a pane too narrow for its tabs shows this instead.
                .or_else(|| self.is_page_buffer(buffer_id).then(|| self.tab_label(buffer_id)))
                .unwrap_or_else(|| self.buffer_display_name(buffer_id));
            // Every pane has a title bar, and an editing pane a
            // breadcrumb bar under it too, so its *content* area is
            // shrunk from the top by whichever of those it actually
            // renders (`pane_chrome_height`). `rect.y + rect.h` (the
            // pane's bottom edge) is unchanged by a top-only shrink, so
            // every existing bit of geometry that keys off it (the
            // divider-line loop, `pane_area`'s own bounds checks) needs
            // no changes at all -- only this local `rect` binding, used
            // for everything downstream in this pane's own block, is
            // adjusted.
            let has_breadcrumb = self.pane_has_breadcrumb(pane);
            let rect = pane_content_rect(rect, line_height, has_breadcrumb);
            let is_focused = pane == focused_pane;
            let pane_visible_lines = text::lines_that_fit(rect.h, line_height);
            // An Explorer/Picker overlay temporarily *replaces* the
            // focused pane's own content for as long as it's open, and
            // the two "this pane's content is a bitmap, not text" blocks
            // below would otherwise claim the pane before either overlay
            // branch further down ever runs -- so a file picker opened
            // over a VNC or PDF pane (splitting off a PDF pane and
            // opening a project file next to it, say) drew nothing at
            // all: an empty pane, no file list, and no way to see what
            // was being typed. Checked here, once, rather than repeated
            // in both blocks' own conditions.
            // The picker floats over the panes as the palette, so it covers
            // none of them.
            let overlay_covers_pane = false;

            // A terminal pane's content is a `vt100` screen grid owned
            // by `terminal_buffers`, not the buffer's own (always empty)
            // rope -- so it renders from that grid, exactly as the popup
            // panel does, rather than through the ordinary buffer path
            // below. Unlike VNC/PDF this is still *text*, so it goes out
            // as an ordinary `PaneRender` with spans; only the
            // background colours need somewhere new to live.
            if self.buffers.get(buffer_id).is_some_and(|ob| ob.kind == BufferKind::Terminal)
                && self.terminal_buffers.contains_key(&buffer_id)
                && !overlay_covers_pane
            {
                // This is the only place a pane's real pixel size is
                // known, so it is also where the shell finds out how big
                // it is: a resize that never reached the PTY would leave
                // the shell wrapping its own output to a width nothing
                // on screen has (see `Terminal::resize`).
                let (want_rows, want_cols) = self.terminal_pane_size(pane);
                if let Some(state) = self.terminal_buffers.get_mut(&buffer_id) {
                    if state.session.screen().size() != (want_rows, want_cols) {
                        let _ = state.session.resize(want_rows, want_cols);
                    }
                }
                let view = self.terminal_buffers.get(&buffer_id).map(|state| self.terminal_row_spans(state.session.screen())).unwrap_or_default();
                let terminal_view::TermView { spans, styles, backgrounds: colored_bg_segments, underlines } = view;
                // The caret is the shell's own cursor, and is drawn only
                // while this terminal actually has the keyboard -- an
                // idle blinking block in a pane that ignores your typing
                // is a lie about where input is going.
                let has_keyboard = self.terminal_buffer_focused == Some(buffer_id) && is_focused;
                let caret = self
                    .terminal_buffers
                    .get(&buffer_id)
                    .filter(|_| has_keyboard)
                    .map(|state| state.session.screen())
                    .filter(|screen| !screen.hide_cursor())
                    .map(|screen| {
                        let (row, col) = screen.cursor_position();
                        (row as usize, col as usize)
                    });
                // Its name, then what it runs and where.
                let title = match self.terminal_title(terminal_mouse::TermTarget::Buffer(buffer_id)) {
                    Some(what) => format!("{} · {what}", pane_title.trim_matches('*')),
                    None => pane_title,
                };
                panes_render.push(PaneRender {
                    pane,
                    rect,
                    title,
                    spans,
                    hl_row: None,
                    hl_row_strong: false,
                    marked_rows: Vec::new(),
                    row_tints: Vec::new(),
                    colored_bg_segments,
                    selection_segments: self.terminal_selection_for(terminal_mouse::TermTarget::Buffer(buffer_id)),
                    pulse_overlay: None,
                    bracket_match_segments: Segments::new(),
                    hlsearch_segments: Segments::new(),
                    extra_carets: Vec::new(),
                    caret,
                    content_frac: 0.0,
                    gutter_px: 0.0,
                    tabs_layout: Vec::new(),
                    tab_active: Vec::new(),
                    tab_spans: Vec::new(),
                    breadcrumb_spans: Vec::new(),
                    tab_ranges: Vec::new(),
                    tab_italic: Vec::new(),
                    has_breadcrumb,
                    gutter_marks: Vec::new(), debug_marks: Vec::new(), trail_marks: Vec::new(),
                    row_accents: Vec::new(),
                    indent_guides: Vec::new(),
                    home: home::HomeOverlay::default(),
                    extra: motion_host::PaneExtras {
                        underlines: underlines.into_iter().chain(self.terminal_hover_for(terminal_mouse::TermTarget::Buffer(buffer_id)).map(|(r, a, b)| (r, a, b, theme.caret))).collect(),
                        span_styles: styles,
                        ..Default::default()
                    },
                });
                continue;
            }

            // A surface's content is pixels its extension streams, not
            // text -- rendered as a separate textured-quad layer (see
            // `code_surface_panes`/`surfaces.draw`'s call site below, after
            // `bg_rect`/`text` are available), regardless of whether
            // this pane is focused (unlike Explorer/Picker, which are
            // per-`MainView` overlays shown only in the focused pane --
            // hence `overlay_covers_pane`). Still gets an ordinary
            // (empty) `PaneRender` so its title bar renders normally and
            // no other pane-bookkeeping in this loop needs a special
            // case for it.
            if self.code_surface(buffer_id).is_some() {
                if overlay_covers_pane {
                    // Falls through to the Explorer/Picker rendering
                    // further down, exactly as before this block existed.
                } else {
                    code_surface_panes.push((buffer_id, rect));
                    surface_marks.extend(self.code_surface_marks(buffer_id, rect));
                    panes_render.push(PaneRender {
                        pane,
                        rect,
                        title: pane_title,
                        spans: Vec::new(),
                        hl_row: None,
                        hl_row_strong: false,
                        marked_rows: Vec::new(),
                        row_tints: Vec::new(),
                        colored_bg_segments: Vec::new(),
                        selection_segments: Segments::new(),
                        pulse_overlay: None,
                        bracket_match_segments: Segments::new(),
                        extra_carets: Vec::new(),
                        hlsearch_segments: Segments::new(),
                        caret: None,
                        content_frac: 0.0,
                        gutter_px: 0.0,
                        tabs_layout: Vec::new(),
                        tab_active: Vec::new(),
                        tab_spans: Vec::new(),
                        breadcrumb_spans: Vec::new(),
                        tab_ranges: Vec::new(),
                        tab_italic: Vec::new(),
                        has_breadcrumb,
                        gutter_marks: Vec::new(), debug_marks: Vec::new(), trail_marks: Vec::new(),
                        row_accents: Vec::new(),
                        indent_guides: Vec::new(),
                        home: home::HomeOverlay::default(),
                        extra: motion_host::PaneExtras::default(),
                    });
                    continue;
                }
            }

            // Plain buffer content -- every pane not currently showing an
            // overlay, focused or not. `buffer_id` was already looked up
            // above, alongside `pane_title`.
            let is_home = self.buffers.get(buffer_id).is_some_and(|ob| ob.kind == BufferKind::Dashboard);
            if is_home {
                if self.home_data.date.is_empty() {
                    self.refresh_home_data(true);
                }
                self.ensure_home_layout(buffer_id, pane, text::cols_that_fit(rect.w, char_width), pane_visible_lines);
            }
            // The wizard's page is drawn the way Home is: laid-out text,
            // its own selection, no caret.
            let is_page = self.is_page_buffer(buffer_id);
            if is_page {
                self.note_page_rows(buffer_id, pane_visible_lines);
                self.ensure_page_layout(buffer_id, pane, text::cols_that_fit(rect.w, char_width));
                self.page_scroll_to_top(buffer_id, pane, pane_visible_lines);
            }
            let is_dashboard = is_home || is_page;
            if !is_focused {
                // A side-by-side pane scrolls with the one beside it.
                self.side_by_side_follow(pane, pane_visible_lines);
                // The Search page's preview, which never had the focus
                // long enough to follow its cursor.
                self.search_reveal(pane, pane_visible_lines);
            }
            if !is_focused && self.reveal_panes.remove(&pane) {
                self.reveal_cursor(pane, buffer_id, pane_visible_lines);
            }
            // The columns of text it has room for, which its lines wrap at
            // if they do; a wrapped pane never scrolls sideways.
            // Recomputes `gutter_chars`/`gutter_px` redundantly rather
            // than reordering the real computation below (`gutter_px`
            // there also folds in dashboard centering, which doesn't
            // apply here) -- cheap, and keeps this already-large
            // function's existing flow untouched.
            let visible_cols = {
                let gutter_chars = self.buffers.get(buffer_id).map(|ob| self.gutter_chars(ob)).unwrap_or(0);
                text::cols_that_fit(rect.w - gutter_chars as f32 * char_width, char_width)
            };
            self.set_pane_text_cols(pane, visible_cols);
            let wrap = if is_dashboard { None } else { self.wrap_cols(pane, buffer_id) };
            // By rows: wrapped, or with rows that aren't text (a code
            // lens's) at `UNBOUNDED`, scrolled sideways as any other.
            let by_rows = if is_dashboard { None } else { self.row_cols(pane, buffer_id) };
            if wrap.is_some() && self.pane_state(pane).scroll_col != 0 {
                self.pane_state_mut(pane).scroll_col = 0;
            }
            if is_focused {
                self.normalize_cursor_for_folds(buffer_id, pane);
                self.ensure_cursor_visible(pane_visible_lines);
                if !is_dashboard && wrap.is_none() {
                    self.ensure_cursor_visible_horizontally(visible_cols);
                }
            }
            // Home and a page are laid out to the pane's width and never
            // scroll sideways: their chips, cards and switches are placed by
            // cell, so text scrolled under them would slide off its boxes.
            if is_dashboard && self.pane_state(pane).scroll_col != 0 {
                self.pane_state_mut(pane).scroll_col = 0;
            }
            // Every pane -- not just the focused one -- has its own
            // `PaneState` (seeded when it was created), so each renders
            // from its own independent cursor/scroll position.
            let pane_state = *self.pane_state(pane);
            // Sideways scrolling eases a column at a time.
            let shown_scroll_col = if is_dashboard || wrap.is_some() { 0 } else { self.scroll_col(pane, pane_state.scroll_col, now) };
            let rendered_scroll = pane_state.rendered_scroll;
            // Scroll positions are display-row based once a scope is
            // collapsed. This mapping is the single conversion point back
            // to document rows for every operation below.
            let display_lines = self.pane_display_lines(pane, buffer_id);
            let render_base_line = rendered_scroll.floor().max(0.0) as usize;
            let visible_document_lines = &display_lines[render_base_line.min(display_lines.len())..];
            let folds_active = display_lines.len() != self.buffers.get(buffer_id).map(|ob| ob.buffer.line_count()).unwrap_or(0);
            // A wrapped pane's scroll snaps a row at a time.
            let render_frac = if by_rows.is_some() { 0.0 } else { rendered_scroll - rendered_scroll.floor() };
            // Computed here (rather than at its previous spot, further
            // down) so the dashboard-centering block below can look up
            // the cursor's own row in the per-line pad table.
            let (line, col) =
                self.buffers.get(buffer_id).map(|ob| ob.buffer.line_col(&pane_state.cursor)).unwrap_or((0, 0));
            // What is on each row -- the one translation between a place
            // in the text and a cell in the pane, for everything drawn over
            // the text below -- with each line's columns (tabs,
            // decorations) and the indent guides.
            let mut layout = self.frame_layout(pane, buffer_id, &display_lines, render_base_line, pane_state.scroll_row, pane_visible_lines + 1, shown_scroll_col, line, is_dashboard);
            let row_map = &layout.map;

            let gutter_chars = self.buffers.get(buffer_id).map(|ob| self.gutter_chars(ob)).unwrap_or(0);
            let gutter_px = gutter_chars as f32 * char_width;
            if is_page && is_focused {
                page_popup_at = Some(PagePopupAt { buffer: buffer_id, pane: rect, first_line: render_base_line, frac: render_frac, gutter_px });
            }
            // Home's layout is already centred; an empty pad list just
            // keeps `~` off the rows past its last line.
            let dashboard_pad: Option<Vec<usize>> = is_dashboard.then(Vec::new);
            // Filler rows (`side_by_side::FILLER`) have no line to highlight.
            let shown_lines: Vec<usize> = row_map.text_lines().collect();
            // What it shows, for the extensions that ask (`view-changed`).
            if let (Some(&first), Some(&last), false) = (shown_lines.first(), shown_lines.get(pane_visible_lines.saturating_sub(1)).or(shown_lines.last()), is_page || is_dashboard) {
                self.wasm.views_drawn.push((buffer_id, pane.number() as u64, first as u32, last as u32));
            }
            let syntax_start = shown_lines.first().copied().unwrap_or(0);
            let syntax_end = visible_document_end_line(&shown_lines, pane_visible_lines);
            // Its lines shown, for semantic tokens asked by range and the
            // lenses resolved: worked out only when either wants them.
            let (semantic, lensed, hinted) = (self.semantic_of.contains_key(&buffer_id), self.lens_of.contains_key(&buffer_id), self.inlay.contains_key(&buffer_id));
            if semantic || lensed || hinted {
                let runs = row_map.runs();
                if hinted {
                    self.note_inlay_view(buffer_id, pane, &runs);
                }
                if semantic {
                    self.note_semantic_view(buffer_id, pane, &runs);
                }
                if lensed {
                    self.note_lens_view(buffer_id, &runs);
                }
            }
            // A wrapped pane: the columns its rows show of each line.
            let row_columns: Option<HashMap<usize, (usize, usize)>> = wrap.map(|_| {
                let mut columns: HashMap<usize, (usize, usize)> = HashMap::new();
                for line in row_map.text_lines() {
                    if let Some(window) = rows::shown_char_columns(row_map, line, layout.col_maps.get(&line)) {
                        columns.insert(line, window);
                    }
                }
                columns
            });
            let syntax_highlights = self.syntax_highlights_in_columns(buffer_id, syntax_start, syntax_end.saturating_sub(syntax_start) + 1, shown_scroll_col, row_columns.as_ref(), true);
            let tab_stops = self.tab_stops_for(buffer_id);
            let content_spans = match self.buffers.get(buffer_id) {
                // A pane by rows is drawn by its rows.
                Some(ob) if by_rows.is_some() => self.row_content_spans(
                    ob, row_map, pane_visible_lines + 1, gutter_chars, &syntax_highlights,
                    line, &tab_stops, self.code_folds.get(&buffer_id).unwrap_or(&HashSet::new()), &layout.decorated,
                ),
                Some(ob) if folds_active => self.folded_content_spans(
                    ob, visible_document_lines, pane_visible_lines + 1, gutter_chars, &syntax_highlights,
                    line, &tab_stops, shown_scroll_col, self.code_folds.get(&buffer_id).unwrap_or(&HashSet::new()), &layout.decorated,
                ),
                Some(ob) => self.content_spans_decorated(
                    ob, render_base_line, pane_visible_lines + 1, gutter_chars, dashboard_pad.as_deref(),
                    &syntax_highlights, line, &tab_stops, shown_scroll_col, &layout.decorated,
                ),
                None => Vec::new(),
            };
            // A page's icons (an explorer's) are drawn in the icon font.
            let is_page = self.buffers.get(buffer_id).is_some_and(|ob| ob.kind == BufferKind::Page);
            let mut spans: RowSpans = if is_page { pages::split_icons(content_spans) } else { content_spans.into_iter().map(|(s, c)| (s, c, false)).collect() };

            // During a large animated pan the cursor's actual line can
            // legitimately be outside the currently-fetched window for
            // part of the transition (it hasn't panned into view yet) --
            // `None` means "don't draw the hl-line/caret this frame," not
            // a bug.
            // The row the cursor is on, of its line's.
            let hl_row = layout.locate(line, col).map(|(row, _)| row);

            let (vcol, locate, to_rows) = (|l, c| layout.vcol(l, c), |l, c| layout.locate(l, c), |segs| layout.to_rows(segs));
            let shown_span = row_map.span();
            let FocusedOverlays { extra_carets, selection: selection_segments, pulse: pulse_overlay, brackets: bracket_match_segments, hlsearch: hlsearch_segments, caret } =
                if is_focused { self.focused_overlays(&layout, line, col) } else { FocusedOverlays::default() };

            // A soft tint behind each TODO-style keyword, in its own
            // kind's color, so they stand out while scrolling past --
            // the glyphs alone are easy to lose in a comment-colored
            // line.
            let (side_tints, side_words) = self.side_by_side_tints(pane, &row_map.lines_by_row());
            let mut todo_bg_segments: Vec<(usize, usize, usize, [f32; 4])> = match (self.todo_spans.get(&buffer_id), self.buffers.get(buffer_id)) {
                (Some(spans), Some(ob)) => spans
                    .iter()
                    .flat_map(|(range, kind)| {
                        let start = ob.buffer.byte_to_char(range.start);
                        let len = ob.buffer.byte_to_char(range.end) - start;
                        let (line, col) = ob.buffer.line_col(&Cursor { char_idx: start, sticky_col: 0 });
                        let [r, g, b, _] = glyphon_to_rgba(theme.todo_color(*kind));
                        to_rows(vec![(line, col, col + len)]).into_iter().map(move |(row, s, e)| (row, s, e, [r, g, b, 0.18]))
                    })
                    .collect(),
                _ => Vec::new(),
            };
            todo_bg_segments.extend(side_words.into_iter().filter_map(|(row, s, e, color)| {
                let line = row_map.row(row).filter(|r| r.kind == rows::RowKind::Text)?.line;
                Some(to_rows(vec![(line, s, e)]).into_iter().map(move |(row, s, e)| (row, s, e, color)))
            }).flatten());
            // The decorations' swatches and tints behind the text, their
            // underlines under it -- already in visual columns.
            let mut decoration_underlines = Vec::new();
            let mut decoration_strikes = Vec::new();
            let mut decoration_chips: Vec<(usize, usize, usize, [f32; 4], Option<[f32; 4]>)> = Vec::new();
            for (line, decorated) in &layout.decoration_rows {
                for (cols, mark) in &decorated.marks {
                    for (row, a, b) in row_map.segments(*line, cols.clone()) {
                        if a >= b {
                            continue;
                        }
                        match mark {
                            crate::decorations::Mark::Swatch((r, g, b2)) => todo_bg_segments.push((row, a, b, [*r as f32 / 255.0, *g as f32 / 255.0, *b2 as f32 / 255.0, 1.0])),
                            crate::decorations::Mark::Tint(shade) => todo_bg_segments.push((row, a, b, pages::shade_color(theme, *shade))),
                            crate::decorations::Mark::Underline(role) => decoration_underlines.push((row, a, b, glyphon_to_rgba(pages::role_color(theme, *role)))),
                            crate::decorations::Mark::Strike(role) => decoration_strikes.push((row, a, b, glyphon_to_rgba(pages::role_color(theme, *role)))),
                            crate::decorations::Mark::Chip(role) => {
                                let [r, g, b2, _] = glyphon_to_rgba(pages::role_color(theme, *role));
                                decoration_chips.push((row, a, b, [r, g, b2, 0.13], None));
                            }
                        }
                    }
                }
            }

            // A `show_tabs` theme's tab strip for this pane -- only for an
            // ordinary file-editing pane (no `pane_titles` override; a
            // fixed-purpose session panel always keeps its plain single
            // title). Every other `PaneRender` push site above (terminal/
            // VNC/PDF/overlay/table) leaves these three fields empty,
            // since tabs make no sense for those. Computed here, not in
            // the later GPU-borrow-split rendering block, since it needs
            // `self.buffer_display_name`/`self.buffers`/`self.workspaces`/
            // `self.windows()`, all off-limits once `text`/`bg_rect` hold
            // exclusive borrows of other `self` fields down there.
            let (tabs_layout, tab_active, tab_spans, breadcrumb_spans, tab_ranges, tab_italic) =
                self.pane_tab_strip(pane, rect, line_height, is_focused, theme, Some((buffer_id, pane_state.cursor.char_idx)));

            // Inline git gutter marks (Part 5) currently in view -- every
            // mark's 1-based file line is remapped to a row relative to
            // `render_base_line`, same convention `hl_row` already uses,
            // so the later GPU-borrow-split drawing loop needs no buffer/
            // scroll lookups of its own. Marks scrolled out of view are
            // simply dropped here, not carried forward.
            let gutter_marks: Vec<(usize, GutterMarkKind)> = self
                .gutter_hunks
                .get(&buffer_id)
                .map(|marks| {
                    marks
                        .iter()
                        .filter_map(|mark| Some((row_map.first_row(mark.line.saturating_sub(1))?, mark.kind)))
                        .collect()
                })
                .unwrap_or_default();

            // A Jira comment's or Agenda note's own header row gets a
            // thin left-margin accent, same rendering role as a git
            // gutter mark but one flat color per buffer kind rather than
            // a per-hunk choice -- see `PaneRender::row_accents`'s own
            // doc comment for why this isn't just another `GutterMark`.
            let row_accents: Vec<(usize, [f32; 4])> = Vec::new();

            // The debugger's breakpoints and stop beside the rows in view;
            // the stopped row tinted across the pane.
            let gutter_lines = row_map.gutter_lines();
            let debug_marks = self.debug_marks(buffer_id, &gutter_lines, pane_visible_lines);
            let trail_marks = self.debug_trail_marks(buffer_id, &gutter_lines, pane_visible_lines);
            let mut row_tints = side_tints;
            if let Some(&(row, _)) = debug_marks.iter().find(|(_, m)| matches!(m, debug_host::DebugMark::Stop | debug_host::DebugMark::StopOnBreakpoint(_))) {
                let [r, g, b, _] = glyphon_to_rgba(theme.git_modified);
                // Every row of the stopped line; the mark is on its first.
                let stopped = row_map.row(row).map_or(row..row + 1, |shown| row_map.rows_of(shown.line));
                row_tints.extend(stopped.map(|row| (row, [r, g, b, 0.14])));
            }

            let (home_segments, mut home_overlay) =
                if is_page {
                    self.page_backgrounds(buffer_id, render_base_line, pane_visible_lines)
                } else if is_home {
                    self.home_backgrounds(buffer_id, pane, render_base_line, pane_visible_lines)
                } else {
                    Default::default()
                };
            let mut extra = motion_host::PaneExtras {
                buffer: Some(buffer_id),
                base_line: render_base_line,
                scroll_col: shown_scroll_col,
                rows: (!is_dashboard).then(|| layout.map.clone()),
                underlines: decoration_underlines,
                strikes: decoration_strikes,
                grid: is_page,
                ..Default::default()
            };
            // A code pane's polish: problems, brackets, selection
            // whitespace, the overview ruler, sticky scroll.
            if !is_dashboard && self.buffers.get(buffer_id).is_some_and(|ob| ob.kind == BufferKind::Text) {
                let tab_lines: HashSet<usize> = layout.col_maps.keys().copied().collect();
                let pane_caret_ats: Vec<usize> = self.pane_carets(pane, buffer_id).iter().map(|c| c.at).collect();
                let view = polish::PaneView {
                    buffer: buffer_id,
                    map: row_map,
                    rows: pane_visible_lines + 1,
                    wrapped: by_rows.is_some(),
                    gutter_chars,
                    scroll_col: shown_scroll_col,
                    rendered_scroll,
                    cursor_line: line,
                    tab_lines: &tab_lines,
                    selection: &selection_segments,
                    vcol: &vcol,
                    carets: &pane_caret_ats,
                    pane,
                };
                let polished = self.polish_pane(&view, &mut spans, now);
                extra.squiggles = polished.squiggles;
                extra.underlines.extend(polished.underlines);
                extra.ruler = polished.ruler;
                // Off in a wrapped pane, for now.
                if !folds_active && by_rows.is_none() {
                    extra.sticky = self.sticky_rows(buffer_id, render_base_line, hl_row, gutter_chars, &tab_stops, line);
                }
            }
            if let Some(flash) = self.beacon_row(buffer_id, &gutter_lines, pane_visible_lines, now) {
                extra.row_flashes.push(flash);
            }
            if is_focused {
                let (added, seams) = self.change_overlays(buffer_id, shown_span.clone(), now);
                let [r, g, b, _] = glyphon_to_rgba(theme.git_staged);
                extra.cell_flashes = added.into_iter().flat_map(|(line, s, e, a)| to_rows(vec![(line, s, e)]).into_iter().map(move |(row, s, e)| (row, s, e, [r, g, b, 0.35 * a]))).collect();
                let [r, g, b, _] = glyphon_to_rgba(theme.git_conflicted);
                extra.seams = seams
                    .into_iter()
                    .filter_map(|(line, col, lines, a)| {
                        let (row, x) = if lines { (row_map.first_row(line)?, 0) } else { locate(line, col)? };
                        Some((row, x, lines, [r, g, b, 0.9 * a]))
                    })
                    .collect();
            }
            if let Some((first, count, shown)) = self.reveal_cover(motion_host::RevealTarget::Buffer(buffer_id), now) {
                if first + count > render_base_line {
                    let row = first.saturating_sub(render_base_line);
                    let count = count - render_base_line.saturating_sub(first);
                    extra.reveal = Some((row, count, shown));
                }
            }
            panes_render.push(PaneRender {
                pane,
                rect,
                title: pane_title,
                spans,
                // Home draws its own selection (the slot's tint and focus
                // rail) and no caret; a full-row highlight would fight it.
                hl_row: if is_dashboard { None } else { hl_row },
                hl_row_strong: false,
                marked_rows: Vec::new(),
                row_tints,
                colored_bg_segments: if is_dashboard { home_segments } else { todo_bg_segments },
                selection_segments,
                pulse_overlay,
                bracket_match_segments,
                hlsearch_segments,
                extra_carets,
                caret: if is_dashboard || (is_focused && main_view == MainView::Picker) { None } else { caret },
                content_frac: render_frac,
                gutter_px,
                tabs_layout,
                tab_active,
                tab_spans,
                breadcrumb_spans,
                tab_ranges,
                tab_italic,
                has_breadcrumb,
                gutter_marks,
                debug_marks,
                trail_marks,
                indent_guides: std::mem::take(&mut layout.indent_guides),
                row_accents,
                home: {
                    home_overlay.chips.extend(decoration_chips.into_iter().map(|(r, a, b, f, o)| (r, a, b, f, o, false)));
                    home_overlay
                },
                extra,
            });
        }

        let modeline_pieces = self.modeline_pieces();
        // Which project owns the focused buffer, ahead of everything else
        // on the modeline -- one language server runs per project root,
        // so this answers "which one am I talking to?".
        let project_spans = if self.main_view == MainView::Editor {
            self.focused_project_identity().map(|(kind, name)| projects::modeline_project_spans(kind, &name, theme)).unwrap_or_default()
        } else {
            Vec::new()
        };
        _profile.mark("CPU pane preparation complete");
        let (badge_bg, _badge_fg) = self.mode_colors();
        let badge_bg = self.rail_color(badge_bg, now);
        // Top-right corner, clear of both the content the user is actively
        // editing (top-left, where the cursor usually is) and the modeline
        // (bottom) -- least likely to sit under whatever they're looking
        // at. Resolved (and, if needed, row-truncated) here rather than
        // left to `text.rs`/`prepare` so its position is known before the
        // `bg_rect` panel-background push below.
        // The which-key drawer, once a sequence has waited long enough.
        // The leader's menu and the open picker, as palette cards over
        // the window above the modeline; Vim's prefixes' hint bar, kept
        // off the cursor's line.
        let card_area = fenix_window::Rect { x: 0.0, y: 0.0, w: window_width, h: modeline_top };
        let focused_render = panes_render.iter().find(|p| p.pane == focused_pane);
        let cursor_y = focused_render.and_then(|p| p.caret.map(|(row, _)| p.rect.y + text::PAD_TOP + row as f32 * line_height));
        let bar_top = focused_render.map(|p| p.rect.y).unwrap_or(0.0);
        let which_key_popup = if overlays_here && self.which_key_open(now) {
            self.which_key_pending().map(|pending| {
                let vim = pending.source == which_key::Source::Vim;
                let area = if vim { fenix_window::Rect { x: 0.0, y: bar_top, w: window_width, h: (modeline_top - bar_top).max(0.0) } } else { card_area };
                let card = self.which_key_card(&pending, area, char_width, line_height, cursor_y);
                self.motion.which_key_pages = card.pages;
                let id = if vim { popup::PopupId::WhichKey } else { popup::PopupId::Leader };
                let out = (id, card.rect, card.spans.clone());
                self.motion.cards.insert(id, card);
                out
            })
        } else {
            None
        };
        let palette_popup = if overlays_here && self.main_view == MainView::Picker {
            let lines = palette::list_lines(card_area, line_height);
            self.palette_view(lines).map(|view| {
                let card = palette::layout(&view, self.theme, card_area, char_width, line_height, 0);
                let out = (card.rect, card.spans.clone());
                self.motion.cards.insert(popup::PopupId::Palette, card);
                out
            })
        } else {
            None
        };
        // Never showing at the same time as `which_key_popup`/
        // `completion_popup` in practice -- the Git session's menu only
        // shows while one of its panes is focused, where a leader/
        // operator-pending sequence or an Insert-mode completion session
        // can't simultaneously be active.
        let git_menu_popup = overlays_here.then(|| self.git_menu_popup(window_width, modeline_top)).flatten();
        // Never `Some` at the same time as `which_key_popup` -- one only
        // appears mid-Normal/-pending-sequence, the other only in Insert
        // mode -- so `popup_rects` below never ends up with more than one
        // entry in practice, even though it's shaped as a list.
        let completion_popup = overlays_here.then_some(()).and_then(|()| panes_render.iter().find(|p| p.pane == focused_pane)).and_then(|focused| {
            self.completion_popup(window_width, modeline_top, focused.rect, focused.caret, focused.gutter_px, focused.content_frac)
        });
        // Same "focused pane only" shape as `completion_popup` above --
        // see `PopupId::Hover`'s own doc comment for why this never
        // coexists with it in practice.
        let hover_popup = overlays_here.then_some(()).and_then(|()| panes_render.iter().find(|p| p.pane == focused_pane)).and_then(|focused| {
            self.hover_popup(window_width, modeline_top, focused.rect, focused.caret, focused.gutter_px, focused.content_frac)
        });
        // See `popup::PopupId::Prompt`'s own doc comment for why this
        // never coexists with any popup above.
        let prompt_popup = overlays_here.then(|| self.prompt_popup(window_width, modeline_top)).flatten();
        let prompt_selection = self.prompt_selection();
        let page_popup = overlays_here.then(|| self.page_popup(page_popup_at, window_width, modeline_top)).flatten();
        // The right click's menu.
        let context_menu_popup = overlays_here.then(|| self.context_menu_popup(window_width, window_height)).flatten();
        // Notifications: the toasts, in their corner, under the tab strip.
        let toasts = if overlays_here { self.toasts(window_width, tab_strip_height(line_height) + line_height, modeline_top, char_width, line_height) } else { Vec::new() };
        self.toast_rects = toasts.iter().map(|t| (t.id, t.rect)).collect();
        // The panel has the keyboard: the editor's caret isn't where keys go.
        let caret_alpha = if self.terminal_focused { 0.0 } else { self.caret_alpha() };
        let message_alpha = self.message_alpha(now);
        let jobs = self.background_jobs();
        let frame_ws = (self.active_frame, self.workspaces.active);
        let corner_radius = self.corner_radius();
        let shadows = self.config.polish.shadows != Some(false);
        let dim_amount = (self.config.polish.dim_unfocused != Some(false) && panes_render.len() > 1)
            .then(|| self.config.polish.dim_amount.unwrap_or(30).min(90) as f32 / 100.0);
        let sidebar_reveal = self.reveal_cover(motion_host::RevealTarget::Sidebar, now);
        let (sidebar_scroll, sidebar_rows_at) = (self.sidebar_view.scroll, self.sidebar_view.rows_at);
        let rounded_selection = self.config.polish.rounded_selection != Some(false) && corner_radius > 0.0;
        let show_indent_guides = self.config.polish.indent_guides != Some(false);
        let show_active_guide = self.config.polish.active_indent_guide != Some(false);


        // Each surface's pane, in pixels: told to its extension after the frame.
        let frame_scale = self.frame_scale();
        self.wasm.sized = code_surface_panes.iter().map(|(b, r)| (*b, (r.w.round().max(1.0) as u32, r.h.round().max(1.0) as u32), frame_scale)).collect();

        let (
            Some(window),
            Some(gpu),
            Some(text),
            Some(bg_rect),
            Some(popup_rect),
            Some(caret_rect),
            Some(surfaces),
            Some(motion_gpu),
        ) = (
            &self.window,
            &mut self.gpu,
            &mut self.text,
            &mut self.bg_rect,
            &mut self.popup_rect,
            &mut self.caret_rect,
            // Mutable: quads are accumulated into it before the render
            // pass and drawn by slot inside it (see `SurfacePipeline`),
            // rather than written one-at-a-time mid-pass.
            &mut self.surfaces,
            &mut self.motion_gpu,
        ) else {
            return;
        };
        let motion_state = &mut self.motion;

        let live_panes: Vec<fenix_window::WindowId> = panes_render.iter().map(|p| p.pane).collect();
        for pane in &panes_render {
            let content_refs: Vec<(&str, glyphon::Color, bool)> =
                pane.spans.iter().map(|(s, c, i)| (s.as_str(), *c, *i)).collect();
            text.set_pane_rich(pane.pane, pane.rect.w, pane.rect.h, &content_refs, pane.extra.grid, &pane.extra.span_styles);
            let sticky: RowSpans = polish::join_rows(pane.extra.sticky.clone());
            let sticky_refs: Vec<(&str, glyphon::Color, bool)> = sticky.iter().map(|(s, c, i)| (s.as_str(), *c, *i)).collect();
            text.set_pane_sticky(pane.pane, pane.rect.w, &sticky_refs);
        }
        text.retain_panes(&live_panes);

        // Title-bar strips for every pane -- `pane.rect` here is already
        // the *shrunk* content rect (see the top-of-loop adjustment
        // above), so the whole reserved title area sits exactly `title_
        // reserved_height` above it, same width, split into its own
        // `tab_strip_height`-tall row and (for a tabbed pane only) a
        // shorter `breadcrumb_strip_height`-tall row beneath it -- two
        // separate `GlyphBuffer`s (`tab_text_rects`/`breadcrumb_text_
        // rects`) rather than two lines of one buffer, since that's the
        // only way to give them different heights. `title_rects` stays
        // the *full* reserved area, used only for the flat background
        // fill below. The focused pane's title is colored with the same
        // accent `caret_text` uses elsewhere (the which-key popup's key
        // column), so which pane has focus reads at a glance across a
        // split -- every other pane's title stays the plain `fg_modeline`
        // it always has.
        let mut title_rects: Vec<(fenix_window::WindowId, fenix_window::Rect)> = Vec::with_capacity(panes_render.len());
        let mut tab_text_rects: Vec<(fenix_window::WindowId, fenix_window::Rect)> = Vec::with_capacity(panes_render.len());
        let mut breadcrumb_text_rects: Vec<(fenix_window::WindowId, fenix_window::Rect)> = Vec::new();
        let tab_h = tab_strip_height(line_height);
        for pane in &mut panes_render {
            // Exactly the height this pane's content rect was shrunk by,
            // from the same flag -- an informational panel's chrome is
            // the title row alone, so its background fill stops where
            // its content starts instead of painting a blank breadcrumb
            // strip over the first row of the list.
            let chrome_h = pane_chrome_height(line_height, pane.has_breadcrumb);
            let top = pane.rect.y - chrome_h;
            let title_rect = fenix_window::Rect { x: pane.rect.x, y: top, w: pane.rect.w, h: chrome_h };
            let tab_rect = fenix_window::Rect { x: pane.rect.x, y: top, w: pane.rect.w, h: tab_h };
            if pane.tabs_layout.is_empty() {
                let color = if pane.pane == focused_pane { theme.caret_text } else { theme.fg_modeline };
                // Cut to the pane with a "…", rather than at its edge
                // mid-word (the agents' prompt says what sends it there).
                let fits = ((tab_rect.w - text::PAD_LEFT * 2.0).max(0.0) / text.title_char_width().max(1.0)).floor() as usize;
                let title = crate::page::fit(&pane.title, fits);
                text.set_pane_title_rich(pane.pane, tab_rect.w, &[(title.as_str(), color, false)], &[]);
            } else {
                let refs: Vec<(&str, glyphon::Color, bool)> =
                    pane.tab_spans.iter().map(|(s, c, i)| (s.as_str(), *c, *i)).collect();
                text.set_pane_title_rich(pane.pane, tab_rect.w, &refs, &pane.tab_italic);
                // Now that the strip is shaped, replace each tab's
                // estimated rect with where its glyphs actually landed.
                // This is the whole fix for the active tab's highlight
                // sitting right of its own tab: everything that draws a
                // tab (background, top accent, divider) and everything
                // that hit-tests one reads `tabs_layout`, so correcting
                // it here corrects all of them at once, and correcting it
                // from measured glyph bounds means it stays correct for
                // any filename, icon, dirty marker, font, or DPI rather
                // than for the one case a constant was tuned against.
                //
                // `text_origin` is the same `rect.x + PAD_LEFT` the title
                // `TextArea` is pushed at (see `TextPipeline::prepare`),
                // which is what puts these buffer-relative offsets into
                // the same screen space as `pane.rect`.
                let probes: Vec<(usize, usize)> =
                    pane.tab_ranges.iter().flat_map(|&(bs, be, cs, ce)| [(bs, be), (cs, ce)]).collect();
                let bounds = text.title_span_bounds(pane.pane, &probes);
                let text_origin = pane.rect.x + text::PAD_LEFT;
                for (tab, measured) in pane.tabs_layout.iter_mut().zip(bounds.as_chunks::<2>().0) {
                    let [Some((body_left, body_right)), Some((close_left, close_right))] = *measured else {
                        continue;
                    };
                    tab.body.x = text_origin + body_left;
                    tab.body.w = body_right - body_left;
                    tab.close.x = text_origin + close_left;
                    tab.close.w = close_right - close_left;
                }
                // A page has no breadcrumb row: its tab says what it is.
                if pane.has_breadcrumb {
                    let crumb_rect =
                        fenix_window::Rect { x: pane.rect.x, y: top + tab_h, w: pane.rect.w, h: breadcrumb_strip_height(line_height) };
                    let crumb_refs: Vec<(&str, glyphon::Color, bool)> =
                        pane.breadcrumb_spans.iter().map(|(s, c, i)| (s.as_str(), *c, *i)).collect();
                    text.set_pane_breadcrumb_rich(pane.pane, crumb_rect.w, &crumb_refs);
                    breadcrumb_text_rects.push((pane.pane, crumb_rect));
                }
            }
            title_rects.push((pane.pane, title_rect));
            tab_text_rects.push((pane.pane, tab_rect));
        }
        text.retain_titles(&live_panes);
        if let Some((sidebar_spans, _, _, _)) = &sidebar_render {
            let sidebar_refs: Vec<(&str, glyphon::Color, bool)> =
                sidebar_spans.iter().map(|(s, c, i)| (s.as_str(), *c, *i)).collect();
            text.set_sidebar_rich(&sidebar_refs);
        }
        if let Some(view) = &terminal_render {
            let terminal_refs: Vec<(&str, glyphon::Color, bool)> = view.spans.iter().map(|(s, c, i)| (s.as_str(), *c, *i)).collect();
            text.set_terminal_rich(&terminal_refs, &view.styles);
        }
        if let Some(dock_spans) = &debug_dock_render {
            let dock_refs: Vec<(&str, glyphon::Color, bool)> = dock_spans.iter().map(|(s, c, i)| (s.as_str(), *c, *i)).collect();
            text.set_debug_dock_rich(&dock_refs);
        }
        let (existing_chars, mode_segment_chars) = {
            let (mode_label, suffix) = &modeline_pieces;
            // `modeline_mode_segment` owns this string's exact shape (the
            // leading glyph, the fixed-width centered label, the trailing
            // gap) -- see its own doc comment. The rail itself is much
            // narrower than `PAD_LEFT`, so it never touches this text.
            let badge = modeline_mode_segment(mode_label);
            let mode_segment_chars = badge.chars().count();
            let project_chars: usize = project_spans.iter().map(|(text, _)| text.chars().count()).sum();
            let existing_chars = mode_segment_chars + project_chars + suffix.chars().count();
            // An unexpired error message tints the suffix red (`git_
            // conflicted`'s accent -- no dedicated error color exists
            // yet, and this reads as a reasonable semantic reuse
            // rather than a new theme field x6 themes) instead of the
            // ordinary modeline foreground.
            let is_error_message =
                self.status_message.as_ref().is_some_and(|m| m.is_error && m.set_at.elapsed() < MESSAGE_DURATION);
            let suffix_fg = if is_error_message { theme.git_conflicted } else { theme.fg_modeline };
            let mut suffix_spans = modeline_suffix_spans(suffix, theme, suffix_fg);
            // A message fades in and out; the file's own details don't.
            if self.status_message.as_ref().is_some_and(|m| m.set_at.elapsed() < MESSAGE_DURATION) {
                for (_, color) in &mut suffix_spans {
                    *color = glyphon::Color::rgba(color.r(), color.g(), color.b(), (color.a() as f32 * message_alpha).round() as u8);
                }
            }
            let mut spans = Vec::with_capacity(suffix_spans.len() + 1);
            // The label reads *in* the mode's own accent now (Signal
            // Rail's rail-plus-label treatment), not as contrasting text
            // over a filled box -- `badge_fg` (`mode_text_dark`/`_light`)
            // was that box's own contrast color and is unused here now.
            spans.push((badge, rgba_to_glyphon(badge_bg)));
            spans.extend(project_spans);
            spans.extend(suffix_spans);
            let refs: Vec<(&str, glyphon::Color)> = spans.iter().map(|(text, color)| (text.as_str(), *color)).collect();
            text.set_modeline_text(&refs);
            (existing_chars, mode_segment_chars)
        };
        // A *separate* buffer/`TextArea` from the modeline's own, right-
        // aligned via cosmic-text's own `Align::Right` layout rather than
        // this crate approximating a pixel position (see `clock`'s doc
        // comment in `text.rs` for why -- in short, this is what fixes
        // two real, reported bugs where the clock ended up clipped by
        // the window's right edge). `theme.fg_modeline`, not `gutter_fg`
        // -- the same color the rest of the modeline's text already
        // uses, guaranteed legible against `bg_modeline` in every theme
        // (white on TempleOS specifically), unlike `gutter_fg`, which is
        // calibrated for contrast against the *content* background and
        // read as barely visible here.
        // Background work leads the clock: its name, with the spinner
        // drawn just left of it (see the overlay pass).
        let job_text = match jobs.as_slice() {
            [] => String::new(),
            [one] => format!("{one}   "),
            [first, rest @ ..] => format!("{first} +{}   ", rest.len()),
        };
        let clock_text = modeline_clock_text();
        let clock_chars = clock_text.chars().count() + job_text.chars().count();
        let box_width = window_width - text::PAD_LEFT;
        // `modeline_char_width`, not the body `char_width`: both this
        // text and the clock render at `MODELINE_FONT_SCALE`, so the body
        // measurement over-estimated every column here.
        let clock_shown = modeline_clock_fits(existing_chars, window_width, modeline_char_width, clock_chars);
        if clock_shown {
            text.set_clock_rich(box_width, &[(job_text.as_str(), theme.gutter_fg), (clock_text.as_str(), theme.fg_modeline)]);
        } else if !job_text.is_empty() {
            text.set_clock_rich(box_width, &[(job_text.as_str(), theme.gutter_fg)]);
        } else {
            text.set_clock_rich(box_width, &[]);
        }
        // Measured after shaping, not estimated from `clock_chars` -- see
        // `TextPipeline::clock_left_edge`.
        let clock_left = text.clock_left_edge();

        // Every popup open this frame, then the ones still fading out --
        // `popup_draws` decides each one's opacity and rise.
        let mut open_popups: Vec<(popup::PopupId, fenix_window::Rect, RowSpans, Option<usize>)> = Vec::new();
        if let Some((id, rect, spans)) = which_key_popup {
            open_popups.push((id, rect, spans, None));
        }
        if let Some((rect, spans)) = palette_popup {
            open_popups.push((popup::PopupId::Palette, rect, spans, None));
        }
        if let Some((rect, spans)) = git_menu_popup {
            open_popups.push((popup::PopupId::GitMenu, rect, spans, None));
        }
        if let Some((rect, spans, selected_row)) = completion_popup {
            open_popups.push((popup::PopupId::Completion, rect, spans, selected_row));
        }
        if let Some((rect, spans)) = hover_popup {
            open_popups.push((popup::PopupId::Hover, rect, spans, None));
        }
        if let Some((rect, spans)) = prompt_popup {
            open_popups.push((popup::PopupId::Prompt, rect, spans, None));
        }
        if let Some((rect, spans)) = page_popup {
            open_popups.push((popup::PopupId::Page, rect, spans, None));
        }
        for toast in &toasts {
            open_popups.push((popup::PopupId::Toast(toast.slot), toast.rect, toast.spans.clone(), None));
        }
        if let Some((rect, spans, selected_row)) = context_menu_popup {
            open_popups.push((popup::PopupId::ContextMenu, rect, spans, selected_row));
        }
        let popup_draws = motion_state.popup_draws(prefs, open_popups, now);
        let mut popup_rects: Vec<(popup::PopupId, fenix_window::Rect)> = Vec::new();
        for draw in &popup_draws {
            let fade = |c: glyphon::Color| glyphon::Color::rgba(c.r(), c.g(), c.b(), (c.a() as f32 * draw.alpha).round() as u8);
            let refs: Vec<(&str, glyphon::Color, bool)> = draw.spans.iter().map(|(s, c, i)| (s.as_str(), fade(*c), *i)).collect();
            text.set_popup_rich(draw.id, draw.rect.w, &refs, motion_state.cards.get(&draw.id).map(|c| c.pitch));
            popup_rects.push((draw.id, draw.rect));
        }
        text.retain_popups(&popup_rects.iter().map(|(id, _)| *id).collect::<Vec<_>>());

        let sidebar_row_y = |row: usize| text::PAD_TOP + text::STRIP_INSET + row as f32 * line_height;

        bg_rect.clear();
        // Every pane's panel, its chrome and all, rounded on the tray.
        for pane in &panes_render {
            let chrome_h = pane_chrome_height(line_height, pane.has_breadcrumb);
            let r = pane.rect;
            let fill = if Some(pane.pane) == composer_pane { theme.input } else { theme.bg };
            bg_rect.push_box(gpu, r.x, r.y - chrome_h, r.w, r.h + chrome_h, crate::rect::BoxStyle { radius: panel_radius, fill, border: 0.0, border_color: [0.0; 4] });
        }
        for pane in &panes_render {
            // Row index (relative to that pane's own render_base_line, or
            // its explorer/picker listing's own scroll) -> pixel y within
            // the pane, shifted up by its own mid-scroll fractional
            // offset so it pans in step with its text (always 0 outside
            // the focused pane's Editor-mode smooth scroll).
            let row_y = |row: usize| pane.rect.y + text::PAD_TOP + row as f32 * line_height - pane.content_frac * line_height;
            if let Some(row) = pane.hl_row {
                let y = row_y(row);
                let color = if pane.hl_row_strong { theme.selection } else { theme.hl_line };
                bg_rect.push_rect(gpu, pane.rect.x, y, pane.rect.w, line_height, color);
            }
            for row in &pane.marked_rows {
                let y = row_y(*row);
                bg_rect.push_rect(gpu, pane.rect.x, y, pane.rect.w, line_height, theme.selection);
            }
            for &(row, color) in &pane.row_tints {
                bg_rect.push_rect(gpu, pane.rect.x, row_y(row), pane.rect.w, line_height, color);
            }
            // Column-math x-offset for this pane's caret/selection/pulse
            // rects: its own left edge, plus `PAD_LEFT`, plus its own
            // line-number gutter -- has to match exactly where `text.
            // prepare`'s `TextArea` for this pane actually renders the
            // text, or these rects drift out of alignment with it.
            let content_x = pane.rect.x + text::PAD_LEFT + pane.gutter_px;
            // Where a page's boxes stop: its text is cut at the pane's edge,
            // and a chip or card past it would be drawn over the pane beside.
            let inside_right = pane.rect.x + pane.rect.w - (char_width * 0.5).round();
            // A page's cards: under its tints, so the focused row's still
            // shows on one. A little wider than their cells.
            for &(row, rows, a, b, fill, outline, tight) in &pane.home.cards {
                let pad = (char_width * 0.6).round();
                let (x, w, top) = (content_x + a as f32 * char_width - pad, (b - a) as f32 * char_width + pad * 2.0, pane.rect.y + text::PAD_TOP - pane.content_frac * line_height);
                // A selected row among others keeps to its own rows, or it
                // would run into the ones above and below.
                let r = if tight { panels::row_rect(x, w, top, line_height, 0, rows.saturating_sub(1)) } else { panels::card_rect(x, w, top, line_height, 0, rows.saturating_sub(1)) };
                let Some(r) = panels::clip_right(r, inside_right) else { continue };
                let y = r.y + row as f32 * line_height;
                let style = crate::rect::BoxStyle { radius: panels::CARD_RADIUS, fill, border: if outline.is_some() { 1.0 } else { 0.0 }, border_color: outline.unwrap_or([0.0; 4]) };
                bg_rect.push_box(gpu, r.x, y, r.w, r.h, style);
            }
            // Drawn before selection/search/pulse so those still read as
            // overlays on top of whatever the shell painted, rather than
            // being covered by it.
            for &(row, col_start, col_end, color) in &pane.colored_bg_segments {
                let x = content_x + col_start as f32 * char_width;
                let y = row_y(row);
                let w = (col_end - col_start) as f32 * char_width;
                bg_rect.push_rect(gpu, x, y, w, line_height, color);
            }
            for &(row, a, b, fill, outline, pill) in &pane.home.chips {
                let r = panels::key_cap(content_x, char_width, row_y(row), line_height, a, b);
                let Some(r) = panels::clip_right(r, inside_right) else { continue };
                let radius = if pill { r.h / 2.0 } else { panels::KEY_RADIUS };
                let style = crate::rect::BoxStyle { radius, fill, border: if outline.is_some() { 1.0 } else { 0.0 }, border_color: outline.unwrap_or([0.0; 4]) };
                bg_rect.push_box(gpu, r.x, r.y, r.w, r.h, style);
            }
            // A commit graph's rows: rails, bends and dots, kept to the
            // pane like the rest.
            for (row, a, b, lanes, marks) in &pane.home.graph {
                let x = content_x + *a as f32 * char_width;
                let w = ((*b - *a) as f32 * char_width).min(inside_right - x);
                if w <= 0.0 {
                    continue;
                }
                let ring = theme.caret;
                for shape in graph_draw::shapes(x, w, row_y(*row), line_height, char_width, *lanes, marks, ring) {
                    match shape {
                        graph_draw::Shape::Bar(x, y, w, h, color) => bg_rect.push_rect(gpu, x, y, w, h, color),
                        graph_draw::Shape::Arc { cx, cy, r, right, down, color } => bg_rect.push_arc(gpu, cx, cy, r, graph_draw::thickness(char_width), right, down, color),
                        graph_draw::Shape::Dot { cx, cy, r, ring, color } => {
                            let style = if ring {
                                crate::rect::BoxStyle { radius: r, fill: [color[0], color[1], color[2], 0.0], border: graph_draw::thickness(char_width), border_color: color }
                            } else {
                                crate::rect::BoxStyle { radius: r, fill: color, border: 0.0, border_color: [0.0; 4] }
                            };
                            bg_rect.push_box(gpu, cx - r, cy - r, r * 2.0, r * 2.0, style);
                        }
                    }
                }
            }
            // A switch's knob, at the end it's on.
            for &(row, a, b, on) in &pane.home.switches {
                let r = panels::key_cap(content_x, char_width, row_y(row), line_height, a, b);
                if r.x + r.w > inside_right {
                    continue;
                }
                let d = (r.h - 4.0).max(2.0);
                let x = if on { r.x + r.w - d - 2.0 } else { r.x + 2.0 };
                let color = if on { glyphon_to_rgba(theme.git_staged) } else { glyphon_to_rgba(theme.gutter_fg) };
                bg_rect.push_box(gpu, x, r.y + 2.0, d, d, crate::rect::BoxStyle { radius: d / 2.0, fill: color, border: 0.0, border_color: [0.0; 4] });
            }
            for &(row, start, end) in &pane.home.rules {
                let y = (row_y(row) + line_height / 2.0).round();
                let x = content_x + start as f32 * char_width;
                let w = ((end - start) as f32 * char_width).min(inside_right - x);
                if w > 0.0 {
                    bg_rect.push_rect(gpu, x, y, w, 1.0, pane.home.rule_color);
                }
            }
            // A canvas's hairlines down it, in the middle of their cells.
            for &(col, row, rows) in &pane.home.vertical_rules {
                let top = (pane.rect.y + text::PAD_TOP + (row as f32 - pane.content_frac) * line_height).max(pane.rect.y);
                let bottom = (pane.rect.y + text::PAD_TOP + (row as f32 + rows as f32 - pane.content_frac) * line_height).min(pane.rect.y + pane.rect.h);
                if bottom > top {
                    bg_rect.push_rect(gpu, (content_x + (col as f32 + 0.5) * char_width).round(), top, 1.0, bottom - top, pane.home.rule_color);
                }
            }
            if let Some((row, col, rows)) = pane.home.rail {
                bg_rect.push_rect(gpu, content_x + col as f32 * char_width, row_y(row), home::RAIL_PX, rows as f32 * line_height, theme.caret);
            }
            let corners = if rounded_selection { polish::selection_corners(&pane.selection_segments, 3.0) } else { Vec::new() };
            for (i, &(row, col_start, col_end)) in pane.selection_segments.iter().enumerate() {
                let x = content_x + col_start as f32 * char_width;
                let y = row_y(row);
                let w = (col_end - col_start) as f32 * char_width;
                match corners.get(i) {
                    Some(&radii) => bg_rect.push_corners(gpu, x, y, w, line_height, radii, theme.selection),
                    None => bg_rect.push_rect(gpu, x, y, w, line_height, theme.selection),
                }
            }
            for &(row, col_start, col_end) in &pane.bracket_match_segments {
                let x = content_x + col_start as f32 * char_width;
                let y = row_y(row);
                let w = (col_end - col_start) as f32 * char_width;
                bg_rect.push_rect(gpu, x, y, w, line_height, theme.bracket_match);
            }
            for &(row, col_start, col_end) in &pane.hlsearch_segments {
                let x = content_x + col_start as f32 * char_width;
                let y = row_y(row);
                let w = (col_end - col_start) as f32 * char_width;
                bg_rect.push_rect(gpu, x, y, w, line_height, theme.search_match);
            }
            if let Some((segments, alpha)) = &pane.pulse_overlay {
                let [r, g, b, _] = theme.caret;
                for &(row, col_start, col_end) in segments {
                    let x = content_x + col_start as f32 * char_width;
                    let y = row_y(row);
                    let w = (col_end - col_start) as f32 * char_width;
                    bg_rect.push_rect(gpu, x, y, w, line_height, [r, g, b, *alpha]);
                }
            }
            // The jump beacon: the whole landing row, once.
            for &(row, color) in &pane.extra.row_flashes {
                bg_rect.push_rect(gpu, pane.rect.x, row_y(row), pane.rect.w, line_height, color);
            }
            // What an undo, redo, repeat or substitution added...
            for &(row, col_start, col_end, color) in &pane.extra.cell_flashes {
                let x = content_x + col_start as f32 * char_width;
                bg_rect.push_rect(gpu, x, row_y(row), (col_end.max(col_start + 1) - col_start) as f32 * char_width, line_height, color);
            }
            // ...and where it took text away.
            for &(row, col, lines, color) in &pane.extra.seams {
                if lines {
                    bg_rect.push_rect(gpu, content_x, row_y(row) - 1.0, pane.rect.x + pane.rect.w - content_x, 2.0, color);
                } else {
                    bg_rect.push_rect(gpu, content_x + col as f32 * char_width - 1.0, row_y(row), 2.0, line_height, color);
                }
            }
        }
        // Sticky scroll's rows sit on the plain background, over whatever
        // highlight the rows beneath them had.
        for pane in panes_render.iter().filter(|p| !p.extra.sticky.is_empty()) {
            let h = text::PAD_TOP + pane.extra.sticky.len() as f32 * line_height;
            bg_rect.push_rect(gpu, pane.rect.x, pane.rect.y, pane.rect.w, h, theme.bg);
        }

        bg_rect.push_rect(gpu, 0.0, modeline_top, window_width, modeline_height, theme.bg_modeline);
        // A full-height accent rail instead of a filled badge box behind
        // the mode label -- Signal Rail's own "read the mode from the
        // eye-corner" bit, borrowed from a mixing console's channel-status
        // LED. Narrower than `PAD_LEFT`, so the label text (which still
        // starts rendering at `PAD_LEFT`, per `text.rs`'s modeline
        // TextArea) always has a small gap of plain modeline background
        // between it and the rail, without needing its own tuned offset.
        // Always drawn: `modeline_pieces` always returns real badge
        // content (a capturing prompt's own text lives in its own popup
        // instead of blanking this out), so there's no "no mode" state to
        // skip this for.
        const MODE_RAIL_WIDTH: f32 = 5.0;
        bg_rect.push_rect(gpu, 0.0, modeline_top, MODE_RAIL_WIDTH, modeline_height, badge_bg);
        // The design's two vertical hairlines: one closing the mode
        // segment, one opening the clock's. They're what make the
        // modeline read as rail | mode | info | clock -- real dividers
        // that are part of the layout rather than more separator dots
        // competing with the `·` ones already inside the info run.
        //
        // Both are placed from `modeline_char_width`-based column math
        // against the same `PAD_LEFT` origin the modeline's own TextArea
        // renders from, so they land in the text's own gaps rather than
        // through a glyph.
        // Both x positions are rounded to whole pixels: a 1px-wide rect at
        // a fractional x rasterizes across two columns at partial coverage
        // each, which on an already-faint rule is the difference between a
        // hairline and a smudge you can't see at all.
        let rule = modeline_rule_color(theme);
        let mode_divider_x = (text::PAD_LEFT + (mode_segment_chars as f32 - 1.0) * modeline_char_width).round();
        bg_rect.push_rect(gpu, mode_divider_x, modeline_top, 1.0, modeline_height, rule);
        // One `PAD_LEFT` gap ahead of the clock's own measured left edge.
        // `None` when the clock is hidden, which is also exactly when this
        // rule should be skipped -- a rule with nothing after it would
        // just be a stray mark at the right edge.
        if let Some(clock_left) = clock_left {
            bg_rect.push_rect(gpu, (clock_left - text::PAD_LEFT).round(), modeline_top, 1.0, modeline_height, rule);
        }
        // Popup backgrounds are deliberately *not* pushed into this batch --
        // see the big comment at the two-pass render sequence below for why.
        if show_sidebar {
            // `theme.sidebar_bg`, not `theme.bg_modeline` -- the sidebar
            // shows file/directory names with the same icon/syntax-
            // adjacent colors the main content area uses (dark, saturated
            // colors meant to read on `theme.bg`, per `TEMPLEOS`'s own doc
            // comment), so it needs a *content*-shaped background, not the
            // modeline's. The previous `bg_modeline` choice produced an
            // unreadable blue-background/black-text combination on the
            // TempleOS theme specifically (ORBIT_DARK's own bg/bg_modeline
            // are close enough in value that this went unnoticed there).
            // `sidebar_bg` defaults to exactly `bg` on every theme but one
            // (see its own doc comment), so this is only a real change for
            // a theme that deliberately wants the two surfaces distinct.
            let panel = panels::sidebar_in(look, sidebar_width, modeline_top - dock_h - terminal_h);
            bg_rect.push_box(gpu, panel.x, panel.y, panel.w, panel.h, crate::rect::BoxStyle { radius: panel_radius, fill: theme.sidebar_bg, border: 0.0, border_color: [0.0; 4] });
            if let Some((_, _, tinted, _)) = &sidebar_render {
                // Marked rows, and the file being edited.
                for &row in tinted {
                    bg_rect.push_rect(gpu, panel.x, sidebar_row_y(row), panel.w, line_height, theme.hl_line);
                }
            }
            if let Some((_, Some(selected_row), _, _)) = &sidebar_render {
                // `theme.selection`, not `theme.hl_line`: the sidebar has no
                // caret of its own, so this highlight is the *only* cue for
                // which entry is selected and needs to actually stand out --
                // matches the picker's own selected-row highlight (which
                // already uses `theme.selection` above) rather
                // than the subtle current-line tint meant to be a secondary
                // cue alongside a visible caret.
                let y = sidebar_row_y(*selected_row);
                bg_rect.push_rect(gpu, panel.x, y, panel.w, line_height, theme.selection);
            }
            if let Some((_, _, _, underlines)) = &sidebar_render {
                for &(row, start, end) in underlines {
                    let y = (sidebar_row_y(row) + line_height - 2.0).round();
                    bg_rect.push_rect(gpu, text::PAD_LEFT + text::STRIP_INSET + start as f32 * char_width, y, (end - start) as f32 * char_width, 1.0, theme.caret);
                }
            }
        }
        if dock_h > 0.0 {
            let dock_top = modeline_top - dock_h;
            let panel = panels::strip_in(look, window_width, dock_top, dock_h);
            bg_rect.push_box(gpu, panel.x, panel.y, panel.w, panel.h, crate::rect::BoxStyle { radius: panel_radius, fill: theme.bg, border: 0.0, border_color: [0.0; 4] });
            // Concept A's drawer: its text is a grid; behind it, the
            // state's chip and the keys' caps in the header, a card per
            // column -- the one with the keys outlined -- and the
            // evaluate line's box.
            let text_top = dock_top + DOCK_TEXT_Y;
            let row_y = |row: usize| text_top + row as f32 * line_height;
            let col_x = |col: usize| DOCK_TEXT_X + col as f32 * char_width;
            let pad = (char_width * 0.8).round();
            let faded = |c: [f32; 4], a: f32| [c[0], c[1], c[2], a];
            let plain = |fill: [f32; 4], radius: f32| crate::rect::BoxStyle { radius, fill, border: 0.0, border_color: [0.0; 4] };
            if let Some((row, a, b, stopped)) = dock_decor.status {
                let color = if stopped { glyphon_to_rgba(theme.git_modified) } else { glyphon_to_rgba(theme.gutter_fg) };
                let r = panels::key_cap(DOCK_TEXT_X, char_width, row_y(row), line_height, a, b);
                bg_rect.push_box(gpu, r.x, r.y, r.w, r.h, plain(faded(color, 0.16), r.h / 2.0));
            }
            for &(row, a, b) in &dock_decor.caps {
                let r = panels::key_cap(DOCK_TEXT_X, char_width, row_y(row), line_height, a, b);
                let outline = faded(glyphon_to_rgba(theme.fg), 0.16);
                bg_rect.push_box(gpu, r.x, r.y, r.w, r.h, crate::rect::BoxStyle { radius: panels::KEY_RADIUS, fill: theme.input, border: 1.0, border_color: outline });
            }
            let last_row = debug_drawer::TITLE_ROW + debug_drawer::LIST_ROWS;
            for (c, &(x, w)) in dock_decor.columns.iter().enumerate() {
                let r = panels::card_rect(col_x(x) - pad, w as f32 * char_width + pad * 2.0, text_top, line_height, debug_drawer::TITLE_ROW, last_row);
                let focus = dock_decor.focused == Some(c);
                let style = crate::rect::BoxStyle { radius: panels::CARD_RADIUS, fill: theme.card, border: if focus { 1.5 } else { 0.0 }, border_color: if focus { theme.mode_command } else { [0.0; 4] } };
                bg_rect.push_box(gpu, r.x, r.y, r.w, r.h, style);
            }
            if let Some((row, col, width)) = debug_drawer_highlight {
                let x = col_x(col) - pad * 0.5;
                bg_rect.push_box(gpu, x, row_y(row), width as f32 * char_width + pad, line_height, plain(theme.selection, panels::KEY_RADIUS));
            }
            // As wide as the cards above it.
            let across = dock_decor.columns.last().map(|&(x, w)| x + w).unwrap_or(dock_cols);
            let eval = panels::card_rect(col_x(0) - pad, across as f32 * char_width + pad * 2.0, text_top, line_height, debug_drawer::EVAL_ROW, debug_drawer::EVAL_ROW);
            let typing = dock_decor.typing;
            bg_rect.push_box(gpu, eval.x, eval.y, eval.w, eval.h, crate::rect::BoxStyle { radius: panels::CARD_RADIUS, fill: theme.input, border: if typing { 1.0 } else { 0.0 }, border_color: if typing { faded(theme.caret, 0.6) } else { [0.0; 4] } });
        }
        if show_terminal {
            let terminal_top = modeline_top - dock_h - terminal_h;
            // Base fill, matching what a "default background" cell
            // resolves to (`terminal_view`: none of its own, so the page)
            // -- so an ordinary cell blends seamlessly into the strip
            // rather than needing its own explicit background segment.
            let panel = panels::strip_in(look, window_width, terminal_top, terminal_h);
            bg_rect.push_box(gpu, panel.x, panel.y, panel.w, panel.h, crate::rect::BoxStyle { radius: panel_radius, fill: theme.bg, border: 0.0, border_color: [0.0; 4] });
            if let Some(view) = &terminal_render {
                let cell_x = |col: usize| text::PAD_LEFT + text::STRIP_INSET + col as f32 * char_width;
                let row_y = |row: usize| terminal_top + text::PAD_TOP + text::STRIP_INSET / 2.0 + row as f32 * line_height;
                for &(row, col_start, col_end, color) in &view.backgrounds {
                    bg_rect.push_rect(gpu, cell_x(col_start), row_y(row), (col_end - col_start) as f32 * char_width, line_height, color);
                }
                for &(row, col_start, col_end, color) in &view.underlines {
                    bg_rect.push_rect(gpu, cell_x(col_start), (row_y(row) + line_height - 2.0).round(), (col_end - col_start) as f32 * char_width, 1.0, color);
                }
                if let Some((row, a, b)) = terminal_link {
                    bg_rect.push_rect(gpu, cell_x(a), (row_y(row) + line_height - 2.0).round(), (b - a) as f32 * char_width, 1.0, theme.caret);
                }
                for &(row, col_start, col_end) in &terminal_selection {
                    bg_rect.push_rect(gpu, cell_x(col_start), row_y(row), (col_end - col_start) as f32 * char_width, line_height, theme.selection);
                }
            }
        }
        // Framed, no lines between panes: the tray's gap is the seam. Flat,
        // the lines it had before 1.4: between panes, along the sidebar's
        // edge, over the terminal, the debug drawer and the modeline.
        if look == panels::Look::Flat {
            for pane in &panes_render {
                let chrome_h = pane_chrome_height(line_height, pane.has_breadcrumb);
                let (top, h) = (pane.rect.y - chrome_h, pane.rect.h + chrome_h);
                if pane.rect.x + pane.rect.w < pane_area.x + pane_area.w - 0.5 {
                    bg_rect.push_rect(gpu, pane.rect.x + pane.rect.w - 1.0, top, 2.0, h, theme.divider);
                }
                if pane.rect.y + pane.rect.h < pane_area.y + pane_area.h - 0.5 {
                    bg_rect.push_rect(gpu, pane.rect.x, pane.rect.y + pane.rect.h - 1.0, pane.rect.w, 2.0, theme.divider);
                }
            }
            if show_sidebar {
                bg_rect.push_rect(gpu, sidebar_width - 1.0, 0.0, 2.0, modeline_top - dock_h - terminal_h, theme.divider);
            }
            if show_terminal {
                bg_rect.push_rect(gpu, 0.0, modeline_top - dock_h - terminal_h, window_width, 2.0, theme.divider);
            }
            if dock_h > 0.0 {
                bg_rect.push_rect(gpu, 0.0, modeline_top - dock_h, window_width, 2.0, theme.divider);
            }
            bg_rect.push_rect(gpu, 0.0, modeline_top, window_width, 1.0, theme.divider);
        }
        // Title-bar backgrounds -- same `bg_modeline` color the modeline
        // itself uses, so a titled pane reads as "a small modeline for
        // just this section" rather than an unrelated new chrome color.
        // A pane with a tab strip (`pane.tabs_layout`) additionally gets
        // one `theme.bg` rect per *active* tab, layered on top of that
        // flat `bg_modeline` fill -- the same "active tab matches the
        // editor canvas, inactive tabs blend into the chrome" convention
        // real VS/VS-Code-style tab strips use -- plus a thin `theme.
        // divider` line closing off the strip's own bottom edge (the
        // divider loop above only draws *between* panes, not under a
        // pane's own title/tab strip).
        let top_corners = [panel_radius, panel_radius, 0.0, 0.0];
        for &(pane, rect) in &title_rects {
            // A pane with a title rather than tabs: the agents' dock and
            // the other tool panels. Their title is on the panel itself.
            let fill = if Some(pane) == composer_pane {
                theme.input
            } else if panes_render.iter().any(|p| p.pane == pane && p.tabs_layout.is_empty()) {
                theme.bg
            } else {
                theme.bg_modeline
            };
            bg_rect.push_corners(gpu, rect.x, rect.y, rect.w, rect.h, top_corners, fill);
        }
        for pane in &panes_render {
            if pane.tabs_layout.is_empty() {
                continue;
            }
            let strip_y = pane.rect.y - pane_chrome_height(line_height, pane.has_breadcrumb);
            let strip_h = tab_strip_height(line_height);
            let breadcrumb_y = strip_y + strip_h;
            let breadcrumb_h = if pane.has_breadcrumb { breadcrumb_strip_height(line_height) } else { 0.0 };
            // Breadcrumbs are navigation chrome, but a shade closer to the
            // editing canvas than the tab row -- and, per the design's own
            // `.crumbs-b`, a visibly shorter bar than the tab row above it
            // (`breadcrumb_h` < `strip_h`), not just a second copy of the
            // same-height strip. This gives the two rows a deliberate
            // hierarchy rather than making them look like two unrelated
            // toolbars of identical weight.
            let active = pane.tabs_layout.iter().zip(&pane.tab_active).find(|(_, a)| **a).map(|(t, _)| t);
            if tab_style == Some(crate::theme::TabStyle::Underline) {
                // Look B: flat on the editor's own background. The active
                // tab is its bright text over an underline in the mode's
                // colour -- muted in a pane without the keyboard -- and a
                // hairline sets Home apart from the tabs after it.
                bg_rect.push_corners(gpu, pane.rect.x, strip_y, pane.rect.w, strip_h + breadcrumb_h, top_corners, theme.bg);
                bg_rect.push_rect(gpu, pane.rect.x, strip_y + strip_h - 1.0, pane.rect.w, 1.0, theme.divider);
                if let Some(home) = pane.tabs_layout.first() {
                    let x = (home.body.x + home.body.w + 2.0).round();
                    bg_rect.push_rect(gpu, x, strip_y + 7.0, 1.0, (strip_h - 14.0).max(4.0), theme.divider);
                }
                if let Some(tab) = active {
                    let key = (frame_ws.0, frame_ws.1, pane.pane);
                    let (x, w) = motion_state.tab_accent(prefs, key, (tab.body.x, tab.body.w), now);
                    let color = if pane.pane == focused_pane { badge_bg } else { glyphon_to_rgba(theme.gutter_fg) };
                    bg_rect.push_rect(gpu, x, strip_y + strip_h - 2.0, w, 2.0, color);
                }
            } else {
                // Breadcrumbs are navigation chrome, but a shade closer to
                // the editing canvas than the tab row, and a visibly
                // shorter bar than it -- a deliberate hierarchy.
                bg_rect.push_rect(gpu, pane.rect.x, breadcrumb_y, pane.rect.w, breadcrumb_h, theme.sidebar_bg);
                bg_rect.push_rect(gpu, pane.rect.x, strip_y + strip_h - 1.0, pane.rect.w, 1.0, theme.divider);
                for (tab, &is_active) in pane.tabs_layout.iter().zip(&pane.tab_active) {
                    if is_active {
                        bg_rect.push_rect(gpu, tab.body.x, strip_y, tab.body.w, strip_h, theme.bg);
                    }
                    bg_rect.push_rect(gpu, tab.body.x + tab.body.w - 1.0, strip_y, 1.0, strip_h, theme.divider);
                }
                // A top-edge accent bar marks the selected tab -- Signal
                // Rail's own convention (mirrors the modeline's mode rail)
                // -- sliding over from the tab you were on.
                if let Some(tab) = active {
                    let key = (frame_ws.0, frame_ws.1, pane.pane);
                    let (x, w) = motion_state.tab_accent(prefs, key, (tab.body.x, tab.body.w), now);
                    bg_rect.push_rect(gpu, x, strip_y, w, 3.0, theme.mode_normal);
                }
            }
            if pane.has_breadcrumb {
                bg_rect.push_rect(gpu, pane.rect.x, breadcrumb_y + breadcrumb_h - 1.0, pane.rect.w, 1.0, theme.divider);
            }
        }
        // Inline git gutter marks (Part 5) -- a thin colored bar, one per
        // changed line still in view. Sits within the pane's own left
        // margin (`text::PAD_LEFT`), *before* the line-number digits
        // (which start exactly at `PAD_LEFT`) rather than squeezed into
        // the ~1-char gap between the digits and the code -- the same
        // "indicator strip, then line numbers, then code" left-to-right
        // order real VS/VS Code use, and the only placement that leaves
        // real breathing room on both sides of the bar without changing
        // `gutter_chars`' own width (which every caret/selection
        // position calculation elsewhere already assumes). Reuses the
        // theme's existing git-status colors (already `glyphon::Color`,
        // converted via `glyphon_to_rgba`) rather than adding dedicated
        // fields -- every theme gets this for free, not just `VISUAL_
        // STUDIO_DARK`.
        const GUTTER_MARK_WIDTH: f32 = 3.0;
        const GUTTER_MARK_MARGIN: f32 = 2.0;
        for pane in &panes_render {
            for &(row, kind) in &pane.gutter_marks {
                let color = glyphon_to_rgba(match kind {
                    GutterMarkKind::Added => theme.git_staged,
                    GutterMarkKind::Modified => theme.git_modified,
                    GutterMarkKind::Deleted => theme.git_conflicted,
                });
                let x = pane.rect.x + GUTTER_MARK_MARGIN;
                let y = pane.rect.y + text::PAD_TOP + row as f32 * line_height - pane.content_frac * line_height;
                bg_rect.push_rect(gpu, x, y, GUTTER_MARK_WIDTH, line_height, color);
            }
            // A Jira comment's or Agenda note's own left-margin accent --
            // same geometry as a gutter mark, just never sharing a pane
            // with one (a Jira/Agenda buffer has no git gutter marks of
            // its own), so reusing the identical bar position reads as
            // "the same kind of thing" rather than a second convention.
            for &(row, color) in &pane.row_accents {
                let x = pane.rect.x + GUTTER_MARK_MARGIN;
                let y = pane.rect.y + text::PAD_TOP + row as f32 * line_height - pane.content_frac * line_height;
                bg_rect.push_rect(gpu, x, y, GUTTER_MARK_WIDTH, line_height, color);
            }
            // The debugger's marks, past the git bar: a breakpoint's square,
            // the stop's bar, or the stop's bar inside a breakpoint's square.
            // The Ember Trail: a stripe at the gutter's inner edge on each
            // line stopped on, cooling a step at a time.
            for &(row, color) in &pane.trail_marks {
                let x = pane.rect.x + text::PAD_LEFT + pane.gutter_px - 6.0;
                let y = pane.rect.y + text::PAD_TOP + row as f32 * line_height - pane.content_frac * line_height;
                bg_rect.push_rect(gpu, x, y, 3.0, line_height, color);
            }
            for &(row, mark) in &pane.debug_marks {
                let size = (line_height * 0.5).round();
                let x = pane.rect.x + GUTTER_MARK_MARGIN + GUTTER_MARK_WIDTH + 3.0;
                let top = pane.rect.y + text::PAD_TOP + row as f32 * line_height - pane.content_frac * line_height;
                let y = top + ((line_height - size) / 2.0).round();
                let stop = glyphon_to_rgba(theme.git_modified);
                // A breakpoint's square: with a hole for a condition, as two
                // bars for a logpoint, dim and hollow when it's off.
                let breakpoint = |look: debug_breakpoints::BreakpointLook, agent: bool, bg_rect: &mut RectRenderer| {
                    use debug_breakpoints::BreakpointLook as L;
                    let color = glyphon_to_rgba(if look == L::Off { theme.gutter_fg } else if agent { theme.git_staged } else { theme.git_conflicted });
                    match look {
                        L::Plain => bg_rect.push_rect(gpu, x, y, size, size, color),
                        L::Log => {
                            bg_rect.push_rect(gpu, x, y, size, (size * 0.35).round(), color);
                            bg_rect.push_rect(gpu, x, y + (size * 0.65).round(), size, (size * 0.35).round(), color);
                        }
                        L::Conditional | L::Off => {
                            bg_rect.push_rect(gpu, x, y, size, size, color);
                            bg_rect.push_rect(gpu, x + size * 0.3, y + size * 0.3, size * 0.4, size * 0.4, theme.bg);
                        }
                    }
                };
                match mark {
                    debug_host::DebugMark::Breakpoint(look) => breakpoint(look, false, &mut *bg_rect),
                    debug_host::DebugMark::AgentBreakpoint(look) => breakpoint(look, true, &mut *bg_rect),
                    debug_host::DebugMark::Stop => bg_rect.push_rect(gpu, x, top + 2.0, size * 0.6, line_height - 4.0, stop),
                    debug_host::DebugMark::StopOnBreakpoint(look) => {
                        breakpoint(look, false, &mut *bg_rect);
                        bg_rect.push_rect(gpu, x + size * 0.25, y + size * 0.25, size * 0.5, size * 0.5, stop);
                    }
                }
            }
        }
        // Indentation guides -- a thin vertical line per indent level
        // within a line's own leading whitespace (`indent_guide_columns`).
        // Blended from `theme.bg` towards `theme.fg` rather than off
        // `theme.divider`: a theme's divider is tuned as its own subtle
        // structural line and, for Visual Studio Dark specifically, sits
        // only ~15/255 above `bg` (real VS's own splitter is nearly
        // invisible) -- fine for a divider but too faint once dimmed
        // further for a guide meant to actually read on screen. `fg`'s
        // contrast against `bg` is large and guaranteed by this file's
        // own contrast tests for every theme, so blending a small slice
        // of it in gives every theme a guide that's actually visible
        // without a new `Theme` field, still weighted light enough to
        // stay polish rather than noise.
        const INDENT_GUIDE_MIX: f32 = 0.16;
        let guide_fg = glyphon_to_rgba(theme.fg);
        let indent_guide_color = [
            theme.bg[0] + (guide_fg[0] - theme.bg[0]) * INDENT_GUIDE_MIX,
            theme.bg[1] + (guide_fg[1] - theme.bg[1]) * INDENT_GUIDE_MIX,
            theme.bg[2] + (guide_fg[2] - theme.bg[2]) * INDENT_GUIDE_MIX,
            1.0,
        ];
        for pane in panes_render.iter().filter(|_| show_indent_guides) {
            let content_x = pane.rect.x + text::PAD_LEFT + pane.gutter_px;
            let active_guide = pane.caret.filter(|_| show_active_guide).and_then(|(row, _)| {
                pane.indent_guides.iter().filter(|(r, _)| *r == row).map(|(_, col)| *col).max()
                    .map(|col| (row, col))
            });
            for (first_row, last_row, col) in indent_guide_runs(&pane.indent_guides) {
                let x = content_x + col as f32 * char_width;
                let y = pane.rect.y + text::PAD_TOP + first_row as f32 * line_height - pane.content_frac * line_height;
                let height = (last_row - first_row + 1) as f32 * line_height;
                let active = active_guide.is_some_and(|(row, column)| column == col && first_row <= row && row <= last_row);
                let color = if active { glyphon_to_rgba(theme.gutter_fg) } else { indent_guide_color };
                bg_rect.push_rect(gpu, x, y, 1.0, height, color);
            }
        }
        // The focus edge: round the panel with the keyboard, over what's in
        // it, when there's more than one to tell apart. Framed only; flat
        // keeps the look it had, the focused pane's title in the accent.
        let panels_shown = panes_render.len() + usize::from(show_sidebar) + usize::from(show_terminal) + usize::from(dock_h > 0.0);
        if look == panels::Look::Framed && panels_shown > 1 && overlays_here {
            let edge_rect = match keyboard_panel {
                panels::KeyboardPanel::Pane(id) => panes_render.iter().find(|p| p.pane == id).map(|p| {
                    let chrome_h = pane_chrome_height(line_height, p.has_breadcrumb);
                    fenix_window::Rect { x: p.rect.x, y: p.rect.y - chrome_h, w: p.rect.w, h: p.rect.h + chrome_h }
                }),
                panels::KeyboardPanel::Sidebar => show_sidebar.then(|| panels::sidebar_in(look, sidebar_width, modeline_top - dock_h - terminal_h)),
                panels::KeyboardPanel::Terminal => show_terminal.then(|| panels::strip_in(look, window_width, modeline_top - dock_h - terminal_h, terminal_h)),
                panels::KeyboardPanel::Dock => (dock_h > 0.0).then(|| panels::strip_in(look, window_width, modeline_top - dock_h, dock_h)),
            };
            if let Some(r) = edge_rect {
                let style = crate::rect::BoxStyle { radius: panel_radius, fill: [0.0; 4], border: 1.0, border_color: panels::focus_edge(badge_bg) };
                bg_rect.push_box(gpu, r.x, r.y, r.w, r.h, style);
            }
        }
        // Layered last -- on top of everything else pushed to `bg_rect`
        // this frame (modeline bar, hl-line, selection, which-key/sidebar
        // backgrounds) -- so a themed border reads as the outermost frame
        // around the whole window rather than something those cover.
        // `BORDER_WIDTH` sits comfortably inside `PAD_TOP`/`PAD_LEFT`, so
        // it can't clip into body text.
        if let Some(border) = theme.border {
            bg_rect.push_rect(gpu, 0.0, 0.0, window_width, BORDER_WIDTH, border);
            bg_rect.push_rect(gpu, 0.0, window_height - BORDER_WIDTH, window_width, BORDER_WIDTH, border);
            bg_rect.push_rect(gpu, 0.0, 0.0, BORDER_WIDTH, window_height, border);
            bg_rect.push_rect(gpu, window_width - BORDER_WIDTH, 0.0, BORDER_WIDTH, window_height, border);
        }
        bg_rect.flush(gpu);

        caret_rect.clear();
        let mut caret_px: Option<(f32, f32)> = None;
        if let Some(focused) = panes_render.iter().find(|p| p.pane == focused_pane) {
            if let Some((row, col)) = focused.caret {
                // Where the caret is drawn: gliding there after a motion.
                let (mut caret_x, mut caret_y) =
                    caret_pixel_pos(focused.rect, row, col, focused.gutter_px, focused.content_frac, char_width, line_height);
                if let Some(buffer) = focused.extra.buffer {
                    let extra = &focused.extra;
                    let spot = motion_host::CaretSpot {
                        pane: (frame_ws.0, frame_ws.1, focused.pane),
                        buffer,
                        row: (row + extra.base_line) as f32,
                        col: (col + extra.scroll_col) as f32,
                    };
                    let visible = text::lines_that_fit(focused.rect.h, line_height);
                    let (shown_row, shown_col) = motion_state.caret_spot(prefs, spot, visible, now);
                    caret_x += (shown_col - spot.col) * char_width;
                    caret_y += (shown_row - spot.row) * line_height;
                }
                caret_px = Some((caret_x, caret_y));
                if caret_alpha > 0.0 {
                    let [r, g, b, a] = theme.caret;
                    // Insert keeps the thin bar (an I-beam-style "about to
                    // type here" marker); every other mode (Normal, Visual,
                    // Replace, Command) gets a full-cell block, matching real
                    // Vim's own cursor-shape convention. The block is drawn
                    // at reduced opacity -- caret_rect is composited *after*
                    // text (so the caret always shows on top, no glyph-recolor
                    // trick needed), and a fully opaque block would otherwise
                    // completely hide the character underneath it instead of
                    // just marking its position.
                    let (width, block_alpha) = if caret_is_block { (char_width, 0.6) } else { (2.0, 1.0) };
                    caret_rect.push_rect(gpu, caret_x, caret_y, width, line_height, [r, g, b, a * caret_alpha * block_alpha]);
                    // The extra cursors, where they are (no glide).
                    for &(row, col) in &focused.extra_carets {
                        let (x, y) = caret_pixel_pos(focused.rect, row, col, focused.gutter_px, focused.content_frac, char_width, line_height);
                        caret_rect.push_rect(gpu, x, y, width, line_height, [r, g, b, a * caret_alpha * block_alpha]);
                    }
                }
            }
        }
        caret_rect.flush(gpu);

        // The extensions' layers: where things are this frame, then what
        // each level draws.
        self.wasm.layers.geometry = layers::Geometry {
            cell: (char_width, line_height),
            window: (window_width, window_height),
            modeline_top,
            panes: panes_render
                .iter()
                .map(|p| layers::GeoPane {
                    id: p.pane.number() as u64,
                    rect: p.rect,
                    text_x: p.rect.x + text::PAD_LEFT + p.gutter_px,
                    text_y: p.rect.y + text::PAD_TOP - p.content_frac * line_height,
                    base_line: p.extra.base_line,
                    scroll_col: p.extra.scroll_col,
                    rows: text::lines_that_fit(p.rect.h, line_height),
                    focused: p.pane == focused_pane,
                    map: p.extra.rows.clone(),
                })
                .collect(),
            caret: caret_px,
        };
        let layer_motion = crate::motion::level(&self.config) != crate::motion::Level::Off;
        {
            let (w, h) = (gpu.config.width, gpu.config.height);
            let motion_host::MotionGpu { layers_below, layers_above, layers_top, layer_sheets, layer_models, .. } = &mut *motion_gpu;
            let fg = [theme.fg.r(), theme.fg.g(), theme.fg.b()].map(|c| c as f32 / 255.0);
            let lighting = crate::model3d::Lighting::from_theme(theme.bg, fg, theme.caret);
            layer_models.begin();
            for (level, renderer) in [(fenix_wasm::host::LayerLevel::BelowText, layers_below), (fenix_wasm::host::LayerLevel::AboveText, layers_above), (fenix_wasm::host::LayerLevel::AbovePopups, layers_top)] {
                renderer.begin(w, h);
                self.wasm.layers.draw(level, renderer, layer_sheets, layer_models, lighting, now, layer_motion);
                renderer.flush();
            }
            // The models' targets drawn, ahead of the frame's own passes.
            layer_models.finish();
        }

        // Drawn over the text, under popups: panes without the keyboard
        // dimmed, and rows still opening under an unfolded row covered
        // from where they've got to.
        let overlay = &mut motion_gpu.overlay;
        overlay.clear();
        if let Some(amount) = dim_amount.filter(|_| overlays_here) {
            let [r, g, b, _] = theme.bg;
            for pane in panes_render.iter().filter(|p| p.pane != focused_pane) {
                let chrome = pane_chrome_height(line_height, pane.has_breadcrumb);
                overlay.push_rect(gpu, pane.rect.x, pane.rect.y - chrome, pane.rect.w, pane.rect.h + chrome, [r, g, b, amount]);
            }
        }
        let cover = |overlay: &mut RectRenderer, x: f32, w: f32, top: f32, rows: usize, shown: f32, clip: (f32, f32), color: [f32; 4]| {
            let from = (top + rows as f32 * line_height * shown).max(clip.0);
            let to = (top + rows as f32 * line_height).min(clip.1);
            if to > from {
                overlay.push_rect(gpu, x, from, w, to - from, color);
            }
        };
        for pane in &panes_render {
            if let Some((row, rows, shown)) = pane.extra.reveal {
                let top = pane.rect.y + text::PAD_TOP + (row as f32 - pane.content_frac) * line_height;
                cover(overlay, pane.rect.x, pane.rect.w, top, rows, shown, (pane.rect.y, pane.rect.y + pane.rect.h), theme.bg);
            }
        }
        if let Some((first, rows, shown)) = sidebar_reveal.filter(|_| show_sidebar) {
            if first + rows > sidebar_scroll {
                let row = sidebar_rows_at + first.saturating_sub(sidebar_scroll);
                cover(overlay, 0.0, sidebar_width - 1.0, sidebar_row_y(row), rows, shown, (0.0, modeline_top), theme.sidebar_bg);
            }
        }
        for pane in &panes_render {
            let content_x = pane.rect.x + text::PAD_LEFT + pane.gutter_px;
            let row_y = |row: usize| pane.rect.y + text::PAD_TOP + row as f32 * line_height - pane.content_frac * line_height;
            let sticky_h = if pane.extra.sticky.is_empty() { 0.0 } else { text::PAD_TOP + pane.extra.sticky.len() as f32 * line_height };
            // A mark's columns as x and width, cut to the pane's text: a
            // problem at the end of a long line, scrolled sideways, mustn't
            // be drawn over the pane beside it.
            let span_x = |col_start: usize, col_end: usize| -> Option<(f32, f32)> {
                let from = (content_x + col_start as f32 * char_width).max(content_x);
                let to = (content_x + col_end as f32 * char_width).min(pane.rect.x + pane.rect.w);
                (to > from).then_some((from, to - from))
            };
            // An extension's underlines: a buffer's decorations, a canvas's
            // cells.
            for &(row, col_start, col_end, color) in pane.extra.underlines.iter().chain(&pane.home.underlines) {
                let y = (row_y(row) + line_height - 3.0).round();
                if y < pane.rect.y + sticky_h || y > pane.rect.y + pane.rect.h {
                    continue;
                }
                let Some((x, w)) = span_x(col_start, col_end) else { continue };
                overlay.push_rect(gpu, x, y, w, 1.0, color);
            }
            // Struck through: a value the Ember Trail says was.
            for &(row, col_start, col_end, color) in &pane.extra.strikes {
                let y = (row_y(row) + line_height / 2.0).round();
                if y < pane.rect.y + sticky_h || y > pane.rect.y + pane.rect.h {
                    continue;
                }
                let Some((x, w)) = span_x(col_start, col_end) else { continue };
                overlay.push_rect(gpu, x, y, w, 1.0, color);
            }
            // Problems, underlined with a wave.
            for &(row, col_start, col_end, color) in &pane.extra.squiggles {
                let y = row_y(row) + line_height - 4.0;
                if y < pane.rect.y + sticky_h || y > pane.rect.y + pane.rect.h {
                    continue;
                }
                let Some((x, w)) = span_x(col_start, col_end) else { continue };
                overlay.push_wave(gpu, x, y - 1.0, w, 5.0, 6.0, 1.3, 1.2, color);
            }
            // Sticky scroll's edge: a hairline and a soft shadow on the
            // text scrolling under it.
            if sticky_h > 0.0 {
                let bottom = pane.rect.y + sticky_h;
                overlay.push_shadow(gpu, pane.rect.x, bottom - 6.0, pane.rect.w, 6.0, 0.0, 5.0, [0.0, 0.0, 0.0, 0.35]);
                overlay.push_rect(gpu, pane.rect.x, bottom - 1.0, pane.rect.w, 1.0, theme.divider);
            }
            // The overview ruler: the whole file down the right edge.
            if let Some(ruler) = &pane.extra.ruler {
                const RULER_W: f32 = 10.0;
                let x = pane.rect.x + pane.rect.w - RULER_W;
                let (top, h) = (pane.rect.y, pane.rect.h);
                let lines = ruler.lines.max(1) as f32;
                let at = |line: f32| top + (line / lines).clamp(0.0, 1.0) * h;
                let fg = glyphon_to_rgba(theme.fg);
                overlay.push_rect(gpu, x, top, RULER_W, h, [fg[0], fg[1], fg[2], 0.03]);
                overlay.push_rect(gpu, x, top, 1.0, h, [fg[0], fg[1], fg[2], 0.08]);
                let thumb_h = (ruler.visible / lines * h).max(8.0).min(h);
                overlay.push_rect(gpu, x + 1.0, at(ruler.first).min(top + h - thumb_h), RULER_W - 1.0, thumb_h, [fg[0], fg[1], fg[2], 0.07]);
                for &(line, mark) in &ruler.marks {
                    let y = at(line as f32);
                    let (dx, w, mh, color) = match mark {
                        motion_host::RulerMark::Search => (2.0, 4.0, 2.0, theme.search_match),
                        motion_host::RulerMark::Error => (5.0, 5.0, 3.0, glyphon_to_rgba(theme.git_conflicted)),
                        motion_host::RulerMark::Warning => (5.0, 5.0, 3.0, glyphon_to_rgba(theme.git_modified)),
                        motion_host::RulerMark::Info => (5.0, 5.0, 2.0, glyphon_to_rgba(theme.git_untracked)),
                        motion_host::RulerMark::Caret => (2.0, 7.0, 2.0, glyphon_to_rgba(theme.fg)),
                        motion_host::RulerMark::Added => (1.0, 3.0, 4.0, glyphon_to_rgba(theme.git_staged)),
                        motion_host::RulerMark::Modified => (1.0, 3.0, 4.0, glyphon_to_rgba(theme.git_modified)),
                        motion_host::RulerMark::Deleted => (1.0, 3.0, 2.0, glyphon_to_rgba(theme.git_conflicted)),
                    };
                    overlay.push_rect(gpu, x + dx, y.min(top + h - mh), w, mh, [color[0], color[1], color[2], 0.9]);
                }
                overlay.push_rect(gpu, x, at(ruler.cursor as f32).min(top + h - 1.0), RULER_W, 1.0, [fg[0], fg[1], fg[2], 0.8]);
            }
        }
        for &((x, y, w, h), color) in &surface_marks {
            overlay.push_rect(gpu, x, y, w, h, color);
        }
        // A diagram's node: the selected one outlined in the accent, the
        // source cursor's tinted.
        for pane in &panes_render {
            for &(i, (fx, fy, fw, fh), strong) in &pane.home.marks {
                let Some((row, col, rows, cols, key, view)) = pane.home.images.get(i) else { continue };
                let Some(tex) = self.page_images.textures.get(key).filter(|t| t.texture.is_some()) else { continue };
                let cell_box = (
                    pane.rect.x + text::PAD_LEFT + pane.gutter_px + *col as f32 * char_width,
                    pane.rect.y + text::PAD_TOP + (*row as f32 - pane.content_frac) * line_height,
                    *cols as f32 * char_width,
                    *rows as f32 * line_height,
                );
                let clip = (pane.rect.x, pane.rect.y + text::PAD_TOP, pane.rect.x + pane.rect.w, pane.rect.y + pane.rect.h);
                let Some((shown, _, (x, y, w, h))) = reading_host::place_picture(cell_box, clip, (tex.w, tex.h), *view) else { continue };
                let (mx, my, mw, mh) = (x + fx * w - 3.0, y + fy * h - 3.0, fw * w + 6.0, fh * h + 6.0);
                let (x0, y0) = (mx.max(shown.0), my.max(shown.1));
                let (x1, y1) = ((mx + mw).min(shown.0 + shown.2), (my + mh).min(shown.1 + shown.3));
                if x1 <= x0 || y1 <= y0 {
                    continue;
                }
                let accent = theme.caret;
                let fill = if strong { [accent[0], accent[1], accent[2], 0.16] } else { [accent[0], accent[1], accent[2], 0.08] };
                overlay.push_rect(gpu, x0, y0, x1 - x0, y1 - y0, fill);
                if strong {
                    let t = 2.0;
                    let edge = [accent[0], accent[1], accent[2], 0.9];
                    overlay.push_rect(gpu, x0, y0, x1 - x0, t, edge);
                    overlay.push_rect(gpu, x0, y1 - t, x1 - x0, t, edge);
                    overlay.push_rect(gpu, x0, y0, t, y1 - y0, edge);
                    overlay.push_rect(gpu, x1 - t, y0, t, y1 - y0, edge);
                }
            }
        }
        overlay.flush(gpu);

        // The spinner: the mark's blades lit in turn, just left of the
        // work it's for.
        let (surface_w, surface_h) = (gpu.config.width, gpu.config.height);
        motion_gpu.sprites.begin(surface_w, surface_h);
        if let (false, Some(clock_left), true) = (jobs.is_empty(), clock_left, prefs.progress) {
            let size = (modeline_height * 0.62).round().max(8.0);
            let lit = motion_host::spinner_blade(now, motion_host::spinner_epoch());
            let x = (clock_left - size - 4.0).round();
            let y = (modeline_top + (modeline_height - size) / 2.0).round();
            motion_gpu.push_spinner(x, y, size, lit);
        }
        motion_gpu.sprites.flush();

        // Over everything: the old theme's picture fading away, and the
        // launch splash fading out over the first frames.
        motion_gpu.top.begin(surface_w, surface_h);
        if let (Some(anim), Some((_, snapshot))) = (motion_state.theme_fade.filter(|a| a.running(now)), &motion_gpu.snapshot) {
            let alpha = 1.0 - anim.eased(now);
            motion_gpu.top.push(snapshot, 0.0, 0.0, surface_w as f32, surface_h as f32, [1.0, 1.0, 1.0, alpha]);
        }
        motion_gpu.top.flush();
        if let Some(fade) = self.splash_fade.as_mut() {
            let t = now.saturating_duration_since(fade.started).as_secs_f32();
            let alpha = fade.alpha(now);
            fade.sprites.begin(surface_w, surface_h);
            fade.scene.draw(&mut fade.sprites, surface_w, surface_h, t, fade.progress.max(1.0), alpha, true);
            fade.sprites.flush();
        }

        let prepare_panes: Vec<(fenix_window::WindowId, fenix_window::Rect, f32, usize)> =
            panes_render.iter().map(|p| (p.pane, p.rect, p.content_frac, p.extra.sticky.len())).collect();
        text.prepare(gpu, theme, &prepare_panes, &tab_text_rects, &breadcrumb_text_rects, show_sidebar, show_terminal, dock_h);

        // Code extensions' surfaces: what they presented since the last
        // frame, uploaded -- the whole of it when its size changed, else
        // the rectangle that changed -- and the pointer's picture.
        let mut surface_scratch = Vec::new();
        for (buffer_id, _) in &code_surface_panes {
            let Some(pages::PageModel::Code(p)) = self.pages.get_mut(buffer_id).map(|s| &mut s.model) else { continue };
            let Some(cs) = p.surface.as_mut() else { continue };
            let mut fb = cs.shared.lock();
            let (w, h) = fb.size();
            let dirty = fb.take_dirty();
            if let Some(cursor) = fb.take_cursor() {
                cs.cursor = cursor.map(|c| (c, None));
            }
            if w > 0 && h > 0 {
                if cs.texture.as_ref().map(|t| t.size()) != Some((w, h)) {
                    let texture = surfaces.create(gpu, w, h);
                    surfaces.upload_rect(gpu, &texture, 0, 0, w, h, fb.pixels());
                    cs.texture = Some(texture);
                } else if let (Some(texture), Some(rect)) = (&cs.texture, dirty) {
                    fb.copy_region(rect, &mut surface_scratch);
                    surfaces.upload_rect(gpu, texture, rect.0, rect.1, rect.2, rect.3, &surface_scratch);
                }
            }
            drop(fb);
            if let Some((image, made @ None)) = &mut cs.cursor {
                let texture = surfaces.create(gpu, image.width, image.height);
                surfaces.upload_rect(gpu, &texture, 0, 0, image.width, image.height, &image.bgra);
                *made = Some(texture);
            }
        }

        // Every surface quad this frame draws is recorded first and uploaded
        // in a single write before the render pass opens: `queue.write_buffer`
        // is applied ahead of every draw in the pass, so writing per-draw would
        // leave all of them using the last quad written (two panes showing one
        // surface is exactly when that becomes visible: the first drew nothing).
        surfaces.clear();
        // A surface's quad, as its scale places it, and its pointer's
        // picture at the pointer, scaled as its pixels are.
        let mut surface_draws: Vec<(BufferId, usize, Option<usize>)> = Vec::with_capacity(code_surface_panes.len());
        for (buffer_id, rect) in &code_surface_panes {
            let Some(pages::PageModel::Code(p)) = self.pages.get(buffer_id).map(|s| &s.model) else { continue };
            let Some(cs) = &p.surface else { continue };
            if cs.texture.is_none() {
                continue;
            }
            let Some((dest, uv)) = cs.placed((rect.x, rect.y, rect.w, rect.h)) else { continue };
            let slot = surfaces.push(gpu, dest, uv, None);
            let cursor_slot = (|| {
                let ((image, Some(_)), (px, py)) = (cs.cursor.as_ref()?, cs.pointer?) else { return None };
                let (sw, sh) = cs.texture.as_ref()?.size();
                let (kx, ky) = (dest.2 / ((uv.2 - uv.0) * sw as f32), dest.3 / ((uv.3 - uv.1) * sh as f32));
                let x = dest.0 + (px as f32 - uv.0 * sw as f32 - image.hot_x as f32) * kx;
                let y = dest.1 + (py as f32 - uv.1 * sh as f32 - image.hot_y as f32) * ky;
                Some(surfaces.push(gpu, (x, y, image.width as f32 * kx, image.height as f32 * ky), crate::surface::FULL, None))
            })();
            surface_draws.push((*buffer_id, slot, cursor_slot));
        }


        // Pictures pages show, read since the last frame: uploaded once.
        for tex in self.page_images.textures.values_mut() {
            if let (None, Some(bgra)) = (&tex.texture, &tex.bgra) {
                if tex.w > 0 && tex.h > 0 {
                    let texture = surfaces.create(gpu, tex.w, tex.h);
                    surfaces.upload_rect(gpu, &texture, 0, 0, tex.w, tex.h, bgra);
                    tex.texture = Some(texture);
                }
            }
        }

        // Home's logo: drawn (by fenix-brand) at exactly the pixel height
        // its rows give it, so it's crisp at any font size, and uploaded
        // only when that height or the theme's text colour changes.
        if let Some(lines) = panes_render.iter().find_map(|p| p.home.logo.map(|(_, _, lines)| lines)) {
            let height = (lines as f32 * line_height * 0.8).round().max(8.0) as u32;
            let key = (height, [theme.fg.r(), theme.fg.g(), theme.fg.b()]);
            if self.home_logo.as_ref().is_none_or(|logo| logo.key != key) {
                self.home_logo = Some(home::HomeLogo { key, image: fenix_brand::lockup(height, key.1), texture: None });
            }
            if let Some(logo) = self.home_logo.as_mut() {
                if logo.texture.is_none() {
                    let texture = surfaces.create(gpu, logo.image.width, logo.image.height);
                    surfaces.upload_rect(gpu, &texture, 0, 0, logo.image.width, logo.image.height, &logo.image.to_bgra());
                    logo.texture = Some(texture);
                }
            }
        }

        // Every other quad this frame draws, after the surfaces: Home's
        // logo, and the pictures on pages fitted into the cells their page
        // left for them. Resolved back to their surfaces inside the pass.
        // In every pane showing a Home, not only the first.
        let logo_slots: Vec<usize> = match self.home_logo.as_ref().filter(|logo| logo.texture.is_some()) {
            None => Vec::new(),
            Some(logo) => panes_render
                .iter()
                .filter_map(|pane| {
                    let (row, col, lines) = pane.home.logo?;
                    let (w, h) = (logo.image.width as f32, logo.image.height as f32);
                    let x = pane.rect.x + text::PAD_LEFT + pane.gutter_px + col as f32 * char_width;
                    let top = pane.rect.y + text::PAD_TOP + (row as f32 - pane.content_frac) * line_height;
                    let y = top + (lines as f32 * line_height - h) / 2.0;
                    Some(surfaces.push(gpu, (x.round(), y.round(), w, h), crate::surface::FULL, None))
                })
                .collect(),
        };
        let mut picture_draws: Vec<(&crate::page::ImageKey, usize)> = Vec::new();
        for pane in &panes_render {
            for (row, col, rows, cols, key, view) in &pane.home.images {
                let Some(tex) = self.page_images.textures.get(key) else { continue };
                if tex.texture.is_none() {
                    continue;
                }
                let cell_box = (
                    pane.rect.x + text::PAD_LEFT + pane.gutter_px + *col as f32 * char_width,
                    pane.rect.y + text::PAD_TOP + (*row as f32 - pane.content_frac) * line_height,
                    *cols as f32 * char_width,
                    *rows as f32 * line_height,
                );
                let clip = (pane.rect.x, pane.rect.y + text::PAD_TOP, pane.rect.x + pane.rect.w, pane.rect.y + pane.rect.h);
                let Some((dest, uv, _)) = reading_host::place_picture(cell_box, clip, (tex.w, tex.h), *view) else { continue };
                picture_draws.push((key, surfaces.push(gpu, (dest.0.round(), dest.1.round(), dest.2.round(), dest.3.round()), uv, None)));
            }
        }
        surfaces.flush(gpu);

        _profile.mark("text and geometry preparation complete");
        let Some(surface) = &gpu.surface else {
            // Only ever on loan to the launch splash, which hands it back
            // before the first frame. Gone otherwise, the window would
            // never be drawn again, so it gets a new one.
            if gpu.surface_missing() && !self.gpu_watch.rebuild_pending() {
                if let Some(context) = &self.gpu_context {
                    crate::watchdog::note("the window had no surface to draw into; making it a new one");
                    match context.recreate_surface(gpu, Arc::clone(window)) {
                        Ok(()) => window.request_redraw(),
                        Err(why) => self.gpu_watch.want_rebuild(why),
                    }
                }
            }
            return;
        };
        let frame = match surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame)
            | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
            // Nothing to draw into for now, which isn't trouble; but nothing
            // else will ask for the frame once there is.
            wgpu::CurrentSurfaceTexture::Occluded => {
                self.gpu_watch.retry_after(now, crate::gpu::RETRY_WAIT);
                return;
            }
            status => {
                let inner = window.inner_size();
                // Minimised, a surface has no size to be: `Resized` brings
                // it back, and none of this is the surface's fault.
                if window.is_minimized() == Some(true) || inner.width == 0 || inner.height == 0 {
                    return;
                }
                // Usually the window changed size since the surface was
                // configured; a run of these means it's stuck, and the
                // window would go on showing its last picture -- see
                // `SurfaceHealth`. `Validation` is what a lost device says
                // here, and `Timeout` is what a wedged one does (each
                // having held this thread for a second or two).
                let response = gpu.health.failed();
                let status = match status {
                    wgpu::CurrentSurfaceTexture::Lost => "lost",
                    wgpu::CurrentSurfaceTexture::Outdated => "out of date",
                    wgpu::CurrentSurfaceTexture::Timeout => "timing out",
                    _ => "unusable",
                };
                let failures = gpu.health.failures();
                if response.report {
                    crate::watchdog::note(&format!("the window's surface is still {status} after {failures} tries; making it a new one"));
                }
                match (response.recovery, &self.gpu_context) {
                    (crate::gpu::Recovery::Rebuild, _) => self.gpu_watch.want_rebuild(format!("the window's surface is still {status} after {failures} tries")),
                    (crate::gpu::Recovery::Recreate, Some(context)) => {
                        if let Err(why) = context.recreate_surface(gpu, Arc::clone(window)) {
                            self.gpu_watch.want_rebuild(why);
                        }
                    }
                    // Configuring on a device that's being replaced is
                    // wasted, and would wait on it.
                    _ if self.gpu_watch.rebuild_pending() => {}
                    _ => gpu.reconfigure(inner),
                }
                if response.retry {
                    window.request_redraw();
                } else {
                    self.gpu_watch.retry_after(now, crate::gpu::RETRY_WAIT);
                }
                return;
            }
        };
        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("frame") });
        {
            // The tray: what shows between the panels -- flat, the page.
            let [r, g, b, a] = if look == panels::Look::Flat { theme.bg } else { theme.tray };
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("main-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: r as f64,
                            g: g as f64,
                            b: b as f64,
                            a: a as f64,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            bg_rect.render(&mut pass);
            // Drawn before `text.render`, not after: a surface pane's title
            // bar is still ordinary glyphon text (from the same `text`
            // pipeline, prepared just above) and needs to stay legible
            // on top of its pixels, not painted over by them. Geometry for
            // all of these was accumulated and uploaded before the pass
            // opened; this only picks the texture and the slot to draw it over.
            for (buffer_id, slot, cursor_slot) in &surface_draws {
                let Some(pages::PageModel::Code(p)) = self.pages.get(buffer_id).map(|s| &s.model) else { continue };
                let Some(cs) = &p.surface else { continue };
                let Some(texture) = &cs.texture else { continue };
                surfaces.draw(&mut pass, texture, *slot, Blend::Opaque);
                if let (Some(slot), Some((_, Some(cursor)))) = (cursor_slot, &cs.cursor) {
                    surfaces.draw(&mut pass, cursor, *slot, Blend::Over);
                }
            }

            if let Some(texture) = self.home_logo.as_ref().and_then(|logo| logo.texture.as_ref()) {
                for &slot in &logo_slots {
                    surfaces.draw(&mut pass, texture, slot, Blend::Over);
                }
            }
            for &(key, slot) in &picture_draws {
                if let Some(texture) = self.page_images.textures.get(key).and_then(|tex| tex.texture.as_ref()) {
                    surfaces.draw(&mut pass, texture, slot, Blend::Over);
                }
            }
            motion_gpu.layers_below.render(&mut pass);
            text.render(&mut pass);
            motion_gpu.overlay.render(&mut pass);
        }
        // Popups (background + text) and the caret are drawn in a second,
        // separate pass layered on top (`LoadOp::Load`, not `Clear`)
        // rather than folded into the pass above. A popup's background is
        // an opaque rect from `bg_rect`, and a pane's content `TextArea`
        // has no way to be clipped around a popup-shaped hole (a single
        // `TextBounds` per pane can't express "everywhere except this
        // corner") -- so if popup content shared the first pass, whichever
        // pane extends under the popup's corner would simply paint its own
        // text over the popup's already-drawn background, exactly the
        // "buffer text on top of the which-key window" bug this two-pass
        // split fixes. Ending pass one first and starting pass two with
        // `Load` guarantees popups (and the caret, which already needed to
        // draw after all text) composite on top of whatever pass one
        // painted, regardless of what that was.
        if !popup_rects.is_empty() {
            // `popup_rect`, not `bg_rect` -- see `App::popup_rect`'s own
            // doc comment for what reusing the base layer's renderer here
            // silently corrupted.
            popup_rect.clear();
            let with_alpha = |[r, g, b, a]: [f32; 4], alpha: f32| [r, g, b, a * alpha];
            for draw in &popup_draws {
                let (rect, alpha) = (draw.rect, draw.alpha);
                if let Some(card) = motion_state.cards.get(&draw.id) {
                    palette::paint(popup_rect, gpu, card, rect, alpha, theme, char_width, line_height, (window_width, window_height), shadows);
                    continue;
                }
                if let popup::PopupId::Toast(slot) = draw.id {
                    if let Some(toast) = toasts.iter().find(|t| t.slot == slot) {
                        App::paint_toast(popup_rect, gpu, toast, rect, alpha, theme, corner_radius, shadows);
                        continue;
                    }
                }
                if shadows {
                    popup_rect.push_shadow(gpu, rect.x, rect.y + 4.0, rect.w, rect.h, corner_radius, 10.0, [0.0, 0.0, 0.0, 0.38 * alpha]);
                }
                let border = glyphon_to_rgba(theme.gutter_fg);
                popup_rect.push_box(gpu, rect.x, rect.y, rect.w, rect.h, crate::rect::BoxStyle {
                    radius: corner_radius,
                    fill: with_alpha(theme.bg_modeline, alpha),
                    border: 1.0,
                    border_color: with_alpha(border, alpha),
                });
                // The completion popup's own selected-candidate row --
                // `theme.selection`, not `theme.hl_line`: the popup has no
                // caret of its own, so this has to stand out on its own.
                // A rename prompt's prefilled name, selected.
                if let (popup::PopupId::Prompt, Some(chars)) = (draw.id, prompt_selection.clone()) {
                    let x = rect.x + text::PAD_LEFT + chars.start as f32 * char_width;
                    let w = (chars.len() as f32 * char_width).min(rect.x + rect.w - x).max(0.0);
                    popup_rect.push_box(gpu, x, rect.y + 4.0, w, line_height, crate::rect::BoxStyle {
                        radius: 2.0,
                        fill: with_alpha(theme.selection, alpha),
                        border: 0.0,
                        border_color: [0.0; 4],
                    });
                }
                if matches!(draw.id, popup::PopupId::Completion | popup::PopupId::ContextMenu) {
                    if let Some(row) = draw.selected {
                        let y = rect.y + COMPLETION_PADDING / 2.0 + row as f32 * line_height;
                        let inset = if corner_radius > 0.0 { 3.0 } else { 0.0 };
                        popup_rect.push_box(gpu, rect.x + inset, y, rect.w - inset * 2.0, line_height, crate::rect::BoxStyle {
                            radius: corner_radius.min(3.0),
                            fill: with_alpha(theme.selection, alpha),
                            border: 0.0,
                            border_color: [0.0; 4],
                        });
                    }
                }
            }
            popup_rect.flush(gpu);
            // Each popup's text inset, and where it's cut off: the
            // which-key drawer never shows below the modeline's top.
            let popup_text: Vec<(popup::PopupId, fenix_window::Rect, (f32, f32), f32)> = popup_rects
                .iter()
                .map(|&(id, rect)| {
                    if let Some(card) = motion_state.cards.get(&id) {
                        (id, rect, palette::inset(card), if id == popup::PopupId::WhichKey { modeline_top } else { f32::MAX })
                    } else if matches!(id, popup::PopupId::Toast(_)) {
                        (id, rect, App::toast_inset(), f32::MAX)
                    } else {
                        (id, rect, (text::PAD_LEFT, 4.0), f32::MAX)
                    }
                })
                .collect();
            text.prepare_popups(gpu, theme, &popup_text);
        }
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("overlay-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            motion_gpu.sprites.render(&mut pass);
            motion_gpu.layers_above.render(&mut pass);
            if !popup_rects.is_empty() {
                popup_rect.render(&mut pass);
                text.render_popups(&mut pass);
            }
            motion_gpu.layers_top.render(&mut pass);
            caret_rect.render(&mut pass);
            motion_gpu.top.render(&mut pass);
            if let Some(fade) = &self.splash_fade {
                fade.sprites.render(&mut pass);
            }
        }
        // The frame a theme change fades out of: copied before it's shown.
        if motion_state.capture_frame {
            let target = motion_gpu.snapshot_target(gpu);
            encoder.copy_texture_to_texture(
                frame.texture.as_image_copy(),
                target.as_image_copy(),
                wgpu::Extent3d { width: surface_w, height: surface_h, depth_or_array_layers: 1 },
            );
        }
        _profile.mark("surface acquired and commands encoded");
        gpu.queue.submit(Some(encoder.finish()));
        window.pre_present_notify();
        gpu.queue.present(frame);
        _profile.mark("present complete");
        if gpu.health.presented() {
            crate::watchdog::note("the window's surface is drawing again");
        }
        self.gpu_watch.presented();
        self.frames_presented += 1;
        if self.first_frame_due.take().is_some_and(|due| Instant::now() > due) {
            eprintln!("fenix: the first frame came late, {:?} after the splash handed over", FIRST_FRAME_WITHIN);
        }
        static FIRST_FRAME: std::sync::Once = std::sync::Once::new();
        FIRST_FRAME.call_once(|| crate::profile::launch_mark("first frame"));
        text.trim();
        // Publish the tab geometry this frame actually drew, so mouse
        // hit-testing (`frame_geometry`) uses the exact same measured
        // rects rather than recomputing an estimate that disagrees with
        // them. Assigned here, after the `text`/`bg_rect` borrows above
        // have all been released.
        self.tab_geometry =
            panes_render.iter().flat_map(|p| p.tabs_layout.iter().map(|t| (p.pane, *t))).collect();
    }
}
