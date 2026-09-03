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

/// Renders `files` into a diff buffer. `collapsed` holds the display
/// paths of files folded down to just their header row -- the same
/// "caller owns the expansion set, renderer just reads it" split
/// `git_panel::render_unstaged` already uses for its directory tree, so
/// folding survives a refresh without this module holding state.
pub fn render(files: &[FileDiff], collapsed: &HashSet<String>) -> DiffView {
    let mut b = Builder::new();
    if files.is_empty() {
        b.push("    (no changes)", Some(DiffViewLine { style: DiffStyle::Meta, content_from: 0, anchor: None }));
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
        b.push(&header, Some(DiffViewLine { style: DiffStyle::FileHeader, content_from: 0, anchor: Some(anchor) }));

        if collapsed.contains(file.display_path()) {
            continue;
        }
        if file.is_binary {
            b.push("    (binary file)", Some(DiffViewLine { style: DiffStyle::Meta, content_from: 0, anchor: Some(anchor) }));
            continue;
        }

        for (hunk_index, hunk) in file.hunks.iter().enumerate() {
            let hunk_anchor = DiffAnchor { file: file_index, hunk: Some(hunk_index), old_line: None, new_line: None };
            b.push(&hunk.header, Some(DiffViewLine { style: DiffStyle::HunkHeader, content_from: 0, anchor: Some(hunk_anchor) }));

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
                b.push(&row, Some(DiffViewLine { style, content_from, anchor: Some(anchor) }));
            }
        }
    }
    b.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

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
