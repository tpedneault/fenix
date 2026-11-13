//! The tree: nodes keyed by `(provider, id)`, what each of their three
//! sources last said, the fields merged from them (§4, "A `found` for
//! an id Fenix has"), and what's dropped when (§4, "What's dropped").
//! Results and states live on the nodes; runs change them (`run.rs`).

use crate::limits::{self, MAX_DEPTH, TESTS_PER_PROVIDER};
use crate::records::{Found, Kind, Location, Outcome};
use crate::run::{Run, RunIds, Selection};
use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::OnceLock;

/// A test provider as the model keys it: the extension whose manifest
/// declares it, and its id there (`[[contributes.test_providers]]`,
/// §5). Two extensions may both declare `rust` (`fenix.rust`,
/// `acme.rust`): they're two providers, whose trees and results never
/// meet. Sorted by its id, then its extension's: the page's order, and
/// the tie-break of §4 "Two providers in one file".
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ProviderId {
    /// The extension's id: `fenix.rust`.
    pub extension: Cow<'static, str>,
    /// The provider's id in its manifest: `rust`, `rust.nextest`.
    pub provider: Cow<'static, str>,
}

impl ProviderId {
    pub fn new(extension: impl Into<String>, provider: impl Into<String>) -> Self {
        ProviderId { extension: Cow::Owned(extension.into()), provider: Cow::Owned(provider.into()) }
    }

    /// One whose names are fixed in the code.
    pub const fn fixed(extension: &'static str, provider: &'static str) -> Self {
        ProviderId { extension: Cow::Borrowed(extension), provider: Cow::Borrowed(provider) }
    }
}

impl Ord for ProviderId {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (&self.provider, &self.extension).cmp(&(&other.provider, &other.extension))
    }
}

impl PartialOrd for ProviderId {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl std::fmt::Display for ProviderId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.provider, self.extension)
    }
}

/// A node as the outside names it: its provider and its own id.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TestKey {
    pub provider: ProviderId,
    pub id: String,
}

impl TestKey {
    pub fn new(provider: &ProviderId, id: impl Into<String>) -> Self {
        TestKey { provider: provider.clone(), id: id.into() }
    }
}

/// A node, as long as it's in the model: a handle that stops naming
/// anything once its node is dropped, never another node.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId {
    index: u32,
    generation: u32,
}

/// A provider of the model, by its place: stable for the model's life.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ProviderRef(pub(crate) u32);

/// A node's state (§4, "States"). Stale is a flag beside it, not one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum State {
    Unknown,
    Queued,
    Running,
    Passed,
    Failed,
    Skipped,
    Errored,
}

impl State {
    /// The gutter's and the page's glyph (§4's table).
    pub fn glyph(self) -> char {
        match self {
            State::Unknown => '?',
            State::Queued => '·',
            State::Running => '●',
            State::Passed => '✓',
            State::Failed => '✗',
            State::Skipped => '○',
            State::Errored => '!',
        }
    }

    /// Worst first: which of several tests on one line gives its mark
    /// (§4, "Two providers in one file"; Decisions, 8).
    pub(crate) fn badness(self) -> u8 {
        match self {
            State::Failed => 6,
            State::Errored => 5,
            State::Running => 4,
            State::Queued => 3,
            State::Unknown => 2,
            State::Skipped => 1,
            State::Passed => 0,
        }
    }
}

impl From<Outcome> for State {
    fn from(outcome: Outcome) -> State {
        match outcome {
            Outcome::Passed => State::Passed,
            Outcome::Failed => State::Failed,
            Outcome::Skipped => State::Skipped,
            Outcome::Errored => State::Errored,
        }
    }
}

/// How many tests under a node are in each state, kept as results land
/// (§4, "A group's state"), never summed per frame. Only a test or case
/// with no test or case under it counts (Decisions, 7).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub unknown: u32,
    pub queued: u32,
    pub running: u32,
    pub passed: u32,
    pub failed: u32,
    pub skipped: u32,
    pub errored: u32,
    /// Of those with a result shown, how many are stale.
    pub stale: u32,
}

impl Counts {
    /// Every test counted.
    pub fn tests(&self) -> u32 {
        self.unknown + self.queued + self.running + self.passed + self.failed + self.skipped + self.errored
    }

    pub fn get(&self, state: State) -> u32 {
        match state {
            State::Unknown => self.unknown,
            State::Queued => self.queued,
            State::Running => self.running,
            State::Passed => self.passed,
            State::Failed => self.failed,
            State::Skipped => self.skipped,
            State::Errored => self.errored,
        }
    }

    fn slot(&mut self, state: State) -> &mut u32 {
        match state {
            State::Unknown => &mut self.unknown,
            State::Queued => &mut self.queued,
            State::Running => &mut self.running,
            State::Passed => &mut self.passed,
            State::Failed => &mut self.failed,
            State::Skipped => &mut self.skipped,
            State::Errored => &mut self.errored,
        }
    }

    pub(crate) fn add(&mut self, look: Look) {
        *self.slot(look.state) += 1;
        if look.stale {
            self.stale += 1;
        }
    }

    pub(crate) fn sub(&mut self, look: Look) {
        let slot = self.slot(look.state);
        *slot = slot.saturating_sub(1);
        if look.stale {
            self.stale = self.stale.saturating_sub(1);
        }
    }

    pub(crate) fn plus(&mut self, other: &Counts) {
        self.unknown += other.unknown;
        self.queued += other.queued;
        self.running += other.running;
        self.passed += other.passed;
        self.failed += other.failed;
        self.skipped += other.skipped;
        self.errored += other.errored;
        self.stale += other.stale;
    }

    /// A group's state from what's under it (§4): failed or errored if
    /// any is, running if any runs, queued if any waits, passed if all
    /// that ran passed, else unknown. `own` is a test's own state, for a
    /// template that has a result of its own beside its cases'.
    pub fn state(&self, own: Option<State>) -> State {
        let has = |s: State| self.get(s) > 0 || own == Some(s);
        [State::Failed, State::Errored, State::Running, State::Queued, State::Passed].into_iter().find(|&s| has(s)).unwrap_or(State::Unknown)
    }

    /// Whether every result shown under it is stale (and there's one).
    pub fn all_stale(&self) -> bool {
        let shown = self.passed + self.failed + self.skipped + self.errored;
        self.stale > 0 && self.stale == shown && self.queued + self.running == 0
    }
}

/// A test's state and staleness as counted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Look {
    pub state: State,
    pub stale: bool,
}

/// A test's last result (§4, "A result"; what's kept, "What's kept
/// across a restart").
#[derive(Clone, Debug, PartialEq)]
pub struct TestResult {
    pub outcome: Outcome,
    pub duration_ms: Option<f64>,
    /// When its step started, in ms since 1970, as the host gave it.
    pub when_ms: u64,
    pub message: Option<String>,
    pub at: Option<Location>,
    /// Its failure's text, its end kept (64 KiB; 4 KiB once restored).
    pub text: Option<String>,
    /// What it printed in its run, its end kept. Not kept across a restart.
    pub printed: String,
    /// It came from the results file: from before Fenix started, so stale.
    pub restored: bool,
    /// The model's own clock: what it's stale against (Decisions, 25).
    pub(crate) seq: u64,
}

/// What a run has done to a test that hasn't ended in it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Live {
    #[default]
    Idle,
    Queued,
    Running,
}

/// Which of a node's three sources a `found` came from (§4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Source {
    Scan,
    Listing,
    Run,
}

pub(crate) struct ScanRecord {
    file: String,
    seq: u64,
    found: Found,
}

pub(crate) struct Node {
    pub provider: u32,
    pub id: String,
    /// A scan's record per file that gave it: one, but for a group
    /// several files' scans give (§4, "What's dropped").
    scans: Vec<ScanRecord>,
    pub listing: Option<Found>,
    pub run: Option<Found>,
    /// What the views and asks use: the sources merged field by field.
    pub merged: Found,
    /// Its label and id, lower-cased, for the filter.
    pub search: String,
    /// When it was first seen: the order of nodes that say nothing else.
    seq: u64,
    // What `build` works out.
    pub parent: Option<NodeId>,
    pub children: Vec<NodeId>,
    pub depth: u16,
    /// A test or case with no test or case under it: what's counted.
    pub leaf: bool,
    /// Its file: its own path, else its parent's unless that's a group.
    pub file: Option<String>,
    /// Its place in display order.
    pub order: u32,
    pub counts: Counts,
    pub result: Option<TestResult>,
    pub live: Live,
    /// The file it's indexed under in its provider's `containers`: a
    /// group, file or suite with a path.
    indexed: Option<String>,
}

impl Node {
    fn latest_scan(&self) -> Option<&Found> {
        self.scans.iter().max_by_key(|s| s.seq).map(|s| &s.found)
    }

    fn data_bytes(&self) -> usize {
        let len = |f: Option<&Found>| f.and_then(|f| f.data.as_ref()).map_or(0, String::len);
        self.scans.iter().map(|s| len(Some(&s.found))).sum::<usize>() + len(self.listing.as_ref()) + len(self.run.as_ref())
    }

    pub fn has_scan(&self) -> bool {
        !self.scans.is_empty()
    }

    pub fn has_sources(&self) -> bool {
        !self.scans.is_empty() || self.listing.is_some() || self.run.is_some()
    }
}

struct Slot {
    generation: u32,
    node: Option<Node>,
}

/// What a provider is in this project (§6.3 `test-provider-set`).
pub(crate) struct Provider {
    pub id: ProviderId,
    pub name: String,
    /// The runner it said it uses; none until it says.
    pub runner: Option<String>,
    pub problem: Option<String>,
    /// Removed from the model: its nodes are gone.
    pub gone: bool,
    pub ids: HashMap<String, NodeId>,
    /// The ids each file's last scan gave.
    pub scanned: HashMap<String, HashSet<String>>,
    pub data_bytes: usize,
    pub roots: Vec<NodeId>,
    /// Its groups, files and suites by their own path: where an unknown
    /// id's `ended` goes is looked for here (Decisions, 21), kept as the
    /// merged fields change, never by a walk of every id.
    pub containers: HashMap<String, Vec<NodeId>>,
    pub counts: Counts,
    /// The limits already said on its output page.
    pub said: HashSet<&'static str>,
}

impl Provider {
    /// "Nothing to run in this project": `runner` "" with no problem
    /// (§6.3). It isn't shown or asked.
    pub fn hidden(&self) -> bool {
        self.gone || (self.runner.as_deref() == Some("") && self.problem.is_none())
    }
}

/// Something to say on a provider's output page: a limit it went over,
/// a report naming an id it never found (§4, §6.5).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    pub provider: ProviderId,
    pub text: String,
}

/// One project's tests: what its providers found, their results, and
/// its runs. Sans-I/O: the host tells it what providers and runs said,
/// and asks it what to show and what to run next.
pub struct Model {
    slots: Vec<Slot>,
    free: Vec<u32>,
    pub(crate) providers: Vec<Provider>,
    /// The model's clock: a number that goes up with everything that
    /// happens (Decisions, 25).
    pub(crate) seq: u64,
    /// Each file's last save, on the model's clock.
    pub(crate) saved: HashMap<String, u64>,
    pub(crate) unsaved: BTreeSet<String>,
    /// Per file, the visible nodes whose own path it is and that have a
    /// line, by line: the gutter's and `test_at`'s index.
    pub(crate) files: BTreeMap<String, Vec<NodeId>>,
    dirty: bool,
    revision: u64,
    notices: Vec<Notice>,
    /// Results read from the results file for nodes not found yet.
    pub(crate) restored: HashMap<TestKey, TestResult>,
    pub(crate) run: Option<Run>,
    pub(crate) waiting: Option<Vec<Selection>>,
    pub(crate) waiting_build: Option<(u32, String, crate::records::Plan, crate::run::RunId)>,
    pub(crate) listings: Vec<u32>,
    /// Where run ids come from: the host's, shared by every project's
    /// model, so they're unique while Fenix runs (§6.4).
    pub(crate) run_ids: RunIds,
    /// How many times the tree's shape was worked out: what a step's
    /// scope is cached against.
    pub(crate) built: u64,
    /// The waiting run's count of tests for the head, worked out once a
    /// revision, not once a frame.
    pub(crate) waiting_count: OnceLock<u32>,
    pub(crate) timeout_minutes: u32,
    pub(crate) auto_listing: bool,
    pub(crate) page_opened: bool,
    pub(crate) last: Option<crate::view::LastRun>,
    pub(crate) last_selection: Option<Vec<Selection>>,
}

impl Default for Model {
    fn default() -> Self {
        Self::new()
    }
}

impl Model {
    /// A model with run ids of its own, unique only within it: for one
    /// project, or a test. The host gives every project's model the same
    /// `RunIds` (`with_run_ids`).
    pub fn new() -> Self {
        Self::with_run_ids(RunIds::new())
    }

    /// A model whose run ids come from `run_ids`, which the host shares
    /// between every project's model so that a run id is unique while
    /// Fenix runs, across projects (§6.4).
    pub fn with_run_ids(run_ids: RunIds) -> Self {
        Model {
            slots: Vec::new(),
            free: Vec::new(),
            providers: Vec::new(),
            seq: 0,
            saved: HashMap::new(),
            unsaved: BTreeSet::new(),
            files: BTreeMap::new(),
            dirty: false,
            revision: 0,
            notices: Vec::new(),
            restored: HashMap::new(),
            run: None,
            waiting: None,
            waiting_build: None,
            listings: Vec::new(),
            run_ids,
            built: 0,
            waiting_count: OnceLock::new(),
            timeout_minutes: 0,
            auto_listing: true,
            page_opened: false,
            last: None,
            last_selection: None,
        }
    }

    /// Goes up whenever anything a view shows changed: a view laid out
    /// at one revision is right until the next.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// What to say on providers' output pages since last asked.
    pub fn take_notices(&mut self) -> Vec<Notice> {
        std::mem::take(&mut self.notices)
    }

    pub(crate) fn tick(&mut self) -> u64 {
        self.seq += 1;
        self.seq
    }

    /// Says `text` on provider `p`'s output page once for the model's life.
    pub(crate) fn say_once(&mut self, p: u32, text: &'static str) {
        let provider = &mut self.providers[p as usize];
        if provider.said.insert(text) {
            self.notices.push(Notice { provider: provider.id.clone(), text: text.to_string() });
        }
    }

    /// Says `text` on provider `p`'s output page, each time.
    pub(crate) fn say(&mut self, p: u32, text: String) {
        let provider = self.providers[p as usize].id.clone();
        self.notices.push(Notice { provider, text });
    }

    pub(crate) fn bound_found(&mut self, p: u32, found: &mut Found) {
        let mut said = Vec::new();
        limits::bound_found(found, &mut |what| said.push(what));
        for what in said {
            self.say_once(p, what);
        }
    }

    // Providers.

    /// Adds provider `id` (`[[contributes.test_providers]]`), shown as
    /// `name` until it says what runner it uses. Adding one that's there
    /// renames it.
    pub fn add_provider(&mut self, id: &ProviderId, name: &str) -> ProviderRef {
        let p = self.provider_index(id);
        let provider = &mut self.providers[p as usize];
        provider.name = limits::cut_chars(name, limits::LABEL_CHARS).to_string();
        if provider.gone {
            provider.gone = false;
            provider.runner = None;
            provider.problem = None;
        }
        self.changed();
        ProviderRef(p)
    }

    pub(crate) fn provider_index(&mut self, id: &ProviderId) -> u32 {
        if let Some(p) = self.providers.iter().position(|p| p.id == *id) {
            return p as u32;
        }
        self.providers.push(Provider {
            id: id.clone(),
            name: id.provider.to_string(),
            runner: None,
            problem: None,
            gone: false,
            ids: HashMap::new(),
            scanned: HashMap::new(),
            data_bytes: 0,
            roots: Vec::new(),
            containers: HashMap::new(),
            counts: Counts::default(),
            said: HashSet::new(),
        });
        (self.providers.len() - 1) as u32
    }

    pub(crate) fn find_provider(&self, id: &ProviderId) -> Option<u32> {
        self.providers.iter().position(|p| p.id == *id && !p.gone).map(|p| p as u32)
    }

    /// `test-provider-set` (§6.3, F8): what provider `id` is in this
    /// project now. True when it changed what the model had; then, once
    /// the page has opened in the session, a listing of it is asked for
    /// as an automatic one (§7.1). `runner` "" with no `problem` hides
    /// the provider: no row, never asked, no marks.
    /// Kept while the model lives: an extension that stops and starts
    /// again (a trap, a move to another project and back) keeps what it
    /// said here; only `remove_provider` forgets it.
    pub fn provider_set(&mut self, id: &ProviderId, runner: &str, problem: Option<&str>) -> bool {
        let p = self.provider_index(id);
        let runner = limits::cut_chars(runner, limits::LABEL_CHARS).to_string();
        let problem = problem.map(|s| limits::cut_start(s, limits::MESSAGE_BYTES).to_string());
        let provider = &mut self.providers[p as usize];
        if provider.runner.as_deref() == Some(runner.as_str()) && provider.problem == problem {
            return false;
        }
        let was_hidden = provider.hidden();
        provider.runner = Some(runner);
        provider.problem = problem;
        if provider.hidden() != was_hidden {
            self.dirty = true;
        }
        if self.page_opened && !self.providers[p as usize].hidden() {
            self.list_provider(p, true);
        }
        self.changed();
        true
    }

    /// Removes provider `id` and everything it found: its extension was
    /// uninstalled or turned off.
    pub fn remove_provider(&mut self, id: &ProviderId) {
        let Some(p) = self.find_provider(id) else { return };
        let nodes: Vec<NodeId> = self.providers[p as usize].ids.values().copied().collect();
        for node in nodes {
            self.free_node(node);
        }
        self.providers[p as usize].containers.clear();
        let provider = &mut self.providers[p as usize];
        provider.gone = true;
        provider.ids.clear();
        provider.scanned.clear();
        provider.data_bytes = 0;
        self.listings.retain(|&l| l != p);
        self.dirty = true;
        self.changed();
    }

    // Nodes.

    pub(crate) fn raw(&self, id: NodeId) -> Option<&Node> {
        let slot = self.slots.get(id.index as usize)?;
        if slot.generation != id.generation {
            return None;
        }
        slot.node.as_ref()
    }

    pub(crate) fn raw_mut(&mut self, id: NodeId) -> Option<&mut Node> {
        let slot = self.slots.get_mut(id.index as usize)?;
        if slot.generation != id.generation {
            return None;
        }
        slot.node.as_mut()
    }

    pub(crate) fn n(&self, id: NodeId) -> &Node {
        self.raw(id).expect("a node the model holds")
    }

    pub(crate) fn nodes(&self) -> impl Iterator<Item = (NodeId, &Node)> {
        self.slots.iter().enumerate().filter_map(|(index, slot)| slot.node.as_ref().map(|node| (NodeId { index: index as u32, generation: slot.generation }, node)))
    }

    pub(crate) fn lookup(&self, p: u32, id: &str) -> Option<NodeId> {
        self.providers.get(p as usize)?.ids.get(&*limits::cut_id(id)).copied()
    }

    /// The node `key` names, its id cut as every id is.
    pub fn find(&self, key: &TestKey) -> Option<NodeId> {
        let p = self.find_provider(&key.provider)?;
        self.lookup(p, &key.id)
    }

    fn alloc(&mut self, node: Node) -> NodeId {
        if let Some(index) = self.free.pop() {
            let slot = &mut self.slots[index as usize];
            slot.node = Some(node);
            NodeId { index, generation: slot.generation }
        } else {
            self.slots.push(Slot { generation: 0, node: Some(node) });
            NodeId { index: (self.slots.len() - 1) as u32, generation: 0 }
        }
    }

    fn free_node(&mut self, id: NodeId) {
        self.unindex(id);
        let slot = &mut self.slots[id.index as usize];
        if slot.generation == id.generation && slot.node.take().is_some() {
            slot.generation = slot.generation.wrapping_add(1);
            self.free.push(id.index);
        }
    }

    /// Puts what `source` says of `found` into its node, adding the node
    /// when it's new. `file` is the scanned file for a scan's record.
    /// None when the provider has no room for another test (§6.5).
    pub(crate) fn set_source(&mut self, p: u32, mut found: Found, source: Source, file: Option<&str>) -> Option<NodeId> {
        self.bound_found(p, &mut found);
        let seq = self.tick();
        let id = match self.providers[p as usize].ids.get(&found.id).copied() {
            Some(id) => id,
            None => {
                if self.providers[p as usize].ids.len() >= TESTS_PER_PROVIDER {
                    self.say_once(p, "past 100,000 tests, what it finds isn't kept");
                    return None;
                }
                let key = TestKey::new(&self.providers[p as usize].id, found.id.clone());
                let result = self.restored.remove(&key);
                let node = Node {
                    provider: p,
                    id: found.id.clone(),
                    scans: Vec::new(),
                    listing: None,
                    run: None,
                    merged: found.clone(),
                    search: String::new(),
                    seq,
                    parent: None,
                    children: Vec::new(),
                    depth: 0,
                    leaf: false,
                    file: None,
                    order: 0,
                    counts: Counts::default(),
                    result,
                    live: Live::Idle,
                    indexed: None,
                };
                let id = self.alloc(node);
                self.providers[p as usize].ids.insert(found.id.clone(), id);
                self.dirty = true;
                id
            }
        };
        // The provider's data budget (§6.5): past it a node's isn't kept.
        let before = self.n(id).data_bytes();
        let incoming = found.data.as_ref().map_or(0, String::len);
        let replaced = {
            let node = self.n(id);
            match source {
                Source::Scan => node.scans.iter().find(|s| Some(s.file.as_str()) == file).and_then(|s| s.found.data.as_ref()).map_or(0, String::len),
                Source::Listing => node.listing.as_ref().and_then(|f| f.data.as_ref()).map_or(0, String::len),
                Source::Run => node.run.as_ref().and_then(|f| f.data.as_ref()).map_or(0, String::len),
            }
        };
        let budget = self.providers[p as usize].data_bytes.saturating_sub(replaced);
        if incoming > 0 && budget + incoming > limits::DATA_PER_PROVIDER {
            found.data = None;
            self.say_once(p, "past 16 MiB of tests' data, a test's data isn't kept");
        }
        let node = self.raw_mut(id).expect("just found");
        match source {
            Source::Scan => {
                let file = file.unwrap_or_default();
                node.scans.retain(|s| s.file != file);
                node.scans.push(ScanRecord { file: file.to_string(), seq, found });
            }
            Source::Listing => node.listing = Some(found),
            Source::Run => node.run = Some(found),
        }
        let after = node.data_bytes();
        let provider = &mut self.providers[p as usize];
        provider.data_bytes = provider.data_bytes.saturating_sub(before) + after;
        self.remerge(id);
        Some(id)
    }

    /// Takes `source`'s record (for a scan, `file`'s) from node `id`,
    /// dropping the node when no source has it any more.
    pub(crate) fn drop_source(&mut self, id: NodeId, source: Source, file: Option<&str>) {
        let Some(node) = self.raw_mut(id) else { return };
        let before = node.data_bytes();
        match source {
            Source::Scan => node.scans.retain(|s| Some(s.file.as_str()) != file),
            Source::Listing => node.listing = None,
            Source::Run => node.run = None,
        }
        let after = node.data_bytes();
        let p = node.provider;
        let keep = node.has_sources();
        let provider = &mut self.providers[p as usize];
        provider.data_bytes = provider.data_bytes.saturating_sub(before) + after;
        if keep {
            self.remerge(id);
        } else {
            // Its result waits for it, as one read at start does, shown
            // stale when it's found again (Decisions, 29): a test cut and
            // pasted back, or a scan of half-typed text, doesn't lose it.
            let node = self.raw_mut(id).expect("held");
            let key = node.id.clone();
            if let Some(mut result) = node.result.take() {
                result.restored = true;
                let provider = self.providers[p as usize].id.clone();
                self.restored.insert(TestKey { provider, id: key.clone() }, result);
            }
            self.providers[p as usize].ids.remove(&key);
            self.free_node(id);
            self.dirty = true;
        }
    }

    /// Works a node's fields out again from its sources (§4): field by
    /// field the scan's, else the listing's, else the run's; its place
    /// (`path`, `line`, `end_line`) as one, from the first that gives a
    /// path or a line (Decisions, 30); `label`, `kind`, `parent` and
    /// `template` from the first that has the node; `skip` the scan's as
    /// it is wherever a scan has the node.
    fn remerge(&mut self, id: NodeId) {
        let node = self.n(id);
        let scan = node.latest_scan();
        let sources = [scan, node.listing.as_ref(), node.run.as_ref()];
        let Some(first) = sources.iter().flatten().next() else { return };
        let pick = |get: fn(&Found) -> Option<&String>| sources.iter().flatten().find_map(|f| get(f)).cloned();
        let place = sources.iter().flatten().find(|f| f.path.is_some() || f.line.is_some());
        let merged = Found {
            id: node.id.clone(),
            parent: first.parent.clone(),
            label: first.label.clone(),
            kind: first.kind,
            path: place.and_then(|f| f.path.clone()),
            line: place.and_then(|f| f.line),
            end_line: place.and_then(|f| f.end_line),
            skip: match scan {
                Some(scan) => scan.skip.clone(),
                None => pick(|f| f.skip.as_ref()),
            },
            template: first.template,
            data: pick(|f| f.data.as_ref()),
        };
        let shape_changed = {
            let old = &node.merged;
            old.parent != merged.parent || old.kind != merged.kind || old.path != merged.path || old.line != merged.line || old.end_line != merged.end_line
        };
        let search = format!("{}\n{}", merged.label.to_lowercase(), merged.id.to_lowercase());
        let node = self.raw_mut(id).expect("held");
        node.merged = merged;
        node.search = search;
        if shape_changed {
            self.dirty = true;
        }
        self.reindex(id);
    }

    /// Puts node `id` in its provider's `containers` under its path, or
    /// takes it out, as its merged fields now say.
    fn reindex(&mut self, id: NodeId) {
        let node = self.n(id);
        let want = (!node.merged.kind.is_test()).then(|| node.merged.path.clone()).flatten();
        if want == node.indexed {
            return;
        }
        self.unindex(id);
        if let Some(path) = want {
            let p = self.n(id).provider as usize;
            self.providers[p].containers.entry(path.clone()).or_default().push(id);
            self.raw_mut(id).expect("held").indexed = Some(path);
        }
    }

    fn unindex(&mut self, id: NodeId) {
        let Some(node) = self.raw_mut(id) else { return };
        let Some(path) = node.indexed.take() else { return };
        let p = node.provider as usize;
        if let Some(list) = self.providers[p].containers.get_mut(&path) {
            list.retain(|&n| n != id);
            if list.is_empty() {
                self.providers[p].containers.remove(&path);
            }
        }
    }

    // Scans.

    /// `tests-scan`'s answer for file `path` (the host's mapping of it,
    /// Decisions, 1): replaces that file's last scan (§6.4, "Scans").
    /// What the last scan gave and this one doesn't loses this file's
    /// record; a group another file's scan still gives stays (§4).
    pub fn scan(&mut self, provider: &ProviderId, path: &str, found: Vec<Found>) {
        let p = self.provider_index(provider);
        self.scan_one(p, path, found);
        self.changed();
    }

    /// Several files' scans at once, as `scan` each: a project's first
    /// scan (§7.1) given a batch at a time works the tree out once a
    /// batch, not once a file.
    pub fn scan_files(&mut self, provider: &ProviderId, files: Vec<(String, Vec<Found>)>) {
        let p = self.provider_index(provider);
        for (path, found) in files {
            self.scan_one(p, &path, found);
        }
        self.changed();
    }

    fn scan_one(&mut self, p: u32, path: &str, found: Vec<Found>) {
        let mut found = found;
        if found.len() > limits::SCAN_FOUND {
            found.truncate(limits::SCAN_FOUND);
            self.say_once(p, "a scan's answer past 10,000 tests was left out");
        }
        let mut given = HashSet::new();
        for mut f in found {
            if f.path.is_none() && f.kind != Kind::Group {
                f.path = Some(path.to_string());
            }
            if let Some(id) = self.set_source(p, f, Source::Scan, Some(path)) {
                given.insert(self.n(id).id.clone());
            }
        }
        let before = self.providers[p as usize].scanned.insert(path.to_string(), given.clone()).unwrap_or_default();
        for gone in before.difference(&given) {
            if let Some(id) = self.lookup(p, gone) {
                self.drop_source(id, Source::Scan, Some(path));
            }
        }
    }

    /// Forgets a file's scan: it was deleted, or left the scanned set.
    pub fn forget_file(&mut self, provider: &ProviderId, path: &str) {
        self.scan(provider, path, Vec::new());
        if let Some(p) = self.find_provider(provider) {
            self.providers[p as usize].scanned.remove(path);
        }
    }

    /// File `path` was saved (§8.4): the tests in it are stale, and a
    /// listing that began before keeps their records (§4). Its scan
    /// comes as `scan`.
    pub fn file_saved(&mut self, path: &str) {
        let seq = self.tick();
        self.saved.insert(path.to_string(), seq);
        self.unsaved.remove(path);
        self.dirty = true;
        self.changed();
    }

    /// Whether file `path` has edits not yet saved, as the host knows
    /// (§8.4, "Unsaved files"). A run takes the files of its tests that
    /// have them when it starts.
    pub fn set_unsaved(&mut self, path: &str, unsaved: bool) {
        if unsaved {
            self.unsaved.insert(path.to_string());
        } else {
            self.unsaved.remove(path);
        }
    }

    // States.

    pub(crate) fn is_stale(&self, node: &Node, result: &TestResult) -> bool {
        result.restored || node.file.as_ref().and_then(|f| self.saved.get(f)).is_some_and(|&saved| saved > result.seq)
    }

    pub(crate) fn look(&self, node: &Node) -> Look {
        match node.live {
            Live::Queued => Look { state: State::Queued, stale: false },
            Live::Running => Look { state: State::Running, stale: false },
            Live::Idle => match &node.result {
                Some(result) => Look { state: result.outcome.into(), stale: self.is_stale(node, result) },
                None => Look { state: State::Unknown, stale: false },
            },
        }
    }

    /// Changes node `id` by `change`, keeping its ancestors' counts: a
    /// result landing costs its depth, not the tree.
    pub(crate) fn update(&mut self, id: NodeId, change: impl FnOnce(&mut Node)) {
        let Some(node) = self.raw(id) else { return };
        let counted = !self.dirty && node.leaf;
        let before = self.look(node);
        change(self.raw_mut(id).expect("held"));
        self.bump();
        if !counted {
            return;
        }
        let node = self.n(id);
        let after = self.look(node);
        if before == after {
            return;
        }
        let p = node.provider;
        let mut at = Some(id);
        while let Some(id) = at {
            let node = self.raw_mut(id).expect("held");
            node.counts.sub(before);
            node.counts.add(after);
            at = node.parent;
        }
        let counts = &mut self.providers[p as usize].counts;
        counts.sub(before);
        counts.add(after);
    }

    // Structure.

    /// Marks that something a view shows changed, and works the tree out
    /// again if its shape did: once per call that changed it, never per
    /// frame.
    pub(crate) fn changed(&mut self) {
        self.bump();
        self.ensure_built();
    }

    /// A new revision: what's cached against the last one goes.
    fn bump(&mut self) {
        self.revision += 1;
        self.waiting_count = OnceLock::new();
    }

    pub(crate) fn ensure_built(&mut self) {
        if self.dirty {
            self.build();
            self.dirty = false;
        }
    }

    /// Works out parents, children in display order, depths, which nodes
    /// are counted, files, counts and the files' index from the merged
    /// fields. A parent the provider hasn't given (yet), or one that
    /// would make a loop, puts the node at its provider's top (Decisions, 6).
    fn build(&mut self) {
        self.built += 1;
        let ids: Vec<NodeId> = self.nodes().map(|(id, _)| id).collect();
        for &id in &ids {
            let node = self.n(id);
            let parent = node.merged.parent.as_deref().and_then(|parent| self.lookup(node.provider, parent)).filter(|&parent| parent != id);
            let node = self.raw_mut(id).expect("held");
            node.parent = parent;
            node.children.clear();
        }
        // Loops and depth: a node whose walk up comes back to itself, or
        // goes on past MAX_DEPTH, goes to its provider's top.
        for &id in &ids {
            let mut at = self.n(id).parent;
            let mut steps = 0;
            while let Some(up) = at {
                steps += 1;
                if up == id || steps > MAX_DEPTH {
                    self.raw_mut(id).expect("held").parent = None;
                    break;
                }
                at = self.n(up).parent;
            }
        }
        for provider in &mut self.providers {
            provider.roots.clear();
        }
        for &id in &ids {
            let node = self.n(id);
            let (p, parent) = (node.provider, node.parent);
            match parent {
                Some(parent) => self.raw_mut(parent).expect("held").children.push(id),
                None => self.providers[p as usize].roots.push(id),
            }
        }
        let order_of = |model: &Model, a: &NodeId, b: &NodeId| {
            let (a, b) = (model.n(*a), model.n(*b));
            fn key(n: &Node) -> (bool, Option<&str>, u32, u64) {
                (n.merged.path.is_none(), n.merged.path.as_deref(), n.merged.line.unwrap_or(u32::MAX), n.seq)
            }
            key(a).cmp(&key(b))
        };
        for &id in &ids {
            let mut children = std::mem::take(&mut self.raw_mut(id).expect("held").children);
            children.sort_by(|a, b| order_of(self, a, b));
            self.raw_mut(id).expect("held").children = children;
        }
        for p in 0..self.providers.len() {
            let mut roots = std::mem::take(&mut self.providers[p].roots);
            roots.sort_by(|a, b| order_of(self, a, b));
            self.providers[p].roots = roots;
        }
        // Depth, file, order and counts, top down then back up.
        let mut order = 0u32;
        let mut providers: Vec<u32> = (0..self.providers.len() as u32).collect();
        providers.sort_by(|a, b| self.providers[*a as usize].id.cmp(&self.providers[*b as usize].id));
        for p in providers {
            let roots = self.providers[p as usize].roots.clone();
            let mut total = Counts::default();
            for root in roots {
                total.plus(&self.build_node(root, 0, None, &mut order));
            }
            self.providers[p as usize].counts = total;
        }
        // The files' index: visible nodes with a path and a line.
        self.files.clear();
        for &id in &ids {
            let node = self.n(id);
            if self.providers[node.provider as usize].hidden() {
                continue;
            }
            if let (Some(path), Some(_)) = (&node.merged.path, node.merged.line) {
                self.files.entry(path.clone()).or_default().push(id);
            }
        }
        let mut files = std::mem::take(&mut self.files);
        for nodes in files.values_mut() {
            nodes.sort_by_key(|id| (self.n(*id).merged.line, self.n(*id).order));
        }
        self.files = files;
    }

    fn build_node(&mut self, id: NodeId, depth: u16, inherited: Option<&str>, order: &mut u32) -> Counts {
        let node = self.n(id);
        let file = node.merged.path.clone().or_else(|| inherited.map(str::to_string));
        let pass_on = if node.merged.kind == Kind::Group { None } else { file.clone() };
        let children = node.children.clone();
        let is_test = node.merged.kind.is_test();
        let leaf = is_test && !children.iter().any(|c| self.n(*c).merged.kind.is_test());
        {
            let node = self.raw_mut(id).expect("held");
            node.depth = depth;
            node.file = file;
            node.leaf = leaf;
            node.order = *order;
        }
        *order += 1;
        let mut counts = Counts::default();
        for child in children {
            counts.plus(&self.build_node(child, depth.saturating_add(1), pass_on.as_deref(), order));
        }
        if leaf {
            counts.add(self.look(self.n(id)));
        }
        self.raw_mut(id).expect("held").counts = counts;
        counts
    }

    /// Node `id` and its ancestors, nearest first.
    pub(crate) fn ancestry(&self, id: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        std::iter::successors(Some(id), move |&at| self.raw(at).and_then(|n| n.parent))
    }

    /// The counted tests under node `id`, itself included.
    pub(crate) fn leaves_under(&self, id: NodeId, out: &mut Vec<NodeId>) {
        let mut stack = vec![id];
        while let Some(at) = stack.pop() {
            let Some(node) = self.raw(at) else { continue };
            if node.leaf {
                out.push(at);
            }
            stack.extend(node.children.iter().copied());
        }
    }

    /// Every counted test of provider `p`.
    pub(crate) fn leaves_of(&self, p: u32) -> Vec<NodeId> {
        self.providers[p as usize].ids.values().copied().filter(|&id| self.n(id).leaf).collect()
    }
}
