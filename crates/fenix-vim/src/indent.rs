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

/// A Markdown-style list marker recognized at the start of a line, once
/// its leading whitespace is stripped -- what `parse_list_item`
/// classifies a line as, and what `list_continuation_text` continues
/// onto the next one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListMarker {
    /// `-`, `*`, or `+` -- the literal marker character, carried
    /// through unchanged (a `*`-bulleted list stays `*`-bulleted).
    Bullet(char),
    /// `N.` or `N)` -- the parsed number.
    Ordered(u64),
}

/// One line's list-item shape, as `parse_list_item` reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListItem {
    /// The line's own leading whitespace, carried onto the continuation
    /// line unchanged -- a nested list stays at the same nesting depth.
    pub indent: String,
    pub marker: ListMarker,
    /// `true` for `)` (`1)`), `false` for `.` (`1.`) -- meaningless for
    /// `Bullet`.
    pub ordered_paren: bool,
    /// Whether the marker is immediately followed by a GFM task
    /// checkbox (`[ ]`/`[x]`/`[X]`) -- a continuation carries an
    /// *unchecked* one regardless of whether this one was checked,
    /// matching every real Markdown editor's own convention (finishing
    /// one task doesn't pre-complete the next).
    pub has_checkbox: bool,
    /// Whether the line has nothing (or only whitespace) after the
    /// marker/checkbox -- see `list_continuation_text`'s own doc
    /// comment for what this changes.
    pub content_is_empty: bool,
}

/// Parses `line`'s own list-marker shape, if it has one -- `None` for
/// an ordinary line. No per-language gate: a line shaped like a list
/// item is treated as one wherever it appears, the same "the text shape
/// alone is the signal" posture this module's own bracket-depth logic
/// already has (and the one `fenix_format::reindent` established at the
/// crate level) -- a bulleted or numbered list inside a doc comment is
/// just as reasonable to continue as one in a `.md` file.
pub fn parse_list_item(buffer: &Buffer, line: usize) -> Option<ListItem> {
    let start = buffer.line_start_char(line);
    let text = buffer.text_range(start, start + buffer.line_len(line));
    parse_list_item_text(&text)
}

/// The actual parsing, over plain text -- separated from `parse_list_
/// item` so it's directly unit-testable without a real `Buffer`.
fn parse_list_item_text(line_text: &str) -> Option<ListItem> {
    let indent_len = line_text.len() - line_text.trim_start_matches(' ').len();
    let (indent, rest) = line_text.split_at(indent_len);

    let (marker, ordered_paren, after_marker) =
        if let Some(c) = rest.chars().next().filter(|&c| c == '-' || c == '*' || c == '+') {
            (ListMarker::Bullet(c), false, &rest[1..])
        } else {
            let digits = rest.chars().take_while(char::is_ascii_digit).count();
            if digits == 0 {
                return None;
            }
            let after_digits = &rest[digits..];
            let delim = after_digits.chars().next().filter(|&c| c == '.' || c == ')')?;
            let n: u64 = rest[..digits].parse().ok()?;
            (ListMarker::Ordered(n), delim == ')', &after_digits[1..])
        };

    // CommonMark requires a space (or nothing at all) right after the
    // marker -- "-item"/"1.5" aren't list items, just text that starts
    // with a similar-looking character.
    let after_marker = if after_marker.is_empty() {
        after_marker
    } else {
        after_marker.strip_prefix(' ')?
    };
    let content = after_marker.trim_start_matches(' ');

    let (has_checkbox, content) = ["[ ] ", "[x] ", "[X] ", "[ ]", "[x]", "[X]"]
        .iter()
        .find_map(|prefix| content.strip_prefix(prefix))
        .map(|rest| (true, rest))
        .unwrap_or((false, content));

    Some(ListItem { indent: indent.to_string(), marker, ordered_paren, has_checkbox, content_is_empty: content.trim().is_empty() })
}

/// What Enter/`o` should insert to continue `item`'s list onto the next
/// line -- `None` for an empty item (nothing typed after its marker
/// yet), which both callers already treat as "fall through to the
/// plain carried-indent path" -- deliberately, since that path already
/// does exactly the right thing here: it carries the bare `indent`
/// alone, no marker, which *is* "leave the list." No separate cleanup
/// of the current line is needed either way -- Enter/`o` only ever
/// insert new text at/after the cursor, never rewrite what's already
/// there, so there's nothing dangling to remove.
///
/// A bulleted item continues with the same marker character; an
/// ordered one continues with its number plus one. Deliberately *not*
/// cascaded through the rest of the list (a numbered item inserted in
/// the middle doesn't renumber everything below it) -- CommonMark
/// renderers only look at a list's first number to decide where it
/// starts, so a "wrong" number past that point is cosmetic in the
/// source and invisible in the rendered output, not worth the
/// complexity of walking the rest of the list to fix up.
pub fn list_continuation_text(item: &ListItem) -> Option<String> {
    if item.content_is_empty {
        return None;
    }
    let mut s = item.indent.clone();
    match item.marker {
        ListMarker::Bullet(c) => {
            s.push(c);
            s.push(' ');
        }
        ListMarker::Ordered(n) => {
            s.push_str(&(n + 1).to_string());
            s.push(if item.ordered_paren { ')' } else { '.' });
            s.push(' ');
        }
    }
    if item.has_checkbox {
        s.push_str("[ ] ");
    }
    Some(s)
}

/// The buffer-absolute char index of `line`'s own task-checkbox state
/// character (between `[` and `]`: a space when unchecked, `x`/`X`
/// when checked), and whether it's currently checked -- `None` for a
/// line that isn't a list item, or one with no checkbox at all.
///
/// Locates it by searching `line`'s own text for the first `[ ]`/
/// `[x]`/`[X]` rather than re-deriving how far past the marker it must
/// sit: `parse_list_item`'s own contract already guarantees the
/// checkbox immediately follows the marker and its required space when
/// `has_checkbox` is true, so the first match *is* the real one, not a
/// look-alike appearing later in the line's own prose -- and it sidesteps
/// having to reconstruct the marker's exact source length, which for an
/// ordered item isn't simply "however many digits its parsed number
/// has" if the source itself has leading zeros.
pub fn checkbox_char_index(buffer: &Buffer, line: usize) -> Option<(usize, bool)> {
    let item = parse_list_item(buffer, line)?;
    if !item.has_checkbox {
        return None;
    }
    let start = buffer.line_start_char(line);
    let text = buffer.text_range(start, start + buffer.line_len(line));
    let bracket_byte = ["[ ]", "[x]", "[X]"].iter().find_map(|pat| text.find(pat))?;
    let state_byte = bracket_byte + 1; // one past the '[' -- still an ASCII (so char-boundary-safe) offset
    let state_char = text[state_byte..].chars().next()?;
    let char_offset = text[..state_byte].chars().count();
    Some((start + char_offset, state_char == 'x' || state_char == 'X'))
}

/// `SPC c x`: flips `line`'s own GFM task checkbox between checked and
/// unchecked in place. A no-op (returns `false`, buffer/cursor
/// untouched) if the line has no checkbox to toggle -- no per-file-type
/// gate, same "the text shape alone is the signal" posture every other
/// list-item function in this module already has.
pub fn toggle_checkbox(buffer: &mut Buffer, cursor: &mut Cursor, line: usize) -> bool {
    let Some((at, checked)) = checkbox_char_index(buffer, line) else { return false };
    let new_char = if checked { ' ' } else { 'x' };
    buffer.replace_range(cursor, at, at + 1, &new_char.to_string());
    true
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
    fn parse_list_item_recognizes_every_bullet_character() {
        for c in ['-', '*', '+'] {
            let line = format!("{c} item");
            let item = parse_list_item_text(&line).unwrap_or_else(|| panic!("{line:?} should parse"));
            assert_eq!(item.marker, ListMarker::Bullet(c));
            assert!(!item.content_is_empty);
        }
    }

    #[test]
    fn parse_list_item_reads_ordered_markers_with_either_delimiter() {
        let dot = parse_list_item_text("1. item").unwrap();
        assert_eq!(dot.marker, ListMarker::Ordered(1));
        assert!(!dot.ordered_paren);

        let paren = parse_list_item_text("42) item").unwrap();
        assert_eq!(paren.marker, ListMarker::Ordered(42));
        assert!(paren.ordered_paren);
    }

    #[test]
    fn parse_list_item_captures_leading_whitespace_as_indent() {
        let item = parse_list_item_text("    - item").unwrap();
        assert_eq!(item.indent, "    ");
    }

    #[test]
    fn parse_list_item_requires_a_space_right_after_the_marker() {
        // A marker glued straight to text isn't a list item at all --
        // just an ordinary line that happens to start with a similar
        // character ("-5" a negative number, "1.5" a decimal).
        assert!(parse_list_item_text("-item").is_none());
        assert!(parse_list_item_text("1.5").is_none());
    }

    #[test]
    fn parse_list_item_accepts_a_bare_marker_alone_on_the_line() {
        // "-" with nothing after it at all (not even a space) is still
        // a real, if empty, list item -- what you get right after
        // typing just the marker.
        let item = parse_list_item_text("-").unwrap();
        assert!(item.content_is_empty);
    }

    #[test]
    fn parse_list_item_detects_a_task_checkbox_and_excludes_it_from_content() {
        let unchecked = parse_list_item_text("- [ ] buy milk").unwrap();
        assert!(unchecked.has_checkbox);
        assert!(!unchecked.content_is_empty);

        let checked = parse_list_item_text("- [x] done").unwrap();
        assert!(checked.has_checkbox);

        let checked_upper = parse_list_item_text("- [X] done").unwrap();
        assert!(checked_upper.has_checkbox);

        // The checkbox alone, nothing typed after it yet, still counts
        // as an empty item.
        let empty = parse_list_item_text("- [ ]").unwrap();
        assert!(empty.has_checkbox);
        assert!(empty.content_is_empty);
    }

    #[test]
    fn parse_list_item_is_none_for_an_ordinary_line() {
        assert!(parse_list_item_text("just some text").is_none());
        assert!(parse_list_item_text("").is_none());
    }

    #[test]
    fn list_continuation_text_repeats_the_same_bullet_character() {
        let item = parse_list_item_text("  * item").unwrap();
        assert_eq!(list_continuation_text(&item), Some("  * ".to_string()));
    }

    #[test]
    fn list_continuation_text_increments_an_ordered_marker() {
        let dot = parse_list_item_text("3. item").unwrap();
        assert_eq!(list_continuation_text(&dot), Some("4. ".to_string()));

        let paren = parse_list_item_text("9) item").unwrap();
        assert_eq!(list_continuation_text(&paren), Some("10) ".to_string()));
    }

    #[test]
    fn list_continuation_text_carries_an_unchecked_checkbox_regardless_of_the_original_state() {
        let checked = parse_list_item_text("- [x] done").unwrap();
        assert_eq!(list_continuation_text(&checked), Some("- [ ] ".to_string()));
    }

    #[test]
    fn list_continuation_text_is_none_for_an_empty_item() {
        let item = parse_list_item_text("- ").unwrap();
        assert_eq!(list_continuation_text(&item), None);
    }

    #[test]
    fn checkbox_char_index_finds_the_state_character_and_reports_unchecked() {
        let b = buf("- [ ] todo");
        let (at, checked) = checkbox_char_index(&b, 0).unwrap();
        assert!(!checked);
        assert_eq!(b.char_at(at), Some(' '));
    }

    #[test]
    fn checkbox_char_index_reports_checked_for_either_case_of_x() {
        let lower = buf("- [x] done");
        let (_, checked) = checkbox_char_index(&lower, 0).unwrap();
        assert!(checked);

        let upper = buf("- [X] done");
        let (_, checked) = checkbox_char_index(&upper, 0).unwrap();
        assert!(checked);
    }

    #[test]
    fn checkbox_char_index_honors_leading_indent_and_an_ordered_marker() {
        let b = buf("   3. [ ] todo");
        let (at, checked) = checkbox_char_index(&b, 0).unwrap();
        assert!(!checked);
        assert_eq!(b.char_at(at), Some(' '));
    }

    #[test]
    fn checkbox_char_index_is_correct_even_with_a_leading_zero_in_the_ordered_number() {
        // The parsed number ("7") is one digit shorter than what's
        // actually in the source ("07") -- proves the position isn't
        // reconstructed from the parsed number's own digit count.
        let b = buf("07. [ ] todo");
        let (at, _) = checkbox_char_index(&b, 0).unwrap();
        assert_eq!(b.char_at(at), Some(' '));
    }

    #[test]
    fn checkbox_char_index_is_none_without_a_checkbox_or_without_a_list_marker_at_all() {
        assert!(checkbox_char_index(&buf("- plain item"), 0).is_none());
        assert!(checkbox_char_index(&buf("just text with [ ] in it"), 0).is_none());
    }

    #[test]
    fn toggle_checkbox_flips_unchecked_to_checked_and_back() {
        let mut b = buf("- [ ] todo");
        let mut c = Cursor::at_start();
        assert!(toggle_checkbox(&mut b, &mut c, 0));
        assert_eq!(b.text(), "- [x] todo");
        assert!(toggle_checkbox(&mut b, &mut c, 0));
        assert_eq!(b.text(), "- [ ] todo");
    }

    #[test]
    fn toggle_checkbox_on_an_uppercase_checked_box_unchecks_it() {
        let mut b = buf("- [X] todo");
        let mut c = Cursor::at_start();
        assert!(toggle_checkbox(&mut b, &mut c, 0));
        assert_eq!(b.text(), "- [ ] todo");
    }

    #[test]
    fn toggle_checkbox_on_a_line_with_no_checkbox_is_a_no_op() {
        let mut b = buf("- plain item");
        let mut c = Cursor::at_start();
        assert!(!toggle_checkbox(&mut b, &mut c, 0));
        assert_eq!(b.text(), "- plain item");
    }

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
