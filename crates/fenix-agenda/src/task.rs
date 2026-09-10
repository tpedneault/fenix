use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};

/// Identity of one task, stable for as long as it exists (including once
/// archived -- only `AgendaStore::delete` ever frees an id, and even then
/// `next_id` is never reused, so a dangling `depends_on`/detail-pane
/// reference always means "really gone," never "now means something else").
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct TaskId(pub u32);

/// Where a task sits in the Kanban board's four columns. `Blocked` is a
/// manual, external state ("waiting on a review", "waiting on IT") --
/// distinct from a task merely having an unresolved `depends_on` entry,
/// which `AgendaStore::is_ready` reports separately and never changes
/// `status` itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Status {
    Todo,
    InProgress,
    Blocked,
    Done,
}

impl Status {
    pub const ALL: [Status; 4] = [Status::Todo, Status::InProgress, Status::Blocked, Status::Done];

    pub fn label(self) -> &'static str {
        match self {
            Status::Todo => "Todo",
            Status::InProgress => "In Progress",
            Status::Blocked => "Blocked",
            Status::Done => "Done",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Priority {
    Low,
    Medium,
    High,
    Urgent,
}

impl Priority {
    pub const ALL: [Priority; 4] = [Priority::Low, Priority::Medium, Priority::High, Priority::Urgent];

    pub fn label(self) -> &'static str {
        match self {
            Priority::Low => "Low",
            Priority::Medium => "Medium",
            Priority::High => "High",
            Priority::Urgent => "Urgent",
        }
    }
}

/// One entry in a task's notes log. Ordinarily append-only -- logging what
/// changed as a *new* entry, rather than rewriting an old one, is what
/// keeps earlier context from being lost the way a single freeform "notes"
/// field would lose it on every edit -- but `AgendaStore::edit_note`/
/// `remove_note` exist to fix a mistaken or accidental entry after the
/// fact, the same "a bad edit shouldn't have to stay forever" posture
/// `remove_subtask` already has.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NoteEntry {
    pub at: DateTime<Local>,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TimeSource {
    /// Recorded by `AgendaStore::clock_in`/`clock_out`.
    Timer,
    /// Recorded by `AgendaStore::log_manual_time`, for time worked before
    /// the timer was started or that never went through it at all.
    Manual,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimeEntry {
    pub start: DateTime<Local>,
    pub end: DateTime<Local>,
    pub source: TimeSource,
}

impl TimeEntry {
    pub fn duration(&self) -> chrono::Duration {
        self.end - self.start
    }
}

/// One step in a task's own checklist -- a lighter-weight breakdown than a
/// full linked task: no status, priority, or time tracking of its own, just
/// text and whether it's done. A step worth tracking on its own terms
/// belongs in `depends_on` instead.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Subtask {
    pub text: String,
    pub done: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: TaskId,
    pub title: String,
    pub description: String,
    pub status: Status,
    pub priority: Priority,
    /// Matches a name in `Config::agenda_categories` by value, not by
    /// index -- so removing a category from `config.ini` never orphans or
    /// fails to load a task that still names it; it just becomes an
    /// unrecognized label wherever it's shown.
    pub category: Option<String>,
    pub notes: Vec<NoteEntry>,
    pub time_entries: Vec<TimeEntry>,
    pub subtasks: Vec<Subtask>,
    /// Tasks that must be `Done` before this one is considered ready to
    /// work (see `AgendaStore::is_ready`). The reverse view ("this blocks
    /// ...") is never stored -- `AgendaStore::blocks` derives it by
    /// scanning every task's own `depends_on`, so the two can never drift
    /// out of sync with each other.
    pub depends_on: Vec<TaskId>,
    pub created_at: DateTime<Local>,
    pub updated_at: DateTime<Local>,
    pub done_at: Option<DateTime<Local>>,
    /// Hides a `Done` task from the board/list while keeping it (and its
    /// time entries) in the time report and in `agenda.json` -- a soft
    /// "cross it off the paper list" rather than a delete.
    pub archived: bool,
    /// Manual position within its status column; lower sorts first. Only
    /// meaningful relative to other tasks sharing the same `status`.
    pub order: i64,
}

impl Task {
    pub fn total_time(&self) -> chrono::Duration {
        self.time_entries.iter().fold(chrono::Duration::zero(), |acc, e| acc + e.duration())
    }
}
