use std::path::{Path, PathBuf};
use std::process::Command;

/// Directories the last-resort plain walk skips outright -- there's no
/// `.gitignore` parsing in that fallback path, so this is a small,
/// hardcoded stand-in for the most common "never want these" directories
/// rather than an attempt at real ignore-file support.
const WALK_IGNORE: &[&str] = &[".git", "node_modules", "target", ".venv", "__pycache__", "dist", "build"];

/// Lists every file under `root`, as absolute paths. Tries, in order:
/// `git ls-files` (when `root` looks like a git repo -- gitignore-aware
/// for free, zero parsing on our end), `fd --type f` (gitignore-aware
/// too, when installed), then a plain recursive walk skipping
/// `WALK_IGNORE` as a last resort. Never fails outright -- the walk
/// always produces something, even if slower and less precise than the
/// other two.
pub fn list_project_files(root: &Path) -> Vec<PathBuf> {
    if root.join(".git").exists() {
        if let Some(files) = git_ls_files(root) {
            return files;
        }
    }
    if let Some(files) = fd_list_files(root) {
        return files;
    }
    walk_files(root)
}

fn git_ls_files(root: &Path) -> Option<Vec<PathBuf>> {
    let output =
        Command::new("git").args(["ls-files", "--cached", "--others", "--exclude-standard"]).current_dir(root).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    Some(stdout.lines().map(|line| root.join(line)).collect())
}

fn fd_list_files(root: &Path) -> Option<Vec<PathBuf>> {
    let output = Command::new("fd").args(["--type", "f", "--hidden", "--exclude", ".git"]).current_dir(root).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    Some(stdout.lines().map(|line| root.join(line)).collect())
}

fn walk_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    walk_dir(root, &mut files);
    files
}

fn walk_dir(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name();
        if WALK_IGNORE.iter().any(|ignored| name == std::ffi::OsStr::new(ignored)) {
            continue;
        }
        let Ok(file_type) = entry.file_type() else { continue };
        if file_type.is_dir() {
            walk_dir(&entry.path(), out);
        } else if file_type.is_file() {
            out.push(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::TempDir;

    fn git(dir: &Path, args: &[&str]) {
        let status = Command::new("git").args(args).current_dir(dir).status().expect("run git");
        assert!(status.success(), "git {args:?} failed");
    }

    fn init_repo(dir: &Path) {
        git(dir, &["init", "-q"]);
        git(dir, &["config", "user.email", "test@example.com"]);
        git(dir, &["config", "user.name", "Test"]);
    }

    #[test]
    fn git_ls_files_includes_tracked_and_untracked_excludes_ignored() {
        let dir = TempDir::new("git_ls_files");
        init_repo(dir.path());
        dir.write("tracked.txt", "x");
        git(dir.path(), &["add", "tracked.txt"]);
        git(dir.path(), &["commit", "-q", "-m", "initial"]);
        dir.write("untracked.txt", "x");
        dir.write(".gitignore", "ignored.txt\n");
        dir.write("ignored.txt", "x");

        let files = git_ls_files(dir.path()).expect("git available");
        let names: Vec<String> = files.iter().map(|p| p.strip_prefix(dir.path()).unwrap().to_string_lossy().into_owned()).collect();
        assert!(names.contains(&"tracked.txt".to_string()));
        assert!(names.contains(&"untracked.txt".to_string()));
        assert!(!names.contains(&"ignored.txt".to_string()));
    }

    #[test]
    fn walk_files_skips_the_hardcoded_ignore_list() {
        let dir = TempDir::new("walk_files_ignore");
        dir.write("real.txt", "x");
        dir.write("target/debug/build_output.txt", "x");
        dir.write("node_modules/pkg/index.js", "x");

        let files = walk_files(dir.path());
        let names: Vec<String> = files.iter().map(|p| p.strip_prefix(dir.path()).unwrap().to_string_lossy().into_owned()).collect();
        assert!(names.iter().any(|n| n == "real.txt"));
        assert!(!names.iter().any(|n| n.starts_with("target")));
        assert!(!names.iter().any(|n| n.starts_with("node_modules")));
    }

    #[test]
    fn walk_files_recurses_into_ordinary_subdirectories() {
        let dir = TempDir::new("walk_files_recurse");
        dir.write("src/deep/nested.rs", "x");
        let files = walk_files(dir.path());
        assert!(files.iter().any(|p| p.ends_with("src/deep/nested.rs")));
    }

    #[test]
    fn list_project_files_finds_files_in_a_real_git_repo() {
        let dir = TempDir::new("list_project_files_git");
        init_repo(dir.path());
        dir.write("a.rs", "x");
        dir.write("sub/b.rs", "x");
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-q", "-m", "initial"]);

        let files = list_project_files(dir.path());
        let names: Vec<String> = files.iter().map(|p| p.strip_prefix(dir.path()).unwrap().to_string_lossy().into_owned()).collect();
        assert!(names.contains(&"a.rs".to_string()));
        assert!(names.iter().any(|n| n.ends_with("b.rs")));
    }

    #[test]
    fn list_project_files_finds_files_without_git() {
        let dir = TempDir::new("list_project_files_no_git");
        dir.write("plain.txt", "x");
        let files = list_project_files(dir.path());
        assert!(files.iter().any(|p| p.ends_with("plain.txt")));
    }
}
