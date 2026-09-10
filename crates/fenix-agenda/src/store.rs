use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};

use crate::task::{NoteEntry, Priority, Status, Subtask, Task, TaskId, TimeEntry, TimeSource};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActiveTimer {
    pub task_id: TaskId,
    pub started_at: DateTime<Local>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgendaStore {
    pub tasks: Vec<Task>,
    next_id: u32,
    pub active_timer: Option<ActiveTimer>,
}

impl AgendaStore {
    pub fn task(&self, id: TaskId) -> Option<&Task> {
        self.tasks.iter().find(|t| t.id == id)
    }

    fn task_mut(&mut self, id: TaskId) -> Option<&mut Task> {
        self.tasks.iter_mut().find(|t| t.id == id)
    }

    /// `title`/`description` are taken as typed -- no validation beyond
    /// "not stored blank" (the caller's prompt flow already refuses an
    /// empty title before this is called, same as `fenix-jira`'s create
    /// prompt). Starts at `Status::Todo`, ordered after every other task
    /// already in that column.
    pub fn create_task(&mut self, title: String, description: String, priority: Priority, category: Option<String>) -> TaskId {
        let id = TaskId(self.next_id);
        self.next_id += 1;
        let now = Local::now();
        let order = self.tasks.iter().filter(|t| t.status == Status::Todo).map(|t| t.order).max().map_or(0, |n| n + 1);
        self.tasks.push(Task {
            id,
            title,
            description,
            status: Status::Todo,
            priority,
            category,
            notes: Vec::new(),
            time_entries: Vec::new(),
            subtasks: Vec::new(),
            depends_on: Vec::new(),
            created_at: now,
            updated_at: now,
            done_at: None,
            archived: false,
            order,
        });
        id
    }

    pub fn set_status(&mut self, id: TaskId, status: Status) {
        let now = Local::now();
        if let Some(task) = self.task_mut(id) {
            task.status = status;
            task.done_at = if status == Status::Done { Some(now) } else { None };
            task.updated_at = now;
        }
    }

    pub fn set_priority(&mut self, id: TaskId, priority: Priority) {
        if let Some(task) = self.task_mut(id) {
            task.priority = priority;
            task.updated_at = Local::now();
        }
    }

    pub fn set_category(&mut self, id: TaskId, category: Option<String>) {
        if let Some(task) = self.task_mut(id) {
            task.category = category;
            task.updated_at = Local::now();
        }
    }

    pub fn set_title(&mut self, id: TaskId, title: String) {
        if let Some(task) = self.task_mut(id) {
            task.title = title;
            task.updated_at = Local::now();
        }
    }

    pub fn set_description(&mut self, id: TaskId, description: String) {
        if let Some(task) = self.task_mut(id) {
            task.description = description;
            task.updated_at = Local::now();
        }
    }

    pub fn add_note(&mut self, id: TaskId, text: String) {
        let now = Local::now();
        if let Some(task) = self.task_mut(id) {
            task.notes.push(NoteEntry { at: now, text });
            task.updated_at = now;
        }
    }

    /// Fixes a mistaken or accidental note's text in place -- keeps its
    /// original `at` timestamp (when it was actually logged), same as
    /// amending a commit message doesn't change its author date.
    pub fn edit_note(&mut self, id: TaskId, index: usize, text: String) {
        let now = Local::now();
        if let Some(task) = self.task_mut(id) {
            if let Some(note) = task.notes.get_mut(index) {
                note.text = text;
                task.updated_at = now;
            }
        }
    }

    pub fn remove_note(&mut self, id: TaskId, index: usize) {
        if let Some(task) = self.task_mut(id) {
            if index < task.notes.len() {
                task.notes.remove(index);
                task.updated_at = Local::now();
            }
        }
    }

    /// Removes a logged time entry outright -- the timer/manual-entry
    /// equivalent of `remove_subtask`, for an accidental clock-in or a
    /// manual entry logged with the wrong duration.
    pub fn remove_time_entry(&mut self, id: TaskId, index: usize) {
        if let Some(task) = self.task_mut(id) {
            if index < task.time_entries.len() {
                task.time_entries.remove(index);
                task.updated_at = Local::now();
            }
        }
    }

    pub fn add_subtask(&mut self, id: TaskId, text: String) {
        if let Some(task) = self.task_mut(id) {
            task.subtasks.push(Subtask { text, done: false });
            task.updated_at = Local::now();
        }
    }

    pub fn toggle_subtask(&mut self, id: TaskId, index: usize) {
        if let Some(task) = self.task_mut(id) {
            if let Some(subtask) = task.subtasks.get_mut(index) {
                subtask.done = !subtask.done;
                task.updated_at = Local::now();
            }
        }
    }

    pub fn remove_subtask(&mut self, id: TaskId, index: usize) {
        if let Some(task) = self.task_mut(id) {
            if index < task.subtasks.len() {
                task.subtasks.remove(index);
                task.updated_at = Local::now();
            }
        }
    }

    /// Moves a card to a different column, resetting `order` to the back
    /// of its new column -- the board's `H`/`L`. Plain `set_status` covers
    /// every other status change (e.g. the picker); this exists so board
    /// reordering doesn't leave a task's `order` colliding with whatever
    /// was already at that position in the column it left.
    pub fn move_to_status(&mut self, id: TaskId, status: Status) {
        let order = self.tasks.iter().filter(|t| t.status == status && t.id != id).map(|t| t.order).max().map_or(0, |n| n + 1);
        self.set_status(id, status);
        if let Some(task) = self.task_mut(id) {
            task.order = order;
        }
    }

    /// Swaps this task's `order` with its neighbor `delta` positions away
    /// within the same status column (`delta` of `-1`/`1` for the board's
    /// `J`/`K`) -- a swap rather than a renumber, so reordering a column
    /// never disturbs any other column's own ordering.
    pub fn reorder_within_status(&mut self, id: TaskId, delta: isize) {
        let Some(task) = self.task(id) else { return };
        let status = task.status;
        let mut siblings: Vec<TaskId> = self.tasks.iter().filter(|t| t.status == status).map(|t| t.id).collect();
        siblings.sort_by_key(|&sid| self.task(sid).map(|t| t.order).unwrap_or(0));
        let Some(pos) = siblings.iter().position(|&sid| sid == id) else { return };
        let new_pos = pos as isize + delta;
        if new_pos < 0 || new_pos as usize >= siblings.len() {
            return;
        }
        let other = siblings[new_pos as usize];
        let (order_a, order_b) = (self.task(id).unwrap().order, self.task(other).unwrap().order);
        self.task_mut(id).unwrap().order = order_b;
        self.task_mut(other).unwrap().order = order_a;
    }

    /// Starts the clock on `id`, first auto-closing whatever timer was
    /// already running (into a completed `TimeEntry` on *that* task) --
    /// switching what you're working on is the common case, not an error,
    /// so it should never require remembering to clock out first.
    pub fn clock_in(&mut self, id: TaskId) {
        self.clock_out();
        self.active_timer = Some(ActiveTimer { task_id: id, started_at: Local::now() });
    }

    /// Stops whatever timer is running, if any, recording the elapsed span
    /// as a `TimeSource::Timer` entry. A no-op when nothing is running.
    pub fn clock_out(&mut self) {
        if let Some(timer) = self.active_timer.take() {
            let now = Local::now();
            if let Some(task) = self.task_mut(timer.task_id) {
                task.time_entries.push(TimeEntry { start: timer.started_at, end: now, source: TimeSource::Timer });
                task.updated_at = now;
            }
        }
    }

    pub fn log_manual_time(&mut self, id: TaskId, duration: chrono::Duration) {
        let now = Local::now();
        if let Some(task) = self.task_mut(id) {
            task.time_entries.push(TimeEntry { start: now - duration, end: now, source: TimeSource::Manual });
            task.updated_at = now;
        }
    }

    /// Total time on a task, including whatever the active timer has
    /// accumulated so far if it's the one running -- what a "time spent"
    /// column should actually show, since `Task::total_time` alone only
    /// sees closed entries.
    pub fn elapsed_on(&self, id: TaskId) -> chrono::Duration {
        let base = self.task(id).map(Task::total_time).unwrap_or_default();
        match &self.active_timer {
            Some(timer) if timer.task_id == id => base + (Local::now() - timer.started_at),
            _ => base,
        }
    }

    /// Rejects `depends_on == id` and any edge that would create a cycle
    /// (a task, directly or transitively, depending on itself) -- a
    /// dependency graph you can't read is worse than one field short, the
    /// same "a bad edit changes nothing and stays on screen to fix"
    /// posture the explorer's bulk rename already has. Returns whether the
    /// dependency was actually added.
    pub fn add_dependency(&mut self, id: TaskId, depends_on: TaskId) -> bool {
        if id == depends_on || self.task(depends_on).is_none() || self.task(id).is_none() {
            return false;
        }
        if self.would_cycle(id, depends_on) {
            return false;
        }
        let task = self.task_mut(id).unwrap();
        if !task.depends_on.contains(&depends_on) {
            task.depends_on.push(depends_on);
            task.updated_at = Local::now();
        }
        true
    }

    /// Whether adding `id -> target` would create a cycle: true if `target`
    /// already (transitively) depends on `id`.
    fn would_cycle(&self, id: TaskId, target: TaskId) -> bool {
        let mut stack = vec![target];
        let mut seen = std::collections::HashSet::new();
        while let Some(current) = stack.pop() {
            if current == id {
                return true;
            }
            if !seen.insert(current) {
                continue;
            }
            if let Some(task) = self.task(current) {
                stack.extend(task.depends_on.iter().copied());
            }
        }
        false
    }

    pub fn remove_dependency(&mut self, id: TaskId, depends_on: TaskId) {
        if let Some(task) = self.task_mut(id) {
            task.depends_on.retain(|&d| d != depends_on);
            task.updated_at = Local::now();
        }
    }

    /// A task is ready when every task it depends on is `Done` (a task
    /// with no dependencies at all is trivially ready). Purely a query --
    /// never refuses `set_status` or anything else; see the module-level
    /// docs on why this is informational only.
    pub fn is_ready(&self, id: TaskId) -> bool {
        match self.task(id) {
            Some(task) => task.depends_on.iter().all(|&dep| self.task(dep).is_none_or(|t| t.status == Status::Done)),
            None => true,
        }
    }

    pub fn blocked_by(&self, id: TaskId) -> Vec<&Task> {
        match self.task(id) {
            Some(task) => task.depends_on.iter().filter_map(|&dep| self.task(dep)).collect(),
            None => Vec::new(),
        }
    }

    /// The reverse of `blocked_by`, derived by scanning every task's own
    /// `depends_on` rather than stored -- so it can never drift out of
    /// sync with what `depends_on` actually says.
    pub fn blocks(&self, id: TaskId) -> Vec<&Task> {
        self.tasks.iter().filter(|t| t.depends_on.contains(&id)).collect()
    }

    /// Candidate tasks for `add_dependency(id, _)` -- excludes `id` itself
    /// and anything that would create a cycle, so a picker built from this
    /// list never offers a choice `add_dependency` would then refuse.
    pub fn dependency_candidates(&self, id: TaskId) -> Vec<&Task> {
        self.tasks.iter().filter(|t| t.id != id && !self.would_cycle(id, t.id)).collect()
    }

    /// Hides a `Done` task from the board/list -- a soft "cross it off,"
    /// distinct from `delete`: it stays in `agenda.json` and in the time
    /// report.
    pub fn archive(&mut self, id: TaskId) {
        if let Some(task) = self.task_mut(id) {
            task.archived = true;
            task.updated_at = Local::now();
        }
    }

    /// Removes a task outright, including any other task's `depends_on`
    /// reference to it -- `id`s are never reused (see `TaskId`'s own doc
    /// comment), so nothing else can silently start meaning this deleted
    /// task.
    pub fn delete(&mut self, id: TaskId) {
        self.tasks.retain(|t| t.id != id);
        for task in &mut self.tasks {
            task.depends_on.retain(|&d| d != id);
        }
        if matches!(&self.active_timer, Some(t) if t.task_id == id) {
            self.active_timer = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store_with_task() -> (AgendaStore, TaskId) {
        let mut store = AgendaStore::default();
        let id = store.create_task("Write the plan".to_string(), "".to_string(), Priority::Medium, None);
        (store, id)
    }

    #[test]
    fn a_new_task_starts_todo_and_ready() {
        let (store, id) = store_with_task();
        let task = store.task(id).unwrap();
        assert_eq!(task.status, Status::Todo);
        assert!(store.is_ready(id));
    }

    #[test]
    fn setting_status_to_done_records_done_at_and_clearing_it_removes_done_at() {
        let (mut store, id) = store_with_task();
        store.set_status(id, Status::Done);
        assert!(store.task(id).unwrap().done_at.is_some());
        store.set_status(id, Status::Todo);
        assert!(store.task(id).unwrap().done_at.is_none());
    }

    #[test]
    fn clocking_into_a_second_task_auto_stops_the_first() {
        let mut store = AgendaStore::default();
        let a = store.create_task("A".to_string(), "".to_string(), Priority::Low, None);
        let b = store.create_task("B".to_string(), "".to_string(), Priority::Low, None);

        store.clock_in(a);
        assert!(store.active_timer.is_some());
        store.clock_in(b);

        assert_eq!(store.active_timer.as_ref().unwrap().task_id, b);
        assert_eq!(store.task(a).unwrap().time_entries.len(), 1, "switching tasks should close out A's running timer");
        assert_eq!(store.task(a).unwrap().time_entries[0].source, TimeSource::Timer);
    }

    #[test]
    fn clocking_out_with_nothing_running_is_a_no_op() {
        let (mut store, _) = store_with_task();
        store.clock_out();
        assert!(store.active_timer.is_none());
    }

    #[test]
    fn manual_time_entry_is_recorded_with_the_manual_source() {
        let (mut store, id) = store_with_task();
        store.log_manual_time(id, chrono::Duration::minutes(45));
        let entries = &store.task(id).unwrap().time_entries;
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].source, TimeSource::Manual);
        assert_eq!(entries[0].duration(), chrono::Duration::minutes(45));
    }

    #[test]
    fn elapsed_on_includes_the_running_timer_but_only_for_the_task_its_running_on() {
        let mut store = AgendaStore::default();
        let a = store.create_task("A".to_string(), "".to_string(), Priority::Low, None);
        let b = store.create_task("B".to_string(), "".to_string(), Priority::Low, None);
        store.log_manual_time(a, chrono::Duration::minutes(10));

        store.clock_in(a);
        // No real sleep in a unit test -- just confirm the closed 10m entry
        // is counted and B (never touched) stays at zero.
        assert!(store.elapsed_on(a) >= chrono::Duration::minutes(10));
        assert_eq!(store.elapsed_on(b), chrono::Duration::zero());
    }

    #[test]
    fn a_task_is_not_ready_while_its_dependency_is_unresolved() {
        let mut store = AgendaStore::default();
        let dep = store.create_task("Dep".to_string(), "".to_string(), Priority::Low, None);
        let task = store.create_task("Task".to_string(), "".to_string(), Priority::Low, None);

        assert!(store.add_dependency(task, dep));
        assert!(!store.is_ready(task));

        store.set_status(dep, Status::Done);
        assert!(store.is_ready(task));
    }

    #[test]
    fn a_task_cannot_depend_on_itself() {
        let (mut store, id) = store_with_task();
        assert!(!store.add_dependency(id, id));
        assert!(store.task(id).unwrap().depends_on.is_empty());
    }

    #[test]
    fn a_dependency_that_would_create_a_cycle_is_rejected() {
        let mut store = AgendaStore::default();
        let a = store.create_task("A".to_string(), "".to_string(), Priority::Low, None);
        let b = store.create_task("B".to_string(), "".to_string(), Priority::Low, None);
        assert!(store.add_dependency(b, a)); // b depends on a
        assert!(!store.add_dependency(a, b)); // a depending on b would cycle
        assert!(store.task(a).unwrap().depends_on.is_empty());
    }

    #[test]
    fn blocks_is_derived_as_the_reverse_of_depends_on() {
        let mut store = AgendaStore::default();
        let a = store.create_task("A".to_string(), "".to_string(), Priority::Low, None);
        let b = store.create_task("B".to_string(), "".to_string(), Priority::Low, None);
        store.add_dependency(b, a);

        let blocks_a: Vec<TaskId> = store.blocks(a).into_iter().map(|t| t.id).collect();
        assert_eq!(blocks_a, vec![b]);
        let blocked_by_b: Vec<TaskId> = store.blocked_by(b).into_iter().map(|t| t.id).collect();
        assert_eq!(blocked_by_b, vec![a]);
    }

    #[test]
    fn deleting_a_task_removes_it_from_other_tasks_dependencies() {
        let mut store = AgendaStore::default();
        let a = store.create_task("A".to_string(), "".to_string(), Priority::Low, None);
        let b = store.create_task("B".to_string(), "".to_string(), Priority::Low, None);
        store.add_dependency(b, a);

        store.delete(a);

        assert!(store.task(a).is_none());
        assert!(store.task(b).unwrap().depends_on.is_empty());
        assert!(store.is_ready(b));
    }

    #[test]
    fn deleting_the_task_whose_timer_is_running_clears_the_active_timer() {
        let (mut store, id) = store_with_task();
        store.clock_in(id);
        store.delete(id);
        assert!(store.active_timer.is_none());
    }

    #[test]
    fn a_note_can_be_edited_in_place_without_changing_its_timestamp() {
        let (mut store, id) = store_with_task();
        store.add_note(id, "started looking into this".to_string());
        let original_at = store.task(id).unwrap().notes[0].at;

        store.edit_note(id, 0, "started looking into this, turns out it's harder than expected".to_string());

        let note = &store.task(id).unwrap().notes[0];
        assert_eq!(note.text, "started looking into this, turns out it's harder than expected");
        assert_eq!(note.at, original_at);
    }

    #[test]
    fn editing_a_note_at_an_out_of_bounds_index_is_a_no_op() {
        let (mut store, id) = store_with_task();
        store.add_note(id, "the only note".to_string());
        store.edit_note(id, 5, "replacement".to_string());
        assert_eq!(store.task(id).unwrap().notes[0].text, "the only note");
    }

    #[test]
    fn a_note_can_be_removed() {
        let (mut store, id) = store_with_task();
        store.add_note(id, "keep".to_string());
        store.add_note(id, "remove me".to_string());

        store.remove_note(id, 1);

        let notes = &store.task(id).unwrap().notes;
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].text, "keep");
    }

    #[test]
    fn removing_a_note_at_an_out_of_bounds_index_is_a_no_op() {
        let (mut store, id) = store_with_task();
        store.add_note(id, "the only note".to_string());
        store.remove_note(id, 5);
        assert_eq!(store.task(id).unwrap().notes.len(), 1);
    }

    #[test]
    fn a_logged_time_entry_can_be_removed() {
        let (mut store, id) = store_with_task();
        store.log_manual_time(id, chrono::Duration::minutes(30));
        store.log_manual_time(id, chrono::Duration::minutes(9999)); // an obvious mistake

        store.remove_time_entry(id, 1);

        let entries = &store.task(id).unwrap().time_entries;
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].duration(), chrono::Duration::minutes(30));
    }

    #[test]
    fn removing_a_time_entry_at_an_out_of_bounds_index_is_a_no_op() {
        let (mut store, id) = store_with_task();
        store.log_manual_time(id, chrono::Duration::minutes(30));
        store.remove_time_entry(id, 5);
        assert_eq!(store.task(id).unwrap().time_entries.len(), 1);
    }

    #[test]
    fn subtasks_can_be_added_toggled_and_removed() {
        let (mut store, id) = store_with_task();
        store.add_subtask(id, "Draft outline".to_string());
        assert!(!store.task(id).unwrap().subtasks[0].done);

        store.toggle_subtask(id, 0);
        assert!(store.task(id).unwrap().subtasks[0].done);

        store.remove_subtask(id, 0);
        assert!(store.task(id).unwrap().subtasks.is_empty());
    }

    #[test]
    fn archiving_keeps_the_task_but_marks_it_archived() {
        let (mut store, id) = store_with_task();
        store.archive(id);
        assert!(store.task(id).unwrap().archived);
    }

    #[test]
    fn reordering_within_a_status_swaps_with_the_neighbor_and_leaves_the_ends_alone() {
        let mut store = AgendaStore::default();
        let a = store.create_task("A".to_string(), "".to_string(), Priority::Low, None);
        let b = store.create_task("B".to_string(), "".to_string(), Priority::Low, None);
        let order_a = store.task(a).unwrap().order;
        let order_b = store.task(b).unwrap().order;

        store.reorder_within_status(b, -1);

        assert_eq!(store.task(b).unwrap().order, order_a);
        assert_eq!(store.task(a).unwrap().order, order_b);

        // Already first: moving further up is a no-op, not a panic.
        store.reorder_within_status(b, -1);
        assert_eq!(store.task(b).unwrap().order, order_a);
    }

    #[test]
    fn moving_to_a_new_status_column_reorders_to_the_back_of_it() {
        let mut store = AgendaStore::default();
        let a = store.create_task("A".to_string(), "".to_string(), Priority::Low, None);
        store.set_status(a, Status::InProgress); // occupies order 0 in InProgress
        let b = store.create_task("B".to_string(), "".to_string(), Priority::Low, None);

        store.move_to_status(b, Status::InProgress);

        assert_eq!(store.task(b).unwrap().status, Status::InProgress);
        assert!(store.task(b).unwrap().order > store.task(a).unwrap().order);
    }

    #[test]
    fn dependency_candidates_exclude_self_and_anything_that_would_cycle() {
        let mut store = AgendaStore::default();
        let a = store.create_task("A".to_string(), "".to_string(), Priority::Low, None);
        let b = store.create_task("B".to_string(), "".to_string(), Priority::Low, None);
        store.add_dependency(b, a); // b depends on a

        let candidates: Vec<TaskId> = store.dependency_candidates(a).into_iter().map(|t| t.id).collect();
        assert!(!candidates.contains(&a), "a task is never its own dependency candidate");
        assert!(!candidates.contains(&b), "b already depends on a, so a depending on b would cycle");
    }
}
