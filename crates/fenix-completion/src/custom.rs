use std::path::Path;

use crate::{CompletionItem, CompletionKind};

/// Reads a user-supplied symbols file: one identifier per line, blank
/// lines and `#`-prefixed comment lines skipped. Every entry is tagged
/// `CompletionKind::Tag` -- see that variant's own doc comment for why
/// custom symbols share a bucket with ctags-sourced definitions rather
/// than getting a third kind.
///
/// Never fails -- a missing or unreadable file yields an empty `Vec`,
/// the same posture `ctags::run` already has for a missing `ctags`
/// binary. The caller (`fenix-gui`) is expected to re-read this on its
/// own refresh cadence (`SPC c r`), not on every keystroke.
pub fn load(path: &Path) -> Vec<CompletionItem> {
    let Ok(contents) = std::fs::read_to_string(path) else { return Vec::new() };
    contents
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|label| CompletionItem { label: label.to_string(), kind: CompletionKind::Tag })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn temp_file(name: &str, contents: &str) -> std::path::PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("fenix-completion-custom-test-{name}-{}-{n}.txt", std::process::id()));
        std::fs::write(&path, contents).unwrap();
        path
    }

    #[test]
    fn loads_one_symbol_per_line() {
        let path = temp_file("symbols", "my_custom_proc\nanother_symbol\n");
        let items = load(&path);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(labels, vec!["my_custom_proc", "another_symbol"]);
        assert!(items.iter().all(|i| i.kind == CompletionKind::Tag));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn skips_blank_lines_and_comment_lines() {
        let path = temp_file("blanks_and_comments", "foo\n\n# a comment\n   \nbar\n");
        let labels: Vec<String> = load(&path).into_iter().map(|i| i.label).collect();
        assert_eq!(labels, vec!["foo".to_string(), "bar".to_string()]);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn trims_surrounding_whitespace_on_each_line() {
        let path = temp_file("whitespace", "  padded_symbol  \n");
        let labels: Vec<String> = load(&path).into_iter().map(|i| i.label).collect();
        assert_eq!(labels, vec!["padded_symbol".to_string()]);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn a_missing_file_yields_an_empty_list_not_an_error() {
        let path = std::env::temp_dir().join("fenix-completion-custom-test-does-not-exist.txt");
        assert!(load(&path).is_empty());
    }
}
