//! The tests page (`SPC T T`, plan `docs/plans/test-explorer.md` §7.2 to
//! §7.5): a project's tests as its test providers found them, their
//! states, the run going and the last one's end, and the focused test's
//! details -- beside the tree when the pane is wide (90 columns or more),
//! under it when it's narrow. This lays the page out and maps keys to
//! actions; running, listing and opening are the host's
//! (`app/tests_host/page.rs`), the tree and its states the model's
//! (`fenix_tests`).
//!
//! The page is laid out to its pane's height, as the Extensions page
//! is: only the tree's rows in view are written, the details stay where
//! they are while the tree scrolls, and the rows themselves come from
//! the model's per-group counts (`Model::rows`), worked out again only
//! when the model's revision, the tab, the filter or what's open changes
//! (§7.5). Every label and message a provider gave is cleaned of escape
//! codes and line breaks before it's drawn (§19).

use std::collections::HashSet;
use std::path::PathBuf;

use fenix_tests::{Counts, Item, Model, NodeId, Row, RowQuery, State, Summary, Tab, TestNode};

use crate::page::{fit, str_cells, Chip, Filter, Grid, Key, Page, Role};

/// From this many columns the details are beside the tree, not under it.
pub const WIDE: usize = 90;

/// `text` with its terminal escape codes taken out (§6.4 "Lines"): CSI
/// sequences (colours, `ESC [ ... m`) and OSC ones (`ESC ] ... BEL`),
/// and any other two-character escape.
pub(crate) fn without_escapes(text: &str) -> std::borrow::Cow<'_, str> {
    if !text.contains(['\u{1b}', '\u{9b}']) {
        return std::borrow::Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        let csi = match c {
            '\u{9b}' => true,
            '\u{1b}' => match chars.peek() {
                Some('[') => {
                    chars.next();
                    true
                }
                Some(']') => {
                    chars.next();
                    // To BEL, or ESC `\`.
                    while let Some(c) = chars.next() {
                        if c == '\u{7}' || (c == '\u{1b}' && chars.next_if_eq(&'\\').is_some()) {
                            break;
                        }
                    }
                    continue;
                }
                Some(_) => {
                    chars.next();
                    continue;
                }
                None => continue,
            },
            c => {
                out.push(c);
                continue;
            }
        };
        if csi {
            for c in chars.by_ref() {
                if ('\u{40}'..='\u{7e}').contains(&c) {
                    break;
                }
            }
        }
    }
    std::borrow::Cow::Owned(out)
}

/// What a provider said, as one row: escape codes out, every run of
/// whitespace and control characters one space (§19, "cleaning labels
/// and messages ... before the page draws them").
pub fn clean(text: &str) -> String {
    crate::page::one_line(&without_escapes(text))
}

/// A line of a failure's text or of what a test printed, as the details
/// show it: escape codes out, a tab four spaces, other control
/// characters gone, its indentation kept.
fn clean_line(line: &str) -> String {
    let line = without_escapes(line);
    let mut out = String::with_capacity(line.len());
    for c in line.chars() {
        match c {
            '\t' => out.push_str("    "),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

/// `line` cut into pieces of at most `width` cells, its spaces kept.
fn chunks(line: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut out = Vec::new();
    let mut piece = String::new();
    let mut used = 0;
    for c in line.chars() {
        let w = crate::page::char_cells(c);
        if used + w > width && !piece.is_empty() {
            out.push(std::mem::take(&mut piece));
            used = 0;
        }
        piece.push(c);
        used += w;
    }
    if !piece.is_empty() || out.is_empty() {
        out.push(piece);
    }
    out
}

/// A duration as the page says it: `0.4 ms`, `12 ms`, `4.1 s`, `2 min 5 s`.
pub fn duration(ms: f64) -> String {
    if ms < 10.0 {
        format!("{ms:.1} ms")
    } else if ms < 1000.0 {
        format!("{ms:.0} ms")
    } else if ms < 60_000.0 {
        format!("{:.1} s", ms / 1000.0)
    } else {
        let s = (ms / 1000.0).round() as u64;
        format!("{} min {} s", s / 60, s % 60)
    }
}

/// `1,204`.
fn grouped(n: u32) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// A state's colour on the page; stale marks are dimmed (§4 "States").
pub fn state_role(state: State, stale: bool) -> Role {
    if stale {
        return Role::Muted;
    }
    match state {
        State::Passed => Role::Good,
        State::Failed | State::Errored => Role::Bad,
        State::Running => Role::Accent,
        State::Queued | State::Unknown | State::Skipped => Role::Muted,
    }
}

fn state_word(state: State) -> &'static str {
    match state {
        State::Unknown => "not run",
        State::Queued => "queued",
        State::Running => "running",
        State::Passed => "passed",
        State::Failed => "failed",
        State::Skipped => "skipped",
        State::Errored => "errored",
    }
}

/// How groups start (§7.2, "Groups start closed except those holding a
/// failure"), and what `zM` and `zR` make of them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Folding {
    /// Closed, but a group holding a failure, a provider's row and a
    /// provider's one top node.
    #[default]
    Failures,
    AllOpen,
    AllClosed,
}

/// What the rows were worked out against: when any of it changes, they're
/// worked out again; otherwise not, however often the page is drawn.
#[derive(Clone, Debug, PartialEq, Eq)]
struct RowsKey {
    revision: u64,
    tab: Tab,
    filter: String,
    folds: u64,
}

/// The page's own state: what it shows and where you are on it. Kept for
/// the session per project when it's closed (§7.6), so `SPC T T` reopens
/// it on the tab, filter and open groups it had.
pub struct TestsPage {
    /// The project, as the host keys it.
    pub root: PathBuf,
    pub name: String,
    pub tab: Tab,
    pub filter: Filter,
    folding: Folding,
    opened: HashSet<Item>,
    closed: HashSet<Item>,
    /// Bumped whenever a row is opened or closed.
    folds: u64,
    /// The focused row: kept by what it is, so it stays on its test as
    /// rows come and go.
    pub focus: Option<Item>,
    cursor: usize,
    /// The first of the tree's rows in view.
    top: usize,
    /// How far the details are scrolled (`J`, `K`).
    details_top: usize,
    /// `g` or `z` pressed, waiting for its second key.
    pending: Option<char>,
    rows: Vec<Row>,
    rows_key: Option<RowsKey>,
    /// How many tree rows the last layout had room for.
    visible: usize,
}

/// What a key asks of the host.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    None,
    Close,
    /// `r`: the focused test, case, file, group or provider.
    Run(Item),
    RunAll,
    RunFailed,
    RunLast,
    Stop,
    /// `o`: the run's output, beside the page.
    Output,
    /// `u`: list the project's tests again.
    List,
    /// `Enter` on a test: its line.
    GoTo(NodeId),
    /// `y` on a failed test: its name, place and message.
    Copy(NodeId),
    /// `d` on a test or case (§9, step 6).
    Debug(NodeId),
    /// Something to say on the modeline.
    Say(String),
}

impl TestsPage {
    pub fn new(root: PathBuf, name: String) -> Self {
        TestsPage {
            root,
            name,
            tab: Tab::All,
            filter: Filter::default(),
            folding: Folding::Failures,
            opened: HashSet::new(),
            closed: HashSet::new(),
            folds: 0,
            focus: None,
            cursor: 0,
            top: 0,
            details_top: 0,
            pending: None,
            rows: Vec::new(),
            rows_key: None,
            visible: 20,
        }
    }

    /// The filter has the keyboard: every key is the page's.
    pub fn typing(&self) -> bool {
        self.filter.typing
    }

    pub fn paste(&mut self, text: &str) {
        self.filter.paste(text);
    }

    /// The rows as last worked out.
    #[cfg(test)]
    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    /// Whether `item` is open: as you left it, else as it starts.
    pub fn is_open(&self, model: &Model, item: Item) -> bool {
        match self.folding {
            Folding::AllOpen => !self.closed.contains(&item),
            Folding::AllClosed => self.opened.contains(&item),
            Folding::Failures => self.opened.contains(&item) || (!self.closed.contains(&item) && starts_open(model, item)),
        }
    }

    fn set_open(&mut self, item: Item, open: bool) {
        if open {
            self.closed.remove(&item);
            self.opened.insert(item);
        } else {
            self.opened.remove(&item);
            self.closed.insert(item);
        }
        self.folds += 1;
    }

    /// The rows worked out again, if what they depend on changed, and the
    /// focus kept on what it was on.
    pub fn refresh(&mut self, model: &Model) {
        let key = RowsKey { revision: model.revision(), tab: self.tab, filter: self.filter.text.clone(), folds: self.folds };
        if self.rows_key.as_ref() != Some(&key) {
            let rows = {
                let open = |item: Item| self.is_open(model, item);
                model.rows(&RowQuery { tab: self.tab, filter: &self.filter.text, open: &open })
            };
            self.rows = rows;
            self.rows_key = Some(key);
        }
        let found = self.focus.and_then(|f| self.rows.iter().position(|r| r.item == f));
        match found {
            Some(at) => self.cursor = at,
            None => {
                self.cursor = self.cursor.min(self.rows.len().saturating_sub(1));
                self.focus = self.rows.get(self.cursor).map(|r| r.item);
            }
        }
        self.scroll();
    }

    /// The tree scrolled so the focused row is in view.
    fn scroll(&mut self) {
        let visible = self.visible.max(1);
        if self.cursor < self.top {
            self.top = self.cursor;
        } else if self.cursor >= self.top + visible {
            self.top = self.cursor + 1 - visible;
        }
        self.top = self.top.min(self.rows.len().saturating_sub(visible));
    }

    fn move_to(&mut self, at: usize) {
        if self.rows.is_empty() {
            return;
        }
        let at = at.min(self.rows.len() - 1);
        if at != self.cursor {
            self.details_top = 0;
        }
        self.cursor = at;
        self.focus = Some(self.rows[at].item);
        self.scroll();
    }

    fn focused_row(&self) -> Option<Row> {
        self.rows.get(self.cursor).copied().filter(|r| Some(r.item) == self.focus)
    }

    fn focused_node(&self) -> Option<NodeId> {
        match self.focus {
            Some(Item::Node(id)) => Some(id),
            _ => None,
        }
    }

    /// Every group above `id` opened, so its row shows.
    fn reveal(&mut self, model: &Model, id: NodeId) {
        let mut at = model.node(id).and_then(|n| n.parent());
        while let Some(up) = at {
            self.closed.remove(&Item::Node(up));
            self.opened.insert(Item::Node(up));
            at = model.node(up).and_then(|n| n.parent());
        }
        if let Some(node) = model.node(id) {
            self.closed.remove(&Item::Provider(node.provider()));
            self.opened.insert(Item::Provider(node.provider()));
        }
        self.folds += 1;
    }

    /// `node`'s row focused, its groups opened (*Show in Tests*, §9.1); on
    /// a tab that doesn't hold it, or hidden by the filter, the page goes
    /// back to All with no filter.
    pub fn show(&mut self, model: &Model, node: NodeId) {
        self.reveal(model, node);
        self.focus = Some(Item::Node(node));
        self.refresh(model);
        if !self.rows.iter().any(|r| r.item == Item::Node(node)) {
            self.tab = Tab::All;
            self.filter = Filter::default();
            self.focus = Some(Item::Node(node));
            self.refresh(model);
        }
        self.details_top = 0;
    }

    /// The row above `item`'s: its parent's, or its provider's.
    fn parent_of(&self, model: &Model, item: Item) -> Option<usize> {
        let Item::Node(id) = item else { return None };
        let node = model.node(id)?;
        let up = match node.parent() {
            Some(p) => Item::Node(p),
            None => Item::Provider(node.provider()),
        };
        self.rows.iter().position(|r| r.item == up)
    }

    /// A key: what the host's to do. `model` is the project's, when it has
    /// one.
    pub fn key(&mut self, key: Key, model: Option<&Model>) -> Action {
        if self.filter.typing {
            if self.filter.key(key) {
                self.cursor = 0;
                self.focus = None;
            }
            if let Some(model) = model {
                self.refresh(model);
            }
            return Action::None;
        }
        let Some(model) = model else {
            return match key {
                Key::Char('q') => Action::Close,
                _ => Action::None,
            };
        };
        self.refresh(model);
        let pending = self.pending.take();
        let action = match (pending, key) {
            (Some('g'), Key::Char('g')) => {
                self.move_to(0);
                Action::None
            }
            (Some('z'), Key::Char('a')) => self.toggle(),
            (Some('z'), Key::Char('M')) => {
                self.folding = Folding::AllClosed;
                self.opened.clear();
                self.closed.clear();
                self.folds += 1;
                Action::None
            }
            (Some('z'), Key::Char('R')) => {
                self.folding = Folding::AllOpen;
                self.opened.clear();
                self.closed.clear();
                self.folds += 1;
                Action::None
            }
            (None, Key::Char(c @ ('g' | 'z'))) => {
                self.pending = Some(c);
                Action::None
            }
            (_, Key::Char('q')) => Action::Close,
            (_, Key::Char('j') | Key::Down) => {
                self.move_to(self.cursor + 1);
                Action::None
            }
            (_, Key::Char('k') | Key::Up) => {
                self.move_to(self.cursor.saturating_sub(1));
                Action::None
            }
            (_, Key::Char('G')) => {
                self.move_to(self.rows.len().saturating_sub(1));
                Action::None
            }
            (_, Key::Char('l') | Key::Right) => match self.focused_row() {
                Some(row) if row.can_open && !row.open => {
                    self.set_open(row.item, true);
                    Action::None
                }
                Some(row) if row.open => {
                    self.move_to(self.cursor + 1);
                    Action::None
                }
                _ => Action::None,
            },
            (_, Key::Char('h') | Key::Left) => match self.focused_row() {
                Some(row) if row.open => {
                    self.set_open(row.item, false);
                    Action::None
                }
                Some(row) => {
                    if let Some(at) = self.parent_of(model, row.item) {
                        self.move_to(at);
                    }
                    Action::None
                }
                None => Action::None,
            },
            (_, Key::Tab) => self.toggle(),
            (_, Key::Enter) => match self.focus {
                Some(Item::Node(id)) if model.node(id).is_some_and(|n| n.found().kind.is_test()) => Action::GoTo(id),
                Some(_) => self.toggle(),
                None => Action::None,
            },
            (_, Key::Char('r')) => self.focus.map_or(Action::None, Action::Run),
            (_, Key::Char('R')) => Action::RunAll,
            (_, Key::Char('f')) => Action::RunFailed,
            (_, Key::Char('.')) => Action::RunLast,
            (_, Key::Char('x')) => Action::Stop,
            (_, Key::Char('o')) => Action::Output,
            (_, Key::Char('u')) => Action::List,
            (_, Key::Char('d')) => match self.focused_node().filter(|&id| model.node(id).is_some_and(|n| n.found().kind.is_test())) {
                Some(id) => Action::Debug(id),
                None => Action::Say("d debugs a test or a case".into()),
            },
            (_, Key::Char('y')) => match self.focused_node().filter(|&id| model.node(id).is_some_and(|n| matches!(n.state(), State::Failed | State::Errored) && n.found().kind.is_test())) {
                Some(id) => Action::Copy(id),
                None => Action::Say("y copies a failure: this one didn't fail".into()),
            },
            (_, Key::Char(c @ ('n' | 'N'))) => self.next_failure(model, c == 'n'),
            (_, Key::Char('/')) => {
                self.filter.start();
                Action::None
            }
            (_, Key::Char(c @ '1'..='3')) => {
                self.tab = [Tab::All, Tab::Failed, Tab::NotRun][c as usize - '1' as usize];
                self.cursor = 0;
                self.focus = None;
                self.top = 0;
                Action::None
            }
            (_, Key::Char('J')) => {
                self.details_top += 1;
                Action::None
            }
            (_, Key::Char('K')) => {
                self.details_top = self.details_top.saturating_sub(1);
                Action::None
            }
            (_, Key::Escape) if !self.filter.is_empty() => {
                self.filter.clear();
                Action::None
            }
            _ => Action::None,
        };
        self.refresh(model);
        action
    }

    /// `Tab`, `za`: the focused group folded or opened.
    fn toggle(&mut self) -> Action {
        if let Some(row) = self.focused_row().filter(|r| r.can_open) {
            self.set_open(row.item, !row.open);
        }
        Action::None
    }

    /// `n` / `N`: the next or previous failed or errored test, its groups
    /// opened. Worked out on the key, never per frame.
    fn next_failure(&mut self, model: &Model, forward: bool) -> Action {
        let failures = model.failures();
        if failures.is_empty() {
            return Action::Say("nothing failed".into());
        }
        let at = self.focused_node().and_then(|id| failures.iter().position(|&f| f == id));
        let id = match (at, forward) {
            (Some(i), true) => failures[(i + 1) % failures.len()],
            (Some(i), false) => failures[(i + failures.len() - 1) % failures.len()],
            // Not on a failure: the first among the rows after it (before
            // it, for `N`), else the first (the last) of all.
            (None, _) => {
                let set: HashSet<NodeId> = failures.iter().copied().collect();
                let is_failure = |r: &Row| matches!(r.item, Item::Node(id) if set.contains(&id));
                let near = if forward { self.rows.iter().skip(self.cursor + 1).find(|r| is_failure(r)) } else { self.rows.iter().take(self.cursor).rev().find(|r| is_failure(r)) };
                match near.map(|r| r.item) {
                    Some(Item::Node(id)) => id,
                    _ if forward => failures[0],
                    _ => failures[failures.len() - 1],
                }
            }
        };
        if self.tab == Tab::NotRun {
            self.tab = Tab::All;
        }
        self.reveal(model, id);
        self.focus = Some(Item::Node(id));
        self.details_top = 0;
        Action::None
    }
}

/// Whether `item` starts open: a provider's row, a provider's only top
/// node (a one-target project shows its tests), a group holding a
/// failure.
fn starts_open(model: &Model, item: Item) -> bool {
    match item {
        Item::Provider(_) => true,
        Item::Node(id) => model.node(id).is_some_and(|n| {
            let c = n.counts();
            c.failed + c.errored > 0 || (n.parent().is_none() && model.roots(n.provider()).len() == 1)
        }),
    }
}

/// What the host gives the layout beside the page's own state.
pub struct View<'a> {
    pub model: &'a Model,
    /// The head, as counted at the model's last revision shown (§10.1).
    pub summary: &'a Summary,
    /// What else the head says: a project that isn't trusted, a first
    /// scan that stopped at its cap, a provider that can't run here.
    pub notes: Vec<(String, Role)>,
    /// `14:02`, from ms since 1970.
    pub clock: &'a dyn Fn(u64) -> String,
}

/// A row's mark and what follows it, for a group: its failures and
/// errors (and what runs), else its count (§7.2 "The tree").
fn group_detail(counts: &Counts, state: State, stale: bool) -> Vec<(String, Role)> {
    let mut out = Vec::new();
    if counts.running > 0 {
        out.push((format!("● {}", grouped(counts.running)), Role::Accent));
    }
    if counts.failed > 0 {
        out.push((format!("✗ {}", grouped(counts.failed)), state_role(State::Failed, false)));
    }
    if counts.errored > 0 {
        out.push((format!("! {}", grouped(counts.errored)), state_role(State::Errored, false)));
    }
    if out.is_empty() && counts.tests() > 0 {
        out.push((format!("{} {}", state.glyph(), grouped(counts.tests())), state_role(state, stale)));
    }
    out
}

/// A test's mark and what follows it: a skip's reason, an error's why,
/// its time.
fn test_detail(node: &TestNode<'_>) -> (String, Option<String>, Role) {
    let (state, stale) = (node.state(), node.stale());
    let glyph = state.glyph().to_string();
    let extra = match (state, node.result()) {
        (State::Skipped, Some(r)) => r.message.as_deref().map(clean),
        (State::Skipped, None) => node.found().skip.as_deref().filter(|s| !s.is_empty()).map(clean),
        (State::Errored, Some(r)) => r.message.as_deref().map(clean),
        (State::Passed | State::Failed, Some(r)) => r.duration_ms.map(duration),
        _ => None,
    };
    (glyph, extra, state_role(state, stale))
}

/// The head's counts: ✓ passed, ✗ failed, ! errored, ○ skipped, ? not
/// run, those there are.
fn count_chips(counts: &Counts) -> Vec<(String, Role)> {
    let mut out = Vec::new();
    for (glyph, n, role) in [("✓", counts.passed, Role::Good), ("✗", counts.failed, Role::Bad), ("!", counts.errored, Role::Bad), ("○", counts.skipped, Role::Muted), ("?", counts.unknown, Role::Muted), ("·", counts.queued, Role::Muted), ("●", counts.running, Role::Accent)] {
        if n > 0 {
            out.push((format!("{glyph} {}", grouped(n)), role));
        }
    }
    out
}

/// What the head says on its right: the run going, or how the last ended.
fn head_right(view: &View<'_>) -> Option<(String, Role)> {
    let s = view.summary;
    if let Some(run) = &s.running {
        if run.stopping {
            return Some(("stopping…".into(), Role::Warn));
        }
        return Some(("x stops it".into(), Role::Muted));
    }
    let last = s.last.as_ref()?;
    let end = &last.end;
    if let Some(broken) = &end.first_broken {
        return Some((format!("{} · o shows it", clean(broken)), Role::Bad));
    }
    let at = (view.clock)(end.started_ms);
    let took = duration(end.ended_ms.saturating_sub(end.started_ms) as f64);
    let verb = match end.kind {
        fenix_tests::RunKind::Listing { .. } => "listed",
        fenix_tests::RunKind::Build { .. } => "built",
        fenix_tests::RunKind::Tests(_) => "ran",
    };
    match &end.halted {
        Some(fenix_tests::Halt::Stopped(why)) => Some((format!("stopped {at}: {}", clean(why)), Role::Warn)),
        Some(fenix_tests::Halt::TimedOut) => Some((format!("timed out {at}, after {took}"), Role::Bad)),
        None => {
            let mut text = format!("{verb} {at} for {took}");
            match s.saved_since {
                0 => {}
                1 => text.push_str(" · 1 file saved since"),
                n => text.push_str(&format!(" · {n} files saved since")),
            }
            Some((text, Role::Muted))
        }
    }
}

/// The page for a pane `cols` wide and `height` rows high.
pub fn layout(page: &mut TestsPage, view: &View<'_>, cols: usize, height: Option<usize>) -> Page {
    let model = view.model;
    let height = height.unwrap_or(30).max(10);
    let left = 1;
    let width = cols.saturating_sub(2).max(24);
    let wide = cols >= WIDE;
    let mut g = Grid::new();

    // The head.
    let mut x = g.put(0, left, "Tests", Role::Title);
    x = g.put(0, x + 2, &fit(&clean(&page.name), 24.min(width / 4)), Role::Text);
    let mut x = (x + 3).max(left + 19).min(left + width);
    let summary = view.summary;
    let right = head_right(view);
    let room_end = left + width;
    let mut pieces: Vec<(String, Role)> = Vec::new();
    match &summary.running {
        Some(run) if run.tests => {
            pieces.push((format!("running {} of {}", grouped(run.done), grouped(run.total)), Role::Accent));
            if run.failed > 0 {
                pieces.push((format!("✗ {}", grouped(run.failed)), Role::Bad));
            }
            if let Some(n) = summary.waiting {
                pieces.push((format!("then: {} test{}", grouped(n), if n == 1 { "" } else { "s" }), Role::Muted));
            }
        }
        Some(_) => pieces.push(("listing…".into(), Role::Accent)),
        None => {
            pieces = count_chips(&summary.counts);
            if pieces.is_empty() && model.providers().is_empty() {
                pieces.push(("no test provider here".into(), Role::Muted));
            } else if pieces.is_empty() {
                pieces.push(("no tests found yet".into(), Role::Muted));
            }
        }
    }
    let running = summary.running.is_some();
    for (i, (text, role)) in pieces.iter().enumerate() {
        let sep = if i == 0 { "" } else if running { " · " } else { "  " };
        if x + str_cells(sep) + str_cells(text) > room_end {
            break;
        }
        x = g.put(0, x, sep, Role::Muted);
        x = g.put(0, x, text, *role);
    }
    if let Some((text, role)) = &right {
        let room = (left + width).saturating_sub(x + 2);
        let shown = fit(text, room);
        if !shown.is_empty() {
            g.put(0, left + width - str_cells(&shown), &shown, *role);
        }
    }
    // What else it says, a line each: unsaved files, a target that didn't
    // build while the run goes, the host's notes.
    let mut notes: Vec<(String, Role)> = Vec::new();
    if let Some(run) = &summary.running {
        if let Some(broken) = &run.first_broken {
            notes.push((format!("{} · o shows it", clean(broken)), Role::Bad));
        }
        match run.unsaved.as_slice() {
            [] => {}
            [one] => notes.push((format!("{} has unsaved edits: the runner reads it as last saved", clean(one)), Role::Warn)),
            [one, rest @ ..] => notes.push((format!("{} and {} more have unsaved edits: the runner reads them as last saved", clean(one), rest.len()), Role::Warn)),
        }
    }
    notes.extend(view.notes.iter().cloned());
    let mut line = 1;
    for (text, role) in notes.iter().take(2) {
        g.put(line, left, &fit(text, width), *role);
        line += 1;
    }
    g.rule(line, left..left + width);
    line += 1;

    // The tabs and the filter.
    let counts = &summary.counts;
    let tabs = [(Tab::All, "All", counts.tests()), (Tab::Failed, "Failed", counts.failed + counts.errored), (Tab::NotRun, "Not run", counts.unknown + counts.skipped)];
    let mut tx = left;
    for (tab, name, n) in tabs {
        let text = format!("{name} {}", grouped(n));
        tx = if tab == page.tab { g.chip(line, tx, &format!(" {text} "), Role::Accent, Chip::Toggle(true)) } else { g.put(line, tx + 1, &text, Role::Muted) + 1 } + 1;
    }
    let room = (left + width).saturating_sub(tx + 1);
    if room >= 8 {
        let (filter, role) = page.filter.line("to filter", room);
        let fx = if page.filter.typing || !page.filter.is_empty() { tx + 1 } else { (left + width).saturating_sub(str_cells(&filter)) };
        g.put(line, fx, &filter, role);
        if page.filter.typing {
            g.panel(line, fx..left + width);
        }
    }
    line += 1;

    // The tree, and the details beside or under it.
    let foot = height.saturating_sub(3);
    let tree_top = line;
    let (tree_w, details_at, tree_end) = if wide {
        let tree_w = (width * 53 / 100).max(40);
        (tree_w, (tree_top, left + tree_w + 2), foot)
    } else {
        let lines = (height / 4).clamp(3, 8);
        let end = foot.saturating_sub(lines + 1).max(tree_top + 1);
        (width, (end + 1, left), end)
    };
    page.visible = tree_end.saturating_sub(tree_top).max(1);
    page.refresh(model);
    let rows = page.rows.clone();
    let tree_right = left + tree_w;
    let mark_x = tree_right.saturating_sub(16);
    if rows.is_empty() {
        let hint = if model.providers().is_empty() {
            "No test provider serves this project: a language pack with tests brings one (SPC h x)."
        } else if !page.filter.is_empty() {
            "Nothing matches the filter."
        } else {
            match page.tab {
                Tab::Failed => "Nothing failed.",
                Tab::NotRun => "Everything here has run.",
                Tab::All => "No tests found yet: u lists them with the project's runner.",
            }
        };
        for (i, piece) in crate::page::wrap(hint, tree_w).into_iter().enumerate().take(tree_end.saturating_sub(tree_top)) {
            g.put(tree_top + i, left, &piece, Role::Muted);
        }
    }
    for (i, row) in rows.iter().enumerate().skip(page.top).take(tree_end.saturating_sub(tree_top)) {
        let y = tree_top + i - page.top;
        let focused = i == page.cursor && page.focus == Some(row.item);
        let x = left + (row.depth as usize * 2).min(tree_w / 2);
        let chevron = if row.can_open { if row.open { "▾ " } else { "▸ " } } else if focused { "› " } else { "  " };
        let x = g.put(y, x, chevron, if focused && !row.can_open { Role::Accent } else { Role::Muted });
        let (label, label_role, detail): (String, Role, Vec<(String, Role)>) = match row.item {
            Item::Provider(p) => match model.provider(p) {
                Some(view) => {
                    let detail = match view.problem {
                        Some(problem) => vec![(fit(&clean(problem), 20), Role::Warn)],
                        None => group_detail(view.counts, view.counts.state(None), view.counts.all_stale()),
                    };
                    (clean(view.name), Role::Title, detail)
                }
                None => continue,
            },
            Item::Node(id) => match model.node(id) {
                Some(node) => {
                    let label = clean(&node.found().label);
                    let role = if node.stale() || node.found_in_run() { Role::Muted } else { Role::Text };
                    if node.is_counted() {
                        let (glyph, extra, role_m) = test_detail(&node);
                        let mut d = vec![(glyph, role_m)];
                        if let Some(extra) = extra {
                            d.push((extra, Role::Muted));
                        }
                        (label, role, d)
                    } else {
                        (label, role, group_detail(node.counts(), node.state(), node.stale()))
                    }
                }
                None => continue,
            },
        };
        let label_room = mark_x.saturating_sub(x + 1);
        g.put(y, x, &fit(&label, label_room), label_role);
        // A test's mark and its reason a space apart; a group's counts two.
        let gap = if matches!(row.item, Item::Node(id) if model.node(id).is_some_and(|n| n.is_counted())) { 1 } else { 2 };
        let mut dx = mark_x.max(x + 1);
        for (j, (text, role)) in detail.iter().enumerate() {
            let sep = if j == 0 { 0 } else { gap };
            let room = tree_right.saturating_sub(dx + sep);
            if room == 0 {
                break;
            }
            let shown = fit(text, room);
            dx = g.put(y, dx + sep, &shown, *role);
        }
        if focused {
            g.focus(y, left..tree_right);
        }
    }

    // The details.
    let (dy, dx) = details_at;
    let (details_w, details_end) = if wide { ((left + width).saturating_sub(dx), foot) } else {
        g.rule(dy - 1, left..left + width);
        (width, foot)
    };
    details(&mut g, page, view, dy, dx, details_w, details_end);
    g.reserve(height.saturating_sub(3));
    let keys: &[(&str, &str)] = if page.filter.typing {
        &[("Enter", "keep it"), ("Esc", "clear it")]
    } else if wide {
        &[("r", "run"), ("R", "all"), ("f", "the failed"), (".", "again"), ("Enter", "go to it"), ("o", "output"), ("/", "filter"), ("x", "stop"), ("u", "list"), ("q", "close")]
    } else {
        &[("r", "run"), ("R", "all"), ("f", "failed"), (".", "again"), ("Enter", "go"), ("o", "output"), ("/", "filter"), ("x", "stop")]
    };
    g.keys(left, width, keys);
    let mut out = g.finish();
    if wide {
        out.vertical_rules.push((tree_right + 1, tree_top..foot));
    }
    out
}

/// The focused row's details (§7.2 "The details"): its name and state,
/// its place, provider and when it ran; its message; then its failure's
/// text and what it printed, from `details_top` (`J`, `K`).
fn details(g: &mut Grid, page: &mut TestsPage, view: &View<'_>, top: usize, x: usize, width: usize, end: usize) {
    let model = view.model;
    if width < 8 || top >= end {
        return;
    }
    let mut y = top;
    let mut body: Vec<(String, Role)> = Vec::new();
    match page.focus {
        Some(Item::Provider(p)) => {
            let Some(provider) = model.provider(p) else { return };
            g.put(y, x, &fit(&clean(provider.name), width), Role::Title);
            y += 1;
            let counts: Vec<String> = count_chips(provider.counts).into_iter().map(|(t, _)| t).collect();
            g.put(y, x, &fit(&format!("{} · {} tests", counts.join("  "), grouped(provider.counts.tests())), width), Role::Muted);
            y += 1;
            if let Some(problem) = provider.problem {
                body.push((format!("It can't run tests here: {} -- SPC p h for the doctor", clean(problem)), Role::Warn));
            }
        }
        Some(Item::Node(id)) => {
            let Some(node) = model.node(id) else { return };
            let found = node.found();
            let state = node.state();
            let stale = node.stale();
            let mark = format!("{} {}", state.glyph(), state_word(state));
            let title_room = width.saturating_sub(str_cells(&mark) + 2);
            g.put(y, x, &fit(&clean(&found.label), title_room), Role::Title);
            g.put(y, x + width - str_cells(&mark), &mark, state_role(state, stale));
            y += 1;
            let mut place: Vec<String> = Vec::new();
            match (&found.path, found.line) {
                (Some(path), Some(line)) => place.push(format!("{}:{}", clean(path), line + 1)),
                (Some(path), None) => place.push(clean(path)),
                (None, _) => {}
            }
            if let Some(provider) = model.provider(node.provider()) {
                place.push(clean(provider.name));
            }
            if let Some(result) = node.result() {
                let when = (view.clock)(result.when_ms);
                let word = match result.outcome {
                    fenix_tests::Outcome::Passed => "passed",
                    fenix_tests::Outcome::Failed => "failed",
                    fenix_tests::Outcome::Skipped => "skipped",
                    fenix_tests::Outcome::Errored => "errored",
                };
                let mut text = if matches!(state, State::Queued | State::Running) || stale { format!("last {word} {when}") } else { format!("ran {when}") };
                if stale {
                    text.push_str(" (stale)");
                }
                place.push(text);
            }
            g.put(y, x, &fit(&place.join(" · "), width), Role::Muted);
            y += 1;
            if node.found_in_run() {
                body.push(("Found in a run: it has no line until a scan or a listing gives it one.".into(), Role::Muted));
            }
            if !node.can_be_named() {
                body.push(("Its id was cut and isn't kept whole: it runs as the group it's in.".into(), Role::Muted));
            }
            if !node.is_counted() {
                let c = node.counts();
                let counts: Vec<String> = count_chips(c).into_iter().map(|(t, _)| t).collect();
                body.push((format!("{} test{}: {}", grouped(c.tests()), if c.tests() == 1 { "" } else { "s" }, counts.join("  ")), Role::Text));
            }
            if let Some(result) = node.result() {
                if let Some(message) = result.message.as_deref().filter(|m| !m.trim().is_empty()) {
                    body.push((String::new(), Role::Text));
                    for l in message.lines() {
                        body.push((clean_line(l), if result.outcome == fenix_tests::Outcome::Failed || result.outcome == fenix_tests::Outcome::Errored { Role::Bad } else { Role::Text }));
                    }
                }
                if let Some(at) = &result.at {
                    body.push((format!("at {}:{}", clean(&at.path), at.line + 1), Role::Muted));
                }
                if let Some(text) = result.text.as_deref().filter(|t| !t.trim().is_empty()) {
                    body.push((String::new(), Role::Text));
                    for l in text.lines() {
                        body.push((clean_line(l), Role::Text));
                    }
                }
                if !result.printed.trim().is_empty() {
                    body.push((String::new(), Role::Text));
                    body.push(("What it printed:".into(), Role::Muted));
                    for l in result.printed.lines() {
                        body.push((clean_line(l), Role::Text));
                    }
                }
            } else if let Some(skip) = found.skip.as_deref() {
                body.push((if skip.is_empty() { "Skipped where it's written.".into() } else { format!("Skipped where it's written: {}", clean(skip)) }, Role::Muted));
            }
        }
        None => return,
    }
    while body.first().is_some_and(|(t, _)| t.is_empty()) {
        body.remove(0);
    }
    // A line between the head and the body, when there's room for both.
    if y + 3 < end {
        y += 1;
    }
    let room = end.saturating_sub(y);
    // The body's lines, each cut to the width, from `details_top`; no more
    // worked out than are shown.
    let mut shown: Vec<(String, Role)> = Vec::new();
    let mut skipped = 0;
    let mut more = false;
    for (text, role) in &body {
        for piece in chunks(text, width) {
            if skipped < page.details_top {
                skipped += 1;
                continue;
            }
            if shown.len() >= room {
                more = true;
                break;
            }
            shown.push((piece, *role));
        }
        if more {
            break;
        }
    }
    // Scrolled past the end: back to the last screenful.
    if shown.is_empty() && page.details_top > 0 {
        page.details_top = skipped.saturating_sub(room.max(1)).min(page.details_top - 1);
    }
    for (i, (text, role)) in shown.iter().enumerate() {
        if more && i + 1 == room && room >= 3 {
            g.put(y + i, x, &fit("… J shows more, o has all of it", width), Role::Muted);
            break;
        }
        if !text.is_empty() {
            g.put(y + i, x, text, *role);
        }
    }
}

#[cfg(test)]
mod tests;
