//! A project's own settings: `.fenix/settings.toml` in its root, a layer
//! on top of yours. Only the settings the schema marks as per-project
//! can be set there (indent width, base branch, reviewers, language
//! servers...); anything else in it is ignored and pointed out. Saved
//! the same way as yours: the file is read again and only the lines of
//! what changed are written, so it stays reviewable in the repository.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};

use crate::schema::{self, Value};
use crate::{line_of, lookup, store, Kind, Problem};

pub struct ProjectSettings {
    path: PathBuf,
    values: HashMap<&'static str, Value>,
    /// Each value as the file last had it, so a save writes only what
    /// changed.
    baseline: HashMap<&'static str, Value>,
    broken: bool,
    pub problems: Vec<Problem>,
}

const HEADER: &str = "# This project's own Fenix settings, used instead of each person's for\n# this project. SPC p , edits them. Meant to be committed.\n\n";

impl ProjectSettings {
    /// `.fenix/settings.toml` under `root`.
    pub fn path_for(root: &Path) -> PathBuf {
        root.join(".fenix").join("settings.toml")
    }

    /// The project at `root`'s settings: none when it has no file.
    pub fn load(root: &Path) -> Self {
        let path = Self::path_for(root);
        let mut settings = ProjectSettings { path, values: HashMap::new(), baseline: HashMap::new(), broken: false, problems: Vec::new() };
        let text = match std::fs::read_to_string(&settings.path) {
            Ok(text) => text,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return settings,
            Err(e) => {
                settings.broken = true;
                settings.problems.push(Problem { key: None, line: None, message: e.to_string() });
                return settings;
            }
        };
        let doc = match text.parse::<toml_edit::ImDocument<String>>() {
            Ok(doc) => doc,
            Err(e) => {
                settings.broken = true;
                settings.problems.push(Problem { key: None, line: e.span().map(|s| line_of(&text, s.start)), message: e.message().trim().to_string() });
                return settings;
            }
        };
        let doc_mut = doc.clone().into_mut();
        for s in schema::settings() {
            let Some(item) = lookup(&doc_mut, s.key) else { continue };
            let line = {
                let mut item = doc.as_item();
                for part in s.key.split('.') {
                    item = match item.as_table_like().and_then(|t| t.get(part)) {
                        Some(i) => i,
                        None => break,
                    };
                }
                item.span().map(|span| line_of(&text, span.start))
            };
            if !s.project || matches!(s.kind, Kind::Secret(_)) {
                settings.problems.push(Problem { key: Some(s.key.into()), line, message: "not something a project can set -- ignored".into() });
                continue;
            }
            match s.kind.read(item).and_then(|v| s.kind.check(&v).map(|_| v)) {
                Ok(v) => {
                    settings.values.insert(s.key, v);
                }
                Err(message) => settings.problems.push(Problem { key: Some(s.key.into()), line, message }),
            }
        }
        settings.problems.sort_by_key(|p| p.line.unwrap_or(usize::MAX));
        settings.baseline = settings.values.clone();
        settings
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Whether it sets anything.
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    pub fn values(&self) -> HashMap<&'static str, Value> {
        self.values.clone()
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        self.values.get(key)
    }

    /// Sets (or, with `None`, stops setting) `key` for this project.
    pub fn set(&mut self, key: &str, value: Option<Value>) -> Result<(), String> {
        let s = schema::setting(key).ok_or_else(|| format!("no setting called {key}"))?;
        if !s.project {
            return Err(format!("{} can't be set per project", s.label));
        }
        match value {
            Some(v) => {
                s.kind.check(&v)?;
                self.values.insert(s.key, v);
            }
            None => {
                self.values.remove(s.key);
            }
        }
        Ok(())
    }

    /// Writes what changed. A project with nothing set and no file yet
    /// gets no file.
    pub fn save(&mut self) -> io::Result<()> {
        if self.broken {
            return Err(io::Error::new(io::ErrorKind::InvalidData, format!("{} can't be read, so it wasn't written over -- fix it first", self.path.display())));
        }
        let on_disk = match std::fs::read_to_string(&self.path) {
            Ok(text) => Some(text),
            Err(e) if e.kind() == io::ErrorKind::NotFound => None,
            Err(e) => return Err(e),
        };
        if on_disk.is_none() && self.values.is_empty() {
            return Ok(());
        }
        let mut doc = match on_disk.as_deref().map(str::parse::<toml_edit::DocumentMut>) {
            Some(Ok(doc)) => doc,
            Some(Err(_)) => return Err(io::Error::new(io::ErrorKind::InvalidData, format!("{} was changed into something that can't be read, so it wasn't written over", self.path.display()))),
            None => {
                let mut doc = toml_edit::DocumentMut::new();
                doc.decor_mut().set_prefix(HEADER);
                doc
            }
        };
        for s in schema::settings().iter().filter(|s| s.project) {
            let now = self.values.get(s.key);
            if self.baseline.get(s.key) == now {
                continue;
            }
            store(&mut doc, s.key, now.map(|v| s.kind.write(v)));
        }
        let text = doc.to_string();
        if on_disk.as_deref() != Some(text.as_str()) {
            if let Some(parent) = self.path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            fenix_storage::write(&self.path, text.as_bytes())?;
        }
        self.baseline = self.values.clone();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("fenix-project-settings-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_project_sets_only_what_the_schema_allows_and_says_what_it_ignored() {
        let root = project("allowed");
        std::fs::create_dir_all(root.join(".fenix")).unwrap();
        std::fs::write(ProjectSettings::path_for(&root), "[editor]\nindent_width = 2\ntheme = \"Nord\"\n[git]\nreviewers = [\"alex\"]\n").unwrap();
        let settings = ProjectSettings::load(&root);
        assert_eq!(settings.get("editor.indent_width"), Some(&Value::Int(2)));
        assert_eq!(settings.get("git.reviewers"), Some(&Value::List(vec!["alex".into()])));
        assert_eq!(settings.get("editor.theme"), None);
        assert_eq!(settings.problems.len(), 1);
        assert!(settings.problems[0].to_string().starts_with("settings.toml:3 -- editor.theme: not something a project can set"), "{:?}", settings.problems);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn saving_writes_only_what_changed_and_nothing_for_a_project_that_sets_nothing() {
        let root = project("save");
        let mut settings = ProjectSettings::load(&root);
        settings.save().unwrap();
        assert!(!ProjectSettings::path_for(&root).exists(), "no file for nothing");
        assert!(settings.set("editor.theme", Some(Value::Text("Nord".into()))).is_err());
        settings.set("editor.tab_width", Some(Value::Int(4))).unwrap();
        settings.save().unwrap();
        let path = ProjectSettings::path_for(&root);
        let text = std::fs::read_to_string(&path).unwrap() + "# a note from the team\n";
        std::fs::write(&path, &text).unwrap();
        let mut settings = ProjectSettings::load(&root);
        settings.set("git.base_branch", Some(Value::Text("develop".into()))).unwrap();
        settings.save().unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("tab_width = 4") && text.contains("base_branch = \"develop\"") && text.contains("# a note from the team"), "{text}");
        settings.set("editor.tab_width", None).unwrap();
        settings.save().unwrap();
        assert!(!std::fs::read_to_string(&path).unwrap().contains("tab_width"));
        let _ = std::fs::remove_dir_all(root);
    }
}
