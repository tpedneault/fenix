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
        self.open_extensions(false);
    }

    /// `SPC h w`, and a fresh install's first screen: the page with packs.
    pub(crate) fn open_welcome_page(&mut self) {
        self.open_extensions(true);
    }

    fn open_extensions(&mut self, welcome: bool) {
        let rows = self.extension_rows();
        let page = if welcome { ExtensionsPage::welcome(rows) } else { ExtensionsPage::new(rows) };
        match self.find_page(|m| matches!(m, PageModel::Extensions(_))) {
            Some(id) => {
                if let Some(state) = self.pages.get_mut(&id) {
                    state.model = PageModel::Extensions(Box::new(page));
                    state.stale = true;
                }
                self.show_page(id);
            }
            None => {
                self.open_page(PageModel::Extensions(Box::new(page)));
            }
        }
    }

    /// At start-up: a fresh install keeps to the core (written, so it
    /// stays so until you pick) and is welcomed; an upgrade from 1.x keeps
    /// everything it had on, and says once where to switch things.
    pub(super) fn extensions_at_startup(&mut self) {
        let marker = fenix_storage::paths::state_file("extensions-introduced");
        let introduced = marker.as_ref().is_some_and(|m| m.exists());
        if let Some(marker) = &marker {
            let _ = marker.parent().map(std::fs::create_dir_all);
            let _ = std::fs::write(marker, "");
        }
        if super::feature::first_run() {
            let all = super::feature::switched_off().to_vec();
            if let Err(e) = self.set_setting("extensions.disabled", Some(fenix_config::Value::List(all))) {
                self.set_error(e);
            }
            self.open_welcome_page();
        } else if !introduced {
            self.set_message("Fenix 2: what's beyond the core is an extension now -- yours are all on. SPC h x switches them");
        }
    }

    pub(super) fn extensions_action(&mut self, id: BufferId, action: Action) {
        match action {
            Action::None => {}
            Action::Close => self.close_page(id),
            Action::Switch(extensions, on) => {
                let mut off: Vec<String> = self.config.extensions_disabled.iter().filter(|x| !extensions.contains(&x.as_str())).cloned().collect();
                if !on {
                    off.extend(extensions.iter().map(|id| id.to_string()));
                }
                let value = (!off.is_empty()).then_some(fenix_config::Value::List(off));
                match self.set_setting("extensions.disabled", value) {
                    Ok(()) => {
                        let names: Vec<&str> = extensions.iter().map(|id| self.features.all_specs().find(|(s, _)| s.id == *id).map_or(*id, |(s, _)| s.name)).collect();
                        let name = names.join(", ");
                        let running = extensions.iter().all(|id| self.features.is_on(id));
                        self.set_message(match (on, running) {
                            (true, true) => format!("{name}: on, as now"),
                            (false, false) => format!("{name}: off, as now"),
                            (true, false) => format!("{name}: on when Fenix next starts"),
                            (false, true) => format!("{name}: off when Fenix next starts"),
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
        app.extensions_action(id, Action::Switch(vec!["fenix.jira"], false));
        assert_eq!(app.config.extensions_disabled, ["fenix.jira"]);
        assert!(app.features.is_on("fenix.jira"), "still on in this run");
        let saved = std::fs::read_to_string(dir.path().join("settings.toml")).unwrap();
        assert!(saved.contains("[extensions]") && saved.contains("fenix.jira"), "{saved}");
        let Some(PageModel::Extensions(page)) = app.pages.get(&id).map(|s| &s.model) else { panic!() };
        assert!(page.rows.iter().any(|r| r.id == "fenix.jira" && !r.on && r.running));
        app.extensions_action(id, Action::Switch(vec!["fenix.jira"], true));
        assert!(app.config.extensions_disabled.is_empty());
        // A pack, from the welcome: all of it at once.
        app.open_welcome_page();
        app.extensions_action(id, Action::Switch(vec!["fenix.mib", "fenix.ccsds", "fenix.tcl"], false));
        assert_eq!(app.config.extensions_disabled, ["fenix.mib", "fenix.ccsds", "fenix.tcl"]);
        let Some(PageModel::Extensions(page)) = app.pages.get(&id).map(|s| &s.model) else { panic!() };
        assert!(!page.packs.is_empty(), "the welcome has its packs");
    }
}
