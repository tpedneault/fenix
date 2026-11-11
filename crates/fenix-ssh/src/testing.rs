//! A remote machine for tests: the real helper, serving in this process
//! over two pipes, with no `ssh` and no other machine. Everything a
//! [`Host`](crate::Host) does with a machine goes through the same
//! protocol to the same code; only the connection is missing.
//!
//! The machine's paths are this machine's own, so it serves only where
//! those are `/` paths: tests that use it are for Unix.

use std::io::{self, Write};
use std::sync::Arc;
use std::thread;

use fenix_vfs::{Meta, Over, Ran, Run};

use crate::machine;
use crate::session::{Error, Session};

/// A machine whose helper runs in this process.
pub struct Loopback {
    session: Arc<Session>,
}

impl Loopback {
    /// Starts a helper here and mounts it as the machine `host`.
    pub fn mount(host: &str) -> io::Result<Arc<Loopback>> {
        let (to_helper_read, to_helper_write) = io::pipe()?;
        let (from_helper_read, mut from_helper_write) = io::pipe()?;
        thread::spawn(move || {
            let hello = serde_json::to_string(&fenix_remote::server::hello()).expect("hello serializes");
            if writeln!(from_helper_write, "{}{hello}", fenix_remote::BANNER).is_ok() {
                let _ = fenix_remote::server::serve(to_helper_read, from_helper_write);
            }
        });
        let session = Session::start(from_helper_read, to_helper_write).map_err(|error| io::Error::other(error.to_string()))?;
        let loopback = Arc::new(Loopback { session: Arc::new(session) });
        fenix_vfs::mount(host, loopback.clone());
        Ok(loopback)
    }

    pub fn session(&self) -> Arc<Session> {
        self.session.clone()
    }

    fn through(&self) -> Result<Arc<Session>, Error> {
        if self.session.lost() { Err(Error::Lost) } else { Ok(self.session.clone()) }
    }
}

impl fenix_vfs::Remote for Loopback {
    fn home(&self) -> String {
        self.session.hello().home.clone()
    }

    fn run(&self, cwd: &str, run: Run<'_>) -> io::Result<Ran> {
        machine::run(self.through(), cwd, run)
    }

    fn stat(&self, path: &str) -> io::Result<Meta> {
        machine::stat(self.through(), path)
    }

    fn list(&self, path: &str) -> io::Result<Vec<Meta>> {
        machine::list(self.through(), path)
    }

    fn read(&self, path: &str) -> io::Result<(Vec<u8>, Meta)> {
        machine::read(self.through(), path)
    }

    fn write(&self, path: &str, contents: &[u8], over: Over) -> io::Result<Meta> {
        machine::write(self.through(), path, contents, over)
    }

    fn rename(&self, from: &str, to: &str, replace: bool) -> io::Result<()> {
        machine::rename(self.through(), from, to, replace)
    }

    fn remove(&self, path: &str, recursive: bool) -> io::Result<()> {
        machine::remove(self.through(), path, recursive)
    }

    fn create_dir(&self, path: &str, parents: bool) -> io::Result<()> {
        machine::create_dir(self.through(), path, parents)
    }

    fn copy(&self, from: &str, to: &str) -> io::Result<()> {
        machine::copy(self.through(), from, to)
    }

    fn terminal(&self, cwd: &str, shell: fenix_vfs::Shell<'_>) -> io::Result<fenix_vfs::Terminal> {
        machine::terminal(self.through(), cwd, shell)
    }

    fn search(&self, search: fenix_vfs::Search<'_>, found: &mut fenix_vfs::Finding<'_>) -> io::Result<fenix_vfs::Searched> {
        machine::search(self.through(), search, found)
    }
}
