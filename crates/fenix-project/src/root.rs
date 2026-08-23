use std::path::{Path, PathBuf};

/// Files/directories that mark a directory as a project root, checked in
/// no particular priority order -- any one of them is enough. `.git` and
/// `.projectile` are ecosystem-agnostic; the rest are the common
/// per-language "this is the top of a package" manifest.
const MARKERS: &[&str] = &[".git", ".projectile", "Cargo.toml", "package.json", "pyproject.toml", "go.mod"];

/// Walks up from `start` (a file or a directory -- a file's own directory
/// is where the walk begins) looking for the closest ancestor containing
/// any marker in `MARKERS`. Returns `None` if none is found all the way
/// to the filesystem root -- callers treat that as "not in a known
/// project," not an error.
pub fn find_project_root(start: &Path) -> Option<PathBuf> {
    let mut dir = if start.is_dir() { start } else { start.parent()? };
    loop {
        if MARKERS.iter().any(|marker| dir.join(marker).exists()) {
            return Some(dir.to_path_buf());
        }
        dir = dir.parent()?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::TempDir;
    use std::fs;

    #[test]
    fn finds_a_git_marker_several_levels_up() {
        let dir = TempDir::new("git_marker");
        fs::create_dir(dir.path().join(".git")).unwrap();
        let nested = dir.path().join("src").join("deep");
        fs::create_dir_all(&nested).unwrap();

        assert_eq!(find_project_root(&nested), Some(dir.path().to_path_buf()));
    }

    #[test]
    fn finds_a_cargo_toml_marker() {
        let dir = TempDir::new("cargo_marker");
        fs::write(dir.path().join("Cargo.toml"), b"[package]").unwrap();
        assert_eq!(find_project_root(dir.path()), Some(dir.path().to_path_buf()));
    }

    #[test]
    fn finds_the_explicit_projectile_marker() {
        let dir = TempDir::new("projectile_marker");
        fs::write(dir.path().join(".projectile"), b"").unwrap();
        assert_eq!(find_project_root(dir.path()), Some(dir.path().to_path_buf()));
    }

    #[test]
    fn accepts_a_file_path_and_searches_from_its_directory() {
        let dir = TempDir::new("file_path_input");
        fs::create_dir(dir.path().join(".git")).unwrap();
        let file = dir.path().join("main.rs");
        fs::write(&file, b"").unwrap();
        assert_eq!(find_project_root(&file), Some(dir.path().to_path_buf()));
    }

    #[test]
    fn the_closest_ancestor_with_a_marker_wins_over_a_farther_one() {
        let dir = TempDir::new("closest_wins");
        fs::create_dir(dir.path().join(".git")).unwrap(); // outer root
        let inner = dir.path().join("vendor").join("subcrate");
        fs::create_dir_all(&inner).unwrap();
        fs::write(inner.join("Cargo.toml"), b"[package]").unwrap(); // closer marker

        assert_eq!(find_project_root(&inner), Some(inner.clone()));
    }

    #[test]
    fn no_marker_anywhere_up_returns_none() {
        // A fresh temp dir with no markers anywhere in its ancestry
        // within the walk (relies on nothing above the OS temp dir
        // having a stray marker, true in any normal environment).
        let dir = TempDir::new("no_marker");
        assert_eq!(find_project_root(dir.path()), None);
    }
}
