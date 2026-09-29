//! The files `SPC SPC` (and `SPC p f`, `SPC f a`, `F` in the explorer)
//! pick from, listed off the UI thread and kept per root.
//!
//! Listing a big repository takes a `git ls-files` over all of it --
//! seconds on a large one, on Windows especially. Done inline, the key
//! seemed to do nothing, then the picker appeared all at once. Now the
//! picker opens at once: on the last listing of that root when there is
//! one (refreshed behind it), otherwise empty and saying it's listing,
//! filling in when the listing arrives -- whatever was typed meanwhile
//! still applies. A project's files are also listed as soon as it becomes
//! the current one, so the first `SPC SPC` usually has them already.

use super::*;

/// A root, and whether the listing includes what `.gitignore` hides.
pub(super) type IndexKey = (PathBuf, bool);

/// A listing is refreshed when the picker opens on it and it's older
/// than this; a quick close and reopen doesn't list the files again.
const FRESH_FOR: Duration = Duration::from_secs(10);

#[derive(Default)]
pub(crate) struct FileIndex {
    listed: HashMap<IndexKey, (Arc<Vec<fenix_picker::Candidate<PathBuf>>>, Instant)>,
    /// Listings under way.
    pending: HashSet<IndexKey>,
    /// The open find-file picker's listing, until it arrives.
    pub(super) waiting: Option<IndexKey>,
    /// Whether that picker closes, saying so, when there's nothing to
    /// pick (the explorer's `F`) rather than staying open empty.
    close_if_empty: bool,
}

/// Every file under `root`, labelled relative to it.
fn list(root: &Path, include_ignored: bool) -> Vec<fenix_picker::Candidate<PathBuf>> {
    let _profile = crate::profile::Scope::new("list project files");
    let files =
        if include_ignored { fenix_project::list_project_files_including_ignored(root) } else { fenix_project::list_project_files(root) };
    files
        .into_iter()
        .map(|path| {
            let label = App::relative_label(root, &path);
            fenix_picker::Candidate::new(label, path)
        })
        .collect()
}

impl App {
    /// Where a find-file picker looks: the current project, else the
    /// folder Fenix was started in.
    pub(super) fn find_file_root(&self) -> PathBuf {
        self.project_root.clone().unwrap_or_else(|| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
    }

    /// Opens the find-file picker over `root`'s files at once, listing
    /// them in the background when there's no recent listing.
    pub(super) fn open_find_file(&mut self, root: &Path, include_ignored: bool) {
        self.open_find_file_with(root, include_ignored, false);
    }

    /// `open_find_file`, closing again with a message when `root` turns
    /// out to have no files at all.
    pub(super) fn open_find_file_with(&mut self, root: &Path, include_ignored: bool, close_if_empty: bool) {
        self.file_index.close_if_empty = close_if_empty;
        let key = (root.to_path_buf(), include_ignored);
        let (candidates, fresh) = match self.file_index.listed.get(&key) {
            Some((files, at)) => (files.as_ref().clone(), at.elapsed() < FRESH_FOR),
            None => (Vec::new(), false),
        };
        self.enter_picker(ActivePicker::FindFile(fenix_picker::PickerState::new(candidates)));
        self.file_index.waiting = None;
        if !fresh {
            // A refresh of a listing already shown goes into the picker
            // too when it lands: a file created since then shows up.
            self.file_index.waiting = Some(key.clone());
            self.request_file_listing(key);
        }
    }

    /// Lists `root`'s files ahead of the first `SPC SPC` in it.
    pub(super) fn prewarm_file_index(&mut self, root: &Path) {
        let key = (root.to_path_buf(), false);
        if !self.file_index.listed.contains_key(&key) {
            self.request_file_listing(key);
        }
    }

    fn request_file_listing(&mut self, key: IndexKey) {
        if !self.file_index.pending.insert(key.clone()) {
            return;
        }
        match self.event_proxy.clone() {
            Some(proxy) => {
                let _ = std::thread::Builder::new().name("fenix-list-files".into()).spawn(move || {
                    let files = list(&key.0, key.1);
                    let _ = proxy.send_event(FenixUserEvent::FilesListed { root: key.0, include_ignored: key.1, files });
                });
            }
            // No event loop (every test): list inline.
            None => {
                let files = list(&key.0, key.1);
                self.apply_files_listed(key.0, key.1, files);
            }
        }
    }

    pub(super) fn apply_files_listed(&mut self, root: PathBuf, include_ignored: bool, files: Vec<fenix_picker::Candidate<PathBuf>>) {
        let key = (root, include_ignored);
        self.file_index.pending.remove(&key);
        if self.file_index.waiting.as_ref() == Some(&key) {
            self.file_index.waiting = None;
            if let Some(ActivePicker::FindFile(state)) = &mut self.active_picker {
                if files.is_empty() && self.file_index.close_if_empty {
                    // Nothing to pick from: say so rather than leave an
                    // empty picker open.
                    self.picker_cancel();
                    self.set_message(format!("nothing under {}", readable_path(&key.0)));
                } else {
                    state.replace_candidates(files.clone());
                }
            }
        }
        self.file_index.listed.insert(key, (Arc::new(files), Instant::now()));
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    /// The find-file picker's modeline note while its files are listed.
    pub(super) fn file_listing_note(&self) -> Option<&'static str> {
        let key = self.file_index.waiting.as_ref()?;
        let shown = matches!(&self.active_picker, Some(ActivePicker::FindFile(state)) if state.total() > 0);
        Some(if shown && self.file_index.listed.contains_key(key) { "refreshing" } else { "listing the files…" })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_picker_opens_on_the_listing_and_keeps_what_was_typed_when_it_is_refreshed() {
        let dir = std::env::temp_dir().join(format!("fenix-file-index-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("alpha.rs"), "").unwrap();
        std::fs::write(dir.join("beta.rs"), "").unwrap();
        let mut app = App::with_file(None);
        app.open_find_file(&dir, false);
        assert!(app.file_index.waiting.is_none(), "listed inline, with no event loop");
        let Some(ActivePicker::FindFile(state)) = &mut app.active_picker else { panic!("a find-file picker") };
        assert_eq!(state.total(), 2);
        state.push_char('b');

        // A listing landing while the picker is open: what's typed stays.
        std::fs::write(dir.join("bravo.rs"), "").unwrap();
        app.file_index.waiting = Some((dir.clone(), false));
        app.apply_files_listed(dir.clone(), false, list(&dir, false));
        let Some(ActivePicker::FindFile(state)) = &app.active_picker else { panic!() };
        assert_eq!(state.query(), "b");
        assert_eq!(state.len(), 2, "beta.rs and the new bravo.rs");
        assert!(app.file_listing_note().is_none());

        // Reopened straight away: the listing it has, no new one.
        app.picker_cancel();
        app.open_find_file(&dir, false);
        assert!(matches!(&app.active_picker, Some(ActivePicker::FindFile(state)) if state.total() == 3));
        std::fs::remove_dir_all(&dir).ok();
    }
}
