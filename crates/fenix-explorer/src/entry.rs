use std::path::PathBuf;
use std::time::SystemTime;

pub use fenix_fs::{Attributes, EntryKind};

/// Git status of a tracked/untracked path, as reported by `git status
/// --porcelain`. `None` on `Entry::git_status` means "not in a git repo,
/// or `git` isn't on `PATH`" -- not "clean" (a genuinely clean tracked
/// file is also `None`, since porcelain output only lists paths that
/// differ from HEAD/are untracked; there's nothing to distinguish
/// "clean" from "no git" without listing the whole tree, which isn't
/// worth doing just to color unchanged files).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitStatus {
    Modified,
    Staged,
    Untracked,
    Ignored,
    Conflicted,
}

/// One row in a directory listing: what the filesystem said about it
/// (`fenix_fs::DirEntryInfo`) plus the two things only this listing
/// knows -- how deep it sits in the expanded tree, and what git makes
/// of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub path: PathBuf,
    pub kind: EntryKind,
    /// Bytes; always 0 for directories -- see `fenix_fs::DirEntryInfo`.
    pub size: u64,
    /// `None` when the entry's metadata could not be read, rendered as
    /// blank rather than as a fabricated timestamp.
    pub modified: Option<SystemTime>,
    pub attributes: Attributes,
    /// `false` when the entry could not be stat'd at all -- it is still
    /// listed (see `fenix_fs::list_dir`), just without trustworthy
    /// size/date/attributes.
    pub readable: bool,
    /// Indentation level within the current (possibly subtree-expanded)
    /// listing -- 0 for the top-level directory's own entries.
    pub depth: usize,
    pub git_status: Option<GitStatus>,
}

impl Entry {
    /// Whether this can be navigated into -- a directory, or a link to
    /// one. A method rather than a stored flag so there is exactly one
    /// answer to "is this a directory", derived from `kind`.
    pub fn is_dir(&self) -> bool {
        self.kind.is_dir_like()
    }

    /// Whether this entry should be hidden when hidden entries are being
    /// hidden. System files count: on Windows they are the ones nobody
    /// browsing a folder wants to see, and they are marked separately
    /// from ordinary hidden files precisely so they can be treated more
    /// firmly.
    pub fn is_hidden(&self) -> bool {
        self.attributes.hidden || self.attributes.system
    }

    /// The part of the name after its last dot, lowercased -- empty for
    /// a directory or a name with no extension. Used for sorting by
    /// type, where grouping every `.rs` together is the point.
    pub fn extension(&self) -> String {
        if self.is_dir() {
            return String::new();
        }
        self.name.rsplit_once('.').map(|(_, ext)| ext.to_lowercase()).unwrap_or_default()
    }

    pub(crate) fn from_fs(info: fenix_fs::DirEntryInfo, depth: usize) -> Self {
        Self {
            name: info.name,
            path: info.path,
            kind: info.kind,
            size: info.size,
            modified: info.modified,
            attributes: info.attributes,
            readable: info.readable,
            depth,
            git_status: None,
        }
    }
}
