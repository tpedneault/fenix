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
        let mut contexts: Vec<LocalContext> = Vec::new();
        // The code extensions' contexts, by their markers.
        let here = self.open().buffer.path().map(Path::to_path_buf).unwrap_or_else(|| self.integration_root());
        let language = self.open().buffer.path().and_then(fenix_syntax::detect_language_from_path).map(crate::lsp::language_config_name);
        // One of an extension's own pages, by its kind.
        let page = match self.pages.get(&self.focused_buffer_id()).map(|s| &s.model) {
            Some(super::pages::PageModel::Code(p)) => Some((p.ext.as_str(), p.kind.as_str())),
            _ => None,
        };
        for ext in &self.wasm.installed {
            for context in &ext.manifest.contributes.contexts {
                let by_language = language.as_ref().is_some_and(|l| context.languages.contains(l));
                let by_markers = !context.markers.is_empty() && context.root_of(&here).is_some();
                let by_page = page.is_some_and(|(owner, kind)| owner == ext.id && context.pages.iter().any(|k| k == kind));
                if !contexts.iter().any(|c| c.name() == context.name) && (by_language || by_markers || by_page) {
                    contexts.push(LocalContext::named(&context.name));
                }
            }
        }
        let language = self.open().buffer.path().and_then(fenix_syntax::detect_language_from_path);
        let reading = matches!(self.pages.get(&self.focused_buffer_id()).map(|s| &s.model), Some(super::pages::PageModel::Reading(_)));
        if language == Some(fenix_syntax::LanguageId::Markdown) || reading {
            contexts.push(LocalContext::Markdown);
        }
        contexts
    }

    /// `SPC m`: opens the menu for the focused buffer's contexts, or says
    /// there isn't one.
    pub(crate) fn start_local_leader(&mut self) {
        let contexts = self.local_contexts();
        if contexts.is_empty() {
            self.local_matcher = None;
            self.set_message("SPC m has nothing for this buffer -- it has menus in PDFs, Markdown, diagrams, Arduino sketches and Tcl files");
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
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn open(path: &Path) -> App {
        let mut app = App::with_file(Some(path.to_string_lossy().into_owned()));
        app.local_matcher = None;
        app
    }

    #[test]
    fn a_buffer_with_no_menu_says_so_and_opens_nothing() {
        let dir = temp("plain");
        let file = dir.join("notes.txt");
        std::fs::write(&file, "notes\n").unwrap();
        let mut app = open(&file);
        app.start_local_leader();
        assert!(app.local_matcher.is_none());
        assert!(app.which_key_pending().is_none());
        assert!(app.status_message.as_ref().unwrap().text.contains("nothing for this buffer"));
    }

    #[test]
    fn an_unbound_key_closes_the_menu_and_says_which_key() {
        let dir = temp("unbound");
        let file = dir.join("notes.md");
        std::fs::write(&file, "# Notes
").unwrap();
        let mut app = open(&file);
        app.start_local_leader();
        assert_eq!(app.local_leader_key(KeyPress::char('z')), Some(None));
        assert!(app.local_matcher.is_none());
        assert!(app.status_message.as_ref().unwrap().text.contains("SPC m z isn't bound here"));
        assert_eq!(app.local_leader_key(KeyPress::char('z')), None, "closed menus consume nothing");
    }

}
