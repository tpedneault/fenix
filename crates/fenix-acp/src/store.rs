//! Threads kept on disk: `<dir>/index.json`, a line of metadata per
//! thread (enough for the threads page and its picker without reading
//! every thread), and `<dir>/threads/<id>.json`, each thread whole.
//! Written through `fenix-storage`'s staged writes, so a crash never
//! leaves half a file.
//!
//! The format is versioned (`"version": 1`) and written by hand rather
//! than derived: what's kept is the thread as the page shows it, in the
//! same JSON shapes ACP uses (a tool call is a `tool_call`, a content
//! block is a content block), so a file stays readable and an older
//! Fenix skips what it doesn't know.

use crate::hub::{ThreadId, ThreadMeta};
use crate::protocol::{ConfigOption, ContentBlock, Mode, Modes, PermissionKind, PermissionOption, PlanStatus, ToolCallFields, ToolContent, ToolKind, ToolStatus};
use crate::thread::{Entry, Notice, NoticeKind, PermissionCard, PermissionState, Thread, ToolCall, Usage};
use serde_json::{json, Value};
use std::io;
use std::path::{Path, PathBuf};

pub const VERSION: u64 = 1;

pub struct Store {
    dir: PathBuf,
}

/// A thread's line in the index.
#[derive(Clone, Debug, PartialEq)]
pub struct IndexEntry {
    pub id: ThreadId,
    pub agent: String,
    pub root: PathBuf,
    pub session_id: Option<String>,
    pub title: String,
    /// Seconds since the Unix epoch.
    pub updated: u64,
}

/// What's read back of a thread.
pub struct Saved {
    pub meta: ThreadMeta,
    pub thread: Thread,
    /// What the caller kept beside it (the GUI: the change ledger).
    pub extra: Value,
    pub updated: u64,
}

impl Store {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn thread_path(&self, id: &ThreadId) -> PathBuf {
        // Ids are Fenix's own (`<prefix>-<n>`); anything else is made safe.
        let name: String = id.0.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
        self.dir.join("threads").join(format!("{name}.json"))
    }

    fn index_path(&self) -> PathBuf {
        self.dir.join("index.json")
    }

    pub fn save(&self, meta: &ThreadMeta, thread: &Thread, extra: &Value, updated: u64) -> io::Result<()> {
        std::fs::create_dir_all(self.dir.join("threads"))?;
        let body = json!({ "version": VERSION, "meta": meta_value(meta), "updated": updated, "thread": thread_value(thread), "extra": extra });
        fenix_storage::write(&self.thread_path(&meta.id), &serde_json::to_vec(&body)?)?;
        let mut index = self.index();
        let line = IndexEntry { id: meta.id.clone(), agent: meta.agent.clone(), root: meta.root.clone(), session_id: meta.session_id.clone(), title: thread.display_title(), updated };
        match index.iter_mut().find(|e| e.id == meta.id) {
            Some(e) => *e = line,
            None => index.push(line),
        }
        self.write_index(&index)
    }

    pub fn load(&self, id: &ThreadId) -> io::Result<Saved> {
        let body: Value = serde_json::from_slice(&std::fs::read(self.thread_path(id))?)?;
        let invalid = |what: &str| io::Error::new(io::ErrorKind::InvalidData, format!("thread {id}: {what}"));
        if body.get("version").and_then(Value::as_u64).unwrap_or(0) > VERSION {
            return Err(invalid("written by a newer Fenix"));
        }
        let meta = parse_meta(body.get("meta").ok_or_else(|| invalid("no meta"))?).ok_or_else(|| invalid("bad meta"))?;
        Ok(Saved { meta, thread: parse_thread(body.get("thread").unwrap_or(&Value::Null)), extra: body.get("extra").cloned().unwrap_or(Value::Null), updated: body.get("updated").and_then(Value::as_u64).unwrap_or(0) })
    }

    /// Every thread's line, newest first. An index that can't be read is
    /// empty (the threads are still there, and the next save rewrites it).
    pub fn index(&self) -> Vec<IndexEntry> {
        let Ok(bytes) = std::fs::read(self.index_path()) else { return Vec::new() };
        let Ok(body) = serde_json::from_slice::<Value>(&bytes) else { return Vec::new() };
        let mut entries: Vec<IndexEntry> = body
            .get("threads")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|t| {
                        Some(IndexEntry {
                            id: ThreadId(t.get("id")?.as_str()?.to_string()),
                            agent: t.get("agent")?.as_str()?.to_string(),
                            root: PathBuf::from(t.get("root")?.as_str()?),
                            session_id: t.get("sessionId").and_then(Value::as_str).map(str::to_string),
                            title: t.get("title").and_then(Value::as_str).unwrap_or("").to_string(),
                            updated: t.get("updated").and_then(Value::as_u64).unwrap_or(0),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        entries.sort_by_key(|e| std::cmp::Reverse(e.updated));
        entries
    }

    pub fn forget(&self, id: &ThreadId) -> io::Result<()> {
        match std::fs::remove_file(self.thread_path(id)) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        let index: Vec<IndexEntry> = self.index().into_iter().filter(|e| &e.id != id).collect();
        self.write_index(&index)
    }

    fn write_index(&self, index: &[IndexEntry]) -> io::Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        let threads: Vec<Value> = index
            .iter()
            .map(|e| json!({ "id": e.id.0, "agent": e.agent, "root": e.root.to_string_lossy(), "sessionId": e.session_id, "title": e.title, "updated": e.updated }))
            .collect();
        fenix_storage::write(&self.index_path(), &serde_json::to_vec_pretty(&json!({ "version": VERSION, "threads": threads }))?)
    }
}

fn meta_value(meta: &ThreadMeta) -> Value {
    json!({ "id": meta.id.0, "agent": meta.agent, "root": meta.root.to_string_lossy(), "sessionId": meta.session_id, "handedOffFrom": meta.handed_off_from.as_ref().map(|t| &t.0) })
}

fn parse_meta(v: &Value) -> Option<ThreadMeta> {
    Some(ThreadMeta {
        id: ThreadId(v.get("id")?.as_str()?.to_string()),
        agent: v.get("agent")?.as_str()?.to_string(),
        root: PathBuf::from(v.get("root")?.as_str()?),
        session_id: v.get("sessionId").and_then(Value::as_str).map(str::to_string),
        handed_off_from: v.get("handedOffFrom").and_then(Value::as_str).map(|s| ThreadId(s.to_string())),
    })
}

fn thread_value(t: &Thread) -> Value {
    json!({
        "title": t.title,
        "entries": t.entries.iter().map(entry_value).collect::<Vec<_>>(),
        "plan": t.plan.iter().map(|p| json!({ "content": p.content, "priority": p.priority, "status": match p.status { PlanStatus::Pending => "pending", PlanStatus::InProgress => "in_progress", PlanStatus::Completed => "completed" } })).collect::<Vec<_>>(),
        "commands": t.commands.iter().map(|c| json!({ "name": c.name, "description": c.description, "input": c.hint.as_ref().map(|h| json!({ "hint": h })) })).collect::<Vec<_>>(),
        "modes": t.modes.as_ref().map(|m| json!({ "currentModeId": m.current, "availableModes": m.available.iter().map(|x| json!({ "id": x.id, "name": x.name, "description": x.description })).collect::<Vec<_>>() })),
        "configOptions": t.config_options.iter().map(config_value).collect::<Vec<_>>(),
        "usage": t.usage.as_ref().map(|u| json!({ "used": u.used, "size": u.size, "cost": u.cost.as_ref().map(|(a, c)| json!({ "amount": a, "currency": c })) })),
    })
}

fn parse_thread(v: &Value) -> Thread {
    let array = |key: &str| v.get(key).and_then(Value::as_array).cloned().unwrap_or_default();
    let mut t = Thread { title: v.get("title").and_then(Value::as_str).map(str::to_string), ..Thread::default() };
    t.entries = array("entries").iter().filter_map(parse_entry).collect();
    if let crate::protocol::SessionUpdate::Plan(plan) = crate::protocol::SessionUpdate::parse(&json!({ "sessionUpdate": "plan", "entries": array("plan") })) {
        t.plan = plan;
    }
    if let crate::protocol::SessionUpdate::AvailableCommands(commands) = crate::protocol::SessionUpdate::parse(&json!({ "sessionUpdate": "available_commands_update", "availableCommands": array("commands") })) {
        t.commands = commands;
    }
    t.modes = v.get("modes").filter(|m| !m.is_null()).and_then(|m| {
        Some(Modes {
            current: m.get("currentModeId")?.as_str()?.to_string(),
            available: m
                .get("availableModes")?
                .as_array()?
                .iter()
                .filter_map(|x| Some(Mode { id: x.get("id")?.as_str()?.to_string(), name: x.get("name")?.as_str()?.to_string(), description: x.get("description").and_then(Value::as_str).map(str::to_string) }))
                .collect(),
        })
    });
    t.config_options = array("configOptions").iter().filter_map(ConfigOption::parse).collect();
    t.usage = v.get("usage").filter(|u| !u.is_null()).map(|u| Usage {
        used: u.get("used").and_then(Value::as_u64).unwrap_or(0),
        size: u.get("size").and_then(Value::as_u64).unwrap_or(0),
        cost: u.get("cost").and_then(|c| Some((c.get("amount")?.as_f64()?, c.get("currency")?.as_str()?.to_string()))),
    });
    t
}

fn config_value(o: &ConfigOption) -> Value {
    let mut v = json!({ "id": o.id, "name": o.name, "description": o.description, "category": o.category });
    match &o.value {
        crate::protocol::ConfigValue::Boolean(on) => {
            v["type"] = json!("boolean");
            v["currentValue"] = json!(on);
        }
        crate::protocol::ConfigValue::Select { current, choices } => {
            v["type"] = json!("select");
            v["currentValue"] = json!(current);
            v["options"] = choices.iter().map(|c| json!({ "value": c.value, "name": c.name, "description": c.description })).collect();
        }
    }
    v
}

fn tool_kind_name(kind: ToolKind) -> &'static str {
    match kind {
        ToolKind::Read => "read",
        ToolKind::Edit => "edit",
        ToolKind::Delete => "delete",
        ToolKind::Move => "move",
        ToolKind::Search => "search",
        ToolKind::Execute => "execute",
        ToolKind::Think => "think",
        ToolKind::Fetch => "fetch",
        ToolKind::SwitchMode => "switch_mode",
        ToolKind::Other => "other",
    }
}

fn tool_status_name(status: ToolStatus) -> &'static str {
    match status {
        ToolStatus::Pending => "pending",
        ToolStatus::InProgress => "in_progress",
        ToolStatus::Completed => "completed",
        ToolStatus::Failed => "failed",
    }
}

/// A tool call in `tool_call`'s own shape.
pub fn tool_call_value(c: &ToolCallFields) -> Value {
    let mut v = json!({ "toolCallId": c.id });
    if let Some(t) = &c.title {
        v["title"] = json!(t);
    }
    if let Some(k) = c.kind {
        v["kind"] = json!(tool_kind_name(k));
    }
    if let Some(s) = c.status {
        v["status"] = json!(tool_status_name(s));
    }
    if let Some(content) = &c.content {
        v["content"] = content
            .iter()
            .map(|x| match x {
                ToolContent::Content(b) => json!({ "type": "content", "content": b.to_value() }),
                ToolContent::Diff { path, old_text, new_text } => json!({ "type": "diff", "path": path, "oldText": old_text, "newText": new_text }),
                ToolContent::Terminal(id) => json!({ "type": "terminal", "terminalId": id }),
            })
            .collect();
    }
    if let Some(locations) = &c.locations {
        v["locations"] = locations.iter().map(|l| json!({ "path": l.path, "line": l.line })).collect();
    }
    if let Some(input) = &c.raw_input {
        v["rawInput"] = input.clone();
    }
    if let Some(output) = &c.raw_output {
        v["rawOutput"] = output.clone();
    }
    v
}

fn entry_value(e: &Entry) -> Value {
    match e {
        Entry::User { blocks, queued } => json!({ "type": "user", "blocks": blocks.iter().map(ContentBlock::to_value).collect::<Vec<_>>(), "queued": queued }),
        Entry::Agent { text, extra, message_id } => json!({ "type": "agent", "text": text, "extra": extra.iter().map(ContentBlock::to_value).collect::<Vec<_>>(), "messageId": message_id }),
        Entry::Thought { text, message_id } => json!({ "type": "thought", "text": text, "messageId": message_id }),
        Entry::Tool(call) => json!({ "type": "tool", "call": tool_call_value(&call.fields()) }),
        Entry::Permission(card) => json!({
            "type": "permission",
            "key": card.key,
            "toolCall": tool_call_value(&card.tool_call),
            "options": card.options.iter().map(|o| json!({ "optionId": o.id, "name": o.name, "kind": match o.kind { PermissionKind::AllowOnce => "allow_once", PermissionKind::AllowAlways => "allow_always", PermissionKind::RejectOnce => "reject_once", PermissionKind::RejectAlways => "reject_always", PermissionKind::Other => "other" } })).collect::<Vec<_>>(),
            "state": match &card.state { PermissionState::Answered(name) => json!({ "answered": name }), _ => json!("withdrawn") },
        }),
        Entry::Notice(n) => json!({ "type": "notice", "kind": notice_name(n.kind), "text": n.text }),
    }
}

fn notice_name(kind: NoticeKind) -> &'static str {
    match kind {
        NoticeKind::Stopped => "stopped",
        NoticeKind::Interrupted => "interrupted",
        NoticeKind::Info => "info",
        NoticeKind::Warning => "warning",
        NoticeKind::Error => "error",
        NoticeKind::Login => "login",
    }
}

fn parse_entry(v: &Value) -> Option<Entry> {
    let s = |key: &str| v.get(key).and_then(Value::as_str).map(str::to_string);
    let blocks = |key: &str| v.get(key).and_then(Value::as_array).map(|a| a.iter().map(ContentBlock::parse).collect()).unwrap_or_default();
    Some(match v.get("type")?.as_str()? {
        // A prompt still queued when Fenix stopped never went out: it's
        // kept as text the user sees, not sent again on its own.
        "user" => Entry::User { blocks: blocks("blocks"), queued: false },
        "agent" => Entry::Agent { text: s("text").unwrap_or_default(), extra: blocks("extra"), message_id: s("messageId") },
        "thought" => Entry::Thought { text: s("text").unwrap_or_default(), message_id: s("messageId") },
        "tool" => {
            let fields = ToolCallFields::parse(v.get("call")?)?;
            let mut call = ToolCall { id: fields.id.clone(), title: String::new(), kind: ToolKind::Other, status: ToolStatus::Completed, content: vec![], locations: vec![], raw_input: None, raw_output: None };
            call.title = fields.title.unwrap_or_default();
            call.kind = fields.kind.unwrap_or(ToolKind::Other);
            // A call still running when Fenix stopped never finished.
            call.status = match fields.status {
                Some(s) if s.is_done() => s,
                _ => ToolStatus::Failed,
            };
            call.content = fields.content.unwrap_or_default();
            call.locations = fields.locations.unwrap_or_default();
            call.raw_input = fields.raw_input;
            call.raw_output = fields.raw_output;
            Entry::Tool(call)
        }
        "permission" => {
            let request = crate::protocol::ClientRequest::parse("session/request_permission", &json!({ "sessionId": "", "toolCall": v.get("toolCall")?, "options": v.get("options")? })).ok()?;
            let crate::protocol::ClientRequest::RequestPermission { tool_call, options, .. } = request else { return None };
            let options: Vec<PermissionOption> = options;
            Entry::Permission(PermissionCard {
                key: s("key").unwrap_or_default(),
                tool_call,
                options,
                state: match v.get("state").and_then(|s| s.get("answered")).and_then(Value::as_str) {
                    Some(name) => PermissionState::Answered(name.to_string()),
                    None => PermissionState::Withdrawn,
                },
            })
        }
        "notice" => Entry::Notice(Notice {
            kind: match v.get("kind").and_then(Value::as_str) {
                Some("stopped") => NoticeKind::Stopped,
                Some("interrupted") => NoticeKind::Interrupted,
                Some("warning") => NoticeKind::Warning,
                Some("error") => NoticeKind::Error,
                Some("login") => NoticeKind::Login,
                _ => NoticeKind::Info,
            },
            text: s("text").unwrap_or_default(),
        }),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::SessionUpdate;

    fn temp() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("fenix-acp-store-{}-{:?}", std::process::id(), std::thread::current().id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn sample() -> (ThreadMeta, Thread) {
        let meta = ThreadMeta { id: ThreadId("t-1".into()), agent: "claude".into(), root: PathBuf::from("/work/p"), session_id: Some("s1".into()), handed_off_from: None };
        let mut t = Thread::default();
        t.push_user(vec![ContentBlock::text("fix it"), ContentBlock::ResourceLink { uri: "file:///work/p/a.rs".into(), name: "a.rs".into(), mime_type: None }], false);
        t.push_user(vec![ContentBlock::text("and then")], true);
        for u in [
            json!({ "sessionUpdate": "agent_thought_chunk", "content": { "type": "text", "text": "hmm" } }),
            json!({ "sessionUpdate": "agent_message_chunk", "content": { "type": "text", "text": "Done." }, "messageId": "m" }),
            json!({ "sessionUpdate": "tool_call", "toolCallId": "t1", "title": "Edit a.rs", "kind": "edit", "status": "completed", "content": [{ "type": "diff", "path": "/work/p/a.rs", "oldText": "a", "newText": "b" }], "locations": [{ "path": "/work/p/a.rs", "line": 2 }] }),
            json!({ "sessionUpdate": "tool_call", "toolCallId": "t2", "title": "Run", "kind": "execute", "status": "in_progress" }),
            json!({ "sessionUpdate": "plan", "entries": [{ "content": "step", "priority": "high", "status": "in_progress" }] }),
            json!({ "sessionUpdate": "available_commands_update", "availableCommands": [{ "name": "init", "description": "d", "input": { "hint": "h" } }] }),
            json!({ "sessionUpdate": "config_option_update", "configOptions": [{ "id": "model", "name": "Model", "category": "model", "type": "select", "currentValue": "x", "options": [{ "value": "x", "name": "X" }] }, { "id": "fast", "name": "Fast", "type": "boolean", "currentValue": true }] }),
            json!({ "sessionUpdate": "usage_update", "used": 1, "size": 10, "cost": { "amount": 0.5, "currency": "USD" } }),
            json!({ "sessionUpdate": "session_info_update", "title": "Fixing" }),
        ] {
            t.apply(SessionUpdate::parse(&u), false);
        }
        t.set_modes(Some(Modes { current: "plan".into(), available: vec![Mode { id: "plan".into(), name: "Plan".into(), description: None }] }));
        t.push_permission(PermissionCard {
            key: "7".into(),
            tool_call: ToolCallFields { id: "t1".into(), title: Some("Edit a.rs".into()), ..Default::default() },
            options: vec![PermissionOption { id: "allow-once".into(), name: "Yes".into(), kind: PermissionKind::AllowOnce }],
            state: PermissionState::Answered("Yes".into()),
        });
        t.notice(NoticeKind::Interrupted, "Cancelled.");
        (meta, t)
    }

    #[test]
    fn a_thread_reads_back_as_it_was_saved() {
        let store = Store::new(temp());
        let (meta, thread) = sample();
        store.save(&meta, &thread, &json!({ "ledger": [1, 2] }), 100).unwrap();
        let saved = store.load(&meta.id).unwrap();
        assert_eq!(saved.meta, meta);
        assert_eq!(saved.extra, json!({ "ledger": [1, 2] }));
        assert_eq!(saved.updated, 100);
        let mut expected = thread.clone();
        // What was queued and running at save time comes back sent and failed.
        if let Entry::User { queued, .. } = &mut expected.entries[1] {
            *queued = false;
        }
        if let Some(Entry::Tool(c)) = expected.entries.iter_mut().find(|e| matches!(e, Entry::Tool(c) if c.id == "t2")) {
            c.status = ToolStatus::Failed;
        }
        expected.revision = 0;
        let mut got = saved.thread;
        got.revision = 0;
        assert_eq!(got, expected);
        let _ = std::fs::remove_dir_all(store.dir());
    }

    #[test]
    fn the_index_lists_threads_newest_first_and_forgets() {
        let store = Store::new(temp());
        let (mut meta, thread) = sample();
        store.save(&meta, &thread, &Value::Null, 100).unwrap();
        meta.id = ThreadId("t-2".into());
        store.save(&meta, &Thread::default(), &Value::Null, 200).unwrap();
        store.save(&meta, &Thread::default(), &Value::Null, 300).unwrap();
        let index = store.index();
        assert_eq!(index.iter().map(|e| e.id.0.as_str()).collect::<Vec<_>>(), vec!["t-2", "t-1"]);
        assert_eq!(index[1].title, "Fixing");
        assert_eq!(index[0].title, "New thread");
        store.forget(&meta.id).unwrap();
        assert_eq!(store.index().len(), 1);
        assert!(store.load(&meta.id).is_err());
        let _ = std::fs::remove_dir_all(store.dir());
    }

    #[test]
    fn a_thread_from_a_newer_fenix_is_refused_not_misread() {
        let store = Store::new(temp());
        std::fs::create_dir_all(store.dir().join("threads")).unwrap();
        std::fs::write(store.dir().join("threads/t-9.json"), br#"{"version": 99, "meta": {}}"#).unwrap();
        assert!(store.load(&ThreadId("t-9".into())).is_err());
        let _ = std::fs::remove_dir_all(store.dir());
    }
}
