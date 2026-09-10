//! The personal task/time-tracking module behind Fenix's `SPC a` agenda --
//! data model and persistence only. Mirrors `fenix-jira`'s own split: this
//! crate has no notion of a buffer, a keymap, or a picker; `fenix-gui`'s
//! `agenda_panel` module renders an `AgendaStore` into a real buffer and
//! dispatches keys back onto `AgendaStore`'s own methods.

mod persist;
mod store;
mod task;

pub use persist::{default_path, load, save};
pub use store::{ActiveTimer, AgendaStore};
pub use task::{NoteEntry, Priority, Status, Subtask, Task, TaskId, TimeEntry, TimeSource};
