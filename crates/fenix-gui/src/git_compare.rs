//! The Compare page (`SPC g c`): your branch -- or what's on disk --
//! against another ref, local or remote. A header says where the two
//! stand (merge base, ahead, behind, how stale the remote is); under it
//! the commits each side has that the other doesn't, and every file
//! that differs with its line counts. `Tab` opens a file's diff inline,
//! a commit to its files.
//!
//! `t` switches between three-dot (what your side did since the two
//! split -- what a pull request shows) and two-dot (every difference
//! between the trees); `w` brings in uncommitted and untracked work;
//! `s` turns the comparison round.
//!
//! Pure layout and decisions, like the other Git pages; the host
//! (`app/git_compare_page.rs`) reads the repository off the UI thread.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use fenix_diff::{FileDiff, LineKind};
use fenix_git::{ChangedFile, Commit, Range, Side};

use crate::git_log::Detail;
use crate::git_status::{ago, count, popup, short, verb, DiffState, Menu};
use crate::page::{fit, frame, Grid, Key, Page, Role};

/// Commits listed per section at most.
pub const COMMIT_LIMIT: usize = 200;

/// A remote ref fetched longer ago than this is called out.
const STALE_SECS: u64 = 24 * 60 * 60;

/// What the page shows, read off the UI thread for one `Range`.
#[derive(Debug, Clone)]
pub struct CompareData {
    /// The comparison this was read for -- a reply to an older one is
    /// dropped.
    pub range: Range,
    /// The checked-out branch, `HEAD (detached)` without one.
    pub branch: String,
    pub merge_base: Option<String>,
    /// On the head side, not the base.
    pub ahead: Vec<Commit>,
    /// On the base side, not the head.
    pub behind: Vec<Commit>,
    pub files: Result<Vec<ChangedFile>, String>,
    /// The remote a remote-tracking side belongs to, and how long ago
    /// this repository last fetched.
    pub remote: Option<String>,
    pub fetched_secs_ago: Option<u64>,
    /// Files changed on disk that a committed comparison leaves out.
    pub uncommitted: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Section {
    Ahead,
    Behind,
    Files,
}

impl Section {
    fn title(self) -> &'static str {
        match self {
            Section::Ahead => "Ahead",
            Section::Behind => "Behind",
            Section::Files => "Files",
        }
    }
}

/// Whose diff a row belongs to: a file of the comparison, or a file of
/// one commit.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DiffKey {
    Range(String),
    Commit { hash: String, path: String },
}

impl DiffKey {
    pub fn path(&self) -> &str {
        match self {
            DiffKey::Range(path) | DiffKey::Commit { path, .. } => path,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row {
    Section(Section),
    Commit { section: Section, hash: String },
    File(DiffKey),
    Hunk(DiffKey, usize),
    Line(DiffKey, usize, usize),
}

/// What the page asks of its host.
#[derive(Debug, Clone, PartialEq)]
pub enum CompareAction {
    None,
    Refresh,
    LoadDiff(DiffKey),
    LoadCommit(String),
    OpenFile { path: String, line: Option<usize> },
    /// A file as it is at a ref, read-only.
    OpenAt { rev: String, path: String },
    /// Choose the ref to compare with.
    PickTarget,
    /// Choose your own side.
    PickOwn,
    Fetch(String),
    Copy(String),
    Close,
}

pub struct GitCompare {
    pub root: PathBuf,
    /// The ref being compared with.
    pub target: String,
    /// Your side; `None` is HEAD.
    pub own: Option<String>,
    pub three_dot: bool,
    /// Your side is the working tree, not HEAD's commit.
    pub worktree: bool,
    /// Turned round: what the target has over your side.
    pub swapped: bool,
    pub data: Option<CompareData>,
    pub cursor: usize,
    pub folded: HashSet<Section>,
    pub open: HashMap<String, Detail>,
    pub diffs: HashMap<DiffKey, DiffState>,
    pub message: Option<(String, bool)>,
    pub busy: Option<String>,
    pub loading: bool,
    help: bool,
    pending_g: bool,
}

impl GitCompare {
    pub fn new(root: PathBuf, target: String, own: Option<String>) -> Self {
        GitCompare {
            root,
            target,
            own,
            three_dot: true,
            worktree: false,
            swapped: false,
            data: None,
            cursor: 0,
            folded: HashSet::from([Section::Behind]),
            open: HashMap::new(),
            diffs: HashMap::new(),
            message: None,
            busy: None,
            loading: false,
            help: false,
            pending_g: false,
        }
    }

    /// Points the page at another comparison, keeping its modes.
    pub fn retarget(&mut self, target: String, own: Option<String>) {
        if own.is_some() {
            self.worktree = false;
        }
        self.target = target;
        self.own = own;
        self.forget();
    }

    /// Drops what was read for the last comparison.
    fn forget(&mut self) {
        self.open.clear();
        self.diffs.clear();
        self.message = None;
    }

    fn own_rev(&self) -> String {
        self.own.clone().unwrap_or_else(|| "HEAD".to_string())
    }

    /// What's compared, as git sees it.
    pub fn range(&self) -> Range {
        if self.swapped {
            return Range { base: self.own_rev(), head: Side::Rev(self.target.clone()), three_dot: self.three_dot };
        }
        let head = if self.worktree { Side::WorkTree } else { Side::Rev(self.own_rev()) };
        Range { base: self.target.clone(), head, three_dot: self.three_dot }
    }

    pub fn set_data(&mut self, data: CompareData) {
        if data.range != self.range() {
            return;
        }
        self.loading = false;
        let before = self.rows().get(self.cursor).cloned();
        // Open diffs of files no longer in the comparison go.
        if let Ok(files) = &data.files {
            self.diffs.retain(|k, _| match k {
                DiffKey::Range(path) => files.iter().any(|f| &f.path == path),
                DiffKey::Commit { .. } => true,
            });
        }
        self.data = Some(data);
        let rows = self.rows();
        self.cursor = before.and_then(|b| rows.iter().position(|r| *r == b)).or_else(|| self.first_row(&rows)).unwrap_or(0);
    }

    /// Where the cursor starts: the first file, else the top.
    fn first_row(&self, rows: &[Row]) -> Option<usize> {
        rows.iter().position(|r| matches!(r, Row::File(DiffKey::Range(_)))).or(if rows.is_empty() { None } else { Some(0) })
    }

    pub fn set_files(&mut self, hash: String, files: Vec<(char, String)>) {
        if self.open.contains_key(&hash) {
            self.open.insert(hash, Detail::Files(files));
        }
    }

    pub fn set_diff(&mut self, key: DiffKey, diff: DiffState) {
        if self.diffs.contains_key(&key) {
            self.diffs.insert(key, diff);
        }
    }

    /// The changed file of the comparison at `path`.
    pub fn changed(&self, path: &str) -> Option<&ChangedFile> {
        self.data.as_ref()?.files.as_ref().ok()?.iter().find(|f| f.path == path)
    }

    fn diff(&self, key: &DiffKey) -> Option<&FileDiff> {
        match self.diffs.get(key) {
            Some(DiffState::Loaded(d)) => Some(d),
            _ => None,
        }
    }

    fn commits(&self, section: Section) -> &[Commit] {
        match (&self.data, section) {
            (Some(d), Section::Ahead) => &d.ahead,
            (Some(d), Section::Behind) => &d.behind,
            _ => &[],
        }
    }

    fn push_diff(&self, rows: &mut Vec<Row>, key: DiffKey) {
        rows.push(Row::File(key.clone()));
        if let Some(diff) = self.diff(&key) {
            for (h, hunk) in diff.hunks.iter().enumerate() {
                rows.push(Row::Hunk(key.clone(), h));
                rows.extend((0..hunk.lines.len()).map(|l| Row::Line(key.clone(), h, l)));
            }
        }
    }

    pub fn rows(&self) -> Vec<Row> {
        let range = self.range();
        let Some(data) = self.data.as_ref().filter(|d| d.range == range) else { return Vec::new() };
        let mut rows = Vec::new();
        for section in [Section::Ahead, Section::Behind, Section::Files] {
            rows.push(Row::Section(section));
            if self.folded.contains(&section) {
                continue;
            }
            if section == Section::Files {
                for file in data.files.as_ref().map(Vec::as_slice).unwrap_or(&[]) {
                    self.push_diff(&mut rows, DiffKey::Range(file.path.clone()));
                }
                continue;
            }
            for commit in self.commits(section) {
                rows.push(Row::Commit { section, hash: commit.hash.clone() });
                if let Some(Detail::Files(files)) = self.open.get(&commit.hash) {
                    for (_, path) in files {
                        self.push_diff(&mut rows, DiffKey::Commit { hash: commit.hash.clone(), path: path.clone() });
                    }
                }
            }
        }
        rows
    }

    fn say(&mut self, text: impl Into<String>) -> CompareAction {
        self.message = Some((text.into(), false));
        CompareAction::None
    }

    fn refresh(&mut self) -> CompareAction {
        self.forget();
        self.loading = true;
        CompareAction::Refresh
    }

    pub fn key(&mut self, key: Key) -> CompareAction {
        if self.help {
            self.help = false;
            return if key == Key::Char('?') || key == Key::Escape { CompareAction::None } else { self.key(key) };
        }
        let rows = self.rows();
        let g = std::mem::take(&mut self.pending_g);
        match key {
            Key::Down | Key::Char('j') => self.step(rows.len(), true),
            Key::Up | Key::Char('k') => self.step(rows.len(), false),
            Key::Char('g') if g => {
                self.cursor = 0;
                CompareAction::None
            }
            Key::Char('g') => {
                self.pending_g = true;
                CompareAction::None
            }
            Key::Char('G') => {
                self.cursor = rows.len().saturating_sub(1);
                CompareAction::None
            }
            Key::Char(']') => self.jump_hunk(&rows, true),
            Key::Char('[') => self.jump_hunk(&rows, false),
            Key::Tab => self.toggle(&rows),
            Key::Enter => match rows.get(self.cursor).cloned() {
                Some(Row::Section(_) | Row::Commit { .. } | Row::Hunk(..)) => self.toggle(&rows),
                Some(Row::File(key)) => CompareAction::OpenFile { path: key.path().to_string(), line: None },
                Some(Row::Line(key, h, l)) => {
                    let line = self.diff(&key).and_then(|d| d.hunks.get(h)).and_then(|hunk| hunk.lines[..=l].iter().rev().find_map(|x| x.new_line));
                    CompareAction::OpenFile { path: key.path().to_string(), line }
                }
                None => CompareAction::None,
            },
            Key::Char('o') => match rows.get(self.cursor) {
                Some(Row::File(DiffKey::Range(path)) | Row::Hunk(DiffKey::Range(path), _) | Row::Line(DiffKey::Range(path), ..)) => {
                    let file = self.changed(path);
                    if file.is_some_and(|f| f.letter == 'A' || f.letter == '?') && !self.swapped {
                        return self.say(format!("{path} isn't on {}", self.target));
                    }
                    // Not turned round, the target is the base side, where
                    // a renamed file still has its old name.
                    let at = if self.swapped { path.clone() } else { file.and_then(|f| f.old_path.clone()).unwrap_or_else(|| path.clone()) };
                    CompareAction::OpenAt { rev: self.target.clone(), path: at }
                }
                _ => self.say(format!("o opens a file as it is on {} -- put the cursor on one", self.target)),
            },
            Key::Char('t') => {
                self.three_dot = !self.three_dot;
                self.refresh()
            }
            Key::Char('w') => {
                if self.own.is_some() {
                    return self.say("w compares the working tree -- only when your side is HEAD");
                }
                if self.swapped {
                    return self.say("w doesn't apply turned round -- s turns it back first");
                }
                self.worktree = !self.worktree;
                self.refresh()
            }
            Key::Char('s') => {
                if self.worktree {
                    return self.say("the working tree can't be turned round -- w goes back to commits first");
                }
                self.swapped = !self.swapped;
                self.refresh()
            }
            Key::Char('r') => CompareAction::PickTarget,
            Key::Char('R') => CompareAction::PickOwn,
            Key::Char('f') => match self.data.as_ref().and_then(|d| d.remote.clone()) {
                Some(remote) if self.busy.is_none() => {
                    self.busy = Some(format!("fetching {remote}"));
                    CompareAction::Fetch(remote)
                }
                Some(_) => CompareAction::None,
                None => self.say(format!("{} isn't on a remote -- nothing to fetch", self.target)),
            },
            Key::Char('y') => CompareAction::Copy(match rows.get(self.cursor) {
                Some(Row::Commit { hash, .. }) => hash.clone(),
                _ => self.range().label(),
            }),
            Key::Char('u') => self.refresh(),
            Key::Char('?') => {
                self.help = true;
                CompareAction::None
            }
            Key::Char('q') => CompareAction::Close,
            _ => CompareAction::None,
        }
    }

    fn step(&mut self, len: usize, forward: bool) -> CompareAction {
        if forward && self.cursor + 1 < len {
            self.cursor += 1;
        } else if !forward && self.cursor > 0 {
            self.cursor -= 1;
        }
        CompareAction::None
    }

    fn jump_hunk(&mut self, rows: &[Row], forward: bool) -> CompareAction {
        let is_hunk = |r: &Row| matches!(r, Row::Hunk(..));
        let found = if forward {
            rows.iter().enumerate().skip(self.cursor + 1).find(|(_, r)| is_hunk(r)).map(|(i, _)| i)
        } else {
            rows.iter().enumerate().take(self.cursor).rev().find(|(_, r)| is_hunk(r)).map(|(i, _)| i)
        };
        if let Some(i) = found {
            self.cursor = i;
        }
        CompareAction::None
    }

    fn toggle(&mut self, rows: &[Row]) -> CompareAction {
        match rows.get(self.cursor).cloned() {
            Some(Row::Section(section)) => {
                if !self.folded.remove(&section) {
                    self.folded.insert(section);
                }
                CompareAction::None
            }
            Some(Row::Commit { hash, .. }) => {
                if self.open.remove(&hash).is_some() {
                    self.diffs.retain(|k, _| !matches!(k, DiffKey::Commit { hash: h, .. } if *h == hash));
                    return CompareAction::None;
                }
                self.open.insert(hash.clone(), Detail::Loading);
                CompareAction::LoadCommit(hash)
            }
            Some(Row::File(key)) => {
                if self.diffs.remove(&key).is_some() {
                    return CompareAction::None;
                }
                self.diffs.insert(key.clone(), DiffState::Loading);
                CompareAction::LoadDiff(key)
            }
            Some(Row::Hunk(key, _) | Row::Line(key, ..)) => {
                self.diffs.remove(&key);
                self.cursor = self.rows().iter().position(|r| *r == Row::File(key.clone())).unwrap_or(self.cursor);
                CompareAction::None
            }
            None => CompareAction::None,
        }
    }

    /// What the page says your side is: `feature/x`, `feature/x + working tree`.
    fn own_label(&self, branch: &str) -> String {
        match &self.own {
            Some(own) => own.clone(),
            None if self.worktree => format!("{branch} + working tree"),
            None => branch.to_string(),
        }
    }

    fn help_menu(&self) -> Menu {
        Menu {
            title: "Compare keys".to_string(),
            groups: vec![(
                None,
                vec![
                    verb("Tab", "open", "a file's diff, a commit's files, a section"),
                    verb("Enter", "open the file", "at the line under the cursor"),
                    verb("o", "the target's version", "of the file, read-only"),
                    verb("] [", "next / previous hunk", ""),
                    verb("t", "three-dot / two-dot", "since splitting, or every difference"),
                    verb("w", "working tree", "bring in uncommitted and untracked work"),
                    verb("s", "turn round", "what the target has over your side"),
                    verb("r", "compare with…", "another ref"),
                    verb("R", "your side…", "a ref other than HEAD"),
                    verb("f", "fetch", "the target's remote"),
                    verb("y", "copy", "the range, or a commit's hash"),
                    verb("u", "refresh", ""),
                    verb("q", "close", ""),
                ],
            )],
        }
    }
}

fn letter_role(letter: char) -> Role {
    match letter {
        'A' | '?' => Role::Good,
        'D' => Role::Bad,
        _ => Role::Warn,
    }
}

/// `+12 −3`, or `binary`.
fn stat(file: &ChangedFile) -> String {
    match (file.added, file.removed) {
        (Some(a), Some(0)) => format!("+{a}"),
        (Some(0), Some(r)) => format!("−{r}"),
        (Some(a), Some(r)) => format!("+{a} −{r}"),
        _ => "binary".to_string(),
    }
}

pub fn layout(page: &GitCompare, cols: usize) -> Page {
    let (left, width) = frame(cols, 150);
    let right = left + width;
    let mut g = Grid::new();
    let mut y = 1;
    let range = page.range();
    // Data read for another comparison (before `t`, `w`, `s` or `r`)
    // would show its numbers under this one's names.
    let Some(data) = page.data.as_ref().filter(|d| d.range == range) else {
        g.put(y, left, &format!("Comparing {}…", range.label()), Role::Muted);
        return g.finish();
    };

    // Which side is which, in words: "feature/x compared with origin/main".
    let own = page.own_label(&data.branch);
    let (head, base) = if page.swapped { (page.target.clone(), own) } else { (own, page.target.clone()) };
    let x = g.put(y, left, &head, Role::Title);
    let x = g.put(y, x + 1, "compared with", Role::Muted);
    g.put(y, x + 1, &base, Role::Accent);
    let mode = format!("{} · {}", if page.three_dot { "since splitting (...)" } else { "every difference (..)" }, if page.worktree { "working tree" } else { "committed" });
    g.put(y, right.saturating_sub(mode.chars().count()), &mode, Role::Muted);
    y += 1;

    let mut facts = Vec::new();
    if let Some(mb) = &data.merge_base {
        facts.push(format!("merge base {}", short(mb)));
    }
    let more = |n: usize| if n >= COMMIT_LIMIT { "+" } else { "" };
    facts.push(format!("{}{} ahead", data.ahead.len(), more(data.ahead.len())));
    facts.push(format!("{}{} behind", data.behind.len(), more(data.behind.len())));
    if let Ok(files) = &data.files {
        let added: usize = files.iter().filter_map(|f| f.added).sum();
        let removed: usize = files.iter().filter_map(|f| f.removed).sum();
        facts.push(format!("{} +{added} −{removed}", count(files.len(), "file")));
    }
    g.put(y, left, &fit(&facts.join(" · "), width), Role::Muted);
    y += 1;

    if let Some(remote) = &data.remote {
        let (text, role) = match data.fetched_secs_ago {
            Some(secs) if secs < STALE_SECS => (format!("{remote} last fetched {}", ago(secs)), Role::Muted),
            Some(secs) => (format!("{remote} last fetched {} -- f fetches", ago(secs)), Role::Warn),
            None => (format!("{remote} never fetched here -- f fetches"), Role::Warn),
        };
        g.put(y, left, &text, role);
        y += 1;
    }
    if !page.worktree && page.own.is_none() && data.uncommitted > 0 {
        g.put(y, left, &format!("{} on disk not included -- w brings them in", count(data.uncommitted, "uncommitted change")), Role::Muted);
        y += 1;
    }
    if let Some(busy) = &page.busy {
        g.put(y, left, &format!("… {busy}"), Role::Accent);
        y += 1;
    } else if let Some((text, failed)) = &page.message {
        g.put(y, left, &fit(text, width), if *failed { Role::Bad } else { Role::Muted });
        y += 1;
    }
    y += 1;

    let rows = page.rows();
    let commit_of: HashMap<&str, &Commit> = data.ahead.iter().chain(&data.behind).map(|c| (c.hash.as_str(), c)).collect();
    for (i, row) in rows.iter().enumerate() {
        let line_y = y;
        match row {
            Row::Section(section) => {
                let n = match section {
                    Section::Files => data.files.as_ref().map(Vec::len).unwrap_or(0),
                    s => page.commits(*s).len(),
                };
                let folded = page.folded.contains(section);
                let x = g.put(y, left, if folded { "▸ " } else { "▾ " }, Role::Muted);
                let x = g.put(y, x, &format!("{} ({n})", section.title()), Role::Title) + 2;
                let what = match section {
                    Section::Ahead => format!("on {head}, not on {base}"),
                    Section::Behind => format!("on {base}, not on {head}"),
                    Section::Files => String::new(),
                };
                g.put(y, x, &fit(&what, right.saturating_sub(x)), Role::Muted);
                if !folded {
                    match (section, &data.files) {
                        (Section::Files, Err(err)) => {
                            y += 1;
                            g.put(y, left + 4, &format!("couldn't compare {}", range.label()), Role::Bad);
                            for line in err.trim().lines() {
                                y += 1;
                                g.put(y, left + 6, &fit(line, width.saturating_sub(6)), Role::Muted);
                            }
                        }
                        (Section::Files, Ok(files)) if files.is_empty() => {
                            y += 1;
                            g.put(y, left + 4, "No differences.", Role::Muted);
                        }
                        (Section::Ahead | Section::Behind, _) if n == 0 => {
                            y += 1;
                            g.put(y, left + 4, "None.", Role::Muted);
                        }
                        _ => {}
                    }
                }
            }
            Row::Commit { hash, .. } => {
                if let Some(c) = commit_of.get(hash.as_str()) {
                    let x = g.put(y, left + 4, &c.short_hash, Role::Accent) + 2;
                    let age = format!("{} · {}", c.author, c.relative_date);
                    let room = right.saturating_sub(x + age.chars().count() + 2);
                    g.put(y, x, &fit(&c.message, room), Role::Text);
                    g.put(y, right.saturating_sub(age.chars().count()), &age, Role::Muted);
                }
            }
            Row::File(key) => {
                let indent = if matches!(key, DiffKey::Range(_)) { 4 } else { 8 };
                let (letter, label, stats) = match key {
                    DiffKey::Range(path) => match page.changed(path) {
                        Some(f) => {
                            let label = match &f.old_path {
                                Some(old) => format!("{old} → {path}"),
                                None => path.clone(),
                            };
                            (f.letter, label, stat(f))
                        }
                        None => ('M', path.clone(), String::new()),
                    },
                    DiffKey::Commit { hash, path } => {
                        let letter = match page.open.get(hash) {
                            Some(Detail::Files(files)) => files.iter().find(|f| &f.1 == path).map(|f| f.0).unwrap_or('M'),
                            _ => 'M',
                        };
                        (letter, path.clone(), String::new())
                    }
                };
                let x = g.put(y, left + indent, &letter.to_string(), letter_role(letter)) + 2;
                let room = right.saturating_sub(x + stats.chars().count() + 2);
                g.put(y, x, &fit(&label, room), Role::Text);
                g.put(y, right.saturating_sub(stats.chars().count()), &stats, Role::Muted);
            }
            Row::Hunk(key, h) => {
                if let Some(hunk) = page.diff(key).and_then(|d| d.hunks.get(*h)) {
                    g.put(y, left + 8, &fit(&hunk.header, width.saturating_sub(8)), Role::Accent);
                }
            }
            Row::Line(key, h, l) => {
                if let Some(line) = page.diff(key).and_then(|d| d.hunks.get(*h)).and_then(|hunk| hunk.lines.get(*l)) {
                    let number = line.new_line.or(line.old_line).map(|n| n.to_string()).unwrap_or_default();
                    g.put(y, left + 8 + 5usize.saturating_sub(number.len()), &number, Role::Muted);
                    let role = match line.kind {
                        LineKind::Added => Role::Good,
                        LineKind::Removed => Role::Bad,
                        _ => Role::Text,
                    };
                    let text = line.raw().trim_end_matches('\r').replace('\t', "    ");
                    g.put(y, left + 15, &fit(&text, width.saturating_sub(15)), role);
                }
            }
        }
        // What's still being read, or had nothing to show, under its row.
        match row {
            Row::Commit { hash, .. } if page.open.get(hash) == Some(&Detail::Loading) => {
                y += 1;
                g.put(y, left + 8, "reading its files…", Role::Muted);
            }
            Row::File(key) => match page.diffs.get(key) {
                Some(DiffState::Loading) => {
                    y += 1;
                    g.put(y, left + 8, "reading the diff…", Role::Muted);
                }
                Some(DiffState::Empty(why)) => {
                    y += 1;
                    g.put(y, left + 8, &fit(why, width.saturating_sub(8)), Role::Muted);
                }
                _ => {}
            },
            _ => {}
        }
        if i == page.cursor {
            g.focus(line_y, left..right);
            if page.help {
                g.popup = popup(line_y, left + 4, Some((&page.help_menu(), "")), None, None);
            }
        }
        y += 1;
    }
    let keys: &[(&str, &str)] = if page.help {
        &[("Esc", "close")]
    } else {
        &[("Tab", "open"), ("Enter", "go to file"), ("t", "... / .."), ("w", "working tree"), ("s", "turn round"), ("r", "compare with"), ("f", "fetch"), ("?", "keys"), ("q", "close")]
    };
    g.keys(left, width, keys);
    g.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commit(hash: &str, message: &str) -> Commit {
        Commit { hash: hash.repeat(8), short_hash: hash.to_string(), message: message.into(), author: "Ada".into(), relative_date: "2 days ago".into() }
    }

    fn file(letter: char, path: &str, added: usize, removed: usize) -> ChangedFile {
        ChangedFile { letter, path: path.into(), old_path: None, added: Some(added), removed: Some(removed) }
    }

    fn data(page: &GitCompare) -> CompareData {
        CompareData {
            range: page.range(),
            branch: "feature/x".into(),
            merge_base: Some("1a2b3c4d5e6f".into()),
            ahead: vec![commit("aaaaaaa", "add decoder")],
            behind: vec![commit("bbbbbbb", "main moved on"), commit("ccccccc", "and again")],
            files: Ok(vec![file('M', "src/decoder.rs", 12, 3), file('A', "tests/decoder.rs", 40, 0)]),
            remote: Some("origin".into()),
            fetched_secs_ago: Some(3 * 24 * 3600),
            uncommitted: 2,
        }
    }

    fn page() -> GitCompare {
        let mut p = GitCompare::new(PathBuf::from("/repo/fenix"), "origin/main".into(), None);
        p.set_data(data(&p));
        p
    }

    #[test]
    fn the_header_says_where_the_two_sides_stand() {
        let text = layout(&page(), 120).text;
        assert!(text.contains("feature/x compared with origin/main"), "{text}");
        assert!(text.contains("merge base 1a2b3c4 · 1 ahead · 2 behind · 2 files +52 −3"), "{text}");
        assert!(text.contains("origin last fetched") && text.contains("f fetches"), "a three-day-old fetch is stale:\n{text}");
        assert!(text.contains("2 uncommitted changes on disk not included -- w brings them in"), "{text}");
        assert!(text.contains("Ahead (1)") && text.contains("add decoder"), "{text}");
        assert!(text.contains("Behind (2)") && !text.contains("main moved on"), "Behind starts folded:\n{text}");
        assert!(text.contains("+12 −3") && text.contains("+40"), "{text}");
    }

    #[test]
    fn the_cursor_starts_on_the_first_file_and_tab_loads_its_diff() {
        let mut p = page();
        assert_eq!(p.rows()[p.cursor], Row::File(DiffKey::Range("src/decoder.rs".into())));
        let key = DiffKey::Range("src/decoder.rs".into());
        assert_eq!(p.key(Key::Tab), CompareAction::LoadDiff(key.clone()));
        let diff = fenix_diff::parse("diff --git a/src/decoder.rs b/src/decoder.rs\n--- a/src/decoder.rs\n+++ b/src/decoder.rs\n@@ -1 +1 @@\n-old\n+new\n").remove(0);
        p.set_diff(key.clone(), DiffState::Loaded(Box::new(diff)));
        let text = layout(&p, 120).text;
        assert!(text.contains("-old") && text.contains("+new"), "{text}");
        p.key(Key::Char(']'));
        p.key(Key::Char('j'));
        p.key(Key::Char('j'));
        assert_eq!(p.key(Key::Enter), CompareAction::OpenFile { path: "src/decoder.rs".into(), line: Some(1) });
        // Tab inside the diff folds it back to its file.
        p.key(Key::Tab);
        assert_eq!(p.rows()[p.cursor], Row::File(key));
    }

    #[test]
    fn the_modes_change_the_range_and_reread() {
        let mut p = page();
        assert_eq!(p.range().label(), "origin/main...HEAD");
        assert_eq!(p.key(Key::Char('t')), CompareAction::Refresh);
        assert_eq!(p.range().label(), "origin/main..HEAD");
        assert_eq!(p.key(Key::Char('w')), CompareAction::Refresh);
        assert_eq!(p.range().label(), "origin/main..(working tree)");
        assert_eq!(p.key(Key::Char('s')), CompareAction::None, "the working tree can't be turned round");
        p.key(Key::Char('w'));
        assert_eq!(p.key(Key::Char('s')), CompareAction::Refresh);
        assert_eq!(p.range().label(), "HEAD..origin/main");
    }

    #[test]
    fn a_page_waiting_on_a_new_comparison_doesnt_show_the_old_ones_numbers() {
        let mut p = page();
        p.key(Key::Char('t'));
        let text = layout(&p, 120).text;
        assert!(text.contains("Comparing origin/main..HEAD…") && !text.contains("ahead"), "{text}");
    }

    #[test]
    fn a_reply_for_an_older_comparison_is_dropped() {
        let mut p = page();
        let old = data(&p);
        p.key(Key::Char('t'));
        p.data = None;
        p.set_data(old);
        assert!(p.data.is_none());
    }

    #[test]
    fn untracked_files_and_the_working_tree_label_show_in_working_tree_mode() {
        let mut p = GitCompare::new(PathBuf::from("/repo"), "main".into(), None);
        p.worktree = true;
        let mut d = data(&p);
        d.files = Ok(vec![ChangedFile { letter: '?', path: "scratch.txt".into(), old_path: None, added: Some(2), removed: Some(0) }]);
        p.set_data(d);
        let text = layout(&p, 120).text;
        assert!(text.contains("feature/x + working tree compared with main"), "{text}");
        assert!(text.contains("? scratch.txt") || text.contains("?  scratch.txt"), "{text}");
        assert!(!text.contains("not included"), "{text}");
    }

    #[test]
    fn an_error_is_said_in_the_page_not_shown_as_no_differences() {
        let mut p = GitCompare::new(PathBuf::from("/repo"), "nope".into(), None);
        let mut d = data(&p);
        d.files = Err("fatal: ambiguous argument 'nope...HEAD': unknown revision\n".into());
        p.set_data(d);
        let text = layout(&p, 120).text;
        assert!(text.contains("couldn't compare nope...HEAD") && text.contains("unknown revision"), "{text}");
        assert!(!text.contains("No differences"), "{text}");
    }

    #[test]
    fn o_opens_the_targets_version_under_its_old_name() {
        let mut p = GitCompare::new(PathBuf::from("/repo"), "main".into(), None);
        let mut d = data(&p);
        d.files = Ok(vec![ChangedFile { letter: 'R', path: "new.rs".into(), old_path: Some("old.rs".into()), added: Some(1), removed: Some(1) }, file('A', "added.rs", 1, 0)]);
        p.set_data(d);
        assert_eq!(p.key(Key::Char('o')), CompareAction::OpenAt { rev: "main".into(), path: "old.rs".into() });
        p.key(Key::Char('j'));
        assert_eq!(p.key(Key::Char('o')), CompareAction::None, "an added file isn't on the target");
    }

    #[test]
    fn a_commit_opens_to_its_files() {
        let mut p = page();
        p.cursor = p.rows().iter().position(|r| matches!(r, Row::Commit { .. })).unwrap();
        let hash = "aaaaaaa".repeat(8);
        assert_eq!(p.key(Key::Tab), CompareAction::LoadCommit(hash.clone()));
        p.set_files(hash.clone(), vec![('M', "src/a.rs".into())]);
        p.key(Key::Char('j'));
        assert_eq!(p.key(Key::Tab), CompareAction::LoadDiff(DiffKey::Commit { hash, path: "src/a.rs".into() }));
    }

    #[test]
    fn f_fetches_the_targets_remote() {
        let mut p = page();
        assert_eq!(p.key(Key::Char('f')), CompareAction::Fetch("origin".into()));
        assert_eq!(p.key(Key::Char('f')), CompareAction::None, "not twice at once");
    }
}
