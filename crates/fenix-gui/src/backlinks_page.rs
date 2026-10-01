//! `SPC n b`: a note's links, beside it -- what links here (with the
//! line it's on), where its name appears without a link, its outline,
//! and what it links to. `Tab` changes list, `Enter` goes there, `l`
//! turns an unlinked mention into a `[[link]]`.

use crate::page::{fit, Grid, Key, Page, Role};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Here,
    Outline,
    Out,
}

/// A row: another entry's line, a heading, or a link out.
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    /// The entry it's in (for `Here`) or points to (for `Out`); empty for
    /// a heading.
    pub id: String,
    pub name: String,
    pub kind: &'static str,
    pub line: usize,
    pub context: String,
    /// For `Out`: whether it goes anywhere yet.
    pub exists: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    None,
    Close,
    /// Open entry `id` at `line` (in the pane the note is in).
    Open { id: String, line: usize },
    /// Go to `line` of the note itself.
    Line(usize),
    /// Follow the note's link on `line` to `name`.
    Follow { name: String, line: usize },
    /// Make the mention on `line` of entry `id` a link.
    Link { id: String, line: usize },
}

pub struct BacklinksPage {
    pub id: String,
    pub name: String,
    pub here: Vec<Item>,
    pub mentions: Vec<Item>,
    pub outline: Vec<Item>,
    pub out: Vec<Item>,
    pub tab: Tab,
    cursor: usize,
}

impl BacklinksPage {
    pub fn new(id: String, name: String) -> Self {
        BacklinksPage { id, name, here: Vec::new(), mentions: Vec::new(), outline: Vec::new(), out: Vec::new(), tab: Tab::Here, cursor: 0 }
    }

    /// The rows of the current tab: for `Here`, the links then the
    /// mentions.
    fn rows(&self) -> Vec<(&Item, bool)> {
        match self.tab {
            Tab::Here => self.here.iter().map(|i| (i, false)).chain(self.mentions.iter().map(|i| (i, true))).collect(),
            Tab::Outline => self.outline.iter().map(|i| (i, false)).collect(),
            Tab::Out => self.out.iter().map(|i| (i, false)).collect(),
        }
    }

    pub fn clamp(&mut self) {
        self.cursor = self.cursor.min(self.rows().len().saturating_sub(1));
    }

    pub fn key(&mut self, key: Key) -> Action {
        let n = self.rows().len();
        match key {
            Key::Down | Key::Char('j') => self.cursor = (self.cursor + 1).min(n.saturating_sub(1)),
            Key::Up | Key::Char('k') => self.cursor = self.cursor.saturating_sub(1),
            Key::Tab | Key::BackTab => {
                self.tab = match (self.tab, key) {
                    (Tab::Here, Key::Tab) | (Tab::Out, Key::BackTab) => Tab::Outline,
                    (Tab::Outline, Key::Tab) | (Tab::Here, Key::BackTab) => Tab::Out,
                    _ => Tab::Here,
                };
                self.cursor = 0;
            }
            Key::Char('q') | Key::Escape => return Action::Close,
            Key::Enter | Key::Char('o') => {
                if let Some((item, _)) = self.rows().get(self.cursor) {
                    return match self.tab {
                        Tab::Here => Action::Open { id: item.id.clone(), line: item.line },
                        Tab::Outline => Action::Line(item.line),
                        Tab::Out => Action::Follow { name: item.name.clone(), line: item.line },
                    };
                }
            }
            Key::Char('l') => {
                if let Some((item, true)) = self.rows().get(self.cursor) {
                    return Action::Link { id: item.id.clone(), line: item.line };
                }
            }
            _ => {}
        }
        Action::None
    }
}

pub fn layout(page: &BacklinksPage, cols: usize) -> Page {
    let width = cols.saturating_sub(2).max(20);
    let left = 1;
    let mut g = Grid::new();
    g.put(0, left, &fit(&page.name, width), Role::Title);
    let mut x = left;
    for (tab, label) in [(Tab::Here, format!("Links here {}", page.here.len())), (Tab::Outline, format!("Outline {}", page.outline.len())), (Tab::Out, format!("Out {}", page.out.len()))] {
        let on = tab == page.tab;
        let end = g.put(1, x, &label, if on { Role::Title } else { Role::Muted });
        if on {
            g.panel(1, x..end);
        }
        x = end + 2;
    }
    g.rule(2, left..left + width);
    let mut y = 3;
    let rows = page.rows();
    if rows.is_empty() {
        let text = match page.tab {
            Tab::Here => "Nothing links here yet. [[name]] in another note does.",
            Tab::Outline => "No headings.",
            Tab::Out => "No links out.",
        };
        g.put(y, left, &fit(text, width), Role::Muted);
    }
    let mut mentions_heading = false;
    for (i, (item, mention)) in rows.iter().enumerate() {
        if *mention && !mentions_heading {
            y += 1;
            g.heading(y, left, width, &format!("Unlinked mentions {}", page.mentions.len()));
            y += 1;
            mentions_heading = true;
        }
        let sel = i == page.cursor;
        match page.tab {
            Tab::Outline => {
                let indent = item.kind.len();
                g.put(y, left + indent, &fit(&item.name, width.saturating_sub(indent)), if sel { Role::Title } else { Role::Text });
                if sel {
                    g.focus(y, left..left + width);
                }
                y += 1;
            }
            _ => {
                let x = g.put(y, left, item.kind, Role::Muted) + 1;
                let role = if !item.exists && page.tab == Tab::Out { Role::Warn } else if sel { Role::Title } else { Role::Text };
                g.put(y, x, &fit(&item.name, width.saturating_sub(x - left)), role);
                if sel {
                    g.focus(y, left..left + width);
                }
                y += 1;
                if !item.context.is_empty() {
                    g.put(y, left + 2, &fit(&item.context, width.saturating_sub(2)), Role::Muted);
                    if sel {
                        g.panel(y, left..left + width);
                    }
                    y += 1;
                }
            }
        }
    }
    let keys: Vec<(&str, &str)> = match page.tab {
        Tab::Here if !page.mentions.is_empty() => vec![("Enter", "go"), ("l", "link it"), ("Tab", "list"), ("q", "close")],
        _ => vec![("Enter", "go"), ("Tab", "list"), ("q", "close")],
    };
    g.keys(left, width, &keys);
    g.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str, name: &str, line: usize, context: &str) -> Item {
        Item { id: id.into(), name: name.into(), kind: "NOTE", line, context: context.into(), exists: true }
    }

    #[test]
    fn links_here_mentions_and_tabs() {
        let mut p = BacklinksPage::new("a".into(), "Uplink".into());
        p.here = vec![item("b", "Log", 3, "see [[Uplink]]")];
        p.mentions = vec![item("c", "Other", 7, "the uplink notes")];
        p.outline = vec![Item { id: String::new(), name: "Decision".into(), kind: "", line: 4, context: String::new(), exists: true }];
        let t = layout(&p, 40).text;
        assert!(t.contains("Links here 1") && t.contains("see [[Uplink]]") && t.contains("UNLINKED MENTIONS 1"), "{t}");
        assert_eq!(p.key(Key::Enter), Action::Open { id: "b".into(), line: 3 });
        assert_eq!(p.key(Key::Char('l')), Action::None);
        p.key(Key::Char('j'));
        assert_eq!(p.key(Key::Char('l')), Action::Link { id: "c".into(), line: 7 });
        p.key(Key::Tab);
        assert_eq!(p.tab, Tab::Outline);
        assert_eq!(p.key(Key::Enter), Action::Line(4));
    }
}
