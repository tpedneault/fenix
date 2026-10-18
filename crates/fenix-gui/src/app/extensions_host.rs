//! The host half of the Extensions page (`extensions_page`): its rows
//! from the built-in features, and a switch written to
//! `extensions.disabled`.

use super::pages::PageModel;
use super::*;
use crate::extensions_page::{Action, ExtensionsPage, Row};

impl App {
    /// The built-in extensions as the page lists them: on now, and on
    /// from the next start.
    fn extension_rows(&self) -> Vec<Row> {
        let off = &self.config.extensions_disabled;
        self.features
            .all_specs()
            .map(|(spec, running)| Row { id: spec.id, name: spec.name, about: spec.about, running, on: !off.iter().any(|id| id == spec.id) })
            .collect()
    }

    /// `SPC h x`.
    pub(crate) fn open_extensions_page(&mut self) {
        let existing = self.find_page(|m| matches!(m, PageModel::Extensions(_)));
        match existing {
            Some(id) => {
                let rows = self.extension_rows();
                if let Some(PageModel::Extensions(page)) = self.pages.get_mut(&id).map(|s| &mut s.model) {
                    page.rows = rows;
                }
                self.show_page(id);
            }
            None => {
                let page = ExtensionsPage::new(self.extension_rows());
                self.open_page(PageModel::Extensions(Box::new(page)));
            }
        }
    }

    pub(super) fn extensions_action(&mut self, id: BufferId, action: Action) {
        match action {
            Action::None => {}
            Action::Close => self.close_page(id),
            Action::Switch(extension, on) => {
                let mut off: Vec<String> = self.config.extensions_disabled.iter().filter(|x| x.as_str() != extension).cloned().collect();
                if !on {
                    off.push(extension.to_string());
                }
                let value = (!off.is_empty()).then_some(fenix_config::Value::List(off));
                match self.set_setting("extensions.disabled", value) {
                    Ok(()) => {
                        let name = self.features.all_specs().find(|(s, _)| s.id == extension).map_or(extension, |(s, _)| s.name);
                        let running = self.features.is_on(extension);
                        self.set_message(match (on, running) {
                            (true, true) | (false, false) => format!("{name} stays {} -- as it is now", if on { "on" } else { "off" }),
                            (true, false) => format!("{name} switches on when Fenix next starts"),
                            (false, true) => format!("{name} switches off when Fenix next starts"),
                        });
                    }
                    Err(e) => self.set_error(e),
                }
                let rows = self.extension_rows();
                if let Some(state) = self.pages.get_mut(&id) {
                    if let PageModel::Extensions(page) = &mut state.model {
                        page.rows = rows;
                    }
                    state.stale = true;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The page lists every built-in, and a switch is written to
    /// `extensions.disabled` -- for the next start; this run keeps what
    /// it has.
    #[test]
    fn a_switch_is_saved_for_the_next_start() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::with_file(None);
        app.config = fenix_config::Config::load_or_default(dir.path().join("settings.toml"));
        app.open_extensions_page();
        let id = app.focused_buffer_id();
        let Some(PageModel::Extensions(page)) = app.pages.get(&id).map(|s| &s.model) else { panic!("the Extensions page") };
        assert_eq!(page.rows.len(), super::super::feature::BUILTIN_SPECS.len());
        assert!(page.rows.iter().all(|r| r.on && r.running));
        app.extensions_action(id, Action::Switch("fenix.jira", false));
        assert_eq!(app.config.extensions_disabled, ["fenix.jira"]);
        assert!(app.features.is_on("fenix.jira"), "still on in this run");
        let saved = std::fs::read_to_string(dir.path().join("settings.toml")).unwrap();
        assert!(saved.contains("[extensions]") && saved.contains("fenix.jira"), "{saved}");
        let Some(PageModel::Extensions(page)) = app.pages.get(&id).map(|s| &s.model) else { panic!() };
        assert!(page.rows.iter().any(|r| r.id == "fenix.jira" && !r.on && r.running));
        app.extensions_action(id, Action::Switch("fenix.jira", true));
        assert!(app.config.extensions_disabled.is_empty());
    }
}
