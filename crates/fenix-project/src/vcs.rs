//! What git says about a project root, for the hub and Home: the branch
//! (read straight from `.git/HEAD`, no process) and a fuller summary --
//! uncommitted changes, ahead/behind, the last commit -- from one `git
//! status` and one `git log`, for callers that can afford two processes
//! (the hub runs them off the UI thread).

use std::path::{Path, PathBuf};

/// The repository `path` is in: the nearest ancestor (or `path` itself)
/// with a `.git` -- so a subproject of a monorepo belongs to the
/// monorepo's repository, not to none.
pub fn repository_root(path: &Path) -> Option<PathBuf> {
    if fenix_vfs::is_remote(path) {
        return remote_repository_root(path);
    }
    path.ancestors().find(|dir| dir.join(".git").exists()).map(Path::to_path_buf)
}

/// How long what's found of a remote folder's repository is believed:
/// each folder looked at is a question over the network, and this is
/// asked as files are opened and switched to.
const REMOTE_ANSWER: std::time::Duration = std::time::Duration::from_secs(30);

/// `repository_root` on a remote machine: looked for there, never above
/// the machine's root, and remembered a while.
fn remote_repository_root(path: &Path) -> Option<PathBuf> {
    use std::collections::HashMap;
    use std::sync::Mutex;
    use std::time::Instant;
    /// A folder, when it was looked at, and its repository's root.
    type Found = HashMap<PathBuf, (Instant, Option<PathBuf>)>;
    static FOUND: Mutex<Option<Found>> = Mutex::new(None);
    if let Some((when, root)) = FOUND.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).get(path) {
        if when.elapsed() < REMOTE_ANSWER {
            return root.clone();
        }
    }
    let mut unsure = false;
    let root = path
        .ancestors()
        .take_while(|dir| fenix_vfs::is_remote(dir))
        .find(|dir| match fenix_vfs::metadata(&dir.join(".git")) {
            Ok(_) => true,
            Err(error) => {
                // A machine that can't be asked says nothing either way.
                unsure |= error.kind() != std::io::ErrorKind::NotFound;
                false
            }
        })
        .map(Path::to_path_buf);
    if !unsure {
        let mut found = FOUND.lock().unwrap_or_else(|e| e.into_inner());
        let found = found.get_or_insert_with(HashMap::new);
        if found.len() > 4096 {
            found.clear();
        }
        found.insert(path.to_path_buf(), (Instant::now(), root.clone()));
    }
    root
}

/// A file's text, wherever it is.
fn read_text(path: &Path) -> Option<String> {
    if fenix_vfs::is_remote(path) {
        return String::from_utf8(fenix_vfs::read(path).ok()?).ok();
    }
    std::fs::read_to_string(path).ok()
}

/// Whether `path` is a file, wherever it is.
fn is_file(path: &Path) -> bool {
    if fenix_vfs::is_remote(path) {
        return fenix_vfs::metadata(path).is_ok_and(|meta| meta.is_file());
    }
    path.is_file()
}

/// The branch checked out in the repository `root` is in, read straight
/// from `.git/HEAD` (a worktree's `.git` file is followed to its real git
/// dir). A detached HEAD reads as its short commit.
pub fn git_branch(root: &Path) -> Option<String> {
    let root = repository_root(root)?;
    let dot_git = root.join(".git");
    let git_dir = if is_file(&dot_git) {
        let pointer = read_text(&dot_git)?;
        let dir = pointer.trim().strip_prefix("gitdir:")?.trim();
        match fenix_vfs::host_of(&root) {
            // A path on that machine, absolute or from the worktree.
            Some(host) if dir.starts_with('/') => fenix_vfs::remote(&host, dir),
            Some(_) => fenix_vfs::join(&root, dir),
            None => {
                let dir = PathBuf::from(dir);
                if dir.is_absolute() { dir } else { root.join(dir) }
            }
        }
    } else {
        dot_git
    };
    let head = read_text(&git_dir.join("HEAD"))?;
    let head = head.trim();
    match head.strip_prefix("ref: refs/heads/") {
        Some(branch) => Some(branch.to_string()),
        None => (head.len() >= 7 && head.chars().all(|c| c.is_ascii_hexdigit())).then(|| head[..7].to_string()),
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GitSummary {
    pub branch: Option<String>,
    /// Files with uncommitted changes, untracked ones included.
    pub changed: usize,
    pub ahead: usize,
    pub behind: usize,
    /// The last commit's subject and when it was made (Unix seconds).
    pub last_commit: Option<(String, i64)>,
}

/// Parses `git status --porcelain=v1 --branch` output.
fn parse_status(text: &str, summary: &mut GitSummary) {
    for line in text.lines() {
        if let Some(header) = line.strip_prefix("## ") {
            // "main...origin/main [ahead 2, behind 1]" or "No commits yet on main"
            let (names, counts) = header.split_once(" [").map(|(n, c)| (n, Some(c.trim_end_matches(']')))).unwrap_or((header, None));
            let name = names.strip_prefix("No commits yet on ").unwrap_or(names);
            let branch = name.split("...").next().unwrap_or(name).trim();
            if !branch.is_empty() && branch != "HEAD (no branch)" {
                summary.branch = Some(branch.to_string());
            }
            for part in counts.into_iter().flat_map(|c| c.split(", ")) {
                if let Some(n) = part.strip_prefix("ahead ") {
                    summary.ahead = n.trim().parse().unwrap_or(0);
                } else if let Some(n) = part.strip_prefix("behind ") {
                    summary.behind = n.trim().parse().unwrap_or(0);
                }
            }
        } else if !line.trim().is_empty() {
            summary.changed += 1;
        }
    }
}

fn git(root: &Path, args: &[&str]) -> Option<String> {
    if let Some(ran) = fenix_vfs::run_there(root, fenix_vfs::Run { program: "git", args, ..fenix_vfs::Run::default() }) {
        let ran = ran.ok()?;
        return ran.success().then(|| String::from_utf8_lossy(&ran.stdout).into_owned());
    }
    let output = crate::process::quiet("git").args(args).current_dir(root).output().ok()?;
    output.status.success().then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

/// `root`'s git summary, or `None` when it isn't in a repository (or git
/// isn't installed). Scoped to `root`: in a monorepo, a subproject's
/// changes and last commit are its own, not the whole repository's. Runs
/// two git processes -- call it off the UI thread.
pub fn git_summary(root: &Path) -> Option<GitSummary> {
    repository_root(root)?;
    let status = git(root, &["status", "--porcelain=v1", "--branch", "--", "."])?;
    let mut summary = GitSummary::default();
    parse_status(&status, &mut summary);
    if summary.branch.is_none() {
        summary.branch = git_branch(root);
    }
    summary.last_commit = git(root, &["log", "-1", "--format=%s%x1f%ct", "--", "."]).and_then(|text| {
        let (subject, time) = text.trim().split_once('\u{1f}')?;
        Some((subject.to_string(), time.trim().parse().ok()?))
    });
    Some(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::TempDir;

    #[test]
    fn status_header_and_changes_are_read() {
        let mut s = GitSummary::default();
        parse_status("## main...origin/main [ahead 2, behind 1]\n M a.rs\n?? b.rs\n", &mut s);
        assert_eq!((s.branch.as_deref(), s.changed, s.ahead, s.behind), (Some("main"), 2, 2, 1));
        let mut s = GitSummary::default();
        parse_status("## No commits yet on master\n", &mut s);
        assert_eq!((s.branch.as_deref(), s.changed), (Some("master"), 0));
    }

    #[test]
    fn git_branch_reads_head_and_follows_a_worktree_pointer() {
        let dir = TempDir::new("vcs_branch");
        dir.write(".git/HEAD", "ref: refs/heads/feature/x\n");
        assert_eq!(git_branch(dir.path()).as_deref(), Some("feature/x"));
        let other = TempDir::new("vcs_worktree");
        let real = other.path().join("realgit");
        std::fs::create_dir_all(&real).unwrap();
        std::fs::write(real.join("HEAD"), "0123456789abcdef0123456789abcdef01234567\n").unwrap();
        other.write("wt/.git", &format!("gitdir: {}\n", real.display()));
        assert_eq!(git_branch(&other.path().join("wt")).as_deref(), Some("0123456"));
        assert_eq!(git_branch(&other.path().join("nowhere")), None);
    }

    #[test]
    fn a_real_repository_is_summarised() {
        let dir = TempDir::new("vcs_summary");
        let run = |args: &[&str]| git(dir.path(), args);
        if run(&["init", "-q", "-b", "trunk"]).is_none() {
            return; // no git here
        }
        run(&["config", "user.email", "t@example.com"]);
        run(&["config", "user.name", "T"]);
        dir.write("a.txt", "a");
        run(&["add", "-A"]);
        run(&["commit", "-q", "-m", "First thing"]);
        dir.write("b.txt", "b");
        let summary = git_summary(dir.path()).unwrap();
        assert_eq!(summary.branch.as_deref(), Some("trunk"));
        assert_eq!(summary.changed, 1);
        assert_eq!(summary.last_commit.as_ref().map(|(s, _)| s.as_str()), Some("First thing"));
        assert!(git_summary(&dir.path().join("missing")).is_none());

        // A subproject: the repository's branch, its own changes only.
        dir.write("sub/Cargo.toml", "[package]");
        run(&["add", "-A"]);
        run(&["commit", "-q", "-m", "Add sub"]);
        dir.write("sub/new.rs", "");
        let sub = git_summary(&dir.path().join("sub")).unwrap();
        assert_eq!(sub.branch.as_deref(), Some("trunk"));
        assert_eq!(sub.changed, 1, "b.txt at the top isn't the subproject's");
        assert_eq!(sub.last_commit.map(|(s, _)| s).as_deref(), Some("Add sub"));
        assert_eq!(repository_root(&dir.path().join("sub")).as_deref(), Some(dir.path()));
        assert_eq!(git_branch(&dir.path().join("sub")).as_deref(), Some("trunk"));
    }

    #[test]
    fn a_remote_repository_is_found_and_read_on_its_machine() {
        let dir = TempDir::new("vcs_remote");
        let _machine = fenix_vfs::testing::Folder::mount("vcs-box", dir.path());
        dir.write("proj/.git/HEAD", "ref: refs/heads/work\n");
        dir.write("proj/src/main.rs", "");
        let root = fenix_vfs::remote("vcs-box", "/proj");
        assert_eq!(repository_root(&fenix_vfs::remote("vcs-box", "/proj/src")), Some(root.clone()));
        assert_eq!(git_branch(&root).as_deref(), Some("work"));
        assert_eq!(repository_root(&fenix_vfs::remote("vcs-box", "/elsewhere")), None, "never this machine's / above it");
        fenix_vfs::unmount("vcs-box");
    }
}
