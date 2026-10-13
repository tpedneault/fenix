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

/// Who is connecting. Only for Fenix's own diagnostics: nothing is
/// allowed or refused on it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClientInfo {
    pub name: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub pid: u32,
}

/// `fenix/hello`'s result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HelloResult {
    pub protocol: u32,
    /// Fenix's version, `1.2.0`.
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
    /// The leader sequence that runs it, as Fenix's own key menu writes
    /// it (`SPC a a`); absent when no key does.
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
    AgendaTasks(TasksParams),
    AgendaClockIn(ClockInParams),
    AgendaClockOut,
}
