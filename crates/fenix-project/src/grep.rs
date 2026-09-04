use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrepMatch {
    pub path: PathBuf,
    pub line: usize,
    pub col: usize,
    pub text: String,
}

/// Searches `root` for `query` via `rg --vimgrep` -- `query` is passed
/// through as-is, so it's already a regex, not just a literal string
/// (same as real ripgrep/`projectile-ripgrep`). Returns a clear error
/// rather than silently degrading to a slower hand-rolled search if `rg`
/// isn't on `PATH` -- disclosed hard requirement for this one feature,
/// per the plan's scope notes.
pub fn grep_project(root: &Path, query: &str) -> io::Result<Vec<GrepMatch>> {
    let output = Command::new("rg")
        .args(["--vimgrep", "--", query])
        .current_dir(root)
        .output()
        .map_err(|_| io::Error::new(io::ErrorKind::NotFound, "ripgrep (rg) not found on PATH"))?;

    // Exit code 1 just means "no matches" for rg, not a real error --
    // only something else (bad pattern, I/O failure) should surface.
    if !output.status.success() && output.status.code() != Some(1) {
        return Err(io::Error::other(String::from_utf8_lossy(&output.stderr).into_owned()));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(stdout.lines().filter_map(|line| parse_vimgrep_line(root, line)).collect())
}

/// Parses one `path:line:col:text` line. Splits on at most the first
/// three colons, since `text` (the matched line's own content) may
/// itself contain colons.
fn parse_vimgrep_line(root: &Path, line: &str) -> Option<GrepMatch> {
    let mut parts = line.splitn(4, ':');
    let path = parts.next()?;
    let line_no: usize = parts.next()?.parse().ok()?;
    let col: usize = parts.next()?.parse().ok()?;
    let text = parts.next().unwrap_or("").to_string();
    Some(GrepMatch { path: root.join(path), line: line_no, col, text })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::TempDir;

    /// Whether `rg` is on `PATH`. Ripgrep is a disclosed hard
    /// requirement for this one feature (see `grep_project`), so on a
    /// machine without it these have nothing to exercise -- they say so
    /// and stop, rather than failing forever and drowning out a real
    /// regression next to them.
    fn ripgrep_available() -> bool {
        std::process::Command::new("rg").arg("--version").output().is_ok_and(|o| o.status.success())
    }

    #[test]
    fn finds_a_match_with_correct_position_and_text() {
        if !ripgrep_available() {
            eprintln!("skipping finds_a_match_with_correct_position_and_text: ripgrep (rg) is not on PATH");
            return;
        }
        let dir = TempDir::new("grep_finds_match");
        dir.write("a.txt", "hello\nneedle here\nworld\n");

        let matches = grep_project(dir.path(), "needle").unwrap();
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].line, 2);
        assert_eq!(matches[0].col, 1);
        assert_eq!(matches[0].text, "needle here");
        assert!(matches[0].path.ends_with("a.txt"));
    }

    #[test]
    fn no_matches_is_ok_with_an_empty_list_not_an_error() {
        if !ripgrep_available() {
            eprintln!("skipping no_matches_is_ok_with_an_empty_list_not_an_error: ripgrep (rg) is not on PATH");
            return;
        }
        let dir = TempDir::new("grep_no_matches");
        dir.write("a.txt", "nothing interesting here\n");
        let matches = grep_project(dir.path(), "totally_absent_pattern_xyz").unwrap();
        assert!(matches.is_empty());
    }

    #[test]
    fn finds_matches_across_multiple_files() {
        if !ripgrep_available() {
            eprintln!("skipping finds_matches_across_multiple_files: ripgrep (rg) is not on PATH");
            return;
        }
        let dir = TempDir::new("grep_multi_file");
        dir.write("a.txt", "shared_term\n");
        dir.write("sub/b.txt", "also has shared_term here\n");

        let matches = grep_project(dir.path(), "shared_term").unwrap();
        assert_eq!(matches.len(), 2);
    }

    #[test]
    fn parse_vimgrep_line_handles_colons_inside_the_matched_text() {
        let root = Path::new("/repo");
        let parsed = parse_vimgrep_line(root, "src/main.rs:10:5:let x: usize = 1;").unwrap();
        assert_eq!(parsed.line, 10);
        assert_eq!(parsed.col, 5);
        assert_eq!(parsed.text, "let x: usize = 1;");
        assert_eq!(parsed.path, root.join("src/main.rs"));
    }

    #[test]
    fn parse_vimgrep_line_rejects_malformed_input() {
        assert!(parse_vimgrep_line(Path::new("/repo"), "not a vimgrep line").is_none());
        assert!(parse_vimgrep_line(Path::new("/repo"), "path:notanumber:5:text").is_none());
    }
}
