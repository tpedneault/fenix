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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse;

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
