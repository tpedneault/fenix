//! Deleting things in a way you can take back.
//!
//! The explorer this replaces called `fs::remove_dir_all` on whatever
//! was marked, immediately and without asking. That is a file manager
//! whose most destructive action is also one of its easiest keystrokes,
//! with no undo anywhere behind it.
//!
//! So deleting means the Recycle Bin, which is what the rest of the
//! system means by it: the files stay recoverable through Windows'
//! own restore, from a place the user already knows to look. Permanent
//! deletion still exists, because sometimes it is genuinely what you
//! want (a folder too large for the bin, a share that has none), but it
//! is a separate, explicit request rather than the default.

use std::io;
use std::path::{Path, PathBuf};

/// What happened to one path in a delete request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub path: PathBuf,
    /// `None` on success; the reason it did not happen otherwise.
    pub error: Option<String>,
}

impl Outcome {
    fn ok(path: PathBuf) -> Self {
        Self { path, error: None }
    }

    fn failed(path: PathBuf, error: impl std::fmt::Display) -> Self {
        Self { path, error: Some(error.to_string()) }
    }

    pub fn succeeded(&self) -> bool {
        self.error.is_none()
    }
}

/// Moves every path to the Recycle Bin, reporting each one's fate.
///
/// Best-effort across the whole set: one path failing does not stop the
/// rest, because a batch that stops halfway leaves the user with no
/// idea what happened and a listing that matches neither the before nor
/// the after. The caller decides what to say about the failures.
///
/// Deliberately does **not** fall back to permanent deletion when the
/// bin refuses. A path the Recycle Bin will not take is usually one it
/// *cannot* take -- a network share, a removable volume with no bin, an
/// item larger than the bin's quota -- and silently destroying it
/// instead would turn "I asked for the safe delete" into the unsafe one
/// at exactly the moment the user was relying on the promise. The
/// failure is reported so the caller can offer the permanent delete as
/// its own decision.
pub fn to_recycle_bin(paths: &[PathBuf]) -> Vec<Outcome> {
    paths
        .iter()
        .map(|path| match trash::delete(path) {
            Ok(()) => Outcome::ok(path.clone()),
            Err(err) => Outcome::failed(path.clone(), describe(&err)),
        })
        .collect()
}

/// Deletes every path outright, with no way back. Only ever reached
/// through an explicit request (`D!`) or a deliberate answer to a
/// failed `to_recycle_bin` -- never as a silent fallback.
pub fn permanently(paths: &[PathBuf]) -> Vec<Outcome> {
    paths
        .iter()
        .map(|path| {
            // `symlink_metadata`, not `metadata`: deleting a link to a
            // directory must remove the link, never walk through it and
            // empty the directory it points at.
            let is_dir = std::fs::symlink_metadata(path).map(|m| m.is_dir()).unwrap_or(false);
            let result = if is_dir { remove_dir_all(path) } else { std::fs::remove_file(path) };
            match result {
                Ok(()) => Outcome::ok(path.clone()),
                Err(err) => Outcome::failed(path.clone(), err),
            }
        })
        .collect()
}

/// `fs::remove_dir_all` with one addition: a read-only file inside the
/// tree is cleared and retried rather than stopping the whole delete.
///
/// Windows refuses to unlink a read-only file, and version-control
/// caches, extracted archives and vendored dependencies are full of
/// them -- so the plain call fails part-way through and leaves a
/// half-deleted tree, which is worse than either outcome. The user
/// asked for the directory to go.
fn remove_dir_all(path: &Path) -> io::Result<()> {
    match std::fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::PermissionDenied => {
            clear_readonly_recursively(path);
            std::fs::remove_dir_all(path)
        }
        Err(err) => Err(err),
    }
}

fn clear_readonly_recursively(path: &Path) {
    let Ok(metadata) = std::fs::symlink_metadata(path) else { return };
    let mut perms = metadata.permissions();
    if perms.readonly() {
        #[allow(clippy::permissions_set_readonly_false)]
        perms.set_readonly(false);
        let _ = std::fs::set_permissions(path, perms);
    }
    // Never through a link: clearing flags on the far side would be
    // touching files outside the tree the user asked to remove.
    if metadata.is_dir() && !metadata.file_type().is_symlink() {
        let Ok(entries) = std::fs::read_dir(path) else { return };
        for entry in entries.flatten() {
            clear_readonly_recursively(&entry.path());
        }
    }
}

/// The `trash` crate's errors carry the useful detail in their `Debug`
/// form; `Display` for the common Windows case is a bare "platform
/// specific error". Prefer whatever is more informative.
fn describe(err: &trash::Error) -> String {
    let display = err.to_string();
    if display.len() < 30 {
        format!("{display} ({err:?})")
    } else {
        display
    }
}

/// These test against the real Recycle Bin, because a mocked one would
/// prove nothing about the thing that actually has to work. A full run
/// therefore leaves a handful of tiny temp files (`gone.txt`, `real.txt`
/// and friends) in the bin -- harmless, and cleared by emptying it, but
/// worth knowing about before you wonder where they came from.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::TempDir;

    #[test]
    fn a_file_goes_to_the_recycle_bin_and_leaves_the_directory() {
        let dir = TempDir::new("recycle_file");
        let path = dir.write("gone.txt", "bye");

        let outcomes = to_recycle_bin(&[path.clone()]);

        assert_eq!(outcomes.len(), 1);
        assert!(outcomes[0].succeeded(), "expected the bin to accept it: {:?}", outcomes[0].error);
        assert!(!path.exists(), "and the file is no longer where it was");
    }

    #[test]
    fn a_directory_goes_to_the_recycle_bin_whole() {
        let dir = TempDir::new("recycle_dir");
        let sub = dir.mkdir("tree");
        std::fs::write(sub.join("inner.txt"), "x").unwrap();

        let outcomes = to_recycle_bin(&[sub.clone()]);

        assert!(outcomes[0].succeeded(), "{:?}", outcomes[0].error);
        assert!(!sub.exists());
    }

    #[test]
    fn one_failure_does_not_stop_the_rest_of_the_batch() {
        // A listing can go stale between reading it and acting on it,
        // and a batch that gives up half way leaves the user unable to
        // tell what happened.
        let dir = TempDir::new("recycle_partial");
        let good = dir.write("real.txt", "x");
        let missing = dir.path().join("never-existed.txt");
        let also_good = dir.write("real2.txt", "x");

        let outcomes = to_recycle_bin(&[good.clone(), missing.clone(), also_good.clone()]);

        assert_eq!(outcomes.len(), 3, "every path is accounted for, not just the ones that worked");
        assert!(outcomes[0].succeeded());
        assert!(!outcomes[1].succeeded(), "a path that isn't there can't be binned");
        assert!(outcomes[2].succeeded(), "and the one after the failure still happened");
        assert!(!good.exists() && !also_good.exists());
    }

    #[test]
    fn a_failure_says_which_path_it_was_about() {
        let dir = TempDir::new("recycle_names_path");
        let missing = dir.path().join("never-existed.txt");
        let outcomes = to_recycle_bin(&[missing.clone()]);
        assert_eq!(outcomes[0].path, missing);
        assert!(outcomes[0].error.as_ref().is_some_and(|e| !e.is_empty()), "an empty reason is no reason");
    }

    #[test]
    fn permanent_deletion_removes_files_and_trees() {
        let dir = TempDir::new("permanent");
        let file = dir.write("a.txt", "x");
        let tree = dir.mkdir("tree");
        std::fs::write(tree.join("inner.txt"), "x").unwrap();

        let outcomes = permanently(&[file.clone(), tree.clone()]);

        assert!(outcomes.iter().all(|o| o.succeeded()), "{outcomes:?}");
        assert!(!file.exists() && !tree.exists());
    }

    #[test]
    fn permanent_deletion_gets_through_a_read_only_file() {
        // Windows will not unlink a read-only file, and the plain
        // `remove_dir_all` gives up part way through, leaving a
        // half-deleted tree.
        let dir = TempDir::new("permanent_readonly");
        let tree = dir.mkdir("tree");
        let locked = tree.join("locked.txt");
        std::fs::write(&locked, "x").unwrap();
        let mut perms = std::fs::metadata(&locked).unwrap().permissions();
        perms.set_readonly(true);
        std::fs::set_permissions(&locked, perms).unwrap();

        let outcomes = permanently(&[tree.clone()]);

        assert!(outcomes[0].succeeded(), "{:?}", outcomes[0].error);
        assert!(!tree.exists());
    }

    #[test]
    fn deleting_a_link_to_a_directory_does_not_empty_what_it_points_at() {
        // The difference between removing a shortcut and removing
        // someone's work.
        let dir = TempDir::new("permanent_link");
        let real = dir.mkdir("real");
        std::fs::write(real.join("precious.txt"), "keep me").unwrap();
        let link = dir.path().join("link");
        if !make_dir_link(&real, &link) {
            return; // no permission to create links here; nothing to check
        }

        let outcomes = permanently(&[link.clone()]);

        assert!(outcomes[0].succeeded(), "{:?}", outcomes[0].error);
        assert!(!link.exists());
        assert!(real.join("precious.txt").exists(), "the target survived");
    }

    /// Creating a directory link needs Developer Mode or elevation on
    /// Windows; `false` means this machine would not allow it, and the
    /// caller should skip rather than fail.
    fn make_dir_link(target: &std::path::Path, link: &std::path::Path) -> bool {
        #[cfg(windows)]
        {
            std::os::windows::fs::symlink_dir(target, link).is_ok()
        }
        #[cfg(not(windows))]
        {
            std::os::unix::fs::symlink(target, link).is_ok()
        }
    }
}
