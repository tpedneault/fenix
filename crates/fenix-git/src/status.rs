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

/// What an extension may know of a repository (`git-state`): the branch,
/// the commit it's on, how it stands against its upstream, and how many
/// files are left in conflict -- from one `git status`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Summary {
    /// None when `HEAD` is detached, or there are no commits yet.
    pub branch: Option<String>,
    /// The commit `HEAD` is on, none before the first.
    pub head: Option<String>,
    pub upstream: Option<String>,
    pub ahead: usize,
    pub behind: usize,
    pub conflicts: usize,
}

/// `repo`'s summary, or none outside a repository (or without `git`).
pub fn summary(repo: &Path) -> Option<Summary> {
    let lines = run_lines(repo, &["status", "--porcelain=v2", "--branch"]);
    parse_summary(&lines)
}

fn parse_summary(lines: &[String]) -> Option<Summary> {
    let status = parse_status(lines)?;
    let head = lines.iter().find_map(|l| l.strip_prefix("# branch.oid ")).filter(|oid| *oid != "(initial)").map(str::to_string);
    let branch = Some(status.branch).filter(|b| b != "(detached)" && b != "(initial)");
    let conflicts = lines.iter().filter(|l| l.starts_with("u ")).count();
    Some(Summary { branch, head, upstream: status.upstream, ahead: status.ahead, behind: status.behind, conflicts })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{git, init_repo, TempDir};

    #[test]
    fn a_summary_has_the_head_and_counts_conflicts() {
        let lines: Vec<String> = [
            "# branch.oid 1234abcd",
            "# branch.head main",
            "# branch.upstream origin/main",
            "# branch.ab +2 -1",
            "u UU N... 100644 100644 100644 100644 abc abc abc a.txt",
            "1 .M N... 100644 100644 100644 abc abc b.txt",
        ]
        .map(String::from)
        .into();
        let s = parse_summary(&lines).unwrap();
        assert_eq!(s, Summary { branch: Some("main".into()), head: Some("1234abcd".into()), upstream: Some("origin/main".into()), ahead: 2, behind: 1, conflicts: 1 });
        let fresh: Vec<String> = ["# branch.oid (initial)", "# branch.head main"].map(String::from).into();
        assert_eq!(parse_summary(&fresh).unwrap().head, None);
        let detached: Vec<String> = ["# branch.oid 99", "# branch.head (detached)"].map(String::from).into();
        assert_eq!(parse_summary(&detached).unwrap().branch, None);
        assert_eq!(parse_summary(&[]), None);
    }

    #[test]
    fn a_real_commit_moves_the_summarys_head() {
        let dir = TempDir::new("summary_head");
        init_repo(dir.path());
        dir.write("a.txt", "1");
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-q", "-m", "one"]);
        let first = summary(dir.path()).unwrap();
        dir.write("a.txt", "2");
        git(dir.path(), &["commit", "-q", "-am", "two"]);
        let second = summary(dir.path()).unwrap();
        assert!(first.head.is_some() && first.head != second.head);
        assert_eq!(first.branch, second.branch);
    }

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
