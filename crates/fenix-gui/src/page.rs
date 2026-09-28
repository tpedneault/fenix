//! The pieces every project page is built from -- the new-project
//! wizard, the project hub, the doctor and the settings page. A page is
//! generated text plus colour spans, hairlines and a focused row, laid
//! out for a pane's width in cells; `App` writes the text into a
//! `BufferKind::Page` buffer and draws the rest (see `app/pages.rs`).
//! Nothing here touches a window, so every page's layout is plain data
//! a test can read.

use std::ops::Range;

use fenix_project::ProjectKind;

/// One keypress, already decoded by `App`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Up,
    Down,
    Left,
    Right,
    Enter,
    Tab,
    BackTab,
    Escape,
    Space,
    Backspace,
    Char(char),
    CtrlC,
    /// Browse for a path, while one is being typed; back, on a MIB
    /// definition's page.
    CtrlO,
    /// Forward, on a MIB definition's page.
    CtrlI,
    /// Done: insert what the form built.
    CtrlEnter,
    /// While a field is typed: the caret to the start, the end.
    Home,
    End,
    /// While a field is typed: the character after the caret.
    Delete,
    /// While a field is typed (`Ctrl-←`, `Ctrl-→`): a word back, forward.
    WordLeft,
    WordRight,
    /// While a field is typed (`Ctrl-Backspace`): the word before the caret.
    DeleteWordBack,
}

/// A colour role, resolved against the theme by `App`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Title,
    Text,
    Muted,
    /// The theme's accent: the current step, keys, `›`.
    Accent,
    Good,
    Warn,
    Bad,
    Kind(ProjectKind),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Span {
    pub line: usize,
    pub cols: Range<usize>,
    pub role: Role,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Page {
    pub text: String,
    pub spans: Vec<Span>,
    /// Hairlines, as (line, cells) -- drawn, not typed.
    pub rules: Vec<(usize, Range<usize>)>,
    /// The focused row: its line and cells, for the tint and the rail.
    pub focus: Option<(usize, Range<usize>)>,
    /// Cells tinted as a panel (a text field being edited).
    pub panels: Vec<(usize, Range<usize>)>,
    /// A menu, field or question floating over the page.
    pub popup: Option<Popup>,
}

/// A menu, field or question drawn over a page in the editor's own
/// popup box -- bordered, shadowed, on the panel colour -- beside the
/// row it's about, rather than written into the page between its rows.
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
    #[cfg(test)]
    pub fn text(&self) -> String {
        self.rows.iter().map(|r| r.iter().map(|(t, _)| t.as_str()).collect::<String>().trim_end().to_string()).collect::<Vec<_>>().join("\n")
    }

    /// Its width in cells: the longest row.
    pub fn cols(&self) -> usize {
        self.rows.iter().map(|r| r.iter().map(|(t, _)| t.chars().count()).sum::<usize>()).max().unwrap_or(0)
    }
}

impl Page {
    /// The page's text with its popup's under it -- what a test reads.
    #[cfg(test)]
    pub fn all_text(&self) -> String {
        match &self.popup {
            Some(p) => format!("{}\n{}", self.text, p.text()),
            None => self.text.clone(),
        }
    }

    /// Where the cursor sits: on the focused row, else the top.
    pub fn cursor(&self) -> (usize, usize) {
        self.focus.as_ref().map(|(line, cols)| (*line, cols.start + 1)).unwrap_or((0, 0))
    }
}

/// A page being laid out: rows of cells, plus what's drawn over them.
pub struct Grid {
    lines: Vec<Vec<char>>,
    spans: Vec<Span>,
    rules: Vec<(usize, Range<usize>)>,
    pub panels: Vec<(usize, Range<usize>)>,
    pub focus: Option<(usize, Range<usize>)>,
    pub popup: Option<Popup>,
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

/// A one-line field's caret: `key` applied to `text` with the caret at
/// char `caret` -- moving it (arrows, `Ctrl` a word at a time, `Home`,
/// `End`) or typing and deleting there. Whether the key was one of those.
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
        Key::Space => {
            text.insert(byte(at), ' ');
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
    if n - at <= max - 1 {
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
/// width, centred and capped the way Home's is.
pub fn frame(cols: usize, max_width: usize) -> (usize, usize) {
    let cols = cols.max(40);
    let width = cols.saturating_sub(4).min(max_width);
    (cols.saturating_sub(width) / 2, width)
}

impl Default for Grid {
    fn default() -> Self {
        Self::new()
    }
}

impl Grid {
    pub fn new() -> Self {
        Grid { lines: Vec::new(), spans: Vec::new(), rules: Vec::new(), panels: Vec::new(), focus: None, popup: None }
    }

    /// Draws `page` with its top-left at (`line`, `col`): its text, its
    /// colours, hairlines, panels and popup, moved there -- and its
    /// focused row, when `focus` says it has the keyboard.
    pub fn embed(&mut self, page: &Page, line: usize, col: usize, focus: bool) {
        for (i, text) in page.text.lines().enumerate() {
            self.write(line + i, col, text);
        }
        let shift = |cols: &Range<usize>| cols.start + col..cols.end + col;
        self.spans.extend(page.spans.iter().map(|s| Span { line: s.line + line, cols: shift(&s.cols), role: s.role }));
        self.rules.extend(page.rules.iter().map(|(l, c)| (l + line, shift(c))));
        self.panels.extend(page.panels.iter().map(|(l, c)| (l + line, shift(c))));
        if focus {
            if let Some((l, c)) = &page.focus {
                self.focus = Some((l + line, shift(c)));
            }
        }
        if let Some(popup) = &page.popup {
            self.popup = Some(Popup { line: popup.line + line, col: popup.col + col, ..popup.clone() });
        }
    }

    /// Writes `text` at (`line`, `col`) without colouring it.
    fn write(&mut self, line: usize, col: usize, text: &str) {
        if self.lines.len() <= line {
            self.lines.resize(line + 1, Vec::new());
        }
        let len = text.chars().count();
        if len == 0 {
            return;
        }
        let row = &mut self.lines[line];
        if row.len() < col + len {
            row.resize(col + len, ' ');
        }
        for (i, c) in text.chars().enumerate() {
            row[col + i] = c;
        }
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
        self.spans.push(Span { line, cols: col..col + len, role });
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

    pub fn finish(self) -> Page {
        let text = self.lines.iter().map(|row| row.iter().collect::<String>().trim_end().to_string()).collect::<Vec<_>>().join("\n");
        Page { text, spans: self.spans, rules: self.rules, focus: self.focus, panels: self.panels, popup: self.popup }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_long_path_is_broken_to_fit() {
        let lines = wrap(&format!("{} already exists", "x".repeat(50)), 20);
        assert!(lines.iter().all(|l| l.chars().count() <= 20), "{lines:?}");
        assert_eq!(lines.concat().replace(' ', ""), format!("{}alreadyexists", "x".repeat(50)));
    }

    #[test]
    fn the_key_strip_keeps_to_the_width() {
        let mut g = Grid::new();
        g.put(0, 0, "x", Role::Text);
        g.keys(0, 20, &[("Enter", "open"), ("j k", "move"), ("q", "close")]);
        let page = g.finish();
        assert!(page.text.lines().all(|l| l.chars().count() <= 20), "{}", page.text);
        assert!(page.text.contains("Enter open"));
    }

    #[test]
    fn fit_ends_in_an_ellipsis_only_when_it_cuts() {
        assert_eq!(fit("abc", 3), "abc");
        assert_eq!(fit("abcd", 3), "ab…");
        assert_eq!(fit("abcd", 0), "");
    }

    #[test]
    fn fit_tail_keeps_the_end_and_marks_the_front() {
        assert_eq!(fit_tail("abc", 3), "abc");
        assert_eq!(fit_tail("abcd", 3), "…cd");
        assert_eq!(fit_tail("abcd", 0), "");
    }

    #[test]
    fn a_field_moves_its_caret_by_character_and_word_and_types_there() {
        let mut text = "cargo build --release".to_string();
        let mut caret = text.chars().count();
        assert!(edit_line(&mut text, &mut caret, Key::WordLeft));
        assert_eq!(caret, 14, "to the start of release");
        edit_line(&mut text, &mut caret, Key::WordLeft);
        assert_eq!(caret, 6, "past the dashes, to build");
        edit_line(&mut text, &mut caret, Key::Char('x'));
        assert_eq!(text, "cargo xbuild --release");
        edit_line(&mut text, &mut caret, Key::Backspace);
        edit_line(&mut text, &mut caret, Key::WordRight);
        assert_eq!(caret, 11);
        edit_line(&mut text, &mut caret, Key::DeleteWordBack);
        assert_eq!(text, "cargo  --release");
        edit_line(&mut text, &mut caret, Key::Home);
        edit_line(&mut text, &mut caret, Key::Delete);
        assert_eq!((text.as_str(), caret), ("argo  --release", 0));
        edit_line(&mut text, &mut caret, Key::End);
        edit_line(&mut text, &mut caret, Key::Left);
        insert_at(&mut text, &mut caret, "é\nz");
        assert_eq!((text.as_str(), caret), ("argo  --releaséze", 16));
        assert!(!edit_line(&mut text, &mut caret, Key::Enter));
    }

    #[test]
    fn a_long_field_keeps_its_caret_in_view() {
        assert_eq!(with_caret("abc", 1, 10), "a▏bc");
        assert_eq!(with_caret("abcdefghij", 10, 6), "…ghij▏");
        assert_eq!(with_caret("abcdefghij", 0, 6), "▏abcd…");
        let mid = with_caret("abcdefghij", 5, 6);
        assert!(mid.starts_with('…') && mid.ends_with('…') && mid.contains('▏') && mid.chars().count() == 6, "{mid}");
    }
}
