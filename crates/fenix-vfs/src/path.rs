//! Remote paths: a machine's name and a path there, as one `PathBuf`.
//!
//! The root is `\\?\fenix-ssh\` on Windows and `/fenix-ssh/` elsewhere,
//! then the host, then the path. On Windows the `\\?\` is what makes it
//! safe: Windows looks such a path up as written, finds no device of
//! that name, and says "not found" at once. (Without it, `\\fenix-ssh\…`
//! is a server to look for on the network, for seconds.)
//!
//! The remote machine's names are not this machine's. A name there may
//! hold `:` or `\`, and its paths start with `/`; `PathBuf::join` reads
//! all three its own way (`join("/etc")` drops the host, `join("a:b")`
//! takes `a:` for a drive). So a remote path is only ever built by
//! [`remote`] and [`child`], and only taken apart by [`split`].

use std::path::{Component, Path, PathBuf};

/// The name of the root.
pub const ROOT: &str = "fenix-ssh";

/// What stands for `\` in a remote name on Windows, where a path can't
/// hold one: a private-use character, as Cygwin and WSL do.
#[cfg(windows)]
const BACKSLASH: char = '\u{f05c}';

/// The path of `there`, an absolute path on `host`, with `/`.
///
/// `host` is whatever names the connection (`devbox`, `tom@10.0.0.7`);
/// it can't hold a `/` or `\`, see [`host_is_valid`].
pub fn remote(host: &str, there: &str) -> PathBuf {
    debug_assert!(host_is_valid(host), "{host:?} can't name a host");
    #[cfg(windows)]
    let mut path = format!(r"\\?\{ROOT}\{host}");
    #[cfg(not(windows))]
    let mut path = format!("/{ROOT}/{host}");
    for name in there.split('/').filter(|name| !name.is_empty() && *name != ".") {
        #[cfg(windows)]
        {
            path.push('\\');
            path.extend(name.chars().map(|c| if c == '\\' { BACKSLASH } else { c }));
        }
        #[cfg(not(windows))]
        {
            path.push('/');
            path.push_str(name);
        }
    }
    PathBuf::from(path)
}

/// Whether `host` can be part of a remote path.
pub fn host_is_valid(host: &str) -> bool {
    !host.is_empty() && !host.contains(['/', '\\']) && host != "." && host != ".."
}

/// A remote path's host and its path there (`/` for the machine's root);
/// `None` for a local path.
pub fn split(path: &Path) -> Option<(String, String)> {
    let mut components = path.components();
    #[cfg(windows)]
    {
        use std::path::Prefix;
        match components.next()? {
            Component::Prefix(prefix) if matches!(prefix.kind(), Prefix::Verbatim(root) if root == ROOT) => {}
            _ => return None,
        }
        if components.next()? != Component::RootDir {
            return None;
        }
    }
    #[cfg(not(windows))]
    {
        if components.next()? != Component::RootDir || components.next()? != Component::Normal(ROOT.as_ref()) {
            return None;
        }
    }
    let Component::Normal(host) = components.next()? else { return None };
    let mut names: Vec<String> = Vec::new();
    for component in components {
        match component {
            Component::Normal(name) if name == ".." => drop(names.pop()),
            Component::ParentDir => drop(names.pop()),
            Component::Normal(name) => {
                let name = name.to_string_lossy().into_owned();
                #[cfg(windows)]
                let name = name.replace(BACKSLASH, "\\");
                names.push(name);
            }
            _ => {}
        }
    }
    Some((host.to_string_lossy().into_owned(), format!("/{}", names.join("/"))))
}

pub fn is_remote(path: &Path) -> bool {
    host_of(path).is_some()
}

/// The host a remote path is on.
pub fn host_of(path: &Path) -> Option<String> {
    split(path).map(|(host, _)| host)
}

/// `name` inside `folder`, where `name` is one name as its machine gave
/// it. For a local folder this is `join`; for a remote one it's what
/// `join` gets wrong.
pub fn child(folder: &Path, name: &str) -> PathBuf {
    match split(folder) {
        Some((host, there)) => remote(&host, &format!("{there}/{name}")),
        None => folder.join(name),
    }
}

/// A path as it's shown: `host:/home/tom/a.rs` for a remote one, the
/// path itself for a local one.
pub fn display(path: &Path) -> String {
    match split(path) {
        Some((host, there)) => format!("{host}:{there}"),
        None => path.display().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_remote_path_is_built_and_taken_apart() {
        let path = remote("tom@devbox", "/home/tom/proj/src/main.rs");
        assert_eq!(split(&path), Some(("tom@devbox".into(), "/home/tom/proj/src/main.rs".into())));
        assert_eq!(display(&path), "tom@devbox:/home/tom/proj/src/main.rs");
        assert!(is_remote(&path));
        assert!(path.is_absolute());
        assert_eq!(path.file_name().unwrap(), "main.rs");
        assert_eq!(path.extension().unwrap(), "rs");
        assert_eq!(path.parent().unwrap(), remote("tom@devbox", "/home/tom/proj/src"));
        assert!(path.starts_with(remote("tom@devbox", "/home/tom/proj")));
        assert_eq!(path.strip_prefix(remote("tom@devbox", "/home/tom/proj")).unwrap(), Path::new("src").join("main.rs"));
    }

    #[test]
    fn the_root_of_a_machine_and_untidy_paths() {
        assert_eq!(split(&remote("devbox", "/")), Some(("devbox".into(), "/".into())));
        assert_eq!(split(&remote("devbox", "//home/./tom/")), Some(("devbox".into(), "/home/tom".into())));
        // A relative join of plain names is safe, and `..` climbs.
        let up = remote("devbox", "/home/tom").join("..").join("ann");
        assert_eq!(split(&up), Some(("devbox".into(), "/home/ann".into())));
    }

    #[test]
    fn a_local_path_is_not_remote() {
        for local in ["/home/tom/a.rs", "relative/a.rs", r"C:\src\a.rs", r"\\server\share\a.rs", r"\\?\C:\src\a.rs", "/fenix-sshx/devbox/a", ""] {
            assert_eq!(split(Path::new(local)), None, "{local}");
        }
        assert_eq!(child(Path::new("folder"), "a.rs"), Path::new("folder").join("a.rs"));
    }

    #[test]
    fn names_this_machine_would_misread_survive() {
        let folder = remote("devbox", "/srv");
        for name in ["a:b", "C:", "trailing.", "sp ace", "back\\slash", "CON", "ünï"] {
            let path = child(&folder, name);
            assert_eq!(split(&path), Some(("devbox".into(), format!("/srv/{name}"))), "{name}");
            assert_eq!(path.parent().unwrap(), folder, "{name}");
        }
        // Two names that differ by case are two files there.
        assert_ne!(child(&folder, "Makefile"), child(&folder, "makefile"));
    }

    #[test]
    fn hosts_that_cannot_be_part_of_a_path_are_refused() {
        assert!(host_is_valid("tom@devbox") && host_is_valid("10.0.0.7") && host_is_valid("[::1]"));
        assert!(!host_is_valid("") && !host_is_valid("a/b") && !host_is_valid(r"a\b") && !host_is_valid(".."));
    }
}
