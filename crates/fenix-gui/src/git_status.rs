//! The Git status page (`SPC g g`): one page buffer instead of the
//! seven-pane panel. A header says where the branch stands against its
//! upstream and its base; sections -- conflicted, untracked, unstaged,
//! staged, stashes, unpulled, unpushed, recent -- fold with `Tab`, and a
//! file expands its diff inline, so staging a hunk or a few lines never
//! means moving to another pane.
//!
//! Every verb with more than one form opens a menu: the first key names
//! the thing (`c` commit, `P` push, `b` branch ...), the menu shows what
//! can be done with it and its flags, the second key does it. The menus
//! open under the row you're on.
//!
//! Like the project pages this module only lays out and decides; the
//! host (`app/git_page.rs`) reads the repository and runs what an
//! `Action` asks for, off the UI thread.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use fenix_diff::{FileDiff, LineKind};
use fenix_forge::{Check, PipelineStatus};
use fenix_git::{ApplyTarget, Commit, CommitFlags, CommitKind, FileEntry, PushOptions, RepoStatus, ResetMode, Stash, StashOptions};

use crate::page::{fit, frame, Grid, Key, Page, Popup, Role};

/// Everything the page shows, read in one go off the UI thread.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub status: Option<RepoStatus>,
    /// The last commit's subject, for the header.
    pub head_subject: Option<String>,
    /// Where a branch with no upstream would be pushed.
    pub push_remote: Option<String>,
    /// The base branch and how far this one is (ahead, behind) of it.
    pub base: Option<(String, usize, usize)>,
    pub fetched_secs_ago: Option<u64>,
    pub files: Vec<FileEntry>,
    pub stashes: Vec<Stash>,
    /// On the upstream, not here.
    pub unpulled: Vec<Commit>,
    /// Here, not on the upstream (or, with no upstream, not on the base).
    pub unpushed: Vec<Commit>,
    pub recent: Vec<Commit>,
    /// The suspended operation's banner, `REBASING 3/7`.
    pub in_progress: Option<String>,
    /// Local branches whose upstream was deleted.
    pub gone: Vec<String>,
    /// The operation log, newest first.
    pub ops: Vec<Op>,
    /// The repository's worktrees -- listed when there's more than one.
    pub worktrees: Vec<fenix_git::Worktree>,
}

/// The branch's pull request, as the header shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestLine {
    pub number: u64,
    pub reference: String,
    pub title: String,
    pub draft: bool,
    pub checks: Option<PipelineStatus>,
    pub approved_by: Vec<String>,
    /// Review threads nobody has resolved yet.
    pub unresolved: usize,
}

/// All of a request's checks as one: failed if any did, running while
/// any is, passed once they all have.
pub fn overall(checks: &[Check]) -> Option<PipelineStatus> {
    if checks.is_empty() {
        return None;
    }
    let any = |f: &dyn Fn(&PipelineStatus) -> bool| checks.iter().any(|c| f(&c.status));
    Some(if any(&|s| s.is_bad()) {
        PipelineStatus::Failed
    } else if any(&|s| matches!(s, PipelineStatus::Running | PipelineStatus::Pending)) {
        PipelineStatus::Running
    } else if any(&|s| *s == PipelineStatus::Manual) {
        PipelineStatus::Manual
    } else {
        PipelineStatus::Success
    })
}

/// One entry of the operation log, as the page lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Op {
    /// The log's id for it: milliseconds since the epoch.
    pub time: u64,
    pub label: String,
    pub ok: bool,
    /// Whether it can be taken back from here.
    pub undoable: bool,
    /// Whether a later entry already took it back.
    pub undone: bool,
    /// Seconds ago, when it was read.
    pub age: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Section {
    Conflicted,
    Untracked,
    Unstaged,
    Staged,
    Stashes,
    Unpulled,
    Unpushed,
    Recent,
    Worktrees,
    Operations,
}

impl Section {
    fn title(self) -> &'static str {
        match self {
            Section::Conflicted => "Conflicted",
            Section::Untracked => "Untracked",
            Section::Unstaged => "Unstaged",
            Section::Staged => "Staged",
            Section::Stashes => "Stashes",
            Section::Unpulled => "Unpulled",
            Section::Unpushed => "Unpushed",
            Section::Recent => "Recent commits",
            Section::Worktrees => "Worktrees",
            Section::Operations => "Operations",
        }
    }

    /// Sections whose files have a diff to expand.
    pub fn has_diffs(self) -> bool {
        matches!(self, Section::Unstaged | Section::Staged)
    }
}

fn unchanged(c: char) -> bool {
    c == '.' || c == ' '
}

fn conflicted(f: &FileEntry) -> bool {
    f.index_status == 'U' || f.worktree_status == 'U' || matches!((f.index_status, f.worktree_status), ('A', 'A') | ('D', 'D'))
}

/// The files of `section`, with the status letter to show.
pub fn files_in(snap: &Snapshot, section: Section) -> Vec<(char, String)> {
    snap.files
        .iter()
        .filter_map(|f| {
            let untracked = f.index_status == '?';
            let letter = match section {
                Section::Conflicted => conflicted(f).then_some('C'),
                _ if conflicted(f) => None,
                Section::Untracked => untracked.then_some('?'),
                Section::Unstaged => (!untracked && !unchanged(f.worktree_status)).then_some(f.worktree_status),
                Section::Staged => (!untracked && !unchanged(f.index_status)).then_some(f.index_status),
                _ => None,
            }?;
            Some((letter, f.path.clone()))
        })
        .collect()
}

/// One line of the page the cursor can sit on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row {
    Section(Section),
    File { section: Section, path: String },
    Hunk { section: Section, path: String, hunk: usize },
    Line { section: Section, path: String, hunk: usize, line: usize },
    Stash(usize),
    Commit { section: Section, hash: String },
    /// An operation-log entry, by its time.
    Op(u64),
    Worktree(PathBuf),
}

impl Row {
    fn section(&self) -> Option<Section> {
        match self {
            Row::Section(s) | Row::File { section: s, .. } | Row::Hunk { section: s, .. } | Row::Line { section: s, .. } | Row::Commit { section: s, .. } => {
                Some(*s)
            }
            Row::Stash(_) => Some(Section::Stashes),
            Row::Op(_) => Some(Section::Operations),
            Row::Worktree(_) => Some(Section::Worktrees),
        }
    }

    fn path(&self) -> Option<&str> {
        match self {
            Row::File { path, .. } | Row::Hunk { path, .. } | Row::Line { path, .. } => Some(path),
            _ => None,
        }
    }
}

/// A diff being shown under its file.
#[derive(Debug, Clone)]
pub enum DiffState {
    Loading,
    Loaded(Box<FileDiff>),
    /// Nothing to show (binary, or the change went away).
    Empty(String),
}

/// A git operation for the host to run off the UI thread.
#[derive(Debug, Clone, PartialEq)]
pub enum Job {
    Stage(Vec<String>),
    Unstage(Vec<String>),
    /// `(path, untracked)` pairs: tracked files are checked out,
    /// untracked ones cleaned.
    Discard(Vec<(String, bool)>),
    Apply { patch: String, target: ApplyTarget, what: String },
    Commit(CommitKind, CommitFlags),
    /// A `fixup!` commit for the hash, then the autosquash rebase.
    FixupNow { hash: String, flags: CommitFlags },
    Push(PushOptions),
    PushTags(String),
    Pull { rebase: bool },
    Fetch,
    Branch { name: String, start: Option<String>, switch: bool },
    Rename { old: String, new: String },
    DeleteBranches(Vec<String>),
    Tag { name: String, at: String },
    Stash(StashOptions),
    StashApply(usize),
    StashPop(usize),
    StashDrop(usize),
    Revert(String),
    CherryPick(String),
    /// Check a commit out, detaching HEAD.
    Checkout(String),
    Reset { target: String, mode: ResetMode },
    Continue,
    Abort,
    Skip,
    /// Take back the logged operation of this time.
    Undo { time: u64, label: String },
    /// An interactive rebase onto `base` (the root when `None`).
    RebasePlan { base: Option<String>, plan: Vec<fenix_git::Planned> },
    /// Check `branch` out in a new worktree at `path`.
    WorktreeAdd { branch: String, path: PathBuf },
    WorktreeRemove(PathBuf),
    WorktreePrune,
}

impl Job {
    /// What the page says while it runs and after.
    pub fn label(&self) -> String {
        let n = |v: usize, one: &str, many: &str| if v == 1 { one.to_string() } else { format!("{v} {many}") };
        match self {
            Job::Stage(p) => format!("stage {}", n(p.len(), &p.first().cloned().unwrap_or_default(), "files")),
            Job::Unstage(p) => format!("unstage {}", n(p.len(), &p.first().cloned().unwrap_or_default(), "files")),
            Job::Discard(p) => format!("discard {}", n(p.len(), &p.first().map(|f| f.0.clone()).unwrap_or_default(), "files")),
            Job::Apply { what, .. } => what.clone(),
            Job::Commit(CommitKind::Extend, _) => "extend the last commit".to_string(),
            Job::Commit(CommitKind::Fixup(h), _) => format!("fixup! {}", short(h)),
            Job::Commit(..) => "commit".to_string(),
            Job::FixupNow { hash, .. } => format!("fix up {}", short(hash)),
            Job::Push(o) => format!("push{} to {}/{}", if o.force_with_lease { " --force-with-lease" } else { "" }, o.remote, o.branch),
            Job::PushTags(r) => format!("push tags to {r}"),
            Job::Pull { rebase: true } => "pull --rebase".to_string(),
            Job::Pull { rebase: false } => "pull (merge)".to_string(),
            Job::Fetch => "fetch --all --prune".to_string(),
            Job::Branch { name, switch, .. } => format!("{} {name}", if *switch { "switch to new branch" } else { "create branch" }),
            Job::Rename { new, .. } => format!("rename branch to {new}"),
            Job::DeleteBranches(b) => format!("delete {}", n(b.len(), &b.first().cloned().unwrap_or_default(), "branches")),
            Job::Tag { name, .. } => format!("tag {name}"),
            Job::Stash(_) => "stash".to_string(),
            Job::StashApply(i) => format!("apply stash@{{{i}}}"),
            Job::StashPop(i) => format!("pop stash@{{{i}}}"),
            Job::StashDrop(i) => format!("drop stash@{{{i}}}"),
            Job::Revert(h) => format!("revert {}", short(h)),
            Job::CherryPick(h) => format!("cherry-pick {}", short(h)),
            Job::Checkout(h) => format!("check out {}", short(h)),
            Job::Reset { target, mode } => format!("reset {} to {}", mode_name(*mode), short(target)),
            Job::Continue => "continue".to_string(),
            Job::Abort => "abort".to_string(),
            Job::Skip => "skip".to_string(),
            Job::Undo { label, .. } => format!("undo {label}"),
            Job::RebasePlan { plan, .. } => format!("rebase {}", count(plan.len(), "commit")),
            Job::WorktreeAdd { branch, .. } => format!("check {branch} out in a worktree"),
            Job::WorktreeRemove(path) => format!("remove the worktree {}", path.display()),
            Job::WorktreePrune => "prune worktrees".to_string(),
        }
    }

}

pub(crate) fn short(hash: &str) -> &str {
    &hash[..hash.len().min(7)]
}

fn mode_name(mode: ResetMode) -> &'static str {
    match mode {
        ResetMode::Soft => "--soft",
        ResetMode::Mixed => "--mixed",
        ResetMode::Hard => "--hard",
    }
}

/// How the commit message editor is opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Compose {
    New(CommitFlags),
    /// Seeded with the last message.
    Amend(CommitFlags),
    Reword(CommitFlags),
}

/// Refs picked in the shared picker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickRef {
    Switch,
    Merge,
    Rebase,
}

/// What the page asks of its host.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    None,
    Refresh,
    LoadDiff { section: Section, path: String },
    Close,
    OpenFile { path: String, line: Option<usize> },
    Run(Job),
    Compose(Compose),
    Pick(PickRef),
    OpenHistory,
    OpenCompare,
    OpenMergeView,
    Copy(String),
    ShowOutput,
    /// Work out what undoing an operation would do -- the latest that
    /// can be, or the one of this time -- and ask.
    PlanUndo(Option<u64>),
    /// Open the interactive rebase page: from this commit on, or (with
    /// `None`) everything not on the upstream or base yet.
    Rebase(Option<String>),
    /// Open a worktree as a workspace of its own.
    OpenWorktree(PathBuf),
    /// Review the branch's pull request, or open one.
    PullRequest,
    /// The review inbox.
    Reviews,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuKind {
    Commit,
    Push,
    Pull,
    Branch,
    Stash,
    Log,
    Rebase,
    CommitRow,
    Reset,
    StashRow,
    Worktree,
    Help,
}

/// One entry of a menu. `id` is what `menu_pick` matches on.
#[derive(Debug, Clone, PartialEq)]
pub struct MenuItem {
    pub key: String,
    pub label: String,
    pub detail: String,
    /// A flag's state; `None` for a verb.
    pub flag: Option<bool>,
    pub danger: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Menu {
    pub title: String,
    pub groups: Vec<(Option<String>, Vec<MenuItem>)>,
}

pub(crate) fn verb(key: &str, label: &str, detail: impl Into<String>) -> MenuItem {
    MenuItem { key: key.to_string(), label: label.to_string(), detail: detail.into(), flag: None, danger: false }
}

fn flag(key: &str, label: &str, on: bool) -> MenuItem {
    MenuItem { key: key.to_string(), label: label.to_string(), detail: String::new(), flag: Some(on), danger: false }
}

pub(crate) fn danger(mut item: MenuItem) -> MenuItem {
    item.danger = true;
    item
}

#[derive(Debug, Clone, PartialEq)]
pub struct MenuState {
    pub kind: MenuKind,
    /// Keys typed so far, for two-character keys like `-a`.
    pub typed: String,
    /// The commit or stash the menu is about.
    pub target: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum InputPurpose {
    NewBranch { switch: bool },
    Rename(String),
    StashMessage(StashOptions),
    BranchAt(String),
    TagAt(String),
    PushElsewhere,
    NewWorktree,
}

/// A one-line field typed into on the page.
#[derive(Debug, Clone, PartialEq)]
pub struct Input {
    pub label: String,
    pub text: String,
    pub purpose: InputPurpose,
}

/// A question before something that can't be taken back, or a plan to
/// agree to: each key runs its job.
#[derive(Debug, Clone, PartialEq)]
pub struct Confirm {
    pub question: String,
    pub detail: Vec<(Role, String)>,
    pub choices: Vec<(char, String, Job)>,
}

/// The page's flags, remembered while it's open.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Flags {
    pub commit: CommitFlags,
    pub force_with_lease: bool,
    pub dry_run: bool,
}

pub struct GitStatus {
    pub root: PathBuf,
    pub name: String,
    pub snap: Option<Snapshot>,
    pub diffs: HashMap<(Section, String), DiffState>,
    pub folded: HashSet<Section>,
    pub expanded: HashSet<(Section, String)>,
    pub cursor: usize,
    /// Where a Visual-line selection started, as a row index.
    pub anchor: Option<usize>,
    pub menu: Option<MenuState>,
    pub input: Option<Input>,
    pub confirm: Option<Confirm>,
    pub flags: Flags,
    /// The last operation's result: (text, failed).
    pub message: Option<(String, bool)>,
    /// What's running now.
    pub busy: Option<String>,
    /// The last operation's full output, for `$`.
    pub output: Vec<String>,
    /// A snapshot is being read; the timed refresh waits for it.
    pub loading: bool,
    /// Open the operation log once the next snapshot lands (`SPC g z`
    /// on a page still reading).
    pub reveal_operations: bool,
    /// A worktree to open once the job adding it has worked.
    pub open_after: Option<PathBuf>,
    /// The branch's pull request, once the forge has been asked --
    /// `Some(None)` when there's none; `None` with no forge to ask.
    pub request: Option<Option<RequestLine>>,
    pending_g: bool,
}

impl GitStatus {
    pub fn new(root: PathBuf) -> Self {
        let name = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| root.display().to_string());
        GitStatus {
            root,
            name,
            snap: None,
            diffs: HashMap::new(),
            folded: [Section::Stashes, Section::Recent, Section::Operations].into_iter().collect(),
            expanded: HashSet::new(),
            cursor: 0,
            anchor: None,
            menu: None,
            input: None,
            confirm: None,
            flags: Flags::default(),
            message: None,
            busy: None,
            output: Vec::new(),
            loading: false,
            reveal_operations: false,
            open_after: None,
            request: None,
            pending_g: false,
        }
    }

    pub fn typing(&self) -> bool {
        self.input.is_some()
    }

    pub fn type_text(&mut self, text: &str) {
        if let Some(input) = &mut self.input {
            input.text.extend(text.chars().filter(|c| !c.is_control()));
        }
    }

    /// Takes a fresh snapshot, keeping the cursor on the same row when
    /// it still exists, and dropping diffs of files that went away.
    pub fn set_snapshot(&mut self, snap: Snapshot) {
        self.loading = false;
        let old_rows = self.rows();
        let before = old_rows.get(self.cursor).cloned();
        self.snap = Some(snap);
        if std::mem::take(&mut self.reveal_operations) {
            self.show_operations();
            return;
        }
        let rows = self.rows();
        if rows == old_rows {
            // The timed refresh, with nothing changed: leave the cursor
            // and a Visual selection exactly where they are.
            return;
        }
        let files: HashSet<(Section, String)> = rows.iter().filter_map(|r| match r {
            Row::File { section, path } => Some((*section, path.clone())),
            _ => None,
        }).collect();
        self.expanded.retain(|k| files.contains(k));
        self.diffs.retain(|k, _| files.contains(k));
        if let Some(before) = before {
            self.cursor = find_again(&rows, &before).unwrap_or(self.cursor);
        }
        self.cursor = self.cursor.min(rows.len().saturating_sub(1));
        self.anchor = None;
    }

    /// Diffs of expanded files, to fetch again after the tree changed.
    pub fn expanded_files(&self) -> Vec<(Section, String)> {
        let mut out: Vec<_> = self.expanded.iter().cloned().collect();
        out.sort();
        out
    }

    pub fn set_diff(&mut self, section: Section, path: String, diff: DiffState) {
        if self.expanded.contains(&(section, path.clone())) {
            let before = self.rows().get(self.cursor).cloned();
            self.diffs.insert((section, path), diff);
            if let Some(before) = before {
                let rows = self.rows();
                self.cursor = find_again(&rows, &before).unwrap_or(self.cursor).min(rows.len().saturating_sub(1));
            }
        }
    }

    fn files(&self, section: Section) -> Vec<(char, String)> {
        self.snap.as_ref().map(|s| files_in(s, section)).unwrap_or_default()
    }

    fn sections(&self) -> Vec<Section> {
        let Some(snap) = &self.snap else { return Vec::new() };
        let mut out = Vec::new();
        for section in [Section::Conflicted, Section::Untracked, Section::Unstaged, Section::Staged] {
            if !files_in(snap, section).is_empty() {
                out.push(section);
            }
        }
        if !snap.stashes.is_empty() {
            out.push(Section::Stashes);
        }
        if !snap.unpulled.is_empty() {
            out.push(Section::Unpulled);
        }
        if !snap.unpushed.is_empty() {
            out.push(Section::Unpushed);
        }
        if !snap.recent.is_empty() {
            out.push(Section::Recent);
        }
        if snap.worktrees.len() > 1 {
            out.push(Section::Worktrees);
        }
        if !snap.ops.is_empty() {
            out.push(Section::Operations);
        }
        out
    }

    /// Unfolds the operation log and puts the cursor on it (`SPC g z`).
    pub fn show_operations(&mut self) {
        self.folded.remove(&Section::Operations);
        if let Some(i) = self.rows().iter().position(|r| *r == Row::Section(Section::Operations)) {
            self.cursor = i;
        }
    }

    fn commits(&self, section: Section) -> &[Commit] {
        let Some(snap) = &self.snap else { return &[] };
        match section {
            Section::Unpulled => &snap.unpulled,
            Section::Unpushed => &snap.unpushed,
            Section::Recent => &snap.recent,
            _ => &[],
        }
    }

    /// Every row, in page order.
    pub fn rows(&self) -> Vec<Row> {
        let Some(snap) = &self.snap else { return Vec::new() };
        let mut rows = Vec::new();
        for section in self.sections() {
            rows.push(Row::Section(section));
            if self.folded.contains(&section) {
                continue;
            }
            match section {
                Section::Stashes => rows.extend(snap.stashes.iter().map(|s| Row::Stash(s.index))),
                Section::Operations => rows.extend(snap.ops.iter().map(|o| Row::Op(o.time))),
                Section::Worktrees => rows.extend(snap.worktrees.iter().map(|w| Row::Worktree(w.path.clone()))),
                Section::Unpulled | Section::Unpushed | Section::Recent => {
                    rows.extend(self.commits(section).iter().map(|c| Row::Commit { section, hash: c.hash.clone() }))
                }
                _ => {
                    for (_, path) in files_in(snap, section) {
                        rows.push(Row::File { section, path: path.clone() });
                        if !self.expanded.contains(&(section, path.clone())) {
                            continue;
                        }
                        if let Some(DiffState::Loaded(diff)) = self.diffs.get(&(section, path.clone())) {
                            for (h, hunk) in diff.hunks.iter().enumerate() {
                                rows.push(Row::Hunk { section, path: path.clone(), hunk: h });
                                rows.extend((0..hunk.lines.len()).map(|line| Row::Line { section, path: path.clone(), hunk: h, line }));
                            }
                        }
                    }
                }
            }
        }
        rows
    }

    fn diff(&self, section: Section, path: &str) -> Option<&FileDiff> {
        match self.diffs.get(&(section, path.to_string())) {
            Some(DiffState::Loaded(diff)) => Some(diff),
            _ => None,
        }
    }

    /// The selected lines of one hunk: the Visual range clipped to the
    /// cursor's hunk, else just the cursor's line.
    fn selected_lines(&self, rows: &[Row]) -> Option<(Section, String, usize, Vec<usize>)> {
        let Some(Row::Line { section, path, hunk, line }) = rows.get(self.cursor) else { return None };
        let mut picked = vec![*line];
        if let Some(anchor) = self.anchor {
            let (a, b) = (anchor.min(self.cursor), anchor.max(self.cursor));
            picked = rows[a..=b]
                .iter()
                .filter_map(|r| match r {
                    Row::Line { section: s, path: p, hunk: h, line } if s == section && p == path && h == hunk => Some(*line),
                    _ => None,
                })
                .collect();
        }
        Some((*section, path.clone(), *hunk, picked))
    }

    fn message(&mut self, text: impl Into<String>, failed: bool) -> Action {
        self.message = Some((text.into(), failed));
        Action::None
    }

    /// One key, when the page has the keyboard.
    pub fn key(&mut self, key: Key) -> Action {
        if self.input.is_some() {
            return self.input_key(key);
        }
        if self.confirm.is_some() {
            return self.confirm_key(key);
        }
        if self.menu.is_some() {
            return self.menu_key(key);
        }
        let rows = self.rows();
        let g = std::mem::take(&mut self.pending_g);
        match key {
            Key::Down | Key::Char('j') => self.move_cursor(1, rows.len()),
            Key::Up | Key::Char('k') => self.move_cursor(-1, rows.len()),
            Key::Char('g') if g => {
                self.cursor = 0;
                Action::None
            }
            Key::Char('g') => {
                self.pending_g = true;
                Action::None
            }
            Key::Char('G') => {
                self.cursor = rows.len().saturating_sub(1);
                Action::None
            }
            Key::Char(']') => self.jump_section(&rows, true),
            Key::Char('[') => self.jump_section(&rows, false),
            Key::Escape => {
                self.anchor = None;
                Action::None
            }
            Key::Tab => self.toggle(&rows),
            Key::Enter => self.enter(&rows),
            Key::Char('V') => {
                if matches!(rows.get(self.cursor), Some(Row::Line { .. })) {
                    self.anchor = if self.anchor.is_some() { None } else { Some(self.cursor) };
                    Action::None
                } else {
                    self.message("V selects lines of an expanded diff -- Tab opens one", false)
                }
            }
            Key::Char('s') => self.stage(&rows),
            Key::Char('S') => self.unstage(&rows),
            Key::Char('d') => self.discard(&rows),
            Key::Char('a') => self.stage_all(),
            Key::Char('A') => {
                let paths: Vec<String> = self.files(Section::Staged).into_iter().map(|(_, p)| p).collect();
                if paths.is_empty() {
                    return self.message("nothing is staged", false);
                }
                Action::Run(Job::Unstage(paths))
            }
            Key::Char('c') => self.open_menu(MenuKind::Commit, None),
            Key::Char('P') => self.open_menu(MenuKind::Push, None),
            Key::Char('p') => self.open_menu(MenuKind::Pull, None),
            Key::Char('b') => self.open_menu(MenuKind::Branch, None),
            Key::Char('z') => self.open_menu(MenuKind::Stash, None),
            Key::Char('l') => self.open_menu(MenuKind::Log, None),
            Key::Char('r') => self.open_menu(MenuKind::Rebase, None),
            Key::Char('w') => self.open_menu(MenuKind::Worktree, None),
            Key::Char('?') | Key::Char('x') => self.open_menu(MenuKind::Help, None),
            Key::Char('.') => match rows.get(self.cursor) {
                Some(Row::Commit { hash, .. }) => self.open_menu(MenuKind::CommitRow, Some(hash.clone())),
                Some(Row::Stash(i)) => self.open_menu(MenuKind::StashRow, Some(i.to_string())),
                _ => Action::None,
            },
            Key::Char('y') => match rows.get(self.cursor) {
                Some(Row::Commit { hash, .. }) => Action::Copy(hash.clone()),
                Some(row) => row.path().map(|p| Action::Copy(p.to_string())).unwrap_or(Action::None),
                None => Action::None,
            },
            Key::Char('u') => Action::Refresh,
            Key::Char('U') => match rows.get(self.cursor) {
                Some(Row::Op(time)) => Action::PlanUndo(Some(*time)),
                _ => Action::PlanUndo(None),
            },
            Key::Char('$') => Action::ShowOutput,
            Key::Char('o') => Action::PullRequest,
            Key::Char('M') => Action::Reviews,
            Key::Char('q') => Action::Close,
            _ => Action::None,
        }
    }

    fn move_cursor(&mut self, delta: isize, len: usize) -> Action {
        if len > 0 {
            self.cursor = (self.cursor as isize + delta).clamp(0, len as isize - 1) as usize;
        }
        Action::None
    }

    fn jump_section(&mut self, rows: &[Row], forward: bool) -> Action {
        let is_stop = |r: &Row| matches!(r, Row::Section(_) | Row::Hunk { .. });
        let found = if forward {
            rows.iter().enumerate().skip(self.cursor + 1).find(|(_, r)| is_stop(r)).map(|(i, _)| i)
        } else {
            rows.iter().enumerate().take(self.cursor).rev().find(|(_, r)| is_stop(r)).map(|(i, _)| i)
        };
        if let Some(i) = found {
            self.cursor = i;
        }
        Action::None
    }

    fn toggle(&mut self, rows: &[Row]) -> Action {
        match rows.get(self.cursor).cloned() {
            Some(Row::Section(section)) => {
                if !self.folded.remove(&section) {
                    self.folded.insert(section);
                }
                Action::None
            }
            Some(Row::File { section, path }) if section.has_diffs() => {
                let key = (section, path.clone());
                if self.expanded.remove(&key) {
                    self.diffs.remove(&key);
                    Action::None
                } else {
                    self.expanded.insert(key.clone());
                    self.diffs.insert(key, DiffState::Loading);
                    Action::LoadDiff { section, path }
                }
            }
            Some(Row::File { .. }) => self.message("only unstaged and staged files have a diff to show", false),
            Some(Row::Hunk { section, path, .. }) | Some(Row::Line { section, path, .. }) => {
                self.expanded.remove(&(section, path.clone()));
                self.diffs.remove(&(section, path.clone()));
                self.anchor = None;
                let rows = self.rows();
                self.cursor = rows.iter().position(|r| *r == Row::File { section, path: path.clone() }).unwrap_or(self.cursor);
                Action::None
            }
            _ => Action::None,
        }
    }

    fn enter(&mut self, rows: &[Row]) -> Action {
        match rows.get(self.cursor).cloned() {
            Some(Row::Section(_)) => self.toggle(rows),
            Some(Row::File { section: Section::Conflicted, .. }) => Action::OpenMergeView,
            Some(Row::File { path, .. }) => Action::OpenFile { path, line: None },
            Some(Row::Hunk { section, path, hunk }) => {
                let line = self.diff(section, &path).and_then(|d| d.hunks.get(hunk)).map(|h| h.new_start.max(1));
                Action::OpenFile { path, line }
            }
            Some(Row::Line { section, path, hunk, line }) => {
                let at = self.diff(section, &path).and_then(|d| d.hunks.get(hunk)).and_then(|h| {
                    // A removed line isn't in the file any more: go to
                    // the nearest line that is.
                    h.lines[..=line].iter().rev().find_map(|l| l.new_line).or(Some(h.new_start.max(1)))
                });
                Action::OpenFile { path, line: at }
            }
            Some(Row::Commit { hash, .. }) => self.open_menu(MenuKind::CommitRow, Some(hash)),
            Some(Row::Stash(i)) => self.open_menu(MenuKind::StashRow, Some(i.to_string())),
            Some(Row::Op(time)) => Action::PlanUndo(Some(time)),
            Some(Row::Worktree(path)) => Action::OpenWorktree(path),
            None => Action::None,
        }
    }

    fn stage_all(&mut self) -> Action {
        let paths: Vec<String> = [Section::Untracked, Section::Unstaged].iter().flat_map(|s| self.files(*s)).map(|(_, p)| p).collect();
        if paths.is_empty() {
            return self.message("nothing to stage", false);
        }
        Action::Run(Job::Stage(paths))
    }

    fn stage(&mut self, rows: &[Row]) -> Action {
        let Some(row) = rows.get(self.cursor).cloned() else { return Action::None };
        let section = row.section();
        match (row, section) {
            (_, Some(Section::Staged)) => self.message("already staged -- S unstages", false),
            (Row::Section(s @ (Section::Untracked | Section::Unstaged | Section::Conflicted)), _) => {
                let paths: Vec<String> = self.files(s).into_iter().map(|(_, p)| p).collect();
                Action::Run(Job::Stage(paths))
            }
            (Row::File { path, .. }, Some(Section::Untracked | Section::Unstaged | Section::Conflicted)) => Action::Run(Job::Stage(vec![path])),
            (Row::Hunk { section, path, hunk }, _) => self.hunk_job(section, &path, hunk, ApplyTarget::Stage),
            (Row::Line { .. }, _) => self.lines_job(rows, ApplyTarget::Stage),
            _ => Action::None,
        }
    }

    fn unstage(&mut self, rows: &[Row]) -> Action {
        let Some(row) = rows.get(self.cursor).cloned() else { return Action::None };
        if row.section() != Some(Section::Staged) {
            return self.message("S unstages -- move to the Staged section", false);
        }
        match row {
            Row::Section(_) => Action::Run(Job::Unstage(self.files(Section::Staged).into_iter().map(|(_, p)| p).collect())),
            Row::File { path, .. } => Action::Run(Job::Unstage(vec![path])),
            Row::Hunk { section, path, hunk } => self.hunk_job(section, &path, hunk, ApplyTarget::Unstage),
            Row::Line { .. } => self.lines_job(rows, ApplyTarget::Unstage),
            _ => Action::None,
        }
    }

    fn discard(&mut self, rows: &[Row]) -> Action {
        let Some(row) = rows.get(self.cursor).cloned() else { return Action::None };
        let section = row.section();
        let job = match (&row, section) {
            (Row::Stash(i), _) => {
                self.confirm = Some(Confirm {
                    question: format!("Drop stash@{{{i}}}?"),
                    detail: Vec::new(),
                    choices: vec![('y', "drop it".to_string(), Job::StashDrop(*i))],
                });
                return Action::None;
            }
            (_, Some(Section::Staged)) => return self.message("unstage it first (S), then discard", false),
            (Row::Section(s @ (Section::Untracked | Section::Unstaged)), _) => {
                Job::Discard(self.files(*s).into_iter().map(|(_, p)| (p, *s == Section::Untracked)).collect())
            }
            (Row::File { path, section }, _) if matches!(section, Section::Untracked | Section::Unstaged) => {
                Job::Discard(vec![(path.clone(), *section == Section::Untracked)])
            }
            (Row::Hunk { section, path, hunk }, _) => match self.hunk_job(*section, path, *hunk, ApplyTarget::Discard) {
                Action::Run(job) => job,
                other => return other,
            },
            (Row::Line { .. }, _) => match self.lines_job(rows, ApplyTarget::Discard) {
                Action::Run(job) => job,
                other => return other,
            },
            _ => return Action::None,
        };
        let question = match &job {
            Job::Discard(files) if files.iter().all(|f| f.1) => format!("Delete {} -- untracked, so git can't bring it back?", count(files.len(), "file")),
            Job::Discard(files) => format!("Discard the changes to {}?", count(files.len(), "file")),
            Job::Apply { what, .. } => format!("{}?", capitalise(what)),
            _ => "Discard?".to_string(),
        };
        let detail = match &job {
            Job::Discard(files) => files.iter().take(6).map(|(p, _)| (Role::Muted, p.clone())).collect(),
            _ => Vec::new(),
        };
        self.confirm = Some(Confirm { question, detail, choices: vec![('y', "discard".to_string(), job)] });
        Action::None
    }

    fn hunk_job(&mut self, section: Section, path: &str, hunk: usize, target: ApplyTarget) -> Action {
        let Some(diff) = self.diff(section, path) else { return Action::None };
        let Some(h) = diff.hunks.get(hunk) else { return Action::None };
        if diff.is_combined || diff.is_binary {
            return self.message("this diff can't be staged by hunk", true);
        }
        let patch = fenix_diff::hunk_patch(diff, h);
        let what = format!("{} a hunk of {path}", verb_of(target));
        Action::Run(Job::Apply { patch, target, what })
    }

    fn lines_job(&mut self, rows: &[Row], target: ApplyTarget) -> Action {
        let Some((section, path, hunk, picked)) = self.selected_lines(rows) else { return Action::None };
        let Some(diff) = self.diff(section, &path) else { return Action::None };
        let Some(h) = diff.hunks.get(hunk) else { return Action::None };
        let reverse = target != ApplyTarget::Stage;
        let Some(patch) = fenix_diff::lines_patch(diff, h, &picked, reverse) else {
            return self.message("no added or removed line selected", false);
        };
        let changed = picked.iter().filter(|i| matches!(h.lines[**i].kind, LineKind::Added | LineKind::Removed)).count();
        let what = format!("{} {} of {path}", verb_of(target), count(changed, "line"));
        self.anchor = None;
        Action::Run(Job::Apply { patch, target, what })
    }

    // -- Menus ---------------------------------------------------------

    fn open_menu(&mut self, kind: MenuKind, target: Option<String>) -> Action {
        self.menu = Some(MenuState { kind, typed: String::new(), target });
        Action::None
    }

    fn staged_count(&self) -> usize {
        self.files(Section::Staged).len()
    }

    fn branch(&self) -> String {
        self.snap.as_ref().and_then(|s| s.status.as_ref()).map(|s| s.branch.clone()).unwrap_or_default()
    }

    fn upstream(&self) -> Option<String> {
        self.snap.as_ref().and_then(|s| s.status.as_ref()).and_then(|s| s.upstream.clone())
    }

    /// Where `P p` pushes: the upstream, else a new branch of the same
    /// name on the default remote.
    fn push_target(&self) -> Option<(String, String, bool)> {
        if let Some(upstream) = self.upstream() {
            let (remote, branch) = upstream.split_once('/')?;
            return Some((remote.to_string(), branch.to_string(), false));
        }
        let remote = self.snap.as_ref()?.push_remote.clone()?;
        Some((remote, self.branch(), true))
    }

    /// The menu of `kind`, built for the page as it is now.
    pub fn menu(&self, state: &MenuState) -> Menu {
        let f = self.flags;
        let target = state.target.clone().unwrap_or_default();
        match state.kind {
            MenuKind::Commit => {
                let staged = self.staged_count();
                let on_commit = matches!(self.rows().get(self.cursor), Some(Row::Commit { .. }));
                Menu {
                    title: format!("Commit · {}", if staged == 0 { "nothing staged".to_string() } else { count(staged, "file") + " staged" }),
                    groups: vec![
                        (
                            None,
                            vec![
                                verb("c", "commit", "write a message"),
                                verb("a", "amend", "the last commit, new message"),
                                verb("e", "extend", "amend, keep the message"),
                                verb("w", "reword", "the last message only"),
                                verb("f", "fixup", if on_commit { "into the commit under the cursor" } else { "put the cursor on a commit" }),
                                verb("F", "fixup & squash", "fold it in now"),
                            ],
                        ),
                        (
                            Some("Flags".to_string()),
                            vec![flag("-a", "stage all tracked", f.commit.all), flag("-s", "sign off", f.commit.signoff), flag("-n", "skip hooks", f.commit.no_verify)],
                        ),
                    ],
                }
            }
            MenuKind::Push => {
                let (ahead, behind) = self.snap.as_ref().and_then(|s| s.status.as_ref()).map(|s| (s.ahead, s.behind)).unwrap_or((0, 0));
                let main = match self.push_target() {
                    Some((remote, branch, true)) => verb("p", &format!("{remote}/{branch}"), "new branch, sets the upstream"),
                    Some((remote, branch, false)) => verb("p", &format!("{remote}/{branch}"), "the upstream"),
                    None => verb("p", "no remote", "add one with git remote add"),
                };
                let remote = self.push_target().map(|t| t.0).unwrap_or_else(|| "origin".to_string());
                let standing = match self.upstream() {
                    Some(_) => format!("↑{ahead} ↓{behind}"),
                    None => format!("{} not pushed yet", count(self.snap.as_ref().map(|s| s.unpushed.len()).unwrap_or(0), "commit")),
                };
                Menu {
                    title: format!("Push {} · {standing}", self.branch()),
                    groups: vec![
                        (None, vec![main, verb("e", "elsewhere…", "remote/branch"), verb("t", "tags", format!("to {remote}"))]),
                        (Some("Flags".to_string()), vec![flag("-f", "force-with-lease", f.force_with_lease), flag("-n", "dry run", f.dry_run)]),
                    ],
                }
            }
            MenuKind::Pull => {
                let behind = self.snap.as_ref().and_then(|s| s.status.as_ref()).map(|s| s.behind).unwrap_or(0);
                Menu {
                    title: format!("Pull into {} · ↓{behind}", self.branch()),
                    groups: vec![(
                        None,
                        vec![
                            verb("p", "rebase onto upstream", "pull --rebase"),
                            verb("m", "merge upstream", "pull --no-rebase"),
                            verb("f", "fetch only", "all remotes, prune"),
                        ],
                    )],
                }
            }
            MenuKind::Branch => {
                let gone = self.snap.as_ref().map(|s| s.gone.len()).unwrap_or(0);
                Menu {
                    title: format!("Branch · on {}", self.branch()),
                    groups: vec![(
                        None,
                        vec![
                            verb("b", "switch…", "pick a branch"),
                            verb("c", "create & switch", "from HEAD"),
                            verb("n", "create", "stay here"),
                            verb("r", "rename", self.branch()),
                            verb("m", "merge into this…", ""),
                            verb("R", "rebase onto…", ""),
                            danger(verb("D", "delete gone", if gone == 0 { "none".to_string() } else { count(gone, "branch") })),
                        ],
                    )],
                }
            }
            MenuKind::Stash => {
                let file = self.rows().get(self.cursor).and_then(|r| r.path().map(str::to_string));
                Menu {
                    title: "Stash".to_string(),
                    groups: vec![(
                        None,
                        vec![
                            verb("z", "everything", "tracked changes"),
                            verb("i", "including untracked", ""),
                            verb("s", "staged only", ""),
                            verb("f", "this file", file.unwrap_or_else(|| "put the cursor on a file".to_string())),
                        ],
                    )],
                }
            }
            MenuKind::StashRow => Menu {
                title: format!("stash@{{{target}}}"),
                groups: vec![(None, vec![verb("a", "apply", "keep it"), verb("p", "pop", "apply and drop"), danger(verb("d", "drop", ""))])],
            },
            MenuKind::Worktree => {
                let on = match self.rows().get(self.cursor) {
                    Some(Row::Worktree(path)) if path != &self.root => Some(path.display().to_string()),
                    _ => None,
                };
                Menu {
                    title: "Worktrees".to_string(),
                    groups: vec![(
                        None,
                        vec![
                            verb("a", "add for a branch…", "beside the repository, as its own workspace"),
                            danger(verb("d", "remove", on.unwrap_or_else(|| "put the cursor on one".to_string()))),
                            verb("p", "prune", "forget ones whose folder is gone"),
                        ],
                    )],
                }
            }
            MenuKind::Log => Menu {
                title: "Log".to_string(),
                groups: vec![(None, vec![verb("l", "history", "every branch, as a graph"), verb("c", "compare…", "two refs")])],
            },
            MenuKind::Rebase => {
                let suspended = self.snap.as_ref().and_then(|s| s.in_progress.clone());
                let items = match &suspended {
                    Some(_) => vec![verb("c", "continue", "after resolving"), verb("s", "skip", "this commit"), danger(verb("a", "abort", "back to before")), verb("x", "resolve conflicts", "the merge view")],
                    None => vec![
                        verb("i", "interactively…", "reorder, reword, squash, drop"),
                        verb("o", "onto…", "pick a ref"),
                        verb("x", "resolve conflicts", "the merge view"),
                    ],
                };
                Menu { title: suspended.unwrap_or_else(|| "Rebase".to_string()), groups: vec![(None, items)] }
            }
            MenuKind::CommitRow => {
                let subject = self.commit(&target).map(|c| c.message.clone()).unwrap_or_default();
                let head = self.snap.as_ref().and_then(|s| s.recent.first()).is_some_and(|c| c.hash == target);
                Menu {
                    title: format!("{} {}", short(&target), fit(&subject, 40)),
                    groups: vec![(
                        None,
                        vec![
                            verb("f", "fixup into this", "staged changes, squashed in now"),
                            verb("w", "reword", if head { "the last commit" } else { "only the last commit, for now" }),
                            verb("v", "revert", "a new commit undoing it"),
                            verb("b", "branch here…", ""),
                            verb("t", "tag here…", ""),
                            danger(verb("r", "reset to here…", "soft / mixed / hard")),
                            verb("y", "copy hash", ""),
                        ],
                    )],
                }
            }
            MenuKind::Reset => Menu {
                title: format!("Reset {} to {}", self.branch(), short(&target)),
                groups: vec![(
                    None,
                    vec![verb("s", "soft", "keep index and files"), verb("m", "mixed", "keep files"), danger(verb("h", "hard", "discard everything since"))],
                )],
            },
            MenuKind::Help => Menu {
                title: "Git keys".to_string(),
                groups: vec![
                    (
                        Some("Menus".to_string()),
                        vec![
                            verb("c", "commit…", ""),
                            verb("P", "push…", ""),
                            verb("p", "pull…", ""),
                            verb("b", "branch…", ""),
                            verb("z", "stash…", ""),
                            verb("l", "log…", ""),
                            verb("r", "rebase…", ""),
                            verb("w", "worktrees…", ""),
                            verb("o", "pull request", "review it, or open one"),
                            verb("M", "reviews", "the inbox"),
                        ],
                    ),
                    (
                        Some("On a row".to_string()),
                        vec![
                            verb("Tab", "fold / show diff", ""),
                            verb("s S", "stage / unstage", "file, hunk or lines"),
                            verb("d", "discard", "asks first"),
                            verb("V", "select lines", "in a diff"),
                            verb("a A", "stage all / unstage all", ""),
                            verb("Enter", "open / commit menu", ""),
                            verb("U", "undo", "the last operation, or the one under the cursor"),
                            verb("u", "refresh", ""),
                            verb("$", "last output", ""),
                            verb("q", "close", ""),
                        ],
                    ),
                ],
            },
        }
    }

    fn commit(&self, hash: &str) -> Option<&Commit> {
        let snap = self.snap.as_ref()?;
        snap.recent.iter().chain(&snap.unpushed).chain(&snap.unpulled).find(|c| c.hash == hash)
    }

    fn menu_key(&mut self, key: Key) -> Action {
        let Some(state) = self.menu.clone() else { return Action::None };
        let c = match key {
            Key::Escape | Key::Char('q') => {
                self.menu = None;
                return Action::None;
            }
            Key::Char(c) => c,
            Key::Tab if state.kind == MenuKind::Help => {
                self.menu = None;
                return Action::None;
            }
            _ => return Action::None,
        };
        let typed = format!("{}{c}", state.typed);
        let menu = self.menu(&state);
        let items: Vec<&MenuItem> = menu.groups.iter().flat_map(|(_, items)| items).collect();
        if items.iter().any(|i| i.key == typed) {
            if let Some(m) = &mut self.menu {
                m.typed.clear();
            }
            let is_flag = items.iter().any(|i| i.key == typed && i.flag.is_some());
            if !is_flag && state.kind != MenuKind::Help {
                self.menu = None;
            }
            return self.menu_pick(&state, &typed);
        }
        if items.iter().any(|i| i.key.starts_with(&typed)) {
            if let Some(m) = &mut self.menu {
                m.typed = typed;
            }
            return Action::None;
        }
        if state.kind == MenuKind::Help {
            // The help menu lists keys; pressing one of them does it.
            self.menu = None;
            return self.key(key);
        }
        if let Some(m) = &mut self.menu {
            m.typed.clear();
        }
        self.message(format!("{typed} isn't in this menu -- Esc closes it"), false)
    }

    fn menu_pick(&mut self, state: &MenuState, key: &str) -> Action {
        let target = state.target.clone().unwrap_or_default();
        let flags = self.flags;
        match (state.kind, key) {
            (MenuKind::Commit, "-a") => self.flags.commit.all = !flags.commit.all,
            (MenuKind::Commit, "-s") => self.flags.commit.signoff = !flags.commit.signoff,
            (MenuKind::Commit, "-n") => self.flags.commit.no_verify = !flags.commit.no_verify,
            (MenuKind::Push, "-f") => self.flags.force_with_lease = !flags.force_with_lease,
            (MenuKind::Push, "-n") => self.flags.dry_run = !flags.dry_run,
            (MenuKind::Commit, "c") => {
                if self.staged_count() == 0 && !flags.commit.all {
                    return self.message("nothing staged -- stage something (s), or c -a to commit every tracked change", false);
                }
                return Action::Compose(Compose::New(flags.commit));
            }
            (MenuKind::Commit, "a") => return Action::Compose(Compose::Amend(flags.commit)),
            (MenuKind::Commit, "w") => return Action::Compose(Compose::Reword(flags.commit)),
            (MenuKind::Commit, "e") => return Action::Run(Job::Commit(CommitKind::Extend, flags.commit)),
            (MenuKind::Commit, "f" | "F") | (MenuKind::CommitRow, "f") => {
                let hash = match (state.kind, self.rows().get(self.cursor)) {
                    (MenuKind::CommitRow, _) => target,
                    (_, Some(Row::Commit { hash, .. })) => hash.clone(),
                    _ => return self.message("put the cursor on the commit to fix up (Unpushed or Recent), then c f", false),
                };
                if self.staged_count() == 0 && !flags.commit.all {
                    return self.message("stage the fix first -- a fixup commits what's staged", false);
                }
                if key == "f" && state.kind == MenuKind::Commit {
                    return Action::Run(Job::Commit(CommitKind::Fixup(hash), flags.commit));
                }
                return Action::Run(Job::FixupNow { hash, flags: flags.commit });
            }
            (MenuKind::Push, "p") => return self.push(),
            (MenuKind::Push, "e") => {
                self.input = Some(Input { label: "Push to (remote/branch)".to_string(), text: String::new(), purpose: InputPurpose::PushElsewhere })
            }
            (MenuKind::Push, "t") => {
                let remote = self.push_target().map(|t| t.0).unwrap_or_else(|| "origin".to_string());
                return Action::Run(Job::PushTags(remote));
            }
            (MenuKind::Pull, "p") => return Action::Run(Job::Pull { rebase: true }),
            (MenuKind::Pull, "m") => return Action::Run(Job::Pull { rebase: false }),
            (MenuKind::Pull, "f") => return Action::Run(Job::Fetch),
            (MenuKind::Branch, "b") => return Action::Pick(PickRef::Switch),
            (MenuKind::Branch, "c" | "n") => {
                self.input = Some(Input {
                    label: "New branch".to_string(),
                    text: String::new(),
                    purpose: InputPurpose::NewBranch { switch: key == "c" },
                })
            }
            (MenuKind::Branch, "r") => {
                let old = self.branch();
                self.input = Some(Input { label: format!("Rename {old} to"), text: old.clone(), purpose: InputPurpose::Rename(old) })
            }
            (MenuKind::Branch, "m") => return Action::Pick(PickRef::Merge),
            (MenuKind::Branch, "R") | (MenuKind::Rebase, "o") => return Action::Pick(PickRef::Rebase),
            (MenuKind::Branch, "D") => {
                let gone = self.snap.as_ref().map(|s| s.gone.clone()).unwrap_or_default();
                if gone.is_empty() {
                    return self.message("no branch has lost its upstream -- p f fetches with --prune to find them", false);
                }
                self.confirm = Some(Confirm {
                    question: format!("Delete {} whose upstream is gone?", count(gone.len(), "branch")),
                    detail: gone.iter().map(|b| (Role::Muted, b.clone())).collect(),
                    choices: vec![('y', "delete them".to_string(), Job::DeleteBranches(gone))],
                });
            }
            (MenuKind::Stash, "z" | "i" | "s") => {
                let options = StashOptions { include_untracked: key == "i", staged: key == "s", ..Default::default() };
                self.input = Some(Input { label: "Stash message (optional)".to_string(), text: String::new(), purpose: InputPurpose::StashMessage(options) });
            }
            (MenuKind::Stash, "f") => {
                let Some(path) = self.rows().get(self.cursor).and_then(|r| r.path().map(str::to_string)) else {
                    return self.message("put the cursor on a file, then z f", false);
                };
                return Action::Run(Job::Stash(StashOptions { paths: vec![path], ..Default::default() }));
            }
            (MenuKind::StashRow, k) => {
                let Ok(i) = target.parse::<usize>() else { return Action::None };
                match k {
                    "a" => return Action::Run(Job::StashApply(i)),
                    "p" => return Action::Run(Job::StashPop(i)),
                    "d" => {
                        self.confirm = Some(Confirm {
                            question: format!("Drop stash@{{{i}}}?"),
                            detail: Vec::new(),
                            choices: vec![('y', "drop it".to_string(), Job::StashDrop(i))],
                        })
                    }
                    _ => {}
                }
            }
            (MenuKind::Worktree, "a") => {
                self.input = Some(Input { label: "Branch to check out (new or existing)".to_string(), text: String::new(), purpose: InputPurpose::NewWorktree })
            }
            (MenuKind::Worktree, "d") => {
                let Some(Row::Worktree(path)) = self.rows().get(self.cursor).cloned() else {
                    return self.message("put the cursor on a worktree (the Worktrees section), then w d", false);
                };
                if path == self.root {
                    return self.message("that's the worktree this page is about -- it can't remove itself", false);
                }
                self.confirm = Some(Confirm {
                    question: format!("Remove the worktree at {}? Refused if it has uncommitted changes.", path.display()),
                    detail: Vec::new(),
                    choices: vec![('y', "remove it".to_string(), Job::WorktreeRemove(path))],
                });
            }
            (MenuKind::Worktree, "p") => return Action::Run(Job::WorktreePrune),
            (MenuKind::Log, "l") => return Action::OpenHistory,
            (MenuKind::Log, "c") => return Action::OpenCompare,
            (MenuKind::Rebase, "c") => return Action::Run(Job::Continue),
            (MenuKind::Rebase, "s") => return Action::Run(Job::Skip),
            (MenuKind::Rebase, "a") => {
                self.confirm = Some(Confirm {
                    question: "Abort, putting the branch back where it started?".to_string(),
                    detail: Vec::new(),
                    choices: vec![('y', "abort".to_string(), Job::Abort)],
                })
            }
            (MenuKind::Rebase, "x") => return Action::OpenMergeView,
            (MenuKind::Rebase, "i") => return Action::Rebase(None),
            (MenuKind::CommitRow, "w") => {
                let head = self.snap.as_ref().and_then(|s| s.recent.first()).is_some_and(|c| c.hash == target);
                if !head {
                    return self.message("only the last commit can be reworded here for now -- the rebase page will do older ones", false);
                }
                return Action::Compose(Compose::Reword(flags.commit));
            }
            (MenuKind::CommitRow, "v") => return Action::Run(Job::Revert(target)),
            (MenuKind::CommitRow, "b") => {
                self.input = Some(Input { label: format!("New branch at {}", short(&target)), text: String::new(), purpose: InputPurpose::BranchAt(target) })
            }
            (MenuKind::CommitRow, "t") => {
                self.input = Some(Input { label: format!("Tag {}", short(&target)), text: String::new(), purpose: InputPurpose::TagAt(target) })
            }
            (MenuKind::CommitRow, "r") => return self.open_menu(MenuKind::Reset, Some(target)),
            (MenuKind::CommitRow, "y") => return Action::Copy(target),
            (MenuKind::Reset, "s") => return Action::Run(Job::Reset { target, mode: ResetMode::Soft }),
            (MenuKind::Reset, "m") => return Action::Run(Job::Reset { target, mode: ResetMode::Mixed }),
            (MenuKind::Reset, "h") => {
                self.confirm = Some(Confirm {
                    question: format!("Reset --hard to {}? Uncommitted changes are lost.", short(&target)),
                    detail: Vec::new(),
                    choices: vec![('y', "reset --hard".to_string(), Job::Reset { target, mode: ResetMode::Hard })],
                })
            }
            (MenuKind::Help, k) => {
                self.menu = None;
                let first = k.chars().next().unwrap_or(' ');
                if k.len() == 1 {
                    return self.key(Key::Char(first));
                }
            }
            _ => {}
        }
        Action::None
    }

    /// `P p`: the push, or the plan to agree to when it would rewrite
    /// what's on the remote.
    fn push(&mut self) -> Action {
        let Some((remote, branch, new)) = self.push_target() else {
            return self.message("no remote to push to -- add one with git remote add", true);
        };
        let (ahead, behind) = self.snap.as_ref().and_then(|s| s.status.as_ref()).map(|s| (s.ahead, s.behind)).unwrap_or((0, 0));
        let options = PushOptions { remote: remote.clone(), branch: branch.clone(), set_upstream: new, force_with_lease: self.flags.force_with_lease, dry_run: self.flags.dry_run };
        if new || options.force_with_lease {
            return Action::Run(Job::Push(options));
        }
        if ahead == 0 && behind == 0 {
            return self.message(format!("nothing to push -- {branch} matches {remote}/{branch}"), false);
        }
        if ahead == 0 {
            return self.message(format!("nothing to push, and {remote}/{branch} is {behind} ahead -- p p pulls"), false);
        }
        if behind == 0 {
            return Action::Run(Job::Push(options));
        }
        // Diverged: pushing would replace what's there. Say exactly what.
        let snap = self.snap.as_ref();
        let mut detail = vec![(Role::Muted, format!("Replaces {} on {remote}/{branch}", count(behind, "commit")))];
        for c in snap.map(|s| s.unpulled.as_slice()).unwrap_or_default().iter().take(8) {
            detail.push((Role::Bad, format!("− {} {}", c.short_hash, c.message)));
        }
        detail.push((Role::Muted, format!("with {}", count(ahead, "commit"))));
        for c in snap.map(|s| s.unpushed.as_slice()).unwrap_or_default().iter().take(8) {
            detail.push((Role::Good, format!("+ {} {}", c.short_hash, c.message)));
        }
        let fetched = snap.and_then(|s| s.fetched_secs_ago).map(ago).unwrap_or_else(|| "never".to_string());
        detail.push((Role::Muted, format!("last fetched {fetched} -- the lease refuses if someone pushed since")));
        self.confirm = Some(Confirm {
            question: format!("{branch} and {remote}/{branch} have diverged"),
            detail,
            choices: vec![
                ('f', "push --force-with-lease".to_string(), Job::Push(PushOptions { force_with_lease: true, ..options })),
                ('p', "pull --rebase first".to_string(), Job::Pull { rebase: true }),
            ],
        });
        Action::None
    }

    fn input_key(&mut self, key: Key) -> Action {
        let Some(input) = &mut self.input else { return Action::None };
        match key {
            Key::Escape => self.input = None,
            Key::Backspace => {
                input.text.pop();
            }
            Key::Char(c) => input.text.push(c),
            Key::Space => input.text.push(' '),
            Key::Enter => {
                let Some(input) = self.input.take() else { return Action::None };
                let text = input.text.trim().to_string();
                let needs_text = !matches!(input.purpose, InputPurpose::StashMessage(_));
                if needs_text && text.is_empty() {
                    self.input = Some(input);
                    return Action::None;
                }
                return match input.purpose {
                    InputPurpose::NewBranch { switch } => Action::Run(Job::Branch { name: text, start: None, switch }),
                    InputPurpose::Rename(old) => Action::Run(Job::Rename { old, new: text }),
                    InputPurpose::StashMessage(options) => Action::Run(Job::Stash(StashOptions { message: (!text.is_empty()).then_some(text), ..options })),
                    InputPurpose::BranchAt(hash) => Action::Run(Job::Branch { name: text, start: Some(hash), switch: false }),
                    InputPurpose::TagAt(hash) => Action::Run(Job::Tag { name: text, at: hash }),
                    InputPurpose::NewWorktree => {
                        let path = fenix_git::default_worktree_path(&self.root, &text);
                        Action::Run(Job::WorktreeAdd { branch: text, path })
                    }
                    InputPurpose::PushElsewhere => {
                        let Some((remote, branch)) = text.split_once('/') else {
                            return self.message("write it as remote/branch, e.g. origin/review", true);
                        };
                        Action::Run(Job::Push(PushOptions {
                            remote: remote.to_string(),
                            branch: branch.to_string(),
                            set_upstream: false,
                            force_with_lease: self.flags.force_with_lease,
                            dry_run: self.flags.dry_run,
                        }))
                    }
                };
            }
            _ => {}
        }
        Action::None
    }

    fn confirm_key(&mut self, key: Key) -> Action {
        let Some(confirm) = &self.confirm else { return Action::None };
        let pressed = match key {
            Key::Enter => confirm.choices.first().map(|c| c.0),
            Key::Char(c) => Some(c),
            _ => None,
        };
        if let Some(choice) = confirm.choices.iter().find(|c| Some(c.0) == pressed) {
            let job = choice.2.clone();
            self.confirm = None;
            return Action::Run(job);
        }
        if matches!(key, Key::Escape | Key::Char('n') | Key::Char('q')) {
            self.confirm = None;
        }
        Action::None
    }
}

fn verb_of(target: ApplyTarget) -> &'static str {
    match target {
        ApplyTarget::Stage => "stage",
        ApplyTarget::Unstage => "unstage",
        ApplyTarget::Discard => "discard",
    }
}

fn capitalise(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

pub(crate) fn count(n: usize, what: &str) -> String {
    let plural = if what.ends_with("ch") || what.ends_with('s') { format!("{what}es") } else { format!("{what}s") };
    format!("{n} {}", if n == 1 { what.to_string() } else { plural })
}

/// "4 min ago" for a number of seconds.
pub fn ago(secs: u64) -> String {
    match secs / 60 {
        0 => "just now".to_string(),
        m @ 1..=59 => format!("{m} min ago"),
        m @ 60..=1439 => format!("{} h ago", m / 60),
        m => format!("{} d ago", m / 1440),
    }
}

/// The row that was `before`, in a new list: the same row, else the
/// same file, else the same section.
fn find_again(rows: &[Row], before: &Row) -> Option<usize> {
    rows.iter().position(|r| r == before).or_else(|| {
        let path = before.path();
        let section = before.section();
        path.and_then(|p| rows.iter().position(|r| r.path() == Some(p) && matches!(r, Row::File { .. })))
            .or_else(|| section.and_then(|s| rows.iter().position(|r| *r == Row::Section(s))))
    })
}

// -- Layout -------------------------------------------------------------

pub fn layout(page: &GitStatus, cols: usize) -> Page {
    let (left, width) = frame(cols, 150);
    let mut g = Grid::new();
    let mut y = 1;
    let Some(snap) = &page.snap else {
        g.put(y, left, &format!("Reading {}…", page.name), Role::Muted);
        return g.finish();
    };
    let status = snap.status.as_ref();

    // The header: where this branch stands.
    let key = |g: &mut Grid, y: usize, label: &str| g.put(y, left, label, Role::Muted) + (8 - label.len().min(7));
    let x = key(&mut g, y, "Head");
    let x = g.put(y, x, status.map(|s| s.branch.as_str()).unwrap_or("(no branch)"), Role::Accent) + 2;
    if let Some(subject) = &snap.head_subject {
        g.put(y, x, &fit(subject, (left + width).saturating_sub(x)), Role::Text);
    }
    y += 1;
    let x = key(&mut g, y, "Push");
    match status.and_then(|s| s.upstream.as_ref()) {
        Some(upstream) => {
            let s = status.unwrap();
            let x = g.put(y, x, upstream, Role::Text) + 1;
            let x = g.put(y, x, &format!("↑{}", s.ahead), if s.ahead > 0 { Role::Good } else { Role::Muted }) + 1;
            let x = g.put(y, x, &format!("↓{}", s.behind), if s.behind > 0 { Role::Warn } else { Role::Muted }) + 1;
            if let Some(secs) = snap.fetched_secs_ago {
                g.put(y, x, &format!("· fetched {}", ago(secs)), Role::Muted);
            }
        }
        None => match &snap.push_remote {
            Some(remote) => {
                let x = g.put(y, x, &format!("{remote}/{}", status.map(|s| s.branch.as_str()).unwrap_or("")), Role::Muted) + 1;
                g.put(y, x, "not pushed yet · P p creates it", Role::Muted);
            }
            None => {
                g.put(y, x, "no remote", Role::Muted);
            }
        },
    }
    y += 1;
    if let Some((base, ahead, behind)) = &snap.base {
        if status.is_some_and(|s| &s.branch != base) {
            let x = key(&mut g, y, "Base");
            let x = g.put(y, x, base, Role::Text) + 1;
            let x = g.put(y, x, &format!("↑{ahead}"), if *ahead > 0 { Role::Good } else { Role::Muted }) + 1;
            g.put(y, x, &format!("↓{behind}"), if *behind > 0 { Role::Warn } else { Role::Muted });
            y += 1;
        }
    }
    let on_base = snap.base.as_ref().is_some_and(|(base, ..)| status.is_some_and(|s| &s.branch == base));
    match &page.request {
        Some(Some(r)) => {
            let x = key(&mut g, y, "Review");
            let mut x = g.put(y, x, &r.reference, Role::Accent) + 1;
            let mut facts: Vec<(String, Role)> = Vec::new();
            if r.draft {
                facts.push(("draft".to_string(), Role::Warn));
            }
            if let Some(c) = &r.checks {
                facts.push((format!("checks {}", c.label()), if c.is_bad() { Role::Bad } else if *c == PipelineStatus::Success { Role::Good } else { Role::Warn }));
            }
            if !r.approved_by.is_empty() {
                facts.push((format!("approved by {}", r.approved_by.join(", ")), Role::Good));
            }
            if r.unresolved > 0 {
                facts.push((count(r.unresolved, "open thread"), Role::Warn));
            }
            let tail: usize = facts.iter().map(|(t, _)| t.chars().count() + 3).sum::<usize>() + 12;
            x = g.put(y, x, &fit(&r.title, (left + width).saturating_sub(x + tail).max(12)), Role::Text);
            for (text, role) in facts {
                x = g.put(y, x, " · ", Role::Muted);
                x = g.put(y, x, &text, role);
            }
            g.put(y, x + 2, "o reviews it", Role::Muted);
            y += 1;
        }
        Some(None) if !on_base && status.is_some() => {
            let x = key(&mut g, y, "Review");
            g.put(y, x, "no pull request yet · o opens one", Role::Muted);
            y += 1;
        }
        _ => {}
    }
    if let Some(op) = &snap.in_progress {
        y += 1;
        let x = g.put(y, left, op, Role::Bad) + 2;
        g.put(y, x, "r c continue · r a abort · r x resolve", Role::Muted);
        y += 1;
    }
    if let Some(busy) = &page.busy {
        y += 1;
        g.put(y, left, &format!("… {busy}"), Role::Accent);
        y += 1;
    } else if let Some((text, failed)) = &page.message {
        y += 1;
        for (i, line) in crate::page::wrap(text, width).into_iter().take(3).enumerate() {
            g.put(y + i, left, &line, if *failed { Role::Bad } else { Role::Muted });
            y += 1;
        }
        y -= 0;
    }

    let rows = page.rows();
    if rows.is_empty() {
        y += 1;
        g.put(y, left, "Nothing to commit, nothing stashed -- the working tree is clean.", Role::Muted);
        y += 1;
    }
    let selection = page.anchor.map(|a| (a.min(page.cursor), a.max(page.cursor)));
    let mut cursor_line = None;
    for (i, row) in rows.iter().enumerate() {
        if matches!(row, Row::Section(_)) {
            y += 1;
        }
        let line_y = y;
        match row {
            Row::Section(section) => {
                let folded = page.folded.contains(section);
                let n = match section {
                    Section::Stashes => snap.stashes.len(),
                    Section::Unpulled => snap.unpulled.len(),
                    Section::Unpushed => snap.unpushed.len(),
                    Section::Recent => snap.recent.len(),
                    Section::Operations => snap.ops.len(),
                    Section::Worktrees => snap.worktrees.len(),
                    s => files_in(snap, *s).len(),
                };
                let x = g.put(y, left, if folded { "▸" } else { "▾" }, Role::Muted) + 1;
                let title = match section {
                    Section::Unpulled => format!("Unpulled from {}", status.and_then(|s| s.upstream.clone()).unwrap_or_default()),
                    Section::Unpushed => match status.and_then(|s| s.upstream.clone()) {
                        Some(upstream) => format!("Unpushed to {upstream}"),
                        None => format!("Not on {}", snap.base.as_ref().map(|b| b.0.as_str()).unwrap_or("the base")),
                    },
                    s => s.title().to_string(),
                };
                let x = g.put(y, x, &title.to_uppercase(), if *section == Section::Conflicted { Role::Bad } else { Role::Text }) + 1;
                let x = g.put(y, x, &n.to_string(), Role::Muted) + 1;
                if x + 2 < left + width {
                    g.rule(y, x..left + width);
                }
            }
            Row::File { section, path } => {
                let letter = files_in(snap, *section).into_iter().find(|(_, p)| p == path).map(|(l, _)| l).unwrap_or(' ');
                let open = page.expanded.contains(&(*section, path.clone()));
                let x = if section.has_diffs() { g.put(y, left + 2, if open { "▾" } else { "▸" }, Role::Muted) + 1 } else { left + 4 };
                let role = match letter {
                    'A' => Role::Good,
                    'D' | 'C' => Role::Bad,
                    '?' => Role::Muted,
                    _ => Role::Warn,
                };
                let x = g.put(y, x, &letter.to_string(), role) + 2;
                let stats = page.diff(*section, path).map(|d| {
                    let add = d.hunks.iter().flat_map(|h| &h.lines).filter(|l| l.kind == LineKind::Added).count();
                    let del = d.hunks.iter().flat_map(|h| &h.lines).filter(|l| l.kind == LineKind::Removed).count();
                    (add, del)
                });
                let room = (left + width).saturating_sub(x + 12);
                g.put(y, x, &fit(path, room), Role::Text);
                if let Some((add, del)) = stats {
                    let text_add = format!("+{add}");
                    let text_del = format!("−{del}");
                    let end = left + width;
                    let xd = end.saturating_sub(text_del.chars().count());
                    g.put(y, xd, &text_del, Role::Bad);
                    g.put(y, xd.saturating_sub(text_add.len() + 1), &text_add, Role::Good);
                }
                if open {
                    if let Some(DiffState::Loading) = page.diffs.get(&(*section, path.clone())) {
                        g.put(y + 1, left + 6, "reading the diff…", Role::Muted);
                        y += 1;
                    } else if let Some(DiffState::Empty(why)) = page.diffs.get(&(*section, path.clone())) {
                        g.put(y + 1, left + 6, why, Role::Muted);
                        y += 1;
                    }
                }
            }
            Row::Hunk { section, path, hunk } => {
                if let Some(h) = page.diff(*section, path).and_then(|d| d.hunks.get(*hunk)) {
                    g.put(y, left + 6, &fit(&h.header, width.saturating_sub(6)), Role::Accent);
                }
            }
            Row::Line { section, path, hunk, line } => {
                if let Some(l) = page.diff(*section, path).and_then(|d| d.hunks.get(*hunk)).and_then(|h| h.lines.get(*line)) {
                    let number = l.new_line.or(l.old_line).map(|n| n.to_string()).unwrap_or_default();
                    g.put(y, left + 6 + 5usize.saturating_sub(number.len()), &number, Role::Muted);
                    let role = match l.kind {
                        LineKind::Added => Role::Good,
                        LineKind::Removed => Role::Bad,
                        _ => Role::Text,
                    };
                    let text = l.raw().trim_end_matches('\r').replace('\t', "    ");
                    g.put(y, left + 13, &fit(&text, width.saturating_sub(13)), role);
                    if selection.is_some_and(|(a, b)| i >= a && i <= b) {
                        g.panels.push((y, left + 12..left + width));
                    }
                }
            }
            Row::Stash(index) => {
                let message = snap.stashes.iter().find(|s| s.index == *index).map(|s| s.message.as_str()).unwrap_or("");
                let x = g.put(y, left + 4, &format!("stash@{{{index}}}"), Role::Accent) + 2;
                g.put(y, x, &fit(message, (left + width).saturating_sub(x)), Role::Text);
            }
            Row::Worktree(path) => {
                if let Some(w) = snap.worktrees.iter().find(|w| &w.path == path) {
                    let here = fenix_lsp::normalize(path.clone()) == fenix_lsp::normalize(page.root.clone());
                    let x = g.put(y, left + 4, &fit(&path.display().to_string(), width / 2), if here { Role::Title } else { Role::Text }) + 2;
                    let what = match (&w.branch, w.prunable) {
                        (_, true) => "folder gone -- w p prunes it".to_string(),
                        (Some(b), _) => b.clone(),
                        (None, _) => format!("detached at {}", short(&w.head)),
                    };
                    let x = g.put(y, x, &what, if w.prunable { Role::Warn } else { Role::Accent }) + 2;
                    if here {
                        g.put(y, x, "this one", Role::Muted);
                    }
                }
            }
            Row::Op(time) => {
                if let Some(op) = snap.ops.iter().find(|o| o.time == *time) {
                    let when = ago(op.age);
                    let x = g.put(y, left + 4, if op.ok { "✓" } else { "✗" }, if op.ok { Role::Good } else { Role::Bad }) + 2;
                    let tail = if op.undone {
                        "undone"
                    } else if op.undoable && op.ok {
                        "U undoes"
                    } else {
                        ""
                    };
                    let room = (left + width).saturating_sub(x + when.chars().count() + tail.chars().count() + 4);
                    g.put(y, x, &fit(&op.label, room), if op.undone { Role::Muted } else { Role::Text });
                    let x = g.put(y, (left + width).saturating_sub(when.chars().count() + tail.chars().count() + 2), tail, Role::Accent);
                    g.put(y, (left + width).saturating_sub(when.chars().count()).max(x + 1), &when, Role::Muted);
                }
            }
            Row::Commit { section, hash } => {
                if let Some(c) = page.commits(*section).iter().find(|c| &c.hash == hash) {
                    let x = g.put(y, left + 4, &c.short_hash, Role::Accent) + 2;
                    let age = &c.relative_date;
                    let room = (left + width).saturating_sub(x + age.chars().count() + 2);
                    g.put(y, x, &fit(&c.message, room), Role::Text);
                    g.put(y, (left + width).saturating_sub(age.chars().count()), age, Role::Muted);
                }
            }
        }
        if i == page.cursor {
            cursor_line = Some(line_y);
            g.focus(line_y, left..left + width);
            g.popup = overlay(page, line_y, left + 4);
        }
        y += 1;
    }
    let _ = cursor_line;

    let keys: Vec<(&str, &str)> = if page.input.is_some() {
        vec![("Enter", "done"), ("Esc", "cancel")]
    } else if page.confirm.is_some() {
        vec![("Esc", "cancel")]
    } else if page.menu.is_some() {
        vec![("Esc", "close"), ("-x", "flags stay while the page is open")]
    } else if page.anchor.is_some() {
        vec![("s", "stage lines"), ("S", "unstage lines"), ("d", "discard lines"), ("Esc", "clear")]
    } else {
        vec![
            ("Tab", "fold"),
            ("s", "stage"),
            ("S", "unstage"),
            ("d", "discard"),
            ("c", "commit…"),
            ("P", "push…"),
            ("p", "pull…"),
            ("b", "branch…"),
            ("z", "stash…"),
            ("l", "log…"),
            ("r", "rebase…"),
            ("U", "undo"),
            ("?", "all keys"),
        ]
    };
    g.keys(left, width, &keys);
    g.finish()
}

/// The open menu, field or question, as a popup beside page line
/// `line`.
fn overlay(page: &GitStatus, line: usize, col: usize) -> Option<Popup> {
    let menu = page.menu.as_ref().map(|state| (page.menu(state), state.typed.clone()));
    popup(line, col, menu.as_ref().map(|(m, t)| (m, t.as_str())), page.input.as_ref(), page.confirm.as_ref())
}

/// A menu (with the keys typed so far), a field or a question as a
/// popup beside page line `line`, its text lined up with column `col`
/// -- shared by every Git page. `None` when nothing is open.
pub(crate) fn popup(line: usize, col: usize, menu: Option<(&Menu, &str)>, input: Option<&Input>, confirm: Option<&Confirm>) -> Option<Popup> {
    let mut rows: Vec<Vec<(String, Role)>> = Vec::new();
    if let Some((menu, typed)) = menu {
        // Wide enough for the widest row: key, label, and its detail
        // pushed to the right edge.
        const KEY: usize = 7;
        let widest = menu
            .groups
            .iter()
            .flat_map(|(_, items)| items)
            .map(|item| {
                let detail = match item.flag {
                    Some(_) => 3,
                    None => item.detail.chars().count(),
                };
                KEY + item.label.chars().count() + if detail > 0 { detail + 4 } else { 0 }
            })
            .chain([menu.title.chars().count()])
            .chain(menu.groups.iter().filter_map(|(t, _)| t.as_ref().map(|t| t.chars().count())))
            .max()
            .unwrap_or(0);
        let w = widest.clamp(28, 72);
        rows.push(vec![(fit(&menu.title, w), Role::Title)]);
        for (title, items) in &menu.groups {
            rows.push(Vec::new());
            if let Some(title) = title {
                rows.push(vec![(title.to_uppercase(), Role::Muted)]);
            }
            for item in items {
                let typed = !typed.is_empty() && item.key.starts_with(typed);
                let key_role = if typed { Role::Title } else if item.danger { Role::Bad } else { Role::Accent };
                let mut row = vec![(format!("{:<KEY$}", item.key), key_role)];
                let label = fit(&item.label, w.saturating_sub(KEY));
                let used = KEY + label.chars().count();
                row.push((label, Role::Text));
                let (detail, role) = match item.flag {
                    Some(true) => ("on".to_string(), Role::Good),
                    Some(false) => ("off".to_string(), Role::Muted),
                    None => (item.detail.clone(), Role::Muted),
                };
                let room = w.saturating_sub(used + 2);
                if !detail.is_empty() && room > 3 {
                    let d = fit(&detail, room);
                    row.push((" ".repeat(w - used - d.chars().count()), Role::Text));
                    row.push((d, role));
                }
                rows.push(row);
            }
        }
    } else if let Some(input) = input {
        let field = format!("{}▏", input.text);
        let used = input.label.chars().count() + 3 + field.chars().count();
        rows.push(vec![(input.label.clone(), Role::Muted), (" › ".to_string(), Role::Accent), (field, Role::Text), (" ".repeat(44usize.saturating_sub(used)), Role::Text)]);
    } else {
        let confirm = confirm?;
        rows.push(vec![(confirm.question.clone(), Role::Warn)]);
        for (role, line) in &confirm.detail {
            rows.push(vec![(format!("  {}", fit(line, 72)), *role)]);
        }
        rows.push(Vec::new());
        let mut choices = Vec::new();
        for (key, label, _) in &confirm.choices {
            choices.push((key.to_string(), Role::Accent));
            choices.push((format!(" {label}   "), Role::Text));
        }
        choices.push(("n".to_string(), Role::Accent));
        choices.push((" cancel".to_string(), Role::Text));
        rows.push(choices);
    }
    Some(Popup { line, col, rows })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, index: char, worktree: char) -> FileEntry {
        FileEntry { path: path.to_string(), index_status: index, worktree_status: worktree }
    }

    fn commit(hash: &str, message: &str) -> Commit {
        Commit { hash: hash.to_string(), short_hash: hash[..7].to_string(), message: message.to_string(), author: "t".into(), relative_date: "2 hours ago".into() }
    }

    fn snapshot() -> Snapshot {
        Snapshot {
            status: Some(RepoStatus { branch: "feature/git".into(), upstream: Some("origin/feature/git".into()), ahead: 2, behind: 0 }),
            head_subject: Some("Git: a status page".into()),
            push_remote: Some("origin".into()),
            base: Some(("main".into(), 7, 1)),
            fetched_secs_ago: Some(240),
            files: vec![file("new.txt", '?', '?'), file("src/a.rs", '.', 'M'), file("README.md", 'M', '.'), file("both.rs", 'M', 'M')],
            stashes: vec![Stash { index: 0, message: "WIP on main".into() }],
            unpulled: Vec::new(),
            unpushed: vec![commit("4d1e9a0aaaa", "sections fold"), commit("b72c311bbbb", "a status page")],
            recent: vec![commit("4d1e9a0aaaa", "sections fold"), commit("b72c311bbbb", "a status page"), commit("7d35b6fcccc", "older")],
            in_progress: None,
            gone: vec!["feature/old".into()],
            ops: Vec::new(),
            worktrees: Vec::new(),
        }
    }

    fn page() -> GitStatus {
        let mut p = GitStatus::new(PathBuf::from("/repo/fenix"));
        p.set_snapshot(snapshot());
        p
    }

    const DIFF: &str = "diff --git a/src/a.rs b/src/a.rs\n--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1,3 +1,4 @@\n one\n-two\n+TWO\n+extra\n three\n";

    fn goto(p: &mut GitStatus, row: Row) {
        p.cursor = p.rows().iter().position(|r| *r == row).unwrap_or_else(|| panic!("{row:?} not in {:?}", p.rows()));
    }

    fn check(status: PipelineStatus) -> Check {
        Check { id: "1".into(), name: "ci".into(), group: String::new(), status, url: String::new(), seconds: None }
    }

    #[test]
    fn checks_add_up_to_the_worst_of_them() {
        assert_eq!(overall(&[]), None);
        assert_eq!(overall(&[check(PipelineStatus::Success), check(PipelineStatus::Skipped)]), Some(PipelineStatus::Success));
        assert_eq!(overall(&[check(PipelineStatus::Success), check(PipelineStatus::Pending)]), Some(PipelineStatus::Running));
        assert_eq!(overall(&[check(PipelineStatus::Running), check(PipelineStatus::Canceled)]), Some(PipelineStatus::Failed));
    }

    #[test]
    fn the_header_says_how_the_branchs_pull_request_stands_and_o_goes_to_it() {
        let mut p = page();
        assert!(!layout(&p, 120).text.contains("Review"), "nothing until the forge has been asked");
        p.request = Some(None);
        assert!(layout(&p, 120).text.contains("Review  no pull request yet · o opens one"));
        p.request = Some(Some(RequestLine {
            number: 22,
            reference: "#22".into(),
            title: "Git: a daily driver".into(),
            draft: true,
            checks: Some(PipelineStatus::Failed),
            approved_by: vec!["alex".into()],
            unresolved: 2,
        }));
        let text = layout(&p, 140).text;
        assert!(text.contains("Review  #22 Git: a daily driver · draft · checks failed · approved by alex · 2 open threads  o reviews it"), "{text}");
        assert_eq!(p.key(Key::Char('o')), Action::PullRequest);
        assert_eq!(p.key(Key::Char('M')), Action::Reviews);
    }

    #[test]
    fn files_land_in_the_right_sections() {
        let snap = snapshot();
        assert_eq!(files_in(&snap, Section::Untracked), [('?', "new.txt".to_string())]);
        assert_eq!(files_in(&snap, Section::Unstaged).iter().map(|f| f.1.as_str()).collect::<Vec<_>>(), ["src/a.rs", "both.rs"]);
        assert_eq!(files_in(&snap, Section::Staged).iter().map(|f| f.1.as_str()).collect::<Vec<_>>(), ["README.md", "both.rs"]);
        let conflict = Snapshot { files: vec![file("x", 'U', 'U')], ..Default::default() };
        assert_eq!(files_in(&conflict, Section::Conflicted).len(), 1);
        assert!(files_in(&conflict, Section::Unstaged).is_empty(), "a conflicted file is only in Conflicted");
    }

    #[test]
    fn stashes_and_recent_start_folded_and_tab_toggles_a_section() {
        let mut p = page();
        let rows = p.rows();
        assert!(rows.contains(&Row::Section(Section::Stashes)));
        assert!(!rows.contains(&Row::Stash(0)), "folded");
        goto(&mut p, Row::Section(Section::Stashes));
        p.key(Key::Tab);
        assert!(p.rows().contains(&Row::Stash(0)));
    }

    #[test]
    fn tab_on_a_file_asks_for_its_diff_and_shows_it_inline() {
        let mut p = page();
        let file = Row::File { section: Section::Unstaged, path: "src/a.rs".into() };
        goto(&mut p, file.clone());
        assert_eq!(p.key(Key::Tab), Action::LoadDiff { section: Section::Unstaged, path: "src/a.rs".into() });
        let diff = fenix_diff::parse(DIFF).remove(0);
        p.set_diff(Section::Unstaged, "src/a.rs".into(), DiffState::Loaded(Box::new(diff)));
        let rows = p.rows();
        assert!(rows.contains(&Row::Hunk { section: Section::Unstaged, path: "src/a.rs".into(), hunk: 0 }));
        assert_eq!(rows.iter().filter(|r| matches!(r, Row::Line { .. })).count(), 5);
        assert_eq!(rows[p.cursor], file, "the cursor stays on the file");
        let text = layout(&p, 120).text;
        assert!(text.contains("+extra") && text.contains("@@ -1,3 +1,4 @@"), "{text}");
    }

    fn expanded() -> GitStatus {
        let mut p = page();
        goto(&mut p, Row::File { section: Section::Unstaged, path: "src/a.rs".into() });
        p.key(Key::Tab);
        p.set_diff(Section::Unstaged, "src/a.rs".into(), DiffState::Loaded(Box::new(fenix_diff::parse(DIFF).remove(0))));
        p
    }

    #[test]
    fn s_stages_a_file_a_hunk_or_the_selected_lines() {
        let mut p = expanded();
        assert_eq!(p.key(Key::Char('s')), Action::Run(Job::Stage(vec!["src/a.rs".into()])));

        goto(&mut p, Row::Hunk { section: Section::Unstaged, path: "src/a.rs".into(), hunk: 0 });
        let Action::Run(Job::Apply { patch, target: ApplyTarget::Stage, .. }) = p.key(Key::Char('s')) else { panic!() };
        assert!(patch.contains("+extra") && patch.contains("+TWO"));

        // V over "+TWO" and "+extra", then s: both lines, not "-two".
        goto(&mut p, Row::Line { section: Section::Unstaged, path: "src/a.rs".into(), hunk: 0, line: 2 });
        p.key(Key::Char('V'));
        p.key(Key::Char('j'));
        let Action::Run(Job::Apply { patch, what, .. }) = p.key(Key::Char('s')) else { panic!() };
        assert_eq!(what, "stage 2 lines of src/a.rs");
        assert!(patch.contains(" two\n"), "the unselected removal stays as context:\n{patch}");
        assert!(p.anchor.is_none(), "the selection is spent");
    }

    #[test]
    fn s_on_a_staged_row_says_to_use_capital_s() {
        let mut p = page();
        goto(&mut p, Row::File { section: Section::Staged, path: "README.md".into() });
        assert_eq!(p.key(Key::Char('s')), Action::None);
        assert!(p.message.as_ref().unwrap().0.contains("S unstages"));
        assert_eq!(p.key(Key::Char('S')), Action::Run(Job::Unstage(vec!["README.md".into()])));
    }

    #[test]
    fn discarding_asks_first_and_names_untracked_files_as_deleted() {
        let mut p = page();
        goto(&mut p, Row::File { section: Section::Untracked, path: "new.txt".into() });
        assert_eq!(p.key(Key::Char('d')), Action::None);
        let confirm = p.confirm.as_ref().unwrap();
        assert!(confirm.question.contains("Delete 1 file"), "{}", confirm.question);
        assert_eq!(p.key(Key::Char('n')), Action::None);
        assert!(p.confirm.is_none(), "n cancels");
        p.key(Key::Char('d'));
        assert_eq!(p.key(Key::Char('y')), Action::Run(Job::Discard(vec![("new.txt".into(), true)])));
    }

    #[test]
    fn a_menu_opens_under_the_row_and_its_second_key_runs_the_verb() {
        let mut p = page();
        goto(&mut p, Row::Section(Section::Staged));
        p.key(Key::Char('c'));
        let page = layout(&p, 120);
        let popup = page.popup.as_ref().expect("the menu floats over the page");
        assert!(popup.text().contains("Commit · 2 files staged") && popup.text().contains("amend"), "{}", popup.text());
        assert!(!page.text.contains("Commit · 2 files staged"), "and isn't written into it");
        assert_eq!(popup.line, page.focus.as_ref().unwrap().0, "beside the row it's about");
        assert_eq!(p.key(Key::Char('c')), Action::Compose(Compose::New(CommitFlags::default())));
        assert!(p.menu.is_none());
        assert!(layout(&p, 120).popup.is_none(), "gone once it's used");
    }

    #[test]
    fn flags_toggle_in_place_and_stay_for_the_next_menu() {
        let mut p = page();
        p.key(Key::Char('c'));
        p.key(Key::Char('-'));
        assert_eq!(p.menu.as_ref().unwrap().typed, "-", "waiting for the second key");
        p.key(Key::Char('s'));
        assert!(p.menu.is_some(), "a flag doesn't close the menu");
        assert!(p.flags.commit.signoff);
        assert_eq!(p.key(Key::Char('e')), Action::Run(Job::Commit(CommitKind::Extend, CommitFlags { signoff: true, ..Default::default() })));
    }

    #[test]
    fn a_first_push_sets_the_upstream() {
        let mut p = page();
        let mut snap = snapshot();
        snap.status.as_mut().unwrap().upstream = None;
        p.set_snapshot(snap);
        assert!(layout(&p, 120).text.contains("not pushed yet"));
        p.key(Key::Char('P'));
        let Action::Run(Job::Push(options)) = p.key(Key::Char('p')) else { panic!() };
        assert_eq!((options.remote.as_str(), options.branch.as_str(), options.set_upstream), ("origin", "feature/git", true));
    }

    #[test]
    fn a_diverged_push_shows_the_plan_and_offers_the_lease() {
        let mut p = page();
        let mut snap = snapshot();
        snap.status.as_mut().unwrap().behind = 3;
        snap.unpulled = vec![commit("0c9e2b7dddd", "fixup! old")];
        p.set_snapshot(snap);
        p.key(Key::Char('P'));
        assert_eq!(p.key(Key::Char('p')), Action::None);
        let text = layout(&p, 120).all_text();
        assert!(text.contains("have diverged") && text.contains("fixup! old") && text.contains("force-with-lease"), "{text}");
        let Action::Run(Job::Push(options)) = p.key(Key::Char('f')) else { panic!() };
        assert!(options.force_with_lease && !options.set_upstream);
    }

    #[test]
    fn fixup_needs_a_commit_under_the_cursor_and_something_staged() {
        let mut p = page();
        p.folded.remove(&Section::Recent);
        goto(&mut p, Row::Section(Section::Staged));
        p.key(Key::Char('c'));
        assert_eq!(p.key(Key::Char('F')), Action::None);
        assert!(p.message.as_ref().unwrap().0.contains("put the cursor on the commit"));
        goto(&mut p, Row::Commit { section: Section::Unpushed, hash: "b72c311bbbb".into() });
        p.key(Key::Char('c'));
        assert_eq!(p.key(Key::Char('F')), Action::Run(Job::FixupNow { hash: "b72c311bbbb".into(), flags: CommitFlags::default() }));
    }

    #[test]
    fn enter_on_a_commit_opens_its_menu_and_a_hard_reset_asks_first() {
        let mut p = page();
        goto(&mut p, Row::Commit { section: Section::Unpushed, hash: "b72c311bbbb".into() });
        p.key(Key::Enter);
        assert_eq!(p.menu.as_ref().unwrap().kind, MenuKind::CommitRow);
        p.key(Key::Char('r'));
        assert_eq!(p.menu.as_ref().unwrap().kind, MenuKind::Reset);
        assert_eq!(p.key(Key::Char('h')), Action::None);
        assert_eq!(p.key(Key::Char('y')), Action::Run(Job::Reset { target: "b72c311bbbb".into(), mode: ResetMode::Hard }));
    }

    #[test]
    fn a_new_branch_is_named_in_a_field_on_the_page() {
        let mut p = page();
        p.key(Key::Char('b'));
        p.key(Key::Char('c'));
        assert!(p.typing());
        p.type_text("topic");
        assert!(layout(&p, 120).all_text().contains("New branch › topic"));
        assert_eq!(p.key(Key::Enter), Action::Run(Job::Branch { name: "topic".into(), start: None, switch: true }));
    }

    #[test]
    fn the_header_says_where_the_branch_stands() {
        let text = layout(&page(), 120).text;
        assert!(text.contains("feature/git") && text.contains("Git: a status page"), "{text}");
        assert!(text.contains("origin/feature/git ↑2 ↓0 · fetched 4 min ago"), "{text}");
        assert!(text.contains("main ↑7 ↓1"), "{text}");
        assert!(text.contains("UNPUSHED TO ORIGIN/FEATURE/GIT 2"), "{text}");
    }

    #[test]
    fn the_cursor_stays_on_its_row_across_a_refresh() {
        let mut p = page();
        goto(&mut p, Row::File { section: Section::Staged, path: "README.md".into() });
        let mut snap = snapshot();
        snap.files.retain(|f| f.path != "new.txt");
        p.set_snapshot(snap);
        assert_eq!(p.rows()[p.cursor], Row::File { section: Section::Staged, path: "README.md".into() });
    }

    #[test]
    fn a_suspended_operation_leads_the_page() {
        let mut p = page();
        let mut snap = snapshot();
        snap.in_progress = Some("REBASING 3/7".into());
        p.set_snapshot(snap);
        assert!(layout(&p, 120).text.contains("REBASING 3/7  r c continue"));
        p.key(Key::Char('r'));
        assert_eq!(p.key(Key::Char('c')), Action::Run(Job::Continue));
    }

    #[test]
    fn the_operation_log_lists_what_ran_and_u_undoes_the_one_under_the_cursor() {
        let mut p = page();
        let mut snap = snapshot();
        snap.ops = vec![
            Op { time: 20, label: "reset --hard to e81a4f2".into(), ok: true, undoable: true, undone: false, age: 40 },
            Op { time: 10, label: "push to origin/feature/git".into(), ok: true, undoable: false, undone: false, age: 600 },
        ];
        p.set_snapshot(snap);
        assert_eq!(p.key(Key::Char('U')), Action::PlanUndo(None), "U anywhere else: the latest");
        p.show_operations();
        let text = layout(&p, 120).text;
        assert!(text.contains("OPERATIONS 2") && text.contains("reset --hard to e81a4f2") && text.contains("U undoes"), "{text}");
        p.key(Key::Char('j'));
        p.key(Key::Char('j'));
        assert_eq!(p.key(Key::Char('U')), Action::PlanUndo(Some(10)));
    }

    #[test]
    fn the_page_keeps_to_its_width() {
        let mut p = expanded();
        p.key(Key::Char('c'));
        let page = layout(&p, 60);
        assert!(page.text.lines().all(|l| l.chars().count() <= 60), "{}", page.text);
    }
}
