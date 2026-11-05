//! Comparing your side -- a ref, or the working tree -- with another
//! ref: which files differ, by how much, and each one's diff.
//!
//! The working tree can't be one end of `git diff A...B`, so a
//! three-dot comparison against it diffs from the merge base instead,
//! which is what three-dot means anyway. Untracked files aren't in any
//! `git diff`; in working-tree mode they're listed and given a diff of
//! their own, made here from the file.

use std::path::Path;

use crate::process::{run_action, run_bytes};

/// One end of a comparison.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Side {
    Rev(String),
    /// What's on disk: committed, staged, unstaged and untracked.
    WorkTree,
}

/// What's being compared: `head` against `base`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Range {
    pub base: String,
    pub head: Side,
    /// Measure from the merge base (what `head` did since the two split)
    /// rather than tree against tree.
    pub three_dot: bool,
}

impl Range {
    /// `origin/main...HEAD`, `origin/main..(working tree)`.
    pub fn label(&self) -> String {
        let dots = if self.three_dot { "..." } else { ".." };
        let head = match &self.head {
            Side::Rev(rev) => rev.as_str(),
            Side::WorkTree => "(working tree)",
        };
        format!("{}{dots}{head}", self.base)
    }

    /// The arguments after `git diff` that select this range. Needs the
    /// repository only for a three-dot comparison with the working tree,
    /// which diffs from the merge base.
    fn diff_args(&self, repo: &Path) -> Result<Vec<String>, String> {
        let merge_base = match (&self.head, self.three_dot) {
            (Side::WorkTree, true) => Some(
                crate::merge_base(repo, &self.base, "HEAD").ok_or_else(|| format!("{} and HEAD share no history (or {} isn't a ref)", self.base, self.base))?,
            ),
            _ => None,
        };
        Ok(range_args(&self.base, &self.head, self.three_dot, merge_base.as_deref()))
    }
}

/// `diff_args` without the repository: `merge_base` is what `base` and
/// HEAD share, needed (and only used) for three-dot against the working
/// tree.
fn range_args(base: &str, head: &Side, three_dot: bool, merge_base: Option<&str>) -> Vec<String> {
    match (head, three_dot) {
        (Side::Rev(head), true) => vec![format!("{base}...{head}")],
        (Side::Rev(head), false) => vec![base.to_string(), head.clone()],
        (Side::WorkTree, true) => vec![merge_base.unwrap_or(base).to_string()],
        (Side::WorkTree, false) => vec![base.to_string()],
    }
}

/// A file that differs between the two sides.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangedFile {
    /// `M`, `A`, `D`, `R`, `C`, `T` -- or `?` for an untracked file.
    pub letter: char,
    pub path: String,
    /// Where a renamed or copied file came from.
    pub old_path: Option<String>,
    /// Lines added and removed; `None` for a binary file.
    pub added: Option<usize>,
    pub removed: Option<usize>,
}

/// Every file that differs, in git's order, untracked files (in
/// working-tree mode) last. An unknown ref is git's own error.
pub fn changed_files(repo: &Path, range: &Range) -> Result<Vec<ChangedFile>, String> {
    let args = range.diff_args(repo)?;
    let with = |flags: &[&str]| {
        let mut all: Vec<String> = ["diff", "--no-color", "-M", "-z"].iter().chain(flags).map(|s| s.to_string()).collect();
        all.extend(args.iter().cloned());
        all.push("--".to_string());
        run_action(repo, &all)
    };
    let mut files = parse_name_status(&with(&["--name-status"])?);
    let stats = parse_numstat(&with(&["--numstat"])?);
    for file in &mut files {
        if let Some((added, removed)) = stats.iter().find(|(path, ..)| *path == file.path).map(|(_, a, r)| (*a, *r)) {
            file.added = added;
            file.removed = removed;
        }
    }
    if range.head == Side::WorkTree {
        for path in untracked(repo) {
            let (added, removed) = match read_text(&repo.join(&path)) {
                Ok(text) => (Some(line_count(&text)), Some(0)),
                Err(_) => (None, None),
            };
            files.push(ChangedFile { letter: '?', path, old_path: None, added, removed });
        }
    }
    Ok(files)
}

/// One changed file's diff, in this range.
pub fn changed_file_diff(repo: &Path, range: &Range, file: &ChangedFile) -> Result<String, String> {
    if file.letter == '?' {
        return untracked_diff(repo, &file.path);
    }
    let mut args: Vec<String> = ["diff", "--no-color", "-M"].iter().map(|s| s.to_string()).collect();
    args.extend(range.diff_args(repo)?);
    args.push("--".to_string());
    if let Some(old) = &file.old_path {
        args.push(old.clone());
    }
    args.push(file.path.clone());
    run_action(repo, &args)
}

/// Untracked files, as `git status` would list them (ignored ones left
/// out), with forward slashes.
fn untracked(repo: &Path) -> Vec<String> {
    let out = run_action(repo, &["ls-files".into(), "--others".into(), "--exclude-standard".into(), "-z".into()]).unwrap_or_default();
    out.split('\0').filter(|p| !p.is_empty()).map(str::to_string).collect()
}

/// Above this an untracked file isn't turned into a diff.
const UNTRACKED_LIMIT: u64 = 1024 * 1024;

/// A file's text; `Err` when it's binary, too big or unreadable.
fn read_text(path: &Path) -> Result<String, String> {
    let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
    if meta.len() > UNTRACKED_LIMIT {
        return Err(format!("too large to show ({} KB)", meta.len() / 1024));
    }
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    if bytes.iter().take(8000).any(|b| *b == 0) {
        return Err("binary".to_string());
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn line_count(text: &str) -> usize {
    text.lines().count()
}

/// An untracked file as a diff that adds every line of it -- what
/// `git diff --no-index /dev/null <file>` would say, without its exit
/// status of 1 or `/dev/null` on Windows to deal with.
pub fn untracked_diff(repo: &Path, path: &str) -> Result<String, String> {
    match read_text(&repo.join(path)) {
        Ok(text) => Ok(added_file_diff(path, &text)),
        Err(e) if e == "binary" => Ok(format!("diff --git a/{path} b/{path}\nnew file mode 100644\nBinary files /dev/null and b/{path} differ\n")),
        Err(e) => Err(e),
    }
}

fn added_file_diff(path: &str, text: &str) -> String {
    let mut out = format!("diff --git a/{path} b/{path}\nnew file mode 100644\n--- /dev/null\n+++ b/{path}\n");
    let lines: Vec<&str> = text.lines().collect();
    if lines.is_empty() {
        return out;
    }
    out.push_str(&format!("@@ -0,0 +1,{} @@\n", lines.len()));
    for line in &lines {
        out.push('+');
        out.push_str(line.strip_suffix('\r').unwrap_or(line));
        out.push('\n');
    }
    if !text.ends_with('\n') {
        out.push_str("\\ No newline at end of file\n");
    }
    out
}

/// `path` as it is at `rev` (`git show rev:path`).
pub fn show_at(repo: &Path, rev: &str, path: &str) -> Result<String, String> {
    let path = path.replace('\\', "/");
    run_bytes(repo, &["show", &format!("{rev}:{path}")]).map(|b| String::from_utf8_lossy(&b).into_owned())
}

/// The remote a remote-tracking ref belongs to -- `origin` for
/// `origin/main`. Matched against the real remotes, longest first,
/// since a remote's name can itself contain a slash.
pub fn remote_of(repo: &Path, rev: &str) -> Option<String> {
    remote_among(&crate::remotes(repo), rev)
}

fn remote_among(remotes: &[String], rev: &str) -> Option<String> {
    let rev = rev.strip_prefix("refs/remotes/").unwrap_or(rev);
    remotes.iter().filter(|r| rev.starts_with(&format!("{r}/"))).max_by_key(|r| r.len()).cloned()
}

/// Fetches one remote, pruning what it deleted.
pub fn fetch_remote(repo: &Path, remote: &str) -> Result<String, String> {
    run_action(repo, &["fetch".to_string(), "--prune".to_string(), remote.to_string()])
}

/// `--name-status -z`: `M\0path\0`, `R100\0old\0new\0`.
fn parse_name_status(out: &str) -> Vec<ChangedFile> {
    let mut parts = out.split('\0').filter(|p| !p.is_empty());
    let mut files = Vec::new();
    while let Some(status) = parts.next() {
        let letter = status.chars().next().unwrap_or('M');
        let (old_path, path) = if matches!(letter, 'R' | 'C') {
            let old = parts.next().unwrap_or_default().to_string();
            (Some(old), parts.next().unwrap_or_default().to_string())
        } else {
            (None, parts.next().unwrap_or_default().to_string())
        };
        if !path.is_empty() {
            files.push(ChangedFile { letter, path, old_path, added: None, removed: None });
        }
    }
    files
}

/// `--numstat -z`: `added\tremoved\tpath\0`, or for a rename
/// `added\tremoved\t\0old\0new\0`; `-` for both counts of a binary
/// file. Returns (new path, added, removed).
fn parse_numstat(out: &str) -> Vec<(String, Option<usize>, Option<usize>)> {
    let mut parts = out.split('\0');
    let mut stats = Vec::new();
    while let Some(entry) = parts.next() {
        if entry.is_empty() {
            continue;
        }
        let mut fields = entry.splitn(3, '\t');
        let added = fields.next().and_then(|n| n.parse().ok());
        let removed = fields.next().and_then(|n| n.parse().ok());
        let path = match fields.next() {
            Some("") | None => {
                let _old = parts.next();
                parts.next().unwrap_or_default().to_string()
            }
            Some(path) => path.to_string(),
        };
        stats.push((path, added, removed));
    }
    stats
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{git, init_repo, TempDir};

    /// `side` and `main` each moved on after splitting; `side` is
    /// checked out.
    fn diverged(name: &str) -> TempDir {
        let dir = TempDir::new(name);
        init_repo(dir.path());
        dir.write("shared.txt", "one\ntwo\nthree\n");
        dir.write("old_name.txt", "a fairly long line so the rename is detected\nand another\nand a third\n");
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-q", "-m", "initial"]);
        git(dir.path(), &["checkout", "-q", "-b", "side"]);
        git(dir.path(), &["checkout", "-q", "main"]);
        dir.write("from_main.txt", "main work\n");
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-q", "-m", "on main"]);
        git(dir.path(), &["checkout", "-q", "side"]);
        dir.write("shared.txt", "one\nTWO\nthree\nfour\n");
        git(dir.path(), &["mv", "old_name.txt", "new_name.txt"]);
        git(dir.path(), &["commit", "-q", "-am", "on side"]);
        dir
    }

    fn range(head: Side, three_dot: bool) -> Range {
        Range { base: "main".to_string(), head, three_dot }
    }

    fn paths(files: &[ChangedFile]) -> Vec<(char, &str)> {
        files.iter().map(|f| (f.letter, f.path.as_str())).collect()
    }

    #[test]
    fn range_args_cover_every_mode() {
        let head = Side::Rev("HEAD".to_string());
        assert_eq!(range_args("main", &head, true, None), vec!["main...HEAD"]);
        assert_eq!(range_args("main", &head, false, None), vec!["main", "HEAD"]);
        assert_eq!(range_args("main", &Side::WorkTree, true, Some("abc")), vec!["abc"]);
        assert_eq!(range_args("main", &Side::WorkTree, false, None), vec!["main"]);
    }

    #[test]
    fn labels_name_both_sides() {
        assert_eq!(range(Side::Rev("HEAD".into()), true).label(), "main...HEAD");
        assert_eq!(range(Side::WorkTree, false).label(), "main..(working tree)");
    }

    #[test]
    fn three_dot_lists_only_what_head_did_with_counts_and_renames() {
        let dir = diverged("cmp_three_dot");
        let files = changed_files(dir.path(), &range(Side::Rev("HEAD".into()), true)).unwrap();
        assert_eq!(paths(&files), vec![('R', "new_name.txt"), ('M', "shared.txt")]);
        assert_eq!(files[0].old_path.as_deref(), Some("old_name.txt"));
        assert_eq!((files[1].added, files[1].removed), (Some(2), Some(1)));
    }

    #[test]
    fn two_dot_also_lists_what_the_base_gained() {
        let dir = diverged("cmp_two_dot");
        let files = changed_files(dir.path(), &range(Side::Rev("HEAD".into()), false)).unwrap();
        assert!(files.iter().any(|f| f.path == "from_main.txt" && f.letter == 'D'), "{files:?}");
    }

    #[test]
    fn the_working_tree_brings_in_uncommitted_and_untracked_changes() {
        let dir = diverged("cmp_worktree");
        dir.write("shared.txt", "one\nTWO\nthree\nfour\nfive\n");
        dir.write("scratch.txt", "a\nb\n");
        let committed = changed_files(dir.path(), &range(Side::Rev("HEAD".into()), true)).unwrap();
        assert!(!committed.iter().any(|f| f.path == "scratch.txt"));
        assert_eq!(committed.iter().find(|f| f.path == "shared.txt").unwrap().added, Some(2));

        let worktree = changed_files(dir.path(), &range(Side::WorkTree, true)).unwrap();
        let shared = worktree.iter().find(|f| f.path == "shared.txt").unwrap();
        assert_eq!(shared.added, Some(3), "the unsaved-to-git line counts too");
        assert!(!worktree.iter().any(|f| f.path == "from_main.txt"), "three-dot: main's own work isn't ours");
        let scratch = worktree.iter().find(|f| f.path == "scratch.txt").unwrap();
        assert_eq!((scratch.letter, scratch.added), ('?', Some(2)));
        let diff = changed_file_diff(dir.path(), &range(Side::WorkTree, true), scratch).unwrap();
        assert!(diff.contains("@@ -0,0 +1,2 @@\n+a\n+b\n"), "{diff}");
    }

    #[test]
    fn a_files_diff_follows_its_rename() {
        let dir = diverged("cmp_file_diff");
        let r = range(Side::Rev("HEAD".into()), true);
        let files = changed_files(dir.path(), &r).unwrap();
        let diff = changed_file_diff(dir.path(), &r, &files[0]).unwrap();
        assert!(diff.contains("rename from old_name.txt"), "{diff}");
        let diff = changed_file_diff(dir.path(), &r, &files[1]).unwrap();
        assert!(diff.contains("+TWO"));
    }

    #[test]
    fn a_binary_file_has_no_line_counts() {
        let dir = diverged("cmp_binary");
        std::fs::write(dir.path().join("blob.bin"), [0u8, 1, 2, 0, 3]).unwrap();
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-q", "-m", "blob"]);
        let files = changed_files(dir.path(), &range(Side::Rev("HEAD".into()), true)).unwrap();
        let blob = files.iter().find(|f| f.path == "blob.bin").unwrap();
        assert_eq!((blob.added, blob.removed), (None, None));
    }

    #[test]
    fn an_unknown_ref_is_gits_error() {
        let dir = diverged("cmp_unknown");
        let r = Range { base: "no-such-branch".into(), head: Side::Rev("HEAD".into()), three_dot: true };
        assert!(changed_files(dir.path(), &r).is_err());
        let r = Range { base: "no-such-branch".into(), head: Side::WorkTree, three_dot: true };
        assert!(changed_files(dir.path(), &r).is_err());
    }

    #[test]
    fn show_at_reads_a_file_at_a_ref() {
        let dir = diverged("cmp_show_at");
        assert_eq!(show_at(dir.path(), "main", "shared.txt").unwrap(), "one\ntwo\nthree\n");
        assert!(show_at(dir.path(), "main", "new_name.txt").is_err());
    }

    #[test]
    fn an_added_file_diff_without_a_final_newline_says_so() {
        let diff = added_file_diff("x.txt", "a\r\nb");
        assert!(diff.ends_with("+a\n+b\n\\ No newline at end of file\n"), "{diff}");
        assert!(diff.contains("@@ -0,0 +1,2 @@"));
    }

    #[test]
    fn a_remote_is_matched_by_its_whole_name() {
        let remotes = vec!["origin".to_string(), "team/fork".to_string(), "team".to_string()];
        assert_eq!(remote_among(&remotes, "origin/main").as_deref(), Some("origin"));
        assert_eq!(remote_among(&remotes, "team/fork/main").as_deref(), Some("team/fork"));
        assert_eq!(remote_among(&remotes, "refs/remotes/team/x").as_deref(), Some("team"));
        assert_eq!(remote_among(&remotes, "main"), None);
    }

    #[test]
    fn name_status_and_numstat_parse_renames() {
        let files = parse_name_status("M\0a.txt\0R087\0old.rs\0new.rs\0");
        assert_eq!(files[1].old_path.as_deref(), Some("old.rs"));
        assert_eq!(files[1].path, "new.rs");
        let stats = parse_numstat("1\t2\ta.txt\0" .to_string().as_str());
        assert_eq!(stats, vec![("a.txt".to_string(), Some(1), Some(2))]);
        let stats = parse_numstat("3\t0\t\0old.rs\0new.rs\0-\t-\tbin\0");
        assert_eq!(stats, vec![("new.rs".to_string(), Some(3), Some(0)), ("bin".to_string(), None, None)]);
    }
}
