//! Notes out of the notebook: as Markdown that renders anywhere (GitHub,
//! GitLab, any editor) and as one self-contained HTML page. `[[links]]`
//! become ordinary links (or plain text, when there's nowhere for them to
//! go), `![[diagram]]` embeds become ```mermaid blocks, and in HTML every
//! diagram is drawn inline as SVG by whoever calls (this crate doesn't
//! draw).

use pulldown_cmark::{html, CodeBlockKind, Event, Options, Parser, Tag, TagEnd};

use crate::meta::{self, FrontMatter, Link};

/// What a `[[link]]` becomes outside the notebook.
pub enum LinkOut {
    /// A link to this (relative) path or URL.
    Href(String),
    /// Just its text.
    Text,
    /// An embed's content, in place: a diagram's (portable) source.
    Diagram(String),
    /// An embed of a note: its text, in place.
    Note(String),
}

/// `text` with its `[[links]]` and `![[embeds]]` rewritten by `out`.
pub fn to_markdown(text: &str, out: &dyn Fn(&Link) -> LinkOut) -> String {
    let fence_lines: std::collections::HashSet<usize> = {
        let prose: std::collections::HashSet<usize> = meta::prose_lines(text).map(|(n, _)| n).collect();
        (0..text.lines().count()).filter(|n| !prose.contains(n)).collect()
    };
    let mut lines = Vec::new();
    for (n, line) in text.lines().enumerate() {
        if fence_lines.contains(&n) {
            lines.push(line.to_string());
            continue;
        }
        let links = meta::links_in_line(line, n);
        if links.iter().all(|l| l.markdown) {
            lines.push(line.to_string());
            continue;
        }
        let chars: Vec<char> = line.chars().collect();
        let mut s = String::new();
        let mut at = 0;
        let alone = links.len() == 1 && line.trim() == chars[links[0].cols.clone()].iter().collect::<String>().trim();
        for l in links.iter().filter(|l| !l.markdown) {
            s.extend(chars[at..l.cols.start].iter());
            let shown = l.alias.clone().unwrap_or_else(|| l.label());
            match out(l) {
                LinkOut::Href(href) => s.push_str(&format!("[{shown}]({})", href.replace(' ', "%20"))),
                LinkOut::Text => s.push_str(&shown),
                LinkOut::Diagram(source) if l.embed => {
                    let block = format!("```mermaid\n{}\n```", source.trim_end());
                    if alone {
                        s.push_str(&block);
                    } else {
                        s.push_str(&format!("\n\n{block}\n\n"));
                    }
                }
                LinkOut::Note(body) if l.embed => {
                    let quoted: Vec<String> = body.trim_end().lines().map(|b| format!("> {b}")).collect();
                    s.push_str(&quoted.join("\n"));
                }
                LinkOut::Diagram(_) | LinkOut::Note(_) => s.push_str(&shown),
            }
            at = l.cols.end;
        }
        s.extend(chars[at..].iter());
        lines.push(s);
    }
    let mut out = lines.join("\n");
    if text.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// The colours the HTML page uses.
#[derive(Debug, Clone)]
pub struct Palette {
    pub bg: String,
    pub fg: String,
    pub muted: String,
    pub accent: String,
    pub code_bg: String,
    pub rule: String,
}

impl Palette {
    pub fn light() -> Palette {
        Palette { bg: "#ffffff".into(), fg: "#1a1a1d".into(), muted: "#5c5f66".into(), accent: "#c2531b".into(), code_bg: "#f2f3f5".into(), rule: "#d6d8dd".into() }
    }
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// `markdown` (already through `to_markdown`) as one HTML page titled
/// `title`; ```mermaid blocks are drawn by `svg` (left as code when it
/// can't).
pub fn to_html(markdown: &str, title: &str, palette: &Palette, svg: &dyn Fn(&str) -> Option<String>) -> String {
    let fm = FrontMatter::parse(markdown);
    let body_start = fm.lines.as_ref().map(|r| markdown.lines().take(r.end).map(|l| l.len() + 1).sum::<usize>()).unwrap_or(0).min(markdown.len());
    let body = &markdown[body_start..];
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_TASKLISTS);
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_FOOTNOTES);
    let mut events = Vec::new();
    let mut in_mermaid: Option<String> = None;
    for event in Parser::new_ext(body, opts) {
        match (&mut in_mermaid, event) {
            (None, Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(info)))) if info.split_whitespace().next() == Some("mermaid") => in_mermaid = Some(String::new()),
            (Some(src), Event::Text(t)) => src.push_str(&t),
            (Some(_), Event::End(TagEnd::CodeBlock)) => {
                let src = in_mermaid.take().unwrap_or_default();
                match svg(&src) {
                    Some(drawn) => events.push(Event::Html(format!("<figure class=\"diagram\">{drawn}</figure>\n").into())),
                    None => events.push(Event::Html(format!("<pre><code class=\"language-mermaid\">{}</code></pre>\n", escape(&src)).into())),
                }
            }
            (Some(_), _) => {}
            (None, e) => events.push(e),
        }
    }
    let mut html_body = String::new();
    html::push_html(&mut html_body, events.into_iter());
    let tags = if fm.tags.is_empty() { String::new() } else { format!("<p class=\"tags\">{}</p>\n", fm.tags.iter().map(|t| format!("#{}", escape(t))).collect::<Vec<_>>().join(" ")) };
    let p = palette;
    format!(
        "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n<title>{title}</title>\n<style>\n\
body {{ margin: 0; background: {bg}; color: {fg}; font: 16px/1.6 'IBM Plex Sans', -apple-system, 'Segoe UI', sans-serif; }}\n\
main {{ max-width: 860px; margin: 0 auto; padding: 48px 24px 96px; }}\n\
h1, h2, h3 {{ line-height: 1.25; }} h1 {{ border-bottom: 1px solid {rule}; padding-bottom: 8px; }} h2 {{ border-bottom: 1px solid {rule}; padding-bottom: 4px; }}\n\
a {{ color: {accent}; }} .tags {{ color: {muted}; font-size: 14px; }}\n\
code {{ font-family: 'JetBrains Mono', Consolas, monospace; font-size: 0.9em; background: {code_bg}; padding: 1px 5px; border-radius: 3px; }}\n\
pre {{ background: {code_bg}; padding: 12px 16px; border-radius: 6px; overflow-x: auto; }} pre code {{ background: none; padding: 0; }}\n\
table {{ border-collapse: collapse; margin: 12px 0; }} th, td {{ border: 1px solid {rule}; padding: 4px 10px; }} th {{ background: {code_bg}; }}\n\
blockquote {{ margin: 12px 0; padding: 4px 16px; border-left: 3px solid {accent}; color: {muted}; }}\n\
img, figure.diagram svg {{ max-width: 100%; height: auto; }} figure.diagram {{ margin: 16px 0; }}\n\
li input[type=checkbox] {{ margin-right: 6px; }}\n\
@media print {{ body {{ background: #fff; }} main {{ padding: 0; }} }}\n\
</style>\n</head>\n<body>\n<main>\n{tags}{html_body}</main>\n</body>\n</html>\n",
        title = escape(title),
        bg = p.bg,
        fg = p.fg,
        muted = p.muted,
        accent = p.accent,
        code_bg = p.code_bg,
        rule = p.rule,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn out(l: &Link) -> LinkOut {
        match l.target.as_str() {
            "ADR 007" => LinkOut::Href("adr-007.md".into()),
            "TC flow" => LinkOut::Diagram("flowchart TD\n  a --> b".into()),
            _ => LinkOut::Text,
        }
    }

    #[test]
    fn links_become_ordinary_links_and_embeds_their_content() {
        let text = "See [[ADR 007|the decision]] and [[Nowhere]].\n\n![[TC flow]]\n\n```\n[[left alone]]\n```\n";
        let md = to_markdown(text, &out);
        assert_eq!(md, "See [the decision](adr-007.md) and Nowhere.\n\n```mermaid\nflowchart TD\n  a --> b\n```\n\n```\n[[left alone]]\n```\n");
    }

    #[test]
    fn html_is_one_page_with_diagrams_drawn_in() {
        let md = "---\ntags: [bench]\n---\n# Title\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n```mermaid\nflowchart TD\n```\n\n```mermaid\nbroken\n```\n";
        let page = to_html(md, "Title & co", &Palette::light(), &|src: &str| src.starts_with("flowchart").then(|| "<svg>drawn</svg>".to_string()));
        assert!(page.starts_with("<!doctype html>"));
        assert!(page.contains("<title>Title &amp; co</title>"));
        assert!(page.contains("<p class=\"tags\">#bench</p>"));
        assert!(page.contains("<h1>Title</h1>"));
        assert!(page.contains("<table>"));
        assert!(page.contains("<figure class=\"diagram\"><svg>drawn</svg></figure>"));
        assert!(page.contains("<code class=\"language-mermaid\">broken\n</code>"));
        assert!(!page.contains("tags: [bench]"));
    }
}
