//! The few files this crate reads and writes itself (`.git`'s state, the
//! operation log, a rebase's plan), wherever the repository is: on this
//! machine through `std::fs`, as always, and on a remote project's
//! machine through `fenix-vfs`.

use std::io;
use std::path::{Path, PathBuf};

pub(crate) fn is_dir(path: &Path) -> bool {
    if fenix_vfs::is_remote(path) {
        return fenix_vfs::metadata(path).is_ok_and(|meta| meta.is_dir());
    }
    path.is_dir()
}

pub(crate) fn is_file(path: &Path) -> bool {
    if fenix_vfs::is_remote(path) {
        return fenix_vfs::metadata(path).is_ok_and(|meta| meta.is_file());
    }
    path.is_file()
}

pub(crate) fn read(path: &Path) -> io::Result<Vec<u8>> {
    if fenix_vfs::is_remote(path) {
        return fenix_vfs::read(path);
    }
    std::fs::read(path)
}

pub(crate) fn read_to_string(path: &Path) -> io::Result<String> {
    if fenix_vfs::is_remote(path) {
        return String::from_utf8(fenix_vfs::read(path)?).map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "not UTF-8"));
    }
    std::fs::read_to_string(path)
}

pub(crate) fn write(path: &Path, contents: &[u8]) -> io::Result<()> {
    if fenix_vfs::is_remote(path) {
        return fenix_vfs::write(path, contents, fenix_vfs::Over::Anything).map(drop);
    }
    std::fs::write(path, contents)
}

pub(crate) fn create_dir_all(path: &Path) -> io::Result<()> {
    if fenix_vfs::is_remote(path) {
        return fenix_vfs::create_dir_all(path);
    }
    std::fs::create_dir_all(path)
}

/// Adds `line` and a line break at the end of a file, making it if
/// there's none. On a remote machine the file is read and written whole.
pub(crate) fn append_line(path: &Path, line: &str) -> io::Result<()> {
    if fenix_vfs::is_remote(path) {
        let mut text = match fenix_vfs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => Vec::new(),
            Err(error) => return Err(error),
        };
        text.extend_from_slice(line.as_bytes());
        text.push(b'\n');
        return fenix_vfs::write(path, &text, fenix_vfs::Over::Anything).map(drop);
    }
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(file, "{line}")
}

/// When a file last changed, and how big it is.
pub(crate) fn modified_and_len(path: &Path) -> io::Result<(std::time::SystemTime, u64)> {
    if fenix_vfs::is_remote(path) {
        let meta = fenix_vfs::metadata(path)?;
        return Ok((meta.modified(), meta.len));
    }
    let meta = std::fs::metadata(path)?;
    Ok((meta.modified()?, meta.len()))
}

/// A path git printed (absolute, as `repo`'s machine names it), as a
/// path here.
pub(crate) fn from_git(repo: &Path, printed: &str) -> PathBuf {
    match fenix_vfs::host_of(repo) {
        Some(host) => fenix_vfs::remote(&host, printed),
        None => PathBuf::from(printed),
    }
}

/// A path here, as git and its shell take it on the path's own machine:
/// with `/` on every platform.
pub(crate) fn for_git(path: &Path) -> String {
    match fenix_vfs::split(path) {
        Some((_, there)) => there,
        None => path.to_string_lossy().replace('\\', "/"),
    }
}

/// A path here as an argument to git: as it is for this machine, as
/// its own machine names it for a remote one.
pub(crate) fn arg(path: &Path) -> String {
    match fenix_vfs::split(path) {
        Some((_, there)) => there,
        None => path.to_string_lossy().to_string(),
    }
}

/// `relative` (as git prints one, with `/`) under `repo`.
pub(crate) fn under(repo: &Path, relative: &str) -> PathBuf {
    fenix_vfs::join(repo, relative)
}

#[cfg(test)]
mod tests {
    use crate::test_util::{git, init_repo, TempDir};

    /// A repository on a "remote machine" that is a folder here: what
    /// `git` is asked runs there, and what's read of `.git` is read there.
    #[test]
    fn a_remote_repository_is_read_and_changed_on_its_machine() {
        let dir = TempDir::new("remote_repo");
        let machine = fenix_vfs::testing::Folder::mount("git-box", dir.path());
        std::fs::create_dir_all(dir.path().join("proj")).unwrap();
        let local = dir.path().join("proj");
        init_repo(&local);
        std::fs::write(local.join("a.txt"), "one\n").unwrap();
        git(&local, &["add", "-A"]);
        git(&local, &["commit", "-q", "-m", "First"]);
        let repo = fenix_vfs::remote("git-box", "/proj");

        assert_eq!(crate::status(&repo).map(|status| status.branch).as_deref(), Some("main"));
        std::fs::write(local.join("a.txt"), "one\ntwo\n").unwrap();
        let files = crate::list_files(&repo);
        assert_eq!(files.len(), 1, "{files:?}");
        crate::stage_file(&repo, "a.txt").unwrap();
        crate::commit(&repo, "Second").unwrap();
        assert_eq!(crate::list_commits(&repo, 10).len(), 2);
        assert!(machine.calls.lock().unwrap().iter().all(|path| path.starts_with("/proj")), "only the machine was asked");

        // `.git`'s own state, read on the machine.
        assert_eq!(crate::in_progress(&repo), None);
        std::fs::write(local.join(".git/MERGE_HEAD"), "0123\n").unwrap();
        std::fs::write(local.join(".git/MERGE_MSG"), "Merge branch 'x'\n# a comment\n").unwrap();
        assert_eq!(crate::in_progress(&repo), Some(crate::InProgress::Merge));
        assert_eq!(crate::merge_message(&repo).as_deref(), Some("Merge branch 'x'"));

        // A machine that's away answers nothing, and fails nothing.
        machine.set_away(true);
        assert!(crate::status(&repo).is_none());
        assert!(crate::run_args(&repo, &["status"]).unwrap_err().contains("couldn't run git"));
        machine.set_away(false);
        fenix_vfs::unmount("git-box");
    }

    /// Through the real helper (in this process): what git prints as a
    /// path there comes back as a path on that machine.
    #[cfg(unix)]
    #[test]
    fn the_operation_log_of_a_remote_repository_is_kept_there() {
        let _machine = fenix_ssh::testing::Loopback::mount("git-loop").unwrap();
        let dir = TempDir::new("remote_oplog");
        init_repo(dir.path());
        std::fs::write(dir.path().join("a.txt"), "one\n").unwrap();
        git(dir.path(), &["add", "-A"]);
        git(dir.path(), &["commit", "-q", "-m", "First"]);
        let repo = fenix_vfs::remote("git-loop", &dir.path().to_string_lossy());

        std::fs::write(dir.path().join("b.txt"), "two\n").unwrap();
        crate::oplog::logged(&repo, "stage b", || crate::stage_file(&repo, "b.txt"), |_| crate::oplog::Undo::Not("test".into())).unwrap();
        let entries = crate::oplog::entries(&repo, 10);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].label, "stage b");
        assert!(dir.path().join(".git/fenix/oplog").is_file(), "kept in the repository's own git dir");
        assert_eq!(super::from_git(&repo, "/x/y"), fenix_vfs::remote("git-loop", "/x/y"));
        assert_eq!(super::arg(&fenix_vfs::remote("git-loop", "/x/y")), "/x/y");
    }
}
