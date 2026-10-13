//! The server's side of one connection: framing, `fenix/hello`, the
//! protocol's errors, and the legacy newline payload. What a request
//! actually does is the caller's (`fenix-gui` answers on its UI
//! thread); this module only ever hands it a checked [`Call`].
//!
//! Generic over `Read`/`Write` so the whole protocol is tested on
//! in-memory streams as well as the real endpoint.

use std::io::{self, BufReader, Read, Write};

use serde::de::DeserializeOwned;
use serde_json::{json, Value};

use crate::protocol::{
    code, method, Call, CaptureParams, HelloParams, HelloResult, OpenParams, OpenPath, RpcError,
    CAPABILITIES, MAX_BODY, MAX_HEADER_LINE, PROTOCOL,
};

/// What the server says about itself in `fenix/hello`.
#[derive(Debug, Clone)]
pub struct ServerInfo {
    pub version: String,
    pub pid: u32,
    /// The methods it answers, listed in `capabilities`. Anything else
    /// is `-32601`, even a method this crate knows, so a client that
    /// checks the list is never told one thing and served another.
    pub capabilities: Vec<String>,
}

impl ServerInfo {
    /// A server answering every method in [`CAPABILITIES`].
    pub fn new(version: impl Into<String>, pid: u32) -> Self {
        ServerInfo {
            version: version.into(),
            pid,
            capabilities: CAPABILITIES.iter().map(|c| c.to_string()).collect(),
        }
    }
}

/// The first bytes of every framed message.
const FRAMED: &[u8] = b"Content-Length:";

/// The most a legacy payload may be: thousands of paths, far more than
/// a file manager hands one launch.
const MAX_LEGACY: u64 = 1024 * 1024;

/// Serves one connection until the client closes it, answering each
/// request in turn; `handle` answers the ones that need the editor.
///
/// A connection whose first bytes aren't `Content-Length:` is a legacy
/// one, from a `fenix` older than this protocol: one path per line,
/// read to the end and handed over as a `fenix/open`, no reply. Kept
/// for one release so an older `fenix` on `PATH` still hands off.
///
/// A message the framing refuses (too big, endless headers) ends the
/// connection after an error reply: past that point the stream can't be
/// trusted to be in step.
pub fn serve<R: Read, W: Write>(
    reader: R,
    mut writer: W,
    info: &ServerInfo,
    mut handle: impl FnMut(Call) -> Result<Value, RpcError>,
) -> io::Result<()> {
    let (framed, prefix) = sniff(reader)?;
    let mut reader = BufReader::new(prefix);
    if !framed {
        let mut raw = Vec::new();
        reader.take(MAX_LEGACY).read_to_end(&mut raw)?;
        let request = parse_legacy(&String::from_utf8_lossy(&raw));
        let paths = request
            .paths
            .into_iter()
            .map(|path| OpenPath {
                path,
                line: None,
                column: None,
            })
            .collect();
        // Nobody is listening for an answer, so an error goes nowhere.
        let _ = handle(Call::Open(OpenParams {
            paths,
            new_window: request.new_window,
            focus: true,
        }));
        return Ok(());
    }
    let mut session = Session {
        info,
        greeted: false,
    };
    loop {
        let body = match fenix_rpc::read_message_limited(&mut reader, MAX_BODY, MAX_HEADER_LINE) {
            Ok(Some(body)) => body,
            Ok(None) => return Ok(()),
            Err(e) if e.kind() == io::ErrorKind::InvalidData => {
                let _ = respond(
                    &mut writer,
                    Value::Null,
                    Err(RpcError::new(code::INVALID_REQUEST, e.to_string())),
                );
                return Err(e);
            }
            Err(e) => return Err(e),
        };
        if let Some((id, reply)) = session.answer(&body, &mut handle) {
            respond(&mut writer, id, reply)?;
        }
    }
}

/// A connection's reader with the bytes `sniff` read put back in front.
type Replayed<R> = io::Chain<io::Cursor<Vec<u8>>, R>;

/// Reads just enough to tell a framed connection from a legacy one,
/// handing back a reader that still yields every byte.
fn sniff<R: Read>(mut reader: R) -> io::Result<(bool, Replayed<R>)> {
    let mut seen = Vec::with_capacity(FRAMED.len());
    let mut buf = [0u8; 15];
    while seen.len() < FRAMED.len() && FRAMED.starts_with(&seen) {
        let want = FRAMED.len() - seen.len();
        let n = match reader.read(&mut buf[..want]) {
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        if n == 0 {
            break;
        }
        seen.extend_from_slice(&buf[..n]);
    }
    let framed = seen == FRAMED;
    Ok((framed, io::Cursor::new(seen).chain(reader)))
}

struct Session<'a> {
    info: &'a ServerInfo,
    greeted: bool,
}

impl Session<'_> {
    /// The reply to one message, with the id to send it under; `None`
    /// for a notification, which JSON-RPC never answers.
    fn answer(
        &mut self,
        body: &[u8],
        handle: &mut impl FnMut(Call) -> Result<Value, RpcError>,
    ) -> Option<(Value, Result<Value, RpcError>)> {
        let message: Value = match serde_json::from_slice(body) {
            Ok(message) => message,
            Err(e) => {
                return Some((
                    Value::Null,
                    Err(RpcError::new(code::PARSE_ERROR, format!("not JSON: {e}"))),
                ))
            }
        };
        let Some(object) = message.as_object() else {
            return Some((
                Value::Null,
                Err(RpcError::new(
                    code::INVALID_REQUEST,
                    "a request is a JSON object; batches aren't supported",
                )),
            ));
        };
        let id = object.get("id").cloned();
        let method = object.get("method").and_then(Value::as_str);
        let (Some(method), true) = (method, object.get("jsonrpc") == Some(&json!("2.0"))) else {
            return Some((
                id.unwrap_or(Value::Null),
                Err(RpcError::new(
                    code::INVALID_REQUEST,
                    "not a JSON-RPC 2.0 request",
                )),
            ));
        };
        let params = object.get("params").cloned().unwrap_or(Value::Null);
        let reply = self.call(method, params, handle);
        // Without an id it's a notification: done all the same, as
        // JSON-RPC says, but never answered.
        Some((id?, reply))
    }

    fn call(
        &mut self,
        method: &str,
        params: Value,
        handle: &mut impl FnMut(Call) -> Result<Value, RpcError>,
    ) -> Result<Value, RpcError> {
        if method == method::HELLO {
            return self.hello(params);
        }
        if !self.greeted {
            return Err(RpcError::new(
                code::HELLO_FIRST,
                format!("send {} first", method::HELLO),
            ));
        }
        if !self.info.capabilities.iter().any(|c| c == method) {
            return Err(RpcError::new(
                code::METHOD_NOT_FOUND,
                format!("no method {method}"),
            ));
        }
        let call = match method {
            method::OPEN => Call::Open(params_of(params)?),
            method::OPEN_PROJECT => Call::OpenProject(params_of(params)?),
            method::STATE => Call::State,
            method::COMMANDS => Call::Commands,
            method::RUN_COMMAND => Call::RunCommand(params_of(params)?),
            method::NOTEBOOK_CAPTURE => {
                let capture: CaptureParams = params_of(params)?;
                if capture.text.trim().is_empty() {
                    return Err(RpcError::invalid_params(
                        "nothing to capture: text is empty",
                    ));
                }
                Call::NotebookCapture(capture)
            }
            method::AGENDA_TASKS => Call::AgendaTasks(params_of(params)?),
            method::AGENDA_CLOCK_IN => Call::AgendaClockIn(params_of(params)?),
            method::AGENDA_CLOCK_OUT => Call::AgendaClockOut,
            // Listed in the server's capabilities but unknown here: the
            // caller asked for more than this crate implements.
            _ => {
                return Err(RpcError::new(
                    code::METHOD_NOT_FOUND,
                    format!("no method {method}"),
                ))
            }
        };
        handle(call)
    }

    fn hello(&mut self, params: Value) -> Result<Value, RpcError> {
        let hello: HelloParams = params_of(params)?;
        if hello.protocol != PROTOCOL {
            let mut error = RpcError::new(
                code::UNSUPPORTED_PROTOCOL,
                format!("protocol {} isn't supported", hello.protocol),
            );
            error.data = Some(json!({ "supported": [PROTOCOL] }));
            return Err(error);
        }
        self.greeted = true;
        let result = HelloResult {
            protocol: PROTOCOL,
            version: self.info.version.clone(),
            pid: self.info.pid,
            capabilities: self.info.capabilities.clone(),
        };
        Ok(serde_json::to_value(result).expect("plain data serializes"))
    }
}

/// `params` as `T`, or an invalid-params error saying why. A missing
/// `params` reads as `{}`, so a method whose fields are all optional
/// can be called without one.
fn params_of<T: DeserializeOwned>(params: Value) -> Result<T, RpcError> {
    let params = if params.is_null() { json!({}) } else { params };
    serde_json::from_value(params).map_err(|e| RpcError::invalid_params(e.to_string()))
}

fn respond(writer: &mut impl Write, id: Value, reply: Result<Value, RpcError>) -> io::Result<()> {
    let message = match reply {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err(error) => json!({ "jsonrpc": "2.0", "id": id, "error": error }),
    };
    fenix_rpc::write_message(
        writer,
        &serde_json::to_vec(&message).expect("plain data serializes"),
    )
}

/// What a legacy payload asks for.
#[derive(Debug, PartialEq)]
pub struct LegacyRequest {
    /// Whether the launch passed `--new-window`.
    pub new_window: bool,
    pub paths: Vec<String>,
}

/// Splits a legacy payload, one argument per line. Blank lines are
/// dropped, so an empty payload (a bare relaunch, meant to bring Fenix
/// forward) asks for no files rather than one blank one.
///
/// Anything else beginning with `-` is dropped too rather than taken for
/// a filename: an unrecognized flag turning into an attempt to open a
/// file called `--colour` is worse than ignoring it, and a real path
/// starting with a dash can still be passed as `./-weird.txt`.
pub fn parse_legacy(raw: &str) -> LegacyRequest {
    let mut request = LegacyRequest {
        new_window: false,
        paths: Vec::new(),
    };
    for line in raw.lines().filter(|line| !line.is_empty()) {
        if line == "--new-window" {
            request.new_window = true;
        } else if !line.starts_with('-') {
            request.paths.push(line.to_string());
        }
    }
    request
}
