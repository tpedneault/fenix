//! What to replace, and with what, to turn one text into another, line
//! by line: the least an editor needs to change in a buffer for a
//! formatter's or a language server's new text, so what's anchored in
//! the lines that didn't change (cursors, marks, decorations) stays put.

use std::ops::Range;
use std::time::{Duration, Instant};

use similar::{Algorithm, ChangeTag, TextDiff};

/// At most this many replacements: past it, one -- from the first changed
/// line to the last -- is cheaper to apply and to follow than many.
pub const MAX_REPLACEMENTS: usize = 500;

/// How long the line diff may take before settling for a coarser (still
/// right) answer: a whole file changed, every line, must not stop the
/// editor.
const DIFF_DEADLINE: Duration = Duration::from_millis(40);

/// The replacements that turn `old` into `new`: each a char range of
/// `old` and the text to put there, in order and not overlapping.
/// Applied from the last to the first, each range is still where it was.
/// Never more than [`MAX_REPLACEMENTS`]; what takes longer than a moment
/// to work out, or would be more, is one replacement of the lines
/// between the first and the last that differ.
pub fn replacements(old: &str, new: &str) -> Vec<(Range<usize>, String)> {
    let out = by_lines(old, new);
    if out.len() <= MAX_REPLACEMENTS {
        return out;
    }
    let (start, end) = (out[0].0.start, out[out.len() - 1].0.end);
    vec![(start..end, one_span(old, new, start, end))]
}

/// The text of `new` in place of `old`'s `start..end`, given that
/// everything before `start` and after `end` is the same in both.
fn one_span(old: &str, new: &str, start: usize, end: usize) -> String {
    let after = old.chars().count() - end;
    let new_len = new.chars().count();
    new.chars().skip(start).take(new_len - after - start).collect()
}

fn by_lines(old: &str, new: &str) -> Vec<(Range<usize>, String)> {
    let diff = TextDiff::configure().algorithm(Algorithm::Myers).deadline(Instant::now() + DIFF_DEADLINE).diff_lines(old, new);
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

    #[test]
    fn past_the_cap_it_is_one_replacement_from_the_first_change_to_the_last() {
        let old: String = (0..3000).map(|i| format!("line {i}\n")).collect();
        let new: String = (0..3000).map(|i| if i % 2 == 1 { format!("LINE {i}\n") } else { format!("line {i}\n") }).collect();
        let reps = replacements(&old, &new);
        assert_eq!(reps.len(), 1, "1500 hunks are one");
        assert_eq!(reps[0].0.start, "line 0\n".len(), "from the first line that differs");
        assert_eq!(apply(&old, &reps), new);
    }

    #[test]
    fn a_large_file_with_every_line_changed_is_quick_and_right() {
        let old: String = (0..100_000).map(|i| format!("{i}\n")).collect();
        let new = old.replace('\n', "\r\n");
        let started = Instant::now();
        let reps = replacements(&old, &new);
        assert!(started.elapsed() < Duration::from_secs(5), "{:?}", started.elapsed());
        assert!(reps.len() <= MAX_REPLACEMENTS);
        assert_eq!(apply(&old, &reps), new);
    }
}
