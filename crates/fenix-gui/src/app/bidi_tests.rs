//! Right-to-left text in a pane (#17): the caret, the selection and a
//! click land on the characters as they're drawn, the runs reordered.
//! (`bidi` has the reordering; `text` that the line is laid out left to
//! right.)

use super::test_keys::app_on_file;

#[test]
fn the_caret_is_on_the_hebrew_letter_it_is_on() {
    // "עברית RTL": the word is drawn reversed in cells 0 to 4, ע in 4.
    let (mut app, _f) = app_on_file("he.txt", "עברית RTL\n");
    assert_eq!(app.focused_overlays_now(5).caret, Some((0, 4)), "a block on ע, drawn rightmost");
    app.press("l");
    assert_eq!(app.focused_overlays_now(5).caret, Some((0, 3)), "ב, left of it");
    // Past the word, the Latin text is where it always was.
    app.press_all("w");
    assert_eq!(app.line_col(), (0, 6));
    assert_eq!(app.focused_overlays_now(5).caret, Some((0, 6)));
}

#[test]
fn a_bar_caret_in_right_to_left_text_is_at_the_letter_s_right_edge() {
    let (mut app, _f) = app_on_file("he.txt", "עברית RTL\n");
    app.press("i");
    // Before ע: at its right edge, cell 5.
    assert_eq!(app.focused_overlays_now(5).caret, Some((0, 5)));
}

#[test]
fn a_line_with_nothing_right_to_left_is_as_it_was() {
    let (mut app, _f) = app_on_file("a.txt", "plain 日本 text\n");
    app.press_all("w");
    assert_eq!(app.focused_overlays_now(5).caret, Some((0, 6)));
}

#[test]
fn a_selection_in_mixed_text_covers_what_it_selects_as_drawn() {
    // "ab אבג cd": the Hebrew is cells 3 to 5, drawn ג ב א.
    let (mut app, _f) = app_on_file("mix.txt", "ab אבג cd\n");
    // From `b` to א: `b`, the space, and א (drawn at cell 5).
    app.press_all("l v l l");
    let selection = app.focused_overlays_now(5).selection;
    assert_eq!(selection, vec![(0, 1, 3), (0, 5, 6)], "{selection:?}");
}
