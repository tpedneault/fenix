//! The host half of the agenda page (`agenda_page`): opening it, the
//! context it reads, and carrying out what its keys ask -- editing the
//! store, queuing the change for Jira, opening a compose pane. The Jira
//! side of each change (transitions, the outbox, worklogs) stays in
//! `agenda_sync`.

use super::pages::PageModel;
use super::*;
use crate::agenda_page::{self, Action as AgendaAction, AgendaPage, NewTask, Project, Tab};
use fenix_agenda::{CodeRef, OpKind, Status};

/// What `SPC a h` captured, waiting for the new-task form to finish.
pub(super) struct Capture {
    pub(super) code: CodeRef,
    pub(super) text: String,
}

impl App {
    pub(super) fn agenda_page_mut(&mut self, id: BufferId) -> Option<&mut AgendaPage> {
        match self.pages.get_mut(&id).map(|s| {
            s.stale = true;
            &mut s.model
        }) {
            Some(PageModel::Agenda(p)) => Some(p),
            _ => None,
        }
    }

    fn agenda_page_id(&self) -> Option<BufferId> {
        self.find_page(|m| matches!(m, PageModel::Agenda(_)))
    }

    /// The project the focused file is in, and its Jira key.
    fn agenda_project(&self) -> Option<Project> {
        let root = self.project_root.as_ref()?;
        let name = root.file_name()?.to_string_lossy().into_owned();
        Some(Project { name, jira_key: fenix_project::meta::jira_key(root) })
    }

    /// "synced 3m ago", "syncing", or nothing when Jira isn't set up.
    pub(super) fn agenda_sync_label(&self) -> String {
        if self.config.jira_base_url.is_none() {
            return String::new();
        }
        if self.agenda_sync.pulling {
            return "syncing".to_string();
        }
        match self.agenda_sync.last_pull {
            Some(at) if at.elapsed().as_secs() < 60 => "synced just now".to_string(),
            Some(at) => format!("synced {}m ago", at.elapsed().as_secs() / 60),
            None if self.agenda_store.tasks.iter().any(|t| t.jira.is_some()) => "not synced yet".to_string(),
            None => String::new(),
        }
    }

    /// `SPC a a` and friends: the agenda page, on `tab` when given, else
    /// where it was left.
    pub(crate) fn open_agenda_page(&mut self, tab: Option<Tab>) -> BufferId {
        self.agenda_sync_if_stale();
        let project = self.agenda_project();
        let id = match self.agenda_page_id() {
            Some(id) => {
                self.show_page(id);
                if let Some(page) = self.agenda_page_mut(id) {
                    if project.is_some() {
                        page.project = project;
                    }
                    if let Some(tab) = tab {
                        page.show_tab(tab);
                    }
                    page.note = None;
                }
                id
            }
            None => self.open_page(PageModel::Agenda(Box::new(AgendaPage::new(tab.unwrap_or(Tab::Today), project)))),
        };
        self.agenda_worklog_reset();
        id
    }

    /// Every agenda page, laid out again -- after anything changed the
    /// store, from here or from a sync.
    pub(super) fn refresh_agenda_pages(&mut self) {
        let ids: Vec<BufferId> = self.pages.iter().filter(|(_, s)| matches!(s.model, PageModel::Agenda(_))).map(|(id, _)| *id).collect();
        for id in ids {
            let store = &self.agenda_store;
            if let Some(state) = self.pages.get_mut(&id) {
                state.stale = true;
                if let PageModel::Agenda(p) = &mut state.model {
                    p.forget_missing(store);
                }
            }
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    /// `SPC a n`: the page, with the new-task form open.
    pub(crate) fn cmd_agenda_new_task(&mut self) {
        self.agenda_capture = None;
        let id = self.open_agenda_page(None);
        self.agenda_page_key_now(id, crate::page::Key::Char('n'));
    }

    /// `SPC a h`: a task from here -- the selection just made (or the
    /// line the cursor's on) becomes its description, and the task keeps
    /// the file and line to come back to.
    pub(crate) fn cmd_agenda_task_from_here(&mut self) {
        let Some(path) = self.open().buffer.path().map(Path::to_path_buf) else {
            self.set_error("SPC a h makes a task from a file -- this buffer isn't one");
            return;
        };
        let cursor = self.cursor();
        let text: Vec<char> = self.open().buffer.text().chars().collect();
        let (line, _) = self.open().buffer.line_col(&cursor);
        let line_range = |at: usize| {
            let start = text[..at.min(text.len())].iter().rposition(|c| *c == '\n').map(|i| i + 1).unwrap_or(0);
            let end = text[at.min(text.len())..].iter().position(|c| *c == '\n').map(|i| at + i).unwrap_or(text.len());
            (start, end)
        };
        let range = match self.vim.visual_selection_range(&self.open().buffer, &cursor) {
            Some((range, _)) => (range.start, range.end),
            None => match self.vim.last_visual_range() {
                // The selection just left for the leader: the cursor is still in it.
                Some((a, b)) if (a..=b).contains(&cursor.char_idx) => (line_range(a).0, line_range(b).1),
                _ => line_range(cursor.char_idx),
            },
        };
        let first_line = text[..range.0.min(text.len())].iter().filter(|c| **c == '\n').count() + 1;
        let body: String = text[range.0.min(text.len())..range.1.min(text.len())].iter().collect();
        self.vim.exit_visual_mode(&cursor);
        let lang = path.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default();
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let text = format!("{name}:{first_line}\n\n```{lang}\n{}\n```", body.trim_end());
        let _ = line;
        self.agenda_capture = Some(Capture { code: CodeRef { path, line: first_line }, text });
        let id = self.open_agenda_page(None);
        self.agenda_page_key_now(id, crate::page::Key::Char('n'));
    }

    /// `SPC a /`: the page, searching.
    pub(crate) fn cmd_agenda_find(&mut self) {
        let id = self.open_agenda_page(None);
        self.agenda_page_key_now(id, crate::page::Key::Char('/'));
    }

    /// Presses `key` on agenda page `id`, as if typed.
    fn agenda_page_key_now(&mut self, id: BufferId, key: crate::page::Key) {
        let worklogs = self.agenda_worklog_rows();
        let sync = self.agenda_sync_label();
        let round = self.agenda_worklog_round();
        let Some(state) = self.pages.get_mut(&id) else { return };
        state.stale = true;
        let PageModel::Agenda(p) = &mut state.model else { return };
        let ctx = agenda_page::Ctx { store: &self.agenda_store, now: chrono::Local::now(), categories: &self.config.agenda_categories, worklogs: &worklogs, round, sync };
        let action = p.key(key, &ctx);
        self.agenda_page_action(id, action);
    }

    /// Starts the worklog review afresh: no edits or drops carried over.
    pub(super) fn agenda_worklog_reset(&mut self) {
        self.agenda_sync.worklog_edits.clear();
        self.agenda_sync.worklog_dropped.clear();
    }

    /// Adds `name` to the categories, if it isn't one.
    fn agenda_ensure_category(&mut self, name: &str) {
        if self.config.agenda_categories.iter().any(|c| c.eq_ignore_ascii_case(name)) {
            return;
        }
        self.config.agenda_categories.push(name.to_string());
        if let Err(err) = self.config.save() {
            self.set_error(format!("couldn't save settings.toml: {err}"));
        }
    }

    fn agenda_create(&mut self, id: BufferId, spec: NewTask) {
        let capture = self.agenda_capture.take();
        let description = capture.as_ref().map(|c| c.text.clone()).unwrap_or_default();
        if let Some(category) = &spec.category {
            self.agenda_ensure_category(category);
        }
        let category = spec.category.as_ref().and_then(|c| self.config.agenda_categories.iter().find(|x| x.eq_ignore_ascii_case(c)).cloned());
        let task = self.agenda_store.create_task(spec.title.clone(), description, spec.priority, category);
        if spec.status != Status::Todo {
            self.agenda_store.move_to_status(task, spec.status);
        }
        self.agenda_store.set_due(task, spec.due);
        if let Some(capture) = capture {
            self.agenda_store.set_code(task, Some(capture.code));
        }
        if spec.clock {
            self.agenda_store.clock_in(task);
        }
        self.agenda_save_and_refresh();
        if let Some(page) = self.agenda_page_mut(id) {
            page.select(task);
            page.note = Some((format!("added \"{}\"", spec.title), false));
        }
        if let Some(key) = spec.link {
            self.agenda_fetch_issue(key.to_uppercase(), agenda_sync::IssueFetch::LinkKey(task));
        }
    }

    pub(super) fn agenda_page_action(&mut self, id: BufferId, action: AgendaAction) {
        use AgendaAction as A;
        match action {
            A::None => {}
            A::Close => self.close_page(id),
            A::SetStatus(task, status) => self.agenda_set_status(task, status),
            A::JiraStatus(task) => self.agenda_start_linked_status_picker(task),
            A::SetPriority(task, priority) => {
                self.agenda_store.set_priority(task, priority);
                self.agenda_save_and_refresh();
            }
            A::JiraPriority(task) => self.agenda_start_linked_priority_picker(task),
            A::SetCategory(task, category) => {
                self.agenda_store.set_category(task, category);
                self.agenda_save_and_refresh();
            }
            A::AddCategory(task, name) => {
                self.agenda_ensure_category(&name);
                self.agenda_store.set_category(task, Some(name));
                self.agenda_save_and_refresh();
            }
            A::SetDue(task, due) => {
                self.agenda_store.set_due(task, due);
                self.agenda_store.enqueue(task, OpKind::SetDue(due.map(|d| d.format("%Y-%m-%d").to_string())));
                self.agenda_after_edit();
            }
            A::SetTitle(task, title) => {
                self.agenda_store.set_title(task, title.clone());
                self.agenda_store.enqueue(task, OpKind::SetSummary(title));
                self.agenda_after_edit();
            }
            A::EditDescription(task) => {
                let seed = self.agenda_store.task(task).map(|t| t.description.clone()).unwrap_or_default();
                self.open_compose_seeded(ComposePurpose::TaskDescription { task }, &seed);
            }
            A::AddNote(task, text) => {
                self.agenda_store.add_note(task, text);
                self.agenda_save_and_refresh();
            }
            A::EditNote(task, i, text) => {
                self.agenda_store.edit_note(task, i, text);
                self.agenda_save_and_refresh();
            }
            A::RemoveNote(task, i) => {
                self.agenda_store.remove_note(task, i);
                self.agenda_save_and_refresh();
            }
            A::Comment(task) => self.open_compose_seeded(ComposePurpose::TaskComment { task }, ""),
            A::PostNote(task, i) => {
                let Some(t) = self.agenda_store.task(task) else { return };
                let (Some(key), Some(note)) = (t.jira_key().map(str::to_string), t.notes.get(i).map(|n| n.text.clone())) else { return };
                self.agenda_store.enqueue(task, OpKind::AddComment(note));
                self.set_message(format!("Posting the note to {key} as a comment..."));
                self.agenda_after_edit();
            }
            A::Clock(task) => self.agenda_clock_toggle(task),
            A::LogSpan(task, start, end) => {
                self.agenda_store.log_span(task, start, end);
                self.agenda_save_and_refresh();
            }
            A::EditTime(task, i, start, end) => {
                if !self.agenda_store.edit_time_entry(task, i, start, end) {
                    self.set_error("the end has to be after the start");
                }
                self.agenda_save_and_refresh();
            }
            A::RemoveTime(task, i) => {
                self.agenda_store.remove_time_entry(task, i);
                self.agenda_save_and_refresh();
            }
            A::AddCheck(task, text) => {
                self.agenda_store.add_subtask(task, text);
                self.agenda_save_and_refresh();
            }
            A::EditCheck(task, i, text) => {
                self.agenda_store.edit_subtask(task, i, text);
                self.agenda_save_and_refresh();
            }
            A::ToggleCheck(task, i) => {
                self.agenda_store.toggle_subtask(task, i);
                self.agenda_save_and_refresh();
            }
            A::RemoveCheck(task, i) => {
                self.agenda_store.remove_subtask(task, i);
                self.agenda_save_and_refresh();
            }
            A::AddDependency(task, on) => {
                if !self.agenda_store.add_dependency(task, on) {
                    self.set_error("that would make the two wait on each other");
                }
                self.agenda_save_and_refresh();
            }
            A::RemoveDependency(task, on) => {
                self.agenda_store.remove_dependency(task, on);
                self.agenda_save_and_refresh();
            }
            A::Resolve(task, field, mine) => {
                self.agenda_store.resolve_conflict(task, field, mine);
                self.agenda_after_edit();
            }
            A::Link(task) => self.agenda_start_link(task),
            A::Assign(task) => self.agenda_start_assignee_picker(task),
            A::CopyLink(task) => self.agenda_copy_link(task),
            A::Browser(task) => self.agenda_open_in_browser(task),
            A::Retry(task) => self.agenda_retry(task),
            A::OpenCode(task) => {
                let Some(code) = self.agenda_store.task(task).and_then(|t| t.code.clone()) else { return };
                if !code.path.exists() {
                    self.set_error(format!("{} isn't there any more", code.path.display()));
                    return;
                }
                self.open_file_from_picker(&code.path);
                self.jump_to_grep_match(&fenix_project::GrepMatch { path: code.path, line: code.line, col: 1, text: String::new() });
            }
            A::Archive(task) => {
                self.agenda_store.archive(task);
                self.agenda_save_and_refresh();
            }
            A::Delete(task) => {
                let key = self.agenda_store.task(task).and_then(|t| t.jira_key()).map(str::to_string);
                self.agenda_store.delete(task);
                self.agenda_save_and_refresh();
                self.set_message(match key {
                    Some(key) => format!("Deleted -- {key} in Jira isn't affected"),
                    None => "Deleted".to_string(),
                });
            }
            A::Reorder(task, by) => {
                self.agenda_store.reorder_within_status(task, by);
                self.agenda_save_and_refresh();
            }
            A::Create(spec) => self.agenda_create(id, spec),
            A::Sync => self.cmd_agenda_sync(),
            A::WorklogMinutes(task, date, minutes) => {
                self.agenda_sync.worklog_edits.insert((task, date), minutes);
                self.refresh_agenda_pages();
            }
            A::WorklogDrop(task, date) => {
                self.agenda_sync.worklog_dropped.insert((task, date));
                self.refresh_agenda_pages();
            }
            A::WorklogDismiss(task, date) => {
                let Some(row) = self.agenda_worklog_rows().into_iter().find(|r| r.task == task && r.date == date) else { return };
                self.agenda_store.mark_sent(task, &row.entries);
                self.agenda_save_and_refresh();
                self.set_message(format!("{} on {}: dismissed, won't be sent", row.key, row.date));
            }
            A::SendWorklogs => self.agenda_send_worklogs(),
            A::Copy(text) => {
                if let Some(clipboard) = &mut self.clipboard {
                    let _ = clipboard.set_text(text);
                }
                self.set_message("Copied the week");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(dir: &tempfile::TempDir) -> App {
        let mut app = App::with_file(None);
        app.agenda_path = dir.path().join("agenda.json");
        app.agenda_store = fenix_agenda::AgendaStore::default();
        app.config.agenda_categories = vec!["ops".into()];
        app
    }

    fn press(app: &mut App, keys: &str) {
        for c in keys.chars() {
            assert!(app.page_key(KeyPress::char(c)), "{c}");
        }
    }

    fn enter(app: &mut App) {
        assert!(app.page_key(KeyPress::named(FenixNamedKey::Enter)));
    }

    fn page_text(app: &mut App) -> String {
        let id = app.focused_buffer_id();
        let pane = app.focused_pane_id();
        app.ensure_page_layout(id, pane, 130);
        app.open().buffer.text()
    }

    #[test]
    fn a_task_added_from_the_page_is_saved_and_shown() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app(&dir);
        app.cmd_agenda_new_task();
        press(&mut app, "Write the report !high #ops due:+2");
        enter(&mut app);
        let task = &app.agenda_store.tasks[0];
        assert_eq!(task.title, "Write the report");
        assert_eq!(task.priority, fenix_agenda::Priority::High);
        assert_eq!(task.category.as_deref(), Some("ops"));
        assert_eq!(task.due, Some(chrono::Local::now().date_naive() + chrono::Duration::days(2)));
        assert!(std::fs::read_to_string(dir.path().join("agenda.json")).unwrap().contains("Write the report"));
        assert!(page_text(&mut app).contains("Write the report"));
    }

    #[test]
    fn keys_on_a_row_change_the_task_and_the_page_follows() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app(&dir);
        let id = app.agenda_store.create_task("Tidy up".into(), String::new(), fenix_agenda::Priority::Medium, None);
        app.open_agenda_page(Some(Tab::List));
        press(&mut app, "s");
        press(&mut app, "2");
        assert_eq!(app.agenda_store.task(id).unwrap().status, Status::InProgress);
        press(&mut app, "t");
        assert!(app.agenda_store.active_timer.is_some());
        press(&mut app, "c");
        press(&mut app, "2");
        assert_eq!(app.agenda_store.task(id).unwrap().category.as_deref(), Some("ops"));
        assert!(page_text(&mut app).contains("#ops"));
    }

    #[test]
    fn a_task_from_here_keeps_the_line_and_the_code() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("main.rs");
        std::fs::write(&file, "fn main() {\n    todo!()\n}\n").unwrap();
        let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
        app.agenda_path = dir.path().join("agenda.json");
        app.agenda_store = fenix_agenda::AgendaStore::default();
        app.test_vim_key(KeyPress::char('j'));
        app.cmd_agenda_task_from_here();
        press(&mut app, "Finish main");
        enter(&mut app);
        let task = &app.agenda_store.tasks[0];
        assert_eq!(task.code.as_ref().map(|c| c.line), Some(2));
        assert!(task.description.contains("todo!()") && task.description.contains("main.rs:2"), "{}", task.description);
    }

    #[test]
    fn deleting_the_open_task_goes_back_to_the_tab() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app(&dir);
        app.agenda_store.create_task("Doomed".into(), String::new(), fenix_agenda::Priority::Medium, None);
        app.open_agenda_page(Some(Tab::List));
        enter(&mut app);
        press(&mut app, "DD");
        assert!(app.agenda_store.tasks.is_empty());
        assert!(page_text(&mut app).contains("No tasks yet"));
    }
}
