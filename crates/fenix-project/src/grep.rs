use std::collections::HashMap;
use std::io;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use crate::search::{self, Options, Pattern, Scope};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrepMatch {
    pub path: PathBuf,
    pub line: usize,
    pub col: usize,
    pub text: String,
}

/// Every match of the regex `query` under `root`, one per match (as
/// `rg --vimgrep` lists them), honouring ignore files -- run by
/// [`search`](crate::search) in this process, case as typed.
pub fn grep_project(root: &Path, query: &str) -> io::Result<Vec<GrepMatch>> {
    let options = Options { regex: true, match_case: true, ..Options::default() };
    let pattern = Pattern::new(query, &options).map_err(io::Error::other)?;
    let mut out = Vec::new();
    search::search(&Scope::Folders(vec![root.to_path_buf()]), &pattern, &options, &HashMap::new(), usize::MAX, &AtomicBool::new(false), |file| {
        for line in &file.lines {
            for m in &line.matches {
                out.push(GrepMatch { path: file.path.clone(), line: line.number, col: m.start + 1, text: line.text.clone() });
            }
        }
        ControlFlow::Continue(())
    })
    .map_err(io::Error::other)?;
    Ok(out)
}

/// Every file under `root` with at least one match for `pattern` (a
/// regex, as with `grep_project`), honouring ignore files the same way.
/// For callers that need to look at a matching file as a whole -- to
/// parse it, say -- rather than at individual matched lines.
pub fn files_matching(root: &Path, pattern: &str) -> io::Result<Vec<PathBuf>> {
    let options = Options { regex: true, match_case: true, ..Options::default() };
    let compiled = Pattern::new(pattern, &options).map_err(io::Error::other)?;
    let mut files = Vec::new();
    search::search(&Scope::Folders(vec![root.to_path_buf()]), &compiled, &options, &HashMap::new(), usize::MAX, &AtomicBool::new(false), |file| {
        files.push(file.path);
        ControlFlow::Continue(())
    })
    .map_err(io::Error::other)?;
    files.sort();
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::TempDir;

    #[test]
    fn finds_a_match_with_correct_position_and_text() {
        let dir = TempDir::new("grep_finds_match");
        dir.write("a.txt", "hello\nneedle here\nworld\n");

        let matches = grep_project(dir.path(), "needle").unwrap();
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].line, 2);
        assert_eq!(matches[0].col, 1);
        assert_eq!(matches[0].text, "needle here");
        assert!(matches[0].path.ends_with("a.txt"));
    }

    #[test]
    fn no_matches_is_ok_with_an_empty_list_not_an_error() {
        let dir = TempDir::new("grep_no_matches");
        dir.write("a.txt", "nothing interesting here\n");
        let matches = grep_project(dir.path(), "totally_absent_pattern_xyz").unwrap();
        assert!(matches.is_empty());
    }

    #[test]
    fn finds_matches_across_multiple_files() {
        let dir = TempDir::new("grep_multi_file");
        dir.write("a.txt", "shared_term\n");
        dir.write("sub/b.txt", "also has shared_term here\n");

        let matches = grep_project(dir.path(), "shared_term").unwrap();
        assert_eq!(matches.len(), 2);
        assert_eq!(matches[1].col, 10);
    }

    #[test]
    fn files_matching_lists_each_matching_file_once() {
        let dir = TempDir::new("grep_files_matching");
        dir.write("a.txt", "needle\nneedle again\n");
        dir.write("sub/b.txt", "a needle\n");
        dir.write("c.txt", "nothing\n");
        let files = files_matching(dir.path(), "needle").unwrap();
        assert_eq!(files.len(), 2);
        assert!(files.iter().any(|f| f.ends_with("a.txt")));
    }

    #[test]
    fn a_bad_regex_is_an_error_not_a_panic() {
        let dir = TempDir::new("grep_bad_regex");
        assert!(grep_project(dir.path(), "(").is_err());
    }
}
