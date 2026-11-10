//! The thread list (`SPC a t`; every project's, `SPC a l`): a pipeline
//! run as one heading with its steps under it, archiving, and a view by
//! project. Laid out by `fenix_acp::listing`; this is the picker around
//! it, its keys, and a run's summary. See docs/plans/thread-list.md.
//!
//! The list is a picker, and what's typed is its query, so its keys are
//! the same with Vim keys and standard keys: `Tab` folds a run, `Alt+A`
//! archives (or brings back), `Alt+H` shows archived threads, `Alt+P`
//! switches between this project and every project, `Alt+R` reviews a
//! run's changes.

use fenix_acp::listing::{self, Listed, Options, Row};
use fenix_keymap::{KeyCode, KeyPress, NamedKey};

use super::*;

/// How the thread list is shown, kept between openings.
#[derive(Debug, Default)]
pub(crate) struct ThreadList {
    /// It's the picker up now.
    pub(crate) open: bool,
    pub(crate) options: Options,
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

impl AgentHost {
    /// Every thread there is, as the list sees it: the store's index, and
    /// what's loaded and live over it.
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
        out
    }
}

impl App {
    /// A thread kept to the store as it is now.
    pub(crate) fn agents_keep_thread(&mut self, id: &ThreadId) {
        self.agents.save(id);
    }

    /// `SPC a t`: this project's threads.
    pub(crate) fn agents_pick_thread(&mut self) {
        self.agents.list.options.by_project = false;
        self.thread_list_show(None, true);
    }

    /// `SPC a l`: every project's threads, by project.
    pub(crate) fn agents_pick_thread_everywhere(&mut self) {
        self.agents.list.options.by_project = true;
        self.thread_list_show(None, true);
    }

    /// The list, laid out again (after a fold, an archive, a filter), the
    /// row `keep` names selected.
    fn thread_list_show(&mut self, keep: Option<Pick>, first: bool) {
        self.agents_configure();
        let root = self.agents_root();
        let listed = self.agents.listed();
        let rows = listing::rows(&listed, &root, &self.agents.list.options);
        if rows.is_empty() && first {
            let why = if self.agents.list.options.by_project { "no agent threads yet" } else { "no agent threads in this project yet" };
            let more = if listed.iter().any(|l| l.archived) { " -- Alt+H in the list shows archived ones" } else { "" };
            self.set_message(format!("{why}: SPC a n starts one{more}"));
            if listed.iter().all(|l| !l.archived) {
                return;
            }
        }
        let now = now_secs();
        let by_id: HashMap<&ThreadId, &Listed> = listed.iter().map(|l| (&l.id, l)).collect();
        let candidates: Vec<fenix_picker::Candidate<Pick>> = rows
            .iter()
            .map(|row| {
                let label = listing::label(row, now);
                match row {
                    Row::Project { root, .. } => fenix_picker::Candidate::new(label, Pick::Project(root.clone())),
                    Row::Run { id, pipeline, request, steps, .. } => {
                        let words: Vec<String> = steps.iter().filter_map(|s| by_id.get(s)).map(|l| format!("{} {}", l.run.as_ref().map(|r| r.step_name.as_str()).unwrap_or(""), l.agent)).collect();
                        fenix_picker::Candidate::new(label, Pick::Run(id.clone())).with_keywords(format!("{pipeline} {request} {}", words.join(" ")))
                    }
                    Row::Step { id, .. } => {
                        let run = by_id.get(id).and_then(|l| l.run.as_ref());
                        let words = run.map(|r| format!("{} {}", r.pipeline, r.request)).unwrap_or_default();
                        fenix_picker::Candidate::new(label, Pick::Thread(id.clone())).with_keywords(words)
                    }
                    Row::Thread { id, .. } => fenix_picker::Candidate::new(label, Pick::Thread(id.clone())),
                }
            })
            .collect();
        let at = keep.and_then(|k| candidates.iter().position(|c| c.payload == k));
        let query = match &self.active_picker {
            Some(ActivePicker::Agents(state)) if !first => state.query().to_string(),
            _ => String::new(),
        };
        self.enter_picker(ActivePicker::Agents(fenix_picker::PickerState::new(candidates)));
        self.agents.list.open = true;
        if let Some(ActivePicker::Agents(state)) = &mut self.active_picker {
            if !query.is_empty() {
                state.set_query(&query);
            }
            if let Some(at) = at.filter(|_| query.is_empty()) {
                state.move_selection(at as isize);
            }
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
                        self.thread_list_show(selected.clone(), false);
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
        let root = self.agents_root();
        let listed = self.agents.listed();
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

    /// The threads of run `id`, in step order.
    fn run_threads(&self, id: &str) -> Vec<Listed> {
        let mut steps: Vec<Listed> = self.agents.listed().into_iter().filter(|l| l.run.as_ref().is_some_and(|r| r.id == id)).collect();
        steps.sort_by_key(|l| l.run.as_ref().map_or(0, |r| r.step));
        steps
    }

    /// `Alt+A`: the thread, or every step of the run, archived -- or, when
    /// it already is, brought back. Nothing is deleted.
    fn thread_list_archive(&mut self, pick: &Pick) {
        let ids: Vec<ThreadId> = match pick {
            Pick::Thread(id) => vec![id.clone()],
            Pick::Run(run) => self.run_threads(run).into_iter().map(|l| l.id).collect(),
            _ => return,
        };
        let all_archived = ids.iter().all(|id| self.agents.listed().iter().any(|l| &l.id == id && l.archived));
        let archive = !all_archived;
        for id in &ids {
            self.agents.load_one(id);
            if let Some(meta) = self.agents.hub.meta_mut(id) {
                meta.archived = archive;
            }
            self.agents_keep_thread(id);
        }
        let what = if matches!(pick, Pick::Run(_)) { "the run" } else { "the thread" };
        self.set_message(if archive { format!("{what} archived -- Alt+H shows archived ones, Alt+A brings it back") } else { format!("{what} brought back") });
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
    /// reviews its changes.
    pub(crate) fn thread_list_open_run(&mut self, id: String) {
        let steps = self.run_threads(&id);
        let Some(first) = steps.first().and_then(|l| l.run.clone()) else { return };
        let ended = steps.iter().find_map(|l| l.run.as_ref().and_then(|r| r.ended.clone()));
        let mut text = format!("# {}\n\n", first.pipeline);
        text.push_str(&format!("Asked {}:\n\n", listing::age(now_secs(), first.started).replace("now", "just now")));
        for line in first.request.lines() {
            text.push_str(&format!("> {line}\n"));
        }
        text.push_str("\n## Steps\n\n");
        for l in &steps {
            let tag = l.run.as_ref();
            let state = if l.state.is_empty() { "✓".to_string() } else { format!("● {}", l.state) };
            let archived = if l.archived { ", archived" } else { "" };
            text.push_str(&format!("{}. {} -- {} {state}{archived}: {}\n", tag.map_or(0, |r| r.step) + 1, tag.map(|r| r.step_name.as_str()).unwrap_or(""), l.agent, l.title));
        }
        text.push_str(&format!("\n## How it ended\n\n{}\n", ended.as_deref().unwrap_or("It's still going.")));
        let ledger = self.run_ledger(&id);
        text.push_str(&match ledger {
            Some(_) => self.keys_text("\nSPC a r reviews what the run changed, every step's, as one review; SPC a t lists its steps.\n"),
            None => "\nIt changed nothing to review.\n".to_string(),
        });
        let syntax = fenix_syntax::SyntaxState::new(fenix_syntax::LanguageId::Markdown, &text);
        let buffer = self.buffers.open_text(&text, Some(syntax));
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
