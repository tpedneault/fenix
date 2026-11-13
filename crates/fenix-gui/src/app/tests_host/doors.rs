//! One story (plan `docs/plans/test-explorer.md` §2, §9.5, §10, step 7):
//! the doors people already use go through the explorer where a test
//! provider serves the file, and keep today's way where none does (§16,
//! Q2 and Q7).
//!
//! - **Which way** (`tests_door`): with `tests.enabled` on, a file whose
//!   tests a provider found (`Model::serves`, the gutter's own test) goes
//!   through the explorer -- the test the line is in, or why the line is
//!   in none. Anything else -- the explorer off, a file no provider found
//!   tests in, a remote project -- is today's way: `SPC p r`'s finder in
//!   the task pane, rust-analyzer's cargo in Task Output. The two never
//!   apply to the same file at once.
//! - **The doors**: `SPC p r` and Fenix's own lens (through `run_test_at`),
//!   the agents' `run_test`, rust-analyzer's run and debug lenses on a
//!   line the explorer has a test at (`tests_ra_lens`), Fenix's own lens
//!   over a file a provider serves ("▶ Run test | Debug",
//!   `tests_lens_lines`), and `SPC p T`, which runs again what `SPC p r`
//!   last ran -- a test run when it went through the explorer
//!   (`tests_rerun_door`).
//! - **A test run tells the rest of Fenix** (§10.1, §10.2, Q1): it's
//!   `last_task` when it ends (`tests: <what ran>`, the model's exact
//!   counts), extensions that ask for `tasks` hear `task-started` and
//!   `task-ended` once each, and one notification comes, as
//!   `notifications.tasks` says, whose *Show the failures* is
//!   `tests.open`. A listing and a debug's build are none of these.

use super::*;

/// Which way a door goes for a line of a file.
#[derive(Debug)]
pub(in crate::app) enum Door {
    /// No provider serves the file, or the explorer is off: today's way.
    Fallback,
    /// A provider serves it: the test or group the line is in.
    Test { root: PathBuf, node: fenix_tests::NodeId },
    /// A provider serves it and the line is in none of its tests: why.
    NotATest(String),
}

/// A door's last run, for `SPC p T`: the project's tests root, what was
/// asked, and what it's called.
pub(crate) type LastDoor = (PathBuf, Vec<Selection>, String);

/// "line 12 of geo.rs isn't in a test -- ..." (§9.4).
pub(in crate::app) fn not_in_a_test(rel: &str, line: u32) -> String {
    let name = Path::new(rel).file_name().map_or(rel.to_string(), |n| n.to_string_lossy().into_owned());
    format!("line {} of {name} isn't in a test -- tests.run_file runs this file's", line + 1)
}

/// What a run of `selections` is called as a task (§10.2): `tests:
/// src/lib.rs`, `tests: the failed`, `tests: adds`.
pub(crate) fn run_name(model: &Model, selections: &[Selection]) -> String {
    let parts: Vec<String> = selections
        .iter()
        .map(|selection| match selection {
            Selection::All => "all".to_string(),
            Selection::Failed => "the failed".to_string(),
            Selection::File(file) => file.clone(),
            Selection::Nodes(keys) => match keys.as_slice() {
                [key] => model.find(key).and_then(|n| model.node(n)).map_or_else(|| key.id.clone(), |n| crate::tests_page::clean(&n.found().label)),
                keys => format!("{} tests", keys.len()),
            },
        })
        .collect();
    format!("tests: {}", parts.join(", "))
}

/// `took` as the notification says it: `in 4 s`, `in 1 min 12 s`.
fn took_text(ms: u64) -> String {
    let secs = ms / 1000;
    if secs >= 60 {
        format!("in {} min {} s", secs / 60, secs % 60)
    } else {
        format!("in {secs} s")
    }
}

impl App {
    /// Which way a door goes for line `line` (from 0) of the file at
    /// `path` (§2): through the explorer only with it on, where a provider
    /// found tests in the file.
    pub(in crate::app) fn tests_door(&mut self, path: &Path, line: u32) -> Door {
        if !self.tests_enabled() {
            return Door::Fallback;
        }
        let Some(root) = self.tests_root_of(path) else { return Door::Fallback };
        if fenix_vfs::is_remote(&root) {
            return Door::Fallback;
        }
        let rel = self.tests_rel(&root, path);
        let Some(project) = self.tests.projects.get(&root) else { return Door::Fallback };
        if !project.model.serves(&rel) {
            return Door::Fallback;
        }
        match project.model.test_at(&rel, line) {
            Some(node) => Door::Test { root, node },
            None => Door::NotATest(not_in_a_test(&rel, line)),
        }
    }

    /// Runs test `node` of the project at `root` as a door asked for it
    /// (`SPC p r`, a lens, `run_test`), as `SPC T t` would, and keeps it as
    /// what `SPC p T` runs again in the project of `path`. What ran and
    /// what came of asking, or why nothing did (said on the modeline too).
    pub(in crate::app) fn tests_door_run(&mut self, path: &Path, root: &Path, node: fenix_tests::NodeId) -> Result<(String, fenix_tests::Requested), String> {
        let Some(n) = self.tests.projects.get(root).and_then(|p| p.model.node(node)) else {
            let why = "that test is gone from its provider's tests".to_string();
            self.set_error(why.clone());
            return Err(why);
        };
        let what = crate::tests_page::clean(&n.found().label);
        let selections = vec![Selection::Nodes(vec![n.key()])];
        let requested = self.tests_request(root, selections.clone(), &what)?;
        self.tests.last_door.insert(super::tool_sessions::root_for_path(path), (root.to_path_buf(), selections, what.clone()));
        Ok((what, requested))
    }

    /// `SPC p T` in the project at `key` (as `SPC p t` and `SPC p r` key
    /// it): when `SPC p r` last went through the explorer there, that
    /// test run again. Whether it was.
    pub(in crate::app) fn tests_rerun_door(&mut self, key: &Path) -> bool {
        if !self.tests_enabled() {
            return false;
        }
        let Some((root, selections, what)) = self.tests.last_door.get(key).cloned() else { return false };
        let _ = self.tests_request(&root, selections, &what);
        true
    }

    /// A task started in the task pane for the project at `key`: `SPC p T`
    /// runs it again, not a door's test run before it.
    pub(in crate::app) fn tests_task_ran(&mut self, key: &Path) {
        self.tests.last_door.remove(key);
    }

    /// Whether the explorer is on and a test provider serves the project
    /// at `root` (as `integration_root` gives it): the agents' `tasks`
    /// toolset is offered for it then (§10.3).
    pub(in crate::app) fn tests_project_has_providers(&self, root: &Path) -> bool {
        if self.tests.projects.is_empty() || !self.tests_enabled() {
            return false;
        }
        let root = self.tests.canon.get(root).map_or(root, PathBuf::as_path);
        self.tests.projects.get(root).is_some_and(|p| !p.providers.is_empty())
    }

    /// Whether test `node` of the project at `root` is offered to debug:
    /// a test or a case, nameable to its provider, of an extension that has
    /// `tests-launch` (step 6's review: no *Debug* where it then fails).
    pub(in crate::app) fn tests_debuggable(&self, root: &Path, node: fenix_tests::NodeId) -> bool {
        let Some(n) = self.tests.projects.get(root).and_then(|p| p.model.node(node)) else { return false };
        n.found().kind.is_test() && n.can_be_named() && !self.tests.no_launch.contains(n.key().provider.extension.as_ref())
    }

    /// The explorer's model revision when it serves the open file at
    /// `path` (Fenix's own lens is then the explorer's, §9.5): a look-up,
    /// what the lens's rows are kept by. None: today's lens.
    pub(in crate::app) fn tests_lens_revision(&self, path: &Path) -> Option<u64> {
        let (root, rel, project) = self.tests_file_of(path)?;
        (!fenix_vfs::is_remote(root) && project.model.serves(rel)).then(|| project.model.revision())
    }

    /// Fenix's own lens over a file a provider serves (§9.5): the lines a
    /// test or group starts on, each with whether a test there is offered
    /// to debug. None: the file isn't served (the explorer off, no
    /// provider found tests in it), and today's lenses are its.
    pub(in crate::app) fn tests_lens_lines(&self, path: &Path) -> Option<Vec<(u32, bool)>> {
        let (root, rel, project) = self.tests_file_of(path)?;
        if fenix_vfs::is_remote(root) || !project.model.serves(rel) {
            return None;
        }
        let model = &project.model;
        let mut lines: Vec<(u32, bool)> = Vec::new();
        for &id in model.in_file(rel) {
            let Some(line) = model.node(id).and_then(|n| n.found().line) else { continue };
            let debuggable = self.tests_debuggable(root, id);
            match lines.last_mut() {
                Some(last) if last.0 == line => last.1 |= debuggable,
                _ => lines.push((line, debuggable)),
            }
        }
        Some(lines)
    }

    /// The lenses of open files of the projects at `roots` (all, when
    /// none) laid out again: the model changed (`tests_views_due`).
    pub(in crate::app) fn tests_lenses_due(&mut self, roots: Option<&[PathBuf]>) {
        if self.lens_of.is_empty() {
            return;
        }
        let ids: Vec<BufferId> = self.tests.seen.iter().filter(|(id, s)| self.lens_of.contains_key(id) && roots.is_none_or(|r| r.contains(&s.root))).map(|(id, _)| *id).collect();
        for id in ids {
            // Worked out again whatever they're kept by: what's offered to
            // debug isn't the model's alone (`no_launch`).
            self.own_lenses.remove(&id);
            self.lay_out_lenses(id);
        }
    }

    /// The explorer off: Fenix's own lenses that were its, as they were
    /// before it (`SPC p r`'s finder's). Once: then none are its.
    pub(in crate::app) fn tests_lenses_off(&mut self) {
        let ids: Vec<BufferId> = self.own_lenses.iter().filter(|(_, (_, revision, _))| revision.is_some()).map(|(id, _)| *id).collect();
        for id in ids {
            self.lay_out_lenses(id);
        }
    }

    /// Fenix's own *Debug* lens at line `line` (from 1) of the file at
    /// `path`: the test there debugged (§11), under the lens's rule that it
    /// starts a program only in a trusted project (§9.5, §12).
    pub(in crate::app) fn tests_lens_debug(&mut self, path: &Path, line: usize) {
        match self.tests_door(path, line.saturating_sub(1) as u32) {
            Door::Test { root, node } => {
                if self.lens_may_start(&root) {
                    let _ = self.tests_debug(&root, node, None);
                }
            }
            Door::NotATest(why) => self.set_error(why),
            Door::Fallback => self.set_error("not run: no test provider serves this file now"),
        }
    }

    /// rust-analyzer's run or debug lens at line `line` (from 0) of the
    /// file at `path`, whose cargo arguments are `cargo_args`: the test the
    /// explorer has there, when it has one starting on that line or whose
    /// lines hold it (§2). A binary's `▶ Run` and a doc test aren't one.
    pub(in crate::app) fn tests_ra_lens(&mut self, path: &Path, line: u32, cargo_args: &[String]) -> Option<(PathBuf, fenix_tests::NodeId)> {
        if cargo_args.first().map(String::as_str) != Some("test") || cargo_args.iter().any(|a| a == "--doc") {
            return None;
        }
        let Door::Test { root, node } = self.tests_door(path, line) else { return None };
        let n = self.tests.projects.get(&root)?.model.node(node)?;
        let found = n.found();
        let holds = found.line == Some(line) || found.line.zip(found.end_line).is_some_and(|(start, end)| start <= line && line <= end);
        holds.then_some((root, node))
    }

    // What a test run tells the rest of Fenix (§10).

    /// A run of tests started in the project at `root` (its first action):
    /// extensions that ask for `tasks` hear `task-started`, once (Q1).
    pub(super) fn tests_door_started(&mut self, root: &Path, run: RunId) {
        let Some(project) = self.tests.projects.get_mut(root) else { return };
        if project.told_started == Some(run) {
            return;
        }
        project.told_started = Some(run);
        let name = run_name(&project.model, project.model.last_selection().unwrap_or_default());
        #[cfg(test)]
        self.tests.told.push(format!("started {name}"));
        self.editor_call("tasks", fenix_wasm::host::Call::TaskStarted(name));
    }

    /// A run of tests ended in the project at `root`: `last_task`, the
    /// extensions' `task-ended`, the notification, and the agents waiting
    /// for it (§10). Once a run: `tests_run_ended` calls it once.
    pub(super) fn tests_door_ended(&mut self, root: &Path, end: &fenix_tests::RunEnd) {
        let fenix_tests::RunKind::Tests(selections) = &end.kind else { return };
        let Some(project) = self.tests.projects.get(root) else { return };
        let name = run_name(&project.model, selections);
        // A run that ended before its first action is told it started too:
        // every `task-ended` has its `task-started`.
        if project.told_started != Some(end.run) {
            let project = self.tests.projects.get_mut(root).expect("a project");
            project.told_started = Some(end.run);
            #[cfg(test)]
            self.tests.told.push(format!("started {name}"));
            self.editor_call("tasks", fenix_wasm::host::Call::TaskStarted(name.clone()));
        }
        // Stopped by you (or the explorer turned off) is a stop; stopped
        // by its provider (a trap, its extension started again elsewhere)
        // is a failure you hear of, as a timeout is (step 7's review, S6,
        // S7).
        let stopped = matches!(end.halted, Some(fenix_tests::Halt::Stopped(_))) && self.tests.projects.get(root).is_some_and(|p| p.stopped_by_you == Some(end.run));
        let failed = end.failed + end.errored;
        let ok = end.halted.is_none() && failed == 0 && end.first_broken.is_none();
        // Stopped, it's as a task cancelled in the task pane: `last_task`
        // and the notification are left as they were (§20, step 7).
        if !stopped {
            self.wasm.last_task = Some(fenix_wasm::host::TaskResult { name: name.clone(), ok, tests: Some((end.passed, failed)) });
        }
        #[cfg(test)]
        self.tests.told.push(format!("ended {ok}"));
        self.editor_event("tasks", fenix_wasm::host::Notice::TaskEnded(ok));
        if !stopped {
            self.tests_notify(root, end, ok);
        }
        self.tests_agents_run_ended(root, end);
    }

    /// The tests' failures in run `end`, in display order: their labels.
    fn tests_failed_in(&self, root: &Path, end: &fenix_tests::RunEnd) -> Vec<fenix_tests::NodeId> {
        let Some(project) = self.tests.projects.get(root) else { return Vec::new() };
        let model = &project.model;
        model.failures().into_iter().filter(|&id| model.node(id).and_then(|n| n.result()).is_some_and(|r| !r.restored && r.when_ms >= end.started_ms)).collect()
    }

    /// One notification for a run that ended (§10.1), as
    /// `notifications.tasks` says (all, long -- 10 s or more --, failed,
    /// never), source `tests` and tag `tests`.
    fn tests_notify(&mut self, root: &Path, end: &fenix_tests::RunEnd, ok: bool) {
        use crate::notify::{Request, Urgency};
        let took = end.ended_ms.saturating_sub(end.started_ms);
        let wanted = match self.config.text_value("notifications.tasks").unwrap_or("long") {
            "all" => true,
            "failed" => !ok,
            "never" => false,
            // A run its provider ended (a trap) is said however quick.
            _ => took >= 10_000 || matches!(end.halted, Some(fenix_tests::Halt::Stopped(_))),
        };
        if !wanted {
            return;
        }
        let halted = match &end.halted {
            Some(fenix_tests::Halt::TimedOut) => Some(("The tests timed out".to_string(), format!("{} passed · tests.timeout_minutes", end.passed))),
            Some(fenix_tests::Halt::Stopped(why)) => Some(("The test run stopped".to_string(), crate::tests_page::clean(why))),
            None => None,
        };
        let request = if let Some((summary, body)) = halted {
            Request::new("tests", summary).body(body).urgency(Urgency::Normal).action("Show the tests", "tests.open").action("Show the output", "tests.output")
        } else if let Some(broken) = &end.first_broken {
            Request::new("tests", "The tests didn't build").body(crate::tests_page::clean(broken)).urgency(Urgency::Normal).action("Show it", "tests.output")
        } else if end.failed + end.errored > 0 {
            let failed = end.failed + end.errored;
            let labels: Vec<String> = self.tests_failed_in(root, end).into_iter().filter_map(|id| self.tests.projects.get(root)?.model.node(id).map(|n| crate::tests_page::clean(&n.found().label))).collect();
            let first = match labels.as_slice() {
                [] => String::new(),
                [one] => format!("{one} · "),
                [one, rest @ ..] => format!("{one} and {} more · ", rest.len()),
            };
            Request::new("tests", if failed == 1 { "1 test failed".to_string() } else { format!("{failed} tests failed") })
                .body(format!("{first}{} passed", end.passed))
                .urgency(Urgency::Normal)
                .action("Show the failures", "tests.open")
                .action("Run the failed again", "tests.run_failed")
        } else {
            Request::new("tests", "Tests passed").body(format!("{} passed {}", end.passed, took_text(took))).urgency(Urgency::Low)
        };
        self.notify(request.tag("tests"));
    }
}
