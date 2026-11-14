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

/// How many ids one run interns at most, Fenix's included. Only Fenix's
/// ids and those switched-on extensions declare are ever interned --
/// never one a repository's settings or a template names -- so this is
/// reached only by installing thousands of different kinds in one run.
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

/// Fenix's own markers -- what it detects its kinds by, and what makes a
/// project's top -- by the kind that owns each (`None`: a root marker of
/// no kind). No extension's kind may name one (compared ignoring ASCII
/// case, as Windows and macOS compare names), except the kind that owns
/// it, where it adds nothing: a pack can never take Fenix's projects.
pub const FENIX_MARKERS: &[(&str, Option<ProjectKind>)] = &[
    ("library.properties", Some(ProjectKind::ARDUINO)),
    ("sketch.yaml", Some(ProjectKind::ARDUINO)),
    ("{folder}.ino", Some(ProjectKind::ARDUINO)),
    ("{folder}.pde", Some(ProjectKind::ARDUINO)),
    ("Cargo.toml", Some(ProjectKind::RUST)),
    ("pyproject.toml", Some(ProjectKind::PYTHON)),
    ("setup.py", Some(ProjectKind::PYTHON)),
    ("requirements.txt", Some(ProjectKind::PYTHON)),
    ("uv.lock", Some(ProjectKind::PYTHON)),
    ("go.mod", Some(ProjectKind::GO)),
    ("package.json", Some(ProjectKind::NODE)),
    ("CMakeLists.txt", Some(ProjectKind::CPP)),
    ("pkgIndex.tcl", Some(ProjectKind::TCL)),
    (".git", None),
    (".projectile", None),
];

/// Which of Fenix's markers `marker` is, ignoring ASCII case: `Some(owner)`.
pub fn fenix_marker(marker: &str) -> Option<Option<ProjectKind>> {
    FENIX_MARKERS.iter().find(|(m, _)| m.eq_ignore_ascii_case(marker)).map(|(_, owner)| *owner)
}

/// Fenix's kinds' tags, which no extension's kind may take.
pub const FENIX_TAGS: [&str; 9] = ["PY", "INO", "RS", "TCL", "C++", "JS", "GO", "MONO", "DIR"];

/// Fenix's kinds' names, which no extension's kind may take (ignoring
/// case and the spaces around it).
pub const FENIX_NAMES: [&str; 9] = ["Python", "Arduino", "Rust", "Tcl", "C/C++", "JavaScript", "Go", "Monorepo", "Project"];

/// Names Windows keeps for devices, with or without an extension.
pub const DEVICE_NAMES: [&str; 22] = ["CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9"];

/// The format characters (Unicode's `Cf`, as of Unicode 16): zero-width
/// spaces and joiners, bidirectional controls, the byte-order mark and
/// the rest that draw nothing or reorder what's around them.
pub const FORMAT_CHARACTERS: &[(char, char)] = &[
    ('\u{00AD}', '\u{00AD}'),
    ('\u{0600}', '\u{0605}'),
    ('\u{061C}', '\u{061C}'),
    ('\u{06DD}', '\u{06DD}'),
    ('\u{070F}', '\u{070F}'),
    ('\u{0890}', '\u{0891}'),
    ('\u{08E2}', '\u{08E2}'),
    ('\u{180E}', '\u{180E}'),
    ('\u{200B}', '\u{200F}'),
    ('\u{202A}', '\u{202E}'),
    ('\u{2060}', '\u{2064}'),
    ('\u{2066}', '\u{206F}'),
    ('\u{FEFF}', '\u{FEFF}'),
    ('\u{FFF9}', '\u{FFFB}'),
    ('\u{110BD}', '\u{110BD}'),
    ('\u{110CD}', '\u{110CD}'),
    ('\u{13430}', '\u{1343F}'),
    ('\u{1BCA0}', '\u{1BCA3}'),
    ('\u{1D173}', '\u{1D17A}'),
    ('\u{E0001}', '\u{E0001}'),
    ('\u{E0020}', '\u{E007F}'),
];

/// A character that draws nothing a reader can see, or moves what's
/// around it: a control character (`Cc`), a format character (`Cf`), or
/// the line and paragraph separators.
pub fn is_invisible(c: char) -> bool {
    c.is_control() || matches!(c, '\u{2028}' | '\u{2029}') || FORMAT_CHARACTERS.iter().any(|(a, b)| (*a..=*b).contains(&c))
}

/// Whether `name` may name a kind as shown: none of it invisible, at
/// least one character neither whitespace nor invisible.
pub fn name_is_shown(name: &str) -> bool {
    !name.chars().any(is_invisible) && name.chars().any(|c| !c.is_whitespace())
}

/// What no file name a kind gives may be, on any system: one with `:`
/// (on Windows `C:x` is another drive's), a device's name, one ending in
/// `.` or a space (Windows drops both, so it names another file).
fn windows_trouble(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or(name).trim_end();
    name.contains(':') || name.ends_with(['.', ' ']) || DEVICE_NAMES.iter().any(|d| d.eq_ignore_ascii_case(stem))
}

/// Whether `marker` is a file name a kind may be marked by: 1 to 64
/// bytes, no path, wildcard, `:` or control character, `{folder}` at most
/// once and no other brace, not Fenix's own settings folder, a device or
/// a name ending in `.` or a space. The manifest's check says why one
/// isn't; this keeps the rule whoever gives the kinds.
pub fn marker_is_a_name(marker: &str) -> bool {
    !marker.is_empty()
        && marker.len() <= MAX_MARKER
        && !marker.contains("..")
        && !marker.chars().any(|c| matches!(c, '/' | '\\' | '*' | '?' | '[' | ']') || c.is_control())
        && !marker.eq_ignore_ascii_case(".fenix")
        && !marker.replacen("{folder}", "", 1).contains(['{', '}'])
        && !windows_trouble(marker)
}

/// Whether `path` is a path below a project a kind may be entered
/// through: 1 to 128 bytes, `/` between parts, none empty, `..`, with a
/// `:`, a device's name or ending in `.` or a space.
pub fn main_is_below(path: &str) -> bool {
    !path.is_empty() && path.len() <= MAX_MAIN_PATH && !path.contains('\\') && !path.split('/').any(|part| part.is_empty() || part == ".." || windows_trouble(part))
}

/// Settles the extensions' kinds, in the order they're detected in --
/// the machine's extensions first, then yours, each by id, and each one's
/// in the order it lists them. A Fenix id (`node`) adds its markers to
/// Fenix's kind and nothing else; an id declared again is the first's; a
/// marker of Fenix's (`FENIX_MARKERS`), a tag or name of Fenix's, past
/// `MAX_EXTENSION_KINDS` kinds or `MAX_EXTENSION_MARKERS` markers: left
/// out. What's left out is said (`extension_kind_notes`). Returns whether
/// anything changed: the same kinds again change nothing.
pub fn set_extension_kinds(kinds: Vec<KindDef>) -> bool {
    let mut registry = REGISTRY.write().unwrap_or_else(|e| e.into_inner());
    if registry.given == kinds {
        return false;
    }
    let mut next = Registry { declared: Vec::new(), added: [const { Vec::new() }; 9], markers: Vec::new(), notes: Vec::new(), given: kinds.clone() };
    for def in kinds {
        let extension = def.extension.clone().unwrap_or_default();
        let mut note = |text: String| next.notes.push((extension.clone(), text));
        let built_in = BUILT_IN[..7].iter().position(|b| b.id == def.id).map(|i| ProjectKind(i as u16));
        if built_in.is_none() && (!ProjectKind::is_well_formed(&def.id) || BUILT_IN.iter().any(|b| b.id == def.id) || ALIASES.iter().any(|(a, _)| *a == def.id)) {
            note(format!("its kind {:?} isn't one an extension may declare: left out", def.id));
            continue;
        }
        if built_in.is_none() {
            if let Some(owner) = next.declared.iter().find(|d| d.kind.id() == def.id).map(|d| d.extension.clone()) {
                note(format!("its kind {} is {owner}'s", def.id));
                continue;
            }
            if next.declared.len() >= MAX_EXTENSION_KINDS {
                note(format!("its kind {} is past the {MAX_EXTENSION_KINDS} kinds Fenix takes from extensions: left out", def.id));
                continue;
            }
            if FENIX_TAGS.contains(&def.tag.as_str()) || FENIX_NAMES.iter().any(|n| n.eq_ignore_ascii_case(def.name.trim())) {
                note(format!("its kind {} is named or tagged as one of Fenix's own: left out", def.id));
                continue;
            }
            if !name_is_shown(&def.name) {
                note(format!("its kind {}'s name has nothing to show, or what can't be seen: left out", def.id));
                continue;
            }
        }
        let mut markers: Vec<String> = Vec::new();
        let mut over = 0;
        for marker in def.markers.iter().filter(|m| marker_is_a_name(m)).take(MAX_MARKERS) {
            if markers.contains(marker) {
                continue;
            }
            match fenix_marker(marker) {
                Some(owner) if built_in.is_some() && owner == built_in => continue,
                Some(_) => {
                    note(format!("marker {marker} is Fenix's own: left out of its kind {}", def.id));
                    continue;
                }
                None => {}
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
        if let Some(kind) = built_in {
            let added = &mut next.added[kind.0 as usize];
            added.extend(markers.into_iter().filter(|m| !added.contains(m)).collect::<Vec<_>>());
            continue;
        }
        let Some(kind) = ProjectKind::intern(&def.id) else {
            note(format!("Fenix has read too many kind ids this run, so its kind {} is left out: start Fenix again to read it", def.id));
            continue;
        };
        let tag: String = def.tag.chars().filter(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || matches!(c, '+' | '#' | '.')).take(4).collect();
        next.declared.push(Declared {
            kind,
            name: keep(def.name.trim()),
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

    /// A built-in id or alias, in any case; else an id an extension has
    /// declared this run, as written. `None` for anything else: an id a
    /// repository's settings or a template names is never interned
    /// (`declared_id` keeps it as text).
    pub fn from_id(id: &str) -> Option<Self> {
        let id = id.trim();
        let lower = id.to_ascii_lowercase();
        if let Some(i) = BUILT_IN.iter().position(|b| b.id == lower) {
            return Some(Self(i as u16));
        }
        if let Some((_, kind)) = ALIASES.iter().find(|(alias, _)| *alias == lower) {
            return Some(*kind);
        }
        INTERNER.read().unwrap_or_else(|e| e.into_inner()).by_id.get(id).map(|&i| Self(i))
    }

    /// `id`'s handle, interned if it's new: only for what extensions
    /// declare (`set_extension_kinds`).
    fn intern(id: &str) -> Option<Self> {
        if !Self::is_well_formed(id) {
            return None;
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

    /// `id` as a kind's id: a built-in's or an alias's own id, in any
    /// case; else the id as written if it's well-formed. What a project
    /// or a template declares, whether or not anyone knows it.
    pub fn normalize_id(id: &str) -> Option<String> {
        if let Some(kind) = Self::from_id(id).filter(|k| k.is_builtin()) {
            return Some(kind.id().to_string());
        }
        Some(id.trim()).filter(|id| Self::is_well_formed(id)).map(str::to_string)
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
            let found = below(self.root, marker, &self.folder).is_some_and(|p| p.is_file());
            self.seen.push((marker, found));
            found
        })
    }
}

/// What `root`'s files say it is, whatever its settings declare -- what
/// the settings page offers as "detect". Fenix's own first, exactly as
/// they always were: Arduino (a sketch, `library.properties`), Rust,
/// Python, Go, JavaScript, C/C++, Tcl. Only a folder none of those name
/// -- Other -- is looked at for more: the markers extensions add to
/// Fenix's kinds, in those kinds' order, then the extensions' own kinds,
/// in theirs. So a pack never changes the kind of a project Fenix
/// recognises, and a project of Fenix's costs what it did. Nothing is
/// read but names.
pub fn detect_kind_from_files(root: &Path) -> ProjectKind {
    if is_remote(root) {
        return ProjectKind::OTHER;
    }
    let has = |name: &str| {
        looked();
        root.join(name).exists()
    };
    // A sketch: `sketch.yaml`, `{folder}.ino`, `{folder}.pde`.
    (0..3).for_each(|_| looked());
    looked();
    if crate::root::is_sketch(root) || root.join("library.properties").is_file() {
        return ProjectKind::ARDUINO;
    }
    if has("Cargo.toml") {
        return ProjectKind::RUST;
    }
    if has("pyproject.toml") || has("setup.py") || has("requirements.txt") || has("uv.lock") {
        return ProjectKind::PYTHON;
    }
    if has("go.mod") {
        return ProjectKind::GO;
    }
    if has("package.json") {
        return ProjectKind::NODE;
    }
    if has("CMakeLists.txt") {
        return ProjectKind::CPP;
    }
    if has("pkgIndex.tcl") || {
        looked();
        has_extension(root, "tcl")
    } {
        return ProjectKind::TCL;
    }
    let registry = registry();
    if registry.markers.is_empty() {
        return ProjectKind::OTHER;
    }
    let mut looks = Looks { root, folder: root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), seen: Vec::new() };
    for kind in [ProjectKind::ARDUINO, ProjectKind::RUST, ProjectKind::PYTHON, ProjectKind::GO, ProjectKind::NODE, ProjectKind::CPP, ProjectKind::TCL] {
        if looks.any(&registry.added[kind.0 as usize]) {
            return kind;
        }
    }
    registry.declared.iter().find(|d| looks.any(&d.markers)).map_or(ProjectKind::OTHER, |d| d.kind)
}

/// `root` joined with `path`, a path below it with `/` between its parts
/// and `{folder}` for the folder's own name -- `None` unless every part is
/// a plain name (`Component::Normal`), so nothing absolute, on another
/// drive (`C:x`), prefixed or `..` can replace or leave `root`, on any
/// system.
fn below(root: &Path, path: &str, folder: &str) -> Option<PathBuf> {
    let mut at = root.to_path_buf();
    for part in path.split('/') {
        if part.contains(':') {
            return None;
        }
        let part = part.replace("{folder}", folder);
        let mut components = Path::new(&part).components();
        match (components.next(), components.next()) {
            (Some(std::path::Component::Normal(name)), None) => at.push(name),
            _ => return None,
        }
    }
    Some(at)
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
    let def = kind.def();
    let mut candidates: Vec<PathBuf> = def.as_ref().map(|d| d.main.iter().filter_map(|m| below(root, m, &name)).collect()).unwrap_or_default();
    // An extension's main file is opened only if it's in the project,
    // links followed: a repository's `config.ru -> ~/.ssh/...` is not.
    if def.is_some_and(|d| d.extension.is_some()) {
        let inside = std::fs::canonicalize(root).ok();
        candidates.retain(|c| inside.as_ref().is_some_and(|r| std::fs::canonicalize(c).is_ok_and(|c| c.starts_with(r))));
    }
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
/// not opened since -- as written, if it's an id (`normalize_id`), known
/// or not: what the settings page shows as declared.
pub fn declared_id(root: &Path) -> Option<String> {
    if is_remote(root) {
        return None;
    }
    let value = fenix_storage::project_file::get(root, "project.kind").or_else(|| {
        let text = std::fs::read_to_string(root.join(".fenix").join("project.ini")).ok()?;
        ini_value(&text, "project", "kind")
    })?;
    ProjectKind::normalize_id(&value)
}

/// The kind the project declares, if it's one someone declares now:
/// Fenix, or a switched-on extension. One no one declares (`kind =
/// "java"` without the Java pack, a misspelt `pyhton`) is ignored, as
/// it always was -- detection decides -- and kept on disk as written,
/// to count again once a pack declaring it is installed.
pub fn declared_kind(root: &Path) -> Option<ProjectKind> {
    ProjectKind::from_id(&declared_id(root)?).filter(|k| k.is_declared())
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
        // An id no extension declared is no kind, as before: nothing a
        // repository or a template says is interned.
        assert_eq!(ProjectKind::from_id("nonsense"), None);
        assert_eq!(ProjectKind::from_id("cobol"), None);
        // ... but it's an id, kept as written.
        assert_eq!(ProjectKind::normalize_id(" cobol ").as_deref(), Some("cobol"));
        assert_eq!(ProjectKind::normalize_id("TS").as_deref(), Some("node"));
        for bad in ["", "Java", "1java", "-java", "ja va", "ja.va", "java/x", "é", &"a".repeat(25)] {
            assert_eq!(ProjectKind::from_id(bad), None, "{bad:?}");
            assert_eq!(ProjectKind::normalize_id(bad), None, "{bad:?}");
        }
        assert!(ProjectKind::normalize_id(&"a".repeat(24)).is_some());
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

    /// `kind = "java"` with nothing declaring Java, or a misspelt
    /// `pyhton`: kept on disk, ignored for what the project is -- its
    /// files decide, its tag, main file and checks theirs, as before.
    #[test]
    fn a_declared_kind_no_one_declares_is_kept_and_ignored() {
        let _kinds = crate::test_util::kinds_lock();
        let dir = TempDir::new("kind_unknown");
        dir.write("pom.xml", "");
        dir.write(".fenix/settings.toml", "[project]\nkind = \"java\"\n");
        assert_eq!(declared_id(dir.path()).as_deref(), Some("java"));
        assert_eq!(declared_kind(dir.path()), None);
        assert_eq!(detect_kind(dir.path()), ProjectKind::OTHER);
        dir.write("pyproject.toml", "");
        dir.write("main.py", "");
        dir.write(".fenix/settings.toml", "[project]\nkind = \"pyhton\"\n");
        assert_eq!(detect_kind(dir.path()), ProjectKind::PYTHON, "a misspelling is ignored, as on master");
        assert_eq!(main_file(dir.path(), detect_kind(dir.path())), Some(dir.path().join("main.py")));
        assert_eq!(declared_id(dir.path()).as_deref(), Some("pyhton"));
        // What isn't an id is ignored, and isn't kept either.
        dir.write(".fenix/settings.toml", "[project]\nkind = \"Not An Id\"\n");
        assert_eq!((declared_id(dir.path()), detect_kind(dir.path())), (None, ProjectKind::PYTHON));
        // Declared by a pack once it's installed: then it counts.
        dir.write(".fenix/settings.toml", "[project]\nkind = \"pyhton\"\n");
        set_extension_kinds(vec![kind("pyhton", "acme.pyhton", &[])]);
        assert_eq!(detect_kind(dir.path()).id(), "pyhton");
        set_extension_kinds(Vec::new());
        assert_eq!(detect_kind(dir.path()), ProjectKind::PYTHON);
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

    /// §2.4's order: declared; Fenix's own, exactly as on master
    /// (Arduino, Rust, Python, Go, JavaScript, C/C++, Tcl); only then the
    /// markers packs add to Fenix's kinds, in those kinds' order; then the
    /// extensions' kinds, in theirs; else Other. A pack never changes the
    /// kind of a project Fenix recognises.
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
        // A Rails app stays JavaScript, as it is without the pack (it can
        // declare kind = "ruby"); a React Native app with a Gemfile too.
        assert_eq!(detected(&["Gemfile", "package.json", "config.ru"]), ProjectKind::NODE);
        assert_eq!(detected(&["composer.json", "package.json", "artisan"]), ProjectKind::NODE);
        assert_eq!(detected(&["Gemfile", "CMakeLists.txt"]), ProjectKind::CPP);
        assert_eq!(detected(&["Gemfile", "main.tcl"]), ProjectKind::TCL);
        // What would be Other is the pack's.
        assert_eq!(detected(&["Gemfile"]), ruby);
        assert_eq!(detected(&["Gemfile", ".projectile"]), ruby);
        // Within the extensions' tier, theirs is the order given.
        assert_eq!(detected(&["Gemfile", "composer.json"]), php);
        assert_eq!(detected(&["package.json", "CMakeLists.txt"]), ProjectKind::NODE);
        assert_eq!(detected(&["CMakeLists.txt", "main.tcl"]), ProjectKind::CPP);
        assert_eq!(detected(&["main.tcl"]), ProjectKind::TCL);
        assert_eq!(detected(&[".projectile"]), ProjectKind::OTHER);
        // A Fenix id's added markers count only where Fenix's own name
        // nothing, in that kind's place, before the extensions' kinds.
        assert_eq!(detected(&["rust-toolchain.toml", "Gemfile"]), ProjectKind::RUST);
        assert_eq!(detected(&["deno.json"]), ProjectKind::NODE);
        assert_eq!(detected(&["deno.json", "Gemfile"]), ProjectKind::NODE);
        assert_eq!(detected(&["deno.json", "CMakeLists.txt"]), ProjectKind::CPP, "C/C++'s own marker beats JavaScript's added one");
        assert_eq!(detected(&["deno.json", "rust-toolchain.toml"]), ProjectKind::RUST, "added markers in Fenix's order");
        // A folder of the name a marker has isn't a marker.
        assert_eq!(detected(&["Gemfile/x"]), ProjectKind::OTHER);
        set_extension_kinds(Vec::new());
        assert_eq!(detected(&["Gemfile", "package.json"]), ProjectKind::NODE, "without the pack, as before");
        assert_eq!(detected(&["Gemfile"]), ProjectKind::OTHER);
    }

    /// M1: no pack's kind may name a marker of Fenix's (ignoring case),
    /// whether its own kind's or one it adds to another of Fenix's: the
    /// marker is left out, and said. The kind that owns a marker may name
    /// it, adding nothing.
    #[test]
    fn a_pack_never_takes_fenixs_markers() {
        let _kinds = crate::test_util::kinds_lock();
        let added = |id: &str, markers: &[&str]| KindDef { name: String::new(), tag: String::new(), ..kind(id, "acme.x", markers) };
        set_extension_kinds(vec![
            added("arduino", &["Cargo.toml"]),
            added("rust", &["package.json", "go.mod", "Cargo.toml", "x.rs-marker"]),
            kind("deno2", "acme.deno", &["package.json", "cmakelists.txt", ".git", ".PROJECTILE", "{FOLDER}.ino", "deno2.json"]),
        ]);
        assert_eq!(detected(&["Cargo.toml"]), ProjectKind::RUST);
        assert_eq!(detected(&["package.json"]), ProjectKind::NODE);
        assert_eq!(detected(&["go.mod"]), ProjectKind::GO);
        assert_eq!(detected(&["CMakeLists.txt"]), ProjectKind::CPP);
        assert_eq!(ProjectKind::RUST.def().unwrap().markers, ["Cargo.toml", "x.rs-marker"]);
        assert_eq!(ProjectKind::ARDUINO.def().unwrap().markers.len(), 4, "nothing added");
        assert_eq!(ProjectKind::from_id("deno2").unwrap().def().unwrap().markers, ["deno2.json"]);
        let notes = extension_kind_notes();
        for marker in ["Cargo.toml", "package.json", "go.mod", "cmakelists.txt", ".git", ".PROJECTILE"] {
            assert!(notes.iter().any(|(_, n)| n.contains(&format!("marker {marker} is Fenix's own"))), "{marker}: {notes:?}");
        }
        set_extension_kinds(Vec::new());
    }

    /// S1: a pack's kind can't pass for one of Fenix's -- its tag or
    /// name -- nor be named by what can't be seen.
    #[test]
    fn a_packs_kind_doesnt_look_like_fenixs() {
        let _kinds = crate::test_util::kinds_lock();
        let named = |id: &str, name: &str, tag: &str| KindDef { name: name.into(), tag: tag.into(), ..kind(id, "acme.x", &[]) };
        set_extension_kinds(vec![named("k1", "Rusty", "RS"), named("k2", " rust ", "RU"), named("k3", "Project", "PR"), named("k4", "\u{200B}", "ZW"), named("k5", "\u{202E}tsuR", "RL"), named("k6", "Fine", "FN")]);
        assert_eq!(ProjectKind::known().iter().filter(|k| !k.is_builtin()).map(|k| k.id()).collect::<Vec<_>>(), ["k6"]);
        assert_eq!(extension_kind_notes().len(), 5);
        assert!(name_is_shown("C and .NET") && !name_is_shown("  ") && !name_is_shown("a\u{FEFF}") && !name_is_shown("\u{2066}x"));
        set_extension_kinds(Vec::new());
    }

    /// M2: nothing a kind gives can name a file anywhere but the project:
    /// no `:` (`C:x` is another drive's on Windows), no device's name, no
    /// name ending in `.` or a space, and every part joined a plain name.
    #[test]
    fn a_kinds_names_stay_in_the_project() {
        for bad in ["C:evil", "a:b", "CON", "nul", "Aux.txt", "com1.json", "LPT9", "NUL.", "Gemfile.", "x ", "{folder}."] {
            assert!(!marker_is_a_name(bad), "marker {bad:?}");
        }
        for fine in ["CONFIG", "console.toml", "Gemfile", "com10", "a.b", "{folder}.sln"] {
            assert!(marker_is_a_name(fine), "marker {fine:?}");
        }
        for bad in ["C:/Windows/win.ini", "C:x", "src/C:x", "src/CON", "src/a.", "src/a ", "./x", "a//b"] {
            assert!(!main_is_below(bad), "main {bad:?}");
        }
        let root = Path::new("project");
        assert_eq!(below(root, "src/{folder}/Main.java", "app"), Some(root.join("src").join("app").join("Main.java")));
        for bad in ["C:x", "/etc/passwd", "..", "a/../b", "a/./b", "src/C:/x"] {
            assert_eq!(below(root, bad, "app"), None, "{bad:?}");
        }
        // Windows only, not checked here: `C:x` and `\\?\` prefixes are
        // what `Component::Normal` refuses there; on Linux `C:x` is a name,
        // refused by the `:` rule above.
    }

    /// An extension's main file is opened only inside the project, links
    /// followed; Fenix's own main files are as they were.
    #[cfg(unix)]
    #[test]
    fn an_extensions_main_file_doesnt_leave_the_project() {
        let _kinds = crate::test_util::kinds_lock();
        let dir = TempDir::new("main_link");
        let outside = dir.write("secret.txt", "");
        let project = dir.path().join("app");
        std::fs::create_dir_all(&project).unwrap();
        std::os::unix::fs::symlink(&outside, project.join("config.ru")).unwrap();
        set_extension_kinds(vec![KindDef { main: vec!["config.ru".into(), "inside.rb".into()], ..kind("ruby", "fenix.ruby", &["Gemfile"]) }]);
        let ruby = ProjectKind::from_id("ruby").unwrap();
        assert_eq!(main_file(&project, ruby), None);
        dir.write("app/inside.rb", "");
        assert_eq!(main_file(&project, ruby), Some(project.join("inside.rb")));
        set_extension_kinds(Vec::new());
        // A Rust project's src/main.rs linked elsewhere opens, as on master.
        std::fs::create_dir_all(project.join("src")).unwrap();
        std::os::unix::fs::symlink(&outside, project.join("src").join("main.rs")).unwrap();
        assert_eq!(main_file(&project, ProjectKind::RUST), Some(project.join("src").join("main.rs")));
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
        let bad = kind("bad", "acme.bad", &["../x", "a/b", "a\\\\b", "*.x", "x?", "[x]", ".fenix", ".FENIX", "{folder}{folder}", "{x}", "", "a\u{7}", &"m".repeat(65), "C:x", "CON", "x.", "ok", "ok"]);
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

    /// The store's packs' declarations (fenix-extensions `kinds/declare`),
    /// as their manifests have them.
    const STORE: &[(&str, &str, &[&str])] = &[
        ("fenix.ada", "ada", &["alire.toml", "{folder}.gpr"]),
        ("fenix.cpp", "cpp", &["CMakePresets.json", "compile_commands.json", "vcpkg.json", "conanfile.txt", "conanfile.py", ".clangd"]),
        ("fenix.dart", "dart", &["pubspec.yaml"]),
        ("fenix.dart", "flutter", &[]),
        ("fenix.dotnet", "dotnet", &["{folder}.sln", "{folder}.slnx", "{folder}.csproj", "global.json", "Directory.Build.props"]),
        ("fenix.elixir", "elixir", &["mix.exs"]),
        ("fenix.haskell", "haskell", &["cabal.project", "stack.yaml", "{folder}.cabal", "package.yaml"]),
        ("fenix.java", "java", &["pom.xml", "build.gradle", "build.gradle.kts", "settings.gradle", "settings.gradle.kts"]),
        ("fenix.java", "kotlin", &[]),
        ("fenix.lua", "lua", &[".luarc.json", ".luarc.jsonc", "selene.toml", "stylua.toml", ".stylua.toml", ".luacheckrc", ".busted", "conf.lua"]),
        ("fenix.ocaml", "ocaml", &["dune-project", "dune-workspace", "{folder}.opam"]),
        ("fenix.php", "php", &["composer.json", "artisan"]),
        ("fenix.ruby", "ruby", &["Gemfile", "gems.rb", ".ruby-version", "config.ru"]),
        ("fenix.swift", "swift", &["Package.swift"]),
        ("fenix.terraform", "terraform", &["main.tf", "versions.tf", "providers.tf", ".terraform.lock.hcl"]),
        ("fenix.zig", "zig", &["build.zig", "build.zig.zon"]),
    ];

    /// What detection costs, in looks at the file system and in time, by
    /// the kind of folder: with no extension, with the store's packs, and
    /// with 30 packs of 8 kinds of 16 markers (capped at 256). Fenix's own
    /// folders cost the same in all three.
    /// `cargo test -p fenix-project -- --ignored --nocapture measure`
    #[test]
    #[ignore]
    fn measure_detection() {
        let _kinds = crate::test_util::kinds_lock();
        let dir = TempDir::new("measure");
        let folders: &[(&str, &[&str])] = &[("Rust", &["Cargo.toml"]), ("Python", &["pyproject.toml"]), ("Node", &["package.json"]), ("C/C++", &["CMakeLists.txt"]), ("Tcl", &["main.tcl"]), ("Ruby (a pack's)", &["Gemfile"]), ("Other", &[".projectile"])];
        let store: Vec<KindDef> = STORE.iter().map(|(e, id, m)| KindDef { name: format!("{id} kind"), tag: "X".into(), ..kind(id, e, m) }).collect();
        let thirty: Vec<KindDef> = (0..30)
            .flat_map(|e| (0..8).map(move |k| KindDef { markers: (0..16).map(|m| format!("e{e}k{k}m{m}.x")).collect(), ..kind(&format!("e{e}-k{k}"), &format!("acme.e{e:02}"), &[]) }))
            .collect();
        let mut rows = Vec::new();
        for (label, kinds) in [("none", Vec::new()), ("store's 14 packs", store), ("30 packs, 256 markers", thirty)] {
            set_extension_kinds(kinds);
            let mut row = format!("| {label} |");
            for (name, files) in folders {
                let folder = dir.path().join(name.replace(['/', ' ', '(', ')', '\''], "_"));
                std::fs::create_dir_all(&folder).unwrap();
                for f in files.iter() {
                    std::fs::write(folder.join(f), "").unwrap();
                }
                LOOKS.with(|n| n.set(0));
                detect_kind_from_files(&folder);
                let looks = LOOKS.with(|n| n.get());
                let start = std::time::Instant::now();
                for _ in 0..200 {
                    detect_kind_from_files(&folder);
                }
                row += &format!(" {looks} / {:.1} µs |", start.elapsed().as_secs_f64() * 1e6 / 200.0);
            }
            rows.push(row);
        }
        println!("| extensions | {} |", folders.iter().map(|(n, _)| *n).collect::<Vec<_>>().join(" | "));
        for row in rows {
            println!("{row}");
        }
        set_extension_kinds(Vec::new());
    }
}
