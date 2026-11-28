//! Text on any page, selected with the mouse and copied: an agent's
//! answer, a Git page's hash or message, a search hit, a task's output, a
//! test's failure, an extension's page. A drag over the page's text
//! selects it (tinted); `y` or `Ctrl+C` copies it; `Esc` lets go, and so
//! does any other key, which then goes to the page as it would have.
//!
//! An explorer page keeps its own clicks (rows, chevrons), and an
//! extension's page that takes the mouse gets it: neither selects.

use super::*;

impl App {
    /// A press on the page in `pane`, whose text starts at `origin`: a
    /// drag over its text may start there. Whether it was a page's.
    pub(super) fn page_select_press(&mut self, pane: fenix_window::WindowId, origin: (f32, f32), pos: (f32, f32)) -> bool {
        let Some(page) = self.windows().content(pane).copied().filter(|b| self.is_page_buffer(*b)) else { return false };
        let takes_mouse = match self.pages.get(&page).map(|s| &s.model) {
            Some(PageModel::Code(p)) => p.mouse(),
            Some(PageModel::Explorer(..)) => true,
            _ => false,
        };
        if takes_mouse {
            return false;
        }
        let at = self.cell_at(pos, origin, self.pane_state(pane).scroll_line);
        // A press anywhere else on the page lets go of what was selected.
        if let Some(state) = self.pages.get_mut(&page) {
            state.selection = Some((at, at));
            state.stale = true;
        }
        self.page_dragging = Some((pane, page, origin));
        true
    }

    /// The pointer moved with the button down: the selection follows it.
    pub(super) fn page_select_drag(&mut self, pos: (f32, f32)) {
        let Some((pane, page, origin)) = self.page_dragging else { return };
        let at = self.cell_at(pos, origin, self.pane_state(pane).scroll_line);
        if let Some(state) = self.pages.get_mut(&page) {
            if let Some((_, head)) = &mut state.selection {
                if *head != at {
                    *head = at;
                    state.stale = true;
                }
            }
        }
        self.wake_caret();
    }

    /// The button let go: a drag's selection stays to be copied; a click
    /// selected nothing.
    pub(super) fn page_select_release(&mut self) {
        let Some((_, page, _)) = self.page_dragging.take() else { return };
        let mut said = false;
        if let Some(state) = self.pages.get_mut(&page) {
            match state.selection {
                Some((a, b)) if a == b => state.selection = None,
                Some(_) => said = true,
                None => {}
            }
            state.stale = true;
        }
        if said {
            self.set_message(if self.standard_keys() { "Ctrl+C copies what's selected" } else { "y copies what's selected -- Esc lets go" });
        }
    }

    /// A key while text is selected on page `id`: `y` or `Ctrl+C` copies
    /// it, `Esc` lets go of it; anything else lets go and is the page's.
    /// Whether the key was taken.
    pub(super) fn page_selection_key(&mut self, id: BufferId, keypress: KeyPress) -> bool {
        let Some(state) = self.pages.get_mut(&id) else { return false };
        let Some((a, b)) = state.selection.take() else { return false };
        state.stale = true;
        let copy = keypress == KeyPress::char('y') || keypress == KeyPress::char('c').with_ctrl();
        if copy {
            let text = crate::page::selected_text(&state.page.text, a, b);
            if let Some(clipboard) = &mut self.clipboard {
                let _ = clipboard.set_text(text.clone());
            }
            self.set_message(format!("copied {} lines", text.lines().count().max(1)));
            return true;
        }
        keypress == KeyPress::named(FenixNamedKey::Escape)
    }
}
