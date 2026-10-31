//! ACP v1's messages, as Fenix reads and writes them.
//!
//! Hand-written rather than taken from `agent-client-protocol-schema`
//! (ADR 0002): that crate turns on `serde_json`'s `preserve_order` for
//! the whole workspace and brings `serde_with`, `strum`, `schemars` and
//! `derive_more` along, for types this file covers in a few hundred
//! lines. More importantly, everything an agent sends is read
//! *leniently*, as the spec asks (`x-deserialize-default-on-error`): an
//! update kind, tool kind, content block or option Fenix doesn't know is
//! kept as `Unknown`/`Other`, never a parse error, so a newer agent
//! never breaks an older Fenix. What Fenix sends is built as plain JSON
//! from the spec's shapes.

use serde_json::{json, Map, Value};

/// The protocol version Fenix speaks: v1, ACP's stable one. (v2 is a
/// draft; see the plan's §1.2.)
pub const PROTOCOL_VERSION: u64 = 1;

/// JSON-RPC error codes an agent or Fenix uses.
pub mod code {
    pub const PARSE_ERROR: i64 = -32700;
    pub const INVALID_REQUEST: i64 = -32600;
    pub const METHOD_NOT_FOUND: i64 = -32601;
    pub const INVALID_PARAMS: i64 = -32602;
    pub const INTERNAL_ERROR: i64 = -32603;
    /// The agent wants a login before it does this.
    pub const AUTH_REQUIRED: i64 = -32000;
    /// The agent doesn't know the session (ended, forgotten, never was).
    pub const SESSION_NOT_FOUND: i64 = -32002;
    /// A request the other side cancelled with `$/cancel_request`.
    pub const REQUEST_CANCELLED: i64 = -32800;
}

pub type SessionId = String;

// ---------------------------------------------------------------------
// Reading leniently.

fn str_of(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(str::to_string)
}

fn bool_of(v: &Value, key: &str) -> bool {
    v.get(key).and_then(Value::as_bool).unwrap_or(false)
}

/// A capability that's an object when offered (`"list": {}`) -- present
/// and not null means yes. Some agents send `true`.
fn offered(v: &Value, key: &str) -> bool {
    match v.get(key) {
        None | Some(Value::Null) | Some(Value::Bool(false)) => false,
        Some(_) => true,
    }
}

fn array_of<'a>(v: &'a Value, key: &str) -> &'a [Value] {
    v.get(key).and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[])
}

// ---------------------------------------------------------------------
// initialize

/// What Fenix tells an agent it can do. Built once per connection.
#[derive(Clone, Debug, Default)]
pub struct ClientCapabilities {
    pub read_text_file: bool,
    pub write_text_file: bool,
    pub terminal: bool,
    pub terminal_auth: bool,
    /// Config options of `type: "boolean"` are understood.
    pub boolean_config_options: bool,
}

impl ClientCapabilities {
    /// Everything Fenix implements.
    pub fn fenix() -> Self {
        Self { read_text_file: true, write_text_file: true, terminal: true, terminal_auth: true, boolean_config_options: true }
    }
}

pub fn initialize_params(capabilities: &ClientCapabilities, client_version: &str) -> Value {
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "clientCapabilities": {
            "fs": { "readTextFile": capabilities.read_text_file, "writeTextFile": capabilities.write_text_file },
            "terminal": capabilities.terminal,
            "auth": { "terminal": capabilities.terminal_auth },
            "session": { "configOptions": { "boolean": capabilities.boolean_config_options } },
        },
        "clientInfo": { "name": "fenix", "title": "Fenix", "version": client_version },
    })
}

/// What the agent said about itself in answer to `initialize`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AgentInfo {
    pub protocol_version: u64,
    pub name: Option<String>,
    pub title: Option<String>,
    pub version: Option<String>,
    pub load_session: bool,
    pub prompt_image: bool,
    pub prompt_audio: bool,
    pub prompt_embedded_context: bool,
    pub mcp_http: bool,
    pub mcp_sse: bool,
    pub session_list: bool,
    pub session_resume: bool,
    pub session_close: bool,
    pub session_delete: bool,
    pub additional_directories: bool,
    pub logout: bool,
    pub auth_methods: Vec<AuthMethod>,
}

impl AgentInfo {
    pub fn parse(result: &Value) -> Self {
        let caps = result.get("agentCapabilities").cloned().unwrap_or(Value::Null);
        let prompt = caps.get("promptCapabilities").cloned().unwrap_or(Value::Null);
        let mcp = caps.get("mcpCapabilities").cloned().unwrap_or(Value::Null);
        let session = caps.get("sessionCapabilities").cloned().unwrap_or(Value::Null);
        let auth = caps.get("auth").cloned().unwrap_or(Value::Null);
        let info = result.get("agentInfo").cloned().unwrap_or(Value::Null);
        Self {
            protocol_version: result.get("protocolVersion").and_then(Value::as_u64).unwrap_or(PROTOCOL_VERSION),
            name: str_of(&info, "name"),
            title: str_of(&info, "title"),
            version: str_of(&info, "version"),
            load_session: bool_of(&caps, "loadSession"),
            prompt_image: bool_of(&prompt, "image"),
            prompt_audio: bool_of(&prompt, "audio"),
            prompt_embedded_context: bool_of(&prompt, "embeddedContext"),
            mcp_http: bool_of(&mcp, "http"),
            mcp_sse: bool_of(&mcp, "sse"),
            session_list: offered(&session, "list"),
            session_resume: offered(&session, "resume"),
            session_close: offered(&session, "close"),
            session_delete: offered(&session, "delete"),
            additional_directories: offered(&session, "additionalDirectories"),
            logout: offered(&auth, "logout"),
            auth_methods: array_of(result, "authMethods").iter().filter_map(AuthMethod::parse).collect(),
        }
    }

    /// The name to show for the agent: its title, else its name.
    pub fn display_name(&self) -> Option<&str> {
        self.title.as_deref().or(self.name.as_deref())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct AuthMethod {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub kind: AuthKind,
}

#[derive(Clone, Debug, PartialEq)]
pub enum AuthKind {
    /// The agent logs in itself: Fenix calls `authenticate`.
    Agent,
    /// Fenix runs the agent's own program with these arguments and
    /// environment in a terminal, for the user to log in there; exit 0
    /// is success. `authenticate` isn't called.
    Terminal { args: Vec<String>, env: Vec<(String, String)> },
    /// A kind this Fenix doesn't know: shown, not offered.
    Other(String),
}

impl AuthMethod {
    fn parse(v: &Value) -> Option<Self> {
        let id = str_of(v, "id")?;
        let kind = match v.get("type").and_then(Value::as_str) {
            None | Some("agent") => AuthKind::Agent,
            Some("terminal") => AuthKind::Terminal {
                args: array_of(v, "args").iter().filter_map(|a| a.as_str().map(str::to_string)).collect(),
                env: v
                    .get("env")
                    .and_then(Value::as_object)
                    .map(|m| m.iter().filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string()))).collect())
                    .unwrap_or_default(),
            },
            Some(other) => AuthKind::Other(other.to_string()),
        };
        Some(Self { name: str_of(v, "name").unwrap_or_else(|| id.clone()), description: str_of(v, "description"), id, kind })
    }
}

// ---------------------------------------------------------------------
// Sessions

/// An MCP server handed to a session. Only stdio servers are built by
/// Fenix (its own `fenix --mcp`, and yours); http/sse ones go through
/// only when the agent says it takes them.
#[derive(Clone, Debug, PartialEq)]
pub enum McpServer {
    Stdio { name: String, command: String, args: Vec<String>, env: Vec<(String, String)> },
    Http { name: String, url: String, headers: Vec<(String, String)> },
    Sse { name: String, url: String, headers: Vec<(String, String)> },
}

impl McpServer {
    pub fn to_value(&self) -> Value {
        let pairs = |pairs: &[(String, String)]| pairs.iter().map(|(n, v)| json!({ "name": n, "value": v })).collect::<Vec<_>>();
        match self {
            Self::Stdio { name, command, args, env } => json!({ "name": name, "command": command, "args": args, "env": pairs(env) }),
            Self::Http { name, url, headers } => json!({ "type": "http", "name": name, "url": url, "headers": pairs(headers) }),
            Self::Sse { name, url, headers } => json!({ "type": "sse", "name": name, "url": url, "headers": pairs(headers) }),
        }
    }

    pub fn name(&self) -> &str {
        match self {
            Self::Stdio { name, .. } | Self::Http { name, .. } | Self::Sse { name, .. } => name,
        }
    }
}

/// `session/new`'s parameters; `session/load` and `session/resume` add
/// the session's id.
pub fn session_params(session_id: Option<&str>, cwd: &str, additional_directories: &[String], mcp_servers: &[McpServer]) -> Value {
    let mut params = json!({
        "cwd": cwd,
        "mcpServers": mcp_servers.iter().map(McpServer::to_value).collect::<Vec<_>>(),
    });
    if !additional_directories.is_empty() {
        params["additionalDirectories"] = json!(additional_directories);
    }
    if let Some(id) = session_id {
        params["sessionId"] = json!(id);
    }
    params
}

/// What `session/new`, `/load` and `/resume` answer: the session's id
/// (new only) and its modes and config options.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SessionSetup {
    pub session_id: Option<SessionId>,
    pub modes: Option<Modes>,
    pub config_options: Option<Vec<ConfigOption>>,
}

impl SessionSetup {
    pub fn parse(result: &Value) -> Self {
        Self {
            session_id: str_of(result, "sessionId"),
            modes: result.get("modes").and_then(Modes::parse),
            config_options: result.get("configOptions").and_then(Value::as_array).map(|a| a.iter().filter_map(ConfigOption::parse).collect()),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Modes {
    pub current: String,
    pub available: Vec<Mode>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Mode {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
}

impl Modes {
    fn parse(v: &Value) -> Option<Self> {
        Some(Self {
            current: str_of(v, "currentModeId")?,
            available: array_of(v, "availableModes")
                .iter()
                .filter_map(|m| {
                    let id = str_of(m, "id")?;
                    Some(Mode { name: str_of(m, "name").unwrap_or_else(|| id.clone()), description: str_of(m, "description"), id })
                })
                .collect(),
        })
    }
}

/// A session config option: a model picker, a reasoning level, a mode,
/// or anything else the agent offers.
#[derive(Clone, Debug, PartialEq)]
pub struct ConfigOption {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    /// `mode`, `model`, `model_config`, `thought_level`, or one of the
    /// agent's own (`_x`). UX only: nothing depends on it for meaning.
    pub category: Option<String>,
    pub value: ConfigValue,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ConfigValue {
    Select { current: String, choices: Vec<ConfigChoice> },
    Boolean(bool),
}

#[derive(Clone, Debug, PartialEq)]
pub struct ConfigChoice {
    pub value: String,
    pub name: String,
    pub description: Option<String>,
    /// The heading it's under, for an agent that groups its choices.
    pub group: Option<String>,
}

impl ConfigOption {
    pub fn parse(v: &Value) -> Option<Self> {
        let id = str_of(v, "id")?;
        let value = match v.get("type").and_then(Value::as_str) {
            Some("boolean") => ConfigValue::Boolean(v.get("currentValue").and_then(Value::as_bool).unwrap_or(false)),
            Some("select") | None => {
                let mut choices = Vec::new();
                for item in array_of(v, "options") {
                    // Grouped choices have `group` and their own `options`.
                    if let Some(inner) = item.get("options").and_then(Value::as_array) {
                        let group = str_of(item, "name").or_else(|| str_of(item, "group"));
                        choices.extend(inner.iter().filter_map(|c| ConfigChoice::parse(c, group.clone())));
                    } else if let Some(choice) = ConfigChoice::parse(item, None) {
                        choices.push(choice);
                    }
                }
                ConfigValue::Select { current: str_of(v, "currentValue").unwrap_or_default(), choices }
            }
            Some(_) => return None,
        };
        Some(Self { name: str_of(v, "name").unwrap_or_else(|| id.clone()), description: str_of(v, "description"), category: str_of(v, "category"), id, value })
    }

    /// The current value as shown: a select's choice name, or on/off.
    pub fn current_label(&self) -> String {
        match &self.value {
            ConfigValue::Select { current, choices } => choices.iter().find(|c| &c.value == current).map(|c| c.name.clone()).unwrap_or_else(|| current.clone()),
            ConfigValue::Boolean(on) => if *on { "on" } else { "off" }.to_string(),
        }
    }
}

impl ConfigChoice {
    fn parse(v: &Value, group: Option<String>) -> Option<Self> {
        let value = str_of(v, "value")?;
        Some(Self { name: str_of(v, "name").unwrap_or_else(|| value.clone()), description: str_of(v, "description"), value, group })
    }
}

/// `session/set_config_option`'s parameters.
pub fn set_config_params(session_id: &str, config_id: &str, value: &ConfigValue) -> Value {
    match value {
        ConfigValue::Boolean(on) => json!({ "sessionId": session_id, "configId": config_id, "type": "boolean", "value": on }),
        ConfigValue::Select { current, .. } => json!({ "sessionId": session_id, "configId": config_id, "value": current }),
    }
}

/// One row of `session/list`.
#[derive(Clone, Debug, PartialEq)]
pub struct ListedSession {
    pub session_id: SessionId,
    pub cwd: String,
    pub title: Option<String>,
    pub updated_at: Option<String>,
}

/// `session/list`'s answer: the rows, and the cursor for the next page.
pub fn parse_session_list(result: &Value) -> (Vec<ListedSession>, Option<String>) {
    let rows = array_of(result, "sessions")
        .iter()
        .filter_map(|s| Some(ListedSession { session_id: str_of(s, "sessionId")?, cwd: str_of(s, "cwd").unwrap_or_default(), title: str_of(s, "title"), updated_at: str_of(s, "updatedAt") }))
        .collect();
    (rows, str_of(result, "nextCursor"))
}

// ---------------------------------------------------------------------
// Content

#[derive(Clone, Debug, PartialEq)]
pub enum ContentBlock {
    Text(String),
    Image { data: String, mime_type: String, uri: Option<String> },
    Audio { data: String, mime_type: String },
    /// A file or URL the agent may read itself.
    ResourceLink { uri: String, name: String, mime_type: Option<String> },
    /// A file's contents, sent along (`promptCapabilities.embeddedContext`).
    /// `text` is `None` for a binary blob.
    Resource { uri: String, text: Option<String>, mime_type: Option<String> },
    /// A block this Fenix doesn't know.
    Unknown(Value),
}

impl ContentBlock {
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text(text.into())
    }

    pub fn parse(v: &Value) -> Self {
        match v.get("type").and_then(Value::as_str) {
            Some("text") => Self::Text(str_of(v, "text").unwrap_or_default()),
            Some("image") => Self::Image { data: str_of(v, "data").unwrap_or_default(), mime_type: str_of(v, "mimeType").unwrap_or_default(), uri: str_of(v, "uri") },
            Some("audio") => Self::Audio { data: str_of(v, "data").unwrap_or_default(), mime_type: str_of(v, "mimeType").unwrap_or_default() },
            Some("resource_link") => Self::ResourceLink { uri: str_of(v, "uri").unwrap_or_default(), name: str_of(v, "name").unwrap_or_default(), mime_type: str_of(v, "mimeType") },
            Some("resource") => {
                let r = v.get("resource").cloned().unwrap_or(Value::Null);
                Self::Resource { uri: str_of(&r, "uri").unwrap_or_default(), text: str_of(&r, "text"), mime_type: str_of(&r, "mimeType") }
            }
            _ => Self::Unknown(v.clone()),
        }
    }

    pub fn to_value(&self) -> Value {
        match self {
            Self::Text(text) => json!({ "type": "text", "text": text }),
            Self::Image { data, mime_type, uri } => {
                let mut v = json!({ "type": "image", "data": data, "mimeType": mime_type });
                if let Some(uri) = uri {
                    v["uri"] = json!(uri);
                }
                v
            }
            Self::Audio { data, mime_type } => json!({ "type": "audio", "data": data, "mimeType": mime_type }),
            Self::ResourceLink { uri, name, mime_type } => {
                let mut v = json!({ "type": "resource_link", "uri": uri, "name": name });
                if let Some(m) = mime_type {
                    v["mimeType"] = json!(m);
                }
                v
            }
            Self::Resource { uri, text, mime_type } => {
                let mut r = json!({ "uri": uri, "text": text.clone().unwrap_or_default() });
                if let Some(m) = mime_type {
                    r["mimeType"] = json!(m);
                }
                json!({ "type": "resource", "resource": r })
            }
            Self::Unknown(v) => v.clone(),
        }
    }

    /// What it reads as, as plain text: the text, or a short stand-in.
    pub fn as_text(&self) -> String {
        match self {
            Self::Text(t) => t.clone(),
            Self::Image { .. } => "[image]".to_string(),
            Self::Audio { .. } => "[audio]".to_string(),
            Self::ResourceLink { name, uri, .. } => format!("@{}", if name.is_empty() { uri } else { name }),
            Self::Resource { uri, .. } => format!("@{uri}"),
            Self::Unknown(_) => String::new(),
        }
    }
}

// ---------------------------------------------------------------------
// session/update

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolKind {
    Read,
    Edit,
    Delete,
    Move,
    Search,
    Execute,
    Think,
    Fetch,
    SwitchMode,
    Other,
}

impl ToolKind {
    fn parse(s: &str) -> Self {
        match s {
            "read" => Self::Read,
            "edit" => Self::Edit,
            "delete" => Self::Delete,
            "move" => Self::Move,
            "search" => Self::Search,
            "execute" => Self::Execute,
            "think" => Self::Think,
            "fetch" => Self::Fetch,
            "switch_mode" => Self::SwitchMode,
            _ => Self::Other,
        }
    }

    /// Whether a tool of this kind changes files: the ones whose files
    /// Fenix snapshots when they're announced.
    pub fn changes_files(self) -> bool {
        matches!(self, Self::Edit | Self::Delete | Self::Move)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolStatus {
    Pending,
    InProgress,
    Completed,
    Failed,
}

impl ToolStatus {
    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "pending" => Self::Pending,
            "in_progress" => Self::InProgress,
            "completed" => Self::Completed,
            "failed" => Self::Failed,
            _ => return None,
        })
    }

    pub fn is_done(self) -> bool {
        matches!(self, Self::Completed | Self::Failed)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum ToolContent {
    Content(ContentBlock),
    Diff { path: String, old_text: Option<String>, new_text: String },
    Terminal(String),
}

impl ToolContent {
    fn parse(v: &Value) -> Option<Self> {
        Some(match v.get("type").and_then(Value::as_str)? {
            "content" => Self::Content(ContentBlock::parse(v.get("content")?)),
            "diff" => Self::Diff { path: str_of(v, "path")?, old_text: str_of(v, "oldText"), new_text: str_of(v, "newText").unwrap_or_default() },
            "terminal" => Self::Terminal(str_of(v, "terminalId")?),
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Location {
    pub path: String,
    /// From 1, when the agent gives one. (ACP doesn't say; agents use
    /// editor lines.)
    pub line: Option<u32>,
}

/// A tool call as announced (`tool_call`) or updated (`tool_call_update`,
/// and a permission request's `toolCall`): every field but the id may be
/// missing from an update, which then leaves it as it was.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ToolCallFields {
    pub id: String,
    pub title: Option<String>,
    pub kind: Option<ToolKind>,
    pub status: Option<ToolStatus>,
    pub content: Option<Vec<ToolContent>>,
    pub locations: Option<Vec<Location>>,
    pub raw_input: Option<Value>,
    pub raw_output: Option<Value>,
}

impl ToolCallFields {
    pub fn parse(v: &Value) -> Option<Self> {
        Some(Self {
            id: str_of(v, "toolCallId")?,
            title: str_of(v, "title"),
            kind: v.get("kind").and_then(Value::as_str).map(ToolKind::parse),
            status: v.get("status").and_then(Value::as_str).and_then(ToolStatus::parse),
            content: v.get("content").and_then(Value::as_array).map(|a| a.iter().filter_map(ToolContent::parse).collect()),
            locations: v.get("locations").and_then(Value::as_array).map(|a| {
                a.iter().filter_map(|l| Some(Location { path: str_of(l, "path")?, line: l.get("line").and_then(Value::as_u64).map(|n| n as u32) })).collect()
            }),
            raw_input: v.get("rawInput").filter(|v| !v.is_null()).cloned(),
            raw_output: v.get("rawOutput").filter(|v| !v.is_null()).cloned(),
        })
    }

    /// The files this tool call names: its diffs' paths, its locations,
    /// and the common path fields of its raw input (`file_path`, `path`,
    /// `filePath`, `notebook_path`). Absolute or not, as the agent gave
    /// them.
    pub fn named_paths(&self) -> Vec<String> {
        let mut paths = Vec::new();
        let mut add = |p: &str| {
            if !p.is_empty() && !paths.iter().any(|q: &String| q == p) {
                paths.push(p.to_string());
            }
        };
        for c in self.content.iter().flatten() {
            if let ToolContent::Diff { path, .. } = c {
                add(path);
            }
        }
        for l in self.locations.iter().flatten() {
            add(&l.path);
        }
        if let Some(input) = &self.raw_input {
            for key in ["file_path", "path", "filePath", "notebook_path"] {
                if let Some(p) = input.get(key).and_then(Value::as_str) {
                    add(p);
                }
            }
        }
        paths
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlanStatus {
    Pending,
    InProgress,
    Completed,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlanEntry {
    pub content: String,
    /// `high`, `medium` or `low`.
    pub priority: String,
    pub status: PlanStatus,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AvailableCommand {
    pub name: String,
    pub description: String,
    /// What to type after it, when it takes input.
    pub hint: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SessionUpdate {
    UserMessage { content: ContentBlock, message_id: Option<String> },
    AgentMessage { content: ContentBlock, message_id: Option<String> },
    AgentThought { content: ContentBlock, message_id: Option<String> },
    ToolCall(ToolCallFields),
    ToolCallUpdate(ToolCallFields),
    Plan(Vec<PlanEntry>),
    AvailableCommands(Vec<AvailableCommand>),
    CurrentMode(String),
    ConfigOptions(Vec<ConfigOption>),
    /// `title`: `Some(None)` clears it, `None` leaves it.
    SessionInfo { title: Option<Option<String>>, updated_at: Option<String> },
    Usage { used: u64, size: u64, cost: Option<(f64, String)> },
    /// A kind this Fenix doesn't know (its name), or a malformed one.
    Unknown(String),
}

impl SessionUpdate {
    /// `session/update`'s params: the session and the update.
    pub fn parse_params(params: &Value) -> Option<(SessionId, Self)> {
        let session = str_of(params, "sessionId")?;
        let update = params.get("update")?;
        Some((session, Self::parse(update)))
    }

    pub fn parse(u: &Value) -> Self {
        let kind = u.get("sessionUpdate").and_then(Value::as_str).unwrap_or("");
        let chunk = || (u.get("content").map(ContentBlock::parse).unwrap_or(ContentBlock::Text(String::new())), str_of(u, "messageId"));
        match kind {
            "user_message_chunk" => {
                let (content, message_id) = chunk();
                Self::UserMessage { content, message_id }
            }
            "agent_message_chunk" => {
                let (content, message_id) = chunk();
                Self::AgentMessage { content, message_id }
            }
            "agent_thought_chunk" => {
                let (content, message_id) = chunk();
                Self::AgentThought { content, message_id }
            }
            "tool_call" => ToolCallFields::parse(u).map(Self::ToolCall).unwrap_or_else(|| Self::Unknown(kind.into())),
            "tool_call_update" => ToolCallFields::parse(u).map(Self::ToolCallUpdate).unwrap_or_else(|| Self::Unknown(kind.into())),
            "plan" => Self::Plan(
                array_of(u, "entries")
                    .iter()
                    .map(|e| PlanEntry {
                        content: str_of(e, "content").unwrap_or_default(),
                        priority: str_of(e, "priority").unwrap_or_else(|| "medium".into()),
                        status: match e.get("status").and_then(Value::as_str) {
                            Some("in_progress") => PlanStatus::InProgress,
                            Some("completed") => PlanStatus::Completed,
                            _ => PlanStatus::Pending,
                        },
                    })
                    .collect(),
            ),
            "available_commands_update" => Self::AvailableCommands(
                array_of(u, "availableCommands")
                    .iter()
                    .filter_map(|c| {
                        Some(AvailableCommand {
                            name: str_of(c, "name")?,
                            description: str_of(c, "description").unwrap_or_default(),
                            hint: c.get("input").and_then(|i| str_of(i, "hint")),
                        })
                    })
                    .collect(),
            ),
            "current_mode_update" => str_of(u, "currentModeId").map(Self::CurrentMode).unwrap_or_else(|| Self::Unknown(kind.into())),
            "config_option_update" => Self::ConfigOptions(array_of(u, "configOptions").iter().filter_map(ConfigOption::parse).collect()),
            "session_info_update" => Self::SessionInfo {
                title: match u.get("title") {
                    None => None,
                    Some(Value::Null) => Some(None),
                    Some(t) => Some(t.as_str().map(str::to_string)),
                },
                updated_at: str_of(u, "updatedAt"),
            },
            "usage_update" => Self::Usage {
                used: u.get("used").and_then(Value::as_u64).unwrap_or(0),
                size: u.get("size").and_then(Value::as_u64).unwrap_or(0),
                cost: u.get("cost").and_then(|c| Some((c.get("amount")?.as_f64()?, str_of(c, "currency")?))),
            },
            other => Self::Unknown(other.to_string()),
        }
    }
}

// ---------------------------------------------------------------------
// session/prompt

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StopReason {
    EndTurn,
    MaxTokens,
    MaxTurnRequests,
    Refusal,
    Cancelled,
    Other(String),
}

impl StopReason {
    pub fn parse(result: &Value) -> Self {
        match result.get("stopReason").and_then(Value::as_str).unwrap_or("end_turn") {
            "end_turn" => Self::EndTurn,
            "max_tokens" => Self::MaxTokens,
            "max_turn_requests" => Self::MaxTurnRequests,
            "refusal" => Self::Refusal,
            "cancelled" => Self::Cancelled,
            other => Self::Other(other.to_string()),
        }
    }
}

pub fn prompt_params(session_id: &str, prompt: &[ContentBlock]) -> Value {
    json!({ "sessionId": session_id, "prompt": prompt.iter().map(ContentBlock::to_value).collect::<Vec<_>>() })
}

// ---------------------------------------------------------------------
// What the agent asks of Fenix

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PermissionKind {
    AllowOnce,
    AllowAlways,
    RejectOnce,
    RejectAlways,
    Other,
}

impl PermissionKind {
    pub fn is_allow(self) -> bool {
        matches!(self, Self::AllowOnce | Self::AllowAlways)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PermissionOption {
    pub id: String,
    pub name: String,
    pub kind: PermissionKind,
}

/// A request from the agent to Fenix, read from its method and params.
#[derive(Clone, Debug, PartialEq)]
pub enum ClientRequest {
    ReadTextFile { session_id: SessionId, path: String, line: Option<u32>, limit: Option<u32> },
    WriteTextFile { session_id: SessionId, path: String, content: String },
    RequestPermission { session_id: SessionId, tool_call: ToolCallFields, options: Vec<PermissionOption> },
    CreateTerminal { session_id: SessionId, command: String, args: Vec<String>, env: Vec<(String, String)>, cwd: Option<String>, output_byte_limit: Option<u64> },
    TerminalOutput { session_id: SessionId, terminal_id: String },
    WaitForTerminalExit { session_id: SessionId, terminal_id: String },
    KillTerminal { session_id: SessionId, terminal_id: String },
    ReleaseTerminal { session_id: SessionId, terminal_id: String },
}

impl ClientRequest {
    /// Reads a request; `Err` carries the JSON-RPC error to answer with:
    /// method not found, or invalid params.
    pub fn parse(method: &str, params: &Value) -> Result<Self, (i64, String)> {
        let need = |key: &str| str_of(params, key).ok_or_else(|| (code::INVALID_PARAMS, format!("`{method}` needs `{key}`")));
        let session_id = || need("sessionId");
        let u32_of = |key: &str| params.get(key).and_then(Value::as_u64).map(|n| n.min(u32::MAX as u64) as u32);
        Ok(match method {
            "fs/read_text_file" => Self::ReadTextFile { session_id: session_id()?, path: need("path")?, line: u32_of("line"), limit: u32_of("limit") },
            "fs/write_text_file" => Self::WriteTextFile { session_id: session_id()?, path: need("path")?, content: need("content")? },
            "session/request_permission" => Self::RequestPermission {
                session_id: session_id()?,
                tool_call: params.get("toolCall").and_then(ToolCallFields::parse).ok_or((code::INVALID_PARAMS, "`session/request_permission` needs `toolCall`".to_string()))?,
                options: array_of(params, "options")
                    .iter()
                    .filter_map(|o| {
                        Some(PermissionOption {
                            id: str_of(o, "optionId")?,
                            name: str_of(o, "name").unwrap_or_default(),
                            kind: match o.get("kind").and_then(Value::as_str) {
                                Some("allow_once") => PermissionKind::AllowOnce,
                                Some("allow_always") => PermissionKind::AllowAlways,
                                Some("reject_once") => PermissionKind::RejectOnce,
                                Some("reject_always") => PermissionKind::RejectAlways,
                                _ => PermissionKind::Other,
                            },
                        })
                    })
                    .collect(),
            },
            "terminal/create" => Self::CreateTerminal {
                session_id: session_id()?,
                command: need("command")?,
                args: array_of(params, "args").iter().filter_map(|a| a.as_str().map(str::to_string)).collect(),
                env: array_of(params, "env").iter().filter_map(|e| Some((str_of(e, "name")?, str_of(e, "value")?))).collect(),
                cwd: str_of(params, "cwd"),
                output_byte_limit: params.get("outputByteLimit").and_then(Value::as_u64),
            },
            "terminal/output" => Self::TerminalOutput { session_id: session_id()?, terminal_id: need("terminalId")? },
            "terminal/wait_for_exit" => Self::WaitForTerminalExit { session_id: session_id()?, terminal_id: need("terminalId")? },
            "terminal/kill" => Self::KillTerminal { session_id: session_id()?, terminal_id: need("terminalId")? },
            "terminal/release" => Self::ReleaseTerminal { session_id: session_id()?, terminal_id: need("terminalId")? },
            _ => return Err((code::METHOD_NOT_FOUND, format!("Fenix doesn't serve `{method}`"))),
        })
    }

    pub fn session_id(&self) -> &str {
        match self {
            Self::ReadTextFile { session_id, .. }
            | Self::WriteTextFile { session_id, .. }
            | Self::RequestPermission { session_id, .. }
            | Self::CreateTerminal { session_id, .. }
            | Self::TerminalOutput { session_id, .. }
            | Self::WaitForTerminalExit { session_id, .. }
            | Self::KillTerminal { session_id, .. }
            | Self::ReleaseTerminal { session_id, .. } => session_id,
        }
    }
}

/// The answer to a permission request: the option picked, or cancelled.
pub fn permission_result(option: Option<&str>) -> Value {
    match option {
        Some(id) => json!({ "outcome": { "outcome": "selected", "optionId": id } }),
        None => json!({ "outcome": { "outcome": "cancelled" } }),
    }
}

/// How a terminal ended, as `terminal/output` and `wait_for_exit` say it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExitStatus {
    pub code: Option<u32>,
    pub signal: Option<String>,
}

impl ExitStatus {
    pub fn to_value(&self) -> Value {
        json!({ "exitCode": self.code, "signal": self.signal })
    }
}

pub fn terminal_output_result(output: &str, truncated: bool, exit: Option<&ExitStatus>) -> Value {
    let mut v = json!({ "output": output, "truncated": truncated });
    if let Some(exit) = exit {
        v["exitStatus"] = exit.to_value();
    }
    v
}

/// Keeps a JSON object's `_meta`-free shape for logs: nothing here reads
/// `_meta`, but it's kept wherever a whole value is (raw input/output).
pub fn without_meta(v: &Value) -> Value {
    match v {
        Value::Object(m) => Value::Object(m.iter().filter(|(k, _)| k.as_str() != "_meta").map(|(k, v)| (k.clone(), without_meta(v))).collect::<Map<_, _>>()),
        Value::Array(a) => Value::Array(a.iter().map(without_meta).collect()),
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_agent_that_says_nothing_offers_nothing() {
        let info = AgentInfo::parse(&json!({ "protocolVersion": 1 }));
        assert!(!info.load_session && !info.session_resume && !info.session_list && info.auth_methods.is_empty());
    }

    #[test]
    fn claude_agent_acps_initialize_reads_as_it_is() {
        let info = AgentInfo::parse(&json!({
            "protocolVersion": 1,
            "agentCapabilities": {
                "_meta": { "claudeCode": { "promptQueueing": true } },
                "promptCapabilities": { "image": true, "embeddedContext": true },
                "mcpCapabilities": { "http": true, "sse": true },
                "auth": { "logout": {} },
                "loadSession": true,
                "sessionCapabilities": { "additionalDirectories": {}, "close": {}, "delete": {}, "fork": {}, "list": {}, "resume": {} }
            },
            "agentInfo": { "name": "@agentclientprotocol/claude-agent-acp", "title": "Claude Agent", "version": "0.99.1" },
            "authMethods": [
                { "id": "claude-ai-login", "name": "Claude Subscription", "type": "terminal", "args": ["--cli", "auth", "login", "--claudeai"] },
                { "id": "x", "name": "Agent's own" },
                { "id": "y", "name": "From the future", "type": "carrier-pigeon" }
            ]
        }));
        assert!(info.load_session && info.session_resume && info.session_list && info.session_close && info.session_delete && info.logout);
        assert!(info.prompt_image && info.prompt_embedded_context && !info.prompt_audio);
        assert_eq!(info.display_name(), Some("Claude Agent"));
        assert_eq!(info.auth_methods[0].kind, AuthKind::Terminal { args: vec!["--cli".into(), "auth".into(), "login".into(), "--claudeai".into()], env: vec![] });
        assert_eq!(info.auth_methods[1].kind, AuthKind::Agent);
        assert_eq!(info.auth_methods[2].kind, AuthKind::Other("carrier-pigeon".into()));
    }

    #[test]
    fn an_unknown_update_is_kept_by_name_not_refused() {
        assert_eq!(SessionUpdate::parse(&json!({ "sessionUpdate": "hologram", "x": 1 })), SessionUpdate::Unknown("hologram".into()));
    }

    #[test]
    fn a_tool_call_names_its_files_from_diffs_locations_and_input() {
        let Some((_, SessionUpdate::ToolCallUpdate(call))) = SessionUpdate::parse_params(&json!({
            "sessionId": "s",
            "update": {
                "sessionUpdate": "tool_call_update", "toolCallId": "t",
                "content": [{ "type": "diff", "path": "/p/a.rs", "oldText": "x", "newText": "y" }, { "type": "content", "content": { "type": "text", "text": "ok" } }],
                "locations": [{ "path": "/p/a.rs", "line": 3 }, { "path": "/p/b.rs" }],
                "rawInput": { "file_path": "/p/c.rs" }
            }
        })) else {
            panic!("not a tool call update")
        };
        assert_eq!(call.named_paths(), vec!["/p/a.rs", "/p/b.rs", "/p/c.rs"]);
        assert_eq!(call.content.as_ref().unwrap().len(), 2);
        assert_eq!(call.kind, None);
    }

    #[test]
    fn config_options_read_select_grouped_and_boolean() {
        let options: Vec<_> = [
            json!({ "id": "model", "name": "Model", "category": "model", "type": "select", "currentValue": "b", "options": [{ "value": "a", "name": "A" }, { "value": "b", "name": "B" }] }),
            json!({ "id": "g", "name": "G", "type": "select", "currentValue": "x", "options": [{ "group": "fast", "name": "Fast", "options": [{ "value": "x", "name": "X" }] }] }),
            json!({ "id": "fast", "name": "Fast", "type": "boolean", "currentValue": true }),
            json!({ "id": "weird", "name": "W", "type": "slider" }),
        ]
        .iter()
        .filter_map(ConfigOption::parse)
        .collect();
        assert_eq!(options.len(), 3);
        assert_eq!(options[0].current_label(), "B");
        let ConfigValue::Select { choices, .. } = &options[1].value else { panic!() };
        assert_eq!(choices[0].group.as_deref(), Some("Fast"));
        assert_eq!(options[2].value, ConfigValue::Boolean(true));
        assert_eq!(set_config_params("s", "fast", &options[2].value), json!({ "sessionId": "s", "configId": "fast", "type": "boolean", "value": true }));
    }

    #[test]
    fn client_requests_read_and_refuse_what_they_must() {
        let read = ClientRequest::parse("fs/read_text_file", &json!({ "sessionId": "s", "path": "/a", "line": 3 })).unwrap();
        assert_eq!(read, ClientRequest::ReadTextFile { session_id: "s".into(), path: "/a".into(), line: Some(3), limit: None });
        assert_eq!(ClientRequest::parse("fs/write_text_file", &json!({ "sessionId": "s", "path": "/a" })).unwrap_err().0, code::INVALID_PARAMS);
        assert_eq!(ClientRequest::parse("fs/delete", &json!({})).unwrap_err().0, code::METHOD_NOT_FOUND);
        let ask = ClientRequest::parse(
            "session/request_permission",
            &json!({ "sessionId": "s", "toolCall": { "toolCallId": "t", "title": "Edit a.rs" }, "options": [{ "optionId": "allow-once", "name": "Yes", "kind": "allow_once" }, { "optionId": "reject", "name": "No", "kind": "reject_once" }] }),
        )
        .unwrap();
        let ClientRequest::RequestPermission { options, .. } = ask else { panic!() };
        assert_eq!(options[0].kind, PermissionKind::AllowOnce);
        assert_eq!(permission_result(Some("allow-once")), json!({ "outcome": { "outcome": "selected", "optionId": "allow-once" } }));
        assert_eq!(permission_result(None), json!({ "outcome": { "outcome": "cancelled" } }));
    }

    #[test]
    fn content_blocks_round_trip() {
        for block in [
            ContentBlock::text("hi"),
            ContentBlock::ResourceLink { uri: "file:///a.rs".into(), name: "a.rs".into(), mime_type: None },
            ContentBlock::Resource { uri: "file:///a.rs".into(), text: Some("fn x() {}".into()), mime_type: Some("text/x-rust".into()) },
            ContentBlock::Image { data: "AAAA".into(), mime_type: "image/png".into(), uri: None },
        ] {
            assert_eq!(ContentBlock::parse(&block.to_value()), block);
        }
    }

    #[test]
    fn session_info_tells_clearing_from_leaving() {
        assert_eq!(SessionUpdate::parse(&json!({ "sessionUpdate": "session_info_update", "title": null })), SessionUpdate::SessionInfo { title: Some(None), updated_at: None });
        assert_eq!(SessionUpdate::parse(&json!({ "sessionUpdate": "session_info_update" })), SessionUpdate::SessionInfo { title: None, updated_at: None });
    }
}
