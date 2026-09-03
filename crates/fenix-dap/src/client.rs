use std::io::{BufReader, Read};
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Mutex;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use debug_adapter_protocol::events::Event;
use debug_adapter_protocol::requests::{DisconnectRequestArguments, Request};
use debug_adapter_protocol::responses::{ErrorResponse, Response, SuccessResponse};
use debug_adapter_protocol::{ProtocolMessage, ProtocolMessageContent};

/// One event surfaced from the adapter -- a response to a request this
/// client previously sent (matched by `request_seq`, whichever value
/// `request` returned), an unsolicited event (`stopped`, `output`,
/// `terminated`, ...), or an adapter-initiated reverse request (e.g.
/// `runInTerminal`, which a spec-compliant client must eventually
/// answer via `respond`) -- same shape `fenix_lsp::LspEvent` already
/// established for the identical role there.
#[derive(Debug, Clone)]
pub enum DapEvent {
    Response { request_seq: i64, result: Result<SuccessResponse, ErrorResponse> },
    Event(Event),
    ReverseRequest { seq: i64, request: Request },
    /// A well-framed, valid-JSON message that failed to decode into
    /// `ProtocolMessage`'s strict, spec-only `Event`/`Request`/
    /// `Response` shapes -- kept as the raw decoded JSON rather than
    /// dropped or treated as a disconnect. This is expected, not
    /// exceptional: DAP's own spec explicitly allows (and real, common
    /// adapters use) vendor-specific custom events beyond the spec'd
    /// set -- confirmed live against `debugpy`, which sends its own
    /// `debugpySockets` event on every session that the `debug-adapter-
    /// protocol` crate's tagged `Event` enum has no variant for. Per
    /// spec, a client should simply ignore an event it doesn't
    /// recognize -- surfaced here rather than silently swallowed so a
    /// caller *can* log/inspect it, without that unrecognized shape
    /// taking down an otherwise perfectly healthy connection the way a
    /// hard decode failure used to.
    Unknown(serde_json::Value),
    /// The connection ended -- a read error, the adapter closing its
    /// stdout, or a message that wasn't even valid JSON at all (as
    /// opposed to valid JSON in an unrecognized shape, which is
    /// `Unknown` instead). No more events follow this one.
    Disconnected(String),
}

/// A live connection to one debug adapter process. Shaped like
/// `fenix_lsp::LspClient`: owns a dedicated background thread reading
/// the adapter's stdout as a stream of decoded `DapEvent`s for as long
/// as the connection lives.
///
/// Does *not* perform the `initialize`/`launch`/`attach` handshake --
/// that's an ordinary request/response (and event-waiting) exchange
/// over this same connection, left to the caller to drive (different
/// adapters want different `initialize` capabilities and `launch`
/// arguments, and the exact request/event ordering -- wait for
/// `initialized` before sending `setBreakpoints`, then
/// `configurationDone` -- is a caller-level state machine, not
/// something a generic client should hardcode).
pub struct DapClient {
    stdin: Mutex<ChildStdin>,
    next_seq: AtomicU64,
    child: Mutex<Child>,
    reader_thread: Option<JoinHandle<()>>,
    stderr_thread: Option<JoinHandle<()>>,
}

impl DapClient {
    /// Spawns `command` (with `args`), working directory `cwd`. Uses
    /// `fenix_rpc::resolve_command` first -- see its own doc comment for
    /// why a bare command name needs that on Windows.
    pub fn spawn(command: &str, args: &[String], cwd: &std::path::Path) -> std::io::Result<(DapClient, Receiver<DapEvent>)> {
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
        // Drained and discarded, never read from the main loop -- same
        // "an adapter that writes enough debug output there would
        // otherwise stall the whole connection" reasoning `fenix_lsp::
        // LspClient::spawn` already established for language servers.
        let stderr_thread = thread::spawn(move || drain_and_discard(stderr));

        Ok((
            DapClient { stdin: Mutex::new(stdin), next_seq: AtomicU64::new(1), child: Mutex::new(child), reader_thread: Some(reader_thread), stderr_thread: Some(stderr_thread) },
            rx,
        ))
    }

    /// Sends a request, returning the `seq` its response will be tagged
    /// with (`DapEvent::Response { request_seq, .. }`). This client does
    /// no correlation beyond tagging the response with that value -- a
    /// caller tracking "what did I ask seq 7 for" does so itself, same
    /// as `LspClient::request`.
    pub fn request(&self, req: Request) -> std::io::Result<i64> {
        let seq = self.next_seq.fetch_add(1, Ordering::Relaxed);
        self.write(&ProtocolMessage { seq, content: ProtocolMessageContent::Request(req) })?;
        Ok(seq as i64)
    }

    /// Answers an adapter-initiated reverse request (a
    /// `DapEvent::ReverseRequest`) -- e.g. `runInTerminal`.
    pub fn respond(&self, request_seq: i64, result: Result<SuccessResponse, ErrorResponse>) -> std::io::Result<()> {
        let seq = self.next_seq.fetch_add(1, Ordering::Relaxed);
        self.write(&ProtocolMessage { seq, content: ProtocolMessageContent::Response(Response { request_seq: request_seq as u64, result }) })
    }

    fn write(&self, message: &ProtocolMessage) -> std::io::Result<()> {
        let body = serde_json::to_vec(message).expect("ProtocolMessage always serializes");
        let mut stdin = self.stdin.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        fenix_rpc::write_message(&mut *stdin, &body)
    }
}

impl Drop for DapClient {
    /// Best-effort graceful shutdown -- DAP's own teardown is a
    /// `disconnect` request, sent without waiting for its reply (this
    /// client is going away regardless), then a short bounded wait for
    /// the process to actually exit before force-killing it. Same "ask
    /// nicely, then force, but never block indefinitely" posture as
    /// `LspClient::drop`'s own doc comment reasons about.
    fn drop(&mut self) {
        let seq = self.next_seq.fetch_add(1, Ordering::Relaxed);
        let disconnect = ProtocolMessage {
            seq,
            content: ProtocolMessageContent::Request(Request::Disconnect(DisconnectRequestArguments::builder().build())),
        };
        let _ = self.write(&disconnect);
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
/// off `stdout` one at a time and forwards each as a classified
/// `DapEvent`, until either the connection closes or the caller drops
/// its `Receiver`.
fn read_loop(stdout: ChildStdout, tx: Sender<DapEvent>) {
    let mut reader = BufReader::new(stdout);
    loop {
        match fenix_rpc::read_message(&mut reader) {
            Ok(Some(bytes)) => {
                let event = decode_message(&bytes);
                let disconnected = matches!(event, DapEvent::Disconnected(_));
                if tx.send(event).is_err() {
                    return; // caller dropped the receiver -- nothing left to forward to
                }
                if disconnected {
                    return;
                }
            }
            Ok(None) => {
                let _ = tx.send(DapEvent::Disconnected("adapter closed its stdout".to_string()));
                return;
            }
            Err(err) => {
                let _ = tx.send(DapEvent::Disconnected(err.to_string()));
                return;
            }
        }
    }
}

/// Decodes one raw framed message body into a `DapEvent` -- `Unknown`/
/// `Disconnected` for the two ways this can go wrong, see their own doc
/// comments. Split out from `read_loop` so this decoding logic (the
/// part that actually needed a regression test after `debugpy`'s own
/// custom `debugpySockets` event broke it) is directly testable without
/// a real child process's `ChildStdout`.
fn decode_message(bytes: &[u8]) -> DapEvent {
    match serde_json::from_slice::<ProtocolMessage>(bytes) {
        Ok(msg) => classify(msg),
        Err(_) => match serde_json::from_slice::<serde_json::Value>(bytes) {
            Ok(raw) => DapEvent::Unknown(raw),
            Err(err) => DapEvent::Disconnected(format!("malformed message from adapter: {err}")),
        },
    }
}

fn classify(msg: ProtocolMessage) -> DapEvent {
    match msg.content {
        ProtocolMessageContent::Response(r) => DapEvent::Response { request_seq: r.request_seq as i64, result: r.result },
        ProtocolMessageContent::Event(e) => DapEvent::Event(e),
        ProtocolMessageContent::Request(req) => DapEvent::ReverseRequest { seq: msg.seq as i64, request: req },
    }
}

fn drain_and_discard(mut stderr: ChildStderr) {
    let mut buf = [0u8; 4096];
    loop {
        match stderr.read(&mut buf) {
            Ok(0) | Err(_) => return,
            Ok(_) => continue,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use debug_adapter_protocol::events::{OutputCategory, OutputEventBody};
    use debug_adapter_protocol::requests::InitializeRequestArguments;
    use debug_adapter_protocol::responses::ErrorResponseBody;
    use debug_adapter_protocol::types::Capabilities;

    fn response_msg(request_seq: u64, result: Result<SuccessResponse, ErrorResponse>) -> ProtocolMessage {
        ProtocolMessage { seq: 99, content: ProtocolMessageContent::Response(Response { request_seq, result }) }
    }

    #[test]
    fn a_response_message_classifies_as_a_response_event() {
        let event = classify(response_msg(1, Ok(SuccessResponse::Initialize(Capabilities::default()))));
        assert!(matches!(event, DapEvent::Response { request_seq: 1, result: Ok(SuccessResponse::Initialize(_)) }));
    }

    #[test]
    fn a_failed_response_carries_err_result() {
        let err = ErrorResponse::builder().command("launch".to_string()).message("boom".to_string()).body(ErrorResponseBody::new(None)).build();
        let event = classify(response_msg(2, Err(err.clone())));
        assert!(matches!(event, DapEvent::Response { request_seq: 2, result: Err(e) } if e == err));
    }

    #[test]
    fn an_event_message_classifies_as_an_event() {
        let msg = ProtocolMessage { seq: 5, content: ProtocolMessageContent::Event(Event::Initialized) };
        let event = classify(msg);
        assert!(matches!(event, DapEvent::Event(Event::Initialized)));
    }

    #[test]
    fn an_output_event_carries_its_body() {
        let body = OutputEventBody::builder().output("hello\n".to_string()).category(OutputCategory::Stdout).build();
        let msg = ProtocolMessage { seq: 6, content: ProtocolMessageContent::Event(Event::Output(body.clone())) };
        let event = classify(msg);
        assert!(matches!(event, DapEvent::Event(Event::Output(b)) if b == body));
    }

    #[test]
    fn a_request_message_classifies_as_a_reverse_request() {
        let msg = ProtocolMessage {
            seq: 7,
            content: ProtocolMessageContent::Request(Request::Initialize(InitializeRequestArguments::builder().adapter_id("test".to_string()).build())),
        };
        let event = classify(msg);
        assert!(matches!(event, DapEvent::ReverseRequest { seq: 7, request: Request::Initialize(_) }));
    }

    #[test]
    fn a_well_known_event_decodes_normally() {
        let bytes = br#"{"seq":1,"type":"event","event":"initialized"}"#;
        assert!(matches!(decode_message(bytes), DapEvent::Event(Event::Initialized)));
    }

    #[test]
    fn an_adapter_specific_custom_event_decodes_as_unknown_not_a_disconnect() {
        // The real regression this guards: found live-testing against
        // `debugpy`, which sends its own `debugpySockets` event on every
        // session -- a shape the `debug-adapter-protocol` crate's `Event`
        // enum has no variant for, and used to kill the whole connection
        // on a perfectly healthy adapter.
        let bytes = br#"{"seq":3,"type":"event","event":"debugpySockets","body":{"sockets":[]}}"#;
        match decode_message(bytes) {
            DapEvent::Unknown(value) => {
                assert_eq!(value["event"], "debugpySockets");
            }
            other => panic!("expected DapEvent::Unknown, got {other:?}"),
        }
    }

    #[test]
    fn bytes_that_are_not_valid_json_at_all_disconnect() {
        let bytes = b"this is not json";
        assert!(matches!(decode_message(bytes), DapEvent::Disconnected(_)));
    }
}
