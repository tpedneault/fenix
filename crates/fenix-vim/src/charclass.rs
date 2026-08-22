/// Vim's "small word" character classes: word runs, punctuation runs, and
/// whitespace runs are each their own kind of word boundary (so `foo.bar`
/// is three words: `foo`, `.`, `bar`). Shared by word motions and the
/// `iw`/`aw` text objects, which both need identical boundary rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CharClass {
    Word,
    Punct,
    Space,
}

pub fn classify(c: char) -> CharClass {
    if c.is_whitespace() {
        CharClass::Space
    } else if c.is_alphanumeric() || c == '_' {
        CharClass::Word
    } else {
        CharClass::Punct
    }
}
