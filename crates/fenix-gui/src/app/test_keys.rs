//! Test helpers that drive the real key path -- `dispatch_keypress`,
//! `route_keypress`, the chords, the leader, the policies -- with no event
//! loop, as a person typing would.

#![cfg(test)]

use std::path::PathBuf;

use fenix_keymap::KeyPress;

use super::App;

/// A file in a directory of its own, removed with it.
pub(crate) struct TestFile {
    dir: PathBuf,
    pub(crate) path: PathBuf,
}

impl Drop for TestFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// An app on a file called `name` holding `text`, the cursor at its
/// start. The file's kept until the `TestFile` goes.
pub(crate) fn app_on_file(name: &str, text: &str) -> (App, TestFile) {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!("fenix-keys-test-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, text).unwrap();
    let app = App::with_file(Some(path.to_string_lossy().into_owned()));
    (app, TestFile { dir, path })
}

impl App {
    /// One key, written as the keymap writes it (`C-s`, `S-Down`, `F12`,
    /// `x`, `SPC`, `Esc`), through the real key path.
    pub(crate) fn press(&mut self, key: &str) {
        let kp = crate::keymap::parse_key(key).unwrap_or_else(|| panic!("{key} isn't a key"));
        self.press_key(kp);
    }

    /// Several keys, space-separated, as `press` takes them.
    pub(crate) fn press_all(&mut self, keys: &str) {
        for key in keys.split_whitespace() {
            self.press(key);
        }
    }

    /// `kp` through the real key path.
    pub(crate) fn press_key(&mut self, kp: KeyPress) {
        // A plain character typed with Shift held arrives as its capital,
        // with no Shift: as `to_keypress` makes it.
        self.dispatch_keypress(kp, None);
    }

    /// Text typed a character at a time (a space is the space key).
    pub(crate) fn type_text(&mut self, text: &str) {
        for c in text.chars() {
            let kp = match c {
                '\n' => KeyPress::named(fenix_keymap::NamedKey::Enter),
                '\t' => KeyPress::named(fenix_keymap::NamedKey::Tab),
                c => KeyPress::char(c),
            };
            self.press_key(kp);
        }
    }

    /// Runs command `id`, with no event loop.
    pub(crate) fn run_cmd(&mut self, id: &str) {
        assert!(self.run_command_with(None, id), "{id} isn't a command");
    }

    /// The focused buffer's text.
    pub(crate) fn text(&self) -> String {
        self.open().buffer.rope().to_string()
    }

    /// The focused pane's cursor, as (line, column).
    pub(crate) fn line_col(&self) -> (usize, usize) {
        let pane = self.focused_pane_id();
        let cursor = &self.workspaces.active_pane_states()[&pane].cursor;
        self.open().buffer.line_col(cursor)
    }
}
