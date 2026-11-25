//! A Mac's keyboard: what Command does where the chords
//! (`mac_keymap.toml`) don't say, and which Option keys are Alt. The
//! decisions are plain functions, tested on every system; only a Mac
//! (`keymap::MAC`) asks them.

use fenix_keymap::{KeyCode, KeyPress, NamedKey};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{ModifiersKeyState, ModifiersState};

use super::pages::PageModel;
use super::App;
use crate::keymap::{self, MAC};

/// What has the keyboard, as far as Command is concerned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Holder {
    /// A prompt, a picker, a confirmation.
    Prompt,
    Terminal,
    /// An extension's page, which is told about Command itself.
    CodePage,
    /// Another page, or the sidebar.
    Page,
    Text,
}

/// What a key held with Command is, where no chord took it.
fn command_key(key: KeyPress, holder: Holder) -> Option<KeyPress> {
    let mut plain = key;
    plain.mods.super_ = false;
    match key.code {
        // A prompt and a page paste with Ctrl+V.
        KeyCode::Char('v' | 'V') if matches!(holder, Holder::Prompt | Holder::Page | Holder::CodePage) => Some(KeyPress::char('v').with_ctrl()),
        // A terminal reads its own copy and paste, and drops the rest.
        _ if holder == Holder::Terminal => Some(key),
        KeyCode::Named(NamedKey::Left) => Some(KeyPress { code: KeyCode::Named(NamedKey::Home), ..plain }),
        KeyCode::Named(NamedKey::Right) => Some(KeyPress { code: KeyCode::Named(NamedKey::End), ..plain }),
        KeyCode::Named(NamedKey::Up) => Some(KeyPress { code: KeyCode::Named(NamedKey::Home), ..plain }.with_ctrl()),
        KeyCode::Named(NamedKey::Down) => Some(KeyPress { code: KeyCode::Named(NamedKey::End), ..plain }.with_ctrl()),
        _ if holder == Holder::CodePage => Some(key),
        // A letter nothing binds isn't typed: Command never types.
        KeyCode::Char(_) => None,
        KeyCode::Named(_) => Some(plain),
    }
}

/// Which Option keys are Alt (`keys.option_as_alt`): (left, right).
fn option_is_alt(setting: Option<&str>) -> (bool, bool) {
    match setting {
        Some("none") => (false, false),
        Some("right") => (false, true),
        Some("both") => (true, true),
        _ => (true, false),
    }
}

/// Whether the Option held is only for its characters: `(left, right)`
/// held, and which are Alt.
fn composes(held: (bool, bool), alt: (bool, bool)) -> bool {
    (held.0 || held.1) && !((held.0 && alt.0) || (held.1 && alt.1))
}

impl App {
    /// Whether the key that follows links and goes to definitions with a
    /// click is held: Ctrl, and Command on a Mac.
    pub(super) fn ctrl_or_command(&self) -> bool {
        self.modifiers.control_key() || (MAC && self.modifiers.super_key())
    }

    /// The modifiers a key is read with: an Option held only for its
    /// characters isn't Alt.
    pub(super) fn key_modifiers(&self) -> ModifiersState {
        let mut mods = self.modifiers;
        if MAC && self.option_composes {
            mods.remove(ModifiersState::ALT);
        }
        mods
    }

    pub(super) fn note_option_keys(&mut self, modifiers: &winit::event::Modifiers) {
        let held = (modifiers.lalt_state() == ModifiersKeyState::Pressed, modifiers.ralt_state() == ModifiersKeyState::Pressed);
        self.option_composes = composes(held, option_is_alt(self.config.text_value("keys.option_as_alt")));
    }

    /// Every window set up for typing on a Mac, as the settings have it
    /// now: its Option keys, and the input method, which dead keys (`´`
    /// then `e`) go through there.
    #[cfg(target_os = "macos")]
    pub(super) fn sync_mac_windows(&mut self) {
        use winit::platform::macos::{OptionAsAlt, WindowExtMacOS};
        let want = match option_is_alt(self.config.text_value("keys.option_as_alt")) {
            (true, true) => OptionAsAlt::Both,
            (true, false) => OptionAsAlt::OnlyLeft,
            (false, true) => OptionAsAlt::OnlyRight,
            (false, false) => OptionAsAlt::None,
        };
        let unsaved = !self.dirty_tracked_buffer_ids().is_empty();
        crate::mac::set_unsaved(unsaved);
        for frame in 0..self.frames.len() {
            let Some(window) = self.frame_window(frame) else { continue };
            if window.option_as_alt() != want {
                window.set_option_as_alt(want);
            }
            if window.is_document_edited() != unsaved {
                window.set_document_edited(unsaved);
            }
            if !self.mac_windows.contains(&window.id()) {
                window.set_ime_allowed(true);
                self.mac_windows.push(window.id());
            }
        }
    }

    /// Whether a prompt or a picker takes every key, before any chord.
    fn prompt_has_keyboard(&self) -> bool {
        self.quit_confirm
            || self.explorer_prompt.is_some()
            || self.explorer_conflict.is_some()
            || self.rename_confirm.is_some()
            || self.git_confirm.is_some()
            || self.git_prompt.is_some()
            || self.wasm.prompt.is_some()
            || self.find_file_prompt.is_some()
            || self.rename_file_prompt.is_some()
            || self.lsp_rename_prompt.is_some()
            || self.goto_line_prompt.is_some()
            || self.workspace_rename_prompt.is_some()
            || self.delete_file_confirm
            || self.server_install.confirm.is_some()
            || self.active_picker.is_some()
    }

    /// A key held with Command, on a Mac: its chord's command run, or the
    /// key it stands for here. `None` when that was all.
    pub(super) fn mac_key(&mut self, key: KeyPress, event_loop: Option<&ActiveEventLoop>) -> Option<KeyPress> {
        if !MAC || !key.mods.super_ || self.binding_keys.is_some() || self.describing_keys.is_some() {
            return Some(key);
        }
        if let Some(window) = self.window.as_ref().filter(|_| !key.mods.alt && !key.mods.shift) {
            match key.code {
                KeyCode::Char('m') if !key.mods.ctrl => {
                    window.set_minimized(true);
                    return None;
                }
                KeyCode::Char('f') if key.mods.ctrl => {
                    let full = window.fullscreen().is_some();
                    window.set_fullscreen((!full).then_some(winit::window::Fullscreen::Borderless(None)));
                    return None;
                }
                _ => {}
            }
        }
        let context = self.chord_context();
        let id = self.focused_buffer_id();
        // As `route_keypress` orders them: a terminal, then a prompt.
        let holder = if context.terminal {
            Holder::Terminal
        } else if self.prompt_has_keyboard() {
            Holder::Prompt
        } else if matches!(self.pages.get(&id).map(|s| &s.model), Some(PageModel::Code(_))) && !self.sidebar_focused {
            Holder::CodePage
        } else if context.text {
            Holder::Text
        } else {
            Holder::Page
        };
        if holder == Holder::Text {
            // A picture pasted into an agent's prompt; what's written sent.
            let ctrl_v = KeyPress::char('v').with_ctrl();
            if key.chord().code == KeyCode::Char('v') && self.agents_composer_key(ctrl_v) {
                self.wake_caret();
                return None;
            }
            let composing = self.compose.as_ref().is_some_and(|c| c.pane == self.focused_pane_id());
            if composing && key.code == KeyCode::Named(NamedKey::Enter) {
                return Some(KeyPress::named(NamedKey::Enter).with_ctrl());
            }
            // The leader's chord with Command for its Ctrl.
            let mut leader = key.chord();
            leader.mods.super_ = false;
            leader.mods.ctrl = true;
            if self.standard_keys() && leader == self.leader_chord().chord() {
                return Some(leader);
            }
        }
        // Quitting reaches through a prompt: it asks first.
        let through = holder != Holder::Prompt || key == keymap::parse_key("s-q").unwrap_or(key);
        if let Some(command) = self.chord_command(key).filter(|_| through) {
            self.leader_matcher.cancel();
            self.local_matcher = None;
            if !super::carets::keeps_carets(command) {
                self.clear_carets();
            }
            self.run_command_with(event_loop, command);
            self.wake_caret();
            return None;
        }
        command_key(key, holder)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command(code: KeyCode) -> KeyPress {
        let mut key = KeyPress { code, mods: Default::default() };
        key.mods.super_ = true;
        key
    }

    #[test]
    fn command_never_types_and_pastes_where_ctrl_does() {
        let v = command(KeyCode::Char('v'));
        assert_eq!(command_key(v, Holder::Prompt), Some(KeyPress::char('v').with_ctrl()));
        assert_eq!(command_key(v, Holder::Page), Some(KeyPress::char('v').with_ctrl()));
        assert_eq!(command_key(v, Holder::Terminal), Some(v), "its own paste");
        assert_eq!(command_key(command(KeyCode::Char('j')), Holder::Text), None);
        assert_eq!(command_key(command(KeyCode::Char('j')), Holder::Prompt), None);
        assert_eq!(command_key(command(KeyCode::Named(NamedKey::Left)), Holder::Text), Some(KeyPress::named(NamedKey::Home)));
        assert_eq!(command_key(command(KeyCode::Named(NamedKey::Down)).with_shift(), Holder::Text), Some(KeyPress::named(NamedKey::End).with_ctrl().with_shift()));
    }

    #[test]
    fn an_option_that_isn_t_alt_is_for_its_characters() {
        let alt = option_is_alt(None);
        assert_eq!(alt, (true, false), "the left is Alt unless you say");
        assert!(!composes((true, false), alt));
        assert!(composes((false, true), alt));
        assert!(!composes((false, false), alt));
        assert!(composes((true, false), option_is_alt(Some("none"))));
    }
}
