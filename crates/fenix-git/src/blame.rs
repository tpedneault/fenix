//! `git blame`, line by line -- and the branches in the order you last
//! had them checked out, which is what a branch switcher should lead
//! with.

use std::collections::HashMap;
use std::path::Path;

use crate::process::{run_action, run_action_stdin, run_lines};

/// Who last changed one line, and in which commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlameLine {
    pub hash: String,
    pub author: String,
    /// Author time, seconds since the Unix epoch.
    pub time: u64,
    pub summary: String,
}

impl BlameLine {
    /// A line nobody has committed yet.
    pub fn uncommitted(&self) -> bool {
        self.hash.chars().all(|c| c == '0')
    }
}

/// One entry per line of `path` -- or of `contents`, when given, so a
/// buffer with unsaved edits is blamed as it stands (its new lines come
/// back uncommitted) and the lines match what's on screen.
pub fn blame(repo: &Path, path: &str, contents: Option<&str>) -> Result<Vec<BlameLine>, String> {
    let output = match contents {
        Some(text) => run_action_stdin(repo, &["blame".into(), "--porcelain".into(), "--contents".into(), "-".into(), "--".into(), path.into()], text)?,
        None => run_action(repo, &["blame".into(), "--porcelain".into(), "--".into(), path.into()])?,
    };
    Ok(parse_porcelain(&output))
}

/// `--porcelain` output: a header line (`<hash> <orig> <final> [<n>]`)
/// per line, the commit's details only the first time a commit appears,
/// then the line itself after a tab.
fn parse_porcelain(output: &str) -> Vec<BlameLine> {
    let mut known: HashMap<String, (String, u64, String)> = HashMap::new();
    let mut lines = Vec::new();
    let mut hash = String::new();
    for line in output.lines() {
        if line.starts_with('\t') {
            let (author, time, summary) = known.get(&hash).cloned().unwrap_or_default();
            lines.push(BlameLine { hash: hash.clone(), author, time, summary });
            continue;
        }
        let mut words = line.splitn(2, ' ');
        let first = words.next().unwrap_or("");
        let rest = words.next().unwrap_or("");
        if first.len() == 40 && first.chars().all(|c| c.is_ascii_hexdigit()) {
            hash = first.to_string();
            known.entry(hash.clone()).or_default();
            continue;
        }
        let entry = known.entry(hash.clone()).or_default();
        match first {
            "author" => entry.0 = rest.to_string(),
            "author-time" => entry.1 = rest.parse().unwrap_or(0),
            "summary" => entry.2 = rest.to_string(),
            _ => {}
        }
    }
    lines
}

/// Local branches, the one you were on most recently first -- from the
/// reflog's "checkout: moving from A to B" lines -- then the rest,
/// newest commit first.
pub fn branches_by_recency(repo: &Path) -> Vec<String> {
    let local = run_lines(repo, &["for-each-ref", "--sort=-committerdate", "refs/heads", "--format=%(refname:short)"]);
    let mut out: Vec<String> = Vec::new();
    for line in run_lines(repo, &["reflog", "--format=%gs", "-n", "2000"]) {
        let Some(moves) = line.strip_prefix("checkout: moving from ") else { continue };
        let Some((from, to)) = moves.split_once(" to ") else { continue };
        for name in [to, from] {
            if local.iter().any(|b| b == name) && !out.iter().any(|b| b == name) {
                out.push(name.to_string());
            }
        }
    }
    for name in local {
        if !out.contains(&name) {
            out.push(name);
        }
    }
    out
}

/// Each local branch's last commit, as git says it relatively ("3 days
/// ago").
pub fn branch_ages(repo: &Path) -> HashMap<String, String> {
    run_lines(repo, &["for-each-ref", "refs/heads", "--format=%(refname:short)\x1f%(committerdate:relative)"])
        .into_iter()
        .filter_map(|l| l.split_once('\x1f').map(|(a, b)| (a.to_string(), b.to_string())))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{git, init_repo, TempDir};

    #[test]
    fn each_line_is_blamed_on_the_commit_that_last_changed_it() {
        let dir = TempDir::new("blame");
        init_repo(dir.path());
        git(dir.path(), &["config", "core.autocrlf", "false"]);
        dir.write("a.txt", "one\ntwo\n");
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-q", "-m", "first"]);
        dir.write("a.txt", "one\nTWO\nthree\n");
        git(dir.path(), &["commit", "-q", "-am", "second"]);

        let lines = blame(dir.path(), "a.txt", None).unwrap();
        assert_eq!(lines.iter().map(|l| l.summary.as_str()).collect::<Vec<_>>(), ["first", "second", "second"]);
        assert_eq!(lines[0].author, "Test");
        assert!(lines[0].time > 0);

        // Unsaved text: its new line is nobody's yet.
        let lines = blame(dir.path(), "a.txt", Some("zero\none\nTWO\nthree\n")).unwrap();
        assert_eq!(lines.len(), 4);
        assert!(lines[0].uncommitted() && !lines[1].uncommitted());
        assert_eq!(lines[1].summary, "first");
    }

    #[test]
    fn branches_come_in_the_order_they_were_last_checked_out() {
        let dir = TempDir::new("recency");
        init_repo(dir.path());
        dir.write("a.txt", "a");
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-q", "-m", "first"]);
        for b in ["alpha", "beta", "gamma"] {
            git(dir.path(), &["branch", b]);
        }
        git(dir.path(), &["switch", "-q", "beta"]);
        git(dir.path(), &["switch", "-q", "alpha"]);
        let order = branches_by_recency(dir.path());
        assert_eq!(&order[..3], ["alpha", "beta", "main"], "{order:?}");
        assert!(order.contains(&"gamma".to_string()), "never checked out, still listed");
        assert!(branch_ages(dir.path()).get("gamma").is_some_and(|a| a.contains("ago")));
    }
}
