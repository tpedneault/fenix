//! The Log page (`SPC g l`): history that acts. The current branch --
//! or every ref, as a graph (`a`) -- one file's history (`SPC g h`), or
//! the history of a few lines (`SPC g H`). `Tab` opens a commit to its
//! files and a file to its diff, inline; `Enter` on a commit is the
//! commit menu -- fix it up with what's staged, reword, revert,
//! cherry-pick, check out, branch or tag there, reset to it.
//!
//! Pure layout and decisions, like the status page; the host
//! (`app/git_log_page.rs`) reads history and runs the jobs.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use fenix_diff::{FileDiff, LineKind};
use fenix_git::{CommitFlags, GraphCommit, GraphRow, ResetMode};

use crate::git_status::{count, danger, popup, short, verb, Action, Compose, Confirm, DiffState, Input, InputPurpose, Job, Menu};
use crate::page::{CardLook, Chip, fit, frame, GraphMark, Grid, Key, NodeLook, Page, Role};

/// Which history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    Branch,
    All,
    /// One file, following renames.
    File(String),
    /// Lines `start..=end` (1-based) of a file.
    Lines { path: String, start: usize, end: usize },
}

impl Scope {
    fn title(&self, branch: &str) -> String {
        match self {
            Scope::Branch => format!("History of {branch}"),
            Scope::All => "History of every branch".to_string(),
            Scope::File(path) => format!("History of {path} · following renames"),
            Scope::Lines { path, start, end } if start == end => format!("History of line {start} of {path}"),
            Scope::Lines { path, start, end } => format!("History of lines {start}–{end} of {path}"),
        }
    }
}

impl GitLog {
    /// What the tab calls the page: the branch it follows (`master`), or
    /// the file, rather than the repository the other pages are named for.
    pub fn tab_name(&self) -> String {
        let file = |path: &str| path.rsplit(['/', '\\']).next().unwrap_or(path).to_string();
        match &self.scope {
            Scope::Branch => self.data.as_ref().map(|d| d.branch.clone()).filter(|b| !b.is_empty()).unwrap_or_else(|| self.name.clone()),
            Scope::All => format!("{} · every branch", self.name),
            Scope::File(path) => file(path),
            Scope::Lines { path, start, end } if start == end => format!("{}:{start}", file(path)),
            Scope::Lines { path, start, end } => format!("{}:{start}–{end}", file(path)),
        }
    }
}

/// What the page shows, read off the UI thread.
#[derive(Debug, Clone, Default)]
pub struct LogData {
    pub commits: Vec<GraphCommit>,
    /// Each commit's place in the graph (`fenix_git::assign_lanes`), one
    /// for one with `commits`.
    pub graph: Vec<GraphRow>,
    /// How many lanes the graph has at its widest.
    pub lanes: usize,
    /// Commits already on the current branch.
    pub on_head: HashSet<String>,
    /// HEAD's commit and the branch's name.
    pub head: Option<String>,
    pub branch: String,
    /// For a line range: how each commit changed those lines.
    pub patches: HashMap<String, Vec<FileDiff>>,
}

/// A commit opened to its files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Detail {
    Loading,
    Files(Vec<(char, String)>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row {
    Commit(String),
    File { hash: String, path: String },
    Hunk { hash: String, path: String, hunk: usize },
    Line { hash: String, path: String, hunk: usize, line: usize },
}

impl Row {
    fn hash(&self) -> Option<&str> {
        match self {
            Row::Commit(h) | Row::File { hash: h, .. } | Row::Hunk { hash: h, .. } | Row::Line { hash: h, .. } => Some(h),
        }
    }
}

/// What the page asks of its host beyond the status page's actions.
#[derive(Debug, Clone, PartialEq)]
pub enum LogAction {
    Git(Action),
    LoadCommit(String),
    LoadFile { hash: String, path: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MenuKind {
    Commit,
    Reset,
    Help,
}

pub struct GitLog {
    pub root: PathBuf,
    pub name: String,
    pub scope: Scope,
    /// `/`'s filter: words to find in messages, or `author:name`.
    pub filter: String,
    pub data: Option<LogData>,
    pub cursor: usize,
    pub open: HashMap<String, Detail>,
    pub diffs: HashMap<(String, String), DiffState>,
    menu: Option<(MenuKind, String, String)>,
    pub input: Option<Input>,
    pub confirm: Option<Confirm>,
    pub message: Option<(String, bool)>,
    pub busy: Option<String>,
    pub output: Vec<String>,
    pub loading: bool,
    pending_g: bool,
}

impl GitLog {
    pub fn new(root: PathBuf, scope: Scope) -> Self {
        let name = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        GitLog {
            root,
            name,
            scope,
            filter: String::new(),
            data: None,
            cursor: 0,
            open: HashMap::new(),
            diffs: HashMap::new(),
            menu: None,
            input: None,
            confirm: None,
            message: None,
            busy: None,
            output: Vec::new(),
            loading: false,
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

    /// The filter as the host passes it on: (words, author).
    pub fn query(&self) -> (Option<String>, Option<String>) {
        let mut words = Vec::new();
        let mut author = None;
        for word in self.filter.split_whitespace() {
            match word.strip_prefix("author:") {
                Some(a) if !a.is_empty() => author = Some(a.to_string()),
                _ => words.push(word),
            }
        }
        ((!words.is_empty()).then(|| words.join(" ")), author)
    }

    pub fn set_data(&mut self, data: LogData) {
        self.loading = false;
        let before = self.rows().get(self.cursor).and_then(|r| r.hash().map(str::to_string));
        // A line range's commits come with their patches: open them all.
        for hash in data.patches.keys() {
            self.open.entry(hash.clone()).or_insert(Detail::Files(Vec::new()));
        }
        self.data = Some(data);
        let rows = self.rows();
        self.cursor = before
            .and_then(|h| rows.iter().position(|r| *r == Row::Commit(h.clone())))
            .unwrap_or(0);
    }

    pub fn set_files(&mut self, hash: String, mut files: Vec<(char, String)>) {
        // A file's history is about that file: show just it, unless it
        // went by another name in this commit.
        if let Scope::File(path) = &self.scope {
            if files.iter().any(|f| &f.1 == path) {
                files.retain(|f| &f.1 == path);
            }
        }
        if self.open.contains_key(&hash) {
            self.open.insert(hash, Detail::Files(files));
        }
    }

    pub fn set_diff(&mut self, hash: String, path: String, diff: DiffState) {
        let key = (hash, path);
        if self.diffs.contains_key(&key) {
            self.diffs.insert(key, diff);
        }
    }

    fn commit(&self, hash: &str) -> Option<&GraphCommit> {
        self.data.as_ref()?.commits.iter().find(|c| c.hash == hash)
    }

    /// The diff shown under a file: the one fetched for it, or for a line
    /// range, the part of the commit's patch about that file.
    fn diff(&self, hash: &str, path: &str) -> Option<&FileDiff> {
        if let Some(DiffState::Loaded(d)) = self.diffs.get(&(hash.to_string(), path.to_string())) {
            return Some(d);
        }
        self.data.as_ref()?.patches.get(hash)?.iter().find(|d| d.display_path() == path)
    }

    pub fn rows(&self) -> Vec<Row> {
        let Some(data) = &self.data else { return Vec::new() };
        let mut rows = Vec::new();
        for commit in &data.commits {
            let hash = &commit.hash;
            rows.push(Row::Commit(hash.clone()));
            let files: Vec<String> = match (self.open.get(hash), data.patches.get(hash)) {
                (Some(_), Some(patches)) => patches.iter().map(|d| d.display_path().to_string()).collect(),
                (Some(Detail::Files(files)), None) => files.iter().map(|f| f.1.clone()).collect(),
                _ => Vec::new(),
            };
            for path in files {
                rows.push(Row::File { hash: hash.clone(), path: path.clone() });
                let shown = data.patches.contains_key(hash) || self.diffs.contains_key(&(hash.clone(), path.clone()));
                if let (true, Some(diff)) = (shown, self.diff(hash, &path)) {
                    for (h, hunk) in diff.hunks.iter().enumerate() {
                        rows.push(Row::Hunk { hash: hash.clone(), path: path.clone(), hunk: h });
                        rows.extend((0..hunk.lines.len()).map(|line| Row::Line { hash: hash.clone(), path: path.clone(), hunk: h, line }));
                    }
                }
            }
        }
        rows
    }

    fn message(&mut self, text: impl Into<String>, failed: bool) -> LogAction {
        self.message = Some((text.into(), failed));
        LogAction::Git(Action::None)
    }

    pub fn key(&mut self, key: Key) -> LogAction {
        if self.input.is_some() {
            return self.input_key(key);
        }
        if let Some(confirm) = &self.confirm {
            let pressed = match key {
                Key::Enter => confirm.choices.first().map(|c| c.0),
                Key::Char(c) => Some(c),
                _ => None,
            };
            if let Some(choice) = confirm.choices.iter().find(|c| Some(c.0) == pressed) {
                let job = choice.2.clone();
                self.confirm = None;
                return LogAction::Git(Action::Run(job));
            }
            if matches!(key, Key::Escape | Key::Char('n') | Key::Char('q')) {
                self.confirm = None;
            }
            return LogAction::Git(Action::None);
        }
        if self.menu.is_some() {
            return self.menu_key(key);
        }
        let rows = self.rows();
        let g = std::mem::take(&mut self.pending_g);
        let none = LogAction::Git(Action::None);
        match key {
            Key::Down | Key::Char('j') => self.step(&rows, true),
            Key::Up | Key::Char('k') => self.step(&rows, false),
            Key::Char('g') if g => {
                self.cursor = 0;
                none
            }
            Key::Char('g') => {
                self.pending_g = true;
                none
            }
            Key::Char('G') => {
                self.cursor = rows.len().saturating_sub(1);
                none
            }
            Key::Tab => self.toggle(&rows),
            Key::Enter => match rows.get(self.cursor).cloned() {
                Some(Row::Commit(hash)) => {
                    self.menu = Some((MenuKind::Commit, String::new(), hash));
                    none
                }
                Some(Row::File { path, .. }) => LogAction::Git(Action::OpenFile { path, line: None }),
                Some(Row::Line { hash, path, hunk, line }) => {
                    let at = self.diff(&hash, &path).and_then(|d| d.hunks.get(hunk)).and_then(|h| h.lines[..=line].iter().rev().find_map(|l| l.new_line));
                    LogAction::Git(Action::OpenFile { path, line: at })
                }
                _ => none,
            },
            Key::Char('/') => {
                self.input = Some(Input { label: "Filter (words in messages, author:name)".to_string(), text: self.filter.clone(), purpose: InputPurpose::PushElsewhere });
                none
            }
            Key::Char('a') => match self.scope {
                Scope::Branch => {
                    self.scope = Scope::All;
                    LogAction::Git(Action::Refresh)
                }
                Scope::All => {
                    self.scope = Scope::Branch;
                    LogAction::Git(Action::Refresh)
                }
                _ => self.message("a switches between this branch and every branch -- not for a file's history", false),
            },
            Key::Char('y') => match rows.get(self.cursor).and_then(|r| r.hash()) {
                Some(hash) => LogAction::Git(Action::Copy(hash.to_string())),
                None => none,
            },
            Key::Char('?') | Key::Char('x') => {
                self.menu = Some((MenuKind::Help, String::new(), String::new()));
                none
            }
            Key::Char('u') => LogAction::Git(Action::Refresh),
            Key::Char('$') => LogAction::Git(Action::ShowOutput),
            Key::Char('q') => LogAction::Git(Action::Close),
            _ => none,
        }
    }

    fn step(&mut self, rows: &[Row], forward: bool) -> LogAction {
        let found = if forward { (self.cursor + 1 < rows.len()).then_some(self.cursor + 1) } else { self.cursor.checked_sub(1) };
        if let Some(i) = found {
            self.cursor = i;
        }
        LogAction::Git(Action::None)
    }

    fn toggle(&mut self, rows: &[Row]) -> LogAction {
        let lines_scope = matches!(self.scope, Scope::Lines { .. });
        match rows.get(self.cursor).cloned() {
            Some(Row::Commit(hash)) => {
                if self.open.remove(&hash).is_some() {
                    return LogAction::Git(Action::None);
                }
                if lines_scope {
                    self.open.insert(hash, Detail::Files(Vec::new()));
                    return LogAction::Git(Action::None);
                }
                self.open.insert(hash.clone(), Detail::Loading);
                LogAction::LoadCommit(hash)
            }
            Some(Row::File { hash, path }) if !lines_scope => {
                let key = (hash.clone(), path.clone());
                if self.diffs.remove(&key).is_some() {
                    return LogAction::Git(Action::None);
                }
                self.diffs.insert(key, DiffState::Loading);
                LogAction::LoadFile { hash, path }
            }
            Some(Row::Hunk { hash, path, .. } | Row::Line { hash, path, .. }) if !lines_scope => {
                self.diffs.remove(&(hash.clone(), path.clone()));
                self.cursor = self.rows().iter().position(|r| *r == Row::File { hash: hash.clone(), path: path.clone() }).unwrap_or(self.cursor);
                LogAction::Git(Action::None)
            }
            _ => LogAction::Git(Action::None),
        }
    }

    /// The commit menu for `hash`, as things stand.
    fn menu(&self, kind: MenuKind, hash: &str) -> Menu {
        let data = self.data.as_ref();
        let branch = data.map(|d| d.branch.clone()).unwrap_or_default();
        let on_head = data.is_some_and(|d| d.on_head.contains(hash));
        let is_head = data.and_then(|d| d.head.as_deref()) == Some(hash);
        let merge = self.commit(hash).is_some_and(|c| c.parents.len() > 1) && !matches!(self.scope, Scope::File(_) | Scope::Lines { .. });
        let subject = self.commit(hash).map(|c| c.subject.clone()).unwrap_or_default();
        match kind {
            MenuKind::Commit => {
                let mut items = Vec::new();
                if on_head && !merge {
                    items.push(verb("f", "fixup into this", "what's staged, squashed in now"));
                }
                if is_head {
                    items.push(verb("w", "reword", "the last commit's message"));
                }
                items.push(verb("v", "revert", "a new commit undoing it"));
                if !on_head {
                    items.push(verb("c", "cherry-pick", format!("onto {branch}")));
                }
                items.push(verb("o", "check out", "detached, to look around"));
                items.push(verb("b", "branch here…", ""));
                items.push(verb("t", "tag here…", ""));
                if on_head {
                    items.push(verb("i", "rebase from here…", "reorder, reword, squash, drop"));
                    items.push(danger(verb("r", "reset to here…", "soft / mixed / hard")));
                }
                items.push(verb("y", "copy hash", ""));
                Menu { title: format!("{} {}", short(hash), fit(&subject, 44)), groups: vec![(None, items)] }
            }
            MenuKind::Reset => Menu {
                title: format!("Reset {branch} to {}", short(hash)),
                groups: vec![(None, vec![verb("s", "soft", "keep index and files"), verb("m", "mixed", "keep files"), danger(verb("h", "hard", "discard everything since"))])],
            },
            MenuKind::Help => Menu {
                title: "Log keys".to_string(),
                groups: vec![(
                    None,
                    vec![
                        verb("Tab", "open a commit / a file", "its files, then the diff"),
                        verb("Enter", "commit menu", "or open the file"),
                        verb("/", "filter", "words, or author:name"),
                        verb("a", "all branches", "or back to this one"),
                        verb("y", "copy hash", ""),
                        verb("u", "refresh", ""),
                        verb("q", "close", ""),
                    ],
                )],
            },
        }
    }

    fn menu_key(&mut self, key: Key) -> LogAction {
        let Some((kind, _, hash)) = self.menu.clone() else { return LogAction::Git(Action::None) };
        let none = LogAction::Git(Action::None);
        let c = match key {
            Key::Escape | Key::Char('q') => {
                self.menu = None;
                return none;
            }
            Key::Char(c) => c,
            _ => return none,
        };
        let menu = self.menu(kind, &hash);
        let offered = menu.groups.iter().flat_map(|(_, items)| items).any(|i| i.key == c.to_string());
        if kind == MenuKind::Help {
            self.menu = None;
            return if c == '?' { none } else { self.key(key) };
        }
        if !offered {
            return self.message(format!("{c} isn't in this menu -- Esc closes it"), false);
        }
        self.menu = None;
        let run = |job: Job| LogAction::Git(Action::Run(job));
        match (kind, c) {
            (MenuKind::Commit, 'f') => run(Job::FixupNow { hash, flags: CommitFlags::default() }),
            (MenuKind::Commit, 'w') => LogAction::Git(Action::Compose(Compose::Reword(CommitFlags::default()))),
            (MenuKind::Commit, 'v') => run(Job::Revert(hash)),
            (MenuKind::Commit, 'c') => run(Job::CherryPick(hash)),
            (MenuKind::Commit, 'o') => run(Job::Checkout(hash)),
            (MenuKind::Commit, 'b') => {
                self.input = Some(Input { label: format!("New branch at {}", short(&hash)), text: String::new(), purpose: InputPurpose::BranchAt(hash) });
                none
            }
            (MenuKind::Commit, 't') => {
                self.input = Some(Input { label: format!("Tag {}", short(&hash)), text: String::new(), purpose: InputPurpose::TagAt(hash) });
                none
            }
            (MenuKind::Commit, 'r') => {
                self.menu = Some((MenuKind::Reset, String::new(), hash));
                none
            }
            (MenuKind::Commit, 'y') => LogAction::Git(Action::Copy(hash)),
            (MenuKind::Commit, 'i') => LogAction::Git(Action::Rebase(Some(hash))),
            (MenuKind::Reset, 's') => run(Job::Reset { target: hash, mode: ResetMode::Soft }),
            (MenuKind::Reset, 'm') => run(Job::Reset { target: hash, mode: ResetMode::Mixed }),
            (MenuKind::Reset, 'h') => {
                self.confirm = Some(Confirm {
                    question: format!("Reset --hard to {}? Uncommitted changes are saved first; U on the Git page brings them back.", short(&hash)),
                    detail: Vec::new(),
                    choices: vec![('y', "reset --hard".to_string(), Job::Reset { target: hash, mode: ResetMode::Hard })],
                });
                none
            }
            _ => none,
        }
    }

    fn input_key(&mut self, key: Key) -> LogAction {
        let none = LogAction::Git(Action::None);
        let Some(input) = &mut self.input else { return none };
        match key {
            Key::Escape => self.input = None,
            Key::Backspace => {
                input.text.pop();
            }
            Key::Char(c) => input.text.push(c),
            Key::Space => input.text.push(' '),
            Key::Enter => {
                let Some(input) = self.input.take() else { return none };
                let text = input.text.trim().to_string();
                return match input.purpose {
                    InputPurpose::BranchAt(hash) if !text.is_empty() => LogAction::Git(Action::Run(Job::Branch { name: text, start: Some(hash), switch: false })),
                    InputPurpose::TagAt(hash) if !text.is_empty() => LogAction::Git(Action::Run(Job::Tag { name: text, at: hash })),
                    // The filter (reusing a purpose the log has no other use for).
                    InputPurpose::PushElsewhere => {
                        self.filter = text;
                        LogAction::Git(Action::Refresh)
                    }
                    _ => none,
                };
            }
            _ => {}
        }
        none
    }
}

/// The colour of lane `lane`: the theme's accents in turn, so a branch
/// keeps its colour down the page.
fn lane_role(lane: usize) -> Role {
    const LANES: [&str; 6] = ["attribute", "function", "string", "keyword", "type", "number"];
    Role::Syntax(LANES[lane % LANES.len()])
}

/// The cells the graph takes for `lanes` lanes on a page `width` wide:
/// two a lane, up to a fifth of the page. Past that the lanes are drawn
/// closer together, and past that the rightmost fold into the last
/// (`App`'s drawing), so a history with many branches keeps room for
/// its messages.
fn graph_cols(lanes: usize, width: usize) -> usize {
    (lanes.max(1) * 2).min((width / 5).max(6))
}

/// How many rows either side of a commit decide where its text starts:
/// enough that it doesn't step left and right with every branch that
/// opens and closes nearby.
const TEXT_WINDOW: usize = 4;

/// A commit's row of the graph: the rails of the lanes passing it, its
/// own lane in from above (when a child heads here) and on down (when it
/// has a parent), the lanes ending at it bending in, the ones a merge
/// opens bending out, and its dot -- `head` when HEAD is on it.
pub(crate) fn commit_marks(row: &GraphRow, head: bool) -> Vec<GraphMark> {
    let mut marks = Vec::new();
    for &lane in &row.live_before {
        if lane != row.lane && !row.closing.contains(&lane) {
            marks.push(GraphMark::Rail { lane, top: true, bottom: row.live_after.contains(&lane), role: lane_role(lane) });
        }
    }
    let (top, bottom) = (row.live_before.contains(&row.lane), row.live_after.contains(&row.lane));
    if top || bottom {
        marks.push(GraphMark::Rail { lane: row.lane, top, bottom, role: lane_role(row.lane) });
    }
    for &from in &row.closing {
        marks.push(GraphMark::Bend { from, to: row.lane, into: true, role: lane_role(from) });
    }
    for &to in &row.branching {
        marks.push(GraphMark::Bend { from: row.lane, to, into: false, role: lane_role(to) });
    }
    let look = if head {
        NodeLook::Head
    } else if row.is_merge {
        NodeLook::Merge
    } else {
        NodeLook::Commit
    };
    marks.push(GraphMark::Node { lane: row.lane, look, role: lane_role(row.lane) });
    marks
}

/// The graph through a row under a commit (its details, files, diff):
/// the lanes live below it, straight on.
pub(crate) fn through_marks(row: &GraphRow) -> Vec<GraphMark> {
    row.live_after.iter().map(|&lane| GraphMark::Rail { lane, top: true, bottom: true, role: lane_role(lane) }).collect()
}

/// A person's initials, for their badge: `Thomas Pedneault` is `TP`.
fn initials(name: &str) -> String {
    let words: Vec<&str> = name.split(|c: char| c.is_whitespace() || c == '.' || c == '-' || c == '_').filter(|w| !w.is_empty()).collect();
    let pick: String = match words.as_slice() {
        [] => "?".into(),
        [one] => one.chars().take(2).collect(),
        [first, .., last] => first.chars().take(1).chain(last.chars().take(1)).collect(),
    };
    pick.to_uppercase()
}

pub fn layout(page: &GitLog, cols: usize) -> Page {
    let (left, width) = frame(cols, 150);
    let mut g = Grid::new();
    let mut y = 1;
    let Some(data) = &page.data else {
        g.put(y, left, &format!("Reading the history of {}…", page.name), Role::Muted);
        return g.finish();
    };
    let x = g.put(y, left, &fit(&page.scope.title(&data.branch), width), Role::Title) + 2;
    let authors: std::collections::HashSet<&str> = data.commits.iter().map(|c| c.author.as_str()).collect();
    let x = g.chip_within(y, x, left + width, &format!(" {} ", count(data.commits.len(), "commit")), Role::Muted, Chip::Badge) + 1;
    g.chip_within(y, x, left + width, &format!(" {} ", count(authors.len(), "author")), Role::Muted, Chip::Badge);
    y += 1;
    if !page.filter.is_empty() {
        let x = g.put(y, left, "filtered by", Role::Muted) + 1;
        let x = g.put(y, x, &page.filter, Role::Accent) + 2;
        g.put(y, x, "/ changes it", Role::Muted);
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
    if data.commits.is_empty() {
        g.put(y, left, "No commits match.", Role::Muted);
    }
    let rows = page.rows();
    // Each commit's place, looked up once rather than scanned for on
    // every row: with every branch, a history runs to hundreds.
    let index_of: HashMap<&str, usize> = data.commits.iter().enumerate().map(|(n, c)| (c.hash.as_str(), n)).collect();
    let list_top = y;
    let left = left + 1;
    let width = width.saturating_sub(2);
    // The graph's cells, then everything else after them.
    let gcols = if data.commits.is_empty() { 0 } else { graph_cols(data.lanes, width) };
    // A cell clear of the focused row's rail, then the graph.
    let graph_at = left + 1..left + 1 + gcols;
    // The lanes keep one spacing down the whole graph, so its lines meet
    // from row to row; the text starts just past the lanes in use around
    // its row, rather than past the widest the history ever gets -- the
    // newest commits of a repository with many branches often need few.
    let extent: Vec<usize> = data.graph.iter().map(|r| r.live_before.iter().chain(&r.live_after).copied().chain([r.lane]).max().map_or(1, |l| l + 1)).collect();
    let pitch = (gcols as f32 / data.lanes.max(1) as f32).clamp(0.75, 2.0);
    let shown = ((gcols as f32 / pitch).floor() as usize).max(1);
    let text_at = |n: usize| {
        let near = extent.get(n.saturating_sub(TEXT_WINDOW)..(n + TEXT_WINDOW + 1).min(extent.len())).and_then(|e| e.iter().copied().max()).unwrap_or(1);
        // Past two lanes, room for four more at a time: the text moves in
        // a few steps, not a cell with every lane.
        let near = if near <= 2 { near } else { near.div_ceil(4) * 4 };
        graph_at.start + ((near.min(shown) as f32 * pitch).ceil() as usize).min(gcols) + 1
    };
    let hash_width = data.commits.iter().map(|c| c.short_hash.chars().count()).max().unwrap_or(7);
    // Who and when, in columns of their own at the right: the badges line
    // up whatever the dates' lengths.
    let who_width = data.commits.iter().map(|c| initials(&c.author).chars().count()).max().unwrap_or(2);
    let age_width = data.commits.iter().map(|c| c.relative_date.chars().count()).max().unwrap_or(0).min(20);
    // The commit whose lanes run on down through the rows under it.
    let mut through: Option<usize> = None;
    for (i, row) in rows.iter().enumerate() {
        let line_y = y;
        // Where this row's text starts: its commit's, or the one it's under.
        let owner = match row {
            Row::Commit(h) => index_of.get(h.as_str()).copied(),
            _ => through,
        };
        let inner = owner.map(text_at).unwrap_or(graph_at.end + 1);
        // The graph straight on through a row under a commit.
        let carry = |g: &mut Grid, y: usize, through: Option<usize>| {
            if let Some(gr) = through.and_then(|n| data.graph.get(n)) {
                g.graph(y, graph_at.clone(), data.lanes, through_marks(gr));
            }
        };
        match row {
            Row::Commit(hash) => {
                let Some(&n) = index_of.get(hash.as_str()) else { continue };
                let c = &data.commits[n];
                if let Some(gr) = data.graph.get(n) {
                    g.graph(y, graph_at.clone(), data.lanes, commit_marks(gr, data.head.as_deref() == Some(hash.as_str())));
                }
                through = Some(n);
                let age = fit(&c.relative_date, age_width);
                let room = width.saturating_sub(age_width + who_width + 6);
                // The hash, the refs as chips and the subject, one after
                // another.
                let mut x = g.put(y, inner, &format!("{:<hash_width$}", c.short_hash), Role::Muted) + 1;
                for r in c.refs.iter().filter(|r| !r.is_empty()) {
                    let (shown, role) = match r.as_str() {
                        r if r.starts_with("HEAD") => (r.replace("->", "→"), Role::Accent),
                        r if r.starts_with("tag: ") => (r.trim_start_matches("tag: ").to_string(), Role::Good),
                        r => (r.to_string(), Role::Syntax("function")),
                    };
                    if x + shown.chars().count() + 3 < left + room {
                        x = g.chip(y, x, &format!(" {shown} "), role, Chip::Status(role)) + 1;
                    }
                }
                let role = if data.on_head.contains(hash) || matches!(page.scope, Scope::File(_) | Scope::Lines { .. }) { Role::Text } else { Role::Muted };
                g.put(y, x, &fit(c.subject.trim(), (left + room).saturating_sub(x)), role);
                // Who, on a badge, and when.
                let who = initials(&c.author);
                let role = lane_role(c.author.bytes().map(usize::from).sum::<usize>());
                let at = (left + width).saturating_sub(age_width + who_width + 4);
                g.chip(y, at, &format!(" {who:^who_width$} "), role, Chip::Status(role));
                g.put(y, (left + width).saturating_sub(age_width), &age, Role::Muted);
                // The commit the cursor is on, opened under it: who in full,
                // when, and the whole hash -- kept inside its card, which
                // hugs its rows; its keys are on the bar at the foot.
                if i == page.cursor {
                    y += 1;
                    carry(&mut g, y, through);
                    let end = left + width - 1;
                    let x = g.put(y, inner, &fit(&c.author, 32.min(end.saturating_sub(inner))), Role::Text) + 1;
                    let when = format!("· {}", c.relative_date);
                    let x = if x + when.chars().count() <= end { g.put(y, x, &when, Role::Muted) + 1 } else { x };
                    if x + 10 <= end {
                        g.put(y, x, &fit(&format!("· {}", c.hash), end - x), Role::Muted);
                    }
                    g.card(line_y..y + 1, left.saturating_sub(1)..left + width + 1, CardLook::Selected);
                }
            }
            Row::File { hash, path } => {
                carry(&mut g, y, through);
                let letter = match page.open.get(hash) {
                    Some(Detail::Files(files)) => files.iter().find(|f| &f.1 == path).map(|f| f.0).unwrap_or('M'),
                    _ => 'M',
                };
                let role = match letter {
                    'A' => Role::Good,
                    'D' => Role::Bad,
                    _ => Role::Warn,
                };
                let x = g.put(y, inner + 1, &letter.to_string(), role) + 2;
                g.put(y, x, &fit(path, (left + width).saturating_sub(x)), Role::Text);
            }
            Row::Hunk { hash, path, hunk } => {
                carry(&mut g, y, through);
                if let Some(h) = page.diff(hash, path).and_then(|d| d.hunks.get(*hunk)) {
                    g.put(y, inner + 3, &fit(&h.header, (left + width).saturating_sub(inner + 3)), Role::Accent);
                }
            }
            Row::Line { hash, path, hunk, line } => {
                carry(&mut g, y, through);
                if let Some(l) = page.diff(hash, path).and_then(|d| d.hunks.get(*hunk)).and_then(|h| h.lines.get(*line)) {
                    let number = l.new_line.or(l.old_line).map(|n| n.to_string()).unwrap_or_default();
                    g.put(y, inner + 3 + 5usize.saturating_sub(number.len()), &number, Role::Muted);
                    let role = match l.kind {
                        LineKind::Added => Role::Good,
                        LineKind::Removed => Role::Bad,
                        _ => Role::Text,
                    };
                    let text = l.raw().trim_end_matches('\r').replace('\t', "    ");
                    g.put(y, inner + 10, &fit(&text, (left + width).saturating_sub(inner + 10)), role);
                }
            }
        }
        if let Row::Commit(hash) = row {
            if page.open.get(hash) == Some(&Detail::Loading) {
                y += 1;
                carry(&mut g, y, through);
                g.put(y, inner + 1, "reading its files…", Role::Muted);
            }
        }
        if let Row::File { hash, path } = row {
            if let Some(DiffState::Loading) = page.diffs.get(&(hash.clone(), path.clone())) {
                y += 1;
                carry(&mut g, y, through);
                g.put(y, inner + 3, "reading the diff…", Role::Muted);
            }
        }
        if i == page.cursor {
            g.focus(line_y, left..left + width);
            let menu = page.menu.as_ref().map(|(kind, typed, hash)| (page.menu(*kind, hash), typed.clone()));
            g.popup = popup(line_y, left + 4, menu.as_ref().map(|(m, t)| (m, t.as_str())), page.input.as_ref(), page.confirm.as_ref());
        }
        y += 1;
    }
    // The history on a card, under the selected commit's.
    if y > list_top && !rows.is_empty() {
        g.card_behind(list_top..y, left.saturating_sub(2)..left + width + 2, CardLook::Plain);
    }
    let keys: Vec<(&str, &str)> = if page.input.is_some() {
        vec![("Enter", "done"), ("Esc", "cancel")]
    } else if page.menu.is_some() || page.confirm.is_some() {
        vec![("Esc", "close")]
    } else {
        vec![("Tab", "open"), ("Enter", "commit menu"), ("/", "filter"), ("a", "all branches"), ("y", "copy hash"), ("u", "refresh"), ("?", "keys"), ("q", "close")]
    };
    g.keys(left, width, &keys);
    g.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Nothing runs past the pane's edge, from a narrow split up.
    #[test]
    fn it_keeps_inside_a_narrow_pane() {
        let mut p = page(); p.key(Key::Char('j'));
        for cols in [34, 38, 40, 50, 60, 72, 80, 100, 120, 160] {
            let over = crate::page::overflow(&layout(&p, cols), cols);
            assert!(over.is_empty(), "{cols} cols: {over:#?}");
        }
    }

    fn commit(n: usize, parents: &[usize], subject: &str) -> GraphCommit {
        let hash = format!("{n:0>40}");
        GraphCommit {
            short_hash: hash[33..].to_string(),
            parents: parents.iter().map(|p| format!("{p:0>40}")).collect(),
            refs: Vec::new(),
            author: "Ada".into(),
            relative_date: "2 days ago".into(),
            subject: subject.into(),
            hash,
        }
    }

    fn data(commits: Vec<GraphCommit>, on_head: &[usize]) -> LogData {
        let graph = fenix_git::assign_lanes(&commits);
        LogData {
            head: commits.first().map(|c| c.hash.clone()),
            on_head: on_head.iter().map(|n| format!("{n:0>40}")).collect(),
            branch: "main".into(),
            lanes: fenix_git::lane_count(&graph),
            graph,
            commits,
            patches: HashMap::new(),
        }
    }

    fn page() -> GitLog {
        let mut p = GitLog::new(PathBuf::from("/repo/fenix"), Scope::Branch);
        p.set_data(data(vec![commit(3, &[2], "third"), commit(2, &[1], "second"), commit(1, &[], "first")], &[3, 2, 1]));
        p
    }

    fn hash(n: usize) -> String {
        format!("{n:0>40}")
    }

    /// The graph is drawn, not written: each commit's row has its marks
    /// over blank cells, HEAD's commit ringed, a merge bending out to its
    /// second parent, and the lanes running on through an opened commit.
    #[test]
    fn the_graph_is_drawn_over_blank_cells() {
        let mut p = GitLog::new(PathBuf::from("/repo/fenix"), Scope::All);
        p.set_data(data(vec![commit(4, &[3, 2], "merge"), commit(3, &[1], "main work"), commit(2, &[1], "side work"), commit(1, &[], "first")], &[4, 3, 1]));
        let page = layout(&p, 120);
        assert!(!page.text.contains('*') && !page.text.contains('\\') && !page.text.contains("|/"), "{}", page.text);
        let merge = &page.graph[0];
        assert_eq!(merge.lanes, 2);
        assert!(merge.marks.contains(&GraphMark::Node { lane: 0, look: NodeLook::Head, role: lane_role(0) }));
        assert!(merge.marks.contains(&GraphMark::Bend { from: 0, to: 1, into: false, role: lane_role(1) }), "{:?}", merge.marks);
        // The cursor's commit is opened under it: the lanes run on through.
        let opened = &page.graph[1];
        assert_eq!(opened.line, merge.line + 1);
        assert!(opened.marks.iter().all(|m| matches!(m, GraphMark::Rail { top: true, bottom: true, .. })) && opened.marks.len() == 2);
        // Both sides meet again at the root: lane 1 bends in.
        let root = page.graph.last().unwrap();
        assert!(root.marks.contains(&GraphMark::Bend { from: 1, to: 0, into: true, role: lane_role(1) }), "{:?}", root.marks);
    }

    /// Where few lanes are in use, the text starts just past them, though
    /// the history is wider further down.
    #[test]
    fn the_text_starts_past_the_lanes_in_use_nearby() {
        // Ten commits in a line, then six branches off the root.
        let mut commits: Vec<GraphCommit> = (11..=20).rev().map(|n| commit(n, &[if n == 11 { 1 } else { n - 1 }], "line")).collect();
        commits.extend((2..=7).map(|n| commit(n, &[1], "tip")));
        commits.push(commit(1, &[], "root"));
        let mut p = GitLog::new(PathBuf::from("/repo/fenix"), Scope::All);
        p.set_data(data(commits, &[]));
        let page = layout(&p, 120);
        let col = |n: usize| page.text.lines().find_map(|l| l.find(&hash(n)[33..])).unwrap();
        assert!(col(20) + 6 <= col(5), "the top's one lane: {} vs {}\n{}", col(20), col(5), page.text);
        assert_eq!(col(20), col(19), "steady from row to row");
    }

    /// A history with many branches live at once keeps its graph to a
    /// fifth of the page; the drawing closes the lanes up to fit.
    #[test]
    fn many_lanes_keep_the_graph_narrow() {
        assert_eq!(graph_cols(1, 150), 2);
        assert_eq!(graph_cols(8, 150), 16);
        assert_eq!(graph_cols(200, 150), 30);
        assert_eq!(graph_cols(200, 20), 6, "never narrower than three lanes' room");
    }

    #[test]
    fn a_persons_badge_is_their_initials() {
        assert_eq!(initials("Thomas Pedneault"), "TP");
        assert_eq!(initials("ada"), "AD");
        assert_eq!(initials("Grace B. Hopper"), "GH");
        assert_eq!(initials(""), "?");
    }

    #[test]
    fn the_tab_names_the_branch_or_the_file() {
        let p = page();
        assert_eq!(p.tab_name(), "main", "the branch, once read");
        let mut f = GitLog::new(PathBuf::from("/repo/fenix"), Scope::File("src/app/main.rs".into()));
        assert_eq!(f.tab_name(), "main.rs");
        f.scope = Scope::Lines { path: "src/a.rs".into(), start: 3, end: 9 };
        assert_eq!(f.tab_name(), "a.rs:3–9");
        let all = GitLog::new(PathBuf::from("/repo/fenix"), Scope::All);
        assert_eq!(all.tab_name(), "fenix · every branch");
    }

    #[test]
    fn the_cursor_moves_between_commits_and_tab_opens_one() {
        let mut p = page();
        assert_eq!(p.rows()[p.cursor], Row::Commit(hash(3)));
        p.key(Key::Char('j'));
        assert_eq!(p.rows()[p.cursor], Row::Commit(hash(2)));
        assert_eq!(p.key(Key::Tab), LogAction::LoadCommit(hash(2)));
        p.set_files(hash(2), vec![('M', "src/a.rs".into())]);
        p.key(Key::Char('j'));
        assert_eq!(p.rows()[p.cursor], Row::File { hash: hash(2), path: "src/a.rs".into() });
        assert_eq!(p.key(Key::Tab), LogAction::LoadFile { hash: hash(2), path: "src/a.rs".into() });
        let text = layout(&p, 120).text;
        assert!(text.contains("History of main") && text.contains("second") && text.contains("src/a.rs"), "{text}");
    }

    /// The open commit's row of who and when stays on its card however
    /// narrow the page, and keeps no keys to poke through its edge.
    #[test]
    fn the_open_commit_stays_inside_its_card() {
        let mut p = page();
        p.data.as_mut().unwrap().commits[0].author = "Margaret Hamilton-Lovelace of the Apollo Guidance Computer".into();
        for cols in [50, 70, 120] {
            let out = layout(&p, cols);
            let (left, width) = frame(cols, 150);
            let card = out.cards.iter().find(|c| c.look == CardLook::Selected).expect("the open commit's card");
            let detail = card.lines.end - 1;
            let line = out.text.lines().nth(detail).unwrap_or_default().trim_end();
            assert!(line.chars().count() <= left + width, "{cols}: {line:?}");
            assert!(!out.chips.iter().any(|(l, _, _)| *l == detail), "{cols}: no key caps on the open row");
            assert!(line.contains("Margaret"), "{line:?}");
        }
        assert!(layout(&p, 120).text.contains(&hash(3)), "the whole hash, room allowing");
    }

    #[test]
    fn the_commit_menu_offers_what_fits_the_commit() {
        let mut p = page();
        p.key(Key::Char('j'));
        p.key(Key::Enter);
        let text = layout(&p, 120).all_text();
        assert!(text.contains("fixup into this") && text.contains("revert") && !text.contains("cherry-pick"), "on this branch: no cherry-pick\n{text}");
        assert!(!text.contains("the last commit's message"), "only the last commit can be reworded");
        assert_eq!(p.key(Key::Char('v')), LogAction::Git(Action::Run(Job::Revert(hash(2)))));
        // A commit from elsewhere can be cherry-picked, not fixed up.
        let mut p = page();
        p.data.as_mut().unwrap().on_head.remove(&hash(2));
        p.key(Key::Char('j'));
        p.key(Key::Enter);
        let text = layout(&p, 120).all_text();
        assert!(text.contains("cherry-pick") && !text.contains("fixup into this"), "{text}");
        assert_eq!(p.key(Key::Char('c')), LogAction::Git(Action::Run(Job::CherryPick(hash(2)))));
    }

    #[test]
    fn a_hard_reset_asks_first() {
        let mut p = page();
        p.key(Key::Char('G'));
        p.key(Key::Enter);
        p.key(Key::Char('r'));
        assert_eq!(p.key(Key::Char('h')), LogAction::Git(Action::None));
        assert_eq!(p.key(Key::Char('y')), LogAction::Git(Action::Run(Job::Reset { target: hash(1), mode: ResetMode::Hard })));
    }

    /// The author badges sit in one column whatever the dates' lengths
    /// and whoever's initials are one letter or two.
    #[test]
    fn author_badges_line_up_down_the_page() {
        let mut p = GitLog::new(PathBuf::from("/repo/fenix"), Scope::Branch);
        let mut a = commit(3, &[2], "third");
        a.relative_date = "5 minutes ago".into();
        let mut b = commit(2, &[1], "second");
        (b.relative_date, b.author) = ("2 days ago".into(), "Q".into());
        let mut c = commit(1, &[], "first");
        (c.relative_date, c.author) = ("11 months ago".into(), "Grace Hopper".into());
        p.set_data(data(vec![a, b, c], &[3, 2, 1]));
        let out = layout(&p, 120);
        let badges: Vec<_> = out.chips.iter().filter(|(_, cols, _)| cols.start > 80 && cols.len() == 4).map(|(_, cols, _)| cols.clone()).collect();
        assert_eq!(badges.len(), 3, "{badges:?}");
        assert!(badges.iter().all(|r| *r == badges[0]), "{badges:?}");
    }

    #[test]
    fn the_filter_splits_words_from_an_author() {
        let mut p = page();
        p.key(Key::Char('/'));
        p.type_text("decoder author:ada");
        assert_eq!(p.key(Key::Enter), LogAction::Git(Action::Refresh));
        assert_eq!(p.query(), (Some("decoder".into()), Some("ada".into())));
        assert!(layout(&p, 120).text.contains("filtered by decoder author:ada"));
    }

    #[test]
    fn a_line_range_shows_each_commits_patch_already_open() {
        let mut p = GitLog::new(PathBuf::from("/repo"), Scope::Lines { path: "a.txt".into(), start: 2, end: 2 });
        let mut d = data(vec![commit(2, &[1], "second"), commit(1, &[], "first")], &[2, 1]);
        let patch = fenix_diff::parse("diff --git a/a.txt b/a.txt\n--- a/a.txt\n+++ b/a.txt\n@@ -2 +2 @@\n-two\n+TWO\n");
        d.patches.insert(hash(2), patch);
        p.set_data(d);
        let text = layout(&p, 120).text;
        assert!(text.contains("History of line 2 of a.txt") && text.contains("+TWO"), "{text}");
    }

    #[test]
    fn a_toggles_every_branch() {
        let mut p = page();
        assert_eq!(p.key(Key::Char('a')), LogAction::Git(Action::Refresh));
        assert_eq!(p.scope, Scope::All);
    }
}
