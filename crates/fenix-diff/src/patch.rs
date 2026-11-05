use crate::{FileDiff, Hunk};

/// Builds a standalone patch containing exactly one hunk of one file --
/// what `git apply --cached` (stage this hunk), `git apply --cached
/// --reverse` (unstage it), or `git apply --reverse` (discard it from
/// the working tree) each take on stdin.
///
/// There's deliberately no "direction" parameter: the patch is always
/// the forward one git itself produced, and *applying* it in reverse is
/// `git apply`'s own `--reverse` flag. One patch shape, three uses,
/// nothing to get backwards here.
///
/// The file's original preamble is re-emitted verbatim rather than
/// rebuilt from `old_path`/`new_path`, so the `index`/`new file mode`/
/// `rename from` lines git wrote (and may care about on the way back in)
/// survive exactly as they were. The hunk's own `@@` line is likewise
/// the original, kept whole -- including any section heading, which git
/// ignores but which costs nothing to preserve and keeps the bytes
/// identical to what git emitted.
///
/// The result always ends in a newline; `git apply` rejects a patch
/// whose final line is unterminated.
pub fn hunk_patch(file: &FileDiff, hunk: &Hunk) -> String {
    let mut out = String::new();
    for line in &file.header {
        out.push_str(line);
        out.push('\n');
    }
    out.push_str(&hunk.header);
    out.push('\n');
    for line in &hunk.lines {
        out.push_str(&line.raw());
        out.push('\n');
    }
    out
}

/// A patch of only some of a hunk's lines -- what staging, unstaging or
/// discarding a Visual selection needs. `selected` are indices into
/// `hunk.lines`; `None` when they pick no added or removed line.
///
/// A line that isn't selected must come out as it stands in whatever
/// the patch is applied *to*. Applied forward (`reverse` false: staging
/// from the working-tree diff) that's the old side, so an unselected
/// removal stays as context and an unselected addition is dropped.
/// Applied with `--reverse` (unstaging from the staged diff, discarding
/// from the working-tree diff) the target holds the new side, so it's
/// the other way round. The same rule `git add -p`'s line editing uses.
pub fn lines_patch(file: &FileDiff, hunk: &Hunk, selected: &[usize], reverse: bool) -> Option<String> {
    use crate::LineKind;
    let picked = |i: usize| selected.contains(&i);
    if !hunk.lines.iter().enumerate().any(|(i, l)| picked(i) && matches!(l.kind, LineKind::Added | LineKind::Removed)) {
        return None;
    }
    let mut body: Vec<String> = Vec::new();
    let (mut old_len, mut new_len) = (0, 0);
    // Whether the previous line of the hunk made it into the patch -- a
    // "\ No newline" marker belongs to the line before it.
    let mut kept_previous = false;
    for (i, line) in hunk.lines.iter().enumerate() {
        let kind = match (line.kind, picked(i), reverse) {
            (LineKind::NoNewline, ..) => {
                if kept_previous {
                    body.push(line.raw());
                }
                continue;
            }
            (LineKind::Context, ..) => Some(' '),
            (LineKind::Added, true, _) => Some('+'),
            (LineKind::Removed, true, _) => Some('-'),
            (LineKind::Added, false, false) | (LineKind::Removed, false, true) => None,
            (LineKind::Added, false, true) | (LineKind::Removed, false, false) => Some(' '),
        };
        kept_previous = kind.is_some();
        let Some(marker) = kind else { continue };
        match marker {
            ' ' => {
                old_len += 1;
                new_len += 1;
            }
            '-' => old_len += 1,
            _ => new_len += 1,
        }
        body.push(format!("{marker}{}", line.text));
    }
    let old_start = hunk.old_start;
    let new_start = if old_len == 0 {
        old_start + 1
    } else if new_len == 0 {
        old_start.saturating_sub(1)
    } else {
        old_start
    };
    // Keep git's section heading, the text after the second "@@".
    let heading = hunk.header.splitn(3, "@@").nth(2).unwrap_or("");
    let mut out = String::new();
    for line in &file.header {
        out.push_str(line);
        out.push('\n');
    }
    out.push_str(&format!("@@ -{old_start},{old_len} +{new_start},{new_len} @@{heading}\n"));
    for line in body {
        out.push_str(&line);
        out.push('\n');
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse;

    const THREE_ADDED: &str = "diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1,3 +1,5 @@ fn main\n one\n-two\n+TWO\n+extra\n three\n";

    #[test]
    fn staging_some_lines_drops_other_additions_and_keeps_other_removals() {
        let files = parse(THREE_ADDED);
        // Lines: 0 " one", 1 "-two", 2 "+TWO", 3 "+extra", 4 " three".
        let patch = lines_patch(&files[0], &files[0].hunks[0], &[3], false).unwrap();
        assert_eq!(patch, "diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1,3 +1,4 @@ fn main\n one\n two\n+extra\n three\n");
    }

    #[test]
    fn unstaging_some_lines_keeps_other_additions_and_drops_other_removals() {
        let files = parse(THREE_ADDED);
        let patch = lines_patch(&files[0], &files[0].hunks[0], &[1, 2], true).unwrap();
        assert_eq!(patch, "diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1,4 +1,4 @@ fn main\n one\n-two\n+TWO\n extra\n three\n");
    }

    #[test]
    fn a_selection_of_only_context_is_no_patch() {
        let files = parse(THREE_ADDED);
        assert_eq!(lines_patch(&files[0], &files[0].hunks[0], &[0, 4], false), None);
    }

    /// The patch is only right if git takes it: stage one added line of
    /// three with the real `git apply --cached`.
    #[test]
    fn git_applies_a_lines_patch_to_the_index() {
        use std::process::Command;
        let dir = std::env::temp_dir().join(format!("fenix-diff-lines-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let git = |args: &[&str]| {
            let out = Command::new("git").current_dir(&dir).args(args).output().unwrap();
            assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
            String::from_utf8_lossy(&out.stdout).into_owned()
        };
        git(&["init", "-q"]);
        git(&["config", "core.autocrlf", "false"]);
        std::fs::write(dir.join("f"), "one\ntwo\nthree\n").unwrap();
        git(&["add", "f"]);
        git(&["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", "init"]);
        std::fs::write(dir.join("f"), "one\nTWO\nextra\nthree\n").unwrap();
        let files = parse(&git(&["diff", "--no-color"]));
        let hunk = &files[0].hunks[0];
        let extra = hunk.lines.iter().position(|l| l.text == "extra").unwrap();
        let patch = lines_patch(&files[0], hunk, &[extra], false).unwrap();
        let mut child = Command::new("git").current_dir(&dir).args(["apply", "--cached", "-"]).stdin(std::process::Stdio::piped()).spawn().unwrap();
        std::io::Write::write_all(child.stdin.as_mut().unwrap(), patch.as_bytes()).unwrap();
        assert!(child.wait().unwrap().success(), "git refused:\n{patch}");
        assert_eq!(git(&["show", ":f"]), "one\ntwo\nextra\nthree\n", "only the chosen line is staged");

        // And back out: stage everything, then unstage just TWO.
        git(&["add", "f"]);
        let files = parse(&git(&["diff", "--cached", "--no-color"]));
        let hunk = &files[0].hunks[0];
        let pick: Vec<usize> = hunk.lines.iter().enumerate().filter(|(_, l)| l.text.eq_ignore_ascii_case("two")).map(|(i, _)| i).collect();
        let patch = lines_patch(&files[0], hunk, &pick, true).unwrap();
        let mut child =
            Command::new("git").current_dir(&dir).args(["apply", "--cached", "--reverse", "-"]).stdin(std::process::Stdio::piped()).spawn().unwrap();
        std::io::Write::write_all(child.stdin.as_mut().unwrap(), patch.as_bytes()).unwrap();
        assert!(child.wait().unwrap().success(), "git refused:\n{patch}");
        assert_eq!(git(&["show", ":f"]), "one\ntwo\nextra\nthree\n", "TWO unstaged, extra still staged");
        let _ = std::fs::remove_dir_all(&dir);
    }

    const TWO_HUNKS: &str = "diff --git a/foo.txt b/foo.txt\nindex 83db48f..bf269f4 100644\n--- a/foo.txt\n+++ b/foo.txt\n@@ -1,3 +1,3 @@\n first\n-second\n+two\n third\n@@ -20,3 +20,3 @@ fn later() {\n twentieth\n-old\n+new\n twenty-second\n";

    #[test]
    fn a_patch_carries_the_files_header_and_only_the_chosen_hunk() {
        let files = parse(TWO_HUNKS);
        let patch = hunk_patch(&files[0], &files[0].hunks[0]);
        assert_eq!(
            patch,
            "diff --git a/foo.txt b/foo.txt\nindex 83db48f..bf269f4 100644\n--- a/foo.txt\n+++ b/foo.txt\n@@ -1,3 +1,3 @@\n first\n-second\n+two\n third\n"
        );
        assert!(!patch.contains("twentieth"), "the other hunk must not be in this patch");
    }

    #[test]
    fn the_second_hunks_patch_carries_that_hunk_and_its_section_heading() {
        let files = parse(TWO_HUNKS);
        let patch = hunk_patch(&files[0], &files[0].hunks[1]);
        assert!(patch.contains("@@ -20,3 +20,3 @@ fn later() {"));
        assert!(patch.contains("+new"));
        assert!(!patch.contains("+two"), "the first hunk must not be in this patch");
        assert!(!patch.contains("@@ -1,3"), "...nor its header");
    }

    #[test]
    fn a_patch_always_ends_with_a_newline() {
        // Even when the diff it came from didn't -- `git apply` rejects
        // an unterminated final line.
        let files = parse("diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1 +1 @@\n-x\n+y");
        assert!(hunk_patch(&files[0], &files[0].hunks[0]).ends_with('\n'));
    }

    #[test]
    fn a_patch_round_trips_back_through_the_parser_unchanged() {
        // The property that actually matters: what comes out is a valid
        // diff describing exactly the hunk that went in.
        let files = parse(TWO_HUNKS);
        let patch = hunk_patch(&files[0], &files[0].hunks[1]);
        let reparsed = parse(&patch);
        assert_eq!(reparsed.len(), 1);
        assert_eq!(reparsed[0].hunks.len(), 1);
        assert_eq!(reparsed[0].hunks[0], files[0].hunks[1]);
        assert_eq!(reparsed[0].header, files[0].header);
    }

    #[test]
    fn a_crlf_hunk_round_trips_byte_for_byte() {
        // The corruption this crate is built to avoid: every `\r` has to
        // still be there, in the right place, after a full parse ->
        // patch -> parse cycle.
        let text = "diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1,2 +1,2 @@\n keep\r\n-old\r\n+new\r\n";
        let files = parse(text);
        let patch = hunk_patch(&files[0], &files[0].hunks[0]);
        assert_eq!(patch, text);
        assert_eq!(parse(&patch)[0].hunks[0], files[0].hunks[0]);
    }

    #[test]
    fn a_no_newline_marker_survives_into_the_patch() {
        let text = "diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1 +1 @@\n-old\n\\ No newline at end of file\n+new\n";
        let files = parse(text);
        assert_eq!(hunk_patch(&files[0], &files[0].hunks[0]), text);
    }

    #[test]
    fn an_added_files_patch_keeps_its_new_file_mode_line() {
        let text = "diff --git a/new.txt b/new.txt\nnew file mode 100644\nindex 0000000..3b18e51\n--- /dev/null\n+++ b/new.txt\n@@ -0,0 +1 @@\n+hello\n";
        let files = parse(text);
        let patch = hunk_patch(&files[0], &files[0].hunks[0]);
        assert!(patch.contains("new file mode 100644"));
        assert_eq!(patch, text);
    }

    #[test]
    fn a_hunk_from_a_multi_file_diff_only_carries_its_own_files_header() {
        let text = "diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1 +1 @@\n-x\n+y\ndiff --git a/b b/b\n--- a/b\n+++ b/b\n@@ -1 +1 @@\n-m\n+n\n";
        let files = parse(text);
        let patch = hunk_patch(&files[1], &files[1].hunks[0]);
        assert!(patch.starts_with("diff --git a/b b/b\n"));
        assert!(!patch.contains("a/a"));
    }
}
