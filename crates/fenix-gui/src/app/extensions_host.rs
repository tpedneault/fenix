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

/// When `extensions.toml` last changed, yours and the machine's.
fn extensions_stamps() -> Vec<Option<std::time::SystemTime>> {
    [crate::installed::store(), crate::installed::machine()]
        .iter()
        .map(|store| store.as_ref().and_then(|s| std::fs::metadata(s.root().join("extensions.toml")).and_then(|m| m.modified()).ok()))
        .collect()
}

impl App {
    /// `SPC h x`.
    pub(crate) fn open_extensions_page(&mut self) {
        self.open_extensions(false);
    }

    /// The extensions' packs, alone: what the welcome's Extensions step
    /// opens (`extensions.welcome`).
    pub(crate) fn open_extension_packs(&mut self) {
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
    /// What installing `ext` would let it do, when it's code that asks
    /// for more than its sandbox: the lines to show before it's installed.
    fn install_asks(&self, ext: &str) -> Option<(String, Vec<String>)> {
        let page = self.find_page(|m| matches!(m, PageModel::Extensions(_)))?;
        let Some(PageModel::Extensions(page)) = self.pages.get(&page).map(|s| &s.model) else { return None };
        let row = page.available().into_iter().find(|r| r.id == ext)?;
        (row.runtime == fenix_extensions::manifest::Runtime::Wasm && !row.permissions.none()).then(|| (row.name.clone(), row.permissions.lines()))
    }

    /// Installs `ext` from the registry -- once you've said yes to what it
    /// asks for, when it's code that asks for anything (#41).
    fn install_from_registry(&mut self, ext: String) {
        if let Some((name, asks)) = self.install_asks(&ext) {
            self.set_message(format!("{name} asks to: {}", asks.join("; ")));
            let candidates = vec![
                fenix_picker::Candidate::new(format!("Install {name}, which may: {}", asks.join("; ")), super::agents::Pick::InstallExtension(ext)),
                fenix_picker::Candidate::new("Don't install it".to_string(), super::agents::Pick::Nothing),
            ];
            self.enter_picker(ActivePicker::Agents(fenix_picker::PickerState::new(candidates)));
            return;
        }
        self.install_from_registry_now(ext);
    }

    /// Installs `ext` from the registry, off the UI thread.
    pub(super) fn install_from_registry_now(&mut self, ext: String) {
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
                    Ok(said) => {
                        let waiting = self.extensions_switched();
                        self.set_message(format!("{} {}", said.replace('\n', " "), in_use_said(&waiting)));
                    }
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

    /// `extensions.toml` changed under Fenix -- `fenix --install-extension`
    /// in a terminal, another Fenix's Extensions page: what changed put in
    /// use, as the Extensions page does, rather than at the next start
    /// (#40). Looked at on the disk poll's tick.
    pub(super) fn extensions_installed_if_changed(&mut self) {
        let stamps = extensions_stamps();
        let Some(before) = self.extensions_stamp.replace(stamps.clone()) else { return };
        if before == stamps {
            return;
        }
        let ids = |installed: &[crate::installed::CodeExtension]| installed.iter().map(|e| (e.id.clone(), e.manifest.extension.version.clone())).collect::<Vec<_>>();
        let was = ids(&self.wasm.installed);
        let waiting = self.extensions_switched();
        if ids(&self.wasm.installed) != was || !waiting.is_empty() {
            let later = if waiting.is_empty() { String::new() } else { format!(" -- {}'s themes and languages come with the next start (SPC q r restarts now)", waiting.join(", ")) };
            self.set_message(format!("Extensions changed outside Fenix: put in use{later}"));
            if let Some(id) = self.find_page(|m| matches!(m, PageModel::Extensions(_))) {
                self.refresh_extensions_page(id);
            }
        }
    }

    /// The installed extensions changed (installed, switched, removed):
    /// what they bring put in use without a restart -- code started or
    /// stopped, keys, commands; snippets, templates, launches and lessons
    /// are read when they're used (#40). Those whose themes or languages
    /// wait for the next start, by id.
    pub(super) fn extensions_switched(&mut self) -> Vec<String> {
        if crate::installed::safe_mode() {
            return Vec::new();
        }
        // What's in use now is what `extensions.toml` says: the disk poll
        // has nothing more to put in use for this change.
        self.extensions_stamp = Some(extensions_stamps());
        self.code_extensions_now(crate::installed::code_extensions());
        let problems = crate::keymap::reload_leader();
        self.leader_matcher = crate::keymap::leader_trie().matcher();
        if !problems.is_empty() {
            self.set_error(format!("keybindings: {}", problems.join("; ")));
        }
        crate::installed::settle_switches()
    }

    /// Where installed extension `id` was, as it was read last: the
    /// Extensions page's row, else nowhere.
    fn extension_dir_of(&self, id: &str) -> PathBuf {
        crate::installed::find(id).map(|(e, _)| e.dir).unwrap_or_default()
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
                    Some(Ok(())) => {
                        let waiting = self.extensions_switched();
                        let state = if on { "on" } else { "off" };
                        if waiting.contains(&ext) {
                            self.set_message(format!("{ext}: {state} from the next start, for its themes and languages -- SPC q r restarts now, where you are"));
                        } else {
                            self.set_message(format!("{ext}: {state} now"));
                        }
                    }
                    Some(Err(e)) => self.set_error(e),
                    None => self.set_error("no folder to keep extensions in"),
                }
                self.refresh_extensions_page(id);
            }
            Action::Uninstall(ext) => {
                // What each brought that's read only at start, looked at
                // while it's still there.
                let at_start: Vec<String> = crate::installed::rows().into_iter().map(|r| r.id).filter(|id| !crate::installed::read_at_start(&self.extension_dir_of(id)).is_empty()).collect();
                match crate::installed::store().map(|store| store.uninstall_unneeded(&ext)) {
                    Some(Ok(removed)) => {
                        let read_at_start = removed.iter().any(|id| at_start.contains(id));
                        self.extensions_switched();
                        for id in &removed {
                            crate::installed::now_in_use(id, false);
                        }
                        if read_at_start {
                            self.set_message(format!("Removed {} -- its themes and languages stay until Fenix next starts (SPC q r restarts now)", crate::installed::said_removed(&removed)));
                        } else {
                            self.set_message(format!("Removed {}", crate::installed::said_removed(&removed)));
                        }
                    }
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

/// What an install says of when it's in use: now, or -- for themes and
/// languages, read only at start -- after `SPC q r`.
fn in_use_said(waiting: &[String]) -> String {
    if waiting.is_empty() {
        "In use now.".to_string()
    } else {
        format!("{}'s themes and languages come with the next start: SPC q r restarts now, where you are.", waiting.join(", "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_install_says_when_it_is_in_use() {
        assert_eq!(in_use_said(&[]), "In use now.");
        assert!(in_use_said(&["acme.dusk".into()]).starts_with("acme.dusk's themes and languages come with the next start"));
    }

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
        app.open_extension_packs();
        assert!(matches!(registry_of(&app), Registry::Off(_)), "nor the welcome, for its bundles");
        app.set_setting("extensions.store", Some(fenix_config::Value::Bool(true))).unwrap();
        app.open_extension_packs();
        assert!(matches!(registry_of(&app), Registry::Looking), "the welcome looks, for its bundles");
    }

    /// A code extension that asks for anything waits for your yes before
    /// it's installed (#41); one that asks for nothing doesn't.
    #[test]
    fn a_code_extension_asks_before_it_installs() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::with_file(None);
        app.config = fenix_config::Config::load_or_default(dir.path().join("settings.toml"));
        app.open_extensions_page();
        let release = |runtime: &str, extra: &str| format!(r#"{{"version": "1.0.0", "fenix": ">=1.0, <2", "runtime": "{runtime}", "url": "u", "sha256": "", "size": 1{extra}}}"#);
        let json = format!(
            r#"{{"format": 1, "extensions": [{{"id": "acme.net", "name": "Net", "description": "x", "versions": [{}]}}, {{"id": "acme.theme", "name": "Theme", "description": "x", "versions": [{}]}}]}}"#,
            release("wasm", r#", "api": "1.0", "permissions": {"network": ["api.example.com"]}"#),
            release("none", ""),
        );
        app.apply_registry_event(RegistryEvent::Read(Ok(serde_json::from_str(&json).unwrap())));
        assert!(app.install_asks("acme.theme").is_none(), "data asks for nothing");
        let (name, asks) = app.install_asks("acme.net").expect("it asks");
        assert_eq!(name, "Net");
        assert!(asks.iter().any(|a| a.contains("api.example.com")), "{asks:?}");
        app.install_from_registry("acme.net".into());
        assert!(matches!(app.active_picker, Some(ActivePicker::Agents(_))), "a question, not an install");
        assert!(app.wasm.installing.is_none());
    }
}
