//! What git says about a project root, for the hub and Home: the branch
//! (read straight from `.git/HEAD`, no process) and a fuller summary --
//! uncommitted changes, ahead/behind, the last commit -- from one `git
//! status` and one `git log`, for callers that can afford two processes
//! (the hub runs them off the UI thread).

use std::path::{Path, PathBuf};
use std::process::Command;

/// The repository `path` is in: the nearest ancestor (or `path` itself)
/// with a `.git` -- so a subproject of a monorepo belongs to the
/// monorepo's repository, not to none.
pub fn repository_root(path: &Path) -> Option<PathBuf> {
    path.ancestors().find(|dir| dir.join(".git").exists()).map(Path::to_path_buf)
}

/// The branch checked out in the repository `root` is in, read straight
/// from `.git/HEAD` (a worktree's `.git` file is followed to its real git
/// dir). A detached HEAD reads as its short commit.
pub fn git_branch(root: &Path) -> Option<String> {
    let root = repository_root(root)?;
    let dot_git = root.join(".git");
    let git_dir = if dot_git.is_file() {
        let pointer = std::fs::read_to_string(&dot_git).ok()?;
        let dir = pointer.trim().strip_prefix("gitdir:")?.trim();
        let dir = PathBuf::from(dir);
        if dir.is_absolute() { dir } else { root.join(dir) }
    } else {
        dot_git
    };
    let head = std::fs::read_to_string(git_dir.join("HEAD")).ok()?;
    let head = head.trim();
    match head.strip_prefix("ref: refs/heads/") {
        Some(branch) => Some(branch.to_string()),
        None => (head.len() >= 7 && head.chars().all(|c| c.is_ascii_hexdigit())).then(|| head[..7].to_string()),
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GitSummary {
    pub branch: Option<String>,
    /// Files with uncommitted changes, untracked ones included.
    pub changed: usize,
    pub ahead: usize,
    pub behind: usize,
    /// The last commit's subject and when it was made (Unix seconds).
    pub last_commit: Option<(String, i64)>,
}

/// Parses `git status --porcelain=v1 --branch` output.
fn parse_status(text: &str, summary: &mut GitSummary) {
    for line in text.lines() {
        if let Some(header) = line.strip_prefix("## ") {
            // "main...origin/main [ahead 2, behind 1]" or "No commits yet on main"
            let (names, counts) = header.split_once(" [").map(|(n, c)| (n, Some(c.trim_end_matches(']')))).unwrap_or((header, None));
            let name = names.strip_prefix("No commits yet on ").unwrap_or(names);
            let branch = name.split("...").next().unwrap_or(name).trim();
            if !branch.is_empty() && branch != "HEAD (no branch)" {
                summary.branch = Some(branch.to_string());
            }
            for part in counts.into_iter().flat_map(|c| c.split(", ")) {
                if let Some(n) = part.strip_prefix("ahead ") {
                    summary.ahead = n.trim().parse().unwrap_or(0);
                } else if let Some(n) = part.strip_prefix("behind ") {
                    summary.behind = n.trim().parse().unwrap_or(0);
                }
            }
        } else if !line.trim().is_empty() {
            summary.changed += 1;
        }
    }
}

fn git(root: &Path, args: &[&str]) -> Option<String> {
    let mut command = Command::new("git");
    command.args(args).current_dir(root);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let output = command.output().ok()?;
    output.status.success().then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

/// `root`'s git summary, or `None` when it isn't in a repository (or git
/// isn't installed). Scoped to `root`: in a monorepo, a subproject's
/// changes and last commit are its own, not the whole repository's. Runs
/// two git processes -- call it off the UI thread.
pub fn git_summary(root: &Path) -> Option<GitSummary> {
    repository_root(root)?;
    let status = git(root, &["status", "--porcelain=v1", "--branch", "--", "."])?;
    let mut summary = GitSummary::default();
    parse_status(&status, &mut summary);
    if summary.branch.is_none() {
        summary.branch = git_branch(root);
    }
    summary.last_commit = git(root, &["log", "-1", "--format=%s%x1f%ct", "--", "."]).and_then(|text| {
        let (subject, time) = text.trim().split_once('\u{1f}')?;
        Some((subject.to_string(), time.trim().parse().ok()?))
    });
    Some(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::TempDir;

    #[test]
    fn status_header_and_changes_are_read() {
        let mut s = GitSummary::default();
        parse_status("## main...origin/main [ahead 2, behind 1]\n M a.rs\n?? b.rs\n", &mut s);
        assert_eq!((s.branch.as_deref(), s.changed, s.ahead, s.behind), (Some("main"), 2, 2, 1));
        let mut s = GitSummary::default();
        parse_status("## No commits yet on master\n", &mut s);
        assert_eq!((s.branch.as_deref(), s.changed), (Some("master"), 0));
    }

    #[test]
    fn git_branch_reads_head_and_follows_a_worktree_pointer() {
        let dir = TempDir::new("vcs_branch");
        dir.write(".git/HEAD", "ref: refs/heads/feature/x\n");
        assert_eq!(git_branch(dir.path()).as_deref(), Some("feature/x"));
        let other = TempDir::new("vcs_worktree");
        let real = other.path().join("realgit");
        std::fs::create_dir_all(&real).unwrap();
        std::fs::write(real.join("HEAD"), "0123456789abcdef0123456789abcdef01234567\n").unwrap();
        other.write("wt/.git", &format!("gitdir: {}\n", real.display()));
        assert_eq!(git_branch(&other.path().join("wt")).as_deref(), Some("0123456"));
        assert_eq!(git_branch(&other.path().join("nowhere")), None);
    }

    #[test]
    fn a_real_repository_is_summarised() {
        let dir = TempDir::new("vcs_summary");
        let run = |args: &[&str]| git(dir.path(), args);
        if run(&["init", "-q", "-b", "trunk"]).is_none() {
            return; // no git here
        }
        run(&["config", "user.email", "t@example.com"]);
        run(&["config", "user.name", "T"]);
        dir.write("a.txt", "a");
        run(&["add", "-A"]);
        run(&["commit", "-q", "-m", "First thing"]);
        dir.write("b.txt", "b");
        let summary = git_summary(dir.path()).unwrap();
        assert_eq!(summary.branch.as_deref(), Some("trunk"));
        assert_eq!(summary.changed, 1);
        assert_eq!(summary.last_commit.as_ref().map(|(s, _)| s.as_str()), Some("First thing"));
        assert!(git_summary(&dir.path().join("missing")).is_none());

        // A subproject: the repository's branch, its own changes only.
        dir.write("sub/Cargo.toml", "[package]");
        run(&["add", "-A"]);
        run(&["commit", "-q", "-m", "Add sub"]);
        dir.write("sub/new.rs", "");
        let sub = git_summary(&dir.path().join("sub")).unwrap();
        assert_eq!(sub.branch.as_deref(), Some("trunk"));
        assert_eq!(sub.changed, 1, "b.txt at the top isn't the subproject's");
        assert_eq!(sub.last_commit.map(|(s, _)| s).as_deref(), Some("Add sub"));
        assert_eq!(repository_root(&dir.path().join("sub")).as_deref(), Some(dir.path()));
        assert_eq!(git_branch(&dir.path().join("sub")).as_deref(), Some("trunk"));
    }
}
