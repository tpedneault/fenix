//! The agenda's Jira side: keeping linked tasks and their issues in step.
//!
//! `fenix-agenda` owns the model and the merge rules; this is everything
//! that needs the network or the UI -- fetching issues and turning them
//! into `RemoteUpdate`s, sending the outbox one op at a time, resolving a
//! board move into a real workflow transition, learning what "Blocked"
//! means in each project, linking/importing issues, and the worklog
//! review. Every request runs on a background thread and comes back as
//! one `AgendaSyncEvent`; without an event loop (tests) the same work runs
//! inline.

use super::*;

use fenix_agenda::{OpKind, RemoteComment, RemoteSnapshot, RemoteUpdate, Status, WorklogRow};
use fenix_config::JiraBlocked;

/// Shown on a linked task whose issue a sync couldn't find.
const NOT_FOUND: &str = "not found in Jira -- deleted, moved, or no longer visible to you";

/// How long a sync stays fresh enough that opening the agenda doesn't
/// start another one.
const STALE_AFTER: Duration = Duration::from_secs(120);

/// Bookkeeping for the agenda's Jira sync, kept apart from the store
/// because none of it is worth saving.
#[derive(Default)]
pub(super) struct AgendaSyncState {
    /// The op currently being sent -- ops go one at a time, in order, so
    /// a title change can't overtake the transition queued before it.
    pub(super) in_flight: Option<u64>,
    pub(super) pulling: bool,
    /// Pull again once the outbox empties (a comment was posted, and its
    /// real author/timestamp should replace the "sending" placeholder).
    pub(super) pull_after_drain: bool,
    pub(super) last_pull: Option<Instant>,
    /// The token owner's username, for "still assigned to you".
    pub(super) me: Option<String>,
    /// The Flagged field's id: `None` until asked, `Some(None)` when the
    /// instance has none.
    pub(super) flag_field: Option<Option<String>>,
    /// What the open `ActivePicker::WorkSync` picker is for.
    pub(super) picker: Option<SyncPickerCtx>,
    /// Worklog review edits and drops, by (task, day). Reset each time
    /// the review is opened.
    pub(super) worklog_edits: HashMap<(TaskId, chrono::NaiveDate), i64>,
    pub(super) worklog_dropped: HashSet<(TaskId, chrono::NaiveDate)>,
    pub(super) sending_worklogs: bool,
    pub(super) ticker_started: bool,
}

pub(super) struct SyncPickerCtx {
    /// The modeline badge while it's open.
    pub(super) label: String,
    pub(super) task: Option<TaskId>,
    pub(super) project: Option<String>,
    /// Where the task's card was before a board move this picker is
    /// finishing -- cancelling puts it back.
    pub(super) revert: Option<(Status, i64)>,
}

/// One row of an `ActivePicker::WorkSync` picker.
#[derive(Debug, Clone)]
pub(crate) enum SyncPick {
    /// Link to this issue.
    Issue(Box<fenix_jira::IssueDetail>),
    CreateIssue,
    CreateIn(String),
    TypeKey,
    Unlink,
    OpenInBrowser,
    /// Add this issue to the agenda (the import picker).
    Import(Box<fenix_jira::IssueDetail>),
    Transition(fenix_jira::Transition),
    /// The "Blocked..." row of a linked task's status picker.
    StartBlocked,
    /// What Blocked means in a project, and -- when chosen while blocking
    /// a task -- the transition that gets there.
    Blocked(JiraBlocked, Option<fenix_jira::Transition>),
    Priority(String),
    Assignee(String, String),
}

/// What an issue fetch was for.
#[derive(Debug, Clone)]
pub enum IssueFetch {
    LinkPicker(TaskId),
    Import,
    LinkKey(TaskId),
    Created(TaskId),
    AddFromPanel { clock: bool },
}

/// Every background result the agenda's Jira side can come back with --
/// carried by `FenixUserEvent::AgendaSync`.
#[derive(Debug)]
pub enum AgendaSyncEvent {
    Pulled {
        keys: Vec<String>,
        quiet: bool,
        me: Option<String>,
        flag_field: Option<Option<String>>,
        result: Result<Vec<fenix_jira::IssueDetail>, String>,
    },
    OpDone { op_id: u64, key: String, what: String, flag_field: Option<Option<String>>, result: Result<(), String> },
    Issues { purpose: IssueFetch, result: Result<Vec<fenix_jira::IssueDetail>, String> },
    Transitions { task: TaskId, target: Option<Status>, prev: (Status, i64), result: Result<Vec<fenix_jira::Transition>, String> },
    Priorities { task: TaskId, result: Result<Vec<fenix_jira::Priority>, String> },
    ProjectStatuses { project: String, result: Result<Vec<fenix_jira::StatusInfo>, String> },
    WorklogsSent { results: Vec<WorklogSent> },
    /// The periodic sync timer.
    Tick,
}

/// How sending one worklog review row went.
#[derive(Debug)]
pub struct WorklogSent {
    task: TaskId,
    entries: Vec<usize>,
    key: String,
    minutes: i64,
    result: Result<(), String>,
}

/// Opens `url` in the default browser.
fn open_in_browser(url: &str) -> std::io::Result<()> {
    #[cfg(windows)]
    let mut command = {
        let mut c = std::process::Command::new("rundll32");
        c.args(["url.dll,FileProtocolHandler", url]);
        c
    };
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut c = std::process::Command::new("open");
        c.arg(url);
        c
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut command = {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(url);
        c
    };
    command.spawn().map(|_| ())
}

fn transition_label(t: &fenix_jira::Transition) -> String {
    if t.to_name.is_empty() || t.to_name == t.name {
        t.name.clone()
    } else {
        format!("{}  ->  {}", t.name, t.to_name)
    }
}

fn issue_label(d: &fenix_jira::IssueDetail) -> String {
    format!("{}  {}  [{}]", d.key, d.summary, d.status)
}

impl App {
    // -- Mapping Jira onto the agenda -------------------------------------

    /// Which agenda column a Jira status lands in: the project's chosen
    /// Blocked status (or the Flagged field, if that's what Blocked means
    /// there) first, then the status's category.
    pub(super) fn agenda_status_for(&self, project: &str, status_id: &str, category: &str, flagged: bool) -> Status {
        match self.config.jira_blocked_for(project) {
            Some(JiraBlocked::Status { id, .. }) if id == status_id => Status::Blocked,
            Some(JiraBlocked::Flag) if flagged => Status::Blocked,
            _ => fenix_agenda::jira::status_for_category(category),
        }
    }

    /// `[jira] priorityN = Name|Level` first, then a guess from the name.
    pub(super) fn agenda_map_priority(&self, name: &str) -> fenix_agenda::Priority {
        self.config
            .jira_priority_map
            .iter()
            .find(|(jira, _)| jira.eq_ignore_ascii_case(name))
            .and_then(|(_, level)| fenix_agenda::Priority::ALL.into_iter().find(|p| p.label().eq_ignore_ascii_case(level)))
            .unwrap_or_else(|| fenix_agenda::jira::guess_priority(name))
    }

    pub(super) fn agenda_remote_update(&self, d: &fenix_jira::IssueDetail) -> RemoteUpdate {
        let project = fenix_jira::project_of(&d.key);
        RemoteUpdate {
            snapshot: RemoteSnapshot {
                summary: d.summary.clone(),
                description: d.description.clone().unwrap_or_default(),
                status_id: d.status_id.clone(),
                status_name: d.status.clone(),
                status_category: d.status_category.clone(),
                priority: d.priority.clone(),
                assignee: d.assignee.clone(),
                assignee_id: d.assignee_id.clone(),
                flagged: d.flagged,
                updated: d.updated.clone(),
                due: d.due.clone(),
                comments: d
                    .comments
                    .iter()
                    .map(|c| RemoteComment { id: c.id.clone(), author: c.author.clone(), body: c.body.clone(), created: c.created.clone() })
                    .collect(),
            },
            status: self.agenda_status_for(project, &d.status_id, &d.status_category, d.flagged),
            priority: d.priority.as_deref().map(|p| self.agenda_map_priority(p)).unwrap_or(fenix_agenda::Priority::Medium),
            mine: self.agenda_sync.me.as_ref().map(|me| d.assignee_id.as_deref() == Some(me.as_str())),
        }
    }

    /// The Flagged field id to request, when any project uses it.
    fn agenda_flag_field_wanted(&self) -> Option<Option<String>> {
        let wanted = self.config.jira_blocked.iter().any(|(_, b)| *b == JiraBlocked::Flag);
        wanted.then(|| self.agenda_sync.flag_field.clone().flatten())
    }

    // -- Background plumbing ----------------------------------------------

    /// Runs `job` against a Jira client off the UI thread, delivering its
    /// event back through the event loop (or inline, with no event loop).
    /// `false` when Jira isn't configured -- the error is already shown.
    pub(super) fn agenda_spawn<F>(&mut self, job: F) -> bool
    where
        F: FnOnce(&fenix_jira::JiraClient) -> AgendaSyncEvent + Send + 'static,
    {
        let Some(client) = self.jira_client() else { return false };
        match self.event_proxy.clone() {
            Some(proxy) => {
                std::thread::spawn(move || {
                    let event = job(&client);
                    let _ = proxy.send_event(FenixUserEvent::AgendaSync(event));
                });
            }
            None => {
                let event = job(&client);
                self.apply_agenda_sync(event);
            }
        }
        true
    }

    /// Saves, re-renders, and starts sending whatever the edit queued.
    pub(super) fn agenda_after_edit(&mut self) {
        self.agenda_save_and_refresh();
        self.agenda_drain_outbox();
    }

    /// Starts the periodic sync timer, once, when there's an event loop
    /// to deliver its ticks and it isn't turned off.
    pub(super) fn agenda_start_ticker(&mut self) {
        if self.agenda_sync.ticker_started {
            return;
        }
        let minutes = self.config.jira_sync_minutes.unwrap_or(10);
        let Some(proxy) = self.event_proxy.clone() else { return };
        if minutes == 0 {
            return;
        }
        self.agenda_sync.ticker_started = true;
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_secs(u64::from(minutes) * 60));
            if proxy.send_event(FenixUserEvent::AgendaSync(AgendaSyncEvent::Tick)).is_err() {
                break;
            }
        });
    }

    // -- Pull ---------------------------------------------------------------

    /// `SPC a s`.
    pub(crate) fn cmd_agenda_sync(&mut self) {
        self.agenda_sync_now(false);
    }

    /// Refreshes every linked task from Jira in one search, then sends
    /// anything waiting. `quiet` skips the "nothing linked"/"synced"
    /// messages and swallows a failed fetch -- for the timer and for
    /// opening the agenda, where a dropped VPN shouldn't shout.
    pub(super) fn agenda_sync_now(&mut self, quiet: bool) {
        self.agenda_start_ticker();
        if self.agenda_sync.pulling {
            return;
        }
        let keys: Vec<String> = self.agenda_store.tasks.iter().filter_map(|t| t.jira_key().map(str::to_string)).collect();
        if keys.is_empty() {
            if !quiet {
                self.set_message("No tasks are linked to Jira yet -- I on a task links one, SPC a i imports your issues");
            }
            return;
        }
        let need_me = self.agenda_sync.me.is_none();
        let want_flag = self.config.jira_blocked.iter().any(|(_, b)| *b == JiraBlocked::Flag);
        let known_flag = self.agenda_sync.flag_field.clone();
        self.agenda_sync.pulling = true;
        self.agenda_sync.last_pull = Some(Instant::now());
        let spawned = self.agenda_spawn(move |client| {
            let me = if need_me { client.myself().ok() } else { None };
            let flag_field = match (&known_flag, want_flag) {
                (None, true) => client.find_flagged_field().ok(),
                _ => None,
            };
            let flag = flag_field.clone().or(known_flag).flatten();
            let mut issues = Vec::new();
            for chunk in keys.chunks(50) {
                match client.search_details(&fenix_jira::build_keys_jql(chunk), chunk.len() as u32, flag.as_deref()) {
                    Ok(found) => issues.extend(found),
                    Err(err) => return AgendaSyncEvent::Pulled { keys, quiet, me, flag_field, result: Err(err) },
                }
            }
            AgendaSyncEvent::Pulled { keys, quiet, me, flag_field, result: Ok(issues) }
        });
        if !spawned {
            self.agenda_sync.pulling = false;
        }
    }

    /// Opening the agenda refreshes it quietly unless it just did.
    pub(super) fn agenda_sync_if_stale(&mut self) {
        let fresh = self.agenda_sync.last_pull.is_some_and(|at| at.elapsed() < STALE_AFTER);
        if !fresh && self.agenda_store.tasks.iter().any(|t| t.jira.is_some()) {
            self.agenda_sync_now(true);
        }
    }

    fn apply_agenda_pulled(&mut self, keys: Vec<String>, quiet: bool, result: Result<Vec<fenix_jira::IssueDetail>, String>) {
        self.agenda_sync.pulling = false;
        let issues = match result {
            Ok(issues) => issues,
            Err(err) => {
                if !quiet {
                    self.set_error(format!("Jira sync failed: {err}"));
                }
                return;
            }
        };
        let mut conflicts = 0;
        for detail in &issues {
            let Some(id) = self.agenda_store.find_by_key(&detail.key) else { continue };
            let update = self.agenda_remote_update(detail);
            self.agenda_store.apply_remote(id, update);
            if self.agenda_store.task(id).and_then(|t| t.jira.as_ref()).is_some_and(|l| l.error.as_deref() == Some(NOT_FOUND)) {
                self.agenda_store.set_link_error(id, None);
            }
            conflicts += self.agenda_store.task(id).and_then(|t| t.jira.as_ref()).map_or(0, |l| l.conflicts.len());
        }
        for key in &keys {
            if !issues.iter().any(|d| &d.key == key) {
                if let Some(id) = self.agenda_store.find_by_key(key) {
                    self.agenda_store.set_link_error(id, Some(NOT_FOUND.to_string()));
                }
            }
        }
        self.agenda_save_and_refresh();
        if conflicts > 0 {
            self.set_error(format!("Jira sync: {conflicts} conflicting change(s) -- open the task and press Enter on the conflict"));
        } else if !quiet {
            self.set_message(format!("Synced {} issue(s) with Jira", issues.len()));
        }
        self.agenda_drain_outbox();
    }

    // -- Push ---------------------------------------------------------------

    /// Sends the next queued op, if nothing's already in flight.
    pub(super) fn agenda_drain_outbox(&mut self) {
        if self.agenda_sync.in_flight.is_some() {
            return;
        }
        let Some(op) = self.agenda_store.next_op().cloned() else {
            if std::mem::take(&mut self.agenda_sync.pull_after_drain) {
                self.agenda_sync_now(true);
            }
            return;
        };
        let Some(key) = self.agenda_store.task(op.task).and_then(|t| t.jira_key()).map(str::to_string) else {
            self.agenda_store.discard_op(op.id);
            return;
        };
        let known_flag = self.agenda_sync.flag_field.clone();
        self.agenda_sync.in_flight = Some(op.id);
        let spawned = self.agenda_spawn(move |client| {
            let mut fetched_flag = None;
            let result = match &op.kind {
                OpKind::SetSummary(s) => client.update_summary(&key, s),
                OpKind::SetDescription(s) => client.update_description(&key, s),
                OpKind::SetPriority(p) => client.update_priority(&key, p),
                OpKind::AddComment(body) => client.add_comment(&key, body),
                OpKind::SetAssignee { id, .. } => client.update_assignee(&key, id),
                OpKind::Transition { id, .. } => client.apply_transition(&key, id),
                OpKind::SetDue(due) => client.update_due(&key, due.as_deref()),
                OpKind::SetFlag(on) => {
                    let field = match known_flag {
                        Some(field) => Ok(field),
                        None => client.find_flagged_field().inspect(|f| fetched_flag = Some(f.clone())),
                    };
                    match field {
                        Ok(Some(field)) => client.set_flagged(&key, &field, *on),
                        Ok(None) => Err("this Jira has no Flagged field -- press b on the project in SPC j j to pick another meaning for Blocked".to_string()),
                        Err(err) => Err(err),
                    }
                }
            };
            AgendaSyncEvent::OpDone { op_id: op.id, key, what: op.kind.describe(), flag_field: fetched_flag, result }
        });
        if !spawned {
            self.agenda_sync.in_flight = None;
        }
    }

    fn apply_agenda_op_done(&mut self, op_id: u64, key: String, what: String, result: Result<(), String>) {
        self.agenda_sync.in_flight = None;
        let comment = self.agenda_store.outbox.iter().any(|op| op.id == op_id && matches!(op.kind, OpKind::AddComment(_)));
        match result {
            Ok(()) => {
                self.agenda_store.op_succeeded(op_id);
                self.agenda_sync.pull_after_drain |= comment;
                self.set_message(format!("{key}: {what}"));
                self.agenda_save_and_refresh();
                self.agenda_drain_outbox();
            }
            Err(err) => {
                self.agenda_store.op_failed(op_id, err.clone());
                self.set_error(format!("{key}: Jira refused a change ({what}): {err} -- r on the task retries"));
                self.agenda_save_and_refresh();
            }
        }
    }

    /// `r` on a task: clears a failed push and tries again.
    pub(super) fn agenda_retry(&mut self, id: TaskId) {
        if self.agenda_store.task(id).and_then(|t| t.jira.as_ref()).is_none() {
            return;
        }
        self.agenda_store.retry(id);
        self.agenda_after_edit();
        self.agenda_sync_now(true);
    }

    // -- Status -------------------------------------------------------------

    /// Moves a task to `target` -- the one path `s`, the board's `H`/`L`
    /// and the status picker all take. A local task just moves. A linked
    /// one moves at once too, then Fenix works out the matching Jira
    /// change: nothing, a Flagged toggle, or a workflow transition (read
    /// fresh, since which ones exist depends on where the issue is now).
    /// If no transition gets there, the card goes back where it was.
    pub(super) fn agenda_set_status(&mut self, id: TaskId, target: Status) {
        let Some(task) = self.agenda_store.task(id) else { return };
        let prev = (task.status, task.order);
        let Some(link) = task.jira.clone() else {
            self.agenda_store.move_to_status(id, target);
            self.agenda_save_and_refresh();
            return;
        };
        if prev.0 == target {
            return;
        }
        let project = link.project().to_string();
        let blocked = self.config.jira_blocked_for(&project).cloned();
        self.agenda_store.move_to_status(id, target);

        if prev.0 == Status::Blocked && blocked == Some(JiraBlocked::Flag) && link.base.flagged {
            self.agenda_store.enqueue(id, OpKind::SetFlag(false));
        }
        let on_blocked_status = matches!(&blocked, Some(JiraBlocked::Status { id: sid, .. }) if *sid == link.base.status_id);
        let need_transition = match (target, &blocked) {
            (Status::Blocked, Some(JiraBlocked::Local)) => false,
            (Status::Blocked, Some(JiraBlocked::Flag)) => {
                self.agenda_store.enqueue(id, OpKind::SetFlag(true));
                false
            }
            (Status::Blocked, Some(JiraBlocked::Status { .. })) => !on_blocked_status,
            (Status::Blocked, None) => true,
            (_, _) => {
                let category = fenix_agenda::jira::category_for_status(target).unwrap_or("new");
                link.base.status_category != category || on_blocked_status
            }
        };
        if !need_transition {
            self.agenda_after_edit();
            return;
        }
        self.agenda_store.set_resolving(id, true);
        self.agenda_save_and_refresh();
        let key = link.key.clone();
        let spawned = self.agenda_spawn(move |client| AgendaSyncEvent::Transitions {
            task: id,
            target: Some(target),
            prev,
            result: client.list_transitions(&key),
        });
        if !spawned {
            self.agenda_store.set_resolving(id, false);
            self.agenda_store.restore_status(id, prev.0, prev.1);
            self.agenda_save_and_refresh();
        }
    }

    /// `s` on a linked task: the issue's real transitions, by name, plus
    /// a Blocked row for whatever Blocked means in its project.
    pub(super) fn agenda_start_linked_status_picker(&mut self, id: TaskId) {
        let Some(task) = self.agenda_store.task(id) else { return };
        let Some(key) = task.jira_key().map(str::to_string) else { return };
        let prev = (task.status, task.order);
        self.agenda_spawn(move |client| AgendaSyncEvent::Transitions { task: id, target: None, prev, result: client.list_transitions(&key) });
    }

    fn apply_agenda_transitions(
        &mut self,
        id: TaskId,
        target: Option<Status>,
        prev: (Status, i64),
        result: Result<Vec<fenix_jira::Transition>, String>,
    ) {
        self.agenda_store.set_resolving(id, false);
        let Some(link) = self.agenda_store.task(id).and_then(|t| t.jira.clone()) else { return };
        let key = link.key.clone();
        let project = link.project().to_string();
        let current = link.base.status_name.clone();
        let revert = |app: &mut App| {
            if target.is_some() {
                app.agenda_store.restore_status(id, prev.0, prev.1);
            }
            app.agenda_save_and_refresh();
        };
        let transitions = match result {
            Ok(transitions) => transitions,
            Err(err) => {
                revert(self);
                self.set_error(format!("{key}: couldn't read Jira's workflow: {err}"));
                return;
            }
        };
        let blocked = self.config.jira_blocked_for(&project).cloned();
        match target {
            None => self.agenda_open_transition_picker(id, &key, transitions, None),
            Some(Status::Blocked) => match blocked {
                Some(JiraBlocked::Status { id: status_id, name }) => match transitions.iter().find(|t| t.to_id == status_id) {
                    Some(t) => {
                        self.agenda_enqueue_transition(id, t.clone());
                        self.agenda_after_edit();
                    }
                    None => {
                        revert(self);
                        self.set_error(format!("{key}: \"{name}\" isn't reachable from \"{current}\" -- pick a step from here"));
                        self.agenda_open_transition_picker(id, &key, transitions, None);
                    }
                },
                None => {
                    let mut candidates: Vec<fenix_picker::Candidate<SyncPick>> = transitions
                        .into_iter()
                        .filter(|t| !t.to_id.is_empty())
                        .map(|t| {
                            let label = format!("{}   (via \"{}\")", t.to_name, t.name);
                            let blocked = JiraBlocked::Status { id: t.to_id.clone(), name: t.to_name.clone() };
                            fenix_picker::Candidate::new(label, SyncPick::Blocked(blocked, Some(t)))
                        })
                        .collect();
                    candidates.push(fenix_picker::Candidate::new("Flag as impediment (keep the status)", SyncPick::Blocked(JiraBlocked::Flag, None)));
                    candidates.push(fenix_picker::Candidate::new(format!("Keep Blocked local for {project}"), SyncPick::Blocked(JiraBlocked::Local, None)));
                    self.agenda_enter_sync_picker(
                        SyncPickerCtx {
                            label: format!("WHAT DOES BLOCKED MEAN IN {project}?"),
                            task: Some(id),
                            project: Some(project.clone()),
                            revert: Some(prev),
                        },
                        candidates,
                    );
                    self.set_message(format!("Pick what Blocked means in {project} -- asked once, then remembered (b on the project in SPC j j changes it)"));
                }
                // Local/Flag never needed a fetch.
                Some(_) => self.agenda_save_and_refresh(),
            },
            Some(target) => {
                let category = fenix_agenda::jira::category_for_status(target).unwrap_or("new");
                let blocked_id = match &blocked {
                    Some(JiraBlocked::Status { id, .. }) => Some(id.clone()),
                    _ => None,
                };
                let matching: Vec<fenix_jira::Transition> = transitions
                    .iter()
                    .filter(|t| t.to_category == category && Some(&t.to_id) != blocked_id.as_ref())
                    .cloned()
                    .collect();
                match matching.len() {
                    0 => {
                        revert(self);
                        self.set_error(format!("{key}: no transition from \"{current}\" to a {} status", target.label()));
                    }
                    1 => {
                        self.agenda_enqueue_transition(id, matching[0].clone());
                        self.agenda_after_edit();
                    }
                    _ => self.agenda_open_transition_picker(id, &key, matching, Some(prev)),
                }
            }
        }
    }

    fn agenda_open_transition_picker(&mut self, id: TaskId, key: &str, transitions: Vec<fenix_jira::Transition>, revert: Option<(Status, i64)>) {
        let mut candidates: Vec<fenix_picker::Candidate<SyncPick>> =
            transitions.into_iter().map(|t| fenix_picker::Candidate::new(transition_label(&t), SyncPick::Transition(t))).collect();
        if revert.is_none() {
            let project = fenix_jira::project_of(key).to_string();
            let blocked = match self.config.jira_blocked_for(&project) {
                Some(JiraBlocked::Status { name, .. }) => format!("Blocked  ({name})"),
                Some(JiraBlocked::Flag) => "Blocked  (flag as impediment)".to_string(),
                Some(JiraBlocked::Local) => "Blocked  (local only)".to_string(),
                None => "Blocked...".to_string(),
            };
            candidates.push(fenix_picker::Candidate::new(blocked, SyncPick::StartBlocked));
        }
        self.agenda_enter_sync_picker(
            SyncPickerCtx { label: format!("{key} STATUS"), task: Some(id), project: None, revert },
            candidates,
        );
    }

    /// Queues `t` and puts the card in the column it lands in.
    fn agenda_enqueue_transition(&mut self, id: TaskId, t: fenix_jira::Transition) {
        let Some(key) = self.agenda_store.task(id).and_then(|t| t.jira_key()).map(str::to_string) else { return };
        let project = fenix_jira::project_of(&key).to_string();
        let flagged = self.agenda_store.task(id).and_then(|t| t.jira.as_ref()).is_some_and(|l| l.base.flagged);
        let status = self.agenda_status_for(&project, &t.to_id, &t.to_category, flagged);
        if self.agenda_store.task(id).is_some_and(|task| task.status != status) {
            self.agenda_store.move_to_status(id, status);
        }
        self.agenda_store.enqueue(
            id,
            OpKind::Transition { id: t.id, to_id: t.to_id, to_name: t.to_name, to_category: t.to_category },
        );
    }

    // -- Priority / assignee ------------------------------------------------

    /// `p` on a linked task: the instance's real priorities.
    pub(super) fn agenda_start_linked_priority_picker(&mut self, id: TaskId) {
        self.agenda_spawn(move |client| AgendaSyncEvent::Priorities { task: id, result: client.list_priorities() });
    }

    /// `A` on a linked task: one of the tracked users.
    pub(super) fn agenda_start_assignee_picker(&mut self, id: TaskId) {
        let Some(key) = self.agenda_store.task(id).and_then(|t| t.jira_key()).map(str::to_string) else {
            self.set_message("A reassigns a Jira issue -- link this task first with I");
            return;
        };
        if self.config.jira_users.is_empty() {
            self.set_error("no tracked Jira users to assign to -- SPC j u a adds one");
            return;
        }
        let candidates = self
            .config
            .jira_users
            .iter()
            .map(|(uid, name)| fenix_picker::Candidate::new(format!("{name} ({uid})"), SyncPick::Assignee(uid.clone(), name.clone())))
            .collect();
        self.agenda_enter_sync_picker(SyncPickerCtx { label: format!("ASSIGN {key}"), task: Some(id), project: None, revert: None }, candidates);
    }

    // -- Linking ------------------------------------------------------------

    /// `I`: link a task to an issue (or, if it already has one, unlink or
    /// open it).
    pub(super) fn agenda_start_link(&mut self, id: TaskId) {
        let Some(task) = self.agenda_store.task(id) else { return };
        if let Some(key) = task.jira_key().map(str::to_string) {
            let candidates = vec![
                fenix_picker::Candidate::new(format!("Open {key} in the browser"), SyncPick::OpenInBrowser),
                fenix_picker::Candidate::new(format!("Unlink {key} (keep this as a local task)"), SyncPick::Unlink),
            ];
            self.agenda_enter_sync_picker(SyncPickerCtx { label: key, task: Some(id), project: None, revert: None }, candidates);
            return;
        }
        let jql = fenix_jira::build_my_open_issues_jql(&self.tracked_project_keys());
        let flag = self.agenda_flag_field_wanted().flatten();
        self.set_message("Fetching your open Jira issues...");
        self.agenda_spawn(move |client| AgendaSyncEvent::Issues {
            purpose: IssueFetch::LinkPicker(id),
            result: client.search_details(&jql, 100, flag.as_deref()),
        });
    }

    /// `SPC a i`: pick any of your open issues to add, several at once.
    pub(crate) fn cmd_agenda_import(&mut self) {
        let jql = fenix_jira::build_my_open_issues_jql(&self.tracked_project_keys());
        let flag = self.agenda_flag_field_wanted().flatten();
        self.set_message("Fetching your open Jira issues...");
        self.agenda_spawn(move |client| AgendaSyncEvent::Issues { purpose: IssueFetch::Import, result: client.search_details(&jql, 200, flag.as_deref()) });
    }

    fn tracked_project_keys(&self) -> Vec<String> {
        self.config.jira_projects.iter().map(|(key, _)| key.clone()).collect()
    }

    /// Fetches one issue by key and hands it to `purpose`.
    pub(super) fn agenda_fetch_issue(&mut self, key: String, purpose: IssueFetch) {
        let flag = self.agenda_flag_field_wanted().flatten();
        self.agenda_spawn(move |client| AgendaSyncEvent::Issues { purpose, result: client.get_issue_with(&key, flag.as_deref()).map(|d| vec![d]) });
    }

    /// Creates an issue from a local task (title, then description), then
    /// links the two.
    pub(super) fn agenda_create_issue_from_task(&mut self, id: TaskId, project: String, issue_type: String) {
        let Some(task) = self.agenda_store.task(id) else { return };
        let (title, description) = (task.title.clone(), task.description.clone());
        let flag = self.agenda_flag_field_wanted().flatten();
        self.set_message(format!("Creating a {project} issue..."));
        self.agenda_spawn(move |client| {
            let result = (|| {
                let key = client.create_issue(&project, &issue_type, &title)?;
                if !description.trim().is_empty() {
                    client.update_description(&key, &description)?;
                }
                client.get_issue_with(&key, flag.as_deref()).map(|d| vec![d])
            })();
            AgendaSyncEvent::Issues { purpose: IssueFetch::Created(id), result }
        });
    }

    fn agenda_link_detail(&mut self, id: TaskId, detail: &fenix_jira::IssueDetail) {
        if let Some(existing) = self.agenda_store.find_by_key(&detail.key).filter(|&t| t != id) {
            let title = self.agenda_store.task(existing).map(|t| t.title.clone()).unwrap_or_default();
            self.set_error(format!("{} is already linked to \"{title}\"", detail.key));
            return;
        }
        let update = self.agenda_remote_update(detail);
        self.agenda_store.link(id, detail.key.clone(), update);
        self.agenda_start_ticker();
        self.agenda_save_and_refresh();
        self.jira_rerender_issues();
        self.set_message(format!("Linked to {} -- edits now go to Jira too", detail.key));
    }

    fn apply_agenda_issues(&mut self, purpose: IssueFetch, result: Result<Vec<fenix_jira::IssueDetail>, String>) {
        match purpose {
            IssueFetch::LinkPicker(id) => {
                let mut candidates = vec![
                    fenix_picker::Candidate::new("+ Create a new Jira issue from this task...", SyncPick::CreateIssue),
                    fenix_picker::Candidate::new("Type an issue key...", SyncPick::TypeKey),
                ];
                match result {
                    Ok(issues) => candidates.extend(
                        issues
                            .into_iter()
                            .filter(|d| self.agenda_store.find_by_key(&d.key).is_none())
                            .map(|d| fenix_picker::Candidate::new(issue_label(&d), SyncPick::Issue(Box::new(d)))),
                    ),
                    Err(err) => self.set_error(format!("couldn't list your Jira issues: {err}")),
                }
                self.agenda_enter_sync_picker(
                    SyncPickerCtx { label: "LINK TO JIRA".to_string(), task: Some(id), project: None, revert: None },
                    candidates,
                );
            }
            IssueFetch::Import => {
                let issues = match result {
                    Ok(issues) => issues,
                    Err(err) => {
                        self.set_error(format!("couldn't list your Jira issues: {err}"));
                        return;
                    }
                };
                let candidates: Vec<_> = issues
                    .into_iter()
                    .filter(|d| self.agenda_store.find_by_key(&d.key).is_none())
                    .map(|d| fenix_picker::Candidate::new(issue_label(&d), SyncPick::Import(Box::new(d))))
                    .collect();
                if candidates.is_empty() {
                    self.set_message("Every open issue assigned to you is already in the agenda");
                    return;
                }
                self.agenda_enter_sync_picker(
                    SyncPickerCtx { label: "IMPORT  (Tab marks)".to_string(), task: None, project: None, revert: None },
                    candidates,
                );
                self.set_message("Tab marks issues to import, Enter adds them");
            }
            IssueFetch::LinkKey(id) | IssueFetch::Created(id) => match result {
                Ok(issues) => match issues.first() {
                    Some(detail) => {
                        let detail = detail.clone();
                        self.agenda_link_detail(id, &detail);
                    }
                    None => self.set_error("Jira returned no issue"),
                },
                Err(err) => self.set_error(format!("couldn't link: {err}")),
            },
            IssueFetch::AddFromPanel { clock } => match result {
                Ok(issues) => {
                    let Some(detail) = issues.first().cloned() else { return };
                    let id = match self.agenda_store.find_by_key(&detail.key) {
                        Some(id) => id,
                        None => {
                            let update = self.agenda_remote_update(&detail);
                            self.agenda_store.create_linked(detail.key.clone(), update, None)
                        }
                    };
                    self.agenda_finish_add_from_panel(id, &detail.key, clock);
                }
                Err(err) => self.set_error(format!("couldn't fetch the issue: {err}")),
            },
        }
    }

    fn agenda_finish_add_from_panel(&mut self, id: TaskId, key: &str, clock: bool) {
        self.agenda_start_ticker();
        if clock {
            self.agenda_store.clock_in(id);
            self.set_message(format!("{key} is in your agenda, clock running"));
        } else {
            self.set_message(format!("{key} is in your agenda"));
        }
        self.agenda_save_and_refresh();
        self.jira_rerender_issues();
    }

    /// `a`/`t` on the Jira panel's Issues/Detail: add the current issue to
    /// the agenda (`t` also starts the clock on it). An issue that's
    /// already there isn't added twice.
    pub(crate) fn jira_add_to_agenda(&mut self, clock: bool) {
        let Some(key) = self.jira_current_issue_key() else { return };
        if let Some(id) = self.agenda_store.find_by_key(&key) {
            self.agenda_finish_add_from_panel(id, &key, clock);
            return;
        }
        self.agenda_fetch_issue(key, IssueFetch::AddFromPanel { clock });
    }

    /// Re-renders the Jira panel's Issues pane so "in agenda" markers
    /// follow what was just linked or unlinked. The rows themselves don't
    /// move, so every pane showing it keeps its cursor on the same line
    /// (unlike `set_jira_buffer`, which starts a fresh listing at the top).
    pub(super) fn jira_rerender_issues(&mut self) {
        let Some(session) = self.jira_session.as_ref() else { return };
        let buffer = session.issues_buffer;
        let panel = jira_panel::render_issues(&session.issues, &self.agenda_linked_keys());
        let lines: Vec<(fenix_window::WindowId, usize)> = match self.buffers.get(buffer) {
            Some(ob) => self
                .windows()
                .windows()
                .into_iter()
                .filter(|&pane| self.windows().content(pane) == Some(&buffer))
                .map(|pane| (pane, ob.buffer.line_col(&self.pane_state(pane).cursor).0))
                .collect(),
            None => Vec::new(),
        };
        self.set_jira_buffer(buffer, panel);
        for (pane, line) in lines {
            let Some(ob) = self.buffers.get(buffer) else { return };
            let line = line.min(ob.buffer.line_count().saturating_sub(1));
            let cursor = Cursor { char_idx: ob.buffer.line_start_char(line), sticky_col: 0 };
            *self.pane_state_mut(pane) = PaneState::seeded_at(cursor);
        }
    }

    pub(super) fn agenda_linked_keys(&self) -> HashSet<String> {
        self.agenda_store.tasks.iter().filter_map(|t| t.jira_key().map(str::to_string)).collect()
    }

    pub(super) fn agenda_issue_url(&mut self, id: TaskId) -> Option<String> {
        let key = self.agenda_store.task(id).and_then(|t| t.jira_key()).map(str::to_string);
        let Some(key) = key else {
            self.set_message("This task isn't linked to Jira -- I links it");
            return None;
        };
        let Some(base_url) = self.config.jira_base_url.clone() else {
            self.set_error("Jira isn't set up yet -- set its server and token in SPC , (Jira & agenda)");
            return None;
        };
        Some(format!("{}/browse/{key}", base_url.trim_end_matches('/')))
    }

    /// `y` on a linked task.
    pub(super) fn agenda_copy_link(&mut self, id: TaskId) {
        let Some(url) = self.agenda_issue_url(id) else { return };
        if let Some(clipboard) = &mut self.clipboard {
            let _ = clipboard.set_text(url.clone());
        }
        self.set_message(format!("Copied: {url}"));
    }

    pub(super) fn open_url(&mut self, url: &str) {
        match open_in_browser(url) {
            Ok(()) => self.set_message(format!("Opened {url}")),
            Err(err) => self.set_error(format!("couldn't open a browser: {err}")),
        }
    }

    /// `o` on a linked task.
    pub(super) fn agenda_open_in_browser(&mut self, id: TaskId) {
        if let Some(url) = self.agenda_issue_url(id) {
            self.open_url(&url);
        }
    }

    /// `o` on the Jira panel's Issues/Detail.
    pub(crate) fn jira_open_in_browser(&mut self) {
        let Some(key) = self.jira_current_issue_key() else { return };
        let Some(base_url) = self.config.jira_base_url.clone() else {
            self.set_error("Jira isn't set up yet -- set its server and token in SPC , (Jira & agenda)");
            return;
        };
        self.open_url(&format!("{}/browse/{key}", base_url.trim_end_matches('/')));
    }

    // -- Blocked, per project -----------------------------------------------

    /// `b` on a project in the Jira panel: choose (or change) what
    /// Blocked means there, from every status the project uses.
    pub(crate) fn jira_start_blocked_setup(&mut self) {
        let Some(jira_panel::JiraEntry::Project(project)) = self.jira_entry_at_cursor() else { return };
        self.agenda_spawn(move |client| {
            let result = client.list_project_statuses(&project);
            AgendaSyncEvent::ProjectStatuses { project, result }
        });
    }

    fn apply_agenda_project_statuses(&mut self, project: String, result: Result<Vec<fenix_jira::StatusInfo>, String>) {
        let statuses = match result {
            Ok(statuses) => statuses,
            Err(err) => {
                self.set_error(format!("couldn't list {project}'s statuses: {err}"));
                return;
            }
        };
        let current = self.config.jira_blocked_for(&project).cloned();
        let mark = |b: &JiraBlocked| if current.as_ref() == Some(b) { "  (current)" } else { "" };
        let mut candidates: Vec<fenix_picker::Candidate<SyncPick>> = statuses
            .into_iter()
            .map(|s| {
                let blocked = JiraBlocked::Status { id: s.id, name: s.name.clone() };
                let label = format!("{}{}", s.name, mark(&blocked));
                fenix_picker::Candidate::new(label, SyncPick::Blocked(blocked, None))
            })
            .collect();
        candidates.push(fenix_picker::Candidate::new(
            format!("Flag as impediment (keep the status){}", mark(&JiraBlocked::Flag)),
            SyncPick::Blocked(JiraBlocked::Flag, None),
        ));
        candidates.push(fenix_picker::Candidate::new(
            format!("Keep Blocked local{}", mark(&JiraBlocked::Local)),
            SyncPick::Blocked(JiraBlocked::Local, None),
        ));
        self.agenda_enter_sync_picker(
            SyncPickerCtx { label: format!("WHAT DOES BLOCKED MEAN IN {project}?"), task: None, project: Some(project), revert: None },
            candidates,
        );
    }

    // -- The WorkSync picker ------------------------------------------------

    fn agenda_enter_sync_picker(&mut self, ctx: SyncPickerCtx, candidates: Vec<fenix_picker::Candidate<SyncPick>>) {
        self.agenda_sync.picker = Some(ctx);
        self.enter_picker(ActivePicker::WorkSync(fenix_picker::PickerState::new(candidates)));
    }

    /// `Esc` on the WorkSync picker: a board move it was finishing goes
    /// back where it came from.
    pub(super) fn agenda_sync_picker_cancel(&mut self) {
        let Some(ctx) = self.agenda_sync.picker.take() else { return };
        if let (Some(id), Some((status, order))) = (ctx.task, ctx.revert) {
            self.agenda_store.restore_status(id, status, order);
            self.agenda_save_and_refresh();
            self.set_message("Move cancelled");
        }
    }

    /// `Enter` on the WorkSync picker. `marked` is what `Tab` marked (the
    /// import picker's multi-select).
    pub(super) fn agenda_sync_picker_confirm(&mut self, pick: SyncPick, marked: Vec<SyncPick>) {
        let ctx = self.agenda_sync.picker.take();
        let task = ctx.as_ref().and_then(|c| c.task);
        let project = ctx.as_ref().and_then(|c| c.project.clone());
        match pick {
            SyncPick::Issue(detail) => {
                if let Some(id) = task {
                    self.agenda_link_detail(id, &detail);
                }
            }
            SyncPick::CreateIssue => {
                if self.config.jira_projects.is_empty() {
                    self.set_error("track a Jira project first (SPC j p a) to create issues in it");
                    return;
                }
                let candidates = self
                    .config
                    .jira_projects
                    .iter()
                    .map(|(key, name)| fenix_picker::Candidate::new(format!("[{key}] {name}"), SyncPick::CreateIn(key.clone())))
                    .collect();
                self.agenda_enter_sync_picker(
                    SyncPickerCtx { label: "CREATE IN".to_string(), task, project: None, revert: None },
                    candidates,
                );
            }
            SyncPick::CreateIn(project) => {
                if let Some(id) = task {
                    self.agenda_prompt = Some(AgendaPrompt { kind: AgendaPromptKind::LinkIssueType { id, project }, input: "Task".to_string() });
                }
            }
            SyncPick::TypeKey => {
                if let Some(id) = task {
                    self.agenda_prompt = Some(AgendaPrompt { kind: AgendaPromptKind::LinkKey { id }, input: String::new() });
                }
            }
            SyncPick::Unlink => {
                if let Some(id) = task {
                    let key = self.agenda_store.task(id).and_then(|t| t.jira_key()).map(str::to_string).unwrap_or_default();
                    self.agenda_store.unlink(id);
                    self.agenda_save_and_refresh();
                    self.jira_rerender_issues();
                    self.set_message(format!("Unlinked {key} -- the task stays, Jira isn't touched"));
                }
            }
            SyncPick::OpenInBrowser => {
                if let Some(id) = task {
                    self.agenda_open_in_browser(id);
                }
            }
            SyncPick::Import(first) => {
                let issues: Vec<Box<fenix_jira::IssueDetail>> = if marked.is_empty() {
                    vec![first]
                } else {
                    marked.into_iter().filter_map(|p| if let SyncPick::Import(d) = p { Some(d) } else { None }).collect()
                };
                let count = issues.len();
                self.agenda_start_ticker();
                for detail in issues {
                    if self.agenda_store.find_by_key(&detail.key).is_none() {
                        let update = self.agenda_remote_update(&detail);
                        self.agenda_store.create_linked(detail.key.clone(), update, None);
                    }
                }
                self.agenda_save_and_refresh();
                self.jira_rerender_issues();
                self.set_message(format!("Added {count} issue(s) to the agenda"));
            }
            SyncPick::Transition(t) => {
                if let Some(id) = task {
                    self.agenda_enqueue_transition(id, t);
                    self.agenda_after_edit();
                }
            }
            SyncPick::StartBlocked => {
                if let Some(id) = task {
                    self.agenda_set_status(id, Status::Blocked);
                }
            }
            SyncPick::Blocked(blocked, transition) => {
                let Some(project) = project else { return };
                self.config.set_jira_blocked(&project, blocked.clone());
                if let Err(err) = self.config.save() {
                    self.set_error(format!("couldn't save settings.toml: {err}"));
                }
                let meaning = match &blocked {
                    JiraBlocked::Status { name, .. } => format!("moves the issue to \"{name}\""),
                    JiraBlocked::Flag => "flags the issue as an impediment".to_string(),
                    JiraBlocked::Local => "stays in your agenda only".to_string(),
                };
                if let Some(id) = task {
                    match (&blocked, transition) {
                        (JiraBlocked::Status { .. }, Some(t)) => self.agenda_enqueue_transition(id, t),
                        (JiraBlocked::Flag, _) => self.agenda_store.enqueue(id, OpKind::SetFlag(true)),
                        _ => {}
                    }
                    self.agenda_after_edit();
                }
                self.set_message(format!("Blocked in {project} now {meaning}"));
            }
            SyncPick::Priority(name) => {
                if let Some(id) = task {
                    let level = self.agenda_map_priority(&name);
                    self.agenda_store.set_priority(id, level);
                    self.agenda_store.enqueue(id, OpKind::SetPriority(name));
                    self.agenda_after_edit();
                }
            }
            SyncPick::Assignee(uid, name) => {
                if let Some(id) = task {
                    self.agenda_store.enqueue(id, OpKind::SetAssignee { id: uid, name });
                    self.agenda_after_edit();
                }
            }
        }
    }

    // -- Worklogs -----------------------------------------------------------

    /// `SPC a w`: the worklog review, fresh, over the Time tab.
    pub(crate) fn cmd_agenda_worklogs(&mut self) {
        let id = self.open_agenda_page(Some(crate::agenda_page::Tab::Time));
        if let Some(page) = self.agenda_page_mut(id) {
            page.open_worklogs();
        }
    }

    pub(super) fn agenda_worklog_round(&self) -> u32 {
        self.config.agenda_worklog_round.unwrap_or(15)
    }

    /// The review's rows: the store's batch minus drops, with edits.
    pub(super) fn agenda_worklog_rows(&self) -> Vec<WorklogRow> {
        let mut rows = self.agenda_store.worklog_batch(self.agenda_worklog_round());
        rows.retain(|r| !self.agenda_sync.worklog_dropped.contains(&(r.task, r.date)));
        for row in &mut rows {
            if let Some(&minutes) = self.agenda_sync.worklog_edits.get(&(row.task, row.date)) {
                row.minutes = minutes;
            }
        }
        rows
    }
    /// `W` on the review: sends every row, one worklog each, filed on the
    /// day the work happened.
    pub(super) fn agenda_send_worklogs(&mut self) {
        if self.agenda_sync.sending_worklogs {
            return;
        }
        let rows: Vec<WorklogRow> = self.agenda_worklog_rows().into_iter().filter(|r| r.minutes > 0).collect();
        if rows.is_empty() {
            self.set_message("Nothing to send");
            return;
        }
        self.agenda_sync.sending_worklogs = true;
        self.set_message("Sending worklogs...");
        let spawned = self.agenda_spawn(move |client| {
            let results = rows
                .into_iter()
                .map(|row| {
                    let started = row.started.format("%Y-%m-%dT%H:%M:%S%.3f%z").to_string();
                    let result = client.add_worklog_at(&row.key, row.minutes * 60, &started);
                    WorklogSent { task: row.task, entries: row.entries, key: row.key, minutes: row.minutes, result }
                })
                .collect();
            AgendaSyncEvent::WorklogsSent { results }
        });
        if !spawned {
            self.agenda_sync.sending_worklogs = false;
        }
    }

    fn apply_agenda_worklogs_sent(&mut self, results: Vec<WorklogSent>) {
        self.agenda_sync.sending_worklogs = false;
        let mut sent = 0;
        let mut minutes = 0;
        let mut failures = Vec::new();
        for WorklogSent { task, entries, key, minutes: row_minutes, result } in results {
            match result {
                Ok(()) => {
                    self.agenda_store.mark_sent(task, &entries);
                    self.agenda_sync.worklog_edits.retain(|(t, _), _| *t != task);
                    sent += 1;
                    minutes += row_minutes;
                }
                Err(err) => failures.push(format!("{key}: {err}")),
            }
        }
        self.agenda_save_and_refresh();
        if failures.is_empty() {
            self.set_message(format!("Sent {sent} worklog(s), {}", crate::agenda_page::format_minutes(minutes)));
        } else {
            self.set_error(format!("Sent {sent}, {} failed -- {}", failures.len(), failures.join("; ")));
        }
    }

    // -- Events -------------------------------------------------------------

    pub(super) fn apply_agenda_sync(&mut self, event: AgendaSyncEvent) {
        match event {
            AgendaSyncEvent::Pulled { keys, quiet, me, flag_field, result } => {
                if me.is_some() {
                    self.agenda_sync.me = me;
                }
                if flag_field.is_some() {
                    self.agenda_sync.flag_field = flag_field;
                }
                self.apply_agenda_pulled(keys, quiet, result);
            }
            AgendaSyncEvent::OpDone { op_id, key, what, flag_field, result } => {
                if flag_field.is_some() {
                    self.agenda_sync.flag_field = flag_field;
                }
                self.apply_agenda_op_done(op_id, key, what, result);
            }
            AgendaSyncEvent::Issues { purpose, result } => self.apply_agenda_issues(purpose, result),
            AgendaSyncEvent::Transitions { task, target, prev, result } => self.apply_agenda_transitions(task, target, prev, result),
            AgendaSyncEvent::Priorities { task, result } => match result {
                Ok(priorities) => {
                    let key = self.agenda_store.task(task).and_then(|t| t.jira_key()).unwrap_or_default().to_string();
                    let candidates =
                        priorities.into_iter().map(|p| fenix_picker::Candidate::new(p.name.clone(), SyncPick::Priority(p.name))).collect();
                    self.agenda_enter_sync_picker(
                        SyncPickerCtx { label: format!("{key} PRIORITY"), task: Some(task), project: None, revert: None },
                        candidates,
                    );
                }
                Err(err) => self.set_error(format!("couldn't read Jira's priorities: {err}")),
            },
            AgendaSyncEvent::ProjectStatuses { project, result } => self.apply_agenda_project_statuses(project, result),
            AgendaSyncEvent::WorklogsSent { results } => self.apply_agenda_worklogs_sent(results),
            AgendaSyncEvent::Tick => {
                // A dropped connection shouldn't need a manual retry once
                // it's back.
                let transient: Vec<TaskId> = self
                    .agenda_store
                    .tasks
                    .iter()
                    .filter(|t| t.jira.as_ref().and_then(|l| l.error.as_deref()).is_some_and(|e| e.starts_with("request failed")))
                    .map(|t| t.id)
                    .collect();
                for id in transient {
                    self.agenda_store.retry(id);
                }
                self.agenda_sync_now(true);
                self.agenda_drain_outbox();
            }
        }
        self.wake_caret();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fenix_agenda::Priority;

    /// An `App` whose agenda and config live in a fresh temp directory, so
    /// saving never touches the real ones.
    fn app() -> App {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("fenix-agenda-sync-test-{}-{n}", std::process::id()));
        // Process ids come round again: start from nothing.
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut app = App::with_file(None);
        app.agenda_store = fenix_agenda::AgendaStore::default();
        app.agenda_path = dir.join("agenda.json");
        app.config = fenix_config::Config::load_or_default(dir.join("settings.toml"));
        app
    }

    fn detail(key: &str, status: (&str, &str, &str)) -> fenix_jira::IssueDetail {
        fenix_jira::IssueDetail {
            key: key.to_string(),
            summary: format!("{key} summary"),
            description: Some("from Jira".to_string()),
            status_id: status.0.to_string(),
            status: status.1.to_string(),
            status_category: status.2.to_string(),
            priority: Some("Major".to_string()),
            ..Default::default()
        }
    }

    const OPEN: (&str, &str, &str) = ("1", "Open", "new");
    const IN_REVIEW: (&str, &str, &str) = ("5", "In Review", "indeterminate");

    fn transition(id: &str, to: (&str, &str, &str)) -> fenix_jira::Transition {
        fenix_jira::Transition {
            id: id.to_string(),
            name: format!("Go {}", to.1),
            to_id: to.0.to_string(),
            to_name: to.1.to_string(),
            to_category: to.2.to_string(),
        }
    }

    /// A task linked to `key`, in the column `status` maps to.
    fn linked(app: &mut App, key: &str, status: (&str, &str, &str)) -> TaskId {
        let update = app.agenda_remote_update(&detail(key, status));
        app.agenda_store.create_linked(key.to_string(), update, None)
    }

    fn last_error(app: &App) -> Option<String> {
        app.status_message.as_ref().filter(|m| m.is_error).map(|m| m.text.clone())
    }

    fn picker_labels(app: &App) -> Vec<String> {
        match &app.active_picker {
            Some(ActivePicker::WorkSync(state)) => state.visible_rows(0, 100).map(|(_, c)| c.label.clone()).collect(),
            _ => panic!("expected the WorkSync picker to be open"),
        }
    }

    fn confirm_row(app: &mut App, label_contains: &str) {
        let Some(ActivePicker::WorkSync(state)) = &mut app.active_picker else { panic!("no WorkSync picker") };
        for c in label_contains.chars() {
            state.push_char(c);
        }
        app.picker_confirm();
    }

    #[test]
    fn a_fetched_issue_maps_its_status_priority_and_blocked_status() {
        let mut app = app();
        app.config.set_jira_blocked("PROJ", JiraBlocked::Status { id: "9".to_string(), name: "On Hold".to_string() });
        app.config.jira_priority_map = vec![("Major".to_string(), "Urgent".to_string())];

        let update = app.agenda_remote_update(&detail("PROJ-1", IN_REVIEW));
        assert_eq!(update.status, Status::InProgress);
        assert_eq!(update.priority, Priority::Urgent, "the configured priority map wins over the guess");

        let update = app.agenda_remote_update(&detail("PROJ-1", ("9", "On Hold", "indeterminate")));
        assert_eq!(update.status, Status::Blocked, "the project's blocked status beats its category");

        app.config.set_jira_blocked("OPS", JiraBlocked::Flag);
        let mut flagged = detail("OPS-1", IN_REVIEW);
        flagged.flagged = true;
        assert_eq!(app.agenda_remote_update(&flagged).status, Status::Blocked);
    }

    #[test]
    fn a_move_within_the_same_jira_category_needs_no_transition() {
        let mut app = app();
        let id = linked(&mut app, "PROJ-1", IN_REVIEW);
        app.agenda_store.set_status(id, Status::Todo); // local-only drift

        app.agenda_set_status(id, Status::InProgress);

        assert_eq!(app.agenda_store.task(id).unwrap().status, Status::InProgress);
        assert!(app.agenda_store.outbox.is_empty());
        assert!(last_error(&app).is_none(), "no Jira call was needed, so no 'not configured' error either");
    }

    #[test]
    fn blocking_uses_the_projects_mapping_local_or_flag() {
        let mut app = app();
        app.config.set_jira_blocked("LOC", JiraBlocked::Local);
        app.config.set_jira_blocked("FLG", JiraBlocked::Flag);
        let local = linked(&mut app, "LOC-1", IN_REVIEW);
        let flag = linked(&mut app, "FLG-1", IN_REVIEW);

        app.agenda_set_status(local, Status::Blocked);
        app.agenda_set_status(flag, Status::Blocked);

        assert_eq!(app.agenda_store.task(local).unwrap().status, Status::Blocked);
        assert!(app.agenda_store.pending_for(local).next().is_none());
        assert_eq!(app.agenda_store.pending_for(flag).map(|op| op.kind.clone()).collect::<Vec<_>>(), vec![OpKind::SetFlag(true)]);
    }

    #[test]
    fn a_move_that_needs_jira_but_has_no_jira_configured_snaps_back() {
        let mut app = app();
        let id = linked(&mut app, "PROJ-1", OPEN);

        app.agenda_set_status(id, Status::Done);

        assert_eq!(app.agenda_store.task(id).unwrap().status, Status::Todo);
        assert!(last_error(&app).unwrap().contains("Jira isn't set up"));
    }

    #[test]
    fn a_single_matching_transition_is_queued_and_the_card_stays_moved() {
        let mut app = app();
        let id = linked(&mut app, "PROJ-1", IN_REVIEW);
        let prev = (Status::InProgress, 0);
        app.agenda_store.move_to_status(id, Status::Done);

        app.apply_agenda_sync(AgendaSyncEvent::Transitions {
            task: id,
            target: Some(Status::Done),
            prev,
            result: Ok(vec![transition("11", OPEN), transition("31", ("6", "Closed", "done"))]),
        });

        assert_eq!(app.agenda_store.task(id).unwrap().status, Status::Done);
        assert!(matches!(&app.agenda_store.outbox[0].kind, OpKind::Transition { id, to_name, .. } if id == "31" && to_name == "Closed"));
    }

    #[test]
    fn a_move_with_no_matching_transition_snaps_back_with_the_reason() {
        let mut app = app();
        let id = linked(&mut app, "PROJ-1", OPEN);
        app.agenda_store.move_to_status(id, Status::Done);

        app.apply_agenda_sync(AgendaSyncEvent::Transitions {
            task: id,
            target: Some(Status::Done),
            prev: (Status::Todo, 0),
            result: Ok(vec![transition("21", IN_REVIEW)]),
        });

        assert_eq!(app.agenda_store.task(id).unwrap().status, Status::Todo);
        assert!(app.agenda_store.outbox.is_empty());
        assert!(last_error(&app).unwrap().contains("no transition from \"Open\" to a Done status"));
    }

    #[test]
    fn several_matching_transitions_ask_and_cancelling_puts_the_card_back() {
        let mut app = app();
        let id = linked(&mut app, "PROJ-1", OPEN);
        app.agenda_store.move_to_status(id, Status::InProgress);

        app.apply_agenda_sync(AgendaSyncEvent::Transitions {
            task: id,
            target: Some(Status::InProgress),
            prev: (Status::Todo, 0),
            result: Ok(vec![transition("21", IN_REVIEW), transition("22", ("7", "Coding", "indeterminate"))]),
        });
        assert_eq!(picker_labels(&app).len(), 2);

        app.picker_cancel();

        assert_eq!(app.agenda_store.task(id).unwrap().status, Status::Todo);
        assert!(app.agenda_store.outbox.is_empty());
    }

    #[test]
    fn blocking_in_an_unmapped_project_asks_once_and_remembers_the_answer() {
        let mut app = app();
        let id = linked(&mut app, "PROJ-1", IN_REVIEW);
        app.agenda_store.move_to_status(id, Status::Blocked);

        app.apply_agenda_sync(AgendaSyncEvent::Transitions {
            task: id,
            target: Some(Status::Blocked),
            prev: (Status::InProgress, 0),
            result: Ok(vec![transition("41", ("9", "On Hold", "indeterminate")), transition("31", ("6", "Closed", "done"))]),
        });
        let labels = picker_labels(&app);
        assert!(labels.iter().any(|l| l.starts_with("On Hold")));
        assert!(labels.iter().any(|l| l.starts_with("Flag as impediment")));
        assert!(labels.iter().any(|l| l == "Keep Blocked local for PROJ"));

        confirm_row(&mut app, "On Hold");

        assert_eq!(app.config.jira_blocked_for("PROJ"), Some(&JiraBlocked::Status { id: "9".to_string(), name: "On Hold".to_string() }));
        let reloaded = fenix_config::Config::load(app.config.path().to_path_buf()).unwrap();
        assert!(reloaded.jira_blocked_for("PROJ").is_some(), "the choice is saved to settings.toml");
        assert_eq!(app.agenda_store.task(id).unwrap().status, Status::Blocked);
        assert!(matches!(&app.agenda_store.outbox[0].kind, OpKind::Transition { to_id, .. } if to_id == "9"));
    }

    #[test]
    fn a_blocked_status_that_isnt_reachable_snaps_back_and_offers_the_steps_that_are() {
        let mut app = app();
        app.config.set_jira_blocked("PROJ", JiraBlocked::Status { id: "9".to_string(), name: "On Hold".to_string() });
        let id = linked(&mut app, "PROJ-1", OPEN);
        app.agenda_store.move_to_status(id, Status::Blocked);

        app.apply_agenda_sync(AgendaSyncEvent::Transitions {
            task: id,
            target: Some(Status::Blocked),
            prev: (Status::Todo, 0),
            result: Ok(vec![transition("21", IN_REVIEW)]),
        });

        assert_eq!(app.agenda_store.task(id).unwrap().status, Status::Todo);
        assert!(last_error(&app).unwrap().contains("\"On Hold\" isn't reachable from \"Open\""));
        assert!(picker_labels(&app).iter().any(|l| l.contains("In Review")));
    }

    #[test]
    fn the_projects_pane_b_picker_lists_every_status_and_saves_the_choice() {
        let mut app = app();
        app.apply_agenda_sync(AgendaSyncEvent::ProjectStatuses {
            project: "OPS".to_string(),
            result: Ok(vec![
                fenix_jira::StatusInfo { id: "1".to_string(), name: "Open".to_string(), category: "new".to_string() },
                fenix_jira::StatusInfo { id: "8".to_string(), name: "Waiting".to_string(), category: "indeterminate".to_string() },
            ]),
        });
        assert_eq!(picker_labels(&app).len(), 4);

        confirm_row(&mut app, "Waiting");

        assert_eq!(app.config.jira_blocked_for("OPS"), Some(&JiraBlocked::Status { id: "8".to_string(), name: "Waiting".to_string() }));
    }

    #[test]
    fn linking_from_the_picker_takes_jiras_fields_and_marks_the_issue_in_the_jira_panel() {
        let mut app = app();
        let id = app.agenda_store.create_task("my title".to_string(), "".to_string(), Priority::Low, None);

        app.apply_agenda_sync(AgendaSyncEvent::Issues { purpose: IssueFetch::LinkPicker(id), result: Ok(vec![detail("PROJ-7", OPEN)]) });
        let labels = picker_labels(&app);
        assert_eq!(labels[0], "+ Create a new Jira issue from this task...");
        assert!(labels.iter().any(|l| l.starts_with("PROJ-7")));
        confirm_row(&mut app, "PROJ-7");

        let task = app.agenda_store.task(id).unwrap();
        assert_eq!(task.jira_key(), Some("PROJ-7"));
        assert_eq!(task.title, "PROJ-7 summary");
        assert!(app.agenda_linked_keys().contains("PROJ-7"));
    }

    #[test]
    fn linking_an_issue_already_linked_elsewhere_is_refused() {
        let mut app = app();
        linked(&mut app, "PROJ-7", OPEN);
        let id = app.agenda_store.create_task("other".to_string(), "".to_string(), Priority::Low, None);

        app.apply_agenda_sync(AgendaSyncEvent::Issues { purpose: IssueFetch::LinkKey(id), result: Ok(vec![detail("PROJ-7", OPEN)]) });

        assert!(app.agenda_store.task(id).unwrap().jira.is_none());
        assert!(last_error(&app).unwrap().contains("already linked"));
    }

    #[test]
    fn importing_adds_every_marked_issue_and_skips_ones_already_linked() {
        let mut app = app();
        linked(&mut app, "PROJ-1", OPEN);

        app.apply_agenda_sync(AgendaSyncEvent::Issues {
            purpose: IssueFetch::Import,
            result: Ok(vec![detail("PROJ-1", OPEN), detail("PROJ-2", OPEN), detail("PROJ-3", IN_REVIEW)]),
        });
        assert_eq!(picker_labels(&app).len(), 2, "PROJ-1 is already in the agenda");
        let Some(ActivePicker::WorkSync(state)) = &mut app.active_picker else { unreachable!() };
        state.toggle_mark();
        state.move_selection(1);
        state.toggle_mark();
        app.picker_confirm();

        assert!(app.agenda_store.find_by_key("PROJ-2").is_some());
        let review = app.agenda_store.find_by_key("PROJ-3").unwrap();
        assert_eq!(app.agenda_store.task(review).unwrap().status, Status::InProgress);
        assert_eq!(app.agenda_store.tasks.len(), 3);
    }

    #[test]
    fn t_on_the_jira_panel_adds_the_issue_and_starts_its_clock() {
        let mut app = app();
        app.apply_agenda_sync(AgendaSyncEvent::Issues { purpose: IssueFetch::AddFromPanel { clock: true }, result: Ok(vec![detail("PROJ-4", OPEN)]) });

        let id = app.agenda_store.find_by_key("PROJ-4").unwrap();
        assert_eq!(app.agenda_store.active_timer.as_ref().map(|t| t.task_id), Some(id));
    }

    #[test]
    fn a_pull_merges_changes_and_flags_issues_it_couldnt_find() {
        let mut app = app();
        let found = linked(&mut app, "PROJ-1", OPEN);
        let gone = linked(&mut app, "PROJ-2", OPEN);
        app.agenda_sync.pulling = true;

        let mut renamed = detail("PROJ-1", IN_REVIEW);
        renamed.summary = "Renamed in Jira".to_string();
        app.apply_agenda_sync(AgendaSyncEvent::Pulled {
            keys: vec!["PROJ-1".to_string(), "PROJ-2".to_string()],
            quiet: true,
            me: Some("me".to_string()),
            flag_field: None,
            result: Ok(vec![renamed]),
        });

        assert!(!app.agenda_sync.pulling);
        assert_eq!(app.agenda_sync.me.as_deref(), Some("me"));
        let task = app.agenda_store.task(found).unwrap();
        assert_eq!(task.title, "Renamed in Jira");
        assert_eq!(task.status, Status::InProgress);
        assert_eq!(app.agenda_store.task(gone).unwrap().jira.as_ref().unwrap().error.as_deref(), Some(NOT_FOUND));
    }

    #[test]
    fn a_sent_op_leaves_the_outbox_and_a_refused_one_stays_with_its_reason() {
        let mut app = app();
        let id = linked(&mut app, "PROJ-1", OPEN);
        app.agenda_store.enqueue(id, OpKind::SetSummary("a".to_string()));
        app.agenda_store.enqueue(id, OpKind::SetSummary("b".to_string()));
        let (first, second) = (app.agenda_store.outbox[0].id, app.agenda_store.outbox[1].id);

        app.agenda_sync.in_flight = Some(first);
        app.apply_agenda_sync(AgendaSyncEvent::OpDone {
            op_id: first,
            key: "PROJ-1".to_string(),
            what: "title updated".to_string(),
            flag_field: None,
            result: Ok(()),
        });
        assert_eq!(app.agenda_store.outbox.iter().map(|op| op.id).collect::<Vec<_>>(), vec![second]);

        app.agenda_sync.in_flight = Some(second);
        app.apply_agenda_sync(AgendaSyncEvent::OpDone {
            op_id: second,
            key: "PROJ-1".to_string(),
            what: "title updated".to_string(),
            flag_field: None,
            result: Err("HTTP 403".to_string()),
        });
        assert_eq!(app.agenda_store.outbox.len(), 1);
        assert_eq!(app.agenda_store.task(id).unwrap().jira.as_ref().unwrap().error.as_deref(), Some("HTTP 403"));
        assert!(last_error(&app).unwrap().contains("r on the task retries"));
    }

    #[test]
    fn editing_a_linked_tasks_description_queues_it_for_jira() {
        let mut app = app();
        let id = linked(&mut app, "PROJ-1", OPEN);
        app.open_compose_seeded(ComposePurpose::TaskDescription { task: id }, "from Jira");
        app.test_set_cursor(Cursor::at_start());
        app.test_insert_str("Updated: ");

        app.compose_submit();

        assert!(app.compose.is_none());
        assert_eq!(app.agenda_store.task(id).unwrap().description, "Updated: from Jira");
        assert_eq!(app.agenda_store.outbox[0].kind, OpKind::SetDescription("Updated: from Jira".to_string()));
    }

    #[test]
    fn a_comment_on_a_linked_task_is_queued_not_lost() {
        let mut app = app();
        let id = linked(&mut app, "PROJ-1", OPEN);
        app.open_compose_seeded(ComposePurpose::TaskComment { task: id }, "");
        app.test_set_cursor(Cursor::at_start());
        app.test_insert_str("On it");

        app.compose_submit();

        assert_eq!(app.agenda_store.outbox[0].kind, OpKind::AddComment("On it".to_string()));
    }

    #[test]
    fn the_worklog_review_applies_edits_and_drops_and_sending_marks_time_sent() {
        let mut app = app();
        let a = linked(&mut app, "PROJ-1", OPEN);
        let b = linked(&mut app, "PROJ-2", OPEN);
        app.agenda_store.log_manual_time(a, chrono::Duration::minutes(50));
        app.agenda_store.log_manual_time(b, chrono::Duration::minutes(20));
        let rows = app.agenda_worklog_rows();
        assert_eq!(rows.iter().map(|r| r.minutes).collect::<Vec<_>>(), vec![45, 15]);

        app.agenda_sync.worklog_edits.insert((a, rows[0].date), 60);
        app.agenda_sync.worklog_dropped.insert((b, rows[1].date));
        let rows = app.agenda_worklog_rows();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].minutes, 60);

        app.agenda_sync.sending_worklogs = true;
        app.apply_agenda_sync(AgendaSyncEvent::WorklogsSent { results: vec![WorklogSent { task: a, entries: rows[0].entries.clone(), key: "PROJ-1".to_string(), minutes: 60, result: Ok(()) }] });

        assert!(!app.agenda_sync.sending_worklogs);
        assert!(app.agenda_store.task(a).unwrap().time_entries[0].sent);
        assert_eq!(app.agenda_store.unsent_time().1, 1, "the dropped row is still unsent, just not in this batch");
    }

    #[test]
    fn the_worklog_view_renders_from_the_review_rows() {
        let mut app = app();
        let a = linked(&mut app, "PROJ-1", OPEN);
        app.agenda_store.log_manual_time(a, chrono::Duration::minutes(50));

        app.cmd_agenda_worklogs();

        let id = app.focused_buffer_id();
        let pane = app.focused_pane_id();
        app.ensure_page_layout(id, pane, 130);
        let popup = app.pages[&id].page.popup.as_ref().expect("the review").text();
        assert!(popup.contains("PROJ-1"));
        assert!(popup.contains("50m → 45m"), "{popup}");
    }

    #[test]
    fn deleting_a_linked_task_says_jira_isnt_affected() {
        let mut app = app();
        linked(&mut app, "PROJ-1", OPEN);
        app.open_agenda_page(Some(crate::agenda_page::Tab::List));
        app.page_key(KeyPress::char('D'));
        let id = app.focused_buffer_id();
        let Some(super::pages::PageModel::Agenda(page)) = app.pages.get(&id).map(|s| &s.model) else { panic!() };
        assert!(page.note.as_ref().unwrap().0.contains("PROJ-1 in Jira isn't touched"));
        app.page_key(KeyPress::char('D'));
        assert!(app.agenda_store.tasks.is_empty());
    }

    #[test]
    fn a_conflict_is_settled_from_its_picker() {
        let mut app = app();
        let id = linked(&mut app, "PROJ-1", OPEN);
        app.agenda_store.set_title(id, "Mine".to_string());
        app.agenda_store.enqueue(id, OpKind::SetSummary("Mine".to_string()));
        let mut theirs = detail("PROJ-1", OPEN);
        theirs.summary = "Theirs".to_string();
        let update = app.agenda_remote_update(&theirs);
        app.agenda_store.apply_remote(id, update);

        let page = app.open_agenda_page(Some(crate::agenda_page::Tab::List));
        app.page_key(KeyPress::named(FenixNamedKey::Enter));
        // The conflict is the row after the fields.
        for _ in 0..5 {
            app.page_key(KeyPress::char('j'));
        }
        app.page_key(KeyPress::named(FenixNamedKey::Enter));
        let pane = app.focused_pane_id();
        app.ensure_page_layout(page, pane, 130);
        let menu = app.pages[&page].page.popup.as_ref().expect("keep or take").text();
        assert!(menu.contains("Keep mine: Mine") && menu.contains("Take Jira's: Theirs"), "{menu}");
        app.page_key(KeyPress::char('2'));

        assert_eq!(app.agenda_store.task(id).unwrap().title, "Theirs");
        assert!(app.agenda_store.outbox.is_empty());
    }

    #[test]
    fn adding_an_issue_to_the_agenda_keeps_the_issues_cursor_where_it_was() {
        let mut app = app();
        app.open_jira_panel();
        let session = app.jira_session.as_mut().unwrap();
        session.issues = (1..=3)
            .map(|n| fenix_jira::IssueSummary { key: format!("PROJ-{n}"), summary: "s".to_string(), status: "Open".to_string(), ..Default::default() })
            .collect();
        let (issues_pane, issues_buffer) = (session.issues_pane, session.issues_buffer);
        app.jira_rerender_issues();
        app.windows_mut().focus(issues_pane);
        let third = app.buffers.get(issues_buffer).unwrap().buffer.line_start_char(2);
        app.test_set_cursor(Cursor { char_idx: third, sticky_col: 0 });

        app.apply_agenda_sync(AgendaSyncEvent::Issues { purpose: IssueFetch::AddFromPanel { clock: false }, result: Ok(vec![detail("PROJ-3", OPEN)]) });

        let ob = app.buffers.get(issues_buffer).unwrap();
        assert_eq!(ob.buffer.line_col(&app.pane_state(issues_pane).cursor).0, 2);
        assert!(ob.buffer.text().lines().nth(2).unwrap().ends_with("· in agenda"));
    }
}
