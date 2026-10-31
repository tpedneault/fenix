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
//! One can bring a language Fenix doesn't have: a tree-sitter
//! grammar compiled to WebAssembly, in the extension's own folder, and its
//! queries, `highlights.scm`, `folds.scm` and `indents.scm`, from
//! `queries/<language>/` there (or the folder `queries` names):
//!
//! ```toml
//! language = "kotlin"
//! grammar = "grammars/tree-sitter-kotlin.wasm"
//! extensions = ["kt", "kts"]
//! ```
//!
//! What it exports is `tree_sitter_<symbol>`: `symbol` says it, else it's
//! the file's name, `tree-sitter-` and `.wasm` taken off, `-` as `_`.
//! Paths are under the folder that holds `languages/` -- the extension's,
//! or your settings folder. `injections.scm` isn't read: Fenix doesn't
//! highlight one language inside another, but Markdown's own.
//!
//! `comment` is what starts a line comment in it, for `gcc`: a
//! grammar's language has none of Fenix's own (`comment = "//"`).
//!
//! The installed extensions' files are read first, then yours: a later
//! one wins a file type, a server, a comment or a grammar. Your `lsp.servers`
//! setting wins over every language file.

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
    grammar: Option<String>,
    symbol: Option<String>,
    queries: Option<String>,
    comment: Option<String>,
}

/// One language file, read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Definition {
    pub language: LanguageId,
    pub extensions: Vec<String>,
    pub filenames: Vec<String>,
    pub server: Option<String>,
    /// What starts a line comment in it.
    pub comment: Option<String>,
}

/// A language file's text, read and checked.
#[cfg(test)]
fn parse(text: &str) -> Result<Definition, String> {
    parse_with(text, &[])
}

/// `parse`, knowing the grammars the language files bring: `grammars[i]`
/// is `LanguageId::Grammar(i)`.
fn parse_with(text: &str, grammars: &[String]) -> Result<Definition, String> {
    let file: File = toml::from_str(text).map_err(|e| e.message().to_string())?;
    let language = LanguageId::ALL.into_iter().find(|l| l.name().eq_ignore_ascii_case(file.language.trim())).or_else(|| grammars.iter().position(|g| g.eq_ignore_ascii_case(file.language.trim())).map(|i| LanguageId::Grammar(i as u16))).ok_or_else(|| {
        let names: Vec<String> = LanguageId::ALL.iter().map(|l| l.name()).chain(grammars.iter().cloned()).collect();
        format!("no language \"{}\" here -- one of {}", file.language, names.join(", "))
    })?;
    let extensions: Vec<String> = file.extensions.iter().map(|e| e.trim().trim_start_matches('.').to_ascii_lowercase()).filter(|e| !e.is_empty()).collect();
    let filenames: Vec<String> = file.filenames.iter().map(|f| f.trim().to_ascii_lowercase()).filter(|f| !f.is_empty()).collect();
    if let Some(bad) = filenames.iter().find(|f| f.contains(['/', '\\'])) {
        return Err(format!("\"{bad}\" is a path -- filenames are names alone"));
    }
    let server = file.server.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    if file.grammar.is_some() && !language.is_grammar() {
        return Err(format!("{} is Fenix's own: a grammar is for a language it doesn't have", language.name()));
    }
    let comment = file.comment.map(|c| c.trim().to_string());
    if comment.as_deref().is_some_and(|c| c.is_empty() || c.contains(char::is_whitespace)) {
        return Err("comment is what starts a line comment, `//` or `#`".into());
    }
    if extensions.is_empty() && filenames.is_empty() && server.is_none() && file.grammar.is_none() && comment.is_none() {
        return Err("it gives no extensions, filenames or server".into());
    }
    Ok(Definition { language, extensions, filenames, server, comment })
}

/// The grammar a language file brings, if it does: read from under `root`
/// (the folder that holds `languages/`), its queries with it.
fn grammar_of(text: &str, root: &Path) -> Result<Option<fenix_syntax::Grammar>, String> {
    let file: File = toml::from_str(text).map_err(|e| e.message().to_string())?;
    let Some(grammar) = file.grammar.as_deref() else {
        if file.symbol.is_some() || file.queries.is_some() {
            return Err("`symbol` and `queries` go with a `grammar`".into());
        }
        return Ok(None);
    };
    let name = file.language.trim().to_ascii_lowercase();
    if LanguageId::ALL.iter().any(|l| l.name() == name) {
        return Err(format!("{name} is Fenix's own: a grammar is for a language it doesn't have"));
    }
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '+' | '#')) {
        return Err(format!("\"{name}\" isn't a language's name: letters, digits, - and _"));
    }
    let wasm = read_under(root, grammar, fenix_syntax::MAX_GRAMMAR)?;
    if !wasm.starts_with(b"\0asm") {
        return Err(format!("{grammar} isn't WebAssembly"));
    }
    let symbol = match file.symbol {
        Some(symbol) => symbol.trim().to_string(),
        None => {
            let stem = Path::new(grammar).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            stem.strip_prefix("tree-sitter-").unwrap_or(&stem).replace('-', "_")
        }
    };
    if symbol.is_empty() || !symbol.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(format!("\"{symbol}\" isn't what a grammar exports (tree_sitter_<symbol>): say it with `symbol`"));
    }
    let queries = file.queries.unwrap_or_else(|| format!("queries/{name}"));
    let query = |which: &str| -> Result<String, String> {
        let path = format!("{}/{which}.scm", queries.trim_end_matches('/'));
        match read_under(root, &path, MAX_QUERY) {
            Ok(bytes) => String::from_utf8(bytes).map_err(|_| format!("{path} isn't UTF-8")),
            Err(_) if !root.join(&path).exists() => Ok(String::new()),
            Err(e) => Err(e),
        }
    };
    Ok(Some(fenix_syntax::Grammar { name, symbol, wasm, highlights: query("highlights")?, folds: query("folds")?, indents: query("indents")? }))
}

/// The most a query file may be.
const MAX_QUERY: u64 = 1 << 20;

/// File `rel` under `root`, no bigger than `max`: relative, and not by way
/// of `..` or a link out of it.
fn read_under(root: &Path, rel: &str, max: u64) -> Result<Vec<u8>, String> {
    let parts = Path::new(rel);
    if rel.trim().is_empty() || parts.is_absolute() || parts.components().any(|c| !matches!(c, std::path::Component::Normal(_) | std::path::Component::CurDir)) {
        return Err(format!("{rel}: a file in its own folder, as `grammars/x.wasm`"));
    }
    let path = root.join(parts);
    let (Ok(real_root), Ok(real)) = (root.canonicalize(), path.canonicalize()) else { return Err(format!("{rel}: no such file")) };
    if !real.starts_with(&real_root) {
        return Err(format!("{rel}: not in its own folder"));
    }
    let size = std::fs::metadata(&real).map_err(|e| format!("{rel}: {e}"))?.len();
    if size > max {
        return Err(format!("{rel}: at most {} MiB", max >> 20));
    }
    std::fs::read(&real).map_err(|e| format!("{rel}: {e}"))
}

/// What the language files add up to.
#[derive(Debug, Default)]
struct Loaded {
    file_types: FileTypes,
    /// The grammars they bring: `LanguageId::Grammar(i)` is `grammars[i]`.
    grammars: Vec<fenix_syntax::Grammar>,
    /// `(language name, command line)`, as `lsp.servers` has them.
    servers: Vec<(String, String)>,
    /// Each language's line comment, as they say it.
    comments: Vec<(LanguageId, String)>,
    problems: Vec<String>,
}

/// Reads every `.toml` in `dirs`, in order (`(shown as, folder)`); a later
/// file wins what an earlier one set.
fn load(dirs: &[(String, PathBuf)]) -> Loaded {
    let mut loaded = Loaded::default();
    let mut servers: BTreeMap<String, String> = BTreeMap::new();
    let mut texts = Vec::new();
    for (shown, dir) in dirs {
        for path in toml_files(dir) {
            let file = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            match std::fs::read_to_string(&path) {
                Ok(text) => texts.push((format!("{shown}{file}"), dir.parent().map(Path::to_path_buf).unwrap_or_default(), text)),
                Err(e) => loaded.problems.push(format!("{shown}{file}: {e}")),
            }
        }
    }
    // The grammars first, so every file may name their languages; a later
    // one of the same name takes its place.
    let mut bad = Vec::new();
    for (i, (shown, root, text)) in texts.iter().enumerate() {
        match grammar_of(text, root) {
            Ok(Some(grammar)) => match loaded.grammars.iter_mut().find(|g| g.name == grammar.name) {
                Some(kept) => *kept = grammar,
                None => loaded.grammars.push(grammar),
            },
            Ok(None) => {}
            Err(e) => {
                loaded.problems.push(format!("{shown}: {e}"));
                bad.push(i);
            }
        }
    }
    let names: Vec<String> = loaded.grammars.iter().map(|g| g.name.clone()).collect();
    for (i, (shown, _, text)) in texts.iter().enumerate() {
        if bad.contains(&i) {
            continue;
        }
        match parse_with(text, &names) {
            Ok(def) => {
                for ext in def.extensions {
                    loaded.file_types.extensions.insert(ext, def.language);
                }
                for name in def.filenames {
                    // `.env.*`: a name by pattern.
                    if name.contains(['*', '?']) {
                        loaded.file_types.patterns.push((name, def.language));
                    } else {
                        loaded.file_types.filenames.insert(name, def.language);
                    }
                }
                if let Some(comment) = def.comment {
                    loaded.comments.retain(|(l, _)| *l != def.language);
                    loaded.comments.push((def.language, comment));
                }
                if let Some(server) = def.server {
                    let name = match def.language {
                        LanguageId::Grammar(g) => names[g as usize].clone(),
                        language => language.name(),
                    };
                    servers.insert(name, server);
                }
            }
            Err(e) => loaded.problems.push(format!("{shown}: {e}")),
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
    fenix_syntax::set_grammars(loaded.grammars.clone());
    fenix_syntax::set_file_types(loaded.file_types.clone());
}

/// The language files' servers, `(language name, command line)`.
pub fn servers() -> &'static [(String, String)] {
    LOADED.get().map(|l| l.servers.as_slice()).unwrap_or(&[])
}

/// What starts a line comment in `language`, as a language file says it
///.
pub fn comment(language: LanguageId) -> Option<&'static str> {
    LOADED.get()?.comments.iter().find(|(l, _)| *l == language).map(|(_, c)| c.as_str())
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
        assert_eq!(def, Definition { language: LanguageId::Sql, extensions: vec!["pks".into(), "pkb".into()], filenames: vec!["schema.ddl".into()], server: Some("sqls".into()), comment: None });
        // a line comment, alone or not.
        assert_eq!(parse("language = \"batch\"\ncomment = \"REM\"\n").unwrap().comment.as_deref(), Some("REM"));
        assert!(parse("language = \"sql\"\ncomment = \"\"\n").unwrap_err().contains("line comment"));
        assert!(parse("language = \"cobol\"\nextensions = [\"cbl\"]\n").unwrap_err().contains("no language \"cobol\" here -- one of rust, toml"));
        assert!(parse("language = \"sql\"\n").unwrap_err().contains("no extensions, filenames or server"));
        assert!(parse("language = \"sql\"\nfilenames = [\"db/schema\"]\n").unwrap_err().contains("is a path"));
        assert!(parse("language = \"sql\"\ncolour = \"x\"\n").is_err(), "an unknown key is a mistake, said");
        assert!(parse("language = \"sql\"\ngrammar = \"x.wasm\"\n").unwrap_err().contains("Fenix's own"));
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

    /// a file name with `*` or `?` is a pattern (`.env.*`).
    #[test]
    fn a_file_name_may_be_a_pattern() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("env.toml"), "language = \"bash\"\nfilenames = [\".env\", \".ENV.*\"]\n").unwrap();
        let loaded = load(&[("languages/".into(), dir.path().to_path_buf())]);
        assert_eq!(loaded.file_types.filenames.get(".env"), Some(&LanguageId::Bash));
        assert_eq!(loaded.file_types.patterns, [(".env.*".to_string(), LanguageId::Bash)]);
    }

    /// a grammar of its own, compiled to WebAssembly, and its
    /// queries, from the extension's folder; another file may then give
    /// its language more file types.
    #[test]
    fn a_language_file_brings_a_grammar_from_its_own_folder() {
        let dir = tempfile::tempdir().unwrap();
        let ext = dir.path().join("ext");
        std::fs::create_dir_all(ext.join("languages")).unwrap();
        std::fs::create_dir_all(ext.join("grammars")).unwrap();
        std::fs::create_dir_all(ext.join("queries/jsonw")).unwrap();
        std::fs::write(ext.join("grammars/tree-sitter-json.wasm"), include_bytes!("../../fenix-syntax/tests/grammars/tree-sitter-json.wasm")).unwrap();
        std::fs::write(ext.join("queries/jsonw/highlights.scm"), "(number) @number\n").unwrap();
        std::fs::write(ext.join("queries/jsonw/folds.scm"), "(object) @fold\n").unwrap();
        std::fs::write(ext.join("languages/a.toml"), "language = \"jsonw\"\ngrammar = \"grammars/tree-sitter-json.wasm\"\nextensions = [\"jw\"]\nserver = \"jw-ls\"\n").unwrap();
        std::fs::write(ext.join("languages/b.toml"), "language = \"JSONW\"\nfilenames = [\"Jwfile\"]\ncomment = \"//\"\n").unwrap();
        std::fs::write(ext.join("languages/c.toml"), "language = \"up\"\ngrammar = \"../x.wasm\"\n").unwrap();
        std::fs::write(ext.join("languages/d.toml"), "language = \"rust\"\ngrammar = \"grammars/tree-sitter-json.wasm\"\n").unwrap();
        std::fs::write(ext.join("languages/e.toml"), "language = \"text\"\ngrammar = \"queries/jsonw/folds.scm\"\n").unwrap();
        let loaded = load(&[("ext/".into(), ext.join("languages"))]);
        assert_eq!(loaded.grammars.len(), 1, "{:?}", loaded.problems);
        let g = &loaded.grammars[0];
        assert_eq!((g.name.as_str(), g.symbol.as_str(), g.highlights.as_str(), g.folds.as_str(), g.indents.as_str()), ("jsonw", "json", "(number) @number\n", "(object) @fold\n", ""));
        assert!(fenix_syntax::load_grammar(&g.symbol, &g.wasm).is_ok());
        assert_eq!(loaded.file_types.extensions.get("jw"), Some(&LanguageId::Grammar(0)));
        assert_eq!(loaded.file_types.filenames.get("jwfile"), Some(&LanguageId::Grammar(0)));
        assert_eq!(loaded.servers, [("jsonw".to_string(), "jw-ls".to_string())]);
        assert_eq!(loaded.comments, [(LanguageId::Grammar(0), "//".to_string())], "gcc's comment for it");
        let problems = loaded.problems.join("\n");
        assert!(problems.contains("ext/c.toml: ../x.wasm: a file in its own folder"), "{problems}");
        assert!(problems.contains("ext/d.toml: rust is Fenix's own"), "{problems}");
        assert!(problems.contains("ext/e.toml: queries/jsonw/folds.scm isn't WebAssembly"), "{problems}");
        assert_eq!(loaded.problems.len(), 3, "{problems}");
    }

    /// A grammar by way of a link out of the extension's folder isn't
    /// read.
    #[cfg(unix)]
    #[test]
    fn a_grammar_isnt_read_through_a_link_out() {
        let dir = tempfile::tempdir().unwrap();
        let ext = dir.path().join("ext");
        std::fs::create_dir_all(ext.join("languages")).unwrap();
        std::fs::write(dir.path().join("outside.wasm"), include_bytes!("../../fenix-syntax/tests/grammars/tree-sitter-json.wasm")).unwrap();
        std::os::unix::fs::symlink("../outside.wasm", ext.join("json.wasm")).unwrap();
        std::fs::write(ext.join("languages/a.toml"), "language = \"jsonw\"\ngrammar = \"json.wasm\"\n").unwrap();
        let loaded = load(&[("ext/".into(), ext.join("languages"))]);
        assert!(loaded.grammars.is_empty());
        assert!(loaded.problems[0].contains("not in its own folder"), "{:?}", loaded.problems);
    }
}
