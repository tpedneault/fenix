use std::sync::OnceLock;

use fenix_keymap::{KeyCode, KeyPress, KeyTrie, Mods, NamedKey as FenixNamedKey};
use winit::event::KeyEvent;
use winit::keyboard::{Key, ModifiersState, NamedKey};

/// Translates a winit key event into fenix-keymap's UI-agnostic `KeyPress`.
/// Named `Space` is normalized to `KeyCode::Char(' ')` -- treating it like
/// any other printable key keeps the leader trie's sequences (`SPC f s`)
/// just a plain char sequence, no special-casing needed downstream.
/// Returns `None` for keys with nothing sensible to bind (F-keys, media
/// keys, ...).
pub fn to_keypress(event: &KeyEvent, mods: ModifiersState) -> Option<KeyPress> {
    let code = match &event.logical_key {
        Key::Named(NamedKey::Space) => KeyCode::Char(' '),
        Key::Named(NamedKey::Escape) => KeyCode::Named(FenixNamedKey::Escape),
        Key::Named(NamedKey::Enter) => KeyCode::Named(FenixNamedKey::Enter),
        Key::Named(NamedKey::Tab) => KeyCode::Named(FenixNamedKey::Tab),
        Key::Named(NamedKey::Backspace) => KeyCode::Named(FenixNamedKey::Backspace),
        Key::Named(NamedKey::Delete) => KeyCode::Named(FenixNamedKey::Delete),
        Key::Named(NamedKey::ArrowLeft) => KeyCode::Named(FenixNamedKey::Left),
        Key::Named(NamedKey::ArrowRight) => KeyCode::Named(FenixNamedKey::Right),
        Key::Named(NamedKey::ArrowUp) => KeyCode::Named(FenixNamedKey::Up),
        Key::Named(NamedKey::ArrowDown) => KeyCode::Named(FenixNamedKey::Down),
        Key::Named(NamedKey::Home) => KeyCode::Named(FenixNamedKey::Home),
        Key::Named(NamedKey::End) => KeyCode::Named(FenixNamedKey::End),
        Key::Named(NamedKey::PageUp) => KeyCode::Named(FenixNamedKey::PageUp),
        Key::Named(NamedKey::PageDown) => KeyCode::Named(FenixNamedKey::PageDown),
        Key::Character(s) => KeyCode::Char(s.chars().next()?),
        _ => return None,
    };
    Some(KeyPress {
        code,
        mods: Mods { ctrl: mods.control_key(), alt: mods.alt_key(), super_: mods.super_key() },
    })
}

/// The `SPC`-leader menu. Includes the leading space itself as the trie's
/// first key, so the whole leader interaction -- from the initial `SPC`
/// through to a resolved command -- is just one uniform walk of this trie.
///
/// Deliberately sparse: only wires groups that have a real command behind
/// them today (`file.save`, `app.quit`). Orbit-emacs's `SPC w`/`SPC b`/
/// `SPC t` groups have nothing to bind to yet -- no splits, multi-buffer,
/// or toggles exist until later phases -- so they're not stubbed in here.
pub fn leader_trie() -> &'static KeyTrie<&'static str> {
    static TRIE: OnceLock<KeyTrie<&'static str>> = OnceLock::new();
    TRIE.get_or_init(|| {
        let mut t = KeyTrie::new();
        let spc = KeyPress::char(' ');
        t.label_group(&[spc], "leader");

        t.label_group(&[spc, KeyPress::char('f')], "files");
        t.insert(&[spc, KeyPress::char('f'), KeyPress::char('s')], "save", "file.save");

        t.label_group(&[spc, KeyPress::char('q')], "quit");
        t.insert(&[spc, KeyPress::char('q'), KeyPress::char('q')], "quit", "app.quit");

        t
    })
}
