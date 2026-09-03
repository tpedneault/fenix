//! Whether the repo is in the middle of a multi-step operation, and how
//! far through it is.
//!
//! Read from the marker files git itself maintains under `.git/` rather
//! than by parsing `git status`'s prose, which is localized and has
//! changed wording between versions. These paths are part of git's
//! documented repository layout (`gitrepository-layout(7)`), and the
//! same ones git's own shell prompt script reads to decide what to show.
//!
//! This is what lets the UI say "REBASING 2/5 -- R continue, A abort"
//! instead of leaving someone stranded mid-rebase wondering why commits
//! aren't landing.

use std::path::Path;

/// A multi-step git operation that's currently suspended, usually
/// because it hit a conflict and is waiting for the working tree to be
/// fixed up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InProgress {
    /// `step`/`total` when git recorded them (an interactive or
    /// multi-commit rebase); `None` for a rebase with nothing to count.
    Rebase { step: Option<(usize, usize)> },
    Merge,
    CherryPick,
    Revert,
    Bisect,
}

impl InProgress {
    /// A short banner label, e.g. `REBASING 2/5`.
    pub fn label(&self) -> String {
        match self {
            InProgress::Rebase { step: Some((n, total)) } => format!("REBASING {n}/{total}"),
            InProgress::Rebase { step: None } => "REBASING".to_string(),
            InProgress::Merge => "MERGING".to_string(),
            InProgress::CherryPick => "CHERRY-PICKING".to_string(),
            InProgress::Revert => "REVERTING".to_string(),
            InProgress::Bisect => "BISECTING".to_string(),
        }
    }
}

/// What `repo` is in the middle of, if anything.
///
/// Checked in git's own precedence order: a rebase can be running while
/// a `MERGE_HEAD` exists (a conflicted rebase step *is* a merge), so
/// rebase is tested first, or every conflicted rebase would report
/// itself as a plain merge and offer the wrong keys to get out of it.
pub fn in_progress(repo: &Path) -> Option<InProgress> {
    let git_dir = repo.join(".git");
    // `rebase-merge` is the modern (and interactive) rebase backend;
    // `rebase-apply` is the older `am`-based one, still used by
    // `--apply` and by `git am` itself.
    if git_dir.join("rebase-merge").is_dir() || git_dir.join("rebase-apply").is_dir() {
        return Some(InProgress::Rebase { step: rebase_step(&git_dir) });
    }
    if git_dir.join("MERGE_HEAD").is_file() {
        return Some(InProgress::Merge);
    }
    if git_dir.join("CHERRY_PICK_HEAD").is_file() {
        return Some(InProgress::CherryPick);
    }
    if git_dir.join("REVERT_HEAD").is_file() {
        return Some(InProgress::Revert);
    }
    if git_dir.join("BISECT_LOG").is_file() {
        return Some(InProgress::Bisect);
    }
    None
}

/// `msgnum`/`end` (rebase-merge) or `next`/`last` (rebase-apply) -- the
/// step counter git writes for a multi-commit rebase. Absent for a
/// single-step one, which is why this is an `Option` rather than a
/// default of `1/1`: claiming a step count git didn't report would be
/// making one up.
fn rebase_step(git_dir: &Path) -> Option<(usize, usize)> {
    for (dir, current, total) in [("rebase-merge", "msgnum", "end"), ("rebase-apply", "next", "last")] {
        let base = git_dir.join(dir);
        let read = |name: &str| std::fs::read_to_string(base.join(name)).ok()?.trim().parse::<usize>().ok();
        if let (Some(n), Some(total)) = (read(current), read(total)) {
            return Some((n, total));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{git, init_repo, TempDir};

    /// A repo where `main` and `side` both changed the same line, so
    /// rebasing or merging one onto the other is guaranteed to conflict.
    fn conflicting_repo(name: &str) -> TempDir {
        let dir = TempDir::new(name);
        init_repo(dir.path());
        dir.write("a.txt", "original\n");
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-q", "-m", "initial"]);
        git(dir.path(), &["checkout", "-q", "-b", "side"]);
        dir.write("a.txt", "side version\n");
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-q", "-m", "side change"]);
        git(dir.path(), &["checkout", "-q", "main"]);
        dir.write("a.txt", "main version\n");
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-q", "-m", "main change"]);
        dir
    }

    #[test]
    fn a_clean_repo_is_not_in_the_middle_of_anything() {
        let dir = conflicting_repo("state_clean");
        assert_eq!(in_progress(dir.path()), None);
    }

    #[test]
    fn a_directory_that_is_not_a_repo_reports_nothing_rather_than_failing() {
        let dir = TempDir::new("state_no_repo");
        assert_eq!(in_progress(dir.path()), None);
    }

    #[test]
    fn a_conflicted_merge_is_reported_as_merging() {
        let dir = conflicting_repo("state_merging");
        // Conflicts, so it exits non-zero and leaves MERGE_HEAD behind.
        let _ = crate::actions::merge(dir.path(), "side");
        assert_eq!(in_progress(dir.path()), Some(InProgress::Merge));
        assert_eq!(in_progress(dir.path()).unwrap().label(), "MERGING");
    }

    #[test]
    fn a_conflicted_rebase_is_reported_as_rebasing_not_as_merging() {
        // The precedence that matters: a conflicted rebase step also has
        // a MERGE_HEAD, and reporting it as a merge would offer keys
        // that can't finish it.
        let dir = conflicting_repo("state_rebasing");
        let _ = crate::actions::rebase(dir.path(), "side");
        match in_progress(dir.path()) {
            Some(InProgress::Rebase { .. }) => {}
            other => panic!("expected a rebase in progress, got {other:?}"),
        }
        assert!(in_progress(dir.path()).unwrap().label().starts_with("REBASING"));
    }

    #[test]
    fn aborting_clears_the_in_progress_state() {
        let dir = conflicting_repo("state_abort");
        let _ = crate::actions::merge(dir.path(), "side");
        assert!(in_progress(dir.path()).is_some());

        crate::actions::merge_abort(dir.path()).expect("aborting a conflicted merge should succeed");
        assert_eq!(in_progress(dir.path()), None);
    }

    #[test]
    fn the_rebase_label_includes_a_step_count_when_git_reports_one() {
        assert_eq!(InProgress::Rebase { step: Some((2, 5)) }.label(), "REBASING 2/5");
        assert_eq!(InProgress::Rebase { step: None }.label(), "REBASING");
    }
}
