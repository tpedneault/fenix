use std::io;
use std::path::PathBuf;

/// Cap on how many paths `RecentFiles` keeps. Unlike `KnownProjects`
/// (only ever grows via an explicit `SPC p a`), this list grows
/// automatically from ordinary file-opening, so it needs a bound to stay
/// from growing forever over a long-lived config directory.
const MAX_RECENT_FILES: usize = 200;

/// A remembered, most-recently-opened-first list of individual file
/// paths -- persisted the same way as `KnownProjects` (a plain
/// newline-separated path list, no serialization crate needed for
/// something this simple), just for files instead of project roots.
pub struct RecentFiles {
    path: PathBuf,
    paths: Vec<PathBuf>,
}

impl RecentFiles {
    /// The default location: `dirs::config_dir()/fenix/recent_files.txt`.
    pub fn default_path() -> Option<PathBuf> {
        dirs::config_dir().map(|dir| dir.join("fenix").join("recent_files.txt"))
    }

    /// Where the explorer remembers the directories it has been.
    ///
    /// The same machinery, a different file. Directories and files are
    /// both "somewhere you were recently", and both want the same
    /// most-recent-first, capped, plain-text-list treatment -- but they
    /// are answers to different questions (`SPC f r` reopens a file,
    /// `SPC e r` goes back to a folder), so mixing them into one list
    /// would make both worse.
    pub fn default_dirs_path() -> Option<PathBuf> {
        dirs::config_dir().map(|dir| dir.join("fenix").join("recent_dirs.txt"))
    }

    /// Loads the recent-files list from `path`. A missing file means "no
    /// recent files yet," not an error -- the common case on first run.
    pub fn load(path: PathBuf) -> io::Result<Self> {
        let paths = match std::fs::read_to_string(&path) {
            Ok(contents) => contents.lines().filter(|l| !l.is_empty()).map(PathBuf::from).collect(),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(e),
        };
        Ok(Self { path, paths })
    }

    /// Same as `load`, but never fails -- any read error just starts
    /// with an empty list, the same "convenience cache, not critical
    /// data" posture as `KnownProjects::load_or_default`.
    pub fn load_or_default(path: PathBuf) -> Self {
        Self::load(path.clone()).unwrap_or_else(|_| Self { path, paths: Vec::new() })
    }

    pub fn paths(&self) -> &[PathBuf] {
        &self.paths
    }

    /// Adds `path` to the front of the list, or moves it there if it's
    /// already known -- most-recently-opened first. Truncates to
    /// `MAX_RECENT_FILES` afterward, dropping the oldest entries.
    pub fn add(&mut self, path: PathBuf) {
        self.paths.retain(|p| p != &path);
        self.paths.insert(0, path);
        self.paths.truncate(MAX_RECENT_FILES);
    }

    pub fn save(&self) -> io::Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let contents: String = self.paths.iter().map(|p| format!("{}\n", p.display())).collect();
        std::fs::write(&self.path, contents)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::TempDir;

    #[test]
    fn loading_a_missing_file_yields_an_empty_list_not_an_error() {
        let dir = TempDir::new("recent_missing_file");
        let recent = RecentFiles::load(dir.path().join("does-not-exist.txt")).unwrap();
        assert!(recent.paths().is_empty());
    }

    #[test]
    fn load_or_default_never_fails_even_when_the_path_is_unreadable() {
        let dir = TempDir::new("recent_load_or_default_unreadable");
        let path = dir.path().join("actually-a-directory");
        std::fs::create_dir(&path).unwrap();

        assert!(RecentFiles::load(path.clone()).is_err());
        let recent = RecentFiles::load_or_default(path);
        assert!(recent.paths().is_empty());
    }

    #[test]
    fn add_then_save_then_load_round_trips() {
        let dir = TempDir::new("recent_round_trip");
        let path = dir.path().join("recent_files.txt");

        let mut recent = RecentFiles::load(path.clone()).unwrap();
        recent.add(PathBuf::from("/repo/one.rs"));
        recent.add(PathBuf::from("/repo/two.rs"));
        recent.save().unwrap();

        let reloaded = RecentFiles::load(path).unwrap();
        assert_eq!(reloaded.paths(), &[PathBuf::from("/repo/two.rs"), PathBuf::from("/repo/one.rs")]);
    }

    #[test]
    fn add_moves_an_existing_entry_to_the_front_instead_of_duplicating() {
        let dir = TempDir::new("recent_add_moves_to_front");
        let mut recent = RecentFiles::load(dir.path().join("does-not-exist.txt")).unwrap();
        recent.add(PathBuf::from("/repo/one.rs"));
        recent.add(PathBuf::from("/repo/two.rs"));
        recent.add(PathBuf::from("/repo/one.rs")); // re-visit -- should move, not duplicate

        assert_eq!(recent.paths(), &[PathBuf::from("/repo/one.rs"), PathBuf::from("/repo/two.rs")]);
    }

    #[test]
    fn add_truncates_to_the_max_entry_cap() {
        let dir = TempDir::new("recent_cap");
        let mut recent = RecentFiles::load(dir.path().join("does-not-exist.txt")).unwrap();
        for i in 0..MAX_RECENT_FILES + 10 {
            recent.add(PathBuf::from(format!("/repo/file{i}.rs")));
        }
        assert_eq!(recent.paths().len(), MAX_RECENT_FILES);
        // Most recently added stays at the front, oldest ones fall off.
        assert_eq!(recent.paths()[0], PathBuf::from(format!("/repo/file{}.rs", MAX_RECENT_FILES + 9)));
    }

    #[test]
    fn save_creates_missing_parent_directories() {
        let dir = TempDir::new("recent_creates_parents");
        let path = dir.path().join("nested").join("config").join("recent_files.txt");
        let mut recent = RecentFiles::load(path.clone()).unwrap();
        recent.add(PathBuf::from("/repo/one.rs"));
        recent.save().unwrap();
        assert!(path.exists());
    }
}
