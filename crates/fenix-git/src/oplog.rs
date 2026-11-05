//! The operation log: every operation Fenix runs on a repository, with
//! what it takes to put things back. Kept in the repository's git dir
//! (`.git/fenix/oplog`), so it's never in the working tree and never
//! pushed, and one line per entry so a crash mid-write loses at most
//! that entry.
//!
//! Undo is recorded *before* an operation runs: the ref it will move and
//! where it pointed, and for anything that throws work away (a discard, a
//! hard reset, a dropped stash) a copy of that work as git objects -- a
//! `stash create` commit for tracked changes, blobs for untracked files.

use std::path::{Path, PathBuf};

use crate::process::{run_action, run_lines};

/// How to take one operation back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Undo {
    /// Move the current branch back to `to`, then re-apply the changes
    /// saved in `saved`, if any. `soft` keeps what the undone commits
    /// changed, staged -- undoing a commit gives its changes back; else
    /// `reset --keep`, which refuses rather than overwrite uncommitted
    /// work.
    Reset { to: String, soft: bool, saved: Option<String> },
    /// Switch back to the branch that was checked out.
    Switch(String),
    /// Delete a branch the operation created.
    DeleteBranch(String),
    /// Recreate the branches the operation deleted, each where it was.
    CreateBranches(Vec<(String, String)>),
    /// Undo creating a branch: switch back first when the operation
    /// switched to it, then delete it.
    Unbranch { name: String, back_to: Option<String> },
    /// Rename a branch back.
    Rename { from: String, to: String },
    /// Delete a tag the operation created.
    DeleteTag(String),
    /// Take a stash entry back off the list and apply it (a stash push).
    PopStash(String),
    /// Put a dropped stash commit back on the list.
    StoreStash { commit: String, message: String },
    /// Bring back discarded changes: tracked ones from a `stash create`
    /// commit, and whole files from their blobs.
    Restore { saved: Option<String>, files: Vec<(String, String)> },
    /// Put back lines a hunk or line discard took out: the patch that was
    /// reversed, saved as a blob, applied forward again -- so other edits
    /// to the same file since don't get in the way.
    ApplyPatch(String),
    /// Nothing to undo locally, and why.
    Not(String),
}

/// One logged operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Milliseconds since the Unix epoch -- also the entry's id, so
    /// `logged` keeps it strictly increasing.
    pub time: u64,
    /// What the page called it: `reset --hard to e81a4f2`.
    pub label: String,
    /// The branch it ran on, when there was one.
    pub branch: Option<String>,
    /// HEAD before and after.
    pub before: String,
    pub after: String,
    pub ok: bool,
    pub undo: Undo,
}

const SEP: char = '\u{1f}';
const SUB: char = '\u{1e}';

fn clean(s: &str) -> String {
    s.replace([SEP, SUB, '\n', '\r'], " ")
}

impl Undo {
    fn encode(&self) -> String {
        let parts: Vec<String> = match self {
            Undo::Reset { to, soft, saved } => vec!["reset".into(), to.clone(), saved.clone().unwrap_or_default(), if *soft { "soft" } else { "keep" }.into()],
            Undo::Switch(b) => vec!["switch".into(), b.clone()],
            Undo::DeleteBranch(b) => vec!["delete-branch".into(), b.clone()],
            Undo::CreateBranches(branches) => std::iter::once("create-branches".to_string()).chain(branches.iter().flat_map(|(n, a)| [n.clone(), a.clone()])).collect(),
            Undo::Unbranch { name, back_to } => vec!["unbranch".into(), name.clone(), back_to.clone().unwrap_or_default()],
            Undo::Rename { from, to } => vec!["rename".into(), from.clone(), to.clone()],
            Undo::DeleteTag(t) => vec!["delete-tag".into(), t.clone()],
            Undo::PopStash(c) => vec!["pop-stash".into(), c.clone()],
            Undo::StoreStash { commit, message } => vec!["store-stash".into(), commit.clone(), message.clone()],
            Undo::Restore { saved, files } => {
                let mut v = vec!["restore".into(), saved.clone().unwrap_or_default()];
                for (path, blob) in files {
                    v.push(path.clone());
                    v.push(blob.clone());
                }
                v
            }
            Undo::ApplyPatch(blob) => vec!["apply-patch".into(), blob.clone()],
            Undo::Not(why) => vec!["not".into(), why.clone()],
        };
        parts.iter().map(|p| clean(p)).collect::<Vec<_>>().join(&SUB.to_string())
    }

    fn decode(text: &str) -> Option<Undo> {
        let parts: Vec<&str> = text.split(SUB).collect();
        let arg = |i: usize| parts.get(i).map(|s| s.to_string());
        let opt = |i: usize| arg(i).filter(|s| !s.is_empty());
        Some(match *parts.first()? {
            "reset" => Undo::Reset { to: arg(1)?, saved: opt(2), soft: arg(3).as_deref() == Some("soft") },
            "switch" => Undo::Switch(arg(1)?),
            "delete-branch" => Undo::DeleteBranch(arg(1)?),
            "create-branches" => Undo::CreateBranches(parts[1..].chunks(2).filter(|c| c.len() == 2).map(|c| (c[0].to_string(), c[1].to_string())).collect()),
            "unbranch" => Undo::Unbranch { name: arg(1)?, back_to: opt(2) },
            "rename" => Undo::Rename { from: arg(1)?, to: arg(2)? },
            "delete-tag" => Undo::DeleteTag(arg(1)?),
            "pop-stash" => Undo::PopStash(arg(1)?),
            "store-stash" => Undo::StoreStash { commit: arg(1)?, message: arg(2).unwrap_or_default() },
            "restore" => Undo::Restore { saved: opt(1), files: parts[2..].chunks(2).filter(|c| c.len() == 2).map(|c| (c[0].to_string(), c[1].to_string())).collect() },
            "apply-patch" => Undo::ApplyPatch(arg(1)?),
            "not" => Undo::Not(arg(1).unwrap_or_default()),
            _ => return None,
        })
    }

    /// Whether there's anything to do.
    pub fn possible(&self) -> bool {
        !matches!(self, Undo::Not(_))
    }
}

impl Entry {
    fn encode(&self) -> String {
        [
            self.time.to_string(),
            clean(&self.label),
            self.branch.clone().unwrap_or_default(),
            self.before.clone(),
            self.after.clone(),
            if self.ok { "ok" } else { "failed" }.to_string(),
            self.undo.encode(),
        ]
        .join(&SEP.to_string())
    }

    fn decode(line: &str) -> Option<Entry> {
        let mut f = line.splitn(7, SEP);
        let time = f.next()?.parse().ok()?;
        let label = f.next()?.to_string();
        let branch = Some(f.next()?.to_string()).filter(|b| !b.is_empty());
        let before = f.next()?.to_string();
        let after = f.next()?.to_string();
        let ok = f.next()? == "ok";
        let undo = Undo::decode(f.next()?)?;
        Some(Entry { time, label, branch, before, after, ok, undo })
    }
}

fn git_dir(repo: &Path) -> Option<PathBuf> {
    run_lines(repo, &["rev-parse", "--absolute-git-dir"]).into_iter().next().map(PathBuf::from)
}

fn log_path(repo: &Path) -> Option<PathBuf> {
    Some(git_dir(repo)?.join("fenix").join("oplog"))
}

/// The commit `rev` names, or empty.
pub fn rev(repo: &Path, rev: &str) -> String {
    run_lines(repo, &["rev-parse", "--verify", "--quiet", rev]).into_iter().next().unwrap_or_default()
}

/// The checked-out branch, `None` when HEAD is detached.
pub fn current_branch(repo: &Path) -> Option<String> {
    run_lines(repo, &["symbolic-ref", "--quiet", "--short", "HEAD"]).into_iter().next()
}

/// Tracked changes (index and working tree) saved as a commit without
/// touching anything -- `None` when there are none.
pub fn save_changes(repo: &Path) -> Option<String> {
    run_lines(repo, &["stash", "create"]).into_iter().next().filter(|s| !s.is_empty())
}

/// An untracked file's content saved as a blob.
pub fn save_file(repo: &Path, path: &str) -> Option<String> {
    run_lines(repo, &["hash-object", "-w", "--", path]).into_iter().next()
}

/// A patch saved as a blob, for `Undo::ApplyPatch`.
pub fn save_patch(repo: &Path, patch: &str) -> Option<String> {
    crate::process::run_action_stdin(repo, &["hash-object".into(), "-w".into(), "--stdin".into()], patch).ok().map(|out| out.trim().to_string()).filter(|h| !h.is_empty())
}

/// Appends `entry` to the log.
pub fn record(repo: &Path, entry: &Entry) -> Result<(), String> {
    let path = log_path(repo).ok_or("not a git repository")?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new().create(true).append(true).open(&path).map_err(|e| e.to_string())?;
    writeln!(file, "{}", entry.encode()).map_err(|e| e.to_string())
}

/// Runs an operation and logs it. `undo` is handed the result and says
/// how to take it back -- worked out from whatever the caller saved
/// before running (a `save_changes` commit, the branch it was on).
pub fn logged(repo: &Path, label: &str, run: impl FnOnce() -> Result<String, String>, undo: impl FnOnce(&Result<String, String>) -> Undo) -> Result<String, String> {
    let before = rev(repo, "HEAD");
    let branch = current_branch(repo);
    let result = run();
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
    // Two operations in the same millisecond still get their own ids.
    let last = entries(repo, 1).first().map(|e| e.time).unwrap_or(0);
    let entry = Entry {
        time: now.max(last + 1),
        label: label.to_string(),
        branch,
        after: rev(repo, "HEAD"),
        before,
        ok: result.is_ok(),
        undo: undo(&result),
    };
    // A log that can't be written mustn't fail the operation itself.
    let _ = record(repo, &entry);
    result
}

/// The log, newest first, at most `limit` entries.
pub fn entries(repo: &Path, limit: usize) -> Vec<Entry> {
    let Some(path) = log_path(repo) else { return Vec::new() };
    let text = std::fs::read_to_string(path).unwrap_or_default();
    text.lines().rev().filter_map(Entry::decode).take(limit).collect()
}

/// What undoing `undo` would do, in a few lines -- for the page to show
/// before it happens.
pub fn preview(repo: &Path, undo: &Undo) -> Vec<String> {
    let subject = |c: &str| run_lines(repo, &["log", "-1", "--format=%h %s", c]).into_iter().next().unwrap_or_else(|| c.chars().take(7).collect());
    let mut out = Vec::new();
    match undo {
        Undo::Reset { to, soft, saved } => {
            let branch = current_branch(repo).unwrap_or_else(|| "HEAD".to_string());
            out.push(format!("{branch} goes back to {}", subject(to)));
            let back = run_lines(repo, &["rev-list", "--count", &format!("HEAD..{to}")]).into_iter().next().unwrap_or_default();
            let gone = run_lines(repo, &["rev-list", "--count", &format!("{to}..HEAD")]).into_iter().next().unwrap_or_default();
            if back != "0" && !back.is_empty() {
                out.push(format!("+ brings back {back} commit(s)"));
            }
            if gone != "0" && !gone.is_empty() {
                if *soft {
                    out.push(format!("- takes off {gone} commit(s); what they changed stays, staged"));
                } else {
                    out.push(format!("- takes off {gone} commit(s) made since (still in the reflog)"));
                }
            }
            if saved.is_some() {
                out.push("+ and the uncommitted changes saved before it".to_string());
            }
        }
        Undo::Switch(b) => out.push(format!("switch back to {b}")),
        Undo::DeleteBranch(b) => out.push(format!("delete the branch {b}")),
        Undo::CreateBranches(branches) => out.extend(branches.iter().map(|(name, at)| format!("+ recreate {name} at {}", subject(at)))),
        Undo::Unbranch { name, back_to } => {
            if let Some(back) = back_to {
                out.push(format!("switch back to {back}"));
            }
            out.push(format!("delete the branch {name}"));
        }
        Undo::Rename { from, to } => out.push(format!("rename {from} back to {to}")),
        Undo::DeleteTag(t) => out.push(format!("delete the tag {t}")),
        Undo::PopStash(_) => out.push("take the stash back off and apply it".to_string()),
        Undo::StoreStash { message, .. } => out.push(format!("put the stash back: {message}")),
        Undo::Restore { saved, files } => {
            if saved.is_some() {
                out.push("+ bring back the discarded changes to tracked files".to_string());
            }
            for (path, _) in files {
                out.push(format!("+ bring back {path}"));
            }
        }
        Undo::ApplyPatch(_) => out.push("+ put the discarded lines back".to_string()),
        Undo::Not(why) => out.push(why.clone()),
    }
    out
}

/// Takes the operation back.
pub fn apply(repo: &Path, undo: &Undo) -> Result<String, String> {
    let s = |v: &[&str]| v.iter().map(|a| a.to_string()).collect::<Vec<_>>();
    match undo {
        Undo::Reset { to, soft, saved } => {
            let mut out = run_action(repo, &s(&["reset", if *soft { "--soft" } else { "--keep" }, to]))?;
            if let Some(saved) = saved {
                out.push_str(&run_action(repo, &s(&["stash", "apply", "--index", saved]))?);
            }
            Ok(out)
        }
        Undo::Switch(b) => run_action(repo, &s(&["switch", b])),
        Undo::DeleteBranch(b) => run_action(repo, &s(&["branch", "-D", b])),
        Undo::CreateBranches(branches) => {
            let mut out = String::new();
            for (name, at) in branches {
                out.push_str(&run_action(repo, &s(&["branch", name, at]))?);
            }
            Ok(out)
        }
        Undo::Unbranch { name, back_to } => {
            let mut out = String::new();
            if let Some(back) = back_to {
                out.push_str(&run_action(repo, &s(&["switch", back]))?);
            }
            out.push_str(&run_action(repo, &s(&["branch", "-D", name]))?);
            Ok(out)
        }
        Undo::Rename { from, to } => run_action(repo, &s(&["branch", "-m", from, to])),
        Undo::DeleteTag(t) => run_action(repo, &s(&["tag", "-d", t])),
        Undo::PopStash(commit) => {
            let index = run_lines(repo, &["stash", "list", "--format=%H"]).iter().position(|h| h == commit).ok_or("that stash isn't on the list any more")?;
            run_action(repo, &s(&["stash", "pop", "--index", &format!("stash@{{{index}}}")]))
        }
        Undo::StoreStash { commit, message } => run_action(repo, &s(&["stash", "store", "-m", message, commit])),
        Undo::Restore { saved, files } => {
            let mut out = String::new();
            if let Some(saved) = saved {
                out.push_str(&run_action(repo, &s(&["stash", "apply", saved]))?);
            }
            for (path, blob) in files {
                let content = run_action_bytes(repo, blob)?;
                let target = repo.join(path);
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                }
                std::fs::write(&target, content).map_err(|e| format!("{path}: {e}"))?;
            }
            Ok(out)
        }
        Undo::ApplyPatch(blob) => {
            let patch = String::from_utf8(run_action_bytes(repo, blob)?).map_err(|e| e.to_string())?;
            crate::process::run_action_stdin(repo, &s(&["apply", "--whitespace=nowarn", "-"]), &patch)
        }
        Undo::Not(why) => Err(why.clone()),
    }
}

/// A blob's bytes, exactly (not through the line-splitting helpers).
fn run_action_bytes(repo: &Path, blob: &str) -> Result<Vec<u8>, String> {
    crate::process::run_bytes(repo, &["cat-file", "blob", blob])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{git, init_repo, TempDir};

    fn repo(name: &str) -> TempDir {
        let dir = TempDir::new(name);
        init_repo(dir.path());
        git(dir.path(), &["config", "core.autocrlf", "false"]);
        dir.write("a.txt", "one\n");
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-q", "-m", "first"]);
        dir
    }

    #[test]
    fn entries_round_trip_newest_first() {
        let dir = repo("oplog_round_trip");
        let undo = Undo::Restore { saved: Some("abc".into()), files: vec![("new.txt".into(), "def".into())] };
        let entry = Entry { time: 1, label: "discard\u{1f}weird\nlabel".into(), branch: Some("main".into()), before: "x".into(), after: "y".into(), ok: true, undo: undo.clone() };
        record(dir.path(), &entry).unwrap();
        record(dir.path(), &Entry { time: 2, label: "second".into(), undo: Undo::Not("pushed".into()), ok: false, branch: None, ..entry.clone() }).unwrap();
        let read = entries(dir.path(), 10);
        assert_eq!(read.len(), 2);
        assert_eq!(read[0].label, "second");
        assert!(!read[0].ok && read[0].branch.is_none());
        assert_eq!(read[1].undo, undo);
        assert_eq!(read[1].label, "discard weird label", "separators can't break the line");
        assert!(!dir.path().join(".fenix").exists() && dir.path().join(".git/fenix/oplog").exists(), "the log lives in the git dir");
    }

    #[test]
    fn a_hard_reset_is_undone_with_its_commits_and_its_uncommitted_work() {
        let dir = repo("oplog_reset");
        let first = rev(dir.path(), "HEAD");
        dir.write("b.txt", "b\n");
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-q", "-m", "second"]);
        dir.write("a.txt", "one, edited\n");

        let before = rev(dir.path(), "HEAD");
        let undo = Undo::Reset { to: before.clone(), soft: false, saved: save_changes(dir.path()) };
        git(dir.path(), &["reset", "-q", "--hard", &first]);
        assert!(preview(dir.path(), &undo).iter().any(|l| l.contains("brings back 1 commit")), "{:?}", preview(dir.path(), &undo));

        apply(dir.path(), &undo).unwrap();
        assert_eq!(rev(dir.path(), "HEAD"), before);
        assert_eq!(std::fs::read_to_string(dir.path().join("a.txt")).unwrap(), "one, edited\n", "the edit came back");
    }

    #[test]
    fn discarded_files_come_back_tracked_and_untracked() {
        let dir = repo("oplog_discard");
        dir.write("a.txt", "one, edited\n");
        dir.write("new.txt", "brand new\n");
        let undo = Undo::Restore { saved: save_changes(dir.path()), files: vec![("new.txt".into(), save_file(dir.path(), "new.txt").unwrap())] };
        git(dir.path(), &["checkout", "--", "a.txt"]);
        std::fs::remove_file(dir.path().join("new.txt")).unwrap();
        apply(dir.path(), &undo).unwrap();
        assert_eq!(std::fs::read_to_string(dir.path().join("a.txt")).unwrap(), "one, edited\n");
        assert_eq!(std::fs::read_to_string(dir.path().join("new.txt")).unwrap(), "brand new\n");
    }

    #[test]
    fn a_dropped_stash_goes_back_on_the_list() {
        let dir = repo("oplog_stash");
        dir.write("a.txt", "one, stashed\n");
        git(dir.path(), &["stash", "push", "-q", "-m", "keep me"]);
        let commit = rev(dir.path(), "stash@{0}");
        git(dir.path(), &["stash", "drop", "-q"]);
        apply(dir.path(), &Undo::StoreStash { commit, message: "keep me".into() }).unwrap();
        assert!(run_lines(dir.path(), &["stash", "list"])[0].contains("keep me"));
    }

    #[test]
    fn branches_come_back_and_go_away() {
        let dir = repo("oplog_branch");
        let head = rev(dir.path(), "HEAD");
        let undo = Undo::CreateBranches(vec![("topic".into(), head.clone()), ("other".into(), head)]);
        assert_eq!(Undo::decode(&undo.encode()), Some(undo.clone()));
        apply(dir.path(), &undo).unwrap();
        assert_eq!(rev(dir.path(), "topic"), rev(dir.path(), "HEAD"));
        assert!(!rev(dir.path(), "other").is_empty());
        apply(dir.path(), &Undo::DeleteBranch("topic".into())).unwrap();
        assert!(rev(dir.path(), "topic").is_empty());

        git(dir.path(), &["switch", "-q", "-c", "made"]);
        apply(dir.path(), &Undo::Unbranch { name: "made".into(), back_to: Some("main".into()) }).unwrap();
        assert_eq!(current_branch(dir.path()).as_deref(), Some("main"));
        assert!(rev(dir.path(), "made").is_empty());
    }

    #[test]
    fn undoing_a_commit_gives_its_changes_back_staged() {
        let dir = repo("oplog_soft");
        let before = rev(dir.path(), "HEAD");
        dir.write("b.txt", "b\n");
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-q", "-m", "second"]);
        let undo = Undo::Reset { to: before.clone(), soft: true, saved: None };
        assert!(preview(dir.path(), &undo).iter().any(|l| l.contains("stays, staged")));
        apply(dir.path(), &undo).unwrap();
        assert_eq!(rev(dir.path(), "HEAD"), before);
        assert_eq!(run_lines(dir.path(), &["diff", "--cached", "--name-only"]), ["b.txt"]);
    }
}
