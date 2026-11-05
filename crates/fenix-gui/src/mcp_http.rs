//! `fenix --mcp --http <address>`: the MCP bridge over Streamable HTTP,
//! for MCP clients that only speak HTTP or run somewhere stdio can't
//! reach. The same `mcp::handle` answers; this is only its transport.
//!
//! - It listens on loopback only, and checks `Origin` (a page in a
//!   browser can't reach it, which the MCP spec asks for against DNS
//!   rebinding).
//! - Every request carries `Authorization: Bearer <token>`: the token in
//!   Fenix's data folder (`mcp-token`, readable only by you), which
//!   `fenix mcp token` prints.
//! - `POST /mcp` takes one JSON-RPC message and answers it as JSON;
//!   `initialize` gives an `Mcp-Session-Id`. `GET /mcp` with that id is
//!   the session's stream of notifications (Server-Sent Events).
//! - Each session is a client Fenix didn't start, named by its
//!   `clientInfo`: the `client:` rules apply to it (`agent_tools::decide`).

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

/// The largest body a request may have.
const MAX_BODY: usize = 4 * 1024 * 1024;

/// A request as far as this server reads one.
#[derive(Debug, Default, PartialEq)]
pub struct Request {
    pub method: String,
    pub path: String,
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
}

/// Reads one HTTP/1.1 request; `None` when the connection closed.
pub fn read_request(reader: &mut impl BufRead) -> Result<Option<Request>, String> {
    let mut line = String::new();
    if reader.read_line(&mut line).map_err(|e| e.to_string())? == 0 {
        return Ok(None);
    }
    let mut parts = line.split_whitespace();
    let (Some(method), Some(path)) = (parts.next(), parts.next()) else { return Err("not an HTTP request".into()) };
    let mut request = Request { method: method.to_string(), path: path.to_string(), ..Default::default() };
    loop {
        let mut header = String::new();
        reader.read_line(&mut header).map_err(|e| e.to_string())?;
        let header = header.trim_end();
        if header.is_empty() {
            break;
        }
        if request.headers.len() > 100 {
            return Err("too many headers".into());
        }
        if let Some((name, value)) = header.split_once(':') {
            request.headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
        }
    }
    let length: usize = request.headers.get("content-length").and_then(|l| l.parse().ok()).unwrap_or(0);
    if length > MAX_BODY {
        return Err("the body is too big".into());
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body).map_err(|e| e.to_string())?;
    request.body = body;
    Ok(Some(request))
}

/// Whether `origin` (the `Origin` header) may call: none (not a
/// browser), or a page on this machine.
pub fn origin_allowed(origin: Option<&str>) -> bool {
    match origin {
        None => true,
        Some(origin) => {
            let host = origin.split("://").nth(1).unwrap_or("").split('/').next().unwrap_or("");
            let host = host.rsplit_once(':').map(|(h, port)| if port.chars().all(|c| c.is_ascii_digit()) { h } else { host }).unwrap_or(host);
            matches!(host, "localhost" | "127.0.0.1" | "[::1]")
        }
    }
}

/// Whether the request carries `token`.
pub fn authorized(request: &Request, token: &str) -> bool {
    let given = request.headers.get("authorization").and_then(|h| h.strip_prefix("Bearer ")).unwrap_or("");
    // Compared in full whatever matches first: no early way out.
    given.len() == token.len() && given.bytes().zip(token.bytes()).fold(0u8, |acc, (a, b)| acc | (a ^ b)) == 0
}

/// The token clients present, made the first time: in Fenix's data
/// folder, readable only by you.
pub fn token() -> Result<String, String> {
    let path = fenix_storage::paths::data_file("mcp-token").ok_or("no data folder for Fenix")?;
    if let Ok(text) = std::fs::read_to_string(&path) {
        let text = text.trim().to_string();
        if text.len() >= 32 {
            return Ok(text);
        }
    }
    let token = fresh_token();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    fenix_storage::write(&path, token.as_bytes()).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(token)
}

/// 256 bits from the OS's randomness, as hex.
fn fresh_token() -> String {
    #[cfg(unix)]
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        use std::io::Read;
        let mut bytes = [0u8; 32];
        if f.read_exact(&mut bytes).is_ok() {
            return bytes.iter().map(|b| format!("{b:02x}")).collect();
        }
    }
    // Each `RandomState` is seeded from the OS.
    use std::hash::{BuildHasher, Hasher};
    (0..4)
        .map(|i| {
            let mut h = std::collections::hash_map::RandomState::new().build_hasher();
            h.write_u64(i);
            h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
            format!("{:016x}", h.finish())
        })
        .collect()
}

fn respond(stream: &mut TcpStream, status: &str, headers: &[(&str, String)], body: &[u8]) {
    let mut head = format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\n", body.len());
    for (name, value) in headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("\r\n");
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body);
    let _ = stream.flush();
}

/// One HTTP session: its MCP session and its notification streams.
struct Session {
    mcp: crate::mcp::Session,
    client: String,
    streams: Vec<TcpStream>,
    /// When the client last asked for anything.
    seen: std::time::Instant,
}

type Sessions = Arc<Mutex<HashMap<String, Session>>>;

/// The sessions of the server this process runs, for the modeline.
static SERVED: std::sync::OnceLock<Sessions> = std::sync::OnceLock::new();

/// How long a client that keeps no stream open counts as connected after
/// its last request.
const LINGER: std::time::Duration = std::time::Duration::from_secs(300);

/// The names of the clients connected over HTTP to the server this
/// process runs: those with a notification stream open, or that asked for
/// something lately. Each name once.
pub fn connected_clients() -> Vec<String> {
    let Some(sessions) = SERVED.get() else { return Vec::new() };
    let Ok(sessions) = sessions.lock() else { return Vec::new() };
    live(&sessions, std::time::Instant::now())
}

fn live(sessions: &HashMap<String, Session>, now: std::time::Instant) -> Vec<String> {
    let mut names: Vec<String> = sessions.values().filter(|s| !s.streams.is_empty() || now.duration_since(s.seen) < LINGER).map(|s| s.client.clone()).collect();
    names.sort();
    names.dedup();
    names
}

/// Serves MCP over HTTP at `address` (loopback only) until the process
/// ends.
pub fn serve(address: &str) -> Result<(), String> {
    let address: SocketAddr = address.parse().map_err(|e| format!("{address}: {e}"))?;
    if !address.ip().is_loopback() {
        return Err(format!("{address} isn't a loopback address: Fenix's MCP server only listens on this machine"));
    }
    let token = token()?;
    let listener = TcpListener::bind(address).map_err(|e| format!("{address}: {e}"))?;
    eprintln!("fenix: MCP over HTTP at http://{}/mcp (token: fenix mcp token)", listener.local_addr().map(|a| a.to_string()).unwrap_or_default());
    let sessions: Sessions = Arc::clone(SERVED.get_or_init(|| Arc::new(Mutex::new(HashMap::new()))));
    for stream in listener.incoming().flatten() {
        let (token, sessions) = (token.clone(), Arc::clone(&sessions));
        std::thread::spawn(move || connection(stream, &token, &sessions));
    }
    Ok(())
}

fn connection(stream: TcpStream, token: &str, sessions: &Sessions) {
    let Ok(read) = stream.try_clone() else { return };
    let mut reader = BufReader::new(read);
    let mut stream = stream;
    loop {
        let request = match read_request(&mut reader) {
            Ok(Some(r)) => r,
            Ok(None) => return,
            Err(why) => {
                respond(&mut stream, "400 Bad Request", &[], why.as_bytes());
                return;
            }
        };
        if !origin_allowed(request.headers.get("origin").map(String::as_str)) {
            respond(&mut stream, "403 Forbidden", &[], b"not from a page on another site");
            return;
        }
        if !authorized(&request, token) {
            respond(&mut stream, "401 Unauthorized", &[("WWW-Authenticate", "Bearer".into())], b"Authorization: Bearer <the token fenix mcp token prints>");
            continue;
        }
        if request.path.split('?').next() != Some("/mcp") {
            respond(&mut stream, "404 Not Found", &[], b"MCP is at /mcp");
            continue;
        }
        let id = request.headers.get("mcp-session-id").cloned();
        match request.method.as_str() {
            "POST" => post(&mut stream, request, id, sessions),
            "GET" => {
                // The session's notifications, for as long as it's open.
                let Some(id) = id.filter(|id| sessions.lock().unwrap().contains_key(id)) else {
                    respond(&mut stream, "404 Not Found", &[], b"no such session");
                    continue;
                };
                let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\n\r\n";
                if stream.write_all(head.as_bytes()).is_ok() {
                    if let (Ok(kept), Some(session)) = (stream.try_clone(), sessions.lock().unwrap().get_mut(&id)) {
                        session.streams.push(kept);
                        session.seen = std::time::Instant::now();
                    }
                }
                return;
            }
            "DELETE" => {
                if let Some(id) = id {
                    sessions.lock().unwrap().remove(&id);
                }
                respond(&mut stream, "200 OK", &[], b"");
            }
            _ => respond(&mut stream, "405 Method Not Allowed", &[("Allow", "GET, POST, DELETE".into())], b""),
        }
    }
}

fn post(stream: &mut TcpStream, request: Request, id: Option<String>, sessions: &Sessions) {
    let Ok(message) = serde_json::from_slice::<Value>(&request.body) else {
        respond(stream, "400 Bad Request", &[], b"not JSON");
        return;
    };
    let initialize = message.get("method").and_then(Value::as_str) == Some("initialize");
    let id = match (id, initialize) {
        (Some(id), _) if sessions.lock().unwrap().contains_key(&id) => id,
        (_, true) => {
            let id = fresh_token()[..32].to_string();
            let client = message.pointer("/params/clientInfo/name").and_then(Value::as_str).unwrap_or("mcp-http").to_string();
            sessions.lock().unwrap().insert(id.clone(), Session { mcp: crate::mcp::Session::default(), client, streams: Vec::new(), seen: std::time::Instant::now() });
            start_watching(id.clone(), Arc::clone(sessions));
            id
        }
        _ => {
            respond(stream, "404 Not Found", &[], b"no such session: initialize first");
            return;
        }
    };
    let client = sessions.lock().unwrap().get_mut(&id).map(|s| {
        s.seen = std::time::Instant::now();
        s.client.clone()
    }).unwrap_or_default();
    let mut session = sessions.lock().unwrap().get(&id).map(|s| crate::mcp::Session { protocol: s.mcp.protocol }).unwrap_or_default();
    let mut call = |method: &str, params: Value| control_call(&client, method, params);
    let output = crate::mcp::handle(&mut session, &message, &mut call);
    if initialize {
        if let Some(s) = sessions.lock().unwrap().get_mut(&id) {
            s.mcp.protocol = session.protocol;
        }
    }
    notify(sessions, &id, &output.after);
    match output.reply {
        Some(reply) => respond(stream, "200 OK", &[("Content-Type", "application/json".into()), ("Mcp-Session-Id", id)], serde_json::to_string(&reply).unwrap_or_default().as_bytes()),
        None => respond(stream, "202 Accepted", &[("Mcp-Session-Id", id)], b""),
    }
}

/// A control call for an HTTP client: a connection of its own, said
/// hello to with the client's name and no thread.
fn control_call(client: &str, method: &str, params: Value) -> Result<Value, String> {
    let mut c = fenix_control::Client::connect(&fenix_control::Endpoint::current()).map_err(|e| e.to_string())?.ok_or("Fenix isn't running")?;
    c.hello_for(client, env!("CARGO_PKG_VERSION"), None).map_err(|e| e.to_string())?;
    c.request::<Value>(method, params).map_err(|e| e.to_string())
}

/// Sends `messages` down the session's notification streams.
fn notify(sessions: &Sessions, id: &str, messages: &[Value]) {
    if messages.is_empty() {
        return;
    }
    let mut sessions = sessions.lock().unwrap();
    let Some(session) = sessions.get_mut(id) else { return };
    session.streams.retain_mut(|stream| messages.iter().all(|m| stream.write_all(format!("event: message\ndata: {m}\n\n").as_bytes()).is_ok()));
}

/// Fenix's news for the session, waited on while it lasts.
fn start_watching(id: String, sessions: Sessions) {
    std::thread::spawn(move || {
        let mut since = 0u64;
        loop {
            let Some(client) = sessions.lock().unwrap().get(&id).map(|s| s.client.clone()) else { return };
            match control_call(&client, fenix_control::protocol::method::AGENT_WAIT, json!({ "since": since })) {
                Ok(news) => {
                    since = news.get("revision").and_then(Value::as_u64).unwrap_or(since);
                    let events = news.get("events").and_then(Value::as_array).cloned().unwrap_or_default();
                    notify(&sessions, &id, &crate::mcp::notifications_for(&events));
                }
                Err(_) => std::thread::sleep(std::time::Duration::from_secs(2)),
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_client_counts_as_connected_while_its_stream_is_open_or_it_asked_lately() {
        let now = std::time::Instant::now();
        let session = |client: &str, ago: u64| Session { mcp: crate::mcp::Session::default(), client: client.into(), streams: Vec::new(), seen: now - std::time::Duration::from_secs(ago) };
        let sessions: HashMap<String, Session> = [("a".into(), session("cursor", 10)), ("b".into(), session("cursor", 20)), ("c".into(), session("old", 3600))].into_iter().collect();
        assert_eq!(live(&sessions, now), vec!["cursor".to_string()]);
    }

    #[test]
    fn a_request_is_read_with_its_headers_and_body() {
        let raw = b"POST /mcp HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer abc\r\nContent-Length: 2\r\n\r\n{}";
        let request = read_request(&mut &raw[..]).unwrap().unwrap();
        assert_eq!((request.method.as_str(), request.path.as_str(), request.body.as_slice()), ("POST", "/mcp", &b"{}"[..]));
        assert!(authorized(&request, "abc"));
        assert!(!authorized(&request, "abd") && !authorized(&request, "ab"));
        assert_eq!(read_request(&mut &b""[..]).unwrap(), None);
        let big = format!("POST / HTTP/1.1\r\nContent-Length: {}\r\n\r\n", MAX_BODY + 1);
        assert!(read_request(&mut big.as_bytes()).is_err());
    }

    #[test]
    fn only_this_machines_pages_may_call() {
        assert!(origin_allowed(None), "not a browser");
        assert!(origin_allowed(Some("http://localhost:3000")));
        assert!(origin_allowed(Some("http://127.0.0.1")));
        assert!(!origin_allowed(Some("https://evil.example")));
        assert!(!origin_allowed(Some("http://localhost.evil.example")));
    }

    #[test]
    fn a_fresh_token_is_long_and_never_the_same() {
        let (a, b) = (fresh_token(), fresh_token());
        assert!(a.len() >= 64 && a != b);
    }

    #[test]
    fn it_refuses_an_address_off_this_machine() {
        assert!(serve("0.0.0.0:0").unwrap_err().contains("loopback"));
    }
}
