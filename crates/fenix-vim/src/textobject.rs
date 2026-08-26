use std::ops::Range;

use fenix_core::{Buffer, Cursor};

use crate::bracket;
use crate::charclass::{classify, CharClass};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextObject {
    InnerWord,
    AWord,
    /// `i"`/`i'`/`` i` `` -- the char is whichever quote mark, so one
    /// pair of variants covers all three (`a"` etc. mirrors it for the
    /// "around" form). Scoped to the current line -- see `quote_span`'s
    /// own doc comment for why.
    InnerQuote(char),
    AQuote(char),
    /// `i(`/`i{`/`i[` (and their `b`/`B` aliases, resolved to the same
    /// variant at the keymap layer) -- the char is always the *opening*
    /// delimiter, `bracket_span` derives the closing one from it.
    InnerBracket(char),
    ABracket(char),
    InnerParagraph,
    AParagraph,
}

/// Whether `obj` should be treated as a linewise range by the caller
/// (`state.rs`'s operator-pending dispatch) -- true only for the
/// paragraph objects, whose natural unit is whole lines (deleting one
/// should remove the lines themselves, not just their text and leave
/// blanks behind, the way charwise objects do). Kept separate from
/// `span`'s own return type rather than folding it in as `(Range<usize>,
/// bool)` -- that would touch every existing call site (direct calls
/// and this file's own tests) just to carry `false` through the ones
/// that never needed it.
pub fn is_linewise(obj: TextObject) -> bool {
    matches!(obj, TextObject::InnerParagraph | TextObject::AParagraph)
}

/// The char range `obj` covers at the cursor.
pub fn span(buffer: &Buffer, cursor: &Cursor, obj: TextObject) -> Range<usize> {
    match obj {
        TextObject::InnerWord => word_span(buffer, cursor, false),
        TextObject::AWord => word_span(buffer, cursor, true),
        TextObject::InnerQuote(q) => quote_span(buffer, cursor, q, false),
        TextObject::AQuote(q) => quote_span(buffer, cursor, q, true),
        TextObject::InnerBracket(open) => bracket_span(buffer, cursor, open, false),
        TextObject::ABracket(open) => bracket_span(buffer, cursor, open, true),
        TextObject::InnerParagraph => paragraph_span(buffer, cursor, false),
        TextObject::AParagraph => paragraph_span(buffer, cursor, true),
    }
}

/// If the cursor sits on whitespace, `iw`/`aw` select that whitespace run
/// itself (matching real Vim: whitespace is its own kind of "word" for
/// this purpose).
fn word_span(buffer: &Buffer, cursor: &Cursor, around: bool) -> Range<usize> {
    let len = buffer.len_chars();
    if len == 0 {
        return 0..0;
    }
    let at = cursor.char_idx.min(len - 1);
    let class = classify(buffer.char_at(at).unwrap());

    let mut start = at;
    while start > 0 && classify(buffer.char_at(start - 1).unwrap()) == class {
        start -= 1;
    }
    let mut end = at + 1;
    while end < len && classify(buffer.char_at(end).unwrap()) == class {
        end += 1;
    }

    if !around {
        return start..end;
    }
    let mut a_end = end;
    let mut took_trailing = false;
    while a_end < len && classify(buffer.char_at(a_end).unwrap()) == CharClass::Space {
        a_end += 1;
        took_trailing = true;
    }
    if took_trailing {
        start..a_end
    } else {
        let mut a_start = start;
        while a_start > 0 && classify(buffer.char_at(a_start - 1).unwrap()) == CharClass::Space {
            a_start -= 1;
        }
        a_start..end
    }
}

/// The positions of the nearest enclosing (or next upcoming) pair of
/// `quote` characters on the cursor's own line -- real Vim's quote text
/// objects never cross a line boundary, since quotes almost never
/// legitimately span one in source code. Pairs up every occurrence of
/// `quote` on the line two-at-a-time (`chunks_exact(2)`, silently
/// dropping a final unpaired quote -- unbalanced-on-this-line is the
/// only case that can produce one, and there's no sane pair to report
/// for it) and picks the first pair whose closing quote is at or after
/// the cursor -- so a cursor sitting before any quotes on the line still
/// resolves to the first upcoming pair, matching real Vim's own
/// "search forward on the line" behavior for this case. `None` if the
/// line has no complete pair at or after the cursor. Shared by
/// `quote_span` and `state.rs`'s surround (`ds`/`cs`) support, which
/// needs the delimiter positions themselves, not the inner span.
pub(crate) fn quote_positions(buffer: &Buffer, cursor: &Cursor, quote: char) -> Option<(usize, usize)> {
    let len = buffer.len_chars();
    if len == 0 {
        return None;
    }
    let at = cursor.char_idx.min(len - 1);
    let (line, _) = buffer.line_col(&Cursor { char_idx: at, sticky_col: 0 });
    let line_start = buffer.line_start_char(line);
    let line_end = line_start + buffer.line_len(line);
    let positions: Vec<usize> = (line_start..line_end).filter(|&i| buffer.char_at(i) == Some(quote)).collect();
    positions.chunks_exact(2).map(|pair| (pair[0], pair[1])).find(|&(_, close)| close >= at)
}

/// `AQuote` includes both delimiters; no whitespace-eating refinement
/// like `AWord`'s (a disclosed simplification -- real Vim's `a"` also
/// eats one adjacent space, this doesn't). A cursor with no quote pair
/// on its line resolves to an empty, no-op range at the cursor itself,
/// same "nothing to select" posture `word_span` has for an empty buffer.
fn quote_span(buffer: &Buffer, cursor: &Cursor, quote: char, around: bool) -> Range<usize> {
    match quote_positions(buffer, cursor, quote) {
        Some((open, close)) => {
            if around {
                open..(close + 1)
            } else {
                (open + 1)..close
            }
        }
        None => {
            let at = cursor.char_idx.min(buffer.len_chars().saturating_sub(1));
            at..at
        }
    }
}

fn matching_close(open: char) -> char {
    match open {
        '{' => '}',
        '[' => ']',
        _ => ')',
    }
}

/// Tries `enclosing_pair` first (cursor already inside a pair); if
/// that fails, falls back to `targets.vim`'s own "search forward on the
/// line for the next pair" (mirrors `quote_span`'s existing forward-
/// search behavior, so `di(` becomes as forgiving as `di"` already is).
/// A no-op empty range at the cursor if neither finds anything.
fn bracket_span(buffer: &Buffer, cursor: &Cursor, open: char, around: bool) -> Range<usize> {
    let close = matching_close(open);
    let at = cursor.char_idx;
    let found = bracket::enclosing_pair(buffer, at, open, close).or_else(|| {
        let (line, _) = buffer.line_col(cursor);
        let line_end = buffer.line_start_char(line) + buffer.line_len(line);
        bracket::next_pair_on_line(buffer, at, open, close, line_end)
    });
    match found {
        Some((o, c)) => {
            if around {
                o..(c + 1)
            } else {
                (o + 1)..c
            }
        }
        None => {
            let at = at.min(buffer.len_chars().saturating_sub(1));
            at..at
        }
    }
}

/// A paragraph is a contiguous run of non-blank lines -- or, if the
/// cursor's own line is blank, the contiguous run of blank lines it
/// sits in (mirrors real Vim: a paragraph object on a blank line
/// selects the blank block, not the next real paragraph). `AParagraph`
/// prefers extending into trailing blank lines, falling back to leading
/// ones if there are none -- the same "prefer trailing, fall back to
/// leading" shape `word_span`'s own `around` handling already uses.
fn paragraph_span(buffer: &Buffer, cursor: &Cursor, around: bool) -> Range<usize> {
    let total_lines = buffer.line_count();
    if total_lines == 0 {
        return 0..0;
    }
    let (line, _) = buffer.line_col(cursor);
    let is_blank = |l: usize| buffer.line_len(l) == 0;
    let blank = is_blank(line);

    let mut start = line;
    while start > 0 && is_blank(start - 1) == blank {
        start -= 1;
    }
    let mut end = line;
    while end + 1 < total_lines && is_blank(end + 1) == blank {
        end += 1;
    }

    if around && !blank {
        let mut a_end = end;
        let mut took_trailing = false;
        while a_end + 1 < total_lines && is_blank(a_end + 1) {
            a_end += 1;
            took_trailing = true;
        }
        if took_trailing {
            return linewise_char_range(buffer, start, a_end);
        }
        let mut a_start = start;
        while a_start > 0 && is_blank(a_start - 1) {
            a_start -= 1;
        }
        return linewise_char_range(buffer, a_start, end);
    }
    linewise_char_range(buffer, start, end)
}

/// `line_a..line_b`'s full char range, including the last line's own
/// terminator (so deleting it collapses the lines away entirely) --
/// same convention `state.rs`'s own `linewise_range` uses for `dd`/`dG`,
/// duplicated here (rather than exposed from `state.rs`) to keep this a
/// small, self-contained module with no dependency back on its own
/// caller.
fn linewise_char_range(buffer: &Buffer, line_a: usize, line_b: usize) -> Range<usize> {
    let start = buffer.line_start_char(line_a);
    let end = if line_b + 1 < buffer.line_count() { buffer.line_start_char(line_b + 1) } else { buffer.len_chars() };
    start..end
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{buf, cur};

    #[test]
    fn inner_word_selects_just_the_word() {
        let b = buf("foo bar baz");
        assert_eq!(span(&b, &cur(5), TextObject::InnerWord), 4..7); // "bar"
    }

    #[test]
    fn a_word_prefers_trailing_whitespace() {
        let b = buf("foo bar baz");
        assert_eq!(span(&b, &cur(5), TextObject::AWord), 4..8); // "bar "
    }

    #[test]
    fn a_word_falls_back_to_leading_whitespace_at_end_of_buffer() {
        let b = buf("foo bar");
        assert_eq!(span(&b, &cur(5), TextObject::AWord), 3..7); // " bar"
    }

    #[test]
    fn text_object_on_punctuation_selects_the_punctuation_run() {
        let b = buf("foo.bar");
        assert_eq!(span(&b, &cur(3), TextObject::InnerWord), 3..4); // "."
    }

    #[test]
    fn text_object_on_whitespace_selects_the_whitespace_run() {
        let b = buf("foo   bar");
        assert_eq!(span(&b, &cur(4), TextObject::InnerWord), 3..6);
    }

    #[test]
    fn inner_quote_selects_the_text_between_the_nearest_pair() {
        let b = buf(r#"say "hello world" now"#);
        assert_eq!(span(&b, &cur(8), TextObject::InnerQuote('"')), 5..16); // "hello world"
    }

    #[test]
    fn a_quote_includes_both_delimiters() {
        let b = buf(r#"say "hello world" now"#);
        assert_eq!(span(&b, &cur(8), TextObject::AQuote('"')), 4..17); // "\"hello world\""
    }

    #[test]
    fn quote_object_before_any_quote_on_the_line_selects_the_upcoming_pair() {
        let b = buf(r#"x = "y""#);
        assert_eq!(span(&b, &cur(0), TextObject::InnerQuote('"')), 5..6); // "y"
    }

    #[test]
    fn quote_object_with_no_pair_on_the_line_is_a_no_op() {
        let b = buf("no quotes here");
        let at = cur(3);
        assert_eq!(span(&b, &at, TextObject::InnerQuote('"')), 3..3);
    }

    #[test]
    fn inner_bracket_selects_the_innermost_enclosing_pair() {
        let b = buf("outer(a, inner(b, c), d)");
        // cursor on 'b' (char index 15), inside the inner parens
        assert_eq!(span(&b, &cur(15), TextObject::InnerBracket('(')), 15..19); // "b, c"
    }

    #[test]
    fn a_bracket_includes_both_delimiters() {
        let b = buf("(hello)");
        assert_eq!(span(&b, &cur(3), TextObject::ABracket('(')), 0..7);
    }

    #[test]
    fn bracket_object_works_when_the_cursor_sits_on_the_open_delimiter() {
        let b = buf("(hello)");
        assert_eq!(span(&b, &cur(0), TextObject::InnerBracket('(')), 1..6);
    }

    #[test]
    fn bracket_object_with_no_enclosing_pair_is_a_no_op() {
        let b = buf("no brackets here");
        let at = cur(3);
        assert_eq!(span(&b, &at, TextObject::InnerBracket('(')), 3..3);
    }

    #[test]
    fn bracket_object_before_any_pair_on_the_line_finds_the_upcoming_one() {
        // targets.vim-style forward search -- the same forgiveness
        // `di"` already has for quotes, now extended to brackets.
        let b = buf("x = (hello)");
        assert_eq!(span(&b, &cur(0), TextObject::InnerBracket('(')), 5..10); // "hello"
    }

    #[test]
    fn inner_paragraph_selects_the_contiguous_non_blank_lines() {
        let b = buf("a\nb\n\nc\nd\n");
        assert_eq!(span(&b, &cur(0), TextObject::InnerParagraph), 0..4); // "a\nb\n"
        assert!(!is_linewise(TextObject::InnerWord));
        assert!(is_linewise(TextObject::InnerParagraph));
    }

    #[test]
    fn a_paragraph_extends_into_trailing_blank_lines() {
        let b = buf("a\nb\n\n\nc\n");
        // cursor on "a" (line 0): paragraph is lines 0-1, "ap" should
        // also eat the two blank lines that follow (2-3).
        assert_eq!(span(&b, &cur(0), TextObject::AParagraph), 0..b.line_start_char(4));
    }

    #[test]
    fn paragraph_object_on_a_blank_line_selects_the_blank_run() {
        let b = buf("a\n\n\nb\n");
        let (line, _) = b.line_col(&Cursor { char_idx: b.line_start_char(1), sticky_col: 0 });
        assert_eq!(line, 1);
        assert_eq!(span(&b, &cur(b.line_start_char(1)), TextObject::InnerParagraph), b.line_start_char(1)..b.line_start_char(3));
    }
}
