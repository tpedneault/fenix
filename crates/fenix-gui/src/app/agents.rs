//! The coding agents (`SPC a`): Fenix's side of the Agent Client
//! Protocol. `fenix_acp::Hub` decides everything about sessions; this
//! carries out what it asks -- agents started and stopped, messages
//! sent, files read and written through Fenix's buffers, permission
//! questions shown -- and shows the threads: the page (`agent_page`)
//! beside your code, with its prompt, a Vim buffer, under it.
//!
//! Files: an agent that writes through Fenix (`fs/write_text_file`)
//! writes into the buffer as one undo step, saved as it's made unless
//! `agents.write = "buffer"`, and never over your unsaved edits without
//! asking. An agent that writes to disk itself -- Claude Code's adapter
//! -- announces each edit first; the file is snapshotted then, and
//! watched while the turn runs, so a clean buffer follows it at once.
//! The snapshots are the baselines the change review starts from.

use super::*;
use crate::agent_page::{self, AgentPage};
use fenix_acp::catalog::{self, CatalogEntry};
use fenix_acp::client::{AcpClient, AcpEvent, Launch, Log, LogSide, RpcError};
use fenix_acp::hub::{Attention, ConnId, ConnState, Effect, Hub, Link, ThreadId, Turn};
use fenix_acp::protocol::{code, ClientCapabilities, ClientRequest, ConfigValue, ContentBlock, ListedSession, McpServer, PermissionKind, PermissionOption, ToolCallFields};
use fenix_acp::store::Store;
use fenix_acp::thread::{NoticeKind, PermissionCard, PermissionState};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, HashSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

/// How often files an agent is changing are looked at during a turn.
const WATCH_EVERY: Duration = Duration::from_millis(250);
/// How often threads changed during a turn are saved.
const SAVE_EVERY: Duration = Duration::from_secs(5);
/// What `SPC h a`'s view is called.
const LOG_VIEW: &str = "*agent connections*";

/// What a `SPC a` picker's row does.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Pick {
    /// A new thread with this agent.
    NewThread(String),
    /// Show this thread in the panel.
    Thread(ThreadId),
    Mode(String),
    Config { id: String, value: ConfigValue },
    Login { conn: ConnId, method: String },
    /// A session the agent has that Fenix doesn't: make it a thread.
    Import { agent: String, root: PathBuf, session: ListedSession },
    /// `agents.save_before_prompt = "ask"`: save first, or don't.
    Send { save: bool },
    Restart(ConnId),
    /// `@` in the prompt: what follows the `@`.
    Mention(String),
    /// `/` at the start of the prompt: the agent's command.
    Command(String),
    /// `SPC a h`: the thread goes on with this agent.
    HandOff(String),
    Delete(ThreadId),
    Nothing,
    /// The ACP registry's agents, to install one.
    Registry,
    Install(Box<fenix_acp::registry::RegistryAgent>),
    Uninstall(String),
    /// `SPC a s`: an API key to keep in the keychain, by its variable.
    Secret(String),
}

/// An agent's write over unsaved edits of yours, waiting for your
/// answer.
struct Guard {
    /// Its card's key in the thread (`fenix-guard:<n>`).
    key: String,
    conn: ConnId,
    thread: ThreadId,
    id: Value,
    path: PathBuf,
    content: String,
}

pub(crate) struct AgentHost {
    pub(super) hub: Hub,
    clients: HashMap<ConnId, AcpClient>,
    /// Each connection's log, kept after it stops (`SPC h a`).
    logs: BTreeMap<ConnId, (String, Arc<Mutex<Log>>)>,
    store: Option<Store>,
    /// Threads read from the store, by id (the rest aren't loaded yet).
    loaded: HashSet<ThreadId>,
    dirty: HashSet<ThreadId>,
    last_save: Instant,
    /// The panel: its page and its prompt.
    pub(super) page: Option<BufferId>,
    pub(super) composer: Option<BufferId>,
    /// The pane the panel was opened beside: where files it opens go.
    code_pane: Option<fenix_window::WindowId>,
    /// The panes given the dock's titles, to take them back from one that
    /// shows something else now.
    pub(super) titled: Vec<fenix_window::WindowId>,
    /// The focus was in the prompt at the last frame.
    pub(super) in_prompt: bool,
    /// What each thread's files were before the agent changed them, by
    /// absolute path: `None` for a file that didn't exist.
    pub(super) baselines: HashMap<ThreadId, BTreeMap<PathBuf, Option<String>>>,
    /// Files announced for change, looked at while a turn runs.
    watched: BTreeMap<PathBuf, Option<DiskFingerprint>>,
    next_watch: Instant,
    guards: Vec<Guard>,
    next_guard: u64,
    /// Without an event loop (tests), what the agents sent waits here.
    inbox: Arc<Mutex<VecDeque<(ConnId, u32, AcpEvent)>>>,
    /// A prompt held while `agents.save_before_prompt` asks.
    held: Option<(ThreadId, Vec<ContentBlock>)>,
    /// Where `Effect::Listed` goes: a picker to import from.
    listing: bool,
    /// Files changed during a turn with nothing to compare them to.
    pub(super) unknown: HashMap<ThreadId, BTreeSet<PathBuf>>,
    /// What git said was changed when each running turn started.
    turn_start: HashMap<ThreadId, HashSet<String>>,
    /// Threads with a turn under way, and since when: to tell when one
    /// starts or ends, and for the modeline.
    turning: HashMap<ThreadId, Instant>,
    /// Files agents changed that you haven't kept or rejected yet, across
    /// threads, as of the last turn's end or review (for the modeline).
    pub(super) to_review: usize,
    /// Follow mode (`SPC a f`), and where it last went.
    following: bool,
    followed: Option<(PathBuf, Option<u32>)>,
    /// `@selection`'s, from `SPC a e`.
    pub(super) selection: Option<context::Selection>,
    /// Pictures pasted into the prompt, to go with it.
    pub(super) images: Vec<ContentBlock>,
    /// Prompts sent since Fenix started, oldest first, for Up in the
    /// prompt.
    pub(super) sent: Vec<String>,
    /// The earlier prompt Up/Down put in the prompt: its place in the
    /// history, and its text, while it's left as it was.
    pub(super) recalled: Option<(usize, String)>,
    /// The thread writing a commit message (`SPC a c`).
    pub(super) commit_message: Option<ThreadId>,
    /// Commands agents run through Fenix, by terminal id.
    pub(super) terminals: HashMap<String, terminals::AgentTerminal>,
    pub(super) next_terminal: u64,
    /// Terminal logins under way: the connection, the terminal's buffer.
    pub(super) logins: Vec<(ConnId, BufferId)>,
    /// The registry's worker, while it reads or installs.
    pub(super) fetched: Option<std::sync::mpsc::Receiver<install::Fetched>>,
    /// The agents extensions brought (`agent-provide`), by extension and
    /// agent id.
    pub(super) brought: BTreeMap<(String, String), fenix_acp::hub::AgentSpec>,
    /// Extensions' questions (`agents.ask`), by the thread asked in.
    pub(super) asks: HashMap<ThreadId, extensions::Ask>,
    /// What `agents.ask` answered, in tests (no extension runs then).
    #[cfg(test)]
    pub(super) replies: Vec<(String, u64, Result<String, String>)>,
}

impl AgentHost {
    pub(crate) fn new() -> Self {
        let prefix = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| format!("{:x}", d.as_millis())).unwrap_or_else(|_| "t".into());
        let mut hub = Hub::new(prefix, env!("CARGO_PKG_VERSION"));
        hub.set_capabilities(ClientCapabilities::fenix());
        AgentHost {
            hub,
            clients: HashMap::new(),
            logs: BTreeMap::new(),
            store: fenix_storage::paths::state_file("agents").map(Store::new),
            loaded: HashSet::new(),
            dirty: HashSet::new(),
            last_save: Instant::now(),
            page: None,
            composer: None,
            code_pane: None,
            titled: Vec::new(),
            in_prompt: false,
            baselines: HashMap::new(),
            watched: BTreeMap::new(),
            next_watch: Instant::now(),
            guards: Vec::new(),
            next_guard: 1,
            inbox: Arc::new(Mutex::new(VecDeque::new())),
            held: None,
            listing: false,
            unknown: HashMap::new(),
            turn_start: HashMap::new(),
            turning: HashMap::new(),
            to_review: 0,
            following: false,
            followed: None,
            selection: None,
            images: Vec::new(),
            sent: Vec::new(),
            recalled: None,
            commit_message: None,
            terminals: HashMap::new(),
            next_terminal: 1,
            logins: Vec::new(),
            fetched: None,
            brought: BTreeMap::new(),
            asks: HashMap::new(),
            #[cfg(test)]
            replies: Vec::new(),
        }
    }

    /// The thread the panel shows.
    pub(super) fn shown(&self, pages: &HashMap<BufferId, pages::PageState>) -> Option<ThreadId> {
        let id = self.page?;
        match pages.get(&id).map(|s| &s.model) {
            Some(PageModel::Agent(p)) => Some(p.thread.clone()),
            _ => None,
        }
    }

    /// Lays the panel's page out.
    pub(super) fn layout(&self, page: &AgentPage, cols: usize) -> crate::page::Page {
        let Some(view) = self.hub.view(&page.thread) else { return crate::page::Page::default() };
        let mut status = self.status_of(&page.thread);
        status.terminals = self.terminals.iter().filter(|(_, t)| t.thread == page.thread).map(|(id, t)| {
            let (tail, exit) = t.tail(12);
            (id.clone(), agent_page::TermView { line: t.line.clone(), tail, exit })
        }).collect();
        agent_page::layout(page, view.thread, view.link, view.turn, &status, cols)
    }

    fn status_of(&self, id: &ThreadId) -> agent_page::Status {
        let Some(view) = self.hub.view(id) else { return agent_page::Status::default() };
        let agent = view
            .connection
            .and_then(|c| c.info.as_ref().and_then(|i| i.display_name().map(str::to_string)).or_else(|| Some(c.agent.name.clone())))
            .or_else(|| self.hub.agent(&view.meta.agent).map(|a| a.name.clone()))
            .unwrap_or_else(|| view.meta.agent.clone());
        let (state, role) = match (view.link, view.turn, view.connection.map(|c| &c.state)) {
            (Link::ReadOnly(why), ..) => (format!("read-only: {why}"), Some(crate::page::Role::Warn)),
            (_, _, Some(ConnState::NeedsAuth)) => ("wants a login".into(), Some(crate::page::Role::Warn)),
            (_, _, Some(ConnState::Dead { reason, .. })) => (format!("stopped: {reason}"), Some(crate::page::Role::Bad)),
            _ if view.thread.pending_permissions() > 0 => ("waiting on you".into(), Some(crate::page::Role::Warn)),
            (_, Turn::Cancelling { .. }, _) => ("cancelling…".into(), Some(crate::page::Role::Warn)),
            (_, Turn::Prompting { .. }, _) => ("working…".into(), Some(crate::page::Role::Accent)),
            (Link::Attaching { .. }, ..) => ("connecting…".into(), Some(crate::page::Role::Muted)),
            (Link::Live { .. }, ..) => ("ready".into(), Some(crate::page::Role::Good)),
            (Link::Detached, ..) => ("not started".into(), Some(crate::page::Role::Muted)),
        };
        let logins = view.connection.filter(|c| c.state == ConnState::NeedsAuth).and_then(|c| c.info.as_ref()).map(|i| i.auth_methods.iter().map(|m| (m.id.clone(), m.name.clone())).collect()).unwrap_or_default();
        agent_page::Status { agent, state, state_role: role, logins, queued: view.queued, terminals: HashMap::new() }
    }

    /// Reads the store's threads for `root` into the hub, newest first,
    /// at most `limit`.
    fn load_threads(&mut self, root: &Path, limit: usize) -> Vec<ThreadId> {
        let Some(store) = &self.store else { return Vec::new() };
        let mut ids = Vec::new();
        for entry in store.index().into_iter().filter(|e| e.root == root).take(limit) {
            if !self.loaded.contains(&entry.id) && self.hub.view(&entry.id).is_none() {
                if let Ok(saved) = store.load(&entry.id) {
                    self.hub.restore(saved.meta, saved.thread);
                    if let Some(baselines) = saved.extra.get("baselines").and_then(Value::as_object) {
                        let map = baselines.iter().map(|(k, v)| (PathBuf::from(k), v.as_str().map(str::to_string))).collect();
                        self.baselines.insert(entry.id.clone(), map);
                    }
                    if let Some(unknown) = saved.extra.get("unknown").and_then(Value::as_array) {
                        self.unknown.insert(entry.id.clone(), unknown.iter().filter_map(Value::as_str).map(PathBuf::from).collect());
                    }
                }
                self.loaded.insert(entry.id.clone());
            }
            if self.hub.view(&entry.id).is_some() {
                ids.push(entry.id);
            }
        }
        ids
    }

    fn save(&mut self, id: &ThreadId) {
        self.dirty.remove(id);
        let (Some(store), Some(view)) = (&self.store, self.hub.view(id)) else { return };
        if view.thread.entries.is_empty() {
            return;
        }
        let baselines: serde_json::Map<String, Value> = self.baselines.get(id).map(|b| b.iter().map(|(p, t)| (p.to_string_lossy().into_owned(), json!(t))).collect()).unwrap_or_default();
        let updated = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let unknown: Vec<String> = self.unknown.get(id).map(|u| u.iter().map(|p| p.to_string_lossy().into_owned()).collect()).unwrap_or_default();
        if let Err(e) = store.save(view.meta, view.thread, &json!({ "baselines": baselines, "unknown": unknown }), updated) {
            eprintln!("fenix: couldn't save agent thread {id}: {e}");
        }
    }
}

/// Whether `path` is under `root`, compared as Fenix opens files (no
/// `..` games: a path with one is refused). On Windows, as Windows reads
/// paths: in any letter case, either slash, with or without `\\?\` -- an
/// agent's own spelling of the project (`c:/users/...`) is the project.
fn within(root: &Path, path: &Path) -> bool {
    if path.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
        return false;
    }
    if cfg!(windows) {
        let spelt = |p: &Path| p.to_string_lossy().replace('/', "\\").trim_start_matches(r"\\?\").to_lowercase();
        let (root, path) = (spelt(root), spelt(path));
        let root = root.trim_end_matches('\\');
        path == root || path.starts_with(&format!("{root}\\"))
    } else {
        path.starts_with(root)
    }
}

impl App {
    // -----------------------------------------------------------------
    // The catalog and the hub's configuration

    /// The agents Fenix can start, from the built-ins, `agents.servers`
    /// and the environment.
    pub(super) fn agents_catalog(&self) -> Vec<CatalogEntry> {
        let configured = self.config.map_value("agents.servers").to_vec();
        let mut env: Vec<(String, String)> = self.config.map_value("agents.env").to_vec();
        for name in self.config.list_value("agents.secrets") {
            if let Some(value) = self.wasm.keychain().get(&format!("agents.{name}")) {
                env.push((name.clone(), value));
            }
        }
        catalog::catalog(&configured, &Self::agents_installed(), &self.agents_brought(), &env, &catalog::find_on_path)
    }

    /// The hub learns the agents, the MCP servers and the idle time from
    /// the settings, as they are now.
    fn agents_configure(&mut self) -> Vec<CatalogEntry> {
        let entries = self.agents_catalog();
        self.agents.hub.set_agents(entries.iter().map(|e| e.spec.clone()).collect());
        let mut servers = Vec::new();
        if self.config.bool_value("agents.mcp.fenix").unwrap_or(true) {
            if let Ok(exe) = std::env::current_exe() {
                servers.push(McpServer::Stdio { name: "fenix".into(), command: exe.to_string_lossy().into_owned(), args: vec!["--mcp".into(), "--thread".into(), "{thread}".into()], env: vec![] });
            }
        }
        for (name, line) in self.config.map_value("agents.mcp.servers") {
            let words = catalog::split_command_line(line);
            if let Some((command, args)) = words.split_first() {
                servers.push(McpServer::Stdio { name: name.clone(), command: command.clone(), args: args.to_vec(), env: vec![] });
            }
        }
        self.agents.hub.set_mcp_servers(servers);
        let minutes = self.config.int_value("agents.idle_minutes").unwrap_or(10).max(1) as u64;
        self.agents.hub.set_idle_timeout(Duration::from_secs(minutes * 60));
        entries
    }

    /// The agent a new thread talks to: `agents.default`, else the first
    /// one found.
    fn agents_default(&self, entries: &[CatalogEntry]) -> Option<String> {
        let wanted = self.config.text_value("agents.default").map(str::trim).filter(|s| !s.is_empty()).map(catalog::slug);
        match wanted {
            Some(id) if entries.iter().any(|e| e.spec.id == id) => Some(id),
            _ => entries.iter().find(|e| e.found).map(|e| e.spec.id.clone()),
        }
    }

    /// The project agents work in: the shown thread's, else the
    /// project's (the panel's page has no file of its own to tell it by).
    fn agents_root(&self) -> PathBuf {
        let focused = Some(self.focused_buffer_id());
        if focused == self.agents.page || focused == self.agents.composer {
            if let Some(root) = self.agents.shown(&self.pages).and_then(|t| self.agents.hub.meta(&t).map(|m| m.root.clone())) {
                return root;
            }
        }
        self.project_root.clone().or_else(|| std::env::current_dir().ok()).unwrap_or_else(|| PathBuf::from("."))
    }

    // -----------------------------------------------------------------
    // Commands

    /// `SPC a a`: the dock, on this project's last thread or a new one,
    /// the prompt ready to write in -- or, from the dock, put away.
    pub(crate) fn agents_open_panel(&mut self) {
        if let (Some(_), Some(_)) = self.agents_dock_panes() {
            if self.agents_is_dock_pane(self.focused_pane_id()) {
                self.agents_close_panel();
            } else {
                self.agents_focus_shown();
                self.agents_focus_composer();
            }
            return;
        }
        // Put away earlier: back on the thread it showed.
        if let Some(thread) = self.agents.shown(&self.pages) {
            self.agents_show(thread);
            self.agents_focus_composer();
            return;
        }
        let entries = self.agents_configure();
        let root = self.agents_root();
        let existing = self.agents.load_threads(&root, 20);
        let thread = existing.into_iter().next().or_else(|| self.agents.hub.thread_ids().iter().rev().find(|t| self.agents.hub.meta(t).is_some_and(|m| m.root == root)).cloned());
        let thread = match thread {
            Some(t) => t,
            None => {
                let Some(agent) = self.agents_default(&entries) else {
                    self.agents_none_found(&entries);
                    return;
                };
                self.agents.hub.new_thread(&agent, &root)
            }
        };
        self.agents_show(thread);
        self.agents_focus_composer();
    }

    fn agents_none_found(&mut self, entries: &[CatalogEntry]) {
        let names: Vec<&str> = entries.iter().map(|e| e.spec.program.as_str()).collect();
        self.set_error(format!("no agent found: install one ({}), or name yours in agents.servers", names.join(", ")));
    }

    /// `SPC a n`: a new thread with the default agent (`agents.default`,
    /// else the first found), in the dock, ready to write in. `SPC a p`
    /// starts one with another. (With `npx` about, every agent it can run
    /// counts as found: asking each time would be asking always.)
    pub(crate) fn agents_new_thread(&mut self) {
        let entries = self.agents_configure();
        let Some(agent) = self.agents_default(&entries) else {
            self.agents_none_found(&entries);
            return;
        };
        let id = self.agents.hub.new_thread(&agent, &self.agents_root());
        self.agents_show(id);
        self.agents_focus_composer();
    }

    /// `SPC a p`: the agents -- a new thread with one, a login when one
    /// wants it, a restart when one stopped.
    pub(crate) fn agents_pick_agent(&mut self) {
        let entries = self.agents_configure();
        let mut candidates = Vec::new();
        let default = self.agents_default(&entries);
        for e in &entries {
            if !e.found {
                continue;
            }
            let mark = if Some(&e.spec.id) == default.as_ref() { " (default)" } else { "" };
            candidates.push(fenix_picker::Candidate::new(format!("New thread with {}{mark}   {} {}", e.spec.name, e.spec.program, e.spec.args.join(" ")), Pick::NewThread(e.spec.id.clone())));
        }
        for (conn, c) in self.agents.hub.connections() {
            match &c.state {
                ConnState::NeedsAuth => {
                    for m in c.info.iter().flat_map(|i| &i.auth_methods) {
                        candidates.push(fenix_picker::Candidate::new(format!("Log in to {}: {}", c.agent.name, m.name), Pick::Login { conn, method: m.id.clone() }));
                    }
                }
                ConnState::Dead { reason, .. } => candidates.push(fenix_picker::Candidate::new(format!("Restart {} (stopped: {reason})", c.agent.name), Pick::Restart(conn))),
                _ => {}
            }
        }
        for e in entries.iter().filter(|e| !e.found) {
            candidates.push(fenix_picker::Candidate::new(format!("(not found) {}: install {}, or set agents.servers", e.spec.name, e.spec.program), Pick::NewThread(e.spec.id.clone())));
        }
        if !self.config.text_value("agents.registry").is_some_and(|u| u.trim().is_empty()) {
            candidates.push(fenix_picker::Candidate::new("Install an agent from the ACP registry…".to_string(), Pick::Registry));
        }
        for e in entries.iter().filter(|e| e.source == catalog::Source::Registry) {
            candidates.push(fenix_picker::Candidate::new(format!("Uninstall {} (from the registry)", e.spec.name), Pick::Uninstall(e.spec.id.clone())));
        }
        self.enter_picker(ActivePicker::Agents(fenix_picker::PickerState::new(candidates)));
    }

    /// `SPC a t`: the project's threads.
    pub(crate) fn agents_pick_thread(&mut self) {
        self.agents_configure();
        let root = self.agents_root();
        self.agents.load_threads(&root, 200);
        let mut candidates = Vec::new();
        for id in self.agents.hub.thread_ids().iter().rev() {
            let Some(view) = self.agents.hub.view(id) else { continue };
            if view.meta.root != root {
                continue;
            }
            let state = if view.is_waiting_on_you() {
                "waiting on you"
            } else if view.is_working() {
                "working"
            } else if matches!(view.link, Link::ReadOnly(_)) {
                "read-only"
            } else {
                ""
            };
            let label = format!("{}   {}{}", view.thread.display_title(), view.meta.agent, if state.is_empty() { String::new() } else { format!(" · {state}") });
            candidates.push(fenix_picker::Candidate::new(label, Pick::Thread(id.clone())));
        }
        // Waiting threads first.
        candidates.sort_by_key(|c| !c.label.contains("waiting on you"));
        if candidates.is_empty() {
            self.set_message("no agent threads in this project yet: SPC a n starts one");
            return;
        }
        self.enter_picker(ActivePicker::Agents(fenix_picker::PickerState::new(candidates)));
    }

    /// `SPC a i`: sessions the agent has for this project that Fenix
    /// doesn't (started in its own CLI): one picked becomes a thread.
    pub(crate) fn agents_import(&mut self) {
        let entries = self.agents_configure();
        let Some(agent) = self.agents_shown_agent().or_else(|| self.agents_default(&entries)) else {
            self.agents_none_found(&entries);
            return;
        };
        self.agents.listing = true;
        let fx = self.agents.hub.list_sessions(&agent, &self.agents_root());
        self.agents_run(fx);
        self.set_message("asking the agent for its sessions…");
    }

    /// `SPC a k`: cancels the turn (or takes back a queued prompt).
    pub(crate) fn agents_cancel(&mut self) {
        let Some(thread) = self.agents.shown(&self.pages) else { return };
        let fx = self.agents.hub.cancel(&thread, Instant::now());
        self.agents_run(fx);
    }

    /// `SPC a m`: the agent's modes.
    pub(crate) fn agents_pick_mode(&mut self) {
        let Some(thread) = self.agents.shown(&self.pages) else {
            self.set_message("no agent thread open: SPC a a");
            return;
        };
        let Some(modes) = self.agents.hub.view(&thread).and_then(|v| v.thread.modes.clone()) else {
            self.set_message("this agent has no modes");
            return;
        };
        let candidates = modes
            .available
            .iter()
            .map(|m| {
                let current = if m.id == modes.current { " (now)" } else { "" };
                let description = m.description.as_deref().map(|d| format!("   {d}")).unwrap_or_default();
                fenix_picker::Candidate::new(format!("{}{current}{description}", m.name), Pick::Mode(m.id.clone()))
            })
            .collect();
        self.enter_picker(ActivePicker::Agents(fenix_picker::PickerState::new(candidates)));
    }

    /// `SPC a M`: the agent's other settings -- its model first.
    pub(crate) fn agents_pick_option(&mut self) {
        let Some(thread) = self.agents.shown(&self.pages) else {
            self.set_message("no agent thread open: SPC a a");
            return;
        };
        let options = self.agents.hub.view(&thread).map(|v| v.thread.config_options.clone()).unwrap_or_default();
        let mut candidates = Vec::new();
        let mut ordered: Vec<_> = options.iter().filter(|o| o.category.as_deref() != Some("mode")).collect();
        ordered.sort_by_key(|o| o.category.as_deref() != Some("model"));
        for option in ordered {
            match &option.value {
                ConfigValue::Select { current, choices } => {
                    for c in choices {
                        let now = if &c.value == current { " (now)" } else { "" };
                        let group = c.group.as_deref().map(|g| format!("{g} / ")).unwrap_or_default();
                        candidates.push(fenix_picker::Candidate::new(format!("{}: {group}{}{now}", option.name, c.name), Pick::Config { id: option.id.clone(), value: ConfigValue::Select { current: c.value.clone(), choices: vec![] } }));
                    }
                }
                ConfigValue::Boolean(on) => {
                    candidates.push(fenix_picker::Candidate::new(format!("{}: turn {}", option.name, if *on { "off" } else { "on" }), Pick::Config { id: option.id.clone(), value: ConfigValue::Boolean(!on) }));
                }
            }
        }
        if candidates.is_empty() {
            self.set_message("this agent has no settings to change");
            return;
        }
        self.enter_picker(ActivePicker::Agents(fenix_picker::PickerState::new(candidates)));
    }

    /// `SPC a R`: starts the shown thread's agent again.
    pub(crate) fn agents_restart(&mut self) {
        let Some(thread) = self.agents.shown(&self.pages) else { return };
        let Some(conn) = self.agents.hub.connection_of(&thread) else {
            let fx = self.agents.hub.focus(&thread);
            self.agents_run(fx);
            return;
        };
        let fx = self.agents.hub.restart(conn, Instant::now());
        self.agents_run(fx);
        self.set_message("restarting the agent");
    }

    /// `SPC h a`: what each agent connection said and was told.
    pub(crate) fn agents_open_log(&mut self) {
        let mut text = String::new();
        if self.agents.logs.is_empty() {
            text.push_str("No agent has been started yet.\n");
        }
        for (conn, (name, log)) in &self.agents.logs {
            text.push_str(&format!("## {name} (connection {})\n\n", conn.0));
            for line in log.lock().unwrap().lines() {
                let side = match line.side {
                    LogSide::Out => "→",
                    LogSide::In => "←",
                    LogSide::Stderr => "!",
                    LogSide::Note => "#",
                };
                text.push_str(&format!("{side} {}\n", line.text));
            }
            text.push('\n');
        }
        // One view, named, in the code's pane, and not kept with the
        // session: it's a look at what's in memory now.
        let existing = self.ref_view_names.iter().find(|(id, name)| name.as_str() == LOG_VIEW && self.buffers.get(**id).is_some()).map(|(id, _)| *id);
        let buffer = match existing {
            Some(id) => {
                self.replace_buffer_from_disk(id, &text);
                id
            }
            None => {
                let id = self.buffers.open_task_output(&text);
                self.ref_view_names.insert(id, LOG_VIEW.to_string());
                id
            }
        };
        if self.agents_is_dock_pane(self.focused_pane_id()) {
            self.agents_focus_code();
        }
        self.open_buffer_in_focused_pane(buffer);
    }

    pub(super) fn agents_picked(&mut self, pick: Pick) {
        match pick {
            Pick::NewThread(agent) => {
                if !self.agents_catalog().iter().any(|e| e.spec.id == agent && e.found) {
                    self.set_error(format!("{agent} isn't installed: install it, or set agents.servers"));
                    return;
                }
                let id = self.agents.hub.new_thread(&agent, &self.agents_root());
                self.agents_show(id);
            }
            Pick::Thread(id) => self.agents_show(id),
            Pick::Mode(mode) => {
                if let Some(thread) = self.agents.shown(&self.pages) {
                    let fx = self.agents.hub.set_mode(&thread, &mode);
                    self.agents_run(fx);
                }
            }
            Pick::Config { id, value } => {
                if let Some(thread) = self.agents.shown(&self.pages) {
                    let fx = self.agents.hub.set_config_option(&thread, &id, value);
                    self.agents_run(fx);
                }
            }
            Pick::Login { conn, method } => {
                let fx = self.agents.hub.authenticate(conn, &method);
                self.agents_run(fx);
            }
            Pick::Import { agent, root, session } => {
                let id = self.agents.hub.import(&agent, &root, &session);
                self.agents_show(id);
            }
            Pick::Send { save } => {
                if let Some((thread, blocks)) = self.agents.held.take() {
                    if save {
                        self.agents_save_project_buffers();
                    }
                    let fx = self.agents.hub.prompt(&thread, blocks);
                    self.agents_run(fx);
                }
            }
            Pick::Restart(conn) => {
                let fx = self.agents.hub.restart(conn, Instant::now());
                self.agents_run(fx);
            }
            Pick::Mention(what) => {
                self.agents_focus_composer();
                self.agents_composer_insert(&format!("{what} "));
            }
            Pick::Command(name) => {
                self.agents_focus_composer();
                self.agents_composer_insert(&format!("{name} "));
            }
            Pick::HandOff(agent) => self.agents_hand_off(&agent),
            Pick::Registry => self.agents_browse_registry(),
            Pick::Install(agent) => self.agents_install(*agent),
            Pick::Uninstall(id) => self.agents_uninstall(&id),
            Pick::Secret(name) => self.agents_secret_prompt(&name),
            Pick::Delete(thread) => self.agents_delete(&thread),
            Pick::Nothing => {}
        }
    }

    fn agents_shown_agent(&self) -> Option<String> {
        let thread = self.agents.shown(&self.pages)?;
        self.agents.hub.meta(&thread).map(|m| m.agent.clone())
    }

    // -----------------------------------------------------------------
    // The panel

    /// Shows `thread` in the dock, putting the dock up if it isn't.
    pub(super) fn agents_show(&mut self, thread: ThreadId) {
        let page = match self.agents.page.filter(|p| self.pages.contains_key(p)) {
            Some(page) => {
                if let Some(PageModel::Agent(p)) = self.pages.get_mut(&page).map(|s| &mut s.model) {
                    **p = AgentPage::new(thread.clone());
                }
                self.restale_page(page);
                page
            }
            None => {
                let page = self.add_page(PageModel::Agent(Box::new(AgentPage::new(thread.clone()))));
                self.agents.page = Some(page);
                page
            }
        };
        self.agents_dock_ensure(page);
        self.main_view = MainView::Editor;
        if let Some(page) = self.agents.page {
            if let Some(PageModel::Agent(p)) = self.pages.get_mut(&page).map(|s| &mut s.model) {
                if let Some(view) = self.agents.hub.view(&thread) {
                    p.follow_up(view.thread);
                }
            }
        }
        let fx = self.agents.hub.focus(&thread);
        self.agents_run(fx);
        self.wake_caret();
    }

    /// The shown thread is focused in the hub (it attaches if it can).
    fn agents_focus_shown(&mut self) {
        if let Some(thread) = self.agents.shown(&self.pages) {
            let fx = self.agents.hub.focus(&thread);
            self.agents_run(fx);
        }
    }

    /// A key for the panel's page.
    pub(super) fn agents_page_key(&mut self, id: BufferId, key: crate::page::Key) {
        let action = {
            let Some(PageModel::Agent(page)) = self.pages.get_mut(&id).map(|s| &mut s.model) else { return };
            let Some(view) = self.agents.hub.view(&page.thread) else { return };
            page.key(view.thread, key)
        };
        let Some(thread) = self.agents.shown(&self.pages) else { return };
        // A thread put back from the last session attaches once it's
        // looked at.
        if self.agents.hub.view(&thread).is_some_and(|v| matches!(v.link, Link::Detached)) {
            self.agents_configure();
            let fx = self.agents.hub.focus(&thread);
            self.agents_run(fx);
        }
        match action {
            agent_page::Action::None => {}
            agent_page::Action::ContinueAnew => self.agents_continue_anew(),
            agent_page::Action::Close => self.agents_close_panel(),
            agent_page::Action::Compose => self.agents_focus_composer(),
            agent_page::Action::Cancel => self.agents_cancel(),
            agent_page::Action::Permission { key, option } => self.agents_answer(&thread, &key, Some(&option)),
            agent_page::Action::Open { path, line } => self.agents_open_location(&thread, &path, line),
            agent_page::Action::Yank(text) => {
                if let Some(clipboard) = &mut self.clipboard {
                    let _ = clipboard.set_text(text.clone());
                }
                self.set_message(format!("copied {} lines", text.lines().count().max(1)));
            }
            agent_page::Action::View(text) => {
                let buffer = self.buffers.open_text_view(&text);
                self.agents_focus_code();
                self.open_buffer_in_focused_pane(buffer);
            }
            agent_page::Action::OtherThread(_) => self.agents_pick_thread(),
        }
    }

    fn agents_answer(&mut self, thread: &ThreadId, key: &str, option: Option<&str>) {
        if let Some(n) = key.strip_prefix("fenix-guard:") {
            let allow = option == Some("allow");
            self.agents_resolve_guard(thread, n, allow);
            return;
        }
        let fx = self.agents.hub.answer_permission(thread, key, option);
        self.agents_run(fx);
    }

    /// Opens a file the agent named, in the pane beside the panel.
    fn agents_open_location(&mut self, thread: &ThreadId, path: &str, line: Option<u32>) {
        let root = self.agents.hub.meta(thread).map(|m| m.root.clone()).unwrap_or_else(|| self.agents_root());
        let path = root.join(path);
        self.agents_focus_code();
        self.open_file_from_picker(&path);
        if let Some(line) = line {
            self.jump_to_grep_match(&fenix_project::GrepMatch { path, line: line.max(1) as usize, col: 1, text: String::new() });
        }
    }

    pub(super) fn agents_close_panel(&mut self) {
        let shown = self.agents.shown(&self.pages);
        for buffer in [self.agents.composer, self.agents.page].into_iter().flatten() {
            for pane in self.agents_panes_showing(buffer) {
                self.agents_close_pane(pane);
            }
        }
        if let Some(page) = self.agents.page.take() {
            self.close_page_quietly(page);
        }
        self.agents_focus_code();
        // Its session is let go (the agent keeps it to resume); an agent
        // with none left stops after `agents.idle_minutes`.
        if let Some(thread) = shown {
            let fx = self.agents.hub.close(&thread, Instant::now());
            self.agents_run(fx);
        }
        self.agents.hub.unfocus();
        self.wake_caret();
    }

    /// A key in the prompt buffer: Enter in Normal mode sends it, as does
    /// Ctrl-Enter in Insert; Ctrl-C in Normal mode cancels the turn; `q`
    /// back to the thread. Everything else is Vim's.
    pub(super) fn agents_composer_key(&mut self, keypress: KeyPress) -> bool {
        let id = self.focused_buffer_id();
        if Some(id) != self.agents.composer || self.buffers.get(id).map(|b| b.kind) != Some(BufferKind::Prompt) {
            return false;
        }
        if self.leader_matcher.is_pending() || self.local_matcher.is_some() {
            return false;
        }
        let enter = keypress.code == KeyCode::Named(FenixNamedKey::Enter);
        let normal = self.vim.mode() == Mode::Normal && self.vim.is_idle();
        // Ctrl-Enter in Insert sends and stays in Insert, for the next one.
        if enter && (normal && keypress.mods == Mods::default() || keypress.mods.ctrl) {
            self.agents_send();
            return true;
        }
        if normal && keypress == KeyPress::char('c').with_ctrl() {
            self.agents_cancel();
            return true;
        }
        self.agents_composer_extra_key(keypress)
    }

    /// Sends the prompt buffer's text to the shown thread, and clears it.
    pub(crate) fn agents_send(&mut self) {
        let Some(composer) = self.agents.composer else { return };
        let Some(thread) = self.agents.shown(&self.pages) else {
            self.set_message("no agent thread open: SPC a a");
            return;
        };
        let text = self.buffers.get(composer).map(|b| b.buffer.text()).unwrap_or_default();
        if text.trim().is_empty() {
            return;
        }
        self.replace_buffer_from_disk(composer, "");
        let said = text.trim_end().to_string();
        self.agents.sent.retain(|p| *p != said);
        self.agents.sent.push(said);
        self.agents.recalled = None;
        let blocks = self.agents_prompt_blocks(&thread, text.trim_end());
        let root = self.agents.hub.meta(&thread).map(|m| m.root.clone()).unwrap_or_else(|| self.agents_root());
        let dirty = self.agents_dirty_buffers(&root);
        let policy = self.config.text_value("agents.save_before_prompt").unwrap_or("ask").to_string();
        if !dirty.is_empty() && policy != "never" {
            if policy == "always" {
                self.agents_save_project_buffers();
            } else {
                self.agents.held = Some((thread, blocks));
                let names: Vec<String> = dirty.iter().map(|id| self.buffer_display_name(*id)).collect();
                let candidates = vec![
                    fenix_picker::Candidate::new(format!("Save {} first, then send (the agent reads files from disk)", names.join(", ")), Pick::Send { save: true }),
                    fenix_picker::Candidate::new("Send without saving: the agent won't see those edits".to_string(), Pick::Send { save: false }),
                ];
                self.enter_picker(ActivePicker::Agents(fenix_picker::PickerState::new(candidates)));
                return;
            }
        }
        let fx = self.agents.hub.prompt(&thread, blocks);
        self.agents_run(fx);
        // Back to the thread, to watch -- out of Insert mode, sent with
        // Ctrl-Enter, so the thread's keys (y, n to a question) are its.
        if self.vim.mode() == Mode::Insert {
            let pane = self.focused_pane_id();
            let id = self.focused_buffer_id();
            if let (Some(ob), Some(state)) = (self.buffers.get_mut(id), self.workspaces.active_pane_states_mut().get_mut(&pane)) {
                self.vim.handle_key(&mut ob.buffer, &mut state.cursor, KeyPress::named(FenixNamedKey::Escape));
            }
        }
        if let Some(page) = self.agents.page {
            if let Some(pane) = self.agents_panes_showing(page).first().copied() {
                self.windows_mut().focus(pane);
            }
            if let Some(PageModel::Agent(p)) = self.pages.get_mut(&page).map(|s| &mut s.model) {
                p.follow = true;
            }
        }
    }

    /// The files under `root` with unsaved edits.
    fn agents_dirty_buffers(&self, root: &Path) -> Vec<BufferId> {
        self.buffers
            .ids_sorted_by_path()
            .into_iter()
            .filter(|id| self.buffers.get(*id).is_some_and(|b| b.kind.tracks_unsaved_changes() && b.buffer.is_dirty() && b.buffer.path().is_some_and(|p| p.starts_with(root))))
            .collect()
    }

    fn agents_save_project_buffers(&mut self) {
        let root = self.agents_root();
        for id in self.agents_dirty_buffers(&root) {
            self.agents_save_buffer(id);
        }
    }

    /// Saves one buffer, as `:w` would, without it being focused.
    fn agents_save_buffer(&mut self, id: BufferId) -> bool {
        let Some(ob) = self.buffers.get_mut(id) else { return false };
        match ob.buffer.save() {
            Ok(()) => {
                self.note_disk_state(id);
                self.discard_recovery_for(id);
                self.refresh_gutter_hunks(id);
                true
            }
            Err(e) => {
                let name = self.buffer_display_name(id);
                self.set_error(format!("couldn't save {name}: {e}"));
                false
            }
        }
    }

    // -----------------------------------------------------------------
    // Carrying out the hub's effects

    pub(super) fn apply_acp_event(&mut self, conn: ConnId, generation: u32, event: AcpEvent) {
        let fx = self.agents.hub.on_event(conn, generation, event, Instant::now());
        self.agents_run(fx);
    }

    /// What arrived without an event loop (tests): taken now.
    pub(super) fn agents_drain_inbox(&mut self) -> usize {
        let events: Vec<_> = self.agents.inbox.lock().unwrap().drain(..).collect();
        let n = events.len();
        for (conn, generation, event) in events {
            self.apply_acp_event(conn, generation, event);
        }
        n
    }

    pub(super) fn agents_run(&mut self, fx: Vec<Effect>) {
        let mut queue: VecDeque<Effect> = fx.into();
        let mut changed = false;
        while let Some(effect) = queue.pop_front() {
            match effect {
                Effect::Spawn { conn, generation, agent, root } => {
                    let launch = Launch { program: agent.program.clone(), args: agent.args.clone(), env: agent.env.clone(), cwd: root };
                    let send: Box<dyn FnMut(AcpEvent) -> bool + Send> = match self.event_proxy.clone() {
                        Some(proxy) => Box::new(move |event| proxy.send_event(FenixUserEvent::Acp { conn, generation, event }).is_ok()),
                        None => {
                            let inbox = self.agents.inbox.clone();
                            Box::new(move |event| {
                                inbox.lock().unwrap().push_back((conn, generation, event));
                                true
                            })
                        }
                    };
                    let result = AcpClient::start(&launch, send).map(|client| {
                        self.agents.logs.insert(conn, (agent.name.clone(), client.log()));
                        self.agents.clients.insert(conn, client);
                    });
                    queue.extend(self.agents.hub.on_spawned(conn, generation, result.map_err(|e| e.to_string()), Instant::now()));
                }
                Effect::Stop { conn } => {
                    if let Some(client) = self.agents.clients.remove(&conn) {
                        client.note("stopped by Fenix");
                    }
                }
                Effect::Request { conn, id, method, params } => {
                    if let Some(c) = self.agents.clients.get(&conn) {
                        let _ = c.request(id, method, params);
                    }
                }
                Effect::Notify { conn, method, params } => {
                    if let Some(c) = self.agents.clients.get(&conn) {
                        let _ = c.notify(method, params);
                    }
                }
                Effect::Respond { conn, id, result } => {
                    if let Some(c) = self.agents.clients.get(&conn) {
                        let _ = c.respond(id, result);
                    }
                }
                Effect::Serve { conn, id, thread, request } => {
                    if let Some(result) = self.agents_serve(conn, &thread, id.clone(), request) {
                        queue.extend(self.agents.hub.on_served(conn, &thread, id, result));
                    }
                }
                Effect::Announced { thread, paths, .. } => self.agents_announced(&thread, &paths),
                Effect::Withdraw { thread } => {
                    self.agents.guards.retain(|g| g.thread != thread);
                    self.agents_kill_terminals(&thread);
                }
                Effect::TerminalLogin { conn, args, .. } => self.agents_start_terminal_login(conn, &args),
                Effect::Listed { agent, root, sessions } => {
                    if std::mem::take(&mut self.agents.listing) {
                        let known: HashSet<String> = self.agents.hub.thread_ids().iter().filter_map(|t| self.agents.hub.meta(t).and_then(|m| m.session_id.clone())).collect();
                        let candidates: Vec<_> = sessions
                            .into_iter()
                            .filter(|s| !known.contains(&s.session_id))
                            .map(|s| {
                                let label = format!("{}   {}", s.title.clone().unwrap_or_else(|| s.session_id.clone()), s.updated_at.clone().unwrap_or_default());
                                fenix_picker::Candidate::new(label, Pick::Import { agent: agent.clone(), root: root.clone(), session: s })
                            })
                            .collect();
                        if candidates.is_empty() {
                            self.set_message("the agent has no other sessions for this project");
                        } else {
                            self.enter_picker(ActivePicker::Agents(fenix_picker::PickerState::new(candidates)));
                        }
                    }
                }
                Effect::Save(thread) => self.agents.save(&thread),
                Effect::Forget(thread) => {
                    if let Some(store) = &self.agents.store {
                        let _ = store.forget(&thread);
                    }
                    self.agents.baselines.remove(&thread);
                }
                Effect::Changed(thread) => {
                    self.agents.dirty.insert(thread.clone());
                    let turning = self.agents.hub.view(&thread).is_some_and(|v| !matches!(v.turn, Turn::Idle));
                    if turning && !self.agents.turning.contains_key(&thread) {
                        self.agents.turning.insert(thread.clone(), Instant::now());
                        self.agents_turn_started(&thread);
                    } else if !turning && self.agents.turning.remove(&thread).is_some() {
                        self.agents_turn_ended(&thread);
                        self.agents_commit_message_done(&thread);
                    }
                    if self.agents.shown(&self.pages).as_ref() == Some(&thread) {
                        changed = true;
                        self.agents_follow(&thread);
                    }
                }
                Effect::ConnectionChanged(_) => changed = true,
                Effect::Attention { thread, why } => {
                    if !self.agents_ask_attention(&thread, why) {
                        self.agents_attention(&thread, why);
                    }
                }
                Effect::TurnEnded { thread, stop } => self.agents_turn_done(&thread, &stop),
            }
        }
        if changed {
            if let Some(page) = self.agents.page {
                if let Some(PageModel::Agent(p)) = self.pages.get_mut(&page).map(|s| &mut s.model) {
                    if let Some(view) = self.agents.hub.view(&p.thread) {
                        p.follow_up(view.thread);
                    }
                }
                self.restale_page(page);
            }
            self.agents_dock_titles();
        }
        self.wake_caret();
    }

    fn agents_attention(&mut self, thread: &ThreadId, why: Attention) {
        let shown = self.agents.shown(&self.pages).as_ref() == Some(thread);
        let title = self.agents.hub.view(thread).map(|v| v.thread.display_title()).unwrap_or_default();
        match why {
            Attention::Permission if !shown => self.set_message(format!("an agent asks permission in \"{title}\": SPC a t")),
            Attention::Login => self.set_message("an agent wants a login: SPC a p"),
            _ => {}
        }
        if self.config.bool_value("agents.notify").unwrap_or(true) && !self.window_focused() {
            if let Some(window) = self.frame_window(self.active_frame) {
                window.request_user_attention(Some(winit::window::UserAttentionType::Informational));
            }
        }
    }

    /// Whether a Fenix window has the keyboard.
    fn window_focused(&self) -> bool {
        (0..self.frames.len()).filter_map(|f| self.frame_window(f)).any(|w| w.has_focus())
    }

    // -----------------------------------------------------------------
    // Files

    /// Snapshots what the agent is about to change, and watches it.
    fn agents_announced(&mut self, thread: &ThreadId, paths: &[String]) {
        let Some(root) = self.agents.hub.meta(thread).map(|m| m.root.clone()) else { return };
        for p in paths {
            let path = root.join(p);
            if !within(&root, &path) {
                continue;
            }
            let open = self.buffers.id_for_path(&path);
            let before = match open.and_then(|id| self.buffers.get(id)) {
                Some(ob) => Some(ob.buffer.text()),
                None => std::fs::read_to_string(&path).ok(),
            };
            self.agents.baselines.entry(thread.clone()).or_default().entry(path.clone()).or_insert(before);
            self.agents.watched.entry(path.clone()).or_insert_with(|| DiskFingerprint::of(&path));
            if let Some(id) = open.filter(|id| self.buffers.get(*id).is_some_and(|b| b.buffer.is_dirty())) {
                let name = self.buffer_display_name(id);
                if let Some(t) = self.agents.hub.thread_mut(thread) {
                    t.notice(NoticeKind::Warning, format!("The agent is about to change {name}, which has unsaved edits of yours. They stay in the buffer: :w! keeps yours, :e! takes the agent's."));
                }
                self.agents.dirty.insert(thread.clone());
            }
        }
    }

    /// Does what the agent asked of Fenix. `None`: answered later (a
    /// question to you first).
    fn agents_serve(&mut self, conn: ConnId, thread: &ThreadId, id: Value, request: ClientRequest) -> Option<Result<Value, RpcError>> {
        let root = self.agents.hub.meta(thread).map(|m| m.root.clone())?;
        let outside = |path: &Path| RpcError::new(code::INVALID_PARAMS, format!("{} is outside the project ({})", path.display(), root.display()));
        match request {
            ClientRequest::ReadTextFile { path, line, limit, .. } => {
                let path = PathBuf::from(path);
                if !within(&root, &path) {
                    return Some(Err(outside(&path)));
                }
                let text = match self.buffers.id_for_path(&path).and_then(|id| self.buffers.get(id)) {
                    Some(ob) => Ok(ob.buffer.text()),
                    None => std::fs::read_to_string(&path).map_err(|e| RpcError::new(code::INTERNAL_ERROR, format!("{}: {e}", path.display()))),
                };
                Some(text.map(|text| {
                    let start = line.map_or(0, |l| l.saturating_sub(1) as usize);
                    let lines: Vec<&str> = text.split_inclusive('\n').skip(start).take(limit.map_or(usize::MAX, |l| l as usize)).collect();
                    json!({ "content": lines.concat() })
                }))
            }
            ClientRequest::WriteTextFile { path, content, .. } => {
                let path = PathBuf::from(path);
                if !within(&root, &path) {
                    return Some(Err(outside(&path)));
                }
                let open = self.buffers.id_for_path(&path);
                if let Some(buffer) = open.filter(|id| self.buffers.get(*id).is_some_and(|b| b.buffer.is_dirty())) {
                    // Your unsaved edits: you're asked first.
                    let n = self.agents.next_guard;
                    self.agents.next_guard += 1;
                    let name = self.buffer_display_name(buffer);
                    if let Some(t) = self.agents.hub.thread_mut(thread) {
                        t.push_permission(PermissionCard {
                            key: format!("fenix-guard:{n}"),
                            tool_call: ToolCallFields { id: format!("fenix-guard-{n}"), title: Some(format!("overwrite {name}, which has unsaved edits of yours")), ..Default::default() },
                            options: vec![
                                PermissionOption { id: "allow".into(), name: "Overwrite them".into(), kind: PermissionKind::AllowOnce },
                                PermissionOption { id: "reject".into(), name: "Keep mine (the agent is told)".into(), kind: PermissionKind::RejectOnce },
                            ],
                            state: PermissionState::Pending,
                        });
                    }
                    self.agents.guards.push(Guard { key: format!("fenix-guard:{n}"), conn, thread: thread.clone(), id, path, content });
                    self.agents_attention(thread, Attention::Permission);
                    let fx = vec![Effect::Changed(thread.clone())];
                    self.agents_run(fx);
                    return None;
                }
                Some(self.agents_write(thread, &path, &content))
            }
            ClientRequest::RequestPermission { .. } => None,
            ClientRequest::CreateTerminal { command, args, env, cwd, output_byte_limit, .. } => Some(self.agents_terminal_create(thread, command, args, env, cwd, output_byte_limit)),
            request => self.agents_terminal_request(conn, thread, id, request),
        }
    }

    fn agents_resolve_guard(&mut self, thread: &ThreadId, n: &str, allow: bool) {
        let key = format!("fenix-guard:{n}");
        let Some(at) = self.agents.guards.iter().position(|g| &g.thread == thread && g.key == key) else { return };
        let guard = self.agents.guards.remove(at);
        let (state, result) = if allow {
            ("Overwrite them".to_string(), self.agents_write(thread, &guard.path, &guard.content))
        } else {
            ("Keep mine".to_string(), Err(RpcError::new(code::INTERNAL_ERROR, format!("{} has unsaved edits the user kept; it wasn't written", guard.path.display()))))
        };
        if let Some(t) = self.agents.hub.thread_mut(thread) {
            t.set_permission_state(&key, PermissionState::Answered(state));
        }
        let mut fx = self.agents.hub.on_served(guard.conn, thread, guard.id, result);
        fx.push(Effect::Changed(thread.clone()));
        self.agents_run(fx);
    }

    /// Writes an agent's file: into its buffer when it's open (one undo
    /// step), saved unless `agents.write = "buffer"`; else to disk.
    fn agents_write(&mut self, thread: &ThreadId, path: &Path, content: &str) -> Result<Value, RpcError> {
        let failed = |e: std::io::Error| RpcError::new(code::INTERNAL_ERROR, format!("{}: {e}", path.display()));
        let open = self.buffers.id_for_path(path);
        let before = match open.and_then(|id| self.buffers.get(id)) {
            Some(ob) => Some(ob.buffer.text()),
            None => std::fs::read_to_string(path).ok(),
        };
        self.agents.baselines.entry(thread.clone()).or_default().entry(path.to_path_buf()).or_insert(before);
        match open {
            Some(id) => {
                self.replace_buffer_from_disk(id, content);
                if self.config.text_value("agents.write") == Some("buffer") {
                    if let Some(ob) = self.buffers.get_mut(id) {
                        ob.buffer.mark_unsaved();
                    }
                } else if !self.agents_save_buffer(id) {
                    return Err(RpcError::new(code::INTERNAL_ERROR, format!("couldn't save {}", path.display())));
                }
                self.refresh_gutter_hunks(id);
            }
            None => {
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent).map_err(failed)?;
                }
                fenix_storage::write(path, content.as_bytes()).map_err(failed)?;
            }
        }
        self.agents.dirty.insert(thread.clone());
        self.agents_refresh_marks();
        Ok(Value::Null)
    }

    // -----------------------------------------------------------------
    // Time

    /// The time passes for the agents: the hub's clocks, files they're
    /// changing looked at, threads saved. When to be called again.
    pub(super) fn agents_tick(&mut self, now: Instant) -> Option<Instant> {
        self.agents_drain_inbox();
        let fx = self.agents.hub.tick(now);
        if !fx.is_empty() {
            self.agents_run(fx);
        }
        self.agents_fetched_tick();
        self.agents_asks_tick(now);
        let running = self.agents_terminals_tick();
        self.agents_logins_tick();
        let status = self.agents.hub.status();
        // A turn waiting on your answer is still under way.
        let busy = status.working > 0 || status.waiting > 0 || running;
        if busy && now >= self.agents.next_watch {
            self.agents.next_watch = now + WATCH_EVERY;
            self.agents_check_watched();
        }
        if !busy && !self.agents.watched.is_empty() {
            // One last look after the turn, then the files are let go.
            self.agents_check_watched();
            self.agents.watched.clear();
        }
        if !self.agents.dirty.is_empty() && (now.duration_since(self.agents.last_save) >= SAVE_EVERY || !busy) {
            self.agents.last_save = now;
            for id in std::mem::take(&mut self.agents.dirty) {
                self.agents.save(&id);
            }
        }
        if busy || !self.agents.inbox.lock().unwrap().is_empty() {
            Some(now + WATCH_EVERY)
        } else if self.agents.hub.connections().next().is_some() {
            Some(now + Duration::from_secs(1))
        } else {
            None
        }
    }

    /// Files announced for change: an open, clean buffer follows the
    /// disk at once (a dirty one is marked changed, as any file is), and
    /// the agent's lines are tinted.
    fn agents_check_watched(&mut self) {
        let paths: Vec<PathBuf> = self.agents.watched.keys().cloned().collect();
        let mut sweep = DiskSweep::default();
        let mut changed = false;
        for path in paths {
            let now = DiskFingerprint::of(&path);
            let before = self.agents.watched.insert(path.clone(), now);
            if before == Some(now) {
                continue;
            }
            changed = true;
            if let Some(id) = self.buffers.id_for_path(&path) {
                self.check_one_file(id, &mut sweep, false, None);
            }
        }
        self.agents_refresh_marks();
        if changed {
            self.agents_count_to_review();
        }
    }

    /// The modeline's agent segment: one agent by name (working how long,
    /// or waiting on you), several by count, and once they're done the
    /// changes left to review.
    pub(super) fn agents_status_segment(&self) -> String {
        let status = self.agents.hub.status();
        let busy: Vec<&ThreadId> = self.agents.turning.keys().collect();
        let name = |t: &ThreadId| self.agents.status_of(t).agent;
        match (status.working, status.waiting) {
            (0, 0) if self.agents.to_review > 0 => {
                let n = self.agents.to_review;
                format!("   ✓ agent changes: {n} file{} to review (SPC a r, ]a)", if n == 1 { "" } else { "s" })
            }
            (0, 0) => String::new(),
            (0, 1) if busy.len() == 1 => format!("   ? {} is waiting on you", name(busy[0])),
            (1, 0) if busy.len() == 1 => {
                let secs = self.agents.turning[busy[0]].elapsed().as_secs();
                format!("   ● {} working {}:{:02}", name(busy[0]), secs / 60, secs % 60)
            }
            (w, 0) => format!("   ● agents: {w} working"),
            (0, n) => format!("   ? agents: {n} waiting on you"),
            (w, n) => format!("   ● agents: {w} working · {n} waiting on you"),
        }
    }

    /// Counts the files agents changed that you haven't kept or rejected,
    /// for the modeline.
    pub(super) fn agents_count_to_review(&mut self) {
        let threads: Vec<ThreadId> = self.agents.baselines.keys().cloned().collect();
        let mut paths: HashSet<PathBuf> = HashSet::new();
        for thread in &threads {
            paths.extend(self.agents_review_files(thread).into_iter().map(|f| f.path));
        }
        self.agents.to_review = paths.len();
    }

    /// Fenix is quitting: threads saved, agents stopped.
    pub(super) fn agents_shutdown(&mut self) {
        let fx = self.agents.hub.shutdown();
        self.agents_run(fx);
        self.agents.clients.clear();
    }
}

mod context;
mod dock;
mod review;
mod terminals;
pub(crate) mod threads;
mod install;
pub(crate) use install::SECRET_PROMPT;
mod extensions;

#[cfg(test)]
mod tests;
