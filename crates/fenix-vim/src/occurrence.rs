//! Where more cursors go when standard keys add them from the keyboard:
//! the word to select under the cursor (`Ctrl+D`'s first press), the next
//! occurrence of a selection (each press after), every occurrence of it
//! (`Ctrl+Shift+L`), and a cursor on the line above or below
//! (`Ctrl+Alt+Up/Down`). Positions are char indices; a range's end is
//! exclusive, as standard keys' selections are.

use std::ops::Range;

use fenix_core::Buffer;

use crate::charclass::{classify, CharClass};

/// The word `Ctrl+D` selects with nothing selected: the one under `at`,
/// else the one just before it (the cursor at a word's end). `None` on
/// spaces or punctuation with no word beside it.
pub fn word_to_select(buffer: &Buffer, at: usize, iskeyword_extra: &[char]) -> Option<Range<usize>> {
    let is_word = |i: usize| buffer.char_at(i).is_some_and(|c| classify(c, iskeyword_extra) == CharClass::Word);
    let at = at.min(buffer.len_chars());
    let start_at = if is_word(at) {
        at
    } else if at > 0 && is_word(at - 1) {
        at - 1
    } else {
        return None;
    };
    let range = crate::textobject::word_at(buffer, start_at, iskeyword_extra);
    (!range.is_empty()).then_some(range)
}

/// Whether `range` is a whole word: word characters, with none just
/// outside it. A whole word finds other whole words; other text finds
/// itself anywhere.
pub fn is_whole_word(buffer: &Buffer, range: &Range<usize>, iskeyword_extra: &[char]) -> bool {
    if range.is_empty() {
        return false;
    }
    let is_word = |i: usize| buffer.char_at(i).is_some_and(|c| classify(c, iskeyword_extra) == CharClass::Word);
    range.clone().all(is_word) && !(range.start > 0 && is_word(range.start - 1)) && !is_word(range.end)
}

/// Every occurrence of `needle` in the buffer, in order and not
/// overlapping -- whole words only when `whole_word` -- as char ranges.
/// Stops at `limit`: `Err` with how many there were past it.
pub fn occurrences(buffer: &Buffer, needle: &str, whole_word: bool, iskeyword_extra: &[char], limit: usize) -> Result<Vec<Range<usize>>, usize> {
    let mut found = Vec::new();
    if needle.is_empty() {
        return Ok(found);
    }
    let text = buffer.rope().to_string();
    let is_word = |c: char| classify(c, iskeyword_extra) == CharClass::Word;
    let mut over = 0usize;
    let mut from = 0usize;
    while let Some(i) = text[from..].find(needle) {
        let start = from + i;
        let end = start + needle.len();
        let whole = !whole_word || (!text[..start].chars().next_back().is_some_and(is_word) && !text[end..].chars().next().is_some_and(is_word));
        if whole {
            if found.len() < limit {
                found.push(buffer.byte_to_char(start)..buffer.byte_to_char(end));
            } else {
                over += 1;
            }
            from = end;
        } else {
            // Past this match's first character, on a char boundary.
            from = start + text[start..].chars().next().map_or(1, char::len_utf8);
        }
    }
    if over > 0 {
        return Err(found.len() + over);
    }
    Ok(found)
}

/// The next occurrence of `needle` that starts at or after `after`,
/// wrapping to the buffer's start, and that overlaps none of `taken`.
pub fn next_occurrence(buffer: &Buffer, needle: &str, whole_word: bool, iskeyword_extra: &[char], after: usize, taken: &[Range<usize>]) -> Option<Range<usize>> {
    let all = occurrences(buffer, needle, whole_word, iskeyword_extra, usize::MAX).ok()?;
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
    fn a_column_on_a_shorter_line_is_its_end() {
        let b = buf("abcdef\nab\n\nxyz");
        assert_eq!(at_column(&b, 1, 4), 9);
        assert_eq!(at_column(&b, 2, 4), 10, "an empty line");
        assert_eq!(at_column(&b, 3, 1), 12, "the last line, no newline");
        assert_eq!(at_column(&b, 3, 9), 14);
    }
}
