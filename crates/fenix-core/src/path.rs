//! What a path means to an open document, however it was spelled.
//!
//! One file reaches Fenix under several spellings on Windows:
//! `std::fs::canonicalize` gives `\\?\C:\src\a.rs`, a language server
//! `C:\src\a.rs`, a project root joined with a forward-slashed name
//! `C:\src/a.rs`, and a drive letter can come in either case. A document
//! keeps one spelling, and an open file is found by its key.

use std::path::{Path, PathBuf};

/// `path` as a person would write it: no `\\?\` prefix, one kind of
/// separator. Elsewhere than Windows, `path`.
pub fn plain_path(path: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        use std::path::{Component, Prefix};
        let mut plain = PathBuf::new();
        for component in path.components() {
            match component {
                Component::Prefix(prefix) => match prefix.kind() {
                    Prefix::VerbatimDisk(drive) => plain.push(format!("{}:", drive as char)),
                    Prefix::VerbatimUNC(server, share) => plain.push(format!(
                        r"\\{}\{}",
                        server.to_string_lossy(),
                        share.to_string_lossy()
                    )),
                    _ => plain.push(prefix.as_os_str()),
                },
                other => plain.push(other.as_os_str()),
            }
        }
        plain
    }
    #[cfg(not(windows))]
    {
        path.to_path_buf()
    }
}

/// What two spellings of one file have in common: `plain_path`, and on
/// Windows, where names don't differ by case, in one case. For comparing
/// only: never shown, and never opened.
pub fn path_key(path: &Path) -> PathBuf {
    let plain = plain_path(path);
    #[cfg(windows)]
    {
        PathBuf::from(plain.to_string_lossy().to_lowercase())
    }
    #[cfg(not(windows))]
    {
        plain
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn plain_path_drops_the_verbatim_prefix_and_mixed_separators() {
        assert_eq!(
            plain_path(Path::new(r"\\?\C:\src\a.rs")).to_str(),
            Some(r"C:\src\a.rs")
        );
        assert_eq!(
            plain_path(Path::new(r"\\?\UNC\server\share\a.rs")).to_str(),
            Some(r"\\server\share\a.rs")
        );
        assert_eq!(
            plain_path(Path::new(r"C:\src/lib/a.rs")).to_str(),
            Some(r"C:\src\lib\a.rs")
        );
        assert_eq!(
            plain_path(Path::new(r"C:\src\a.rs")).to_str(),
            Some(r"C:\src\a.rs")
        );
    }

    #[cfg(windows)]
    #[test]
    fn every_spelling_of_a_file_has_one_key() {
        let key = path_key(Path::new(r"C:\src\a.rs"));
        for spelling in [
            r"\\?\C:\src\a.rs",
            r"C:\src/a.rs",
            r"c:\SRC\A.rs",
            r"\\?\c:\src\A.RS",
        ] {
            assert_eq!(path_key(Path::new(spelling)), key, "{spelling}");
        }
        assert_ne!(path_key(Path::new(r"C:\src\b.rs")), key);
    }

    #[cfg(not(windows))]
    #[test]
    fn elsewhere_a_path_is_itself() {
        assert_eq!(plain_path(Path::new("/src/A.rs")), Path::new("/src/A.rs"));
        assert_ne!(
            path_key(Path::new("/src/A.rs")),
            path_key(Path::new("/src/a.rs"))
        );
    }
}
