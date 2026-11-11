//! What travels over the endpoint: JSON-RPC 2.0, framed with
//! `Content-Length` headers like LSP. The reference for other tools is
//! `docs/CONTROL.md`; the types here are its shapes.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The protocol version this build speaks, the one `fenix/hello`
/// negotiates. Raised only for a change an existing client would
/// misread; new methods are announced in `capabilities` instead.
pub const PROTOCOL: u32 = 1;

/// The largest message body the server reads.
pub const MAX_BODY: usize = 4 * 1024 * 1024;

/// The longest header line the server reads, its line ending included.
pub const MAX_HEADER_LINE: usize = 1024;

/// Method names.
pub mod method {
    pub const HELLO: &str = "fenix/hello";
    pub const OPEN: &str = "fenix/open";
    pub const OPEN_PROJECT: &str = "fenix/openProject";
    pub const STATE: &str = "fenix/state";
    pub const COMMANDS: &str = "fenix/commands";
    pub const RUN_COMMAND: &str = "fenix/runCommand";
    pub const NOTEBOOK_CAPTURE: &str = "fenix/notebook.capture";
    pub const AGENDA_TASKS: &str = "fenix/agenda.tasks";
    pub const AGENDA_CLOCK_IN: &str = "fenix/agenda.clockIn";
    pub const AGENDA_CLOCK_OUT: &str = "fenix/agenda.clockOut";
    pub const DIAGNOSTICS: &str = "fenix/diagnostics";
    pub const BUFFERS: &str = "fenix/buffers";
    pub const SELECTION: &str = "fenix/selection";
    pub const TASKS: &str = "fenix/tasks";
    pub const TASKS_RUN: &str = "fenix/tasks.run";
    pub const AGENT_COMMAND: &str = "fenix/agentCommand";
    pub const BUFFER_READ: &str = "fenix/buffer.read";
    pub const LSP: &str = "fenix/lsp";
    pub const DEBUG_STATE: &str = "fenix/debug.state";
    pub const DEBUG_EVALUATE: &str = "fenix/debug.evaluate";
    pub const TOOLS: &str = "fenix/tools";
    pub const TOOLS_CALL: &str = "fenix/tools.call";
    pub const AGENT_TOOLS: &str = "fenix/agent.tools";
    pub const AGENT_CALL: &str = "fenix/agent.call";
    pub const AGENT_CANCEL: &str = "fenix/agent.cancel";
    pub const AGENT_WAIT: &str = "fenix/agent.wait";
    pub const AGENT_RESOURCES: &str = "fenix/agent.resources";
    pub const AGENT_READ: &str = "fenix/agent.read";
    pub const AGENT_SUBSCRIBE: &str = "fenix/agent.subscribe";
    pub const AGENT_PROMPTS: &str = "fenix/agent.prompts";
    pub const AGENT_PROMPT: &str = "fenix/agent.prompt";
    pub const NOTIFY: &str = "fenix/notify";
}

/// Every method this crate knows, besides `fenix/hello` itself, which
/// every build has: what Fenix lists in `capabilities` unless a server
/// is set up with fewer ([`crate::ServerInfo::capabilities`]).
pub const CAPABILITIES: &[&str] = &[
    method::OPEN,
    method::OPEN_PROJECT,
    method::STATE,
    method::COMMANDS,
    method::RUN_COMMAND,
    method::NOTEBOOK_CAPTURE,
    method::AGENDA_TASKS,
    method::AGENDA_CLOCK_IN,
    method::AGENDA_CLOCK_OUT,
    method::DIAGNOSTICS,
    method::BUFFERS,
    method::SELECTION,
    method::TASKS,
    method::TASKS_RUN,
    method::AGENT_COMMAND,
    method::BUFFER_READ,
    method::LSP,
    method::DEBUG_STATE,
    method::DEBUG_EVALUATE,
    method::TOOLS,
    method::TOOLS_CALL,
    method::AGENT_TOOLS,
    method::AGENT_CALL,
    method::AGENT_CANCEL,
    method::AGENT_WAIT,
    method::AGENT_RESOURCES,
    method::AGENT_READ,
    method::AGENT_SUBSCRIBE,
    method::AGENT_PROMPTS,
    method::AGENT_PROMPT,
    method::NOTIFY,
];

/// Error codes: JSON-RPC's own, and Fenix's in the range JSON-RPC
/// leaves to servers.
pub mod code {
    pub const PARSE_ERROR: i64 = -32700;
    pub const INVALID_REQUEST: i64 = -32600;
    pub const METHOD_NOT_FOUND: i64 = -32601;
    pub const INVALID_PARAMS: i64 = -32602;
    pub const INTERNAL_ERROR: i64 = -32603;
    /// `fenix/hello` asked for a protocol this build doesn't speak.
    pub const UNSUPPORTED_PROTOCOL: i64 = -32001;
    /// A request came before `fenix/hello`.
    pub const HELLO_FIRST: i64 = -32002;
}

/// A JSON-RPC error object.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl RpcError {
    pub fn new(code: i64, message: impl Into<String>) -> Self {
        RpcError {
            code,
            message: message.into(),
            data: None,
        }
    }

    pub fn invalid_params(message: impl Into<String>) -> Self {
        RpcError::new(code::INVALID_PARAMS, message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        RpcError::new(code::INTERNAL_ERROR, message)
    }
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.message, self.code)
    }
}

impl std::error::Error for RpcError {}

/// `fenix/hello`'s params.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HelloParams {
    pub protocol: u32,
    pub client: ClientInfo,
}

/// Who is connecting. Its name is for Fenix's own diagnostics; `thread`
/// says which agent thread an MCP bridge (`fenix --mcp --thread <id>`)
/// speaks for, so the `fenix/agent.*` methods know whose rules apply.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClientInfo {
    pub name: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub pid: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread: Option<String>,
}

/// Who a `fenix/agent.*` call comes from: the connection's
/// `fenix/hello`, filled in by the server, never by the caller.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct AgentCaller {
    /// The client's name (`fenix-mcp`, an MCP client's own).
    pub client: String,
    /// The agent thread it speaks for, when Fenix started it.
    pub thread: Option<String>,
}

/// `fenix/hello`'s result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HelloResult {
    pub protocol: u32,
    /// Fenix's version, `1.5.2`.
    pub version: String,
    /// Fenix's process id: what a client on Windows passes to
    /// `AllowSetForegroundWindow` so Fenix may come to the front.
    pub pid: u32,
    pub capabilities: Vec<String>,
}

/// `fenix/open`'s params.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenParams {
    pub paths: Vec<OpenPath>,
    /// Open another window for them instead of using the focused one.
    #[serde(default)]
    pub new_window: bool,
    /// Bring Fenix's window to the front.
    #[serde(default = "yes")]
    pub focus: bool,
}

fn yes() -> bool {
    true
}

/// One file to open, with where to put the cursor (1-based).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenPath {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column: Option<usize>,
}

/// `fenix/openProject`'s params.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenProjectParams {
    pub root: String,
}

/// `fenix/state`'s result. Each field is left out when there's nothing
/// to report: no project, a page rather than a file in focus.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StateResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// The cursor's line in `file`, 1-based.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<usize>,
    /// The agenda's clock, while it runs.
    #[serde(
        default,
        rename = "agendaTimer",
        skip_serializing_if = "Option::is_none"
    )]
    pub agenda_timer: Option<AgendaTimer>,
    /// How much this Fenix has drawn and woken since it started: an idle
    /// window's counts stand still.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stats: Option<Stats>,
}

/// `fenix/state`'s `stats`: counts since Fenix started.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Stats {
    /// Frames that reached the screen, every window's.
    pub frames: u64,
    /// Times the event loop woke up.
    pub wakes: u64,
    /// Times the extensions' deadline clock woke up; it sleeps while no
    /// extension code runs.
    pub extension_clock_wakes: u64,
}

/// The agenda clock that's running: on which task, since when.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgendaTimer {
    pub task_id: u32,
    pub title: String,
    /// When the clock started, RFC 3339 with the local offset.
    pub since: String,
}

/// `fenix/commands`' result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandsResult {
    pub commands: Vec<CommandInfo>,
}

/// One of Fenix's commands, as a launcher or palette lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandInfo {
    /// What `fenix/runCommand` takes: `agenda.open`.
    pub id: String,
    pub description: String,
    /// The keys that run it now, as Fenix's own key menu writes them: a
    /// chord (`C-s`) or the leader's keys (`SPC a a`, `C-k a a` with
    /// standard keys), yours over Fenix's; absent when no key does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keys: Option<String>,
    /// Whether `fenix/runCommand` runs it: true for commands that make
    /// sense with nobody looking at a file in Fenix (open the agenda,
    /// switch project), false for ones that act on what's in front of
    /// you (save, undo, stage this hunk).
    pub external: bool,
}

/// `fenix/runCommand`'s params.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunCommandParams {
    pub id: String,
}

/// Where `fenix/notebook.capture` files a line.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CaptureTarget {
    /// The notebook's Inbox note, made if it isn't there yet.
    #[default]
    Inbox,
    /// Today's journal day, under its Log.
    Journal,
}

/// `fenix/notify`'s params: a notification, from a script or a tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotifyParams {
    pub summary: String,
    #[serde(default)]
    pub body: String,
    /// `low`, `normal` or `critical`.
    #[serde(default)]
    pub urgency: Option<String>,
    /// Who it's from, as the history and the rules name it; `fenix --notify`
    /// says `cli`.
    #[serde(default)]
    pub source: Option<String>,
    /// Another with the same source and tag replaces it.
    #[serde(default)]
    pub tag: Option<String>,
    /// 0 to 100.
    #[serde(default)]
    pub progress: Option<u8>,
    /// What it offers to do: `{ "label", "command" }`, a Fenix command
    /// that may be run from outside, `url:https://...` or `open:<path>[:line]`.
    #[serde(default)]
    pub actions: Vec<NotifyAction>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotifyAction {
    pub label: String,
    pub command: String,
}

/// `fenix/notebook.capture`'s params.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureParams {
    pub text: String,
    #[serde(default)]
    pub target: CaptureTarget,
}

/// `fenix/notebook.capture`'s result: the file the line went into.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureResult {
    pub entry: String,
}

/// A task's place on the agenda's board, as the protocol writes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TaskStatus {
    Todo,
    InProgress,
    Blocked,
    Done,
}

/// `fenix/agenda.tasks`' params.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TasksParams {
    /// Only tasks in one of these; every task when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<Vec<TaskStatus>>,
}

/// `fenix/agenda.tasks`' result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TasksResult {
    pub tasks: Vec<AgendaTask>,
}

/// One agenda task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgendaTask {
    /// What `fenix/agenda.clockIn` takes. Stable for the task's life
    /// and never reused.
    pub id: u32,
    pub title: String,
    pub status: TaskStatus,
    /// The Jira issue it's linked to, `ABC-12`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jira_key: Option<String>,
    /// Every minute logged on it, the running clock's included.
    pub elapsed_minutes: i64,
}

/// `fenix/agenda.clockIn`'s params.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClockInParams {
    /// A task's `id` from `fenix/agenda.tasks`. Its decimal string is
    /// taken too, for clients that keep every id as text.
    #[serde(deserialize_with = "task_id")]
    pub task_id: u32,
}

/// A task id given as a number or as its decimal string.
fn task_id<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<u32, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Id {
        Number(u32),
        Text(String),
    }
    match Id::deserialize(deserializer)? {
        Id::Number(id) => Ok(id),
        Id::Text(text) => text
            .parse()
            .map_err(|_| serde::de::Error::custom(format!("no task has the id {text:?}"))),
    }
}

/// A request the editor itself has to answer, on its UI thread: what
/// the server hands its caller after the protocol's own checks.
#[derive(Debug, Clone, PartialEq)]
pub enum Call {
    Open(OpenParams),
    OpenProject(OpenProjectParams),
    State,
    Commands,
    RunCommand(RunCommandParams),
    NotebookCapture(CaptureParams),
    /// A notification to show (`fenix/notify`).
    Notify(NotifyParams),
    AgendaTasks(TasksParams),
    AgendaClockIn(ClockInParams),
    AgendaClockOut,
    /// `{ "path"?: string }`: the language servers' problems in a file,
    /// or in every open one.
    Diagnostics(Value),
    /// The open files: path, unsaved or not, focused or not.
    Buffers,
    /// What's selected in the editor (or the cursor's line).
    Selection,
    /// The project's tasks, by name.
    Tasks,
    /// `{ "name": string }`: runs a project task (agents' write tools).
    TasksRun(Value),
    /// `{ "id": string }`: runs a Fenix command for an agent, when
    /// `agents.mcp.fenix_write` lets it.
    AgentCommand(Value),
    /// `{ "path": string, "startLine"?: number, "endLine"?: number }`: an
    /// open file as it stands in Fenix, unsaved edits and all.
    BufferRead(Value),
    /// `{ "kind": string, "path"?: string, "line"?: number, "column"?:
    /// number, "symbol"?: string, "query"?: string }`: the language
    /// servers' answer.
    Lsp(Value),
    /// Where the debugger is stopped, and what it can see there.
    DebugState,
    /// `{ "expression": string }`: evaluated where the debugger is
    /// stopped, when `agents.mcp.fenix_write` lets it.
    DebugEvaluate(Value),
    /// The tools extensions offer agents.
    Tools,
    /// `{ "name": string, "arguments"?: object }`: runs one of them.
    ToolsCall(Value),
    /// The `fenix/agent.*` methods, which MCP bridges use: `method` is
    /// the part after `fenix/agent.` (`tools`, `call`, `cancel`, `wait`,
    /// `resources`, `read`, `subscribe`, `prompts`, `prompt`), `caller`
    /// who asked. Each tool's class and the user's rules decide what it
    /// may do, whoever calls (`docs/CONTROL.md`).
    Agent { method: String, caller: AgentCaller, params: Value },
}
