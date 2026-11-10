//! Where more cursors go when standard keys add them from the keyboard:
//! the word to select under the cursor (`Ctrl+D`'s first press), the next
//! occurrence of a selection (each press after), every occurrence of it
//! (`Ctrl+Shift+L`), and a cursor on the line above or below
//! (`Ctrl+Alt+Up/Down`). Positions are char indices; a range's end is
//! exclusive, as standard keys' selections are.

use std::ops::Range;

use fenix_core::Buffer;

use crate::charclass::{classify, CharClass};

/// A word's character: a letter, a digit, `iskeyword`'s, or a combining
/// mark on one (the accent of a decomposed `é`), which is part of it.
fn word_char(c: char, iskeyword_extra: &[char]) -> bool {
    classify(c, iskeyword_extra) == CharClass::Word || combining(c)
}

/// The combining marks' main blocks.
fn combining(c: char) -> bool {
    matches!(c as u32, 0x0300..=0x036F | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF | 0x20D0..=0x20FF | 0xFE20..=0xFE2F)
}

/// The word `Ctrl+D` selects with nothing selected: the one under `at`,
/// else the one just before it (the cursor at a word's end). `None` on
/// spaces or punctuation with no word beside it.
pub fn word_to_select(buffer: &Buffer, at: usize, iskeyword_extra: &[char]) -> Option<Range<usize>> {
    let is_word = |i: usize| buffer.char_at(i).is_some_and(|c| word_char(c, iskeyword_extra));
    let at = at.min(buffer.len_chars());
    let start_at = if is_word(at) {
        at
    } else if at > 0 && is_word(at - 1) {
        at - 1
    } else {
        return None;
    };
    let mut range = crate::textobject::word_at(buffer, start_at, iskeyword_extra);
    // A word's combining marks are its own.
    while range.end < buffer.len_chars() && buffer.char_at(range.end).is_some_and(|c| word_char(c, iskeyword_extra)) {
        range.end += 1;
    }
    while range.start > 0 && buffer.char_at(range.start - 1).is_some_and(|c| word_char(c, iskeyword_extra)) {
        range.start -= 1;
    }
    (!range.is_empty()).then_some(range)
}

/// Whether `range` is a whole word: word characters, with none just
/// outside it. A whole word finds other whole words; other text finds
/// itself anywhere.
pub fn is_whole_word(buffer: &Buffer, range: &Range<usize>, iskeyword_extra: &[char]) -> bool {
    if range.is_empty() {
        return false;
    }
    let is_word = |i: usize| buffer.char_at(i).is_some_and(|c| word_char(c, iskeyword_extra));
    range.clone().all(is_word) && !(range.start > 0 && is_word(range.start - 1)) && !is_word(range.end)
}

/// Every occurrence of `needle` in the buffer, in order and not
/// overlapping -- whole words only when `whole_word` -- as char ranges.
/// Stops at `limit`: `Err` with how many there were past it.
pub fn occurrences(buffer: &Buffer, needle: &str, whole_word: bool, iskeyword_extra: &[char], limit: usize) -> Result<Vec<Range<usize>>, usize> {
    occurrences_around(buffer, needle, whole_word, iskeyword_extra, limit, None)
}

/// `occurrences`, the scan anchored at the selection `at` (a char range)
/// when there's one: from it to the end, then from the start to it, so
/// the selection is always one of them however they'd overlap ("aaaaa"
/// with chars 1..3 selected: 1..3 and 3..5, not 0..2 and 2..4). In order.
pub fn occurrences_around(buffer: &Buffer, needle: &str, whole_word: bool, iskeyword_extra: &[char], limit: usize, at: Option<&Range<usize>>) -> Result<Vec<Range<usize>>, usize> {
    let mut found = Vec::new();
    if needle.is_empty() {
        return Ok(found);
    }
    // The whole text, once per press: fine for what a person selects in
    // a file Fenix opens; a scan of the rope by chunks would spare it.
    let text = buffer.rope().to_string();
    let anchor = at.map_or(0, |r| buffer.char_to_byte(r.start));
    let (mut after, after_over) = scan(&text, needle, whole_word, iskeyword_extra, anchor, text.len(), limit);
    let (before, before_over) = scan(&text, needle, whole_word, iskeyword_extra, 0, anchor, limit.saturating_sub(after.len()));
    found.extend(before.into_iter().map(|(s, e)| buffer.byte_to_char(s)..buffer.byte_to_char(e)));
    found.extend(after.drain(..).map(|(s, e)| buffer.byte_to_char(s)..buffer.byte_to_char(e)));
    let over = after_over + before_over;
    if over > 0 {
        return Err(found.len() + over);
    }
    Ok(found)
}

/// Non-overlapping matches of `needle` in `text` starting at or after
/// `from` and ending at or before `to`, as byte ranges, at most `limit`;
/// and how many more there were.
fn scan(text: &str, needle: &str, whole_word: bool, iskeyword_extra: &[char], from: usize, to: usize, limit: usize) -> (Vec<(usize, usize)>, usize) {
    let is_word = |c: char| word_char(c, iskeyword_extra);
    let mut found = Vec::new();
    let mut over = 0usize;
    let mut from = from;
    while let Some(i) = text[from..to].find(needle) {
        let start = from + i;
        let end = start + needle.len();
        let whole = !whole_word || (!text[..start].chars().next_back().is_some_and(is_word) && !text[end..].chars().next().is_some_and(is_word));
        if whole {
            if found.len() < limit {
                found.push((start, end));
            } else {
                over += 1;
            }
            from = end;
        } else {
            // Past this match's first character, on a char boundary.
            from = start + text[start..].chars().next().map_or(1, char::len_utf8);
        }
    }
    (found, over)
}

/// The next occurrence of `needle` that starts at or after `after`,
/// wrapping to the buffer's start, and that overlaps none of `taken`.
pub fn next_occurrence(buffer: &Buffer, needle: &str, whole_word: bool, iskeyword_extra: &[char], after: usize, taken: &[Range<usize>]) -> Option<Range<usize>> {
    // Anchored at the last taken (the selection), so it's one of them.
    let all = occurrences_around(buffer, needle, whole_word, iskeyword_extra, usize::MAX, taken.last()).ok()?;
    let free = |r: &&Range<usize>| !taken.iter().any(|t| r.start < t.end && t.start < r.end);
    all.iter().filter(free).find(|r| r.start >= after).or_else(|| all.iter().find(free)).cloned()
}

/// Where a cursor goes on `line` keeping to column `col`: the line's end
/// when it's shorter.
pub fn at_column(buffer: &Buffer, line: usize, col: usize) -> usize {
    buffer.line_start_char(line) + col.min(buffer.line_len(line))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::charclass::DEFAULT_ISKEYWORD_EXTRA as KW;
    use crate::test_util::buf;

    #[test]
    fn the_word_under_the_cursor_or_just_before_it() {
        let b = buf("let foo = bar;");
        assert_eq!(word_to_select(&b, 5, KW), Some(4..7), "inside");
        assert_eq!(word_to_select(&b, 4, KW), Some(4..7), "at its start");
        assert_eq!(word_to_select(&b, 7, KW), Some(4..7), "just after it");
        assert_eq!(word_to_select(&b, 8, KW), None, "on the =");
        assert_eq!(word_to_select(&b, 14, KW), None, "the end, after ;");
        assert_eq!(word_to_select(&buf(""), 0, KW), None, "an empty buffer");
        assert_eq!(word_to_select(&buf("ab"), 2, KW), Some(0..2), "the last line, no newline");
    }

    #[test]
    fn words_are_whole_only_with_nothing_wordy_beside_them() {
        let b = buf("foo foobar");
        assert!(is_whole_word(&b, &(0..3), KW));
        assert!(!is_whole_word(&b, &(4..7), KW), "foo in foobar");
        assert!(!is_whole_word(&b, &(2..5), KW), "o f");
        assert!(!is_whole_word(&b, &(3..3), KW), "empty");
    }

    #[test]
    fn occurrences_whole_words_or_anywhere() {
        let b = buf("foo foobar foo\nfoo");
        assert_eq!(occurrences(&b, "foo", true, KW, 10), Ok(vec![0..3, 11..14, 15..18]));
        assert_eq!(occurrences(&b, "foo", false, KW, 10).map(|v| v.len()), Ok(4));
        assert_eq!(occurrences(&b, "foo", false, KW, 2), Err(4), "past the limit: how many");
        assert_eq!(occurrences(&b, "", false, KW, 10), Ok(vec![]));
        assert_eq!(occurrences(&buf("aaaa"), "aa", false, KW, 10), Ok(vec![0..2, 2..4]), "not overlapping");
    }

    #[test]
    fn occurrences_are_char_indices_in_multi_byte_text() {
        let b = buf("été é été\nété");
        assert_eq!(occurrences(&b, "été", true, KW, 10), Ok(vec![0..3, 6..9, 10..13]));
        // `é` alone is a word of its own only where it stands alone.
        assert_eq!(occurrences(&b, "é", true, KW, 10), Ok(vec![Range { start: 4, end: 5 }]));
        let found = occurrences(&b, "t", false, KW, 10).unwrap();
        assert_eq!(found, vec![1..2, 7..8, 11..12]);
    }

    #[test]
    fn the_next_occurrence_wraps_and_skips_those_taken() {
        let b = buf("x foo foo foo");
        assert_eq!(next_occurrence(&b, "foo", true, KW, 5, &[Range { start: 2, end: 5 }]), Some(6..9));
        assert_eq!(next_occurrence(&b, "foo", true, KW, 13, &[Range { start: 10, end: 13 }]), Some(2..5), "wraps");
        assert_eq!(next_occurrence(&b, "foo", true, KW, 0, &[2..5, 6..9, 10..13]), None, "all taken");
        assert_eq!(next_occurrence(&b, "bar", true, KW, 0, &[]), None);
    }

    #[test]
    fn the_scan_starts_at_the_selection_so_overlapping_text_finds_the_rest() {
        let b = buf("aaaaa");
        assert_eq!(next_occurrence(&b, "aa", false, KW, 3, &[Range { start: 1, end: 3 }]), Some(3..5));
        assert_eq!(occurrences_around(&b, "aa", false, KW, 10, Some(&(1..3))), Ok(vec![1..3, 3..5]), "the selection's own among them");
    }

    #[test]
    fn a_combining_mark_is_part_of_its_word() {
        let b = buf("cafe\u{301} cafe");
        assert_eq!(word_to_select(&b, 1, KW), Some(0..5), "the accent with it");
        assert_eq!(occurrences(&b, "cafe", true, KW, 10), Ok(vec![Range { start: 6, end: 10 }]), "plain cafe isn't the accented one's start");
    }

    #[test]
    fn a_column_on_a_shorter_line_is_its_end() {
        let b = buf("abcdef\nab\n\nxyz");
        assert_eq!(at_column(&b, 1, 4), 9);
        assert_eq!(at_column(&b, 2, 4), 10, "an empty line");
        assert_eq!(at_column(&b, 3, 1), 12, "the last line, no newline");
        assert_eq!(at_column(&b, 3, 9), 14);
    }
}
