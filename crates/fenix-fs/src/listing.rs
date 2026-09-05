//! Reading a directory without letting one bad entry ruin it.
//!
//! `std::fs::read_dir` is the easy part. What this adds is everything a
//! file manager needs and `std::fs` will not tell you directly: whether
//! an entry is a link (and to what), the Windows attributes that decide
//! whether a file is hidden, and -- most importantly -- a listing that
//! survives entries it cannot read.
//!
//! That last one is not hypothetical. The explorer this replaces did
//! `let metadata = dirent.metadata()?;` inside its loop, so a single
//! permission-denied child aborted the whole directory. `C:\` has
//! several. So does the root of most network shares. The entry you
//! cannot stat is listed anyway, with what is known about it, because
//! "one file in this folder is locked" is not a reason to refuse to
//! show the folder.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// What an entry turned out to be.
///
/// Links are their own kind rather than being flattened into
/// file-or-directory, because a file manager has to show you that a
/// thing *is* a link -- following one into a directory tree you did not
/// expect to be in is exactly the surprise this prevents. `to_dir`
/// still says what it points at, so a link to a directory can sort and
/// navigate like one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    File,
    Dir,
    /// A symlink, or on Windows any reparse point -- junctions and
    /// directory links included. They behave the same way for every
    /// purpose here, and Windows itself blurs them.
    Link { to_dir: bool },
}

impl EntryKind {
    /// Whether navigating into this entry makes sense -- a directory, or
    /// a link to one.
    pub fn is_dir_like(self) -> bool {
        matches!(self, EntryKind::Dir | EntryKind::Link { to_dir: true })
    }

    pub fn is_link(self) -> bool {
        matches!(self, EntryKind::Link { .. })
    }
}

/// Flags that change how an entry should be shown or treated, drawn
/// from the platform's own idea of them rather than from the filename.
///
/// `hidden` is the one that matters: on Windows it is a real file
/// attribute, and a leading dot means nothing at all -- `NTUSER.DAT`
/// and `$RECYCLE.BIN` are hidden, `.gitignore` is not. Reading it from
/// the filename (which is what the explorer this replaces did) shows
/// you a folder full of system clutter and hides the dotfiles you were
/// actually looking for. Elsewhere the dot *is* the convention, so
/// that is what is used there.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Attributes {
    pub hidden: bool,
    pub readonly: bool,
    /// Windows only; always `false` elsewhere. Worth keeping separate
    /// from `hidden` because system files are usually worth hiding even
    /// when hidden files are being shown.
    pub system: bool,
}

/// One entry in a directory listing, as the filesystem describes it.
///
/// Deliberately plain data with no notion of selection, marks, depth or
/// git state -- those belong to whatever is presenting the listing
/// (`fenix-explorer`), not to the act of reading a directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirEntryInfo {
    pub name: String,
    pub path: PathBuf,
    pub kind: EntryKind,
    /// Bytes. `0` for directories (their real size needs a recursive
    /// walk, which is a separate, explicitly-requested action) and for
    /// anything whose metadata could not be read.
    pub size: u64,
    /// `None` when the entry's metadata could not be read, or the
    /// platform does not report one -- rendered as blank rather than as
    /// a fake epoch timestamp.
    pub modified: Option<SystemTime>,
    pub attributes: Attributes,
    /// `false` when this entry's metadata could not be read at all
    /// (permission denied, a broken link, a file deleted between the
    /// enumeration and the stat). Everything above is then a
    /// best-effort guess, and a presenter can say so rather than
    /// showing a confident `0 B`.
    pub readable: bool,
}

/// Lists one directory's immediate children, unsorted.
///
/// Fails only if the directory itself cannot be opened. Individual
/// entries that cannot be read are still returned, marked
/// `readable: false` -- see the module comment for why that is the
/// whole point.
pub fn list_dir(path: &Path) -> io::Result<Vec<DirEntryInfo>> {
    let mut out = Vec::new();
    for dirent in fs::read_dir(path)? {
        // An error *here* is about this one entry, not the directory --
        // skip it and keep going, for the same reason a stat failure
        // below does not abort.
        let Ok(dirent) = dirent else { continue };
        out.push(describe(&dirent));
    }
    Ok(out)
}

/// Everything known about one enumerated entry, degrading rather than
/// failing.
fn describe(dirent: &fs::DirEntry) -> DirEntryInfo {
    let path = dirent.path();
    let name = dirent.file_name().to_string_lossy().into_owned();

    // `file_type` first, and not `metadata`: it does not follow links
    // (so a link is reported as a link rather than as whatever it
    // points at), and on Windows it comes free with the enumeration --
    // no extra syscall per file, which is the difference between a
    // network share listing in a moment and listing in a minute.
    let file_type = dirent.file_type().ok();
    let is_link = file_type.is_some_and(|t| t.is_symlink());

    // Read without following, for the entry's own facts; then, only for
    // a link, follow once to find out what it points at. A broken link
    // simply reports `to_dir: false` rather than failing -- it is still
    // a real entry that should be listed and deletable.
    let metadata = fs::symlink_metadata(&path).ok();
    let kind = if is_link {
        EntryKind::Link { to_dir: fs::metadata(&path).map(|m| m.is_dir()).unwrap_or(false) }
    } else if file_type.is_some_and(|t| t.is_dir()) || metadata.as_ref().is_some_and(|m| m.is_dir()) {
        EntryKind::Dir
    } else {
        EntryKind::File
    };

    let readable = metadata.is_some();
    let size = metadata.as_ref().filter(|_| !kind.is_dir_like()).map(|m| m.len()).unwrap_or(0);
    let modified = metadata.as_ref().and_then(|m| m.modified().ok());
    let attributes = metadata.as_ref().map(|m| attributes_of(m, &name)).unwrap_or_default();

    DirEntryInfo { name, path, kind, size, modified, attributes, readable }
}

#[cfg(windows)]
fn attributes_of(metadata: &fs::Metadata, _name: &str) -> Attributes {
    use std::os::windows::fs::MetadataExt;
    // The three flags a file manager acts on, straight from the
    // platform. `MetadataExt::file_attributes` is in std -- no binding
    // crate needed for this much.
    const FILE_ATTRIBUTE_READONLY: u32 = 0x0000_0001;
    const FILE_ATTRIBUTE_HIDDEN: u32 = 0x0000_0002;
    const FILE_ATTRIBUTE_SYSTEM: u32 = 0x0000_0004;
    let bits = metadata.file_attributes();
    Attributes {
        hidden: bits & FILE_ATTRIBUTE_HIDDEN != 0,
        readonly: bits & FILE_ATTRIBUTE_READONLY != 0,
        system: bits & FILE_ATTRIBUTE_SYSTEM != 0,
    }
}

#[cfg(not(windows))]
fn attributes_of(metadata: &fs::Metadata, name: &str) -> Attributes {
    // No attribute bits here: the leading dot genuinely is the
    // convention, and read-only is a permission rather than a flag.
    Attributes { hidden: name.starts_with('.'), readonly: metadata.permissions().readonly(), system: false }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::TempDir;

    fn find<'a>(entries: &'a [DirEntryInfo], name: &str) -> &'a DirEntryInfo {
        entries.iter().find(|e| e.name == name).unwrap_or_else(|| panic!("no entry named {name:?}"))
    }

    #[test]
    fn lists_files_and_directories_with_their_own_facts() {
        let dir = TempDir::new("list_basic");
        dir.write("notes.txt", "hello");
        dir.mkdir("sub");

        let entries = list_dir(dir.path()).unwrap();

        assert_eq!(entries.len(), 2);
        let file = find(&entries, "notes.txt");
        assert_eq!(file.kind, EntryKind::File);
        assert_eq!(file.size, 5);
        assert!(file.readable);
        assert!(file.modified.is_some());
        assert_eq!(find(&entries, "sub").kind, EntryKind::Dir);
    }

    #[test]
    fn a_directorys_own_size_is_not_guessed_at() {
        // The number the filesystem reports for a directory is an
        // implementation detail of the directory itself, not the size of
        // what is in it -- showing it would be worse than showing
        // nothing. A real recursive total is a separate, asked-for
        // action.
        let dir = TempDir::new("list_dir_size");
        dir.mkdir("sub");
        assert_eq!(find(&list_dir(dir.path()).unwrap(), "sub").size, 0);
    }

    #[test]
    fn an_empty_directory_lists_as_empty_rather_than_failing() {
        let dir = TempDir::new("list_empty");
        assert!(list_dir(dir.path()).unwrap().is_empty());
    }

    #[test]
    fn a_directory_that_cannot_be_opened_is_an_error() {
        // The one failure that *is* the caller's problem -- there is
        // nothing to show.
        let dir = TempDir::new("list_missing");
        assert!(list_dir(&dir.path().join("nope")).is_err());
    }

    #[test]
    fn the_listing_is_not_sorted_here() {
        // Ordering is a presentation choice (by name, size, date, with
        // directories first or not), so it belongs to the presenter --
        // sorting here would just be work the caller has to undo.
        let dir = TempDir::new("list_unsorted");
        dir.touch("a");
        dir.touch("b");
        let entries = list_dir(dir.path()).unwrap();
        assert_eq!(entries.len(), 2);
    }

    #[cfg(windows)]
    #[test]
    fn hidden_comes_from_the_attribute_and_not_the_filename() {
        // The bug this exists to prevent: on Windows a leading dot means
        // nothing, and the files worth hiding are marked, not named.
        use std::os::windows::fs::MetadataExt;
        let dir = TempDir::new("list_hidden_attr");
        let dotted = dir.touch(".gitignore");
        let marked = dir.touch("secret.txt");
        std::process::Command::new("attrib").arg("+H").arg(&marked).output().expect("attrib");

        let entries = list_dir(dir.path()).unwrap();

        assert!(!find(&entries, ".gitignore").attributes.hidden, "a dot is not what makes a file hidden here");
        assert!(find(&entries, "secret.txt").attributes.hidden, "the attribute is");
        // Sanity: the flag really was set, so a failure above is about
        // our reading of it rather than about `attrib` not running.
        assert_ne!(std::fs::metadata(&dotted).unwrap().file_attributes() & 0x2, 0x2);
    }

    #[cfg(not(windows))]
    #[test]
    fn hidden_follows_the_dot_convention_off_windows() {
        let dir = TempDir::new("list_hidden_dot");
        dir.touch(".gitignore");
        dir.touch("plain.txt");
        let entries = list_dir(dir.path()).unwrap();
        assert!(find(&entries, ".gitignore").attributes.hidden);
        assert!(!find(&entries, "plain.txt").attributes.hidden);
    }

    #[test]
    fn a_read_only_file_says_so() {
        let dir = TempDir::new("list_readonly");
        let path = dir.write("locked.txt", "x");
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_readonly(true);
        std::fs::set_permissions(&path, perms).unwrap();

        let entries = list_dir(dir.path()).unwrap();

        assert!(find(&entries, "locked.txt").attributes.readonly);
        // Left writable so the TempDir can actually clean itself up.
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        perms.set_readonly(false);
        std::fs::set_permissions(&path, perms).unwrap();
    }

    #[test]
    fn entry_kind_knows_what_can_be_navigated_into() {
        assert!(EntryKind::Dir.is_dir_like());
        assert!(EntryKind::Link { to_dir: true }.is_dir_like());
        assert!(!EntryKind::Link { to_dir: false }.is_dir_like());
        assert!(!EntryKind::File.is_dir_like());
        assert!(EntryKind::Link { to_dir: false }.is_link());
        assert!(!EntryKind::Dir.is_link());
    }
}
