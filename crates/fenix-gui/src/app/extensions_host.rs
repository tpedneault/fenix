//! The host half of the Extensions page (`extensions_page`): its rows
//! from the built-in features, a switch written to `extensions.disabled`,
//! and the registry read and installed from off the UI thread.

use super::pages::PageModel;
use super::*;
use crate::extensions_page::{Action, ExtensionsPage, Registry, Row, Target};

/// What comes back from the registry, off the UI thread.
#[derive(Debug)]
pub(crate) enum RegistryEvent {
    /// The index, its signature checked.
    Read(Result<fenix_extensions::registry::Index, String>),
    /// An install (or update) from it, done: what to say.
    Installed(Result<String, String>),
}

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
        let mut page = if welcome { ExtensionsPage::welcome(rows) } else { ExtensionsPage::new(rows) };
        page.safe_mode = crate::installed::safe_mode();
        page.installed = crate::installed::rows();
        page.release = crate::installed::release();
        if !welcome {
            page.registry = match crate::installed::registry(Some(&self.config)) {
                Ok(_) => Registry::Looking,
                Err(e) => Registry::Off(e),
            };
        }
        let look = matches!(page.registry, Registry::Looking) && !welcome;
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
        if look {
            self.read_registry();
        }
    }

    /// Changes the Extensions page, if it's open, and lays it out again.
    fn with_extensions_page(&mut self, change: impl FnOnce(&mut ExtensionsPage)) {
        let Some(id) = self.find_page(|m| matches!(m, PageModel::Extensions(_))) else { return };
        if let Some(state) = self.pages.get_mut(&id) {
            if let PageModel::Extensions(page) = &mut state.model {
                change(page);
                page.focus = page.focus.min(page.count().saturating_sub(1));
            }
            state.stale = true;
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    /// Reads the registry's index on another thread; the page shows it
    /// when it lands. Not with no event loop (tests): nothing is fetched.
    fn read_registry(&mut self) {
        let registry = match crate::installed::registry(Some(&self.config)) {
            Ok(registry) => registry,
            Err(e) => return self.with_extensions_page(|p| p.registry = Registry::Off(e)),
        };
        let Some(proxy) = self.event_proxy.clone() else { return };
        let _ = std::thread::Builder::new().name("fenix-registry".into()).spawn(move || {
            let result = registry.index(&fenix_extensions::registry::Http);
            let _ = proxy.send_event(FenixUserEvent::Registry(RegistryEvent::Read(result)));
        });
    }

    /// Installs `ext` from the registry on another thread, one at a time.
    fn install_from_registry(&mut self, ext: String) {
        let Some(store) = crate::installed::store() else { return self.set_error("no folder to keep extensions in") };
        let registry = match crate::installed::registry(Some(&self.config)) {
            Ok(registry) => registry,
            Err(e) => return self.set_error(e),
        };
        let Some(proxy) = self.event_proxy.clone() else { return };
        self.set_message(format!("Installing {ext} from the registry…"));
        self.with_extensions_page(|p| p.installing = Some(ext.clone()));
        let _ = std::thread::Builder::new().name("fenix-registry-install".into()).spawn(move || {
            let result = crate::installed::install_from(&registry, &ext, &store);
            let _ = proxy.send_event(FenixUserEvent::Registry(RegistryEvent::Installed(result)));
        });
    }

    /// `d`: an extension's details page, in the reading view in the
    /// Extensions page's place; `q` comes back.
    fn open_extension_details(&mut self, page_id: BufferId, target: Target) {
        let text = match &target {
            Target::BuiltIn(ext) => self.extension_rows().into_iter().find(|r| r.id == *ext).map(|r| crate::extensions_page::builtin_details(&r)),
            Target::Installed(ext) => crate::installed::find(ext).map(|(e, managed)| {
                let read = |name: &str| std::fs::read_to_string(e.dir.join(name)).ok();
                crate::extensions_page::installed_details(&e, managed, read("README.md").as_deref(), read("CHANGELOG.md").as_deref())
            }),
            Target::Available(ext) => match self.pages.get(&page_id).map(|s| &s.model) {
                Some(PageModel::Extensions(page)) => match &page.registry {
                    Registry::Read(index) => index.extensions.iter().find(|l| l.id == *ext).map(|l| crate::extensions_page::available_details(l, &page.release)),
                    _ => None,
                },
                _ => None,
            },
        };
        let Some(text) = text else { return self.set_error("that extension isn't there any more") };
        let source = self.buffers.open_text_view_like(&text, Path::new("details.md"));
        let mut page = crate::reading_page::ReadingPage::new(source, None, true);
        page.back = Some(page_id);
        self.open_page(PageModel::Reading(Box::new(page)));
    }

    pub(super) fn apply_registry_event(&mut self, event: RegistryEvent) {
        match event {
            RegistryEvent::Read(result) => self.with_extensions_page(|p| {
                p.registry = match result {
                    Ok(index) => Registry::Read(index),
                    Err(e) => Registry::Failed(e),
                }
            }),
            RegistryEvent::Installed(result) => {
                match result {
                    Ok(said) => self.set_message(said.replace('\n', " ")),
                    Err(e) => self.set_error(e),
                }
                let rows = crate::installed::rows();
                self.with_extensions_page(|p| {
                    p.installing = None;
                    p.installed = rows;
                });
            }
        }
    }

    /// Page `id`'s installed rows read again, after one was switched or
    /// removed.
    fn refresh_extensions_page(&mut self, id: BufferId) {
        let rows = crate::installed::rows();
        if let Some(state) = self.pages.get_mut(&id) {
            if let PageModel::Extensions(page) = &mut state.model {
                page.installed = rows;
                page.focus = page.focus.min(page.count().saturating_sub(1));
            }
            state.stale = true;
        }
    }

    /// At start-up: a fresh install keeps to the core (written, so it
    /// stays so until you pick) and is welcomed; an upgrade from 1.x keeps
    /// everything it had on, and says once where to switch things.
    pub(super) fn extensions_at_startup(&mut self) {
        if crate::installed::safe_mode() {
            self.set_message("Safe mode (--disable-extensions): the core alone this run -- no built-in or installed extensions");
            return;
        }
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
            Action::Say(text) => self.set_message(text),
            Action::Install(ext) => self.install_from_registry(ext),
            Action::Details(target) => self.open_extension_details(id, target),
            Action::Refresh => {
                self.set_message("Looking at the registry again…");
                self.with_extensions_page(|p| p.registry = Registry::Looking);
                self.read_registry();
            }
            Action::SwitchInstalled(ext, on) => {
                match crate::installed::store().map(|store| store.set_enabled(&ext, on)) {
                    Some(Ok(())) => self.set_message(format!("{ext}: {} from the next start", if on { "on" } else { "off" })),
                    Some(Err(e)) => self.set_error(e),
                    None => self.set_error("no folder to keep extensions in"),
                }
                self.refresh_extensions_page(id);
            }
            Action::Uninstall(ext) => {
                match crate::installed::store().map(|store| store.uninstall(&ext)) {
                    Some(Ok(())) => self.set_message(format!("Removed {ext} -- what it brought stays until Fenix next starts")),
                    Some(Err(e)) => self.set_error(e),
                    None => self.set_error("no folder to keep extensions in"),
                }
                self.refresh_extensions_page(id);
            }
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

    fn registry_of(app: &App) -> &Registry {
        let id = app.find_page(|m| matches!(m, PageModel::Extensions(_))).expect("the page");
        let Some(PageModel::Extensions(page)) = app.pages.get(&id).map(|s| &s.model) else { panic!() };
        &page.registry
    }

    /// `d` opens a details page in the Extensions page's place; `q`
    /// closes it, its text with it, and goes back.
    #[test]
    fn details_open_in_the_reading_view_and_come_back() {
        let mut app = App::with_file(None);
        app.open_extensions_page();
        let id = app.focused_buffer_id();
        let buffers = app.buffers.len();
        app.extensions_action(id, Action::Details(Target::BuiltIn("fenix.jira")));
        let details = app.focused_buffer_id();
        let Some(PageModel::Reading(p)) = app.pages.get(&details).map(|s| &s.model) else { panic!("the reading view") };
        assert_eq!(p.back, Some(id));
        assert_eq!(app.page_title(details), "*details*");
        let text = app.buffers.get(p.source).unwrap().buffer.text();
        assert!(text.starts_with("# Jira"), "{text}");
        app.reading_action(details, crate::reading_page::Action::Close);
        assert_eq!(app.focused_buffer_id(), id, "back on the Extensions page");
        assert_eq!(app.buffers.len(), buffers, "the details' text is gone with it");
    }

    /// The page looks at the registry when it opens -- unless the store
    /// is switched off -- and shows the index, or why there isn't one,
    /// when it lands.
    #[test]
    fn the_registry_is_looked_at_unless_the_store_is_off() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::with_file(None);
        app.config = fenix_config::Config::load_or_default(dir.path().join("settings.toml"));
        app.open_extensions_page();
        assert!(matches!(registry_of(&app), Registry::Looking));
        let index = serde_json::from_str(r#"{"format": 1, "extensions": []}"#).unwrap();
        app.apply_registry_event(RegistryEvent::Read(Ok(index)));
        assert!(matches!(registry_of(&app), Registry::Read(_)));
        app.apply_registry_event(RegistryEvent::Read(Err("offline".into())));
        assert!(matches!(registry_of(&app), Registry::Failed(e) if e == "offline"));
        app.set_setting("extensions.store", Some(fenix_config::Value::Bool(false))).unwrap();
        app.open_extensions_page();
        assert!(matches!(registry_of(&app), Registry::Off(e) if e.contains("extensions.store")));
        app.open_welcome_page();
        assert!(matches!(registry_of(&app), Registry::Looking), "the welcome doesn't look");
    }
}
