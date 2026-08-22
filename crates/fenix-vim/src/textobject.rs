use std::ops::Range;

use fenix_core::{Buffer, Cursor};

use crate::charclass::{classify, CharClass};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextObject {
    InnerWord,
    AWord,
}

/// The char range `obj` covers at the cursor. If the cursor sits on
/// whitespace, `iw`/`aw` select that whitespace run itself (matching real
/// Vim: whitespace is its own kind of "word" for this purpose).
pub fn span(buffer: &Buffer, cursor: &Cursor, obj: TextObject) -> Range<usize> {
    let len = buffer.len_chars();
    if len == 0 {
        return 0..0;
    }
    let at = cursor.char_idx.min(len - 1);
    let class = classify(buffer.char_at(at).unwrap());

    let mut start = at;
    while start > 0 && classify(buffer.char_at(start - 1).unwrap()) == class {
        start -= 1;
    }
    let mut end = at + 1;
    while end < len && classify(buffer.char_at(end).unwrap()) == class {
        end += 1;
    }

    match obj {
        TextObject::InnerWord => start..end,
        TextObject::AWord => {
            let mut a_end = end;
            let mut took_trailing = false;
            while a_end < len && classify(buffer.char_at(a_end).unwrap()) == CharClass::Space {
                a_end += 1;
                took_trailing = true;
            }
            if took_trailing {
                start..a_end
            } else {
                let mut a_start = start;
                while a_start > 0 && classify(buffer.char_at(a_start - 1).unwrap()) == CharClass::Space {
                    a_start -= 1;
                }
                a_start..end
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{buf, cur};

    #[test]
    fn inner_word_selects_just_the_word() {
        let b = buf("foo bar baz");
        assert_eq!(span(&b, &cur(5), TextObject::InnerWord), 4..7); // "bar"
    }

    #[test]
    fn a_word_prefers_trailing_whitespace() {
        let b = buf("foo bar baz");
        assert_eq!(span(&b, &cur(5), TextObject::AWord), 4..8); // "bar "
    }

    #[test]
    fn a_word_falls_back_to_leading_whitespace_at_end_of_buffer() {
        let b = buf("foo bar");
        assert_eq!(span(&b, &cur(5), TextObject::AWord), 3..7); // " bar"
    }

    #[test]
    fn text_object_on_punctuation_selects_the_punctuation_run() {
        let b = buf("foo.bar");
        assert_eq!(span(&b, &cur(3), TextObject::InnerWord), 3..4); // "."
    }

    #[test]
    fn text_object_on_whitespace_selects_the_whitespace_run() {
        let b = buf("foo   bar");
        assert_eq!(span(&b, &cur(4), TextObject::InnerWord), 3..6);
    }
}
