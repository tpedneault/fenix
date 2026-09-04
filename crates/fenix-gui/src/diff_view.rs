//! Renders parsed diffs (`fenix_diff::FileDiff`) into a real buffer's
//! worth of text plus per-line metadata -- the one diff surface behind
//! every view that shows changes: the Git panel's working tree, commit
//! detail, ref-to-ref comparison, and (later) merge-request review.
//!
//! The metadata is what makes it more than colored text. Every content
//! row carries a `DiffAnchor` naming the file, the hunk, and the old/new
//! line numbers it came from, so "stage the hunk under the cursor",
//! "open the real file at this line" and "comment on this line of the
//! MR" are all the same lookup against whatever row the cursor happens
//! to be on -- see `App::diff_anchor_at_cursor`.
//!
//! Mirrors `docker_panel`/`git_panel`/`jira_panel`'s own contract (a
//! `{text, lines}` pair for a dedicated `BufferKind`, colors resolved
//! by the host from per-line metadata), just with a richer per-line
//! payload than a style tag.

use std::collections::HashSet;

use fenix_diff::{FileDiff, FileStatus, LineKind};

/// Where a rendered row came from in the diff model. A file-header row
/// has no `hunk`; a hunk-header row has no line numbers; a content row
/// has whichever of old/new its kind gives it (an added line has no old
/// line, a removed line has no new one), exactly as `fenix_diff`
/// resolved them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiffAnchor {
    pub file: usize,
    pub hunk: Option<usize>,
    pub old_line: Option<usize>,
    pub new_line: Option<usize>,
}

/// How one rendered row should be colored -- same "tag the meaning, let
/// the host pick the color" split `git_panel::GitLineStyle` already
/// uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffStyle {
    /// A caller-supplied title above the diff (`commit abc1234`, a merge
    /// request's own heading) -- distinct from `FileHeader` even though
    /// they look alike, so that scanning the rendered rows for "the
    /// files in this diff" can't accidentally match the title.
    Title,
    /// The per-file summary row (`M  src/main.rs   +12 -3`).
    FileHeader,
    /// A `@@ -a,b +c,d @@` row.
    HunkHeader,
    Context,
    Added,
    Removed,
    /// A `\ No newline at end of file` marker, a `(binary file)` note,
    /// or the "nothing to show" placeholder -- dim, never something the
    /// user acts on.
    Meta,
    /// A review thread's own header row: who wrote it and whether it's
    /// resolved. Distinct from `NoteBody` so a thread is scannable
    /// among the diff lines it's wedged between.
    NoteHeader,
    /// A line of a review comment's text.
    NoteBody,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffViewLine {
    pub style: DiffStyle,
    /// Char column where this row's real content starts, past the
    /// line-number gutter. The host dims `0..content_from` and applies
    /// `style`'s color from there on, so the gutter never competes with
    /// the diff itself for attention. `0` on rows with no gutter (file
    /// and hunk headers).
    pub content_from: usize,
    pub anchor: Option<DiffAnchor>,
    /// The review thread this row belongs to, for the rows a comment
    /// contributes. `None` on every diff row -- which is also how
    /// "reply to the thread under the cursor" tells a comment row from
    /// the code it's attached to.
    pub thread: Option<String>,
}

/// The generated diff view: `text` is real content for a real
/// `BufferKind::Diff` buffer; `lines[i]` describes `text`'s line `i`.
pub struct DiffView {
    pub text: String,
    pub lines: Vec<Option<DiffViewLine>>,
}

struct Builder {
    text: String,
    lines: Vec<Option<DiffViewLine>>,
}

impl Builder {
    fn new() -> Self {
        Self { text: String::new(), lines: Vec::new() }
    }

    fn push(&mut self, text: &str, meta: Option<DiffViewLine>) {
        self.text.push_str(text);
        self.text.push('\n');
        self.lines.push(meta);
    }

    fn finish(self) -> DiffView {
        DiffView { text: self.text, lines: self.lines }
    }
}

fn status_marker(status: FileStatus) -> char {
    match status {
        FileStatus::Added => 'A',
        FileStatus::Deleted => 'D',
        FileStatus::Modified => 'M',
        FileStatus::Renamed => 'R',
    }
}

/// Width of each of the two line-number columns, sized to the largest
/// line number anywhere in `files` so the gutter never reflows
/// mid-scroll, with a floor of 3 (the common case -- a four-digit file
/// widens it, a two-line file doesn't shrink it to something cramped).
fn gutter_width(files: &[FileDiff]) -> usize {
    let max = files
        .iter()
        .flat_map(|f| f.hunks.iter())
        .map(|h| (h.old_start + h.old_len).max(h.new_start + h.new_len))
        .max()
        .unwrap_or(0);
    max.to_string().len().max(3)
}

/// One content row: `{old:>w} {new:>w} {marker}{text}`, where the
/// marker+text half is `fenix_diff`'s own verbatim line -- so what's on
/// screen past the gutter is exactly what's in the patch, which matters
/// when the thing being reviewed is whitespace.
fn content_row(width: usize, old: Option<usize>, new: Option<usize>, raw: &str) -> (String, usize) {
    let blank = " ".repeat(width);
    let old_col = old.map(|n| format!("{n:>width$}")).unwrap_or_else(|| blank.clone());
    let new_col = new.map(|n| format!("{n:>width$}")).unwrap_or(blank);
    let content_from = width * 2 + 2;
    (format!("{old_col} {new_col} {raw}"), content_from)
}

/// One review thread's rows, indented past the diff's own line-number
/// gutter so it reads as hanging off the line above rather than as more
/// diff.
fn push_thread(b: &mut Builder, gutter: usize, thread: &ThreadAnnotation, anchor: DiffAnchor) {
    let indent = " ".repeat(gutter * 2 + 3);
    let state = if thread.resolved { " (resolved)" } else { "" };
    let meta = |style: DiffStyle| {
        Some(DiffViewLine { style, content_from: indent.chars().count(), anchor: Some(anchor), thread: Some(thread.id.clone()) })
    };
    for (index, (author, body)) in thread.notes.iter().enumerate() {
        // Only the first note carries the resolved marker: it belongs to
        // the thread, not to each reply.
        let suffix = if index == 0 { state } else { "" };
        b.push(&format!("{indent}| {author}{suffix}:"), meta(DiffStyle::NoteHeader));
        for line in body.trim_end().split('\n') {
            b.push(&format!("{indent}|   {line}"), meta(DiffStyle::NoteBody));
        }
    }
    b.push(&format!("{indent}| r reply, R resolve"), meta(DiffStyle::Meta));
}

/// A review thread to show inline, under the diff line it hangs on.
///
/// Matched by path plus line rather than by index: the diff is fetched
/// separately from the threads, so nothing guarantees the two agree on
/// how many files there are or what order they're in. A thread whose
/// line isn't in the diff at all (the file was collapsed, or the
/// comment predates a force-push) simply isn't drawn -- silently
/// dropping it beats putting it on the wrong line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadAnnotation {
    /// Thread id, carried onto every row it produces so reply/resolve
    /// can find it from wherever the cursor lands.
    pub id: String,
    /// The file the thread's position names, as `display_path` gives it.
    pub path: String,
    pub old_line: Option<usize>,
    pub new_line: Option<usize>,
    pub resolved: bool,
    /// `(author, body)` per comment, oldest first.
    pub notes: Vec<(String, String)>,
}

impl ThreadAnnotation {
    /// Whether this hangs on the diff row with these line numbers --
    /// see `fenix_forge::Position::anchors_to`, which this mirrors and
    /// for the same reason: a forge records a context line's position
    /// with both numbers or with only the side that was clicked.
    fn anchors_to(&self, old: Option<usize>, new: Option<usize>) -> bool {
        match (self.new_line, self.old_line) {
            (Some(n), _) if new == Some(n) => true,
            (Some(_), Some(o)) | (None, Some(o)) => old == Some(o),
            _ => false,
        }
    }
}

/// Renders `files` into a diff buffer. `collapsed` holds the display
/// paths of files folded down to just their header row -- the same
/// "caller owns the expansion set, renderer just reads it" split
/// `git_panel::render_unstaged` already uses for its directory tree, so
/// folding survives a refresh without this module holding state.
pub fn render(files: &[FileDiff], collapsed: &HashSet<String>) -> DiffView {
    render_with_header(&[], files, collapsed)
}

/// `render`, preceded by caller-supplied rows.
///
/// The header is what makes a *commit* readable rather than just its
/// changes: `git show` puts the author, date and message above the diff,
/// but `fenix_diff::parse` deliberately keeps only the `diff --git`
/// sections, so without this the commit pane showed a diff with no
/// indication of whose commit it was or what it claimed to do. Merge
/// request detail (author, source -> target, pipeline status) will want
/// the same slot.
///
/// Header rows carry no anchor: there's no hunk or line under them to
/// act on, so `diff_apply_hunk` and friends correctly decline on them.
pub fn render_with_header(header: &[(String, DiffStyle)], files: &[FileDiff], collapsed: &HashSet<String>) -> DiffView {
    render_with_threads(header, files, collapsed, &[])
}

/// `render_with_header`, with review threads drawn in under the diff
/// lines they hang on.
///
/// Inline rather than in a pane of their own, because a review comment
/// is about *this line* and reading it anywhere else means holding the
/// line in your head while you look somewhere else for what was said
/// about it. Every row a thread produces carries its id, so replying or
/// resolving works with the cursor anywhere in it.
pub fn render_with_threads(
    header: &[(String, DiffStyle)],
    files: &[FileDiff],
    collapsed: &HashSet<String>,
    threads: &[ThreadAnnotation],
) -> DiffView {
    let mut b = Builder::new();
    for (text, style) in header {
        b.push(text, Some(DiffViewLine { style: *style, content_from: 0, anchor: None, thread: None }));
    }
    if files.is_empty() {
        b.push("    (no changes)", Some(DiffViewLine { style: DiffStyle::Meta, content_from: 0, anchor: None, thread: None }));
        return b.finish();
    }


    let width = gutter_width(files);
    for (file_index, file) in files.iter().enumerate() {
        let (adds, dels) = file
            .hunks
            .iter()
            .flat_map(|h| h.lines.iter())
            .fold((0usize, 0usize), |(a, d), l| match l.kind {
                LineKind::Added => (a + 1, d),
                LineKind::Removed => (a, d + 1),
                _ => (a, d),
            });
        let path = if file.status == FileStatus::Renamed && file.old_path != file.new_path {
            format!("{} -> {}", file.old_path, file.new_path)
        } else {
            file.display_path().to_string()
        };
        let header = format!("{}  {path}   +{adds} -{dels}", status_marker(file.status));
        let anchor = DiffAnchor { file: file_index, hunk: None, old_line: None, new_line: None };
        b.push(&header, Some(DiffViewLine { style: DiffStyle::FileHeader, content_from: 0, anchor: Some(anchor), thread: None }));

        if collapsed.contains(file.display_path()) {
            continue;
        }
        if file.is_binary {
            b.push("    (binary file)", Some(DiffViewLine { style: DiffStyle::Meta, content_from: 0, anchor: Some(anchor), thread: None }));
            continue;
        }

        for (hunk_index, hunk) in file.hunks.iter().enumerate() {
            let hunk_anchor = DiffAnchor { file: file_index, hunk: Some(hunk_index), old_line: None, new_line: None };
            b.push(&hunk.header, Some(DiffViewLine { style: DiffStyle::HunkHeader, content_from: 0, anchor: Some(hunk_anchor), thread: None }));

            for line in &hunk.lines {
                let style = match line.kind {
                    LineKind::Context => DiffStyle::Context,
                    LineKind::Added => DiffStyle::Added,
                    LineKind::Removed => DiffStyle::Removed,
                    LineKind::NoNewline => DiffStyle::Meta,
                };
                let (row, content_from) = content_row(width, line.old_line, line.new_line, &line.raw());
                let anchor = DiffAnchor {
                    file: file_index,
                    hunk: Some(hunk_index),
                    old_line: line.old_line,
                    new_line: line.new_line,
                };
                b.push(&row, Some(DiffViewLine { style, content_from, anchor: Some(anchor), thread: None }));

                let path = file.display_path();
                for thread in threads.iter().filter(|t| t.path == path && t.anchors_to(line.old_line, line.new_line)) {
                    push_thread(&mut b, width, thread, anchor);
                }
            }
        }
    }
    b.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn thread(id: &str, new_line: Option<usize>, old_line: Option<usize>, resolved: bool) -> ThreadAnnotation {
        ThreadAnnotation {
            id: id.to_string(),
            path: "foo.txt".to_string(),
            old_line,
            new_line,
            resolved,
            notes: vec![("Alice".to_string(), "This looks wrong.".to_string()), ("Bob".to_string(), "Fixed.".to_string())],
        }
    }

    fn render_with(threads: &[ThreadAnnotation]) -> DiffView {
        render_with_threads(&[], &fenix_diff::parse(SIMPLE), &HashSet::new(), threads)
    }

    #[test]
    fn a_thread_is_drawn_under_the_line_it_hangs_on() {
        // `+two` is new line 2 in SIMPLE.
        let view = render_with(&[thread("t1", Some(2), None, false)]);
        let rows: Vec<&str> = view.text.lines().collect();
        let code = rows.iter().position(|r| r.ends_with("+two")).expect("the commented line");
        assert!(rows[code + 1].contains("Alice:"), "the thread follows its line:\n{}", view.text);
        assert!(rows[code + 2].contains("This looks wrong."));
        assert!(rows[code + 3].contains("Bob:"), "replies in order:\n{}", view.text);
    }

    #[test]
    fn every_row_of_a_thread_carries_its_id_so_reply_works_from_anywhere_in_it() {
        let view = render_with(&[thread("t1", Some(2), None, false)]);
        let ids: Vec<Option<String>> =
            view.lines.iter().flatten().filter(|l| l.thread.is_some()).map(|l| l.thread.clone()).collect();
        assert!(ids.len() >= 5, "header, body, reply header, reply body, key hint");
        assert!(ids.iter().all(|id| id.as_deref() == Some("t1")));
    }

    #[test]
    fn diff_rows_carry_no_thread_which_is_how_a_comment_row_is_told_apart() {
        let view = render_with(&[thread("t1", Some(2), None, false)]);
        let code_rows = view.lines.iter().flatten().filter(|l| {
            matches!(l.style, DiffStyle::Added | DiffStyle::Removed | DiffStyle::Context | DiffStyle::HunkHeader)
        });
        assert!(code_rows.into_iter().all(|l| l.thread.is_none()));
    }

    #[test]
    fn a_resolved_thread_says_so_once_rather_than_on_every_reply() {
        let view = render_with(&[thread("t1", Some(2), None, true)]);
        assert_eq!(view.text.matches("(resolved)").count(), 1, "got:\n{}", view.text);
        assert!(view.text.contains("Alice (resolved):"), "got:\n{}", view.text);
    }

    #[test]
    fn a_thread_on_a_removed_line_hangs_on_the_old_side() {
        // `-second` is old line 2.
        let view = render_with(&[thread("t1", None, Some(2), false)]);
        let rows: Vec<&str> = view.text.lines().collect();
        let code = rows.iter().position(|r| r.ends_with("-second")).expect("the removed line");
        assert!(rows[code + 1].contains("Alice:"), "got:\n{}", view.text);
    }

    #[test]
    fn a_thread_on_a_line_this_diff_does_not_contain_is_dropped_not_misplaced() {
        // The comment predates a force-push, or names a collapsed file.
        let mut elsewhere = thread("t1", Some(999), None, false);
        elsewhere.path = "other.txt".to_string();
        let view = render_with(&[thread("t2", Some(999), None, false), elsewhere]);
        assert!(!view.text.contains("Alice:"), "got:\n{}", view.text);
        // And the diff itself is untouched.
        assert_eq!(view.text.lines().count(), render_simple().text.lines().count());
    }

    #[test]
    fn threads_and_metadata_stay_the_same_length_as_the_text() {
        let view = render_with(&[thread("t1", Some(2), None, false), thread("t2", None, Some(2), true)]);
        assert_eq!(view.text.lines().count(), view.lines.len());
    }

    #[test]
    fn a_thread_row_is_anchored_to_the_same_file_and_hunk_as_its_line() {
        // So "comment on this line" from inside a thread still knows
        // which file it's in.
        let view = render_with(&[thread("t1", Some(2), None, false)]);
        let row = view.lines.iter().flatten().find(|l| l.thread.is_some()).unwrap();
        let anchor = row.anchor.expect("thread rows keep an anchor");
        assert_eq!(anchor.file, 0);
        assert_eq!(anchor.hunk, Some(0));
    }

    const SIMPLE: &str = "diff --git a/foo.txt b/foo.txt\nindex 83db48f..bf269f4 100644\n--- a/foo.txt\n+++ b/foo.txt\n@@ -1,3 +1,4 @@\n first\n-second\n+two\n+extra\n third\n";

    fn render_simple() -> DiffView {
        render(&fenix_diff::parse(SIMPLE), &HashSet::new())
    }

    #[test]
    fn text_and_lines_stay_the_same_length() {
        let view = render_simple();
        assert_eq!(view.text.lines().count(), view.lines.len());
    }

    #[test]
    fn a_file_header_row_names_the_path_and_counts_both_sides() {
        let view = render_simple();
        let first = view.text.lines().next().unwrap();
        assert!(first.starts_with("M  foo.txt"), "got {first:?}");
        assert!(first.contains("+2 -1"), "got {first:?}");
        assert_eq!(view.lines[0].as_ref().unwrap().style, DiffStyle::FileHeader);
    }

    #[test]
    fn the_hunk_header_row_is_the_diffs_own_header_verbatim() {
        let view = render_simple();
        assert_eq!(view.text.lines().nth(1).unwrap(), "@@ -1,3 +1,4 @@");
        assert_eq!(view.lines[1].as_ref().unwrap().style, DiffStyle::HunkHeader);
    }

    #[test]
    fn content_rows_carry_both_line_number_columns_and_the_verbatim_patch_line() {
        let view = render_simple();
        let rows: Vec<&str> = view.text.lines().skip(2).collect();
        // Two 3-wide number columns, then the verbatim patch line -- so
        // the +/-/space marker always lands in the same column (8),
        // whichever side the line belongs to.
        assert_eq!(rows[0], "  1   1  first");
        assert_eq!(rows[1], "  2     -second");
        assert_eq!(rows[2], "      2 +two");
        assert_eq!(rows[3], "      3 +extra");
        assert_eq!(rows[4], "  3   4  third");
        assert!(rows.iter().all(|r| r.chars().nth(8).is_some_and(|c| matches!(c, ' ' | '+' | '-'))));
    }

    #[test]
    fn every_content_row_anchors_back_to_its_file_hunk_and_line_numbers() {
        let view = render_simple();
        let removed = view.lines[3].as_ref().unwrap();
        assert_eq!(removed.style, DiffStyle::Removed);
        assert_eq!(removed.anchor, Some(DiffAnchor { file: 0, hunk: Some(0), old_line: Some(2), new_line: None }));
        let added = view.lines[4].as_ref().unwrap();
        assert_eq!(added.style, DiffStyle::Added);
        assert_eq!(added.anchor, Some(DiffAnchor { file: 0, hunk: Some(0), old_line: None, new_line: Some(2) }));
    }

    #[test]
    fn content_from_lands_exactly_where_the_patch_line_starts() {
        let view = render_simple();
        let row = view.text.lines().nth(2).unwrap();
        let meta = view.lines[2].as_ref().unwrap();
        // Everything before `content_from` is gutter; everything from it
        // on is the verbatim patch line, marker included.
        assert_eq!(&row[meta.content_from..], " first");
        assert!(row[..meta.content_from].chars().all(|c| c.is_ascii_digit() || c == ' '), "the gutter holds only line numbers");
    }

    #[test]
    fn a_hunk_header_row_anchors_to_its_hunk_with_no_line_numbers() {
        let view = render_simple();
        assert_eq!(view.lines[1].as_ref().unwrap().anchor, Some(DiffAnchor { file: 0, hunk: Some(0), old_line: None, new_line: None }));
    }

    #[test]
    fn a_file_header_row_anchors_to_the_file_with_no_hunk() {
        let view = render_simple();
        assert_eq!(view.lines[0].as_ref().unwrap().anchor, Some(DiffAnchor { file: 0, hunk: None, old_line: None, new_line: None }));
    }

    #[test]
    fn a_collapsed_file_renders_only_its_header_row() {
        let files = fenix_diff::parse(SIMPLE);
        let collapsed: HashSet<String> = ["foo.txt".to_string()].into_iter().collect();
        let view = render(&files, &collapsed);
        assert_eq!(view.lines.len(), 1);
        assert_eq!(view.lines[0].as_ref().unwrap().style, DiffStyle::FileHeader);
    }

    #[test]
    fn several_files_each_get_their_own_header_and_anchor_index() {
        let text = "diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1 +1 @@\n-x\n+y\ndiff --git a/b b/b\n--- a/b\n+++ b/b\n@@ -1 +1 @@\n-m\n+n\n";
        let view = render(&fenix_diff::parse(text), &HashSet::new());
        let file_rows: Vec<usize> = view
            .lines
            .iter()
            .enumerate()
            .filter(|(_, l)| l.as_ref().is_some_and(|l| l.style == DiffStyle::FileHeader))
            .map(|(i, _)| i)
            .collect();
        assert_eq!(file_rows.len(), 2);
        assert_eq!(view.lines[file_rows[1]].as_ref().unwrap().anchor.unwrap().file, 1);
    }

    #[test]
    fn a_binary_file_shows_a_placeholder_instead_of_hunks() {
        let text = "diff --git a/logo.png b/logo.png\nindex 1234567..89abcde 100644\nBinary files a/logo.png and b/logo.png differ\n";
        let view = render(&fenix_diff::parse(text), &HashSet::new());
        assert!(view.text.contains("(binary file)"));
        assert_eq!(view.lines[1].as_ref().unwrap().style, DiffStyle::Meta);
    }

    #[test]
    fn a_rename_header_shows_both_paths() {
        let text = "diff --git a/old.txt b/new.txt\nsimilarity index 100%\nrename from old.txt\nrename to new.txt\n";
        let view = render(&fenix_diff::parse(text), &HashSet::new());
        assert!(view.text.starts_with("R  old.txt -> new.txt"), "got {:?}", view.text.lines().next());
    }

    #[test]
    fn a_no_newline_marker_renders_as_a_meta_row() {
        let text = "diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1 +1 @@\n-old\n\\ No newline at end of file\n+new\n";
        let view = render(&fenix_diff::parse(text), &HashSet::new());
        let meta = view.lines.iter().flatten().find(|l| l.style == DiffStyle::Meta).unwrap();
        assert_eq!(meta.anchor.unwrap().old_line, None);
        assert!(view.text.contains("\\ No newline at end of file"));
    }

    #[test]
    fn a_header_is_rendered_above_the_diff_and_carries_no_anchor() {
        let header = vec![
            ("abc1234  Jane Doe  2026-09-03 14:22".to_string(), DiffStyle::Title),
            ("Fix the thing".to_string(), DiffStyle::Meta),
        ];
        let view = render_with_header(&header, &fenix_diff::parse(SIMPLE), &HashSet::new());
        let lines: Vec<&str> = view.text.lines().collect();
        assert_eq!(lines[0], "abc1234  Jane Doe  2026-09-03 14:22");
        assert_eq!(lines[1], "Fix the thing");
        assert!(lines[2].starts_with("M  foo.txt"), "the diff still follows: {:?}", lines[2]);
        // Nothing to stage or open on a header row.
        assert!(view.lines[0].as_ref().unwrap().anchor.is_none());
        assert_eq!(view.text.lines().count(), view.lines.len());
    }

    #[test]
    fn a_header_still_shows_when_the_commit_changed_nothing() {
        let header = vec![("abc1234  an empty commit".to_string(), DiffStyle::Title)];
        let view = render_with_header(&header, &[], &HashSet::new());
        assert!(view.text.starts_with("abc1234"), "got: {:?}", view.text);
        assert!(view.text.contains("(no changes)"));
    }

    #[test]
    fn an_empty_diff_says_so_rather_than_rendering_nothing() {
        let view = render(&[], &HashSet::new());
        assert!(view.text.contains("(no changes)"));
        assert_eq!(view.lines.len(), 1);
    }

    #[test]
    fn the_gutter_widens_for_four_digit_line_numbers_without_reflowing_per_file() {
        let text = "diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1,1 +1,1 @@\n-x\n+y\ndiff --git a/b b/b\n--- a/b\n+++ b/b\n@@ -1200,1 +1200,1 @@\n-m\n+n\n";
        let view = render(&fenix_diff::parse(text), &HashSet::new());
        // Both files share one gutter width, driven by the widest number
        // anywhere -- so scrolling between them doesn't shift the text.
        let rows: Vec<&str> = view.text.lines().filter(|l| l.contains("-x") || l.contains("-m")).collect();
        assert_eq!(rows[0], "   1      -x");
        assert_eq!(rows[1], "1200      -m");
    }

    #[test]
    fn a_crlf_line_keeps_its_carriage_return_in_the_rendered_row() {
        let text = "diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1 +1 @@\n-old\r\n+new\r\n";
        let view = render(&fenix_diff::parse(text), &HashSet::new());
        assert!(view.text.contains("-old\r"), "the rendered row must not quietly drop the CR");
    }
}
