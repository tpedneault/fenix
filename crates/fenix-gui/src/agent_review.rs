//! *Agent changes* (`SPC a r`): every file an agent's thread changed, hunk
//! by hunk, against what the file was before the agent touched it -- to
//! keep or to reject. `app/agents/review.rs` reads the files and does what
//! the keys ask; this lays the page out, reads its keys, and works out
//! what keeping or rejecting a hunk makes of the texts, as plain data.

use crate::page::{fit, frame, Grid, Key, Page, Role};
use fenix_diff::{align, BlockKind};
use std::ops::Range;
use std::path::PathBuf;

/// What a file was before the agent changed it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Baseline {
    Text(String),
    /// It didn't exist: the agent made it.
    Absent,
    /// Changed during a turn without being announced, with nothing to
    /// compare it to.
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hunk {
    /// Lines of the baseline it replaces, and of the file now.
    pub old: Range<usize>,
    pub new: Range<usize>,
    pub removed: Vec<String>,
    pub added: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ReviewFile {
    pub path: PathBuf,
    /// As shown: relative to the project.
    pub name: String,
    pub baseline: Baseline,
    /// The file now; `None` when it's gone.
    pub current: Option<String>,
    pub hunks: Vec<Hunk>,
}

impl ReviewFile {
    pub fn new(path: PathBuf, name: String, baseline: Baseline, current: Option<String>) -> Self {
        let hunks = match (&baseline, &current) {
            (Baseline::Text(old), Some(new)) => hunks(old, new),
            (Baseline::Absent, Some(new)) => hunks("", new),
            (Baseline::Text(old), None) => hunks(old, ""),
            _ => Vec::new(),
        };
        ReviewFile { path, name, baseline, current, hunks }
    }

    /// Whether there's anything left to review.
    pub fn changed(&self) -> bool {
        match (&self.baseline, &self.current) {
            // Line by line: a file the agent left only with other line
            // endings has nothing in it to keep or reject.
            (Baseline::Text(_), Some(_)) => !self.hunks.is_empty(),
            (Baseline::Absent, None) => false,
            _ => true,
        }
    }

    fn summary(&self) -> String {
        let (plus, minus) = self.hunks.iter().fold((0, 0), |(p, m), h| (p + h.added.len(), m + h.removed.len()));
        match (&self.baseline, &self.current) {
            (Baseline::Absent, _) => format!("new · +{plus}"),
            (_, None) => "deleted".to_string(),
            (Baseline::Unknown, _) => "changed, nothing to compare it to".to_string(),
            _ => format!("+{plus} −{minus}"),
        }
    }
}

fn lines(text: &str) -> Vec<String> {
    text.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l).to_string()).collect()
}

/// The hunks between `old` and `new`, line by line.
pub fn hunks(old: &str, new: &str) -> Vec<Hunk> {
    let (a, b) = (lines(old), lines(new));
    align(old, new)
        .blocks
        .into_iter()
        .filter(|block| block.kind != BlockKind::Equal)
        .map(|block| Hunk { removed: a[block.old.clone()].to_vec(), added: b[block.new.clone()].to_vec(), old: block.old, new: block.new })
        .collect()
}

/// The line ending `text` uses: Windows' when it has any, else `\n`.
fn line_ending(text: &str) -> &'static str {
    if text.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    }
}

/// `text` with lines `range` replaced, its line endings kept.
fn splice(text: &str, range: Range<usize>, with: &[String]) -> String {
    let mut ls = lines(text);
    ls.splice(range, with.iter().cloned());
    ls.join(line_ending(text))
}

/// Keeping hunk `i`: the baseline takes the change, so it's no longer
/// one. The new baseline.
pub fn keep(baseline: &str, current: &str, i: usize) -> Option<String> {
    let hunk = hunks(baseline, current).into_iter().nth(i)?;
    Some(splice(baseline, hunk.old, &hunk.added))
}

/// Rejecting hunk `i`: the file goes back to the baseline there. The new
/// text of the file.
pub fn reject(baseline: &str, current: &str, i: usize) -> Option<String> {
    let hunk = hunks(baseline, current).into_iter().nth(i)?;
    Some(splice(current, hunk.new, &hunk.removed))
}

pub struct ReviewPage {
    pub thread: fenix_acp::hub::ThreadId,
    pub files: Vec<ReviewFile>,
    /// The focused row: a file, and one of its hunks (`None`: the file's
    /// own row).
    pub focus: (usize, Option<usize>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    None,
    Close,
    Refresh,
    Keep { file: usize, hunk: usize },
    Reject { file: usize, hunk: usize },
    KeepFile(usize),
    RejectFile(usize),
    /// The file, at a line (from 1).
    Open { file: usize, line: usize },
}

impl ReviewPage {
    pub fn new(thread: fenix_acp::hub::ThreadId, files: Vec<ReviewFile>) -> Self {
        let mut page = ReviewPage { thread, files, focus: (0, None) };
        page.focus = page.rows().first().copied().unwrap_or((0, None));
        page
    }

    /// Every row, in order: each file, then its hunks.
    fn rows(&self) -> Vec<(usize, Option<usize>)> {
        let mut rows = Vec::new();
        for (f, file) in self.files.iter().enumerate() {
            rows.push((f, None));
            rows.extend((0..file.hunks.len()).map(|h| (f, Some(h))));
        }
        rows
    }

    /// New files after a change: the focus stays on the same place, or
    /// the nearest one left.
    pub fn set_files(&mut self, files: Vec<ReviewFile>) {
        let (f, h) = self.focus;
        self.files = files;
        let rows = self.rows();
        if !rows.contains(&self.focus) {
            let f = f.min(self.files.len().saturating_sub(1));
            let h = h.and_then(|h| self.files.get(f).filter(|file| !file.hunks.is_empty()).map(|file| h.min(file.hunks.len() - 1)));
            self.focus = if rows.contains(&(f, h)) { (f, h) } else { rows.first().copied().unwrap_or((0, None)) };
        }
    }

    pub fn key(&mut self, key: Key) -> Action {
        let rows = self.rows();
        let at = rows.iter().position(|r| *r == self.focus).unwrap_or(0);
        let (f, h) = self.focus;
        match key {
            Key::Char('q') | Key::Escape => return Action::Close,
            Key::Char('j') | Key::Down => self.focus = rows.get(at + 1).copied().unwrap_or(self.focus),
            Key::Char('k') | Key::Up => self.focus = rows.get(at.saturating_sub(1)).copied().unwrap_or(self.focus),
            Key::Char('J') => self.focus = rows[at..].iter().skip(1).find(|r| r.1.is_none()).copied().unwrap_or(self.focus),
            Key::Char('K') => self.focus = rows[..at].iter().rev().find(|r| r.1.is_none()).copied().unwrap_or(self.focus),
            Key::Char('g') => self.focus = rows.first().copied().unwrap_or(self.focus),
            Key::Char('G') => self.focus = rows.last().copied().unwrap_or(self.focus),
            Key::Char('r') => return Action::Refresh,
            Key::Char('a') => {
                return match h {
                    Some(hunk) => Action::Keep { file: f, hunk },
                    None if !self.files.is_empty() => Action::KeepFile(f),
                    None => Action::None,
                }
            }
            Key::Char('x') => {
                return match h {
                    Some(hunk) => Action::Reject { file: f, hunk },
                    None if !self.files.is_empty() => Action::RejectFile(f),
                    None => Action::None,
                }
            }
            Key::Char('A') if !self.files.is_empty() => return Action::KeepFile(f),
            Key::Char('X') if !self.files.is_empty() => return Action::RejectFile(f),
            Key::Enter if !self.files.is_empty() => {
                let line = h.and_then(|h| self.files[f].hunks.get(h)).map_or(1, |hunk| hunk.new.start + 1);
                return Action::Open { file: f, line };
            }
            _ => {}
        }
        Action::None
    }
}

pub fn layout(page: &ReviewPage, title: &str, cols: usize) -> Page {
    let (left, width) = frame(cols, 120);
    let mut g = Grid::new();
    g.put(1, left, "Agent changes", Role::Title);
    g.put(1, left + 15, &fit(&format!("· {title}"), width.saturating_sub(15)), Role::Muted);
    g.rule(2, left..left + width);
    let mut y = 4;
    if page.files.is_empty() {
        g.put(y, left, "Nothing to review: the agent hasn't changed a file in this thread, or it's all been kept.", Role::Muted);
        y += 2;
    }
    for (f, file) in page.files.iter().enumerate() {
        if page.focus == (f, None) {
            g.focus(y, left..left + width);
        }
        let x = g.put(y, left, &fit(&file.name, width.saturating_sub(24)), Role::Title);
        g.put(y, x + 2, &file.summary(), if matches!(file.baseline, Baseline::Unknown) { Role::Warn } else { Role::Muted });
        y += 1;
        for (h, hunk) in file.hunks.iter().enumerate() {
            if page.focus == (f, Some(h)) {
                g.focus(y, left..left + width);
            }
            g.put(y, left + 2, &format!("@@ line {}", hunk.new.start + 1), Role::Accent);
            y += 1;
            for line in hunk.removed.iter().take(40) {
                g.put(y, left + 2, &fit(&format!("-{line}"), width - 2), Role::Bad);
                y += 1;
            }
            for line in hunk.added.iter().take(40) {
                g.put(y, left + 2, &fit(&format!("+{line}"), width - 2), Role::Good);
                y += 1;
            }
            if hunk.removed.len() > 40 || hunk.added.len() > 40 {
                g.put(y, left + 2, "… (Enter opens the file)", Role::Muted);
                y += 1;
            }
        }
        y += 1;
    }
    g.keys(left, width, &[("j k", "move"), ("J K", "files"), ("a", "keep"), ("x", "reject"), ("A X", "the whole file"), ("Enter", "open"), ("r", "refresh"), ("q", "close")]);
    g.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hunks_keep_and_reject_change_the_right_text() {
        let old = "a\nb\nc\nd\ne\n";
        let new = "a\nB\nc\nd\nE\nf\n";
        let hs = hunks(old, new);
        assert_eq!(hs.len(), 2);
        assert_eq!((hs[0].removed.clone(), hs[0].added.clone()), (vec!["b".to_string()], vec!["B".to_string()]));
        // Keeping the first: the baseline has B, the second is still a change.
        let kept = keep(old, new, 0).unwrap();
        assert_eq!(kept, "a\nB\nc\nd\ne\n");
        assert_eq!(hunks(&kept, new).len(), 1);
        // Rejecting the second: the file has e back, and no f.
        let rejected = reject(old, new, 1).unwrap();
        assert_eq!(rejected, "a\nB\nc\nd\ne\n");
        assert_eq!(hunks(old, &rejected).len(), 1);
        assert_eq!(keep(old, new, 5), None);
    }

    #[test]
    fn keeping_and_rejecting_keep_windows_line_endings() {
        let old = "def veg():\r\n    return 'carrot'\r\n\r\nprint(veg())\r\n";
        let new = "def veg():\r\n    return 'potato'\r\n\r\nprint(veg())\r\n";
        assert_eq!(reject(old, new, 0).unwrap(), old, "byte for byte");
        assert_eq!(keep(old, new, 0).unwrap(), new);
        // Only the endings changed: nothing to review.
        let endings = ReviewFile::new("/p/e.py".into(), "e.py".into(), Baseline::Text(old.into()), Some(old.replace("\r\n", "\n")));
        assert!(!endings.changed());
    }

    #[test]
    fn a_new_file_and_a_deleted_one_are_one_hunk_each() {
        let made = ReviewFile::new("/p/n.rs".into(), "n.rs".into(), Baseline::Absent, Some("x\ny\n".into()));
        assert!(made.changed() && made.summary().starts_with("new"));
        let gone = ReviewFile::new("/p/g.rs".into(), "g.rs".into(), Baseline::Text("x\n".into()), None);
        assert_eq!(gone.summary(), "deleted");
        let same = ReviewFile::new("/p/s.rs".into(), "s.rs".into(), Baseline::Text("x\n".into()), Some("x\n".into()));
        assert!(!same.changed());
    }

    #[test]
    fn keys_walk_rows_and_ask_for_hunks_and_files() {
        let files = vec![
            ReviewFile::new("/p/a.rs".into(), "a.rs".into(), Baseline::Text("1\n2\n3\n".into()), Some("1\nTWO\n3\n".into())),
            ReviewFile::new("/p/b.rs".into(), "b.rs".into(), Baseline::Absent, Some("new\n".into())),
        ];
        let mut page = ReviewPage::new(fenix_acp::hub::ThreadId("t".into()), files);
        assert_eq!(page.focus, (0, None));
        page.key(Key::Char('j'));
        assert_eq!(page.key(Key::Char('x')), Action::Reject { file: 0, hunk: 0 });
        assert_eq!(page.key(Key::Char('a')), Action::Keep { file: 0, hunk: 0 });
        assert_eq!(page.key(Key::Enter), Action::Open { file: 0, line: 2 });
        page.key(Key::Char('J'));
        assert_eq!(page.focus, (1, None));
        assert_eq!(page.key(Key::Char('X')), Action::RejectFile(1));
        let text = layout(&page, "Fix it", 100).all_text();
        assert!(text.contains("a.rs  +1 −1") && text.contains("-2") && text.contains("+TWO") && text.contains("b.rs  new · +1"), "{text}");
        page.set_files(vec![]);
        assert_eq!(page.focus, (0, None));
        assert!(layout(&page, "Fix it", 100).all_text().contains("Nothing to review"));
    }
}
