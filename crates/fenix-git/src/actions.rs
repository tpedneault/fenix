use std::path::Path;

use crate::process::run_action;

fn stage_args(path: &str) -> Vec<String> {
    vec!["add".to_string(), "--".to_string(), path.to_string()]
}

fn unstage_args(path: &str) -> Vec<String> {
    vec!["restore".to_string(), "--staged".to_string(), "--".to_string(), path.to_string()]
}

fn discard_tracked_args(path: &str) -> Vec<String> {
    vec!["checkout".to_string(), "--".to_string(), path.to_string()]
}

/// Untracked files aren't affected by `checkout` at all (there's nothing
/// in the index/`HEAD` for it to restore) -- `clean` is the actual
/// "delete this untracked file" primitive, so `discard_file`'s
/// `untracked` flag picks between the two rather than one command
/// covering both.
fn discard_untracked_args(path: &str) -> Vec<String> {
    vec!["clean".to_string(), "-f".to_string(), "--".to_string(), path.to_string()]
}

fn commit_args(message: &str) -> Vec<String> {
    vec!["commit".to_string(), "-m".to_string(), message.to_string()]
}

fn delete_branch_args(name: &str, force: bool) -> Vec<String> {
    vec!["branch".to_string(), if force { "-D" } else { "-d" }.to_string(), name.to_string()]
}

fn stash_ref(index: usize) -> String {
    format!("stash@{{{index}}}")
}

pub fn stage_file(repo: &Path, path: &str) -> Result<String, String> {
    run_action(repo, &stage_args(path))
}

pub fn unstage_file(repo: &Path, path: &str) -> Result<String, String> {
    run_action(repo, &unstage_args(path))
}

pub fn stage_all(repo: &Path) -> Result<String, String> {
    run_action(repo, &["add".to_string(), "-A".to_string()])
}

pub fn unstage_all(repo: &Path) -> Result<String, String> {
    run_action(repo, &["restore".to_string(), "--staged".to_string(), ".".to_string()])
}

pub fn discard_file(repo: &Path, path: &str, untracked: bool) -> Result<String, String> {
    let args = if untracked { discard_untracked_args(path) } else { discard_tracked_args(path) };
    run_action(repo, &args)
}

/// Directory-scoped discard. Unlike `discard_file`'s single-file
/// tracked/untracked branch (a lone file is always purely one or the
/// other), a directory routinely holds *both* kinds of change at once
/// -- a full discard needs `checkout --` (restores every tracked file
/// under it) followed by `clean -fd --` (removes every untracked file
/// and subdirectory under it). `checkout`'s own error when nothing
/// tracked needed restoring under the path is expected in the common
/// case (an all-untracked directory) and silently ignored; `clean`'s
/// result is the one returned, since it should never fail under normal
/// conditions and is the more useful thing to surface if it does.
pub fn discard_dir(repo: &Path, path: &str) -> Result<String, String> {
    let _ = run_action(repo, &discard_tracked_args(path));
    run_action(repo, &["clean".to_string(), "-fd".to_string(), "--".to_string(), path.to_string()])
}

pub fn commit(repo: &Path, message: &str) -> Result<String, String> {
    run_action(repo, &commit_args(message))
}

pub fn push(repo: &Path) -> Result<String, String> {
    run_action(repo, &["push".to_string()])
}

pub fn pull(repo: &Path) -> Result<String, String> {
    run_action(repo, &["pull".to_string()])
}

/// `git fetch --all --prune` -- refreshes every remote's tracking refs
/// and drops the ones whose remote branch is gone.
///
/// `--prune` is the whole point of doing this from an editor rather than
/// leaving it to a background `git fetch`: without it, branches deleted
/// after a merge request landed linger in `refs/remotes` forever and the
/// refs list slowly fills with things that no longer exist. Pruning is
/// what makes a local branch's upstream correctly report `[gone]` (see
/// `Branch::upstream_gone`) instead of looking merely up to date.
///
/// Touches the network, so callers run it off the input thread -- it's
/// the one operation in this crate that can take seconds.
pub fn fetch(repo: &Path) -> Result<String, String> {
    run_action(repo, &["fetch".to_string(), "--all".to_string(), "--prune".to_string()])
}

/// How long ago this repo last fetched, in seconds -- the mtime of
/// `.git/FETCH_HEAD`, which git rewrites on every fetch (including one
/// that turned out to have nothing to download).
///
/// `None` when the repo has never fetched, when `.git` is a *file*
/// rather than a directory (a worktree or submodule, where the real git
/// dir lives elsewhere -- resolving that is more machinery than a
/// staleness hint is worth), or when the clock says the file is from the
/// future. Purely informational: it answers "is what I'm looking at
/// current?", and answering "unknown" is fine.
pub fn seconds_since_fetch(repo: &Path) -> Option<u64> {
    let modified = std::fs::metadata(repo.join(".git").join("FETCH_HEAD")).ok()?.modified().ok()?;
    std::time::SystemTime::now().duration_since(modified).ok().map(|d| d.as_secs())
}

pub fn checkout_branch(repo: &Path, name: &str) -> Result<String, String> {
    run_action(repo, &["checkout".to_string(), name.to_string()])
}

pub fn create_branch(repo: &Path, name: &str) -> Result<String, String> {
    run_action(repo, &["checkout".to_string(), "-b".to_string(), name.to_string()])
}

/// `force` (`-D`) is for the caller's confirmation prompt to gate on --
/// this function itself has no opinion, same as `fenix-docker::
/// remove_container` not itself confirming a destructive action.
pub fn delete_branch(repo: &Path, name: &str, force: bool) -> Result<String, String> {
    run_action(repo, &delete_branch_args(name, force))
}

pub fn stash_push(repo: &Path) -> Result<String, String> {
    run_action(repo, &["stash".to_string(), "push".to_string()])
}

pub fn stash_pop(repo: &Path, index: usize) -> Result<String, String> {
    run_action(repo, &["stash".to_string(), "pop".to_string(), stash_ref(index)])
}

pub fn stash_apply(repo: &Path, index: usize) -> Result<String, String> {
    run_action(repo, &["stash".to_string(), "apply".to_string(), stash_ref(index)])
}

pub fn stash_drop(repo: &Path, index: usize) -> Result<String, String> {
    run_action(repo, &["stash".to_string(), "drop".to_string(), stash_ref(index)])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::files::list_files;
    use crate::test_util::{git, init_repo, TempDir};

    fn committed_repo(name: &str) -> TempDir {
        let dir = TempDir::new(name);
        init_repo(dir.path());
        dir.write("a.txt", "v1\n");
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-q", "-m", "initial"]);
        dir
    }

    #[test]
    fn stage_then_unstage_a_file() {
        let dir = committed_repo("actions_stage_unstage");
        dir.write("a.txt", "v2\n");

        stage_file(dir.path(), "a.txt").unwrap();
        let files = list_files(dir.path());
        assert_eq!(files[0].index_status, 'M');
        assert_eq!(files[0].worktree_status, '.');

        unstage_file(dir.path(), "a.txt").unwrap();
        let files = list_files(dir.path());
        assert_eq!(files[0].index_status, '.');
        assert_eq!(files[0].worktree_status, 'M');
    }

    #[test]
    fn stage_all_and_unstage_all() {
        let dir = committed_repo("actions_stage_all");
        dir.write("a.txt", "v2\n");
        dir.write("b.txt", "new\n");

        stage_all(dir.path()).unwrap();
        assert!(list_files(dir.path()).iter().all(|f| f.index_status != '.' && f.index_status != '?'));

        unstage_all(dir.path()).unwrap();
        assert!(list_files(dir.path()).iter().all(|f| f.index_status == '.' || f.index_status == '?'));
    }

    #[test]
    fn discard_file_reverts_a_tracked_modification() {
        let dir = committed_repo("actions_discard_tracked");
        dir.write("a.txt", "changed\n");
        discard_file(dir.path(), "a.txt", false).unwrap();
        assert_eq!(std::fs::read_to_string(dir.path().join("a.txt")).unwrap(), "v1\n");
    }

    #[test]
    fn discard_file_removes_an_untracked_file() {
        let dir = committed_repo("actions_discard_untracked");
        let path = dir.write("new.txt", "new\n");
        discard_file(dir.path(), "new.txt", true).unwrap();
        assert!(!path.exists());
    }

    #[test]
    fn stage_and_unstage_accept_a_directory_path() {
        let dir = committed_repo("actions_stage_unstage_dir");
        dir.write("sub/a.txt", "changed\n");
        dir.write("sub/b.txt", "new\n");
        git(dir.path(), &["add", "sub/a.txt"]);
        git(dir.path(), &["commit", "-q", "-m", "add sub/a.txt"]);
        dir.write("sub/a.txt", "changed again\n");

        stage_file(dir.path(), "sub").unwrap();
        let files = list_files(dir.path());
        assert!(files.iter().all(|f| f.index_status != '.'), "every file under sub/ should be staged: {files:?}");

        unstage_file(dir.path(), "sub").unwrap();
        let files = list_files(dir.path());
        // `b.txt` is untracked, so "unstaged" for it means reverting to
        // the `?`/`?` pair (its own distinct porcelain entry kind), not
        // literally `.` -- both `.` and `?` mean "not currently staged."
        assert!(
            files.iter().all(|f| f.index_status == '.' || f.index_status == '?'),
            "every file under sub/ should be unstaged: {files:?}"
        );
    }

    #[test]
    fn discard_dir_reverts_tracked_and_removes_untracked_files_under_it() {
        let dir = committed_repo("actions_discard_dir");
        dir.write("sub/a.txt", "v1\n");
        git(dir.path(), &["add", "sub/a.txt"]);
        git(dir.path(), &["commit", "-q", "-m", "add sub/a.txt"]);
        dir.write("sub/a.txt", "changed\n");
        let untracked = dir.write("sub/new.txt", "new\n");

        discard_dir(dir.path(), "sub").unwrap();
        assert_eq!(std::fs::read_to_string(dir.path().join("sub/a.txt")).unwrap(), "v1\n");
        assert!(!untracked.exists());
    }

    #[test]
    fn discard_dir_never_fails_when_the_directory_is_entirely_untracked() {
        let dir = committed_repo("actions_discard_dir_all_untracked");
        let untracked = dir.write("sub/new.txt", "new\n");
        discard_dir(dir.path(), "sub").unwrap();
        assert!(!untracked.exists());
    }

    #[test]
    fn commit_creates_a_new_commit_from_staged_changes() {
        let dir = committed_repo("actions_commit");
        dir.write("a.txt", "v2\n");
        stage_file(dir.path(), "a.txt").unwrap();
        commit(dir.path(), "second commit").unwrap();
        assert_eq!(crate::commit::list_commits(dir.path(), 1)[0].message, "second commit");
    }

    #[test]
    fn create_then_checkout_branches() {
        let dir = committed_repo("actions_branches");
        create_branch(dir.path(), "feature").unwrap();
        assert!(crate::branch::list_branches(dir.path()).iter().find(|b| b.name == "feature").unwrap().current);

        checkout_branch(dir.path(), "main").unwrap();
        assert!(crate::branch::list_branches(dir.path()).iter().find(|b| b.name == "main").unwrap().current);
    }

    #[test]
    fn delete_branch_removes_a_fully_merged_branch() {
        let dir = committed_repo("actions_delete_branch");
        create_branch(dir.path(), "throwaway").unwrap();
        checkout_branch(dir.path(), "main").unwrap();
        delete_branch(dir.path(), "throwaway", false).unwrap();
        assert!(crate::branch::list_branches(dir.path()).iter().all(|b| b.name != "throwaway"));
    }

    #[test]
    fn stash_push_apply_and_drop_round_trip() {
        let dir = committed_repo("actions_stash");
        dir.write("a.txt", "stashed change\n");
        stash_push(dir.path()).unwrap();
        assert_eq!(std::fs::read_to_string(dir.path().join("a.txt")).unwrap(), "v1\n");
        assert_eq!(crate::stash::list_stashes(dir.path()).len(), 1);

        stash_apply(dir.path(), 0).unwrap();
        assert_eq!(std::fs::read_to_string(dir.path().join("a.txt")).unwrap(), "stashed change\n");
        assert_eq!(crate::stash::list_stashes(dir.path()).len(), 1); // apply keeps the entry

        stash_drop(dir.path(), 0).unwrap();
        assert!(crate::stash::list_stashes(dir.path()).is_empty());
    }

    #[test]
    fn stash_pop_applies_and_removes_the_entry_in_one_step() {
        let dir = committed_repo("actions_stash_pop");
        dir.write("a.txt", "popped change\n");
        stash_push(dir.path()).unwrap();

        stash_pop(dir.path(), 0).unwrap();
        assert_eq!(std::fs::read_to_string(dir.path().join("a.txt")).unwrap(), "popped change\n");
        assert!(crate::stash::list_stashes(dir.path()).is_empty());
    }

    #[test]
    fn fetch_never_panics_without_a_configured_remote() {
        let dir = TempDir::new("fetch_no_remote");
        init_repo(dir.path());
        // No remote configured: `git fetch --all` succeeds with nothing
        // to do rather than failing, but either way this must not panic.
        let _ = fetch(dir.path());
    }

    #[test]
    fn seconds_since_fetch_is_none_before_any_fetch_and_some_after_one() {
        let remote = TempDir::new("fetch_age_remote");
        git(remote.path(), &["init", "-q", "--bare"]);
        let dir = TempDir::new("fetch_age_clone");
        init_repo(dir.path());
        git(dir.path(), &["remote", "add", "origin", remote.path().to_str().unwrap()]);
        dir.write("a.txt", "v1");
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-q", "-m", "first"]);
        git(dir.path(), &["push", "-q", "-u", "origin", "main"]);

        assert_eq!(seconds_since_fetch(dir.path()), None, "nothing fetched yet");
        fetch(dir.path()).expect("fetching a real remote should succeed");
        let age = seconds_since_fetch(dir.path()).expect("FETCH_HEAD exists after a fetch");
        assert!(age < 60, "a fetch that just happened should read as seconds old, got {age}");
    }

    #[test]
    fn push_and_pull_never_panic_without_a_configured_remote() {
        let dir = committed_repo("actions_push_pull_no_remote");
        assert!(push(dir.path()).is_err());
        assert!(pull(dir.path()).is_err());
    }
}
