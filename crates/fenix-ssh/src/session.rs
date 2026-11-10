//! One connection to a helper: requests with deadlines, and programs
//! whose output comes back as it's written.

use std::collections::HashMap;
use std::fmt;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::process::Child;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use fenix_remote::{Entry, Exec, Exit, Failure, Frame, Hello, Kind, Request, Version, BANNER, MISSING, PIECE, PROTOCOL};

/// How long the helper may say nothing to a request before the
/// connection is taken for dead. Nothing waits on a remote machine
/// without a limit. A large file moves in pieces, each within it.
pub const DEADLINE: Duration = Duration::from_secs(30);

/// How much a shell may print before the helper's first line.
const MOST_PREAMBLE: usize = 1024 * 1024;

#[derive(Debug)]
pub enum Error {
    /// The connection is gone.
    Lost,
    /// No answer within the deadline.
    TimedOut,
    /// Stopped because the caller said so.
    Cancelled,
    /// The helper answered that it couldn't.
    Remote(Failure),
    /// Something that isn't the protocol.
    Protocol(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Lost => f.write_str("the connection was lost"),
            Error::TimedOut => f.write_str("the remote machine didn't answer in time"),
            Error::Cancelled => f.write_str("stopped"),
            Error::Remote(failure) => f.write_str(&failure.message),
            Error::Protocol(what) => write!(f, "unexpected from the helper: {what}"),
        }
    }
}

impl std::error::Error for Error {}

/// Why a connection didn't start.
#[derive(Debug)]
pub enum StartError {
    /// The machine has no helper of this version; what `uname -sm` says
    /// of it.
    Missing(String),
    /// The other end closed before a helper spoke. What it printed.
    Closed(String),
    /// A helper of another protocol version.
    Version(u32),
    Io(io::Error),
}

impl fmt::Display for StartError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StartError::Missing(system) => write!(f, "no helper on the remote machine ({system})"),
            StartError::Closed(said) if said.trim().is_empty() => f.write_str("the connection closed before the helper started"),
            StartError::Closed(said) => write!(f, "the connection closed before the helper started: {}", said.trim()),
            StartError::Version(theirs) => write!(f, "the helper speaks protocol {theirs}, Fenix {PROTOCOL}"),
            StartError::Io(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for StartError {}

struct Shared {
    writer: Mutex<Box<dyn Write + Send>>,
    /// Where each request's answers go.
    routes: Mutex<HashMap<u32, Sender<Frame>>>,
    next: AtomicU32,
    lost: AtomicBool,
}

impl Shared {
    fn send(&self, frames: &[Frame]) -> Result<(), Error> {
        let mut writer = self.writer.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let sent = frames.iter().try_for_each(|frame| frame.write(&mut *writer)).and_then(|()| writer.flush());
        sent.map_err(|_| {
            self.lost.store(true, Ordering::SeqCst);
            Error::Lost
        })
    }

    fn open(&self) -> (u32, Receiver<Frame>) {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = mpsc::channel();
        self.routes.lock().unwrap().insert(id, sender);
        (id, receiver)
    }

    fn close(&self, id: u32) {
        self.routes.lock().unwrap().remove(&id);
    }
}

/// A connection to a helper.
pub struct Session {
    hello: Hello,
    shared: Arc<Shared>,
    /// The `ssh` that carries it, when there is one: ended with the
    /// session.
    carrier: Mutex<Option<Child>>,
}

impl Session {
    /// Starts a session over a helper's output and input. Reads past
    /// whatever was printed before the helper's first line.
    pub fn start(output: impl Read + Send + 'static, input: impl Write + Send + 'static) -> Result<Session, StartError> {
        let mut reader = BufReader::new(Box::new(output) as Box<dyn Read + Send>);
        let hello = preamble(&mut reader)?;
        Session::greeted(reader, input, hello)
    }

    /// Starts a session with a helper whose first line was read already.
    pub(crate) fn greeted(reader: BufReader<Box<dyn Read + Send>>, input: impl Write + Send + 'static, hello: Hello) -> Result<Session, StartError> {
        if hello.protocol != PROTOCOL {
            return Err(StartError::Version(hello.protocol));
        }
        let shared = Arc::new(Shared { writer: Mutex::new(Box::new(input)), routes: Mutex::default(), next: AtomicU32::new(1), lost: AtomicBool::new(false) });
        let routing = shared.clone();
        thread::Builder::new().name("fenix-ssh-read".into()).spawn(move || route(reader, &routing)).map_err(StartError::Io)?;
        Ok(Session { hello, shared, carrier: Mutex::new(None) })
    }

    /// Cuts the connection under the session, as a network would.
    pub fn sever(&self) {
        if let Some(carrier) = self.carrier.lock().unwrap().as_mut() {
            let _ = carrier.kill();
        }
    }

    /// Has the session end `carrier` when it ends itself.
    pub fn carried_by(self, carrier: Child) -> Session {
        *self.carrier.lock().unwrap() = Some(carrier);
        self
    }

    pub fn hello(&self) -> &Hello {
        &self.hello
    }

    /// Whether the connection is known to be gone.
    pub fn lost(&self) -> bool {
        self.shared.lost.load(Ordering::SeqCst)
    }

    pub fn stat(&self, path: &str) -> Result<Entry, Error> {
        self.ask(Request::Stat { path: path.into() })
    }

    pub fn list(&self, path: &str) -> Result<Vec<Entry>, Error> {
        self.ask(Request::List { path: path.into() })
    }

    /// A file's contents, and the file as it was when read.
    pub fn read(&self, path: &str) -> Result<(Vec<u8>, Entry), Error> {
        self.read_watched(path, &mut |_| true)
    }

    /// `read`, telling `progress` how many bytes have come so far; it
    /// answers whether to go on.
    pub fn read_watched(&self, path: &str, progress: &mut dyn FnMut(u64) -> bool) -> Result<(Vec<u8>, Entry), Error> {
        let (bytes, value) = self.call(Request::Read { path: path.into() }, None, progress)?;
        Ok((bytes, parse(value)?))
    }

    /// Replaces a file's contents, whole or not at all. With `expect`,
    /// only if it is still as it was (`FailureCode::Changed` otherwise);
    /// with `create_new`, only if there's no such file.
    pub fn write(&self, path: &str, contents: &[u8], expect: Option<Version>, create_new: bool) -> Result<Entry, Error> {
        self.write_watched(path, contents, expect, create_new, &mut |_| true)
    }

    /// `write`, telling `progress` how many bytes have gone so far; it
    /// answers whether to go on. Stopped, the file is as it was.
    pub fn write_watched(&self, path: &str, contents: &[u8], expect: Option<Version>, create_new: bool, progress: &mut dyn FnMut(u64) -> bool) -> Result<Entry, Error> {
        self.call(Request::Write { path: path.into(), expect, create_new }, Some(contents), progress).and_then(|(_, value)| parse(value))
    }

    pub fn rename(&self, from: &str, to: &str, replace: bool) -> Result<(), Error> {
        self.ask::<serde_json::Value>(Request::Rename { from: from.into(), to: to.into(), replace }).map(drop)
    }

    /// Deletes, for good.
    pub fn remove(&self, path: &str, recursive: bool) -> Result<(), Error> {
        self.ask::<serde_json::Value>(Request::Remove { path: path.into(), recursive }).map(drop)
    }

    pub fn create_dir(&self, path: &str, parents: bool) -> Result<Entry, Error> {
        self.ask(Request::CreateDir { path: path.into(), parents })
    }

    /// Copies a file or a folder within the remote machine.
    pub fn copy(&self, from: &str, to: &str) -> Result<(), Error> {
        self.ask::<serde_json::Value>(Request::Copy { from: from.into(), to: to.into() }).map(drop)
    }

    fn ask<T: serde::de::DeserializeOwned>(&self, request: Request) -> Result<T, Error> {
        self.call(request, None, &mut |_| true).and_then(|(_, value)| parse(value))
    }

    /// One request to its answer: `upload`, if any, sent in pieces after
    /// it, and whatever bytes come back gathered.
    fn call(&self, request: Request, upload: Option<&[u8]>, progress: &mut dyn FnMut(u64) -> bool) -> Result<(Vec<u8>, serde_json::Value), Error> {
        let (id, answers) = self.shared.open();
        let stop = || {
            let _ = self.shared.send(&[Frame::new(Kind::Kill, id, [])]);
            Error::Cancelled
        };
        let result = (|| {
            self.shared.send(&[Frame::json(Kind::Request, id, &request)])?;
            let mut early = None;
            if let Some(contents) = upload {
                let mut sent = 0u64;
                // A piece a send, so other requests get in between.
                for piece in contents.chunks(PIECE) {
                    // Refused already (the file changed): no use sending
                    // the rest.
                    if let Ok(frame) = answers.try_recv() {
                        early = Some(frame);
                        break;
                    }
                    self.shared.send(&[Frame::new(Kind::Body, id, piece)])?;
                    sent += piece.len() as u64;
                    if !progress(sent) {
                        return Err(stop());
                    }
                }
                if early.is_none() {
                    self.shared.send(&[Frame::new(Kind::BodyEnd, id, [])])?;
                }
            }
            let mut bytes = Vec::new();
            loop {
                let frame = match early.take() {
                    Some(frame) => frame,
                    None => match answers.recv_timeout(DEADLINE) {
                        Ok(frame) => frame,
                        Err(RecvTimeoutError::Timeout) => return Err(Error::TimedOut),
                        Err(RecvTimeoutError::Disconnected) => return Err(Error::Lost),
                    },
                };
                match frame.kind {
                    Kind::Bytes => {
                        bytes.extend_from_slice(&frame.payload);
                        if !progress(bytes.len() as u64) {
                            return Err(stop());
                        }
                    }
                    Kind::Ok => return serde_json::from_slice(&frame.payload).map(|value| (bytes, value)).map_err(|error| Error::Protocol(error.to_string())),
                    Kind::Err => return Err(Error::Remote(parse_slice(&frame.payload)?)),
                    other => return Err(Error::Protocol(format!("{other:?} in answer to a request"))),
                }
            }
        })();
        self.shared.close(id);
        result
    }

    /// Starts a program on the remote machine.
    pub fn exec(&self, exec: Exec) -> Result<Process, Error> {
        let (id, frames) = self.shared.open();
        if let Err(error) = self.shared.send(&[Frame::json(Kind::Request, id, &Request::Exec(exec))]) {
            self.shared.close(id);
            return Err(error);
        }
        Ok(Process { input: ProcessInput { id, shared: self.shared.clone() }, frames })
    }

    /// Runs a program to its end and gives back all it wrote.
    pub fn output(&self, exec: Exec) -> Result<Finished, Error> {
        self.exec(exec)?.finish()
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if let Some(mut carrier) = self.carrier.lock().unwrap().take() {
            let _ = carrier.kill();
            let _ = carrier.wait();
        }
    }
}

fn parse<T: serde::de::DeserializeOwned>(value: serde_json::Value) -> Result<T, Error> {
    serde_json::from_value(value).map_err(|error| Error::Protocol(error.to_string()))
}

fn parse_slice<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, Error> {
    serde_json::from_slice(bytes).map_err(|error| Error::Protocol(error.to_string()))
}

/// Reads up to the helper's first line. A login shell may print a
/// greeting first; none of it is the protocol's.
pub(crate) fn preamble(reader: &mut impl BufRead) -> Result<Hello, StartError> {
    let mut seen = Vec::new();
    loop {
        let mut line = Vec::new();
        let read = reader.read_until(b'\n', &mut line).map_err(StartError::Io)?;
        let text = String::from_utf8_lossy(&line);
        let text = text.trim();
        if let Some(hello) = text.strip_prefix(BANNER) {
            return serde_json::from_str(hello).map_err(|error| StartError::Closed(format!("a helper's greeting Fenix can't read: {error}")));
        }
        if let Some(system) = text.strip_prefix(MISSING) {
            return Err(StartError::Missing(system.to_string()));
        }
        seen.extend_from_slice(&line);
        if read == 0 || seen.len() > MOST_PREAMBLE {
            return Err(StartError::Closed(String::from_utf8_lossy(&seen).into_owned()));
        }
    }
}

/// Hands each frame to the request it answers, until the stream ends.
fn route(mut reader: impl Read, shared: &Shared) {
    while let Ok(Some(frame)) = Frame::read(&mut reader) {
        let last = matches!(frame.kind, Kind::Ok | Kind::Err | Kind::Exit);
        let mut routes = shared.routes.lock().unwrap();
        let id = frame.id;
        if let Some(route) = routes.get(&id) {
            // A request nobody waits on any more: its answer is dropped.
            let _ = route.send(frame);
        }
        if last {
            routes.remove(&id);
        }
    }
    shared.lost.store(true, Ordering::SeqCst);
    // Every waiting request wakes to a closed channel: `Error::Lost`.
    shared.routes.lock().unwrap().clear();
}

/// What a running program says.
#[derive(Debug, PartialEq, Eq)]
pub enum Output {
    Stdout(Vec<u8>),
    Stderr(Vec<u8>),
    /// It ended; `None` when a signal ended it. Nothing follows.
    Exit(Option<i32>),
}

/// All of a program's output.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Finished {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub code: Option<i32>,
}

/// A program running on the remote machine.
pub struct Process {
    input: ProcessInput,
    frames: Receiver<Frame>,
}

/// The way in to a running program: its stdin, and stopping it. Apart
/// from the `Process`, so one thread can feed it while another reads.
#[derive(Clone)]
pub struct ProcessInput {
    id: u32,
    shared: Arc<Shared>,
}

impl ProcessInput {
    pub fn write(&self, bytes: &[u8]) -> Result<(), Error> {
        self.shared.send(&[Frame::new(Kind::Body, self.id, bytes)])
    }

    /// Ends the program's stdin.
    pub fn close(&self) -> Result<(), Error> {
        self.shared.send(&[Frame::new(Kind::BodyEnd, self.id, [])])
    }

    /// Kills the program and everything it started.
    pub fn kill(&self) -> Result<(), Error> {
        self.shared.send(&[Frame::new(Kind::Kill, self.id, [])])
    }
}

impl Process {
    pub fn input(&self) -> ProcessInput {
        self.input.clone()
    }

    /// Waits for the program's next output, or its end. An error if it
    /// never started, or the connection went.
    pub fn next(&self) -> Result<Output, Error> {
        let frame = self.frames.recv().map_err(|_| Error::Lost)?;
        match frame.kind {
            Kind::Stdout => Ok(Output::Stdout(frame.payload)),
            Kind::Stderr => Ok(Output::Stderr(frame.payload)),
            Kind::Exit => Ok(Output::Exit(parse_slice::<Exit>(&frame.payload)?.code)),
            Kind::Err => Err(Error::Remote(parse_slice(&frame.payload)?)),
            other => Err(Error::Protocol(format!("{other:?} from a program"))),
        }
    }

    pub fn finish(self) -> Result<Finished, Error> {
        let mut finished = Finished::default();
        loop {
            match self.next()? {
                Output::Stdout(bytes) => finished.stdout.extend(bytes),
                Output::Stderr(bytes) => finished.stderr.extend(bytes),
                Output::Exit(code) => {
                    finished.code = code;
                    return Ok(finished);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fenix_remote::FailureCode;

    /// A session with a helper running in this process, on two pipes.
    fn local() -> Session {
        let (to_helper_read, to_helper_write) = io::pipe().unwrap();
        let (from_helper_read, mut from_helper_write) = io::pipe().unwrap();
        thread::spawn(move || {
            let hello = serde_json::to_string(&fenix_remote::server::hello()).unwrap();
            // What a chatty `.bashrc` does to the stream.
            write!(from_helper_write, "Welcome to devbox!\nLast login: yesterday\n\n{BANNER}{hello}\n").unwrap();
            let _ = fenix_remote::server::serve(to_helper_read, from_helper_write);
        });
        Session::start(from_helper_read, to_helper_write).unwrap()
    }

    fn remote(path: &std::path::Path) -> String {
        path.to_string_lossy().into_owned()
    }

    #[test]
    fn a_greeting_before_the_helper_is_skipped() {
        let session = local();
        assert_eq!(session.hello().protocol, PROTOCOL);
        assert!(!session.lost());
    }

    #[test]
    fn a_machine_without_a_helper_says_what_it_is() {
        let said = format!("motd\n{MISSING}Linux aarch64\n");
        let started = Session::start(io::Cursor::new(said.into_bytes()), io::sink());
        assert!(matches!(started, Err(StartError::Missing(system)) if system == "Linux aarch64"));
    }

    #[test]
    fn a_connection_that_closes_first_reports_what_was_said() {
        let started = Session::start(io::Cursor::new(b"Permission denied (publickey).\n".to_vec()), io::sink());
        assert!(matches!(started, Err(StartError::Closed(said)) if said.contains("Permission denied")));
    }

    #[test]
    fn files_are_listed_read_and_written() {
        let session = local();
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.txt");
        std::fs::write(&file, b"one\r\ntwo\n\xff").unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();

        let mut names: Vec<_> = session.list(&remote(dir.path())).unwrap().into_iter().map(|entry| (entry.name, entry.kind)).collect();
        names.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(names, [("a.txt".to_string(), fenix_remote::FileKind::File), ("sub".to_string(), fenix_remote::FileKind::Dir)]);

        let (bytes, read) = session.read(&remote(&file)).unwrap();
        assert_eq!(bytes, b"one\r\ntwo\n\xff");
        let written = session.write(&remote(&file), b"three", Some(read.version()), false).unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), b"three");
        assert_eq!(written.len, 5);
        // Nothing staged is left beside it.
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
    }

    #[test]
    fn a_checked_write_refuses_a_file_that_changed() {
        let session = local();
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.txt");
        std::fs::write(&file, b"mine").unwrap();
        let (_, read) = session.read(&remote(&file)).unwrap();
        std::fs::write(&file, b"someone else's, longer").unwrap();

        let refused = session.write(&remote(&file), b"over it", Some(read.version()), false);
        assert!(matches!(refused, Err(Error::Remote(failure)) if failure.code == FailureCode::Changed));
        assert_eq!(std::fs::read(&file).unwrap(), b"someone else's, longer");
        // Unchecked, it goes through.
        session.write(&remote(&file), b"over it", None, false).unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), b"over it");
    }

    #[test]
    fn a_missing_file_is_not_found() {
        let session = local();
        let dir = tempfile::tempdir().unwrap();
        let missing = session.read(&remote(&dir.path().join("nope")));
        assert!(matches!(missing, Err(Error::Remote(failure)) if failure.code == FailureCode::NotFound));
    }

    #[test]
    fn a_large_file_moves_in_pieces_and_can_be_stopped() {
        let session = local();
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("big.bin");
        let big: Vec<u8> = (0..3 * PIECE + 17).map(|i| (i % 251) as u8).collect();
        let mut steps = Vec::new();
        let written = session.write_watched(&remote(&file), &big, None, true, &mut |sent| {
            steps.push(sent);
            true
        });
        written.unwrap();
        assert_eq!(steps, [PIECE as u64, 2 * PIECE as u64, 3 * PIECE as u64, big.len() as u64]);
        let mut seen = 0;
        let read = session.read_watched(&remote(&file), &mut |got| {
            seen = got;
            true
        });
        assert_eq!(read.unwrap().0, big);
        assert_eq!(seen, big.len() as u64);

        // Stopped part way, a write leaves the file as it was, and
        // nothing beside it.
        let stopped = session.write_watched(&remote(&file), &vec![0u8; 4 * PIECE], None, false, &mut |sent| sent < 2 * PIECE as u64);
        assert!(matches!(stopped, Err(Error::Cancelled)));
        assert!(matches!(session.read_watched(&remote(&file), &mut |_| false), Err(Error::Cancelled)));
        // The helper tidies in its own time.
        for _ in 0..100 {
            if std::fs::read_dir(dir.path()).unwrap().count() == 1 {
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
        assert_eq!(std::fs::read(&file).unwrap(), big);
        // An empty file is a file.
        let empty = dir.path().join("empty");
        assert_eq!(session.write(&remote(&empty), b"", None, true).unwrap().len, 0);
        let again = session.write(&remote(&empty), b"x", None, true);
        assert!(matches!(again, Err(Error::Remote(failure)) if failure.code == FailureCode::Exists));
    }

    #[test]
    fn folders_and_files_are_made_moved_copied_and_deleted() {
        let session = local();
        let dir = tempfile::tempdir().unwrap();
        let at = |name: &str| remote(&dir.path().join(name));
        session.create_dir(&at("a/b/c"), true).unwrap();
        assert!(matches!(session.create_dir(&at("x/y"), false), Err(Error::Remote(failure)) if failure.code == FailureCode::NotFound));
        session.write(&at("a/b/c/f.txt"), b"f", None, true).unwrap();
        session.copy(&at("a"), &at("copy")).unwrap();
        assert_eq!(std::fs::read(dir.path().join("copy/b/c/f.txt")).unwrap(), b"f");
        assert!(matches!(session.copy(&at("a"), &at("copy")), Err(Error::Remote(failure)) if failure.code == FailureCode::Exists));
        session.rename(&at("copy/b/c/f.txt"), &at("g.txt"), false).unwrap();
        assert!(matches!(session.rename(&at("a/b/c/f.txt"), &at("g.txt"), false), Err(Error::Remote(failure)) if failure.code == FailureCode::Exists));
        session.rename(&at("a/b/c/f.txt"), &at("g.txt"), true).unwrap();
        assert!(session.remove(&at("a"), false).is_err(), "a folder with something in it");
        session.remove(&at("a"), true).unwrap();
        session.remove(&at("g.txt"), false).unwrap();
        let mut left: Vec<_> = session.list(&remote(dir.path())).unwrap().into_iter().map(|entry| entry.name).collect();
        left.sort();
        assert_eq!(left, ["copy"]);
    }

    fn shell(script: &str, stdin: bool) -> Exec {
        let (program, flag) = if cfg!(windows) { ("cmd", "/C") } else { ("sh", "-c") };
        Exec { program: program.into(), args: vec![flag.into(), script.into()], stdin, ..Exec::default() }
    }

    #[test]
    fn a_program_gives_its_output_and_exit_code() {
        let session = local();
        let finished = session.output(shell("echo out& echo err 1>&2& exit 3", false)).unwrap();
        assert_eq!(String::from_utf8_lossy(&finished.stdout).trim(), "out");
        assert_eq!(String::from_utf8_lossy(&finished.stderr).trim(), "err");
        assert_eq!(finished.code, Some(3));
    }

    #[test]
    fn a_program_that_cannot_start_is_an_error() {
        let session = local();
        let started = session.output(Exec { program: "fenix-no-such-program-0123".into(), ..Exec::default() });
        assert!(matches!(started, Err(Error::Remote(failure)) if failure.code == FailureCode::NotFound));
    }

    #[test]
    fn stdin_reaches_a_program_and_its_end_ends_it() {
        let session = local();
        let sorter = if cfg!(windows) { "sort" } else { "cat" };
        let process = session.exec(shell(sorter, true)).unwrap();
        let input = process.input();
        input.write(b"hello\n").unwrap();
        input.close().unwrap();
        let finished = process.finish().unwrap();
        assert_eq!(String::from_utf8_lossy(&finished.stdout).trim(), "hello");
        assert_eq!(finished.code, Some(0));
    }

    #[test]
    fn requests_run_side_by_side() {
        let session = Arc::new(local());
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.txt");
        std::fs::write(&file, vec![b'x'; 200_000]).unwrap();
        let path = remote(&file);
        let readers: Vec<_> = (0..8)
            .map(|_| {
                let (session, path) = (session.clone(), path.clone());
                thread::spawn(move || session.read(&path).unwrap().0.len())
            })
            .collect();
        for reader in readers {
            assert_eq!(reader.join().unwrap(), 200_000);
        }
    }

    #[test]
    fn a_lost_connection_fails_requests_instead_of_hanging() {
        let (to_helper_read, to_helper_write) = io::pipe().unwrap();
        let (from_helper_read, mut from_helper_write) = io::pipe().unwrap();
        let hello = serde_json::to_string(&fenix_remote::server::hello()).unwrap();
        writeln!(from_helper_write, "{BANNER}{hello}").unwrap();
        let session = Session::start(from_helper_read, to_helper_write).unwrap();
        // The far end goes away without a word.
        drop(from_helper_write);
        drop(to_helper_read);
        assert!(matches!(session.stat("/"), Err(Error::Lost)));
        assert!(session.lost());
    }
}
