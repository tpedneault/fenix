//! Pure logic for the autocompletion popup: extracting the in-progress
//! identifier prefix at the cursor, and the buffer-word candidate source
//! (the one completion source that works for any language, not just
//! Tcl). No knowledge of rendering, Vim modes, or the Tcl-specific
//! completion *sources* -- `App` (in `app.rs`) owns the actual popup
//! state and drives `fenix-completion`/`fenix-picker` with what this
//! module computes, the same "small self-contained module" role
//! `dashboard.rs`/`icon.rs` already play.

use std::collections::HashSet;

use fenix_core::{Buffer, Cursor};

fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The identifier prefix ending exactly at the cursor -- e.g. with the
/// cursor right after `se` in `se|t`, returns `(prefix_start_char_idx,
/// "se")`. Scans backward only: unlike `fenix-vim`'s `textobject::span`
/// (built for Normal-mode text objects, where the cursor sits *inside* a
/// word and both directions matter), nothing after the cursor in Insert
/// mode is part of what's being typed. `None` when the cursor isn't
/// immediately preceded by an identifier char (on whitespace, punctuation,
/// buffer start, or an empty buffer) -- the completion popup has nothing
/// to filter by and should stay closed.
///
/// Deliberately not `::`-aware (a plain alnum-or-underscore classifier,
/// the same "Word" class `fenix-vim`'s own `charclass::classify` uses
/// elsewhere, just not reused directly since that's crate-private) -- a
/// namespace-qualified prefix like `myns::gr` only completes the `gr`
/// part after the last `::`. A disclosed simplification, not a bug.
pub fn prefix_at_cursor(buffer: &Buffer, cursor: &Cursor) -> Option<(usize, String)> {
    let end = cursor.char_idx.min(buffer.len_chars());
    let mut start = end;
    while start > 0 && buffer.char_at(start - 1).is_some_and(is_ident_char) {
        start -= 1;
    }
    if start == end {
        return None;
    }
    let prefix: String = (start..end).map(|i| buffer.char_at(i).unwrap()).collect();
    Some((start, prefix))
}

/// Every distinct identifier-like token already present in `buffer` --
/// the "complete a word from what's already been typed in this file"
/// source real Vim's own `<C-n>`/`<C-p>` draws on, and the only
/// completion source that isn't Tcl-specific (see `App::completion_
/// candidates`, which layers this on top of the Tcl keyword/ctags pool
/// for a Tcl buffer, or uses it alone for anything else). Purely-numeric
/// tokens (`123`) are skipped -- not something worth ever completing to.
/// Scans the whole buffer on every call, same "just clone the text" cost
/// as `Buffer::text()`'s other callers (`format_buffer`, syntax
/// highlighting) -- fine at the size files are actually edited at,
/// expensive on a huge one; not attempting anything cleverer here.
/// Iteration order is whatever `HashSet` happens to produce -- callers
/// already fuzzy-filter/sort the result, so token order was never
/// meaningful.
pub fn buffer_words(buffer: &Buffer) -> HashSet<String> {
    let text = buffer.text();
    text.split(|c: char| !is_ident_char(c))
        .filter(|w| !w.is_empty() && !w.chars().all(|c| c.is_ascii_digit()))
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buf(text: &str) -> Buffer {
        Buffer::from_text(text)
    }

    fn cur(char_idx: usize) -> Cursor {
        Cursor { char_idx, sticky_col: 0 }
    }

    #[test]
    fn cursor_mid_word_returns_the_prefix_up_to_the_cursor() {
        let buffer = buf("set");
        // cursor between "se" and "t"
        let (start, prefix) = prefix_at_cursor(&buffer, &cur(2)).unwrap();
        assert_eq!(start, 0);
        assert_eq!(prefix, "se");
    }

    #[test]
    fn cursor_right_after_a_full_word_returns_the_whole_word() {
        let buffer = buf("puts");
        let (start, prefix) = prefix_at_cursor(&buffer, &cur(4)).unwrap();
        assert_eq!(start, 0);
        assert_eq!(prefix, "puts");
    }

    #[test]
    fn cursor_on_whitespace_returns_none() {
        let buffer = buf("set x");
        // cursor right after "set " (on the space, before "x")
        let (_, none_or_x) = match prefix_at_cursor(&buffer, &cur(4)) {
            Some((s, p)) => (s, p),
            None => return,
        };
        panic!("expected None, got prefix starting elsewhere: {none_or_x}");
    }

    #[test]
    fn cursor_right_after_punctuation_returns_none() {
        let buffer = buf("foo::");
        assert!(prefix_at_cursor(&buffer, &cur(5)).is_none());
    }

    #[test]
    fn cursor_at_buffer_start_returns_none() {
        let buffer = buf("set");
        assert!(prefix_at_cursor(&buffer, &cur(0)).is_none());
    }

    #[test]
    fn empty_buffer_returns_none() {
        let buffer = buf("");
        assert!(prefix_at_cursor(&buffer, &cur(0)).is_none());
    }

    #[test]
    fn namespace_qualified_prefix_only_completes_after_the_last_separator() {
        let buffer = buf("myns::gr");
        let (start, prefix) = prefix_at_cursor(&buffer, &cur(8)).unwrap();
        assert_eq!(start, 6);
        assert_eq!(prefix, "gr");
    }

    #[test]
    fn underscore_is_an_identifier_char() {
        let buffer = buf("my_var");
        let (start, prefix) = prefix_at_cursor(&buffer, &cur(6)).unwrap();
        assert_eq!(start, 0);
        assert_eq!(prefix, "my_var");
    }

    #[test]
    fn buffer_words_finds_every_distinct_identifier_token() {
        let buffer = buf("let seek_value = 1;\nlet other = seek_value;\n");
        let words = buffer_words(&buffer);
        assert!(words.contains("let"));
        assert!(words.contains("seek_value"));
        assert!(words.contains("other"));
        // "seek_value" appears twice but is only one distinct word.
        assert_eq!(words.iter().filter(|w| *w == "seek_value").count(), 1);
    }

    #[test]
    fn buffer_words_skips_purely_numeric_tokens() {
        let buffer = buf("let x = 123;\nlet y2 = 4;\n");
        let words = buffer_words(&buffer);
        assert!(!words.contains("123"));
        assert!(!words.contains("4"));
        assert!(words.contains("y2")); // alphanumeric but not *purely* digits
    }

    #[test]
    fn buffer_words_is_empty_for_an_empty_buffer() {
        let buffer = buf("");
        assert!(buffer_words(&buffer).is_empty());
    }

    #[test]
    fn buffer_words_ignores_punctuation_and_whitespace_boundaries() {
        let buffer = buf("foo::bar, baz.qux(quux)");
        let words = buffer_words(&buffer);
        assert_eq!(words, HashSet::from(["foo", "bar", "baz", "qux", "quux"].map(str::to_string)));
    }
}
