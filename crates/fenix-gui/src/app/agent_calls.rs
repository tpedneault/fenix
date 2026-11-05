//! The `fenix/agent.*` methods: what MCP bridges (`fenix --mcp`) ask the
//! editor. Every tool, Fenix's and the extensions', goes through
//! `control_agent` here, which looks up its class and toolset, decides by
//! the user's rules (`crate::agent_tools::decide`), asks on the caller's
//! thread when the rule is `ask`, and only then runs it. The tools
//! themselves are the control methods they always were, or the new ones
//! each toolset adds.
//!
//! - `fenix/agent.tools`: the tools the caller is offered now.
//! - `fenix/agent.call`: runs one (`{call, tool, arguments, progressToken?}`).
//! - `fenix/agent.cancel`: the agent cancelled `{call}`.
//! - `fenix/agent.wait`: news since `{since}` (list changes, resource
//!   updates, progress), answered when there is some or after a while.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::sync::mpsc::Sender;

use fenix_acp::hub::ThreadId;
use fenix_control::{AgentCaller, Call, RpcError};
use serde_json::{json, Value};

use super::*;
use crate::agent_tools::{self, Class, Facts, Rule, Rules};
use crate::ipc::Reply;

/// How long `fenix/agent.wait` holds a request with nothing to say.
const WAIT: Duration = Duration::from_secs(20);

/// The news kept for bridges to catch up on.
const NEWS_KEPT: usize = 256;

/// The calls kept in the log (`SPC h a`, "Fenix's tools").
const LOG_KEPT: usize = 500;

/// A caller's toolsets, as `fenix_toolsets` changed them.
#[derive(Default, Clone)]
pub(super) struct SessionTools {
    pub enabled: BTreeSet<String>,
    pub disabled: BTreeSet<String>,
}

/// A question on a thread, waiting for the user.
struct Question {
    call: String,
    thread: String,
    tool: String,
    run: Box<dyn FnOnce(&mut App)>,
    refuse: Sender<Reply>,
}

/// News for the bridges: `event`, for `to` (a caller key) or everyone.
struct News {
    revision: u64,
    to: Option<String>,
    event: Value,
}

struct Waiter {
    caller: String,
    since: u64,
    reply: Sender<Reply>,
    until: Instant,
}

#[derive(Default)]
pub(crate) struct AgentCalls {
    sessions: HashMap<String, SessionTools>,
    /// "Allow" or "refuse for this thread", by thread and tool.
    thread_rules: HashMap<String, HashMap<String, bool>>,
    /// Questions shown, by card key.
    questions: HashMap<String, Question>,
    next_question: u64,
    revision: u64,
    news: VecDeque<News>,
    waiters: Vec<Waiter>,
    /// Calls under way that a cancellation can reach: call id -> what
    /// cancels them.
    running: HashMap<String, Box<dyn FnOnce(&mut App)>>,
    /// Set while a call the rules allowed runs: the older switch
    /// (`agents.mcp.fenix_write`) doesn't apply to it then.
    pub(super) authorized: bool,
    /// What the log of Fenix's tools shows.
    pub(super) log: VecDeque<String>,
    /// Progress tokens of calls under way, by call id: (caller, token).
    progress: HashMap<String, (String, Value)>,
    /// The project and the extensions' tools at the last look: when they
    /// change, so do the lists bridges were given.
    seen: Option<(Option<PathBuf>, Vec<String>)>,
    /// Agents waiting for a task to end (`run_task`).
    pub(super) task_waits: Vec<super::agent_toolsets::TaskWait>,
}

/// The key a caller's toolsets and news are kept under.
fn caller_key(caller: &AgentCaller) -> String {
    match &caller.thread {
        Some(thread) => format!("thread:{thread}"),
        None => format!("client:{}", caller.client),
    }
}

/// What the card says, in a line: the tool and its arguments, short.
fn question_title(tool: &str, arguments: &Value) -> String {
    let mut parts = Vec::new();
    if let Some(map) = arguments.as_object() {
        for (key, value) in map {
            let text = match value {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            let text: String = if text.chars().count() > 60 { format!("{}…", text.chars().take(60).collect::<String>()) } else { text };
            parts.push(format!("{key}: {text}"));
        }
    }
    if parts.is_empty() {
        format!("Fenix: {tool}")
    } else {
        format!("Fenix: {tool} ({})", parts.join(", "))
    }
}

fn tool_kind(class: Class) -> fenix_acp::protocol::ToolKind {
    use fenix_acp::protocol::ToolKind;
    match class {
        Class::Read => ToolKind::Read,
        Class::Show => ToolKind::Other,
        Class::Edit => ToolKind::Edit,
        Class::Run => ToolKind::Execute,
        Class::External => ToolKind::Fetch,
    }
}

/// A tool as the dispatcher knows it, Fenix's or an extension's.
#[derive(Clone, Debug)]
pub(super) struct KnownTool {
    pub name: String,
    pub toolset: String,
    pub class: Class,
    /// The extension it belongs to; `None` for Fenix's own.
    pub extension: Option<String>,
    pub listed: Value,
}

impl App {
    /// The project's facts the default toolsets follow.
    fn agent_facts(&self) -> Facts {
        let root = self.integration_root();
        let lsp = self.lsp_sessions.keys().any(|k| k.root.starts_with(&root) || root.starts_with(&k.root));
        let git = root.ancestors().any(|dir| dir.join(".git").exists());
        let tasks = self.tasks_for(&root).is_ok_and(|t| !t.is_empty());
        let launches = fenix_project::tools::ProjectTools::read(&root).is_ok_and(|t| !t.launch.is_empty());
        Facts { lsp, git, tasks, debug: self.debug_session.is_some() || launches }
    }

    /// The rules for `caller`: the user's and the project's.
    pub(super) fn agent_rules(&self, caller: &AgentCaller) -> Rules {
        let read = |map: &[(String, String)]| agent_tools::parse_rules(&map.iter().cloned().collect::<BTreeMap<_, _>>());
        let user = agent_tools::migrated(read(self.config.map_value("agents.mcp.allow")), self.config.bool_value("agents.mcp.fenix_write"), self.config.bool_value("agents.mcp.fenix_auto_read"));
        let project = match self.project_settings.as_ref().and_then(|(_, p)| p.get("agents.mcp.allow")) {
            Some(fenix_config::Value::Map(entries)) => read(entries),
            _ => BTreeMap::new(),
        };
        // A bridge Fenix started names its thread; any other caller is a
        // client of yours, with the stricter rules.
        let external_client = caller.thread.as_ref().is_none_or(|t| self.agents.hub.view(&ThreadId(t.clone())).is_none());
        Rules { user, project, external_client }
    }

    /// Every tool there is: Fenix's, then the extensions'.
    pub(super) fn agent_known_tools(&self) -> Vec<KnownTool> {
        let mut tools: Vec<KnownTool> = agent_tools::TOOLS.iter().map(|t| KnownTool { name: t.name.into(), toolset: t.toolset.into(), class: t.class, extension: None, listed: agent_tools::listed(t) }).collect();
        for tool in self.control_tools()["tools"].as_array().into_iter().flatten() {
            let name = tool["name"].as_str().unwrap_or_default().to_string();
            let class = tool.get("class").and_then(Value::as_str).and_then(Class::parse).unwrap_or(if tool["writes"].as_bool() == Some(true) { Class::Run } else { Class::Read });
            let extension = tool["extension"].as_str().unwrap_or_default().to_string();
            let toolset = tool.get("toolset").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| extension.rsplit('.').next().unwrap_or(&extension).replace('-', "_"));
            tools.push(KnownTool { listed: json!({ "name": name, "description": tool["description"], "inputSchema": tool["inputSchema"] }), name, toolset, class, extension: Some(extension) });
        }
        tools
    }

    /// The extensions' toolsets, with whether each is offered by default
    /// and a line about it.
    fn agent_extension_toolsets(&self) -> Vec<(String, bool, String)> {
        let mut sets: BTreeMap<String, (bool, String)> = BTreeMap::new();
        let listed = self.control_tools();
        for tool in listed["tools"].as_array().into_iter().flatten() {
            let extension = tool["extension"].as_str().unwrap_or_default();
            let toolset = tool.get("toolset").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| extension.rsplit('.').next().unwrap_or(extension).replace('-', "_"));
            let name = self.wasm.installed.iter().find(|e| e.id == extension).map(|e| e.manifest.extension.name.clone()).unwrap_or_else(|| extension.to_string());
            let entry = sets.entry(toolset).or_insert((false, format!("The {name} extension's tools.")));
            entry.0 |= tool.get("default").and_then(Value::as_bool).unwrap_or(false);
        }
        sets.into_iter().map(|(name, (default, line))| (name, default, line)).collect()
    }

    /// The toolsets `caller` is shown now.
    fn agent_toolsets_on(&self, caller: &AgentCaller) -> BTreeSet<String> {
        let session = self.agent_calls.sessions.get(&caller_key(caller)).cloned().unwrap_or_default();
        let mut setting: Vec<String> = self.config.list_value("agents.mcp.toolsets").to_vec();
        if let Some(fenix_config::Value::List(project)) = self.project_settings.as_ref().and_then(|(_, p)| p.get("agents.mcp.toolsets")) {
            setting.extend(project.iter().cloned());
        }
        let extensions: Vec<(String, bool)> = self.agent_extension_toolsets().into_iter().map(|(n, d, _)| (n, d)).collect();
        agent_tools::offered(self.agent_facts(), &setting, &session.enabled, &session.disabled, &extensions)
    }

    /// `fenix/agent.*`, answered on `reply` now or later.
    pub(super) fn control_agent(&mut self, method: &str, caller: AgentCaller, params: Value, reply: Sender<Reply>) {
        match method {
            "tools" => {
                let on = self.agent_toolsets_on(&caller);
                let rules = self.agent_rules(&caller);
                let tools: Vec<Value> = self
                    .agent_known_tools()
                    .into_iter()
                    // Listed when its toolset is on and the rules don't
                    // refuse it outright.
                    .filter(|t| on.contains(&t.toolset) && agent_tools::decide(&rules, &t.name, &t.toolset, t.class) != Rule::Deny)
                    .map(|t| t.listed)
                    .collect();
                let _ = reply.send(Ok(json!({ "tools": tools })));
            }
            "call" => self.agent_call(caller, params, reply),
            "cancel" => {
                let call = params.get("call").and_then(Value::as_str).unwrap_or_default().to_string();
                self.agent_cancel(&call);
                let _ = reply.send(Ok(json!({})));
            }
            "wait" => {
                // Revisions start at 1: 0 is a bridge that's new.
                self.agent_calls.revision = self.agent_calls.revision.max(1);
                let since = params.get("since").and_then(Value::as_u64).unwrap_or(0);
                let key = caller_key(&caller);
                if !self.agent_news_since(&key, since, &reply) {
                    self.agent_calls.waiters.push(Waiter { caller: key, since, reply, until: Instant::now() + WAIT });
                }
            }
            "resources" | "read" | "subscribe" | "prompts" | "prompt" => self.agent_context(method, caller, params, reply),
            other => {
                let _ = reply.send(Err(RpcError::new(fenix_control::protocol::code::METHOD_NOT_FOUND, format!("no method fenix/agent.{other}"))));
            }
        }
    }

    /// Answers `reply` with the news since `since`, if there is any.
    fn agent_news_since(&self, caller: &str, since: u64, reply: &Sender<Reply>) -> bool {
        // A bridge that's new (0) or fell behind starts from now.
        let oldest = self.agent_calls.news.front().map(|n| n.revision).unwrap_or(self.agent_calls.revision + 1);
        if since == 0 || since + 1 < oldest {
            let _ = reply.send(Ok(json!({ "revision": self.agent_calls.revision, "events": [] })));
            return true;
        }
        let events: Vec<Value> = self.agent_calls.news.iter().filter(|n| n.revision > since && n.to.as_deref().is_none_or(|to| to == caller)).map(|n| n.event.clone()).collect();
        if events.is_empty() {
            return false;
        }
        let _ = reply.send(Ok(json!({ "revision": self.agent_calls.revision, "events": events })));
        true
    }

    /// Something bridges should hear: `to` one caller (its key), or all.
    pub(super) fn agent_news(&mut self, to: Option<String>, event: Value) {
        let calls = &mut self.agent_calls;
        calls.revision += 1;
        calls.news.push_back(News { revision: calls.revision, to, event });
        while calls.news.len() > NEWS_KEPT {
            calls.news.pop_front();
        }
        let waiters = std::mem::take(&mut self.agent_calls.waiters);
        for waiter in waiters {
            if !self.agent_news_since(&waiter.caller, waiter.since, &waiter.reply) {
                self.agent_calls.waiters.push(waiter);
            }
        }
    }

    /// Progress on call `call`, for the agent that made it, when it asked
    /// for progress.
    #[allow(dead_code)] // Extensions' progress comes through here (extension API 1.2).
    pub(super) fn agent_progress(&mut self, call: &str, progress: f64, total: Option<f64>, message: Option<String>) {
        let Some((caller, token)) = self.agent_calls.progress.get(call).cloned() else { return };
        self.agent_news(Some(caller), json!({ "kind": "progress", "token": token, "progress": progress, "total": total, "message": message }));
    }

    /// The time passes: waits with nothing to say are answered, and a
    /// tool list that changed (an extension came, went or was turned off,
    /// another project is in front) is announced.
    pub(super) fn agent_calls_tick(&mut self, now: Instant) -> Option<Instant> {
        if !self.agent_calls.waiters.is_empty() {
            let names: Vec<String> = self.control_tools()["tools"].as_array().into_iter().flatten().filter_map(|t| t["name"].as_str().map(str::to_string)).collect();
            let seen = Some((self.project_root.clone(), names));
            if self.agent_calls.seen.is_some() && self.agent_calls.seen != seen {
                self.agent_tools_changed();
            }
            self.agent_calls.seen = seen;
        }
        self.agent_task_waits_tick(now);
        let revision = self.agent_calls.revision;
        self.agent_calls.waiters.retain(|w| {
            if now >= w.until {
                let _ = w.reply.send(Ok(json!({ "revision": revision.max(w.since), "events": [] })));
                false
            } else {
                true
            }
        });
        self.agent_calls.waiters.iter().map(|w| w.until).chain(self.agent_calls.task_waits.iter().map(|w| w.until)).min()
    }

    /// The tool list may be different now (an extension came or went, a
    /// toolset was turned on): bridges are told.
    pub(super) fn agent_tools_changed(&mut self) {
        self.agent_news(None, json!({ "kind": "tools" }));
    }

    fn agent_log(&mut self, line: String) {
        let log = &mut self.agent_calls.log;
        if log.len() == LOG_KEPT {
            log.pop_front();
        }
        log.push_back(line);
    }

    /// `fenix/agent.call`.
    fn agent_call(&mut self, caller: AgentCaller, params: Value, reply: Sender<Reply>) {
        let call = params.get("call").and_then(Value::as_str).unwrap_or_default().to_string();
        let mut tool = params.get("tool").and_then(Value::as_str).unwrap_or_default().to_string();
        let mut arguments = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
        // The gateway: the tool it names is the one decided on.
        if tool == "fenix_call" {
            tool = arguments.get("tool").and_then(Value::as_str).unwrap_or_default().to_string();
            arguments = arguments.get("arguments").cloned().unwrap_or_else(|| json!({}));
            if tool == "fenix_call" {
                let _ = reply.send(Err(RpcError::invalid_params("fenix_call calls another tool, not itself")));
                return;
            }
        }
        if let Some(token) = params.get("progressToken").filter(|t| !t.is_null()) {
            self.agent_calls.progress.insert(call.clone(), (caller_key(&caller), token.clone()));
        }
        let Some(known) = self.agent_known_tools().into_iter().find(|t| t.name == tool) else {
            let _ = reply.send(Err(RpcError::invalid_params(format!("Fenix has no tool {tool}; fenix_toolsets lists them"))));
            return;
        };
        let rules = self.agent_rules(&caller);
        let rule = agent_tools::decide(&rules, &known.name, &known.toolset, known.class);
        let who = caller.thread.clone().unwrap_or_else(|| caller.client.clone());
        // An answer for this thread, from an earlier card.
        let remembered = caller.thread.as_ref().and_then(|t| self.agent_calls.thread_rules.get(t)).and_then(|r| r.get(&known.name)).copied();
        let rule = match (rule, remembered) {
            (Rule::Deny, _) => Rule::Deny,
            (_, Some(true)) => Rule::Allow,
            (_, Some(false)) => Rule::Deny,
            (rule, None) => rule,
        };
        self.agent_log(format!("{who}: {} ({}, {}): {}", known.name, known.class.as_str(), known.toolset, rule.as_str()));
        match rule {
            Rule::Deny => {
                let why = if remembered == Some(false) { "you refused it for this thread".to_string() } else { format!("refused by your rules (agents.mcp.allow: {} is {})", known.name, rule.as_str()) };
                let _ = reply.send(Err(RpcError::invalid_params(format!("Fenix didn't run {}: {why}", known.name))));
            }
            Rule::Allow => self.agent_run(caller, call, known, arguments, reply),
            Rule::Ask => {
                let Some(thread) = caller.thread.clone() else {
                    let _ = reply.send(Err(RpcError::invalid_params(format!("Fenix didn't run {}: it asks first, and there's no thread to ask on (agents.mcp.allow, client: rules)", known.name))));
                    return;
                };
                self.agent_calls.next_question += 1;
                let key = format!("fenix-q{}", self.agent_calls.next_question);
                let title = question_title(&known.name, &arguments);
                let fx = self.agents.hub.ask_fenix(&ThreadId(thread.clone()), &key, &title, tool_kind(known.class), arguments.clone());
                if fx.is_empty() {
                    let _ = reply.send(Err(RpcError::invalid_params(format!("Fenix didn't run {}: its thread isn't open to ask on", known.name))));
                    return;
                }
                let refuse = reply.clone();
                let name = known.name.clone();
                let asked = call.clone();
                let run = Box::new(move |app: &mut App| app.agent_run(caller, call, known, arguments, reply));
                self.agent_calls.questions.insert(key, Question { call: asked, thread, tool: name, run, refuse });
                self.agents_run(fx);
            }
        }
    }

    /// The user answered a card `agent_call` put up.
    pub(super) fn agent_question_answered(&mut self, key: &str, option: Option<&str>) {
        let Some(question) = self.agent_calls.questions.remove(key) else { return };
        let remember = |app: &mut App, allow: bool| {
            app.agent_calls.thread_rules.entry(question.thread.clone()).or_default().insert(question.tool.clone(), allow);
        };
        match option {
            Some("allow_once") => (question.run)(self),
            Some("allow_thread") => {
                remember(self, true);
                (question.run)(self);
            }
            Some("reject_thread") => {
                remember(self, false);
                let _ = question.refuse.send(Err(RpcError::invalid_params(format!("the user refused {} for this thread", question.tool))));
            }
            Some(_) => {
                let _ = question.refuse.send(Err(RpcError::invalid_params(format!("the user refused {}", question.tool))));
            }
            None => {
                let _ = question.refuse.send(Err(RpcError::invalid_params(format!("{} wasn't run: the question was withdrawn (the turn ended or was cancelled)", question.tool))));
            }
        }
    }

    /// The agent cancelled call `call`: a question about it is taken
    /// back, and a call under way told to stop.
    fn agent_cancel(&mut self, call: &str) {
        if call.is_empty() {
            return;
        }
        if let Some(key) = self.agent_calls.questions.iter().find(|(_, q)| q.call == call).map(|(k, _)| k.clone()) {
            let question = self.agent_calls.questions.remove(&key).unwrap();
            let fx = self.agents.hub.withdraw_fenix(&ThreadId(question.thread.clone()), &key);
            self.agents_run(fx);
            let _ = question.refuse.send(Err(RpcError::invalid_params("cancelled")));
        }
        if let Some(stop) = self.agent_calls.running.remove(call) {
            stop(self);
        }
        self.agent_calls.progress.remove(call);
    }

    /// Registers what stops call `call` if the agent cancels it.
    #[allow(dead_code)]
    pub(super) fn agent_on_cancel(&mut self, call: &str, stop: Box<dyn FnOnce(&mut App)>) {
        if !call.is_empty() {
            self.agent_calls.running.insert(call.to_string(), stop);
        }
    }

    /// Runs a tool the rules let through, answering `reply` with
    /// `{answer}` (and `toolsChanged` when the list is different now).
    fn agent_run(&mut self, caller: AgentCaller, call: String, known: KnownTool, arguments: Value, reply: Sender<Reply>) {
        // What answers the tool goes to `wrapped`, which hands it on as
        // `{answer}`, with nothing waiting on the UI thread.
        let (wrapped, answered) = std::sync::mpsc::channel::<Reply>();
        std::thread::spawn(move || {
            if let Ok(answer) = answered.recv() {
                let _ = reply.send(answer.map(|a| if a.get("toolsChanged").is_some() { a } else { json!({ "answer": a }) }));
            }
        });
        if known.name == "fenix_toolsets" {
            let answer = self.agent_toolsets(&caller, &arguments);
            let _ = wrapped.send(answer);
            return;
        }
        let call_id = call;
        self.agent_calls.authorized = true;
        let routed = self.agent_route(&caller, &call_id, &known, arguments, wrapped);
        self.agent_calls.authorized = false;
        if let Err((call, reply)) = routed {
            match call {
                Some(call) => {
                    // The tools that are control methods of their own.
                    self.agent_calls.authorized = true;
                    let fallback = match self.control_by_extension(call, reply) {
                        Ok(()) => None,
                        Err((call, reply)) => Some((self.apply_control(call), reply)),
                    };
                    self.agent_calls.authorized = false;
                    if let Some((answer, reply)) = fallback {
                        let _ = reply.send(answer);
                    }
                }
                None => {
                    let _ = reply.send(Err(RpcError::internal(format!("{} isn't answered by this Fenix", known.name))));
                }
            }
        }
    }

    /// `fenix_toolsets`: lists them, or turns one on or off for the caller.
    fn agent_toolsets(&mut self, caller: &AgentCaller, arguments: &Value) -> Reply {
        let extensions = self.agent_extension_toolsets();
        let known: BTreeSet<String> = agent_tools::TOOLSETS.iter().map(|t| t.name.to_string()).chain(extensions.iter().map(|(n, _, _)| n.clone())).collect();
        let key = caller_key(caller);
        let mut changed = None;
        if let Some(name) = arguments.get("enable").and_then(Value::as_str).filter(|n| !n.is_empty()) {
            if !known.contains(name) {
                return Err(RpcError::invalid_params(format!("no toolset {name}; leave enable out to list them")));
            }
            let session = self.agent_calls.sessions.entry(key.clone()).or_default();
            session.disabled.remove(name);
            session.enabled.insert(name.to_string());
            changed = Some(name.to_string());
        }
        if let Some(name) = arguments.get("disable").and_then(Value::as_str).filter(|n| !n.is_empty() && *n != "core") {
            let session = self.agent_calls.sessions.entry(key.clone()).or_default();
            session.enabled.remove(name);
            session.disabled.insert(name.to_string());
            changed.get_or_insert_with(String::new);
        }
        let on = self.agent_toolsets_on(caller);
        let rules = self.agent_rules(caller);
        let turned_on = changed.as_deref().filter(|n| !n.is_empty()).map(|name| {
            let tools: Vec<Value> = self.agent_known_tools().into_iter().filter(|t| t.toolset == name && agent_tools::decide(&rules, &t.name, &t.toolset, t.class) != Rule::Deny).map(|t| t.listed).collect();
            (name.to_string(), tools)
        });
        let lines: Vec<(String, String)> = extensions.iter().map(|(n, _, line)| (n.clone(), line.clone())).collect();
        let answer = agent_tools::toolsets_answer(&on, &lines, turned_on.as_ref().map(|(n, t)| (n.as_str(), t.clone())));
        if changed.is_some() {
            // Only this caller's list changed: its bridge says so itself.
            Ok(json!({ "answer": answer, "toolsChanged": true }))
        } else {
            Ok(answer)
        }
    }

    /// The tool's own work. `Err((Some(call), reply))`: a control method
    /// answers it; `Err((None, reply))`: nothing does.
    #[allow(clippy::result_large_err)]
    fn agent_route(&mut self, _caller: &AgentCaller, _call: &str, known: &KnownTool, arguments: Value, reply: Sender<Reply>) -> Result<(), (Option<Call>, Sender<Reply>)> {
        if known.extension.is_some() {
            return Err((Some(Call::ToolsCall(json!({ "name": known.name, "arguments": arguments }))), reply));
        }
        let lsp = |kind: &str, mut arguments: Value| {
            arguments["kind"] = json!(kind);
            Call::Lsp(arguments)
        };
        let call = match known.name.as_str() {
            "diagnostics" => Call::Diagnostics(arguments),
            "open_files" => Call::Buffers,
            "read_buffer" => Call::BufferRead(arguments),
            "selection" => Call::Selection,
            "show_file" => match serde_json::from_value::<fenix_control::protocol::OpenParams>(json!({ "paths": [{ "path": arguments.get("path"), "line": arguments.get("line") }], "focus": false })) {
                Ok(params) => Call::Open(params),
                Err(e) => {
                    let _ = reply.send(Err(RpcError::invalid_params(e.to_string())));
                    return Ok(());
                }
            },
            kind @ ("definition" | "references" | "hover" | "document_symbols" | "workspace_symbols") => lsp(kind, arguments),
            "tasks" => Call::Tasks,
            "run_command" => Call::AgentCommand(arguments),
            "debug_state" => Call::DebugState,
            "debug_evaluate" => Call::DebugEvaluate(arguments),
            name => {
                return match self.agent_toolset_call(_caller, name, arguments, reply) {
                    Ok(()) => Ok(()),
                    Err(reply) => Err((None, reply)),
                };
            }
        };
        Err((Some(call), reply))
    }

    /// Resources and prompts (`fenix/agent.resources`, `read`,
    /// `subscribe`, `prompts`, `prompt`).
    fn agent_context(&mut self, method: &str, _caller: AgentCaller, _params: Value, reply: Sender<Reply>) {
        let answer = match method {
            "resources" => Ok(json!({ "resources": [], "resourceTemplates": [] })),
            "prompts" => Ok(json!({ "prompts": [] })),
            "subscribe" => Ok(json!({})),
            _ => Err(RpcError::invalid_params("Fenix has no such resource or prompt")),
        };
        let _ = reply.send(answer);
    }

    /// The log of Fenix's tools, as `SPC h a` shows it.
    pub(super) fn agent_calls_log(&self) -> Vec<String> {
        self.agent_calls.log.iter().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caller(thread: Option<&str>) -> AgentCaller {
        AgentCaller { client: "fenix-mcp".into(), thread: thread.map(str::to_string) }
    }

    /// `fenix/agent.<method>`'s answer, waiting for one that comes later.
    fn ask(app: &mut App, method: &str, who: AgentCaller, params: Value) -> Option<Reply> {
        let (reply, answer) = std::sync::mpsc::channel();
        app.control_agent(method, who, params, reply);
        answer.recv_timeout(Duration::from_secs(5)).ok()
    }

    fn names(listed: &Value) -> Vec<String> {
        listed["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap().to_string()).collect()
    }

    #[test]
    fn a_card_says_what_the_tool_will_do() {
        assert_eq!(question_title("rename", &json!({ "path": "a.rs", "new_name": "parse_line" })), "Fenix: rename (path: a.rs, new_name: parse_line)");
        assert_eq!(question_title("run_task", &json!({})), "Fenix: run_task");
        let long = question_title("x", &json!({ "text": "y".repeat(100) }));
        assert!(long.chars().count() < 90 && long.ends_with("…)"));
    }

    #[test]
    fn callers_are_kept_by_thread_else_by_client() {
        assert_eq!(caller_key(&caller(Some("t1"))), "thread:t1");
        assert_eq!(caller_key(&AgentCaller { client: "claude-desktop".into(), thread: None }), "client:claude-desktop");
    }

    #[test]
    fn the_list_has_the_core_and_leaves_out_what_the_rules_refuse() {
        let mut app = App::with_file(None);
        let listed = ask(&mut app, "tools", caller(None), json!({})).unwrap().unwrap();
        let names = names(&listed);
        for core in ["diagnostics", "read_buffer", "definition", "fenix_toolsets", "fenix_call"] {
            assert!(names.contains(&core.to_string()), "{core}: {names:?}");
        }
        assert!(!names.contains(&"run_command".to_string()), "commands is off by default");
    }

    #[test]
    fn a_read_is_answered_and_a_run_is_refused_for_a_client_fenix_didnt_start() {
        let mut app = App::with_file(None);
        let answer = ask(&mut app, "call", caller(None), json!({ "call": "1", "tool": "diagnostics", "arguments": {} })).unwrap().unwrap();
        assert!(answer["answer"]["diagnostics"].is_array(), "{answer}");
        let refused = ask(&mut app, "call", caller(None), json!({ "call": "2", "tool": "run_task", "arguments": { "name": "test" } })).unwrap().unwrap_err();
        assert!(refused.message.contains("refused by your rules"), "{}", refused.message);
        let gateway = ask(&mut app, "call", caller(None), json!({ "call": "3", "tool": "fenix_call", "arguments": { "tool": "run_task", "arguments": {} } })).unwrap().unwrap_err();
        assert!(gateway.message.contains("refused"), "the gateway is as strict as what it calls: {}", gateway.message);
        let unknown = ask(&mut app, "call", caller(None), json!({ "call": "4", "tool": "nope" })).unwrap().unwrap_err();
        assert!(unknown.message.contains("fenix_toolsets"));
    }

    #[test]
    fn an_ask_is_a_card_on_the_thread_and_allow_for_the_thread_is_remembered() {
        let mut app = App::with_file(None);
        let root = std::env::temp_dir();
        let thread = app.agents.hub.new_thread("claude", &root);
        let who = caller(Some(&thread.0));
        let (reply, answer) = std::sync::mpsc::channel();
        app.control_agent("call", who.clone(), json!({ "call": "7", "tool": "run_task", "arguments": { "name": "nothing-by-this-name" } }), reply);
        assert!(answer.recv_timeout(Duration::from_millis(200)).is_err(), "it waits for the user");
        assert_eq!(app.agents.hub.status().waiting, 1, "a card is up");
        let key = app.agent_calls.questions.keys().next().unwrap().clone();
        let fx = app.agents.hub.answer_permission(&thread, &key, Some("allow_thread"));
        app.agents_run(fx);
        let ran = answer.recv_timeout(Duration::from_secs(5)).unwrap().unwrap_err();
        assert!(ran.message.contains("no task"), "allowed, so it ran (and found no such task): {}", ran.message);
        // Allowed for the thread: no card the second time.
        let again = ask(&mut app, "call", who.clone(), json!({ "call": "8", "tool": "run_task", "arguments": { "name": "nothing-by-this-name" } })).unwrap().unwrap_err();
        assert!(again.message.contains("no task"), "{}", again.message);
        assert_eq!(app.agents.hub.status().waiting, 0);
    }

    #[test]
    fn a_refusal_and_a_cancellation_answer_the_agent() {
        let mut app = App::with_file(None);
        let thread = app.agents.hub.new_thread("claude", &std::env::temp_dir());
        let who = caller(Some(&thread.0));
        let (reply, answer) = std::sync::mpsc::channel();
        app.control_agent("call", who.clone(), json!({ "call": "1", "tool": "run_task", "arguments": { "name": "x" } }), reply);
        let key = app.agent_calls.questions.keys().next().unwrap().clone();
        let fx = app.agents.hub.answer_permission(&thread, &key, Some("reject_thread"));
        app.agents_run(fx);
        assert!(answer.recv_timeout(Duration::from_secs(5)).unwrap().unwrap_err().message.contains("refused"));
        let again = ask(&mut app, "call", who.clone(), json!({ "call": "2", "tool": "run_task", "arguments": { "name": "x" } })).unwrap().unwrap_err();
        assert!(again.message.contains("for this thread"), "{}", again.message);
        // Another tool asks; the agent cancels the call.
        let (reply, answer) = std::sync::mpsc::channel();
        app.control_agent("call", who.clone(), json!({ "call": "3", "tool": "debug_evaluate", "arguments": { "expression": "x" } }), reply);
        assert_eq!(app.agents.hub.status().waiting, 1);
        ask(&mut app, "cancel", who, json!({ "call": "3" })).unwrap().unwrap();
        assert_eq!(answer.recv_timeout(Duration::from_secs(5)).unwrap().unwrap_err().message, "cancelled");
        assert_eq!(app.agents.hub.status().waiting, 0, "its card is taken back");
    }

    #[test]
    fn a_toolset_turned_on_is_in_the_list_and_says_so() {
        let mut app = App::with_file(None);
        let who = caller(None);
        assert!(!names(&ask(&mut app, "tools", who.clone(), json!({})).unwrap().unwrap()).contains(&"run_command".to_string()));
        let answer = ask(&mut app, "call", who.clone(), json!({ "call": "1", "tool": "fenix_toolsets", "arguments": { "enable": "debug" } })).unwrap().unwrap();
        assert_eq!(answer["toolsChanged"], true);
        assert_eq!(answer["answer"]["enabled"], "debug", "{answer}");
        let tools: Vec<&str> = answer["answer"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert!(tools.contains(&"debug_state"), "{tools:?}");
        assert!(!tools.contains(&"debug_evaluate"), "a client Fenix didn't start isn't offered what it may not run");
        assert!(names(&ask(&mut app, "tools", who.clone(), json!({})).unwrap().unwrap()).contains(&"debug_state".to_string()));
        let listing = ask(&mut app, "call", who.clone(), json!({ "call": "2", "tool": "fenix_toolsets", "arguments": {} })).unwrap().unwrap();
        assert!(listing.get("toolsChanged").is_none());
        assert!(listing["answer"]["toolsets"].as_array().unwrap().iter().any(|t| t["name"] == "debug" && t["on"] == true));
        let bad = ask(&mut app, "call", who, json!({ "call": "3", "tool": "fenix_toolsets", "arguments": { "enable": "nope" } })).unwrap().unwrap_err();
        assert!(bad.message.contains("no toolset"));
    }

    #[test]
    fn news_reaches_the_bridges_waiting_for_it() {
        let mut app = App::with_file(None);
        let who = caller(Some("t1"));
        let first = ask(&mut app, "wait", who.clone(), json!({ "since": 0 })).unwrap().unwrap();
        let since = first["revision"].as_u64().unwrap();
        let (reply, answer) = std::sync::mpsc::channel();
        app.control_agent("wait", who.clone(), json!({ "since": since }), reply);
        assert!(answer.try_recv().is_err(), "nothing new: it waits");
        app.agent_news(Some("thread:someone-else".into()), json!({ "kind": "progress", "token": "x" }));
        assert!(answer.try_recv().is_err(), "news for another caller isn't its");
        app.agent_tools_changed();
        let news = answer.recv_timeout(Duration::from_secs(1)).unwrap().unwrap();
        assert_eq!(news["events"], json!([{ "kind": "tools" }]));
        // A wait with nothing to say ends after a while.
        let (reply, answer) = std::sync::mpsc::channel();
        app.control_agent("wait", who, json!({ "since": news["revision"] }), reply);
        app.agent_calls_tick(Instant::now() + WAIT + Duration::from_secs(1));
        assert_eq!(answer.try_recv().unwrap().unwrap()["events"], json!([]));
    }
}
