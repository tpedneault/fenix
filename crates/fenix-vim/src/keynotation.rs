//! Vim's own keycode notation -- the plain-text representation register
//! content uses for anything beyond a literal printable character (a
//! literal `<` escapes as `<lt>`; special keys and modified chords use
//! `<...>` tokens like `<Esc>`, `<CR>`, `<C-r>`). This is what makes
//! unified register storage real: a `q{register}...q` recording is
//! encoded into this notation and written into the register as ordinary
//! text (`VimState::finish_recording`), and `@{register}` decodes
//! whatever text is there -- typed, yanked, or recorded -- back into
//! keystrokes to replay.
//!
//! Shift is never a modifier token here (matching `KeyPress`'s own doc
//! comment: the front end already reports the shifted character, e.g.
//! `Char('A')`, not a separate Shift flag) -- only Ctrl/Alt/Super
//! (`<C-x>`/`<M-x>`/`<D-x>`, stackable as `<C-M-x>`) ever appear.

use fenix_keymap::{KeyCode, KeyPress, Mods, NamedKey};

/// `NamedKey` <-> Vim's own established token, in both directions.
const NAMED_TOKENS: &[(NamedKey, &str)] = &[
    (NamedKey::Escape, "Esc"),
    (NamedKey::Enter, "CR"),
    (NamedKey::Tab, "Tab"),
    (NamedKey::Backspace, "BS"),
    (NamedKey::Delete, "Del"),
    (NamedKey::Left, "Left"),
    (NamedKey::Right, "Right"),
    (NamedKey::Up, "Up"),
    (NamedKey::Down, "Down"),
    (NamedKey::Home, "Home"),
    (NamedKey::End, "End"),
    (NamedKey::PageUp, "PageUp"),
    (NamedKey::PageDown, "PageDown"),
];

fn named_token(n: NamedKey) -> &'static str {
    NAMED_TOKENS.iter().find(|(k, _)| *k == n).map(|(_, s)| *s).unwrap_or("?")
}

fn token_to_named(token: &str) -> Option<NamedKey> {
    NAMED_TOKENS.iter().find(|(_, s)| s.eq_ignore_ascii_case(token)).map(|(k, _)| *k)
}

/// Encodes `keys` into Vim's own keycode notation.
pub fn encode(keys: &[KeyPress]) -> String {
    let mut out = String::new();
    for key in keys {
        encode_one(*key, &mut out);
    }
    out
}

fn encode_one(key: KeyPress, out: &mut String) {
    let mut mod_prefix = String::new();
    if key.mods.ctrl {
        mod_prefix.push_str("C-");
    }
    if key.mods.alt {
        mod_prefix.push_str("M-");
    }
    if key.mods.super_ {
        mod_prefix.push_str("D-");
    }
    match key.code {
        KeyCode::Char('<') if mod_prefix.is_empty() => out.push_str("<lt>"),
        KeyCode::Char(c) if mod_prefix.is_empty() => out.push(c),
        KeyCode::Char(c) => {
            out.push('<');
            out.push_str(&mod_prefix);
            out.push(c);
            out.push('>');
        }
        KeyCode::Named(n) => {
            out.push('<');
            out.push_str(&mod_prefix);
            out.push_str(named_token(n));
            out.push('>');
        }
    }
}

/// Decodes Vim's own keycode notation back into keystrokes -- the
/// inverse of `encode`, tolerant of anything it doesn't recognize (an
/// unclosed or unknown `<...>` degrades to its own literal characters
/// rather than erroring: pasted/hand-edited register text should never
/// be able to crash a `@` replay).
pub fn decode(text: &str) -> Vec<KeyPress> {
    let mut keys = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '<' {
            keys.push(KeyPress::char(c));
            continue;
        }
        // Collect up to the matching '>', if any, without consuming
        // input past an unclosed '<'.
        let mut token = String::new();
        let mut closed = false;
        let mut consumed = Vec::new();
        for next in chars.by_ref() {
            consumed.push(next);
            if next == '>' {
                closed = true;
                break;
            }
            token.push(next);
        }
        if closed {
            if let Some(key) = decode_token(&token) {
                keys.push(key);
                continue;
            }
        }
        // Not a recognized (or not a closed) token -- fall back to the
        // literal characters, including the '<' itself.
        keys.push(KeyPress::char('<'));
        for c in consumed {
            keys.push(KeyPress::char(c));
        }
    }
    keys
}

fn decode_token(token: &str) -> Option<KeyPress> {
    if token.eq_ignore_ascii_case("lt") {
        return Some(KeyPress::char('<'));
    }
    let mut mods = Mods::default();
    let mut rest = token;
    loop {
        let mut split = rest.splitn(2, '-');
        let head = split.next().unwrap_or("");
        let Some(tail) = split.next() else { break };
        match head.to_ascii_uppercase().as_str() {
            "C" => mods.ctrl = true,
            "M" => mods.alt = true,
            "D" => mods.super_ = true,
            _ => break, // not a recognized modifier prefix -- stop consuming
        }
        rest = tail;
    }
    if let Some(named) = token_to_named(rest) {
        return Some(KeyPress { code: KeyCode::Named(named), mods });
    }
    let mut chars = rest.chars();
    let c = chars.next()?;
    if chars.next().is_some() {
        return None; // more than one char left after stripping mods -- not decodable
    }
    Some(KeyPress { code: KeyCode::Char(c), mods })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(keys: &[KeyPress]) {
        assert_eq!(decode(&encode(keys)), keys);
    }

    #[test]
    fn plain_chars_encode_literally() {
        let keys = vec![KeyPress::char('d'), KeyPress::char('w'), KeyPress::char('j')];
        assert_eq!(encode(&keys), "dwj");
        roundtrip(&keys);
    }

    #[test]
    fn a_literal_less_than_escapes_as_lt() {
        let keys = vec![KeyPress::char('<'), KeyPress::char('x')];
        assert_eq!(encode(&keys), "<lt>x");
        roundtrip(&keys);
    }

    #[test]
    fn every_named_key_round_trips() {
        for &(n, token) in NAMED_TOKENS {
            let keys = vec![KeyPress::named(n)];
            assert_eq!(encode(&keys), format!("<{token}>"));
            roundtrip(&keys);
        }
    }

    #[test]
    fn ctrl_chord_round_trips() {
        let keys = vec![KeyPress::char('r').with_ctrl()];
        assert_eq!(encode(&keys), "<C-r>");
        roundtrip(&keys);
    }

    #[test]
    fn stacked_modifiers_round_trip() {
        let keys = vec![KeyPress::char('x').with_ctrl().with_alt()];
        assert_eq!(encode(&keys), "<C-M-x>");
        roundtrip(&keys);
    }

    #[test]
    fn a_modified_named_key_round_trips() {
        let keys = vec![KeyPress::named(NamedKey::Enter).with_ctrl()];
        assert_eq!(encode(&keys), "<C-CR>");
        roundtrip(&keys);
    }

    #[test]
    fn decode_is_case_insensitive_for_tokens() {
        assert_eq!(decode("<esc>"), vec![KeyPress::named(NamedKey::Escape)]);
        assert_eq!(decode("<CR>"), vec![KeyPress::named(NamedKey::Enter)]);
    }

    #[test]
    fn decode_of_an_unknown_token_falls_back_to_literal_characters() {
        assert_eq!(
            decode("<Bogus>"),
            vec![
                KeyPress::char('<'),
                KeyPress::char('B'),
                KeyPress::char('o'),
                KeyPress::char('g'),
                KeyPress::char('u'),
                KeyPress::char('s'),
                KeyPress::char('>'),
            ]
        );
    }

    #[test]
    fn decode_of_an_unclosed_bracket_falls_back_to_literal_characters_without_panicking() {
        assert_eq!(decode("<Esc"), vec![KeyPress::char('<'), KeyPress::char('E'), KeyPress::char('s'), KeyPress::char('c')]);
    }

    #[test]
    fn a_realistic_recorded_macro_round_trips() {
        // "dw" then Escape then ":wq" then Enter -- a plausible qa...q body.
        let keys = vec![
            KeyPress::char('d'),
            KeyPress::char('w'),
            KeyPress::named(NamedKey::Escape),
            KeyPress::char(':'),
            KeyPress::char('w'),
            KeyPress::char('q'),
            KeyPress::named(NamedKey::Enter),
        ];
        assert_eq!(encode(&keys), "dw<Esc>:wq<CR>");
        roundtrip(&keys);
    }
}
