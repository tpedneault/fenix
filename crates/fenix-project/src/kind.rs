//! What kind of project a root is -- the one fact the hub, Home, the
//! modeline and the new-project wizard all name a project by. Detected
//! from the same markers `find_project_root` walks for, or declared in
//! `.fenix/settings.toml` (once `.fenix/project.ini`) when detection
//! guesses wrong:
//!
//! ```toml
//! [project]
//! kind = "monorepo"
//! ```
//!
//! A kind is data: an id, a name, a tag, a colour, the files that mark
//! one and the files it's entered through. Fenix declares nine of its own
//! here, in code; a kind it reads that nobody declares -- `kind = "java"`
//! where no installed extension knows Java -- is kept by its id and shown
//! by it, muted, never rewritten.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex, RwLock};

/// A project kind: a handle to an interned id. `Copy`, and the same id is
/// the same handle for the whole run, whoever declares it now -- so a
/// handle a page or a cache holds never turns into another kind. Fenix's
/// own are constants (`ProjectKind::RUST`), and `match` takes them.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProjectKind(u16);

impl std::fmt::Debug for ProjectKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ProjectKind({:?})", self.id())
    }
}

/// The theme colour a kind's tag is drawn in: one of the theme's syntax
/// roles (`keyword`, `string` ...), never a colour value, so it's
/// readable on every theme; or one of the three only Fenix's own use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KindColor {
    /// One of `KindColor::ROLES`.
    Syntax(&'static str),
    /// The insert-mode colour (Arduino's).
    Insert,
    /// The plain foreground (Monorepo's).
    Fg,
    /// The gutter's muted colour: Other's, and a kind no one declares.
    Muted,
}

impl KindColor {
    /// The theme roles a kind may name: the syntax colours every theme
    /// sets and keeps apart.
    pub const ROLES: [&'static str; 8] = ["keyword", "string", "function", "type", "number", "constant", "attribute", "variable"];

    /// What a kind that names none is drawn in.
    pub const DEFAULT: KindColor = KindColor::Syntax("constant");

    /// One of `ROLES` by its name.
    pub fn from_role(role: &str) -> Option<KindColor> {
        Self::ROLES.iter().find(|r| **r == role).map(|r| KindColor::Syntax(r))
    }
}

/// What a kind is: everything the places that name one ask.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KindDef {
    pub id: String,
    pub name: String,
    /// One to four of `A-Z 0-9 + # .`.
    pub tag: String,
    pub color: KindColor,
    /// Files at a project's top that say it's of this kind (`{folder}` is
    /// the folder's own name). None: it's only ever declared.
    pub markers: Vec<String>,
    /// The files a project of it is entered through, `/` between parts,
    /// the first that exists.
    pub main: Vec<String>,
    /// The extension that declared it; `None` for Fenix's own.
    pub extension: Option<String>,
}

/// Fenix's nine: id, tag, name, colour, markers -- in the order they're
/// listed and cycled in. The index is the constant's.
struct BuiltIn {
    id: &'static str,
    tag: &'static str,
    label: &'static str,
    color: KindColor,
    /// What `def()` lists; detection is `detect_kind_from_files`'s own
    /// code (a sketch, `*.tcl` at the top are rules, not names).
    markers: &'static [&'static str],
    main: &'static [&'static str],
}

const BUILT_IN: [BuiltIn; 9] = [
    BuiltIn { id: "python", tag: "PY", label: "Python", color: KindColor::Syntax("type"), markers: &["pyproject.toml", "setup.py", "requirements.txt", "uv.lock"], main: &["main.py", "app.py"] },
    BuiltIn { id: "arduino", tag: "INO", label: "Arduino", color: KindColor::Insert, markers: &["library.properties", "sketch.yaml", "{folder}.ino", "{folder}.pde"], main: &["{folder}.ino", "{folder}.pde", "src/{folder}.cpp", "src/{folder}.h"] },
    BuiltIn { id: "rust", tag: "RS", label: "Rust", color: KindColor::Syntax("string"), markers: &["Cargo.toml"], main: &["src/main.rs", "src/lib.rs"] },
    BuiltIn { id: "tcl", tag: "TCL", label: "Tcl", color: KindColor::Syntax("function"), markers: &["pkgIndex.tcl"], main: &["main.tcl"] },
    BuiltIn { id: "cpp", tag: "C++", label: "C/C++", color: KindColor::Syntax("keyword"), markers: &["CMakeLists.txt"], main: &["src/main.cpp", "main.cpp", "src/main.c", "main.c"] },
    BuiltIn { id: "node", tag: "JS", label: "JavaScript", color: KindColor::Syntax("number"), markers: &["package.json"], main: &["src/index.ts", "src/index.js", "index.ts", "index.js"] },
    BuiltIn { id: "go", tag: "GO", label: "Go", color: KindColor::Syntax("type"), markers: &["go.mod"], main: &["main.go"] },
    // A repository of many projects. Declared (`kind = monorepo`), not
    // guessed: plenty of single projects have a Cargo or uv workspace.
    BuiltIn { id: "monorepo", tag: "MONO", label: "Monorepo", color: KindColor::Fg, markers: &[], main: &[] },
    // A root with a marker (`.git`, `.projectile`) but nothing that says
    // what's in it.
    BuiltIn { id: "other", tag: "DIR", label: "Project", color: KindColor::Muted, markers: &[], main: &[] },
];

/// The other names Fenix reads as one of its own, lowercased. A MIB was a
/// kind of its own until 1.6: a project that still says so is a project
/// like any other.
const ALIASES: &[(&str, ProjectKind)] = &[
    ("py", ProjectKind::PYTHON),
    ("uv", ProjectKind::PYTHON),
    ("ino", ProjectKind::ARDUINO),
    ("sketch", ProjectKind::ARDUINO),
    ("rs", ProjectKind::RUST),
    ("c++", ProjectKind::CPP),
    ("c", ProjectKind::CPP),
    ("cmake", ProjectKind::CPP),
    ("js", ProjectKind::NODE),
    ("ts", ProjectKind::NODE),
    ("javascript", ProjectKind::NODE),
    ("typescript", ProjectKind::NODE),
    ("mib", ProjectKind::OTHER),
];

/// The longest id there is.
pub const MAX_ID: usize = 24;

/// How many ids one run interns at most, Fenix's included. Every id is
/// one a manifest, a template or a project's settings said, and each is
/// at most `MAX_ID` long; past this many, a new one is read as no kind at
/// all, as a malformed one is.
const MAX_INTERNED: usize = 4096;

struct Interner {
    ids: Vec<&'static str>,
    by_id: HashMap<&'static str, u16>,
}

static INTERNER: LazyLock<RwLock<Interner>> = LazyLock::new(|| {
    let ids: Vec<&'static str> = BUILT_IN.iter().map(|b| b.id).collect();
    let by_id = ids.iter().enumerate().map(|(i, id)| (*id, i as u16)).collect();
    RwLock::new(Interner { ids, by_id })
});

/// Text kept for the run, once each: the ids, names and tags kinds are
/// shown by, so `tag()` and `label()` hand out `&'static str` without an
/// allocation each frame. Bounded by what's read: manifests' limited
/// fields and the interned ids.
fn keep(text: &str) -> &'static str {
    static KEPT: LazyLock<Mutex<HashSet<&'static str>>> = LazyLock::new(|| Mutex::new(HashSet::new()));
    let mut kept = KEPT.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(found) = kept.get(text) {
        return found;
    }
    let leaked: &'static str = Box::leak(text.to_string().into_boxed_str());
    kept.insert(leaked);
    leaked
}

impl ProjectKind {
    pub const PYTHON: Self = Self(0);
    pub const ARDUINO: Self = Self(1);
    pub const RUST: Self = Self(2);
    pub const TCL: Self = Self(3);
    pub const CPP: Self = Self(4);
    pub const NODE: Self = Self(5);
    pub const GO: Self = Self(6);
    pub const MONOREPO: Self = Self(7);
    pub const OTHER: Self = Self(8);

    /// Fenix's own, in the order they're listed and cycled in.
    pub const BUILT_IN: [Self; 9] = [Self::PYTHON, Self::ARDUINO, Self::RUST, Self::TCL, Self::CPP, Self::NODE, Self::GO, Self::MONOREPO, Self::OTHER];

    fn built_in(self) -> Option<&'static BuiltIn> {
        BUILT_IN.get(self.0 as usize)
    }

    /// Whether it's one of Fenix's nine.
    pub fn is_builtin(self) -> bool {
        self.built_in().is_some()
    }

    /// The name used in `.fenix/settings.toml` and `template.toml`.
    pub fn id(self) -> &'static str {
        if let Some(b) = self.built_in() {
            return b.id;
        }
        INTERNER.read().unwrap_or_else(|e| e.into_inner()).ids.get(self.0 as usize).copied().unwrap_or("other")
    }

    /// Whether `id` is an id as a kind's declaration writes one: 1 to 24
    /// characters, `a-z` first, then `a-z 0-9 - _`.
    pub fn is_well_formed(id: &str) -> bool {
        let mut chars = id.chars();
        chars.next().is_some_and(|c| c.is_ascii_lowercase()) && id.len() <= MAX_ID && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
    }

    /// The other names of Fenix's kinds (`py`, `ts`, `mib` ...): read as
    /// the kind, never declared by anyone else.
    pub fn aliases() -> impl Iterator<Item = &'static str> {
        ALIASES.iter().map(|(alias, _)| *alias)
    }

    /// A built-in id or alias, in any case; else any well-formed id
    /// (`is_well_formed`, as written), interned. `None` for anything else.
    pub fn from_id(id: &str) -> Option<Self> {
        let id = id.trim();
        let lower = id.to_ascii_lowercase();
        if let Some(i) = BUILT_IN.iter().position(|b| b.id == lower) {
            return Some(Self(i as u16));
        }
        if let Some((_, kind)) = ALIASES.iter().find(|(alias, _)| *alias == lower) {
            return Some(*kind);
        }
        if !Self::is_well_formed(id) {
            return None;
        }
        if let Some(&i) = INTERNER.read().unwrap_or_else(|e| e.into_inner()).by_id.get(id) {
            return Some(Self(i));
        }
        let mut interner = INTERNER.write().unwrap_or_else(|e| e.into_inner());
        if let Some(&i) = interner.by_id.get(id) {
            return Some(Self(i));
        }
        if interner.ids.len() >= MAX_INTERNED {
            return None;
        }
        let kept = keep(id);
        let i = interner.ids.len() as u16;
        interner.ids.push(kept);
        interner.by_id.insert(kept, i);
        Some(Self(i))
    }

    /// Who declares it now: `None` for an id no one does.
    pub fn def(self) -> Option<KindDef> {
        let b = self.built_in()?;
        Some(KindDef {
            id: b.id.to_string(),
            name: b.label.to_string(),
            tag: b.tag.to_string(),
            color: b.color,
            markers: b.markers.iter().map(|m| m.to_string()).collect(),
            main: b.main.iter().map(|m| m.to_string()).collect(),
            extension: None,
        })
    }

    /// Whether anyone declares it now: Fenix, or an installed extension.
    pub fn is_declared(self) -> bool {
        self.is_builtin()
    }

    /// The one-to-four-character tag every place a project is named shows
    /// in front of it -- the same everywhere, so it reads as one mark. For
    /// an id no one declares, the id, upper-cased, cut to four.
    pub fn tag(self) -> &'static str {
        if let Some(b) = self.built_in() {
            return b.tag;
        }
        let tag: String = self.id().to_ascii_uppercase().chars().take(4).collect();
        keep(&tag)
    }

    /// Its name, as the settings page, the doctor and the explorer say it;
    /// for an id no one declares, the id.
    pub fn label(self) -> &'static str {
        if let Some(b) = self.built_in() {
            return b.label;
        }
        self.id()
    }

    /// What its tag is drawn in.
    pub fn color(self) -> KindColor {
        self.built_in().map_or(KindColor::Muted, |b| b.color)
    }

    /// Every kind declared now: Fenix's in their order, then the
    /// extensions' by name. What the settings page and the hub cycle.
    pub fn known() -> Vec<Self> {
        Self::BUILT_IN.to_vec()
    }
}

/// `root`'s kind: the one `.fenix/settings.toml` declares, else the first
/// detection that matches, most specific first -- a sketch can live
/// inside anything, so it's checked before the language manifests.
pub fn detect_kind(root: &Path) -> ProjectKind {
    declared_kind(root).unwrap_or_else(|| detect_kind_from_files(root))
}

/// What `root`'s files say it is, whatever its settings declare -- what
/// the settings page offers as "detect".
pub fn detect_kind_from_files(root: &Path) -> ProjectKind {
    if crate::root::is_sketch(root) || root.join("library.properties").is_file() {
        return ProjectKind::ARDUINO;
    }
    let has = |name: &str| root.join(name).exists();
    if has("Cargo.toml") {
        ProjectKind::RUST
    } else if has("pyproject.toml") || has("setup.py") || has("requirements.txt") || has("uv.lock") {
        ProjectKind::PYTHON
    } else if has("go.mod") {
        ProjectKind::GO
    } else if has("package.json") {
        ProjectKind::NODE
    } else if has("CMakeLists.txt") {
        ProjectKind::CPP
    } else if has("pkgIndex.tcl") || has_extension(root, "tcl") {
        ProjectKind::TCL
    } else {
        ProjectKind::OTHER
    }
}

/// `root` joined with `path`, a path below it with `/` between its parts
/// and `{folder}` for the folder's own name.
fn below(root: &Path, path: &str, folder: &str) -> PathBuf {
    path.replace("{folder}", folder).split('/').filter(|part| !part.is_empty()).fold(root.to_path_buf(), |at, part| at.join(part))
}

/// The file a project of `kind` is usually entered through -- a sketch's
/// `.ino`, `src/main.rs`, a Python package's `__init__.py` -- if it has
/// one. What opening a project lands on when you've never had a file of
/// it open.
pub fn main_file(root: &Path, kind: ProjectKind) -> Option<PathBuf> {
    let name = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let first_in = |dir: &Path, extension: &str| -> Option<PathBuf> {
        let mut found: Vec<_> = std::fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).filter(|p| p.is_file() && p.extension().is_some_and(|e| e.eq_ignore_ascii_case(extension))).collect();
        found.sort();
        found.into_iter().next()
    };
    let mut candidates: Vec<PathBuf> = kind.def().map(|d| d.main.iter().map(|m| below(root, m, &name)).collect()).unwrap_or_default();
    if kind == ProjectKind::PYTHON {
        // A src-layout package: src/<package>/__init__.py.
        let mut packages: Vec<_> = std::fs::read_dir(root.join("src")).into_iter().flatten().flatten().map(|e| e.path().join("__init__.py")).collect();
        packages.sort();
        candidates.extend(packages);
    }
    candidates.into_iter().find(|p| p.is_file()).or_else(|| if kind == ProjectKind::TCL { first_in(root, "tcl") } else { None })
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
/// not opened since -- if present and an id (`ProjectKind::from_id`):
/// one no installed extension declares is kept, not ignored.
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
            ("Cargo.toml", ProjectKind::RUST),
            ("pyproject.toml", ProjectKind::PYTHON),
            ("go.mod", ProjectKind::GO),
            ("package.json", ProjectKind::NODE),
            ("CMakeLists.txt", ProjectKind::CPP),
            ("main.tcl", ProjectKind::TCL),
            ("mib/ccf.dat", ProjectKind::OTHER),
            ("pcf.dat", ProjectKind::OTHER),
            ("vdf.dat", ProjectKind::OTHER),
            (".projectile", ProjectKind::OTHER),
        ];
        for (file, kind) in cases {
            let dir = TempDir::new("kind");
            dir.write(file, "");
            assert_eq!(detect_kind(dir.path()), *kind, "{file}");
        }
    }

    /// An ops repository of Tcl scripts and its MIB: Tcl. The MIB names no
    /// kind any more.
    #[test]
    fn tcl_scripts_beside_a_mib_are_tcl() {
        let dir = TempDir::new("kind_tcl_mib");
        dir.write("main.tcl", "");
        dir.write("mib/ccf.dat", "");
        dir.write("vdf.dat", "");
        assert_eq!(detect_kind(dir.path()), ProjectKind::TCL);
    }

    #[test]
    fn a_sketch_is_arduino_even_with_other_manifests() {
        let dir = TempDir::new("kind_sketch");
        let sketch = dir.path().join("Blink");
        std::fs::create_dir_all(&sketch).unwrap();
        std::fs::write(sketch.join("Blink.ino"), "").unwrap();
        std::fs::write(sketch.join("CMakeLists.txt"), "").unwrap();
        assert_eq!(detect_kind(&sketch), ProjectKind::ARDUINO);
    }

    #[test]
    fn main_files_are_found_by_kind() {
        let dir = TempDir::new("main_file");
        dir.write("Lab/Lab.ino", "");
        assert_eq!(main_file(&dir.path().join("Lab"), ProjectKind::ARDUINO), Some(dir.path().join("Lab/Lab.ino")));
        dir.write("py/src/orbit/__init__.py", "");
        assert_eq!(main_file(&dir.path().join("py"), ProjectKind::PYTHON), Some(dir.path().join("py/src/orbit/__init__.py")));
        dir.write("rs/src/lib.rs", "");
        assert_eq!(main_file(&dir.path().join("rs"), ProjectKind::RUST), Some(dir.path().join("rs/src/lib.rs")));
        assert_eq!(main_file(&dir.path().join("rs"), ProjectKind::GO), None);
    }

    /// Each built-in's main files, in the order they were looked for
    /// before kinds were data: the first that exists wins.
    #[test]
    fn built_in_main_files_keep_their_order() {
        let dir = TempDir::new("main_order");
        let cases: &[(ProjectKind, &[&str])] = &[
            (ProjectKind::ARDUINO, &["Lab/Lab.ino", "Lab/Lab.pde", "Lab/src/Lab.cpp", "Lab/src/Lab.h"]),
            (ProjectKind::RUST, &["Lab/src/main.rs", "Lab/src/lib.rs"]),
            (ProjectKind::PYTHON, &["Lab/main.py", "Lab/app.py", "Lab/src/a/__init__.py"]),
            (ProjectKind::GO, &["Lab/main.go"]),
            (ProjectKind::NODE, &["Lab/src/index.ts", "Lab/src/index.js", "Lab/index.ts", "Lab/index.js"]),
            (ProjectKind::CPP, &["Lab/src/main.cpp", "Lab/main.cpp", "Lab/src/main.c", "Lab/main.c"]),
            (ProjectKind::TCL, &["Lab/main.tcl", "Lab/a.tcl"]),
        ];
        for (kind, files) in cases {
            let _ = std::fs::remove_dir_all(dir.path().join("Lab"));
            for file in files.iter() {
                dir.write(file, "");
            }
            for file in files.iter() {
                assert_eq!(main_file(&dir.path().join("Lab"), *kind), Some(dir.path().join(file)), "{kind:?}");
                std::fs::remove_file(dir.path().join(file)).unwrap();
            }
            assert_eq!(main_file(&dir.path().join("Lab"), *kind), None, "{kind:?}");
        }
        assert_eq!(main_file(dir.path(), ProjectKind::MONOREPO), None);
        assert_eq!(main_file(dir.path(), ProjectKind::OTHER), None);
    }

    #[test]
    fn a_declared_kind_wins_over_detection() {
        let dir = TempDir::new("kind_declared");
        dir.write("Cargo.toml", "");
        dir.write(".fenix/project.ini", "[monitor]\nkind = rust\n[project]\n kind = Monorepo \n");
        assert_eq!(detect_kind(dir.path()), ProjectKind::MONOREPO);
    }

    /// A MIB is no kind of its own since 1.6: a project that declares one,
    /// in `project.ini` or in `settings.toml`, loads as a project like any
    /// other -- not as an unknown kind, not as its files' kind.
    #[test]
    fn a_project_declared_a_mib_is_a_project_like_any_other() {
        let ini = TempDir::new("kind_mib_ini");
        ini.write("vdf.dat", "OPS\tops\n");
        ini.write("Cargo.toml", "");
        ini.write(".fenix/project.ini", "[project]\nkind = mib\n");
        assert_eq!(declared_kind(ini.path()), Some(ProjectKind::OTHER));
        assert_eq!(detect_kind(ini.path()), ProjectKind::OTHER);

        let toml = TempDir::new("kind_mib_toml");
        toml.write(".fenix/settings.toml", "[project]\nkind = \"mib\"\n");
        assert_eq!(declared_kind(toml.path()), Some(ProjectKind::OTHER));
        assert_eq!(main_file(toml.path(), detect_kind(toml.path())), None);
    }

    #[test]
    fn ids_and_aliases_round_trip() {
        for kind in ProjectKind::BUILT_IN {
            assert_eq!(ProjectKind::from_id(kind.id()), Some(kind));
            assert_eq!(ProjectKind::from_id(&kind.id().to_ascii_uppercase()), Some(kind), "built-ins in any case");
            assert!(kind.is_builtin() && kind.is_declared());
        }
        for (alias, kind) in [("py", ProjectKind::PYTHON), ("uv", ProjectKind::PYTHON), ("ino", ProjectKind::ARDUINO), ("sketch", ProjectKind::ARDUINO), ("rs", ProjectKind::RUST), ("c++", ProjectKind::CPP), ("C", ProjectKind::CPP), ("cmake", ProjectKind::CPP), ("js", ProjectKind::NODE), ("TS", ProjectKind::NODE), ("javascript", ProjectKind::NODE), ("typescript", ProjectKind::NODE), (" mib ", ProjectKind::OTHER)] {
            assert_eq!(ProjectKind::from_id(alias), Some(kind), "{alias}");
        }
        // Any well-formed id is a kind, the same handle each time.
        let java = ProjectKind::from_id("java").unwrap();
        assert_eq!(ProjectKind::from_id(" java "), Some(java));
        assert_eq!(java.id(), "java");
        assert!(!java.is_builtin() && !java.is_declared());
        assert_eq!(ProjectKind::from_id("nonsense").map(|k| k.id()), Some("nonsense"));
        // What isn't an id isn't a kind: case, characters, length.
        for bad in ["", "Java", "1java", "-java", "ja va", "ja.va", "java/x", "é", &"a".repeat(25)] {
            assert_eq!(ProjectKind::from_id(bad), None, "{bad:?}");
        }
        assert!(ProjectKind::from_id(&"a".repeat(24)).is_some());
    }

    /// The tags, names and colours Fenix's own have always had.
    #[test]
    fn built_ins_keep_their_tags_names_and_colours() {
        let tags: Vec<_> = ProjectKind::BUILT_IN.iter().map(|k| k.tag()).collect();
        assert_eq!(tags, ["PY", "INO", "RS", "TCL", "C++", "JS", "GO", "MONO", "DIR"]);
        let labels: Vec<_> = ProjectKind::BUILT_IN.iter().map(|k| k.label()).collect();
        assert_eq!(labels, ["Python", "Arduino", "Rust", "Tcl", "C/C++", "JavaScript", "Go", "Monorepo", "Project"]);
        let ids: Vec<_> = ProjectKind::BUILT_IN.iter().map(|k| k.id()).collect();
        assert_eq!(ids, ["python", "arduino", "rust", "tcl", "cpp", "node", "go", "monorepo", "other"]);
        assert_eq!(ProjectKind::ARDUINO.color(), KindColor::Insert);
        assert_eq!(ProjectKind::MONOREPO.color(), KindColor::Fg);
        assert_eq!(ProjectKind::OTHER.color(), KindColor::Muted);
        assert_eq!(ProjectKind::RUST.color(), KindColor::Syntax("string"));
        assert_eq!(ProjectKind::known(), ProjectKind::BUILT_IN.to_vec());
    }

    /// `kind = "java"` with nothing declaring Java: kept, by its id, muted.
    #[test]
    fn a_declared_kind_no_one_declares_is_kept_and_shown_by_its_id() {
        let dir = TempDir::new("kind_unknown");
        dir.write("pom.xml", "");
        dir.write(".fenix/settings.toml", "[project]\nkind = \"java\"\n");
        let kind = detect_kind(dir.path());
        assert_eq!(kind.id(), "java");
        assert_eq!((kind.tag(), kind.label(), kind.color()), ("JAVA", "java", KindColor::Muted));
        assert_eq!(kind.def(), None);
        assert!(!ProjectKind::known().contains(&kind));
        assert_eq!(main_file(dir.path(), kind), None);
        let long = ProjectKind::from_id("my-kind_2").unwrap();
        assert_eq!(long.tag(), "MY-K");
        // What isn't an id is ignored, as before: detection decides.
        dir.write(".fenix/settings.toml", "[project]\nkind = \"Not An Id\"\n");
        assert_eq!(detect_kind(dir.path()), ProjectKind::OTHER);
    }

    #[test]
    fn colour_roles_are_the_eight() {
        for role in KindColor::ROLES {
            assert_eq!(KindColor::from_role(role), Some(KindColor::Syntax(role)));
        }
        for bad in ["#cc7832", "Keyword", "muted", "fg", ""] {
            assert_eq!(KindColor::from_role(bad), None);
        }
    }
}
