use std::path::{Path, PathBuf};
use std::sync::RwLock;

/// Files/directories that mark a directory as a project root, checked in
/// no particular priority order -- any one of them is enough. `.git` and
/// `.projectile` are ecosystem-agnostic; the rest are the common
/// per-language "this is the top of a package" manifest.
const MARKERS: &[&str] = &[".git", ".projectile", "Cargo.toml", "package.json", "pyproject.toml", "go.mod", "CMakeLists.txt", "library.properties", "pkgIndex.tcl"];

/// An Arduino sketch folder: `sketch.yaml`, or a main `.ino`/`.pde` named
/// after the folder itself -- the one rule the Arduino tools hold every
/// sketch to. A sketch inside a larger repository is its own project, so
/// a class repo with one folder per lab gets one root per lab.
pub fn is_sketch(dir: &Path) -> bool {
    if dir.join("sketch.yaml").is_file() {
        return true;
    }
    let Some(name) = dir.file_name().and_then(|n| n.to_str()) else { return false };
    dir.join(format!("{name}.ino")).is_file() || dir.join(format!("{name}.pde")).is_file()
}

/// What the language packs say marks their projects (`[[contributes.
/// contexts]]` `markers`; `{folder}` is the folder's own name): a project's
/// top when nothing in `MARKERS` is found above a file -- a `dotnet new
/// console` folder, with its `{folder}.csproj` and no `.git`.
static EXTENSION_MARKERS: RwLock<Vec<String>> = RwLock::new(Vec::new());

/// Settles the language packs' markers (see `EXTENSION_MARKERS`).
pub fn set_extension_markers(markers: Vec<String>) {
    *EXTENSION_MARKERS.write().unwrap_or_else(|e| e.into_inner()) = markers;
}

/// Whether `dir` itself holds a language pack's marker
/// (`set_extension_markers`).
pub(crate) fn holds_pack_marker(dir: &Path) -> bool {
    let markers = EXTENSION_MARKERS.read().unwrap_or_else(|e| e.into_inner());
    crate::kind::with_kind_markers(|kinds| holds_one_of(dir, &markers, kinds))
}

/// Whether `dir` holds one of `contexts`' or `kinds`' markers (`{folder}`
/// is its own name). A context's marker that isn't a plain file name is
/// never looked for: it would leave `dir`.
fn holds_one_of(dir: &Path, contexts: &[String], kinds: &[String]) -> bool {
    let folder = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    contexts.iter().chain(kinds.iter().filter(|k| !contexts.contains(k))).any(|m| {
        let name = m.replace("{folder}", &folder);
        let mut parts = Path::new(&name).components();
        matches!((parts.next(), parts.next()), (Some(std::path::Component::Normal(_)), None)) && !m.contains(':') && dir.join(&name).is_file()
    })
}

/// Walks up from `start` (a file or a directory -- a file's own directory
/// is where the walk begins) looking for the closest ancestor containing
/// any marker in `MARKERS`, or that is an Arduino sketch. A folder of MIB
/// tables is no marker: it's in the project around it. Returns `None` if
/// none is found all the way to the filesystem root -- callers treat that
/// as "not in a known project," not an error. With none, the closest holding a language
/// pack's marker: its contexts' (`set_extension_markers`) or its project
/// kinds' (`set_extension_kinds`) -- never nearer than one of Fenix's, so
/// a kind's marker in a repository doesn't make a project of its own.
pub fn find_project_root(start: &Path) -> Option<PathBuf> {
    find_project_root_below(start, dirs::home_dir().as_deref())
}

/// Whether an extension's marker may make `dir` a project's top: never
/// the home folder (`home`), a folder above it, or a file system's root
/// -- a dotfile there (`~/.ruby-version`) would make every loose file
/// under it one project. Fenix's own markers aren't held to this.
fn pack_may_root(dir: &Path, home: Option<&Path>) -> bool {
    dir.parent().is_some() && !home.is_some_and(|home| home.starts_with(dir))
}

/// `find_project_root`, with `home` as the home folder.
fn find_project_root_below(start: &Path, home: Option<&Path>) -> Option<PathBuf> {
    let first = if start.is_dir() { start } else { start.parent()? };
    let found = first.ancestors().find(|dir| {
        dir.join(".fenix/tools.json").is_file()
            || dir.join(".fenix/settings.toml").is_file()
            || dir.join(".fenix/project.ini").is_file()
            || MARKERS.iter().any(|marker| dir.join(marker).exists())
            || is_sketch(dir)
    });
    if found.is_some() {
        return found.map(Path::to_path_buf);
    }
    let markers = EXTENSION_MARKERS.read().unwrap_or_else(|e| e.into_inner());
    crate::kind::with_kind_markers(|kinds| first.ancestors().find(|dir| pack_may_root(dir, home) && holds_one_of(dir, &markers, kinds)).map(Path::to_path_buf))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::TempDir;
    use std::fs;

    #[test]
    fn finds_a_git_marker_several_levels_up() {
        let dir = TempDir::new("git_marker");
        fs::create_dir(dir.path().join(".git")).unwrap();
        let nested = dir.path().join("src").join("deep");
        fs::create_dir_all(&nested).unwrap();

        assert_eq!(find_project_root(&nested), Some(dir.path().to_path_buf()));
    }

    #[test]
    fn an_arduino_sketch_inside_a_repository_is_its_own_root() {
        let dir = TempDir::new("sketch_marker");
        fs::create_dir_all(dir.path().join(".git")).unwrap();
        let sketch = dir.path().join("Lab1");
        fs::create_dir_all(sketch.join("src")).unwrap();
        fs::write(sketch.join("Lab1.ino"), b"void setup() {}").unwrap();
        assert_eq!(find_project_root(&sketch.join("src").join("pins.h")), Some(sketch.clone()));
        assert_eq!(find_project_root(&sketch.join("Lab1.ino")), Some(sketch));

        let stray = dir.path().join("notes");
        fs::create_dir_all(&stray).unwrap();
        fs::write(stray.join("scratch.ino"), b"").unwrap();
        assert_eq!(find_project_root(&stray.join("scratch.ino")), Some(dir.path().to_path_buf()), "an .ino not named after its folder isn't a sketch");
    }

    /// A folder of MIB tables is a folder like any other: in a
    /// repository it's the repository's, alone it's in no project, and
    /// only a `.fenix` of its own (the MIB template's) makes it one.
    #[test]
    fn a_folder_of_mib_tables_is_not_a_project_of_its_own() {
        let dir = TempDir::new("mib_marker");
        let mib = dir.path().join("ops").join("mib");
        fs::create_dir_all(&mib).unwrap();
        fs::write(mib.join("vdf.dat"), b"OPS\tops\n").unwrap();
        fs::write(mib.join("ccf.dat"), b"").unwrap();
        assert_eq!(find_project_root(&mib.join("ccf.dat")), None, "alone, it's in no project");

        fs::create_dir_all(dir.path().join(".git")).unwrap();
        assert_eq!(find_project_root(&mib.join("ccf.dat")), Some(dir.path().to_path_buf()), "in a repository, the repository's");
        assert_eq!(find_project_root(&mib), Some(dir.path().to_path_buf()));

        fs::create_dir_all(mib.join(".fenix")).unwrap();
        fs::write(mib.join(".fenix").join("settings.toml"), b"[ccsds]\npus = \"c\"\n").unwrap();
        assert_eq!(find_project_root(&mib.join("ccf.dat")), Some(mib.clone()), "with a .fenix of its own, a project");
    }

    /// A language pack's marker is a root only where nothing of Fenix's own
    /// is found above: a lone `dotnet new console`, not a folder in a repo.
    /// (One test sets the markers: they're global.)
    #[test]
    fn a_language_pack_s_marker_is_a_root_when_nothing_else_is() {
        let _kinds = crate::test_util::kinds_lock();
        set_extension_markers(vec!["{folder}.csproj".into(), "global.json".into()]);
        let dir = TempDir::new("pack_marker");
        let app = dir.path().join("hello");
        fs::create_dir_all(app.join("Models")).unwrap();
        fs::write(app.join("hello.csproj"), b"<Project/>").unwrap();
        assert_eq!(find_project_root(&app.join("Models").join("User.cs")), Some(app.clone()));
        fs::write(app.join("other.csproj"), b"").unwrap();
        let stray = dir.path().join("stray");
        fs::create_dir_all(&stray).unwrap();
        fs::write(stray.join("x.csproj"), b"").unwrap();
        assert_eq!(find_project_root(&stray.join("a.cs")), None, "{{folder}} is the folder's own name");
        // In a repository, the repository is still the project.
        fs::create_dir_all(dir.path().join(".git")).unwrap();
        assert_eq!(find_project_root(&app.join("Program.cs")), Some(dir.path().to_path_buf()));
        set_extension_markers(Vec::new());
    }

    /// A kind's markers find its project's top as a context's do, and
    /// under the same rule: only where nothing of Fenix's is above (Q6).
    #[test]
    fn a_kinds_marker_is_a_root_when_nothing_else_is() {
        let _kinds = crate::test_util::kinds_lock();
        let kind = |id: &str, markers: &[&str]| crate::KindDef {
            id: id.into(),
            name: id.into(),
            tag: "X".into(),
            color: crate::KindColor::DEFAULT,
            markers: markers.iter().map(|m| m.to_string()).collect(),
            main: Vec::new(),
            extension: Some("acme.x".into()),
        };
        crate::set_extension_kinds(vec![kind("gradle-x", &["build.gradle.kts"]), kind("node", &["deno.json"])]);
        let dir = TempDir::new("kind_marker");
        let app = dir.path().join("app");
        fs::create_dir_all(app.join("src").join("main")).unwrap();
        fs::write(app.join("build.gradle.kts"), b"").unwrap();
        assert_eq!(find_project_root(&app.join("src").join("main").join("A.kt")), Some(app.clone()));
        assert!(holds_pack_marker(&app));
        let deno = dir.path().join("deno");
        fs::create_dir_all(&deno).unwrap();
        fs::write(deno.join("deno.json"), b"{}").unwrap();
        assert_eq!(find_project_root(&deno.join("main.ts")), Some(deno.clone()), "a Fenix id's added marker too");
        // In a repository, the repository is still the project.
        fs::create_dir_all(dir.path().join(".git")).unwrap();
        assert_eq!(find_project_root(&app.join("src").join("main").join("A.kt")), Some(dir.path().to_path_buf()));
        crate::set_extension_kinds(Vec::new());
        fs::remove_dir_all(dir.path().join(".git")).unwrap();
        assert!(!holds_pack_marker(&app));
    }

    /// No extension's marker -- a kind's or a context's -- makes the home
    /// folder, a folder above it or a file system's root a project; Fenix's
    /// own markers do, as they always did.
    #[test]
    fn a_packs_marker_never_makes_home_a_project() {
        let _kinds = crate::test_util::kinds_lock();
        set_extension_markers(vec![".ruby-version".into()]);
        crate::set_extension_kinds(vec![crate::KindDef {
            id: "lua".into(),
            name: "Lua".into(),
            tag: "LUA".into(),
            color: crate::KindColor::DEFAULT,
            markers: vec![".luarc.json".into()],
            main: Vec::new(),
            extension: Some("acme.lua".into()),
        }]);
        let dir = TempDir::new("home_marker");
        let home = dir.path().join("home").join("me");
        let loose = home.join("notes").join("today");
        fs::create_dir_all(&loose).unwrap();
        for marker in [".ruby-version", ".luarc.json"] {
            fs::write(home.join(marker), b"").unwrap();
            assert_eq!(find_project_root_below(&loose.join("a.md"), Some(&home)), None, "{marker} in home");
            fs::write(dir.path().join("home").join(marker), b"").unwrap();
            assert_eq!(find_project_root_below(&loose.join("a.md"), Some(&home)), None, "{marker} above home");
            // Below home, a pack's marker is a project's top as before.
            fs::write(home.join("notes").join(marker), b"").unwrap();
            assert_eq!(find_project_root_below(&loose.join("a.md"), Some(&home)), Some(home.join("notes")));
            for f in [home.join(marker), dir.path().join("home").join(marker), home.join("notes").join(marker)] {
                fs::remove_file(f).unwrap();
            }
        }
        assert!(!pack_may_root(Path::new("/"), None), "a file system's root");
        // Fenix's own markers are what they were: home with a .git is a project.
        fs::create_dir_all(home.join(".git")).unwrap();
        assert_eq!(find_project_root_below(&loose.join("a.md"), Some(&home)), Some(home.clone()));
        set_extension_markers(Vec::new());
        crate::set_extension_kinds(Vec::new());
    }

    #[test]
    fn finds_a_cargo_toml_marker() {
        let dir = TempDir::new("cargo_marker");
        fs::write(dir.path().join("Cargo.toml"), b"[package]").unwrap();
        assert_eq!(find_project_root(dir.path()), Some(dir.path().to_path_buf()));
    }

    #[test]
    fn finds_a_cmakelists_txt_marker() {
        let dir = TempDir::new("cmake_marker");
        fs::write(dir.path().join("CMakeLists.txt"), b"project(x)").unwrap();
        assert_eq!(find_project_root(dir.path()), Some(dir.path().to_path_buf()));
    }

    #[test]
    fn finds_the_explicit_projectile_marker() {
        let dir = TempDir::new("projectile_marker");
        fs::write(dir.path().join(".projectile"), b"").unwrap();
        assert_eq!(find_project_root(dir.path()), Some(dir.path().to_path_buf()));
    }

    #[test]
    fn accepts_a_file_path_and_searches_from_its_directory() {
        let dir = TempDir::new("file_path_input");
        fs::create_dir(dir.path().join(".git")).unwrap();
        let file = dir.path().join("main.rs");
        fs::write(&file, b"").unwrap();
        assert_eq!(find_project_root(&file), Some(dir.path().to_path_buf()));
    }

    #[test]
    fn the_closest_ancestor_with_a_marker_wins_over_a_farther_one() {
        let dir = TempDir::new("closest_wins");
        fs::create_dir(dir.path().join(".git")).unwrap(); // outer root
        let inner = dir.path().join("vendor").join("subcrate");
        fs::create_dir_all(&inner).unwrap();
        fs::write(inner.join("Cargo.toml"), b"[package]").unwrap(); // closer marker

        assert_eq!(find_project_root(&inner), Some(inner.clone()));
    }

    #[test]
    fn no_marker_anywhere_up_returns_none() {
        // A fresh temp dir with no markers anywhere in its ancestry
        // within the walk (relies on nothing above the OS temp dir
        // having a stray marker, true in any normal environment).
        let dir = TempDir::new("no_marker");
        assert_eq!(find_project_root(dir.path()), None);
    }
}
