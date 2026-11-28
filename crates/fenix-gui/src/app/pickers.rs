//! The pickers: opening one, its keys, and what Enter on a row does.
//!
//! Methods of `App` moved here from `app.rs` as they were.

use super::*;

impl App {
    /// `l` at the `quit_confirm` prompt -- the same `SPC b b` picker
    /// machinery as `picker_switch_buffer`, restricted to just the
    /// buffers blocking `:qa`/`SPC q q`.
    pub(super) fn picker_switch_dirty_buffers(&mut self) {
        let ids = self.dirty_tracked_buffer_ids();
        let candidates = self.buffer_candidates(&ids);
        self.enter_picker(ActivePicker::SwitchBuffer(fenix_picker::PickerState::new(candidates)));
    }

    /// `SPC p f`: a fuzzy file picker scoped to the current project (or
    /// the process's cwd, if no project was detected -- still useful,
    /// just not project-scoped).
    pub(crate) fn picker_find_file(&mut self) {
        let root = self.find_file_root();
        self.open_find_file(&root, false);
    }

    /// `SPC f a`: `picker_find_file`'s gitignore-blind sibling. Reuses
    /// `ActivePicker::FindFile` as-is -- `picker_confirm`'s own arm only
    /// cares that the payload is a path, not how the candidate list was
    /// built.
    pub(crate) fn picker_find_file_all(&mut self) {
        let root = self.find_file_root();
        self.open_find_file(&root, true);
    }

    /// `SPC f r`: a fuzzy picker over `recent_files` (already loaded,
    /// already backing the dashboard's own recent-file rows -- just
    /// never exposed as a standalone picker before). Labeled with the
    /// full path, not root-relative -- recent files can span multiple
    /// projects, so "relative to the current project" wouldn't always
    /// make sense. Reuses `ActivePicker::FindFile`, same reasoning as
    /// `picker_find_file_all`.
    pub(crate) fn picker_recent_files(&mut self) {
        let candidates = self
            .recent_files
            .paths()
            .iter()
            .map(|path| fenix_picker::Candidate::new(path.display().to_string(), path.clone()))
            .collect();
        self.enter_picker(ActivePicker::FindFile(fenix_picker::PickerState::new(candidates)));
    }

    /// `SPC p p`: a fuzzy picker over the persisted, MRU-ordered known-
    /// projects list.
    pub(crate) fn picker_switch_project(&mut self) {
        let candidates = self.known_project_candidates();
        self.enter_picker(ActivePicker::SwitchProject(fenix_picker::PickerState::new(candidates)));
    }

    /// `SPC p d`: same list as `SPC p p`, but confirming a selection
    /// removes it from `known_projects` instead of switching to it (see
    /// `ActivePicker::DeleteProject`/`picker_confirm`).
    pub(crate) fn picker_delete_project(&mut self) {
        let candidates = self.known_project_candidates();
        self.enter_picker(ActivePicker::DeleteProject(fenix_picker::PickerState::new(candidates)));
    }

    /// `SPC p a`: opens the full-buffer file explorer, in "pick a
    /// directory" mode, to browse to and register a project in
    /// `known_projects` -- the *only* way a project gets into the switch-
    /// project list now (see `refresh_project_root`'s own doc comment for
    /// why auto-registration-on-visit was dropped: it made "switch
    /// project" list every project you'd ever opened a file in, not a
    /// deliberately curated set worth switching between). Starts browsing
    /// at the current project root (or the process's cwd, if none
    /// detected); navigate with the explorer's usual `j`/`k`/`l`/`h`/
    /// `Enter`, then `S` to register whichever directory is currently
    /// being browsed. `q`/`Escape` cancels without registering anything,
    /// same as leaving the explorer any other time.
    pub(crate) fn picker_add_project_prompt(&mut self) {
        let start = self.project_root.clone().unwrap_or_else(|| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        self.open_folder_picker(&start, ExplorerPurpose::PickProjectDir);
    }

    /// `SPC b b`: a fuzzy picker over every open buffer, MRU-ordered
    /// (most-recently-touched first, matching Doom's own buffer switcher).
    /// The palette draws each with its icon, its name then its folder,
    /// and whether it has unsaved changes.
    pub(crate) fn picker_switch_buffer(&mut self) {
        let ids = self.buffers.mru().to_vec();
        let candidates = self.buffer_candidates(&ids);
        self.enter_picker(ActivePicker::SwitchBuffer(fenix_picker::PickerState::new(candidates)));
    }

    /// `SPC s l`: a fuzzy picker over every line in the focused buffer --
    /// candidates are the line's own text (trimmed of surrounding
    /// whitespace, right-aligned line number prefix for context), payload
    /// its start char offset. Live-filtered "for free" by `PickerState`'s
    /// own already-built fuzzy matching, the same mechanism `FindFile`/
    /// `Symbol` already lean on -- no new filtering logic needed here.
    pub(crate) fn picker_search_buffer(&mut self) {
        let buffer = &self.open().buffer;
        let visual_lines = buffer.visual_line_count();
        let candidates = (0..visual_lines)
            .map(|line| {
                let start = buffer.line_start_char(line);
                let len = buffer.line_len(line);
                let text = buffer.text_range(start, start + len);
                let label = format!("{:>4}  {}", line + 1, text.trim());
                fenix_picker::Candidate::new(label, start)
            })
            .collect();
        self.enter_picker(ActivePicker::BufferSearch(fenix_picker::PickerState::new(candidates)));
    }

    /// `SPC c o`: a fuzzy picker over every ATX heading in the focused
    /// Markdown buffer -- same "one candidate per matching line, payload
    /// its start char offset" shape `picker_search_buffer` already
    /// established for `SPC s l`, filtered to headings and labeled by
    /// nesting depth (`markdown::heading_label`) instead of a line
    /// number.
    ///
    /// Gated to a detected Markdown buffer, unlike list continuation/
    /// checkbox toggling: `#` at the start of a line means "comment,"
    /// not "heading," in half the languages this editor highlights
    /// (Python, Bash, YAML, Dockerfile, ...), so scanning for it without
    /// actually knowing the buffer is Markdown would list every comment
    /// in one of those as a heading.
    ///
    /// An empty result (no headings at all) still opens the picker,
    /// same as `SPC s l` -- "nothing to show" is a legitimate answer
    /// here, not an error worth a special-cased message.
    pub(crate) fn picker_outline(&mut self) {
        if self.focused_language() == Some(fenix_syntax::LanguageId::Xml) {
            self.picker_xml_outline();
            return;
        }
        if self.focused_language() != Some(fenix_syntax::LanguageId::Markdown) {
            self.set_error("SPC c o needs a Markdown or XML buffer");
            return;
        }
        let buffer = &self.open().buffer;
        let visual_lines = buffer.visual_line_count();
        let candidates = (0..visual_lines)
            .filter_map(|line| {
                let start = buffer.line_start_char(line);
                let len = buffer.line_len(line);
                let text = buffer.text_range(start, start + len);
                let (level, title) = markdown::parse_heading(&text)?;
                Some(fenix_picker::Candidate::new(markdown::heading_label(level, &title), start))
            })
            .collect();
        self.enter_picker(ActivePicker::Outline(fenix_picker::PickerState::new(candidates)));
    }

    /// `SPC t t` -- fuzzy-picks a discovered task
    /// (`fenix_tasks::discover_tasks`) to run. A no-op (with a message)
    /// with no known project root, or with a project root that has no
    /// discoverable tasks at all (no recognized marker file, and no
    /// `.fenix/project.ini` `[tasks]` overrides) -- there's genuinely
    /// nothing to offer the picker in either case.
    pub(crate) fn picker_tasks(&mut self) {
        let Some(root) = self.project_root.clone() else {
            self.set_error("no project root detected for the focused buffer".to_string());
            return;
        };
        // A file of a project nested in this one (a crate, a package):
        // its own tasks first -- where test-at-cursor and code lenses run
        // -- then the whole project's.
        let member = self.open().buffer.path().filter(|f| path_within(f, &root)).map(tool_sessions::root_for_path).filter(|m| place_key(m) != place_key(&root));
        let mut candidates = Vec::new();
        let mut first_error = None;
        for at in member.iter().cloned().chain([root.clone()]) {
            let tasks = match self.tasks_for(&at) {
                Ok(tasks) => tasks,
                Err(error) => {
                    first_error.get_or_insert(error);
                    continue;
                }
            };
            let own = self.project_tools(&at).map(|tools| tools.tasks).unwrap_or_default();
            let prefix = if member.is_some() { format!("{}: ", at.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()) } else { String::new() };
            for t in tasks {
                let label = format!("{prefix}{}", task_picker_label(&t, &self.task_source(&at, &t, own.contains_key(&t.name))));
                candidates.push(fenix_picker::Candidate::new(label, (t, at.clone())));
            }
        }
        if candidates.is_empty() {
            match first_error {
                Some(error) => self.set_error(error),
                None => self.set_error(format!("no known tasks for {}", root.display())),
            }
            return;
        }
        self.enter_picker(ActivePicker::Task(fenix_picker::PickerState::new(candidates)));
    }

    /// `SPC f v`: what a previous session left unsaved.
    pub(crate) fn picker_recovery(&mut self) {
        let Some(dir) = self.recovery_dir.clone() else {
            self.set_error("no config directory to keep recovery snapshots in".to_string());
            return;
        };
        let snapshots = fenix_recovery::list(&dir);
        if snapshots.is_empty() {
            self.set_message("nothing to recover -- no unsaved work was left behind");
            return;
        }
        let candidates = snapshots
            .into_iter()
            .map(|snapshot| {
                let label = format!("{}   ({})", snapshot.label(), describe_age(snapshot.age()));
                fenix_picker::Candidate::new(label, snapshot)
            })
            .collect();
        self.enter_picker(ActivePicker::Recovery(fenix_picker::PickerState::new(candidates)));
    }

    /// `SPC g r`: pick a ref to replay the current branch onto.
    pub(crate) fn start_rebase_picker(&mut self) {
        let repo_root = self.git_action_repo_root();
        let refs = self.compare_ref_candidates(&repo_root);
        if refs.len() <= 1 {
            self.set_error("no refs to rebase onto -- is this a git repository?".to_string());
            return;
        }
        let candidates = refs.into_iter().skip(1).map(|r| fenix_picker::Candidate::new(r.clone(), r)).collect();
        self.enter_picker(ActivePicker::RebaseOnto(fenix_picker::PickerState::new(candidates)));
    }

    /// `SPC g m`: pick a ref to merge into the current branch.
    pub(crate) fn start_merge_picker(&mut self) {
        let repo_root = self.git_action_repo_root();
        let refs = self.compare_ref_candidates(&repo_root);
        if refs.len() <= 1 {
            self.set_error("no refs to merge -- is this a git repository?".to_string());
            return;
        }
        let candidates = refs.into_iter().skip(1).map(|r| fenix_picker::Candidate::new(r.clone(), r)).collect();
        self.enter_picker(ActivePicker::MergeFrom(fenix_picker::PickerState::new(candidates)));
    }

    /// No stashing needed -- same reasoning as `explorer_jump`, now that
    /// buffers live in `self.buffers` rather than direct `App` fields.
    pub(super) fn enter_picker(&mut self, picker: ActivePicker) {
        // Whatever opens it says what it is: the thread list sets its own
        // mark after.
        self.agents.list.open = false;
        self.active_picker = Some(picker);
        self.picker_scroll = 0;
        self.main_view = MainView::Picker;
        self.wake_caret();
    }

    /// `Escape`: cancels the active picker -- no file/project opened, the
    /// focused pane's buffer was never touched, so returning to the
    /// editor just means switching `main_view` back.
    pub(super) fn picker_cancel(&mut self) {
        if let Some(ActivePicker::Code { ext, id, many, .. }) = &self.active_picker {
            let (ext, id, many) = (ext.clone(), *id, *many);
            self.active_picker = None;
            self.main_view = MainView::Editor;
            if many {
                return self.code_picked_many(&ext, id, None);
            }
            return self.code_picked(&ext, id, None);
        }
        self.active_picker = None;
        self.main_view = MainView::Editor;
    }

    /// `Enter`: confirms the selected candidate, dispatching differently
    /// per picker kind. A no-op (stays open) if the filtered list is
    /// currently empty -- nothing to confirm.
    pub(super) fn picker_confirm(&mut self) {
        self.picker_confirm_inner();
        // A picker that closed leaves its page: what's drawn is where the
        // keys go. A command it ran that opens a view of its own (another
        // picker, a page) sets that view when it runs.
        if self.active_picker.is_none() && self.main_view == MainView::Picker {
            self.main_view = MainView::Editor;
        }
    }

    pub(super) fn picker_confirm_inner(&mut self) {
        match &self.active_picker {
            Some(ActivePicker::Commands(state)) => {
                let Some(id) = state.selected().map(|c| c.payload.clone()) else { return };
                self.active_picker = None;
                if std::mem::take(&mut self.palette_describes) {
                    let text = self.describe_command(&id);
                    self.set_message(text);
                    return;
                }
                // Run by `handle_key`, which has the event loop a command
                // may need.
                self.queued_command = Some(id);
            }
            Some(ActivePicker::Bindings(state)) => {
                let Some(id) = state.selected().map(|c| c.payload.clone()) else { return };
                self.active_picker = None;
                self.queued_command = Some(id);
            }
            Some(ActivePicker::FindFile(state)) => {
                let Some(path) = state.selected().map(|c| c.payload.clone()) else { return };
                self.active_picker = None;
                self.open_file_from_picker(&path);
            }
            Some(ActivePicker::SwitchProject(state)) => {
                let Some(root) = state.selected().map(|c| c.payload.clone()) else { return };
                self.active_picker = None;
                self.open_project(root, None, true);
            }
            Some(ActivePicker::ProjectTodos(state) | ActivePicker::Problems(state) | ActivePicker::ProjectSymbols(state)) => {
                let Some(m) = state.selected().map(|c| c.payload.clone()) else { return };
                self.active_picker = None;
                let from = JumpEntry { buffer: self.focused_buffer_id(), char_idx: self.cursor().char_idx };
                self.open_file_as_preview(&m.path);
                self.jump_to_grep_match(&m);
                self.record_jump(from);
                // So `SPC p n`/`SPC p N` continue from wherever the
                // picker actually landed (which candidate got confirmed
                // may not be `quickfix[0]` -- the picker's own fuzzy
                // filter can reorder/narrow what's shown), not always
                // restart from the top of the list.
                self.quickfix_index = self.quickfix.iter().position(|entry| matches!(entry, QuickfixEntry::Grep(gm) if gm == &m));
            }
            Some(ActivePicker::SwitchBuffer(state)) => {
                let Some(id) = state.selected().map(|c| c.payload) else { return };
                self.active_picker = None;
                self.switch_focused_to_buffer(id);
            }
            Some(ActivePicker::Recovery(state)) => {
                let Some(snapshot) = state.selected().map(|c| c.payload.clone()) else { return };
                self.active_picker = None;
                self.recover_snapshot(&snapshot);
            }
            Some(ActivePicker::Agents(state)) => {
                let Some(pick) = state.selected().map(|c| c.payload.clone()) else { return };
                self.active_picker = None;
                self.main_view = MainView::Editor;
                self.agents_picked(pick);
            }
            Some(ActivePicker::Places(state)) => {
                let Some(path) = state.selected().map(|c| c.payload.clone()) else { return };
                self.active_picker = None;
                self.main_view = MainView::Editor;
                self.explorer_open_path(&path);
            }
            Some(ActivePicker::RemoteHost(state)) => {
                // A name that matches none of the listed is a machine
                // too: `user@host`, as typed.
                let typed = Some(state.query().trim().to_string()).filter(|typed| !typed.is_empty());
                let Some(host) = state.selected().map(|c| c.payload.clone()).or(typed) else { return };
                self.active_picker = None;
                self.main_view = MainView::Editor;
                self.remote_host_picked(&host);
            }
            Some(ActivePicker::DeleteProject(state)) => {
                let Some(root) = state.selected().map(|c| c.payload.clone()) else { return };
                self.active_picker = None;
                self.main_view = MainView::Editor;
                self.known_projects.remove(&root);
                if let Err(err) = self.known_projects.save() {
                    eprintln!("fenix: couldn't save project history: {err}");
                }
            }
            Some(ActivePicker::Snippet(state)) => {
                let Some(choice) = state.selected().map(|c| c.payload.clone()) else { return };
                self.active_picker = None;
                self.main_view = MainView::Editor;
                self.insert_picked_snippet(choice);
            }
            Some(ActivePicker::Encoding(state)) => {
                let Some((encoding, reopen)) = state.selected().map(|c| c.payload) else { return };
                self.active_picker = None;
                self.main_view = MainView::Editor;
                if reopen {
                    self.reopen_with_encoding(encoding);
                } else {
                    self.save_with_encoding(encoding);
                }
            }
            Some(ActivePicker::Theme(state)) => {
                let Some(theme) = state.selected().map(|c| c.payload) else { return };
                self.active_picker = None;
                self.main_view = MainView::Editor;
                self.apply_theme(theme);
            }
            Some(ActivePicker::Symbol(state)) => {
                let Some(tag) = state.selected().map(|c| c.payload.clone()) else { return };
                self.active_picker = None;
                let from = JumpEntry { buffer: self.focused_buffer_id(), char_idx: self.cursor().char_idx };
                self.open_file_as_preview(&tag.file);
                self.jump_to_tag(&tag);
                self.record_jump(from);
            }
            Some(ActivePicker::Launch(state)) => {
                let Some(template) = state.selected().map(|c| c.payload.clone()) else { return };
                self.active_picker = None;
                self.main_view = MainView::Editor;
                self.debug_start_from(template);
            }
            Some(ActivePicker::Task(state)) => {
                let Some((task, root)) = state.selected().map(|c| c.payload.clone()) else { return };
                self.active_picker = None;
                self.main_view = MainView::Editor;
                self.run_task(task, root);
            }

            Some(ActivePicker::SettingChoice { key, picker }) => {
                let Some(value) = picker.selected().map(|c| c.payload.clone()) else { return };
                let key = *key;
                self.active_picker = None;
                self.main_view = MainView::Editor;
                self.setting_chosen(key, &value);
            }
            Some(ActivePicker::CompareTarget(state)) => {
                let Some(target) = state.selected().map(|c| c.payload.clone()) else { return };
                self.active_picker = None;
                self.main_view = MainView::Editor;
                self.open_git_compare(target, None);
            }
            Some(ActivePicker::Versus { path, picker }) => {
                let Some(rev) = picker.selected().map(|c| c.payload.clone()) else { return };
                let path = path.clone();
                self.active_picker = None;
                self.main_view = MainView::Editor;
                self.open_side_by_side(&path, &rev);
            }
            Some(ActivePicker::CompareOwn { target, picker }) => {
                let Some(own) = picker.selected().map(|c| c.payload.clone()) else { return };
                let target = target.clone();
                self.active_picker = None;
                self.main_view = MainView::Editor;
                self.open_git_compare(target, Some(own));
            }
            Some(ActivePicker::RebaseOnto(state)) => {
                let Some(onto) = state.selected().map(|c| c.payload.clone()) else { return };
                self.active_picker = None;
                self.main_view = MainView::Editor;
                self.git_rebase_onto(&onto);
            }
            Some(ActivePicker::MergeFrom(state)) => {
                let Some(branch) = state.selected().map(|c| c.payload.clone()) else { return };
                self.active_picker = None;
                self.main_view = MainView::Editor;
                self.git_merge_from(&branch);
            }
            Some(ActivePicker::SwitchBranch(state)) => {
                let Some(branch) = state.selected().map(|c| c.payload.clone()) else { return };
                self.active_picker = None;
                self.main_view = MainView::Editor;
                self.git_switch_to(&branch);
            }

            Some(ActivePicker::TableColumn(state)) => {
                let Some(column) = state.selected().map(|c| c.payload) else { return };
                self.active_picker = None;
                self.main_view = MainView::Editor;
                let line = self.open().buffer.line_col(&self.cursor()).0;
                let idx = self.table_column_start(line, column);
                let pane = self.focused_pane_id();
                let cursor = &mut self.pane_state_mut(pane).cursor;
                cursor.char_idx = idx;
                cursor.sticky_col = 0;
            }
            Some(ActivePicker::BufferSearch(state) | ActivePicker::BufferTodos(state)) => {
                let Some(offset) = state.selected().map(|c| c.payload) else { return };
                self.active_picker = None;
                self.main_view = MainView::Editor;
                let (buffer, cursor) = self.focused_buffer_and_cursor_mut();
                cursor.char_idx = offset.min(buffer.len_chars());
                let (_, col) = buffer.line_col(cursor);
                cursor.sticky_col = col;
            }
            Some(ActivePicker::Lenses(state)) => {
                let Some(pick) = state.selected().map(|c| c.payload.clone()) else { return };
                self.active_picker = None;
                self.main_view = MainView::Editor;
                self.run_lens(&pick);
            }
            Some(ActivePicker::SwitchWorkspace(state)) => {
                let Some(idx) = state.selected().map(|c| c.payload) else { return };
                self.active_picker = None;
                self.main_view = MainView::Editor;
                self.workspaces.switch_to_index(idx);
            }
            Some(ActivePicker::WorkspaceLauncher(state)) => {
                let Some(name) = state.selected().map(|c| c.payload.clone()) else { return };
                self.active_picker = None;
                self.main_view = MainView::Editor;
                self.open_configured_workspace(&name);
            }
            Some(ActivePicker::Outline(state)) => {
                let Some(offset) = state.selected().map(|c| c.payload) else { return };
                self.active_picker = None;
                self.main_view = MainView::Editor;
                let (buffer, cursor) = self.focused_buffer_and_cursor_mut();
                cursor.char_idx = offset.min(buffer.len_chars());
                let (_, col) = buffer.line_col(cursor);
                cursor.sticky_col = col;
            }
            Some(ActivePicker::Code { ext, id, many, state, .. }) => {
                let Some(choice) = state.selected().map(|c| c.payload.clone()) else { return };
                let marked: Vec<String> = state.marked().cloned().collect();
                let (ext, id, many) = (ext.clone(), *id, *many);
                self.active_picker = None;
                self.main_view = MainView::Editor;
                if many {
                    // What's marked, or else the row Enter was on.
                    self.code_picked_many(&ext, id, Some(if marked.is_empty() { vec![choice] } else { marked }));
                } else {
                    self.code_picked(&ext, id, Some(choice));
                }
            }
            None => {}
        }
        self.wake_caret();
    }

    /// Points the focused pane at `path` (opening it in the registry --
    /// reusing an already-open buffer for the same path, per
    /// `BufferList::open_path`), exactly like opening a file from the
    /// explorer. A picker always fully took over `main_view` to get
    /// here, so confirming always means "this is current now, back to
    /// the editor."
    pub(super) fn open_file_from_picker(&mut self, path: &Path) {
        self.open_file_as(path, false);
        // Opened on purpose: a preview of this file is now a tab you keep.
        let pane = self.focused_pane_id();
        if self.workspaces.active_workspace().preview.get(&pane) == Some(&self.focused_buffer_id()) {
            self.keep_preview(pane);
        }
    }

    /// Routes one keypress to the active picker: plain characters edit
    /// the query and re-filter, Up/Down or Ctrl-N/Ctrl-P move the
    /// selection, Enter confirms, Escape cancels -- everything else is
    /// ignored (the picker is a text-input mode, not an action trie like
    /// the explorer's, so unrecognized keys just don't do anything
    /// rather than falling through to something else).
    pub(super) fn picker_key(&mut self, key: KeyPress) {
        // The thread list's own keys (`Tab`, `Alt+A`...), ahead of the query.
        if self.thread_list_key(key) {
            self.wake_caret();
            return;
        }
        // Tab in the palette: give the command under the cursor keys.
        if key == KeyPress::named(FenixNamedKey::Tab) && !self.palette_describes {
            if let Some(ActivePicker::Commands(state)) = &self.active_picker {
                if let Some(id) = state.selected().map(|c| c.payload.clone()) {
                    self.active_picker = None;
                    self.bind_keys_start(id);
                }
                return;
            }
        }
        // C-y in the palette: the command's id, to bind it or run it from
        // outside.
        if key == KeyPress::char('y').with_ctrl() {
            if let Some(ActivePicker::Commands(state)) = &self.active_picker {
                if let Some(id) = state.selected().map(|c| c.payload.clone()) {
                    if let Some(clipboard) = &mut self.clipboard {
                        let _ = clipboard.set_text(id.clone());
                    }
                    self.set_message(format!("copied {id}"));
                }
                return;
            }
        }
        if key == KeyPress::char('v').with_ctrl() {
            let pasted = self.clipboard_text();
            if let (Some(picker), Some(text)) = (&mut self.active_picker, pasted) {
                // One `push_char` per pasted char, same as fast typing --
                // each fuzzy-picker's own `push_char` re-filters as it
                // goes, so this keeps the candidate list correct at every
                // step, not just at the end.
                for c in text.chars() {
                    picker_push_char(picker, c);
                }
            }
            self.wake_caret();
            return;
        }
        let Some(picker) = &mut self.active_picker else {
            // No picker left to take the key: its page goes, rather than
            // staying up blank while keys reach nothing.
            if self.main_view == MainView::Picker {
                self.main_view = MainView::Editor;
                self.wake_caret();
            }
            return;
        };
        match key.code {
            KeyCode::Named(FenixNamedKey::Escape) => self.picker_cancel(),
            KeyCode::Named(FenixNamedKey::Enter) => self.picker_confirm(),
            KeyCode::Named(FenixNamedKey::Backspace) => picker_backspace(picker),
            // As in a shell: `Ctrl-W` the word before, `Ctrl-U` all of it.
            KeyCode::Char(c @ ('w' | 'u')) if key.mods.ctrl => {
                let query: Vec<char> = picker_query(picker).chars().collect();
                let mut keep = query.len();
                if c == 'u' {
                    keep = 0;
                } else {
                    while keep > 0 && query[keep - 1].is_whitespace() {
                        keep -= 1;
                    }
                    let word = |c: char| c.is_alphanumeric() || c == '_';
                    let in_word = keep > 0 && word(query[keep - 1]);
                    while keep > 0 && !query[keep - 1].is_whitespace() && word(query[keep - 1]) == in_word {
                        keep -= 1;
                    }
                }
                for _ in keep..query.len() {
                    picker_backspace(picker);
                }
            }
            KeyCode::Named(FenixNamedKey::Down) => picker_move_selection(picker, 1),
            KeyCode::Named(FenixNamedKey::Up) => picker_move_selection(picker, -1),
            KeyCode::Char('n') if key.mods.ctrl => picker_move_selection(picker, 1),
            KeyCode::Char('p') if key.mods.ctrl => picker_move_selection(picker, -1),
            // Toggles-and-does-not-close, unlike `Enter` -- a no-op for
            // every single-select picker (nothing ever reads their
            // marks). Chosen over `Space` since status names routinely
            // contain spaces (`"In Progress"`), which would make it
            // impossible to type a query that filters a long status list.
            KeyCode::Named(FenixNamedKey::Tab) => picker_toggle_mark(picker),
            KeyCode::Char(c) if !key.mods.ctrl => picker_push_char(picker, c),
            _ => {}
        }
        self.wake_caret();
    }

    /// `SPC TAB TAB`: a fuzzy picker over every currently open
    /// workspace, by name -- picking one directly rather than the
    /// relative `]`/`[` cycling `next_workspace`/`prev_workspace`
    /// already cover. The active workspace is marked with a leading
    /// `*` so re-opening the picker shows at a glance which one you're
    /// already on.
    pub(crate) fn picker_switch_workspace(&mut self) {
        let active = self.workspaces.active_index();
        let candidates = self
            .workspaces
            .summaries()
            .into_iter()
            .map(|(idx, name)| {
                let marker = if idx == active { "* " } else { "  " };
                fenix_picker::Candidate::new(format!("{marker}{name}"), idx)
            })
            .collect();
        self.enter_picker(ActivePicker::SwitchWorkspace(fenix_picker::PickerState::new(candidates)));
    }

    /// `SPC TAB f`: a fuzzy picker over `config.ini`'s `[workspaces]`
    /// launcher list -- same "hand-configured shelf, one candidate
    /// list" shape `start_document_picker` already
    /// established.
    ///
    /// An empty index says so rather than opening a picker over
    /// nothing, same reasoning as `start_document_picker`'s own check:
    /// this list can only be populated by hand-editing `config.ini`, so
    /// "no matches" would otherwise be indistinguishable from "you
    /// haven't set this up yet".
    pub(crate) fn start_workspace_launcher_picker(&mut self) {
        if self.config.workspaces.is_empty() {
            self.set_error(
                "no workspaces on the shelf yet -- add them in SPC , (Workspaces)".to_string(),
            );
            return;
        }
        let candidates =
            self.config.workspaces.iter().map(|(name, _)| fenix_picker::Candidate::new(name.clone(), name.clone())).collect();
        self.enter_picker(ActivePicker::WorkspaceLauncher(fenix_picker::PickerState::new(candidates)));
    }

    /// `SPC t p`: a fuzzy picker over every shipped theme (`theme::ALL`),
    /// for jumping straight to one by name rather than cycling through
    /// them one at a time.
    pub(crate) fn picker_pick_theme(&mut self) {
        let candidates =
            theme::all().iter().map(|t| fenix_picker::Candidate::new(t.name.to_string(), *t)).collect();
        self.enter_picker(ActivePicker::Theme(fenix_picker::PickerState::new(candidates)));
    }

    /// The encodings, to read the focused file with (`reopen`) or save it in.
    pub(crate) fn picker_pick_encoding(&mut self, reopen: bool) {
        if self.open().buffer.path().is_none() && reopen {
            return self.set_error("this buffer has no file to read".to_string());
        }
        let candidates = fenix_core::TextEncoding::choices().into_iter().map(|e| fenix_picker::Candidate::new(e.label().to_string(), (e, reopen))).collect();
        self.enter_picker(ActivePicker::Encoding(fenix_picker::PickerState::new(candidates)));
    }

    /// `c` on a `BufferKind::Table` buffer: a fuzzy picker over its
    /// synthesized column names -- confirming (`picker_confirm`'s own
    /// `TableColumn` arm) jumps to that column's start on the current
    /// row. A no-op if the buffer has no `TableView` yet (shouldn't
    /// happen in practice: `sync_table_view` runs every redraw a `Table`
    /// buffer is visible, but this guards the interception key itself
    /// firing before the first redraw ever ran).
    pub(super) fn open_table_column_picker(&mut self) {
        let id = self.focused_buffer_id();
        let Some(view) = self.table_views.get(&id) else { return };
        let candidates =
            view.columns.iter().enumerate().map(|(i, name)| fenix_picker::Candidate::new(name.clone(), i)).collect();
        self.enter_picker(ActivePicker::TableColumn(fenix_picker::PickerState::new(candidates)));
    }
}
