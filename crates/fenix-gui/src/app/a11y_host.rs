//! Fenix to a screen reader: the main window's AccessKit adapter, fed a
//! tree (`crate::a11y`) of what's in front -- the focused text with its
//! caret and selection, a prompt, the modeline -- and announcements of
//! the mode and of messages as they change. See
//! [the plan](../../../../docs/plans/screen-reader.md).
//!
//! Nothing is built until an assistive technology asks (`active`): the
//! adapter answers the platform's question of whether Fenix is
//! accessible, and that's all, until one does. Then the tree is worked
//! out after each round of events (`about_to_wait`) and sent when it
//! changed.

use std::sync::Arc;

use crate::a11y::{Announcement, EditorView, PromptView, Snapshot};

use super::*;

/// The setting that turns it off. Read when the main window opens.
pub(crate) const SETTING: &str = "accessibility.screen_reader";

/// The adapter and what it was last told.
#[derive(Default)]
pub(crate) struct A11yHost {
    adapter: Option<accesskit_winit::Adapter>,
    window: Option<Arc<Window>>,
    /// An assistive technology asked for the tree, and hasn't let go.
    active: bool,
    last: Option<Snapshot>,
    /// The mode and the message last said, so a change is said once.
    mode: &'static str,
    message_at: Option<Instant>,
    announcement: Option<Announcement>,
}

impl std::fmt::Debug for A11yHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("A11yHost").field("adapter", &self.adapter.is_some()).field("active", &self.active).finish()
    }
}

/// Whether the main window gets an adapter: unless the setting is off.
pub(crate) fn wanted(config: &fenix_config::Config) -> bool {
    config.bool_value(SETTING) != Some(false)
}

/// The adapter for `window`, which hasn't been shown yet; `None` when it
/// has been (the platform's adapter would have no way to attach).
pub(crate) fn adapter_for(event_loop: &ActiveEventLoop, window: &Window, proxy: winit::event_loop::EventLoopProxy<FenixUserEvent>) -> Option<accesskit_winit::Adapter> {
    if window.is_visible() == Some(true) {
        return None;
    }
    Some(accesskit_winit::Adapter::with_event_loop_proxy(event_loop, window, proxy))
}

impl From<accesskit_winit::Event> for FenixUserEvent {
    fn from(event: accesskit_winit::Event) -> Self {
        FenixUserEvent::Accessibility(event)
    }
}

/// A mode's badge as it's said: `INSERT` is "Insert".
fn spoken_mode(badge: &str) -> String {
    let lower = badge.to_lowercase();
    let mut chars = lower.chars();
    chars.next().map(|c| c.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

impl App {
    /// Gives the main window, `window`, its adapter.
    pub(crate) fn attach_accessibility(&mut self, adapter: accesskit_winit::Adapter, window: Arc<Window>) {
        self.a11y.adapter = Some(adapter);
        self.a11y.window = Some(window);
    }

    /// A window event, which the adapter sees first (the window's bounds
    /// and focus) -- only the main window's.
    pub(super) fn a11y_window_event(&mut self, id: WindowId, event: &WindowEvent) {
        let A11yHost { adapter: Some(adapter), window: Some(window), .. } = &mut self.a11y else { return };
        if window.id() == id {
            adapter.process_event(window, event);
        }
    }

    /// What the adapter passed on: the tree asked for, an action, the
    /// assistive technology gone.
    pub(super) fn a11y_event(&mut self, event: accesskit_winit::Event) {
        match event.window_event {
            accesskit_winit::WindowEvent::InitialTreeRequested => {
                self.a11y.active = true;
                // What's true now isn't news.
                self.a11y.mode = self.mode_badge_label();
                self.a11y.message_at = self.status_message.as_ref().map(|m| m.set_at);
                self.a11y.last = None;
                self.a11y_update();
            }
            accesskit_winit::WindowEvent::AccessibilityDeactivated => {
                self.a11y.active = false;
                self.a11y.last = None;
            }
            // Phase 1 offers no actions: moving the caret from a braille
            // display, focusing a node, scrolling are phase 2.
            accesskit_winit::WindowEvent::ActionRequested(_) => {}
        }
    }

    /// Sends the tree, if an assistive technology is listening and it
    /// changed.
    pub(super) fn a11y_update(&mut self) {
        if !self.a11y.active || self.a11y.adapter.is_none() {
            return;
        }
        let snapshot = self.a11y_snapshot();
        if self.a11y.last.as_ref() == Some(&snapshot) {
            return;
        }
        if let Some(adapter) = self.a11y.adapter.as_mut() {
            adapter.update_if_active(|| crate::a11y::tree(&snapshot));
        }
        self.a11y.last = Some(snapshot);
    }

    /// What's in front, as the tree says it: and anything that changed
    /// since it was last said (the mode, a message) announced.
    pub(super) fn a11y_snapshot(&mut self) -> Snapshot {
        let mode = self.mode_badge_label();
        let mut said = Vec::new();
        if mode != self.a11y.mode {
            self.a11y.mode = mode;
            said.push((spoken_mode(mode), false));
        }
        let message = self.status_message.as_ref().map(|m| (m.set_at, m.text.clone(), m.is_error));
        let message_at = message.as_ref().map(|m| m.0);
        if message_at != self.a11y.message_at {
            self.a11y.message_at = message_at;
            if let Some((_, text, is_error)) = message {
                said.push((text, is_error));
            }
        }
        if !said.is_empty() {
            let serial = self.a11y.announcement.as_ref().map_or(0, |a| a.serial) + 1;
            let urgent = said.iter().any(|(_, urgent)| *urgent);
            let text = said.into_iter().map(|(text, _)| text).collect::<Vec<_>>().join(". ");
            self.a11y.announcement = Some(Announcement { text, urgent, serial });
        }

        let ob = self.open();
        let name = self.buffer_display_name(self.focused_buffer_id());
        let cursor = self.cursor();
        let caret = ob.buffer.line_col(&cursor);
        let anchor = self.a11y_selection_anchor();
        let buffer = &ob.buffer;
        let mut editor = EditorView::around(format!("{name}, {}", spoken_mode(mode)), buffer.line_count(), caret, anchor, |line| {
            let start = buffer.line_start_char(line);
            buffer.text_range(start, start + buffer.line_len(line))
        });
        editor.read_only = ob.kind != BufferKind::Text;
        // Vim's selection takes in the character under the cursor: the
        // selection's far end is after it.
        if let Some(anchor) = editor.anchor {
            if self.vim.policy() != fenix_vim::EditPolicy::Standard && self.vim.visual_kind() == VisualKind::Char {
                if anchor <= editor.caret {
                    editor.caret.1 += 1;
                } else {
                    editor.anchor = Some((anchor.0, anchor.1 + 1));
                }
            }
        }
        let prompt = self.active_prompt_text().map(|text| {
            let caret = text.chars().count();
            PromptView { label: "Prompt".into(), text, caret }
        });
        let (badge, rest) = self.modeline_pieces();
        Snapshot {
            title: self.a11y.window.as_ref().map(|w| w.title()).unwrap_or_else(|| "Fenix".into()),
            editor: Some(editor),
            prompt,
            modeline: format!("{badge} {}", rest.trim()).trim().to_string(),
            announcement: self.a11y.announcement.clone(),
        }
    }

    /// Where the selection starts, in Visual: (line, column).
    fn a11y_selection_anchor(&self) -> Option<(usize, usize)> {
        if self.vim.mode() != Mode::Visual {
            return None;
        }
        let buffer = &self.open().buffer;
        let at = self.vim.visual_anchor().min(buffer.len_chars());
        Some(buffer.line_col(&Cursor { char_idx: at, sticky_col: 0 }))
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_keys::app_on_file;

    #[test]
    fn the_snapshot_is_the_focused_text_its_caret_and_the_mode() {
        let (mut app, _f) = app_on_file("notes.txt", "one\ntwo three\n");
        app.press_all("j w");
        let snap = app.a11y_snapshot();
        let editor = snap.editor.unwrap();
        assert_eq!(editor.label, "notes.txt, Normal");
        assert_eq!(editor.caret, (1, 4));
        assert_eq!(editor.lines[1], "two three");
        assert!(snap.modeline.starts_with("NORMAL"), "{}", snap.modeline);
    }

    #[test]
    fn a_mode_change_and_a_message_are_announced_once() {
        let (mut app, _f) = app_on_file("a.txt", "x\n");
        app.a11y.mode = app.mode_badge_label();
        assert_eq!(app.a11y_snapshot().announcement, None);
        app.press("i");
        let said = app.a11y_snapshot().announcement.unwrap();
        assert_eq!((said.text.as_str(), said.urgent), ("Insert", false));
        // Nothing new: the same announcement, not another.
        assert_eq!(app.a11y_snapshot().announcement.unwrap().serial, said.serial);
        app.press("Esc");
        app.set_error("E32: No file name".to_string());
        let said = app.a11y_snapshot().announcement.unwrap();
        assert_eq!(said.text, "Normal. E32: No file name");
        assert!(said.urgent);
    }

    #[test]
    fn a_vim_selection_takes_in_the_character_under_the_cursor() {
        let (mut app, _f) = app_on_file("a.txt", "hello world\n");
        app.press_all("v e");
        let editor = app.a11y_snapshot().editor.unwrap();
        assert_eq!((editor.anchor, editor.caret), (Some((0, 0)), (0, 5)), "`hello`, all five");
        let tree = accesskit_consumer::Tree::new(crate::a11y::tree(&app.a11y_snapshot()), true);
        assert_eq!(tree.state().focus().unwrap().text_selection().unwrap().text(), "hello");
    }

    #[test]
    fn a_prompt_is_in_the_snapshot() {
        let (mut app, _f) = app_on_file("a.txt", "x\n");
        app.press_all(": w");
        let prompt = app.a11y_snapshot().prompt.unwrap();
        assert_eq!(prompt.text, ":w");
    }

    #[test]
    fn without_an_assistive_technology_nothing_is_built() {
        let (mut app, _f) = app_on_file("a.txt", "x\n");
        app.press("i");
        app.a11y_update();
        assert!(app.a11y.last.is_none());
        // And the mode it went to isn't kept to announce later.
        assert_eq!(app.a11y.mode, "");
    }
}
