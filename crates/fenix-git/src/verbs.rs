//! The verbs behind the Git status page's menus. Each one is a pair: an
//! `*_args` function that says exactly which `git` command will run --
//! what the page's plans and the operation log show, and what the tests
//! read -- and the call that runs it.

use std::path::Path;

use crate::process::{run_action, run_lines};

fn strings(args: &[&str]) -> Vec<String> {
    args.iter().map(|a| a.to_string()).collect()
}

/// The commit menu's flags.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CommitFlags {
    /// `--all`: stage every tracked change first.
    pub all: bool,
    /// `--signoff`: add a `Signed-off-by` trailer.
    pub signoff: bool,
    /// `--no-verify`: skip the pre-commit and commit-msg hooks.
    pub no_verify: bool,
}

/// What a commit does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommitKind {
    /// A new commit with this message.
    New(String),
    /// Replaces the last commit: what's staged, and this message.
    Amend(String),
    /// Replaces the last commit with what's staged, keeping its message.
    Extend,
    /// Changes only the last commit's message; what's staged stays staged.
    Reword(String),
    /// A `fixup!` commit for the named commit, for autosquash to fold in.
    Fixup(String),
}

pub fn commit_args(kind: &CommitKind, flags: CommitFlags) -> Vec<String> {
    let mut args = strings(&["commit"]);
    match kind {
        CommitKind::New(message) => args.extend(["-m".to_string(), message.clone()]),
        CommitKind::Amend(message) => args.extend(["--amend".to_string(), "-m".to_string(), message.clone()]),
        CommitKind::Extend => args.extend(strings(&["--amend", "--no-edit"])),
        // `--only` with no paths commits none of the index: the message
        // is all that changes.
        CommitKind::Reword(message) => args.extend(["--amend".to_string(), "--only".to_string(), "-m".to_string(), message.clone()]),
        CommitKind::Fixup(hash) => args.push(format!("--fixup={hash}")),
    }
    if flags.all {
        args.push("--all".to_string());
    }
    if flags.signoff {
        args.push("--signoff".to_string());
    }
    if flags.no_verify {
        args.push("--no-verify".to_string());
    }
    args
}

pub fn commit_with(repo: &Path, kind: &CommitKind, flags: CommitFlags) -> Result<String, String> {
    run_action(repo, &commit_args(kind, flags))
}

/// The full message of `rev` (`HEAD` for the last commit) -- what amend
/// and reword start from.
pub fn commit_message(repo: &Path, rev: &str) -> Option<String> {
    let lines = run_lines(repo, &["log", "-1", "--format=%B", rev]);
    let text = lines.join("\n").trim_end().to_string();
    (!lines.is_empty()).then_some(text)
}

/// Whether `hash` is a root commit (it has no parent to rebase from).
fn is_root(repo: &Path, hash: &str) -> bool {
    run_lines(repo, &["rev-list", "--parents", "-n", "1", hash]).first().is_some_and(|l| l.split_whitespace().count() == 1)
}

/// Folds every `fixup!` commit into its target, starting from `hash`
/// (the oldest commit a fixup names). `--autostash` so uncommitted work
/// rides along instead of stopping the rebase before it starts; the
/// todo list is accepted as written because `GIT_SEQUENCE_EDITOR` is
/// defused for every command this crate runs.
pub fn autosquash_args(hash: &str, root: bool) -> Vec<String> {
    let mut args = strings(&["rebase", "-i", "--autosquash", "--autostash"]);
    if root {
        args.push("--root".to_string());
    } else {
        args.push(format!("{hash}~1"));
    }
    args
}

pub fn autosquash(repo: &Path, hash: &str) -> Result<String, String> {
    run_action(repo, &autosquash_args(hash, is_root(repo, hash)))
}

/// The push menu's choices, resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushOptions {
    pub remote: String,
    /// The branch on the remote; the local side is always `HEAD`.
    pub branch: String,
    /// `-u`: make this the upstream -- a new branch's first push.
    pub set_upstream: bool,
    /// Never a bare `--force`: with-lease refuses when the remote moved
    /// since the last fetch, so it can't discard someone else's work.
    pub force_with_lease: bool,
    pub dry_run: bool,
}

pub fn push_args(options: &PushOptions) -> Vec<String> {
    let mut args = strings(&["push"]);
    if options.set_upstream {
        args.push("-u".to_string());
    }
    if options.force_with_lease {
        args.push("--force-with-lease".to_string());
    }
    if options.dry_run {
        args.push("--dry-run".to_string());
    }
    args.push(options.remote.clone());
    args.push(format!("HEAD:refs/heads/{}", options.branch));
    args
}

pub fn push_with(repo: &Path, options: &PushOptions) -> Result<String, String> {
    run_action(repo, &push_args(options))
}

pub fn push_tags(repo: &Path, remote: &str) -> Result<String, String> {
    run_action(repo, &["push".to_string(), remote.to_string(), "--tags".to_string()])
}

/// `git pull --no-rebase`: merge what was fetched.
pub fn pull_merge(repo: &Path) -> Result<String, String> {
    run_action(repo, &strings(&["pull", "--no-rebase"]))
}

pub fn remotes(repo: &Path) -> Vec<String> {
    run_lines(repo, &["remote"])
}

/// Where a branch with no upstream would be pushed: the configured
/// `pushDefault`, else `origin`, else the only remote there is.
pub fn default_remote(repo: &Path) -> Option<String> {
    let all = remotes(repo);
    if let Some(configured) = run_lines(repo, &["config", "--get", "remote.pushDefault"]).into_iter().next() {
        if all.contains(&configured) {
            return Some(configured);
        }
    }
    if all.iter().any(|r| r == "origin") {
        return Some("origin".to_string());
    }
    all.into_iter().next()
}

/// How far `head` has gone past `base` and fallen behind it: (ahead,
/// behind). `None` when either doesn't resolve.
pub fn ahead_behind(repo: &Path, base: &str, head: &str) -> Option<(usize, usize)> {
    let line = run_lines(repo, &["rev-list", "--left-right", "--count", &format!("{base}...{head}")]).into_iter().next()?;
    let mut parts = line.split_whitespace().map(|n| n.parse::<usize>().ok());
    let behind = parts.next()??;
    let ahead = parts.next()??;
    Some((ahead, behind))
}

fn resolves(repo: &Path, rev: &str) -> bool {
    !run_lines(repo, &["rev-parse", "--verify", "--quiet", &format!("{rev}^{{commit}}")]).is_empty()
}

/// The branch this one is measured against: `configured` (`[git]
/// base_branch`) when it exists, else `main` or `master` -- on `origin`
/// when it's there, since that's what a pull request targets and a local
/// copy goes stale between pulls; else the local branch.
pub fn resolve_base(repo: &Path, configured: Option<&str>) -> Option<String> {
    let candidates = configured.into_iter().map(str::to_string).chain(["main".to_string(), "master".to_string()]);
    for name in candidates {
        let remote = format!("origin/{name}");
        if resolves(repo, &remote) {
            return Some(remote);
        }
        if resolves(repo, &name) {
            return Some(name);
        }
    }
    None
}

/// Whether `commit` is already contained in `rev` (an upstream, say).
pub fn contains(repo: &Path, rev: &str, commit: &str) -> bool {
    crate::process::run_status(repo, &["merge-base", "--is-ancestor", commit, rev])
}

/// The stash menu's choices.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StashOptions {
    pub message: Option<String>,
    /// `-u`: untracked files too.
    pub include_untracked: bool,
    /// `--staged`: only what's in the index.
    pub staged: bool,
    /// Only these paths.
    pub paths: Vec<String>,
}

pub fn stash_args(options: &StashOptions) -> Vec<String> {
    let mut args = strings(&["stash", "push"]);
    if options.include_untracked {
        args.push("-u".to_string());
    }
    if options.staged {
        args.push("--staged".to_string());
    }
    if let Some(message) = options.message.as_ref().filter(|m| !m.trim().is_empty()) {
        args.extend(["-m".to_string(), message.trim().to_string()]);
    }
    if !options.paths.is_empty() {
        args.push("--".to_string());
        args.extend(options.paths.iter().cloned());
    }
    args
}

pub fn stash_with(repo: &Path, options: &StashOptions) -> Result<String, String> {
    run_action(repo, &stash_args(options))
}

/// `git switch -c name [start]`, or just `git branch name [start]` when
/// `switch` is off.
pub fn branch_args(name: &str, start: Option<&str>, switch: bool) -> Vec<String> {
    let mut args = if switch { strings(&["switch", "-c", name]) } else { strings(&["branch", name]) };
    if let Some(start) = start {
        args.push(start.to_string());
    }
    args
}

pub fn create_branch_at(repo: &Path, name: &str, start: Option<&str>, switch: bool) -> Result<String, String> {
    run_action(repo, &branch_args(name, start, switch))
}

pub fn rename_branch(repo: &Path, old: &str, new: &str) -> Result<String, String> {
    run_action(repo, &strings(&["branch", "-m", old, new]))
}

pub fn tag(repo: &Path, name: &str, at: &str) -> Result<String, String> {
    run_action(repo, &strings(&["tag", name, at]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{git, init_repo, TempDir};

    fn repo(name: &str) -> TempDir {
        let dir = TempDir::new(name);
        init_repo(dir.path());
        dir.write("a.txt", "one\n");
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-q", "-m", "first"]);
        dir
    }

    fn log(dir: &Path) -> Vec<String> {
        run_lines(dir, &["log", "--format=%s"])
    }

    /// A file's text with line endings normalised -- `core.autocrlf` on
    /// Windows rewrites what git checks out.
    fn read(dir: &Path, name: &str) -> String {
        std::fs::read_to_string(dir.join(name)).unwrap().replace("\r\n", "\n")
    }

    fn staged(dir: &Path) -> Vec<String> {
        run_lines(dir, &["diff", "--cached", "--name-only"])
    }

    #[test]
    fn commit_flags_become_the_matching_options() {
        let flags = CommitFlags { all: true, signoff: true, no_verify: true };
        assert_eq!(commit_args(&CommitKind::New("m".into()), flags), ["commit", "-m", "m", "--all", "--signoff", "--no-verify"]);
        assert_eq!(commit_args(&CommitKind::Extend, CommitFlags::default()), ["commit", "--amend", "--no-edit"]);
        assert_eq!(commit_args(&CommitKind::Fixup("abc".into()), CommitFlags::default()), ["commit", "--fixup=abc"]);
    }

    #[test]
    fn amend_and_extend_replace_the_last_commit() {
        let dir = repo("verbs_amend");
        dir.write("b.txt", "two\n");
        git(dir.path(), &["add", "b.txt"]);
        commit_with(dir.path(), &CommitKind::Extend, CommitFlags::default()).unwrap();
        assert_eq!(log(dir.path()), ["first"], "extend keeps the message and adds no commit");
        assert!(run_lines(dir.path(), &["show", "--name-only", "--format="]).contains(&"b.txt".to_string()));

        commit_with(dir.path(), &CommitKind::Amend("first, better".into()), CommitFlags::default()).unwrap();
        assert_eq!(log(dir.path()), ["first, better"]);
        assert_eq!(commit_message(dir.path(), "HEAD").as_deref(), Some("first, better"));
    }

    #[test]
    fn reword_changes_the_message_and_leaves_the_index_alone() {
        let dir = repo("verbs_reword");
        dir.write("a.txt", "one\nmore\n");
        git(dir.path(), &["add", "a.txt"]);
        commit_with(dir.path(), &CommitKind::Reword("renamed".into()), CommitFlags::default()).unwrap();
        assert_eq!(log(dir.path()), ["renamed"]);
        assert_eq!(staged(dir.path()), ["a.txt"], "the staged change wasn't swallowed into the commit");
    }

    #[test]
    fn a_fixup_folds_into_an_older_commit_and_keeps_uncommitted_work() {
        let dir = repo("verbs_fixup");
        dir.write("b.txt", "two\n");
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-q", "-m", "second"]);
        dir.write("c.txt", "three\n");
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-q", "-m", "third"]);
        let second = run_lines(dir.path(), &["rev-parse", "HEAD~1"]).remove(0);

        dir.write("b.txt", "two, fixed\n");
        git(dir.path(), &["add", "b.txt"]);
        dir.write("a.txt", "one, still editing\n");
        commit_with(dir.path(), &CommitKind::Fixup(second.clone()), CommitFlags::default()).unwrap();
        assert_eq!(log(dir.path())[0], "fixup! second");

        autosquash(dir.path(), &second).unwrap();
        assert_eq!(log(dir.path()), ["third", "second", "first"]);
        let b = run_lines(dir.path(), &["show", "HEAD~1:b.txt"]);
        assert_eq!(b, ["two, fixed"], "the fix is in the commit it named");
        assert_eq!(read(dir.path(), "a.txt"), "one, still editing\n", "autostash kept the edit");
    }

    #[test]
    fn a_fixup_of_the_root_commit_rebases_from_the_root() {
        let dir = repo("verbs_fixup_root");
        let root = run_lines(dir.path(), &["rev-parse", "HEAD"]).remove(0);
        assert_eq!(autosquash_args(&root, true).last().unwrap(), "--root");
        dir.write("a.txt", "one, fixed\n");
        git(dir.path(), &["add", "."]);
        commit_with(dir.path(), &CommitKind::Fixup(root.clone()), CommitFlags::default()).unwrap();
        autosquash(dir.path(), &root).unwrap();
        assert_eq!(log(dir.path()), ["first"]);
    }

    #[test]
    fn a_first_push_sets_the_upstream_and_a_rewrite_needs_the_lease() {
        let remote = TempDir::new("verbs_push_remote");
        git(remote.path(), &["init", "-q", "--bare"]);
        let dir = repo("verbs_push");
        git(dir.path(), &["remote", "add", "origin", remote.path().to_str().unwrap()]);
        assert_eq!(default_remote(dir.path()).as_deref(), Some("origin"));

        let first = PushOptions { remote: "origin".into(), branch: "main".into(), set_upstream: true, force_with_lease: false, dry_run: false };
        assert_eq!(push_args(&first), ["push", "-u", "origin", "HEAD:refs/heads/main"]);
        push_with(dir.path(), &first).unwrap();
        assert_eq!(crate::status::status(dir.path()).unwrap().upstream.as_deref(), Some("origin/main"));

        commit_with(dir.path(), &CommitKind::Amend("first, rewritten".into()), CommitFlags::default()).unwrap();
        let plain = PushOptions { set_upstream: false, ..first.clone() };
        assert!(push_with(dir.path(), &plain).is_err(), "a rewrite is rejected without the lease");
        push_with(dir.path(), &PushOptions { force_with_lease: true, ..plain }).unwrap();
        assert_eq!(run_lines(remote.path(), &["log", "--format=%s", "main"]), ["first, rewritten"]);
    }

    #[test]
    fn ahead_and_behind_are_counted_against_the_base() {
        let dir = repo("verbs_ahead");
        git(dir.path(), &["switch", "-q", "-c", "topic"]);
        for n in 0..2 {
            dir.write("t.txt", &format!("{n}\n"));
            git(dir.path(), &["add", "."]);
            git(dir.path(), &["commit", "-q", "-m", "topic"]);
        }
        git(dir.path(), &["switch", "-q", "main"]);
        dir.write("m.txt", "m\n");
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-q", "-m", "main moved"]);
        assert_eq!(ahead_behind(dir.path(), "main", "topic"), Some((2, 1)));
        assert_eq!(ahead_behind(dir.path(), "nope", "topic"), None);
    }

    #[test]
    fn the_base_is_the_configured_branch_else_main_or_master() {
        let dir = repo("verbs_base");
        assert_eq!(resolve_base(dir.path(), None).as_deref(), Some("main"));
        assert_eq!(resolve_base(dir.path(), Some("develop")).as_deref(), Some("main"), "a missing configured base falls back");
        git(dir.path(), &["branch", "develop"]);
        assert_eq!(resolve_base(dir.path(), Some("develop")).as_deref(), Some("develop"));
        // With a remote, its copy wins: the local one goes stale.
        let remote = TempDir::new("verbs_base_remote");
        git(remote.path(), &["init", "-q", "--bare"]);
        git(dir.path(), &["remote", "add", "origin", remote.path().to_str().unwrap()]);
        git(dir.path(), &["push", "-q", "origin", "main"]);
        assert_eq!(resolve_base(dir.path(), None).as_deref(), Some("origin/main"));
    }

    #[test]
    fn stash_options_pick_what_is_stashed() {
        let dir = repo("verbs_stash");
        dir.write("a.txt", "one, staged\n");
        git(dir.path(), &["add", "a.txt"]);
        dir.write("new.txt", "untracked\n");
        let options = StashOptions { message: Some("wip".into()), include_untracked: true, ..Default::default() };
        assert_eq!(stash_args(&options), ["stash", "push", "-u", "-m", "wip"]);
        stash_with(dir.path(), &options).unwrap();
        assert!(!dir.path().join("new.txt").exists(), "untracked went into the stash");
        assert!(run_lines(dir.path(), &["stash", "list"])[0].ends_with("wip"));

        git(dir.path(), &["stash", "pop", "-q"]);
        dir.write("b.txt", "b\n");
        let only_a = StashOptions { paths: vec!["a.txt".into()], ..Default::default() };
        stash_with(dir.path(), &only_a).unwrap();
        assert_eq!(read(dir.path(), "a.txt"), "one\n");
        assert!(dir.path().join("b.txt").exists(), "a path-limited stash leaves other files alone");
    }

    #[test]
    fn branches_are_created_renamed_and_tagged() {
        let dir = repo("verbs_branch");
        create_branch_at(dir.path(), "topic", None, true).unwrap();
        assert_eq!(crate::status::status(dir.path()).unwrap().branch, "topic");
        rename_branch(dir.path(), "topic", "feature/topic").unwrap();
        assert_eq!(crate::status::status(dir.path()).unwrap().branch, "feature/topic");
        create_branch_at(dir.path(), "later", Some("HEAD"), false).unwrap();
        assert_eq!(crate::status::status(dir.path()).unwrap().branch, "feature/topic", "without switch it stays put");
        tag(dir.path(), "v1", "HEAD").unwrap();
        assert!(contains(dir.path(), "v1", "HEAD"));
    }
}
