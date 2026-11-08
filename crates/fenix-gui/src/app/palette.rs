//! The palette: every picker (`SPC SPC`, `SPC :`, buffers, symbols,
//! projects, themes, TODOs ...) and the leader's menu, drawn as one
//! rounded card floating over the editor -- which stays where it was,
//! dimmed, rather than being replaced by a list. The card has an input
//! row with chips (what's being picked from, how many match), a list --
//! sections, icons, the characters the query matched in the accent, a
//! second, quieter line where a row has one, keys on caps -- a pane
//! beside it (a file's first lines, a command's details) when the window
//! is wide enough, and the keys along its foot.
//!
//! And its smaller sibling, the hint bar: Vim's own prefixes (`g`, `z`,
//! `[`, `]`, an operator waiting for its motion) show what comes next in
//! a strip growing out of the modeline, which never covers the cursor's
//! line.
//!
//! Everything is laid out on the text grid -- one monospace buffer, a
//! cell a character, an icon one cell -- so the boxes drawn under the
//! text (the input, chips, caps, the selected row, the side pane) are
//! placed by cell, the same way the pages' cards and caps are.

use super::*;
use crate::theme::{mix, rgba_of};

/// The card's text inset from its edge.
pub(super) const PAD_X: f32 = 14.0;
pub(super) const PAD_Y: f32 = 14.0;
/// Room left between the card and the window's sides, its top, and the
/// modeline.
const MARGIN_X: f32 = 48.0;
const TOP: f32 = 44.0;
const BOTTOM: f32 = 30.0;
/// The widest the card gets.
const MAX_WIDTH: f32 = 1040.0;
/// Below this many columns the side pane goes: the list needs the room.
const SIDE_MIN_COLS: usize = 88;
/// The most of a file the preview reads, and shows.
const PREVIEW_BYTES: u64 = 64 * 1024;
const PREVIEW_LINES: usize = 60;

/// A box drawn under the card's text, in cells from its text origin.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct CardBox {
    pub row: usize,
    pub rows: usize,
    pub col: usize,
    pub cols: usize,
    pub kind: BoxKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BoxKind {
    /// The query's row.
    Input,
    /// A chip on the input row: in the accent when `accent`.
    Chip { accent: bool },
    /// A count beside a section's name.
    Badge,
    /// A key's cap.
    Key,
    /// The row the cursor is on.
    Selected,
    /// The pane beside the list.
    Side,
}

/// How the card sits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CardStyle {
    /// Floating over the dimmed editor.
    Floating,
    /// Growing out of the modeline (`grows_up`), or hanging under the
    /// tabs when the cursor is low: the hint bar.
    Bar { grows_up: bool },
}

/// A card laid out: where it is, its text, and its boxes.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Card {
    pub rect: fenix_window::Rect,
    pub spans: RowSpans,
    pub boxes: Vec<CardBox>,
    pub style: CardStyle,
    /// Pages of a menu too long for the card, and the one shown.
    pub page: usize,
    pub pages: usize,
}

/// One row of a palette's list.
#[derive(Debug, Clone, Default, PartialEq)]
pub(super) struct Row {
    /// A section starts here: its name and how many it holds.
    pub section: Option<(String, Option<usize>)>,
    pub icon: Option<(char, glyphon::Color)>,
    /// Before the title, in the groups' colour, then `›`: a command's area.
    pub area: Option<String>,
    pub area_hits: Vec<usize>,
    pub title: String,
    pub title_hits: Vec<usize>,
    /// After the title, quieter: a file's folder.
    pub aside: String,
    pub aside_hits: Vec<usize>,
    /// A second line, quieter: a command's description.
    pub detail: Option<String>,
    /// Keys on caps at the row's right (`SPC g g`).
    pub keys: Option<String>,
    /// Quiet text at the row's right, when there are no keys.
    pub right: String,
    pub selected: bool,
    /// Can't be used here: drawn faded.
    pub dimmed: bool,
}

impl Row {
    fn lines(&self) -> usize {
        usize::from(self.section.is_some()) + 1 + usize::from(self.detail.is_some())
    }
}

/// What the pane beside the list shows.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Side {
    /// A file's first lines.
    Preview { icon: Option<(char, glyphon::Color)>, title: String, about: String, lines: Vec<String> },
    /// A command's details: its name, what it does, and facts about it
    /// -- `(label, value, keys)`: a value on caps when `keys`.
    Details { title: String, text: String, facts: Vec<(String, String, bool)> },
}

/// What's laid out: a picker's or the leader's.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Body {
    List(Vec<Row>),
    /// The leader's menu: recent chords, then sections of entries in
    /// columns -- `(key, label, group, count)`.
    Grid { recent: Vec<(String, String)>, sections: Vec<(String, Vec<(String, String, bool, usize)>)> },
}

/// A palette to lay out.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct View {
    /// The input row's lead: `›`, `:`, or a key's cap (`SPC`).
    pub prompt: String,
    pub prompt_is_key: bool,
    pub query: String,
    /// Shown, quieter, while there's no query.
    pub placeholder: String,
    /// On the input row's right: `(text, accent)`.
    pub chips: Vec<(String, bool)>,
    pub body: Body,
    pub side: Option<Side>,
    /// The keys along the foot: `(keys, what)`.
    pub footer: Vec<(String, String)>,
    /// What an empty list says.
    pub empty: String,
}

/// How many rows a card's list gets in `area` (the window above the
/// modeline): what's left between the input row and the foot.
pub(super) fn list_lines(area: fenix_window::Rect, line_height: f32) -> usize {
    let h = area.h - TOP - BOTTOM - PAD_Y * 2.0;
    ((h / line_height).floor() as usize).saturating_sub(4).max(3)
}

/// `text` cut to `max` characters, ending in "…" when it had to be.
fn fit(text: &str, max: usize) -> String {
    crate::page::fit(text, max)
}

/// One row of the card's text being written: spans placed by column,
/// padded with spaces between them.
#[derive(Default)]
struct Line {
    spans: Vec<(String, glyphon::Color, bool)>,
    col: usize,
}

impl Line {
    fn put(&mut self, col: usize, text: &str, color: glyphon::Color, icon: bool) -> usize {
        if col > self.col {
            self.spans.push((" ".repeat(col - self.col), color, false));
            self.col = col;
        }
        if !text.is_empty() {
            self.spans.push((text.to_string(), color, icon));
            self.col += text.chars().count();
        }
        self.col
    }

    /// `text` at `col`, the characters at `hits` in `hit`, the rest in
    /// `color`.
    fn put_hits(&mut self, col: usize, text: &str, hits: &[usize], color: glyphon::Color, hit: glyphon::Color) -> usize {
        let mut at = col;
        let mut run = String::new();
        let mut run_hit = false;
        for (i, ch) in text.chars().enumerate() {
            let is_hit = hits.contains(&i);
            if is_hit != run_hit && !run.is_empty() {
                at = self.put(at, &run, if run_hit { hit } else { color }, false);
                run.clear();
            }
            run_hit = is_hit;
            run.push(ch);
        }
        if !run.is_empty() {
            at = self.put(at, &run, if run_hit { hit } else { color }, false);
        }
        at.max(col)
    }
}

/// Keys as caps, `SPC g g` -> `SPC` `g` `g`, written at `col` with a cell
/// between caps; their boxes go on `row`. Returns the column after.
fn put_keys(line: &mut Line, boxes: &mut Vec<CardBox>, row: usize, col: usize, keys: &str, color: glyphon::Color) -> usize {
    let mut at = col;
    for (i, key) in keys.split(' ').filter(|k| !k.is_empty()).enumerate() {
        if i > 0 {
            at += 1;
        }
        let n = key.chars().count();
        boxes.push(CardBox { row, rows: 1, col: at, cols: n, kind: BoxKind::Key });
        at = line.put(at, key, color, false);
    }
    at
}

/// How many columns `keys` takes as caps.
fn keys_width(keys: &str) -> usize {
    let words: Vec<&str> = keys.split(' ').filter(|k| !k.is_empty()).collect();
    words.iter().map(|k| k.chars().count()).sum::<usize>() + words.len().saturating_sub(1)
}

/// The palette `view` laid out in `area` (the window above the modeline),
/// with the theme's colours, its cells `char_width` by `line_height`.
pub(super) fn layout(view: &View, theme: &Theme, area: fenix_window::Rect, char_width: f32, line_height: f32, page: usize) -> Card {
    let width = if area.w < 560.0 { (area.w - 12.0).max(120.0) } else { (area.w - MARGIN_X * 2.0).min(MAX_WIDTH) };
    let cols = (((width - PAD_X * 2.0) / char_width).floor() as usize).max(20);
    let side = view.side.as_ref().filter(|_| cols >= SIDE_MIN_COLS);
    let left = if side.is_some() { (cols * 44 / 100).clamp(36, 64) } else { cols };
    let side_col = left + 2;
    let avail = list_lines(area, line_height);
    let (fg, quiet, accent, group) = (theme.fg, theme.gutter_fg, theme.caret_text, theme.syntax_keyword);
    let mut boxes = Vec::new();
    let mut lines: Vec<Line> = Vec::new();

    // The input row: the prompt, the query or what to type, the chips.
    let mut input = Line::default();
    let mut at = 1;
    if view.prompt_is_key {
        at = put_keys(&mut input, &mut boxes, 0, at, &view.prompt, accent) + 2;
    } else {
        at = input.put(at, &view.prompt, accent, false) + 1;
    }
    let chips_width: usize = view.chips.iter().map(|(t, _)| t.chars().count() + 3).sum();
    let room = cols.saturating_sub(at + chips_width + 2);
    if view.query.is_empty() {
        // The caret only where something's typed: a menu takes keys, not text.
        if !view.prompt_is_key {
            input.put(at, "▏", accent, false);
        }
        input.put(at + 1, &fit(&view.placeholder, room.saturating_sub(1)), quiet, false);
    } else {
        let shown = crate::page::fit_tail(&view.query, room.saturating_sub(1));
        let end = input.put(at, &shown, fg, false);
        input.put(end, "▏", accent, false);
    }
    let mut chip_at = cols.saturating_sub(chips_width) + 1;
    for (text, is_accent) in &view.chips {
        let n = text.chars().count();
        boxes.push(CardBox { row: 0, rows: 1, col: chip_at, cols: n, kind: BoxKind::Chip { accent: *is_accent } });
        input.put(chip_at, text, if *is_accent { accent } else { quiet }, false);
        chip_at += n + 3;
    }
    boxes.push(CardBox { row: 0, rows: 1, col: 0, cols, kind: BoxKind::Input });
    lines.push(input);
    lines.push(Line::default());

    // The body.
    let first = lines.len();
    let mut body: Vec<Line> = Vec::new();
    let mut pages = 1;
    let mut page = page;
    match &view.body {
        Body::List(rows) => {
            for row in rows {
                if body.len() + row.lines() > avail {
                    break;
                }
                if let Some((name, count)) = &row.section {
                    let mut l = Line::default();
                    let end = l.put(1, &name.to_uppercase(), quiet, false);
                    if let Some(n) = count {
                        let text = n.to_string();
                        boxes.push(CardBox { row: first + body.len(), rows: 1, col: end + 1, cols: text.chars().count(), kind: BoxKind::Badge });
                        l.put(end + 1, &text, quiet, false);
                    }
                    body.push(l);
                }
                let r = first + body.len();
                let mut l = Line::default();
                let (title_color, hit) = if row.dimmed { (quiet, quiet) } else { (fg, accent) };
                let mut c = 1;
                if let Some((icon, color)) = row.icon {
                    // An icon draws wider than its cell: a cell more after it.
                    l.put(c, &icon.to_string(), if row.dimmed { quiet } else { color }, true);
                    c += 3;
                }
                let text_col = c;
                let right_w = match &row.keys {
                    Some(k) => keys_width(k),
                    None => row.right.chars().count(),
                };
                let room = left.saturating_sub(c + right_w + 3);
                if let Some(area_name) = &row.area {
                    c = l.put_hits(c, area_name, &row.area_hits, if row.dimmed { quiet } else { group }, hit);
                    c = l.put(c, " › ", quiet, false);
                }
                let used = c - text_col;
                let title = fit(&row.title, room.saturating_sub(used));
                c = l.put_hits(c, &title, &row.title_hits, title_color, hit);
                if !row.aside.is_empty() && c + 3 < text_col + room {
                    let aside = fit(&row.aside, (text_col + room).saturating_sub(c + 2));
                    l.put_hits(c + 2, &aside, &row.aside_hits, quiet, hit);
                }
                let right_col = left.saturating_sub(right_w + 1);
                match &row.keys {
                    Some(k) => {
                        put_keys(&mut l, &mut boxes, r, right_col, k, if row.dimmed { quiet } else { accent });
                    }
                    None => {
                        l.put(right_col, &row.right, quiet, false);
                    }
                }
                body.push(l);
                if let Some(detail) = &row.detail {
                    let mut d = Line::default();
                    d.put(text_col, &fit(detail, left.saturating_sub(text_col + 1)), quiet, false);
                    body.push(d);
                }
                if row.selected {
                    boxes.push(CardBox { row: r, rows: 1 + usize::from(row.detail.is_some()), col: 0, cols: left, kind: BoxKind::Selected });
                }
            }
            if rows.is_empty() {
                let mut l = Line::default();
                l.put(1, &fit(&view.empty, left.saturating_sub(2)), quiet, false);
                body.push(l);
            }
        }
        Body::Grid { recent, sections } => {
            // Everything as lines first, then the page that fits.
            let mut all: Vec<(Line, Vec<CardBox>)> = Vec::new();
            let header = |name: &str| {
                let mut l = Line::default();
                l.put(1, &name.to_uppercase(), quiet, false);
                (l, Vec::new())
            };
            if !recent.is_empty() {
                all.push(header("recent"));
                let mut l = Line::default();
                let mut b = Vec::new();
                let mut c = 1;
                for (keys, label) in recent {
                    let w = keys_width(keys) + 1 + label.chars().count();
                    if c + w + 2 > cols {
                        break;
                    }
                    b.push(CardBox { row: 0, rows: 1, col: c.saturating_sub(1), cols: w + 2, kind: BoxKind::Chip { accent: false } });
                    let end = put_keys(&mut l, &mut b, 0, c, keys, accent);
                    c = l.put(end + 1, label, fg, false) + 4;
                }
                all.push((l, b));
            }
            let ncols = if cols >= 70 { 2 } else { 1 };
            let colw = cols.saturating_sub(2) / ncols;
            for (name, entries) in sections {
                all.push(header(name));
                for chunk in entries.chunks(ncols) {
                    let mut l = Line::default();
                    let mut b = Vec::new();
                    for (i, (key, label, is_group, count)) in chunk.iter().enumerate() {
                        let c0 = 1 + i * colw;
                        let end = put_keys(&mut l, &mut b, 0, c0, key, accent);
                        let after = c0 + keys_width(key).max(3) + 2;
                        let count_text = if *is_group { count.to_string() } else { String::new() };
                        let room = colw.saturating_sub(after - c0 + count_text.chars().count() + 3);
                        let e = l.put(after.max(end + 2), &fit(label, room), if *is_group { group } else { fg }, false);
                        if *is_group {
                            b.push(CardBox { row: 0, rows: 1, col: e + 1, cols: count_text.chars().count(), kind: BoxKind::Badge });
                            l.put(e + 1, &count_text, quiet, false);
                        }
                    }
                    all.push((l, b));
                }
            }
            pages = all.len().div_ceil(avail.max(1)).max(1);
            page = page.min(pages - 1);
            for (l, b) in all.into_iter().skip(page * avail).take(avail) {
                let r = first + body.len();
                boxes.extend(b.into_iter().map(|bx| CardBox { row: r, ..bx }));
                body.push(l);
            }
        }
    }

    // The side pane, beside the list.
    let side_rows = if side.is_some() { body.len().max(avail.min(18)) } else { body.len() };
    while body.len() < side_rows {
        body.push(Line::default());
    }
    if let Some(side) = side {
        let width = cols.saturating_sub(side_col + 2);
        let mut side_lines: Vec<Vec<(usize, String, glyphon::Color, bool, bool)>> = Vec::new();
        match side {
            Side::Preview { icon, title, about, lines: text } => {
                let mut head = Vec::new();
                let mut c = 1;
                if let Some((i, color)) = icon {
                    head.push((c, i.to_string(), *color, true, false));
                    c += 3;
                }
                head.push((c, fit(title, width.saturating_sub(c + 2)), fg, false, false));
                let after = c + title.chars().count().min(width) + 2;
                if after + 4 < width {
                    head.push((after, fit(about, width - after), quiet, false, false));
                }
                side_lines.push(head);
                side_lines.push(Vec::new());
                let numbers = text.len().to_string().len().max(2);
                for (i, l) in text.iter().enumerate() {
                    side_lines.push(vec![
                        (1, format!("{:>numbers$}", i + 1), quiet, false, false),
                        (numbers + 3, fit(l, width.saturating_sub(numbers + 4)), fg, false, false),
                    ]);
                }
            }
            Side::Details { title, text, facts } => {
                side_lines.push(vec![(1, fit(title, width.saturating_sub(2)), fg, false, false)]);
                side_lines.push(Vec::new());
                for l in crate::page::wrap(text, width.saturating_sub(2)) {
                    side_lines.push(vec![(1, l, quiet, false, false)]);
                }
                side_lines.push(Vec::new());
                for (label, value, keys) in facts {
                    side_lines.push(vec![(1, label.clone(), quiet, false, false), (14, fit(value, width.saturating_sub(15)), if *keys { accent } else { fg }, false, *keys)]);
                }
            }
        }
        for (i, parts) in side_lines.into_iter().take(side_rows.saturating_sub(1)).enumerate() {
            let line = &mut body[i];
            for (c, text, color, icon, keys) in parts {
                if keys {
                    put_keys(line, &mut boxes, first + i, side_col + c, &text, color);
                } else {
                    line.put(side_col + c, &text, color, icon);
                }
            }
        }
        boxes.push(CardBox { row: first, rows: side_rows, col: side_col, cols: cols - side_col, kind: BoxKind::Side });
    }
    lines.extend(body);

    // The foot: the keys.
    lines.push(Line::default());
    let foot_row = lines.len();
    let mut foot = Line::default();
    let mut c = 1;
    let footer: Vec<(String, String)> = if pages > 1 {
        let mut f = view.footer.clone();
        f.insert(0, ("C-n".to_string(), format!("page {} of {pages}", page + 1)));
        f
    } else {
        view.footer.clone()
    };
    for (keys, what) in &footer {
        if c + keys_width(keys) + what.chars().count() + 2 > cols {
            break;
        }
        let end = put_keys(&mut foot, &mut boxes, foot_row, c, keys, accent);
        c = foot.put(end + 1, what, quiet, false) + 3;
    }
    lines.push(foot);

    let rows = lines.len();
    let height = rows as f32 * line_height + PAD_Y * 2.0;
    let rect = fenix_window::Rect { x: (area.x + (area.w - width) / 2.0).round(), y: area.y + TOP.min((area.h - height).max(0.0)), w: width, h: height.min(area.h) };
    Card { rect, spans: join(lines, fg), boxes, style: CardStyle::Floating, page, pages }
}

/// The lines as one span list, rows ended by "\n".
fn join(lines: Vec<Line>, fg: glyphon::Color) -> RowSpans {
    let n = lines.len();
    let mut spans = Vec::new();
    for (i, l) in lines.into_iter().enumerate() {
        spans.extend(l.spans);
        if i + 1 < n {
            spans.push(("\n".to_string(), fg, false));
        }
    }
    spans
}

/// The hint bar for Vim's own prefixes: the keys so far and their name,
/// then what can follow, a cap and a word each, as many rows as they
/// take (up to four, in pages after that). Under the code's bottom when
/// the cursor is above it (`cursor_y`), else hanging under the top of
/// `area`, so it never covers the line being worked on.
#[allow(clippy::too_many_arguments)]
pub(super) fn hint_bar(path: &[KeyPress], title: &str, hints: &[fenix_keymap::Hint], theme: &Theme, area: fenix_window::Rect, char_width: f32, line_height: f32, cursor_y: Option<f32>, page: usize) -> Card {
    let width = (area.w - 12.0).max(120.0);
    let cols = (((width - PAD_X * 2.0) / char_width).floor() as usize).max(20);
    let (fg, quiet, accent, group) = (theme.fg, theme.gutter_fg, theme.caret_text, theme.syntax_keyword);
    let mut boxes = Vec::new();
    // The keys so far, the name, and the way out.
    let mut head = Line::default();
    let mut c = 1;
    for (i, key) in path.iter().enumerate() {
        if i > 0 {
            c = head.put(c + 1, "›", quiet, false) + 1;
        }
        c = put_keys(&mut head, &mut boxes, 0, c, &keymap::describe_keypress(key), accent);
    }
    c = head.put(c + 2, title, group, false);
    head.put(c, &format!("  · {} keys", hints.len()), quiet, false);
    // Backspace only when it goes back: at the first key it closes, as
    // Esc does, and one "close" is enough.
    let out: &[(&str, &str)] = if path.len() > 1 { &[("Bksp", "back"), ("Esc", "close")] } else { &[("Esc", "close")] };
    let out_w: usize = out.iter().map(|(k, w)| keys_width(k) + w.chars().count() + 4).sum();
    let mut oc = cols.saturating_sub(out_w) + 2;
    for &(k, w) in out {
        let end = put_keys(&mut head, &mut boxes, 0, oc, k, accent);
        oc = head.put(end + 1, w, quiet, false) + 3;
    }
    // What can follow, flowing.
    let mut rows: Vec<(Line, Vec<CardBox>)> = Vec::new();
    let mut line = Line::default();
    let mut b = Vec::new();
    let mut c = 1;
    for h in hints {
        let key = keymap::describe_keypress(&h.key);
        let w = keys_width(&key) + 1 + h.label.chars().count();
        if c > 1 && c + w + 1 > cols {
            rows.push((std::mem::take(&mut line), std::mem::take(&mut b)));
            c = 1;
        }
        let end = put_keys(&mut line, &mut b, 0, c, &key, accent);
        c = line.put(end + 1, h.label, if h.group { group } else { fg }, false) + 3;
    }
    if c > 1 {
        rows.push((line, b));
    }
    const MAX_ROWS: usize = 4;
    let pages = rows.len().div_ceil(MAX_ROWS).max(1);
    let page = page.min(pages - 1);
    let mut lines = vec![head];
    if pages > 1 {
        let note = format!("page {} of {pages}", page + 1);
        let at = lines[0].col + 2;
        lines[0].put(at, &note, quiet, false);
    }
    for (l, bx) in rows.into_iter().skip(page * MAX_ROWS).take(MAX_ROWS) {
        let r = lines.len();
        boxes.extend(bx.into_iter().map(|x| CardBox { row: r, ..x }));
        lines.push(l);
    }
    let height = lines.len() as f32 * line_height + PAD_Y * 1.5;
    let bottom = area.y + area.h;
    let grows_up = cursor_y.is_none_or(|y| y + line_height < bottom - height - 4.0);
    let y = if grows_up { bottom - height } else { area.y + 6.0 };
    let rect = fenix_window::Rect { x: area.x + 6.0, y, w: width, h: height };
    Card { rect, spans: join(lines, fg), boxes, style: CardStyle::Bar { grows_up }, page, pages }
}

/// Draws `card`'s surface and boxes under its text, at `alpha`; a
/// floating card dims the window behind it first.
#[allow(clippy::too_many_arguments)]
pub(super) fn paint(rects: &mut RectRenderer, gpu: &GpuState, card: &Card, rect: fenix_window::Rect, alpha: f32, theme: &Theme, char_width: f32, line_height: f32, window: (f32, f32), shadows: bool) {
    let a = |c: [f32; 4], k: f32| [c[0], c[1], c[2], c[3] * k * alpha];
    let style = |radius: f32, fill: [f32; 4], border: f32, border_color: [f32; 4]| crate::rect::BoxStyle { radius, fill, border, border_color };
    let hair = a(rgba_of(theme.fg), 0.12);
    match card.style {
        CardStyle::Floating => {
            rects.push_rect(gpu, 0.0, 0.0, window.0, window.1, [0.0, 0.0, 0.0, 0.42 * alpha]);
            if shadows {
                rects.push_shadow(gpu, rect.x, rect.y + 10.0, rect.w, rect.h, 12.0, 28.0, [0.0, 0.0, 0.0, 0.55 * alpha]);
            }
            rects.push_box(gpu, rect.x, rect.y, rect.w, rect.h, style(12.0, a(theme.card, 1.0), 1.0, hair));
        }
        CardStyle::Bar { grows_up } => {
            if shadows {
                rects.push_shadow(gpu, rect.x, rect.y + if grows_up { 0.0 } else { 8.0 }, rect.w, rect.h, 8.0, 16.0, [0.0, 0.0, 0.0, 0.45 * alpha]);
            }
            let radii = if grows_up { [8.0, 8.0, 0.0, 0.0] } else { [8.0, 8.0, 8.0, 8.0] };
            rects.push_corners(gpu, rect.x, rect.y, rect.w, rect.h, radii, a(theme.bg_modeline, 1.0));
        }
    }
    let origin = (rect.x + PAD_X, rect.y + if matches!(card.style, CardStyle::Bar { .. }) { PAD_Y * 0.75 } else { PAD_Y });
    let at = |b: &CardBox| (origin.0 + b.col as f32 * char_width, origin.1 + b.row as f32 * line_height);
    // Under everything: the side pane, then the selected row; the caps
    // and chips over them.
    let order = |k: BoxKind| match k {
        BoxKind::Side => 0,
        BoxKind::Input => 1,
        BoxKind::Selected => 2,
        _ => 3,
    };
    let mut boxes = card.boxes.clone();
    boxes.sort_by_key(|b| order(b.kind));
    for b in &boxes {
        let (x, y) = at(b);
        let w = b.cols as f32 * char_width;
        let h = b.rows as f32 * line_height;
        match b.kind {
            BoxKind::Input => {
                let pad = (line_height * 0.3).round();
                rects.push_box(gpu, rect.x + PAD_X * 0.6, y - pad, rect.w - PAD_X * 1.2, h + pad * 2.0, style(8.0, a(theme.input, 1.0), 0.0, [0.0; 4]));
            }
            BoxKind::Side => {
                rects.push_box(gpu, x - char_width * 0.5, y - 4.0, (rect.x + rect.w - PAD_X * 0.6) - (x - char_width * 0.5), h + 8.0, style(8.0, a(theme.bg, 1.0), 0.0, [0.0; 4]));
            }
            BoxKind::Selected => {
                rects.push_box(gpu, x + char_width * 0.2, y, w - char_width * 0.4, h, style(6.0, a(theme.selection, 1.0), 0.0, [0.0; 4]));
            }
            BoxKind::Key => {
                let r = panels::key_cap(origin.0, char_width, y, line_height, b.col, b.col + b.cols);
                rects.push_box(gpu, r.x, r.y, r.w, r.h, style(panels::KEY_RADIUS, a(theme.input, 1.0), 1.0, hair));
            }
            BoxKind::Chip { accent } => {
                let r = panels::key_cap(origin.0, char_width, y, line_height, b.col, b.col + b.cols);
                let fill = if accent { a(theme.caret, 0.16) } else { a(mix(theme.input, rgba_of(theme.fg), 0.05), 1.0) };
                rects.push_box(gpu, r.x - char_width * 0.4, r.y, r.w + char_width * 0.8, r.h, style(r.h / 2.0, fill, 0.0, [0.0; 4]));
            }
            BoxKind::Badge => {
                let r = panels::key_cap(origin.0, char_width, y, line_height, b.col, b.col + b.cols);
                rects.push_box(gpu, r.x, r.y + 1.0, r.w, r.h - 2.0, style((r.h - 2.0) / 2.0, a(theme.input, 1.0), 0.0, [0.0; 4]));
            }
        }
    }
}

/// The text inset of a card.
pub(super) fn inset(card: &Card) -> (f32, f32) {
    match card.style {
        CardStyle::Floating => (PAD_X, PAD_Y),
        CardStyle::Bar { .. } => (PAD_X, PAD_Y * 0.75),
    }
}

/// The friendly name of what a picker picks from, for its chip.
pub(super) fn picker_name(picker: &ActivePicker) -> &'static str {
    match picker {
        ActivePicker::FindFile(_) => "Files",
        ActivePicker::Launch(_) => "Launch",
        ActivePicker::Commands(_) => "Commands",
        ActivePicker::Bindings(_) => "Keys",
        ActivePicker::BufferTodos(_) | ActivePicker::ProjectTodos(_) => "TODOs",
        ActivePicker::SwitchProject(_) => "Projects",
        ActivePicker::SwitchBuffer(_) => "Buffers",
        ActivePicker::Recovery(_) => "Recover",
        ActivePicker::Places(_) => "Places",
        ActivePicker::Agents(_) => "Agents",
        ActivePicker::DeleteProject(_) => "Remove a project",
        ActivePicker::Theme(_) => "Themes",
        ActivePicker::Encoding(_) => "Encodings",
        ActivePicker::Snippet(_) => "Snippets",
        ActivePicker::Symbol(_) => "Symbols",
        ActivePicker::Task(_) => "Tasks",
        ActivePicker::CompareTarget(_) | ActivePicker::CompareOwn { .. } | ActivePicker::Versus { .. } | ActivePicker::RebaseOnto(_) | ActivePicker::MergeFrom(_) => "Refs",
        ActivePicker::SwitchBranch(_) => "Branches",
        ActivePicker::SettingChoice { .. } => "Choices",
        ActivePicker::TableColumn(_) => "Columns",
        ActivePicker::BufferSearch(_) => "Lines",
        ActivePicker::SwitchWorkspace(_) | ActivePicker::WorkspaceLauncher(_) => "Workspaces",
        ActivePicker::Outline(_) => "Headings",
        ActivePicker::Code { badge, .. } => badge,
    }
}

/// `$body` with `$s` bound to whichever picker state `$picker` holds.
macro_rules! with_state {
    ($picker:expr, $s:ident => $body:expr) => {
        match $picker {
            ActivePicker::FindFile($s) => $body,
            ActivePicker::SwitchProject($s) => $body,
            ActivePicker::SwitchBuffer($s) => $body,
            ActivePicker::Recovery($s) => $body,
            ActivePicker::Places($s) => $body,
            ActivePicker::Agents($s) => $body,
            ActivePicker::DeleteProject($s) => $body,
            ActivePicker::Encoding($s) => $body,
            ActivePicker::Theme($s) => $body,
            ActivePicker::Commands($s) => $body,
            ActivePicker::Bindings($s) => $body,
            ActivePicker::Snippet($s) => $body,
            ActivePicker::Symbol($s) => $body,
            ActivePicker::TableColumn($s) => $body,
            ActivePicker::BufferSearch($s) => $body,
            ActivePicker::BufferTodos($s) => $body,
            ActivePicker::ProjectTodos($s) => $body,
            ActivePicker::SwitchWorkspace($s) => $body,
            ActivePicker::WorkspaceLauncher($s) => $body,
            ActivePicker::Outline($s) => $body,
            ActivePicker::Task($s) => $body,
            ActivePicker::Launch($s) => $body,
            ActivePicker::CompareTarget($s) => $body,
            ActivePicker::RebaseOnto($s) => $body,
            ActivePicker::MergeFrom($s) => $body,
            ActivePicker::SwitchBranch($s) => $body,
            ActivePicker::Code { state: $s, .. } => $body,
            ActivePicker::CompareOwn { picker: $s, .. } => $body,
            ActivePicker::Versus { picker: $s, .. } => $body,
            ActivePicker::SettingChoice { picker: $s, .. } => $body,
        }
    };
}

/// A picker's rows in `[offset, offset + count)`: whether it's the
/// selected one, its label, its boost, and the characters matched.
fn generic_rows(picker: &ActivePicker, offset: usize, count: usize) -> Vec<(bool, String, i64, Vec<usize>)> {
    with_state!(picker, s => s.visible_rows(offset, count).map(|(sel, c)| (sel, c.label.clone(), c.boost, s.positions(&c.label))).collect())
}

/// How many candidates a picker has before filtering.
fn picker_total(picker: &ActivePicker) -> usize {
    with_state!(picker, s => s.total())
}

/// A file's name and its folder, from a picker label (`src/app/main.rs`
/// -> `main.rs`, `src/app`), and the matched characters of each.
fn split_path(label: &str, hits: &[usize]) -> (String, Vec<usize>, String, Vec<usize>) {
    let chars: Vec<char> = label.chars().collect();
    let name_start = chars.iter().rposition(|&c| c == '/' || c == '\\').map(|i| i + 1).unwrap_or(0);
    let name: String = chars[name_start..].iter().collect();
    let dir: String = chars[..name_start.saturating_sub(1)].iter().collect();
    let name_hits = hits.iter().filter(|&&i| i >= name_start).map(|&i| i - name_start).collect();
    let dir_hits = hits.iter().filter(|&&i| i + 1 < name_start).copied().collect();
    (name, name_hits, dir, dir_hits)
}

/// A command's area, from its id's first part: `git.open` -> `Git`.
pub(super) fn command_area(id: &str) -> String {
    let head = id.split('.').next().unwrap_or(id);
    match head {
        "file" => "Files".to_string(),
        "app" => "Fenix".to_string(),
        "lsp" => "LSP".to_string(),
        "insert" | "snippets" => "Snippets".to_string(),
        "commands" => "Commands".to_string(),
        other => {
            let mut c = other.chars();
            c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default().replace('_', " ")
        }
    }
}

/// A command's short name: the menu's label for its key when it has one
/// (`status page` -> `Status page`), else its description's first clause.
pub(super) fn command_name(label: Option<&str>, description: &str) -> String {
    let raw = match label {
        Some(l) if !l.trim().is_empty() => l.trim().to_string(),
        _ => {
            let cut = [": ", ", ", "; ", " (", " -- ", " — "].iter().filter_map(|s| description.find(s)).min().unwrap_or(description.len());
            description[..cut].trim().trim_end_matches('.').to_string()
        }
    };
    let mut c = raw.chars();
    let name = c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default();
    if name.chars().count() > 44 {
        let cut: String = name.chars().take(43).collect();
        match cut.rfind(' ') {
            Some(i) if i > 20 => format!("{}…", &cut[..i]),
            _ => format!("{cut}…"),
        }
    } else {
        name
    }
}

/// Each command bound under the leader: its keys (`SPC g g`), the
/// groups on the way (`git`), and its own label (`status page`).
/// The least a recently run command is ranked up by: more than any
/// command's usefulness (`command_usefulness`), so the two never mix.
const RECENT_BOOST: i64 = 10;

/// How much a command is ranked up among ones its query matches as well:
/// bound under the leader (`keys`, as "SPC g g"), and the more for fewer
/// keys; and an area's own main command -- its last key its group's
/// again, as `SPC g g` (Git's status page), `SPC p p`, `SPC a a` --
/// first of all. Small: only an even match is decided by it.
pub(super) fn command_usefulness(keys: Option<&str>) -> i64 {
    let Some(keys) = keys else { return 0 };
    let parts: Vec<&str> = keys.split(' ').collect();
    let shallow = if parts.len() <= 3 { 1 } else { 0 };
    let main = if parts.len() >= 3 && parts[parts.len() - 1] == parts[parts.len() - 2] { 2 } else { 0 };
    3 + shallow + main
}

pub(super) type Places = HashMap<&'static str, (String, Vec<&'static str>, &'static str)>;

/// Each command bound under the leader (see `Places`).
pub(super) fn leader_places() -> Places {
    let trie = keymap::leader_trie();
    let mut out: Places = HashMap::new();
    for (seq, id) in trie.bindings() {
        let Some((last, prefix)) = seq.split_last() else { continue };
        let mut m = trie.matcher();
        let mut groups = Vec::new();
        for (i, k) in prefix.iter().enumerate() {
            m.feed(*k);
            if i > 0 {
                if let Some(l) = m.label() {
                    groups.push(l);
                }
            }
        }
        let label = m.pending_hints().into_iter().find(|h| h.key == *last).map(|h| h.label).unwrap_or("");
        let keys = seq.iter().map(keymap::describe_keypress).collect::<Vec<_>>().join(" ");
        let better = out.get(*id).is_none_or(|(k, _, _)| keys.len() < k.len());
        if better {
            out.insert(id, (keys, groups, label));
        }
    }
    out
}

impl App {
    /// `leader_places`, kept until the leader's keymap is reloaded.
    pub(super) fn places(&self) -> std::rc::Rc<Places> {
        self.leader_places.borrow_mut().get_or_insert_with(|| std::rc::Rc::new(leader_places())).clone()
    }

    /// Why command `id` can't run here, when that's plain without asking
    /// anything slow: XML's commands outside XML, a debug session's
    /// without one.
    fn command_unavailable(&self, id: &str) -> Option<&'static str> {
        if id.starts_with("code.xml_") && self.focused_language() != Some(fenix_syntax::LanguageId::Xml) {
            return Some("Not an XML file");
        }
        let needs_session = ["debug.step_over", "debug.step_into", "debug.step_out", "debug.stop", "debug.pause", "debug.restart", "debug.run_to_cursor", "debug.focus_drawer", "debug.evaluate"];
        if needs_session.contains(&id) && self.debug_session.is_none() {
            return Some("No debug session is running");
        }
        None
    }

    /// The command palette's candidates: each command's area and short
    /// name (with its id, so `git.open` finds it too) as its label, its
    /// description as words it's also found by; the ones run lately
    /// first, the ones that can't run here last; the rest by area.
    pub(super) fn command_candidates(&self) -> Vec<fenix_picker::Candidate<String>> {
        let places = self.places();
        let rank = |id: &str| self.recent_commands.iter().position(|c| c == id);
        let mut rows: Vec<(i64, String, i64, fenix_picker::Candidate<String>)> = self
            .commands
            .iter()
            .filter(|c| c.id != "commands.palette")
            .map(|c| {
                let area = command_area(&c.id);
                let name = command_name(places.get(c.id.as_ref()).map(|p| p.2), &c.description);
                let from = c.source.map(|id| format!(" {}", extension_name(id))).unwrap_or_default();
                let recent = match (self.command_unavailable(&c.id), rank(&c.id)) {
                    (Some(_), _) => -1000,
                    (None, Some(r)) if r < 5 => 30 - r as i64 * 2,
                    (None, Some(_)) => RECENT_BOOST,
                    (None, None) => 0,
                };
                let useful = command_usefulness(places.get(c.id.as_ref()).map(|p| p.0.as_str()));
                let label = format!("{area} › {name}  {}", c.id);
                (recent, area, useful, fenix_picker::Candidate::new(label, c.id.to_string()).with_keywords(format!("{}{from}", c.description)).with_boost(recent + useful))
            })
            .collect();
        // With nothing typed they show in this order: lately run, then by
        // area -- in an area its main commands first -- then the ones that
        // can't run here.
        rows.sort_by(|a, b| b.0.min(1).cmp(&a.0.min(1)).then(b.0.cmp(&a.0)).then(a.1.cmp(&b.1)).then(b.2.cmp(&a.2)).then(a.3.label.cmp(&b.3.label)));
        rows.into_iter().map(|(_, _, _, c)| c).collect()
    }

    /// The recently opened files among `candidates` ranked up, so with
    /// nothing typed they come first (and with a query, a little ahead).
    pub(super) fn boost_recent_files(&self, candidates: Vec<fenix_picker::Candidate<PathBuf>>) -> Vec<fenix_picker::Candidate<PathBuf>> {
        let recent: HashMap<&Path, usize> = self.recent_files.paths().iter().take(8).enumerate().map(|(i, p)| (p.as_path(), i)).collect();
        let mut out: Vec<_> = candidates.into_iter().map(|c| match recent.get(c.payload.as_path()) {
            Some(&i) => { let boost = 8 - i as i64; c.with_boost(boost) }
            None => c,
        }).collect();
        // Stable: the recent ones first, in the order they were opened.
        out.sort_by_key(|c| -c.boost);
        out
    }

    /// The first lines of `path`, read once per selection and kept; at
    /// most `PREVIEW_BYTES` of it, and nothing of a binary file.
    fn preview_lines(&self, path: &Path) -> Vec<String> {
        if let Some((p, lines)) = &*self.palette_preview.borrow() {
            if p == path {
                return lines.clone();
            }
        }
        let lines = (|| -> Option<Vec<String>> {
            use std::io::Read;
            let mut bytes = Vec::new();
            std::fs::File::open(path).ok()?.take(PREVIEW_BYTES).read_to_end(&mut bytes).ok()?;
            if bytes.contains(&0) {
                return Some(vec!["(a binary file)".to_string()]);
            }
            let text = String::from_utf8_lossy(&bytes);
            Some(text.lines().take(PREVIEW_LINES).map(|l| l.replace('\t', "    ").chars().filter(|c| !c.is_control()).collect()).collect())
        })()
        .unwrap_or_default();
        *self.palette_preview.borrow_mut() = Some((path.to_path_buf(), lines.clone()));
        lines
    }

    /// The open picker as a palette, its list scrolled to keep the
    /// selected row in `lines` rows.
    pub(super) fn palette_view(&mut self, lines: usize) -> Option<View> {
        let theme = self.theme;
        let picker = self.active_picker.as_ref()?;
        let query = picker_query(picker).to_string();
        let len = picker_len(picker);
        let total = picker_total(picker);
        let selected = picker_selected_row(picker);
        let is_commands = matches!(picker, ActivePicker::Commands(_));
        let lines_per = if is_commands { 2 } else { 1 };
        // Room for a few section heads besides the rows.
        let per_view = (lines.saturating_sub(if query.is_empty() { 3 } else { 2 }) / lines_per).max(1);
        self.picker_scroll = scroll_to_include(self.picker_scroll, selected, per_view);
        let offset = self.picker_scroll;
        let picker = self.active_picker.as_ref()?;
        let count_chip = if query.is_empty() { format!("{total}") } else { format!("{len} of {total}") };
        let mut chips = vec![(picker_name(picker).to_string(), false)];
        let footer = vec![("Enter".to_string(), "pick".to_string()), ("C-n C-p".to_string(), "move".to_string()), ("C-v".to_string(), "paste".to_string()), ("Esc".to_string(), "close".to_string())];
        let mut view = View { prompt: "›".to_string(), prompt_is_key: false, query: query.clone(), placeholder: "Type to filter".to_string(), chips: Vec::new(), body: Body::List(Vec::new()), side: None, footer, empty: "Nothing matches".to_string() };
        match picker {
            ActivePicker::FindFile(state) => {
                if let Some(root) = &self.project_root {
                    chips.push((root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), false));
                }
                view.placeholder = "Find a file by its name or path".to_string();
                view.footer[0].1 = "open".to_string();
                let mut rows = Vec::new();
                let mut last_section: Option<&str> = None;
                for (sel, c) in state.visible_rows(offset, per_view) {
                    let hits = state.positions(&c.label);
                    let (name, name_hits, dir, dir_hits) = split_path(&c.label, &hits);
                    let section = if query.is_empty() {
                        let s = if c.boost > 0 { "recent" } else { "files" };
                        (last_section != Some(s)).then(|| { last_section = Some(s); (s.to_string(), None) })
                    } else {
                        None
                    };
                    rows.push(Row { section, icon: Some((icon::file_icon(&name), theme.icon_file)), title: name, title_hits: name_hits, aside: dir, aside_hits: dir_hits, selected: sel, ..Row::default() });
                }
                if let Some(path) = state.selected().map(|c| c.payload.clone()) {
                    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                    let about = if size < 1024 { format!("{size} B") } else { format!("{:.1} KB", size as f64 / 1024.0) };
                    let rel = self.project_root.as_ref().and_then(|r| path.strip_prefix(r).ok()).map(|p| p.to_string_lossy().replace('\\', "/")).unwrap_or(name.clone());
                    view.side = Some(Side::Preview { icon: Some((icon::file_icon(&name), theme.icon_file)), title: rel, about, lines: self.preview_lines(&path) });
                }
                view.body = Body::List(rows);
                view.empty = if total == 0 { "Listing the files…".to_string() } else { "No file matches".to_string() };
            }
            ActivePicker::Commands(state) => {
                view.prompt = ":".to_string();
                view.placeholder = "Run a command: type its name, its area or its key".to_string();
                view.footer = vec![("Enter".to_string(), "run".to_string()), ("Tab".to_string(), "give it a key".to_string()), ("C-y".to_string(), "copy the id".to_string()), ("C-n C-p".to_string(), "move".to_string()), ("Esc".to_string(), "close".to_string())];
                let places = self.places();
                // The area the query names, when it names one: its rows
                // first under its name, the rest "elsewhere".
                let first_word = query.split_whitespace().next().unwrap_or("").to_lowercase();
                let all: Vec<(String, String)> = state.visible_rows(0, len).map(|(_, c)| (c.payload.clone(), c.label.clone())).collect();
                let named_area = (!first_word.is_empty()).then(|| all.first().map(|(id, _)| command_area(id))).flatten().filter(|a| a.to_lowercase().starts_with(&first_word));
                let in_area = named_area.as_ref().map(|a| all.iter().filter(|(id, _)| command_area(id) == *a).count()).unwrap_or(0);
                if let Some(a) = &named_area {
                    chips.push((format!("{in_area} in {a}"), true));
                }
                let mut rows = Vec::new();
                let mut last_section: Option<String> = None;
                for (i, (sel, c)) in state.visible_rows(offset, per_view).enumerate() {
                    let id = c.payload.as_str();
                    let area = command_area(id);
                    let unavailable = self.command_unavailable(id);
                    let section_name = if unavailable.is_some() {
                        "can't run here".to_string()
                    } else if query.is_empty() {
                        if c.boost >= RECENT_BOOST { "recent".to_string() } else { area.clone() }
                    } else {
                        match &named_area {
                            Some(a) if *a == area && offset + i < in_area => a.clone(),
                            Some(_) => "elsewhere".to_string(),
                            None => String::new(),
                        }
                    };
                    let section = (!section_name.is_empty() && last_section.as_deref() != Some(section_name.as_str())).then(|| {
                        last_section = Some(section_name.clone());
                        let count = if section_name == area && query.is_empty() {
                            Some(all.iter().filter(|(cid, l)| command_area(cid) == area && !l.is_empty()).count())
                        } else if Some(&section_name) == named_area.as_ref() {
                            Some(in_area)
                        } else if section_name == "elsewhere" {
                            Some(len.saturating_sub(in_area))
                        } else {
                            None
                        };
                        (section_name.clone(), count)
                    });
                    let hits = state.positions(&c.label);
                    let name_start = area.chars().count() + 3;
                    let name: String = c.label.chars().skip(name_start).take_while(|_| true).collect::<String>();
                    let name = name.rsplit_once("  ").map(|(n, _)| n.to_string()).unwrap_or(name);
                    let area_hits = hits.iter().filter(|&&h| h < area.chars().count()).copied().collect();
                    let title_hits = hits.iter().filter(|&&h| h >= name_start && h < name_start + name.chars().count()).map(|h| h - name_start).collect();
                    let description = self.commands.get(id).map(|c| c.description.to_string()).unwrap_or_default();
                    let detail = unavailable.map(str::to_string).or_else(|| (description.to_lowercase() != name.to_lowercase()).then_some(description));
                    rows.push(Row {
                        section,
                        area: Some(area),
                        area_hits,
                        title: name,
                        title_hits,
                        detail,
                        keys: places.get(id).map(|p| p.0.clone()),
                        right: "no key".to_string(),
                        selected: sel,
                        dimmed: unavailable.is_some(),
                        ..Row::default()
                    });
                }
                if let Some(id) = state.selected().map(|c| c.payload.clone()) {
                    let cmd = self.commands.get(&id);
                    let description = cmd.map(|c| c.description.to_string()).unwrap_or_default();
                    let place = places.get(id.as_str());
                    let name = command_name(place.map(|p| p.2), &description);
                    let mut facts = Vec::new();
                    match place {
                        Some((keys, groups, label)) => {
                            facts.push(("keys".to_string(), keys.clone(), true));
                            let mut path = vec!["SPC".to_string()];
                            path.extend(groups.iter().map(|g| g.to_string()));
                            path.push(label.to_string());
                            facts.push(("menu".to_string(), path.join(" › "), false));
                        }
                        None => facts.push(("keys".to_string(), "none yet -- Tab gives it some".to_string(), false)),
                    }
                    facts.push(("id".to_string(), id.clone(), false));
                    if let Some(src) = cmd.and_then(|c| c.source) {
                        facts.push(("from".to_string(), extension_name(src), false));
                    }
                    if cmd.is_some_and(|c| c.external()) {
                        facts.push(("outside".to_string(), "fenix/runCommand runs it".to_string(), false));
                    }
                    view.side = Some(Side::Details { title: format!("{} › {name}", command_area(&id)), text: description, facts });
                }
                view.body = Body::List(rows);
                view.empty = "No command matches".to_string();
            }
            _ => {
                let mut rows = Vec::new();
                for (sel, label, _, hits) in generic_rows(picker, offset, per_view) {
                    rows.push(Row { title: label, title_hits: hits, selected: sel, ..Row::default() });
                }
                view.body = Body::List(rows);
            }
        }
        chips.push((count_chip, false));
        view.chips = chips;
        Some(view)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area() -> fenix_window::Rect {
        fenix_window::Rect { x: 0.0, y: 0.0, w: 1200.0, h: 800.0 }
    }

    fn text(card: &Card) -> String {
        card.spans.iter().map(|(t, _, _)| t.as_str()).collect()
    }

    fn view(rows: Vec<Row>) -> View {
        View { prompt: "›".into(), prompt_is_key: false, query: "fra".into(), placeholder: String::new(), chips: vec![("Files".into(), false), ("3 of 6".into(), true)], body: Body::List(rows), side: None, footer: vec![("Enter".into(), "open".into())], empty: "Nothing".into() }
    }

    /// The input row has the query and chips on an input box; each row
    /// its title, folder and keys; the selected one a box; the foot its
    /// keys on caps.
    #[test]
    fn a_card_lays_out_the_input_the_rows_and_the_foot() {
        let rows = vec![
            Row { title: "frames.rs".into(), title_hits: vec![0, 1, 2], aside: "src".into(), selected: true, keys: Some("SPC f f".into()), ..Row::default() },
            Row { title: "frames.rs".into(), aside: "tests".into(), ..Row::default() },
        ];
        let card = layout(&view(rows), crate::theme::default_theme(), area(), 9.0, 20.0, 0);
        let t = text(&card);
        let lines: Vec<&str> = t.lines().collect();
        assert!(lines[0].contains("› fra▏") && lines[0].contains("Files") && lines[0].contains("3 of 6"), "{t}");
        assert!(lines[2].contains("frames.rs  src") && lines[2].contains("SPC f g".replace('g', "f").as_str()), "{t}");
        assert!(card.boxes.iter().any(|b| b.kind == BoxKind::Input && b.row == 0));
        assert!(card.boxes.iter().any(|b| b.kind == BoxKind::Selected && b.row == 2));
        assert_eq!(card.boxes.iter().filter(|b| b.kind == BoxKind::Key && b.row == 2).count(), 3, "SPC, f and f each on a cap");
        assert!(lines.last().unwrap().contains("Enter open"));
        assert!(card.rect.w <= MAX_WIDTH && card.rect.x > 0.0);
    }

    /// The matched characters are in the accent; a dimmed row isn't.
    #[test]
    fn matched_characters_are_in_the_accent() {
        let theme = crate::theme::default_theme();
        let rows = vec![Row { title: "frames.rs".into(), title_hits: vec![0, 1, 2], selected: true, ..Row::default() }];
        let card = layout(&view(rows), theme, area(), 9.0, 20.0, 0);
        assert!(card.spans.iter().any(|(t, c, _)| t == "fra" && *c == theme.caret_text));
        let dim = vec![Row { title: "frames.rs".into(), title_hits: vec![0, 1, 2], dimmed: true, ..Row::default() }];
        let card = layout(&view(dim), theme, area(), 9.0, 20.0, 0);
        assert!(!card.spans.iter().any(|(t, c, _)| t == "fra" && *c == theme.caret_text));
    }

    /// A side pane beside the list when there's room; none in a narrow
    /// window, where the list gets it all.
    #[test]
    fn the_side_pane_goes_in_a_narrow_window() {
        let mut v = view(vec![Row { title: "a".into(), ..Row::default() }]);
        v.side = Some(Side::Preview { icon: None, title: "src/a.rs".into(), about: "1 KB".into(), lines: vec!["fn a() {}".into()] });
        let wide = layout(&v, crate::theme::default_theme(), area(), 9.0, 20.0, 0);
        assert!(wide.boxes.iter().any(|b| b.kind == BoxKind::Side) && text(&wide).contains("fn a() {}"));
        let narrow = layout(&v, crate::theme::default_theme(), fenix_window::Rect { w: 600.0, ..area() }, 9.0, 20.0, 0);
        assert!(!narrow.boxes.iter().any(|b| b.kind == BoxKind::Side) && !text(&narrow).contains("fn a() {}"));
    }

    /// A file's label splits into its name and folder, matches with them.
    #[test]
    fn a_path_splits_into_name_and_folder() {
        assert_eq!(split_path("src/frames.rs", &[0, 4, 5]), ("frames.rs".into(), vec![0, 1], "src".into(), vec![0]));
        assert_eq!(split_path("Cargo.toml", &[0]), ("Cargo.toml".into(), vec![0], String::new(), vec![]));
    }

    /// A command's area is its id's first part, its name the menu's label
    /// for it or the start of what it does.
    #[test]
    fn commands_get_an_area_and_a_short_name() {
        assert_eq!(command_area("git.open"), "Git");
        assert_eq!(command_area("file.save"), "Files");
        assert_eq!(command_name(Some("status page"), "Show the Git status"), "Status page");
        assert_eq!(command_name(None, "Cycle the line-number gutter: off, absolute, relative"), "Cycle the line-number gutter");
        assert!(command_name(None, &"word ".repeat(30)).chars().count() <= 45);
    }

    /// The leader's places: keys, the groups on the way, the label.
    /// Among commands a query matches as well, an area's main one comes
    /// first, then the bound ones by how few keys they take.
    #[test]
    fn an_areas_main_command_is_first_among_equals() {
        assert!(command_usefulness(Some("SPC g g")) > command_usefulness(Some("SPC g B")));
        assert!(command_usefulness(Some("SPC g B")) > command_usefulness(Some("SPC g x a")));
        assert!(command_usefulness(Some("SPC g x a")) > command_usefulness(None));
        assert!(command_usefulness(Some("SPC g g")) < RECENT_BOOST, "never mistaken for a recent one");
        let mut app = App::with_file(None);
        app.open_command_palette();
        for c in "git".chars() {
            app.picker_key(KeyPress::char(c));
        }
        let Some(ActivePicker::Commands(state)) = app.active_picker.as_ref() else { panic!("the command picker") };
        let first = state.visible_rows(0, 1).next().map(|(_, c)| c.payload.clone());
        assert_eq!(first.as_deref(), Some("git.open"), "git selects the status page first");
    }

    #[test]
    fn the_leader_knows_where_each_command_is() {
        let places = leader_places();
        let (keys, groups, label) = places.get("git.open").expect("SPC g g");
        assert_eq!(keys, "SPC g g");
        assert_eq!(groups, &vec!["git"]);
        assert_eq!(*label, "status page");
    }

    /// The hint bar sits on the modeline, unless the cursor is down there.
    #[test]
    fn the_hint_bar_keeps_off_the_cursors_line() {
        let hints: Vec<fenix_keymap::Hint> = vec![fenix_keymap::Hint { key: KeyPress::char('g'), label: "top", group: false, count: 0, section: None }];
        let theme = crate::theme::default_theme();
        let low = hint_bar(&[KeyPress::char('g')], "go", &hints, theme, area(), 9.0, 20.0, Some(100.0), 0);
        assert_eq!(low.style, CardStyle::Bar { grows_up: true });
        assert!(low.rect.y + low.rect.h >= 799.0);
        let high = hint_bar(&[KeyPress::char('g')], "go", &hints, theme, area(), 9.0, 20.0, Some(780.0), 0);
        assert_eq!(high.style, CardStyle::Bar { grows_up: false });
        assert!(high.rect.y < 100.0);
        assert!(text(&low).contains("g top"));
    }
}
