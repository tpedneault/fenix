use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::entry::{Entry, GitStatus};

/// Runs `git status --porcelain=v1 --ignored -- .` in `dir` and returns a
/// map from absolute path to status, for every path git reports as
/// changed/untracked/ignored/conflicted anywhere under `dir` (`git
/// status` is always recursive regardless of how deep the directory
/// listing itself goes). Empty map -- not an error -- outside a git repo
/// or without `git` on `PATH`; git status is an annotation on top of the
/// listing, not something the listing should fail over.
pub(crate) fn status_for_dir(dir: &Path) -> HashMap<PathBuf, GitStatus> {
    let Ok(output) =
        Command::new("git").args(["status", "--porcelain=v1", "--ignored", "--", "."]).current_dir(dir).output()
    else {
        return HashMap::new();
    };
    if !output.status.success() {
        return HashMap::new();
    }
    let stdout = String::from_utf8_lossy(&output.stdout);

    let mut map = HashMap::new();
    for line in stdout.lines() {
        if line.len() < 4 {
            continue;
        }
        let Some(status) = parse_status(&line[0..2]) else { continue };
        let rest = &line[3..];
        // Renames report "OLD -> NEW"; the path that exists now is what matters.
        let rel = rest.rsplit_once(" -> ").map_or(rest, |(_, new)| new).trim_matches('"');
        map.insert(dir.join(rel), status);
    }
    map
}

/// Parses porcelain v1's two-char `XY` status code (index/worktree
/// columns). Staged (non-space index column) takes priority for display
/// over a merely-modified worktree, since staged is usually the more
/// actionable state to surface.
fn parse_status(xy: &str) -> Option<GitStatus> {
    let mut chars = xy.chars();
    let x = chars.next()?;
    let y = chars.next()?;
    match (x, y) {
        ('?', '?') => Some(GitStatus::Untracked),
        ('!', '!') => Some(GitStatus::Ignored),
        _ if x == 'U' || y == 'U' || (x, y) == ('A', 'A') || (x, y) == ('D', 'D') => Some(GitStatus::Conflicted),
        (' ', ' ') => None,
        (' ', _) => Some(GitStatus::Modified),
        _ => Some(GitStatus::Staged),
    }
}

/// Applies `statuses` onto a flat entry list: an exact path match wins;
/// a directory with no exact match but *some* changed path nested under
/// it is annotated `Modified` too, so a subdirectory containing changes
/// still shows a badge even when it's collapsed and the listing doesn't
/// show what's inside it.
pub(crate) fn annotate_entries(entries: &mut [Entry], statuses: &HashMap<PathBuf, GitStatus>) {
    for entry in entries.iter_mut() {
        entry.git_status = statuses.get(&entry.path).copied().or_else(|| {
            if entry.is_dir && statuses.keys().any(|p| p != &entry.path && p.starts_with(&entry.path)) {
                Some(GitStatus::Modified)
            } else {
                None
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::TempDir;
    use std::process::Command;

    fn git(dir: &Path, args: &[&str]) {
        let status = Command::new("git").args(args).current_dir(dir).status().expect("run git");
        assert!(status.success(), "git {args:?} failed");
    }

    fn init_repo(dir: &Path) {
        git(dir, &["init", "-q"]);
        git(dir, &["config", "user.email", "test@example.com"]);
        git(dir, &["config", "user.name", "Test"]);
    }

    #[test]
    fn empty_map_outside_a_git_repo() {
        let dir = TempDir::new("git_status_no_repo");
        dir.touch("a.txt");
        let statuses = status_for_dir(dir.path());
        assert!(statuses.is_empty());
    }

    #[test]
    fn detects_untracked_modified_and_staged_files() {
        let dir = TempDir::new("git_status_states");
        init_repo(dir.path());
        dir.write("committed.txt", "v1");
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-q", "-m", "initial"]);

        dir.write("committed.txt", "v2"); // modified, unstaged
        dir.write("staged.txt", "new"); // will be staged
        git(dir.path(), &["add", "staged.txt"]);
        dir.write("untracked.txt", "new"); // never added

        let statuses = status_for_dir(dir.path());
        assert_eq!(statuses.get(&dir.path().join("committed.txt")), Some(&GitStatus::Modified));
        assert_eq!(statuses.get(&dir.path().join("staged.txt")), Some(&GitStatus::Staged));
        assert_eq!(statuses.get(&dir.path().join("untracked.txt")), Some(&GitStatus::Untracked));
    }

    #[test]
    fn annotate_entries_marks_a_directory_containing_nested_changes() {
        let mut entries = vec![Entry {
            name: "sub".to_string(),
            path: PathBuf::from("/repo/sub"),
            is_dir: true,
            size: 0,
            modified: std::time::SystemTime::UNIX_EPOCH,
            depth: 0,
            git_status: None,
        }];
        let mut statuses = HashMap::new();
        statuses.insert(PathBuf::from("/repo/sub/nested.txt"), GitStatus::Modified);

        annotate_entries(&mut entries, &statuses);
        assert_eq!(entries[0].git_status, Some(GitStatus::Modified));
    }

    #[test]
    fn annotate_entries_leaves_unrelated_entries_untouched() {
        let mut entries = vec![Entry {
            name: "clean.txt".to_string(),
            path: PathBuf::from("/repo/clean.txt"),
            is_dir: false,
            size: 0,
            modified: std::time::SystemTime::UNIX_EPOCH,
            depth: 0,
            git_status: None,
        }];
        let statuses = HashMap::new();
        annotate_entries(&mut entries, &statuses);
        assert_eq!(entries[0].git_status, None);
    }
}
