//! The helper's side: answers requests read from one stream on another,
//! until the first ends.
//!
//! Nothing here outlives the connection. When the input ends, every
//! program started over it is killed with all it started, every file
//! half written is put away, and `serve` returns.

use std::collections::HashMap;
use std::ffi::OsString;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant, UNIX_EPOCH};

use crate::{Entry, Exec, Exit, Failure, FailureCode, FileKind, Frame, Hello, Kind, Pty, Request, Size, Version, ENVIRONMENT, PIECE, PROTOCOL};

/// How long the user's login shell gets to say what its environment is
/// before programs are started with the helper's own.
const SHELL_PATIENCE: Duration = Duration::from_secs(5);

/// Who the helper is, for its first line.
pub fn hello() -> Hello {
    let var = |names: &[&str]| names.iter().find_map(|name| std::env::var(name).ok()).unwrap_or_default();
    Hello {
        protocol: PROTOCOL,
        version: env!("CARGO_PKG_VERSION").to_string(),
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        home: var(&["HOME", "USERPROFILE"]),
        user: var(&["USER", "LOGNAME", "USERNAME"]),
    }
}

/// This process's environment, as `--print-env` prints it.
pub fn environment_line() -> String {
    let pairs: Vec<(String, String)> = std::env::vars_os().map(|(key, value)| (key.to_string_lossy().into_owned(), value.to_string_lossy().into_owned())).collect();
    format!("{ENVIRONMENT}{}", serde_json::to_string(&pairs).expect("strings serialize"))
}

#[derive(Clone)]
struct Out(Arc<Mutex<Box<dyn Write + Send>>>);

impl Out {
    /// Sends a frame whole. A failure means the connection is gone, which
    /// the reading side finds out for itself.
    fn send(&self, frame: Frame) {
        let mut out = self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let _ = frame.write(&mut *out).and_then(|()| out.flush());
    }

    fn ok(&self, id: u32, value: &impl serde::Serialize) {
        self.send(Frame::json(Kind::Ok, id, value));
    }

    fn fail(&self, id: u32, failure: &Failure) {
        self.send(Frame::json(Kind::Err, id, failure));
    }
}

fn failure(error: &io::Error) -> Failure {
    let code = match error.kind() {
        io::ErrorKind::NotFound => FailureCode::NotFound,
        io::ErrorKind::PermissionDenied => FailureCode::Denied,
        io::ErrorKind::AlreadyExists => FailureCode::Exists,
        _ => FailureCode::Other,
    };
    Failure { code, message: error.to_string() }
}

fn failed(code: FailureCode, message: impl Into<String>) -> Failure {
    Failure { code, message: message.into() }
}

/// Something that goes on past its request, and can be fed or stopped.
enum Ongoing {
    Program { stdin: Option<Sender<Vec<u8>>>, pid: u32 },
    /// A program in a terminal: what's typed, and the terminal's end to
    /// give a new size to.
    Terminal { input: Sender<Vec<u8>>, pid: u32, master: Arc<crate::pty::Master> },
    /// A file being written: its pieces, then `None` for the end.
    Upload(Sender<Option<Vec<u8>>>),
    /// A file being read, or a search: set to stop it.
    Stoppable(Arc<AtomicBool>),
}

type Ongoings = Arc<Mutex<HashMap<u32, Ongoing>>>;

type Environment = Vec<(OsString, OsString)>;

/// The environment programs are started with: the user's login shell's,
/// asked once, so that what they installed for themselves is found.
struct LoginEnvironment {
    /// Set once the shell has answered, or failed to.
    found: Arc<OnceLock<Option<Environment>>>,
    until: Instant,
}

impl LoginEnvironment {
    fn ask() -> LoginEnvironment {
        let found = Arc::new(OnceLock::new());
        let finding = found.clone();
        thread::spawn(move || {
            let _ = finding.set(ask_login_shell());
        });
        LoginEnvironment { found, until: Instant::now() + SHELL_PATIENCE }
    }

    /// The login environment, waiting for it while there's still hope.
    fn get(&self) -> Option<&[(OsString, OsString)]> {
        while self.found.get().is_none() && Instant::now() < self.until {
            thread::sleep(Duration::from_millis(10));
        }
        self.found.get().and_then(|found| found.as_deref())
    }
}

/// Has the user's shell, as a login shell, run this very program to print
/// the environment it was given. Whatever the shell's files print is no
/// trouble: only the marked line is read.
#[cfg(unix)]
fn ask_login_shell() -> Option<Environment> {
    let shell = std::env::var_os("SHELL").filter(|shell| !shell.is_empty())?;
    let helper = std::env::current_exe().ok()?;
    // Single quotes mean the same to sh, bash, zsh, fish and csh.
    let quoted = format!("'{}'", helper.to_string_lossy().replace('\'', r"'\''"));
    let mut command = Command::new(shell);
    command.args(["-l", "-c", &format!("{quoted} --print-env")]).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
    own_group(&mut command);
    let mut child = command.spawn().ok()?;
    let pid = child.id();
    let mut stdout = child.stdout.take()?;
    // A shell that waits on something never comes back: it's put down
    // when patience runs out, which ends the read below.
    let done = Arc::new(AtomicBool::new(false));
    let watching = done.clone();
    thread::spawn(move || {
        thread::sleep(SHELL_PATIENCE);
        if !watching.load(Ordering::SeqCst) {
            kill_tree(pid);
        }
    });
    let mut said = Vec::new();
    let _ = stdout.read_to_end(&mut said);
    done.store(true, Ordering::SeqCst);
    let _ = child.wait();
    let said = String::from_utf8_lossy(&said);
    let line = said.lines().find_map(|line| line.strip_prefix(ENVIRONMENT))?;
    let pairs: Vec<(String, String)> = serde_json::from_str(line).ok()?;
    Some(pairs.into_iter().map(|(key, value)| (key.into(), value.into())).collect())
}

#[cfg(not(unix))]
fn ask_login_shell() -> Option<Environment> {
    None
}

/// Serves requests from `input`, answering on `output`, until `input`
/// ends.
pub fn serve(mut input: impl Read, output: impl Write + Send + 'static) -> io::Result<()> {
    let out = Out(Arc::new(Mutex::new(Box::new(output))));
    let ongoing: Ongoings = Arc::default();
    let environment = LoginEnvironment::ask();

    let result = loop {
        let frame = match Frame::read(&mut input) {
            Ok(Some(frame)) => frame,
            Ok(None) => break Ok(()),
            Err(error) => break Err(error),
        };
        let id = frame.id;
        match frame.kind {
            Kind::Request => match serde_json::from_slice::<Request>(&frame.payload) {
                Ok(Request::Write { path, expect, create_new }) => {
                    let (pieces, receiver) = mpsc::channel();
                    ongoing.lock().unwrap().insert(id, Ongoing::Upload(pieces));
                    let (out, ongoing) = (out.clone(), ongoing.clone());
                    thread::spawn(move || {
                        let written = write(Path::new(&path), receiver, expect, create_new, id);
                        ongoing.lock().unwrap().remove(&id);
                        match written {
                            Ok(entry) => out.ok(id, &entry),
                            Err(failure) => out.fail(id, &failure),
                        }
                    });
                }
                Ok(Request::Read { path }) => {
                    let stop = Arc::new(AtomicBool::new(false));
                    ongoing.lock().unwrap().insert(id, Ongoing::Stoppable(stop.clone()));
                    let (out, ongoing) = (out.clone(), ongoing.clone());
                    thread::spawn(move || {
                        let read = read(Path::new(&path), path.clone(), id, &out, &stop);
                        ongoing.lock().unwrap().remove(&id);
                        match read {
                            Ok(entry) => out.ok(id, &entry),
                            Err(failure) => out.fail(id, &failure),
                        }
                    });
                }
                Ok(Request::Exec(exec)) => start(id, exec, &out, &ongoing, environment.get()),
                Ok(Request::Pty(pty)) => start_terminal(id, pty, &out, &ongoing, environment.get()),
                Ok(Request::Search(search)) => {
                    let stop = Arc::new(AtomicBool::new(false));
                    ongoing.lock().unwrap().insert(id, Ongoing::Stoppable(stop.clone()));
                    let (out, ongoing) = (out.clone(), ongoing.clone());
                    thread::spawn(move || {
                        let searched = crate::search::run(&search, &stop, &mut |found| out.send(Frame::json(Kind::Found, id, &found)));
                        ongoing.lock().unwrap().remove(&id);
                        match searched {
                            Ok(searched) => out.ok(id, &searched),
                            Err(failure) => out.fail(id, &failure),
                        }
                    });
                }
                Ok(request) => {
                    let out = out.clone();
                    thread::spawn(move || match answer(request) {
                        Ok(value) => out.ok(id, &value),
                        Err(error) => out.fail(id, &failure(&error)),
                    });
                }
                Err(error) => out.fail(id, &failed(FailureCode::Other, format!("not a request: {error}"))),
            },
            Kind::Body => match ongoing.lock().unwrap().get(&id) {
                Some(Ongoing::Program { stdin: Some(stdin), .. }) => drop(stdin.send(frame.payload)),
                Some(Ongoing::Terminal { input, .. }) => drop(input.send(frame.payload)),
                Some(Ongoing::Upload(pieces)) => drop(pieces.send(Some(frame.payload))),
                _ => {}
            },
            Kind::BodyEnd => match ongoing.lock().unwrap().get_mut(&id) {
                Some(Ongoing::Program { stdin, .. }) => *stdin = None,
                Some(Ongoing::Upload(pieces)) => drop(pieces.send(None)),
                _ => {}
            },
            Kind::Resize => {
                if let (Some(Ongoing::Terminal { master, .. }), Ok(size)) = (ongoing.lock().unwrap().get(&id), serde_json::from_slice::<Size>(&frame.payload)) {
                    let _ = crate::pty::resize(master, size);
                }
            }
            Kind::Kill => {
                let mut ongoing = ongoing.lock().unwrap();
                match ongoing.get(&id) {
                    Some(Ongoing::Program { pid, .. }) => kill_tree(*pid),
                    Some(Ongoing::Terminal { pid, .. }) => {
                        let pid = *pid;
                        thread::spawn(move || hang_up(pid));
                    }
                    Some(Ongoing::Stoppable(stop)) => stop.store(true, Ordering::SeqCst),
                    Some(Ongoing::Upload(_)) | None => {}
                }
                // An upload's pieces end without the word that they're
                // all there: the write gives up and tidies.
                if matches!(ongoing.get(&id), Some(Ongoing::Upload(_))) {
                    ongoing.remove(&id);
                }
            }
            // Answers travel the other way only.
            _ => {}
        }
    };

    // Dropping the uploads' senders ends each write without its file.
    let left: Vec<Ongoing> = ongoing.lock().unwrap().drain().map(|(_, left)| left).collect();
    let terminals: Vec<u32> = left.iter().filter_map(|left| if let Ongoing::Terminal { pid, .. } = left { Some(*pid) } else { None }).collect();
    let hanging_up: Vec<_> = terminals.into_iter().map(|pid| thread::spawn(move || hang_up(pid))).collect();
    for left in left {
        match left {
            Ongoing::Program { pid, .. } => kill_tree(pid),
            Ongoing::Terminal { .. } => {}
            Ongoing::Stoppable(stop) => stop.store(true, Ordering::SeqCst),
            Ongoing::Upload(_) => {}
        }
    }
    for hanging_up in hanging_up {
        let _ = hanging_up.join();
    }
    result
}

fn answer(request: Request) -> io::Result<serde_json::Value> {
    match request {
        Request::Stat { path } => entry(Path::new(&path), path.clone()).map(|entry| json(&entry)),
        Request::List { path } => list(Path::new(&path)).map(|entries| json(&entries)),
        Request::Rename { from, to, replace } => {
            if !replace && fs::symlink_metadata(&to).is_ok() {
                return Err(io::Error::new(io::ErrorKind::AlreadyExists, format!("{to} is already there")));
            }
            fs::rename(&from, &to).map(|()| serde_json::Value::Null)
        }
        Request::Remove { path, recursive } => remove(Path::new(&path), recursive).map(|()| serde_json::Value::Null),
        Request::CreateDir { path, parents } => {
            if parents { fs::create_dir_all(&path) } else { fs::create_dir(&path) }?;
            entry(Path::new(&path), path.clone()).map(|entry| json(&entry))
        }
        Request::Copy { from, to } => {
            if fs::symlink_metadata(&to).is_ok() {
                return Err(io::Error::new(io::ErrorKind::AlreadyExists, format!("{to} is already there")));
            }
            copy(Path::new(&from), Path::new(&to)).map(|()| serde_json::Value::Null)
        }
        Request::Read { .. } | Request::Write { .. } | Request::Exec(_) | Request::Pty(_) | Request::Search(_) => unreachable!("served where they're read"),
    }
}

fn json(value: &impl serde::Serialize) -> serde_json::Value {
    serde_json::to_value(value).expect("protocol types serialize")
}

fn entry(path: &Path, name: String) -> io::Result<Entry> {
    let own = fs::symlink_metadata(path)?;
    let symlink = own.file_type().is_symlink();
    // A link that leads nowhere is still an entry: itself.
    let (meta, dangling) = match symlink.then(|| fs::metadata(path)) {
        Some(Ok(target)) => (target, false),
        Some(Err(_)) => (own, true),
        None => (own, false),
    };
    let kind = if dangling {
        FileKind::Other
    } else if meta.is_dir() {
        FileKind::Dir
    } else if meta.is_file() {
        FileKind::File
    } else {
        FileKind::Other
    };
    let mtime = meta.modified().ok().and_then(|time| time.duration_since(UNIX_EPOCH).ok()).map_or(0, |since| since.as_nanos() as i64);
    Ok(Entry { name, kind, symlink, len: meta.len(), mtime, mode: mode(&meta) })
}

#[cfg(unix)]
fn mode(meta: &fs::Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    meta.permissions().mode() & 0o7777
}

#[cfg(not(unix))]
fn mode(_: &fs::Metadata) -> u32 {
    0
}

fn list(path: &Path) -> io::Result<Vec<Entry>> {
    let mut entries = Vec::new();
    for item in fs::read_dir(path)? {
        let item = item?;
        // Gone between the listing and the look: not there, then.
        if let Ok(entry) = entry(&item.path(), item.file_name().to_string_lossy().into_owned()) {
            entries.push(entry);
        }
    }
    Ok(entries)
}

/// Sends a file a piece at a time, and gives the file as it was when the
/// read began: what a later checked write is held against.
fn read(path: &Path, name: String, id: u32, out: &Out, stop: &AtomicBool) -> Result<Entry, Failure> {
    let io = |error: io::Error| failure(&error);
    let before = entry(path, name).map_err(io)?;
    let mut file = fs::File::open(path).map_err(io)?;
    let mut piece = vec![0u8; PIECE];
    loop {
        if stop.load(Ordering::SeqCst) {
            return Err(failed(FailureCode::Cancelled, "stopped"));
        }
        match file.read(&mut piece).map_err(io)? {
            0 => return Ok(before),
            n => out.send(Frame::new(Kind::Bytes, id, &piece[..n])),
        }
    }
}

/// Replaces `path`'s contents with the pieces received: a complete,
/// synced file beside it first, then a rename over it, so there is never
/// half a file under the name.
fn write(path: &Path, pieces: Receiver<Option<Vec<u8>>>, expect: Option<Version>, create_new: bool, id: u32) -> Result<Entry, Failure> {
    let name = path.to_string_lossy().into_owned();
    // A link is written through, not replaced by a file.
    let target: PathBuf = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let unchanged = |existing: &Option<Entry>| -> Result<(), Failure> {
        if create_new && existing.is_some() {
            return Err(failed(FailureCode::Exists, format!("{name} is already there")));
        }
        let Some(expected) = expect else { return Ok(()) };
        match existing.as_ref().map(Entry::version) {
            Some(now) if now == expected => Ok(()),
            Some(_) => Err(failed(FailureCode::Changed, "the file changed since it was read")),
            None => Err(failed(FailureCode::Changed, "the file is no longer there")),
        }
    };
    let existing = entry(&target, name.clone()).ok();
    unchanged(&existing)?;

    let io = |error: io::Error| failure(&error);
    let folder = target.parent().ok_or_else(|| failed(FailureCode::Other, "no folder to write in"))?;
    let file_name = target.file_name().unwrap_or_default().to_string_lossy();
    let staged = folder.join(format!(".{file_name}.fenix-{}-{id}.tmp", std::process::id()));
    let written = (|| -> Result<(), Failure> {
        let mut file = fs::OpenOptions::new().write(true).create_new(true).open(&staged).map_err(io)?;
        loop {
            match pieces.recv() {
                Ok(Some(piece)) => file.write_all(&piece).map_err(io)?,
                Ok(None) => break,
                // Stopped, or the connection went, before the end.
                Err(_) => return Err(failed(FailureCode::Cancelled, "stopped before the whole file arrived")),
            }
        }
        if let Some(existing) = &existing {
            keep_mode(&file, existing.mode).map_err(io)?;
        }
        file.sync_all().map_err(io)?;
        drop(file);
        // A long upload leaves time for someone else to save.
        unchanged(&entry(&target, name.clone()).ok())?;
        if create_new {
            place_new(&staged, &target).map_err(io)
        } else {
            fs::rename(&staged, &target).map_err(io)
        }
    })();
    if let Err(failure) = written {
        let _ = fs::remove_file(&staged);
        return Err(failure);
    }
    entry(&target, name).map_err(io)
}

/// Gives the staged file its name only if nothing has it: a link fails
/// where a rename would replace.
#[cfg(unix)]
fn place_new(staged: &Path, target: &Path) -> io::Result<()> {
    fs::hard_link(staged, target)?;
    fs::remove_file(staged)
}

#[cfg(not(unix))]
fn place_new(staged: &Path, target: &Path) -> io::Result<()> {
    if target.exists() {
        return Err(io::Error::new(io::ErrorKind::AlreadyExists, "already there"));
    }
    fs::rename(staged, target)
}

#[cfg(unix)]
fn keep_mode(file: &fs::File, mode: u32) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    file.set_permissions(fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
fn keep_mode(_: &fs::File, _: u32) -> io::Result<()> {
    Ok(())
}

fn remove(path: &Path, recursive: bool) -> io::Result<()> {
    // A link to a folder is a link: it goes, the folder stays.
    let meta = fs::symlink_metadata(path)?;
    if !meta.is_dir() {
        fs::remove_file(path)
    } else if recursive {
        fs::remove_dir_all(path)
    } else {
        fs::remove_dir(path)
    }
}

fn copy(from: &Path, to: &Path) -> io::Result<()> {
    let meta = fs::symlink_metadata(from)?;
    if meta.file_type().is_symlink() {
        return copy_link(from, to);
    }
    if !meta.is_dir() {
        return fs::copy(from, to).map(drop);
    }
    fs::create_dir(to)?;
    for item in fs::read_dir(from)? {
        let item = item?;
        copy(&item.path(), &to.join(item.file_name()))?;
    }
    fs::set_permissions(to, meta.permissions())
}

#[cfg(unix)]
fn copy_link(from: &Path, to: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(fs::read_link(from)?, to)
}

#[cfg(not(unix))]
fn copy_link(from: &Path, to: &Path) -> io::Result<()> {
    fs::copy(from, to).map(drop)
}

fn start(id: u32, exec: Exec, out: &Out, ongoing: &Ongoings, environment: Option<&[(OsString, OsString)]>) {
    let mut command = Command::new(&exec.program);
    command.args(&exec.args);
    if let Some(environment) = environment {
        command.env_clear().envs(environment.iter().map(|(key, value)| (key, value)));
    }
    command.envs(exec.env.iter().map(|(key, value)| (key, value)));
    if let Some(cwd) = &exec.cwd {
        command.current_dir(cwd);
    }
    command.stdin(if exec.stdin { Stdio::piped() } else { Stdio::null() }).stdout(Stdio::piped()).stderr(Stdio::piped());
    own_group(&mut command);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            out.fail(id, &failure(&io::Error::new(error.kind(), format!("couldn't run {}: {error}", exec.program))));
            return;
        }
    };

    let stdin = child.stdin.take().map(|mut pipe| {
        let (sender, receiver) = mpsc::channel::<Vec<u8>>();
        // The pipe closes, and the program reads its end of input, when
        // the sender is dropped (`BodyEnd`) and this loop runs out.
        thread::spawn(move || {
            for bytes in receiver {
                if pipe.write_all(&bytes).and_then(|()| pipe.flush()).is_err() {
                    break;
                }
            }
        });
        sender
    });
    ongoing.lock().unwrap().insert(id, Ongoing::Program { stdin, pid: child.id() });

    let pumps = [
        child.stdout.take().map(|pipe| pump(pipe, Kind::Stdout, id, out.clone())),
        child.stderr.take().map(|pipe| pump(pipe, Kind::Stderr, id, out.clone())),
    ];
    let (out, ongoing) = (out.clone(), ongoing.clone());
    thread::spawn(move || {
        // Output first, to its end: `Exit` is the last word on a program.
        for pump in pumps.into_iter().flatten() {
            let _ = pump.join();
        }
        let code = wait(&mut child);
        ongoing.lock().unwrap().remove(&id);
        out.send(Frame::json(Kind::Exit, id, &Exit { code }));
    });
}

/// How long a terminal's last output has, once its program has ended,
/// to come before the word that it ended.
const LAST_OUTPUT: Duration = Duration::from_secs(2);

/// Starts a program in a terminal: the user's shell, as a login shell,
/// unless another is named.
fn start_terminal(id: u32, pty: Pty, out: &Out, ongoing: &Ongoings, environment: Option<&[(OsString, OsString)]>) {
    let shell = || {
        let from_login = environment.and_then(|environment| environment.iter().find(|(key, _)| key == "SHELL").map(|(_, value)| value.clone()));
        from_login.or_else(|| std::env::var_os("SHELL")).filter(|shell| !shell.is_empty()).unwrap_or_else(|| "/bin/sh".into())
    };
    let (program, args) = match pty.program {
        Some(program) => (OsString::from(program), pty.args),
        None => (shell(), vec!["-l".to_string()]),
    };
    let mut command = Command::new(&program);
    command.args(&args);
    if let Some(environment) = environment {
        command.env_clear().envs(environment.iter().map(|(key, value)| (key, value)));
    }
    // What a terminal emulator says it is; Fenix's is xterm's kind.
    command.env("TERM", "xterm-256color").env("COLORTERM", "truecolor");
    command.envs(pty.env.iter().map(|(key, value)| (key, value)));
    if let Some(cwd) = &pty.cwd {
        command.current_dir(cwd);
    }
    let started = match crate::pty::start(command, pty.size) {
        Ok(started) => started,
        Err(error) => {
            let message = format!("couldn't start {} in a terminal: {error}", program.to_string_lossy());
            out.fail(id, &failure(&io::Error::new(error.kind(), message)));
            return;
        }
    };
    let crate::pty::Started { mut child, master, reader, mut writer } = started;
    let (input, typed) = mpsc::channel::<Vec<u8>>();
    thread::spawn(move || {
        for bytes in typed {
            if writer.write_all(&bytes).and_then(|()| writer.flush()).is_err() {
                break;
            }
        }
    });
    let pid = child.id();
    ongoing.lock().unwrap().insert(id, Ongoing::Terminal { input, pid, master: Arc::new(master) });
    let pump = pump(reader, Kind::Stdout, id, out.clone());
    let (out, ongoing) = (out.clone(), ongoing.clone());
    thread::spawn(move || {
        let code = wait(&mut child);
        // What it left running in its session goes with it, as when a
        // terminal is closed; then the pty has no one left to write.
        kill_tree(pid);
        let until = Instant::now() + LAST_OUTPUT;
        while !pump.is_finished() && Instant::now() < until {
            thread::sleep(Duration::from_millis(10));
        }
        ongoing.lock().unwrap().remove(&id);
        out.send(Frame::json(Kind::Exit, id, &Exit { code }));
    });
}

fn wait(child: &mut Child) -> Option<i32> {
    child.wait().ok().and_then(|status| status.code())
}

fn pump(mut pipe: impl Read + Send + 'static, kind: Kind, id: u32, out: Out) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut buffer = vec![0u8; 64 * 1024];
        loop {
            match pipe.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(n) => out.send(Frame::new(kind, id, &buffer[..n])),
            }
        }
    })
}

/// Puts the program at the head of a process group of its own, so that
/// killing the group takes what it started with it.
#[cfg(unix)]
fn own_group(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    command.process_group(0);
}

#[cfg(not(unix))]
fn own_group(_: &mut Command) {}

/// How long a terminal's shell has, once hung up, to pass it on to its
/// jobs and end, before it and its group are killed.
const HANG_UP: Duration = Duration::from_millis(500);

/// Ends a terminal as closing one does: its shell is hung up on, which
/// a shell passes on to the jobs it started (each a process group of
/// its own); what's left of its group after a moment is killed.
#[cfg(unix)]
fn hang_up(pid: u32) {
    // SAFETY: a signal to a process group this helper created.
    unsafe {
        libc::kill(-(pid as i32), libc::SIGHUP);
    }
    let until = Instant::now() + HANG_UP;
    // SAFETY: asks whether the group has anyone left; sends nothing.
    while Instant::now() < until && unsafe { libc::kill(-(pid as i32), 0) } == 0 {
        thread::sleep(Duration::from_millis(10));
    }
    kill_tree(pid);
    // A shell that doesn't pass the hang-up on leaves its jobs running,
    // each in a group of its own but all in the terminal's session.
    for member in session_members(pid) {
        // SAFETY: a signal to a process of the session this helper made.
        unsafe {
            libc::kill(member as i32, libc::SIGKILL);
        }
    }
}

/// The processes of the session `leader` began (what was started in a
/// terminal and stayed there), as Linux's `/proc` tells them.
#[cfg(target_os = "linux")]
fn session_members(leader: u32) -> Vec<u32> {
    let Ok(entries) = fs::read_dir("/proc") else { return Vec::new() };
    entries
        .flatten()
        .filter_map(|entry| entry.file_name().to_str()?.parse::<u32>().ok())
        .filter(|&pid| {
            // `pid (name) state ppid pgrp session ...`: the name may hold
            // spaces and brackets, so the fields are counted after it.
            let Ok(stat) = fs::read_to_string(format!("/proc/{pid}/stat")) else { return false };
            let after = stat.rfind(')').map_or("", |end| &stat[end + 1..]);
            after.split_whitespace().nth(3).and_then(|session| session.parse::<u32>().ok()) == Some(leader)
        })
        .collect()
}

#[cfg(all(unix, not(target_os = "linux")))]
fn session_members(_: u32) -> Vec<u32> {
    Vec::new()
}

#[cfg(not(unix))]
fn hang_up(pid: u32) {
    kill_tree(pid);
}

#[cfg(unix)]
fn kill_tree(pid: u32) {
    // SAFETY: a signal to a process group this helper created.
    unsafe {
        libc::kill(-(pid as i32), libc::SIGKILL);
    }
}

#[cfg(not(unix))]
fn kill_tree(pid: u32) {
    let _ = Command::new("taskkill").args(["/T", "/F", "/PID", &pid.to_string()]).stdout(Stdio::null()).stderr(Stdio::null()).status();
}
