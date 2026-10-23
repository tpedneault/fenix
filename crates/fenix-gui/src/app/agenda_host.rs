//! The host half of the agenda page (`agenda_page`): opening it, the
//! context it reads, and carrying out what its keys ask -- editing the
//! store, queuing the change for Jira, opening a compose pane. The Jira
//! side of each change (transitions, the outbox, worklogs) stays in
//! `agenda_sync`.

use super::feature::{page_as, page_as_mut};
use super::pages::PageModel;
use super::*;
use crate::agenda_page::{self, Action as AgendaAction, AgendaPage, NewTask, Project, Tab};
use fenix_agenda::{CodeRef, OpKind, Status};

/// One row of the clock picker (`SPC a t`), or of the question asked
/// when the clock ran on while you were away.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ClockPick {
    Stop,
    Start(TaskId),
    /// Keep the time up to when you were last seen, and stop there.
    KeepUntil(chrono::DateTime<chrono::Local>),
    /// It was all work: carry on.
    KeepAll,
    /// Drop this sitting.
    Drop,
}

/// How often a running clock's last-seen time is saved.
const SEEN_SAVE_EVERY: std::time::Duration = std::time::Duration::from_secs(300);

/// The agenda's settings: `[agenda]` in `settings.toml`, on the page's
/// Jira & agenda section.
static SETTINGS: [fenix_config::Setting; 3] = {
    use fenix_config::{Category::Jira, Kind, Setting};
    [
        Setting::stored("agenda.categories", Jira, "Agenda categories", Kind::List, "Categories offered when you file a task."),
        Setting::stored("agenda.worklog_round", Jira, "Round worklogs to", Kind::Minutes, "Time logged to Jira is rounded to this many minutes.").with_default("15"),
        Setting::stored("agenda.idle_minutes", Jira, "Ask after idle", Kind::Minutes, "With the clock running, how long without a key press before Fenix asks what time to keep. 0 never asks.").with_default("60"),
    ]
};

/// The agenda's commands and its `SPC a` group.
pub(crate) static SPEC: super::feature::FeatureSpec = {
    use super::feature::FeatureCommand as C;
    use super::feature::LeaderEntry::{Bind, Group};
    super::feature::FeatureSpec {
        id: "fenix.agenda",
        name: "Agenda",
        about: "Tasks on a board, and a clock for your time",
        commands: &[
            C { id: "agenda.open", description: "Open the agenda page where it was left", external: true, run: |app| app.cmd_agenda_open() },
            C { id: "agenda.board", description: "Open the agenda on its board", external: true, run: |app| app.cmd_agenda_board() },
            C { id: "agenda.list", description: "Open the agenda on its list", external: true, run: |app| app.cmd_agenda_list() },
            C { id: "agenda.report", description: "Open the agenda on this week's time", external: true, run: |app| app.cmd_agenda_report() },
            C { id: "agenda.new_task", description: "Add a task to the agenda", external: true, run: |app| app.cmd_agenda_new_task() },
            C { id: "agenda.from_here", description: "Add a task about the selection or line here", external: false, run: |app| app.cmd_agenda_task_from_here() },
            C { id: "agenda.find", description: "Search every task", external: true, run: |app| app.cmd_agenda_find() },
            C { id: "agenda.toggle_clock", description: "Stop the clock, resume the last task, or switch to another", external: true, run: |app| app.cmd_agenda_toggle_clock() },
            C { id: "agenda.resume", description: "Start the clock again on what you worked on last", external: true, run: |app| app.cmd_agenda_resume() },
            C { id: "agenda.import", description: "Pick Jira issues assigned to you to add to the agenda", external: true, run: |app| app.cmd_agenda_import() },
            C { id: "agenda.sync", description: "Refresh linked tasks from Jira and send pending changes", external: true, run: |app| app.cmd_agenda_sync() },
            C { id: "agenda.worklogs", description: "Review unsent time on linked tasks and send it to Jira", external: true, run: |app| app.cmd_agenda_worklogs() },
        ],
        leader: &[
            Group(&['a'], "agenda"),
            Bind(&['a', 'a'], "open agenda", "agenda.open"),
            Bind(&['a', 'b'], "agenda: board", "agenda.board"),
            // The board's old key, kept for a release.
            Bind(&['a', 'k'], "agenda: board (now SPC a b)", "agenda.board"),
            Bind(&['a', 'l'], "agenda: list", "agenda.list"),
            Bind(&['a', 'r'], "agenda: this week's time", "agenda.report"),
            Bind(&['a', 'n'], "agenda: new task", "agenda.new_task"),
            Bind(&['a', 'h'], "agenda: task from here", "agenda.from_here"),
            Bind(&['a', '/'], "agenda: find a task", "agenda.find"),
            Bind(&['a', 't'], "agenda: clock -- stop, resume, switch", "agenda.toggle_clock"),
            Bind(&['a', 'T'], "agenda: resume the clock", "agenda.resume"),
            // The Jira side: pick which of your issues to track, refresh the
            // ones you track, and review/send time as worklogs.
            Bind(&['a', 'i'], "agenda: import Jira issues", "agenda.import"),
            Bind(&['a', 's'], "agenda: sync with Jira", "agenda.sync"),
            Bind(&['a', 'w'], "agenda: review worklogs", "agenda.worklogs"),
        ],
        local_menu: &[],
        local_context: None,
        language_server: None,
        tasks: None,
        hover_fallback: None,
        completion_at_cursor: None,
        completion_pool_at: None,
        hover_first: None,
        picker_confirm: Some(|app, pick, _marked, _query| {
            if let Some(pick) = pick.downcast_ref::<ClockPick>() {
                app.agenda_clock_pick(pick.clone());
            }
        }),
        picker_cancel: None,
        modeline_clock: None,
        // A key on the agenda page, handled against the store.
        page_action: Some(|app, id, key| {
            if let Ok(key) = key.downcast::<crate::page::Key>() {
                app.agenda_page_key_now(id, *key);
            }
        }),
        page_context: Some(agenda_page_context),
        page_title: None,
        page_after_layout: None,
        on_event: None,
        settings: &SETTINGS,
        services: &[],
        contributes: &[],
        session_documents: None,
        session_reopen: None,
        home: Some(|app, _| app.agenda_home()),
        home_activate: Some(|app, _| {
            app.open_agenda_page(Some(crate::agenda_page::Tab::Today));
        }),
        doctor: None,
        opener: None,
        lsp_first: None,
        prompt_key: None,
        wake: None,
        links: None,
    }
};

/// The agenda's state: the store and its file, the prompt and the
/// pickers' context, and the Jira sync's bookkeeping. Its code is still
/// `App` methods (its page is a core `PageModel`), so it reaches this by
/// type (`Features::agenda`); every entry point is a spec function, which
/// runs with it in place, and its only instance hook, `picker_label`,
/// only reads.
pub(crate) struct AgendaState {
    /// The personal task/time-tracking agenda's own data (`SPC a`) --
    /// loaded at startup from `path`, read again when another writer
    /// changes the file (`reload_agenda_if_changed`), and saved back after
    /// every mutation (create/status/priority/category/note/subtask/
    /// dependency/time), the same "save immediately, not on quit" posture
    /// `Config` already has. There's no credential/setup step, just a
    /// local JSON file.
    pub(super) store: fenix_agenda::AgendaStore,
    /// `fenix_agenda::default_path()`, resolved once at startup.
    pub(super) path: std::path::PathBuf,
    /// `path`'s modification time as Fenix last read or wrote it: a
    /// different one on the disk poll's tick means someone else wrote the
    /// file (another machine, through a roaming profile).
    pub(super) stamp: Option<std::time::SystemTime>,
    /// Linking a task to an issue typed by key, or naming the issue type
    /// of one being created from it.
    pub(super) prompt: Option<AgendaPrompt>,
    /// What `SPC a h` captured for the new-task form it opened.
    pub(super) capture: Option<Capture>,
    /// The clock picker open is asking about time away, not switching.
    pub(super) gap_asked: bool,
    /// When the running clock's last-seen time was last saved.
    pub(super) seen_saved: Option<Instant>,
    /// Worklog review edits and drops, by (task, day). Reset each time
    /// the review is opened.
    pub(super) worklog_edits: HashMap<(TaskId, chrono::NaiveDate), i64>,
    pub(super) worklog_dropped: HashSet<(TaskId, chrono::NaiveDate)>,
    /// The minute each agenda page was last laid out in, while a clock
    /// runs.
    pub(super) page_minutes: HashMap<BufferId, i64>,
}

impl Default for AgendaState {
    /// Read-only, same posture as `known_projects`/`recent_files`: a test
    /// that constructs `App` via `with_file` reads whatever real
    /// `agenda.json` happens to exist on the machine running the suite,
    /// but nothing here ever writes -- only a later call to a mutating
    /// agenda method (`save_agenda`) does that, the same "load freely,
    /// save deliberately" split `config`/`known_projects` already have. A
    /// test that needs to exercise a save reassigns `path` first,
    /// mirroring how existing tests reassign `app.config`.
    fn default() -> Self {
        let path = fenix_agenda::default_path().unwrap_or_else(|| PathBuf::from("fenix-agenda.json"));
        Self {
            store: fenix_agenda::load(&path),
            stamp: std::fs::metadata(&path).and_then(|m| m.modified()).ok(),
            path,
            prompt: None,
            capture: None,
            gap_asked: false,
            seen_saved: None,
            worklog_edits: HashMap::new(),
            worklog_dropped: HashSet::new(),
            page_minutes: HashMap::new(),
        }
    }
}

#[derive(Default)]
pub(crate) struct AgendaHost {
    pub(super) state: AgendaState,
}

impl super::feature::Feature for AgendaHost {
    fn spec(&self) -> &'static super::feature::FeatureSpec {
        &SPEC
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

impl super::feature::Features {
    pub(super) fn agenda(&self) -> &AgendaState {
        &self.get::<AgendaHost>().expect("the agenda has no instance hooks that run code, so it's always in place").state
    }

    pub(super) fn agenda_mut(&mut self) -> &mut AgendaState {
        &mut self.get_mut::<AgendaHost>().expect("the agenda has no instance hooks that run code, so it's always in place").state
    }
}

/// What the agenda page shows, laid out here: it reads the store, which
/// the page can't be handed. A running clock's minute moving on lays it
/// out again too.
struct AgendaLayout {
    page: Option<crate::page::Page>,
    tick: bool,
}

fn agenda_page_context(app: &mut App, id: BufferId, need: super::feature::PageNeed) -> Box<dyn std::any::Any> {
    let super::feature::PageNeed::Layout { cols, due } = need else { return Box::new(()) };
    let minute = chrono::Local::now().timestamp() / 60;
    let running = app.features.agenda().store.active_timer.is_some();
    let tick = running && app.features.agenda_mut().page_minutes.insert(id, minute) != Some(minute);
    if !due && !tick {
        return Box::new(AgendaLayout { page: None, tick: false });
    }
    let (worklogs, sync, round) = (app.agenda_worklog_rows(), app.agenda_sync_label(), app.agenda_worklog_round());
    let page = app.feature_page::<AgendaPage>(id).map(|p| {
        let ctx = agenda_page::Ctx { store: &app.features.agenda().store, now: chrono::Local::now(), categories: app.config.list_value("agenda.categories"), worklogs: &worklogs, round, sync };
        agenda_page::layout(p, &ctx, cols)
    });
    Box::new(AgendaLayout { page, tick })
}

/// The agenda page (`SPC a a`).
impl super::feature::FeaturePage for AgendaPage {
    fn feature(&self) -> &'static str {
        SPEC.id
    }

    fn title(&self) -> String {
        "*agenda*".to_string()
    }

    fn typing(&self) -> bool {
        AgendaPage::typing(self)
    }

    fn claims_space(&self) -> bool {
        AgendaPage::claims_space(self)
    }

    fn paste(&mut self, text: &str) {
        AgendaPage::paste(self, text);
    }

    fn changed(&self, ctx: &dyn std::any::Any) -> bool {
        ctx.downcast_ref::<AgendaLayout>().is_some_and(|l| l.tick)
    }

    /// Laid out already, by `agenda_page_context`.
    fn layout(&mut self, ctx: Box<dyn std::any::Any>, _cols: usize) -> crate::page::Page {
        ctx.downcast::<AgendaLayout>().ok().and_then(|l| l.page).unwrap_or_default()
    }

    /// The key itself: the agenda's `page_action` handles it against the
    /// store.
    fn key(&mut self, key: crate::page::Key, _ctx: Box<dyn std::any::Any>) -> Box<dyn std::any::Any> {
        Box::new(key)
    }
}

/// An agenda picker over `candidates`, with the badge `badge`.
pub(super) fn agenda_picker<T: std::any::Any>(badge: &'static str, candidates: Vec<fenix_picker::Candidate<T>>) -> ActivePicker {
    let candidates = candidates.into_iter().map(|c| super::feature::FeaturePick::candidate(c.label, c.payload)).collect();
    ActivePicker::Feature { feature: SPEC.id, badge, state: fenix_picker::PickerState::new(candidates) }
}

/// The agenda's `agenda_sync` contribution point: what keeps linked tasks
/// in step with an issue tracker (Jira's `agenda_sync::SYNC`). The
/// agenda owns the store, its outbox and the worklog batch; the sync
/// pulls, sends, and runs the pickers that need the tracker. Without
/// one the agenda is local.
pub(crate) const AGENDA_SYNC: &str = "agenda_sync";

/// What a sync plugs into `agenda_sync`.
pub(crate) struct AgendaSync {
    /// At startup: whatever should run while Fenix does.
    pub start: fn(&mut App),
    /// `SPC a s`: refresh linked tasks and send what's waiting; `quiet`
    /// keeps a failure from shouting.
    pub sync: fn(&mut App, bool),
    /// Opening the agenda: refresh unless it just did.
    pub sync_if_stale: fn(&mut App),
    /// Send whatever an edit queued in the outbox.
    pub send: fn(&mut App),
    /// "synced 3m ago", for the page.
    pub status: fn(&App) -> String,
    /// What it's doing in the background, for the modeline.
    pub busy: fn(&App) -> Option<String>,
    /// Move a linked task to a status, through the tracker's workflow.
    pub move_linked: fn(&mut App, TaskId, Status),
    /// The pickers a linked task's keys open.
    pub pick_status: fn(&mut App, TaskId),
    pub pick_priority: fn(&mut App, TaskId),
    pub pick_assignee: fn(&mut App, TaskId),
    /// Link a task (or unlink/open it, when it's linked).
    pub link: fn(&mut App, TaskId),
    /// Link a task to the issue with this key.
    pub link_key: fn(&mut App, TaskId, String),
    /// Clear a failed send and try again.
    pub retry: fn(&mut App, TaskId),
    /// `SPC a i`: pick issues to add.
    pub import: fn(&mut App),
    /// A linked task's issue, on the web.
    pub issue_url: fn(&mut App, TaskId) -> Option<String>,
    /// Send the worklog review's rows.
    pub send_worklogs: fn(&mut App),
}

impl App {
    /// The agenda's sync, when a feature plugs one in.
    pub(super) fn agenda_sync(&self) -> Option<&'static AgendaSync> {
        self.contributions(AGENDA_SYNC).find_map(|c| c.downcast_ref::<AgendaSync>())
    }

    /// Runs `f` with the agenda's sync, or says there isn't one.
    fn agenda_with_sync(&mut self, f: impl FnOnce(&mut App, &'static AgendaSync)) {
        match self.agenda_sync() {
            Some(sync) => f(self, sync),
            None => self.set_message("Nothing syncs the agenda -- that needs Jira switched on"),
        }
    }

    /// At startup.
    pub(super) fn agenda_sync_start(&mut self) {
        if let Some(sync) = self.agenda_sync() {
            (sync.start)(self);
        }
    }

    /// Saves, re-renders, and has the sync send whatever the edit queued.
    pub(super) fn agenda_after_edit(&mut self) {
        self.agenda_save_and_refresh();
        if let Some(sync) = self.agenda_sync() {
            (sync.send)(self);
        }
    }

    /// `SPC a s`.
    pub(crate) fn cmd_agenda_sync(&mut self) {
        self.agenda_with_sync(|app, sync| (sync.sync)(app, false));
    }

    /// `SPC a i`.
    pub(crate) fn cmd_agenda_import(&mut self) {
        self.agenda_with_sync(|app, sync| (sync.import)(app));
    }

    /// Links `task` to the issue `key`.
    pub(super) fn agenda_link_key(&mut self, task: TaskId, key: String) {
        self.agenda_with_sync(|app, sync| (sync.link_key)(app, task, key));
    }

    /// `r` on a task: clears a failed send and tries again.
    pub(super) fn agenda_retry(&mut self, task: TaskId) {
        self.agenda_with_sync(|app, sync| (sync.retry)(app, task));
    }

    /// Moves a task to `target` -- the one path `s`, the board's `H`/`L`
    /// and the status picker all take. A local task just moves; a linked
    /// one goes through the sync, which works out the tracker's side.
    pub(super) fn agenda_set_status(&mut self, id: TaskId, target: Status) {
        let Some(task) = self.features.agenda().store.task(id) else { return };
        match (task.jira.is_some(), self.agenda_sync()) {
            (true, Some(sync)) => (sync.move_linked)(self, id, target),
            _ => {
                self.features.agenda_mut().store.move_to_status(id, target);
                self.agenda_save_and_refresh();
            }
        }
    }

    /// `y` on a linked task.
    pub(super) fn agenda_copy_link(&mut self, id: TaskId) {
        let Some(url) = self.agenda_sync().and_then(|sync| (sync.issue_url)(self, id)) else { return };
        if let Some(clipboard) = &mut self.clipboard {
            let _ = clipboard.set_text(url.clone());
        }
        self.set_message(format!("Copied: {url}"));
    }

    /// `o` on a linked task.
    pub(super) fn agenda_open_in_browser(&mut self, id: TaskId) {
        if let Some(url) = self.agenda_sync().and_then(|sync| (sync.issue_url)(self, id)) {
            self.open_url(&url);
        }
    }

    /// `SPC a w`: the worklog review, fresh, over the Time tab.
    pub(crate) fn cmd_agenda_worklogs(&mut self) {
        let id = self.open_agenda_page(Some(crate::agenda_page::Tab::Time));
        if let Some(page) = self.agenda_page_mut(id) {
            page.open_worklogs();
        }
    }

    pub(super) fn agenda_worklog_round(&self) -> u32 {
        self.config.int_value("agenda.worklog_round").map_or(15, |m| m as u32)
    }

    /// The review's rows: the store's batch minus drops, with edits.
    pub(super) fn agenda_worklog_rows(&self) -> Vec<fenix_agenda::WorklogRow> {
        let mut rows = self.features.agenda().store.worklog_batch(self.agenda_worklog_round());
        rows.retain(|r| !self.features.agenda().worklog_dropped.contains(&(r.task, r.date)));
        for row in &mut rows {
            if let Some(&minutes) = self.features.agenda().worklog_edits.get(&(row.task, row.date)) {
                row.minutes = minutes;
            }
        }
        rows
    }
}

/// What `SPC a h` captured, waiting for the new-task form to finish.
pub(super) struct Capture {
    pub(super) code: CodeRef,
    pub(super) text: String,
}

impl App {
    pub(super) fn agenda_page_mut(&mut self, id: BufferId) -> Option<&mut AgendaPage> {
        self.feature_page_mut::<AgendaPage>(id)
    }

    fn agenda_page_id(&self) -> Option<BufferId> {
        self.find_page(|m| page_as::<AgendaPage>(m).is_some())
    }

    /// The project the focused file is in, and its Jira key.
    fn agenda_project(&self) -> Option<Project> {
        let root = self.project_root.as_ref()?;
        let name = root.file_name()?.to_string_lossy().into_owned();
        Some(Project { name, jira_key: fenix_project::meta::jira_key(root) })
    }

    /// What the agenda's sync says about itself ("synced 3m ago"), or
    /// nothing without one.
    pub(super) fn agenda_sync_label(&self) -> String {
        self.agenda_sync().map(|sync| (sync.status)(self)).unwrap_or_default()
    }

    /// `SPC a a` and friends: the agenda page, on `tab` when given, else
    /// where it was left.
    pub(crate) fn open_agenda_page(&mut self, tab: Option<Tab>) -> BufferId {
        if let Some(sync) = self.agenda_sync() {
            (sync.sync_if_stale)(self);
        }
        let project = self.agenda_project();
        let id = match self.agenda_page_id() {
            Some(id) => {
                self.show_page(id);
                if let Some(page) = self.agenda_page_mut(id) {
                    if project.is_some() {
                        page.project = project;
                    }
                    if let Some(tab) = tab {
                        page.show_tab(tab);
                    }
                    page.note = None;
                }
                id
            }
            None => self.open_page(PageModel::Feature(Box::new(AgendaPage::new(tab.unwrap_or(Tab::Today), project)))),
        };
        self.agenda_worklog_reset();
        id
    }

    /// Every agenda page, laid out again -- after anything changed the
    /// store, from here or from a sync.
    pub(super) fn refresh_agenda_pages(&mut self) {
        let ids: Vec<BufferId> = self.pages.iter().filter(|(_, s)| page_as::<AgendaPage>(&s.model).is_some()).map(|(id, _)| *id).collect();
        for id in ids {
            let store = &self.features.agenda().store;
            if let Some(state) = self.pages.get_mut(&id) {
                state.stale = true;
                if let Some(p) = page_as_mut::<AgendaPage>(&mut state.model) {
                    p.forget_missing(store);
                }
            }
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    /// `SPC a n`: the page, with the new-task form open.
    pub(crate) fn cmd_agenda_new_task(&mut self) {
        self.features.agenda_mut().capture = None;
        let id = self.open_agenda_page(None);
        self.agenda_page_key_now(id, crate::page::Key::Char('n'));
    }

    /// `SPC a h`: a task from here -- the selection just made (or the
    /// line the cursor's on) becomes its description, and the task keeps
    /// the file and line to come back to.
    pub(crate) fn cmd_agenda_task_from_here(&mut self) {
        let Some(path) = self.open().buffer.path().map(Path::to_path_buf) else {
            self.set_error("SPC a h makes a task from a file -- this buffer isn't one");
            return;
        };
        let cursor = self.cursor();
        let text: Vec<char> = self.open().buffer.text().chars().collect();
        let (line, _) = self.open().buffer.line_col(&cursor);
        let line_range = |at: usize| {
            let start = text[..at.min(text.len())].iter().rposition(|c| *c == '\n').map(|i| i + 1).unwrap_or(0);
            let end = text[at.min(text.len())..].iter().position(|c| *c == '\n').map(|i| at + i).unwrap_or(text.len());
            (start, end)
        };
        let range = match self.vim.visual_selection_range(&self.open().buffer, &cursor) {
            Some((range, _)) => (range.start, range.end),
            None => match self.vim.last_visual_range() {
                // The selection just left for the leader: the cursor is still in it.
                Some((a, b)) if (a..=b).contains(&cursor.char_idx) => (line_range(a).0, line_range(b).1),
                _ => line_range(cursor.char_idx),
            },
        };
        let first_line = text[..range.0.min(text.len())].iter().filter(|c| **c == '\n').count() + 1;
        let body: String = text[range.0.min(text.len())..range.1.min(text.len())].iter().collect();
        self.vim.exit_visual_mode(&cursor);
        let lang = path.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default();
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let text = format!("{name}:{first_line}\n\n```{lang}\n{}\n```", body.trim_end());
        let _ = line;
        self.features.agenda_mut().capture = Some(Capture { code: CodeRef { path, line: first_line }, text });
        let id = self.open_agenda_page(None);
        self.agenda_page_key_now(id, crate::page::Key::Char('n'));
    }

    /// `SPC a t`: stop the running clock, resume what you worked on last,
    /// or switch -- recent tasks first, then what's in progress.
    pub(crate) fn cmd_agenda_toggle_clock(&mut self) {
        let store = &self.features.agenda().store;
        let running = store.active_timer.as_ref().map(|t| t.task_id);
        let label = |t: &fenix_agenda::Task| match t.jira_key() {
            Some(key) => format!("{key} {}", t.title),
            None => t.title.clone(),
        };
        let mut candidates: Vec<fenix_picker::Candidate<ClockPick>> = Vec::new();
        if let Some(timer) = &store.active_timer {
            let name = store.task(timer.task_id).map(label).unwrap_or_default();
            let minutes = (chrono::Local::now() - timer.started_at).num_minutes();
            candidates.push(fenix_picker::Candidate::new(format!("Stop  ·  {name}  ·  {}", agenda_page::format_minutes(minutes)), ClockPick::Stop));
        }
        let mut seen: Vec<TaskId> = running.into_iter().collect();
        let mut add = |t: &fenix_agenda::Task, prefix: &str, candidates: &mut Vec<fenix_picker::Candidate<ClockPick>>| {
            if seen.contains(&t.id) {
                return;
            }
            seen.push(t.id);
            candidates.push(fenix_picker::Candidate::new(format!("{prefix}{}", label(t)), ClockPick::Start(t.id)));
        };
        for (i, (id, _)) in store.recently_worked().into_iter().enumerate() {
            if let Some(t) = store.task(id).filter(|t| t.status != Status::Done) {
                let prefix = if i == 0 && running.is_none() { "Resume  ·  " } else { "" };
                add(t, prefix, &mut candidates);
            }
        }
        let mut rest: Vec<&fenix_agenda::Task> = store.tasks.iter().filter(|t| !t.archived && t.status != Status::Done).collect();
        rest.sort_by_key(|t| (t.status != Status::InProgress, std::cmp::Reverse(t.priority), t.order));
        for t in rest {
            add(t, "", &mut candidates);
        }
        if candidates.is_empty() {
            self.set_error("no agenda tasks to clock in on yet -- SPC a n to add one");
            return;
        }
        self.features.agenda_mut().gap_asked = false;
        self.enter_picker(agenda_picker("CLOCK", candidates));
    }

    /// `SPC a T`: the clock back on what you worked on last.
    pub(crate) fn cmd_agenda_resume(&mut self) {
        if let Some(timer) = &self.features.agenda().store.active_timer {
            let name = self.features.agenda().store.task(timer.task_id).map(|t| t.title.clone()).unwrap_or_default();
            self.set_message(format!("The clock is already on \"{name}\""));
            return;
        }
        let Some((id, _)) = self.features.agenda().store.recently_worked().into_iter().next() else {
            self.set_error("nothing worked on yet to resume -- SPC a t picks a task");
            return;
        };
        self.agenda_clock_pick(ClockPick::Start(id));
    }

    pub(super) fn agenda_clock_pick(&mut self, pick: ClockPick) {
        self.features.agenda_mut().gap_asked = false;
        match pick {
            ClockPick::Stop => self.features.agenda_mut().store.clock_out(),
            ClockPick::Start(id) => {
                self.features.agenda_mut().store.clock_in(id);
                let name = self.features.agenda().store.task(id).map(|t| t.title.clone()).unwrap_or_default();
                self.set_message(format!("Clock on \"{name}\""));
            }
            ClockPick::KeepUntil(at) => {
                self.features.agenda_mut().store.clock_out_at(at);
                self.set_message(format!("Kept the time until {} -- SPC a T starts the clock again", at.format("%H:%M")));
            }
            ClockPick::KeepAll => {
                if let Some(timer) = &mut self.features.agenda_mut().store.active_timer {
                    timer.last_seen = Some(chrono::Local::now());
                }
            }
            ClockPick::Drop => {
                self.features.agenda_mut().store.drop_timer();
                self.set_message("Dropped that sitting -- SPC a T starts the clock again");
            }
        }
        self.features.agenda_mut().seen_saved = Some(std::time::Instant::now());
        self.agenda_save_and_refresh();
    }

    /// Called on every key press: notes that you're here while the clock
    /// runs, and when you've been away longer than `agenda.idle_minutes`,
    /// asks what to keep instead -- returns true when it asked (that key
    /// is spent on bringing the question up).
    pub(super) fn agenda_note_activity(&mut self) -> bool {
        let Some(timer) = &self.features.agenda().store.active_timer else { return false };
        if self.features.agenda().gap_asked {
            return false;
        }
        let now = chrono::Local::now();
        // Not knowing when you were last here (a clock started before
        // Fenix kept track) is no reason to ask: you're here now.
        let last = timer.last_seen.map(|seen| seen.max(timer.started_at));
        let idle = self.config.int_value("agenda.idle_minutes").map_or(60, |m| m as u32);
        if let Some(last) = last.filter(|last| idle > 0 && now - *last > chrono::Duration::minutes(i64::from(idle))) {
            self.agenda_ask_about_gap(last);
            return true;
        }
        if let Some(timer) = &mut self.features.agenda_mut().store.active_timer {
            timer.last_seen = Some(now);
        }
        if self.features.agenda().seen_saved.is_none_or(|at| at.elapsed() >= SEEN_SAVE_EVERY) {
            self.features.agenda_mut().seen_saved = Some(std::time::Instant::now());
            self.save_agenda();
        }
        false
    }

    fn agenda_ask_about_gap(&mut self, last: chrono::DateTime<chrono::Local>) {
        let Some(timer) = &self.features.agenda().store.active_timer else { return };
        let now = chrono::Local::now();
        let name = self.features.agenda().store.task(timer.task_id).map(|t| t.title.clone()).unwrap_or_default();
        let kept = agenda_page::format_minutes((last - timer.started_at).num_minutes());
        let all = agenda_page::format_minutes((now - timer.started_at).num_minutes());
        let candidates = vec![
            fenix_picker::Candidate::new(format!("Keep until {}  ·  {kept}", last.format("%H:%M")), ClockPick::KeepUntil(last)),
            fenix_picker::Candidate::new(format!("Keep all of it  ·  {all}"), ClockPick::KeepAll),
            fenix_picker::Candidate::new("Drop this sitting", ClockPick::Drop),
        ];
        self.set_message(format!(
            "The clock ran on \"{name}\" while you were away -- nothing happened in Fenix from {} to {}",
            last.format("%H:%M"),
            now.format("%H:%M")
        ));
        self.features.agenda_mut().gap_asked = true;
        self.enter_picker(agenda_picker("AWAY", candidates));
    }

    /// `SPC a /`: the page, searching.
    pub(crate) fn cmd_agenda_find(&mut self) {
        let id = self.open_agenda_page(None);
        self.agenda_page_key_now(id, crate::page::Key::Char('/'));
    }

    /// Presses `key` on agenda page `id`, as if typed.
    pub(super) fn agenda_page_key_now(&mut self, id: BufferId, key: crate::page::Key) {
        let worklogs = self.agenda_worklog_rows();
        let sync = self.agenda_sync_label();
        let round = self.agenda_worklog_round();
        let Some(state) = self.pages.get_mut(&id) else { return };
        state.stale = true;
        let Some(p) = page_as_mut::<AgendaPage>(&mut state.model) else { return };
        let ctx = agenda_page::Ctx { store: &self.features.agenda().store, now: chrono::Local::now(), categories: self.config.list_value("agenda.categories"), worklogs: &worklogs, round, sync };
        let action = p.key(key, &ctx);
        self.agenda_page_action(id, action);
    }

    /// Starts the worklog review afresh: no edits or drops carried over.
    pub(super) fn agenda_worklog_reset(&mut self) {
        self.features.agenda_mut().worklog_edits.clear();
        self.features.agenda_mut().worklog_dropped.clear();
    }

    /// Adds `name` to the categories, if it isn't one.
    fn agenda_ensure_category(&mut self, name: &str) {
        let mut categories = self.config.list_value("agenda.categories").to_vec();
        if categories.iter().any(|c| c.eq_ignore_ascii_case(name)) {
            return;
        }
        categories.push(name.to_string());
        self.config.set_value("agenda.categories", Some(fenix_config::Value::List(categories)));
        if let Err(err) = self.config.save() {
            self.set_error(format!("couldn't save settings.toml: {err}"));
        }
    }

    fn agenda_create(&mut self, id: BufferId, spec: NewTask) {
        let capture = self.features.agenda_mut().capture.take();
        let description = capture.as_ref().map(|c| c.text.clone()).unwrap_or_default();
        if let Some(category) = &spec.category {
            self.agenda_ensure_category(category);
        }
        let category = spec.category.as_ref().and_then(|c| self.config.list_value("agenda.categories").iter().find(|x| x.eq_ignore_ascii_case(c)).cloned());
        let task = self.features.agenda_mut().store.create_task(spec.title.clone(), description, spec.priority, category);
        if spec.status != Status::Todo {
            self.features.agenda_mut().store.move_to_status(task, spec.status);
        }
        self.features.agenda_mut().store.set_due(task, spec.due);
        if let Some(capture) = capture {
            self.features.agenda_mut().store.set_code(task, Some(capture.code));
        }
        if spec.clock {
            self.features.agenda_mut().store.clock_in(task);
        }
        self.agenda_save_and_refresh();
        if let Some(page) = self.agenda_page_mut(id) {
            page.select(task);
            page.note = Some((format!("added \"{}\"", spec.title), false));
        }
        if let Some(key) = spec.link {
            self.agenda_link_key(task, key);
        }
    }

    pub(super) fn agenda_page_action(&mut self, id: BufferId, action: AgendaAction) {
        use AgendaAction as A;
        match action {
            A::None => {}
            A::Close => self.close_page(id),
            A::SetStatus(task, status) => self.agenda_set_status(task, status),
            A::JiraStatus(task) => self.agenda_with_sync(|app, sync| (sync.pick_status)(app, task)),
            A::SetPriority(task, priority) => {
                self.features.agenda_mut().store.set_priority(task, priority);
                self.agenda_save_and_refresh();
            }
            A::JiraPriority(task) => self.agenda_with_sync(|app, sync| (sync.pick_priority)(app, task)),
            A::SetCategory(task, category) => {
                self.features.agenda_mut().store.set_category(task, category);
                self.agenda_save_and_refresh();
            }
            A::AddCategory(task, name) => {
                self.agenda_ensure_category(&name);
                self.features.agenda_mut().store.set_category(task, Some(name));
                self.agenda_save_and_refresh();
            }
            A::SetDue(task, due) => {
                self.features.agenda_mut().store.set_due(task, due);
                self.features.agenda_mut().store.enqueue(task, OpKind::SetDue(due.map(|d| d.format("%Y-%m-%d").to_string())));
                self.agenda_after_edit();
            }
            A::SetTitle(task, title) => {
                self.features.agenda_mut().store.set_title(task, title.clone());
                self.features.agenda_mut().store.enqueue(task, OpKind::SetSummary(title));
                self.agenda_after_edit();
            }
            A::EditDescription(task) => {
                let seed = self.features.agenda().store.task(task).map(|t| t.description.clone()).unwrap_or_default();
                self.open_compose_seeded(ComposePurpose::TaskDescription { task }, &seed);
            }
            A::AddNote(task, text) => {
                self.features.agenda_mut().store.add_note(task, text);
                self.agenda_save_and_refresh();
            }
            A::EditNote(task, i, text) => {
                self.features.agenda_mut().store.edit_note(task, i, text);
                self.agenda_save_and_refresh();
            }
            A::RemoveNote(task, i) => {
                self.features.agenda_mut().store.remove_note(task, i);
                self.agenda_save_and_refresh();
            }
            A::Comment(task) => self.open_compose_seeded(ComposePurpose::TaskComment { task }, ""),
            A::PostNote(task, i) => {
                let Some(t) = self.features.agenda().store.task(task) else { return };
                let (Some(key), Some(note)) = (t.jira_key().map(str::to_string), t.notes.get(i).map(|n| n.text.clone())) else { return };
                self.features.agenda_mut().store.enqueue(task, OpKind::AddComment(note));
                self.set_message(format!("Posting the note to {key} as a comment..."));
                self.agenda_after_edit();
            }
            A::Clock(task) => self.agenda_clock_toggle(task),
            A::LogSpan(task, start, end) => {
                self.features.agenda_mut().store.log_span(task, start, end);
                self.agenda_save_and_refresh();
            }
            A::EditTime(task, i, start, end) => {
                if !self.features.agenda_mut().store.edit_time_entry(task, i, start, end) {
                    self.set_error("the end has to be after the start");
                }
                self.agenda_save_and_refresh();
            }
            A::RemoveTime(task, i) => {
                self.features.agenda_mut().store.remove_time_entry(task, i);
                self.agenda_save_and_refresh();
            }
            A::AddCheck(task, text) => {
                self.features.agenda_mut().store.add_subtask(task, text);
                self.agenda_save_and_refresh();
            }
            A::EditCheck(task, i, text) => {
                self.features.agenda_mut().store.edit_subtask(task, i, text);
                self.agenda_save_and_refresh();
            }
            A::ToggleCheck(task, i) => {
                self.features.agenda_mut().store.toggle_subtask(task, i);
                self.agenda_save_and_refresh();
            }
            A::RemoveCheck(task, i) => {
                self.features.agenda_mut().store.remove_subtask(task, i);
                self.agenda_save_and_refresh();
            }
            A::AddDependency(task, on) => {
                if !self.features.agenda_mut().store.add_dependency(task, on) {
                    self.set_error("that would make the two wait on each other");
                }
                self.agenda_save_and_refresh();
            }
            A::RemoveDependency(task, on) => {
                self.features.agenda_mut().store.remove_dependency(task, on);
                self.agenda_save_and_refresh();
            }
            A::Resolve(task, field, mine) => {
                self.features.agenda_mut().store.resolve_conflict(task, field, mine);
                self.agenda_after_edit();
            }
            A::Link(task) => self.agenda_with_sync(|app, sync| (sync.link)(app, task)),
            A::Assign(task) => self.agenda_with_sync(|app, sync| (sync.pick_assignee)(app, task)),
            A::CopyLink(task) => self.agenda_copy_link(task),
            A::Browser(task) => self.agenda_open_in_browser(task),
            A::Retry(task) => self.agenda_retry(task),
            A::OpenCode(task) => {
                let Some(code) = self.features.agenda().store.task(task).and_then(|t| t.code.clone()) else { return };
                if !code.path.exists() {
                    self.set_error(format!("{} isn't there any more", code.path.display()));
                    return;
                }
                self.open_file_from_picker(&code.path);
                self.jump_to_grep_match(&fenix_project::GrepMatch { path: code.path, line: code.line, col: 1, text: String::new() });
            }
            A::Archive(task) => {
                self.features.agenda_mut().store.archive(task);
                self.agenda_save_and_refresh();
            }
            A::Delete(task) => {
                let key = self.features.agenda().store.task(task).and_then(|t| t.jira_key()).map(str::to_string);
                self.features.agenda_mut().store.delete(task);
                self.agenda_save_and_refresh();
                self.set_message(match key {
                    Some(key) => format!("Deleted -- {key} in Jira isn't affected"),
                    None => "Deleted".to_string(),
                });
            }
            A::Reorder(task, by) => {
                self.features.agenda_mut().store.reorder_within_status(task, by);
                self.agenda_save_and_refresh();
            }
            A::Create(spec) => self.agenda_create(id, spec),
            A::Sync => self.cmd_agenda_sync(),
            A::WorklogMinutes(task, date, minutes) => {
                self.features.agenda_mut().worklog_edits.insert((task, date), minutes);
                self.refresh_agenda_pages();
            }
            A::WorklogDrop(task, date) => {
                self.features.agenda_mut().worklog_dropped.insert((task, date));
                self.refresh_agenda_pages();
            }
            A::WorklogDismiss(task, date) => {
                let Some(row) = self.agenda_worklog_rows().into_iter().find(|r| r.task == task && r.date == date) else { return };
                self.features.agenda_mut().store.mark_sent(task, &row.entries);
                self.agenda_save_and_refresh();
                self.set_message(format!("{} on {}: dismissed, won't be sent", row.key, row.date));
            }
            A::SendWorklogs => self.agenda_with_sync(|app, sync| (sync.send_worklogs)(app)),
            A::Copy(text) => {
                if let Some(clipboard) = &mut self.clipboard {
                    let _ = clipboard.set_text(text);
                }
                self.set_message("Copied the week");
            }
        }
    }
}

impl App {
    /// Home's today: open tasks the way the agenda's Today puts them.
    pub(super) fn agenda_home(&mut self) -> Vec<crate::dashboard::HomeSection> {
        let now = chrono::Local::now();
        // Open tasks the way the agenda's Today puts them: the one on the
        // clock, then in progress, then what's due within a week, then
        // what's ready to start. Waiting and blocked ones stay on the page.
        let live = self.features.agenda().store.active_timer.as_ref().map(|t| (t.task_id, now.signed_duration_since(t.started_at)));
        let week = now.date_naive() + chrono::Duration::days(7);
        let store = &self.features.agenda().store;
        let mut open: Vec<&fenix_agenda::Task> = store
            .tasks
            .iter()
            .filter(|t| !t.archived && t.status != fenix_agenda::Status::Done)
            .filter(|t| t.status == fenix_agenda::Status::InProgress || (t.status == fenix_agenda::Status::Todo && store.is_ready(t.id)) || live.is_some_and(|(id, _)| id == t.id))
            .collect();
        open.sort_by_key(|t| {
            (
                live.is_none_or(|(id, _)| id != t.id),
                t.status != fenix_agenda::Status::InProgress,
                t.due.filter(|d| *d <= week).is_none(),
                t.due,
                std::cmp::Reverse(t.priority),
                t.order,
            )
        });
        let rows = open
            .into_iter()
            .map(|t| crate::dashboard::HomeRow {
                name: match t.jira_key() {
                    Some(key) => format!("{key} {}", t.title),
                    None => t.title.clone(),
                },
                detail: String::new(),
                mark: match live.filter(|(id, _)| *id == t.id) {
                    Some((_, d)) => crate::dashboard::RowMark::Live(super::home::format_elapsed(d)),
                    None => crate::dashboard::RowMark::Task(matches!(t.priority, fenix_agenda::Priority::High | fenix_agenda::Priority::Urgent)),
                },
                entry: crate::dashboard::HomeEntry::Feature { feature: SPEC.id, key: "today".into() },
            })
            .collect();
        vec![crate::dashboard::HomeSection { title: "today", key: "SPC a a", column: crate::dashboard::HomeColumn::Tasks, rank: 10, rows, max: Some(4) }]

    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(dir: &tempfile::TempDir) -> App {
        let mut app = App::with_file(None);
        app.features.agenda_mut().path = dir.path().join("agenda.json");
        app.features.agenda_mut().store = fenix_agenda::AgendaStore::default();
        app.config.set_value("agenda.categories", Some(fenix_config::Value::List(vec!["ops".into()])));
        app
    }

    fn press(app: &mut App, keys: &str) {
        for c in keys.chars() {
            assert!(app.page_key(KeyPress::char(c)), "{c}");
        }
    }

    fn enter(app: &mut App) {
        assert!(app.page_key(KeyPress::named(FenixNamedKey::Enter)));
    }

    fn page_text(app: &mut App) -> String {
        let id = app.focused_buffer_id();
        let pane = app.focused_pane_id();
        app.ensure_page_layout(id, pane, 130);
        app.open().buffer.text()
    }

    #[test]
    fn a_task_added_from_the_page_is_saved_and_shown() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app(&dir);
        app.cmd_agenda_new_task();
        press(&mut app, "Write the report !high #ops due:+2");
        enter(&mut app);
        let task = &app.features.agenda().store.tasks[0];
        assert_eq!(task.title, "Write the report");
        assert_eq!(task.priority, fenix_agenda::Priority::High);
        assert_eq!(task.category.as_deref(), Some("ops"));
        assert_eq!(task.due, Some(chrono::Local::now().date_naive() + chrono::Duration::days(2)));
        assert!(std::fs::read_to_string(dir.path().join("agenda.json")).unwrap().contains("Write the report"));
        assert!(page_text(&mut app).contains("Write the report"));
    }

    #[test]
    fn g_on_the_page_is_a_tab_key_or_else_reaches_the_page() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app(&dir);
        let home = app.ensure_workspace_home();
        let first = app.features.agenda_mut().store.create_task("First".into(), String::new(), fenix_agenda::Priority::Medium, None);
        app.features.agenda_mut().store.create_task("Second".into(), String::new(), fenix_agenda::Priority::Medium, None);
        let page = app.open_agenda_page(Some(Tab::List));
        // `gg` still means the top: `s` then acts on the first task.
        press(&mut app, "jggs2");
        assert_eq!(app.features.agenda().store.task(first).unwrap().status, Status::InProgress);
        press(&mut app, "gh");
        assert_eq!(app.focused_buffer_id(), home);
        app.move_tab(fenix_vim::TabMove::Prev(1));
        assert_eq!(app.focused_buffer_id(), page);
        press(&mut app, "gt");
        assert_eq!(app.focused_buffer_id(), home, "gt from the page wraps to Home");
    }

    #[test]
    fn keys_on_a_row_change_the_task_and_the_page_follows() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app(&dir);
        let id = app.features.agenda_mut().store.create_task("Tidy up".into(), String::new(), fenix_agenda::Priority::Medium, None);
        app.open_agenda_page(Some(Tab::List));
        press(&mut app, "s");
        press(&mut app, "2");
        assert_eq!(app.features.agenda().store.task(id).unwrap().status, Status::InProgress);
        press(&mut app, "t");
        assert!(app.features.agenda().store.active_timer.is_some());
        press(&mut app, "c");
        press(&mut app, "2");
        assert_eq!(app.features.agenda().store.task(id).unwrap().category.as_deref(), Some("ops"));
        assert!(page_text(&mut app).contains("#ops"));
    }

    #[test]
    fn a_task_from_here_keeps_the_line_and_the_code() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("main.rs");
        std::fs::write(&file, "fn main() {\n    todo!()\n}\n").unwrap();
        let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
        app.features.agenda_mut().path = dir.path().join("agenda.json");
        app.features.agenda_mut().store = fenix_agenda::AgendaStore::default();
        app.test_vim_key(KeyPress::char('j'));
        app.cmd_agenda_task_from_here();
        press(&mut app, "Finish main");
        enter(&mut app);
        let task = &app.features.agenda().store.tasks[0];
        assert_eq!(task.code.as_ref().map(|c| c.line), Some(2));
        assert!(task.description.contains("todo!()") && task.description.contains("main.rs:2"), "{}", task.description);
    }

    #[test]
    fn a_clock_left_running_while_away_asks_what_to_keep() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app(&dir);
        let id = app.features.agenda_mut().store.create_task("Long one".into(), String::new(), fenix_agenda::Priority::Medium, None);
        app.features.agenda_mut().store.clock_in(id);
        let now = chrono::Local::now();
        let timer = app.features.agenda_mut().store.active_timer.as_mut().unwrap();
        timer.started_at = now - chrono::Duration::hours(5);
        timer.last_seen = Some(now - chrono::Duration::hours(4));

        assert!(app.agenda_note_activity(), "the key brings up the question");
        assert!(app.features.agenda().gap_asked);
        app.picker_confirm();
        assert!(app.features.agenda().store.active_timer.is_none());
        let entry = app.features.agenda().store.task(id).unwrap().time_entries[0];
        assert_eq!(entry.duration().num_minutes(), 60, "kept until last seen");

        app.cmd_agenda_resume();
        assert_eq!(app.features.agenda().store.active_timer.as_ref().map(|t| t.task_id), Some(id));
        assert!(!app.agenda_note_activity(), "just started: nothing to ask");

        // A clock that never saw you: nothing to go on, so nothing asked.
        app.features.agenda_mut().store.active_timer.as_mut().unwrap().started_at = now - chrono::Duration::hours(5);
        app.features.agenda_mut().store.active_timer.as_mut().unwrap().last_seen = None;
        assert!(!app.agenda_note_activity());
        assert!(app.features.agenda().store.active_timer.as_ref().unwrap().last_seen.is_some(), "and now it knows");
    }

    #[test]
    fn the_clock_picker_offers_stop_then_recent_tasks() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app(&dir);
        let a = app.features.agenda_mut().store.create_task("Older".into(), String::new(), fenix_agenda::Priority::Medium, None);
        let b = app.features.agenda_mut().store.create_task("Recent".into(), String::new(), fenix_agenda::Priority::Medium, None);
        let now = chrono::Local::now();
        app.features.agenda_mut().store.log_span(a, now - chrono::Duration::hours(5), now - chrono::Duration::hours(4));
        app.features.agenda_mut().store.log_span(b, now - chrono::Duration::hours(2), now - chrono::Duration::hours(1));
        app.cmd_agenda_toggle_clock();
        let Some(ActivePicker::Feature { badge: "CLOCK", state, .. }) = &app.active_picker else { panic!() };
        let labels: Vec<String> = state.visible_rows(0, 5).map(|(_, c)| c.label.clone()).collect();
        assert_eq!(labels[0], "Resume  ·  Recent");
        assert_eq!(labels[1], "Older");
    }

    #[test]
    fn deleting_the_open_task_goes_back_to_the_tab() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app(&dir);
        app.features.agenda_mut().store.create_task("Doomed".into(), String::new(), fenix_agenda::Priority::Medium, None);
        app.open_agenda_page(Some(Tab::List));
        enter(&mut app);
        press(&mut app, "DD");
        assert!(app.features.agenda().store.tasks.is_empty());
        assert!(page_text(&mut app).contains("No tasks yet"));
    }

    // Through its spec, the way `App` reaches the agenda now.

    #[test]
    fn the_agenda_contributes_its_commands_and_keys_from_its_spec() {
        let registry = CommandRegistry::with_builtins();
        let agenda: Vec<(&str, bool)> = registry.iter().filter(|c| c.id.starts_with("agenda.")).map(|c| (c.id.as_ref(), c.external())).collect();
        assert_eq!(agenda.len(), 12, "{agenda:?}");
        assert!(agenda.contains(&("agenda.toggle_clock", true)) && agenda.contains(&("agenda.from_here", false)));
        let keys = keymap::leader_keys();
        assert_eq!(keys.get("agenda.open").map(String::as_str), Some("SPC a a"));
        assert_eq!(keys.get("agenda.worklogs").map(String::as_str), Some("SPC a w"));
    }

    /// The agenda's sync is whatever plugs into `agenda_sync`: Jira's,
    /// and only Jira's.
    #[test]
    fn jira_plugs_its_sync_into_the_agendas_contribution_point() {
        let dir = tempfile::tempdir().unwrap();
        let app = app(&dir);
        assert_eq!(app.contributions(AGENDA_SYNC).count(), 1);
        let sync = app.agenda_sync().expect("Jira's sync");
        assert!(std::ptr::eq(sync, &super::super::agenda_sync::SYNC));
        assert_eq!(SPEC.contributes.len(), 0, "the agenda defines the point; it doesn't plug into it");
        assert_eq!((sync.busy)(&app), None, "nothing running yet");
    }

    #[test]
    fn the_clock_picker_is_the_agendas_and_a_pick_reaches_it_through_the_spec() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app(&dir);
        let a = app.features.agenda_mut().store.create_task("Write".into(), String::new(), fenix_agenda::Priority::Medium, None);
        app.cmd_agenda_toggle_clock();
        let Some(ActivePicker::Feature { feature, badge, .. }) = &app.active_picker else { panic!("a feature picker") };
        assert_eq!((*feature, *badge), ("fenix.agenda", "CLOCK"));
        app.picker_confirm();
        assert_eq!(app.features.agenda().store.active_timer.as_ref().map(|t| t.task_id), Some(a));
        assert!(SPEC.picker_confirm.is_some());
        // The sync's picker (and its cancel, which puts a board move back)
        // is the Jira feature's, which plugs the sync in.
        assert!(super::super::jira_host::SPEC.picker_confirm.is_some() && super::super::jira_host::SPEC.picker_cancel.is_some());
    }

    #[test]
    fn a_running_clock_lays_the_agenda_page_out_again_each_minute_through_its_context() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app(&dir);
        let a = app.features.agenda_mut().store.create_task("Write".into(), String::new(), fenix_agenda::Priority::Medium, None);
        let id = app.open_agenda_page(None);
        let need = |due| super::super::feature::PageNeed::Layout { cols: 100, due };
        let changed = |app: &mut App, due| {
            let ctx = app.feature_page_context(id, need(due));
            let Some(PageModel::Feature(p)) = app.pages.get(&id).map(|s| &s.model) else { panic!("a feature page") };
            p.changed(&*ctx)
        };
        assert!(!changed(&mut app, false), "no clock, nothing moves");
        app.features.agenda_mut().store.clock_in(a);
        assert!(changed(&mut app, false), "a running clock's minute: laid out again");
        assert!(!changed(&mut app, false), "once a minute");
        app.ensure_page_layout(id, app.focused_pane_id(), 100);
        assert!(app.open().buffer.text().contains("Write"), "laid out by the agenda's context");
    }
}
