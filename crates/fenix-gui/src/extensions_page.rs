//! `SPC h x`, the Extensions page: the built-in extensions, each switched
//! on or off (`extensions.disabled`). A switch takes effect when Fenix
//! next starts -- what's on decides which commands, keys and settings
//! there are -- so a row says both what's running now and what will be.
//!
//! The welcome (`SPC h w`, and a fresh install's first screen) is the
//! same page with packs on top: a few extensions that go together,
//! switched at once.
//!
//! Below the built-in ones: the installed ones with a newer version in
//! the registry (`u` updates one), the rest you installed, then what the
//! registry has that you haven't (Enter installs one). `/` searches them
//! all, and `d` opens one's details page in the reading view.

use fenix_extensions::registry::Index;

use crate::page::{fit, frame, Filter, Grid, Key, Page, Role};

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
    /// The machine's, which only its administrator changes.
    pub managed: bool,
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
    /// Show this one's details page.
    Details(Target),
    /// Say this on the page.
    Say(String),
}

/// What `d` shows the details of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    BuiltIn(&'static str),
    Installed(String),
    Available(String),
}

/// One line the cursor goes over, in the order the page shows them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Entry {
    Pack(usize),
    BuiltIn(usize),
    /// An installed one: with a newer version in the registry, it's
    /// under Updates.
    Installed(usize),
    Available(usize),
}

pub struct ExtensionsPage {
    pub rows: Vec<Row>,
    /// The welcome's packs, above the rows; none on the plain page.
    pub packs: Vec<Pack>,
    /// The cursor, over what's shown (`entries`).
    pub focus: usize,
    /// This run is safe mode: nothing's running whatever the rows say.
    pub safe_mode: bool,
    /// The extensions you installed, below the built-in ones.
    pub installed: Vec<InstalledRow>,
    /// The installed extension `x` was pressed on once; a second `x`
    /// removes it.
    pub armed: Option<String>,
    /// The registry, below what's installed (not on the welcome).
    pub registry: Registry,
    /// The Fenix release, for which of the registry's versions fit.
    pub release: semver::Version,
    /// The extension being installed, one at a time.
    pub installing: Option<String>,
    /// `/`: only what has every word typed, in its name, id or about.
    pub filter: Filter,
}

impl ExtensionsPage {
    pub fn new(rows: Vec<Row>) -> Self {
        ExtensionsPage {
            rows,
            packs: Vec::new(),
            focus: 0,
            safe_mode: false,
            installed: Vec::new(),
            armed: None,
            registry: Registry::default(),
            release: semver::Version::new(2, 0, 0),
            installing: None,
            filter: Filter::default(),
        }
    }

    /// The welcome: the page with `PACKS` on top -- the ones with
    /// anything in this build.
    pub fn welcome(rows: Vec<Row>) -> Self {
        let packs = PACKS.iter().filter(|pack| pack.ids.iter().any(|id| rows.iter().any(|r| r.id == *id))).copied().collect();
        let mut page = ExtensionsPage::new(rows);
        page.packs = packs;
        page
    }

    fn welcome_page(&self) -> bool {
        !self.packs.is_empty()
    }

    /// The extensions of `pack` this build has.
    fn built<'a>(&'a self, pack: &'a Pack) -> impl Iterator<Item = &'static str> + 'a {
        pack.ids.iter().copied().filter(|id| self.rows.iter().any(|r| r.id == *id))
    }

    /// Whether a switch is waiting for a restart.
    pub fn pending(&self) -> usize {
        self.rows.iter().filter(|r| r.on != r.running).count() + self.installed.iter().filter(|r| r.on != r.running).count()
    }

    /// What the registry has that isn't installed, for this Fenix; none
    /// on the welcome.
    pub fn available(&self) -> Vec<AvailableRow> {
        let Registry::Read(index) = &self.registry else { return Vec::new() };
        if self.welcome_page() {
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
        if row.managed {
            return None;
        }
        let have = [(row.id.clone(), row.version.clone())];
        fenix_extensions::registry::updates(index, &have, &self.release).into_iter().next().map(|u| u.latest)
    }

    /// What's shown, in order: the packs, the built-in ones, the updates,
    /// the rest installed, then what's available -- each kept to the
    /// filter.
    fn entries(&self) -> Vec<Entry> {
        let shown = |name: &str, id: &str, about: &str| self.filter.matches(&format!("{name} {id} {about}"));
        let mut out: Vec<Entry> = (0..self.packs.len()).map(Entry::Pack).collect();
        out.extend(self.rows.iter().enumerate().filter(|(_, r)| shown(r.name, r.id, r.about)).map(|(i, _)| Entry::BuiltIn(i)));
        if !self.welcome_page() {
            let installed: Vec<usize> = (0..self.installed.len()).filter(|&i| shown(&self.installed[i].name, &self.installed[i].id, &self.installed[i].about)).collect();
            let (updates, rest): (Vec<usize>, Vec<usize>) = installed.into_iter().partition(|&i| self.update_of(&self.installed[i]).is_some());
            out.extend(updates.into_iter().chain(rest).map(Entry::Installed));
            out.extend(self.available().iter().enumerate().filter(|(_, r)| shown(&r.name, &r.id, &r.about)).map(|(i, _)| Entry::Available(i)));
        }
        out
    }

    /// Every row the cursor goes over.
    pub fn count(&self) -> usize {
        self.entries().len()
    }

    fn at_focus(&self) -> Option<Entry> {
        self.entries().get(self.focus).copied()
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

    /// Whether the filter has the keyboard.
    pub fn typing(&self) -> bool {
        self.filter.typing
    }

    pub fn paste(&mut self, text: &str) {
        let text: String = text.chars().filter(|c| !c.is_control()).collect();
        self.filter.paste(&text);
        self.focus = 0;
    }

    pub fn key(&mut self, key: Key) -> Action {
        if self.filter.typing {
            if self.filter.key(key) {
                self.focus = 0;
            }
            return Action::None;
        }
        let count = self.count();
        let entry = self.at_focus();
        let installed = match entry {
            Some(Entry::Installed(i)) => Some(&self.installed[i]),
            _ => None,
        };
        // Removing takes a second `x` on the same row; anything else
        // backs out.
        let armed = self.armed.take();
        // The machine's is its administrator's to change.
        if let Some(row) = installed.filter(|r| r.managed) {
            if matches!(key, Key::Enter | Key::Char('x') | Key::Char('u')) {
                return Action::Say(format!("{} is managed on this machine -- its administrator switches, updates and removes it", row.name));
            }
        }
        match key {
            Key::Char('x') => match installed {
                Some(row) if armed.as_deref() == Some(row.id.as_str()) => Action::Uninstall(row.id.clone()),
                Some(row) => {
                    let said = format!("x again removes {} -- anything else keeps it", row.name);
                    self.armed = Some(row.id.clone());
                    Action::Say(said)
                }
                None => Action::None,
            },
            Key::Char('q') => Action::Close,
            Key::Escape if !self.filter.is_empty() => {
                self.filter.clear();
                self.focus = 0;
                Action::None
            }
            Key::Escape => Action::Close,
            Key::Char('/') if !self.welcome_page() => {
                self.filter.start();
                self.focus = 0;
                Action::None
            }
            Key::Char('r') if !self.welcome_page() => Action::Refresh,
            Key::Char('d') => match entry {
                Some(Entry::BuiltIn(i)) => Action::Details(Target::BuiltIn(self.rows[i].id)),
                Some(Entry::Installed(i)) => Action::Details(Target::Installed(self.installed[i].id.clone())),
                Some(Entry::Available(i)) => self.available().get(i).map_or(Action::None, |r| Action::Details(Target::Available(r.id.clone()))),
                _ => Action::None,
            },
            Key::Char('u') => match installed {
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
            Key::Enter => match entry {
                Some(Entry::Pack(i)) => {
                    let pack = self.packs[i];
                    Action::Switch(self.built(&pack).collect(), !self.pack_on(&pack))
                }
                Some(Entry::BuiltIn(i)) => Action::Switch(vec![self.rows[i].id], !self.rows[i].on),
                Some(Entry::Installed(i)) => {
                    let row = &self.installed[i];
                    match &row.broken {
                        None => Action::SwitchInstalled(row.id.clone(), !row.on),
                        Some(_) => Action::Say(format!("{} can't be read -- x x removes it", row.name)),
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

pub fn layout(page: &ExtensionsPage, cols: usize) -> Page {
    let (left, width) = frame(cols, 100);
    let mut g = Grid::new();
    let welcome = page.welcome_page();
    let end = g.put(1, left, if welcome { "Welcome to Fenix" } else { "Extensions" }, Role::Title);
    g.put(1, end + 2, if welcome { "· pick what you'll use" } else { "· built in, installed and in the registry" }, Role::Muted);
    let on = page.rows.iter().filter(|r| r.on).count();
    let status = format!("{on} of {} on", page.rows.len());
    g.put(1, (left + width).saturating_sub(status.chars().count()), &status, Role::Muted);
    g.rule(2, left..left + width);
    let pending = page.pending();
    let note = match (welcome, pending) {
        _ if page.safe_mode => "Safe mode: none of these runs this time. What's below is what your settings switch on when Fenix next starts without --disable-extensions.".to_string(),
        (true, 0) => "The core is yours already: editing, files, projects, Git, language servers, debugging, the terminal. Add what else you'll use.".to_string(),
        (false, 0) => "Enter switches one on or off, or installs it. What's off has no commands, keys, settings or pages.".to_string(),
        (_, n) => format!("{n} change{} -- restart Fenix to apply", if n == 1 { "" } else { "s" }),
    };
    g.put(3, left + 2, &fit(&note, width.saturating_sub(4)), if pending == 0 || page.safe_mode { Role::Muted } else { Role::Warn });
    let mut y = 5;
    if !welcome {
        let (text, role) = page.filter.line("search: words in a name, id or description", width.min(70));
        let end = g.put(y, left, &text, role);
        if page.filter.typing {
            g.panels.push((y, left..end.max(left + 40)));
        }
        y += 2;
    }
    let name_width = 16;
    let about_x = left + 5 + name_width;
    let entries = page.entries();
    let available = page.available();
    // A row: its mark, name, about and state, then its id (and what
    // follows it) below.
    let row = |g: &mut Grid, y: &mut usize, at: usize, mark: (&str, Role), name: &str, about: (&str, Role), state: (&str, Role), id: &str| {
        if at == page.focus {
            g.focus(*y, left..left + width);
        }
        g.put(*y, left + 2, mark.0, mark.1);
        g.put(*y, left + 4, &fit(name, name_width), if mark.1 == Role::Muted { Role::Muted } else { Role::Text });
        let state_x = (left + width).saturating_sub(state.0.chars().count());
        g.put(*y, about_x, &fit(about.0, state_x.saturating_sub(about_x + 2)), about.1);
        g.put(*y, state_x, state.0, state.1);
        *y += 1;
        let end = g.put(*y, about_x, id, Role::Muted);
        *y += 1;
        end
    };
    // Section by section, each with its entries -- or, empty, why.
    let mut drawn = false;
    for section in 0..5 {
        let these: Vec<(usize, Entry)> = entries
            .iter()
            .copied()
            .enumerate()
            .filter(|(_, e)| match (section, e) {
                (0, Entry::Pack(_)) | (1, Entry::BuiltIn(_)) | (4, Entry::Available(_)) => true,
                (2, Entry::Installed(i)) => page.update_of(&page.installed[*i]).is_some(),
                (3, Entry::Installed(i)) => page.update_of(&page.installed[*i]).is_none(),
                _ => false,
            })
            .collect();
        let empty: Option<(String, Role)> = match section {
            _ if welcome || !these.is_empty() => None,
            3 if page.installed.is_empty() => Some(("None yet -- install one from Available below, or with fenix --install-extension <file.fenixext>.".into(), Role::Muted)),
            4 => match &page.registry {
                Registry::Off(why) => Some((why.clone(), Role::Muted)),
                Registry::Looking => Some(("Looking at the registry…".into(), Role::Muted)),
                Registry::Failed(why) => Some((format!("The registry couldn't be read: {why} -- r tries again"), Role::Warn)),
                Registry::Read(_) if available.is_empty() => Some(("Nothing more for this Fenix -- you have everything the registry has.".into(), Role::Muted)),
                Registry::Read(_) => None,
            },
            _ => None,
        };
        if these.is_empty() && empty.is_none() {
            continue;
        }
        if drawn {
            y += 1;
        }
        drawn = true;
        let title = match section {
            0 => "Packs",
            1 if welcome => "One by one",
            1 => "Built in",
            2 => "Updates",
            3 => "Installed",
            _ => "Available",
        };
        g.heading(y, left, width, title);
        y += 1;
        if let Some((text, role)) = empty {
            g.put(y, left + 2, &fit(&text, width.saturating_sub(4)), role);
            y += 1;
        }
        for (at, entry) in these {
            match entry {
                Entry::Pack(i) => {
                    if at == page.focus {
                        g.focus(y, left..left + width);
                    }
                    let pack = &page.packs[i];
                    let on = page.pack_on(pack);
                    g.put(y, left + 2, if on { "●" } else { "○" }, if on { Role::Good } else { Role::Muted });
                    g.put(y, left + 4, &fit(pack.name, name_width), if on { Role::Text } else { Role::Muted });
                    g.put(y, about_x, &fit(pack.about, (left + width).saturating_sub(about_x)), Role::Muted);
                    y += 1;
                }
                Entry::BuiltIn(i) => {
                    let r = &page.rows[i];
                    let mark = if r.on { ("●", Role::Good) } else { ("○", Role::Muted) };
                    let state = (switch_state(r.running, r.on), if r.on == r.running { Role::Muted } else { Role::Warn });
                    row(&mut g, &mut y, at, mark, r.name, (r.about, Role::Muted), state, r.id);
                }
                Entry::Installed(i) => {
                    let r = &page.installed[i];
                    let armed = page.armed.as_deref() == Some(r.id.as_str());
                    let state = if armed {
                        "x again removes it"
                    } else if page.installing.as_deref() == Some(r.id.as_str()) {
                        "updating…"
                    } else if r.managed {
                        if r.on { "managed" } else { "managed, off" }
                    } else {
                        switch_state(r.running, r.on)
                    };
                    let mark = if r.on { ("●", Role::Good) } else { ("○", Role::Muted) };
                    let about = match &r.broken {
                        Some(why) => (why.as_str(), Role::Warn),
                        None => (r.about.as_str(), Role::Muted),
                    };
                    let state_role = if r.on == r.running && !armed { Role::Muted } else { Role::Warn };
                    let end = row(&mut g, &mut y, at, mark, &r.name, about, (state, state_role), &format!("{} {}", r.id, r.version));
                    if let Some(latest) = page.update_of(r) {
                        g.put(y - 1, end + 2, &format!("· {latest} available -- u updates"), Role::Good);
                    }
                }
                Entry::Available(i) => {
                    let r = &available[i];
                    let installing = page.installing.as_deref() == Some(r.id.as_str());
                    let state = if installing { ("installing…", Role::Warn) } else { ("Enter installs", Role::Muted) };
                    row(&mut g, &mut y, at, ("+", Role::Text), &r.name, (&r.about, Role::Muted), state, &format!("{} {}", r.id, r.version));
                }
            }
        }
    }
    if !page.filter.is_empty() && entries.is_empty() {
        g.put(y + 1, left + 2, "Nothing matches -- Esc clears the search.", Role::Muted);
    }
    let keys: Vec<(&str, &str)> = if page.filter.typing {
        vec![("Enter", "keep"), ("Esc", "clear")]
    } else {
        let mut keys = vec![("Enter", if welcome { "switch on / off" } else { "switch / install" })];
        if !welcome {
            keys.extend([("d", "details"), ("/", "search")]);
        }
        if page.installed.iter().any(|r| page.update_of(r).is_some()) {
            keys.push(("u", "update"));
        }
        if page.installed.iter().any(|r| !r.managed) {
            keys.push(("x x", "remove installed"));
        }
        if !welcome {
            keys.push(("r", "registry again"));
        }
        keys.extend([("j k", "move"), ("q", "close")]);
        keys
    };
    g.keys(left, width, &keys);
    g.finish()
}

/// What an extension of data asks for: nothing. Code extensions, when
/// they come, list their permissions here.
const DATA_PERMISSIONS: &str = "None. It carries data (themes, snippets, templates, keys, language files) and runs no code.";

fn runtime_permissions(runtime: fenix_extensions::Runtime) -> &'static str {
    match runtime {
        fenix_extensions::Runtime::None => DATA_PERMISSIONS,
        _ => "It runs code, which this Fenix doesn't install yet.",
    }
}

fn dependencies(deps: &std::collections::BTreeMap<String, String>) -> String {
    if deps.is_empty() {
        return "none".into();
    }
    deps.iter().map(|(id, range)| format!("`{id}` {range}")).collect::<Vec<_>>().join(", ")
}

/// A built-in extension's details page, as Markdown.
pub fn builtin_details(row: &Row) -> String {
    format!(
        "# {}\n\n{}\n\n- **Id:** `{}`\n- **Built in:** it comes with this Fenix and updates with it.\n- **Now:** {}\n\nEnter on the Extensions page switches it; what's off has no commands, keys, settings or pages.\n",
        row.name,
        row.about,
        row.id,
        switch_state(row.running, row.on)
    )
}

/// An installed extension's details page, as Markdown: what it is, then
/// its README and changelog when it carries them.
pub fn installed_details(e: &fenix_extensions::Installed, managed: bool, readme: Option<&str>, changelog: Option<&str>) -> String {
    let mut out = String::new();
    match &e.manifest {
        Ok(m) => {
            let info = &m.extension;
            out += &format!("# {}\n\n{}\n\n", info.name, info.description);
            out += &format!("- **Id:** `{}`\n- **Version:** {}\n", info.id, info.version);
            out += &format!("- **Installed:** {}{}\n", if managed { "for everyone on this machine, by its administrator" } else { "by you" }, if e.enabled { "" } else { " (switched off)" });
            out += &format!("- **For Fenix:** {}\n- **Needs:** {}\n", m.engine.fenix, dependencies(&m.dependencies));
            if !info.authors.is_empty() {
                out += &format!("- **By:** {}\n", info.authors.join(", "));
            }
            if let Some(license) = &info.license {
                out += &format!("- **License:** {license}\n");
            }
            if let Some(repo) = &info.repository {
                out += &format!("- **Source:** <{repo}>\n");
            }
            out += &format!("- **Permissions:** {}\n", runtime_permissions(info.runtime));
            let carries: Vec<&str> = fenix_extensions::DATA_DIRS.iter().copied().filter(|d| e.dir.join(d).is_dir()).collect();
            if !carries.is_empty() {
                out += &format!("- **Carries:** {}\n", carries.join(", "));
            }
        }
        Err(why) => out += &format!("# {}\n\nIts manifest can't be read: {why}\n\n- **Id:** `{}`\n- **Version:** {}\n", e.id, e.id, e.version),
    }
    out += &format!("- **Folder:** `{}`\n", e.dir.display());
    for (title, text) in [("Read me", readme), ("Changes", changelog)] {
        if let Some(text) = text.map(str::trim).filter(|t| !t.is_empty()) {
            out += &format!("\n---\n\n## {title}\n\n{}\n", demote(text));
        }
    }
    out
}

/// The registry's extension's details page, as Markdown, from its
/// listing: the README comes with the package.
pub fn available_details(l: &fenix_extensions::registry::Listing, release: &semver::Version) -> String {
    let mut out = format!("# {}\n\n{}\n\n- **Id:** `{}`\n", l.name, l.description, l.id);
    match l.latest_for(release) {
        Some(r) => {
            out += &format!("- **Newest for this Fenix:** {} (for Fenix {})\n- **Needs:** {}\n", r.version, r.fenix, dependencies(&r.dependencies));
            out += &format!("- **Download:** {} KB, checked against the signed index\n", r.size.div_ceil(1024));
            out += &format!("- **Permissions:** {}\n", runtime_permissions(r.runtime));
        }
        None => out += "- **Newest for this Fenix:** none -- what the registry has is for another Fenix, or runs code\n",
    }
    if !l.categories.is_empty() {
        out += &format!("- **Kind:** {}\n", l.categories.join(", "));
    }
    if let Some(repo) = &l.repository {
        out += &format!("- **Source:** <{repo}>\n");
    }
    let mut versions: Vec<&str> = l.versions.iter().map(|r| r.version.as_str()).collect();
    versions.reverse();
    out += &format!("- **Versions:** {}\n\nEnter on the Extensions page installs it, and its read-me with it.\n", versions.join(", "));
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
        InstalledRow { id: id.into(), name: id.into(), version: "1.0.0".into(), about: "Themes".into(), running: on, on, broken: broken.map(str::to_string), managed: false }
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

    /// The machine's extension is said to be its administrator's: no
    /// switch, update or removal from here.
    #[test]
    fn a_managed_extension_is_left_to_the_administrator() {
        let mut p = page();
        p.installed = vec![InstalledRow { managed: true, ..installed("acme.dusk", true, None) }];
        p.registry = Registry::Read(index(&[("acme.dusk", "1.2.0")]));
        p.focus = 2;
        for key in [Key::Enter, Key::Char('x'), Key::Char('u')] {
            assert!(matches!(p.key(key), Action::Say(s) if s.contains("managed on this machine")));
        }
        let text = layout(&p, 160).all_text();
        assert!(text.contains("managed") && !text.contains("u updates") && !text.contains("remove installed"), "{text}");
    }

    fn type_in(p: &mut ExtensionsPage, text: &str) {
        for c in text.chars() {
            p.key(Key::Char(c));
        }
    }

    /// `/` searches every section by name, id and about; the cursor goes
    /// over only what matches, and Esc brings the rest back.
    #[test]
    fn search_keeps_to_what_matches() {
        let mut p = page();
        p.installed = vec![installed("acme.dusk", true, None)];
        p.registry = Registry::Read(index(&[("acme.dawn", "1.0.0")]));
        assert_eq!(p.count(), 4);
        p.key(Key::Char('/'));
        assert!(p.typing());
        type_in(&mut p, "dawn");
        p.key(Key::Enter);
        assert!(!p.typing());
        assert_eq!(p.count(), 1);
        assert_eq!(p.key(Key::Enter), Action::Install("acme.dawn".into()), "the one match is under the cursor");
        let text = layout(&p, 160).all_text();
        assert!(text.contains("acme.dawn") && !text.contains("Jira") && !text.contains("acme.dusk"), "{text}");
        p.key(Key::Char('/'));
        type_in(&mut p, "x");
        p.key(Key::Enter);
        assert!(layout(&p, 160).all_text().contains("Nothing matches"));
        assert_eq!(p.key(Key::Escape), Action::None, "Esc clears the search first");
        assert_eq!(p.count(), 4);
        assert_eq!(p.key(Key::Escape), Action::Close);
    }

    /// An installed one with a newer version is under Updates, above the
    /// rest installed.
    #[test]
    fn updates_come_before_the_rest_installed() {
        let mut p = page();
        p.installed = vec![installed("acme.calm", true, None), installed("acme.dusk", true, None)];
        p.registry = Registry::Read(index(&[("acme.dusk", "1.1.0")]));
        let text = layout(&p, 160).all_text();
        let (updates, installed, dusk, calm) = (text.find("UPDATES").unwrap(), text.find("INSTALLED").unwrap(), text.find("acme.dusk 1.0.0").unwrap(), text.find("acme.calm 1.0.0").unwrap());
        assert!(updates < dusk && dusk < installed && installed < calm, "{text}");
        p.focus = 2;
        assert_eq!(p.key(Key::Char('u')), Action::Install("acme.dusk".into()), "the first after the built-in ones is the update");
    }

    #[test]
    fn d_opens_the_details_of_any_row() {
        let mut p = page();
        p.installed = vec![installed("acme.dusk", true, None)];
        p.registry = Registry::Read(index(&[("acme.dawn", "1.0.0")]));
        let mut seen = Vec::new();
        for _ in 0..4 {
            seen.push(p.key(Key::Char('d')));
            p.key(Key::Char('j'));
        }
        assert_eq!(
            seen,
            [
                Action::Details(Target::BuiltIn("fenix.jira")),
                Action::Details(Target::BuiltIn("fenix.pdf")),
                Action::Details(Target::Installed("acme.dusk".into())),
                Action::Details(Target::Available("acme.dawn".into()))
            ]
        );
    }

    #[test]
    fn details_say_what_an_extension_is_and_asks_for() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = fenix_extensions::Manifest::parse("[extension]\nid = \"acme.dusk\"\nname = \"Dusk\"\nversion = \"1.2.0\"\ndescription = \"Late themes\"\nruntime = \"none\"\nlicense = \"MIT\"\n\n[engine]\nfenix = \">=2.0, <3\"\n\n[dependencies]\n\"acme.base\" = \"^1\"\n").unwrap();
        std::fs::create_dir_all(dir.path().join("themes")).unwrap();
        let e = fenix_extensions::Installed { id: "acme.dusk".into(), version: "1.2.0".into(), enabled: true, dir: dir.path().to_path_buf(), manifest: Ok(manifest) };
        let md = installed_details(&e, true, Some("# Dusk\n\nTwo themes.\n\n```\n# not a heading\n```"), Some("## 1.2.0\n- calmer"));
        for want in ["# Dusk\n\nLate themes", "**Version:** 1.2.0", "by its administrator", "`acme.base` ^1", "**License:** MIT", "runs no code", "**Carries:** themes", "## Read me\n\n## Dusk", "# not a heading", "## Changes\n\n### 1.2.0"] {
            assert!(md.contains(want), "{want:?} in {md}");
        }
        let listing: fenix_extensions::registry::Listing = index(&[("acme.dawn", "1.0.0")]).extensions.remove(0);
        let md = available_details(&listing, &semver::Version::new(2, 0, 0));
        assert!(md.contains("**Newest for this Fenix:** 1.0.0") && md.contains("**Download:** 1 KB") && md.contains("runs no code"), "{md}");
        let md = builtin_details(&page().rows[0]);
        assert!(md.contains("# Jira") && md.contains("`fenix.jira`") && md.contains("**Now:** on"), "{md}");
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
