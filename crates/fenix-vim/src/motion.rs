use fenix_core::{Buffer, Cursor};

use crate::charclass::{classify, CharClass};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Motion {
    Left,
    Right,
    Up,
    Down,
    WordForward,
    WordBackward,
    WordEndForward,
    /// WORD motions: whitespace-only boundaries, no word/punctuation
    /// split, so "foo.bar" is one WORD instead of three.
    BigWordForward,
    BigWordBackward,
    BigWordEndForward,
    LineStart,
    LineFirstNonBlank,
    LineEnd,
    BufferTop,
    BufferBottom,
}

/// Whether composing this motion with an operator includes the char it
/// lands on (inclusive, e.g. `$`/`e`) or stops just before it (exclusive,
/// e.g. `w`/`0`). Doesn't apply to linewise motions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Inclusivity {
    Inclusive,
    Exclusive,
}

impl Motion {
    pub fn inclusivity(self) -> Inclusivity {
        match self {
            Motion::WordEndForward | Motion::BigWordEndForward | Motion::LineEnd => Inclusivity::Inclusive,
            _ => Inclusivity::Exclusive,
        }
    }

    /// Whether this motion composes with an operator over whole lines
    /// (`dG`, `dgg`) rather than a char range.
    pub fn is_linewise(self) -> bool {
        matches!(self, Motion::BufferTop | Motion::BufferBottom)
    }
}

/// Where `motion` would move the cursor, following Normal-mode Vim
/// conventions: unlike Insert mode, the cursor never rests past a
/// non-empty line's last character. Pure -- doesn't mutate `cursor`;
/// callers apply the result and update `sticky_col` themselves (vertical
/// motions want it preserved across the call, horizontal ones don't).
pub fn target(buffer: &Buffer, cursor: &Cursor, motion: Motion) -> usize {
    match motion {
        Motion::Left => {
            let (line, col) = buffer.line_col(cursor);
            if col > 0 {
                cursor.char_idx - 1
            } else {
                buffer.line_start_char(line)
            }
        }
        Motion::Right => {
            let (line, col) = buffer.line_col(cursor);
            let last_col = buffer.line_len(line).saturating_sub(1);
            if col < last_col {
                cursor.char_idx + 1
            } else {
                cursor.char_idx
            }
        }
        Motion::Up => vertical(buffer, cursor, -1),
        Motion::Down => vertical(buffer, cursor, 1),
        Motion::WordForward => word_forward(buffer, cursor.char_idx, classify),
        Motion::WordBackward => word_backward(buffer, cursor.char_idx, classify),
        Motion::WordEndForward => word_end_forward(buffer, cursor.char_idx, classify),
        Motion::BigWordForward => word_forward(buffer, cursor.char_idx, classify_big),
        Motion::BigWordBackward => word_backward(buffer, cursor.char_idx, classify_big),
        Motion::BigWordEndForward => word_end_forward(buffer, cursor.char_idx, classify_big),
        Motion::LineStart => {
            let (line, _) = buffer.line_col(cursor);
            buffer.line_start_char(line)
        }
        Motion::LineFirstNonBlank => {
            let (line, _) = buffer.line_col(cursor);
            line_first_non_blank(buffer, line)
        }
        Motion::LineEnd => {
            let (line, _) = buffer.line_col(cursor);
            buffer.line_start_char(line) + buffer.line_len(line).saturating_sub(1)
        }
        Motion::BufferTop => line_first_non_blank(buffer, 0),
        Motion::BufferBottom => line_first_non_blank(buffer, last_line(buffer)),
    }
}

/// True if the char at `idx` is a word or punctuation char (not
/// whitespace, and not past the end of the buffer). Used for the
/// `cw`-behaves-like-`ce` special case.
pub fn is_non_blank_at(buffer: &Buffer, idx: usize) -> bool {
    matches!(buffer.char_at(idx).map(classify), Some(CharClass::Word) | Some(CharClass::Punct))
}

/// WORD boundaries: whitespace vs. everything else, no word/punctuation
/// split -- used by `W`/`B`/`E`.
fn classify_big(c: char) -> CharClass {
    if c.is_whitespace() {
        CharClass::Space
    } else {
        CharClass::Word
    }
}

/// Ropey counts a phantom empty final line after a trailing `\n` (a file
/// "a\nb\n" reports 3 lines: "a\n", "b\n", ""). Vim's `G`/`j` shouldn't be
/// able to land the cursor there -- it isn't a line the file actually has.
pub(crate) fn last_line(buffer: &Buffer) -> usize {
    let last = buffer.line_count().saturating_sub(1);
    if last > 0 && buffer.line_len(last) == 0 {
        last - 1
    } else {
        last
    }
}

fn vertical(buffer: &Buffer, cursor: &Cursor, delta: isize) -> usize {
    let (line, _) = buffer.line_col(cursor);
    let last_line = last_line(buffer);
    let target_line = (line as isize + delta).clamp(0, last_line as isize) as usize;
    let max_col = buffer.line_len(target_line).saturating_sub(1);
    let col = cursor.sticky_col.min(max_col);
    buffer.line_start_char(target_line) + col
}

fn line_first_non_blank(buffer: &Buffer, line: usize) -> usize {
    let start = buffer.line_start_char(line);
    let end = start + buffer.line_len(line);
    let mut i = start;
    while i < end {
        match buffer.char_at(i) {
            Some(c) if classify(c) != CharClass::Space => return i,
            _ => i += 1,
        }
    }
    start
}

fn word_forward(buffer: &Buffer, idx: usize, classify: fn(char) -> CharClass) -> usize {
    let len = buffer.len_chars();
    let mut i = idx;
    if i >= len {
        return len;
    }
    let start_class = classify(buffer.char_at(i).unwrap());
    if start_class != CharClass::Space {
        while i < len && classify(buffer.char_at(i).unwrap()) == start_class {
            i += 1;
        }
    }
    while i < len && classify(buffer.char_at(i).unwrap()) == CharClass::Space {
        i += 1;
    }
    i
}

fn word_end_forward(buffer: &Buffer, idx: usize, classify: fn(char) -> CharClass) -> usize {
    let len = buffer.len_chars();
    if len == 0 {
        return 0;
    }
    let mut i = (idx + 1).min(len);
    while i < len && classify(buffer.char_at(i).unwrap()) == CharClass::Space {
        i += 1;
    }
    if i >= len {
        return len - 1;
    }
    let class = classify(buffer.char_at(i).unwrap());
    while i + 1 < len && classify(buffer.char_at(i + 1).unwrap()) == class {
        i += 1;
    }
    i
}

fn word_backward(buffer: &Buffer, idx: usize, classify: fn(char) -> CharClass) -> usize {
    if idx == 0 {
        return 0;
    }
    let mut i = idx - 1;
    while i > 0 && classify(buffer.char_at(i).unwrap()) == CharClass::Space {
        i -= 1;
    }
    if classify(buffer.char_at(i).unwrap()) == CharClass::Space {
        return 0;
    }
    let class = classify(buffer.char_at(i).unwrap());
    while i > 0 && classify(buffer.char_at(i - 1).unwrap()) == class {
        i -= 1;
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{buf, cur};

    #[test]
    fn left_right_stop_at_line_boundaries_not_wrap() {
        let b = buf("ab\ncd");
        assert_eq!(target(&b, &cur(0), Motion::Left), 0);
        assert_eq!(target(&b, &cur(1), Motion::Right), 1); // already on last char of "ab"
        assert_eq!(target(&b, &cur(3), Motion::Left), 3); // start of "cd", can't cross to "ab"
    }

    #[test]
    fn word_forward_skips_current_word_and_following_space() {
        let b = buf("foo bar baz");
        assert_eq!(target(&b, &cur(0), Motion::WordForward), 4);
        assert_eq!(target(&b, &cur(4), Motion::WordForward), 8);
    }

    #[test]
    fn word_forward_treats_punctuation_as_its_own_word() {
        let b = buf("foo.bar");
        assert_eq!(target(&b, &cur(0), Motion::WordForward), 3); // start of "."
        assert_eq!(target(&b, &cur(3), Motion::WordForward), 4); // start of "bar"
    }

    #[test]
    fn word_end_forward_advances_at_least_one_char() {
        let b = buf("foo bar");
        assert_eq!(target(&b, &cur(0), Motion::WordEndForward), 2); // end of "foo"
        assert_eq!(target(&b, &cur(2), Motion::WordEndForward), 6); // jumps to end of "bar"
    }

    #[test]
    fn word_backward_finds_previous_word_start() {
        let b = buf("foo bar baz");
        assert_eq!(target(&b, &cur(8), Motion::WordBackward), 4);
        assert_eq!(target(&b, &cur(4), Motion::WordBackward), 0);
    }

    #[test]
    fn line_start_first_non_blank_and_end() {
        let b = buf("  hi there\n");
        assert_eq!(target(&b, &cur(5), Motion::LineStart), 0);
        assert_eq!(target(&b, &cur(5), Motion::LineFirstNonBlank), 2);
        assert_eq!(target(&b, &cur(0), Motion::LineEnd), 9); // lands ON 'e' of "there"
    }

    #[test]
    fn buffer_top_and_bottom_land_on_first_non_blank() {
        let b = buf("a\n  b\nc\n");
        assert_eq!(target(&b, &cur(0), Motion::BufferTop), 0);
        assert_eq!(target(&b, &cur(0), Motion::BufferBottom), 6);
    }

    #[test]
    fn vertical_motion_uses_sticky_column_clamped_to_short_lines() {
        let b = buf("longline\nhi\nlongline");
        let c = Cursor { char_idx: 8, sticky_col: 8 }; // end of "longline"
        let after_down = target(&b, &c, Motion::Down);
        assert_eq!(b.line_col(&Cursor { char_idx: after_down, sticky_col: 0 }), (1, 1)); // clamped onto "hi"
    }

    #[test]
    fn big_word_forward_ignores_punctuation_boundaries() {
        let b = buf("foo.bar baz");
        // unlike WordForward, "foo.bar" is one WORD, not three
        assert_eq!(target(&b, &cur(0), Motion::BigWordForward), 8);
    }

    #[test]
    fn big_word_end_and_backward_ignore_punctuation_too() {
        let b = buf("foo.bar baz");
        assert_eq!(target(&b, &cur(0), Motion::BigWordEndForward), 6); // end of "foo.bar"
        assert_eq!(target(&b, &cur(8), Motion::BigWordBackward), 0);
    }

    #[test]
    fn is_non_blank_at_distinguishes_space_from_word() {
        let b = buf("a b");
        assert!(is_non_blank_at(&b, 0));
        assert!(!is_non_blank_at(&b, 1));
        assert!(is_non_blank_at(&b, 2));
    }
}
