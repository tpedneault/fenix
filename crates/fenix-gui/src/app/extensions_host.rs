//! The host half of the Extensions page (`extensions_page`): the
//! installed extensions switched and removed, and the registry read and
//! installed from off the UI thread.

use super::pages::PageModel;
use super::*;
use crate::extensions_page::{Action, ExtensionsPage, Registry, Target};

/// What comes back from the registry, off the UI thread.
#[derive(Debug)]
pub(crate) enum RegistryEvent {
    /// The index, its signature checked.
    Read(Result<fenix_extensions::registry::Index, String>),
    /// An install (or update) from it, done: what to say.
    Installed(Result<String, String>),
    /// An extension's package read for its details (`Release::preview`):
    /// its id, and the folder its README, changes and screenshots are in.
    Preview(String, Result<PathBuf, String>),
}

impl App {
    /// `SPC h x`.
    pub(crate) fn open_extensions_page(&mut self) {
        self.open_extensions(false);
    }

    /// `SPC h w`, and a fresh install's first screen: the page with the
    /// bundles.
    pub(crate) fn open_welcome_page(&mut self) {
        self.open_extensions(true);
    }

    fn open_extensions(&mut self, welcome: bool) {
        let mut page = if welcome { ExtensionsPage::welcome() } else { ExtensionsPage::new() };
        page.safe_mode = crate::installed::safe_mode();
        page.installed = crate::installed::rows();
        page.release = crate::installed::release();
        // The welcome too: its bundles are the registry's.
        page.registry = match crate::installed::registry(Some(&self.config)) {
            Ok(_) => Registry::Looking,
            Err(e) => Registry::Off(e),
        };
        let look = matches!(page.registry, Registry::Looking);
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

    /// Installs each of `exts` from the registry, one after another.
    fn install_all(&mut self, exts: Vec<String>) {
        self.wasm.install_queue.extend(exts);
        if self.wasm.installing.is_none() && !self.wasm.install_queue.is_empty() {
            let next = self.wasm.install_queue.remove(0);
            self.install_from_registry(next);
        }
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
        self.wasm.installing = Some(ext.clone());
        self.with_extensions_page(|p| p.installing = Some(ext.clone()));
        let _ = std::thread::Builder::new().name("fenix-registry-install".into()).spawn(move || {
            let result = crate::installed::install_from(&registry, &ext, &store);
            let _ = proxy.send_event(FenixUserEvent::Registry(RegistryEvent::Installed(result)));
        });
    }

    /// The bundles the Extensions page knows with `id` in them.
    fn bundles_of(&self, id: &str) -> Vec<String> {
        match self.find_page(|m| matches!(m, PageModel::Extensions(_))).and_then(|p| self.pages.get(&p)).map(|s| &s.model) {
            Some(PageModel::Extensions(page)) => page.bundles_of(id),
            _ => Vec::new(),
        }
    }

    /// `d`: an extension's details page, in the reading view in the
    /// Extensions page's place; `q` comes back. One from the registry has
    /// its package read first (off the UI thread) for its README, changes
    /// and screenshots.
    fn open_extension_details(&mut self, page_id: BufferId, target: Target) {
        let text = match &target {
            Target::Installed(ext) => crate::installed::find(ext).map(|(e, managed)| {
                let read = |name: &str| std::fs::read_to_string(e.dir.join(name)).ok();
                let shots = crate::extensions_page::screenshots(&e.dir);
                crate::extensions_page::installed_details(&e, managed, read("README.md").as_deref(), read("CHANGELOG.md").as_deref(), &shots, &self.bundles_of(ext))
            }),
            Target::Available(ext) => {
                let found = match self.pages.get(&page_id).map(|s| &s.model) {
                    Some(PageModel::Extensions(page)) => match &page.registry {
                        Registry::Read(index) => index.listing(ext).and_then(|l| l.latest_for(&page.release)).cloned(),
                        _ => None,
                    },
                    _ => None,
                };
                let (Some(release), Some(store), Some(proxy)) = (found, crate::installed::store(), self.event_proxy.clone()) else {
                    return self.show_available_details(page_id, ext, None);
                };
                self.set_message(format!("Reading {ext}'s package for its details…"));
                let ext = ext.clone();
                let _ = std::thread::Builder::new().name("fenix-registry-preview".into()).spawn(move || {
                    let result = release.preview(&ext, &fenix_extensions::registry::Http, &store.root().join(".previews"));
                    let _ = proxy.send_event(FenixUserEvent::Registry(RegistryEvent::Preview(ext, result)));
                });
                return;
            }
        };
        let Some(text) = text else { return self.set_error("that extension isn't there any more") };
        self.show_details(page_id, &text);
    }

    /// Registry extension `ext`'s details, with what its package carries
    /// when that's been read (`preview`).
    fn show_available_details(&mut self, page_id: BufferId, ext: &str, preview: Option<&Path>) {
        let text = match self.pages.get(&page_id).map(|s| &s.model) {
            Some(PageModel::Extensions(page)) => match &page.registry {
                Registry::Read(index) => index.listing(ext).map(|l| crate::extensions_page::available_details(l, &page.release, preview, &page.bundles_of(ext))),
                _ => None,
            },
            _ => None,
        };
        match text {
            Some(text) => self.show_details(page_id, &text),
            None => self.set_error("that extension isn't there any more"),
        }
    }

    fn show_details(&mut self, page_id: BufferId, text: &str) {
        let source = self.buffers.open_text_view_like(text, Path::new("details.md"));
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
                self.wasm.installing = None;
                self.with_extensions_page(|p| {
                    p.installing = None;
                    p.installed = rows;
                });
                self.install_all(Vec::new());
            }
            RegistryEvent::Preview(ext, result) => {
                let Some(page_id) = self.find_page(|m| matches!(m, PageModel::Extensions(_))) else { return };
                match result {
                    Ok(dir) => self.show_available_details(page_id, &ext, Some(&dir)),
                    Err(e) => {
                        self.set_error(format!("{e} -- its details without its README"));
                        self.show_available_details(page_id, &ext, None);
                    }
                }
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

    /// At start-up: a first run keeps to the core and is welcomed, to
    /// pick what else it wants.
    pub(super) fn extensions_at_startup(&mut self) {
        if crate::installed::safe_mode() {
            self.set_message("Safe mode (--disable-extensions): the core alone this run -- no installed extensions");
            return;
        }
        if super::startup::first_run() {
            self.open_welcome_page();
        }
    }
}

impl App {

    pub(super) fn extensions_action(&mut self, id: BufferId, action: Action) {
        match action {
            Action::None => {}
            Action::Close => self.close_page(id),
            Action::Say(text) => self.set_message(text),
            Action::Tutorial => self.open_tutorial(),
            Action::Install(ext) => self.install_from_registry(ext),
            Action::UpdateAll(exts) => self.install_all(exts),
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
                match crate::installed::store().map(|store| store.uninstall_unneeded(&ext)) {
                    Some(Ok(removed)) => self.set_message(format!("Removed {} -- what they brought stays until Fenix next starts", crate::installed::said_removed(&removed))),
                    Some(Err(e)) => self.set_error(e),
                    None => self.set_error("no folder to keep extensions in"),
                }
                self.refresh_extensions_page(id);
            }
            Action::SwitchBundle { on, members, install } => {
                let have = crate::installed::rows();
                let mut missing = Vec::new();
                for ext in members {
                    match have.iter().find(|r| r.id == ext) {
                        Some(row) if row.on != on && !row.managed => self.extensions_action(id, Action::SwitchInstalled(ext, on)),
                        Some(_) => {}
                        None if on => missing.push(ext),
                        None => {}
                    }
                }
                // The registry's bundle brings its members itself, marked
                // as its; without it, they come one by one.
                match install {
                    Some(bundle) => self.install_all(vec![bundle]),
                    None => self.install_all(missing),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry_of(app: &App) -> &Registry {
        let id = app.find_page(|m| matches!(m, PageModel::Extensions(_))).expect("the page");
        let Some(PageModel::Extensions(page)) = app.pages.get(&id).map(|s| &s.model) else { panic!() };
        &page.registry
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
        assert!(matches!(registry_of(&app), Registry::Off(_)), "nor the welcome, for its bundles");
        app.set_setting("extensions.store", Some(fenix_config::Value::Bool(true))).unwrap();
        app.open_welcome_page();
        assert!(matches!(registry_of(&app), Registry::Looking), "the welcome looks, for its bundles");
    }
}
