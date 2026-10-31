//! A conversation with a coding agent (`SPC a a`): the thread as cards --
//! your prompts, the agent's answers and thinking, its tool calls with
//! their diffs and output, permission questions, notices -- with the
//! plan pinned below them and the state of the agent above.
//! `app/agents.rs` owns the threads and the agents; this lays one out
//! and reads its keys, as plain data a test can check.
//!
//! The prompt itself is written in a buffer of its own under this page,
//! with Vim's keys (`app/agents.rs`, the composer).

use crate::page::{fit, frame, wrap, Filter, Grid, Key, Page, Role};
use fenix_acp::hub::{Link, ThreadId, Turn};
use fenix_acp::protocol::{ContentBlock, PermissionKind, PlanStatus, ToolContent, ToolKind, ToolStatus};
use fenix_acp::thread::{Entry, NoticeKind, PermissionState, Thread};
use std::collections::HashSet;

pub struct AgentPage {
    pub thread: ThreadId,
    /// The card the cursor is on, by its entry's index.
    pub focus: usize,
    /// Cards opened (`Tab`): thinking and tool output are folded.
    pub open: HashSet<usize>,
    /// Cards closed that open by themselves (a failed tool, a diff).
    pub closed: HashSet<usize>,
    pub filter: Filter,
    /// The cursor follows the newest card as the agent writes, until you
    /// move it up.
    pub follow: bool,
}

/// What the page shows of the agent, besides the thread.
#[derive(Clone, Debug, Default)]
pub struct Status {
    pub agent: String,
    /// What it's doing: "working…", "waiting on you", "ready", …
    pub state: String,
    pub state_role: Option<Role>,
    /// Logins the agent offers, when it wants one: (id, name).
    pub logins: Vec<(String, String)>,
    pub queued: usize,
    /// Each terminal the agent ran through Fenix, by its id.
    pub terminals: std::collections::HashMap<String, TermView>,
}

/// A terminal an agent ran through Fenix, as its tool card shows it.
#[derive(Clone, Debug, Default)]
pub struct TermView {
    /// The command line, as asked for.
    pub line: String,
    /// Its last lines.
    pub tail: String,
    /// How it ended, once it has.
    pub exit: Option<fenix_acp::protocol::ExitStatus>,
}

/// The command a tool call ran, when its title doesn't already say:
/// "Ran command" is all some agents call one (Devin). From its input, else
/// from the terminal Fenix ran it in.
fn command_shown(call: &fenix_acp::thread::ToolCall, status: &Status) -> Option<String> {
    let line = call.command_line().or_else(|| {
        call.content.iter().find_map(|c| match c {
            ToolContent::Terminal(id) => status.terminals.get(id).map(|t| t.line.clone()),
            _ => None,
        })
    })?;
    let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
    (!line.is_empty() && !call.title.contains(&line)).then_some(line)
}

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    None,
    Close,
    /// To the prompt.
    Compose,
    Cancel,
    Permission { key: String, option: String },
    /// Open a file at a line (from 1).
    Open { path: String, line: Option<u32> },
    Yank(String),
    /// A card's text, read-only in a buffer (`v`).
    View(String),
    /// `]t` / `[t`: the next or previous thread.
    OtherThread(bool),
    /// `C`: a read-only thread continued in a new session.
    ContinueAnew,
}

impl AgentPage {
    pub fn new(thread: ThreadId) -> Self {
        AgentPage { thread, focus: 0, open: HashSet::new(), closed: HashSet::new(), filter: Filter::default(), follow: true }
    }

    pub fn typing(&self) -> bool {
        self.filter.typing
    }

    fn is_open(&self, i: usize, entry: &Entry) -> bool {
        if self.open.contains(&i) {
            return true;
        }
        if self.closed.contains(&i) {
            return false;
        }
        match entry {
            // A failed tool, an edit's diff and a question show what they
            // are about without asking.
            Entry::Tool(call) => {
                call.status == ToolStatus::Failed
                    || (call.kind.changes_files() && call.content.iter().any(|c| matches!(c, ToolContent::Diff { .. })))
                    // A command running in a terminal of Fenix's: its output, live.
                    || call.content.iter().any(|c| matches!(c, ToolContent::Terminal(_)))
            }
            Entry::Thought { .. } => false,
            _ => true,
        }
    }

    /// The cards the filter lets through, by index.
    fn visible(&self, thread: &Thread) -> Vec<usize> {
        let words = self.filter.words();
        (0..thread.entries.len())
            .filter(|&i| {
                if words.is_empty() {
                    return true;
                }
                let text = entry_text(&thread.entries[i]).to_lowercase();
                words.iter().all(|w| text.contains(w.as_str()))
            })
            .collect()
    }

    /// The thread changed: the cursor stays with the newest card while
    /// following.
    pub fn follow_up(&mut self, thread: &Thread) {
        if self.follow {
            if let Some(&last) = self.visible(thread).last() {
                self.focus = last;
            }
        }
    }

    pub fn key(&mut self, thread: &Thread, key: Key) -> Action {
        if self.filter.typing {
            self.filter.key(key);
            self.snap(thread);
            return Action::None;
        }
        let visible = self.visible(thread);
        let at = visible.iter().position(|&i| i == self.focus).unwrap_or(visible.len().saturating_sub(1));
        let go = |page: &mut Self, to: usize| {
            if let Some(&i) = visible.get(to) {
                page.focus = i;
                page.follow = to + 1 == visible.len();
            }
        };
        // A question waiting: its keys answer it, wherever the cursor is.
        if let Some(answer) = answer_for(thread, key) {
            return answer;
        }
        match key {
            Key::Char('q') => return Action::Close,
            Key::Escape if !self.filter.is_empty() => {
                self.filter.clear();
            }
            Key::Char('j') | Key::Down => go(self, (at + 1).min(visible.len().saturating_sub(1))),
            Key::Char('k') | Key::Up => go(self, at.saturating_sub(1)),
            Key::Char('g') => go(self, 0),
            Key::Char('G') => go(self, visible.len().saturating_sub(1)),
            Key::Char(']') | Key::Char('[') => {
                let forward = key == Key::Char(']');
                let is_turn = |i: usize| matches!(thread.entries[i], Entry::User { .. });
                let next = if forward { visible[at + 1..].iter().position(|&i| is_turn(i)).map(|p| at + 1 + p) } else { visible[..at].iter().rposition(|&i| is_turn(i)) };
                if let Some(to) = next {
                    go(self, to);
                }
            }
            Key::Char('T') => return Action::OtherThread(true),
            Key::Char('C') => return Action::ContinueAnew,
            Key::Tab | Key::Char('o') => {
                if let Some(entry) = thread.entries.get(self.focus) {
                    let open = self.is_open(self.focus, entry);
                    if open {
                        self.open.remove(&self.focus);
                        self.closed.insert(self.focus);
                    } else {
                        self.closed.remove(&self.focus);
                        self.open.insert(self.focus);
                    }
                }
            }
            Key::Enter => match thread.entries.get(self.focus) {
                Some(Entry::Tool(call)) => {
                    if let Some(l) = call.locations.first() {
                        return Action::Open { path: l.path.clone(), line: l.line };
                    }
                    if let Some(ToolContent::Diff { path, .. }) = call.content.iter().find(|c| matches!(c, ToolContent::Diff { .. })) {
                        return Action::Open { path: path.clone(), line: None };
                    }
                }
                _ => return Action::Compose,
            },
            Key::Char('i') | Key::Char('a') | Key::Char('c') => return Action::Compose,
            Key::CtrlC => return Action::Cancel,
            Key::Char('y') => return thread.entries.get(self.focus).map(|e| Action::Yank(entry_text(e))).unwrap_or(Action::None),
            Key::Char('Y') => {
                // The answer the cursor is in, else the whole thread.
                return match thread.entries.get(self.focus) {
                    Some(Entry::Agent { text, .. }) => Action::Yank(text.clone()),
                    _ => Action::Yank(thread.transcript()),
                };
            }
            Key::Char('v') => return thread.entries.get(self.focus).map(|e| Action::View(entry_text(e))).unwrap_or(Action::None),
            Key::Char('/') => self.filter.start(),
            _ => {}
        }
        Action::None
    }

    /// Keeps the cursor on a card the filter lets through.
    fn snap(&mut self, thread: &Thread) {
        let visible = self.visible(thread);
        if !visible.contains(&self.focus) {
            if let Some(&last) = visible.last() {
                self.focus = last;
            }
        }
    }
}

/// The answer `key` gives the question waiting in `thread`, if one is and
/// it's one of its keys: `y`/`Y` yes (always), `n`/`N` no (always), `1`-`9`
/// by its place. The thread's keys and the prompt's (empty) both answer.
pub fn answer_for(thread: &Thread, key: Key) -> Option<Action> {
    let card = thread.entries.iter().rev().find_map(|e| match e {
        Entry::Permission(c) if c.state == PermissionState::Pending => Some(c),
        _ => None,
    })?;
    let by_kind = |kind: PermissionKind| card.options.iter().find(|o| o.kind == kind).map(|o| o.id.clone());
    let option = match key {
        Key::Char('y') => by_kind(PermissionKind::AllowOnce).or_else(|| by_kind(PermissionKind::AllowAlways)),
        Key::Char('Y') => by_kind(PermissionKind::AllowAlways),
        Key::Char('n') => by_kind(PermissionKind::RejectOnce).or_else(|| by_kind(PermissionKind::RejectAlways)),
        Key::Char('N') => by_kind(PermissionKind::RejectAlways),
        Key::Char(c @ '1'..='9') => card.options.get(c as usize - '1' as usize).map(|o| o.id.clone()),
        _ => None,
    }?;
    Some(Action::Permission { key: card.key.clone(), option })
}

/// A card's text, for `y` and `v`.
pub fn entry_text(entry: &Entry) -> String {
    match entry {
        Entry::User { blocks, .. } => blocks.iter().map(ContentBlock::as_text).collect::<Vec<_>>().join("\n"),
        Entry::Agent { text, .. } | Entry::Thought { text, .. } => text.clone(),
        Entry::Tool(call) => {
            let mut out = call.title.clone();
            for content in &call.content {
                out.push('\n');
                match content {
                    ToolContent::Content(block) => out.push_str(&block.as_text()),
                    ToolContent::Diff { path, old_text, new_text } => out.push_str(&unified(path, old_text.as_deref().unwrap_or(""), new_text)),
                    ToolContent::Terminal(id) => out.push_str(&format!("[terminal {id}]")),
                }
            }
            out
        }
        Entry::Permission(card) => card.tool_call.title.clone().unwrap_or_default(),
        Entry::Notice(n) => n.text.clone(),
    }
}

/// A small unified diff of a tool's change: the lines that differ, with a
/// line of context.
pub fn unified(path: &str, old: &str, new: &str) -> String {
    let mut out = format!("--- {path}\n+++ {path}\n");
    for line in diff_lines(old, new) {
        out.push_str(&line);
        out.push('\n');
    }
    out
}

/// `-`/`+`/` ` lines between `old` and `new`, by a longest common
/// subsequence of lines (they're an edit's few lines, not whole files).
fn diff_lines(old: &str, new: &str) -> Vec<String> {
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();
    if a.len() * b.len() > 4_000_000 {
        return a.iter().map(|l| format!("-{l}")).chain(b.iter().map(|l| format!("+{l}"))).collect();
    }
    let mut lcs = vec![vec![0u32; b.len() + 1]; a.len() + 1];
    for i in (0..a.len()).rev() {
        for j in (0..b.len()).rev() {
            lcs[i][j] = if a[i] == b[j] { lcs[i + 1][j + 1] + 1 } else { lcs[i + 1][j].max(lcs[i][j + 1]) };
        }
    }
    let (mut i, mut j, mut out) = (0, 0, Vec::new());
    while i < a.len() || j < b.len() {
        if i < a.len() && j < b.len() && a[i] == b[j] {
            out.push(format!(" {}", a[i]));
            i += 1;
            j += 1;
        } else if i < a.len() && (j == b.len() || lcs[i + 1][j] >= lcs[i][j + 1]) {
            // Deletions before insertions, as a unified diff reads.
            out.push(format!("-{}", a[i]));
            i += 1;
        } else {
            out.push(format!("+{}", b[j]));
            j += 1;
        }
    }
    // Context: a line each side of a change.
    let changed: Vec<bool> = out.iter().map(|l| !l.starts_with(' ')).collect();
    out.into_iter()
        .enumerate()
        .filter(|(k, l)| !l.starts_with(' ') || changed.get(k.wrapping_sub(1)).copied().unwrap_or(false) || changed.get(k + 1).copied().unwrap_or(false))
        .map(|(_, l)| l)
        .collect()
}

fn kind_mark(kind: ToolKind) -> &'static str {
    match kind {
        ToolKind::Read => "read",
        ToolKind::Edit => "edit",
        ToolKind::Delete => "delete",
        ToolKind::Move => "move",
        ToolKind::Search => "search",
        ToolKind::Execute => "run",
        ToolKind::Think => "think",
        ToolKind::Fetch => "fetch",
        ToolKind::SwitchMode => "mode",
        ToolKind::Other => "tool",
    }
}

fn status_mark(status: ToolStatus) -> (&'static str, Role) {
    match status {
        ToolStatus::Pending => ("…", Role::Muted),
        ToolStatus::InProgress => ("●", Role::Accent),
        ToolStatus::Completed => ("✓", Role::Good),
        ToolStatus::Failed => ("✗", Role::Bad),
    }
}

/// The page: the header, the cards, the plan, the keys.
pub fn layout(page: &AgentPage, thread: &Thread, link: &Link, turn: &Turn, status: &Status, cols: usize) -> Page {
    let (left, width) = frame(cols, 110);
    let mut g = Grid::new();
    let mut y = 1;
    g.put(y, left, &fit(&thread.display_title(), width), Role::Title);
    y += 1;
    let mut x = g.put(y, left, &status.agent, Role::Accent);
    let piece = |g: &mut Grid, x: &mut usize, text: &str, role: Role| {
        if text.is_empty() {
            return;
        }
        *x = g.put(y, *x, " · ", Role::Muted);
        *x = g.put(y, *x, text, role);
    };
    piece(&mut g, &mut x, &status.state, status.state_role.unwrap_or(Role::Muted));
    if let Some(modes) = &thread.modes {
        let name = modes.available.iter().find(|m| m.id == modes.current).map(|m| m.name.as_str()).unwrap_or(modes.current.as_str());
        piece(&mut g, &mut x, name, Role::Text);
    }
    if let Some(model) = thread.config_options.iter().find(|o| o.category.as_deref() == Some("model")) {
        piece(&mut g, &mut x, &model.current_label(), Role::Text);
    }
    if let Some(usage) = &thread.usage {
        let mut text = format!("{}% context", usage.percent());
        if let Some((amount, currency)) = &usage.cost {
            text.push_str(&format!(" · {amount:.2} {currency}"));
        }
        piece(&mut g, &mut x, &text, Role::Muted);
    }
    let _ = (link, turn);
    y += 1;
    g.rule(y, left..left + width);
    y += 2;

    if page.filter.typing || !page.filter.is_empty() {
        let (line, role) = page.filter.line("filter the thread", width);
        g.put(y, left, &line, role);
        if page.filter.typing {
            g.panel(y, left..left + width);
        }
        y += 2;
    }

    let visible = page.visible(thread);
    if visible.is_empty() {
        let hint = if thread.entries.is_empty() { "Write a prompt below (i), and Enter in Normal mode sends it." } else { "Nothing matches the filter." };
        g.put(y, left, hint, Role::Muted);
        y += 2;
    }
    let inner = width.saturating_sub(2);
    for &i in &visible {
        let entry = &thread.entries[i];
        let top = y;
        let open = page.is_open(i, entry);
        match entry {
            Entry::User { blocks, queued } => {
                let mut x = g.put(y, left, "› You", Role::Accent);
                if *queued {
                    g.put(y, x + 1, "(queued — C-c takes it back)", Role::Muted);
                }
                y += 1;
                for block in blocks {
                    match block {
                        ContentBlock::Text(text) => {
                            for line in text.lines().flat_map(|l| wrap(l, inner)) {
                                g.put(y, left + 2, &line, Role::Text);
                                y += 1;
                            }
                        }
                        other => {
                            x = g.put(y, left + 2, &fit(&other.as_text(), inner), Role::Accent);
                            let _ = x;
                            y += 1;
                        }
                    }
                }
            }
            Entry::Agent { text, extra, .. } => {
                let mut code = false;
                for line in text.lines() {
                    let fence = line.trim_start().starts_with("```");
                    let role = if fence {
                        Role::Muted
                    } else if code {
                        Role::Syntax("string")
                    } else if line.starts_with('#') {
                        Role::Title
                    } else {
                        Role::Text
                    };
                    if fence {
                        code = !code;
                    }
                    let lines = if code && !fence { vec![fit(line, width)] } else { wrap(line, width) };
                    if lines.is_empty() {
                        y += 1;
                    }
                    for piece in lines {
                        g.put(y, left, &piece, role);
                        y += 1;
                    }
                }
                for block in extra {
                    g.put(y, left, &fit(&block.as_text(), width), Role::Muted);
                    y += 1;
                }
            }
            Entry::Thought { text, .. } => {
                let first = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
                let x = g.put(y, left, if open { "▾ thinking" } else { "▸ thinking" }, Role::Muted);
                if !open {
                    g.put(y, x + 2, &fit(first, width.saturating_sub(x - left + 2)), Role::Muted);
                }
                y += 1;
                if open {
                    for line in text.lines().flat_map(|l| wrap(l, inner)) {
                        g.put(y, left + 2, &line, Role::Muted);
                        y += 1;
                    }
                }
            }
            Entry::Tool(call) => {
                let (mark, role) = status_mark(call.status);
                let mut x = g.put(y, left, mark, role) + 1;
                x = g.put(y, x, kind_mark(call.kind), Role::Muted) + 1;
                g.put(y, x, &fit(&call.title, (left + width).saturating_sub(x)), Role::Text);
                y += 1;
                if let Some(command) = command_shown(call, status) {
                    g.put(y, left + 2, &fit(&format!("$ {command}"), inner), Role::Accent);
                    y += 1;
                }
                if let Some(l) = call.locations.first() {
                    let place = match l.line {
                        Some(line) => format!("{}:{line}", l.path),
                        None => l.path.clone(),
                    };
                    g.put(y, left + 2, &fit(&place, inner), Role::Accent);
                    y += 1;
                }
                if open {
                    for content in &call.content {
                        match content {
                            ToolContent::Diff { path, old_text, new_text } => {
                                g.put(y, left + 2, &fit(path, inner), Role::Muted);
                                y += 1;
                                for line in diff_lines(old_text.as_deref().unwrap_or(""), new_text).iter().take(60) {
                                    let role = match line.chars().next() {
                                        Some('+') => Role::Good,
                                        Some('-') => Role::Bad,
                                        _ => Role::Muted,
                                    };
                                    g.put(y, left + 2, &fit(line, inner), role);
                                    y += 1;
                                }
                            }
                            ToolContent::Content(block) => {
                                let text = block.as_text();
                                let lines: Vec<&str> = text.lines().filter(|l| !l.trim_start().starts_with("```")).collect();
                                let shown = lines.len().min(40);
                                for line in &lines[lines.len() - shown..] {
                                    g.put(y, left + 2, &fit(line, inner), Role::Muted);
                                    y += 1;
                                }
                                if lines.len() > shown {
                                    g.put(y, left + 2, &format!("({} lines above; v shows all)", lines.len() - shown), Role::Muted);
                                    y += 1;
                                }
                            }
                            ToolContent::Terminal(id) => match status.terminals.get(id) {
                                Some(TermView { tail, exit, .. }) => {
                                    for line in tail.lines() {
                                        g.put(y, left + 2, &fit(line, inner), Role::Muted);
                                        y += 1;
                                    }
                                    let (text, role) = match exit {
                                        None => ("running…".to_string(), Role::Accent),
                                        Some(e) if e.code == Some(0) => ("exited 0".to_string(), Role::Good),
                                        Some(e) => (match (&e.code, &e.signal) {
                                            (Some(c), _) => format!("exited {c}"),
                                            (None, Some(s)) => format!("stopped ({s})"),
                                            _ => "ended".to_string(),
                                        }, Role::Bad),
                                    };
                                    g.put(y, left + 2, &text, role);
                                    y += 1;
                                }
                                None => {
                                    g.put(y, left + 2, &format!("terminal {id}"), Role::Muted);
                                    y += 1;
                                }
                            },
                        }
                    }
                }
            }
            Entry::Permission(card) => {
                let title = card.tool_call.title.clone().unwrap_or_else(|| "a tool".into());
                match &card.state {
                    PermissionState::Pending => {
                        g.put(y, left, &fit(&format!("? The agent asks to: {title}"), width), Role::Warn);
                        y += 1;
                        let mut x = left + 2;
                        for (n, option) in card.options.iter().enumerate() {
                            let key = match option.kind {
                                PermissionKind::AllowOnce => "y".to_string(),
                                PermissionKind::AllowAlways => "Y".to_string(),
                                PermissionKind::RejectOnce => "n".to_string(),
                                PermissionKind::RejectAlways => "N".to_string(),
                                PermissionKind::Other => (n + 1).to_string(),
                            };
                            let shown = format!("{key}/{}", n + 1);
                            if x + shown.len() + option.name.chars().count() + 4 > left + width {
                                y += 1;
                                x = left + 2;
                            }
                            x = g.put(y, x, &shown, Role::Accent) + 1;
                            x = g.put(y, x, &option.name, Role::Text) + 3;
                        }
                        y += 1;
                    }
                    PermissionState::Answered(name) => {
                        g.put(y, left, &fit(&format!("✓ {title} — {name}"), width), Role::Muted);
                        y += 1;
                    }
                    PermissionState::Withdrawn => {
                        g.put(y, left, &fit(&format!("– {title} — not answered"), width), Role::Muted);
                        y += 1;
                    }
                }
            }
            Entry::Notice(n) => {
                let role = match n.kind {
                    NoticeKind::Error => Role::Bad,
                    NoticeKind::Warning | NoticeKind::Login => Role::Warn,
                    NoticeKind::Interrupted | NoticeKind::Stopped => Role::Warn,
                    NoticeKind::Info => Role::Muted,
                };
                for line in wrap(&n.text, width) {
                    g.put(y, left, &line, role);
                    y += 1;
                }
                if n.kind == NoticeKind::Login && !status.logins.is_empty() {
                    let names: Vec<String> = status.logins.iter().enumerate().map(|(k, (_, name))| format!("{}: {name}", k + 1)).collect();
                    g.put(y, left + 2, &fit(&format!("Log in with SPC a p ({})", names.join(", ")), inner), Role::Accent);
                    y += 1;
                }
            }
        }
        if i == page.focus {
            g.focus(top, left..left + width);
        }
        y += 1;
    }

    if !thread.plan.is_empty() {
        g.heading(y, left, width, &format!("plan · {}/{}", thread.plan.iter().filter(|p| p.status == PlanStatus::Completed).count(), thread.plan.len()));
        y += 1;
        for entry in &thread.plan {
            let (mark, role) = match entry.status {
                PlanStatus::Completed => ("☑", Role::Good),
                PlanStatus::InProgress => ("◐", Role::Accent),
                PlanStatus::Pending => ("☐", Role::Muted),
            };
            let x = g.put(y, left, mark, role) + 1;
            g.put(y, x, &fit(&entry.content, width.saturating_sub(2)), if entry.status == PlanStatus::Completed { Role::Muted } else { Role::Text });
            y += 1;
        }
    }
    if status.queued > 0 {
        g.put(y, left, &format!("{} prompt(s) queued", status.queued), Role::Muted);
    }
    g.keys(left, width, &[("i", "prompt"), ("j k", "cards"), ("Tab", "fold"), ("Enter", "open"), ("y Y", "yank"), ("C-c", "cancel"), ("/", "filter"), ("q", "close")]);
    g.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use fenix_acp::protocol::{PermissionOption, SessionUpdate, ToolCallFields};
    use fenix_acp::thread::PermissionCard;
    use serde_json::json;

    fn thread() -> Thread {
        let mut t = Thread::default();
        t.push_user(vec![ContentBlock::text("Fix the parser")], false);
        for u in [
            json!({ "sessionUpdate": "agent_thought_chunk", "content": { "type": "text", "text": "Look at parse.rs first.\nThen the tests." } }),
            json!({ "sessionUpdate": "tool_call", "toolCallId": "t1", "title": "Edit src/parse.rs", "kind": "edit", "status": "completed", "content": [{ "type": "diff", "path": "/p/src/parse.rs", "oldText": "a\nb\nc", "newText": "a\nB\nc" }], "locations": [{ "path": "/p/src/parse.rs", "line": 2 }] }),
            json!({ "sessionUpdate": "tool_call", "toolCallId": "t2", "title": "cargo test", "kind": "execute", "status": "completed", "content": [{ "type": "content", "content": { "type": "text", "text": "```console\ntest ok\n```" } }] }),
            json!({ "sessionUpdate": "agent_message_chunk", "content": { "type": "text", "text": "Fixed it.\n\n```rust\nfn x() {}\n```" } }),
            json!({ "sessionUpdate": "plan", "entries": [{ "content": "Fix", "priority": "high", "status": "completed" }, { "content": "Test", "priority": "medium", "status": "in_progress" }] }),
        ] {
            t.apply(SessionUpdate::parse(&u), false);
        }
        t
    }

    fn status() -> Status {
        Status { agent: "Claude Code".into(), state: "ready".into(), ..Status::default() }
    }

    #[test]
    fn the_thread_reads_as_cards_with_the_plan_below() {
        let t = thread();
        let mut page = AgentPage::new(ThreadId("x".into()));
        page.follow_up(&t);
        let text = layout(&page, &t, &Link::Detached, &Turn::Idle, &status(), 100).all_text();
        for want in ["Fix the parser", "Claude Code · ready", "› You", "▸ thinking  Look at parse.rs first.", "✓ edit Edit src/parse.rs", "/p/src/parse.rs:2", "-b", "+B", "✓ run cargo test", "Fixed it.", "fn x() {}", "PLAN · 1/2", "☑ Fix", "◐ Test"] {
            assert!(text.contains(want), "missing {want:?} in\n{text}");
        }
        assert!(!text.contains("test ok"), "a command's output is folded");
        assert!(!text.contains("Then the tests."), "thinking is folded");
        assert_eq!(page.focus, 4, "following the newest card");
    }

    /// A card titled only "Ran command" says which: from the call's input,
    /// else from the terminal Fenix ran it in. One whose title is the
    /// command already doesn't say it twice.
    #[test]
    fn a_command_card_says_what_ran() {
        let mut t = thread();
        for u in [
            json!({ "sessionUpdate": "tool_call", "toolCallId": "t3", "title": "Ran command", "kind": "execute", "status": "completed", "rawInput": { "command": "rg -n session  *.tcl" } }),
            json!({ "sessionUpdate": "tool_call", "toolCallId": "t4", "title": "Ran command", "kind": "execute", "status": "completed", "content": [{ "type": "terminal", "terminalId": "term-1" }] }),
            json!({ "sessionUpdate": "tool_call", "toolCallId": "t5", "title": "cargo build", "kind": "execute", "status": "completed", "rawInput": { "command": ["cargo", "build"] } }),
        ] {
            t.apply(SessionUpdate::parse(&u), false);
        }
        let mut status = status();
        status.terminals.insert("term-1".into(), TermView { line: "Get-Content 'a b.tcl'".into(), ..TermView::default() });
        let page = AgentPage::new(ThreadId("x".into()));
        let text = layout(&page, &t, &Link::Detached, &Turn::Idle, &status, 100).all_text();
        assert!(text.contains("$ rg -n session *.tcl"), "{text}");
        assert!(text.contains("$ Get-Content 'a b.tcl'"), "{text}");
        assert!(!text.contains("$ cargo build"), "{text}");
    }

    #[test]
    fn keys_move_fold_open_and_yank() {
        let t = thread();
        let mut page = AgentPage::new(ThreadId("x".into()));
        page.key(&t, Key::Char('g'));
        assert_eq!(page.focus, 0);
        assert!(!page.follow);
        page.key(&t, Key::Char('j'));
        page.key(&t, Key::Tab);
        let text = layout(&page, &t, &Link::Detached, &Turn::Idle, &status(), 100).all_text();
        assert!(text.contains("Then the tests."), "opened");
        page.key(&t, Key::Char('j'));
        assert_eq!(page.key(&t, Key::Enter), Action::Open { path: "/p/src/parse.rs".into(), line: Some(2) });
        assert!(matches!(page.key(&t, Key::Char('y')), Action::Yank(s) if s.contains("+B")));
        page.key(&t, Key::Char('G'));
        assert_eq!(page.key(&t, Key::Char('Y')), Action::Yank("Fixed it.\n\n```rust\nfn x() {}\n```".into()));
        assert!(page.follow);
        page.key(&t, Key::Char('['));
        assert_eq!(page.focus, 0, "back to the prompt");
        assert_eq!(page.key(&t, Key::Char('i')), Action::Compose);
        assert_eq!(page.key(&t, Key::CtrlC), Action::Cancel);
        assert_eq!(page.key(&t, Key::Char('q')), Action::Close);
    }

    #[test]
    fn a_question_takes_y_and_n_wherever_the_cursor_is() {
        let mut t = thread();
        t.push_permission(PermissionCard {
            key: "7".into(),
            tool_call: ToolCallFields { id: "t3".into(), title: Some("rm -rf build".into()), ..Default::default() },
            options: vec![
                PermissionOption { id: "allow-once".into(), name: "Yes".into(), kind: PermissionKind::AllowOnce },
                PermissionOption { id: "allow-always".into(), name: "Yes, and don't ask again".into(), kind: PermissionKind::AllowAlways },
                PermissionOption { id: "reject".into(), name: "No".into(), kind: PermissionKind::RejectOnce },
            ],
            state: PermissionState::Pending,
        });
        let mut page = AgentPage::new(ThreadId("x".into()));
        let text = layout(&page, &t, &Link::Detached, &Turn::Idle, &status(), 100).all_text();
        assert!(text.contains("? The agent asks to: rm -rf build") && text.contains("y/1 Yes") && text.contains("n/3 No"), "{text}");
        page.key(&t, Key::Char('g'));
        assert_eq!(page.key(&t, Key::Char('n')), Action::Permission { key: "7".into(), option: "reject".into() });
        assert_eq!(page.key(&t, Key::Char('Y')), Action::Permission { key: "7".into(), option: "allow-always".into() });
        assert_eq!(page.key(&t, Key::Char('2')), Action::Permission { key: "7".into(), option: "allow-always".into() });
    }

    #[test]
    fn the_filter_keeps_matching_cards() {
        let t = thread();
        let mut page = AgentPage::new(ThreadId("x".into()));
        page.key(&t, Key::Char('/'));
        for c in "cargo".chars() {
            page.key(&t, Key::Char(c));
        }
        page.key(&t, Key::Enter);
        let text = layout(&page, &t, &Link::Detached, &Turn::Idle, &status(), 100).all_text();
        assert!(text.contains("cargo test") && !text.contains("Fixed it."), "{text}");
    }

    #[test]
    fn diffs_show_changes_with_a_line_of_context() {
        assert_eq!(diff_lines("a\nb\nc\nd\ne", "a\nb\nC\nd\ne"), vec![" b", "-c", "+C", " d"]);
        assert_eq!(diff_lines("", "new"), vec!["+new"]);
    }
}
