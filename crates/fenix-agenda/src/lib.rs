//! The personal task/time-tracking module behind Fenix's `SPC a` agenda --
//! data model and persistence only. Mirrors `fenix-jira`'s own split: this
//! crate has no notion of a buffer, a keymap, or a picker; `fenix-gui`'s
//! `agenda_panel` module renders an `AgendaStore` into a real buffer and
//! dispatches keys back onto `AgendaStore`'s own methods.

pub mod jira;
mod persist;
mod store;
mod sync;
mod task;

pub use persist::{default_path, load, save};
pub use jira::{Conflict, JiraLink, OpKind, PendingOp, RemoteComment, RemoteSnapshot, RemoteUpdate, SyncField};
pub use store::{ActiveTimer, AgendaStore};
pub use sync::{parse_due, WorklogRow};
pub use task::{CodeRef, NoteEntry, Priority, Status, Subtask, Task, TaskId, TimeEntry, TimeSource};
