use std::path::PathBuf;
use std::time::SystemTime;

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

/// One row in a directory listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    pub size: u64,
    pub modified: SystemTime,
    /// Indentation level within the current (possibly subtree-expanded)
    /// listing -- 0 for the top-level directory's own entries.
    pub depth: usize,
    pub git_status: Option<GitStatus>,
}
