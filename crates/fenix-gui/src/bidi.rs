//! Right-to-left text in a left-to-right editor (#17).
//!
//! Fenix lays every row out left to right: the gutter, then the text, in
//! cells. cosmic-text gives each line the direction of its first strong
//! character, so a row whose text starts with Hebrew or Arabic was a
//! right-to-left paragraph -- right-aligned past the pane's edge, its
//! line number reordered along with it. Every line Fenix shapes now
//! starts with a LEFT-TO-RIGHT MARK (`ltr_lines`), so it stays a
//! left-to-right paragraph; right-to-left runs within it are still
//! reordered and shaped as they should be, and read correctly.
//!
//! The caret, the selection and the search's matches are worked out in
//! logical cells (the Nth character is cell N); `visual_order` says where
//! each character of a row is drawn once its runs are reordered, so they
//! can be moved onto what's drawn. Only what's needed for that is here:
//! the logical order is still what motions, edits and columns go by.

use std::ops::Range;

use unicode_bidi::{bidi_class, BidiClass, BidiInfo, Level};

/// LEFT-TO-RIGHT MARK: invisible, and makes a paragraph it starts left to
/// right.
pub const LRM: &str = "\u{200E}";

/// Whether `text` has a character that's written right to left (Hebrew,
/// Arabic, ...) or an explicit right-to-left control: the only text whose
/// layout differs from its logical order.
pub fn has_rtl(text: &str) -> bool {
    !text.is_ascii() && text.chars().any(is_rtl_char)
}

fn is_rtl_char(c: char) -> bool {
    !c.is_ascii() && matches!(bidi_class(c), BidiClass::R | BidiClass::AL | BidiClass::RLE | BidiClass::RLO | BidiClass::RLI)
}

/// The visual position of each character of `text`, laid out as a
/// left-to-right paragraph: `order[i]` is the cell character `i` is drawn
/// in (a character a cell), and `rtl[i]` whether it's in a right-to-left
/// run. `None` for text with nothing right to left, which is drawn in its
/// own order.
pub fn visual_order(text: &str) -> Option<VisualOrder> {
    if !has_rtl(text) {
        return None;
    }
    let info = BidiInfo::new(text, Some(Level::ltr()));
    let para = info.paragraphs.first()?;
    // Levels by byte, as unicode-bidi gives them; one for each character.
    let levels = info.reordered_levels(para, para.range.clone());
    let starts: Vec<usize> = text.char_indices().map(|(i, _)| i).collect();
    let char_levels: Vec<Level> = starts.iter().map(|&i| levels[i]).collect();
    // Logical indices in visual order.
    let visual = BidiInfo::reorder_visual(&char_levels);
    let mut order = vec![0; starts.len()];
    for (cell, &logical) in visual.iter().enumerate() {
        order[logical] = cell;
    }
    let rtl = char_levels.iter().map(|l| l.is_rtl()).collect();
    Some(VisualOrder { order, rtl })
}

/// `visual_order`'s answer.
#[derive(Debug, Clone, PartialEq)]
pub struct VisualOrder {
    pub order: Vec<usize>,
    pub rtl: Vec<bool>,
}

impl VisualOrder {
    pub fn len(&self) -> usize {
        self.order.len()
    }

    /// Where a caret before character `i` is drawn: on that character's
    /// cell for a block (`block`), and for a bar at the edge of it that
    /// comes first in reading order -- its right edge in a right-to-left
    /// run. Past the end, where it was.
    pub fn caret(&self, i: usize, block: bool) -> usize {
        match self.order.get(i) {
            Some(&cell) if !block && self.rtl[i] => cell + 1,
            Some(&cell) => cell,
            None => i,
        }
    }

    /// The cells characters `range` are drawn in, as the fewest runs of
    /// cells, left to right. Past the end, cells are where they were.
    pub fn cells(&self, range: Range<usize>) -> Vec<Range<usize>> {
        let n = self.len();
        let mut cells: Vec<usize> = (range.start.min(n)..range.end.min(n)).map(|i| self.order[i]).collect();
        cells.sort_unstable();
        let mut out: Vec<Range<usize>> = Vec::new();
        for c in cells {
            match out.last_mut() {
                Some(last) if last.end == c => last.end = c + 1,
                _ => out.push(c..c + 1),
            }
        }
        if range.end > n {
            let tail = range.start.max(n)..range.end;
            match out.last_mut() {
                Some(last) if last.end == tail.start => last.end = tail.end,
                _ => out.push(tail),
            }
        }
        out
    }

    /// The character drawn in cell `cell`: where a click there lands.
    pub fn char_at(&self, cell: usize) -> usize {
        self.order.iter().position(|&c| c == cell).unwrap_or(cell)
    }
}

/// `spans`, each line (each run of text up to a `\n`) starting with a
/// left-to-right mark in `plain` attributes -- so cosmic-text lays it out
/// left to right whatever its first letter -- when any of it is right to
/// left; otherwise as they are.
pub fn ltr_lines<'a, A: Clone>(spans: Vec<(&'a str, A)>, plain: &A) -> Vec<(&'a str, A)> {
    if !spans.iter().any(|(text, _)| has_rtl(text)) {
        return spans;
    }
    let mut out = Vec::with_capacity(spans.len() + 8);
    let mut line_start = true;
    for (text, attrs) in spans {
        let mut rest = text;
        while !rest.is_empty() {
            if line_start {
                out.push((LRM, plain.clone()));
                line_start = false;
            }
            match rest.find('\n') {
                Some(i) => {
                    out.push((&rest[..=i], attrs.clone()));
                    rest = &rest[i + 1..];
                    line_start = true;
                }
                None => {
                    out.push((rest, attrs.clone()));
                    rest = "";
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_text_with_something_right_to_left_is_reordered() {
        assert!(!has_rtl("plain ascii"));
        assert!(!has_rtl("日本語 😀 é"));
        assert!(has_rtl("עברית RTL"));
        assert!(has_rtl("x = \"مرحبا\""));
        assert_eq!(visual_order("日本語"), None);
    }

    #[test]
    fn a_right_to_left_run_is_drawn_reversed_where_it_starts() {
        // "אבג x": the Hebrew is drawn ג ב א, then " x".
        let v = visual_order("אבג x").unwrap();
        assert_eq!(v.order, vec![2, 1, 0, 3, 4]);
        assert_eq!(v.rtl, vec![true, true, true, false, false]);
        // Mid-line: "x אבג y".
        let v = visual_order("x אבג y").unwrap();
        assert_eq!(v.order, vec![0, 1, 4, 3, 2, 5, 6]);
    }

    #[test]
    fn numbers_in_right_to_left_text_keep_their_own_order() {
        // "אב 12": the paragraph is left to right, the run "אב 12" is
        // right to left with the number in it left to right: 12 בא.
        let v = visual_order("אב 12").unwrap();
        assert_eq!(v.order, vec![4, 3, 2, 0, 1]);
    }

    #[test]
    fn a_caret_lands_on_the_character_it_s_on() {
        let v = visual_order("אבג x").unwrap();
        // A block on א is on the cell א is drawn in.
        assert_eq!(v.caret(0, true), 2);
        // A bar before א is at its right edge, where you'd type next to it.
        assert_eq!(v.caret(0, false), 3);
        assert_eq!(v.caret(4, false), 4);
        // Past the end, as it was.
        assert_eq!(v.caret(5, false), 5);
        assert_eq!(v.char_at(2), 0);
        assert_eq!(v.char_at(9), 9);
    }

    #[test]
    fn a_range_s_cells_are_the_runs_its_characters_are_drawn_in() {
        let v = visual_order("x אבג y").unwrap();
        // "x א": cell 0 and א's, cell 4 -- with the space, 0..2 and 4..5.
        assert_eq!(v.cells(0..3), vec![0..2, 4..5]);
        // The whole Hebrew word is one run.
        assert_eq!(v.cells(2..5), vec![2..5]);
        // On past the line's end, as it was.
        assert_eq!(v.cells(5..9), vec![5..9]);
        assert_eq!(v.cells(3..3), Vec::<Range<usize>>::new());
    }

    #[test]
    fn every_line_of_right_to_left_text_starts_left_to_right() {
        let spans = vec![("  1 עברית\n  2 ", 'a'), ("x\n", 'b'), ("  3 ok", 'c')];
        let out = ltr_lines(spans, &'-');
        assert_eq!(out, vec![(LRM, '-'), ("  1 עברית\n", 'a'), (LRM, '-'), ("  2 ", 'a'), ("x\n", 'b'), (LRM, '-'), ("  3 ok", 'c')]);
        // Nothing right to left: as it was.
        let plain = vec![("  1 abc\n", 'a'), ("  2 日本", 'b')];
        assert_eq!(ltr_lines(plain.clone(), &'-'), plain);
    }
}
