//! The host half of the Jira page (`jira_page`): opening it on the right
//! searches, running them and fetching issues off the UI thread, and
//! carrying out what its keys ask. Anything an issue in the agenda needs
//! afterwards (a pull, so its task follows) goes through `agenda_sync`.

use super::pages::{PageEvent, PageModel};
use super::*;
use crate::jira_page::{self, Action as JiraAction, Choice, JiraPage, Loaded, QueryKind};

/// How many issues a search lists.
const SEARCH_LIMIT: u32 = 100;

impl App {
    fn jira_page_mut(&mut self, id: BufferId) -> Option<&mut JiraPage> {
        match self.pages.get_mut(&id).map(|s| {
            s.stale = true;
            &mut s.model
        }) {
            Some(PageModel::Jira(p)) => Some(p),
            _ => None,
        }
    }

    pub(super) fn jira_page_id(&self) -> Option<BufferId> {
        self.find_page(|m| matches!(m, PageModel::Jira(_)))
    }

    /// The open project's Jira key.
    fn jira_here(&self) -> Option<String> {
        self.project_root.as_deref().and_then(fenix_project::meta::jira_key)
    }

    fn jira_queries(&self, here: Option<&str>) -> Vec<jira_page::Query> {
        jira_page::queries(&self.config.jira_projects, &self.config.jira_users, &self.config.jira_queries, here)
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
                self.open_page(PageModel::Jira(Box::new(JiraPage::new(queries, here))))
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
        let Some(t) = self.agenda_store.task(task) else { return };
        let from = (task, t.title.clone(), t.description.clone());
        let id = self.open_jira_page();
        let projects = self.jira_form_projects();
        let Some(action) = self.jira_page_mut(id).map(|p| p.start_issue(projects, Some(from))) else { return };
        self.jira_page_action(id, action);
    }

    /// The projects an issue can be created in, the open one first.
    fn jira_form_projects(&self) -> Vec<String> {
        let mut projects: Vec<String> = self.jira_here().into_iter().collect();
        for (key, _) in &self.config.jira_projects {
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
            send(PageEvent::JiraIssues { buffer, jql, result });
        });
    }

    fn jira_fetch_detail(&mut self, buffer: BufferId, key: String) {
        let Some(client) = self.jira_client() else { return };
        self.page_spawn(move |send| {
            let result = client.get_issue(&key);
            send(PageEvent::JiraDetail { buffer, key, result });
        });
    }

    /// A write to issue `key`: shown when done, and the page (and the
    /// agenda, when the issue's in it) brought up to date.
    fn jira_write(&mut self, buffer: BufferId, key: String, job: impl FnOnce(&fenix_jira::JiraClient) -> Result<String, String> + Send + 'static) {
        let Some(client) = self.jira_client() else { return };
        self.page_spawn(move |send| {
            let result = job(&client);
            send(PageEvent::JiraDone { buffer, key, result });
        });
    }

    pub(super) fn apply_jira_event(&mut self, event: PageEvent) {
        match event {
            PageEvent::JiraIssues { buffer, jql, result } => {
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
            PageEvent::JiraDetail { buffer, key, result } => {
                if let Some(page) = self.jira_page_mut(buffer) {
                    page.details.insert(key, result);
                }
            }
            PageEvent::JiraOffer { buffer, title, key, result } => match result {
                Ok(items) => {
                    if let Some(page) = self.jira_page_mut(buffer) {
                        page.offer(title, key, items);
                    }
                }
                Err(err) => self.set_error(format!("{key}: {err}")),
            },
            PageEvent::JiraTypes { buffer, project, types, priorities } => {
                let Some(action) = self.jira_page_mut(buffer).map(|p| p.types_loaded(&project, types, priorities)) else { return };
                self.jira_page_action(buffer, action);
            }
            PageEvent::JiraFields { buffer, type_id, result } => {
                if let Some(page) = self.jira_page_mut(buffer) {
                    page.fields_loaded(&type_id, result);
                }
            }
            PageEvent::JiraDone { buffer, key, result } => {
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
                if self.agenda_store.find_by_key(&key).is_some() {
                    self.agenda_sync_now(true);
                }
            }
            _ => {}
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
                let Some(task) = self.agenda_store.find_by_key(&key) else { return };
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
                    send(PageEvent::JiraOffer { buffer: id, title: format!("Move {key}"), key, result });
                });
            }
            A::Priorities(key) => {
                let Some(client) = self.jira_client() else { return };
                self.page_spawn(move |send| {
                    let result = client.list_priorities().map(|ps| ps.into_iter().map(|p| (p.name.clone(), Choice::Priority(p.name))).collect());
                    send(PageEvent::JiraOffer { buffer: id, title: format!("{key} priority"), key, result });
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
                let me = self.agenda_sync.me.clone();
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
                self.open_compose_seeded(ComposePurpose::IssueDescription { key }, &seed);
            }
            A::Comment(key) => self.open_compose_seeded(ComposePurpose::IssueComment { key }, ""),
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
                if !self.config.jira_projects.iter().any(|(k, _)| k.eq_ignore_ascii_case(&key)) {
                    self.config.jira_projects.push((key.clone(), name));
                }
                self.set_message(format!("{key}'s open issues and sprint are on the left now"));
                self.jira_save_config();
            }
            A::AddPerson { id: user, name } => {
                if !self.config.jira_users.iter().any(|(u, _)| *u == user) {
                    self.config.jira_users.push((user, name));
                }
                self.jira_save_config();
            }
            A::SaveQuery { name, jql } => {
                match self.config.jira_queries.iter_mut().find(|(n, _)| *n == name) {
                    Some(q) => q.1 = jql,
                    None => self.config.jira_queries.push((name.clone(), jql)),
                }
                self.set_message(format!("Saved \"{name}\""));
                self.jira_save_config();
            }
            A::RemoveQuery(q) => {
                match &q.kind {
                    QueryKind::Project(key) => self.config.jira_projects.retain(|(k, _)| k != key),
                    QueryKind::Person(user) => self.config.jira_users.retain(|(u, _)| u != user),
                    QueryKind::Saved => self.config.jira_queries.retain(|(n, _)| *n != q.name),
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
                    send(PageEvent::JiraTypes { buffer: id, project, types, priorities });
                });
            }
            A::CreateFields { project, type_id } => {
                let Some(client) = self.jira_client() else { return };
                self.page_spawn(move |send| {
                    let result = client.create_fields(&project, &type_id);
                    send(PageEvent::JiraFields { buffer: id, type_id, result });
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
                send(PageEvent::JiraDone { buffer, key, result: result.map(|k| format!("Created {k}")) });
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
            self.agenda_store.clock_in(task);
            self.agenda_save_and_refresh();
        }
        let _ = buffer;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> App {
        let mut app = App::with_file(None);
        app.config.jira_projects = vec![("OPS".into(), "Ops".into())];
        app.config.jira_users = Vec::new();
        app.config.jira_queries = Vec::new();
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
        assert_eq!(app.config.jira_projects, vec![("FEN".to_string(), "Fenix".to_string())]);
        assert!(std::fs::read_to_string(dir.path().join("settings.toml")).unwrap().contains("FEN"));
    }
}
