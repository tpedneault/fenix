use std::path::Path;

use crate::files::{parse_files, FileEntry};
use crate::process::run_lines;

/// The current branch plus its upstream tracking state -- mirrors
/// `fenix-docker::ContainerStat`'s role of small, frequently-polled,
/// live-updating data, fed to the Git panel's Status pane every ~2s by
/// its own poller thread (see `fenix-gui`'s `GitStatusPoller`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoStatus {
    /// The branch name, or a literal `"(detached)"`/`"(initial)"` when
    /// there's no branch to name (detached `HEAD`, or a brand-new repo
    /// with no commits yet) -- whatever `git status`'s own `# branch.head`
    /// line reports, shown as-is rather than translated.
    pub branch: String,
    pub upstream: Option<String>,
    pub ahead: usize,
    pub behind: usize,
}

/// `git status --porcelain=v2 --branch`'s header lines, parsed -- `None`
/// outside a git repo, without `git` on `PATH`, or if `git` reports no
/// `branch.head` line at all (shouldn't happen for a real repo, but
/// never a hard error either way).
pub fn status(repo: &Path) -> Option<RepoStatus> {
    let lines = run_lines(repo, &["status", "--porcelain=v2", "--branch"]);
    parse_status(&lines)
}

/// `status()` and `files::list_files()` combined into one shell-out --
/// `--branch`'s header lines are strictly additive on top of plain
/// `--porcelain=v2`'s entry lines, so the two never needed separate
/// `git status` invocations in the first place. Callers that used to
/// call both (`fenix-gui`'s panel open/refresh) should use this instead;
/// `status()`/`list_files()` themselves are unchanged for anyone still
/// calling just one.
pub fn status_and_files(repo: &Path) -> (Option<RepoStatus>, Vec<FileEntry>) {
    let lines = run_lines(repo, &["status", "--porcelain=v2", "--branch"]);
    (parse_status(&lines), parse_files(&lines))
}

/// Verified against real `git status --porcelain=v2 --branch` output
/// (run directly in this repo this session, not guessed): header lines
/// `# branch.oid <sha>`, `# branch.head <name>`, `# branch.upstream
/// <name>` (absent when there's no upstream), `# branch.ab +<ahead>
/// -<behind>` (also absent without an upstream to compare against).
fn parse_status(lines: &[String]) -> Option<RepoStatus> {
    let mut branch = None;
    let mut upstream = None;
    let mut ahead = 0;
    let mut behind = 0;
    for line in lines {
        if let Some(rest) = line.strip_prefix("# branch.head ") {
            branch = Some(rest.to_string());
        } else if let Some(rest) = line.strip_prefix("# branch.upstream ") {
            upstream = Some(rest.to_string());
        } else if let Some(rest) = line.strip_prefix("# branch.ab ") {
            for part in rest.split_whitespace() {
                if let Some(n) = part.strip_prefix('+') {
                    ahead = n.parse().unwrap_or(0);
                } else if let Some(n) = part.strip_prefix('-') {
                    behind = n.parse().unwrap_or(0);
                }
            }
        }
    }
    branch.map(|branch| RepoStatus { branch, upstream, ahead, behind })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{git, init_repo, TempDir};

    #[test]
    fn status_and_files_matches_the_two_separate_calls() {
        let dir = TempDir::new("status_and_files");
        init_repo(dir.path());
        dir.write("committed.txt", "v1");
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-q", "-m", "initial"]);
        dir.write("committed.txt", "v2");
        dir.write("untracked.txt", "new");

        let (s, files) = status_and_files(dir.path());
        assert_eq!(s, status(dir.path()));
        assert_eq!(files, crate::files::list_files(dir.path()));
        assert_eq!(files.len(), 2);
    }

    #[test]
    fn none_outside_a_git_repo() {
        let dir = TempDir::new("status_no_repo");
        assert_eq!(status(dir.path()), None);
    }

    #[test]
    fn parse_status_handles_no_upstream() {
        let lines: Vec<String> = ["# branch.oid abc123", "# branch.head main"].iter().map(|s| s.to_string()).collect();
        assert_eq!(parse_status(&lines), Some(RepoStatus { branch: "main".to_string(), upstream: None, ahead: 0, behind: 0 }));
    }

    #[test]
    fn parse_status_reads_upstream_and_ahead_behind() {
        let lines: Vec<String> = [
            "# branch.oid abc123",
            "# branch.head main",
            "# branch.upstream origin/main",
            "# branch.ab +2 -3",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(
            parse_status(&lines),
            Some(RepoStatus { branch: "main".to_string(), upstream: Some("origin/main".to_string()), ahead: 2, behind: 3 })
        );
    }

    #[test]
    fn reports_the_real_branch_name_for_a_fresh_commit() {
        let dir = TempDir::new("status_fresh_repo");
        init_repo(dir.path());
        dir.write("a.txt", "hello");
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-q", "-m", "initial"]);

        let s = status(dir.path()).unwrap();
        assert_eq!(s.branch, "main");
        assert_eq!(s.upstream, None);
    }

    #[test]
    fn reports_ahead_count_against_a_real_upstream() {
        let remote = TempDir::new("status_ahead_remote");
        git(remote.path(), &["init", "-q", "--bare"]);

        let dir = TempDir::new("status_ahead_clone");
        init_repo(dir.path());
        git(dir.path(), &["remote", "add", "origin", remote.path().to_str().unwrap()]);
        dir.write("a.txt", "v1");
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-q", "-m", "first"]);
        git(dir.path(), &["push", "-q", "-u", "origin", "main"]);

        dir.write("a.txt", "v2");
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-q", "-m", "second"]);

        let s = status(dir.path()).unwrap();
        assert_eq!(s.upstream.as_deref(), Some("origin/main"));
        assert_eq!(s.ahead, 1);
        assert_eq!(s.behind, 0);
    }
}
