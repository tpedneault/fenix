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

use fenix_vfs::{Meta, Over, Ran, Run};

use crate::askpass::Askpass;
use crate::bridge::Bridge;
use crate::machine;
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

impl fenix_vfs::Remote for Host {
    fn home(&self) -> String {
        self.state.lock().unwrap().home.clone()
    }

    fn run(&self, cwd: &str, run: Run<'_>) -> io::Result<Ran> {
        machine::run(self.session(), cwd, run)
    }

    fn stat(&self, path: &str) -> io::Result<Meta> {
        machine::stat(self.session(), path)
    }

    fn list(&self, path: &str) -> io::Result<Vec<Meta>> {
        machine::list(self.session(), path)
    }

    fn read(&self, path: &str) -> io::Result<(Vec<u8>, Meta)> {
        machine::read(self.session(), path)
    }

    fn write(&self, path: &str, contents: &[u8], over: Over) -> io::Result<Meta> {
        machine::write(self.session(), path, contents, over)
    }

    fn rename(&self, from: &str, to: &str, replace: bool) -> io::Result<()> {
        machine::rename(self.session(), from, to, replace)
    }

    fn remove(&self, path: &str, recursive: bool) -> io::Result<()> {
        machine::remove(self.session(), path, recursive)
    }

    fn create_dir(&self, path: &str, parents: bool) -> io::Result<()> {
        machine::create_dir(self.session(), path, parents)
    }

    fn copy(&self, from: &str, to: &str) -> io::Result<()> {
        machine::copy(self.session(), from, to)
    }

    fn terminal(&self, cwd: &str, shell: fenix_vfs::Shell<'_>) -> io::Result<fenix_vfs::Terminal> {
        machine::terminal(self.session(), cwd, shell)
    }

    fn search(&self, search: fenix_vfs::Search<'_>, found: &mut fenix_vfs::Finding<'_>) -> io::Result<fenix_vfs::Searched> {
        machine::search(self.session(), search, found)
    }
}
