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
    pub font_family: Option<String>,
    pub indent_width: Option<usize>,
    /// How many visual columns a literal `\t` character expands to when
    /// rendered (real Vim's own `:set tabstop`) -- distinct from
    /// `indent_width`, which governs what typing Tab in Insert mode or
    /// `>>`/`<<` actually *inserts* (always spaces; Fenix never inserts
    /// a real tab character itself). Consulted by `fenix-gui`'s
    /// `tabstops` module.
    pub tab_width: Option<usize>,
    pub completion_symbols_file: Option<PathBuf>,
    /// Configured SCOS-2000 MIB directories, `(label, path)`, in the
    /// order they appear in `config.ini`'s `[mib]` section -- an actual
    /// list, not an `Option<T>` like every other field here, since
    /// "nothing configured" is just an empty `Vec`, no need to
    /// distinguish that from "key present but empty" the way a scalar
    /// setting would. Hand-authored by the user (see `[mib]`'s own
    /// numbered-key format in `load`), never written by the app itself
    /// -- `save` still round-trips it losslessly since it regenerates
    /// every section from struct state on every call.
    pub mib_roots: Vec<(String, PathBuf)>,
    pub mib_telecommand_template: Option<String>,
    pub mib_telecommand_argument_template: Option<String>,
    pub mib_telecommand_argument_separator: Option<String>,
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
        let mib = sections.get("mib");

        Ok(Self {
            path,
            theme: editor.and_then(|s| s.get("theme")).cloned(),
            font_size: editor.and_then(|s| s.get("font_size")).and_then(|v| v.parse().ok()),
            font_family: editor.and_then(|s| s.get("font_family")).cloned(),
            indent_width: editor.and_then(|s| s.get("indent_width")).and_then(|v| v.parse().ok()),
            tab_width: editor.and_then(|s| s.get("tab_width")).and_then(|v| v.parse().ok()),
            completion_symbols_file: completion.and_then(|s| s.get("symbols_file")).map(PathBuf::from),
            mib_roots: mib.map(parse_mib_roots).unwrap_or_default(),
            mib_telecommand_template: mib.and_then(|s| s.get("telecommand_template")).cloned(),
            mib_telecommand_argument_template: mib.and_then(|s| s.get("telecommand_argument_template")).cloned(),
            mib_telecommand_argument_separator: mib.and_then(|s| s.get("telecommand_argument_separator")).cloned(),
        })
    }

    /// Same as `load`, but never fails -- any read error (not just a
    /// missing file) just starts with every field `None`. A convenience
    /// preference, not critical data, same posture as `KnownProjects::
    /// load_or_default`.
    pub fn load_or_default(path: PathBuf) -> Self {
        Self::load(path.clone()).unwrap_or(Self {
            path,
            theme: None,
            font_size: None,
            font_family: None,
            indent_width: None,
            tab_width: None,
            completion_symbols_file: None,
            mib_roots: Vec::new(),
            mib_telecommand_template: None,
            mib_telecommand_argument_template: None,
            mib_telecommand_argument_separator: None,
        })
    }

    /// Writes the known two-section, five-key layout, creating parent
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
            out.push_str(&format!("theme = {}\n", ini::quote_if_needed(theme)));
        }
        if let Some(font_size) = self.font_size {
            out.push_str(&format!("font_size = {font_size}\n"));
        }
        if let Some(font_family) = &self.font_family {
            out.push_str(&format!("font_family = {}\n", ini::quote_if_needed(font_family)));
        }
        if let Some(indent_width) = self.indent_width {
            out.push_str(&format!("indent_width = {indent_width}\n"));
        }
        if let Some(tab_width) = self.tab_width {
            out.push_str(&format!("tab_width = {tab_width}\n"));
        }
        out.push('\n');
        out.push_str("[completion]\n");
        if let Some(symbols_file) = &self.completion_symbols_file {
            out.push_str(&format!("symbols_file = {}\n", symbols_file.display()));
        }
        out.push('\n');
        out.push_str("[mib]\n");
        for (i, (label, root_path)) in self.mib_roots.iter().enumerate() {
            out.push_str(&format!("root{} = {label}|{}\n", i + 1, root_path.display()));
        }
        if let Some(template) = &self.mib_telecommand_template {
            out.push_str(&format!("telecommand_template = {}\n", ini::quote_if_needed(template)));
        }
        if let Some(template) = &self.mib_telecommand_argument_template {
            out.push_str(&format!("telecommand_argument_template = {}\n", ini::quote_if_needed(template)));
        }
        if let Some(separator) = &self.mib_telecommand_argument_separator {
            out.push_str(&format!("telecommand_argument_separator = {}\n", ini::quote_if_needed(separator)));
        }
        std::fs::write(&self.path, out)
    }

    #[cfg(test)]
    fn path(&self) -> &std::path::Path {
        &self.path
    }
}

/// Parses the `[mib]` section's `root1 = LABEL|PATH`, `root2 = ...`
/// numbered keys into an ordered `(label, path)` list. Numbered rather
/// than one `roots = ...` key: the INI parser (`ini::parse`) is single-
/// value-per-key, so a growable list needs its own key per entry, the
/// same convention plenty of other hand-rolled INI readers use for
/// lists. `|` splits label from path (not `:`, which Windows paths
/// contain); any key that isn't `rootN`, or a value with no `|`, is
/// silently skipped -- same "a bad entry loses only itself" posture
/// every other field in this file already has. Sorted by the numeric
/// ordinal, not by key string, so `root2` sorts before `root10`.
fn parse_mib_roots(section: &std::collections::BTreeMap<String, String>) -> Vec<(String, PathBuf)> {
    let mut roots: Vec<(usize, String, PathBuf)> = section
        .iter()
        .filter_map(|(key, value)| {
            let n = key.strip_prefix("root")?.parse::<usize>().ok()?;
            let (label, path) = value.split_once('|')?;
            Some((n, label.trim().to_string(), PathBuf::from(path.trim())))
        })
        .collect();
    roots.sort_by_key(|(n, _, _)| *n);
    roots.into_iter().map(|(_, label, path)| (label, path)).collect()
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
        assert!(config.font_family.is_none());
        assert!(config.indent_width.is_none());
        assert!(config.tab_width.is_none());
        assert!(config.completion_symbols_file.is_none());
        assert!(config.mib_roots.is_empty());
        assert!(config.mib_telecommand_template.is_none());
        assert!(config.mib_telecommand_argument_template.is_none());
        assert!(config.mib_telecommand_argument_separator.is_none());
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
    fn all_five_fields_round_trip_through_save_and_load() {
        let path = temp_path("round_trip");
        let mut config = Config::load_or_default(path.clone());
        config.theme = Some("TempleOS".to_string());
        config.font_size = Some(18.0);
        config.font_family = Some("Fira Code".to_string());
        config.indent_width = Some(2);
        config.tab_width = Some(4);
        config.completion_symbols_file = Some(PathBuf::from("/home/thomas/tcl-symbols.txt"));
        config.save().unwrap();

        let reloaded = Config::load(path.clone()).unwrap();
        assert_eq!(reloaded.theme, Some("TempleOS".to_string()));
        assert_eq!(reloaded.font_size, Some(18.0));
        assert_eq!(reloaded.font_family, Some("Fira Code".to_string()));
        assert_eq!(reloaded.indent_width, Some(2));
        assert_eq!(reloaded.tab_width, Some(4));
        assert_eq!(reloaded.completion_symbols_file, Some(PathBuf::from("/home/thomas/tcl-symbols.txt")));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn mib_roots_and_templates_round_trip_through_save_and_load() {
        let path = temp_path("mib_round_trip");
        let mut config = Config::load_or_default(path.clone());
        config.mib_roots = vec![
            ("MIB-A".to_string(), PathBuf::from("/data/mib-a")),
            ("MIB-B".to_string(), PathBuf::from("/data/mib-b")),
        ];
        config.mib_telecommand_template = Some("TC {mnemo}".to_string());
        config.mib_telecommand_argument_template = Some("{name}:{value}".to_string());
        config.mib_telecommand_argument_separator = Some(";".to_string());
        config.save().unwrap();

        let reloaded = Config::load(path.clone()).unwrap();
        assert_eq!(
            reloaded.mib_roots,
            vec![("MIB-A".to_string(), PathBuf::from("/data/mib-a")), ("MIB-B".to_string(), PathBuf::from("/data/mib-b"))]
        );
        assert_eq!(reloaded.mib_telecommand_template, Some("TC {mnemo}".to_string()));
        assert_eq!(reloaded.mib_telecommand_argument_template, Some("{name}:{value}".to_string()));
        assert_eq!(reloaded.mib_telecommand_argument_separator, Some(";".to_string()));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn a_whitespace_only_argument_separator_round_trips_through_save_and_load() {
        // The specific gap `ini::quote_if_needed`/`ini::parse`'s quote
        // handling exists to close: a separator that's pure whitespace
        // (a single space, here) previously trimmed away to nothing on
        // every save/load round trip -- there was no way to configure
        // one at all.
        let path = temp_path("whitespace_separator_round_trip");
        let mut config = Config::load_or_default(path.clone());
        config.mib_telecommand_argument_separator = Some(" ".to_string());
        config.save().unwrap();

        let contents = std::fs::read_to_string(&path).unwrap();
        assert!(
            contents.contains("telecommand_argument_separator = \" \""),
            "expected the written value to be quoted, got:\n{contents}"
        );

        let reloaded = Config::load(path.clone()).unwrap();
        assert_eq!(reloaded.mib_telecommand_argument_separator, Some(" ".to_string()));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn mib_roots_survive_a_save_triggered_by_an_unrelated_field_change() {
        // Regression guard: `save` regenerates every section from struct
        // state on every call, so a hand-authored `[mib]` section must
        // still be in the in-memory `Config` (loaded once at startup) or
        // an unrelated theme-cycle save would silently wipe it from disk.
        let path = temp_path("mib_survives_unrelated_save");
        std::fs::write(&path, "[mib]\nroot1 = MIB-A|/data/mib-a\n").unwrap();
        let mut config = Config::load(path.clone()).unwrap();
        config.theme = Some("Nord".to_string()); // unrelated change
        config.save().unwrap();

        let reloaded = Config::load(path.clone()).unwrap();
        assert_eq!(reloaded.mib_roots, vec![("MIB-A".to_string(), PathBuf::from("/data/mib-a"))]);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn mib_roots_are_ordered_by_numeric_ordinal_not_key_string() {
        let path = temp_path("mib_root_order");
        // Deliberately written out of string order (root10 sorts before
        // root2 as a plain string) to prove numeric ordering is used.
        std::fs::write(&path, "[mib]\nroot2 = SECOND|/b\nroot10 = TENTH|/j\nroot1 = FIRST|/a\n").unwrap();

        let config = Config::load(path.clone()).unwrap();

        assert_eq!(
            config.mib_roots,
            vec![
                ("FIRST".to_string(), PathBuf::from("/a")),
                ("SECOND".to_string(), PathBuf::from("/b")),
                ("TENTH".to_string(), PathBuf::from("/j")),
            ]
        );
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn a_mib_root_entry_with_no_separator_is_skipped_not_an_error() {
        let path = temp_path("mib_root_bad_entry");
        std::fs::write(&path, "[mib]\nroot1 = MIB-A|/data/a\nroot2 = no-separator-here\nroot3 = MIB-C|/data/c\n").unwrap();

        let config = Config::load(path.clone()).unwrap();

        assert_eq!(
            config.mib_roots,
            vec![("MIB-A".to_string(), PathBuf::from("/data/a")), ("MIB-C".to_string(), PathBuf::from("/data/c"))]
        );
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
