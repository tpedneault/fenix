//! What Fenix remembers about each merge or pull request you review:
//! which files you've viewed (and at which version of each), the
//! comments you've written but not sent yet, the head you last reviewed
//! at, and when you last looked. Kept in the repository's git dir
//! (`.git/fenix/reviews/`), one small JSON file per request -- never in
//! the working tree, never pushed, and surviving a restart, since a
//! half-written review is work.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use fenix_forge::{DraftComment, Position};
use serde::{Deserialize, Serialize};

/// A comment waiting for the review to be submitted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pending {
    pub path: String,
    pub old_path: String,
    pub old_line: Option<usize>,
    pub new_line: Option<usize>,
    /// For a range: its first line, on the same side.
    pub start_line: Option<usize>,
    pub body: String,
}

impl Pending {
    pub fn draft(&self, refs: &fenix_forge::DiffRefs) -> DraftComment {
        DraftComment {
            position: Position {
                base_sha: refs.base_sha.clone(),
                head_sha: refs.head_sha.clone(),
                start_sha: refs.start_sha.clone(),
                old_path: self.old_path.clone(),
                new_path: self.path.clone(),
                old_line: self.old_line,
                new_line: self.new_line,
            },
            start_line: self.start_line,
            body: self.body.clone(),
        }
    }

    /// The line it's shown under: the new side's, else the old side's.
    pub fn line(&self) -> Option<usize> {
        self.new_line.or(self.old_line)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewState {
    /// Viewed files, each with a fingerprint of the diff it was viewed
    /// at -- a file whose diff changes since is no longer viewed.
    pub viewed: HashMap<String, u64>,
    pub pending: Vec<Pending>,
    /// The head you submitted your last review at.
    pub reviewed_head: Option<String>,
    /// The request's `updated_at` when you last opened it.
    pub seen: Option<String>,
}

/// A fingerprint of a file's diff, for the viewed marks.
pub fn fingerprint(diff: &str) -> u64 {
    // FNV-1a: stable across runs and builds, unlike `DefaultHasher`.
    diff.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| (h ^ b as u64).wrapping_mul(0x0100_0000_01b3))
}

/// Where requests of `project` are remembered, under the git dir.
fn dir(git_dir: &Path, project: &str) -> PathBuf {
    let slug: String = project.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '.' { c } else { '_' }).collect();
    git_dir.join("fenix").join("reviews").join(slug)
}

fn git_dir(repo: &Path) -> Option<PathBuf> {
    let out = std::process::Command::new("git").current_dir(repo).args(["rev-parse", "--git-common-dir"]).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if !out.status.success() || text.is_empty() {
        return None;
    }
    let path = PathBuf::from(text);
    Some(if path.is_absolute() { path } else { repo.join(path) })
}

/// What's remembered about request `number` of `project` -- shared by
/// every worktree of the repository, so a review started in a review
/// worktree carries on in the main checkout.
pub fn load(repo: &Path, project: &str, number: u64) -> ReviewState {
    let Some(git) = git_dir(repo) else { return ReviewState::default() };
    let file = dir(&git, project).join(format!("{number}.json"));
    std::fs::read_to_string(file).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

pub fn save(repo: &Path, project: &str, number: u64, state: &ReviewState) -> Result<(), String> {
    let git = git_dir(repo).ok_or("not a git repository")?;
    let dir = dir(&git, project);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let text = serde_json::to_string_pretty(state).map_err(|e| e.to_string())?;
    std::fs::write(dir.join(format!("{number}.json")), text).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_review_survives_being_saved_and_loaded() {
        let dir = std::env::temp_dir().join(format!("fenix-review-store-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::process::Command::new("git").current_dir(&dir).args(["init", "-q"]).output().unwrap();
        let mut state = ReviewState { reviewed_head: Some("abc".into()), ..Default::default() };
        state.viewed.insert("a.rs".into(), fingerprint("@@ -1 +1 @@"));
        state.pending.push(Pending { path: "a.rs".into(), old_path: "a.rs".into(), old_line: None, new_line: Some(3), start_line: Some(1), body: "Why?".into() });
        save(&dir, "group/project", 42, &state).unwrap();
        assert_eq!(load(&dir, "group/project", 42), state);
        assert_eq!(load(&dir, "group/project", 43), ReviewState::default());
        assert!(dir.join(".git/fenix/reviews/group_project/42.json").exists(), "in the git dir, never the tree");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_pending_comment_becomes_a_draft_for_the_head_reviewed() {
        let p = Pending { path: "b.rs".into(), old_path: "a.rs".into(), old_line: None, new_line: Some(9), start_line: None, body: "x".into() };
        let refs = fenix_forge::DiffRefs { base_sha: "b".into(), head_sha: "h".into(), start_sha: "s".into() };
        let d = p.draft(&refs);
        assert_eq!((d.position.head_sha.as_str(), d.position.new_line, d.position.old_path.as_str()), ("h", Some(9), "a.rs"));
        assert_ne!(fingerprint("a"), fingerprint("b"));
    }
}
