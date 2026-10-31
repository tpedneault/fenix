//! The extensions installed on this machine: each unpacked under
//! `<root>/<id>/<version>/`, and `<root>/extensions.toml` saying which
//! are installed and which are switched on.
//!
//! Installing unpacks into `.staging/` first and renames the finished
//! folder into place, then records it; the record is written whole or not
//! at all (`fenix_storage::write`). An interrupted install leaves a
//! staging folder (cleared next time), never a half-unpacked extension.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::manifest::Manifest;
use crate::package::Package;

/// What `extensions.toml` keeps for one extension.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Entry {
    version: String,
    #[serde(default = "yes")]
    enabled: bool,
    /// Installed only because another needed it (a bundle's member): it
    /// goes when nothing does any more.
    #[serde(default, skip_serializing_if = "no")]
    dependency: bool,
}

fn yes() -> bool {
    true
}

fn no(b: &bool) -> bool {
    !*b
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct State {
    #[serde(default)]
    extensions: BTreeMap<String, Entry>,
}

/// What an administrator decides for everyone on a machine, in
/// `policy.toml` beside the managed extensions.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    /// `false`: no one here looks up or downloads from a registry.
    pub store: Option<bool>,
    /// The registry everyone here uses (a mirror), whatever their
    /// `extensions.registry` says.
    pub registry: Option<String>,
}

/// An installed extension.
#[derive(Debug, Clone)]
pub struct Installed {
    pub id: String,
    pub version: String,
    pub enabled: bool,
    /// Installed only because another needed it.
    pub dependency: bool,
    /// Where it's unpacked.
    pub dir: PathBuf,
    /// Its manifest, as read from there; `Err` when that's gone or broken.
    pub manifest: Result<Manifest, String>,
}

impl Installed {
    pub fn name(&self) -> String {
        self.manifest.as_ref().map(|m| m.extension.name.clone()).unwrap_or_else(|_| self.id.clone())
    }
}

/// This machine's installed extensions, under `root`.
pub struct Store {
    root: PathBuf,
}

impl Store {
    pub fn new(root: impl Into<PathBuf>) -> Store {
        Store { root: root.into() }
    }

    /// `<local>/extensions` for this machine (`FENIX_HOME` when set).
    pub fn current() -> Option<Store> {
        fenix_storage::paths::extensions_dir().map(Store::new)
    }

    /// The machine-wide store an administrator fills, for everyone here
    /// (`fenix_storage::paths::managed_extensions_dir`).
    pub fn managed() -> Option<Store> {
        fenix_storage::paths::managed_extensions_dir().map(Store::new)
    }

    /// This store's `policy.toml`; none when there isn't one.
    pub fn policy(&self) -> Result<Policy, String> {
        let path = self.root.join("policy.toml");
        match std::fs::read_to_string(&path) {
            Ok(text) => toml::from_str(&text).map_err(|e| format!("{}: {}", path.display(), e.message())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Policy::default()),
            Err(e) => Err(format!("{}: {e}", path.display())),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn state_path(&self) -> PathBuf {
        self.root.join("extensions.toml")
    }

    fn state(&self) -> Result<State, String> {
        match std::fs::read_to_string(self.state_path()) {
            Ok(text) => toml::from_str(&text).map_err(|e| format!("extensions.toml: {}", e.message())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(State::default()),
            Err(e) => Err(format!("extensions.toml: {e}")),
        }
    }

    fn save(&self, state: &State) -> Result<(), String> {
        std::fs::create_dir_all(&self.root).map_err(|e| format!("{}: {e}", self.root.display()))?;
        let text = "# The extensions installed here, and whether each is on. Fenix writes this;\n# SPC h x and fenix --install-extension change it.\n\n".to_string()
            + &toml::to_string(state).map_err(|e| e.to_string())?;
        fenix_storage::write(&self.state_path(), text.as_bytes()).map_err(|e| format!("couldn't write extensions.toml: {e}"))
    }

    fn dir_of(&self, id: &str, version: &str) -> PathBuf {
        self.root.join(id).join(version)
    }

    /// Every installed extension, by id.
    pub fn installed(&self) -> Result<Vec<Installed>, String> {
        let state = self.state()?;
        Ok(state
            .extensions
            .into_iter()
            .map(|(id, entry)| {
                let dir = self.dir_of(&id, &entry.version);
                let manifest = std::fs::read_to_string(dir.join("extension.toml")).map_err(|e| format!("{}: {e}", dir.display())).and_then(|t| Manifest::parse(&t));
                Installed { id, version: entry.version, enabled: entry.enabled, dependency: entry.dependency, dir, manifest }
            })
            .collect())
    }

    /// Where the switched-on extensions keep data of kind `kind` (`themes`,
    /// `snippets`, `templates`), those that carry it.
    pub fn data_dirs(&self, kind: &str) -> Vec<PathBuf> {
        self.installed().unwrap_or_default().into_iter().filter(|e| e.enabled && e.manifest.is_ok()).map(|e| e.dir.join(kind)).filter(|d| d.is_dir()).collect()
    }

    /// Installs the package at `path` for this Fenix (`fenix`, its
    /// version), replacing any version of it already here, switched on.
    /// One already here (an update) stays installed the way it was: for
    /// itself, or for another.
    pub fn install(&self, path: &Path, fenix: &semver::Version) -> Result<Installed, String> {
        self.install_as(path, fenix, false)
    }

    /// `install`, for another that needs it (`Registry::install`'s
    /// dependencies, a bundle's members): removed with the last that does
    /// (`uninstall_unneeded`).
    pub fn install_dependency(&self, path: &Path, fenix: &semver::Version) -> Result<Installed, String> {
        self.install_as(path, fenix, true)
    }

    fn install_as(&self, path: &Path, fenix: &semver::Version, dependency: bool) -> Result<Installed, String> {
        let package = Package::open(path)?;
        let m = &package.manifest;
        m.runs_in(fenix)?;
        if m.extension.runtime == crate::Runtime::Wasm && !package.files.iter().any(|(rel, _)| std::path::Path::new(rel) == std::path::Path::new("extension.wasm")) {
            return Err(format!("{} is a WebAssembly extension without its code (extension.wasm)", m.extension.name));
        }
        let mut state = self.state()?;
        for (dep, range) in &m.dependencies {
            let req = semver::VersionReq::parse(range).map_err(|e| e.to_string())?;
            match state.extensions.get(dep) {
                Some(e) if semver::Version::parse(&e.version).is_ok_and(|v| req.matches(&v)) => {}
                Some(e) => return Err(format!("{} needs {dep} {range}, and {} is installed", m.extension.name, e.version)),
                None => return Err(format!("{} needs {dep} {range} -- install that first", m.extension.name)),
            }
        }
        let (id, version) = (m.extension.id.clone(), m.extension.version.clone());

        // Unpacked beside its place, then renamed into it.
        let staging = self.root.join(".staging");
        let _ = std::fs::remove_dir_all(&staging);
        let unpacked = staging.join(format!("{id}-{version}"));
        for (rel, bytes) in &package.files {
            let to = unpacked.join(rel);
            if let Some(parent) = to.parent() {
                std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
            }
            std::fs::write(&to, bytes).map_err(|e| format!("{}: {e}", to.display()))?;
        }
        let dest = self.dir_of(&id, &version);
        if dest.exists() {
            std::fs::remove_dir_all(&dest).map_err(|e| format!("couldn't replace {}: {e}", dest.display()))?;
        }
        std::fs::create_dir_all(dest.parent().expect("under the root")).map_err(|e| e.to_string())?;
        std::fs::rename(&unpacked, &dest).map_err(|e| format!("couldn't move {} into place: {e}", id))?;
        let _ = std::fs::remove_dir_all(&staging);

        let dependency = state.extensions.get(&id).map_or(dependency, |e| e.dependency);
        let before = state.extensions.insert(id.clone(), Entry { version: version.clone(), enabled: true, dependency });
        self.save(&state)?;
        // Then the version it replaced, if another.
        if let Some(old) = before.filter(|old| old.version != version) {
            let _ = std::fs::remove_dir_all(self.dir_of(&id, &old.version));
        }
        Ok(Installed { id, version, enabled: true, dependency, dir: dest, manifest: Ok(package.manifest) })
    }

    /// Removes extension `id`: from the record first, then its files. A
    /// bundle doesn't keep its members: one goes on its own, and the
    /// bundle is the rest.
    pub fn uninstall(&self, id: &str) -> Result<(), String> {
        let mut state = self.state()?;
        let needs = |e: &Installed| e.id != id && e.manifest.as_ref().is_ok_and(|m| m.extension.runtime != crate::Runtime::Bundle && m.dependencies.contains_key(id));
        let needed: Vec<String> = self.installed()?.into_iter().filter(needs).map(|e| e.name()).collect();
        if !needed.is_empty() {
            return Err(format!("{} need{} {id} -- remove {} first", needed.join(", "), if needed.len() == 1 { "s" } else { "" }, if needed.len() == 1 { "it" } else { "them" }));
        }
        if state.extensions.remove(id).is_none() {
            return Err(format!("{id} isn't installed"));
        }
        self.save(&state)?;
        let _ = std::fs::remove_dir_all(self.root.join(id));
        Ok(())
    }

    /// Removes `id`, then what was installed only for it, or for another
    /// gone with it, that nothing still here needs -- a bundle's members
    /// you didn't install yourself. What was removed, `id` first.
    pub fn uninstall_unneeded(&self, id: &str) -> Result<Vec<String>, String> {
        self.uninstall(id)?;
        let mut removed = vec![id.to_string()];
        loop {
            let installed = self.installed()?;
            let needed = |id: &str| installed.iter().any(|e| e.manifest.as_ref().is_ok_and(|m| m.dependencies.contains_key(id)));
            let Some(next) = installed.iter().find(|e| e.dependency && !needed(&e.id)) else { break };
            self.uninstall(&next.id)?;
            removed.push(next.id.clone());
        }
        Ok(removed)
    }

    /// Switches extension `id` on or off, keeping it installed.
    pub fn set_enabled(&self, id: &str, enabled: bool) -> Result<(), String> {
        let mut state = self.state()?;
        let entry = state.extensions.get_mut(id).ok_or_else(|| format!("{id} isn't installed"))?;
        entry.enabled = enabled;
        self.save(&state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::sample;
    use crate::package::zip_of;

    fn fenix() -> semver::Version {
        semver::Version::parse("1.0.0").unwrap()
    }

    fn package(dir: &Path, id: &str, version: &str, extra: &str) -> PathBuf {
        let path = dir.join(format!("{id}-{version}.fenixext"));
        let manifest = sample(id, version, "none") + extra;
        zip_of(&path, &[("extension.toml", &manifest), ("themes/dusk.toml", version)]);
        path
    }

    #[test]
    fn a_policy_is_read_when_there_is_one() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path());
        assert_eq!(store.policy().unwrap(), Policy::default());
        std::fs::write(dir.path().join("policy.toml"), "store = false\nregistry = \"https://mirror.test/index.json\"\n").unwrap();
        assert_eq!(store.policy().unwrap(), Policy { store: Some(false), registry: Some("https://mirror.test/index.json".into()) });
        std::fs::write(dir.path().join("policy.toml"), "stores = false\n").unwrap();
        assert!(store.policy().unwrap_err().contains("policy.toml"));
    }

    #[test]
    fn a_code_extension_needs_its_code() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("store"));
        let manifest = sample("acme.code", "1.0.0", "wasm").replace("fenix = ", "api = \"1.0\"\nfenix = ");
        let without = dir.path().join("without.fenixext");
        zip_of(&without, &[("extension.toml", &manifest)]);
        assert!(store.install(&without, &fenix()).unwrap_err().contains("without its code"));
        let with = dir.path().join("with.fenixext");
        zip_of(&with, &[("extension.toml", &manifest), ("extension.wasm", "\0asm")]);
        assert!(store.install(&with, &fenix()).is_ok());
    }

    #[test]
    fn install_upgrade_switch_off_and_uninstall() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("extensions"));
        assert!(store.installed().unwrap().is_empty(), "nothing installed: no file yet");

        let one = store.install(&package(dir.path(), "acme.themes", "1.0.0", ""), &fenix()).unwrap();
        assert!(one.dir.ends_with("acme.themes/1.0.0") && one.dir.join("themes/dusk.toml").is_file());
        assert_eq!(store.data_dirs("themes"), [one.dir.join("themes")]);
        assert!(store.data_dirs("snippets").is_empty(), "it carries none");
        assert!(!store.root().join(".staging").exists(), "staging cleared");

        let two = store.install(&package(dir.path(), "acme.themes", "1.1.0", ""), &fenix()).unwrap();
        assert_eq!(std::fs::read_to_string(two.dir.join("themes/dusk.toml")).unwrap(), "1.1.0");
        assert!(!one.dir.exists(), "the version it replaced is gone");
        let listed = store.installed().unwrap();
        assert_eq!((listed.len(), listed[0].version.as_str(), listed[0].enabled), (1, "1.1.0", true));

        store.set_enabled("acme.themes", false).unwrap();
        assert!(store.data_dirs("themes").is_empty(), "off: its data isn't read");
        assert!(!store.installed().unwrap()[0].enabled);
        assert!(store.set_enabled("acme.nope", true).is_err());

        store.uninstall("acme.themes").unwrap();
        assert!(store.installed().unwrap().is_empty());
        assert!(!store.root().join("acme.themes").exists());
        assert!(store.uninstall("acme.themes").unwrap_err().contains("isn't installed"));
    }

    #[test]
    fn dependencies_are_installed_first_and_removed_last() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("extensions"));
        let needs = "\n[dependencies]\n\"acme.base\" = \"^1\"\n";
        let pack = package(dir.path(), "acme.pack", "1.0.0", needs);
        assert!(store.install(&pack, &fenix()).unwrap_err().contains("install that first"));
        store.install(&package(dir.path(), "acme.base", "2.0.0", ""), &fenix()).unwrap();
        assert!(store.install(&pack, &fenix()).unwrap_err().contains("2.0.0 is installed"));
        store.install(&package(dir.path(), "acme.base", "1.4.0", ""), &fenix()).unwrap();
        store.install(&pack, &fenix()).unwrap();
        assert!(store.uninstall("acme.base").unwrap_err().contains("remove it first"));
        store.uninstall("acme.pack").unwrap();
        store.uninstall("acme.base").unwrap();
    }

    /// A bundle brings its members; removing it takes those that came for
    /// it and nothing else needs, and leaves the one you installed
    /// yourself and the one another needs.
    #[test]
    fn a_bundle_goes_with_the_members_only_it_needed() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("extensions"));
        let bundle = |id: &str, members: &[&str]| {
            let path = dir.path().join(format!("{id}.fenixext"));
            let deps: String = members.iter().map(|m| format!("\"{m}\" = \"^1\"\n")).collect();
            zip_of(&path, &[("extension.toml", &(sample(id, "1.0.0", "bundle") + "\n[dependencies]\n" + &deps)), ("README.md", "# A bundle")]);
            path
        };
        store.install(&package(dir.path(), "acme.mine", "1.0.0", ""), &fenix()).unwrap();
        for id in ["acme.notes", "acme.board", "acme.shared"] {
            store.install_dependency(&package(dir.path(), id, "1.0.0", ""), &fenix()).unwrap();
        }
        let personal = store.install(&bundle("acme.personal", &["acme.notes", "acme.board", "acme.mine", "acme.shared"]), &fenix()).unwrap();
        assert_eq!(personal.manifest.unwrap().members().len(), 4);
        store.install(&bundle("acme.other", &["acme.shared"]), &fenix()).unwrap();
        assert!(store.installed().unwrap().iter().any(|e| e.id == "acme.notes" && e.dependency));
        // An update keeps how it came.
        store.install(&package(dir.path(), "acme.notes", "1.1.0", ""), &fenix()).unwrap();
        assert!(store.installed().unwrap().iter().any(|e| e.id == "acme.notes" && e.dependency && e.version == "1.1.0"));

        assert_eq!(store.uninstall_unneeded("acme.personal").unwrap(), ["acme.personal", "acme.board", "acme.notes"]);
        let left: Vec<String> = store.installed().unwrap().into_iter().map(|e| e.id).collect();
        assert_eq!(left, ["acme.mine", "acme.other", "acme.shared"], "yours, and what another needs");
        // A member goes on its own; the bundle stays.
        store.uninstall("acme.shared").unwrap();
        assert!(store.installed().unwrap().iter().any(|e| e.id == "acme.other"));
    }

    #[test]
    fn what_this_fenix_cant_install_leaves_nothing_behind() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("extensions"));
        let wasm = dir.path().join("code.fenixext");
        zip_of(&wasm, &[("extension.toml", &sample("acme.code", "1.0.0", "wasm"))]);
        assert!(store.install(&wasm, &fenix()).is_err());
        let old = package(dir.path(), "acme.themes", "1.0.0", "");
        assert!(store.install(&old, &semver::Version::parse("2.0.0").unwrap()).is_err(), "made for 1.x");
        assert!(!store.root().exists() || store.installed().unwrap().is_empty());
    }

    #[test]
    fn a_broken_record_is_said_not_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path());
        std::fs::write(dir.path().join("extensions.toml"), "extensions = 3").unwrap();
        assert!(store.installed().is_err());
        assert!(store.install(&package(dir.path(), "acme.themes", "1.0.0", ""), &fenix()).is_err());
        assert_eq!(std::fs::read_to_string(dir.path().join("extensions.toml")).unwrap(), "extensions = 3");
    }
}
