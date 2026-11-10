//! The hosts a person has named in `~/.ssh/config`, to offer when they
//! open a remote project. Only the names: what a name means (the real
//! host, the user, the port, the key, the way through a jump host) stays
//! `ssh`'s to work out, when it's run with that name.

use std::path::{Path, PathBuf};

/// How deep `Include` is followed. `ssh` itself stops at 16.
const DEEPEST: usize = 8;

/// The `Host` names of the user's `ssh` configuration, in the order
/// they're written, each once. Patterns (`*.example.com`, `!bastion`)
/// aren't names and are left out.
pub fn hosts() -> Vec<String> {
    match home() {
        Some(home) => hosts_in(&home.join(".ssh").join("config")),
        None => Vec::new(),
    }
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).filter(|home| !home.is_empty()).map(PathBuf::from)
}

/// The `Host` names of one configuration file and those it includes.
pub fn hosts_in(config: &Path) -> Vec<String> {
    let mut names = Vec::new();
    read(config, 0, &mut names);
    names
}

fn read(config: &Path, depth: usize, names: &mut Vec<String>) {
    let Ok(text) = std::fs::read_to_string(config) else { return };
    for line in text.lines() {
        let line = line.trim();
        // `Host a b`, `Host=a`, `host  a`: a keyword, then its words.
        let (keyword, rest) = line.split_once(|c: char| c.is_whitespace() || c == '=').unwrap_or((line, ""));
        let words = rest.trim_start_matches(|c: char| c.is_whitespace() || c == '=');
        if keyword.eq_ignore_ascii_case("host") {
            for name in words.split_whitespace().map(|word| word.trim_matches('"')) {
                let pattern = name.contains(['*', '?', '!']);
                if !pattern && !name.is_empty() && !names.iter().any(|known| known == name) {
                    names.push(name.to_string());
                }
            }
        } else if keyword.eq_ignore_ascii_case("include") && depth < DEEPEST {
            for word in words.split_whitespace().map(|word| word.trim_matches('"')) {
                for included in included(config, word) {
                    read(&included, depth + 1, names);
                }
            }
        }
    }
}

/// The files an `Include` names: relative to the configuration's folder,
/// `~` for home, and `*` in the last part for several.
fn included(config: &Path, word: &str) -> Vec<PathBuf> {
    let path = match word.strip_prefix("~/") {
        Some(rest) => home().map(|home| home.join(rest)),
        None => Some(PathBuf::from(word)),
    };
    let Some(path) = path else { return Vec::new() };
    let path = if path.is_absolute() { path } else { config.parent().unwrap_or(Path::new(".")).join(path) };
    let pattern = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    if !pattern.contains('*') {
        return vec![path];
    }
    let (before, after) = pattern.split_once('*').expect("has a star");
    let Ok(folder) = std::fs::read_dir(path.parent().unwrap_or(Path::new("."))) else { return Vec::new() };
    let mut matched: Vec<PathBuf> = folder
        .flatten()
        .filter(|item| {
            let name = item.file_name().to_string_lossy().into_owned();
            name.len() >= before.len() + after.len() && name.starts_with(before) && name.ends_with(after)
        })
        .map(|item| item.path())
        .collect();
    matched.sort();
    matched
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_read_and_patterns_left_out() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config");
        std::fs::write(
            &config,
            "# my machines\nHost devbox lab-1\n    HostName 10.0.0.7\n    User tom\n\nhost=bastion\nHost *.example.com !bad ?x\n  ProxyJump bastion\nHost \"quoted\" devbox\nMatch host foo\n",
        )
        .unwrap();
        assert_eq!(hosts_in(&config), ["devbox", "lab-1", "bastion", "quoted"]);
        assert!(hosts_in(&dir.path().join("none")).is_empty());
    }

    #[test]
    fn included_files_are_read_where_they_are_named() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("conf.d")).unwrap();
        std::fs::write(dir.path().join("conf.d").join("b.conf"), "Host from-b\n").unwrap();
        std::fs::write(dir.path().join("conf.d").join("a.conf"), "Host from-a\n").unwrap();
        std::fs::write(dir.path().join("conf.d").join("notes.txt"), "Host not-this\n").unwrap();
        std::fs::write(dir.path().join("work"), "Host work\nInclude work\n").unwrap();
        let config = dir.path().join("config");
        std::fs::write(&config, "Include conf.d/*.conf work missing\nHost last\n").unwrap();
        assert_eq!(hosts_in(&config), ["from-a", "from-b", "work", "last"]);
    }
}
