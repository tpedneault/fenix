//! A diagram, drawn: beside its source as you type (`SPC m p` in a
//! `.mmd`, and whenever a notebook diagram opens) or on its own as a
//! viewer (`v`). `+`/`-`/`=`/`0` zoom, `h`/`j`/`k`/`l` pan, `Tab` walks
//! the nodes (the one you're on is outlined, and the source cursor's
//! node is too), `Enter` goes to a node's line, `/` finds a node by its
//! label, `T` shows it in another theme without changing it, `t` sets
//! its theme, `e` exports, `i` edits.

use crate::page::{fit, with_caret, Grid, ImageKey, Key, Page, Role};

/// What the page knows of the drawing.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Drawn {
    /// Its size in the SVG's units.
    pub size: (f32, f32),
    pub nodes: Vec<fenix_diagram::NodeBox>,
    pub theme: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    None,
    Close,
    /// Go to this source line (and to the source).
    Line(usize),
    /// Pick a theme to write into the source.
    PickTheme,
    /// Show it in the next theme (not written).
    CycleTheme,
    Export,
    /// Back to the source.
    Edit,
}

pub struct DiagramPage {
    /// The buffer drawn; a viewer of a file not open has none.
    pub source: Option<fenix_buffers::BufferId>,
    pub title: String,
    pub viewer: bool,
    /// The picture to show: the last one that drew.
    pub key: Option<ImageKey>,
    /// What's being drawn now (may not have arrived yet).
    pub want: Option<ImageKey>,
    pub drawn: Drawn,
    /// Why the source doesn't draw, and on which line.
    pub error: Option<(usize, String)>,
    pub zoom: f32,
    pub center: (f32, f32),
    pub selected: Option<usize>,
    /// The node the source cursor is in.
    pub cursor_node: Option<usize>,
    /// Shown in this theme instead (not written).
    pub theme_over: Option<String>,
    find: Option<(String, usize)>,
    last_find: Option<String>,
    pub note: Option<(String, bool)>,
    /// The source's edit count it was last drawn from.
    pub seen: Option<u64>,
}

impl DiagramPage {
    pub fn new(source: Option<fenix_buffers::BufferId>, title: String, viewer: bool) -> Self {
        DiagramPage { source, title, viewer, key: None, want: None, drawn: Drawn::default(), error: None, zoom: 1.0, center: (0.5, 0.5), selected: None, cursor_node: None, theme_over: None, find: None, last_find: None, note: None, seen: None }
    }

    pub fn typing(&self) -> bool {
        self.find.is_some()
    }

    pub fn paste(&mut self, text: &str) {
        if let Some((t, c)) = &mut self.find {
            crate::page::insert_at(t, c, text);
        }
    }

    /// Centres the view on node `i`.
    fn go_to(&mut self, i: usize) {
        self.selected = Some(i);
        if let Some(n) = self.drawn.nodes.get(i) {
            let (w, h) = self.drawn.size;
            if w > 0.0 && h > 0.0 && self.zoom > 1.0 {
                self.center = ((n.x + n.w / 2.0) / w, (n.y + n.h / 2.0) / h);
            }
        }
    }

    /// The nodes in reading order: top to bottom, left to right.
    fn order(&self) -> Vec<usize> {
        let mut idx: Vec<usize> = (0..self.drawn.nodes.len()).collect();
        idx.sort_by(|&a, &b| {
            let (na, nb) = (&self.drawn.nodes[a], &self.drawn.nodes[b]);
            (na.y.round() as i64, na.x.round() as i64).cmp(&(nb.y.round() as i64, nb.x.round() as i64))
        });
        idx
    }

    fn step(&mut self, forward: bool) {
        let order = self.order();
        if order.is_empty() {
            return;
        }
        let at = self.selected.and_then(|s| order.iter().position(|&i| i == s));
        let next = match (at, forward) {
            (Some(p), true) => order[(p + 1) % order.len()],
            (Some(p), false) => order[(p + order.len() - 1) % order.len()],
            (None, true) => order[0],
            (None, false) => order[order.len() - 1],
        };
        self.go_to(next);
    }

    fn find_next(&mut self, text: &str, from: usize) -> bool {
        let text = text.to_lowercase();
        let order = self.order();
        let n = order.len();
        for k in 0..n {
            let i = order[(from + k) % n];
            let node = &self.drawn.nodes[i];
            if node.label.to_lowercase().contains(&text) || node.id.to_lowercase().contains(&text) {
                self.go_to(i);
                return true;
            }
        }
        false
    }

    pub fn key(&mut self, key: Key) -> Action {
        if let Some((mut text, mut caret)) = self.find.take() {
            match key {
                Key::Escape => {}
                Key::Enter => {
                    if !self.find_next(&text, 0) {
                        self.note = Some((format!("no node says {text}"), true));
                    }
                    self.find = None;
                    self.last_find = Some(text);
                }
                Key::Space => {
                    crate::page::insert_at(&mut text, &mut caret, " ");
                    self.find = Some((text, caret));
                }
                k => {
                    crate::page::edit_line(&mut text, &mut caret, k);
                    self.find = Some((text, caret));
                }
            }
            return Action::None;
        }
        self.note = None;
        let pan = 0.12 / self.zoom;
        match key {
            Key::Char('+') => self.zoom = (self.zoom * 1.25).min(8.0),
            Key::Char('-') => {
                self.zoom = (self.zoom / 1.25).max(1.0);
                if self.zoom <= 1.0 {
                    self.center = (0.5, 0.5);
                }
            }
            Key::Char('=') | Key::Char('0') => {
                self.zoom = 1.0;
                self.center = (0.5, 0.5);
            }
            Key::Char('h') | Key::Left => self.center.0 = (self.center.0 - pan).max(0.0),
            Key::Char('l') | Key::Right => self.center.0 = (self.center.0 + pan).min(1.0),
            Key::Char('k') | Key::Up => self.center.1 = (self.center.1 - pan).max(0.0),
            Key::Char('j') | Key::Down => self.center.1 = (self.center.1 + pan).min(1.0),
            Key::Tab => self.step(true),
            Key::BackTab => self.step(false),
            Key::Char('/') => self.find = Some((String::new(), 0)),
            Key::Char('n') | Key::Char('N') => {
                if let Some(text) = self.last_find.clone() {
                    let order = self.order();
                    let at = self.selected.and_then(|s| order.iter().position(|&i| i == s)).map(|p| p + 1).unwrap_or(0);
                    self.find_next(&text, at);
                }
            }
            Key::Enter => {
                if let Some(line) = self.selected.and_then(|i| self.drawn.nodes.get(i)).and_then(|n| n.line) {
                    return Action::Line(line);
                }
                return Action::Edit;
            }
            Key::Char('i') => return Action::Edit,
            Key::Char('T') => return Action::CycleTheme,
            Key::Char('t') => return Action::PickTheme,
            Key::Char('e') => return Action::Export,
            Key::Char('q') | Key::Escape => {
                if self.selected.is_some() {
                    self.selected = None;
                } else {
                    return Action::Close;
                }
            }
            _ => {}
        }
        Action::None
    }
}

pub fn layout(page: &DiagramPage, cols: usize, rows: usize) -> Page {
    let mut g = Grid::new();
    let left = 1;
    let width = cols.saturating_sub(2).max(20);
    let x = g.put(0, left, &fit(&page.title, width / 2), Role::Title) + 2;
    let theme = page.theme_over.clone().unwrap_or_else(|| page.drawn.theme.clone());
    let mut facts = Vec::new();
    if !theme.is_empty() {
        facts.push(if page.theme_over.is_some() { format!("{theme} (shown)") } else { theme });
    }
    if !page.drawn.nodes.is_empty() {
        facts.push(format!("{} nodes", page.drawn.nodes.len()));
    }
    facts.push(if page.zoom <= 1.0 { "fit".into() } else { format!("{:.0}%", page.zoom * 100.0) });
    g.put(0, x, &fit(&facts.join(" · "), width.saturating_sub(x - left)), Role::Muted);
    let mut y = 1;
    if let Some((line, message)) = &page.error {
        g.put(y, left, &fit(&format!("line {}: {message}", line + 1), width), Role::Bad);
        y += 1;
        if page.key.is_some() {
            g.put(y, left, &fit("showing the last drawing that worked", width), Role::Muted);
            y += 1;
        }
    }
    if let Some((text, bad)) = &page.note {
        g.put(y, left, &fit(text, width), if *bad { Role::Bad } else { Role::Good });
        y += 1;
    }
    let node = page.selected.or(page.cursor_node).and_then(|i| page.drawn.nodes.get(i));
    match (&page.find, node) {
        (Some((text, caret)), _) => {
            let end = g.put(y, left, &format!("/ {}", with_caret(text, *caret, 40)), Role::Title);
            g.panel(y, left..end.max(left + 30));
            y += 1;
        }
        (None, Some(n)) => {
            let label = if n.label.is_empty() { n.id.clone() } else { format!("{} ({})", n.label, n.id) };
            let at = n.line.map(|l| format!(" · line {}", l + 1)).unwrap_or_default();
            g.put(y, left, &fit(&format!("{label}{at}"), width), Role::Accent);
            y += 1;
        }
        _ => {}
    }
    y += 1;
    // The picture fills what's left, above the keys.
    let room = rows.saturating_sub(y + 3).max(4);
    match &page.key {
        Some(key) => {
            g.image(y, left, room, width, key.clone());
            if let Some(img) = g.images.last_mut() {
                img.view = Some((page.zoom, page.center.0, page.center.1));
            }
            y += room;
        }
        None if page.error.is_none() => {
            g.put(y, left, "drawing...", Role::Muted);
            y += room;
        }
        None => y += 1,
    }
    let _ = y;
    let keys: &[(&str, &str)] = if page.viewer {
        &[("+ - 0", "zoom"), ("hjkl", "pan"), ("Tab", "nodes"), ("/", "find"), ("Enter", "its line"), ("T", "try a theme"), ("e", "export"), ("i", "edit"), ("q", "close")]
    } else {
        &[("+ - 0", "zoom"), ("hjkl", "pan"), ("Tab", "nodes"), ("/", "find"), ("Enter", "its line"), ("t", "theme"), ("T", "try one"), ("e", "export"), ("q", "close")]
    };
    g.keys(left, width, keys);
    g.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: &str, label: &str, x: f32, y: f32, line: usize) -> fenix_diagram::NodeBox {
        fenix_diagram::NodeBox { id: id.into(), label: label.into(), x, y, w: 50.0, h: 20.0, line: Some(line) }
    }

    fn page() -> DiagramPage {
        let mut p = DiagramPage::new(None, "TC flow".into(), true);
        p.key = Some(ImageKey::Diagram(1));
        p.drawn = Drawn { size: (300.0, 400.0), nodes: vec![node("b", "Validate", 100.0, 100.0, 2), node("a", "Request", 100.0, 10.0, 1)], theme: "fenix".into() };
        p
    }

    #[test]
    fn it_fills_the_pane_and_says_what_it_shows() {
        let p = page();
        let g = layout(&p, 80, 40);
        assert!(g.text.starts_with(" TC flow  fenix · 2 nodes · fit"), "{}", g.text);
        let img = &g.images[0];
        assert_eq!((img.line, img.rows, img.view), (2, 35, Some((1.0, 0.5, 0.5))));
    }

    #[test]
    fn nodes_are_walked_found_and_jumped_to() {
        let mut p = page();
        p.key(Key::Tab);
        assert_eq!(p.selected, Some(1), "top first");
        assert_eq!(p.key(Key::Enter), Action::Line(1));
        p.key(Key::Char('/'));
        for c in "valid".chars() {
            p.key(Key::Char(c));
        }
        p.key(Key::Enter);
        assert_eq!(p.selected, Some(0));
        assert!(layout(&p, 80, 40).text.contains("Validate (b) · line 3"));
        p.key(Key::Char('+'));
        p.key(Key::Char('+'));
        assert!((p.zoom - 1.5625).abs() < 1e-4);
        p.key(Key::Char('0'));
        assert_eq!((p.zoom, p.center), (1.0, (0.5, 0.5)));
        p.key(Key::Char('q'));
        assert_eq!(p.key(Key::Char('q')), Action::Close);
    }
}
