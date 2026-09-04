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

/// Who "ours" and "theirs" actually are in the conflict markers, in
/// terms of branches rather than git's own point of view.
///
/// This exists because of the single most reliable way to resolve a
/// conflict backwards. During a **rebase**, git replays your commits on
/// top of the target branch, so at each step `HEAD` is *the target* and
/// the commit being applied is "theirs" -- meaning `<<<<<<< HEAD` is
/// the branch you're rebasing onto, and `>>>>>>>` is your own work.
/// That's the opposite of what almost everyone expects, and picking
/// "ours" to mean "keep my changes" silently throws those changes away.
///
/// During a merge or a cherry-pick the sides read the intuitive way
/// round: `HEAD` is the branch you're on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConflictSides {
    /// What `<<<<<<< HEAD` is, named: a branch where one can be found,
    /// else a short hash.
    pub ours: String,
    /// What `>>>>>>>` is, named the same way.
    pub theirs: String,
    /// Why that side is there, in the user's terms -- "the branch
    /// you're rebasing onto", "your commit being replayed".
    pub ours_role: &'static str,
    pub theirs_role: &'static str,
}

/// `ConflictSides` for whatever `repo` is in the middle of, or `None`
/// when it isn't in the middle of anything.
///
/// Names come from `.git`'s own operation state -- `rebase-merge/onto`
/// and `head-name`, `MERGE_HEAD`, `CHERRY_PICK_HEAD` -- resolved to
/// branch names with `git name-rev`. A hash that no ref points at stays
/// a hash rather than being dressed up as something it isn't.
pub fn conflict_sides(repo: &Path) -> Option<ConflictSides> {
    let git_dir = repo.join(".git");
    let read = |rel: &str| std::fs::read_to_string(git_dir.join(rel)).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    let current = || {
        crate::process::run_lines(repo, &["rev-parse", "--abbrev-ref", "HEAD"])
            .into_iter()
            .next()
            .filter(|b| b != "HEAD")
            .unwrap_or_else(|| "HEAD".to_string())
    };

    match in_progress(repo)? {
        InProgress::Rebase { .. } => {
            // `head-name` is the branch being rebased (yours); `onto` is
            // the commit it's being replayed onto (the target).
            let onto = read("rebase-merge/onto").or_else(|| read("rebase-apply/onto"));
            let head_name = read("rebase-merge/head-name").or_else(|| read("rebase-apply/head-name"));
            Some(ConflictSides {
                ours: onto.map(|o| name_rev(repo, &o)).unwrap_or_else(|| "the rebase target".to_string()),
                theirs: head_name.map(|h| short_ref(&h)).unwrap_or_else(current),
                ours_role: "the branch you are rebasing onto",
                theirs_role: "your own commit being replayed",
            })
        }
        InProgress::Merge => Some(ConflictSides {
            ours: current(),
            theirs: read("MERGE_HEAD").map(|h| name_rev(repo, &h)).unwrap_or_else(|| "the merged branch".to_string()),
            ours_role: "the branch you are on",
            theirs_role: "the branch being merged in",
        }),
        InProgress::CherryPick => Some(ConflictSides {
            ours: current(),
            theirs: read("CHERRY_PICK_HEAD").map(|h| name_rev(repo, &h)).unwrap_or_else(|| "the picked commit".to_string()),
            ours_role: "the branch you are on",
            theirs_role: "the commit being picked",
        }),
        InProgress::Revert => Some(ConflictSides {
            ours: current(),
            theirs: read("REVERT_HEAD").map(|h| name_rev(repo, &h)).unwrap_or_else(|| "the reverted commit".to_string()),
            ours_role: "the branch you are on",
            theirs_role: "the commit being reverted",
        }),
        // Bisect checks out commits but never produces conflicts.
        InProgress::Bisect => None,
    }
}

/// `refs/heads/foo` -> `foo`, anything else unchanged.
fn short_ref(name: &str) -> String {
    name.strip_prefix("refs/heads/").or_else(|| name.strip_prefix("refs/remotes/")).unwrap_or(name).to_string()
}

/// A branch name for `sha` where one exists, else the short hash.
///
/// `name-rev` can answer with a relative name (`develop~2`, or
/// `undefined` for a commit no ref reaches); those describe a position
/// rather than name a branch, so they're rejected in favour of the hash,
/// which at least doesn't claim to be something it isn't.
fn name_rev(repo: &Path, sha: &str) -> String {
    let short = sha.chars().take(7).collect::<String>();
    let lines = crate::process::run_lines(repo, &["name-rev", "--name-only", "--refs=refs/heads/*", "--refs=refs/remotes/*", sha]);
    match lines.into_iter().next() {
        Some(name) if name != "undefined" && !name.contains('~') && !name.contains('^') => short_ref(&name),
        _ => short,
    }
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

    /// The conflicting step of a rebase/merge exits non-zero by design,
    /// so it can't go through `test_util::git`, which asserts success.
    fn git_expecting_conflict(dir: &std::path::Path, args: &[&str]) {
        std::process::Command::new("git").current_dir(dir).args(args).output().expect("run git");
    }

    /// Sets up `base` -> `develop` and `base` -> `myfeature`, each
    /// changing the same line, so rebasing one onto the other conflicts.
    fn diverged(name: &str) -> TempDir {
        let dir = TempDir::new(name);
        init_repo(dir.path());
        dir.write("f.txt", "a
v=base
");
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-q", "-m", "init"]);
        git(dir.path(), &["checkout", "-q", "-b", "develop"]);
        dir.write("f.txt", "a
v=DEVELOP
");
        git(dir.path(), &["commit", "-q", "-am", "dev"]);
        git(dir.path(), &["checkout", "-q", "-b", "myfeature", "HEAD~1"]);
        dir.write("f.txt", "a
v=FEATURE
");
        git(dir.path(), &["commit", "-q", "-am", "feat"]);
        dir
    }

    /// The whole reason `conflict_sides` exists: during a rebase git's
    /// `HEAD` is the branch you're rebasing *onto*, not yours, so
    /// "ours" means the target and "theirs" means your own work. Anyone
    /// reading the raw markers and picking "ours" to mean "keep mine"
    /// throws their own commit away.
    #[test]
    fn a_rebase_names_the_target_as_ours_and_your_branch_as_theirs() {
        let dir = diverged("sides_rebase");
        git_expecting_conflict(dir.path(), &["rebase", "develop"]);

        let sides = conflict_sides(dir.path()).expect("a conflicted rebase is in progress");
        assert_eq!(sides.ours, "develop", "`<<<<<<< HEAD` is the rebase target");
        assert_eq!(sides.theirs, "myfeature", "and `>>>>>>>` is your own branch");
        assert_eq!(sides.ours_role, "the branch you are rebasing onto");
    }

    #[test]
    fn a_merge_names_the_sides_the_intuitive_way_round() {
        let dir = diverged("sides_merge");
        git(dir.path(), &["checkout", "-q", "develop"]);
        git_expecting_conflict(dir.path(), &["merge", "myfeature"]);

        let sides = conflict_sides(dir.path()).expect("a conflicted merge is in progress");
        assert_eq!(sides.ours, "develop", "the branch you are on");
        assert_eq!(sides.theirs, "myfeature", "the one being merged in");
        assert_eq!(sides.theirs_role, "the branch being merged in");
    }

    #[test]
    fn a_clean_repo_has_no_sides_to_name() {
        let dir = TempDir::new("sides_clean");
        init_repo(dir.path());
        assert_eq!(conflict_sides(dir.path()), None);
    }

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
