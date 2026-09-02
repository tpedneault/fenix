//! Winit/`fenix-keymap` key -> X11 keysym mapping. RFB's `KeyEvent`
//! message wants a keysym (`ClientKeyEvent.keycode` in `vnc-rs`), not a
//! scancode or ANSI byte sequence the way `fenix-terminal`'s
//! `encode_terminal_key` works -- this is a different, larger table, not
//! reusable from that path.
//!
//! Deliberately ignores `KeyPress::mods` -- Ctrl/Alt held during a key
//! aren't a distinct `KeyCode` in this app's model (they're bundled onto
//! whichever real key was pressed, see `fenix_keymap::KeyPress`'s own doc
//! comment), so wrapping a forwarded key in Control_L/Alt_L down/up when
//! a modifier is held is the caller's job (in `fenix-gui`, which is what
//! actually knows about down/up timing) -- this module only maps the
//! "real" key itself. The modifier keysyms it would need for that
//! wrapping are exported here as constants so the caller doesn't need
//! its own copy of the X11 keysymdef values.

use fenix_keymap::{KeyCode, KeyPress, NamedKey};

pub const CONTROL_L: u32 = 0xFFE3;
pub const ALT_L: u32 = 0xFFE9;
pub const SUPER_L: u32 = 0xFFEB;
/// Not part of `keysym_for`'s table: winit reports Caps Lock as its own
/// `NamedKey` with no equivalent in `fenix_keymap::NamedKey` (it isn't a
/// meaningful vim/leader keybinding), so `fenix-gui` forwards it as a
/// direct press/release special case, bypassing `keysym_for` entirely --
/// exported here so that caller doesn't need its own copy of the X11
/// keysymdef value, same reasoning as `CONTROL_L`/`ALT_L`/`SUPER_L`.
pub const CAPS_LOCK: u32 = 0xFFE5;

/// Maps one resolved key (ignoring modifiers, see module doc) to its X11
/// keysym, or `None` for anything this table doesn't cover.
pub fn keysym_for(kp: KeyPress) -> Option<u32> {
    match kp.code {
        // Printable ASCII: the X11 keysym for 0x20..=0x7E is defined to
        // equal the character's own Latin-1 codepoint. Anything outside
        // that (both control characters and non-Latin-1 Unicode) falls
        // through to the Unicode keysym convention below.
        KeyCode::Char(c) if (' '..='~').contains(&c) => Some(c as u32),
        // X11's "Unicode keysym" extension: any other codepoint is
        // `0x01000000 | codepoint`. Covers everything from accented
        // letters to CJK/emoji that a real keyboard layout could type.
        KeyCode::Char(c) => Some(0x0100_0000 | c as u32),
        KeyCode::Named(named) => named_keysym(named),
    }
}

fn named_keysym(named: NamedKey) -> Option<u32> {
    let keysym = match named {
        NamedKey::Escape => 0xFF1B,
        NamedKey::Enter => 0xFF0D,
        NamedKey::Tab => 0xFF09,
        NamedKey::Backspace => 0xFF08,
        NamedKey::Delete => 0xFFFF,
        NamedKey::Left => 0xFF51,
        NamedKey::Right => 0xFF53,
        NamedKey::Up => 0xFF52,
        NamedKey::Down => 0xFF54,
        NamedKey::Home => 0xFF50,
        NamedKey::End => 0xFF57,
        NamedKey::PageUp => 0xFF55,
        NamedKey::PageDown => 0xFF56,
    };
    Some(keysym)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_printable_ascii_char_maps_to_its_own_codepoint() {
        for c in ' '..='~' {
            assert_eq!(keysym_for(KeyPress::char(c)), Some(c as u32), "char {c:?}");
        }
    }

    #[test]
    fn a_non_latin1_char_uses_the_unicode_keysym_convention() {
        assert_eq!(keysym_for(KeyPress::char('é')), Some(0x0100_0000 | 'é' as u32));
        assert_eq!(keysym_for(KeyPress::char('★')), Some(0x0100_0000 | '★' as u32));
    }

    #[test]
    fn named_keys_map_to_their_documented_x11_keysym() {
        assert_eq!(keysym_for(KeyPress::named(NamedKey::Escape)), Some(0xFF1B));
        assert_eq!(keysym_for(KeyPress::named(NamedKey::Enter)), Some(0xFF0D));
        assert_eq!(keysym_for(KeyPress::named(NamedKey::Tab)), Some(0xFF09));
        assert_eq!(keysym_for(KeyPress::named(NamedKey::Backspace)), Some(0xFF08));
        assert_eq!(keysym_for(KeyPress::named(NamedKey::Delete)), Some(0xFFFF));
        assert_eq!(keysym_for(KeyPress::named(NamedKey::Left)), Some(0xFF51));
        assert_eq!(keysym_for(KeyPress::named(NamedKey::Right)), Some(0xFF53));
        assert_eq!(keysym_for(KeyPress::named(NamedKey::Up)), Some(0xFF52));
        assert_eq!(keysym_for(KeyPress::named(NamedKey::Down)), Some(0xFF54));
        assert_eq!(keysym_for(KeyPress::named(NamedKey::Home)), Some(0xFF50));
        assert_eq!(keysym_for(KeyPress::named(NamedKey::End)), Some(0xFF57));
        assert_eq!(keysym_for(KeyPress::named(NamedKey::PageUp)), Some(0xFF55));
        assert_eq!(keysym_for(KeyPress::named(NamedKey::PageDown)), Some(0xFF56));
    }

    #[test]
    fn modifier_keysym_constants_match_the_documented_x11_values() {
        assert_eq!(CONTROL_L, 0xFFE3);
        assert_eq!(ALT_L, 0xFFE9);
        assert_eq!(SUPER_L, 0xFFEB);
        assert_eq!(CAPS_LOCK, 0xFFE5);
    }
}
