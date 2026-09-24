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
use fenix_git::{CommitFlags, GraphCommit, ResetMode};

use crate::git_status::{count, danger, draw_popups, short, verb, Action, Compose, Confirm, DiffState, Input, InputPurpose, Job, Menu};
use crate::graph_view::GraphSpan;
use crate::page::{fit, frame, Grid, Key, Page, Role};

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

/// One drawn line of the graph: its text, the spans that colour it, and
/// the commit it draws (a connector row draws none).
#[derive(Debug, Clone, PartialEq)]
pub struct GraphLine {
    pub text: String,
    pub spans: Vec<(usize, GraphSpan)>,
    pub commit: Option<String>,
}

/// What the page shows, read off the UI thread.
#[derive(Debug, Clone, Default)]
pub struct LogData {
    pub commits: Vec<GraphCommit>,
    pub lines: Vec<GraphLine>,
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
    /// A connector line of the graph, by index -- drawn, never the cursor's.
    Rail(usize),
    Commit(String),
    File { hash: String, path: String },
    Hunk { hash: String, path: String, hunk: usize },
    Line { hash: String, path: String, hunk: usize, line: usize },
}

impl Row {
    fn focusable(&self) -> bool {
        !matches!(self, Row::Rail(_))
    }

    fn hash(&self) -> Option<&str> {
        match self {
            Row::Rail(_) => None,
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
            .or_else(|| rows.iter().position(Row::focusable))
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
        for (i, line) in data.lines.iter().enumerate() {
            let Some(hash) = &line.commit else {
                rows.push(Row::Rail(i));
                continue;
            };
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
                self.cursor = rows.iter().position(Row::focusable).unwrap_or(0);
                none
            }
            Key::Char('g') => {
                self.pending_g = true;
                none
            }
            Key::Char('G') => {
                self.cursor = rows.iter().rposition(Row::focusable).unwrap_or(0);
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
        let found = if forward {
            rows.iter().enumerate().skip(self.cursor + 1).find(|(_, r)| r.focusable()).map(|(i, _)| i)
        } else {
            rows.iter().enumerate().take(self.cursor).rev().find(|(_, r)| r.focusable()).map(|(i, _)| i)
        };
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

pub fn layout(page: &GitLog, cols: usize) -> Page {
    let (left, width) = frame(cols, 150);
    let mut g = Grid::new();
    let mut y = 1;
    let Some(data) = &page.data else {
        g.put(y, left, &format!("Reading the history of {}…", page.name), Role::Muted);
        return g.finish();
    };
    let x = g.put(y, left, &page.scope.title(&data.branch), Role::Title) + 2;
    g.put(y, x, &count(data.commits.len(), "commit"), Role::Muted);
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
    for (i, row) in rows.iter().enumerate() {
        let line_y = y;
        match row {
            Row::Rail(n) => {
                if let Some(line) = data.lines.get(*n) {
                    g.put(y, left, &line.text, Role::Muted);
                }
            }
            Row::Commit(hash) => {
                let Some(n) = data.lines.iter().position(|l| l.commit.as_deref() == Some(hash)) else { continue };
                let line = &data.lines[n];
                let chars: Vec<char> = line.text.chars().collect();
                let age = page.commit(hash).map(|c| format!("{} · {}", c.author, c.relative_date)).unwrap_or_default();
                let room = width.saturating_sub(age.chars().count() + 2);
                for (k, (start, span)) in line.spans.iter().enumerate() {
                    let end = line.spans.get(k + 1).map(|s| s.0).unwrap_or(chars.len()).min(room);
                    if *start >= end {
                        continue;
                    }
                    let text: String = chars[*start..end].iter().collect();
                    let role = match span {
                        GraphSpan::Rails => Role::Muted,
                        GraphSpan::Node | GraphSpan::Hash => Role::Accent,
                        GraphSpan::Refs => Role::Good,
                        GraphSpan::Subject => {
                            if data.on_head.contains(hash) || matches!(page.scope, Scope::File(_) | Scope::Lines { .. }) {
                                Role::Text
                            } else {
                                Role::Muted
                            }
                        }
                        GraphSpan::Meta => Role::Muted,
                    };
                    g.put(y, left + start, &text, role);
                }
                g.put(y, (left + width).saturating_sub(age.chars().count()), &age, Role::Muted);
            }
            Row::File { hash, path } => {
                let letter = match page.open.get(hash) {
                    Some(Detail::Files(files)) => files.iter().find(|f| &f.1 == path).map(|f| f.0).unwrap_or('M'),
                    _ => 'M',
                };
                let role = match letter {
                    'A' => Role::Good,
                    'D' => Role::Bad,
                    _ => Role::Warn,
                };
                let x = g.put(y, left + 6, &letter.to_string(), role) + 2;
                g.put(y, x, &fit(path, (left + width).saturating_sub(x)), Role::Text);
            }
            Row::Hunk { hash, path, hunk } => {
                if let Some(h) = page.diff(hash, path).and_then(|d| d.hunks.get(*hunk)) {
                    g.put(y, left + 8, &fit(&h.header, width.saturating_sub(8)), Role::Accent);
                }
            }
            Row::Line { hash, path, hunk, line } => {
                if let Some(l) = page.diff(hash, path).and_then(|d| d.hunks.get(*hunk)).and_then(|h| h.lines.get(*line)) {
                    let number = l.new_line.or(l.old_line).map(|n| n.to_string()).unwrap_or_default();
                    g.put(y, left + 8 + 5usize.saturating_sub(number.len()), &number, Role::Muted);
                    let role = match l.kind {
                        LineKind::Added => Role::Good,
                        LineKind::Removed => Role::Bad,
                        _ => Role::Text,
                    };
                    let text = l.raw().trim_end_matches('\r').replace('\t', "    ");
                    g.put(y, left + 15, &fit(&text, width.saturating_sub(15)), role);
                }
            }
        }
        if let Row::Commit(hash) = row {
            if page.open.get(hash) == Some(&Detail::Loading) {
                y += 1;
                g.put(y, left + 6, "reading its files…", Role::Muted);
            }
        }
        if let Row::File { hash, path } = row {
            if let Some(DiffState::Loading) = page.diffs.get(&(hash.clone(), path.clone())) {
                y += 1;
                g.put(y, left + 8, "reading the diff…", Role::Muted);
            }
        }
        if i == page.cursor {
            g.focus(line_y, left..left + width);
            let menu = page.menu.as_ref().map(|(kind, typed, hash)| (page.menu(*kind, hash), typed.clone()));
            if let Some((end, cols)) = draw_popups(&mut g, y + 1, left, width, menu.as_ref().map(|(m, t)| (m, t.as_str())), page.input.as_ref(), page.confirm.as_ref()) {
                g.panels.push((line_y, left..left + width));
                g.focus(end, cols);
                y = end;
            }
        }
        y += 1;
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
        let rows = fenix_git::assign_lanes(&commits);
        let panel = crate::graph_view::render_graph(&commits, &rows, crate::graph_view::GraphStyle::Ascii);
        let lines = panel.text.lines().zip(panel.lines).map(|(text, line)| {
            let line = line.unwrap();
            GraphLine { text: text.to_string(), spans: line.spans, commit: line.commit }
        });
        LogData {
            head: commits.first().map(|c| c.hash.clone()),
            on_head: on_head.iter().map(|n| format!("{n:0>40}")).collect(),
            branch: "main".into(),
            lines: lines.collect(),
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

    #[test]
    fn the_commit_menu_offers_what_fits_the_commit() {
        let mut p = page();
        p.key(Key::Char('j'));
        p.key(Key::Enter);
        let text = layout(&p, 120).text;
        assert!(text.contains("fixup into this") && text.contains("revert") && !text.contains("cherry-pick"), "on this branch: no cherry-pick\n{text}");
        assert!(!text.contains("reword"), "only the last commit can be reworded");
        assert_eq!(p.key(Key::Char('v')), LogAction::Git(Action::Run(Job::Revert(hash(2)))));
        // A commit from elsewhere can be cherry-picked, not fixed up.
        let mut p = page();
        p.data.as_mut().unwrap().on_head.remove(&hash(2));
        p.key(Key::Char('j'));
        p.key(Key::Enter);
        let text = layout(&p, 120).text;
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
