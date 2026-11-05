use std::path::Path;

use crate::process::run_lines;

/// One changed/untracked/conflicted file, as `git status --porcelain=v2`
/// reports it -- `index_status`/`worktree_status` are the raw `X`/`Y`
/// status-code characters from git's own two-column convention (`X` =
/// staged/index state, `Y` = unstaged/worktree state; `' '` means
/// "unchanged in that column"), not translated into this crate's own
/// vocabulary, so the renderer (`fenix-gui`'s `git_panel.rs`) can map
/// them to badges/colors however it wants without this crate needing to
/// know about themes or UI concerns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEntry {
    pub path: String,
    pub index_status: char,
    pub worktree_status: char,
}

/// `git status --porcelain=v2`'s entry lines, parsed -- empty (not an
/// error) outside a git repo, without `git` on `PATH`, or in a clean
/// working tree with nothing to show.
pub fn list_files(repo: &Path) -> Vec<FileEntry> {
    run_lines(repo, &["status", "--porcelain=v2"]).iter().filter_map(|l| parse_line(l)).collect()
}

/// Parses entry lines out of a `status --porcelain=v2 --branch` output
/// (or plain `--porcelain=v2`, since the header lines this skips simply
/// don't match any of the four line-kind prefixes below) -- shared with
/// `status::status_and_files`, which shells the `--branch` form once and
/// needs both the header lines *and* these entry lines from the same
/// output.
pub(crate) fn parse_files(lines: &[String]) -> Vec<FileEntry> {
    lines.iter().filter_map(|l| parse_line(l)).collect()
}

/// Verified against real `git status --porcelain=v2` output (run
/// directly in this repo this session) and `git help status`'s own
/// documented Porcelain Format Version 2 field layout for each of the
/// three real entry kinds it can emit for changed paths -- not guessed:
/// - Ordinary: `1 XY sub mH mI mW hH hI path` (8 fields before path)
/// - Renamed/copied: `2 XY sub mH mI mW hH hI Xscore path<TAB>origPath`
///   (9 fields before the path+origPath pair; only the *new* path is
///   kept, matching how the caller only ever acts on the file as it
///   exists now)
/// - Unmerged/conflict: `u XY sub m1 m2 m3 mW h1 h2 h3 path` (10 fields
///   before path -- three merge-stage mode fields instead of ordinary's
///   one)
/// - Untracked: `? path` (no status columns at all -- both status chars
///   are synthesized as `?` here so the renderer can treat it uniformly
///   with every other kind)
///
/// Ignored (`!`) lines never appear since `--ignored` isn't passed.
fn parse_line(line: &str) -> Option<FileEntry> {
    let (kind, rest) = line.split_once(' ')?;
    match kind {
        "1" => {
            let mut fields = rest.splitn(8, ' ');
            let xy = fields.next()?;
            let path = fields.nth(6)?; // sub, mH, mI, mW, hH, hI, then path
            let (index_status, worktree_status) = xy_chars(xy)?;
            Some(FileEntry { path: path.to_string(), index_status, worktree_status })
        }
        "2" => {
            let mut fields = rest.splitn(9, ' ');
            let xy = fields.next()?;
            let path_and_orig = fields.nth(7)?; // sub, mH, mI, mW, hH, hI, Xscore, then path<TAB>orig
            let path = path_and_orig.split('\t').next()?;
            let (index_status, worktree_status) = xy_chars(xy)?;
            Some(FileEntry { path: path.to_string(), index_status, worktree_status })
        }
        "u" => {
            let mut fields = rest.splitn(10, ' ');
            let xy = fields.next()?;
            let path = fields.nth(8)?; // sub, m1, m2, m3, mW, h1, h2, h3, then path
            let (index_status, worktree_status) = xy_chars(xy)?;
            Some(FileEntry { path: path.to_string(), index_status, worktree_status })
        }
        "?" => Some(FileEntry { path: rest.to_string(), index_status: '?', worktree_status: '?' }),
        _ => None,
    }
}

fn xy_chars(xy: &str) -> Option<(char, char)> {
    let mut chars = xy.chars();
    Some((chars.next()?, chars.next()?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{git, init_repo, TempDir};

    #[test]
    fn parse_line_reads_an_ordinary_unstaged_modification() {
        let entry = parse_line("1 .M N... 100644 100644 100644 abc abc test.tcl").unwrap();
        assert_eq!(entry, FileEntry { path: "test.tcl".to_string(), index_status: '.', worktree_status: 'M' });
    }

    #[test]
    fn parse_line_reads_an_untracked_file() {
        let entry = parse_line("? new_file.txt").unwrap();
        assert_eq!(entry, FileEntry { path: "new_file.txt".to_string(), index_status: '?', worktree_status: '?' });
    }

    #[test]
    fn parse_line_reads_a_staged_addition() {
        let entry = parse_line("1 A. N... 000000 100644 100644 abc abc new.txt").unwrap();
        assert_eq!(entry, FileEntry { path: "new.txt".to_string(), index_status: 'A', worktree_status: '.' });
    }

    #[test]
    fn parse_line_reads_a_rename_keeping_only_the_new_path() {
        let entry = parse_line("2 R. N... 100644 100644 100644 abc abc R100 new_name.txt\told_name.txt").unwrap();
        assert_eq!(entry, FileEntry { path: "new_name.txt".to_string(), index_status: 'R', worktree_status: '.' });
    }

    #[test]
    fn parse_line_reads_an_unmerged_conflict() {
        let entry = parse_line("u UU N... 100644 100644 100644 100644 abc abc abc conflicted.txt").unwrap();
        assert_eq!(entry, FileEntry { path: "conflicted.txt".to_string(), index_status: 'U', worktree_status: 'U' });
    }

    #[test]
    fn parse_line_ignores_an_unrecognized_kind() {
        assert!(parse_line("! ignored.txt").is_none());
        assert!(parse_line("garbage").is_none());
    }

    #[test]
    fn list_files_empty_outside_a_git_repo() {
        let dir = TempDir::new("files_no_repo");
        assert!(list_files(dir.path()).is_empty());
    }

    #[test]
    fn list_files_detects_untracked_modified_and_staged_files() {
        let dir = TempDir::new("files_states");
        init_repo(dir.path());
        dir.write("committed.txt", "v1");
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-q", "-m", "initial"]);

        dir.write("committed.txt", "v2"); // unstaged modification
        dir.write("staged.txt", "new");
        git(dir.path(), &["add", "staged.txt"]);
        dir.write("untracked.txt", "new");

        let files = list_files(dir.path());
        let by_path = |p: &str| files.iter().find(|f| f.path == p);
        assert_eq!(by_path("committed.txt"), Some(&FileEntry { path: "committed.txt".to_string(), index_status: '.', worktree_status: 'M' }));
        assert_eq!(by_path("staged.txt"), Some(&FileEntry { path: "staged.txt".to_string(), index_status: 'A', worktree_status: '.' }));
        assert_eq!(by_path("untracked.txt"), Some(&FileEntry { path: "untracked.txt".to_string(), index_status: '?', worktree_status: '?' }));
    }
}
