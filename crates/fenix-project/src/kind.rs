//! What kind of project a root is -- the one fact the hub, Home, the
//! modeline and the new-project wizard all name a project by. Detected
//! from the same markers `find_project_root` walks for, or declared in
//! `.fenix/project.ini` when detection guesses wrong:
//!
//! ```ini
//! [project]
//! kind = mib
//! ```

use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProjectKind {
    Python,
    Arduino,
    /// An SCOS-2000 MIB workspace: `.dat` tables at the root or in `mib/`.
    Mib,
    Rust,
    Tcl,
    Cpp,
    Node,
    Go,
    /// A root with a marker (`.git`, `.projectile`) but nothing that says
    /// what's in it.
    Other,
}

/// The tables that say "this directory is a MIB": telecommands, TM
/// parameters and packets. Any one is enough -- a TM-only database has
/// no `ccf.dat`.
const MIB_TABLES: &[&str] = &["ccf.dat", "pcf.dat", "pid.dat", "tpcf.dat"];

impl ProjectKind {
    pub const ALL: [ProjectKind; 9] = [
        ProjectKind::Python,
        ProjectKind::Arduino,
        ProjectKind::Mib,
        ProjectKind::Rust,
        ProjectKind::Tcl,
        ProjectKind::Cpp,
        ProjectKind::Node,
        ProjectKind::Go,
        ProjectKind::Other,
    ];

    /// The name used in `project.ini` and `template.toml`.
    pub fn id(self) -> &'static str {
        match self {
            ProjectKind::Python => "python",
            ProjectKind::Arduino => "arduino",
            ProjectKind::Mib => "mib",
            ProjectKind::Rust => "rust",
            ProjectKind::Tcl => "tcl",
            ProjectKind::Cpp => "cpp",
            ProjectKind::Node => "node",
            ProjectKind::Go => "go",
            ProjectKind::Other => "other",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        let id = id.trim().to_ascii_lowercase();
        Self::ALL.into_iter().find(|kind| kind.id() == id).or(match id.as_str() {
            "py" | "uv" => Some(ProjectKind::Python),
            "ino" | "sketch" => Some(ProjectKind::Arduino),
            "rs" => Some(ProjectKind::Rust),
            "c++" | "c" | "cmake" => Some(ProjectKind::Cpp),
            "js" | "ts" | "javascript" | "typescript" => Some(ProjectKind::Node),
            _ => None,
        })
    }

    /// The two-or-three-letter tag every place a project is named shows
    /// in front of it -- the same everywhere, so it reads as one mark.
    pub fn tag(self) -> &'static str {
        match self {
            ProjectKind::Python => "PY",
            ProjectKind::Arduino => "INO",
            ProjectKind::Mib => "MIB",
            ProjectKind::Rust => "RS",
            ProjectKind::Tcl => "TCL",
            ProjectKind::Cpp => "C++",
            ProjectKind::Node => "JS",
            ProjectKind::Go => "GO",
            ProjectKind::Other => "DIR",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ProjectKind::Python => "Python",
            ProjectKind::Arduino => "Arduino",
            ProjectKind::Mib => "SCOS-2000 MIB",
            ProjectKind::Rust => "Rust",
            ProjectKind::Tcl => "Tcl",
            ProjectKind::Cpp => "C/C++",
            ProjectKind::Node => "JavaScript",
            ProjectKind::Go => "Go",
            ProjectKind::Other => "Project",
        }
    }
}

/// `root`'s kind: the one `.fenix/project.ini` declares, else the first
/// detection that matches, most specific first -- a sketch or a MIB can
/// live inside anything, so they're checked before the language manifests.
pub fn detect_kind(root: &Path) -> ProjectKind {
    if let Some(kind) = declared_kind(root) {
        return kind;
    }
    if crate::root::is_sketch(root) {
        return ProjectKind::Arduino;
    }
    if [root.to_path_buf(), root.join("mib")].iter().any(|dir| MIB_TABLES.iter().any(|t| dir.join(t).is_file())) {
        return ProjectKind::Mib;
    }
    let has = |name: &str| root.join(name).exists();
    if has("Cargo.toml") {
        ProjectKind::Rust
    } else if has("pyproject.toml") || has("setup.py") || has("requirements.txt") || has("uv.lock") {
        ProjectKind::Python
    } else if has("go.mod") {
        ProjectKind::Go
    } else if has("package.json") {
        ProjectKind::Node
    } else if has("CMakeLists.txt") {
        ProjectKind::Cpp
    } else if has("pkgIndex.tcl") || has_extension(root, "tcl") {
        ProjectKind::Tcl
    } else {
        ProjectKind::Other
    }
}

fn has_extension(dir: &Path, extension: &str) -> bool {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .any(|entry| entry.path().extension().is_some_and(|e| e.eq_ignore_ascii_case(extension)) && entry.path().is_file())
}

/// `[project] kind = ...` from `.fenix/project.ini`, if present and known.
pub fn declared_kind(root: &Path) -> Option<ProjectKind> {
    let text = std::fs::read_to_string(root.join(".fenix").join("project.ini")).ok()?;
    ini_value(&text, "project", "kind").and_then(|v| ProjectKind::from_id(&v))
}

/// One `key = value` from one `[section]` of an INI text -- the same
/// forgiving read `fenix-tasks`' `project.ini` reader does.
pub(crate) fn ini_value(text: &str, section: &str, key: &str) -> Option<String> {
    let mut in_section = false;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with(['#', ';']) {
            continue;
        }
        if let Some(header) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            in_section = header.trim() == section;
            continue;
        }
        if in_section {
            if let Some((k, v)) = line.split_once('=') {
                if k.trim() == key {
                    return Some(v.trim().to_string());
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::TempDir;

    #[test]
    fn detects_each_kind_from_its_marker() {
        let cases: &[(&str, ProjectKind)] = &[
            ("Cargo.toml", ProjectKind::Rust),
            ("pyproject.toml", ProjectKind::Python),
            ("go.mod", ProjectKind::Go),
            ("package.json", ProjectKind::Node),
            ("CMakeLists.txt", ProjectKind::Cpp),
            ("main.tcl", ProjectKind::Tcl),
            ("mib/ccf.dat", ProjectKind::Mib),
            ("pcf.dat", ProjectKind::Mib),
            (".projectile", ProjectKind::Other),
        ];
        for (file, kind) in cases {
            let dir = TempDir::new("kind");
            dir.write(file, "");
            assert_eq!(detect_kind(dir.path()), *kind, "{file}");
        }
    }

    #[test]
    fn a_sketch_is_arduino_even_with_other_manifests() {
        let dir = TempDir::new("kind_sketch");
        let sketch = dir.path().join("Blink");
        std::fs::create_dir_all(&sketch).unwrap();
        std::fs::write(sketch.join("Blink.ino"), "").unwrap();
        std::fs::write(sketch.join("CMakeLists.txt"), "").unwrap();
        assert_eq!(detect_kind(&sketch), ProjectKind::Arduino);
    }

    #[test]
    fn a_declared_kind_wins_over_detection() {
        let dir = TempDir::new("kind_declared");
        dir.write("Cargo.toml", "");
        dir.write(".fenix/project.ini", "[monitor]\nkind = rust\n[project]\n kind = MIB \n");
        assert_eq!(detect_kind(dir.path()), ProjectKind::Mib);
    }

    #[test]
    fn ids_and_aliases_round_trip() {
        for kind in ProjectKind::ALL {
            assert_eq!(ProjectKind::from_id(kind.id()), Some(kind));
        }
        assert_eq!(ProjectKind::from_id("uv"), Some(ProjectKind::Python));
        assert_eq!(ProjectKind::from_id("nonsense"), None);
    }
}
