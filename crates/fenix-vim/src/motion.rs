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
    /// `f{c}`: next occurrence of `c` on the current line, cursor lands
    /// on it. Never crosses to another line.
    FindChar(char),
    /// `F{c}`: previous occurrence of `c` on the current line.
    FindCharBack(char),
    /// `t{c}`: stops just *before* the next occurrence of `c`.
    TillChar(char),
    /// `T{c}`: stops just *after* the previous occurrence of `c`.
    TillCharBack(char),
    /// `%`: the bracket matching the one under the cursor, via
    /// `bracket::find_match` -- a no-op (stays put) off a bracket or with
    /// no match, matching real Vim.
    MatchingBracket,
    /// `}`: the next blank line (paragraph boundary), or the end of the
    /// buffer if there isn't one.
    ParagraphForward,
    /// `{`: the previous blank line, or the start of the buffer.
    ParagraphBackward,
    /// `gj`: a screen row down -- in a pane whose lines wrap, the line's
    /// next row; without wrap, the next line.
    ScreenDown,
    /// `gk`: a screen row up.
    ScreenUp,
    /// `g_`: the last non-blank of the line (with a count, `count - 1`
    /// lines down).
    LineLastNonBlank,
    /// `ge`: back to the end of the previous word.
    WordEndBackward,
    /// `gE`: back to the end of the previous WORD.
    BigWordEndBackward,
    /// `|`: to the screen column the count names (1 without one).
    Column,
    /// `+` / Enter: the first non-blank of the next line.
    NextLineStart,
    /// `-`: the first non-blank of the line before.
    PrevLineStart,
    /// `_`: the first non-blank of the line `count - 1` lines down.
    LineDownFirstNonBlank,
    /// `)`: the start of the next sentence.
    SentenceForward,
    /// `(`: the start of this sentence, or the one before.
    SentenceBackward,
    /// `H`: the window's top line (with a count, that many lines from it).
    WindowTop,
    /// `M`: the window's middle line.
    WindowMiddle,
    /// `L`: the window's bottom line (with a count, that many from it).
    WindowBottom,
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
            Motion::WordEndForward
            | Motion::BigWordEndForward
            | Motion::LineEnd
            | Motion::FindChar(_)
            | Motion::TillChar(_)
            | Motion::MatchingBracket
            | Motion::LineLastNonBlank
            | Motion::WordEndBackward
            | Motion::BigWordEndBackward => Inclusivity::Inclusive,
            // `FindCharBack`/`TillCharBack` land at or before the found
            // char with the cursor itself past it, so the plain
            // start..cursor range (what Exclusive already gives every
            // backward motion, e.g. `WordBackward`) is already correct
            // without extending it -- see `motion.rs`'s module-level
            // reasoning in the `f`/`F`/`t`/`T` implementation notes.
            _ => Inclusivity::Exclusive,
        }
    }

    /// Whether this motion composes with an operator over whole lines
    /// (`dG`, `dgg`, `dj`, `dL`) rather than a char range. `gj`/`gk` are
    /// charwise, as in Vim.
    pub fn is_linewise(self) -> bool {
        matches!(
            self,
            Motion::BufferTop
                | Motion::BufferBottom
                | Motion::Up
                | Motion::Down
                | Motion::NextLineStart
                | Motion::PrevLineStart
                | Motion::LineDownFirstNonBlank
                | Motion::WindowTop
                | Motion::WindowMiddle
                | Motion::WindowBottom
        )
    }

    /// Whether Vim counts this motion as a jump (`:h jump-motions`): the
    /// place it left goes on the jumplist.
    pub fn is_jump(self) -> bool {
        matches!(
            self,
            Motion::BufferTop
                | Motion::BufferBottom
                | Motion::MatchingBracket
                | Motion::SentenceForward
                | Motion::SentenceBackward
                | Motion::WindowTop
                | Motion::WindowMiddle
                | Motion::WindowBottom
        )
    }
}

/// Where `motion` would move the cursor, following Normal-mode Vim
/// conventions: unlike Insert mode, the cursor never rests past a
/// non-empty line's last character. Pure -- doesn't mutate `cursor`;
/// callers apply the result and update `sticky_col` themselves (vertical
/// motions want it preserved across the call, horizontal ones don't).
pub fn target(buffer: &Buffer, cursor: &Cursor, motion: Motion, iskeyword_extra: &[char]) -> usize {
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
        Motion::Up | Motion::ScreenUp => vertical(buffer, cursor, -1),
        Motion::Down | Motion::ScreenDown => vertical(buffer, cursor, 1),
        Motion::WordForward => word_forward(buffer, cursor.char_idx, |c| classify(c, iskeyword_extra)),
        Motion::WordBackward => word_backward(buffer, cursor.char_idx, |c| classify(c, iskeyword_extra)),
        Motion::WordEndForward => word_end_forward(buffer, cursor.char_idx, |c| classify(c, iskeyword_extra)),
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
        Motion::FindChar(c) => find_char_forward(buffer, cursor, c).unwrap_or(cursor.char_idx),
        Motion::FindCharBack(c) => find_char_backward(buffer, cursor, c).unwrap_or(cursor.char_idx),
        // `- 1`/`+ 1` are always in-bounds when a match was found: a
        // forward match is strictly past the cursor (so >= line_start +
        // 1), a backward one strictly before it.
        Motion::TillChar(c) => find_char_forward(buffer, cursor, c).map(|i| i - 1).unwrap_or(cursor.char_idx),
        Motion::TillCharBack(c) => find_char_backward(buffer, cursor, c).map(|i| i + 1).unwrap_or(cursor.char_idx),
        Motion::MatchingBracket => crate::bracket::find_match(buffer, cursor.char_idx).unwrap_or(cursor.char_idx),
        Motion::ParagraphForward => paragraph_forward(buffer, cursor),
        Motion::ParagraphBackward => paragraph_backward(buffer, cursor),
        Motion::WordEndBackward => word_end_backward(buffer, cursor.char_idx, |c| classify(c, iskeyword_extra)),
        Motion::BigWordEndBackward => word_end_backward(buffer, cursor.char_idx, classify_big),
        Motion::NextLineStart => {
            let (line, _) = buffer.line_col(cursor);
            if line >= last_line(buffer) {
                cursor.char_idx
            } else {
                line_first_non_blank(buffer, line + 1)
            }
        }
        Motion::PrevLineStart => {
            let (line, _) = buffer.line_col(cursor);
            if line == 0 {
                cursor.char_idx
            } else {
                line_first_non_blank(buffer, line - 1)
            }
        }
        Motion::SentenceForward => sentence_forward(buffer, cursor.char_idx),
        Motion::SentenceBackward => sentence_backward(buffer, cursor.char_idx),
        // The motions a count doesn't repeat: `counted_target` has them.
        Motion::LineLastNonBlank | Motion::Column | Motion::LineDownFirstNonBlank | Motion::WindowTop | Motion::WindowMiddle | Motion::WindowBottom => {
            counted_target(buffer, cursor, motion, 1, None).unwrap_or(cursor.char_idx)
        }
    }
}

/// Where a motion whose count isn't a repeat lands: `|` (the count is a
/// column), `g_` and `_` (`count - 1` lines down), `H`/`L` (`count`
/// lines in from the window's edge) and `M`. `viewport` is the window's
/// first and last line, `None` for the whole buffer. `None` for any
/// other motion.
pub fn counted_target(buffer: &Buffer, cursor: &Cursor, motion: Motion, count: u32, viewport: Option<(usize, usize)>) -> Option<usize> {
    let count = count.max(1) as usize;
    let (line, _) = buffer.line_col(cursor);
    let last = last_line(buffer);
    let (top, bottom) = viewport.map_or((0, last), |(top, bottom)| (top.min(last), bottom.min(last).max(top.min(last))));
    Some(match motion {
        Motion::Column => buffer.line_start_char(line) + (count - 1).min(buffer.line_len(line).saturating_sub(1)),
        Motion::LineLastNonBlank => line_last_non_blank(buffer, (line + count - 1).min(last)),
        Motion::LineDownFirstNonBlank => line_first_non_blank(buffer, (line + count - 1).min(last)),
        Motion::WindowTop => line_first_non_blank(buffer, (top + count - 1).min(bottom)),
        Motion::WindowBottom => line_first_non_blank(buffer, bottom.saturating_sub(count - 1).max(top)),
        Motion::WindowMiddle => line_first_non_blank(buffer, top + (bottom - top) / 2),
        _ => return None,
    })
}

/// The line's last non-blank char, or its start when it's all blank.
pub(crate) fn line_last_non_blank(buffer: &Buffer, line: usize) -> usize {
    let start = buffer.line_start_char(line);
    let mut i = start + buffer.line_len(line);
    while i > start {
        if buffer.char_at(i - 1).is_some_and(|c| !c.is_whitespace()) {
            return i - 1;
        }
        i -= 1;
    }
    start
}

/// Whether `i` is on an empty line (its own `\n`, nothing before it).
fn empty_line_at(buffer: &Buffer, i: usize) -> bool {
    buffer.char_at(i) == Some('\n') && (i == 0 || buffer.char_at(i - 1) == Some('\n'))
}

/// Where a sentence starts, as Vim reads one: after a `.`, `!` or `?`
/// (closed by any `)`, `]`, `"` or `'`) and whitespace; the first
/// non-blank after an empty line, and the first empty line of a run;
/// and the buffer's first non-blank.
fn is_sentence_start(buffer: &Buffer, i: usize) -> bool {
    let Some(c) = buffer.char_at(i) else { return false };
    if c == '\n' {
        return empty_line_at(buffer, i) && !(i > 0 && empty_line_at(buffer, i - 1));
    }
    if c.is_whitespace() {
        return false;
    }
    let mut j = i;
    let mut blank_before = false;
    while j > 0 && buffer.char_at(j - 1).is_some_and(char::is_whitespace) {
        if empty_line_at(buffer, j - 1) {
            return true;
        }
        blank_before = true;
        j -= 1;
    }
    if j == 0 {
        return true;
    }
    if !blank_before {
        return false;
    }
    while j > 0 && matches!(buffer.char_at(j - 1), Some(')' | ']' | '"' | '\'')) {
        j -= 1;
    }
    j > 0 && matches!(buffer.char_at(j - 1), Some('.' | '!' | '?'))
}

/// `)`: the next sentence's start, or the buffer's last char.
fn sentence_forward(buffer: &Buffer, idx: usize) -> usize {
    let len = buffer.len_chars();
    (idx + 1..len).find(|&i| is_sentence_start(buffer, i)).unwrap_or(len.saturating_sub(1).max(idx.min(len)))
}

/// `(`: the start of the sentence the cursor is in (or, at one's start,
/// the one before), or the buffer's start.
fn sentence_backward(buffer: &Buffer, idx: usize) -> usize {
    (0..idx.min(buffer.len_chars())).rev().find(|&i| is_sentence_start(buffer, i)).unwrap_or(0)
}

/// `ge`/`gE`: back to the last char of the word before the one the
/// cursor is in -- an empty line counts as a word, as in Vim.
fn word_end_backward(buffer: &Buffer, idx: usize, classify: impl Fn(char) -> CharClass) -> usize {
    let len = buffer.len_chars();
    if idx == 0 || len == 0 {
        return 0;
    }
    let mut i = idx.min(len - 1);
    let class_at = |i: usize| buffer.char_at(i).map_or(CharClass::Space, &classify);
    // To the start of the word the cursor is in.
    let start = class_at(i);
    if start != CharClass::Space {
        while i > 0 && class_at(i - 1) == start {
            i -= 1;
        }
    }
    if i == 0 {
        return 0;
    }
    i -= 1;
    while class_at(i) == CharClass::Space {
        if empty_line_at(buffer, i) {
            return i;
        }
        if i == 0 {
            return 0;
        }
        i -= 1;
    }
    i
}

/// Char index of the next `target` char on the cursor's current line,
/// strictly after the cursor -- `f`/`t`'s shared scan, never crossing to
/// another line (real Vim's own `f`/`t` scope).
fn find_char_forward(buffer: &Buffer, cursor: &Cursor, target: char) -> Option<usize> {
    let (line, _) = buffer.line_col(cursor);
    let line_end = buffer.line_start_char(line) + buffer.line_len(line);
    let mut i = cursor.char_idx + 1;
    while i < line_end {
        if buffer.char_at(i) == Some(target) {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// Char index of the previous `target` char on the cursor's current
/// line, strictly before the cursor -- `F`/`T`'s shared scan.
fn find_char_backward(buffer: &Buffer, cursor: &Cursor, target: char) -> Option<usize> {
    let (line, _) = buffer.line_col(cursor);
    let line_start = buffer.line_start_char(line);
    let mut i = cursor.char_idx;
    while i > line_start {
        i -= 1;
        if buffer.char_at(i) == Some(target) {
            return Some(i);
        }
    }
    None
}

/// The start of the next blank line after the cursor's, or the end of
/// the buffer if there isn't one -- Vim's own definition of a paragraph
/// boundary (a fully empty line, not just visually short).
fn paragraph_forward(buffer: &Buffer, cursor: &Cursor) -> usize {
    let (line, _) = buffer.line_col(cursor);
    let last = last_line(buffer);
    let mut l = line;
    while l < last {
        l += 1;
        if buffer.line_len(l) == 0 {
            return buffer.line_start_char(l);
        }
    }
    buffer.line_start_char(last) + buffer.line_len(last)
}

/// The start of the previous blank line before the cursor's, or the
/// start of the buffer if there isn't one.
fn paragraph_backward(buffer: &Buffer, cursor: &Cursor) -> usize {
    let (line, _) = buffer.line_col(cursor);
    let mut l = line;
    while l > 0 {
        l -= 1;
        if buffer.line_len(l) == 0 {
            return buffer.line_start_char(l);
        }
    }
    0
}

/// True if the char at `idx` is a word or punctuation char (not
/// whitespace, and not past the end of the buffer). Used for the
/// `cw`-behaves-like-`ce` special case.
pub fn is_non_blank_at(buffer: &Buffer, idx: usize) -> bool {
    // "word or punctuation, i.e. not whitespace" doesn't depend on
    // which extra characters `'iskeyword'` treats as word-class --
    // `_` reads as `Word` under the default and `Punct` with `:set
    // iskeyword-=_`, but either way it's not `Space`, so this can
    // classify with no extra keyword characters at all.
    matches!(buffer.char_at(idx).map(|c| classify(c, &[])), Some(CharClass::Word) | Some(CharClass::Punct))
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

/// Where `{count}gg`/`{count}G` land: the first non-blank of absolute
/// line `count` (1-indexed, clamped to the buffer's real last line --
/// `500gg` in a 10-line file lands on line 10, not past the end) when a
/// count was actually typed, or `motion`'s own bottom/top-of-buffer
/// default otherwise. Takes `Option<u32>` rather than the already-
/// defaulted-to-1 `u32` every other motion's count uses, since `gg`
/// (no count) and `1gg` happen to coincide but bare `G` (last line) and
/// `1G` (line 1) very much don't -- `apply_motion`'s generic repeat-N-
/// times loop can't express "target is a line number, not a repeat
/// count" at all, which is why this is a separate function instead of
/// another case inside `target`.
pub fn buffer_line_target(buffer: &Buffer, motion: Motion, count: Option<u32>) -> usize {
    match count {
        Some(n) => {
            let line = (n.max(1) as usize - 1).min(last_line(buffer));
            line_first_non_blank(buffer, line)
        }
        None => match motion {
            Motion::BufferTop => line_first_non_blank(buffer, 0),
            Motion::BufferBottom => line_first_non_blank(buffer, last_line(buffer)),
            _ => unreachable!("buffer_line_target is only ever called with BufferTop/BufferBottom"),
        },
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

pub(crate) fn line_first_non_blank(buffer: &Buffer, line: usize) -> usize {
    let start = buffer.line_start_char(line);
    let end = start + buffer.line_len(line);
    let mut i = start;
    while i < end {
        match buffer.char_at(i) {
            // Same reasoning as `is_non_blank_at`: only "is this
            // whitespace" matters here, which `'iskeyword'` never
            // changes.
            Some(c) if classify(c, &[]) != CharClass::Space => return i,
            _ => i += 1,
        }
    }
    start
}

fn word_forward(buffer: &Buffer, idx: usize, classify: impl Fn(char) -> CharClass) -> usize {
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

fn word_end_forward(buffer: &Buffer, idx: usize, classify: impl Fn(char) -> CharClass) -> usize {
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

fn word_backward(buffer: &Buffer, idx: usize, classify: impl Fn(char) -> CharClass) -> usize {
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
    use crate::charclass::DEFAULT_ISKEYWORD_EXTRA;
    use crate::test_util::{buf, cur};

    #[test]
    fn left_right_stop_at_line_boundaries_not_wrap() {
        let b = buf("ab\ncd");
        assert_eq!(target(&b, &cur(0), Motion::Left, DEFAULT_ISKEYWORD_EXTRA), 0);
        assert_eq!(target(&b, &cur(1), Motion::Right, DEFAULT_ISKEYWORD_EXTRA), 1); // already on last char of "ab"
        assert_eq!(target(&b, &cur(3), Motion::Left, DEFAULT_ISKEYWORD_EXTRA), 3); // start of "cd", can't cross to "ab"
    }

    #[test]
    fn word_forward_skips_current_word_and_following_space() {
        let b = buf("foo bar baz");
        assert_eq!(target(&b, &cur(0), Motion::WordForward, DEFAULT_ISKEYWORD_EXTRA), 4);
        assert_eq!(target(&b, &cur(4), Motion::WordForward, DEFAULT_ISKEYWORD_EXTRA), 8);
    }

    #[test]
    fn word_forward_treats_punctuation_as_its_own_word() {
        let b = buf("foo.bar");
        assert_eq!(target(&b, &cur(0), Motion::WordForward, DEFAULT_ISKEYWORD_EXTRA), 3); // start of "."
        assert_eq!(target(&b, &cur(3), Motion::WordForward, DEFAULT_ISKEYWORD_EXTRA), 4); // start of "bar"
    }

    #[test]
    fn word_end_forward_advances_at_least_one_char() {
        let b = buf("foo bar");
        assert_eq!(target(&b, &cur(0), Motion::WordEndForward, DEFAULT_ISKEYWORD_EXTRA), 2); // end of "foo"
        assert_eq!(target(&b, &cur(2), Motion::WordEndForward, DEFAULT_ISKEYWORD_EXTRA), 6); // jumps to end of "bar"
    }

    #[test]
    fn word_backward_finds_previous_word_start() {
        let b = buf("foo bar baz");
        assert_eq!(target(&b, &cur(8), Motion::WordBackward, DEFAULT_ISKEYWORD_EXTRA), 4);
        assert_eq!(target(&b, &cur(4), Motion::WordBackward, DEFAULT_ISKEYWORD_EXTRA), 0);
    }

    #[test]
    fn line_start_first_non_blank_and_end() {
        let b = buf("  hi there\n");
        assert_eq!(target(&b, &cur(5), Motion::LineStart, DEFAULT_ISKEYWORD_EXTRA), 0);
        assert_eq!(target(&b, &cur(5), Motion::LineFirstNonBlank, DEFAULT_ISKEYWORD_EXTRA), 2);
        assert_eq!(target(&b, &cur(0), Motion::LineEnd, DEFAULT_ISKEYWORD_EXTRA), 9); // lands ON 'e' of "there"
    }

    #[test]
    fn buffer_top_and_bottom_land_on_first_non_blank() {
        let b = buf("a\n  b\nc\n");
        assert_eq!(target(&b, &cur(0), Motion::BufferTop, DEFAULT_ISKEYWORD_EXTRA), 0);
        assert_eq!(target(&b, &cur(0), Motion::BufferBottom, DEFAULT_ISKEYWORD_EXTRA), 6);
    }

    #[test]
    fn buffer_line_target_with_no_count_falls_back_to_the_motions_own_default() {
        let b = buf("a\n  b\nc\n");
        assert_eq!(buffer_line_target(&b, Motion::BufferTop, None), 0);
        assert_eq!(buffer_line_target(&b, Motion::BufferBottom, None), 6); // "c", not top
    }

    #[test]
    fn buffer_line_target_with_a_count_lands_on_that_1_indexed_lines_first_non_blank() {
        let b = buf("a\n  b\nc\n");
        // Line 2 (1-indexed) is "  b" -- first non-blank is at offset 2 within it.
        assert_eq!(buffer_line_target(&b, Motion::BufferTop, Some(2)), 4);
        assert_eq!(buffer_line_target(&b, Motion::BufferBottom, Some(2)), 4); // motion doesn't matter once a count is given
    }

    #[test]
    fn buffer_line_target_clamps_a_count_past_the_last_real_line() {
        let b = buf("a\nb\nc");
        assert_eq!(buffer_line_target(&b, Motion::BufferBottom, Some(500)), 4); // "c", not past it
    }

    #[test]
    fn buffer_line_target_treats_a_zero_count_as_line_one() {
        let b = buf("a\nb\nc");
        assert_eq!(buffer_line_target(&b, Motion::BufferTop, Some(0)), 0);
    }

    #[test]
    fn vertical_motion_uses_sticky_column_clamped_to_short_lines() {
        let b = buf("longline\nhi\nlongline");
        let c = Cursor { char_idx: 8, sticky_col: 8 }; // end of "longline"
        let after_down = target(&b, &c, Motion::Down, DEFAULT_ISKEYWORD_EXTRA);
        assert_eq!(b.line_col(&Cursor { char_idx: after_down, sticky_col: 0 }), (1, 1)); // clamped onto "hi"
    }

    #[test]
    fn big_word_forward_ignores_punctuation_boundaries() {
        let b = buf("foo.bar baz");
        // unlike WordForward, "foo.bar" is one WORD, not three
        assert_eq!(target(&b, &cur(0), Motion::BigWordForward, DEFAULT_ISKEYWORD_EXTRA), 8);
    }

    #[test]
    fn big_word_end_and_backward_ignore_punctuation_too() {
        let b = buf("foo.bar baz");
        assert_eq!(target(&b, &cur(0), Motion::BigWordEndForward, DEFAULT_ISKEYWORD_EXTRA), 6); // end of "foo.bar"
        assert_eq!(target(&b, &cur(8), Motion::BigWordBackward, DEFAULT_ISKEYWORD_EXTRA), 0);
    }

    #[test]
    fn find_char_forward_and_backward_stay_on_the_current_line() {
        let b = buf("abcXbcX\nXafter");
        assert_eq!(target(&b, &cur(0), Motion::FindChar('X'), DEFAULT_ISKEYWORD_EXTRA), 3); // first X on this line
        assert_eq!(target(&b, &cur(0), Motion::FindCharBack('X'), DEFAULT_ISKEYWORD_EXTRA), 0); // none before -- no-op
        assert_eq!(target(&b, &cur(6), Motion::FindCharBack('X'), DEFAULT_ISKEYWORD_EXTRA), 3); // scans back to the first line's X
        // Never crosses to the next line even though it has an 'X' too.
        assert_eq!(target(&b, &cur(6), Motion::FindChar('X'), DEFAULT_ISKEYWORD_EXTRA), 6);
    }

    #[test]
    fn till_char_stops_one_short_of_the_found_char() {
        let b = buf("abcXdef");
        assert_eq!(target(&b, &cur(0), Motion::TillChar('X'), DEFAULT_ISKEYWORD_EXTRA), 2); // right before the X
        assert_eq!(target(&b, &cur(6), Motion::TillCharBack('X'), DEFAULT_ISKEYWORD_EXTRA), 4); // right after the X
    }

    #[test]
    fn find_and_till_char_are_no_ops_when_the_char_is_not_on_the_line() {
        let b = buf("abcdef");
        assert_eq!(target(&b, &cur(0), Motion::FindChar('Z'), DEFAULT_ISKEYWORD_EXTRA), 0);
        assert_eq!(target(&b, &cur(0), Motion::TillChar('Z'), DEFAULT_ISKEYWORD_EXTRA), 0);
    }

    #[test]
    fn matching_bracket_delegates_to_bracket_find_match() {
        let b = buf("(hello)");
        assert_eq!(target(&b, &cur(0), Motion::MatchingBracket, DEFAULT_ISKEYWORD_EXTRA), 6);
        assert_eq!(target(&b, &cur(6), Motion::MatchingBracket, DEFAULT_ISKEYWORD_EXTRA), 0);
    }

    #[test]
    fn matching_bracket_is_a_no_op_off_a_bracket_or_unmatched() {
        let b = buf("(hello");
        assert_eq!(target(&b, &cur(1), Motion::MatchingBracket, DEFAULT_ISKEYWORD_EXTRA), 1); // 'h', not a bracket
        assert_eq!(target(&b, &cur(0), Motion::MatchingBracket, DEFAULT_ISKEYWORD_EXTRA), 0); // unmatched '('
    }

    #[test]
    fn paragraph_forward_and_backward_find_the_nearest_blank_line() {
        let b = buf("a\nb\n\nc\nd\n\ne");
        // Lines: 0 "a", 1 "b", 2 "" (blank), 3 "c", 4 "d", 5 "" (blank), 6 "e"
        assert_eq!(target(&b, &cur(0), Motion::ParagraphForward, DEFAULT_ISKEYWORD_EXTRA), b.line_start_char(2));
        assert_eq!(target(&b, &cur(0), Motion::ParagraphBackward, DEFAULT_ISKEYWORD_EXTRA), 0); // none before -- buffer start

        let on_c = Cursor { char_idx: b.line_start_char(3), sticky_col: 0 };
        assert_eq!(target(&b, &on_c, Motion::ParagraphForward, DEFAULT_ISKEYWORD_EXTRA), b.line_start_char(5));
        assert_eq!(target(&b, &on_c, Motion::ParagraphBackward, DEFAULT_ISKEYWORD_EXTRA), b.line_start_char(2));
    }

    #[test]
    fn paragraph_forward_lands_at_the_buffer_end_when_no_blank_line_follows() {
        let b = buf("a\nb\nc");
        let end = b.line_start_char(2) + b.line_len(2);
        assert_eq!(target(&b, &cur(0), Motion::ParagraphForward, DEFAULT_ISKEYWORD_EXTRA), end);
    }

    #[test]
    fn is_non_blank_at_distinguishes_space_from_word() {
        let b = buf("a b");
        assert!(is_non_blank_at(&b, 0));
        assert!(!is_non_blank_at(&b, 1));
        assert!(is_non_blank_at(&b, 2));
    }
}
