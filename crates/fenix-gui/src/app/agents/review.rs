//! The agents' change review: *Agent changes* (`SPC a r`), the tint on
//! lines an agent changed, follow mode, and catching changes no tool call
//! announced. What a hunk is and what keeping or rejecting one does is
//! `agent_review`'s; this reads the files, writes the answers back --
//! through the buffer when one is open, as one undo step -- and keeps
//! each thread's baselines up to date.

use super::*;
use crate::agent_review::{self, Baseline, ReviewFile, ReviewPage};

/// Who the tints on agent-changed lines belong to, among a buffer's
/// decorations.
const MARKS: &str = "fenix:agents";

impl App {
    /// A file's text now: its buffer's when it's open, else the disk's.
    pub(super) fn agents_current(&self, path: &Path) -> Option<String> {
        match self.buffers.id_for_path(path).and_then(|id| self.buffers.get(id)) {
            Some(ob) => Some(ob.buffer.text()),
            None => std::fs::read_to_string(path).ok(),
        }
    }

    /// Every file `thread` changed, as the review shows them.
    pub(super) fn agents_review_files(&self, thread: &ThreadId) -> Vec<ReviewFile> {
        let root = self.agents.hub.meta(thread).map(|m| m.root.clone()).unwrap_or_default();
        let mut files = Vec::new();
        for (path, before) in self.agents.baselines.get(thread).into_iter().flatten() {
            let baseline = match before {
                Some(text) => Baseline::Text(text.clone()),
                None => Baseline::Absent,
            };
            let name = path.strip_prefix(&root).map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|_| path.to_string_lossy().into_owned());
            let file = ReviewFile::new(path.clone(), name, baseline, self.agents_current(path));
            if file.changed() {
                files.push(file);
            }
        }
        for path in self.agents.unknown.get(thread).into_iter().flatten() {
            let name = path.strip_prefix(&root).map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
            files.push(ReviewFile::new(path.clone(), name, Baseline::Unknown, self.agents_current(path)));
        }
        files
    }

    /// `SPC a r`: the shown thread's changes.
    pub(crate) fn agents_open_review(&mut self) {
        let Some(thread) = self.agents.shown(&self.pages).or_else(|| self.agents.hub.focused().cloned()) else {
            self.set_message("no agent thread open: SPC a a");
            return;
        };
        let files = self.agents_review_files(&thread);
        // It's about code: never in the dock, and the one already up is
        // the one shown.
        if self.agents_is_dock_pane(self.focused_pane_id()) {
            self.agents_focus_code();
        }
        if let Some(id) = self.find_page(|m| matches!(m, PageModel::AgentReview(_))) {
            if let Some(PageModel::AgentReview(p)) = self.pages.get_mut(&id).map(|s| &mut s.model) {
                **p = ReviewPage::new(thread, files);
            }
            self.restale_page(id);
            match self.agents_panes_showing(id).first() {
                Some(pane) => {
                    self.windows_mut().focus(*pane);
                    self.wake_caret();
                }
                None => self.show_page(id),
            }
            return;
        }
        self.open_page(PageModel::AgentReview(Box::new(ReviewPage::new(thread, files))));
    }

    pub(crate) fn agents_review_key(&mut self, id: BufferId, key: crate::page::Key) {
        let (action, thread) = {
            let Some(PageModel::AgentReview(page)) = self.pages.get_mut(&id).map(|s| &mut s.model) else { return };
            (page.key(key), page.thread.clone())
        };
        let file = |app: &App, f: usize| match app.pages.get(&id).map(|s| &s.model) {
            Some(PageModel::AgentReview(p)) => p.files.get(f).cloned(),
            _ => None,
        };
        match action {
            agent_review::Action::None => return,
            agent_review::Action::Close => {
                self.close_page(id);
                return;
            }
            agent_review::Action::Refresh => {}
            agent_review::Action::Keep { file: f, hunk } => {
                if let Some(file) = file(self, f) {
                    self.agents_keep(&thread, &file, Some(hunk));
                }
            }
            agent_review::Action::KeepFile(f) => {
                if let Some(file) = file(self, f) {
                    self.agents_keep(&thread, &file, None);
                }
            }
            agent_review::Action::Reject { file: f, hunk } => {
                if let Some(file) = file(self, f) {
                    self.agents_reject(&thread, &file, Some(hunk));
                }
            }
            agent_review::Action::RejectFile(f) => {
                if let Some(file) = file(self, f) {
                    self.agents_reject(&thread, &file, None);
                }
            }
            agent_review::Action::Open { file: f, line } => {
                if let Some(file) = file(self, f) {
                    self.open_file_from_picker(&file.path);
                    self.jump_to_grep_match(&fenix_project::GrepMatch { path: file.path, line, col: 1, text: String::new() });
                }
                return;
            }
        }
        self.agents_review_refresh(&thread);
    }

    /// The review page, read again, and the tints. A file with nothing
    /// left to review is no longer the thread's: an edit another thread
    /// makes to it later isn't shown as this one's.
    fn agents_review_refresh(&mut self, thread: &ThreadId) {
        let settled: Vec<PathBuf> = self
            .agents
            .baselines
            .get(thread)
            .into_iter()
            .flatten()
            .filter(|(path, before)| {
                let baseline = match before {
                    Some(text) => Baseline::Text(text.clone()),
                    None => Baseline::Absent,
                };
                !ReviewFile::new((*path).clone(), String::new(), baseline, self.agents_current(path)).changed()
            })
            .map(|(path, _)| path.clone())
            .collect();
        if let Some(baselines) = self.agents.baselines.get_mut(thread) {
            for path in settled {
                baselines.remove(&path);
            }
        }
        let files = self.agents_review_files(thread);
        if let Some(id) = self.find_page(|m| matches!(m, PageModel::AgentReview(p) if &p.thread == thread)) {
            if let Some(PageModel::AgentReview(p)) = self.pages.get_mut(&id).map(|s| &mut s.model) {
                p.set_files(files);
            }
            self.restale_page(id);
        }
        self.agents_refresh_marks();
        self.agents_count_to_review();
        self.agents.dirty.insert(thread.clone());
    }

    /// Keeps a hunk, or the whole file: what the agent did there is
    /// accepted, and no longer shown as a change.
    fn agents_keep(&mut self, thread: &ThreadId, file: &ReviewFile, hunk: Option<usize>) {
        let baselines = self.agents.baselines.entry(thread.clone()).or_default();
        match (hunk, &file.baseline, &file.current) {
            (_, Baseline::Unknown, _) => {
                if let Some(unknown) = self.agents.unknown.get_mut(thread) {
                    unknown.remove(&file.path);
                }
            }
            (None, _, _) => {
                baselines.remove(&file.path);
            }
            (Some(h), baseline, Some(current)) => {
                let old = match baseline {
                    Baseline::Text(t) => t.clone(),
                    _ => String::new(),
                };
                if let Some(kept) = agent_review::keep(&old, current, h) {
                    if &kept == current {
                        baselines.remove(&file.path);
                    } else {
                        baselines.insert(file.path.clone(), Some(kept));
                    }
                }
            }
            (Some(_), _, None) => {
                baselines.remove(&file.path);
            }
        }
    }

    /// Rejects a hunk, or the whole file: it goes back to what it was. An
    /// open buffer takes it as one undo step, and is saved; with unsaved
    /// edits of yours in it, you're told to deal with those first.
    fn agents_reject(&mut self, thread: &ThreadId, file: &ReviewFile, hunk: Option<usize>) {
        let open = self.buffers.id_for_path(&file.path);
        if let Some(id) = open.filter(|id| self.buffers.get(*id).is_some_and(|b| b.buffer.is_dirty())) {
            let name = self.buffer_display_name(id);
            self.set_error(format!("{name} has unsaved edits: save them (or :e! them away) before rejecting the agent's"));
            return;
        }
        let target = match (&file.baseline, hunk, &file.current) {
            (Baseline::Unknown, ..) => {
                self.set_error("there's nothing to put this file back to");
                return;
            }
            (Baseline::Text(old), None, _) => Some(old.clone()),
            (Baseline::Absent, None, _) => None,
            (baseline, Some(h), Some(current)) => {
                let old = match baseline {
                    Baseline::Text(t) => t.as_str(),
                    _ => "",
                };
                agent_review::reject(old, current, h).filter(|t| !(matches!(baseline, Baseline::Absent) && t.is_empty()))
            }
            (Baseline::Text(old), Some(_), None) => Some(old.clone()),
            (Baseline::Absent, Some(_), None) => None,
        };
        match target {
            Some(text) => {
                match open {
                    Some(id) => {
                        self.replace_buffer_from_disk(id, &text);
                        if !self.agents_save_buffer(id) {
                            return;
                        }
                    }
                    None => {
                        if let Some(parent) = file.path.parent() {
                            let _ = std::fs::create_dir_all(parent);
                        }
                        if let Err(e) = fenix_storage::write(&file.path, text.as_bytes()) {
                            self.set_error(format!("couldn't put {} back: {e}", file.name));
                            return;
                        }
                    }
                }
                self.set_message(format!("{}: the agent's change rejected", file.name));
            }
            None => {
                // A file the agent made: to the Recycle Bin.
                let outcomes = fenix_fs::to_recycle_bin(std::slice::from_ref(&file.path));
                if outcomes.iter().any(|o| !o.succeeded()) {
                    self.set_error(format!("couldn't remove {}", file.name));
                    return;
                }
                if let Some(id) = open {
                    self.buffers.close(id);
                    let fallback = self.buffers.mru().first().copied().unwrap_or_else(|| self.buffers.open_scratch());
                    self.repoint_panes_showing(id, fallback);
                }
                if let Some(b) = self.agents.baselines.get_mut(thread) {
                    b.remove(&file.path);
                }
                self.set_message(format!("{}: the agent's new file is in the Recycle Bin", file.name));
            }
        }
    }

    /// `:agent-reject-all`, `agents.reject_all`: every change of the
    /// shown thread, rejected.
    pub(crate) fn agents_reject_all(&mut self) {
        let Some(thread) = self.agents.shown(&self.pages).or_else(|| self.agents.hub.focused().cloned()) else { return };
        for file in self.agents_review_files(&thread) {
            if !matches!(file.baseline, Baseline::Unknown) {
                self.agents_reject(&thread, &file, None);
            }
        }
        self.agents_review_refresh(&thread);
    }

    /// `agents.accept_all`: every change of the shown thread, kept.
    pub(crate) fn agents_accept_all(&mut self) {
        let Some(thread) = self.agents.shown(&self.pages).or_else(|| self.agents.hub.focused().cloned()) else { return };
        self.agents.baselines.remove(&thread);
        self.agents.unknown.remove(&thread);
        self.agents_review_refresh(&thread);
        self.set_message("the agent's changes are kept");
    }

    /// The tint on lines agents changed, in every open buffer.
    pub(super) fn agents_refresh_marks(&mut self) {
        let mut marks: HashMap<BufferId, Vec<crate::decorations::Decoration>> = HashMap::new();
        for baselines in self.agents.baselines.values() {
            for (path, before) in baselines {
                let Some(id) = self.buffers.id_for_path(path) else { continue };
                let Some(ob) = self.buffers.get(id) else { continue };
                let now = ob.buffer.text();
                let old = before.clone().unwrap_or_default();
                if old == now {
                    continue;
                }
                let set = marks.entry(id).or_default();
                for hunk in agent_review::hunks(&old, &now) {
                    let last = ob.buffer.line_count();
                    for line in hunk.new.clone().filter(|l| *l < last) {
                        let start = ob.buffer.line_start_char(line);
                        set.push(crate::decorations::Decoration { start, end: start + ob.buffer.line_len(line), look: crate::decorations::Look::Tint(crate::page::Shade::Accent), id: None });
                    }
                }
            }
        }
        // Buffers that had tints and have none now lose them.
        for sets in self.wasm.decorations.values_mut() {
            sets.retain(|(owner, _)| owner != MARKS);
        }
        for (id, set) in marks {
            self.wasm.decorations.entry(id).or_default().push((MARKS.to_string(), set));
        }
    }

    /// A turn started: what git says is changed now, to tell later what
    /// changed during it.
    pub(super) fn agents_turn_started(&mut self, thread: &ThreadId) {
        let Some(root) = self.agents.hub.meta(thread).map(|m| m.root.clone()) else { return };
        let changed: HashSet<String> = fenix_git::list_files(&root).into_iter().map(|f| f.path).collect();
        self.agents.turn_start.insert(thread.clone(), changed);
    }

    /// A turn ended: files git shows changed that weren't before it, and
    /// that no tool call announced, are the agent's too -- compared to
    /// the index when they're tracked, else listed with nothing to
    /// compare them to.
    pub(super) fn agents_turn_ended(&mut self, thread: &ThreadId) {
        let Some(before) = self.agents.turn_start.remove(thread) else { return };
        let Some(root) = self.agents.hub.meta(thread).map(|m| m.root.clone()) else { return };
        for entry in fenix_git::list_files(&root) {
            if before.contains(&entry.path) {
                continue;
            }
            let path = root.join(&entry.path);
            if self.agents.baselines.get(thread).is_some_and(|b| b.contains_key(&path)) {
                continue;
            }
            match entry.index_status {
                // Untracked, or added: it didn't exist before.
                '?' | 'A' => {
                    self.agents.baselines.entry(thread.clone()).or_default().insert(path, None);
                }
                _ => match fenix_git::show_at(&root, "", &entry.path) {
                    Ok(text) => {
                        self.agents.baselines.entry(thread.clone()).or_default().insert(path, Some(text));
                    }
                    Err(_) => {
                        self.agents.unknown.entry(thread.clone()).or_default().insert(path);
                    }
                },
            }
        }
        self.agents_refresh_marks();
        self.agents_count_to_review();
        self.agents.dirty.insert(thread.clone());
    }

    /// Follow mode: the pane beside the panel shows where the agent works.
    pub(super) fn agents_follow(&mut self, thread: &ThreadId) {
        if !self.agents.following {
            return;
        }
        let Some(root) = self.agents.hub.meta(thread).map(|m| m.root.clone()) else { return };
        let Some((path, line)) = self.agents.hub.view(thread).and_then(|v| {
            v.thread.entries.iter().rev().find_map(|e| match e {
                fenix_acp::thread::Entry::Tool(call) => call.locations.first().map(|l| (root.join(&l.path), l.line)),
                _ => None,
            })
        }) else {
            return;
        };
        if self.agents.followed.as_ref() == Some(&(path.clone(), line)) || !within(&root, &path) || !path.is_file() {
            return;
        }
        self.agents.followed = Some((path.clone(), line));
        let Some(pane) = self.agents.code_pane.filter(|p| self.windows().windows().contains(p)) else { return };
        let back = self.focused_pane_id();
        self.windows_mut().focus(pane);
        self.open_file_from_picker(&path);
        if let Some(line) = line {
            self.jump_to_grep_match(&fenix_project::GrepMatch { path, line: line.max(1) as usize, col: 1, text: String::new() });
        }
        if self.windows().windows().contains(&back) {
            self.windows_mut().focus(back);
        }
    }

    // -----------------------------------------------------------------
    // In the code: the agent's hunk under the cursor, like a git hunk.

    /// The agent's change under the cursor in the focused file: the thread
    /// it's from, the file as the review sees it, and which hunk. For a
    /// deletion, the line after it.
    fn agents_hunk_here(&mut self, quiet: bool) -> Option<(ThreadId, ReviewFile, usize)> {
        let path = self.open().buffer.path().map(Path::to_path_buf);
        let (line, _) = self.open().buffer.line_col(&self.cursor());
        let found = path.and_then(|path| {
            let threads: Vec<ThreadId> = self.agents.baselines.iter().filter(|(_, b)| b.contains_key(&path)).map(|(t, _)| t.clone()).collect();
            threads.into_iter().find_map(|thread| {
                let file = self.agents_review_files(&thread).into_iter().find(|f| f.path == path)?;
                let hunk = file.hunks.iter().position(|h| {
                    let end = if h.new.is_empty() { h.new.start + 1 } else { h.new.end };
                    line >= h.new.start && line < end
                })?;
                Some((thread, file, hunk))
            })
        });
        if found.is_none() && !quiet {
            self.set_message("the cursor isn't on an agent's change -- ]a goes to the next one");
        }
        found
    }

    /// `SPC a y`: keeps the agent's change under the cursor.
    pub(crate) fn agents_keep_here(&mut self) {
        let Some((thread, file, hunk)) = self.agents_hunk_here(false) else { return };
        self.agents_keep(&thread, &file, Some(hunk));
        self.agents_review_refresh(&thread);
        self.set_message(format!("{}: the agent's change kept", file.name));
    }

    /// `SPC a x`: puts the agent's change under the cursor back.
    pub(crate) fn agents_reject_here(&mut self) {
        let Some((thread, file, hunk)) = self.agents_hunk_here(false) else { return };
        self.agents_reject(&thread, &file, Some(hunk));
        self.agents_review_refresh(&thread);
    }

    /// `SPC a v`: what the agent's change under the cursor replaced, in
    /// the hover popup.
    pub(crate) fn agents_preview_here(&mut self) {
        let Some((thread, file, hunk)) = self.agents_hunk_here(false) else { return };
        let h = &file.hunks[hunk];
        let title = self.agents.hub.view(&thread).map(|v| v.thread.display_title()).unwrap_or_default();
        let mut text = format!("Agent change {} of {} · {title}\n", hunk + 1, file.hunks.len());
        for line in h.removed.iter().take(30) {
            text.push_str(&format!("- {line}\n"));
        }
        for line in h.added.iter().take(30) {
            text.push_str(&format!("+ {line}\n"));
        }
        text.push_str("\nSPC a y keep · SPC a x reject · ]a next");
        self.lsp_hover = Some(text);
        self.wake_caret();
    }

    /// `]a`/`[a`: the `count`th next or previous agent change, in this
    /// file or the next one with any (every thread's, by path).
    pub(crate) fn agents_jump_to_hunk(&mut self, forward: bool, count: u32) {
        let threads: Vec<ThreadId> = self.agents.baselines.keys().cloned().collect();
        let mut stops: Vec<(PathBuf, usize)> = threads
            .iter()
            .flat_map(|t| self.agents_review_files(t))
            .filter(|f| f.current.is_some())
            .flat_map(|f| f.hunks.iter().map(|h| (f.path.clone(), h.new.start)).collect::<Vec<_>>())
            .collect();
        stops.sort();
        stops.dedup();
        let here = (self.open().buffer.path().map(Path::to_path_buf).unwrap_or_default(), self.open().buffer.line_col(&self.cursor()).0);
        let ahead: Vec<&(PathBuf, usize)> = if forward { stops.iter().filter(|s| **s > here).collect() } else { stops.iter().rev().filter(|s| **s < here).collect() };
        let Some((path, line)) = ahead.get((count.max(1) as usize - 1).min(ahead.len().saturating_sub(1))).map(|s| (*s).clone()) else {
            self.set_message(if forward { "no more agent changes after this" } else { "no more agent changes before this" });
            return;
        };
        if self.agents_is_dock_pane(self.focused_pane_id()) {
            self.agents_focus_code();
        }
        self.open_file_from_picker(&path);
        self.jump_to_grep_match(&fenix_project::GrepMatch { path, line: line + 1, col: 1, text: String::new() });
    }

    /// `SPC a f`: follow mode on or off.
    pub(crate) fn agents_toggle_follow(&mut self) {
        self.agents.following = !self.agents.following;
        self.agents.followed = None;
        self.set_message(if self.agents.following { "following the agent: the pane beside it shows where it works" } else { "not following the agent" });
    }
}
