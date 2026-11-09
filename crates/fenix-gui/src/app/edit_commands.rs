//! Commands for actions that were only Vim keys, Ex commands or the
//! right-click menu's: hover, definition, references, the clipboard,
//! comments, moving and sorting lines, going to a line, and the panes'
//! and tabs' moves. Both key schemes bind them (`SPC h c` lists them).

use fenix_keymap::{KeyCode, KeyPress, Mods, NamedKey as FenixNamedKey};
use fenix_vim::{Mode, VisualKind};

use super::{App, BufferId};

/// A sort `edit.sort_lines…` asks for: `:sort`'s flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SortLines {
    Plain,
    Reverse,
    IgnoreCase,
    Numeric,
    Unique,
}

impl App {
    /// `code.hover`: what `K` shows -- the debugger's lens while stopped
    /// on a name, the language server's hover otherwise.
    pub(crate) fn cmd_hover(&mut self) {
        if !self.debug_lens() {
            self.request_hover();
        }
    }

    /// The lines a line command works on: the selection's, else the
    /// cursor's.
    pub(crate) fn selected_lines(&self) -> (usize, usize) {
        let pane = self.focused_pane_id();
        let Some(state) = self.workspaces.active_pane_states().get(&pane) else { return (0, 0) };
        let buffer = &self.open().buffer;
        let (line, _) = buffer.line_col(&state.cursor);
        if self.vim.mode() == Mode::Visual {
            let anchor = self.vim.visual_anchor().min(buffer.rope().len_chars());
            let anchor_line = buffer.rope().char_to_line(anchor);
            return (anchor_line.min(line), anchor_line.max(line));
        }
        (line, line)
    }

    /// `code.toggle_comment`: `gcc` on the cursor's line, or `gc` over the
    /// selection's lines.
    pub(crate) fn cmd_toggle_comment(&mut self) {
        let (start, end) = self.selected_lines();
        self.toggle_comment_lines(start, end);
        if self.vim.mode() == Mode::Visual {
            self.leave_visual();
        }
    }

    /// Leaves Visual as Escape would.
    pub(crate) fn leave_visual(&mut self) {
        let pane = self.focused_pane_id();
        if let Some(state) = self.workspaces.active_pane_states().get(&pane) {
            let cursor = state.cursor;
            self.vim.exit_visual_mode(&cursor);
        }
    }

    /// Runs Vim keys from Normal (or Visual) mode: out of Insert first,
    /// where the cursor is, and back into it after if it was there.
    pub(crate) fn vim_keys_from_normal(&mut self, keys: &[KeyPress]) {
        let was_inserting = self.vim.is_inserting();
        if was_inserting {
            self.leave_insert_in_place();
        }
        self.mouse_vim_keys(keys);
        if was_inserting && self.vim.mode() == Mode::Normal {
            self.enter_insert_in_place();
        }
    }

    /// Leaves Insert without Escape's step back a character.
    pub(crate) fn leave_insert_in_place(&mut self) {
        if !self.vim.is_inserting() {
            return;
        }
        let id = self.focused_buffer_id();
        if let Some(ob) = self.buffers.get_mut(id) {
            self.vim.leave_insert_here(&mut ob.buffer);
        }
    }

    /// Goes into Insert where the cursor is, as `i` does.
    pub(crate) fn enter_insert_in_place(&mut self) {
        if self.vim.mode() != Mode::Normal {
            return;
        }
        let (id, pane) = (self.focused_buffer_id(), self.focused_pane_id());
        if let (Some(ob), Some(state)) = (self.buffers.get(id), self.workspaces.active_pane_states_mut().get_mut(&pane)) {
            self.vim.enter_insert_here(&ob.buffer, &mut state.cursor);
        }
    }

    /// An Ex command line run as if typed after `:`, from Normal mode
    /// (a selection is left first: give the line its range).
    pub(crate) fn run_ex_line(&mut self, line: &str) {
        if self.vim.mode() == Mode::Visual {
            self.leave_visual();
        }
        let mut keys = vec![KeyPress::char(':')];
        keys.extend(line.chars().map(KeyPress::char));
        keys.push(KeyPress::named(FenixNamedKey::Enter));
        self.vim_keys_from_normal(&keys);
    }

    /// `edit.sort_lines…`: the selection's lines sorted, or the whole
    /// buffer's.
    pub(crate) fn cmd_sort_lines(&mut self, how: SortLines) {
        let (start, end) = self.selected_lines();
        let range = if self.vim.mode() == Mode::Visual { format!("{},{}", start + 1, end + 1) } else { "%".to_string() };
        let line = match how {
            SortLines::Plain => format!("{range}sort"),
            SortLines::Reverse => format!("{range}sort!"),
            SortLines::IgnoreCase => format!("{range}sort i"),
            SortLines::Numeric => format!("{range}sort n"),
            SortLines::Unique => format!("{range}sort u"),
        };
        self.run_ex_line(&line);
    }

    /// `edit.move_lines_up` / `_down`: the cursor's line, or the
    /// selection's, one line up or down; the selection stays on them.
    pub(crate) fn cmd_move_lines(&mut self, down: bool) {
        let (start, end) = self.selected_lines();
        let last = self.open().buffer.line_count().saturating_sub(1);
        if (down && end >= last) || (!down && start == 0) {
            return;
        }
        let visual = self.vim.mode() == Mode::Visual;
        let target = if down { end + 2 } else { start.saturating_sub(1) };
        // `:m` takes the line to put them after, numbered from 1.
        self.run_ex_line(&format!("{},{}m{}", start + 1, end + 1, target));
        if visual {
            self.reselect_lines(if down { start + 1 } else { start - 1 }, if down { end + 1 } else { end - 1 });
        }
    }

    /// `edit.copy_lines_up` / `_down`: the cursor's line, or the
    /// selection's, copied above or below them.
    pub(crate) fn cmd_copy_lines(&mut self, down: bool) {
        let (start, end) = self.selected_lines();
        let visual = self.vim.mode() == Mode::Visual;
        let after = if down { end + 1 } else { start };
        self.run_ex_line(&format!("{},{}t{}", start + 1, end + 1, after));
        let n = end - start;
        let (first, last) = if down { (end + 1, end + 1 + n) } else { (start, end) };
        if visual {
            self.reselect_lines(first, last);
        } else {
            self.put_cursor_on_line(first);
        }
    }

    /// The cursor to line `line`'s first character.
    fn put_cursor_on_line(&mut self, line: usize) {
        let pane = self.focused_pane_id();
        let at = self.open().buffer.line_start_char(line);
        self.put_cursor(pane, at);
    }

    /// Lines `first..=last` selected linewise.
    fn reselect_lines(&mut self, first: usize, last: usize) {
        let pane = self.focused_pane_id();
        let (a, b) = (self.open().buffer.line_start_char(first), self.open().buffer.line_start_char(last));
        if self.vim.is_inserting() {
            self.leave_insert_in_place();
        }
        self.vim.select(VisualKind::Line, a);
        self.put_cursor(pane, b);
    }

    /// The selection's range, its text, and whether it's whole lines.
    fn selection(&self) -> Option<(std::ops::Range<usize>, String, bool)> {
        let pane = self.focused_pane_id();
        let cursor = self.workspaces.active_pane_states().get(&pane)?.cursor;
        let buffer = &self.open().buffer;
        let (range, linewise) = self.vim.visual_selection_range(buffer, &cursor)?;
        let text = buffer.rope().slice(range.clone()).to_string();
        Some((range, text, linewise))
    }

    /// `edit.copy`: the selection to the clipboard, or the cursor's line.
    pub(crate) fn cmd_copy(&mut self) {
        if let Some(text) = self.copy_every_caret() {
            self.vim.set_register(text, false);
            self.push_clipboard_after_edit();
            return;
        }
        if let Some((range, text, linewise)) = self.selection() {
            self.vim.set_register(text, linewise);
            self.push_clipboard_after_edit();
            // Vim's `y` leaves Visual at the selection's start; standard
            // keys keep what's selected selected.
            if self.vim.policy() == fenix_vim::EditPolicy::Vim {
                let pane = self.focused_pane_id();
                self.leave_visual();
                self.put_cursor(pane, range.start);
            }
            return;
        }
        let pane = self.focused_pane_id();
        let at = self.workspaces.active_pane_states().get(&pane).map(|s| s.cursor.char_idx);
        self.vim_keys_from_normal(&[KeyPress::char('y'), KeyPress::char('y')]);
        // Copying the line doesn't move you.
        if let (Some(at), Some(state)) = (at, self.workspaces.active_pane_states_mut().get_mut(&pane)) {
            state.cursor.char_idx = at;
        }
        self.push_clipboard_after_edit();
    }

    /// `edit.cut`: the selection to the clipboard, or the cursor's line.
    pub(crate) fn cmd_cut(&mut self) {
        if let Some((range, text, linewise)) = self.selection() {
            self.vim.set_register(text, linewise);
            self.push_clipboard_after_edit();
            self.delete_selected(range);
            return;
        }
        self.vim_keys_from_normal(&[KeyPress::char('d'), KeyPress::char('d')]);
        self.push_clipboard_after_edit();
    }

    /// `range` deleted and the selection over, the cursor where it was.
    fn delete_selected(&mut self, range: std::ops::Range<usize>) {
        self.leave_visual();
        let pane = self.focused_pane_id();
        let (buffer, cursor) = self.focused_buffer_and_cursor_mut();
        buffer.begin_undo_group();
        if !range.is_empty() {
            buffer.delete_range(cursor, range.start, range.end.min(buffer.len_chars()));
        }
        buffer.end_undo_group();
        let at = range.start.min(self.open().buffer.len_chars());
        self.put_cursor(pane, at);
    }

    /// `edit.paste`: the clipboard at the cursor, or over the selection.
    pub(crate) fn cmd_paste(&mut self) {
        let text = self.clipboard_text().or_else(|| Some(self.vim.register().0.to_string())).filter(|t| !t.is_empty());
        if self.vim.policy() == fenix_vim::EditPolicy::Vim && !self.vim.is_inserting() {
            self.run_menu_action(super::mouse::MenuAction::Paste);
            return;
        }
        let Some(text) = text else { return };
        if self.paste_at_every_caret(&text) {
            return;
        }
        if let Some((range, _, _)) = self.selection() {
            self.delete_selected(range);
        }
        let linewise = text.ends_with('\n') && !self.vim.is_inserting();
        let (buffer, cursor) = self.focused_buffer_and_cursor_mut();
        if linewise {
            let (line, _) = buffer.line_col(cursor);
            cursor.char_idx = buffer.line_start_char(line);
        }
        buffer.break_undo_run();
        buffer.insert_str(cursor, &text);
        buffer.break_undo_run();
        let (_, col) = buffer.line_col(cursor);
        cursor.sticky_col = col;
    }

    /// `edit.select_all`.
    pub(crate) fn cmd_select_all(&mut self) {
        self.run_menu_action(super::mouse::MenuAction::SelectAll);
    }

    /// `window.focus_N`: the `n`th pane, in reading order, from 1.
    pub(crate) fn cmd_focus_nth_pane(&mut self, n: usize) {
        let panes = self.windows().windows();
        let Some(&pane) = panes.get(n.saturating_sub(1)) else {
            self.set_message(format!("there's no pane {n} -- {} open", panes.len()));
            return;
        };
        self.windows_mut().focus(pane);
        self.sidebar_focused = false;
        self.terminal_focused = false;
        self.refresh_project_root();
        self.wake_caret();
    }

    /// `window.move_tab_next` / `_prev`: the tab in front to the next pane
    /// (or the one before), which shows it.
    pub(crate) fn cmd_move_tab_to_next_pane(&mut self, forward: bool) {
        let panes = self.windows().windows();
        if panes.len() < 2 {
            self.set_message("one pane -- split first (SPC w v)");
            return;
        }
        let from = self.focused_pane_id();
        let Some(i) = panes.iter().position(|&p| p == from) else { return };
        let to = panes[if forward { (i + 1) % panes.len() } else { (i + panes.len() - 1) % panes.len() }];
        let id: BufferId = self.focused_buffer_id();
        self.move_tab_to_pane(from, id, to);
    }

    /// `window.focus_sidebar`: the file explorer's sidebar, opened if it
    /// isn't, with the keyboard; again, back to the editor.
    pub(crate) fn cmd_focus_sidebar(&mut self) {
        if self.sidebar_open && self.sidebar_focused {
            self.sidebar_focused = false;
        } else {
            if !self.sidebar_open {
                self.open_sidebar();
            }
            self.sidebar_focused = self.sidebar_open;
            self.terminal_focused = false;
        }
        self.wake_caret();
    }

    /// `terminal.focus`: to the terminal panel, opening it if it's
    /// hidden; from it, back to the editor.
    pub(crate) fn cmd_focus_terminal(&mut self) {
        if self.terminal_open && self.terminal_focused {
            self.terminal_focused = false;
        } else if !self.terminal_open {
            self.toggle_terminal();
        } else {
            self.terminal_focused = self.terminal.as_mut().is_some_and(|t| t.session.is_alive());
            self.sidebar_focused = false;
        }
        self.wake_caret();
    }

    /// `nav.goto_line`: asks for a line, and goes there.
    pub(crate) fn start_goto_line_prompt(&mut self) {
        self.goto_line_prompt = Some(String::new());
        self.wake_caret();
    }

    /// A key while the go-to-line prompt is open.
    pub(super) fn goto_line_prompt_key(&mut self, key: KeyPress) {
        let Some(input) = &mut self.goto_line_prompt else { return };
        match key.code {
            KeyCode::Named(FenixNamedKey::Escape) => self.goto_line_prompt = None,
            KeyCode::Named(FenixNamedKey::Enter) => {
                let input = self.goto_line_prompt.take().unwrap_or_default();
                let lines = self.open().buffer.line_count();
                match input.trim().parse::<usize>() {
                    Ok(n) if n >= 1 => {
                        let line = n.min(lines) - 1;
                        let from = super::JumpEntry { buffer: self.focused_buffer_id(), char_idx: self.cursor().char_idx };
                        self.record_jump(from);
                        self.put_cursor_on_line(line);
                    }
                    _ if input.trim().is_empty() => {}
                    _ => self.set_error(format!("{} isn't a line number", input.trim())),
                }
            }
            KeyCode::Named(FenixNamedKey::Backspace) => {
                input.pop();
            }
            KeyCode::Char(c) if key.mods == Mods::default() && c.is_ascii_digit() => input.push(c),
            _ => {}
        }
        self.wake_caret();
    }

    /// What the modeline says while the go-to-line prompt is open.
    pub(super) fn goto_line_prompt_text(&self) -> Option<String> {
        let input = self.goto_line_prompt.as_ref()?;
        Some(format!("go to line (1-{}): {input}", self.open().buffer.line_count()))
    }

    /// `view.key_scheme`: Vim keys or standard keys, switched now.
    pub(crate) fn cmd_toggle_key_scheme(&mut self) {
        let next = if self.standard_keys() { "vim" } else { "standard" };
        match self.set_setting("editor.keys", Some(fenix_config::Value::Text(next.to_string()))) {
            Ok(()) => {
                let lead = if next == "standard" { format!("Standard keys -- {} opens the leader's menu, SPC t k (as {} t k) switches back", self.leader_chord_text(), self.leader_chord_text()) } else { "Vim keys -- SPC opens the leader's menu, SPC t k switches back".to_string() };
                self.set_message(lead);
            }
            Err(e) => self.set_error(e),
        }
    }

    /// The leader chord, as which-key writes it.
    pub(crate) fn leader_chord_text(&self) -> String {
        crate::keymap::describe_keypress(&self.leader_chord())
    }

    /// The chord that opens the leader with standard keys:
    /// `keys.leader_chord`, else `Ctrl+K`.
    pub(crate) fn leader_chord(&self) -> KeyPress {
        self.config.text_value("keys.leader_chord").and_then(crate::keymap::parse_key).unwrap_or_else(|| KeyPress::char('k').with_ctrl())
    }

    /// `help.keys_sheet`: every chord in use and the leader's groups, by
    /// area, on a page of their own.
    pub(crate) fn open_keys_sheet(&mut self) {
        let text = self.keys_sheet_text();
        let path = std::env::temp_dir().join(format!("fenix-keys-{}.md", self.key_scheme().name()));
        if let Err(e) = std::fs::write(&path, text) {
            self.set_error(format!("couldn't write the keys page: {e}"));
            return;
        }
        self.open_file_from_picker(&path);
    }

    /// The keys page's Markdown.
    pub(crate) fn keys_sheet_text(&self) -> String {
        let scheme = self.key_scheme();
        let describe = |id: &str| self.commands().get(id).map(|c| c.description.to_string()).unwrap_or_default();
        let leader = if scheme == crate::keymap::Scheme::Standard { self.leader_chord_text() } else { "SPC".to_string() };
        let mut out = format!(
            "# Keys\n\n{} keys. `{leader}` opens the leader's menu: its groups are below. `SPC h b` lists every key, `SPC t k` switches between Vim and standard keys.\n\n## Chords\n\n| Keys | Runs | What it does |\n|---|---|---|\n",
            if scheme == crate::keymap::Scheme::Standard { "Standard" } else { "Vim" }
        );
        let mut chords: Vec<_> = crate::keymap::chord_map().in_use(scheme).into_iter().filter_map(|c| Some((c.command?, crate::keymap::describe_keypress(&c.key)))).collect();
        chords.sort_by(|a, b| a.0.cmp(b.0).then(a.1.cmp(&b.1)));
        for (id, keys) in chords {
            out.push_str(&format!("| `{keys}` | `{id}` | {} |\n", describe(id).replace('|', "\\|")));
        }
        out.push_str("\n## The leader's groups\n\n| Keys | Group |\n|---|---|\n");
        let mut m = crate::keymap::leader_trie().matcher();
        m.feed(KeyPress::char(' '));
        for hint in m.pending_hints().into_iter().filter(|h| h.group) {
            out.push_str(&format!("| `{leader} {}` | {} ({} keys) |\n", crate::keymap::describe_keypress(&hint.key), hint.label, hint.count));
        }
        out
    }

    /// `buffer.switch_recent`: the recently used buffers, the one before
    /// this already picked -- Enter (or letting go of Ctrl) goes to it,
    /// Ctrl+Tab again steps on.
    pub(crate) fn cmd_switch_recent(&mut self) {
        self.picker_switch_buffer();
        if let Some(super::ActivePicker::SwitchBuffer(picker)) = &mut self.active_picker {
            picker.move_selection(1);
        }
        self.switching_recent = true;
    }
}

impl App {
    /// Ctrl let go: the recent-buffer switcher, if it's up, goes to its
    /// row.
    pub(crate) fn recent_switch_released(&mut self) {
        if !std::mem::take(&mut self.switching_recent) {
            return;
        }
        if matches!(self.active_picker, Some(super::ActivePicker::SwitchBuffer(_))) {
            self.picker_key(KeyPress::named(FenixNamedKey::Enter));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_keys::app_on_file;

    #[test]
    fn toggle_comment_comments_the_line_then_the_selection_and_back() {
        let (mut app, _f) = app_on_file("a.rs", "fn a() {}\nfn b() {}\nfn c() {}\n");
        app.run_cmd("code.toggle_comment");
        assert_eq!(app.text(), "// fn a() {}\nfn b() {}\nfn c() {}\n");
        app.run_cmd("code.toggle_comment");
        assert_eq!(app.text(), "fn a() {}\nfn b() {}\nfn c() {}\n");
        app.press_all("V j");
        app.run_cmd("code.toggle_comment");
        assert_eq!(app.text(), "// fn a() {}\n// fn b() {}\nfn c() {}\n");
        assert_eq!(app.vim.mode(), fenix_vim::Mode::Normal, "the selection's done with");
    }

    #[test]
    fn lines_move_and_copy_up_and_down_and_a_selection_follows_them() {
        let (mut app, _f) = app_on_file("a.txt", "one\ntwo\nthree\n");
        app.run_cmd("edit.move_lines_down");
        assert_eq!(app.text(), "two\none\nthree\n");
        assert_eq!(app.line_col().0, 1, "the cursor goes with its line");
        app.run_cmd("edit.move_lines_up");
        assert_eq!(app.text(), "one\ntwo\nthree\n");
        app.run_cmd("edit.move_lines_up");
        assert_eq!(app.text(), "one\ntwo\nthree\n", "the first line can't go up");
        app.press_all("V j");
        app.run_cmd("edit.move_lines_down");
        assert_eq!(app.text(), "three\none\ntwo\n");
        assert_eq!(app.selected_lines(), (1, 2), "still selected");
        app.press("Esc");
        app.press("g");
        app.press("g");
        app.run_cmd("edit.copy_lines_down");
        assert_eq!(app.text(), "three\nthree\none\ntwo\n");
        assert_eq!(app.line_col().0, 1, "on the copy");
    }

    #[test]
    fn sort_lines_sorts_the_buffer_or_the_selection() {
        let (mut app, _f) = app_on_file("a.txt", "b\nc\na\nb\n");
        app.run_cmd("edit.sort_lines");
        assert_eq!(app.text(), "a\nb\nb\nc\n");
        app.run_cmd("edit.sort_lines_unique");
        assert_eq!(app.text(), "a\nb\nc\n");
        app.run_cmd("edit.sort_lines_reverse");
        assert_eq!(app.text(), "c\nb\na\n");
        app.press_all("j V j");
        app.run_cmd("edit.sort_lines");
        assert_eq!(app.text(), "c\na\nb\n", "only the selected lines");
    }

    #[test]
    fn go_to_line_asks_for_a_number_and_goes_there() {
        let (mut app, _f) = app_on_file("a.txt", "one\ntwo\nthree\n");
        app.run_cmd("nav.goto_line");
        assert!(app.goto_line_prompt_text().is_some_and(|t| t.starts_with("go to line")), "{:?}", app.goto_line_prompt_text());
        app.type_text("3x");
        app.press("Enter");
        assert_eq!(app.line_col(), (2, 0));
        assert!(app.goto_line_prompt.is_none());
        app.run_cmd("nav.goto_line");
        app.type_text("99");
        app.press("Enter");
        assert_eq!(app.line_col().0, 3, "past the end is the last line");
        app.press("C-o");
        assert_eq!(app.line_col().0, 2, "a jump, on the jumplist");
    }

    #[test]
    fn copy_and_cut_take_the_line_without_a_selection_and_paste_puts_it_back() {
        let (mut app, _f) = app_on_file("a.txt", "one\ntwo\n");
        app.press("l");
        app.run_cmd("edit.copy");
        assert_eq!(app.vim.register(), ("one\n", true));
        assert_eq!(app.line_col(), (0, 1), "copying doesn't move you");
        app.run_cmd("edit.cut");
        assert_eq!(app.text(), "two\n");
        app.press_all("v l");
        app.run_cmd("edit.copy");
        assert_eq!(app.vim.register().0, "tw");
        app.run_cmd("edit.select_all");
        assert_eq!(app.vim.mode(), fenix_vim::Mode::Visual);
    }

    #[test]
    fn panes_are_focused_by_number_and_a_tab_moves_to_the_next_one() {
        let (mut app, _f) = app_on_file("a.txt", "one\n");
        let file = app.focused_buffer_id();
        app.run_cmd("window.split_vertical");
        let panes = app.windows().windows();
        assert_eq!(panes.len(), 2);
        app.run_cmd("window.focus_1");
        assert_eq!(app.focused_pane_id(), panes[0]);
        app.run_cmd("window.focus_2");
        assert_eq!(app.focused_pane_id(), panes[1]);
        app.run_cmd("window.focus_3");
        assert_eq!(app.focused_pane_id(), panes[1], "no pane 3: stays");
        app.run_cmd("window.focus_1");
        app.run_cmd("window.move_tab_next");
        assert_eq!(app.focused_pane_id(), panes[1], "the tab's pane is focused");
        assert_eq!(app.windows().content(panes[1]), Some(&file));
        assert_ne!(app.windows().content(panes[0]), Some(&file), "it left the first");
    }

    #[test]
    fn the_new_leader_keys_reach_their_commands() {
        let trie = crate::keymap::leader_trie();
        let seq = |text: &str| crate::keymap::parse_keys(text).unwrap();
        assert_eq!(trie.get(&seq("SPC ] d")), Some(&"nav.next_problem"));
        assert_eq!(trie.get(&seq("SPC [ h")), Some(&"nav.prev_hunk"));
        assert_eq!(trie.get(&seq("SPC C-i")), Some(&"code.hover"));
        assert_eq!(trie.get(&seq("SPC w Left")), Some(&"window.navigate_left"));
        assert_eq!(trie.get(&seq("SPC t k")), Some(&"view.key_scheme"));
        // Through the real key path: SPC, then Ctrl+I, isn't the jumplist's.
        let (mut app, _f) = app_on_file("a.txt", "one\ntwo\n");
        app.press_all("j SPC C-i");
        assert!(!app.leader_matcher.is_pending(), "the leader took it");
        assert_eq!(app.line_col().0, 1, "not a jump");
    }

    #[test]
    fn the_keys_page_lists_the_chords_and_the_leader_s_groups() {
        let (app, _f) = app_on_file("a.txt", "");
        let text = app.keys_sheet_text();
        assert!(text.contains("| `C-s` | `file.save` |"), "{text}");
        assert!(text.contains("`SPC g` | git"), "{text}");
    }
}
#[cfg(test)]
mod switcher_tests {
    use super::super::test_keys::app_on_file;

    #[test]
    fn ctrl_tab_picks_the_buffer_before_and_letting_go_of_ctrl_goes_there() {
        let (mut app, _f) = app_on_file("a.txt", "a\n");
        let a = app.focused_buffer_id();
        app.run_cmd("buffer.scratch");
        let scratch = app.focused_buffer_id();
        assert_ne!(a, scratch);
        app.run_cmd("buffer.switch_recent");
        app.recent_switch_released();
        assert_eq!(app.focused_buffer_id(), a, "the one before");
        app.run_cmd("buffer.switch_recent");
        app.recent_switch_released();
        assert_eq!(app.focused_buffer_id(), scratch, "and back");
    }
}
