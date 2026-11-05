//! The filter's search beyond what's been read: through a list of every
//! file under a folder (the project's file index), for names containing
//! what's typed. A plain function over a list, so the host can run it
//! off the UI thread.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::entry::Entry;

/// Every file and folder under `root` whose name contains `needle`,
/// ignoring case -- folders taken from the paths of the `files` under
/// them -- read from the disk, at most `limit` of them.
pub fn search<'a>(files: impl IntoIterator<Item = &'a Path>, root: &Path, needle: &str, limit: usize) -> Vec<Entry> {
    let needle = needle.trim().to_lowercase();
    if needle.is_empty() {
        return Vec::new();
    }
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut found = Vec::new();
    for file in files {
        let Ok(rel) = file.strip_prefix(root) else { continue };
        let mut at = root.to_path_buf();
        for part in rel.components() {
            at.push(part);
            if part.as_os_str().to_string_lossy().to_lowercase().contains(&needle) && seen.insert(at.clone()) {
                found.push(Entry::at(&at));
                if found.len() >= limit {
                    return found;
                }
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::TempDir;

    #[test]
    fn finds_files_and_folders_by_name_anywhere_below() {
        let dir = TempDir::new("search_names");
        let deep = dir.path().join("src").join("parser");
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::write(deep.join("lexer.rs"), "").unwrap();
        std::fs::write(deep.join("other.rs"), "").unwrap();
        let files = [deep.join("lexer.rs"), deep.join("other.rs")];

        let found = search(files.iter().map(PathBuf::as_path), dir.path(), "PARS", 10);
        assert_eq!(found.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), ["parser"], "a folder, from the paths under it");
        assert!(found[0].is_dir());

        let found = search(files.iter().map(PathBuf::as_path), dir.path(), "lex", 10);
        assert_eq!(found[0].path, deep.join("lexer.rs"));
        assert_eq!(found[0].size, 0);
        assert!(found[0].readable);

        assert_eq!(search(files.iter().map(PathBuf::as_path), dir.path(), "rs", 1).len(), 1, "no more than asked for");
    }
}
