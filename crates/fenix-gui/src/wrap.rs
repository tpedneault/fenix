//! Word-wrapping, two ways.
//!
//! `soft_wrap` is the editor's: where a buffer line breaks into screen
//! rows when a pane soft-wraps (the agent's prompt, a compose pane,
//! `editor.soft_wrap`, `SPC t w`). It only says where the rows start
//! and end -- the text is never changed -- and the pane's display rows
//! (`App::pane_display_rows`) are built from it.
//!
//! `wrap_text` is for generated panel text -- the Docker/Git/Jira
//! panels' `label: value` detail rows and Jira's issue description and
//! comment prose -- written into the panel's buffer as separate lines,
//! at a fixed width rather than the pane's. Their list rows and Git's
//! diff pane stay one line each on purpose: a wrapped list row would
//! misalign against its neighbours, and a wrapped unified diff would
//! break the one-line-per-change convention diff viewers rely on.

use std::ops::Range;

use crate::tabstops::{self, TabStops};

/// One screen row of a soft-wrapped pane: chars `start..end` of buffer
/// line `line`. `last` is the line's last row, which also takes the
/// position just past its end (where Insert's caret goes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Row {
    pub line: usize,
    pub start: usize,
    pub end: usize,
    pub last: bool,
}

impl Row {
    /// The row a whole, unwrapped line `line` of `len` chars takes.
    pub fn whole(line: usize, len: usize) -> Row {
        Row { line, start: 0, end: len, last: true }
    }

    /// Whether char column `col` of its line is drawn on this row.
    pub fn holds(&self, col: usize) -> bool {
        col >= self.start && (col < self.end || self.last)
    }

    /// The part of chars `s..e` of the line on this row.
    pub fn clip(&self, s: usize, e: usize) -> Option<(usize, usize)> {
        let (a, b) = (s.max(self.start), if self.last { e } else { e.min(self.end) });
        (a < b).then_some((a, b))
    }
}

/// The index in `rows` (top to bottom, lines in order) of the row char
/// column `col` of line `line` is on; past the line's last row, that
/// row. `None` when the line has no row (it's folded away).
pub fn row_holding(rows: &[Row], line: usize, col: usize) -> Option<usize> {
    let first = rows.partition_point(|r| r.line < line);
    let mut found = None;
    for (i, row) in rows.iter().enumerate().skip(first).take_while(|(_, r)| r.line == line) {
        found = Some(i);
        if row.holds(col) {
            break;
        }
    }
    found
}

/// How many cells char `c` takes: two for a wide (CJK) one, else one.
fn cell_width(c: char) -> usize {
    unicode_width::UnicodeWidthChar::width(c).unwrap_or(1).max(1)
}

/// Where `line` breaks into rows of at most `width` cells: after the
/// last whitespace that fits, or -- a word longer than the row -- at
/// the width itself. Tabs take the cells `tab_stops` expands them to,
/// counted from the start of the line, and a wide char two. Whitespace
/// that doesn't fit wraps like any other char, so no row ever runs past
/// `width`. Each range is chars of `line`; together they cover all of
/// it, and an empty line is one empty row.
pub fn soft_wrap(line: &str, width: usize, tab_stops: &TabStops) -> Vec<Range<usize>> {
    let chars: Vec<char> = line.chars().collect();
    // Too short to need wrapping even if every char were wide.
    if width == 0 || (chars.len() <= width / 2 && !line.contains('\t')) {
        return std::iter::once(0..chars.len()).collect();
    }
    let col_map = if line.contains('\t') { Some(tabstops::expand_line(line, tab_stops).1) } else { None };
    let widths: Vec<usize> = chars
        .iter()
        .enumerate()
        .map(|(i, &c)| match (&col_map, c) {
            (Some(map), '\t') => map[i + 1] - map[i],
            _ => cell_width(c),
        })
        .collect();
    let mut rows = Vec::new();
    let (mut start, mut used) = (0usize, 0usize);
    // Just after the last whitespace on the row being built.
    let mut brk: Option<usize> = None;
    for i in 0..chars.len() {
        while used + widths[i] > width && i > start {
            let cut = brk.filter(|&b| b > start && b <= i).unwrap_or(i);
            rows.push(start..cut);
            start = cut;
            used = widths[cut..i].iter().sum();
            brk = None;
        }
        used += widths[i];
        if chars[i].is_whitespace() {
            brk = Some(i + 1);
        }
    }
    rows.push(start..chars.len());
    rows
}

/// The wrap width panel builders default to, in characters -- a
/// comfortable prose-reading width (`rustfmt`'s own default `max_width`
/// is the same number) that most real split widths in this app
/// comfortably exceed, so most short `label: value` rows never wrap at
/// all; only genuinely long values/paragraphs do.
pub const DEFAULT_WRAP_WIDTH: usize = 100;

/// Wraps `text` to `width` columns: primarily at whitespace runs
/// (`split_whitespace`, so wrapped text has its internal whitespace
/// collapsed to single spaces, an unavoidable side effect of splitting
/// into words at all -- exactly what word-wrapped prose already
/// implies), but a single token that's *itself* longer than `width` --
/// a Docker bind-mount path, a long image digest, anything with no
/// spaces at all -- is hard-broken into `width`-sized chunks rather
/// than left whole and still overflowing the pane, which would defeat
/// the entire point of wrapping for exactly the case it matters most
/// for. Text that's already no wider than `width` needs none of this
/// and is returned completely untouched, spacing included -- never
/// zero lines either, so a blank paragraph-break line still round-trips
/// as its own blank line rather than disappearing.
pub fn wrap_text(text: &str, width: usize) -> Vec<String> {
    if width == 0 || text.chars().count() <= width {
        return vec![text.to_string()];
    }
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut current_len = 0usize;
    for word in text.split_whitespace() {
        let word_len = word.chars().count();
        if word_len <= width {
            place(&mut lines, &mut current, &mut current_len, word, word_len, width);
            continue;
        }
        // `word` alone doesn't fit even on an empty line: flush
        // whatever's building first, then hard-break it into
        // `width`-sized chunks, each pushed directly as its own line --
        // including the final, possibly-shorter remainder, which stays
        // on its own line rather than being handed to `place` for the
        // next real word to pack onto. That keeps a hard break visually
        // unambiguous (the fragment never silently merges with an
        // unrelated following word) at the cost of a little wasted
        // width on that last, short line -- a fine trade for text this
        // is never expecting to need hard-breaking often to begin with.
        if !current.is_empty() {
            lines.push(std::mem::take(&mut current));
            current_len = 0;
        }
        let mut remaining = word;
        while !remaining.is_empty() {
            let split_at = remaining.char_indices().nth(width).map(|(i, _)| i).unwrap_or(remaining.len());
            lines.push(remaining[..split_at].to_string());
            remaining = &remaining[split_at..];
        }
    }
    if !current.is_empty() || lines.is_empty() {
        lines.push(current);
    }
    lines
}

/// Appends `word` (already known to fit within `width` on its own) to
/// `current`, starting a fresh line first if it wouldn't fit alongside
/// what's already there.
fn place(lines: &mut Vec<String>, current: &mut String, current_len: &mut usize, word: &str, word_len: usize, width: usize) {
    if current.is_empty() {
        current.push_str(word);
        *current_len = word_len;
    } else if *current_len + 1 + word_len <= width {
        current.push(' ');
        current.push_str(word);
        *current_len += 1 + word_len;
    } else {
        lines.push(std::mem::take(current));
        current.push_str(word);
        *current_len = word_len;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TABS: TabStops = TabStops::Fixed(4);

    fn rows_of(line: &str, width: usize) -> Vec<String> {
        let chars: Vec<char> = line.chars().collect();
        soft_wrap(line, width, &TABS).into_iter().map(|r| chars[r].iter().collect()).collect()
    }

    #[test]
    fn a_line_that_fits_is_one_row() {
        assert_eq!(rows_of("hello", 10), ["hello"]);
        assert_eq!(rows_of("0123456789", 10), ["0123456789"]);
    }

    #[test]
    fn an_empty_line_is_one_empty_row() {
        assert_eq!(rows_of("", 10), [""]);
    }

    #[test]
    fn a_line_breaks_after_the_last_whitespace_that_fits() {
        assert_eq!(rows_of("the quick brown fox jumps", 11), ["the quick ", "brown fox ", "jumps"]);
    }

    #[test]
    fn the_rows_cover_the_whole_line_with_nothing_lost() {
        let line = "Explain the build pipeline, then   list its   steps and the files each reads.";
        for width in 1..30 {
            let rows = soft_wrap(line, width, &TABS);
            assert_eq!(rows.first().unwrap().start, 0);
            assert_eq!(rows.last().unwrap().end, line.chars().count());
            assert!(rows.windows(2).all(|w| w[0].end == w[1].start), "{rows:?}");
            assert!(rows.iter().all(|r| r.len() <= width), "width {width}: {rows:?}");
        }
    }

    #[test]
    fn a_word_longer_than_the_row_is_hard_broken() {
        assert_eq!(rows_of(&"a".repeat(25), 10), ["a".repeat(10), "a".repeat(10), "a".repeat(5)]);
        assert_eq!(rows_of(&format!("short {}", "b".repeat(12)), 10), ["short ".to_string(), "b".repeat(10), "bb".to_string()]);
    }

    #[test]
    fn trailing_spaces_stay_on_their_row_while_they_fit_and_wrap_once_they_dont() {
        assert_eq!(rows_of("word   ", 10), ["word   "]);
        assert_eq!(rows_of("word      next", 8), ["word    ", "  next"]);
        assert_eq!(rows_of("abcdefghij ", 10), ["abcdefghij", " "]);
    }

    #[test]
    fn a_wide_char_takes_two_cells_and_moves_down_whole() {
        // Five two-cell chars in a row of nine: four fit.
        assert_eq!(rows_of("日本語のテキスト", 9), ["日本語の", "テキスト"]);
        assert_eq!(rows_of("ab日", 3), ["ab", "日"]);
        // Wider than the row itself: it still gets a row of its own.
        assert_eq!(rows_of("日日", 1), ["日", "日"]);
    }

    #[test]
    fn a_tab_takes_the_cells_it_expands_to() {
        // `\t` at column 2 runs to 4; "ab\tcd" is six cells.
        assert_eq!(rows_of("ab\tcd", 6), ["ab\tcd"]);
        // A tab is whitespace: the row may break after it.
        assert_eq!(rows_of("ab\tcd ef", 6), ["ab\t", "cd ef"]);
        assert_eq!(rows_of("\t\t\tx", 8), ["\t\t", "\tx"]);
    }

    #[test]
    fn the_row_a_column_is_on_is_found_and_a_folded_line_has_none() {
        let rows = [Row::whole(0, 3), Row { line: 1, start: 0, end: 4, last: false }, Row { line: 1, start: 4, end: 7, last: true }, Row::whole(5, 2)];
        assert_eq!(row_holding(&rows, 0, 2), Some(0));
        assert_eq!(row_holding(&rows, 1, 3), Some(1));
        assert_eq!(row_holding(&rows, 1, 4), Some(2));
        assert_eq!(row_holding(&rows, 1, 7), Some(2), "the end of the line, on its last row");
        assert_eq!(row_holding(&rows, 3, 0), None);
        assert_eq!(row_holding(&rows, 5, 9), Some(3));
    }

    #[test]
    fn a_row_holds_its_own_columns_and_the_last_one_the_end_too() {
        let first = Row { line: 0, start: 0, end: 6, last: false };
        let last = Row { line: 0, start: 6, end: 10, last: true };
        assert!(first.holds(5) && !first.holds(6) && last.holds(6) && last.holds(10) && last.holds(11));
        assert_eq!(first.clip(4, 8), Some((4, 6)));
        assert_eq!(last.clip(4, 12), Some((6, 12)));
        assert_eq!(first.clip(6, 9), None);
    }

    #[test]
    fn text_no_wider_than_width_is_returned_unchanged_as_one_line() {
        assert_eq!(wrap_text("short", 10), vec!["short".to_string()]);
    }

    #[test]
    fn empty_text_yields_one_empty_line_not_zero_lines() {
        assert_eq!(wrap_text("", 10), vec![String::new()]);
    }

    #[test]
    fn wraps_at_the_last_word_boundary_that_fits() {
        assert_eq!(wrap_text("the quick brown fox jumps", 11), vec!["the quick".to_string(), "brown fox".to_string(), "jumps".to_string()]);
    }

    #[test]
    fn a_single_token_longer_than_width_is_hard_broken_into_width_sized_chunks() {
        let long_token = "a".repeat(25);
        let result = wrap_text(&long_token, 10);
        assert_eq!(result, vec!["a".repeat(10), "a".repeat(10), "a".repeat(5)]);
    }

    #[test]
    fn a_long_token_flushes_the_line_being_built_before_starting_its_own_chunks() {
        let long_token = "b".repeat(25);
        let result = wrap_text(&format!("short {long_token}"), 10);
        assert_eq!(result, vec!["short".to_string(), "b".repeat(10), "b".repeat(10), "b".repeat(5)]);
    }

    #[test]
    fn wrapping_continues_normally_with_ordinary_words_after_a_hard_broken_token() {
        let long_token = "c".repeat(25);
        let result = wrap_text(&format!("{long_token} then normal words"), 10);
        assert_eq!(result, vec!["c".repeat(10), "c".repeat(10), "c".repeat(5), "then".to_string(), "normal".to_string(), "words".to_string()]);
    }

    #[test]
    fn text_already_within_width_is_preserved_byte_for_byte_including_its_own_spacing() {
        // No wrapping needed at all, so nothing about the text is
        // touched -- unlike the case below, where wrapping's own word-
        // splitting can't help but normalize spacing along the way.
        assert_eq!(wrap_text("a    b", 10), vec!["a    b".to_string()]);
    }

    #[test]
    fn wrapping_collapses_runs_of_internal_whitespace_to_a_single_space() {
        // Only text long enough to actually need wrapping goes through
        // `split_whitespace`, which is where the collapsing comes from --
        // an unavoidable side effect of splitting into words at all, not
        // something this function sets out to do to short text (see the
        // test above).
        assert_eq!(wrap_text("first    second    third    fourth", 12), vec!["first second".to_string(), "third fourth".to_string()]);
    }

    #[test]
    fn width_zero_returns_the_whole_text_as_one_line_rather_than_looping_forever() {
        assert_eq!(wrap_text("some text", 0), vec!["some text".to_string()]);
    }

    #[test]
    fn text_that_is_only_whitespace_and_within_width_is_preserved_verbatim() {
        // Caught by the short-circuit before word-splitting even runs --
        // a blank description line must stay a blank line, not vanish.
        assert_eq!(wrap_text("   ", 10), vec!["   ".to_string()]);
    }

    #[test]
    fn exactly_at_width_does_not_wrap() {
        assert_eq!(wrap_text("0123456789", 10), vec!["0123456789".to_string()]);
    }

    #[test]
    fn one_character_over_width_wraps() {
        assert_eq!(wrap_text("01234567 9x", 10), vec!["01234567".to_string(), "9x".to_string()]);
    }

    #[test]
    fn a_long_path_with_no_spaces_still_wraps_instead_of_overflowing() {
        // The actual motivating case: a Docker bind-mount path or a long
        // image digest has no whitespace at all for word-wrap to break
        // on -- it must still wrap, or this feature does nothing for
        // exactly the values it exists to handle.
        let path = format!("/var/lib/docker/volumes/{}/_data", "x".repeat(90));
        let result = wrap_text(&path, 40);
        assert!(result.len() > 1, "expected the long path to wrap onto multiple lines, got {result:?}");
        assert!(result.iter().all(|l| l.chars().count() <= 40));
        assert_eq!(result.concat(), path, "no characters should be lost across the wrap");
    }

    #[test]
    fn a_non_ascii_token_is_hard_broken_at_a_char_boundary_not_a_byte_boundary() {
        // café repeated -- multi-byte UTF-8 characters throughout, so a
        // byte-index split would panic or corrupt the string if this
        // used byte length/indices instead of char ones.
        let long_token = "café".repeat(10); // 40 chars, well over width
        let result = wrap_text(&long_token, 6);
        assert_eq!(result.concat(), long_token);
        assert!(result.iter().all(|l| l.chars().count() <= 6));
    }
}
