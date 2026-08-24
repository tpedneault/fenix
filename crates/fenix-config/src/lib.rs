//! Fenix's unified settings file -- `dirs::config_dir()/fenix/config.ini`
//! (`%AppData%\fenix\config.ini` on Windows, `~/.config/fenix/config.ini`
//! on Unix). Replaces what used to be three separate flat files (theme
//! choice, font size, indent width), each with its own free-function
//! trio, now that there's more than one setting worth persisting -- one
//! shared file, one struct, the same `load`/`save` shape `fenix_project::
//! KnownProjects`/`RecentFiles` already established for multi-field
//! persisted state.
//!
//! Every field is `Option<T>`: "this key was present and parsed" vs.
//! not. `Config` itself has no notion of what a *sane default* is for
//! any of them -- those constants (`theme::ORBIT_DARK`, `text::
//! FONT_SIZE`, `fenix_vim::DEFAULT_INDENT_WIDTH`) live in crates that
//! would create a dependency cycle if `fenix-config` depended on them,
//! so each real consumer in `fenix-gui` supplies its own already-
//! existing default via `.unwrap_or(...)`. This also means a corrupted
//! or missing individual key only ever loses *that* setting, not every
//! setting in the file.

mod ini;

use std::io;
use std::path::PathBuf;

pub struct Config {
    path: PathBuf,
    pub theme: Option<String>,
    pub font_size: Option<f32>,
    pub indent_width: Option<usize>,
    pub completion_symbols_file: Option<PathBuf>,
}

impl Config {
    /// `dirs::config_dir()/fenix/config.ini` -- same location convention
    /// every other persisted file in this project already established.
    /// `None` only on a platform with no notion of a config directory.
    pub fn default_path() -> Option<PathBuf> {
        dirs::config_dir().map(|dir| dir.join("fenix").join("config.ini"))
    }

    /// Loads settings from `path`. A missing file means "nothing
    /// configured yet," not an error -- every field comes back `None`,
    /// same as `KnownProjects::load` starting with an empty list.
    pub fn load(path: PathBuf) -> io::Result<Self> {
        let sections = match std::fs::read_to_string(&path) {
            Ok(contents) => ini::parse(&contents),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Default::default(),
            Err(e) => return Err(e),
        };

        let editor = sections.get("editor");
        let completion = sections.get("completion");

        Ok(Self {
            path,
            theme: editor.and_then(|s| s.get("theme")).cloned(),
            font_size: editor.and_then(|s| s.get("font_size")).and_then(|v| v.parse().ok()),
            indent_width: editor.and_then(|s| s.get("indent_width")).and_then(|v| v.parse().ok()),
            completion_symbols_file: completion.and_then(|s| s.get("symbols_file")).map(PathBuf::from),
        })
    }

    /// Same as `load`, but never fails -- any read error (not just a
    /// missing file) just starts with every field `None`. A convenience
    /// preference, not critical data, same posture as `KnownProjects::
    /// load_or_default`.
    pub fn load_or_default(path: PathBuf) -> Self {
        Self::load(path.clone()).unwrap_or(Self { path, theme: None, font_size: None, indent_width: None, completion_symbols_file: None })
    }

    /// Writes the known two-section, four-key layout, creating parent
    /// directories as needed -- only `Some` fields are written, so a
    /// setting cleared back to `None` doesn't leave a stale empty line
    /// behind on the next save. Not a generic INI writer: there's only
    /// ever this one shape to write, so building one would be unused
    /// machinery.
    pub fn save(&self) -> io::Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut out = String::new();
        out.push_str("[editor]\n");
        if let Some(theme) = &self.theme {
            out.push_str(&format!("theme = {theme}\n"));
        }
        if let Some(font_size) = self.font_size {
            out.push_str(&format!("font_size = {font_size}\n"));
        }
        if let Some(indent_width) = self.indent_width {
            out.push_str(&format!("indent_width = {indent_width}\n"));
        }
        out.push('\n');
        out.push_str("[completion]\n");
        if let Some(symbols_file) = &self.completion_symbols_file {
            out.push_str(&format!("symbols_file = {}\n", symbols_file.display()));
        }
        std::fs::write(&self.path, out)
    }

    #[cfg(test)]
    fn path(&self) -> &std::path::Path {
        &self.path
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn temp_path(name: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("fenix-config-test-{name}-{}-{n}.ini", std::process::id()))
    }

    #[test]
    fn loading_a_missing_file_yields_every_field_none_not_an_error() {
        let config = Config::load(temp_path("missing")).unwrap();
        assert!(config.theme.is_none());
        assert!(config.font_size.is_none());
        assert!(config.indent_width.is_none());
        assert!(config.completion_symbols_file.is_none());
    }

    #[test]
    fn load_or_default_never_fails_even_when_the_path_is_unreadable() {
        let path = temp_path("unreadable");
        std::fs::create_dir(&path).unwrap(); // a directory, not a file -- read_to_string fails non-NotFound
        assert!(Config::load(path.clone()).is_err());
        let config = Config::load_or_default(path.clone());
        assert!(config.theme.is_none());
        std::fs::remove_dir_all(&path).ok();
    }

    #[test]
    fn all_four_fields_round_trip_through_save_and_load() {
        let path = temp_path("round_trip");
        let mut config = Config::load_or_default(path.clone());
        config.theme = Some("TempleOS".to_string());
        config.font_size = Some(18.0);
        config.indent_width = Some(2);
        config.completion_symbols_file = Some(PathBuf::from("/home/thomas/tcl-symbols.txt"));
        config.save().unwrap();

        let reloaded = Config::load(path.clone()).unwrap();
        assert_eq!(reloaded.theme, Some("TempleOS".to_string()));
        assert_eq!(reloaded.font_size, Some(18.0));
        assert_eq!(reloaded.indent_width, Some(2));
        assert_eq!(reloaded.completion_symbols_file, Some(PathBuf::from("/home/thomas/tcl-symbols.txt")));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn a_corrupted_font_size_does_not_blank_out_the_other_settings() {
        let path = temp_path("partial_corruption");
        std::fs::write(&path, "[editor]\ntheme = TempleOS\nfont_size = not-a-number\nindent_width = 3\n").unwrap();

        let config = Config::load(path.clone()).unwrap();
        assert_eq!(config.theme, Some("TempleOS".to_string()));
        assert!(config.font_size.is_none()); // only this one is lost
        assert_eq!(config.indent_width, Some(3));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn save_creates_missing_parent_directories() {
        let path = temp_path("creates_parents").parent().unwrap().join("nested-fenix-config-test").join("config.ini");
        let mut config = Config::load_or_default(path.clone());
        config.theme = Some("Orbit Dark".to_string());
        config.save().unwrap();
        assert!(path.exists());
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn save_omits_keys_that_are_none_instead_of_writing_an_empty_value() {
        let path = temp_path("omits_none");
        let mut config = Config::load_or_default(path.clone());
        config.theme = Some("Orbit Dark".to_string());
        config.save().unwrap();

        let contents = std::fs::read_to_string(config.path()).unwrap();
        assert!(!contents.contains("font_size"));
        assert!(!contents.contains("symbols_file"));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn default_path_is_under_a_fenix_directory_named_config_dot_ini() {
        if let Some(path) = Config::default_path() {
            assert!(path.ends_with("fenix/config.ini") || path.ends_with("fenix\\config.ini"));
        }
    }
}
