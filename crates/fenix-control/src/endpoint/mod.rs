//! The control endpoint: a channel only this user, in this login
//! session, can reach. The first Fenix listens on it; a later `fenix`
//! launch, or any tool the user runs, connects to it.
//!
//! Windows: a named pipe, `\\.\pipe\fenix-<SID>-<session>`, created with
//! a DACL granting that SID alone, `FILE_FLAG_FIRST_PIPE_INSTANCE` (so
//! nothing that created the name first can receive our clients'
//! requests) and `PIPE_REJECT_REMOTE_CLIENTS`. The session is in the name
//! because pipe names are machine-wide: the same user logged in twice on
//! a Remote Desktop host gets two editors, each opening files in its own
//! session.
//!
//! Linux: a Unix socket, `$XDG_RUNTIME_DIR/fenix/control.sock`, or
//! `/tmp/fenix-$UID/control.sock` when there is no runtime directory,
//! in a directory created `0700` and checked to be ours before use. A
//! lock file beside the socket, held for the server's life, decides who
//! the server is, so a socket left behind by a crash is replaced safely
//! and two launches starting together can't both bind.
//!
//! With `FENIX_HOME` set, the name gains `-<16 hex digits>` (a hash of
//! that folder), so a portable or test Fenix runs beside the installed
//! one instead of handing its files to it.
//!
//! These names are a contract: other tools build them the same way to
//! find Fenix (see `docs/CONTROL.md`).
//!
//! It replaces a loopback TCP port, which was one port for the whole
//! machine: on a shared machine the first user's Fenix received every
//! other user's `fenix somefile`, and any local process could make it
//! open a path.

use std::ffi::OsStr;
use std::fmt;
use std::io;

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

#[cfg(unix)]
use self::unix as platform;
#[cfg(windows)]
use self::windows as platform;

pub use self::platform::{Listener, Stream};

/// Why a Fenix couldn't take the endpoint.
#[derive(Debug)]
pub enum BindError {
    /// Another Fenix has it, or is taking it right now.
    InUse,
    /// Anything else: this Fenix runs without serving the endpoint.
    Io(io::Error),
}

impl fmt::Display for BindError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BindError::InUse => f.write_str("another Fenix is serving the control endpoint"),
            BindError::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for BindError {}

impl From<io::Error> for BindError {
    fn from(e: io::Error) -> Self {
        BindError::Io(e)
    }
}

/// Which endpoint: this user's, or a separate one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    /// Appended to the endpoint's name after a `-`; empty for the
    /// user's own.
    instance: String,
}

impl Endpoint {
    /// The endpoint for this process's environment: its `FENIX_HOME`,
    /// if any.
    pub fn current() -> Self {
        Endpoint::for_home(std::env::var_os("FENIX_HOME").as_deref())
    }

    /// This user's endpoint for a Fenix whose `FENIX_HOME` is
    /// `fenix_home` (`None`, or empty, for an ordinary install).
    pub fn for_home(fenix_home: Option<&OsStr>) -> Self {
        let instance = match fenix_home.filter(|h| !h.is_empty()) {
            Some(home) => format!("{:016x}", fnv1a(home.as_encoded_bytes())),
            None => String::new(),
        };
        Endpoint { instance }
    }

    /// A separate endpoint named by `instance`, for tests.
    pub fn instance(instance: &str) -> Self {
        Endpoint {
            instance: instance.to_owned(),
        }
    }

    /// Takes the endpoint and starts listening.
    pub fn bind(&self) -> Result<Listener, BindError> {
        Listener::bind(self)
    }

    /// Connects to the Fenix serving this endpoint; `None` when there
    /// isn't one.
    pub fn connect(&self) -> io::Result<Option<Stream>> {
        Stream::connect(self)
    }

    /// Where the endpoint lives: the pipe's name on Windows, the
    /// socket's path elsewhere. Creates the private directory on Linux,
    /// as connecting would.
    pub fn address(&self) -> io::Result<String> {
        platform::address(self)
    }

    fn suffix(&self) -> String {
        if self.instance.is_empty() {
            String::new()
        } else {
            format!("-{}", self.instance)
        }
    }
}

/// FNV-1a, 64-bit: a stable, dependency-free hash for naming an endpoint
/// after a folder, simple enough for any other tool to reproduce. Not
/// for security; the endpoint's access control is.
pub fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, &b| {
        (hash ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::sync::atomic::{AtomicU32, Ordering};

    pub(super) fn unique() -> Endpoint {
        static N: AtomicU32 = AtomicU32::new(0);
        Endpoint::instance(&format!(
            "test-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[test]
    fn fnv1a_matches_the_published_test_vectors() {
        assert_eq!(fnv1a(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(fnv1a(b"foobar"), 0x8594_4171_f739_67e8);
    }

    #[test]
    fn a_fenix_home_gets_its_own_endpoint_named_by_its_hash() {
        let home = Endpoint::for_home(Some(OsStr::new("foobar")));
        assert_eq!(home.suffix(), "-85944171f73967e8");
        assert_ne!(home, Endpoint::for_home(Some(OsStr::new("/b"))));
        assert_eq!(
            Endpoint::for_home(Some(OsStr::new(""))),
            Endpoint::for_home(None)
        );
        assert_eq!(Endpoint::for_home(None).suffix(), "");
    }

    #[test]
    fn nobody_listening_is_none() {
        assert!(unique().connect().unwrap().is_none());
    }

    #[test]
    fn a_second_bind_is_in_use() {
        let endpoint = unique();
        let _first = endpoint.bind().unwrap();
        assert!(matches!(endpoint.bind(), Err(BindError::InUse)));
    }

    #[test]
    fn the_endpoint_is_free_again_after_the_listener_goes() {
        let endpoint = unique();
        drop(endpoint.bind().unwrap());
        drop(endpoint.bind().unwrap());
    }

    #[test]
    fn clients_and_the_listener_talk_both_ways() {
        let endpoint = unique();
        let mut listener = endpoint.bind().unwrap();
        let server = std::thread::spawn(move || {
            for _ in 0..2 {
                let stream = listener.accept().unwrap();
                let mut writer = stream.try_clone().unwrap();
                let mut line = String::new();
                BufReader::new(stream).read_line(&mut line).unwrap();
                writer.write_all(format!("echo {line}").as_bytes()).unwrap();
                writer.flush().unwrap();
            }
        });
        for word in ["one", "two"] {
            let stream = endpoint.connect().unwrap().expect("listening");
            let mut writer = stream.try_clone().unwrap();
            writer.write_all(format!("{word}\n").as_bytes()).unwrap();
            writer.flush().unwrap();
            let mut reply = String::new();
            BufReader::new(stream).read_line(&mut reply).unwrap();
            assert_eq!(reply, format!("echo {word}\n"));
        }
        server.join().unwrap();
    }
}
