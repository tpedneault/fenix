use fenix_core::{Buffer, Cursor};

use crate::motion;

/// The default spaces-per-indent-level, before any `:set shiftwidth=N`
/// (`VimState::indent_width`, runtime-configurable -- see `state.rs` and
/// `substitute.rs`'s `:set` handling). Tabs are always inserted as
/// spaces, not a literal `'\t'`: the render pipeline has no tab-stop
/// logic (every character occupies exactly one fixed-width column), so
/// a real tab character would render as a single narrow column instead
/// of expanding to the configured width -- a disclosed scope cut, not
/// an oversight.
pub const DEFAULT_INDENT_WIDTH: usize = 4;

/// How many spaces a Tab press at column `col` should insert to reach
/// the next multiple of `width` (soft-tab / tab-stop behavior), rather
/// than always inserting a flat `width` regardless of where the cursor
/// already is.
pub fn spaces_to_next_stop(col: usize, width: usize) -> usize {
    width - (col % width)
}

/// The exact leading-whitespace substring of `line` (spaces and/or
/// tabs, verbatim) -- what Enter/`o`/`O` carry over onto a new line.
pub fn leading_whitespace(buffer: &Buffer, line: usize) -> String {
    let start = buffer.line_start_char(line);
    let first = motion::line_first_non_blank(buffer, line);
    buffer.text_range(start, first)
}

/// `>>`: prepends `width` spaces at the start of `line`. Its own atomic
/// undo step, same as `finish_operator`'s delete/yank calls -- `>>`/`<<`
/// are stand-alone Normal-mode actions, not part of an active
/// Insert-mode coalescing run.
pub fn indent_line(buffer: &mut Buffer, cursor: &mut Cursor, line: usize, width: usize) {
    let at = buffer.line_start_char(line);
    cursor.char_idx = at;
    buffer.insert_str(cursor, &" ".repeat(width));
}

/// `<<`: removes up to `width` leading whitespace chars (spaces or
/// tabs) from the start of `line` -- fewer if the line has less,
/// stopping at the first non-space/tab char either way.
pub fn dedent_line(buffer: &mut Buffer, cursor: &mut Cursor, line: usize, width: usize) {
    let start = buffer.line_start_char(line);
    let end_of_line = start + buffer.line_len(line);
    let mut end = start;
    while end < end_of_line && end - start < width && matches!(buffer.char_at(end), Some(' ') | Some('\t')) {
        end += 1;
    }
    buffer.delete_range(cursor, start, end);
}

pub fn is_opening_bracket(c: char) -> bool {
    matches!(c, '{' | '(' | '[')
}

pub fn is_closing_bracket(c: char) -> bool {
    matches!(c, '}' | ')' | ']')
}

/// The closing bracket that pairs with an opening one, for auto-closing
/// in Insert mode -- `None` for anything `is_opening_bracket` wouldn't
/// accept.
pub fn matching_close_bracket(c: char) -> Option<char> {
    match c {
        '{' => Some('}'),
        '(' => Some(')'),
        '[' => Some(']'),
        _ => None,
    }
}

/// Whether every char from the start of the cursor's line up to (not
/// including) the cursor itself is whitespace -- i.e. nothing real has
/// been typed on this line yet. Used to gate the electric-dedent-on-
/// closing-bracket behavior to only the "first char on the line" case.
pub fn line_blank_before_cursor(buffer: &Buffer, cursor: &Cursor) -> bool {
    let (line, _) = buffer.line_col(cursor);
    let start = buffer.line_start_char(line);
    buffer.text_range(start, cursor.char_idx).chars().all(|c| c == ' ' || c == '\t')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::buf;

    #[test]
    fn spaces_to_next_stop_lands_on_the_next_multiple_of_indent_width() {
        assert_eq!(spaces_to_next_stop(0, 4), 4);
        assert_eq!(spaces_to_next_stop(2, 4), 2);
        assert_eq!(spaces_to_next_stop(4, 4), 4);
        assert_eq!(spaces_to_next_stop(5, 4), 3);
    }

    #[test]
    fn spaces_to_next_stop_honors_a_non_default_width() {
        assert_eq!(spaces_to_next_stop(0, 3), 3);
        assert_eq!(spaces_to_next_stop(2, 3), 1);
        assert_eq!(spaces_to_next_stop(3, 3), 3);
    }

    #[test]
    fn leading_whitespace_returns_the_exact_prefix() {
        let b = buf("    let x = 1;\nno_indent();\n\tone_tab();");
        assert_eq!(leading_whitespace(&b, 0), "    ");
        assert_eq!(leading_whitespace(&b, 1), "");
        assert_eq!(leading_whitespace(&b, 2), "\t");
    }

    #[test]
    fn indent_line_prepends_a_level_and_moves_cursor_to_line_start() {
        let mut b = buf("foo\nbar");
        let mut c = Cursor::at_start();
        indent_line(&mut b, &mut c, 1, 4);
        assert_eq!(b.text(), "foo\n    bar");
    }

    #[test]
    fn indent_line_honors_a_non_default_width() {
        let mut b = buf("foo\nbar");
        let mut c = Cursor::at_start();
        indent_line(&mut b, &mut c, 1, 3);
        assert_eq!(b.text(), "foo\n   bar");
    }

    #[test]
    fn dedent_line_removes_up_to_indent_width_leading_chars() {
        let mut b = buf("        deeply indented");
        let mut c = Cursor::at_start();
        dedent_line(&mut b, &mut c, 0, 4);
        assert_eq!(b.text(), "    deeply indented");
    }

    #[test]
    fn dedent_line_removes_only_whats_there_when_less_than_indent_width() {
        let mut b = buf("  two spaces");
        let mut c = Cursor::at_start();
        dedent_line(&mut b, &mut c, 0, 4);
        assert_eq!(b.text(), "two spaces");
    }

    #[test]
    fn dedent_line_on_unindented_text_is_a_no_op() {
        let mut b = buf("no indent here");
        let mut c = Cursor::at_start();
        dedent_line(&mut b, &mut c, 0, 4);
        assert_eq!(b.text(), "no indent here");
    }

    #[test]
    fn dedent_line_stops_at_a_non_whitespace_char() {
        // Tab followed by a real char, width 4 -- only the tab (1 char)
        // should be removed, not eat into "x".
        let mut b = buf("\tx");
        let mut c = Cursor::at_start();
        dedent_line(&mut b, &mut c, 0, 4);
        assert_eq!(b.text(), "x");
    }

    #[test]
    fn dedent_line_honors_a_non_default_width() {
        let mut b = buf("   three spaces");
        let mut c = Cursor::at_start();
        dedent_line(&mut b, &mut c, 0, 3);
        assert_eq!(b.text(), "three spaces");
    }

    #[test]
    fn bracket_classification() {
        for c in ['{', '(', '['] {
            assert!(is_opening_bracket(c));
            assert!(!is_closing_bracket(c));
        }
        for c in ['}', ')', ']'] {
            assert!(is_closing_bracket(c));
            assert!(!is_opening_bracket(c));
        }
        assert!(!is_opening_bracket('a'));
        assert!(!is_closing_bracket('a'));
    }

    #[test]
    fn matching_close_bracket_pairs_each_opener_and_rejects_non_openers() {
        assert_eq!(matching_close_bracket('{'), Some('}'));
        assert_eq!(matching_close_bracket('('), Some(')'));
        assert_eq!(matching_close_bracket('['), Some(']'));
        assert_eq!(matching_close_bracket('}'), None);
        assert_eq!(matching_close_bracket('a'), None);
    }

    #[test]
    fn line_blank_before_cursor_true_only_when_nothing_real_precedes_it() {
        let b = buf("    x");
        let blank = Cursor { char_idx: 4, sticky_col: 4 }; // right after the 4 spaces, before "x"
        assert!(line_blank_before_cursor(&b, &blank));

        let past_x = Cursor { char_idx: 5, sticky_col: 5 }; // right after "x"
        assert!(!line_blank_before_cursor(&b, &past_x));
    }
}
