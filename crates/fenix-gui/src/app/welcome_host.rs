//! The welcome's host: opens it (a fresh install's first screen, `SPC h
//! w`), makes each choice as it's made, and presses the key on a cap
//! that's clicked. The page itself is `welcome_page.rs`.

use super::pages::PageModel;
use super::{App, BufferId};
use crate::welcome_page::{key_on_cap, Action, Pack, WelcomePage};

impl App {
    /// `SPC h w`, and a fresh install's first screen: the welcome, from
    /// its first step.
    pub(crate) fn open_welcome_page(&mut self) {
        let packs = crate::extensions_page::FALLBACK_BUNDLES.iter().map(|b| Pack { name: b.name.to_string(), about: b.about.to_string() }).collect();
        let page = WelcomePage::new(self.key_scheme().name(), self.theme.name.to_string(), crate::theme::all().len(), self.logical_font_size(), packs);
        match self.find_page(|m| matches!(m, PageModel::Welcome(_))) {
            Some(id) => {
                if let Some(state) = self.pages.get_mut(&id) {
                    state.model = PageModel::Welcome(Box::new(page));
                    state.stale = true;
                }
                self.show_page(id);
            }
            None => {
                self.open_page(PageModel::Welcome(Box::new(page)));
            }
        }
    }

    /// Changes the welcome, if it's open, and lays it out again.
    pub(super) fn with_welcome_page(&mut self, change: impl FnOnce(&mut WelcomePage)) {
        let Some(id) = self.find_page(|m| matches!(m, PageModel::Welcome(_))) else { return };
        if let Some(state) = self.pages.get_mut(&id) {
            if let PageModel::Welcome(page) = &mut state.model {
                change(page);
            }
            state.stale = true;
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    pub(super) fn welcome_action(&mut self, id: BufferId, action: Action) {
        match action {
            Action::None => {}
            Action::Keys(scheme) => {
                if self.key_scheme().name() != scheme {
                    if let Err(e) = self.set_setting("editor.keys", Some(fenix_config::Value::Text(scheme.to_string()))) {
                        self.set_error(e);
                    }
                }
                let now = self.key_scheme().name();
                self.with_welcome_page(|p| p.keys = now);
                // Every page says the keys picked from now on.
                for state in self.pages.values_mut() {
                    state.stale = true;
                }
            }
            Action::Theme(name) => {
                if let Some(theme) = crate::theme::by_name(name) {
                    self.apply_theme(theme);
                }
                let now = self.theme.name.to_string();
                self.with_welcome_page(|p| p.theme = now);
            }
            Action::FontSize(step) => {
                self.adjust_font_size(self.logical_font_size() + step as f32);
                let now = self.logical_font_size();
                self.with_welcome_page(|p| p.font_size = now);
            }
            Action::Extensions => self.open_extension_packs(),
            Action::Tutorial => self.open_tutorial(),
            Action::KeysPage => self.open_keys_sheet(),
            Action::Import => self.open_import_page(),
            Action::OpenProject => {
                self.run_command_with(None, "project.hub");
            }
            Action::Finish => {
                // Written down, so a first run is over: what's in use is
                // what was picked, or the default when nothing was.
                let keys = self.key_scheme().name();
                if let Err(e) = self.set_setting("editor.keys", Some(fenix_config::Value::Text(keys.to_string()))) {
                    self.set_error(e);
                }
                self.close_page(id);
                self.go_home();
                self.set_message("Welcome to Fenix -- SPC h w brings the welcome back, SPC h T the tutorial");
            }
        }
    }

    /// A click on the welcome at (`line`, `col`) of its page: the key on
    /// the cap there, pressed. Whether it was one.
    pub(crate) fn welcome_click(&mut self, page: BufferId, line: usize, col: usize) -> bool {
        let Some(state) = self.pages.get(&page) else { return false };
        if !matches!(state.model, PageModel::Welcome(_)) {
            return false;
        }
        let Some(row) = state.page.text.lines().nth(line) else { return false };
        let cells = crate::page::line_cells(row);
        let cell = crate::page::cell_of(cells.as_deref(), col);
        let key = state.page.chips.iter().filter(|(l, cols, chip)| *l == line && cols.contains(&cell) && matches!(chip, crate::page::Chip::Key | crate::page::Chip::Chosen)).find_map(|(_, cols, _)| {
            let cap: String = row.chars().skip(cols.start).take(cols.len()).collect();
            key_on_cap(&cap)
        });
        // Or anywhere on a row with one cap (a choice, a theme): its key.
        let key = key.or_else(|| {
            let caps: Vec<_> = state.page.chips.iter().filter(|(l, _, chip)| *l == line && matches!(chip, crate::page::Chip::Key | crate::page::Chip::Chosen)).collect();
            match caps.as_slice() {
                [(_, cols, _)] => key_on_cap(&row.chars().skip(cols.start).take(cols.len()).collect::<String>()),
                _ => None,
            }
        });
        let Some(key) = key else { return false };
        let Some(state) = self.pages.get_mut(&page) else { return false };
        let PageModel::Welcome(p) = &mut state.model else { return false };
        let action = p.key(key);
        state.stale = true;
        self.welcome_action(page, action);
        self.wake_caret();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::welcome_page::Step;

    fn welcome(app: &App) -> &WelcomePage {
        let id = app.find_page(|m| matches!(m, PageModel::Welcome(_))).expect("the welcome is open");
        match &app.pages[&id].model {
            PageModel::Welcome(p) => p,
            _ => unreachable!(),
        }
    }

    #[test]
    fn the_welcome_walks_through_keys_look_extensions_and_learning() {
        let (mut app, _f) = super::super::test_keys::app_on_file("a.txt", "");
        app.open_welcome_page();
        assert_eq!(welcome(&app).step, Step::Hello);
        assert_eq!(welcome(&app).keys, "vim", "Vim keys until another is picked");
        app.press_all("Enter s");
        assert_eq!(app.key_scheme().name(), "standard", "in use as it's picked");
        assert_eq!(welcome(&app).keys, "standard");
        app.press_all("v");
        assert_eq!(app.key_scheme().name(), "vim");
        app.press_all("Enter 2");
        assert_eq!(app.theme.name, "Tokyo Night Storm");
        app.press_all("Enter Enter");
        assert_eq!(welcome(&app).step, Step::Learn);
        app.press("Enter");
        assert!(app.find_page(|m| matches!(m, PageModel::Welcome(_))).is_none(), "done: closed");
        assert_eq!(app.config.text_value("editor.keys"), Some("vim"), "written down, so it's not a first run again");
    }

    #[test]
    fn a_click_on_a_cap_presses_its_key() {
        let (mut app, _f) = super::super::test_keys::app_on_file("a.txt", "");
        app.open_welcome_page();
        let id = app.find_page(|m| matches!(m, PageModel::Welcome(_))).unwrap();
        app.press("Enter");
        app.ensure_page_layout(id, app.focused_pane_id(), 100);
        let text = app.pages[&id].page.text.clone();
        let line = text.lines().position(|l| l.contains("Standard keys")).unwrap();
        let col = text.lines().nth(line).unwrap().find(" s ").unwrap() + 1;
        assert!(app.welcome_click(id, line, col));
        assert_eq!(app.key_scheme().name(), "standard");
        app.ensure_page_layout(id, app.focused_pane_id(), 100);
        let text = app.pages[&id].page.text.clone();
        let line = text.lines().position(|l| l.contains("Enter")).unwrap();
        let col = text.lines().nth(line).unwrap().find("Enter").unwrap();
        assert!(app.welcome_click(id, line, col), "the footer's Enter");
        assert_eq!(welcome(&app).step, Step::Look);
    }
}
