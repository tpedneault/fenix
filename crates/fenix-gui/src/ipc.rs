//! Single-instance IPC: makes a second `fenix` launch (Windows
//! Explorer's "Open With" on a double-clicked file, or just blindly
//! relaunching `fenix.exe` from a shortcut) hand its file arguments off
//! to whichever instance is already running instead of opening a
//! second window -- the same "one real process, every later launch is
//! a thin client" shape `emacsclient`/`emacs --daemon` has.
//!
//! Deliberately a bare loopback TCP socket on a fixed port, not a
//! platform IPC primitive (a Windows named pipe, a Unix domain socket)
//! -- `std::net` needs no new dependency, and this only ever needs to
//! talk to itself on the same machine. The tradeoff: if some unrelated
//! process happens to already be bound to this exact port, this
//! instance can't tell that apart from "another Fenix is running" and
//! just falls back to running standalone (see `negotiate`'s own doc
//! comment) -- disclosed, not fixed, since it needs an unlikely
//! coincidence to ever matter in practice.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::time::Duration;

use winit::event_loop::EventLoopProxy;

use crate::app::FenixUserEvent;

/// An arbitrary, uncommon high port -- nothing else on a typical
/// developer machine has a reason to bind this exact number. Loopback
/// only (`127.0.0.1`), never exposed off-machine.
const IPC_PORT: u16 = 47821;

/// How long a client (the *second* `fenix` invocation) waits for the
/// existing instance to accept the connection before giving up and
/// falling back to a standalone launch instead of hanging Explorer's
/// double-click indefinitely.
const CONNECT_TIMEOUT: Duration = Duration::from_millis(300);

/// What this launch turned out to be, once single-instance negotiation
/// settles.
pub enum Role {
    /// The first/only instance -- bound the port, and owns it for as
    /// long as this process runs. Pass the listener to `spawn_accept_
    /// loop`.
    Server(TcpListener),
    /// Another instance was already listening and accepted this
    /// launch's file list -- this process should exit immediately
    /// without opening a window.
    HandedOff,
    /// Couldn't bind the port (something else has it) *and* couldn't
    /// hand off to it either (it didn't answer, or wasn't really a
    /// Fenix instance) -- proceed as an ordinary standalone launch,
    /// just without serving IPC to any future launch.
    Standalone,
}

/// Binds the IPC port if nobody else has it yet; otherwise tries to
/// hand `args` (the file paths this launch was given, possibly empty)
/// off to whoever does.
pub fn negotiate(args: &[String]) -> Role {
    negotiate_at(SocketAddr::from(([127, 0, 0, 1], IPC_PORT)), args)
}

fn negotiate_at(addr: SocketAddr, args: &[String]) -> Role {
    match TcpListener::bind(addr) {
        Ok(listener) => Role::Server(listener),
        Err(_) => {
            if send_to(addr, args) {
                Role::HandedOff
            } else {
                Role::Standalone
            }
        }
    }
}

/// One line per path, then closes the connection -- the server reads
/// to EOF and splits on `\n` (see `parse_paths`). `false` on any
/// failure (nothing listening, or it stopped listening mid-connect),
/// which `negotiate_at` treats as "there wasn't really a live instance
/// after all."
fn send_to(addr: SocketAddr, args: &[String]) -> bool {
    let Ok(mut stream) = TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT) else { return false };
    let mut payload = String::new();
    for arg in args {
        payload.push_str(arg);
        payload.push('\n');
    }
    stream.write_all(payload.as_bytes()).is_ok()
}

/// Splits a received payload into its file paths -- blank lines
/// dropped, so an empty payload (a bare relaunch with no file
/// arguments, just meant to focus the existing window) yields an empty
/// list rather than one blank entry.
fn parse_paths(raw: &str) -> Vec<String> {
    raw.lines().map(str::to_string).filter(|l| !l.is_empty()).collect()
}

/// Spawns the background thread that accepts hand-offs from later
/// `fenix` launches for as long as this process (the server) runs.
/// Each connection is read to completion and delivered to the running
/// app as `FenixUserEvent::OpenFiles` -- the same cross-thread wake-
/// and-hand-data-over shape every other background thread in this app
/// already uses (`GitSession`'s poller, `jira_sync_issues`, the
/// terminal's async spawn, ...). Deliberately thin and not unit tested
/// on its own (spawning a real thread and blocking on real accepted
/// sockets isn't something a unit test should do) -- `parse_paths` and
/// `negotiate_at` carry the logic that's actually worth pinning down.
pub fn spawn_accept_loop(listener: TcpListener, proxy: EventLoopProxy<FenixUserEvent>) {
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut buf = String::new();
            if stream.read_to_string(&mut buf).is_err() {
                continue;
            }
            let _ = proxy.send_event(FenixUserEvent::OpenFiles(parse_paths(&buf)));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    /// Binds an ephemeral port (`:0`) and immediately frees it --
    /// yields an address nothing is listening on, without the
    /// collision risk a fixed port would have across parallel test
    /// threads or a real Fenix instance running on this machine.
    fn free_addr() -> SocketAddr {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        listener.local_addr().unwrap()
    }

    #[test]
    fn parse_paths_splits_on_newlines_and_drops_blank_lines() {
        assert_eq!(parse_paths("a\nb\n\nc\n"), vec!["a", "b", "c"]);
    }

    #[test]
    fn parse_paths_of_an_empty_payload_is_an_empty_list() {
        assert!(parse_paths("").is_empty());
    }

    #[test]
    fn send_to_returns_false_when_nothing_is_listening() {
        assert!(!send_to(free_addr(), &["a.txt".to_string()]));
    }

    #[test]
    fn negotiate_at_becomes_the_server_when_the_port_is_free() {
        assert!(matches!(negotiate_at(free_addr(), &[]), Role::Server(_)));
    }

    #[test]
    fn negotiate_at_hands_off_to_an_existing_listener() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = String::new();
            stream.read_to_string(&mut buf).unwrap();
            tx.send(buf).unwrap();
        });

        let role = negotiate_at(addr, &["C:\\file.tcl".to_string(), "C:\\other.tcl".to_string()]);
        assert!(matches!(role, Role::HandedOff));

        let received = rx.recv_timeout(Duration::from_secs(2)).expect("the accept thread should have received the payload");
        assert_eq!(parse_paths(&received), vec!["C:\\file.tcl", "C:\\other.tcl"]);
    }
}
