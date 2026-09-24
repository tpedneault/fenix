//! The host half of the Git status page (`git_status`): reading the
//! repository into a snapshot, fetching a file's diff when it's
//! expanded, and running what the page's keys and menus ask for -- all
//! off the UI thread, through the same page machinery the project pages
//! use (`pages.rs`).

use super::pages::{PageEvent, PageModel};
use super::*;
use crate::git_status::{Action, Compose, Confirm, DiffState, GitStatus, Job, Op, PickRef, Section, Snapshot};
use crate::page::Role as PageRole;
use fenix_git::oplog::{self, Undo};
use fenix_git::{ApplyTarget, CommitKind, ResetMode};

/// Commits listed in the Unpushed/Unpulled sections at most.
const COMMIT_LIMIT: usize = 50;
/// Commits in Recent.
const RECENT: usize = 10;

/// Everything the page shows, read in one pass.
fn read_snapshot(root: &Path, base: Option<&str>) -> Snapshot {
    let (status, files) = fenix_git::status_and_files(root);
    let upstream = status.as_ref().and_then(|s| s.upstream.clone());
    let branch = status.as_ref().map(|s| s.branch.clone()).unwrap_or_default();
    let base = fenix_git::resolve_base(root, base).and_then(|b| fenix_git::ahead_behind(root, &b, "HEAD").map(|(ahead, behind)| (b, ahead, behind)));
    let (unpulled, unpushed) = match &upstream {
        Some(upstream) => (fenix_git::commits_between(root, "HEAD", upstream, COMMIT_LIMIT), fenix_git::commits_between(root, upstream, "HEAD", COMMIT_LIMIT)),
        // With nothing pushed yet, what this branch has that its base
        // doesn't is what a first push would send.
        None => match &base {
            Some((b, ..)) if *b != branch && !b.ends_with(&format!("/{branch}")) => (Vec::new(), fenix_git::commits_between(root, b, "HEAD", COMMIT_LIMIT)),
            _ => (Vec::new(), Vec::new()),
        },
    };
    let recent = fenix_git::list_commits(root, RECENT);
    let log = oplog::entries(root, 30);
    let now = now_millis();
    let undone = undone_times(&oplog::entries(root, 200));
    let ops = log
        .into_iter()
        .map(|e| Op { age: now.saturating_sub(e.time) / 1000, undoable: e.undo.possible(), undone: undone.contains(&e.time), ok: e.ok, time: e.time, label: e.label })
        .collect();
    Snapshot {
        ops,
        head_subject: recent.first().map(|c| c.message.clone()),
        push_remote: fenix_git::default_remote(root),
        base,
        fetched_secs_ago: fenix_git::seconds_since_fetch(root),
        files,
        stashes: fenix_git::list_stashes(root),
        unpulled,
        unpushed,
        recent,
        in_progress: fenix_git::in_progress(root).map(|op| op.label()),
        gone: fenix_git::list_branches(root).into_iter().filter(|b| b.upstream_gone && !b.current).map(|b| b.name).collect(),
        status,
    }
}

fn read_diff(root: &Path, section: Section, path: &str) -> DiffState {
    match fenix_git::file_diff(root, path, section == Section::Staged) {
        Ok(text) => match fenix_diff::parse(&text).into_iter().next() {
            Some(diff) if diff.is_binary => DiffState::Empty("a binary file -- no text to show".to_string()),
            Some(diff) if !diff.hunks.is_empty() => DiffState::Loaded(Box::new(diff)),
            _ => DiffState::Empty("no changes left to show".to_string()),
        },
        Err(e) => DiffState::Empty(first_line(&e)),
    }
}

fn now_millis() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// The times of the entries a later "undo #time: ..." entry took back.
fn undone_times(entries: &[oplog::Entry]) -> Vec<u64> {
    entries.iter().filter_map(|e| e.label.strip_prefix("undo #")?.split(':').next()?.parse().ok()).collect()
}

/// What `U` with nothing chosen undoes: the newest operation that
/// worked, can be taken back, and hasn't been.
fn latest_undoable(entries: &[oplog::Entry]) -> Option<&oplog::Entry> {
    let undone = undone_times(entries);
    entries.iter().find(|e| e.ok && e.undo.possible() && !undone.contains(&e.time))
}

/// The question to ask before undoing: what it'll do, from the log.
fn plan_undo(root: &Path, time: Option<u64>) -> Result<Confirm, String> {
    let entries = oplog::entries(root, 200);
    let entry = match time {
        Some(t) => entries.iter().find(|e| e.time == t).ok_or("that operation isn't in the log any more")?,
        None => latest_undoable(&entries).ok_or("nothing Fenix ran here can be undone")?,
    };
    if let Undo::Not(why) = &entry.undo {
        return Err(format!("{} can't be undone: {why}", entry.label));
    }
    if !entry.ok {
        return Err(format!("{} failed, so there's nothing to undo", entry.label));
    }
    let now = now_millis();
    let detail = oplog::preview(root, &entry.undo)
        .into_iter()
        .map(|line| {
            let role = if line.starts_with('+') {
                PageRole::Good
            } else if line.starts_with('-') {
                PageRole::Bad
            } else {
                PageRole::Muted
            };
            (role, line)
        })
        .collect();
    Ok(Confirm {
        question: format!("Undo \"{}\" ({})?", entry.label, crate::git_status::ago(now.saturating_sub(entry.time) / 1000)),
        detail,
        choices: vec![('y', "undo it".to_string(), Job::Undo { time: entry.time, label: entry.label.clone() })],
    })
}

/// Takes an operation back -- logged too, with how to put it back again.
fn run_undo(root: &Path, time: u64) -> Result<String, String> {
    let entry = oplog::entries(root, 200).into_iter().find(|e| e.time == time).ok_or("that operation isn't in the log any more")?;
    let head = oplog::rev(root, "HEAD");
    let redo = match &entry.undo {
        Undo::Reset { .. } | Undo::Restore { .. } => Undo::Reset { to: head, soft: false, saved: None },
        Undo::Switch(_) | Undo::Unbranch { back_to: Some(_), .. } => match oplog::current_branch(root) {
            Some(branch) => Undo::Switch(branch),
            None => Undo::Not("HEAD was detached".to_string()),
        },
        Undo::Rename { from, to } => Undo::Rename { from: to.clone(), to: from.clone() },
        Undo::DeleteBranch(name) => Undo::CreateBranches(vec![(name.clone(), oplog::rev(root, name))]),
        Undo::CreateBranches(branches) if branches.len() == 1 => Undo::DeleteBranch(branches[0].0.clone()),
        _ => Undo::Not("redo it by hand".to_string()),
    };
    let label = format!("undo #{}: {}", entry.time, entry.label);
    oplog::logged(root, &label, || oplog::apply(root, &entry.undo), move |_| redo)
}

/// How to take a job back, given how it went.
type UndoFor = Box<dyn FnOnce(&Result<String, String>) -> Undo>;

/// Runs `job`, logged with how to take it back -- worked out before it
/// runs, from what it's about to change.
pub(super) fn run_logged(root: &Path, job: &Job) -> Result<String, String> {
    if let Job::Undo { time, .. } = job {
        return run_undo(root, *time);
    }
    let head = oplog::rev(root, "HEAD");
    let branch = oplog::current_branch(root);
    let not = |why: &str| -> UndoFor {
        let why = why.to_string();
        Box::new(move |_| Undo::Not(why))
    };
    let fixed = |undo: Undo| -> UndoFor { Box::new(move |_| undo) };
    let undo: UndoFor = match job {
        Job::Stage(_) | Job::Unstage(_) | Job::Apply { target: ApplyTarget::Stage | ApplyTarget::Unstage, .. } => not("stage or unstage it back (s / S)"),
        Job::Apply { patch, target: ApplyTarget::Discard, .. } => fixed(match oplog::save_patch(root, patch) {
            Some(blob) => Undo::ApplyPatch(blob),
            None => Undo::Not("the discarded lines couldn't be saved".to_string()),
        }),
        // Each file as it stands, so bringing it back doesn't depend on
        // what else has changed since. A folder (an untracked one) can't
        // be saved this way, and is said to be gone for good.
        Job::Discard(files) => {
            let blobs: Vec<(String, String)> = files.iter().filter_map(|(p, _)| oplog::save_file(root, p).map(|b| (p.clone(), b))).collect();
            if blobs.is_empty() {
                not("a folder can't be saved before it's deleted")
            } else {
                fixed(Undo::Restore { saved: None, files: blobs })
            }
        }
        // Undoing a commit gives its changes back, staged.
        Job::Commit(..) | Job::FixupNow { .. } | Job::Reset { mode: ResetMode::Soft | ResetMode::Mixed, .. } => fixed(Undo::Reset { to: head, soft: true, saved: None }),
        Job::Reset { mode: ResetMode::Hard, .. } => fixed(Undo::Reset { to: head, soft: false, saved: oplog::save_changes(root) }),
        Job::Pull { .. } | Job::Revert(_) | Job::CherryPick(_) => fixed(Undo::Reset { to: head, soft: false, saved: None }),
        Job::Checkout(_) => fixed(match branch {
            Some(branch) => Undo::Switch(branch),
            None => Undo::Reset { to: head, soft: false, saved: None },
        }),
        Job::Push(_) | Job::PushTags(_) => not("a push can't be taken back from here -- push the old commit with --force-with-lease if nobody has pulled it"),
        Job::Fetch => not("a fetch only updates what Fenix knows about the remote"),
        Job::Branch { name, switch, .. } => fixed(Undo::Unbranch { name: name.clone(), back_to: if *switch { branch } else { None } }),
        Job::Rename { old, new } => fixed(Undo::Rename { from: new.clone(), to: old.clone() }),
        Job::DeleteBranches(names) => fixed(Undo::CreateBranches(names.iter().map(|n| (n.clone(), oplog::rev(root, n))).collect())),
        Job::Tag { name, .. } => fixed(Undo::DeleteTag(name.clone())),
        Job::Stash(_) => {
            let root = root.to_path_buf();
            Box::new(move |result| match result {
                Ok(_) => Undo::PopStash(oplog::rev(&root, "stash@{0}")),
                Err(_) => Undo::Not("nothing was stashed".to_string()),
            })
        }
        Job::StashDrop(i) => {
            let message = fenix_git::list_stashes(root).into_iter().find(|s| s.index == *i).map(|s| s.message).unwrap_or_default();
            fixed(Undo::StoreStash { commit: oplog::rev(root, &format!("stash@{{{i}}}")), message })
        }
        Job::StashApply(_) | Job::StashPop(_) => not("the stash went into your files -- discard them to take it back"),
        Job::Continue | Job::Abort | Job::Skip => not("a rebase or merge step -- undo the whole rebase or merge instead"),
        Job::Undo { .. } => unreachable!("handled above"),
    };
    oplog::logged(root, &job.label(), || run_job(root, job), undo)
}

/// The line of git's output worth showing: the first that isn't a hint.
fn first_line(text: &str) -> String {
    text.lines().map(str::trim).find(|l| !l.is_empty() && !l.starts_with("hint:")).unwrap_or("").to_string()
}

/// Runs one job, returning what git printed.
fn run_job(root: &Path, job: &Job) -> Result<String, String> {
    let each = |items: &[String], f: &dyn Fn(&str) -> Result<String, String>| -> Result<String, String> {
        let mut out = String::new();
        for item in items {
            out.push_str(&f(item)?);
        }
        Ok(out)
    };
    match job {
        Job::Stage(paths) => each(paths, &|p| fenix_git::stage_file(root, p)),
        Job::Unstage(paths) => each(paths, &|p| fenix_git::unstage_file(root, p)),
        Job::Discard(files) => {
            let mut out = String::new();
            for (path, untracked) in files {
                out.push_str(&fenix_git::discard_file(root, path, *untracked)?);
            }
            Ok(out)
        }
        Job::Apply { patch, target, .. } => fenix_git::apply_patch(root, patch, *target),
        Job::Commit(kind, flags) => fenix_git::commit_with(root, kind, *flags),
        Job::FixupNow { hash, flags } => {
            let mut out = fenix_git::commit_with(root, &CommitKind::Fixup(hash.clone()), *flags)?;
            out.push_str(&fenix_git::autosquash(root, hash)?);
            Ok(out)
        }
        Job::Push(options) => fenix_git::push_with(root, options),
        Job::PushTags(remote) => fenix_git::push_tags(root, remote),
        Job::Pull { rebase: true } => fenix_git::pull_rebase(root),
        Job::Pull { rebase: false } => fenix_git::pull_merge(root),
        Job::Fetch => fenix_git::fetch(root),
        Job::Branch { name, start, switch } => fenix_git::create_branch_at(root, name, start.as_deref(), *switch),
        Job::Rename { old, new } => fenix_git::rename_branch(root, old, new),
        // Force: a branch whose upstream is gone was usually merged by
        // squash, which `-d` can't see -- and the page asked first.
        Job::DeleteBranches(names) => each(names, &|n| fenix_git::delete_branch(root, n, true)),
        Job::Tag { name, at } => fenix_git::tag(root, name, at),
        Job::Stash(options) => fenix_git::stash_with(root, options),
        Job::StashApply(i) => fenix_git::stash_apply(root, *i),
        Job::StashPop(i) => fenix_git::stash_pop(root, *i),
        Job::StashDrop(i) => fenix_git::stash_drop(root, *i),
        Job::Revert(hash) => fenix_git::revert(root, hash),
        Job::CherryPick(hash) => fenix_git::cherry_pick(root, hash),
        Job::Checkout(hash) => fenix_git::checkout_detached(root, hash),
        Job::Reset { target, mode } => fenix_git::reset(root, target, *mode),
        Job::Continue => match fenix_git::in_progress(root) {
            Some(fenix_git::InProgress::Rebase { .. }) => fenix_git::rebase_continue(root),
            Some(fenix_git::InProgress::Merge) => fenix_git::commit(root, "Merge"),
            Some(op) => Err(format!("{} can't be continued from here -- finish it with git", op.label())),
            None => Err("nothing in progress to continue".to_string()),
        },
        Job::Abort => match fenix_git::in_progress(root) {
            Some(fenix_git::InProgress::Rebase { .. }) => fenix_git::rebase_abort(root),
            Some(fenix_git::InProgress::Merge) => fenix_git::merge_abort(root),
            Some(op) => Err(format!("{} can't be aborted from here -- finish it with git", op.label())),
            None => Err("nothing in progress to abort".to_string()),
        },
        Job::Skip => fenix_git::rebase_skip(root),
        Job::Undo { time, .. } => run_undo(root, *time),
    }
}

impl App {
    /// `SPC g g`: the Git status page, or the seven-pane panel when
    /// `[git] layout = panes`.
    pub(crate) fn open_git(&mut self) {
        if self.config.git_layout.as_deref() == Some("panes") {
            self.open_git_panel();
        } else {
            self.open_git_status();
        }
    }

    /// Opens the status page for the repository the focused buffer is
    /// in -- the one already open for it, or a new one.
    pub(crate) fn open_git_status(&mut self) {
        // The focused file's own folder first: the project root can be a
        // workspace's, or a relative fallback, neither of which says which
        // repository this file is in.
        let start = match self.open().buffer.path() {
            Some(path) if self.focused_git_page_root().is_none() => path.parent().map(Path::to_path_buf).unwrap_or_else(|| self.git_action_repo_root()),
            _ => self.git_action_repo_root(),
        };
        let start = std::fs::canonicalize(&start).map(fenix_lsp::normalize).unwrap_or(start);
        let Some(root) = fenix_project::vcs::repository_root(&start) else {
            self.set_error(format!("{} isn't in a git repository", readable_path(&start)));
            return;
        };
        let root = fenix_lsp::normalize(root);
        let id = match self.find_page(|m| matches!(m, PageModel::Git(g) if g.root == root)) {
            Some(id) => {
                self.show_page(id);
                id
            }
            None => self.open_page(PageModel::Git(Box::new(GitStatus::new(root)))),
        };
        self.git_page_refresh(id);
    }

    /// `SPC g z`: the status page, open on its operation log.
    pub(crate) fn open_git_operations(&mut self) {
        self.open_git_status();
        let id = self.focused_buffer_id();
        if let Some(g) = self.git_page(id) {
            if g.snap.is_some() && !g.loading {
                g.show_operations();
            } else {
                g.reveal_operations = true;
            }
        }
    }

    fn git_page(&mut self, id: BufferId) -> Option<&mut GitStatus> {
        match self.pages.get_mut(&id).map(|s| {
            s.stale = true;
            &mut s.model
        }) {
            Some(PageModel::Git(g)) => Some(g),
            _ => None,
        }
    }

    /// The repository of the Git page in the focused pane, if that's
    /// what's focused -- what every Git action targets from there.
    pub(super) fn focused_git_page_root(&self) -> Option<PathBuf> {
        match self.pages.get(&self.focused_buffer_id()).map(|s| &s.model) {
            Some(PageModel::Git(g)) => Some(g.root.clone()),
            _ => None,
        }
    }

    /// Reads the repository again for page `id`, and the diffs it has
    /// open.
    pub(super) fn git_page_refresh(&mut self, id: BufferId) {
        let base = self.config.git_base_branch.clone();
        let Some(g) = self.git_page(id) else { return };
        g.loading = true;
        let root = g.root.clone();
        let expanded = g.expanded_files();
        self.page_spawn(move |send| {
            let snapshot = read_snapshot(&root, base.as_deref());
            send(PageEvent::GitSnapshot { buffer: id, snapshot: Box::new(snapshot) });
            for (section, path) in expanded {
                let diff = read_diff(&root, section, &path);
                send(PageEvent::GitDiff { buffer: id, section, path, diff });
            }
        });
    }

    /// Refreshes every Git page -- after an operation (`timed` false),
    /// or on the disk poll's tick, when only the visible ones that
    /// aren't already reading or running something are.
    pub(super) fn refresh_git_pages(&mut self, timed: bool) {
        if timed {
            self.refresh_stale_blames();
        } else {
            self.refresh_git_logs();
        }
        let visible: Vec<BufferId> = self.windows().windows().iter().filter_map(|p| self.windows().content(*p).copied()).collect();
        let ids: Vec<BufferId> = self
            .pages
            .iter()
            .filter_map(|(id, s)| match &s.model {
                PageModel::Git(g) if !timed || (visible.contains(id) && !g.loading && g.busy.is_none()) => Some(*id),
                _ => None,
            })
            .collect();
        for id in ids {
            self.git_page_refresh(id);
        }
    }

    pub(super) fn git_page_action(&mut self, id: BufferId, action: Action) {
        let Some(root) = self.git_page(id).map(|g| g.root.clone()) else { return };
        match action {
            Action::None => {}
            Action::Refresh => self.git_page_refresh(id),
            Action::LoadDiff { section, path } => {
                self.page_spawn(move |send| {
                    let diff = read_diff(&root, section, &path);
                    send(PageEvent::GitDiff { buffer: id, section, path, diff });
                });
            }
            Action::Close => self.close_page(id),
            Action::OpenFile { path, line } => {
                let full = root.join(&path);
                if !full.is_file() {
                    self.set_error(format!("{path} isn't a file on disk"));
                    return;
                }
                self.open_file_from_picker(&full);
                if let Some(line) = line {
                    self.jump_to_grep_match(&fenix_project::GrepMatch { path: full, line, col: 1, text: String::new() });
                }
            }
            Action::Run(job) => self.git_page_run(id, job),
            Action::Compose(compose) => {
                let seed = match compose {
                    Compose::New(_) => String::new(),
                    Compose::Amend(_) | Compose::Reword(_) => fenix_git::commit_message(&root, "HEAD").unwrap_or_default(),
                };
                self.open_compose_seeded(ComposePurpose::GitCommit { repo_root: root, compose }, &seed);
            }
            Action::Pick(PickRef::Switch) => self.start_switch_picker(),
            Action::Pick(PickRef::Merge) => self.start_merge_picker(),
            Action::Pick(PickRef::Rebase) => self.start_rebase_picker(),
            Action::OpenHistory => self.open_history_view(),
            Action::OpenCompare => self.start_compare_picker(),
            Action::OpenMergeView => self.open_merge_view(),
            Action::Copy(text) => {
                if let Some(clipboard) = &mut self.clipboard {
                    let _ = clipboard.set_text(text.clone());
                }
                if let Some(g) = self.git_page(id) {
                    g.message = Some((format!("copied {}", crate::page::fit(&text, 60)), false));
                }
            }
            Action::PlanUndo(time) => {
                self.page_spawn(move |send| {
                    let confirm = plan_undo(&root, time);
                    send(PageEvent::GitConfirm { buffer: id, confirm });
                });
            }
            Action::ShowOutput => {
                let Some(g) = self.git_page(id) else { return };
                let text = if g.output.is_empty() { "(nothing has run yet)".to_string() } else { g.output.join("\n") };
                let buffer = self.buffers.open_text_view(&text);
                self.open_buffer_in_focused_pane(buffer);
            }
        }
    }

    /// Runs `job` for page `id` off the UI thread; one at a time.
    fn git_page_run(&mut self, id: BufferId, job: Job) {
        let Some(g) = self.git_page(id) else { return };
        if let Some(busy) = &g.busy {
            g.message = Some((format!("still running {busy} -- wait for it to finish"), true));
            return;
        }
        let label = job.label();
        g.busy = Some(label.clone());
        g.message = None;
        let root = g.root.clone();
        self.page_spawn(move |send| {
            let result = run_logged(&root, &job);
            send(PageEvent::GitDone { buffer: id, label, result });
        });
    }

    /// A message written in the compose buffer, committed the way the
    /// page's commit menu asked.
    pub(super) fn git_page_commit(&mut self, root: PathBuf, compose: Compose, body: String) {
        let (kind, flags) = match compose {
            Compose::New(flags) => (CommitKind::New(body.clone()), flags),
            Compose::Amend(flags) => (CommitKind::Amend(body.clone()), flags),
            Compose::Reword(flags) => (CommitKind::Reword(body.clone()), flags),
        };
        let head = oplog::rev(&root, "HEAD");
        let label = match &kind {
            CommitKind::Amend(_) => "amend",
            CommitKind::Reword(_) => "reword",
            _ => "commit",
        };
        let result = oplog::logged(&root, &format!("{label}: {}", body.lines().next().unwrap_or_default()), || fenix_git::commit_with(&root, &kind, flags), move |_| {
            Undo::Reset { to: head, soft: true, saved: None }
        });
        match result {
            Ok(_) => {
                self.close_compose();
                let summary = body.lines().next().unwrap_or_default().to_string();
                let verb = match kind {
                    CommitKind::Amend(_) => "amended",
                    CommitKind::Reword(_) => "reworded",
                    _ => "committed",
                };
                self.set_message(format!("{verb}: {summary}"));
                self.git_refresh_all_views();
            }
            // The draft stays: a hook that rejected the message is fixed
            // in the message.
            Err(err) => self.set_error(format!("commit failed: {} -- the message is still here", first_line(&err))),
        }
    }

    pub(super) fn apply_git_page_event(&mut self, event: PageEvent) {
        match event {
            PageEvent::GitSnapshot { buffer, snapshot } => {
                if let Some(g) = self.git_page(buffer) {
                    g.set_snapshot(*snapshot);
                }
            }
            PageEvent::GitDiff { buffer, section, path, diff } => {
                if let Some(g) = self.git_page(buffer) {
                    g.set_diff(section, path, diff);
                }
            }
            PageEvent::GitDone { buffer, label, result } => {
                let Some(g) = self.git_page(buffer) else { return };
                g.busy = None;
                let stopped = fenix_git::in_progress(&g.root).map(|op| op.label());
                let (text, failed) = match &result {
                    Ok(out) => (out.clone(), false),
                    Err(err) => (err.clone(), true),
                };
                g.output = std::iter::once(format!("$ {label}")).chain(text.lines().map(str::to_string)).collect();
                g.message = Some(match (failed, stopped) {
                    (true, Some(op)) => (format!("{label} stopped: {op} -- resolve the conflicts (r x), then r c"), true),
                    (true, None) => (format!("{label} failed: {}  ($ shows everything)", first_line(&text)), true),
                    (false, _) => (format!("{label} ✓"), false),
                });
                self.git_refresh_all_views();
            }
            PageEvent::GitConfirm { buffer, confirm } => {
                let Some(g) = self.git_page(buffer) else { return };
                match confirm {
                    Ok(confirm) => g.confirm = Some(confirm),
                    Err(why) => g.message = Some((why, true)),
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git_status::Row;
    use std::process::Command;

    /// A repository with one commit on `main` and a bare `origin`,
    /// removed when dropped.
    struct Repo {
        dir: PathBuf,
        remote: PathBuf,
    }

    impl Repo {
        fn new(name: &str) -> Self {
            let base = std::env::temp_dir().join(format!("fenix-git-page-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&base);
            let (dir, remote) = (base.join("work"), base.join("origin.git"));
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::create_dir_all(&remote).unwrap();
            let dir = fenix_lsp::normalize(std::fs::canonicalize(&dir).unwrap());
            let repo = Repo { dir, remote };
            git(&repo.remote, &["init", "-q", "--bare"]);
            git(&repo.dir, &["init", "-q", "-b", "main"]);
            git(&repo.dir, &["config", "user.email", "t@example.com"]);
            git(&repo.dir, &["config", "user.name", "T"]);
            git(&repo.dir, &["config", "core.autocrlf", "false"]);
            git(&repo.dir, &["remote", "add", "origin", repo.remote.to_str().unwrap()]);
            repo.write("a.txt", "one\ntwo\nthree\n");
            git(&repo.dir, &["add", "."]);
            git(&repo.dir, &["commit", "-q", "-m", "first"]);
            repo
        }

        fn write(&self, name: &str, text: &str) -> PathBuf {
            let path = self.dir.join(name);
            std::fs::write(&path, text).unwrap();
            path
        }

        fn git(&self, args: &[&str]) -> String {
            git(&self.dir, args)
        }
    }

    impl Drop for Repo {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(self.dir.parent().unwrap());
        }
    }

    fn git(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git").current_dir(dir).args(args).output().unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).trim_end().to_string()
    }

    /// An app with a file of `repo` open and its status page showing.
    fn app_on(repo: &Repo, file: &str) -> App {
        let mut app = App::with_file(Some(repo.dir.join(file).to_string_lossy().into_owned()));
        app.open_git_status();
        app
    }

    fn page_mut(app: &mut App) -> &mut GitStatus {
        let id = app.focused_buffer_id();
        match app.pages.get_mut(&id).map(|s| &mut s.model) {
            Some(PageModel::Git(g)) => g,
            _ => panic!("the git page isn't focused"),
        }
    }

    fn press(app: &mut App, keys: &str) {
        for c in keys.chars() {
            let key = match c {
                '\n' => KeyPress::named(FenixNamedKey::Enter),
                '\t' => KeyPress::named(FenixNamedKey::Tab),
                c => KeyPress::char(c),
            };
            assert!(app.page_key(key), "the page claims {c:?}");
        }
    }

    fn text(app: &mut App) -> String {
        let (id, pane) = (app.focused_buffer_id(), app.focused_pane_id());
        app.ensure_page_layout(id, pane, 120);
        app.open().buffer.text()
    }

    /// Puts the page's cursor on the first row matching `is`.
    fn goto(app: &mut App, is: impl Fn(&Row) -> bool) {
        let g = page_mut(app);
        g.cursor = g.rows().iter().position(is).expect("no such row");
    }

    #[test]
    fn the_page_opens_on_the_repository_and_stages_a_file() {
        let repo = Repo::new("stage");
        repo.write("a.txt", "one\nTWO\nthree\n");
        let mut app = app_on(&repo, "a.txt");
        let shown = text(&mut app);
        assert!(shown.contains("Head    main  first") && shown.contains("UNSTAGED 1") && shown.contains("a.txt"), "{shown}");
        assert!(shown.contains("origin/main") && shown.contains("not pushed yet"), "{shown}");
        goto(&mut app, |r| matches!(r, Row::File { path, .. } if path == "a.txt"));
        press(&mut app, "s");
        assert_eq!(repo.git(&["diff", "--cached", "--name-only"]), "a.txt");
        let shown = text(&mut app);
        assert!(shown.contains("STAGED 1") && !shown.contains("UNSTAGED"), "the page read the tree again:\n{shown}");
        assert!(shown.contains("stage a.txt ✓"), "{shown}");
    }

    #[test]
    fn lines_selected_in_an_inline_diff_are_staged_alone() {
        let repo = Repo::new("lines");
        repo.write("a.txt", "one\nTWO\nextra\nthree\n");
        let mut app = app_on(&repo, "a.txt");
        goto(&mut app, |r| matches!(r, Row::File { .. }));
        press(&mut app, "\t");
        assert!(text(&mut app).contains("+extra"), "the diff opened inline");
        goto(&mut app, |r| matches!(r, Row::Line { line: 3, .. }));
        press(&mut app, "s");
        assert_eq!(repo.git(&["show", ":a.txt"]), "one\ntwo\nextra\nthree", "only +extra went in");
        assert!(text(&mut app).contains("stage 1 line of a.txt ✓"));
    }

    #[test]
    fn c_c_commits_what_the_compose_buffer_says() {
        let repo = Repo::new("commit");
        repo.write("b.txt", "new\n");
        repo.git(&["add", "b.txt"]);
        let mut app = app_on(&repo, "a.txt");
        press(&mut app, "cc");
        let compose = app.compose.as_ref().expect("the message editor opened").buffer;
        if let Some(ob) = app.buffers.get_mut(compose) {
            let mut c = Cursor::at_start();
            ob.buffer.replace_range(&mut c, 0, 0, "Add b\n\nBecause.");
        }
        app.compose_submit();
        assert_eq!(repo.git(&["log", "-1", "--format=%B"]), "Add b\n\nBecause.");
        assert!(app.compose.is_none());
        assert!(!text(&mut app).contains("STAGED"), "the page caught up");
    }

    #[test]
    fn amend_starts_from_the_last_message() {
        let repo = Repo::new("amend");
        let mut app = app_on(&repo, "a.txt");
        press(&mut app, "ca");
        let compose = app.compose.as_ref().unwrap().buffer;
        assert_eq!(app.buffers.get(compose).unwrap().buffer.text(), "first");
        assert!(app.compose.as_ref().unwrap().purpose.title().starts_with("Amend"));
    }

    #[test]
    fn the_first_push_sets_the_upstream() {
        let repo = Repo::new("push");
        let mut app = app_on(&repo, "a.txt");
        press(&mut app, "Pp");
        assert_eq!(repo.git(&["rev-parse", "--abbrev-ref", "main@{upstream}"]), "origin/main");
        let shown = text(&mut app);
        assert!(shown.contains("push to origin/main ✓") && shown.contains("origin/main ↑0 ↓0"), "{shown}");
    }

    #[test]
    fn a_fixup_from_the_commit_menu_folds_into_that_commit() {
        let repo = Repo::new("fixup");
        repo.write("b.txt", "b\n");
        repo.git(&["add", "."]);
        repo.git(&["commit", "-q", "-m", "second"]);
        repo.write("c.txt", "c\n");
        repo.git(&["add", "."]);
        repo.git(&["commit", "-q", "-m", "third"]);
        repo.write("b.txt", "b, fixed\n");
        repo.git(&["add", "b.txt"]);
        let mut app = app_on(&repo, "a.txt");
        page_mut(&mut app).folded.clear();
        let second = repo.git(&["rev-parse", "HEAD~1"]);
        goto(&mut app, |r| matches!(r, Row::Commit { hash, section: Section::Recent } if *hash == second));
        press(&mut app, "\nf");
        assert_eq!(repo.git(&["log", "--format=%s"]), "third\nsecond\nfirst");
        assert_eq!(repo.git(&["show", "HEAD~1:b.txt"]), "b, fixed");
    }

    #[test]
    fn switching_to_a_remote_branch_makes_a_local_one() {
        let repo = Repo::new("switch");
        repo.git(&["push", "-q", "origin", "main:review"]);
        repo.git(&["fetch", "-q", "origin"]);
        let mut app = app_on(&repo, "a.txt");
        app.git_switch_to("origin/review");
        assert_eq!(repo.git(&["rev-parse", "--abbrev-ref", "HEAD"]), "review");
        assert_eq!(repo.git(&["rev-parse", "--abbrev-ref", "review@{upstream}"]), "origin/review");
        assert!(text(&mut app).contains("Head    review"));
    }

    #[test]
    fn a_hard_reset_is_undone_with_u_and_the_undo_is_undone_again() {
        let repo = Repo::new("undo");
        repo.write("b.txt", "b\n");
        repo.git(&["add", "."]);
        repo.git(&["commit", "-q", "-m", "second"]);
        repo.write("a.txt", "one\nedited\nthree\n");
        let second = repo.git(&["rev-parse", "HEAD"]);
        let first = repo.git(&["rev-parse", "HEAD~1"]);
        let mut app = app_on(&repo, "a.txt");
        page_mut(&mut app).folded.clear();
        goto(&mut app, |r| matches!(r, Row::Commit { hash, section: Section::Recent } if *hash == first));
        press(&mut app, "\nrhy");
        assert_eq!(repo.git(&["rev-parse", "HEAD"]), first, "reset --hard ran");
        assert_eq!(std::fs::read_to_string(repo.dir.join("a.txt")).unwrap(), "one\ntwo\nthree\n");

        press(&mut app, "U");
        let shown = text(&mut app);
        assert!(shown.contains("Undo \"reset --hard to") && shown.contains("brings back 1 commit") && shown.contains("uncommitted changes"), "{shown}");
        press(&mut app, "y");
        assert_eq!(repo.git(&["rev-parse", "HEAD"]), second, "the commit is back");
        assert_eq!(std::fs::read_to_string(repo.dir.join("a.txt")).unwrap(), "one\nedited\nthree\n", "and the edit");

        app.open_git_operations();
        let shown = text(&mut app);
        assert!(shown.contains("OPERATIONS") && shown.contains("undo #") && shown.contains("undone"), "{shown}");
    }

    #[test]
    fn undoing_a_commit_leaves_its_changes_staged() {
        let repo = Repo::new("undo_commit");
        repo.write("b.txt", "b\n");
        repo.git(&["add", "b.txt"]);
        let before = repo.git(&["rev-parse", "HEAD"]);
        let mut app = app_on(&repo, "a.txt");
        press(&mut app, "cc");
        let compose = app.compose.as_ref().unwrap().buffer;
        if let Some(ob) = app.buffers.get_mut(compose) {
            let mut c = Cursor::at_start();
            ob.buffer.replace_range(&mut c, 0, 0, "Add b");
        }
        app.compose_submit();
        assert_ne!(repo.git(&["rev-parse", "HEAD"]), before);
        press(&mut app, "Uy");
        assert_eq!(repo.git(&["rev-parse", "HEAD"]), before);
        assert_eq!(repo.git(&["diff", "--cached", "--name-only"]), "b.txt");
    }

    #[test]
    fn a_push_says_why_it_cannot_be_undone() {
        let repo = Repo::new("undo_push");
        let mut app = app_on(&repo, "a.txt");
        press(&mut app, "Pp");
        app.open_git_operations();
        goto(&mut app, |r| matches!(r, Row::Op(_)));
        press(&mut app, "U");
        assert!(page_mut(&mut app).message.as_ref().unwrap().0.contains("can't be taken back"));
    }

    #[test]
    fn layout_panes_keeps_the_old_panel() {
        let repo = Repo::new("panes");
        let mut app = App::with_file(Some(repo.dir.join("a.txt").to_string_lossy().into_owned()));
        app.config.git_layout = Some("panes".into());
        app.open_git();
        assert!(app.git_session.is_some());
        assert!(!app.pages.values().any(|s| matches!(s.model, PageModel::Git(_))));
    }

    #[test]
    fn opening_it_again_reuses_the_page() {
        let repo = Repo::new("reuse");
        let mut app = app_on(&repo, "a.txt");
        let first = app.focused_buffer_id();
        app.open_file_from_picker(&repo.dir.join("a.txt"));
        app.open_git_status();
        assert_eq!(app.focused_buffer_id(), first);
        assert_eq!(app.pages.values().filter(|s| matches!(s.model, PageModel::Git(_))).count(), 1);
    }
}
