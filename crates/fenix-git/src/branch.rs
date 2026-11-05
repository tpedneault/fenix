use std::path::Path;

use crate::process::run_lines;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Branch {
    pub name: String,
    pub current: bool,
    pub upstream: Option<String>,
    pub ahead: usize,
    pub behind: usize,
    /// The branch has a configured upstream that no longer exists on the
    /// remote (`%(upstream:track)` reports `[gone]`) -- distinct from
    /// "no upstream at all" (`upstream: None`) and from "in sync"
    /// (`ahead`/`behind` both 0), both of which would otherwise look
    /// identical in a `0/0` sync badge. Usually means the remote branch
    /// was deleted after a merge request landed.
    pub upstream_gone: bool,
}

/// `\x1f` (unit separator) rather than a printable delimiter like `|` --
/// virtually never appears in a real branch name/upstream string, unlike
/// `|`, which is a legal (if unusual) git ref character.
const FORMAT: &str = "--format=%(refname:short)\x1f%(HEAD)\x1f%(upstream:short)\x1f%(upstream:track)";

/// Local branches via `git for-each-ref refs/heads` -- deliberately not
/// `git branch` (whose plain output isn't reliably machine-parsable, per
/// git's own docs recommending `for-each-ref`/`branch --format` instead).
pub fn list_branches(repo: &Path) -> Vec<Branch> {
    run_lines(repo, &["for-each-ref", "refs/heads", FORMAT]).iter().filter_map(|l| parse_line(l)).collect()
}

/// Verified against real `git for-each-ref` output (run directly in this
/// repo this session): `%(HEAD)` is a literal `*` for the checked-out
/// branch, a space otherwise; `%(upstream:track)` is `[ahead N]`,
/// `[behind N]`, `[ahead N, behind M]`, `[gone]`, or empty (up to date,
/// or no upstream at all).
fn parse_line(line: &str) -> Option<Branch> {
    let mut fields = line.split('\x1f');
    let name = fields.next()?.to_string();
    let current = fields.next()? == "*";
    let upstream_raw = fields.next()?;
    let upstream = (!upstream_raw.is_empty()).then(|| upstream_raw.to_string());
    let track = fields.next().unwrap_or("");
    let (ahead, behind) = parse_track(track);
    Some(Branch { name, current, upstream, ahead, behind, upstream_gone: track.contains("gone") })
}

/// Remote-tracking branches (`refs/remotes`), e.g. `origin/main` --
/// names only: a remote ref has no upstream of its own to compare
/// against, so there's nothing of `Branch`'s sync fields to fill in.
///
/// A remote's `HEAD` symref is filtered out: it points at whichever
/// branch that remote calls default, so listing it would show the same
/// branch twice under two names.
///
/// The filter is "the short name contains no `/`", not "it ends in
/// `/HEAD`", because `%(refname:short)` *already* shortens
/// `refs/remotes/origin/HEAD` all the way down to `origin` -- so the
/// obvious check never matches, and a bare remote name (`origin`) leaks
/// into what is supposed to be a list of branches.
pub fn list_remote_branches(repo: &Path) -> Vec<String> {
    run_lines(repo, &["for-each-ref", "refs/remotes", "--format=%(refname:short)"])
        .into_iter()
        .filter(|name| name.contains('/'))
        .collect()
}

/// Tags (`refs/tags`), newest first -- `--sort=-creatordate` covers both
/// lightweight tags (no date of their own, so this falls back to the
/// tagged commit's) and annotated ones.
pub fn list_tags(repo: &Path) -> Vec<String> {
    run_lines(repo, &["for-each-ref", "refs/tags", "--sort=-creatordate", "--format=%(refname:short)"])
}

fn parse_track(track: &str) -> (usize, usize) {
    let inner = track.trim_start_matches('[').trim_end_matches(']');
    let mut ahead = 0;
    let mut behind = 0;
    for part in inner.split(", ") {
        if let Some(n) = part.strip_prefix("ahead ") {
            ahead = n.trim().parse().unwrap_or(0);
        } else if let Some(n) = part.strip_prefix("behind ") {
            behind = n.trim().parse().unwrap_or(0);
        }
    }
    (ahead, behind)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{git, init_repo, TempDir};

    #[test]
    fn parse_line_reads_the_current_branch_with_no_upstream() {
        let branch = parse_line("main\x1f*\x1f\x1f").unwrap();
        assert_eq!(
            branch,
            Branch { name: "main".to_string(), current: true, upstream: None, ahead: 0, behind: 0, upstream_gone: false }
        );
    }

    #[test]
    fn parse_line_reads_a_non_current_branch() {
        let branch = parse_line("feature\x1f \x1f\x1f").unwrap();
        assert!(!branch.current);
    }

    #[test]
    fn parse_track_reads_ahead_and_behind_together() {
        assert_eq!(parse_track("[ahead 2, behind 3]"), (2, 3));
        assert_eq!(parse_track("[ahead 1]"), (1, 0));
        assert_eq!(parse_track("[behind 4]"), (0, 4));
        assert_eq!(parse_track(""), (0, 0));
        assert_eq!(parse_track("[gone]"), (0, 0));
    }

    #[test]
    fn parse_line_flags_an_upstream_that_no_longer_exists() {
        let branch = parse_line("feature\x1f \x1forigin/feature\x1f[gone]").unwrap();
        assert!(branch.upstream_gone);
        assert_eq!(branch.upstream.as_deref(), Some("origin/feature"));
        // Deliberately still 0/0 -- but `upstream_gone` is what stops
        // that reading as "in sync", which it very much isn't.
        assert_eq!((branch.ahead, branch.behind), (0, 0));
    }

    #[test]
    fn an_in_sync_branch_is_not_flagged_as_gone() {
        let branch = parse_line("main\x1f*\x1forigin/main\x1f").unwrap();
        assert!(!branch.upstream_gone);
        assert_eq!((branch.ahead, branch.behind), (0, 0));
    }

    #[test]
    fn list_remote_branches_and_tags_are_empty_outside_a_git_repo() {
        let dir = TempDir::new("refs_no_repo");
        assert!(list_remote_branches(dir.path()).is_empty());
        assert!(list_tags(dir.path()).is_empty());
    }

    #[test]
    fn list_tags_returns_both_lightweight_and_annotated_tags() {
        let dir = TempDir::new("refs_tags");
        init_repo(dir.path());
        dir.write("a.txt", "v1");
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-q", "-m", "initial"]);
        git(dir.path(), &["tag", "v1.0"]);
        git(dir.path(), &["tag", "-a", "v2.0", "-m", "annotated"]);

        let tags = list_tags(dir.path());
        assert!(tags.contains(&"v1.0".to_string()), "got {tags:?}");
        assert!(tags.contains(&"v2.0".to_string()), "got {tags:?}");
    }

    #[test]
    fn list_remote_branches_reports_a_real_tracking_ref_and_skips_its_head() {
        let remote = TempDir::new("refs_remote_bare");
        git(remote.path(), &["init", "-q", "--bare"]);

        let dir = TempDir::new("refs_remote_clone");
        init_repo(dir.path());
        git(dir.path(), &["remote", "add", "origin", remote.path().to_str().unwrap()]);
        dir.write("a.txt", "v1");
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-q", "-m", "first"]);
        git(dir.path(), &["push", "-q", "-u", "origin", "main"]);

        let remotes = list_remote_branches(dir.path());
        assert_eq!(remotes, vec!["origin/main".to_string()]);
    }

    #[test]
    fn list_remote_branches_never_reports_a_bare_remote_name() {
        // The real bug: a remote's HEAD symref is `refs/remotes/origin/
        // HEAD`, which `%(refname:short)` shortens all the way to
        // `origin` -- so it slipped past an `ends_with("/HEAD")` filter
        // and showed up in the ref list as if it were a branch.
        let remote = TempDir::new("refs_remote_head_bare");
        git(remote.path(), &["init", "-q", "--bare"]);

        let dir = TempDir::new("refs_remote_head_clone");
        init_repo(dir.path());
        git(dir.path(), &["remote", "add", "origin", remote.path().to_str().unwrap()]);
        dir.write("a.txt", "v1");
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-q", "-m", "first"]);
        git(dir.path(), &["push", "-q", "-u", "origin", "main"]);
        // Creates refs/remotes/origin/HEAD, exactly as a real clone has.
        git(dir.path(), &["remote", "set-head", "origin", "main"]);

        let remotes = list_remote_branches(dir.path());
        assert!(!remotes.contains(&"origin".to_string()), "a bare remote name is not a branch: {remotes:?}");
        assert_eq!(remotes, vec!["origin/main".to_string()]);
    }

    #[test]
    fn list_branches_empty_outside_a_git_repo() {
        let dir = TempDir::new("branch_no_repo");
        assert!(list_branches(dir.path()).is_empty());
    }

    #[test]
    fn list_branches_marks_the_checked_out_branch() {
        let dir = TempDir::new("branch_current");
        init_repo(dir.path());
        dir.write("a.txt", "hello");
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-q", "-m", "initial"]);
        git(dir.path(), &["branch", "feature"]);

        let branches = list_branches(dir.path());
        assert_eq!(branches.len(), 2);
        let main = branches.iter().find(|b| b.name == "main").unwrap();
        let feature = branches.iter().find(|b| b.name == "feature").unwrap();
        assert!(main.current);
        assert!(!feature.current);
    }

    #[test]
    fn list_branches_reports_ahead_against_a_real_upstream() {
        let remote = TempDir::new("branch_ahead_remote");
        git(remote.path(), &["init", "-q", "--bare"]);

        let dir = TempDir::new("branch_ahead_clone");
        init_repo(dir.path());
        git(dir.path(), &["remote", "add", "origin", remote.path().to_str().unwrap()]);
        dir.write("a.txt", "v1");
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-q", "-m", "first"]);
        git(dir.path(), &["push", "-q", "-u", "origin", "main"]);
        dir.write("a.txt", "v2");
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-q", "-m", "second"]);

        let branches = list_branches(dir.path());
        let main = branches.iter().find(|b| b.name == "main").unwrap();
        assert_eq!(main.upstream.as_deref(), Some("origin/main"));
        assert_eq!(main.ahead, 1);
        assert_eq!(main.behind, 0);
    }
}
