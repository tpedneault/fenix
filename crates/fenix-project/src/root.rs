use std::path::{Path, PathBuf};

/// Files/directories that mark a directory as a project root, checked in
/// no particular priority order -- any one of them is enough. `.git` and
/// `.projectile` are ecosystem-agnostic; the rest are the common
/// per-language "this is the top of a package" manifest.
const MARKERS: &[&str] = &[".git", ".projectile", "Cargo.toml", "package.json", "pyproject.toml", "go.mod", "CMakeLists.txt"];

/// An Arduino sketch folder: `sketch.yaml`, or a main `.ino`/`.pde` named
/// after the folder itself -- the one rule the Arduino tools hold every
/// sketch to. A sketch inside a larger repository is its own project, so
/// a class repo with one folder per lab gets one root per lab.
pub(crate) fn is_sketch(dir: &Path) -> bool {
    if dir.join("sketch.yaml").is_file() {
        return true;
    }
    let Some(name) = dir.file_name().and_then(|n| n.to_str()) else { return false };
    dir.join(format!("{name}.ino")).is_file() || dir.join(format!("{name}.pde")).is_file()
}

/// Walks up from `start` (a file or a directory -- a file's own directory
/// is where the walk begins) looking for the closest ancestor containing
/// any marker in `MARKERS`, or that is an Arduino sketch. Returns `None` if none is found all the way
/// to the filesystem root -- callers treat that as "not in a known
/// project," not an error.
pub fn find_project_root(start: &Path) -> Option<PathBuf> {
    let mut dir = if start.is_dir() { start } else { start.parent()? };
    loop {
        if dir.join(".fenix/tools.json").is_file()
            || dir.join(".fenix/project.ini").is_file()
            || MARKERS.iter().any(|marker| dir.join(marker).exists())
            || is_sketch(dir)
            || crate::kind::holds_mib_tables(dir)
        {
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
    fn an_arduino_sketch_inside_a_repository_is_its_own_root() {
        let dir = TempDir::new("sketch_marker");
        fs::create_dir_all(dir.path().join(".git")).unwrap();
        let sketch = dir.path().join("Lab1");
        fs::create_dir_all(sketch.join("src")).unwrap();
        fs::write(sketch.join("Lab1.ino"), b"void setup() {}").unwrap();
        assert_eq!(find_project_root(&sketch.join("src").join("pins.h")), Some(sketch.clone()));
        assert_eq!(find_project_root(&sketch.join("Lab1.ino")), Some(sketch));

        let stray = dir.path().join("notes");
        fs::create_dir_all(&stray).unwrap();
        fs::write(stray.join("scratch.ino"), b"").unwrap();
        assert_eq!(find_project_root(&stray.join("scratch.ino")), Some(dir.path().to_path_buf()), "an .ino not named after its folder isn't a sketch");
    }

    #[test]
    fn a_folder_of_mib_tables_is_a_project() {
        let dir = TempDir::new("mib_marker");
        fs::write(dir.path().join("vdf.dat"), b"M	m
").unwrap();
        assert_eq!(find_project_root(&dir.path().join("vdf.dat")), Some(dir.path().to_path_buf()));
    }

    #[test]
    fn finds_a_cargo_toml_marker() {
        let dir = TempDir::new("cargo_marker");
        fs::write(dir.path().join("Cargo.toml"), b"[package]").unwrap();
        assert_eq!(find_project_root(dir.path()), Some(dir.path().to_path_buf()));
    }

    #[test]
    fn finds_a_cmakelists_txt_marker() {
        let dir = TempDir::new("cmake_marker");
        fs::write(dir.path().join("CMakeLists.txt"), b"project(x)").unwrap();
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
