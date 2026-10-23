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

/// The switched-on installed extensions' data folders of kind `kind`
/// (`themes`, `snippets`, `templates`). None in tests.
pub fn data_dirs(kind: &str) -> Vec<PathBuf> {
    if cfg!(test) {
        return Vec::new();
    }
    fenix_extensions::Store::current().map(|store| store.data_dirs(kind)).unwrap_or_default()
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
            INSTALL_FLAG => return Ok(Some(Request::Install(PathBuf::from(value("a .fenixext file or an extension's folder")?)))),
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
