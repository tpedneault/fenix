//! Wrapping, two kinds.
//!
//! `break_line` is a code pane's soft wrap: where a line wider than its
//! pane breaks into rows, at the pane's real width, for the row map
//! (`app/rows.rs`) to lay out -- the text isn't changed, only drawn on
//! more than one row. A row ends after the last whitespace that fits, in
//! the middle of a word only when the word alone is wider; the rows after
//! the first keep the line's indent (`continuation_indent`), at most half
//! the pane. Columns are the grid's, a char each (tabs expanded and
//! decorations laid out first), as the caret counts them.
//!
//! `wrap_text` is plain word-wrapping for generated panel text -- the
//! Docker/Git/Jira panels' own `label: value` detail rows and Jira's
//! issue description/comment prose, specifically (see each panel
//! module's own call sites), *not* their list rows (Containers/Images/
//! Files/Branches/Commits/...) or Git's diff pane, which stay single-
//! line/unwrapped on purpose: a wrapped list row would misalign against
//! its neighbors the same way `docker_panel::render_containers`' own doc
//! comment already rejected letting a row's content get clipped instead
//! of trimmed, and wrapping a unified diff would break the one-line-per-
//! change convention every diff viewer (this app's own included) relies
//! on to stay readable. It has no notion of the pane it'll be rendered
//! in: the text it returns is the panel's lines, so `width` is a fixed,
//! generous approximation of a typical split's width rather than the
//! pane's real current column count.

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

/// Where a code pane's line wider than `width` columns breaks: the
/// visual columns of `display` (the line as drawn, tabs expanded and
/// decorations laid out, a column a char) on each row. A row's text
/// takes all but its last column, which is the caret's after the row's
/// last char; it ends after the last word that fits, the blanks after
/// that word hanging at its end (one in the kept column, any more past
/// the pane's edge); a word wider than the row alone is broken where
/// the row ends, never between a char and a mark drawn on it. The rows
/// after the first are `indent` columns narrower (they're drawn that far
/// in), never narrower than one column. An empty line is one empty row.
#[cfg_attr(not(test), allow(dead_code))] // the row walk's, which scrolls
pub fn break_line(display: &str, width: usize, indent: usize) -> Vec<std::ops::Range<usize>> {
    break_line_rows(display, width, indent, usize::MAX).0
}

/// `break_line`'s first `max_rows` rows, reading no more of `display`
/// than they need, and whether they are all its rows.
pub fn break_line_rows(display: &str, width: usize, indent: usize, max_rows: usize) -> (Vec<std::ops::Range<usize>>, bool) {
    let width = width.max(1);
    // A row's text: one column short of the row, for the caret.
    let text_cols = |cols: usize| if cols > 1 { cols - 1 } else { 1 };
    let (first, rest) = (text_cols(width), text_cols(width.saturating_sub(indent).max(1)));
    // As much of the line as `max_rows` rows can hold, and a char more.
    let limit = max_rows.saturating_mul(width).saturating_add(width + 1);
    let chars: Vec<char> = display.chars().take(limit).collect();
    let whole = chars.len() < limit || display.chars().nth(limit).is_none();
    let n = chars.len();
    let blank = |i: usize| chars[i].is_whitespace();
    let mut rows = Vec::new();
    let mut start = 0;
    loop {
        let fits = if rows.is_empty() { first } else { rest };
        if n - start <= fits && (whole || n - start < fits) {
            rows.push(start..n);
            return (rows, whole);
        }
        let edge = start + fits;
        if edge >= n {
            // Past what was read: the rest is beyond the rows wanted.
            rows.push(start..n);
            return (rows, false);
        }
        // After the last word that ends on the row, its blanks with it.
        let soft = (start + 1..=edge).rev().find(|&end| !blank(end - 1) && blank(end));
        let end = match soft {
            Some(mut end) => {
                while end < n && blank(end) {
                    end += 1;
                }
                end
            }
            // Only blanks before a word on the row (an indent): before
            // the word, when the next row has room for it.
            None => match (start + 1..=edge).rev().find(|&end| blank(end - 1) && !blank(end)) {
                Some(end) if (end..n).take_while(|&i| !blank(i)).count() <= rest => end,
                _ => (start + 1..=edge).rev().find(|&end| !joins(chars[end])).unwrap_or(edge),
            },
        };
        rows.push(start..end);
        if end >= n {
            return (rows, whole);
        }
        if rows.len() >= max_rows {
            return (rows, false);
        }
        start = end;
    }
}

/// Whether `c` is drawn on the char before it: a combining mark, a
/// joiner, a variation selector.
fn joins(c: char) -> bool {
    matches!(c as u32, 0x300..=0x36F | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF | 0x200D | 0x20D0..=0x20FF | 0xFE00..=0xFE0F | 0xFE20..=0xFE2F)
}

/// How far a line's continuation rows are drawn in, in a pane `width`
/// columns wide: as far as its own indent (`display`'s leading blanks),
/// but no more than half the pane -- and not at all when the indent
/// alone doesn't fit on the first row, whose break is then inside it.
pub fn continuation_indent(display: &str, width: usize) -> usize {
    let leading = display.chars().take_while(|c| *c == ' ' || *c == '\t').count();
    // A row's last column is the caret's: the indent must leave text room.
    if leading + 1 >= width { 0 } else { leading.min(width / 2) }
}

#[cfg(test)]
// A line of one row is a list of one range.
#[allow(clippy::single_range_in_vec_init)]
mod tests {
    use super::*;

    fn rows(display: &str, width: usize, indent: usize) -> Vec<&str> {
        let chars: Vec<(usize, char)> = display.char_indices().collect();
        let byte = |col: usize| chars.get(col).map_or(display.len(), |&(b, _)| b);
        break_line(display, width, indent).into_iter().map(|r| &display[byte(r.start)..byte(r.end)]).collect()
    }

    #[test]
    fn a_line_breaks_after_the_last_word_that_fits_keeping_a_column_for_the_caret() {
        assert_eq!(rows("the quick brown fox", 10, 0), vec!["the quick ", "brown fox"]);
        assert_eq!(rows("aaa bbb ccc ddd", 7, 0), vec!["aaa ", "bbb ", "ccc ", "ddd"]);
    }

    #[test]
    fn a_word_ending_at_the_text_s_edge_stays_and_the_next_row_never_starts_with_its_blank() {
        assert_eq!(rows("abcd efgh", 5, 0), vec!["abcd ", "efgh"]);
        assert_eq!(rows("abcde fgh", 5, 0), vec!["abcd", "e ", "fgh"]);
    }

    #[test]
    fn the_blanks_at_a_break_hang_at_the_end_of_its_row() {
        assert_eq!(rows("aaa      bbb", 5, 0), vec!["aaa      ", "bbb"], "not `aaa  ` / `    b` / `bb`");
        assert_eq!(rows("ab  cd", 3, 0), vec!["ab  ", "cd"]);
        assert_eq!(rows("ab   ", 3, 0), vec!["ab   "]);
    }

    #[test]
    fn a_word_after_an_indent_goes_whole_to_the_next_row_when_it_fits_there() {
        assert_eq!(rows("        quick brown", 12, 6), vec!["        ", "quick ", "brown"]);
        assert_eq!(rows("        abcdefghijk", 12, 6), vec!["        abc", "defgh", "ijk"], "too long for any row: broken");
    }

    #[test]
    fn a_word_wider_than_the_row_is_broken_where_the_row_ends() {
        assert_eq!(rows("abcdefghijkl", 5, 0), vec!["abcd", "efgh", "ijkl"]);
        assert_eq!(rows("ab cdefghijkl", 5, 0), vec!["ab ", "cdef", "ghij", "kl"]);
    }

    #[test]
    fn a_line_one_short_of_the_pane_and_an_empty_one_are_one_row_and_a_full_one_is_two() {
        assert_eq!(break_line("abcd", 5, 0), [0..4]);
        assert_eq!(break_line("", 5, 0), [0..0]);
        assert_eq!(break_line("abcde", 5, 0), vec![0..4, 4..5], "the last column is the caret's, after the last char");
    }

    #[test]
    fn continuation_rows_are_narrower_by_the_indent() {
        let line = "    let total = first + second;";
        let indent = continuation_indent(line, 16);
        assert_eq!(indent, 4);
        // The first row's indent isn't a place to break.
        assert_eq!(rows(line, 16, indent), vec!["    let total = ", "first + ", "second;"]);
        // Never more than half the pane, and none when the indent leaves
        // the first row no text.
        assert_eq!(continuation_indent("            x", 20), 10);
        assert_eq!(continuation_indent("            x", 10), 0);
        assert_eq!(rows("        ab cd", 6, continuation_indent("        ab cd", 6)), vec!["     ", "   ab ", "cd"]);
    }

    #[test]
    fn a_pane_one_column_wide_is_a_char_a_row() {
        assert_eq!(rows("a b", 1, 0), vec!["a ", "b"]);
        assert_eq!(rows("ab", 1, 3), vec!["a", "b"], "an indent wider than the pane still leaves a column");
        assert_eq!(break_line("ab", 0, 0), vec![0..1, 1..2]);
    }

    #[test]
    fn wide_and_combining_chars_are_a_column_each_as_the_caret_counts_them() {
        assert_eq!(rows("日本語 テキスト", 4, 0), vec!["日本語 ", "テキス", "ト"]);
        assert_eq!(rows("e\u{301}e\u{301}e\u{301}", 5, 0), vec!["e\u{301}e\u{301}", "e\u{301}"]);
        // Not between a char and its mark.
        assert_eq!(rows("e\u{301}e\u{301}e\u{301}", 4, 0), vec!["e\u{301}", "e\u{301}", "e\u{301}"]);
    }

    #[test]
    fn only_the_rows_wanted_are_found_and_read() {
        let long = "word ".repeat(200_000);
        let (rows, whole) = break_line_rows(&long, 10, 0, 3);
        assert_eq!(rows, vec![0..10, 10..20, 20..30]);
        assert!(!whole);
        let (rows, whole) = break_line_rows("abc def", 10, 0, 3);
        assert_eq!(rows, [0..7]);
        assert!(whole);
        // A row of blanks a million long: no search over every end.
        let blanks = format!("x{}y", " ".repeat(1_000_000));
        let started = std::time::Instant::now();
        let (rows, _) = break_line_rows(&blanks, 200, 0, 50);
        assert_eq!(rows[0].start, 0);
        assert!(started.elapsed() < std::time::Duration::from_secs(2), "{:?}", started.elapsed());
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
