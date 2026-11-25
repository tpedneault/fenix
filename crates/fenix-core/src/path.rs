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
/// Windows, where names don't differ by case, in one case; on macOS, with the
/// links in it followed (`/tmp` is `/private/tmp`) and in the disk's own
/// case. For comparing
/// only: never shown, and never opened.
///
/// A file on a remote machine (`fenix_vfs::remote`) keeps its case
/// wherever Fenix runs: `Makefile` and `makefile` are two files there.
pub fn path_key(path: &Path) -> PathBuf {
    let plain = plain_path(path);
    #[cfg(windows)]
    {
        if fenix_vfs::is_remote(&plain) {
            return plain;
        }
        PathBuf::from(plain.to_string_lossy().to_lowercase())
    }
    #[cfg(target_os = "macos")]
    {
        if fenix_vfs::is_remote(&plain) {
            return plain;
        }
        real_path(&plain).unwrap_or(plain)
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        plain
    }
}

/// `path` as the disk has it: links followed, and each name in the case it
/// was made with, which is the only way to know whether a Mac's volume
/// tells cases apart (most don't). A file not there yet is its folder's,
/// under the name it's given.
#[cfg(target_os = "macos")]
fn real_path(path: &Path) -> Option<PathBuf> {
    if let Ok(real) = path.canonicalize() {
        return Some(real);
    }
    Some(path.parent()?.canonicalize().ok()?.join(path.file_name()?))
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

    #[test]
    fn a_remote_path_stays_as_it_is_and_keeps_its_case() {
        let upper = fenix_vfs::remote("devbox", "/src/Makefile");
        assert_eq!(plain_path(&upper), upper);
        assert_eq!(path_key(&upper), upper);
        assert_ne!(path_key(&upper), path_key(&fenix_vfs::remote("devbox", "/src/makefile")));
        assert_ne!(path_key(&upper), path_key(&fenix_vfs::remote("other", "/src/Makefile")));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_macs_links_and_cases_have_one_key() {
        let name = format!("fenix-path-key-{}", std::process::id());
        let dir = Path::new("/tmp").join(&name);
        std::fs::create_dir_all(&dir).unwrap();
        let real = Path::new("/private/tmp").join(&name);
        assert_eq!(path_key(&dir.join("new.rs")), real.join("new.rs"), "one not there yet");
        std::fs::write(dir.join("A.rs"), "").unwrap();
        assert_eq!(path_key(&dir.join("A.rs")), real.join("A.rs"));
        if dir.join("a.rs").exists() {
            assert_eq!(path_key(&dir.join("a.rs")), real.join("A.rs"));
        }
        std::fs::remove_dir_all(&dir).unwrap();
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
