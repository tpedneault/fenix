//! The file manager's work on a remote machine's files.
//!
//! A remote path (`fenix_vfs::remote`) reaches each of this crate's entry
//! points like any other. What can be done there is done through
//! `fenix-vfs`; what is this machine's own business (the Recycle Bin, the
//! shell, archives) says so instead of failing with "not found".

use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::listing::{Attributes, DirEntryInfo, EntryKind};
use crate::recycle::Outcome;
use crate::transfer::OnConflict;

/// Fails, saying why, when `path` is on a remote machine and `what` is
/// something only this machine does.
pub(crate) fn local_only(path: &Path, what: &str) -> io::Result<()> {
    if fenix_vfs::is_remote(path) {
        return Err(io::Error::new(io::ErrorKind::Unsupported, format!("{what} isn't available for a file on a remote machine yet")));
    }
    Ok(())
}

pub(crate) fn any_remote<'a>(paths: impl IntoIterator<Item = &'a PathBuf>) -> bool {
    paths.into_iter().any(|path| fenix_vfs::is_remote(path))
}

/// A remote folder's entries, as `list_dir` gives a local one's.
pub(crate) fn list_dir(path: &Path) -> io::Result<Vec<DirEntryInfo>> {
    let entries = fenix_vfs::list(path)?;
    Ok(entries
        .into_iter()
        .map(|meta| {
            let kind = match (meta.symlink, meta.is_dir()) {
                (true, to_dir) => EntryKind::Link { to_dir },
                (false, true) => EntryKind::Dir,
                (false, false) => EntryKind::File,
            };
            DirEntryInfo {
                path: fenix_vfs::child(path, &meta.name),
                kind,
                size: if meta.is_dir() { 0 } else { meta.len },
                modified: Some(meta.modified()).filter(|time| *time != SystemTime::UNIX_EPOCH),
                attributes: Attributes {
                    // The convention there, wherever Fenix runs.
                    hidden: meta.name.starts_with('.'),
                    // Nobody may write it. Whether *you* may is the
                    // machine's to say when you try.
                    readonly: meta.mode != 0 && meta.mode & 0o222 == 0,
                    system: false,
                },
                readable: true,
                name: meta.name,
            }
        })
        .collect())
}

/// A remote machine has no bin Fenix can put things in: each path is
/// refused, with the way that does work.
pub(crate) fn no_recycle_bin(path: &Path) -> Outcome {
    Outcome { path: path.to_path_buf(), error: Some("a remote machine has no Recycle Bin: delete it for good instead".to_string()) }
}

pub(crate) fn permanently(path: &Path) -> Outcome {
    Outcome { path: path.to_path_buf(), error: fenix_vfs::remove(path, true).err().map(|error| error.to_string()) }
}

/// Copies or moves `sources` into `dest` when either is on a remote
/// machine. A file at a time, checked for `cancel` between them; what's
/// done when it stops stays done.
pub(crate) fn transfer(sources: &[PathBuf], dest: &Path, on_conflict: OnConflict, cancel: &dyn Fn() -> bool, moving: bool) -> Vec<Outcome> {
    sources
        .iter()
        .map(|source| {
            let error = if cancel() { Some("cancelled".to_string()) } else { transfer_one(source, dest, on_conflict, cancel, moving).err().map(|error| error.to_string()) };
            Outcome { path: source.clone(), error }
        })
        .collect()
}

fn transfer_one(source: &Path, dest: &Path, on_conflict: OnConflict, cancel: &dyn Fn() -> bool, moving: bool) -> io::Result<()> {
    let name = source.file_name().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no filename to copy under"))?.to_string_lossy().into_owned();
    // The name as its own machine has it, not as this one spells it.
    let name = fenix_vfs::split(source).map_or(name, |(_, there)| there.rsplit('/').next().unwrap_or_default().to_string());
    let mut target = fenix_vfs::child(dest, &name);
    if target == source {
        match on_conflict {
            OnConflict::KeepBoth => target = free_name(dest, &name),
            _ => return Err(io::Error::new(io::ErrorKind::InvalidInput, "source and destination are the same")),
        }
    } else if fenix_vfs::exists(&target) {
        match on_conflict {
            OnConflict::Skip => return Err(io::Error::new(io::ErrorKind::AlreadyExists, "skipped -- already exists")),
            OnConflict::KeepBoth => target = free_name(dest, &name),
            // What's there goes first. (A local copy merges a folder
            // into a folder; between machines the folder is replaced.)
            OnConflict::Overwrite => fenix_vfs::remove(&target, true)?,
        }
    }
    if moving && fenix_vfs::rename(source, &target, false).is_ok() {
        return Ok(());
    }
    fenix_vfs::copy(source, &target, &|| !cancel())?;
    if moving {
        fenix_vfs::remove(source, true)?;
    }
    Ok(())
}

/// `notes.txt` -> `notes (2).txt`, as a local copy names them.
fn free_name(dest: &Path, name: &str) -> PathBuf {
    let (stem, ext) = match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => (stem, Some(ext)),
        _ => (name, None),
    };
    (2..10_000)
        .map(|n| match ext {
            Some(ext) => format!("{stem} ({n}).{ext}"),
            None => format!("{stem} ({n})"),
        })
        .map(|candidate| fenix_vfs::child(dest, &candidate))
        .find(|candidate| !fenix_vfs::exists(candidate))
        .unwrap_or_else(|| fenix_vfs::child(dest, name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{copy_into, list_dir, move_into, permanently, rename_all, to_recycle_bin};

    fn machine(host: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        fenix_vfs::testing::Folder::mount(host, dir.path());
        dir
    }

    #[test]
    fn a_remote_folder_lists_like_a_local_one() {
        let dir = machine("fs-list");
        std::fs::create_dir(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join(".hidden"), b"12345").unwrap();
        let root = fenix_vfs::remote("fs-list", "/");
        let mut entries = list_dir(&root).unwrap();
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        let seen: Vec<_> = entries.iter().map(|e| (e.name.as_str(), e.kind, e.size, e.attributes.hidden, e.path.clone())).collect();
        assert_eq!(seen, [(".hidden", EntryKind::File, 5, true, fenix_vfs::child(&root, ".hidden")), ("src", EntryKind::Dir, 0, false, fenix_vfs::child(&root, "src"))]);
        assert!(entries.iter().all(|e| e.readable && e.modified.is_some()));
        assert!(list_dir(&fenix_vfs::remote("fs-list", "/nope")).is_err());
    }

    #[test]
    fn deleting_there_is_for_good_and_says_so() {
        let dir = machine("fs-delete");
        std::fs::create_dir_all(dir.path().join("old/deep")).unwrap();
        std::fs::write(dir.path().join("old/deep/f"), b"x").unwrap();
        let old = fenix_vfs::remote("fs-delete", "/old");
        let binned = to_recycle_bin(std::slice::from_ref(&old));
        assert!(binned[0].error.as_deref().unwrap().contains("no Recycle Bin"));
        assert!(dir.path().join("old").exists(), "refused, not deleted");
        assert_eq!(permanently(std::slice::from_ref(&old))[0].error, None);
        assert!(!dir.path().join("old").exists());
    }

    #[test]
    fn copies_and_moves_go_to_and_from_a_machine() {
        let dir = machine("fs-move");
        let here = tempfile::tempdir().unwrap();
        std::fs::write(here.path().join("a.txt"), b"a").unwrap();
        let root = fenix_vfs::remote("fs-move", "/");

        assert_eq!(copy_into(&[here.path().join("a.txt")], &root, OnConflict::Skip)[0].error, None);
        assert_eq!(std::fs::read(dir.path().join("a.txt")).unwrap(), b"a");
        // Again: skipped, then kept beside it.
        assert!(copy_into(&[here.path().join("a.txt")], &root, OnConflict::Skip)[0].error.is_some());
        assert_eq!(copy_into(&[here.path().join("a.txt")], &root, OnConflict::KeepBoth)[0].error, None);
        assert!(dir.path().join("a (2).txt").exists());
        // Within the machine, a move is a rename there.
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        assert_eq!(move_into(&[fenix_vfs::child(&root, "a (2).txt")], &fenix_vfs::child(&root, "sub"), OnConflict::Skip)[0].error, None);
        assert!(dir.path().join("sub/a (2).txt").exists() && !dir.path().join("a (2).txt").exists());
        // Back to this machine: the remote one no longer has it.
        assert_eq!(move_into(&[fenix_vfs::child(&root, "sub")], here.path(), OnConflict::Skip)[0].error, None);
        assert!(here.path().join("sub/a (2).txt").exists() && !dir.path().join("sub").exists());

        let renamed = rename_all(&[(fenix_vfs::child(&root, "a.txt"), fenix_vfs::child(&root, "b.txt"))], false);
        assert_eq!(renamed[0].error, None);
        assert!(dir.path().join("b.txt").exists());
    }

    #[test]
    fn what_only_this_machine_does_says_so() {
        let file = fenix_vfs::remote("fs-none", "/a.txt");
        for refused in [crate::properties(&file).err(), crate::open_with_default(&file).err(), crate::reveal(&file).err(), crate::set_readonly(&file, true).err()] {
            let refused = refused.expect("refused");
            assert_eq!(refused.kind(), io::ErrorKind::Unsupported, "{refused}");
        }
    }
}
