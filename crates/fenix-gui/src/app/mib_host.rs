//! The host half of the MIB pages (`mib_page`, `mib_def`, `mib_form`):
//! which MIBs the focused project uses, reading them off the UI thread
//! and again when their files change, the `SPC k` commands, what the
//! pages' keys ask for, and the MIB in the editor -- `K`, `gd` and
//! completion on a name from it.

use std::sync::Arc;

use fenix_mib::{DefRef, Kind, MibRoot, MibSet};

use super::pages::{PageEvent, PageModel};
use super::*;
use crate::mib_def::{self, DefPage};
use crate::mib_form::{self, InsertForm};
use crate::mib_page::{self, MibKey, MibPage};

/// A set of MIBs, read or being read.
pub(crate) struct MibSlot {
    pub(super) set: Option<Arc<MibSet>>,
    /// A read is under way: the first, or a reload after a change.
    pub(super) loading: bool,
}

/// What asked for a set before it was ready.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MibPending {
    Search,
    Pick(Kind),
    Insert,
    EditCall,
}

/// Where an insert form's command goes: a buffer, and the characters it
/// takes the place of (none, for an insert at the cursor).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MibOrigin {
    pub(super) buffer: BufferId,
    pub(super) range: std::ops::Range<usize>,
}

impl App {
    /// The MIBs the focused project uses: its own `mib.roots` (paths
    /// relative to it), else the project itself when it's a MIB, else
    /// yours -- with yours added to the project's when it says so. And
    /// where they come from, for the page to say.
    pub(super) fn mib_key_here(&self) -> (MibKey, String) {
        let project = self.project_root.clone();
        let name = project.as_ref().and_then(|r| r.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let mut roots: Vec<MibRoot> = Vec::new();
        if let (Some(root), Some(fenix_config::Value::Map(entries))) = (&project, self.mib_project_value("mib.roots")) {
            for (label, path) in entries {
                let path = PathBuf::from(path);
                let path = if path.is_absolute() { path } else { root.join(path) };
                roots.push(MibRoot { label: label.clone(), path });
            }
        }
        if roots.is_empty() {
            if let Some(root) = &project {
                if self.project_kind_of(root) == fenix_project::ProjectKind::Mib {
                    let dir = if root.join("mib").is_dir() { root.join("mib") } else { root.clone() };
                    roots.push(MibRoot { label: name.clone(), path: dir });
                }
            }
        }
        let own = !roots.is_empty();
        let include = match self.mib_project_value("mib.include_yours") {
            Some(fenix_config::Value::Bool(b)) => *b,
            _ => self.config.mib_include_yours.unwrap_or(false),
        };
        if !own || include {
            for (label, path) in &self.config.mib_roots {
                if !roots.iter().any(|r| &r.path == path) {
                    roots.push(MibRoot { label: label.clone(), path: path.clone() });
                }
            }
        }
        let default = match self.mib_project_value("mib.default") {
            Some(fenix_config::Value::Text(t)) => Some(t.clone()),
            _ => self.config.mib_default.clone(),
        };
        let source = match (own, include && !self.config.mib_roots.is_empty()) {
            (true, false) => format!("from {name}'s settings"),
            (true, true) => format!("from {name}'s settings and yours"),
            (false, _) if roots.is_empty() => String::new(),
            (false, _) => "from your settings".to_string(),
        };
        (MibKey { roots, default, project }, source)
    }

    fn mib_project_value(&self, key: &str) -> Option<&fenix_config::Value> {
        self.project_settings.as_ref().and_then(|(_, p)| p.get(key))
    }

    pub(super) fn mib_apid_hex(&self) -> bool {
        self.config.mib_apid_format.as_deref() != Some("decimal")
    }

    /// The set for `key` when it's been read; starts reading it when it
    /// hasn't.
    pub(super) fn mib_set(&mut self, key: &MibKey) -> Option<Arc<MibSet>> {
        if key.roots.is_empty() {
            return None;
        }
        match self.mib_sets.get(key) {
            Some(slot) => slot.set.clone(),
            None => {
                self.mib_load(key.clone());
                self.mib_sets.get(key).and_then(|s| s.set.clone())
            }
        }
    }

    /// The set for `key`, only if it's already read -- for the editor,
    /// which never waits.
    pub(super) fn mib_set_ready(&self, key: &MibKey) -> Option<Arc<MibSet>> {
        self.mib_sets.get(key).and_then(|s| s.set.clone())
    }

    fn mib_loading(&self, key: &MibKey) -> bool {
        self.mib_sets.get(key).is_some_and(|s| s.loading)
    }

    /// Reads `key`'s MIBs off the UI thread; what was read before stays
    /// in use until the new set arrives.
    fn mib_load(&mut self, key: MibKey) {
        let slot = self.mib_sets.entry(key.clone()).or_insert(MibSlot { set: None, loading: false });
        if slot.loading {
            return;
        }
        slot.loading = true;
        self.page_spawn(move |send| {
            let set = MibSet::load(key.roots.clone(), key.default.as_deref());
            send(PageEvent::MibLoaded { key, set: Arc::new(set) });
        });
    }

    pub(super) fn apply_mib_loaded(&mut self, key: MibKey, set: Arc<MibSet>) {
        let problems = set.problems().len();
        let fresh = self.mib_sets.get(&key).is_none_or(|s| s.set.is_none());
        self.mib_sets.insert(key.clone(), MibSlot { set: Some(set), loading: false });
        self.mib_mark_pages();
        if !fresh && problems > 0 {
            self.set_message(format!("MIB read again -- {problems} problem{} (! on the MIB page)", if problems == 1 { "" } else { "s" }));
        } else if !fresh {
            self.set_message("MIB read again");
        }
        if let Some((pending_key, what)) = self.mib_pending.take() {
            if pending_key == key {
                match what {
                    MibPending::Search => self.cmd_mib_search(),
                    MibPending::Pick(kind) => self.cmd_mib_pick(kind),
                    MibPending::Insert => self.cmd_mib_insert(),
                    MibPending::EditCall => self.cmd_mib_edit_call(),
                }
            } else {
                self.mib_pending = Some((pending_key, what));
            }
        }
    }

    /// Every MIB page is laid out again.
    fn mib_mark_pages(&mut self) {
        for state in self.pages.values_mut() {
            if matches!(state.model, PageModel::Mib(_) | PageModel::MibDef(_) | PageModel::MibForm(_)) {
                state.stale = true;
            }
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    /// The disk poll's tick: a set whose files changed is read again.
    pub(super) fn mib_poll(&mut self) {
        if !self.config.mib_watch.unwrap_or(true) {
            return;
        }
        let stale: Vec<MibKey> = self
            .mib_sets
            .iter()
            .filter(|(_, slot)| !slot.loading)
            .filter_map(|(key, slot)| slot.set.as_ref().filter(|set| fenix_mib::Stamp::of(&key.roots) != *set.stamp()).map(|_| key.clone()))
            .collect();
        for key in stale {
            self.mib_load(key);
        }
    }

    /// A project was focused: its MIBs start being read, so `K`, `gd` and
    /// completion have them by the time they're asked.
    pub(super) fn mib_preload(&mut self) {
        let (key, _) = self.mib_key_here();
        if !key.roots.is_empty() && !self.mib_sets.contains_key(&key) {
            self.mib_load(key);
        }
    }

    /// The MIB settings changed: the MIB page follows the project's MIBs.
    pub(super) fn mib_settings_changed(&mut self) {
        let (key, _) = self.mib_key_here();
        if let Some(id) = self.find_page(|m| matches!(m, PageModel::Mib(_))) {
            if let Some(PageModel::Mib(page)) = self.pages.get_mut(&id).map(|s| &mut s.model) {
                page.key = key;
            }
        }
        self.mib_mark_pages();
    }

    /// The set for the focused project, or a word about why there's
    /// none yet -- `pending` runs once it's read.
    fn mib_set_here(&mut self, pending: MibPending) -> Option<(MibKey, Arc<MibSet>)> {
        let (key, _) = self.mib_key_here();
        if key.roots.is_empty() {
            self.set_error("no MIB for this project -- SPC k , lists one in its settings");
            return None;
        }
        match self.mib_set(&key) {
            Some(set) => Some((key, set)),
            None => {
                self.set_message("reading the MIB…");
                self.mib_pending = Some((key, pending));
                None
            }
        }
    }

    // -- Commands ---------------------------------------------------------

    /// `SPC k k`: the MIB page, on the focused project's MIBs.
    pub(crate) fn cmd_mib_page(&mut self) {
        let (key, _) = self.mib_key_here();
        let _ = self.mib_set(&key);
        match self.find_page(|m| matches!(m, PageModel::Mib(_))) {
            Some(id) => {
                if let Some(PageModel::Mib(page)) = self.pages.get_mut(&id).map(|s| &mut s.model) {
                    page.key = key;
                }
                self.show_page(id);
            }
            None => {
                self.open_page(PageModel::Mib(Box::new(MibPage::new(key))));
            }
        }
    }

    fn mib_picker(&mut self, set: &MibSet, kinds: &[Kind]) -> Vec<fenix_picker::Candidate<DefRef>> {
        let hex = self.mib_apid_hex();
        let many = set.roots().len() > 1;
        let mut out = Vec::new();
        for &kind in kinds {
            for (index, e) in set.entries(kind).iter().enumerate() {
                let def = DefRef { kind, index };
                let cols: Vec<String> = (1..kind.headers().len()).map(|c| mib_page::cell(set, def, c, hex)).filter(|c| !c.is_empty()).collect();
                let mib = if many { format!("[{}]  ", set.root_label(e.root)) } else { String::new() };
                let tag = if kinds.len() > 1 { format!("{:<4}", kind.tag()) } else { String::new() };
                let label = format!("{tag}{:<10}  {:<24}  {mib}{}", e.name, cols.join("  "), e.description);
                out.push(fenix_picker::Candidate::new(label, def));
            }
        }
        out
    }

    /// `SPC k /`: every kind of definition, by name or description.
    pub(crate) fn cmd_mib_search(&mut self) {
        let Some((key, set)) = self.mib_set_here(MibPending::Search) else { return };
        let candidates = self.mib_picker(&set, &Kind::ALL);
        self.mib_picker_key = key;
        self.enter_picker(ActivePicker::MibDef(fenix_picker::PickerState::new(candidates)));
    }

    /// `SPC k t`/`p`/`m`/`n`/`c`: one kind of definition, by name.
    pub(crate) fn cmd_mib_pick(&mut self, kind: Kind) {
        let Some((key, set)) = self.mib_set_here(MibPending::Pick(kind)) else { return };
        let candidates = self.mib_picker(&set, &[kind]);
        if candidates.is_empty() {
            self.set_message(format!("no {} in this project's MIBs", kind.plural().to_lowercase()));
            return;
        }
        self.mib_picker_key = key;
        self.enter_picker(ActivePicker::MibDef(fenix_picker::PickerState::new(candidates)));
    }

    /// `SPC k i`: a telecommand, then its insert form.
    pub(crate) fn cmd_mib_insert(&mut self) {
        let Some((key, set)) = self.mib_set_here(MibPending::Insert) else { return };
        let candidates = self.mib_picker(&set, &[Kind::Telecommand]);
        self.mib_picker_key = key;
        self.enter_picker(ActivePicker::MibInsert(fenix_picker::PickerState::new(candidates)));
    }

    /// `SPC k r`: the MIBs read again now.
    pub(crate) fn cmd_mib_reload(&mut self) {
        let (key, _) = self.mib_key_here();
        if key.roots.is_empty() {
            self.set_error("no MIB for this project -- SPC k , lists one in its settings");
            return;
        }
        self.set_message("reading the MIB again…");
        self.mib_load(key);
    }

    /// `SPC k ,`: the MIB settings -- the project's, or yours outside one.
    pub(crate) fn cmd_mib_settings(&mut self) {
        self.mib_open_settings(true);
    }

    fn mib_open_settings(&mut self, project: bool) {
        let scope = match self.settings_project() {
            Some((root, name)) if project => crate::settings_page::Scope::Project { root, name },
            _ => crate::settings_page::Scope::You,
        };
        self.open_settings_page(scope, Some("mib.roots"));
    }

    /// `SPC k e`: the telecommand call on the cursor's line, back in the
    /// insert form; inserting puts it back in place of the line's call.
    pub(crate) fn cmd_mib_edit_call(&mut self) {
        let Some((key, set)) = self.mib_set_here(MibPending::EditCall) else { return };
        let buffer = self.focused_buffer_id();
        let (line_text, line_start) = {
            let ob = self.open();
            let (line, _) = ob.buffer.line_col(&self.cursor());
            (ob.buffer.line(line).to_string(), ob.buffer.line_start_char(line))
        };
        let line_text = line_text.trim_end_matches(['\n', '\r']).to_string();
        let tc = words(&line_text).into_iter().find_map(|(_, w)| set.resolve(w).filter(|d| d.kind == Kind::Telecommand));
        let Some(tc) = tc else {
            self.set_error("no telecommand from this project's MIBs on this line");
            return;
        };
        let templates = self.mib_templates(&key);
        let names: Vec<String> = fenix_mib::telecommand::tc_parameters(set.index(), &set.get(tc).row).into_iter().filter(|p| !p.fixed).map(|p| p.name).collect();
        let args = mib_form::read_arguments(&line_text, &names, &templates);
        let indent = line_text.chars().take_while(|c| c.is_whitespace()).count();
        let range = line_start + indent..line_start + line_text.chars().count();
        let mut form = InsertForm::new(&set, key.clone(), tc, templates, None, self.mib_apid_hex());
        form.fill(&args);
        form.replacing = true;
        let id = self.open_page(PageModel::MibForm(Box::new(form)));
        self.mib_origins.insert(id, MibOrigin { buffer, range });
    }

    /// How `key`'s project writes a command: its templates, else yours.
    fn mib_templates(&self, key: &MibKey) -> mib_form::Templates {
        let project = key.project.as_deref().map(fenix_config::ProjectSettings::load);
        let text = |name: &str, mine: &Option<String>, default: &str| match project.as_ref().and_then(|p| p.get(name)) {
            Some(fenix_config::Value::Text(t)) => t.clone(),
            _ => mine.clone().unwrap_or_else(|| default.to_string()),
        };
        mib_form::Templates {
            command: text("mib.telecommand_template", &self.config.mib_telecommand_template, fenix_mib::telecommand::DEFAULT_TEMPLATE),
            argument: text("mib.telecommand_argument_template", &self.config.mib_telecommand_argument_template, fenix_mib::telecommand::DEFAULT_ARGUMENT_TEMPLATE),
            separator: text("mib.telecommand_argument_separator", &self.config.mib_telecommand_argument_separator, fenix_mib::telecommand::DEFAULT_ARGUMENT_SEPARATOR),
        }
    }

    // -- Pages --------------------------------------------------------------

    /// `def`'s page: the open definition page goes to it (so `Ctrl-o`
    /// comes back), else a new one opens.
    pub(super) fn open_mib_def(&mut self, key: MibKey, def: DefRef) {
        let existing = self.find_page(|m| matches!(m, PageModel::MibDef(p) if p.key == key));
        match existing {
            Some(id) => {
                if let Some(PageModel::MibDef(page)) = self.pages.get_mut(&id).map(|s| &mut s.model) {
                    page.go(def);
                }
                self.show_page(id);
            }
            None => {
                self.open_page(PageModel::MibDef(Box::new(DefPage::new(key, def))));
            }
        }
    }

    /// The insert form for `tc`, inserting where the cursor was in the
    /// last file you were in.
    pub(super) fn open_mib_form(&mut self, key: MibKey, tc: DefRef) {
        let Some(set) = self.mib_set_ready(&key) else { return };
        let Some(buffer) = self.mib_insert_target() else {
            self.set_error("open the file the telecommand goes in first -- the form inserts where its cursor is");
            return;
        };
        let at = if buffer == self.focused_buffer_id() { self.cursor().char_idx } else { self.mib_buffer_cursor(buffer) };
        let remembered = self.mib_last.get(&(key.project.clone(), set.get(tc).name.clone()));
        let templates = self.mib_templates(&key);
        let form = InsertForm::new(&set, key, tc, templates, remembered, self.mib_apid_hex());
        let id = self.open_page(PageModel::MibForm(Box::new(form)));
        self.mib_origins.insert(id, MibOrigin { buffer, range: at..at });
    }

    /// The text buffer a form's command goes into: the focused one, else
    /// the one used most recently.
    fn mib_insert_target(&self) -> Option<BufferId> {
        let editable = |id: BufferId| !self.is_page_buffer(id) && self.buffers.get(id).is_some_and(|ob| ob.kind.tracks_unsaved_changes());
        let focused = self.focused_buffer_id();
        if editable(focused) {
            return Some(focused);
        }
        self.buffers.mru().iter().copied().find(|&id| editable(id))
    }

    /// Where `buffer`'s cursor is, in a pane showing it, else where it
    /// was left when its tab was last in front.
    fn mib_buffer_cursor(&self, buffer: BufferId) -> usize {
        let ws = self.workspaces.active_workspace();
        ws.windows
            .windows()
            .into_iter()
            .find(|pane| ws.windows.content(*pane) == Some(&buffer))
            .and_then(|pane| self.workspaces.active_pane_states().get(&pane).map(|s| s.cursor.char_idx))
            .or_else(|| self.buffers.get(buffer).map(|ob| ob.cursor.char_idx))
            .unwrap_or(0)
    }

    fn mib_copy(&mut self, text: String) {
        if let Some(clipboard) = &mut self.clipboard {
            let _ = clipboard.set_text(text.clone());
        }
        self.set_message(format!("Copied {text}"));
    }

    fn mib_open_file(&mut self, path: PathBuf, line: usize) {
        self.jump_to_grep_match(&fenix_project::GrepMatch { path, line, col: 1, text: String::new() });
    }

    pub(super) fn mib_page_action(&mut self, action: mib_page::Action) {
        let key = self.mib_focused_key();
        match action {
            mib_page::Action::None => {}
            mib_page::Action::Close => {
                let id = self.focused_buffer_id();
                self.close_page(id);
            }
            mib_page::Action::Open(def) => self.open_mib_def(key, def),
            mib_page::Action::Insert(def) => self.open_mib_form(key, def),
            mib_page::Action::Copy(text) => self.mib_copy(text),
            mib_page::Action::Reload => self.cmd_mib_reload(),
            mib_page::Action::Settings(project) => self.mib_open_settings(project),
            mib_page::Action::OpenFile(path, line) => self.mib_open_file(path, line),
        }
    }

    pub(super) fn mib_def_action(&mut self, action: mib_def::Action) {
        let key = self.mib_focused_key();
        match action {
            mib_def::Action::None => {}
            mib_def::Action::Close => {
                let id = self.focused_buffer_id();
                self.close_page(id);
            }
            mib_def::Action::Insert(def) => self.open_mib_form(key, def),
            mib_def::Action::Copy(text) => self.mib_copy(text),
            mib_def::Action::OpenFile(path, line) => self.mib_open_file(path, line),
            mib_def::Action::MibPage => self.cmd_mib_page(),
        }
    }

    pub(super) fn mib_form_action(&mut self, action: mib_form::Action) {
        let id = self.focused_buffer_id();
        let key = self.mib_focused_key();
        match action {
            mib_form::Action::None => {}
            mib_form::Action::Close => {
                self.mib_origins.remove(&id);
                self.close_page(id);
            }
            mib_form::Action::Open(def) => self.open_mib_def(key, def),
            mib_form::Action::Insert(text) => self.mib_form_insert(id, text),
        }
    }

    /// The form's command goes where it was opened, the form closes, and
    /// the file it went into is back in front with the cursor after it.
    fn mib_form_insert(&mut self, id: BufferId, text: String) {
        let Some(origin) = self.mib_origins.remove(&id) else { return };
        let (tc, project, values) = match self.pages.get(&id).map(|s| &s.model) {
            Some(PageModel::MibForm(form)) => (form.name().to_string(), form.key.project.clone(), form.values()),
            _ => return,
        };
        self.mib_last.insert((project, tc.clone()), values);
        let Some(ob) = self.buffers.get_mut(origin.buffer) else {
            self.set_error("the file the form was opened from is closed");
            return;
        };
        let len = ob.buffer.len_chars();
        let start = origin.range.start.min(len);
        let end = origin.range.end.min(len);
        let mut cursor = Cursor { char_idx: start, sticky_col: 0 };
        if end > start {
            ob.buffer.replace_range(&mut cursor, start, end, &text);
        } else {
            ob.buffer.insert_str(&mut cursor, &text);
        }
        self.close_page(id);
        self.open_buffer_in_focused_pane(origin.buffer);
        let pane = self.focused_pane_id();
        if let Some(state) = self.workspaces.active_pane_states_mut().get_mut(&pane) {
            state.cursor = cursor;
        }
        self.refresh_project_root();
        self.set_message(format!("inserted {tc}"));
        self.wake_caret();
    }

    /// The MIBs of the focused MIB page.
    fn mib_focused_key(&self) -> MibKey {
        match self.pages.get(&self.focused_buffer_id()).map(|s| &s.model) {
            Some(PageModel::Mib(p)) => p.key.clone(),
            Some(PageModel::MibDef(p)) => p.key.clone(),
            Some(PageModel::MibForm(p)) => p.key.clone(),
            _ => self.mib_key_here().0,
        }
    }

    /// The focused page's MIB, for laying it out.
    pub(super) fn mib_page_ctx_set(&mut self, key: &MibKey) -> (Option<Arc<MibSet>>, bool) {
        let set = self.mib_set(key);
        (set, self.mib_loading(key))
    }

    // -- The editor ---------------------------------------------------------

    /// Whether the focused file is one where MIB names mean something.
    fn mib_in_this_file(&self) -> bool {
        if self.is_page_buffer(self.focused_buffer_id()) {
            return false;
        }
        let exts: Vec<String> = match self.mib_project_value("mib.editor_files") {
            Some(fenix_config::Value::List(l)) => l.clone(),
            _ => self.config.mib_editor_files.clone(),
        };
        if exts.is_empty() {
            return true;
        }
        let ext = self.open().buffer.path().and_then(|p| p.extension()).map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
        exts.iter().any(|e| e.trim_start_matches('.').eq_ignore_ascii_case(&ext))
    }

    /// The MIB definition the word under the cursor names.
    fn mib_word_here(&mut self) -> Option<(MibKey, Arc<MibSet>, DefRef)> {
        if !self.mib_in_this_file() {
            return None;
        }
        let (key, _) = self.mib_key_here();
        if key.roots.is_empty() {
            return None;
        }
        let set = self.mib_set(&key)?;
        let (line, col) = self.open().buffer.line_col(&self.cursor());
        let text = self.open().buffer.line(line).to_string();
        let word = words(&text).into_iter().find(|(range, _)| range.contains(&col)).map(|(_, w)| w.to_string())?;
        let def = set.resolve(&word)?;
        Some((key, set, def))
    }

    /// `K` on a MIB name: its card.
    pub(super) fn mib_hover(&mut self) -> Option<String> {
        let (_, set, def) = self.mib_word_here()?;
        Some(mib_hover_text(&set, def, self.mib_apid_hex()))
    }

    /// `gd` on a MIB name: its page. Whether it was one.
    pub(super) fn mib_goto_definition(&mut self) -> bool {
        let Some((key, _, def)) = self.mib_word_here() else { return false };
        let from = JumpEntry { buffer: self.focused_buffer_id(), char_idx: self.cursor().char_idx };
        self.record_jump(from);
        self.open_mib_def(key, def);
        true
    }

    /// Completion's MIB names: telecommands and parameters, with what
    /// they are.
    pub(super) fn mib_completion_items(&mut self) -> Vec<fenix_picker::Candidate<crate::completion::Item>> {
        if !self.mib_in_this_file() {
            return Vec::new();
        }
        let (key, _) = self.mib_key_here();
        let Some(set) = self.mib_set(&key) else { return Vec::new() };
        let mut out = Vec::new();
        for kind in [Kind::Telecommand, Kind::TcParam, Kind::TmParam] {
            for e in set.entries(kind) {
                let mut item = crate::completion::Item::text(e.name.clone(), crate::completion::Source::Mib);
                item.detail = format!("{} · {}", kind.tag(), e.description);
                out.push(fenix_picker::Candidate::new(e.name.clone(), item));
            }
        }
        out
    }
}

/// The words of `line` (letters, digits, `_`), each with the columns it
/// spans.
fn words(line: &str) -> Vec<(std::ops::Range<usize>, &str)> {
    let mut out = Vec::new();
    let mut start = None;
    let chars: Vec<(usize, char)> = line.char_indices().collect();
    for (col, &(byte, c)) in chars.iter().enumerate() {
        let word = c.is_alphanumeric() || c == '_';
        match (word, start) {
            (true, None) => start = Some((col, byte)),
            (false, Some((sc, sb))) => {
                out.push((sc..col, &line[sb..byte]));
                start = None;
            }
            _ => {}
        }
    }
    if let Some((sc, sb)) = start {
        out.push((sc..chars.len(), &line[sb..]));
    }
    out
}

/// What `K` shows on a MIB name.
pub(super) fn mib_hover_text(set: &MibSet, def: DefRef, apid_hex: bool) -> String {
    let e = set.get(def);
    let mut lines = vec![format!("{} {} · {}", def.kind.tag(), e.name, set.root_label(e.root)), e.description.clone()];
    let r = &e.row;
    match def.kind {
        Kind::Telecommand => {
            lines.push(format!("PUS {},{} · APID {} · {}", r.clean("CCF_TYPE"), r.clean("CCF_STYPE"), fenix_mib::types::apid(r.clean("CCF_APID"), apid_hex), r.clean("CCF_SUBSYS")));
            let els: Vec<_> = fenix_mib::detail::tc_elements(set, def).into_iter().filter(|el| el.fixed.is_none() && !el.name.is_empty()).collect();
            if !els.is_empty() {
                lines.push(String::new());
                lines.push("Arguments".to_string());
                for el in els {
                    let what = [el.description.as_str(), el.allowed.as_str()].iter().filter(|s| !s.is_empty()).cloned().collect::<Vec<_>>().join(" · ");
                    lines.push(format!("  {:<10} {what}", el.name));
                }
            }
            lines.push(String::new());
            lines.push("gd opens it · SPC k e edits this call".to_string());
        }
        _ => {
            let cols: Vec<String> = (1..def.kind.headers().len())
                .map(|c| (def.kind.headers()[c], mib_page::cell(set, def, c, apid_hex)))
                .filter(|(_, v)| !v.is_empty())
                .map(|(h, v)| format!("{} {v}", h.to_lowercase()))
                .collect();
            if !cols.is_empty() {
                lines.push(cols.join(" · "));
            }
            lines.push(String::new());
            lines.push("gd opens it".to_string());
        }
    }
    lines.retain(|l| !l.trim().is_empty() || l.is_empty());
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A project with a MIB of its own at `ops/`, listed in its settings
    /// with its own templates, and a Tcl script holding `script`.
    struct Project(PathBuf);
    impl Drop for Project {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn project(name: &str, script: &str) -> (Project, App) {
        let dir = std::env::temp_dir().join(format!("fenix-mib-host-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::create_dir_all(dir.join("ops")).unwrap();
        std::fs::create_dir_all(dir.join(".fenix")).unwrap();
        let dir = fenix_lsp::normalize(std::fs::canonicalize(&dir).unwrap());
        let w = |t: &str, c: &str| std::fs::write(dir.join("ops").join(format!("{t}.dat")), c).unwrap();
        w("ccf", "ZTC08101\tSet heater control mode\t\t\tN\t\t8\t1\t1010\t2\t\t\t\t\tTCS\nZTC17001\tConnection test\t\t\tN\t\t17\t1\t1008\t0\n");
        w("cdf", "ZTC08101\tE\tLine\t8\t0\t0\tPTH00101\t\t\t\nZTC08101\tE\tMode\t8\t8\t0\tPTH00102\t\t\t\n");
        w("cpc", "PTH00101\tHeater line\t3\t4\t\t\t\t\tPRF00017\nPTH00102\tHeater control mode\t2\t8\t\t\t\t\t\t\tPAF00042\t\tAUTO\n");
        w("paf", "PAF00042\tHeater mode\tU\t3\n");
        w("pas", "PAF00042\tOFF\t0\nPAF00042\tON\t1\nPAF00042\tAUTO\t2\n");
        w("prf", "PRF00017\tHeater lines\t\t\t\t1\t\n");
        w("prv", "PRF00017\t1\t8\n");
        w("pcf", "NTH00123\tHeater 3 temperature\t\tdegC\t3\t12\n");
        std::fs::write(
            dir.join(".fenix").join("settings.toml"),
            "[mib]\ntelecommand_template = \"tc::send {mnemo} {arguments}\"\ntelecommand_argument_template = \"-{name} {value}\"\n\n[mib.roots]\nOPS = \"ops\"\n",
        )
        .unwrap();
        let file = dir.join("checkout.tcl");
        std::fs::write(&file, script).unwrap();
        let mut app = App::with_file(Some(file.display().to_string()));
        app.config.mib_roots = vec![("MINE".to_string(), dir.join("elsewhere"))];
        app.config.mib_include_yours = None;
        app.config.mib_telecommand_template = None;
        app.config.mib_telecommand_argument_template = None;
        app.config.mib_telecommand_argument_separator = None;
        app.config.mib_editor_files = Vec::new();
        app.refresh_project_root();
        (Project(dir), app)
    }

    fn set_cursor(app: &mut App, char_idx: usize) {
        let pane = app.focused_pane_id();
        if let Some(state) = app.workspaces.active_pane_states_mut().get_mut(&pane) {
            state.cursor = Cursor { char_idx, sticky_col: 0 };
        }
    }

    fn page_text(app: &mut App) -> String {
        let (id, pane) = (app.focused_buffer_id(), app.focused_pane_id());
        app.ensure_page_layout(id, pane, 160);
        app.open().buffer.text()
    }

    fn press(app: &mut App, keys: &[KeyPress]) {
        for &k in keys {
            assert!(app.page_key(k), "the page claims {k:?}");
        }
    }

    #[test]
    fn a_projects_mibs_take_the_place_of_yours_and_are_relative_to_it() {
        let (p, mut app) = project("replace", "");
        let (key, source) = app.mib_key_here();
        assert_eq!(key.roots, vec![MibRoot { label: "OPS".into(), path: p.0.join("ops") }]);
        assert!(source.contains("settings"), "{source}");
        app.config.mib_include_yours = Some(true);
        assert_eq!(app.mib_key_here().0.roots.len(), 2, "yours added when asked");
    }

    #[test]
    fn the_mib_page_lists_the_projects_definitions_and_opens_one() {
        let (_p, mut app) = project("page", "");
        app.cmd_mib_page();
        let text = page_text(&mut app);
        assert!(text.contains("ZTC08101") && text.contains("ZTC17001"), "{text}");
        assert_eq!(app.buffer_display_name(app.focused_buffer_id()), "*mib*");
        press(&mut app, &[KeyPress::named(FenixNamedKey::Enter)]);
        assert_eq!(app.buffer_display_name(app.focused_buffer_id()), "*mib: ZTC08101*");
        let text = page_text(&mut app);
        assert!(text.contains("PTH00102") && text.contains("OFF ON AUTO"), "{text}");
    }

    #[test]
    fn the_insert_form_writes_the_call_where_the_cursor_was_and_remembers_it() {
        let (_p, mut app) = project("insert", "proc main {} {\n    \n}\n");
        set_cursor(&mut app, 19);
        let script = app.focused_buffer_id();
        let (key, _) = app.mib_key_here();
        let set = app.mib_set(&key).unwrap();
        // From the MIB page, where no file is in front.
        app.cmd_mib_page();
        app.open_mib_form(key, set.find(Kind::Telecommand, 0, "ZTC08101").unwrap());
        assert!(page_text(&mut app).contains("Insert a telecommand"));
        let enter = KeyPress::named(FenixNamedKey::Enter);
        press(&mut app, &[enter, KeyPress::char('3'), enter, KeyPress::char('j'), KeyPress::char('h'), enter.with_ctrl()]);
        assert_eq!(app.focused_buffer_id(), script);
        assert_eq!(app.open().buffer.text(), "proc main {} {\n    tc::send ZTC08101 -PTH00101 3, -PTH00102 ON\n}\n");
        let remembered = app.mib_last.values().next().unwrap();
        assert_eq!(remembered.get("PTH00102@").map(String::as_str), Some("ON"));
    }

    #[test]
    fn edit_call_reads_the_line_back_into_the_form_and_puts_it_back() {
        let (_p, mut app) = project("edit", "proc main {} {\n    tc::send ZTC08101 -PTH00101 3, -PTH00102 ON\n}\n");
        set_cursor(&mut app, 25);
        app.cmd_mib_edit_call();
        assert!(page_text(&mut app).contains("Edit a telecommand call"));
        press(&mut app, &[KeyPress::char('j'), KeyPress::char('l'), KeyPress::named(FenixNamedKey::Enter).with_ctrl()]);
        assert_eq!(app.open().buffer.text(), "proc main {} {\n    tc::send ZTC08101 -PTH00101 3, -PTH00102 AUTO\n}\n");
    }

    #[test]
    fn k_gd_and_completion_know_the_projects_names() {
        let (_p, mut app) = project("editor", "tc::send ZTC08101\n");
        set_cursor(&mut app, 12);
        app.request_hover();
        let card = app.lsp_hover.clone().unwrap_or_default();
        assert!(card.contains("Set heater control mode") && card.contains("PTH00102"), "{card}");
        let names: Vec<String> = app.completion_candidates().into_iter().filter(|c| c.payload.source == crate::completion::Source::Mib).map(|c| c.label).collect();
        assert!(names.contains(&"ZTC17001".to_string()) && names.contains(&"NTH00123".to_string()), "{names:?}");
        let script = app.focused_buffer_id();
        app.request_goto_definition();
        assert_eq!(app.buffer_display_name(app.focused_buffer_id()), "*mib: ZTC08101*");

        // Only in the files it's asked for.
        app.config.mib_editor_files = vec!["py".into()];
        app.open_buffer_in_focused_pane(script);
        set_cursor(&mut app, 12);
        app.lsp_hover = None;
        app.request_hover();
        assert!(app.lsp_hover.as_deref().is_none_or(|h| !h.contains("heater")), "not in a .tcl file now");
    }

    #[test]
    fn a_changed_table_is_read_again_on_the_disk_poll() {
        let (p, mut app) = project("poll", "");
        let (key, _) = app.mib_key_here();
        assert_eq!(app.mib_set(&key).unwrap().entries(Kind::Telecommand).len(), 2);
        let ccf = p.0.join("ops").join("ccf.dat");
        let mut text = std::fs::read_to_string(&ccf).unwrap();
        text.push_str("ZTC20003\tSet on-board parameter\t\t\tN\t\t20\t3\t1009\t0\n");
        std::fs::write(&ccf, text).unwrap();
        app.mib_poll();
        assert_eq!(app.mib_set(&key).unwrap().entries(Kind::Telecommand).len(), 3);
    }

    #[test]
    fn words_carry_their_columns() {
        let w = words("  tc::send ZTC08101 -PTH00101 3");
        assert_eq!(w[0], (2..4, "tc"));
        assert_eq!(w[2], (11..19, "ZTC08101"));
        assert_eq!(w[3], (21..29, "PTH00101"));
    }
}
