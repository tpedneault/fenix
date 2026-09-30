//! Markdown as the reading view lays it out: a flat list of blocks, each
//! with the source line it starts on and how deep in quotes it sits,
//! whose text is runs of styled spans. CommonMark plus GitHub's tables,
//! task lists, strikethrough and footnotes, `> [!note]` callouts,
//! `[[links]]`, `![[embeds]]`, `#tags` and ```mermaid blocks.

use pulldown_cmark::{Alignment, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

use crate::meta::{self, FrontMatter, Link};

/// What a run of text looks like.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Style {
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
    pub strike: bool,
    pub tag: bool,
}

/// Where a run of text goes when followed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkTo {
    /// `[[...]]` (or a Markdown link to a relative file).
    Wiki(Link),
    Url(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    pub style: Style,
    pub link: Option<LinkTo>,
}

impl Span {
    fn plain(text: impl Into<String>, style: Style) -> Span {
        Span { text: text.into(), style, link: None }
    }
}

/// A list item's marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Marker {
    Bullet,
    Number(u64),
    Task(bool),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    /// Front matter, shown as a line of tags.
    Meta { tags: Vec<String>, about: Option<String> },
    Heading { level: usize, spans: Vec<Span> },
    Paragraph { spans: Vec<Span> },
    Item { depth: usize, marker: Marker, spans: Vec<Span> },
    Code { lang: String, text: String },
    Mermaid { source: String },
    /// The first line of a `> [!kind] title` callout.
    Callout { kind: String, title: String },
    Table { align: Vec<Alignment>, header: Vec<Vec<Span>>, rows: Vec<Vec<Vec<Span>>> },
    Rule,
    Image { src: String, alt: String },
    /// `![[...]]` on a line of its own.
    Embed { link: Link },
    Footnote { label: String, spans: Vec<Span> },
}

/// A block with where it came from.
#[derive(Debug, Clone, PartialEq)]
pub struct Blk {
    pub block: Block,
    /// Its first source line.
    pub line: usize,
    /// How many `>` deep.
    pub quote: usize,
}

fn line_of(starts: &[usize], byte: usize) -> usize {
    match starts.binary_search(&byte) {
        Ok(i) => i,
        Err(i) => i.saturating_sub(1),
    }
}

/// Splits `[[links]]` and `#tags` out of a run of plain text.
fn split_text(text: &str, style: Style, line: usize, out: &mut Vec<Span>) {
    if style.code {
        out.push(Span::plain(text, style));
        return;
    }
    let links = meta::links_in_line(text, line).into_iter().filter(|l| !l.markdown).collect::<Vec<_>>();
    let chars: Vec<char> = text.chars().collect();
    let mut at = 0;
    let push_plain = |from: usize, to: usize, out: &mut Vec<Span>| {
        if from >= to {
            return;
        }
        let piece: String = chars[from..to].iter().collect();
        let mut last = 0;
        let pc: Vec<char> = piece.chars().collect();
        for (cols, _) in meta::inline_tags(&piece) {
            if cols.start > last {
                out.push(Span::plain(pc[last..cols.start].iter().collect::<String>(), style));
            }
            out.push(Span::plain(pc[cols.clone()].iter().collect::<String>(), Style { tag: true, ..style }));
            last = cols.end;
        }
        if last < pc.len() {
            out.push(Span::plain(pc[last..].iter().collect::<String>(), style));
        }
    };
    for l in links {
        push_plain(at, l.cols.start, out);
        out.push(Span { text: l.label(), style, link: Some(LinkTo::Wiki(l.clone())) });
        at = l.cols.end;
    }
    push_plain(at, chars.len(), out);
}

/// `text` as blocks.
pub fn parse(text: &str) -> Vec<Blk> {
    let fm = FrontMatter::parse(text);
    let mut starts = vec![0];
    for (i, b) in text.bytes().enumerate() {
        if b == b'\n' {
            starts.push(i + 1);
        }
    }
    let mut out: Vec<Blk> = Vec::new();
    let body_from = match &fm.lines {
        Some(r) => {
            if !fm.tags.is_empty() || fm.about.is_some() {
                out.push(Blk { block: Block::Meta { tags: fm.tags.clone(), about: fm.about.clone() }, line: 0, quote: 0 });
            }
            starts.get(r.end).copied().unwrap_or(text.len())
        }
        None => 0,
    };
    let body = &text[body_from..];
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_TASKLISTS);
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_FOOTNOTES);

    // What's being built.
    let mut spans: Vec<Span> = Vec::new();
    let mut style = Style::default();
    let mut link: Option<(LinkTo, usize)> = None;
    let mut quote = 0usize;
    let mut lists: Vec<Option<u64>> = Vec::new();
    let mut item: Option<(usize, Marker, usize)> = None;
    let mut code: Option<(String, String, usize)> = None;
    let mut heading: Option<(usize, usize)> = None;
    let mut para_line: Option<usize> = None;
    let mut table: Option<(Vec<Alignment>, Vec<Vec<Span>>, Vec<Vec<Vec<Span>>>, usize, bool)> = None;
    let mut row: Vec<Vec<Span>> = Vec::new();
    let mut image: Option<(String, String, usize)> = None;
    let mut footnote: Option<(String, usize)> = None;
    let mut quote_first: Vec<bool> = Vec::new();
    let mut text_buf = String::new();
    let mut text_line = 0usize;

    let mut events = Parser::new_ext(body, opts).into_offset_iter().peekable();

    macro_rules! flush_text {
        () => {
            if !text_buf.is_empty() {
                let t = std::mem::take(&mut text_buf);
                match &link {
                    Some((to, _)) => spans.push(Span { text: t, style, link: Some(to.clone()) }),
                    None => split_text(&t, style, text_line, &mut spans),
                }
            }
        };
    }

    while let Some((event, range)) = events.next() {
        let line = line_of(&starts, body_from + range.start);
        match event {
            Event::Start(tag) => {
                if !matches!(tag, Tag::Emphasis | Tag::Strong | Tag::Strikethrough | Tag::Link { .. }) {
                    flush_text!();
                }
                match tag {
                    Tag::Paragraph => {
                        para_line = Some(line);
                        spans.clear();
                    }
                    Tag::Heading { level, .. } => {
                        let level = match level {
                            HeadingLevel::H1 => 1,
                            HeadingLevel::H2 => 2,
                            HeadingLevel::H3 => 3,
                            HeadingLevel::H4 => 4,
                            HeadingLevel::H5 => 5,
                            HeadingLevel::H6 => 6,
                        };
                        heading = Some((level, line));
                        spans.clear();
                    }
                    Tag::BlockQuote(_) => {
                        quote += 1;
                        quote_first.push(true);
                    }
                    Tag::CodeBlock(kind) => {
                        let lang = match kind {
                            CodeBlockKind::Fenced(info) => info.split_whitespace().next().unwrap_or("").to_lowercase(),
                            CodeBlockKind::Indented => String::new(),
                        };
                        code = Some((lang, String::new(), line));
                    }
                    Tag::List(first) => lists.push(first),
                    Tag::Item => {
                        // A new item: whatever the previous one had is done.
                        if let Some((depth, marker, l)) = item.take() {
                            if !spans.is_empty() {
                                out.push(Blk { block: Block::Item { depth, marker, spans: std::mem::take(&mut spans) }, line: l, quote });
                            }
                        }
                        let depth = lists.len().saturating_sub(1);
                        let marker = match lists.last_mut() {
                            Some(Some(n)) => {
                                let m = Marker::Number(*n);
                                *n += 1;
                                m
                            }
                            _ => Marker::Bullet,
                        };
                        item = Some((depth, marker, line));
                        spans.clear();
                    }
                    Tag::Emphasis => {
                        flush_text!();
                        style.italic = true;
                    }
                    Tag::Strong => {
                        flush_text!();
                        style.bold = true;
                    }
                    Tag::Strikethrough => {
                        flush_text!();
                        style.strike = true;
                    }
                    Tag::Link { dest_url, .. } => {
                        flush_text!();
                        let url = dest_url.to_string();
                        let to = if url.contains("://") || url.starts_with("mailto:") {
                            LinkTo::Url(url)
                        } else {
                            let (t, h) = match url.split_once('#') {
                                Some((t, h)) => (t.to_string(), Some(h.to_string())),
                                None => (url.clone(), None),
                            };
                            LinkTo::Wiki(Link { target: t, heading: h, alias: None, embed: false, markdown: true, line, cols: 0..0 })
                        };
                        link = Some((to, spans.len()));
                    }
                    Tag::Image { dest_url, .. } => image = Some((dest_url.to_string(), String::new(), line)),
                    Tag::Table(align) => table = Some((align, Vec::new(), Vec::new(), line, true)),
                    Tag::TableHead => row.clear(),
                    Tag::TableRow => row.clear(),
                    Tag::TableCell => spans.clear(),
                    Tag::FootnoteDefinition(label) => {
                        footnote = Some((label.to_string(), line));
                        spans.clear();
                    }
                    _ => {}
                }
            }
            Event::End(end) => {
                if !matches!(end, TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough) {
                    flush_text!();
                }
                match end {
                    TagEnd::Paragraph => {
                        let l = para_line.take().unwrap_or(line);
                        let in_quote_first = quote_first.last().copied().unwrap_or(false);
                        if let Some(last) = quote_first.last_mut() {
                            *last = false;
                        }
                        if let Some((_, _, _)) = &item {
                            // An item's text stays with the item.
                        } else if footnote.is_some() {
                        } else if in_quote_first && callout_of(&spans).is_some() {
                            let (kind, title, rest) = callout_of(&spans).unwrap();
                            out.push(Blk { block: Block::Callout { kind, title }, line: l, quote });
                            if !rest.is_empty() {
                                out.push(Blk { block: Block::Paragraph { spans: rest }, line: l + 1, quote });
                            }
                            spans.clear();
                        } else if spans.len() == 1 && matches!(&spans[0].link, Some(LinkTo::Wiki(k)) if k.embed) {
                            let Some(LinkTo::Wiki(k)) = spans[0].link.clone() else { unreachable!() };
                            out.push(Blk { block: Block::Embed { link: k }, line: l, quote });
                            spans.clear();
                        } else if !spans.is_empty() {
                            out.push(Blk { block: Block::Paragraph { spans: std::mem::take(&mut spans) }, line: l, quote });
                        }
                    }
                    TagEnd::Heading(_) => {
                        if let Some((level, l)) = heading.take() {
                            out.push(Blk { block: Block::Heading { level, spans: std::mem::take(&mut spans) }, line: l, quote });
                        }
                    }
                    TagEnd::BlockQuote(_) => {
                        quote = quote.saturating_sub(1);
                        quote_first.pop();
                    }
                    TagEnd::CodeBlock => {
                        if let Some((lang, text, l)) = code.take() {
                            let text = text.trim_end_matches('\n').to_string();
                            let block = if lang == "mermaid" { Block::Mermaid { source: text } } else { Block::Code { lang, text } };
                            out.push(Blk { block, line: l, quote });
                        }
                    }
                    TagEnd::List(_) => {
                        if let Some((depth, marker, l)) = item.take() {
                            if !spans.is_empty() {
                                out.push(Blk { block: Block::Item { depth, marker, spans: std::mem::take(&mut spans) }, line: l, quote });
                            }
                        }
                        lists.pop();
                    }
                    TagEnd::Item => {
                        if let Some((depth, marker, l)) = item.take() {
                            out.push(Blk { block: Block::Item { depth, marker, spans: std::mem::take(&mut spans) }, line: l, quote });
                        }
                    }
                    TagEnd::Emphasis => {
                        flush_text!();
                        style.italic = false;
                    }
                    TagEnd::Strong => {
                        flush_text!();
                        style.bold = false;
                    }
                    TagEnd::Strikethrough => {
                        flush_text!();
                        style.strike = false;
                    }
                    TagEnd::Link => link = None,
                    TagEnd::Image => {
                        if let Some((src, alt, l)) = image.take() {
                            out.push(Blk { block: Block::Image { src, alt }, line: l, quote });
                        }
                    }
                    TagEnd::TableCell => row.push(std::mem::take(&mut spans)),
                    TagEnd::TableHead => {
                        if let Some(t) = &mut table {
                            t.1 = std::mem::take(&mut row);
                        }
                    }
                    TagEnd::TableRow => {
                        if let Some(t) = &mut table {
                            t.2.push(std::mem::take(&mut row));
                        }
                    }
                    TagEnd::Table => {
                        if let Some((align, header, rows, l, _)) = table.take() {
                            out.push(Blk { block: Block::Table { align, header, rows }, line: l, quote });
                        }
                    }
                    TagEnd::FootnoteDefinition => {
                        if let Some((label, l)) = footnote.take() {
                            out.push(Blk { block: Block::Footnote { label, spans: std::mem::take(&mut spans) }, line: l, quote });
                        }
                    }
                    _ => {}
                }
            }
            Event::Text(t) => {
                if let Some((_, text, _)) = &mut code {
                    text.push_str(&t);
                } else if let Some((_, alt, _)) = &mut image {
                    alt.push_str(&t);
                } else {
                    if text_buf.is_empty() {
                        text_line = line;
                    }
                    text_buf.push_str(&t);
                }
            }
            Event::Code(t) => {
                flush_text!();
                let s = Style { code: true, ..style };
                match &link {
                    Some((to, _)) => spans.push(Span { text: t.to_string(), style: s, link: Some(to.clone()) }),
                    None => spans.push(Span::plain(t.to_string(), s)),
                }
            }
            Event::SoftBreak => {
                if text_buf.is_empty() {
                    text_line = line;
                }
                text_buf.push(SOFT);
            }
            Event::HardBreak => {
                flush_text!();
                spans.push(Span::plain("\n", style));
            }
            Event::Rule => out.push(Blk { block: Block::Rule, line, quote }),
            Event::TaskListMarker(done) => {
                if let Some((_, marker, _)) = &mut item {
                    *marker = Marker::Task(done);
                }
            }
            Event::FootnoteReference(label) => {
                flush_text!();
                spans.push(Span::plain(format!("[{label}]"), Style { tag: true, ..style }));
            }
            Event::Html(h) | Event::InlineHtml(h) => {
                if code.is_none() {
                    let h = h.trim_end_matches('\n');
                    if !h.trim_start().starts_with("<!--") {
                        flush_text!();
                        spans.push(Span::plain(h.to_string(), Style { code: true, ..style }));
                    }
                }
            }
            _ => {}
        }
    }
    for b in &mut out {
        match &mut b.block {
            Block::Heading { spans, .. } | Block::Paragraph { spans } | Block::Item { spans, .. } | Block::Footnote { spans, .. } => unsoft(spans),
            Block::Table { header, rows, .. } => {
                header.iter_mut().for_each(|c| unsoft(c));
                rows.iter_mut().flatten().for_each(|c| unsoft(c));
            }
            _ => {}
        }
    }
    out
}

/// A soft line break inside a paragraph, until blocks are finished.
const SOFT: char = '\u{1f}';

fn unsoft(spans: &mut [Span]) {
    for s in spans {
        if s.text.contains(SOFT) {
            s.text = s.text.replace(SOFT, " ");
        }
    }
}

/// `[!kind] title` at the start of a quote's first paragraph.
fn callout_of(spans: &[Span]) -> Option<(String, String, Vec<Span>)> {
    let first = spans.first()?;
    let t = first.text.trim_start();
    let rest = t.strip_prefix("[!")?;
    let (kind, after) = rest.split_once(']')?;
    let kind = kind.trim().trim_end_matches(['-', '+']).to_lowercase();
    if kind.is_empty() || kind.contains(' ') {
        return None;
    }
    let mut rest_spans: Vec<Span> = Vec::new();
    let after = after.trim_start_matches(['-', '+']);
    let mut title = after.trim().to_string();
    // The first line is the title; the rest (after a soft break) is body.
    if let Some((t, b)) = after.split_once(SOFT) {
        title = t.trim().to_string();
        if !b.trim().is_empty() {
            rest_spans.push(Span::plain(b.trim_start().to_string(), first.style));
        }
    }
    rest_spans.extend(spans[1..].iter().cloned());
    let title = if title.is_empty() {
        let mut k = kind.clone();
        if let Some(c) = k.get_mut(0..1) {
            c.make_ascii_uppercase();
        }
        k
    } else {
        title
    };
    Some((kind, title, rest_spans))
}

/// The plain text of some spans.
pub fn plain(spans: &[Span]) -> String {
    spans.iter().map(|s| s.text.as_str()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(text: &str) -> Vec<String> {
        parse(text)
            .into_iter()
            .map(|b| match b.block {
                Block::Meta { .. } => "meta".into(),
                Block::Heading { level, spans } => format!("h{level} {}", plain(&spans)),
                Block::Paragraph { spans } => format!("p {}", plain(&spans)),
                Block::Item { depth, marker, spans } => format!("item{depth} {marker:?} {}", plain(&spans)),
                Block::Code { lang, text } => format!("code {lang} {text}"),
                Block::Mermaid { source } => format!("mermaid {source}"),
                Block::Callout { kind, title } => format!("callout {kind} {title}"),
                Block::Table { header, rows, .. } => format!("table {} cols {} rows", header.len(), rows.len()),
                Block::Rule => "rule".into(),
                Block::Image { src, alt } => format!("img {src} {alt}"),
                Block::Embed { link } => format!("embed {}", link.target),
                Block::Footnote { label, .. } => format!("fn {label}"),
            })
            .collect()
    }

    #[test]
    fn a_note_becomes_blocks_with_their_lines() {
        let text = "---\ntags: [a]\n---\n# Title\n\nSome **bold** and [[Other note]] #tag.\n\n- [x] done\n- [ ] open\n  - nested\n1. one\n2. two\n\n```c\nint x;\n```\n\n---\n";
        assert_eq!(
            kinds(text),
            vec![
                "meta",
                "h1 Title",
                "p Some bold and Other note #tag.",
                "item0 Task(true) done",
                "item0 Task(false) open",
                "item1 Bullet nested",
                "item0 Number(1) one",
                "item0 Number(2) two",
                "code c int x;",
                "rule",
            ]
        );
        let blocks = parse(text);
        assert_eq!(blocks[1].line, 3);
        assert_eq!(blocks[2].line, 5);
        let Block::Paragraph { spans } = &blocks[2].block else { panic!() };
        assert!(spans.iter().any(|s| s.style.bold && s.text == "bold"));
        assert!(spans.iter().any(|s| matches!(&s.link, Some(LinkTo::Wiki(l)) if l.target == "Other note")));
        assert!(spans.iter().any(|s| s.style.tag && s.text == "#tag"));
        assert_eq!(blocks[4].line, 8);
    }

    #[test]
    fn callouts_tables_images_embeds_and_mermaid() {
        let text = "> [!warning] Careful\n> OBC build only.\n\n| A | B |\n|---|---|\n| 1 | 2 |\n\n![scope](a.png)\n\n![[TC flow]]\n\n```mermaid\nflowchart TD\n  a-->b\n```\n";
        assert_eq!(kinds(text), vec!["callout warning Careful", "p OBC build only.", "table 2 cols 1 rows", "img a.png scope", "embed TC flow", "mermaid flowchart TD\n  a-->b"]);
        let blocks = parse(text);
        assert_eq!(blocks[0].quote, 1);
        assert_eq!(blocks[4].line, 9);
        assert_eq!(blocks[5].line, 11);
    }

    #[test]
    fn markdown_links_to_files_and_urls() {
        let blocks = parse("See [spec](docs/spec.md#intro) and <https://x.org>.\n");
        let Block::Paragraph { spans } = &blocks[0].block else { panic!() };
        assert!(spans.iter().any(|s| matches!(&s.link, Some(LinkTo::Wiki(l)) if l.target == "docs/spec.md" && l.heading.as_deref() == Some("intro"))));
        assert!(spans.iter().any(|s| matches!(&s.link, Some(LinkTo::Url(u)) if u == "https://x.org")));
    }
}
