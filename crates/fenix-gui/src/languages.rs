//! Language files: `languages/*.toml`, in an installed extension or in
//! your settings folder. One gives a language Fenix already highlights
//! more file types, by extension or whole file name, and can name the
//! language server to run for it:
//!
//! ```toml
//! language = "sql"
//! extensions = ["pks", "pkb"]
//! filenames = ["schema.ddl"]
//! server = "sqls"
//! ```
//!
//! A grammar of its own needs code, so that waits for code extensions.
//! The installed extensions' files are read first, then yours: a later
//! one wins a file type or a server. Your `lsp.servers` setting wins
//! over every language file.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use fenix_syntax::{FileTypes, LanguageId};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    language: String,
    #[serde(default)]
    extensions: Vec<String>,
    #[serde(default)]
    filenames: Vec<String>,
    server: Option<String>,
}

/// One language file, read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Definition {
    pub language: LanguageId,
    pub extensions: Vec<String>,
    pub filenames: Vec<String>,
    pub server: Option<String>,
}

/// A language file's text, read and checked.
pub fn parse(text: &str) -> Result<Definition, String> {
    let file: File = toml::from_str(text).map_err(|e| e.message().to_string())?;
    let language = LanguageId::from_name(&file.language).ok_or_else(|| {
        let names: Vec<String> = LanguageId::ALL.iter().map(|l| l.name()).collect();
        format!("no language \"{}\" here -- one of {}", file.language, names.join(", "))
    })?;
    let extensions: Vec<String> = file.extensions.iter().map(|e| e.trim().trim_start_matches('.').to_ascii_lowercase()).filter(|e| !e.is_empty()).collect();
    let filenames: Vec<String> = file.filenames.iter().map(|f| f.trim().to_ascii_lowercase()).filter(|f| !f.is_empty()).collect();
    if let Some(bad) = filenames.iter().find(|f| f.contains(['/', '\\'])) {
        return Err(format!("\"{bad}\" is a path -- filenames are names alone"));
    }
    let server = file.server.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    if extensions.is_empty() && filenames.is_empty() && server.is_none() {
        return Err("it gives no extensions, filenames or server".into());
    }
    Ok(Definition { language, extensions, filenames, server })
}

/// What the language files add up to.
#[derive(Debug, Default)]
struct Loaded {
    file_types: FileTypes,
    /// `(language name, command line)`, as `lsp.servers` has them.
    servers: Vec<(String, String)>,
    problems: Vec<String>,
}

/// Reads every `.toml` in `dirs`, in order (`(shown as, folder)`); a later
/// file wins what an earlier one set.
fn load(dirs: &[(String, PathBuf)]) -> Loaded {
    let mut loaded = Loaded::default();
    let mut servers: BTreeMap<String, String> = BTreeMap::new();
    for (shown, dir) in dirs {
        for path in toml_files(dir) {
            let file = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            match std::fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|text| parse(&text)) {
                Ok(def) => {
                    for ext in def.extensions {
                        loaded.file_types.extensions.insert(ext, def.language);
                    }
                    for name in def.filenames {
                        loaded.file_types.filenames.insert(name, def.language);
                    }
                    if let Some(server) = def.server {
                        servers.insert(def.language.name(), server);
                    }
                }
                Err(e) => loaded.problems.push(format!("{shown}{file}: {e}")),
            }
        }
    }
    loaded.servers = servers.into_iter().collect();
    loaded
}

fn toml_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut files: Vec<PathBuf> = entries.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("toml"))).collect();
    files.sort();
    files
}

static LOADED: std::sync::OnceLock<Loaded> = std::sync::OnceLock::new();

/// Reads the language files and settles their file types, in `main`
/// before a file is opened. Nothing in tests.
pub fn init() {
    let loaded = LOADED.get_or_init(|| {
        if cfg!(test) {
            return Loaded::default();
        }
        let mut dirs: Vec<(String, PathBuf)> = crate::installed::data_dirs("languages").into_iter().map(|d| (format!("{}/", d.display()), d)).collect();
        dirs.extend(fenix_storage::paths::languages_dir().map(|d| ("languages/".to_string(), d)));
        load(&dirs)
    });
    fenix_syntax::set_file_types(loaded.file_types.clone());
}

/// The language files' servers, `(language name, command line)`.
pub fn servers() -> &'static [(String, String)] {
    LOADED.get().map(|l| l.servers.as_slice()).unwrap_or(&[])
}

/// What's wrong with the language files, to be said once.
pub fn problems() -> &'static [String] {
    LOADED.get().map(|l| l.problems.as_slice()).unwrap_or(&[])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_language_file_is_read_and_checked() {
        let def = parse("language = \"SQL\"\nextensions = [\".PKS\", \"pkb\"]\nfilenames = [\"Schema.ddl\"]\nserver = \"sqls\"\n").unwrap();
        assert_eq!(def, Definition { language: LanguageId::Sql, extensions: vec!["pks".into(), "pkb".into()], filenames: vec!["schema.ddl".into()], server: Some("sqls".into()) });
        assert!(parse("language = \"cobol\"\nextensions = [\"cbl\"]\n").unwrap_err().contains("no language \"cobol\" here -- one of rust, toml"));
        assert!(parse("language = \"sql\"\n").unwrap_err().contains("no extensions, filenames or server"));
        assert!(parse("language = \"sql\"\nfilenames = [\"db/schema\"]\n").unwrap_err().contains("is a path"));
        assert!(parse("language = \"sql\"\ngrammar = \"x\"\n").is_err(), "an unknown key is a mistake, said");
    }

    /// The extensions' files, then yours: the later wins a file type or
    /// a server, and a file that doesn't read is said, the rest kept.
    #[test]
    fn later_files_win_and_bad_ones_are_said() {
        let dir = tempfile::tempdir().unwrap();
        let (ext, yours) = (dir.path().join("ext"), dir.path().join("yours"));
        std::fs::create_dir_all(&ext).unwrap();
        std::fs::create_dir_all(&yours).unwrap();
        std::fs::write(ext.join("plsql.toml"), "language = \"sql\"\nextensions = [\"pks\", \"inc\"]\nserver = \"plsql-ls --stdio\"\n").unwrap();
        std::fs::write(yours.join("inc.toml"), "language = \"cpp\"\nextensions = [\"inc\"]\n").unwrap();
        std::fs::write(yours.join("sql.toml"), "language = \"sql\"\nserver = \"sqls\"\n").unwrap();
        std::fs::write(yours.join("broken.toml"), "language = 3\n").unwrap();
        let loaded = load(&[("ext/".into(), ext), ("languages/".into(), yours)]);
        assert_eq!(loaded.file_types.extensions.get("pks"), Some(&LanguageId::Sql));
        assert_eq!(loaded.file_types.extensions.get("inc"), Some(&LanguageId::Cpp), "yours wins");
        assert_eq!(loaded.servers, [("sql".to_string(), "sqls".to_string())]);
        assert_eq!(loaded.problems.len(), 1);
        assert!(loaded.problems[0].starts_with("languages/broken.toml: "), "{:?}", loaded.problems);
    }
}
