//! A project's own file, `.fenix/settings.toml`, for the crates that
//! keep a key of their own in it: the project's kind and Jira key
//! (`[project]`), a sketch's serial monitor speed (`[monitor]`). The
//! settings a project may override (`[editor]`, `[git]`) are read and
//! written by `fenix_config::ProjectSettings`, the same way: a key is
//! changed in place, and every other line -- comments included -- stays
//! as it was.

use std::io;
use std::path::{Path, PathBuf};

use toml_edit::{value, Array, DocumentMut, Item};

/// What goes at the top of a project's file when Fenix makes it.
pub const HEADER: &str = "# This project's own Fenix settings, used instead of each person's for\n# this project. SPC p , edits them. Meant to be committed.\n\n";

/// A value for one key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Text(String),
    Int(i64),
    List(Vec<String>),
}

/// `.fenix/settings.toml` under `root`.
pub fn path(root: &Path) -> PathBuf {
    root.join(".fenix").join("settings.toml")
}

fn read(root: &Path) -> Option<DocumentMut> {
    std::fs::read_to_string(path(root)).ok()?.parse().ok()
}

fn item<'a>(doc: &'a DocumentMut, key: &str) -> Option<&'a Item> {
    let mut item = doc.as_item();
    for part in key.split('.') {
        item = item.as_table_like()?.get(part)?;
    }
    Some(item)
}

/// `key` (`project.kind`) as text: a string as it is, a number written
/// out. `None` when the file, the key or a readable value isn't there.
pub fn get(root: &Path, key: &str) -> Option<String> {
    let doc = read(root)?;
    let item = item(&doc, key)?;
    item.as_str().map(str::to_string).or_else(|| item.as_integer().map(|n| n.to_string())).filter(|s| !s.trim().is_empty())
}

/// `key` as a list of words (`["alex", "sam"]`).
pub fn get_list(root: &Path, key: &str) -> Option<Vec<String>> {
    let doc = read(root)?;
    let list: Vec<String> = item(&doc, key)?.as_array()?.iter().filter_map(|v| v.as_str().map(str::to_string)).collect();
    (!list.is_empty()).then_some(list)
}

/// Sets `key`, or with `None` removes it, leaving the rest of the file
/// as it was. The file is made (with a header) when it's needed and
/// isn't there; a file that can't be parsed is never written over.
pub fn set(root: &Path, key: &str, new: Option<Value>) -> io::Result<()> {
    let file = path(root);
    let mut doc = match std::fs::read_to_string(&file) {
        Ok(text) => text.parse::<DocumentMut>().map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("{} can't be read, so it wasn't written over: {}", file.display(), e.message().trim())))?,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            if new.is_none() {
                return Ok(());
            }
            let mut doc = DocumentMut::new();
            doc.decor_mut().set_prefix(HEADER);
            doc
        }
        Err(e) => return Err(e),
    };
    let parts: Vec<&str> = key.split('.').collect();
    let (last, parents) = parts.split_last().expect("a key");
    let mut table: &mut dyn toml_edit::TableLike = doc.as_table_mut();
    for part in parents {
        if table.get(part).and_then(|i| i.as_table_like()).is_none() {
            if new.is_none() {
                return Ok(());
            }
            let mut t = toml_edit::Table::new();
            t.set_implicit(true);
            table.insert(part, Item::Table(t));
        }
        table = table.get_mut(part).and_then(|i| i.as_table_like_mut()).expect("just made");
    }
    match new {
        Some(Value::Text(t)) => table.insert(last, value(t)),
        Some(Value::Int(n)) => table.insert(last, value(n)),
        Some(Value::List(l)) => table.insert(last, value(l.iter().map(String::as_str).collect::<Array>())),
        None => table.remove(last),
    };
    std::fs::create_dir_all(file.parent().expect("under .fenix"))?;
    crate::write(&file, doc.to_string().as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_is_set_read_and_removed_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        assert_eq!(get(root, "project.kind"), None);
        set(root, "project.kind", None).unwrap();
        assert!(!path(root).exists(), "nothing to remove, no file made");
        set(root, "project.kind", Some(Value::Text("rust".into()))).unwrap();
        set(root, "monitor.baudrate", Some(Value::Int(115200))).unwrap();
        set(root, "git.reviewers", Some(Value::List(vec!["alex".into(), "sam".into()]))).unwrap();
        let text = std::fs::read_to_string(path(root)).unwrap() + "# the team's note\n";
        std::fs::write(path(root), &text).unwrap();
        set(root, "project.jira", Some(Value::Text("FNX".into()))).unwrap();
        assert_eq!(get(root, "project.kind").as_deref(), Some("rust"));
        assert_eq!(get(root, "monitor.baudrate").as_deref(), Some("115200"));
        assert_eq!(get_list(root, "git.reviewers"), Some(vec!["alex".to_string(), "sam".to_string()]));
        set(root, "project.kind", None).unwrap();
        let text = std::fs::read_to_string(path(root)).unwrap();
        assert!(text.starts_with("# This project's own Fenix settings") && text.contains("# the team's note") && !text.contains("kind"), "{text}");
        assert!(!text.contains("[project]\n\n[project]"));
    }

    #[test]
    fn a_file_that_cant_be_parsed_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".fenix")).unwrap();
        std::fs::write(path(dir.path()), "[project\nkind = \"rust\"\n").unwrap();
        assert!(set(dir.path(), "project.jira", Some(Value::Text("FNX".into()))).is_err());
        assert_eq!(std::fs::read_to_string(path(dir.path())).unwrap(), "[project\nkind = \"rust\"\n");
    }
}
