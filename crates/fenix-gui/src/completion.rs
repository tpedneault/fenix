//! Pure logic for the autocompletion popup: extracting the in-progress
//! identifier prefix at the cursor. No knowledge of rendering, Vim modes,
//! or completion *sources* -- `App` (in `app.rs`) owns the actual popup
//! state and drives `fenix-completion`/`fenix-picker` with what this
//! module computes, the same "small self-contained module" role
//! `dashboard.rs`/`icon.rs` already play.

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
}
