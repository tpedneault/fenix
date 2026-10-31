//! Laying a page out cell by cell, the way Fenix lays out its own: a
//! [`Grid`] of lines, coloured runs, hairlines, a focused row, panels, a
//! popup and pictures, finished as a [`Canvas`] for [`Page::canvas`]. And
//! the pieces Fenix's pages are built from: fitting text to a width,
//! wrapping it, a `/` filter, a one-line field's caret.
//!
//! [`Page::canvas`]: crate::Page::canvas

use std::ops::Range;

use crate::{Canvas, CanvasPopup, CellSpan, Cells, Key, Picture, Role, Span};

/// Cells `cols` of line `line`.
fn cells(line: usize, cols: &Range<usize>) -> Cells {
    Cells { line: line as u32, start: cols.start as u32, end: cols.end as u32 }
}

/// A menu, field or question drawn over a page in Fenix's popup box,
/// beside the row it's about.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Popup {
    /// The page line it's about: it opens under it, or over it when
    /// there's no room below.
    pub line: usize,
    /// The page column its text lines up with.
    pub col: usize,
    /// Its rows, each a run of pieces.
    pub rows: Vec<Vec<(String, Role)>>,
}

impl Popup {
    /// Its rows as plain text.
    pub fn text(&self) -> String {
        self.rows.iter().map(|r| r.iter().map(|(t, _)| t.as_str()).collect::<String>().trim_end().to_string()).collect::<Vec<_>>().join("\n")
    }
}

/// A page being laid out: rows of cells, plus what's drawn over them.
#[derive(Debug, Clone, Default)]
pub struct Grid {
    lines: Vec<Vec<char>>,
    spans: Vec<(usize, Range<usize>, Role)>,
    rules: Vec<(usize, Range<usize>)>,
    pub panels: Vec<(usize, Range<usize>)>,
    pub focus: Option<(usize, Range<usize>)>,
    pub popup: Option<Popup>,
    pictures: Vec<Picture>,
}

/// Cuts `s` to `max` chars, ending in "…" when it had to.
pub fn fit(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let mut out: String = s.chars().take(max - 1).collect();
    out.push('…');
    out
}

/// `s` in at most `max` columns keeping its end, `…` marking what went
/// off the front -- for a field being typed, where the end is where the
/// typing is.
pub fn fit_tail(s: &str, max: usize) -> String {
    let n = s.chars().count();
    if n <= max {
        return s.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let mut out = String::from('…');
    out.extend(s.chars().skip(n - (max - 1)));
    out
}

/// A page's `/` filter: what's typed, whether it has the keyboard, and
/// its caret. Every word typed must appear, in any case.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Filter {
    pub text: String,
    pub typing: bool,
    caret: Option<usize>,
}

impl Filter {
    /// `/`: the keyboard to the filter, the caret at the end of it.
    pub fn start(&mut self) {
        self.typing = true;
        self.caret = None;
    }

    pub fn clear(&mut self) {
        self.text.clear();
        self.typing = false;
        self.caret = None;
    }

    pub fn is_empty(&self) -> bool {
        self.text.trim().is_empty()
    }

    /// A key while typing: `Esc` clears it, `Enter` (or `Down`) keeps it
    /// and hands the keyboard back, the rest edit it. Whether what it
    /// matches may have changed.
    pub fn key(&mut self, key: Key) -> bool {
        match key {
            Key::Escape => {
                self.clear();
                true
            }
            Key::Enter | Key::Down | Key::Tab => {
                self.typing = false;
                false
            }
            _ => {
                let mut at = self.caret.unwrap_or(self.text.chars().count());
                let changed = edit_line(&mut self.text, &mut at, key);
                self.caret = Some(at);
                changed
            }
        }
    }

    pub fn paste(&mut self, text: &str) {
        if self.typing {
            let mut at = self.caret.unwrap_or(usize::MAX);
            insert_at(&mut self.text, &mut at, text);
            self.caret = Some(at);
        }
    }

    /// The words typed, lowercased.
    pub fn words(&self) -> Vec<String> {
        self.text.split_whitespace().map(str::to_lowercase).collect()
    }

    /// Whether `hay` has every word typed.
    pub fn matches(&self, hay: &str) -> bool {
        let hay = hay.to_lowercase();
        self.words().iter().all(|w| hay.contains(w.as_str()))
    }

    /// The filter's line: being typed, set, or a `hint` of what it does.
    pub fn line(&self, hint: &str, width: usize) -> (String, Role) {
        if self.typing {
            let caret = self.caret.unwrap_or(usize::MAX).min(self.text.chars().count());
            (format!("/ {}", with_caret(&self.text, caret, width.saturating_sub(2))), Role::Title)
        } else if self.is_empty() {
            (fit(&format!("/ {hint}"), width), Role::Muted)
        } else {
            (fit_tail(&format!("/ {}  (/ changes it, Esc clears)", self.text), width), Role::Accent)
        }
    }
}

/// A one-line field's caret: `key` applied to `text` with the caret at
/// char `caret` -- moving it (arrows, a word at a time, `Home`, `End`)
/// or typing and deleting there. Whether the key was one of those.
pub fn edit_line(text: &mut String, caret: &mut usize, key: Key) -> bool {
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let at = (*caret).min(n);
    let word = |c: char| c.is_alphanumeric() || c == '_';
    // The start of the word before `at`, the end of the one after.
    let back = || {
        let mut i = at;
        while i > 0 && !word(chars[i - 1]) {
            i -= 1;
        }
        while i > 0 && word(chars[i - 1]) {
            i -= 1;
        }
        i
    };
    let forward = || {
        let mut i = at;
        while i < n && !word(chars[i]) {
            i += 1;
        }
        while i < n && word(chars[i]) {
            i += 1;
        }
        i
    };
    let byte = |i: usize| chars[..i].iter().map(|c| c.len_utf8()).sum::<usize>();
    match key {
        Key::Left => *caret = at.saturating_sub(1),
        Key::Right => *caret = (at + 1).min(n),
        Key::Home => *caret = 0,
        Key::End => *caret = n,
        Key::WordLeft => *caret = back(),
        Key::WordRight => *caret = forward(),
        Key::Backspace => {
            if at > 0 {
                text.remove(byte(at - 1));
                *caret = at - 1;
            } else {
                *caret = 0;
            }
        }
        Key::DeleteWordBack => {
            let from = back();
            text.replace_range(byte(from)..byte(at), "");
            *caret = from;
        }
        Key::Delete => {
            if at < n {
                text.remove(byte(at));
            }
            *caret = at;
        }
        Key::Char(c) => {
            text.insert(byte(at), c);
            *caret = at + 1;
        }
        _ => return false,
    }
    true
}

/// `more` typed (pasted) into `text` at `caret`, the caret after it.
pub fn insert_at(text: &mut String, caret: &mut usize, more: &str) {
    let at = (*caret).min(text.chars().count());
    let byte: usize = text.chars().take(at).map(|c| c.len_utf8()).sum();
    let more: String = more.chars().filter(|c| !c.is_control()).collect();
    text.insert_str(byte, &more);
    *caret = at + more.chars().count();
}

/// A field being typed, in at most `max` columns: `text` with `▏` at
/// char `caret`, cut down around the caret when it doesn't fit, `…`
/// marking what's cut.
pub fn with_caret(text: &str, caret: usize, max: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    let at = caret.min(chars.len());
    let mut all: Vec<char> = chars[..at].to_vec();
    all.push('▏');
    all.extend_from_slice(&chars[at..]);
    let n = all.len();
    if n <= max {
        return all.into_iter().collect();
    }
    if max < 3 {
        return fit_tail(&all.into_iter().collect::<String>(), max);
    }
    // The caret near the end: the end, as `fit_tail` shows it.
    if n - at < max {
        return std::iter::once('…').chain(all[n - (max - 1)..].iter().copied()).collect();
    }
    let start = at.saturating_sub(max / 2);
    if start == 0 {
        return all[..max - 1].iter().copied().chain(std::iter::once('…')).collect();
    }
    std::iter::once('…').chain(all[start + 1..start + max - 1].iter().copied()).chain(std::iter::once('…')).collect()
}

/// `text` wrapped at word boundaries to lines of at most `width`; a
/// word longer than that (a path) is broken wherever it has to be.
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut lines = Vec::new();
    let mut line = String::new();
    let mut words: Vec<String> = Vec::new();
    for word in text.split_whitespace() {
        let chars: Vec<char> = word.chars().collect();
        words.extend(chars.chunks(width).map(|c| c.iter().collect::<String>()));
    }
    for word in &words {
        if !line.is_empty() && line.chars().count() + 1 + word.chars().count() > width {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

/// Where a page's content goes in a pane `cols` wide: its left edge and
/// width, centred and capped the way Fenix's own pages are.
pub fn frame(cols: usize, max_width: usize) -> (usize, usize) {
    let cols = cols.max(40);
    let width = cols.saturating_sub(4).min(max_width);
    (cols.saturating_sub(width) / 2, width)
}

impl Grid {
    pub fn new() -> Self {
        Grid::default()
    }

    /// Leaves `rows` lines for image `image` (from [`crate::image::load`]),
    /// `cols` wide at (`line`, `col`).
    pub fn picture(&mut self, line: usize, col: usize, rows: usize, cols: usize, image: u32, view: Option<(f32, f32, f32)>) {
        if self.lines.len() < line + rows {
            self.lines.resize(line + rows, Vec::new());
        }
        self.pictures.push(Picture { cells: cells(line, &(col..col + cols)), rows: rows as u32, image, view, marks: Vec::new() });
    }

    /// Boxes outlined over the last picture: its nodes, say.
    pub fn marks(&mut self, marks: Vec<crate::Mark>) {
        if let Some(p) = self.pictures.last_mut() {
            p.marks = marks;
        }
    }

    /// Tints `cols` of `line` as a panel.
    pub fn panel(&mut self, line: usize, cols: Range<usize>) {
        if self.lines.len() <= line {
            self.lines.resize(line + 1, Vec::new());
        }
        self.panels.push((line, cols));
    }

    /// How many lines have been written.
    pub fn height(&self) -> usize {
        self.lines.len()
    }

    /// Writes `text` at (`line`, `col`); returns the column after it.
    pub fn put(&mut self, line: usize, col: usize, text: &str, role: Role) -> usize {
        let len = text.chars().count();
        if self.lines.len() <= line {
            self.lines.resize(line + 1, Vec::new());
        }
        if len == 0 {
            return col;
        }
        let row = &mut self.lines[line];
        if row.len() < col + len {
            row.resize(col + len, ' ');
        }
        for (i, c) in text.chars().enumerate() {
            row[col + i] = c;
        }
        self.spans.push((line, col..col + len, role));
        col + len
    }

    pub fn rule(&mut self, line: usize, cols: Range<usize>) {
        if self.lines.len() <= line {
            self.lines.resize(line + 1, Vec::new());
        }
        self.rules.push((line, cols));
    }

    /// A section heading: "FILES · 8 ─────────".
    pub fn heading(&mut self, line: usize, x: usize, width: usize, text: &str) {
        let end = self.put(line, x, &text.to_uppercase(), Role::Muted);
        if end + 2 < x + width {
            self.rule(line, end + 1..x + width);
        }
    }

    /// Marks row `line` across `cols` as the focused one.
    pub fn focus(&mut self, line: usize, cols: Range<usize>) {
        self.focus = Some((line, cols));
    }

    /// The key strip: a hairline, then "Enter open   j k move ..." --
    /// as many as fit.
    pub fn keys(&mut self, left: usize, width: usize, keys: &[(&str, &str)]) {
        let foot = self.lines.len() + 1;
        self.rule(foot, left..left + width);
        let mut x = left;
        for (key, label) in keys {
            if x + key.chars().count() + 1 + label.chars().count() > left + width {
                break;
            }
            x = self.put(foot + 1, x, key, Role::Accent) + 1;
            x = self.put(foot + 1, x, label, Role::Muted) + 3;
        }
    }

    /// Its text so far: what a test reads.
    pub fn text(&self) -> String {
        self.lines.iter().map(|row| row.iter().collect::<String>().trim_end().to_string()).collect::<Vec<_>>().join("\n")
    }

    pub fn finish(self) -> Canvas {
        Canvas {
            text: self.text(),
            spans: self.spans.iter().map(|(line, cols, role)| CellSpan { cells: cells(*line, cols), role: role.clone() }).collect(),
            rules: self.rules.iter().map(|(line, cols)| cells(*line, cols)).collect(),
            focus: self.focus.as_ref().map(|(line, cols)| cells(*line, cols)),
            panels: self.panels.iter().map(|(line, cols)| cells(*line, cols)).collect(),
            pictures: self.pictures,
            popup: self.popup.map(|p| CanvasPopup {
                line: p.line as u32,
                column: p.col as u32,
                rows: p.rows.into_iter().map(|row| row.into_iter().map(|(text, role)| Span { text, role }).collect()).collect(),
            }),
        }
    }
}

impl CanvasPopup {
    /// Its rows as plain text.
    pub fn text(&self) -> String {
        self.rows.iter().map(|r| r.iter().map(|s| s.text.as_str()).collect::<String>().trim_end().to_string()).collect::<Vec<_>>().join("\n")
    }
}

impl Canvas {
    /// Its text with its popup's under it: what a test reads.
    pub fn all_text(&self) -> String {
        match &self.popup {
            Some(p) => format!("{}\n{}", self.text, p.text()),
            None => self.text.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_grid_finishes_as_a_canvas() {
        let mut g = Grid::new();
        let end = g.put(1, 2, "Today", Role::Title);
        g.heading(3, 2, 30, "doing · 1");
        g.focus(4, 2..32);
        g.put(4, 4, "Write the report", Role::Text);
        g.keys(2, 30, &[("Enter", "open"), ("q", "close")]);
        let c = g.finish();
        assert_eq!(end, 7);
        assert!(c.text.contains("  Today") && c.text.contains("DOING · 1") && c.text.contains("Enter open"), "{}", c.text);
        assert_eq!(c.focus, Some(Cells { line: 4, start: 2, end: 32 }));
        assert_eq!(c.spans[0], CellSpan { cells: Cells { line: 1, start: 2, end: 7 }, role: Role::Title });
        assert!(c.rules.iter().any(|r| r.line == 3));
    }

    #[test]
    fn a_field_is_edited_a_word_at_a_time() {
        let (mut text, mut at) = ("fix the bug".to_string(), 11);
        assert!(edit_line(&mut text, &mut at, Key::DeleteWordBack));
        assert_eq!((text.as_str(), at), ("fix the ", 8));
        edit_line(&mut text, &mut at, Key::WordLeft);
        assert_eq!(at, 4);
        insert_at(&mut text, &mut at, "all ");
        assert_eq!(text, "fix all the ");
        assert_eq!(with_caret("abc", 1, 10), "a▏bc");
        assert_eq!(fit("abcdef", 4), "abc…");
        assert_eq!(wrap("one two three", 7), ["one two", "three"]);
        assert_eq!(frame(100, 80), (10, 80));
    }
}
