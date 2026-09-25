//! `SPC a a`: the agenda as a page. Four tabs -- Today, Board, List and
//! Time -- and a page per task, all answering one set of keys: a letter
//! for each thing you can do to a task (`s` status, `p` priority, `t`
//! clock, ...), the same on a row, a card and the task's own page.
//! Choices open as menus beside the row, typing as a small form, and the
//! key strip follows what the cursor is on. `/` searches every tab, `f`
//! narrows it, `P` keeps to the project the page was opened from.
//!
//! Like the settings page this is pure: it reads the store through a
//! `Ctx` the host builds, and answers a key with an `Action` the host
//! carries out (saving, syncing with Jira, opening a compose pane).

use std::collections::BTreeSet;

use chrono::{DateTime, Datelike, Duration, Local, NaiveDate, NaiveTime, TimeZone, Weekday};
use fenix_agenda::{AgendaStore, Priority, Status, SyncField, Task, TaskId, WorklogRow};

use crate::page::{fit, fit_tail, frame, wrap, Grid, Key, Page, Popup, Role};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Today,
    Board,
    List,
    Time,
}

impl Tab {
    pub const ALL: [Tab; 4] = [Tab::Today, Tab::Board, Tab::List, Tab::Time];

    pub fn label(self) -> &'static str {
        match self {
            Tab::Today => "Today",
            Tab::Board => "Board",
            Tab::List => "List",
            Tab::Time => "Time",
        }
    }

    fn index(self) -> usize {
        Tab::ALL.iter().position(|t| *t == self).unwrap_or(0)
    }
}

/// The project the page was opened from: tasks filed under a category
/// of its name, and issues in its Jira project, are its tasks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    pub name: String,
    pub jira_key: Option<String>,
}

/// What the page reads, built by the host for each key and layout.
pub struct Ctx<'a> {
    pub store: &'a AgendaStore,
    pub now: DateTime<Local>,
    pub categories: &'a [String],
    /// The worklog review's rows: unsent time, as it would be sent.
    pub worklogs: &'a [WorklogRow],
    pub round: u32,
    /// "synced 3m ago", "syncing", or empty when Jira isn't set up.
    pub sync: String,
}

impl Ctx<'_> {
    fn today(&self) -> NaiveDate {
        self.now.date_naive()
    }

    fn task(&self, id: TaskId) -> Option<&Task> {
        self.store.task(id)
    }
}

/// What narrows every tab, besides the search.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Filter {
    /// High and Urgent only.
    pub pressing: bool,
    /// `Some(true)` linked to Jira only, `Some(false)` local only.
    pub linked: Option<bool>,
    /// Due within a week, or overdue.
    pub due_soon: bool,
    pub categories: BTreeSet<String>,
}

impl Filter {
    fn is_empty(&self) -> bool {
        *self == Filter::default()
    }
}

/// Which page is showing: a tab, or one task's own page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Tab(Tab),
    Task(TaskId),
}

/// A field at the top of a task's page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Title,
    Status,
    Priority,
    Category,
    Due,
}

impl Field {
    const ALL: [Field; 5] = [Field::Title, Field::Status, Field::Priority, Field::Category, Field::Due];

    fn label(self) -> &'static str {
        match self {
            Field::Title => "Title",
            Field::Status => "Status",
            Field::Priority => "Priority",
            Field::Category => "Category",
            Field::Due => "Due",
        }
    }
}

/// What the row under the cursor is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row {
    Task(TaskId),
    /// Today's "+ N more in List".
    More,
    /// Today's unsent-time line: goes to Time.
    Unsent,
    Field(TaskId, Field),
    Description(TaskId),
    Code(TaskId),
    Check(TaskId, usize),
    AddCheck(TaskId),
    DependsOn(TaskId, TaskId),
    AddDependency(TaskId),
    Blocks(TaskId, TaskId),
    Conflict(TaskId, SyncField),
    Note(TaskId, usize),
    Comment(TaskId, usize),
    AddNote(TaskId),
    Time(TaskId, usize),
    AddTime(TaskId),
}

impl Row {
    /// The task a task key (`s`, `p`, `t`, ...) acts on from this row.
    fn task(self) -> Option<TaskId> {
        match self {
            Row::Task(id)
            | Row::Field(id, _)
            | Row::Description(id)
            | Row::Code(id)
            | Row::Check(id, _)
            | Row::AddCheck(id)
            | Row::DependsOn(id, _)
            | Row::AddDependency(id)
            | Row::Blocks(id, _)
            | Row::Conflict(id, _)
            | Row::Note(id, _)
            | Row::Comment(id, _)
            | Row::AddNote(id)
            | Row::Time(id, _)
            | Row::AddTime(id) => Some(id),
            Row::More | Row::Unsent => None,
        }
    }
}

/// A new task, as the form left it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewTask {
    pub title: String,
    pub priority: Priority,
    pub category: Option<String>,
    pub due: Option<NaiveDate>,
    pub status: Status,
    pub clock: bool,
    /// A Jira key typed in the title: link the task to it.
    pub link: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    None,
    Close,
    SetStatus(TaskId, Status),
    /// A linked task's status: its real transitions, from Jira.
    JiraStatus(TaskId),
    SetPriority(TaskId, Priority),
    /// A linked task's priority: the instance's own names.
    JiraPriority(TaskId),
    SetCategory(TaskId, Option<String>),
    /// A category that doesn't exist yet: add it, and file the task.
    AddCategory(TaskId, String),
    SetDue(TaskId, Option<NaiveDate>),
    SetTitle(TaskId, String),
    EditDescription(TaskId),
    AddNote(TaskId, String),
    EditNote(TaskId, usize, String),
    RemoveNote(TaskId, usize),
    /// Write a Jira comment in a compose pane.
    Comment(TaskId),
    /// Post a private note as a Jira comment.
    PostNote(TaskId, usize),
    /// Start the task's clock, or stop it if it's this one running.
    Clock(TaskId),
    LogSpan(TaskId, DateTime<Local>, DateTime<Local>),
    EditTime(TaskId, usize, DateTime<Local>, DateTime<Local>),
    RemoveTime(TaskId, usize),
    AddCheck(TaskId, String),
    EditCheck(TaskId, usize, String),
    ToggleCheck(TaskId, usize),
    RemoveCheck(TaskId, usize),
    AddDependency(TaskId, TaskId),
    RemoveDependency(TaskId, TaskId),
    Resolve(TaskId, SyncField, bool),
    /// Link to an issue, or unlink (`I`).
    Link(TaskId),
    Assign(TaskId),
    CopyLink(TaskId),
    Browser(TaskId),
    Retry(TaskId),
    OpenCode(TaskId),
    Archive(TaskId),
    Delete(TaskId),
    Reorder(TaskId, isize),
    Create(NewTask),
    Sync,
    WorklogMinutes(TaskId, NaiveDate, i64),
    WorklogDrop(TaskId, NaiveDate),
    WorklogDismiss(TaskId, NaiveDate),
    SendWorklogs,
    /// The week as text, for the clipboard.
    Copy(String),
}

/// One choice in a menu.
#[derive(Debug, Clone, PartialEq)]
enum Pick {
    Status(Status),
    Priority(Priority),
    Category(Option<String>),
    NewCategory,
    Due(Option<NaiveDate>),
    TypeDue,
    Dependency(TaskId),
    Keep(bool),
}

#[derive(Debug, Clone, PartialEq)]
struct Menu {
    title: String,
    task: TaskId,
    items: Vec<(String, Pick)>,
    at: usize,
    /// Which conflict a Keep pick settles.
    field: Option<SyncField>,
}

/// What a one-line form is typing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Typing {
    Title(TaskId),
    Note(TaskId),
    EditNote(TaskId, usize),
    Check(TaskId),
    EditCheck(TaskId, usize),
    Due(TaskId),
    LogTime(TaskId),
    EditTime(TaskId, usize),
    Category(TaskId),
    Worklog(TaskId, NaiveDate),
}

impl Typing {
    fn label(self) -> &'static str {
        match self {
            Typing::Title(_) => "Title",
            Typing::Note(_) => "Private note",
            Typing::EditNote(..) => "Note",
            Typing::Check(_) => "Checklist item",
            Typing::EditCheck(..) => "Checklist item",
            Typing::Due(_) => "Due (fri, +3, 2026-10-02, none)",
            Typing::LogTime(_) => "Log time (1h 30m, or 9:15-10:40)",
            Typing::EditTime(..) => "When (9:15-10:40)",
            Typing::Category(_) => "New category",
            Typing::Worklog(..) => "Send (1h 30m)",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Form {
    Line { typing: Typing, text: String },
    New { title: String, priority: Priority, category: Option<String>, due: String, status: Status, clock: bool, at: usize },
}

/// The new-task form's rows.
const NEW_FIELDS: [&str; 5] = ["Title", "Priority", "Category", "Due", "Start"];

pub struct AgendaPage {
    pub view: View,
    /// Where `Esc` goes back to from a task's page, and what was selected.
    back: Vec<(View, Option<Row>)>,
    /// The tab last shown, for `SPC a a`.
    pub tab: Tab,
    pub project: Option<Project>,
    pub only_project: bool,
    pub filter: Filter,
    search: Option<String>,
    searching: bool,
    /// The selected row, and its index as a fallback when it's gone.
    sel: Option<Row>,
    idx: usize,
    /// The board's column, for an empty one.
    col: usize,
    /// Weeks back from this one, on Time.
    pub week: i64,
    menu: Option<Menu>,
    form: Option<Form>,
    /// The worklog review, open over Time, and its row.
    worklogs: Option<usize>,
    /// The filter chips, open, and the chip under the cursor.
    filters: Option<usize>,
    help: bool,
    /// `D` once asks; again deletes.
    armed: Option<Row>,
    pub note: Option<(String, bool)>,
}

impl AgendaPage {
    pub fn new(tab: Tab, project: Option<Project>) -> Self {
        AgendaPage {
            view: View::Tab(tab),
            back: Vec::new(),
            tab,
            only_project: false,
            project,
            filter: Filter::default(),
            search: None,
            searching: false,
            sel: None,
            idx: 0,
            col: 0,
            week: 0,
            menu: None,
            form: None,
            worklogs: None,
            filters: None,
            help: false,
            armed: None,
            note: None,
        }
    }

    pub fn show_tab(&mut self, tab: Tab) {
        self.view = View::Tab(tab);
        self.tab = tab;
        self.back.clear();
        self.sel = None;
        self.idx = 0;
        self.menu = None;
        self.form = None;
        self.worklogs = None;
    }

    /// Opens a task's page, remembering where it was opened from.
    pub fn open_task(&mut self, id: TaskId) {
        if self.view == View::Task(id) {
            return;
        }
        self.back.push((self.view, self.sel));
        self.view = View::Task(id);
        self.sel = None;
        self.idx = 0;
    }

    /// Opens a task's page on its Time section.
    fn open_task_time(&mut self, id: TaskId, ctx: &Ctx) {
        self.open_task(id);
        let rows = self.rows(ctx);
        if let Some(i) = rows.iter().position(|r| matches!(r, Row::Time(..) | Row::AddTime(_))) {
            self.idx = i;
            self.sel = Some(rows[i]);
        }
    }

    /// Opens the worklog review over Time.
    pub fn open_worklogs(&mut self) {
        self.show_tab(Tab::Time);
        self.worklogs = Some(0);
    }

    /// Selects `id`'s row, if it has one on the current view.
    pub fn select(&mut self, id: TaskId) {
        self.sel = Some(Row::Task(id));
    }

    /// The page's task, when it's showing one that no longer exists
    /// (deleted from here, or elsewhere), goes back.
    pub fn forget_missing(&mut self, store: &AgendaStore) {
        while let View::Task(id) = self.view {
            if store.task(id).is_some() {
                break;
            }
            self.go_back();
        }
    }

    fn go_back(&mut self) {
        match self.back.pop() {
            Some((view, sel)) => {
                self.view = view;
                self.sel = sel;
            }
            None => self.view = View::Tab(self.tab),
        }
    }

    pub fn typing(&self) -> bool {
        self.searching || self.form.is_some()
    }

    /// Space is the page's while typing, in a menu or the filters, and on
    /// a checklist row.
    pub fn claims_space(&self) -> bool {
        self.typing() || self.menu.is_some() || self.filters.is_some() || matches!(self.sel, Some(Row::Check(..)))
    }

    pub fn paste(&mut self, text: &str) {
        let text: String = text.chars().filter(|c| !c.is_control()).collect();
        if let Some(t) = self.typed() {
            t.push_str(&text);
        }
    }

    fn typed(&mut self) -> Option<&mut String> {
        if self.searching {
            return self.search.as_mut();
        }
        match &mut self.form {
            Some(Form::Line { text, .. }) => Some(text),
            Some(Form::New { title, at: 0, .. }) => Some(title),
            Some(Form::New { due, at: 3, .. }) => Some(due),
            _ => None,
        }
    }

    fn scope_label(&self) -> Option<String> {
        let p = self.project.as_ref().filter(|_| self.only_project)?;
        Some(format!("{} only", p.name))
    }

    // -- What shows ----------------------------------------------------------

    /// Whether `t` passes the search, the filters and the project scope.
    fn shown(&self, t: &Task, ctx: &Ctx) -> bool {
        let searching = self.search.as_deref().is_some_and(|s| !s.trim().is_empty());
        if t.archived && !searching {
            return false;
        }
        if let Some(needle) = self.search.as_deref().map(|s| s.trim().to_lowercase()).filter(|s| !s.is_empty()) {
            let hay = [t.title.as_str(), t.description.as_str(), t.category.as_deref().unwrap_or(""), t.jira_key().unwrap_or("")];
            let hit = hay.iter().any(|h| h.to_lowercase().contains(&needle)) || t.notes.iter().any(|n| n.text.to_lowercase().contains(&needle));
            if !hit {
                return false;
            }
        }
        let f = &self.filter;
        if f.pressing && t.priority < Priority::High {
            return false;
        }
        if let Some(linked) = f.linked {
            if t.jira.is_some() != linked {
                return false;
            }
        }
        if f.due_soon && !t.due.is_some_and(|d| t.status != Status::Done && d <= ctx.today() + Duration::days(7)) {
            return false;
        }
        if !f.categories.is_empty() && !t.category.as_ref().is_some_and(|c| f.categories.contains(c)) {
            return false;
        }
        if let Some(p) = self.project.as_ref().filter(|_| self.only_project) {
            if !t.in_project(&p.name, p.jira_key.as_deref()) {
                return false;
            }
        }
        true
    }

    fn tasks<'a>(&self, ctx: &'a Ctx) -> Vec<&'a Task> {
        ctx.store.tasks.iter().filter(|t| self.shown(t, ctx)).collect()
    }

    /// Today's sections: title, and the rows in it.
    fn today(&self, ctx: &Ctx) -> (Vec<(String, Vec<Row>)>, usize) {
        let tasks = self.tasks(ctx);
        let today = ctx.today();
        let rank = |t: &&Task| (std::cmp::Reverse(t.priority), t.order);
        let mut taken: BTreeSet<TaskId> = BTreeSet::new();
        let mut sections = Vec::new();

        let mut progress: Vec<&Task> = tasks.iter().copied().filter(|t| t.status == Status::InProgress && !t.archived).collect();
        progress.sort_by_key(rank);
        taken.extend(progress.iter().map(|t| t.id));
        sections.push(("In progress".to_string(), progress.iter().map(|t| Row::Task(t.id)).collect::<Vec<_>>()));

        let mut due: Vec<&Task> = tasks
            .iter()
            .copied()
            .filter(|t| !taken.contains(&t.id) && t.status != Status::Done && !t.archived && t.due.is_some_and(|d| d <= today + Duration::days(7)))
            .collect();
        due.sort_by_key(|t| (t.due, std::cmp::Reverse(t.priority)));
        taken.extend(due.iter().map(|t| t.id));
        sections.push(("Due".to_string(), due.iter().map(|t| Row::Task(t.id)).collect()));

        let mut next: Vec<&Task> =
            tasks.iter().copied().filter(|t| !taken.contains(&t.id) && t.status == Status::Todo && !t.archived && ctx.store.is_ready(t.id)).collect();
        next.sort_by_key(rank);
        let more = next.len().saturating_sub(NEXT_UP);
        let mut rows: Vec<Row> = next.iter().take(NEXT_UP).map(|t| Row::Task(t.id)).collect();
        taken.extend(next.iter().take(NEXT_UP).map(|t| t.id));
        if more > 0 {
            rows.push(Row::More);
        }
        let title = if more > 0 { format!("Next up · {} of {}", NEXT_UP, next.len()) } else { "Next up".to_string() };
        sections.push((title, rows));

        let mut waiting: Vec<&Task> = tasks
            .iter()
            .copied()
            .filter(|t| !taken.contains(&t.id) && !t.archived && (t.status == Status::Blocked || (t.status == Status::Todo && !ctx.store.is_ready(t.id))))
            .collect();
        waiting.sort_by_key(rank);
        taken.extend(waiting.iter().map(|t| t.id));
        sections.push(("Waiting".to_string(), waiting.iter().map(|t| Row::Task(t.id)).collect()));

        let mut needs: Vec<Row> = tasks
            .iter()
            .filter(|t| t.jira.as_ref().is_some_and(|l| !l.conflicts.is_empty() || l.error.is_some()))
            .map(|t| Row::Task(t.id))
            .collect();
        if ctx.worklogs.iter().any(|r| r.minutes > 0) {
            needs.push(Row::Unsent);
        }
        sections.push(("Needs you".to_string(), needs));

        let done_today = tasks.iter().filter(|t| t.status == Status::Done && t.done_at.is_some_and(|d| d.date_naive() == today)).count();
        (sections, done_today)
    }

    /// The board's columns: each status's cards, and how many older Done
    /// cards are folded away.
    fn columns<'a>(&self, ctx: &'a Ctx) -> Vec<(Vec<&'a Task>, usize)> {
        let tasks = self.tasks(ctx);
        let week_ago = ctx.now - Duration::days(7);
        Status::ALL
            .iter()
            .map(|&s| {
                let mut col: Vec<&Task> = tasks.iter().copied().filter(|t| t.status == s).collect();
                col.sort_by_key(|t| (!ctx.store.is_ready(t.id), std::cmp::Reverse(t.priority), t.order));
                if s == Status::Done {
                    col.sort_by_key(|t| std::cmp::Reverse(t.done_at));
                    let recent = col.iter().filter(|t| t.done_at.is_some_and(|d| d >= week_ago)).count().max(col.len().min(3));
                    let older = col.len() - recent;
                    col.truncate(recent);
                    return (col, older);
                }
                (col, 0)
            })
            .collect()
    }

    /// List's sections: one per status.
    fn list(&self, ctx: &Ctx) -> Vec<(String, Vec<Row>)> {
        let tasks = self.tasks(ctx);
        Status::ALL
            .iter()
            .map(|&s| {
                let mut col: Vec<&Task> = tasks.iter().copied().filter(|t| t.status == s).collect();
                col.sort_by_key(|t| (!ctx.store.is_ready(t.id), std::cmp::Reverse(t.priority), t.order));
                (s.label().to_string(), col.iter().map(|t| Row::Task(t.id)).collect())
            })
            .collect()
    }

    /// The Monday the Time tab shows.
    fn monday(&self, ctx: &Ctx) -> NaiveDate {
        let today = ctx.today();
        today - Duration::days(today.weekday().num_days_from_monday() as i64) - Duration::weeks(self.week)
    }

    /// Minutes per task per weekday in the shown week, running clock
    /// included; tasks in the order they were first worked on.
    fn week_minutes(&self, ctx: &Ctx) -> Vec<(TaskId, [i64; 7])> {
        let monday = self.monday(ctx);
        let mut out: Vec<(TaskId, [i64; 7])> = Vec::new();
        let mut add = |id: TaskId, start: DateTime<Local>, end: DateTime<Local>| {
            let day = start.date_naive();
            let offset = (day - monday).num_days();
            if !(0..7).contains(&offset) {
                return;
            }
            let minutes = (end - start).num_minutes().max(0);
            match out.iter_mut().find(|(t, _)| *t == id) {
                Some((_, days)) => days[offset as usize] += minutes,
                None => {
                    let mut days = [0; 7];
                    days[offset as usize] = minutes;
                    out.push((id, days));
                }
            }
        };
        for t in ctx.store.tasks.iter().filter(|t| self.shown_for_time(t, ctx)) {
            for e in &t.time_entries {
                add(t.id, e.start, e.end);
            }
        }
        if let Some(timer) = &ctx.store.active_timer {
            if ctx.task(timer.task_id).is_some_and(|t| self.shown_for_time(t, ctx)) {
                add(timer.task_id, timer.started_at, ctx.now);
            }
        }
        out
    }

    /// Time shows archived tasks too: the time was still spent.
    fn shown_for_time(&self, t: &Task, ctx: &Ctx) -> bool {
        if t.archived {
            let mut unarchived = t.clone();
            unarchived.archived = false;
            return self.shown(&unarchived, ctx);
        }
        self.shown(t, ctx)
    }

    /// A task's own page, row by row.
    fn task_rows(&self, id: TaskId, ctx: &Ctx) -> Vec<Row> {
        let Some(t) = ctx.task(id) else { return Vec::new() };
        let mut rows: Vec<Row> = Field::ALL.iter().map(|f| Row::Field(id, *f)).collect();
        if t.code.is_some() {
            rows.push(Row::Code(id));
        }
        if let Some(link) = &t.jira {
            rows.extend(link.conflicts.iter().map(|c| Row::Conflict(id, c.field)));
        }
        rows.push(Row::Description(id));
        rows.extend((0..t.subtasks.len()).map(|i| Row::Check(id, i)));
        rows.push(Row::AddCheck(id));
        rows.extend(t.depends_on.iter().map(|d| Row::DependsOn(id, *d)));
        rows.push(Row::AddDependency(id));
        rows.extend(ctx.store.blocks(id).iter().map(|b| Row::Blocks(id, b.id)));
        rows.extend(activity(t).into_iter().map(|a| match a {
            Activity::Note(i) => Row::Note(id, i),
            Activity::Comment(i) => Row::Comment(id, i),
        }));
        rows.push(Row::AddNote(id));
        rows.extend((0..t.time_entries.len()).map(|i| Row::Time(id, i)));
        rows.push(Row::AddTime(id));
        rows
    }

    /// Every selectable row of the current view, in order.
    fn rows(&self, ctx: &Ctx) -> Vec<Row> {
        match self.view {
            View::Task(id) => self.task_rows(id, ctx),
            View::Tab(Tab::Today) => self.today(ctx).0.into_iter().flat_map(|(_, rows)| rows).collect(),
            View::Tab(Tab::List) => self.list(ctx).into_iter().flat_map(|(_, rows)| rows).collect(),
            View::Tab(Tab::Board) => self.columns(ctx).into_iter().flat_map(|(col, _)| col.into_iter().map(|t| Row::Task(t.id))).collect(),
            View::Tab(Tab::Time) => self.week_minutes(ctx).into_iter().map(|(id, _)| Row::Task(id)).collect(),
        }
    }

    /// The selected row, found again after the rows changed.
    fn resolve(&mut self, rows: &[Row]) -> Option<Row> {
        if let Some(i) = self.sel.and_then(|s| rows.iter().position(|r| *r == s)) {
            self.idx = i;
        } else {
            self.idx = self.idx.min(rows.len().saturating_sub(1));
        }
        self.sel = rows.get(self.idx).copied();
        self.sel
    }

    /// The selection as the page is now -- for the layout, which can't
    /// move it.
    fn current(&self, rows: &[Row]) -> Option<Row> {
        match self.sel.and_then(|s| rows.iter().position(|r| *r == s)) {
            Some(i) => rows.get(i).copied(),
            None => rows.get(self.idx.min(rows.len().saturating_sub(1))).copied(),
        }
    }

    fn move_by(&mut self, rows: &[Row], by: isize) {
        if rows.is_empty() {
            return;
        }
        self.idx = (self.idx as isize + by).clamp(0, rows.len() as isize - 1) as usize;
        self.sel = rows.get(self.idx).copied();
    }

    // -- Keys ------------------------------------------------------------------

    pub fn key(&mut self, key: Key, ctx: &Ctx) -> Action {
        if key != Key::Char('D') && key != Key::Char('d') {
            self.armed = None;
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
        if let Some(at) = self.worklogs {
            return self.worklog_key(key, at, ctx);
        }
        if let Some(at) = self.filters {
            self.filter_key(key, at, ctx);
            return Action::None;
        }
        if self.searching {
            match key {
                Key::Escape => {
                    self.searching = false;
                    self.search = None;
                }
                Key::Enter | Key::Down => self.searching = false,
                Key::Backspace => {
                    if let Some(s) = &mut self.search {
                        s.pop();
                    }
                }
                Key::Char(c) => self.search.get_or_insert_with(String::new).push(c),
                Key::Space => self.search.get_or_insert_with(String::new).push(' '),
                _ => {}
            }
            self.idx = 0;
            self.sel = None;
            return Action::None;
        }

        let rows = self.rows(ctx);
        let row = self.resolve(&rows);
        let task = row.and_then(Row::task).and_then(|id| ctx.task(id));

        // Keys every view shares.
        match key {
            Key::Char(c @ '1'..='4') => {
                self.show_tab(Tab::ALL[c as usize - '1' as usize]);
                return Action::None;
            }
            Key::Tab | Key::BackTab => {
                let i = self.tab.index() + if key == Key::Tab { 1 } else { 3 };
                self.show_tab(Tab::ALL[i % 4]);
                return Action::None;
            }
            Key::Char('q') => return Action::Close,
            Key::Char('?') => {
                self.help = true;
                return Action::None;
            }
            Key::Char('/') => {
                self.searching = true;
                self.search = Some(String::new());
                return Action::None;
            }
            Key::Char('f') => {
                self.filters = Some(0);
                return Action::None;
            }
            Key::Char('F') => {
                self.filter = Filter::default();
                self.search = None;
                self.only_project = false;
                self.note = Some(("filters cleared".into(), false));
                return Action::None;
            }
            Key::Char('P') => {
                match &self.project {
                    Some(p) => {
                        self.only_project = !self.only_project;
                        let what = if self.only_project { format!("only {}", p.name) } else { "every project".to_string() };
                        self.note = Some((what, false));
                    }
                    None => self.note = Some(("opened outside a project -- open the agenda from a file in one".into(), true)),
                }
                return Action::None;
            }
            Key::Char('R') => return Action::Sync,
            Key::Char('n') => {
                self.new_task(ctx);
                return Action::None;
            }
            Key::Char('g') => {
                self.idx = 0;
                self.sel = rows.first().copied();
                return Action::None;
            }
            Key::Char('G') => {
                self.idx = rows.len().saturating_sub(1);
                self.sel = rows.last().copied();
                return Action::None;
            }
            Key::Escape => {
                if matches!(self.view, View::Task(_)) {
                    self.go_back();
                } else if self.search.is_some() {
                    self.search = None;
                }
                return Action::None;
            }
            _ => {}
        }

        if self.view == View::Tab(Tab::Board) {
            if let Some(action) = self.board_key(key, ctx) {
                return action;
            }
        }
        if self.view == View::Tab(Tab::Time) {
            match key {
                Key::Char('[') => {
                    self.week += 1;
                    return Action::None;
                }
                Key::Char(']') => {
                    self.week = (self.week - 1).max(0);
                    return Action::None;
                }
                Key::Char('W') => {
                    self.worklogs = Some(0);
                    return Action::None;
                }
                Key::Char('y') => return Action::Copy(self.week_text(ctx)),
                Key::Enter => {
                    if let Some(Row::Task(id)) = row {
                        self.open_task_time(id, ctx);
                    }
                    return Action::None;
                }
                _ => {}
            }
        }

        match key {
            Key::Down | Key::Char('j') => {
                self.move_by(&rows, 1);
                return Action::None;
            }
            Key::Up | Key::Char('k') => {
                self.move_by(&rows, -1);
                return Action::None;
            }
            _ => {}
        }

        if let View::Task(_) = self.view {
            if let Some(action) = self.task_page_key(key, row, ctx) {
                return action;
            }
        } else if key == Key::Enter {
            match row {
                Some(Row::Task(id)) => self.open_task(id),
                Some(Row::More) => self.show_tab(Tab::List),
                Some(Row::Unsent) => self.open_worklogs(),
                _ => {}
            }
            return Action::None;
        }

        let Some(t) = task else { return Action::None };
        self.task_key(key, t, row, ctx)
    }

    /// The keys a task answers wherever it is.
    fn task_key(&mut self, key: Key, t: &Task, row: Option<Row>, ctx: &Ctx) -> Action {
        let id = t.id;
        let linked = t.jira.is_some();
        match key {
            Key::Char('s') => {
                if linked {
                    return Action::JiraStatus(id);
                }
                self.status_menu(t);
            }
            Key::Char('p') => {
                if linked {
                    return Action::JiraPriority(id);
                }
                self.priority_menu(t);
            }
            Key::Char('c') => self.category_menu(t, ctx),
            Key::Char('u') => self.due_menu(t, ctx),
            Key::Char('e') => self.type_line(Typing::Title(id), &t.title),
            Key::Char('E') => return Action::EditDescription(id),
            Key::Char('N') => self.type_line(Typing::Note(id), ""),
            Key::Char('C') => {
                if let Some(Row::Note(_, i)) = row {
                    if linked {
                        return Action::PostNote(id, i);
                    }
                }
                if !linked {
                    self.note = Some(("C posts a Jira comment -- link this task with I first, or N for a private note".into(), true));
                    return Action::None;
                }
                return Action::Comment(id);
            }
            Key::Char('t') => return Action::Clock(id),
            Key::Char('T') => self.type_line(Typing::LogTime(id), ""),
            Key::Char('I') => return Action::Link(id),
            Key::Char('A') if linked => return Action::Assign(id),
            Key::Char('y') if linked => return Action::CopyLink(id),
            Key::Char('o') if linked => return Action::Browser(id),
            Key::Char('r') if linked => return Action::Retry(id),
            Key::Char('x') => {
                self.note = Some((format!("\"{}\" archived -- / finds it again", fit(&t.title, 40)), false));
                return Action::Archive(id);
            }
            Key::Char('D') => {
                let target = Row::Task(id);
                if self.armed != Some(target) {
                    self.armed = Some(target);
                    let jira = t.jira_key().map(|k| format!(" ({k} in Jira isn't touched)")).unwrap_or_default();
                    self.note = Some((format!("D again deletes \"{}\"{jira}", fit(&t.title, 40)), true));
                    return Action::None;
                }
                self.armed = None;
                self.note = None;
                return Action::Delete(id);
            }
            Key::Char('J') if self.view == View::Tab(Tab::List) => return Action::Reorder(id, 1),
            Key::Char('K') if self.view == View::Tab(Tab::List) => return Action::Reorder(id, -1),
            _ => {}
        }
        Action::None
    }

    fn board_key(&mut self, key: Key, ctx: &Ctx) -> Option<Action> {
        let columns = self.columns(ctx);
        let rows: Vec<Row> = columns.iter().flat_map(|(c, _)| c.iter().map(|t| Row::Task(t.id))).collect();
        let at = self.resolve(&rows);
        let col = match at {
            Some(Row::Task(id)) => columns.iter().position(|(c, _)| c.iter().any(|t| t.id == id)).unwrap_or(self.col),
            _ => self.col,
        };
        self.col = col;
        let pos = match at {
            Some(Row::Task(id)) => columns[col].0.iter().position(|t| t.id == id).unwrap_or(0),
            _ => 0,
        };
        let go = |page: &mut Self, c: usize, i: usize| {
            page.col = c;
            if let Some(t) = columns[c].0.get(i.min(columns[c].0.len().saturating_sub(1))) {
                page.sel = Some(Row::Task(t.id));
            } else {
                page.sel = None;
            }
        };
        match key {
            Key::Char('h') | Key::Left => go(self, col.saturating_sub(1), pos),
            Key::Char('l') | Key::Right => go(self, (col + 1).min(3), pos),
            Key::Char('j') | Key::Down => go(self, col, pos + 1),
            Key::Char('k') | Key::Up => go(self, col, pos.saturating_sub(1)),
            Key::Char('H') | Key::Char('L') => {
                let Some(Row::Task(id)) = at else { return Some(Action::None) };
                let to = if key == Key::Char('L') { col + 1 } else { col.wrapping_sub(1) };
                let Some(&status) = Status::ALL.get(to) else { return Some(Action::None) };
                self.col = to;
                // A linked card's move goes through Jira's transitions.
                return Some(Action::SetStatus(id, status));
            }
            Key::Char('J') | Key::Char('K') => {
                let Some(Row::Task(id)) = at else { return Some(Action::None) };
                return Some(Action::Reorder(id, if key == Key::Char('J') { 1 } else { -1 }));
            }
            Key::Enter => {
                if let Some(Row::Task(id)) = at {
                    self.open_task(id);
                }
            }
            _ => return None,
        }
        Some(Action::None)
    }

    /// Keys only a task's own page has: acting on the row under the
    /// cursor, and `a`/`d` on its lists.
    fn task_page_key(&mut self, key: Key, row: Option<Row>, ctx: &Ctx) -> Option<Action> {
        let row = row?;
        let id = row.task()?;
        let t = ctx.task(id)?;
        let linked = t.jira.is_some();
        let action = match (key, row) {
            (Key::Enter, Row::Field(_, Field::Title)) => {
                self.type_line(Typing::Title(id), &t.title);
                Action::None
            }
            (Key::Enter, Row::Field(_, Field::Status)) => return Some(self.task_key(Key::Char('s'), t, Some(row), ctx)),
            (Key::Enter, Row::Field(_, Field::Priority)) => return Some(self.task_key(Key::Char('p'), t, Some(row), ctx)),
            (Key::Enter, Row::Field(_, Field::Category)) => {
                self.category_menu(t, ctx);
                Action::None
            }
            (Key::Enter, Row::Field(_, Field::Due)) => {
                self.due_menu(t, ctx);
                Action::None
            }
            (Key::Char('h') | Key::Char('l') | Key::Left | Key::Right, Row::Field(_, field)) => {
                let forward = matches!(key, Key::Char('l') | Key::Right);
                return Some(self.step_field(t, field, forward, ctx));
            }
            (Key::Enter, Row::Description(_)) => Action::EditDescription(id),
            (Key::Enter, Row::Code(_)) => Action::OpenCode(id),
            (Key::Enter | Key::Space, Row::Check(_, i)) => Action::ToggleCheck(id, i),
            (Key::Char('e'), Row::Check(_, i)) => {
                self.type_line(Typing::EditCheck(id, i), &t.subtasks[i].text);
                Action::None
            }
            (Key::Enter | Key::Char('a'), Row::AddCheck(_)) | (Key::Char('a'), Row::Check(..)) | (Key::Char('a'), Row::Field(..) | Row::Description(_) | Row::Code(_)) => {
                self.type_line(Typing::Check(id), "");
                Action::None
            }
            (Key::Enter, Row::DependsOn(_, dep)) | (Key::Enter, Row::Blocks(_, dep)) => {
                self.open_task(dep);
                Action::None
            }
            (Key::Enter | Key::Char('a'), Row::AddDependency(_)) | (Key::Char('a'), Row::DependsOn(..) | Row::Blocks(..)) => {
                self.dependency_menu(t, ctx);
                Action::None
            }
            (Key::Enter, Row::Conflict(_, field)) => {
                self.conflict_menu(t, field);
                Action::None
            }
            (Key::Enter | Key::Char('e'), Row::Note(_, i)) => {
                self.type_line(Typing::EditNote(id, i), &t.notes[i].text);
                Action::None
            }
            (Key::Enter | Key::Char('a'), Row::AddNote(_)) | (Key::Char('a'), Row::Note(..) | Row::Comment(..)) => {
                self.type_line(Typing::Note(id), "");
                Action::None
            }
            (Key::Enter | Key::Char('e'), Row::Time(_, i)) => {
                let e = &t.time_entries[i];
                self.type_line(Typing::EditTime(id, i), &format!("{}-{}", e.start.format("%H:%M"), e.end.format("%H:%M")));
                Action::None
            }
            (Key::Enter | Key::Char('a'), Row::AddTime(_)) | (Key::Char('a'), Row::Time(..) | Row::Conflict(..)) => {
                self.type_line(Typing::LogTime(id), "");
                Action::None
            }
            (Key::Char('d'), Row::Check(_, _) | Row::DependsOn(..) | Row::Note(..) | Row::Time(..)) => {
                if self.armed != Some(row) {
                    self.armed = Some(row);
                    self.note = Some(("d again deletes it".into(), true));
                    return Some(Action::None);
                }
                self.armed = None;
                self.note = None;
                match row {
                    Row::Check(_, i) => Action::RemoveCheck(id, i),
                    Row::DependsOn(_, dep) => Action::RemoveDependency(id, dep),
                    Row::Note(_, i) => Action::RemoveNote(id, i),
                    Row::Time(_, i) => Action::RemoveTime(id, i),
                    _ => Action::None,
                }
            }
            (Key::Char('d'), Row::Comment(..)) => {
                self.note = Some(("a Jira comment isn't yours to delete from here".into(), true));
                Action::None
            }
            (Key::Char('o'), _) if !linked && t.code.is_some() => Action::OpenCode(id),
            _ => return None,
        };
        Some(action)
    }

    /// `h`/`l` on a field: the value one step along.
    fn step_field(&mut self, t: &Task, field: Field, forward: bool, ctx: &Ctx) -> Action {
        let id = t.id;
        let linked = t.jira.is_some();
        let step = |i: usize, n: usize| if forward { (i + 1).min(n - 1) } else { i.saturating_sub(1) };
        match field {
            Field::Title => Action::None,
            Field::Status if linked => Action::JiraStatus(id),
            Field::Priority if linked => Action::JiraPriority(id),
            Field::Status => {
                let i = Status::ALL.iter().position(|s| *s == t.status).unwrap_or(0);
                let to = Status::ALL[step(i, 4)];
                if to == t.status { Action::None } else { Action::SetStatus(id, to) }
            }
            Field::Priority => {
                let i = Priority::ALL.iter().position(|p| *p == t.priority).unwrap_or(0);
                let to = Priority::ALL[step(i, 4)];
                if to == t.priority { Action::None } else { Action::SetPriority(id, to) }
            }
            Field::Category => {
                let mut choices: Vec<Option<String>> = vec![None];
                choices.extend(ctx.categories.iter().cloned().map(Some));
                let i = choices.iter().position(|c| *c == t.category).unwrap_or(0);
                let to = choices[step(i, choices.len())].clone();
                if to == t.category { Action::None } else { Action::SetCategory(id, to) }
            }
            Field::Due => {
                let base = t.due.unwrap_or(ctx.today());
                let to = if t.due.is_none() { base } else { base + Duration::days(if forward { 1 } else { -1 }) };
                Action::SetDue(id, Some(to))
            }
        }
    }

    // -- Menus -------------------------------------------------------------

    fn open_menu(&mut self, title: String, task: TaskId, items: Vec<(String, Pick)>, current: Option<usize>) {
        self.menu = Some(Menu { title, task, items, at: current.unwrap_or(0), field: None });
    }

    fn status_menu(&mut self, t: &Task) {
        let items = Status::ALL.iter().map(|s| (s.label().to_string(), Pick::Status(*s))).collect();
        let at = Status::ALL.iter().position(|s| *s == t.status);
        self.open_menu("Status".into(), t.id, items, at);
    }

    fn priority_menu(&mut self, t: &Task) {
        let items = Priority::ALL.iter().rev().map(|p| (p.label().to_string(), Pick::Priority(*p))).collect();
        let at = Priority::ALL.iter().rev().position(|p| *p == t.priority);
        self.open_menu("Priority".into(), t.id, items, at);
    }

    fn category_menu(&mut self, t: &Task, ctx: &Ctx) {
        let mut items = vec![("none".to_string(), Pick::Category(None))];
        items.extend(ctx.categories.iter().map(|c| (c.clone(), Pick::Category(Some(c.clone())))));
        items.push(("+ new category".to_string(), Pick::NewCategory));
        let at = items.iter().position(|(_, p)| *p == Pick::Category(t.category.clone()));
        self.open_menu("Category".into(), t.id, items, at);
    }

    fn due_menu(&mut self, t: &Task, ctx: &Ctx) {
        let today = ctx.today();
        let friday = next_weekday(today, Weekday::Fri);
        let monday = next_weekday(today + Duration::days(1), Weekday::Mon);
        let items = vec![
            ("none".to_string(), Pick::Due(None)),
            (format!("today · {}", today.format("%a %d %b")), Pick::Due(Some(today))),
            (format!("tomorrow · {}", (today + Duration::days(1)).format("%a %d %b")), Pick::Due(Some(today + Duration::days(1)))),
            (format!("Friday · {}", friday.format("%d %b")), Pick::Due(Some(friday))),
            (format!("next Monday · {}", monday.format("%d %b")), Pick::Due(Some(monday))),
            (format!("in a week · {}", (today + Duration::days(7)).format("%a %d %b")), Pick::Due(Some(today + Duration::days(7)))),
            ("type a date".to_string(), Pick::TypeDue),
        ];
        let at = items.iter().position(|(_, p)| *p == Pick::Due(t.due));
        self.open_menu("Due".into(), t.id, items, at);
    }

    fn dependency_menu(&mut self, t: &Task, ctx: &Ctx) {
        let items: Vec<(String, Pick)> =
            ctx.store.dependency_candidates(t.id).into_iter().map(|d| (keyed(d), Pick::Dependency(d.id))).collect();
        if items.is_empty() {
            self.note = Some(("no task it could wait on".into(), true));
            return;
        }
        self.open_menu("Waits on".into(), t.id, items, None);
    }

    fn conflict_menu(&mut self, t: &Task, field: SyncField) {
        let Some(conflict) = t.jira.as_ref().and_then(|l| l.conflicts.iter().find(|c| c.field == field)) else { return };
        let clip = |s: String| fit(s.lines().next().unwrap_or_default(), 50);
        let items = vec![
            (format!("Keep mine: {}", clip(t.sync_value(field))), Pick::Keep(true)),
            (format!("Take Jira's: {}", clip(conflict.their_value())), Pick::Keep(false)),
        ];
        self.open_menu(format!("Both changed the {}", field.label()), t.id, items, None);
        if let Some(m) = &mut self.menu {
            m.field = Some(field);
        }
    }

    fn menu_key(&mut self, key: Key) -> Action {
        let Some(menu) = &mut self.menu else { return Action::None };
        let n = menu.items.len();
        match key {
            Key::Escape | Key::Char('q') => self.menu = None,
            Key::Down | Key::Char('j') | Key::Tab => menu.at = (menu.at + 1) % n.max(1),
            Key::Up | Key::Char('k') | Key::BackTab => menu.at = (menu.at + n.max(1) - 1) % n.max(1),
            Key::Char(c @ '1'..='9') if (c as usize - '1' as usize) < n => {
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
        let Some((_, pick)) = menu.items.get(menu.at).cloned() else { return Action::None };
        let id = menu.task;
        match pick {
            Pick::Status(s) => Action::SetStatus(id, s),
            Pick::Priority(p) => Action::SetPriority(id, p),
            Pick::Category(c) => Action::SetCategory(id, c),
            Pick::NewCategory => {
                self.type_line(Typing::Category(id), "");
                Action::None
            }
            Pick::Due(d) => Action::SetDue(id, d),
            Pick::TypeDue => {
                self.type_line(Typing::Due(id), "");
                Action::None
            }
            Pick::Dependency(dep) => Action::AddDependency(id, dep),
            Pick::Keep(mine) => menu.field.map(|f| Action::Resolve(id, f, mine)).unwrap_or(Action::None),
        }
    }

    // -- Forms -------------------------------------------------------------

    fn type_line(&mut self, typing: Typing, text: &str) {
        self.form = Some(Form::Line { typing, text: text.to_string() });
    }

    /// `n`: the new-task form, filed under the project when the page is
    /// scoped to one, and in the Board column the cursor is in.
    fn new_task(&mut self, ctx: &Ctx) {
        let category = self.project.as_ref().filter(|_| self.only_project).map(|p| p.name.clone()).or_else(|| {
            let p = self.project.as_ref()?;
            ctx.categories.iter().find(|c| c.eq_ignore_ascii_case(&p.name)).cloned()
        });
        let status = match self.view {
            View::Tab(Tab::Board) => Status::ALL[self.col.min(3)],
            _ => Status::Todo,
        };
        self.form = Some(Form::New { title: String::new(), priority: Priority::Medium, category, due: String::new(), status, clock: false, at: 0 });
    }

    fn form_key(&mut self, key: Key, ctx: &Ctx) -> Action {
        match key {
            Key::Escape => {
                self.form = None;
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
            Some(Form::Line { typing, text }) if key == Key::Enter => self.finish_line(typing, text, ctx),
            Some(Form::New { .. }) => self.new_form_key(key, ctx),
            _ => Action::None,
        }
    }

    fn new_form_key(&mut self, key: Key, ctx: &Ctx) -> Action {
        let Some(Form::New { title, priority, category, due, status, clock, at }) = &mut self.form else { return Action::None };
        let n = NEW_FIELDS.len();
        match key {
            Key::Tab | Key::Down => *at = (*at + 1) % n,
            Key::BackTab | Key::Up => *at = (*at + n - 1) % n,
            Key::Char('h') | Key::Char('l') | Key::Left | Key::Right | Key::Space => {
                let forward = !matches!(key, Key::Char('h') | Key::Left);
                match *at {
                    1 => {
                        let i = Priority::ALL.iter().position(|p| p == priority).unwrap_or(1);
                        *priority = Priority::ALL[if forward { (i + 1).min(3) } else { i.saturating_sub(1) }];
                    }
                    2 => {
                        let mut choices: Vec<Option<String>> = vec![None];
                        choices.extend(ctx.categories.iter().cloned().map(Some));
                        let i = choices.iter().position(|c| c == category).unwrap_or(0);
                        let n = choices.len();
                        *category = choices[if forward { (i + 1) % n } else { (i + n - 1) % n }].clone();
                    }
                    4 => *clock = !*clock,
                    _ => {}
                }
            }
            Key::Enter => {
                let parsed = parse_title(title);
                if parsed.title.trim().is_empty() {
                    self.note = Some(("a task needs a title".into(), true));
                    *at = 0;
                    return Action::None;
                }
                let due_text = parsed.due.clone().unwrap_or_else(|| due.clone());
                let due = match parse_date(&due_text, ctx.today()) {
                    Ok(d) => d,
                    Err(why) => {
                        self.note = Some((why, true));
                        *at = 3;
                        return Action::None;
                    }
                };
                let spec = NewTask {
                    title: parsed.title.trim().to_string(),
                    priority: parsed.priority.unwrap_or(*priority),
                    category: parsed.category.or_else(|| category.clone()),
                    due,
                    status: *status,
                    clock: *clock,
                    link: parsed.link,
                };
                self.form = None;
                return Action::Create(spec);
            }
            _ => {}
        }
        Action::None
    }

    fn finish_line(&mut self, typing: Typing, text: String, ctx: &Ctx) -> Action {
        let value = text.trim().to_string();
        let refuse = |page: &mut Self, why: String| {
            page.note = Some((why, true));
            Action::None
        };
        let done = |page: &mut Self, action: Action| {
            page.form = None;
            page.note = None;
            action
        };
        match typing {
            Typing::Title(_) if value.is_empty() => refuse(self, "a task needs a title".into()),
            Typing::Title(id) => done(self, if ctx.task(id).is_some_and(|t| t.title == value) { Action::None } else { Action::SetTitle(id, value) }),
            Typing::Note(_) | Typing::EditNote(..) | Typing::Check(_) | Typing::EditCheck(..) | Typing::Category(_) if value.is_empty() => {
                done(self, Action::None)
            }
            Typing::Note(id) => done(self, Action::AddNote(id, value)),
            Typing::EditNote(id, i) => done(self, Action::EditNote(id, i, value)),
            Typing::Check(id) => {
                // Stays open for the next item.
                self.form = Some(Form::Line { typing, text: String::new() });
                Action::AddCheck(id, value)
            }
            Typing::EditCheck(id, i) => done(self, Action::EditCheck(id, i, value)),
            Typing::Category(id) => done(self, Action::AddCategory(id, value)),
            Typing::Due(id) => match parse_date(&value, ctx.today()) {
                Ok(d) => done(self, Action::SetDue(id, d)),
                Err(why) => refuse(self, why),
            },
            Typing::LogTime(id) => match parse_span(&value, ctx.today(), ctx.now) {
                Ok((start, end)) => done(self, Action::LogSpan(id, start, end)),
                Err(why) => refuse(self, why),
            },
            Typing::EditTime(id, i) => {
                let Some(day) = ctx.task(id).and_then(|t| t.time_entries.get(i)).map(|e| e.start.date_naive()) else { return done(self, Action::None) };
                match parse_clock_span(&value, day) {
                    Some((start, end)) => done(self, Action::EditTime(id, i, start, end)),
                    None => refuse(self, "write it as 9:15-10:40".into()),
                }
            }
            Typing::Worklog(id, date) => match parse_duration(&value) {
                Some(d) => {
                    self.form = None;
                    Action::WorklogMinutes(id, date, d.num_minutes())
                }
                None => refuse(self, "write it as 1h 30m".into()),
            },
        }
    }

    // -- The worklog review and the filters -------------------------------

    fn worklog_key(&mut self, key: Key, at: usize, ctx: &Ctx) -> Action {
        let rows = ctx.worklogs;
        let row = rows.get(at.min(rows.len().saturating_sub(1)));
        match key {
            Key::Escape | Key::Char('q') => self.worklogs = None,
            Key::Down | Key::Char('j') => self.worklogs = Some((at + 1).min(rows.len().saturating_sub(1))),
            Key::Up | Key::Char('k') => self.worklogs = Some(at.saturating_sub(1)),
            Key::Char('e') | Key::Enter => {
                if let Some(r) = row {
                    self.type_line(Typing::Worklog(r.task, r.date), &format_minutes(r.minutes));
                }
            }
            Key::Char('D') => {
                if let Some(r) = row {
                    return Action::WorklogDrop(r.task, r.date);
                }
            }
            Key::Char('x') => {
                if let Some(r) = row {
                    return Action::WorklogDismiss(r.task, r.date);
                }
            }
            Key::Char('W') => return Action::SendWorklogs,
            _ => {}
        }
        Action::None
    }

    /// The filter chips: each one's label and whether it's on.
    fn chips(&self, ctx: &Ctx) -> Vec<(String, bool)> {
        let mut chips = Vec::new();
        if let Some(p) = &self.project {
            chips.push((format!("only {} (P)", p.name), self.only_project));
        }
        chips.push(("High and Urgent".to_string(), self.filter.pressing));
        chips.push(("linked to Jira".to_string(), self.filter.linked == Some(true)));
        chips.push(("local only".to_string(), self.filter.linked == Some(false)));
        chips.push(("due within a week".to_string(), self.filter.due_soon));
        for c in ctx.categories {
            chips.push((format!("#{c}"), self.filter.categories.contains(c)));
        }
        chips
    }

    fn filter_key(&mut self, key: Key, at: usize, ctx: &Ctx) {
        let n = self.chips(ctx).len();
        match key {
            Key::Escape | Key::Char('f') | Key::Char('q') => self.filters = None,
            Key::Down | Key::Char('j') => self.filters = Some((at + 1).min(n - 1)),
            Key::Up | Key::Char('k') => self.filters = Some(at.saturating_sub(1)),
            Key::Char('F') => {
                self.filter = Filter::default();
                self.only_project = false;
            }
            Key::Space | Key::Enter => {
                let mut i = at;
                if self.project.is_some() {
                    if i == 0 {
                        self.only_project = !self.only_project;
                        return;
                    }
                    i -= 1;
                }
                match i {
                    0 => self.filter.pressing = !self.filter.pressing,
                    1 => self.filter.linked = if self.filter.linked == Some(true) { None } else { Some(true) },
                    2 => self.filter.linked = if self.filter.linked == Some(false) { None } else { Some(false) },
                    3 => self.filter.due_soon = !self.filter.due_soon,
                    _ => {
                        if let Some(c) = ctx.categories.get(i - 4) {
                            if !self.filter.categories.remove(c) {
                                self.filter.categories.insert(c.clone());
                            }
                        }
                    }
                }
                self.sel = None;
            }
            _ => {}
        }
    }

    /// The shown week as text: one line per task, then the day totals.
    fn week_text(&self, ctx: &Ctx) -> String {
        let monday = self.monday(ctx);
        let mut out = format!("Week of {}\n", monday.format("%Y-%m-%d"));
        let week = self.week_minutes(ctx);
        let mut days = [0i64; 7];
        for (id, mins) in &week {
            let title = ctx.task(*id).map(keyed).unwrap_or_default();
            let cells: Vec<String> = mins.iter().map(|m| if *m == 0 { "-".to_string() } else { format_minutes(*m) }).collect();
            out.push_str(&format!("{title}\t{}\t{}\n", cells.join("\t"), format_minutes(mins.iter().sum())));
            for (d, m) in days.iter_mut().zip(mins) {
                *d += m;
            }
        }
        let cells: Vec<String> = days.iter().map(|m| format_minutes(*m)).collect();
        out.push_str(&format!("Total\t{}\t{}\n", cells.join("\t"), format_minutes(days.iter().sum())));
        out
    }
}

/// Today's Next up shows this many; the rest are in List.
const NEXT_UP: usize = 5;

enum Activity {
    Note(usize),
    Comment(usize),
}

/// A task's notes and its Jira comments, oldest first.
fn activity(t: &Task) -> Vec<Activity> {
    let mut items: Vec<(Option<DateTime<Local>>, Activity)> = t.notes.iter().enumerate().map(|(i, n)| (Some(n.at), Activity::Note(i))).collect();
    if let Some(link) = &t.jira {
        items.extend(link.base.comments.iter().enumerate().map(|(i, c)| (parse_jira_time(&c.created), Activity::Comment(i))));
    }
    items.sort_by_key(|(at, _)| *at);
    items.into_iter().map(|(_, a)| a).collect()
}

fn parse_jira_time(raw: &str) -> Option<DateTime<Local>> {
    DateTime::parse_from_str(raw, "%Y-%m-%dT%H:%M:%S%.3f%z").ok().map(|t| t.with_timezone(&Local))
}

/// "KEY title", or the title.
fn keyed(t: &Task) -> String {
    match t.jira_key() {
        Some(key) => format!("{key} {}", t.title),
        None => t.title.clone(),
    }
}

/// The next `day` on or after `from`.
fn next_weekday(from: NaiveDate, day: Weekday) -> NaiveDate {
    let ahead = (7 + day.num_days_from_monday() as i64 - from.weekday().num_days_from_monday() as i64) % 7;
    from + Duration::days(ahead)
}

/// "1h 05m" / "45m".
pub fn format_minutes(minutes: i64) -> String {
    let minutes = minutes.max(0);
    if minutes >= 60 {
        format!("{}h {:02}m", minutes / 60, minutes % 60)
    } else {
        format!("{minutes}m")
    }
}

/// A due date as typed: `today`, `tomorrow`, a weekday (`fri`, the next
/// one, today included), `+3`/`3d`/`2w`, `2026-10-02` or `10-02`; empty
/// or `none` is no date.
pub fn parse_date(text: &str, today: NaiveDate) -> Result<Option<NaiveDate>, String> {
    let t = text.trim().to_lowercase();
    if t.is_empty() || t == "none" || t == "-" {
        return Ok(None);
    }
    let bad = || format!("\"{text}\" isn't a date -- try fri, +3, 2w or 2026-10-02");
    match t.as_str() {
        "today" | "tod" => return Ok(Some(today)),
        "tomorrow" | "tom" => return Ok(Some(today + Duration::days(1))),
        _ => {}
    }
    let days = [
        (Weekday::Mon, "monday"),
        (Weekday::Tue, "tuesday"),
        (Weekday::Wed, "wednesday"),
        (Weekday::Thu, "thursday"),
        (Weekday::Fri, "friday"),
        (Weekday::Sat, "saturday"),
        (Weekday::Sun, "sunday"),
    ];
    if t.len() >= 3 {
        if let Some((day, _)) = days.iter().find(|(_, name)| name.starts_with(&t)) {
            return Ok(Some(next_weekday(today, *day)));
        }
    }
    let rel = t.strip_prefix('+').unwrap_or(&t);
    if let Some(n) = rel.strip_suffix('w').and_then(|n| n.parse::<i64>().ok()) {
        return Ok(Some(today + Duration::weeks(n)));
    }
    if let Some(n) = rel.strip_suffix('d').unwrap_or(rel).parse::<i64>().ok().filter(|_| t.starts_with('+') || t.ends_with('d')) {
        return Ok(Some(today + Duration::days(n)));
    }
    if let Ok(d) = NaiveDate::parse_from_str(&t, "%Y-%m-%d") {
        return Ok(Some(d));
    }
    if let Some((m, d)) = t.split_once('-') {
        if let (Ok(m), Ok(d)) = (m.parse::<u32>(), d.parse::<u32>()) {
            let this_year = NaiveDate::from_ymd_opt(today.year(), m, d).ok_or_else(bad)?;
            let date = if this_year < today { NaiveDate::from_ymd_opt(today.year() + 1, m, d).ok_or_else(bad)? } else { this_year };
            return Ok(Some(date));
        }
    }
    Err(bad())
}

/// `1h 30m`, `45m`, `2h`, `90`.
pub fn parse_duration(text: &str) -> Option<Duration> {
    let mut total = 0i64;
    let mut number = String::new();
    let mut any = false;
    for c in text.trim().chars() {
        match c {
            '0'..='9' => number.push(c),
            ' ' => {}
            'h' | 'H' => {
                total += number.parse::<i64>().ok()? * 60;
                number.clear();
                any = true;
            }
            'm' | 'M' => {
                total += number.parse::<i64>().ok()?;
                number.clear();
                any = true;
            }
            _ => return None,
        }
    }
    if !number.is_empty() {
        total += number.parse::<i64>().ok()?;
        any = true;
    }
    (any && total > 0).then(|| Duration::minutes(total))
}

fn parse_clock(text: &str) -> Option<NaiveTime> {
    let t = text.trim();
    NaiveTime::parse_from_str(t, "%H:%M").ok().or_else(|| t.parse::<u32>().ok().and_then(|h| NaiveTime::from_hms_opt(h, 0, 0)))
}

/// `9:15-10:40` on `day`.
fn parse_clock_span(text: &str, day: NaiveDate) -> Option<(DateTime<Local>, DateTime<Local>)> {
    let (a, b) = text.split_once('-')?;
    let at = |t: NaiveTime| Local.from_local_datetime(&day.and_time(t)).earliest();
    let (start, end) = (at(parse_clock(a)?)?, at(parse_clock(b)?)?);
    (end > start).then_some((start, end))
}

/// Time to log: a span today (`9:15-10:40`), or a length ending now.
pub fn parse_span(text: &str, today: NaiveDate, now: DateTime<Local>) -> Result<(DateTime<Local>, DateTime<Local>), String> {
    if text.contains('-') || text.contains(':') {
        return parse_clock_span(text, today).ok_or_else(|| "write a span as 9:15-10:40".to_string());
    }
    let d = parse_duration(text).ok_or_else(|| "write it as 1h 30m, or 9:15-10:40".to_string())?;
    Ok((now - d, now))
}

/// What a new task's title line says besides the title: `!high`,
/// `#category`, `due:fri`, and a Jira key to link.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Parsed {
    pub title: String,
    pub priority: Option<Priority>,
    pub category: Option<String>,
    pub due: Option<String>,
    pub link: Option<String>,
}

pub fn parse_title(text: &str) -> Parsed {
    let mut p = Parsed::default();
    let mut words = Vec::new();
    for word in text.split_whitespace() {
        let lower = word.to_lowercase();
        let priority = match lower.as_str() {
            "!!" | "!urgent" => Some(Priority::Urgent),
            "!" | "!high" => Some(Priority::High),
            "!med" | "!medium" => Some(Priority::Medium),
            "!low" => Some(Priority::Low),
            _ => None,
        };
        if let Some(pr) = priority {
            p.priority = Some(pr);
        } else if let Some(c) = word.strip_prefix('#').filter(|c| !c.is_empty()) {
            p.category = Some(c.to_string());
        } else if let Some(d) = lower.strip_prefix("due:").filter(|d| !d.is_empty()) {
            p.due = Some(d.to_string());
        } else if p.link.is_none() && is_issue_key(word) {
            p.link = Some(word.to_string());
        } else {
            words.push(word);
        }
    }
    p.title = words.join(" ");
    p
}

/// `PROJ-12`: capitals, a dash, digits.
fn is_issue_key(word: &str) -> bool {
    let Some((project, number)) = word.rsplit_once('-') else { return false };
    !project.is_empty()
        && project.chars().next().is_some_and(|c| c.is_ascii_uppercase())
        && project.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        && !number.is_empty()
        && number.chars().all(|c| c.is_ascii_digit())
}

// -- Layout ----------------------------------------------------------------

fn priority_mark(p: Priority) -> (&'static str, Role) {
    match p {
        Priority::Urgent => ("!!", Role::Bad),
        Priority::High => ("! ", Role::Warn),
        Priority::Medium => ("· ", Role::Muted),
        Priority::Low => ("  ", Role::Muted),
    }
}

/// "today", "Fri", "3 Oct", "2d late" -- and how it reads.
fn due_label(due: NaiveDate, today: NaiveDate, done: bool) -> (String, Role) {
    let days = (due - today).num_days();
    let text = match days {
        d if d < -1 => format!("{}d late", -d),
        -1 => "yesterday".to_string(),
        0 => "today".to_string(),
        1 => "tomorrow".to_string(),
        2..=6 => due.format("%a").to_string(),
        _ => due.format("%-d %b").to_string(),
    };
    let role = match days {
        _ if done => Role::Muted,
        d if d < 0 => Role::Bad,
        0 => Role::Warn,
        _ => Role::Muted,
    };
    (text, role)
}

/// The sync mark on a linked task: what needs you first.
fn sync_mark(ctx: &Ctx, t: &Task) -> Option<(String, Role)> {
    let link = t.jira.as_ref()?;
    if !link.conflicts.is_empty() {
        return Some((format!("⚠ {} changed in both", link.conflicts[0].field.label()), Role::Bad));
    }
    if let Some(err) = &link.error {
        return Some((format!("⚠ not synced: {}", fit(err, 40)), Role::Bad));
    }
    if link.resolving || ctx.store.pending_for(t.id).next().is_some() {
        return Some(("↑".to_string(), Role::Warn));
    }
    if link.not_mine {
        return Some(("reassigned".to_string(), Role::Muted));
    }
    None
}

/// Jira's status name, when it says more than the column.
fn jira_status(t: &Task) -> Option<&str> {
    let name = t.jira.as_ref()?.base.status_name.as_str();
    (!name.is_empty() && !name.eq_ignore_ascii_case(t.status.label())).then_some(name)
}

/// One task as a row: `!! KEY Title (category)` then, from the right,
/// what else there is to say about it.
fn task_line(g: &mut Grid, y: usize, x: usize, width: usize, t: &Task, ctx: &Ctx, show_status: bool) {
    let (mark, role) = priority_mark(t.priority);
    let mut right: Vec<(String, Role)> = Vec::new();
    if let Some(timer) = ctx.store.active_timer.as_ref().filter(|a| a.task_id == t.id) {
        right.push((format!("● {}", format_minutes((ctx.now - timer.started_at).num_minutes())), Role::Good));
    }
    if let Some((text, role)) = sync_mark(ctx, t) {
        right.push((text, role));
    }
    if let Some(name) = jira_status(t) {
        right.push((name.to_string(), Role::Muted));
    } else if show_status {
        right.push((t.status.label().to_string(), Role::Muted));
    }
    if let Some(due) = t.due {
        right.push(due_label(due, ctx.today(), t.status == Status::Done));
    }
    let time = ctx.store.elapsed_on(t.id).num_minutes();
    if time > 0 {
        right.push((format_minutes(time), Role::Muted));
    }
    if t.archived {
        right.push(("archived".to_string(), Role::Muted));
    }
    let right_w: usize = right.iter().map(|(s, _)| s.chars().count() + 3).sum();
    let mut cx = g.put(y, x, mark, role) + 1;
    if let Some(key) = t.jira_key() {
        cx = g.put(y, cx, key, Role::Accent) + 1;
    }
    let cat = t.category.as_ref().map(|c| format!("  #{c}")).unwrap_or_default();
    let room = (x + width).saturating_sub(cx + right_w + 1);
    let title = fit(&t.title, room.saturating_sub(cat.chars().count()).max(8));
    cx = g.put(y, cx, &title, if t.status == Status::Done { Role::Muted } else { Role::Text });
    if !cat.is_empty() {
        g.put(y, cx, &fit(&cat, (x + width).saturating_sub(cx + right_w + 1)), Role::Muted);
    }
    let mut rx = x + width;
    for (text, role) in right.iter().rev() {
        rx = rx.saturating_sub(text.chars().count());
        g.put(y, rx, text, *role);
        rx = rx.saturating_sub(3);
    }
}

pub fn layout(page: &AgendaPage, ctx: &Ctx, cols: usize) -> Page {
    let (left, width) = frame(cols, 140);
    let mut g = Grid::new();

    // The header: the title, the running clock, how sync stands.
    let crumb = match page.view {
        View::Task(_) => "Agenda ›",
        View::Tab(_) => "Agenda",
    };
    let x = g.put(1, left, crumb, if matches!(page.view, View::Task(_)) { Role::Muted } else { Role::Title }) + 2;
    if let Some(scope) = page.scope_label() {
        g.put(1, x, &scope, Role::Accent);
    }
    let mut right = ctx.sync.clone();
    if let Some(timer) = &ctx.store.active_timer {
        let name = ctx.task(timer.task_id).map(keyed).unwrap_or_default();
        let clock = format!("● {} · {}", fit(&name, 40), format_minutes((ctx.now - timer.started_at).num_minutes()));
        right = if right.is_empty() { clock } else { format!("{clock}   {right}") };
    }
    let rx = (left + width).saturating_sub(right.chars().count());
    if ctx.store.active_timer.is_some() {
        g.put(1, rx, &right, Role::Good);
    } else {
        g.put(1, rx, &right, Role::Muted);
    }

    // The search line.
    let shown = ctx.store.tasks.iter().filter(|t| page.shown(t, ctx)).count();
    let search = match (&page.search, page.searching) {
        (Some(s), true) => format!("/ {s}▏"),
        (Some(s), false) if !s.is_empty() => format!("/ {s}  ·  {shown} match  (Esc clears)"),
        _ => format!("/ search {} tasks", ctx.store.tasks.iter().filter(|t| !t.archived).count()),
    };
    let end = g.put(2, left, &fit(&search, width), if page.searching { Role::Title } else { Role::Muted });
    if page.searching {
        g.panels.push((2, left..end.max(left + 30).min(left + width)));
    }
    let mut y = 3;
    if let Some((text, bad)) = &page.note {
        g.put(y, left, &fit(text, width), if *bad { Role::Bad } else { Role::Good });
        y += 1;
    } else if !page.filter.is_empty() {
        g.put(y, left, "filtered -- f to change, F to clear", Role::Warn);
        y += 1;
    }

    // The tabs.
    y += 1;
    let mut tx = left;
    for tab in Tab::ALL {
        let on = page.view == View::Tab(tab);
        let start = tx;
        tx = g.put(y, tx, &format!("{}", tab.index() + 1), Role::Accent) + 1;
        tx = g.put(y, tx, tab.label(), if on { Role::Title } else { Role::Muted });
        if on {
            g.panels.push((y, start.saturating_sub(1)..tx + 1));
        }
        tx += 4;
    }
    if page.view == View::Tab(Tab::Time) {
        let monday = page.monday(ctx);
        let label = if page.week == 0 { format!("[ this week · {} ]", monday.format("%-d %b")) } else { format!("[ week of {} ]", monday.format("%-d %b %Y")) };
        g.put(y, (left + width).saturating_sub(label.chars().count()), &label, Role::Muted);
    }
    y += 1;
    g.rule(y, left..left + width);
    y += 1;

    let rows = page.rows(ctx);
    let sel = page.current(&rows);
    let mut anchor = y;
    match page.view {
        View::Tab(Tab::Today) => layout_today(&mut g, page, ctx, sel, left, width, &mut y, &mut anchor),
        View::Tab(Tab::List) => layout_list(&mut g, page, ctx, sel, left, width, &mut y, &mut anchor),
        View::Tab(Tab::Board) => layout_board(&mut g, page, ctx, sel, left, width, &mut y, &mut anchor),
        View::Tab(Tab::Time) => layout_time(&mut g, page, ctx, sel, left, width, &mut y, &mut anchor),
        View::Task(id) => layout_task(&mut g, ctx, id, sel, left, width, &mut y, &mut anchor),
    }

    popups(&mut g, page, ctx, anchor, left, width);
    let keys = key_strip(page, sel, ctx);
    g.keys(left, width, &keys);
    g.finish()
}

#[allow(clippy::too_many_arguments)]
fn layout_today(g: &mut Grid, page: &AgendaPage, ctx: &Ctx, sel: Option<Row>, left: usize, width: usize, y: &mut usize, anchor: &mut usize) {
    let (sections, done_today) = page.today(ctx);
    let side = if width >= 110 { 30 } else { 0 };
    let main_w = width - if side > 0 { side + 4 } else { 0 };
    let top = *y;
    let mut any = false;
    for (title, rows) in &sections {
        if rows.is_empty() {
            continue;
        }
        any = true;
        let count = rows.iter().filter(|r| matches!(r, Row::Task(_))).count();
        g.heading(*y, left, main_w, &format!("{title} · {count}"));
        *y += 1;
        for row in rows {
            let is_sel = sel == Some(*row);
            match row {
                Row::Task(id) => {
                    let Some(t) = ctx.task(*id) else { continue };
                    task_line(g, *y, left + 1, main_w - 1, t, ctx, false);
                }
                Row::More => {
                    g.put(*y, left + 3, "+ the rest are in List", Role::Accent);
                }
                Row::Unsent => {
                    let minutes: i64 = ctx.worklogs.iter().map(|r| r.minutes).sum();
                    let issues: BTreeSet<&str> = ctx.worklogs.iter().map(|r| r.key.as_str()).collect();
                    let plural = if issues.len() == 1 { "issue" } else { "issues" };
                    g.put(*y, left + 1, &format!("⚠ {} unsent to Jira across {} {plural}", format_minutes(minutes), issues.len()), Role::Warn);
                    g.put(*y, (left + main_w).saturating_sub(11), "W to review", Role::Muted);
                }
                _ => {}
            }
            if is_sel {
                g.focus(*y, left..left + main_w);
                *anchor = *y;
            }
            *y += 1;
        }
        *y += 1;
    }
    if !any {
        let empty = if ctx.store.tasks.is_empty() { "Nothing yet -- n adds a task, SPC a i brings in your Jira issues." } else { "Nothing needs you right now." };
        g.put(*y, left + 1, empty, Role::Muted);
        *y += 2;
    }
    if done_today > 0 {
        g.put(*y, left + 1, &format!("✓ {done_today} done today"), Role::Good);
        *y += 1;
    }

    // This week, by day, beside it.
    if side > 0 {
        let sx = left + width - side;
        let mut sy = top;
        g.heading(sy, sx, side, "This week");
        sy += 1;
        let monday = page.monday(ctx);
        let week = page.week_minutes(ctx);
        let mut days = [0i64; 7];
        for (_, mins) in &week {
            for (d, m) in days.iter_mut().zip(mins) {
                *d += m;
            }
        }
        let max = days.iter().copied().max().unwrap_or(0).max(8 * 60);
        for (i, mins) in days.iter().enumerate() {
            let day = monday + Duration::days(i as i64);
            if i >= 5 && *mins == 0 {
                continue;
            }
            let is_today = day == ctx.today();
            g.put(sy, sx, &day.format("%a").to_string(), if is_today { Role::Title } else { Role::Muted });
            let bar_w = side - 16;
            let filled = ((*mins as f64 / max as f64) * bar_w as f64).round() as usize;
            g.put(sy, sx + 5, &"█".repeat(filled.min(bar_w)), Role::Accent);
            g.put(sy, sx + 5 + filled.min(bar_w), &"·".repeat(bar_w - filled.min(bar_w)), Role::Muted);
            let label = if *mins == 0 { "-".to_string() } else { format_minutes(*mins) };
            g.put(sy, sx + side - label.chars().count(), &label, if is_today { Role::Title } else { Role::Text });
            sy += 1;
        }
        let total = format_minutes(days.iter().sum());
        g.put(sy + 1, sx, "week", Role::Muted);
        g.put(sy + 1, sx + side - total.chars().count(), &total, Role::Text);
        *y = (*y).max(sy + 2);
    }
}

#[allow(clippy::too_many_arguments)]
fn layout_list(g: &mut Grid, page: &AgendaPage, ctx: &Ctx, sel: Option<Row>, left: usize, width: usize, y: &mut usize, anchor: &mut usize) {
    let sections = page.list(ctx);
    if sections.iter().all(|(_, r)| r.is_empty()) {
        g.put(*y, left + 1, if ctx.store.tasks.is_empty() { "No tasks yet -- n adds one." } else { "Nothing matches." }, Role::Muted);
        *y += 2;
        return;
    }
    for (title, rows) in &sections {
        if rows.is_empty() {
            continue;
        }
        g.heading(*y, left, width, &format!("{title} · {}", rows.len()));
        *y += 1;
        for row in rows {
            let Row::Task(id) = row else { continue };
            let Some(t) = ctx.task(*id) else { continue };
            task_line(g, *y, left + 1, width - 1, t, ctx, false);
            if sel == Some(*row) {
                g.focus(*y, left..left + width);
                *anchor = *y;
            }
            *y += 1;
        }
        *y += 1;
    }
}

#[allow(clippy::too_many_arguments)]
fn layout_board(g: &mut Grid, page: &AgendaPage, ctx: &Ctx, sel: Option<Row>, left: usize, width: usize, y: &mut usize, anchor: &mut usize) {
    let columns = page.columns(ctx);
    let gap = 3;
    let col_w = (width.saturating_sub(gap * 3)) / 4;
    let top = *y;
    let mut bottom = top;
    for (c, (cards, older)) in columns.iter().enumerate() {
        let x = left + c * (col_w + gap);
        let status = Status::ALL[c];
        let count = if status == Status::Done && *older > 0 { format!("{} this week", cards.len()) } else { cards.len().to_string() };
        g.heading(top, x, col_w, &format!("{} · {count}", status.label()));
        let mut cy = top + 1;
        if cards.is_empty() && page.col == c && sel.is_none() {
            g.focus(cy, x..x + col_w);
            *anchor = cy;
        }
        for t in cards {
            let is_sel = sel == Some(Row::Task(t.id));
            let (mark, role) = priority_mark(t.priority);
            let lines = wrap(&t.title, col_w.saturating_sub(3).max(8));
            let first = cy;
            for (i, line) in lines.iter().take(2).enumerate() {
                let text = if i == 1 && lines.len() > 2 { fit(&format!("{line} {}", lines[2]), col_w - 3) } else { line.clone() };
                if i == 0 {
                    g.put(cy, x, mark, role);
                }
                g.put(cy, x + 2, &text, if t.status == Status::Done { Role::Muted } else { Role::Text });
                cy += 1;
            }
            let mut mx = x + 2;
            if let Some(key) = t.jira_key() {
                mx = g.put(cy, mx, key, Role::Accent) + 1;
            }
            let mut meta: Vec<(String, Role)> = Vec::new();
            if let Some(name) = jira_status(t) {
                meta.push((format!("· {name}"), Role::Muted));
            } else if let Some(c) = &t.category {
                meta.push((format!("#{c}"), Role::Muted));
            }
            if let Some(due) = t.due {
                meta.push(due_label(due, ctx.today(), t.status == Status::Done));
            }
            if !ctx.store.is_ready(t.id) && t.status != Status::Done {
                meta.push(("waiting".into(), Role::Muted));
            }
            if ctx.store.active_timer.as_ref().is_some_and(|a| a.task_id == t.id) {
                meta.push(("●".into(), Role::Good));
            }
            if let Some((text, role)) = sync_mark(ctx, t) {
                meta.push((text.split(' ').next().unwrap_or_default().to_string(), role));
            }
            for (text, role) in meta {
                if mx + text.chars().count() > x + col_w {
                    break;
                }
                mx = g.put(cy, mx, &text, role) + 1;
            }
            if is_sel {
                g.focus(first, x..x + col_w);
                for l in first + 1..=cy {
                    g.panels.push((l, x..x + col_w));
                }
                *anchor = first;
            }
            cy += 2;
        }
        if *older > 0 {
            g.put(cy, x + 2, &format!("+ {older} older"), Role::Muted);
            cy += 1;
        }
        bottom = bottom.max(cy);
    }
    *y = bottom;
}

#[allow(clippy::too_many_arguments)]
fn layout_time(g: &mut Grid, page: &AgendaPage, ctx: &Ctx, sel: Option<Row>, left: usize, width: usize, y: &mut usize, anchor: &mut usize) {
    let week = page.week_minutes(ctx);
    let monday = page.monday(ctx);
    let mut days = [0i64; 7];
    for (_, mins) in &week {
        for (d, m) in days.iter_mut().zip(mins) {
            *d += m;
        }
    }
    // Weekends only when something was done on them.
    let shown_days: Vec<usize> = (0..7).filter(|&i| i < 5 || days[i] > 0).collect();
    let cell = 9;
    let unsent_w = 10;
    let name_w = width.saturating_sub(cell * (shown_days.len() + 1) + unsent_w).max(16);
    let put_right = |g: &mut Grid, y: usize, end: usize, text: &str, role: Role| {
        g.put(y, end.saturating_sub(text.chars().count()), text, role);
    };
    let mut x = left + name_w;
    g.put(*y, left, "Task", Role::Muted);
    for &i in &shown_days {
        let day = monday + Duration::days(i as i64);
        x += cell;
        put_right(g, *y, x, &day.format("%a %d").to_string(), if day == ctx.today() { Role::Title } else { Role::Muted });
    }
    x += cell;
    put_right(g, *y, x, "Total", Role::Muted);
    put_right(g, *y, x + unsent_w, "Unsent", Role::Muted);
    *y += 1;
    g.rule(*y, left..left + width);
    *y += 1;
    if week.is_empty() {
        g.put(*y, left + 1, "No time this week -- t on a task starts its clock, T logs time.", Role::Muted);
        *y += 2;
    }
    for (id, mins) in &week {
        let Some(t) = ctx.task(*id) else { continue };
        let mut cx = left;
        if let Some(key) = t.jira_key() {
            cx = g.put(*y, cx, key, Role::Accent) + 1;
        }
        g.put(*y, cx, &fit(&t.title, (left + name_w).saturating_sub(cx + 2)), Role::Text);
        let mut x = left + name_w;
        for &i in &shown_days {
            x += cell;
            let text = if mins[i] == 0 { "-".to_string() } else { format_minutes(mins[i]) };
            put_right(g, *y, x, &text, if mins[i] == 0 { Role::Muted } else { Role::Text });
        }
        x += cell;
        put_right(g, *y, x, &format_minutes(mins.iter().sum()), Role::Title);
        let unsent: i64 = ctx.worklogs.iter().filter(|r| r.task == *id).map(|r| r.minutes).sum();
        let (text, role) = match (t.jira.is_some(), unsent) {
            (false, _) => ("local".to_string(), Role::Muted),
            (true, 0) => ("sent".to_string(), Role::Muted),
            (true, m) => (format_minutes(m), Role::Warn),
        };
        put_right(g, *y, x + unsent_w, &text, role);
        if sel == Some(Row::Task(*id)) {
            g.focus(*y, left..left + width);
            *anchor = *y;
        }
        *y += 1;
    }
    g.rule(*y, left..left + width);
    *y += 1;
    g.put(*y, left, "Day", Role::Muted);
    let mut x = left + name_w;
    for &i in &shown_days {
        x += cell;
        put_right(g, *y, x, &if days[i] == 0 { "-".to_string() } else { format_minutes(days[i]) }, if days[i] == 0 { Role::Muted } else { Role::Text });
    }
    x += cell;
    put_right(g, *y, x, &format_minutes(days.iter().sum()), Role::Title);
    let unsent: i64 = ctx.worklogs.iter().map(|r| r.minutes).sum();
    if unsent > 0 {
        put_right(g, *y, x + unsent_w, &format_minutes(unsent), Role::Warn);
    }
    *y += 2;
}

#[allow(clippy::too_many_arguments)]
fn layout_task(g: &mut Grid, ctx: &Ctx, id: TaskId, sel: Option<Row>, left: usize, width: usize, y: &mut usize, anchor: &mut usize) {
    let Some(t) = ctx.task(id) else {
        g.put(*y, left, "This task is gone.", Role::Muted);
        return;
    };
    let label_w = 12;
    let vx = left + label_w;
    let mut focus = |g: &mut Grid, row: Row, y: usize, from: usize| {
        if sel == Some(row) {
            g.focus(y, from..left + width);
            *anchor = y;
        }
    };

    // The heading: key and title, and where it stands with Jira.
    let mut hx = left;
    if let Some(key) = t.jira_key() {
        hx = g.put(*y, hx, key, Role::Accent) + 2;
    }
    g.put(*y, hx, &fit(&t.title, width.saturating_sub(hx - left + 24)), Role::Title);
    let state = match (&t.jira, sync_mark(ctx, t)) {
        (Some(_), Some((text, role))) => Some((text, role)),
        (Some(link), None) => Some((if link.last_synced.is_some() { "✓ synced".to_string() } else { "linked".to_string() }, Role::Good)),
        (None, _) => Some(("local task".to_string(), Role::Muted)),
    };
    if let Some((text, role)) = state {
        g.put(*y, (left + width).saturating_sub(text.chars().count()), &text, role);
    }
    *y += 2;

    // The fields.
    for field in Field::ALL {
        let row = Row::Field(id, field);
        g.put(*y, left + 1, field.label(), Role::Muted);
        let (value, role, hint): (String, Role, String) = match field {
            Field::Title => (t.title.clone(), Role::Text, String::new()),
            Field::Status => (t.status.label().to_string(), Role::Text, jira_status(t).map(|s| format!("Jira: {s}")).unwrap_or_default()),
            Field::Priority => {
                let jira = t.jira.as_ref().and_then(|l| l.base.priority.clone()).map(|p| format!("Jira: {p}")).unwrap_or_default();
                (t.priority.label().to_string(), priority_mark(t.priority).1, jira)
            }
            Field::Category => match &t.category {
                Some(c) => (c.clone(), Role::Text, String::new()),
                None => ("none".to_string(), Role::Muted, String::new()),
            },
            Field::Due => match t.due {
                Some(d) => {
                    let (label, role) = due_label(d, ctx.today(), t.status == Status::Done);
                    (format!("{} · {label}", d.format("%a %d %b %Y")), role, String::new())
                }
                None => ("none".to_string(), Role::Muted, "u sets it".to_string()),
            },
        };
        let cyc = sel == Some(row) && field != Field::Title;
        let shown = if cyc { format!("‹ {value} ›") } else { value };
        g.put(*y, vx, &fit(&shown, width.saturating_sub(label_w + hint.chars().count() + 3)), role);
        if !hint.is_empty() {
            g.put(*y, (left + width).saturating_sub(hint.chars().count()), &hint, Role::Muted);
        }
        focus(g, row, *y, left);
        *y += 1;
    }
    if let Some(link) = &t.jira {
        g.put(*y, left + 1, "Assignee", Role::Muted);
        g.put(*y, vx, link.base.assignee.as_deref().unwrap_or("nobody"), if link.not_mine { Role::Warn } else { Role::Text });
        *y += 1;
    }
    if let Some(code) = &t.code {
        g.put(*y, left + 1, "Code", Role::Muted);
        let name = code.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        g.put(*y, vx, &format!("{name}:{}", code.line), Role::Accent);
        g.put(*y, (left + width).saturating_sub(10), "Enter goes", Role::Muted);
        focus(g, Row::Code(id), *y, left);
        *y += 1;
    }
    let time = ctx.store.elapsed_on(id).num_minutes();
    g.put(*y, left + 1, "Time", Role::Muted);
    let unsent: i64 = ctx.worklogs.iter().filter(|r| r.task == id).map(|r| r.minutes).sum();
    let mut time_text = format_minutes(time);
    if unsent > 0 {
        time_text.push_str(&format!(" · {} unsent", format_minutes(unsent)));
    }
    g.put(*y, vx, &time_text, Role::Text);
    *y += 1;

    if let Some(link) = &t.jira {
        if !link.conflicts.is_empty() {
            *y += 1;
            g.heading(*y, left, width, "Changed here and in Jira -- Enter to choose");
            *y += 1;
            for c in &link.conflicts {
                g.put(*y, left + 1, &format!("⚠ {}: Jira has \"{}\", you have \"{}\"", c.field.label(), fit(&c.their_value(), 30), fit(&t.sync_value(c.field), 30)), Role::Bad);
                focus(g, Row::Conflict(id, c.field), *y, left);
                *y += 1;
            }
        }
        if let Some(err) = &link.error {
            g.put(*y, left + 1, &fit(&format!("⚠ Jira refused the last change: {err} -- r retries"), width - 1), Role::Bad);
            *y += 1;
        }
    }

    // Description.
    *y += 1;
    heading_with_hint(g, *y, left, width, "Description", "E edits");
    *y += 1;
    let first = *y;
    if t.description.trim().is_empty() {
        g.put(*y, left + 1, "none", Role::Muted);
        *y += 1;
    } else {
        let lines: Vec<String> = t.description.lines().flat_map(|l| if l.trim().is_empty() { vec![String::new()] } else { wrap(l, width - 2) }).collect();
        for line in lines.iter().take(12) {
            g.put(*y, left + 1, line, Role::Text);
            *y += 1;
        }
        if lines.len() > 12 {
            g.put(*y, left + 1, &format!("… {} more lines -- E shows it all", lines.len() - 12), Role::Muted);
            *y += 1;
        }
    }
    focus(g, Row::Description(id), first, left);

    // Checklist.
    *y += 1;
    let done = t.subtasks.iter().filter(|s| s.done).count();
    g.heading(*y, left, width, &if t.subtasks.is_empty() { "Checklist".to_string() } else { format!("Checklist · {done} / {}", t.subtasks.len()) });
    *y += 1;
    for (i, s) in t.subtasks.iter().enumerate() {
        g.put(*y, left + 1, if s.done { "[x]" } else { "[ ]" }, if s.done { Role::Good } else { Role::Muted });
        g.put(*y, left + 5, &fit(&s.text, width - 6), if s.done { Role::Muted } else { Role::Text });
        focus(g, Row::Check(id, i), *y, left);
        *y += 1;
    }
    g.put(*y, left + 1, "+ add an item", Role::Accent);
    focus(g, Row::AddCheck(id), *y, left);
    *y += 1;

    // Dependencies.
    *y += 1;
    g.heading(*y, left, width, "Waits on");
    *y += 1;
    for dep in &t.depends_on {
        if let Some(d) = ctx.task(*dep) {
            let role = if d.status == Status::Done { Role::Good } else { Role::Muted };
            let x = g.put(*y, left + 1, &format!("{:<12}", d.status.label()), role);
            g.put(*y, x, &fit(&keyed(d), width - 14), Role::Text);
        }
        focus(g, Row::DependsOn(id, *dep), *y, left);
        *y += 1;
    }
    g.put(*y, left + 1, "+ add a task it waits on", Role::Accent);
    focus(g, Row::AddDependency(id), *y, left);
    *y += 1;
    let blocks = ctx.store.blocks(id);
    if !blocks.is_empty() {
        g.put(*y, left + 1, "Blocks", Role::Muted);
        *y += 1;
        for b in blocks {
            g.put(*y, left + 1, &format!("{:<12}", b.status.label()), Role::Muted);
            g.put(*y, left + 13, &fit(&keyed(b), width - 14), Role::Text);
            focus(g, Row::Blocks(id, b.id), *y, left);
            *y += 1;
        }
    }

    // Activity: your notes and Jira's comments.
    *y += 1;
    heading_with_hint(g, *y, left, width, "Activity", if t.jira.is_some() { "N note · C comment" } else { "N note" });
    *y += 1;
    for a in activity(t) {
        let (row, who, when, body) = match a {
            Activity::Note(i) => {
                let n = &t.notes[i];
                (Row::Note(id, i), "note".to_string(), ago(n.at, ctx.now), n.text.clone())
            }
            Activity::Comment(i) => {
                let c = &t.jira.as_ref().unwrap().base.comments[i];
                let when = parse_jira_time(&c.created).map(|at| ago(at, ctx.now)).unwrap_or_default();
                (Row::Comment(id, i), format!("{} in Jira", c.author), when, c.body.clone())
            }
        };
        let x = g.put(*y, left + 1, &who, if matches!(a, Activity::Comment(_)) { Role::Accent } else { Role::Muted });
        g.put(*y, x + 2, &when, Role::Muted);
        focus(g, row, *y, left);
        *y += 1;
        for line in body.lines().flat_map(|l| wrap(l, width - 4)).take(6) {
            g.put(*y, left + 3, &line, Role::Text);
            *y += 1;
        }
    }
    g.put(*y, left + 1, "+ add a note", Role::Accent);
    focus(g, Row::AddNote(id), *y, left);
    *y += 1;

    // Time.
    *y += 1;
    g.heading(*y, left, width, &format!("Time · {}", format_minutes(time)));
    *y += 1;
    for (i, e) in t.time_entries.iter().enumerate() {
        let when = format!("{}  {}–{}", e.start.format("%a %d %b"), e.start.format("%H:%M"), e.end.format("%H:%M"));
        g.put(*y, left + 1, &when, Role::Text);
        let tail = match (t.jira.is_some(), e.sent) {
            (true, true) => format!("{} · sent", format_minutes(e.duration().num_minutes())),
            (true, false) => format!("{} · unsent", format_minutes(e.duration().num_minutes())),
            (false, _) => format_minutes(e.duration().num_minutes()),
        };
        g.put(*y, (left + width).saturating_sub(tail.chars().count()), &tail, Role::Muted);
        focus(g, Row::Time(id, i), *y, left);
        *y += 1;
    }
    if let Some(timer) = ctx.store.active_timer.as_ref().filter(|a| a.task_id == id) {
        g.put(*y, left + 1, &format!("{}  {}–now", timer.started_at.format("%a %d %b"), timer.started_at.format("%H:%M")), Role::Good);
        let live = format!("● {}", format_minutes((ctx.now - timer.started_at).num_minutes()));
        g.put(*y, (left + width).saturating_sub(live.chars().count()), &live, Role::Good);
        *y += 1;
    }
    g.put(*y, left + 1, "+ log time", Role::Accent);
    focus(g, Row::AddTime(id), *y, left);
    *y += 1;
}

/// A section heading whose rule stops short of a hint at its right end.
fn heading_with_hint(g: &mut Grid, y: usize, left: usize, width: usize, title: &str, hint: &str) {
    let n = hint.chars().count();
    g.heading(y, left, width.saturating_sub(n + 2), title);
    g.put(y, (left + width).saturating_sub(n), hint, Role::Muted);
}

fn ago(at: DateTime<Local>, now: DateTime<Local>) -> String {
    let minutes = (now - at).num_minutes();
    match minutes {
        m if m < 1 => "just now".to_string(),
        m if m < 60 => format!("{m}m ago"),
        m if m < 24 * 60 => format!("{}h ago", m / 60),
        _ => at.format("%-d %b %H:%M").to_string(),
    }
}

/// The menu, form, review, filters or key list floating over the page.
fn popups(g: &mut Grid, page: &AgendaPage, ctx: &Ctx, line: usize, left: usize, width: usize) {
    let col = left + 4;
    if page.help {
        let rows = help_rows(page);
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
        Some(Form::Line { typing, text }) => {
            let mut rows = vec![vec![(typing.label().to_string(), Role::Title)], Vec::new()];
            rows.push(vec![(format!("{:<50}", fit_tail(&format!("{text}▏"), 60)), Role::Title)]);
            if let Some((why, true)) = &page.note {
                rows.push(Vec::new());
                rows.push(vec![(why.clone(), Role::Bad)]);
            }
            rows.push(Vec::new());
            let hint = if matches!(typing, Typing::Check(_)) { "Enter adds and stays for the next · Esc done" } else { "Enter keeps · Esc leaves it" };
            rows.push(vec![(hint.into(), Role::Muted)]);
            g.popup = Some(Popup { line, col, rows });
            return;
        }
        Some(Form::New { title, priority, category, due, status, clock, at }) => {
            let scope = page.project.as_ref().map(|p| format!(" · {}", p.name)).unwrap_or_default();
            let mut rows = vec![vec![(format!("New task{scope}"), Role::Title)], Vec::new()];
            let values = [
                if *at == 0 { fit_tail(&format!("{title}▏"), 56) } else if title.is_empty() { "…".to_string() } else { fit(title, 56) },
                format!("‹ {} ›", priority.label()),
                format!("‹ {} ›", category.as_deref().unwrap_or("none")),
                if *at == 3 { format!("{due}▏") } else if due.is_empty() { "none".to_string() } else { due.clone() },
                if *clock { "[x] start the clock".to_string() } else { "[ ] start the clock".to_string() },
            ];
            for (i, (label, value)) in NEW_FIELDS.iter().zip(values).enumerate() {
                let on = i == *at;
                rows.push(vec![(format!("{label:<10}"), if on { Role::Accent } else { Role::Muted }), (format!("{value:<40}"), if on { Role::Title } else { Role::Text })]);
            }
            if *status != Status::Todo {
                rows.push(vec![(format!("{:<10}{}", "Column", status.label()), Role::Muted)]);
            }
            rows.push(Vec::new());
            rows.push(vec![("!high #category due:fri PROJ-12 work in the title".into(), Role::Muted)]);
            if let Some((why, true)) = &page.note {
                rows.push(vec![(why.clone(), Role::Bad)]);
            }
            rows.push(vec![("Enter adds · Tab next field · h/l change · Esc cancels".into(), Role::Muted)]);
            g.popup = Some(Popup { line, col, rows });
            return;
        }
        None => {}
    }
    if let Some(at) = page.worklogs {
        let round = if ctx.round > 1 { format!(" · rounded to {}", format_minutes(ctx.round as i64)) } else { String::new() };
        let mut rows = vec![vec![(format!("Worklogs to send{round}"), Role::Title)], Vec::new()];
        if ctx.worklogs.is_empty() {
            rows.push(vec![("Nothing to send -- all your time on linked tasks is in Jira.".into(), Role::Muted)]);
        }
        let total: i64 = ctx.worklogs.iter().map(|r| r.minutes).sum();
        for (i, r) in ctx.worklogs.iter().enumerate() {
            let on = i == at.min(ctx.worklogs.len() - 1);
            let amount = if r.minutes == r.actual_minutes { format_minutes(r.minutes) } else { format!("{} → {}", format_minutes(r.actual_minutes), format_minutes(r.minutes)) };
            rows.push(vec![
                (if on { "› ".to_string() } else { "  ".to_string() }, Role::Accent),
                (format!("{}  ", r.date.format("%a %d %b")), Role::Muted),
                (format!("{:<10}", r.key), Role::Accent),
                (format!("{:<32}", fit(&r.title, 30)), if on { Role::Title } else { Role::Text }),
                (amount, Role::Text),
            ]);
        }
        rows.push(Vec::new());
        if total > 0 {
            rows.push(vec![(format!("Total {}", format_minutes(total)), Role::Title)]);
        }
        rows.push(vec![("e amount · D leave out · x never send · W send all · Esc close".into(), Role::Muted)]);
        g.popup = Some(Popup { line, col, rows });
        return;
    }
    if let Some(at) = page.filters {
        let mut rows = vec![vec![("Show only".to_string(), Role::Title)], Vec::new()];
        for (i, (label, on)) in page.chips(ctx).into_iter().enumerate() {
            let cur = i == at;
            rows.push(vec![(if on { "[x] " } else { "[ ] " }.to_string(), if on { Role::Good } else { Role::Muted }), (label, if cur { Role::Title } else { Role::Text }), (if cur { "  ‹" } else { "" }.to_string(), Role::Accent)]);
        }
        rows.push(Vec::new());
        rows.push(vec![("Space toggles · F clears all · Esc done".into(), Role::Muted)]);
        g.popup = Some(Popup { line, col, rows });
    }
    let _ = width;
}

fn help_rows(page: &AgendaPage) -> Vec<Vec<(String, Role)>> {
    let mut groups: Vec<(&str, Vec<(&str, &str)>)> = vec![
        ("Move", vec![("1-4 Tab", "Today, Board, List, Time"), ("j k g G", "rows"), ("Enter", "open"), ("Esc", "back"), ("/", "search"), ("f F", "filter, clear"), ("P", "this project only"), ("R", "sync with Jira"), ("q", "close")]),
        (
            "A task",
            vec![
                ("n", "new task"),
                ("s p c u", "status, priority, category, due"),
                ("e E", "title, description"),
                ("t T", "clock, log time"),
                ("N C", "private note, Jira comment"),
                ("I A", "link to Jira, assign"),
                ("y o r", "copy link, browser, retry"),
                ("x D", "archive, delete"),
            ],
        ),
    ];
    match page.view {
        View::Tab(Tab::Board) => groups.push(("Board", vec![("h l j k", "move around"), ("H L", "move the card"), ("J K", "reorder")])),
        View::Tab(Tab::Time) => groups.push(("Time", vec![("[ ]", "week before, after"), ("W", "review and send worklogs"), ("y", "copy the week")])),
        View::Tab(Tab::List) => groups.push(("List", vec![("J K", "reorder")])),
        View::Task(_) => groups.push(("This page", vec![("h l", "change a field"), ("a", "add to the section"), ("d d", "delete the row"), ("Space", "tick an item")])),
        View::Tab(Tab::Today) => {}
    }
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
    rows
}

fn key_strip(page: &AgendaPage, sel: Option<Row>, ctx: &Ctx) -> Vec<(&'static str, &'static str)> {
    if page.searching {
        return vec![("Enter", "keep"), ("Esc", "clear")];
    }
    if page.help || page.menu.is_some() {
        return vec![("Enter", "pick"), ("Esc", "leave it")];
    }
    if page.form.is_some() {
        return vec![("Enter", "keep"), ("Esc", "leave it")];
    }
    if page.worklogs.is_some() {
        return vec![("e", "amount"), ("D", "leave out"), ("x", "never send"), ("W", "send all"), ("Esc", "close")];
    }
    if page.filters.is_some() {
        return vec![("Space", "toggle"), ("F", "clear"), ("Esc", "done")];
    }
    let linked = sel.and_then(Row::task).and_then(|id| ctx.task(id)).is_some_and(|t| t.jira.is_some());
    let mut keys: Vec<(&str, &str)> = match (page.view, sel) {
        (View::Task(_), Some(Row::Field(..))) => vec![("h/l", "change"), ("Enter", "edit"), ("t", "clock"), ("E", "description"), ("a", "checklist item")],
        (View::Task(_), Some(Row::Check(..))) => vec![("Space", "tick"), ("e", "edit"), ("a", "add"), ("d", "delete")],
        (View::Task(_), Some(Row::Note(..))) => vec![("Enter", "edit"), ("a", "add"), ("d", "delete")],
        (View::Task(_), Some(Row::Time(..))) => vec![("Enter", "correct"), ("a", "log time"), ("d", "delete")],
        (View::Task(_), Some(Row::DependsOn(..))) => vec![("Enter", "open"), ("a", "add"), ("d", "remove")],
        (View::Task(_), Some(Row::Conflict(..))) => vec![("Enter", "choose")],
        (View::Task(_), Some(Row::Code(_))) => vec![("Enter", "go to the code")],
        (View::Task(_), _) => vec![("Enter", "add"), ("t", "clock")],
        (View::Tab(Tab::Board), _) => vec![("H/L", "move card"), ("J/K", "reorder"), ("Enter", "open"), ("t", "clock"), ("s", "status")],
        (View::Tab(Tab::Time), _) => vec![("[ ]", "week"), ("Enter", "entries"), ("W", "send worklogs"), ("y", "copy week"), ("t", "clock")],
        (View::Tab(_), Some(Row::Unsent)) => vec![("Enter", "review worklogs")],
        (View::Tab(_), _) => vec![("Enter", "open"), ("t", "clock"), ("s", "status"), ("p", "priority"), ("u", "due")],
    };
    if linked && !matches!(page.view, View::Tab(Tab::Time)) {
        keys.push(("C", "comment"));
    }
    if matches!(page.view, View::Task(_)) {
        keys.push(("Esc", "back"));
    } else {
        keys.extend([("n", "new"), ("1-4", "tabs"), ("/", "search"), ("f", "filter")]);
    }
    keys.extend([("?", "all keys"), ("q", "close")]);
    keys
}

#[cfg(test)]
mod tests {
    use super::*;
    use fenix_agenda::{RemoteSnapshot, RemoteUpdate};

    fn now() -> DateTime<Local> {
        Local.with_ymd_and_hms(2026, 9, 24, 14, 0, 0).unwrap()
    }

    fn store() -> (AgendaStore, TaskId, TaskId, TaskId) {
        let mut s = AgendaStore::default();
        let a = s.create_task("Write the agenda page".into(), String::new(), Priority::High, Some("fenix".into()));
        let b = s.create_task("Rotate the token".into(), String::new(), Priority::Urgent, Some("ops".into()));
        let c = s.create_task("Tidy the wiki".into(), String::new(), Priority::Low, None);
        s.set_status(a, Status::InProgress);
        s.set_due(b, Some(NaiveDate::from_ymd_opt(2026, 9, 23).unwrap()));
        (s, a, b, c)
    }

    fn ctx<'a>(store: &'a AgendaStore, categories: &'a [String]) -> Ctx<'a> {
        Ctx { store, now: now(), categories, worklogs: &[], round: 15, sync: String::new() }
    }

    fn page() -> AgendaPage {
        AgendaPage::new(Tab::Today, Some(Project { name: "fenix".into(), jira_key: Some("FEN".into()) }))
    }

    fn cats() -> Vec<String> {
        vec!["fenix".into(), "ops".into()]
    }

    #[test]
    fn today_says_what_is_running_due_next_and_done() {
        let (s, _, _, _) = store();
        let cats = cats();
        let text = layout(&page(), &ctx(&s, &cats), 130).text;
        let at = |needle: &str| text.find(needle).unwrap_or_else(|| panic!("{needle} missing:\n{text}"));
        assert!(at("IN PROGRESS") < at("Write the agenda page"));
        assert!(at("DUE") < at("Rotate the token"), "overdue shows under Due");
        assert!(text.contains("yesterday"));
        assert!(at("NEXT UP") < at("Tidy the wiki"));
        assert!(text.contains("THIS WEEK"));
    }

    #[test]
    fn number_keys_switch_tabs_and_enter_opens_a_task_that_esc_leaves() {
        let (s, a, _, _) = store();
        let cats = cats();
        let c = ctx(&s, &cats);
        let mut p = page();
        p.key(Key::Char('2'), &c);
        assert_eq!(p.view, View::Tab(Tab::Board));
        p.key(Key::Char('1'), &c);
        assert_eq!(p.key(Key::Enter, &c), Action::None);
        assert_eq!(p.view, View::Task(a), "the first row is In progress");
        assert!(layout(&p, &c, 130).text.contains("CHECKLIST"));
        p.key(Key::Escape, &c);
        assert_eq!(p.view, View::Tab(Tab::Today));
    }

    #[test]
    fn a_local_tasks_status_is_a_menu_and_a_linked_ones_goes_to_jira() {
        let (mut s, a, b, _) = store();
        let cats = cats();
        let mut p = page();
        p.key(Key::Char('s'), &ctx(&s, &cats));
        assert!(layout(&p, &ctx(&s, &cats), 130).popup.unwrap().text().contains("Blocked"));
        p.key(Key::Char('j'), &ctx(&s, &cats));
        assert_eq!(p.key(Key::Enter, &ctx(&s, &cats)), Action::SetStatus(a, Status::Blocked));

        let update = RemoteUpdate { snapshot: RemoteSnapshot::default(), status: Status::Todo, priority: Priority::High, mine: None };
        s.link(b, "OPS-88".into(), update);
        p.select(b);
        assert_eq!(p.key(Key::Char('s'), &ctx(&s, &cats)), Action::JiraStatus(b));
    }

    #[test]
    fn search_and_filters_narrow_every_tab_and_p_keeps_to_the_project() {
        let (s, _, _, _) = store();
        let cats = cats();
        let c = ctx(&s, &cats);
        let mut p = page();
        p.key(Key::Char('3'), &c);
        p.key(Key::Char('P'), &c);
        let text = layout(&p, &c, 130).text;
        assert!(text.contains("Write the agenda page") && !text.contains("Rotate the token"), "{text}");
        p.key(Key::Char('P'), &c);
        p.key(Key::Char('/'), &c);
        for ch in "wiki".chars() {
            p.key(Key::Char(ch), &c);
        }
        p.key(Key::Enter, &c);
        let text = layout(&p, &c, 130).text;
        assert!(text.contains("Tidy the wiki") && !text.contains("Write the agenda page"));
        p.key(Key::Escape, &c);
        p.key(Key::Char('f'), &c);
        p.key(Key::Char('j'), &c);
        p.key(Key::Space, &c);
        p.key(Key::Escape, &c);
        assert!(p.filter.pressing);
        let text = layout(&p, &c, 130).text;
        assert!(!text.contains("Tidy the wiki"), "Low is filtered out");
    }

    #[test]
    fn the_new_task_form_reads_its_title_line_and_files_under_the_project() {
        let (s, _, _, _) = store();
        let cats = cats();
        let c = ctx(&s, &cats);
        let mut p = page();
        p.key(Key::Char('P'), &c);
        p.key(Key::Char('n'), &c);
        assert!(p.typing());
        p.paste("Retry sync after a VPN drop !high due:fri FEN-12");
        let Action::Create(spec) = p.key(Key::Enter, &c) else { panic!("{:?}", layout(&p, &c, 130).all_text()) };
        assert_eq!(spec.title, "Retry sync after a VPN drop");
        assert_eq!(spec.priority, Priority::High);
        assert_eq!(spec.category.as_deref(), Some("fenix"));
        assert_eq!(spec.due, NaiveDate::from_ymd_opt(2026, 9, 25), "the next Friday");
        assert_eq!(spec.link.as_deref(), Some("FEN-12"));
    }

    #[test]
    fn the_task_page_edits_fields_in_place_and_its_lists_answer_a_and_d() {
        let (mut s, a, _, _) = store();
        s.add_subtask(a, "Tabs".into());
        let cats = cats();
        let mut p = page();
        p.open_task(a);
        let c = ctx(&s, &cats);
        p.key(Key::Char('j'), &c);
        assert_eq!(p.key(Key::Char('l'), &c), Action::SetStatus(a, Status::Blocked));
        p.key(Key::Char('j'), &c);
        p.key(Key::Char('j'), &c);
        p.key(Key::Char('j'), &c);
        assert_eq!(p.key(Key::Char('l'), &c), Action::SetDue(a, Some(now().date_naive())), "l on no date starts at today");
        p.key(Key::Char('j'), &c);
        p.key(Key::Char('j'), &c);
        assert_eq!(p.sel, Some(Row::Check(a, 0)));
        assert_eq!(p.key(Key::Space, &c), Action::ToggleCheck(a, 0));
        assert_eq!(p.key(Key::Char('d'), &c), Action::None, "d once asks");
        assert_eq!(p.key(Key::Char('d'), &c), Action::RemoveCheck(a, 0));
        p.key(Key::Char('a'), &c);
        p.paste("Filters");
        assert_eq!(p.key(Key::Enter, &c), Action::AddCheck(a, "Filters".into()));
        assert!(p.typing(), "stays open for the next item");
    }

    #[test]
    fn the_board_moves_cards_between_columns_with_h_and_l() {
        let (s, a, _, _) = store();
        let cats = cats();
        let c = ctx(&s, &cats);
        let mut p = page();
        p.key(Key::Char('2'), &c);
        p.key(Key::Char('l'), &c);
        assert_eq!(p.sel, Some(Row::Task(a)));
        assert_eq!(p.key(Key::Char('L'), &c), Action::SetStatus(a, Status::Blocked));
        let text = layout(&p, &c, 130).text;
        assert!(text.contains("IN PROGRESS · 1") && text.contains("Write the agenda"), "{text}");
    }

    #[test]
    fn time_shows_the_week_and_opens_the_review() {
        let (mut s, a, _, _) = store();
        let day = |h| Local.with_ymd_and_hms(2026, 9, 22, h, 0, 0).unwrap();
        s.log_span(a, day(9), day(11));
        let cats = cats();
        let c = ctx(&s, &cats);
        let mut p = page();
        p.key(Key::Char('4'), &c);
        let text = layout(&p, &c, 130).text;
        assert!(text.contains("Tue 22") && text.contains("2h 00m"), "{text}");
        p.key(Key::Char('W'), &c);
        assert!(layout(&p, &c, 130).popup.unwrap().text().contains("Worklogs to send"));
        let Action::Copy(week) = ({
            p.key(Key::Escape, &c);
            p.key(Key::Char('y'), &c)
        }) else { panic!() };
        assert!(week.contains("Write the agenda page"));
    }

    #[test]
    fn dates_and_spans_are_read_the_way_people_write_them() {
        let today = NaiveDate::from_ymd_opt(2026, 9, 24).unwrap();
        assert_eq!(parse_date("fri", today), Ok(NaiveDate::from_ymd_opt(2026, 9, 25)));
        assert_eq!(parse_date("thu", today), Ok(Some(today)), "today counts");
        assert_eq!(parse_date("+3", today), Ok(NaiveDate::from_ymd_opt(2026, 9, 27)));
        assert_eq!(parse_date("2w", today), Ok(NaiveDate::from_ymd_opt(2026, 10, 8)));
        assert_eq!(parse_date("10-02", today), Ok(NaiveDate::from_ymd_opt(2026, 10, 2)));
        assert_eq!(parse_date("01-05", today), Ok(NaiveDate::from_ymd_opt(2027, 1, 5)), "a past day is next year's");
        assert_eq!(parse_date("none", today), Ok(None));
        assert!(parse_date("someday", today).is_err());
        let (start, end) = parse_span("9:15-10:40", today, now()).unwrap();
        assert_eq!((end - start).num_minutes(), 85);
        let (start, end) = parse_span("1h 30m", today, now()).unwrap();
        assert_eq!((end, (end - start).num_minutes()), (now(), 90));
        assert!(parse_span("10:00-9:00", today, now()).is_err());
    }

    #[test]
    fn a_title_line_keeps_what_isnt_syntax() {
        let p = parse_title("Fix the #ui login !! due:tom OPS-3 now");
        assert_eq!(p.title, "Fix the login now");
        assert_eq!(p.priority, Some(Priority::Urgent));
        assert_eq!(p.category.as_deref(), Some("ui"));
        assert_eq!(p.due.as_deref(), Some("tom"));
        assert_eq!(p.link.as_deref(), Some("OPS-3"));
        assert_eq!(parse_title("Ship v2-3 build").link, None);
    }

    #[test]
    fn deleting_a_task_asks_first() {
        let (s, a, _, _) = store();
        let cats = cats();
        let c = ctx(&s, &cats);
        let mut p = page();
        assert_eq!(p.key(Key::Char('D'), &c), Action::None);
        assert!(p.note.as_ref().unwrap().0.contains("D again"));
        assert_eq!(p.key(Key::Char('D'), &c), Action::Delete(a));
    }

    #[test]
    fn question_mark_lists_every_key_and_any_key_closes_it() {
        let (s, _, _, _) = store();
        let cats = cats();
        let c = ctx(&s, &cats);
        let mut p = page();
        p.key(Key::Char('?'), &c);
        assert!(layout(&p, &c, 130).popup.unwrap().text().contains("status, priority, category, due"));
        p.key(Key::Char('x'), &c);
        assert!(layout(&p, &c, 130).popup.is_none());
    }
}
