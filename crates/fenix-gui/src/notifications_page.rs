//! `SPC o n`, the notifications' history: every notification Fenix kept,
//! newest first, each with where it came from, how long ago, its
//! urgency and its body; do not disturb's state at the top. Enter does a
//! notification's default action, `1`-`9` one of its actions, `r` shows it again,
//! `d` forgets it, `D` clears the history, `t` toggles do not disturb, `/`
//! filters. `App` (`app/notifications.rs`) gives it the notes and runs
//! what it asks for.

use std::time::SystemTime;

use crate::notify::{ago, wrap, Note, Urgency};
use crate::page::{fit, frame, Grid, Key, Page, Role};

pub struct NotificationsPage {
    /// The history, newest first, as last given.
    pub notes: Vec<Note>,
    pub dnd: bool,
    /// The focused notification's index in `shown()`.
    pub focus: usize,
    pub filter: String,
    pub filtering: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    None,
    Close,
    /// Its action `index`.
    Act(u64, usize),
    Reshow(u64),
    Forget(u64),
    ClearAll,
    ToggleDnd,
}

impl NotificationsPage {
    pub fn new(notes: Vec<Note>, dnd: bool) -> Self {
        NotificationsPage { notes, dnd, focus: 0, filter: String::new(), filtering: false }
    }

    pub fn typing(&self) -> bool {
        self.filtering
    }

    pub fn paste(&mut self, text: &str) {
        if self.filtering {
            self.filter.extend(text.chars().filter(|c| !c.is_control()));
            self.focus = 0;
        }
    }

    /// The notes the filter lets through.
    pub fn shown(&self) -> Vec<&Note> {
        let words: Vec<String> = self.filter.split_whitespace().map(str::to_lowercase).collect();
        self.notes
            .iter()
            .filter(|n| {
                let hay = format!("{} {} {}", n.source, n.summary, n.body).to_lowercase();
                words.iter().all(|w| hay.contains(w.as_str()))
            })
            .collect()
    }

    /// Takes the newest history, keeping the focus on the note it was on.
    pub fn update(&mut self, notes: Vec<Note>, dnd: bool) {
        let focused = self.shown().get(self.focus).map(|n| n.id);
        self.notes = notes;
        self.dnd = dnd;
        let shown = self.shown();
        self.focus = focused.and_then(|id| shown.iter().position(|n| n.id == id)).unwrap_or(self.focus).min(shown.len().saturating_sub(1));
    }

    pub fn key(&mut self, key: Key) -> Action {
        if self.filtering {
            match key {
                Key::Escape => {
                    self.filtering = false;
                    self.filter.clear();
                }
                Key::Enter => self.filtering = false,
                Key::Backspace => {
                    self.filter.pop();
                }
                Key::Char(c) => self.filter.push(c),
                Key::Space => self.filter.push(' '),
                _ => {}
            }
            self.focus = 0;
            return Action::None;
        }
        let shown = self.shown();
        let last = shown.len().saturating_sub(1);
        let focused = shown.get(self.focus).map(|n| n.id);
        match key {
            Key::Char('q') => return Action::Close,
            Key::Escape if !self.filter.is_empty() => {
                self.filter.clear();
                self.focus = 0;
            }
            Key::Escape => return Action::Close,
            Key::Char('j') | Key::Down => self.focus = (self.focus + 1).min(last),
            Key::Char('k') | Key::Up => self.focus = self.focus.saturating_sub(1),
            Key::Char('g') => self.focus = 0,
            Key::Char('G') => self.focus = last,
            Key::Char('/') => self.filtering = true,
            Key::Char('t') => return Action::ToggleDnd,
            Key::Char('D') => return Action::ClearAll,
            Key::Enter => return focused.map_or(Action::None, |id| Action::Act(id, 0)),
            Key::Char(c @ '1'..='9') => return focused.map_or(Action::None, |id| Action::Act(id, c as usize - '1' as usize)),
            Key::Char('r') => return focused.map_or(Action::None, Action::Reshow),
            Key::Char('d') | Key::Char('x') => return focused.map_or(Action::None, Action::Forget),
            _ => {}
        }
        Action::None
    }
}

fn urgency_role(u: Urgency) -> Role {
    match u {
        Urgency::Low => Role::Muted,
        Urgency::Normal => Role::Accent,
        Urgency::Critical => Role::Bad,
    }
}

pub fn layout(page: &NotificationsPage, cols: usize) -> Page {
    let (left, width) = frame(cols, 110);
    let mut g = Grid::new();
    g.put(1, left, "Notifications", Role::Title);
    let state = if page.dnd { "do not disturb: on -- only critical ones show" } else { "do not disturb: off" };
    g.put(1, left + 15, &format!("· {} kept · {state}", page.notes.len()), if page.dnd { Role::Warn } else { Role::Muted });
    g.rule(2, left..left + width);
    let mut row = 3;
    if page.filtering || !page.filter.is_empty() {
        let caret = if page.filtering { "▏" } else { "" };
        g.put(row, left, &format!("/ {}{caret}", page.filter), Role::Accent);
        row += 1;
    }
    row += 1;
    let shown = page.shown();
    if shown.is_empty() {
        let none = if page.notes.is_empty() { "No notifications yet." } else { "None match the filter." };
        g.put(row, left, none, Role::Muted);
    }
    let now = SystemTime::now();
    let age_col = left + width.saturating_sub(8);
    for (i, note) in shown.iter().enumerate() {
        let start = row;
        let marker = match note.urgency {
            Urgency::Critical => "!",
            _ if !note.seen => "·",
            _ => " ",
        };
        g.put(row, left, marker, urgency_role(note.urgency));
        let source_w = 14.min(width / 4);
        g.put(row, left + 2, &fit(&note.source, source_w), Role::Muted);
        let mut summary = note.summary.clone();
        if note.count > 1 {
            summary.push_str(&format!("  ×{}", note.count));
        }
        let summary_x = left + 3 + source_w;
        g.put(row, summary_x, &fit(&summary, age_col.saturating_sub(summary_x + 1)), if note.urgency == Urgency::Critical { Role::Bad } else { Role::Text });
        g.put(row, age_col, &format!("{:>8}", ago(note.at, now)), Role::Muted);
        row += 1;
        let body_w = width.saturating_sub(summary_x - left);
        let lines = if i == page.focus { 6 } else { 1 };
        for line in wrap(&note.body, body_w, lines) {
            if !line.is_empty() {
                g.put(row, summary_x, &line, Role::Muted);
            }
            row += 1;
        }
        if i == page.focus && !note.actions.is_empty() {
            let labels: Vec<String> = note.actions.iter().enumerate().map(|(i, a)| if i == 0 { format!("↵ {}", a.label) } else { format!("{} {}", i + 1, a.label) }).collect();
            g.put(row, summary_x, &fit(&labels.join("   "), body_w), Role::Accent);
            row += 1;
        }
        if i == page.focus {
            g.focus(start, left..left + width);
        }
        row += 1;
    }
    g.keys(left, width, &[("↵", "act"), ("1-9", "action"), ("r", "again"), ("d", "forget"), ("D", "clear"), ("t", "quiet"), ("/", "filter"), ("q", "close")]);
    g.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notify::{Center, Request, Settings};

    fn notes() -> (Vec<Note>, Vec<u64>) {
        let s = Settings::default();
        let mut c = Center::new();
        c.push(Request::new("tasks", "cargo build passed").body("in 41 s"), &s);
        c.push(Request::new("agents", "Claude Code asks to run a command").body("cargo test --workspace").urgency(Urgency::Critical).action("Open the thread", "agents.thread:1"), &s);
        let notes: Vec<Note> = c.history().iter().rev().cloned().collect();
        let ids = notes.iter().map(|n| n.id).collect();
        (notes, ids)
    }

    #[test]
    fn it_lists_the_newest_first_and_acts_on_the_focused() {
        let (notes, ids) = notes();
        let mut p = NotificationsPage::new(notes, false);
        let text = layout(&p, 120).all_text();
        assert!(text.contains("Notifications") && text.contains("2 kept"), "{text}");
        let asks = text.find("asks to run").unwrap();
        assert!(asks < text.find("build passed").unwrap(), "newest first");
        assert!(text.contains("↵ Open the thread"), "the focused one's actions");
        assert_eq!(p.key(Key::Enter), Action::Act(ids[0], 0));
        p.key(Key::Char('j'));
        assert_eq!(p.key(Key::Char('d')), Action::Forget(ids[1]));
        assert_eq!(p.key(Key::Char('t')), Action::ToggleDnd);
        assert_eq!(p.key(Key::Char('q')), Action::Close);
    }

    #[test]
    fn the_filter_narrows_and_keeps_the_keys_its_own() {
        let (notes, ids) = notes();
        let mut p = NotificationsPage::new(notes, true);
        p.key(Key::Char('/'));
        for c in "cargo b".chars() {
            p.key(if c == ' ' { Key::Space } else { Key::Char(c) });
        }
        assert!(p.typing());
        assert_eq!(p.shown().len(), 1);
        p.key(Key::Enter);
        assert_eq!(p.key(Key::Enter), Action::Act(ids[1], 0));
        let text = layout(&p, 120).all_text();
        assert!(text.contains("do not disturb: on") && !text.contains("asks to run"), "{text}");
        assert_eq!(p.key(Key::Escape), Action::None, "Escape clears the filter first");
        assert_eq!(p.shown().len(), 2);
    }
}
