//! What to replace, and with what, to turn one text into another, line
//! by line: the least an editor needs to change in a buffer for a
//! formatter's or a language server's new text, so what's anchored in
//! the lines that didn't change (cursors, marks, decorations) stays put.

use std::ops::Range;

use similar::{Algorithm, ChangeTag, TextDiff};

/// The replacements that turn `old` into `new`: each a char range of
/// `old` and the text to put there, in order and not overlapping.
/// Applied from the last to the first, each range is still where it was.
pub fn replacements(old: &str, new: &str) -> Vec<(Range<usize>, String)> {
    let diff = TextDiff::configure().algorithm(Algorithm::Myers).diff_lines(old, new);
    let mut out: Vec<(Range<usize>, String)> = Vec::new();
    let mut at = 0usize;
    // The replacement being gathered: where it started in `old`, how far
    // it's reached, and what goes there.
    let mut open: Option<(usize, usize, String)> = None;
    for change in diff.iter_all_changes() {
        let len = change.value().chars().count();
        match change.tag() {
            ChangeTag::Equal => {
                if let Some((start, end, text)) = open.take() {
                    out.push((start..end, text));
                }
                at += len;
            }
            ChangeTag::Delete => {
                let entry = open.get_or_insert_with(|| (at, at, String::new()));
                at += len;
                entry.1 = at;
            }
            ChangeTag::Insert => {
                let entry = open.get_or_insert_with(|| (at, at, String::new()));
                entry.2.push_str(change.value());
            }
        }
    }
    if let Some((start, end, text)) = open {
        out.push((start..end, text));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `old` with `replacements` applied, last first.
    fn apply(old: &str, reps: &[(Range<usize>, String)]) -> String {
        let mut chars: Vec<char> = old.chars().collect();
        for (range, text) in reps.iter().rev() {
            chars.splice(range.clone(), text.chars());
        }
        chars.into_iter().collect()
    }

    #[test]
    fn only_the_lines_that_changed_are_replaced() {
        let old = "a\nb\nc\nd\ne\n";
        let new = "a\nB\nc\nd\nE\n";
        let reps = replacements(old, new);
        assert_eq!(reps, [(2..4, "B\n".to_string()), (8..10, "E\n".to_string())], "two hunks, apart");
        assert_eq!(apply(old, &reps), new);
    }

    #[test]
    fn insertions_deletions_multi_byte_and_no_final_newline_come_out_right() {
        for (old, new) in [("", "x"), ("x", ""), ("été\nb", "été\nbé"), ("a\nb\n", "a\nx\ny\nb\n"), ("a\nb\nc", "a\nc"), ("same\n", "same\n")] {
            assert_eq!(apply(old, &replacements(old, new)), new, "{old:?} -> {new:?}");
        }
        assert!(replacements("same\n", "same\n").is_empty());
    }
}
