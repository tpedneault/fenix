//! `SPC m`, the local leader: one menu whose contents depend on what's
//! focused, the way Doom Emacs' `SPC m` does. The global leader stays the
//! same everywhere; only this branch of it changes, so muscle memory for
//! everything else is safe.

use super::*;

use crate::keymap::LocalContext;

impl App {
    /// Which `SPC m` menus apply to the focused buffer, most specific
    /// first -- the project it belongs to, then its language.
    pub(super) fn local_contexts(&self) -> Vec<LocalContext> {
        let mut contexts = Vec::new();
        if fenix_embedded::project_root_of(&self.integration_root()).is_some() {
            contexts.push(LocalContext::Arduino);
        }
        let language = self.open().buffer.path().and_then(fenix_syntax::detect_language_from_path);
        if language == Some(fenix_syntax::LanguageId::Tcl) {
            contexts.push(LocalContext::Tcl);
        }
        contexts
    }

    /// `SPC m`: opens the menu for the focused buffer's contexts, or says
    /// there isn't one.
    pub(crate) fn start_local_leader(&mut self) {
        let contexts = self.local_contexts();
        if contexts.is_empty() {
            self.local_matcher = None;
            self.set_message("SPC m has nothing for this buffer -- it has menus in Arduino sketches and Tcl files");
            return;
        }
        let names: Vec<&str> = contexts.iter().map(|c| c.name()).collect();
        self.local_matcher = Some(keymap::local_trie(&contexts).matcher());
        self.set_message(format!("SPC m -- {}", names.join(" · ")));
    }

    /// Feeds one key to an open `SPC m` menu: `None` when no menu is open
    /// (the key is someone else's), `Some(None)` when the menu took it
    /// (still open, or closed by a key it doesn't bind -- the same as an
    /// unbound key under the global leader), `Some(Some(id))` when it
    /// resolved to a command for the caller to run.
    pub(super) fn local_leader_key(&mut self, keypress: KeyPress) -> Option<Option<&'static str>> {
        let matcher = self.local_matcher.as_mut()?;
        let resolved = match matcher.feed(keypress) {
            Step::Matched(&id) => {
                self.local_matcher = None;
                Some(id)
            }
            Step::Pending(_) => None,
            Step::NoMatch => {
                self.local_matcher = None;
                if keypress.code != KeyCode::Named(FenixNamedKey::Escape) {
                    self.set_message(format!("SPC m {} isn't bound here", keymap::describe_keypress(&keypress)));
                }
                None
            }
        };
        self.wake_caret();
        Some(resolved)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("fenix-gui-local-leader-{name}-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn open(path: &Path) -> App {
        let mut app = App::with_file(Some(path.to_string_lossy().into_owned()));
        app.local_matcher = None;
        app
    }

    fn hint_labels(app: &App) -> Vec<&'static str> {
        let mut labels: Vec<&str> = app.pending_hints().into_iter().map(|(_, l)| l).collect();
        labels.sort();
        labels
    }

    #[test]
    fn a_sketch_gets_the_arduino_menu() {
        let dir = temp("sketch");
        let ino = fenix_embedded::arduino::create_sketch(&dir, "Blink", "arduino:avr:uno").unwrap();
        let mut app = open(&ino);
        assert_eq!(app.local_contexts(), [LocalContext::Arduino]);

        app.start_local_leader();
        let labels = hint_labels(&app);
        assert!(labels.contains(&"upload to board") && labels.contains(&"serial monitor"), "{labels:?}");
        assert!(!labels.contains(&"lookup telecommand"));
        assert!(app.status_message.as_ref().unwrap().text.contains("arduino"));
    }

    #[test]
    fn a_tcl_file_gets_the_mib_and_tcl_menu() {
        let dir = temp("tcl");
        let file = dir.join("procedure.tcl");
        std::fs::write(&file, "proc main {} {}\n").unwrap();
        let mut app = open(&file);
        assert_eq!(app.local_contexts(), [LocalContext::Tcl]);

        app.start_local_leader();
        let labels = hint_labels(&app);
        for expected in ["insert telecommand", "lookup telecommand", "refresh tags", "symbols"] {
            assert!(labels.contains(&expected), "{expected} missing from {labels:?}");
        }
    }

    #[test]
    fn a_buffer_with_no_menu_says_so_and_opens_nothing() {
        let dir = temp("plain");
        let file = dir.join("notes.md");
        std::fs::write(&file, "# notes\n").unwrap();
        let mut app = open(&file);
        app.start_local_leader();
        assert!(app.local_matcher.is_none());
        assert!(app.pending_hints().is_empty());
        assert!(app.status_message.as_ref().unwrap().text.contains("nothing for this buffer"));
    }

    #[test]
    fn an_unbound_key_closes_the_menu_and_says_which_key() {
        let dir = temp("unbound");
        let file = dir.join("procedure.tcl");
        std::fs::write(&file, "").unwrap();
        let mut app = open(&file);
        app.start_local_leader();
        assert_eq!(app.local_leader_key(KeyPress::char('z')), Some(None));
        assert!(app.local_matcher.is_none());
        assert!(app.status_message.as_ref().unwrap().text.contains("SPC m z isn't bound here"));
        assert_eq!(app.local_leader_key(KeyPress::char('z')), None, "closed menus consume nothing");
    }

    #[test]
    fn a_bound_key_resolves_its_command_and_closes_the_menu() {
        let dir = temp("bound");
        let ino = fenix_embedded::arduino::create_sketch(&dir, "Blink", "arduino:avr:uno").unwrap();
        let mut app = open(&ino);
        app.start_local_leader();
        assert_eq!(app.local_leader_key(KeyPress::char('n')), Some(Some("embedded.new_sketch")));
        assert!(app.local_matcher.is_none());
    }
}
