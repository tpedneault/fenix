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
}
