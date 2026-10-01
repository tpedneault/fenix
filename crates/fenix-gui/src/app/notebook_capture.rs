//! Getting things into the notebook and finding them again: `SPC n c`
//! captures a line (to today's journal, or the Inbox) from anywhere,
//! with a link back to where you were; `SPC n /` searches every note
//! and diagram with the notebook's query syntax; `SPC n #` lists the
//! tags; `SPC n p` the notes about this project; `SPC n t` sends the
//! checkbox under the cursor to the agenda; and in a journal day,
//! `SPC m a` refreshes its agenda list and `SPC m g` adds today's
//! commits.

use super::notebook_host::{NotebookPick, NotebookPrompt, NotebookPromptKind};
use super::pages::PageModel;
use super::*;
use fenix_notebook::query::{self, Query};
use fenix_notebook::Kind as NbKind;

/// Where a capture goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CaptureTo {
    Journal,
    Inbox,
}

impl CaptureTo {
    pub(super) fn label(self) -> &'static str {
        match self {
            CaptureTo::Journal => "today's journal",
            CaptureTo::Inbox => "Inbox",
        }
    }
}

impl App {
    /// `SPC n c`: a line into the notebook from wherever you are.
    pub(crate) fn notebook_capture(&mut self) {
        if self.notebook().is_none() {
            return;
        }
        let to = if self.config.notebook_journal.unwrap_or(true) { CaptureTo::Journal } else { CaptureTo::Inbox };
        // A link back to the line, when there's a file here.
        let link = self.open().buffer.path().map(Path::to_path_buf).filter(|p| !self.notebook.book.as_ref().is_some_and(|nb| nb.by_path(p).is_some())).map(|path| {
            let (line, _) = self.open().buffer.line_col(&self.cursor());
            self.notebook_link_to(&path, Some(line + 1))
        });
        self.notebook.prompt = Some(NotebookPrompt { kind: NotebookPromptKind::Capture { to, task: false, link }, input: String::new() });
        self.wake_caret();
    }

    /// The capture prompt's own keys: `Tab` changes where it goes,
    /// `Ctrl-T` makes it a task, `Ctrl-L` drops the link back. Whether
    /// the key was one of those.
    pub(super) fn notebook_capture_key(&mut self, keypress: KeyPress) -> bool {
        let Some(NotebookPrompt { kind: NotebookPromptKind::Capture { to, task, link }, .. }) = &mut self.notebook.prompt else { return false };
        if keypress.code == KeyCode::Named(FenixNamedKey::Tab) {
            *to = match to {
                CaptureTo::Journal => CaptureTo::Inbox,
                CaptureTo::Inbox => CaptureTo::Journal,
            };
            return true;
        }
        if keypress == KeyPress::char('t').with_ctrl() {
            *task = !*task;
            return true;
        }
        if keypress == KeyPress::char('l').with_ctrl() {
            *link = None;
            return true;
        }
        false
    }

    /// Files a capture.
    pub(super) fn notebook_capture_done(&mut self, to: CaptureTo, task: bool, link: Option<String>, text: &str) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        let now = chrono::Local::now();
        let line = fenix_notebook::templates::capture_line(text, task, link.as_deref(), now.time());
        let id = match to {
            CaptureTo::Journal => {
                let date = now.date_naive();
                let template = self.journal_template(date);
                self.notebook().and_then(|nb| nb.ensure_day(date, || template).ok()).map(|(id, _)| id)
            }
            CaptureTo::Inbox => {
                let Some(nb) = self.notebook() else { return };
                match nb.by_name("Inbox") {
                    Some(e) => Some(e.id.clone()),
                    None => nb.create(NbKind::Note, "Inbox", "# Inbox\n\nThings captured to sort out later.\n").ok(),
                }
            }
        };
        let Some(id) = id else {
            self.set_error("couldn't open the notebook to capture into");
            return;
        };
        self.notebook_flush(&id);
        let before = self.notebook.book.as_ref().and_then(|nb| nb.get(&id)).map(|e| e.text.clone()).unwrap_or_default();
        let after = fenix_notebook::templates::append_to_log(&before, &line);
        self.notebook_write_entry(&id, &after);
        self.set_message(format!("captured to {}", to.label()));
    }

    // ------------------------------------------------------------------
    // Finding
    // ------------------------------------------------------------------

    /// `SPC n /`: the query, then what matches.
    pub(crate) fn notebook_search(&mut self) {
        if self.notebook().is_none() {
            return;
        }
        self.notebook.prompt = Some(NotebookPrompt { kind: NotebookPromptKind::Search, input: String::new() });
        self.wake_caret();
    }

    /// Every line `query` matches, as a picker.
    pub(super) fn notebook_search_results(&mut self, text: &str) {
        let Some(nb) = self.notebook() else { return };
        let _ = nb.rescan();
        let q = Query::parse(text);
        let results = query::search(nb, &q, 20);
        let mut candidates = Vec::new();
        for (id, hits) in &results {
            let Some(e) = nb.get(id) else { continue };
            for hit in hits {
                let label = match hit.line {
                    Some(l) => format!("{} {}  {:>4}  {}", e.kind.tag(), e.display_name(), l + 1, hit.text),
                    None => format!("{} {}", e.kind.tag(), e.display_name()),
                };
                candidates.push(fenix_picker::Candidate::new(label, NotebookPick::Hit { id: id.clone(), line: hit.line }));
            }
        }
        if candidates.is_empty() {
            self.set_message(format!("nothing in the notebook matches {text}"));
            return;
        }
        let found = results.len();
        self.completion = None;
        self.enter_picker(ActivePicker::Notebook { title: "FOUND", picker: fenix_picker::PickerState::new(candidates) });
        self.set_message(format!("{found} match{} -- typing narrows them", if found == 1 { "es" } else { "" }));
    }

    /// Opens an entry at a line.
    pub(super) fn notebook_open_at(&mut self, id: &str, line: Option<usize>) {
        self.open_notebook_entry(id);
        if let Some(l) = line {
            self.notebook_place_cursor(l, 0);
        }
    }

    /// `SPC n #`: every tag, most used first.
    pub(crate) fn notebook_tags(&mut self) {
        let Some(nb) = self.notebook() else { return };
        let tags = nb.tags();
        if tags.is_empty() {
            self.set_message("no tags yet -- #word in a note, or t on the notebook page");
            return;
        }
        let candidates = tags.into_iter().map(|(t, n)| fenix_picker::Candidate::new(format!("#{t}  ({n})"), NotebookPick::Filter(format!("tag:{t}")))).collect();
        self.completion = None;
        self.enter_picker(ActivePicker::Notebook { title: "TAG", picker: fenix_picker::PickerState::new(candidates) });
    }

    /// The notebook page, filtered.
    pub(super) fn notebook_page_filtered(&mut self, filter: &str) {
        self.open_notebook_page();
        let id = self.focused_buffer_id();
        if let Some(PageModel::Notebook(p)) = self.pages.get_mut(&id).map(|s| {
            s.stale = true;
            &mut s.model
        }) {
            p.filter.clear();
            p.filter.text = filter.to_string();
        }
    }

    /// `SPC n p`: the notes about the project you're in.
    pub(crate) fn notebook_project_notes(&mut self) {
        match self.notebook_project() {
            Some(name) => self.notebook_page_filtered(&format!("project:{name}")),
            None => self.set_error("no project here -- open a file in one first"),
        }
    }

    // ------------------------------------------------------------------
    // Tasks
    // ------------------------------------------------------------------

    /// `SPC n t`: the checkbox under the cursor, as an agenda task (once;
    /// the line gets a mark that it went).
    pub(crate) fn notebook_to_agenda(&mut self) {
        let (line, _) = self.open().buffer.line_col(&self.cursor());
        let text = self.open().buffer.line(line).to_string();
        let text = text.trim_end_matches(['\n', '\r']).to_string();
        let Some(task) = fenix_notebook::meta::task_on(&text, line) else {
            self.set_error("SPC n t sends a checkbox line (- [ ] ...) to the agenda");
            return;
        };
        if text.contains("→ agenda") {
            self.set_message("that one's in the agenda already");
            return;
        }
        let title: String = task.text.split(" · ").next().unwrap_or(&task.text).trim().to_string();
        let id = self.agenda_store.create_task(title.clone(), String::new(), fenix_agenda::Priority::Medium, None);
        if let Some(path) = self.open().buffer.path().map(Path::to_path_buf) {
            self.agenda_store.set_code(id, Some(fenix_agenda::CodeRef { path, line: line + 1 }));
        }
        self.agenda_save_and_refresh();
        let end = self.open().buffer.line_start_char(line) + self.open().buffer.line_len(line);
        let (buffer, _) = self.focused_buffer_and_cursor_mut();
        let mut at = Cursor::at_start();
        at.char_idx = end;
        buffer.insert_str(&mut at, " → agenda");
        self.set_message(format!("added to the agenda: {title}"));
    }

    // ------------------------------------------------------------------
    // Journal days
    // ------------------------------------------------------------------

    /// The date of the focused journal day, if it is one.
    fn focused_day(&self) -> Option<chrono::NaiveDate> {
        let path = self.open().buffer.path()?;
        self.notebook.book.as_ref()?.by_path(path)?.date()
    }

    /// `SPC m a` in a journal day: its agenda list, as the agenda is now.
    pub(crate) fn journal_refresh_agenda(&mut self) {
        let Some(date) = self.focused_day() else {
            self.set_error("SPC m a refreshes a journal day's agenda list");
            return;
        };
        let fresh = self.journal_template(date);
        let section = |text: &str| -> Option<String> {
            let start = text.find("## Agenda")?;
            let rest = &text[start..];
            let end = rest[3..].find("\n## ").map(|e| e + 4).unwrap_or(rest.len());
            Some(rest[..end].trim_end().to_string())
        };
        let text = self.open().buffer.text();
        let new = match (section(&text), section(&fresh)) {
            (Some(old), Some(new)) => text.replacen(&old, &new, 1),
            (Some(old), None) => text.replacen(&old, "## Agenda\n\nNothing due.", 1),
            (None, Some(new)) => match text.find("\n## ") {
                Some(at) => format!("{}\n\n{new}\n{}", &text[..at].trim_end(), &text[at..]),
                None => format!("{}\n\n{new}\n", text.trim_end()),
            },
            (None, None) => {
                self.set_message("nothing due in the agenda");
                return;
            }
        };
        self.notebook_replace_focused_text(&new);
        self.set_message("agenda list refreshed");
    }

    /// Replaces the focused buffer's text (as one edit).
    fn notebook_replace_focused_text(&mut self, text: &str) {
        let (buffer, _) = self.focused_buffer_and_cursor_mut();
        if buffer.text() != text {
            let end = buffer.len_chars();
            let mut scratch = Cursor::at_start();
            buffer.replace_range(&mut scratch, 0, end, text);
        }
    }

    /// `SPC m g` in a journal day: what you committed that day, in every
    /// known project.
    pub(crate) fn journal_commits(&mut self) {
        let Some(date) = self.focused_day() else {
            self.set_error("SPC m g adds a journal day's commits");
            return;
        };
        let mut roots: Vec<PathBuf> = self.known_projects.roots().to_vec();
        if let Some(r) = &self.project_root {
            if !roots.contains(r) {
                roots.push(r.clone());
            }
        }
        let since = date.format("%Y-%m-%d 00:00").to_string();
        let until = (date + chrono::Duration::days(1)).format("%Y-%m-%d 00:00").to_string();
        let mut lines = Vec::new();
        for root in roots {
            let git = |args: &[&str]| -> Option<String> {
                let mut cmd = std::process::Command::new("git");
                cmd.arg("-C").arg(&root).args(args);
                #[cfg(windows)]
                {
                    use std::os::windows::process::CommandExt;
                    cmd.creation_flags(0x0800_0000);
                }
                let out = cmd.output().ok()?;
                out.status.success().then(|| String::from_utf8_lossy(&out.stdout).to_string())
            };
            let Some(email) = git(&["config", "user.email"]).map(|e| e.trim().to_string()).filter(|e| !e.is_empty()) else { continue };
            let Some(log) = git(&["log", "--all", "--no-merges", &format!("--author={email}"), &format!("--since={since}"), &format!("--until={until}"), "--format=%h %s"]) else { continue };
            let name = root.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            for l in log.lines().filter(|l| !l.trim().is_empty()) {
                let (hash, subject) = l.split_once(' ').unwrap_or((l, ""));
                lines.push(format!("- {name} `{hash}` {subject}"));
            }
        }
        if lines.is_empty() {
            self.set_message(format!("no commits of yours on {}", date.format("%b %-d")));
            return;
        }
        let text = self.open().buffer.text();
        let block = format!("## Commits\n\n{}\n", lines.join("\n"));
        let new = match text.find("## Commits") {
            Some(at) => {
                let rest = &text[at..];
                let end = rest[3..].find("\n## ").map(|e| e + 4).unwrap_or(rest.len());
                format!("{}{}{}", &text[..at], block, &rest[end..])
            }
            None => format!("{}\n\n{block}", text.trim_end()),
        };
        self.notebook_replace_focused_text(&new);
        self.set_message(format!("{} commit{} added", lines.len(), if lines.len() == 1 { "" } else { "s" }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> (tempfile::TempDir, App) {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::with_file(None);
        app.config.notebook_folder = Some(dir.path().join("nb"));
        // Never the agenda of the Fenix you use.
        app.agenda_path = dir.path().join("agenda.json");
        app.agenda_store = Default::default();
        (dir, app)
    }

    #[test]
    fn captures_go_under_todays_log_or_the_inbox() {
        let (_dir, mut app) = app();
        app.notebook();
        app.notebook_capture_done(CaptureTo::Journal, false, Some("[[fenix:src/a.c:3]]".into()), "retry is 3");
        let today = chrono::Local::now().date_naive();
        let day = app.notebook.book.as_ref().unwrap().day(today).unwrap().text.clone();
        assert!(day.contains("## Log\n\n- `") && day.contains("` retry is 3 · [[fenix:src/a.c:3]]\n"), "{day}");
        app.notebook_capture_done(CaptureTo::Inbox, true, None, "call OBSW");
        let inbox = app.notebook.book.as_ref().unwrap().by_name("Inbox").unwrap().text.clone();
        assert!(inbox.ends_with("- [ ] call OBSW\n"), "{inbox}");
    }

    #[test]
    fn a_checkbox_goes_to_the_agenda_once() {
        let (_dir, mut app) = app();
        app.notebook_create_note("Meeting", "# Meeting\n- [ ] Raise FSW-212\n");
        app.notebook_place_cursor(1, 0);
        let before = app.agenda_store.tasks.len();
        app.notebook_to_agenda();
        assert_eq!(app.agenda_store.tasks.len(), before + 1);
        assert_eq!(app.agenda_store.tasks.last().unwrap().title, "Raise FSW-212");
        assert_eq!(app.open().buffer.text(), "# Meeting\n- [ ] Raise FSW-212 → agenda\n");
        app.notebook_to_agenda();
        assert_eq!(app.agenda_store.tasks.len(), before + 1);
    }

    #[test]
    fn search_lists_matching_lines() {
        let (_dir, mut app) = app();
        app.notebook_create_note("Bench", "---\ntags: [bench]\n---\nretry limit is 3\n");
        app.notebook_create_note("Other", "nothing\n");
        app.notebook_search_results("retry tag:bench");
        let Some(ActivePicker::Notebook { title, picker }) = &app.active_picker else { panic!("the results") };
        assert_eq!(*title, "FOUND");
        assert_eq!(picker.len(), 1);
        assert!(picker.selected().unwrap().label.contains("retry limit is 3"));
    }
}
