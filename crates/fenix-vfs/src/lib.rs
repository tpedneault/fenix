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

use std::collections::HashMap;
use std::fmt;
use std::io;
use std::path::Path;
use std::sync::{Arc, OnceLock, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub use path::{child, display, host_of, is_remote, remote, split};

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

/// A machine's files. Paths are the machine's own: absolute, with `/`.
///
/// Every call may take as long as the network does, and fails rather
/// than wait without end; none belongs on a thread that draws.
pub trait Remote: Send + Sync {
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

fn local_meta(path: &Path, name: String) -> io::Result<Meta> {
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

pub fn create_dir_all(path: &Path) -> io::Result<()> {
    match resolve(path)? {
        Some((remote, there)) => remote.create_dir(&there, true),
        None => std::fs::create_dir_all(path),
    }
}

#[cfg(test)]
pub(crate) mod testing {
    use super::*;
    use std::sync::Mutex;

    /// A machine that is a folder on this one.
    pub struct Folder {
        pub root: std::path::PathBuf,
        pub calls: Mutex<Vec<String>>,
    }

    impl Folder {
        fn local(&self, path: &str) -> std::path::PathBuf {
            self.calls.lock().unwrap().push(path.to_string());
            self.root.join(path.trim_start_matches('/'))
        }
    }

    impl Remote for Folder {
        fn stat(&self, path: &str) -> io::Result<Meta> {
            let local = self.local(path);
            local_meta(&local, name_of(&local))
        }
        fn list(&self, path: &str) -> io::Result<Vec<Meta>> {
            super::list(&self.local(path))
        }
        fn read(&self, path: &str) -> io::Result<(Vec<u8>, Meta)> {
            super::read_with_meta(&self.local(path))
        }
        fn write(&self, path: &str, contents: &[u8], over: Over) -> io::Result<Meta> {
            super::write(&self.local(path), contents, over)
        }
        fn rename(&self, from: &str, to: &str, replace: bool) -> io::Result<()> {
            super::rename(&self.local(from), &self.local(to), replace)
        }
        fn remove(&self, path: &str, recursive: bool) -> io::Result<()> {
            super::remove(&self.local(path), recursive)
        }
        fn create_dir(&self, path: &str, _parents: bool) -> io::Result<()> {
            std::fs::create_dir_all(self.local(path))
        }
        fn copy(&self, from: &str, to: &str) -> io::Result<()> {
            std::fs::copy(self.local(from), self.local(to)).map(drop)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mounted_folder(host: &str) -> (tempfile::TempDir, Arc<testing::Folder>) {
        let dir = tempfile::tempdir().unwrap();
        let folder = Arc::new(testing::Folder { root: dir.path().to_path_buf(), calls: Default::default() });
        mount(host, folder.clone());
        (dir, folder)
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
