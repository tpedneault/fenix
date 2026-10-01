//! Markdown as the reading view shows it: `fenix_notebook::blocks` laid
//! out on the page grid -- wrapped paragraphs with their emphasis, links
//! and tags coloured, headings with a rule, lists with their markers and
//! checkboxes, code on a panel and highlighted, tables with a grid,
//! callouts and quotes with a bar, and room left for pictures and
//! diagrams, which `App` draws over it. Pure: what it can't know (how a
//! code block highlights, how big a picture is, what an embed is) comes
//! from `Ctx`.

use std::ops::Range;
use std::path::PathBuf;

use fenix_notebook::blocks::{self, Blk, Block, LinkTo, Marker, Span};
use fenix_notebook::Link;

use crate::page::{fit, Grid, ImageKey, Page, Role};

/// What an `![[embed]]` turned out to be.
pub enum Embedded {
    Diagram { name: String, source: String },
    Note { name: String, text: String },
    Missing(String),
}

/// What the layout asks of the world.
pub struct Ctx<'a> {
    /// A code block's colours: per line, (columns, capture name).
    pub highlight: &'a dyn Fn(&str, &str) -> Vec<Vec<(Range<usize>, &'static str)>>,
    /// A picture's size in pixels, once it's known.
    pub image_size: &'a dyn Fn(&ImageKey) -> Option<(u32, u32)>,
    /// The key a diagram's source is drawn under.
    pub diagram_key: &'a dyn Fn(&str) -> ImageKey,
    /// Why a diagram's source doesn't draw, if it doesn't.
    pub diagram_error: &'a dyn Fn(&str) -> Option<String>,
    /// Where an image's `src` is on disk.
    pub image_path: &'a dyn Fn(&str) -> Option<PathBuf>,
    pub embed: &'a dyn Fn(&Link) -> Embedded,
    /// Whether a `[[link]]` goes somewhere that exists.
    pub link_exists: &'a dyn Fn(&Link) -> bool,
    /// One cell's size in pixels, for pictures' proportions.
    pub cell: (f32, f32),
}

impl Ctx<'_> {
    /// A plain context: no highlighting, no pictures known, every link
    /// found. For tests and previews that don't draw.
    #[cfg(test)]
    pub fn plain() -> Ctx<'static> {
        Ctx {
            highlight: &|_, _| Vec::new(),
            image_size: &|_| None,
            diagram_key: &|s| ImageKey::Diagram(s.len() as u64),
            diagram_error: &|_| None,
            image_path: &|s| Some(PathBuf::from(s)),
            embed: &|l| Embedded::Missing(l.target.clone()),
            link_exists: &|_| true,
            cell: (8.0, 17.0),
        }
    }
}

/// Something on the page that does something.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Link(LinkTo),
    /// The checkbox on this source line.
    Checkbox { line: usize, done: bool },
    /// A diagram: in a ```mermaid block on this line, or an embed.
    Diagram { line: usize, embed: Option<Link> },
    Image(PathBuf),
}

/// A place on the page and what it does.
#[derive(Debug, Clone, PartialEq)]
pub struct Target {
    pub line: usize,
    pub cols: Range<usize>,
    pub action: Action,
}

/// A laid-out document.
#[derive(Debug, Clone, Default)]
pub struct Rendered {
    pub page: Page,
    pub targets: Vec<Target>,
    /// For each page line, the source line it came from.
    pub source: Vec<Option<usize>>,
}

impl Rendered {
    /// The page line a source line is shown on (the nearest before it).
    pub fn page_line_of(&self, source_line: usize) -> usize {
        let mut best = 0;
        for (i, s) in self.source.iter().enumerate() {
            if let Some(s) = s {
                if *s <= source_line {
                    best = i;
                } else {
                    break;
                }
            }
        }
        best
    }

    /// The source line a page line came from.
    pub fn source_line_of(&self, page_line: usize) -> Option<usize> {
        self.source.iter().take(page_line + 1).rev().flatten().next().copied()
    }
}

/// Lays `text` out `width` cells wide.
pub fn layout(text: &str, width: usize, ctx: &Ctx) -> Rendered {
    let blocks = blocks::parse(text);
    let mut w = Writer { g: Grid::new(), y: 0, width: width.max(20), targets: Vec::new(), source: Vec::new() };
    let mut prev: Option<&Blk> = None;
    for b in &blocks {
        // A blank line between blocks, not between a list's items.
        if let Some(p) = prev {
            let both_items = matches!(p.block, Block::Item { .. }) && matches!(b.block, Block::Item { .. });
            let callout_body = matches!(p.block, Block::Callout { .. });
            if !both_items && !callout_body {
                w.blank(b.quote.min(p.quote));
            }
        }
        w.block(b, ctx);
        prev = Some(b);
    }
    let lines = w.g.height();
    w.source.resize(lines, None);
    Rendered { page: w.g.finish(), targets: w.targets, source: w.source }
}

struct Writer {
    g: Grid,
    y: usize,
    width: usize,
    targets: Vec<Target>,
    source: Vec<Option<usize>>,
}

/// The colour a run of text gets.
fn role_of(span: &Span, ctx: &Ctx) -> Role {
    match &span.link {
        Some(LinkTo::Wiki(l)) if !(ctx.link_exists)(l) => Role::Warn,
        Some(_) => Role::Syntax("text.uri"),
        None if span.style.code => Role::Syntax("text.literal"),
        None if span.style.tag => Role::Syntax("text.reference"),
        None if span.style.strike => Role::Muted,
        None if span.style.bold => Role::Syntax("text.strong"),
        None if span.style.italic => Role::Syntax("text.emphasis"),
        None => Role::Text,
    }
}

impl Writer {
    fn mark(&mut self, line: usize) {
        if self.source.len() <= self.y {
            self.source.resize(self.y + 1, None);
        }
        self.source[self.y] = Some(line);
    }

    /// The quote bars for depth `quote`, and the column after them.
    fn bars(&mut self, quote: usize) -> usize {
        for q in 0..quote {
            self.g.put(self.y, q * 2, "│", Role::Muted);
        }
        quote * 2
    }

    fn blank(&mut self, quote: usize) {
        self.bars(quote);
        self.g.put(self.y, 0, "", Role::Text);
        self.y += 1;
    }

    /// Writes `spans` wrapped into columns `x..width`, the first line
    /// starting at `first_x`; returns the lines used.
    fn spans(&mut self, spans: &[Span], x: usize, first_x: usize, quote: usize, ctx: &Ctx, muted: bool) -> usize {
        // Words (and the spaces after them), each char with its role and
        // the link it belongs to.
        let mut pieces: Vec<(String, Role, Option<LinkTo>)> = Vec::new();
        for s in spans {
            let role = if muted { Role::Muted } else { role_of(s, ctx) };
            if s.text == "\n" {
                pieces.push(("\n".into(), role, None));
                continue;
            }
            let mut word = String::new();
            for c in s.text.chars() {
                word.push(c);
                if c == ' ' {
                    pieces.push((std::mem::take(&mut word), role, s.link.clone()));
                }
            }
            if !word.is_empty() {
                pieces.push((word, role, s.link.clone()));
            }
        }
        let right = self.width;
        let mut col = first_x;
        let mut lines = 1;
        self.bars(quote);
        for (piece, role, link) in pieces {
            if piece == "\n" {
                self.y += 1;
                lines += 1;
                self.bars(quote);
                col = x;
                continue;
            }
            let len = piece.trim_end().chars().count();
            if col > x && col + len > right {
                self.y += 1;
                lines += 1;
                self.bars(quote);
                col = x;
                if piece.trim().is_empty() {
                    continue;
                }
            }
            // A word longer than the line is cut to it.
            let shown: String = if len > right.saturating_sub(x) { piece.chars().take(right.saturating_sub(x).max(1)).collect() } else { piece.clone() };
            let start = col;
            let trimmed = shown.trim_end();
            let end = self.g.put(self.y, col, trimmed, role);
            if let Some(link) = link {
                // Joined onto the previous piece of the same link.
                match self.targets.last_mut() {
                    Some(t) if t.line == self.y && t.cols.end + 1 >= start && t.action == Action::Link(link.clone()) => t.cols.end = end,
                    _ => self.targets.push(Target { line: self.y, cols: start..end, action: Action::Link(link) }),
                }
            }
            col = start + shown.chars().count();
        }
        self.y += 1;
        lines
    }

    fn block(&mut self, b: &Blk, ctx: &Ctx) {
        let x0 = b.quote * 2;
        self.mark(b.line);
        match &b.block {
            Block::Meta { tags, about } => {
                self.bars(b.quote);
                let mut col = x0;
                for t in tags {
                    col = self.g.put(self.y, col, &format!("#{t}"), Role::Syntax("text.reference")) + 1;
                }
                if let Some(a) = about {
                    let text = if tags.is_empty() { format!("about {a}") } else { format!("· about {a}") };
                    self.g.put(self.y, col, &text, Role::Muted);
                }
                self.y += 1;
            }
            Block::Heading { level, spans } => {
                let title: String = blocks::plain(spans);
                let role = if *level <= 2 { Role::Syntax("text.title") } else { Role::Title };
                let prefix = if *level >= 3 { "#".repeat(*level - 2) + " " } else { String::new() };
                self.bars(b.quote);
                let col = if prefix.is_empty() { x0 } else { self.g.put(self.y, x0, &prefix, Role::Muted) };
                let shown = if *level == 1 { title.to_uppercase() } else { title };
                let end = self.g.put(self.y, col, &fit(&shown, self.width.saturating_sub(col)), role);
                // A hairline after the title: to the edge for a chapter,
                // a short one for a section.
                if *level <= 2 {
                    let stop = if *level == 1 { self.width } else { (end + 12).min(self.width) };
                    if end + 2 < stop {
                        self.g.rule(self.y, end + 1..stop);
                    }
                }
                self.y += 1;
            }
            Block::Paragraph { spans } => {
                self.spans(spans, x0, x0, b.quote, ctx, false);
            }
            Block::Item { depth, marker, spans } => {
                let indent = x0 + depth * 3;
                self.bars(b.quote);
                let (mark, role) = match marker {
                    Marker::Bullet => ("•".to_string(), Role::Muted),
                    Marker::Number(n) => (format!("{n}."), Role::Muted),
                    Marker::Task(true) => ("[x]".to_string(), Role::Good),
                    Marker::Task(false) => ("[ ]".to_string(), Role::Accent),
                };
                let end = self.g.put(self.y, indent, &mark, role);
                if let Marker::Task(done) = marker {
                    self.targets.push(Target { line: self.y, cols: indent..end, action: Action::Checkbox { line: b.line, done: *done } });
                }
                let text_x = end + 1;
                let done = matches!(marker, Marker::Task(true));
                self.spans(spans, text_x, text_x, b.quote, ctx, done);
            }
            Block::Code { lang, text } => self.code(lang, text, b.quote, ctx),
            Block::Mermaid { source } => self.diagram(source, None, b.line, b.quote, ctx),
            Block::Callout { kind, title } => {
                let role = match kind.as_str() {
                    "warning" | "caution" | "attention" => Role::Warn,
                    "danger" | "error" | "bug" | "failure" => Role::Bad,
                    "tip" | "success" | "check" | "done" => Role::Good,
                    _ => Role::Accent,
                };
                let bar_x = x0.saturating_sub(2);
                self.g.put(self.y, bar_x, "┃", role);
                let label = format!("{}", title);
                self.g.put(self.y, x0, &fit(&label, self.width.saturating_sub(x0)), role);
                self.y += 1;
            }
            Block::Table { align, header, rows } => self.table(align, header, rows, b.quote, ctx),
            Block::Rule => {
                self.bars(b.quote);
                self.g.rule(self.y, x0..self.width);
                self.y += 1;
            }
            Block::Image { src, alt } => match (ctx.image_path)(src) {
                Some(path) => {
                    let key = ImageKey::File(path.clone());
                    self.picture(key, x0, b.quote, ctx, Action::Image(path));
                    if !alt.is_empty() {
                        self.bars(b.quote);
                        self.g.put(self.y, x0, &fit(alt, self.width.saturating_sub(x0)), Role::Muted);
                        self.y += 1;
                    }
                }
                None => {
                    self.bars(b.quote);
                    self.g.put(self.y, x0, &fit(&format!("[image {src} isn't there]"), self.width.saturating_sub(x0)), Role::Warn);
                    self.y += 1;
                }
            },
            Block::Embed { link } => match (ctx.embed)(link) {
                Embedded::Diagram { name, source } => {
                    self.diagram(&source, Some((name, link.clone())), b.line, b.quote, ctx);
                }
                Embedded::Note { name, text } => {
                    self.bars(b.quote);
                    let end = self.g.put(self.y, x0, "↳ ", Role::Muted);
                    let end2 = self.g.put(self.y, end, &name, Role::Syntax("text.uri"));
                    self.targets.push(Target { line: self.y, cols: end..end2, action: Action::Link(LinkTo::Wiki(link.clone())) });
                    self.y += 1;
                    // Its first lines, quoted.
                    let inner = layout(&text, self.width.saturating_sub(x0 + 2), ctx);
                    for (i, line) in inner.page.text.lines().take(8).enumerate() {
                        self.bars(b.quote + 1);
                        let row: Vec<&crate::page::Span> = inner.page.spans.iter().filter(|s| s.line == i).collect();
                        let plain = line.to_string();
                        if row.is_empty() {
                            self.g.put(self.y, x0 + 2, &plain, Role::Muted);
                        }
                        for s in row {
                            let piece: String = plain.chars().skip(s.cols.start).take(s.cols.len()).collect();
                            self.g.put(self.y, x0 + 2 + s.cols.start, &piece, s.role);
                        }
                        self.y += 1;
                    }
                }
                Embedded::Missing(name) => {
                    self.bars(b.quote);
                    self.g.put(self.y, x0, &fit(&format!("![[{name}]] -- nothing by that name yet"), self.width.saturating_sub(x0)), Role::Warn);
                    self.y += 1;
                }
            },
            Block::Footnote { label, spans } => {
                self.bars(b.quote);
                let end = self.g.put(self.y, x0, &format!("[{label}]"), Role::Muted) + 1;
                self.spans(spans, end, end, b.quote, ctx, true);
            }
        }
    }

    fn code(&mut self, lang: &str, text: &str, quote: usize, ctx: &Ctx) {
        let x0 = quote * 2;
        let colours = (ctx.highlight)(lang, text);
        let lines: Vec<&str> = if text.is_empty() { vec![""] } else { text.lines().collect() };
        for (i, line) in lines.iter().enumerate() {
            self.bars(quote);
            let shown: String = line.chars().take(self.width.saturating_sub(x0 + 2)).collect();
            self.g.panel(self.y, x0..self.width);
            // Coloured pieces and plain gaps, never overlapping: a run
            // of text is one colour.
            let chars: Vec<char> = shown.chars().collect();
            let mut at = 0;
            let mut spans: Vec<(Range<usize>, &'static str)> = colours.get(i).cloned().unwrap_or_default();
            spans.sort_by_key(|(r, _)| r.start);
            for (cols, name) in spans {
                let (start, end) = (cols.start.max(at).min(chars.len()), cols.end.min(chars.len()));
                if start >= end {
                    continue;
                }
                if start > at {
                    self.g.put(self.y, x0 + 1 + at, &chars[at..start].iter().collect::<String>(), Role::Text);
                }
                self.g.put(self.y, x0 + 1 + start, &chars[start..end].iter().collect::<String>(), Role::Syntax(name));
                at = end;
            }
            if at < chars.len() {
                self.g.put(self.y, x0 + 1 + at, &chars[at..].iter().collect::<String>(), Role::Text);
            }
            if i == 0 && !lang.is_empty() && shown.chars().count() + lang.len() + 3 < self.width - x0 {
                self.g.put(self.y, self.width - lang.len(), lang, Role::Muted);
            }
            self.y += 1;
        }
    }

    /// Room for a picture: as wide as it is (in cells) up to the line,
    /// as tall as its proportions make that.
    fn picture(&mut self, key: ImageKey, x0: usize, quote: usize, ctx: &Ctx, action: Action) {
        let avail = self.width.saturating_sub(x0).max(10);
        let (cw, ch) = ctx.cell;
        let (cols, rows) = match (ctx.image_size)(&key) {
            Some((w, h)) if w > 0 && h > 0 => {
                let natural = (w as f32 / cw).ceil() as usize;
                let cols = natural.clamp(8, avail);
                let rows = ((cols as f32 * cw) * h as f32 / w as f32 / ch).ceil() as usize;
                (cols, rows.clamp(2, 60))
            }
            _ => (avail.min(60), 10),
        };
        let start = self.y;
        for r in 0..rows {
            self.bars(quote);
            self.g.put(start + r, x0, "", Role::Text);
        }
        self.g.image(start, x0, rows, cols, key);
        self.targets.push(Target { line: start, cols: x0..x0 + cols, action });
        self.y = start + rows;
    }

    fn diagram(&mut self, source: &str, embed: Option<(String, Link)>, line: usize, quote: usize, ctx: &Ctx) {
        let x0 = quote * 2;
        if let Some(err) = (ctx.diagram_error)(source) {
            self.bars(quote);
            self.g.put(self.y, x0, &fit(&format!("diagram: {err}"), self.width.saturating_sub(x0)), Role::Bad);
            self.y += 1;
            self.code("mermaid", source, quote, ctx);
            return;
        }
        let key = (ctx.diagram_key)(source);
        let action = Action::Diagram { line, embed: embed.as_ref().map(|(_, l)| l.clone()) };
        self.picture(key, x0, quote, ctx, action);
        self.bars(quote);
        let caption = match &embed {
            Some((name, _)) => format!("{name} · diagram"),
            None => "diagram".to_string(),
        };
        self.g.put(self.y, x0, &fit(&caption, self.width.saturating_sub(x0)), Role::Muted);
        self.y += 1;
    }

    fn table(&mut self, align: &[pulldown_cmark::Alignment], header: &[Vec<Span>], rows: &[Vec<Vec<Span>>], quote: usize, ctx: &Ctx) {
        let x0 = quote * 2;
        let ncols = header.len().max(rows.iter().map(Vec::len).max().unwrap_or(0));
        if ncols == 0 {
            return;
        }
        let text = |cell: &Vec<Span>| blocks::plain(cell);
        let mut widths = vec![1usize; ncols];
        for (i, cell) in header.iter().enumerate() {
            widths[i] = widths[i].max(text(cell).chars().count());
        }
        for row in rows {
            for (i, cell) in row.iter().enumerate() {
                widths[i] = widths[i].max(text(cell).chars().count());
            }
        }
        // Squeezed, widest first, to fit.
        let avail = self.width.saturating_sub(x0 + 3 * (ncols - 1));
        while widths.iter().sum::<usize>() > avail {
            let (i, &w) = widths.iter().enumerate().max_by_key(|(_, w)| **w).unwrap();
            if w <= 4 {
                break;
            }
            widths[i] = w - 1;
        }
        let total: usize = widths.iter().sum::<usize>() + 3 * (ncols - 1);
        let write_row = |me: &mut Writer, cells: &[Vec<Span>], head: bool| {
            me.bars(quote);
            let mut x = x0;
            for (i, w) in widths.iter().enumerate() {
                let cell = cells.get(i).cloned().unwrap_or_default();
                let t = fit(&text(&cell), *w);
                let pad = w.saturating_sub(t.chars().count());
                let start = match align.get(i) {
                    Some(pulldown_cmark::Alignment::Right) => x + pad,
                    Some(pulldown_cmark::Alignment::Center) => x + pad / 2,
                    _ => x,
                };
                // One role per cell: its first span's (a link keeps its target).
                let role = if head { Role::Title } else { cell.first().map(|s| role_of(s, ctx)).unwrap_or(Role::Text) };
                let end = me.g.put(me.y, start, &t, role);
                if let Some(link) = cell.iter().find_map(|s| s.link.clone()) {
                    me.targets.push(Target { line: me.y, cols: start..end, action: Action::Link(link) });
                }
                x += w;
                if i + 1 < ncols {
                    me.g.put(me.y, x + 1, "│", Role::Muted);
                    x += 3;
                }
            }
            me.y += 1;
        };
        write_row(self, header, true);
        // The rule under the header gets a row of its own: hairlines are
        // drawn through a row's middle.
        self.bars(quote);
        self.g.rule(self.y, x0..x0 + total);
        self.y += 1;
        for row in rows {
            write_row(self, row, false);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_note_lays_out_with_headings_lists_and_links() {
        let text = "# Title\n\nSome **bold** text and a [[Link]] that wraps when the line is narrow.\n\n- [ ] open\n- [x] done\n";
        let r = layout(text, 30, &Ctx::plain());
        let lines: Vec<&str> = r.page.text.lines().collect();
        assert_eq!(lines[0], "TITLE");
        assert_eq!(lines[2], "Some bold text and a Link that");
        assert_eq!(lines[3], "wraps when the line is narrow.");
        assert_eq!(lines[5], "[ ] open");
        assert_eq!(lines[6], "[x] done");
        assert!(r.targets.iter().any(|t| t.line == 2 && t.cols == (21..25) && matches!(&t.action, Action::Link(LinkTo::Wiki(l)) if l.target == "Link")));
        assert!(r.targets.iter().any(|t| t.line == 5 && t.action == Action::Checkbox { line: 4, done: false }));
        assert_eq!(r.source_line_of(6), Some(5));
        assert_eq!(r.page_line_of(4), 5);
        assert!(r.page.rules.iter().any(|(l, _)| *l == 0));
    }

    #[test]
    fn tables_code_and_diagrams() {
        let text = "| A | Long header |\n|---|---:|\n| 1 | 2 |\n\n```c\nint x;\n```\n\n```mermaid\nflowchart TD\n a-->b\n```\n";
        let r = layout(text, 40, &Ctx::plain());
        let lines: Vec<&str> = r.page.text.lines().collect();
        assert_eq!(lines[0], "A │ Long header");
        assert_eq!(lines[2], "1 │           2");
        assert!(r.page.rules.iter().any(|(l, _)| *l == 1));
        assert_eq!(lines[4], " int x;                                c");
        assert!(r.page.panels.iter().any(|(l, _)| *l == 4));
        let image = &r.page.images[0];
        assert_eq!((image.line, image.rows), (6, 10));
        assert_eq!(lines[16], "diagram");
    }

    #[test]
    fn quotes_callouts_and_missing_links() {
        let ctx = Ctx { link_exists: &|l| l.target != "Nope", ..Ctx::plain() };
        let r = layout("> [!warning] Careful\n> body [[Nope]]\n", 40, &ctx);
        let lines: Vec<&str> = r.page.text.lines().collect();
        assert_eq!(lines[0], "┃ Careful");
        assert_eq!(lines[1], "│ body Nope");
        assert!(r.page.spans.iter().any(|s| s.line == 1 && s.role == Role::Warn));
    }
}
