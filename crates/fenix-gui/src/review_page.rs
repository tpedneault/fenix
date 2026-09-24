//! The review page: one merge or pull request, read file by file and
//! reviewed as a whole. Files are ticked off as viewed (`v`); comments
//! written here wait as a pending review, visible only to you, until
//! `s` submits them together with a verdict. Threads sit under the line
//! they're about; `]t` walks the open ones. `i` narrows the diff to what
//! changed since your last review. `m` merges, once.
//!
//! Pure layout and decisions; the host (`app/review_host.rs`) talks to
//! the forge and remembers the review (`review_store`).

use std::collections::HashSet;
use std::path::PathBuf;

use fenix_diff::{FileDiff, LineKind};
use fenix_forge::{Approvals, Check, Discussion, MergeOptions, MergeRequest, PipelineStatus, Verdict};

use crate::git_status::{count, draw_popups, short, Menu, MenuItem};
use crate::page::{fit, frame, wrap, Grid, Key, Page, Role};
use crate::review_store::{fingerprint, Pending, ReviewState};

/// One changed file, with its diff parsed.
#[derive(Debug, Clone)]
pub struct FileView {
    pub path: String,
    pub old_path: String,
    pub letter: char,
    pub diff: Option<FileDiff>,
    /// A fingerprint of the diff text, for the viewed marks.
    pub print: u64,
}

impl FileView {
    pub fn new(path: String, old_path: String, letter: char, text: &str) -> Self {
        let diff = fenix_diff::parse(text).into_iter().next();
        FileView { path, old_path, letter, diff, print: fingerprint(text) }
    }

    fn stats(&self) -> (usize, usize) {
        let lines = self.diff.iter().flat_map(|d| &d.hunks).flat_map(|h| &h.lines);
        let (mut add, mut del) = (0, 0);
        for l in lines {
            match l.kind {
                LineKind::Added => add += 1,
                LineKind::Removed => del += 1,
                _ => {}
            }
        }
        (add, del)
    }
}

/// Everything the page shows, read from the forge off the UI thread.
#[derive(Debug, Clone)]
pub struct ReviewData {
    pub request: MergeRequest,
    pub approvals: Option<Approvals>,
    pub files: Vec<FileView>,
    pub threads: Vec<Discussion>,
    pub checks: Vec<Check>,
    /// The diff since the head last reviewed at, worked out locally.
    pub since: Option<Vec<FileView>>,
    /// Something that failed without stopping the rest.
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Section {
    Description,
    Files,
    Conversations,
    Checks,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row {
    Section(Section),
    Description(usize),
    File(String),
    Hunk { path: String, hunk: usize },
    Line { path: String, hunk: usize, line: usize },
    /// A note of a thread under a diff line.
    Note { thread: String, note: usize },
    Pending(usize),
    Conversation(String),
    Check(String),
}

/// What the page asks of its host.
#[derive(Debug, Clone, PartialEq)]
pub enum ReviewAction {
    None,
    Refresh,
    Close,
    /// Write a new pending comment (or edit pending comment `index`).
    Comment { pending: Pending, index: Option<usize> },
    /// Reply to a thread -- sent straight away, like a reply.
    Reply(String),
    Resolve { thread: String, resolved: bool },
    /// Write the review's summary.
    Summary,
    Submit { verdict: Verdict, body: String },
    Merge(MergeOptions),
    OpenFile { path: String, line: Option<usize> },
    OpenBrowser,
    CopyUrl,
    ReviewInWorktree,
    CheckOutHere,
    ShowLog(Check),
    Retry(Check),
    /// The review state changed (viewed marks, pending comments): save it.
    Save,
    /// Asks for the diff since the last review.
    LoadSince,
}

#[derive(Debug, Clone, PartialEq)]
enum Mode {
    Normal,
    Submit { verdict: Verdict, armed: bool },
    Merge { options: MergeOptions, armed: bool },
}

pub struct ReviewPage {
    pub root: PathBuf,
    pub forge: String,
    pub project: String,
    pub number: u64,
    pub data: Option<ReviewData>,
    pub state: ReviewState,
    pub summary: String,
    pub cursor: usize,
    pub open: HashSet<String>,
    pub folded: HashSet<Section>,
    pub since_review: bool,
    /// Reviewing in a worktree of its own.
    pub in_worktree: bool,
    mode: Mode,
    anchor: Option<usize>,
    pub message: Option<(String, bool)>,
    pub busy: Option<String>,
    pub loading: bool,
    pending_g: bool,
    pending_bracket: Option<char>,
}

impl ReviewPage {
    pub fn new(root: PathBuf, forge: String, project: String, number: u64, state: ReviewState) -> Self {
        ReviewPage {
            root,
            forge,
            project,
            number,
            data: None,
            state,
            summary: String::new(),
            cursor: 0,
            open: HashSet::new(),
            folded: [Section::Description].into_iter().collect(),
            since_review: false,
            in_worktree: false,
            mode: Mode::Normal,
            anchor: None,
            message: None,
            busy: None,
            loading: false,
            pending_g: false,
            pending_bracket: None,
        }
    }

    pub fn reference(&self) -> String {
        self.data.as_ref().map(|d| d.request.reference()).unwrap_or_else(|| format!("#{}", self.number))
    }

    pub fn set_data(&mut self, data: ReviewData) {
        self.loading = false;
        let before = self.rows().get(self.cursor).cloned();
        self.data = Some(data);
        let rows = self.rows();
        self.cursor = before.and_then(|b| rows.iter().position(|r| *r == b)).unwrap_or(self.cursor).min(rows.len().saturating_sub(1));
    }

    /// The files shown: the whole change, or what changed since you last
    /// reviewed.
    fn files(&self) -> &[FileView] {
        match (&self.data, self.since_review) {
            (Some(d), true) => d.since.as_deref().unwrap_or(&d.files),
            (Some(d), false) => &d.files,
            (None, _) => &[],
        }
    }

    fn file(&self, path: &str) -> Option<&FileView> {
        self.files().iter().find(|f| f.path == path)
    }

    pub fn viewed(&self, file: &FileView) -> bool {
        self.state.viewed.get(&file.path) == Some(&file.print)
    }

    fn threads(&self) -> &[Discussion] {
        self.data.as_ref().map(|d| d.threads.as_slice()).unwrap_or_default()
    }

    fn thread(&self, id: &str) -> Option<&Discussion> {
        self.threads().iter().find(|t| t.id == id)
    }

    /// The threads under one diff line of `file`, shown after it.
    fn threads_at(&self, file: &FileView, old: Option<usize>, new: Option<usize>) -> Vec<&Discussion> {
        self.threads()
            .iter()
            .filter(|t| t.is_human())
            .filter(|t| t.position.as_ref().is_some_and(|p| (p.new_path == file.path || p.old_path == file.old_path) && p.anchors_to(old, new)))
            .collect()
    }

    fn pending_at(&self, file: &FileView, old: Option<usize>, new: Option<usize>) -> Vec<usize> {
        self.state
            .pending
            .iter()
            .enumerate()
            .filter(|(_, p)| p.path == file.path && ((p.new_line.is_some() && p.new_line == new) || (p.new_line.is_none() && p.old_line.is_some() && p.old_line == old)))
            .map(|(i, _)| i)
            .collect()
    }

    pub fn rows(&self) -> Vec<Row> {
        let Some(data) = &self.data else { return Vec::new() };
        let mut rows = vec![Row::Section(Section::Description)];
        if !self.folded.contains(&Section::Description) {
            let lines = data.request.description.lines().count().max(1);
            rows.extend((0..lines.min(40)).map(Row::Description));
        }
        rows.push(Row::Section(Section::Files));
        if !self.folded.contains(&Section::Files) {
            for file in self.files() {
                rows.push(Row::File(file.path.clone()));
                if !self.open.contains(&file.path) {
                    continue;
                }
                let Some(diff) = &file.diff else { continue };
                let mut shown: HashSet<String> = HashSet::new();
                for (h, hunk) in diff.hunks.iter().enumerate() {
                    rows.push(Row::Hunk { path: file.path.clone(), hunk: h });
                    for (l, line) in hunk.lines.iter().enumerate() {
                        rows.push(Row::Line { path: file.path.clone(), hunk: h, line: l });
                        for t in self.threads_at(file, line.old_line, line.new_line) {
                            if shown.insert(t.id.clone()) {
                                rows.extend((0..t.notes.len()).map(|n| Row::Note { thread: t.id.clone(), note: n }));
                            }
                        }
                        rows.extend(self.pending_at(file, line.old_line, line.new_line).into_iter().map(Row::Pending));
                    }
                }
            }
        }
        rows.push(Row::Section(Section::Conversations));
        if !self.folded.contains(&Section::Conversations) {
            rows.extend(self.threads().iter().filter(|t| t.is_human()).map(|t| Row::Conversation(t.id.clone())));
        }
        rows.push(Row::Section(Section::Checks));
        if !self.folded.contains(&Section::Checks) {
            rows.extend(data.checks.iter().map(|c| Row::Check(c.id.clone())));
        }
        rows
    }

    fn diff_line(&self, path: &str, hunk: usize, line: usize) -> Option<&fenix_diff::DiffLine> {
        self.file(path)?.diff.as_ref()?.hunks.get(hunk)?.lines.get(line)
    }

    fn say(&mut self, text: impl Into<String>, failed: bool) -> ReviewAction {
        self.message = Some((text.into(), failed));
        ReviewAction::None
    }

    /// The pending comment the cursor (and a Visual range) would write.
    fn new_comment(&self, rows: &[Row], suggestion: bool) -> Option<Pending> {
        // A selection's lines come from its range -- threads shown between
        // them are skipped -- starting from the line it was begun on.
        let start = self.anchor.unwrap_or(self.cursor);
        let Some(Row::Line { path, hunk, line }) = rows.get(start) else { return None };
        let file = self.file(path)?;
        let here = self.diff_line(path, *hunk, *line)?;
        let mut start_line = None;
        let mut selected = vec![here.clone()];
        if let Some(anchor) = self.anchor {
            let (a, b) = (anchor.min(self.cursor), anchor.max(self.cursor));
            selected = rows[a..=b]
                .iter()
                .filter_map(|r| match r {
                    Row::Line { path: p, hunk: h, line } if p == path && h == hunk => self.diff_line(p, *h, *line).cloned(),
                    _ => None,
                })
                .collect();
            start_line = selected.iter().filter_map(|l| l.new_line.or(l.old_line)).min();
        }
        let last = selected.last().cloned().unwrap_or(here.clone());
        let (old_line, new_line) = match last.kind {
            LineKind::Removed => (last.old_line, None),
            _ => (None, last.new_line),
        };
        let end = new_line.or(old_line);
        let body = if suggestion {
            let text: Vec<String> = selected.iter().filter(|l| l.kind != LineKind::Removed).map(|l| l.text.trim_end_matches('\r').to_string()).collect();
            format!("```suggestion\n{}\n```", text.join("\n"))
        } else {
            String::new()
        };
        Some(Pending { path: file.path.clone(), old_path: file.old_path.clone(), old_line, new_line, start_line: start_line.filter(|s| Some(*s) != end), body })
    }

    pub fn key(&mut self, key: Key) -> ReviewAction {
        match self.mode.clone() {
            Mode::Submit { verdict, armed } => return self.submit_key(key, verdict, armed),
            Mode::Merge { options, armed } => return self.merge_key(key, options, armed),
            Mode::Normal => {}
        }
        let rows = self.rows();
        if let Some(bracket) = self.pending_bracket.take() {
            return match (bracket, key) {
                (b, Key::Char('f')) => self.jump(&rows, b == ']', |r| matches!(r, Row::File(_))),
                (b, Key::Char('t')) => {
                    let open: HashSet<String> = self.threads().iter().filter(|t| t.resolvable && !t.resolved).map(|t| t.id.clone()).collect();
                    self.jump(&rows, b == ']', |r| matches!(r, Row::Note { thread, note: 0 } if open.contains(thread)))
                }
                _ => ReviewAction::None,
            };
        }
        let g = std::mem::take(&mut self.pending_g);
        let n = rows.len();
        match key {
            Key::Down | Key::Char('j') => self.cursor = (self.cursor + 1).min(n.saturating_sub(1)),
            Key::Up | Key::Char('k') => self.cursor = self.cursor.saturating_sub(1),
            Key::Char('g') if g => self.cursor = 0,
            Key::Char('g') => self.pending_g = true,
            Key::Char('G') => self.cursor = n.saturating_sub(1),
            Key::Char(']') | Key::Char('[') => {
                if let Key::Char(c) = key {
                    self.pending_bracket = Some(c);
                }
            }
            Key::Escape => self.anchor = None,
            Key::Tab => return self.toggle(&rows),
            Key::Enter => return self.enter(&rows),
            Key::Char('V') => {
                if matches!(rows.get(self.cursor), Some(Row::Line { .. })) {
                    self.anchor = if self.anchor.is_some() { None } else { Some(self.cursor) };
                }
            }
            Key::Char('v') => return self.mark_viewed(&rows),
            Key::Char('C') | Key::Char('S') => {
                let Some(pending) = self.new_comment(&rows, key == Key::Char('S')) else {
                    return self.say("put the cursor on a line of a diff (Tab opens a file), then C comments -- S suggests", false);
                };
                self.anchor = None;
                return ReviewAction::Comment { pending, index: None };
            }
            Key::Char('e') => {
                if let Some(Row::Pending(i)) = rows.get(self.cursor) {
                    return ReviewAction::Comment { pending: self.state.pending[*i].clone(), index: Some(*i) };
                }
            }
            Key::Char('x') => {
                if let Some(Row::Pending(i)) = rows.get(self.cursor) {
                    self.state.pending.remove(*i);
                    return ReviewAction::Save;
                }
            }
            Key::Char('r') => match rows.get(self.cursor) {
                Some(Row::Note { thread, .. } | Row::Conversation(thread)) => return ReviewAction::Reply(thread.clone()),
                Some(Row::Check(id)) => {
                    if let Some(check) = self.data.as_ref().and_then(|d| d.checks.iter().find(|c| &c.id == id)) {
                        return ReviewAction::Retry(check.clone());
                    }
                }
                _ => return self.say("r replies on a thread, or reruns a check", false),
            },
            Key::Char('R') => match rows.get(self.cursor) {
                Some(Row::Note { thread, .. } | Row::Conversation(thread)) => {
                    let Some(t) = self.thread(thread) else { return ReviewAction::None };
                    if !t.resolvable {
                        return self.say("a comment on the whole request can't be resolved", false);
                    }
                    return ReviewAction::Resolve { thread: thread.clone(), resolved: !t.resolved };
                }
                _ => return self.say("R resolves the thread under the cursor", false),
            },
            Key::Char('i') => {
                if self.since_review {
                    self.since_review = false;
                    return ReviewAction::None;
                }
                let head = self.data.as_ref().map(|d| d.request.sha.clone()).unwrap_or_default();
                return match &self.state.reviewed_head {
                    None => self.say("no review of yours yet -- this is the whole change", false),
                    Some(r) if *r == head => self.say("nothing changed since your review", false),
                    Some(_) => {
                        self.since_review = true;
                        self.cursor = 0;
                        ReviewAction::LoadSince
                    }
                };
            }
            Key::Char('s') => {
                self.mode = Mode::Submit { verdict: Verdict::Comment, armed: false };
            }
            Key::Char('m') => {
                self.mode = Mode::Merge { options: MergeOptions { sha: self.data.as_ref().map(|d| d.request.sha.clone()), ..Default::default() }, armed: false };
            }
            Key::Char('K') => {
                self.folded.remove(&Section::Checks);
                self.cursor = self.rows().iter().position(|r| *r == Row::Section(Section::Checks)).unwrap_or(self.cursor);
            }
            Key::Char('w') => return ReviewAction::ReviewInWorktree,
            Key::Char('c') => return ReviewAction::CheckOutHere,
            Key::Char('o') => return ReviewAction::OpenBrowser,
            Key::Char('y') => return ReviewAction::CopyUrl,
            Key::Char('u') => return ReviewAction::Refresh,
            Key::Char('q') => return ReviewAction::Close,
            _ => {}
        }
        ReviewAction::None
    }

    fn jump(&mut self, rows: &[Row], forward: bool, is: impl Fn(&Row) -> bool) -> ReviewAction {
        let found = if forward {
            rows.iter().enumerate().skip(self.cursor + 1).find(|(_, r)| is(r)).map(|(i, _)| i)
        } else {
            rows.iter().enumerate().take(self.cursor).rev().find(|(_, r)| is(r)).map(|(i, _)| i)
        };
        match found {
            Some(i) => {
                self.cursor = i;
                ReviewAction::None
            }
            None => self.say(if forward { "nothing further down" } else { "nothing further up" }, false),
        }
    }

    fn toggle(&mut self, rows: &[Row]) -> ReviewAction {
        match rows.get(self.cursor).cloned() {
            Some(Row::Section(s)) => {
                if !self.folded.remove(&s) {
                    self.folded.insert(s);
                }
            }
            Some(Row::File(path)) => {
                if !self.open.remove(&path) {
                    self.open.insert(path);
                }
            }
            Some(Row::Hunk { path, .. } | Row::Line { path, .. }) => {
                self.open.remove(&path);
                self.cursor = self.rows().iter().position(|r| *r == Row::File(path.clone())).unwrap_or(self.cursor);
            }
            _ => {}
        }
        ReviewAction::None
    }

    fn enter(&mut self, rows: &[Row]) -> ReviewAction {
        match rows.get(self.cursor).cloned() {
            Some(Row::Section(_) | Row::File(_)) => self.toggle(rows),
            Some(Row::Line { path, hunk, line }) => {
                let at = self.file(&path).and_then(|f| f.diff.as_ref()).and_then(|d| d.hunks.get(hunk)).and_then(|h| h.lines[..=line].iter().rev().find_map(|l| l.new_line));
                ReviewAction::OpenFile { path, line: at }
            }
            Some(Row::Conversation(id)) => {
                // Go to the thread in its file's diff, opening the file.
                let path = self.thread(&id).and_then(|t| t.position.as_ref()).map(|p| p.new_path.clone());
                if let Some(path) = path {
                    self.folded.remove(&Section::Files);
                    self.open.insert(path);
                    if let Some(i) = self.rows().iter().position(|r| *r == Row::Note { thread: id.clone(), note: 0 }) {
                        self.cursor = i;
                        return ReviewAction::None;
                    }
                    return self.say("that thread's line isn't in the diff any more -- it's outdated", false);
                }
                ReviewAction::Reply(id)
            }
            Some(Row::Note { thread, .. }) => ReviewAction::Reply(thread),
            Some(Row::Check(id)) => match self.data.as_ref().and_then(|d| d.checks.iter().find(|c| c.id == id)) {
                Some(check) => ReviewAction::ShowLog(check.clone()),
                None => ReviewAction::None,
            },
            _ => ReviewAction::None,
        }
    }

    /// `v`: marks the file (under the cursor, or whose diff it's in) as
    /// viewed -- or not, if it was -- folds it, and moves to the next one
    /// not viewed yet.
    fn mark_viewed(&mut self, rows: &[Row]) -> ReviewAction {
        let path = match rows.get(self.cursor) {
            Some(Row::File(p) | Row::Hunk { path: p, .. } | Row::Line { path: p, .. }) => p.clone(),
            _ => return self.say("v marks a file as viewed -- put the cursor on one", false),
        };
        let Some(file) = self.file(&path).cloned() else { return ReviewAction::None };
        if self.viewed(&file) {
            self.state.viewed.remove(&path);
        } else {
            self.state.viewed.insert(path.clone(), file.print);
            self.open.remove(&path);
            let next = self.files().iter().find(|f| !self.viewed(f)).map(|f| f.path.clone());
            let rows = self.rows();
            self.cursor = match next {
                Some(next) => rows.iter().position(|r| *r == Row::File(next.clone())).unwrap_or(self.cursor),
                None => rows.iter().position(|r| *r == Row::File(path.clone())).unwrap_or(self.cursor),
            };
        }
        ReviewAction::Save
    }

    fn unviewed(&self) -> Vec<String> {
        self.files().iter().filter(|f| !self.viewed(f)).map(|f| f.path.clone()).collect()
    }

    fn submit_menu(&self, verdict: Verdict, armed: bool) -> Menu {
        let chip = |key: &str, v: Verdict| MenuItem { key: key.into(), label: v.label().into(), detail: String::new(), flag: Some(v == verdict), danger: false };
        let summary = self.summary.lines().next().map(|l| fit(l, 40)).unwrap_or_else(|| "none yet".to_string());
        let mut groups = vec![
            (Some("Verdict".to_string()), vec![chip("a", Verdict::Approve), chip("r", Verdict::RequestChanges), chip("c", Verdict::Comment)]),
            (Some("Summary".to_string()), vec![MenuItem { key: "e".into(), label: "write it".into(), detail: summary, flag: None, danger: false }]),
        ];
        let pending: Vec<MenuItem> = self
            .state
            .pending
            .iter()
            .map(|p| MenuItem { key: String::new(), label: format!("{}:{}", p.path, p.line().unwrap_or(0)), detail: fit(p.body.lines().next().unwrap_or(""), 40), flag: None, danger: false })
            .collect();
        if !pending.is_empty() {
            groups.push((Some(format!("Sends {}", count(pending.len(), "comment"))), pending));
        }
        let mut before = Vec::new();
        let unviewed = self.unviewed();
        if !unviewed.is_empty() {
            before.push(MenuItem { key: String::new(), label: format!("{} not viewed", count(unviewed.len(), "file")), detail: fit(&unviewed.join(", "), 40), flag: None, danger: true });
        }
        if let Some(d) = &self.data {
            let open = d.threads.iter().filter(|t| t.resolvable && !t.resolved).count();
            if open > 0 {
                before.push(MenuItem { key: String::new(), label: format!("{} still open", count(open, "thread")), detail: String::new(), flag: None, danger: false });
            }
            if let Some(p) = &d.request.pipeline {
                before.push(MenuItem { key: String::new(), label: format!("checks {}", p.label()), detail: format!("on {}", short(&d.request.sha)), flag: None, danger: p.is_bad() });
            }
        }
        if !before.is_empty() {
            groups.push((Some("Before you send".to_string()), before));
        }
        let title = if armed { "C-c again to submit -- any other key goes back".to_string() } else { format!("Submit your review of {} · {}", self.reference(), verdict.label()) };
        Menu { title, groups }
    }

    fn submit_key(&mut self, key: Key, verdict: Verdict, armed: bool) -> ReviewAction {
        let set = |page: &mut Self, v: Verdict| page.mode = Mode::Submit { verdict: v, armed: false };
        match key {
            Key::Char('a') => set(self, Verdict::Approve),
            Key::Char('r') => set(self, Verdict::RequestChanges),
            Key::Char('c') => set(self, Verdict::Comment),
            Key::Char('e') => return ReviewAction::Summary,
            Key::CtrlC if armed => {
                self.mode = Mode::Normal;
                return ReviewAction::Submit { verdict, body: self.summary.clone() };
            }
            Key::CtrlC => self.mode = Mode::Submit { verdict, armed: true },
            Key::Escape | Key::Char('q') => self.mode = Mode::Normal,
            _ => set(self, verdict),
        }
        ReviewAction::None
    }

    fn merge_menu(&self, options: &MergeOptions, armed: bool) -> Menu {
        let item = |key: &str, label: &str, detail: String, flag: Option<bool>, danger: bool| MenuItem { key: key.into(), label: label.into(), detail, flag, danger };
        let mut ready = Vec::new();
        if let Some(d) = &self.data {
            match &d.approvals {
                Some(a) if a.approved => ready.push(item("", "approved", a.approved_by.join(", "), None, false)),
                Some(a) if a.left > 0 => ready.push(item("", &format!("{} more approval(s) needed", a.left), String::new(), None, true)),
                _ => ready.push(item("", "no approvals yet", String::new(), None, true)),
            }
            if let Some(p) = &d.request.pipeline {
                ready.push(item("", &format!("checks {}", p.label()), String::new(), None, p.is_bad() || *p == PipelineStatus::Running));
            }
            let open = d.threads.iter().filter(|t| t.resolvable && !t.resolved).count();
            ready.push(item("", &format!("{} open", count(open, "thread")), String::new(), None, open > 0));
            if d.request.has_conflicts {
                ready.push(item("", "conflicts with the target", "rebase or merge it first".into(), None, true));
            }
        }
        let how = vec![
            item("s", "squash", String::new(), Some(options.squash), false),
            item("b", "rebase", String::new(), Some(options.rebase), false),
            item("d", "delete the source branch", String::new(), Some(options.remove_source_branch), false),
            item("w", "when the checks pass", String::new(), Some(options.when_checks_pass), false),
            item("m", if armed { "merge -- for real" } else { "merge" }, if armed { "m again".into() } else { String::new() }, None, true),
        ];
        let target = self.data.as_ref().map(|d| d.request.target_branch.clone()).unwrap_or_default();
        Menu { title: format!("Merge {} into {target}", self.reference()), groups: vec![(Some("Ready?".into()), ready), (Some("How".into()), how)] }
    }

    fn merge_key(&mut self, key: Key, mut options: MergeOptions, armed: bool) -> ReviewAction {
        match key {
            Key::Char('s') => {
                options.squash = !options.squash;
                options.rebase = false;
            }
            Key::Char('b') => {
                options.rebase = !options.rebase;
                options.squash = false;
            }
            Key::Char('d') => options.remove_source_branch = !options.remove_source_branch,
            Key::Char('w') => options.when_checks_pass = !options.when_checks_pass,
            Key::Char('m') if armed => {
                self.mode = Mode::Normal;
                return ReviewAction::Merge(options);
            }
            Key::Char('m') => {
                self.mode = Mode::Merge { options, armed: true };
                return ReviewAction::None;
            }
            _ => {
                self.mode = Mode::Normal;
                return ReviewAction::None;
            }
        }
        self.mode = Mode::Merge { options, armed: false };
        ReviewAction::None
    }
}

pub fn layout(page: &ReviewPage, cols: usize) -> Page {
    let (left, width) = frame(cols, 150);
    let mut g = Grid::new();
    let mut y = 1;
    let Some(data) = &page.data else {
        g.put(y, left, &format!("Reading {} from {}…", page.reference(), page.forge), Role::Muted);
        if let Some((text, _)) = &page.message {
            g.put(y + 2, left, text, Role::Bad);
        }
        return g.finish();
    };
    let r = &data.request;
    let x = g.put(y, left, &r.reference(), Role::Accent) + 1;
    g.put(y, x, &fit(&r.title, width.saturating_sub(x - left)), Role::Title);
    y += 1;
    let x = g.put(y, left, &format!("{} → {}", r.source_branch, r.target_branch), Role::Text) + 2;
    let x = g.put(y, x, &format!("· {} · {}{}", r.author, r.state.label(), if r.draft { " · draft" } else { "" }), Role::Muted) + 2;
    let x = match &r.pipeline {
        Some(p) => g.put(y, x, &format!("checks {}", p.label()), if p.is_bad() { Role::Bad } else if *p == PipelineStatus::Success { Role::Good } else { Role::Warn }) + 2,
        None => x,
    };
    if let Some(a) = &data.approvals {
        let text = if a.approved { format!("approved by {}", a.approved_by.join(", ")) } else { "not approved yet".to_string() };
        g.put(y, x, &text, if a.approved { Role::Good } else { Role::Muted });
    }
    y += 1;
    let files = page.files();
    let viewed = files.iter().filter(|f| page.viewed(f)).count();
    let x = g.put(y, left, &format!("viewed {viewed}/{}", files.len()), if viewed == files.len() && !files.is_empty() { Role::Good } else { Role::Muted }) + 2;
    let x = if page.state.pending.is_empty() { x } else { g.put(y, x, &format!("{} pending -- s submits", count(page.state.pending.len(), "comment")), Role::Warn) + 2 };
    if page.since_review {
        g.put(y, x, &format!("since your review at {}", page.state.reviewed_head.as_deref().map(short).unwrap_or("")), Role::Accent);
    } else if page.in_worktree {
        g.put(y, x, "in a worktree of its own", Role::Muted);
    }
    y += 1;
    if let Some(busy) = &page.busy {
        g.put(y, left, &format!("… {busy}"), Role::Accent);
        y += 1;
    } else if let Some((text, failed)) = &page.message {
        g.put(y, left, &fit(text, width), if *failed { Role::Bad } else { Role::Muted });
        y += 1;
    }
    if let Some(err) = &data.error {
        g.put(y, left, &fit(err, width), Role::Warn);
        y += 1;
    }
    // Submit and merge panels sit under the header, where they're seen.
    let panel = match &page.mode {
        Mode::Submit { verdict, armed } => Some(page.submit_menu(*verdict, *armed)),
        Mode::Merge { options, armed } => Some(page.merge_menu(options, *armed)),
        Mode::Normal => None,
    };
    if let Some(menu) = &panel {
        if let Some((end, cols)) = draw_popups(&mut g, y + 1, left.saturating_sub(4), width + 4, Some((menu, "")), None, None) {
            g.focus(end, cols);
            y = end + 1;
        }
    }

    let rows = page.rows();
    let selection = page.anchor.map(|a| (a.min(page.cursor), a.max(page.cursor)));
    let mut current_file: Option<&FileView> = None;
    for (i, row) in rows.iter().enumerate() {
        if matches!(row, Row::Section(_)) {
            y += 1;
        }
        let line_y = y;
        match row {
            Row::Section(section) => {
                let folded = page.folded.contains(section);
                let (title, n) = match section {
                    Section::Description => ("Description".to_string(), String::new()),
                    Section::Files => (format!("Files · {viewed}/{} viewed", files.len()), String::new()),
                    Section::Conversations => {
                        let human: Vec<&Discussion> = data.threads.iter().filter(|t| t.is_human()).collect();
                        let open = human.iter().filter(|t| t.resolvable && !t.resolved).count();
                        ("Conversations".to_string(), format!("{open} open · {} resolved", human.iter().filter(|t| t.resolved).count()))
                    }
                    Section::Checks => ("Checks".to_string(), data.checks.len().to_string()),
                };
                let x = g.put(y, left, if folded { "▸" } else { "▾" }, Role::Muted) + 1;
                let x = g.put(y, x, &title.to_uppercase(), Role::Text) + 1;
                let x = g.put(y, x, &n, Role::Muted) + 1;
                if x + 2 < left + width {
                    g.rule(y, x..left + width);
                }
            }
            Row::Description(n) => {
                let text = r.description.lines().nth(*n).unwrap_or(if r.description.trim().is_empty() { "(no description)" } else { "" });
                g.put(y, left + 2, &fit(text, width - 2), Role::Text);
            }
            Row::File(path) => {
                let Some(file) = page.file(path) else { continue };
                current_file = Some(file);
                let seen = page.viewed(file);
                let x = g.put(y, left + 2, if seen { "[x]" } else { "[ ]" }, if seen { Role::Good } else { Role::Muted }) + 1;
                let role = match file.letter {
                    'A' => Role::Good,
                    'D' => Role::Bad,
                    _ => Role::Warn,
                };
                let x = g.put(y, x, &file.letter.to_string(), role) + 2;
                let (add, del) = file.stats();
                let threads = data.threads.iter().filter(|t| t.is_human() && t.position.as_ref().is_some_and(|p| p.new_path == file.path)).count();
                let tail = format!("+{add} −{del}{}", if threads > 0 { format!(" · {}", count(threads, "thread")) } else { String::new() });
                g.put(y, x, &fit(path, (left + width).saturating_sub(x + tail.chars().count() + 2)), if seen { Role::Muted } else { Role::Text });
                g.put(y, (left + width).saturating_sub(tail.chars().count()), &tail, Role::Muted);
            }
            Row::Hunk { path, hunk } => {
                if let Some(h) = page.file(path).and_then(|f| f.diff.as_ref()).and_then(|d| d.hunks.get(*hunk)) {
                    g.put(y, left + 6, &fit(&h.header, width.saturating_sub(6)), Role::Accent);
                }
            }
            Row::Line { path, hunk, line } => {
                if let Some(l) = page.diff_line(path, *hunk, *line) {
                    let number = l.new_line.or(l.old_line).map(|n| n.to_string()).unwrap_or_default();
                    g.put(y, left + 6 + 5usize.saturating_sub(number.len()), &number, Role::Muted);
                    let role = match l.kind {
                        LineKind::Added => Role::Good,
                        LineKind::Removed => Role::Bad,
                        _ => Role::Text,
                    };
                    g.put(y, left + 13, &fit(&l.raw().trim_end_matches('\r').replace('\t', "    "), width.saturating_sub(13)), role);
                    if selection.is_some_and(|(a, b)| i >= a && i <= b) {
                        g.panels.push((y, left + 12..left + width));
                    }
                }
            }
            Row::Note { thread, note } => {
                if let Some(t) = page.thread(thread) {
                    if let Some(n) = t.notes.get(*note) {
                        let indent = left + 14;
                        let x = if *note == 0 {
                            let (tag, role) = match (t.resolved, t.resolvable) {
                                (true, _) => ("RESOLVED", Role::Good),
                                (false, true) => ("OPEN", Role::Warn),
                                _ => ("", Role::Muted),
                            };
                            g.put(y, indent, tag, role) + 1
                        } else {
                            indent + 2
                        };
                        let x = g.put(y, x, &n.author, Role::Title) + 1;
                        let body = wrap(&n.body.replace('\r', ""), (left + width).saturating_sub(x + 1).max(20));
                        for (k, part) in body.iter().take(8).enumerate() {
                            g.put(y + k, x, part, Role::Text);
                            g.panels.push((y + k, indent - 1..left + width));
                        }
                        y += body.len().clamp(1, 8) - 1;
                    }
                }
            }
            Row::Pending(index) => {
                if let Some(p) = page.state.pending.get(*index) {
                    let indent = left + 14;
                    let x = g.put(y, indent, "PENDING", Role::Warn) + 1;
                    let x = g.put(y, x, "you", Role::Title) + 1;
                    let body = wrap(&p.body, (left + width).saturating_sub(x + 1).max(20));
                    for (k, part) in body.iter().take(10).enumerate() {
                        let role = if part.starts_with('+') || part.starts_with("```") { Role::Accent } else { Role::Text };
                        g.put(y + k, x, part, role);
                        g.panels.push((y + k, indent - 1..left + width));
                    }
                    y += body.len().clamp(1, 10) - 1;
                }
            }
            Row::Conversation(id) => {
                if let Some(t) = page.thread(id) {
                    let (dot, role) = match (t.resolved, t.resolvable) {
                        (true, _) => ("✓", Role::Good),
                        (false, true) => ("●", Role::Warn),
                        _ => ("·", Role::Muted),
                    };
                    let x = g.put(y, left + 2, dot, role) + 1;
                    let place = t.position.as_ref().map(|p| format!("{}:{}", p.new_path, p.new_line.or(p.old_line).unwrap_or(0))).unwrap_or_else(|| "on the request".to_string());
                    let x = g.put(y, x, &place, Role::Accent) + 1;
                    let first = t.first().map(|n| format!("{}: {}", n.author, n.body.lines().next().unwrap_or(""))).unwrap_or_default();
                    g.put(y, x, &fit(&first, (left + width).saturating_sub(x + 12)), Role::Text);
                    let replies = t.notes.len().saturating_sub(1);
                    if replies > 0 {
                        let tail = count(replies, "reply");
                        g.put(y, (left + width).saturating_sub(tail.chars().count()), &tail, Role::Muted);
                    }
                }
            }
            Row::Check(id) => {
                if let Some(c) = data.checks.iter().find(|c| &c.id == id) {
                    let role = match c.status {
                        PipelineStatus::Success => Role::Good,
                        ref s if s.is_bad() => Role::Bad,
                        PipelineStatus::Running | PipelineStatus::Pending => Role::Warn,
                        _ => Role::Muted,
                    };
                    let x = g.put(y, left + 2, "■", role) + 1;
                    let x = g.put(y, x, &c.name, Role::Text) + 2;
                    let x = g.put(y, x, c.status.label(), role) + 2;
                    if !c.group.is_empty() {
                        g.put(y, x, &c.group, Role::Muted);
                    }
                    if let Some(s) = c.seconds {
                        let t = format!("{}m {:02}s", s / 60, s % 60);
                        g.put(y, (left + width).saturating_sub(t.len()), &t, Role::Muted);
                    }
                }
            }
        }
        let _ = current_file;
        if i == page.cursor && panel.is_none() {
            g.focus(line_y, left..left + width);
        }
        y += 1;
    }
    let keys: &[(&str, &str)] = match &page.mode {
        Mode::Submit { .. } => &[("a/r/c", "verdict"), ("e", "summary"), ("C-c C-c", "submit"), ("Esc", "back")],
        Mode::Merge { .. } => &[("s/b/d/w", "options"), ("m m", "merge"), ("any key", "back")],
        Mode::Normal if page.anchor.is_some() => &[("C", "comment on these lines"), ("S", "suggest a change"), ("Esc", "clear")],
        Mode::Normal => &[
            ("Tab", "open"),
            ("v", "viewed"),
            ("C", "comment"),
            ("S", "suggest"),
            ("r", "reply"),
            ("R", "resolve"),
            ("]t", "next thread"),
            ("i", "since review"),
            ("s", "submit"),
            ("m", "merge"),
            ("w", "worktree"),
            ("o", "browser"),
            ("q", "close"),
        ],
    };
    g.keys(left, width, keys);
    g.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use fenix_forge::{DiffRefs, MrState, Note, Position};

    const DIFF: &str = "diff --git a/decoder.py b/decoder.py\n--- a/decoder.py\n+++ b/decoder.py\n@@ -1,4 +1,6 @@\n def decode(raw):\n     header = raw[:6]\n+    if header[0] == 17:\n+        return \"connection test\", None\n     body = raw[6:]\n     return header, body\n";

    fn request() -> MergeRequest {
        MergeRequest {
            number: 1,
            title: "Decode PUS-17".into(),
            description: "Replies have no body.\nSo they were unknown.".into(),
            state: MrState::Open,
            draft: false,
            source_branch: "feature/pus17".into(),
            target_branch: "main".into(),
            author: "alex".into(),
            web_url: "https://github.com/o/r/pull/1".into(),
            has_conflicts: false,
            sha: "head2".into(),
            diff_refs: DiffRefs { base_sha: "b".into(), head_sha: "head2".into(), start_sha: "b".into() },
            comment_count: 1,
            pipeline: Some(PipelineStatus::Success),
            updated_at: "t".into(),
        }
    }

    fn thread(id: &str, line: usize, resolved: bool) -> Discussion {
        Discussion {
            id: id.into(),
            notes: vec![Note { id: 1, author: "sam".into(), body: "Name the constant?".into(), created_at: "t".into(), system: false }],
            resolved,
            resolvable: true,
            position: Some(Position { base_sha: String::new(), head_sha: String::new(), start_sha: String::new(), old_path: "decoder.py".into(), new_path: "decoder.py".into(), old_line: None, new_line: Some(line) }),
        }
    }

    fn page() -> ReviewPage {
        let mut p = ReviewPage::new(PathBuf::from("/r"), "GitHub".into(), "o/r".into(), 1, ReviewState::default());
        p.set_data(ReviewData {
            request: request(),
            approvals: None,
            files: vec![FileView::new("decoder.py".into(), "decoder.py".into(), 'M', DIFF), FileView::new("test.py".into(), "test.py".into(), 'A', "diff --git a/test.py b/test.py\n--- /dev/null\n+++ b/test.py\n@@ -0,0 +1 @@\n+x\n")],
            threads: vec![thread("T1", 3, false)],
            checks: vec![Check { id: "9".into(), name: "pytest".into(), group: "CI".into(), status: PipelineStatus::Success, url: String::new(), seconds: Some(61) }],
            since: None,
            error: None,
        });
        p
    }

    fn goto(p: &mut ReviewPage, row: Row) {
        p.cursor = p.rows().iter().position(|r| *r == row).unwrap_or_else(|| panic!("{row:?} not in {:?}", p.rows()));
    }

    #[test]
    fn a_file_opens_to_its_diff_with_its_threads_under_their_lines() {
        let mut p = page();
        goto(&mut p, Row::File("decoder.py".into()));
        p.key(Key::Tab);
        let rows = p.rows();
        let line3 = rows.iter().position(|r| matches!(r, Row::Line { line: 2, .. })).unwrap();
        assert_eq!(rows[line3 + 1], Row::Note { thread: "T1".into(), note: 0 }, "the thread follows its line");
        let text = layout(&p, 120).text;
        assert!(text.contains("#1 Decode PUS-17") && text.contains("OPEN sam Name the constant?") && text.contains("viewed 0/2"), "{text}");
    }

    #[test]
    fn v_marks_viewed_and_moves_on_and_a_changed_diff_unmarks_it() {
        let mut p = page();
        goto(&mut p, Row::File("decoder.py".into()));
        assert_eq!(p.key(Key::Char('v')), ReviewAction::Save);
        assert_eq!(p.rows()[p.cursor], Row::File("test.py".into()), "on to the next unviewed");
        assert!(layout(&p, 120).text.contains("viewed 1/2"));
        // A new push changes decoder.py's diff: it's not viewed any more.
        let mut data = p.data.clone().unwrap();
        data.files[0] = FileView::new("decoder.py".into(), "decoder.py".into(), 'M', &DIFF.replace("17", "18"));
        p.set_data(data);
        assert!(layout(&p, 120).text.contains("viewed 0/2"));
    }

    #[test]
    fn a_comment_and_a_suggestion_start_from_the_lines_under_the_cursor() {
        let mut p = page();
        goto(&mut p, Row::File("decoder.py".into()));
        p.key(Key::Tab);
        goto(&mut p, Row::Line { path: "decoder.py".into(), hunk: 0, line: 2 });
        let ReviewAction::Comment { pending, index: None } = p.key(Key::Char('C')) else { panic!() };
        assert_eq!((pending.new_line, pending.old_line, pending.start_line), (Some(3), None, None));
        p.key(Key::Char('V'));
        p.key(Key::Char('j')); // the thread under line 3
        p.key(Key::Char('j')); // line 4
        let ReviewAction::Comment { pending, .. } = p.key(Key::Char('S')) else { panic!() };
        assert_eq!((pending.start_line, pending.new_line), (Some(3), Some(4)));
        assert!(pending.body.starts_with("```suggestion\n    if header[0] == 17:\n"), "{}", pending.body);
    }

    #[test]
    fn pending_comments_show_under_their_line_and_count_in_the_header() {
        let mut p = page();
        p.state.pending.push(Pending { path: "decoder.py".into(), old_path: "decoder.py".into(), old_line: None, new_line: Some(4), start_line: None, body: "Frozen?".into() });
        goto(&mut p, Row::File("decoder.py".into()));
        p.key(Key::Tab);
        assert!(p.rows().contains(&Row::Pending(0)));
        let text = layout(&p, 120).text;
        assert!(text.contains("PENDING you Frozen?") && text.contains("1 comment pending -- s submits"), "{text}");
        goto(&mut p, Row::Pending(0));
        assert_eq!(p.key(Key::Char('x')), ReviewAction::Save);
        assert!(p.state.pending.is_empty());
    }

    #[test]
    fn submitting_takes_a_verdict_and_two_c_c() {
        let mut p = page();
        p.key(Key::Char('s'));
        let text = layout(&p, 120).text;
        assert!(text.contains("Submit your review of #1") && text.contains("2 files not viewed"), "{text}");
        p.key(Key::Char('r'));
        assert_eq!(p.key(Key::CtrlC), ReviewAction::None);
        assert_eq!(p.key(Key::CtrlC), ReviewAction::Submit { verdict: Verdict::RequestChanges, body: String::new() });
    }

    #[test]
    fn merging_arms_first_and_carries_its_options() {
        let mut p = page();
        p.key(Key::Char('m'));
        p.key(Key::Char('s'));
        p.key(Key::Char('d'));
        assert_eq!(p.key(Key::Char('m')), ReviewAction::None);
        let ReviewAction::Merge(options) = p.key(Key::Char('m')) else { panic!() };
        assert!(options.squash && options.remove_source_branch && options.sha.as_deref() == Some("head2"));
    }

    #[test]
    fn bracket_t_walks_open_threads_and_enter_on_a_conversation_goes_to_it() {
        let mut p = page();
        p.folded.remove(&Section::Conversations);
        goto(&mut p, Row::Conversation("T1".into()));
        p.key(Key::Enter);
        assert_eq!(p.rows()[p.cursor], Row::Note { thread: "T1".into(), note: 0 });
        p.cursor = 0;
        p.key(Key::Char(']'));
        p.key(Key::Char('t'));
        assert_eq!(p.rows()[p.cursor], Row::Note { thread: "T1".into(), note: 0 });
        assert_eq!(p.key(Key::Char('R')), ReviewAction::Resolve { thread: "T1".into(), resolved: true });
    }

    #[test]
    fn since_your_review_needs_a_review_and_a_new_head() {
        let mut p = page();
        p.key(Key::Char('i'));
        assert!(p.message.as_ref().unwrap().0.contains("no review of yours yet"));
        p.state.reviewed_head = Some("head1".into());
        assert_eq!(p.key(Key::Char('i')), ReviewAction::LoadSince);
        assert!(p.since_review);
    }
}
