//! `AgendaStore`'s Jira side: linking, the outbox of pending changes,
//! merging a fetched issue in, settling conflicts, and batching unsent
//! time into worklogs. See `crate::jira` for the model.

use chrono::{DateTime, Local, NaiveDate};

use crate::jira::{Conflict, JiraLink, OpKind, PendingOp, RemoteUpdate, SyncField};
use crate::store::AgendaStore;
use crate::task::{Status, TaskId};
#[cfg(test)]
use crate::task::Priority;

/// One worklog the review would send: every unsent entry on one linked
/// task on one day, added up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorklogRow {
    pub task: TaskId,
    pub key: String,
    pub title: String,
    pub date: NaiveDate,
    /// When the day's first entry started -- what Jira files it under.
    pub started: DateTime<Local>,
    /// What was actually tracked, in minutes.
    pub actual_minutes: i64,
    /// What will be sent: `actual_minutes` rounded, unless edited.
    pub minutes: i64,
    /// Indices into the task's `time_entries` this row covers.
    pub entries: Vec<usize>,
}

impl AgendaStore {
    pub fn find_by_key(&self, key: &str) -> Option<TaskId> {
        self.tasks.iter().find(|t| t.jira_key() == Some(key)).map(|t| t.id)
    }

    /// Links `id` to `key`. Jira wins for everything the two share: its
    /// summary, description, status and priority replace the task's own.
    /// A local description that would be lost is kept as a note, so
    /// linking never throws anything away.
    pub fn link(&mut self, id: TaskId, key: String, update: RemoteUpdate) {
        let Some(task) = self.task(id) else { return };
        let old_description = task.description.trim().to_string();
        if !old_description.is_empty() && old_description != update.snapshot.description.trim() {
            self.add_note(id, format!("Local description before linking to {key}:\n{old_description}"));
        }
        self.outbox.retain(|op| op.task != id);
        let now = Local::now();
        if let Some(task) = self.task_mut(id) {
            task.title = update.snapshot.summary.clone();
            task.description = update.snapshot.description.clone();
            task.priority = update.priority;
            task.updated_at = now;
        }
        if self.task(id).is_some_and(|t| t.status != update.status) {
            self.move_to_status(id, update.status);
        }
        if let Some(task) = self.task_mut(id) {
            task.jira = Some(JiraLink {
                key,
                base: update.snapshot,
                last_synced: Some(now),
                error: None,
                conflicts: Vec::new(),
                not_mine: update.mine == Some(false),
                resolving: false,
            });
        }
    }

    /// A new task tracking `key`, filed under `category`.
    pub fn create_linked(&mut self, key: String, update: RemoteUpdate, category: Option<String>) -> TaskId {
        let id = self.create_task(update.snapshot.summary.clone(), String::new(), update.priority, category);
        self.link(id, key, update);
        id
    }

    /// Back to a local-only task: keeps everything it has now, forgets the
    /// issue and anything still waiting to be sent to it.
    pub fn unlink(&mut self, id: TaskId) {
        self.outbox.retain(|op| op.task != id);
        if let Some(task) = self.task_mut(id) {
            task.jira = None;
            task.updated_at = Local::now();
        }
    }

    /// Queues `kind` for a linked task. A no-op on a local task, so
    /// callers can enqueue after every edit without checking first.
    pub fn enqueue(&mut self, id: TaskId, kind: OpKind) {
        if self.task(id).and_then(|t| t.jira.as_ref()).is_none() {
            return;
        }
        let op_id = self.next_op_id;
        self.next_op_id += 1;
        self.outbox.push(PendingOp { id: op_id, task: id, kind });
    }

    pub fn pending_for(&self, id: TaskId) -> impl Iterator<Item = &PendingOp> {
        self.outbox.iter().filter(move |op| op.task == id)
    }

    /// The next op worth sending: oldest first, skipping any held by a
    /// conflict on its field and any task whose last push failed (those
    /// wait for a retry, so one bad op doesn't spin forever).
    pub fn next_op(&self) -> Option<&PendingOp> {
        self.outbox.iter().find(|op| {
            let Some(link) = self.task(op.task).and_then(|t| t.jira.as_ref()) else { return false };
            if link.error.is_some() {
                return false;
            }
            match op.kind.field() {
                Some(field) => !link.conflicts.iter().any(|c| c.field == field),
                None => true,
            }
        })
    }

    /// Jira accepted op `op_id`: drop it and fold it into the base.
    pub fn op_succeeded(&mut self, op_id: u64) {
        let Some(pos) = self.outbox.iter().position(|op| op.id == op_id) else { return };
        let op = self.outbox.remove(pos);
        if let Some(link) = self.task_mut(op.task).and_then(|t| t.jira.as_mut()) {
            op.kind.apply_to(&mut link.base);
            link.error = None;
        }
    }

    /// Jira refused op `op_id`: keep it, and say why on its task.
    pub fn op_failed(&mut self, op_id: u64, error: String) {
        let Some(task) = self.outbox.iter().find(|op| op.id == op_id).map(|op| op.task) else { return };
        if let Some(link) = self.task_mut(task).and_then(|t| t.jira.as_mut()) {
            link.error = Some(error);
        }
    }

    /// Drops op `op_id` without sending it -- a status change whose
    /// transition turned out not to exist, say.
    pub fn discard_op(&mut self, op_id: u64) {
        self.outbox.retain(|op| op.id != op_id);
    }

    /// Clears a task's push error so its ops are tried again.
    pub fn retry(&mut self, id: TaskId) {
        if let Some(link) = self.task_mut(id).and_then(|t| t.jira.as_mut()) {
            link.error = None;
        }
    }

    pub fn set_link_error(&mut self, id: TaskId, error: Option<String>) {
        if let Some(link) = self.task_mut(id).and_then(|t| t.jira.as_mut()) {
            link.error = error;
        }
    }

    pub fn set_resolving(&mut self, id: TaskId, resolving: bool) {
        if let Some(link) = self.task_mut(id).and_then(|t| t.jira.as_mut()) {
            link.resolving = resolving;
        }
    }

    /// Puts a card back where it was -- for a board move Jira refused.
    pub fn restore_status(&mut self, id: TaskId, status: Status, order: i64) {
        self.set_status(id, status);
        if let Some(task) = self.task_mut(id) {
            task.order = order;
        }
    }

    /// Merges a freshly fetched issue into its task, field by field (see
    /// the module docs on `crate::jira`).
    pub fn apply_remote(&mut self, id: TaskId, update: RemoteUpdate) {
        let Some(link) = self.task(id).and_then(|t| t.jira.as_ref()) else { return };
        let base = link.base.clone();
        let theirs = &update.snapshot;
        let changed = |field: SyncField| match field {
            SyncField::Title => theirs.summary != base.summary,
            SyncField::Description => theirs.description != base.description,
            SyncField::Status => theirs.status_id != base.status_id || theirs.flagged != base.flagged,
            SyncField::Priority => theirs.priority != base.priority,
        };
        let pending: Vec<SyncField> = self.pending_for(id).filter_map(|op| op.kind.field()).collect();

        let mut new_conflicts = Vec::new();
        let mut take = Vec::new();
        for field in [SyncField::Title, SyncField::Description, SyncField::Status, SyncField::Priority] {
            if !changed(field) {
                continue;
            }
            if pending.contains(&field) {
                new_conflicts.push(Conflict { field, theirs: update.clone() });
            } else {
                take.push(field);
            }
        }
        for field in take {
            self.take_theirs(id, field, &update);
        }

        let now = Local::now();
        if let Some(link) = self.task_mut(id).and_then(|t| t.jira.as_mut()) {
            // Everything that never conflicts just follows Jira.
            link.base.comments = update.snapshot.comments.clone();
            link.base.assignee = update.snapshot.assignee.clone();
            link.base.assignee_id = update.snapshot.assignee_id.clone();
            link.base.updated = update.snapshot.updated.clone();
            if link.base.status_id == update.snapshot.status_id {
                link.base.status_name = update.snapshot.status_name.clone();
                link.base.status_category = update.snapshot.status_category.clone();
            }
            if let Some(mine) = update.mine {
                link.not_mine = !mine;
            }
            for conflict in new_conflicts {
                link.conflicts.retain(|c| c.field != conflict.field);
                link.conflicts.push(conflict);
            }
            link.last_synced = Some(now);
        }
    }

    /// Settles a conflict: `keep_mine` sends your value over Jira's (the
    /// held op goes through), otherwise Jira's value replaces yours and
    /// your pending change for that field is dropped.
    pub fn resolve_conflict(&mut self, id: TaskId, field: SyncField, keep_mine: bool) {
        let Some(conflict) = self
            .task(id)
            .and_then(|t| t.jira.as_ref())
            .and_then(|link| link.conflicts.iter().find(|c| c.field == field))
            .cloned()
        else {
            return;
        };
        if let Some(link) = self.task_mut(id).and_then(|t| t.jira.as_mut()) {
            link.conflicts.retain(|c| c.field != field);
        }
        if keep_mine {
            // Their value becomes the base, so our op is the only change.
            if let Some(link) = self.task_mut(id).and_then(|t| t.jira.as_mut()) {
                copy_field(field, &conflict.theirs, &mut link.base);
            }
        } else {
            self.outbox.retain(|op| !(op.task == id && op.kind.field() == Some(field)));
            self.take_theirs(id, field, &conflict.theirs);
        }
    }

    /// Jira's value for `field` becomes both the task's own and the base.
    fn take_theirs(&mut self, id: TaskId, field: SyncField, update: &RemoteUpdate) {
        let theirs = &update.snapshot;
        match field {
            SyncField::Title => self.set_title(id, theirs.summary.clone()),
            SyncField::Description => self.set_description(id, theirs.description.clone()),
            SyncField::Priority => self.set_priority(id, update.priority),
            SyncField::Status => {
                if self.task(id).is_some_and(|t| t.status != update.status) {
                    self.move_to_status(id, update.status);
                }
            }
        }
        if let Some(link) = self.task_mut(id).and_then(|t| t.jira.as_mut()) {
            copy_field(field, update, &mut link.base);
        }
    }

    /// Every unsent day of time on a linked task, rounded to `round`
    /// minutes -- what the worklog review shows. The running timer isn't a
    /// time entry yet, so it's never included.
    pub fn worklog_batch(&self, round: u32) -> Vec<WorklogRow> {
        let mut rows: Vec<WorklogRow> = Vec::new();
        for task in &self.tasks {
            let Some(key) = task.jira_key() else { continue };
            for (i, entry) in task.time_entries.iter().enumerate() {
                if entry.sent {
                    continue;
                }
                let date = entry.start.date_naive();
                let minutes = entry.duration().num_minutes().max(0);
                match rows.iter_mut().find(|r| r.task == task.id && r.date == date) {
                    Some(row) => {
                        row.actual_minutes += minutes;
                        row.started = row.started.min(entry.start);
                        row.entries.push(i);
                    }
                    None => rows.push(WorklogRow {
                        task: task.id,
                        key: key.to_string(),
                        title: task.title.clone(),
                        date,
                        started: entry.start,
                        actual_minutes: minutes,
                        minutes: 0,
                        entries: vec![i],
                    }),
                }
            }
        }
        for row in &mut rows {
            row.minutes = crate::jira::round_minutes(row.actual_minutes, round);
        }
        rows.sort_by(|a, b| a.date.cmp(&b.date).then_with(|| a.key.cmp(&b.key)));
        rows
    }

    /// Marks `entries` of `id` as sent (or dismissed).
    pub fn mark_sent(&mut self, id: TaskId, entries: &[usize]) {
        if let Some(task) = self.task_mut(id) {
            for &i in entries {
                if let Some(entry) = task.time_entries.get_mut(i) {
                    entry.sent = true;
                }
            }
        }
    }

    /// Total unsent time on linked tasks and how many issues it spans.
    pub fn unsent_time(&self) -> (chrono::Duration, usize) {
        let mut total = chrono::Duration::zero();
        let mut issues = 0;
        for task in self.tasks.iter().filter(|t| t.jira.is_some()) {
            let unsent: chrono::Duration =
                task.time_entries.iter().filter(|e| !e.sent).fold(chrono::Duration::zero(), |acc, e| acc + e.duration());
            if unsent > chrono::Duration::zero() {
                total += unsent;
                issues += 1;
            }
        }
        (total, issues)
    }
}

fn copy_field(field: SyncField, from: &RemoteUpdate, base: &mut crate::jira::RemoteSnapshot) {
    let theirs = &from.snapshot;
    match field {
        SyncField::Title => base.summary = theirs.summary.clone(),
        SyncField::Description => base.description = theirs.description.clone(),
        SyncField::Priority => base.priority = theirs.priority.clone(),
        SyncField::Status => {
            base.status_id = theirs.status_id.clone();
            base.status_name = theirs.status_name.clone();
            base.status_category = theirs.status_category.clone();
            base.flagged = theirs.flagged;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jira::RemoteSnapshot;

    fn snapshot(summary: &str) -> RemoteSnapshot {
        RemoteSnapshot {
            summary: summary.to_string(),
            description: "remote description".to_string(),
            status_id: "1".to_string(),
            status_name: "Open".to_string(),
            status_category: "new".to_string(),
            priority: Some("Major".to_string()),
            ..Default::default()
        }
    }

    fn update(snapshot: RemoteSnapshot) -> RemoteUpdate {
        RemoteUpdate { snapshot, status: Status::Todo, priority: Priority::High, mine: Some(true) }
    }

    fn linked() -> (AgendaStore, TaskId) {
        let mut store = AgendaStore::default();
        let id = store.create_task("local".to_string(), "".to_string(), Priority::Low, None);
        store.link(id, "PROJ-1".to_string(), update(snapshot("Fix it")));
        (store, id)
    }

    #[test]
    fn linking_takes_jiras_fields_and_keeps_a_differing_local_description_as_a_note() {
        let mut store = AgendaStore::default();
        let id = store.create_task("local".to_string(), "my own notes".to_string(), Priority::Low, None);
        store.link(id, "PROJ-1".to_string(), update(snapshot("Fix it")));

        let task = store.task(id).unwrap();
        assert_eq!(task.title, "Fix it");
        assert_eq!(task.description, "remote description");
        assert_eq!(task.priority, Priority::High);
        assert_eq!(task.jira_key(), Some("PROJ-1"));
        assert!(task.notes[0].text.contains("my own notes"));
        assert_eq!(store.find_by_key("PROJ-1"), Some(id));
    }

    #[test]
    fn enqueue_is_a_no_op_on_a_local_task() {
        let mut store = AgendaStore::default();
        let id = store.create_task("local".to_string(), "".to_string(), Priority::Low, None);
        store.enqueue(id, OpKind::SetSummary("x".to_string()));
        assert!(store.outbox.is_empty());
    }

    #[test]
    fn a_remote_change_to_an_untouched_field_is_taken() {
        let (mut store, id) = linked();
        store.apply_remote(id, update(snapshot("Renamed in Jira")));
        assert_eq!(store.task(id).unwrap().title, "Renamed in Jira");
        assert!(store.task(id).unwrap().jira.as_ref().unwrap().conflicts.is_empty());
    }

    #[test]
    fn a_pending_local_change_with_no_remote_change_survives_a_pull() {
        let (mut store, id) = linked();
        store.set_title(id, "Mine".to_string());
        store.enqueue(id, OpKind::SetSummary("Mine".to_string()));
        store.apply_remote(id, update(snapshot("Fix it")));
        assert_eq!(store.task(id).unwrap().title, "Mine");
        assert_eq!(store.next_op().map(|op| op.kind.clone()), Some(OpKind::SetSummary("Mine".to_string())));
    }

    #[test]
    fn both_sides_changing_a_field_is_a_conflict_that_holds_the_op() {
        let (mut store, id) = linked();
        store.set_title(id, "Mine".to_string());
        store.enqueue(id, OpKind::SetSummary("Mine".to_string()));
        store.apply_remote(id, update(snapshot("Theirs")));

        let link = store.task(id).unwrap().jira.as_ref().unwrap();
        assert_eq!(link.conflicts.len(), 1);
        assert_eq!(link.conflicts[0].their_value(), "Theirs");
        assert_eq!(store.task(id).unwrap().title, "Mine", "a conflict never silently overwrites yours");
        assert!(store.next_op().is_none(), "the conflicting op waits for a decision");
    }

    #[test]
    fn keeping_mine_releases_the_op_and_taking_theirs_drops_it() {
        let (mut store, id) = linked();
        store.set_title(id, "Mine".to_string());
        store.enqueue(id, OpKind::SetSummary("Mine".to_string()));
        store.apply_remote(id, update(snapshot("Theirs")));
        let mut theirs_store = store.clone();

        store.resolve_conflict(id, SyncField::Title, true);
        assert_eq!(store.task(id).unwrap().title, "Mine");
        assert!(store.next_op().is_some());
        assert_eq!(store.task(id).unwrap().jira.as_ref().unwrap().base.summary, "Theirs");

        theirs_store.resolve_conflict(id, SyncField::Title, false);
        assert_eq!(theirs_store.task(id).unwrap().title, "Theirs");
        assert!(theirs_store.outbox.is_empty());
    }

    #[test]
    fn a_successful_op_updates_the_base_so_the_next_pull_sees_no_change() {
        let (mut store, id) = linked();
        store.set_title(id, "Mine".to_string());
        store.enqueue(id, OpKind::SetSummary("Mine".to_string()));
        let op = store.next_op().unwrap().id;
        store.op_succeeded(op);

        store.apply_remote(id, update(snapshot("Mine")));
        assert_eq!(store.task(id).unwrap().title, "Mine");
        assert!(store.outbox.is_empty());
    }

    #[test]
    fn a_failed_op_is_kept_and_blocks_its_task_until_retried() {
        let (mut store, id) = linked();
        store.enqueue(id, OpKind::AddComment("hi".to_string()));
        let op = store.next_op().unwrap().id;
        store.op_failed(op, "HTTP 403".to_string());
        assert!(store.next_op().is_none());
        assert_eq!(store.task(id).unwrap().jira.as_ref().unwrap().error.as_deref(), Some("HTTP 403"));

        store.retry(id);
        assert_eq!(store.next_op().map(|o| o.id), Some(op));
    }

    #[test]
    fn a_remote_status_change_moves_the_card() {
        let (mut store, id) = linked();
        let mut done = snapshot("Fix it");
        done.status_id = "3".to_string();
        done.status_name = "Closed".to_string();
        done.status_category = "done".to_string();
        store.apply_remote(id, RemoteUpdate { snapshot: done, status: Status::Done, priority: Priority::High, mine: Some(false) });

        let task = store.task(id).unwrap();
        assert_eq!(task.status, Status::Done);
        assert!(task.jira.as_ref().unwrap().not_mine);
    }

    #[test]
    fn unlinking_keeps_the_task_and_drops_its_pending_ops() {
        let (mut store, id) = linked();
        store.enqueue(id, OpKind::AddComment("hi".to_string()));
        store.unlink(id);
        assert!(store.task(id).unwrap().jira.is_none());
        assert_eq!(store.task(id).unwrap().title, "Fix it");
        assert!(store.outbox.is_empty());
    }

    #[test]
    fn the_worklog_batch_groups_unsent_linked_time_by_task_and_day() {
        let (mut store, id) = linked();
        let local = store.create_task("local".to_string(), "".to_string(), Priority::Low, None);
        store.log_manual_time(id, chrono::Duration::minutes(20));
        store.log_manual_time(id, chrono::Duration::minutes(50));
        store.log_manual_time(local, chrono::Duration::minutes(90));

        let rows = store.worklog_batch(15);
        assert_eq!(rows.len(), 1, "local tasks never produce worklogs");
        assert_eq!(rows[0].key, "PROJ-1");
        assert_eq!(rows[0].actual_minutes, 70);
        assert_eq!(rows[0].minutes, 75);
        assert_eq!(rows[0].entries, vec![0, 1]);
        assert_eq!(store.unsent_time(), (chrono::Duration::minutes(70), 1));

        store.mark_sent(id, &rows[0].entries);
        assert!(store.worklog_batch(15).is_empty());
        assert_eq!(store.unsent_time().1, 0);
    }

    #[test]
    fn the_outbox_survives_a_save_and_load() {
        let (mut store, id) = linked();
        store.enqueue(id, OpKind::AddComment("hi".to_string()));
        let json = serde_json::to_string(&store).unwrap();
        let reloaded: AgendaStore = serde_json::from_str(&json).unwrap();
        assert_eq!(reloaded.outbox.len(), 1);
        assert_eq!(reloaded.task(id).unwrap().jira_key(), Some("PROJ-1"));
    }

    #[test]
    fn an_agenda_file_from_before_jira_links_still_loads() {
        let json = r#"{"tasks":[{"id":0,"title":"Old","description":"","status":"Todo","priority":"Low","category":null,
            "notes":[],"time_entries":[{"start":"2026-01-01T10:00:00+00:00","end":"2026-01-01T11:00:00+00:00","source":"Timer"}],
            "subtasks":[],"depends_on":[],"created_at":"2026-01-01T10:00:00+00:00","updated_at":"2026-01-01T10:00:00+00:00",
            "done_at":null,"archived":false,"order":0}],"next_id":1,"active_timer":null}"#;
        let store: AgendaStore = serde_json::from_str(json).unwrap();
        assert!(store.tasks[0].jira.is_none());
        assert!(!store.tasks[0].time_entries[0].sent);
        assert!(store.outbox.is_empty());
    }
}
