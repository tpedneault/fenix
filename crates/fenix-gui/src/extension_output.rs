//! `SPC h l`, the extensions' output page: each extension that runs code,
//! where it is, and what it said (`app/wasm_host.rs` writes the text).
//! Read-only, and laid out again as they say more.

use crate::page::{fit, frame, Grid, Key, Page, Role};

pub struct OutputPage {
    /// What `App::extension_output_text` made of it.
    pub text: String,
    /// The line the cursor is on.
    pub focus: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    None,
    Close,
}

impl OutputPage {
    pub fn new(text: String) -> Self {
        OutputPage { text, focus: 0 }
    }

    fn lines(&self) -> usize {
        self.text.lines().count().max(1)
    }

    pub fn key(&mut self, key: Key) -> Action {
        let last = self.lines() - 1;
        match key {
            Key::Char('q') | Key::Escape => return Action::Close,
            Key::Char('j') | Key::Down => self.focus = (self.focus + 1).min(last),
            Key::Char('k') | Key::Up => self.focus = self.focus.saturating_sub(1),
            Key::Char('g') => self.focus = 0,
            Key::Char('G') => self.focus = last,
            _ => {}
        }
        Action::None
    }
}

/// How a line reads: an extension's heading, an error, Fenix's own note,
/// or what it said.
fn role(line: &str) -> Role {
    let said = line.trim_start();
    if !line.starts_with(' ') {
        Role::Title
    } else if said.starts_with("error") || said.starts_with("stderr") || said.starts_with("-- ") && said.contains("stopped") {
        Role::Warn
    } else if said.starts_with("-- ") || said.starts_with("debug") {
        Role::Muted
    } else {
        Role::Text
    }
}

pub fn layout(page: &OutputPage, cols: usize) -> Page {
    let (left, width) = frame(cols, 120);
    let mut g = Grid::new();
    g.put(1, left, "Extensions' output", Role::Title);
    g.put(1, left + 20, "· what the extensions that run code said", Role::Muted);
    g.rule(2, left..left + width);
    let top = 4;
    for (i, line) in page.text.lines().enumerate() {
        if i == page.focus {
            g.focus(top + i, left..left + width);
        }
        g.put(top + i, left, &fit(line, width), role(line));
    }
    g.keys(left, width, &[("j k", "move"), ("q", "close")]);
    g.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_shows_the_text_and_moves_through_it() {
        let mut p = OutputPage::new("Hello 1.0.0 (test.hello): running\n  -- ready\n    info  hello, Fenix\n   error  hello.fail: asked to fail\n".into());
        let text = layout(&p, 120).all_text();
        assert!(text.contains("Extensions' output") && text.contains("hello, Fenix") && text.contains("asked to fail"), "{text}");
        p.key(Key::Char('G'));
        assert_eq!(p.focus, 3);
        p.key(Key::Char('j'));
        assert_eq!(p.focus, 3, "not past the end");
        assert_eq!(p.key(Key::Char('q')), Action::Close);
        assert_eq!(role("   error  x"), Role::Warn);
        assert_eq!(role("  -- ready"), Role::Muted);
        assert_eq!(role("  -- test.hello stopped: it crashed"), Role::Warn);
        assert_eq!(role("Hello 1.0.0"), Role::Title);
    }
}
