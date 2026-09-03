use crate::{DiffLine, FileDiff, FileStatus, Hunk, LineKind};

/// Parses unified diff text (`git diff`, `git show`, `git diff
/// base...head`, or a forge API's diff payload) into one `FileDiff` per
/// file.
///
/// Never fails: anything unrecognized is skipped rather than erroring,
/// the same "a bad entry loses only itself" posture `fenix-git`'s own
/// parsers and `fenix-config` already take. A diff this can't make sense
/// of yields fewer files or fewer hunks, never a panic and never a
/// half-applied patch, because `hunk_patch` only ever re-emits lines
/// this actually understood.
///
/// Splits on `'\n'` rather than using `str::lines`, which strips a
/// trailing `'\r'` -- see the crate doc comment for why that would
/// quietly corrupt every CRLF file.
pub fn parse(text: &str) -> Vec<FileDiff> {
    let mut files: Vec<FileDiff> = Vec::new();
    // How many old-side and new-side lines the hunk currently being read
    // still expects, straight from its own `@@` header. Counting down is
    // what tells us a hunk has ended, rather than guessing from the next
    // line's prefix -- which matters because a hunk's last line can be
    // followed by anything at all (another file's `diff --git`, a
    // trailing blank line, or `git show`'s commit trailer).
    let mut old_remaining = 0usize;
    let mut new_remaining = 0usize;
    let mut old_line = 0usize;
    let mut new_line = 0usize;

    for line in split_lines(text) {
        // A new file's header always ends whatever came before, however
        // incomplete it was.
        if line.starts_with("diff --git ") {
            files.push(FileDiff {
                old_path: String::new(),
                new_path: String::new(),
                status: FileStatus::Modified,
                is_binary: false,
                header: vec![line.to_string()],
                hunks: Vec::new(),
            });
            old_remaining = 0;
            new_remaining = 0;
            continue;
        }

        let Some(file) = files.last_mut() else { continue };
        let in_hunk = old_remaining > 0 || new_remaining > 0;

        if !in_hunk {
            if let Some((old_start, old_len, new_start, new_len)) = parse_hunk_header(line) {
                file.hunks.push(Hunk { old_start, old_len, new_start, new_len, header: line.to_string(), lines: Vec::new() });
                old_remaining = old_len;
                new_remaining = new_len;
                old_line = old_start;
                new_line = new_start;
                continue;
            }
            // A `\ No newline at end of file` marker trailing the last
            // line of a hunk arrives *after* that hunk's line counts are
            // already satisfied (it annotates the line before it rather
            // than being a line of its own), so it lands here rather
            // than in the in-hunk branch below. It still belongs to that
            // hunk: dropping it -- or worse, letting it fall through
            // into the header -- produces a patch `git apply` rejects
            // outright as "patch with only garbage".
            if line.starts_with('\\') {
                if let Some(hunk) = file.hunks.last_mut() {
                    hunk.lines.push(DiffLine { kind: LineKind::NoNewline, old_line: None, new_line: None, text: line[1..].to_string() });
                    continue;
                }
            }
            if !file.hunks.is_empty() {
                // Past this file's last hunk and not a marker or a new
                // `@@`: trailing text that isn't part of the diff at all
                // (`git show`'s commit trailer, a pager's footer, an
                // enclosing message). Ignored -- appending it to the
                // header would corrupt every patch built from this file.
                continue;
            }
            // Still in the file's preamble. Kept verbatim for patch
            // synthesis, and mined for the few facts the header is the
            // only source of.
            if line.starts_with("new file mode ") {
                file.status = FileStatus::Added;
            } else if line.starts_with("deleted file mode ") {
                file.status = FileStatus::Deleted;
            } else if line.starts_with("rename from ") || line.starts_with("rename to ") {
                file.status = FileStatus::Renamed;
            } else if line.starts_with("Binary files ") || line.starts_with("GIT binary patch") {
                file.is_binary = true;
            } else if let Some(path) = line.strip_prefix("--- ") {
                file.old_path = strip_path_prefix(path);
            } else if let Some(path) = line.strip_prefix("+++ ") {
                file.new_path = strip_path_prefix(path);
            }
            file.header.push(line.to_string());
            // A rename with no content change (`similarity index 100%`)
            // has a `rename from`/`rename to` pair and no `---`/`+++`
            // lines at all, so the paths come from there instead.
            if let Some(path) = line.strip_prefix("rename from ") {
                file.old_path = path.to_string();
            } else if let Some(path) = line.strip_prefix("rename to ") {
                file.new_path = path.to_string();
            }
            continue;
        }

        // Inside a hunk: classify by the one marker character.
        let Some(hunk) = file.hunks.last_mut() else { continue };
        let (kind, text) = match line.chars().next() {
            Some(' ') => (LineKind::Context, &line[1..]),
            Some('+') => (LineKind::Added, &line[1..]),
            Some('-') => (LineKind::Removed, &line[1..]),
            Some('\\') => (LineKind::NoNewline, &line[1..]),
            // A completely empty line where a context line was expected.
            // `git diff` itself always writes an empty context line as a
            // single space, but diffs that have been round-tripped
            // through something that strips trailing whitespace (a forge
            // API, a copy-paste, an editor) lose it -- treated as the
            // empty context line it plainly is, which also repairs it on
            // the way back out through `DiffLine::raw`.
            None => (LineKind::Context, line),
            // Anything else means the hunk's line counts lied about how
            // much content there was; stop reading it rather than
            // swallowing unrelated text into the hunk.
            Some(_) => {
                old_remaining = 0;
                new_remaining = 0;
                continue;
            }
        };

        let (old_no, new_no) = match kind {
            LineKind::Context => {
                let pair = (Some(old_line), Some(new_line));
                old_line += 1;
                new_line += 1;
                old_remaining = old_remaining.saturating_sub(1);
                new_remaining = new_remaining.saturating_sub(1);
                pair
            }
            LineKind::Added => {
                let pair = (None, Some(new_line));
                new_line += 1;
                new_remaining = new_remaining.saturating_sub(1);
                pair
            }
            LineKind::Removed => {
                let pair = (Some(old_line), None);
                old_line += 1;
                old_remaining = old_remaining.saturating_sub(1);
                pair
            }
            // Consumes no line on either side -- it annotates the
            // previous line rather than being one.
            LineKind::NoNewline => (None, None),
        };

        hunk.lines.push(DiffLine { kind, old_line: old_no, new_line: new_no, text: text.to_string() });
    }

    files
}

/// `text` split on `'\n'`, with the final empty piece a trailing newline
/// produces dropped -- so `"a\nb\n"` is two lines, not three, while
/// `"a\nb"` is still two.
fn split_lines(text: &str) -> impl Iterator<Item = &str> {
    let trimmed = text.strip_suffix('\n').unwrap_or(text);
    if trimmed.is_empty() && text.is_empty() {
        // `"".split('\n')` yields one empty string; an empty diff has no
        // lines at all.
        return Vec::new().into_iter();
    }
    trimmed.split('\n').collect::<Vec<_>>().into_iter()
}

/// Strips git's `a/`/`b/` diff-path prefixes. `/dev/null` (an added or
/// deleted file's other side) has no prefix and passes through
/// unchanged, as does any path from a diff generated with
/// `--no-prefix`.
fn strip_path_prefix(path: &str) -> String {
    // A `---`/`+++` line can carry a trailing tab plus timestamp in
    // non-git unified diffs; git's own output doesn't, but a diff from
    // elsewhere might, and keeping it would corrupt the path.
    let path = path.split('\t').next().unwrap_or(path);
    path.strip_prefix("a/").or_else(|| path.strip_prefix("b/")).unwrap_or(path).to_string()
}

/// `@@ -old_start[,old_len] +new_start[,new_len] @@[ heading]` -> the
/// four numbers. An omitted length means 1, which is git's own
/// convention for a single-line range. `None` for any line that isn't a
/// hunk header at all.
fn parse_hunk_header(line: &str) -> Option<(usize, usize, usize, usize)> {
    let rest = line.strip_prefix("@@ ")?;
    let (ranges, _) = rest.split_once(" @@")?;
    let (old, new) = ranges.split_once(' ')?;
    let (old_start, old_len) = parse_range(old.strip_prefix('-')?)?;
    let (new_start, new_len) = parse_range(new.strip_prefix('+')?)?;
    Some((old_start, old_len, new_start, new_len))
}

fn parse_range(range: &str) -> Option<(usize, usize)> {
    match range.split_once(',') {
        Some((start, len)) => Some((start.parse().ok()?, len.parse().ok()?)),
        None => Some((range.parse().ok()?, 1)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIMPLE: &str = "diff --git a/foo.txt b/foo.txt\nindex 83db48f..bf269f4 100644\n--- a/foo.txt\n+++ b/foo.txt\n@@ -1,3 +1,4 @@\n first\n-second\n+two\n+extra\n third\n";

    #[test]
    fn parses_one_file_with_its_paths_and_status() {
        let files = parse(SIMPLE);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].old_path, "foo.txt");
        assert_eq!(files[0].new_path, "foo.txt");
        assert_eq!(files[0].status, FileStatus::Modified);
        assert!(!files[0].is_binary);
    }

    #[test]
    fn keeps_the_whole_preamble_as_the_files_header() {
        let files = parse(SIMPLE);
        assert_eq!(
            files[0].header,
            vec![
                "diff --git a/foo.txt b/foo.txt".to_string(),
                "index 83db48f..bf269f4 100644".to_string(),
                "--- a/foo.txt".to_string(),
                "+++ b/foo.txt".to_string(),
            ]
        );
    }

    #[test]
    fn parses_the_hunk_header_numbers_and_keeps_the_line_verbatim() {
        let hunk = &parse(SIMPLE)[0].hunks[0];
        assert_eq!((hunk.old_start, hunk.old_len, hunk.new_start, hunk.new_len), (1, 3, 1, 4));
        assert_eq!(hunk.header, "@@ -1,3 +1,4 @@");
    }

    #[test]
    fn classifies_every_line_and_numbers_both_sides() {
        let hunk = &parse(SIMPLE)[0].hunks[0];
        let got: Vec<(LineKind, Option<usize>, Option<usize>, &str)> =
            hunk.lines.iter().map(|l| (l.kind, l.old_line, l.new_line, l.text.as_str())).collect();
        assert_eq!(
            got,
            vec![
                (LineKind::Context, Some(1), Some(1), "first"),
                (LineKind::Removed, Some(2), None, "second"),
                (LineKind::Added, None, Some(2), "two"),
                (LineKind::Added, None, Some(3), "extra"),
                (LineKind::Context, Some(3), Some(4), "third"),
            ]
        );
    }

    #[test]
    fn a_hunk_header_with_no_lengths_means_one_line_each_side() {
        let files = parse("diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1 +1 @@\n-old\n+new\n");
        let hunk = &files[0].hunks[0];
        assert_eq!((hunk.old_start, hunk.old_len, hunk.new_start, hunk.new_len), (1, 1, 1, 1));
        assert_eq!(hunk.lines.len(), 2);
    }

    #[test]
    fn a_hunk_header_with_a_section_heading_keeps_it_in_the_header_line() {
        let files = parse("diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -10,2 +10,2 @@ fn foo() {\n-a\n+b\n");
        assert_eq!(files[0].hunks[0].header, "@@ -10,2 +10,2 @@ fn foo() {");
        assert_eq!(files[0].hunks[0].old_start, 10);
    }

    #[test]
    fn line_numbers_start_from_the_hunks_own_offsets_not_from_one() {
        let files = parse("diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -40,2 +50,2 @@\n context\n-gone\n+here\n");
        let lines = &files[0].hunks[0].lines;
        assert_eq!((lines[0].old_line, lines[0].new_line), (Some(40), Some(50)));
        assert_eq!((lines[1].old_line, lines[1].new_line), (Some(41), None));
        assert_eq!((lines[2].old_line, lines[2].new_line), (None, Some(51)));
    }

    #[test]
    fn parses_several_files_and_several_hunks_in_one_diff() {
        let text = "diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1,1 +1,1 @@\n-x\n+y\n@@ -10,1 +10,1 @@\n-p\n+q\ndiff --git a/b b/b\n--- a/b\n+++ b/b\n@@ -1,1 +1,1 @@\n-m\n+n\n";
        let files = parse(text);
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].hunks.len(), 2);
        assert_eq!(files[1].hunks.len(), 1);
        assert_eq!(files[1].new_path, "b");
    }

    #[test]
    fn recognizes_an_added_file() {
        let text = "diff --git a/new.txt b/new.txt\nnew file mode 100644\nindex 0000000..3b18e51\n--- /dev/null\n+++ b/new.txt\n@@ -0,0 +1 @@\n+hello\n";
        let file = &parse(text)[0];
        assert_eq!(file.status, FileStatus::Added);
        assert_eq!(file.old_path, "/dev/null");
        assert_eq!(file.new_path, "new.txt");
        assert_eq!(file.display_path(), "new.txt");
    }

    #[test]
    fn recognizes_a_deleted_file_and_names_it_by_its_old_path() {
        let text = "diff --git a/gone.txt b/gone.txt\ndeleted file mode 100644\nindex 3b18e51..0000000\n--- a/gone.txt\n+++ /dev/null\n@@ -1 +0,0 @@\n-hello\n";
        let file = &parse(text)[0];
        assert_eq!(file.status, FileStatus::Deleted);
        assert_eq!(file.new_path, "/dev/null");
        assert_eq!(file.display_path(), "gone.txt");
    }

    #[test]
    fn recognizes_a_pure_rename_with_no_hunks() {
        let text = "diff --git a/old.txt b/new.txt\nsimilarity index 100%\nrename from old.txt\nrename to new.txt\n";
        let file = &parse(text)[0];
        assert_eq!(file.status, FileStatus::Renamed);
        assert_eq!(file.old_path, "old.txt");
        assert_eq!(file.new_path, "new.txt");
        assert!(file.hunks.is_empty());
    }

    #[test]
    fn recognizes_a_binary_file() {
        let text = "diff --git a/logo.png b/logo.png\nindex 1234567..89abcde 100644\nBinary files a/logo.png and b/logo.png differ\n";
        let file = &parse(text)[0];
        assert!(file.is_binary);
        assert!(file.hunks.is_empty());
    }

    #[test]
    fn keeps_the_no_newline_marker_as_its_own_line_consuming_neither_side() {
        let text = "diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1 +1 @@\n-old\n\\ No newline at end of file\n+new\n";
        let lines = &parse(text)[0].hunks[0].lines;
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[1].kind, LineKind::NoNewline);
        assert_eq!(lines[1].old_line, None);
        assert_eq!(lines[1].new_line, None);
        assert_eq!(lines[1].raw(), "\\ No newline at end of file");
        // The `+new` after it still gets the line number the marker
        // didn't consume.
        assert_eq!(lines[2].new_line, Some(1));
    }

    #[test]
    fn a_trailing_carriage_return_is_content_and_survives_parsing() {
        // The CRLF hazard this crate exists to get right: `str::lines`
        // would silently eat the `\r` here, and the patch built back out
        // of it would no longer match the file on disk.
        let text = "diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1 +1 @@\n-old\r\n+new\r\n";
        let lines = &parse(text)[0].hunks[0].lines;
        assert_eq!(lines[0].text, "old\r");
        assert_eq!(lines[1].text, "new\r");
        assert_eq!(lines[1].raw(), "+new\r");
    }

    #[test]
    fn stops_reading_a_hunk_once_its_declared_line_counts_are_satisfied() {
        // `git show` puts a commit trailer after the diff; none of it
        // should end up inside the last hunk.
        let text = "diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1 +1 @@\n-x\n+y\nnot part of the hunk\n";
        let file = &parse(text)[0];
        assert_eq!(file.hunks[0].lines.len(), 2);
        // ...nor in the file's header, which is re-emitted verbatim into
        // every patch built from it -- a stray line there is a patch
        // `git apply` rejects.
        assert_eq!(file.header, vec!["diff --git a/a b/a".to_string(), "--- a/a".to_string(), "+++ b/a".to_string()]);
    }

    #[test]
    fn a_no_newline_marker_trailing_the_last_line_of_a_hunk_belongs_to_that_hunk() {
        // The real bug this caught: with both sides' line counts already
        // satisfied by `+BETA`, the marker after it fell out of the hunk
        // and into the file header, and `git apply` rejected the
        // resulting patch as "patch with only garbage at <stdin>:5".
        let text = "diff --git a/n.txt b/n.txt\n--- a/n.txt\n+++ b/n.txt\n@@ -1,2 +1,2 @@\n alpha\n-beta\n\\ No newline at end of file\n+BETA\n\\ No newline at end of file\n";
        let file = &parse(text)[0];
        assert_eq!(file.header.len(), 3, "the trailing marker must not land in the header: {:?}", file.header);
        let lines = &file.hunks[0].lines;
        assert_eq!(lines.len(), 5);
        assert_eq!(lines[4].kind, LineKind::NoNewline);
        assert_eq!(lines[4].raw(), "\\ No newline at end of file");
    }

    #[test]
    fn an_empty_line_inside_a_hunk_is_read_as_an_empty_context_line() {
        let text = "diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1,3 +1,3 @@\n first\n\n-x\n+y\n";
        let lines = &parse(text)[0].hunks[0].lines;
        assert_eq!(lines[1].kind, LineKind::Context);
        assert_eq!(lines[1].text, "");
        // ...and comes back out as the space-prefixed form git wants.
        assert_eq!(lines[1].raw(), " ");
    }

    #[test]
    fn empty_input_yields_no_files() {
        assert!(parse("").is_empty());
    }

    #[test]
    fn text_that_is_not_a_diff_at_all_yields_no_files() {
        assert!(parse("just some text\nnothing diff-shaped here\n").is_empty());
    }

    #[test]
    fn a_path_with_spaces_survives_prefix_stripping() {
        let text = "diff --git a/my dir/a b.txt b/my dir/a b.txt\n--- a/my dir/a b.txt\n+++ b/my dir/a b.txt\n@@ -1 +1 @@\n-x\n+y\n";
        assert_eq!(parse(text)[0].new_path, "my dir/a b.txt");
    }

    #[test]
    fn a_diff_with_no_trailing_newline_still_parses_its_last_line() {
        let text = "diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1 +1 @@\n-x\n+y";
        assert_eq!(parse(text)[0].hunks[0].lines.len(), 2);
    }
}
