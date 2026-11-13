//! The tests in the file (plan `docs/plans/test-explorer.md` §9, step 5):
//! the gutter's test column and its marks, a result after a test's line,
//! failures as problems under `fenix:#tests`, the right click's items and
//! a click on a mark. All of it from the project's `fenix_tests::Model`.
//!
//! **What a frame costs.** The column's width is a look-up of the file's
//! path (`TestsHost::by_path`, kept as buffers are seen, never worked out
//! as a frame is drawn) and of the model's index of files; its marks are
//! the model's `marks` for the lines drawn, a search into the file's
//! tests by line and then those lines' tests only. Problems and the
//! results after the lines are worked out when the model changes, at
//! most ten times a second (`tests_views_due`), and the results only for
//! the lines a pane shows and `MARGIN` around them; a frame draws what
//! they set, as any decoration.
//!
//! **Off.** With `tests.enabled` off there's no column, no width, no
//! marks, no results after the lines, no problems and no menu items:
//! what was shown is taken away at the next wake, and a frame costs a
//! look at an empty map, or at the setting.

use super::*;
use crate::decorations::{Decoration, Look};
use crate::page::Role;
use fenix_tests::State;
use std::ops::RangeInclusive;

/// Whose decorations the results after the lines are (§9.2), among an
/// extension's, the debugger's and the inlay hints'.
pub(in crate::app) const TESTS_OWNER: &str = "fenix.tests";
/// Whose problems failures are (§9.3): beside a language server's and an
/// extension's `<ext>:#problems`, never in their place.
pub(in crate::app) const PROBLEMS_KEY: &str = "fenix:#tests";
/// A failure's message after its line is cut at this many characters.
const MESSAGE_CHARS: usize = 48;
/// The lines above and below a pane's whose results are set after them.
const MARGIN: u32 = 100;

/// How results are shown after a test's line (`tests.inline`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum InlineMode {
    All,
    Failed,
    Off,
}

/// The settings what's shown in the files was worked out under.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::app) struct Settings {
    problems: bool,
    inline: InlineMode,
}

/// A buffer's results after its lines, as last set.
pub(in crate::app) struct Inline {
    /// The lines they were worked out for: none while no pane shows it.
    window: Option<RangeInclusive<u32>>,
    /// What was set: each line's text and role.
    shown: Vec<(u32, String, Role)>,
}

/// The test column's marks for the lines drawn: by line, its glyph, its
/// colour and what a click on it runs.
pub(in crate::app) struct TestCells {
    cells: Vec<(usize, char, glyphon::Color, fenix_tests::NodeId)>,
}

impl TestCells {
    /// Line `line`'s mark, if it has one.
    pub fn at(&self, line: usize) -> Option<(char, glyphon::Color, fenix_tests::NodeId)> {
        let i = self.cells.binary_search_by_key(&line, |c| c.0).ok()?;
        let (_, glyph, color, node) = self.cells[i];
        Some((glyph, color, node))
    }
}

/// A failure's message as it's shown after its line and as a problem:
/// one row, escape codes out, cut at `MESSAGE_CHARS` (`…` its last).
pub(crate) fn short_message(message: &str) -> String {
    let clean = crate::tests_page::clean(message);
    if clean.chars().count() <= MESSAGE_CHARS {
        return clean;
    }
    let mut cut: String = clean.chars().take(MESSAGE_CHARS - 1).collect();
    cut.push('…');
    cut
}

impl App {
    fn tests_settings(&self) -> Settings {
        let inline = match self.config.text_value("tests.inline").unwrap_or("failed") {
            "all" => InlineMode::All,
            "off" => InlineMode::Off,
            _ => InlineMode::Failed,
        };
        Settings { problems: self.config.bool_value("tests.problems") != Some(false), inline }
    }

    /// The project of the open file at `path`, its path there and its
    /// tests, with the explorer on: a look-up, no file system call.
    pub(in crate::app) fn tests_file_of(&self, path: &Path) -> Option<(&PathBuf, &str, &ProjectTests)> {
        if self.tests.by_path.is_empty() || !self.tests_enabled() {
            return None;
        }
        let (root, rel) = self.tests.by_path.get(path)?;
        let project = self.tests.projects.get(root)?;
        Some((root, rel.as_str(), project))
    }

    /// Whether `ob`'s gutter has the test column (§9.1): a file a
    /// provider found tests in, with line numbers on (the column is one
    /// more cell of theirs, as the fold column is), the explorer on and
    /// `tests.gutter` not off. A file with no tests has none.
    pub(in crate::app) fn tests_column(&self, ob: &OpenBuffer) -> bool {
        if self.tests.by_path.is_empty() || ob.kind != BufferKind::Text || self.line_number_mode == LineNumberMode::Off {
            return false;
        }
        if self.config.bool_value("tests.gutter") == Some(false) {
            return false;
        }
        let Some(path) = ob.buffer.path() else { return false };
        self.tests_file_of(path).is_some_and(|(_, rel, project)| project.model.serves(rel))
    }

    /// The test column's marks for `lines` of `ob` (§9.1); none when it
    /// has no column. Costs the lines asked for, not the file.
    pub(in crate::app) fn tests_cells(&self, ob: &OpenBuffer, lines: RangeInclusive<usize>) -> Option<TestCells> {
        if !self.tests_column(ob) {
            return None;
        }
        let path = ob.buffer.path()?;
        let (_, rel, project) = self.tests_file_of(path)?;
        let low = u32::try_from(*lines.start()).unwrap_or(u32::MAX);
        let high = u32::try_from(*lines.end()).unwrap_or(u32::MAX);
        let marks = project.model.marks(rel, low..=high);
        let mut cells = Vec::with_capacity(marks.len());
        if marks.is_empty() {
            return Some(TestCells { cells });
        }
        // A breakpoint's square and the stop's bar are drawn over the
        // column's cell when no blame comes before it: on such a line the
        // breakpoint wins (§17's fallback), its mark not drawn under it.
        let covered = if self.blame_width(ob) == 0 { self.tests_debug_lines(path) } else { HashSet::new() };
        for mark in marks {
            let line = mark.line as usize;
            if covered.contains(&line) {
                continue;
            }
            let role = crate::tests_page::state_role(mark.state, mark.stale);
            cells.push((line, mark.state.glyph(), crate::app::pages::role_color(self.theme, role), mark.node));
        }
        Some(TestCells { cells })
    }

    /// The 0-based lines of the file at `path` the debugger marks: a
    /// breakpoint, the stop.
    fn tests_debug_lines(&self, path: &Path) -> HashSet<usize> {
        let stop = self.debug_stop_location();
        if self.breakpoints.values().all(|l| l.is_empty()) && stop.is_none() {
            return HashSet::new();
        }
        let same = crate::app::debug_host::same_file;
        let mut lines: HashSet<usize> = self.breakpoints.iter().filter(|(p, _)| same(p, path)).flat_map(|(_, lines)| lines.keys().map(|l| l.saturating_sub(1))).collect();
        if let Some((_, line)) = stop.filter(|(p, _)| same(p, path)) {
            lines.insert(line.saturating_sub(1));
        }
        lines
    }

    /// A click in the gutter of `pane`'s row `row` (from the top of what
    /// it shows), at cell `col`: on a test's mark, it runs that test or
    /// group (§9.1). True when it was one.
    pub(in crate::app) fn tests_mark_click(&mut self, pane: fenix_window::WindowId, row: usize, col: usize) -> bool {
        let Some(&buffer) = self.windows().content(pane) else { return false };
        let Some(ob) = self.buffers.get(buffer) else { return false };
        // The column's cell is the gutter's first after the blame.
        if !self.tests_column(ob) || col != self.blame_width(ob) {
            return false;
        }
        let state = *self.pane_state(pane);
        // A mark is on a line's first row only.
        let Some(line) = self.pane_rows(pane, buffer, state.scroll_line, state.scroll_row, row + 1).row(row).filter(|r| r.kind == crate::app::rows::RowKind::Text && r.first).map(|r| r.line) else { return false };
        let Some((_, _, node)) = self.buffers.get(buffer).and_then(|ob| self.tests_cells(ob, line..=line)).and_then(|c| c.at(line)) else { return false };
        let Some(root) = ob_root(self, buffer) else { return false };
        self.tests_run_node(&root, node);
        self.wake_caret();
        true
    }

    /// Runs node `node` of the project at `root`: a mark clicked.
    fn tests_run_node(&mut self, root: &Path, node: fenix_tests::NodeId) {
        if fenix_vfs::is_remote(root) {
            return self.set_error("running tests isn't available on a remote project yet");
        }
        let Some(n) = self.tests.projects.get(root).and_then(|p| p.model.node(node)) else { return };
        let (key, what) = (n.key(), crate::tests_page::clean(&n.found().label));
        self.tests_request(root, vec![Selection::Nodes(vec![key])], &what);
    }

    /// The test the cursor is in (§9.4), in the focused file, with its
    /// project: what the right click's items are about.
    pub(in crate::app) fn tests_test_here(&self) -> Option<(PathBuf, fenix_tests::NodeId)> {
        let ob = self.open();
        let (root, rel, project) = self.tests_file_of(ob.buffer.path()?)?;
        let line = ob.buffer.line_col(&self.cursor()).0 as u32;
        project.model.test_at(rel, line).map(|node| (root.clone(), node))
    }

    /// *Show in Tests*: the project's tests page, on the test the cursor
    /// is in, its groups opened.
    pub(in crate::app) fn tests_show_here(&mut self) {
        let Some((root, node)) = self.tests_test_here() else {
            return self.set_error("the cursor isn't in a test");
        };
        self.tests_open();
        let Some(id) = self.find_page(|m| matches!(m, PageModel::Tests(p) if p.root == root)) else { return };
        let Some(project) = self.tests.projects.get(&root) else { return };
        if let Some(PageState { model: PageModel::Tests(page), .. }) = self.pages.get_mut(&id) {
            page.show(&project.model, node);
        }
        self.restale_page(id);
    }

    /// The open file at `path` changed on disk and was read again: its
    /// results are older than it, as after a save (§4 "States").
    pub(in crate::app) fn tests_file_changed_on_disk(&mut self, path: &Path) {
        if !self.tests_enabled() {
            return;
        }
        let Some((root, rel)) = self.tests.by_path.get(path).cloned() else { return };
        if let Some(project) = self.tests.projects.get_mut(&root) {
            project.model.file_saved(&rel);
        }
    }

    // What's worked out when the model changes.

    /// Once a wake, with the projects whose model changed (counted at most
    /// ten times a second, `tests_views_due`): their failures published
    /// as problems, and the results after the lines of each open file of
    /// theirs that a pane shows; the same when a setting changed, a file
    /// was opened, or a pane moved past what was worked out. Whether
    /// anything shown changed.
    pub(in crate::app) fn tests_in_files(&mut self, changed: &[PathBuf]) -> bool {
        let settings = self.tests_settings();
        let settings_changed = self.tests.file_settings.as_ref() != Some(&settings);
        self.tests.file_settings = Some(settings.clone());
        let fresh: HashSet<PathBuf> = self.tests.seen.iter().filter(|(id, _)| !self.tests.inline.contains_key(id)).map(|(_, s)| s.root.clone()).collect();
        let mut drawn = false;
        let roots: Vec<PathBuf> = self.tests.projects.keys().filter(|r| settings_changed || changed.contains(r) || fresh.contains(*r)).cloned().collect();
        for root in roots {
            drawn |= self.tests_sync_problems(&root, settings.problems);
        }
        let views = self.tests_buffer_views();
        let ids: Vec<BufferId> = self.tests.seen.keys().copied().collect();
        for id in ids {
            let root = self.tests.seen[&id].root.clone();
            let view = views.get(&id).cloned();
            let redo = match self.tests.inline.get(&id) {
                None => true,
                Some(inline) => {
                    settings_changed
                        || changed.contains(&root)
                        || match (&view, &inline.window) {
                            (Some(view), Some(window)) => !(window.contains(view.start()) && window.contains(view.end())),
                            (Some(_), None) => true,
                            (None, _) => false,
                        }
                }
            };
            if redo {
                drawn |= self.tests_sync_inline(id, view, settings.inline);
            }
        }
        drawn
    }

    /// The lines each open buffer's panes show: from the last frame
    /// drawn, else (none drawn yet) from each pane's scroll.
    fn tests_buffer_views(&self) -> HashMap<BufferId, RangeInclusive<u32>> {
        let mut views: HashMap<BufferId, RangeInclusive<u32>> = HashMap::new();
        let mut widen = |id: BufferId, first: u32, last: u32| {
            let range = match views.get(&id) {
                Some(r) => (*r.start()).min(first)..=(*r.end()).max(last),
                None => first..=last,
            };
            views.insert(id, range);
        };
        for &(id, _, first, last) in &self.wasm.views_drawn {
            widen(id, first, last);
        }
        let drawn: HashSet<BufferId> = self.wasm.views_drawn.iter().map(|v| v.0).collect();
        for pane in self.windows().windows() {
            let Some(&id) = self.windows().content(pane) else { continue };
            if drawn.contains(&id) || !self.tests.seen.contains_key(&id) {
                continue;
            }
            let first = self.pane_state(pane).scroll_line as u32;
            widen(id, first, first.saturating_add(MARGIN));
        }
        views
    }

    /// Buffer `id`'s results after its lines (§9.2), for `view` and
    /// `MARGIN` around it, set as decorations of Fenix's own
    /// (`fenix.tests`) when they differ from what's there. Whether they
    /// did.
    fn tests_sync_inline(&mut self, id: BufferId, view: Option<RangeInclusive<u32>>, mode: InlineMode) -> bool {
        let window = view.map(|v| v.start().saturating_sub(MARGIN)..=v.end().saturating_add(MARGIN));
        let mut shown: Vec<(u32, String, Role)> = Vec::new();
        if let (Some(window), Some(seen), true) = (&window, self.tests.seen.get(&id), mode != InlineMode::Off) {
            if let Some(project) = self.tests.projects.get(&seen.root) {
                let model = &project.model;
                for mark in model.marks(&seen.rel, window.clone()) {
                    let Some(node) = model.node(mark.node).filter(|n| n.found().kind.is_test()) else { continue };
                    let Some(result) = node.result() else { continue };
                    let role = |bad| if mark.stale { Role::Muted } else { bad };
                    let text = match mark.state {
                        State::Failed | State::Errored => {
                            let message = result.message.as_deref().filter(|m| !m.trim().is_empty()).unwrap_or(if mark.state == State::Failed { "failed" } else { "errored" });
                            (format!(" {} {}", mark.state.glyph(), short_message(message)), role(Role::Bad))
                        }
                        State::Passed if mode == InlineMode::All => {
                            let took = result.duration_ms.map(|ms| format!(" {}", crate::tests_page::duration(ms))).unwrap_or_default();
                            (format!(" {}{took}", mark.state.glyph()), Role::Muted)
                        }
                        _ => continue,
                    };
                    shown.push((mark.line, text.0, text.1));
                }
            }
        }
        let same = self.tests.inline.get(&id).is_some_and(|i| i.shown == shown);
        self.tests.inline.insert(id, Inline { window, shown: shown.clone() });
        if same {
            return false;
        }
        let Some(ob) = self.buffers.get_mut(id) else { return false };
        let lines = ob.buffer.line_count();
        let decorations: Vec<Decoration> = shown
            .into_iter()
            .filter(|(line, _, _)| (*line as usize) < lines)
            .map(|(line, text, role)| {
                let at = ob.buffer.line_start_char(line as usize) + ob.buffer.line_len(line as usize);
                Decoration { start: at, end: at, look: Look::Text { text, role, after: true }, id: None }
            })
            .collect();
        // They move with the edits from here on, as the inlay hints do;
        // the others are first moved by the edits not yet applied to them.
        ob.buffer.track_anchors();
        let edits = ob.buffer.take_anchor_edits();
        self.wasm.decorations_rev += 1;
        let sets = self.wasm.decorations.entry(id).or_default();
        for (_, set) in sets.iter_mut() {
            crate::decorations::shift(set, &edits);
        }
        sets.retain(|(who, _)| who != TESTS_OWNER);
        if !decorations.is_empty() {
            sets.push((TESTS_OWNER.to_string(), decorations));
        }
        if sets.is_empty() {
            self.wasm.decorations.remove(&id);
        }
        true
    }

    /// Buffer `id`'s results after its lines taken away.
    pub(in crate::app) fn tests_inline_clear(&mut self, id: BufferId) {
        self.tests.inline.remove(&id);
        if let Some(sets) = self.wasm.decorations.get_mut(&id) {
            let before = sets.len();
            sets.retain(|(who, _)| who != TESTS_OWNER);
            if sets.len() != before {
                self.wasm.decorations_rev += 1;
            }
            if sets.is_empty() {
                self.wasm.decorations.remove(&id);
            }
        }
    }

    /// The project at `root`'s failures as problems (§9.3), each at its
    /// `at` when that's a file of the project, else at its test's own
    /// line, under `fenix:#tests` for the project: published where they
    /// differ from what was, and taken away from files that have none
    /// now. Whether any were.
    fn tests_sync_problems(&mut self, root: &Path, on: bool) -> bool {
        let mut placed: Vec<(PathBuf, u32, Option<u32>, lsp_types::Diagnostic)> = Vec::new();
        if let (true, Some(project)) = (on, self.tests.projects.get(root)) {
            let model = &project.model;
            let names: HashMap<fenix_tests::ProviderRef, String> = model.providers().iter().map(|p| (p.provider, crate::tests_page::clean(p.name))).collect();
            for problem in model.problems() {
                // A place outside the project (a library's frame) puts it
                // at the test's own line.
                let inside = problem.at.as_ref().filter(|at| !is_full(&at.path.replace('\\', "/"))).and_then(|at| Some((place_path(root, &at.path)?, at.line, at.column)));
                let Some((path, line, column)) = inside.or_else(|| Some((place_path(root, &problem.file)?, problem.line, None))) else { continue };
                let source = model.node(problem.node).and_then(|n| names.get(&n.provider()).cloned());
                let message = if problem.message.trim().is_empty() { "failed".to_string() } else { crate::tests_page::clean(&problem.message) };
                let diagnostic = lsp_types::Diagnostic {
                    severity: Some(lsp_types::DiagnosticSeverity::ERROR),
                    code: Some(lsp_types::NumberOrString::String(crate::tests_page::clean(&problem.label))),
                    source,
                    message,
                    ..Default::default()
                };
                placed.push((path, line, column, diagnostic));
            }
        }
        let mut want: HashMap<PathBuf, Vec<lsp_types::Diagnostic>> = HashMap::new();
        for (path, line, column, mut diagnostic) in placed {
            // Problems are keyed by the file's own path: a symlink inside
            // the project as the file it is.
            let canonical = self.tests_canon(&path);
            // Columns in UTF-16 units, as a language server's, from the
            // open buffer's line when there is one; to the line's end.
            let text = self.buffers.id_for_path(&path).or_else(|| self.buffers.id_for_path(&canonical)).and_then(|id| self.buffers.get(id)).filter(|ob| (line as usize) < ob.buffer.line_count()).map(|ob| ob.buffer.line(line as usize).to_string());
            let units = |chars: usize| text.as_deref().map_or(chars, |t| t.trim_end_matches(['\n', '\r']).chars().take(chars).map(char::len_utf16).sum());
            let start = units(column.unwrap_or(0) as usize) as u32;
            let end = text.as_deref().map_or(start + 1, |t| t.trim_end_matches(['\n', '\r']).encode_utf16().count() as u32).max(start + 1);
            diagnostic.range = lsp_types::Range { start: lsp_types::Position { line, character: start }, end: lsp_types::Position { line, character: end } };
            want.entry(canonical).or_default().push(diagnostic);
        }
        let Some(project) = self.tests.projects.get_mut(root) else { return false };
        let old = std::mem::take(&mut project.problems);
        project.problems = want.clone();
        let key = LspKey { server: LspServer::Additional(PROBLEMS_KEY.to_string()), root: root.to_path_buf() };
        let mut published = false;
        for (path, diagnostics) in want {
            if old.get(&path) != Some(&diagnostics) {
                self.publish_diagnostics(path, key.clone(), "tests", diagnostics);
                published = true;
            }
        }
        for path in old.into_keys() {
            if !self.tests.projects[root].problems.contains_key(&path) {
                self.publish_diagnostics(path, key.clone(), "tests", Vec::new());
                published = true;
            }
        }
        published
    }

    /// The explorer turned off: what the files showed of it taken away,
    /// once. Whether there was any.
    pub(in crate::app) fn tests_in_files_off(&mut self) -> bool {
        let shown = !self.tests.inline.is_empty() || self.tests.projects.values().any(|p| !p.problems.is_empty());
        if !shown {
            return false;
        }
        self.tests.file_settings = None;
        let ids: Vec<BufferId> = self.tests.inline.keys().copied().collect();
        for id in ids {
            self.tests_inline_clear(id);
        }
        let roots: Vec<PathBuf> = self.tests.projects.keys().cloned().collect();
        for root in roots {
            self.tests_sync_problems(&root, false);
        }
        true
    }
}

/// The project of buffer `id`, as the host keys it.
fn ob_root(app: &App, id: BufferId) -> Option<PathBuf> {
    app.tests.seen.get(&id).map(|s| s.root.clone())
}
