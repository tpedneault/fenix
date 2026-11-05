use std::io;
use std::path::{Path, PathBuf};

/// A remembered, most-recently-used-ordered list of project roots --
/// the `known` key of `state/projects.json`, which it shares with the
/// projects' groups and pins (`ProjectMeta`).
pub struct KnownProjects {
    path: PathBuf,
    roots: Vec<PathBuf>,
}

impl KnownProjects {
    /// `state/projects.json`.
    pub fn default_path() -> Option<PathBuf> {
        fenix_storage::paths::state_file("projects.json")
    }

    /// Loads the known-projects list from `path`. A missing file means
    /// "no known projects yet," not an error -- the common case on first
    /// run, before anything's ever been saved.
    ///
    /// Roots come back in their plain form, and one saved twice -- once
    /// plain, once with Windows' `\\?\` prefix -- comes back once, where
    /// the first of the two was.
    pub fn load(path: PathBuf) -> io::Result<Self> {
        let saved: Vec<PathBuf> = fenix_storage::state::read(&path, KEY)?.unwrap_or_default();
        let mut roots: Vec<PathBuf> = Vec::with_capacity(saved.len());
        for root in saved.into_iter().map(plain_path) {
            if !roots.contains(&root) {
                roots.push(root);
            }
        }
        Ok(Self { path, roots })
    }

    /// Same as `load`, but never fails -- any read error (not just a
    /// missing file) just starts with an empty list. This is a
    /// convenience cache, not critical data; a caller starting up
    /// shouldn't have to special-case "couldn't read my project
    /// history" as a hard error.
    pub fn load_or_default(path: PathBuf) -> Self {
        Self::load(path.clone()).unwrap_or_else(|_| Self { path, roots: Vec::new() })
    }

    pub fn roots(&self) -> &[PathBuf] {
        &self.roots
    }

    /// Adds `root` to the front of the list, or moves it there if it's
    /// already known -- most-recently-used first. Callers decide when
    /// this fires: explicit registration (`SPC p a`) and re-selecting an
    /// already-known project from the switch-project picker both call
    /// this, but opening an arbitrary file no longer does (see the
    /// project-management plan's own note on why auto-registration was
    /// dropped in favor of an explicitly curated list).
    ///
    /// `root` is kept in its plain form (`plain_path`), so a directory
    /// reached once through `canonicalize` and once not is one project.
    pub fn add(&mut self, root: PathBuf) {
        let root = plain_path(root);
        self.roots.retain(|r| r != &root);
        self.roots.insert(0, root);
    }

    /// Removes `root` from the list if present. Returns whether it
    /// actually was -- lets a caller distinguish "removed" from "wasn't
    /// there to begin with," though most callers (picking `root` from a
    /// picker built off `roots()` itself) can only ever hit the former.
    pub fn remove(&mut self, root: &Path) -> bool {
        let root = plain_path(root.to_path_buf());
        let before = self.roots.len();
        self.roots.retain(|r| r != &root);
        self.roots.len() != before
    }

    pub fn save(&self) -> io::Result<()> {
        fenix_storage::state::write(&self.path, KEY, &self.roots)
    }
}

const KEY: &str = "known";

/// `path` without Windows' extended-length `\\?\` prefix, which
/// `std::fs::canonicalize` always adds there: `\\?\C:\x` becomes `C:\x`
/// and `\\?\UNC\server\share` becomes `\\server\share`. Any other path
/// comes back as it is.
///
/// Both spellings name the same directory but compare unequal -- which
/// is how one project came to be listed twice.
pub fn plain_path(path: PathBuf) -> PathBuf {
    let Some(text) = path.to_str() else { return path };
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{rest}"))
    } else if let Some(rest) = text.strip_prefix(r"\\?\") {
        PathBuf::from(rest)
    } else {
        path
    }
}

/// A list of paths, one per line -- how `projects.txt`, `recent_files.txt`
/// and `recent_dirs.txt` were kept before the state files. For moving
/// them over.
pub fn read_path_list(path: &Path) -> io::Result<Vec<PathBuf>> {
    Ok(std::fs::read_to_string(path)?.lines().map(str::trim).filter(|l| !l.is_empty()).map(PathBuf::from).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::TempDir;

    #[test]
    fn loading_a_missing_file_yields_an_empty_list_not_an_error() {
        let dir = TempDir::new("known_missing_file");
        let known = KnownProjects::load(dir.path().join("does-not-exist.txt")).unwrap();
        assert!(known.roots().is_empty());
    }

    #[test]
    fn load_or_default_never_fails_even_when_the_path_is_unreadable() {
        // A directory (not a file) at the target path makes `read_to_string`
        // fail with something other than `NotFound` -- `load` would
        // propagate that; `load_or_default` should just start empty.
        let dir = TempDir::new("known_load_or_default_unreadable");
        let path = dir.path().join("actually-a-directory");
        std::fs::create_dir(&path).unwrap();

        assert!(KnownProjects::load(path.clone()).is_err());
        let known = KnownProjects::load_or_default(path);
        assert!(known.roots().is_empty());
    }

    #[test]
    fn add_then_save_then_load_round_trips() {
        let dir = TempDir::new("known_round_trip");
        let path = dir.path().join("projects.txt");

        let mut known = KnownProjects::load(path.clone()).unwrap();
        known.add(PathBuf::from("/repo/one"));
        known.add(PathBuf::from("/repo/two"));
        known.save().unwrap();

        let reloaded = KnownProjects::load(path).unwrap();
        assert_eq!(reloaded.roots(), &[PathBuf::from("/repo/two"), PathBuf::from("/repo/one")]);
    }

    #[test]
    fn add_moves_an_existing_entry_to_the_front_instead_of_duplicating() {
        let dir = TempDir::new("known_add_moves_to_front");
        let mut known = KnownProjects::load(dir.path().join("does-not-exist.txt")).unwrap();
        known.add(PathBuf::from("/repo/one"));
        known.add(PathBuf::from("/repo/two"));
        known.add(PathBuf::from("/repo/one")); // re-visit -- should move, not duplicate

        assert_eq!(known.roots(), &[PathBuf::from("/repo/one"), PathBuf::from("/repo/two")]);
    }

    #[test]
    fn remove_drops_the_matching_entry_and_reports_it_was_there() {
        let dir = TempDir::new("known_remove");
        let mut known = KnownProjects::load(dir.path().join("does-not-exist.txt")).unwrap();
        known.add(PathBuf::from("/repo/one"));
        known.add(PathBuf::from("/repo/two"));

        assert!(known.remove(&PathBuf::from("/repo/one")));
        assert_eq!(known.roots(), &[PathBuf::from("/repo/two")]);
    }

    #[test]
    fn remove_of_an_unknown_root_is_a_no_op_and_reports_it_was_not_there() {
        let dir = TempDir::new("known_remove_unknown");
        let mut known = KnownProjects::load(dir.path().join("does-not-exist.txt")).unwrap();
        known.add(PathBuf::from("/repo/one"));

        assert!(!known.remove(&PathBuf::from("/repo/nonexistent")));
        assert_eq!(known.roots(), &[PathBuf::from("/repo/one")]);
    }

    #[test]
    fn plain_path_drops_the_verbatim_prefix() {
        assert_eq!(plain_path(PathBuf::from(r"\\?\C:\Users\me\proj")), PathBuf::from(r"C:\Users\me\proj"));
        assert_eq!(plain_path(PathBuf::from(r"\\?\UNC\server\share\proj")), PathBuf::from(r"\\server\share\proj"));
        assert_eq!(plain_path(PathBuf::from(r"C:\Users\me\proj")), PathBuf::from(r"C:\Users\me\proj"));
        assert_eq!(plain_path(PathBuf::from("/repo/one")), PathBuf::from("/repo/one"));
    }

    #[test]
    fn a_verbatim_root_is_the_same_project_as_its_plain_spelling() {
        let dir = TempDir::new("known_verbatim_same_project");
        let mut known = KnownProjects::load(dir.path().join("does-not-exist.txt")).unwrap();
        known.add(PathBuf::from(r"C:\repo\one"));
        known.add(PathBuf::from(r"\\?\C:\repo\one"));
        assert_eq!(known.roots(), &[PathBuf::from(r"C:\repo\one")]);

        assert!(known.remove(Path::new(r"\\?\C:\repo\one")));
        assert!(known.roots().is_empty());
    }

    #[test]
    fn duplicates_already_saved_collapse_on_load() {
        let dir = TempDir::new("known_duplicates_collapse");
        let path = dir.path().join("projects.json");
        let saved = vec![PathBuf::from(r"\\?\C:\repo\one"), PathBuf::from(r"C:\repo\two"), PathBuf::from(r"C:\repo\one")];
        fenix_storage::state::write(&path, KEY, &saved).unwrap();

        let known = KnownProjects::load(path).unwrap();
        assert_eq!(known.roots(), &[PathBuf::from(r"C:\repo\one"), PathBuf::from(r"C:\repo\two")]);
    }

    #[test]
    fn save_creates_missing_parent_directories() {
        let dir = TempDir::new("known_creates_parents");
        let path = dir.path().join("nested").join("config").join("projects.txt");
        let mut known = KnownProjects::load(path.clone()).unwrap();
        known.add(PathBuf::from("/repo"));
        known.save().unwrap();
        assert!(path.exists());
    }
}
