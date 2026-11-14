//! The pieces every project page is built from -- the new-project
//! wizard, the project hub, the doctor and the settings page. A page is
//! generated text plus colour spans, hairlines and a focused row, laid
//! out for a pane's width in cells; `App` writes the text into a
//! `BufferKind::Page` buffer and draws the rest (see `app/pages.rs`).
//! Nothing here touches a window, so every page's layout is plain data
//! a test can read.

use std::ops::Range;
use std::path::{Path, PathBuf};

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
    /// Browse for a path, while one is being typed.
    CtrlO,
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
    /// While a field is typed (`Ctrl-U`): everything before the caret.
    DeleteToStart,
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
    /// A syntax capture's colour (`"keyword"`, `"text.title"`), for
    /// highlighted code and rendered Markdown.
    Syntax(&'static str),
    /// A colour of its own: an extension's (`page-render-ex`).
    Rgb(u8, u8, u8),
    /// What git says of a file: the theme's colour for it.
    Git(fenix_explorer::GitStatus),
}

/// An image drawn over a page's cells: a rendered diagram or a picture
/// in a note. The page leaves the cells blank; `App` draws the texture.
#[derive(Debug, Clone, PartialEq)]
pub struct PageImage {
    pub line: usize,
    pub col: usize,
    pub rows: usize,
    pub cols: usize,
    pub key: ImageKey,
    /// Zoomed and panned: (zoom, 1 = fit; the centre shown, as fractions
    /// of the picture). `None`: fitted, no bigger than it is.
    pub view: Option<(f32, f32, f32)>,
}

/// Which picture: a file on disk, or a diagram's source in a theme.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ImageKey {
    File(std::path::PathBuf),
    /// A hash of the diagram's source and the theme it's drawn in.
    Diagram(u64),
    /// An extension's image (`image-load`, `image-file`), by
    /// `wasm_host::image_key`.
    Code(u64),
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
    /// Pictures drawn over blank cells.
    pub images: Vec<PageImage>,
    /// Cells on a background of their own (an extension's canvas:
    /// matches, a selection), as (line, cells, which).
    pub backgrounds: Vec<(usize, Range<usize>, Shade)>,
    /// Cells underlined in a role's colour: what a filter matched, an
    /// extension's.
    pub underlines: Vec<(usize, Range<usize>, Role)>,
    /// Hairlines down the page: a cell column (a wide character two), and
    /// the lines it goes from and to.
    pub vertical_rules: Vec<(usize, Range<usize>)>,
    /// Focused rows besides `focus`: tinted, not kept in view.
    pub focuses: Vec<(usize, Range<usize>)>,
    /// Rounded boxes behind lines of the page: what belongs together
    /// (an agent's tool calls, a question it asks), set off as a card.
    pub cards: Vec<Card>,
    /// Small rounded boxes behind a few cells: a key's cap, a status.
    pub chips: Vec<(usize, Range<usize>, Chip)>,
    /// A commit graph's rows, drawn as lines over blank cells.
    pub graph: Vec<GraphStrip>,
}

/// One row of a commit graph, drawn over the blank cells `cols` of
/// `line`: its lanes' rails, the bends into and out of its commit, and
/// the commit's dot. Drawn rather than written, so lanes are spaced to
/// fit the cells however many there are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphStrip {
    pub line: usize,
    pub cols: Range<usize>,
    /// How many lanes the whole graph has: every row spaces them alike.
    pub lanes: usize,
    pub marks: Vec<GraphMark>,
}

/// What a commit graph's row draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphMark {
    /// A lane's rail through the row: its upper half, its lower half, or
    /// both.
    Rail { lane: usize, top: bool, bottom: bool, role: Role },
    /// A lane bending from `from` to `to`: down from the row's top into
    /// the commit at its middle (`into`), or out of the commit at its
    /// middle and down to the row's bottom.
    Bend { from: usize, to: usize, into: bool, role: Role },
    /// The commit itself, in its lane.
    Node { lane: usize, look: NodeLook, role: Role },
}

impl GraphMark {
    /// The colour it's drawn in.
    pub fn role(&self) -> Role {
        match self {
            GraphMark::Rail { role, .. } | GraphMark::Bend { role, .. } | GraphMark::Node { role, .. } => *role,
        }
    }
}

/// How a commit's dot is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeLook {
    Commit,
    /// A merge: a ring.
    Merge,
    /// Where HEAD is: ringed in the caret's colour.
    Head,
}

/// A card behind `lines` of a page, across `cols`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Card {
    pub lines: Range<usize>,
    pub cols: Range<usize>,
    pub look: CardLook,
}

/// How a card is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardLook {
    /// A step off the page: what a tool did.
    Plain,
    /// A step further, where you type or what you wrote.
    Input,
    /// Waiting on you: outlined in the accent.
    Ask,
    /// A box you type in: an input's surface, outlined faintly.
    Field,
    /// A box you're typing in: outlined in the accent.
    FieldFocused,
    /// The row the cursor is on, opened: tinted and outlined.
    Selected,
}

/// How a chip is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Chip {
    /// A key's cap.
    Key,
    /// The key of the choice the cursor is on: in the accent.
    Chosen,
    /// A status, faded in its role's colour.
    Status(Role),
    /// An option that's on or off: on, tinted and outlined; off, plain.
    Toggle(bool),
    /// A count beside a title, quiet.
    Badge,
    /// A value you can change: on an input's surface.
    Value,
    /// On or off, drawn as a switch over its blank cells.
    Switch(bool),
}

/// A background for cells: one of the theme's tints, or a colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shade {
    Panel,
    Focus,
    Selection,
    Accent,
    Good,
    Warn,
    Bad,
    Rgb(u8, u8, u8),
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

    /// The cell each character of line `line` starts at, and the one
    /// past its last: a wide character (CJK, an emoji) takes two, as the
    /// glyph is drawn. None when every one takes one -- most lines.
    pub fn cells(&self, line: usize) -> Option<Vec<usize>> {
        line_cells(self.text.lines().nth(line)?)
    }

    /// Where the cursor sits: on the focused row, else the top.
    pub fn cursor(&self) -> (usize, usize) {
        self.focus.as_ref().map(|(line, cols)| (*line, cols.start + 1)).unwrap_or((0, 0))
    }
}

/// How many cells `c` takes: two for a wide character, as a terminal
/// lays it out and Fenix draws it (a glyph's advance rounded to the
/// grid), none for a combining one.
pub fn char_cells(c: char) -> usize {
    unicode_width::UnicodeWidthChar::width(c).unwrap_or(1)
}

/// The cell each character of `line` starts at, and one past the end;
/// none when every character is one cell.
pub fn line_cells(line: &str) -> Option<Vec<usize>> {
    if line.chars().all(|c| char_cells(c) == 1) {
        return None;
    }
    let mut at = 0;
    let mut cells: Vec<usize> = line.chars().map(|c| { let here = at; at += char_cells(c); here }).collect();
    cells.push(at);
    Some(cells)
}

/// Column `col` (in characters) of a line as a cell, by its `cells`
/// (`line_cells`): past the end, a blank cell a character.
pub fn cell_of(cells: Option<&[usize]>, col: usize) -> usize {
    match cells {
        Some(cells) if col >= cells.len() => cells[cells.len() - 1] + col + 1 - cells.len(),
        Some(cells) => cells[col],
        None => col,
    }
}

/// Cell `cell` of a line as a column in characters, the inverse of
/// `cell_of`: either half of a wide character is it.
pub fn col_of(cells: Option<&[usize]>, cell: usize) -> usize {
    match cells {
        Some(cells) => {
            let end = cells[cells.len() - 1];
            if cell >= end {
                return cells.len() - 1 + cell - end;
            }
            cells.iter().rposition(|&c| c <= cell).unwrap_or(0)
        }
        None => cell,
    }
}

/// Columns `cols` of a line as the cells a box across them takes -- a
/// tint, a hairline, a card: from where its first column starts, as many
/// cells as it has columns, or on to the end of the line's text in it
/// when a wide character takes that further. Text that was fitted to
/// cells keeps a box to its width; text that wasn't stays inside it.
pub fn box_cells(cells: Option<&[usize]>, cols: &Range<usize>) -> (usize, usize) {
    let start = cell_of(cells, cols.start);
    let text = cells.map_or(0, |c| c.len() - 1);
    (start, (start + cols.len()).max(cell_of(cells, cols.end.min(text))))
}

/// The cells a card takes: as `box_cells`, over each of its lines.
pub fn card_cells(page: &Page, card: &Card) -> (usize, usize) {
    let lines: Vec<&str> = page.text.lines().skip(card.lines.start).take(card.lines.len()).collect();
    let start = cell_of(lines.first().and_then(|l| line_cells(l)).as_deref(), card.cols.start);
    let end = lines.iter().map(|l| box_cells(line_cells(l).as_deref(), &card.cols).1).max().unwrap_or(start + card.cols.len());
    (start, end.max(start + card.cols.len()))
}

/// A page being laid out: rows of cells, plus what's drawn over them.
pub struct Grid {
    lines: Vec<Vec<char>>,
    spans: Vec<Span>,
    rules: Vec<(usize, Range<usize>)>,
    pub panels: Vec<(usize, Range<usize>)>,
    pub focus: Option<(usize, Range<usize>)>,
    pub popup: Option<Popup>,
    pub images: Vec<PageImage>,
    pub underlines: Vec<(usize, Range<usize>, Role)>,
    pub cards: Vec<Card>,
    pub chips: Vec<(usize, Range<usize>, Chip)>,
    pub graph: Vec<GraphStrip>,
}

/// `path` inside `root`, as `src/main.rs`: matched however either was
/// spelt -- `\\?\`, the drive letter's case, `/` or `\` -- since a root is
/// kept canonical and a buffer's path as it was opened.
pub fn path_within(root: &Path, path: &Path) -> Option<String> {
    if let Ok(rest) = path.strip_prefix(root) {
        return Some(rest.to_string_lossy().replace('\\', "/"));
    }
    let plain = |p: &Path| fenix_lsp::normalize(p.to_path_buf()).to_string_lossy().replace('/', "\\");
    let (root, path) = (plain(root), plain(path));
    let root = format!("{}\\", root.trim_end_matches('\\'));
    let inside = path.len() > root.len() && path.is_char_boundary(root.len()) && path[..root.len()].eq_ignore_ascii_case(&root);
    inside.then(|| path[root.len()..].replace('\\', "/"))
}

/// `path` whole, its home folder as `~`: a file outside the project.
pub fn home_relative(path: &Path) -> String {
    // A file on a remote machine, as that machine names it.
    if fenix_vfs::is_remote(path) {
        return fenix_vfs::display(path);
    }
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from);
    match home.and_then(|h| path_within(&h, path)) {
        Some(rest) => format!("~/{rest}"),
        None => path.display().to_string(),
    }
}

/// How many cells `s` takes, a wide character two (`char_cells`).
pub fn str_cells(s: &str) -> usize {
    s.chars().map(char_cells).sum()
}

/// `text` as one row: each run of whitespace -- a newline, a tab, a
/// `\r` -- one space, and any other control character too. For a title,
/// a name or an entry that's to be read on a line of its own, before
/// `fit` cuts it.
pub fn one_line(text: &str) -> String {
    text.split(|c: char| c.is_whitespace() || c.is_control()).filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" ")
}

/// Cuts `s` to `max` cells (a wide character takes two), ending in "…"
/// when it had to.
pub fn fit(s: &str, max: usize) -> String {
    if str_cells(s) <= max {
        return s.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut used = 0;
    for c in s.chars() {
        let w = char_cells(c);
        if used + w > max - 1 {
            break;
        }
        used += w;
        out.push(c);
    }
    out.push('…');
    out
}

/// `s` in at most `max` cells keeping its end, `…` marking what went
/// off the front -- for a field being typed, where the end is where the
/// typing is.
pub fn fit_tail(s: &str, max: usize) -> String {
    if str_cells(s) <= max {
        return s.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let mut kept: Vec<char> = Vec::new();
    let mut used = 0;
    for c in s.chars().rev() {
        let w = char_cells(c);
        if used + w > max - 1 {
            break;
        }
        used += w;
        kept.push(c);
    }
    std::iter::once('…').chain(kept.into_iter().rev()).collect()
}

/// A path in at most `max` columns, cut in the middle with `…`: its
/// start, and its end -- the last folder, the one being checked, whole
/// when there's room for it.
pub fn fit_path(s: &str, max: usize) -> String {
    let n = s.chars().count();
    if n <= max {
        return s.to_string();
    }
    if max < 8 {
        return fit_tail(s, max);
    }
    let keep = max - 1;
    let last = s.trim_end_matches(['/', '\\']).rsplit(['/', '\\']).next().map_or(0, |l| l.chars().count() + 1);
    let back = last.max(keep * 2 / 3).min(keep);
    let front = keep - back;
    let mut out: String = s.chars().take(front).collect();
    out.push('…');
    out.extend(s.chars().skip(n - back));
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
        Key::DeleteToStart => {
            text.replace_range(..byte(at), "");
            *caret = 0;
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
    if n - at < max {
        return std::iter::once('…').chain(all[n - (max - 1)..].iter().copied()).collect();
    }
    let start = at.saturating_sub(max / 2);
    if start == 0 {
        return all[..max - 1].iter().copied().chain(std::iter::once('…')).collect();
    }
    std::iter::once('…').chain(all[start + 1..start + max - 1].iter().copied()).chain(std::iter::once('…')).collect()
}

/// `text` broken into pieces of at most `width` cells, wherever it has
/// to be: a word too long for a line (a path).
fn chunks(text: &str, width: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut piece = String::new();
    let mut used = 0;
    for c in text.chars() {
        let w = char_cells(c);
        if used + w > width && !piece.is_empty() {
            out.push(std::mem::take(&mut piece));
            used = 0;
        }
        used += w;
        piece.push(c);
    }
    if !piece.is_empty() {
        out.push(piece);
    }
    out
}

/// `text` wrapped at word boundaries to lines of at most `width` cells;
/// a word longer than that (a path) is broken wherever it has to be.
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut lines = Vec::new();
    let mut line = String::new();
    let mut words: Vec<String> = Vec::new();
    for word in text.split(|c: char| c.is_whitespace() || c.is_control()).filter(|w| !w.is_empty()) {
        words.extend(chunks(word, width));
    }
    for word in &words {
        if !line.is_empty() && str_cells(&line) + 1 + str_cells(word) > width {
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
/// width, centred and capped the way Home's is. A pane under 40 columns
/// gets a cell of margin a side rather than two -- never a frame wider
/// than it, which would run every page past its edge.
pub fn frame(cols: usize, max_width: usize) -> (usize, usize) {
    let margin = if cols >= 40 { 4 } else { 2 };
    let width = cols.saturating_sub(margin).min(max_width);
    (cols.saturating_sub(width) / 2, width)
}

impl Default for Grid {
    fn default() -> Self {
        Self::new()
    }
}

impl Grid {
    pub fn new() -> Self {
        Grid { lines: Vec::new(), spans: Vec::new(), rules: Vec::new(), panels: Vec::new(), focus: None, popup: None, images: Vec::new(), underlines: Vec::new(), cards: Vec::new(), chips: Vec::new(), graph: Vec::new() }
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
        self.images.extend(page.images.iter().map(|i| PageImage { line: i.line + line, col: i.col + col, ..i.clone() }));
        self.underlines.extend(page.underlines.iter().map(|(l, c, r)| (l + line, shift(c), *r)));
        self.cards.extend(page.cards.iter().map(|c| Card { lines: c.lines.start + line..c.lines.end + line, cols: shift(&c.cols), look: c.look }));
        self.chips.extend(page.chips.iter().map(|(l, c, chip)| (l + line, shift(c), *chip)));
        self.graph.extend(page.graph.iter().map(|g| GraphStrip { line: g.line + line, cols: shift(&g.cols), ..g.clone() }));
    }

    /// Draws a commit graph's row over cells `cols` of `line`, which are
    /// left blank.
    pub fn graph(&mut self, line: usize, cols: Range<usize>, lanes: usize, marks: Vec<GraphMark>) {
        if marks.is_empty() || cols.is_empty() {
            return;
        }
        if self.lines.len() <= line {
            self.lines.resize(line + 1, Vec::new());
        }
        self.graph.push(GraphStrip { line, cols, lanes, marks });
    }

    /// Sets `lines` off as a card across `cols`.
    pub fn card(&mut self, lines: Range<usize>, cols: Range<usize>, look: CardLook) {
        if lines.is_empty() {
            return;
        }
        if self.lines.len() < lines.end {
            self.lines.resize(lines.end, Vec::new());
        }
        self.cards.push(Card { lines, cols, look });
    }

    /// Sets `lines` off as a card behind every card already made: a list's
    /// card, drawn under the card of the row selected in it.
    pub fn card_behind(&mut self, lines: Range<usize>, cols: Range<usize>, look: CardLook) {
        if lines.is_empty() {
            return;
        }
        if self.lines.len() < lines.end {
            self.lines.resize(lines.end, Vec::new());
        }
        self.cards.insert(0, Card { lines, cols, look });
    }

    /// Writes `text` at (`line`, `col`) on a chip; returns the column
    /// after it.
    pub fn chip(&mut self, line: usize, col: usize, text: &str, role: Role, chip: Chip) -> usize {
        let end = self.put(line, col, text, role);
        if end > col {
            self.chips.push((line, col..end, chip));
        }
        end
    }

    /// A chip that keeps short of `end`: `text` (padded " like this ")
    /// cut to fit, or left out when even a few letters wouldn't -- for a
    /// row of chips in a narrow pane. Returns the column after it, or
    /// `col` when it's left out.
    pub fn chip_within(&mut self, line: usize, col: usize, end: usize, text: &str, role: Role, chip: Chip) -> usize {
        let room = end.saturating_sub(col);
        let inner = text.trim();
        if text.chars().count() <= room {
            return self.chip(line, col, text, role, chip);
        }
        if room < 6 {
            return col;
        }
        self.chip(line, col, &format!(" {} ", fit(inner, room - 2)), role, chip)
    }

    /// A count beside a title, on a quiet badge; returns the column after.
    pub fn badge(&mut self, line: usize, col: usize, count: usize) -> usize {
        self.chip(line, col, &format!(" {count} "), Role::Muted, Chip::Badge)
    }

    /// A box you type in across `cols` of `line`: `text`, or the
    /// `placeholder` quieter while it's empty, outlined in the accent
    /// while `focused`.
    pub fn input_box(&mut self, line: usize, cols: Range<usize>, text: &str, placeholder: &str, focused: bool) {
        let width = cols.len().saturating_sub(2);
        if text.is_empty() {
            self.put(line, cols.start + 1, &fit(placeholder, width), Role::Muted);
        } else {
            self.put(line, cols.start + 1, &fit(text, width), Role::Text);
        }
        self.card(line..line + 1, cols, if focused { CardLook::FieldFocused } else { CardLook::Field });
    }

    /// An option shown on or off (`Aa`, `.*`, a scope); returns the column after.
    pub fn toggle(&mut self, line: usize, col: usize, label: &str, on: bool) -> usize {
        self.chip(line, col, &format!(" {label} "), if on { Role::Accent } else { Role::Muted }, Chip::Toggle(on))
    }

    /// A switch, three cells wide; returns the column after.
    pub fn switch(&mut self, line: usize, col: usize, on: bool) -> usize {
        self.write(line, col, "   ");
        self.chips.push((line, col..col + 3, Chip::Switch(on)));
        col + 3
    }

    /// A number you step with h and l: `‹ 4 ›`; returns the column after.
    pub fn stepper(&mut self, line: usize, col: usize, value: &str) -> usize {
        let x = self.put(line, col, "‹ ", Role::Muted);
        let x = self.put(line, x, value, Role::Accent);
        let end = self.put(line, x, " ›", Role::Muted);
        self.chips.push((line, col..end, Chip::Value));
        end
    }

    /// A value on a field: a setting's text, a path.
    pub fn value(&mut self, line: usize, col: usize, text: &str, role: Role) -> usize {
        self.chip(line, col, &format!(" {text} "), role, Chip::Value)
    }

    /// Leaves `rows` lines for a picture `cols` wide at (`line`, `col`).
    pub fn image(&mut self, line: usize, col: usize, rows: usize, cols: usize, key: ImageKey) {
        if self.lines.len() < line + rows {
            self.lines.resize(line + rows, Vec::new());
        }
        self.images.push(PageImage { line, col, rows, cols, key, view: None });
    }

    /// Underlines `cols` of `line` in the accent.
    pub fn underline(&mut self, line: usize, cols: Range<usize>) {
        self.underlines.push((line, cols, Role::Accent));
    }

    /// Tints `cols` of `line` as a panel.
    pub fn panel(&mut self, line: usize, cols: Range<usize>) {
        if self.lines.len() <= line {
            self.lines.resize(line + 1, Vec::new());
        }
        self.panels.push((line, cols));
    }

    /// Makes the page at least `lines` long, blank below what's written.
    pub fn reserve(&mut self, lines: usize) {
        if self.lines.len() < lines {
            self.lines.resize(lines, Vec::new());
        }
    }

    /// How many lines have been written.
    pub fn height(&self) -> usize {
        self.lines.len()
    }

    /// Writes `text` at (`line`, `col`) without colouring it. A row is
    /// one line: a control character (`\n`, `\r`, a tab) is stored as a
    /// space, one cell for one, so no text can push the rows under it
    /// off what's drawn for them and the columns worked out still hold.
    fn write(&mut self, line: usize, col: usize, text: &str) -> usize {
        if self.lines.len() <= line {
            self.lines.resize(line + 1, Vec::new());
        }
        let len = text.chars().count();
        if len == 0 {
            return 0;
        }
        let row = &mut self.lines[line];
        if row.len() < col + len {
            row.resize(col + len, ' ');
        }
        for (i, c) in text.chars().enumerate() {
            row[col + i] = if c.is_control() { ' ' } else { c };
        }
        len
    }

    /// Writes `text` at (`line`, `col`); returns the column after it.
    pub fn put(&mut self, line: usize, col: usize, text: &str, role: Role) -> usize {
        let len = self.write(line, col, text);
        if len == 0 {
            return col;
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

    /// A section heading: "FILES · 8 ─────────", cut to `width`.
    pub fn heading(&mut self, line: usize, x: usize, width: usize, text: &str) {
        let end = self.put(line, x, &fit(&text.to_uppercase(), width), Role::Muted);
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
            x = self.chip(foot + 1, x, key, Role::Accent, Chip::Key) + 1;
            x = self.put(foot + 1, x, label, Role::Muted) + 3;
        }
    }

    pub fn finish(self) -> Page {
        let text = self.lines.iter().map(|row| row.iter().collect::<String>().trim_end().to_string()).collect::<Vec<_>>().join("\n");
        // What's addressed by row is the line of that number.
        debug_assert!(self.lines.is_empty() || text.split('\n').count() == self.lines.len(), "a row of the page holds a line break");
        Page { text, spans: self.spans, rules: self.rules, focus: self.focus, panels: self.panels, popup: self.popup, images: self.images, underlines: self.underlines, cards: self.cards, chips: self.chips, graph: self.graph, ..Page::default() }
    }
}

/// What of `page` runs past `cols` cells: text, a chip or a card that
/// would be cut at the pane's edge. Empty when it all fits -- for pages'
/// tests to check they keep inside a narrow window.
#[cfg(test)]
pub fn overflow(page: &Page, cols: usize) -> Vec<String> {
    let mut out = Vec::new();
    for (n, line) in page.text.lines().enumerate() {
        let width = str_cells(line.trim_end());
        if width > cols {
            out.push(format!("line {n} is {width} wide: {:?}", line.trim_end()));
        }
    }
    // Text written over text: the later wins, and the line reads as a
    // jumble of the two.
    let mut by_line: Vec<&Span> = page.spans.iter().filter(|s| !s.cols.is_empty()).collect();
    by_line.sort_by_key(|s| (s.line, s.cols.start));
    for pair in by_line.windows(2) {
        if pair[0].line == pair[1].line && pair[1].cols.start < pair[0].cols.end {
            let line = page.text.lines().nth(pair[0].line).unwrap_or_default().trim_end();
            out.push(format!("text over text on line {} at {:?} and {:?}: {line:?}", pair[0].line, pair[0].cols, pair[1].cols));
        }
    }
    // In cells, as they're drawn.
    let map = |line: usize| page.cells(line);
    for (line, range, _) in &page.chips {
        let end = cell_of(map(*line).as_deref(), range.end);
        if end > cols {
            out.push(format!("a chip on line {line} ends at {end}"));
        }
    }
    for card in &page.cards {
        let (_, end) = card_cells(page, card);
        if end > cols {
            out.push(format!("a card on lines {:?} ends at {end}", card.lines));
        }
    }
    for (line, range) in page.panels.iter().chain(page.focus.iter()).chain(page.focuses.iter()) {
        let (_, end) = box_cells(map(*line).as_deref(), range);
        if end > cols {
            out.push(format!("a tint on line {line} ends at {end}"));
        }
    }
    out
}

/// Held by every test that sets the process's project kinds
/// (`fenix_project::set_extension_kinds`) or asks which are declared:
/// tests run side by side, and the kinds are global.
#[cfg(test)]
pub(crate) fn project_kinds_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A wide character takes two cells, as `ui-kit` lays it out: columns
    /// (characters) go to cells and back, either half of one wide
    /// character being it; a line of narrow ones is cells as it is.
    #[test]
    fn a_wide_character_takes_two_cells() {
        let page = Page { text: "ab\n日本 ok\n😀x".into(), ..Page::default() };
        assert_eq!(page.cells(0), None);
        let cells = page.cells(1).unwrap();
        assert_eq!(cells, [0, 2, 4, 5, 6, 7]);
        assert_eq!((cell_of(Some(&cells), 2), cell_of(Some(&cells), 5), cell_of(Some(&cells), 7)), (4, 7, 9), "past the end, a cell a character");
        assert_eq!((col_of(Some(&cells), 0), col_of(Some(&cells), 1), col_of(Some(&cells), 3), col_of(Some(&cells), 4)), (0, 0, 1, 2));
        assert_eq!(col_of(Some(&cells), 9), 7);
        assert_eq!(page.cells(2).unwrap(), [0, 2, 3]);
        assert_eq!((cell_of(None, 3), col_of(None, 3)), (3, 3));
    }

    /// A row is one line: whatever control character is put or written
    /// into it is a space, one for one, and the page has as many lines
    /// as rows.
    #[test]
    fn a_row_holds_no_line_break_whatever_it_is_given() {
        let controls: String = (0u32..0x20).chain(0x7f..0xa0).filter_map(char::from_u32).collect();
        let mut g = Grid::new();
        let end = g.put(0, 2, &format!("a{controls}b"), Role::Text);
        g.write(1, 0, "x\ny\r\nz\t");
        g.chip(2, 0, "\nk\n", Role::Accent, Chip::Key);
        let page = g.finish();
        assert_eq!(page.text.split('\n').count(), 3, "{:?}", page.text);
        let lines: Vec<&str> = page.text.split('\n').collect();
        assert_eq!(lines[0], format!("  a{}b", " ".repeat(controls.chars().count())));
        assert_eq!(end, 2 + controls.chars().count() + 2, "a cell for each character");
        assert_eq!(lines[1], "x y  z");
        assert_eq!(page.chips, vec![(2, 0..3, Chip::Key)]);
        assert!(page.text.chars().all(|c| c == '\n' || !c.is_control()));
    }

    /// `fit` and `wrap` count cells: a wide character takes two.
    #[test]
    fn fit_and_wrap_count_cells() {
        assert_eq!(fit("日本語です", 6), "日本…");
        assert_eq!(str_cells(&fit("日本語です", 6)), 5);
        assert_eq!(fit("日本", 4), "日本");
        assert_eq!(fit_tail("日本語です", 6), "…です");
        assert!(wrap(&"日本語 ".repeat(10), 8).iter().all(|l| str_cells(l) <= 8));
        assert!(wrap(&"😀".repeat(30), 7).iter().all(|l| str_cells(l) <= 7));
        assert_eq!(one_line("cat <<EOF\n# Plan\r\n\t1. a\u{1b}x"), "cat <<EOF # Plan 1. a x");
    }

    /// A box across a line with wide characters is as many cells as it has
    /// columns, or to the end of the text in it.
    #[test]
    fn a_box_over_wide_text_keeps_its_width() {
        let cells = line_cells("  日本 ok").unwrap();
        assert_eq!(box_cells(Some(&cells), &(0..20)), (0, 20), "text fitted to cells: the box is its columns");
        assert_eq!(box_cells(Some(&cells), &(0..4)), (0, 6), "text that wasn't: the box holds it");
        assert_eq!(box_cells(None, &(3..9)), (3, 9));
    }

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
        assert_eq!(page.chips.first(), Some(&(3, 0..5, Chip::Key)), "each key on its cap");
    }

    /// The widgets write their text and the box they're drawn on.
    #[test]
    fn widgets_are_text_on_their_boxes() {
        let mut g = Grid::new();
        g.input_box(0, 0..21, "", "Search 133 settings", false);
        let x = g.toggle(1, 0, "Aa", false);
        let x = g.toggle(1, x + 1, "ab", true);
        g.switch(2, 0, true);
        g.stepper(2, 6, "4");
        g.badge(3, 0, 28);
        let x2 = g.value(3, 6, "pwsh -NoLogo", Role::Text);
        let page = g.finish();
        let lines: Vec<&str> = page.text.lines().collect();
        assert_eq!(lines[0], " Search 133 settings");
        assert_eq!(lines[1], " Aa   ab");
        assert_eq!(lines[2].trim_end(), "      ‹ 4 ›");
        assert_eq!(lines[3], " 28    pwsh -NoLogo");
        assert_eq!(page.cards[0].look, CardLook::Field);
        assert!(page.chips.contains(&(1, 0..4, Chip::Toggle(false))) && page.chips.contains(&(1, 5..9, Chip::Toggle(true))));
        assert!(page.chips.contains(&(2, 0..3, Chip::Switch(true))));
        assert!(page.chips.contains(&(2, 6..11, Chip::Value)));
        assert_eq!((x, x2), (9, 20), "each returns the column after it");
    }

    /// A card is kept, and moved with the page it's in.
    #[test]
    fn cards_and_chips_move_with_an_embedded_page() {
        let mut inner = Grid::new();
        inner.put(0, 0, "a", Role::Text);
        inner.card(0..2, 0..10, CardLook::Ask);
        inner.chip(1, 2, "y", Role::Accent, Chip::Chosen);
        let inner = inner.finish();
        let mut g = Grid::new();
        g.embed(&inner, 5, 3, false);
        let page = g.finish();
        assert_eq!(page.cards, vec![Card { lines: 5..7, cols: 3..13, look: CardLook::Ask }]);
        assert_eq!(page.chips, vec![(6, 5..6, Chip::Chosen)]);
        assert!(page.text.lines().count() >= 7, "the card's lines are the page's");
    }

    #[test]
    fn fit_ends_in_an_ellipsis_only_when_it_cuts() {
        assert_eq!(fit("abc", 3), "abc");
        assert_eq!(fit("abcd", 3), "ab…");
        assert_eq!(fit("abcd", 0), "");
    }

    #[test]
    fn fit_path_cuts_the_middle_and_keeps_the_last_folder() {
        let long = r"C:\Users\thoma\AppData\Local\Temp\claude\C--Users-thoma-Development-fenix\dd8aac7e-b3fa-45cf-88aa\ledger";
        let fitted = fit_path(long, 40);
        assert_eq!(fitted.chars().count(), 40, "{fitted}");
        assert!(fitted.starts_with(r"C:\Users") && fitted.ends_with(r"\ledger") && fitted.contains('…'), "{fitted}");
        assert_eq!(fit_path("/a/b", 10), "/a/b");
        // A last folder longer than the room: its end.
        assert!(fit_path("/x/averyveryverylongname", 12).ends_with("longname"));
    }

    #[test]
    fn fit_tail_keeps_the_end_and_marks_the_front() {
        assert_eq!(fit_tail("abc", 3), "abc");
        assert_eq!(fit_tail("abcd", 3), "…cd");
        assert_eq!(fit_tail("abcd", 0), "");
    }

    #[test]
    fn ctrl_u_in_a_field_deletes_everything_before_the_caret() {
        let mut text = "cargo build".to_string();
        let mut caret = 6;
        assert!(edit_line(&mut text, &mut caret, Key::DeleteToStart));
        assert_eq!((text.as_str(), caret), ("build", 0));
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
