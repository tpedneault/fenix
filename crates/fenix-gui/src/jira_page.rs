//! `SPC j j`: Jira as a page. Searches on the left -- yours (assigned to
//! you, reported by you, watching, recently updated), each tracked
//! project's open issues and current sprint, each tracked person's, and
//! any JQL saved under a name -- the selected one's issues in the middle,
//! and a preview of the selected issue on the right. `Enter` opens an
//! issue's own page (or, when it's in your agenda, its task page there).
//!
//! An issue answers the same keys a task does on the agenda page: `s`
//! status (its real transitions), `p` priority, `A` assign, `u` due,
//! `e`/`E` title and description, `C` comment, `T` log time, `a`/`t` add
//! to the agenda (and start the clock), `y`/`o` link and browser.
//!
//! Pure like the agenda page: the host fetches and sends, this lays out
//! and answers keys with an `Action`.

use std::collections::{BTreeSet, HashMap, HashSet};

use chrono::{Duration, NaiveDate};
use fenix_jira::{CreateField, IssueDetail, IssueSummary, IssueType};

use crate::page::{fit, fit_tail, frame, wrap, Grid, Key, Page, Popup, Role};

/// Which kind of search a query is: what `d` and `b` can do to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueryKind {
    Mine,
    Project(String),
    Person(String),
    Saved,
    /// A search typed with `SPC j /`, until it's saved or closed.
    Search,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Query {
    /// The heading it's listed under: "Me", a project key, "People",
    /// "Saved".
    pub group: String,
    pub name: String,
    pub jql: String,
    pub kind: QueryKind,
}

fn q(group: &str, name: &str, jql: String, kind: QueryKind) -> Query {
    Query { group: group.to_string(), name: name.to_string(), jql, kind }
}

/// Every search the page lists: yours, the open project's (first) and
/// the other tracked projects', each tracked person's, and saved ones.
pub fn queries(projects: &[(String, String)], people: &[(String, String)], saved: &[(String, String)], here: Option<&str>) -> Vec<Query> {
    let open = "statusCategory != Done";
    let mut out = vec![
        q("Me", "Assigned to me", format!("assignee = currentUser() AND {open} ORDER BY updated DESC"), QueryKind::Mine),
        q("Me", "Reported by me", format!("reporter = currentUser() AND {open} ORDER BY updated DESC"), QueryKind::Mine),
        q("Me", "Watching", format!("watcher = currentUser() AND {open} ORDER BY updated DESC"), QueryKind::Mine),
        q(
            "Me",
            "Recently updated",
            "(assignee = currentUser() OR reporter = currentUser() OR watcher = currentUser()) AND updated >= -7d ORDER BY updated DESC".to_string(),
            QueryKind::Mine,
        ),
    ];
    let mut keys: Vec<String> = here.map(str::to_string).into_iter().collect();
    for (key, _) in projects {
        if !keys.iter().any(|k| k.eq_ignore_ascii_case(key)) {
            keys.push(key.clone());
        }
    }
    for key in keys {
        out.push(q(&key, "Current sprint", format!("project = \"{key}\" AND sprint in openSprints() ORDER BY Rank ASC"), QueryKind::Project(key.clone())));
        out.push(q(&key, "Open", format!("project = \"{key}\" AND {open} ORDER BY updated DESC"), QueryKind::Project(key.clone())));
    }
    for (id, name) in people {
        let label = if name.is_empty() { id.as_str() } else { name.as_str() };
        out.push(q("People", &format!("Assigned to {label}"), format!("assignee = \"{id}\" AND {open} ORDER BY updated DESC"), QueryKind::Person(id.clone())));
    }
    for (name, jql) in saved {
        out.push(q("Saved", name, jql.clone(), QueryKind::Saved));
    }
    out
}

/// What a search came back with, so far.
#[derive(Debug, Clone, PartialEq)]
pub enum Loaded {
    Loading,
    Failed(String),
    Issues(Vec<IssueSummary>),
}

/// What the page reads besides its own state.
pub struct Ctx<'a> {
    /// Keys of the issues in the agenda.
    pub in_agenda: &'a HashSet<String>,
    pub people: &'a [(String, String)],
    /// The server, when one is set up; "" when not.
    pub server: String,
    pub today: NaiveDate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Col {
    Queries,
    Issues,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum View {
    List,
    Issue(String),
}

/// A choice in a menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Choice {
    Transition { id: String, name: String },
    Priority(String),
    Assignee { id: String, name: String },
    Due(Option<NaiveDate>),
    TypeDue,
    AddProject,
    AddPerson,
    AddSaved,
}

#[derive(Debug, Clone, PartialEq)]
struct Menu {
    title: String,
    key: String,
    items: Vec<(String, Choice)>,
    at: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Line {
    Title,
    LogTime,
    Due,
    Project,
    Person,
    SavedName,
    SavedJql,
    Goto,
    Search,
    SaveSearch,
}

impl Line {
    fn label(self) -> &'static str {
        match self {
            Line::Title => "Summary",
            Line::LogTime => "Log time (2h 30m)",
            Line::Due => "Due (fri, +3, 2026-10-02, none)",
            Line::Project => "Project key, then its name (FEN Fenix)",
            Line::Person => "Username, then their name (jdoe Jo Doe)",
            Line::SavedName => "Name the search",
            Line::SavedJql => "JQL",
            Line::Goto => "Issue key (FEN-12)",
            Line::Search => "Search Jira (words, or :JQL)",
            Line::SaveSearch => "Save this search as",
        }
    }
}

/// The new-issue form.
#[derive(Debug, Clone, PartialEq)]
pub struct IssueForm {
    pub projects: Vec<String>,
    pub project: usize,
    pub types: Option<Result<Vec<IssueType>, String>>,
    pub issue_type: usize,
    pub summary: String,
    pub priorities: Vec<String>,
    /// 0 is the project's default.
    pub priority: usize,
    /// 0 you, 1 unassigned, then the tracked people.
    pub assignee: usize,
    pub description: String,
    pub fields: Vec<CreateField>,
    /// A value per asked field: an allowed value's index, or text.
    pub values: Vec<String>,
    pub agenda: bool,
    pub clock: bool,
    /// The agenda task it's for, when made from one (`I` on a task).
    pub task: Option<fenix_agenda::TaskId>,
    at: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FormRow {
    Project,
    Type,
    Summary,
    Priority,
    Assignee,
    Description,
    Field(usize),
    Agenda,
    Clock,
}

impl IssueForm {
    fn rows(&self) -> Vec<FormRow> {
        let mut rows = vec![FormRow::Project, FormRow::Type, FormRow::Summary, FormRow::Priority, FormRow::Assignee, FormRow::Description];
        rows.extend(self.asked().into_iter().map(FormRow::Field));
        if self.task.is_none() {
            rows.push(FormRow::Agenda);
        }
        rows.push(FormRow::Clock);
        rows
    }

    /// Indices of the fields the form asks for.
    fn asked(&self) -> Vec<usize> {
        self.fields.iter().enumerate().filter(|(_, f)| f.asked()).map(|(i, _)| i).collect()
    }

    fn row(&self) -> FormRow {
        let rows = self.rows();
        rows[self.at.min(rows.len() - 1)]
    }

    fn type_id(&self) -> Option<&IssueType> {
        match &self.types {
            Some(Ok(types)) => types.get(self.issue_type),
            _ => None,
        }
    }

    fn text(&mut self) -> Option<&mut String> {
        match self.row() {
            FormRow::Summary => Some(&mut self.summary),
            FormRow::Description => Some(&mut self.description),
            FormRow::Field(i) if self.fields[i].allowed.is_empty() => self.values.get_mut(i),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Form {
    Line { line: Line, key: String, text: String },
    Issue(Box<IssueForm>),
}

/// What's needed to create an issue, and what to do after.
#[derive(Debug, Clone, PartialEq)]
pub struct Create {
    pub issue: fenix_jira::NewIssue,
    pub agenda: bool,
    pub clock: bool,
    pub task: Option<fenix_agenda::TaskId>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    None,
    Close,
    /// Run a search (its JQL).
    Fetch(String),
    Detail(String),
    /// A fresh look at the search and issue showing.
    Refresh,
    /// The issue is in the agenda: open its task there.
    OpenTask(String),
    Transitions(String),
    Priorities(String),
    Transition { key: String, id: String, name: String },
    SetPriority { key: String, name: String },
    Assign { key: String, id: String, name: String },
    SetDue { key: String, due: Option<NaiveDate> },
    SetTitle { key: String, title: String },
    EditDescription(String),
    Comment(String),
    LogTime { key: String, time: String },
    AddToAgenda { key: String, clock: bool },
    CopyLink(String),
    Browser(String),
    AddProject { key: String, name: String },
    AddPerson { id: String, name: String },
    SaveQuery { name: String, jql: String },
    RemoveQuery(Query),
    /// What Blocked means in this project.
    Blocked(String),
    /// The new-issue form needs `project`'s issue types (and the
    /// instance's priorities).
    IssueTypes(String),
    CreateFields { project: String, type_id: String },
    Create(Box<Create>),
}

pub struct JiraPage {
    pub queries: Vec<Query>,
    at: usize,
    col: Col,
    pub results: HashMap<String, Loaded>,
    pub details: HashMap<String, Result<IssueDetail, String>>,
    sel: Option<String>,
    idx: usize,
    /// Statuses hidden with the chips.
    hidden: BTreeSet<String>,
    filter: Option<String>,
    filtering: bool,
    pub view: View,
    /// The issue page's row.
    row: usize,
    menu: Option<Menu>,
    form: Option<Form>,
    chips: Option<usize>,
    help: bool,
    armed: bool,
    pub note: Option<(String, bool)>,
    /// The open project's Jira key.
    pub here: Option<String>,
}

impl JiraPage {
    pub fn new(queries: Vec<Query>, here: Option<String>) -> Self {
        JiraPage {
            queries,
            at: 0,
            col: Col::Queries,
            results: HashMap::new(),
            details: HashMap::new(),
            sel: None,
            idx: 0,
            hidden: BTreeSet::new(),
            filter: None,
            filtering: false,
            view: View::List,
            row: 0,
            menu: None,
            form: None,
            chips: None,
            help: false,
            armed: false,
            note: None,
            here,
        }
    }

    /// New searches from the settings, keeping the one selected (and any
    /// typed search).
    pub fn set_queries(&mut self, mut queries: Vec<Query>) {
        let current = self.queries.get(self.at).cloned();
        queries.extend(self.queries.iter().filter(|q| q.kind == QueryKind::Search).cloned());
        self.queries = queries;
        if let Some(i) = current.and_then(|c| self.queries.iter().position(|q| q.jql == c.jql)) {
            self.at = i;
        }
        self.at = self.at.min(self.queries.len().saturating_sub(1));
    }

    /// The search showing.
    pub fn query(&self) -> Option<&Query> {
        self.queries.get(self.at)
    }

    /// The search's JQL, when it hasn't been run yet.
    pub fn wants_fetch(&self) -> Option<String> {
        let jql = &self.query()?.jql;
        (!self.results.contains_key(jql)).then(|| jql.clone())
    }

    /// Runs `text` as a search (words, or JQL after `:`) and shows it.
    pub fn search(&mut self, text: &str) -> Action {
        let text = text.trim();
        if text.is_empty() {
            return Action::None;
        }
        let jql = match text.strip_prefix(':') {
            Some(jql) => jql.trim().to_string(),
            None => format!("text ~ \"{}\" ORDER BY updated DESC", text.replace('"', "\\\"")),
        };
        self.queries.retain(|q| q.kind != QueryKind::Search);
        self.queries.push(q("Search", text, jql.clone(), QueryKind::Search));
        self.at = self.queries.len() - 1;
        self.col = Col::Issues;
        self.view = View::List;
        self.sel = None;
        self.idx = 0;
        self.results.remove(&jql);
        Action::Fetch(jql)
    }

    /// Opens issue `key`'s own page.
    pub fn open_issue(&mut self, key: &str) -> Action {
        self.view = View::Issue(key.to_string());
        self.row = 0;
        if self.details.contains_key(key) { Action::None } else { Action::Detail(key.to_string()) }
    }

    pub fn start_goto(&mut self) {
        self.form = Some(Form::Line { line: Line::Goto, key: String::new(), text: String::new() });
    }

    pub fn start_search(&mut self) {
        self.form = Some(Form::Line { line: Line::Search, key: String::new(), text: String::new() });
    }

    /// The new-issue form, in `projects[0]` (the open project first),
    /// filled from an agenda task when it's for one.
    pub fn start_issue(&mut self, projects: Vec<String>, from: Option<(fenix_agenda::TaskId, String, String)>) -> Action {
        if projects.is_empty() {
            self.note = Some(("add a project first -- a in the searches column".into(), true));
            return Action::None;
        }
        let (task, summary, description) = match from {
            Some((id, s, d)) => (Some(id), s, d),
            None => (None, String::new(), String::new()),
        };
        let project = projects[0].clone();
        self.form = Some(Form::Issue(Box::new(IssueForm {
            projects,
            project: 0,
            types: None,
            issue_type: 0,
            summary,
            priorities: Vec::new(),
            priority: 0,
            assignee: 0,
            description,
            fields: Vec::new(),
            values: Vec::new(),
            agenda: true,
            clock: false,
            task,
            at: if task.is_some() { 1 } else { 2 },
        })));
        Action::IssueTypes(project)
    }

    /// The new-issue form's types (and the instance's priorities) came.
    pub fn types_loaded(&mut self, project: &str, types: Result<Vec<IssueType>, String>, priorities: Vec<String>) -> Action {
        let Some(Form::Issue(f)) = &mut self.form else { return Action::None };
        if f.projects.get(f.project).map(String::as_str) != Some(project) {
            return Action::None;
        }
        let types = types.map(|t| t.into_iter().filter(|t| !t.subtask).collect::<Vec<_>>());
        // A task's usual type first, when there is one.
        f.issue_type = types.as_ref().ok().and_then(|t| t.iter().position(|t| t.name == "Task")).unwrap_or(0);
        f.types = Some(types);
        f.priorities = priorities;
        match f.type_id() {
            Some(t) => Action::CreateFields { project: project.to_string(), type_id: t.id.clone() },
            None => Action::None,
        }
    }

    pub fn fields_loaded(&mut self, type_id: &str, fields: Result<Vec<CreateField>, String>) {
        let Some(Form::Issue(f)) = &mut self.form else { return };
        if f.type_id().map(|t| t.id.as_str()) != Some(type_id) {
            return;
        }
        match fields {
            Ok(fields) => {
                f.values = fields.iter().map(|x| if x.allowed.is_empty() { String::new() } else { "0".to_string() }).collect();
                f.fields = fields;
            }
            Err(err) => self.note = Some((format!("couldn't read what {} needs: {err}", f.projects[f.project]), true)),
        }
    }

    /// A choice for `key` from the host: its transitions, or priorities.
    pub fn offer(&mut self, title: String, key: String, items: Vec<(String, Choice)>) {
        if items.is_empty() {
            self.note = Some((format!("{title}: nothing to choose from"), true));
            return;
        }
        self.menu = Some(Menu { title, key, items, at: 0 });
    }

    pub fn typing(&self) -> bool {
        self.filtering || matches!(self.form, Some(Form::Line { .. })) || matches!(&self.form, Some(Form::Issue(f)) if matches!(f.row(), FormRow::Summary | FormRow::Description) || matches!(f.row(), FormRow::Field(i) if f.fields[i].allowed.is_empty()))
    }

    pub fn claims_space(&self) -> bool {
        self.typing() || self.menu.is_some() || self.chips.is_some() || self.form.is_some()
    }

    pub fn paste(&mut self, text: &str) {
        let text: String = text.chars().filter(|c| !c.is_control()).collect();
        if let Some(t) = self.typed() {
            t.push_str(&text);
        }
    }

    fn typed(&mut self) -> Option<&mut String> {
        if self.filtering {
            return self.filter.as_mut();
        }
        match &mut self.form {
            Some(Form::Line { text, .. }) => Some(text),
            Some(Form::Issue(f)) => f.text(),
            None => None,
        }
    }

    /// The selected search's issues, after the chips and the filter.
    fn issues(&self) -> Vec<&IssueSummary> {
        let Some(Loaded::Issues(issues)) = self.query().and_then(|q| self.results.get(&q.jql)) else { return Vec::new() };
        let needle = self.filter.as_deref().map(|f| f.trim().to_lowercase()).filter(|f| !f.is_empty());
        issues
            .iter()
            .filter(|i| !self.hidden.contains(&i.status))
            .filter(|i| needle.as_ref().is_none_or(|n| i.summary.to_lowercase().contains(n) || i.key.to_lowercase().contains(n)))
            .collect()
    }

    /// The statuses the search came back with.
    fn statuses(&self) -> Vec<String> {
        let Some(Loaded::Issues(issues)) = self.query().and_then(|q| self.results.get(&q.jql)) else { return Vec::new() };
        let set: BTreeSet<String> = issues.iter().map(|i| i.status.clone()).collect();
        set.into_iter().collect()
    }

    /// The selected issue in the list, found again after a refresh.
    fn selected(&mut self) -> Option<String> {
        let keys: Vec<String> = self.issues().iter().map(|i| i.key.clone()).collect();
        if let Some(i) = self.sel.as_ref().and_then(|s| keys.iter().position(|k| k == s)) {
            self.idx = i;
        }
        self.idx = self.idx.min(keys.len().saturating_sub(1));
        self.sel = keys.get(self.idx).cloned();
        self.sel.clone()
    }

    fn current(&self) -> Option<String> {
        let keys: Vec<&str> = self.issues().iter().map(|i| i.key.as_str()).collect();
        match self.sel.as_deref().and_then(|s| keys.iter().position(|k| *k == s)) {
            Some(i) => Some(keys[i].to_string()),
            None => keys.get(self.idx.min(keys.len().saturating_sub(1))).map(|k| k.to_string()),
        }
    }

    /// After moving onto an issue: its detail, when it isn't here yet.
    fn want_detail(&mut self) -> Action {
        match self.selected() {
            Some(key) if !self.details.contains_key(&key) => Action::Detail(key),
            _ => Action::None,
        }
    }

    fn pick_query(&mut self, at: usize) -> Action {
        self.at = at.min(self.queries.len().saturating_sub(1));
        self.sel = None;
        self.idx = 0;
        self.hidden.clear();
        match self.wants_fetch() {
            Some(jql) => {
                self.results.insert(jql.clone(), Loaded::Loading);
                Action::Fetch(jql)
            }
            None => Action::None,
        }
    }

    // -- Keys ------------------------------------------------------------------

    pub fn key(&mut self, key: Key, ctx: &Ctx) -> Action {
        if key != Key::Char('d') {
            self.armed = false;
        }
        if self.help {
            self.help = false;
            return Action::None;
        }
        if self.menu.is_some() {
            return self.menu_key(key);
        }
        if self.form.is_some() {
            return self.form_key(key, ctx);
        }
        if let Some(at) = self.chips {
            let statuses = self.statuses();
            match key {
                Key::Escape | Key::Char('f') | Key::Char('q') => self.chips = None,
                Key::Down | Key::Char('j') => self.chips = Some((at + 1).min(statuses.len().saturating_sub(1))),
                Key::Up | Key::Char('k') => self.chips = Some(at.saturating_sub(1)),
                Key::Char('F') => self.hidden.clear(),
                Key::Space | Key::Enter => {
                    if let Some(s) = statuses.get(at) {
                        if !self.hidden.remove(s) {
                            self.hidden.insert(s.clone());
                        }
                    }
                }
                _ => {}
            }
            return Action::None;
        }
        if self.filtering {
            match key {
                Key::Escape => {
                    self.filtering = false;
                    self.filter = None;
                }
                Key::Enter | Key::Down => self.filtering = false,
                Key::Backspace => {
                    if let Some(f) = &mut self.filter {
                        f.pop();
                    }
                }
                Key::Char(c) => self.filter.get_or_insert_with(String::new).push(c),
                Key::Space => self.filter.get_or_insert_with(String::new).push(' '),
                _ => {}
            }
            self.col = Col::Issues;
            return Action::None;
        }

        match key {
            Key::Char('q') => return Action::Close,
            Key::Char('?') => {
                self.help = true;
                return Action::None;
            }
            Key::Char('R') => {
                if let Some(q) = self.query() {
                    let jql = q.jql.clone();
                    self.results.insert(jql, Loaded::Loading);
                }
                return Action::Refresh;
            }
            Key::Char('n') => {
                let mut projects: Vec<String> = self.here.iter().cloned().collect();
                for q in &self.queries {
                    if let QueryKind::Project(key) = &q.kind {
                        if !projects.contains(key) {
                            projects.push(key.clone());
                        }
                    }
                }
                return self.start_issue(projects, None);
            }
            Key::Char('/') => {
                self.filtering = true;
                self.filter = Some(String::new());
                return Action::None;
            }
            Key::Char('f') => {
                if self.statuses().is_empty() {
                    self.note = Some(("nothing to filter yet".into(), true));
                } else {
                    self.chips = Some(0);
                }
                return Action::None;
            }
            Key::Char('F') => {
                self.hidden.clear();
                self.filter = None;
                return Action::None;
            }
            _ => {}
        }

        if let View::Issue(key_) = self.view.clone() {
            if key == Key::Escape {
                self.view = View::List;
                return Action::None;
            }
            match key {
                Key::Down | Key::Char('j') => {
                    self.row += 1;
                    return Action::None;
                }
                Key::Up | Key::Char('k') => {
                    self.row = self.row.saturating_sub(1);
                    return Action::None;
                }
                Key::Enter => {
                    if ctx.in_agenda.contains(&key_) {
                        return Action::OpenTask(key_);
                    }
                    return Action::None;
                }
                _ => {}
            }
            return self.issue_keys(key, key_, ctx);
        }

        match (key, self.col) {
            (Key::Tab | Key::BackTab | Key::Char('l') | Key::Right, Col::Queries) => {
                self.col = Col::Issues;
                self.want_detail()
            }
            (Key::Enter, Col::Queries) => {
                self.col = Col::Issues;
                self.want_detail()
            }
            (Key::Tab | Key::BackTab | Key::Char('h') | Key::Left | Key::Escape, Col::Issues) => {
                self.col = Col::Queries;
                Action::None
            }
            (Key::Down | Key::Char('j'), Col::Queries) => self.pick_query(self.at + 1),
            (Key::Up | Key::Char('k'), Col::Queries) => self.pick_query(self.at.saturating_sub(1)),
            (Key::Char('g'), Col::Queries) => self.pick_query(0),
            (Key::Char('G'), Col::Queries) => self.pick_query(self.queries.len()),
            (Key::Down | Key::Char('j'), Col::Issues) => {
                let n = self.issues().len();
                self.selected();
                self.idx = (self.idx + 1).min(n.saturating_sub(1));
                self.sel = None;
                self.want_detail()
            }
            (Key::Up | Key::Char('k'), Col::Issues) => {
                self.selected();
                self.idx = self.idx.saturating_sub(1);
                self.sel = None;
                self.want_detail()
            }
            (Key::Char('g'), Col::Issues) => {
                self.idx = 0;
                self.sel = None;
                self.want_detail()
            }
            (Key::Char('G'), Col::Issues) => {
                self.idx = usize::MAX;
                self.sel = None;
                self.want_detail()
            }
            (Key::Enter, Col::Issues) => match self.selected() {
                Some(k) if ctx.in_agenda.contains(&k) => Action::OpenTask(k),
                Some(k) => self.open_issue(&k),
                None => Action::None,
            },
            (Key::Char('a'), Col::Queries) => {
                let items = vec![
                    ("A project: its open issues and sprint".to_string(), Choice::AddProject),
                    ("A person: what's assigned to them".to_string(), Choice::AddPerson),
                    ("A saved search (JQL)".to_string(), Choice::AddSaved),
                ];
                self.menu = Some(Menu { title: "Add a search".into(), key: String::new(), items, at: 0 });
                Action::None
            }
            (Key::Char('d'), Col::Queries) => {
                let Some(q) = self.query().cloned() else { return Action::None };
                if q.kind == QueryKind::Mine {
                    self.note = Some(("yours are always here".into(), true));
                    return Action::None;
                }
                if q.kind == QueryKind::Search {
                    self.queries.remove(self.at);
                    return self.pick_query(self.at.saturating_sub(1));
                }
                if !self.armed {
                    self.armed = true;
                    let what = match &q.kind {
                        QueryKind::Project(k) => format!("stop tracking {k}"),
                        QueryKind::Person(_) => format!("remove \"{}\"", q.name),
                        _ => format!("delete \"{}\"", q.name),
                    };
                    self.note = Some((format!("d again to {what}"), true));
                    return Action::None;
                }
                self.armed = false;
                self.note = None;
                Action::RemoveQuery(q)
            }
            (Key::Char('S'), Col::Queries) if self.query().is_some_and(|q| q.kind == QueryKind::Search) => {
                self.form = Some(Form::Line { line: Line::SaveSearch, key: String::new(), text: String::new() });
                Action::None
            }
            (Key::Char('e'), Col::Queries) => {
                let Some(q) = self.query().cloned().filter(|q| q.kind == QueryKind::Saved) else { return Action::None };
                self.form = Some(Form::Line { line: Line::SavedJql, key: q.name.clone(), text: q.jql.clone() });
                Action::None
            }
            (Key::Char('b'), Col::Queries) => match self.query().map(|q| q.kind.clone()) {
                Some(QueryKind::Project(key)) => Action::Blocked(key),
                _ => Action::None,
            },
            (_, Col::Issues) => match self.selected() {
                Some(k) => self.issue_keys(key, k, ctx),
                None => Action::None,
            },
            _ => Action::None,
        }
    }

    /// The keys an issue answers, on its row or its page.
    fn issue_keys(&mut self, key: Key, k: String, ctx: &Ctx) -> Action {
        match key {
            Key::Char('s') => Action::Transitions(k),
            Key::Char('p') => Action::Priorities(k),
            Key::Char('A') => {
                let mut items: Vec<(String, Choice)> = vec![("Me".to_string(), Choice::Assignee { id: String::new(), name: "you".into() })];
                items.extend(ctx.people.iter().map(|(id, name)| (if name.is_empty() { id.clone() } else { name.clone() }, Choice::Assignee { id: id.clone(), name: name.clone() })));
                self.menu = Some(Menu { title: format!("Assign {k}"), key: k, items, at: 0 });
                Action::None
            }
            Key::Char('u') => {
                let t = ctx.today;
                let friday = t + Duration::days((4 + 7 - chrono::Datelike::weekday(&t).num_days_from_monday() as i64) % 7);
                let items = vec![
                    ("none".to_string(), Choice::Due(None)),
                    ("today".to_string(), Choice::Due(Some(t))),
                    ("tomorrow".to_string(), Choice::Due(Some(t + Duration::days(1)))),
                    (format!("Friday · {}", friday.format("%d %b")), Choice::Due(Some(friday))),
                    (format!("in a week · {}", (t + Duration::days(7)).format("%d %b")), Choice::Due(Some(t + Duration::days(7)))),
                    ("type a date".to_string(), Choice::TypeDue),
                ];
                self.menu = Some(Menu { title: format!("{k} due"), key: k, items, at: 0 });
                Action::None
            }
            Key::Char('e') => {
                let title = self.summary_of(&k).unwrap_or_default();
                self.form = Some(Form::Line { line: Line::Title, key: k, text: title });
                Action::None
            }
            Key::Char('E') => Action::EditDescription(k),
            Key::Char('C') => Action::Comment(k),
            Key::Char('T') => {
                self.form = Some(Form::Line { line: Line::LogTime, key: k, text: String::new() });
                Action::None
            }
            Key::Char('a') => Action::AddToAgenda { key: k, clock: false },
            Key::Char('t') => Action::AddToAgenda { key: k, clock: true },
            Key::Char('y') => Action::CopyLink(k),
            Key::Char('o') => Action::Browser(k),
            _ => Action::None,
        }
    }

    fn summary_of(&self, key: &str) -> Option<String> {
        if let Some(Ok(d)) = self.details.get(key) {
            return Some(d.summary.clone());
        }
        self.results.values().find_map(|l| match l {
            Loaded::Issues(i) => i.iter().find(|i| i.key == key).map(|i| i.summary.clone()),
            _ => None,
        })
    }

    fn menu_key(&mut self, key: Key) -> Action {
        let Some(menu) = &mut self.menu else { return Action::None };
        let n = menu.items.len().max(1);
        match key {
            Key::Escape | Key::Char('q') => self.menu = None,
            Key::Down | Key::Char('j') | Key::Tab => menu.at = (menu.at + 1) % n,
            Key::Up | Key::Char('k') | Key::BackTab => menu.at = (menu.at + n - 1) % n,
            Key::Char(c @ '1'..='9') if (c as usize - '1' as usize) < menu.items.len() => {
                menu.at = c as usize - '1' as usize;
                return self.menu_pick();
            }
            Key::Enter | Key::Space => return self.menu_pick(),
            _ => {}
        }
        Action::None
    }

    fn menu_pick(&mut self) -> Action {
        let Some(menu) = self.menu.take() else { return Action::None };
        let Some((_, choice)) = menu.items.get(menu.at).cloned() else { return Action::None };
        let key = menu.key;
        let line = |page: &mut Self, line: Line| {
            page.form = Some(Form::Line { line, key: key.clone(), text: String::new() });
            Action::None
        };
        match choice {
            Choice::Transition { id, name } => Action::Transition { key, id, name },
            Choice::Priority(name) => Action::SetPriority { key, name },
            Choice::Assignee { id, name } => Action::Assign { key, id, name },
            Choice::Due(due) => Action::SetDue { key, due },
            Choice::TypeDue => line(self, Line::Due),
            Choice::AddProject => line(self, Line::Project),
            Choice::AddPerson => line(self, Line::Person),
            Choice::AddSaved => line(self, Line::SavedName),
        }
    }

    fn form_key(&mut self, key: Key, ctx: &Ctx) -> Action {
        match key {
            Key::Escape => {
                self.form = None;
                self.note = None;
                return Action::None;
            }
            Key::Backspace => {
                if let Some(t) = self.typed() {
                    t.pop();
                }
                return Action::None;
            }
            Key::Char(c) if self.typed().is_some() => {
                if let Some(t) = self.typed() {
                    t.push(c);
                }
                return Action::None;
            }
            Key::Space if self.typed().is_some() => {
                if let Some(t) = self.typed() {
                    t.push(' ');
                }
                return Action::None;
            }
            _ => {}
        }
        match self.form.clone() {
            Some(Form::Line { line, key: k, text }) if key == Key::Enter => self.finish_line(line, k, text, ctx),
            Some(Form::Issue(_)) => self.issue_form_key(key, ctx),
            _ => Action::None,
        }
    }

    fn finish_line(&mut self, line: Line, key: String, text: String, ctx: &Ctx) -> Action {
        let value = text.trim().to_string();
        self.form = None;
        let refuse = |page: &mut Self, why: &str| {
            page.form = Some(Form::Line { line, key: key.clone(), text: text.clone() });
            page.note = Some((why.to_string(), true));
            Action::None
        };
        if value.is_empty() {
            return Action::None;
        }
        match line {
            Line::Title => Action::SetTitle { key, title: value },
            Line::LogTime => Action::LogTime { key, time: value },
            Line::Due => match crate::agenda_page::parse_date(&value, ctx.today) {
                Ok(due) => Action::SetDue { key, due },
                Err(why) => refuse(self, &why),
            },
            Line::Project | Line::Person => {
                let (first, rest) = value.split_once(' ').unwrap_or((&value, ""));
                if line == Line::Project {
                    Action::AddProject { key: first.to_uppercase(), name: rest.trim().to_string() }
                } else {
                    Action::AddPerson { id: first.to_string(), name: rest.trim().to_string() }
                }
            }
            Line::SavedName => {
                self.form = Some(Form::Line { line: Line::SavedJql, key: value, text: String::new() });
                Action::None
            }
            Line::SavedJql => Action::SaveQuery { name: key, jql: value },
            Line::SaveSearch => match self.query().filter(|q| q.kind == QueryKind::Search).cloned() {
                Some(q) => Action::SaveQuery { name: value, jql: q.jql },
                None => Action::None,
            },
            Line::Goto => {
                let k = value.to_uppercase();
                if ctx.in_agenda.contains(&k) {
                    return Action::OpenTask(k);
                }
                self.open_issue(&k)
            }
            Line::Search => self.search(&value),
        }
    }

    fn issue_form_key(&mut self, key: Key, ctx: &Ctx) -> Action {
        let Some(Form::Issue(f)) = &mut self.form else { return Action::None };
        let n = f.rows().len();
        let row = f.row();
        let step = |i: &mut usize, len: usize, forward: bool| {
            if len > 0 {
                *i = if forward { (*i + 1) % len } else { (*i + len - 1) % len };
            }
        };
        match key {
            Key::Tab | Key::Down => f.at = (f.at + 1) % n,
            Key::BackTab | Key::Up => f.at = (f.at + n - 1) % n,
            Key::Char('h') | Key::Char('l') | Key::Left | Key::Right | Key::Space => {
                let forward = !matches!(key, Key::Char('h') | Key::Left);
                match row {
                    FormRow::Project => {
                        step(&mut f.project, f.projects.len(), forward);
                        f.types = None;
                        f.fields.clear();
                        return Action::IssueTypes(f.projects[f.project].clone());
                    }
                    FormRow::Type => {
                        let len = match &f.types {
                            Some(Ok(t)) => t.len(),
                            _ => 0,
                        };
                        step(&mut f.issue_type, len, forward);
                        f.fields.clear();
                        if let Some(t) = f.type_id() {
                            return Action::CreateFields { project: f.projects[f.project].clone(), type_id: t.id.clone() };
                        }
                    }
                    FormRow::Priority => step(&mut f.priority, f.priorities.len() + 1, forward),
                    FormRow::Assignee => step(&mut f.assignee, ctx.people.len() + 2, forward),
                    FormRow::Field(i) => {
                        let len = f.fields[i].allowed.len();
                        let mut at: usize = f.values[i].parse().unwrap_or(0);
                        step(&mut at, len, forward);
                        f.values[i] = at.to_string();
                    }
                    FormRow::Agenda => f.agenda = !f.agenda,
                    FormRow::Clock => f.clock = !f.clock,
                    _ => {}
                }
            }
            Key::Enter => return self.submit_issue(ctx),
            _ => {}
        }
        Action::None
    }

    fn submit_issue(&mut self, ctx: &Ctx) -> Action {
        let Some(Form::Issue(f)) = &mut self.form else { return Action::None };
        let bad = |page: &mut Self, why: String| {
            page.note = Some((why, true));
            Action::None
        };
        let Some(issue_type) = f.type_id().cloned() else {
            let why = match &f.types {
                Some(Err(e)) => format!("couldn't read the issue types: {e}"),
                _ => "still reading the project's issue types".to_string(),
            };
            return bad(self, why);
        };
        if f.summary.trim().is_empty() {
            f.at = 2;
            return bad(self, "an issue needs a summary".into());
        }
        let mut extra = Vec::new();
        for i in f.asked() {
            let field = &f.fields[i];
            let value = &f.values[i];
            let v = if field.allowed.is_empty() {
                if value.trim().is_empty() {
                    let name = field.name.clone();
                    return bad(self, format!("{name} is required for {}", issue_type.name));
                }
                value.trim().to_string()
            } else {
                let at: usize = value.parse().unwrap_or(0);
                field.allowed.get(at).map(|(id, _)| id.clone()).unwrap_or_default()
            };
            extra.push((field.id.clone(), field.value_json(&v)));
        }
        let assignee = match f.assignee {
            0 => None,
            1 => Some(String::new()),
            n => ctx.people.get(n - 2).map(|(id, _)| id.clone()),
        };
        let create = Create {
            issue: fenix_jira::NewIssue {
                project: f.projects[f.project].clone(),
                issue_type_id: issue_type.id,
                summary: f.summary.trim().to_string(),
                description: f.description.clone(),
                priority: f.priority.checked_sub(1).and_then(|i| f.priorities.get(i).cloned()),
                assignee,
                extra,
            },
            agenda: f.agenda || f.task.is_some(),
            clock: f.clock,
            task: f.task,
        };
        self.form = None;
        self.note = None;
        Action::Create(Box::new(create))
    }
}

// -- Layout ----------------------------------------------------------------

fn status_role(category: &str) -> Role {
    match category {
        "done" => Role::Good,
        "indeterminate" => Role::Warn,
        _ => Role::Muted,
    }
}

fn priority_mark(name: Option<&str>) -> (&'static str, Role) {
    match name.map(fenix_agenda::jira::guess_priority) {
        Some(fenix_agenda::Priority::Urgent) => ("!!", Role::Bad),
        Some(fenix_agenda::Priority::High) => ("! ", Role::Warn),
        Some(fenix_agenda::Priority::Medium) => ("· ", Role::Muted),
        _ => ("  ", Role::Muted),
    }
}

fn when(raw: &str) -> String {
    chrono::DateTime::parse_from_str(raw, "%Y-%m-%dT%H:%M:%S%.3f%z").map(|t| t.with_timezone(&chrono::Local).format("%-d %b %H:%M").to_string()).unwrap_or_default()
}

pub fn layout(page: &JiraPage, ctx: &Ctx, cols: usize) -> Page {
    let (left, width) = frame(cols, 170);
    let mut g = Grid::new();
    let crumb = if matches!(page.view, View::Issue(_)) { "Jira ›" } else { "Jira" };
    g.put(1, left, crumb, if matches!(page.view, View::Issue(_)) { Role::Muted } else { Role::Title });
    let server = if ctx.server.is_empty() { "not set up -- Jira server and token on the settings page (SPC ,)".to_string() } else { ctx.server.clone() };
    g.put(1, (left + width).saturating_sub(server.chars().count()), &server, if ctx.server.is_empty() { Role::Warn } else { Role::Muted });
    let filter = match (&page.filter, page.filtering) {
        (Some(f), true) => format!("/ {f}▏"),
        (Some(f), false) if !f.is_empty() => format!("/ {f}  (Esc clears)"),
        _ => "/ filter these issues".to_string(),
    };
    let end = g.put(2, left, &filter, if page.filtering { Role::Title } else { Role::Muted });
    if page.filtering {
        g.panels.push((2, left..end.max(left + 30)));
    }
    let mut y = 3;
    if let Some((text, bad)) = &page.note {
        g.put(y, left, &fit(text, width), if *bad { Role::Bad } else { Role::Good });
        y += 1;
    } else if !page.hidden.is_empty() {
        g.put(y, left, &fit(&format!("hiding {} -- f to change", page.hidden.iter().cloned().collect::<Vec<_>>().join(", ")), width), Role::Warn);
        y += 1;
    }
    g.rule(y, left..left + width);
    y += 1;
    let top = y;
    let mut anchor = top;
    match &page.view {
        View::List => layout_list(&mut g, page, ctx, left, width, top, &mut anchor),
        View::Issue(key) => layout_issue(&mut g, page, ctx, key, left, width, top, &mut anchor),
    }
    popups(&mut g, page, ctx, anchor, left);
    g.keys(left, width, &key_strip(page));
    g.finish()
}

fn layout_list(g: &mut Grid, page: &JiraPage, ctx: &Ctx, left: usize, width: usize, top: usize, anchor: &mut usize) {
    let qw = 30;
    let preview = if width >= 130 { (width - qw) * 2 / 5 } else { 0 };
    let ix = left + qw + 2;
    let iw = width.saturating_sub(qw + 2 + if preview > 0 { preview + 3 } else { 0 });

    // The searches.
    let mut y = top;
    let mut group = "";
    for (i, q) in page.queries.iter().enumerate() {
        if q.group != group {
            if !group.is_empty() {
                y += 1;
            }
            let label = if Some(q.group.as_str()) == page.here.as_deref() { format!("{} · this project", q.group) } else { q.group.clone() };
            g.heading(y, left, qw, &label);
            group = &q.group;
            y += 1;
        }
        let on = i == page.at;
        g.put(y, left + 1, &fit(&q.name, qw - 6), if on { Role::Title } else { Role::Text });
        if let Some(loaded) = page.results.get(&q.jql) {
            let n = match loaded {
                Loaded::Issues(i) => i.len().to_string(),
                Loaded::Loading => "…".to_string(),
                Loaded::Failed(_) => "!".to_string(),
            };
            g.put(y, left + qw - n.chars().count(), &n, if matches!(loaded, Loaded::Failed(_)) { Role::Bad } else { Role::Muted });
        }
        if on {
            if page.col == Col::Queries {
                g.focus(y, left..left + qw);
                *anchor = y;
            } else {
                g.panels.push((y, left..left + qw));
            }
        }
        y += 1;
    }
    y += 1;
    g.put(y, left + 1, "+ add a search  (a)", Role::Accent);

    // The issues.
    let mut y = top;
    let title = page.query().map(|q| q.name.clone()).unwrap_or_default();
    g.heading(y, ix, iw, &title);
    y += 1;
    let current = page.current();
    match page.query().and_then(|q| page.results.get(&q.jql)) {
        None | Some(Loaded::Loading) => {
            g.put(y, ix + 1, "Loading…", Role::Muted);
        }
        Some(Loaded::Failed(err)) => {
            for line in wrap(&format!("Jira said: {err}"), iw - 2).into_iter().take(6) {
                g.put(y, ix + 1, &line, Role::Bad);
                y += 1;
            }
        }
        Some(Loaded::Issues(_)) => {
            let issues = page.issues();
            if issues.is_empty() {
                g.put(y, ix + 1, "Nothing here.", Role::Muted);
            }
            for issue in issues {
                let (mark, role) = priority_mark(issue.priority.as_deref());
                let mut x = g.put(y, ix, mark, role) + 1;
                x = g.put(y, x, &format!("{:<10}", issue.key), Role::Accent) + 1;
                let mut right: Vec<(String, Role)> = vec![(issue.status.clone(), status_role(&issue.status_category))];
                if ctx.in_agenda.contains(&issue.key) {
                    right.insert(0, ("✓ agenda".to_string(), Role::Good));
                }
                let right_w: usize = right.iter().map(|(t, _)| t.chars().count() + 2).sum();
                g.put(y, x, &fit(&issue.summary, (ix + iw).saturating_sub(x + right_w + 1)), Role::Text);
                let mut rx = ix + iw;
                for (text, role) in right.iter().rev() {
                    rx = rx.saturating_sub(text.chars().count());
                    g.put(y, rx, text, *role);
                    rx = rx.saturating_sub(2);
                }
                if current.as_deref() == Some(issue.key.as_str()) {
                    if page.col == Col::Issues {
                        g.focus(y, ix..ix + iw);
                        *anchor = y;
                    } else {
                        g.panels.push((y, ix..ix + iw));
                    }
                }
                y += 1;
            }
        }
    }

    // The preview.
    if preview > 0 {
        let px = left + width - preview;
        let Some(key) = current else { return };
        let mut y = top;
        match page.details.get(&key) {
            None => {
                g.put(y, px, &format!("{key}  loading…"), Role::Muted);
            }
            Some(Err(err)) => {
                g.put(y, px, &fit(&format!("{key}: {err}"), preview), Role::Bad);
            }
            Some(Ok(d)) => {
                let x = g.put(y, px, &d.key, Role::Accent) + 2;
                let mut sub = d.issue_type.clone().unwrap_or_default();
                if ctx.in_agenda.contains(&d.key) {
                    sub.push_str(" · in your agenda");
                }
                g.put(y, x, &sub, Role::Muted);
                y += 1;
                for line in wrap(&d.summary, preview).into_iter().take(3) {
                    g.put(y, px, &line, Role::Title);
                    y += 1;
                }
                y += 1;
                let rows = [
                    ("Status", d.status.clone(), status_role(&d.status_category)),
                    ("Priority", d.priority.clone().unwrap_or_else(|| "none".into()), Role::Text),
                    ("Assignee", d.assignee.clone().unwrap_or_else(|| "nobody".into()), Role::Text),
                    ("Reporter", d.reporter.clone().unwrap_or_default(), Role::Text),
                    ("Due", d.due.clone().unwrap_or_else(|| "none".into()), Role::Text),
                    ("Updated", when(&d.updated), Role::Muted),
                ];
                for (label, value, role) in rows {
                    g.put(y, px, label, Role::Muted);
                    g.put(y, px + 10, &fit(&value, preview - 10), role);
                    y += 1;
                }
                if let Some(desc) = d.description.as_deref().filter(|s| !s.trim().is_empty()) {
                    y += 1;
                    g.heading(y, px, preview, "Description");
                    y += 1;
                    for line in desc.lines().flat_map(|l| wrap(l, preview)).take(8) {
                        g.put(y, px, &line, Role::Text);
                        y += 1;
                    }
                }
                if let Some(c) = d.comments.last() {
                    y += 1;
                    g.heading(y, px, preview, &format!("Latest comment · {}", d.comments.len()));
                    y += 1;
                    g.put(y, px, &format!("{}  {}", c.author, when(&c.created)), Role::Accent);
                    y += 1;
                    for line in c.body.lines().flat_map(|l| wrap(l, preview)).take(5) {
                        g.put(y, px, &line, Role::Text);
                        y += 1;
                    }
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn layout_issue(g: &mut Grid, page: &JiraPage, ctx: &Ctx, key: &str, left: usize, width: usize, top: usize, anchor: &mut usize) {
    let mut y = top;
    let d = match page.details.get(key) {
        None => {
            g.put(y, left, &format!("{key}  loading…"), Role::Muted);
            return;
        }
        Some(Err(err)) => {
            g.put(y, left, &fit(&format!("{key}: {err}"), width), Role::Bad);
            return;
        }
        Some(Ok(d)) => d,
    };
    let mut rows: Vec<usize> = Vec::new();
    let x = g.put(y, left, &d.key, Role::Accent) + 2;
    g.put(y, x, &fit(&d.summary, width.saturating_sub(x - left + 20)), Role::Title);
    let tag = if ctx.in_agenda.contains(&d.key) { "✓ in your agenda · Enter" } else { "a adds it to your agenda" };
    g.put(y, (left + width).saturating_sub(tag.chars().count()), tag, if ctx.in_agenda.contains(&d.key) { Role::Good } else { Role::Muted });
    y += 2;
    let fields = [
        ("Type", d.issue_type.clone().unwrap_or_default(), Role::Text, ""),
        ("Status", d.status.clone(), status_role(&d.status_category), "s"),
        ("Priority", d.priority.clone().unwrap_or_else(|| "none".into()), Role::Text, "p"),
        ("Assignee", d.assignee.clone().unwrap_or_else(|| "nobody".into()), Role::Text, "A"),
        ("Reporter", d.reporter.clone().unwrap_or_default(), Role::Text, ""),
        ("Due", d.due.clone().unwrap_or_else(|| "none".into()), Role::Text, "u"),
        ("Created", when(&d.created), Role::Muted, ""),
        ("Updated", when(&d.updated), Role::Muted, ""),
    ];
    for (label, value, role, key) in fields {
        g.put(y, left + 1, label, Role::Muted);
        g.put(y, left + 12, &fit(&value, width - 20), role);
        if !key.is_empty() {
            g.put(y, (left + width).saturating_sub(1), key, Role::Accent);
        }
        rows.push(y);
        y += 1;
    }
    y += 1;
    g.heading(y, left, width.saturating_sub(10), "Description");
    g.put(y, (left + width).saturating_sub(7), "E edits", Role::Muted);
    y += 1;
    rows.push(y);
    match d.description.as_deref().filter(|s| !s.trim().is_empty()) {
        Some(desc) => {
            for line in desc.lines().flat_map(|l| if l.trim().is_empty() { vec![String::new()] } else { wrap(l, width - 2) }) {
                g.put(y, left + 1, &line, Role::Text);
                y += 1;
            }
        }
        None => {
            g.put(y, left + 1, "none", Role::Muted);
            y += 1;
        }
    }
    y += 1;
    g.heading(y, left, width.saturating_sub(12), &format!("Comments · {}", d.comments.len()));
    g.put(y, (left + width).saturating_sub(9), "C writes", Role::Muted);
    y += 1;
    for c in &d.comments {
        rows.push(y);
        g.put(y, left + 1, &format!("{}  {}", c.author, when(&c.created)), Role::Accent);
        y += 1;
        for line in c.body.lines().flat_map(|l| wrap(l, width - 4)) {
            g.put(y, left + 3, &line, Role::Text);
            y += 1;
        }
        y += 1;
    }
    let at = rows[page.row.min(rows.len() - 1)];
    g.focus(at, left..left + width);
    *anchor = at;
}

fn popups(g: &mut Grid, page: &JiraPage, ctx: &Ctx, line: usize, left: usize) {
    let col = left + 4;
    if page.help {
        let groups: [(&str, &[(&str, &str)]); 3] = [
            ("Move", &[("h l Tab", "searches, issues"), ("j k g G", "rows"), ("Enter", "open"), ("Esc", "back"), ("/", "filter"), ("f F", "hide statuses, show all"), ("R", "refresh"), ("q", "close")]),
            ("Searches", &[("a", "add a project, person or saved search"), ("d d", "remove it"), ("e", "edit a saved search"), ("S", "save a typed search"), ("b", "what Blocked means in a project")]),
            (
                "An issue",
                &[("s p A u", "status, priority, assign, due"), ("e E", "summary, description"), ("C T", "comment, log time"), ("a t", "add to agenda, and start the clock"), ("y o", "copy link, browser"), ("n", "new issue")],
            ),
        ];
        let mut rows = vec![vec![("Keys".to_string(), Role::Title)]];
        for (title, keys) in groups {
            rows.push(Vec::new());
            rows.push(vec![(title.to_uppercase(), Role::Muted)]);
            for (k, what) in keys {
                rows.push(vec![(format!("{k:<10}"), Role::Accent), (what.to_string(), Role::Text)]);
            }
        }
        rows.push(Vec::new());
        rows.push(vec![("any key closes this".into(), Role::Muted)]);
        g.popup = Some(Popup { line, col, rows });
        return;
    }
    if let Some(menu) = &page.menu {
        let mut rows = vec![vec![(menu.title.clone(), Role::Title)], Vec::new()];
        for (i, (label, _)) in menu.items.iter().enumerate() {
            let on = i == menu.at;
            let n = if i < 9 { format!("{} ", i + 1) } else { "  ".to_string() };
            rows.push(vec![(n, Role::Accent), (format!("{}{label}", if on { "› " } else { "  " }), if on { Role::Title } else { Role::Text })]);
        }
        rows.push(Vec::new());
        rows.push(vec![("Enter picks · Esc leaves it".into(), Role::Muted)]);
        g.popup = Some(Popup { line, col, rows });
        return;
    }
    match &page.form {
        Some(Form::Line { line: l, key, text }) => {
            let title = if key.is_empty() || matches!(l, Line::SavedJql) { l.label().to_string() } else { format!("{key} · {}", l.label()) };
            let mut rows = vec![vec![(title, Role::Title)], Vec::new(), vec![(format!("{:<50}", fit_tail(&format!("{text}▏"), 70)), Role::Title)]];
            if let Some((why, true)) = &page.note {
                rows.push(Vec::new());
                rows.push(vec![(why.clone(), Role::Bad)]);
            }
            rows.push(Vec::new());
            rows.push(vec![("Enter keeps · Esc leaves it".into(), Role::Muted)]);
            g.popup = Some(Popup { line, col, rows });
            return;
        }
        Some(Form::Issue(f)) => {
            let title = if f.task.is_some() { "New issue from the task" } else { "New issue" };
            let mut rows = vec![vec![(title.to_string(), Role::Title)], Vec::new()];
            let current = f.row();
            let caret = |s: &str, on: bool| if on { fit_tail(&format!("{s}▏"), 56) } else if s.is_empty() { "…".to_string() } else { fit(s, 56) };
            for row in f.rows() {
                let on = row == current;
                let (label, value) = match row {
                    FormRow::Project => ("Project".to_string(), format!("‹ {} ›", f.projects[f.project])),
                    FormRow::Type => (
                        "Type".to_string(),
                        match &f.types {
                            None => "loading…".to_string(),
                            Some(Err(e)) => fit(&format!("couldn't read: {e}"), 50),
                            Some(Ok(t)) => t.get(f.issue_type).map(|t| format!("‹ {} ›", t.name)).unwrap_or_else(|| "none".into()),
                        },
                    ),
                    FormRow::Summary => ("Summary".to_string(), caret(&f.summary, on)),
                    FormRow::Priority => ("Priority".to_string(), format!("‹ {} ›", f.priority.checked_sub(1).and_then(|i| f.priorities.get(i)).map(String::as_str).unwrap_or("project default"))),
                    FormRow::Assignee => (
                        "Assignee".to_string(),
                        format!(
                            "‹ {} ›",
                            match f.assignee {
                                0 => "you".to_string(),
                                1 => "unassigned".to_string(),
                                n => ctx.people.get(n - 2).map(|(id, name)| if name.is_empty() { id.clone() } else { name.clone() }).unwrap_or_default(),
                            }
                        ),
                    ),
                    FormRow::Description => ("Description".to_string(), caret(&f.description, on)),
                    FormRow::Field(i) => {
                        let field = &f.fields[i];
                        let value = if field.allowed.is_empty() {
                            caret(&f.values[i], on)
                        } else {
                            let at: usize = f.values[i].parse().unwrap_or(0);
                            format!("‹ {} ›", field.allowed.get(at).map(|(_, n)| n.as_str()).unwrap_or("none"))
                        };
                        (format!("{} *", field.name), value)
                    }
                    FormRow::Agenda => ("Agenda".to_string(), if f.agenda { "[x] add it to your agenda".into() } else { "[ ] add it to your agenda".into() }),
                    FormRow::Clock => ("Clock".to_string(), if f.clock { "[x] start the clock on it".into() } else { "[ ] start the clock on it".into() }),
                };
                rows.push(vec![(format!("{:<14}", fit(&label, 13)), if on { Role::Accent } else { Role::Muted }), (format!("{value:<44}"), if on { Role::Title } else { Role::Text })]);
            }
            if let Some((why, true)) = &page.note {
                rows.push(Vec::new());
                rows.push(vec![(why.clone(), Role::Bad)]);
            }
            rows.push(Vec::new());
            rows.push(vec![("Enter creates · Tab next field · h/l change · Esc cancels".into(), Role::Muted)]);
            g.popup = Some(Popup { line, col, rows });
            return;
        }
        None => {}
    }
    if let Some(at) = page.chips {
        let mut rows = vec![vec![("Show statuses".to_string(), Role::Title)], Vec::new()];
        for (i, s) in page.statuses().into_iter().enumerate() {
            let on = !page.hidden.contains(&s);
            rows.push(vec![
                (if on { "[x] " } else { "[ ] " }.to_string(), if on { Role::Good } else { Role::Muted }),
                (s, if i == at { Role::Title } else { Role::Text }),
                (if i == at { "  ‹" } else { "" }.to_string(), Role::Accent),
            ]);
        }
        rows.push(Vec::new());
        rows.push(vec![("Space toggles · F shows all · Esc done".into(), Role::Muted)]);
        g.popup = Some(Popup { line, col, rows });
    }
}

fn key_strip(page: &JiraPage) -> Vec<(&'static str, &'static str)> {
    if page.filtering {
        return vec![("Enter", "keep"), ("Esc", "clear")];
    }
    if page.menu.is_some() || page.help {
        return vec![("Enter", "pick"), ("Esc", "leave it")];
    }
    if page.form.is_some() {
        return vec![("Enter", "keep"), ("Esc", "leave it")];
    }
    if page.chips.is_some() {
        return vec![("Space", "toggle"), ("F", "show all"), ("Esc", "done")];
    }
    let mut keys = match (&page.view, page.col) {
        (View::Issue(_), _) => vec![("s", "status"), ("p", "priority"), ("A", "assign"), ("u", "due"), ("C", "comment"), ("T", "log time"), ("a", "agenda"), ("t", "clock"), ("o", "browser"), ("Esc", "back")],
        (View::List, Col::Queries) => vec![("j/k", "search"), ("l", "issues"), ("a", "add a search"), ("d", "remove"), ("b", "Blocked means")],
        (View::List, Col::Issues) => vec![("Enter", "open"), ("s", "status"), ("p", "priority"), ("A", "assign"), ("a", "agenda"), ("t", "clock"), ("C", "comment"), ("h", "searches")],
    };
    keys.extend([("n", "new issue"), ("/", "filter"), ("R", "refresh"), ("?", "all keys"), ("q", "close")]);
    keys
}

#[cfg(test)]
mod tests {
    use super::*;

    fn issue(key: &str, status: &str, category: &str) -> IssueSummary {
        IssueSummary { key: key.into(), summary: format!("{key} summary"), status: status.into(), status_category: category.into(), priority: Some("Major".into()), ..Default::default() }
    }

    fn page() -> JiraPage {
        let projects = vec![("OPS".to_string(), "Ops".to_string())];
        let people = vec![("jdoe".to_string(), "Jo Doe".to_string())];
        JiraPage::new(queries(&projects, &people, &[("Bugs".into(), "type = Bug".into())], Some("FEN")), Some("FEN".into()))
    }

    fn ctx<'a>(in_agenda: &'a HashSet<String>, people: &'a [(String, String)]) -> Ctx<'a> {
        Ctx { in_agenda, people, server: "jira.example.com".into(), today: NaiveDate::from_ymd_opt(2026, 9, 24).unwrap() }
    }

    #[test]
    fn searches_start_with_yours_then_this_project_then_the_rest() {
        let p = page();
        let names: Vec<(String, String)> = p.queries.iter().map(|q| (q.group.clone(), q.name.clone())).collect();
        assert_eq!(names[0], ("Me".into(), "Assigned to me".into()));
        assert_eq!(names[4], ("FEN".into(), "Current sprint".into()), "the open project comes first");
        assert_eq!(names[6].0, "OPS");
        assert_eq!(names[8], ("People".into(), "Assigned to Jo Doe".into()));
        assert_eq!(names[9], ("Saved".into(), "Bugs".into()));
        assert!(p.queries[4].jql.contains("openSprints()"));
    }

    #[test]
    fn moving_through_searches_fetches_each_once_and_issues_fetch_their_detail() {
        let mut p = page();
        let set = HashSet::new();
        let c = ctx(&set, &[]);
        let Action::Fetch(jql) = p.key(Key::Char('j'), &c) else { panic!() };
        assert!(jql.starts_with("reporter"));
        assert_eq!(p.key(Key::Char('k'), &c), Action::Fetch(p.queries[0].jql.clone()));
        p.results.insert(p.queries[0].jql.clone(), Loaded::Issues(vec![issue("FEN-1", "To Do", "new"), issue("FEN-2", "In Review", "indeterminate")]));
        assert_eq!(p.key(Key::Char('l'), &c), Action::Detail("FEN-1".into()));
        assert_eq!(p.key(Key::Char('j'), &c), Action::Detail("FEN-2".into()));
        let text = layout(&p, &c, 160).text;
        assert!(text.contains("FEN-2") && text.contains("In Review"), "{text}");
    }

    #[test]
    fn an_issue_answers_the_task_keys_and_one_in_the_agenda_opens_its_task() {
        let mut p = page();
        let mut set = HashSet::new();
        set.insert("FEN-2".to_string());
        let people = vec![("jdoe".to_string(), "Jo Doe".to_string())];
        let c = ctx(&set, &people);
        p.results.insert(p.queries[0].jql.clone(), Loaded::Issues(vec![issue("FEN-1", "To Do", "new"), issue("FEN-2", "To Do", "new")]));
        p.key(Key::Char('l'), &c);
        assert_eq!(p.key(Key::Char('s'), &c), Action::Transitions("FEN-1".into()));
        assert_eq!(p.key(Key::Char('t'), &c), Action::AddToAgenda { key: "FEN-1".into(), clock: true });
        p.key(Key::Char('A'), &c);
        assert_eq!(p.key(Key::Char('2'), &c), Action::Assign { key: "FEN-1".into(), id: "jdoe".into(), name: "Jo Doe".into() });
        p.key(Key::Char('e'), &c);
        p.paste(" now");
        assert_eq!(p.key(Key::Enter, &c), Action::SetTitle { key: "FEN-1".into(), title: "FEN-1 summary now".into() });
        assert_eq!(p.key(Key::Enter, &c), Action::Detail("FEN-1".into()), "not in the agenda: its own page");
        assert_eq!(p.view, View::Issue("FEN-1".into()));
        p.key(Key::Escape, &c);
        p.key(Key::Char('j'), &c);
        assert_eq!(p.key(Key::Enter, &c), Action::OpenTask("FEN-2".into()));
    }

    #[test]
    fn searches_are_added_and_removed_from_the_left_column() {
        let mut p = page();
        let set = HashSet::new();
        let c = ctx(&set, &[]);
        p.key(Key::Char('a'), &c);
        p.key(Key::Char('1'), &c);
        p.paste("abc Alpha Beta");
        assert_eq!(p.key(Key::Enter, &c), Action::AddProject { key: "ABC".into(), name: "Alpha Beta".into() });
        p.key(Key::Char('a'), &c);
        p.key(Key::Char('3'), &c);
        p.paste("Mine");
        p.key(Key::Enter, &c);
        p.paste("assignee = currentUser()");
        assert_eq!(p.key(Key::Enter, &c), Action::SaveQuery { name: "Mine".into(), jql: "assignee = currentUser()".into() });
        p.key(Key::Char('G'), &c);
        assert_eq!(p.key(Key::Char('d'), &c), Action::None, "asks first");
        assert_eq!(p.key(Key::Char('d'), &c), Action::RemoveQuery(p.queries.last().unwrap().clone()));
        p.key(Key::Char('g'), &c);
        p.key(Key::Char('d'), &c);
        assert!(p.note.as_ref().unwrap().0.contains("always here"));
    }

    #[test]
    fn a_typed_search_is_words_or_jql_and_can_be_saved() {
        let mut p = page();
        assert_eq!(p.search("login crash"), Action::Fetch("text ~ \"login crash\" ORDER BY updated DESC".into()));
        assert_eq!(p.search(":project = FEN"), Action::Fetch("project = FEN".into()));
        assert_eq!(p.queries.iter().filter(|q| q.kind == QueryKind::Search).count(), 1, "one typed search at a time");
    }

    #[test]
    fn the_issue_form_uses_the_projects_types_and_asks_for_required_fields() {
        let mut p = page();
        let set = HashSet::new();
        let c = ctx(&set, &[]);
        assert_eq!(p.key(Key::Char('n'), &c), Action::IssueTypes("FEN".into()));
        let types = vec![IssueType { id: "1".into(), name: "Bug".into(), subtask: false }, IssueType { id: "3".into(), name: "Task".into(), subtask: false }];
        assert_eq!(p.types_loaded("FEN", Ok(types), vec!["Major".into()]), Action::CreateFields { project: "FEN".into(), type_id: "3".into() });
        let field = CreateField { id: "components".into(), name: "Component/s".into(), required: true, allowed: vec![("10".into(), "UI".into())], kind: "array".into() };
        p.fields_loaded("3", Ok(vec![field]));
        let text = layout(&p, &c, 160).popup.unwrap().text();
        assert!(text.contains("‹ Task ›") && text.contains("Component/s *"), "{text}");
        p.paste("Crash on save");
        let Action::Create(create) = p.key(Key::Enter, &c) else { panic!("{:?}", p.note) };
        assert_eq!(create.issue.summary, "Crash on save");
        assert_eq!(create.issue.issue_type_id, "3");
        assert_eq!(create.issue.extra[0].1, serde_json::json!([{"id": "10"}]));
        assert!(create.agenda);
    }
}
