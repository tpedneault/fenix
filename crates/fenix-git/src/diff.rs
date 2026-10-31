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

/// The diff between two refs.
///
/// `three_dot` selects `base...head` -- everything `head` added *since
/// it diverged from* `base`, measured from their merge base -- rather
/// than `base..head`, which also reports as "removed" every change
/// `base` gained in the meantime. Three-dot is what people mean by "what
/// does my branch do", and what every code-review tool shows, so it's
/// the default; two-dot stays available for the genuine "what is
/// literally different between these two trees" question.
pub fn diff_refs(repo: &Path, base: &str, head: &str, three_dot: bool) -> Result<String, String> {
    let range = if three_dot { format!("{base}...{head}") } else { format!("{base}..{head}") };
    run_action(repo, &["diff".to_string(), "--no-color".to_string(), range])
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

    /// A repo whose `side` branch and `main` have each moved on since
    /// they diverged -- the only shape where three-dot and two-dot
    /// actually differ, which is the whole point of having both.
    fn diverged_repo(name: &str) -> TempDir {
        let dir = TempDir::new(name);
        init_repo(dir.path());
        dir.write("shared.txt", "base\n");
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-q", "-m", "initial"]);
        git(dir.path(), &["checkout", "-q", "-b", "side"]);
        dir.write("from_side.txt", "side work\n");
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-q", "-m", "on side"]);
        git(dir.path(), &["checkout", "-q", "main"]);
        dir.write("from_main.txt", "main work\n");
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-q", "-m", "on main"]);
        dir
    }

    #[test]
    fn diff_refs_three_dot_shows_only_what_the_head_branch_added() {
        let dir = diverged_repo("diff_refs_three_dot");
        let diff = diff_refs(dir.path(), "main", "side", true).unwrap();
        assert!(diff.contains("from_side.txt"), "side's own work is the point of the comparison:\n{diff}");
        assert!(!diff.contains("from_main.txt"), "main's later work is not side's doing:\n{diff}");
    }

    #[test]
    fn diff_refs_two_dot_also_reports_what_the_base_gained_since_diverging() {
        let dir = diverged_repo("diff_refs_two_dot");
        let diff = diff_refs(dir.path(), "main", "side", false).unwrap();
        assert!(diff.contains("from_side.txt"));
        // Present as a *deletion*, because two-dot compares the trees
        // directly: `side` doesn't have main's newer file.
        assert!(diff.contains("from_main.txt"), "two-dot sees the base's own newer work too:\n{diff}");
    }

    #[test]
    fn diff_refs_between_identical_refs_is_empty() {
        let dir = diverged_repo("diff_refs_same");
        assert!(diff_refs(dir.path(), "main", "main", true).unwrap().trim().is_empty());
    }

    #[test]
    fn diff_refs_reports_gits_own_error_for_an_unknown_ref() {
        let dir = diverged_repo("diff_refs_unknown");
        assert!(diff_refs(dir.path(), "main", "no-such-branch", true).is_err());
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
