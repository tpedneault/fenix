//! The interactive rebase page: the commits a rebase would replay, newest
//! at the top like the log, one key per verb and `J`/`K` to move one --
//! and underneath, what the branch will look like after. `fixup!` and
//! `squash!` commits start out paired with the commit they name. Nothing
//! runs until `C-c C-c`; `Enter` only looks.

use std::path::PathBuf;

use fenix_git::{Planned, Replayed, Step};

use crate::page::{fit, frame, Grid, Key, Page, Role};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verb {
    Pick,
    Reword,
    Edit,
    Squash,
    Fixup,
    Drop,
}

impl Verb {
    fn name(self) -> &'static str {
        match self {
            Verb::Pick => "pick",
            Verb::Reword => "reword",
            Verb::Edit => "edit",
            Verb::Squash => "squash",
            Verb::Fixup => "fixup",
            Verb::Drop => "drop",
        }
    }

    fn role(self) -> Role {
        match self {
            Verb::Pick => Role::Accent,
            Verb::Reword => Role::Warn,
            Verb::Edit => Role::Good,
            Verb::Squash | Verb::Fixup => Role::Title,
            Verb::Drop => Role::Bad,
        }
    }

    fn folds(self) -> bool {
        matches!(self, Verb::Squash | Verb::Fixup)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub hash: String,
    pub short: String,
    pub subject: String,
    pub verb: Verb,
    /// A reword's new message.
    pub message: Option<String>,
    /// Moved from where git listed it.
    pub moved: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RebaseAction {
    None,
    Close,
    /// Run it: onto `base` (the root when `None`), doing `plan`.
    Start { base: Option<String>, plan: Vec<Planned> },
    /// Write a reword's message, starting from `seed`'s commit message.
    EditMessage { hash: String },
    ShowCommit(String),
}

pub struct RebasePage {
    pub root: PathBuf,
    pub branch: String,
    /// What the commits are replayed onto; `None` rebases from the root.
    pub base: Option<String>,
    pub base_label: String,
    /// Oldest first, as the todo list runs.
    pub items: Vec<Item>,
    original: Vec<Item>,
    /// How many of the commits are already on the upstream.
    pub pushed: usize,
    /// The cursor, as a row from the top (newest first).
    pub cursor: usize,
    pub message: Option<(String, bool)>,
    pub busy: bool,
    armed: bool,
}

/// Puts each `fixup!`/`squash!` commit right after the commit it names,
/// with that verb -- what `git rebase --autosquash` would do.
fn autosquash(mut items: Vec<Item>) -> Vec<Item> {
    let mut i = 0;
    while i < items.len() {
        let (verb, target) = match items[i].subject.strip_prefix("fixup! ") {
            Some(t) => (Verb::Fixup, t.to_string()),
            None => match items[i].subject.strip_prefix("squash! ") {
                Some(t) => (Verb::Squash, t.to_string()),
                None => {
                    i += 1;
                    continue;
                }
            },
        };
        let Some(at) = items[..i].iter().position(|t| t.subject == target) else {
            i += 1;
            continue;
        };
        let mut item = items.remove(i);
        item.verb = verb;
        item.moved = at + 1 != i;
        // After the target and any fixups already folded into it.
        let mut to = at + 1;
        while to < items.len() && items[to].verb.folds() && to < i {
            to += 1;
        }
        items.insert(to, item);
        i += 1;
    }
    items
}

impl RebasePage {
    pub fn new(root: PathBuf, branch: String, base: Option<String>, base_label: String, commits: Vec<Replayed>, pushed: usize) -> Self {
        let items: Vec<Item> = commits
            .into_iter()
            .map(|c| Item { hash: c.hash, short: c.short_hash, subject: c.subject, verb: Verb::Pick, message: None, moved: false })
            .collect();
        let items = autosquash(items);
        RebasePage { root, branch, base, base_label, original: items.clone(), items, pushed, cursor: 0, message: None, busy: false, armed: false }
    }

    /// The item under the cursor's index in `items`.
    fn index(&self) -> Option<usize> {
        (!self.items.is_empty()).then(|| self.items.len() - 1 - self.cursor.min(self.items.len() - 1))
    }

    pub fn set_message(&mut self, hash: &str, message: String) {
        if let Some(item) = self.items.iter_mut().find(|i| i.hash == hash) {
            item.verb = Verb::Reword;
            item.message = Some(message);
        }
    }

    /// The plan the todo list is written from.
    pub fn plan(&self) -> Vec<Planned> {
        self.items
            .iter()
            .map(|i| Planned {
                hash: i.hash.clone(),
                step: match i.verb {
                    Verb::Pick => Step::Pick,
                    Verb::Reword => Step::Reword(i.message.clone().unwrap_or_else(|| i.subject.clone())),
                    Verb::Edit => Step::Edit,
                    Verb::Squash => Step::Squash,
                    Verb::Fixup => Step::Fixup,
                    Verb::Drop => Step::Drop,
                },
            })
            .collect()
    }

    /// The commits there'll be after: each with how many fold into it.
    fn result(&self) -> Vec<(&Item, usize)> {
        let mut out: Vec<(&Item, usize)> = Vec::new();
        for item in &self.items {
            match item.verb {
                Verb::Drop => {}
                v if v.folds() => {
                    if let Some(last) = out.last_mut() {
                        last.1 += 1;
                    }
                }
                _ => out.push((item, 0)),
            }
        }
        out
    }

    fn problem(&self) -> Option<String> {
        let first = self.items.iter().find(|i| i.verb != Verb::Drop)?;
        first.verb.folds().then(|| format!("{} {} has nothing before it to fold into", first.verb.name(), first.short))
    }

    pub fn key(&mut self, key: Key) -> RebaseAction {
        if key != Key::CtrlC {
            self.armed = false;
        }
        let n = self.items.len();
        match key {
            Key::Down | Key::Char('j') => self.cursor = (self.cursor + 1).min(n.saturating_sub(1)),
            Key::Up | Key::Char('k') => self.cursor = self.cursor.saturating_sub(1),
            Key::Char('p') => self.set(Verb::Pick),
            Key::Char('e') => self.set(Verb::Edit),
            Key::Char('s') => self.set(Verb::Squash),
            Key::Char('f') => self.set(Verb::Fixup),
            Key::Char('d') => self.set(Verb::Drop),
            Key::Char('r') => {
                if let Some(i) = self.index() {
                    return RebaseAction::EditMessage { hash: self.items[i].hash.clone() };
                }
            }
            // J moves the commit down the page (older), K up (newer).
            Key::Char('J') => self.shift(false),
            Key::Char('K') => self.shift(true),
            Key::Char('u') => {
                self.items = self.original.clone();
                self.message = Some(("back to how git listed them".to_string(), false));
            }
            Key::Enter => {
                if let Some(i) = self.index() {
                    return RebaseAction::ShowCommit(self.items[i].hash.clone());
                }
            }
            Key::Char('q') | Key::Escape => return RebaseAction::Close,
            Key::CtrlC => {
                if let Some(problem) = self.problem() {
                    self.message = Some((problem, true));
                    return RebaseAction::None;
                }
                if !self.armed {
                    self.armed = true;
                    return RebaseAction::None;
                }
                self.armed = false;
                return RebaseAction::Start { base: self.base.clone(), plan: self.plan() };
            }
            _ => {}
        }
        RebaseAction::None
    }

    fn set(&mut self, verb: Verb) {
        if let Some(i) = self.index() {
            self.items[i].verb = verb;
        }
    }

    fn shift(&mut self, newer: bool) {
        let Some(i) = self.index() else { return };
        let j = if newer { i + 1 } else { i.wrapping_sub(1) };
        if j >= self.items.len() {
            return;
        }
        self.items.swap(i, j);
        self.items[j].moved = true;
        self.cursor = self.items.len() - 1 - j;
    }
}

pub fn layout(page: &RebasePage, cols: usize) -> Page {
    let (left, width) = frame(cols, 120);
    let mut g = Grid::new();
    let mut y = 1;
    let x = g.put(y, left, &format!("Rebase {} onto {}", page.branch, page.base_label), Role::Title) + 2;
    g.put(y, x, &format!("{} commits", page.items.len()), Role::Muted);
    y += 1;
    if page.busy {
        g.put(y, left, "… rebasing", Role::Accent);
        y += 1;
    } else if let Some((text, failed)) = &page.message {
        g.put(y, left, &fit(text, width), if *failed { Role::Bad } else { Role::Muted });
        y += 1;
    }
    y += 1;
    g.heading(y, left, width, "Newest first");
    y += 1;
    for (row, item) in page.items.iter().rev().enumerate() {
        let x = g.put(y, left + 2, item.verb.name(), item.verb.role());
        let x = g.put(y, x.max(left + 10), &item.short, Role::Accent) + 2;
        let subject = match (&item.verb, &item.message) {
            (Verb::Reword, Some(m)) => m.lines().next().unwrap_or("").to_string(),
            _ => item.subject.clone(),
        };
        let role = if item.verb == Verb::Drop { Role::Muted } else { Role::Text };
        g.put(y, x, &fit(&subject, (left + width).saturating_sub(x + 10)), role);
        if item.moved {
            g.put(y, (left + width).saturating_sub(5), "moved", Role::Muted);
        }
        if row == page.cursor {
            g.focus(y, left..left + width);
        }
        y += 1;
    }
    let base = page.base.as_deref().map(|b| &b[..7.min(b.len())]).unwrap_or("the root");
    g.put(y, left + 2, &format!("└ onto {} {base}", page.base_label), Role::Muted);
    y += 2;

    let result = page.result();
    g.heading(y, left, width, &format!("Result · {} commits, was {}", result.len(), page.items.len()));
    y += 1;
    for (item, folded) in result.iter().rev() {
        let subject = match (&item.verb, &item.message) {
            (Verb::Reword, Some(m)) => m.lines().next().unwrap_or("").to_string(),
            _ => item.subject.clone(),
        };
        let x = g.put(y, left + 2, &fit(&subject, width.saturating_sub(20)), if item.verb == Verb::Reword { Role::Warn } else { Role::Text }) + 2;
        if *folded > 0 {
            g.put(y, x, &format!("+{folded} folded in"), Role::Muted);
        }
        if item.verb == Verb::Edit {
            g.put(y, x, "stops here to amend", Role::Good);
        }
        y += 1;
    }
    let dropped: Vec<&Item> = page.items.iter().filter(|i| i.verb == Verb::Drop).collect();
    if !dropped.is_empty() {
        y += 1;
        g.heading(y, left, width, "Dropped");
        y += 1;
        for item in dropped {
            g.put(y, left + 2, &fit(&format!("{} {}", item.short, item.subject), width - 2), Role::Muted);
            y += 1;
        }
    }
    y += 1;
    g.put(y, left, "Undo point recorded when it runs -- U on the Git page takes it back.", Role::Muted);
    y += 1;
    if page.pushed > 0 {
        g.put(y, left, &format!("{} of these are already pushed -- the push afterwards will need --force-with-lease.", page.pushed), Role::Warn);
    }
    let keys: &[(&str, &str)] = if page.armed {
        &[("C-c", "again to rebase"), ("any key", "back")]
    } else {
        &[("p", "pick"), ("r", "reword"), ("e", "edit"), ("s", "squash"), ("f", "fixup"), ("d", "drop"), ("J/K", "move"), ("Enter", "show"), ("C-c C-c", "rebase"), ("q", "cancel")]
    };
    g.keys(left, width, keys);
    g.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commits(subjects: &[&str]) -> Vec<Replayed> {
        subjects.iter().enumerate().map(|(i, s)| Replayed { hash: format!("{i:0>40}"), short_hash: format!("{i:0>7}"), subject: s.to_string() }).collect()
    }

    fn page(subjects: &[&str]) -> RebasePage {
        RebasePage::new(PathBuf::from("/r"), "topic".into(), Some("base".into()), "main".into(), commits(subjects), 0)
    }

    #[test]
    fn fixups_start_next_to_their_commit() {
        let p = page(&["status page", "sections fold", "fixup! status page", "push plan"]);
        let order: Vec<(&str, Verb)> = p.items.iter().map(|i| (i.subject.as_str(), i.verb)).collect();
        assert_eq!(order, [("status page", Verb::Pick), ("fixup! status page", Verb::Fixup), ("sections fold", Verb::Pick), ("push plan", Verb::Pick)]);
        assert!(p.items[1].moved);
        assert!(layout(&p, 100).text.contains("RESULT · 3 COMMITS, WAS 4"));
    }

    #[test]
    fn verbs_and_moves_become_the_plan() {
        let mut p = page(&["a", "b", "c"]);
        // Newest first: c, b, a.
        p.key(Key::Char('d'));
        p.key(Key::Char('j'));
        p.key(Key::Char('J'));
        let plan: Vec<(String, Step)> = p.plan().into_iter().map(|s| (s.hash[39..].to_string(), s.step)).collect();
        assert_eq!(plan, [("1".into(), Step::Pick), ("0".into(), Step::Pick), ("2".into(), Step::Drop)]);
        let text = layout(&p, 100).text;
        assert!(text.contains("DROPPED") && text.contains("moved"), "{text}");
    }

    #[test]
    fn nothing_runs_until_c_c_twice_and_a_leading_fixup_is_refused() {
        let mut p = page(&["a", "b"]);
        assert_eq!(p.key(Key::CtrlC), RebaseAction::None);
        assert!(matches!(p.key(Key::CtrlC), RebaseAction::Start { .. }));
        p.cursor = 1; // a, the oldest
        p.key(Key::Char('f'));
        assert_eq!(p.key(Key::CtrlC), RebaseAction::None);
        assert!(p.message.as_ref().unwrap().0.contains("nothing before it"));
    }

    #[test]
    fn a_reword_asks_for_the_message_and_shows_it() {
        let mut p = page(&["a", "b"]);
        assert_eq!(p.key(Key::Char('r')), RebaseAction::EditMessage { hash: format!("{:0>40}", 1) });
        p.set_message(&format!("{:0>40}", 1), "b, better\n\nbody".into());
        assert_eq!(p.plan()[1].step, Step::Reword("b, better\n\nbody".into()));
        assert!(layout(&p, 100).text.contains("b, better"));
    }
}
