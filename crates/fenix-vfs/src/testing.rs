//! A remote machine for tests: a folder on this one.
//!
//! What's under [`Folder::root`] is the machine's `/`. Tests of anything
//! that takes a remote path mount one and work on real files, with no
//! connection to make.

use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

use crate::{Meta, Over, Ran, Remote, Run};

pub struct Folder {
    pub root: PathBuf,
    /// Every path the machine was asked about, in order.
    pub calls: Mutex<Vec<String>>,
    /// While set, every call fails as a lost connection does.
    pub away: Mutex<bool>,
}

impl Folder {
    /// Mounts `root` as the machine `host`.
    pub fn mount(host: &str, root: &Path) -> Arc<Folder> {
        let folder = Arc::new(Folder { root: root.to_path_buf(), calls: Mutex::default(), away: Mutex::new(false) });
        crate::mount(host, folder.clone());
        folder
    }

    /// Makes the machine unreachable, or reachable again.
    pub fn set_away(&self, away: bool) {
        *self.away.lock().unwrap() = away;
    }

    fn local(&self, path: &str) -> io::Result<PathBuf> {
        if *self.away.lock().unwrap() {
            return Err(io::Error::new(io::ErrorKind::NotConnected, "the connection to the remote machine was lost"));
        }
        self.calls.lock().unwrap().push(path.to_string());
        let mut local = self.root.clone();
        local.extend(path.split('/').filter(|name| !name.is_empty()));
        Ok(local)
    }

    fn named(path: &str, mut meta: Meta) -> Meta {
        meta.name = path.trim_end_matches('/').rsplit('/').next().unwrap_or_default().to_string();
        meta
    }
}

impl Remote for Folder {
    fn home(&self) -> String {
        "/".into()
    }

    fn run(&self, cwd: &str, run: Run<'_>) -> io::Result<Ran> {
        let mut command = Command::new(run.program);
        command.args(run.args).envs(run.env.iter().copied()).current_dir(self.local(cwd)?);
        command.stdin(if run.stdin.is_some() { Stdio::piped() } else { Stdio::null() }).stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = command.spawn()?;
        if let (Some(bytes), Some(mut pipe)) = (run.stdin, child.stdin.take()) {
            use std::io::Write;
            let _ = pipe.write_all(bytes);
        }
        let output = child.wait_with_output()?;
        Ok(Ran { code: output.status.code(), stdout: output.stdout, stderr: output.stderr })
    }

    fn stat(&self, path: &str) -> io::Result<Meta> {
        crate::local_meta(&self.local(path)?, String::new()).map(|meta| Folder::named(path, meta))
    }

    fn list(&self, path: &str) -> io::Result<Vec<Meta>> {
        crate::list(&self.local(path)?)
    }

    fn read(&self, path: &str) -> io::Result<(Vec<u8>, Meta)> {
        crate::read_with_meta(&self.local(path)?).map(|(bytes, meta)| (bytes, Folder::named(path, meta)))
    }

    fn write(&self, path: &str, contents: &[u8], over: Over) -> io::Result<Meta> {
        crate::write(&self.local(path)?, contents, over).map(|meta| Folder::named(path, meta))
    }

    fn rename(&self, from: &str, to: &str, replace: bool) -> io::Result<()> {
        crate::rename(&self.local(from)?, &self.local(to)?, replace)
    }

    fn remove(&self, path: &str, recursive: bool) -> io::Result<()> {
        crate::remove(&self.local(path)?, recursive)
    }

    fn create_dir(&self, path: &str, parents: bool) -> io::Result<()> {
        let local = self.local(path)?;
        if parents {
            std::fs::create_dir_all(local)
        } else {
            std::fs::create_dir(local)
        }
    }

    fn copy(&self, from: &str, to: &str) -> io::Result<()> {
        fn copy(from: &Path, to: &Path) -> io::Result<()> {
            if !from.is_dir() {
                return std::fs::copy(from, to).map(drop);
            }
            std::fs::create_dir(to)?;
            for item in std::fs::read_dir(from)? {
                let item = item?;
                copy(&item.path(), &to.join(item.file_name()))?;
            }
            Ok(())
        }
        let (from, to) = (self.local(from)?, self.local(to)?);
        if to.exists() {
            return Err(io::Error::new(io::ErrorKind::AlreadyExists, "already there"));
        }
        copy(&from, &to)
    }
}
