//! The thread list (`SPC a t`; every project's, `SPC a l`): a pipeline
//! run as one heading with its steps under it, archiving, and a view by
//! project. Laid out by `fenix_acp::listing`; this is the picker around
//! it, its keys, and a run's summary. See docs/plans/thread-list.md.
//!
//! The list is a picker, and what's typed is its query, so its keys are
//! the same with Vim keys and standard keys: `Tab` folds a run, `Alt+A`
//! archives (or brings back), `Alt+H` shows archived threads, `Alt+P`
//! switches between this project and every project, `Alt+R` reviews a
//! run's changes. The commands (`agents.archive`, `agents.threads_archived`,
//! `agents.threads_everywhere`) do the same from the palette and keymap.
//! While a query is typed the rows are flat -- each one saying its run
//! and project -- since they come in the order they match.

use std::path::{Path, PathBuf};

use fenix_acp::listing::{self, Listed, Options, Row};
use fenix_keymap::{KeyCode, KeyPress, NamedKey};

use super::*;

/// How the thread list is shown, kept between openings.
#[derive(Debug, Default)]
pub(crate) struct ThreadList {
    /// It's the picker up now.
    pub(crate) open: bool,
    pub(crate) options: Options,
    /// Each run's summary page, by run id: opened again, not anew.
    pub(crate) summaries: HashMap<String, BufferId>,
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// One project, however it's spelt: on Windows in any letter case, with
/// either slash, with or without `\\?\` -- as `within` compares paths.
fn same_project(a: &Path, b: &Path) -> bool {
    if cfg!(windows) {
        let spelt = |p: &Path| p.to_string_lossy().replace('/', "\\").trim_start_matches(r"\\?\").trim_end_matches('\\').to_lowercase();
        spelt(a) == spelt(b)
    } else {
        a == b
    }
}

impl AgentHost {
    /// Every thread there is, as the list sees it: the store's index, and
    /// what's loaded and live over it. A brand-new thread that's said
    /// nothing yet isn't one to list.
    fn listed(&self) -> Vec<Listed> {
        let mut out: Vec<Listed> = Vec::new();
        let mut seen: HashSet<ThreadId> = HashSet::new();
        for id in self.hub.thread_ids().iter().rev() {
            let Some(view) = self.hub.view(id) else { continue };
            if view.thread.entries.is_empty() && view.meta.run.is_none() {
                continue;
            }
            let state = if view.is_waiting_on_you() {
                "waiting on you"
            } else if view.is_working() {
                "working"
            } else if matches!(view.link, Link::ReadOnly(_)) {
                "read-only"
            } else {
                ""
            };
            out.push(Listed { id: id.clone(), agent: view.meta.agent.clone(), root: view.meta.root.clone(), title: view.thread.display_title(), updated: now_secs(), run: view.meta.run.clone(), archived: view.meta.archived, state: state.to_string() });
            seen.insert(id.clone());
        }
        let index = self.store.as_ref().map(|s| s.index()).unwrap_or_default();
        let saved: HashMap<ThreadId, u64> = index.iter().map(|e| (e.id.clone(), e.updated)).collect();
        // A loaded thread's time is when it was last kept; one just
        // made, now.
        for l in &mut out {
            if let Some(&at) = saved.get(&l.id) {
                l.updated = at;
            }
        }
        for e in index {
            if seen.contains(&e.id) {
                continue;
            }
            out.push(Listed { id: e.id, agent: e.agent, root: e.root, title: e.title, updated: e.updated, run: e.run, archived: e.archived, state: String::new() });
        }
        // One project, one spelling: the first one met.
        let mut roots: Vec<PathBuf> = Vec::new();
        for l in &mut out {
            match roots.iter().find(|r| same_project(r, &l.root)) {
                Some(r) => l.root = r.clone(),
                None => roots.push(l.root.clone()),
            }
        }
        out
    }
}

impl App {
    /// A thread kept to the store as it is now.
    pub(crate) fn agents_keep_thread(&mut self, id: &ThreadId) {
        self.agents.save(id);
    }

    /// The threads as the list shows them: a run left behind by a Fenix
    /// that stopped mid-run (no step of it at work, and not running here)
    /// said to have been interrupted, not running.
    fn thread_list_listed(&self) -> Vec<Listed> {
        let mut listed = self.agents.listed();
        let live: HashSet<&str> = self.pipelines.runs.values().map(|r| r.tag.id.as_str()).collect();
        let busy: HashSet<String> = listed.iter().filter(|l| l.busy()).filter_map(|l| l.run.as_ref().map(|r| r.id.clone())).collect();
        for l in &mut listed {
            if let Some(run) = l.run.as_mut().filter(|r| r.ended.is_none() && !live.contains(r.id.as_str()) && !busy.contains(&r.id)) {
                run.ended = Some("stopped: interrupted, when Fenix closed".to_string());
            }
        }
        listed
    }

    /// The project the list is about, as the list spells it.
    fn thread_list_root(&self, listed: &[Listed]) -> PathBuf {
        let root = self.agents_root();
        listed.iter().map(|l| &l.root).find(|r| same_project(r, &root)).cloned().unwrap_or(root)
    }

    /// `SPC a t`: this project's threads.
    pub(crate) fn agents_pick_thread(&mut self) {
        self.agents.list.options.by_project = false;
        self.agents.list.options.flat = false;
        self.thread_list_show(None, true);
    }

    /// `SPC a l`: every project's threads, by project.
    pub(crate) fn agents_pick_thread_everywhere(&mut self) {
        self.agents.list.options.by_project = true;
        self.agents.list.options.flat = false;
        self.thread_list_show(None, true);
    }

    /// `agents.threads_archived`: this project's threads, archived ones
    /// too -- or, again, without them.
    pub(crate) fn agents_pick_thread_archived(&mut self) {
        let options = &mut self.agents.list.options;
        options.archived = !options.archived;
        options.flat = false;
        let shown = options.archived;
        self.thread_list_show(None, true);
        self.set_message(if shown { "archived threads shown too -- again hides them" } else { "archived threads hidden" });
    }

    /// The list, laid out again (after a fold, an archive, a filter, a
    /// query), the row `keep` names selected -- else the one at `row`.
    fn thread_list_show(&mut self, keep: Option<Pick>, first: bool) {
        self.agents_configure();
        let listed = self.thread_list_listed();
        let root = self.thread_list_root(&listed);
        let options = self.agents.list.options.clone();
        let rows = listing::rows(&listed, &root, &options);
        if rows.is_empty() && first {
            let why = if options.by_project { "no agent threads yet" } else { "no agent threads in this project yet" };
            // Archived ones there are, here: Alt+H.
            let hidden = listed.iter().any(|l| l.archived && (options.by_project || same_project(&l.root, &root)));
            let more = if hidden && !options.archived { " -- Alt+H in the list shows the archived ones" } else { "" };
            self.set_message(format!("{why}: SPC a n starts one{more}"));
            if !hidden {
                return;
            }
        }
        let now = now_secs();
        let flat = options.flat;
        let by_id: HashMap<&ThreadId, &Listed> = listed.iter().map(|l| (&l.id, l)).collect();
        let candidates: Vec<fenix_picker::Candidate<Pick>> = rows
            .iter()
            .map(|row| {
                let label = if flat { listing::flat_label(row, now) } else { listing::label(row, now) };
                match row {
                    Row::Project { root, .. } => fenix_picker::Candidate::new(label, Pick::Project(root.clone())),
                    Row::Run { id, pipeline, request, steps, .. } => {
                        let words: Vec<String> = steps.iter().filter_map(|s| by_id.get(s)).map(|l| format!("{} {}", l.run.as_ref().map(|r| r.step_name.as_str()).unwrap_or(""), l.agent)).collect();
                        fenix_picker::Candidate::new(label, Pick::Run(id.clone())).with_keywords(format!("{pipeline} {request} {}", words.join(" ")))
                    }
                    Row::Step { id, request, pipeline, .. } => fenix_picker::Candidate::new(label, Pick::Thread(id.clone())).with_keywords(format!("{pipeline} {request}")),
                    Row::Thread { id, .. } => fenix_picker::Candidate::new(label, Pick::Thread(id.clone())),
                }
            })
            .collect();
        let (query, row) = match &self.active_picker {
            Some(ActivePicker::Agents(state)) if !first => (state.query().to_string(), state.selected_row()),
            _ => (String::new(), 0),
        };
        self.enter_picker(ActivePicker::Agents(fenix_picker::PickerState::new(candidates)));
        self.agents.list.open = true;
        if let Some(ActivePicker::Agents(state)) = &mut self.active_picker {
            if !query.is_empty() {
                state.set_query(&query);
            }
            // The row it was on, as the list now has it; else the one at
            // the same place, so the next key doesn't land at the top.
            let len = state.len();
            let at = keep.and_then(|k| state.visible_rows(0, len).position(|(_, c)| c.payload == k)).unwrap_or(row.min(len.saturating_sub(1)));
            state.move_selection(at as isize);
        }
    }

    /// After a key in the list: typing a query (or clearing it) lays the
    /// rows out flat (or grouped again).
    pub(crate) fn thread_list_follow_query(&mut self) {
        if !self.agents.list.open {
            return;
        }
        let Some(ActivePicker::Agents(state)) = &self.active_picker else {
            self.agents.list.open = false;
            return;
        };
        let typed = !state.query().is_empty();
        if typed != self.agents.list.options.flat {
            self.agents.list.options.flat = typed;
            let keep = state.selected().map(|c| c.payload.clone());
            self.thread_list_show(keep, false);
        }
    }

    /// What's selected in the list.
    fn thread_list_selected(&self) -> Option<Pick> {
        match &self.active_picker {
            Some(ActivePicker::Agents(state)) => state.selected().map(|c| c.payload.clone()),
            _ => None,
        }
    }

    /// A key in the thread list, when it's up: true when it was the list's.
    pub(crate) fn thread_list_key(&mut self, key: KeyPress) -> bool {
        if !self.agents.list.open || !matches!(self.active_picker, Some(ActivePicker::Agents(_))) {
            self.agents.list.open = false;
            return false;
        }
        let selected = self.thread_list_selected();
        let alt = key.mods.alt && !key.mods.ctrl;
        match key.code {
            KeyCode::Named(NamedKey::Tab) if !key.mods.ctrl && !key.mods.alt => {
                if let Some(Pick::Run(id)) = &selected {
                    self.thread_list_toggle_fold(id);
                    self.thread_list_show(selected.clone(), false);
                }
                true
            }
            KeyCode::Char('a' | 'A') if alt => {
                match &selected {
                    Some(pick @ (Pick::Run(_) | Pick::Thread(_))) => {
                        self.thread_list_archive(pick);
                        // The row after it, now where it was.
                        self.thread_list_show(None, false);
                    }
                    _ => self.set_message("archive a thread or a run: Alt+A on it"),
                }
                true
            }
            KeyCode::Char('h' | 'H') if alt => {
                let options = &mut self.agents.list.options;
                options.archived = !options.archived;
                let shown = options.archived;
                self.thread_list_show(selected, false);
                self.set_message(if shown { "archived threads shown too -- Alt+H hides them" } else { "archived threads hidden -- Alt+H shows them" });
                true
            }
            KeyCode::Char('p' | 'P') if alt => {
                let options = &mut self.agents.list.options;
                options.by_project = !options.by_project;
                self.thread_list_show(selected, false);
                true
            }
            KeyCode::Char('r' | 'R') if alt => {
                match selected {
                    Some(Pick::Run(id)) => {
                        self.picker_close_for_list();
                        self.thread_list_review_run(&id);
                    }
                    _ => self.set_message("Alt+R on a run reviews what it changed"),
                }
                true
            }
            _ => false,
        }
    }

    /// The list goes, for what was picked from it.
    fn picker_close_for_list(&mut self) {
        self.active_picker = None;
        self.agents.list.open = false;
        self.main_view = MainView::Editor;
    }

    fn thread_list_toggle_fold(&mut self, id: &str) {
        let listed = self.thread_list_listed();
        let root = self.thread_list_root(&listed);
        let folded_now = listing::rows(&listed, &root, &self.agents.list.options).iter().any(|r| matches!(r, Row::Run { id: rid, folded: true, .. } if rid == id));
        let options = &mut self.agents.list.options;
        if folded_now {
            options.folded.remove(id);
            options.unfolded.insert(id.to_string());
        } else {
            options.unfolded.remove(id);
            options.folded.insert(id.to_string());
        }
    }

    /// The threads of run `id`, in the order they were started.
    fn run_threads(&self, id: &str) -> Vec<Listed> {
        let mut steps: Vec<Listed> = self.thread_list_listed().into_iter().filter(|l| l.run.as_ref().is_some_and(|r| r.id == id)).collect();
        steps.sort_by_key(|l| l.run.as_ref().map_or((0, 0), |r| (r.seq, r.step)));
        steps
    }

    /// `Alt+A` (`agents.archive`): the thread, or every step of the run --
    /// a step of a run is archived with its run -- archived, or, when it
    /// already is, brought back. Nothing is deleted, and when it was
    /// last at work stays as it was. A thread at work or waiting on you
    /// isn't put away.
    pub(crate) fn thread_list_archive(&mut self, pick: &Pick) {
        let listed = self.thread_list_listed();
        let run_of = |id: &ThreadId| listed.iter().find(|l| &l.id == id).and_then(|l| l.run.as_ref().map(|r| r.id.clone()));
        let (ids, what) = match pick {
            Pick::Thread(id) => match run_of(id) {
                Some(run) => (self.run_threads(&run).into_iter().map(|l| l.id).collect::<Vec<_>>(), "the run"),
                None => (vec![id.clone()], "the thread"),
            },
            Pick::Run(run) => (self.run_threads(run).into_iter().map(|l| l.id).collect(), "the run"),
            _ => return,
        };
        if ids.iter().any(|id| listed.iter().any(|l| &l.id == id && l.busy())) {
            self.set_message(format!("{what} is at work or waiting on you -- archive it once it's done"));
            return;
        }
        let archive = !ids.iter().all(|id| listed.iter().any(|l| &l.id == id && l.archived));
        let mut failed = 0;
        for id in &ids {
            if !self.agents.load_one(id) {
                failed += 1;
                continue;
            }
            if let Some(meta) = self.agents.hub.meta_mut(id) {
                meta.archived = archive;
            }
            if self.agents.save_keeping_time(id).is_err() {
                failed += 1;
            }
        }
        if failed > 0 {
            self.set_error(format!("{failed} of {} couldn't be kept as {} -- where threads are kept can't be read or written", ids.len(), if archive { "archived" } else { "brought back" }));
            return;
        }
        self.set_message(if archive { format!("{what} archived -- Alt+H shows archived ones, Alt+A brings it back") } else { format!("{what} brought back") });
    }

    /// `agents.archive`: the thread the agents' panel shows (its run, when
    /// it's a step of one) archived, or brought back.
    pub(crate) fn agents_archive_shown(&mut self) {
        let Some(thread) = self.agents.shown(&self.pages) else {
            self.set_message("no agent thread open: SPC a t lists them, Alt+A there archives one");
            return;
        };
        self.thread_list_archive(&Pick::Thread(thread));
    }

    /// A thread picked from the list: read in if it isn't, then shown.
    pub(crate) fn thread_list_open_thread(&mut self, id: ThreadId) {
        if !self.agents.load_one(&id) {
            self.set_error("that thread couldn't be read from where threads are kept");
            return;
        }
        self.agents_show(id);
    }

    /// The thread whose ledger holds run `id`'s changes: what its steps
    /// were told when it ended, or the run's own while it goes on.
    fn run_ledger(&self, id: &str) -> Option<ThreadId> {
        let steps = self.run_threads(id);
        if let Some(ledger) = steps.iter().find_map(|l| l.run.as_ref().and_then(|r| r.ledger.clone())) {
            return Some(ledger);
        }
        self.pipelines.runs.values().find(|r| r.tag.id == id).and_then(|r| r.ledger.clone())
    }

    /// `Alt+R` on a run, or `SPC a r` from its summary: every step's
    /// changes as one review.
    pub(crate) fn thread_list_review_run(&mut self, id: &str) {
        let Some(ledger) = self.run_ledger(id) else {
            self.set_message("that run changed nothing to review");
            return;
        };
        self.agents.load_one(&ledger);
        self.agents_open_review_of(ledger);
    }

    /// `Enter` on a run: its summary, on a page of its own -- what was
    /// asked, each step and how it went, how it ended, and the key that
    /// reviews its changes. The same run's page is reused.
    pub(crate) fn thread_list_open_run(&mut self, id: String) {
        let steps = self.run_threads(&id);
        let Some(first) = steps.first().and_then(|l| l.run.clone()) else { return };
        // The whole request: the index keeps only its start.
        if let Some(tag) = self.agents.load_one(&steps[0].id).then(|| self.agents.hub.meta(&steps[0].id).and_then(|m| m.run.clone())).flatten() {
            if tag.request.len() > first.request.len() {
                return self.thread_list_open_run_with(id, steps, RunTagWhole(tag));
            }
        }
        self.thread_list_open_run_with(id, steps, RunTagWhole(first));
    }

    fn thread_list_open_run_with(&mut self, id: String, steps: Vec<Listed>, first: RunTagWhole) {
        let first = first.0;
        let ended = steps.iter().find_map(|l| l.run.as_ref().and_then(|r| r.ended.clone()));
        // Each step's mark, as the list says it.
        let listed = self.thread_list_listed();
        let root = self.thread_list_root(&listed);
        let options = Options { unfolded: [id.clone()].into(), archived: true, by_project: true, ..Options::default() };
        let marks: HashMap<ThreadId, (String, String)> = listing::rows(&listed, &root, &options)
            .into_iter()
            .filter_map(|r| match r {
                Row::Step { id, name, mark, .. } => Some((id, (name, mark))),
                _ => None,
            })
            .collect();
        let mut text = format!("# {}\n\n", first.pipeline);
        text.push_str(&format!("Asked {}:\n\n", listing::age(now_secs(), first.started).replace("now", "just now")));
        for line in first.request.lines() {
            text.push_str(&format!("> {line}\n"));
        }
        text.push_str("\n## Steps\n\n");
        for l in &steps {
            let (name, mark) = marks.get(&l.id).cloned().unwrap_or_default();
            let archived = if l.archived { ", archived" } else { "" };
            text.push_str(&format!("{}. {name} -- {} {mark}{archived}: {}\n", l.run.as_ref().map_or(0, |r| r.step) + 1, l.agent, l.title));
        }
        text.push_str(&format!("\n## How it ended\n\n{}\n", ended.as_deref().unwrap_or("It's still going.")));
        let ledger = self.run_ledger(&id);
        text.push_str(&match ledger {
            Some(_) => self.keys_text("\nSPC a r reviews what the run changed, every step's, as one review; SPC a t lists its steps.\n"),
            None => "\nIt changed nothing to review.\n".to_string(),
        });
        // Its page again, when it's still open: brought up to date.
        let buffer = match self.agents.list.summaries.get(&id).copied().filter(|b| self.buffers.get(*b).is_some()) {
            Some(buffer) => {
                if let Some(ob) = self.buffers.get_mut(buffer) {
                    let mut scratch = Cursor::at_start();
                    let end = ob.buffer.len_chars();
                    ob.buffer.replace_range(&mut scratch, 0, end, &text);
                    ob.buffer.mark_saved();
                }
                buffer
            }
            None => {
                let syntax = fenix_syntax::SyntaxState::new(fenix_syntax::LanguageId::Markdown, &text);
                self.buffers.open_text(&text, Some(syntax))
            }
        };
        self.agents.list.summaries.insert(id, buffer);
        self.agents.run_summaries.retain(|b, _| self.buffers.get(*b).is_some());
        if let Some(ledger) = ledger {
            self.agents.run_summaries.insert(buffer, ledger);
        }
        if self.agents_is_dock_pane(self.focused_pane_id()) {
            self.agents_focus_code();
        }
        self.open_buffer_in_focused_pane(buffer);
        self.wake_caret();
    }
}

/// A run's tag with its whole request, read from its thread.
struct RunTagWhole(fenix_acp::hub::RunTag);
