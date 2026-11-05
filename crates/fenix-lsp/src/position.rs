//! Converts between ropey's native indexing (char offsets -- Unicode
//! *scalar values*, one per `char`) and LSP's `Position` (UTF-16
//! *code-unit* line/character offsets, mandated by the spec regardless
//! of the server's or client's own native string representation). The
//! two disagree on exactly one thing: any character outside the Basic
//! Multilingual Plane (most emoji, some CJK extension/historic-script
//! characters) is always a single `char`/scalar value but encodes as
//! *two* UTF-16 code units (a surrogate pair) -- so a line containing
//! one of those needs its LSP column computed by actually walking the
//! line's characters and summing `char::len_utf16()`, not by assuming
//! "column N" means the same thing in both indexings.
//!
//! This is the one place that distinction is handled -- get it wrong
//! here and every position this crate's caller sends to or receives
//! from a server silently drifts by however many wide characters
//! preceded it on that line.

use ropey::RopeSlice;

/// Converts a char offset into `rope` to an LSP `Position`.
pub fn char_offset_to_position(rope: &ropey::Rope, char_offset: usize) -> lsp_types::Position {
    let line = rope.char_to_line(char_offset);
    let line_start_char = rope.line_to_char(line);
    let col_chars = char_offset - line_start_char;
    let line_slice = rope.line(line);
    let utf16_col: usize = line_slice.chars().take(col_chars).map(char::len_utf16).sum();
    lsp_types::Position { line: line as u32, character: utf16_col as u32 }
}

/// The inverse: an LSP `Position` to a char offset into `rope`. Clamps
/// rather than panicking on a position past what `rope` actually
/// contains -- a server describing a position against a slightly
/// stale view of a buffer this client has since edited further is a
/// normal, recoverable race (LSP is inherently asynchronous), not a
/// bug worth crashing over. A `character` that lands *inside* a
/// surrogate pair (a genuinely malformed position no compliant server
/// should send) is clamped to just before that character rather than
/// splitting it.
pub fn position_to_char_offset(rope: &ropey::Rope, position: lsp_types::Position) -> usize {
    let last_line = rope.len_lines().saturating_sub(1);
    let line = (position.line as usize).min(last_line);
    let line_start_char = rope.line_to_char(line);
    let line_slice = rope.line(line);
    let content_len = line_content_char_len(line_slice);

    let mut utf16_remaining = position.character as usize;
    let mut chars_consumed = 0;
    for c in line_slice.chars().take(content_len) {
        if utf16_remaining == 0 {
            break;
        }
        let units = c.len_utf16();
        if units > utf16_remaining {
            break;
        }
        utf16_remaining -= units;
        chars_consumed += 1;
    }
    line_start_char + chars_consumed
}

/// How many of `line`'s chars are actual content, excluding its own
/// line terminator (`\n` or `\r\n`) -- ropey's `Rope::line` includes the
/// terminator in the slice it returns, which would otherwise let
/// `position_to_char_offset` walk into it and land one line too far in
/// when a position's `character` overruns the real content (a stale-
/// position race, same reasoning as this function's own caller).
fn line_content_char_len(line: RopeSlice) -> usize {
    let total = line.len_chars();
    if total == 0 {
        return 0;
    }
    if line.char(total - 1) == '\n' {
        if total >= 2 && line.char(total - 2) == '\r' {
            total - 2
        } else {
            total - 1
        }
    } else {
        total
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ropey::Rope;

    #[test]
    fn ascii_only_column_matches_char_offset_exactly() {
        let rope = Rope::from_str("hello\nworld");
        // char offset 8 is the 'r' in "world" (line 1, col 2)
        assert_eq!(char_offset_to_position(&rope, 8), lsp_types::Position { line: 1, character: 2 });
        assert_eq!(position_to_char_offset(&rope, lsp_types::Position { line: 1, character: 2 }), 8);
    }

    #[test]
    fn a_bmp_multibyte_char_still_counts_as_one_utf16_unit() {
        // 'é' is 2 bytes in UTF-8 but exactly 1 UTF-16 code unit (it's
        // in the BMP) -- the LSP column right after it should be 1, not
        // 2, and not byte-length-dependent at all.
        let rope = Rope::from_str("éx");
        assert_eq!(char_offset_to_position(&rope, 1), lsp_types::Position { line: 0, character: 1 });
        assert_eq!(char_offset_to_position(&rope, 2), lsp_types::Position { line: 0, character: 2 });
        assert_eq!(position_to_char_offset(&rope, lsp_types::Position { line: 0, character: 1 }), 1);
    }

    #[test]
    fn a_non_bmp_character_counts_as_two_utf16_units_but_one_char_offset() {
        // U+1F600 (😀) needs a UTF-16 surrogate pair -- one char offset
        // in ropey's indexing, but two in LSP's.
        let rope = Rope::from_str("😀x");
        assert_eq!('😀'.len_utf16(), 2);
        // char offset 1 is right after the emoji -- LSP column 2, not 1.
        assert_eq!(char_offset_to_position(&rope, 1), lsp_types::Position { line: 0, character: 2 });
        // char offset 2 is right after the 'x' that follows it.
        assert_eq!(char_offset_to_position(&rope, 2), lsp_types::Position { line: 0, character: 3 });
        assert_eq!(position_to_char_offset(&rope, lsp_types::Position { line: 0, character: 2 }), 1);
        assert_eq!(position_to_char_offset(&rope, lsp_types::Position { line: 0, character: 3 }), 2);
    }

    #[test]
    fn round_trips_through_multiple_lines_with_mixed_width_characters() {
        let rope = Rope::from_str("plain\ncafé 😀 done\nlast");
        for char_offset in 0..rope.len_chars() {
            let pos = char_offset_to_position(&rope, char_offset);
            assert_eq!(position_to_char_offset(&rope, pos), char_offset, "round-trip failed for char_offset {char_offset}");
        }
    }

    #[test]
    fn a_position_on_a_line_past_the_end_of_the_rope_clamps_to_the_last_line() {
        let rope = Rope::from_str("only line");
        let pos = lsp_types::Position { line: 50, character: 0 };
        // Clamps to the start of the only real line rather than
        // panicking on an out-of-range line index.
        assert_eq!(position_to_char_offset(&rope, pos), 0);
    }

    #[test]
    fn a_character_column_past_the_lines_real_content_clamps_to_the_lines_end() {
        let rope = Rope::from_str("hi\nmore");
        let pos = lsp_types::Position { line: 0, character: 999 };
        // Clamps to right before the newline (char offset 2), not past
        // it into the next line.
        assert_eq!(position_to_char_offset(&rope, pos), 2);
    }

    #[test]
    fn handles_crlf_line_endings_without_counting_the_carriage_return_as_content() {
        let rope = Rope::from_str("one\r\ntwo");
        assert_eq!(char_offset_to_position(&rope, 5), lsp_types::Position { line: 1, character: 0 });
        assert_eq!(position_to_char_offset(&rope, lsp_types::Position { line: 1, character: 0 }), 5);
        // Overrunning line 0's real content (3 chars: "one") still
        // clamps before the \r\n, not into it.
        assert_eq!(position_to_char_offset(&rope, lsp_types::Position { line: 0, character: 999 }), 3);
    }

    #[test]
    fn the_very_first_and_last_char_offsets_convert_correctly() {
        let rope = Rope::from_str("abc");
        assert_eq!(char_offset_to_position(&rope, 0), lsp_types::Position { line: 0, character: 0 });
        assert_eq!(char_offset_to_position(&rope, 3), lsp_types::Position { line: 0, character: 3 });
    }
}
