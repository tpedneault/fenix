use std::io;
use std::path::PathBuf;

/// A remembered, most-recently-used-ordered list of project roots --
/// persisted as a plain newline-separated path list. Not TOML/JSON: a
/// flat list of paths doesn't need a serialization format, so this
/// avoids a `serde` dependency for something this simple.
pub struct KnownProjects {
    path: PathBuf,
    roots: Vec<PathBuf>,
}

impl KnownProjects {
    /// The default location: `dirs::config_dir()/fenix/projects.txt`.
    /// `None` on the rare platform where `dirs::config_dir()` itself
    /// returns `None` (no notion of a config directory at all).
    pub fn default_path() -> Option<PathBuf> {
        dirs::config_dir().map(|dir| dir.join("fenix").join("projects.txt"))
    }

    /// Loads the known-projects list from `path`. A missing file means
    /// "no known projects yet," not an error -- the common case on first
    /// run, before anything's ever been saved.
    pub fn load(path: PathBuf) -> io::Result<Self> {
        let roots = match std::fs::read_to_string(&path) {
            Ok(contents) => contents.lines().filter(|l| !l.is_empty()).map(PathBuf::from).collect(),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(e),
        };
        Ok(Self { path, roots })
    }

    pub fn roots(&self) -> &[PathBuf] {
        &self.roots
    }

    /// Adds `root` to the front of the list, or moves it there if it's
    /// already known -- most-recently-used first, matching Projectile's
    /// own auto-registration of every project you visit a file in.
    pub fn add(&mut self, root: PathBuf) {
        self.roots.retain(|r| r != &root);
        self.roots.insert(0, root);
    }

    pub fn save(&self) -> io::Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let contents: String = self.roots.iter().map(|r| format!("{}\n", r.display())).collect();
        std::fs::write(&self.path, contents)
    }
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
    fn save_creates_missing_parent_directories() {
        let dir = TempDir::new("known_creates_parents");
        let path = dir.path().join("nested").join("config").join("projects.txt");
        let mut known = KnownProjects::load(path.clone()).unwrap();
        known.add(PathBuf::from("/repo"));
        known.save().unwrap();
        assert!(path.exists());
    }
}
