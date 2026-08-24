use std::path::Path;

use crate::process::run_action;

/// A tracked file's diff -- `staged` selects `--cached` (the diff
/// between the index and `HEAD`) vs. the working-tree diff (index vs.
/// disk). Untracked files have nothing for plain `git diff` to show
/// (they aren't tracked yet) -- disclosed, not handled here: the caller
/// (`fenix-gui`'s Main-pane renderer) special-cases an untracked
/// selection with its own placeholder instead of calling this.
pub fn file_diff(repo: &Path, path: &str, staged: bool) -> Result<String, String> {
    let mut args = vec!["diff".to_string(), "--no-color".to_string()];
    if staged {
        args.push("--cached".to_string());
    }
    args.push("--".to_string());
    args.push(path.to_string());
    run_action(repo, &args)
}

/// A commit's full message plus its diff -- `git show`'s own default
/// output already includes both.
pub fn commit_diff(repo: &Path, hash: &str) -> Result<String, String> {
    run_action(repo, &["show".to_string(), "--no-color".to_string(), hash.to_string()])
}

pub fn stash_diff(repo: &Path, index: usize) -> Result<String, String> {
    run_action(repo, &["stash".to_string(), "show".to_string(), "-p".to_string(), "--no-color".to_string(), format!("stash@{{{index}}}")])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{git, init_repo, TempDir};

    #[test]
    fn file_diff_shows_an_unstaged_change() {
        let dir = TempDir::new("diff_file_unstaged");
        init_repo(dir.path());
        dir.write("a.txt", "line one\n");
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-q", "-m", "initial"]);
        dir.write("a.txt", "line one changed\n");

        let diff = file_diff(dir.path(), "a.txt", false).unwrap();
        assert!(diff.contains("line one changed"));
    }

    #[test]
    fn file_diff_shows_a_staged_change_with_cached() {
        let dir = TempDir::new("diff_file_staged");
        init_repo(dir.path());
        dir.write("a.txt", "line one\n");
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-q", "-m", "initial"]);
        dir.write("a.txt", "line one changed\n");
        git(dir.path(), &["add", "."]);

        assert!(file_diff(dir.path(), "a.txt", false).unwrap().is_empty());
        assert!(file_diff(dir.path(), "a.txt", true).unwrap().contains("line one changed"));
    }

    #[test]
    fn commit_diff_includes_the_message_and_the_change() {
        let dir = TempDir::new("diff_commit");
        init_repo(dir.path());
        dir.write("a.txt", "hello\n");
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-q", "-m", "add hello"]);

        let out = commit_diff(dir.path(), "HEAD").unwrap();
        assert!(out.contains("add hello"));
        assert!(out.contains("hello"));
    }

    #[test]
    fn stash_diff_shows_the_stashed_change() {
        let dir = TempDir::new("diff_stash");
        init_repo(dir.path());
        dir.write("a.txt", "v1\n");
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-q", "-m", "initial"]);
        dir.write("a.txt", "v2\n");
        git(dir.path(), &["stash", "push"]);

        let out = stash_diff(dir.path(), 0).unwrap();
        assert!(out.contains("v2"));
    }
}
