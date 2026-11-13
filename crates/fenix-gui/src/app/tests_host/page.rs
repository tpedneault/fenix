//! The tests page's host (plan `docs/plans/test-explorer.md`, step 4):
//! `tests.open` (`SPC T T`) and the page's actions -- runs, the stop, a
//! listing, a test's line, a failure copied, *Test output* beside it --
//! and the modeline's item (§10.1). The page itself is
//! `crate::tests_page`, laid out from the project's model.
//!
//! **What a frame costs.** The head and the modeline's item are counted
//! from `Model::summary` once a revision of the model, and at most ten
//! times a second while it changes (`tests_views_due`, from `plan_wake`,
//! which is also what lays the page out again then); a frame reads what
//! was counted. The page's rows are worked out when the revision, the
//! tab, the filter or what's open changes, never per frame (§7.5).
//!
//! **Off.** With `tests.enabled` off there's no page: `tests.open`
//! says the explorer is off, `SPC T` isn't offered, the palette doesn't
//! list the explorer's commands, the modeline has no item, and a page
//! left open when it's turned off is closed. Nothing here runs per
//! frame then but a look at one setting.

use super::*;
use crate::tests_page::{Action as PageAction, TestsPage, View};

/// How often the head and the modeline's item are counted again while
/// the model changes, and the page laid out again (§7.5).
const VIEW_EVERY: Duration = Duration::from_millis(100);

/// The head and the modeline's item as counted at a revision.
pub(crate) struct Shown {
    revision: u64,
    at: Instant,
    pub summary: fenix_tests::Summary,
}

/// `14:02`, local time, from ms since 1970.
pub(crate) fn clock(ms: u64) -> String {
    use chrono::TimeZone;
    chrono::Local.timestamp_millis_opt(ms as i64).single().map_or_else(String::new, |t| t.format("%H:%M").to_string())
}

impl App {
    /// The project the explorer's commands are about: the focused file's,
    /// else the project you're in (a page, Home). With the explorer on and
    /// the project not remote; else why not, said.
    pub(super) fn tests_page_root(&mut self) -> Option<PathBuf> {
        if !self.tests_enabled() {
            self.set_error("the test explorer is off (tests.enabled)");
            return None;
        }
        let path = self.open().buffer.path().map(Path::to_path_buf);
        let focused = self.focused_buffer_id();
        // A tests page's project, or a *Test output*'s.
        let here = match self.pages.get(&focused).map(|s| &s.model) {
            Some(PageModel::Tests(p)) => Some(p.root.clone()),
            _ => self.tests.projects.iter().find(|(_, p)| p.output == Some(focused)).map(|(r, _)| r.clone()),
        };
        let root = match here.or_else(|| path.as_deref().and_then(|p| self.tests_root_of(p))) {
            Some(root) => root,
            None => match self.project_root.clone().or_else(|| self.last_project.clone()) {
                Some(raw) => {
                    let root = self.tests_canon(&raw);
                    self.tests_alias(&root, &raw);
                    root
                }
                None => {
                    self.set_error("no project here, so no tests");
                    return None;
                }
            },
        };
        if fenix_vfs::is_remote(&root) {
            self.set_error("running tests isn't available on a remote project yet");
            return None;
        }
        self.tests_project(&root);
        Some(root)
    }

    /// `tests.open` (`SPC T T`): the project's tests page, in the focused
    /// pane -- the one already open, else as it was when last closed this
    /// session, else new. The first time in a session, its providers list
    /// their tests (§7.1).
    pub(crate) fn tests_open(&mut self) {
        let Some(root) = self.tests_page_root() else { return };
        if let Some(id) = self.find_page(|m| matches!(m, PageModel::Tests(p) if p.root == root)) {
            self.show_page(id);
            return;
        }
        let name = root.file_name().map_or_else(|| root.to_string_lossy().into_owned(), |n| n.to_string_lossy().into_owned());
        let page = self.tests.views.remove(&root).unwrap_or_else(|| TestsPage::new(root.clone(), name));
        self.tests.page_shown = true;
        self.open_page(PageModel::Tests(Box::new(page)));
        self.tests_page_opened(&root);
    }

    /// A tests page about to close: kept, for `SPC T T` to open again as it
    /// was (§7.6).
    pub(in crate::app) fn tests_page_closing(&mut self, id: BufferId) {
        if !matches!(self.pages.get(&id).map(|s| &s.model), Some(PageModel::Tests(_))) {
            return;
        }
        if let Some(PageState { model: PageModel::Tests(page), .. }) = self.pages.remove(&id) {
            self.tests.views.insert(page.root.clone(), *page);
        }
    }

    /// The head, counted again for the project at `root` if its model has
    /// changed since: as the page is laid out.
    pub(in crate::app) fn tests_shown(&mut self, root: &Path) {
        let Some(project) = self.tests.projects.get_mut(root) else { return };
        let revision = project.model.revision();
        if project.shown.as_ref().is_none_or(|s| s.revision != revision) {
            project.shown = Some(Shown { revision, at: Instant::now(), summary: project.model.summary() });
        }
    }

    /// What the head says besides the run (§7.2, §12): a project that
    /// isn't trusted, a first scan stopped at its cap, a provider that
    /// can't run here. Worked out as the page is laid out.
    pub(in crate::app) fn tests_page_notes(&mut self, root: &Path) -> Vec<(String, crate::page::Role)> {
        use crate::page::Role;
        let mut notes = Vec::new();
        let listing = self.config.bool_value("tests.list_automatically") != Some(false);
        let has_providers = self.tests.projects.get(root).is_some_and(|p| !p.model.providers().is_empty());
        if has_providers && listing && !self.tests_trusted(root) {
            notes.push(("not listed on their own in a project that isn't trusted (SPC p y): u lists them, running the project's runner".to_string(), Role::Warn));
        }
        let Some(project) = self.tests.projects.get(root) else { return notes };
        let mut scans: Vec<(&ProviderId, &(usize, usize))> = project.scanned_of.iter().filter(|(_, (done, of))| done < of).collect();
        scans.sort();
        for (provider, (done, of)) in scans {
            notes.push((format!("{}: {} of {} files scanned; the others' tests are found when they're opened", provider.provider, grouped(*done), grouped(*of)), Role::Muted));
        }
        for provider in project.model.providers() {
            if let Some(problem) = provider.problem {
                notes.push((format!("{}: {} -- SPC p h for the doctor", crate::tests_page::clean(provider.name), crate::tests_page::clean(problem)), Role::Warn));
            }
        }
        notes
    }

    /// Lays tests page `page` out at `cols` and `rows`, from its
    /// project's model.
    /// (An associated function: the page is borrowed from `App::pages`
    /// while the model is read from `App::tests`.)
    pub(in crate::app) fn tests_page_layout_with(tests: &TestsHost, page: &mut TestsPage, notes: Vec<(String, crate::page::Role)>, cols: usize, rows: Option<usize>) -> crate::page::Page {
        let Some(project) = tests.projects.get(&page.root) else { return crate::page::Page::default() };
        let counted;
        let summary = match &project.shown {
            Some(shown) => &shown.summary,
            None => {
                counted = project.model.summary();
                &counted
            }
        };
        crate::tests_page::layout(page, &View { model: &project.model, summary, notes, clock: &clock }, cols, rows)
    }

    /// Once a wake: each project whose model changed has its head counted
    /// again, and its tests page marked to be laid out -- at most every
    /// `VIEW_EVERY`. Whether a frame's wanted, and when to look again.
    pub(in crate::app) fn tests_views_due(&mut self, now: Instant) -> (bool, Option<Instant>) {
        if self.tests.projects.is_empty() && !self.tests.page_shown {
            return (false, None);
        }
        if !self.tests_enabled() {
            if self.tests.page_shown {
                self.tests_pages_off();
                return (true, None);
            }
            return (false, None);
        }
        let mut changed: Vec<PathBuf> = Vec::new();
        let mut due: Option<Instant> = None;
        for (root, project) in self.tests.projects.iter_mut() {
            let revision = project.model.revision();
            match &project.shown {
                Some(shown) if shown.revision == revision => continue,
                Some(shown) if now < shown.at + VIEW_EVERY => {
                    let at = shown.at + VIEW_EVERY;
                    due = Some(due.map_or(at, |d| d.min(at)));
                    continue;
                }
                _ => {}
            }
            project.shown = Some(Shown { revision, at: now, summary: project.model.summary() });
            changed.push(root.clone());
        }
        if changed.is_empty() {
            return (false, due);
        }
        if self.tests.page_shown {
            for state in self.pages.values_mut() {
                if matches!(&state.model, PageModel::Tests(p) if changed.contains(&p.root)) {
                    state.stale = true;
                }
            }
        }
        (true, due)
    }

    /// The explorer turned off: its pages close, and what they kept goes.
    fn tests_pages_off(&mut self) {
        self.tests.page_shown = false;
        let ids: Vec<BufferId> = self.pages.iter().filter(|(_, s)| matches!(s.model, PageModel::Tests(_))).map(|(id, _)| *id).collect();
        for id in ids {
            self.close_page_quietly(id);
        }
        self.tests.views.clear();
    }

    /// The modeline's item (§10.1): `tests 12/40 ✗ 1` while a run of the
    /// project you're in goes, `✗ 3 tests` while some have failed, nothing
    /// when all passed. Read from what was counted: no work per frame.
    pub(crate) fn tests_segment(&self) -> String {
        if self.tests.projects.is_empty() || !self.tests_enabled() {
            return String::new();
        }
        // The project of the tests page you're on, else the one you're in.
        let on_page = match self.pages.get(&self.focused_buffer_id()).map(|s| &s.model) {
            Some(PageModel::Tests(p)) => Some(&p.root),
            _ => None,
        };
        let Some(raw) = on_page.or(self.project_root.as_ref()) else { return String::new() };
        let root = self.tests.canon.get(raw).unwrap_or(raw);
        let Some(shown) = self.tests.projects.get(root).and_then(|p| p.shown.as_ref()) else { return String::new() };
        let summary = &shown.summary;
        match &summary.running {
            Some(run) if run.tests => {
                let failed = if run.failed > 0 { format!(" ✗ {}", run.failed) } else { String::new() };
                format!("   tests {}/{}{failed}", run.done, run.total)
            }
            _ => match summary.counts.failed + summary.counts.errored {
                0 => String::new(),
                1 => "   ✗ 1 test".to_string(),
                n => format!("   ✗ {n} tests"),
            },
        }
    }

    /// What a key on tests page `id` asked for, done.
    pub(in crate::app) fn tests_page_action(&mut self, id: BufferId, action: PageAction) {
        let Some(root) = self.pages.get(&id).and_then(|s| match &s.model {
            PageModel::Tests(p) => Some(p.root.clone()),
            _ => None,
        }) else {
            return;
        };
        let runs = matches!(action, PageAction::Run(_) | PageAction::RunAll | PageAction::RunFailed | PageAction::RunLast | PageAction::List);
        if runs && !self.tests_enabled() {
            self.set_error("the test explorer is off (tests.enabled)");
            return;
        }
        if runs && fenix_vfs::is_remote(&root) {
            self.set_error("running tests isn't available on a remote project yet");
            return;
        }
        match action {
            PageAction::None => {}
            PageAction::Close => return self.close_page(id),
            PageAction::Say(text) => self.set_message(text),
            PageAction::Run(item) => {
                let Some(project) = self.tests.projects.get(&root) else { return };
                let model = &project.model;
                let (keys, what) = match item {
                    fenix_tests::Item::Node(node) => match model.node(node) {
                        Some(n) => (vec![n.key()], crate::tests_page::clean(&n.found().label)),
                        None => return,
                    },
                    fenix_tests::Item::Provider(p) => {
                        let keys = model.roots(p).iter().filter_map(|&r| model.node(r)).map(|n| n.key()).collect();
                        (keys, model.provider(p).map(|v| crate::tests_page::clean(v.name)).unwrap_or_default())
                    }
                };
                self.tests_request(&root, vec![Selection::Nodes(keys)], &what);
            }
            PageAction::RunAll => self.tests_request(&root, vec![Selection::All], "all the tests"),
            PageAction::RunFailed => self.tests_request(&root, vec![Selection::Failed], "the failed"),
            PageAction::RunLast => match self.tests.projects.get(&root).and_then(|p| p.model.last_selection()).map(<[Selection]>::to_vec) {
                Some(last) => self.tests_request(&root, last, "the last run"),
                None => self.set_error("no test run yet in this project"),
            },
            PageAction::Stop => {
                if self.tests_stop_in(&root, "stopped") {
                    self.set_message("stopping the test run");
                } else {
                    self.set_message("no test run to stop");
                }
            }
            PageAction::List => {
                self.tests_list(&root);
                self.set_message("listing the tests");
            }
            PageAction::Output => self.tests_output_beside(&root, Some(id)),
            PageAction::GoTo(node) => self.tests_go_to(id, &root, node),
            PageAction::Copy(node) => self.tests_copy_failure(&root, node),
            // Debugging a test is step 6's (`tests.debug_here`, §11).
            PageAction::Debug(_) => self.set_message("debugging a test from the page isn't in Fenix yet (tests.debug_here)"),
        }
        self.restale_page(id);
    }

    /// The pane beside page `page` focused: another pane, else one split
    /// off it.
    fn tests_beside(&mut self, page: BufferId) {
        let page_pane = self.pane_showing(page).unwrap_or_else(|| self.focused_pane_id());
        match self.windows().windows().into_iter().find(|&p| p != page_pane) {
            Some(pane) => {
                self.windows_mut().focus(pane);
            }
            None => {
                self.windows_mut().focus(page_pane);
                self.split_vertical();
            }
        }
        self.sidebar_focused = false;
        self.main_view = MainView::Editor;
    }

    /// `o`, `SPC T o`: the project at `root`'s *Test output*, beside tests
    /// page `page` when it's shown (§8.5), else in the focused pane; the
    /// keyboard with it, so `Enter` on a place goes there.
    pub(in crate::app) fn tests_output_beside(&mut self, root: &Path, page: Option<BufferId>) {
        let Some(buffer) = self.tests.projects.get(root).and_then(|p| p.output).filter(|b| self.buffers.get(*b).is_some()) else {
            return self.set_message("no test output yet in this project");
        };
        if let Some(pane) = self.pane_showing(buffer) {
            self.windows_mut().focus(pane);
            self.wake_caret();
            return;
        }
        if let Some(page) = page.filter(|p| self.pane_showing(*p).is_some()) {
            self.tests_beside(page);
        }
        self.open_buffer_in_focused_pane(buffer);
        self.wake_caret();
    }

    /// `Enter` on a test: its file at its line, beside the page, the
    /// keyboard with it.
    fn tests_go_to(&mut self, page: BufferId, root: &Path, node: fenix_tests::NodeId) {
        let Some(project) = self.tests.projects.get(root) else { return };
        let Some(n) = project.model.node(node) else { return };
        let label = crate::tests_page::clean(&n.found().label);
        let (file, line) = (n.found().path.clone().or_else(|| n.file().map(str::to_string)), n.found().line);
        let Some(file) = file else {
            return self.set_error(format!("{label}: no file is known for it"));
        };
        let Some(line) = line else {
            return self.set_error(format!("{label} was found in a run: no line is known for it"));
        };
        let path = if Path::new(&file).is_absolute() { PathBuf::from(&file) } else { root.join(&file) };
        if !path.is_file() {
            return self.set_error(format!("{} isn't a file", readable_path(&path)));
        }
        self.tests_beside(page);
        let from = JumpEntry { buffer: self.focused_buffer_id(), char_idx: self.cursor().char_idx };
        self.open_file_as_preview(&path);
        self.jump_to_grep_match(&fenix_project::GrepMatch { path, line: line as usize + 1, col: 1, text: String::new() });
        self.record_jump(from);
        self.wake_caret();
    }

    /// `y` on a failed test: its name, place and message, on the clipboard.
    fn tests_copy_failure(&mut self, root: &Path, node: fenix_tests::NodeId) {
        let Some(n) = self.tests.projects.get(root).and_then(|p| p.model.node(node)) else { return };
        let label = crate::tests_page::clean(&n.found().label);
        let mut text = label.clone();
        let result = n.result();
        let place = result.and_then(|r| r.at.as_ref()).map(|at| (at.path.clone(), at.line)).or_else(|| n.found().path.clone().zip(n.found().line));
        if let Some((path, line)) = place {
            text.push_str(&format!("\n{path}:{}", line + 1));
        }
        if let Some(message) = result.and_then(|r| r.message.as_deref()) {
            text.push('\n');
            text.push_str(&crate::tests_page::without_escapes(message));
        }
        if let Some(clipboard) = &mut self.clipboard {
            let _ = clipboard.set_text(text.clone());
        }
        #[cfg(test)]
        {
            self.tests.copied = Some(text);
        }
        self.set_message(format!("copied {label}'s failure"));
    }

    /// What a place on line `line` of *Test output* buffer `buffer` is
    /// relative to: the folder of the step whose lines it's among, else
    /// the project. None: it's no project's *Test output*.
    pub(in crate::app) fn tests_output_base(&self, buffer: BufferId, line: usize) -> Option<PathBuf> {
        let (root, project) = self.tests.projects.iter().find(|(_, p)| p.output == Some(buffer))?;
        // Past 16 MiB the first line is the note of what was dropped.
        let at = if project.output_dropped > 0 {
            if line == 0 {
                return Some(root.clone());
            }
            line as u64 - 1 + project.output_dropped
        } else {
            line as u64
        };
        let step = project.output_steps.iter().rev().find(|(start, _)| *start <= at);
        Some(step.map_or_else(|| root.clone(), |(_, cwd)| cwd.clone()))
    }
}
