//! Worktrees: another branch checked out in a folder of its own, beside
//! the repository -- so looking at `main`, or reviewing someone's
//! branch, doesn't mean stashing your own work.

use std::path::{Path, PathBuf};

use crate::process::{run_action, run_lines};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Worktree {
    pub path: PathBuf,
    /// The branch checked out there; `None` when HEAD is detached.
    pub branch: Option<String>,
    pub head: String,
    /// Its folder is gone; `prune` forgets it.
    pub prunable: bool,
}

/// Every worktree of the repository, the main one first.
pub fn list_worktrees(repo: &Path) -> Vec<Worktree> {
    let mut out: Vec<Worktree> = Vec::new();
    for line in run_lines(repo, &["worktree", "list", "--porcelain"]) {
        if let Some(path) = line.strip_prefix("worktree ") {
            out.push(Worktree { path: PathBuf::from(path), branch: None, head: String::new(), prunable: false });
        } else if let Some(last) = out.last_mut() {
            if let Some(head) = line.strip_prefix("HEAD ") {
                last.head = head.to_string();
            } else if let Some(branch) = line.strip_prefix("branch ") {
                last.branch = Some(branch.strip_prefix("refs/heads/").unwrap_or(branch).to_string());
            } else if line.starts_with("prunable") {
                last.prunable = true;
            }
        }
    }
    out
}

/// Where a new worktree for `branch` goes by default: beside the
/// repository, named after both -- `fenix.feature-x` next to `fenix`.
pub fn default_worktree_path(repo: &Path, branch: &str) -> PathBuf {
    let name = repo.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "repo".to_string());
    let folder = format!("{name}.{}", branch.replace(['/', '\\', ':'], "-"));
    repo.parent().map(|p| p.join(&folder)).unwrap_or_else(|| PathBuf::from(folder))
}

/// Checks `branch` out at `path` -- creating the branch from `start` (HEAD
/// when `None`) if it doesn't exist yet.
pub fn add_worktree(repo: &Path, path: &Path, branch: &str, start: Option<&str>) -> Result<String, String> {
    let exists = !run_lines(repo, &["rev-parse", "--verify", "--quiet", &format!("refs/heads/{branch}")]).is_empty();
    let path = path.to_string_lossy().to_string();
    let mut args = vec!["worktree".to_string(), "add".to_string()];
    if exists {
        args.extend([path, branch.to_string()]);
    } else {
        args.extend(["-b".to_string(), branch.to_string(), path]);
        if let Some(start) = start {
            args.push(start.to_string());
        }
    }
    run_action(repo, &args)
}

/// Removes a worktree's folder and forgets it -- refused while it has
/// uncommitted changes, rather than forced.
pub fn remove_worktree(repo: &Path, path: &Path) -> Result<String, String> {
    run_action(repo, &["worktree".into(), "remove".into(), path.to_string_lossy().to_string()])
}

pub fn prune_worktrees(repo: &Path) -> Result<String, String> {
    run_action(repo, &["worktree".into(), "prune".into()])
}

/// The folder a failed checkout says `branch` is already checked out in.
pub fn checked_out_elsewhere(error: &str) -> Option<PathBuf> {
    if !(error.contains("already checked out at") || error.contains("already used by worktree at")) {
        return None;
    }
    let end = error.rfind('\'')?;
    let start = error[..end].rfind('\'')?;
    Some(PathBuf::from(&error[start + 1..end]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{git, init_repo, TempDir};

    #[test]
    fn worktrees_are_added_listed_and_removed() {
        let dir = TempDir::new("worktree");
        init_repo(dir.path());
        dir.write("a.txt", "a");
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-q", "-m", "first"]);
        let path = default_worktree_path(dir.path(), "feature/x");
        assert!(path.file_name().unwrap().to_string_lossy().ends_with(".feature-x"));
        let _ = std::fs::remove_dir_all(&path);
        add_worktree(dir.path(), &path, "feature/x", None).unwrap();
        let list = list_worktrees(dir.path());
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].branch.as_deref(), Some("main"));
        assert_eq!(list[1].branch.as_deref(), Some("feature/x"));
        assert!(path.join("a.txt").exists());

        let error = crate::actions::checkout_branch(dir.path(), "feature/x").unwrap_err();
        let elsewhere = checked_out_elsewhere(&error).expect(&error);
        assert_eq!(std::fs::canonicalize(elsewhere).unwrap(), std::fs::canonicalize(&path).unwrap());

        remove_worktree(dir.path(), &path).unwrap();
        assert_eq!(list_worktrees(dir.path()).len(), 1);
        assert!(!path.exists());
    }
}
