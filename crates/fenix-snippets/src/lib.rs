//! Native, data-only snippets. All public ranges use Unicode character offsets,
//! matching `fenix-core`; no snippet can run shell commands or editor code.
mod session;
mod template;

pub use session::{Edit, Session};
pub use template::{Context, Rendered, Template};

use std::path::Path;

#[derive(Clone, Debug)]
pub struct Snippet {
    pub name: String,
    pub trigger: String,
    pub scopes: Vec<String>,
    pub template: Template,
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
    /// Sorted user files override bundled entries with the same scope/key.
    /// Bad files are reported individually; healthy snippets remain usable.
    pub fn load(directory: &Path) -> Self {
        let mut catalog = Self::bundled();
        let entries = match std::fs::read_dir(directory) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return catalog,
            Err(e) => {
                catalog.errors.push(format!("{}: {e}", directory.display()));
                return catalog;
            }
        };
        let mut paths = Vec::new();
        for entry in entries {
            match entry {
                Ok(entry) if entry.path().extension().is_some_and(|e| e == "snippet") => {
                    paths.push(entry.path())
                }
                Ok(_) => {}
                Err(e) => catalog.errors.push(e.to_string()),
            }
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
                Ok(snippet) => catalog.snippets.push(snippet),
                Err(e) => catalog.errors.push(format!("{}: {e}", path.display())),
            }
        }
        catalog
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
