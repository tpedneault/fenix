//! Auto-save (`editor.auto_save`): a file's unsaved edits written for
//! you, after a pause in editing (`after_delay`, `editor.auto_save_delay_ms`)
//! or when you leave it -- another buffer, another pane, another
//! application (`on_focus_change`). Off unless you turn it on.
//!
//! Only a file's own text is saved: not a buffer with no file yet, not
//! one Fenix shows read-only, not a remote project's file (its machine
//! can go away mid-write), and never one that changed on disk since it
//! was read -- that's left for you to settle, as a save would refuse.
//! Auto-saving doesn't format (`editor.format_on_save` is for the saves
//! you make: text moving under you while you type is worse than an
//! unformatted file).

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
    edits: std::collections::HashMap<BufferId, (u64, Instant)>,
    /// The buffer that had the keyboard at the last look.
    focused: Option<BufferId>,
    /// A buffer that couldn't be saved, at the edit count it failed at:
    /// not tried again until it's edited again.
    failed: std::collections::HashMap<BufferId, u64>,
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

    /// Whether `id` is a buffer auto-save may write: a local file's text,
    /// edited, that Fenix doesn't show read-only.
    fn auto_saves(&self, id: BufferId) -> bool {
        let Some(ob) = self.buffers.get(id) else { return false };
        let Some(path) = ob.buffer.path() else { return false };
        ob.buffer.is_dirty()
            && ob.kind.tracks_unsaved_changes()
            && !self.buffer_edits_are_reverted(id, ob.kind)
            && !fenix_vfs::is_remote(path)
            && !self.externally_changed.contains(&id)
            && !self.is_kept_compose(id)
            && self.code_scratch_name(id).is_none()
            && self.tutorial_buffer_name(id).is_none()
            && self.auto_save.failed.get(&id) != Some(&ob.buffer.edit_count())
    }

    /// Writes `id` for auto-save, if the file on disk is still the one it
    /// was read from: true when it was written.
    pub(crate) fn auto_save_buffer(&mut self, id: BufferId) -> bool {
        if !self.auto_saves(id) {
            return false;
        }
        let mut sweep = DiskSweep::default();
        self.check_one_file(id, &mut sweep, false, None);
        if self.externally_changed.contains(&id) || !sweep.reloaded.is_empty() {
            return false;
        }
        let Some(ob) = self.buffers.get_mut(id) else { return false };
        let edits = ob.buffer.edit_count();
        match ob.buffer.save() {
            Ok(()) => {
                let path = ob.buffer.path().map(std::path::Path::to_path_buf);
                self.note_disk_state(id);
                self.discard_recovery_for(id);
                self.refresh_gutter_hunks(id);
                if let Some(path) = path {
                    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    self.editor_event("saved", fenix_wasm::host::Notice::Saved(name));
                    self.code_file_saved(&path);
                }
                self.auto_save.edits.remove(&id);
                true
            }
            Err(err) => {
                self.auto_save.failed.insert(id, edits);
                let name = self.buffer_display_name(id);
                self.set_error(format!("auto-save couldn't write {name}: {err}"));
                false
            }
        }
    }

    /// Every buffer auto-save may write, written: the window has lost the
    /// keyboard, with `on_focus_change`.
    pub(crate) fn auto_save_all(&mut self) {
        if self.auto_save_mode() != AutoSave::OnFocusChange {
            return;
        }
        for id in self.dirty_tracked_buffer_ids() {
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
                let dirty = self.dirty_tracked_buffer_ids();
                self.auto_save.edits.retain(|id, _| dirty.contains(id));
                let mut next: Option<Instant> = None;
                for id in dirty {
                    if !self.auto_saves(id) {
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
