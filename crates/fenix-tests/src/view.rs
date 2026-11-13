//! What the page and the file views ask (§7.2, §9): a node as shown,
//! the tree's rows in display order under a tab and a filter, the
//! gutter's marks for lines in view, the test the cursor is in, the
//! failures as problems, and the page's head. Each walks only what it
//! shows, from counts kept as results land (§7.5).

use crate::model::{Counts, Model, Node, NodeId, ProviderId, ProviderRef, State, TestKey, TestResult};
use crate::records::{Found, Location, Outcome};
use crate::run::{RunEnd, RunId};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::ops::RangeInclusive;

/// A node of the tree as the views see it.
#[derive(Clone, Copy)]
pub struct TestNode<'a> {
    model: &'a Model,
    id: NodeId,
    node: &'a Node,
}

impl<'a> TestNode<'a> {
    pub fn id(&self) -> NodeId {
        self.id
    }

    pub fn key(&self) -> TestKey {
        TestKey::new(&self.model.providers[self.node.provider as usize].id, self.node.id.clone())
    }

    pub fn provider(&self) -> ProviderRef {
        ProviderRef(self.node.provider)
    }

    /// Its fields, merged from its sources (§4): what the page shows,
    /// and what's given back to its provider.
    pub fn found(&self) -> &'a Found {
        &self.node.merged
    }

    /// It as Fenix gives it back to its provider (`tests-launch`, and in
    /// a `run` ask): `found` with its `parent` the one Fenix put it under
    /// (none at its provider's top). Its `path` is the project's form
    /// (Decisions, 1): the host gives a file of the project back as
    /// `/project/<path>`.
    pub fn given_back(&self) -> Found {
        self.model.given_back(self.id)
    }

    /// Whether its provider can be asked for it by itself: false for a
    /// node whose id was cut (over 1 KiB) and whose `data` isn't kept.
    /// Such a node is asked as its nearest ancestor that can be, and isn't
    /// debugged (Decisions, 32).
    pub fn can_be_named(&self) -> bool {
        self.model.can_be_named(self.id)
    }

    /// Its file: its own path, else its parent's unless that's a group.
    pub fn file(&self) -> Option<&'a str> {
        self.node.file.as_deref()
    }

    pub fn parent(&self) -> Option<NodeId> {
        self.node.parent
    }

    pub fn children(&self) -> &'a [NodeId] {
        &self.node.children
    }

    pub fn depth(&self) -> u16 {
        self.node.depth
    }

    /// Only a run has found it (a macro-made test, a Go subtest, a case):
    /// it has no line until a scan or a listing gives it one (§4).
    pub fn found_in_run(&self) -> bool {
        self.node.run.is_some() && self.node.listing.is_none() && !self.node.has_scan()
    }

    /// A test or case with no test or case under it: what's counted.
    pub fn is_counted(&self) -> bool {
        self.node.leaf
    }

    /// Its state: a test's own, a group's from what's under it (§4).
    pub fn state(&self) -> State {
        self.model.state_of(self.node)
    }

    /// Its mark is dimmed: its result is from before its file was saved
    /// or Fenix started (§4 "States"); a group's when all under it are.
    pub fn stale(&self) -> bool {
        self.model.stale_of(self.node)
    }

    pub fn result(&self) -> Option<&'a TestResult> {
        self.node.result.as_ref()
    }

    /// How many tests under it are in each state (itself, for a test).
    pub fn counts(&self) -> &'a Counts {
        &self.node.counts
    }
}

/// A provider, as the page's top rows and head show it.
#[derive(Clone, Copy)]
pub struct ProviderView<'a> {
    pub provider: ProviderRef,
    pub id: &'a ProviderId,
    /// The runner it said it uses, else its manifest's name.
    pub name: &'a str,
    /// Why it can't run tests here, when it said.
    pub problem: Option<&'a str>,
    pub counts: &'a Counts,
}

/// The page's tabs (§7.2): all, failed and errored, unknown and skipped.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tab {
    #[default]
    All,
    Failed,
    NotRun,
}

/// What a row of the tree is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Item {
    /// A provider's row: only when two or more are shown (§4).
    Provider(ProviderRef),
    Node(NodeId),
}

/// A row of the tree, in display order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Row {
    pub item: Item,
    pub depth: u16,
    /// It has rows under it.
    pub can_open: bool,
    /// Its rows follow: opened, or opened by the filter for a match.
    pub open: bool,
}

/// What `rows` lays out.
pub struct RowQuery<'a> {
    pub tab: Tab,
    /// Typed on the page: words matched in labels and ids, in any order,
    /// case-insensitive (§7.2 "The filter").
    pub filter: &'a str,
    /// Whether the page has a row open.
    pub open: &'a dyn Fn(Item) -> bool,
}

/// A gutter mark (§9.1): the worst state of what starts on a line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mark {
    pub line: u32,
    pub state: State,
    pub stale: bool,
    /// What gives the mark: what a click runs.
    pub node: NodeId,
}

/// A failed test's failure, as a problem (§9.3).
#[derive(Clone, Debug, PartialEq)]
pub struct Problem {
    pub node: NodeId,
    /// The test's own file and line: where the problem goes when `at`
    /// is outside the project, or none.
    pub file: String,
    pub line: u32,
    pub at: Option<Location>,
    pub message: String,
    /// Its code: the test's label.
    pub label: String,
}

/// The last run that ended, as the page's head says it (§7.2).
#[derive(Clone, Debug, PartialEq)]
pub struct LastRun {
    pub end: RunEnd,
    pub(crate) started_seq: u64,
    pub(crate) files: BTreeSet<String>,
}

/// The run under way.
#[derive(Clone, Debug, PartialEq)]
pub struct Progress {
    pub run: RunId,
    /// A run of tests (else a listing or a build).
    pub tests: bool,
    /// A debug launch's `build-first` plan (§11).
    pub build: bool,
    /// Its tests with a result, of all it has: `running 12 of 40`.
    pub done: u32,
    pub total: u32,
    pub failed: u32,
    /// The step under way's label.
    pub step: Option<String>,
    /// It's been asked to stop and is stopping.
    pub stopping: bool,
    pub first_broken: Option<String>,
    /// Its tests' files with unsaved edits (§8.4): "the runner reads
    /// them as last saved".
    pub unsaved: Vec<String>,
}

/// The page's head (§7.2) and the modeline's item (§10.1).
#[derive(Clone, Debug, PartialEq)]
pub struct Summary {
    /// Every shown provider's tests.
    pub counts: Counts,
    pub running: Option<Progress>,
    /// How many tests the run waiting holds: `then: 3 tests`.
    pub waiting: Option<u32>,
    pub last: Option<LastRun>,
    /// How many of the last run's files were saved since it started.
    pub saved_since: u32,
}

impl Model {
    pub(crate) fn state_of(&self, node: &Node) -> State {
        if node.leaf {
            return self.look(node).state;
        }
        let own = (node.merged.kind.is_test() && (node.result.is_some() || node.live != crate::model::Live::Idle)).then(|| self.look(node).state);
        node.counts.state(own)
    }

    pub(crate) fn stale_of(&self, node: &Node) -> bool {
        if node.leaf {
            self.look(node).stale
        } else {
            node.counts.all_stale()
        }
    }

    pub fn node(&self, id: NodeId) -> Option<TestNode<'_>> {
        self.raw(id).map(|node| TestNode { model: self, id, node })
    }

    /// The providers shown, in the order the page shows them (by id).
    pub fn providers(&self) -> Vec<ProviderView<'_>> {
        let mut out: Vec<ProviderView<'_>> = self
            .providers
            .iter()
            .enumerate()
            .filter(|(_, p)| !p.hidden())
            .map(|(i, p)| ProviderView { provider: ProviderRef(i as u32), id: &p.id, name: p.runner.as_deref().filter(|r| !r.is_empty()).unwrap_or(&p.name), problem: p.problem.as_deref(), counts: &p.counts })
            .collect();
        out.sort_by(|a, b| a.id.cmp(b.id));
        out
    }

    pub fn provider(&self, provider: ProviderRef) -> Option<ProviderView<'_>> {
        self.providers().into_iter().find(|p| p.provider == provider)
    }

    /// A provider's top nodes, in display order.
    pub fn roots(&self, provider: ProviderRef) -> &[NodeId] {
        self.providers.get(provider.0 as usize).map_or(&[], |p| &p.roots)
    }

    fn in_tab(&self, node: &Node, tab: Tab) -> bool {
        let counts = &node.counts;
        let state = self.state_of(node);
        match tab {
            Tab::All => true,
            Tab::Failed => matches!(state, State::Failed | State::Errored) || counts.failed + counts.errored > 0,
            Tab::NotRun => {
                if node.leaf {
                    matches!(state, State::Unknown | State::Skipped)
                } else {
                    counts.unknown + counts.skipped > 0
                }
            }
        }
    }

    /// The tree's rows in display order: only open nodes' children are
    /// laid out (§7.5). With a filter, matches show with their groups
    /// opened, and what's under a matching node shows as it's opened.
    pub fn rows(&self, query: &RowQuery<'_>) -> Vec<Row> {
        let words: Vec<String> = query.filter.split_whitespace().map(str::to_lowercase).collect();
        let mut matched: HashSet<NodeId> = HashSet::new();
        let mut below: HashSet<NodeId> = HashSet::new();
        if !words.is_empty() {
            for (id, node) in self.nodes() {
                if self.providers[node.provider as usize].hidden() || !words.iter().all(|w| node.search.contains(w.as_str())) {
                    continue;
                }
                matched.insert(id);
                let mut at = node.parent;
                while let Some(up) = at {
                    if !below.insert(up) {
                        break;
                    }
                    at = self.n(up).parent;
                }
            }
        }
        let filtering = !words.is_empty();
        let providers = self.providers();
        let with_rows = providers.len() >= 2;
        let mut rows = Vec::new();
        for provider in providers {
            let roots = self.roots(provider.provider);
            let has_match = !filtering || roots.iter().any(|r| matched.contains(r) || below.contains(r));
            if !has_match {
                continue;
            }
            let mut depth = 0;
            if with_rows {
                let tab_ok = match query.tab {
                    Tab::All => true,
                    Tab::Failed => provider.counts.failed + provider.counts.errored > 0,
                    Tab::NotRun => provider.counts.unknown + provider.counts.skipped > 0,
                };
                if !tab_ok {
                    continue;
                }
                let open = filtering || (query.open)(Item::Provider(provider.provider));
                rows.push(Row { item: Item::Provider(provider.provider), depth: 0, can_open: !roots.is_empty(), open });
                if !open {
                    continue;
                }
                depth = 1;
            }
            for &root in roots {
                self.lay_out(root, depth, false, query, &matched, &below, filtering, &mut rows);
            }
        }
        rows
    }

    #[allow(clippy::too_many_arguments)]
    fn lay_out(&self, id: NodeId, depth: u16, matched_above: bool, query: &RowQuery<'_>, matched: &HashSet<NodeId>, below: &HashSet<NodeId>, filtering: bool, rows: &mut Vec<Row>) {
        let node = self.n(id);
        if !self.in_tab(node, query.tab) {
            return;
        }
        let is_match = matched.contains(&id);
        let has_below = below.contains(&id);
        if filtering && !(is_match || has_below || matched_above) {
            return;
        }
        let shown_children = node.children.iter().any(|&c| self.in_tab(self.n(c), query.tab));
        let open = shown_children && ((filtering && has_below) || (query.open)(Item::Node(id)));
        rows.push(Row { item: Item::Node(id), depth, can_open: shown_children, open });
        if open {
            for &child in &node.children {
                self.lay_out(child, depth.saturating_add(1), matched_above || is_match, query, matched, below, filtering, rows);
            }
        }
    }

    /// Whether a provider shown found anything in file `path`: the file
    /// gets the gutter's test column (§9.1).
    pub fn serves(&self, path: &str) -> bool {
        self.files.contains_key(path)
    }

    /// The tests and groups that start in file `path`, by line (the lens,
    /// §9.5; problems).
    pub fn in_file(&self, path: &str) -> &[NodeId] {
        self.files.get(path).map_or(&[], Vec::as_slice)
    }

    /// The gutter's marks for `lines` of file `path` (§9.1): on each line
    /// where a test or group starts, the worst state of all that start
    /// there, every provider's (§4 "Two providers in one file"). Costs
    /// the lines in view, not the file.
    pub fn marks(&self, path: &str, lines: RangeInclusive<u32>) -> Vec<Mark> {
        let nodes = self.in_file(path);
        let start = nodes.partition_point(|&id| self.n(id).merged.line.unwrap_or(0) < *lines.start());
        let mut marks: Vec<Mark> = Vec::new();
        for &id in &nodes[start..] {
            let node = self.n(id);
            let line = node.merged.line.unwrap_or(0);
            if line > *lines.end() {
                break;
            }
            let mark = Mark { line, state: self.state_of(node), stale: self.stale_of(node), node: id };
            match marks.last_mut() {
                Some(last) if last.line == line => {
                    if mark.state.badness() > last.state.badness() {
                        *last = mark;
                    }
                }
                _ => marks.push(mark),
            }
        }
        marks
    }

    /// The test the cursor is in (§9.4): the innermost test whose lines
    /// hold `line`; with no `end_line`, the last that starts at or above
    /// it within its parent's lines; on a group's first line, that group.
    /// Of all providers' tests (§4); on a tie, the provider whose id
    /// sorts first. None: the line isn't in a test.
    pub fn test_at(&self, path: &str, line: u32) -> Option<NodeId> {
        let nodes = self.in_file(path);
        let starts = |id: NodeId| self.n(id).merged.line.unwrap_or(0);
        let upto = nodes.partition_point(|&id| starts(id) <= line);
        let candidates = &nodes[..upto];
        // A test with no `end_line` holds the line when no test of its
        // provider and parent starts after it and at or above the line:
        // the latest start of each such family, in one pass, not one per
        // test (§9.4, on every cursor move).
        let mut latest: HashMap<(u32, Option<NodeId>), u32> = HashMap::new();
        for &id in candidates {
            let node = self.n(id);
            if node.merged.kind.is_test() {
                let start = latest.entry((node.provider, node.parent)).or_insert(0);
                *start = (*start).max(starts(id));
            }
        }
        let parent_holds = |node: &Node| match node.parent.map(|p| self.n(p)) {
            Some(parent) if parent.merged.path == node.merged.path => match (parent.merged.line, parent.merged.end_line) {
                (Some(start), Some(end)) => start <= line && line <= end,
                _ => true,
            },
            _ => true,
        };
        let holds = |id: NodeId| {
            let node = self.n(id);
            let start = starts(id);
            if !node.merged.kind.is_test() {
                return start == line;
            }
            match node.merged.end_line {
                Some(end) => line <= end,
                None => parent_holds(node) && latest.get(&(node.provider, node.parent)).is_none_or(|&last| last <= start),
            }
        };
        candidates.iter().copied().filter(|&id| holds(id)).max_by(|&a, &b| {
            let (na, nb) = (self.n(a), self.n(b));
            let end = |n: &Node| n.merged.end_line.unwrap_or(u32::MAX);
            starts(a)
                .cmp(&starts(b))
                .then(end(nb).cmp(&end(na)))
                .then(na.depth.cmp(&nb.depth))
                .then_with(|| self.providers[nb.provider as usize].id.cmp(&self.providers[na.provider as usize].id))
        })
    }

    /// Failed tests' failures as problems (§9.3): at `at`, else the
    /// test's own line. None for a result that's stale (its file saved
    /// since, or from before a restart) or a test being run again
    /// (Decisions, 19).
    pub fn problems(&self) -> Vec<Problem> {
        let mut out = Vec::new();
        for (id, node) in self.nodes() {
            let Some(result) = &node.result else { continue };
            if result.outcome != Outcome::Failed || !node.merged.kind.is_test() || node.live != crate::model::Live::Idle || self.is_stale(node, result) || self.providers[node.provider as usize].hidden() {
                continue;
            }
            let file = node.file.clone().or_else(|| result.at.as_ref().map(|a| a.path.clone()));
            let line = node.merged.line.or(result.at.as_ref().map(|a| a.line));
            let (Some(file), Some(line)) = (file, line) else { continue };
            out.push(Problem { node: id, file, line, at: result.at.clone(), message: result.message.clone().unwrap_or_default(), label: node.merged.label.clone() });
        }
        out.sort_by_key(|p| self.n(p.node).order);
        out
    }

    /// The tests whose last result is failed or errored, in display
    /// order (`n`/`N`, the notification, `test_failures`).
    pub fn failures(&self) -> Vec<NodeId> {
        let mut out: Vec<NodeId> = self
            .nodes()
            .filter(|(_, n)| n.merged.kind.is_test() && !self.providers[n.provider as usize].hidden() && n.result.as_ref().is_some_and(|r| matches!(r.outcome, Outcome::Failed | Outcome::Errored)))
            .map(|(id, _)| id)
            .collect();
        out.sort_by_key(|&id| self.n(id).order);
        out
    }

    /// The page's head (§7.2).
    pub fn summary(&self) -> Summary {
        let mut counts = Counts::default();
        for provider in self.providers.iter().filter(|p| !p.hidden()) {
            counts.plus(&provider.counts);
        }
        let running = self.run.as_ref().map(|run| {
            let failed = run.results.values().filter(|o| matches!(o, Outcome::Failed | Outcome::Errored)).count() as u32;
            Progress {
                run: run.id,
                tests: run.is_tests(),
                build: run.is_build(),
                done: run.results.len() as u32,
                total: run.total,
                failed,
                step: run.step_label.clone(),
                stopping: run.halt.is_some(),
                first_broken: run.first_broken.clone(),
                unsaved: run.unsaved.iter().cloned().collect(),
            }
        });
        // Drawn every frame: counted once a revision (`waiting_count`).
        let waiting = self.waiting.as_ref().map(|w| *self.waiting_count.get_or_init(|| self.count_selected(w)));
        let saved_since = self.last.as_ref().map_or(0, |last| last.files.iter().filter(|f| self.saved.get(*f).is_some_and(|&s| s > last.started_seq)).count() as u32);
        Summary { counts, running, waiting, last: self.last.clone(), saved_since }
    }

    /// Whether a run of tests is going (not a listing or a build).
    pub fn running_tests(&self) -> bool {
        self.run.as_ref().is_some_and(|r| r.is_tests())
    }

    /// What the last run that ended was.
    pub fn last_run(&self) -> Option<&RunEnd> {
        self.last.as_ref().map(|l| &l.end)
    }
}
