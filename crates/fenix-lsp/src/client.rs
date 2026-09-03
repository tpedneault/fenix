use std::io::{BufReader, Read};
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Mutex;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use serde_json::Value;

use crate::envelope::{RawMessage, ResponseError};

/// One event surfaced from the server -- a response to a request this
/// client previously sent (matched by `id`, whichever value `request`
/// returned), a notification (e.g. `textDocument/publishDiagnostics`),
/// or a server-to-client request (e.g. `workspace/configuration`, which
/// a spec-compliant client must eventually answer via `respond`).
///
/// Deliberately generic (`method` plus raw `params`/`result`) rather
/// than one variant per LSP message shape -- there are dozens, and
/// enumerating them here would duplicate what `lsp_types::request::
/// Request`/`lsp_types::notification::Notification` (and their
/// `Params`/`Result` associated types) already describe. A caller
/// decodes `params`/`result` into the right `lsp_types` struct itself,
/// keyed off `method`.
#[derive(Debug, Clone)]
pub enum LspEvent {
    Response { id: i64, result: Result<Value, ResponseError> },
    Notification { method: String, params: Value },
    ServerRequest { id: Value, method: String, params: Value },
    /// The connection ended -- a read error, the server closing its
    /// stdout, or a malformed message this client couldn't make sense
    /// of (see `read_loop`'s own reasoning for treating that the same
    /// as a disconnect rather than skipping it). No more events follow
    /// this one.
    Disconnected(String),
}

/// A live connection to one language server process. Shaped like
/// `fenix_vnc::VncClient`: owns a dedicated background thread reading
/// the server's stdout as a stream of decoded `LspEvent`s for as long as
/// the connection lives, rather than handing a raw reader out the way
/// `fenix_terminal::Terminal::spawn` does -- responses and unsolicited
/// server notifications need to land in one shared, ordered stream
/// immediately, not be handed to the caller as opaque bytes it would
/// have to frame and decode itself.
///
/// Does *not* perform the `initialize` handshake -- that's an ordinary
/// request/response exchange over this same connection, left to the
/// caller to drive (different servers/languages want different
/// advertised client capabilities and `initializationOptions`).
pub struct LspClient {
    stdin: Mutex<ChildStdin>,
    next_id: AtomicI64,
    child: Mutex<Child>,
    reader_thread: Option<JoinHandle<()>>,
    stderr_thread: Option<JoinHandle<()>>,
}

impl LspClient {
    /// Spawns `command` (with `args`), working directory `cwd` (a
    /// language server resolves relative paths -- and often its whole
    /// notion of "the project" -- against whatever directory it was
    /// started in, on top of whatever `rootUri`/`workspaceFolders` the
    /// `initialize` request itself carries).
    pub fn spawn(command: &str, args: &[String], cwd: &std::path::Path) -> std::io::Result<(LspClient, Receiver<LspEvent>)> {
        let resolved = fenix_rpc::resolve_command(command);
        let mut child = Command::new(&resolved)
            .args(args)
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        let stdin = child.stdin.take().expect("stdin was requested as piped");
        let stdout = child.stdout.take().expect("stdout was requested as piped");
        let stderr = child.stderr.take().expect("stderr was requested as piped");

        let (tx, rx) = mpsc::channel();
        let reader_thread = thread::spawn(move || read_loop(stdout, tx));
        // A server's stderr is drained and discarded on its own thread,
        // never read from the main loop above -- an LSP server that
        // writes enough diagnostic/debug output there (many do) would
        // otherwise fill that pipe's OS buffer and block the *server*
        // itself the moment nothing on this side ever reads it, stalling
        // the whole connection.
        let stderr_thread = thread::spawn(move || drain_and_discard(stderr));

        Ok((
            LspClient { stdin: Mutex::new(stdin), next_id: AtomicI64::new(1), child: Mutex::new(child), reader_thread: Some(reader_thread), stderr_thread: Some(stderr_thread) },
            rx,
        ))
    }

    /// Sends a typed request (e.g. `lsp_types::request::HoverRequest`),
    /// returning the id its response will be tagged with
    /// (`LspEvent::Response { id, .. }`). This client does no
    /// correlation beyond tagging the response with that id -- a caller
    /// tracking "what did I ask id 7 for" does so itself.
    pub fn request<R: lsp_types::request::Request>(&self, params: R::Params) -> std::io::Result<i64> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let params = serde_json::to_value(params).expect("lsp_types request params always serialize");
        self.write(&RawMessage::request(id, R::METHOD, params))?;
        Ok(id)
    }

    /// Sends a typed notification (e.g.
    /// `lsp_types::notification::DidOpenTextDocument`).
    pub fn notify<N: lsp_types::notification::Notification>(&self, params: N::Params) -> std::io::Result<()> {
        let params = serde_json::to_value(params).expect("lsp_types notification params always serialize");
        self.write(&RawMessage::notification(N::METHOD, params))
    }

    /// Answers a server-initiated request (an `LspEvent::ServerRequest`)
    /// -- e.g. `workspace/configuration`, which a spec-compliant server
    /// expects a reply to even when the reply is an empty result.
    pub fn respond(&self, id: Value, result: Result<Value, ResponseError>) -> std::io::Result<()> {
        self.write(&RawMessage::response(id, result))
    }

    fn write(&self, message: &RawMessage) -> std::io::Result<()> {
        let body = serde_json::to_vec(message).expect("RawMessage always serializes");
        let mut stdin = self.stdin.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        fenix_rpc::write_message(&mut *stdin, &body)
    }
}

impl Drop for LspClient {
    /// Best-effort graceful shutdown -- the LSP spec's own teardown
    /// sequence is a `shutdown` request followed by an `exit`
    /// notification, which a well-behaved server responds to by exiting
    /// on its own. Sent without waiting for `shutdown`'s reply (this
    /// client is going away regardless of whether one ever arrives),
    /// then a short bounded wait for the process to actually exit before
    /// force-killing it -- same "ask nicely, then force, but never block
    /// indefinitely" posture as `fenix_terminal::Terminal::kill`'s own
    /// doc comment reasons about, and for the same underlying concern:
    /// this may run on a thread that must not hang.
    fn drop(&mut self) {
        let _ = self.write(&RawMessage::request(0, "shutdown", Value::Null));
        let _ = self.write(&RawMessage::notification("exit", Value::Null));
        if let Ok(mut child) = self.child.lock() {
            let mut exited = false;
            for _ in 0..20 {
                match child.try_wait() {
                    Ok(Some(_)) => {
                        exited = true;
                        break;
                    }
                    Ok(None) => thread::sleep(Duration::from_millis(50)),
                    Err(_) => break,
                }
            }
            if !exited {
                let _ = child.kill();
            }
            let _ = child.wait();
        }
        if let Some(t) = self.reader_thread.take() {
            let _ = t.join();
        }
        if let Some(t) = self.stderr_thread.take() {
            let _ = t.join();
        }
    }
}

/// The connection's entire background lifetime: decodes framed messages
/// off `stdout` one at a time (`fenix_rpc::read_message` blocks until
/// the next one arrives, or the connection ends) and forwards each as a
/// classified `LspEvent`, until either the connection closes or the
/// caller drops its `Receiver`.
fn read_loop(stdout: ChildStdout, tx: Sender<LspEvent>) {
    let mut reader = BufReader::new(stdout);
    loop {
        match fenix_rpc::read_message(&mut reader) {
            Ok(Some(bytes)) => match serde_json::from_slice::<RawMessage>(&bytes) {
                Ok(msg) => {
                    if tx.send(classify(msg)).is_err() {
                        return; // caller dropped the receiver -- nothing left to forward to
                    }
                }
                Err(err) => {
                    // Can't usefully recover mid-stream from a message
                    // that didn't even parse as JSON-RPC -- there's no
                    // way to know what it *meant* to say, so this is
                    // treated the same as a disconnect rather than
                    // silently dropped, which would otherwise leave a
                    // caller with no idea why nothing more ever arrives.
                    let _ = tx.send(LspEvent::Disconnected(format!("malformed message from server: {err}")));
                    return;
                }
            },
            Ok(None) => {
                let _ = tx.send(LspEvent::Disconnected("server closed its stdout".to_string()));
                return;
            }
            Err(err) => {
                let _ = tx.send(LspEvent::Disconnected(err.to_string()));
                return;
            }
        }
    }
}

/// Classifies one decoded `RawMessage` into an `LspEvent`, purely from
/// which of `id`/`method` are present -- exactly how JSON-RPC 2.0
/// itself distinguishes a request, a notification, and a response (see
/// `RawMessage`'s own doc comment).
fn classify(msg: RawMessage) -> LspEvent {
    match (msg.id, msg.method) {
        (Some(id), Some(method)) => LspEvent::ServerRequest { id, method, params: msg.params.unwrap_or(Value::Null) },
        (None, Some(method)) => LspEvent::Notification { method, params: msg.params.unwrap_or(Value::Null) },
        (Some(id), None) => {
            // A response: `result`/`error` are mutually exclusive per
            // spec. The id a compliant server echoes back is always one
            // this client itself generated as a plain integer (see
            // `next_id`), so this is the one spot a numeric id is
            // assumed rather than handled generically -- a server
            // replying with anything else is already not spec-compliant
            // with what was sent.
            let id = id.as_i64().unwrap_or(-1);
            let result = match msg.error {
                Some(err) => Err(err),
                None => Ok(msg.result.unwrap_or(Value::Null)),
            };
            LspEvent::Response { id, result }
        }
        (None, None) => {
            // Neither a request/notification nor a response -- not
            // spec-compliant from any server, but not worth treating as
            // a fatal disconnect either; surfaced as an empty-method
            // notification so a caller can at least notice and log it
            // rather than this thread silently swallowing it.
            LspEvent::Notification { method: String::new(), params: Value::Null }
        }
    }
}

/// Reads and discards a server's stderr for as long as the process
/// lives -- see `spawn`'s own doc comment for why this can't just be
/// left unread.
fn drain_and_discard(mut stderr: ChildStderr) {
    let mut buf = [0u8; 4096];
    loop {
        match stderr.read(&mut buf) {
            Ok(0) | Err(_) => return,
            Ok(_) => continue,
        }
    }
}

// `resolve_command`'s own tests now live in `fenix_rpc` (see its own
// doc comment for why it moved there: the Windows PATHEXT-search bug
// it fixes is shared by `fenix-dap`, not LSP-specific).

#[cfg(test)]
mod tests {
    use super::*;

    fn request_msg(id: i64, method: &str) -> RawMessage {
        RawMessage::request(id, method, Value::Null)
    }

    #[test]
    fn a_message_with_both_id_and_method_is_a_server_request() {
        let event = classify(request_msg(1, "workspace/configuration"));
        assert!(matches!(event, LspEvent::ServerRequest { id, method, .. } if id == Value::from(1) && method == "workspace/configuration"));
    }

    #[test]
    fn a_message_with_method_but_no_id_is_a_notification() {
        let event = classify(RawMessage::notification("textDocument/publishDiagnostics", Value::from("params")));
        assert!(matches!(event, LspEvent::Notification { method, params } if method == "textDocument/publishDiagnostics" && params == Value::from("params")));
    }

    #[test]
    fn a_successful_response_carries_ok_result() {
        let event = classify(RawMessage::response(Value::from(5), Ok(Value::from(42))));
        assert!(matches!(event, LspEvent::Response { id: 5, result: Ok(v) } if v == Value::from(42)));
    }

    #[test]
    fn a_failed_response_carries_err_result() {
        let err = ResponseError { code: -32601, message: "method not found".to_string(), data: None };
        let event = classify(RawMessage::response(Value::from(6), Err(err.clone())));
        assert!(matches!(event, LspEvent::Response { id: 6, result: Err(e) } if e == err));
    }

    #[test]
    fn a_response_id_this_client_never_generated_as_a_string_falls_back_to_negative_one() {
        // Not spec-compliant of a server (this client only ever sends
        // numeric ids), but shouldn't panic -- see `classify`'s own doc
        // comment.
        let event = classify(RawMessage::response(Value::from("weird-id"), Ok(Value::Null)));
        assert!(matches!(event, LspEvent::Response { id: -1, .. }));
    }

    #[test]
    fn neither_id_nor_method_classifies_as_an_empty_notification_rather_than_panicking() {
        let msg = RawMessage { jsonrpc: "2.0".to_string(), id: None, method: None, params: None, result: None, error: None };
        let event = classify(msg);
        assert!(matches!(event, LspEvent::Notification { method, .. } if method.is_empty()));
    }
}
