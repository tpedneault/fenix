//! A task's optional link to a Jira issue, and the pieces that keep the two
//! in step -- still no network code: `fenix-gui` fetches issues, turns them
//! into a `RemoteUpdate`, and sends what's in `AgendaStore::outbox`. What
//! lives here is the part worth unit-testing on its own: which side wins
//! when both changed.
//!
//! Sync is field-by-field against `JiraLink::base`, the last Jira state
//! this task agreed with. A field Jira changed and you didn't takes Jira's
//! value; a field you changed (an op still in the outbox) and Jira didn't
//! gets pushed; a field both changed becomes a `Conflict` you settle by
//! hand, and its op is held until you do.

use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};

use crate::task::{Priority, Status, TaskId};

/// One comment as Jira has it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteComment {
    pub id: String,
    pub author: String,
    pub body: String,
    /// Jira's own timestamp string, kept verbatim.
    pub created: String,
}

/// The Jira side of a linked task, as of one fetch.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteSnapshot {
    pub summary: String,
    pub description: String,
    pub status_id: String,
    pub status_name: String,
    /// `"new"`, `"indeterminate"` or `"done"`.
    pub status_category: String,
    pub priority: Option<String>,
    pub assignee: Option<String>,
    pub assignee_id: Option<String>,
    pub flagged: bool,
    pub updated: String,
    pub comments: Vec<RemoteComment>,
}

/// A fetched issue plus what it means for the agenda's own fields -- the
/// caller does the mapping, since it depends on config (which status is
/// "Blocked" in this project, how priority names map) this crate doesn't
/// see.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteUpdate {
    pub snapshot: RemoteSnapshot,
    pub status: Status,
    pub priority: Priority,
    /// Whether the issue is still assigned to you; `None` when unknown.
    pub mine: Option<bool>,
}

/// The fields that sync both ways, and so can conflict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SyncField {
    Title,
    Description,
    Status,
    Priority,
}

impl SyncField {
    pub fn label(self) -> &'static str {
        match self {
            SyncField::Title => "title",
            SyncField::Description => "description",
            SyncField::Status => "status",
            SyncField::Priority => "priority",
        }
    }
}

/// Both sides changed `field` since the last sync. `theirs` is what Jira
/// had when that was noticed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Conflict {
    pub field: SyncField,
    pub theirs: RemoteUpdate,
}

impl Conflict {
    /// Jira's value for the conflicting field, for display.
    pub fn their_value(&self) -> String {
        let s = &self.theirs.snapshot;
        match self.field {
            SyncField::Title => s.summary.clone(),
            SyncField::Description => s.description.clone(),
            SyncField::Status => s.status_name.clone(),
            SyncField::Priority => s.priority.clone().unwrap_or_default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JiraLink {
    pub key: String,
    /// The last Jira state this task agreed with -- what "changed" is
    /// measured against on both sides.
    pub base: RemoteSnapshot,
    pub last_synced: Option<DateTime<Local>>,
    /// Why the last push failed; cleared by the next success or a retry.
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub conflicts: Vec<Conflict>,
    /// Jira reassigned the issue to someone else. The task stays; this
    /// just says so.
    #[serde(default)]
    pub not_mine: bool,
    /// A status change is waiting on its transitions fetch -- shown as
    /// pending, never saved.
    #[serde(skip)]
    pub resolving: bool,
}

impl JiraLink {
    pub fn project(&self) -> &str {
        self.key.rsplit_once('-').map_or(self.key.as_str(), |(project, _)| project)
    }
}

/// One change waiting to be sent to Jira.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingOp {
    pub id: u64,
    pub task: TaskId,
    pub kind: OpKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum OpKind {
    SetSummary(String),
    SetDescription(String),
    /// A Jira priority *name*.
    SetPriority(String),
    AddComment(String),
    SetAssignee { id: String, name: String },
    /// A workflow transition, already chosen, with where it lands.
    Transition { id: String, to_id: String, to_name: String, to_category: String },
    SetFlag(bool),
}

impl OpKind {
    /// Which conflict-checked field this op writes, if any.
    pub fn field(&self) -> Option<SyncField> {
        match self {
            OpKind::SetSummary(_) => Some(SyncField::Title),
            OpKind::SetDescription(_) => Some(SyncField::Description),
            OpKind::SetPriority(_) => Some(SyncField::Priority),
            OpKind::Transition { .. } | OpKind::SetFlag(_) => Some(SyncField::Status),
            OpKind::AddComment(_) | OpKind::SetAssignee { .. } => None,
        }
    }

    /// A short past-tense description for status messages.
    pub fn describe(&self) -> String {
        match self {
            OpKind::SetSummary(_) => "title updated".to_string(),
            OpKind::SetDescription(_) => "description updated".to_string(),
            OpKind::SetPriority(name) => format!("priority set to {name}"),
            OpKind::AddComment(_) => "comment posted".to_string(),
            OpKind::SetAssignee { name, .. } => format!("assigned to {name}"),
            OpKind::Transition { to_name, .. } => format!("moved to {to_name}"),
            OpKind::SetFlag(true) => "flagged".to_string(),
            OpKind::SetFlag(false) => "unflagged".to_string(),
        }
    }

    /// Folds a successful op into the base snapshot, so the next fetch
    /// doesn't mistake our own change for Jira's.
    pub(crate) fn apply_to(&self, base: &mut RemoteSnapshot) {
        match self {
            OpKind::SetSummary(s) => base.summary = s.clone(),
            OpKind::SetDescription(s) => base.description = s.clone(),
            OpKind::SetPriority(p) => base.priority = Some(p.clone()),
            OpKind::AddComment(_) => {}
            OpKind::SetAssignee { id, name } => {
                base.assignee_id = Some(id.clone());
                base.assignee = Some(name.clone());
            }
            OpKind::Transition { to_id, to_name, to_category, .. } => {
                base.status_id = to_id.clone();
                base.status_name = to_name.clone();
                base.status_category = to_category.clone();
            }
            OpKind::SetFlag(on) => base.flagged = *on,
        }
    }
}

/// Todo/In Progress/Done from Jira's status category -- Blocked is never
/// a category, the caller decides that one from project config.
pub fn status_for_category(category: &str) -> Status {
    match category {
        "done" => Status::Done,
        "indeterminate" => Status::InProgress,
        _ => Status::Todo,
    }
}

/// The category a board column moves an issue into -- `None` for
/// Blocked, which every project spells differently.
pub fn category_for_status(status: Status) -> Option<&'static str> {
    match status {
        Status::Todo => Some("new"),
        Status::InProgress => Some("indeterminate"),
        Status::Done => Some("done"),
        Status::Blocked => None,
    }
}

/// A best guess at which agenda level a Jira priority name means, for
/// instances that don't configure one -- the usual Blocker/Critical/Major/
/// Minor/Trivial and Highest/High/Medium/Low/Lowest schemes both land
/// where you'd expect.
pub fn guess_priority(name: &str) -> Priority {
    let lower = name.to_lowercase();
    if ["highest", "blocker", "critical", "urgent"].iter().any(|w| lower.contains(w)) {
        Priority::Urgent
    } else if lower.contains("high") || lower.contains("major") {
        Priority::High
    } else if ["low", "minor", "trivial"].iter().any(|w| lower.contains(w)) {
        Priority::Low
    } else {
        Priority::Medium
    }
}

/// Rounds a worklog to the nearest `round` minutes, never down to zero for
/// time that was actually worked.
pub fn round_minutes(minutes: i64, round: u32) -> i64 {
    let round = i64::from(round.max(1));
    if minutes <= 0 {
        return 0;
    }
    (((minutes + round / 2) / round) * round).max(round)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guess_priority_covers_both_common_schemes() {
        assert_eq!(guess_priority("Highest"), Priority::Urgent);
        assert_eq!(guess_priority("Blocker"), Priority::Urgent);
        assert_eq!(guess_priority("High"), Priority::High);
        assert_eq!(guess_priority("Major"), Priority::High);
        assert_eq!(guess_priority("Medium"), Priority::Medium);
        assert_eq!(guess_priority("Lowest"), Priority::Low);
        assert_eq!(guess_priority("Trivial"), Priority::Low);
        assert_eq!(guess_priority("P3 - whatever"), Priority::Medium);
    }

    #[test]
    fn rounding_goes_to_the_nearest_unit_but_never_to_zero() {
        assert_eq!(round_minutes(7, 15), 15);
        assert_eq!(round_minutes(22, 15), 15);
        assert_eq!(round_minutes(23, 15), 30);
        assert_eq!(round_minutes(80, 15), 75);
        assert_eq!(round_minutes(83, 15), 90);
        assert_eq!(round_minutes(83, 1), 83);
        assert_eq!(round_minutes(0, 15), 0);
    }

    #[test]
    fn categories_map_to_columns_and_back() {
        for status in [Status::Todo, Status::InProgress, Status::Done] {
            assert_eq!(status_for_category(category_for_status(status).unwrap()), status);
        }
        assert_eq!(category_for_status(Status::Blocked), None);
    }
}
