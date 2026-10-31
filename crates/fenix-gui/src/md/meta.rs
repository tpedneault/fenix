//! What Fenix reads out of a note's text: its front matter (`tags`,
//! `about`, `title`), the `[[links]]` and `#tags` in its body, its
//! headings and its checkboxes. Line-based and forgiving -- a note is
//! something being typed, so half-finished syntax is the normal case.

use std::ops::Range;

/// The YAML block at the very top of a note, between two `---` lines.
/// Only the handful of keys Fenix uses are read; everything else is
/// left alone.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FrontMatter {
    pub tags: Vec<String>,
    pub about: Option<String>,
    pub title: Option<String>,
    /// The lines it spans, fences included; `None` when there's none.
    pub lines: Option<Range<usize>>,
}

impl FrontMatter {
    pub fn parse(text: &str) -> FrontMatter {
        let mut fm = FrontMatter::default();
        let lines: Vec<&str> = text.lines().collect();
        if lines.first().map(|l| l.trim_end()) != Some("---") {
            return fm;
        }
        let Some(end) = lines.iter().skip(1).position(|l| l.trim_end() == "---" || l.trim_end() == "...") else { return fm };
        let end = end + 1;
        fm.lines = Some(0..end + 1);
        let mut i = 1;
        while i < end {
            let line = lines[i];
            if let Some((key, value)) = line.split_once(':') {
                if !line.starts_with(' ') && !line.starts_with('-') {
                    let value = value.trim();
                    match key.trim() {
                        "tags" | "tag" => {
                            if value.is_empty() {
                                // A block list on the lines under it.
                                while i + 1 < end {
                                    let next = lines[i + 1].trim_start();
                                    let Some(item) = next.strip_prefix("- ") else { break };
                                    push_tag(&mut fm.tags, item);
                                    i += 1;
                                }
                            } else {
                                let inner = value.trim_start_matches('[').trim_end_matches(']');
                                for item in inner.split([',', ' ']) {
                                    push_tag(&mut fm.tags, item);
                                }
                            }
                        }
                        "about" | "project" => fm.about = Some(unquote(value)).filter(|v| !v.is_empty()),
                        "title" => fm.title = Some(unquote(value)).filter(|v| !v.is_empty()),
                        _ => {}
                    }
                }
            }
            i += 1;
        }
        fm
    }
}

fn unquote(s: &str) -> String {
    let s = s.trim();
    let s = s.strip_prefix('"').and_then(|s| s.strip_suffix('"')).or_else(|| s.strip_prefix('\'').and_then(|s| s.strip_suffix('\''))).unwrap_or(s);
    s.to_string()
}

fn push_tag(tags: &mut Vec<String>, raw: &str) {
    let tag = unquote(raw).trim_start_matches('#').trim().to_string();
    if !tag.is_empty() && !tags.iter().any(|t| t.eq_ignore_ascii_case(&tag)) {
        tags.push(tag);
    }
}

/// One `[[link]]`, `![[embed]]` or `[text](file.md)` in a note.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    /// What it points at: a note's name, a path, `mib:X`, ...
    pub target: String,
    /// `#Heading` after the target.
    pub heading: Option<String>,
    /// `|shown text`.
    pub alias: Option<String>,
    /// `![[...]]`: shown in place, not linked.
    pub embed: bool,
    /// `[text](path)` rather than `[[...]]`.
    pub markdown: bool,
    pub line: usize,
    /// Its columns (chars) on the line, brackets included.
    pub cols: Range<usize>,
}

impl Link {
    /// What the reader sees for it.
    pub fn label(&self) -> String {
        if let Some(alias) = &self.alias {
            return alias.clone();
        }
        match &self.heading {
            Some(h) if self.target.is_empty() => h.clone(),
            Some(h) => format!("{} › {h}", self.target),
            None => self.target.clone(),
        }
    }
}

/// Every link in `text`, outside code.
pub fn links(text: &str) -> Vec<Link> {
    let mut out = Vec::new();
    for (line_no, line) in prose_lines(text) {
        out.extend(links_in_line(line, line_no));
    }
    out
}

/// The links on one line of prose.
pub fn links_in_line(line: &str, line_no: usize) -> Vec<Link> {
    let chars: Vec<char> = line.chars().collect();
    let code = code_span_mask(&chars);
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if code[i] {
            i += 1;
            continue;
        }
        if chars[i] == '[' && chars.get(i + 1) == Some(&'[') {
            let embed = i > 0 && chars[i - 1] == '!';
            if let Some(close) = find_seq(&chars, i + 2, &[']', ']']) {
                let inner: String = chars[i + 2..close].iter().collect();
                if !inner.contains('[') && !inner.trim().is_empty() {
                    let (rest, alias) = match inner.split_once('|') {
                        Some((a, b)) => (a.to_string(), Some(b.trim().to_string())),
                        None => (inner.clone(), None),
                    };
                    let (target, heading) = match rest.split_once('#') {
                        Some((t, h)) if !t.contains("://") => (t.trim().to_string(), Some(h.trim().to_string()).filter(|h| !h.is_empty())),
                        _ => (rest.trim().to_string(), None),
                    };
                    let start = if embed { i - 1 } else { i };
                    out.push(Link { target, heading, alias, embed, markdown: false, line: line_no, cols: start..close + 2 });
                    i = close + 2;
                    continue;
                }
            }
        }
        if chars[i] == '[' && (i == 0 || chars[i - 1] != '[') {
            // [text](target) -- a Markdown link to a note or file.
            if let Some(close) = chars[i + 1..].iter().position(|&c| c == ']').map(|p| p + i + 1) {
                if chars.get(close + 1) == Some(&'(') {
                    if let Some(end) = chars[close + 2..].iter().position(|&c| c == ')').map(|p| p + close + 2) {
                        let text: String = chars[i + 1..close].iter().collect();
                        let target: String = chars[close + 2..end].iter().collect();
                        let target = target.trim().to_string();
                        let image = i > 0 && chars[i - 1] == '!';
                        if !target.is_empty() && !image {
                            let (t, heading) = match target.split_once('#') {
                                Some((t, h)) if !t.contains("://") => (t.to_string(), Some(h.to_string())),
                                _ => (target.clone(), None),
                            };
                            out.push(Link { target: t, heading, alias: Some(text), embed: false, markdown: true, line: line_no, cols: i..end + 1 });
                        }
                        i = end + 1;
                        continue;
                    }
                }
            }
        }
        i += 1;
    }
    out
}

fn find_seq(chars: &[char], from: usize, seq: &[char]) -> Option<usize> {
    (from..chars.len().saturating_sub(seq.len() - 1)).find(|&j| chars[j..].starts_with(seq))
}

/// Which chars of a line sit inside `` `code` ``.
fn code_span_mask(chars: &[char]) -> Vec<bool> {
    let mut mask = vec![false; chars.len()];
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '`' {
            let run = chars[i..].iter().take_while(|&&c| c == '`').count();
            let fence: Vec<char> = vec!['`'; run];
            if let Some(close) = find_seq(chars, i + run, &fence) {
                for m in mask.iter_mut().take(close + run).skip(i) {
                    *m = true;
                }
                i = close + run;
                continue;
            }
            i += run;
            continue;
        }
        i += 1;
    }
    mask
}

/// The lines of `text` that are prose: not front matter, not inside a
/// fenced code block. With their line numbers.
pub fn prose_lines(text: &str) -> impl Iterator<Item = (usize, &str)> {
    let fm_end = FrontMatter::parse(text).lines.map(|r| r.end).unwrap_or(0);
    let mut fence: Option<(char, usize)> = None;
    text.lines().enumerate().filter(move |(n, line)| {
        if *n < fm_end {
            return false;
        }
        let trimmed = line.trim_start();
        let marker = trimmed.chars().next().filter(|c| *c == '`' || *c == '~');
        if let Some(m) = marker {
            let run = trimmed.chars().take_while(|&c| c == m).count();
            if run >= 3 {
                match fence {
                    None => {
                        fence = Some((m, run));
                        return false;
                    }
                    Some((fm, fr)) if fm == m && run >= fr && trimmed[run..].trim().is_empty() => {
                        fence = None;
                        return false;
                    }
                    _ => {}
                }
            }
        }
        fence.is_none()
    })
}

/// The `#tags` on one line, with their columns (the `#` included).
pub fn inline_tags(line: &str) -> Vec<(Range<usize>, String)> {
    let chars: Vec<char> = line.chars().collect();
    let code = code_span_mask(&chars);
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let starts = chars[i] == '#' && !code[i] && (i == 0 || chars[i - 1].is_whitespace() || chars[i - 1] == '(');
        if starts {
            let word: String = chars[i + 1..].iter().take_while(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '/')).collect();
            let len = word.chars().count();
            if len > 0 && word.chars().any(|c| c.is_alphabetic()) {
                out.push((i..i + 1 + len, word));
                i += 1 + len;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// One `- [ ]` / `- [x]` line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Task {
    pub done: bool,
    pub text: String,
    pub line: usize,
    /// The char column of the mark inside the brackets.
    pub mark_col: usize,
}

pub fn task_on(line: &str, n: usize) -> Option<Task> {
    let indent = line.chars().take_while(|c| c.is_whitespace()).count();
    let rest: String = line.chars().skip(indent).collect();
    let mut chars = rest.chars();
    let bullet = chars.next()?;
    let marker_len = if matches!(bullet, '-' | '*' | '+') {
        1
    } else if bullet.is_ascii_digit() {
        let digits = rest.chars().take_while(|c| c.is_ascii_digit()).count();
        let after = rest.chars().nth(digits)?;
        if !matches!(after, '.' | ')') {
            return None;
        }
        digits + 1
    } else {
        return None;
    };
    let after: String = rest.chars().skip(marker_len).collect();
    let after = after.strip_prefix(' ')?;
    let mut ac = after.chars();
    if ac.next()? != '[' {
        return None;
    }
    let mark = ac.next()?;
    if ac.next()? != ']' || !matches!(mark, ' ' | 'x' | 'X') {
        return None;
    }
    let text: String = ac.collect();
    Some(Task { done: mark != ' ', text: text.trim().to_string(), line: n, mark_col: indent + marker_len + 2 })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn front_matter_reads_flow_and_block_lists() {
        let fm = FrontMatter::parse("---\ntags: [mission, tc]\nabout: fenix-sat\n---\n# T\n");
        assert_eq!(fm.tags, vec!["mission", "tc"]);
        assert_eq!(fm.about.as_deref(), Some("fenix-sat"));
        assert_eq!(fm.lines, Some(0..4));
        let fm = FrontMatter::parse("---\ntags:\n  - a\n  - \"#b\"\ntitle: 'Hi'\n---\n");
        assert_eq!(fm.tags, vec!["a", "b"]);
        assert_eq!(fm.title.as_deref(), Some("Hi"));
        assert_eq!(FrontMatter::parse("# no front matter\n"), FrontMatter::default());
    }

    #[test]
    fn wiki_links_embeds_headings_aliases_and_markdown_links() {
        let l = links("See [[ADR 007#Decision|the decision]] and ![[TC flow]].\nAlso [spec](docs/spec.md#intro), not ![img](a.png)\n");
        assert_eq!(l.len(), 3);
        assert_eq!(l[0].target, "ADR 007");
        assert_eq!(l[0].heading.as_deref(), Some("Decision"));
        assert_eq!(l[0].alias.as_deref(), Some("the decision"));
        assert_eq!(l[0].cols, 4..37);
        assert!(l[1].embed);
        assert_eq!(l[1].target, "TC flow");
        assert!(l[2].markdown);
        assert_eq!(l[2].target, "docs/spec.md");
        assert_eq!(l[2].line, 1);
    }

    #[test]
    fn links_in_code_are_not_links() {
        let text = "`[[not a link]]` but [[a link]]\n```\n[[in a fence]]\n```\n";
        let l = links(text);
        assert_eq!(l.len(), 1);
        assert_eq!(l[0].target, "a link");
    }

    #[test]
    fn tags_and_checkboxes_on_a_line() {
        let tags: Vec<String> = inline_tags("retry #tc and #bench/uplink but not `#code` or #123").into_iter().map(|(_, t)| t).collect();
        assert_eq!(tags, vec!["tc", "bench/uplink"]);
        let t = task_on("- [ ] open item", 3).unwrap();
        assert!(!t.done && t.text == "open item" && t.mark_col == 3 && t.line == 3);
        assert!(task_on("  - [x] done", 0).is_some_and(|t| t.done && t.mark_col == 5));
        assert_eq!(task_on("1. [ ] numbered", 0).map(|t| t.mark_col), Some(4));
        assert!(task_on("- not a task", 0).is_none());
    }
}
