//! Native, data-only snippets. All public ranges use Unicode character offsets,
//! matching `fenix-core`; no snippet can run shell commands or editor code.
mod session;
mod template;

pub use session::{Edit, Session};
pub use template::{Context, Rendered, Template};

use std::path::{Path, PathBuf};

/// Where a snippet comes from. A closer layer wins on the same trigger:
/// a project's over yours, yours over the built-in ones.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Source {
    BuiltIn,
    User,
    Project,
}

impl Source {
    pub fn label(self) -> &'static str {
        match self {
            Source::BuiltIn => "built-in",
            Source::User => "yours",
            Source::Project => "project",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Snippet {
    pub name: String,
    pub trigger: String,
    pub scopes: Vec<String>,
    pub template: Template,
    pub source: Source,
    /// Its file; `None` for a built-in one.
    pub file: Option<PathBuf>,
    /// The file's text, header and all.
    pub text: String,
}

/// The folder a snippet for `scopes` goes in: its first language, or
/// `all` for one that works everywhere.
pub fn folder_for(scopes: &[String]) -> String {
    match scopes.first().map(String::as_str) {
        None | Some("*") => "all".to_string(),
        Some(scope) => scope.to_string(),
    }
}

/// Text as a snippet's body that inserts exactly that text: `$`, `}`
/// and `\` escaped.
pub fn escape_body(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if matches!(c, '$' | '}' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// A new snippet file's text: the header, then `body`.
pub fn file_text(name: &str, trigger: &str, scopes: &[String], body: &str) -> String {
    let scope = if scopes.is_empty() { "*".to_string() } else { scopes.join(", ") };
    let mut body = body.to_string();
    // The cursor ends up where the text ends. Nothing after `$0`: a
    // newline there would be inserted too.
    if !body.contains("$0") {
        body.push_str("$0");
    }
    format!("# name: {name}\n# key: {trigger}\n# scope: {scope}\n# --\n{body}")
}

impl Snippet {
    /// Yas-like metadata followed by `# --` and a literal multiline body.
    pub fn parse(source: &str) -> Result<Self, String> {
        let normalized = source.replace("\r\n", "\n");
        let normalized = normalized.trim_start_matches('\u{feff}');
        let mut name = None;
        let mut trigger = None;
        let mut scopes = vec!["*".to_string()];
        let mut offset = 0;
        let mut body = None;
        for line in normalized.split_inclusive('\n') {
            offset += line.len();
            if line.trim() == "# --" {
                body = Some(&normalized[offset..]);
                break;
            }
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if let Some(value) = line.strip_prefix("# name:") {
                name = Some(value.trim().to_string());
            } else if let Some(value) = line.strip_prefix("# key:") {
                trigger = Some(value.trim().to_string());
            } else if let Some(value) = line.strip_prefix("# scope:") {
                scopes = value.split(',').map(|s| s.trim().to_lowercase()).collect();
            } else {
                return Err(format!("unknown metadata: {line}"));
            }
        }
        let trigger = trigger.ok_or("missing # key:")?;
        if trigger.is_empty() || trigger.chars().any(char::is_whitespace) {
            return Err("trigger must be nonempty and contain no whitespace".into());
        }
        if scopes.iter().any(String::is_empty) {
            return Err("empty scope".into());
        }
        Ok(Self {
            name: name.unwrap_or_else(|| trigger.clone()),
            trigger,
            scopes,
            template: Template::parse(body.ok_or("missing # -- separator")?)?,
            source: Source::BuiltIn,
            file: None,
            text: source.to_string(),
        })
    }
}

#[derive(Default)]
pub struct Catalog {
    pub snippets: Vec<Snippet>,
    pub errors: Vec<String>,
}

impl Catalog {
    /// One effective definition per trigger, using the same precedence as Tab.
    pub fn available(&self, scope: &str) -> Vec<&Snippet> {
        let mut triggers: Vec<_> = self.snippets.iter().map(|s| s.trigger.as_str()).collect();
        triggers.sort_unstable();
        triggers.dedup();
        triggers
            .into_iter()
            .filter_map(|trigger| self.matching(trigger, scope))
            .collect()
    }
    /// The built-in snippets, then the user's in `directory`: a user file
    /// overrides a bundled entry with the same scope and key. Bad files
    /// are reported individually; healthy snippets remain usable.
    pub fn load(directory: &Path) -> Self {
        Self::layers(true, Some(directory), None)
    }

    /// Every layer: the built-in snippets (unless `builtin` is off), the
    /// user's in `user`, the project's in `project`. Each folder holds a
    /// folder per language (`tcl/proc.snippet`), and loose files too.
    pub fn layers(builtin: bool, user: Option<&Path>, project: Option<&Path>) -> Self {
        let mut catalog = if builtin { Self::bundled() } else { Self::default() };
        if let Some(dir) = user {
            catalog.read_layer(dir, Source::User);
        }
        if let Some(dir) = project {
            catalog.read_layer(dir, Source::Project);
        }
        catalog
    }

    fn read_layer(&mut self, directory: &Path, source: Source) {
        let mut paths = Vec::new();
        let mut folders = vec![directory.to_path_buf()];
        let mut depth = 0;
        while !folders.is_empty() && depth < 2 {
            let mut next = Vec::new();
            for folder in folders {
                let entries = match std::fs::read_dir(&folder) {
                    Ok(entries) => entries,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                    Err(e) => {
                        self.errors.push(format!("{}: {e}", folder.display()));
                        continue;
                    }
                };
                for entry in entries {
                    match entry {
                        Ok(entry) if entry.path().is_dir() => next.push(entry.path()),
                        Ok(entry) if entry.path().extension().is_some_and(|e| e == "snippet") => paths.push(entry.path()),
                        Ok(_) => {}
                        Err(e) => self.errors.push(e.to_string()),
                    }
                }
            }
            folders = next;
            depth += 1;
        }
        paths.sort();
        for path in paths {
            let parsed = (|| {
                if std::fs::metadata(&path).map_err(|e| e.to_string())?.len() > 1_048_576 {
                    return Err("snippet exceeds 1 MiB".into());
                }
                Snippet::parse(&std::fs::read_to_string(&path).map_err(|e| e.to_string())?)
            })();
            match parsed {
                Ok(mut snippet) => {
                    snippet.source = source;
                    snippet.file = Some(path);
                    self.snippets.push(snippet);
                }
                Err(e) => self.errors.push(format!("{}: {e}", path.display())),
            }
        }
    }

    pub fn bundled() -> Self {
        Self {
            snippets: [
                include_str!("../examples/header.snippet"),
                include_str!("../examples/section.snippet"),
                include_str!("../examples/proc.snippet"),
                include_str!("../examples/header-c.snippet"),
                include_str!("../examples/section-c.snippet"),
            ]
            .into_iter()
            .map(|s| Snippet::parse(s).expect("valid bundled snippet"))
            .collect(),
            errors: Vec::new(),
        }
    }

    /// Exact suffix, at a word boundary. Language-specific beats global,
    /// then longest trigger, then the last (user) definition wins.
    pub fn matching(&self, before_cursor: &str, scope: &str) -> Option<&Snippet> {
        self.snippets
            .iter()
            .enumerate()
            .filter_map(|(index, snippet)| {
                let specific = snippet.scopes.iter().any(|s| s == scope);
                if !specific && !snippet.scopes.iter().any(|s| s == "*") {
                    return None;
                }
                let prefix = before_cursor.strip_suffix(&snippet.trigger)?;
                if prefix
                    .chars()
                    .next_back()
                    .is_some_and(|c| c.is_alphanumeric() || c == '_')
                {
                    return None;
                }
                Some(((specific, snippet.trigger.len(), index), snippet))
            })
            .max_by_key(|(key, _)| *key)
            .map(|(_, snippet)| snippet)
    }
}
