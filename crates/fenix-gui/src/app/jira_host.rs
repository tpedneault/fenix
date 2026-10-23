//! The host half of the Jira page (`jira_page`): opening it on the right
//! searches, running them and fetching issues off the UI thread, and
//! carrying out what its keys ask. Anything an issue in the agenda needs
//! afterwards (a pull, so its task follows) goes through `agenda_sync`.

use super::pages::{PageEvent, PageModel};
use super::agenda_sync::{AgendaSyncState, SyncPick};
use super::*;
use crate::jira_page::{self, Action as JiraAction, Choice, JiraPage, Loaded, QueryKind};

/// What the Jira page's background jobs send back (as the Jira feature's
/// page events).
#[derive(Debug)]
pub(crate) enum JiraJob {
    /// A search on the Jira page came back.
    Issues { buffer: BufferId, jql: String, result: Result<Vec<fenix_jira::IssueSummary>, String> },
    Detail { buffer: BufferId, key: String, result: Result<fenix_jira::IssueDetail, String> },
    /// Choices for an issue, fetched: its transitions, or priorities.
    Offer { buffer: BufferId, title: String, key: String, result: Result<Vec<(String, jira_page::Choice)>, String> },
    Types { buffer: BufferId, project: String, types: Result<Vec<fenix_jira::IssueType>, String>, priorities: Vec<String> },
    Fields { buffer: BufferId, type_id: String, result: Result<Vec<fenix_jira::CreateField>, String> },
    /// A change to an issue went through, or didn't.
    Done { buffer: BufferId, key: String, result: Result<String, String> },
}

/// How many issues a search lists.
const SEARCH_LIMIT: u32 = 100;

/// The Jira page's commands and the `SPC j` group. The agenda's Jira
/// side (linking, sync, worklogs) is the agenda's.
/// Jira's settings: `[jira]` in `settings.toml`. Its server and token
/// stay core's for now, beside the other tokens (Phase 0 item 6).
static SETTINGS: [fenix_config::Setting; 6] = {
    use fenix_config::{Category::Jira, Kind, Setting};
    [
        Setting::stored("jira.sync_minutes", Jira, "Sync every", Kind::Minutes, "How often linked agenda tasks are brought up to date, in minutes.").with_default("10"),
        Setting::stored("jira.projects", Jira, "Projects", Kind::Map { key: "Key", value: "Name", paths: false }, "Jira projects the Jira page lists open issues and the current sprint for."),
        Setting::stored("jira.users", Jira, "People", Kind::Map { key: "Username", value: "Name", paths: false }, "People the Jira page lists issues assigned to."),
        Setting::stored("jira.queries", Jira, "Saved searches", Kind::Map { key: "Name", value: "JQL", paths: false }, "Searches saved on the Jira page, by name."),
        Setting::stored("jira.blocked", Jira, "What Blocked means", Kind::Map { key: "Project", value: "Meaning", paths: false }, "Per project: flag, local, or the status to move to, as ID: Name. Learned the first time you block a task.").checked(|v| match v {
            fenix_config::Value::Map(entries) => entries.iter().try_for_each(|(_, meaning)| JiraBlocked::parse(meaning).map(|_| ())),
            _ => Ok(()),
        }),
        Setting::stored("jira.priorities", Jira, "Priority names", Kind::Map { key: "Jira priority", value: "Agenda priority", paths: false }, "How a Jira priority maps onto the agenda's."),
    ]
};

/// What "Blocked" means for one Jira project (`jira.blocked`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum JiraBlocked {
    /// Transition to this status (matched by id; `name` is for display).
    Status { id: String, name: String },
    /// Set the issue's Flagged (impediment) field; leave the status alone.
    Flag,
    /// Don't touch Jira at all.
    Local,
}

impl JiraBlocked {
    /// `flag`, `local`, or a status as `ID: Name`.
    pub(crate) fn parse(text: &str) -> Result<JiraBlocked, String> {
        match text.trim() {
            "flag" => Ok(JiraBlocked::Flag),
            "local" => Ok(JiraBlocked::Local),
            other => match other.split_once(':') {
                Some((id, name)) if !id.trim().is_empty() => Ok(JiraBlocked::Status { id: id.trim().into(), name: name.trim().into() }),
                _ => Err(format!("\"{other}\": expected flag, local, or a status as ID: Name")),
            },
        }
    }

    /// As `jira.blocked` holds it.
    pub(crate) fn text(&self) -> String {
        match self {
            JiraBlocked::Flag => "flag".into(),
            JiraBlocked::Local => "local".into(),
            JiraBlocked::Status { id, name } => format!("{id}: {name}"),
        }
    }
}

impl App {
    /// What Blocked means in `project`, if it's been chosen.
    pub(super) fn jira_blocked_for(&self, project: &str) -> Option<JiraBlocked> {
        self.config.map_value("jira.blocked").iter().find(|(p, _)| p == project).and_then(|(_, meaning)| JiraBlocked::parse(meaning).ok())
    }

    /// Whether Blocked means the Flagged field in any project.
    pub(super) fn jira_blocked_flags_anywhere(&self) -> bool {
        self.config.map_value("jira.blocked").iter().any(|(_, meaning)| JiraBlocked::parse(meaning) == Ok(JiraBlocked::Flag))
    }

    /// Records (or replaces) `project`'s Blocked meaning.
    pub(super) fn set_jira_blocked(&mut self, project: &str, blocked: JiraBlocked) {
        self.jira_edit_map("jira.blocked", |entries| match entries.iter_mut().find(|(p, _)| p == project) {
            Some(entry) => entry.1 = blocked.text(),
            None => entries.push((project.to_string(), blocked.text())),
        });
    }

    /// Edits one of Jira's lists (`jira.projects`, `.users`, `.queries`,
    /// `.blocked`) in place, by key.
    pub(super) fn jira_edit_map(&mut self, key: &str, edit: impl FnOnce(&mut Vec<(String, String)>)) {
        let mut entries = self.config.map_value(key).to_vec();
        edit(&mut entries);
        self.config.set_value(key, (!entries.is_empty()).then_some(fenix_config::Value::Map(entries)));
    }
}

pub(crate) static SPEC: super::feature::FeatureSpec = {
    use super::feature::FeatureCommand as C;
    use super::feature::LeaderEntry::{Bind, Group};
    super::feature::FeatureSpec {
        id: "fenix.jira",
        name: "Jira",
        about: "Your issues, and the agenda kept in step with them",
        commands: &[
            C {
                id: "jira.open",
                description: "Open the Jira page: your searches, their issues, a preview",
                external: true,
                run: |app| {
                    app.open_jira_page();
                },
            },
            C { id: "jira.refresh", description: "Run the Jira page's search again", external: false, run: |app| app.jira_refresh() },
            C { id: "jira.goto_issue", description: "Open any issue by key", external: true, run: |app| app.cmd_jira_goto() },
            C { id: "jira.search", description: "Search Jira: words, or JQL after :", external: true, run: |app| app.cmd_jira_search() },
            C { id: "jira.create_issue", description: "Create an issue, with the project's real types and fields", external: true, run: |app| app.cmd_jira_new_issue() },
        ],
        leader: &[
            Group(&['j'], "jira"),
            Bind(&['j', 'j'], "open jira", "jira.open"),
            Bind(&['j', 'r'], "refresh jira", "jira.refresh"),
            Bind(&['j', 'g'], "go to issue", "jira.goto_issue"),
            Bind(&['j', '/'], "search jira", "jira.search"),
            Bind(&['j', 'n'], "new issue", "jira.create_issue"),
        ],
        local_menu: &[],
        local_context: None,
        language_server: None,
        tasks: None,
        hover_fallback: None,
        completion_at_cursor: None,
        completion_pool_at: None,
        hover_first: None,
        // The agenda sync's picker (link, import, status, priority,
        // assignee, what Blocked means).
        picker_confirm: Some(|app, pick, marked, _query| {
            if let Some(pick) = pick.downcast_ref::<SyncPick>() {
                let marked = marked.iter().filter_map(|m| m.0.downcast_ref::<SyncPick>().cloned()).collect();
                app.agenda_sync_picker_confirm(pick.clone(), marked);
            }
        }),
        picker_cancel: Some(|app| app.agenda_sync_picker_cancel()),
        modeline_clock: None,
        page_action: Some(|app, id, action| {
            if let Ok(action) = action.downcast::<JiraAction>() {
                app.jira_page_action(id, *action);
            }
        }),
        page_context: Some(|app, _id, need| match need {
            super::feature::PageNeed::Layout { due: false, .. } => Box::new(()),
            _ => Box::new(JiraCtx::of(app)),
        }),
        page_title: None,
        page_after_layout: None,
        on_event: Some(|app, event| {
            let event = match event.downcast::<JiraDone>() {
                Ok(done) => return app.apply_jira_action_done(done.0),
                Err(event) => event,
            };
            let event = match event.downcast::<JiraJob>() {
                Ok(job) => return app.apply_jira_event(*job),
                Err(event) => event,
            };
            if let Ok(event) = event.downcast::<AgendaSyncEvent>() {
                app.apply_agenda_sync(*event);
            }
        }),
        settings: &SETTINGS,
        services: &SERVICES,
        // Its sync for the agenda.
        contributes: &[
            super::feature::Contribution { point: super::agenda_host::AGENDA_SYNC, value: &super::agenda_sync::SYNC },
            super::feature::Contribution { point: super::settings_host::SECRET_CHECKS, value: &SECRET_CHECK },
        ],
        session_documents: None,
        session_reopen: None,
        home: None,
        home_activate: None,
        doctor: None,
        opener: None,
        lsp_first: None,
        prompt_key: None,
        wake: None,
        links: None,
        on_start: None,
        before_key: None,
    }
};

/// A finished write (create, comment, title, description, time,
/// transition) from `spawn_jira_action`'s thread. Never dropped for being
/// late, unlike a read: `Ok` is a confirmation for the modeline, `Err`
/// the reason it failed.
pub(crate) struct JiraDone(pub(crate) Result<String, String>);

/// What the Jira page reads besides itself: which issues are in the
/// agenda, the people to pick from, the server, today.
struct JiraCtx {
    in_agenda: HashSet<String>,
    people: Vec<(String, String)>,
    server: String,
    today: chrono::NaiveDate,
}

impl JiraCtx {
    fn of(app: &App) -> Self {
        JiraCtx {
            in_agenda: app.features.agenda().store.tasks.iter().filter_map(|t| t.jira_key().map(str::to_string)).collect(),
            people: app.config.map_value("jira.users").to_vec(),
            server: app.config.jira_base_url.clone().unwrap_or_default(),
            today: chrono::Local::now().date_naive(),
        }
    }

    fn page(&self) -> jira_page::Ctx<'_> {
        jira_page::Ctx { in_agenda: &self.in_agenda, people: &self.people, server: self.server.clone(), today: self.today }
    }
}

/// The Jira page (`SPC j j`).
impl super::feature::FeaturePage for JiraPage {
    fn feature(&self) -> &'static str {
        SPEC.id
    }

    fn title(&self) -> String {
        "*jira*".to_string()
    }

    fn typing(&self) -> bool {
        JiraPage::typing(self)
    }

    fn claims_space(&self) -> bool {
        JiraPage::claims_space(self)
    }

    fn paste(&mut self, text: &str) {
        JiraPage::paste(self, text);
    }

    fn layout(&mut self, ctx: Box<dyn std::any::Any>, cols: usize) -> crate::page::Page {
        match ctx.downcast::<JiraCtx>() {
            Ok(ctx) => jira_page::layout(self, &ctx.page(), cols),
            Err(_) => crate::page::Page::default(),
        }
    }

    fn key(&mut self, key: crate::page::Key, ctx: Box<dyn std::any::Any>) -> Box<dyn std::any::Any> {
        Box::new(match ctx.downcast::<JiraCtx>() {
            Ok(ctx) => JiraPage::key(self, key, &ctx.page()),
            Err(_) => JiraAction::None,
        })
    }
}

/// The Jira page keeps its state on the page, so its feature holds none.
#[derive(Default)]
pub(crate) struct JiraHost {
    /// Its agenda sync's bookkeeping (`agenda_sync`).
    pub(super) sync: AgendaSyncState,
}

impl super::feature::Feature for JiraHost {
    fn spec(&self) -> &'static super::feature::FeatureSpec {
        &SPEC
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }

    /// What the open agenda sync pick is for ("WHAT DOES BLOCKED MEAN IN
    /// PROJ?", "PROJ-12 STATUS") -- the badge alone can't say.
    fn picker_label(&self) -> Option<&str> {
        self.sync.picker.as_ref().map(|ctx| ctx.label.as_str())
    }
}

impl super::feature::Features {
    /// Jira's agenda sync bookkeeping. Jira has no instance hooks that run
    /// code, so it's always in place.
    pub(super) fn jira_sync(&self) -> &AgendaSyncState {
        &self.get::<JiraHost>().expect("Jira is always in place").sync
    }

    pub(super) fn jira_sync_mut(&mut self) -> &mut AgendaSyncState {
        &mut self.get_mut::<JiraHost>().expect("Jira is always in place").sync
    }
}

/// One of the Jira feature's pickers (its agenda sync's).
pub(super) fn jira_picker<T: std::any::Any>(badge: &'static str, candidates: Vec<fenix_picker::Candidate<T>>) -> ActivePicker {
    let candidates = candidates.into_iter().map(|c| super::feature::FeaturePick::candidate(c.label, c.payload)).collect();
    ActivePicker::Feature { feature: SPEC.id, badge, state: fenix_picker::PickerState::new(candidates) }
}

impl App {
    fn jira_page_mut(&mut self, id: BufferId) -> Option<&mut JiraPage> {
        self.feature_page_mut::<JiraPage>(id)
    }

    pub(super) fn jira_page_id(&self) -> Option<BufferId> {
        self.find_page(|m| super::feature::page_as::<JiraPage>(m).is_some())
    }

    /// The open project's Jira key.
    fn jira_here(&self) -> Option<String> {
        self.project_root.as_deref().and_then(fenix_project::meta::jira_key)
    }

    fn jira_queries(&self, here: Option<&str>) -> Vec<jira_page::Query> {
        jira_page::queries(self.config.map_value("jira.projects"), self.config.map_value("jira.users"), self.config.map_value("jira.queries"), here)
    }

    /// `SPC j j`: the Jira page, on the searches for the open project.
    pub(crate) fn open_jira_page(&mut self) -> BufferId {
        let here = self.jira_here();
        let id = match self.jira_page_id() {
            Some(id) => {
                self.show_page(id);
                let known = self.jira_page_mut(id).and_then(|p| p.here.clone());
                let queries = self.jira_queries(here.as_deref().or(known.as_deref()));
                if let Some(page) = self.jira_page_mut(id) {
                    if here.is_some() {
                        page.here = here;
                    }
                    page.set_queries(queries);
                    page.note = None;
                }
                id
            }
            None => {
                let queries = self.jira_queries(here.as_deref());
                self.open_page(PageModel::Feature(Box::new(JiraPage::new(queries, here))))
            }
        };
        let want = self.jira_page_mut(id).and_then(|p| p.wants_fetch());
        if let Some(jql) = want {
            self.jira_fetch(id, jql);
        }
        id
    }

    /// `SPC j g`: an issue by key.
    pub(crate) fn cmd_jira_goto(&mut self) {
        let id = self.open_jira_page();
        if let Some(page) = self.jira_page_mut(id) {
            page.start_goto();
        }
    }

    /// `SPC j /`: search Jira -- words, or JQL after `:`.
    pub(crate) fn cmd_jira_search(&mut self) {
        let id = self.open_jira_page();
        if let Some(page) = self.jira_page_mut(id) {
            page.start_search();
        }
    }

    /// `SPC j n`: the new-issue form.
    pub(crate) fn cmd_jira_new_issue(&mut self) {
        let id = self.open_jira_page();
        let projects = self.jira_form_projects();
        let Some(action) = self.jira_page_mut(id).map(|p| p.start_issue(projects, None)) else { return };
        self.jira_page_action(id, action);
    }

    /// `I` on a task -> "+ Create a new Jira issue from this task": the
    /// form, filled from the task, linking the two when it's created.
    pub(super) fn jira_new_issue_from_task(&mut self, task: TaskId) {
        let Some(t) = self.features.agenda().store.task(task) else { return };
        let from = (task, t.title.clone(), t.description.clone());
        let id = self.open_jira_page();
        let projects = self.jira_form_projects();
        let Some(action) = self.jira_page_mut(id).map(|p| p.start_issue(projects, Some(from))) else { return };
        self.jira_page_action(id, action);
    }

    /// The projects an issue can be created in, the open one first.
    fn jira_form_projects(&self) -> Vec<String> {
        let mut projects: Vec<String> = self.jira_here().into_iter().collect();
        for (key, _) in self.config.map_value("jira.projects") {
            if !projects.iter().any(|p| p.eq_ignore_ascii_case(key)) {
                projects.push(key.clone());
            }
        }
        projects
    }

    /// The searches again (the settings changed), and the showing one.
    pub(super) fn refresh_jira_page(&mut self) {
        let Some(id) = self.jira_page_id() else { return };
        let here = self.jira_page_mut(id).and_then(|p| p.here.clone());
        let queries = self.jira_queries(here.as_deref());
        if let Some(page) = self.jira_page_mut(id) {
            page.set_queries(queries);
        }
        let want = self.jira_page_mut(id).and_then(|p| p.wants_fetch());
        if let Some(jql) = want {
            self.jira_fetch(id, jql);
        }
    }

    /// Runs the showing search again, and the showing issue's detail.
    pub(crate) fn jira_refresh(&mut self) {
        let Some(id) = self.jira_page_id() else { return };
        let Some(page) = self.jira_page_mut(id) else { return };
        let jql = page.query().map(|q| q.jql.clone());
        let keys: Vec<String> = page.details.keys().cloned().collect();
        page.details.clear();
        if let Some(jql) = jql {
            page.results.insert(jql.clone(), Loaded::Loading);
            self.jira_fetch(id, jql);
        }
        // Just the ones in view come back; the rest are fetched on demand.
        if let Some(key) = keys.into_iter().next() {
            self.jira_fetch_detail(id, key);
        }
    }

    fn jira_fetch(&mut self, buffer: BufferId, jql: String) {
        let Some(client) = self.jira_client() else {
            if let Some(page) = self.jira_page_mut(buffer) {
                page.results.insert(jql, Loaded::Failed("Jira isn't set up -- its server and token go on the settings page (SPC ,)".into()));
            }
            return;
        };
        if let Some(page) = self.jira_page_mut(buffer) {
            page.results.insert(jql.clone(), Loaded::Loading);
        }
        self.page_spawn(move |send| {
            let result = client.search_issues(&jql, SEARCH_LIMIT);
            send(PageEvent::feature(SPEC.id, "JiraIssues", JiraJob::Issues { buffer, jql, result }));
        });
    }

    fn jira_fetch_detail(&mut self, buffer: BufferId, key: String) {
        let Some(client) = self.jira_client() else { return };
        self.page_spawn(move |send| {
            let result = client.get_issue(&key);
            send(PageEvent::feature(SPEC.id, "JiraDetail", JiraJob::Detail { buffer, key, result }));
        });
    }

    /// A write to issue `key`: shown when done, and the page (and the
    /// agenda, when the issue's in it) brought up to date.
    fn jira_write(&mut self, buffer: BufferId, key: String, job: impl FnOnce(&fenix_jira::JiraClient) -> Result<String, String> + Send + 'static) {
        let Some(client) = self.jira_client() else { return };
        self.page_spawn(move |send| {
            let result = job(&client);
            send(PageEvent::feature(SPEC.id, "JiraDone", JiraJob::Done { buffer, key, result }));
        });
    }

    pub(super) fn apply_jira_event(&mut self, event: JiraJob) {
        match event {
            JiraJob::Issues { buffer, jql, result } => {
                if let Some(page) = self.jira_page_mut(buffer) {
                    page.results.insert(jql, result.map(Loaded::Issues).unwrap_or_else(Loaded::Failed));
                }
                // The first issue's preview, once there are issues.
                let want = self.jira_page_mut(buffer).and_then(|p| p.query().map(|q| q.jql.clone()).and_then(|jql| match p.results.get(&jql) {
                    Some(Loaded::Issues(i)) => i.first().map(|i| i.key.clone()),
                    _ => None,
                }));
                if let Some(key) = want.filter(|k| self.jira_page_mut(buffer).is_some_and(|p| !p.details.contains_key(k))) {
                    self.jira_fetch_detail(buffer, key);
                }
            }
            JiraJob::Detail { buffer, key, result } => {
                if let Some(page) = self.jira_page_mut(buffer) {
                    page.details.insert(key, result);
                }
            }
            JiraJob::Offer { buffer, title, key, result } => match result {
                Ok(items) => {
                    if let Some(page) = self.jira_page_mut(buffer) {
                        page.offer(title, key, items);
                    }
                }
                Err(err) => self.set_error(format!("{key}: {err}")),
            },
            JiraJob::Types { buffer, project, types, priorities } => {
                let Some(action) = self.jira_page_mut(buffer).map(|p| p.types_loaded(&project, types, priorities)) else { return };
                self.jira_page_action(buffer, action);
            }
            JiraJob::Fields { buffer, type_id, result } => {
                if let Some(page) = self.jira_page_mut(buffer) {
                    page.fields_loaded(&type_id, result);
                }
            }
            JiraJob::Done { buffer, key, result } => {
                match result {
                    Ok(message) => self.set_message(message),
                    Err(err) => self.set_error(format!("{key}: {err}")),
                }
                if let Some(page) = self.jira_page_mut(buffer) {
                    page.details.remove(&key);
                    if let Some(jql) = page.query().map(|q| q.jql.clone()) {
                        page.results.remove(&jql);
                    }
                }
                let want = self.jira_page_mut(buffer).and_then(|p| p.wants_fetch());
                if let Some(jql) = want {
                    self.jira_fetch(buffer, jql);
                }
                self.jira_fetch_detail(buffer, key.clone());
                if self.features.agenda().store.find_by_key(&key).is_some() {
                    self.agenda_sync_now(true);
                }
            }
        }
    }

    fn jira_issue_url(&mut self, key: &str) -> Option<String> {
        match self.config.jira_base_url.clone() {
            Some(base) => Some(format!("{}/browse/{key}", base.trim_end_matches('/'))),
            None => {
                self.set_error("Jira isn't set up -- its server goes on the settings page (SPC ,)");
                None
            }
        }
    }

    fn jira_save_config(&mut self) {
        if let Err(err) = self.config.save() {
            self.set_error(format!("couldn't save settings.toml: {err}"));
        }
        self.refresh_jira_page();
        self.refresh_settings_pages();
    }

    pub(super) fn jira_page_action(&mut self, id: BufferId, action: JiraAction) {
        use JiraAction as A;
        match action {
            A::None => {}
            A::Close => self.close_page(id),
            A::Fetch(jql) => self.jira_fetch(id, jql),
            A::Detail(key) => self.jira_fetch_detail(id, key),
            A::Refresh => self.jira_refresh(),
            A::OpenTask(key) => {
                let Some(task) = self.features.agenda().store.find_by_key(&key) else { return };
                let page = self.open_agenda_page(None);
                if let Some(p) = self.agenda_page_mut(page) {
                    p.open_task(task);
                }
            }
            A::Transitions(key) => {
                let Some(client) = self.jira_client() else { return };
                self.page_spawn(move |send| {
                    let result = client.list_transitions(&key).map(|ts| {
                        ts.into_iter().map(|t| (format!("{}  →  {}", t.name, if t.to_name.is_empty() { t.name.clone() } else { t.to_name.clone() }), Choice::Transition { id: t.id, name: t.name })).collect()
                    });
                    send(PageEvent::feature(SPEC.id, "JiraOffer", JiraJob::Offer { buffer: id, title: format!("Move {key}"), key, result }));
                });
            }
            A::Priorities(key) => {
                let Some(client) = self.jira_client() else { return };
                self.page_spawn(move |send| {
                    let result = client.list_priorities().map(|ps| ps.into_iter().map(|p| (p.name.clone(), Choice::Priority(p.name))).collect());
                    send(PageEvent::feature(SPEC.id, "JiraOffer", JiraJob::Offer { buffer: id, title: format!("{key} priority"), key, result }));
                });
            }
            A::Transition { key, id: transition, name } => {
                let k = key.clone();
                self.jira_write(id, key, move |c| c.apply_transition(&k, &transition).map(|_| format!("{k}: {name}")));
            }
            A::SetPriority { key, name } => {
                let k = key.clone();
                self.jira_write(id, key, move |c| c.update_priority(&k, &name).map(|_| format!("{k} is {name} now")));
            }
            A::Assign { key, id: user, name } => {
                let k = key.clone();
                let me = self.features.jira_sync().me.clone();
                self.jira_write(id, key, move |c| {
                    let user = if user.is_empty() { me.map(Ok).unwrap_or_else(|| c.myself())? } else { user };
                    c.update_assignee(&k, &user).map(|_| format!("{k} assigned to {}", if name.is_empty() { user.as_str() } else { name.as_str() }))
                });
            }
            A::SetDue { key, due } => {
                let k = key.clone();
                let due = due.map(|d| d.format("%Y-%m-%d").to_string());
                self.jira_write(id, key, move |c| {
                    c.update_due(&k, due.as_deref()).map(|_| match &due {
                        Some(d) => format!("{k} due {d}"),
                        None => format!("{k} has no due date now"),
                    })
                });
            }
            A::SetTitle { key, title } => {
                let k = key.clone();
                self.jira_write(id, key, move |c| c.update_summary(&k, &title).map(|_| format!("{k} renamed")));
            }
            A::LogTime { key, time } => {
                let k = key.clone();
                self.jira_write(id, key, move |c| c.add_worklog(&k, &time).map(|_| format!("Logged {time} on {k}")));
            }
            A::EditDescription(key) => {
                let seed = self.jira_page_mut(id).and_then(|p| p.details.get(&key).and_then(|d| d.as_ref().ok()).and_then(|d| d.description.clone())).unwrap_or_default();
                self.open_compose_seeded(issue_compose("description", key), &seed);
            }
            A::Comment(key) => self.open_compose_seeded(issue_compose("comment", key), ""),
            A::AddToAgenda { key, clock } => self.jira_add_to_agenda(key, clock),
            A::CopyLink(key) => {
                let Some(url) = self.jira_issue_url(&key) else { return };
                if let Some(clipboard) = &mut self.clipboard {
                    let _ = clipboard.set_text(url.clone());
                }
                self.set_message(format!("Copied {url}"));
            }
            A::Browser(key) => {
                if let Some(url) = self.jira_issue_url(&key) {
                    self.open_url(&url);
                }
            }
            A::AddProject { key, name } => {
                self.jira_edit_map("jira.projects", |projects| {
                    if !projects.iter().any(|(k, _)| k.eq_ignore_ascii_case(&key)) {
                        projects.push((key.clone(), name));
                    }
                });
                self.set_message(format!("{key}'s open issues and sprint are on the left now"));
                self.jira_save_config();
            }
            A::AddPerson { id: user, name } => {
                self.jira_edit_map("jira.users", |users| {
                    if !users.iter().any(|(u, _)| *u == user) {
                        users.push((user, name));
                    }
                });
                self.jira_save_config();
            }
            A::SaveQuery { name, jql } => {
                self.jira_edit_map("jira.queries", |queries| match queries.iter_mut().find(|(n, _)| *n == name) {
                    Some(q) => q.1 = jql,
                    None => queries.push((name.clone(), jql)),
                });
                self.set_message(format!("Saved \"{name}\""));
                self.jira_save_config();
            }
            A::RemoveQuery(q) => {
                match &q.kind {
                    QueryKind::Project(key) => self.jira_edit_map("jira.projects", |projects| projects.retain(|(k, _)| k != key)),
                    QueryKind::Person(user) => self.jira_edit_map("jira.users", |users| users.retain(|(u, _)| u != user)),
                    QueryKind::Saved => self.jira_edit_map("jira.queries", |queries| queries.retain(|(n, _)| *n != q.name)),
                    QueryKind::Mine | QueryKind::Search => return,
                }
                self.jira_save_config();
            }
            A::Blocked(project) => self.jira_start_blocked_setup(project),
            A::IssueTypes(project) => {
                let Some(client) = self.jira_client() else { return };
                self.page_spawn(move |send| {
                    let types = client.issue_types(&project);
                    let priorities = client.list_priorities().map(|ps| ps.into_iter().map(|p| p.name).collect()).unwrap_or_default();
                    send(PageEvent::feature(SPEC.id, "JiraTypes", JiraJob::Types { buffer: id, project, types, priorities }));
                });
            }
            A::CreateFields { project, type_id } => {
                let Some(client) = self.jira_client() else { return };
                self.page_spawn(move |send| {
                    let result = client.create_fields(&project, &type_id);
                    send(PageEvent::feature(SPEC.id, "JiraFields", JiraJob::Fields { buffer: id, type_id, result }));
                });
            }
            A::Create(create) => self.jira_create(id, *create),
        }
    }

    fn jira_create(&mut self, buffer: BufferId, create: jira_page::Create) {
        let jira_page::Create { issue, agenda, clock, task } = create;
        self.set_message(format!("Creating a {} issue...", issue.project));
        if !agenda {
            let Some(client) = self.jira_client() else { return };
            self.page_spawn(move |send| {
                let result = client.create(&issue);
                let key = result.as_ref().cloned().unwrap_or_default();
                send(PageEvent::feature(SPEC.id, "JiraDone", JiraJob::Done { buffer, key, result: result.map(|k| format!("Created {k}")) }));
            });
            return;
        }
        // Into the agenda: the same fetch that links an issue by key.
        let flag = self.agenda_flag_field_wanted_now();
        let purpose = match task {
            Some(task) => agenda_sync::IssueFetch::Created(task),
            None => agenda_sync::IssueFetch::AddFromJira { clock },
        };
        let clock_after = task.filter(|_| clock);
        self.agenda_spawn(move |client| {
            let result = client.create(&issue).and_then(|key| client.get_issue_with(&key, flag.as_deref()).map(|d| vec![d]));
            AgendaSyncEvent::Issues { purpose, result }
        });
        if let Some(task) = clock_after {
            self.features.agenda_mut().store.clock_in(task);
            self.agenda_save_and_refresh();
        }
        let _ = buffer;
    }
}

impl App {
    /// A ready-to-use `fenix_jira::JiraClient` built from `Config`'s
    /// `[jira]` settings, or `None` (with a surfaced error) when either
    /// `base_url` or `token` isn't configured yet.
    pub(super) fn jira_client(&mut self) -> Option<fenix_jira::JiraClient> {
        let base_url = self.config.jira_base_url.clone();
        let token = self.config.token(fenix_config::Secret::Jira);
        match (base_url, token) {
            (Some(base_url), Some(token)) => Some(fenix_jira::JiraClient::new(base_url, token)),
            _ => {
                self.set_error("Jira isn't set up yet -- set its server and token in SPC , (Jira & agenda)");
                None
            }
        }
    }

    /// The shared shape every new Jira write action (create, comment,
    /// title, description, log time, transition) needs: build a
    /// `JiraClient` from `Config` (via `jira_client`, which already
    /// surfaces its own error and returns `None` when unconfigured -- so
    /// this is a safe no-op-without-credentials early return, same
    /// posture `jira_sync_issues` already has), run `action` on a
    /// background thread (or synchronously when `event_proxy` is `None`,
    /// for tests -- mirrors `jira_sync_issues`'s own `Some`/`None`
    /// branching exactly), and report the result via `FenixUserEvent::
    /// JiraActionDone`/`apply_jira_action_done`.
    pub(super) fn spawn_jira_action<F>(&mut self, action: F)
    where
        F: FnOnce(&fenix_jira::JiraClient) -> Result<String, String> + Send + 'static,
    {
        let Some(client) = self.jira_client() else { return };
        match self.event_proxy.clone() {
            Some(proxy) => {
                std::thread::spawn(move || {
                    let result = action(&client);
                    let _ = proxy.send_event(FenixUserEvent::feature(jira_host::SPEC.id, "JiraActionDone", jira_host::JiraDone(result)));
                });
            }
            None => {
                let result = action(&client);
                self.apply_jira_action_done(result);
            }
        }
    }

    /// `FenixUserEvent::JiraActionDone` handling: on success, shows the
    /// action's own human-readable confirmation and refreshes the panel
    /// (`jira_refresh` -- picks up whatever changed, e.g. a new comment
    /// or an updated status); on failure, surfaces the error. No
    /// staleness guard, unlike `apply_jira_issues`/`apply_jira_detail` --
    /// a write action should never be silently discarded just because
    /// the user navigated elsewhere while it was in flight.
    pub(super) fn apply_jira_action_done(&mut self, result: Result<String, String>) {
        match result {
            Ok(message) => {
                self.set_message(message);
                self.jira_refresh();
            }
            Err(err) => self.set_error(err),
        }
    }
}

/// An issue's description or a new comment on it, written in the compose
/// pane and handed back to `jira.compose`.
pub(super) fn issue_compose(kind: &'static str, key: String) -> ComposePurpose {
    let (what, verb, allow_empty) = match kind {
        "description" => (format!("{key} description"), "save", true),
        _ => (format!("Comment on {key}"), "post", false),
    };
    ComposePurpose::Feature { service: "jira.compose", kind, arg: key, what, verb, allow_empty }
}

/// The Jira token's check on the settings page: who it signs in as.
static SECRET_CHECK: super::settings_host::SecretCheck = super::settings_host::SecretCheck {
    secret: fenix_config::Secret::Jira,
    check: |server, token| fenix_jira::JiraClient::new(server, token).myself(),
};

/// What Jira offers others.
static SERVICES: [super::feature::Service; 1] = [
    // `{kind: "description" | "comment", arg: issue key, body}` -> sent
    // to Jira (the compose pane's `issue_compose`).
    super::feature::Service {
        name: "jira.compose",
        call: super::feature::ServiceCall::Mut(|app, payload| {
            let call: super::ComposeCall = serde_json::from_str(payload).map_err(|e| format!("a Jira call: {e}"))?;
            let (key, body) = (call.arg, call.body);
            match call.kind.as_str() {
                "description" => app.spawn_jira_action(move |client| {
                    client.update_description(&key, &body)?;
                    Ok(format!("Description updated for {key}"))
                }),
                "comment" => app.spawn_jira_action(move |client| {
                    client.add_comment(&key, &body)?;
                    Ok(format!("Comment added to {key}"))
                }),
                other => return Err(format!("nothing to write to Jira called {other}")),
            }
            Ok(String::new())
        }),
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_blocked_meaning_reads_back_as_it_was_written() {
        for b in [JiraBlocked::Flag, JiraBlocked::Local, JiraBlocked::Status { id: "10103".into(), name: "On Hold".into() }] {
            assert_eq!(JiraBlocked::parse(&b.text()), Ok(b));
        }
        assert!(JiraBlocked::parse("maybe").is_err());
    }

    /// Jira's and the agenda's settings, registered from their specs:
    /// edited in the app, saved, loaded back by key, in order; and a
    /// Blocked meaning that isn't one is refused.
    #[test]
    fn jira_and_agenda_settings_save_and_load_by_key() {
        super::super::feature::register_settings();
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::with_file(None);
        app.config = fenix_config::Config::load_or_default(dir.path().join("settings.toml"));
        // Ten, so "project10" sorting before "project2" as text would show.
        let projects: Vec<(String, String)> = (1..=10).map(|i| (format!("P{i}"), format!("Project {i}"))).collect();
        app.jira_edit_map("jira.projects", |p| p.extend(projects.clone()));
        app.jira_edit_map("jira.users", |u| u.push(("jo1111111".into(), "John Doe".into())));
        app.set_jira_blocked("PROJ", JiraBlocked::Status { id: "10103".into(), name: "On Hold".into() });
        app.set_jira_blocked("OPS", JiraBlocked::Flag);
        app.set_jira_blocked("OPS", JiraBlocked::Local);
        app.config.set_value("jira.priorities", Some(fenix_config::Value::Map(vec![("Major".into(), "High".into())])));
        app.config.set_value("agenda.categories", Some(fenix_config::Value::List(vec!["Fenix".into(), "Personal".into()])));
        app.config.set_value("agenda.worklog_round", Some(fenix_config::Value::Int(30)));
        app.config.save().unwrap();

        app.config = fenix_config::Config::load(dir.path().join("settings.toml")).unwrap();
        assert_eq!(app.config.map_value("jira.projects"), projects.as_slice());
        assert_eq!(app.config.map_value("jira.users"), [("jo1111111".to_string(), "John Doe".to_string())]);
        assert_eq!(app.jira_blocked_for("PROJ"), Some(JiraBlocked::Status { id: "10103".into(), name: "On Hold".into() }));
        assert_eq!(app.jira_blocked_for("OPS"), Some(JiraBlocked::Local), "setting a project twice replaces its entry");
        assert_eq!(app.jira_blocked_for("NOPE"), None);
        assert_eq!(app.agenda_map_priority("Major"), fenix_agenda::Priority::High);
        assert_eq!(app.config.list_value("agenda.categories"), ["Fenix".to_string(), "Personal".to_string()]);
        assert_eq!(app.agenda_worklog_round(), 30);

        let blocked = fenix_config::schema::setting("jira.blocked").unwrap();
        let bad = fenix_config::Value::Map(vec![("X".into(), "maybe".into())]);
        assert!(blocked.set(&mut app.config, Some(bad)).unwrap_err().contains("expected flag, local"));
    }

    fn app() -> App {
        let mut app = App::with_file(None);
        app.config.set_value("jira.projects", Some(fenix_config::Value::Map(vec![("OPS".into(), "Ops".into())])));
        app.config.set_value("jira.users", None);
        app.config.set_value("jira.queries", None);
        app.config.jira_base_url = None;
        app
    }

    #[test]
    fn the_page_opens_on_your_searches_and_says_when_jira_isnt_set_up() {
        let mut app = app();
        let id = app.open_jira_page();
        let pane = app.focused_pane_id();
        app.ensure_page_layout(id, pane, 160);
        let text = app.open().buffer.text();
        assert!(text.contains("Assigned to me") && text.contains("OPS") && text.contains("Current sprint"), "{text}");
        assert!(text.contains("isn't set up"), "{text}");
        assert!(app.page_title(id).contains("jira"));
    }

    #[test]
    fn a_project_added_from_the_page_is_saved_in_the_settings() {
        let mut app = app();
        let dir = tempfile::tempdir().unwrap();
        app.config = fenix_config::Config::load_or_default(dir.path().join("settings.toml"));
        app.open_jira_page();
        for c in "a1".chars() {
            app.page_key(KeyPress::char(c));
        }
        for c in "FEN Fenix".chars() {
            app.page_key(KeyPress::char(c));
        }
        app.page_key(KeyPress::named(FenixNamedKey::Enter));
        assert_eq!(app.config.map_value("jira.projects"), [("FEN".to_string(), "Fenix".to_string())]);
        assert!(std::fs::read_to_string(dir.path().join("settings.toml")).unwrap().contains("FEN"));
    }

    #[test]
    fn jira_contributes_its_commands_and_keys_from_its_spec() {
        let registry = CommandRegistry::with_builtins();
        let jira: Vec<(&str, bool)> = registry.iter().filter(|c| c.id.starts_with("jira.")).map(|c| (c.id.as_ref(), c.external())).collect();
        assert_eq!(jira, [("jira.open", true), ("jira.refresh", false), ("jira.goto_issue", true), ("jira.search", true), ("jira.create_issue", true)]);
        assert_eq!(keymap::leader_keys().get("jira.search").map(String::as_str), Some("SPC j /"));
        assert!(app().features.specs().any(|spec| spec.id == "fenix.jira"));
    }

    #[test]
    fn the_jira_page_is_jiras_laid_out_and_keyed_with_its_context() {
        let mut app = app();
        let id = app.open_jira_page();
        let Some(PageModel::Feature(p)) = app.pages.get(&id).map(|s| &s.model) else { panic!("a feature page") };
        assert_eq!((p.feature(), p.title()), ("fenix.jira", "*jira*".to_string()));
        let idle = app.feature_page_context(id, super::super::feature::PageNeed::Layout { cols: 100, due: false });
        assert!(idle.downcast_ref::<()>().is_some(), "nothing read when no layout is due");
        app.ensure_page_layout(id, app.focused_pane_id(), 100);
        assert!(app.open().buffer.text().contains("OPS"), "laid out with its context: the tracked project");
        assert!(app.page_key(KeyPress::char('q')));
        assert!(!app.is_page_buffer(id), "q closed it, through `page_action`");
    }

    #[test]
    fn a_finished_write_reaches_jira_as_its_feature_event_through_the_spec() {
        let mut app = app();
        app.handle_user_event(FenixUserEvent::feature(SPEC.id, "JiraActionDone", JiraDone(Err("network unreachable".to_string()))));
        assert!(app.status_message.as_ref().is_some_and(|m| m.is_error && m.text.contains("network unreachable")));
        // Another feature's payload, or one Jira doesn't know, is ignored.
        app.handle_user_event(FenixUserEvent::feature(SPEC.id, "test", 42u8));
    }
}
