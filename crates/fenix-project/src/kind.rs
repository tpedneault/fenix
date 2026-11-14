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

/// How many kinds the extensions may declare in all, and how many
/// distinct markers among them: a pack has at most 8 kinds of 16 markers
/// (`[[contributes.project_kinds]]`), and this bounds what any number of
/// packs make Fenix look for at each folder.
pub const MAX_EXTENSION_KINDS: usize = 256;
pub const MAX_EXTENSION_MARKERS: usize = 256;
const MAX_MARKERS: usize = 16;
const MAX_MARKER: usize = 64;
const MAX_MAIN: usize = 8;
const MAX_MAIN_PATH: usize = 128;

/// A kind an extension declares, as it's kept.
struct Declared {
    kind: ProjectKind,
    name: &'static str,
    tag: &'static str,
    color: KindColor,
    markers: Vec<String>,
    main: Vec<String>,
    extension: String,
}

/// The extensions' kinds (`set_extension_kinds`).
struct Registry {
    /// In the order they're detected in: the extensions' order, then each
    /// one's own. One per id, the first's.
    declared: Vec<Declared>,
    /// The markers extensions add to Fenix's own kinds, by the built-in's
    /// index: they join its tier.
    added: [Vec<String>; 9],
    /// Every distinct marker of the two: what's looked for at a folder, at
    /// most `MAX_EXTENSION_MARKERS`.
    markers: Vec<String>,
    /// What was left out and why, by extension.
    notes: Vec<(String, String)>,
    /// What it was set from, to tell a change from the same again.
    given: Vec<KindDef>,
}

static REGISTRY: RwLock<Registry> = RwLock::new(Registry { declared: Vec::new(), added: [const { Vec::new() }; 9], markers: Vec::new(), notes: Vec::new(), given: Vec::new() });

fn registry() -> std::sync::RwLockReadGuard<'static, Registry> {
    REGISTRY.read().unwrap_or_else(|e| e.into_inner())
}

/// Whether `marker` is a file name a kind may be marked by: 1 to 64
/// bytes, no path, wildcard or control character, `{folder}` at most once
/// and no other brace, not Fenix's own settings folder. The manifest's
/// check says why one isn't; this keeps the rule whoever gives the kinds.
pub fn marker_is_a_name(marker: &str) -> bool {
    !marker.is_empty()
        && marker.len() <= MAX_MARKER
        && !marker.contains("..")
        && !marker.chars().any(|c| matches!(c, '/' | '\\' | '*' | '?' | '[' | ']') || c.is_control())
        && !marker.eq_ignore_ascii_case(".fenix")
        && !marker.replacen("{folder}", "", 1).contains(['{', '}'])
}

/// Whether `path` is a path below a project a kind may be entered
/// through: 1 to 128 bytes, `/` between parts, none empty or `..`.
pub fn main_is_below(path: &str) -> bool {
    !path.is_empty() && path.len() <= MAX_MAIN_PATH && !path.contains('\\') && !path.split('/').any(|part| part.is_empty() || part == "..")
}

/// Settles the extensions' kinds, in the order they're detected in --
/// the machine's extensions first, then yours, each by id, and each one's
/// in the order it lists them. A Fenix id (`node`) adds its markers to
/// Fenix's kind and nothing else; an id declared again is the first's;
/// past `MAX_EXTENSION_KINDS` kinds or `MAX_EXTENSION_MARKERS` markers,
/// the rest are left out. What's left out is said
/// (`extension_kind_notes`). Returns whether anything changed: the same
/// kinds again change nothing.
pub fn set_extension_kinds(kinds: Vec<KindDef>) -> bool {
    let mut registry = REGISTRY.write().unwrap_or_else(|e| e.into_inner());
    if registry.given == kinds {
        return false;
    }
    let mut next = Registry { declared: Vec::new(), added: [const { Vec::new() }; 9], markers: Vec::new(), notes: Vec::new(), given: kinds.clone() };
    for def in kinds {
        let extension = def.extension.clone().unwrap_or_default();
        let mut note = |text: String| next.notes.push((extension.clone(), text));
        let kind = ProjectKind::from_id(&def.id).filter(|k| k.id() == def.id);
        let Some(kind) = kind.filter(|k| *k != ProjectKind::MONOREPO && *k != ProjectKind::OTHER) else {
            note(format!("its kind {:?} isn't one an extension may declare: left out", def.id));
            continue;
        };
        if !kind.is_builtin() && next.declared.iter().any(|d| d.kind == kind) {
            let owner = next.declared.iter().find(|d| d.kind == kind).map(|d| d.extension.clone()).unwrap_or_default();
            note(format!("its kind {} is {owner}'s", def.id));
            continue;
        }
        if !kind.is_builtin() && next.declared.len() >= MAX_EXTENSION_KINDS {
            note(format!("its kind {} is past the {MAX_EXTENSION_KINDS} kinds Fenix takes from extensions: left out", def.id));
            continue;
        }
        let mut markers: Vec<String> = Vec::new();
        let mut over = 0;
        for marker in def.markers.iter().filter(|m| marker_is_a_name(m)).take(MAX_MARKERS) {
            if markers.contains(marker) {
                continue;
            }
            if !next.markers.contains(marker) {
                if next.markers.len() >= MAX_EXTENSION_MARKERS {
                    over += 1;
                    continue;
                }
                next.markers.push(marker.clone());
            }
            markers.push(marker.clone());
        }
        if over > 0 {
            note(format!("{over} of its kind {}'s markers are past the {MAX_EXTENSION_MARKERS} Fenix looks for: left out", def.id));
        }
        if kind.is_builtin() {
            let added = &mut next.added[kind.0 as usize];
            added.extend(markers.into_iter().filter(|m| !added.contains(m)).collect::<Vec<_>>());
            continue;
        }
        let tag: String = def.tag.chars().filter(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || matches!(c, '+' | '#' | '.')).take(4).collect();
        next.declared.push(Declared {
            kind,
            name: keep(if def.name.trim().is_empty() { &def.id } else { &def.name }),
            tag: keep(if tag.is_empty() { &def.id } else { &tag }),
            color: def.color,
            markers,
            main: def.main.into_iter().filter(|m| main_is_below(m)).take(MAX_MAIN).collect(),
            extension: extension.clone(),
        });
    }
    *registry = next;
    true
}

/// What `set_extension_kinds` left out, and why: (extension, what).
pub fn extension_kind_notes() -> Vec<(String, String)> {
    registry().notes.clone()
}

/// Every marker the extensions' kinds are found by, Fenix's ids' added
/// ones included: a project's top where nothing of Fenix's is found
/// (`find_project_root`).
pub(crate) fn with_kind_markers<T>(f: impl FnOnce(&[String]) -> T) -> T {
    f(&registry().markers)
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
        let registry = registry();
        if let Some(b) = self.built_in() {
            return Some(KindDef {
                id: b.id.to_string(),
                name: b.label.to_string(),
                tag: b.tag.to_string(),
                color: b.color,
                markers: b.markers.iter().map(|m| m.to_string()).chain(registry.added[self.0 as usize].iter().cloned()).collect(),
                main: b.main.iter().map(|m| m.to_string()).collect(),
                extension: None,
            });
        }
        let d = registry.declared.iter().find(|d| d.kind == self)?;
        Some(KindDef { id: self.id().to_string(), name: d.name.to_string(), tag: d.tag.to_string(), color: d.color, markers: d.markers.clone(), main: d.main.clone(), extension: Some(d.extension.clone()) })
    }

    /// Whether anyone declares it now: Fenix, or an installed extension.
    pub fn is_declared(self) -> bool {
        self.is_builtin() || registry().declared.iter().any(|d| d.kind == self)
    }

    /// The extension that declares it now, if one does.
    pub fn extension(self) -> Option<String> {
        if self.is_builtin() {
            return None;
        }
        registry().declared.iter().find(|d| d.kind == self).map(|d| d.extension.clone())
    }

    /// The one-to-four-character tag every place a project is named shows
    /// in front of it -- the same everywhere, so it reads as one mark. For
    /// an id no one declares, the id, upper-cased, cut to four.
    pub fn tag(self) -> &'static str {
        if let Some(b) = self.built_in() {
            return b.tag;
        }
        if let Some(tag) = registry().declared.iter().find(|d| d.kind == self).map(|d| d.tag) {
            return tag;
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
        registry().declared.iter().find(|d| d.kind == self).map_or_else(|| self.id(), |d| d.name)
    }

    /// What its tag is drawn in.
    pub fn color(self) -> KindColor {
        if let Some(b) = self.built_in() {
            return b.color;
        }
        registry().declared.iter().find(|d| d.kind == self).map_or(KindColor::Muted, |d| d.color)
    }

    /// Every kind declared now: Fenix's in their order, then the
    /// extensions' by name. What the settings page and the hub cycle.
    pub fn known() -> Vec<Self> {
        let mut theirs: Vec<(&'static str, Self)> = registry().declared.iter().map(|d| (d.name, d.kind)).collect();
        theirs.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()).then_with(|| a.1.id().cmp(b.1.id())));
        Self::BUILT_IN.into_iter().chain(theirs.into_iter().map(|(_, k)| k)).collect()
    }
}

/// `root`'s kind: the one `.fenix/settings.toml` declares, else the first
/// detection that matches, most specific first -- a sketch can live
/// inside anything, so it's checked before the language manifests. A
/// remote root is Other: nothing of it is looked at from here.
pub fn detect_kind(root: &Path) -> ProjectKind {
    declared_kind(root).unwrap_or_else(|| detect_kind_from_files(root))
}

/// Whether `root` is on another machine: its kind is never looked for on
/// this one (no `stat` goes to the network on the UI thread).
fn is_remote(root: &Path) -> bool {
    fenix_vfs::path::is_remote_text(&root.to_string_lossy())
}

#[cfg(test)]
thread_local! {
    /// How many times detection asked the file system, on this thread.
    pub(crate) static LOOKS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// One question to the file system, counted in tests.
fn looked() {
    #[cfg(test)]
    LOOKS.with(|n| n.set(n.get() + 1));
}

/// A folder's markers, each looked for once however many kinds name it.
struct Looks<'a> {
    root: &'a Path,
    folder: String,
    seen: Vec<(&'a str, bool)>,
}

impl<'a> Looks<'a> {
    fn any(&mut self, markers: &'a [String]) -> bool {
        markers.iter().any(|marker| {
            if let Some((_, found)) = self.seen.iter().find(|(m, _)| *m == marker.as_str()) {
                return *found;
            }
            looked();
            let found = self.root.join(marker.replace("{folder}", &self.folder)).is_file();
            self.seen.push((marker, found));
            found
        })
    }
}

/// What `root`'s files say it is, whatever its settings declare -- what
/// the settings page offers as "detect". In this order: Arduino (a
/// sketch, `library.properties`); Rust, Python, Go; the extensions' kinds,
/// in theirs; JavaScript, C/C++; Tcl; else Other. The markers extensions
/// add to Fenix's own kinds count in that kind's place. Nothing is read
/// but names.
pub fn detect_kind_from_files(root: &Path) -> ProjectKind {
    if is_remote(root) {
        return ProjectKind::OTHER;
    }
    let registry = registry();
    let mut looks = Looks { root, folder: root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), seen: Vec::new() };
    let added = |kind: ProjectKind| &registry.added[kind.0 as usize][..];
    let has = |name: &str| {
        looked();
        root.join(name).exists()
    };
    let is_file = |name: &str| {
        looked();
        root.join(name).is_file()
    };
    // A sketch: `sketch.yaml`, `{folder}.ino`, `{folder}.pde`.
    (0..3).for_each(|_| looked());
    if crate::root::is_sketch(root) || is_file("library.properties") || looks.any(added(ProjectKind::ARDUINO)) {
        return ProjectKind::ARDUINO;
    }
    if has("Cargo.toml") || looks.any(added(ProjectKind::RUST)) {
        return ProjectKind::RUST;
    }
    if has("pyproject.toml") || has("setup.py") || has("requirements.txt") || has("uv.lock") || looks.any(added(ProjectKind::PYTHON)) {
        return ProjectKind::PYTHON;
    }
    if has("go.mod") || looks.any(added(ProjectKind::GO)) {
        return ProjectKind::GO;
    }
    if let Some(d) = registry.declared.iter().find(|d| looks.any(&d.markers)) {
        return d.kind;
    }
    if has("package.json") || looks.any(added(ProjectKind::NODE)) {
        return ProjectKind::NODE;
    }
    if has("CMakeLists.txt") || looks.any(added(ProjectKind::CPP)) {
        return ProjectKind::CPP;
    }
    if has("pkgIndex.tcl") || {
        looked();
        has_extension(root, "tcl")
    } || looks.any(added(ProjectKind::TCL))
    {
        return ProjectKind::TCL;
    }
    ProjectKind::OTHER
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
    if is_remote(root) {
        return None;
    }
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
    if is_remote(root) {
        return None;
    }
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
        let _kinds = crate::test_util::kinds_lock();
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
        let _kinds = crate::test_util::kinds_lock();
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

    fn kind(id: &str, extension: &str, markers: &[&str]) -> KindDef {
        KindDef {
            id: id.into(),
            name: id[..1].to_uppercase() + &id[1..],
            tag: id.to_ascii_uppercase().chars().take(4).collect(),
            color: KindColor::DEFAULT,
            markers: markers.iter().map(|m| m.to_string()).collect(),
            main: Vec::new(),
            extension: Some(extension.into()),
        }
    }

    /// The extensions' kinds used below: Ruby's and PHP's, Kotlin's (only
    /// ever declared), and markers added to Fenix's Rust and JavaScript.
    fn store_kinds() -> Vec<KindDef> {
        vec![
            kind("php", "fenix.php", &["composer.json", "artisan"]),
            kind("ruby", "fenix.ruby", &["Gemfile", "config.ru"]),
            kind("kotlin", "fenix.java", &[]),
            KindDef { name: String::new(), tag: String::new(), ..kind("rust", "fenix.rust", &["rust-toolchain.toml"]) },
            KindDef { name: String::new(), tag: String::new(), ..kind("node", "fenix.web", &["deno.json", "tsconfig.json"]) },
        ]
    }

    fn detected(files: &[&str]) -> ProjectKind {
        let dir = TempDir::new("tiers");
        for file in files {
            dir.write(file, "");
        }
        detect_kind(dir.path())
    }

    /// §2.4's order, each tier against the next: declared, Arduino, Rust,
    /// Python, Go, the extensions' kinds (in their order), Node, C/C++,
    /// Tcl, Other -- and a Fenix id's added markers in that id's tier.
    #[test]
    fn kinds_are_detected_in_their_tiers() {
        let _kinds = crate::test_util::kinds_lock();
        assert!(set_extension_kinds(store_kinds()));
        let ruby = ProjectKind::from_id("ruby").unwrap();
        let php = ProjectKind::from_id("php").unwrap();
        let declared = TempDir::new("tiers_declared");
        declared.write("Cargo.toml", "");
        declared.write(".fenix/settings.toml", "[project]\nkind = \"ruby\"\n");
        assert_eq!(detect_kind(declared.path()), ruby, "declared beats everything");
        assert_eq!(detected(&["library.properties", "Cargo.toml"]), ProjectKind::ARDUINO);
        assert_eq!(detected(&["library.properties", "Gemfile"]), ProjectKind::ARDUINO);
        assert_eq!(detected(&["Cargo.toml", "pyproject.toml", "Gemfile"]), ProjectKind::RUST);
        assert_eq!(detected(&["pyproject.toml", "go.mod", "Gemfile"]), ProjectKind::PYTHON);
        assert_eq!(detected(&["go.mod", "Gemfile"]), ProjectKind::GO);
        // A Rails app: Ruby with JavaScript in it.
        assert_eq!(detected(&["Gemfile", "package.json", "config.ru"]), ruby);
        assert_eq!(detected(&["composer.json", "package.json", "artisan"]), php, "Laravel");
        assert_eq!(detected(&["Gemfile", "CMakeLists.txt"]), ruby);
        assert_eq!(detected(&["Gemfile", "main.tcl"]), ruby);
        assert_eq!(detected(&["Gemfile"]), ruby);
        // Within the extensions' tier, theirs is the order given.
        assert_eq!(detected(&["Gemfile", "composer.json"]), php);
        assert_eq!(detected(&["package.json", "CMakeLists.txt"]), ProjectKind::NODE);
        assert_eq!(detected(&["CMakeLists.txt", "main.tcl"]), ProjectKind::CPP);
        assert_eq!(detected(&["main.tcl"]), ProjectKind::TCL);
        assert_eq!(detected(&[".projectile"]), ProjectKind::OTHER);
        // A Fenix id's added markers count in its own tier, not theirs.
        assert_eq!(detected(&["rust-toolchain.toml", "Gemfile"]), ProjectKind::RUST);
        assert_eq!(detected(&["deno.json"]), ProjectKind::NODE);
        assert_eq!(detected(&["deno.json", "Gemfile"]), ruby, "JavaScript's added marker is in JavaScript's tier");
        assert_eq!(detected(&["deno.json", "CMakeLists.txt"]), ProjectKind::NODE);
        // A folder of the name a marker has isn't a marker.
        assert_eq!(detected(&["Gemfile/x"]), ProjectKind::OTHER);
        set_extension_kinds(Vec::new());
        assert_eq!(detected(&["Gemfile", "package.json"]), ProjectKind::NODE, "without the pack, as before");
    }

    /// An extension's kind is named, tagged, coloured and listed as
    /// Fenix's own are; a Fenix id it declares stays Fenix's, with its
    /// markers added.
    #[test]
    fn an_extensions_kind_is_named_as_fenixs_are() {
        let _kinds = crate::test_util::kinds_lock();
        let mut java = kind("java2", "fenix.java", &["pom.xml"]);
        java.name = "Java".into();
        java.tag = "JAVA".into();
        java.color = KindColor::Syntax("keyword");
        java.main = vec!["src/{folder}/Main.java".into(), "app/Main.java".into()];
        let mut kinds = store_kinds();
        kinds.push(java);
        set_extension_kinds(kinds);
        let k = ProjectKind::from_id("java2").unwrap();
        assert!(k.is_declared() && !k.is_builtin());
        assert_eq!((k.tag(), k.label(), k.color()), ("JAVA", "Java", KindColor::Syntax("keyword")));
        assert_eq!(k.extension().as_deref(), Some("fenix.java"));
        assert_eq!(ProjectKind::from_id("kotlin").unwrap().color(), KindColor::DEFAULT);
        let known: Vec<&str> = ProjectKind::known().iter().map(|k| k.id()).collect();
        assert_eq!(known, ["python", "arduino", "rust", "tcl", "cpp", "node", "go", "monorepo", "other", "java2", "kotlin", "php", "ruby"], "Fenix's, then theirs by name");
        let node = ProjectKind::NODE.def().unwrap();
        assert_eq!((node.tag.as_str(), node.name.as_str(), node.color), ("JS", "JavaScript", KindColor::Syntax("number")));
        assert_eq!(node.markers, ["package.json", "deno.json", "tsconfig.json"]);
        assert_eq!(ProjectKind::NODE.extension(), None);
        // Its main files, {folder} its own name.
        let dir = TempDir::new("ext_main");
        dir.write("lab/app/Main.java", "");
        assert_eq!(main_file(&dir.path().join("lab"), k), Some(dir.path().join("lab").join("app").join("Main.java")));
        dir.write("lab/src/lab/Main.java", "");
        assert_eq!(main_file(&dir.path().join("lab"), k), Some(dir.path().join("lab").join("src").join("lab").join("Main.java")));
        // Switched off: the same handle, known by its id only.
        set_extension_kinds(Vec::new());
        assert!(!k.is_declared());
        assert_eq!((k.tag(), k.label(), k.color()), ("JAVA", "java2", KindColor::Muted));
        assert_eq!(main_file(&dir.path().join("lab"), k), None);
        assert_eq!(ProjectKind::known(), ProjectKind::BUILT_IN.to_vec());
    }

    /// The same id from two extensions is the first's, the other's said;
    /// what no extension may declare is left out and said; the same
    /// kinds again change nothing.
    #[test]
    fn the_first_declaration_of_an_id_is_the_kind() {
        let _kinds = crate::test_util::kinds_lock();
        let mut second = kind("ruby", "acme.ruby", &["Rakefile"]);
        second.tag = "RBY".into();
        let mut kinds = store_kinds();
        kinds.push(second);
        for reserved in ["other", "monorepo", "py", "mib", "Ruby", "c++"] {
            kinds.push(kind(reserved, "acme.bad", &["x"]));
        }
        assert!(set_extension_kinds(kinds.clone()));
        assert!(!set_extension_kinds(kinds), "the same again");
        let ruby = ProjectKind::from_id("ruby").unwrap();
        assert_eq!((ruby.tag(), ruby.extension().as_deref()), ("RUBY", Some("fenix.ruby")));
        assert_eq!(detected(&["Rakefile"]), ProjectKind::OTHER, "the second's markers aren't the kind's");
        let notes = extension_kind_notes();
        assert!(notes.contains(&("acme.ruby".to_string(), "its kind ruby is fenix.ruby's".to_string())), "{notes:?}");
        assert_eq!(notes.iter().filter(|(e, _)| e == "acme.bad").count(), 6, "{notes:?}");
        assert_eq!(ProjectKind::OTHER.tag(), "DIR");
        assert_eq!(ProjectKind::PYTHON.def().unwrap().markers, ["pyproject.toml", "setup.py", "requirements.txt", "uv.lock"]);
        set_extension_kinds(Vec::new());
        assert!(extension_kind_notes().is_empty());
    }

    /// Whatever extensions give, what's looked for at a folder is bounded:
    /// 16 markers a kind, 256 kinds, 256 distinct markers; a marker that's
    /// a path, a wildcard or `.fenix` is never looked for.
    #[test]
    fn what_extensions_make_fenix_look_for_is_bounded() {
        let _kinds = crate::test_util::kinds_lock();
        let many: Vec<KindDef> = (0..300)
            .map(|i| {
                let markers: Vec<String> = (0..20).map(|m| format!("k{i}-m{m}")).collect();
                KindDef { markers, ..kind(&format!("k{i}"), &format!("acme.e{}", i / 8), &[]) }
            })
            .collect();
        set_extension_kinds(many);
        assert_eq!(ProjectKind::known().len(), 9 + MAX_EXTENSION_KINDS);
        assert_eq!(with_kind_markers(|m| m.len()), MAX_EXTENSION_MARKERS);
        assert_eq!(ProjectKind::from_id("k0").unwrap().def().unwrap().markers.len(), 16);
        assert!(extension_kind_notes().iter().any(|(_, n)| n.contains("past the 256 Fenix looks for")));
        assert!(extension_kind_notes().iter().any(|(_, n)| n.contains("past the 256 kinds")));
        let looked_for = |dir: &Path| {
            LOOKS.with(|n| n.set(0));
            detect_kind_from_files(dir);
            LOOKS.with(|n| n.get())
        };
        let dir = TempDir::new("bounded");
        assert!(looked_for(dir.path()) <= 14 + MAX_EXTENSION_MARKERS, "{}", looked_for(dir.path()));
        let bad = kind("bad", "acme.bad", &["../x", "a/b", "a\\\\b", "*.x", "x?", "[x]", ".fenix", ".FENIX", "{folder}{folder}", "{x}", "", "a\u{7}", &"m".repeat(65), "ok", "ok"]);
        set_extension_kinds(vec![bad]);
        assert_eq!(ProjectKind::from_id("bad").unwrap().def().unwrap().markers, ["ok"]);
        set_extension_kinds(Vec::new());
        assert_eq!(looked_for(dir.path()), 14);
    }

    /// A remote root is never looked at from here: Other, nothing
    /// declared, no main file -- and not one question to the file system.
    #[test]
    fn a_remote_root_is_never_looked_at() {
        let _kinds = crate::test_util::kinds_lock();
        set_extension_kinds(store_kinds());
        let remote = fenix_vfs::path::remote("build-box", "/srv/app");
        LOOKS.with(|n| n.set(0));
        assert_eq!(detect_kind(&remote), ProjectKind::OTHER);
        assert_eq!(declared_kind(&remote), None);
        assert_eq!(main_file(&remote, ProjectKind::RUST), None);
        assert_eq!(LOOKS.with(|n| n.get()), 0);
        set_extension_kinds(Vec::new());
    }

    /// With no extensions, every one of Fenix's rules costs what it did:
    /// one look per marker, a folder listing for Tcl.
    #[test]
    fn with_no_extensions_detection_looks_as_it_did() {
        let _kinds = crate::test_util::kinds_lock();
        set_extension_kinds(Vec::new());
        let dir = TempDir::new("looks");
        LOOKS.with(|n| n.set(0));
        assert_eq!(detect_kind_from_files(dir.path()), ProjectKind::OTHER);
        // Sketch (3), library.properties, Cargo.toml, four of Python's,
        // go.mod, package.json, CMakeLists.txt, pkgIndex.tcl, the listing.
        assert_eq!(LOOKS.with(|n| n.get()), 14);
    }

    /// How long detection takes with 30 extensions' kinds registered (8
    /// kinds of 16 markers each, every one distinct: past the bound), for
    /// a hub of 50 projects and a file in no project.
    /// `cargo test -p fenix-project --release -- --ignored --nocapture measure`
    #[test]
    #[ignore]
    fn measure_detection_with_thirty_extensions() {
        let _kinds = crate::test_util::kinds_lock();
        let dir = TempDir::new("measure");
        let hub = dir.path().join("hub");
        let markers = ["Cargo.toml", "pyproject.toml", "package.json", "go.mod", "CMakeLists.txt", "main.tcl", "Gemfile", ".projectile", "pom.xml", "composer.json"];
        for i in 0..50 {
            let project = hub.join(format!("p{i:02}"));
            std::fs::create_dir_all(project.join("src")).unwrap();
            std::fs::write(project.join(markers[i % markers.len()]), "").unwrap();
            std::fs::write(project.join(".projectile"), "").unwrap();
        }
        let deep = dir.path().join("a/b/c/d/e/f/g/h");
        std::fs::create_dir_all(&deep).unwrap();
        let run = |label: &str| {
            LOOKS.with(|n| n.set(0));
            let start = std::time::Instant::now();
            let found = crate::workspace::subprojects(&hub);
            let listing = start.elapsed();
            let looks = LOOKS.with(|n| n.get());
            let start = std::time::Instant::now();
            for _ in 0..100 {
                crate::find_project_root(&deep.join("x.txt"));
            }
            let root = start.elapsed() / 100;
            let start = std::time::Instant::now();
            for (p, _) in &found {
                detect_kind(p);
            }
            let detect = start.elapsed() / found.len().max(1) as u32;
            println!("{label}: hub of {} projects listed (subprojects + kinds) in {listing:?}, {looks} looks by detection ({} a project); detect_kind {detect:?} a project; find_project_root for a file in no project 8 folders deep {root:?}", found.len(), looks / found.len().max(1));
        };
        set_extension_kinds(Vec::new());
        run("no extensions");
        let thirty: Vec<KindDef> = (0..30)
            .flat_map(|e| (0..8).map(move |k| KindDef { markers: (0..16).map(|m| format!("e{e}k{k}m{m}.x")).collect(), ..kind(&format!("e{e}-k{k}"), &format!("acme.e{e:02}"), &[]) }))
            .collect();
        set_extension_kinds(thirty);
        run("30 extensions x 8 kinds x 16 markers (capped at 256 markers)");
        set_extension_kinds(Vec::new());
    }
}
