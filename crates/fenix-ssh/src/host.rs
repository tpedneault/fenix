//! A remote machine Fenix stays connected to.
//!
//! A [`Host`] owns the connection: it connects, mounts the machine's
//! files under its remote paths (`fenix-vfs`), and when the connection
//! is lost, connects again by itself. While it's away every request
//! fails at once with "not connected"; nothing waits on a machine that
//! may not come back.

use std::io;
use std::process::Command;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, Weak};
use std::thread;
use std::time::Duration;

use fenix_remote::{Entry, FailureCode, FileKind};
use fenix_remote::Exec;
use fenix_vfs::{Kind, Meta, Over, Ran, Run};

use crate::askpass::Askpass;
use crate::bridge::Bridge;
use crate::session::{Error, Session};
use crate::ssh::{connect, ConnectError, Helpers, Target};

/// How often the connection is looked at.
const WATCH: Duration = Duration::from_millis(250);

/// The waits between tries to connect again; the last one repeats.
const BACKOFF: [u64; 6] = [1, 2, 4, 8, 15, 30];

/// How a host's connection is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Connected,
    /// Lost, and being tried again: which try this is, and why the last
    /// one failed.
    Reconnecting { attempt: u32, why: String },
    /// Lost for a reason trying again won't mend (a sign-in refused):
    /// waiting for [`Host::reconnect`].
    Down(String),
}

struct State {
    session: Option<Arc<Session>>,
    status: Status,
    /// The user's home there, as the helper last said.
    home: String,
}

type Watcher = Box<dyn Fn(&Status) + Send + Sync>;

/// A remote machine, connected or trying to be.
pub struct Host {
    name: String,
    target: Target,
    helpers: Helpers,
    askpass: Askpass,
    state: Mutex<State>,
    watchers: Mutex<Vec<Watcher>>,
    bridge: Mutex<Option<Bridge>>,
    /// Wakes the watching thread: `true` to try now, `false` to end.
    wake: Mutex<Sender<bool>>,
}

impl Host {
    /// Connects to `target` and mounts its files under `name` (see
    /// `fenix_vfs::remote`). `ask` answers what `ssh` asks; an answer is
    /// asked for once and remembered for the host's life.
    pub fn connect(name: &str, target: Target, helpers: Helpers, ask: impl Fn(&str) -> Option<String> + Send + Sync + 'static) -> Result<Arc<Host>, ConnectError> {
        // A name that starts with `-` would be read by `ssh` as an option.
        if !fenix_vfs::path::host_is_valid(name) || name.starts_with('-') || target.host.starts_with('-') {
            return Err(ConnectError::Io(io::Error::new(io::ErrorKind::InvalidInput, format!("{name:?} can't name a host"))));
        }
        let askpass = Askpass::start(ask)?;
        let session = Arc::new(connect(&target, &helpers, Some(&askpass))?.session);
        let (wake, woken) = mpsc::channel();
        let host = Arc::new(Host {
            name: name.to_string(),
            target,
            helpers,
            askpass,
            state: Mutex::new(State { home: session.hello().home.clone(), session: Some(session), status: Status::Connected }),
            watchers: Mutex::default(),
            bridge: Mutex::new(None),
            wake: Mutex::new(wake),
        });
        fenix_vfs::mount(name, host.clone());
        let watched = Arc::downgrade(&host);
        thread::Builder::new().name(format!("fenix-ssh-{name}")).spawn(move || watch(&watched, &woken))?;
        Ok(host)
    }

    /// The name its files are mounted under.
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn status(&self) -> Status {
        self.state.lock().unwrap().status.clone()
    }

    /// Calls `watcher` with each change of status, from another thread.
    pub fn on_status(&self, watcher: impl Fn(&Status) + Send + Sync + 'static) {
        self.watchers.lock().unwrap().push(Box::new(watcher));
    }

    /// The connection, if there is one right now.
    pub fn session(&self) -> Result<Arc<Session>, Error> {
        let state = self.state.lock().unwrap();
        state.session.clone().filter(|session| !session.lost()).ok_or(Error::Lost)
    }

    /// Tries to connect now, whatever the wait or the reason it's down.
    pub fn reconnect(&self) {
        let _ = self.wake.lock().unwrap().send(true);
    }

    /// A `Command` that runs `program` on this machine, in `cwd` there,
    /// through a stand-in process. Its stdin, stdout, stderr and exit
    /// code are the program's; `bridge::env` sets its variables.
    pub fn command(self: &Arc<Self>, program: &str, cwd: Option<&str>) -> io::Result<Command> {
        let mut bridge = self.bridge.lock().unwrap();
        if bridge.is_none() {
            let host = Arc::downgrade(self);
            *bridge = Some(Bridge::start(move || host.upgrade().ok_or(Error::Lost)?.session())?);
        }
        bridge.as_ref().expect("just started").command(program, cwd)
    }

    /// Ends the connection and takes the machine's files away. Programs
    /// running there end with it.
    pub fn close(&self) {
        let _ = self.wake.lock().unwrap().send(false);
        fenix_vfs::unmount(&self.name);
        self.state.lock().unwrap().session = None;
        self.askpass.forget();
    }

    fn set(&self, session: Option<Arc<Session>>, status: Status) {
        {
            let mut state = self.state.lock().unwrap();
            if let Some(session) = &session {
                state.home = session.hello().home.clone();
            }
            state.session = session;
            if state.status == status {
                return;
            }
            state.status = status.clone();
        }
        for watcher in self.watchers.lock().unwrap().iter() {
            watcher(&status);
        }
    }
}

/// Watches a host's connection and mends it, until the host is closed or
/// dropped.
fn watch(host: &Weak<Host>, woken: &mpsc::Receiver<bool>) {
    let mut attempt = 0u32;
    let mut wait = WATCH;
    loop {
        let now = match woken.recv_timeout(wait) {
            Ok(true) => true,
            Ok(false) | Err(RecvTimeoutError::Disconnected) => return,
            Err(RecvTimeoutError::Timeout) => false,
        };
        let Some(host) = host.upgrade() else { return };
        let status = host.status();
        let lost = host.state.lock().unwrap().session.as_ref().is_none_or(|session| session.lost());
        if !lost {
            wait = WATCH;
            continue;
        }
        if matches!(status, Status::Down(_)) && !now {
            continue;
        }
        if now {
            attempt = 0;
        }
        attempt += 1;
        if status == Status::Connected {
            host.set(None, Status::Reconnecting { attempt, why: "the connection was lost".into() });
        }
        match connect(&host.target, &host.helpers, Some(&host.askpass)) {
            Ok(connected) => {
                attempt = 0;
                wait = WATCH;
                host.set(Some(Arc::new(connected.session)), Status::Connected);
            }
            Err(error) if error.worth_retrying() => {
                wait = Duration::from_secs(BACKOFF[(attempt as usize - 1).min(BACKOFF.len() - 1)]);
                host.set(None, Status::Reconnecting { attempt: attempt + 1, why: error.to_string() });
            }
            Err(error) => {
                // What was remembered may be what was refused.
                host.askpass.forget();
                wait = WATCH;
                host.set(None, Status::Down(error.to_string()));
            }
        }
    }
}

fn meta(entry: Entry, name: Option<&str>) -> Meta {
    let kind = match entry.kind {
        FileKind::File => Kind::File,
        FileKind::Dir => Kind::Dir,
        FileKind::Other => Kind::Other,
    };
    // A listing names its entries; anything else was asked by its path.
    let name = match name {
        Some(path) => path.trim_end_matches('/').rsplit('/').next().unwrap_or_default().to_string(),
        None => entry.name,
    };
    Meta { name, kind, symlink: entry.symlink, len: entry.len, version: fenix_vfs::Version { len: entry.len, mtime: entry.mtime }, mode: entry.mode }
}

fn io_error(error: Error) -> io::Error {
    match error {
        Error::Lost => io::Error::new(io::ErrorKind::NotConnected, "the connection to the remote machine was lost"),
        Error::TimedOut => io::Error::new(io::ErrorKind::TimedOut, "the remote machine didn't answer in time"),
        Error::Cancelled => io::Error::new(io::ErrorKind::Interrupted, "stopped"),
        Error::Protocol(what) => io::Error::other(format!("unexpected from Fenix's helper: {what}")),
        Error::Remote(failure) => match failure.code {
            FailureCode::NotFound => io::Error::new(io::ErrorKind::NotFound, failure.message),
            FailureCode::Denied => io::Error::new(io::ErrorKind::PermissionDenied, failure.message),
            FailureCode::Exists => io::Error::new(io::ErrorKind::AlreadyExists, failure.message),
            FailureCode::Cancelled => io::Error::new(io::ErrorKind::Interrupted, failure.message),
            FailureCode::Changed => io::Error::other(fenix_vfs::Changed(failure.message)),
            FailureCode::Other => io::Error::other(failure.message),
        },
    }
}

impl fenix_vfs::Remote for Host {
    fn home(&self) -> String {
        self.state.lock().unwrap().home.clone()
    }

    fn run(&self, cwd: &str, run: Run<'_>) -> io::Result<Ran> {
        let exec = Exec {
            program: run.program.to_string(),
            args: run.args.iter().map(|arg| arg.to_string()).collect(),
            cwd: Some(cwd.to_string()),
            env: run.env.iter().map(|(key, value)| (key.to_string(), value.to_string())).collect(),
            stdin: run.stdin.is_some(),
        };
        let process = self.session().and_then(|session| session.exec(exec)).map_err(io_error)?;
        if let Some(bytes) = run.stdin {
            let input = process.input();
            // In pieces, so a large patch doesn't hold the connection.
            for piece in bytes.chunks(fenix_remote::PIECE) {
                input.write(piece).map_err(io_error)?;
            }
            input.close().map_err(io_error)?;
        }
        let finished = process.finish().map_err(io_error)?;
        Ok(Ran { code: finished.code, stdout: finished.stdout, stderr: finished.stderr })
    }

    fn stat(&self, path: &str) -> io::Result<Meta> {
        self.session().and_then(|session| session.stat(path)).map(|entry| meta(entry, Some(path))).map_err(io_error)
    }

    fn list(&self, path: &str) -> io::Result<Vec<Meta>> {
        let entries = self.session().and_then(|session| session.list(path)).map_err(io_error)?;
        Ok(entries.into_iter().map(|entry| meta(entry, None)).collect())
    }

    fn read(&self, path: &str) -> io::Result<(Vec<u8>, Meta)> {
        self.session().and_then(|session| session.read(path)).map(|(bytes, entry)| (bytes, meta(entry, Some(path)))).map_err(io_error)
    }

    fn write(&self, path: &str, contents: &[u8], over: Over) -> io::Result<Meta> {
        let (expect, create_new) = match over {
            Over::Anything => (None, false),
            Over::Version(version) => (Some(fenix_remote::Version { len: version.len, mtime: version.mtime }), false),
            Over::Nothing => (None, true),
        };
        self.session().and_then(|session| session.write(path, contents, expect, create_new)).map(|entry| meta(entry, Some(path))).map_err(io_error)
    }

    fn rename(&self, from: &str, to: &str, replace: bool) -> io::Result<()> {
        self.session().and_then(|session| session.rename(from, to, replace)).map_err(io_error)
    }

    fn remove(&self, path: &str, recursive: bool) -> io::Result<()> {
        self.session().and_then(|session| session.remove(path, recursive)).map_err(io_error)
    }

    fn create_dir(&self, path: &str, parents: bool) -> io::Result<()> {
        self.session().and_then(|session| session.create_dir(path, parents)).map(drop).map_err(io_error)
    }

    fn copy(&self, from: &str, to: &str) -> io::Result<()> {
        self.session().and_then(|session| session.copy(from, to)).map_err(io_error)
    }
}
