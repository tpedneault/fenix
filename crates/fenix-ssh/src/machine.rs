//! A machine's files, programs, terminals and searches over a session:
//! what a [`Host`](crate::Host) is to `fenix-vfs`, and what
//! [`testing::Loopback`](crate::testing::Loopback) is to tests.
//!
//! Each call is given the session to go over, or why there's none.

use std::io;
use std::sync::{Arc, Mutex};

use fenix_remote::{Entry, Exec, FailureCode, FileKind, Pty, Search, Size};
use fenix_vfs::{FoundLine, Kind, Meta, Over, Ran, Run, Searched, Shell, Terminal, TerminalControl};

use crate::session::{Error, Output, ProcessInput, Session};

/// The most a program run to its end (`Remote::run`: a listing of the
/// project's files, a Git command) may print before it's stopped. What a
/// remote machine sends is kept in memory here, and a machine that never
/// stops sending must not be able to fill it.
const MOST_OUTPUT: usize = 256 * 1024 * 1024;

type Through = Result<Arc<Session>, Error>;

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

pub(crate) fn io_error(error: Error) -> io::Error {
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

pub(crate) fn run(session: Through, cwd: &str, run: Run<'_>) -> io::Result<Ran> {
    let exec = Exec {
        program: run.program.to_string(),
        args: run.args.iter().map(|arg| arg.to_string()).collect(),
        cwd: Some(cwd.to_string()),
        env: run.env.iter().map(|(key, value)| (key.to_string(), value.to_string())).collect(),
        stdin: run.stdin.is_some(),
    };
    let process = session.and_then(|session| session.exec(exec)).map_err(io_error)?;
    if let Some(bytes) = run.stdin {
        let input = process.input();
        // In pieces, so a large patch doesn't hold the connection.
        for piece in bytes.chunks(fenix_remote::PIECE) {
            input.write(piece).map_err(io_error)?;
        }
        input.close().map_err(io_error)?;
    }
    let mut ran = Ran::default();
    loop {
        match process.next().map_err(io_error)? {
            Output::Stdout(bytes) => ran.stdout.extend(bytes),
            Output::Stderr(bytes) => ran.stderr.extend(bytes),
            Output::Exit(code) => {
                ran.code = code;
                return Ok(ran);
            }
        }
        if ran.stdout.len() + ran.stderr.len() > MOST_OUTPUT {
            let _ = process.input().kill();
            return Err(io::Error::other(format!("{} printed more than Fenix will hold, and was stopped", run.program)));
        }
    }
}

pub(crate) fn stat(session: Through, path: &str) -> io::Result<Meta> {
    session.and_then(|session| session.stat(path)).map(|entry| meta(entry, Some(path))).map_err(io_error)
}

pub(crate) fn list(session: Through, path: &str) -> io::Result<Vec<Meta>> {
    let entries = session.and_then(|session| session.list(path)).map_err(io_error)?;
    // A folder's entries are names in that folder. One that says it is
    // elsewhere (`..`, `a/b`) is no name a file system gives, and comes
    // from a machine that isn't to be believed: left out.
    Ok(entries.into_iter().filter(|entry| fenix_vfs::path::is_one_name(&entry.name)).map(|entry| meta(entry, None)).collect())
}

pub(crate) fn read(session: Through, path: &str) -> io::Result<(Vec<u8>, Meta)> {
    session.and_then(|session| session.read(path)).map(|(bytes, entry)| (bytes, meta(entry, Some(path)))).map_err(io_error)
}

pub(crate) fn write(session: Through, path: &str, contents: &[u8], over: Over) -> io::Result<Meta> {
    let (expect, create_new) = match over {
        Over::Anything => (None, false),
        Over::Version(version) => (Some(fenix_remote::Version { len: version.len, mtime: version.mtime }), false),
        Over::Nothing => (None, true),
    };
    session.and_then(|session| session.write(path, contents, expect, create_new)).map(|entry| meta(entry, Some(path))).map_err(io_error)
}

pub(crate) fn rename(session: Through, from: &str, to: &str, replace: bool) -> io::Result<()> {
    session.and_then(|session| session.rename(from, to, replace)).map_err(io_error)
}

pub(crate) fn remove(session: Through, path: &str, recursive: bool) -> io::Result<()> {
    session.and_then(|session| session.remove(path, recursive)).map_err(io_error)
}

pub(crate) fn create_dir(session: Through, path: &str, parents: bool) -> io::Result<()> {
    session.and_then(|session| session.create_dir(path, parents)).map(drop).map_err(io_error)
}

pub(crate) fn copy(session: Through, from: &str, to: &str) -> io::Result<()> {
    session.and_then(|session| session.copy(from, to)).map_err(io_error)
}

/// A remote terminal's program, for the one who shows it.
struct Control {
    input: ProcessInput,
    exit: Arc<Mutex<Option<Option<i32>>>>,
}

impl TerminalControl for Control {
    fn resize(&self, rows: u16, cols: u16) -> io::Result<()> {
        self.input.resize(rows, cols).map_err(io_error)
    }

    fn exited(&self) -> Option<Option<i32>> {
        *self.exit.lock().unwrap()
    }

    fn kill(&self) {
        let _ = self.input.kill();
    }
}

/// What's typed into a remote terminal, as its program's stdin.
struct Typing(ProcessInput);

impl io::Write for Typing {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.write(bytes).map_err(io_error)?;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub(crate) fn terminal(session: Through, cwd: &str, shell: Shell<'_>) -> io::Result<Terminal> {
    let pty = Pty {
        program: shell.program.map(str::to_string),
        args: shell.args.to_vec(),
        cwd: Some(cwd.to_string()),
        env: shell.env.iter().map(|(key, value)| (key.to_string(), value.to_string())).collect(),
        size: Size { rows: shell.rows, cols: shell.cols },
    };
    let started = session.and_then(|session| session.terminal(pty)).map_err(io_error)?;
    let control = Control { input: started.input.clone(), exit: started.exit };
    Ok(Terminal { reader: Box::new(started.output), writer: Box::new(Typing(started.input)), control: Box::new(control) })
}

pub(crate) fn search(session: Through, search: fenix_vfs::Search<'_>, found: &mut fenix_vfs::Finding<'_>) -> io::Result<Searched> {
    let asked = Search {
        folders: search.folders.to_vec(),
        files: search.files.to_vec(),
        pattern: search.pattern.to_string(),
        hidden: search.hidden,
        globs: search.globs.to_vec(),
        context: search.context,
        skip: search.skip.to_vec(),
    };
    let session = session.map_err(io_error)?;
    let searched = session.search(asked, &mut |file| found(file.map(|(path, lines)| (path, lines.into_iter().map(|line| FoundLine { number: line.number, text: line.text, matched: line.matched }).collect()))));
    let searched = searched.map_err(io_error)?;
    Ok(Searched { files_searched: searched.files_searched, binary_skipped: searched.binary_skipped })
}
