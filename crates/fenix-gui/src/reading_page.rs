//! The reading view: a Markdown buffer rendered (`reading::layout`), in a
//! pane beside it that follows its cursor (`SPC m p`) or in its place
//! (`SPC m r`). `j`/`k` move through it, `Tab` steps between its links
//! and checkboxes, `Enter` follows a link or goes to the source line,
//! `x` ticks a checkbox (in the source), `i` goes back to editing there.

use std::path::PathBuf;

use crate::page::{Grid, Key, Page, Role};
use crate::reading::{Action as RAction, Rendered};

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    None,
    Close,
    /// Do what the target does.
    Target(RAction),
    /// Edit the source at this line.
    Edit(usize),
}

pub struct ReadingPage {
    /// The buffer it renders.
    pub source: fenix_buffers::BufferId,
    pub path: Option<PathBuf>,
    /// In place of the source (a tab of its own) rather than beside it.
    pub replaces: bool,
    pub rendered: Rendered,
    /// What it was rendered from: the source's edit count and the width.
    pub seen: Option<(u64, usize)>,
    /// The source cursor's line when the view last followed it.
    pub followed: Option<usize>,
    /// The page line the view is on.
    pub line: usize,
    /// The selected target, by index into `rendered.targets`.
    pub target: Option<usize>,
}

impl ReadingPage {
    pub fn new(source: fenix_buffers::BufferId, path: Option<PathBuf>, replaces: bool) -> Self {
        ReadingPage { source, path, replaces, rendered: Rendered::default(), seen: None, followed: None, line: 0, target: None }
    }

    fn lines(&self) -> usize {
        self.rendered.page.text.lines().count().max(1)
    }

    /// Puts the view on the block that renders source line `line`.
    pub fn follow(&mut self, line: usize) {
        self.line = self.rendered.page_line_of(line);
        self.target = None;
        self.followed = Some(line);
    }

    /// The first target on or after (`forward`) / before the current one.
    fn step_target(&mut self, forward: bool) {
        let t = &self.rendered.targets;
        if t.is_empty() {
            return;
        }
        let next = match (self.target, forward) {
            (Some(i), true) => (i + 1) % t.len(),
            (Some(i), false) => (i + t.len() - 1) % t.len(),
            (None, true) => t.iter().position(|x| x.line >= self.line).unwrap_or(0),
            (None, false) => t.iter().rposition(|x| x.line <= self.line).unwrap_or(t.len() - 1),
        };
        self.target = Some(next);
        self.line = t[next].line;
    }

    pub fn key(&mut self, key: Key) -> Action {
        let n = self.lines();
        match key {
            Key::Down | Key::Char('j') => {
                self.line = (self.line + 1).min(n - 1);
                self.target = None;
            }
            Key::Up | Key::Char('k') => {
                self.line = self.line.saturating_sub(1);
                self.target = None;
            }
            Key::Char('d') => {
                self.line = (self.line + 20).min(n - 1);
                self.target = None;
            }
            Key::Char('u') => {
                self.line = self.line.saturating_sub(20);
                self.target = None;
            }
            Key::Char('g') => {
                self.line = 0;
                self.target = None;
            }
            Key::Char('G') => {
                self.line = n - 1;
                self.target = None;
            }
            Key::Tab | Key::Char('n') => self.step_target(true),
            Key::BackTab | Key::Char('N') => self.step_target(false),
            Key::Char('q') | Key::Escape => return Action::Close,
            Key::Char('i') => return Action::Edit(self.rendered.source_line_of(self.line).unwrap_or(0)),
            Key::Enter | Key::Char('o') => {
                if let Some(t) = self.selected() {
                    return Action::Target(t.action.clone());
                }
                return Action::Edit(self.rendered.source_line_of(self.line).unwrap_or(0));
            }
            Key::Char('x') => {
                let on_line = self.rendered.targets.iter().find(|t| t.line == self.line && matches!(t.action, RAction::Checkbox { .. }));
                if let Some(t) = self.selected().filter(|t| matches!(t.action, RAction::Checkbox { .. })).or(on_line) {
                    return Action::Target(t.action.clone());
                }
            }
            _ => {}
        }
        Action::None
    }

    /// The selected target, or the only one on the current line.
    fn selected(&self) -> Option<&crate::reading::Target> {
        match self.target {
            Some(i) => self.rendered.targets.get(i),
            None => {
                let mut on = self.rendered.targets.iter().filter(|t| t.line == self.line);
                let first = on.next();
                first.filter(|_| on.next().is_none())
            }
        }
    }
}

pub fn layout(page: &ReadingPage, cols: usize) -> Page {
    let mut g = Grid::new();
    let left = 1;
    g.embed(&page.rendered.page, 0, left, false);
    let width = cols.saturating_sub(2);
    // The line (or target) the view is on.
    let line = page.line.min(page.rendered.page.text.lines().count().saturating_sub(1));
    match page.target.and_then(|i| page.rendered.targets.get(i)) {
        Some(t) => g.focus(t.line, t.cols.start + left..t.cols.end + left),
        None => g.focus(line, left..left + width),
    }
    if page.rendered.page.text.trim().is_empty() {
        g.put(0, left, "Nothing to read yet.", Role::Muted);
    }
    let keys: &[(&str, &str)] = &[("Tab", "next link"), ("Enter", "follow"), ("x", "tick"), ("i", "edit here"), ("q", "close")];
    g.keys(left, width, keys);
    g.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reading::{layout as render, Ctx};

    fn page(text: &str) -> ReadingPage {
        let mut p = ReadingPage::new(fenix_buffers::BufferList::new().open_scratch(), None, false);
        p.rendered = render(text, 60, &Ctx::plain());
        p
    }

    #[test]
    fn it_follows_the_source_and_steps_through_targets() {
        let mut p = page("# T\n\ntext [[A]] and [[B]]\n\n- [ ] task\n");
        p.follow(4);
        assert_eq!(p.line, 4);
        assert_eq!(p.key(Key::Char('x')), Action::Target(RAction::Checkbox { line: 4, done: false }));
        p.key(Key::Char('g'));
        p.key(Key::Tab);
        assert_eq!(p.line, 2);
        assert!(matches!(p.key(Key::Enter), Action::Target(RAction::Link(_))));
        p.key(Key::Tab);
        p.key(Key::Tab);
        assert_eq!(p.line, 4);
        p.key(Key::Char('k'));
        assert_eq!(p.key(Key::Char('i')), Action::Edit(2));
        assert!(layout(&p, 60).text.contains("text A and B"));
    }
}
