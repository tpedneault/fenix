/// A single, already-resolved keypress: what was pressed plus which
/// non-shift modifiers were held. Shift isn't tracked separately — for
/// printable keys the front end already reports the shifted character
/// (`'A'` vs `'a'`), which is all a key-sequence trie needs to tell them
/// apart.
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Mods {
    pub ctrl: bool,
    pub alt: bool,
    pub super_: bool,
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
}
