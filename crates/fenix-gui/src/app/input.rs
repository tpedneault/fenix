//! Keys and the mouse, routed to what has the focus.
//!
//! Methods of `App` moved here from `app.rs` as they were.

use super::*;

impl App {
    pub(super) fn handle_key(&mut self, event: &KeyEvent, event_loop: &ActiveEventLoop) {
        self.motion.key_repeat = event.repeat;
        // A surface that has the keyboard takes every key, pressed and let go.
        if self.wasm.captured.is_some() {
            let caps = event.logical_key == Key::Named(NamedKey::CapsLock);
            let down = event.state == ElementState::Pressed;
            let keypress = if caps { None } else { keymap::to_keypress(event, self.modifiers) };
            if self.code_surface_captured_key(keypress, caps, down) {
                return;
            }
        }
        // Caps Lock, and every key let go, mean nothing to Fenix itself:
        // only a surface that has the keyboard (above) hears them.
        if event.logical_key == Key::Named(NamedKey::CapsLock) || event.state != ElementState::Pressed {
            return;
        }

        if let Key::Named(named) = &event.logical_key {
            if self.terminal_function_key(named) {
                self.wake_caret();
                return;
            }
            // An F-key a `[[keys]]` entry binds (yours, or a keymap
            // flavour's) goes to it; the rest are the debugger's.
            let bound = keymap::to_keypress(event, self.modifiers).is_some_and(|k| self.chord_takes_function_key(k));
            if !bound && self.debug_function_key(named) {
                return;
            }
        }
        let keypresses = keymap::to_keypresses(&event.logical_key, event.physical_key, event.text.as_deref(), self.modifiers);
        self.dispatch_keypresses(keypresses, Some(event_loop));
    }

    pub(super) fn route_keypress(&mut self, keypress: KeyPress, event_loop: Option<&ActiveEventLoop>) {
        let _profile = crate::profile::Scope::new("route_keypress");
        // What a move by row goes by: the focused pane's rows now, for a
        // key typed and for each a macro or `.` plays (which may have
        // gone to another pane); and the cells the last key's moves by
        // row kept.
        self.sync_screen_rows();
        self.sync_vim_view(keypress);
        self.vim.begin_key();
        // Everything but the chords reads keys as it always has, without
        // Shift (see `KeyPress::legacy`).
        // Standard keys in a listing: F2 renames and Delete deletes, as
        // its `R` and `D` do.
        let keypress = self.standard_explorer_alias(keypress).unwrap_or(keypress);
        let full = keypress;
        let keypress = keypress.legacy();
        // A feature's informational menu (the Docker panel's `x`) is not a
        // capturing prompt -- cleared unconditionally on *every* keypress
        // before that keypress is dispatched normally below, so pressing
        // any key both dismisses the popup and still does whatever that
        // key would otherwise do (real Lazydocker's own "view command
        // options" convention).
        // Same reasoning, for the Git panel's own `x` which-key menu.
        self.git_menu_open = false;
        // Same reasoning, for the LSP hover popup (`K`) -- purely
        // informational, dismissed by whatever key comes next rather
        // than needing its own dedicated close key.
        self.lsp_hover = None;
        self.lsp_hover_fanout = None;
        // And the one the pointer opened: typing closes it.
        self.close_mouse_hover();

        // The terminal panel, when focused, owns *every* key -- checked
        // before even the capturing prompts/confirms below, since a
        // real shell needs Space/Escape/Ctrl-C/etc. to reach it
        // unmodified, not intercepted by something else first. `Ctrl-\`
        // is the one reserved chord that unfocuses (not closes) it
        // instead of forwarding, mirroring Neovim's own `:terminal`
        // convention -- see `App::terminal_focused`'s own doc comment.
        // A chord that skips the shell (standard keys' Ctrl+P, Ctrl+`...)
        // works from a terminal too.
        if self.terminal_focused || self.terminal_buffer_focused.is_some() {
            if let Some(id) = keymap::chord_map().skips_terminal(full, self.key_scheme()) {
                self.run_command_with(event_loop, id);
                self.wake_caret();
                return;
            }
        }
        if self.terminal_focused {
            if keypress == KeyPress::char('\\').with_ctrl() {
                self.terminal_focused = false;
            } else if !self.terminal_clipboard_key(terminal_mouse::TermTarget::Panel, keypress) {
                self.terminal_typed(terminal_mouse::TermTarget::Panel);
                self.write_terminal_input(keypress);
            }
            self.wake_caret();
            return;
        }

        // A pane-resident terminal owns every key on exactly the same
        // terms as the panel above, and is checked in the same tier --
        // the two can never both hold the keyboard (`focus_terminal_
        // buffer` clears the other), so their order here is arbitrary.
        // `active_terminal_buffer`, rather than the field, is what makes
        // typing follow the pane you are actually looking at.
        if let Some(id) = self.active_terminal_buffer() {
            if keypress == KeyPress::char('\\').with_ctrl() {
                self.unfocus_terminal_buffer();
            } else if !self.terminal_clipboard_key(terminal_mouse::TermTarget::Buffer(id), keypress) {
                self.terminal_typed(terminal_mouse::TermTarget::Buffer(id));
                self.write_terminal_buffer_input(id, keypress);
            }
            self.wake_caret();
            return;
        }

        // An armed quit confirmation (`:q`/`SPC q q`/the window's close
        // button with unsaved buffers) captures the very next key --
        // same "y confirms, anything else cancels" shape as `docker_
        // confirm_remove`/`git_confirm` below, checked first since it's
        // the one prompt that can be reached from *outside* Normal mode
        // too (the window close button).
        if self.quit_confirm {
            self.quit_confirm_key(keypress, event_loop);
            return;
        }

        // An active prompt (rename/create-name/confirm-delete), or an
        // unanswered "something is already there", captures every
        // keystroke until it resolves -- taking priority over everything
        // else, including global Ctrl-chords, so e.g. Ctrl-S can't
        // accidentally interrupt an in-progress rename, and a half-
        // specified copy can't be left pending while you go and edit
        // something.
        if self.explorer_prompt.is_some() || self.explorer_conflict.is_some() {
            self.explorer_prompt_key(keypress);
            return;
        }

        // An armed bulk rename captures the very next key -- same
        // arm-then-confirm shape as every other destructive action here.
        if self.rename_confirm.is_some() {
            self.rename_confirm_key(keypress);
            return;
        }

        // Same capturing shape for the Git panel's own destructive-action
        // confirm and its commit-message/new-branch-name prompt -- mirrors
        // `DockerHost::confirm_remove`/`explorer_prompt` exactly.
        if self.git_confirm.is_some() {
            self.git_confirm_key(keypress);
            return;
        }
        if self.git_prompt.is_some() {
            self.git_prompt_key(keypress);
            return;
        }
        // A code extension's prompt.
        if self.code_prompt_key(keypress) {
            return;
        }
        // `SPC f f`/`SPC f R`/`SPC f D` -- same capturing-prompt tier as
        // everything else here.
        if self.find_file_prompt.is_some() {
            self.find_file_prompt_key(keypress);
            return;
        }

        if self.rename_file_prompt.is_some() {
            self.rename_file_prompt_key(keypress);
            return;
        }
        // `SPC c r` -- same capturing-prompt tier.
        if self.lsp_rename_prompt.is_some() {
            self.lsp_rename_prompt_key(keypress);
            return;
        }
        // `SPC TAB r` -- same capturing-prompt tier.
        if self.goto_line_prompt.is_some() {
            self.goto_line_prompt_key(keypress);
            return;
        }
        if self.workspace_rename_prompt.is_some() {
            self.workspace_rename_prompt_key(keypress);
            return;
        }
        if self.delete_file_confirm {
            self.delete_file_confirm_key(keypress);
            return;
        }
        if self.server_install.confirm.is_some() {
            self.server_install_confirm_key(keypress);
            return;
        }

        // The grep search-term prompt and an open picker both capture
        // input the same way -- checked ahead of the explorer/sidebar-
        // focus check below since a picker can be opened while the
        // sidebar is focused (switch-project's find-file chain, for
        // instance). The add-project flow no longer has its own prompt
        // state to check here -- it's the directory picker, an explorer
        // page routed the same way `SPC f j`'s is.
        if self.binding_keys.is_some() {
            self.bind_keys_feed(full);
            self.wake_caret();
            return;
        }
        if self.describing_keys.is_some() {
            self.describe_key_feed(full);
            self.wake_caret();
            return;
        }
        // Ctrl+Tab again, with the recent-buffer switcher up: the next row
        // (with Shift, the one before).
        if self.switching_recent && full.code == KeyCode::Named(FenixNamedKey::Tab) && full.mods.ctrl {
            if let Some(ActivePicker::SwitchBuffer(picker)) = &mut self.active_picker {
                picker.move_selection(if full.mods.shift { -1 } else { 1 });
            }
            self.wake_caret();
            return;
        }
        if self.active_picker.is_some() {
            self.picker_key(keypress);
            self.ask_project_symbols();
            self.thread_list_follow_query();
            if let Some(id) = self.queued_command.take() {
                self.run_command_with(event_loop, &id);
                self.wake_caret();
            }
            return;
        }
        // The new-project wizard's page: ahead of the leader, so a space
        // typed into one of its fields is a space.
        // Alt-x: the command palette, from anywhere -- a page or Insert
        // too -- but not from inside a picker or prompt (`SPC :` in Normal).
        if keypress.mods.alt && !keypress.mods.ctrl && keypress.code == KeyCode::Char('x') {
            self.run_command_with(event_loop, "commands.palette");
            self.wake_caret();
            return;
        }
        // The debug drawer, gone into (`SPC u f`): every key but SPC.
        if self.debug_drawer_focused() && self.debug_drawer_key(keypress) {
            self.wake_caret();
            return;
        }
        if self.main_view == MainView::Editor && !self.sidebar_focused && self.page_key(keypress) {
            self.wake_caret();
            return;
        }
        // The prompt under an agent's thread: Enter in Normal mode sends it.
        if self.main_view == MainView::Editor && !self.sidebar_focused && self.agents_composer_key(keypress) {
            self.wake_caret();
            return;
        }

        // The sidebar is different: a persistent panel meant to coexist
        // with active editing, not a modal takeover, so SPC still reaches
        // the leader menu below (e.g. `SPC w l` to jump back to another
        // window) instead of being swallowed. Pressing it also hands focus
        // back to the editor, since leader commands act on the window/
        // buffer layer, not the sidebar.
        if self.sidebar_focused {
            if (keypress == KeyPress::char(' ') || self.leader_starts(full)) && self.leader_starts(full) && !self.explorer_matcher.is_pending() && !self.explorer_editing(ExplorerTarget::Sidebar) {
                self.sidebar_focused = false;
            } else if self.explorer_page_key(ExplorerTarget::Sidebar, keypress) {
                return;
            }
        }

        // In an open leader menu, Backspace goes up a level and Ctrl-n /
        // Ctrl-p page through it (see `which_key`) -- ahead of the chords,
        // which may bind those keys elsewhere.
        if (self.local_matcher.is_some() || self.leader_matcher.is_pending())
            && (self.which_key_back(keypress) || self.which_key_paging(keypress))
        {
            self.wake_caret();
            return;
        }
        // Chords: `[[keys]]` in Fenix's keymap, the extensions' and yours
        // (`C-s`, `C-z`, `F12`...). They read the key as it came, Shift
        // and all (`full`); a macro's recorded key carries its own
        // modifiers, so a replay runs the same chord.
        let in_leader = self.local_matcher.is_some() || self.leader_matcher.is_pending();
        // A compose pane's own keys, with standard keys, go to it first.
        let composing = self.standard_keys() && self.compose.as_ref().is_some_and(|c| c.pane == self.focused_pane_id());
        let in_leader = in_leader || (composing && (full.chord() == KeyPress::named(FenixNamedKey::Enter).with_ctrl() || full.chord() == KeyPress::char('a').with_ctrl().with_shift()));
        if let Some(id) = self.chord_command(full).filter(|_| !in_leader) {
            // Only the clipboard's own work at every caret, and the
            // commands that add carets keep those there are.
            if !carets::keeps_carets(id) {
                self.clear_carets();
            }
            self.run_command_with(event_loop, id);
            self.wake_caret();
            return;
        }
        // An F-key nothing took means nothing below.
        if full.is_function() {
            return;
        }

        // Window-size-aware paging is a GUI concern, handled the same way
        // regardless of Vim mode -- fenix-vim doesn't know about viewport size.
        if keypress == KeyPress::named(FenixNamedKey::PageUp) || keypress == KeyPress::named(FenixNamedKey::PageDown) {
            let page_size = match (&self.gpu, &self.text) {
                (Some(gpu), Some(text)) => {
                    text::visible_line_count(gpu.size.height as f32, text.modeline_height(), text.line_height())
                }
                (Some(gpu), None) => text::visible_line_count(gpu.size.height as f32, text::LINE_HEIGHT, text::LINE_HEIGHT),
                (None, _) => 20,
            };
            let down = keypress == KeyPress::named(FenixNamedKey::PageDown);
            // A pane by rows: by its rows, a pane less one. One that
            // doesn't wrap (lens rows above its lines) counts them, as
            // Vim's rows don't.
            let (id, pane) = (self.focused_buffer_id(), self.focused_pane_id());
            if self.row_cols(pane, id).is_some() {
                let rows = self.pane_text_rows(pane).unwrap_or(page_size).saturating_sub(1).max(1) as isize;
                if self.page_by_rows(pane, id, if down { rows } else { -rows }) {
                    self.wake_caret();
                    return;
                }
                if let (Some(ob), Some(state)) = (self.buffers.get_mut(id), self.workspaces.active_pane_states_mut().get_mut(&pane)) {
                    if self.vim.move_rows(&mut ob.buffer, &mut state.cursor, if down { rows } else { -rows }) {
                        self.wake_caret();
                        return;
                    }
                }
            }
            let (buffer, cursor) = self.focused_buffer_and_cursor_mut();
            buffer.move_page(cursor, page_size, down);
            self.wake_caret();
            return;
        }

        // Leader sequences span multiple keystrokes: stay routed here while
        // one is already in progress, or start one on SPC from Normal or
        // Visual mode (matching orbit-emacs, where SPC is evil-visual-
        // state's leader too -- in Insert mode it should just insert a
        // space). Fenix's own Visual keymap has no binding for a bare
        // space, so this doesn't take anything away from Visual mode --
        // it only makes `SPC c f` (format the active selection) and the
        // rest of the leader menu reachable without leaving the
        // selection first. The leader keys themselves (here, `c`/`f`)
        // never reach `self.vim`, so the selection's anchor/cursor stay
        // exactly as they were until whatever command they resolve to
        // (e.g. `format_selection`) reads them.
        if let Some(resolved) = self.local_leader_key(keypress) {
            if let Some(id) = resolved {
                // As by its chord: the extra cursors stay only for what
                // works at each of them.
                if !carets::keeps_carets(id) {
                    self.clear_carets();
                }
                self.run_command_with(event_loop, id);
            }
            return;
        }
        if self.leader_matcher.is_pending() || self.leader_starts(full) {
            // The leader chord starts the leader as SPC does: its keys
            // follow.
            let fed = if self.leader_matcher.is_pending() { keypress.chord().legacy() } else {
                self.leader_uses = self.leader_uses.saturating_add(1);
                KeyPress::char(' ')
            };
            // A group that's off (the test explorer's `SPC T`) is as
            // unbound as a key that isn't there.
            if self.leader_hidden(self.leader_matcher.path(), fed) {
                self.leader_matcher.cancel();
                self.wake_caret();
                return;
            }
            if let Step::Matched(&id) = self.leader_matcher.feed(fed) {
                if !carets::keeps_carets(id) {
                    self.clear_carets();
                }
                self.run_command_with(event_loop, id);
            }
            self.wake_caret();
            return;
        }
        if self.debug_trail_key(keypress) || self.debug_mode_key(keypress) {
            self.wake_caret();
            return;
        }

        // Shift+Tab keeps its Shift for a snippet: it goes back a field.
        let snippet_key = if full == KeyPress::named(FenixNamedKey::Tab).with_shift() { full } else { keypress };
        let before = (self.open().buffer.edit_count(), self.cursor().char_idx);
        if self.snippet_key(snippet_key) || self.completion_key(keypress) {
            // A completion taken or a snippet's field moved to is the
            // primary's alone: the others go, as for any other key that
            // edits. Moving in the completion menu leaves them be.
            if (self.open().buffer.edit_count(), self.cursor().char_idx) != before {
                self.clear_carets();
            }
            self.wake_caret();
            return;
        }

        // The dashboard is a real Vim-navigable buffer (see `BufferKind`'s
        // own doc comment) -- every other key still reaches Vim below
        // unchanged (movement, `/` search, `gg`/`G`...); only `Enter`
        // means something special on it.
        if self.home_key(keypress) {
            self.wake_caret();
            return;
        }

        // Names being edited as text (`SPC e w`): every character is a
        // character somebody might be typing into a filename, so only
        // Escape means anything, and it means "throw the edits away".
        if self.rename_mode.contains_key(&self.focused_buffer_id())
            && keypress.code == KeyCode::Named(FenixNamedKey::Escape)
            // Standard keys type here, and Escape is how they say no.
            && (self.vim.mode() == Mode::Normal || (self.standard_keys() && self.vim.is_inserting()))
        {
            let id = self.focused_buffer_id();
            self.cancel_rename_mode(id);
            return;
        }

        // A table view (`SPC f t`) is likewise real Vim-navigable text --
        // ordinary motions and edits reach Vim below unchanged (editing a
        // cell is just ordinary Vim text editing on the buffer's real
        // tab-separated content; nothing table-specific is needed for
        // that at all). Only `]`/`[` (next/prev column) and `j`/`k`
        // (reinterpreted: same *elastic column* on the adjacent row, not
        // plain char-based `sticky_col`, which stops tracking "same
        // column" once rows have different raw lengths up to it -- see
        // `table_move_row`'s own doc comment) and `c` (jump to a column
        // by name) are claimed here first.
        if self.open().kind == BufferKind::Table {
            match keypress.code {
                KeyCode::Char(']') if keypress.mods == Mods::default() => {
                    self.table_move_column(1);
                    self.wake_caret();
                    return;
                }
                KeyCode::Char('[') if keypress.mods == Mods::default() => {
                    self.table_move_column(-1);
                    self.wake_caret();
                    return;
                }
                KeyCode::Char('j') if keypress.mods == Mods::default() => {
                    self.table_move_row(1);
                    self.wake_caret();
                    return;
                }
                KeyCode::Char('k') if keypress.mods == Mods::default() => {
                    self.table_move_row(-1);
                    self.wake_caret();
                    return;
                }
                KeyCode::Char('c') if keypress.mods == Mods::default() => {
                    self.open_table_column_picker();
                    return;
                }
                _ => {}
            }
        }


        // The `SPC s p` project-replace review buffer -- ordinary
        // motions (`j k gg G / n N`) navigate it like any real buffer
        // (it's genuinely one), but its own action keys are claimed
        // here first, same "claim a few keys" shape as the dired/table
        // blocks above. `q`/`Esc` deliberately mean "close this pane"
        // here, not whatever they'd mean in ordinary editing (macro-
        // record `q`, in particular) -- same precedent the Explorer/
        // Docker/Git panels already established for their own `q`.
        if self.refactor_key(keypress) { return; }

        // Diff-viewer keys, claimed wherever a rendered diff is focused
        // rather than per-panel: the working tree's Main pane, a
        // commit's detail in the History view and a comparison's changes
        // are all the same viewer, so `Tab` folding a file and `]`/`[`
        // stepping between hunks have to mean the same thing in all
        // three. Everything else falls through to the per-view blocks
        // below, and from there to Vim.
        // The compose pane's own keys, checked before anything else
        // claims them. Only in Normal mode: in Insert, `Enter` is a
        // newline and `q` is a letter, which is the whole point of
        // writing a comment in a real buffer.
        // Not halfway through a leader key, though: `SPC q q` from the
        // pane quits, it doesn't drop what's written.
        // Standard keys: Ctrl+Enter sends, Escape asks before it throws
        // what's written away, Ctrl+Shift+A has the agent write it.
        if self.standard_keys() && self.compose.as_ref().is_some_and(|c| c.pane == self.focused_pane_id()) && !self.leader_matcher.is_pending() && self.local_matcher.is_none() {
            let chord = full.chord();
            if chord == KeyPress::named(FenixNamedKey::Enter).with_ctrl() {
                self.compose_discard_armed = false;
                self.compose_submit();
                return;
            }
            if chord == KeyPress::char('a').with_ctrl().with_shift() {
                if self.compose_agent_offered() {
                    self.compose_agent_write();
                } else {
                    self.set_message("the agent writes a box that's empty -- clear it to ask again");
                }
                return;
            }
            if full == KeyPress::named(FenixNamedKey::Escape) && self.vim.mode() != Mode::Visual && self.extra_carets().is_empty() {
                let written = self.compose.as_ref().and_then(|c| self.buffers.get(c.buffer).map(|ob| (ob.buffer.text(), c.seed.clone()))).is_some_and(|(text, seed)| !text.trim().is_empty() && text.trim_end() != seed.trim_end());
                if !written || std::mem::take(&mut self.compose_discard_armed) {
                    self.compose_cancel();
                } else {
                    self.compose_discard_armed = true;
                    self.set_message("Esc again throws what's written away -- Ctrl+Enter sends it");
                }
                return;
            }
            self.compose_discard_armed = false;
        }
        if self.compose.as_ref().is_some_and(|c| c.pane == self.focused_pane_id())
            && self.vim.mode() == Mode::Normal
            && !self.standard_keys()
            && keypress.mods == Mods::default()
            && !self.leader_matcher.is_pending()
            && self.local_matcher.is_none()
        {
            match keypress.code {
                KeyCode::Named(FenixNamedKey::Enter) => {
                    self.compose_submit();
                    return;
                }
                KeyCode::Char('q') => {
                    self.compose_cancel();
                    return;
                }
                KeyCode::Char('A') if self.compose_agent_offered() => {
                    self.compose_agent_write();
                    return;
                }
                _ => {}
            }
        }
        // `Enter` on a line of a task's output that names a place --
        // `src/lib.rs:89:9`, a panic, a test's stack -- goes there.
        if keypress == KeyPress::named(FenixNamedKey::Enter)
            && self.vim.mode() == Mode::Normal
            && !self.leader_matcher.is_pending()
            && self.open().kind == BufferKind::TaskOutput
            && self.task_output_open_location_at_cursor()
        {
            return;
        }
        // `q` in a side-by-side's read-only pane closes it -- that pane
        // is nothing to record a macro in.
        if keypress == KeyPress::char('q') && self.vim.mode() == Mode::Normal && self.focused_is_side_by_side_ref() {
            self.side_by_side_close();
            return;
        }
        if self.view_closed_by_q(keypress) {
            return;
        }

        if self.focused_diff_buffer().is_some() && keypress.mods == Mods::default() {
            match keypress.code {
                KeyCode::Char('s') => {
                    self.diff_apply_hunk(fenix_git::ApplyTarget::Stage);
                    self.wake_caret();
                    return;
                }
                KeyCode::Char('S') => {
                    self.diff_apply_hunk(fenix_git::ApplyTarget::Unstage);
                    self.wake_caret();
                    return;
                }
                KeyCode::Char('d') => {
                    self.diff_discard_request();
                    self.wake_caret();
                    return;
                }
                KeyCode::Char(']') => {
                    self.diff_jump_hunk(true);
                    return;
                }
                KeyCode::Char('[') => {
                    self.diff_jump_hunk(false);
                    return;
                }
                KeyCode::Named(FenixNamedKey::Tab) => {
                    self.diff_toggle_fold();
                    return;
                }
                KeyCode::Named(FenixNamedKey::Enter) => {
                    self.diff_open_file_at_cursor();
                    return;
                }
                _ => {}
            }
        }

        // The History and Compare views claim their own small key sets
        // the same way, ahead of the working-tree panel's block below --
        // they're separate workspaces, so there's no ambiguity about
        // which one a keypress is meant for.
        if self.history_session.as_ref().is_some_and(|s| {
            let focused = self.focused_pane_id();
            focused == s.graph_pane || focused == s.refs_pane || focused == s.detail_pane
        }) && keypress.mods == Mods::default()
        {
            match keypress.code {
                KeyCode::Char('u') => {
                    self.history_refresh();
                    self.wake_caret();
                    return;
                }
                KeyCode::Char('f') => {
                    self.git_fetch();
                    self.wake_caret();
                    return;
                }
                KeyCode::Char('x') => {
                    self.git_menu_open = true;
                    self.wake_caret();
                    return;
                }
                // Closes the view, as `q` closes the other Git views --
                // not Vim's macro recording, which a read-only pane has
                // no use for.
                KeyCode::Char('q') => {
                    self.history_close();
                    self.wake_caret();
                    return;
                }
                // Jump straight to a numbered pane, the same shortcut
                // the Docker and Git panels' own titles advertise.
                KeyCode::Char(c) if c.is_ascii_digit() => {
                    if let Some(pane) = self.history_pane_by_number(c.to_digit(10).unwrap_or(0)) {
                        self.windows_mut().focus(pane);
                        // Landing on the graph re-syncs the detail pane
                        // to whatever row the cursor was left on there.
                        self.history_sync_detail();
                        self.wake_caret();
                    }
                    return;
                }
                _ => {}
            }
        }
        // The Merge view's own keys. `o`/`t` mean the same thing in both
        // its panes -- keep the left side, keep the right side -- and
        // differ only in scope: on the file list it's the whole file, in
        // the columns it's the conflict under the cursor. That's the one
        // thing worth learning here, so it's the one thing that's
        // consistent.
        if let Some((on_files, on_merge)) = self.merge_session.as_ref().map(|s| {
            let focused = self.focused_pane_id();
            (focused == s.files_pane, focused == s.merge_pane)
        }) {
            if (on_files || on_merge) && keypress.mods == Mods::default() {
                match keypress.code {
                    KeyCode::Char('o') if on_files => {
                        self.merge_take_whole_file(fenix_git::Resolution::Ours);
                        return;
                    }
                    KeyCode::Char('t') if on_files => {
                        self.merge_take_whole_file(fenix_git::Resolution::Theirs);
                        return;
                    }
                    KeyCode::Char('o') if on_merge => {
                        self.resolve_conflict_at_cursor(fenix_git::Resolution::Ours);
                        return;
                    }
                    KeyCode::Char('t') if on_merge => {
                        self.resolve_conflict_at_cursor(fenix_git::Resolution::Theirs);
                        return;
                    }
                    KeyCode::Char('b') if on_merge => {
                        self.resolve_conflict_at_cursor(fenix_git::Resolution::Both);
                        return;
                    }
                    KeyCode::Char('n') if on_merge => {
                        self.goto_conflict(true);
                        return;
                    }
                    KeyCode::Char('p') if on_merge => {
                        self.goto_conflict(false);
                        return;
                    }
                    KeyCode::Named(fenix_keymap::NamedKey::Enter) if on_files => {
                        self.merge_select_at_cursor();
                        return;
                    }
                    KeyCode::Char('s') => {
                        self.merge_stage_resolved();
                        return;
                    }
                    KeyCode::Char('u') => {
                        self.merge_restore_conflict();
                        return;
                    }
                    KeyCode::Char('r') => {
                        self.merge_refresh();
                        self.wake_caret();
                        return;
                    }
                    KeyCode::Char('x') => {
                        self.git_menu_open = true;
                        self.wake_caret();
                        return;
                    }
                    KeyCode::Char(c) if c.is_ascii_digit() => {
                        if let Some(pane) = self.merge_pane_by_number(c.to_digit(10).unwrap_or(0)) {
                            self.windows_mut().focus(pane);
                            self.wake_caret();
                        }
                        return;
                    }
                    _ => {}
                }
            }
        }

        // Lazygit-style multi-pane panel -- same "claim a small, pane-
        // scoped action-key set, everything else falls through to Vim"
        // shape as the Docker block above. Real lazygit's own `<space>`
        // stage-toggle isn't reachable here: `SPC` is already the global
        // leader-key trigger in Normal mode, checked earlier than this
        // block ever runs (see `git_menu_popup`'s own doc comment) -- so
        // Files uses separate `s`/`S` stage/unstage keys instead, matching
        // Docker's own `s`/`S`/`R` precedent of distinct keys per action
        // rather than a single toggle.
        if let Some(role) = self.git_focused_role() {
            use GitPaneRole::*;
            match (role, keypress.code) {
                (Staged | Unstaged, KeyCode::Char('s')) if keypress.mods == Mods::default() => {
                    self.git_stage_selected();
                    self.wake_caret();
                    return;
                }
                (Staged | Unstaged, KeyCode::Char('S')) if keypress.mods == Mods::default() => {
                    self.git_unstage_selected();
                    self.wake_caret();
                    return;
                }
                (Staged | Unstaged, KeyCode::Char('a')) if keypress.mods == Mods::default() => {
                    self.git_stage_all();
                    self.wake_caret();
                    return;
                }
                (Staged | Unstaged, KeyCode::Char('A')) if keypress.mods == Mods::default() => {
                    self.git_unstage_all();
                    self.wake_caret();
                    return;
                }
                (Staged | Unstaged, KeyCode::Char('z')) if keypress.mods == Mods::default() => {
                    self.git_stash_all();
                    self.wake_caret();
                    return;
                }
                (Staged | Unstaged, KeyCode::Char('P')) if keypress.mods == Mods::default() => {
                    self.git_push();
                    self.wake_caret();
                    return;
                }
                (Staged | Unstaged, KeyCode::Char('p')) if keypress.mods == Mods::default() => {
                    self.git_pull();
                    self.wake_caret();
                    return;
                }
                (Staged | Unstaged, KeyCode::Char('c')) if keypress.mods == Mods::default() => {
                    self.git_commit_prompt();
                    self.wake_caret();
                    return;
                }
                (Staged | Unstaged, KeyCode::Named(FenixNamedKey::Tab)) if keypress.mods == Mods::default() => {
                    self.git_toggle_dir_expand();
                    self.wake_caret();
                    return;
                }
                (Branches, KeyCode::Char('c')) if keypress.mods == Mods::default() => {
                    self.git_checkout_selected_branch();
                    self.wake_caret();
                    return;
                }
                (Branches, KeyCode::Char('n')) if keypress.mods == Mods::default() => {
                    self.git_new_branch_prompt();
                    self.wake_caret();
                    return;
                }
                (Stash, KeyCode::Char('a')) if keypress.mods == Mods::default() => {
                    self.git_stash_apply_selected();
                    self.wake_caret();
                    return;
                }
                (Stash, KeyCode::Char('g')) if keypress.mods == Mods::default() => {
                    self.git_stash_pop_selected();
                    self.wake_caret();
                    return;
                }
                // Discard is Unstaged-only -- discarding a *staged*
                // change means unstaging it first (`S`), matching git's
                // own semantics (`checkout`/`clean` act on the worktree,
                // not the index).
                (Unstaged | Branches | Stash, KeyCode::Char('d')) if keypress.mods == Mods::default() => {
                    self.git_arm_confirm();
                    self.wake_caret();
                    return;
                }
                // The Main pane's own diff keys (`s`/`S`/`d`, `]`/`[`,
                // Tab, Enter) are handled by the shared diff-viewer
                // block above, which covers every pane showing a
                // rendered diff rather than just this one.
                (_, KeyCode::Char('u')) if keypress.mods == Mods::default() => {
                    self.git_refresh_session();
                    self.wake_caret();
                    return;
                }
                (Staged | Unstaged | Branches | Commits | Stash | Main, KeyCode::Char('x')) if keypress.mods == Mods::default() => {
                    self.git_menu_open = true;
                    self.wake_caret();
                    return;
                }
                // Same digit-key jump-to-pane shortcut as the Docker
                // block above (`1. Status`, `2. Staged`, ...).
                (_, KeyCode::Char(c)) if keypress.mods == Mods::default() && c.is_ascii_digit() => {
                    if let Some(pane) = self.git_pane_by_number(c.to_digit(10).unwrap_or(0)) {
                        self.windows_mut().focus(pane);
                        if matches!(self.git_focused_role(), Some(GitPaneRole::Staged | GitPaneRole::Unstaged | GitPaneRole::Commits | GitPaneRole::Stash)) {
                            self.git_sync_main();
                        }
                        self.wake_caret();
                    }
                    return;
                }
                _ => {}
            }
        }

        // `%` on an XML tag goes to its partner tag -- claimed only when
        // Vim has nothing pending (no count, operator or prefix) so `d%`
        // and `3%` keep their own meaning, and only when the cursor is
        // actually on a tag; otherwise it's bracket matching as usual.
        if keypress == KeyPress::char('%') && self.vim.is_idle() && self.xml_jump_matching_tag() {
            self.wake_caret();
            return;
        }

        // More than one caret: the key at each.
        if self.multi_caret_key(full) {
            self.wake_caret();
            return;
        }
        self.pull_clipboard_before_paste(keypress);
        let id = self.focused_buffer_id();
        let pane = self.focused_pane_id();
        // Standard keys read Shift (Shift+arrows select); Vim's don't.
        let vim_key = if self.vim.policy() == fenix_vim::EditPolicy::Standard { full } else { keypress };
        let vim_event = {
            let reverts_edits = self.buffers.get(id).is_some_and(|ob| self.buffer_edits_are_reverted(id, ob.kind));
            let Some(ob) = self.buffers.get_mut(id) else { return };
            let Some(pane_state) = self.workspaces.active_pane_states_mut().get_mut(&pane) else { return };
            let mode_before = self.vim.mode();
            let edit_count_before = ob.buffer.edit_count();
            // Enter, where an extension's grammar says what indents.
            if keypress == KeyPress::named(FenixNamedKey::Enter) && mode_before == Mode::Insert {
                let indent = ob.syntax.as_ref().filter(|syntax| syntax.language().is_grammar()).and_then(|syntax| {
                    let text = ob.buffer.text();
                    syntax.indents_after(&text, ob.buffer.char_to_byte(pane_state.cursor.char_idx))
                });
                self.vim.set_syntax_indent(indent);
            }
            let event = self.vim.handle_key(&mut ob.buffer, &mut pane_state.cursor, vim_key);
            // See `dispatch_keypress`'s own doc comment for `change_
            // capture`/`replaying_change` -- suppressed only while `.`
            // itself is replaying, so its own outer span doesn't get
            // polluted by the edits its replayed keys make.
            if ob.buffer.edit_count() != edit_count_before {
                if !self.replaying_change {
                    self.change_capture_dirty = true;
                }
                // Automatic `` `. `` mark (real Vim's own "position of
                // the last change") -- `resolve_mark`/`jump_to_mark`
                // already accept any char as a mark name, so writing
                // this entry is the only piece that's missing; `` `. ``
                // itself works for free the moment it exists.
                self.marks.insert('.', JumpEntry { buffer: id, char_idx: pane_state.cursor.char_idx });
                // Docker/Git/Jira panel buffers are read-only -- an edit
                // slipped through Vim's own dispatch anyway (there's no
                // clean "is this an edit" answer available *before* the
                // fact), so it's immediately reverted here instead. Safe
                // specifically because these buffers get wholesale-
                // regenerated on every refresh anyway -- a locally-
                // corrupted undo stack has no real consequence. See `is_
                // readonly_buffer_kind`'s own doc comment.
                if reverts_edits {
                    ob.buffer.undo(&mut pane_state.cursor);
                }
            }
            // Automatic `` `^ `` mark (real Vim's own "position when
            // Insert mode was last exited") -- same reasoning, on the
            // Insert -> non-Insert transition instead of an edit.
            if matches!(mode_before, Mode::Insert | Mode::Replace) && !self.vim.is_inserting() {
                let char_idx = self.vim.last_insert_exit().unwrap_or(pane_state.cursor.char_idx);
                self.marks.insert('^', JumpEntry { buffer: id, char_idx });
            }
            event
        };
        self.after_vim_key(keypress);
        self.push_clipboard_after_edit();
        self.sync_completion();
        // Same "cheap, derive fresh from whatever's under the cursor now"
        // re-sync for the Git panel's Main pane, after ordinary Vim
        // movement in Files/Commits/Stash (Branches has no diff of its
        // own to show -- see `git_sync_main`'s own doc comment).
        if matches!(self.git_focused_role(), Some(GitPaneRole::Staged | GitPaneRole::Unstaged | GitPaneRole::Commits | GitPaneRole::Stash)) {
            self.git_sync_main();
        }
        // Same again for the History view: moving down the graph shows
        // whichever commit the cursor lands on.
        if self.history_session.as_ref().is_some_and(|s| s.graph_pane == self.focused_pane_id()) {
            self.history_sync_detail();
        }
        match vim_event {
            VimEvent::RequestSessionSave => { self.save_session_explicit(); }
            VimEvent::RequestProjectNew(args) => self.project_new_with(&args),
            VimEvent::RequestSessionQuit => { if self.save_session_explicit() { if let Some(event_loop) = event_loop { event_loop.exit(); } } }
            VimEvent::RequestRestartLsp(name) => { self.restart_project_lsp(&name); }
            VimEvent::RequestUndoRefactor => { self.undo_refactor(); }
            VimEvent::RequestSave => {
                self.run_command_with(event_loop, "file.save");
            }
            VimEvent::RequestSaveAs(path) => {
                self.save_unnamed_as(&path);
            }
            VimEvent::RequestOpenFile { path, split } => {
                if let Some(way) = split {
                    self.run_command_with(event_loop, if way == fenix_vim::SplitWay::Vertical { "window.split_vertical" } else { "window.split_horizontal" });
                }
                self.open_typed_path(&path);
            }
            VimEvent::RunCommand(id) => {
                self.run_command_with(event_loop, id);
            }
            VimEvent::RequestForceSave => {
                self.run_command_with(event_loop, "file.save_force");
            }
            VimEvent::RequestReloadFile => {
                self.run_command_with(event_loop, "file.reload");
            }
            VimEvent::RequestCloseBuffer => {
                self.run_command_with(event_loop, "buffer.kill");
            }
            VimEvent::RequestForceCloseBuffer => {
                self.run_command_with(event_loop, "buffer.kill_force");
            }
            VimEvent::RequestSaveAndCloseBuffer => {
                self.run_command_with(event_loop, "buffer.save_and_kill");
            }
            VimEvent::RequestQuitAll => {
                self.run_command_with(event_loop, "app.quit");
            }
            VimEvent::RequestForceQuitAll => {
                self.run_command_with(event_loop, "app.quit_force");
            }
            VimEvent::RequestSaveAllAndQuit => {
                self.run_command_with(event_loop, "app.save_all_and_quit");
            }
            VimEvent::Error(msg) => self.set_error(msg),
            VimEvent::Message(msg) => self.set_message(msg),
            VimEvent::ScrollWindow(target) => self.scroll_window(target),
            VimEvent::Fold(command) => self.fold_command(command),
            VimEvent::RepeatLastChange { count } => self.repeat_last_change(count, event_loop),
            VimEvent::ToggleComment { start_line, end_line } => self.toggle_comment_lines(start_line, end_line),
            VimEvent::FilterLines { first, last, command } => self.filter_lines(first, last, &command),
            VimEvent::Pulse(range) => {
                if self.motion().yank_pulse {
                    self.pulse = Some(Pulse { range, started: Instant::now() });
                }
            }
            VimEvent::IndentWidthChanged(width) => {
                self.config.indent_width = Some(width);
                if let Err(err) = self.config.save() {
                    eprintln!("fenix: couldn't save indent width ({err})");
                }
            }
            VimEvent::IsKeywordChanged(chars) => {
                self.config.iskeyword_extra = Some(chars.into_iter().collect());
                if let Err(err) = self.config.save() {
                    eprintln!("fenix: couldn't save iskeyword ({err})");
                }
            }
            VimEvent::JumpRecorded(from) => {
                let buffer = self.focused_buffer_id();
                self.record_jump(JumpEntry { buffer, char_idx: from });
            }
            VimEvent::MarkSet(name, char_idx) => {
                let buffer = self.focused_buffer_id();
                self.marks.insert(name, JumpEntry { buffer, char_idx });
            }
            VimEvent::JumpToMark { name, linewise } => {
                self.jump_to_mark(name, linewise);
            }
            VimEvent::MacroRecordStart(register) => {
                // The register-select keystrokes (`q`, then the register
                // name) were already tapped into `macro_capture` by
                // `dispatch_keypress` before `VimState` ever resolved
                // them into this event -- real Vim doesn't record either
                // of those two keys, only what happens *after* recording
                // starts, so it's cleared here rather than left to
                // accumulate from this point on.
                self.macro_capture.clear();
                self.macro_recording_append = register.is_ascii_uppercase();
            }
            VimEvent::MacroRecordStop(register) => {
                // The closing `q` itself was also just tapped (recording
                // was still active when `dispatch_keypress` saw it) --
                // trimmed the same way, rather than included as part of
                // the macro's own content.
                self.macro_capture.pop();
                let keys = std::mem::take(&mut self.macro_capture);
                self.vim.finish_recording(register, &keys, self.macro_recording_append);
            }
            VimEvent::MacroPlay { register, count } => {
                self.play_macro(register, count, event_loop);
            }
            VimEvent::RequestLsp(kind) => {
                match kind {
                    fenix_vim::LspRequestKind::GoToDefinition => self.request_goto_definition(),
                    fenix_vim::LspRequestKind::References => self.request_references(),
                    // Stopped, on a name: the lens, its value.
                    fenix_vim::LspRequestKind::Hover => {
                        if !self.debug_lens() {
                            self.request_hover();
                        }
                    }
                    fenix_vim::LspRequestKind::FileUnderCursor => self.follow_link_under_cursor(),
                }
            }
            // Along the Ember Trail while there's one here; the TODOs
            // otherwise.
            VimEvent::BracketJump { target: fenix_vim::BracketTarget::Todo, forward, count } if self.debug_trail_here() => self.debug_trail_scrub(forward, count as usize),
            VimEvent::BracketJump { target: fenix_vim::BracketTarget::Todo, forward, count } => self.jump_to_todo(forward, count),
            VimEvent::BracketJump { target: fenix_vim::BracketTarget::Hunk, forward, count } => self.jump_to_hunk(forward, count),
            VimEvent::BracketJump { target: fenix_vim::BracketTarget::Diagnostic, forward, count } => self.jump_to_diagnostic(forward, count),
            VimEvent::BracketJump { target: fenix_vim::BracketTarget::Change, forward, count } => self.side_by_side_jump(forward, count),
            VimEvent::BracketJump { target: fenix_vim::BracketTarget::AgentHunk, forward, count } => self.agents_jump_to_hunk(forward, count),
            VimEvent::Diff { put } => self.side_by_side_take(put),
            VimEvent::Tab(mv) => self.move_tab(mv),
            VimEvent::None => {}
        }
        self.wake_caret();
    }

    /// Left-click: focuses whatever the click landed on, and in a pane's
    /// text puts the cursor on the glyph clicked (`text_press`). Mirrors the exact flag
    /// semantics `explorer_quit`/`navigate_window`'s own sidebar hand-
    /// off already use: `sidebar_focused`/`terminal_focused` are plain
    /// override flags independent of the tree's own focused pane, so
    /// focusing the sidebar/terminal never touches `windows_mut()`, and
    /// focusing a pane always clears both.
    pub(super) fn handle_click(&mut self, pos: (f32, f32)) {
        let Some((window_width, window_height)) = self.gpu.as_ref().map(|gpu| (gpu.size.width as f32, gpu.size.height as f32))
        else {
            return;
        };
        self.handle_click_at(pos, window_width, window_height);
    }

    /// The GPU-independent core of `handle_click` -- takes the window's
    /// dimensions as plain parameters instead of reading `self.gpu`
    /// (which no headless test ever constructs), so it's directly
    /// testable the same way `frame_geometry` already is.
    pub(super) fn handle_click_at(&mut self, pos: (f32, f32), window_width: f32, window_height: f32) {
        // A click that moves the cursors is a move Ctrl+U takes back.
        let before = self.caret_mark();
        self.handle_click_at_inner(pos, window_width, window_height);
        self.note_caret_move(before);
    }

    pub(super) fn handle_click_at_inner(&mut self, pos: (f32, f32), window_width: f32, window_height: f32) {
        let (sidebar_px, terminal_h, modeline_top) = self.frame_metrics(window_height);
        let geometry = self.frame_geometry(window_width, sidebar_px, terminal_h, modeline_top);
        if self.click_breadcrumb(&geometry, pos) { return }
        match hit_test(&geometry, pos) {
            Some(ScrollTarget::Sidebar) => {
                self.sidebar_focused = true;
                self.terminal_focused = false;
                let (line, col) = self.cell_at(pos, (text::STRIP_INSET, text::STRIP_INSET), 0);
                self.explorer_click(ExplorerTarget::Sidebar, line, col);
            }
            Some(ScrollTarget::Terminal) => {
                self.terminal_focused = true;
                self.sidebar_focused = false;
            }
            Some(ScrollTarget::Pane(id)) => {
                self.windows_mut().focus(id);
                self.sidebar_focused = false;
                self.terminal_focused = false;
                if let Some(&(_, rect)) = geometry.panes.iter().find(|(p, _)| *p == id) {
                    if self.ruler_click(id, rect, pos) {
                        self.wake_caret();
                        return;
                    }
                    let line_height = self.text.as_ref().map(|t| t.line_height()).unwrap_or(text::LINE_HEIGHT);
                    let content = pane_content_rect(rect, line_height, self.pane_has_breadcrumb(id));
                    if pos.1 >= content.y && self.explorer_in_pane(id).is_none() {
                        let (row, col) = self.cell_at(pos, (content.x, content.y), 0);
                        if self.fold_marker_click(id, row, col) {
                            return;
                        }
                        // A test's mark: it runs (§9.1) -- a click on its own
                        // cell only, never left of the text's padding, where
                        // the Git bar and a breakpoint's square are.
                        if pos.0 - content.x >= text::PAD_LEFT && self.tests_mark_click(id, row, col) {
                            return;
                        }
                    }
                    // A listing's row, chevron or breadcrumb.
                    if let Some(target) = self.explorer_in_pane(id) {
                        let line_height = self.text.as_ref().map(|t| t.line_height()).unwrap_or(text::LINE_HEIGHT);
                        let content = pane_content_rect(rect, line_height, self.pane_has_breadcrumb(id));
                        if pos.1 >= content.y {
                            let (line, col) = self.cell_at(pos, (content.x, content.y), self.pane_state(id).scroll_line);
                            self.explorer_click(target, line, col);
                        }
                        self.wake_caret();
                        return;
                    }
                    // The welcome's caps: a click presses the key on one.
                    if pos.1 >= content.y {
                        if let Some(&page) = self.windows().content(id) {
                            let (line, col) = self.cell_at(pos, (content.x, content.y), self.pane_state(id).scroll_line);
                            if self.welcome_click(page, line, col) {
                                self.wake_caret();
                                return;
                            }
                        }
                    }
                    // Standard keys: an answer on an agent's question card.
                    if pos.1 >= content.y && self.standard_keys() {
                        if let Some(&page) = self.windows().content(id) {
                            let (line, _) = self.cell_at(pos, (content.x, content.y), self.pane_state(id).scroll_line);
                            if self.agents_card_click(page, line) {
                                self.wake_caret();
                                return;
                            }
                        }
                    }
                    // A page's text: a drag over it selects it.
                    if pos.1 >= content.y && self.page_select_press(id, (content.x, content.y), pos) {
                        self.wake_caret();
                        return;
                    }
                    // Text: the cursor to the glyph, a selection started.
                    if pos.1 >= content.y && self.text_press(id, rect, pos, Instant::now()) {
                        self.unfocus_terminal_buffer();
                        self.code_surface_sync_capture();
                        self.wake_caret();
                        return;
                    }
                }
                // Clicking a surface that captures the keyboard (a remote
                // desktop) both focuses and starts interacting with it
                // (ordinary desktop convention); clicking anything else
                // releases capture if some other surface had it.
                self.code_surface_sync_capture();
                // Same convention for a terminal pane: clicking into a
                // shell means you intend to type in it. Clicking any
                // other pane releases the capture, so a click is also
                // the way out, not just `Ctrl-\`.
                match self.windows().content(id).copied().filter(|b| self.terminal_buffers.contains_key(b)) {
                    Some(buffer) => self.focus_terminal_buffer(buffer),
                    None => self.unfocus_terminal_buffer(),
                }
            }
            Some(ScrollTarget::SplitPane(pane)) => {
                self.windows_mut().focus(pane);
                self.sidebar_focused = false;
                self.terminal_focused = false;
                self.split_vertical();
                self.refresh_project_root();
            }
            Some(ScrollTarget::SwitchTab(pane, buffer)) => {
                self.windows_mut().focus(pane);
                self.sidebar_focused = false;
                self.terminal_focused = false;
                self.set_pane_content(pane, buffer);
                self.tab_clicked(pane, buffer);
                // As switching tabs from the keyboard does: the project,
                // the title and each project's sidebar follow the tab.
                self.refresh_project_root();
            }
            Some(ScrollTarget::CloseTab(pane, buffer)) if self.workspaces.active_workspace().home == Some(buffer) => {
                self.windows_mut().focus(pane);
                self.sidebar_focused = false;
                self.terminal_focused = false;
                self.set_pane_content(pane, buffer);
            }
            Some(ScrollTarget::CloseTab(pane, buffer)) => self.close_pane_tab(pane, buffer),
            None => return,
        }
        self.wake_caret();
    }

    pub(super) fn handle_wheel(&mut self, pos: (f32, f32), delta: MouseScrollDelta) {
        let Some((window_width, window_height)) = self.gpu.as_ref().map(|gpu| (gpu.size.width as f32, gpu.size.height as f32))
        else {
            return;
        };
        self.handle_wheel_at(pos, delta, window_width, window_height);
    }

    /// The GPU-independent core of `handle_wheel` -- see `handle_click_
    /// at`'s own doc comment for why this split exists.
    pub(super) fn handle_wheel_at(&mut self, pos: (f32, f32), delta: MouseScrollDelta, window_width: f32, window_height: f32) {
        let (sidebar_px, terminal_h, modeline_top) = self.frame_metrics(window_height);
        let line_height = self.text.as_ref().map(|t| t.line_height()).unwrap_or(text::LINE_HEIGHT);
        self.motion.key_repeat = false;
        // A trackpad's small moves add up instead of each rounding away.
        let lines = match delta {
            MouseScrollDelta::PixelDelta(p) => {
                let total = self.motion.wheel_remainder + p.y as f32 / line_height;
                let whole = total.trunc();
                self.motion.wheel_remainder = total - whole;
                whole as isize
            }
            _ => wheel_delta_lines(delta, line_height),
        };
        if lines == 0 {
            return;
        }
        let geometry = self.frame_geometry(window_width, sidebar_px, terminal_h, modeline_top);
        let Some(target) = hit_test(&geometry, pos) else { return };
        // A wheel notch landing on a tab or its close glyph scrolls that
        // tab's own pane, same as landing anywhere else in it -- neither
        // "switch tab" nor "close tab" is a wheel gesture.
        let target = match target {
            ScrollTarget::SwitchTab(pane, _) | ScrollTarget::CloseTab(pane, _) | ScrollTarget::SplitPane(pane) => ScrollTarget::Pane(pane),
            other => other,
        };
        match target {
            ScrollTarget::Sidebar => self.explorer_wheel(ExplorerTarget::Sidebar, lines),
            ScrollTarget::Terminal => {
                if let Some(terminal) = &mut self.terminal {
                    terminal.session.scroll(lines);
                }
            }
            ScrollTarget::Pane(pane) => {
                if self.code_surface_wheel(pane, pos, lines) || self.code_page_wheel(pane, pos, lines) {
                    // A surface turns the wheel itself: a page scrolled, a
                    // zoom; so does a page that takes the mouse.
                } else if let Some(target) = self.explorer_in_pane(pane) {
                    // A listing lays out the rows that fit itself, so
                    // the wheel scrolls those.
                    self.explorer_wheel(target, lines);
                } else if self.windows().content(pane).is_some_and(|b| self.terminal_buffers.contains_key(b)) {
                    // A terminal pane's history lives in the `vt100`
                    // parser's own scrollback, not in the buffer's rope
                    // (which is empty), so the wheel moves that instead
                    // -- the same thing it does over the popup panel.
                    if let Some(state) = self.windows().content(pane).copied().and_then(|b| self.terminal_buffers.get_mut(&b)) {
                        state.session.scroll(lines);
                    }
                } else if pane == self.focused_pane_id() {
                    // A wrapped pane's view goes by rows, the cursor only
                    // as far as keeps it in view. One by rows only for the
                    // rows above its lines (a code lens's) moves its cursor
                    // by lines, as 1.5 did, and the view follows.
                    let wrapped = self.main_view == MainView::Editor
                        && self.wrap_cols(pane, self.focused_buffer_id()).is_some()
                        && geometry.panes.iter().find(|(id, _)| *id == pane).is_some_and(|(_, rect)| {
                            let visible = text::lines_that_fit(pane_content_rect(*rect, line_height, self.pane_has_breadcrumb(pane)).h, line_height);
                            let buffer = self.focused_buffer_id();
                            self.scroll_focused_by_rows(pane, buffer, -lines, visible)
                        });
                    if !wrapped {
                        self.scroll_focused_pane(lines);
                    }
                } else if let Some((_, rect)) = geometry.panes.iter().find(|(id, _)| *id == pane) {
                    // Match `redraw`'s own chrome shrink for this pane so
                    // `max_scroll` agrees with what's actually visible.
                    let rect_h = pane_content_rect(*rect, line_height, self.pane_has_breadcrumb(pane)).h;
                    self.scroll_unfocused_pane(pane, lines, rect_h, line_height);
                }
            }
            ScrollTarget::SwitchTab(..) | ScrollTarget::CloseTab(..) | ScrollTarget::SplitPane(..) => unreachable!("normalized to Pane above"),
        }
        self.wake_caret();
    }
}
