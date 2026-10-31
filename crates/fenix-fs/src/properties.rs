//! What a file actually is, beyond its name.
//!
//! The listing shows a size and an age because those fit in a column.
//! This is the rest of it -- every timestamp, the attributes, where a
//! link points -- plus the one number a listing structurally cannot
//! give you: how much a *directory* contains, which nothing knows
//! without walking it.

use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::listing::{Attributes, EntryKind};

/// Everything worth knowing about one path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Properties {
    pub path: PathBuf,
    pub kind: EntryKind,
    /// Bytes. For a directory this is whatever the filesystem says about
    /// the directory entry itself, which is not the size of its
    /// contents -- see `measure_tree` for that.
    pub size: u64,
    pub created: Option<SystemTime>,
    pub modified: Option<SystemTime>,
    pub accessed: Option<SystemTime>,
    pub attributes: Attributes,
    /// Where a link points, unresolved -- the text of the link, which
    /// is what you want to see, rather than where it happens to land
    /// today.
    pub link_target: Option<PathBuf>,
}

/// Reads everything about `path` without following it: a link reports
/// on itself, not on whatever it points at.
///
/// Deliberately has no "size on disk". Getting the truthful number
/// means asking Windows about allocated ranges through an API this
/// workspace has no binding for, and the plausible approximations
/// (rounding to a cluster size that is itself guessed) would be a
/// confident wrong answer where none at all is honest.
pub fn properties(path: &Path) -> io::Result<Properties> {
    let metadata = std::fs::symlink_metadata(path)?;
    let file_type = metadata.file_type();
    let kind = if file_type.is_symlink() {
        EntryKind::Link { to_dir: std::fs::metadata(path).map(|m| m.is_dir()).unwrap_or(false) }
    } else if metadata.is_dir() {
        EntryKind::Dir
    } else {
        EntryKind::File
    };
    Ok(Properties {
        path: path.to_path_buf(),
        kind,
        size: if metadata.is_dir() { 0 } else { metadata.len() },
        created: metadata.created().ok(),
        modified: metadata.modified().ok(),
        accessed: metadata.accessed().ok(),
        attributes: crate::listing::attributes_for(
            &metadata,
            &path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        ),
        link_target: file_type.is_symlink().then(|| std::fs::read_link(path).ok()).flatten(),
    })
}

/// Turns the read-only flag on or off.
///
/// The one attribute worth being able to change from here: it is the
/// one that stops an ordinary save, and hunting for the Windows
/// properties dialog to clear it is exactly the trip out of the editor
/// this explorer exists to remove.
pub fn set_readonly(path: &Path, readonly: bool) -> io::Result<()> {
    let mut permissions = std::fs::symlink_metadata(path)?.permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    permissions.set_readonly(readonly);
    std::fs::set_permissions(path, permissions)
}

/// What a directory contains, counted.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Total {
    pub files: usize,
    pub directories: usize,
    pub bytes: u64,
    /// `true` when the walk stopped early because it was cancelled, so
    /// a caller can say "at least this much" instead of presenting a
    /// partial count as final.
    pub partial: bool,
}

/// Walks `root` and adds up what is under it.
///
/// The one number a listing cannot show without doing this work, and
/// the reason it is a separate, asked-for action rather than another
/// column: on a large tree it is thousands of metadata calls, and on a
/// share it is thousands of round trips.
///
/// Never follows links -- a junction into a larger tree would otherwise
/// count that tree, or loop forever if it points at an ancestor.
pub fn measure_tree(root: &Path, cancel: &impl Fn() -> bool) -> Total {
    let mut total = Total::default();
    walk(root, cancel, &mut total);
    total
}

fn walk(path: &Path, cancel: &impl Fn() -> bool, total: &mut Total) {
    if total.partial {
        return;
    }
    if cancel() {
        total.partial = true;
        return;
    }
    let Ok(metadata) = std::fs::symlink_metadata(path) else { return };
    if metadata.file_type().is_symlink() {
        // Counted as the one entry it is, not as what it points at.
        total.files += 1;
        return;
    }
    if metadata.is_dir() {
        total.directories += 1;
        let Ok(entries) = std::fs::read_dir(path) else { return };
        for entry in entries.flatten() {
            walk(&entry.path(), cancel, total);
        }
        return;
    }
    total.files += 1;
    total.bytes += metadata.len();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::TempDir;

    fn never() -> impl Fn() -> bool {
        || false
    }

    #[test]
    fn a_file_reports_its_size_times_and_attributes() {
        let dir = TempDir::new("props_file");
        let file = dir.write("a.txt", "hello");

        let props = properties(&file).unwrap();

        assert_eq!(props.kind, EntryKind::File);
        assert_eq!(props.size, 5);
        assert!(props.modified.is_some());
        assert!(!props.attributes.readonly);
        assert_eq!(props.link_target, None);
    }

    #[test]
    fn a_directorys_own_size_is_not_offered_as_its_contents() {
        // The number the filesystem reports for a directory entry says
        // nothing about what is in it; `measure_tree` is the question
        // that has a real answer.
        let dir = TempDir::new("props_dir");
        let sub = dir.mkdir("sub");
        std::fs::write(sub.join("big.txt"), vec![b'x'; 5000]).unwrap();

        let props = properties(&sub).unwrap();

        assert_eq!(props.kind, EntryKind::Dir);
        assert_eq!(props.size, 0);
    }

    #[test]
    fn read_only_can_be_turned_on_and_off() {
        // The attribute that stops an ordinary save, and the reason
        // people go looking for the Windows properties dialog.
        let dir = TempDir::new("props_readonly");
        let file = dir.write("a.txt", "hello");

        set_readonly(&file, true).unwrap();
        assert!(properties(&file).unwrap().attributes.readonly);

        set_readonly(&file, false).unwrap();
        assert!(!properties(&file).unwrap().attributes.readonly);
    }

    #[test]
    fn a_missing_path_is_an_error_rather_than_an_empty_answer() {
        let dir = TempDir::new("props_missing");
        assert!(properties(&dir.path().join("nope")).is_err());
    }

    #[test]
    fn measuring_a_tree_counts_files_directories_and_bytes() {
        let dir = TempDir::new("props_measure");
        std::fs::write(dir.path().join("a.txt"), "12345").unwrap();
        let sub = dir.mkdir("sub");
        std::fs::write(sub.join("b.txt"), "123").unwrap();

        let total = measure_tree(dir.path(), &never());

        assert_eq!(total.files, 2);
        assert_eq!(total.directories, 2, "the root counts as one");
        assert_eq!(total.bytes, 8);
        assert!(!total.partial);
    }

    #[test]
    fn measuring_a_single_file_is_just_that_file() {
        let dir = TempDir::new("props_measure_file");
        let file = dir.write("a.txt", "12345");
        let total = measure_tree(&file, &never());
        assert_eq!((total.files, total.directories, total.bytes), (1, 0, 5));
    }

    #[test]
    fn a_cancelled_walk_says_it_did_not_finish() {
        // So a caller can say "at least this much" rather than passing
        // off a partial count as the answer.
        let dir = TempDir::new("props_measure_cancel");
        std::fs::write(dir.path().join("a.txt"), "12345").unwrap();

        let total = measure_tree(dir.path(), &|| true);

        assert!(total.partial);
    }

    #[test]
    fn a_link_is_counted_as_itself_not_as_what_it_points_at() {
        // A junction into a larger tree would otherwise count that tree
        // -- or loop forever, if it points at an ancestor.
        let dir = TempDir::new("props_measure_link");
        let real = dir.mkdir("real");
        std::fs::write(real.join("big.txt"), vec![b'x'; 4096]).unwrap();
        let link = dir.path().join("link");
        #[cfg(windows)]
        let made = std::os::windows::fs::symlink_dir(&real, &link).is_ok();
        #[cfg(not(windows))]
        let made = std::os::unix::fs::symlink(&real, &link).is_ok();
        if !made {
            return; // no permission to create links here
        }

        let total = measure_tree(&link, &never());

        assert_eq!(total.files, 1);
        assert_eq!(total.bytes, 0, "the tree behind it is not counted twice");
    }
}
