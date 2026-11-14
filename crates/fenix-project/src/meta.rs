//! What you've said about your projects beyond where they are: which are
//! pinned and which group each belongs to ("Mission ops", "Class labs").
//! That's yours, not the project's, so it lives beside the known
//! projects in `state/projects.json` rather than in the repository. What *is* the
//! project's -- its declared kind, its Jira key -- lives in its own
//! `.fenix/project.ini`, written here with `set_ini_value`.

use std::collections::HashSet;
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
    /// `state/projects.json`, beside the known projects.
    pub fn default_path() -> Option<PathBuf> {
        fenix_storage::paths::state_file("projects.json")
    }

    /// Loads it, starting empty when the file is missing or unreadable
    /// -- a convenience, not critical data.
    pub fn load_or_default(path: PathBuf) -> Self {
        let meta: Self = fenix_storage::state::read(&path, "meta").ok().flatten().unwrap_or_default();
        meta.plain(path)
    }

    /// The whole-file `project_meta.json` it used to be kept in, for
    /// moving it over; `None` when there's none to read.
    pub fn read_legacy(path: &Path, into: PathBuf) -> Option<Self> {
        let meta: Self = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
        Some(meta.plain(into))
    }

    /// Every root in the plain form the known projects keep them in
    /// (`crate::plain_path`), so a pin made on a `\\?\` root still
    /// finds its project.
    fn plain(self, path: PathBuf) -> Self {
        Self {
            pinned: self.pinned.into_iter().map(crate::plain_path).collect(),
            groups: self.groups.into_iter().map(|(root, group)| (crate::plain_path(root), group)).collect(),
            path,
        }
    }

    fn key(root: &Path) -> PathBuf {
        crate::plain_path(root.to_path_buf())
    }

    pub fn save(&self) -> io::Result<()> {
        fenix_storage::state::write(&self.path, "meta", self)
    }

    pub fn is_pinned(&self, root: &Path) -> bool {
        self.pinned.contains(&Self::key(root))
    }

    pub fn set_pinned(&mut self, root: &Path, pinned: bool) {
        if pinned {
            self.pinned.insert(Self::key(root));
        } else {
            self.pinned.remove(&Self::key(root));
        }
    }

    pub fn group(&self, root: &Path) -> Option<&str> {
        self.groups.get(&Self::key(root)).map(String::as_str)
    }

    /// Sets `root`'s group; a blank name ungroups it.
    pub fn set_group(&mut self, root: &Path, group: &str) {
        let group = group.trim();
        if group.is_empty() {
            self.groups.remove(&Self::key(root));
        } else {
            self.groups.insert(Self::key(root), group.to_string());
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
        let root = Self::key(root);
        self.pinned.remove(&root);
        self.groups.remove(&root);
    }
}

/// `root/.fenix/project.ini`, where a project's own settings were kept
/// before `.fenix/settings.toml`.
pub fn project_ini(root: &Path) -> PathBuf {
    root.join(".fenix").join("project.ini")
}

/// One key from the old `project.ini`, for a project not opened since.
fn legacy(root: &Path, section: &str, key: &str) -> Option<String> {
    let text = std::fs::read_to_string(project_ini(root)).ok()?;
    crate::kind::ini_value(&text, section, key).filter(|v| !v.trim().is_empty())
}

/// `[project] jira = "FNX"` -- the Jira project key a project's work is
/// tracked under.
pub fn jira_key(root: &Path) -> Option<String> {
    fenix_storage::project_file::get(root, "project.jira").or_else(|| legacy(root, "project", "jira"))
}

/// Sets (or, with `None`, removes) the project's Jira key.
pub fn set_jira_key(root: &Path, key: Option<&str>) -> io::Result<()> {
    fenix_storage::project_file::set(root, "project.jira", key.filter(|k| !k.trim().is_empty()).map(|k| fenix_storage::project_file::Value::Text(k.trim().to_string())))
}

/// Declares (or, with `None`, stops declaring) the project's kind.
pub fn set_kind(root: &Path, kind: Option<crate::ProjectKind>) -> io::Result<()> {
    set_kind_id(root, kind.map(|k| k.id()))
}

/// Declares the project's kind by its id, known or not (a template's
/// kind no installed extension declares), or stops declaring one.
pub fn set_kind_id(root: &Path, id: Option<&str>) -> io::Result<()> {
    fenix_storage::project_file::set(root, "project.kind", id.map(|id| fenix_storage::project_file::Value::Text(id.to_string())))
}

/// `[git] reviewers = ["alex", "sam"]` -- who a new pull request for
/// this project asks for a review.
pub fn reviewers(root: &Path) -> Option<Vec<String>> {
    fenix_storage::project_file::get_list(root, "git.reviewers").or_else(|| {
        let names: Vec<String> = legacy(root, "git", "reviewers")?.split([',', ' ']).map(|n| n.trim().trim_start_matches('@')).filter(|n| !n.is_empty()).map(str::to_string).collect();
        (!names.is_empty()).then_some(names)
    })
}

/// Whether `key` in `[section]` of `project.ini` is one the move reads
/// (`migrate_project_ini`): what it moves, or leaves for tools.json's own.
fn moved_key(section: &str, key: &str, value: &str) -> bool {
    match section {
        "project" => key == "kind" || key == "jira",
        "git" => key == "reviewers",
        "monitor" => key == "baudrate",
        "tasks" => key.starts_with("task") && value.split_once('|').is_some_and(|(_, c)| !c.trim().is_empty()),
        "launch" => key == "program" || key == "args",
        _ => false,
    }
}

/// Moves a project's `.fenix/project.ini` into `.fenix/settings.toml`
/// (its kind, Jira key, reviewers and serial monitor speed) and
/// `.fenix/tools.json` (its tasks and debug launch, where that doesn't
/// have them already). Nothing is lost, and the folder stays a project:
/// once everything in it is moved it's deleted; what the move doesn't
/// read (another section, another key, a line it can't parse) stays in
/// it, with its comments, the moved keys taken out; one with nothing to
/// move (empty, comments alone, only what it doesn't read) is left as it
/// is. `None` when there's no `project.ini` or nothing in it to move;
/// otherwise what moved.
pub fn migrate_project_ini(root: &Path) -> Option<Result<Vec<String>, String>> {
    let ini = project_ini(root);
    let text = std::fs::read_to_string(&ini).ok()?;
    // What of it the move reads, and what stays: the lines it doesn't,
    // under their sections, with the comments.
    let mut section = String::new();
    let mut any = false;
    let mut kept: Vec<(String, &str)> = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(header) = trimmed.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            section = header.trim().to_string();
            kept.push((section.clone(), line));
            continue;
        }
        let read = !trimmed.starts_with(['#', ';']) && trimmed.split_once('=').is_some_and(|(k, v)| moved_key(&section, k.trim(), v.trim()));
        if read {
            any = true;
        } else {
            kept.push((section.clone(), line));
        }
    }
    if !any {
        return None;
    }
    // A section's header stays when something of it that isn't a comment
    // or blank does.
    let content = |l: &str| {
        let t = l.trim();
        !t.is_empty() && !t.starts_with(['#', ';']) && !(t.starts_with('[') && t.ends_with(']'))
    };
    let rest: String = {
        let live: HashSet<&str> = kept.iter().filter(|(_, l)| content(l)).map(|(s, _)| s.as_str()).collect();
        let mut out = String::new();
        let mut header_live = true;
        for (sec, line) in &kept {
            let t = line.trim();
            if t.starts_with('[') && t.ends_with(']') {
                header_live = live.contains(sec.as_str());
            }
            if header_live || content(line) {
                out.push_str(line);
                out.push('\n');
            }
        }
        out
    };
    let rest_matters = kept.iter().any(|(_, l)| content(l));
    Some((|| {
        use fenix_storage::project_file::{set, Value};
        let mut moved = Vec::new();
        let value = |section: &str, key: &str| crate::kind::ini_value(&text, section, key).filter(|v| !v.trim().is_empty());
        let err = |e: io::Error| e.to_string();
        if let Some(kind) = value("project", "kind") {
            set(root, "project.kind", Some(Value::Text(kind))).map_err(err)?;
            moved.push("kind".to_string());
        }
        if let Some(jira) = value("project", "jira") {
            set(root, "project.jira", Some(Value::Text(jira))).map_err(err)?;
            moved.push("Jira key".to_string());
        }
        if let Some(names) = value("git", "reviewers") {
            let names: Vec<String> = names.split([',', ' ']).map(|n| n.trim().trim_start_matches('@')).filter(|n| !n.is_empty()).map(str::to_string).collect();
            set(root, "git.reviewers", Some(Value::List(names))).map_err(err)?;
            moved.push("reviewers".to_string());
        }
        if let Some(baud) = value("monitor", "baudrate").and_then(|b| b.trim().parse::<i64>().ok()) {
            set(root, "monitor.baudrate", Some(Value::Int(baud))).map_err(err)?;
            moved.push("monitor speed".to_string());
        }
        // Tasks and the launch go where tools.json keeps them now.
        let mut tools = crate::tools::ProjectTools::read(root)?;
        let mut changed = false;
        let mut in_section = String::new();
        for line in text.lines().map(str::trim) {
            if let Some(header) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
                in_section = header.trim().to_string();
                continue;
            }
            let Some((key, val)) = line.split_once('=') else { continue };
            let (key, val) = (key.trim(), val.trim());
            match in_section.as_str() {
                "tasks" if key.starts_with("task") => {
                    let Some((name, command)) = val.split_once('|') else { continue };
                    let mut parts = command.split_whitespace().map(str::to_string);
                    let Some(program) = parts.next() else { continue };
                    if !tools.tasks.contains_key(name.trim()) {
                        tools.tasks.insert(name.trim().to_string(), crate::tools::CommandSpec::new(program, parts.collect()));
                        changed = true;
                    }
                }
                "launch" if key == "program" && tools.launch.program.is_none() => {
                    tools.launch.program = Some(PathBuf::from(val));
                    changed = true;
                }
                "launch" if key == "args" && tools.launch.args.is_none() => {
                    tools.launch.args = Some(val.split_whitespace().map(str::to_string).collect());
                    changed = true;
                }
                _ => {}
            }
        }
        if changed {
            tools.write(root)?;
            moved.push("tasks and launch (to tools.json)".to_string());
        }
        if rest_matters {
            // What it doesn't read stays where it was.
            std::fs::write(&ini, rest).map_err(|e| format!("{}: {e}", ini.display()))?;
        } else {
            std::fs::remove_file(&ini).map_err(|e| format!("{}: {e}", ini.display()))?;
        }
        Ok(moved)
    })())
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
    fn project_ini_moves_into_settings_toml_and_tools_json_and_goes() {
        let dir = crate::test_util::TempDir::new("migrate_project_ini");
        let root = dir.path();
        dir.write(".fenix/project.ini", "[project]\nkind = rust\njira = FNX\n[git]\nreviewers = alex, @sam\n[monitor]\nbaudrate = 9600\n[tasks]\ntask1 = Build|cargo build --release\n[launch]\nprogram = target/debug/app\nargs = --verbose\n");
        let moved = migrate_project_ini(root).unwrap().unwrap();
        assert_eq!(moved.len(), 5, "{moved:?}");
        assert!(!project_ini(root).exists());
        assert_eq!(crate::declared_kind(root), Some(crate::ProjectKind::RUST));
        assert_eq!(jira_key(root).as_deref(), Some("FNX"));
        assert_eq!(reviewers(root), Some(vec!["alex".to_string(), "sam".to_string()]));
        assert_eq!(fenix_storage::project_file::get(root, "monitor.baudrate").as_deref(), Some("9600"));
        let tools = crate::tools::ProjectTools::read(root).unwrap();
        assert_eq!((tools.tasks["Build"].executable.as_str(), tools.tasks["Build"].args.clone()), ("cargo", vec!["build".to_string(), "--release".to_string()]));
        assert_eq!(tools.launch.program, Some(PathBuf::from("target/debug/app")));
        assert!(migrate_project_ini(root).is_none(), "nothing left to move");
    }

    /// A kind no installed extension declares moves as written, is
    /// ignored for what the project is, and is kept.
    #[test]
    fn a_kind_no_one_declares_survives_the_move() {
        let dir = crate::test_util::TempDir::new("migrate_unknown_kind");
        let root = dir.path();
        dir.write("Cargo.toml", "");
        dir.write(".fenix/project.ini", "[project]\nkind = java\n");
        migrate_project_ini(root).unwrap().unwrap();
        assert_eq!(fenix_storage::project_file::get(root, "project.kind").as_deref(), Some("java"));
        assert_eq!(crate::declared_id(root).as_deref(), Some("java"));
        assert_eq!(crate::declared_kind(root), None);
        assert_eq!(crate::detect_kind(root), crate::ProjectKind::RUST, "detection decides");
        set_kind_id(root, None).unwrap();
        assert_eq!(crate::declared_id(root), None);
        set_kind_id(root, Some("java")).unwrap();
        assert_eq!(fenix_storage::project_file::get(root, "project.kind").as_deref(), Some("java"));
    }
}
