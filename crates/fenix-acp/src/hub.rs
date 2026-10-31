//! Sessions: which thread is bound to which agent session, and
//! everything that happens to that binding -- starting, attaching,
//! resuming, prompting, cancelling, switching, crashing, closing.
//!
//! ACP sessions belong to an agent process and die with it. Fenix's
//! threads are the conversations you see, kept on disk, and outlive any
//! process. The hub is the only place that binds one to the other.
//!
//! It is written sans-I/O: it holds no threads, processes or clock. The
//! caller tells it what happened -- your intents (`new_thread`, `prompt`,
//! `cancel`…), the agents' events (`on_event`), a process started
//! (`on_spawned`), a request served (`on_served`), the time (`tick`) --
//! and carries out the `Effect`s it returns: start a process, send a
//! message, serve a file read, save a thread, redraw. So every flow,
//! crashes and races included, is a plain unit test: inputs in, effects
//! out. See `docs/plans/agents-acp.md` §3.5 for the design.

use crate::client::{AcpEvent, RpcError};
use crate::protocol::{self, code, AgentInfo, AuthKind, ClientCapabilities, ClientRequest, ConfigValue, ContentBlock, ListedSession, McpServer, PermissionOption, SessionId, SessionSetup, SessionUpdate, StopReason, ToolCallFields};
use crate::thread::{NoticeKind, PermissionCard, PermissionState, Thread};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// How long a cancelled turn may take to end before Fenix ends it.
pub const CANCEL_GRACE: Duration = Duration::from_secs(10);
/// Crashes within this window count toward giving up.
pub const CRASH_WINDOW: Duration = Duration::from_secs(60);
/// After this many crashes in the window, an agent stays down until
/// restarted by hand.
pub const MAX_CRASHES: usize = 3;

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ThreadId(pub String);

impl std::fmt::Display for ThreadId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ConnId(pub u64);

/// An agent Fenix can start: what the catalog gives the hub.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentSpec {
    /// Stable, short: `claude`, `codex`, `devin`, or yours.
    pub id: String,
    pub name: String,
    pub program: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ThreadMeta {
    pub id: ThreadId,
    /// The agent's id (`AgentSpec::id`).
    pub agent: String,
    pub root: PathBuf,
    /// The agent's session, once there's been one.
    pub session_id: Option<SessionId>,
    /// The thread this one was handed off from, if any.
    pub handed_off_from: Option<ThreadId>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Link {
    /// On disk only; no process involved.
    Detached,
    /// `session/new`, `/resume` or `/load` sent, or waiting for the agent
    /// to be ready to send it.
    Attaching { conn: ConnId, how: Attach },
    Live { conn: ConnId, session: SessionId },
    /// The agent can't take this thread back; its transcript stays
    /// readable. Why.
    ReadOnly(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Attach {
    New,
    Resume,
    Load,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Turn {
    Idle,
    Prompting { request: i64 },
    Cancelling { request: i64, since: Instant },
}

#[derive(Clone, Debug, PartialEq)]
pub enum ConnState {
    Spawning,
    Initializing,
    Ready,
    /// The agent wants a login before it goes on.
    NeedsAuth,
    /// A login is under way: `authenticate` sent, or the terminal login
    /// running.
    Authenticating,
    Dead { reason: String, retry_at: Option<Instant> },
}

pub struct Connection {
    pub agent: AgentSpec,
    pub root: PathBuf,
    pub generation: u32,
    pub state: ConnState,
    pub info: Option<AgentInfo>,
    next_id: i64,
    pending: HashMap<i64, Pending>,
    /// What waits for the agent to be ready.
    deferred: VecDeque<Deferred>,
    crashes: VecDeque<Instant>,
    idle_since: Option<Instant>,
}

impl Connection {
    pub fn is_up(&self) -> bool {
        !matches!(self.state, ConnState::Dead { .. })
    }
}

#[derive(Clone, Debug)]
enum Pending {
    Initialize,
    Attach { thread: ThreadId, how: Attach },
    Prompt { thread: ThreadId },
    SetMode { thread: ThreadId, previous: Option<String> },
    SetConfig { thread: ThreadId, previous: Vec<protocol::ConfigOption> },
    Authenticate,
    Logout,
    List { root: PathBuf },
    /// `session/close`, `/delete`: nothing waits on the answer.
    Ignore,
}

#[derive(Clone, Debug)]
enum Deferred {
    Attach(ThreadId),
    List,
}

struct Slot {
    meta: ThreadMeta,
    thread: Thread,
    link: Link,
    turn: Turn,
    /// Prompts waiting to go out, oldest first.
    queue: VecDeque<Vec<ContentBlock>>,
    /// A `session/load`'s replay, built apart and swapped in when it ends.
    replay: Option<Thread>,
    /// A transcript to hand to the next session (hand-off, or continuing
    /// a read-only thread).
    handoff: Option<String>,
    /// Permission cards waiting: card key -> (connection, request id).
    permissions: HashMap<String, (ConnId, Value)>,
    /// fs and terminal requests handed to the caller, not yet answered.
    served: HashSet<(ConnId, String)>,
    /// Files already announced for editing in this thread.
    announced: HashSet<String>,
}

/// What the caller does for the hub. Effects live for a moment, from
/// the hub to whoever carries them out, so their size doesn't matter.
#[derive(Clone, Debug, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum Effect {
    /// Start the agent; then call `on_spawned` with this generation.
    Spawn { conn: ConnId, generation: u32, agent: AgentSpec, root: PathBuf },
    /// Stop the agent and everything it started.
    Stop { conn: ConnId },
    Request { conn: ConnId, id: i64, method: &'static str, params: Value },
    Notify { conn: ConnId, method: &'static str, params: Value },
    Respond { conn: ConnId, id: Value, result: Result<Value, RpcError> },
    /// Do what the agent asked -- read or write a file, run a terminal --
    /// and call `on_served` with the answer.
    Serve { conn: ConnId, id: Value, thread: ThreadId, request: ClientRequest },
    /// The agent announced it is about to change these files (as given:
    /// absolute, or relative to the thread's root): snapshot them now,
    /// before it writes.
    Announced { thread: ThreadId, tool_call: String, paths: Vec<String> },
    /// Drop whatever is being served for this thread (terminals killed):
    /// its turn was cancelled or its agent went away.
    Withdraw { thread: ThreadId },
    /// Run the agent's terminal login (`args`, `env` on the agent's own
    /// program) in a terminal; then call `terminal_login_finished`.
    TerminalLogin { conn: ConnId, method: String, args: Vec<String>, env: Vec<(String, String)> },
    /// Sessions `session/list` found.
    Listed { agent: String, root: PathBuf, sessions: Vec<ListedSession> },
    Save(ThreadId),
    /// The thread's file should go (it was deleted).
    Forget(ThreadId),
    Changed(ThreadId),
    ConnectionChanged(ConnId),
    /// Ask for the user's attention (Fenix may not be focused).
    Attention { thread: ThreadId, why: Attention },
    /// A prompt is done with: its turn ended (`stop` the agent's
    /// `stopReason`), or it never ran -- or was cut off -- because the
    /// agent couldn't take it (`error`).
    TurnEnded { thread: ThreadId, stop: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Attention {
    Permission,
    TurnEnded,
    Login,
}

/// A thread as the UI sees it.
pub struct ThreadView<'a> {
    pub meta: &'a ThreadMeta,
    pub thread: &'a Thread,
    pub link: &'a Link,
    pub turn: &'a Turn,
    pub queued: usize,
    pub connection: Option<&'a Connection>,
}

impl ThreadView<'_> {
    pub fn is_working(&self) -> bool {
        !matches!(self.turn, Turn::Idle) || matches!(self.link, Link::Attaching { .. })
    }

    pub fn is_waiting_on_you(&self) -> bool {
        self.thread.pending_permissions() > 0 || self.connection.is_some_and(|c| c.state == ConnState::NeedsAuth)
    }
}

/// What the modeline says.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Status {
    pub working: usize,
    pub waiting: usize,
}

pub struct Hub {
    agents: HashMap<String, AgentSpec>,
    capabilities: ClientCapabilities,
    client_version: String,
    /// Handed to every session; `{thread}` in an argument becomes the
    /// thread's id.
    mcp_servers: Vec<McpServer>,
    idle_timeout: Duration,
    connections: HashMap<ConnId, Connection>,
    next_conn: u64,
    threads: HashMap<ThreadId, Slot>,
    order: Vec<ThreadId>,
    routes: HashMap<(ConnId, SessionId), ThreadId>,
    focused: Option<ThreadId>,
    id_prefix: String,
    next_thread: u64,
}

impl Hub {
    /// `id_prefix` makes new thread ids unique across runs (the caller
    /// passes something time-based).
    pub fn new(id_prefix: impl Into<String>, client_version: impl Into<String>) -> Self {
        Self {
            agents: HashMap::new(),
            capabilities: ClientCapabilities::fenix(),
            client_version: client_version.into(),
            mcp_servers: Vec::new(),
            idle_timeout: Duration::from_secs(600),
            connections: HashMap::new(),
            next_conn: 1,
            threads: HashMap::new(),
            order: Vec::new(),
            routes: HashMap::new(),
            focused: None,
            id_prefix: id_prefix.into(),
            next_thread: 1,
        }
    }

    // -----------------------------------------------------------------
    // Configuration

    pub fn set_agents(&mut self, agents: Vec<AgentSpec>) {
        self.agents = agents.into_iter().map(|a| (a.id.clone(), a)).collect();
    }

    pub fn agent(&self, id: &str) -> Option<&AgentSpec> {
        self.agents.get(id)
    }

    pub fn set_capabilities(&mut self, capabilities: ClientCapabilities) {
        self.capabilities = capabilities;
    }

    pub fn set_mcp_servers(&mut self, servers: Vec<McpServer>) {
        self.mcp_servers = servers;
    }

    pub fn set_idle_timeout(&mut self, timeout: Duration) {
        self.idle_timeout = timeout;
    }

    // -----------------------------------------------------------------
    // Reading

    pub fn view(&self, id: &ThreadId) -> Option<ThreadView<'_>> {
        let slot = self.threads.get(id)?;
        let conn = match &slot.link {
            Link::Attaching { conn, .. } | Link::Live { conn, .. } => self.connections.get(conn),
            _ => self.find_conn(&slot.meta.agent, &slot.meta.root).and_then(|c| self.connections.get(&c)),
        };
        Some(ThreadView { meta: &slot.meta, thread: &slot.thread, link: &slot.link, turn: &slot.turn, queued: slot.queue.len(), connection: conn })
    }

    /// Every thread, oldest first.
    pub fn thread_ids(&self) -> &[ThreadId] {
        &self.order
    }

    pub fn focused(&self) -> Option<&ThreadId> {
        self.focused.as_ref()
    }

    pub fn connection(&self, conn: ConnId) -> Option<&Connection> {
        self.connections.get(&conn)
    }

    pub fn connections(&self) -> impl Iterator<Item = (ConnId, &Connection)> {
        self.connections.iter().map(|(id, c)| (*id, c))
    }

    /// The connection a thread is on, or would be.
    pub fn connection_of(&self, thread: &ThreadId) -> Option<ConnId> {
        let slot = self.threads.get(thread)?;
        match &slot.link {
            Link::Attaching { conn, .. } | Link::Live { conn, .. } => Some(*conn),
            _ => self.find_conn(&slot.meta.agent, &slot.meta.root),
        }
    }

    pub fn status(&self) -> Status {
        let mut status = Status::default();
        for id in &self.order {
            let view = self.view(id).expect("ordered threads exist");
            if view.is_waiting_on_you() {
                status.waiting += 1;
            } else if view.is_working() {
                status.working += 1;
            }
        }
        status
    }

    /// The thread a session is bound to.
    pub fn route(&self, conn: ConnId, session: &str) -> Option<&ThreadId> {
        self.routes.get(&(conn, session.to_string()))
    }

    // -----------------------------------------------------------------
    // Threads

    /// A new thread with `agent`, in `root`. Nothing starts until it's
    /// focused or prompted.
    pub fn new_thread(&mut self, agent: &str, root: &Path) -> ThreadId {
        let id = ThreadId(format!("{}-{}", self.id_prefix, self.next_thread));
        self.next_thread += 1;
        let meta = ThreadMeta { id: id.clone(), agent: agent.to_string(), root: root.to_path_buf(), session_id: None, handed_off_from: None };
        self.insert(meta, Thread::default());
        id
    }

    /// A thread read back from disk: detached until it's looked at.
    pub fn restore(&mut self, meta: ThreadMeta, thread: Thread) {
        if !self.threads.contains_key(&meta.id) {
            self.insert(meta, thread);
        }
    }

    fn insert(&mut self, meta: ThreadMeta, thread: Thread) {
        let id = meta.id.clone();
        self.order.push(id.clone());
        self.threads.insert(
            id,
            Slot { meta, thread, link: Link::Detached, turn: Turn::Idle, queue: VecDeque::new(), replay: None, handoff: None, permissions: HashMap::new(), served: HashSet::new(), announced: HashSet::new() },
        );
    }

    /// A thread for a session the agent listed (`session/list`), not yet
    /// known to Fenix.
    pub fn import(&mut self, agent: &str, root: &Path, listed: &ListedSession) -> ThreadId {
        if let Some(existing) = self.order.iter().find(|id| self.threads[*id].meta.session_id.as_deref() == Some(&listed.session_id) && self.threads[*id].meta.agent == agent) {
            return existing.clone();
        }
        let id = self.new_thread(agent, root);
        let slot = self.threads.get_mut(&id).unwrap();
        slot.meta.session_id = Some(listed.session_id.clone());
        slot.thread.title = listed.title.clone();
        id
    }

    /// The panel shows this thread now. A detached thread attaches, so
    /// its modes and commands are there when you look.
    pub fn focus(&mut self, id: &ThreadId) -> Vec<Effect> {
        if !self.threads.contains_key(id) {
            return Vec::new();
        }
        self.focused = Some(id.clone());
        let mut fx = Vec::new();
        if matches!(self.threads[id].link, Link::Detached) {
            self.attach(id, &mut fx);
        }
        fx
    }

    pub fn unfocus(&mut self) {
        self.focused = None;
    }

    /// Sends a prompt, or queues it: while a turn runs (one turn at a
    /// time per session), or until the thread is attached.
    pub fn prompt(&mut self, id: &ThreadId, mut blocks: Vec<ContentBlock>) -> Vec<Effect> {
        let mut fx = Vec::new();
        let Some(slot) = self.threads.get_mut(id) else { return fx };
        if let Link::ReadOnly(why) = &slot.link {
            let why = why.clone();
            slot.thread.notice(NoticeKind::Warning, format!("This thread can't continue: {why}. Continue it in a new session to go on."));
            fx.push(Effect::Changed(id.clone()));
            return fx;
        }
        if let Some(transcript) = slot.handoff.take() {
            blocks.insert(0, ContentBlock::Resource { uri: "fenix://handoff/transcript.md".into(), text: Some(transcript), mime_type: Some("text/markdown".into()) });
            blocks.insert(0, ContentBlock::text("This continues an earlier conversation; its transcript is attached. Carry on from where it ended."));
        }
        slot.thread.push_user(blocks.clone(), true);
        slot.queue.push_back(blocks);
        fx.push(Effect::Changed(id.clone()));
        match slot.link {
            Link::Detached => self.attach(id, &mut fx),
            Link::Live { .. } => self.drain(id, &mut fx),
            _ => {}
        }
        fx
    }

    /// Cancels the turn under way; with none, takes back the newest
    /// queued prompt.
    pub fn cancel(&mut self, id: &ThreadId, now: Instant) -> Vec<Effect> {
        let mut fx = Vec::new();
        let Some(slot) = self.threads.get_mut(id) else { return fx };
        match (&slot.turn, &slot.link) {
            (Turn::Prompting { request }, Link::Live { conn, session }) => {
                let (conn, request) = (*conn, *request);
                fx.push(Effect::Notify { conn, method: "session/cancel", params: json!({ "sessionId": session }) });
                slot.turn = Turn::Cancelling { request, since: now };
                Self::withdraw_permissions(slot, &mut fx);
            }
            (Turn::Idle, _) if slot.queue.pop_back().is_some() => {
                slot.thread.unqueue_last();
            }
            _ => {}
        }
        fx.push(Effect::Changed(id.clone()));
        fx
    }

    fn withdraw_permissions(slot: &mut Slot, fx: &mut Vec<Effect>) {
        for key in slot.thread.withdraw_permissions() {
            if let Some((conn, request)) = slot.permissions.remove(&key) {
                fx.push(Effect::Respond { conn, id: request, result: Ok(protocol::permission_result(None)) });
            }
        }
    }

    /// The user picked a permission option (`None`: dismissed it, which
    /// answers as cancelled).
    pub fn answer_permission(&mut self, id: &ThreadId, key: &str, option: Option<&str>) -> Vec<Effect> {
        let mut fx = Vec::new();
        let Some(slot) = self.threads.get_mut(id) else { return fx };
        let Some((conn, request)) = slot.permissions.remove(key) else { return fx };
        let name = slot.thread.permission_mut(key).and_then(|c| option.and_then(|o| c.options.iter().find(|x| x.id == o)).map(|o| o.name.clone()));
        let state = match name {
            Some(name) => PermissionState::Answered(name),
            None => PermissionState::Withdrawn,
        };
        slot.thread.set_permission_state(key, state);
        fx.push(Effect::Respond { conn, id: request, result: Ok(protocol::permission_result(option)) });
        fx.push(Effect::Changed(id.clone()));
        fx
    }

    pub fn set_mode(&mut self, id: &ThreadId, mode: &str) -> Vec<Effect> {
        let mut fx = Vec::new();
        let Some(slot) = self.threads.get_mut(id) else { return fx };
        let Link::Live { conn, session } = &slot.link else { return fx };
        let (conn, session) = (*conn, session.clone());
        let previous = slot.thread.modes.as_ref().map(|m| m.current.clone());
        slot.thread.apply(SessionUpdate::CurrentMode(mode.to_string()), false);
        let request = self.send(conn, "session/set_mode", json!({ "sessionId": session, "modeId": mode }), Pending::SetMode { thread: id.clone(), previous }, &mut fx);
        if request.is_some() {
            fx.push(Effect::Changed(id.clone()));
        }
        fx
    }

    pub fn set_config_option(&mut self, id: &ThreadId, config_id: &str, value: ConfigValue) -> Vec<Effect> {
        let mut fx = Vec::new();
        let Some(slot) = self.threads.get_mut(id) else { return fx };
        let Link::Live { conn, session } = &slot.link else { return fx };
        let (conn, session) = (*conn, session.clone());
        let previous = slot.thread.config_options.clone();
        let mut options = previous.clone();
        if let Some(option) = options.iter_mut().find(|o| o.id == config_id) {
            option.value = match (&option.value, &value) {
                (ConfigValue::Select { choices, .. }, ConfigValue::Select { current, .. }) => ConfigValue::Select { current: current.clone(), choices: choices.clone() },
                (_, v) => v.clone(),
            };
        }
        slot.thread.set_config_options(options);
        self.send(conn, "session/set_config_option", protocol::set_config_params(&session, config_id, &value), Pending::SetConfig { thread: id.clone(), previous }, &mut fx);
        fx.push(Effect::Changed(id.clone()));
        fx
    }

    /// The thread's page was closed: its session is let go (`session/close`
    /// when the agent has it), the thread stays.
    pub fn close(&mut self, id: &ThreadId, now: Instant) -> Vec<Effect> {
        let mut fx = Vec::new();
        if matches!(self.threads.get(id).map(|s| &s.turn), Some(Turn::Prompting { .. })) {
            fx.extend(self.cancel(id, now));
        }
        let Some(slot) = self.threads.get_mut(id) else { return fx };
        if let Link::Live { conn, session } = slot.link.clone() {
            self.routes.remove(&(conn, session.clone()));
            slot.link = Link::Detached;
            slot.turn = Turn::Idle;
            if self.connections.get(&conn).and_then(|c| c.info.as_ref()).is_some_and(|i| i.session_close) {
                self.send(conn, "session/close", json!({ "sessionId": session }), Pending::Ignore, &mut fx);
            }
            self.mark_idle(conn, now);
        }
        if self.focused.as_ref() == Some(id) {
            self.focused = None;
        }
        fx.push(Effect::Save(id.clone()));
        fx.push(Effect::Changed(id.clone()));
        fx
    }

    /// Deletes the thread: its session too, when the agent can.
    pub fn delete(&mut self, id: &ThreadId, now: Instant) -> Vec<Effect> {
        let mut fx = self.close(id, now);
        fx.retain(|e| !matches!(e, Effect::Save(_)));
        let Some(slot) = self.threads.remove(id) else { return fx };
        self.order.retain(|t| t != id);
        if let Some(session) = &slot.meta.session_id {
            if let Some(conn) = self.find_conn(&slot.meta.agent, &slot.meta.root) {
                if self.connections.get(&conn).is_some_and(|c| c.state == ConnState::Ready && c.info.as_ref().is_some_and(|i| i.session_delete)) {
                    self.send(conn, "session/delete", json!({ "sessionId": session }), Pending::Ignore, &mut fx);
                }
            }
        }
        fx.push(Effect::Forget(id.clone()));
        fx
    }

    /// A new thread with `agent`, carrying this one's transcript (trimmed
    /// to `max_bytes`, its end kept) to its first prompt.
    pub fn hand_off(&mut self, from: &ThreadId, agent: &str, max_bytes: usize) -> Option<ThreadId> {
        let (root, transcript, title) = {
            let slot = self.threads.get(from)?;
            (slot.meta.root.clone(), slot.thread.transcript(), slot.thread.display_title())
        };
        let id = self.new_thread(agent, &root);
        let slot = self.threads.get_mut(&id).unwrap();
        slot.meta.handed_off_from = Some(from.clone());
        slot.handoff = Some(trim_front(&transcript, max_bytes));
        slot.thread.title = Some(format!("{title} (continued)"));
        slot.thread.notice(NoticeKind::Info, "Continues an earlier thread: its transcript goes with your first prompt.");
        Some(id)
    }

    /// A read-only thread goes on in a new session of the same agent,
    /// with its transcript handed over.
    pub fn continue_in_new_session(&mut self, id: &ThreadId, max_bytes: usize) -> Vec<Effect> {
        let Some(slot) = self.threads.get_mut(id) else { return Vec::new() };
        if !matches!(slot.link, Link::ReadOnly(_)) {
            return Vec::new();
        }
        slot.handoff = Some(trim_front(&slot.thread.transcript(), max_bytes));
        slot.meta.session_id = None;
        slot.link = Link::Detached;
        slot.thread.notice(NoticeKind::Info, "Continuing in a new session: the transcript goes with your next prompt.");
        vec![Effect::Changed(id.clone()), Effect::Save(id.clone())]
    }

    /// Lists the sessions `agent` has for `root` (`session/list`): the
    /// answer comes as `Effect::Listed`.
    pub fn list_sessions(&mut self, agent: &str, root: &Path) -> Vec<Effect> {
        let mut fx = Vec::new();
        let Some(conn) = self.ensure_conn(agent, root, &mut fx) else { return fx };
        let c = self.connections.get_mut(&conn).unwrap();
        if c.state == ConnState::Ready {
            self.send_list(conn, &mut fx);
        } else {
            c.deferred.push_back(Deferred::List);
        }
        fx
    }

    fn send_list(&mut self, conn: ConnId, fx: &mut Vec<Effect>) {
        let c = &self.connections[&conn];
        if !c.info.as_ref().is_some_and(|i| i.session_list) {
            fx.push(Effect::Listed { agent: c.agent.id.clone(), root: c.root.clone(), sessions: Vec::new() });
            return;
        }
        let root = c.root.clone();
        self.send(conn, "session/list", json!({ "cwd": path_text(&root) }), Pending::List { root }, fx);
    }

    // -----------------------------------------------------------------
    // Logins

    /// Logs in with one of the agent's methods.
    pub fn authenticate(&mut self, conn: ConnId, method: &str) -> Vec<Effect> {
        let mut fx = Vec::new();
        let Some(c) = self.connections.get_mut(&conn) else { return fx };
        let Some(m) = c.info.as_ref().and_then(|i| i.auth_methods.iter().find(|m| m.id == method)).cloned() else { return fx };
        c.state = ConnState::Authenticating;
        match m.kind {
            AuthKind::Agent => {
                self.send(conn, "authenticate", json!({ "methodId": m.id }), Pending::Authenticate, &mut fx);
            }
            AuthKind::Terminal { args, env } => fx.push(Effect::TerminalLogin { conn, method: m.id, args, env }),
            AuthKind::Other(_) => {
                c.state = ConnState::NeedsAuth;
            }
        }
        fx.push(Effect::ConnectionChanged(conn));
        fx
    }

    /// The terminal login ended; `ok` when it exited 0.
    pub fn terminal_login_finished(&mut self, conn: ConnId, ok: bool) -> Vec<Effect> {
        let mut fx = Vec::new();
        if let Some(c) = self.connections.get_mut(&conn) {
            if ok {
                c.state = ConnState::Ready;
                self.ready(conn, &mut fx);
            } else {
                c.state = ConnState::NeedsAuth;
            }
            fx.push(Effect::ConnectionChanged(conn));
        }
        fx
    }

    pub fn logout(&mut self, conn: ConnId) -> Vec<Effect> {
        let mut fx = Vec::new();
        if self.connections.get(&conn).and_then(|c| c.info.as_ref()).is_some_and(|i| i.logout) {
            self.send(conn, "logout", json!({}), Pending::Logout, &mut fx);
        }
        fx
    }

    /// Starts a dead agent again by hand (`SPC a R`), its crash count
    /// cleared.
    pub fn restart(&mut self, conn: ConnId, now: Instant) -> Vec<Effect> {
        let mut fx = Vec::new();
        let Some(c) = self.connections.get_mut(&conn) else { return fx };
        c.crashes.clear();
        if c.is_up() {
            fx.push(Effect::Stop { conn });
            self.lost(conn, "restarted".to_string(), now, &mut fx, false);
        }
        self.respawn(conn, &mut fx);
        fx
    }

    // -----------------------------------------------------------------
    // Attaching

    fn find_conn(&self, agent: &str, root: &Path) -> Option<ConnId> {
        self.connections.iter().find(|(_, c)| c.agent.id == agent && c.root == root).map(|(id, _)| *id)
    }

    /// The connection for `agent` in `root`, started if there's none or
    /// it died and may start again.
    fn ensure_conn(&mut self, agent: &str, root: &Path, fx: &mut Vec<Effect>) -> Option<ConnId> {
        if let Some(conn) = self.find_conn(agent, root) {
            let c = &self.connections[&conn];
            if let ConnState::Dead { retry_at, .. } = &c.state {
                if retry_at.is_some() {
                    // Starting again is the tick's to do, after the backoff.
                } else if c.crashes.len() < MAX_CRASHES {
                    self.respawn(conn, fx);
                }
            }
            return Some(conn);
        }
        let spec = self.agents.get(agent)?.clone();
        let conn = ConnId(self.next_conn);
        self.next_conn += 1;
        self.connections.insert(
            conn,
            Connection { agent: spec.clone(), root: root.to_path_buf(), generation: 1, state: ConnState::Spawning, info: None, next_id: 1, pending: HashMap::new(), deferred: VecDeque::new(), crashes: VecDeque::new(), idle_since: None },
        );
        fx.push(Effect::Spawn { conn, generation: 1, agent: spec, root: root.to_path_buf() });
        fx.push(Effect::ConnectionChanged(conn));
        Some(conn)
    }

    fn respawn(&mut self, conn: ConnId, fx: &mut Vec<Effect>) {
        let Some(c) = self.connections.get_mut(&conn) else { return };
        c.generation += 1;
        c.state = ConnState::Spawning;
        c.info = None;
        c.pending.clear();
        fx.push(Effect::Spawn { conn, generation: c.generation, agent: c.agent.clone(), root: c.root.clone() });
        fx.push(Effect::ConnectionChanged(conn));
    }

    fn attach(&mut self, id: &ThreadId, fx: &mut Vec<Effect>) {
        let Some(slot) = self.threads.get(id) else { return };
        if !matches!(slot.link, Link::Detached) {
            return;
        }
        let (agent, root) = (slot.meta.agent.clone(), slot.meta.root.clone());
        let how = if slot.meta.session_id.is_none() { Attach::New } else { Attach::Resume };
        let Some(conn) = self.ensure_conn(&agent, &root, fx) else {
            let slot = self.threads.get_mut(id).unwrap();
            slot.link = Link::ReadOnly(format!("the agent `{agent}` isn't set up in Fenix"));
            slot.thread.notice(NoticeKind::Error, format!("The agent `{agent}` isn't set up any more (`agents.servers`)."));
            Self::drop_queue(slot, id, fx);
            fx.push(Effect::Changed(id.clone()));
            return;
        };
        self.threads.get_mut(id).unwrap().link = Link::Attaching { conn, how };
        fx.push(Effect::Changed(id.clone()));
        let c = self.connections.get_mut(&conn).unwrap();
        c.idle_since = None;
        if c.state == ConnState::Ready {
            self.send_attach(id, conn, fx);
        } else {
            c.deferred.push_back(Deferred::Attach(id.clone()));
        }
    }

    /// Sends `session/new`, or takes an existing session back the best
    /// way the agent offers: resume, else load, else the thread is
    /// read-only.
    fn send_attach(&mut self, id: &ThreadId, conn: ConnId, fx: &mut Vec<Effect>) {
        let info = self.connections[&conn].info.clone().unwrap_or_default();
        let servers = self.mcp_values(&info, id);
        let Some(slot) = self.threads.get_mut(id) else { return };
        let mut params = json!({ "cwd": path_text(&slot.meta.root), "mcpServers": servers });
        let (method, how) = match &slot.meta.session_id {
            None => ("session/new", Attach::New),
            Some(session) => {
                params["sessionId"] = json!(session);
                if info.session_resume {
                    ("session/resume", Attach::Resume)
                } else if info.load_session {
                    ("session/load", Attach::Load)
                } else {
                    slot.link = Link::ReadOnly("the agent can't resume or load a session".into());
                    slot.thread.notice(NoticeKind::Warning, "This agent can't take an earlier session back. The transcript stays readable; continue it in a new session to go on.");
                    Self::drop_queue(slot, id, fx);
                    fx.push(Effect::Changed(id.clone()));
                    fx.push(Effect::Save(id.clone()));
                    return;
                }
            }
        };
        slot.link = Link::Attaching { conn, how };
        if let Some(session) = &slot.meta.session_id {
            // The replay (load) and anything the agent sends while taking
            // the session back (resume) need a route already.
            self.routes.insert((conn, session.clone()), id.clone());
            if how == Attach::Load {
                slot.replay = Some(Thread::default());
            }
        }
        self.send(conn, method, params, Pending::Attach { thread: id.clone(), how }, fx);
    }

    /// The MCP servers a session of `id` gets: the stdio ones always,
    /// http and sse ones when the agent takes them.
    fn mcp_values(&self, info: &AgentInfo, id: &ThreadId) -> Vec<Value> {
        self.mcp_servers
            .iter()
            .filter(|s| match s {
                McpServer::Stdio { .. } => true,
                McpServer::Http { .. } => info.mcp_http,
                McpServer::Sse { .. } => info.mcp_sse,
            })
            .map(|s| with_thread(s, id).to_value())
            .collect()
    }

    /// The prompts waiting are dropped: each one done with, as an error.
    fn drop_queue(slot: &mut Slot, id: &ThreadId, fx: &mut Vec<Effect>) {
        while slot.queue.pop_back().is_some() {
            slot.thread.unqueue_last();
            fx.push(Effect::TurnEnded { thread: id.clone(), stop: "error".into() });
        }
    }

    /// Sends the next queued prompt, if the thread can take one.
    fn drain(&mut self, id: &ThreadId, fx: &mut Vec<Effect>) {
        let Some(slot) = self.threads.get_mut(id) else { return };
        let (Link::Live { conn, session }, Turn::Idle) = (&slot.link, &slot.turn) else { return };
        let (conn, session) = (*conn, session.clone());
        if self.connections.get(&conn).map(|c| &c.state) != Some(&ConnState::Ready) {
            return;
        }
        let Some(blocks) = slot.queue.pop_front() else { return };
        slot.thread.mark_sent();
        if let Some(request) = self.send(conn, "session/prompt", protocol::prompt_params(&session, &blocks), Pending::Prompt { thread: id.clone() }, fx) {
            let slot = self.threads.get_mut(id).unwrap();
            slot.turn = Turn::Prompting { request };
        }
        fx.push(Effect::Changed(id.clone()));
    }

    fn send(&mut self, conn: ConnId, method: &'static str, params: Value, pending: Pending, fx: &mut Vec<Effect>) -> Option<i64> {
        let c = self.connections.get_mut(&conn)?;
        let id = c.next_id;
        c.next_id += 1;
        c.pending.insert(id, pending);
        fx.push(Effect::Request { conn, id, method, params });
        Some(id)
    }

    fn mark_idle(&mut self, conn: ConnId, now: Instant) {
        let live = self.threads.values().any(|s| matches!(&s.link, Link::Live { conn: c, .. } | Link::Attaching { conn: c, .. } if *c == conn));
        if let Some(c) = self.connections.get_mut(&conn) {
            c.idle_since = if live { None } else { Some(now) };
        }
    }

    // -----------------------------------------------------------------
    // Events

    /// The agent process started (or couldn't).
    pub fn on_spawned(&mut self, conn: ConnId, generation: u32, result: Result<(), String>, now: Instant) -> Vec<Effect> {
        let mut fx = Vec::new();
        let Some(c) = self.connections.get_mut(&conn) else { return fx };
        if c.generation != generation {
            return fx;
        }
        match result {
            Ok(()) => {
                c.state = ConnState::Initializing;
                let params = protocol::initialize_params(&self.capabilities, &self.client_version);
                self.send(conn, "initialize", params, Pending::Initialize, &mut fx);
            }
            Err(e) => {
                let name = c.agent.name.clone();
                // A program that won't start won't start on its own later:
                // no retry until asked.
                c.state = ConnState::Dead { reason: e.clone(), retry_at: None };
                c.crashes.extend(std::iter::repeat_n(now, MAX_CRASHES));
                self.fail_deferred(conn, &format!("Couldn't start {name}: {e}"), &mut fx);
            }
        }
        fx.push(Effect::ConnectionChanged(conn));
        fx
    }

    /// Everything waiting for a connection that won't come.
    fn fail_deferred(&mut self, conn: ConnId, why: &str, fx: &mut Vec<Effect>) {
        let deferred: Vec<_> = self.connections.get_mut(&conn).map(|c| c.deferred.drain(..).collect()).unwrap_or_default();
        for d in deferred {
            match d {
                Deferred::Attach(id) => {
                    if let Some(slot) = self.threads.get_mut(&id) {
                        slot.link = Link::Detached;
                        slot.thread.notice(NoticeKind::Error, why);
                        Self::drop_queue(slot, &id, fx);
                        fx.push(Effect::Changed(id));
                    }
                }
                Deferred::List => {
                    let c = &self.connections[&conn];
                    fx.push(Effect::Listed { agent: c.agent.id.clone(), root: c.root.clone(), sessions: Vec::new() });
                }
            }
        }
    }

    pub fn on_event(&mut self, conn: ConnId, generation: u32, event: AcpEvent, now: Instant) -> Vec<Effect> {
        let mut fx = Vec::new();
        let Some(c) = self.connections.get(&conn) else { return fx };
        if c.generation != generation {
            return fx;
        }
        match event {
            AcpEvent::Response { id, result } => self.on_response(conn, id, result, now, &mut fx),
            AcpEvent::Notification { method, params } => self.on_notification(conn, &method, &params, &mut fx),
            AcpEvent::Request { id, method, params } => self.on_request(conn, id, &method, &params, &mut fx),
            AcpEvent::Disconnected(why) => self.lost(conn, why, now, &mut fx, true),
        }
        fx
    }

    fn on_response(&mut self, conn: ConnId, id: i64, result: Result<Value, RpcError>, now: Instant, fx: &mut Vec<Effect>) {
        let Some(pending) = self.connections.get_mut(&conn).and_then(|c| c.pending.remove(&id)) else { return };
        match pending {
            Pending::Initialize => {
                let c = self.connections.get_mut(&conn).unwrap();
                match result {
                    Ok(v) => {
                        let info = AgentInfo::parse(&v);
                        if info.protocol_version != protocol::PROTOCOL_VERSION {
                            c.state = ConnState::Dead { reason: format!("it speaks ACP v{}; Fenix speaks v{}", info.protocol_version, protocol::PROTOCOL_VERSION), retry_at: None };
                            fx.push(Effect::Stop { conn });
                            self.fail_deferred(conn, "This agent speaks another version of ACP than Fenix.", fx);
                        } else {
                            c.info = Some(info);
                            c.state = ConnState::Ready;
                            self.ready(conn, fx);
                        }
                    }
                    Err(e) => {
                        c.state = ConnState::Dead { reason: format!("initialize failed: {e}"), retry_at: None };
                        fx.push(Effect::Stop { conn });
                        self.fail_deferred(conn, &format!("The agent didn't start a conversation: {e}"), fx);
                    }
                }
                fx.push(Effect::ConnectionChanged(conn));
            }
            Pending::Attach { thread, how } => self.attached(conn, &thread, how, result, now, fx),
            Pending::Prompt { thread } => self.turn_ended(conn, &thread, result, fx),
            Pending::SetMode { thread, previous } => {
                if let (Err(e), Some(slot)) = (&result, self.threads.get_mut(&thread)) {
                    if let (Some(modes), Some(previous)) = (&mut slot.thread.modes, previous) {
                        modes.current = previous;
                    }
                    slot.thread.notice(NoticeKind::Warning, format!("The agent kept its mode: {}", e.message));
                    fx.push(Effect::Changed(thread));
                }
            }
            Pending::SetConfig { thread, previous } => {
                if let Some(slot) = self.threads.get_mut(&thread) {
                    match result {
                        Ok(v) => {
                            if let Some(options) = v.get("configOptions").and_then(Value::as_array) {
                                slot.thread.set_config_options(options.iter().filter_map(protocol::ConfigOption::parse).collect());
                            }
                        }
                        Err(e) => {
                            slot.thread.set_config_options(previous);
                            slot.thread.notice(NoticeKind::Warning, format!("The agent kept its setting: {}", e.message));
                        }
                    }
                    fx.push(Effect::Changed(thread));
                }
            }
            Pending::Authenticate => {
                let c = self.connections.get_mut(&conn).unwrap();
                match result {
                    Ok(_) => {
                        c.state = ConnState::Ready;
                        self.ready(conn, fx);
                    }
                    Err(e) => {
                        c.state = ConnState::NeedsAuth;
                        self.notice_conn(conn, NoticeKind::Login, &format!("Login failed: {}", e.message), fx);
                    }
                }
                fx.push(Effect::ConnectionChanged(conn));
            }
            Pending::Logout => {
                let c = self.connections.get_mut(&conn).unwrap();
                if result.is_ok() {
                    c.state = ConnState::NeedsAuth;
                }
                fx.push(Effect::ConnectionChanged(conn));
            }
            Pending::List { root } => {
                let agent = self.connections[&conn].agent.id.clone();
                let sessions = match result {
                    Ok(v) => protocol::parse_session_list(&v).0.into_iter().filter(|s| Path::new(&s.cwd) == root).collect(),
                    Err(_) => Vec::new(),
                };
                fx.push(Effect::Listed { agent, root, sessions });
            }
            Pending::Ignore => {}
        }
    }

    /// The connection is ready: send what waited for it, and the prompts
    /// of threads already live on it.
    fn ready(&mut self, conn: ConnId, fx: &mut Vec<Effect>) {
        let deferred: Vec<_> = self.connections.get_mut(&conn).map(|c| c.deferred.drain(..).collect()).unwrap_or_default();
        for d in deferred {
            match d {
                Deferred::Attach(id) => {
                    if matches!(self.threads.get(&id).map(|s| &s.link), Some(Link::Attaching { .. })) {
                        self.send_attach(&id, conn, fx);
                    }
                }
                Deferred::List => self.send_list(conn, fx),
            }
        }
        let live: Vec<ThreadId> = self.order.iter().filter(|id| matches!(&self.threads[*id].link, Link::Live { conn: c, .. } if *c == conn)).cloned().collect();
        for id in live {
            self.drain(&id, fx);
        }
    }

    fn attached(&mut self, conn: ConnId, id: &ThreadId, how: Attach, result: Result<Value, RpcError>, now: Instant, fx: &mut Vec<Effect>) {
        let Some(slot) = self.threads.get_mut(id) else { return };
        if !matches!(slot.link, Link::Attaching { .. }) {
            return;
        }
        match result {
            Ok(v) => {
                let setup = SessionSetup::parse(&v);
                let session = match (how, &setup.session_id, &slot.meta.session_id) {
                    (Attach::New, Some(new), _) => new.clone(),
                    (Attach::New, None, _) => {
                        slot.link = Link::Detached;
                        slot.thread.notice(NoticeKind::Error, "The agent started a session without an id.");
                        Self::drop_queue(slot, id, fx);
                        fx.push(Effect::Changed(id.clone()));
                        return;
                    }
                    (_, _, Some(existing)) => existing.clone(),
                    (_, _, None) => return,
                };
                if how == Attach::Load {
                    if let Some(replayed) = slot.replay.take() {
                        // An agent that loads without replaying leaves the
                        // stored conversation as it was.
                        if !replayed.entries.is_empty() {
                            let local_queue: Vec<_> = slot.thread.entries.iter().filter(|e| matches!(e, crate::thread::Entry::User { queued: true, .. })).cloned().collect();
                            slot.thread.entries = replayed.entries;
                            slot.thread.entries.extend(local_queue);
                            slot.thread.plan = replayed.plan;
                        }
                        if !replayed.commands.is_empty() {
                            slot.thread.commands = replayed.commands;
                        }
                        if replayed.usage.is_some() {
                            slot.thread.usage = replayed.usage;
                        }
                        if replayed.title.is_some() {
                            slot.thread.title = replayed.title;
                        }
                        slot.thread.revision += 1;
                    }
                }
                if setup.modes.is_some() {
                    slot.thread.set_modes(setup.modes);
                }
                if let Some(options) = setup.config_options {
                    slot.thread.set_config_options(options);
                }
                slot.meta.session_id = Some(session.clone());
                slot.link = Link::Live { conn, session: session.clone() };
                self.routes.insert((conn, session), id.clone());
                fx.push(Effect::Changed(id.clone()));
                fx.push(Effect::Save(id.clone()));
                self.drain(id, fx);
            }
            Err(e) => {
                slot.replay = None;
                if let Some(session) = &slot.meta.session_id {
                    self.routes.remove(&(conn, session.clone()));
                }
                if e.code == code::AUTH_REQUIRED {
                    slot.link = Link::Attaching { conn, how };
                    let c = self.connections.get_mut(&conn).unwrap();
                    c.state = ConnState::NeedsAuth;
                    c.deferred.push_front(Deferred::Attach(id.clone()));
                    self.notice_login(conn, id, fx);
                    return;
                }
                match how {
                    Attach::Resume if self.connections[&conn].info.as_ref().is_some_and(|i| i.load_session) && e.code != code::SESSION_NOT_FOUND => {
                        // Resume failed for its own reasons: load may work.
                        slot.link = Link::Attaching { conn, how: Attach::Load };
                        slot.replay = Some(Thread::default());
                        let session = slot.meta.session_id.clone().unwrap_or_default();
                        let root = path_text(&slot.meta.root);
                        self.routes.insert((conn, session.clone()), id.clone());
                        let info = self.connections[&conn].info.clone().unwrap_or_default();
                        let params = json!({ "sessionId": session, "cwd": root, "mcpServers": self.mcp_values(&info, id) });
                        self.send(conn, "session/load", params, Pending::Attach { thread: id.clone(), how: Attach::Load }, fx);
                    }
                    Attach::Resume | Attach::Load => {
                        slot.link = Link::ReadOnly(format!("the agent no longer has this session ({})", e.message));
                        slot.thread.notice(NoticeKind::Warning, format!("The agent couldn't take this session back: {}. The transcript stays readable; continue it in a new session to go on.", e.message));
                        Self::drop_queue(slot, id, fx);
                        fx.push(Effect::Save(id.clone()));
                    }
                    Attach::New => {
                        slot.link = Link::Detached;
                        slot.thread.notice(NoticeKind::Error, format!("The agent couldn't start a session: {}", e.message));
                        Self::drop_queue(slot, id, fx);
                    }
                }
                fx.push(Effect::Changed(id.clone()));
                self.mark_idle(conn, now);
            }
        }
    }

    fn notice_login(&mut self, conn: ConnId, id: &ThreadId, fx: &mut Vec<Effect>) {
        let name = self.connections[&conn].agent.name.clone();
        if let Some(slot) = self.threads.get_mut(id) {
            slot.thread.notice(NoticeKind::Login, format!("{name} wants you to log in. Pick a way below (`SPC a p` too)."));
        }
        fx.push(Effect::Changed(id.clone()));
        fx.push(Effect::ConnectionChanged(conn));
        fx.push(Effect::Attention { thread: id.clone(), why: Attention::Login });
    }

    fn notice_conn(&mut self, conn: ConnId, kind: NoticeKind, text: &str, fx: &mut Vec<Effect>) {
        let ids: Vec<ThreadId> = self.order.iter().filter(|id| self.connection_of(id) == Some(conn) && self.focused.as_ref() == Some(*id)).cloned().collect();
        for id in ids {
            if let Some(slot) = self.threads.get_mut(&id) {
                slot.thread.notice(kind, text);
                fx.push(Effect::Changed(id));
            }
        }
    }

    fn turn_ended(&mut self, conn: ConnId, id: &ThreadId, result: Result<Value, RpcError>, fx: &mut Vec<Effect>) {
        let Some(slot) = self.threads.get_mut(id) else { return };
        slot.turn = Turn::Idle;
        let stop = match &result {
            Ok(v) => v.get("stopReason").and_then(Value::as_str).unwrap_or("end_turn").to_string(),
            Err(_) => "error".to_string(),
        };
        match result {
            Ok(v) => match StopReason::parse(&v) {
                StopReason::EndTurn => {}
                StopReason::Cancelled => {
                    slot.thread.end_running_tools();
                    slot.thread.notice(NoticeKind::Interrupted, "Cancelled.");
                }
                StopReason::MaxTokens => slot.thread.notice(NoticeKind::Stopped, "The agent hit its token limit for this answer."),
                StopReason::MaxTurnRequests => slot.thread.notice(NoticeKind::Stopped, "The agent hit its limit of requests for one turn."),
                StopReason::Refusal => slot.thread.notice(NoticeKind::Stopped, "The agent refused to go on."),
                StopReason::Other(reason) => slot.thread.notice(NoticeKind::Stopped, format!("The turn ended: {reason}.")),
            },
            Err(e) if e.code == code::AUTH_REQUIRED => {
                // The prompt goes again once logged in.
                if let Some(crate::thread::Entry::User { blocks, .. }) = slot.thread.entries.iter().rev().find(|e| matches!(e, crate::thread::Entry::User { queued: false, .. })).cloned() {
                    slot.queue.push_front(blocks);
                    if let Some(crate::thread::Entry::User { queued, .. }) = slot.thread.entries.iter_mut().rev().find(|e| matches!(e, crate::thread::Entry::User { queued: false, .. })) {
                        *queued = true;
                    }
                }
                self.connections.get_mut(&conn).unwrap().state = ConnState::NeedsAuth;
                self.notice_login(conn, id, fx);
                return;
            }
            Err(e) if e.code == code::SESSION_NOT_FOUND => {
                if let Link::Live { session, .. } = &slot.link {
                    self.routes.remove(&(conn, session.clone()));
                }
                slot.link = Link::ReadOnly("the agent lost this session".into());
                slot.thread.notice(NoticeKind::Error, "The agent no longer has this session. Continue it in a new session to go on.");
                fx.push(Effect::TurnEnded { thread: id.clone(), stop });
                Self::drop_queue(slot, id, fx);
                fx.push(Effect::Changed(id.clone()));
                fx.push(Effect::Save(id.clone()));
                return;
            }
            Err(e) => slot.thread.notice(NoticeKind::Error, format!("The agent failed: {}", e.message)),
        }
        fx.push(Effect::Changed(id.clone()));
        fx.push(Effect::Save(id.clone()));
        fx.push(Effect::Attention { thread: id.clone(), why: Attention::TurnEnded });
        fx.push(Effect::TurnEnded { thread: id.clone(), stop });
        self.drain(id, fx);
    }

    fn on_notification(&mut self, conn: ConnId, method: &str, params: &Value, fx: &mut Vec<Effect>) {
        if method != "session/update" {
            // `$/cancel_request` for a request of the agent's Fenix is
            // serving, and agents' own `_` extensions: nothing to do.
            return;
        }
        let Some((session, update)) = SessionUpdate::parse_params(params) else { return };
        let Some(id) = self.routes.get(&(conn, session)).cloned() else { return };
        let Some(slot) = self.threads.get_mut(&id) else { return };
        if let SessionUpdate::ToolCall(fields) | SessionUpdate::ToolCallUpdate(fields) = &update {
            let kind = fields.kind.or_else(|| slot.thread.tool(&fields.id).map(|c| c.kind));
            if kind.is_some_and(|k| k.changes_files()) && slot.replay.is_none() {
                let paths: Vec<String> = fields.named_paths().into_iter().filter(|p| slot.announced.insert(p.clone())).collect();
                if !paths.is_empty() {
                    fx.push(Effect::Announced { thread: id.clone(), tool_call: fields.id.clone(), paths });
                }
            }
        }
        match &mut slot.replay {
            Some(replay) => replay.apply(update, true),
            None => slot.thread.apply(update, false),
        }
        fx.push(Effect::Changed(id));
    }

    fn on_request(&mut self, conn: ConnId, id: Value, method: &str, params: &Value, fx: &mut Vec<Effect>) {
        let request = match ClientRequest::parse(method, params) {
            Ok(r) => r,
            Err((code, message)) => {
                fx.push(Effect::Respond { conn, id, result: Err(RpcError::new(code, message)) });
                return;
            }
        };
        let Some(thread) = self.routes.get(&(conn, request.session_id().to_string())).cloned() else {
            fx.push(Effect::Respond { conn, id, result: Err(RpcError::new(code::INVALID_PARAMS, format!("no session {}", request.session_id()))) });
            return;
        };
        let slot = self.threads.get_mut(&thread).unwrap();
        match request {
            ClientRequest::RequestPermission { tool_call, options, .. } => {
                let key = id.to_string();
                slot.permissions.insert(key.clone(), (conn, id));
                slot.thread.push_permission(PermissionCard { key, tool_call: merged_tool_call(&slot.thread, tool_call), options: options_or_default(options), state: PermissionState::Pending });
                fx.push(Effect::Changed(thread.clone()));
                fx.push(Effect::Attention { thread, why: Attention::Permission });
            }
            request => {
                slot.served.insert((conn, id.to_string()));
                fx.push(Effect::Serve { conn, id, thread, request });
            }
        }
    }

    /// The answer to a request `Effect::Serve` asked for.
    pub fn on_served(&mut self, conn: ConnId, thread: &ThreadId, id: Value, result: Result<Value, RpcError>) -> Vec<Effect> {
        let Some(slot) = self.threads.get_mut(thread) else { return Vec::new() };
        if !slot.served.remove(&(conn, id.to_string())) {
            // Withdrawn meanwhile: the agent restarted or the turn ended.
            return Vec::new();
        }
        vec![Effect::Respond { conn, id, result }]
    }

    /// The agent went away: crashed, closed, or was stopped.
    fn lost(&mut self, conn: ConnId, why: String, now: Instant, fx: &mut Vec<Effect>, crashed: bool) {
        let ids: Vec<ThreadId> = self.order.iter().filter(|id| matches!(&self.threads[*id].link, Link::Live { conn: c, .. } | Link::Attaching { conn: c, .. } if *c == conn)).cloned().collect();
        self.routes.retain(|(c, _), _| *c != conn);
        let mut wanted = false;
        for id in &ids {
            let slot = self.threads.get_mut(id).unwrap();
            slot.link = Link::Detached;
            slot.replay = None;
            if !matches!(slot.turn, Turn::Idle) {
                slot.turn = Turn::Idle;
                slot.thread.end_running_tools();
                slot.thread.notice(NoticeKind::Interrupted, format!("Interrupted: the agent stopped ({why})."));
                fx.push(Effect::TurnEnded { thread: id.clone(), stop: "error".into() });
            }
            for key in slot.thread.withdraw_permissions() {
                slot.permissions.remove(&key);
            }
            slot.permissions.clear();
            slot.served.clear();
            wanted |= !slot.queue.is_empty() || self.focused.as_ref() == Some(id);
            fx.push(Effect::Withdraw { thread: id.clone() });
            fx.push(Effect::Changed(id.clone()));
            fx.push(Effect::Save(id.clone()));
        }
        let Some(c) = self.connections.get_mut(&conn) else { return };
        c.pending.clear();
        if !crashed {
            c.state = ConnState::Dead { reason: why, retry_at: None };
            fx.push(Effect::ConnectionChanged(conn));
            return;
        }
        while c.crashes.front().is_some_and(|t| now.duration_since(*t) > CRASH_WINDOW) {
            c.crashes.pop_front();
        }
        c.crashes.push_back(now);
        let retry_at = if c.crashes.len() >= MAX_CRASHES {
            None
        } else if wanted {
            Some(now + Duration::from_secs(1 << (c.crashes.len() - 1)))
        } else {
            None
        };
        c.state = ConnState::Dead { reason: why, retry_at };
        fx.push(Effect::ConnectionChanged(conn));
        // Threads that were live come back (resume or load) when the
        // agent does, or when they're next looked at.
        if retry_at.is_some() {
            for id in ids {
                if matches!(self.threads[&id].link, Link::Detached) && (!self.threads[&id].queue.is_empty() || self.focused.as_ref() == Some(&id)) {
                    self.threads.get_mut(&id).unwrap().link = Link::Attaching { conn, how: Attach::Resume };
                    self.connections.get_mut(&conn).unwrap().deferred.push_back(Deferred::Attach(id));
                }
            }
        }
    }

    /// The time passes: cancelled turns that never ended, agents to start
    /// again after their backoff, agents idle long enough to stop.
    pub fn tick(&mut self, now: Instant) -> Vec<Effect> {
        let mut fx = Vec::new();
        let stuck: Vec<ThreadId> = self.order.iter().filter(|id| matches!(self.threads[*id].turn, Turn::Cancelling { since, .. } if now.duration_since(since) >= CANCEL_GRACE)).cloned().collect();
        for id in stuck {
            let slot = self.threads.get_mut(&id).unwrap();
            if let (Turn::Cancelling { request, .. }, Link::Live { conn, .. }) = (&slot.turn, &slot.link) {
                if let Some(c) = self.connections.get_mut(conn) {
                    c.pending.remove(request);
                }
            }
            slot.turn = Turn::Idle;
            slot.thread.end_running_tools();
            slot.thread.notice(NoticeKind::Interrupted, "Cancelled. (The agent didn't confirm it stopped.)");
            fx.push(Effect::Changed(id.clone()));
            fx.push(Effect::TurnEnded { thread: id.clone(), stop: "cancelled".into() });
            self.drain(&id, &mut fx);
        }
        let due: Vec<ConnId> = self.connections.iter().filter(|(_, c)| matches!(c.state, ConnState::Dead { retry_at: Some(t), .. } if now >= t)).map(|(id, _)| *id).collect();
        for conn in due {
            self.respawn(conn, &mut fx);
        }
        let idle: Vec<ConnId> = self.connections.iter().filter(|(_, c)| c.idle_since.is_some_and(|t| now.duration_since(t) >= self.idle_timeout) && c.deferred.is_empty()).map(|(id, _)| *id).collect();
        for conn in idle {
            fx.push(Effect::Stop { conn });
            self.connections.remove(&conn);
            fx.push(Effect::ConnectionChanged(conn));
        }
        fx
    }

    /// Fenix is quitting: every agent stops, and every thread is saved.
    pub fn shutdown(&mut self) -> Vec<Effect> {
        let mut fx: Vec<Effect> = self.order.iter().map(|id| Effect::Save(id.clone())).collect();
        fx.extend(self.connections.keys().map(|conn| Effect::Stop { conn: *conn }));
        fx
    }

    pub fn meta(&self, id: &ThreadId) -> Option<&ThreadMeta> {
        self.threads.get(id).map(|s| &s.meta)
    }

    /// The thread, to change it from outside the protocol (a notice of
    /// Fenix's own: unsaved edits under an announced write).
    pub fn thread_mut(&mut self, id: &ThreadId) -> Option<&mut Thread> {
        self.threads.get_mut(id).map(|s| &mut s.thread)
    }
}

fn options_or_default(options: Vec<PermissionOption>) -> Vec<PermissionOption> {
    if options.is_empty() {
        // An agent that asks without options gets yes and no.
        vec![
            PermissionOption { id: "allow".into(), name: "Allow".into(), kind: protocol::PermissionKind::AllowOnce },
            PermissionOption { id: "reject".into(), name: "Reject".into(), kind: protocol::PermissionKind::RejectOnce },
        ]
    } else {
        options
    }
}

/// A permission request's tool call, filled in from the card already in
/// the thread for that call: agents send only what changed.
fn merged_tool_call(thread: &Thread, asked: ToolCallFields) -> ToolCallFields {
    let Some(known) = thread.tool(&asked.id) else { return asked };
    let mut fields = known.fields();
    if asked.title.is_some() {
        fields.title = asked.title;
    }
    if asked.kind.is_some() {
        fields.kind = asked.kind;
    }
    if asked.content.is_some() {
        fields.content = asked.content;
    }
    if asked.locations.is_some() {
        fields.locations = asked.locations;
    }
    if asked.raw_input.is_some() {
        fields.raw_input = asked.raw_input;
    }
    fields
}

fn with_thread(server: &McpServer, id: &ThreadId) -> McpServer {
    match server {
        McpServer::Stdio { name, command, args, env } => McpServer::Stdio { name: name.clone(), command: command.clone(), args: args.iter().map(|a| a.replace("{thread}", &id.0)).collect(), env: env.clone() },
        other => other.clone(),
    }
}

fn path_text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// The end of `text`, at most `max` bytes, cut at a line.
fn trim_front(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let mut start = text.len() - max;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    let cut = &text[start..];
    let cut = cut.find('\n').map(|i| &cut[i + 1..]).unwrap_or(cut);
    format!("[earlier conversation trimmed]\n\n{cut}")
}

#[cfg(test)]
mod tests;
