//! The extensions installed on this machine, as the editor uses them: the
//! data the switched-on ones carry (themes, snippets, templates), read
//! beside yours, and `fenix --install-extension` and friends, which change
//! what's installed without opening a window.

use std::path::PathBuf;

/// The Fenix release this is, for an extension's `engine.fenix` range.
/// `release/2.0` is 2.0 before its version bump (held until release), so
/// this says so rather than the workspace's version.
pub const RELEASE: &str = "2.0.0";

fn release() -> semver::Version {
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
/// (`themes`, `snippets`, `templates`). None in tests, or in safe mode.
pub fn data_dirs(kind: &str) -> Vec<PathBuf> {
    if cfg!(test) || safe_mode() {
        return Vec::new();
    }
    fenix_extensions::Store::current().map(|store| store.data_dirs(kind)).unwrap_or_default()
}

/// This machine's store of installed extensions; none in tests.
pub fn store() -> Option<fenix_extensions::Store> {
    if cfg!(test) {
        return None;
    }
    fenix_extensions::Store::current()
}

/// The installed extensions that were switched on when Fenix started --
/// whose data is in use this run (none in safe mode).
fn running() -> &'static [String] {
    static RUNNING: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    RUNNING.get_or_init(|| {
        if safe_mode() {
            return Vec::new();
        }
        store().and_then(|s| s.installed().ok()).unwrap_or_default().into_iter().filter(|e| e.enabled).map(|e| e.id).collect()
    })
}

/// The Extensions page's rows for what's installed.
pub fn rows() -> Vec<crate::extensions_page::InstalledRow> {
    let running = running();
    store()
        .and_then(|s| s.installed().ok())
        .unwrap_or_default()
        .into_iter()
        .map(|e| crate::extensions_page::InstalledRow {
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

/// The registry your settings name, unless they switch the store off.
fn registry(config: Option<&fenix_config::Config>) -> Result<fenix_extensions::registry::Registry<'static>, String> {
    if config.and_then(|c| c.bool_value("extensions.store")) == Some(false) {
        return Err("the store is switched off here (extensions.store) -- install from a file instead".into());
    }
    let url = config.and_then(|c| c.text_value("extensions.registry")).filter(|u| !u.trim().is_empty()).unwrap_or(fenix_extensions::registry::DEFAULT_URL);
    Ok(fenix_extensions::registry::Registry { url: url.to_string(), public_key: REGISTRY_KEY })
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

/// Carries out `request` on `store`: what to print, and whether it went.
pub fn run(request: &Request, store: &fenix_extensions::Store) -> Result<String, String> {
    match request {
        // An id, not a file there: from the registry.
        Request::Install(path) if !path.exists() && path.to_str().is_some_and(fenix_extensions::manifest::valid_id) => {
            let config = settings();
            let registry = registry(config.as_ref())?;
            let id = path.to_str().expect("checked");
            let done = registry.install(id, store, &fenix_extensions::registry::Http, &release())?;
            Ok(done.iter().map(|e| format!("Installed {} {} ({}) from the registry.", e.name(), e.version, e.id)).collect::<Vec<_>>().join("\n") + " It's used from the next start.")
        }
        Request::Install(path) => {
            let installed = store.install(path, &release())?;
            Ok(format!("Installed {} {} ({}). It's used from the next start.", installed.name(), installed.version, installed.id))
        }
        Request::Uninstall(id) => {
            store.uninstall(id)?;
            Ok(format!("Removed {id}. It's gone from the next start."))
        }
        Request::List => {
            let installed = store.installed()?;
            if installed.is_empty() {
                return Ok(format!("No extensions installed (in {}).", store.root().display()));
            }
            Ok(installed
                .iter()
                .map(|e| {
                    let state = match (&e.manifest, e.enabled) {
                        (Err(err), _) => format!("broken: {err}"),
                        (Ok(_), true) => "on".to_string(),
                        (Ok(_), false) => "off".to_string(),
                    };
                    format!("{}  {}  {}  ({state})", e.id, e.version, e.name())
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
        assert!(run(&Request::List, &store).unwrap().starts_with("No extensions installed"));
        let pack = dir.path().join("acme");
        std::fs::create_dir_all(pack.join("themes")).unwrap();
        std::fs::write(pack.join("extension.toml"), "[extension]\nid = \"acme.dusk\"\nname = \"Dusk\"\nversion = \"1.0.0\"\nruntime = \"none\"\n\n[engine]\nfenix = \">=2.0, <3\"\n").unwrap();
        assert_eq!(run(&Request::Install(pack), &store).unwrap(), "Installed Dusk 1.0.0 (acme.dusk). It's used from the next start.");
        assert_eq!(run(&Request::List, &store).unwrap(), "acme.dusk  1.0.0  Dusk  (on)");
        assert!(run(&Request::Uninstall("acme.dusk".into()), &store).unwrap().starts_with("Removed acme.dusk"));
        assert!(run(&Request::Uninstall("acme.dusk".into()), &store).unwrap_err().contains("isn't installed"));
    }
}
