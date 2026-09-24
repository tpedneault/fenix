//! The review inbox (`SPC g M`): the requests waiting on your review
//! first, then yours and every open one (`Tab`), each with a dot when
//! something happened since you last opened it. The row under the
//! cursor opens up to say where it's going and what it's about. `Enter`
//! reviews it here, `w` in a worktree of its own.

use std::path::PathBuf;

use fenix_forge::{MergeRequest, MrFilter, PipelineStatus};

use crate::git_status::count;
use crate::page::{fit, frame, Grid, Key, Page, Role};

#[derive(Debug, Clone)]
pub struct Entry {
    pub request: MergeRequest,
    /// Something changed since you last opened it.
    pub new: bool,
    /// Comments of yours waiting to be submitted.
    pub pending: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InboxAction {
    None,
    Refresh,
    Close,
    Open(u64),
    Worktree(u64),
    CheckOut(u64),
    Browser(String),
}

pub struct Inbox {
    pub root: PathBuf,
    pub forge: String,
    pub project: String,
    pub filter: MrFilter,
    pub entries: Option<Vec<Entry>>,
    pub cursor: usize,
    pub message: Option<(String, bool)>,
    pub loading: bool,
}

impl Inbox {
    pub fn new(root: PathBuf, forge: String, project: String) -> Self {
        Inbox { root, forge, project, filter: MrFilter::ReviewRequested, entries: None, cursor: 0, message: None, loading: false }
    }

    pub fn set_entries(&mut self, entries: Vec<Entry>) {
        self.loading = false;
        let before = self.entries.as_ref().and_then(|e| e.get(self.cursor)).map(|e| e.request.number);
        self.cursor = before.and_then(|n| entries.iter().position(|e| e.request.number == n)).unwrap_or(0);
        self.entries = Some(entries);
    }

    fn selected(&self) -> Option<&Entry> {
        self.entries.as_ref()?.get(self.cursor)
    }

    pub fn key(&mut self, key: Key) -> InboxAction {
        let n = self.entries.as_ref().map(Vec::len).unwrap_or(0);
        match key {
            Key::Down | Key::Char('j') => self.cursor = (self.cursor + 1).min(n.saturating_sub(1)),
            Key::Up | Key::Char('k') => self.cursor = self.cursor.saturating_sub(1),
            Key::Tab => {
                self.filter = match self.filter {
                    MrFilter::ReviewRequested => MrFilter::Mine,
                    MrFilter::Mine => MrFilter::AllOpen,
                    _ => MrFilter::ReviewRequested,
                };
                self.cursor = 0;
                return InboxAction::Refresh;
            }
            Key::Enter => return self.selected().map(|e| InboxAction::Open(e.request.number)).unwrap_or(InboxAction::None),
            Key::Char('w') => return self.selected().map(|e| InboxAction::Worktree(e.request.number)).unwrap_or(InboxAction::None),
            Key::Char('c') => return self.selected().map(|e| InboxAction::CheckOut(e.request.number)).unwrap_or(InboxAction::None),
            Key::Char('o') => return self.selected().map(|e| InboxAction::Browser(e.request.web_url.clone())).unwrap_or(InboxAction::None),
            Key::Char('u') => return InboxAction::Refresh,
            Key::Char('q') => return InboxAction::Close,
            _ => {}
        }
        InboxAction::None
    }
}

pub fn layout(page: &Inbox, cols: usize) -> Page {
    let (left, width) = frame(cols, 140);
    let mut g = Grid::new();
    let mut y = 1;
    let x = g.put(y, left, &format!("Reviews · {}", page.project), Role::Title) + 2;
    g.put(y, x, &page.forge, Role::Muted);
    y += 1;
    let mut x = left;
    for filter in [MrFilter::ReviewRequested, MrFilter::Mine, MrFilter::AllOpen] {
        let on = filter == page.filter;
        let label = format!(" {} ", filter.label());
        x = g.put(y, x, &label, if on { Role::Title } else { Role::Muted }) + 1;
        if on {
            g.panels.push((y, x - label.chars().count() - 1..x - 1));
        }
    }
    g.put(y, x + 1, "Tab", Role::Accent);
    y += 1;
    if let Some((text, failed)) = &page.message {
        g.put(y, left, &fit(text, width), if *failed { Role::Bad } else { Role::Muted });
        y += 1;
    }
    y += 1;
    let Some(entries) = &page.entries else {
        g.put(y, left, "Reading…", Role::Muted);
        return g.finish();
    };
    if entries.is_empty() {
        let none = match page.filter {
            MrFilter::ReviewRequested => "Nothing is waiting on your review. Tab shows yours and every open one.",
            MrFilter::Mine => "You have nothing open.",
            _ => "Nothing is open.",
        };
        g.put(y, left, none, Role::Muted);
    }
    for (i, e) in entries.iter().enumerate() {
        let r = &e.request;
        let line_y = y;
        let x = g.put(y, left, if e.new { "●" } else { " " }, Role::Accent) + 1;
        let x = g.put(y, x, &r.reference(), Role::Accent) + 1;
        let tail = format!("{} · {}", r.author, r.updated_at.get(..10).unwrap_or(&r.updated_at));
        g.put(y, x, &fit(&format!("{}{}", if r.draft { "Draft: " } else { "" }, r.title), (left + width).saturating_sub(x + tail.chars().count() + 2)), Role::Text);
        g.put(y, (left + width).saturating_sub(tail.chars().count()), &tail, Role::Muted);
        if i == page.cursor {
            g.focus(line_y, left..left + width);
            y += 1;
            let mut x = g.put(y, left + 4, &format!("{} → {}", r.source_branch, r.target_branch), Role::Muted) + 2;
            if let Some(p) = &r.pipeline {
                x = g.put(y, x, &format!("checks {}", p.label()), if p.is_bad() { Role::Bad } else if *p == PipelineStatus::Success { Role::Good } else { Role::Warn }) + 2;
            }
            if r.comment_count > 0 {
                x = g.put(y, x, &count(r.comment_count, "comment"), Role::Muted) + 2;
            }
            if e.pending > 0 {
                x = g.put(y, x, &format!("{} of yours pending", count(e.pending, "comment")), Role::Warn) + 2;
            }
            if r.has_conflicts {
                g.put(y, x, "conflicts", Role::Bad);
            }
            for line in r.description.lines().filter(|l| !l.trim().is_empty()).take(2) {
                y += 1;
                g.put(y, left + 4, &fit(line.trim(), width.saturating_sub(4)), Role::Text);
            }
            for row in line_y + 1..=y {
                g.panels.push((row, left..left + width));
            }
        }
        y += 1;
    }
    g.keys(left, width, &[("Enter", "review"), ("w", "in a worktree"), ("c", "check out here"), ("Tab", "filter"), ("o", "browser"), ("u", "refresh"), ("q", "close")]);
    g.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use fenix_forge::{DiffRefs, MrState};

    fn entry(number: u64, title: &str, new: bool) -> Entry {
        Entry {
            request: MergeRequest {
                number,
                title: title.into(),
                description: "Replies have no body.\n\nSo they were unknown.".into(),
                state: MrState::Open,
                draft: false,
                source_branch: "feature/x".into(),
                target_branch: "main".into(),
                author: "alex".into(),
                web_url: format!("https://gitlab.example.com/g/p/-/merge_requests/{number}"),
                has_conflicts: false,
                sha: "h".into(),
                diff_refs: DiffRefs::default(),
                comment_count: 2,
                pipeline: Some(PipelineStatus::Failed),
                updated_at: "2026-09-24T10:00:00Z".into(),
            },
            new,
            pending: 1,
        }
    }

    #[test]
    fn the_row_under_the_cursor_opens_up_and_new_activity_has_a_dot() {
        let mut p = Inbox::new(PathBuf::from("/r"), "GitLab".into(), "g/p".into());
        p.set_entries(vec![entry(142, "Decode PUS-17", true), entry(139, "Validate PCF", false)]);
        let text = layout(&p, 110).text;
        assert!(text.contains("● !142 Decode PUS-17") && text.contains("feature/x → main  checks failed  2 comments  1 comment of yours pending"), "{text}");
        assert!(text.contains("Replies have no body.") && !text.contains("Validate PCF\n    feature"), "only the selected row opens");
        assert_eq!(p.key(Key::Char('j')), InboxAction::None);
        assert_eq!(p.key(Key::Enter), InboxAction::Open(139));
        assert_eq!(p.key(Key::Char('w')), InboxAction::Worktree(139));
    }

    #[test]
    fn tab_cycles_the_filters_starting_from_what_needs_you() {
        let mut p = Inbox::new(PathBuf::from("/r"), "GitHub".into(), "o/r".into());
        assert_eq!(p.filter, MrFilter::ReviewRequested);
        assert_eq!(p.key(Key::Tab), InboxAction::Refresh);
        assert_eq!(p.filter, MrFilter::Mine);
        p.set_entries(Vec::new());
        assert!(layout(&p, 100).text.contains("You have nothing open."));
    }
}
