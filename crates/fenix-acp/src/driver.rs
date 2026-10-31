//! A headless host for the hub: carries out its effects with real
//! processes (or the mock, in this process), files on disk and plain
//! child processes for terminals, and answers permission requests by a
//! policy. What Fenix's GUI does with buffers and pages, this does
//! without them: for `fenix-acp`'s own tests, the `chat` example, and
//! the live conformance suite run against real agents.

use crate::client::{AcpClient, AcpEvent, Launch, RpcError};
use crate::hub::{ConnId, Effect, Hub, ThreadId};
use crate::protocol::{self, code, ClientRequest, ExitStatus, ListedSession, PermissionKind};
use crate::thread::{Entry, PermissionCard, PermissionState};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

enum Inbox {
    Agent(ConnId, u32, AcpEvent),
    Served(ConnId, ThreadId, Value, Result<Value, RpcError>),
}

/// How a permission request is answered: an option's id, or `None` to
/// dismiss it.
pub type Policy = Box<dyn FnMut(&PermissionCard) -> Option<String> + Send>;

/// Allows once, else the first option.
pub fn allow_once() -> Policy {
    Box::new(|card| card.options.iter().find(|o| o.kind == PermissionKind::AllowOnce).or(card.options.first()).map(|o| o.id.clone()))
}

/// Rejects once, else dismisses.
pub fn reject_once() -> Policy {
    Box::new(|card| card.options.iter().find(|o| o.kind == PermissionKind::RejectOnce).map(|o| o.id.clone()))
}

struct Terminal {
    output: Arc<Mutex<Vec<u8>>>,
    limit: Option<u64>,
    exit: Arc<Mutex<Option<ExitStatus>>>,
    child: Arc<Mutex<Option<std::process::Child>>>,
}

pub struct Driver {
    pub hub: Hub,
    clients: HashMap<ConnId, AcpClient>,
    tx: Sender<Inbox>,
    rx: Receiver<Inbox>,
    terminals: HashMap<String, Terminal>,
    next_terminal: u64,
    pub policy: Policy,
    /// Everything `Effect::Announced` named, in order.
    pub announced: Vec<(ThreadId, Vec<String>)>,
    pub listed: Vec<ListedSession>,
    /// Terminal logins asked for: the connection and the method.
    pub logins: Vec<(ConnId, String)>,
    pub stopped: Vec<ConnId>,
}

impl Driver {
    pub fn new(hub: Hub) -> Self {
        let (tx, rx) = mpsc::channel();
        Self { hub, clients: HashMap::new(), tx, rx, terminals: HashMap::new(), next_terminal: 1, policy: allow_once(), announced: Vec::new(), listed: Vec::new(), logins: Vec::new(), stopped: Vec::new() }
    }

    pub fn client(&self, conn: ConnId) -> Option<&AcpClient> {
        self.clients.get(&conn)
    }

    /// Carries out effects, and those that follow from them.
    pub fn apply(&mut self, fx: Vec<Effect>) {
        let mut queue: std::collections::VecDeque<Effect> = fx.into();
        while let Some(effect) = queue.pop_front() {
            queue.extend(self.carry_out(effect));
            queue.extend(self.answer_permissions());
        }
    }

    fn carry_out(&mut self, effect: Effect) -> Vec<Effect> {
        match effect {
            Effect::Spawn { conn, generation, agent, root } => {
                let tx = self.tx.clone();
                let launch = Launch { program: agent.program, args: agent.args, env: agent.env, cwd: root };
                let result = AcpClient::start(&launch, move |event| tx.send(Inbox::Agent(conn, generation, event)).is_ok()).map(|client| {
                    self.clients.insert(conn, client);
                });
                self.hub.on_spawned(conn, generation, result.map_err(|e| e.to_string()), Instant::now())
            }
            Effect::Stop { conn } => {
                self.clients.remove(&conn);
                self.stopped.push(conn);
                Vec::new()
            }
            Effect::Request { conn, id, method, params } => {
                if let Some(c) = self.clients.get(&conn) {
                    let _ = c.request(id, method, params);
                }
                Vec::new()
            }
            Effect::Notify { conn, method, params } => {
                if let Some(c) = self.clients.get(&conn) {
                    let _ = c.notify(method, params);
                }
                Vec::new()
            }
            Effect::Respond { conn, id, result } => {
                if let Some(c) = self.clients.get(&conn) {
                    let _ = c.respond(id, result);
                }
                Vec::new()
            }
            Effect::Serve { conn, id, thread, request } => {
                if let Some(result) = self.serve(conn, &thread, &id, request) {
                    return self.hub.on_served(conn, &thread, id, result);
                }
                Vec::new()
            }
            Effect::Announced { thread, paths, .. } => {
                self.announced.push((thread, paths));
                Vec::new()
            }
            Effect::Listed { sessions, .. } => {
                self.listed.extend(sessions);
                Vec::new()
            }
            Effect::TerminalLogin { conn, method, .. } => {
                self.logins.push((conn, method));
                Vec::new()
            }
            Effect::Withdraw { .. } | Effect::Save(_) | Effect::Forget(_) | Effect::Changed(_) | Effect::ConnectionChanged(_) | Effect::Attention { .. } | Effect::TurnEnded { .. } => Vec::new(),
        }
    }

    fn answer_permissions(&mut self) -> Vec<Effect> {
        let mut fx = Vec::new();
        let ids: Vec<ThreadId> = self.hub.thread_ids().to_vec();
        for id in ids {
            let pending: Vec<PermissionCard> = self
                .hub
                .view(&id)
                .map(|v| v.thread.entries.iter().filter_map(|e| if let Entry::Permission(c) = e { (c.state == PermissionState::Pending).then(|| c.clone()) } else { None }).collect())
                .unwrap_or_default();
            for card in pending {
                let option = (self.policy)(&card);
                fx.extend(self.hub.answer_permission(&id, &card.key, option.as_deref()));
            }
        }
        fx
    }

    /// Does what the agent asked. `None`: the answer comes later (a
    /// terminal's exit).
    fn serve(&mut self, conn: ConnId, thread: &ThreadId, id: &Value, request: ClientRequest) -> Option<Result<Value, RpcError>> {
        let failed = |e: std::io::Error| RpcError::new(code::INTERNAL_ERROR, e.to_string());
        Some(match request {
            ClientRequest::ReadTextFile { path, line, limit, .. } => std::fs::read_to_string(&path).map_err(failed).map(|text| {
                let start = line.map_or(0, |l| l.saturating_sub(1) as usize);
                let lines: Vec<&str> = text.split_inclusive('\n').skip(start).take(limit.map_or(usize::MAX, |l| l as usize)).collect();
                json!({ "content": lines.concat() })
            }),
            ClientRequest::WriteTextFile { path, content, .. } => {
                if let Some(parent) = std::path::Path::new(&path).parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                std::fs::write(&path, content).map(|_| json!(null)).map_err(failed)
            }
            ClientRequest::RequestPermission { .. } => return None,
            ClientRequest::CreateTerminal { command, args, env, cwd, output_byte_limit, .. } => {
                let root = self.hub.meta(thread).map(|m| m.root.clone()).unwrap_or_default();
                let mut cmd = Command::new(&command);
                cmd.args(&args).current_dir(cwd.map(Into::into).unwrap_or(root)).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
                for (k, v) in env {
                    cmd.env(k, v);
                }
                match cmd.spawn() {
                    Err(e) => Err(failed(e)),
                    Ok(mut child) => {
                        let output = Arc::new(Mutex::new(Vec::new()));
                        for pipe in [child.stdout.take().map(|p| Box::new(p) as Box<dyn Read + Send>), child.stderr.take().map(|p| Box::new(p) as Box<dyn Read + Send>)].into_iter().flatten() {
                            let output = output.clone();
                            std::thread::spawn(move || {
                                let mut pipe = pipe;
                                let mut buf = [0u8; 4096];
                                while let Ok(n) = pipe.read(&mut buf) {
                                    if n == 0 {
                                        break;
                                    }
                                    output.lock().unwrap().extend_from_slice(&buf[..n]);
                                }
                            });
                        }
                        let terminal_id = format!("term-{}", self.next_terminal);
                        self.next_terminal += 1;
                        self.terminals.insert(terminal_id.clone(), Terminal { output, limit: output_byte_limit, exit: Arc::new(Mutex::new(None)), child: Arc::new(Mutex::new(Some(child))) });
                        Ok(json!({ "terminalId": terminal_id }))
                    }
                }
            }
            ClientRequest::TerminalOutput { terminal_id, .. } => match self.terminals.get(&terminal_id) {
                None => Err(RpcError::new(code::INVALID_PARAMS, "no such terminal")),
                Some(t) => {
                    poll_exit(t);
                    let (text, truncated) = limited(&t.output.lock().unwrap(), t.limit);
                    Ok(protocol::terminal_output_result(&text, truncated, t.exit.lock().unwrap().as_ref()))
                }
            },
            ClientRequest::WaitForTerminalExit { terminal_id, .. } => match self.terminals.get(&terminal_id) {
                None => Err(RpcError::new(code::INVALID_PARAMS, "no such terminal")),
                Some(t) => {
                    let (tx, exit, child, thread, id) = (self.tx.clone(), t.exit.clone(), t.child.clone(), thread.clone(), id.clone());
                    std::thread::spawn(move || {
                        let status = loop {
                            if let Some(s) = exit.lock().unwrap().clone() {
                                break s;
                            }
                            let waited = child.lock().unwrap().as_mut().and_then(|c| c.try_wait().ok().flatten());
                            if let Some(s) = waited {
                                let s = ExitStatus { code: s.code().map(|c| c as u32), signal: None };
                                *exit.lock().unwrap() = Some(s.clone());
                                break s;
                            }
                            std::thread::sleep(Duration::from_millis(20));
                        };
                        let _ = tx.send(Inbox::Served(conn, thread, id, Ok(status.to_value())));
                    });
                    return None;
                }
            },
            ClientRequest::KillTerminal { terminal_id, .. } => {
                if let Some(t) = self.terminals.get(&terminal_id) {
                    if let Some(c) = t.child.lock().unwrap().as_mut() {
                        let _ = c.kill();
                    }
                }
                Ok(json!(null))
            }
            ClientRequest::ReleaseTerminal { terminal_id, .. } => {
                if let Some(t) = self.terminals.remove(&terminal_id) {
                    if let Some(c) = t.child.lock().unwrap().as_mut() {
                        let _ = c.kill();
                    }
                }
                Ok(json!(null))
            }
        })
    }

    /// Takes one event (waiting up to `timeout`), and the time's passing.
    /// False when nothing came.
    pub fn pump(&mut self, timeout: Duration) -> bool {
        let got = match self.rx.recv_timeout(timeout) {
            Ok(Inbox::Agent(conn, generation, event)) => {
                let fx = self.hub.on_event(conn, generation, event, Instant::now());
                self.apply(fx);
                true
            }
            Ok(Inbox::Served(conn, thread, id, result)) => {
                let fx = self.hub.on_served(conn, &thread, id, result);
                self.apply(fx);
                true
            }
            Err(_) => false,
        };
        let fx = self.hub.tick(Instant::now());
        self.apply(fx);
        got
    }

    /// Takes events until `until` holds, or `timeout` passes.
    pub fn wait(&mut self, timeout: Duration, until: impl Fn(&Hub) -> bool) -> bool {
        let deadline = Instant::now() + timeout;
        while !until(&self.hub) {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            self.pump(left.min(Duration::from_millis(50)));
        }
        true
    }

    /// Stops every agent.
    pub fn shutdown(&mut self) {
        let fx = self.hub.shutdown();
        self.apply(fx);
    }
}

fn poll_exit(t: &Terminal) {
    let mut exit = t.exit.lock().unwrap();
    if exit.is_none() {
        if let Some(s) = t.child.lock().unwrap().as_mut().and_then(|c| c.try_wait().ok().flatten()) {
            *exit = Some(ExitStatus { code: s.code().map(|c| c as u32), signal: None });
        }
    }
}

/// The output, cut from the front to the limit at a character boundary.
pub fn limited(bytes: &[u8], limit: Option<u64>) -> (String, bool) {
    let text = String::from_utf8_lossy(bytes);
    match limit {
        Some(limit) if text.len() as u64 > limit => {
            let mut start = text.len() - limit as usize;
            while !text.is_char_boundary(start) {
                start += 1;
            }
            (text[start..].to_string(), true)
        }
        _ => (text.into_owned(), false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_is_cut_from_the_front_at_a_character() {
        assert_eq!(limited(b"hello", None), ("hello".into(), false));
        assert_eq!(limited(b"hello", Some(3)), ("llo".into(), true));
        assert_eq!(limited("aé".as_bytes(), Some(1)), ("".into(), true));
    }
}
