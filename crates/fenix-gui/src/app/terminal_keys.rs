//! The bytes a key sends to a terminal's program, as xterm sends them --
//! what PowerShell's line editor (PSReadLine), readline, zsh and the
//! full-screen programs read: `Ctrl`, `Shift` and `Alt` on the arrows,
//! Home, End, Page Up and Down and Delete as xterm's modifier parameter
//! (`ESC [ 1 ; 5 D` is Ctrl-Left, a word back), `Ctrl-Space`
//! (PSReadLine's menu completion), `Shift-Tab`, `Alt` as an `ESC` before
//! the key, the F-keys, and the arrows' application mode (`ESC O A`)
//! when the program asks for it.
//!
//! Shift isn't part of a `KeyPress` -- a printable key already says it by
//! its character -- so it comes in beside it, read from the keyboard as
//! the key goes down.

use fenix_keymap::{KeyCode, KeyPress, NamedKey};

/// xterm's modifier parameter: 1, plus 1 for Shift, 2 for Alt, 4 for Ctrl.
/// None with no modifier: the key's plain sequence then.
fn modifier_param(shift: bool, alt: bool, ctrl: bool) -> Option<u8> {
    let m = 1 + u8::from(shift) + 2 * u8::from(alt) + 4 * u8::from(ctrl);
    (m > 1).then_some(m)
}

/// A key's bytes: `ESC [ <final>` -- or `ESC O <final>` in application
/// mode -- and `ESC [ 1 ; <mods> <final>` with modifiers.
fn csi_letter(letter: u8, mods: Option<u8>, application: bool) -> Vec<u8> {
    match mods {
        Some(m) => format!("\x1b[1;{m}{}", letter as char).into_bytes(),
        None if application => vec![0x1b, b'O', letter],
        None => vec![0x1b, b'[', letter],
    }
}

/// `ESC [ <n> ~`, and `ESC [ <n> ; <mods> ~` with modifiers.
fn csi_tilde(n: u8, mods: Option<u8>) -> Vec<u8> {
    match mods {
        Some(m) => format!("\x1b[{n};{m}~").into_bytes(),
        None => format!("\x1b[{n}~").into_bytes(),
    }
}

/// What Ctrl makes of a character: a letter's control code, and the few
/// others terminals have always given one (`Ctrl-Space` is NUL).
fn control_code(c: char) -> Option<u8> {
    match c {
        'a'..='z' => Some(c as u8 - b'a' + 1),
        'A'..='Z' => Some(c as u8 - b'A' + 1),
        ' ' | '@' | '2' => Some(0),
        '[' | '3' => Some(0x1b),
        ']' | '5' => Some(0x1d),
        '^' | '6' => Some(0x1e),
        '_' | '-' | '/' | '7' => Some(0x1f),
        '?' | '8' => Some(0x7f),
        _ => None,
    }
}

/// The bytes `key` sends, `shift` held or not, the arrows in application
/// mode when the program asked (`application`). None for a key a terminal
/// has no bytes for.
///
/// In a session that takes Windows key events (`win32`, ConPTY's
/// win32-input-mode), Ctrl-Space is sent as the key itself: as NUL,
/// ConPTY gives PowerShell Ctrl-@, and its menu completion
/// (`Ctrl+Spacebar`) never opens.
pub(super) fn encode(key: KeyPress, shift: bool, application: bool, win32: bool) -> Option<Vec<u8>> {
    let (ctrl, alt) = (key.mods.ctrl, key.mods.alt);
    if win32 && ctrl && !alt && key.code == KeyCode::Char(' ') {
        let state = fenix_terminal::LEFT_CTRL_PRESSED | if shift { fenix_terminal::SHIFT_PRESSED } else { 0 };
        return Some(fenix_terminal::win32_key(0x20, 0x39, ' ', state));
    }
    let esc = |mut bytes: Vec<u8>, alt: bool| {
        if alt {
            bytes.insert(0, 0x1b);
        }
        bytes
    };
    match key.code {
        KeyCode::Char(c) => {
            // Ctrl and Alt both on a character that isn't a letter is AltGr
            // on a European layout (`@`, `{`, `\`): the character itself.
            if ctrl && alt && !c.is_ascii_alphabetic() {
                return Some(c.to_string().into_bytes());
            }
            if ctrl {
                return control_code(c).map(|b| esc(vec![b], alt));
            }
            Some(esc(c.to_string().into_bytes(), alt))
        }
        KeyCode::Named(NamedKey::F(n)) => encode_function(n, shift, alt, ctrl),
        KeyCode::Named(named) => {
            let mods = modifier_param(shift, alt, ctrl);
            Some(match named {
                NamedKey::Up => csi_letter(b'A', mods, application),
                NamedKey::Down => csi_letter(b'B', mods, application),
                NamedKey::Right => csi_letter(b'C', mods, application),
                NamedKey::Left => csi_letter(b'D', mods, application),
                NamedKey::Home => csi_letter(b'H', mods, application),
                NamedKey::End => csi_letter(b'F', mods, application),
                NamedKey::PageUp => csi_tilde(5, mods),
                NamedKey::PageDown => csi_tilde(6, mods),
                NamedKey::Delete => csi_tilde(3, mods),
                NamedKey::Tab if shift => b"\x1b[Z".to_vec(),
                NamedKey::Tab => esc(b"\t".to_vec(), alt),
                NamedKey::Enter => esc(b"\r".to_vec(), alt),
                NamedKey::Escape => vec![0x1b],
                // Ctrl-Backspace deletes a word back. Windows Terminal
                // sends ^H for it, which ConPTY gives PSReadLine as
                // Ctrl-Backspace (BackwardKillWord); readline and zsh
                // take ^W.
                NamedKey::Backspace if ctrl => vec![if cfg!(windows) { 0x08 } else { 0x17 }],
                NamedKey::Backspace => esc(vec![0x7f], alt),
                NamedKey::F(_) => return None,
            })
        }
    }
}

/// The bytes F`n` (1 to 12) sends, with its modifiers.
pub(super) fn encode_function(n: u8, shift: bool, alt: bool, ctrl: bool) -> Option<Vec<u8>> {
    let mods = modifier_param(shift, alt, ctrl);
    Some(match n {
        1..=4 => {
            let letter = b"PQRS"[n as usize - 1];
            match mods {
                Some(m) => format!("\x1b[1;{m}{}", letter as char).into_bytes(),
                None => vec![0x1b, b'O', letter],
            }
        }
        5 => csi_tilde(15, mods),
        6..=10 => csi_tilde(n + 11, mods),
        11 | 12 => csi_tilde(n + 12, mods),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode, ctrl: bool, alt: bool) -> KeyPress {
        KeyPress { code, mods: fenix_keymap::Mods { ctrl, alt, ..fenix_keymap::Mods::default() } }
    }
    fn named(n: NamedKey, ctrl: bool, alt: bool) -> KeyPress {
        key(KeyCode::Named(n), ctrl, alt)
    }

    /// What PSReadLine and readline move and select by: Ctrl-arrows by
    /// word, Shift-arrows selecting, both together, Home and End.
    #[test]
    fn modifiers_on_the_arrows_are_xterms() {
        assert_eq!(encode(named(NamedKey::Left, false, false), false, false, false).unwrap(), b"\x1b[D");
        assert_eq!(encode(named(NamedKey::Left, true, false), false, false, false).unwrap(), b"\x1b[1;5D", "Ctrl-Left: a word back");
        assert_eq!(encode(named(NamedKey::Right, false, false), true, false, false).unwrap(), b"\x1b[1;2C", "Shift-Right selects");
        assert_eq!(encode(named(NamedKey::Right, true, false), true, false, false).unwrap(), b"\x1b[1;6C", "Ctrl-Shift-Right selects a word");
        assert_eq!(encode(named(NamedKey::Up, false, true), false, false, false).unwrap(), b"\x1b[1;3A", "Alt-Up");
        assert_eq!(encode(named(NamedKey::Home, false, false), true, false, false).unwrap(), b"\x1b[1;2H");
        assert_eq!(encode(named(NamedKey::Delete, true, false), false, false, false).unwrap(), b"\x1b[3;5~");
        assert_eq!(encode(named(NamedKey::PageUp, false, false), false, false, false).unwrap(), b"\x1b[5~");
    }

    /// A full-screen program that asks for application mode gets `ESC O`;
    /// with a modifier it's the same `ESC [ 1 ;` either way.
    #[test]
    fn application_mode_arrows() {
        assert_eq!(encode(named(NamedKey::Up, false, false), false, true, false).unwrap(), b"\x1bOA");
        assert_eq!(encode(named(NamedKey::End, false, false), false, true, false).unwrap(), b"\x1bOF");
        assert_eq!(encode(named(NamedKey::Up, true, false), false, true, false).unwrap(), b"\x1b[1;5A");
    }

    /// Ctrl-Space is NUL (PSReadLine's menu completion), Shift-Tab goes
    /// back, Alt is an ESC first, Ctrl-Backspace deletes a word.
    #[test]
    fn the_keys_line_editors_bind() {
        assert_eq!(encode(key(KeyCode::Char(' '), true, false), false, false, false).unwrap(), [0]);
        assert_eq!(encode(named(NamedKey::Tab, false, false), true, false, false).unwrap(), b"\x1b[Z");
        assert_eq!(encode(key(KeyCode::Char('b'), false, true), false, false, false).unwrap(), b"\x1bb", "Alt-b: a word back");
        assert_eq!(encode(key(KeyCode::Char('c'), true, false), false, false, false).unwrap(), [3]);
        assert_eq!(encode(key(KeyCode::Char('A'), false, false), true, false, false).unwrap(), b"A");
        assert_eq!(encode(named(NamedKey::Backspace, false, true), false, false, false).unwrap(), b"\x1b\x7f");
        let word = encode(named(NamedKey::Backspace, true, false), false, false, false).unwrap();
        assert_eq!(word, [if cfg!(windows) { 0x08 } else { 0x17 }]);
        assert_eq!(encode(key(KeyCode::Char('@'), true, true), false, false, false).unwrap(), b"@", "AltGr's @ types @");
    }

    /// Under ConPTY, Ctrl-Space is the Windows key event, which PowerShell
    /// reads as `Ctrl+Spacebar`; other keys are VT as before.
    #[test]
    fn ctrl_space_is_a_windows_key_event_under_conpty() {
        assert_eq!(encode(key(KeyCode::Char(' '), true, false), false, false, true).unwrap(), b"\x1b[32;57;32;1;8;1_\x1b[32;57;32;0;8;1_");
        assert_eq!(encode(key(KeyCode::Char(' '), true, false), true, false, true).unwrap(), b"\x1b[32;57;32;1;24;1_\x1b[32;57;32;0;24;1_", "with Shift");
        assert_eq!(encode(key(KeyCode::Char('c'), true, false), false, false, true).unwrap(), [3]);
        assert_eq!(encode(key(KeyCode::Char(' '), false, false), false, false, true).unwrap(), b" ");
    }

    #[test]
    fn function_keys() {
        assert_eq!(encode_function(1, false, false, false).unwrap(), b"\x1bOP");
        assert_eq!(encode_function(4, false, false, true).unwrap(), b"\x1b[1;5S");
        assert_eq!(encode_function(5, false, false, false).unwrap(), b"\x1b[15~");
        assert_eq!(encode_function(6, false, false, false).unwrap(), b"\x1b[17~");
        assert_eq!(encode_function(10, true, false, false).unwrap(), b"\x1b[21;2~");
        assert_eq!(encode_function(11, false, false, false).unwrap(), b"\x1b[23~");
        assert_eq!(encode_function(12, false, false, false).unwrap(), b"\x1b[24~");
        assert_eq!(encode_function(13, false, false, false), None);
    }
}
