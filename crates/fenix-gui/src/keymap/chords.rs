//! Chords: single keys with modifiers (`C-s`, `C-S-p`, `F12`, `M-Left`)
//! bound to commands outside the leader -- the `[[keys]]` tables of
//! Fenix's keymap, the extensions' and yours.
//!
//! An entry says which key scheme it's for (`scheme`) and where it
//! applies (`when`). Entries are laid in order -- Fenix's, the
//! extensions', yours -- and the last that fits wins; an `unbind` entry
//! that fits takes the key away instead.

use fenix_keymap::KeyPress;

/// The key scheme an entry is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Scheme {
    Vim,
    Standard,
    #[default]
    Any,
}

impl Scheme {
    pub fn parse(text: &str) -> Option<Scheme> {
        Some(match text {
            "vim" => Scheme::Vim,
            "standard" => Scheme::Standard,
            "any" => Scheme::Any,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Scheme::Vim => "vim",
            Scheme::Standard => "standard",
            Scheme::Any => "any",
        }
    }

    /// Whether an entry for `self` is in use under `scheme` (`Vim` or
    /// `Standard`).
    pub fn applies(self, scheme: Scheme) -> bool {
        self == Scheme::Any || self == scheme
    }
}

/// Where an entry applies. A short, fixed list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum When {
    /// Wherever the chord map is asked: not in a focused terminal.
    #[default]
    Anywhere,
    /// A text buffer is focused, whatever its mode.
    Text,
    /// Text is being typed (Vim's Insert; standard mode's typing).
    Typing,
    /// Text isn't being typed.
    NotTyping,
    /// A page is focused.
    Page,
    /// Anywhere, and in a focused terminal too: the shell doesn't get it.
    Terminal,
}

impl When {
    pub fn parse(text: &str) -> Option<When> {
        Some(match text {
            "anywhere" => When::Anywhere,
            "text" => When::Text,
            "typing" => When::Typing,
            "not_typing" => When::NotTyping,
            "page" => When::Page,
            "terminal" => When::Terminal,
            _ => return None,
        })
    }

    fn fits(self, at: &ChordContext) -> bool {
        if at.terminal {
            return self == When::Terminal;
        }
        match self {
            When::Anywhere | When::Terminal => true,
            When::Text => at.text,
            When::Typing => at.typing,
            When::NotTyping => !at.typing,
            When::Page => at.page,
        }
    }
}

/// What's in front of you when a chord is looked up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ChordContext {
    pub scheme: Scheme,
    /// A terminal has the keyboard.
    pub terminal: bool,
    /// A text buffer is focused.
    pub text: bool,
    /// Text is being typed.
    pub typing: bool,
    /// A page is focused.
    pub page: bool,
}

/// One chord entry, its keys in the chord form (`KeyPress::chord`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chord {
    pub key: KeyPress,
    pub scheme: Scheme,
    pub when: When,
    /// `None`: an `unbind` entry.
    pub command: Option<&'static str>,
    pub label: Option<&'static str>,
    /// Who it's from: `None` for Fenix's own, the file's name otherwise.
    pub from: Option<&'static str>,
}

/// Every chord in use, in the order they were laid.
#[derive(Debug, Clone, Default)]
pub struct ChordMap {
    entries: Vec<Chord>,
}

impl ChordMap {
    pub fn new() -> ChordMap {
        ChordMap::default()
    }

    pub fn push(&mut self, chord: Chord) {
        self.entries.push(chord);
    }

    /// The command `key` runs here, if any.
    pub fn lookup(&self, key: KeyPress, at: &ChordContext) -> Option<&'static str> {
        let key = key.chord();
        let scheme = if at.scheme == Scheme::Any { Scheme::Vim } else { at.scheme };
        let found = self.entries.iter().rev().find(|c| c.key == key && c.scheme.applies(scheme) && (c.command.is_none() || c.when.fits(at)))?;
        found.command
    }

    /// Whether some entry for `scheme` takes `key` while a terminal has
    /// the keyboard.
    pub fn skips_terminal(&self, key: KeyPress, scheme: Scheme) -> Option<&'static str> {
        self.lookup(key, &ChordContext { scheme, terminal: true, ..ChordContext::default() })
    }

    /// The chords in use under `scheme`, the winner for each key and
    /// `when` only.
    pub fn in_use(&self, scheme: Scheme) -> Vec<&Chord> {
        let mut out: Vec<&Chord> = Vec::new();
        for c in self.entries.iter().rev() {
            if !c.scheme.applies(scheme) || out.iter().any(|o| o.key == c.key && (o.when == c.when || o.command.is_none())) {
                continue;
            }
            out.push(c);
        }
        out.retain(|c| c.command.is_some());
        out.reverse();
        out
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    fn chord(key: KeyPress, scheme: Scheme, when: When, command: Option<&'static str>) -> Chord {
        Chord { key, scheme, when, command, label: None, from: None }
    }

    #[test]
    fn the_last_entry_that_fits_wins_and_an_unbind_takes_the_key_away() {
        let mut map = ChordMap::new();
        let save = KeyPress::char('s').with_ctrl();
        map.push(chord(save, Scheme::Any, When::Anywhere, Some("file.save")));
        let at = ChordContext { scheme: Scheme::Vim, ..ChordContext::default() };
        assert_eq!(map.lookup(save, &at), Some("file.save"));
        map.push(chord(save, Scheme::Any, When::Anywhere, Some("file.save_force")));
        assert_eq!(map.lookup(save, &at), Some("file.save_force"));
        map.push(chord(save, Scheme::Any, When::Anywhere, None));
        assert_eq!(map.lookup(save, &at), None);
    }

    #[test]
    fn scheme_and_when_decide_what_applies() {
        let mut map = ChordMap::new();
        let o = KeyPress::char('o').with_ctrl();
        map.push(chord(o, Scheme::Vim, When::NotTyping, Some("nav.jump_back")));
        map.push(chord(o, Scheme::Standard, When::Anywhere, Some("file.find")));
        let vim = ChordContext { scheme: Scheme::Vim, text: true, ..ChordContext::default() };
        assert_eq!(map.lookup(o, &vim), Some("nav.jump_back"));
        assert_eq!(map.lookup(o, &ChordContext { typing: true, ..vim }), None, "Insert's own Ctrl-O");
        let standard = ChordContext { scheme: Scheme::Standard, ..vim };
        assert_eq!(map.lookup(o, &standard), Some("file.find"));
    }

    #[test]
    fn only_terminal_entries_reach_a_focused_terminal() {
        let mut map = ChordMap::new();
        let p = KeyPress::char('p').with_ctrl();
        let j = KeyPress::char('j').with_ctrl();
        map.push(chord(p, Scheme::Standard, When::Terminal, Some("project.find_file")));
        map.push(chord(j, Scheme::Standard, When::Anywhere, Some("terminal.toggle")));
        assert_eq!(map.skips_terminal(p, Scheme::Standard), Some("project.find_file"));
        assert_eq!(map.skips_terminal(j, Scheme::Standard), None);
        assert_eq!(map.skips_terminal(p, Scheme::Vim), None);
    }

    #[test]
    fn a_letter_matches_however_the_platform_cased_it() {
        let mut map = ChordMap::new();
        map.push(chord(KeyPress::char('p').with_ctrl().with_shift(), Scheme::Any, When::Anywhere, Some("commands.palette")));
        let at = ChordContext { scheme: Scheme::Vim, ..ChordContext::default() };
        assert_eq!(map.lookup(KeyPress::char('P').with_ctrl(), &at), Some("commands.palette"));
        assert_eq!(map.lookup(KeyPress::char('p').with_ctrl(), &at), None);
    }
}
