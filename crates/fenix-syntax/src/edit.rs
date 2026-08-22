use tree_sitter::{InputEdit, Point};

/// A single low-level text edit, in char offsets against the *current*
/// (post-edit) source, plus the text that was removed.
///
/// `removed` is needed because `old_end_char`'s byte/line/column position
/// only existed in the *old* text -- by the time `apply_edits` runs, only
/// the new `source` is available, so the old end has to be reconstructed
/// from "how far does the removed text itself advance the position,
/// starting from `start_char`" rather than looked up directly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawEdit {
    pub start_char: usize,
    pub new_end_char: usize,
    pub removed: String,
}

/// Byte offset and (row, column-in-bytes) of the `target_char`-th char in
/// `source`, found by scanning forward from the start. tree-sitter works
/// in bytes (and byte-columns, not char-columns); `RawEdit` works in
/// chars, so every edit needs this conversion once per endpoint. `Buffer`
/// could do this faster via `ropey`'s indexed lookups, but `fenix-syntax`
/// deliberately doesn't depend on `fenix-core` (see the crate's design
/// notes) -- an `O(source length)` scan here is the accepted cost of
/// staying decoupled and testable with plain strings.
fn locate(source: &str, target_char: usize) -> (usize, Point) {
    let mut row = 0usize;
    let mut column = 0usize;
    for (i, (byte_idx, ch)) in source.char_indices().enumerate() {
        if i == target_char {
            return (byte_idx, Point { row, column });
        }
        if ch == '\n' {
            row += 1;
            column = 0;
        } else {
            column += ch.len_utf8();
        }
    }
    (source.len(), Point { row, column })
}

/// Where the removed text's own end falls, given where it started --
/// walking `removed` itself rather than the (no-longer-available) old
/// source, since nothing before `start` changed.
fn advance(start: Point, removed: &str) -> (usize, Point) {
    let mut row = start.row;
    let mut column = start.column;
    for ch in removed.chars() {
        if ch == '\n' {
            row += 1;
            column = 0;
        } else {
            column += ch.len_utf8();
        }
    }
    (removed.len(), Point { row, column })
}

pub(crate) fn to_input_edit(source: &str, edit: &RawEdit) -> InputEdit {
    let (start_byte, start_position) = locate(source, edit.start_char);
    let (new_end_byte, new_end_position) = locate(source, edit.new_end_char);
    let (removed_len, old_end_position) = advance(start_position, &edit.removed);
    InputEdit {
        start_byte,
        old_end_byte: start_byte + removed_len,
        new_end_byte,
        start_position,
        old_end_position,
        new_end_position,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pure_insertion_at_start() {
        // "hi" -> "xhi": inserted "x" at char 0, nothing removed.
        let edit = RawEdit { start_char: 0, new_end_char: 1, removed: String::new() };
        let result = to_input_edit("xhi", &edit);
        assert_eq!(result.start_byte, 0);
        assert_eq!(result.old_end_byte, 0); // nothing removed
        assert_eq!(result.new_end_byte, 1); // "x" is 1 byte
        assert_eq!(result.start_position, Point { row: 0, column: 0 });
        assert_eq!(result.old_end_position, Point { row: 0, column: 0 });
        assert_eq!(result.new_end_position, Point { row: 0, column: 1 });
    }

    #[test]
    fn pure_deletion() {
        // "hello" -> "ho": removed "ell" (chars 1..4).
        let edit = RawEdit { start_char: 1, new_end_char: 1, removed: "ell".to_string() };
        let result = to_input_edit("ho", &edit);
        assert_eq!(result.start_byte, 1);
        assert_eq!(result.old_end_byte, 4); // 1 + "ell".len()
        assert_eq!(result.new_end_byte, 1); // nothing inserted, end == start
    }

    #[test]
    fn replacement_spanning_a_newline_in_the_removed_text() {
        // "a\nb\nc" -> "aXc": removed "\nb\n" (chars 1..4), inserted "X".
        let edit = RawEdit { start_char: 1, new_end_char: 2, removed: "\nb\n".to_string() };
        let result = to_input_edit("aXc", &edit);
        assert_eq!(result.start_position, Point { row: 0, column: 1 });
        // removed text crosses two newlines -> old end is row 2, column 0
        assert_eq!(result.old_end_position, Point { row: 2, column: 0 });
        assert_eq!(result.new_end_position, Point { row: 0, column: 2 });
    }

    #[test]
    fn multi_byte_chars_use_byte_columns_not_char_columns() {
        // "café" -> "café!": 'é' is 2 bytes in UTF-8. Appending "!" after
        // it (char index 4) must land at byte column 5 (c=1,a=1,f=1,é=2),
        // not char column 4 -- tree-sitter's Point.column is byte-based.
        let edit = RawEdit { start_char: 4, new_end_char: 5, removed: String::new() };
        let result = to_input_edit("café!", &edit);
        assert_eq!(result.start_byte, 5);
        assert_eq!(result.start_position, Point { row: 0, column: 5 });
        assert_eq!(result.new_end_byte, 6);
    }

    #[test]
    fn insertion_at_end_of_file() {
        let edit = RawEdit { start_char: 2, new_end_char: 3, removed: String::new() };
        let result = to_input_edit("hi!", &edit);
        assert_eq!(result.start_byte, 2);
        assert_eq!(result.new_end_byte, 3);
    }
}
