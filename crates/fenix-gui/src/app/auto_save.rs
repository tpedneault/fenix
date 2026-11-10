//! Auto-save (`editor.auto_save`): a file's unsaved edits written for
//! you, after a pause in editing (`after_delay`, `editor.auto_save_delay_ms`)
//! or when you leave it -- another buffer, another pane, another
//! application (`on_focus_change`). Off unless you turn it on.
//!
//! Only a file's own text is saved: not a buffer with no file yet, nor a
//! file that wasn't there when it was opened (its first save is yours),
//! not one Fenix shows read-only, not a remote project's file (its
//! machine can go away mid-write), and never one that changed on disk
//! since it was read: that, once found, holds auto-save back for the
//! buffer until you've settled it yourself (`:w!` or `:e!`). A save that
//! fails says so once and holds it back the same way, until a save of
//! yours goes through.
//! Auto-saving doesn't format (`editor.format_on_save` is for the saves
//! you make: text moving under you while you type is worse than an
//! unformatted file).

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use super::{App, BufferId, DiskSweep};

/// When auto-save saves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AutoSave {
    Off,
    AfterDelay(Duration),
    OnFocusChange,
}

/// The shortest pause `after_delay` waits.
const MIN_DELAY: Duration = Duration::from_millis(100);

/// What auto-save has seen of each buffer.
#[derive(Debug, Default)]
pub(crate) struct AutoSaveState {
    /// Each dirty buffer's edit count, and when it was first seen at it.
    edits: HashMap<BufferId, (u64, Instant)>,
    /// The buffer that had the keyboard at the last look.
    focused: Option<BufferId>,
    /// Held back until you save (or reload) it yourself: it changed on
    /// disk, or a save of it failed.
    held: HashSet<BufferId>,
    /// Edited by what its own save set off (an extension that formats on
    /// save, say), at this edit count: not saved again for that.
    settled_at: HashMap<BufferId, u64>,
}

impl App {
    /// `editor.auto_save` and its delay.
    pub(crate) fn auto_save_mode(&self) -> AutoSave {
        match self.config.text_value("editor.auto_save") {
            Some("after_delay") => {
                let ms = self.config.int_value("editor.auto_save_delay_ms").unwrap_or(1000).max(0) as u64;
                AutoSave::AfterDelay(Duration::from_millis(ms).max(MIN_DELAY))
            }
            Some("on_focus_change") => AutoSave::OnFocusChange,
            _ => AutoSave::Off,
        }
    }

    /// A save of yours: whatever held auto-save back for `id` is settled.
    pub(crate) fn auto_save_settled(&mut self, id: BufferId) {
        self.auto_save.held.remove(&id);
        self.auto_save.settled_at.remove(&id);
    }

    /// Whether `id` is a buffer auto-save may write: a local file's text,
    /// edited, that was on disk when it was read, that Fenix doesn't show
    /// read-only, and that nothing holds back.
    fn auto_saves(&mut self, id: BufferId) -> bool {
        if self.externally_changed.contains(&id) {
            // Found changed on disk: never auto-saved over until you say.
            self.auto_save.held.insert(id);
        }
        let Some(ob) = self.buffers.get(id) else { return false };
        let Some(path) = ob.buffer.path() else { return false };
        ob.buffer.is_dirty()
            && ob.kind.tracks_unsaved_changes()
            && !self.auto_save.held.contains(&id)
            && self.disk_state.contains_key(&id)
            && !self.buffer_edits_are_reverted(id, ob.kind)
            && !fenix_vfs::is_remote(path)
            && !self.is_kept_compose(id)
            && self.code_scratch_name(id).is_none()
            && self.tutorial_buffer_name(id).is_none()
            && self.auto_save.settled_at.get(&id) != Some(&ob.buffer.edit_count())
    }

    /// Writes `id` for auto-save, if the file on disk is still the one it
    /// was read from: true when it was written.
    pub(crate) fn auto_save_buffer(&mut self, id: BufferId) -> bool {
        if !self.auto_saves(id) {
            return false;
        }
        let mut sweep = DiskSweep::default();
        self.check_one_file(id, &mut sweep, false, None);
        if self.externally_changed.contains(&id) {
            self.auto_save.held.insert(id);
            return false;
        }
        if !sweep.reloaded.is_empty() {
            return false;
        }
        let Some(ob) = self.buffers.get_mut(id) else { return false };
        match ob.buffer.save() {
            Ok(()) => {
                let Some(path) = ob.buffer.path().map(std::path::Path::to_path_buf) else { return true };
                self.auto_save.edits.remove(&id);
                // The same as a save of yours from here on, and the keys
                // read again when it's your keybindings.toml.
                self.after_saved(id, &path);
                self.reload_keybindings_if(&path);
                // What the save itself set off edited it again: not a
                // reason to save it again, or a loop.
                if let Some(ob) = self.buffers.get(id).filter(|ob| ob.buffer.is_dirty()) {
                    self.auto_save.settled_at.insert(id, ob.buffer.edit_count());
                }
                true
            }
            Err(err) => {
                self.auto_save.held.insert(id);
                let name = self.buffer_display_name(id);
                self.set_error(format!("auto-save couldn't write {name}: {err} -- it waits until you save it yourself"));
                false
            }
        }
    }

    /// The dirty buffers that could be auto-saved, in no order.
    fn auto_save_candidates(&self) -> Vec<BufferId> {
        self.buffers.ids().filter(|&id| self.buffers.get(id).is_some_and(|ob| ob.buffer.is_dirty() && ob.buffer.path().is_some() && ob.kind.tracks_unsaved_changes())).collect()
    }

    /// Every buffer auto-save may write, written: the window has lost the
    /// keyboard, with `on_focus_change`.
    pub(crate) fn auto_save_all(&mut self) {
        if self.auto_save_mode() != AutoSave::OnFocusChange {
            return;
        }
        for id in self.auto_save_candidates() {
            self.auto_save_buffer(id);
        }
    }

    /// Looked at as the event loop goes to sleep: with `on_focus_change`,
    /// the buffer just left is saved; with `after_delay`, each one whose
    /// edits have rested long enough. When to look again, if there's
    /// something waiting.
    pub(crate) fn auto_save_tick(&mut self, now: Instant) -> Option<Instant> {
        let mode = self.auto_save_mode();
        let focused = Some(self.focused_buffer_id());
        let left = std::mem::replace(&mut self.auto_save.focused, focused).filter(|was| Some(*was) != focused);
        // What a clean buffer held back is settled: :e!, or a save.
        let clean = |app: &App, id: &BufferId| app.buffers.get(*id).is_none_or(|ob| !ob.buffer.is_dirty());
        let settled: Vec<BufferId> = self.auto_save.held.iter().filter(|id| clean(self, id)).copied().collect();
        settled.into_iter().for_each(|id| self.auto_save_settled(id));
        match mode {
            AutoSave::Off => {
                self.auto_save.edits.clear();
                None
            }
            AutoSave::OnFocusChange => {
                if let Some(id) = left {
                    self.auto_save_buffer(id);
                }
                None
            }
            AutoSave::AfterDelay(delay) => {
                let dirty = self.auto_save_candidates();
                self.auto_save.edits.retain(|id, _| dirty.contains(id));
                let mut next: Option<Instant> = None;
                for id in dirty {
                    if !self.auto_saves(id) {
                        // Waiting on nothing: a held one is looked at
                        // again only once it's settled.
                        self.auto_save.edits.remove(&id);
                        continue;
                    }
                    let edits = self.buffers.get(id).map_or(0, |ob| ob.buffer.edit_count());
                    let seen = match self.auto_save.edits.get(&id) {
                        Some(&(e, at)) if e == edits => at,
                        _ => {
                            self.auto_save.edits.insert(id, (edits, now));
                            now
                        }
                    };
                    let due = seen + delay;
                    if due <= now {
                        self.auto_save_buffer(id);
                    } else {
                        next = Some(next.map_or(due, |n| n.min(due)));
                    }
                }
                next
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_keys::app_on_file;
    use super::*;

    fn auto_save(app: &mut App, how: &str, delay_ms: Option<u64>) {
        app.config.set_value("editor.auto_save", Some(fenix_config::Value::Text(how.into())));
        if let Some(ms) = delay_ms {
            app.config.set_value("editor.auto_save_delay_ms", Some(fenix_config::Value::Int(ms as i64)));
        }
    }

    #[test]
    fn off_unless_turned_on() {
        let (mut app, f) = app_on_file("a.txt", "a\n");
        assert_eq!(app.auto_save_mode(), AutoSave::Off);
        app.press_all("i x Esc");
        let now = Instant::now();
        assert_eq!(app.auto_save_tick(now), None);
        assert_eq!(app.auto_save_tick(now + Duration::from_secs(60)), None);
        assert_eq!(std::fs::read_to_string(&f.path).unwrap(), "a\n");
        assert!(app.open().buffer.is_dirty());
    }

    #[test]
    fn after_a_pause_in_editing_the_file_is_saved() {
        let (mut app, f) = app_on_file("a.txt", "a\n");
        auto_save(&mut app, "after_delay", Some(500));
        app.press_all("i x");
        let now = Instant::now();
        let due = app.auto_save_tick(now).expect("waits for the pause");
        assert_eq!(due, now + Duration::from_millis(500));
        assert!(app.open().buffer.is_dirty(), "not yet");
        app.press("y");
        assert!(app.auto_save_tick(now + Duration::from_millis(400)).is_some(), "an edit starts the wait again");
        assert!(app.open().buffer.is_dirty());
        assert_eq!(app.auto_save_tick(now + Duration::from_millis(1000)), None);
        assert!(!app.open().buffer.is_dirty());
        assert_eq!(std::fs::read_to_string(&f.path).unwrap(), "xya\n");
    }

    #[test]
    fn a_file_changed_on_disk_is_never_saved_over() {
        let (mut app, f) = app_on_file("a.txt", "a\n");
        auto_save(&mut app, "after_delay", Some(100));
        app.poll_files_changed_on_disk();
        app.press_all("i x");
        std::fs::write(&f.path, "theirs, longer\n").unwrap();
        let later = std::time::SystemTime::now() + Duration::from_secs(5);
        std::fs::File::options().write(true).open(&f.path).unwrap().set_modified(later).unwrap();
        let now = Instant::now();
        app.auto_save_tick(now);
        app.auto_save_tick(now + Duration::from_secs(1));
        assert_eq!(std::fs::read_to_string(&f.path).unwrap(), "theirs, longer\n", "theirs is kept");
        assert!(app.open().buffer.is_dirty(), "and yours");
        assert!(app.externally_changed.contains(&app.focused_buffer_id()));
    }

    #[test]
    fn leaving_the_buffer_saves_it_with_on_focus_change() {
        let (mut app, f) = app_on_file("a.txt", "a\n");
        auto_save(&mut app, "on_focus_change", None);
        let now = Instant::now();
        app.auto_save_tick(now);
        app.press_all("i x Esc");
        app.auto_save_tick(now);
        assert!(app.open().buffer.is_dirty(), "still here: not saved");
        app.run_cmd("buffer.scratch");
        app.auto_save_tick(now);
        assert_eq!(std::fs::read_to_string(&f.path).unwrap(), "xa\n", "left: saved");
        // A buffer with no file is never written.
        app.press_all("i y Esc");
        app.auto_save_all();
        assert!(app.open().buffer.is_dirty());
    }

    /// Changes `path` on disk as another program would: other text, a
    /// later time.
    fn changed_elsewhere(path: &std::path::Path, text: &str) {
        std::fs::write(path, text).unwrap();
        let later = std::time::SystemTime::now() + Duration::from_secs(5);
        std::fs::File::options().write(true).open(path).unwrap().set_modified(later).unwrap();
    }

    #[test]
    fn a_conflict_holds_auto_save_back_even_after_opening_the_file_again() {
        let (mut app, f) = app_on_file("a.txt", "a\n");
        auto_save(&mut app, "after_delay", Some(100));
        app.press_all("i x Esc");
        changed_elsewhere(&f.path, "theirs, longer\n");
        app.poll_files_changed_on_disk();
        assert!(app.externally_changed.contains(&app.focused_buffer_id()), "found");
        let now = Instant::now();
        app.auto_save_tick(now);
        // Opened again -- a picker, a link -- in the same buffer.
        app.open_text_file(&f.path, false);
        assert!(app.externally_changed.contains(&app.focused_buffer_id()), "opening it again doesn't forget the conflict");
        app.auto_save_tick(now + Duration::from_secs(1));
        app.auto_save_tick(now + Duration::from_secs(2));
        assert_eq!(std::fs::read_to_string(&f.path).unwrap(), "theirs, longer\n", "theirs is kept");
        assert!(app.open().buffer.is_dirty(), "and yours");
        // Settled by you (:w!): auto-save goes on after that.
        app.save_force();
        assert_eq!(std::fs::read_to_string(&f.path).unwrap(), "xa\n");
        app.press_all("i y Esc");
        app.auto_save_tick(now + Duration::from_secs(3));
        app.auto_save_tick(now + Duration::from_secs(4));
        assert_eq!(std::fs::read_to_string(&f.path).unwrap(), "yxa\n");
    }

    #[test]
    fn a_new_file_s_first_save_is_yours_and_one_made_meanwhile_is_kept() {
        let (mut app, f) = app_on_file("a.txt", "");
        let new = f.path.with_file_name("new.md");
        app.open_text_file(&new, false);
        auto_save(&mut app, "after_delay", Some(100));
        app.press_all("i x Esc");
        let now = Instant::now();
        app.auto_save_tick(now);
        app.auto_save_tick(now + Duration::from_secs(1));
        assert!(!new.exists(), "not made by auto-save");
        std::fs::write(&new, "from git pull\n").unwrap();
        app.auto_save_tick(now + Duration::from_secs(2));
        app.auto_save_tick(now + Duration::from_secs(3));
        assert_eq!(std::fs::read_to_string(&new).unwrap(), "from git pull\n", "someone else's file, kept");
    }

    #[test]
    fn a_save_that_fails_says_so_once_and_waits_for_yours() {
        let (mut app, f) = app_on_file("a.txt", "a\n");
        auto_save(&mut app, "after_delay", Some(100));
        let latin = fenix_core::TextEncoding::choices().into_iter().find(|e| matches!(e, fenix_core::TextEncoding::Legacy(_))).unwrap();
        app.open_mut().buffer.set_encoding(latin);
        app.press_all("i");
        app.type_text("€✓");
        app.press("Esc");
        let now = Instant::now();
        app.auto_save_tick(now);
        app.auto_save_tick(now + Duration::from_secs(1));
        assert!(app.modeline_text().contains("auto-save couldn't write"), "{}", app.modeline_text());
        assert!(app.auto_save.held.contains(&app.focused_buffer_id()));
        app.set_message("");
        app.press_all("i z Esc");
        app.auto_save_tick(now + Duration::from_secs(2));
        app.auto_save_tick(now + Duration::from_secs(3));
        assert!(!app.modeline_text().contains("auto-save"), "not again after every pause: {}", app.modeline_text());
        assert_eq!(std::fs::read_to_string(&f.path).unwrap(), "a\n");
    }

    #[test]
    fn a_remote_file_and_one_shown_read_only_are_left_alone() {
        let (mut app, _f) = app_on_file("a.txt", "a\n");
        auto_save(&mut app, "after_delay", Some(100));
        app.press_all("i x Esc");
        let id = app.focused_buffer_id();
        assert!(app.auto_saves(id), "an ordinary file: yes");
        app.ref_view_names.insert(id, "HEAD".into());
        assert!(!app.auto_saves(id), "a file as it is at a ref: no");
        app.ref_view_names.remove(&id);
        // A remote project's file, as if it had been read from there: no.
        let mut remote = fenix_core::Buffer::from_text_at(fenix_vfs::remote("host", "/srv/a.txt"), "a\n");
        let mut c = fenix_core::Cursor::at_start();
        remote.insert_char(&mut c, 'x');
        let rid = app.buffers.insert_loaded_document(remote);
        let seen = app.disk_state.get(&id).copied().unwrap();
        app.disk_state.insert(rid, seen);
        assert!(!app.auto_saves(rid), "a remote project's file: no");
    }

    #[test]
    fn a_read_only_buffer_or_one_with_no_file_is_left_alone() {
        let mut app = App::with_file(None);
        auto_save(&mut app, "after_delay", Some(100));
        app.press_all("i x");
        let now = Instant::now();
        app.auto_save_tick(now);
        assert_eq!(app.auto_save_tick(now + Duration::from_secs(1)), None);
        assert!(!app.auto_saves(app.focused_buffer_id()), "no file");
    }
}
