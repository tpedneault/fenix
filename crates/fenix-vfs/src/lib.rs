//! Where a file is: on this machine, or on one Fenix is connected to.
//!
//! A file on a remote machine is still a `PathBuf`, under a root no
//! local file can have:
//!
//! ```text
//! \\?\fenix-ssh\<host>\home\tom\proj\src\main.rs     on Windows
//! /fenix-ssh/<host>/home/tom/proj/src/main.rs        elsewhere
//! ```
//!
//! so everything that names a file by its path (buffers, tabs, recent
//! files, sessions, recovery) goes on working. What *touches* a file
//! comes through this crate's functions instead of `std::fs`: a local
//! path goes to `std::fs` as it always did, a remote one to the machine
//! [`mount`]ed for its host. `std::fs` given a remote path fails at once
//! with "not found", and can never touch a local file by mistake.
//!
//! [`path`] builds and takes apart remote paths. Never `join` text that
//! came from a remote machine onto one: see [`path::child`].

pub mod path;
pub mod testing;

use std::collections::HashMap;
use std::fmt;
use std::io;
use std::path::Path;
use std::sync::{Arc, OnceLock, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub use path::{child, display, host_of, is_remote, join, remote, split};

/// What tells one state of a file from the next: what a save is held
/// against, to not write over someone else's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Version {
    pub len: u64,
    /// Nanoseconds since the Unix epoch.
    pub mtime: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    File,
    Dir,
    /// A device, a socket, a link that leads nowhere.
    Other,
}

/// A file or folder, wherever it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Meta {
    /// Its name alone.
    pub name: String,
    /// What it is, a link followed to its end.
    pub kind: Kind,
    pub symlink: bool,
    pub len: u64,
    pub version: Version,
    /// Unix permission bits; 0 where there are none.
    pub mode: u32,
}

impl Meta {
    pub fn is_dir(&self) -> bool {
        self.kind == Kind::Dir
    }

    pub fn is_file(&self) -> bool {
        self.kind == Kind::File
    }

    pub fn modified(&self) -> SystemTime {
        UNIX_EPOCH + Duration::from_nanos(self.version.mtime.max(0) as u64)
    }
}

/// How a write treats what's there.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Over {
    /// Whatever is there is replaced.
    #[default]
    Anything,
    /// Only the file as it was last seen: [`Changed`] otherwise.
    Version(Version),
    /// Nothing: fails with `AlreadyExists` if there's a file.
    Nothing,
}

/// A save found the file changed since it was read. Inside the
/// `io::Error` a checked write fails with: see [`changed`].
#[derive(Debug)]
pub struct Changed(pub String);

impl fmt::Display for Changed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Changed {}

/// Whether a write failed because the file changed under it.
pub fn changed(error: &io::Error) -> bool {
    error.get_ref().is_some_and(|inner| inner.is::<Changed>())
}

/// What a program run to its end gave.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Ran {
    /// Its exit code; `None` when a signal ended it.
    pub code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl Ran {
    pub fn success(&self) -> bool {
        self.code == Some(0)
    }
}

/// A program to run on a machine, to its end.
#[derive(Debug, Clone, Copy, Default)]
pub struct Run<'a> {
    pub program: &'a str,
    pub args: &'a [&'a str],
    /// Variables set over the machine's own.
    pub env: &'a [(&'a str, &'a str)],
    pub stdin: Option<&'a [u8]>,
}

/// A program to run in a terminal on a machine.
#[derive(Debug, Clone, Copy, Default)]
pub struct Shell<'a> {
    /// What to run; none for the user's own shell there, as a login
    /// shell.
    pub program: Option<&'a str>,
    pub args: &'a [String],
    /// Variables set over the machine's own.
    pub env: &'a [(&'a str, &'a str)],
    pub rows: u16,
    pub cols: u16,
}

/// A terminal on a machine: what it prints, what's typed into it, and
/// the rest.
pub struct Terminal {
    /// What the program prints, to its end.
    pub reader: Box<dyn io::Read + Send>,
    /// What's typed. A write may wait on the network: whoever types
    /// from a thread that draws queues them.
    pub writer: Box<dyn io::Write + Send>,
    pub control: Box<dyn TerminalControl>,
}

/// A terminal's program: its size, its end.
pub trait TerminalControl: Send + Sync {
    fn resize(&self, rows: u16, cols: u16) -> io::Result<()>;
    /// `None` while it runs; how it ended once it has (`Some(None)`
    /// when a signal ended it, or the connection went).
    fn exited(&self) -> Option<Option<i32>>;
    /// Ends it, and all it started.
    fn kill(&self);
}

/// A search through a machine's files, as ripgrep does one. Paths are
/// the machine's own.
#[derive(Debug, Clone, Copy, Default)]
pub struct Search<'a> {
    /// Folders walked, leaving out what `.gitignore` and the like do.
    pub folders: &'a [String],
    /// Files searched whatever the ignore files say; a folder among them
    /// is walked.
    pub files: &'a [String],
    /// A regex, as the `regex` crate reads it.
    pub pattern: &'a str,
    /// Hidden files, and the ones the ignore files leave out, too.
    pub hidden: bool,
    pub globs: &'a [String],
    /// Lines given around each match.
    pub context: usize,
    /// Files not to search.
    pub skip: &'a [String],
}

/// A line a search found: a match, or context around one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoundLine {
    /// 1-based.
    pub number: u64,
    pub text: String,
    pub matched: bool,
}

/// How a search through a machine's files ended.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Searched {
    pub files_searched: usize,
    pub binary_skipped: usize,
}

/// What a search's caller is handed as it goes: a file's lines, or
/// `None` now and then while there's nothing yet. It answers whether to
/// go on.
pub type Finding<'a> = dyn FnMut(Option<(String, Vec<FoundLine>)>) -> bool + 'a;

/// A machine's files and programs. Paths are the machine's own:
/// absolute, with `/`.
///
/// Every file call may take as long as the network does, and fails
/// rather than wait without end; none belongs on a thread that draws.
pub trait Remote: Send + Sync {
    /// The user's home folder there.
    fn home(&self) -> String;
    /// Runs a program in `cwd` there and waits for it, however long it
    /// takes: an error only if it couldn't be started or the connection
    /// went.
    fn run(&self, cwd: &str, run: Run<'_>) -> io::Result<Ran>;
    fn stat(&self, path: &str) -> io::Result<Meta>;
    fn list(&self, path: &str) -> io::Result<Vec<Meta>>;
    /// A file's contents, and the file as it was when read.
    fn read(&self, path: &str) -> io::Result<(Vec<u8>, Meta)>;
    /// Replaces a file's contents, whole or not at all.
    fn write(&self, path: &str, contents: &[u8], over: Over) -> io::Result<Meta>;
    fn rename(&self, from: &str, to: &str, replace: bool) -> io::Result<()>;
    /// Deletes for good: a remote machine has no bin Fenix can use.
    fn remove(&self, path: &str, recursive: bool) -> io::Result<()>;
    fn create_dir(&self, path: &str, parents: bool) -> io::Result<()>;
    /// Copies within the machine.
    fn copy(&self, from: &str, to: &str) -> io::Result<()>;
    /// Starts a program in a terminal there, in `cwd`.
    fn terminal(&self, cwd: &str, shell: Shell<'_>) -> io::Result<Terminal> {
        let _ = (cwd, shell);
        Err(io::Error::new(io::ErrorKind::Unsupported, "this machine has no terminals"))
    }
    /// Searches files there, handing each file's lines to `found` as
    /// they come; `found` answering no stops it.
    fn search(&self, search: Search<'_>, found: &mut Finding<'_>) -> io::Result<Searched> {
        let _ = (search, found);
        Err(io::Error::new(io::ErrorKind::Unsupported, "this machine can't search its files"))
    }
}

fn mounts() -> &'static RwLock<HashMap<String, Arc<dyn Remote>>> {
    static MOUNTS: OnceLock<RwLock<HashMap<String, Arc<dyn Remote>>>> = OnceLock::new();
    MOUNTS.get_or_init(RwLock::default)
}

/// Makes `host`'s files reachable under its remote paths.
pub fn mount(host: &str, remote: Arc<dyn Remote>) {
    mounts().write().unwrap_or_else(|poisoned| poisoned.into_inner()).insert(host.to_string(), remote);
}

pub fn unmount(host: &str) {
    mounts().write().unwrap_or_else(|poisoned| poisoned.into_inner()).remove(host);
}

/// The hosts mounted now.
pub fn mounted() -> Vec<String> {
    mounts().read().unwrap_or_else(|poisoned| poisoned.into_inner()).keys().cloned().collect()
}

/// A path's machine and its path there; `None` for a local path.
fn resolve(path: &Path) -> io::Result<Option<(Arc<dyn Remote>, String)>> {
    let Some((host, there)) = split(path) else { return Ok(None) };
    let mounted = mounts().read().unwrap_or_else(|poisoned| poisoned.into_inner()).get(&host).cloned();
    match mounted {
        Some(remote) => Ok(Some((remote, there))),
        None => Err(io::Error::new(io::ErrorKind::NotConnected, format!("not connected to {host}"))),
    }
}

/// Runs a program in `dir`, when `dir` is on a remote machine: `None`
/// for a local folder, which the caller runs its own way.
pub fn run_there(dir: &Path, run: Run<'_>) -> Option<io::Result<Ran>> {
    match resolve(dir) {
        Ok(Some((remote, there))) => Some(remote.run(&there, run)),
        Ok(None) => None,
        Err(error) => Some(Err(error)),
    }
}

/// Starts a program in a terminal in `dir`, when `dir` is on a remote
/// machine: `None` for a local folder, whose terminal is the caller's.
pub fn terminal(dir: &Path, shell: Shell<'_>) -> Option<io::Result<Terminal>> {
    match resolve(dir) {
        Ok(Some((remote, there))) => Some(remote.terminal(&there, shell)),
        Ok(None) => None,
        Err(error) => Some(Err(error)),
    }
}

/// What to search on a remote machine: [`Search`], with this side's
/// paths.
#[derive(Debug, Clone, Default)]
pub struct SearchPaths {
    pub folders: Vec<std::path::PathBuf>,
    pub files: Vec<std::path::PathBuf>,
    pub skip: Vec<std::path::PathBuf>,
}

/// Searches on the remote machine `paths` are on, handing each file
/// found (as a remote path) to `found`: `None` when they're local, for
/// the caller to search its own way. Every path must be on the one
/// machine.
pub fn search(paths: &SearchPaths, search: Search<'_>, found: &mut dyn FnMut(Option<(std::path::PathBuf, Vec<FoundLine>)>) -> bool) -> Option<io::Result<Searched>> {
    let first = paths.folders.iter().chain(&paths.files).next()?;
    let host = host_of(first)?;
    let there = |list: &[std::path::PathBuf]| -> io::Result<Vec<String>> {
        list.iter()
            .map(|path| match split(path) {
                Some((on, there)) if on == host => Ok(there),
                _ => Err(io::Error::new(io::ErrorKind::Unsupported, format!("{} isn't on {host}: a search stays on one machine", display(path)))),
            })
            .collect()
    };
    let mut run = || -> io::Result<Searched> {
        let Some((machine, _)) = resolve(first)? else { unreachable!("a remote path resolves to its machine") };
        let (folders, files) = (there(&paths.folders)?, there(&paths.files)?);
        // What isn't on this machine can't be found on it: no need to say.
        let skip: Vec<String> = paths.skip.iter().filter_map(|path| split(path).filter(|(on, _)| *on == host).map(|(_, there)| there)).collect();
        let search = Search { folders: &folders, files: &files, skip: &skip, ..search };
        machine.search(search, &mut |file| found(file.map(|(path, lines)| (remote(&host, &path), lines))))
    };
    Some(run())
}

/// The home folder of the machine `host` is mounted for, as a remote
/// path.
pub fn home(host: &str) -> io::Result<std::path::PathBuf> {
    let mounted = mounts().read().unwrap_or_else(|poisoned| poisoned.into_inner()).get(host).cloned();
    match mounted {
        Some(machine) => Ok(remote(host, &machine.home())),
        None => Err(io::Error::new(io::ErrorKind::NotConnected, format!("not connected to {host}"))),
    }
}

pub(crate) fn local_meta(path: &Path, name: String) -> io::Result<Meta> {
    let own = std::fs::symlink_metadata(path)?;
    let symlink = own.file_type().is_symlink();
    let (meta, dangling) = match symlink.then(|| std::fs::metadata(path)) {
        Some(Ok(target)) => (target, false),
        Some(Err(_)) => (own, true),
        None => (own, false),
    };
    let kind = match () {
        () if dangling => Kind::Other,
        () if meta.is_dir() => Kind::Dir,
        () if meta.is_file() => Kind::File,
        () => Kind::Other,
    };
    let mtime = meta.modified().ok().and_then(|time| time.duration_since(UNIX_EPOCH).ok()).map_or(0, |since| since.as_nanos() as i64);
    #[cfg(unix)]
    let mode = std::os::unix::fs::PermissionsExt::mode(&meta.permissions()) & 0o7777;
    #[cfg(not(unix))]
    let mode = 0;
    Ok(Meta { name, kind, symlink, len: meta.len(), version: Version { len: meta.len(), mtime }, mode })
}

fn name_of(path: &Path) -> String {
    path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default()
}

/// What `path` is. A link is followed.
pub fn metadata(path: &Path) -> io::Result<Meta> {
    match resolve(path)? {
        Some((remote, there)) => remote.stat(&there),
        None => local_meta(path, name_of(path)),
    }
}

/// Whether there is anything at `path`. A remote machine that can't be
/// asked counts as no.
pub fn exists(path: &Path) -> bool {
    metadata(path).is_ok()
}

/// A folder's entries, in no order.
pub fn list(path: &Path) -> io::Result<Vec<Meta>> {
    match resolve(path)? {
        Some((remote, there)) => remote.list(&there),
        None => {
            let mut entries = Vec::new();
            for item in std::fs::read_dir(path)? {
                let item = item?;
                if let Ok(meta) = local_meta(&item.path(), item.file_name().to_string_lossy().into_owned()) {
                    entries.push(meta);
                }
            }
            Ok(entries)
        }
    }
}

pub fn read(path: &Path) -> io::Result<Vec<u8>> {
    read_with_meta(path).map(|(bytes, _)| bytes)
}

/// A file's contents, and the file as it was when read: what a later
/// [`Over::Version`] write is held against.
pub fn read_with_meta(path: &Path) -> io::Result<(Vec<u8>, Meta)> {
    match resolve(path)? {
        Some((remote, there)) => remote.read(&there),
        None => {
            // The look first: a change in between is then caught by a
            // checked write, where the other order would miss it.
            let meta = local_meta(path, name_of(path))?;
            Ok((std::fs::read(path)?, meta))
        }
    }
}

/// Replaces a file's contents. On a remote machine: whole or not at all,
/// through a file beside it. On this one: `std::fs::write`, and callers
/// with more to lose use `fenix-storage`'s staged write themselves.
pub fn write(path: &Path, contents: &[u8], over: Over) -> io::Result<Meta> {
    match resolve(path)? {
        Some((remote, there)) => remote.write(&there, contents, over),
        None => {
            match over {
                Over::Anything => std::fs::write(path, contents)?,
                Over::Version(expected) => {
                    if local_meta(path, String::new()).ok().map(|meta| meta.version) != Some(expected) {
                        return Err(io::Error::other(Changed("the file changed since it was read".into())));
                    }
                    std::fs::write(path, contents)?;
                }
                Over::Nothing => {
                    use std::io::Write;
                    std::fs::OpenOptions::new().write(true).create_new(true).open(path)?.write_all(contents)?;
                }
            }
            local_meta(path, name_of(path))
        }
    }
}

/// Moves or renames. Both ends on one machine: a move between machines
/// is a copy and a delete, which is the caller's to decide.
pub fn rename(from: &Path, to: &Path, replace: bool) -> io::Result<()> {
    match (resolve(from)?, split(to)) {
        (Some((remote, there)), Some((to_host, to_there))) if host_of(from).as_deref() == Some(to_host.as_str()) => remote.rename(&there, &to_there, replace),
        (None, None) => {
            if !replace && std::fs::symlink_metadata(to).is_ok() {
                return Err(io::Error::new(io::ErrorKind::AlreadyExists, format!("{} is already there", to.display())));
            }
            std::fs::rename(from, to)
        }
        _ => Err(io::Error::new(io::ErrorKind::Unsupported, "a rename can't cross from one machine to another")),
    }
}

/// Deletes a file or an empty folder; with `recursive`, a folder and all
/// in it. For good, on either kind of machine: the Recycle Bin is
/// `fenix-fs`'s, and local only.
pub fn remove(path: &Path, recursive: bool) -> io::Result<()> {
    match resolve(path)? {
        Some((remote, there)) => remote.remove(&there, recursive),
        None => {
            let meta = std::fs::symlink_metadata(path)?;
            if !meta.is_dir() {
                std::fs::remove_file(path)
            } else if recursive {
                std::fs::remove_dir_all(path)
            } else {
                std::fs::remove_dir(path)
            }
        }
    }
}

/// Copies a file, or a folder and all in it, to a place where nothing
/// is. Within one remote machine it's done there; between machines, or
/// between one and this one, each file is read from the one and written
/// to the other. `go_on` is asked before each file, and stops the copy
/// with what's done left as it is.
pub fn copy(from: &Path, to: &Path, go_on: &dyn Fn() -> bool) -> io::Result<()> {
    if let (Some((host, there)), Some((to_host, to_there))) = (split(from), split(to)) {
        if host == to_host {
            return match resolve(from)? {
                Some((machine, _)) => machine.copy(&there, &to_there),
                None => unreachable!("a remote path resolves to its machine"),
            };
        }
    }
    if !go_on() {
        return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
    }
    let meta = metadata(from)?;
    if !meta.is_dir() {
        let bytes = read(from)?;
        return write(to, &bytes, Over::Nothing).map(drop);
    }
    if exists(to) {
        return Err(io::Error::new(io::ErrorKind::AlreadyExists, format!("{} is already there", display(to))));
    }
    create_dir_all(to)?;
    for entry in list(from)? {
        // Onto this machine, a name has to be one that stays in the
        // folder it's put in. What a remote machine calls its files is
        // its own business there, and not a way to write elsewhere here.
        if !is_remote(to) && !path::is_local_name(&entry.name) {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("{:?} in {} can't be a file's name on this machine", entry.name, display(from))));
        }
        copy(&child(from, &entry.name), &child(to, &entry.name), go_on)?;
    }
    Ok(())
}

pub fn create_dir_all(path: &Path) -> io::Result<()> {
    match resolve(path)? {
        Some((remote, there)) => remote.create_dir(&there, true),
        None => std::fs::create_dir_all(path),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mounted_folder(host: &str) -> (tempfile::TempDir, Arc<testing::Folder>) {
        let dir = tempfile::tempdir().unwrap();
        let folder = testing::Folder::mount(host, dir.path());
        (dir, folder)
    }

    #[test]
    fn a_program_runs_on_the_machine_its_folder_is_on() {
        let (dir, _folder) = mounted_folder("box-run");
        std::fs::write(dir.path().join("here.txt"), b"x").unwrap();
        assert!(run_there(dir.path(), Run::default()).is_none(), "a local folder is the caller's to run in");
        let (program, args): (&str, &[&str]) = if cfg!(windows) { ("cmd", &["/C", "dir /b"]) } else { ("ls", &[]) };
        let ran = run_there(&remote("box-run", "/"), Run { program, args, ..Run::default() }).unwrap().unwrap();
        assert!(ran.success() && String::from_utf8_lossy(&ran.stdout).contains("here.txt"), "{ran:?}");
        assert_eq!(home("box-run").unwrap(), remote("box-run", "/"));
        assert_eq!(run_there(&remote("nobody", "/"), Run::default()).unwrap().unwrap_err().kind(), io::ErrorKind::NotConnected);
    }

    #[test]
    fn a_remote_path_goes_to_its_machine_and_a_local_one_to_the_disk() {
        let (dir, folder) = mounted_folder("box-a");
        std::fs::create_dir(dir.path().join("src")).unwrap();
        let file = remote("box-a", "/src/main.rs");

        let written = write(&file, b"fn main() {}", Over::Nothing).unwrap();
        assert_eq!(written.len, 12);
        assert_eq!(std::fs::read(dir.path().join("src/main.rs")).unwrap(), b"fn main() {}");
        assert_eq!(read(&file).unwrap(), b"fn main() {}");
        assert!(metadata(&file).unwrap().is_file());
        assert_eq!(list(&remote("box-a", "/src")).unwrap().len(), 1);
        assert!(folder.calls.lock().unwrap().iter().all(|path| path.starts_with("/src")));

        // The same calls on a local path never ask the machine.
        let before = folder.calls.lock().unwrap().len();
        let local = dir.path().join("local.txt");
        write(&local, b"here", Over::Anything).unwrap();
        assert_eq!(read(&local).unwrap(), b"here");
        assert_eq!(folder.calls.lock().unwrap().len(), before);
    }

    #[test]
    fn a_checked_write_fails_as_changed_and_says_so() {
        let (dir, _folder) = mounted_folder("box-b");
        let file = remote("box-b", "/a.txt");
        write(&file, b"one", Over::Anything).unwrap();
        let (_, seen) = read_with_meta(&file).unwrap();
        std::fs::write(dir.path().join("a.txt"), b"someone else's").unwrap();

        let refused = write(&file, b"mine", Over::Version(seen.version)).unwrap_err();
        assert!(changed(&refused), "{refused}");
        assert!(!changed(&io::Error::other("anything else")));
        assert_eq!(write(&file, b"again", Over::Nothing).unwrap_err().kind(), io::ErrorKind::AlreadyExists);
    }

    #[test]
    fn a_machine_not_connected_is_an_error_not_a_local_file() {
        let file = remote("nobody-home", "/etc/hosts");
        assert_eq!(read(&file).unwrap_err().kind(), io::ErrorKind::NotConnected);
        assert!(!exists(&file));
        // And `std::fs` finds nothing under the reserved root.
        assert_eq!(std::fs::read(&file).unwrap_err().kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn a_folder_is_copied_to_a_machine_and_back() {
        let (dir, _folder) = mounted_folder("box-copy");
        let here = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(here.path().join("proj/src")).unwrap();
        std::fs::write(here.path().join("proj/src/main.rs"), b"fn main() {}").unwrap();
        std::fs::write(here.path().join("proj/a:b"), b"odd").ok();

        copy(&here.path().join("proj"), &remote("box-copy", "/up"), &|| true).unwrap();
        assert_eq!(std::fs::read(dir.path().join("up/src/main.rs")).unwrap(), b"fn main() {}");
        assert_eq!(copy(&here.path().join("proj"), &remote("box-copy", "/up"), &|| true).unwrap_err().kind(), io::ErrorKind::AlreadyExists);
        copy(&remote("box-copy", "/up"), &remote("box-copy", "/beside"), &|| true).unwrap();
        copy(&remote("box-copy", "/beside"), &here.path().join("back"), &|| true).unwrap();
        assert_eq!(std::fs::read(here.path().join("back/src/main.rs")).unwrap(), b"fn main() {}");
        assert_eq!(copy(&remote("box-copy", "/up"), &here.path().join("stopped"), &|| false).unwrap_err().kind(), io::ErrorKind::Interrupted);
    }

    /// A machine that lies about what a folder holds.
    struct Liar(&'static str);

    impl Remote for Liar {
        fn home(&self) -> String {
            "/".into()
        }
        fn run(&self, _: &str, _: Run<'_>) -> io::Result<Ran> {
            Err(io::ErrorKind::Unsupported.into())
        }
        fn stat(&self, path: &str) -> io::Result<Meta> {
            let dir = path == "/loot";
            Ok(Meta { name: "loot".into(), kind: if dir { Kind::Dir } else { Kind::File }, symlink: false, len: 4, version: Version { len: 4, mtime: 0 }, mode: 0o644 })
        }
        fn list(&self, _: &str) -> io::Result<Vec<Meta>> {
            Ok(vec![Meta { name: self.0.into(), kind: Kind::File, symlink: false, len: 4, version: Version { len: 4, mtime: 0 }, mode: 0o644 }])
        }
        fn read(&self, path: &str) -> io::Result<(Vec<u8>, Meta)> {
            Ok((b"evil".to_vec(), self.stat(path)?))
        }
        fn write(&self, _: &str, _: &[u8], _: Over) -> io::Result<Meta> {
            Err(io::ErrorKind::Unsupported.into())
        }
        fn rename(&self, _: &str, _: &str, _: bool) -> io::Result<()> {
            Ok(())
        }
        fn remove(&self, _: &str, _: bool) -> io::Result<()> {
            Ok(())
        }
        fn create_dir(&self, _: &str, _: bool) -> io::Result<()> {
            Ok(())
        }
        fn copy(&self, _: &str, _: &str) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn a_remote_machine_cannot_name_its_way_out_of_the_folder_copied_into() {
        let here = tempfile::tempdir().unwrap();
        let outside = here.path().join("outside.txt");
        let mut lies = vec!["../outside.txt", "..", "/etc/passwd", "a/b"];
        if cfg!(windows) {
            lies.extend(["..\\outside.txt", "C:\\outside.txt", "nul"]);
        }
        for (i, lie) in lies.into_iter().enumerate() {
            let host = format!("liar-{i}");
            mount(&host, Arc::new(Liar(lie)));
            let into = here.path().join("into").join(i.to_string());
            std::fs::create_dir_all(into.parent().unwrap()).unwrap();
            let refused = copy(&remote(&host, "/loot"), &into, &|| true).unwrap_err();
            assert_eq!(refused.kind(), io::ErrorKind::InvalidInput, "{lie}: {refused}");
            assert!(!outside.exists(), "{lie} wrote outside the folder");
            unmount(&host);
        }
    }

    #[test]
    fn a_rename_stays_on_one_machine() {
        let (dir, _folder) = mounted_folder("box-c");
        let (_other_dir, _other) = mounted_folder("box-d");
        std::fs::write(dir.path().join("a"), b"x").unwrap();
        rename(&remote("box-c", "/a"), &remote("box-c", "/b"), false).unwrap();
        assert!(dir.path().join("b").exists());
        let crossing = rename(&remote("box-c", "/b"), &remote("box-d", "/b"), false).unwrap_err();
        assert_eq!(crossing.kind(), io::ErrorKind::Unsupported);
        assert_eq!(rename(&remote("box-c", "/b"), &dir.path().join("c"), false).unwrap_err().kind(), io::ErrorKind::Unsupported);
        remove(&remote("box-c", "/b"), false).unwrap();
        assert!(!dir.path().join("b").exists());
    }
}
