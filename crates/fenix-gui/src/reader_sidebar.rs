//! The PDF reader's sidebar: the outline, the last search's matches and
//! the document's marks, as lists down the reader pane's left edge. This
//! is its pure half -- the rows a list shows, folding the outline, where
//! the reader is in it, and the sidebar's keys. `app/reader.rs` owns the
//! state and draws it.

use std::collections::HashSet;

use fenix_keymap::{KeyCode, KeyPress, Mods, NamedKey};
use fenix_pdf::outline::OutlineEntry;

/// Columns the sidebar is wide, text only.
pub const COLS: usize = 32;

/// A pane narrower than this many columns has the sidebar over its pages
/// rather than beside them.
pub const BESIDE_MIN_COLS: usize = 70;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Outline,
    Matches,
    Marks,
}

impl Tab {
    fn next(self) -> Tab {
        match self {
            Tab::Outline => Tab::Matches,
            Tab::Matches => Tab::Marks,
            Tab::Marks => Tab::Outline,
        }
    }
}

/// Where a row takes the document.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Target {
    Page(u32),
    /// A match of the last search, by its index.
    Match(usize),
    Mark(char),
}

/// One row of a list.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    /// Indent, fold marker and title, cut to fit.
    pub text: String,
    /// The page number shown at the right, if any.
    pub right: String,
    pub target: Option<Target>,
    /// The section being read, or the current match.
    pub here: bool,
}

/// One match, as the Matches list shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct MatchRow {
    pub page: u32,
    pub context: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Sidebar {
    pub tab: Option<Tab>,
    /// Whether it has the keyboard (else the pages do).
    pub focused: bool,
    pub cursor: usize,
    pub scroll: usize,
    /// Outline entries folded, by index.
    pub folded: HashSet<usize>,
    prefix: Option<char>,
}

/// What a key in the sidebar asked for.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SideKey {
    /// Taken: it moved, folded or switched lists.
    Taken,
    /// `Enter` on a row: go there.
    Go(Target),
    /// `o`, `q`, `Esc`: close it.
    Close,
    /// Not the sidebar's: the leader, `:`, `Ctrl-w`.
    Pass,
}

impl Sidebar {
    pub fn open(tab: Tab) -> Self {
        Sidebar { tab: Some(tab), focused: true, ..Default::default() }
    }

    pub fn tab(&self) -> Tab {
        self.tab.unwrap_or(Tab::Outline)
    }

    /// Keeps the cursor on a row and in sight, `height` rows showing.
    pub fn settle(&mut self, rows: usize, height: usize) {
        self.cursor = self.cursor.min(rows.saturating_sub(1));
        let height = height.max(1);
        if self.cursor < self.scroll {
            self.scroll = self.cursor;
        } else if self.cursor >= self.scroll + height {
            self.scroll = self.cursor + 1 - height;
        }
    }

    /// Puts the cursor on the row marked `here`, if there is one.
    pub fn to_here(&mut self, rows: &[Row]) {
        if let Some(at) = rows.iter().position(|r| r.here) {
            self.cursor = at;
        }
    }

    pub fn key(&mut self, key: KeyPress, rows: &[Row], outline: &[OutlineEntry]) -> SideKey {
        let last = rows.len().saturating_sub(1);
        if let Some(prefix) = self.prefix.take() {
            match (prefix, key.code) {
                ('g', KeyCode::Char('g')) => self.cursor = 0,
                ('z', KeyCode::Char('a')) => {
                    if let Some(i) = self.outline_index(outline) {
                        if has_children(outline, i) && !self.folded.remove(&i) {
                            self.folded.insert(i);
                        }
                    }
                }
                ('z', KeyCode::Char('M')) => self.folded = (0..outline.len()).filter(|&i| has_children(outline, i)).collect(),
                ('z', KeyCode::Char('R')) => self.folded.clear(),
                _ => {}
            }
            return SideKey::Taken;
        }
        if key.mods != Mods::default() {
            return SideKey::Pass;
        }
        match key.code {
            KeyCode::Char('j') | KeyCode::Named(NamedKey::Down) => self.cursor = (self.cursor + 1).min(last),
            KeyCode::Char('k') | KeyCode::Named(NamedKey::Up) => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Char('G') => self.cursor = last,
            KeyCode::Char(c @ ('g' | 'z')) => self.prefix = Some(c),
            KeyCode::Named(NamedKey::Tab) => {
                self.tab = Some(self.tab().next());
                self.cursor = 0;
                self.scroll = 0;
            }
            KeyCode::Char('h') | KeyCode::Named(NamedKey::Left) => {
                if let Some(i) = self.outline_index(outline) {
                    if has_children(outline, i) && !self.folded.contains(&i) {
                        self.folded.insert(i);
                    } else if let Some(parent) = parent_of(outline, i) {
                        if let Some(row) = visible_outline(outline, &self.folded).iter().position(|&v| v == parent) {
                            self.cursor = row;
                        }
                    }
                }
            }
            KeyCode::Char('l') | KeyCode::Named(NamedKey::Right) => {
                if let Some(i) = self.outline_index(outline) {
                    self.folded.remove(&i);
                }
            }
            KeyCode::Named(NamedKey::Enter) => {
                if let Some(target) = rows.get(self.cursor).and_then(|r| r.target) {
                    return SideKey::Go(target);
                }
            }
            KeyCode::Char('o') | KeyCode::Char('q') | KeyCode::Named(NamedKey::Escape) => return SideKey::Close,
            _ => return SideKey::Pass,
        }
        SideKey::Taken
    }

    /// The outline entry the cursor is on, when the outline is showing.
    fn outline_index(&self, outline: &[OutlineEntry]) -> Option<usize> {
        if self.tab() != Tab::Outline {
            return None;
        }
        visible_outline(outline, &self.folded).get(self.cursor).copied()
    }

    /// The rows of the list showing. `current_page` marks the section
    /// being read; `current_match` the match `n` last went to.
    pub fn rows(&self, outline: &[OutlineEntry], matches: &[MatchRow], marks: &[(char, u32)], current_page: u32, current_match: Option<usize>) -> Vec<Row> {
        match self.tab() {
            Tab::Outline => {
                let visible = visible_outline(outline, &self.folded);
                let here = here_entry(outline, current_page).map(|i| visible_ancestor(outline, &visible, i));
                visible
                    .iter()
                    .map(|&i| {
                        let e = &outline[i];
                        let marker = if !has_children(outline, i) {
                            ' '
                        } else if self.folded.contains(&i) {
                            '\u{25b8}'
                        } else {
                            '\u{25be}'
                        };
                        Row {
                            text: format!("{}{marker} {}", "  ".repeat(e.depth as usize), e.title),
                            right: (e.page_index + 1).to_string(),
                            target: Some(Target::Page(e.page_index)),
                            here: here == Some(Some(i)),
                        }
                    })
                    .collect()
            }
            Tab::Matches => matches
                .iter()
                .enumerate()
                .map(|(i, m)| Row { text: m.context.clone(), right: (m.page + 1).to_string(), target: Some(Target::Match(i)), here: current_match == Some(i) })
                .collect(),
            Tab::Marks => marks
                .iter()
                .map(|&(c, page)| Row { text: format!("{c}  mark"), right: (page + 1).to_string(), target: Some(Target::Mark(c)), here: false })
                .collect(),
        }
    }

    /// The header: the three lists, the one showing first in `[ ]`.
    pub fn header(&self, matches: usize, marks: usize, searching: bool) -> String {
        let name = |tab: Tab| match tab {
            Tab::Outline => "Outline".to_string(),
            Tab::Matches => format!("Matches {matches}{}", if searching { "\u{2026}" } else { "" }),
            Tab::Marks => format!("Marks {marks}"),
        };
        [Tab::Outline, Tab::Matches, Tab::Marks]
            .into_iter()
            .map(|t| if t == self.tab() { format!("[{}]", name(t)) } else { name(t) })
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// A row cut or padded to `cols`, its page number at the right.
pub fn fit(row: &Row, cols: usize) -> String {
    let right = if row.right.is_empty() { String::new() } else { format!(" {}", row.right) };
    let room = cols.saturating_sub(right.chars().count());
    let mut text: String = row.text.chars().take(room).collect();
    if row.text.chars().count() > room && room > 0 {
        text.pop();
        text.push('\u{2026}');
    }
    let pad = room.saturating_sub(text.chars().count());
    format!("{text}{}{right}", " ".repeat(pad))
}

pub fn has_children(outline: &[OutlineEntry], i: usize) -> bool {
    outline.get(i + 1).is_some_and(|next| next.depth > outline[i].depth)
}

fn parent_of(outline: &[OutlineEntry], i: usize) -> Option<usize> {
    let depth = outline.get(i)?.depth;
    (0..i).rev().find(|&j| outline[j].depth < depth)
}

/// The outline's entries not inside a folded one, in order.
pub fn visible_outline(outline: &[OutlineEntry], folded: &HashSet<usize>) -> Vec<usize> {
    let mut visible = Vec::new();
    let mut hide_below: Option<u32> = None;
    for (i, e) in outline.iter().enumerate() {
        if let Some(depth) = hide_below {
            if e.depth > depth {
                continue;
            }
            hide_below = None;
        }
        visible.push(i);
        if folded.contains(&i) {
            hide_below = Some(e.depth);
        }
    }
    visible
}

/// The section being read on `page`: the last entry starting at or
/// before it.
pub fn here_entry(outline: &[OutlineEntry], page: u32) -> Option<usize> {
    outline.iter().rposition(|e| e.page_index <= page)
}

/// `i`, or the nearest of its ancestors that's showing.
fn visible_ancestor(outline: &[OutlineEntry], visible: &[usize], mut i: usize) -> Option<usize> {
    loop {
        if visible.contains(&i) {
            return Some(i);
        }
        i = parent_of(outline, i)?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(title: &str, page: u32, depth: u32) -> OutlineEntry {
        OutlineEntry { title: title.to_string(), page_index: page, depth }
    }

    fn outline() -> Vec<OutlineEntry> {
        vec![entry("1 Intro", 0, 0), entry("2 Overview", 4, 0), entry("2.1 Parts", 5, 1), entry("2.2 Links", 8, 1), entry("3 Annex", 12, 0)]
    }

    fn texts(rows: &[Row]) -> Vec<String> {
        rows.iter().map(|r| r.text.clone()).collect()
    }

    #[test]
    fn the_outline_shows_its_tree_and_marks_the_section_being_read() {
        let side = Sidebar::open(Tab::Outline);
        let rows = side.rows(&outline(), &[], &[], 6, None);
        assert_eq!(texts(&rows), vec!["  1 Intro", "\u{25be} 2 Overview", "    2.1 Parts", "    2.2 Links", "  3 Annex"]);
        assert_eq!(rows.iter().position(|r| r.here), Some(2), "page 7 is in 2.1");
        assert_eq!(rows[4].right, "13");
    }

    #[test]
    fn folding_hides_children_and_here_moves_to_the_folded_parent() {
        let mut side = Sidebar::open(Tab::Outline);
        let o = outline();
        let rows = side.rows(&o, &[], &[], 6, None);
        side.cursor = 1;
        side.key(KeyPress::char('z'), &rows, &o);
        side.key(KeyPress::char('a'), &rows, &o);
        let rows = side.rows(&o, &[], &[], 6, None);
        assert_eq!(texts(&rows), vec!["  1 Intro", "\u{25b8} 2 Overview", "  3 Annex"]);
        assert_eq!(rows.iter().position(|r| r.here), Some(1));
        side.key(KeyPress::char('l'), &rows, &o);
        assert_eq!(side.rows(&o, &[], &[], 6, None).len(), 5, "l unfolds");
        side.key(KeyPress::char('z'), &rows, &o);
        side.key(KeyPress::char('M'), &rows, &o);
        assert_eq!(side.rows(&o, &[], &[], 6, None).len(), 3, "zM folds everything");
    }

    #[test]
    fn h_folds_then_goes_to_the_parent() {
        let mut side = Sidebar::open(Tab::Outline);
        let o = outline();
        side.cursor = 3;
        let rows = side.rows(&o, &[], &[], 0, None);
        side.key(KeyPress::char('h'), &rows, &o);
        assert_eq!(side.cursor, 1, "2.2 has no children, so h goes up to 2");
        let rows = side.rows(&o, &[], &[], 0, None);
        side.key(KeyPress::char('h'), &rows, &o);
        assert!(side.folded.contains(&1));
    }

    #[test]
    fn keys_move_switch_lists_go_and_close() {
        let mut side = Sidebar::open(Tab::Outline);
        let o = outline();
        let rows = side.rows(&o, &[], &[], 0, None);
        side.key(KeyPress::char('j'), &rows, &o);
        side.key(KeyPress::char('j'), &rows, &o);
        assert_eq!(side.key(KeyPress::named(NamedKey::Enter), &rows, &o), SideKey::Go(Target::Page(5)));
        side.key(KeyPress::char('G'), &rows, &o);
        assert_eq!(side.cursor, 4);
        side.key(KeyPress::char('g'), &rows, &o);
        side.key(KeyPress::char('g'), &rows, &o);
        assert_eq!(side.cursor, 0);
        side.key(KeyPress::named(NamedKey::Tab), &rows, &o);
        assert_eq!(side.tab(), Tab::Matches);
        assert_eq!(side.key(KeyPress::char('o'), &rows, &o), SideKey::Close);
        assert_eq!(side.key(KeyPress::char(' '), &rows, &o), SideKey::Pass);
        assert_eq!(side.key(KeyPress::char('w').with_ctrl(), &rows, &o), SideKey::Pass);
    }

    #[test]
    fn matches_and_marks_lists() {
        let mut side = Sidebar::open(Tab::Matches);
        let matches = vec![MatchRow { page: 2, context: "a header here".into() }, MatchRow { page: 9, context: "another".into() }];
        let rows = side.rows(&[], &matches, &[], 0, Some(1));
        assert_eq!(rows[1].target, Some(Target::Match(1)));
        assert!(rows[1].here);
        side.tab = Some(Tab::Marks);
        let rows = side.rows(&[], &matches, &[('a', 4)], 0, None);
        assert_eq!((rows[0].text.as_str(), rows[0].right.as_str()), ("a  mark", "5"));
        assert_eq!(side.header(2, 1, true), "Outline Matches 2\u{2026} [Marks 1]");
    }

    #[test]
    fn a_row_fits_its_columns_with_the_page_at_the_right() {
        let row = Row { text: "  2.1 A rather long section title".into(), right: "38".into(), target: None, here: false };
        let line = fit(&row, 20);
        assert_eq!(line.chars().count(), 20);
        assert!(line.ends_with(" 38") && line.contains('\u{2026}'), "{line}");
        let short = Row { text: "Intro".into(), right: "1".into(), target: None, here: false };
        assert_eq!(fit(&short, 10), "Intro    1");
    }

    #[test]
    fn settle_keeps_the_cursor_in_sight() {
        let mut side = Sidebar::open(Tab::Outline);
        side.cursor = 12;
        side.settle(20, 5);
        assert_eq!(side.scroll, 8);
        side.cursor = 2;
        side.settle(20, 5);
        assert_eq!(side.scroll, 2);
        side.cursor = 30;
        side.settle(20, 5);
        assert_eq!(side.cursor, 19);
    }
}
