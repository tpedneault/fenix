//! Runs (§4 "Runs", §8): the queue, one run at a time; what each
//! provider is asked; its plan's steps as they start and end; reports
//! applied to the tree; what a step covers and how what ended without a
//! result is judged; stop and timeout; listings (§7.1) and a debug
//! launch's build (§11) through the same machinery.
//!
//! The host drives it: it calls `next` whenever nothing is under way,
//! and does what the `Action` says -- ask a provider for a plan, start a
//! step, tell a provider its steps won't start, or show that a run
//! ended -- then tells the model what came of it (`planned`, `reports`,
//! `step_ended`).

use crate::limits::{self, ASK_RECORDS, MESSAGE_BYTES, PLAN_COVERS, PLAN_STEPS, REPORTS_PER_CALL, TEXT_BYTES};
use crate::model::{Live, Model, NodeId, Source, TestKey, TestResult};
use crate::records::{Ask, Ended, Found, Kind, Location, Outcome, Plan, Report, StepEnd};
use crate::view::LastRun;
use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::ops::Range;

/// A run, as the provider sees it (`run: u64`, §6.4): unique for the
/// model's life. A listing and a build have one too.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RunId(pub u64);

/// What to run (§4 "Runs", §8.4). Worked out into tests when the run
/// starts, not when it's asked for: the failed are those failed then.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Selection {
    /// Every test of every provider shown: each asked `run-all`.
    All,
    /// The tests whose last result is failed or errored, a case as itself.
    Failed,
    /// A file's tests, from every provider that has them (`SPC T f`).
    File(String),
    /// These nodes: tests, cases, files, suites, groups (all under them).
    Nodes(Vec<TestKey>),
}

/// How a request meets a run that's going (§8.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Waits for the run going; joins the one waiting, as their union.
    Queue,
    /// Takes the waiting one's place (run on save, §8.4).
    ReplaceWaiting,
    /// Stops the run going and goes next (`stopped`, "by a run that
    /// replaces it", §4).
    Replace,
}

/// What came of a request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Requested {
    /// Nothing was going: `next` starts it.
    Starts,
    /// It waits for the run going.
    Waits,
    /// It joined the run already waiting.
    Joined,
    /// The same run is going and none waits: nothing changes (§8.1, "`f`
    /// or `R` while the same run is going does nothing").
    AlreadyGoing,
    /// It stopped the run going, and goes next: the host stops the step
    /// under way as for `stop`.
    Replaces,
    /// It names no test the model has.
    Nothing,
}

/// What the host does next for the run under way (from `next`).
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    /// Ask `provider` for a plan (`tests-plan(provider, run, ask)`),
    /// then give its answer to `planned`.
    Plan { run: RunId, provider: String, ask: Ask },
    /// Start step `step` of `provider`'s plan, feed its output's reports
    /// to `reports` and its end to `step_ended`.
    Step { run: RunId, provider: String, step: u32, label: String },
    /// These steps of `provider`'s plan won't start: give it
    /// `tests-step-ended(stopped)` for each (§6.4, "Order").
    NotStarted { run: RunId, provider: String, steps: Range<u32> },
    /// The run ended.
    Ended(RunEnd),
}

/// What a run was.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunKind {
    Tests(Vec<Selection>),
    /// A provider's listing (`discover`).
    Listing { provider: String },
    /// A debug launch's `build-first` plan for a test (§11).
    Build { provider: String, test: String },
}

/// Why a run ended early.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Halt {
    /// Stopped: by you, by a run that replaced it, by its provider
    /// stopping. Its tests went back to what they were. Why.
    Stopped(String),
    /// `tests.timeout_minutes` passed: its unfinished tests errored.
    TimedOut,
}

/// How a run ended, for the page's head, the notification and
/// `last_task` (§10).
#[derive(Clone, Debug, PartialEq)]
pub struct RunEnd {
    pub run: RunId,
    pub kind: RunKind,
    pub halted: Option<Halt>,
    /// The results this run gave, the last per test.
    pub passed: u32,
    pub failed: u32,
    pub skipped: u32,
    pub errored: u32,
    /// The first `broken`'s message: "orbit (lib) didn't build: ...".
    pub first_broken: Option<String>,
    /// For a build: every step exited 0 and nothing was `broken` or
    /// `crashed`, so the launch may be asked again (§11, F19).
    pub build_ok: bool,
    /// Its tests' files that had unsaved edits when it started (§8.4).
    pub unsaved: Vec<String>,
    pub started_ms: u64,
    pub ended_ms: u64,
}

/// What a provider part covers of the selection.
#[derive(Clone, Debug)]
enum Sel {
    All,
    Nodes(Vec<NodeId>),
}

enum Job {
    Tests(Vec<Selection>),
    Listing,
    Build(String),
}

#[derive(PartialEq, Eq)]
enum Phase {
    NeedPlan,
    AwaitPlan,
    Steps,
}

/// One provider's turn in a run.
struct Part {
    provider: u32,
    sel: Sel,
    asked: HashSet<NodeId>,
    phase: Phase,
    labels: Vec<String>,
    covers: Vec<HashSet<String>>,
    /// Every id some step's `covers` names.
    named_by_steps: HashSet<String>,
    list_after: bool,
    next_step: u32,
    current: Option<u32>,
    /// A `started` came in the current step.
    started_seen: bool,
    rest_running: bool,
    all_exited: bool,
    all_zero: bool,
    planned_ok: bool,
    broken: bool,
    crashed: bool,
    /// Its provider stopped (a trap, a restart): no call goes to it.
    dead: bool,
    /// The step each test ended in.
    ended_in: HashMap<NodeId, u32>,
    /// What a report named in this run: found, started, ended, printed.
    named: HashSet<NodeId>,
    /// What a test printed before its result came.
    printed: HashMap<NodeId, String>,
}

pub(crate) struct Run {
    pub id: RunId,
    job: Job,
    pub started_ms: u64,
    step_ms: u64,
    pub started_seq: u64,
    pub halt: Option<Halt>,
    queue: VecDeque<(u32, Sel)>,
    part: Option<Part>,
    /// Each test's result in this run.
    pub results: HashMap<NodeId, Outcome>,
    /// Tests this run put in `Queued` or `Running`.
    overlaid: HashSet<NodeId>,
    pub total: u32,
    pub first_broken: Option<String>,
    pub unsaved: BTreeSet<String>,
    /// The files of its tests: what "saved since" counts (§7.2's head).
    pub files: BTreeSet<String>,
    build_ok: bool,
    pub step_label: Option<String>,
    /// The provider a listing or a build is of.
    provider: Option<u32>,
}

impl Run {
    pub fn is_tests(&self) -> bool {
        matches!(self.job, Job::Tests(_))
    }

    fn current_provider(&self) -> Option<u32> {
        self.part.as_ref().map(|p| p.provider)
    }
}

const NO_RESULT: &str = "the run ended without a result for it";

impl Model {
    /// The model's message for a run that timed out (§8.3).
    fn timed_out_text(&self) -> String {
        format!("timed out after {} min", self.timeout_minutes)
    }

    /// `tests.timeout_minutes`, for the message a timed-out test gets.
    pub fn set_timeout_minutes(&mut self, minutes: u32) {
        self.timeout_minutes = minutes;
    }

    /// Whether listings no key asked for may run: the project is trusted
    /// and `tests.list_automatically` is on (§7.1, §12).
    pub fn set_automatic_listings(&mut self, allowed: bool) {
        self.auto_listing = allowed;
        if !allowed {
            self.listings.clear();
        }
    }

    /// The tests page opened. The first time in the session, every
    /// provider shown is listed, as an automatic listing (§7.1).
    pub fn page_opened(&mut self) {
        if self.page_opened {
            return;
        }
        self.page_opened = true;
        for p in 0..self.providers.len() as u32 {
            if !self.providers[p as usize].hidden() {
                self.list_provider(p, true);
            }
        }
    }

    /// `u` on the page: list `provider`, or every provider shown. Asked
    /// with a key, so it runs whatever `set_automatic_listings` says.
    pub fn list(&mut self, provider: Option<&str>) {
        match provider {
            Some(id) => {
                if let Some(p) = self.find_provider(id) {
                    self.list_provider(p, false);
                }
            }
            None => {
                for p in 0..self.providers.len() as u32 {
                    if !self.providers[p as usize].hidden() {
                        self.list_provider(p, false);
                    }
                }
            }
        }
    }

    /// Asks for a listing of provider `p`, after the run going if one is
    /// (§7.1, "never while a run goes").
    pub(crate) fn list_provider(&mut self, p: u32, automatic: bool) {
        if automatic && !self.auto_listing {
            return;
        }
        if !self.listings.contains(&p) {
            self.listings.push(p);
        }
    }

    /// Asks to run `selection` (§8.1). One run goes at a time; one waits.
    pub fn request(&mut self, selection: Selection, mode: Mode) -> Requested {
        self.ensure_built();
        if self.resolve(std::slice::from_ref(&selection)).is_empty() {
            return Requested::Nothing;
        }
        self.revision_bump();
        let Some(run) = self.run.as_ref() else {
            match (&mut self.waiting, mode) {
                (Some(waiting), Mode::Queue) => {
                    if !waiting.contains(&selection) {
                        waiting.push(selection);
                    }
                }
                (waiting, _) => *waiting = Some(vec![selection]),
            }
            return Requested::Starts;
        };
        let same = matches!(&run.job, Job::Tests(s) if s.len() == 1 && s[0] == selection) && run.halt.is_none();
        match mode {
            Mode::Replace => {
                self.stop("replaced by another run");
                self.waiting = Some(vec![selection]);
                Requested::Replaces
            }
            Mode::ReplaceWaiting => {
                self.waiting = Some(vec![selection]);
                Requested::Waits
            }
            Mode::Queue => match &mut self.waiting {
                Some(waiting) => {
                    if !waiting.contains(&selection) {
                        waiting.push(selection);
                    }
                    Requested::Joined
                }
                None if same => Requested::AlreadyGoing,
                None => {
                    self.waiting = Some(vec![selection]);
                    Requested::Waits
                }
            },
        }
    }

    /// How many tests `selections` holds now.
    pub(crate) fn count_selected(&self, selections: &[Selection]) -> u32 {
        self.resolve(selections).iter().map(|(p, sel)| self.sel_leaves(*p, sel).len() as u32).sum()
    }

    /// The selection the last run started with (`.`, `SPC T l`).
    pub fn last_selection(&self) -> Option<&[Selection]> {
        self.last_selection.as_deref()
    }

    /// Asks to run a debug launch's `build-first` plan for test `id` of
    /// `provider` (§11). It goes before a waiting run: you're waiting at
    /// the debugger for it.
    pub fn request_build(&mut self, provider: &str, id: &str, plan: Plan) -> Requested {
        let Some(p) = self.find_provider(provider) else { return Requested::Nothing };
        let plan = self.bound_plan(p, plan);
        self.waiting_build = Some((p, limits::cut_id(id).to_string(), plan));
        if self.run.is_some() {
            Requested::Waits
        } else {
            Requested::Starts
        }
    }

    /// Stops the run going (`x`, `SPC T k`): the step under way is
    /// stopped, those after it don't start, and its tests that hadn't
    /// ended go back to what they were (§4, §8.2). The host kills the
    /// step under way; reports from a call already under way, and the
    /// step's end, still come and stand. False when nothing was going.
    pub fn stop(&mut self, why: &str) -> bool {
        match &mut self.run {
            Some(run) if run.halt.is_none() => {
                run.halt = Some(Halt::Stopped(why.to_string()));
                self.revision_bump();
                true
            }
            _ => false,
        }
    }

    /// `tests.timeout_minutes` passed (§8.3): the host kills the step
    /// under way and ends it `timed-out`; those after it are `stopped`,
    /// and every test of the run without a result is errored.
    pub fn time_out(&mut self) -> bool {
        match &mut self.run {
            Some(run) if run.halt.is_none() => {
                run.halt = Some(Halt::TimedOut);
                true
            }
            _ => false,
        }
    }

    /// Provider `provider`'s extension stopped: a trap, or started again
    /// in another project (§6.4, "After a trap", "One project"). When its
    /// turn in the run is under way, the run is stopped (the host kills
    /// its step) and no further call goes to it; when its turn hadn't
    /// come, it's left out. Its waiting listing or build is dropped.
    pub fn provider_stopped(&mut self, provider: &str, why: &str) {
        let Some(p) = self.find_provider(provider) else { return };
        self.listings.retain(|&l| l != p);
        if self.waiting_build.as_ref().is_some_and(|b| b.0 == p) {
            self.waiting_build = None;
        }
        let Some(run) = &mut self.run else { return };
        run.queue.retain(|(q, _)| *q != p);
        if run.current_provider() == Some(p) {
            if let Some(part) = run.part.as_mut() {
                part.dead = true;
                part.current = None;
                part.phase = Phase::Steps;
                part.labels.clear();
                part.covers.clear();
            }
            if run.halt.is_none() {
                run.halt = Some(Halt::Stopped(why.to_string()));
            }
        }
        self.revision_bump();
    }

    pub(crate) fn revision_bump(&mut self) {
        // `changed` without the structure: a run's state moved.
        let _ = self.tick();
        self.changed();
    }

    /// What the host does next, or none while something is under way
    /// (a plan asked, a step running) or nothing is left to do. `now` is
    /// the time, in ms since 1970: when a run or step starts and ends.
    pub fn next(&mut self, now: u64) -> Option<Action> {
        self.ensure_built();
        loop {
            if self.run.is_none() {
                if let Some((p, test, plan)) = self.waiting_build.take() {
                    self.start_run(Job::Build(test), vec![(p, Sel::Nodes(Vec::new()))], now);
                    let part = self.run.as_mut().and_then(|r| r.part.as_mut());
                    if let Some(part) = part {
                        Self::take_plan(part, plan);
                    }
                    continue;
                }
                if let Some(selections) = self.waiting.take() {
                    let parts = self.resolve(&selections);
                    self.last_selection = Some(selections.clone());
                    self.start_run(Job::Tests(selections), parts, now);
                    continue;
                }
                while !self.listings.is_empty() {
                    let p = self.listings.remove(0);
                    if !self.providers[p as usize].hidden() {
                        self.start_run(Job::Listing, vec![(p, Sel::All)], now);
                        break;
                    }
                }
                self.run.as_ref()?;
                continue;
            }
            let run = self.run.as_mut().expect("a run");
            if run.part.is_none() {
                match run.queue.pop_front() {
                    Some((p, sel)) if run.halt.is_none() => {
                        run.part = Some(Self::new_part(p, sel));
                        continue;
                    }
                    _ => return Some(Action::Ended(self.end_run(now))),
                }
            }
            let run_id = run.id;
            let halted = run.halt.is_some();
            let part = run.part.as_mut().expect("a part");
            let provider = self.providers[part.provider as usize].id.clone();
            match part.phase {
                Phase::NeedPlan if !part.dead && !halted => {
                    part.phase = Phase::AwaitPlan;
                    let ask = self.ask();
                    return Some(Action::Plan { run: run_id, provider, ask });
                }
                Phase::NeedPlan => {
                    self.end_part();
                    continue;
                }
                Phase::AwaitPlan => return None,
                Phase::Steps => {
                    if part.current.is_some() {
                        return None;
                    }
                    let len = part.labels.len() as u32;
                    if halted || part.dead {
                        if part.next_step < len && !part.dead {
                            let steps = part.next_step..len;
                            part.next_step = len;
                            return Some(Action::NotStarted { run: run_id, provider, steps });
                        }
                        self.end_part();
                        continue;
                    }
                    if part.next_step < len {
                        let step = part.next_step;
                        let label = part.labels[step as usize].clone();
                        self.start_step(step, now);
                        return Some(Action::Step { run: run_id, provider, step, label });
                    }
                    self.end_part();
                    continue;
                }
            }
        }
    }

    fn new_part(provider: u32, sel: Sel) -> Part {
        let asked = match &sel {
            Sel::All => HashSet::new(),
            Sel::Nodes(nodes) => nodes.iter().copied().collect(),
        };
        Part {
            provider,
            sel,
            asked,
            phase: Phase::NeedPlan,
            labels: Vec::new(),
            covers: Vec::new(),
            named_by_steps: HashSet::new(),
            list_after: false,
            next_step: 0,
            current: None,
            started_seen: false,
            rest_running: false,
            all_exited: true,
            all_zero: true,
            planned_ok: false,
            broken: false,
            crashed: false,
            dead: false,
            ended_in: HashMap::new(),
            named: HashSet::new(),
            printed: HashMap::new(),
        }
    }

    fn take_plan(part: &mut Part, plan: Plan) {
        part.labels = plan.steps.iter().map(|s| s.label.clone()).collect();
        part.covers = plan.steps.iter().map(|s| s.covers.iter().cloned().collect()).collect();
        part.named_by_steps = part.covers.iter().flatten().cloned().collect();
        part.list_after = plan.list_after;
        part.phase = Phase::Steps;
        part.planned_ok = true;
    }

    fn start_run(&mut self, job: Job, parts: Vec<(u32, Sel)>, now: u64) {
        let id = RunId(self.next_run);
        self.next_run += 1;
        let started_seq = self.tick();
        let provider = parts.first().map(|p| p.0);
        let mut run = Run {
            provider,
            id,
            job,
            started_ms: now,
            step_ms: now,
            started_seq,
            halt: None,
            queue: parts.into_iter().collect(),
            part: None,
            results: HashMap::new(),
            overlaid: HashSet::new(),
            total: 0,
            first_broken: None,
            unsaved: BTreeSet::new(),
            files: BTreeSet::new(),
            build_ok: false,
            step_label: None,
        };
        if matches!(run.job, Job::Tests(_)) {
            // Every test the run has is queued (§4 "States").
            let mut leaves = Vec::new();
            for (p, sel) in &run.queue {
                leaves.extend(self.sel_leaves(*p, sel));
            }
            let leaves: BTreeSet<NodeId> = leaves.into_iter().collect();
            run.total = leaves.len() as u32;
            for &leaf in &leaves {
                if let Some(file) = &self.n(leaf).file {
                    run.files.insert(file.clone());
                    if self.unsaved.contains(file) {
                        run.unsaved.insert(file.clone());
                    }
                }
            }
            run.overlaid = leaves.iter().copied().collect();
            self.run = Some(run);
            for leaf in leaves {
                self.update(leaf, |n| n.live = Live::Queued);
            }
        } else {
            if let Job::Build(_) = run.job {
                run.part = run.queue.pop_front().map(|(p, sel)| Self::new_part(p, sel));
            }
            self.run = Some(run);
        }
        self.revision_bump();
    }

    /// The counted tests a part's selection holds, as the tree has them now.
    fn sel_leaves(&self, p: u32, sel: &Sel) -> Vec<NodeId> {
        match sel {
            Sel::All => self.leaves_of(p),
            Sel::Nodes(nodes) => {
                let mut out = Vec::new();
                for &node in nodes {
                    self.leaves_under(node, &mut out);
                }
                out.sort();
                out.dedup();
                out
            }
        }
    }

    /// Selections worked out into each provider's part: the topmost of
    /// the nodes selected, in provider order; `All` wins over the rest.
    fn resolve(&self, selections: &[Selection]) -> Vec<(u32, Sel)> {
        let mut all: BTreeSet<u32> = BTreeSet::new();
        let mut nodes: HashMap<u32, HashSet<NodeId>> = HashMap::new();
        let visible = |p: u32| !self.providers[p as usize].hidden();
        for selection in selections {
            match selection {
                Selection::All => {
                    for p in 0..self.providers.len() as u32 {
                        if visible(p) && !self.providers[p as usize].ids.is_empty() {
                            all.insert(p);
                        }
                    }
                }
                Selection::Failed => {
                    for (id, node) in self.nodes() {
                        let failed = node.merged.kind.is_test() && node.result.as_ref().is_some_and(|r| matches!(r.outcome, Outcome::Failed | Outcome::Errored));
                        if failed && visible(node.provider) {
                            nodes.entry(node.provider).or_default().insert(id);
                        }
                    }
                }
                Selection::File(path) => {
                    for (id, node) in self.nodes() {
                        if node.merged.kind != Kind::Group && node.file.as_deref() == Some(path.as_str()) && visible(node.provider) {
                            nodes.entry(node.provider).or_default().insert(id);
                        }
                    }
                }
                Selection::Nodes(keys) => {
                    for key in keys {
                        if let Some(id) = self.find(key) {
                            let p = self.n(id).provider;
                            if visible(p) {
                                nodes.entry(p).or_default().insert(id);
                            }
                        }
                    }
                }
            }
        }
        let mut providers: Vec<u32> = all.iter().copied().chain(nodes.keys().copied()).collect::<BTreeSet<_>>().into_iter().collect();
        providers.sort_by(|a, b| self.providers[*a as usize].id.cmp(&self.providers[*b as usize].id));
        let mut out = Vec::new();
        for p in providers {
            if all.contains(&p) {
                out.push((p, Sel::All));
                continue;
            }
            let set = &nodes[&p];
            let mut top: Vec<NodeId> = set.iter().copied().filter(|&id| !self.ancestry(id).skip(1).any(|a| set.contains(&a))).collect();
            top.sort_by_key(|&id| self.n(id).order);
            out.push((p, Sel::Nodes(top)));
        }
        out
    }

    /// The ask for the current part (§4 "What the provider is asked",
    /// F2, R1): the nodes selected as the model has them now, each `file`
    /// or `suite` followed by the tests and cases under it while the ask
    /// stays within 10,000 records; a selection of more as their
    /// parents, coarser but never fewer; the whole project as `run-all`.
    fn ask(&mut self) -> Ask {
        let run = self.run.as_ref().expect("a run");
        let part = run.part.as_ref().expect("a part");
        if matches!(run.job, Job::Listing) {
            return Ask::Discover;
        }
        let nodes = match &part.sel {
            Sel::All => return Ask::RunAll,
            Sel::Nodes(nodes) => nodes.iter().copied().filter(|&n| self.raw(n).is_some()).collect::<Vec<_>>(),
        };
        match self.ask_records(nodes) {
            Some(records) => Ask::Run(records),
            None => Ask::RunAll,
        }
    }

    /// The records of a `run` ask for `nodes`, or none for `run-all`.
    pub(crate) fn ask_records(&self, mut nodes: Vec<NodeId>) -> Option<Vec<Found>> {
        while nodes.len() > ASK_RECORDS {
            let deepest = nodes.iter().map(|&n| self.n(n).depth).max().unwrap_or(0);
            if deepest == 0 {
                return None;
            }
            let mut coarser: Vec<NodeId> = nodes.iter().map(|&n| if self.n(n).depth == deepest { self.n(n).parent.unwrap_or(n) } else { n }).collect();
            coarser.sort();
            coarser.dedup();
            let set: HashSet<NodeId> = coarser.iter().copied().collect();
            nodes = coarser.into_iter().filter(|&id| !self.ancestry(id).skip(1).any(|a| set.contains(&a))).collect();
        }
        nodes.sort_by_key(|&n| self.n(n).order);
        let mut room = ASK_RECORDS - nodes.len();
        let mut records = Vec::new();
        for node in nodes {
            records.push(self.n(node).merged.clone());
            if !matches!(self.n(node).merged.kind, Kind::File | Kind::Suite) {
                continue;
            }
            let mut under = Vec::new();
            self.tests_under(node, &mut under);
            if under.len() <= room {
                room -= under.len();
                records.extend(under.into_iter().map(|n| self.n(n).merged.clone()));
            }
        }
        Some(records)
    }

    /// The tests and cases under `id`, in display order.
    fn tests_under(&self, id: NodeId, out: &mut Vec<NodeId>) {
        for &child in &self.n(id).children {
            if self.n(child).merged.kind.is_test() {
                out.push(child);
            }
            self.tests_under(child, out);
        }
    }

    fn bound_plan(&mut self, p: u32, mut plan: Plan) -> Plan {
        if plan.steps.len() > PLAN_STEPS {
            plan.steps.truncate(PLAN_STEPS);
            self.say_once(p, "a plan's steps past 256 were left out");
        }
        let mut ids = 0usize;
        let mut said = Vec::new();
        for step in &mut plan.steps {
            limits::bound_chars(&mut step.label, limits::LABEL_CHARS, "a step's label over 256 characters was cut", &mut |w| said.push(w));
            let room = PLAN_COVERS.saturating_sub(ids);
            if step.covers.len() > room {
                step.covers.truncate(room);
                said.push("a plan's covers past 10,000 ids were left out");
            }
            ids += step.covers.len();
            for id in &mut step.covers {
                limits::bound(id, limits::ID_BYTES, "an id over 1 KiB was cut", &mut |w| said.push(w));
            }
        }
        for what in said {
            self.say_once(p, what);
        }
        plan
    }

    fn part_for(&self, run: RunId) -> Option<&Part> {
        self.run.as_ref().filter(|r| r.id == run).and_then(|r| r.part.as_ref()).filter(|p| !p.dead)
    }

    /// `tests-plan`'s answer for the plan `next` asked for. An error
    /// errors the provider's tests in the run with it (Decisions, 12).
    pub fn planned(&mut self, run: RunId, plan: Result<Plan, String>) {
        let Some(part) = self.part_for(run).filter(|p| p.phase == Phase::AwaitPlan) else { return };
        let p = part.provider;
        match plan {
            Ok(plan) => {
                let plan = self.bound_plan(p, plan);
                let part = self.part_mut().expect("a part");
                Self::take_plan(part, plan);
            }
            Err(error) => {
                let error = limits::cut_start(&error, MESSAGE_BYTES).to_string();
                let part = self.part_mut().expect("a part");
                part.phase = Phase::Steps;
                part.all_exited = false;
                if self.is_tests_run() {
                    let leaves = self.universe();
                    for leaf in leaves {
                        if !self.has_run_result(leaf) {
                            self.give_result(leaf, Outcome::Errored, Some(error.clone()), None, None, None, None);
                        }
                    }
                }
            }
        }
        self.revision_bump();
    }

    fn part_mut(&mut self) -> Option<&mut Part> {
        self.run.as_mut().and_then(|r| r.part.as_mut())
    }

    fn part(&self) -> &Part {
        self.run.as_ref().and_then(|r| r.part.as_ref()).expect("a part")
    }

    fn is_tests_run(&self) -> bool {
        self.run.as_ref().is_some_and(|r| matches!(r.job, Job::Tests(_)))
    }

    fn has_run_result(&self, id: NodeId) -> bool {
        self.run.as_ref().is_some_and(|r| r.results.contains_key(&id))
    }

    /// The counted tests the current part answers for: those its
    /// selection holds now, cases found in the run included. Steps judge
    /// nothing outside it (Decisions, 10).
    fn universe(&self) -> Vec<NodeId> {
        let part = self.part();
        self.sel_leaves(part.provider, &part.sel)
    }

    fn in_covers(&self, id: NodeId, covers: &HashSet<String>) -> bool {
        !covers.is_empty() && self.ancestry(id).any(|a| covers.contains(&self.n(a).id))
    }

    /// Step `step`'s own tests, or, for a step with empty `covers`, the
    /// plan's rest: what of the ask no step names (§8.5; Decisions, 11).
    fn step_scope(&self, step: u32) -> Vec<NodeId> {
        let part = self.part();
        let covers = &part.covers[step as usize];
        let leaves = self.universe();
        if covers.is_empty() {
            leaves.into_iter().filter(|&l| !self.in_covers(l, &part.named_by_steps)).collect()
        } else {
            leaves.into_iter().filter(|&l| self.in_covers(l, covers)).collect()
        }
    }

    fn start_step(&mut self, step: u32, now: u64) {
        let label = {
            let run = self.run.as_mut().expect("a run");
            run.step_ms = now;
            let part = run.part.as_mut().expect("a part");
            part.current = Some(step);
            part.started_seen = false;
            part.labels[step as usize].clone()
        };
        self.run.as_mut().expect("a run").step_label = Some(label);
        if !self.is_tests_run() {
            return;
        }
        // Its tests run (§4 "States": for a runner that doesn't say,
        // its step started); the plan's rest with the first step that
        // claims nothing (§8.5).
        let empty = self.part().covers[step as usize].is_empty();
        if empty && self.part().rest_running {
            return;
        }
        if empty {
            self.part_mut().expect("a part").rest_running = true;
        }
        for leaf in self.step_scope(step) {
            if !self.has_run_result(leaf) {
                self.run.as_mut().expect("a run").overlaid.insert(leaf);
                self.update(leaf, |n| n.live = Live::Running);
            }
        }
    }

    /// Reports `tests-read` gave for step `step` of run `run`, applied in
    /// order. Reports for a run that isn't going, or a step that isn't
    /// under way, are dropped.
    pub fn reports(&mut self, run: RunId, step: u32, reports: Vec<Report>) {
        if self.part_for(run).is_none_or(|p| p.current != Some(step)) {
            return;
        }
        self.apply_reports(step, reports);
        self.changed();
    }

    /// Step `step` of run `run` ended (`tests-step-ended`), with what
    /// its reader still had. Its tests without a result are judged by how
    /// it ended (§8.5): stopped, back to what they were; exited, errored
    /// "the run ended without a result for it", unless their file had
    /// unsaved edits; timed out or failed, errored with why. At the
    /// plan's last step, the plan's rest is judged the same way.
    pub fn step_ended(&mut self, run: RunId, step: u32, end: StepEnd, reports: Vec<Report>) {
        if self.part_for(run).is_none_or(|p| p.current != Some(step)) {
            return;
        }
        self.apply_reports(step, reports);
        self.ensure_built();
        let end = match end {
            StepEnd::Failed(why) => StepEnd::Failed(limits::cut_start(&why, MESSAGE_BYTES).to_string()),
            end => end,
        };
        {
            let part = self.part_mut().expect("a part");
            part.all_exited &= matches!(end, StepEnd::Exited(_));
            part.all_zero &= end == StepEnd::Exited(Some(0));
        }
        if self.is_tests_run() {
            let mut judged: Vec<NodeId> = Vec::new();
            if !self.part().covers[step as usize].is_empty() {
                judged.extend(self.step_scope(step));
            }
            if step as usize + 1 == self.part().labels.len() {
                let named = self.part().named_by_steps.clone();
                judged.extend(self.universe().into_iter().filter(|&l| !self.in_covers(l, &named)));
            }
            self.judge(judged, &end);
        }
        let part = self.part_mut().expect("a part");
        part.current = None;
        part.next_step = step + 1;
        self.run.as_mut().expect("a run").step_label = None;
        self.changed();
    }

    /// The provider fell behind its step's output (§6.7): the step's
    /// tests without a result are errored with `message`.
    pub fn step_unread(&mut self, run: RunId, step: u32, message: &str) {
        if self.part_for(run).is_none_or(|p| p.current != Some(step)) || !self.is_tests_run() {
            return;
        }
        self.ensure_built();
        for leaf in self.step_scope(step) {
            if !self.has_run_result(leaf) {
                self.give_result(leaf, Outcome::Errored, Some(message.to_string()), None, None, None, None);
            }
        }
        self.changed();
    }

    fn judge(&mut self, leaves: Vec<NodeId>, end: &StepEnd) {
        let timed_out = self.timed_out_text();
        for leaf in leaves {
            if self.has_run_result(leaf) || self.raw(leaf).is_none() {
                continue;
            }
            let unsaved = self.n(leaf).file.as_ref().is_some_and(|f| self.run.as_ref().expect("a run").unsaved.contains(f));
            match end {
                StepEnd::Stopped => self.update(leaf, |n| n.live = Live::Idle),
                StepEnd::Exited(_) if unsaved => self.update(leaf, |n| n.live = Live::Idle),
                StepEnd::Exited(_) => self.give_result(leaf, Outcome::Errored, Some(NO_RESULT.to_string()), None, None, None, None),
                StepEnd::TimedOut => self.give_result(leaf, Outcome::Errored, Some(timed_out.clone()), None, None, None, None),
                StepEnd::Failed(why) => self.give_result(leaf, Outcome::Errored, Some(why.clone()), None, None, None, None),
            }
        }
    }

    /// Node `id`'s result in this run: the last `ended` wins (§4 "A result").
    #[allow(clippy::too_many_arguments)]
    fn give_result(&mut self, id: NodeId, outcome: Outcome, message: Option<String>, at: Option<Location>, text: Option<String>, duration_ms: Option<f64>, step: Option<u32>) {
        let seq = self.tick();
        let run = self.run.as_mut().expect("a run");
        let when_ms = run.step_ms;
        let again = run.results.insert(id, outcome).is_some();
        let pending = run.part.as_mut().and_then(|part| {
            if let Some(step) = step {
                part.ended_in.insert(id, step);
            }
            part.printed.remove(&id)
        });
        self.update(id, |n| {
            // A second `ended` in the run keeps what the test printed.
            let mut printed = if again { n.result.take().map(|r| r.printed).unwrap_or_default() } else { String::new() };
            printed.push_str(&pending.unwrap_or_default());
            n.result = Some(TestResult { outcome, duration_ms, when_ms, message, at, text, printed, restored: false, seq });
            n.live = Live::Idle;
        });
    }

    fn apply_reports(&mut self, step: u32, mut reports: Vec<Report>) {
        let p = self.part().provider;
        if reports.len() > REPORTS_PER_CALL {
            reports.truncate(REPORTS_PER_CALL);
            self.say_once(p, "a call's reports past 10,000 were left out");
        }
        for report in reports {
            self.apply(p, step, report);
        }
    }

    fn apply(&mut self, p: u32, step: u32, report: Report) {
        let job = match &self.run.as_ref().expect("a run").job {
            Job::Tests(_) => 0,
            Job::Listing => 1,
            Job::Build(_) => 2,
        };
        match report {
            Report::Found(found) => {
                let source = match job {
                    0 => Source::Run,
                    1 => Source::Listing,
                    _ => return,
                };
                if let Some(id) = self.set_source(p, found, source, None) {
                    self.part_mut().expect("a part").named.insert(id);
                }
            }
            Report::Started(id) => {
                if job != 0 {
                    return;
                }
                let Some(id) = self.lookup(p, &id) else { return };
                self.part_mut().expect("a part").named.insert(id);
                if !self.part().started_seen {
                    // The runner says when each starts: the rest of the
                    // step's go back to queued (Decisions, 9).
                    self.part_mut().expect("a part").started_seen = true;
                    let running: Vec<NodeId> = self.run.as_ref().expect("a run").overlaid.iter().copied().filter(|&n| self.raw(n).is_some_and(|n| n.live == Live::Running)).collect();
                    for node in running {
                        self.update(node, |n| n.live = Live::Queued);
                    }
                }
                self.run.as_mut().expect("a run").overlaid.insert(id);
                self.update(id, |n| n.live = Live::Running);
            }
            Report::Ended(ended) => {
                if job != 0 {
                    return;
                }
                self.ended(p, step, ended);
            }
            Report::Output { id: Some(id), text } => {
                if job != 0 {
                    return;
                }
                let Some(id) = self.lookup(p, &id) else { return };
                let in_run = self.has_run_result(id);
                let part = self.part_mut().expect("a part");
                part.named.insert(id);
                if in_run {
                    if let Some(node) = self.raw_mut(id) {
                        if let Some(result) = &mut node.result {
                            result.printed.push_str(&text);
                            limits::bound_end(&mut result.printed, TEXT_BYTES, "", &mut |_| {});
                        }
                    }
                } else {
                    let printed = part.printed.entry(id).or_default();
                    printed.push_str(&text);
                    limits::bound_end(printed, TEXT_BYTES, "", &mut |_| {});
                }
            }
            Report::Output { id: None, .. } => {}
            Report::Broken { id, message, mut at } => {
                let message = limits::cut_start(&message, MESSAGE_BYTES).to_string();
                if let Some(at) = &mut at {
                    let mut cut = false;
                    limits::bound(&mut at.path, limits::PATH_BYTES, "", &mut |_| cut = true);
                    if cut {
                        self.say_once(p, "a path over 4 KiB was cut");
                    }
                }
                let run = self.run.as_mut().expect("a run");
                if run.first_broken.is_none() {
                    run.first_broken = Some(message.clone());
                }
                self.part_mut().expect("a part").broken = true;
                if job == 0 {
                    self.errored_under(p, step, id, message, at, "broken");
                }
            }
            Report::Crashed { id, message } => {
                let message = limits::cut_start(&message, MESSAGE_BYTES).to_string();
                self.part_mut().expect("a part").crashed = true;
                if job == 0 {
                    self.errored_under(p, step, id, message, None, "crashed");
                }
            }
        }
    }

    /// `ended` (§4 "Ids, and matching"): a known id's result; one found
    /// earlier in the run is known by then; an unknown one is added as a
    /// test labelled by its id, found in a run, under the deepest node of
    /// the file its `at` names, else at its provider's top.
    fn ended(&mut self, p: u32, step: u32, ended: Ended) {
        let mut ended = ended;
        let mut said = Vec::new();
        limits::bound(&mut ended.id, limits::ID_BYTES, "an id over 1 KiB was cut", &mut |w| said.push(w));
        if let Some(message) = &mut ended.message {
            limits::bound(message, MESSAGE_BYTES, "a message over 4 KiB was cut", &mut |w| said.push(w));
        }
        if let Some(text) = &mut ended.text {
            limits::bound_end(text, TEXT_BYTES, "a failure's text over 64 KiB was cut", &mut |w| said.push(w));
        }
        if let Some(at) = &mut ended.at {
            limits::bound(&mut at.path, limits::PATH_BYTES, "a path over 4 KiB was cut", &mut |w| said.push(w));
        }
        for what in said {
            self.say_once(p, what);
        }
        let duration = ended.duration_ms.filter(|d| d.is_finite() && *d >= 0.0);
        let id = match self.lookup(p, &ended.id) {
            Some(id) => id,
            None => {
                self.ensure_built();
                let parent = ended.at.as_ref().and_then(|at| self.deepest_in(p, &at.path, at.line));
                let mut found = Found::new(ended.id.clone(), parent.map(|n| self.n(n).id.as_str()), limits::cut_chars(&ended.id, limits::LABEL_CHARS), Kind::Test);
                found.parent = parent.map(|n| self.n(n).id.clone());
                let Some(id) = self.set_source(p, found, Source::Run, None) else { return };
                id
            }
        };
        self.part_mut().expect("a part").named.insert(id);
        self.give_result(id, ended.outcome, ended.message, ended.at, ended.text, duration, Some(step));
    }

    /// The deepest group, file or suite of provider `p` in file `path`,
    /// preferring one whose lines hold `line` (Decisions, 21).
    fn deepest_in(&self, p: u32, path: &str, line: u32) -> Option<NodeId> {
        let holds = |id: NodeId| {
            let node = self.n(id);
            match (node.merged.line, node.merged.end_line) {
                (Some(start), Some(end)) => start <= line && line <= end,
                (Some(start), None) => start <= line,
                _ => false,
            }
        };
        self.providers[p as usize]
            .ids
            .values()
            .copied()
            .filter(|&id| {
                let node = self.n(id);
                !node.merged.kind.is_test() && node.merged.path.as_deref() == Some(path)
            })
            .max_by_key(|&id| (holds(id), self.n(id).depth, std::cmp::Reverse(self.n(id).order)))
    }

    /// A `broken` or `crashed` (§8.5, F4): what's under `id` and hasn't
    /// ended in this step is errored with `message`; with no `id`, or one
    /// the provider never found (said on its page), what the step covers.
    fn errored_under(&mut self, p: u32, step: u32, id: Option<String>, message: String, at: Option<Location>, what: &str) {
        self.ensure_built();
        let node = match id {
            Some(id) => match self.lookup(p, &id) {
                Some(node) => Some(node),
                None => {
                    let id = limits::cut_chars(&id, 200).to_string();
                    self.say(p, format!("a `{what}` named {id}, which it hasn't found: taken as all the step covers"));
                    None
                }
            },
            None => None,
        };
        let targets: Vec<NodeId> = match node {
            Some(node) => self.universe().into_iter().filter(|&l| self.ancestry(l).any(|a| a == node)).collect(),
            None => self.step_scope(step),
        };
        for leaf in targets {
            if self.part().ended_in.get(&leaf) == Some(&step) {
                continue;
            }
            self.give_result(leaf, Outcome::Errored, Some(message.clone()), at.clone(), None, None, Some(step));
        }
    }

    /// The current part is done: its records dropped as its sources'
    /// rules say (§4 "What's dropped"), a listing after it asked for
    /// (§7.1), and the next provider's turn.
    fn end_part(&mut self) {
        self.ensure_built();
        let run = self.run.as_mut().expect("a run");
        let halted = run.halt.is_some();
        let started_seq = run.started_seq;
        let job = match &run.job {
            Job::Tests(_) => 0,
            Job::Listing => 1,
            Job::Build(_) => 2,
        };
        let Some(part) = run.part.take() else { return };
        let stepped = !part.labels.is_empty();
        let complete = !halted && !part.dead && part.planned_ok && stepped && part.all_exited && !part.broken && !part.crashed;
        let p = part.provider;
        match job {
            0 => {
                if !halted && !part.dead && part.planned_ok && !stepped {
                    // A plan of no steps: its whole ask is the plan's
                    // rest, judged at once as if it ended (Decisions, 13).
                    self.run.as_mut().expect("a run").part = Some(part);
                    let leaves = self.universe();
                    self.judge(leaves, &StepEnd::Exited(Some(0)));
                    let part = self.run.as_mut().expect("a run").part.take().expect("a part");
                    return self.after_part(part, false);
                }
                if complete {
                    // A run's records go when their template or group ran
                    // again, complete, without naming them (§4).
                    let gone: Vec<NodeId> = self.providers[p as usize]
                        .ids
                        .values()
                        .copied()
                        .filter(|&id| {
                            let node = self.n(id);
                            node.run.is_some() && !part.named.contains(&id) && (matches!(part.sel, Sel::All) || self.ancestry(id).skip(1).any(|a| part.asked.contains(&a)))
                        })
                        .collect();
                    for id in gone {
                        self.drop_source(id, Source::Run, None);
                    }
                }
                let list = part.list_after && !halted && !part.dead && part.planned_ok && part.all_exited && !part.broken;
                self.after_part(part, list);
            }
            1 => {
                if complete {
                    // A listing's records go when a complete listing
                    // doesn't give them, unless their file was saved
                    // since it began (§4, F14).
                    let gone: Vec<NodeId> = self.providers[p as usize]
                        .ids
                        .values()
                        .copied()
                        .filter(|&id| {
                            let node = self.n(id);
                            let saved_since = node.file.as_ref().and_then(|f| self.saved.get(f)).is_some_and(|&s| s > started_seq);
                            node.listing.is_some() && !part.named.contains(&id) && !saved_since
                        })
                        .collect();
                    for id in gone {
                        self.drop_source(id, Source::Listing, None);
                    }
                }
                self.after_part(part, false);
            }
            _ => {
                let ok = !halted && !part.dead && part.all_zero && !part.broken && !part.crashed;
                self.run.as_mut().expect("a run").build_ok = ok;
                self.after_part(part, false);
            }
        }
    }

    fn after_part(&mut self, part: Part, list: bool) {
        if list {
            self.list_provider(part.provider, true);
        }
        self.changed();
    }

    fn end_run(&mut self, now: u64) -> RunEnd {
        let run = self.run.take().expect("a run");
        let timed_out = self.timed_out_text();
        // What hadn't ended goes back to what it was; after a timeout,
        // it's errored (§8.3; Decisions, 14).
        let seq = self.tick();
        let mut results = run.results.clone();
        for id in run.overlaid.iter().copied() {
            let Some(node) = self.raw(id) else { continue };
            if node.live == Live::Idle {
                continue;
            }
            if run.halt == Some(Halt::TimedOut) && node.leaf && !results.contains_key(&id) {
                let message = timed_out.clone();
                let when_ms = run.step_ms;
                results.insert(id, Outcome::Errored);
                self.update(id, |n| {
                    n.result = Some(TestResult { outcome: Outcome::Errored, duration_ms: None, when_ms, message: Some(message), at: None, text: None, printed: String::new(), restored: false, seq });
                    n.live = Live::Idle;
                });
            } else {
                self.update(id, |n| n.live = Live::Idle);
            }
        }
        let count = |o: Outcome| results.values().filter(|&&r| r == o).count() as u32;
        let kind = match run.job {
            Job::Tests(selections) => RunKind::Tests(selections),
            Job::Listing | Job::Build(_) => {
                let provider = run.provider.map(|p| self.providers[p as usize].id.clone()).unwrap_or_default();
                match run.job {
                    Job::Build(test) => RunKind::Build { provider, test },
                    _ => RunKind::Listing { provider },
                }
            }
        };
        let end = RunEnd {
            run: run.id,
            kind,
            halted: run.halt.clone(),
            passed: count(Outcome::Passed),
            failed: count(Outcome::Failed),
            skipped: count(Outcome::Skipped),
            errored: count(Outcome::Errored),
            first_broken: run.first_broken.clone(),
            build_ok: run.build_ok,
            unsaved: run.unsaved.iter().cloned().collect(),
            started_ms: run.started_ms,
            ended_ms: now,
        };
        if matches!(end.kind, RunKind::Tests(_)) {
            self.last = Some(LastRun { end: end.clone(), started_seq: run.started_seq, files: run.files.clone() });
        }
        self.changed();
        end
    }

}
