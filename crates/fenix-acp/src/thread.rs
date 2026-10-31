//! A conversation as Fenix shows it: the reducer every `session/update`
//! goes through. Pure -- no I/O, no clock -- so every update kind, and
//! every odd order an agent sends them in, is a plain unit test.
//!
//! The agent streams; this keeps the result. Message and thought chunks
//! are joined into one entry per message; tool calls are upserted by id
//! (an update for a call never announced makes the card); the plan,
//! commands, modes, config options, usage and title are the latest the
//! agent sent. Fenix's own entries -- the prompts you sent, notices,
//! permission cards -- sit in the same list, in order.

use crate::protocol::{AvailableCommand, ConfigOption, ContentBlock, Location, Modes, PermissionOption, PlanEntry, SessionUpdate, ToolCallFields, ToolContent, ToolKind, ToolStatus};
use serde_json::Value;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Thread {
    pub entries: Vec<Entry>,
    pub plan: Vec<PlanEntry>,
    pub commands: Vec<AvailableCommand>,
    pub modes: Option<Modes>,
    pub config_options: Vec<ConfigOption>,
    pub usage: Option<Usage>,
    pub title: Option<String>,
    /// Bumped by every change, so a page knows when to draw again.
    pub revision: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Usage {
    pub used: u64,
    pub size: u64,
    pub cost: Option<(f64, String)>,
}

impl Usage {
    /// How full the context is, 0-100.
    pub fn percent(&self) -> u8 {
        self.used.saturating_mul(100).checked_div(self.size).map_or(0, |p| p.min(100) as u8)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Entry {
    /// A prompt: sent, or `queued` until the agent can take it.
    User { blocks: Vec<ContentBlock>, queued: bool },
    /// The agent's answer. Text chunks are joined into `text`; anything
    /// else it sends (a picture) goes in `extra`.
    Agent { text: String, extra: Vec<ContentBlock>, message_id: Option<String> },
    Thought { text: String, message_id: Option<String> },
    Tool(ToolCall),
    Permission(PermissionCard),
    Notice(Notice),
}

#[derive(Clone, Debug, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub title: String,
    pub kind: ToolKind,
    pub status: ToolStatus,
    pub content: Vec<ToolContent>,
    pub locations: Vec<Location>,
    pub raw_input: Option<Value>,
    pub raw_output: Option<Value>,
}

impl ToolCall {
    fn new(id: &str) -> Self {
        Self { id: id.to_string(), title: String::new(), kind: ToolKind::Other, status: ToolStatus::Pending, content: Vec::new(), locations: Vec::new(), raw_input: None, raw_output: None }
    }

    /// The command line it runs, from its raw input as agents give it:
    /// `command` (a string, or a program and its arguments), `cmd`,
    /// `commandLine`, with `args` after a program.
    pub fn command_line(&self) -> Option<String> {
        let input = self.raw_input.as_ref()?;
        let words = |v: &Value| -> Option<String> {
            match v {
                Value::String(s) => Some(s.clone()),
                Value::Array(a) => Some(a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" ")),
                _ => None,
            }
        };
        let mut line = ["command", "cmd", "commandLine", "command_line"].iter().find_map(|k| input.get(*k).and_then(words))?;
        if let Some(args) = input.get("args").and_then(words).filter(|a| !a.is_empty()) {
            line.push(' ');
            line.push_str(&args);
        }
        let line = line.trim().to_string();
        (!line.is_empty()).then_some(line)
    }

    fn merge(&mut self, fields: &ToolCallFields) {
        if let Some(title) = &fields.title {
            self.title = title.clone();
        }
        if let Some(kind) = fields.kind {
            self.kind = kind;
        }
        if let Some(status) = fields.status {
            self.status = status;
        }
        if let Some(content) = &fields.content {
            self.content = content.clone();
        }
        if let Some(locations) = &fields.locations {
            self.locations = locations.clone();
        }
        if fields.raw_input.is_some() {
            self.raw_input = fields.raw_input.clone();
        }
        if fields.raw_output.is_some() {
            self.raw_output = fields.raw_output.clone();
        }
    }

    pub fn fields(&self) -> ToolCallFields {
        ToolCallFields {
            id: self.id.clone(),
            title: Some(self.title.clone()),
            kind: Some(self.kind),
            status: Some(self.status),
            content: Some(self.content.clone()),
            locations: Some(self.locations.clone()),
            raw_input: self.raw_input.clone(),
            raw_output: self.raw_output.clone(),
        }
    }
}

/// A `session/request_permission`, as a card in the thread.
#[derive(Clone, Debug, PartialEq)]
pub struct PermissionCard {
    /// The request's JSON-RPC id, as text: what the answer is matched by.
    pub key: String,
    pub tool_call: ToolCallFields,
    pub options: Vec<PermissionOption>,
    pub state: PermissionState,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PermissionState {
    Pending,
    /// Answered with the option of this name.
    Answered(String),
    /// Cancelled, or the agent went away, before it was answered.
    Withdrawn,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Notice {
    pub kind: NoticeKind,
    pub text: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoticeKind {
    /// How a turn ended, when it didn't end the ordinary way.
    Stopped,
    /// The turn was cut short: cancelled, or the agent went away.
    Interrupted,
    /// Something to know: a warning about unsaved edits, a hand-off.
    Info,
    Warning,
    Error,
    /// The agent wants a login.
    Login,
}

impl Thread {
    /// Applies one update from the agent. `replaying`: it comes from
    /// `session/load`'s replay, which is the only time an agent's echo of
    /// a user message is taken (live, Fenix already shows what it sent).
    pub fn apply(&mut self, update: SessionUpdate, replaying: bool) {
        match update {
            SessionUpdate::UserMessage { content, .. } => {
                if !replaying {
                    return;
                }
                match self.entries.last_mut() {
                    Some(Entry::User { blocks, queued: false }) => push_block(blocks, content),
                    _ => self.entries.push(Entry::User { blocks: vec![content], queued: false }),
                }
            }
            SessionUpdate::AgentMessage { content, message_id } => {
                let joined = match self.entries.last_mut() {
                    Some(Entry::Agent { text, extra, message_id: last }) if same_message(last, &message_id) => {
                        match content.clone() {
                            ContentBlock::Text(t) => text.push_str(&t),
                            other => extra.push(other),
                        }
                        if last.is_none() {
                            *last = message_id.clone();
                        }
                        true
                    }
                    _ => false,
                };
                if !joined {
                    let (text, extra) = match content {
                        ContentBlock::Text(t) => (t, Vec::new()),
                        other => (String::new(), vec![other]),
                    };
                    self.entries.push(Entry::Agent { text, extra, message_id });
                }
            }
            SessionUpdate::AgentThought { content, message_id } => {
                let piece = content.as_text();
                match self.entries.last_mut() {
                    Some(Entry::Thought { text, message_id: last }) if same_message(last, &message_id) => {
                        text.push_str(&piece);
                        if last.is_none() {
                            *last = message_id;
                        }
                    }
                    _ => self.entries.push(Entry::Thought { text: piece, message_id }),
                }
            }
            SessionUpdate::ToolCall(fields) | SessionUpdate::ToolCallUpdate(fields) => self.upsert_tool(&fields),
            SessionUpdate::Plan(entries) => self.plan = entries,
            SessionUpdate::AvailableCommands(commands) => self.commands = commands,
            SessionUpdate::CurrentMode(id) => {
                if let Some(modes) = &mut self.modes {
                    modes.current = id.clone();
                }
                // Agents that report the mode as a config option too keep
                // both in step; one that only sends this still moves the
                // option.
                for option in &mut self.config_options {
                    if option.category.as_deref() == Some("mode") {
                        if let crate::protocol::ConfigValue::Select { current, .. } = &mut option.value {
                            *current = id.clone();
                        }
                    }
                }
            }
            SessionUpdate::ConfigOptions(options) => self.set_config_options(options),
            SessionUpdate::SessionInfo { title, .. } => {
                if let Some(title) = title {
                    self.title = title.filter(|t| !t.trim().is_empty());
                }
            }
            SessionUpdate::Usage { used, size, cost } => self.usage = Some(Usage { used, size, cost }),
            SessionUpdate::Unknown(_) => return,
        }
        self.revision += 1;
    }

    fn upsert_tool(&mut self, fields: &ToolCallFields) {
        if let Some(Entry::Tool(call)) = self.entries.iter_mut().rev().find(|e| matches!(e, Entry::Tool(c) if c.id == fields.id)) {
            call.merge(fields);
            return;
        }
        let mut call = ToolCall::new(&fields.id);
        call.merge(fields);
        if call.title.is_empty() {
            call.title = "Tool".to_string();
        }
        self.entries.push(Entry::Tool(call));
    }

    pub fn tool(&self, id: &str) -> Option<&ToolCall> {
        self.entries.iter().rev().find_map(|e| match e {
            Entry::Tool(c) if c.id == id => Some(c),
            _ => None,
        })
    }

    /// Sets the config options, and the modes from a `mode` option when
    /// the agent sends modes only that way.
    pub fn set_config_options(&mut self, options: Vec<ConfigOption>) {
        if let Some(modes) = &mut self.modes {
            if let Some(crate::protocol::ConfigValue::Select { current, .. }) = options.iter().find(|o| o.category.as_deref() == Some("mode")).map(|o| &o.value) {
                modes.current = current.clone();
            }
        }
        self.config_options = options;
        self.revision += 1;
    }

    pub fn set_modes(&mut self, modes: Option<Modes>) {
        self.modes = modes;
        self.revision += 1;
    }

    /// A prompt Fenix sends (or queues).
    pub fn push_user(&mut self, blocks: Vec<ContentBlock>, queued: bool) {
        self.entries.push(Entry::User { blocks, queued });
        self.revision += 1;
    }

    /// The oldest queued prompt is going out now.
    pub fn mark_sent(&mut self) {
        if let Some(Entry::User { queued, .. }) = self.entries.iter_mut().find(|e| matches!(e, Entry::User { queued: true, .. })) {
            *queued = false;
            self.revision += 1;
        }
    }

    /// Takes the newest queued prompt back out of the thread.
    pub fn unqueue_last(&mut self) -> bool {
        if let Some(i) = self.entries.iter().rposition(|e| matches!(e, Entry::User { queued: true, .. })) {
            self.entries.remove(i);
            self.revision += 1;
            return true;
        }
        false
    }

    pub fn notice(&mut self, kind: NoticeKind, text: impl Into<String>) {
        self.entries.push(Entry::Notice(Notice { kind, text: text.into() }));
        self.revision += 1;
    }

    pub fn push_permission(&mut self, card: PermissionCard) {
        self.entries.push(Entry::Permission(card));
        self.revision += 1;
    }

    pub fn permission_mut(&mut self, key: &str) -> Option<&mut PermissionCard> {
        self.entries.iter_mut().rev().find_map(|e| match e {
            Entry::Permission(card) if card.key == key => Some(card),
            _ => None,
        })
    }

    pub fn set_permission_state(&mut self, key: &str, state: PermissionState) {
        if let Some(card) = self.permission_mut(key) {
            card.state = state;
            self.revision += 1;
        }
    }

    /// Every permission card still waiting, withdrawn (the turn was
    /// cancelled, or the agent went away). Their keys.
    pub fn withdraw_permissions(&mut self) -> Vec<String> {
        let mut keys = Vec::new();
        for entry in &mut self.entries {
            if let Entry::Permission(card) = entry {
                if card.state == PermissionState::Pending {
                    card.state = PermissionState::Withdrawn;
                    keys.push(card.key.clone());
                }
            }
        }
        if !keys.is_empty() {
            self.revision += 1;
        }
        keys
    }

    /// Tool calls still running when a turn was cut short end as failed:
    /// nothing more will come for them.
    pub fn end_running_tools(&mut self) {
        let mut changed = false;
        for entry in &mut self.entries {
            if let Entry::Tool(call) = entry {
                if !call.status.is_done() {
                    call.status = ToolStatus::Failed;
                    changed = true;
                }
            }
        }
        if changed {
            self.revision += 1;
        }
    }

    pub fn pending_permissions(&self) -> usize {
        self.entries.iter().filter(|e| matches!(e, Entry::Permission(c) if c.state == PermissionState::Pending)).count()
    }

    /// The first prompt's text, cut short: a title until the agent sends one.
    pub fn fallback_title(&self) -> Option<String> {
        self.entries.iter().find_map(|e| match e {
            Entry::User { blocks, .. } => {
                let text: String = blocks.iter().filter_map(|b| if let ContentBlock::Text(t) = b { Some(t.as_str()) } else { None }).collect::<Vec<_>>().join(" ");
                let line = text.lines().find(|l| !l.trim().is_empty())?.trim();
                Some(if line.chars().count() > 60 { format!("{}…", line.chars().take(59).collect::<String>()) } else { line.to_string() })
            }
            _ => None,
        })
    }

    pub fn display_title(&self) -> String {
        self.title.clone().or_else(|| self.fallback_title()).unwrap_or_else(|| "New thread".to_string())
    }

    /// What the agent said since the last prompt that went: its answer to
    /// it, as text.
    pub fn last_answer(&self) -> String {
        let from = self.entries.iter().rposition(|e| matches!(e, Entry::User { queued: false, .. })).map_or(0, |i| i + 1);
        let parts: Vec<&str> = self.entries[from..].iter().filter_map(|e| if let Entry::Agent { text, .. } = e { Some(text.trim()) } else { None }).filter(|t| !t.is_empty()).collect();
        parts.join("\n\n")
    }

    /// The conversation as Markdown: what `Y` yanks, and what a hand-off
    /// carries to another agent.
    pub fn transcript(&self) -> String {
        let mut out = String::new();
        for entry in &self.entries {
            match entry {
                Entry::User { blocks, queued: false } => {
                    out.push_str("## You\n\n");
                    out.push_str(&blocks.iter().map(ContentBlock::as_text).collect::<Vec<_>>().join("\n"));
                    out.push_str("\n\n");
                }
                Entry::Agent { text, .. } if !text.is_empty() => {
                    out.push_str("## Agent\n\n");
                    out.push_str(text.trim_end());
                    out.push_str("\n\n");
                }
                Entry::Tool(call) => {
                    let status = match call.status {
                        ToolStatus::Pending => "pending",
                        ToolStatus::InProgress => "running",
                        ToolStatus::Completed => "done",
                        ToolStatus::Failed => "failed",
                    };
                    out.push_str(&format!("- {} ({status})\n\n", call.title));
                }
                _ => {}
            }
        }
        out
    }
}

/// Chunks belong to the same message when the agent gives them the same
/// id, or gives none to the newer one.
fn same_message(last: &Option<String>, new: &Option<String>) -> bool {
    match (last, new) {
        (_, None) => true,
        (None, Some(_)) => true,
        (Some(a), Some(b)) => a == b,
    }
}

fn push_block(blocks: &mut Vec<ContentBlock>, block: ContentBlock) {
    if let (Some(ContentBlock::Text(last)), ContentBlock::Text(more)) = (blocks.last_mut(), &block) {
        last.push_str(more);
        return;
    }
    blocks.push(block);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{ConfigValue, PermissionKind, PlanStatus};
    use serde_json::json;

    fn up(v: Value) -> SessionUpdate {
        SessionUpdate::parse(&v)
    }

    fn feed(thread: &mut Thread, updates: &[Value]) {
        for u in updates {
            thread.apply(up(u.clone()), false);
        }
    }

    #[test]
    fn chunks_of_one_message_join_and_a_new_id_starts_another() {
        let mut t = Thread::default();
        feed(
            &mut t,
            &[
                json!({ "sessionUpdate": "agent_thought_chunk", "content": { "type": "text", "text": "Let me " }, "messageId": "m1" }),
                json!({ "sessionUpdate": "agent_thought_chunk", "content": { "type": "text", "text": "think." }, "messageId": "m1" }),
                json!({ "sessionUpdate": "agent_message_chunk", "content": { "type": "text", "text": "Hel" }, "messageId": "m2" }),
                json!({ "sessionUpdate": "agent_message_chunk", "content": { "type": "text", "text": "lo" } }),
                json!({ "sessionUpdate": "agent_message_chunk", "content": { "type": "text", "text": "Again" }, "messageId": "m3" }),
            ],
        );
        assert_eq!(t.entries.len(), 3);
        assert!(matches!(&t.entries[0], Entry::Thought { text, .. } if text == "Let me think."));
        assert!(matches!(&t.entries[1], Entry::Agent { text, message_id: Some(id), .. } if text == "Hello" && id == "m2"));
        assert!(matches!(&t.entries[2], Entry::Agent { text, .. } if text == "Again"));
    }

    #[test]
    fn a_tool_call_between_chunks_splits_the_answer() {
        let mut t = Thread::default();
        feed(
            &mut t,
            &[
                json!({ "sessionUpdate": "agent_message_chunk", "content": { "type": "text", "text": "Reading." } }),
                json!({ "sessionUpdate": "tool_call", "toolCallId": "t1", "title": "Read a.rs", "kind": "read", "status": "pending" }),
                json!({ "sessionUpdate": "agent_message_chunk", "content": { "type": "text", "text": "Done." } }),
            ],
        );
        assert_eq!(t.entries.len(), 3);
        assert_eq!(t.last_answer(), "Reading.\n\nDone.");
        t.push_user(vec![ContentBlock::text("more")], false);
        assert_eq!(t.last_answer(), "", "nothing yet to the new prompt");
        feed(&mut t, &[json!({ "sessionUpdate": "agent_message_chunk", "content": { "type": "text", "text": "Sure." } })]);
        assert_eq!(t.last_answer(), "Sure.");
    }

    #[test]
    fn tool_calls_are_upserted_and_an_update_for_an_unknown_one_makes_it() {
        let mut t = Thread::default();
        feed(
            &mut t,
            &[
                json!({ "sessionUpdate": "tool_call", "toolCallId": "t1", "title": "Edit", "kind": "edit", "status": "pending", "rawInput": {} }),
                json!({ "sessionUpdate": "tool_call_update", "toolCallId": "t1", "title": "Edit a.rs", "rawInput": { "file_path": "/p/a.rs" }, "content": [{ "type": "diff", "path": "/p/a.rs", "oldText": "a", "newText": "b" }] }),
                json!({ "sessionUpdate": "tool_call_update", "toolCallId": "t1", "status": "completed" }),
                json!({ "sessionUpdate": "tool_call_update", "toolCallId": "ghost", "status": "in_progress" }),
            ],
        );
        let call = t.tool("t1").unwrap();
        assert_eq!((call.title.as_str(), call.kind, call.status), ("Edit a.rs", ToolKind::Edit, ToolStatus::Completed));
        assert_eq!(call.content.len(), 1, "an update without content leaves it");
        assert_eq!(t.tool("ghost").unwrap().title, "Tool");
        assert_eq!(t.entries.len(), 2);
    }

    #[test]
    fn user_chunks_are_taken_only_from_a_replay() {
        let mut t = Thread::default();
        t.apply(up(json!({ "sessionUpdate": "user_message_chunk", "content": { "type": "text", "text": "echo" } })), false);
        assert!(t.entries.is_empty());
        t.apply(up(json!({ "sessionUpdate": "user_message_chunk", "content": { "type": "text", "text": "fix " } })), true);
        t.apply(up(json!({ "sessionUpdate": "user_message_chunk", "content": { "type": "text", "text": "it" } })), true);
        assert_eq!(t.entries, vec![Entry::User { blocks: vec![ContentBlock::text("fix it")], queued: false }]);
    }

    #[test]
    fn plan_commands_mode_options_usage_and_title_are_the_latest() {
        let mut t = Thread::default();
        t.set_modes(Some(Modes { current: "default".into(), available: vec![] }));
        feed(
            &mut t,
            &[
                json!({ "sessionUpdate": "plan", "entries": [{ "content": "a", "priority": "high", "status": "pending" }] }),
                json!({ "sessionUpdate": "plan", "entries": [{ "content": "a", "priority": "high", "status": "completed" }, { "content": "b", "priority": "low", "status": "in_progress" }] }),
                json!({ "sessionUpdate": "available_commands_update", "availableCommands": [{ "name": "init", "description": "Write CLAUDE.md" }, { "name": "review", "description": "Review", "input": { "hint": "a branch" } }] }),
                json!({ "sessionUpdate": "config_option_update", "configOptions": [{ "id": "mode", "name": "Mode", "category": "mode", "type": "select", "currentValue": "plan", "options": [] }] }),
                json!({ "sessionUpdate": "current_mode_update", "currentModeId": "acceptEdits" }),
                json!({ "sessionUpdate": "usage_update", "used": 50, "size": 200, "cost": { "amount": 0.12, "currency": "USD" } }),
                json!({ "sessionUpdate": "session_info_update", "title": "Fix the parser" }),
            ],
        );
        assert_eq!(t.plan.len(), 2);
        assert_eq!(t.plan[0].status, PlanStatus::Completed);
        assert_eq!(t.commands[1].hint.as_deref(), Some("a branch"));
        assert_eq!(t.modes.as_ref().unwrap().current, "acceptEdits");
        assert!(matches!(&t.config_options[0].value, ConfigValue::Select { current, .. } if current == "acceptEdits"));
        assert_eq!(t.usage.as_ref().unwrap().percent(), 25);
        assert_eq!(t.display_title(), "Fix the parser");
        assert!(t.entries.is_empty(), "none of those are entries");
    }

    #[test]
    fn queued_prompts_go_out_in_order_and_the_last_can_be_taken_back() {
        let mut t = Thread::default();
        t.push_user(vec![ContentBlock::text("one")], true);
        t.push_user(vec![ContentBlock::text("two")], true);
        t.push_user(vec![ContentBlock::text("three")], true);
        t.mark_sent();
        assert!(t.unqueue_last());
        assert_eq!(t.entries, vec![Entry::User { blocks: vec![ContentBlock::text("one")], queued: false }, Entry::User { blocks: vec![ContentBlock::text("two")], queued: true }]);
        assert_eq!(t.display_title(), "one");
    }

    #[test]
    fn withdrawing_leaves_answered_cards_alone_and_running_tools_fail() {
        let mut t = Thread::default();
        let card = |key: &str| PermissionCard { key: key.into(), tool_call: ToolCallFields { id: "t".into(), ..Default::default() }, options: vec![PermissionOption { id: "y".into(), name: "Yes".into(), kind: PermissionKind::AllowOnce }], state: PermissionState::Pending };
        t.push_permission(card("1"));
        t.push_permission(card("2"));
        t.set_permission_state("1", PermissionState::Answered("Yes".into()));
        feed(&mut t, &[json!({ "sessionUpdate": "tool_call", "toolCallId": "t", "title": "Run", "status": "in_progress" })]);
        assert_eq!(t.withdraw_permissions(), vec!["2".to_string()]);
        t.end_running_tools();
        assert_eq!(t.tool("t").unwrap().status, ToolStatus::Failed);
        assert_eq!(t.pending_permissions(), 0);
    }

    #[test]
    fn every_change_bumps_the_revision_and_unknown_updates_do_not() {
        let mut t = Thread::default();
        t.apply(up(json!({ "sessionUpdate": "hologram" })), false);
        assert_eq!(t.revision, 0);
        t.apply(up(json!({ "sessionUpdate": "agent_message_chunk", "content": { "type": "text", "text": "x" } })), false);
        assert_eq!(t.revision, 1);
    }
}
