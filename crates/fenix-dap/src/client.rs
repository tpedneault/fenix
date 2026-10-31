use std::io::{BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

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
    /// A reverse request the `debug-adapter-protocol` crate doesn't model
    /// -- js-debug's `startDebugging`, newer than it -- answered with
    /// `DapClient::respond_json`.
    RawRequest { seq: i64, command: String, arguments: serde_json::Value },
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
    outbox: Arc<Mutex<Outbox>>,
    next_seq: AtomicU64,
    /// The adapter's process, when this client started it.
    child: Option<Arc<Mutex<Child>>>,
    /// Over a socket: the connection, once made, shut when this goes.
    socket: Arc<Mutex<Option<TcpStream>>>,
    closing: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
}

/// Where messages for the adapter go: its stdin, or its socket once it's
/// connected -- until then they wait here, in order.
#[derive(Default)]
struct Outbox {
    out: Option<Box<dyn Write + Send>>,
    waiting: Vec<Vec<u8>>,
}

impl Outbox {
    fn send(&mut self, body: &[u8]) -> std::io::Result<()> {
        match &mut self.out {
            Some(out) => fenix_rpc::write_message(out, body),
            None => {
                self.waiting.push(body.to_vec());
                Ok(())
            }
        }
    }
}

/// How long an adapter that listens on a port has to start listening.
const LISTENING: Duration = Duration::from_secs(10);

/// A port on 127.0.0.1 nothing listens on now: one for an adapter told to
/// listen there (`--port ${port}`).
pub fn free_port() -> std::io::Result<u16> {
    Ok(std::net::TcpListener::bind("127.0.0.1:0")?.local_addr()?.port())
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl DapClient {
    /// Spawns `command` (with `args`), working directory `cwd`. Uses
    /// `fenix_rpc::resolve_command` first -- see its own doc comment for
    /// why a bare command name needs that on Windows.
    pub fn spawn(command: &str, args: &[String], cwd: &std::path::Path) -> std::io::Result<(DapClient, Receiver<DapEvent>)> {
        let mut process = Command::new(fenix_rpc::resolve_command(command));
        process.args(args).current_dir(cwd);
        Self::spawn_command(process)
    }

    /// Spawn a configured process, preserving its explicit arguments, cwd and environment.
    pub fn spawn_command(mut process: Command) -> std::io::Result<(DapClient, Receiver<DapEvent>)> {
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // Fenix is a windowed app: without this every server (and every
            // tool it starts in turn) opens a console window on Windows.
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            process.creation_flags(CREATE_NO_WINDOW);
        }
        let mut child = process.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;

        let stdin = child.stdin.take().expect("stdin was requested as piped");
        let stdout = child.stdout.take().expect("stdout was requested as piped");
        let stderr = child.stderr.take().expect("stderr was requested as piped");

        let (tx, rx) = mpsc::channel();
        let said = StderrTail::default();
        let said2 = said.clone();
        let reader_thread = thread::spawn(move || read_loop(stdout, tx, Some(said2)));
        // Drained, never read from the main loop -- same "an adapter that
        // writes enough debug output there would otherwise stall the whole
        // connection" reasoning `fenix_lsp::LspClient::spawn` already
        // established for language servers -- its end kept, for why an
        // adapter that quits did.
        let stderr_thread = thread::spawn(move || drain_keeping_tail(stderr, &said));

        let outbox = Outbox { out: Some(Box::new(stdin)), waiting: Vec::new() };
        Ok((
            DapClient {
                outbox: Arc::new(Mutex::new(outbox)),
                next_seq: AtomicU64::new(1),
                child: Some(Arc::new(Mutex::new(child))),
                socket: Arc::default(),
                closing: Arc::default(),
                threads: vec![reader_thread, stderr_thread],
            },
            rx,
        ))
    }

    /// An adapter that speaks over a socket rather than stdio (js-debug,
    /// CodeLLDB's `--port`): `process` started, when given -- told the
    /// port in its arguments -- then 127.0.0.1:`port` connected to as soon
    /// as it listens there, within 10 seconds. Without a process, an
    /// adapter already listening. Returns at once: what's sent meanwhile
    /// waits, and a connection that can't be made is a `Disconnected`.
    pub fn connect_command(process: Option<Command>, port: u16) -> std::io::Result<(DapClient, Receiver<DapEvent>)> {
        let mut threads = Vec::new();
        let child = match process {
            Some(mut process) => {
                #[cfg(windows)]
                {
                    use std::os::windows::process::CommandExt;
                    process.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
                }
                let mut child = process.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
                // Its output isn't the protocol here: drained, as stderr
                // is for one on stdio.
                let (stdout, stderr) = (child.stdout.take().expect("piped"), child.stderr.take().expect("piped"));
                threads.push(thread::spawn(move || drain_and_discard(stdout)));
                threads.push(thread::spawn(move || drain_and_discard(stderr)));
                Some(Arc::new(Mutex::new(child)))
            }
            None => None,
        };
        let (outbox, socket, closing) = (Arc::new(Mutex::new(Outbox::default())), Arc::new(Mutex::new(None::<TcpStream>)), Arc::new(AtomicBool::new(false)));
        let (tx, rx) = mpsc::channel();
        let (out2, socket2, closing2, child2) = (outbox.clone(), socket.clone(), closing.clone(), child.clone());
        threads.push(thread::spawn(move || {
            let address = SocketAddr::from(([127, 0, 0, 1], port));
            let started = Instant::now();
            let stream = loop {
                if closing2.load(Ordering::Relaxed) {
                    return;
                }
                if let Ok(stream) = TcpStream::connect_timeout(&address, Duration::from_millis(500)) {
                    break stream;
                }
                let exited = child2.as_ref().and_then(|c| lock(c).try_wait().ok().flatten());
                if let Some(status) = exited {
                    let _ = tx.send(DapEvent::Disconnected(format!("the adapter ended ({status}) before it listened on {address}")));
                    return;
                }
                if started.elapsed() > LISTENING {
                    let _ = tx.send(DapEvent::Disconnected(format!("nothing listened on {address} within {} seconds", LISTENING.as_secs())));
                    return;
                }
                thread::sleep(Duration::from_millis(100));
            };
            let _ = stream.set_nodelay(true);
            let (Ok(writing), Ok(kept)) = (stream.try_clone(), stream.try_clone()) else {
                let _ = tx.send(DapEvent::Disconnected("couldn't share the connection".into()));
                return;
            };
            *lock(&socket2) = Some(kept);
            {
                let mut outbox = lock(&out2);
                let mut writing: Box<dyn Write + Send> = Box::new(writing);
                for body in std::mem::take(&mut outbox.waiting) {
                    if let Err(e) = fenix_rpc::write_message(&mut writing, &body) {
                        let _ = tx.send(DapEvent::Disconnected(e.to_string()));
                        return;
                    }
                }
                outbox.out = Some(writing);
            }
            read_loop(stream, tx, None);
        }));
        Ok((DapClient { outbox, next_seq: AtomicU64::new(1), child, socket, closing, threads }, rx))
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

    /// Sends `attach` with `arguments` -- what the adapter takes to find
    /// the program (a process id, a port). The `debug-adapter-protocol`
    /// crate's `AttachRequestArguments` has no room for those, so this
    /// one is written as JSON; its response decodes as any other.
    pub fn attach(&self, arguments: serde_json::Map<String, serde_json::Value>) -> std::io::Result<i64> {
        self.request_json("attach", serde_json::Value::Object(arguments))
    }

    /// Sends request `command` with `arguments` as they are: for what the
    /// crate's types have no room for -- `initialize`'s
    /// `supportsStartDebuggingRequest`, `attach`'s process id. Its response
    /// decodes as any other.
    pub fn request_json(&self, command: &str, arguments: serde_json::Value) -> std::io::Result<i64> {
        let seq = self.next_seq.fetch_add(1, Ordering::Relaxed);
        let body = serde_json::to_vec(&request_message(seq, command, arguments)).expect("JSON always serializes");
        lock(&self.outbox).send(&body)?;
        Ok(seq as i64)
    }

    /// Answers a reverse request the crate doesn't model
    /// (`DapEvent::RawRequest`): with a body, or an error's message.
    pub fn respond_json(&self, request_seq: i64, command: &str, result: Result<serde_json::Value, String>) -> std::io::Result<()> {
        let seq = self.next_seq.fetch_add(1, Ordering::Relaxed);
        let body = serde_json::to_vec(&response_message(seq, request_seq, command, result)).expect("JSON always serializes");
        lock(&self.outbox).send(&body)
    }

    /// Answers an adapter-initiated reverse request (a
    /// `DapEvent::ReverseRequest`) -- e.g. `runInTerminal`.
    pub fn respond(&self, request_seq: i64, result: Result<SuccessResponse, ErrorResponse>) -> std::io::Result<()> {
        let seq = self.next_seq.fetch_add(1, Ordering::Relaxed);
        self.write(&ProtocolMessage { seq, content: ProtocolMessageContent::Response(Response { request_seq: request_seq as u64, result }) })
    }

    fn write(&self, message: &ProtocolMessage) -> std::io::Result<()> {
        let body = serde_json::to_vec(message).expect("ProtocolMessage always serializes");
        lock(&self.outbox).send(&body)
    }
}

impl Drop for DapClient {
    fn drop(&mut self) {
        self.close();
    }
}

impl DapClient {
    /// Best-effort graceful shutdown -- DAP's own teardown is a
    /// `disconnect` request, sent without waiting for its reply (this
    /// client is going away regardless), then a short bounded wait for
    /// the process to actually exit before force-killing it. Same "ask
    /// nicely, then force, but never block indefinitely" posture as
    /// `LspClient::drop`'s own doc comment reasons about. Once: what's
    /// sent after it goes nowhere, and dropping it then does nothing more.
    pub fn close(&mut self) {
        if self.closing.swap(true, Ordering::Relaxed) && self.threads.is_empty() {
            return;
        }
        let seq = self.next_seq.fetch_add(1, Ordering::Relaxed);
        let disconnect = ProtocolMessage {
            seq,
            content: ProtocolMessageContent::Request(Request::Disconnect(DisconnectRequestArguments::builder().build())),
        };
        let _ = self.write(&disconnect);
        self.closing.store(true, Ordering::Relaxed);
        if let Some(mut child) = self.child.as_ref().map(|c| lock(c)) {
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
        // Its threads end on their own, not waited for. Over a socket, the
        // reader ends when the connection does -- which on Windows is when
        // the adapter closes its end: a shutdown here doesn't wake a read
        // blocked there; a closing adapter that keeps its socket open held
        // Fenix two minutes. Over stdio, the pipes close when every process
        // holding them has: killing `uv run` leaves the Python it started,
        // and the debuggee, with them, which held Fenix until they quit.
        if let Some(socket) = lock(&self.socket).take() {
            let _ = socket.shutdown(std::net::Shutdown::Both);
        }
        // Its stdin closed, so what `uv run` left reads its end and quits.
        lock(&self.outbox).out = None;
        self.threads.clear();
    }
}

/// The connection's entire background lifetime: decodes framed messages
/// off `stdout` one at a time and forwards each as a classified
/// `DapEvent`, until either the connection closes or the caller drops
/// its `Receiver`.
/// Request `command`, numbered `seq`.
fn request_message(seq: u64, command: &str, arguments: serde_json::Value) -> serde_json::Value {
    serde_json::json!({ "seq": seq, "type": "request", "command": command, "arguments": arguments })
}

/// The answer to reverse request `request_seq`.
fn response_message(seq: u64, request_seq: i64, command: &str, result: Result<serde_json::Value, String>) -> serde_json::Value {
    match result {
        Ok(body) => serde_json::json!({ "seq": seq, "type": "response", "request_seq": request_seq, "success": true, "command": command, "body": body }),
        Err(message) => serde_json::json!({ "seq": seq, "type": "response", "request_seq": request_seq, "success": false, "command": command, "message": message }),
    }
}

/// The end of what an adapter wrote to stderr.
type StderrTail = Arc<Mutex<Vec<u8>>>;

/// How much of an adapter's stderr is kept.
const STDERR_TAIL: usize = 4096;

/// Why an adapter on stdio went away: its stdout closed, and the last line
/// it wrote to stderr, if any, given a moment to arrive.
fn closed_stdout(said: Option<&StderrTail>) -> String {
    let last = said.and_then(|said| {
        thread::sleep(Duration::from_millis(200));
        let text = String::from_utf8_lossy(&lock(said)).into_owned();
        text.lines().map(str::trim).rfind(|l| !l.is_empty()).map(str::to_string)
    });
    match last {
        Some(line) => format!("adapter closed its stdout: {line}"),
        None => "adapter closed its stdout".to_string(),
    }
}

fn read_loop(stdout: impl Read, tx: Sender<DapEvent>, said: Option<StderrTail>) {
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
                let _ = tx.send(DapEvent::Disconnected(closed_stdout(said.as_ref())));
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
    if let Ok(msg) = serde_json::from_slice::<ProtocolMessage>(bytes) {
        return classify(msg);
    }
    let mut raw = match serde_json::from_slice::<serde_json::Value>(bytes) {
        Ok(raw) => raw,
        Err(err) => return DapEvent::Disconnected(format!("malformed message from adapter: {err}")),
    };
    // What the crate's types are stricter about than adapters are, put
    // right and read again (`lenient`).
    if lenient(&mut raw) {
        if let Ok(msg) = serde_json::from_value::<ProtocolMessage>(raw.clone()) {
            return classify(msg);
        }
    }
    if raw["type"] == "request" && raw["command"].is_string() {
        return DapEvent::RawRequest {
            seq: raw["seq"].as_i64().unwrap_or_default(),
            command: raw["command"].as_str().unwrap_or_default().to_string(),
            arguments: raw.get("arguments").cloned().unwrap_or(serde_json::Value::Null),
        };
    }
    DapEvent::Unknown(raw)
}

/// `raw` with what adapters send that the crate's types refuse put right;
/// false when there was nothing to. An event's body the crate wants
/// other than it came: none where the spec makes it optional
/// (`terminated`), or an empty one where the crate has none (js-debug's
/// `initialized`), and the same of a response's (its `launch`).
/// js-debug's stack frames say `presentationHint:
/// "deemphasize"`, a hint only sources have. netcoredbg numbers its
/// messages with strings (`"seq":"2"`); lldb-dap's `initialized` carries
/// its statistics; CodeLLDB refuses a request with `"command":""`, no body
/// and a `show_user` of its own.
fn lenient(raw: &mut serde_json::Value) -> bool {
    let mut changed = false;
    for key in ["seq", "request_seq"] {
        if let Some(number) = raw.get(key).and_then(|v| v.as_str()).and_then(|s| s.trim().parse::<u64>().ok()) {
            raw[key] = serde_json::json!(number);
            changed = true;
        }
    }
    // A refusal is read for its message: an error body the crate can't
    // take, or none, becomes an empty one, and what the spec doesn't name
    // goes.
    if raw["type"] == "response" && raw["success"] == false {
        if let Some(message) = raw.as_object_mut() {
            let keep = ["seq", "type", "request_seq", "success", "command", "message"];
            message.retain(|k, _| keep.contains(&k.as_str()));
            message.insert("body".into(), serde_json::json!({}));
            if !message.get("command").is_some_and(|c| c.is_string()) {
                message.insert("command".into(), serde_json::json!(""));
            }
            if !message.get("message").is_some_and(|m| m.is_string()) {
                message.insert("message".into(), serde_json::json!("the adapter refused, without saying why"));
            }
            changed = true;
        }
    }
    // A success with an empty body where the crate has none (js-debug's
    // `launch`, `configurationDone`).
    if raw["type"] == "response" && raw["success"] == true && raw["body"].as_object().is_some_and(|o| o.is_empty()) {
        if let Some(message) = raw.as_object_mut() {
            message.remove("body");
            changed = true;
        }
    }
    if raw["type"] == "event" {
        if let Some(message) = raw.as_object_mut() {
            match message.get("body") {
                None => {
                    message.insert("body".into(), serde_json::json!({}));
                    changed = true;
                }
                Some(body) if body.is_null() || body.as_object().is_some_and(|o| o.is_empty()) => {
                    message.remove("body");
                    changed = true;
                }
                Some(_) => {}
            }
        }
    }
    // `initialized` has nothing to say: whatever an adapter puts in its
    // body is dropped.
    if raw["type"] == "event" && raw["event"] == "initialized" {
        if let Some(message) = raw.as_object_mut() {
            changed |= message.remove("body").is_some();
        }
    }
    if let Some(frames) = raw.pointer_mut("/body/stackFrames").and_then(|f| f.as_array_mut()) {
        for frame in frames.iter_mut().filter_map(|f| f.as_object_mut()) {
            if frame.get("presentationHint").and_then(|h| h.as_str()).is_some_and(|h| !matches!(h, "normal" | "label" | "subtle")) {
                frame.remove("presentationHint");
                changed = true;
            }
        }
    }
    changed
}

fn classify(msg: ProtocolMessage) -> DapEvent {
    match msg.content {
        ProtocolMessageContent::Response(r) => DapEvent::Response { request_seq: r.request_seq as i64, result: r.result },
        ProtocolMessageContent::Event(e) => DapEvent::Event(e),
        ProtocolMessageContent::Request(req) => DapEvent::ReverseRequest { seq: msg.seq as i64, request: req },
    }
}

fn drain_keeping_tail(mut stderr: impl Read, said: &StderrTail) {
    let mut buf = [0u8; 4096];
    loop {
        match stderr.read(&mut buf) {
            Ok(0) | Err(_) => return,
            Ok(n) => {
                let mut tail = lock(said);
                tail.extend_from_slice(&buf[..n]);
                let over = tail.len().saturating_sub(STDERR_TAIL);
                tail.drain(..over);
            }
        }
    }
}

fn drain_and_discard(mut stderr: impl Read) {
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
    fn an_attach_carries_what_the_adapter_takes_and_its_answer_decodes() {
        let mut arguments = serde_json::Map::new();
        arguments.insert("processId".into(), serde_json::json!(42));
        let message = request_message(7, "attach", serde_json::Value::Object(arguments));
        assert_eq!(message, serde_json::json!({ "seq": 7, "type": "request", "command": "attach", "arguments": { "processId": 42 } }));
        let answer = br#"{"seq":3,"type":"response","request_seq":7,"success":true,"command":"attach"}"#;
        assert!(matches!(decode_message(answer), DapEvent::Response { request_seq: 7, result: Ok(SuccessResponse::Attach) }), "{:?}", decode_message(answer));
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

    /// The tests that take a free port, one at a time: two given the same
    /// one at once, one's client connected to the other's listener (and
    /// closed it), and the first read there found nothing.
    static PORTS: Mutex<()> = Mutex::new(());

    /// An adapter over a socket: what's sent before it listens waits, and
    /// goes once it's connected; its answers come back as from stdio.
    #[test]
    fn an_adapter_on_a_port_is_connected_to_once_it_listens() {
        let _one = lock(&PORTS);
        let port = free_port().unwrap();
        let (client, rx) = DapClient::connect_command(None, port).unwrap();
        let seq = client.request(Request::Initialize(InitializeRequestArguments::builder().adapter_id("fenix".to_string()).build())).unwrap();
        // It starts listening a moment later.
        std::thread::sleep(Duration::from_millis(300));
        let listener = std::net::TcpListener::bind(("127.0.0.1", port)).unwrap();
        // The client's connection: the first that says something (another
        // program's, on a port just freed, would close at once).
        let (stream, mut reading, first) = loop {
            let (stream, _) = listener.accept().unwrap();
            let mut reading = BufReader::new(stream.try_clone().unwrap());
            if let Ok(Some(first)) = fenix_rpc::read_message(&mut reading) {
                break (stream, reading, first);
            }
        };
        let asked: serde_json::Value = serde_json::from_slice(&first).unwrap();
        assert_eq!((asked["command"].as_str(), asked["seq"].as_i64()), (Some("initialize"), Some(seq)));
        let answer = format!(r#"{{"seq":1,"type":"response","request_seq":{seq},"success":true,"command":"initialize","body":{{}}}}"#);
        fenix_rpc::write_message(&mut &stream, answer.as_bytes()).unwrap();
        assert!(matches!(rx.recv_timeout(Duration::from_secs(10)).unwrap(), DapEvent::Response { request_seq, result: Ok(_) } if request_seq == seq));
        drop(client);
        // Its `disconnect`, then the connection's end.
        let bye: serde_json::Value = serde_json::from_slice(&fenix_rpc::read_message(&mut reading).unwrap().unwrap()).unwrap();
        assert_eq!(bye["command"], "disconnect");
    }

    #[test]
    fn nothing_listening_is_said() {
        let _one = lock(&PORTS);
        let mut never = Command::new(std::env::current_exe().unwrap());
        never.arg("--exact").arg("no-such-test");
        let (_client, rx) = DapClient::connect_command(Some(never), free_port().unwrap()).unwrap();
        assert!(matches!(rx.recv_timeout(Duration::from_secs(15)).unwrap(), DapEvent::Disconnected(why) if why.contains("before it listened")));
    }

    /// js-debug's `startDebugging`, which the crate has no type
    /// for, comes as a raw request, and is answered as one.
    #[test]
    fn a_reverse_request_the_crate_doesnt_know_is_raw_and_answered_so() {
        let bytes = br#"{"seq":9,"type":"request","command":"startDebugging","arguments":{"request":"launch","configuration":{"type":"pwa-node","__pendingTargetId":"t1"}}}"#;
        match decode_message(bytes) {
            DapEvent::RawRequest { seq, command, arguments } => {
                assert_eq!((seq, command.as_str()), (9, "startDebugging"));
                assert_eq!(arguments["configuration"]["__pendingTargetId"], "t1");
            }
            other => panic!("expected a raw request, got {other:?}"),
        }
        assert_eq!(response_message(3, 9, "startDebugging", Ok(serde_json::json!({}))), serde_json::json!({ "seq": 3, "type": "response", "request_seq": 9, "success": true, "command": "startDebugging", "body": {} }));
        assert_eq!(response_message(4, 9, "x", Err("no".into()))["success"], false);
    }

    /// `terminated` with no body, as the spec allows, is still
    /// `terminated`; js-debug's `initialized` with an empty one, still
    /// `initialized`.
    #[test]
    fn an_event_with_its_body_left_out_or_empty_decodes() {
        assert!(matches!(decode_message(br#"{"seq":4,"type":"event","event":"terminated"}"#), DapEvent::Event(Event::Terminated(_))));
        assert!(matches!(decode_message(br#"{"seq":2,"type":"event","event":"initialized","body":{}}"#), DapEvent::Event(Event::Initialized)));
        assert!(matches!(decode_message(br#"{"seq":2,"type":"event","event":"initialized","body":null}"#), DapEvent::Event(Event::Initialized)));
        assert!(matches!(decode_message(br#"{"seq":7,"type":"response","request_seq":2,"command":"launch","success":true,"body":{}}"#), DapEvent::Response { request_seq: 2, result: Ok(SuccessResponse::Launch) }));
    }

    /// js-debug's stack frames, `deemphasize`d, still read as a stack.
    #[test]
    fn a_stack_frame_hinted_as_a_source_is_still_read() {
        let bytes = br#"{"seq":162,"type":"response","request_seq":7,"command":"stackTrace","success":true,"body":{"stackFrames":[{"id":0,"name":"<anonymous>","line":2,"column":11,"presentationHint":"normal"},{"id":1,"name":"run","line":9,"column":1,"source":{"sourceReference":7,"presentationHint":"deemphasize","origin":"Skipped by skipFiles"},"presentationHint":"deemphasize"}],"totalFrames":2}}"#;
        match decode_message(bytes) {
            DapEvent::Response { result: Ok(SuccessResponse::StackTrace(body)), .. } => assert_eq!(body.stack_frames.len(), 2),
            other => panic!("expected a stack, got {other:?}"),
        }
    }

    /// netcoredbg 3.1.2 numbers its messages with strings (`"seq":"2"`):
    /// its `initialize` answer still reads, so `launch` goes out.
    #[test]
    fn netcoredbg_s_string_numbers_still_read() {
        let initialize = include_bytes!("../tests/adapters/netcoredbg-3.1.2-initialize.json");
        assert!(matches!(decode_message(initialize), DapEvent::Response { request_seq: 1, result: Ok(SuccessResponse::Initialize(_)) }), "{:?}", decode_message(initialize));
        let initialized = include_bytes!("../tests/adapters/netcoredbg-3.1.2-initialized.json");
        assert!(matches!(decode_message(initialized), DapEvent::Event(Event::Initialized)), "{:?}", decode_message(initialized));
        // Its own `capabilities` event, before the answer: not a disconnect.
        let capabilities = include_bytes!("../tests/adapters/netcoredbg-3.1.2-capabilities.json");
        assert!(matches!(decode_message(capabilities), DapEvent::Event(Event::Capabilities(_)) | DapEvent::Unknown(_)), "{:?}", decode_message(capabilities));
    }

    /// lldb-dap 23 sends `initialized` with its statistics as the body:
    /// still `initialized`, so the breakpoints and `configurationDone` go.
    #[test]
    fn lldb_dap_s_initialized_with_a_body_is_initialized() {
        let initialized = include_bytes!("../tests/adapters/lldb-dap-23.1.4-initialized.json");
        assert!(matches!(decode_message(initialized), DapEvent::Event(Event::Initialized)), "{:?}", decode_message(initialized));
    }

    /// CodeLLDB 1.12.3 refuses a launch with `"command":""`, no body and
    /// `show_user`: still the launch's error, with its message.
    #[test]
    fn codelldb_s_failed_launch_is_an_error_with_its_message() {
        let failed = include_bytes!("../tests/adapters/codelldb-1.12.3-launch-failed.json");
        match decode_message(failed) {
            DapEvent::Response { request_seq: 2, result: Err(err) } => assert!(err.message.contains("lldb_commands - file not found"), "{}", err.message),
            other => panic!("expected the launch's error, got {other:?}"),
        }
    }

    #[test]
    fn bytes_that_are_not_valid_json_at_all_disconnect() {
        let bytes = b"this is not json";
        assert!(matches!(decode_message(bytes), DapEvent::Disconnected(_)));
    }

    #[test]
    fn an_adapter_that_quits_says_why_with_its_last_stderr_line() {
        let mut process = if cfg!(windows) { Command::new("cmd") } else { Command::new("sh") };
        if cfg!(windows) {
            process.args(["/c", "echo starting 1>&2 && echo No module named debugpy 1>&2"]);
        } else {
            process.args(["-c", "echo starting >&2; echo No module named debugpy >&2"]);
        }
        let (_client, rx) = DapClient::spawn_command(process).unwrap();
        match rx.recv_timeout(Duration::from_secs(10)).unwrap() {
            DapEvent::Disconnected(why) => assert_eq!(why, "adapter closed its stdout: No module named debugpy"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn closing_doesnt_wait_on_what_the_adapter_started() {
        // A grandchild holding the adapter's stdout and stderr past its end.
        let mut process = if cfg!(windows) { Command::new("cmd") } else { Command::new("sh") };
        if cfg!(windows) {
            process.args(["/c", "start /b ping -n 30 127.0.0.1 & ping -n 30 127.0.0.1"]);
        } else {
            process.args(["-c", "sleep 30 & sleep 30"]);
        }
        let (mut client, _rx) = DapClient::spawn_command(process).unwrap();
        thread::sleep(Duration::from_millis(300));
        let started = Instant::now();
        client.close();
        assert!(started.elapsed() < Duration::from_secs(5), "{:?}", started.elapsed());
    }
}
