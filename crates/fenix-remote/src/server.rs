//! The helper's side: answers requests read from one stream on another,
//! until the first ends.
//!
//! Nothing here outlives the connection. When the input ends, every
//! program started over it is killed with all it started, and `serve`
//! returns.

use std::collections::HashMap;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::UNIX_EPOCH;

use crate::{Entry, Exec, Exit, Failure, FailureCode, FileKind, Frame, Hello, Kind, Request, Version, MAX_FRAME, PROTOCOL};

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

#[derive(Clone)]
struct Out(Arc<Mutex<Box<dyn Write + Send>>>);

impl Out {
    /// Sends a frame whole. A failure means the connection is gone, which
    /// the reading side finds out for itself.
    fn send(&self, frame: Frame) {
        let mut out = self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let _ = frame.write(&mut *out).and_then(|()| out.flush());
    }

    fn fail(&self, id: u32, error: &io::Error) {
        self.send(Frame::json(Kind::Err, id, &failure(error)));
    }
}

fn failure(error: &io::Error) -> Failure {
    let code = match error.kind() {
        io::ErrorKind::NotFound => FailureCode::NotFound,
        io::ErrorKind::PermissionDenied => FailureCode::Denied,
        _ => FailureCode::Other,
    };
    Failure { code, message: error.to_string() }
}

/// A started program: where its stdin goes, and what to kill.
struct Running {
    stdin: Option<Sender<Vec<u8>>>,
    pid: u32,
}

type Programs = Arc<Mutex<HashMap<u32, Running>>>;

/// Serves requests from `input`, answering on `output`, until `input`
/// ends.
pub fn serve(mut input: impl Read, output: impl Write + Send + 'static) -> io::Result<()> {
    let out = Out(Arc::new(Mutex::new(Box::new(output))));
    let programs: Programs = Arc::default();
    // A write's request, waiting for the body that follows it.
    let mut writes: HashMap<u32, (String, Option<Version>)> = HashMap::new();

    let result = loop {
        let frame = match Frame::read(&mut input) {
            Ok(Some(frame)) => frame,
            Ok(None) => break Ok(()),
            Err(error) => break Err(error),
        };
        let id = frame.id;
        match frame.kind {
            Kind::Request => match serde_json::from_slice::<Request>(&frame.payload) {
                Ok(Request::Write { path, expect }) => {
                    writes.insert(id, (path, expect));
                }
                Ok(Request::Exec(exec)) => start(id, exec, &out, &programs),
                Ok(request) => {
                    let out = out.clone();
                    thread::spawn(move || answer(id, request, &out));
                }
                Err(error) => out.send(Frame::json(Kind::Err, id, &Failure { code: FailureCode::Other, message: format!("not a request: {error}") })),
            },
            Kind::Body => {
                if let Some((path, expect)) = writes.remove(&id) {
                    let out = out.clone();
                    thread::spawn(move || match write(Path::new(&path), &frame.payload, expect, id) {
                        Ok(entry) => out.send(Frame::json(Kind::Ok, id, &entry)),
                        Err(failure) => out.send(Frame::json(Kind::Err, id, &failure)),
                    });
                } else if let Some(stdin) = programs.lock().unwrap().get(&id).and_then(|running| running.stdin.clone()) {
                    let _ = stdin.send(frame.payload);
                }
            }
            Kind::BodyEnd => {
                if let Some(running) = programs.lock().unwrap().get_mut(&id) {
                    running.stdin = None;
                }
            }
            Kind::Kill => {
                if let Some(running) = programs.lock().unwrap().get(&id) {
                    kill_tree(running.pid);
                }
            }
            // Answers travel the other way only.
            _ => {}
        }
    };

    for running in programs.lock().unwrap().values() {
        kill_tree(running.pid);
    }
    result
}

fn answer(id: u32, request: Request, out: &Out) {
    let result = match request {
        Request::Stat { path } => entry(Path::new(&path), path.clone()).map(|entry| Frame::json(Kind::Ok, id, &entry)),
        Request::List { path } => list(Path::new(&path)).map(|entries| Frame::json(Kind::Ok, id, &entries)),
        Request::Read { path } => read(Path::new(&path), path.clone()).map(|(bytes, entry)| {
            out.send(Frame::new(Kind::Bytes, id, bytes));
            Frame::json(Kind::Ok, id, &entry)
        }),
        Request::Write { .. } | Request::Exec(_) => return,
    };
    match result {
        Ok(frame) => out.send(frame),
        Err(error) => out.fail(id, &error),
    }
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

fn read(path: &Path, name: String) -> io::Result<(Vec<u8>, Entry)> {
    let before = entry(path, name)?;
    if before.len as usize > MAX_FRAME {
        return Err(io::Error::other(format!("{} bytes is more than can be sent at once", before.len)));
    }
    let bytes = fs::read(path)?;
    Ok((bytes, before))
}

/// Replaces `path`'s contents: a complete, synced file beside it first,
/// then a rename over it, so there is never half a file under the name.
fn write(path: &Path, contents: &[u8], expect: Option<Version>, id: u32) -> Result<Entry, Failure> {
    let name = path.to_string_lossy().into_owned();
    // A link is written through, not replaced by a file.
    let target: PathBuf = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let existing = entry(&target, name.clone()).ok();
    if let Some(expected) = expect {
        let now = existing.as_ref().map(Entry::version);
        if now != Some(expected) {
            let message = match now {
                Some(_) => "the file changed since it was read",
                None => "the file is no longer there",
            };
            return Err(Failure { code: FailureCode::Changed, message: message.to_string() });
        }
    }
    let io = |error: io::Error| failure(&error);
    let folder = target.parent().ok_or_else(|| Failure { code: FailureCode::Other, message: "no folder to write in".into() })?;
    let file_name = target.file_name().unwrap_or_default().to_string_lossy();
    let staged = folder.join(format!(".{file_name}.fenix-{}-{id}.tmp", std::process::id()));
    let written = (|| {
        let mut file = fs::OpenOptions::new().write(true).create_new(true).open(&staged)?;
        file.write_all(contents)?;
        if let Some(existing) = &existing {
            keep_mode(&file, existing.mode)?;
        }
        file.sync_all()?;
        drop(file);
        fs::rename(&staged, &target)
    })();
    if let Err(error) = written {
        let _ = fs::remove_file(&staged);
        return Err(io(error));
    }
    entry(&target, name).map_err(io)
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

fn start(id: u32, exec: Exec, out: &Out, programs: &Programs) {
    let mut command = Command::new(&exec.program);
    command.args(&exec.args).envs(exec.env.iter().map(|(key, value)| (key, value)));
    if let Some(cwd) = &exec.cwd {
        command.current_dir(cwd);
    }
    command.stdin(if exec.stdin { Stdio::piped() } else { Stdio::null() }).stdout(Stdio::piped()).stderr(Stdio::piped());
    own_group(&mut command);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            out.fail(id, &io::Error::new(error.kind(), format!("couldn't run {}: {error}", exec.program)));
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
    programs.lock().unwrap().insert(id, Running { stdin, pid: child.id() });

    let pumps = [
        child.stdout.take().map(|pipe| pump(pipe, Kind::Stdout, id, out.clone())),
        child.stderr.take().map(|pipe| pump(pipe, Kind::Stderr, id, out.clone())),
    ];
    let (out, programs) = (out.clone(), programs.clone());
    thread::spawn(move || {
        // Output first, to its end: `Exit` is the last word on a program.
        for pump in pumps.into_iter().flatten() {
            let _ = pump.join();
        }
        let code = wait(&mut child);
        programs.lock().unwrap().remove(&id);
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
