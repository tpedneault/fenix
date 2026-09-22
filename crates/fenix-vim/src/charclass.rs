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

/// Real Vim's own default `'iskeyword'`: letters, digits, and
/// underscore are keyword (word-class) characters; everything else
/// that isn't whitespace is punctuation. `VimState::iskeyword_extra`
/// starts here and can be narrowed with `:set iskeyword-=_` (or
/// widened with `+=`) -- see `classify`'s own doc comment for why
/// `_` is a separate, user-adjustable set rather than baked into this
/// function the way letters/digits are.
pub const DEFAULT_ISKEYWORD_EXTRA: &[char] = &['_'];

/// `c`'s class for word-motion/text-object purposes. `extra_keyword_chars`
/// is real Vim's `'iskeyword'` option, narrowed to just the punctuation
/// it adds on top of "every alphanumeric character" (which `'iskeyword'`
/// also always includes, via its own `@` token, and isn't itself
/// configurable here) -- `DEFAULT_ISKEYWORD_EXTRA` (just `_`) reproduces
/// Vim's actual factory default, under which `testing_variables` is one
/// single word to `w`/`e`/`b`; passing `&[]` (real Vim's `:set
/// iskeyword-=_`) instead makes `_` a punctuation-class boundary of its
/// own, so `e` on `testing_variables` stops at the end of `testing` --
/// the snake_case-aware navigation some editors (Doom Emacs's evil-mode
/// among them) ship as their own default, achievable in real Vim with
/// that one `:set`.
pub fn classify(c: char, extra_keyword_chars: &[char]) -> CharClass {
    if c.is_whitespace() {
        CharClass::Space
    } else if c.is_alphanumeric() || extra_keyword_chars.contains(&c) {
        CharClass::Word
    } else {
        CharClass::Punct
    }
}
