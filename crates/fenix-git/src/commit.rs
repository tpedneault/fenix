use std::path::Path;

use crate::process::run_lines;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit {
    pub hash: String,
    pub short_hash: String,
    pub message: String,
    pub author: String,
    pub relative_date: String,
}

/// The current branch's commit log, most recent first (`git log`'s own
/// default order) -- `%s` is deliberately the subject line only (commit
/// messages can have a body, but a single-line list row has no room for
/// one; `commit_diff`, not this, is where the full message shows up).
pub fn list_commits(repo: &Path, limit: usize) -> Vec<Commit> {
    let n = format!("-n{limit}");
    run_lines(repo, &["log", &n, "--format=%H\x1f%h\x1f%s\x1f%an\x1f%ar"]).iter().filter_map(|l| parse_line(l)).collect()
}

/// The commits `head` has that `base` doesn't (`git log base..head`),
/// newest first -- "what would this merge bring in", the list that
/// belongs beside a ref comparison's diff.
///
/// Two-dot here even though `diff_refs` defaults to three-dot: for a
/// *commit list* the two mean the same thing (`base...head` on `git log`
/// would additionally list base's own commits, which is not what's being
/// asked), so this is the direct equivalent of three-dot's diff.
pub fn commits_between(repo: &Path, base: &str, head: &str, limit: usize) -> Vec<Commit> {
    let n = format!("-n{limit}");
    let range = format!("{base}..{head}");
    run_lines(repo, &["log", &n, &range, "--format=%H\x1f%h\x1f%s\x1f%an\x1f%ar"]).iter().filter_map(|l| parse_line(l)).collect()
}

fn parse_line(line: &str) -> Option<Commit> {
    let mut fields = line.split('\x1f');
    Some(Commit {
        hash: fields.next()?.to_string(),
        short_hash: fields.next()?.to_string(),
        message: fields.next()?.to_string(),
        author: fields.next()?.to_string(),
        relative_date: fields.next()?.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{git, init_repo, TempDir};

    #[test]
    fn parse_line_reads_every_field() {
        let commit = parse_line("abc123full\x1fabc123\x1fFix the thing\x1fJane Doe\x1f3 days ago").unwrap();
        assert_eq!(
            commit,
            Commit {
                hash: "abc123full".to_string(),
                short_hash: "abc123".to_string(),
                message: "Fix the thing".to_string(),
                author: "Jane Doe".to_string(),
                relative_date: "3 days ago".to_string(),
            }
        );
    }

    #[test]
    fn list_commits_empty_outside_a_git_repo() {
        let dir = TempDir::new("commit_no_repo");
        assert!(list_commits(dir.path(), 10).is_empty());
    }

    #[test]
    fn list_commits_empty_for_a_repo_with_no_commits_yet() {
        let dir = TempDir::new("commit_no_commits");
        init_repo(dir.path());
        assert!(list_commits(dir.path(), 10).is_empty());
    }

    #[test]
    fn list_commits_returns_real_commits_most_recent_first() {
        let dir = TempDir::new("commit_real");
        init_repo(dir.path());
        dir.write("a.txt", "v1");
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-q", "-m", "first commit"]);
        dir.write("a.txt", "v2");
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-q", "-m", "second commit"]);

        let commits = list_commits(dir.path(), 10);
        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].message, "second commit");
        assert_eq!(commits[1].message, "first commit");
        assert!(commits[0].hash.starts_with(&commits[0].short_hash));
    }

    #[test]
    fn commits_between_lists_only_what_the_head_ref_adds() {
        let dir = TempDir::new("commits_between");
        init_repo(dir.path());
        dir.write("a.txt", "v1");
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-q", "-m", "initial"]);
        git(dir.path(), &["checkout", "-q", "-b", "side"]);
        dir.write("b.txt", "side");
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-q", "-m", "on side"]);

        let commits = commits_between(dir.path(), "main", "side", 50);
        assert_eq!(commits.len(), 1, "{commits:?}");
        assert_eq!(commits[0].message, "on side");
        // ...and nothing in the other direction.
        assert!(commits_between(dir.path(), "side", "main", 50).is_empty());
    }

    #[test]
    fn commits_between_is_empty_for_an_unknown_ref_rather_than_failing() {
        let dir = TempDir::new("commits_between_unknown");
        init_repo(dir.path());
        assert!(commits_between(dir.path(), "main", "no-such-branch", 50).is_empty());
    }

    #[test]
    fn list_commits_respects_the_limit() {
        let dir = TempDir::new("commit_limit");
        init_repo(dir.path());
        for i in 0..5 {
            dir.write("a.txt", &i.to_string());
            git(dir.path(), &["add", "."]);
            git(dir.path(), &["commit", "-q", "-m", &format!("commit {i}")]);
        }
        assert_eq!(list_commits(dir.path(), 2).len(), 2);
    }
}
