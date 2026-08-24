use std::ops::Range;

use fenix_core::{Buffer, Cursor};
use regex::Regex;

use crate::textobject::{self, TextObject};

/// Where a `/pattern` (forward) or `?pattern` (backward) search from the
/// cursor's current position would land -- the char index of the found
/// match's start, or the compiled-regex error if `pattern` doesn't
/// parse. Searches the whole buffer as one `String` (via `Buffer::
/// text()`) rather than a windowed slice: this isn't a per-frame path
/// (only runs once per confirmed search/`n`/`N`/`*`/`#`), the same
/// reasoning already used for whole-buffer syntax reparse.
///
/// Forward search starts strictly *after* the cursor (so `/` on a match
/// you're already sitting on finds the *next* one, not the same one
/// again) and wraps to the first match in the buffer if nothing
/// qualifies past the cursor -- Vim's own default `wrapscan` behavior.
/// Backward search is the mirror: strictly before the cursor, wrapping
/// to the last match in the buffer.
pub fn find_next(buffer: &Buffer, cursor: &Cursor, pattern: &str, forward: bool) -> Result<Option<usize>, regex::Error> {
    let re = Regex::new(pattern)?;
    let text = buffer.text();
    let cursor_byte = buffer.char_to_byte(cursor.char_idx);

    let found = if forward {
        re.find_iter(&text)
            .find(|m| m.start() > cursor_byte)
            .or_else(|| re.find(&text))
    } else {
        re.find_iter(&text)
            .filter(|m| m.start() < cursor_byte)
            .last()
            .or_else(|| re.find_iter(&text).last())
    };

    Ok(found.map(|m| buffer.byte_to_char(m.start())))
}

/// Every match of `pattern` whose start falls within `byte_range`
/// (typically the currently-visible line range, converted to byte
/// offsets by the caller the same way `fenix-syntax`'s highlight spans
/// already are) -- the byte-range equivalent of `find_next`, but
/// returning *every* match in the window instead of just the nearest
/// one, for Vim's `hlsearch` (persistent match highlighting after a
/// confirmed search). Unlike `find_next`, this only scans the given
/// slice of `buffer.text()`, not the whole buffer -- called once per
/// `redraw()` while a search is active, so it follows the same
/// windowed-rendering discipline `syntax_highlights_for_visible_range`
/// already established, not `find_next`'s own "not a per-frame path"
/// whole-buffer scan. `byte_range` is clamped to the text's actual
/// length; an out-of-order or empty range after clamping yields an
/// empty `Vec` rather than a panic.
pub fn all_matches_in_range(buffer: &Buffer, pattern: &str, byte_range: Range<usize>) -> Result<Vec<Range<usize>>, regex::Error> {
    let re = Regex::new(pattern)?;
    let text = buffer.text();
    let start = byte_range.start.min(text.len());
    let end = byte_range.end.min(text.len());
    if start >= end {
        return Ok(Vec::new());
    }
    Ok(re.find_iter(&text[start..end]).map(|m| (start + m.start())..(start + m.end())).collect())
}

/// The word under the cursor, as a whole-word regex pattern (`\b...\b`,
/// with any regex-special characters in the word itself escaped) -- what
/// `*`/`#` seed their search from. `None` if the cursor isn't on a word
/// (e.g. sitting on whitespace or at the end of an empty buffer).
/// Reuses `textobject::span`'s existing inner-word scan rather than
/// re-implementing word-boundary detection.
pub fn word_under_cursor_pattern(buffer: &Buffer, cursor: &Cursor) -> Option<String> {
    let range = textobject::span(buffer, cursor, TextObject::InnerWord);
    if range.is_empty() {
        return None;
    }
    let word = buffer.text_range(range.start, range.end);
    if word.trim().is_empty() {
        return None;
    }
    Some(format!(r"\b{}\b", regex::escape(&word)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{buf, cur};

    #[test]
    fn forward_search_finds_the_next_match_past_the_cursor() {
        let b = buf("foo bar foo baz");
        let found = find_next(&b, &cur(0), "foo", true).unwrap();
        assert_eq!(found, Some(8)); // the second "foo", not the one under the cursor
    }

    #[test]
    fn forward_search_wraps_around_when_nothing_qualifies_past_the_cursor() {
        let b = buf("foo bar baz");
        let found = find_next(&b, &cur(1), "foo", true).unwrap(); // already past the only match
        assert_eq!(found, Some(0)); // wraps back to it
    }

    #[test]
    fn backward_search_finds_the_previous_match_before_the_cursor() {
        let b = buf("foo bar foo baz");
        let found = find_next(&b, &cur(15), "foo", false).unwrap();
        assert_eq!(found, Some(8));
    }

    #[test]
    fn backward_search_wraps_around_when_nothing_qualifies_before_the_cursor() {
        let b = buf("foo bar baz");
        let found = find_next(&b, &cur(0), "foo", false).unwrap();
        assert_eq!(found, Some(0)); // only match is at/after the cursor -- wraps to it
    }

    #[test]
    fn search_returns_none_when_the_pattern_matches_nothing() {
        let b = buf("foo bar baz");
        assert_eq!(find_next(&b, &cur(0), "xyz", true).unwrap(), None);
    }

    #[test]
    fn search_propagates_an_invalid_pattern_as_an_error() {
        let b = buf("foo");
        assert!(find_next(&b, &cur(0), "(unclosed", true).is_err());
    }

    #[test]
    fn search_supports_real_regex_syntax() {
        let b = buf("a1 b22 c333");
        let found = find_next(&b, &cur(0), r"\d+", true).unwrap();
        assert_eq!(found, Some(1)); // "1" in "a1"
    }

    #[test]
    fn word_under_cursor_pattern_is_whole_word_bounded() {
        let b = buf("foo.bar baz");
        let pattern = word_under_cursor_pattern(&b, &cur(0)).unwrap();
        assert_eq!(pattern, r"\bfoo\b");
        // Wouldn't match "foobar" as a substring -- confirm via find_next.
        let b2 = buf("foobar foo");
        let found = find_next(&b2, &cur(0), &pattern, true).unwrap();
        assert_eq!(found, Some(7)); // the standalone "foo", not the "foo" inside "foobar"
    }

    #[test]
    fn word_under_cursor_pattern_is_none_on_whitespace() {
        let b = buf("foo   bar");
        assert_eq!(word_under_cursor_pattern(&b, &cur(4)), None);
    }

    #[test]
    fn all_matches_in_range_finds_every_occurrence_in_the_slice() {
        let b = buf("foo bar foo baz foo");
        let matches = all_matches_in_range(&b, "foo", 0..b.text().len()).unwrap();
        assert_eq!(matches, vec![0..3, 8..11, 16..19]);
    }

    #[test]
    fn all_matches_in_range_excludes_matches_starting_outside_the_range() {
        let b = buf("foo bar foo baz foo");
        // Only the middle "foo" (bytes 8..11) starts inside [4, 15).
        let matches = all_matches_in_range(&b, "foo", 4..15).unwrap();
        assert_eq!(matches, vec![8..11]);
    }

    #[test]
    fn all_matches_in_range_returns_empty_for_no_matches() {
        let b = buf("bar baz");
        let matches = all_matches_in_range(&b, "foo", 0..b.text().len()).unwrap();
        assert!(matches.is_empty());
    }

    #[test]
    fn all_matches_in_range_clamps_an_out_of_bounds_range_instead_of_panicking() {
        let b = buf("foo");
        let matches = all_matches_in_range(&b, "foo", 0..1000).unwrap();
        assert_eq!(matches, vec![0..3]);
    }

    #[test]
    fn all_matches_in_range_returns_empty_for_a_backwards_range() {
        let b = buf("foo foo");
        let (start, end) = (5, 2); // deliberately backwards, not a literal clippy would flag
        let matches = all_matches_in_range(&b, "foo", start..end).unwrap();
        assert!(matches.is_empty());
    }

    #[test]
    fn all_matches_in_range_propagates_an_invalid_pattern_as_an_error() {
        let b = buf("foo");
        assert!(all_matches_in_range(&b, "(unclosed", 0..3).is_err());
    }
}
