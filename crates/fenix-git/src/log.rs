//! History for the Log page: a branch's or every ref's commits, one
//! file's (following renames), or one range of lines' (`git log -L`) --
//! and what a commit changed, file by file.

use std::path::Path;

use crate::graph::GraphCommit;
use crate::process::{run_action, run_lines};

const FORMAT: &str = "--format=%H\x1f%h\x1f%P\x1f%D\x1f%an\x1f%ar\x1f%s";

/// What to list.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LogQuery {
    /// Every ref (`--all`), else the current branch.
    pub all: bool,
    /// Only commits touching this file, following renames.
    pub path: Option<String>,
    /// `--grep`, case-insensitive.
    pub grep: Option<String>,
    /// `--author`.
    pub author: Option<String>,
    pub limit: usize,
}

fn parse(line: &str) -> Option<GraphCommit> {
    let mut f = line.split('\x1f');
    let hash = f.next()?.to_string();
    if hash.len() != 40 {
        return None;
    }
    Some(GraphCommit {
        hash,
        short_hash: f.next()?.to_string(),
        parents: f.next()?.split_whitespace().map(str::to_string).collect(),
        refs: f.next()?.split(", ").filter(|r| !r.is_empty()).map(str::to_string).collect(),
        author: f.next()?.to_string(),
        relative_date: f.next()?.to_string(),
        subject: f.next().unwrap_or("").to_string(),
    })
}

/// A filtered list is drawn as one straight line: each commit's parent is
/// the next one listed, whatever it really was.
fn linear(mut commits: Vec<GraphCommit>) -> Vec<GraphCommit> {
    let next: Vec<Option<String>> = commits.iter().skip(1).map(|c| Some(c.hash.clone())).chain(std::iter::once(None)).collect();
    for (commit, parent) in commits.iter_mut().zip(next) {
        commit.parents = parent.into_iter().collect();
    }
    commits
}

pub fn log(repo: &Path, query: &LogQuery) -> Vec<GraphCommit> {
    let limit = format!("-n{}", query.limit.max(1));
    let mut args: Vec<String> = vec!["log".into(), "--date-order".into(), limit, FORMAT.into()];
    if query.all && query.path.is_none() {
        args.push("--all".into());
    }
    if let Some(grep) = &query.grep {
        args.extend(["-i".into(), format!("--grep={grep}")]);
    }
    if let Some(author) = &query.author {
        args.push(format!("--author={author}"));
    }
    if let Some(path) = &query.path {
        args.extend(["--follow".into(), "--".into(), path.clone()]);
    }
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let commits: Vec<GraphCommit> = run_lines(repo, &refs).iter().filter_map(|l| parse(l)).collect();
    let filtered = query.path.is_some() || query.grep.is_some() || query.author.is_some();
    if filtered { linear(commits) } else { commits }
}

/// `git log -L start,end:path`: the commits that changed those lines, each
/// with the patch showing how.
pub fn line_log(repo: &Path, path: &str, start: usize, end: usize, limit: usize) -> Result<Vec<(GraphCommit, String)>, String> {
    let range = format!("-L{start},{end}:{path}");
    let out = run_action(repo, &["log".into(), format!("-n{}", limit.max(1)), FORMAT.into(), range])?;
    let mut entries: Vec<(GraphCommit, String)> = Vec::new();
    for line in out.lines() {
        if let Some(commit) = line.contains('\x1f').then(|| parse(line)).flatten() {
            entries.push((commit, String::new()));
        } else if let Some((_, patch)) = entries.last_mut() {
            patch.push_str(line);
            patch.push('\n');
        }
    }
    let (commits, patches): (Vec<_>, Vec<_>) = entries.into_iter().unzip();
    Ok(linear(commits).into_iter().zip(patches).collect())
}

/// What `hash` changed: each file's status letter and path.
pub fn commit_files(repo: &Path, hash: &str) -> Vec<(char, String)> {
    run_lines(repo, &["show", "--no-color", "--name-status", "--format=", "--first-parent", hash])
        .into_iter()
        .filter_map(|l| {
            let mut f = l.split('\t');
            let status = f.next()?.chars().next()?;
            // A rename lists old then new; the new one is what's there.
            let path = f.next_back()?.to_string();
            Some((status, path))
        })
        .collect()
}

/// The diff `hash` made to one file.
pub fn commit_file_diff(repo: &Path, hash: &str, path: &str) -> Result<String, String> {
    run_action(repo, &["show".into(), "--no-color".into(), "--format=".into(), "--first-parent".into(), hash.into(), "--".into(), path.into()])
}

/// The commits of `rev`'s history among the most recent `limit` -- to
/// tell which listed commits are already on the current branch.
pub fn reachable(repo: &Path, rev: &str, limit: usize) -> Vec<String> {
    run_lines(repo, &["rev-list", &format!("-n{limit}"), rev])
}

/// Checks out a commit, detaching HEAD.
pub fn checkout_detached(repo: &Path, hash: &str) -> Result<String, String> {
    run_action(repo, &["switch".into(), "--detach".into(), hash.into()])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{git, init_repo, TempDir};

    fn repo(name: &str) -> TempDir {
        let dir = TempDir::new(name);
        init_repo(dir.path());
        git(dir.path(), &["config", "core.autocrlf", "false"]);
        dir.write("a.txt", "one\ntwo\nthree\n");
        dir.write("b.txt", "b\n");
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-q", "-m", "first"]);
        dir.write("a.txt", "one\nTWO\nthree\n");
        git(dir.path(), &["commit", "-q", "-am", "second: a"]);
        dir.write("b.txt", "B\n");
        git(dir.path(), &["commit", "-q", "-am", "third: b"]);
        dir
    }

    #[test]
    fn a_files_history_lists_only_its_commits_as_one_line() {
        let dir = repo("log_file");
        let all = log(dir.path(), &LogQuery { limit: 10, ..Default::default() });
        assert_eq!(all.iter().map(|c| c.subject.as_str()).collect::<Vec<_>>(), ["third: b", "second: a", "first"]);
        let a = log(dir.path(), &LogQuery { path: Some("a.txt".into()), limit: 10, ..Default::default() });
        assert_eq!(a.iter().map(|c| c.subject.as_str()).collect::<Vec<_>>(), ["second: a", "first"]);
        assert_eq!(a[0].parents, [a[1].hash.clone()], "drawn as a straight line");
        let grep = log(dir.path(), &LogQuery { grep: Some("THIRD".into()), limit: 10, ..Default::default() });
        assert_eq!(grep.len(), 1);
    }

    #[test]
    fn a_line_range_history_comes_with_its_patches() {
        let dir = repo("log_lines");
        let entries = line_log(dir.path(), "a.txt", 2, 2, 10).unwrap();
        assert_eq!(entries.iter().map(|(c, _)| c.subject.as_str()).collect::<Vec<_>>(), ["second: a", "first"]);
        assert!(entries[0].1.contains("+TWO") && entries[0].1.contains("-two"), "{}", entries[0].1);
    }

    #[test]
    fn a_commit_lists_its_files_and_their_diffs() {
        let dir = repo("log_files");
        let head = run_lines(dir.path(), &["rev-parse", "HEAD~1"]).remove(0);
        assert_eq!(commit_files(dir.path(), &head), [('M', "a.txt".to_string())]);
        assert!(commit_file_diff(dir.path(), &head, "a.txt").unwrap().contains("+TWO"));
        assert_eq!(reachable(dir.path(), "HEAD", 10).len(), 3);
    }
}
