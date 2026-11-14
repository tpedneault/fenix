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

    /// Once, ever: the known projects that were a folder of MIB tables
    /// inside a repository you also have as a project, taken off the list.
    /// Until 1.6 a folder of MIB tables was a project of its own; now it's
    /// in the project around it, so an entry for one opens that project
    /// under the MIB folder's name. It runs once and can't be redone, so it
    /// takes off only what's certain (`taken_by`); everything else stays,
    /// and opens as the folder you chose, as any folder you add does. Only
    /// the list changes -- nothing in the folder, its `.fenix/` included,
    /// and not its pin or group, which come back if it's added again.
    /// Returns each taken off with the project it's in now. A list it can't
    /// read or save is left as it is, to try again next time.
    pub fn drop_mib_folders_once(&mut self) -> Vec<(PathBuf, PathBuf)> {
        self.drop_mib_folders_once_from(dirs::home_dir().as_deref())
    }

    /// `drop_mib_folders_once`, with `home` the home folder.
    pub fn drop_mib_folders_once_from(&mut self, home: Option<&Path>) -> Vec<(PathBuf, PathBuf)> {
        if !matches!(fenix_storage::state::read::<bool>(&self.path, MIB_FOLDERS_DROPPED), Ok(None | Some(false))) {
            return Vec::new();
        }
        let dropped: Vec<(PathBuf, PathBuf)> = self.roots.iter().filter_map(|root| taken_by(root, home, &self.roots).map(|around| (root.clone(), around))).collect();
        if !dropped.is_empty() {
            let was = self.roots.clone();
            self.roots.retain(|r| !dropped.iter().any(|(d, _)| d == r));
            if self.save().is_err() {
                self.roots = was;
                return Vec::new();
            }
        }
        // Saved without it, it's done: a flag not written means looking
        // again next time, which finds nothing more to take.
        let _ = fenix_storage::state::write(&self.path, MIB_FOLDERS_DROPPED, &true);
        dropped
    }
}

const KEY: &str = "known";

/// Set once `drop_mib_folders_once` has been.
const MIB_FOLDERS_DROPPED: &str = "mib_folders_dropped";

/// The tables that made a folder a project before 1.6: its version table,
/// telecommands, TM parameters and packets, calibrations.
const MIB_TABLES: &[&str] = &["vdf.dat", "ccf.dat", "pcf.dat", "pid.dat", "tpcf.dat", "caf.dat", "txf.dat"];

/// The project `root` was taken into, when it's to come off the list: a
/// folder of MIB tables with nothing of a project's own -- no marker of
/// Fenix's or a language pack's, no kind its files give it (`setup.py`, a
/// `.tcl`), nothing in a `.fenix/` (snippets, settings) -- inside a project
/// that is on the list itself (`known`) and isn't the home folder or above
/// it (a dotfiles `.git`, a stray `package.json`). None, else.
fn taken_by(root: &Path, home: Option<&Path>, known: &[PathBuf]) -> Option<PathBuf> {
    let text = root.to_str()?;
    if fenix_vfs::path::is_remote_text(text) || !MIB_TABLES.iter().any(|t| root.join(t).is_file()) {
        return None;
    }
    let fenix_has_something = std::fs::read_dir(root.join(".fenix")).is_ok_and(|mut entries| entries.next().is_some());
    if fenix_has_something || crate::root::holds_pack_marker(root) || crate::detect_kind_from_files(root) != crate::ProjectKind::Other {
        return None;
    }
    let around = crate::find_project_root(root).filter(|around| around != root)?;
    if home.is_some_and(|home| home.starts_with(&around)) || !known.contains(&around) {
        return None;
    }
    Some(around)
}

/// `path` without Windows' extended-length `\\?\` prefix, which
/// `std::fs::canonicalize` always adds there: `\\?\C:\x` becomes `C:\x`
/// and `\\?\UNC\server\share` becomes `\\server\share`. Any other path
/// comes back as it is.
///
/// Both spellings name the same directory but compare unequal -- which
/// is how one project came to be listed twice.
pub fn plain_path(path: PathBuf) -> PathBuf {
    let Some(text) = path.to_str() else { return path };
    // A project on a remote machine: its `\\?\` is what makes it one.
    if fenix_vfs::path::is_remote_text(text) {
        return path;
    }
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

    /// A MIB folder in a repository on the list was a project of its own
    /// until 1.6: taken off the list, once, saying where it is now. Anything
    /// less certain stays -- and the folders are untouched.
    #[test]
    fn mib_folders_inside_a_listed_project_leave_the_list_once() {
        let dir = TempDir::new("known_mib");
        let (repo, home, home2, unlisted) = (dir.path().join("repo"), dir.path().join("home"), dir.path().join("home2"), dir.path().join("unlisted"));
        dir.write("repo/.git/HEAD", "ref: refs/heads/main\n");
        dir.write("repo/ops/vdf.dat", "OPS\tops\n");
        dir.write("repo/made/ccf.dat", "");
        dir.write("repo/made/.fenix/settings.toml", "[ccsds]\n");
        dir.write("repo/snip/pcf.dat", "");
        dir.write("repo/snip/.fenix/snippets/tc.toml", "");
        dir.write("repo/py/pcf.dat", "");
        dir.write("repo/py/setup.py", "");
        dir.write("repo/sub/pcf.dat", "");
        dir.write("repo/sub/.git/HEAD", "ref: refs/heads/main\n");
        dir.write("alone/pcf.dat", "");
        dir.write("home/.git/HEAD", "ref: refs/heads/main\n");
        dir.write("home/mibs/ops/pcf.dat", "");
        dir.write("home2/package.json", "{}");
        dir.write("home2/work/ops/pcf.dat", "");
        dir.write("unlisted/.git/HEAD", "ref: refs/heads/main\n");
        dir.write("unlisted/ops/pcf.dat", "");
        let ops = repo.join("ops");
        let kept: Vec<PathBuf> = [repo.join("made"), repo.join("snip"), repo.join("py"), repo.join("sub"), dir.path().join("alone"), home.join("mibs/ops"), home2.join("work/ops"), unlisted.join("ops")].into();
        let path = dir.path().join("state").join("projects.json");
        let mut known = KnownProjects::load_or_default(path.clone());
        // home2 itself isn't on the list: its package.json is no project of yours.
        for root in kept.iter().chain([&repo, &home, &ops]).rev() {
            known.add(root.clone());
        }
        known.save().unwrap();
        let before = known.roots().to_vec();

        let dropped = known.drop_mib_folders_once_from(Some(&home));
        assert_eq!(dropped, [(ops.clone(), repo.clone())]);
        let mut known = KnownProjects::load(path.clone()).unwrap();
        assert_eq!(known.roots(), before.iter().filter(|r| **r != ops).cloned().collect::<Vec<_>>(), "saved, the rest in order");
        assert!(ops.join("vdf.dat").is_file() && repo.join("snip/.fenix/snippets/tc.toml").is_file(), "the folders are untouched");

        // Added again by hand: it stays.
        known.add(ops.clone());
        known.save().unwrap();
        assert!(known.drop_mib_folders_once_from(Some(&home)).is_empty());
        assert_eq!(known.roots()[0], ops);
    }

    /// Above the home folder is no project of yours either: a stray
    /// `package.json` there takes nothing off.
    #[test]
    fn a_marker_above_home_takes_nothing() {
        let dir = TempDir::new("known_mib_home");
        dir.write("package.json", "{}");
        dir.write("me/work/ops/pcf.dat", "");
        let ops = dir.path().join("me/work/ops");
        let mut known = KnownProjects::load_or_default(dir.path().join("projects.json"));
        known.add(dir.path().to_path_buf());
        known.add(ops.clone());
        assert!(known.drop_mib_folders_once_from(Some(&dir.path().join("me"))).is_empty());
        assert!(known.roots().contains(&ops));
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
