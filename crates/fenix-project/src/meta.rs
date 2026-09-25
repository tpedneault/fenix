//! What you've said about your projects beyond where they are: which are
//! pinned and which group each belongs to ("Mission ops", "Class labs").
//! That's yours, not the project's, so it lives beside `projects.txt` in
//! the config directory rather than in the repository. What *is* the
//! project's -- its declared kind, its Jira key -- lives in its own
//! `.fenix/project.ini`, written here with `set_ini_value`.

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProjectMeta {
    pinned: BTreeSet<PathBuf>,
    groups: BTreeMap<PathBuf, String>,
    #[serde(skip)]
    path: PathBuf,
}

impl ProjectMeta {
    /// `config_dir/fenix/project_meta.json`.
    pub fn default_path() -> Option<PathBuf> {
        dirs::config_dir().map(|dir| dir.join("fenix").join("project_meta.json"))
    }

    /// Loads it, starting empty when the file is missing or unreadable
    /// -- a convenience, not critical data.
    pub fn load_or_default(path: PathBuf) -> Self {
        let mut meta: Self = std::fs::read_to_string(&path).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default();
        meta.path = path;
        meta
    }

    pub fn save(&self) -> io::Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&self.path, serde_json::to_string_pretty(self).map_err(io::Error::other)? + "\n")
    }

    pub fn is_pinned(&self, root: &Path) -> bool {
        self.pinned.contains(root)
    }

    pub fn set_pinned(&mut self, root: &Path, pinned: bool) {
        if pinned {
            self.pinned.insert(root.to_path_buf());
        } else {
            self.pinned.remove(root);
        }
    }

    pub fn group(&self, root: &Path) -> Option<&str> {
        self.groups.get(root).map(String::as_str)
    }

    /// Sets `root`'s group; a blank name ungroups it.
    pub fn set_group(&mut self, root: &Path, group: &str) {
        let group = group.trim();
        if group.is_empty() {
            self.groups.remove(root);
        } else {
            self.groups.insert(root.to_path_buf(), group.to_string());
        }
    }

    /// Every group in use, sorted -- what cycling a project's group
    /// offers.
    pub fn groups(&self) -> Vec<String> {
        let set: BTreeSet<&String> = self.groups.values().collect();
        set.into_iter().cloned().collect()
    }

    /// Forgets `root` entirely (it was removed from the project list).
    pub fn forget(&mut self, root: &Path) {
        self.pinned.remove(root);
        self.groups.remove(root);
    }
}

/// `root/.fenix/project.ini`.
pub fn project_ini(root: &Path) -> PathBuf {
    root.join(".fenix").join("project.ini")
}

/// `[project] jira = FNX` -- the Jira project key a project's work is
/// tracked under.
pub fn jira_key(root: &Path) -> Option<String> {
    let text = std::fs::read_to_string(project_ini(root)).ok()?;
    crate::kind::ini_value(&text, "project", "jira").filter(|v| !v.is_empty())
}

/// `[git] reviewers = alex, sam` -- who a new pull request for this
/// project asks for a review, as written.
pub fn reviewers(root: &Path) -> Option<String> {
    let text = std::fs::read_to_string(project_ini(root)).ok()?;
    crate::kind::ini_value(&text, "git", "reviewers").filter(|v| !v.trim().is_empty())
}

/// Sets `key` in `[section]` of the INI file at `path` -- replacing the
/// line if it's there, adding it (and the section) if not, removing it
/// when `value` is `None` -- and leaves every other line as it was.
pub fn set_ini_value(path: &Path, section: &str, key: &str, value: Option<&str>) -> io::Result<()> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e),
    };
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let header = |line: &str| line.trim().strip_prefix('[').and_then(|s| s.strip_suffix(']')).map(|s| s.trim().to_string());
    let start = lines.iter().position(|l| header(l).as_deref() == Some(section));
    let new_line = value.map(|v| format!("{key} = {v}"));
    match start {
        Some(start) => {
            let end = lines[start + 1..].iter().position(|l| header(l).is_some()).map_or(lines.len(), |i| start + 1 + i);
            let existing = (start + 1..end).find(|&i| lines[i].split_once('=').is_some_and(|(k, _)| k.trim() == key));
            match (existing, new_line) {
                (Some(i), Some(line)) => lines[i] = line,
                (Some(i), None) => {
                    lines.remove(i);
                }
                (None, Some(line)) => {
                    // After the section's last non-blank line.
                    let at = (start + 1..end).rev().find(|&i| !lines[i].trim().is_empty()).map_or(start + 1, |i| i + 1);
                    lines.insert(at, line);
                }
                (None, None) => return Ok(()),
            }
        }
        None => {
            let Some(line) = new_line else { return Ok(()) };
            if lines.last().is_some_and(|l| !l.trim().is_empty()) {
                lines.push(String::new());
            }
            lines.push(format!("[{section}]"));
            lines.push(line);
        }
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, lines.join("\n") + "\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::TempDir;

    #[test]
    fn pins_and_groups_round_trip() {
        let dir = TempDir::new("meta_round_trip");
        let path = dir.path().join("meta.json");
        let mut meta = ProjectMeta::load_or_default(path.clone());
        meta.set_pinned(Path::new("/a"), true);
        meta.set_group(Path::new("/b"), " Class labs ");
        meta.set_group(Path::new("/c"), "Mission ops");
        meta.save().unwrap();
        let mut back = ProjectMeta::load_or_default(path);
        assert!(back.is_pinned(Path::new("/a")));
        assert_eq!(back.group(Path::new("/b")), Some("Class labs"));
        assert_eq!(back.groups(), ["Class labs", "Mission ops"]);
        back.set_group(Path::new("/b"), "");
        back.forget(Path::new("/a"));
        assert_eq!(back.group(Path::new("/b")), None);
        assert!(!back.is_pinned(Path::new("/a")));
    }

    #[test]
    fn ini_values_are_set_added_and_removed_in_place() {
        let dir = TempDir::new("meta_ini");
        let path = dir.path().join(".fenix/project.ini");
        set_ini_value(&path, "project", "jira", Some("FNX")).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "[project]\njira = FNX\n");
        std::fs::write(&path, "# mine\n[monitor]\nbaudrate = 9600\n\n[project]\nkind = mib\n").unwrap();
        set_ini_value(&path, "project", "jira", Some("OPS")).unwrap();
        set_ini_value(&path, "monitor", "baudrate", Some("115200")).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "# mine\n[monitor]\nbaudrate = 115200\n\n[project]\nkind = mib\njira = OPS\n");
        assert_eq!(jira_key(dir.path()).as_deref(), Some("OPS"));
        set_ini_value(&path, "project", "kind", None).unwrap();
        assert!(!std::fs::read_to_string(&path).unwrap().contains("kind"));
    }
}
