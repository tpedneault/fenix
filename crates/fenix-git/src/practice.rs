//! A repository to practise in: the tutorial's Git lesson stages and
//! commits in one of these, never in yours.

use std::path::{Path, PathBuf};

use crate::process::run_action;

/// A fresh repository at `dir` (whatever was there goes): `file` holding
/// `committed` as its one commit on `main`, then `working` written over
/// it, so there's a change to stage. Its own name and email, so a commit
/// needs nothing of yours. The file's path.
pub fn practice_repository(dir: &Path, file: &str, committed: &str, working: &str) -> Result<PathBuf, String> {
    let _ = std::fs::remove_dir_all(dir);
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let git = |args: &[&str]| run_action(dir, &args.iter().map(|a| a.to_string()).collect::<Vec<_>>()).map(|_| ());
    git(&["init", "-q", "-b", "main"])?;
    git(&["config", "user.name", "You"])?;
    git(&["config", "user.email", "you@example.invalid"])?;
    git(&["config", "commit.gpgsign", "false"])?;
    let path = dir.join(file);
    std::fs::write(&path, committed).map_err(|e| format!("{}: {e}", path.display()))?;
    git(&["add", "--", file])?;
    git(&["commit", "-q", "-m", "The notes, as they were"])?;
    std::fs::write(&path, working).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_practice_repository_has_one_commit_and_a_change() {
        let dir = crate::test_util::TempDir::new("practice");
        let at = dir.path().join("practice");
        let file = practice_repository(&at, "notes.txt", "one\n", "one\ntwo\n").unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "one\ntwo\n");
        assert_eq!(crate::log(&at, &crate::LogQuery::default()).len(), 1);
        assert!(crate::file_diff(&at, &file.to_string_lossy(), false).unwrap().contains("+two"));
        // Again: fresh, the old one gone.
        practice_repository(&at, "notes.txt", "one\n", "one\n3\n").unwrap();
        assert_eq!(crate::log(&at, &crate::LogQuery::default()).len(), 1);
    }
}
