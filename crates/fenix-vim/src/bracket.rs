use fenix_core::Buffer;

/// If the character at `char_idx` is a bracket (`(`, `)`, `{`, `}`, `[`,
/// `]`), returns the char index of its matching pair by counting nested
/// same-type brackets between them. Ignores string/comment context --
/// the same scope real Vim's own `%` motion has without the `matchit`/
/// tree-sitter-aware plugins layered on top of it -- so a bracket inside
/// a string literal can still pair with unrelated code; a real fix would
/// need `fenix-syntax`'s parsed tree, a larger change than this pass.
/// Returns `None` if the character isn't a bracket, or no match is found
/// (unbalanced code).
pub fn find_match(buffer: &Buffer, char_idx: usize) -> Option<usize> {
    let ch = buffer.char_at(char_idx)?;
    let (open, close) = match ch {
        '(' | ')' => ('(', ')'),
        '{' | '}' => ('{', '}'),
        '[' | ']' => ('[', ']'),
        _ => return None,
    };
    if ch == open { find_forward(buffer, char_idx, open, close) } else { find_backward(buffer, char_idx, open, close) }
}

fn find_forward(buffer: &Buffer, from: usize, open: char, close: char) -> Option<usize> {
    let mut depth = 0i32;
    let mut i = from;
    loop {
        let c = buffer.char_at(i)?;
        if c == open {
            depth += 1;
        } else if c == close {
            depth -= 1;
            if depth == 0 {
                return Some(i);
            }
        }
        i += 1;
    }
}

/// The next `open`/`close` pair starting at or after `at`, bounded to
/// `line_end` (exclusive) -- `targets.vim`'s own "not inside a pair yet?
/// search forward on the line" fallback for `enclosing_pair`, the same
/// "search forward, don't just fail" behavior `quote_positions` already
/// has for quote objects. `None` if there's no `open` in `at..line_end`.
pub fn next_pair_on_line(buffer: &Buffer, at: usize, open: char, close: char, line_end: usize) -> Option<(usize, usize)> {
    let mut i = at;
    while i < line_end {
        if buffer.char_at(i) == Some(open) {
            return find_forward(buffer, i, open, close).map(|c| (i, c));
        }
        i += 1;
    }
    None
}

/// The innermost bracket pair enclosing `char_idx` -- unlike `find_match`,
/// which requires the cursor to already be sitting *on* a bracket, this
/// works from anywhere inside the pair (real Vim's `di(` doesn't require
/// sitting on the `(` itself). Scans backward from `char_idx`, tracking
/// nesting depth: a `close` crossed along the way means a complete
/// nested pair to skip over (one more unmatched `open` is needed to
/// cancel it out); the first `open` found at depth zero is the enclosing
/// one, and its own match (found via `find_forward`, reusing the same
/// logic `find_match` itself uses) is the pair's close. The char at
/// `char_idx` itself, if it's `close`, is exempted from the depth count
/// -- so sitting exactly on the closing delimiter still resolves to that
/// pair, not treats it as an already-matched nested pair to skip past.
/// Same "ignores string/comment context" scope as `find_match`. `None`
/// if `char_idx` isn't enclosed by a complete pair (unbalanced code, or
/// genuinely outside any).
pub fn enclosing_pair(buffer: &Buffer, char_idx: usize, open: char, close: char) -> Option<(usize, usize)> {
    let len = buffer.len_chars();
    if len == 0 {
        return None;
    }
    let at = char_idx.min(len - 1);
    let mut depth = 0i32;
    let mut i = at as isize;
    loop {
        if i < 0 {
            return None;
        }
        let idx = i as usize;
        match buffer.char_at(idx) {
            Some(c) if c == close && idx != at => depth += 1,
            Some(c) if c == open => {
                if depth == 0 {
                    let close_idx = find_forward(buffer, idx, open, close)?;
                    return Some((idx, close_idx));
                }
                depth -= 1;
            }
            _ => {}
        }
        i -= 1;
    }
}

fn find_backward(buffer: &Buffer, from: usize, open: char, close: char) -> Option<usize> {
    let mut depth = 0i32;
    let mut i = from as isize;
    loop {
        if i < 0 {
            return None;
        }
        let c = buffer.char_at(i as usize)?;
        if c == close {
            depth += 1;
        } else if c == open {
            depth -= 1;
            if depth == 0 {
                return Some(i as usize);
            }
        }
        i -= 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::buf;

    #[test]
    fn matches_a_simple_pair_from_either_side() {
        let b = buf("(a)");
        assert_eq!(find_match(&b, 0), Some(2));
        assert_eq!(find_match(&b, 2), Some(0));
    }

    #[test]
    fn matches_nested_pairs_of_the_same_type() {
        let b = buf("{a{b}c}");
        assert_eq!(find_match(&b, 0), Some(6)); // outer { -> outer }
        assert_eq!(find_match(&b, 6), Some(0)); // outer } -> outer {
        assert_eq!(find_match(&b, 2), Some(4)); // inner { -> inner }
        assert_eq!(find_match(&b, 4), Some(2)); // inner } -> inner {
    }

    #[test]
    fn each_bracket_type_only_pairs_with_its_own_kind() {
        // `(` at 0 skips over the unrelated `[`/`]` entirely and pairs
        // with the first `)` -- doesn't validate overall nesting
        // correctness, just counts same-type brackets.
        let b = buf("([)]");
        assert_eq!(find_match(&b, 0), Some(2));
        assert_eq!(find_match(&b, 1), Some(3));
    }

    #[test]
    fn returns_none_for_unbalanced_code() {
        let b = buf("(a");
        assert_eq!(find_match(&b, 0), None);
    }

    #[test]
    fn returns_none_when_not_on_a_bracket() {
        let b = buf("(a)");
        assert_eq!(find_match(&b, 1), None);
    }

    #[test]
    fn returns_none_past_the_end_of_the_buffer() {
        let b = buf("(a)");
        assert_eq!(find_match(&b, 100), None);
    }

    #[test]
    fn enclosing_pair_finds_the_pair_from_anywhere_inside_it() {
        let b = buf("(hello)");
        assert_eq!(enclosing_pair(&b, 3, '(', ')'), Some((0, 6))); // sitting on 'l'
    }

    #[test]
    fn enclosing_pair_works_when_sitting_on_either_delimiter() {
        let b = buf("(hello)");
        assert_eq!(enclosing_pair(&b, 0, '(', ')'), Some((0, 6)));
        assert_eq!(enclosing_pair(&b, 6, '(', ')'), Some((0, 6)));
    }

    #[test]
    fn enclosing_pair_finds_the_innermost_of_nested_pairs() {
        let b = buf("outer(a, inner(b, c), d)");
        assert_eq!(enclosing_pair(&b, 16, '(', ')'), Some((14, 19))); // on the inner ','
        assert_eq!(enclosing_pair(&b, 7, '(', ')'), Some((5, 23))); // on the outer ','
    }

    #[test]
    fn enclosing_pair_returns_none_when_not_enclosed() {
        let b = buf("(a) b (c)");
        assert_eq!(enclosing_pair(&b, 3, '(', ')'), None); // the space between the two pairs
    }

    #[test]
    fn enclosing_pair_returns_none_for_unbalanced_code() {
        let b = buf("(a");
        assert_eq!(enclosing_pair(&b, 1, '(', ')'), None);
    }

    #[test]
    fn enclosing_pair_ignores_a_different_bracket_type() {
        let b = buf("[a(b)c]");
        // Looking for () around 'a' (index 1), which is only inside [],
        // not any (): must not accidentally match the unrelated pair.
        assert_eq!(enclosing_pair(&b, 1, '(', ')'), None);
    }

    #[test]
    fn next_pair_on_line_finds_a_pair_starting_ahead_of_the_cursor() {
        let b = buf("x = (hello)");
        assert_eq!(next_pair_on_line(&b, 0, '(', ')', b.len_chars()), Some((4, 10)));
    }

    #[test]
    fn next_pair_on_line_finds_the_pair_it_already_starts_on() {
        let b = buf("(hello)");
        assert_eq!(next_pair_on_line(&b, 0, '(', ')', b.len_chars()), Some((0, 6)));
    }

    #[test]
    fn next_pair_on_line_returns_none_past_line_end() {
        let b = buf("x = (hello)");
        // Bounded to stop before the '(' -- nothing to find.
        assert_eq!(next_pair_on_line(&b, 0, '(', ')', 4), None);
    }

    #[test]
    fn next_pair_on_line_returns_none_with_no_open_at_all() {
        let b = buf("no brackets here");
        assert_eq!(next_pair_on_line(&b, 0, '(', ')', b.len_chars()), None);
    }
}
