//! `SPC h x`, the Extensions page, in four tabs (`Tab`, or `1`-`4`):
//! Installed -- each switched on or off, which takes effect when Fenix
//! next starts, so a row says both what's running now and what will be;
//! Updates (`u` one, `U` all); Browse, what the registry has that you
//! haven't (Enter installs one); and Bundles, extensions that go
//! together, installed and removed as one. The categories down the left
//! (`h`, `l` back) keep a tab to one kind; `/` searches by words, and
//! `category:`, `runtime:` (data, code, bundle) and `asks:` (a
//! permission: network, secrets, process, ...) narrow it. The selected
//! one's details are on the right; `d` opens them whole, its README,
//! changes and screenshots with them, in the reading view. Only the rows
//! that fit are laid out, so hundreds are as quick as ten.
//!
//! The welcome (`SPC h w`, and a fresh install's first screen) is the
//! bundles alone: the registry's, or -- until it's read, or when it
//! can't be -- `FALLBACK_BUNDLES`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use fenix_extensions::manifest::CATEGORIES;
use fenix_extensions::registry::{Index, Listing};
use fenix_extensions::{Permissions, Runtime};

use crate::page::{fit, frame, wrap, Filter, Grid, ImageKey, Key, Page, Role};

/// A bundle Fenix knows without the registry: the registry's own
/// (fenix-extensions' `extensions/*-bundle`), as they were when this
/// Fenix was built.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fallback {
    pub id: &'static str,
    pub name: &'static str,
    pub about: &'static str,
    pub categories: &'static [&'static str],
    pub members: &'static [&'static str],
}

/// The welcome is a fresh install's first screen, maybe before there's a
/// network: these are shown until the registry is read, and in its place
/// when it can't be or has no bundles yet, so there's always something
/// to pick. Picking one installs its members by id, which takes the
/// registry -- offline, the page says why it can't. This is also the
/// order bundles are shown in.
pub const FALLBACK_BUNDLES: &[Fallback] = &[
    Fallback { id: "fenix.personal-bundle", name: "Personal", about: "Notebook, Diagrams, Agenda", categories: &["tools"], members: &["fenix.notebook", "fenix.diagrams", "fenix.agenda"] },
    Fallback { id: "fenix.mission-bundle", name: "Mission", about: "SCOS-2000 MIB, CCSDS & PUS, Tcl", categories: &["mission"], members: &["fenix.mib", "fenix.ccsds", "fenix.tcl"] },
    Fallback { id: "fenix.review-bundle", name: "Review", about: "GitHub pull requests, GitLab merge requests", categories: &["tools"], members: &["fenix.review"] },
    Fallback { id: "fenix.jira-bundle", name: "Jira", about: "Jira, with the agenda kept in step", categories: &["tools"], members: &["fenix.jira", "fenix.agenda"] },
    Fallback { id: "fenix.lab-bundle", name: "Lab", about: "Arduino, containers, VNC", categories: &["tools"], members: &["fenix.containers", "fenix.arduino", "fenix.vnc"] },
    Fallback { id: "fenix.reading-bundle", name: "Reading", about: "The PDF reader", categories: &["tools"], members: &["fenix.pdf"] },
];

/// What `asks:` takes: the permissions an extension may ask for.
pub const ASKS: &[&str] = &["network", "secrets", "process", "sockets", "files", "folders", "draw", "editor", "fonts", "clipboard", "settings"];

/// The permissions `p` asks for, by `ASKS`' names.
fn asks(p: &Permissions) -> Vec<&'static str> {
    let mut out = Vec::new();
    let mut add = |on: bool, what| {
        if on {
            out.push(what)
        }
    };
    add(!p.network.is_empty(), "network");
    add(!p.secrets.is_empty(), "secrets");
    add(!p.process.is_empty(), "process");
    add(!p.sockets.is_empty(), "sockets");
    add(p.filesystem != fenix_extensions::manifest::Filesystem::None, "files");
    add(!p.folders.is_empty(), "folders");
    add(p.draw != fenix_extensions::manifest::Draw::None, "draw");
    add(p.editor != fenix_extensions::manifest::Editor::None, "editor");
    add(p.fonts, "fonts");
    add(p.clipboard, "clipboard");
    add(!p.settings.is_empty(), "settings");
    out
}

/// The pictures in an extension's `screenshots/`, by name.
pub fn screenshots(dir: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(dir.join("screenshots"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| ["png", "jpg", "jpeg", "gif", "webp"].contains(&x.to_string_lossy().to_ascii_lowercase().as_str())))
        .collect();
    out.sort();
    out
}

/// An extension you installed (`fenix --install-extension`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
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
    /// The machine's, which only its administrator changes.
    pub managed: bool,
    /// What runs it; none when its manifest can't be read.
    pub runtime: Option<Runtime>,
    pub categories: Vec<String>,
    pub permissions: Permissions,
    /// What it needs (a bundle's members), with the versions it takes.
    pub dependencies: BTreeMap<String, String>,
    /// Installed only because another needed it.
    pub dependency: bool,
    /// Its README, CHANGELOG and screenshots, for the details.
    pub readme: Option<String>,
    pub changelog: Option<String>,
    pub screenshots: Vec<PathBuf>,
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
    pub runtime: Runtime,
    pub categories: Vec<String>,
    pub permissions: Permissions,
}

/// A bundle: the registry's, one installed, or one this Fenix knows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BundleRow {
    pub id: String,
    pub name: String,
    pub about: String,
    pub categories: Vec<String>,
    pub members: Vec<String>,
    /// The registry's newest for this Fenix; none when it isn't there.
    pub version: Option<String>,
    /// The bundle itself is installed (not only its members).
    pub installed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    None,
    Close,
    /// Switch an installed extension on or off from the next start.
    SwitchInstalled(String, bool),
    /// Remove an installed extension, and what it brought that nothing
    /// else needs.
    Uninstall(String),
    /// Install this extension (or bundle) from the registry, or update it
    /// to its newest version there.
    Install(String),
    /// Update each of these.
    UpdateAll(Vec<String>),
    /// Read the registry again.
    Refresh,
    /// Show this one's details page.
    Details(Target),
    /// A bundle: its members switched to `on` where they're installed;
    /// switched on, `install` (the registry's bundle) is installed, or --
    /// when the registry has none -- the members that aren't.
    SwitchBundle { on: bool, members: Vec<String>, install: Option<String> },
    /// Say this on the page.
    Say(String),
    /// The welcome's `t`: the tutorial.
    Tutorial,
}

/// What `d` shows the details of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Installed(String),
    Available(String),
}

/// The page's tabs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Tab {
    #[default]
    Installed,
    Updates,
    Browse,
    Bundles,
}

impl Tab {
    pub const ALL: [Tab; 4] = [Tab::Installed, Tab::Updates, Tab::Browse, Tab::Bundles];

    pub fn label(self) -> &'static str {
        match self {
            Tab::Installed => "Installed",
            Tab::Updates => "Updates",
            Tab::Browse => "Browse",
            Tab::Bundles => "Bundles",
        }
    }
}

/// One row the cursor goes over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Entry {
    Installed(usize),
    Available(usize),
    Bundle(usize),
}

/// The search, read: its words, and what `category:`, `runtime:` and
/// `asks:` keep it to.
#[derive(Debug, Default)]
struct Query {
    words: Vec<String>,
    category: Option<String>,
    runtime: Option<String>,
    asks: Vec<String>,
}

/// What a row is searched and sorted by.
struct Facets<'a> {
    hay: String,
    categories: &'a [String],
    runtime: Option<Runtime>,
    permissions: Option<&'a Permissions>,
}

/// Whether `categories` file a row under `category` (`other`: none of
/// the page's).
fn filed_under(categories: &[String], category: &str) -> bool {
    match category {
        "other" => !categories.iter().any(|c| CATEGORIES.iter().any(|(k, _)| k == c)),
        _ => categories.iter().any(|c| c == category),
    }
}

/// Whether `runtime:word` takes `runtime`.
fn runtime_is(word: &str, runtime: Option<Runtime>) -> bool {
    match word {
        "data" | "none" => runtime == Some(Runtime::None),
        "code" => matches!(runtime, Some(Runtime::Wasm | Runtime::Lua)),
        "wasm" => runtime == Some(Runtime::Wasm),
        "lua" => runtime == Some(Runtime::Lua),
        "bundle" => runtime == Some(Runtime::Bundle),
        _ => false,
    }
}

/// The lines above the list: title, rule, tabs, note, search, and the
/// line that says what's above.
const LIST_TOP: usize = 7;

pub struct ExtensionsPage {
    /// The welcome: bundles alone, no tabs.
    pub welcome: bool,
    pub tab: Tab,
    /// The category the list keeps to (`other` for none of the page's);
    /// none, every one.
    pub category: Option<String>,
    /// The categories have the keyboard, not the list.
    pub sidebar: bool,
    /// The cursor, over what's shown (`entries`).
    pub focus: usize,
    /// The first entry laid out: the list is a window over them.
    pub top: usize,
    /// The lines the pane shows, as it last said.
    pub rows: usize,
    /// This run is safe mode: nothing's running whatever the rows say.
    pub safe_mode: bool,
    /// The extensions you installed.
    pub installed: Vec<InstalledRow>,
    /// The installed extension `x` was pressed on once; a second `x`
    /// removes it.
    pub armed: Option<String>,
    pub registry: Registry,
    /// The Fenix release, for which of the registry's versions fit.
    pub release: semver::Version,
    /// The extension being installed, one at a time.
    pub installing: Option<String>,
    /// `/`: only what has every word typed, in its name, id or about, and
    /// what its `category:`, `runtime:` and `asks:` keep it to.
    pub filter: Filter,
}

impl ExtensionsPage {
    pub fn new() -> Self {
        ExtensionsPage {
            welcome: false,
            tab: Tab::Installed,
            category: None,
            sidebar: false,
            focus: 0,
            top: 0,
            rows: 40,
            safe_mode: false,
            installed: Vec::new(),
            armed: None,
            registry: Registry::default(),
            release: crate::installed::release(),
            installing: None,
            filter: Filter::default(),
        }
    }

    /// The welcome: the bundles.
    pub fn welcome() -> Self {
        ExtensionsPage { welcome: true, tab: Tab::Bundles, ..ExtensionsPage::new() }
    }

    /// Whether a switch is waiting for a restart.
    pub fn pending(&self) -> usize {
        self.installed.iter().filter(|r| r.runtime != Some(Runtime::Bundle) && r.on != r.running).count()
    }

    fn index(&self) -> Option<&Index> {
        match &self.registry {
            Registry::Read(index) => Some(index),
            _ => None,
        }
    }

    /// What the registry has that isn't installed, for this Fenix:
    /// extensions, not bundles.
    pub fn available(&self) -> Vec<AvailableRow> {
        let Some(index) = self.index() else { return Vec::new() };
        index
            .extensions
            .iter()
            .filter(|l| !self.installed.iter().any(|r| r.id == l.id))
            .filter_map(|l| {
                let r = l.latest_for(&self.release)?;
                let categories = if r.categories.is_empty() { l.categories.clone() } else { r.categories.clone() };
                Some(AvailableRow { id: l.id.clone(), name: l.name.clone(), version: r.version.clone(), about: l.description.clone(), runtime: r.runtime, categories, permissions: r.permissions.clone() })
            })
            .collect()
    }

    /// The bundles: the registry's for this Fenix, those installed it
    /// hasn't -- and, when it has none (not read, not readable, or from
    /// before bundles), `FALLBACK_BUNDLES`. In that one's order, then by
    /// name.
    pub fn bundles(&self) -> Vec<BundleRow> {
        let installed = |id: &str| self.installed.iter().any(|r| r.id == id && r.runtime == Some(Runtime::Bundle));
        let mut out: Vec<BundleRow> = Vec::new();
        for l in self.index().map(|i| i.bundles.as_slice()).unwrap_or_default() {
            if let Some(r) = l.latest_for(&self.release) {
                let categories = if r.categories.is_empty() { l.categories.clone() } else { r.categories.clone() };
                out.push(BundleRow { id: l.id.clone(), name: l.name.clone(), about: l.description.clone(), categories, members: r.members(), version: Some(r.version.clone()), installed: installed(&l.id) });
            }
        }
        for r in self.installed.iter().filter(|r| r.runtime == Some(Runtime::Bundle)) {
            if out.iter().any(|b| b.id == r.id) {
                continue;
            }
            out.push(BundleRow { id: r.id.clone(), name: r.name.clone(), about: r.about.clone(), categories: r.categories.clone(), members: r.dependencies.keys().cloned().collect(), version: None, installed: true });
        }
        if !out.iter().any(|b| b.version.is_some()) {
            for f in FALLBACK_BUNDLES {
                if out.iter().any(|b| b.id == f.id) {
                    continue;
                }
                out.push(BundleRow {
                    id: f.id.into(),
                    name: f.name.into(),
                    about: f.about.into(),
                    categories: f.categories.iter().map(|c| c.to_string()).collect(),
                    members: f.members.iter().map(|m| m.to_string()).collect(),
                    version: None,
                    installed: false,
                });
            }
        }
        let rank = |b: &BundleRow| FALLBACK_BUNDLES.iter().position(|f| f.id == b.id).unwrap_or(usize::MAX);
        out.sort_by(|a, b| rank(a).cmp(&rank(b)).then_with(|| a.name.cmp(&b.name)));
        out
    }

    /// The bundles' names that have `id` in them.
    pub fn bundles_of(&self, id: &str) -> Vec<String> {
        self.bundles().into_iter().filter(|b| b.members.iter().any(|m| m == id)).map(|b| b.name).collect()
    }

    /// How bundle `b` stands: members installed, on, and all of them.
    pub fn bundle_state(&self, b: &BundleRow) -> (usize, usize, usize) {
        let rows: Vec<&InstalledRow> = b.members.iter().filter_map(|m| self.installed.iter().find(|r| r.id == *m)).collect();
        (rows.len(), rows.iter().filter(|r| r.on).count(), b.members.len())
    }

    /// The newer version the registry has of installed row `row`, if any.
    pub fn update_of(&self, row: &InstalledRow) -> Option<String> {
        let index = self.index()?;
        if row.managed {
            return None;
        }
        let have = [(row.id.clone(), row.version.clone())];
        fenix_extensions::registry::updates(index, &have, &self.release).into_iter().next().map(|u| u.latest)
    }

    fn query(&self) -> Query {
        let mut q = Query::default();
        for word in self.filter.words() {
            match word.split_once(':') {
                Some(("category" | "cat", v)) => q.category = Some(v.to_string()),
                Some(("runtime" | "kind", v)) => q.runtime = Some(v.to_string()),
                Some(("asks" | "permission", v)) => q.asks.push(v.to_string()),
                _ => q.words.push(word),
            }
        }
        q
    }

    /// What row `entry` is searched by.
    fn facets<'a>(&'a self, entry: Entry, available: &'a [AvailableRow], bundles: &'a [BundleRow]) -> Facets<'a> {
        match entry {
            Entry::Installed(i) => {
                let r = &self.installed[i];
                Facets { hay: format!("{} {} {}", r.name, r.id, r.about).to_lowercase(), categories: &r.categories, runtime: r.runtime, permissions: Some(&r.permissions) }
            }
            Entry::Available(i) => {
                let r = &available[i];
                Facets { hay: format!("{} {} {}", r.name, r.id, r.about).to_lowercase(), categories: &r.categories, runtime: Some(r.runtime), permissions: Some(&r.permissions) }
            }
            Entry::Bundle(i) => {
                let b = &bundles[i];
                Facets { hay: format!("{} {} {} {}", b.name, b.id, b.about, b.members.join(" ")).to_lowercase(), categories: &b.categories, runtime: Some(Runtime::Bundle), permissions: None }
            }
        }
    }

    /// Whether `f` is in what the search (and, `with_category`, the
    /// category) keeps to.
    fn admits(&self, q: &Query, f: &Facets, with_category: bool) -> bool {
        // Typed, a category is its start too (`category:lang`); picked down
        // the left, the whole of it.
        let typed = q.category.as_deref().is_none_or(|c| filed_under(f.categories, c) || (c != "other" && f.categories.iter().any(|k| k.starts_with(c))));
        let picked = if with_category && q.category.is_none() { self.category.as_deref() } else { None };
        q.words.iter().all(|w| f.hay.contains(w.as_str()))
            && typed
            && picked.is_none_or(|c| filed_under(f.categories, c))
            && q.runtime.as_deref().is_none_or(|r| runtime_is(r, f.runtime))
            && q.asks.iter().all(|a| match f.permissions {
                Some(p) if a == "nothing" || a == "none" => p.none(),
                Some(p) => asks(p).contains(&a.as_str()),
                None => false,
            })
    }

    /// Every row of `tab`, before the search.
    fn tab_entries(&self, tab: Tab, available: &[AvailableRow], bundles: &[BundleRow]) -> Vec<Entry> {
        let not_bundle = |i: &usize| self.installed[*i].runtime != Some(Runtime::Bundle);
        match tab {
            Tab::Installed => (0..self.installed.len()).filter(not_bundle).map(Entry::Installed).collect(),
            Tab::Updates => (0..self.installed.len()).filter(|&i| self.update_of(&self.installed[i]).is_some()).map(Entry::Installed).collect(),
            Tab::Browse => (0..available.len()).map(Entry::Available).collect(),
            Tab::Bundles => (0..bundles.len()).map(Entry::Bundle).collect(),
        }
    }

    /// What `tab` shows: its rows the search and category keep.
    fn entries_of(&self, tab: Tab, available: &[AvailableRow], bundles: &[BundleRow]) -> Vec<Entry> {
        let q = self.query();
        self.tab_entries(tab, available, bundles).into_iter().filter(|e| self.admits(&q, &self.facets(*e, available, bundles), true)).collect()
    }

    fn entries(&self) -> Vec<Entry> {
        self.entries_of(self.tab, &self.available(), &self.bundles())
    }

    /// The sidebar: every category (`None`, all of them), with how many of
    /// the tab's rows the search keeps are in it.
    pub fn categories(&self) -> Vec<(Option<String>, String, usize)> {
        let (available, bundles) = (self.available(), self.bundles());
        let q = self.query();
        let all: Vec<Facets> = self.tab_entries(self.tab, &available, &bundles).into_iter().map(|e| self.facets(e, &available, &bundles)).filter(|f| self.admits(&q, f, false)).collect();
        let count = |c: &str| all.iter().filter(|f| filed_under(f.categories, c)).count();
        let mut out = vec![(None, "All".to_string(), all.len())];
        out.extend(CATEGORIES.iter().map(|(key, label)| (Some(key.to_string()), label.to_string(), count(key))));
        out.push((Some("other".into()), "Other".into(), count("other")));
        out
    }

    /// Every row the cursor goes over.
    pub fn count(&self) -> usize {
        self.entries().len()
    }

    fn at_focus(&self) -> Option<Entry> {
        self.entries().get(self.focus).copied()
    }

    /// How many rows fit below the page's top and above its keys, two
    /// lines each.
    pub fn shown(&self) -> usize {
        (self.rows.saturating_sub(LIST_TOP + 4) / 2).max(3)
    }

    /// The first row laid out, so the cursor's is among those shown.
    fn start(&self, count: usize) -> usize {
        let n = self.shown();
        let top = self.top.min(count.saturating_sub(n));
        if self.focus < top {
            self.focus
        } else if self.focus >= top + n {
            self.focus + 1 - n
        } else {
            top
        }
    }

    /// One install at a time; then `id`.
    fn install(&self, id: &str) -> Action {
        match &self.installing {
            Some(busy) => Action::Say(format!("{busy} is still being installed -- one at a time")),
            None => Action::Install(id.to_string()),
        }
    }

    /// Enter on bundle `b`: its members on (installed when they aren't),
    /// or off again when they all are.
    fn switch_bundle(&self, b: &BundleRow) -> Action {
        let (have, on, all) = self.bundle_state(b);
        let switch_on = on < all;
        // The registry's: installed as a bundle, so it goes as one.
        let install = (switch_on && b.version.is_some() && (have < all || !b.installed)).then(|| b.id.clone());
        if install.is_some() {
            if let Some(busy) = &self.installing {
                return Action::Say(format!("{busy} is still being installed -- one at a time"));
            }
        }
        Action::SwitchBundle { on: switch_on, members: b.members.clone(), install }
    }

    /// Whether the filter has the keyboard.
    pub fn typing(&self) -> bool {
        self.filter.typing
    }

    pub fn paste(&mut self, text: &str) {
        let text: String = text.chars().filter(|c| !c.is_control()).collect();
        self.filter.paste(&text);
        self.focus = 0;
    }

    /// The page shows `tab`, from its top.
    fn show_tab(&mut self, tab: Tab) {
        self.tab = tab;
        self.focus = 0;
        self.top = 0;
    }

    pub fn key(&mut self, key: Key) -> Action {
        let action = self.key_at(key);
        self.top = self.start(self.count());
        action
    }

    fn key_at(&mut self, key: Key) -> Action {
        if self.filter.typing {
            if self.filter.key(key) {
                self.focus = 0;
            }
            return Action::None;
        }
        // Removing takes a second `x` on the same row; anything else
        // backs out.
        let armed = self.armed.take();
        if !self.welcome {
            match key {
                Key::Tab | Key::BackTab => {
                    let at = Tab::ALL.iter().position(|t| *t == self.tab).unwrap_or(0);
                    let next = if key == Key::Tab { (at + 1) % 4 } else { (at + 3) % 4 };
                    self.show_tab(Tab::ALL[next]);
                    return Action::None;
                }
                Key::Char(c @ '1'..='4') => {
                    self.show_tab(Tab::ALL[c as usize - '1' as usize]);
                    return Action::None;
                }
                Key::Char('h') | Key::Left => {
                    self.sidebar = true;
                    return Action::None;
                }
                Key::Char('l') | Key::Right => {
                    self.sidebar = false;
                    return Action::None;
                }
                _ => {}
            }
            if self.sidebar {
                let categories = self.categories();
                let at = categories.iter().position(|(c, _, _)| *c == self.category).unwrap_or(0);
                let to = match key {
                    Key::Char('j') | Key::Down => Some((at + 1).min(categories.len() - 1)),
                    Key::Char('k') | Key::Up => Some(at.saturating_sub(1)),
                    Key::Char('g') => Some(0),
                    Key::Char('G') => Some(categories.len() - 1),
                    Key::Enter => {
                        self.sidebar = false;
                        return Action::None;
                    }
                    _ => None,
                };
                if let Some(to) = to {
                    self.category = categories[to].0.clone();
                    self.focus = 0;
                    self.top = 0;
                    return Action::None;
                }
            }
        }
        let count = self.count();
        let entry = self.at_focus();
        let installed = match entry {
            Some(Entry::Installed(i)) => Some(&self.installed[i]),
            _ => None,
        };
        // The machine's is its administrator's to change.
        if let Some(row) = installed.filter(|r| r.managed) {
            if matches!(key, Key::Enter | Key::Char('x') | Key::Char('u')) {
                return Action::Say(format!("{} is managed on this machine -- its administrator switches, updates and removes it", row.name));
            }
        }
        let bundle = match entry {
            Some(Entry::Bundle(i)) => self.bundles().get(i).cloned(),
            _ => None,
        };
        match key {
            Key::Char('x') => {
                let (id, name) = match (installed, &bundle) {
                    (Some(row), _) => (row.id.clone(), row.name.clone()),
                    (None, Some(b)) if b.installed => (b.id.clone(), b.name.clone()),
                    (None, Some(b)) => return Action::Say(format!("{} isn't installed as a bundle -- remove its members one by one under Installed", b.name)),
                    (None, None) => return Action::None,
                };
                if armed.as_deref() == Some(id.as_str()) {
                    return Action::Uninstall(id);
                }
                self.armed = Some(id);
                Action::Say(format!("x again removes {name}, and what it brought that nothing else needs -- anything else keeps it"))
            }
            Key::Char('q') => Action::Close,
            Key::Escape if !self.filter.is_empty() => {
                self.filter.clear();
                self.focus = 0;
                Action::None
            }
            Key::Escape if self.category.is_some() => {
                self.category = None;
                self.focus = 0;
                Action::None
            }
            Key::Escape => Action::Close,
            Key::Char('/') if !self.welcome => {
                self.filter.start();
                self.focus = 0;
                Action::None
            }
            Key::Char('r') => Action::Refresh,
            Key::Char('t') if self.welcome => Action::Tutorial,
            Key::Char('d') => match entry {
                Some(Entry::Installed(i)) => Action::Details(Target::Installed(self.installed[i].id.clone())),
                Some(Entry::Available(i)) => self.available().get(i).map_or(Action::None, |r| Action::Details(Target::Available(r.id.clone()))),
                Some(Entry::Bundle(_)) => match &bundle {
                    Some(b) if b.installed => Action::Details(Target::Installed(b.id.clone())),
                    Some(b) if b.version.is_some() => Action::Details(Target::Available(b.id.clone())),
                    Some(b) => Action::Say(format!("{}: {} -- the registry has no more of it yet", b.name, b.members.join(", "))),
                    None => Action::None,
                },
                None => Action::None,
            },
            Key::Char('u') => match installed {
                Some(row) => match self.update_of(row) {
                    Some(_) => self.install(&row.id),
                    None => Action::Say(format!("{} is the newest the registry has for this Fenix", row.name)),
                },
                None => Action::None,
            },
            Key::Char('U') => {
                let ids: Vec<String> = self.installed.iter().filter(|r| self.update_of(r).is_some()).map(|r| r.id.clone()).collect();
                match (&self.installing, ids.is_empty()) {
                    (_, true) => Action::Say("Everything installed is the newest the registry has for this Fenix".into()),
                    (Some(busy), _) => Action::Say(format!("{busy} is still being installed -- one at a time")),
                    (None, false) => Action::UpdateAll(ids),
                }
            }
            Key::Char('j') | Key::Down => {
                self.focus = (self.focus + 1).min(count.saturating_sub(1));
                Action::None
            }
            // On the welcome, with no tabs, Tab moves as j and k do.
            Key::Tab => {
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
            Key::Enter => match entry {
                Some(Entry::Bundle(_)) => bundle.map_or(Action::None, |b| self.switch_bundle(&b)),
                Some(Entry::Installed(i)) => {
                    let row = &self.installed[i];
                    match (&row.broken, self.tab) {
                        (None, Tab::Updates) => self.install(&row.id),
                        (None, _) => Action::SwitchInstalled(row.id.clone(), !row.on),
                        (Some(_), _) => Action::Say(format!("{} can't be read -- x x removes it", row.name)),
                    }
                }
                Some(Entry::Available(i)) => self.available().get(i).map_or(Action::None, |r| self.install(&r.id)),
                None => Action::None,
            },
            _ => Action::None,
        }
    }
}

/// What a switch shows: on now, and from the next start.
fn switch_state(running: bool, on: bool) -> &'static str {
    match (running, on) {
        (true, true) => "on",
        (false, false) => "off",
        (true, false) => "off after a restart",
        (false, true) => "on after a restart",
    }
}

/// What runs it, as the page says.
fn kind(runtime: Option<Runtime>) -> &'static str {
    match runtime {
        Some(Runtime::None) => "data",
        Some(Runtime::Wasm) => "code",
        Some(Runtime::Lua) => "Lua code",
        Some(Runtime::Bundle) => "bundle",
        None => "unreadable",
    }
}

/// Up to `max` lines of a README or CHANGELOG, as plain text: its title
/// (the name, said already) left out, and, of a changelog, its newest
/// entry alone.
fn excerpt(text: &str, changelog: bool, max: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut lines = text.lines().skip_while(|l| l.trim().is_empty()).peekable();
    if lines.peek().is_some_and(|l| l.starts_with("# ")) {
        lines.next();
    }
    let mut entries = 0;
    for line in lines {
        if changelog && line.starts_with("## ") {
            entries += 1;
            if entries > 1 {
                break;
            }
        }
        if out.is_empty() && line.trim().is_empty() {
            continue;
        }
        out.push(line.trim_start_matches('#').trim_start().to_string());
        if out.len() >= max {
            break;
        }
    }
    while out.last().is_some_and(|l| l.trim().is_empty()) {
        out.pop();
    }
    out
}

/// The details pane, for the row under the cursor: lines, and a picture.
struct Pane {
    lines: Vec<(String, Role)>,
    picture: Option<PathBuf>,
}

impl Pane {
    fn put(&mut self, text: &str, role: Role, width: usize) {
        if text.is_empty() {
            self.lines.push((String::new(), role));
        }
        self.lines.extend(wrap(text, width).into_iter().map(|l| (l, role)));
    }

    fn heading(&mut self, text: &str) {
        self.lines.push((String::new(), Role::Muted));
        self.lines.push((text.to_uppercase(), Role::Muted));
    }
}

fn pane_of(page: &ExtensionsPage, entry: Entry, available: &[AvailableRow], bundles: &[BundleRow], width: usize) -> Pane {
    let mut p = Pane { lines: Vec::new(), picture: None };
    let index = page.index();
    let listing = |id: &str| index.and_then(|i| i.listing(id));
    let versions = |p: &mut Pane, l: Option<&Listing>| {
        if let Some(l) = l {
            let mut v: Vec<&str> = l.versions.iter().map(|r| r.version.as_str()).collect();
            v.reverse();
            p.put(&format!("Versions: {}", v.join(", ")), Role::Muted, width);
        }
    };
    let common = |p: &mut Pane, id: &str, categories: &[String], runtime: Option<Runtime>, permissions: &Permissions, deps: &BTreeMap<String, String>| {
        let filed: Vec<String> = categories.iter().map(|c| CATEGORIES.iter().find(|(k, _)| k == c).map_or(c.clone(), |(_, l)| l.to_string())).collect();
        p.put(&format!("Kind: {}{}", kind(runtime), if filed.is_empty() { String::new() } else { format!(" · {}", filed.join(", ")) }), Role::Muted, width);
        p.heading("Permissions");
        p.put(&runtime.map_or("Its manifest can't be read".into(), |r| runtime_permissions(r, permissions)), Role::Text, width);
        if runtime != Some(Runtime::Bundle) {
            p.put(&format!("Needs: {}", dependencies(deps).replace('`', "")), Role::Muted, width);
            let of = page.bundles_of(id);
            if !of.is_empty() {
                p.put(&format!("In bundles: {}", of.join(", ")), Role::Muted, width);
            }
        }
    };
    match entry {
        Entry::Installed(i) => {
            let r = &page.installed[i];
            p.put(&r.name, Role::Title, width);
            p.put(&format!("{} {} · {}{}", r.id, r.version, if r.on { "on" } else { "off" }, if r.managed { ", managed" } else if r.dependency { ", for another" } else { "" }), Role::Muted, width);
            p.put("", Role::Muted, width);
            match &r.broken {
                Some(why) => p.put(why, Role::Warn, width),
                None => p.put(&r.about, Role::Text, width),
            }
            if let Some(latest) = page.update_of(r) {
                p.put(&format!("{latest} is in the registry -- u updates"), Role::Good, width);
            }
            common(&mut p, &r.id, &r.categories, r.runtime, &r.permissions, &r.dependencies);
            versions(&mut p, listing(&r.id));
            p.picture = r.screenshots.first().cloned();
            if let Some(text) = &r.readme {
                p.heading("Read me");
                for line in excerpt(text, false, 8) {
                    p.put(&line, Role::Text, width);
                }
            }
            if let Some(text) = &r.changelog {
                p.heading("Changes");
                for line in excerpt(text, true, 6) {
                    p.put(&line, Role::Muted, width);
                }
            }
        }
        Entry::Available(i) => {
            let r = &available[i];
            p.put(&r.name, Role::Title, width);
            p.put(&format!("{} {} · in the registry", r.id, r.version), Role::Muted, width);
            p.put("", Role::Muted, width);
            p.put(&r.about, Role::Text, width);
            let l = listing(&r.id);
            let release = l.and_then(|l| l.latest_for(&page.release));
            common(&mut p, &r.id, &r.categories, Some(r.runtime), &r.permissions, &release.map(|r| r.dependencies.clone()).unwrap_or_default());
            versions(&mut p, l);
            if let Some(release) = release {
                p.put("", Role::Muted, width);
                p.put(&format!("d reads its README, changes and screenshots from the package ({} KB, checked against the signed index).", release.size.div_ceil(1024)), Role::Muted, width);
            }
        }
        Entry::Bundle(i) => {
            let b = &bundles[i];
            p.put(&b.name, Role::Title, width);
            let from = match (&b.version, b.installed) {
                (Some(v), true) => format!("{} {v} · installed", b.id),
                (Some(v), false) => format!("{} {v} · in the registry", b.id),
                (None, true) => format!("{} · installed", b.id),
                (None, false) => "a bundle this Fenix knows; the registry's aren't read".into(),
            };
            p.put(&from, Role::Muted, width);
            p.put("", Role::Muted, width);
            p.put(&b.about, Role::Text, width);
            p.heading("Members");
            for m in &b.members {
                let (mark, role) = match page.installed.iter().find(|r| r.id == *m) {
                    Some(r) if r.on => ("●", Role::Good),
                    Some(_) => ("○ off", Role::Muted),
                    None => ("+ not installed", Role::Muted),
                };
                let name = page.installed.iter().find(|r| r.id == *m).map(|r| r.name.clone()).or_else(|| listing(m).map(|l| l.name.clone())).unwrap_or_else(|| m.clone());
                p.lines.push((fit(&format!("{name} ({m}) {mark}"), width), role));
            }
            p.put("", Role::Muted, width);
            p.put("Enter installs and switches on what isn't; x x removes the bundle and the members only it brought.", Role::Muted, width);
            if let Some(r) = page.installed.iter().find(|r| r.id == b.id) {
                p.picture = r.screenshots.first().cloned();
            }
            versions(&mut p, listing(&b.id));
        }
    }
    p
}

/// The welcome's rows, or a tab's list: two lines each, from `start`.
pub fn layout(page: &ExtensionsPage, cols: usize) -> Page {
    let welcome = page.welcome;
    let (left, width) = frame(cols, if welcome { 100 } else { 160 });
    let mut g = Grid::new();
    let end = g.put(1, left, if welcome { "Welcome to Fenix" } else { "Extensions" }, Role::Title);
    g.put(1, end + 2, if welcome { "· pick what you'll use" } else { "· installed and in the registry" }, Role::Muted);
    let plain = |r: &&InstalledRow| r.runtime != Some(Runtime::Bundle);
    if !welcome {
        let on = page.installed.iter().filter(plain).filter(|r| r.on).count();
        let status = format!("{on} of {} on", page.installed.iter().filter(plain).count());
        g.put(1, (left + width).saturating_sub(status.chars().count()), &status, Role::Muted);
    }
    g.rule(2, left..left + width);
    let available = page.available();
    let bundles = page.bundles();
    if !welcome {
        // The tabs, each with what it has that the search keeps.
        let mut x = left;
        for (i, tab) in Tab::ALL.iter().enumerate() {
            let n = page.entries_of(*tab, &available, &bundles).len();
            let label = format!("{} {} {n}", i + 1, tab.label());
            let role = if *tab == page.tab { Role::Title } else { Role::Muted };
            let end = g.put(3, x, &label, role);
            if *tab == page.tab {
                g.panels.push((3, x.saturating_sub(1)..end + 1));
            }
            x = end + 4;
        }
    }
    let pending = page.pending();
    let note = match (welcome, pending) {
        _ if page.safe_mode => "Safe mode: none of these runs this time. What's below is what your settings switch on when Fenix next starts without --disable-extensions.".to_string(),
        (true, 0) => "The core is yours already: editing, files, projects, Git, language servers, debugging, the terminal. Add what else you'll use.".to_string(),
        (false, 0) => "Enter switches one on or off, or installs it. What's off isn't loaded: no commands, keys, settings or pages.".to_string(),
        (_, n) => format!("{n} change{} -- restart Fenix to apply", if n == 1 { "" } else { "s" }),
    };
    let note_role = if pending == 0 || page.safe_mode { Role::Muted } else { Role::Warn };
    // The welcome has the room to say it all, and offers the tutorial.
    let mut welcome_top = 4;
    if welcome {
        for (i, line) in wrap(&note, width).iter().enumerate() {
            g.put(3 + i, left, line, note_role);
            welcome_top = 3 + i + 2;
        }
        let x = g.put(welcome_top - 1, left, "New to Fenix's keys? ", Role::Text);
        let x = g.put(welcome_top - 1, x, "t", Role::Accent);
        g.put(welcome_top - 1, x, " starts the tutorial, a lesson at a time (SPC h T later).", Role::Text);
        welcome_top += 1;
    } else {
        g.put(4, left, &fit(&note, width), note_role);
    }
    if !welcome {
        let hint = format!("search: words, category:tools, runtime:data|code|bundle, asks:{}", ASKS.join("|"));
        let (text, role) = page.filter.line(&hint, width.min(110));
        let end = g.put(5, left, &text, role);
        if page.filter.typing {
            g.panels.push((5, left..end.max(left + 40)));
        }
    }

    // The columns: categories, the list, the details.
    let side_w = if welcome || width < 90 { 0 } else { 18 };
    let pane_w = if welcome || width < 130 { 0 } else { (width * 2 / 5).min(64) };
    let list_x = left + if side_w > 0 { side_w + 2 } else { 0 };
    let list_w = width.saturating_sub(list_x - left + if pane_w > 0 { pane_w + 3 } else { 0 });
    let pane_x = list_x + list_w + 3;
    let top = if welcome { welcome_top } else { LIST_TOP };

    if side_w > 0 {
        for (i, (category, label, n)) in page.categories().into_iter().enumerate() {
            let y = top + i;
            let chosen = category == page.category;
            let role = if chosen { Role::Title } else if n == 0 { Role::Muted } else { Role::Text };
            g.put(y, left, &fit(&label, side_w - 5), role);
            let count = n.to_string();
            g.put(y, left + side_w - count.chars().count(), &count, Role::Muted);
            if chosen {
                if page.sidebar {
                    g.focus(y, left..left + side_w);
                } else {
                    g.panels.push((y, left..left + side_w));
                }
            }
        }
    }

    let entries = page.entries_of(page.tab, &available, &bundles);
    let start = page.start(entries.len());
    let shown = page.shown();
    let window = start..(start + shown).min(entries.len());
    let name_width = 18.min(list_w / 4);
    let about_x = list_x + 4 + name_width + 1;
    let mut y = top;
    if start > 0 {
        g.put(top - 1, list_x + 2, &format!("↑ {start} more"), Role::Muted);
    }
    // A row: its mark, name, about and state, then its id (and what
    // follows it) below.
    let row = |g: &mut Grid, y: &mut usize, at: usize, mark: (&str, Role), name: &str, about: (&str, Role), state: (&str, Role), id: &str| {
        if at == page.focus && !page.sidebar {
            g.focus(*y, list_x..list_x + list_w);
        }
        g.put(*y, list_x + 1, mark.0, mark.1);
        g.put(*y, list_x + 4, &fit(name, name_width), if mark.1 == Role::Muted { Role::Muted } else { Role::Text });
        let state_x = (list_x + list_w).saturating_sub(state.0.chars().count());
        g.put(*y, about_x, &fit(about.0, state_x.saturating_sub(about_x + 2)), about.1);
        g.put(*y, state_x, state.0, state.1);
        *y += 1;
        let end = g.put(*y, about_x, &fit(id, (list_x + list_w).saturating_sub(about_x)), Role::Muted);
        *y += 1;
        end
    };
    for at in window.clone() {
        match entries[at] {
            Entry::Installed(i) => {
                let r = &page.installed[i];
                let armed = page.armed.as_deref() == Some(r.id.as_str());
                let update = page.update_of(r);
                let state = if armed {
                    "x again removes it"
                } else if page.installing.as_deref() == Some(r.id.as_str()) {
                    "updating…"
                } else if r.managed {
                    if r.on { "managed" } else { "managed, off" }
                } else if page.tab == Tab::Updates {
                    "Enter updates"
                } else {
                    switch_state(r.running, r.on)
                };
                let mark = if r.on { ("●", Role::Good) } else { ("○", Role::Muted) };
                let about = match &r.broken {
                    Some(why) => (why.as_str(), Role::Warn),
                    None => (r.about.as_str(), Role::Muted),
                };
                let state_role = if (r.on == r.running || page.tab == Tab::Updates) && !armed { Role::Muted } else { Role::Warn };
                let end = row(&mut g, &mut y, at, mark, &r.name, about, (state, state_role), &format!("{} {}", r.id, r.version));
                if let Some(latest) = update {
                    g.put(y - 1, end + 2, &fit(&format!("· {latest} available -- u updates"), (list_x + list_w).saturating_sub(end + 2)), Role::Good);
                }
            }
            Entry::Available(i) => {
                let r = &available[i];
                let installing = page.installing.as_deref() == Some(r.id.as_str());
                let state = if installing { ("installing…", Role::Warn) } else { ("Enter installs", Role::Muted) };
                row(&mut g, &mut y, at, ("+", Role::Text), &r.name, (&r.about, Role::Muted), state, &format!("{} {} · {}", r.id, r.version, kind(Some(r.runtime))));
            }
            Entry::Bundle(i) => {
                let b = &bundles[i];
                let (have, on, all) = page.bundle_state(b);
                let mark = if on == all { ("●", Role::Good) } else if have > 0 { ("◐", Role::Text) } else { ("○", Role::Muted) };
                let armed = page.armed.as_deref() == Some(b.id.as_str());
                let installing = page.installing.as_deref().is_some_and(|id| id == b.id || b.members.iter().any(|m| m == id));
                let state = if armed {
                    ("x again removes it".to_string(), Role::Warn)
                } else if installing {
                    ("installing…".to_string(), Role::Warn)
                } else if on == all {
                    ("on".to_string(), Role::Muted)
                } else if have == 0 {
                    ("Enter installs".to_string(), Role::Muted)
                } else {
                    (format!("{on} of {all} on"), Role::Muted)
                };
                let id = format!("{}{} · {}", b.id, b.version.as_deref().map(|v| format!(" {v}")).unwrap_or_default(), b.members.join(", "));
                row(&mut g, &mut y, at, mark, &b.name, (&b.about, Role::Muted), (&state.0, state.1), &id);
            }
        }
    }
    if window.end < entries.len() {
        g.put(y, list_x + 2, &format!("↓ {} more", entries.len() - window.end), Role::Muted);
        y += 1;
    }
    if entries.is_empty() {
        let (text, role): (String, Role) = match (&page.registry, page.tab) {
            _ if !page.filter.is_empty() || page.category.is_some() => ("Nothing matches -- Esc clears the search, then the category.".into(), Role::Muted),
            (_, Tab::Installed) => ("None yet -- install one from Browse or Bundles (Tab), or with fenix --install-extension <file.fenixext>.".into(), Role::Muted),
            (Registry::Off(why), _) => (why.clone(), Role::Muted),
            (Registry::Looking, _) => ("Looking at the registry…".into(), Role::Muted),
            (Registry::Failed(why), _) => (format!("The registry couldn't be read: {why} -- r tries again"), Role::Warn),
            (Registry::Read(_), Tab::Updates) => ("Everything installed is the newest the registry has for this Fenix.".into(), Role::Muted),
            (Registry::Read(_), _) => ("Nothing more for this Fenix -- you have everything the registry has.".into(), Role::Muted),
        };
        for line in wrap(&text, list_w.saturating_sub(2)) {
            g.put(y, list_x + 1, &line, role);
            y += 1;
        }
    } else if page.tab == Tab::Bundles && !bundles.iter().any(|b| b.version.is_some()) {
        let why = match &page.registry {
            Registry::Failed(why) => format!("The registry couldn't be read: {why} -- r tries again. These are the bundles this Fenix knows; installing one takes the registry."),
            Registry::Looking => "Looking at the registry…".into(),
            Registry::Off(why) => why.clone(),
            Registry::Read(_) => "The registry has no bundles yet: these are the ones this Fenix knows, installed member by member.".into(),
        };
        y += 1;
        for line in wrap(&why, list_w.saturating_sub(2)) {
            g.put(y, list_x + 1, &line, if matches!(page.registry, Registry::Failed(_)) { Role::Warn } else { Role::Muted });
            y += 1;
        }
    }

    // The details of the row under the cursor, as far down as the list
    // goes.
    if let (true, Some(&entry)) = (pane_w > 0, entries.get(page.focus)) {
        let pane = pane_of(page, entry, &available, &bundles, pane_w);
        let bottom = (top + shown * 2).max(y);
        let mut py = top;
        let mut lines = pane.lines.iter();
        // The name and what it is, then the picture, then the rest.
        for (text, role) in lines.by_ref().take(4) {
            g.put(py, pane_x, text, *role);
            py += 1;
        }
        if let Some(path) = &pane.picture {
            let rows = 10.min(bottom.saturating_sub(py + 2));
            if rows >= 4 {
                g.image(py, pane_x, rows, pane_w, ImageKey::File(path.clone()));
                py += rows;
            }
        }
        for (text, role) in lines {
            if py + 1 >= bottom {
                g.put(py, pane_x, "… d shows it all", Role::Muted);
                break;
            }
            g.put(py, pane_x, text, *role);
            py += 1;
        }
    }

    let keys: Vec<(&str, &str)> = if page.filter.typing {
        vec![("Enter", "keep"), ("Esc", "clear")]
    } else {
        let enter = match (welcome, page.tab) {
            (true, _) | (_, Tab::Bundles) => "install / switch",
            (_, Tab::Installed) => "switch on / off",
            (_, Tab::Updates) => "update",
            (_, Tab::Browse) => "install",
        };
        let mut keys = vec![("Enter", enter)];
        if welcome {
            keys.push(("t", "tutorial"));
        } else {
            keys.extend([("Tab", "next tab"), ("d", "details"), ("/", "search"), ("h l", "categories")]);
        }
        if page.installed.iter().any(|r| page.update_of(r).is_some()) {
            keys.extend([("u", "update"), ("U", "update all")]);
        }
        if page.installed.iter().any(|r| !r.managed) {
            keys.push(("x x", "remove"));
        }
        keys.extend([("r", "registry again"), ("j k", "move"), ("q", "close")]);
        keys
    };
    g.keys(left, width, &keys);
    g.finish()
}

/// What an extension of data asks for: nothing.
const DATA_PERMISSIONS: &str = "None. It carries data (themes, snippets, templates, keys, language files) and runs no code.";

/// What an extension may do, as the details page says it.
fn runtime_permissions(runtime: Runtime, asked: &Permissions) -> String {
    match runtime {
        Runtime::None => DATA_PERMISSIONS.into(),
        Runtime::Wasm if asked.none() => "None. Its code runs sandboxed, with no files, network, programs or secrets.".into(),
        Runtime::Wasm => format!("Its code runs sandboxed, with only these: {}.", asked.lines().join("; ")),
        Runtime::Lua => "It runs Lua code, which this Fenix doesn't install yet.".into(),
        Runtime::Bundle => "None of its own: it's the extensions in it, each asking for what it asks.".into(),
    }
}

fn dependencies(deps: &BTreeMap<String, String>) -> String {
    if deps.is_empty() {
        return "none".into();
    }
    deps.iter().map(|(id, range)| format!("`{id}` {range}")).collect::<Vec<_>>().join(", ")
}

/// A README, changelog and screenshots, after what an extension is.
fn read_me_and_changes(out: &mut String, readme: Option<&str>, changelog: Option<&str>, screenshots: &[PathBuf]) {
    if !screenshots.is_empty() {
        *out += "\n---\n\n## Screenshots\n\n";
        for path in screenshots {
            let name = path.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            *out += &format!("![{name}]({})\n\n", path.display().to_string().replace(' ', "%20"));
        }
    }
    for (title, text) in [("Read me", readme), ("Changes", changelog)] {
        if let Some(text) = text.map(str::trim).filter(|t| !t.is_empty()) {
            *out += &format!("\n---\n\n## {title}\n\n{}\n", demote(text));
        }
    }
}

/// An installed extension's details page, as Markdown: what it is, the
/// bundles it's in (`bundles`), then its screenshots, README and
/// changelog when it carries them.
pub fn installed_details(e: &fenix_extensions::Installed, managed: bool, readme: Option<&str>, changelog: Option<&str>, screenshots: &[PathBuf], bundles: &[String]) -> String {
    let mut out = String::new();
    match &e.manifest {
        Ok(m) => {
            let info = &m.extension;
            out += &format!("# {}\n\n{}\n\n", info.name, info.description);
            out += &format!("- **Id:** `{}`\n- **Version:** {}\n", info.id, info.version);
            let how = if managed {
                "for everyone on this machine, by its administrator"
            } else if e.dependency {
                "for another that needs it, and removed with the last that does"
            } else {
                "by you"
            };
            out += &format!("- **Installed:** {how}{}\n", if e.enabled { "" } else { " (switched off)" });
            if info.runtime == Runtime::Bundle {
                out += &format!("- **For Fenix:** {}\n- **Members:** {}\n", m.engine.fenix, dependencies(&m.dependencies));
            } else {
                out += &format!("- **For Fenix:** {}\n- **Needs:** {}\n", m.engine.fenix, dependencies(&m.dependencies));
            }
            if !bundles.is_empty() {
                out += &format!("- **In bundles:** {}\n", bundles.join(", "));
            }
            if !info.categories.is_empty() {
                out += &format!("- **Kind:** {}\n", info.categories.join(", "));
            }
            if !info.authors.is_empty() {
                out += &format!("- **By:** {}\n", info.authors.join(", "));
            }
            if let Some(license) = &info.license {
                out += &format!("- **License:** {license}\n");
            }
            if let Some(repo) = &info.repository {
                out += &format!("- **Source:** <{repo}>\n");
            }
            out += &format!("- **Permissions:** {}\n", runtime_permissions(info.runtime, &m.permissions));
            let carries: Vec<&str> = fenix_extensions::DATA_DIRS.iter().copied().filter(|d| e.dir.join(d).is_dir()).collect();
            if !carries.is_empty() {
                out += &format!("- **Carries:** {}\n", carries.join(", "));
            }
        }
        Err(why) => out += &format!("# {}\n\nIts manifest can't be read: {why}\n\n- **Id:** `{}`\n- **Version:** {}\n", e.id, e.id, e.version),
    }
    out += &format!("- **Folder:** `{}`\n", e.dir.display());
    read_me_and_changes(&mut out, readme, changelog, screenshots);
    out
}

/// The registry's extension's details page, as Markdown, from its
/// listing -- and, once it's been fetched (`Release::preview`), the
/// README, changelog and screenshots its package carries, from `preview`.
pub fn available_details(l: &Listing, release: &semver::Version, preview: Option<&Path>, bundles: &[String]) -> String {
    let mut out = format!("# {}\n\n{}\n\n- **Id:** `{}`\n", l.name, l.description, l.id);
    match l.latest_for(release) {
        Some(r) => {
            let needs = if r.runtime == Runtime::Bundle { "Members" } else { "Needs" };
            out += &format!("- **Newest for this Fenix:** {} (for Fenix {})\n- **{needs}:** {}\n", r.version, r.fenix, dependencies(&r.dependencies));
            out += &format!("- **Download:** {} KB, checked against the signed index\n", r.size.div_ceil(1024));
            out += &format!("- **Permissions:** {}\n", runtime_permissions(r.runtime, &r.permissions));
        }
        None => out += "- **Newest for this Fenix:** none -- what the registry has is for another Fenix, or runs code\n",
    }
    if !bundles.is_empty() {
        out += &format!("- **In bundles:** {}\n", bundles.join(", "));
    }
    if !l.categories.is_empty() {
        out += &format!("- **Kind:** {}\n", l.categories.join(", "));
    }
    if let Some(repo) = &l.repository {
        out += &format!("- **Source:** <{repo}>\n");
    }
    let mut versions: Vec<&str> = l.versions.iter().map(|r| r.version.as_str()).collect();
    versions.reverse();
    out += &format!("- **Versions:** {}\n\nEnter on the Extensions page installs it.\n", versions.join(", "));
    if let Some(dir) = preview {
        let read = |name: &str| std::fs::read_to_string(dir.join(name)).ok();
        read_me_and_changes(&mut out, read("README.md").as_deref(), read("CHANGELOG.md").as_deref(), &screenshots(dir));
    }
    out
}

/// `text`'s headings one level down, under the page's own.
fn demote(text: &str) -> String {
    let mut fenced = false;
    text.lines()
        .map(|line| {
            if line.trim_start().starts_with("```") {
                fenced = !fenced;
            }
            if !fenced && line.starts_with('#') { format!("#{line}") } else { line.to_string() }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page() -> ExtensionsPage {
        ExtensionsPage::new()
    }

    fn installed(id: &str, on: bool, broken: Option<&str>) -> InstalledRow {
        InstalledRow { id: id.into(), name: id.into(), version: "1.0.0".into(), about: "Themes".into(), running: on, on, broken: broken.map(str::to_string), runtime: broken.is_none().then_some(Runtime::None), categories: vec!["themes".into()], ..Default::default() }
    }

    /// Enter switches an installed extension, `x` twice removes it, and
    /// anything between backs out.
    #[test]
    fn installed_extensions_switch_and_are_removed_with_a_second_x() {
        let mut p = page();
        p.installed = vec![installed("acme.dusk", true, None), installed("acme.broken", true, Some("extension.toml: no version"))];
        let text = layout(&p, 160).all_text();
        assert!(text.contains("1 Installed 2") && text.contains("acme.dusk 1.0.0") && text.contains("no version"), "{text}");
        assert_eq!(p.key(Key::Enter), Action::SwitchInstalled("acme.dusk".into(), false));
        assert!(matches!(p.key(Key::Char('x')), Action::Say(s) if s.starts_with("x again removes acme.dusk")));
        assert!(layout(&p, 160).all_text().contains("x again removes it"));
        assert_eq!(p.key(Key::Char('x')), Action::Uninstall("acme.dusk".into()));
        p.key(Key::Char('x'));
        p.key(Key::Char('j'));
        assert_eq!(p.armed, None, "moving away backs out");
        assert_eq!(p.focus, 1);
        assert!(matches!(p.key(Key::Enter), Action::Say(s) if s.contains("can't be read")));
        assert_eq!(p.key(Key::Char('q')), Action::Close);
    }

    fn release(version: &str, runtime: &str, extra: &str) -> String {
        format!(r#"{{"version": "{version}", "fenix": ">=1.0, <2", "runtime": "{runtime}", "url": "u", "sha256": "", "size": 1{extra}}}"#)
    }

    fn index(json_versions: &[(&str, &str)]) -> Index {
        let listings: Vec<String> = json_versions.iter().map(|(id, version)| format!(r#"{{"id": "{id}", "name": "{id}", "description": "Themes", "categories": ["themes"], "versions": [{}]}}"#, release(version, "none", ""))).collect();
        serde_json::from_str(&format!(r#"{{"format": 1, "extensions": [{}]}}"#, listings.join(","))).unwrap()
    }

    /// The registry's bundle `id` of `members`.
    fn with_bundle(mut index: Index, id: &str, name: &str, members: &[&str]) -> Index {
        let deps: Vec<String> = members.iter().map(|m| format!(r#""{m}": "^1""#)).collect();
        let bundle = members.iter().map(|m| format!("\"{m}\"")).collect::<Vec<_>>().join(", ");
        let extra = format!(r#", "dependencies": {{{}}}, "bundle": [{bundle}], "categories": ["tools"]"#, deps.join(", "));
        let json = format!(r#"{{"id": "{id}", "name": "{name}", "description": "Together", "versions": [{}]}}"#, release("1.0.0", "bundle", &extra));
        index.bundles.push(serde_json::from_str(&json).unwrap());
        index
    }

    /// Browse is what the registry has and you don't: Enter installs it;
    /// Updates is an installed one with a newer version there, one at a
    /// time -- or `U`, all of them.
    #[test]
    fn the_registry_offers_what_isnt_installed_and_newer_versions() {
        let mut p = page();
        p.tab = Tab::Browse;
        assert!(layout(&p, 160).all_text().contains("Looking at the registry"));
        p.installed = vec![installed("acme.dusk", true, None), installed("acme.calm", true, None)];
        p.registry = Registry::Read(index(&[("acme.dusk", "1.2.0"), ("acme.calm", "1.1.0"), ("acme.dawn", "1.0.0")]));
        assert_eq!(p.available().iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["acme.dawn"]);
        let text = layout(&p, 160).all_text();
        assert!(text.contains("3 Browse 1") && text.contains("acme.dawn 1.0.0 · data") && text.contains("2 Updates 2"), "{text}");
        assert_eq!(p.key(Key::Enter), Action::Install("acme.dawn".into()));
        p.installing = Some("acme.dawn".into());
        assert!(layout(&p, 160).all_text().contains("installing…"));
        assert!(matches!(p.key(Key::Enter), Action::Say(s) if s.contains("one at a time")));
        p.installing = None;

        p.key(Key::BackTab);
        assert_eq!(p.tab, Tab::Updates);
        let text = layout(&p, 160).all_text();
        assert!(text.contains("1.2.0 available -- u updates") && text.contains("Enter updates"), "{text}");
        assert_eq!(p.key(Key::Char('u')), Action::Install("acme.dusk".into()));
        assert_eq!(p.key(Key::Char('U')), Action::UpdateAll(vec!["acme.dusk".into(), "acme.calm".into()]));
        p.installed[0].version = "1.2.0".into();
        p.installed[1].version = "1.1.0".into();
        assert!(matches!(p.key(Key::Char('U')), Action::Say(s) if s.contains("the newest")));
        assert!(layout(&p, 160).all_text().contains("Everything installed is the newest"));
        assert_eq!(p.key(Key::Char('r')), Action::Refresh);
        p.registry = Registry::Failed("offline".into());
        p.key(Key::Char('3'));
        assert!(layout(&p, 160).all_text().contains("couldn't be read: offline -- r tries again"));
        assert!(p.available().is_empty());
    }

    /// The machine's extension is said to be its administrator's: no
    /// switch, update or removal from here.
    #[test]
    fn a_managed_extension_is_left_to_the_administrator() {
        let mut p = page();
        p.installed = vec![InstalledRow { managed: true, ..installed("acme.dusk", true, None) }];
        p.registry = Registry::Read(index(&[("acme.dusk", "1.2.0")]));
        for key in [Key::Enter, Key::Char('x'), Key::Char('u')] {
            assert!(matches!(p.key(key), Action::Say(s) if s.contains("managed on this machine")));
        }
        let text = layout(&p, 160).all_text();
        assert!(text.contains("managed") && !text.contains("u updates") && !text.contains("x x"), "{text}");
    }

    fn type_in(p: &mut ExtensionsPage, text: &str) {
        for c in text.chars() {
            p.key(Key::Char(c));
        }
    }

    /// `/` searches the tab by name, id and about; the cursor goes over
    /// only what matches, and Esc brings the rest back.
    #[test]
    fn search_keeps_to_what_matches() {
        let mut p = page();
        p.installed = vec![installed("acme.dusk", true, None), installed("acme.dawn", true, None)];
        assert_eq!(p.count(), 2);
        p.key(Key::Char('/'));
        assert!(p.typing());
        type_in(&mut p, "dawn");
        p.key(Key::Enter);
        assert!(!p.typing());
        assert_eq!(p.count(), 1);
        assert_eq!(p.key(Key::Enter), Action::SwitchInstalled("acme.dawn".into(), false), "the one match is under the cursor");
        let text = layout(&p, 160).all_text();
        assert!(text.contains("acme.dawn") && !text.contains("acme.dusk"), "{text}");
        p.key(Key::Char('/'));
        type_in(&mut p, "x");
        p.key(Key::Enter);
        assert!(layout(&p, 160).all_text().contains("Nothing matches"));
        assert_eq!(p.key(Key::Escape), Action::None, "Esc clears the search first");
        assert_eq!(p.count(), 2);
        assert_eq!(p.key(Key::Escape), Action::Close);
    }

    /// `runtime:`, `asks:` and `category:` narrow the search; the
    /// categories down the left keep the list to one.
    #[test]
    fn filters_keep_to_a_kind_a_permission_and_a_category() {
        let json = format!(
            r#"{{"format": 1, "extensions": [
                {{"id": "acme.dusk", "name": "Dusk", "categories": ["themes"], "versions": [{}]}},
                {{"id": "acme.jira", "name": "Jira", "categories": ["tools"], "versions": [{}]}},
                {{"id": "acme.ctr", "name": "Containers", "categories": ["tools"], "versions": [{}]}},
                {{"id": "acme.odd", "name": "Odd", "categories": ["weird"], "versions": [{}]}}]}}"#,
            release("1.0.0", "none", ""),
            release("1.0.0", "wasm", r#", "api": "1.0", "permissions": {"network": ["jira.example.com"], "secrets": ["jira.token"]}"#),
            release("1.0.0", "wasm", r#", "api": "1.0", "permissions": {"process": ["docker"]}"#),
            release("1.0.0", "none", ""),
        );
        let mut p = page();
        p.tab = Tab::Browse;
        p.registry = Registry::Read(serde_json::from_str(&json).unwrap());
        let shown = |p: &ExtensionsPage| p.entries().iter().map(|e| match e { Entry::Available(i) => p.available()[*i].id.clone(), _ => String::new() }).collect::<Vec<_>>();
        assert_eq!(shown(&p).len(), 4);
        for (search, want) in [("runtime:code", vec!["acme.jira", "acme.ctr"]), ("runtime:data", vec!["acme.dusk", "acme.odd"]), ("asks:network", vec!["acme.jira"]), ("asks:process runtime:code", vec!["acme.ctr"]), ("asks:nothing", vec!["acme.dusk", "acme.odd"]), ("category:tools jira", vec!["acme.jira"]), ("category:too jira", vec!["acme.jira"]), ("category:other", vec!["acme.odd"])] {
            p.filter.text = search.into();
            assert_eq!(shown(&p), want, "{search}");
        }
        p.filter.text.clear();
        // The sidebar: h to it, j down to a category.
        let categories = p.categories();
        assert_eq!(categories[0], (None, "All".to_string(), 4));
        assert!(categories.iter().any(|c| c == &(Some("tools".to_string()), "Tools".to_string(), 2)) && categories.last().unwrap().2 == 1, "{categories:?}");
        p.key(Key::Char('h'));
        assert!(p.sidebar);
        let at_tools = categories.iter().position(|c| c.0.as_deref() == Some("tools")).unwrap();
        for _ in 0..at_tools {
            p.key(Key::Char('j'));
        }
        assert_eq!(p.category.as_deref(), Some("tools"));
        assert_eq!(shown(&p), ["acme.jira", "acme.ctr"]);
        let text = layout(&p, 160).all_text();
        assert!(text.contains("Tools") && text.contains("Containers") && !text.contains("Dusk"), "{text}");
        p.key(Key::Char('l'));
        assert!(!p.sidebar);
        assert_eq!(p.key(Key::Escape), Action::None, "Esc lets go of the category");
        assert_eq!(p.category, None);
    }

    /// Hundreds of rows: only those that fit are laid out, the cursor's
    /// among them.
    #[test]
    fn a_long_list_lays_out_only_what_fits() {
        let ids: Vec<String> = (0..500).map(|i| format!("acme.theme-{i:03}")).collect();
        let pairs: Vec<(&str, &str)> = ids.iter().map(|id| (id.as_str(), "1.0.0")).collect();
        let mut p = page();
        p.tab = Tab::Browse;
        p.rows = 31;
        p.registry = Registry::Read(index(&pairs));
        assert_eq!(p.shown(), 10);
        let text = layout(&p, 160).all_text();
        assert!(text.contains("acme.theme-009") && !text.contains("acme.theme-010") && text.contains("↓ 490 more"), "{text}");
        assert!(text.lines().count() < 40);
        p.key(Key::Char('G'));
        let text = layout(&p, 160).all_text();
        assert!(text.contains("acme.theme-499") && text.contains("↑ 490 more") && !text.contains("acme.theme-489 "), "{text}");
        let start = std::time::Instant::now();
        p.key(Key::Char('/'));
        type_in(&mut p, "theme-25");
        assert_eq!(p.count(), 10);
        assert!(start.elapsed() < std::time::Duration::from_secs(1), "a search is instant");
    }

    #[test]
    fn d_opens_the_details_of_any_row() {
        let mut p = page();
        p.installed = vec![installed("acme.dusk", true, None)];
        p.registry = Registry::Read(index(&[("acme.dawn", "1.0.0")]));
        let mut seen = vec![p.key(Key::Char('d'))];
        p.key(Key::Char('3'));
        seen.push(p.key(Key::Char('d')));
        assert_eq!(seen, [Action::Details(Target::Installed("acme.dusk".into())), Action::Details(Target::Available("acme.dawn".into()))]);
    }

    /// The pane beside the list: what it is, asks for, needs, which
    /// bundles it's in, its versions, README and changes -- and its first
    /// screenshot.
    #[test]
    fn the_details_pane_says_what_the_selected_one_is() {
        let mut p = page();
        p.installed = vec![InstalledRow {
            readme: Some("# Dusk\n\nTwo late themes.\n\n## Use\n\nPick one.".into()),
            changelog: Some("# Changelog\n\n## 1.0.0\n\n- First\n\n## 0.9.0\n\n- Older".into()),
            screenshots: vec![PathBuf::from("/x/screenshots/dusk.png")],
            ..installed("acme.dusk", true, None)
        }];
        p.registry = Registry::Read(with_bundle(index(&[("acme.dusk", "1.0.0")]), "acme.evening", "Evening", &["acme.dusk"]));
        p.rows = 80;
        let page = layout(&p, 200);
        let text = page.all_text();
        for want in ["Kind: data · Themes", "PERMISSIONS", "runs no code", "Needs: none", "In bundles: Evening", "Versions: 1.0.0", "READ ME", "Two late themes.", "Use", "CHANGES", "1.0.0", "- First"] {
            assert!(text.contains(want), "{want:?} in {text}");
        }
        assert!(!text.contains("Older"), "the newest change alone");
        assert_eq!(page.images.iter().map(|i| i.key.clone()).collect::<Vec<_>>(), [ImageKey::File(PathBuf::from("/x/screenshots/dusk.png"))]);
        assert!(!layout(&p, 100).all_text().contains("PERMISSIONS"), "no room for it: d opens it");
        p.rows = 30;
        assert!(layout(&p, 200).all_text().contains("… d shows it all"), "cut where the list ends");
    }

    #[test]
    fn details_say_what_an_extension_is_and_asks_for() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = fenix_extensions::Manifest::parse("[extension]\nid = \"acme.dusk\"\nname = \"Dusk\"\nversion = \"1.2.0\"\ndescription = \"Late themes\"\nruntime = \"none\"\nlicense = \"MIT\"\n\n[engine]\nfenix = \">=1.0, <2\"\n\n[dependencies]\n\"acme.base\" = \"^1\"\n").unwrap();
        std::fs::create_dir_all(dir.path().join("themes")).unwrap();
        let e = fenix_extensions::Installed { id: "acme.dusk".into(), version: "1.2.0".into(), enabled: true, dependency: false, dir: dir.path().to_path_buf(), manifest: Ok(manifest) };
        let shot = dir.path().join("screenshots").join("late night.png");
        let md = installed_details(&e, true, Some("# Dusk\n\nTwo themes.\n\n```\n# not a heading\n```"), Some("## 1.2.0\n- calmer"), std::slice::from_ref(&shot), &["Evening".into()]);
        for want in ["# Dusk\n\nLate themes", "**Version:** 1.2.0", "by its administrator", "`acme.base` ^1", "**In bundles:** Evening", "**License:** MIT", "runs no code", "**Carries:** themes", "## Screenshots", "late%20night.png)", "## Read me\n\n## Dusk", "# not a heading", "## Changes\n\n### 1.2.0"] {
            assert!(md.contains(want), "{want:?} in {md}");
        }
        let listing: Listing = index(&[("acme.dawn", "1.0.0")]).extensions.remove(0);
        let md = available_details(&listing, &semver::Version::new(1, 0, 0), None, &[]);
        assert!(md.contains("**Newest for this Fenix:** 1.0.0") && md.contains("**Download:** 1 KB") && md.contains("runs no code") && !md.contains("Read me"), "{md}");
        // Its package's README and screenshots, once fetched.
        let preview = dir.path().join("preview");
        std::fs::create_dir_all(preview.join("screenshots")).unwrap();
        std::fs::write(preview.join("README.md"), "# Dawn\n\nEarly.").unwrap();
        std::fs::write(preview.join("screenshots/a.png"), "").unwrap();
        let md = available_details(&listing, &semver::Version::new(1, 0, 0), Some(&preview), &[]);
        assert!(md.contains("## Read me\n\n## Dawn") && md.contains("## Screenshots") && md.contains("a.png)"), "{md}");
    }

    /// What a code extension may do is on its details before it's
    /// installed: from the index, as its manifest asks.
    #[test]
    fn a_code_extensions_permissions_are_shown_before_install() {
        let json = r#"{"format": 1, "extensions": [{"id": "fenix.containers", "name": "Docker", "versions": [
            {"version": "2.0.0", "fenix": ">=1.0, <2", "runtime": "wasm", "api": "1.0", "url": "u", "sha256": "", "size": 1,
             "permissions": {"process": ["docker", "podman"]}}]}]}"#;
        let mut index: Index = serde_json::from_str(json).unwrap();
        let md = available_details(&index.extensions[0], &semver::Version::new(1, 0, 0), None, &[]);
        assert!(md.contains("sandboxed, with only these: Run the programs: docker, podman."), "{md}");
        index.extensions[0].versions[0].permissions = Default::default();
        let md = available_details(&index.extensions[0], &semver::Version::new(1, 0, 0), None, &[]);
        assert!(md.contains("no files, network, programs or secrets"), "{md}");
    }

    #[test]
    fn with_nothing_installed_the_page_says_how() {
        let text = layout(&page(), 160).all_text();
        assert!(text.contains("None yet -- install one from Browse or Bundles"), "{text}");
    }

    #[test]
    fn safe_mode_says_nothing_runs_rather_than_changes_waiting() {
        let mut p = page();
        p.installed = vec![InstalledRow { running: false, ..installed("acme.dusk", true, None) }];
        p.safe_mode = true;
        let text = layout(&p, 160).all_text();
        assert!(text.contains("Safe mode: none of these runs") && !text.contains("restart Fenix to apply"), "{text}");
    }

    #[test]
    fn a_switch_waiting_for_a_restart_says_so() {
        let mut p = page();
        p.installed = vec![installed("acme.dusk", true, None), installed("acme.dawn", false, None)];
        let text = layout(&p, 120).all_text();
        assert!(text.contains("acme.dusk") && text.contains("acme.dawn") && text.contains("1 of 2 on"), "{text}");
        assert!(!text.contains("restart Fenix"));
        p.installed[0].on = false;
        let text = layout(&p, 120).all_text();
        assert!(text.contains("off after a restart") && text.contains("1 change -- restart Fenix to apply"), "{text}");
    }

    /// The welcome is the bundles this Fenix knows until the registry is
    /// read; Enter installs a bundle's members, on unless they're all on
    /// already.
    #[test]
    fn the_welcomes_bundles_are_fenixs_own_until_the_registry_is_read() {
        let mut p = ExtensionsPage::welcome();
        let text = layout(&p, 120).all_text();
        assert!(text.contains("Welcome to Fenix") && text.contains("Looking at the registry") && !text.contains("Browse"), "{text}");
        assert_eq!(p.bundles().iter().map(|b| b.name.as_str()).collect::<Vec<_>>(), ["Personal", "Mission", "Review", "Jira", "Lab", "Reading"]);
        p.focus = 1;
        let mission: Vec<String> = ["fenix.mib", "fenix.ccsds", "fenix.tcl"].map(String::from).to_vec();
        assert_eq!(p.key(Key::Enter), Action::SwitchBundle { on: true, members: mission.clone(), install: None });
        for id in ["fenix.mib", "fenix.ccsds"] {
            p.installed.push(InstalledRow { version: "2.0.0".into(), ..installed(id, true, None) });
        }
        assert!(layout(&p, 120).all_text().contains("2 of 3 on"));
        assert_eq!(p.key(Key::Enter), Action::SwitchBundle { on: true, members: mission.clone(), install: None }, "not all on without Tcl");
        p.installed.push(installed("fenix.tcl", true, None));
        assert_eq!(p.key(Key::Enter), Action::SwitchBundle { on: false, members: mission, install: None }, "all on: off again");
        p.registry = Registry::Failed("offline".into());
        assert!(layout(&p, 120).all_text().contains("offline -- r tries again. These are the bundles this Fenix knows"));
    }

    /// The welcome offers the tutorial; the Extensions page's `t` is
    /// nothing.
    #[test]
    fn the_welcome_offers_the_tutorial() {
        let mut p = ExtensionsPage::welcome();
        let text = layout(&p, 120).all_text();
        assert!(text.contains("New to Fenix's keys? t starts the tutorial") && text.contains("t tutorial"), "{text}");
        assert_eq!(p.key(Key::Char('t')), Action::Tutorial);
        let mut e = ExtensionsPage::new();
        assert!(!layout(&e, 160).all_text().contains("tutorial"));
        assert_eq!(e.key(Key::Char('t')), Action::None);
    }

    /// Read, the registry's bundles take the place of Fenix's own:
    /// installed as one, and removed as one once they are.
    #[test]
    fn the_registrys_bundles_install_and_go_as_one() {
        let mut p = ExtensionsPage::welcome();
        p.registry = Registry::Read(with_bundle(with_bundle(index(&[("acme.notes", "1.0.0")]), "acme.zeta", "Zeta", &["acme.notes"]), "fenix.lab-bundle", "Lab", &["acme.notes", "acme.board"]));
        assert_eq!(p.bundles().iter().map(|b| b.name.as_str()).collect::<Vec<_>>(), ["Lab", "Zeta"], "Fenix's order, then by name; the fallback gone");
        assert_eq!(p.key(Key::Enter), Action::SwitchBundle { on: true, members: vec!["acme.notes".into(), "acme.board".into()], install: Some("fenix.lab-bundle".into()) });
        assert!(matches!(p.key(Key::Char('x')), Action::Say(s) if s.contains("isn't installed as a bundle")));
        p.installed = vec![
            InstalledRow { runtime: Some(Runtime::Bundle), dependencies: [("acme.notes".to_string(), "^1".to_string()), ("acme.board".to_string(), "^1".to_string())].into(), ..installed("fenix.lab-bundle", true, None) },
            InstalledRow { dependency: true, ..installed("acme.notes", true, None) },
            installed("acme.board", false, None),
        ];
        assert_eq!(p.bundles().len(), 2, "the installed one is the registry's");
        assert_eq!(p.key(Key::Enter), Action::SwitchBundle { on: true, members: vec!["acme.notes".into(), "acme.board".into()], install: None }, "installed: switched");
        p.key(Key::Char('x'));
        assert_eq!(p.key(Key::Char('x')), Action::Uninstall("fenix.lab-bundle".into()));
        assert_eq!(p.bundles_of("acme.notes"), ["Lab", "Zeta"]);
        // On the plain page: the bundle isn't one of what's installed.
        let mut p2 = page();
        p2.installed = p.installed.clone();
        p2.registry = p.registry.clone();
        assert_eq!(p2.count(), 2);
        let text = layout(&p2, 160).all_text();
        assert!(text.contains("1 of 2 on") && !text.contains("fenix.lab-bundle"), "{text}");
        p2.key(Key::Char('4'));
        assert_eq!(p2.count(), 2);
        let text = layout(&p2, 160).all_text();
        assert!(text.contains("fenix.lab-bundle 1.0.0 · acme.notes, acme.board") && text.contains("MEMBERS"), "{text}");
    }

    #[test]
    fn a_readme_and_changes_are_cut_to_their_start() {
        assert_eq!(excerpt("# Name\n\nFirst.\n\n## How\n\nSo.", false, 8), ["First.", "", "How", "", "So."]);
        assert_eq!(excerpt("# Changelog\n\n## 2.0.0\n\n- New\n\n## 1.0.0\n\n- Old", true, 8), ["2.0.0", "", "- New"]);
        assert_eq!(excerpt("## 2.0.0\n- New\n## 1.0.0", true, 8), ["2.0.0", "- New"], "no title");
    }
}
