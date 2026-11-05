//! The explorer's preview (`p`): a page beside the listing showing what
//! the selected row holds -- a file's text, highlighted; a picture; a
//! diagram an extension draws; a folder's facts -- following the
//! selection as it moves. What's shown is read by the host
//! (`app/explorer_host.rs`); this lays it out.

use std::ops::Range;
use std::path::PathBuf;
use std::time::SystemTime;

use crate::explorer_page::{age_text, size_text};
use crate::page::{fit, Grid, ImageKey, Page, Role};

/// What the preview shows.
#[derive(Debug, Clone, PartialEq)]
pub enum Content {
    /// Nothing selected.
    Nothing,
    /// A text file's first lines, with each line's colours as (columns,
    /// syntax capture); `cut` when there was more than was read.
    Text { lines: Vec<String>, colours: Vec<Vec<(Range<usize>, &'static str)>>, language: Option<String>, cut: bool },
    /// A picture the page can draw.
    Picture,
    /// A diagram an extension drew from the file, by its picture key.
    Diagram(u64),
    /// A folder: what's in it, folders first.
    Folder { folders: usize, files: usize, bytes: u64, names: Vec<(String, bool)> },
    /// Bytes that aren't text.
    Binary,
    /// On a share: not read on the UI thread.
    Remote,
    /// It couldn't be read, and why.
    Unreadable(String),
}

/// The preview page: which path, what it holds, how far it's scrolled.
#[derive(Debug, Clone, PartialEq)]
pub struct PreviewPage {
    pub path: Option<PathBuf>,
    pub content: Content,
    pub size: Option<u64>,
    pub modified: Option<SystemTime>,
    /// The first text line (or folder entry) shown.
    pub scroll: usize,
}

impl Default for PreviewPage {
    fn default() -> Self {
        PreviewPage { path: None, content: Content::Nothing, size: None, modified: None, scroll: 0 }
    }
}

/// What a key on the preview page asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    None,
    Close,
    /// `P`: open the file as the editor would.
    Open,
}

impl PreviewPage {
    /// A key while the preview itself has the keyboard: it scrolls,
    /// opens what it shows, or closes.
    pub fn key(&mut self, key: crate::page::Key) -> Action {
        use crate::page::Key;
        match key {
            Key::Char('j' | 'J') | Key::Down => self.scroll_by(1),
            Key::Char('k' | 'K') | Key::Up => self.scroll_by(-1),
            Key::Char('P') | Key::Enter => return Action::Open,
            Key::Char('q' | 'p') | Key::Escape => return Action::Close,
            _ => {}
        }
        Action::None
    }

    /// How many lines the content has to scroll through.
    fn length(&self) -> usize {
        match &self.content {
            Content::Text { lines, .. } => lines.len(),
            Content::Folder { names, .. } => names.len(),
            _ => 0,
        }
    }

    /// Scrolls by `lines` (down when positive), within the content.
    pub fn scroll_by(&mut self, lines: isize) {
        self.scroll = self.scroll.saturating_add_signed(lines).min(self.length().saturating_sub(1));
    }
}

/// Lays the preview out `cols` wide and, when known, `height` tall.
pub fn layout(page: &PreviewPage, cols: usize, height: Option<usize>) -> Page {
    let mut g = Grid::new();
    let (left, width) = (1, cols.saturating_sub(2).max(8));
    let Some(path) = &page.path else {
        g.put(1, left, "Nothing selected", Role::Muted);
        return g.finish();
    };
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string());
    g.put(0, left, &fit(&name, width), Role::Title);
    let mut facts: Vec<String> = Vec::new();
    match &page.content {
        Content::Text { lines, language, cut, .. } => {
            facts.extend(language.clone());
            facts.push(format!("{}{} line{}", if *cut { "first " } else { "" }, lines.len(), if lines.len() == 1 { "" } else { "s" }));
        }
        Content::Folder { folders, files, bytes, .. } => {
            facts.push(format!("{folders} folder{}", if *folders == 1 { "" } else { "s" }));
            facts.push(format!("{files} file{}", if *files == 1 { "" } else { "s" }));
            if *files > 0 {
                facts.push(size_text(*bytes));
            }
        }
        _ => {}
    }
    if let (Some(size), false) = (page.size, matches!(page.content, Content::Folder { .. })) {
        facts.push(size_text(size));
    }
    let age = age_text(page.modified, SystemTime::now());
    if !age.is_empty() {
        facts.push(format!("{age} ago").replace("now ago", "just now"));
    }
    g.put(1, left, &fit(&facts.join(" · "), width), Role::Muted);
    g.rule(2, left..left + width);
    let top = 3;
    // The lines between the header and the key strip.
    let room = height.map(|h| h.saturating_sub(top + 3)).unwrap_or(usize::MAX).max(1);

    match &page.content {
        Content::Nothing => {}
        Content::Text { lines, colours, .. } => {
            let gutter = lines.len().max(1).to_string().len();
            for (i, n) in (page.scroll..lines.len()).take(room).enumerate() {
                let y = top + i;
                g.put(y, left, &format!("{:>gutter$}", n + 1), Role::Muted);
                let x = left + gutter + 1;
                let text = fit(&lines[n], width.saturating_sub(gutter + 1));
                let chars: Vec<char> = text.chars().collect();
                // Each coloured stretch, and plain text only between them:
                // where two spans overlap the renderer keeps the first, so
                // a plain span under the colours would hide them.
                let mut stretches: Vec<_> = colours.get(n).into_iter().flatten().collect();
                stretches.sort_by_key(|(cols, _)| cols.start);
                let mut at = 0;
                for (cols, capture) in stretches {
                    let (a, b) = (cols.start.max(at).min(chars.len()), cols.end.min(chars.len()));
                    if a >= b {
                        continue;
                    }
                    if a > at {
                        g.put(y, x + at, &chars[at..a].iter().collect::<String>(), Role::Text);
                    }
                    g.put(y, x + a, &chars[a..b].iter().collect::<String>(), Role::Syntax(capture));
                    at = b;
                }
                if at < chars.len() {
                    g.put(y, x + at, &chars[at..].iter().collect::<String>(), Role::Text);
                }
            }
        }
        Content::Picture => {
            let rows = height.map(|h| h.saturating_sub(top + 3)).unwrap_or(20).max(1);
            g.image(top, left, rows, width, ImageKey::File(path.clone()));
        }
        Content::Diagram(key) => {
            let rows = height.map(|h| h.saturating_sub(top + 3)).unwrap_or(20).max(1);
            g.image(top, left, rows, width, ImageKey::Diagram(*key));
        }
        Content::Folder { names, .. } => {
            if names.is_empty() {
                g.put(top, left, "empty", Role::Muted);
            }
            for (i, (name, dir)) in names.iter().skip(page.scroll).take(room).enumerate() {
                let shown = if *dir { format!("{name}/") } else { name.clone() };
                g.put(top + i, left, &fit(&shown, width), if *dir { Role::Title } else { Role::Text });
            }
        }
        Content::Binary => {
            g.put(top, left, &fit("Not text. P opens it as the editor would; SPC e o with the system.", width), Role::Muted);
        }
        Content::Remote => {
            g.put(top, left, &fit("On a share: not read ahead. P opens it.", width), Role::Muted);
        }
        Content::Unreadable(why) => {
            g.put(top, left, &fit(&format!("Can't read it: {why}"), width), Role::Bad);
        }
    }
    if let Some(h) = height {
        g.reserve(h.saturating_sub(3));
    }
    g.keys(left, width, &[("J K", "scroll"), ("P", "open"), ("p", "close")]);
    g.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(lines: &[&str]) -> PreviewPage {
        PreviewPage {
            path: Some(PathBuf::from("/x/main.rs")),
            content: Content::Text {
                lines: lines.iter().map(|l| l.to_string()).collect(),
                colours: vec![vec![(0..2, "keyword")]; lines.len()],
                language: Some("Rust".into()),
                cut: false,
            },
            size: Some(1126),
            modified: None,
            scroll: 0,
        }
    }

    #[test]
    fn text_is_numbered_and_coloured_under_a_header() {
        let page = layout(&text(&["fn main() {}", "}"]), 60, Some(12));
        let lines: Vec<&str> = page.text.lines().collect();
        assert!(lines[0].contains("main.rs"));
        assert!(lines[1].contains("Rust") && lines[1].contains("2 lines") && lines[1].contains("1.1 K"), "{}", lines[1]);
        assert!(lines[3].contains("1 fn main() {}"), "{}", page.text);
        assert!(page.spans.iter().any(|s| s.line == 3 && s.role == Role::Syntax("keyword")));
        // No plain span under the colours: the renderer keeps the first of
        // two that overlap, so it would hide them.
        let row: Vec<_> = page.spans.iter().filter(|s| s.line == 3).collect();
        for (i, s) in row.iter().enumerate() {
            assert!(row[i + 1..].iter().all(|t| t.cols.start >= s.cols.end || t.cols.end <= s.cols.start), "overlapping spans: {row:?}");
        }
        assert!(page.text.lines().last().unwrap().contains("J K scroll"));
    }

    #[test]
    fn scrolling_stays_within_the_text() {
        let mut page = text(&["a", "b", "c"]);
        page.scroll_by(10);
        assert_eq!(page.scroll, 2);
        page.scroll_by(-10);
        assert_eq!(page.scroll, 0);
        page.scroll_by(1);
        assert!(layout(&page, 40, Some(12)).text.lines().nth(3).unwrap().contains("2 b"));
    }

    #[test]
    fn a_picture_and_a_folder_are_shown_for_what_they_are() {
        let picture = PreviewPage { path: Some(PathBuf::from("/x/a.png")), content: Content::Picture, ..Default::default() };
        let page = layout(&picture, 40, Some(20));
        assert_eq!(page.images.len(), 1);
        assert_eq!(page.images[0].key, ImageKey::File(PathBuf::from("/x/a.png")));

        let folder = PreviewPage {
            path: Some(PathBuf::from("/x/src")),
            content: Content::Folder { folders: 1, files: 2, bytes: 2048, names: vec![("sub".into(), true), ("a.rs".into(), false)] },
            ..Default::default()
        };
        let text = layout(&folder, 40, Some(20)).text;
        assert!(text.contains("1 folder · 2 files · 2.0 K") && text.contains("sub/") && text.contains("a.rs"), "{text}");
    }
}
