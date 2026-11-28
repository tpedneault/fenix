//! The Git panel, the history, merge and compare views, and the Git actions they share.
//!
//! Methods of `App` moved here from `app.rs` as they were.

use super::*;

impl App {
    /// `SPC g g`: opens (or, if one's already open, refocuses/refreshes)
    /// the Lazygit-style multi-pane session -- Status/Files/Branches/
    /// Commits/Stash stacked on the left, Main (diff view) on the right,
    /// each with its own title bar. Mirrors `DockerHost::open` closely
    /// (session-reuse-on-reopen, its own workspace, a background
    /// poller) -- see `GitSession`'s own doc comment for the one real
    /// structural difference (every `fenix-git` call targets a specific
    /// repo root, not a daemon reachable regardless of cwd).
    pub(crate) fn open_git_panel(&mut self) {
        let wanted = self.git_repo_wanted();
        if let Some(session) = &mut self.git_session {
            // Asked for from another project: the panel shows its
            // repository from now on.
            if let Some(wanted) = wanted.filter(|w| session.workspace_index != self.workspaces.active && !same_repository(w, &session.repo_root)) {
                session.repo_root = wanted;
                session.files.clear();
            }
            let (workspace_index, staged_pane) = (session.workspace_index, session.staged_pane);
            self.workspaces.switch_to_index(workspace_index);
            self.windows_mut().focus(staged_pane);
            self.git_refresh_session();
            self.wake_caret();
            return;
        }

        let repo_root = self.project_root.clone().unwrap_or_else(|| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

        let (status, files) = fenix_git::status_and_files(&repo_root);
        let in_progress = fenix_git::in_progress(&repo_root);
        let sides = fenix_git::conflict_sides(&repo_root);
        let branches = fenix_git::list_branches(&repo_root);
        let commits = fenix_git::list_commits(&repo_root, 50);
        let stashes = fenix_git::list_stashes(&repo_root);

        let (staged, unstaged, untracked) = file_counts(&files);
        let status_panel =
            git_panel::render_status(status.as_ref(), staged, unstaged, untracked, in_progress.as_ref(), conflict_count(&files), sides.as_ref());
        let staged_panel = git_panel::render_staged(&files, &HashSet::new());
        let unstaged_panel = git_panel::render_unstaged(&files, &HashSet::new());
        let branches_panel = git_panel::render_branches(&branches);
        let commits_panel = git_panel::render_commits(&commits);
        let stash_panel = git_panel::render_stash(&stashes);

        let status_buffer = self.buffers.open_git(&status_panel.text);
        let staged_buffer = self.buffers.open_git(&staged_panel.text);
        let unstaged_buffer = self.buffers.open_git(&unstaged_panel.text);
        let branches_buffer = self.buffers.open_git(&branches_panel.text);
        let commits_buffer = self.buffers.open_git(&commits_panel.text);
        let stash_buffer = self.buffers.open_git(&stash_panel.text);
        // The Main pane is a real `BufferKind::Diff` buffer, not another
        // Git one: it renders through `diff_view` (hunk-aware, with a
        // line-number gutter and per-row anchors) rather than as
        // prefix-colored text, which is what makes hunk staging and
        // "open the file at this line" possible at all.
        let main_view = diff_view::render(&[], &HashSet::new());
        let main_buffer = self.buffers.open_diff(&main_view.text);
        self.git_lines.insert(status_buffer, status_panel.lines);
        self.git_lines.insert(staged_buffer, staged_panel.lines);
        self.git_lines.insert(unstaged_buffer, unstaged_panel.lines);
        self.git_lines.insert(branches_buffer, branches_panel.lines);
        self.git_lines.insert(commits_buffer, commits_panel.lines);
        self.git_lines.insert(stash_buffer, stash_panel.lines);
        self.diff_lines.insert(main_buffer, main_view.lines);

        let cursor = Cursor::at_start();
        self.workspaces.new_workspace(status_buffer, cursor);
        let workspace_index = self.workspaces.active_index();
        let status_pane = self.focused_pane_id();

        // Right column first, at the outer split -- same ~35/65 left/
        // right proportions `DockerHost::open` uses for its own Status/
        // Logs vs. Containers/Images/Volumes split, and for the same
        // reason: list rows don't need much width, a diff view does.
        let main_pane = self.windows_mut().split(SplitKind::Vertical, main_buffer);
        self.workspaces.active_pane_states_mut().insert(main_pane, PaneState::seeded_at(cursor));
        self.windows_mut().resize_focused(-0.15);

        // Left column: Status/Staged/Unstaged/Branches/Commits/Stash
        // stacked. Status is shrunk well below the default 50/50 split
        // (it's a handful of fixed overview lines, not a list);
        // everything after that cascades naturally the same way
        // Containers/Images/Volumes already do for the Docker panel --
        // Staged (split off Status) keeps the biggest remaining share
        // "for free," and each further split below it takes half of
        // whatever's left, without needing its own explicit resize.
        self.windows_mut().focus(status_pane);
        let staged_pane = self.windows_mut().split(SplitKind::Horizontal, staged_buffer);
        self.workspaces.active_pane_states_mut().insert(staged_pane, PaneState::seeded_at(cursor));
        self.windows_mut().resize_focused(-0.35);
        let unstaged_pane = self.windows_mut().split(SplitKind::Horizontal, unstaged_buffer);
        self.workspaces.active_pane_states_mut().insert(unstaged_pane, PaneState::seeded_at(cursor));
        let branches_pane = self.windows_mut().split(SplitKind::Horizontal, branches_buffer);
        self.workspaces.active_pane_states_mut().insert(branches_pane, PaneState::seeded_at(cursor));
        let commits_pane = self.windows_mut().split(SplitKind::Horizontal, commits_buffer);
        self.workspaces.active_pane_states_mut().insert(commits_pane, PaneState::seeded_at(cursor));
        let stash_pane = self.windows_mut().split(SplitKind::Horizontal, stash_buffer);
        self.workspaces.active_pane_states_mut().insert(stash_pane, PaneState::seeded_at(cursor));
        self.windows_mut().focus(staged_pane);

        self.pane_titles.insert(status_pane, "1. Status".to_string());
        self.pane_titles.insert(staged_pane, "2. Staged".to_string());
        self.pane_titles.insert(unstaged_pane, "3. Unstaged".to_string());
        self.pane_titles.insert(branches_pane, "4. Branches".to_string());
        self.pane_titles.insert(commits_pane, "5. Commits".to_string());
        self.pane_titles.insert(stash_pane, "6. Stash".to_string());
        self.pane_titles.insert(main_pane, "7. Main".to_string());

        // Same "only a real, main.rs-launched App spawns anything" posture
        // as `DockerHost::open`'s own stats poller.
        let status_poller = self
            .event_proxy
            .clone()
            .map(|proxy| GitStatusPoller::spawn(repo_root.clone(), move |event| proxy.send_event(event).is_ok()));

        self.git_session = Some(GitSession {
            workspace_index,
            status_pane,
            staged_pane,
            unstaged_pane,
            branches_pane,
            commits_pane,
            stash_pane,
            main_pane,
            status_buffer,
            staged_buffer,
            unstaged_buffer,
            branches_buffer,
            commits_buffer,
            stash_buffer,
            main_buffer,
            repo_root,
            accordion_focus: None,
            files,
            branches,
            commits,
            stashes,
            status,
            in_progress,
            sides,
            status_poller,
            staged_expanded_dirs: HashSet::new(),
            unstaged_expanded_dirs: HashSet::new(),
            last_main_entry: None,
            last_main_fetch: MainFetch::Skip,
            main_request_id: 0,
            refresh_request_id: 0,
        });

        self.refresh_project_root();
        self.wake_caret();
    }

    /// `Tab` on Staged or Unstaged: toggles whether the directory under
    /// the cursor is expanded *in that pane's own tree*, then re-renders
    /// just that pane keeping the cursor on the same row (see `set_git_
    /// buffer_preserving_line`). A no-op on a file row, or anywhere but
    /// those two panes -- only directories are expandable, and each
    /// pane's expand state is independent (`GitSession::staged_expanded_
    /// dirs`'s own doc comment explains why).
    pub(super) fn git_toggle_dir_expand(&mut self) {
        let Some(git_panel::GitEntry::Dir(path)) = self.git_entry_at_cursor() else { return };
        let Some(role) = self.git_focused_role() else { return };
        let Some(session) = self.git_session.as_mut() else { return };
        let (buffer, panel) = match role {
            GitPaneRole::Staged => {
                if !session.staged_expanded_dirs.remove(&path) {
                    session.staged_expanded_dirs.insert(path);
                }
                (session.staged_buffer, git_panel::render_staged(&session.files, &session.staged_expanded_dirs))
            }
            GitPaneRole::Unstaged => {
                if !session.unstaged_expanded_dirs.remove(&path) {
                    session.unstaged_expanded_dirs.insert(path);
                }
                (session.unstaged_buffer, git_panel::render_unstaged(&session.files, &session.unstaged_expanded_dirs))
            }
            _ => return,
        };
        self.set_git_buffer_preserving_line(buffer, panel);
    }

    /// `u` (on any pane), and after every mutating action (stage/unstage/
    /// commit/checkout/stash/...): re-lists files/branches/commits/
    /// stashes and the repo status. Same two-part async treatment as
    /// `git_sync_main` (its own doc comment explains the "why" in full):
    /// the four `git` calls this needs (`status_and_files`, `list_
    /// branches`, `list_commits`, `list_stashes`) used to run
    /// synchronously on the input thread, blocking the UI for every
    /// single one of those actions, not just `u`. Now they run on a
    /// background thread for a real app (`apply_git_refresh` applies the
    /// result when it lands, guarded by `refresh_request_id` against a
    /// slow request a faster later one already superseded), or inline
    /// when there's no `event_proxy` (every test) so existing tests
    /// asserting on pane content right after calling this keep working
    /// unchanged.
    pub(super) fn git_refresh_session(&mut self) {
        let Some(session) = self.git_session.as_ref() else { return };
        let repo_root = session.repo_root.clone();

        let Some(session) = self.git_session.as_mut() else { return };
        session.refresh_request_id += 1;
        let request_id = session.refresh_request_id;

        match self.event_proxy.clone() {
            Some(proxy) => {
                std::thread::spawn(move || {
                    let data = fetch_git_refresh_data(&repo_root);
                    let _ = proxy.send_event(FenixUserEvent::GitRefreshReady { request_id, data });
                });
            }
            None => {
                let data = fetch_git_refresh_data(&repo_root);
                self.apply_git_refresh(request_id, data);
            }
        }
    }

    /// Which of the current `git_session`'s seven panes (if any) is
    /// focused right now -- mirrors `DockerHost::focused_role`, including the
    /// same active-workspace guard (see its own doc comment for why
    /// that's needed: `WindowId`s are only unique within one workspace).
    pub(super) fn git_focused_role(&self) -> Option<GitPaneRole> {
        let session = self.git_session.as_ref()?;
        if self.workspaces.active_index() != session.workspace_index {
            return None;
        }
        let focused = self.focused_pane_id();
        if focused == session.status_pane {
            Some(GitPaneRole::Status)
        } else if focused == session.staged_pane {
            Some(GitPaneRole::Staged)
        } else if focused == session.unstaged_pane {
            Some(GitPaneRole::Unstaged)
        } else if focused == session.branches_pane {
            Some(GitPaneRole::Branches)
        } else if focused == session.commits_pane {
            Some(GitPaneRole::Commits)
        } else if focused == session.stash_pane {
            Some(GitPaneRole::Stash)
        } else if focused == session.main_pane {
            Some(GitPaneRole::Main)
        } else {
            None
        }
    }

    /// The pane whose title bar is numbered `n` (`1. Status` etc., see
    /// `open_git_panel`'s own title strings) -- mirrors `DockerHost::
    /// pane_by_number`, backs the same digit-key jump-to-pane shortcut.
    pub(super) fn git_pane_by_number(&self, n: u32) -> Option<fenix_window::WindowId> {
        let session = self.git_session.as_ref()?;
        match n {
            1 => Some(session.status_pane),
            2 => Some(session.staged_pane),
            3 => Some(session.unstaged_pane),
            4 => Some(session.branches_pane),
            5 => Some(session.commits_pane),
            6 => Some(session.stash_pane),
            7 => Some(session.main_pane),
            _ => None,
        }
    }

    /// What the cursor's current line on a Git buffer targets -- mirrors
    /// `DockerHost::entry_at_cursor`.
    pub(super) fn git_entry_at_cursor(&self) -> Option<git_panel::GitEntry> {
        let cursor = self.cursor();
        let line = self.open().buffer.line_col(&cursor).0;
        self.git_lines.get(&self.focused_buffer_id()).and_then(|lines| lines.get(line)).and_then(|meta| meta.as_ref()).and_then(|meta| meta.entry.clone())
    }

    /// The selected row's full `FileEntry` (not just its path), looked
    /// up in the session's own cached `files` -- needed by discard (to
    /// know whether it's untracked) and by `git_sync_main` (to know
    /// whether it's staged).
    pub(super) fn git_selected_file(&self) -> Option<fenix_git::FileEntry> {
        let git_panel::GitEntry::File(path) = self.git_entry_at_cursor()? else { return None };
        self.git_session.as_ref()?.files.iter().find(|f| f.path == path).cloned()
    }

    /// Re-renders Main from whichever row is now under the cursor in
    /// Staged/Unstaged/Commits/Stash -- a no-op for Branches/Status/Main
    /// itself (see `git_panel::render_status`'s own doc comment for why
    /// Branches doesn't drive Main the way the others do: a branch has
    /// no single "diff" of its own to show; a directory row aggregates
    /// an unknown mix of files, same reasoning).
    ///
    /// Two performance properties, both load-bearing for why this no
    /// longer visibly stutters the UI on every keystroke (it used to
    /// shell a real `git diff`/`show` synchronously on the input thread
    /// after *every* key handled while focus was on one of those panes,
    /// not just navigation -- see `GitSession::last_main_entry`'s and
    /// `main_request_id`'s own doc comments for the two-part fix):
    /// - Returns immediately, no work at all, when the cursor's entry
    ///   hasn't actually changed since the last sync.
    /// - The real `git` shell-out (when one's actually needed) runs on a
    ///   background thread for a real app (`self.event_proxy.is_some()`)
    ///   -- `apply_git_main` applies the result when it lands. Falls
    ///   back to a synchronous fetch when there's no event proxy (every
    ///   test, per `App::new`'s own doc comment -- same "only a real app
    ///   spawns anything" posture `DockerStatsPoller`/`GitStatusPoller`
    ///   already have), so existing tests asserting on Main's content
    ///   directly keep working with no mocking needed.
    pub(super) fn git_sync_main(&mut self) {
        let Some(session) = self.git_session.as_ref() else { return };
        let entry = self.git_entry_at_cursor();
        if entry == session.last_main_entry {
            return;
        }

        let fetch = match &entry {
            Some(git_panel::GitEntry::File(path)) => match session.files.iter().find(|f| &f.path == path) {
                Some(f) if f.index_status == '?' && f.worktree_status == '?' => MainFetch::Untracked,
                Some(f) => MainFetch::File { path: path.clone(), staged: f.index_status != '.' && f.index_status != '?' },
                None => MainFetch::Skip,
            },
            Some(git_panel::GitEntry::Commit(hash)) => MainFetch::Commit(hash.clone()),
            Some(git_panel::GitEntry::Stash(index)) => MainFetch::Stash(*index),
            Some(git_panel::GitEntry::Branch(_)) | Some(git_panel::GitEntry::Dir(_)) | None => MainFetch::Skip,
        };
        let repo_root = session.repo_root.clone();
        let main_buffer = session.main_buffer;

        let Some(session) = self.git_session.as_mut() else { return };
        session.last_main_entry = entry;
        session.last_main_fetch = fetch.clone();

        if let MainFetch::Skip = fetch {
            return;
        }
        if let MainFetch::Untracked = fetch {
            self.set_diff_buffer(main_buffer, None, DiffSource::ReadOnly, &repo_root);
            return;
        }

        session.main_request_id += 1;
        let request_id = session.main_request_id;

        match self.event_proxy.clone() {
            Some(proxy) => {
                std::thread::spawn(move || {
                    let diff = fetch_main_diff(&repo_root, &fetch);
                    let _ = proxy.send_event(FenixUserEvent::GitMainReady { request_id, buffer: main_buffer, diff });
                });
            }
            None => {
                let source = DiffSource::of(&fetch);
                let diff = fetch_main_diff(&repo_root, &fetch);
                self.set_diff_buffer(main_buffer, diff.as_deref(), source, &repo_root);
            }
        }
    }

    /// `s` on Files: stages the file under the cursor.
    pub(super) fn git_stage_selected(&mut self) {
        let Some(session) = self.git_session.as_ref() else { return };
        let repo_root = session.repo_root.clone();
        // `git add`/`git restore` are directory-transparent -- staging
        // a `Dir` path stages every changed file underneath it in one
        // call, same primitive as a single `File`, no separate function
        // needed.
        let path = match self.git_entry_at_cursor() {
            Some(git_panel::GitEntry::File(path)) | Some(git_panel::GitEntry::Dir(path)) => path,
            _ => return,
        };
        if let Err(err) = fenix_git::stage_file(&repo_root, &path) {
            self.set_error(format!("git stage failed: {err}"));
        }
        self.git_refresh_session();
    }

    /// `S` on Files: unstages the file (or every file under the
    /// directory) under the cursor.
    pub(super) fn git_unstage_selected(&mut self) {
        let Some(session) = self.git_session.as_ref() else { return };
        let repo_root = session.repo_root.clone();
        let path = match self.git_entry_at_cursor() {
            Some(git_panel::GitEntry::File(path)) | Some(git_panel::GitEntry::Dir(path)) => path,
            _ => return,
        };
        if let Err(err) = fenix_git::unstage_file(&repo_root, &path) {
            self.set_error(format!("git unstage failed: {err}"));
        }
        self.git_refresh_session();
    }

    /// `a` on Files: stages every changed file.
    pub(super) fn git_stage_all(&mut self) {
        let Some(session) = self.git_session.as_ref() else { return };
        let repo_root = session.repo_root.clone();
        if let Err(err) = fenix_git::stage_all(&repo_root) {
            self.set_error(format!("git stage all failed: {err}"));
        }
        self.git_refresh_session();
    }

    /// `A` on Files: unstages every staged file.
    pub(super) fn git_unstage_all(&mut self) {
        let Some(session) = self.git_session.as_ref() else { return };
        let repo_root = session.repo_root.clone();
        if let Err(err) = fenix_git::unstage_all(&repo_root) {
            self.set_error(format!("git unstage all failed: {err}"));
        }
        self.git_refresh_session();
    }

    /// `z` on Files: stashes every change (tracked + untracked staged/
    /// unstaged), git's own default `stash push` behavior.
    pub(super) fn git_stash_all(&mut self) {
        let Some(session) = self.git_session.as_ref() else { return };
        let repo_root = session.repo_root.clone();
        if let Err(err) = fenix_git::stash_push(&repo_root) {
            self.set_error(format!("git stash failed: {err}"));
        }
        self.git_refresh_session();
    }

    /// `P` on Files: pushes the current branch.
    pub(super) fn git_push(&mut self) {
        let Some(session) = self.git_session.as_ref() else { return };
        let repo_root = session.repo_root.clone();
        if let Err(err) = fenix_git::push(&repo_root) {
            self.set_error(format!("git push failed: {err}"));
        }
        self.git_refresh_session();
    }

    /// `p` on Files: pulls into the current branch.
    pub(super) fn git_pull(&mut self) {
        let Some(session) = self.git_session.as_ref() else { return };
        let repo_root = session.repo_root.clone();
        if let Err(err) = fenix_git::pull(&repo_root) {
            self.set_error(format!("git pull failed: {err}"));
        }
        self.git_refresh_session();
    }

    /// `c` on Branches: checks out the branch under the cursor.
    pub(super) fn git_checkout_selected_branch(&mut self) {
        let Some(session) = self.git_session.as_ref() else { return };
        let repo_root = session.repo_root.clone();
        if let Some(git_panel::GitEntry::Branch(name)) = self.git_entry_at_cursor() {
            if let Err(err) = fenix_git::checkout_branch(&repo_root, &name) {
                self.set_error(format!("git checkout failed: {err}"));
            }
            self.git_refresh_session();
        }
    }

    /// `a` on Stash: applies the entry under the cursor (keeps it in the
    /// stash list -- see `g` for apply-and-drop).
    pub(super) fn git_stash_apply_selected(&mut self) {
        let Some(session) = self.git_session.as_ref() else { return };
        let repo_root = session.repo_root.clone();
        if let Some(git_panel::GitEntry::Stash(index)) = self.git_entry_at_cursor() {
            if let Err(err) = fenix_git::stash_apply(&repo_root, index) {
                self.set_error(format!("git stash apply failed: {err}"));
            }
            self.git_refresh_session();
        }
    }

    /// `g` on Stash: applies the entry under the cursor and removes it
    /// from the stash list in one step.
    pub(super) fn git_stash_pop_selected(&mut self) {
        let Some(session) = self.git_session.as_ref() else { return };
        let repo_root = session.repo_root.clone();
        if let Some(git_panel::GitEntry::Stash(index)) = self.git_entry_at_cursor() {
            if let Err(err) = fenix_git::stash_pop(&repo_root, index) {
                self.set_error(format!("git stash pop failed: {err}"));
            }
            self.git_refresh_session();
        }
    }

    /// `c` on Files: opens a compose buffer for the commit message.
    ///
    /// A buffer rather than the single-line prompt every other Git
    /// action here uses, because a commit message is the one piece of
    /// text in this panel that is routinely more than one line -- a
    /// subject, a blank line and a body is the convention, and a prompt
    /// can express none of it.
    pub(super) fn git_commit_prompt(&mut self) {
        let repo_root = self.git_action_repo_root();
        if fenix_git::list_files(&repo_root).iter().all(|f| f.index_status == '.' || f.index_status == '?') {
            // `git commit` would refuse anyway, but it refuses *after*
            // the message has been written, which is the wrong moment
            // to find out.
            self.set_error("nothing staged to commit".to_string());
            return;
        }
        self.open_compose(ComposePurpose::CommitMessage { repo_root });
    }

    /// `n` on Branches: arms the new-branch-name prompt.
    pub(super) fn git_new_branch_prompt(&mut self) {
        self.git_prompt = Some(GitPrompt { kind: GitPromptKind::NewBranch, input: String::new() });
    }

    /// `d` on Files/Branches/Stash: arms the matching destructive-action
    /// confirmation for whatever's under the cursor.
    pub(super) fn git_arm_confirm(&mut self) {
        // In the diff pane the cursor sits on a diff row, not a
        // `GitEntry` row -- `d` there means "discard this hunk".
        if let Some((buffer, anchor)) = self.diff_anchor_at_cursor() {
            if anchor.hunk.is_some() {
                self.git_confirm = Some(GitConfirmAction::DiscardHunk { buffer, anchor });
            } else {
                self.set_error("put the cursor inside a hunk to discard it".to_string());
            }
            return;
        }
        let Some(entry) = self.git_entry_at_cursor() else { return };
        let action = match entry {
            git_panel::GitEntry::File(path) => {
                let untracked = self.git_selected_file().is_some_and(|f| f.index_status == '?' && f.worktree_status == '?');
                GitConfirmAction::DiscardFile { path, untracked }
            }
            git_panel::GitEntry::Dir(path) => GitConfirmAction::DiscardDir { path },
            git_panel::GitEntry::Branch(name) => GitConfirmAction::DeleteBranch { name },
            git_panel::GitEntry::Stash(index) => GitConfirmAction::DropStash { index },
            git_panel::GitEntry::Commit(_) => return,
        };
        self.git_confirm = Some(action);
    }

    // -- Rebase, merge and the operations that suspend -----------------

    /// The repository the Git panel or the history is wanted for, asked
    /// from where you are: the focused project's.
    pub(super) fn git_repo_wanted(&self) -> Option<PathBuf> {
        self.project_root.clone()
    }

    /// The repo any Git action should target: the Git page's, the Git
    /// panel's or the history's you're in, else the focused buffer's
    /// project. A panel left open in another workspace -- another
    /// project's -- isn't where you are.
    pub(super) fn git_action_repo_root(&self) -> PathBuf {
        if let Some(root) = self.focused_git_page_root() {
            return root;
        }
        let here = self.workspaces.active_index();
        self.git_session
            .as_ref()
            .filter(|s| s.workspace_index == here)
            .map(|s| s.repo_root.clone())
            .or_else(|| self.history_session.as_ref().filter(|s| s.workspace_index == here).map(|s| s.repo_root.clone()))
            .or_else(|| self.project_root.clone())
            .unwrap_or_else(|| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
    }

    /// Refreshes whichever Git views are open -- every operation here
    /// can change the working tree, the branch, and the graph at once.
    pub(super) fn git_refresh_all_views(&mut self) {
        let sweep = self.reload_buffers_changed_on_disk();
        if let Some(message) = sweep.message() {
            if sweep.is_bad() {
                self.set_error(message);
            } else {
                self.set_message(message);
            }
        }
        if self.git_session.is_some() {
            self.git_refresh_session();
        }
        if self.history_session.is_some() {
            self.history_refresh();
        }
        if self.merge_session.is_some() {
            self.merge_refresh();
        }
        self.refresh_git_pages(false);
    }

    // -- Merge view (`SPC g x`) --------------------------------------------

    /// `SPC g x x`: opens (or refocuses and refreshes) the Merge view --
    /// every conflicted file on the left, the selected one shown as two
    /// aligned columns on the right.
    pub(crate) fn open_merge_view(&mut self) {
        if let Some(session) = &self.merge_session {
            let (workspace_index, files_pane) = (session.workspace_index, session.files_pane);
            self.workspaces.switch_to_index(workspace_index);
            self.windows_mut().focus(files_pane);
            self.merge_refresh();
            self.wake_caret();
            return;
        }
        let repo_root = self.compare_repo_root();
        if fenix_git::status(&repo_root).is_none() {
            self.set_error(format!("{} isn't a git repository", repo_root.display()));
            return;
        }

        let files_buffer = self.buffers.open_git("");
        let merge_buffer = self.buffers.open_merge("");

        let cursor = Cursor::at_start();
        self.workspaces.new_workspace(files_buffer, cursor);
        let workspace_index = self.workspaces.active_index();
        let files_pane = self.focused_pane_id();
        let merge_pane = self.windows_mut().split(SplitKind::Vertical, merge_buffer);
        self.workspaces.active_pane_states_mut().insert(merge_pane, PaneState::seeded_at(cursor));
        // The file list is a short index; the two columns need the
        // width, and they're what's actually being read. Negative,
        // because the delta grows the *first* child (the list).
        self.windows_mut().resize_focused(-0.25);
        self.windows_mut().focus(files_pane);

        self.pane_titles.insert(files_pane, "1. Conflicts".to_string());
        self.pane_titles.insert(merge_pane, "2. Merge".to_string());

        self.merge_session = Some(MergeSession {
            workspace_index,
            files_pane,
            merge_pane,
            files_buffer,
            merge_buffer,
            repo_root,
            files: Vec::new(),
            selected: None,
            labels: merge_view::SideLabels::unknown(),
        });
        self.merge_refresh();
        self.wake_caret();
    }

    /// Re-lists the conflicted files and re-renders both panes.
    pub(super) fn merge_refresh(&mut self) {
        let Some(session) = self.merge_session.as_ref() else { return };
        let repo_root = session.repo_root.clone();
        let (files_buffer, merge_buffer, merge_pane) = (session.files_buffer, session.merge_buffer, session.merge_pane);

        // `u` entries are exactly git's own unmerged paths -- the same
        // ones `git status` calls "Unmerged", and the same list
        // `git rebase --continue` refuses to move past.
        let files: Vec<String> =
            fenix_git::list_files(&repo_root).into_iter().filter(|f| f.index_status == 'U' || f.worktree_status == 'U').map(|f| f.path).collect();
        let labels = fenix_git::conflict_sides(&repo_root).as_ref().map(merge_view::SideLabels::from).unwrap_or_else(merge_view::SideLabels::unknown);

        // Keep showing whatever was selected as long as it's still a
        // real path -- a file that has just been resolved should show
        // "nothing left to resolve" rather than silently jumping to a
        // different file the user wasn't looking at.
        let selected = session.selected.clone().filter(|p| files.contains(p) || repo_root.join(p).exists()).or_else(|| files.first().cloned());

        if let Some(session) = self.merge_session.as_mut() {
            session.files = files.clone();
            session.labels = labels.clone();
            session.selected = selected.clone();
        }
        self.set_git_buffer_preserving_line(files_buffer, git_panel::render_conflicts(&files, &labels.ours, &labels.theirs, &labels.ours_role, &labels.theirs_role));

        // Sized to the pane, so the two columns fill it rather than
        // being cut to a fixed 160 in a wider one or overflowing a
        // narrower one.
        let cols = self.pane_cols_or(merge_pane, merge_view::DEFAULT_WIDTH);
        let view = match &selected {
            Some(path) => {
                let full = repo_root.join(path);
                match std::fs::read_to_string(&full) {
                    Ok(text) => {
                        let conflicts = fenix_git::find_conflicts(&text);
                        merge_view::render(path, &text, &conflicts, &labels, cols)
                    }
                    Err(err) => {
                        merge_view::render(path, &format!("couldn't read {}: {err}\n", full.display()), &[], &labels, cols)
                    }
                }
            }
            None => merge_view::render("", "", &[], &labels, cols),
        };
        self.set_merge_buffer(merge_buffer, view);
    }

    /// The conflicted file the Conflicts pane's cursor is on -- or, on
    /// its header row with only one file listed, that one, which is what
    /// the hints above it offer to act on.
    pub(super) fn merge_file_at_cursor(&self) -> Option<String> {
        if let Some(git_panel::GitEntry::File(path)) = self.git_entry_at_cursor() {
            return Some(path);
        }
        match self.merge_session.as_ref().map(|s| s.files.as_slice()) {
            Some([only]) => Some(only.clone()),
            _ => None,
        }
    }

    /// `Enter` on the Conflicts pane: show that file in the right pane.
    pub(crate) fn merge_select_at_cursor(&mut self) {
        let Some(path) = self.merge_file_at_cursor() else {
            self.set_message("put the cursor on a conflicted file");
            return;
        };
        if let Some(session) = self.merge_session.as_mut() {
            session.selected = Some(path);
        }
        let merge_pane = self.merge_session.as_ref().map(|s| s.merge_pane);
        self.merge_refresh();
        if let Some(pane) = merge_pane {
            self.windows_mut().focus(pane);
        }
        self.wake_caret();
    }

    /// `o`/`t` on the Conflicts pane: resolve a whole file by taking one
    /// side, then stage it -- the "I already know which version wins"
    /// path.
    pub(crate) fn merge_take_whole_file(&mut self, side: fenix_git::Resolution) {
        let Some(session) = self.merge_session.as_ref() else { return };
        let repo_root = session.repo_root.clone();
        let labels = session.labels.clone();
        let Some(path) = self.merge_file_at_cursor() else {
            self.set_message("put the cursor on a conflicted file");
            return;
        };
        let kept = match side {
            fenix_git::Resolution::Ours => labels.ours.clone(),
            fenix_git::Resolution::Theirs => labels.theirs.clone(),
            fenix_git::Resolution::Both => {
                self.set_error("keeping both is a per-conflict choice -- open the file with Enter".to_string());
                return;
            }
        };
        match fenix_git::checkout_side(&repo_root, &path, side) {
            // Named by branch, not by "ours"/"theirs": the point of
            // saying it back is to confirm the *right* version survived.
            Ok(_) => self.set_message(format!("{path}: kept {kept}'s version, staged")),
            Err(err) => self.set_error(format!("couldn't take that side: {err}")),
        }
        self.merge_refresh();
        self.reload_buffers_changed_on_disk();
        self.wake_caret();
    }

    /// `u`: puts the selected file back the way git first wrote it,
    /// markers and all -- the undo for a resolution that went the wrong
    /// way.
    pub(crate) fn merge_restore_conflict(&mut self) {
        let Some(session) = self.merge_session.as_ref() else { return };
        let (repo_root, selected) = (session.repo_root.clone(), session.selected.clone());
        let Some(path) = selected else {
            self.set_message("no file selected");
            return;
        };
        match fenix_git::restore_conflict(&repo_root, &path) {
            Ok(_) => self.set_message(format!("{path}: conflict restored")),
            Err(err) => self.set_error(err),
        }
        self.merge_refresh();
        self.reload_buffers_changed_on_disk();
        self.wake_caret();
    }

    /// `s`: stages the selected file as resolved. Refuses while markers
    /// are still in it -- staging a file with `<<<<<<<` in it is how
    /// conflict markers end up committed.
    pub(crate) fn merge_stage_resolved(&mut self) {
        let Some(session) = self.merge_session.as_ref() else {
            self.set_message("no Merge view open -- SPC g x opens it");
            return;
        };
        let (repo_root, selected) = (session.repo_root.clone(), session.selected.clone());
        let Some(path) = selected else {
            self.set_message("no file selected");
            return;
        };
        let full = repo_root.join(&path);
        if let Ok(text) = std::fs::read_to_string(&full) {
            let left = fenix_git::find_conflicts(&text).len();
            if left > 0 {
                self.set_error(format!("{path} still has {left} unresolved conflict(s)"));
                return;
            }
        }
        match fenix_git::stage_file(&repo_root, &path) {
            Ok(_) => self.set_message(format!("{path}: staged as resolved")),
            Err(err) => self.set_error(format!("couldn't stage {path}: {err}")),
        }
        self.merge_refresh();
        self.wake_caret();
    }

    /// The conflict the cursor is on in the Merge pane, if it's there.
    pub(super) fn merge_conflict_at_cursor(&self) -> Option<usize> {
        let session = self.merge_session.as_ref()?;
        if self.focused_pane_id() != session.merge_pane {
            return None;
        }
        let line = self.open().buffer.line_col(&self.cursor()).0;
        let lines = self.merge_lines.get(&session.merge_buffer)?;
        // A row between conflicts belongs to none, so fall forward to
        // the next one -- the same "act on the next one down" rule
        // `resolve_conflict_at_cursor` uses in a plain buffer.
        lines
            .get(line)
            .and_then(|l| l.as_ref())
            .and_then(|l| l.conflict)
            .or_else(|| lines.iter().skip(line).flatten().find_map(|l| l.conflict))
    }

    /// `SPC g x o`/`t`/`b` inside the Merge view: resolve the conflict
    /// under the cursor, on disk, and re-render.
    ///
    /// Writes the file rather than a buffer because the Merge pane isn't
    /// the file -- it's a rendering of it. `u` restores the conflict if
    /// this was the wrong call.
    pub(super) fn merge_resolve_at_cursor(&mut self, resolution: fenix_git::Resolution) -> bool {
        let Some(index) = self.merge_conflict_at_cursor() else { return false };
        let Some(session) = self.merge_session.as_ref() else { return false };
        let (repo_root, selected, labels) = (session.repo_root.clone(), session.selected.clone(), session.labels.clone());
        let Some(path) = selected else { return false };
        let full = repo_root.join(&path);
        let Ok(text) = std::fs::read_to_string(&full) else {
            self.set_error(format!("couldn't read {}", full.display()));
            return true;
        };
        let conflicts = fenix_git::find_conflicts(&text);
        let Some(conflict) = conflicts.get(index) else {
            self.set_error("that conflict is gone -- the file changed underneath this view".to_string());
            self.merge_refresh();
            return true;
        };
        let resolved = fenix_git::resolve_conflict(&text, conflict, resolution);
        if let Err(err) = std::fs::write(&full, &resolved) {
            self.set_error(format!("couldn't write {}: {err}", full.display()));
            return true;
        }
        let kept = match resolution {
            fenix_git::Resolution::Ours => labels.ours.clone(),
            fenix_git::Resolution::Theirs => labels.theirs.clone(),
            fenix_git::Resolution::Both => "both sides".to_string(),
        };
        let left = fenix_git::find_conflicts(&resolved).len();
        self.set_message(if left == 0 {
            format!("kept {kept} -- {path} is resolved, SPC g x s stages it")
        } else {
            format!("kept {kept} -- {left} conflict(s) left in {path}")
        });
        self.merge_refresh();
        self.reload_buffers_changed_on_disk();
        self.wake_caret();
        true
    }

    /// `SPC g q` (in its workspace): closes the Merge view.
    pub(crate) fn merge_close(&mut self) {
        let Some(session) = self.merge_session.take() else { return };
        for id in [session.files_buffer, session.merge_buffer] {
            self.buffers.close(id);
            self.git_lines.remove(&id);
            self.merge_lines.remove(&id);
        }
        for pane in [session.files_pane, session.merge_pane] {
            self.pane_titles.remove(&pane);
        }
        self.workspaces.switch_to_index(session.workspace_index);
        self.remove_active_workspace();
        self.refresh_project_root();
        self.wake_caret();
    }

    pub(crate) fn git_rebase_onto(&mut self, onto: &str) {
        let repo_root = self.git_action_repo_root();
        let before = fenix_git::oplog::rev(&repo_root, "HEAD");
        let result = fenix_git::oplog::logged(&repo_root, &format!("rebase onto {onto}"), || fenix_git::rebase(&repo_root, onto), |_| {
            fenix_git::oplog::Undo::Reset { to: before, soft: false, saved: None }
        });
        self.run_git_operation(&format!("rebase onto {onto}"), result);
    }

    pub(crate) fn git_merge_from(&mut self, branch: &str) {
        let repo_root = self.git_action_repo_root();
        let before = fenix_git::oplog::rev(&repo_root, "HEAD");
        let result = fenix_git::oplog::logged(&repo_root, &format!("merge {branch}"), || fenix_git::merge(&repo_root, branch), |_| {
            fenix_git::oplog::Undo::Reset { to: before, soft: false, saved: None }
        });
        self.run_git_operation(&format!("merge {branch}"), result);
    }

    /// `SPC g x c`: carries on whichever operation is suspended.
    ///
    /// One key for all of them because from the user's side it's one
    /// question -- "I've fixed it, keep going" -- and having to remember
    /// whether this was a rebase or a cherry-pick to pick the right key
    /// is exactly the friction the banner exists to remove.
    pub(crate) fn git_operation_continue(&mut self) {
        let repo_root = self.git_action_repo_root();
        let Some(op) = fenix_git::in_progress(&repo_root) else {
            self.set_error("nothing in progress to continue".to_string());
            return;
        };
        // Staging is the caller's job in git's model, and skipping it is
        // the single most common way `--continue` fails; say so rather
        // than surfacing git's own longer complaint.
        let unresolved = conflict_count(&fenix_git::list_files(&repo_root));
        if unresolved > 0 {
            self.set_error(format!("{unresolved} file(s) still conflicted -- resolve and stage them first"));
            return;
        }
        let result = match op {
            fenix_git::InProgress::Rebase { .. } => fenix_git::rebase_continue(&repo_root),
            // Git's prepared message, in the composer to be edited, as
            // `git commit` opens it in an editor.
            fenix_git::InProgress::Merge => {
                let message = fenix_git::merge_message(&repo_root).unwrap_or_else(|| "Merge".to_string());
                self.open_compose_seeded(ComposePurpose::CommitMessage { repo_root }, &message);
                return;
            }
            fenix_git::InProgress::CherryPick | fenix_git::InProgress::Revert | fenix_git::InProgress::Bisect => {
                self.set_error(format!("{} can't be continued from here -- finish it with git", op.label()));
                return;
            }
        };
        self.run_git_operation("continue", result);
    }

    /// `SPC g x a`: abandons whichever operation is suspended, putting the
    /// tree back where it started.
    pub(crate) fn git_operation_abort(&mut self) {
        let repo_root = self.git_action_repo_root();
        let Some(op) = fenix_git::in_progress(&repo_root) else {
            self.set_error("nothing in progress to abort".to_string());
            return;
        };
        let result = match op {
            fenix_git::InProgress::Rebase { .. } => fenix_git::rebase_abort(&repo_root),
            fenix_git::InProgress::Merge => fenix_git::merge_abort(&repo_root),
            fenix_git::InProgress::CherryPick | fenix_git::InProgress::Revert | fenix_git::InProgress::Bisect => {
                self.set_error(format!("{} can't be aborted from here -- finish it with git", op.label()));
                return;
            }
        };
        self.run_git_operation("abort", result);
    }

    /// `SPC g p`: pull, rebasing local commits on top rather than adding
    /// a merge commit for every sync.
    pub(crate) fn git_pull_rebase(&mut self) {
        let repo_root = self.git_action_repo_root();
        let result = fenix_git::pull_rebase(&repo_root);
        self.run_git_operation("pull --rebase", result);
    }

    /// `git.force_push`: push after a rebase rewrote history.
    ///
    /// Always `--force-with-lease`: it refuses if the remote moved since
    /// the last fetch, so it can't silently discard someone else's work.
    /// Confirmed first anyway, since it does rewrite a published branch.
    pub(crate) fn git_force_push(&mut self) {
        self.git_confirm = Some(GitConfirmAction::ForcePush);
    }

    /// `SPC g x j`/`SPC g x k` inside the Merge view: jump to the next or
    /// previous conflict's separator row. Returns whether it applied,
    /// so the plain-buffer path can take over when it didn't.
    pub(super) fn merge_goto_conflict(&mut self, forward: bool) -> bool {
        let Some(session) = self.merge_session.as_ref() else { return false };
        if self.focused_pane_id() != session.merge_pane {
            return false;
        }
        let Some(lines) = self.merge_lines.get(&session.merge_buffer) else { return true };
        // The separator row is the one to land on: it names the conflict
        // ("conflict 2 of 3") and sits directly above both columns.
        let starts: Vec<usize> = lines
            .iter()
            .enumerate()
            .filter(|(_, l)| l.as_ref().is_some_and(|l| l.style == merge_view::MergeStyle::Separator))
            .map(|(i, _)| i)
            .collect();
        if starts.is_empty() {
            self.set_message("no conflicts left in this file");
            return true;
        }
        let line = self.open().buffer.line_col(&self.cursor()).0;
        // Wraps, like the plain-buffer path: there are only ever a
        // handful, and cycling beats being stuck at the last one.
        let target = if forward {
            starts.iter().find(|&&s| s > line).copied().unwrap_or(starts[0])
        } else {
            starts.iter().rev().find(|&&s| s < line).copied().unwrap_or(*starts.last().unwrap())
        };
        let char_idx = self.open().buffer.line_start_char(target);
        let pane = self.focused_pane_id();
        self.pane_state_mut(pane).cursor = Cursor { char_idx, sticky_col: 0 };
        self.wake_caret();
        true
    }

    // -- History view (`SPC g l`) ------------------------------------------

    /// `SPC g l`: opens (or refocuses and refreshes) the History view --
    /// the commit graph across *every* branch, a refs tree showing how
    /// each local branch stands against its upstream, and the selected
    /// commit's diff in the shared diff viewer.
    pub(crate) fn open_history_view(&mut self) {
        let wanted = self.git_repo_wanted();
        if let Some(session) = &mut self.history_session {
            if let Some(wanted) = wanted.filter(|w| session.workspace_index != self.workspaces.active && !same_repository(w, &session.repo_root)) {
                session.repo_root = wanted;
                session.commits.clear();
                session.last_detail_commit = None;
            }
            let (workspace_index, graph_pane) = (session.workspace_index, session.graph_pane);
            self.workspaces.switch_to_index(workspace_index);
            self.windows_mut().focus(graph_pane);
            self.history_refresh();
            self.wake_caret();
            return;
        }

        let repo_root = self.project_root.clone().unwrap_or_else(|| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        let graph_buffer = self.buffers.open_graph("");
        let refs_buffer = self.buffers.open_git("");
        let detail_view = diff_view::render(&[], &HashSet::new());
        let detail_buffer = self.buffers.open_diff(&detail_view.text);
        self.diff_lines.insert(detail_buffer, detail_view.lines);

        let cursor = Cursor::at_start();
        self.workspaces.new_workspace(graph_buffer, cursor);
        let workspace_index = self.workspaces.active_index();
        let graph_pane = self.focused_pane_id();

        // Graph on the left, refs above the commit's diff on the right --
        // the graph is the thing being read, so it keeps the larger
        // share, same proportions the Docker panel's own list/detail
        // split already uses.
        let detail_pane = self.windows_mut().split(SplitKind::Vertical, detail_buffer);
        self.workspaces.active_pane_states_mut().insert(detail_pane, PaneState::seeded_at(cursor));
        // `resize_focused` adjusts the *first* child's share of the
        // nearest split whichever side is focused, so this gives the
        // graph 40% and the right-hand column 60%, where the diffs are.
        self.windows_mut().resize_focused(-0.1);
        let refs_pane = self.windows_mut().split(SplitKind::Horizontal, refs_buffer);
        self.workspaces.active_pane_states_mut().insert(refs_pane, PaneState::seeded_at(cursor));
        // Commit detail (the split's first child) takes three quarters:
        // it holds a whole diff, while the refs tree is a short list
        // that only ever needs a handful of rows.
        self.windows_mut().resize_focused(0.25);
        self.windows_mut().focus(graph_pane);

        self.pane_titles.insert(graph_pane, "1. Graph".to_string());
        self.pane_titles.insert(refs_pane, "2. Refs".to_string());
        self.pane_titles.insert(detail_pane, "3. Commit".to_string());

        self.history_session = Some(HistorySession {
            workspace_index,
            graph_pane,
            refs_pane,
            detail_pane,
            graph_buffer,
            refs_buffer,
            detail_buffer,
            repo_root,
            commits: Vec::new(),
            last_detail_commit: None,
            detail_request_id: 0,
            refresh_request_id: 0,
        });
        self.history_refresh();
        self.wake_caret();
    }

    /// Re-reads the graph and the refs tree from disk, off the UI
    /// thread: on a large repository `git log --all` and the refs'
    /// ahead/behind counts take long enough to stall typing. Synchronous
    /// only without an `event_proxy` (every test), like the detail pane.
    pub(super) fn history_refresh(&mut self) {
        let limit = self.config.git_graph_limit.unwrap_or(200);
        let Some(session) = self.history_session.as_mut() else { return };
        session.refresh_request_id += 1;
        let (request_id, repo_root, graph_buffer, first) = (session.refresh_request_id, session.repo_root.clone(), session.graph_buffer, session.commits.is_empty());
        match self.event_proxy.clone() {
            Some(proxy) => {
                if first {
                    self.replace_buffer_text(graph_buffer, "    reading the history…\n");
                }
                std::thread::spawn(move || {
                    let data = HistoryData::read(&repo_root, limit);
                    let _ = proxy.send_event(FenixUserEvent::GitHistoryReady { request_id, data: Box::new(data) });
                });
            }
            None => {
                let data = HistoryData::read(&repo_root, limit);
                self.apply_history(request_id, data);
            }
        }
    }

    /// `1`/`2`/`3` in the History view, matching the pane titles -- the
    /// same jump-to-pane shortcut the Docker and Git panels already
    /// advertise in theirs.
    pub(super) fn history_pane_by_number(&self, n: u32) -> Option<fenix_window::WindowId> {
        let session = self.history_session.as_ref()?;
        match n {
            1 => Some(session.graph_pane),
            2 => Some(session.refs_pane),
            3 => Some(session.detail_pane),
            _ => None,
        }
    }

    /// `1`/`2` in the Compare view.
    pub(super) fn merge_pane_by_number(&self, n: u32) -> Option<fenix_window::WindowId> {
        let session = self.merge_session.as_ref()?;
        match n {
            1 => Some(session.files_pane),
            2 => Some(session.merge_pane),
            _ => None,
        }
    }

    /// The commit the cursor is on in the Graph pane, if that's where it
    /// is.
    pub(super) fn history_commit_at_cursor(&self) -> Option<String> {
        let session = self.history_session.as_ref()?;
        if self.focused_pane_id() != session.graph_pane {
            return None;
        }
        let line = self.open().buffer.line_col(&self.cursor()).0;
        self.graph_lines.get(&session.graph_buffer)?.get(line)?.as_ref()?.commit.clone()
    }

    /// Fetches the selected commit's diff into the detail pane, skipping
    /// the work when the cursor hasn't moved to a different commit --
    /// same shape as `git_sync_main`, including its synchronous fallback
    /// when there's no `event_proxy` (every test).
    pub(super) fn history_sync_detail(&mut self) {
        let Some(session) = self.history_session.as_ref() else { return };
        let commit = match self.history_commit_at_cursor() {
            Some(hash) => Some(hash),
            // The graph's connector rows (`|\`, `|/`) draw no commit of
            // their own. Scrolling across one must not yank the detail
            // pane back to the newest commit -- keep showing whatever
            // was selected.
            None if session.last_detail_commit.is_some() => return,
            // Nothing selected yet (the view just opened): show the
            // newest commit rather than an empty pane.
            None => session.commits.first().map(|c| c.hash.clone()),
        };
        if commit == session.last_detail_commit {
            return;
        }
        let (repo_root, detail_buffer) = (session.repo_root.clone(), session.detail_buffer);
        let Some(session) = self.history_session.as_mut() else { return };
        session.last_detail_commit = commit.clone();
        session.detail_request_id += 1;
        let request_id = session.detail_request_id;

        let Some(hash) = commit else {
            self.set_diff_buffer(detail_buffer, None, DiffSource::ReadOnly, &repo_root);
            return;
        };

        match self.event_proxy.clone() {
            Some(proxy) => {
                std::thread::spawn(move || {
                    let detail = commit_detail(&repo_root, &hash);
                    let _ = proxy.send_event(FenixUserEvent::GitHistoryDetailReady { request_id, buffer: detail_buffer, diff: detail });
                });
            }
            None => {
                let detail = commit_detail(&repo_root, &hash);
                self.apply_history_detail(request_id, detail_buffer, detail);
            }
        }
    }

    /// `SPC g f`: `git fetch --all --prune`, off the input thread since
    /// it touches the network. Refreshes whichever Git views are open
    /// once it lands, so the sync badges reflect what was just fetched.
    pub(crate) fn git_fetch(&mut self) {
        let repo_root = self
            .history_session
            .as_ref()
            .map(|s| s.repo_root.clone())
            .or_else(|| self.git_session.as_ref().map(|s| s.repo_root.clone()))
            .or_else(|| self.project_root.clone())
            .unwrap_or_else(|| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

        match self.event_proxy.clone() {
            Some(proxy) => {
                self.set_message("fetching...");
                self.motion.git_fetching = true;
                std::thread::spawn(move || {
                    let result = fenix_git::fetch(&repo_root);
                    let _ = proxy.send_event(FenixUserEvent::GitFetched { result });
                });
            }
            None => {
                let result = fenix_git::fetch(&repo_root);
                self.apply_git_fetched(result);
            }
        }
    }

    // -- Compare view (`SPC g c`) ------------------------------------------

    /// Every ref that can be compared: local branches, remote-tracking
    /// branches, tags. `HEAD` leads the list, since "what does my
    /// working checkout add" is the overwhelmingly common question.
    pub(super) fn compare_ref_candidates(&self, repo_root: &Path) -> Vec<String> {
        let mut refs = vec!["HEAD".to_string()];
        refs.extend(fenix_git::list_branches(repo_root).into_iter().map(|b| b.name));
        refs.extend(fenix_git::list_remote_branches(repo_root));
        refs.extend(fenix_git::list_tags(repo_root));
        refs
    }

    pub(super) fn compare_repo_root(&self) -> PathBuf {
        self.git_session
            .as_ref()
            .map(|s| s.repo_root.clone())
            .or_else(|| self.project_root.clone())
            .unwrap_or_else(|| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
    }

    /// `SPC g q` (in its workspace): closes the History view.
    pub(crate) fn history_close(&mut self) {
        let Some(session) = self.history_session.take() else { return };
        for id in [session.graph_buffer, session.refs_buffer, session.detail_buffer] {
            self.buffers.close(id);
            self.git_lines.remove(&id);
            self.graph_lines.remove(&id);
            self.diff_lines.remove(&id);
            self.diff_models.remove(&id);
        }
        for pane in [session.graph_pane, session.refs_pane, session.detail_pane] {
            self.pane_titles.remove(&pane);
        }
        self.workspaces.switch_to_index(session.workspace_index);
        self.remove_active_workspace();
        self.refresh_project_root();
        self.wake_caret();
    }

    /// Closes the whole Git panel session -- mirrors
    /// `DockerHost::close` exactly.
    pub(crate) fn git_session_close(&mut self) {
        let Some(session) = self.git_session.take() else { return };
        for id in [
            session.status_buffer,
            session.staged_buffer,
            session.unstaged_buffer,
            session.branches_buffer,
            session.commits_buffer,
            session.stash_buffer,
            session.main_buffer,
        ] {
            self.buffers.close(id);
            self.git_lines.remove(&id);
            self.diff_lines.remove(&id);
            self.diff_models.remove(&id);
        }
        for pane in [
            session.status_pane,
            session.staged_pane,
            session.unstaged_pane,
            session.branches_pane,
            session.commits_pane,
            session.stash_pane,
            session.main_pane,
        ] {
            self.pane_titles.remove(&pane);
        }
        self.workspaces.switch_to_index(session.workspace_index);
        self.remove_active_workspace();
        self.refresh_project_root();
        self.wake_caret();
    }

    /// Modeline text while a Git destructive action is armed -- mirrors
    /// `DockerHost::confirm_text`.
    pub(super) fn git_confirm_text(&self) -> Option<String> {
        let action = self.git_confirm.as_ref()?;
        Some(match action {
            GitConfirmAction::DiscardEditorHunk { .. } => "Discard this hunk? It goes back to what's staged -- U on the Git page undoes it (y/n)".to_string(),
            GitConfirmAction::SwitchWithStash { branch } => {
                format!("Your changes would be overwritten by switching to {branch} -- stash them and switch? They come back when you return (y/n)")
            }
            GitConfirmAction::DiscardFile { path, .. } => format!("Discard changes to {path}? (y/n)"),
            GitConfirmAction::DiscardDir { path } => format!("Discard all changes under {path}/? (y/n)"),
            GitConfirmAction::DeleteBranch { name } => format!("Delete branch {name}? (y/n)"),
            GitConfirmAction::DropStash { index } => format!("Drop stash@{{{index}}}? (y/n)"),
            GitConfirmAction::ForcePush => "Force-push (with lease), rewriting the remote branch? (y/n)".to_string(),
            GitConfirmAction::DiscardHunk { buffer, anchor } => {
                let path = self
                    .diff_models
                    .get(buffer)
                    .and_then(|m| m.files.get(anchor.file))
                    .map(|f| f.display_path().to_string())
                    .unwrap_or_else(|| "this file".to_string());
                format!("Discard this hunk of {path}? (y/n)")
            }
        })
    }

    /// Resolves an armed Git destructive action: `y` confirms and runs
    /// it, anything else cancels. Mirrors `DockerHost::confirm_key`.
    pub(super) fn git_confirm_key(&mut self, keypress: KeyPress) {
        let action = self.git_confirm.take();
        match (keypress.code == KeyCode::Char('y'), action) {
            (true, Some(GitConfirmAction::DiscardEditorHunk { buffer, line })) => {
                self.git_hunk_discard(buffer, line);
                self.wake_caret();
                return;
            }
            (true, Some(GitConfirmAction::SwitchWithStash { branch })) => {
                self.git_switch_stashing(&branch);
                self.wake_caret();
                return;
            }
            (_, Some(GitConfirmAction::DiscardEditorHunk { .. } | GitConfirmAction::SwitchWithStash { .. })) => {
                self.wake_caret();
                return;
            }
            (_, action) => self.git_confirm = action,
        }
        let action = self.git_confirm.take();
        if keypress.code == KeyCode::Char('y') {
            if let (Some(action), Some(session)) = (action, self.git_session.as_ref()) {
                let repo_root = session.repo_root.clone();
                let result = match action {
                    GitConfirmAction::DiscardFile { path, untracked } => fenix_git::discard_file(&repo_root, &path, untracked),
                    GitConfirmAction::DiscardDir { path } => fenix_git::discard_dir(&repo_root, &path),
                    GitConfirmAction::DeleteBranch { name } => fenix_git::delete_branch(&repo_root, &name, false),
                    GitConfirmAction::DropStash { index } => fenix_git::stash_drop(&repo_root, index),
                    GitConfirmAction::ForcePush => fenix_git::push_force_with_lease(&repo_root),
                    GitConfirmAction::DiscardHunk { buffer, anchor } => self.hunk_patch_for(buffer, anchor).map_or_else(
                        || Err("that hunk is no longer in the diff".to_string()),
                        |patch| fenix_git::apply_patch(&repo_root, &patch, fenix_git::ApplyTarget::Discard),
                    ),
                    GitConfirmAction::DiscardEditorHunk { .. } | GitConfirmAction::SwitchWithStash { .. } => unreachable!("handled above"),
                };
                if let Err(err) = result {
                    self.set_error(format!("git action failed: {err}"));
                }
                self.git_refresh_session();
            }
        }
        self.wake_caret();
    }

    /// What to show in place of the modeline while `git_prompt` is
    /// active -- mirrors `explorer_prompt_text`.
    pub(super) fn git_prompt_text(&self) -> Option<String> {
        let prompt = self.git_prompt.as_ref()?;
        Some(match prompt.kind {
            GitPromptKind::NewBranch => format!("New branch name: {}", prompt.input),
        })
    }

    /// Mirrors `explorer_prompt_key`'s text-accumulation shape exactly.
    pub(super) fn git_prompt_key(&mut self, key: KeyPress) {
        if key == KeyPress::char('v').with_ctrl() {
            let pasted = self.clipboard_text();
            if let (Some(prompt), Some(text)) = (&mut self.git_prompt, pasted) {
                prompt.input.push_str(&text);
            }
            self.wake_caret();
            return;
        }
        let Some(prompt) = &mut self.git_prompt else { return };
        match key.code {
            KeyCode::Named(FenixNamedKey::Escape) => self.git_prompt = None,
            KeyCode::Named(FenixNamedKey::Enter) => {
                let GitPrompt { kind, input } = self.git_prompt.take().unwrap();
                self.git_prompt_submit(kind, &input);
            }
            KeyCode::Named(FenixNamedKey::Backspace) => {
                prompt.input.pop();
            }
            KeyCode::Char(c) => prompt.input.push(c),
            _ => {}
        }
        self.wake_caret();
    }

    pub(super) fn git_prompt_submit(&mut self, kind: GitPromptKind, input: &str) {
        if input.trim().is_empty() {
            return;
        }
        let Some(session) = self.git_session.as_ref() else { return };
        let repo_root = session.repo_root.clone();
        let result = match kind {
            GitPromptKind::NewBranch => fenix_git::create_branch(&repo_root, input),
        };
        if let Err(err) = result {
            self.set_error(format!("git operation failed: {err}"));
        }
        self.git_refresh_session();
    }

    /// The which-key popup's rich-text spans (key column in
    /// `theme.caret_text`, label/overflow-summary in the modeline's own
    /// text color, sorted alphabetically by label for scannability) and
    /// its resolved on-screen rect, or `None` when nothing is pending. The
    /// popup shares the modeline's background (`bg_modeline`), so both
    /// colors here are picked from that family, not the content one --
    /// `syntax_keyword` was tried for the key column originally, but it's
    /// calibrated for `bg` (the content background), and on TempleOS
    /// specifically its value is identical to `bg_modeline`, making the
    /// key column invisible. `caret_text` is guaranteed high-contrast
    /// against `bg_modeline` in every theme (`caret` already needs that
    /// against `bg` to work as a caret), which is what makes it a safe
    /// second accent color here too.
    /// Two other things could previously make the panel not actually fit
    /// its own content, both fixed here: its *width* is now sized to the
    /// longest visible label at the font's real measured `char_width`
    /// (clamped to `[WHICH_KEY_MIN_WIDTH, WHICH_KEY_MAX_WIDTH]` and to
    /// what the window can hold) instead of a fixed 260px that didn't
    /// scale with font size or label length -- a big font or a long
    /// label (real examples: "insert at line start", "WORD backward")
    /// used to just get cut off; and its *height* truncates to whatever
    /// `popup::max_rows` says actually fits above the modeline, with a
    /// trailing "+N more" summary row instead of letting the panel run
    /// under it.
    /// The Git panel's contextual "view command options" popup (`x` on a
    /// Files/Branches/Commits/Stash pane) -- mirrors `DockerHost::menu_popup`
    /// exactly, including its own note on why literal Vim/lazygit
    /// `<space>`-based stage-toggle isn't used here: `SPC` is already
    /// globally claimed as Fenix's leader-key trigger (checked earlier
    /// than any pane-role routing in `handle_key`), so Files uses
    /// separate `s`/`S` stage/unstage keys instead, matching the
    /// existing Docker-panel precedent of separate keys over a single
    /// toggle (`s`/`S` start/stop, `R` restart) rather than inventing a
    /// new convention. `None` on Status/Main, which have no bindings of
    /// their own beyond the global `u` (mirrors Docker's Status/Logs).
    pub(super) fn git_menu_popup(&self, window_width: f32, modeline_top: f32) -> Option<(fenix_window::Rect, RowSpans)> {
        if !self.git_menu_open {
            return None;
        }
        // The History and Compare views are separate workspaces with no
        // `GitPaneRole` of their own, but `x` means the same thing in
        // them -- "what can I press here" -- so they get their own key
        // lists rather than no menu at all.
        if let Some(session) = &self.history_session {
            let focused = self.focused_pane_id();
            let bindings: Option<&[(&str, &str)]> = if focused == session.graph_pane || focused == session.refs_pane {
                Some(&[("1/2/3", "go to pane"), ("u", "refresh"), ("f", "fetch")])
            } else if focused == session.detail_pane {
                Some(&[("Tab", "fold/unfold file"), ("]", "next hunk"), ("[", "prev hunk"), ("Enter", "open file here"), ("u", "refresh")])
            } else {
                None
            };
            if let Some(bindings) = bindings {
                return self.git_menu_popup_at(bindings, window_width, modeline_top);
            }
        }
        if let Some(session) = &self.merge_session {
            let focused = self.focused_pane_id();
            // The labels name the branches rather than saying "ours" and
            // "theirs" -- the popup is for someone who isn't sure which
            // key does what, so it's the wrong place to make them
            // translate git's own vocabulary.
            let (ours, theirs) = (session.labels.ours.as_str(), session.labels.theirs.as_str());
            let keep_ours = format!("keep {ours}");
            let keep_theirs = format!("keep {theirs}");
            let bindings: Option<Vec<(&str, &str)>> = if focused == session.files_pane {
                Some(vec![
                    ("1/2", "go to pane"),
                    ("Enter", "resolve line by line"),
                    ("o", keep_ours.as_str()),
                    ("t", keep_theirs.as_str()),
                    ("s", "stage as resolved"),
                    ("u", "put the conflict back"),
                    ("r", "refresh"),
                ])
            } else if focused == session.merge_pane {
                Some(vec![
                    ("1/2", "go to pane"),
                    ("n/p", "next/prev conflict"),
                    ("o", keep_ours.as_str()),
                    ("t", keep_theirs.as_str()),
                    ("b", "keep both"),
                    ("s", "stage as resolved"),
                    ("u", "put the conflict back"),
                ])
            } else {
                None
            };
            if let Some(bindings) = bindings {
                return self.git_menu_popup_at(&bindings, window_width, modeline_top);
            }
        }
        let bindings: &[(&str, &str)] = match self.git_focused_role()? {
            GitPaneRole::Staged => &[
                ("Tab", "expand/collapse"),
                ("s", "stage"),
                ("S", "unstage"),
                ("a", "stage all"),
                ("A", "unstage all"),
                ("c", "commit"),
                ("z", "stash all"),
                ("P", "push"),
                ("p", "pull"),
                ("u", "refresh"),
            ],
            GitPaneRole::Unstaged => &[
                ("Tab", "expand/collapse"),
                ("s", "stage"),
                ("S", "unstage"),
                ("a", "stage all"),
                ("A", "unstage all"),
                ("c", "commit"),
                ("d", "discard"),
                ("z", "stash all"),
                ("P", "push"),
                ("p", "pull"),
                ("u", "refresh"),
            ],
            GitPaneRole::Branches => &[("c", "checkout"), ("n", "new branch"), ("d", "delete"), ("u", "refresh")],
            GitPaneRole::Commits => &[("u", "refresh")],
            GitPaneRole::Stash => &[("a", "apply"), ("g", "pop"), ("d", "drop"), ("u", "refresh")],
            GitPaneRole::Main => &[
                ("s", "stage hunk"),
                ("S", "unstage hunk"),
                ("d", "discard hunk"),
                ("]", "next hunk"),
                ("[", "prev hunk"),
                ("Tab", "fold file"),
                ("Enter", "open file here"),
                ("u", "refresh"),
            ],
            GitPaneRole::Status => return None,
        };
        self.git_menu_popup_at(bindings, window_width, modeline_top)
    }

    /// Lays out one key list as the contextual popup -- the shared half
    /// of `git_menu_popup`, so the History and Compare views' own key
    /// lists render identically to the Git panel's without duplicating
    /// the geometry.
    pub(super) fn git_menu_popup_at(
        &self,
        bindings: &[(&str, &str)],
        window_width: f32,
        modeline_top: f32,
    ) -> Option<(fenix_window::Rect, RowSpans)> {
        let (char_width, line_height) = match &self.text {
            Some(text) => (text.char_width(), text.line_height()),
            None => (text::CHAR_WIDTH, text::LINE_HEIGHT),
        };

        let max_rows = popup::max_rows(modeline_top, text::WHICH_KEY_MARGIN, line_height, WHICH_KEY_PADDING).max(1);
        let shown_count = bindings.len().min(max_rows);

        const KEY_COLUMN_CHARS: usize = 6;
        let longest_label = bindings[..shown_count].iter().map(|(_, label)| label.chars().count()).max().unwrap_or(0);
        let content_chars = KEY_COLUMN_CHARS + longest_label + 1;
        let max_width = (window_width - 2.0 * text::WHICH_KEY_MARGIN).max(text::WHICH_KEY_MIN_WIDTH);
        let width = (content_chars as f32 * char_width + WHICH_KEY_PADDING)
            .clamp(text::WHICH_KEY_MIN_WIDTH, text::WHICH_KEY_MAX_WIDTH.min(max_width));

        let theme = self.theme;
        let mut spans = Vec::new();
        for (i, (key, label)) in bindings[..shown_count].iter().enumerate() {
            if i > 0 {
                spans.push(("\n".to_string(), theme.fg_modeline, false));
            }
            spans.push((format!("{key:<KEY_COLUMN_CHARS$}"), theme.caret_text, false));
            spans.push(((*label).to_string(), theme.fg_modeline, false));
        }

        let height = shown_count as f32 * line_height + WHICH_KEY_PADDING;
        let rect = popup::resolve(popup::Anchor::TopRight { margin: text::WHICH_KEY_MARGIN }, width, height, window_width, modeline_top);
        Some((rect, spans))
    }
}
