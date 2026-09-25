//! The host half of the project pages -- the new-project wizard (`SPC p
//! c`), the hub (`SPC p p`), the doctor (`SPC p h`) and the settings page
//! (`SPC p ,`). Each is a `BufferKind::Page` buffer whose text is laid out
//! by its own pure module (`project_wizard`, `project_hub`, ...); this
//! file owns the buffers, routes keys to them, draws their colours and
//! does everything that touches the world: running commands and checks
//! off the UI thread, writing settings, and opening projects -- each in a
//! workspace of its own (see `open_project`).

use super::projects::kind_color;
use super::*;
use crate::git_log::{self, GitLog};
use crate::git_rebase::{self, RebasePage};
use crate::git_request::{self, RequestPage};
use crate::agenda_page::{self, AgendaPage};
use crate::jira_page::{self, JiraPage};
use crate::settings_page::{self, SettingsPage};
use crate::snippets_page::{self, SnippetsPage};
use crate::review_inbox::{self, Inbox};
use crate::review_page::{self, ReviewPage};
use crate::git_status::{self, GitStatus};
use crate::page::{Key, Page, Role as PageRole};
use crate::project_doctor::{self, DoctorPage, Fixing};
use crate::project_hub::{self, Hub, HubProject};
use crate::project_settings::{self, Settings};
use crate::project_wizard::{self, RunWork, Status, Wizard};
use fenix_project::doctor::{self, Check, EditorFix, FixAction, Health, Probe};
use fenix_project::vcs::GitSummary;
use std::collections::BTreeMap;
use std::io::BufRead;
use std::sync::{Arc, Mutex};

pub(super) enum PageModel {
    Wizard(Wizard),
    Hub(Hub),
    Doctor(DoctorPage),
    Git(Box<GitStatus>),
    Log(Box<GitLog>),
    Rebase(Box<RebasePage>),
    Request(Box<RequestPage>),
    UserSettings(Box<SettingsPage>),
    Snippets(Box<SnippetsPage>),
    Inbox(Box<Inbox>),
    Review(Box<ReviewPage>),
    Agenda(Box<AgendaPage>),
    Jira(Box<JiraPage>),
}

pub(super) struct PageState {
    pub(super) model: PageModel,
    pub(super) page: Page,
    /// The pane width `page` was laid out for, and whether the model has
    /// changed since.
    cols: usize,
    pub(super) stale: bool,
    /// Bumped for every background job started, so what a superseded job
    /// sends (a step since retried, a check since restarted) is dropped.
    generation: u64,
    started: Option<Instant>,
    /// The minute it was last laid out in, for pages that show a clock.
    minute: i64,
}

impl PageState {
    fn new(model: PageModel) -> Self {
        PageState { model, page: Page::default(), cols: 0, stale: true, generation: 0, started: None, minute: 0 }
    }

    /// Whether a text field on the page has the keyboard -- then every
    /// key is the page's, Space and Ctrl-V included.
    fn typing(&self) -> bool {
        match &self.model {
            PageModel::Wizard(w) => w.editing.is_some(),
            PageModel::Hub(h) => h.filtering || h.editing_group.is_some(),
            PageModel::Doctor(_) => false,
            PageModel::Git(g) => g.typing(),
            PageModel::Log(l) => l.typing(),
            PageModel::Request(r) => r.editing.is_some(),
            PageModel::UserSettings(p) => p.typing(),
            PageModel::Snippets(p) => p.typing(),
            PageModel::Agenda(p) => p.typing(),
            PageModel::Jira(p) => p.typing(),
            PageModel::Rebase(_) | PageModel::Inbox(_) | PageModel::Review(_) => false,
        }
    }

    /// Whether Space means something on the focused row.
    fn claims_space(&self) -> bool {
        self.typing()
            || match &self.model {
                PageModel::Wizard(w) => w.claims_space(),
                PageModel::Request(r) => r.field == git_request::Field::Draft,
                PageModel::UserSettings(p) => p.claims_space(),
                PageModel::Agenda(p) => p.claims_space(),
                PageModel::Jira(p) => p.claims_space(),
                _ => false,
            }
    }

    fn type_text(&mut self, text: &str) {
        match &mut self.model {
            PageModel::Wizard(w) => w.type_text(text),
            PageModel::Hub(h) => {
                let target = if let Some(group) = &mut h.editing_group { group } else { &mut h.filter };
                target.extend(text.chars().filter(|c| !c.is_control()));
            }
            PageModel::Doctor(_) => {}
            PageModel::Git(g) => g.type_text(text),
            PageModel::Log(l) => l.type_text(text),
            PageModel::Request(r) => r.paste(text),
            PageModel::UserSettings(p) => p.paste(text),
            PageModel::Snippets(p) => p.paste(text),
            PageModel::Agenda(p) => p.paste(text),
            PageModel::Jira(p) => p.paste(text),
            PageModel::Rebase(_) | PageModel::Inbox(_) | PageModel::Review(_) => {}
        }
        self.stale = true;
    }
}

/// What a background job sends back to its page.
#[derive(Debug)]
pub enum PageEvent {
    Output { buffer: BufferId, generation: u64, line: String },
    Finished { buffer: BufferId, generation: u64, result: Result<(), String> },
    Checks { buffer: BufferId, generation: u64, checks: Vec<Check> },
    HubInfo { buffer: BufferId, root: PathBuf, git: Option<GitSummary>, health: (Health, usize) },
    Subprojects { buffer: BufferId, root: PathBuf, found: Vec<(PathBuf, fenix_project::ProjectKind)> },
    GitSnapshot { buffer: BufferId, snapshot: Box<git_status::Snapshot> },
    GitDiff { buffer: BufferId, section: git_status::Section, path: String, diff: git_status::DiffState },
    GitDone { buffer: BufferId, label: String, result: Result<String, String> },
    /// A question for the page to ask -- an undo's preview.
    GitConfirm { buffer: BufferId, confirm: Result<git_status::Confirm, String> },
    /// A file's blame, read off the UI thread.
    Blame { path: PathBuf, edits: u64, result: Result<Vec<fenix_git::BlameLine>, String> },
    GitLogData { buffer: BufferId, data: Box<git_log::LogData> },
    GitLogFiles { buffer: BufferId, hash: String, files: Vec<(char, String)> },
    GitLogDiff { buffer: BufferId, hash: String, path: String, diff: git_status::DiffState },
    /// Where the focused file's repository stands, for the modeline.
    ChromeGit(Box<super::git_editor::ChromeGit>),
    /// A background fetch finished.
    AutoFetched { ok: bool },
    InboxData { buffer: BufferId, result: Result<Vec<review_inbox::Entry>, String> },
    ReviewData { buffer: BufferId, result: Result<Box<review_page::ReviewData>, String> },
    ReviewSince { buffer: BufferId, result: Result<Vec<review_page::FileView>, String> },
    ReviewDone { buffer: BufferId, label: String, after: super::review_host::After, result: Result<(), String> },
    ReviewLog { buffer: BufferId, name: String, result: Result<String, String> },
    /// The request already open for the new request page's branch.
    RequestExisting { buffer: BufferId, result: Result<Option<fenix_forge::MergeRequest>, String> },
    /// The new request was opened -- with a word about what didn't go
    /// on it -- or wasn't.
    RequestOpened { buffer: BufferId, result: Result<(fenix_forge::MergeRequest, Option<String>), String> },
    /// The status page's branch's request.
    GitRequest { buffer: BufferId, result: Result<Option<crate::git_status::RequestLine>, String> },
    /// A line for the settings page: a token's test came back.
    SettingsNote { buffer: BufferId, note: (String, bool) },
    /// A search on the Jira page came back.
    JiraIssues { buffer: BufferId, jql: String, result: Result<Vec<fenix_jira::IssueSummary>, String> },
    JiraDetail { buffer: BufferId, key: String, result: Result<fenix_jira::IssueDetail, String> },
    /// Choices for an issue, fetched: its transitions, or priorities.
    JiraOffer { buffer: BufferId, title: String, key: String, result: Result<Vec<(String, jira_page::Choice)>, String> },
    JiraTypes { buffer: BufferId, project: String, types: Result<Vec<fenix_jira::IssueType>, String>, priorities: Vec<String> },
    JiraFields { buffer: BufferId, type_id: String, result: Result<Vec<fenix_jira::CreateField>, String> },
    /// A change to an issue went through, or didn't.
    JiraDone { buffer: BufferId, key: String, result: Result<String, String> },
}

pub(super) type Sender = Arc<dyn Fn(PageEvent) + Send + Sync>;

/// Sends every line `stream` prints. Progress bars rewrite a line with
/// `\r`; only the last state of each is worth a line in the log.
fn forward_lines(stream: impl std::io::Read, buffer: BufferId, generation: u64, send: &Sender) {
    for line in std::io::BufReader::new(stream).lines().map_while(Result::ok) {
        let line = line.rsplit('\r').find(|part| !part.trim().is_empty()).unwrap_or_default().trim_end().to_string();
        if !line.is_empty() {
            send(PageEvent::Output { buffer, generation, line });
        }
    }
}

/// Runs `command` to completion, sending each line it prints (both
/// streams) and then how it ended.
fn run_command(mut command: std::process::Command, buffer: BufferId, generation: u64, send: Sender) {
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(e) => {
            send(PageEvent::Finished { buffer, generation, result: Err(format!("couldn't start: {e}")) });
            return;
        }
    };
    let stderr = child.stderr.take().map(|stream| {
        let send = send.clone();
        std::thread::spawn(move || forward_lines(stream, buffer, generation, &send))
    });
    if let Some(stdout) = child.stdout.take() {
        forward_lines(stdout, buffer, generation, &send);
    }
    if let Some(thread) = stderr {
        let _ = thread.join();
    }
    let result = match child.wait() {
        Ok(status) if status.success() => Ok(()),
        Ok(status) => Err(match status.code() {
            Some(code) => format!("exited with code {code}"),
            None => "was stopped".to_string(),
        }),
        Err(e) => Err(e.to_string()),
    };
    send(PageEvent::Finished { buffer, generation, result });
}

/// What the doctor asks of the world, answered the way the rest of
/// Fenix would: Arduino's tools through the Arduino integration's own
/// search, everything else on PATH; MIB roots from `config.ini`.
pub(super) struct AppProbe {
    tools: fenix_embedded::Tools,
    mib_roots: Vec<PathBuf>,
}

fn same_dir(a: &Path, b: &Path) -> bool {
    let canon = |p: &Path| fenix_lsp::normalize(std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf()));
    canon(a) == canon(b)
}

impl Probe for AppProbe {
    fn locate(&self, program: &str) -> Option<PathBuf> {
        match program {
            "arduino-cli" => self.tools.arduino_cli.clone(),
            "clangd" => self.tools.clangd.clone().or_else(|| doctor::which(program)),
            "arduino-language-server" => self.tools.arduino_language_server.clone(),
            _ => doctor::which(program),
        }
    }

    fn run(&self, program: &Path, args: &[&str], dir: &Path) -> Option<(bool, String)> {
        doctor::run_program(program, args, dir)
    }

    fn mib_registered(&self, dir: &Path) -> bool {
        self.mib_roots.iter().any(|root| same_dir(root, dir))
    }
}

/// "5 min", "3 h", "2 d" since Unix time `then`.
fn age_since(then: i64) -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(then);
    let mins = (now - then).max(0) / 60;
    match mins {
        0 => "now".to_string(),
        1..=59 => format!("{mins} min"),
        60..=1439 => format!("{} h", mins / 60),
        1440..=20159 => format!("{} d", mins / 1440),
        _ => format!("{} w", mins / 10080),
    }
}

impl App {
    pub(super) fn app_probe(&self) -> AppProbe {
        AppProbe { tools: self.embedded_tools(), mib_roots: self.config.mib_roots.iter().map(|(_, path)| path.clone()).collect() }
    }

    /// Where a program a page runs is -- `AppProbe::locate`, for a command
    /// about to be spawned.
    fn locate_program(&self, program: &str) -> Option<PathBuf> {
        self.app_probe().locate(program)
    }

    /// What a page is called in its tab, the modeline and `SPC b b`.
    pub(super) fn page_title(&self, id: BufferId) -> String {
        match self.pages.get(&id).map(|s| &s.model) {
            Some(PageModel::Wizard(_)) | None => "*new project*".to_string(),
            Some(PageModel::Hub(_)) => "*projects*".to_string(),
            Some(PageModel::Doctor(d)) => format!("*doctor: {}*", d.name),
            Some(PageModel::Git(g)) => format!("*git: {}*", g.name),
            Some(PageModel::Log(l)) => format!("*log: {}*", l.name),
            Some(PageModel::Rebase(r)) => format!("*rebase: {}*", r.branch),
            Some(PageModel::Request(r)) => format!("*new request: {}*", r.branch),
            Some(PageModel::Snippets(_)) => "*snippets*".to_string(),
            Some(PageModel::UserSettings(p)) => match &p.scope {
                settings_page::Scope::You => "*settings*".to_string(),
                settings_page::Scope::Project { name, .. } => format!("*settings: {name}*"),
            },
            Some(PageModel::Inbox(i)) => format!("*reviews: {}*", i.project),
            Some(PageModel::Review(r)) => format!("*review: {}*", r.reference()),
            Some(PageModel::Agenda(_)) => "*agenda*".to_string(),
            Some(PageModel::Jira(_)) => "*jira*".to_string(),
        }
    }

    pub(super) fn is_page_buffer(&self, id: BufferId) -> bool {
        self.pages.contains_key(&id)
    }

    /// The open page of one kind, if there is one -- the wizard, the hub
    /// and the settings page each exist at most once.
    pub(super) fn find_page(&self, is: impl Fn(&PageModel) -> bool) -> Option<BufferId> {
        self.pages.iter().find(|(_, s)| is(&s.model)).map(|(id, _)| *id)
    }

    /// Opens `model` as a page in the focused pane.
    pub(super) fn open_page(&mut self, model: PageModel) -> BufferId {
        let buffer = self.buffers.open_page("");
        self.pages.insert(buffer, PageState::new(model));
        self.show_page(buffer);
        buffer
    }

    pub(super) fn show_page(&mut self, buffer: BufferId) {
        self.open_buffer_in_focused_pane(buffer);
        self.main_view = MainView::Editor;
        self.refresh_project_root();
        if let Some(state) = self.pages.get_mut(&buffer) {
            state.stale = true;
        }
        self.wake_caret();
    }

    /// Closes page `id` and its buffer; panes showing it go back to the
    /// buffer used before.
    pub(super) fn close_page(&mut self, id: BufferId) {
        self.pages.remove(&id);
        self.buffers.close(id);
        let fallback = self.buffers.mru().first().copied().unwrap_or_else(|| self.buffers.open_scratch());
        self.repoint_panes_showing(id, fallback);
        self.refresh_project_root();
    }

    /// Runs `job` off the UI thread, handing it a way to send events back
    /// -- or, with no event loop (tests), runs it here and applies what
    /// it sent.
    pub(super) fn page_spawn(&mut self, job: impl FnOnce(Sender) + Send + 'static) {
        match self.event_proxy.clone() {
            Some(proxy) => {
                let proxy = Mutex::new(proxy);
                let send: Sender = Arc::new(move |event| {
                    if let Ok(proxy) = proxy.lock() {
                        let _ = proxy.send_event(FenixUserEvent::Page(event));
                    }
                });
                std::thread::spawn(move || job(send));
            }
            None => {
                let events = Arc::new(Mutex::new(Vec::new()));
                let sink = events.clone();
                job(Arc::new(move |event| sink.lock().unwrap().push(event)));
                let events = std::mem::take(&mut *events.lock().unwrap());
                for event in events {
                    self.apply_page_event(event);
                }
            }
        }
    }

    /// Starts `step` for page `buffer` in `dir`; its output and end come
    /// back as page events under a new generation.
    fn page_run(&mut self, buffer: BufferId, step: fenix_project::template::Step, dir: &Path) {
        let Some(state) = self.pages.get_mut(&buffer) else { return };
        state.generation += 1;
        state.started = Some(Instant::now());
        let generation = state.generation;
        let mut step = step;
        if let Some(located) = self.locate_program(&step.spec.executable) {
            step.spec.executable = located.display().to_string();
        }
        match step.command(dir) {
            Ok(command) => self.page_spawn(move |send| run_command(command, buffer, generation, send)),
            Err(e) => self.apply_page_event(PageEvent::Finished { buffer, generation, result: Err(e.to_string()) }),
        }
    }

    /// Lays page `id` out for a pane `cols` cells wide, if it changed or
    /// the pane did, and puts the cursor on its focused row.
    pub(super) fn ensure_page_layout(&mut self, id: BufferId, pane: fenix_window::WindowId, cols: usize) {
        // What the agenda page reads besides the store, worked out before
        // the page is borrowed.
        let agenda = matches!(self.pages.get(&id).map(|s| &s.model), Some(PageModel::Agenda(_)))
            .then(|| (self.agenda_worklog_rows(), self.agenda_sync_label(), self.agenda_worklog_round()));
        let Some(state) = self.pages.get_mut(&id) else { return };
        // A running clock's minutes move on by themselves.
        let minute = chrono::Local::now().timestamp() / 60;
        if agenda.is_some() && self.agenda_store.active_timer.is_some() && state.minute != minute {
            state.minute = minute;
            state.stale = true;
        }
        if !state.stale && state.cols == cols {
            return;
        }
        state.page = match &state.model {
            PageModel::Agenda(p) => {
                let (worklogs, sync, round) = agenda.unwrap_or_default();
                let ctx = agenda_page::Ctx { store: &self.agenda_store, now: chrono::Local::now(), categories: &self.config.agenda_categories, worklogs: &worklogs, round, sync };
                agenda_page::layout(p, &ctx, cols)
            }
            PageModel::Jira(p) => {
                let in_agenda: HashSet<String> = self.agenda_store.tasks.iter().filter_map(|t| t.jira_key().map(str::to_string)).collect();
                let ctx = jira_page::Ctx {
                    in_agenda: &in_agenda,
                    people: &self.config.jira_users,
                    server: self.config.jira_base_url.clone().unwrap_or_default(),
                    today: chrono::Local::now().date_naive(),
                };
                jira_page::layout(p, &ctx, cols)
            }
            PageModel::Wizard(w) => project_wizard::layout(w, cols),
            PageModel::Hub(h) => project_hub::layout(h, cols),
            PageModel::Doctor(d) => project_doctor::layout(d, cols),
            PageModel::Git(g) => git_status::layout(g, cols),
            PageModel::Log(l) => git_log::layout(l, cols),
            PageModel::Rebase(r) => git_rebase::layout(r, cols),
            PageModel::Request(r) => git_request::layout(r, cols),
            PageModel::UserSettings(p) => settings_page::layout(p, cols),
            PageModel::Snippets(p) => snippets_page::layout(p, cols),
            PageModel::Inbox(i) => review_inbox::layout(i, cols),
            PageModel::Review(r) => review_page::layout(r, cols),
        };
        state.cols = cols;
        state.stale = false;
        let text = state.page.text.clone();
        let (line, col) = state.page.cursor();
        let Some(ob) = self.buffers.get_mut(id) else { return };
        if ob.buffer.text() != text {
            let end = ob.buffer.len_chars();
            let mut scratch = Cursor::at_start();
            ob.buffer.replace_range(&mut scratch, 0, end, &text);
            ob.buffer.mark_saved();
            ob.buffer.drain_edits();
        }
        self.home_place_cursor(id, pane, line, col);
    }

    /// Keys a page claims while it's focused. While a field has the
    /// keyboard that's every key; otherwise the leader and `:` still reach
    /// Vim, so `SPC w` and friends work from a page.
    pub(super) fn page_key(&mut self, keypress: KeyPress) -> bool {
        let id = self.focused_buffer_id();
        let Some(state) = self.pages.get(&id) else { return false };
        // A leader sequence under way is the leader's, not the page's.
        if !self.vim.is_idle() || self.leader_matcher.is_pending() {
            return false;
        }
        let typing = state.typing();
        let claims_space = state.claims_space();
        if keypress == KeyPress::char('v').with_ctrl() && typing {
            if let Some(text) = self.clipboard_text() {
                if let Some(state) = self.pages.get_mut(&id) {
                    state.type_text(&text);
                }
            }
            return true;
        }
        let shift = self.modifiers.shift_key();
        let key = match (keypress.code, keypress.mods.ctrl) {
            (KeyCode::Char('c'), true) => Key::CtrlC,
            (KeyCode::Char('o'), true) if typing => Key::CtrlO,
            (_, true) => return false,
            (KeyCode::Char(' '), false) if claims_space => Key::Space,
            (KeyCode::Char(':'), false) if typing => Key::Char(':'),
            // The leader and the command line, while nothing's typed.
            (KeyCode::Char(' ' | ':'), false) => return false,
            (KeyCode::Char(c), false) => Key::Char(c),
            (KeyCode::Named(FenixNamedKey::Enter), _) => Key::Enter,
            (KeyCode::Named(FenixNamedKey::Tab), _) if shift => Key::BackTab,
            (KeyCode::Named(FenixNamedKey::Tab), _) => Key::Tab,
            (KeyCode::Named(FenixNamedKey::Escape), _) => Key::Escape,
            (KeyCode::Named(FenixNamedKey::Backspace), _) => Key::Backspace,
            (KeyCode::Named(FenixNamedKey::Up), _) => Key::Up,
            (KeyCode::Named(FenixNamedKey::Down), _) => Key::Down,
            (KeyCode::Named(FenixNamedKey::Left), _) => Key::Left,
            (KeyCode::Named(FenixNamedKey::Right), _) => Key::Right,
            _ => return false,
        };
        let agenda = matches!(self.pages.get(&id).map(|s| &s.model), Some(PageModel::Agenda(_)))
            .then(|| (self.agenda_worklog_rows(), self.agenda_sync_label(), self.agenda_worklog_round()));
        let Some(state) = self.pages.get_mut(&id) else { return false };
        state.stale = true;
        match &mut state.model {
            PageModel::Agenda(p) => {
                let (worklogs, sync, round) = agenda.unwrap_or_default();
                let ctx = agenda_page::Ctx { store: &self.agenda_store, now: chrono::Local::now(), categories: &self.config.agenda_categories, worklogs: &worklogs, round, sync };
                let action = p.key(key, &ctx);
                self.agenda_page_action(id, action);
            }
            PageModel::Jira(p) => {
                let in_agenda: HashSet<String> = self.agenda_store.tasks.iter().filter_map(|t| t.jira_key().map(str::to_string)).collect();
                let ctx = jira_page::Ctx {
                    in_agenda: &in_agenda,
                    people: &self.config.jira_users,
                    server: self.config.jira_base_url.clone().unwrap_or_default(),
                    today: chrono::Local::now().date_naive(),
                };
                let action = p.key(key, &ctx);
                self.jira_page_action(id, action);
            }
            PageModel::Wizard(w) => {
                let action = w.key(key);
                self.wizard_action(id, action);
            }
            PageModel::Hub(h) => {
                let action = h.key(key);
                self.hub_action(id, action);
            }
            PageModel::Doctor(d) => {
                let action = d.key(key);
                self.doctor_action(id, action);
            }
            PageModel::Git(g) => {
                let action = g.key(key);
                self.git_page_action(id, action);
            }
            PageModel::Log(l) => {
                let action = l.key(key);
                self.git_log_action(id, action);
            }
            PageModel::Rebase(r) => {
                let action = r.key(key);
                self.git_rebase_action(id, action);
            }
            PageModel::Request(r) => {
                let action = r.key(key);
                self.request_action(id, action);
            }
            PageModel::UserSettings(p) => {
                let action = p.key(key);
                self.user_settings_action(id, action);
            }
            PageModel::Snippets(p) => {
                let action = p.key(key);
                self.snippets_action(id, action);
            }
            PageModel::Inbox(i) => {
                let action = i.key(key);
                self.inbox_action(id, action);
            }
            PageModel::Review(r) => {
                let action = r.key(key);
                self.review_action(id, action);
            }
        }
        self.wake_caret();
        true
    }

    /// A background job's news for its page.
    pub(super) fn apply_page_event(&mut self, event: PageEvent) {
        match event {
            PageEvent::GitDone { buffer, .. } if matches!(self.pages.get(&buffer).map(|s| &s.model), Some(PageModel::Log(_))) => self.apply_git_log_event(event),
            PageEvent::GitDone { buffer, label, result } if matches!(self.pages.get(&buffer).map(|s| &s.model), Some(PageModel::Rebase(_))) => {
                self.apply_git_rebase_done(buffer, label, result)
            }
            event @ (PageEvent::GitSnapshot { .. } | PageEvent::GitDiff { .. } | PageEvent::GitDone { .. } | PageEvent::GitConfirm { .. }) => {
                self.apply_git_page_event(event)
            }
            event @ (PageEvent::GitLogData { .. } | PageEvent::GitLogFiles { .. } | PageEvent::GitLogDiff { .. }) => self.apply_git_log_event(event),
            PageEvent::Blame { path, edits, result } => self.apply_blame(path, edits, result),
            PageEvent::ChromeGit(state) => self.chrome_git = Some(*state),
            event @ PageEvent::SettingsNote { .. } => self.apply_settings_event(event),
            event @ (PageEvent::JiraIssues { .. }
            | PageEvent::JiraDetail { .. }
            | PageEvent::JiraOffer { .. }
            | PageEvent::JiraTypes { .. }
            | PageEvent::JiraFields { .. }
            | PageEvent::JiraDone { .. }) => self.apply_jira_event(event),
            event @ (PageEvent::RequestExisting { .. } | PageEvent::RequestOpened { .. } | PageEvent::GitRequest { .. }) => self.apply_request_event(event),
            event @ (PageEvent::InboxData { .. } | PageEvent::ReviewData { .. } | PageEvent::ReviewSince { .. } | PageEvent::ReviewDone { .. } | PageEvent::ReviewLog { .. }) => {
                self.apply_review_event(event)
            }
            PageEvent::AutoFetched { ok } => {
                if ok {
                    self.refresh_git_pages(false);
                }
            }
            PageEvent::Output { buffer, generation, line } => {
                let Some(state) = self.pages.get_mut(&buffer).filter(|s| s.generation == generation) else { return };
                state.stale = true;
                match &mut state.model {
                    PageModel::Wizard(w) => {
                        if let Some(run) = w.run.as_mut() {
                            if let Some(i) = run.running() {
                                run.push_output(i, line);
                            }
                        }
                    }
                    PageModel::Doctor(d) => d.push_output(line),
                    _ => {}
                }
            }
            PageEvent::Finished { buffer, generation, result } => {
                let Some(state) = self.pages.get_mut(&buffer).filter(|s| s.generation == generation) else { return };
                state.stale = true;
                let elapsed = state.started.map(|t| t.elapsed()).unwrap_or_default();
                match &mut state.model {
                    PageModel::Wizard(w) => {
                        let Some(run) = w.run.as_mut() else { return };
                        let Some(i) = run.running() else { return };
                        let ok = result.is_ok();
                        run.steps[i].status = match result {
                            Ok(()) => Status::Done(elapsed),
                            Err(e) => Status::Failed(e),
                        };
                        if ok {
                            self.wizard_run_next(buffer);
                        }
                    }
                    PageModel::Doctor(d) => {
                        let ok = result.is_ok();
                        let label = d.fixing.as_ref().and_then(|f| d.check(f.check)).and_then(|c| c.fix.as_ref()).map(|f| f.label.clone());
                        if let Some(label) = label {
                            d.last_fix = Some((label, result.clone()));
                        }
                        if let Some(fixing) = d.fixing.as_mut() {
                            fixing.result = Some(result);
                        }
                        if ok {
                            // The next queued fix, else look again.
                            let next = (!d.queue.is_empty()).then(|| d.queue.remove(0));
                            match next {
                                Some(i) => self.doctor_fix(buffer, i),
                                None => self.doctor_check(buffer),
                            }
                        } else {
                            d.queue.clear();
                        }
                    }
                    _ => {}
                }
            }
            PageEvent::Checks { buffer, generation, checks } => {
                let Some(state) = self.pages.get_mut(&buffer).filter(|s| s.generation == generation) else { return };
                state.stale = true;
                if let PageModel::Doctor(d) = &mut state.model {
                    let root = d.root.clone();
                    let health = (doctor::worst(&checks), checks.iter().filter(|c| c.health >= Health::Warn).count());
                    d.set_checks(checks);
                    d.fixing = None;
                    self.project_health.insert(root.clone(), health);
                    for state in self.pages.values_mut() {
                        if let PageModel::Hub(hub) = &mut state.model {
                            if let Some(p) = hub.projects.iter_mut().find(|p| p.root == root) {
                                p.health = Some(health);
                                state.stale = true;
                            }
                        }
                    }
                }
            }
            PageEvent::Subprojects { buffer, root, found } => {
                let Some(state) = self.pages.get_mut(&buffer) else { return };
                state.stale = true;
                if let PageModel::Hub(h) = &mut state.model {
                    h.set_subprojects(&root, found);
                    for p in h.projects.iter_mut().filter(|p| p.parent.is_some() && p.health.is_none()) {
                        p.health = self.project_health.get(&p.root).copied();
                    }
                }
            }
            PageEvent::HubInfo { buffer, root, git, health } => {
                self.project_health.insert(root.clone(), health);
                let Some(state) = self.pages.get_mut(&buffer) else { return };
                state.stale = true;
                if let PageModel::Hub(h) = &mut state.model {
                    if let Some(p) = h.projects.iter_mut().find(|p| p.root == root) {
                        p.last_commit_age = git.as_ref().and_then(|g| g.last_commit.as_ref()).map(|(_, t)| age_since(*t));
                        p.git = git;
                        p.health = Some(health);
                    }
                }
            }
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    /// The page's colours for the visible lines, in the shape syntax
    /// highlighting hands the renderer.
    pub(super) fn page_highlights(&self, id: BufferId, first_line: usize, rows: usize) -> Vec<(std::ops::Range<usize>, glyphon::Color)> {
        let (Some(state), Some(ob)) = (self.pages.get(&id), self.buffers.get(id)) else { return Vec::new() };
        let theme = self.theme;
        let color = |role: PageRole| match role {
            PageRole::Title | PageRole::Text => theme.fg,
            PageRole::Muted => theme.gutter_fg,
            PageRole::Accent => rgba_to_glyphon(theme.caret),
            PageRole::Good => theme.git_staged,
            PageRole::Warn => theme.git_modified,
            PageRole::Bad => theme.git_conflicted,
            PageRole::Kind(kind) => kind_color(kind, theme),
        };
        let last = first_line + rows;
        let mut ranges: Vec<(std::ops::Range<usize>, glyphon::Color)> = state
            .page
            .spans
            .iter()
            .filter(|s| s.line >= first_line && s.line <= last && s.line < ob.buffer.line_count())
            .map(|s| {
                let start = ob.buffer.line_start_char(s.line);
                let len = ob.buffer.line_len(s.line);
                let a = ob.buffer.char_to_byte(start + s.cols.start.min(len));
                let b = ob.buffer.char_to_byte(start + s.cols.end.min(len));
                (a..b, color(s.role))
            })
            .filter(|(r, _)| r.start < r.end)
            .collect();
        ranges.sort_by_key(|(r, _)| r.start);
        ranges
    }

    /// The page's backgrounds -- the focused row's tint and rail, a field
    /// being typed into -- and its hairlines, in Home's shapes.
    pub(super) fn page_backgrounds(&self, id: BufferId, first_line: usize, rows: usize) -> (home::BgSegments, home::HomeOverlay) {
        let Some(state) = self.pages.get(&id) else { return Default::default() };
        let theme = self.theme;
        let visible = |line: usize| (line >= first_line && line <= first_line + rows).then(|| line - first_line);
        let mut segments = Vec::new();
        let mut overlay = home::HomeOverlay::default();
        if let Some((line, cols)) = &state.page.focus {
            if let Some(row) = visible(*line) {
                segments.push((row, cols.start, cols.end, theme.hl_line));
                overlay.rail = Some((row, cols.start, 1));
            }
        }
        let panel = if theme.sidebar_bg != theme.bg { theme.sidebar_bg } else { theme.bg_modeline };
        for (line, cols) in &state.page.panels {
            if let Some(row) = visible(*line) {
                segments.push((row, cols.start, cols.end, panel));
            }
        }
        overlay.rules = state.page.rules.iter().filter_map(|(line, cols)| visible(*line).map(|row| (row, cols.start, cols.end))).collect();
        overlay.rule_color = home::home_rule(theme);
        (segments, overlay)
    }

    // --------------------------------------------------------------
    // Opening projects: a workspace each
    // --------------------------------------------------------------

    /// The file to land on in a project: the one you had open most
    /// recently there, else its main file, else its README.
    fn project_entry_file(&self, root: &Path) -> Option<PathBuf> {
        self.recent_files
            .paths()
            .iter()
            .find(|p| p.starts_with(root) && p.is_file())
            .cloned()
            .or_else(|| fenix_project::main_file(root, self.project_kind_of(root)))
            .or_else(|| Some(root.join("README.md")).filter(|p| p.is_file()))
    }

    /// Opens project `root` -- on `file` if given, else where you last
    /// were in it -- in a workspace of its own: the one it already has,
    /// or the current one if that isn't anyone's and holds nothing but
    /// Home or a page, or a new one named after it. With
    /// `workspace_per_project = false` it's the old behaviour: the file,
    /// or a find-file picker in the root.
    pub(super) fn open_project(&mut self, root: PathBuf, file: Option<PathBuf>, register: bool) {
        let root = std::fs::canonicalize(&root).map(fenix_lsp::normalize).unwrap_or(root);
        if register {
            self.known_projects.add(root.clone());
            if let Err(err) = self.known_projects.save() {
                eprintln!("fenix: couldn't save project history: {err}");
            }
        }
        let file = file.or_else(|| self.project_entry_file(&root));
        if self.config.workspace_per_project != Some(false) {
            let name = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| root.display().to_string());
            if let Some(i) = self.workspaces.workspaces.iter().position(|w| w.project.as_deref() == Some(root.as_path())) {
                self.workspaces.switch_to_index(i);
                if let Some(file) = &file {
                    self.open_file_from_picker(file);
                }
                self.main_view = MainView::Editor;
                self.refresh_project_root();
                self.wake_caret();
                return;
            }
            let reusable = {
                let ws = &self.workspaces.workspaces[self.workspaces.active_index()];
                let panes = ws.windows.windows();
                ws.project.is_none()
                    && panes.len() == 1
                    && ws.windows.content(panes[0]).and_then(|id| self.buffers.get(*id)).is_none_or(|ob| {
                        matches!(ob.kind, BufferKind::Dashboard | BufferKind::Page) || (ob.kind == BufferKind::Text && ob.buffer.path().is_none() && ob.buffer.len_chars() == 0)
                    })
            };
            if !reusable {
                let content = match &file {
                    Some(file) => self.buffers.open_path(file),
                    None => self.new_home_buffer(),
                };
                let cursor = self.buffers.get(content).map(|ob| ob.cursor).unwrap_or(Cursor::at_start());
                self.workspaces.new_workspace(content, cursor);
            }
            self.workspaces.rename_active(name);
            let active = self.workspaces.active_index();
            self.workspaces.workspaces[active].project = Some(root.clone());
        }
        match file {
            Some(file) => self.open_file_from_picker(&file),
            None => self.switch_to_project(root),
        }
        self.wake_caret();
    }

    // --------------------------------------------------------------
    // The wizard
    // --------------------------------------------------------------

    /// `SPC p c`: the new-project wizard, in the focused pane. A second
    /// press brings back the one already open, answers and all.
    pub(crate) fn cmd_project_new(&mut self) {
        self.project_new_with("");
    }

    /// `:project-new [template] [name] [key=value ...]`.
    pub(super) fn project_new_with(&mut self, args: &str) {
        if let Some(id) = self.find_page(|m| matches!(m, PageModel::Wizard(_))) {
            if args.is_empty() {
                self.show_page(id);
                return;
            }
            self.close_page(id);
        }
        let user_dir = fenix_project::template::user_templates_dir();
        let (templates, errors) = fenix_project::template::available_templates(user_dir.as_deref(), self.project_root.as_deref());
        let probe = self.app_probe();
        let mut found = BTreeMap::new();
        for program in templates.iter().flat_map(fenix_project::template::programs_needed).chain(["git".to_string()]) {
            if let std::collections::btree_map::Entry::Vacant(entry) = found.entry(program) {
                let located = probe.locate(entry.key()).is_some();
                entry.insert(located);
            }
        }
        // New projects land beside the current one, else in the home
        // folder.
        let parent = self.project_root.as_deref().and_then(Path::parent).map(Path::to_path_buf).or_else(dirs::home_dir).unwrap_or_default();
        let mut wizard = Wizard::new(templates, errors, found, &parent);
        if let Err(e) = wizard.preset(args) {
            self.set_error(e);
            return;
        }
        self.open_page(PageModel::Wizard(wizard));
    }

    fn wizard_action(&mut self, id: BufferId, action: project_wizard::Action) {
        use project_wizard::Action;
        match action {
            Action::None => {}
            Action::Close => {
                let running = matches!(self.pages.get(&id).map(|s| &s.model), Some(PageModel::Wizard(w)) if w.run.as_ref().is_some_and(|r| r.running().is_some()));
                if running {
                    self.set_error("a command is still running -- C-c stops after it");
                } else {
                    self.close_page(id);
                }
            }
            Action::Create => self.wizard_run_next(id),
            Action::Retry | Action::Skip => {
                if let Some(PageModel::Wizard(w)) = self.pages.get_mut(&id).map(|s| &mut s.model) {
                    if let Some(run) = w.run.as_mut() {
                        if let Some(i) = run.failed() {
                            run.steps[i].output.clear();
                            run.steps[i].status = if action == Action::Retry { Status::Pending } else { Status::Skipped };
                        }
                    }
                }
                self.wizard_run_next(id);
            }
            Action::Finish => self.wizard_finish(id),
            Action::BrowseParent => {
                let Some(PageModel::Wizard(w)) = self.pages.get(&id).map(|s| &s.model) else { return };
                // Start where it points now, or the nearest folder of it
                // that exists.
                let start = Path::new(w.parent.trim()).ancestors().find(|p| p.is_dir()).map(Path::to_path_buf).or_else(dirs::home_dir).unwrap_or_default();
                match ExplorerState::opened(&start) {
                    Ok(explorer) => {
                        self.explorer = Some(explorer);
                        self.explorer_purpose = ExplorerPurpose::PickWizardParent;
                        self.main_view = MainView::Explorer;
                    }
                    Err(e) => self.set_error(format!("couldn't list {} ({e})", start.display())),
                }
            }
        }
    }

    /// `S` in the explorer the wizard opened: that folder becomes where
    /// the project is created, and the wizard comes back.
    pub(super) fn wizard_parent_picked(&mut self, dir: &Path) {
        self.explorer = None;
        self.explorer_purpose = ExplorerPurpose::Browse;
        self.main_view = MainView::Editor;
        let Some(id) = self.find_page(|m| matches!(m, PageModel::Wizard(_))) else { return };
        if let Some(PageModel::Wizard(w)) = self.pages.get_mut(&id).map(|s| &mut s.model) {
            w.set_parent(dir);
        }
        self.show_page(id);
    }

    /// Starts the next pending step, if the run should go on: writing the
    /// files happens here and now, a command on its own thread. Once
    /// nothing's left and nothing failed, finishes.
    fn wizard_run_next(&mut self, id: BufferId) {
        let Some(state) = self.pages.get_mut(&id) else { return };
        state.stale = true;
        let PageModel::Wizard(w) = &mut state.model else { return };
        let Some(plan) = w.plan.clone() else { return };
        let Some(run) = w.run.as_mut() else { return };
        let Some(i) = run.next_pending() else {
            if run.finished() && !run.cancel_requested && run.failed().is_none() {
                self.wizard_finish(id);
            }
            return;
        };
        run.steps[i].status = Status::Running;
        match run.steps[i].work.clone() {
            work @ (RunWork::WriteFiles | RunWork::WriteAfterFiles) => {
                state.generation += 1;
                state.started = Some(Instant::now());
                let generation = state.generation;
                let result = if matches!(work, RunWork::WriteFiles) { plan.write_files() } else { plan.write_after_files() }.map(|_| ());
                self.apply_page_event(PageEvent::Finished { buffer: id, generation, result });
            }
            RunWork::Command(step) => self.page_run(id, step, &plan.dir),
        }
    }

    /// The run is over (or `o` on a failure): the template's editor-side
    /// hooks, then the project opened -- in its own workspace -- on its
    /// main file, and the wizard closed.
    fn wizard_finish(&mut self, id: BufferId) {
        let Some(state) = self.pages.remove(&id) else { return };
        let PageModel::Wizard(wizard) = state.model else { return };
        let Some(plan) = wizard.plan else {
            self.close_page(id);
            return;
        };
        let dir = std::fs::canonicalize(&plan.dir).map(fenix_lsp::normalize).unwrap_or_else(|_| plan.dir.clone());
        self.project_kinds.borrow_mut().clear();
        for hook in &plan.hooks {
            match hook {
                fenix_project::template::Hook::MibRoot { path, label } => {
                    let root = if path == "." { dir.clone() } else { dir.join(path) };
                    self.add_mib_root(root, label.clone());
                }
            }
        }
        let steps = wizard.run.as_ref().map(|r| r.steps.len()).unwrap_or(0);
        self.open_project(dir.clone(), plan.file_to_open(), wizard.register);
        self.close_page(id);
        self.refresh_home_data(false);
        self.set_message(format!("created {} -- {steps} step{}", dir.display(), if steps == 1 { "" } else { "s" }));
    }

    // --------------------------------------------------------------
    // The hub
    // --------------------------------------------------------------

    /// What the hub shows for each known project, before git and the
    /// doctor have reported in.
    fn hub_projects(&self) -> Vec<HubProject> {
        let open_work: Vec<&fenix_agenda::Task> =
            self.agenda_store.tasks.iter().filter(|t| !t.archived && t.status != fenix_agenda::Status::Done && t.jira_key().is_some()).collect();
        self.known_projects
            .roots()
            .iter()
            .map(|root| {
                let mut p = HubProject::new(root.clone(), self.project_kind_of(root));
                p.pinned = self.project_meta.is_pinned(root);
                p.group = self.project_meta.group(root).map(str::to_string);
                p.branch = fenix_project::vcs::git_branch(root);
                p.jira = fenix_project::meta::jira_key(root);
                if let Some(key) = &p.jira {
                    let prefix = format!("{key}-");
                    p.work = open_work.iter().filter(|t| t.jira_key().is_some_and(|k| k.starts_with(&prefix))).map(|t| format!("{} {}", t.jira_key().unwrap_or_default(), t.title)).collect();
                }
                let mut tasks: Vec<String> = fenix_project::tools::ProjectTools::read(root).map(|t| t.tasks.into_keys().collect()).unwrap_or_default();
                for task in fenix_tasks::discover_tasks(root) {
                    if !tasks.contains(&task.name) {
                        tasks.push(task.name);
                    }
                }
                p.tasks = tasks;
                p.health = self.project_health.get(root).copied();
                p
            })
            .collect()
    }

    /// `SPC p p`: the hub. A second press brings back the open one,
    /// refreshed.
    pub(crate) fn cmd_project_hub(&mut self) {
        let id = match self.find_page(|m| matches!(m, PageModel::Hub(_))) {
            Some(id) => {
                self.show_page(id);
                self.hub_reload(id);
                id
            }
            None => {
                let hub = Hub::new(self.hub_projects());
                self.open_page(PageModel::Hub(hub))
            }
        };
        self.hub_fetch(id);
    }

    /// Rebuilds the hub's rows from the project list, keeping what git
    /// and the doctor already said and the selection where it was.
    fn hub_reload(&mut self, id: BufferId) {
        let fresh = self.hub_projects();
        let Some(PageModel::Hub(hub)) = self.pages.get_mut(&id).map(|s| &mut s.model) else { return };
        let selected = hub.selected().map(|p| p.root.clone());
        let old = std::mem::take(&mut hub.projects);
        hub.projects = fresh
            .into_iter()
            .map(|mut p| {
                if let Some(o) = old.iter().find(|o| o.root == p.root) {
                    p.git = o.git.clone();
                    p.last_commit_age = o.last_commit_age.clone();
                    p.health = p.health.or(o.health);
                }
                p
            })
            .collect();
        // Subprojects of projects still listed, until the next scan.
        let listed: Vec<PathBuf> = hub.projects.iter().map(|p| p.root.clone()).collect();
        hub.projects.extend(old.into_iter().filter(|o| o.parent.as_ref().is_some_and(|parent| listed.contains(parent)) && !listed.contains(&o.root)));
        if let Some(root) = selected {
            hub.focus_root(&root);
        }
        if let Some(state) = self.pages.get_mut(&id) {
            state.stale = true;
        }
    }

    /// Git summaries and the doctor's quick pass for every project, off
    /// the UI thread, one project at a time.
    fn hub_fetch(&mut self, id: BufferId) {
        let Some(PageModel::Hub(hub)) = self.pages.get(&id).map(|s| &s.model) else { return };
        let projects: Vec<(PathBuf, fenix_project::ProjectKind)> =
            hub.projects.iter().filter(|p| p.exists && p.parent.is_none()).map(|p| (p.root.clone(), p.kind)).collect();
        let probe = self.app_probe();
        self.page_spawn(move |send| {
            let info = |root: PathBuf, kind: fenix_project::ProjectKind| {
                let git = fenix_project::vcs::git_summary(&root);
                let checks = doctor::diagnose(&root, kind, &probe, false);
                let health = (doctor::worst(&checks), checks.iter().filter(|c| c.health >= Health::Warn).count());
                send(PageEvent::HubInfo { buffer: id, root, git, health });
            };
            // Every project's own row first, then what's inside each.
            let mut found_all = Vec::new();
            for (root, kind) in &projects {
                let found = fenix_project::workspace::subprojects(root);
                send(PageEvent::Subprojects { buffer: id, root: root.clone(), found: found.clone() });
                info(root.clone(), *kind);
                found_all.extend(found);
            }
            for (root, kind) in found_all {
                info(root, kind);
            }
        });
    }

    fn hub_action(&mut self, id: BufferId, action: project_hub::Action) {
        use project_hub::Action;
        match action {
            Action::None => {}
            Action::Close => self.close_page(id),
            // Opened while the hub still holds its pane, so a workspace
            // holding nothing but the hub is reused, not left behind.
            Action::Open(root) => {
                let subproject = matches!(self.pages.get(&id).map(|s| &s.model), Some(PageModel::Hub(h)) if h.projects.iter().any(|p| p.root == root && p.parent.is_some()));
                self.open_project(root, None, !subproject);
                self.close_page(id);
            }
            Action::Add(path) => {
                self.register_project_dir(&path);
                self.hub_reload(id);
                let root = std::fs::canonicalize(&path).map(fenix_lsp::normalize).unwrap_or(path);
                if let Some(PageModel::Hub(hub)) = self.pages.get_mut(&id).map(|s| &mut s.model) {
                    hub.focus_root(&root);
                }
                self.hub_fetch(id);
            }
            Action::Browse => {
                self.close_page(id);
                self.picker_add_project_prompt();
            }
            Action::New => {
                self.close_page(id);
                self.cmd_project_new();
            }
            Action::Doctor(root) => self.open_doctor(root),
            Action::Settings(root) => {
                let name = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| root.display().to_string());
                self.open_settings_page(crate::settings_page::Scope::Project { root, name }, None);
            }
            Action::TogglePin(root) => {
                let pinned = !self.project_meta.is_pinned(&root);
                self.project_meta.set_pinned(&root, pinned);
                self.save_project_meta();
                self.hub_reload(id);
                if let Some(PageModel::Hub(hub)) = self.pages.get_mut(&id).map(|s| &mut s.model) {
                    hub.focus_root(&root);
                }
            }
            Action::SetGroup(root, group) => {
                self.project_meta.set_group(&root, &group);
                self.save_project_meta();
                self.hub_reload(id);
                if let Some(PageModel::Hub(hub)) = self.pages.get_mut(&id).map(|s| &mut s.model) {
                    hub.focus_root(&root);
                }
            }
            Action::Remove(root) => {
                self.known_projects.remove(&root);
                if let Err(err) = self.known_projects.save() {
                    eprintln!("fenix: couldn't save project history: {err}");
                }
                self.project_meta.forget(&root);
                self.save_project_meta();
                self.hub_reload(id);
                self.set_message(format!("removed {} from your projects -- the folder is untouched", root.display()));
            }
        }
    }

    fn save_project_meta(&mut self) {
        if let Err(err) = self.project_meta.save() {
            self.set_error(format!("couldn't save project pins and groups: {err}"));
        }
    }

    // --------------------------------------------------------------
    // The doctor
    // --------------------------------------------------------------

    /// `SPC p h`: the doctor for the focused buffer's project.
    pub(crate) fn cmd_project_doctor(&mut self) {
        match self.project_root.clone() {
            Some(root) => self.open_doctor(root),
            None => self.set_error("not in a project -- SPC p p, then h on one"),
        }
    }

    pub(super) fn open_doctor(&mut self, root: PathBuf) {
        let existing = self.pages.iter().find(|(_, s)| matches!(&s.model, PageModel::Doctor(d) if d.root == root)).map(|(id, _)| *id);
        let id = match existing {
            Some(id) => {
                self.show_page(id);
                id
            }
            None => {
                let kind = self.project_kind_of(&root);
                self.open_page(PageModel::Doctor(DoctorPage::new(root, kind)))
            }
        };
        self.doctor_check(id);
    }

    /// Runs the deep check for doctor page `id` off the UI thread.
    fn doctor_check(&mut self, id: BufferId) {
        let Some(state) = self.pages.get_mut(&id) else { return };
        let PageModel::Doctor(d) = &mut state.model else { return };
        let (root, kind) = (d.root.clone(), d.kind);
        d.checks = None;
        state.generation += 1;
        state.stale = true;
        let generation = state.generation;
        let probe = self.app_probe();
        self.page_spawn(move |send| {
            let checks = doctor::diagnose(&root, kind, &probe, true);
            send(PageEvent::Checks { buffer: id, generation, checks });
        });
    }

    /// Runs check `i`'s fix.
    fn doctor_fix(&mut self, id: BufferId, i: usize) {
        let Some(PageModel::Doctor(d)) = self.pages.get_mut(&id).map(|s| &mut s.model) else { return };
        let Some(fix) = d.check(i).and_then(|c| c.fix.clone()) else { return };
        let root = d.root.clone();
        match fix.action {
            FixAction::Run(step) => {
                d.fixing = Some(Fixing { check: i, output: Vec::new(), result: None });
                self.page_run(id, step, &root);
            }
            FixAction::Editor(fix) => {
                match fix {
                    EditorFix::RegisterMibRoot { path, label } => {
                        self.add_mib_root(path, label);
                        self.set_message("registered -- SPC m t finds its telecommands now");
                    }
                    EditorFix::UseLanguageServer { language, executable, args } => {
                        let result = fenix_project::tools::ProjectTools::read(&root).and_then(|mut tools| {
                            tools.lsp.insert(language, fenix_project::tools::CommandSpec::new(executable.display().to_string(), args));
                            tools.write(&root)
                        });
                        if let Some(PageModel::Doctor(d)) = self.pages.get_mut(&id).map(|s| &mut s.model) {
                            d.last_fix = Some(("use it for this project".to_string(), result.clone()));
                        }
                        match result {
                            Ok(()) => self.set_message("written to .fenix/tools.json -- :lsp-restart to start it"),
                            Err(e) => self.set_error(e),
                        }
                    }
                    EditorFix::PickPort | EditorFix::PickBoard => {
                        let Some(project) = self.embedded_project_at(&root) else {
                            self.set_error("not an Arduino sketch Fenix can drive -- is arduino-cli installed?");
                            return;
                        };
                        let root = project.root().to_path_buf();
                        if fix == EditorFix::PickPort {
                            self.set_message("looking for boards...");
                            self.embedded_spawn(move || EmbeddedEvent::Ports { root, then: None, result: project.ports() });
                        } else {
                            self.embedded_spawn(move || EmbeddedEvent::Boards { root, result: project.boards() });
                        }
                        // The picker has the keyboard now; r checks again
                        // once it's done.
                        return;
                    }
                }
                self.doctor_check(id);
            }
        }
    }

    fn doctor_action(&mut self, id: BufferId, action: project_doctor::Action) {
        use project_doctor::Action;
        match action {
            Action::None => {}
            Action::Close => self.close_page(id),
            Action::Recheck => self.doctor_check(id),
            Action::Fix(i) => self.doctor_fix(id, i),
            Action::FixAllSafe => {
                let Some(PageModel::Doctor(d)) = self.pages.get_mut(&id).map(|s| &mut s.model) else { return };
                let mut queue = d.safe_fixes();
                if queue.is_empty() {
                    return;
                }
                let first = queue.remove(0);
                d.queue = queue;
                self.doctor_fix(id, first);
            }
            Action::Open(path, line) => {
                self.open_file_from_picker(&path);
                self.jump_to_grep_match(&fenix_project::GrepMatch { path, line, col: 1, text: String::new() });
            }
            Action::CopyReport => {
                let Some(PageModel::Doctor(d)) = self.pages.get(&id).map(|s| &s.model) else { return };
                let report = d.report();
                match self.clipboard.as_mut().map(|c| c.set_text(report)) {
                    Some(Ok(())) => self.set_message("copied the doctor's report"),
                    _ => self.set_error("couldn't reach the clipboard"),
                }
            }
        }
    }

    // --------------------------------------------------------------
    // Settings
    // --------------------------------------------------------------

    /// `SPC p ,`: the focused buffer's project's settings.
    /// `SPC p ,`: the focused project's settings page, which leads to
    /// its own page (kind, group, tasks, launch) too.
    pub(crate) fn cmd_project_settings(&mut self) {
        match self.project_root.clone() {
            Some(_) => self.open_project_settings_page(),
            None => self.set_error("not in a project -- SPC p p, then , on one"),
        }
    }

    /// The project's own section of the settings page, read for `root`.
    pub(super) fn project_page_model(&self, root: &Path) -> Settings {
        let mut settings = Settings::new(root.to_path_buf(), fenix_project::detect_kind_from_files(root));
        settings.declared = fenix_project::declared_kind(root);
        settings.pinned = self.project_meta.is_pinned(root);
        settings.group = self.project_meta.group(root).unwrap_or_default().to_string();
        settings.jira = fenix_project::meta::jira_key(root).unwrap_or_default();
        settings
    }

    /// What the settings page's project section asked for, for the
    /// project at `root`.
    pub(super) fn project_page_action(&mut self, root: PathBuf, action: project_settings::Action) {
        use project_settings::Action;
        match action {
            Action::None | Action::Close => {}
            Action::SaveTools(tools) => match tools.write(&root) {
                Ok(()) => self.set_message("saved .fenix/tools.json"),
                Err(e) => self.set_error(e),
            },
            Action::SetKind(kind) => {
                match fenix_project::meta::set_kind(&root, kind) {
                    Ok(()) => self.set_message("saved .fenix/settings.toml"),
                    Err(e) => self.set_error(e.to_string()),
                }
                self.project_kinds.borrow_mut().clear();
            }
            Action::SetJira(key) => {
                if let Err(e) = fenix_project::meta::set_jira_key(&root, Some(key.as_str())) {
                    self.set_error(e.to_string());
                }
            }
            Action::SetGroup(group) => {
                self.project_meta.set_group(&root, &group);
                self.save_project_meta();
            }
            Action::SetPinned(pinned) => {
                self.project_meta.set_pinned(&root, pinned);
                self.save_project_meta();
            }
            Action::RunTask(name) => self.run_task(fenix_tasks::TaskDef { name, command: String::new(), args: Vec::new() }, root),
            Action::OpenRaw => {
                let path = root.join(".fenix").join("tools.json");
                if !path.exists() {
                    let _ = std::fs::create_dir_all(root.join(".fenix"));
                    let _ = std::fs::write(&path, "{}\n");
                }
                self.open_file_from_picker(&path);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh folder under the temp dir, removed when dropped.
    struct Scratch(PathBuf);
    impl Scratch {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("fenix-pages-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Scratch(fenix_lsp::normalize(std::fs::canonicalize(&dir).unwrap()))
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn press(app: &mut App, keys: &str) {
        for c in keys.chars() {
            let key = match c {
                '\n' => KeyPress::named(FenixNamedKey::Enter),
                '\u{1b}' => KeyPress::named(FenixNamedKey::Escape),
                '\t' => KeyPress::named(FenixNamedKey::Tab),
                c => KeyPress::char(c),
            };
            assert!(app.page_key(key), "the page claims {c:?}");
        }
    }

    fn page_text(app: &mut App) -> String {
        let (id, pane) = (app.focused_buffer_id(), app.focused_pane_id());
        app.ensure_page_layout(id, pane, 120);
        app.open().buffer.text()
    }

    /// An app whose known projects are exactly `roots`.
    fn app_with(roots: &[&Path]) -> App {
        let mut app = App::with_file(None);
        for root in app.known_projects.roots().to_vec() {
            app.known_projects.remove(&root);
        }
        for root in roots.iter().rev() {
            app.known_projects.add(root.to_path_buf());
        }
        app
    }

    /// The wizard open on `template`, pointed at `parent`, not registering.
    fn wizard_app(template: &str, parent: &Path, git: bool) -> App {
        let mut app = App::with_file(None);
        app.cmd_project_new();
        let id = app.focused_buffer_id();
        let Some(PageModel::Wizard(w)) = app.pages.get_mut(&id).map(|s| &mut s.model) else { panic!("no wizard") };
        w.parent = parent.display().to_string();
        (w.register, w.git_init, w.git_commit) = (false, git, false);
        let i = w.templates.iter().position(|t| t.id == template).unwrap();
        w.focus = w.fields().iter().position(|f| *f == project_wizard::Field::Template(i)).unwrap();
        app
    }

    fn wizard(app: &App) -> &Wizard {
        match app.pages.values().map(|s| &s.model).find(|m| matches!(m, PageModel::Wizard(_))) {
            Some(PageModel::Wizard(w)) => w,
            _ => panic!("no wizard"),
        }
    }

    #[test]
    fn spc_p_c_opens_the_wizard_as_a_page_and_lays_it_out() {
        let mut app = App::with_file(None);
        app.cmd_project_new();
        assert_eq!(app.open().kind, BufferKind::Page);
        assert_eq!(app.buffer_display_name(app.focused_buffer_id()), "*new project*");
        let id = app.focused_buffer_id();
        let text = page_text(&mut app);
        assert!(text.contains("New project") && text.contains("Python · uv"), "{text}");
        assert!(!app.page_highlights(id, 0, 40).is_empty());
        assert!(!app.page_key(KeyPress::char(' ')), "the leader still works from the page");
        app.leader_matcher.feed(KeyPress::char(' '));
        assert!(!app.page_key(KeyPress::char('G')), "a pending leader sequence gets its keys");
        app.leader_matcher.cancel();
        app.cmd_project_new();
        assert_eq!(app.focused_buffer_id(), id, "a second SPC p c brings back the same wizard");
    }

    #[test]
    fn a_space_typed_into_a_field_is_a_space_not_the_leader() {
        let dir = Scratch::new("space");
        let mut app = wizard_app("empty", &dir.0, false);
        press(&mut app, "\n");
        assert!(app.page_key(KeyPress::char(' ')));
        assert_eq!(wizard(&app).editing.as_deref(), Some(" "));
    }

    #[test]
    fn creating_a_project_writes_it_runs_git_and_opens_it_in_its_own_workspace() {
        let dir = Scratch::new("create");
        let mut app = wizard_app("empty", &dir.0, true);
        press(&mut app, "\nfresh\nG\n");
        assert_eq!(wizard(&app).step, project_wizard::Step::Review);
        press(&mut app, "\n");
        let project = dir.0.join("fresh");
        assert_eq!(std::fs::read_to_string(project.join("README.md")).unwrap(), "# fresh\n");
        assert!(project.join(".git").is_dir(), "git init ran");
        assert!(app.pages.is_empty(), "the wizard closed itself");
        assert!(app.open().buffer.path().is_some_and(|p| p.ends_with("README.md")), "and opened the README");
        assert_eq!(app.workspaces.active_name(), "fresh");
        assert_eq!(app.workspaces.workspaces[app.workspaces.active_index()].project.as_deref(), Some(project.as_path()));
        assert_eq!(app.workspaces.len(), 1, "Home's workspace was reused, not left behind empty");
    }

    #[test]
    fn a_failing_step_stops_the_run_and_skip_goes_on() {
        let dir = Scratch::new("fail");
        let mut app = wizard_app("empty", &dir.0, false);
        press(&mut app, "\nfresh\nG\n");
        let id = app.focused_buffer_id();
        if let Some(PageModel::Wizard(w)) = app.pages.get_mut(&id).map(|s| &mut s.model) {
            w.plan.as_mut().unwrap().steps.push(fenix_project::template::Step::new("definitely-not-a-program-xyz", &[]));
        }
        press(&mut app, "\n");
        let run = wizard(&app).run.as_ref().unwrap();
        assert!(matches!(run.steps.last().unwrap().status, Status::Failed(_)));
        assert!(dir.0.join("fresh/README.md").is_file(), "what was done stays");
        assert!(page_text(&mut app).contains("r retry"));
        press(&mut app, "s");
        assert!(app.pages.is_empty(), "skipping the last step finishes");
    }

    #[test]
    fn b_browses_for_the_folder_and_s_brings_it_back() {
        let dir = Scratch::new("browse");
        std::fs::create_dir_all(dir.0.join("inner")).unwrap();
        let mut app = wizard_app("empty", &dir.0, false);
        press(&mut app, "\nfresh\n");
        press(&mut app, "b");
        assert_eq!(app.main_view, MainView::Explorer);
        assert_eq!(app.explorer_purpose, ExplorerPurpose::PickWizardParent);
        assert_eq!(app.explorer.as_ref().unwrap().cwd, dir.0);
        app.wizard_parent_picked(&dir.0.join("inner"));
        assert_eq!(app.main_view, MainView::Editor);
        assert_eq!(app.open().kind, BufferKind::Page, "back on the wizard");
        assert_eq!(wizard(&app).target(), dir.0.join("inner").join("fresh"));
    }

    #[test]
    fn project_new_with_arguments_goes_straight_to_review() {
        let name = format!("fenix-preset-{}", std::process::id());
        let mut app = App::with_file(None);
        app.project_new_with(&format!("empty {name}"));
        let w = wizard(&app);
        assert_eq!(w.step, project_wizard::Step::Review);
        assert!(!w.target().exists(), "reviewing writes nothing");
        app.project_new_with("nope");
        assert!(app.modeline_pieces().1.contains("no template called nope"));
    }

    #[test]
    fn the_hub_lists_known_projects_and_opens_one_in_its_own_workspace() {
        let a = Scratch::new("hub-a");
        let b = Scratch::new("hub-b");
        std::fs::write(a.0.join("Cargo.toml"), "[package]").unwrap();
        std::fs::write(a.0.join("README.md"), "# a").unwrap();
        std::fs::write(b.0.join("pyproject.toml"), "").unwrap();
        let mut app = app_with(&[&a.0, &b.0]);
        app.cmd_project_hub();
        let hub_id = app.focused_buffer_id();
        let text = page_text(&mut app);
        let a_name = a.0.file_name().unwrap().to_string_lossy().into_owned();
        assert!(text.contains(&a_name) && text.contains("RS") && text.contains("PY"), "{text}");
        assert!(text.contains("no problems found") || text.contains("problem"), "the quick pass reported in:\n{text}");
        // Open the first (a).
        press(&mut app, "\n");
        assert!(!app.pages.contains_key(&hub_id), "the hub closes behind it");
        app.cmd_project_hub();
        assert_eq!(app.buffer_display_name(app.focused_buffer_id()), "*projects*");
        press(&mut app, "q");
        assert_eq!(app.workspaces.active_name(), a_name);
        assert!(app.open().buffer.path().is_some_and(|p| p.ends_with("README.md")));
        // Back to b: a new workspace; back to a: its own again.
        app.cmd_project_hub();
        press(&mut app, "j\n");
        assert_eq!(app.workspaces.len(), 2);
        app.cmd_project_hub();
        let a_row = app.pages.values().find_map(|s| match &s.model {
            PageModel::Hub(h) => h.rows().iter().position(|(_, i)| h.projects[*i].root == a.0),
            _ => None,
        });
        for _ in 0..a_row.unwrap() {
            press(&mut app, "j");
        }
        press(&mut app, "\n");
        assert_eq!(app.workspaces.len(), 2, "no third workspace: a already had one");
        assert_eq!(app.workspaces.active_name(), a_name);
    }

    #[test]
    fn opening_from_a_hub_in_its_own_workspace_reuses_that_workspace() {
        let a = Scratch::new("hub-reuse");
        std::fs::write(a.0.join("README.md"), "# a").unwrap();
        let mut app = app_with(&[&a.0]);
        // A second, empty workspace holding only the hub.
        let home = app.focused_buffer_id();
        app.workspaces.new_workspace(home, Cursor::at_start());
        app.cmd_project_hub();
        let before = app.workspaces.len();
        press(&mut app, "
");
        assert_eq!(app.workspaces.len(), before, "the hub's workspace became the project's");
        assert!(app.workspaces.workspaces.iter().all(|w| w.windows.windows().iter().all(|p| w.windows.content(*p).is_some_and(|id| app.buffers.get(*id).is_some()))));
    }

    #[test]
    fn a_registered_monorepo_shows_its_subprojects_which_open_without_being_registered() {
        let mono = Scratch::new("hub-mono");
        std::fs::create_dir_all(mono.0.join(".git")).unwrap();
        std::fs::write(mono.0.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        std::fs::write(mono.0.join("Cargo.toml"), "[workspace]\n").unwrap();
        std::fs::create_dir_all(mono.0.join("crates/core/src")).unwrap();
        std::fs::write(mono.0.join("crates/core/Cargo.toml"), "[package]\nname = \"core\"\n").unwrap();
        std::fs::write(mono.0.join("crates/core/src/lib.rs"), "").unwrap();
        let mut app = app_with(&[&mono.0]);
        app.cmd_project_hub();
        let text = page_text(&mut app);
        assert!(text.contains("crates/core"), "the subproject is listed:\n{text}");
        press(&mut app, "j\n");
        assert!(app.open().buffer.path().is_some_and(|p| p.ends_with("lib.rs")), "its main file");
        assert_eq!(app.workspaces.active_name(), "core");
        assert_eq!(app.known_projects.roots(), std::slice::from_ref(&mono.0), "not added to the list");
        // Its language server runs for the whole Cargo workspace.
        let lib = mono.0.join("crates/core/src/lib.rs");
        assert_eq!(tool_sessions::lsp_root_for_path(&lib), mono.0);
        assert_eq!(tool_sessions::root_for_path(&lib), mono.0.join("crates/core"), "tasks and the modeline stay the member's");
    }

    #[test]
    fn hub_pins_and_groups_are_kept_and_remove_needs_two_presses() {
        let a = Scratch::new("pin-a");
        let b = Scratch::new("pin-b");
        let mut app = app_with(&[&a.0, &b.0]);
        app.project_meta = fenix_project::meta::ProjectMeta::load_or_default(a.0.join("meta.json"));
        app.cmd_project_hub();
        press(&mut app, "jP");
        assert!(app.project_meta.is_pinned(&b.0));
        assert!(page_text(&mut app).contains("PINNED"));
        press(&mut app, "g");
        for c in "Labs\n".chars() {
            press(&mut app, &c.to_string());
        }
        assert_eq!(app.project_meta.group(&b.0), Some("Labs"));
        assert!(fenix_project::meta::ProjectMeta::load_or_default(a.0.join("meta.json")).is_pinned(&b.0), "saved to disk");
        press(&mut app, "d");
        assert!(app.known_projects.roots().contains(&b.0), "one d only asks");
        press(&mut app, "d");
        assert!(!app.known_projects.roots().contains(&b.0));
        assert!(b.0.is_dir(), "the folder itself is untouched");
    }

    #[test]
    fn the_doctor_checks_the_focused_project_and_runs_a_safe_fix() {
        let dir = Scratch::new("doctor");
        std::fs::write(dir.0.join("Cargo.toml"), "[package]").unwrap();
        let mut app = App::with_file(None);
        app.open_doctor(dir.0.clone());
        let text = page_text(&mut app);
        assert!(text.contains("TOOLCHAIN") && text.contains("cargo"), "{text}");
        assert!(text.contains("not a repository") && text.contains("git init"), "{text}");
        // Focus the git row and fix it.
        let id = app.focused_buffer_id();
        let row = match &app.pages[&id].model {
            PageModel::Doctor(d) => d.order().iter().position(|&i| d.check(i).unwrap().label == "git").unwrap(),
            _ => unreachable!(),
        };
        if let PageModel::Doctor(d) = &mut app.pages.get_mut(&id).unwrap().model {
            d.focus = row;
        }
        press(&mut app, "f");
        assert!(dir.0.join(".git").is_dir(), "git init ran");
        let text = page_text(&mut app);
        assert!(!text.contains("not a repository"), "it checked again afterwards:\n{text}");
        assert!(matches!(app.project_health.get(&dir.0), Some((_, _))), "the result feeds the hub's dot");
    }

    #[test]
    fn settings_edits_write_tools_json_and_the_projects_settings() {
        let dir = Scratch::new("settings");
        std::fs::write(dir.0.join("pyproject.toml"), "").unwrap();
        let mut app = App::with_file(None);
        app.project_meta = fenix_project::meta::ProjectMeta::load_or_default(dir.0.join("meta.json"));
        // SPC p , on the project, then its own section at the top.
        app.open_settings_page(crate::settings_page::Scope::Project { root: dir.0.clone(), name: "settings".into() }, None);
        press(&mut app, "\tk\n");
        let text = page_text(&mut app);
        assert!(text.contains("detect: Python") && text.contains("TASKS · 0"), "{text}");
        // Kind -> declared Python, then Jira.
        press(&mut app, "l");
        assert!(std::fs::read_to_string(dir.0.join(".fenix/settings.toml")).unwrap().contains("kind = \"python\""));
        press(&mut app, "jjj\nfnx\n");
        assert_eq!(fenix_project::meta::jira_key(&dir.0).as_deref(), Some("FNX"));
        // Add a task.
        press(&mut app, "ja");
        for c in "test = uv run pytest -q\n".chars() {
            let key = if c == ' ' { KeyPress::char(' ') } else if c == '\n' { KeyPress::named(FenixNamedKey::Enter) } else { KeyPress::char(c) };
            assert!(app.page_key(key));
        }
        let tools = fenix_project::tools::ProjectTools::read(&dir.0).unwrap();
        assert_eq!(tools.tasks["test"].args, ["run", "pytest", "-q"]);
    }

    #[test]
    fn a_sketch_with_no_history_opens_on_its_ino() {
        let dir = Scratch::new("entry");
        let sketch = dir.0.join("Blinky");
        std::fs::create_dir_all(&sketch).unwrap();
        std::fs::write(sketch.join("Blinky.ino"), "void setup() {}").unwrap();
        let mut app = App::with_file(None);
        app.open_project(sketch.clone(), None, false);
        assert!(app.open().buffer.path().is_some_and(|p| p.ends_with("Blinky.ino")), "not a find-file picker");
        assert_eq!(app.main_view, MainView::Editor);
    }

    #[test]
    fn with_workspace_per_project_off_opening_stays_in_the_current_workspace() {
        let dir = Scratch::new("no-ws");
        std::fs::write(dir.0.join("README.md"), "# s").unwrap();
        let mut app = App::with_file(None);
        app.config.workspace_per_project = Some(false);
        let before = app.workspaces.active_name().to_string();
        app.open_project(dir.0.clone(), None, false);
        assert_eq!(app.workspaces.active_name(), before);
        assert!(app.open().buffer.path().is_some_and(|p| p.ends_with("README.md")));
    }
}
