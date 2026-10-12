//! The control endpoint on Linux: a Unix socket in a private directory,
//! with a lock file deciding who the server is. See the parent module.

use std::ffi::OsStr;
use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};

use super::{BindError, Endpoint};

/// The server's end.
pub struct Listener {
    listener: UnixListener,
    socket: PathBuf,
    /// Held, locked, for the server's life: the lock is what makes this
    /// process the server, not the socket file.
    _lock: File,
}

/// One connection, from either end.
pub struct Stream {
    stream: UnixStream,
}

impl Listener {
    pub(super) fn bind(endpoint: &Endpoint) -> Result<Listener, BindError> {
        let dir = private_dir()?;
        let (socket, lock_path) = paths(&dir, endpoint);
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .mode(0o600)
            .open(&lock_path)?;
        match lock.try_lock() {
            Ok(()) => {}
            Err(fs::TryLockError::WouldBlock) => return Err(BindError::InUse),
            Err(fs::TryLockError::Error(e)) => return Err(BindError::Io(e)),
        }
        // Holding the lock, any socket file here belongs to a Fenix that
        // is gone.
        match fs::remove_file(&socket) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(BindError::Io(e)),
        }
        let listener = UnixListener::bind(&socket)?;
        Ok(Listener {
            listener,
            socket,
            _lock: lock,
        })
    }

    /// Waits for the next client.
    pub fn accept(&mut self) -> io::Result<Stream> {
        let (stream, _) = self.listener.accept()?;
        Ok(Stream { stream })
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.socket);
    }
}

impl Stream {
    pub(super) fn connect(endpoint: &Endpoint) -> io::Result<Option<Stream>> {
        let dir = private_dir()?;
        let (socket, _) = paths(&dir, endpoint);
        match UnixStream::connect(&socket) {
            Ok(stream) => Ok(Some(Stream { stream })),
            // No socket, or one nobody is listening on (left by a crash).
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
                ) =>
            {
                Ok(None)
            }
            Err(e) => Err(e),
        }
    }

    /// A second handle to the same connection.
    pub fn try_clone(&self) -> io::Result<Stream> {
        Ok(Stream {
            stream: self.stream.try_clone()?,
        })
    }
}

impl Read for Stream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.stream.read(buf)
    }
}

impl Write for Stream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.stream.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.stream.flush()
    }
}

pub(super) fn address(endpoint: &Endpoint) -> io::Result<String> {
    let (socket, _) = paths(&private_dir()?, endpoint);
    Ok(socket.to_string_lossy().into_owned())
}

/// `control[-<instance>].sock` and `control[-<instance>].lock` in `dir`.
fn paths(dir: &Path, endpoint: &Endpoint) -> (PathBuf, PathBuf) {
    let suffix = endpoint.suffix();
    (
        dir.join(format!("control{suffix}.sock")),
        dir.join(format!("control{suffix}.lock")),
    )
}

fn uid() -> u32 {
    // SAFETY: getuid has no preconditions and cannot fail.
    unsafe { libc::getuid() }
}

/// The private directory for this environment, created and checked.
fn private_dir() -> io::Result<PathBuf> {
    let dir = dir_for(std::env::var_os("XDG_RUNTIME_DIR").as_deref(), uid());
    ensure_private(&dir, uid())?;
    Ok(dir)
}

/// `$XDG_RUNTIME_DIR/fenix`, or `/tmp/fenix-$UID` when there's no
/// runtime directory.
fn dir_for(runtime_dir: Option<&OsStr>, uid: u32) -> PathBuf {
    match runtime_dir.filter(|d| !d.is_empty()) {
        Some(runtime) => PathBuf::from(runtime).join("fenix"),
        None => PathBuf::from("/tmp").join(format!("fenix-{uid}")),
    }
}

/// Creates `dir` `0700` if it isn't there, then checks it is a real
/// directory (not a symlink), owned by `uid` and closed to everyone
/// else, before anything is put in it. `/tmp` is shared, so someone else
/// could have made the directory first, to read our socket's traffic or
/// stand in for Fenix; that is refused rather than used.
fn ensure_private(dir: &Path, uid: u32) -> io::Result<()> {
    match DirBuilder::new().mode(0o700).create(dir) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e),
    }
    let meta = fs::symlink_metadata(dir)?;
    let problem = if !meta.file_type().is_dir() {
        Some("isn't a directory")
    } else if meta.uid() != uid {
        Some("belongs to another user")
    } else if meta.permissions().mode() & 0o077 != 0 {
        Some("is open to other users")
    } else {
        None
    };
    match problem {
        Some(problem) => Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("{} {problem}; Fenix won't use it", dir.display()),
        )),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_socket_lives_in_the_runtime_dir_or_a_tmp_dir_named_for_the_user() {
        assert_eq!(
            dir_for(Some(OsStr::new("/run/user/1000")), 1000),
            PathBuf::from("/run/user/1000/fenix")
        );
        assert_eq!(
            dir_for(Some(OsStr::new("")), 1000),
            PathBuf::from("/tmp/fenix-1000")
        );
        assert_eq!(dir_for(None, 1000), PathBuf::from("/tmp/fenix-1000"));
        let dir = Path::new("/run/user/1000/fenix");
        assert_eq!(
            paths(dir, &Endpoint::for_home(None)),
            (dir.join("control.sock"), dir.join("control.lock"))
        );
        assert_eq!(
            paths(dir, &Endpoint::for_home(Some(OsStr::new("foobar")))),
            (
                dir.join("control-85944171f73967e8.sock"),
                dir.join("control-85944171f73967e8.lock")
            )
        );
    }

    #[test]
    fn the_directory_is_created_private() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("fenix");
        ensure_private(&dir, uid()).unwrap();
        assert_eq!(
            fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
        // And the real one, where the tests' endpoints live.
        let real = private_dir().unwrap();
        assert_eq!(
            fs::metadata(&real).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }

    #[test]
    fn a_directory_open_to_others_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("fenix");
        fs::create_dir(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            ensure_private(&dir, uid()).unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
    }

    #[test]
    fn a_directory_owned_by_someone_else_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("fenix");
        ensure_private(&dir, uid()).unwrap();
        assert_eq!(
            ensure_private(&dir, uid().wrapping_add(1))
                .unwrap_err()
                .kind(),
            io::ErrorKind::PermissionDenied
        );
    }

    #[test]
    fn a_symlink_in_place_of_the_directory_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("elsewhere");
        ensure_private(&target, uid()).unwrap();
        let link = tmp.path().join("fenix");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert_eq!(
            ensure_private(&link, uid()).unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
    }

    #[test]
    fn a_socket_left_by_a_crash_is_replaced() {
        let endpoint = Endpoint::instance(&format!("stale-{}", std::process::id()));
        let (socket, _) = paths(&private_dir().unwrap(), &endpoint);
        // A bound-then-dropped std listener leaves its file behind, as a
        // crashed Fenix would.
        drop(UnixListener::bind(&socket).unwrap());
        assert!(socket.exists());
        assert!(endpoint.connect().unwrap().is_none());
        let _listener = endpoint.bind().unwrap();
        assert!(endpoint.connect().unwrap().is_some());
    }

    #[test]
    fn the_lock_not_the_socket_decides_who_serves() {
        let endpoint = Endpoint::instance(&format!("locked-{}", std::process::id()));
        let _server = endpoint.bind().unwrap();
        let (socket, _) = paths(&private_dir().unwrap(), &endpoint);
        // Even with its socket file gone, a live server keeps the name.
        fs::remove_file(&socket).unwrap();
        assert!(matches!(endpoint.bind(), Err(BindError::InUse)));
    }
}
