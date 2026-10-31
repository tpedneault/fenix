//! Threads over time: the panel kept with the session and put back after
//! a restart (its prompt's unsent text too), a thread handed off to
//! another agent, a read-only one continued in a new session, one
//! deleted.

use super::*;
use crate::app::wasm_host::SavedPage;

/// What the session calls the agents' pages, among extensions' pages
/// (no extension can have this id: ids are `publisher.name`).
pub(crate) const PAGE_EXT: &str = "fenix:agents";

impl AgentHost {
    /// Reads one thread from the store into the hub, if it's not there.
    pub(super) fn load_one(&mut self, id: &ThreadId) -> bool {
        if self.hub.view(id).is_some() {
            return true;
        }
        let Some(store) = &self.store else { return false };
        let Ok(saved) = store.load(id) else { return false };
        self.hub.restore(saved.meta, saved.thread);
        if let Some(baselines) = saved.extra.get("baselines").and_then(Value::as_object) {
            self.baselines.insert(id.clone(), baselines.iter().map(|(k, v)| (PathBuf::from(k), v.as_str().map(str::to_string))).collect());
        }
        self.loaded.insert(id.clone());
        true
    }
}

impl App {
    /// The panel's page and prompt, for the session.
    pub(in crate::app) fn agents_pages_to_save(&self) -> Vec<SavedPage> {
        let mut pages = Vec::new();
        let Some(thread) = self.agents.shown(&self.pages) else { return pages };
        // A thread with nothing in it has nothing to come back to.
        if self.agents.hub.view(&thread).is_none_or(|v| v.thread.entries.is_empty()) {
            return pages;
        }
        if let Some(page) = self.agents.page {
            pages.push(SavedPage { buffer: page, ext: PAGE_EXT.into(), kind: "thread".into(), title: self.page_title(page), state: thread.0.clone(), file: None, given: Vec::new() });
        }
        if let Some(composer) = self.agents.composer.filter(|c| self.buffers.get(*c).is_some()) {
            let text = self.buffers.get(composer).map(|b| b.buffer.text()).unwrap_or_default();
            pages.push(SavedPage { buffer: composer, ext: PAGE_EXT.into(), kind: "prompt".into(), title: "prompt".into(), state: text, file: None, given: Vec::new() });
        }
        pages
    }

    /// A page of the agents' from the last session, put back: the thread
    /// read from the store (detached, until it's looked at or prompted),
    /// or the prompt with what was written in it.
    pub(crate) fn agents_restore_page(&mut self, kind: &str, state: &str) -> Option<BufferId> {
        match kind {
            "thread" => {
                let thread = ThreadId(state.to_string());
                if !self.agents.load_one(&thread) {
                    return None;
                }
                self.agents_configure();
                let page = self.add_page(PageModel::Agent(Box::new(AgentPage::new(thread))));
                self.agents.page = Some(page);
                Some(page)
            }
            "prompt" => {
                let composer = self.buffers.open_prompt();
                if !state.is_empty() {
                    if let Some(ob) = self.buffers.get_mut(composer) {
                        let mut cursor = Cursor::at_start();
                        ob.buffer.replace_range(&mut cursor, 0, 0, state);
                    }
                }
                self.agents.composer = Some(composer);
                Some(composer)
            }
            _ => None,
        }
    }

    /// `SPC a h`: the thread goes on with another agent, its transcript
    /// carried to the new one's first prompt.
    pub(crate) fn agents_pick_handoff(&mut self) {
        let Some(thread) = self.agents.shown(&self.pages) else {
            self.set_message("no agent thread open: SPC a a");
            return;
        };
        let current = self.agents.hub.meta(&thread).map(|m| m.agent.clone()).unwrap_or_default();
        let entries = self.agents_configure();
        let candidates: Vec<_> = entries
            .iter()
            .filter(|e| e.found && e.spec.id != current)
            .map(|e| fenix_picker::Candidate::new(format!("Continue with {}", e.spec.name), Pick::HandOff(e.spec.id.clone())))
            .collect();
        if candidates.is_empty() {
            self.set_message("no other agent found to hand this thread to");
            return;
        }
        self.enter_picker(ActivePicker::Agents(fenix_picker::PickerState::new(candidates)));
    }

    pub(super) fn agents_hand_off(&mut self, agent: &str) {
        let Some(from) = self.agents.shown(&self.pages) else { return };
        self.agents_configure();
        let kib = self.config.int_value("agents.handoff_kib").unwrap_or(64).clamp(4, 1024) as usize;
        let Some(id) = self.agents.hub.hand_off(&from, agent, kib * 1024) else { return };
        self.agents_show(id);
        self.set_message("handed off: your next prompt carries the conversation so far");
    }

    /// The shown thread, read-only because its agent couldn't take it
    /// back, continued in a new session of the same agent.
    pub(crate) fn agents_continue_anew(&mut self) {
        let Some(thread) = self.agents.shown(&self.pages) else { return };
        let kib = self.config.int_value("agents.handoff_kib").unwrap_or(64).clamp(4, 1024) as usize;
        let fx = self.agents.hub.continue_in_new_session(&thread, kib * 1024);
        if fx.is_empty() {
            self.set_message("this thread isn't read-only: just write to it");
        }
        self.agents_run(fx);
    }

    /// `agents.delete`: the shown thread, deleted -- asked first.
    pub(crate) fn agents_delete_thread(&mut self) {
        let Some(thread) = self.agents.shown(&self.pages) else { return };
        let title = self.agents.hub.view(&thread).map(|v| v.thread.display_title()).unwrap_or_default();
        let candidates = vec![
            fenix_picker::Candidate::new(format!("Delete \"{title}\" (and its session, when the agent can)"), Pick::Delete(thread)),
            fenix_picker::Candidate::new("Keep it".to_string(), Pick::Nothing),
        ];
        self.enter_picker(ActivePicker::Agents(fenix_picker::PickerState::new(candidates)));
    }

    pub(super) fn agents_delete(&mut self, thread: &ThreadId) {
        let fx = self.agents.hub.delete(thread, Instant::now());
        self.agents_run(fx);
        self.agents.baselines.remove(thread);
        self.agents.unknown.remove(thread);
        // The panel moves to the project's next thread, or closes.
        let root = self.agents_root();
        let next = self.agents.hub.thread_ids().iter().rev().find(|t| self.agents.hub.meta(t).is_some_and(|m| m.root == root)).cloned();
        match next {
            Some(next) => self.agents_show(next),
            None => self.agents_close_panel(),
        }
        self.set_message("thread deleted");
    }
}
