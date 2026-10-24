//! `SPC h x`, the Extensions page: the built-in extensions, each switched
//! on or off (`extensions.disabled`). A switch takes effect when Fenix
//! next starts -- what's on decides which commands, keys and settings
//! there are -- so a row says both what's running now and what will be.
//!
//! The welcome (`SPC h w`, and a fresh install's first screen) is the
//! same page with packs on top: a few extensions that go together,
//! switched at once.
//!
//! Below the built-in ones: what you installed, then what the registry
//! has that you haven't -- Enter installs one, `u` updates one that has a
//! newer version.

use fenix_extensions::registry::Index;

use crate::page::{fit, frame, Grid, Key, Page, Role};

/// One built-in extension.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub id: &'static str,
    pub name: &'static str,
    pub about: &'static str,
    /// On in this run.
    pub running: bool,
    /// On from the next start (`extensions.disabled` doesn't list it).
    pub on: bool,
}

/// Extensions that go together, offered as one on the welcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pack {
    pub name: &'static str,
    pub about: &'static str,
    pub ids: &'static [&'static str],
}

/// What the welcome offers.
pub const PACKS: &[Pack] = &[
    Pack { name: "Personal", about: "Notebook, Diagrams, Agenda", ids: &["fenix.notebook", "fenix.diagrams", "fenix.agenda"] },
    Pack { name: "Mission", about: "SCOS-2000 MIB, CCSDS & PUS, Tcl", ids: &["fenix.mib", "fenix.ccsds", "fenix.tcl"] },
    Pack { name: "Review", about: "GitHub pull requests, GitLab merge requests", ids: &["fenix.review"] },
    Pack { name: "Jira", about: "Jira, with the agenda kept in step", ids: &["fenix.jira", "fenix.agenda"] },
    Pack { name: "Lab", about: "Arduino, Docker, VNC", ids: &["fenix.arduino", "fenix.docker", "fenix.vnc"] },
    Pack { name: "Reading", about: "The PDF reader", ids: &["fenix.pdf"] },
];

/// An extension you installed (`fenix --install-extension`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledRow {
    pub id: String,
    pub name: String,
    pub version: String,
    pub about: String,
    /// Its data is in use this run.
    pub running: bool,
    /// On from the next start (`extensions.toml`).
    pub on: bool,
    /// What's wrong with it, when its manifest can't be read.
    pub broken: Option<String>,
}

/// What the page knows of the registry.
#[derive(Debug, Clone, Default)]
pub enum Registry {
    /// Not looked at: the store is off, or there's no registry here.
    Off(String),
    /// Being read.
    #[default]
    Looking,
    /// It couldn't be read, or wasn't trusted.
    Failed(String),
    Read(Index),
}

/// An extension the registry has, in a version this Fenix can install.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AvailableRow {
    pub id: String,
    pub name: String,
    pub version: String,
    pub about: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    None,
    Close,
    /// Switch these extensions on (`true`) or off from the next start.
    Switch(Vec<&'static str>, bool),
    /// Switch an installed extension on or off from the next start.
    SwitchInstalled(String, bool),
    /// Remove an installed extension.
    Uninstall(String),
    /// Install this extension from the registry, or update it to its
    /// newest version there.
    Install(String),
    /// Read the registry again.
    Refresh,
    /// Say this on the page.
    Say(String),
}

pub struct ExtensionsPage {
    pub rows: Vec<Row>,
    /// The welcome's packs, above the rows; none on the plain page.
    pub packs: Vec<Pack>,
    /// The cursor, over the packs then the rows.
    pub focus: usize,
    /// This run is safe mode: nothing's running whatever the rows say.
    pub safe_mode: bool,
    /// The extensions you installed, below the built-in ones.
    pub installed: Vec<InstalledRow>,
    /// The installed row `x` was pressed on once; a second `x` removes it.
    pub armed: Option<usize>,
    /// The registry, below what's installed (not on the welcome).
    pub registry: Registry,
    /// The Fenix release, for which of the registry's versions fit.
    pub release: semver::Version,
    /// The extension being installed, one at a time.
    pub installing: Option<String>,
}

impl ExtensionsPage {
    pub fn new(rows: Vec<Row>) -> Self {
        ExtensionsPage { rows, packs: Vec::new(), focus: 0, safe_mode: false, installed: Vec::new(), armed: None, registry: Registry::default(), release: semver::Version::new(2, 0, 0), installing: None }
    }

    /// The welcome: the page with `PACKS` on top -- the ones with
    /// anything in this build.
    pub fn welcome(rows: Vec<Row>) -> Self {
        let packs = PACKS.iter().filter(|pack| pack.ids.iter().any(|id| rows.iter().any(|r| r.id == *id))).copied().collect();
        let mut page = ExtensionsPage::new(rows);
        page.packs = packs;
        page
    }

    /// The extensions of `pack` this build has.
    fn built<'a>(&'a self, pack: &'a Pack) -> impl Iterator<Item = &'static str> + 'a {
        pack.ids.iter().copied().filter(|id| self.rows.iter().any(|r| r.id == *id))
    }

    /// Whether a switch is waiting for a restart.
    pub fn pending(&self) -> usize {
        self.rows.iter().filter(|r| r.on != r.running).count() + self.installed.iter().filter(|r| r.on != r.running).count()
    }

    /// The installed row under the cursor, if it's on one.
    fn installed_at_focus(&self) -> Option<usize> {
        self.focus.checked_sub(self.packs.len() + self.rows.len()).filter(|i| *i < self.installed.len())
    }

    /// What the registry has that isn't installed, for this Fenix; none
    /// on the welcome.
    pub fn available(&self) -> Vec<AvailableRow> {
        let Registry::Read(index) = &self.registry else { return Vec::new() };
        if !self.packs.is_empty() {
            return Vec::new();
        }
        index
            .extensions
            .iter()
            .filter(|l| !self.installed.iter().any(|r| r.id == l.id))
            .filter_map(|l| l.latest_for(&self.release).map(|r| AvailableRow { id: l.id.clone(), name: l.name.clone(), version: r.version.clone(), about: l.description.clone() }))
            .collect()
    }

    /// The newer version the registry has of installed row `row`, if any.
    pub fn update_of(&self, row: &InstalledRow) -> Option<String> {
        let Registry::Read(index) = &self.registry else { return None };
        let have = [(row.id.clone(), row.version.clone())];
        fenix_extensions::registry::updates(index, &have, &self.release).into_iter().next().map(|u| u.latest)
    }

    /// Every row the cursor goes over.
    pub fn count(&self) -> usize {
        self.packs.len() + self.rows.len() + self.installed.len() + self.available().len()
    }

    /// One install at a time; then `id`.
    fn install(&self, id: &str) -> Action {
        match &self.installing {
            Some(busy) => Action::Say(format!("{busy} is still being installed -- one at a time")),
            None => Action::Install(id.to_string()),
        }
    }

    /// Whether every extension in `pack` is on from the next start.
    pub fn pack_on(&self, pack: &Pack) -> bool {
        self.built(pack).all(|id| self.rows.iter().any(|r| r.id == id && r.on))
    }

    pub fn key(&mut self, key: Key) -> Action {
        let count = self.count();
        // Removing takes a second `x` on the same row; anything else
        // backs out.
        let armed = self.armed.take();
        if key == Key::Char('x') {
            let Some(i) = self.installed_at_focus() else { return Action::None };
            let row = &self.installed[i];
            if armed == Some(i) {
                return Action::Uninstall(row.id.clone());
            }
            self.armed = Some(i);
            return Action::Say(format!("x again removes {} -- anything else keeps it", row.name));
        }
        match key {
            Key::Char('q') | Key::Escape => Action::Close,
            Key::Char('r') if self.packs.is_empty() => Action::Refresh,
            Key::Char('u') => match self.installed_at_focus().map(|i| &self.installed[i]) {
                Some(row) => match self.update_of(row) {
                    Some(_) => self.install(&row.id),
                    None => Action::Say(format!("{} is the newest the registry has for this Fenix", row.name)),
                },
                None => Action::None,
            },
            Key::Char('j') | Key::Down | Key::Tab => {
                self.focus = (self.focus + 1).min(count.saturating_sub(1));
                Action::None
            }
            Key::Char('k') | Key::Up | Key::BackTab => {
                self.focus = self.focus.saturating_sub(1);
                Action::None
            }
            Key::Char('g') => {
                self.focus = 0;
                Action::None
            }
            Key::Char('G') => {
                self.focus = count.saturating_sub(1);
                Action::None
            }
            // Space is the leader, on a page as anywhere.
            Key::Enter => match self.focus.checked_sub(self.packs.len()) {
                None => {
                    let pack = self.packs[self.focus];
                    Action::Switch(self.built(&pack).collect(), !self.pack_on(&pack))
                }
                Some(i) => match self.rows.get(i) {
                    Some(row) => Action::Switch(vec![row.id], !row.on),
                    None => match self.installed.get(i - self.rows.len()) {
                        Some(row) if row.broken.is_none() => Action::SwitchInstalled(row.id.clone(), !row.on),
                        Some(row) => Action::Say(format!("{} can't be read -- x x removes it", row.name)),
                        None => match self.available().get(i - self.rows.len() - self.installed.len()) {
                            Some(row) => self.install(&row.id),
                            None => Action::None,
                        },
                    },
                },
            },
            _ => Action::None,
        }
    }
}

pub fn layout(page: &ExtensionsPage, cols: usize) -> Page {
    let (left, width) = frame(cols, 100);
    let mut g = Grid::new();
    let welcome = !page.packs.is_empty();
    let end = g.put(1, left, if welcome { "Welcome to Fenix" } else { "Extensions" }, Role::Title);
    g.put(1, end + 2, if welcome { "· pick what you'll use" } else { "· built in" }, Role::Muted);
    let on = page.rows.iter().filter(|r| r.on).count();
    let status = format!("{on} of {} on", page.rows.len());
    g.put(1, (left + width).saturating_sub(status.chars().count()), &status, Role::Muted);
    g.rule(2, left..left + width);
    let pending = page.pending();
    let note = match (welcome, pending) {
        _ if page.safe_mode => "Safe mode: none of these runs this time. What's below is what your settings switch on when Fenix next starts without --disable-extensions.".to_string(),
        (true, 0) => "The core is yours already: editing, files, projects, Git, language servers, debugging, the terminal. Add what else you'll use.".to_string(),
        (false, 0) => "Enter switches one on or off. What's off has no commands, keys, settings or pages.".to_string(),
        (_, n) => format!("{n} change{} -- restart Fenix to apply", if n == 1 { "" } else { "s" }),
    };
    g.put(3, left + 2, &fit(&note, width.saturating_sub(4)), if pending == 0 || page.safe_mode { Role::Muted } else { Role::Warn });
    let name_width = 16;
    let about_x = left + 5 + name_width;
    let mut y = 5;
    if welcome {
        g.heading(y, left, width, "Packs");
        y += 1;
        for (i, pack) in page.packs.iter().enumerate() {
            if i == page.focus {
                g.focus(y, left..left + width);
            }
            let on = page.pack_on(pack);
            g.put(y, left + 2, if on { "●" } else { "○" }, if on { Role::Good } else { Role::Muted });
            g.put(y, left + 4, &fit(pack.name, name_width), if on { Role::Text } else { Role::Muted });
            g.put(y, about_x, &fit(pack.about, (left + width).saturating_sub(about_x)), Role::Muted);
            y += 1;
        }
        y += 1;
        g.heading(y, left, width, "One by one");
        y += 1;
    }
    for (i, row) in page.rows.iter().enumerate() {
        if i + page.packs.len() == page.focus {
            g.focus(y, left..left + width);
        }
        g.put(y, left + 2, if row.on { "●" } else { "○" }, if row.on { Role::Good } else { Role::Muted });
        g.put(y, left + 4, &fit(row.name, name_width), if row.on { Role::Text } else { Role::Muted });
        let state = match (row.running, row.on) {
            (true, true) => "on",
            (false, false) => "off",
            (true, false) => "off after a restart",
            (false, true) => "on after a restart",
        };
        let state_x = (left + width).saturating_sub(state.chars().count());
        g.put(y, about_x, &fit(row.about, state_x.saturating_sub(about_x + 2)), Role::Muted);
        g.put(y, state_x, state, if row.on == row.running { Role::Muted } else { Role::Warn });
        y += 1;
        g.put(y, about_x, row.id, Role::Muted);
        y += 1;
    }
    // Yours, installed: their data (themes, snippets, templates) read
    // beside yours.
    if !welcome {
        y += 1;
        g.heading(y, left, width, "Installed");
        y += 1;
        if page.installed.is_empty() {
            g.put(y, left + 2, &fit("None yet -- install one from Available below, or with fenix --install-extension <file.fenixext>.", width.saturating_sub(4)), Role::Muted);
            y += 1;
        }
        let first = page.packs.len() + page.rows.len();
        for (i, row) in page.installed.iter().enumerate() {
            if first + i == page.focus {
                g.focus(y, left..left + width);
            }
            g.put(y, left + 2, if row.on { "●" } else { "○" }, if row.on { Role::Good } else { Role::Muted });
            g.put(y, left + 4, &fit(&row.name, name_width), if row.on { Role::Text } else { Role::Muted });
            let update = page.update_of(row);
            let state = if page.armed == Some(i) {
                "x again removes it"
            } else if page.installing.as_deref() == Some(row.id.as_str()) {
                "updating…"
            } else {
                match (row.running, row.on) {
                    (true, true) => "on",
                    (false, false) => "off",
                    (true, false) => "off after a restart",
                    (false, true) => "on after a restart",
                }
            };
            let state_x = (left + width).saturating_sub(state.chars().count());
            let about = row.broken.as_deref().unwrap_or(&row.about);
            g.put(y, about_x, &fit(about, state_x.saturating_sub(about_x + 2)), if row.broken.is_some() { Role::Warn } else { Role::Muted });
            g.put(y, state_x, state, if row.on == row.running && page.armed != Some(i) { Role::Muted } else { Role::Warn });
            y += 1;
            let end = g.put(y, about_x, &format!("{} {}", row.id, row.version), Role::Muted);
            if let Some(latest) = &update {
                g.put(y, end + 2, &format!("· {latest} available -- u updates"), Role::Good);
            }
            y += 1;
        }
        // The registry's, not installed.
        y += 1;
        g.heading(y, left, width, "Available");
        y += 1;
        let available = page.available();
        let say = match &page.registry {
            Registry::Off(why) => Some((why.clone(), Role::Muted)),
            Registry::Looking => Some(("Looking at the registry…".to_string(), Role::Muted)),
            Registry::Failed(why) => Some((format!("The registry couldn't be read: {why} -- r tries again"), Role::Warn)),
            Registry::Read(_) if available.is_empty() => Some(("Nothing more for this Fenix -- you have everything the registry has.".to_string(), Role::Muted)),
            Registry::Read(_) => None,
        };
        if let Some((text, role)) = say {
            g.put(y, left + 2, &fit(&text, width.saturating_sub(4)), role);
        }
        let first = page.packs.len() + page.rows.len() + page.installed.len();
        for (i, row) in available.iter().enumerate() {
            if first + i == page.focus {
                g.focus(y, left..left + width);
            }
            g.put(y, left + 2, "+", Role::Muted);
            g.put(y, left + 4, &fit(&row.name, name_width), Role::Text);
            let state = if page.installing.as_deref() == Some(row.id.as_str()) { "installing…" } else { "Enter installs" };
            let state_x = (left + width).saturating_sub(state.chars().count());
            g.put(y, about_x, &fit(&row.about, state_x.saturating_sub(about_x + 2)), Role::Muted);
            g.put(y, state_x, state, if page.installing.as_deref() == Some(row.id.as_str()) { Role::Warn } else { Role::Muted });
            y += 1;
            g.put(y, about_x, &format!("{} {}", row.id, row.version), Role::Muted);
            y += 1;
        }
    }
    let mut keys = vec![("Enter", if welcome { "switch on / off" } else { "switch / install" })];
    if page.installed.iter().any(|r| page.update_of(r).is_some()) {
        keys.push(("u", "update"));
    }
    if !page.installed.is_empty() {
        keys.push(("x x", "remove installed"));
    }
    if !welcome {
        keys.push(("r", "registry again"));
    }
    keys.extend([("j k", "move"), ("q", "close")]);
    g.keys(left, width, &keys);
    g.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page() -> ExtensionsPage {
        ExtensionsPage::new(vec![
            Row { id: "fenix.jira", name: "Jira", about: "Your issues", running: true, on: true },
            Row { id: "fenix.pdf", name: "PDF reader", about: "Read PDFs in a pane", running: false, on: false },
        ])
    }

    #[test]
    fn enter_switches_the_row_under_the_cursor() {
        let mut p = page();
        assert_eq!(p.key(Key::Enter), Action::Switch(vec!["fenix.jira"], false));
        p.key(Key::Char('j'));
        assert_eq!(p.key(Key::Enter), Action::Switch(vec!["fenix.pdf"], true));
        assert_eq!(p.key(Key::Char('q')), Action::Close);
    }

    fn installed(id: &str, on: bool, broken: Option<&str>) -> InstalledRow {
        InstalledRow { id: id.into(), name: id.into(), version: "1.0.0".into(), about: "Themes".into(), running: on, on, broken: broken.map(str::to_string) }
    }

    /// Installed extensions sit below the built-in ones: Enter switches
    /// one, `x` twice removes it, and anything between backs out.
    #[test]
    fn installed_extensions_switch_and_are_removed_with_a_second_x() {
        let mut p = page();
        p.installed = vec![installed("acme.dusk", true, None), installed("acme.broken", true, Some("extension.toml: no version"))];
        let text = layout(&p, 160).all_text();
        assert!(text.contains("INSTALLED") && text.contains("acme.dusk 1.0.0") && text.contains("no version"), "{text}");
        p.focus = 2;
        assert_eq!(p.key(Key::Enter), Action::SwitchInstalled("acme.dusk".into(), false));
        assert!(matches!(p.key(Key::Char('x')), Action::Say(s) if s.starts_with("x again removes acme.dusk")));
        assert!(layout(&p, 160).all_text().contains("x again removes it"));
        assert_eq!(p.key(Key::Char('x')), Action::Uninstall("acme.dusk".into()));
        p.key(Key::Char('x'));
        p.key(Key::Char('j'));
        assert_eq!(p.armed, None, "moving away backs out");
        assert_eq!(p.focus, 3);
        assert!(matches!(p.key(Key::Enter), Action::Say(s) if s.contains("can't be read")));
        p.focus = 0;
        assert_eq!(p.key(Key::Char('x')), Action::None, "x does nothing on a built-in one");
    }

    fn index(json_versions: &[(&str, &str)]) -> Index {
        let listings: Vec<String> = json_versions
            .iter()
            .map(|(id, version)| format!(r#"{{"id": "{id}", "name": "{id}", "description": "Themes", "versions": [{{"version": "{version}", "fenix": ">=2.0, <3", "runtime": "none", "url": "u", "sha256": "", "size": 1}}]}}"#))
            .collect();
        serde_json::from_str(&format!(r#"{{"format": 1, "extensions": [{}]}}"#, listings.join(","))).unwrap()
    }

    /// What the registry has and you don't is listed below what you
    /// have: Enter installs it, `u` updates an installed one with a newer
    /// version there, one at a time.
    #[test]
    fn the_registry_offers_what_isnt_installed_and_newer_versions() {
        let mut p = page();
        assert!(layout(&p, 160).all_text().contains("Looking at the registry"));
        p.installed = vec![installed("acme.dusk", true, None)];
        p.registry = Registry::Read(index(&[("acme.dusk", "1.2.0"), ("acme.dawn", "1.0.0")]));
        assert_eq!(p.available().iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["acme.dawn"]);
        let text = layout(&p, 160).all_text();
        assert!(text.contains("AVAILABLE") && text.contains("acme.dawn 1.0.0") && text.contains("1.2.0 available -- u updates"), "{text}");
        p.focus = 2;
        assert_eq!(p.key(Key::Char('u')), Action::Install("acme.dusk".into()));
        p.key(Key::Char('j'));
        assert_eq!(p.focus, 3, "onto the available one");
        assert_eq!(p.key(Key::Enter), Action::Install("acme.dawn".into()));
        p.installing = Some("acme.dawn".into());
        assert!(layout(&p, 160).all_text().contains("installing…"));
        assert!(matches!(p.key(Key::Enter), Action::Say(s) if s.contains("one at a time")));
        p.installed[0].version = "1.2.0".into();
        p.focus = 2;
        assert!(matches!(p.key(Key::Char('u')), Action::Say(s) if s.contains("the newest")));
        assert_eq!(p.key(Key::Char('r')), Action::Refresh);
        p.registry = Registry::Failed("offline".into());
        assert!(layout(&p, 160).all_text().contains("couldn't be read: offline -- r tries again"));
        assert!(p.available().is_empty());
    }

    /// The welcome is for the built-in ones: no registry on it.
    #[test]
    fn the_welcome_offers_no_registry() {
        let mut p = ExtensionsPage::welcome(vec![Row { id: "fenix.pdf", name: "PDF", about: "", running: false, on: false }]);
        p.registry = Registry::Read(index(&[("acme.dawn", "1.0.0")]));
        assert!(p.available().is_empty());
        assert!(!layout(&p, 160).all_text().contains("AVAILABLE"));
    }

    #[test]
    fn with_nothing_installed_the_page_says_how() {
        let text = layout(&page(), 160).all_text();
        assert!(text.contains("None yet -- install one from Available below"), "{text}");
    }

    #[test]
    fn safe_mode_says_nothing_runs_rather_than_changes_waiting() {
        let mut p = page();
        p.rows[0].running = false;
        p.safe_mode = true;
        let text = layout(&p, 160).all_text();
        assert!(text.contains("Safe mode: none of these runs") && !text.contains("restart Fenix to apply"), "{text}");
    }

    #[test]
    fn a_switch_waiting_for_a_restart_says_so() {
        let mut p = page();
        let text = layout(&p, 120).all_text();
        assert!(text.contains("Jira") && text.contains("PDF reader") && text.contains("1 of 2 on"), "{text}");
        assert!(!text.contains("restart Fenix"));
        p.rows[0].on = false;
        let text = layout(&p, 120).all_text();
        assert!(text.contains("off after a restart") && text.contains("1 change -- restart Fenix to apply"), "{text}");
    }

    /// The welcome's packs come first; Enter switches all of one, on
    /// unless it's all on already.
    #[test]
    fn a_pack_switches_its_extensions_together() {
        let rows = PACKS.iter().flat_map(|p| p.ids).map(|id| Row { id, name: id, about: "", running: false, on: false }).collect::<Vec<_>>();
        let mut rows: Vec<Row> = rows.into_iter().fold(Vec::new(), |mut out, r| {
            if !out.iter().any(|o: &Row| o.id == r.id) {
                out.push(r);
            }
            out
        });
        let mut p = ExtensionsPage::welcome(rows.clone());
        assert!(layout(&p, 120).all_text().contains("Welcome to Fenix"));
        assert_eq!(p.key(Key::Enter), Action::Switch(vec!["fenix.notebook", "fenix.diagrams", "fenix.agenda"], true));
        for r in rows.iter_mut().filter(|r| ["fenix.notebook", "fenix.diagrams", "fenix.agenda"].contains(&r.id)) {
            r.on = true;
        }
        p.rows = rows;
        assert_eq!(p.key(Key::Enter), Action::Switch(vec!["fenix.notebook", "fenix.diagrams", "fenix.agenda"], false), "all on: off again");
        p.focus = PACKS.len();
        assert!(matches!(p.key(Key::Enter), Action::Switch(ids, _) if ids.len() == 1), "below the packs, one at a time");
    }

    /// A build without some extensions offers only what it has: a pack
    /// with nothing built isn't shown, and one with part of it switches
    /// that part.
    #[test]
    fn packs_keep_to_the_extensions_built() {
        let row = |id| Row { id, name: id, about: "", running: false, on: false };
        let mut p = ExtensionsPage::welcome(vec![row("fenix.arduino"), row("fenix.vnc"), row("fenix.pdf")]);
        assert_eq!(p.packs.iter().map(|pack| pack.name).collect::<Vec<_>>(), ["Lab", "Reading"]);
        assert_eq!(p.key(Key::Enter), Action::Switch(vec!["fenix.arduino", "fenix.vnc"], true), "no Docker in this build");
        p.rows[0].on = true;
        p.rows[1].on = true;
        assert!(p.pack_on(&p.packs[0]), "on, as far as this build goes");
    }
}
