//! `SPC h x`, the Extensions page: the built-in extensions, each switched
//! on or off (`extensions.disabled`). A switch takes effect when Fenix
//! next starts -- what's on decides which commands, keys and settings
//! there are -- so a row says both what's running now and what will be.
//!
//! The welcome (`SPC h w`, and a fresh install's first screen) is the
//! same page with packs on top: a few extensions that go together,
//! switched at once.

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    None,
    Close,
    /// Switch these extensions on (`true`) or off from the next start.
    Switch(Vec<&'static str>, bool),
}

pub struct ExtensionsPage {
    pub rows: Vec<Row>,
    /// The welcome's packs, above the rows; none on the plain page.
    pub packs: Vec<Pack>,
    /// The cursor, over the packs then the rows.
    pub focus: usize,
    /// This run is safe mode: nothing's running whatever the rows say.
    pub safe_mode: bool,
}

impl ExtensionsPage {
    pub fn new(rows: Vec<Row>) -> Self {
        ExtensionsPage { rows, packs: Vec::new(), focus: 0, safe_mode: false }
    }

    /// The welcome: the page with `PACKS` on top -- the ones with
    /// anything in this build.
    pub fn welcome(rows: Vec<Row>) -> Self {
        let packs = PACKS.iter().filter(|pack| pack.ids.iter().any(|id| rows.iter().any(|r| r.id == *id))).copied().collect();
        ExtensionsPage { rows, packs, focus: 0, safe_mode: false }
    }

    /// The extensions of `pack` this build has.
    fn built<'a>(&'a self, pack: &'a Pack) -> impl Iterator<Item = &'static str> + 'a {
        pack.ids.iter().copied().filter(|id| self.rows.iter().any(|r| r.id == *id))
    }

    /// Whether a switch is waiting for a restart.
    pub fn pending(&self) -> usize {
        self.rows.iter().filter(|r| r.on != r.running).count()
    }

    /// Whether every extension in `pack` is on from the next start.
    pub fn pack_on(&self, pack: &Pack) -> bool {
        self.built(pack).all(|id| self.rows.iter().any(|r| r.id == id && r.on))
    }

    pub fn key(&mut self, key: Key) -> Action {
        let count = self.packs.len() + self.rows.len();
        match key {
            Key::Char('q') | Key::Escape => Action::Close,
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
                    None => Action::None,
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
    g.keys(left, width, &[("Enter", "switch on / off"), ("j k", "move"), ("q", "close")]);
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
