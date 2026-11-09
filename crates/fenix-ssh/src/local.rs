//! A listener on this machine only, for the two small programs that
//! talk back to Fenix: the askpass hook and the stand-in.
//!
//! Spike: a loopback TCP port and a token. Fenix's control pipe can't
//! carry a program's stdin and stdout at once (one synchronous handle,
//! so a waiting read holds up every write), so the real thing wants an
//! overlapped pipe, or a Unix socket, of its own.

use std::hash::{BuildHasher, Hasher};
use std::io;
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::thread;

/// Where a listener is, and what a caller has to say to be heard.
#[derive(Debug, Clone)]
pub struct Door {
    pub addr: SocketAddr,
    pub token: String,
}

/// Listens on a loopback port; `serve` gets each connection on a thread
/// of its own. Runs until the process ends.
pub(crate) fn listen(name: &str, serve: impl Fn(TcpStream, &str) + Send + Sync + 'static) -> io::Result<Door> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    let door = Door { addr: listener.local_addr()?, token: token() };
    let token = door.token.clone();
    let serve = std::sync::Arc::new(serve);
    thread::Builder::new().name(name.into()).spawn(move || {
        for stream in listener.incoming().flatten() {
            let _ = stream.set_nodelay(true);
            let (serve, token) = (serve.clone(), token.clone());
            thread::spawn(move || serve(stream, &token));
        }
    })?;
    Ok(door)
}

pub(crate) fn knock(addr: &str) -> io::Result<TcpStream> {
    let stream = TcpStream::connect(addr)?;
    stream.set_nodelay(true)?;
    Ok(stream)
}

/// 128 bits from the standard library's own randomly keyed hasher.
fn token() -> String {
    let word = || std::collections::hash_map::RandomState::new().build_hasher().finish();
    format!("{:016x}{:016x}", word(), word())
}
