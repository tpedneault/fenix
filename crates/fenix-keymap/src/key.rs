/// A single, already-resolved keypress: what was pressed plus which
/// modifiers were held.
///
/// Shift is kept only where it says something the key itself doesn't: a
/// printable character pressed with no Ctrl or Alt never has it, since the
/// front end already reports the shifted character (`'A'` vs `'a'`), which
/// is all a key-sequence trie (Vim's grammar, the leader) needs to tell
/// them apart. With Ctrl or Alt held (`C-S-p`), or on a named key
/// (`S-<Down>`), it's kept.
///
/// Most of Fenix reads keys through [`KeyPress::legacy`], which drops it
/// again: only the chord map and the standard editing policy look at it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct KeyPress {
    pub code: KeyCode,
    pub mods: Mods,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeyCode {
    Char(char),
    Named(NamedKey),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NamedKey {
    Escape,
    Enter,
    Tab,
    Backspace,
    Delete,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    /// F1 to F12.
    F(u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Mods {
    pub ctrl: bool,
    pub alt: bool,
    pub super_: bool,
    /// See [`KeyPress`]: never set on a plain printable character.
    pub shift: bool,
}

impl Mods {
    /// No modifier at all.
    pub fn none(self) -> bool {
        self == Mods::default()
    }
}

impl KeyPress {
    pub fn char(c: char) -> Self {
        Self { code: KeyCode::Char(c), mods: Mods::default() }
    }

    pub fn named(n: NamedKey) -> Self {
        Self { code: KeyCode::Named(n), mods: Mods::default() }
    }

    #[must_use]
    pub fn with_ctrl(mut self) -> Self {
        self.mods.ctrl = true;
        self
    }

    #[must_use]
    pub fn with_alt(mut self) -> Self {
        self.mods.alt = true;
        self
    }

    #[must_use]
    pub fn with_shift(mut self) -> Self {
        self.mods.shift = true;
        self
    }

    /// The key as Fenix read keys before Shift was kept: Shift dropped.
    /// Everything that matched keys exactly before still does.
    #[must_use]
    pub fn legacy(mut self) -> Self {
        self.mods.shift = false;
        self
    }

    /// The form chords are bound and looked up in: a letter pressed with
    /// Ctrl or Alt is lower case, its Shift in `mods` (`C-S-p`, however the
    /// platform reported the letter). Any other character carries its
    /// Shift itself (`C-+`, `C-?`), so it has none.
    #[must_use]
    pub fn chord(mut self) -> Self {
        if let KeyCode::Char(c) = self.code {
            if (self.mods.ctrl || self.mods.alt) && c.is_alphabetic() {
                if c.is_uppercase() {
                    self.mods.shift = true;
                }
                self.code = KeyCode::Char(c.to_lowercase().next().unwrap_or(c));
            } else if !c.is_alphabetic() {
                self.mods.shift = false;
            }
        }
        self
    }

    /// An F-key.
    pub fn is_function(self) -> bool {
        matches!(self.code, KeyCode::Named(NamedKey::F(_)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chord_form_lowers_a_ctrl_letter_and_keeps_its_shift() {
        let upper = KeyPress::char('P').with_ctrl();
        assert_eq!(upper.chord(), KeyPress::char('p').with_ctrl().with_shift());
        assert_eq!(KeyPress::char('p').with_ctrl().with_shift().chord(), KeyPress::char('p').with_ctrl().with_shift());
        // A plain letter is left alone: its case is the key.
        assert_eq!(KeyPress::char('P').chord(), KeyPress::char('P'));
        // Punctuation carries its own Shift.
        assert_eq!(KeyPress::char('+').with_ctrl().with_shift().chord(), KeyPress::char('+').with_ctrl());
    }

    #[test]
    fn legacy_drops_shift_only() {
        let k = KeyPress::named(NamedKey::Down).with_shift().with_ctrl();
        assert_eq!(k.legacy(), KeyPress::named(NamedKey::Down).with_ctrl());
    }
}
