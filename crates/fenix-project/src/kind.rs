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
    /// A repository of many projects. Declared (`kind = monorepo`), not
    /// guessed: plenty of single projects have a Cargo or uv workspace.
    Monorepo,
    /// A root with a marker (`.git`, `.projectile`) but nothing that says
    /// what's in it.
    Other,
}

/// The tables that say "this directory is a MIB": its version table,
/// telecommands, TM parameters and packets, calibrations. Any one is
/// enough -- a TM-only database has no `ccf.dat`.
const MIB_TABLES: &[&str] = &["vdf.dat", "ccf.dat", "pcf.dat", "pid.dat", "tpcf.dat", "caf.dat", "txf.dat"];

/// Whether `dir` holds MIB tables itself (not in a `mib/` below it).
pub(crate) fn holds_mib_tables(dir: &Path) -> bool {
    MIB_TABLES.iter().any(|t| dir.join(t).is_file())
}

impl ProjectKind {
    pub const ALL: [ProjectKind; 10] = [
        ProjectKind::Python,
        ProjectKind::Arduino,
        ProjectKind::Mib,
        ProjectKind::Rust,
        ProjectKind::Tcl,
        ProjectKind::Cpp,
        ProjectKind::Node,
        ProjectKind::Go,
        ProjectKind::Monorepo,
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
            ProjectKind::Monorepo => "monorepo",
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
            ProjectKind::Monorepo => "MONO",
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
            ProjectKind::Monorepo => "Monorepo",
            ProjectKind::Other => "Project",
        }
    }
}

/// `root`'s kind: the one `.fenix/project.ini` declares, else the first
/// detection that matches, most specific first -- a sketch or a MIB can
/// live inside anything, so they're checked before the language manifests.
pub fn detect_kind(root: &Path) -> ProjectKind {
    declared_kind(root).unwrap_or_else(|| detect_kind_from_files(root))
}

/// What `root`'s files say it is, whatever `project.ini` declares -- what
/// the settings page offers as "detect".
pub fn detect_kind_from_files(root: &Path) -> ProjectKind {
    if crate::root::is_sketch(root) || root.join("library.properties").is_file() {
        return ProjectKind::Arduino;
    }
    if holds_mib_tables(root) || holds_mib_tables(&root.join("mib")) {
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

/// The file a project of `kind` is usually entered through -- a sketch's
/// `.ino`, `src/main.rs`, a Python package's `__init__.py` -- if it has
/// one. What opening a project lands on when you've never had a file of
/// it open.
pub fn main_file(root: &Path, kind: ProjectKind) -> Option<std::path::PathBuf> {
    let name = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let first_in = |dir: &Path, extension: &str| -> Option<std::path::PathBuf> {
        let mut found: Vec<_> = std::fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).filter(|p| p.is_file() && p.extension().is_some_and(|e| e.eq_ignore_ascii_case(extension))).collect();
        found.sort();
        found.into_iter().next()
    };
    let candidates: Vec<std::path::PathBuf> = match kind {
        ProjectKind::Arduino => vec![root.join(format!("{name}.ino")), root.join(format!("{name}.pde")), root.join(format!("src/{name}.cpp")), root.join(format!("src/{name}.h"))],
        ProjectKind::Rust => vec![root.join("src/main.rs"), root.join("src/lib.rs")],
        ProjectKind::Python => {
            let mut c = vec![root.join("main.py"), root.join("app.py")];
            // A src-layout package: src/<package>/__init__.py.
            let mut packages: Vec<_> = std::fs::read_dir(root.join("src")).into_iter().flatten().flatten().map(|e| e.path().join("__init__.py")).collect();
            packages.sort();
            c.extend(packages);
            c
        }
        ProjectKind::Go => vec![root.join("main.go")],
        ProjectKind::Node => vec![root.join("src/index.ts"), root.join("src/index.js"), root.join("index.ts"), root.join("index.js")],
        ProjectKind::Cpp => vec![root.join("src/main.cpp"), root.join("main.cpp"), root.join("src/main.c"), root.join("main.c")],
        ProjectKind::Tcl => vec![root.join("main.tcl")],
        ProjectKind::Mib => ["ccf.dat", "pcf.dat", "vdf.dat"].iter().flat_map(|t| [root.join(t), root.join("mib").join(t)]).collect(),
        ProjectKind::Monorepo | ProjectKind::Other => Vec::new(),
    };
    candidates.into_iter().find(|p| p.is_file()).or_else(|| if kind == ProjectKind::Tcl { first_in(root, "tcl") } else { None })
}

fn has_extension(dir: &Path, extension: &str) -> bool {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .any(|entry| entry.path().extension().is_some_and(|e| e.eq_ignore_ascii_case(extension)) && entry.path().is_file())
}

/// `[project] kind = ...` from the project's `.fenix/settings.toml` --
/// or from the `.fenix/project.ini` it used to be kept in, for a project
/// not opened since -- if present and known.
pub fn declared_kind(root: &Path) -> Option<ProjectKind> {
    let value = fenix_storage::project_file::get(root, "project.kind").or_else(|| {
        let text = std::fs::read_to_string(root.join(".fenix").join("project.ini")).ok()?;
        ini_value(&text, "project", "kind")
    })?;
    ProjectKind::from_id(&value)
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
            ("vdf.dat", ProjectKind::Mib),
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
    fn main_files_are_found_by_kind() {
        let dir = TempDir::new("main_file");
        dir.write("Lab/Lab.ino", "");
        assert_eq!(main_file(&dir.path().join("Lab"), ProjectKind::Arduino), Some(dir.path().join("Lab/Lab.ino")));
        dir.write("py/src/orbit/__init__.py", "");
        assert_eq!(main_file(&dir.path().join("py"), ProjectKind::Python), Some(dir.path().join("py/src/orbit/__init__.py")));
        dir.write("rs/src/lib.rs", "");
        assert_eq!(main_file(&dir.path().join("rs"), ProjectKind::Rust), Some(dir.path().join("rs/src/lib.rs")));
        dir.write("db/vdf.dat", "");
        dir.write("db/pcf.dat", "");
        assert_eq!(main_file(&dir.path().join("db"), ProjectKind::Mib), Some(dir.path().join("db/pcf.dat")));
        assert_eq!(main_file(&dir.path().join("rs"), ProjectKind::Go), None);
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
