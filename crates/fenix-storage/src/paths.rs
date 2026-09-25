//! Where Fenix keeps things, in one place. Four kinds, each where the
//! platform expects it:
//!
//! - **settings** -- what you chose (`settings.toml`), your snippets and
//!   your project templates. Roams with your profile on Windows
//!   (`%AppData%\fenix`); `~/.config/fenix` on Linux.
//! - **data** -- what you made in Fenix (the agenda). Beside the settings
//!   on Windows (`%AppData%\fenix\data`), `~/.local/share/fenix` on Linux.
//! - **state** -- what Fenix noticed about this machine: the session,
//!   window placement, recent files, known projects. Never roams
//!   (`%LocalAppData%\fenix\state`, `~/.local/state/fenix/state`).
//! - **local** -- unsaved buffers (`recovery`), downloaded language
//!   servers (`tools`) and the files a migration replaced (`backup`),
//!   beside the state.
//!
//! `FENIX_HOME` puts all four in one folder instead -- a portable
//! install, or a test run that mustn't touch the real ones. Nothing else
//! in Fenix names these folders.

use std::path::{Path, PathBuf};

/// The folders, resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Roots {
    pub settings: PathBuf,
    pub data: PathBuf,
    /// `state`, `recovery`, `tools` and `backup` live under this one.
    pub local: PathBuf,
}

impl Roots {
    /// All four under `home`.
    pub fn portable(home: &Path) -> Self {
        Roots { settings: home.to_path_buf(), data: home.join("data"), local: home.to_path_buf() }
    }

    /// The platform's own folders, `None` where it has no notion of them.
    pub fn platform() -> Option<Self> {
        let settings = dirs::config_dir()?.join("fenix");
        let data = match dirs::data_dir() {
            // Windows' and macOS's data folder is the settings folder.
            Some(dir) if dir.join("fenix") != settings => dir.join("fenix"),
            _ => settings.join("data"),
        };
        let local = dirs::state_dir().or_else(dirs::data_local_dir).map(|d| d.join("fenix")).unwrap_or_else(|| settings.clone());
        // A local folder that is the settings folder (macOS) keeps the
        // two apart by a subfolder.
        let local = if local == settings { settings.join("local") } else { local };
        Some(Roots { settings, data, local })
    }

    /// `FENIX_HOME` when it's set, else the platform's folders.
    pub fn current() -> Option<Self> {
        match std::env::var_os("FENIX_HOME").filter(|v| !v.is_empty()) {
            Some(home) => Some(Self::portable(Path::new(&home))),
            None => Self::platform(),
        }
    }

    pub fn settings_file(&self) -> PathBuf {
        self.settings.join("settings.toml")
    }

    pub fn snippets(&self) -> PathBuf {
        self.settings.join("snippets")
    }

    pub fn templates(&self) -> PathBuf {
        self.settings.join("templates")
    }

    pub fn state(&self, name: &str) -> PathBuf {
        self.local.join("state").join(name)
    }

    pub fn recovery(&self) -> PathBuf {
        self.local.join("recovery")
    }

    pub fn tools(&self) -> PathBuf {
        self.local.join("tools")
    }

    pub fn backup(&self) -> PathBuf {
        self.local.join("backup")
    }
}

/// `settings.toml`.
pub fn settings_file() -> Option<PathBuf> {
    Roots::current().map(|r| r.settings_file())
}

/// Your snippets, one folder per language.
pub fn snippets_dir() -> Option<PathBuf> {
    Roots::current().map(|r| r.snippets())
}

/// Your project templates.
pub fn templates_dir() -> Option<PathBuf> {
    Roots::current().map(|r| r.templates())
}

/// A file of your own data, like `agenda.json`.
pub fn data_file(name: &str) -> Option<PathBuf> {
    Roots::current().map(|r| r.data.join(name))
}

/// A state file, like `session.json`.
pub fn state_file(name: &str) -> Option<PathBuf> {
    Roots::current().map(|r| r.state(name))
}

/// Unsaved buffers.
pub fn recovery_dir() -> Option<PathBuf> {
    Roots::current().map(|r| r.recovery())
}

/// Downloaded language servers and tools.
pub fn tools_dir() -> Option<PathBuf> {
    Roots::current().map(|r| r.tools())
}

/// Files a migration replaced, kept as they were.
pub fn backup_dir() -> Option<PathBuf> {
    Roots::current().map(|r| r.backup())
}

/// Where everything lived before this layout: `config_dir()/fenix`, or
/// `FENIX_HOME` itself.
pub fn legacy_dir() -> Option<PathBuf> {
    match std::env::var_os("FENIX_HOME").filter(|v| !v.is_empty()) {
        Some(home) => Some(PathBuf::from(home)),
        None => dirs::config_dir().map(|d| d.join("fenix")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_portable_home_holds_everything() {
        let r = Roots::portable(Path::new("/p"));
        assert_eq!(r.settings_file(), Path::new("/p/settings.toml"));
        assert_eq!(r.state("session.json"), Path::new("/p/state/session.json"));
        assert_eq!(r.data.join("agenda.json"), Path::new("/p/data/agenda.json"));
        for dir in [r.recovery(), r.tools(), r.backup(), r.snippets(), r.templates()] {
            assert!(dir.starts_with("/p"), "{}", dir.display());
        }
    }

    #[test]
    fn the_platform_keeps_settings_apart_from_this_machines_state() {
        let Some(r) = Roots::platform() else { return };
        assert!(r.settings.ends_with("fenix"));
        assert_ne!(r.local, r.settings, "state and downloads don't roam with the settings");
        assert!(!r.state("session.json").starts_with(&r.settings) || r.local.starts_with(&r.settings));
        if cfg!(windows) {
            assert!(r.settings.to_string_lossy().contains("Roaming"), "{}", r.settings.display());
            assert!(r.local.to_string_lossy().contains("Local"), "{}", r.local.display());
            assert_eq!(r.data, r.settings.join("data"));
        }
    }
}
