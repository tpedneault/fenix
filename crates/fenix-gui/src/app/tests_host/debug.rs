//! Debugging a test (plan `docs/plans/test-explorer.md` §11, step 6):
//! `tests.debug_here` (`SPC T d`), `d` on the tests page and *Debug test*
//! in the right click's menu.
//!
//! Fenix draws a run id (`Model::run_id`) and asks the test's provider
//! `tests-launch(provider, run, test)`. A `launch` is started at once; a
//! `build-first` plan runs as that run's steps (`Model::request_build`,
//! its output in *Test output*, read by the provider), and when every
//! step exited 0 with nothing `broken` or `crashed`, `tests-launch` is
//! asked again with the same run, once. The launch it gives is started
//! through `debug_start_from` as an extension's launch is -- its
//! variables filled in, its `preLaunchTask` first -- with no picker of
//! launches and without being remembered as the project's last launch
//! (`Template::test`), the test's file open at its line.
//!
//! **One at a time per project**: while a test is being got ready to
//! debug (`Debugging`: asked, its build waiting or running, asked again),
//! another is refused, saying which; `SPC T k` lets it go. A build waits
//! for a run going, and goes before a run waiting (the model's rule).
//!
//! **What stops it**: the provider's refusal (said; nothing else stopped),
//! a build that didn't build (said on the modeline and the page's head,
//! the first `broken`'s message or the step's end, `o` shows the rest), a
//! second `build-first` ("it asked to build twice"), a stop, the
//! provider's extension stopping, the explorer turned off. An extension
//! with no `tests-launch` export is said once on its output page, and its
//! tests are never asked about again while Fenix runs: they still run.

use super::*;
use fenix_wasm::host::Launching;

/// A test being got ready to debug in its project.
pub(crate) struct Debugging {
    /// The run its first `tests-launch` was asked with: its build runs as
    /// it, and the second ask carries it.
    pub run: RunId,
    pub provider: ProviderId,
    pub id: String,
    pub node: fenix_tests::NodeId,
    pub label: String,
    /// The tests page it was asked from: its file opens beside it.
    page: Option<BufferId>,
    /// The `tests-launch` out, if one is.
    pub asking: Option<u64>,
    /// Asked again, after its build.
    second: bool,
    /// Its `build-first` plan's steps as the provider gave them, until its
    /// run's first step takes them.
    steps: Option<Vec<PlannedStep>>,
    /// Its build is waiting or running.
    pub building: bool,
    /// How a step of its build ended, when not with 0: what's said when no
    /// `broken` says more.
    step_end: Option<String>,
}

impl Debugging {
    /// A step of its build ended.
    pub(super) fn step_ended(&mut self, end: &fenix_tests::StepEnd) {
        let said = match end {
            fenix_tests::StepEnd::Exited(Some(0)) => return,
            fenix_tests::StepEnd::Exited(Some(code)) => format!("its build exited {code}"),
            fenix_tests::StepEnd::Exited(None) => "its build ended by a signal".to_string(),
            fenix_tests::StepEnd::Stopped => "its build was stopped".to_string(),
            fenix_tests::StepEnd::TimedOut => "its build timed out".to_string(),
            fenix_tests::StepEnd::Failed(why) => format!("its build couldn't run: {why}"),
        };
        self.step_end.get_or_insert(said);
    }
}

impl App {
    /// `tests.debug_here` (`SPC T d`, *Debug test*): the test the cursor
    /// is in (§9.4).
    pub(crate) fn tests_debug_here(&mut self) {
        let Some((root, rel)) = self.tests_here() else { return };
        let line = self.open().buffer.line_col(&self.cursor()).0 as u32;
        let Some(node) = self.tests.projects[&root].model.test_at(&rel, line) else {
            let name = Path::new(&rel).file_name().map_or(rel.clone(), |n| n.to_string_lossy().into_owned());
            return self.set_error(format!("line {} of {name} isn't in a test", line + 1));
        };
        let _ = self.tests_debug(&root, node, None);
    }

    /// Debugs test `node` of the project at `root`, from tests page `page`
    /// or the file: its provider asked how (§11). The run it's asked as,
    /// or why it isn't (said on the modeline too), for a door or an agent
    /// to act on (step 7); how it ends comes to `tests_debug_done`.
    pub(in crate::app) fn tests_debug(&mut self, root: &Path, node: fenix_tests::NodeId, page: Option<BufferId>) -> Result<RunId, String> {
        let refused = |app: &mut App, why: String| {
            app.set_error(why.clone());
            Err(why)
        };
        if !self.tests_enabled() {
            return refused(self, "the test explorer is off (tests.enabled)".into());
        }
        if fenix_vfs::is_remote(root) {
            return refused(self, "debugging a test isn't available on a remote project yet".into());
        }
        let project = self.tests_project(root);
        let Some(n) = project.model.node(node) else { return refused(self, "that test is gone from its provider's tests".into()) };
        let label = crate::tests_page::clean(&n.found().label);
        if !n.found().kind.is_test() {
            return refused(self, format!("{label} isn't a test or a case: only one test is debugged at a time"));
        }
        // §19, finding 2: a cut id with no `data` can't be named to it.
        if !n.can_be_named() {
            return refused(self, format!("{label} can't be debugged: its id is over 1 KiB and its provider's data for it isn't kept"));
        }
        let key = n.key();
        // As Fenix has it, its path `/project/<path>` (§19, finding 2).
        let found = api_found(n.given_back());
        if let Some(busy) = project.debug.as_ref().map(|b| format!("already {} {} -- SPC T k lets it go", if b.building { "building" } else { "asking how to debug" }, b.label)) {
            return refused(self, busy);
        }
        if self.debug_session.as_ref().is_some_and(|s| !s.ended) {
            return refused(self, "a debug session is already running: stop it first".into());
        }
        let ext = Self::tests_ext(&key.provider);
        if self.tests.no_launch.contains(&ext) {
            return refused(self, format!("{label} can't be debugged: {} has no tests-launch (said on its output page)", key.provider.provider));
        }
        if let Reach::No(why) = self.tests_reach(&ext, root, true) {
            return refused(self, format!("{label} can't be debugged now: {why}"));
        }
        let project = self.tests.projects.get_mut(root).expect("a project");
        let run = project.model.run_id();
        let call = Call::TestsLaunch { provider: key.provider.provider.to_string(), run: run.0, test: found };
        let token = self.tests_send(&ext, call, CallFor::Launch { root: root.to_path_buf(), run });
        let project = self.tests.projects.get_mut(root).expect("a project");
        project.debug = Some(Debugging { run, provider: key.provider.clone(), id: key.id.clone(), node, label: label.clone(), page, asking: Some(token), second: false, steps: None, building: false, step_end: None });
        project.debug_said = None;
        self.set_message(format!("asking {} how to debug {label}", key.provider.provider));
        Ok(run)
    }

    /// `tests-launch`'s answer for run `run` of the project at `root`.
    pub(super) fn tests_launched(&mut self, root: &Path, run: RunId, token: u64, launched: Result<Launching, String>) {
        let enabled = self.tests_enabled();
        let Some(project) = self.tests.projects.get_mut(root) else { return };
        // Let go meanwhile (a stop, the explorer off): nothing to do.
        let Some(debugging) = project.debug.as_mut().filter(|d| d.run == run && d.asking == Some(token)) else { return };
        debugging.asking = None;
        if !enabled {
            project.debug = None;
            return self.tests_debug_done(root, Err("the test explorer is off (tests.enabled)".into()));
        }
        match launched {
            Ok(Launching::Launch(config)) => {
                let debugging = project.debug.take().expect("asked");
                project.debug_said = None;
                self.tests_debug_start(root, debugging, config);
            }
            Ok(Launching::BuildFirst(_)) if debugging.second => {
                let debugging = project.debug.take().expect("asked");
                self.tests_debug_ended(root, format!("{} can't be debugged: {} asked to build it twice", debugging.label, debugging.provider.provider));
            }
            Ok(Launching::BuildFirst(planned)) => {
                let plan = fenix_tests::Plan { steps: planned.steps.iter().map(|s| fenix_tests::Step { label: s.step.label.clone(), covers: s.step.covers.clone() }).collect(), list_after: planned.list_after };
                let (provider, id) = (debugging.provider.clone(), debugging.id.clone());
                debugging.steps = Some(planned.steps);
                debugging.building = true;
                let label = debugging.label.clone();
                match project.model.request_build(run, &provider, &id, plan) {
                    fenix_tests::Requested::Nothing => {
                        project.debug = None;
                        return self.tests_debug_ended(root, format!("{label} can't be debugged: {} isn't this project's now", provider.provider));
                    }
                    fenix_tests::Requested::Waits => {
                        project.debug_said = Some((format!("debug {label}: its build waits for the run going"), crate::page::Role::Accent));
                        self.set_message(format!("{label}: its build to debug it waits for the run going"));
                    }
                    _ => {
                        project.debug_said = Some((format!("building {label} to debug it -- o shows it, x stops it"), crate::page::Role::Accent));
                        self.set_message(format!("building {label} to debug it"));
                    }
                }
                self.tests_pump();
            }
            // Its refusal: said, and nothing else stopped.
            Err(why) => {
                let debugging = project.debug.take().expect("asked");
                self.tests_debug_ended(root, format!("{} can't be debugged: {}", debugging.label, crate::tests_page::clean(&why)));
            }
        }
    }

    /// A `tests-launch` call that failed: a trap or a deadline (the
    /// extension's stop comes as its own event), or an extension with no
    /// such export (`tests_export_missing`), which isn't asked again.
    pub(super) fn tests_launch_failed(&mut self, root: &Path, run: RunId, why: &str) {
        let label = self.tests.projects.get_mut(root).and_then(|p| p.debug.take_if(|d| d.run == run)).map(|d| d.label);
        if let Some(label) = label {
            self.tests_debug_ended(root, format!("{label} can't be debugged: {}", crate::tests_page::clean(why)));
        }
    }

    /// The run's first step: a debug build's steps, as its provider gave
    /// them, are the plan under way.
    pub(super) fn tests_debug_plan(&mut self, root: &Path, run: RunId) {
        let Some(project) = self.tests.projects.get_mut(root) else { return };
        if project.plan.as_ref().is_some_and(|(r, _, _)| *r == run) {
            return;
        }
        if let Some(debugging) = project.debug.as_mut().filter(|d| d.run == run) {
            if let Some(steps) = debugging.steps.take() {
                project.plan = Some((run, debugging.provider.clone(), steps));
            }
        }
    }

    /// A debug build's run ended (`RunKind::Build`): asked again when it
    /// built, else why not said.
    pub(super) fn tests_built(&mut self, root: &Path, end: &fenix_tests::RunEnd) {
        let Some(project) = self.tests.projects.get_mut(root) else { return };
        let Some(debugging) = project.debug.as_mut().filter(|d| d.run == end.run) else { return };
        debugging.building = false;
        let label = debugging.label.clone();
        match &end.halted {
            Some(fenix_tests::Halt::Stopped(why)) => {
                project.debug = None;
                return self.tests_debug_ended(root, format!("{label} isn't debugged: its build was stopped ({})", crate::tests_page::clean(why)));
            }
            Some(fenix_tests::Halt::TimedOut) => {
                project.debug = None;
                return self.tests_debug_ended(root, format!("{label} isn't debugged: its build timed out (tests.timeout_minutes)"));
            }
            None => {}
        }
        if !end.build_ok {
            let why = match (&end.first_broken, &debugging.step_end) {
                (Some(broken), _) => crate::tests_page::clean(broken),
                (None, Some(step)) => format!("{label} didn't build: {step}"),
                (None, None) => format!("{label} didn't build"),
            };
            project.debug = None;
            let said = format!("{why} · o shows it");
            project.debug_said = Some((said.clone(), crate::page::Role::Bad));
            self.set_error(said.clone());
            return self.tests_debug_done(root, Err(said));
        }
        // Built: asked again, once, with the same run (§11).
        let (provider, node) = (debugging.provider.clone(), debugging.node);
        let ext = Self::tests_ext(&provider);
        let found = project.model.node(node).map(|n| api_found(n.given_back()));
        let Some(found) = found else {
            project.debug = None;
            return self.tests_debug_ended(root, format!("{label} isn't debugged: it's gone from its provider's tests"));
        };
        if let Reach::No(why) = self.tests_reach(&ext, root, true) {
            let project = self.tests.projects.get_mut(root).expect("a project");
            project.debug = None;
            return self.tests_debug_ended(root, format!("{label} isn't debugged: {why}"));
        }
        let call = Call::TestsLaunch { provider: provider.provider.to_string(), run: end.run.0, test: found };
        let token = self.tests_send(&ext, call, CallFor::Launch { root: root.to_path_buf(), run: end.run });
        let project = self.tests.projects.get_mut(root).expect("a project");
        if let Some(debugging) = project.debug.as_mut() {
            debugging.asking = Some(token);
            debugging.second = true;
        }
        project.debug_said = Some((format!("{label} built: asking {} how to debug it", provider.provider), crate::page::Role::Accent));
    }

    /// The launch a provider gave, started: the test's file open at its
    /// line (from the page, beside it), then `debug_start_from` -- its
    /// variables, its `preLaunchTask`, the session -- with no picker of
    /// launches and not remembered as the project's last.
    fn tests_debug_start(&mut self, root: &Path, debugging: Debugging, config: fenix_wasm::host::LaunchConfig) {
        let mut template = match crate::dap::Template::from_config(config) {
            Ok(template) => template,
            Err(why) => return self.tests_debug_ended(root, format!("{} can't be debugged: its launch: {why}", debugging.label)),
        };
        template.test = true;
        // From the file: it stays as it is when it's the test's.
        let here = self.open().buffer.path().map(Path::to_path_buf).and_then(|p| self.tests.by_path.get(&p).cloned());
        let file = self.tests.projects.get(root).and_then(|p| p.model.node(debugging.node)).and_then(|n| n.file().map(str::to_string));
        let in_it = debugging.page.is_none() && here.is_some_and(|(r, rel)| r == root && Some(rel) == file);
        if !in_it {
            let page = debugging.page.filter(|p| self.pane_showing(*p).is_some());
            // No file to open: the session starts from what's focused.
            let _ = self.tests_open_test(root, debugging.node, page);
        }
        self.set_message(format!("debugging {}", debugging.label));
        self.debug_start_from(template);
        // Started, or waiting on its `preLaunchTask` in the task pane (the
        // session starts when that ends: an agent waits on, as
        // `debug_start`'s does), or why not as `debug_start_from` said it
        // (step 7's review, S3).
        let outcome = debug_outcome(self.debug_session.as_ref().is_some_and(|s| !s.ended), self.debug_after_task.is_some(), || self.modeline_pieces().1);
        self.tests_debug_done(root, outcome);
    }

    /// The debug asked for ended without a session: `why`, on the
    /// modeline and the page's head.
    fn tests_debug_ended(&mut self, root: &Path, why: String) {
        if let Some(project) = self.tests.projects.get_mut(root) {
            project.debug_said = Some((why.clone(), crate::page::Role::Bad));
        }
        self.set_error(why.clone());
        self.tests_debug_done(root, Err(why));
    }

    /// A debug of the project at `root` ended: its session started, or
    /// why none did. An agent waiting for it hears (`debug_test`).
    pub(super) fn tests_debug_done(&mut self, root: &Path, outcome: Result<(), String>) {
        self.tests_agents_debug_done(root, outcome);
    }

    /// A stop (`SPC T k`, `x`): a debug waiting for its provider's answer
    /// or for its build's turn is let go (a build under way is stopped as
    /// the run going, and ends so). The test let go, if one was.
    pub(super) fn tests_debug_let_go(&mut self, root: &Path, why: &str) -> Option<String> {
        let project = self.tests.projects.get_mut(root)?;
        let going = project.model.summary().running.is_some_and(|r| project.debug.as_ref().is_some_and(|d| d.run == r.run));
        if going || project.debug.is_none() {
            return None;
        }
        let debugging = project.debug.take().expect("one");
        project.model.drop_build();
        let said = format!("{} isn't debugged: {why}", debugging.label);
        project.debug_said = Some((said.clone(), crate::page::Role::Muted));
        self.tests_debug_done(root, Err(said));
        Some(debugging.label)
    }

    /// The providers of `ext` stopped in the project at `root`: a debug of
    /// one of theirs goes with them (its build, if it ran, ends stopped).
    pub(super) fn tests_debug_provider_stopped(&mut self, root: &Path, ext: &str, why: &str) {
        let Some(project) = self.tests.projects.get_mut(root) else { return };
        let Some(debugging) = project.debug.take_if(|d| d.provider.extension == ext) else { return };
        project.model.drop_build();
        self.tests_debug_ended(root, format!("{} isn't debugged: {}", debugging.label, crate::tests_page::clean(why)));
    }
}

/// How a launch handed to `debug_start_from` went, for whoever waits on
/// it: a session running, or one waiting on its `preLaunchTask`, is under
/// way; else it didn't start, `said` saying why.
pub(super) fn debug_outcome(running: bool, task_pending: bool, said: impl FnOnce() -> String) -> Result<(), String> {
    if running || task_pending {
        Ok(())
    } else {
        Err(said())
    }
}
