//! Agents and extensions (API): the agents extensions bring
//! (`contributes.agents`, `agent-provide`), what they ask one
//! (`agents.ask`), and what they're told when a turn ends
//! (`agent-turn-ended`).

use super::*;
use fenix_acp::hub::AgentSpec;
use fenix_wasm::host::{AgentTurn, Call, Checked, Env, Notice};

/// An ask that isn't answered after this long is answered with an error,
/// and its turn cancelled.
pub(super) const ASK_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// An extension's question to an agent, waiting for its turn to end.
pub(crate) struct Ask {
    /// The extension that asked, and its service call to answer.
    pub ext: String,
    pub call: u64,
    pub since: Instant,
}

impl App {
    /// The agents extensions bring, as the catalog takes them: each one an
    /// installed extension's manifest names, with the command line it gave
    /// (none yet: listed, not offered).
    pub(super) fn agents_brought(&self) -> Vec<catalog::Brought> {
        let mut brought = Vec::new();
        for ext in &self.wasm.installed {
            for agent in &ext.manifest.contributes.agents {
                let spec = self.agents.brought.get(&(ext.id.clone(), agent.id.clone())).cloned().unwrap_or_else(|| AgentSpec { id: agent.id.clone(), name: agent.name.clone(), program: String::new(), args: Vec::new(), env: Vec::new() });
                let login = agent.login.clone().or_else(|| Some(format!("set up by the {} extension", ext.name)));
                brought.push(catalog::Brought { spec, login });
            }
        }
        brought
    }

    /// `agent-provide`: one of `ext`'s agents, as the command line that
    /// starts it, or taken away.
    pub(crate) fn agents_provided(&mut self, ext: &str, id: &str, program: Option<(Checked, Env)>) {
        let Some(contribution) = self.wasm.installed.iter().find(|e| e.id == ext).and_then(|e| e.manifest.contributes.agents.iter().find(|a| a.id == id)).cloned() else {
            self.note_output(ext, format!("  warn  agent-provide: {id} isn't one of its agents (contributes.agents)"));
            return;
        };
        let key = (ext.to_string(), id.to_string());
        match program {
            Some(((program, args, _), env)) => {
                self.agents.brought.insert(key, AgentSpec { id: contribution.id, name: contribution.name, program, args, env });
            }
            None => {
                self.agents.brought.remove(&key);
            }
        }
        self.agents_configure();
    }

    /// `agents.ask`, from extension `ext` working in `root`: a thread of
    /// its own on the agent asked for (or the default one), the prompt
    /// sent, and the answer given when its turn ends. It may read, not
    /// change: whatever it asks permission for is refused.
    pub(crate) fn agents_ask(&mut self, ext: &str, call: u64, payload: &str, root: Option<PathBuf>) {
        let reply = |app: &mut App, answer: Result<String, String>| app.agents_reply(ext, call, answer);
        if !self.wasm.installed.iter().any(|e| e.id == ext && e.manifest.permissions.agents) {
            return reply(self, Err("agents.ask needs agents = true in its permissions".into()));
        }
        let asked: Value = serde_json::from_str(payload).unwrap_or(Value::Null);
        let Some(prompt) = asked.get("prompt").and_then(Value::as_str).map(str::trim).filter(|p| !p.is_empty()) else {
            return reply(self, Err("agents.ask: {\"prompt\": \"...\"} -- and \"agent\", to name one".into()));
        };
        let entries = self.agents_configure();
        let agent = match asked.get("agent").and_then(Value::as_str) {
            Some(id) => match entries.iter().find(|e| e.spec.id == id) {
                Some(e) if e.found => Some(id.to_string()),
                Some(e) => return reply(self, Err(format!("{} isn't found on this machine", e.spec.name))),
                None => return reply(self, Err(format!("no agent {id}"))),
            },
            None => self.agents_default(&entries),
        };
        let Some(agent) = agent else { return reply(self, Err("no coding agent found: SPC a p".into())) };
        let root = root.unwrap_or_else(|| self.agents_root());
        let thread = self.agents.hub.new_thread(&agent, &root);
        let name = self.wasm.installed.iter().find(|e| e.id == ext).map(|e| e.name.clone()).unwrap_or_else(|| ext.to_string());
        if let Some(t) = self.agents.hub.thread_mut(&thread) {
            t.title = Some(format!("{name}: {}", prompt.lines().next().unwrap_or_default().chars().take(60).collect::<String>()));
        }
        self.agents.asks.insert(thread.clone(), Ask { ext: ext.to_string(), call, since: Instant::now() });
        let blocks = vec![ContentBlock::text(prompt), ContentBlock::text(format!("(Asked by the {name} extension in Fenix. Answer in text; don't change any files.)"))];
        let fx = self.agents.hub.prompt(&thread, blocks);
        self.agents_run(fx);
    }

    /// A turn ended (`Effect::TurnEnded`): an ask on it answered and its
    /// session let go, and the extensions that listen told.
    pub(super) fn agents_turn_done(&mut self, thread: &ThreadId, stop: &str) {
        let (text, title) = self.agents.hub.view(thread).map(|v| (v.thread.last_answer(), v.thread.display_title())).unwrap_or_default();
        let (agent, root) = self.agents.hub.meta(thread).map(|m| (m.agent.clone(), m.root.clone())).unwrap_or_default();
        if let Some(ask) = self.agents.asks.remove(thread) {
            let answer = match stop {
                "error" => Err(self.agents_last_error(thread).unwrap_or_else(|| "the agent couldn't answer".into())),
                _ => Ok(json!({ "text": text, "stopReason": stop, "agent": agent, "thread": thread.0 }).to_string()),
            };
            self.agents_reply(&ask.ext, ask.call, answer);
            let fx = self.agents.hub.close(thread, Instant::now());
            self.agents_run(fx);
        }
        let turn = AgentTurn { agent, thread: thread.0.clone(), title, root: root.to_string_lossy().into_owned(), stop_reason: stop.to_string(), text };
        self.editor_call("agents", Call::AgentTurnEnded(turn));
    }

    /// The last error the thread was told of.
    fn agents_last_error(&self, thread: &ThreadId) -> Option<String> {
        let view = self.agents.hub.view(thread)?;
        view.thread.entries.iter().rev().find_map(|e| match e {
            fenix_acp::thread::Entry::Notice(n) if matches!(n.kind, NoticeKind::Error | NoticeKind::Warning | NoticeKind::Interrupted | NoticeKind::Login) => Some(n.text.clone()),
            _ => None,
        })
    }

    /// An asked thread wants a permission, or a login: refused, since
    /// nobody's there to say yes.
    pub(super) fn agents_ask_attention(&mut self, thread: &ThreadId, why: Attention) -> bool {
        if !self.agents.asks.contains_key(thread) {
            return false;
        }
        match why {
            Attention::Permission => {
                let pending: Vec<(String, Option<String>)> = self
                    .agents
                    .hub
                    .view(thread)
                    .map(|v| {
                        v.thread
                            .entries
                            .iter()
                            .filter_map(|e| match e {
                                fenix_acp::thread::Entry::Permission(card) if card.state == PermissionState::Pending => {
                                    let reject = card.options.iter().find(|o| o.kind == PermissionKind::RejectOnce).or_else(|| card.options.iter().find(|o| o.kind == PermissionKind::RejectAlways));
                                    Some((card.key.clone(), reject.map(|o| o.id.clone())))
                                }
                                _ => None,
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                for (key, option) in pending {
                    let fx = self.agents.hub.answer_permission(thread, &key, option.as_deref());
                    self.agents_run(fx);
                }
            }
            Attention::Login => {
                if let Some(ask) = self.agents.asks.remove(thread) {
                    self.agents_reply(&ask.ext, ask.call, Err("the agent wants you to log in first: SPC a p".into()));
                    let fx = self.agents.hub.close(thread, Instant::now());
                    self.agents_run(fx);
                }
            }
            Attention::TurnEnded => {}
        }
        true
    }

    /// An extension's `agents.ask` answered.
    fn agents_reply(&mut self, ext: &str, call: u64, answer: Result<String, String>) {
        #[cfg(test)]
        self.agents.replies.push((ext.to_string(), call, answer.clone()));
        self.code_deliver(ext, Notice::ServiceReply((call, answer)));
    }

    /// Asks waiting too long: answered with an error, their turns
    /// cancelled.
    pub(super) fn agents_asks_tick(&mut self, now: Instant) {
        let late: Vec<ThreadId> = self.agents.asks.iter().filter(|(_, a)| now.duration_since(a.since) >= ASK_TIMEOUT).map(|(t, _)| t.clone()).collect();
        for thread in late {
            let Some(ask) = self.agents.asks.remove(&thread) else { continue };
            self.agents_reply(&ask.ext, ask.call, Err("the agent took too long to answer".into()));
            let fx = self.agents.hub.close(&thread, now);
            self.agents_run(fx);
        }
    }
}
