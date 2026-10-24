//! The extensions installed on this machine, as the editor uses them: the
//! data the switched-on ones carry (themes, snippets, templates), read
//! beside yours, and `fenix --install-extension` and friends, which change
//! what's installed without opening a window.
//!
//! Two stores: yours, and the machine's (`managed`), which an
//! administrator fills for everyone with `--machine`. A managed extension
//! is pinned: it takes the place of yours with the same id, and only the
//! administrator switches, updates or removes it. The machine's
//! `policy.toml` can switch the store off, or name the registry, for
//! everyone.

use std::path::PathBuf;

use fenix_extensions::{Installed, Policy, Store};

/// The Fenix release this is, for an extension's `engine.fenix` range.
/// `release/2.0` is 2.0 before its version bump (held until release), so
/// this says so rather than the workspace's version.
pub const RELEASE: &str = "2.0.0";

pub fn release() -> semver::Version {
    semver::Version::parse(RELEASE).expect("a version")
}

/// `fenix --disable-extensions`: safe mode, the core alone for this run.
pub const SAFE_MODE_FLAG: &str = "--disable-extensions";

static SAFE_MODE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();

/// Whether this command line asks for safe mode.
pub fn wants_safe_mode(args: &[String]) -> bool {
    args.iter().any(|arg| arg == SAFE_MODE_FLAG)
}

/// Settles safe mode for this run -- in `main`, before anything asks.
pub fn set_safe_mode(on: bool) {
    let _ = SAFE_MODE.set(on);
}

/// Whether this run is in safe mode: no built-in extension switched on
/// and nothing read from an installed one. Your own settings, themes and
/// snippets still are.
pub fn safe_mode() -> bool {
    SAFE_MODE.get().copied().unwrap_or(false)
}

/// The switched-on installed extensions' data folders of kind `kind`
/// (`themes`, `snippets`, `templates`), the managed ones' first. None in
/// tests, or in safe mode.
pub fn data_dirs(kind: &str) -> Vec<PathBuf> {
    if cfg!(test) || safe_mode() {
        return Vec::new();
    }
    dirs_of(&everything(machine().as_ref(), store().as_ref()), kind)
}

/// Your store of installed extensions; none in tests.
pub fn store() -> Option<Store> {
    if cfg!(test) {
        return None;
    }
    Store::current()
}

/// The machine's store, which an administrator fills; none in tests.
pub fn machine() -> Option<Store> {
    if cfg!(test) {
        return None;
    }
    Store::managed()
}

/// `--machine`: the extension flags work on the machine's store.
pub const MACHINE_FLAG: &str = "--machine";

pub fn wants_machine(args: &[String]) -> bool {
    args.iter().any(|arg| arg == MACHINE_FLAG)
}

/// The managed extensions, then yours that none of them replaces:
/// `(extension, managed)`.
fn everything(machine: Option<&Store>, yours: Option<&Store>) -> Vec<(Installed, bool)> {
    let mut all: Vec<(Installed, bool)> = machine.and_then(|m| m.installed().ok()).unwrap_or_default().into_iter().map(|e| (e, true)).collect();
    for e in yours.and_then(|s| s.installed().ok()).unwrap_or_default() {
        if !all.iter().any(|(m, _)| m.id == e.id) {
            all.push((e, false));
        }
    }
    all
}

/// Where the switched-on ones in `all` keep data of kind `kind`.
fn dirs_of(all: &[(Installed, bool)], kind: &str) -> Vec<PathBuf> {
    all.iter().filter(|(e, _)| e.enabled && e.manifest.is_ok()).map(|(e, _)| e.dir.join(kind)).filter(|d| d.is_dir()).collect()
}

/// The installed extensions that were switched on when Fenix started --
/// whose data is in use this run (none in safe mode).
fn running() -> &'static [String] {
    static RUNNING: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    RUNNING.get_or_init(|| {
        if safe_mode() {
            return Vec::new();
        }
        everything(machine().as_ref(), store().as_ref()).into_iter().filter(|(e, _)| e.enabled).map(|(e, _)| e.id).collect()
    })
}

/// The Extensions page's rows for what's installed.
pub fn rows() -> Vec<crate::extensions_page::InstalledRow> {
    let running = running();
    everything(machine().as_ref(), store().as_ref())
        .into_iter()
        .map(|(e, managed)| crate::extensions_page::InstalledRow {
            managed,
            name: e.name(),
            about: e.manifest.as_ref().map(|m| m.extension.description.clone()).unwrap_or_default(),
            broken: e.manifest.as_ref().err().cloned(),
            running: running.contains(&e.id),
            on: e.enabled,
            version: e.version,
            id: e.id,
        })
        .collect()
}

/// The registry's public key (minisign, base64; key id 20C95551642957AA),
/// built in: an index not signed with its secret half is refused. The
/// secret half is `MINISIGN_SECRET_KEY` in tpedneault/fenix-extensions'
/// Actions secrets.
pub const REGISTRY_KEY: &str = "RWSqVylkUVXJIH6oGO82ACjpSnK7iZGhkVnxqp3yL6yc4j+kra8fR0tI";

/// Your settings, read on their own (the command line runs before the
/// editor's are).
fn settings() -> Option<fenix_config::Config> {
    fenix_config::Config::load(fenix_storage::paths::settings_file()?).ok()
}

/// The registry this machine's policy and your settings name, unless one
/// of them switches the store off. A policy that can't be read keeps the
/// store shut rather than guess.
pub fn registry(config: Option<&fenix_config::Config>) -> Result<fenix_extensions::registry::Registry<'static>, String> {
    let policy = machine().map(|m| m.policy()).transpose()?.unwrap_or_default();
    registry_with(config, &policy)
}

fn registry_with(config: Option<&fenix_config::Config>, policy: &Policy) -> Result<fenix_extensions::registry::Registry<'static>, String> {
    if policy.store == Some(false) {
        return Err("the store is switched off on this machine by its administrator -- install from a file instead".into());
    }
    if config.and_then(|c| c.bool_value("extensions.store")) == Some(false) {
        return Err("the store is switched off here (extensions.store) -- install from a file instead".into());
    }
    let url = match policy.registry.as_deref().filter(|u| !u.trim().is_empty()) {
        Some(url) => url.to_string(),
        None => config.and_then(|c| c.text_value("extensions.registry")).filter(|u| !u.trim().is_empty()).unwrap_or(fenix_extensions::registry::DEFAULT_URL).to_string(),
    };
    Ok(fenix_extensions::registry::Registry { url, public_key: REGISTRY_KEY })
}

/// Installs `id` from `registry` into `store`: what to say.
pub fn install_from(registry: &fenix_extensions::registry::Registry, id: &str, store: &fenix_extensions::Store) -> Result<String, String> {
    let done = registry.install(id, store, &fenix_extensions::registry::Http, &release())?;
    Ok(done.iter().map(|e| format!("Installed {} {} ({}) from the registry.", e.name(), e.version, e.id)).collect::<Vec<_>>().join("\n") + " It's used from the next start.")
}

/// The flags, as typed.
pub const INSTALL_FLAG: &str = "--install-extension";
pub const UNINSTALL_FLAG: &str = "--uninstall-extension";
pub const LIST_FLAG: &str = "--list-extensions";

/// What one of the extension flags asks, if the command line has one.
#[derive(Debug, PartialEq)]
pub enum Request {
    Install(PathBuf),
    Uninstall(String),
    List,
}

/// The extension flag on this command line, if any. `Err` when it's
/// missing what it needs.
pub fn request(args: &[String]) -> Result<Option<Request>, String> {
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let (flag, inline) = match arg.split_once('=') {
            Some((flag, value)) => (flag, Some(value.to_string())),
            None => (arg.as_str(), None),
        };
        let mut value = |what: &str| inline.clone().or_else(|| args.next().cloned()).ok_or_else(|| format!("{flag} needs {what}"));
        match flag {
            INSTALL_FLAG => return Ok(Some(Request::Install(PathBuf::from(value("a .fenixext file, an extension's folder, or an id from the registry")?)))),
            UNINSTALL_FLAG => return Ok(Some(Request::Uninstall(value("an extension's id")?))),
            LIST_FLAG => return Ok(Some(Request::List)),
            _ => {}
        }
    }
    Ok(None)
}

/// Refuses `id` when the machine has it: it's the administrator's.
fn not_managed(id: &str, managed: Option<&Store>) -> Result<(), String> {
    match managed.and_then(|m| m.installed().ok()).unwrap_or_default().into_iter().find(|e| e.id == id) {
        Some(e) => Err(format!("{id} {} is managed on this machine -- its administrator installs, updates and removes it", e.version)),
        None => Ok(()),
    }
}

/// Carries out `request` on `store`: what to print, and whether it went.
/// `managed` is the machine's store when `store` is yours.
pub fn run(request: &Request, store: &Store, managed: Option<&Store>) -> Result<String, String> {
    match request {
        // An id, not a file there: from the registry.
        Request::Install(path) if !path.exists() && path.to_str().is_some_and(fenix_extensions::manifest::valid_id) => {
            let id = path.to_str().expect("checked");
            not_managed(id, managed)?;
            let config = settings();
            let registry = registry(config.as_ref())?;
            install_from(&registry, id, store)
        }
        Request::Install(path) => {
            not_managed(&fenix_extensions::Package::open(path)?.manifest.extension.id, managed)?;
            let installed = store.install(path, &release())?;
            Ok(format!("Installed {} {} ({}). It's used from the next start.", installed.name(), installed.version, installed.id))
        }
        Request::Uninstall(id) => {
            not_managed(id, managed)?;
            store.uninstall(id)?;
            Ok(format!("Removed {id}. It's gone from the next start."))
        }
        Request::List => {
            let all = everything(managed, Some(store));
            if all.is_empty() {
                return Ok(format!("No extensions installed (in {}).", store.root().display()));
            }
            Ok(all
                .iter()
                .map(|(e, by_machine)| {
                    let state = match (&e.manifest, e.enabled) {
                        (Err(err), _) => format!("broken: {err}"),
                        (Ok(_), true) => "on".to_string(),
                        (Ok(_), false) => "off".to_string(),
                    };
                    let managed = if *by_machine { ", managed" } else { "" };
                    format!("{}  {}  {}  ({state}{managed})", e.id, e.version, e.name())
                })
                .collect::<Vec<_>>()
                .join("\n"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn the_extension_flags_are_read() {
        assert_eq!(request(&args(&["--console", "--install-extension", "x.fenixext"])), Ok(Some(Request::Install("x.fenixext".into()))));
        assert_eq!(request(&args(&["--uninstall-extension=acme.x"])), Ok(Some(Request::Uninstall("acme.x".into()))));
        assert_eq!(request(&args(&["--list-extensions"])), Ok(Some(Request::List)));
        assert_eq!(request(&args(&["a.rs"])), Ok(None));
        assert!(request(&args(&["--install-extension"])).unwrap_err().contains("needs a .fenixext"));
    }

    #[test]
    fn the_built_in_registry_key_is_a_minisign_key() {
        assert!(minisign_verify::PublicKey::from_base64(REGISTRY_KEY).is_ok());
    }

    #[test]
    fn the_registrys_default_is_the_settings_default() {
        assert_eq!(fenix_config::setting("extensions.registry").map(|s| s.default), Some(fenix_extensions::registry::DEFAULT_URL));
    }

    #[test]
    fn with_the_store_off_nothing_is_looked_up() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        std::fs::write(&path, "[extensions]\nstore = false\n").unwrap();
        let config = fenix_config::Config::load(path).unwrap();
        assert!(registry(Some(&config)).err().unwrap().contains("extensions.store"));
        assert_eq!(registry(None).ok().map(|r| r.url), Some(fenix_extensions::registry::DEFAULT_URL.to_string()));
    }

    #[test]
    fn safe_mode_is_asked_for_by_its_flag() {
        assert!(wants_safe_mode(&args(&["--disable-extensions", "a.rs"])));
        assert!(!wants_safe_mode(&args(&["a.rs", "--console"])));
    }

    #[test]
    fn this_release_is_at_least_the_packages_version() {
        assert!(release() >= semver::Version::parse(env!("CARGO_PKG_VERSION")).unwrap());
    }

    #[test]
    fn installing_listing_and_removing_say_what_happened() {
        let dir = tempfile::tempdir().unwrap();
        let store = fenix_extensions::Store::new(dir.path().join("extensions"));
        assert!(run(&Request::List, &store, None).unwrap().starts_with("No extensions installed"));
        let pack = extension(dir.path(), "acme.dusk", "1.0.0");
        assert_eq!(run(&Request::Install(pack), &store, None).unwrap(), "Installed Dusk 1.0.0 (acme.dusk). It's used from the next start.");
        assert_eq!(run(&Request::List, &store, None).unwrap(), "acme.dusk  1.0.0  Dusk  (on)");
        assert!(run(&Request::Uninstall("acme.dusk".into()), &store, None).unwrap().starts_with("Removed acme.dusk"));
        assert!(run(&Request::Uninstall("acme.dusk".into()), &store, None).unwrap_err().contains("isn't installed"));
    }

    /// An extension's folder, `id` at `version`, carrying a theme folder.
    fn extension(dir: &std::path::Path, id: &str, version: &str) -> PathBuf {
        let pack = dir.join(format!("{id}-{version}"));
        std::fs::create_dir_all(pack.join("themes")).unwrap();
        std::fs::write(pack.join("themes").join("dusk.toml"), "").unwrap();
        std::fs::write(pack.join("extension.toml"), format!("[extension]\nid = \"{id}\"\nname = \"Dusk\"\nversion = \"{version}\"\nruntime = \"none\"\n\n[engine]\nfenix = \">=2.0, <3\"\n")).unwrap();
        pack
    }

    /// What the machine has is the administrator's: it replaces yours of
    /// the same id, and you can't install, update or remove it.
    #[test]
    fn a_managed_extension_is_pinned_and_replaces_yours() {
        let dir = tempfile::tempdir().unwrap();
        let (yours, machine) = (Store::new(dir.path().join("yours")), Store::new(dir.path().join("machine")));
        run(&Request::Install(extension(dir.path(), "acme.dusk", "1.0.0")), &yours, None).unwrap();
        run(&Request::Install(extension(dir.path(), "acme.dawn", "1.0.0")), &yours, None).unwrap();
        // The administrator, with --machine.
        run(&Request::Install(extension(dir.path(), "acme.dusk", "0.9.0")), &machine, None).unwrap();
        let all = everything(Some(&machine), Some(&yours));
        assert_eq!(all.iter().map(|(e, m)| (e.id.as_str(), e.version.as_str(), *m)).collect::<Vec<_>>(), [("acme.dusk", "0.9.0", true), ("acme.dawn", "1.0.0", false)]);
        assert_eq!(dirs_of(&all, "themes"), [machine.root().join("acme.dusk").join("0.9.0").join("themes"), yours.root().join("acme.dawn").join("1.0.0").join("themes")]);
        let pinned = run(&Request::Install(extension(dir.path(), "acme.dusk", "2.0.0")), &yours, Some(&machine)).unwrap_err();
        assert!(pinned.contains("acme.dusk 0.9.0 is managed on this machine"), "{pinned}");
        assert!(run(&Request::Install("acme.dusk".into()), &yours, Some(&machine)).unwrap_err().contains("managed"), "not from the registry either");
        assert!(run(&Request::Uninstall("acme.dusk".into()), &yours, Some(&machine)).unwrap_err().contains("managed"));
        assert_eq!(run(&Request::List, &yours, Some(&machine)).unwrap(), "acme.dusk  0.9.0  Dusk  (on, managed)\nacme.dawn  1.0.0  Dusk  (on)");
    }

    #[test]
    fn the_machines_policy_wins_over_your_settings() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        std::fs::write(&path, "[extensions]\nregistry = \"https://yours.test/index.json\"\n").unwrap();
        let config = fenix_config::Config::load(path).unwrap();
        let off = Policy { store: Some(false), registry: None };
        assert!(registry_with(Some(&config), &off).err().unwrap().contains("by its administrator"));
        let mirror = Policy { store: None, registry: Some("https://mirror.test/index.json".into()) };
        assert_eq!(registry_with(Some(&config), &mirror).ok().map(|r| r.url).as_deref(), Some("https://mirror.test/index.json"));
        assert_eq!(registry_with(Some(&config), &Policy::default()).ok().map(|r| r.url).as_deref(), Some("https://yours.test/index.json"));
    }

    #[test]
    fn machine_is_asked_for_by_its_flag() {
        assert!(wants_machine(&args(&["--install-extension", "x.fenixext", "--machine"])));
        assert!(!wants_machine(&args(&["--install-extension", "x.fenixext"])));
    }
}
