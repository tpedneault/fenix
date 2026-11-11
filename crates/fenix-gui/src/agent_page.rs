//! A conversation with a coding agent (`SPC a a`): the thread as cards --
//! your prompts, the agent's answers and thinking, its tool calls with
//! their diffs and output, permission questions, notices -- with the
//! plan pinned below them and the state of the agent above.
//! `app/agents.rs` owns the threads and the agents; this lays one out
//! and reads its keys, as plain data a test can check.
//!
//! The prompt itself is written in a buffer of its own under this page,
//! with Vim's keys (`app/agents.rs`, the composer).

use crate::page::{fit, frame, one_line, str_cells, wrap, CardLook, Chip, Filter, Grid, Key, Page, Role};
use fenix_acp::hub::{Link, ThreadId, Turn};
use fenix_acp::protocol::{ContentBlock, PermissionKind, PlanStatus, ToolContent, ToolKind, ToolStatus};
use fenix_acp::thread::{ElicitationState, Entry, NoticeKind, PermissionState, Thread};
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
    /// The last question the cursor was taken to.
    pub asked: Option<String>,
    /// `]` or `[` just pressed: an `s` after it is a step, not a turn.
    pub bracket: Option<bool>,
    /// The answer `j` and `k` are on in the question waiting, by the
    /// question's key: Enter gives it.
    pub choice: Option<(String, usize)>,
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
    /// How a code block in what the agent says is coloured: per line,
    /// (columns, capture name). None: plain.
    pub highlight: Option<Highlight>,
    /// The thread's project: a path in it is shown from there.
    pub root: Option<std::path::PathBuf>,
}

/// A path the agent named, shorter: from the project when it's in it,
/// else from `~`, else whole.
fn short_path(status: &Status, path: &str) -> String {
    let path = std::path::Path::new(path);
    status.root.as_deref().and_then(|root| crate::page::path_within(root, path)).unwrap_or_else(|| crate::page::home_relative(path))
}

/// A code block's colours by its language: per line, (columns, capture).
pub type Highlight = fn(&str, &str) -> Vec<Vec<(std::ops::Range<usize>, &'static str)>>;

/// What the agent said, as Markdown laid out the way the reading view
/// lays it out -- headings, emphasis, inline code, lists, quotes, tables
/// and highlighted code -- `width` cells wide. Pictures and diagrams stay
/// text: the panel draws none.
fn markdown(text: &str, width: usize, highlight: Option<Highlight>) -> Page {
    let colours = |lang: &str, code: &str| highlight.map(|h| h(lang, code)).unwrap_or_default();
    let ctx = crate::reading::Ctx {
        highlight: &colours,
        image_size: &|_| None,
        diagram_key: &|source| crate::page::ImageKey::Diagram(source.len() as u64),
        diagram_error: &|_| None,
        drawn: &|_| false,
        image_path: &|_| None,
        embed: &|link| crate::reading::Embedded::Unresolved(link.target.clone()),
        link_exists: &|_| true,
        cell: (1.0, 2.0),
    };
    crate::reading::layout(text, width, &ctx).page
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
    let line = one_line(&line);
    (!line.is_empty() && !one_line(&call.title).contains(&line)).then_some(line)
}

/// The command a permission asks to run, whole: from the call's input,
/// else its title when it's a command's (`execute`).
fn asked_command(call: &fenix_acp::protocol::ToolCallFields) -> Option<String> {
    let as_call = fenix_acp::thread::ToolCall { id: call.id.clone(), title: String::new(), kind: ToolKind::Other, status: ToolStatus::Pending, content: Vec::new(), locations: Vec::new(), raw_input: call.raw_input.clone(), raw_output: None };
    let line = as_call.command_line().or_else(|| (call.kind == Some(ToolKind::Execute)).then(|| call.title.clone()).flatten())?;
    (!line.trim().is_empty()).then_some(line)
}

/// Rows of a command, the most a question shows of it: its first.
const COMMAND_HEAD: usize = 5;
/// The rows of a command a question shows whole, at most; one longer
/// shows its first five, how many more, and its last.
const COMMAND_ROWS: usize = 8;

/// A command's lines as rows `width` cells wide: `$ ` before the first,
/// the rest under it, each wrapped where it has to be. Its indentation is
/// kept; a tab is four spaces.
fn command_rows(command: &str, width: usize) -> Vec<String> {
    let width = width.max(4);
    let mut rows = Vec::new();
    for (i, line) in command.trim_end().lines().enumerate() {
        let line: String = line.trim_end().replace('\t', "    ").chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
        let lead = if i == 0 { "$ " } else { "  " };
        let mut piece = String::from(lead);
        let mut used = 2;
        for c in line.chars() {
            let w = crate::page::char_cells(c);
            if used + w > width {
                rows.push(std::mem::take(&mut piece));
                piece.push_str("  ");
                used = 2;
            }
            used += w;
            piece.push(c);
        }
        rows.push(piece);
    }
    rows
}

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    None,
    Close,
    /// To the prompt.
    Compose,
    Cancel,
    Permission { key: String, option: String },
    /// An agent's question (`elicitation/create`), answered.
    Elicit { key: String, how: ElicitAnswer },
    /// Open a file at a line (from 1).
    Open { path: String, line: Option<u32> },
    Yank(String),
    /// A card's text, read-only in a buffer (`v`).
    View(String),
    /// `]t` / `[t`: the next or previous thread.
    OtherThread(bool),
    /// `]s` / `[s`: a pipeline's next or previous step.
    OtherStep(bool),
    /// `C`: a read-only thread continued in a new session.
    ContinueAnew,
}

impl AgentPage {
    pub fn new(thread: ThreadId) -> Self {
        AgentPage { thread, focus: 0, open: HashSet::new(), closed: HashSet::new(), filter: Filter::default(), follow: true, bracket: None, choice: None, asked: None }
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
        // A question the agent has just asked takes the cursor, once, even
        // when you'd moved up the thread: `j`, `k` and Enter answer it.
        if let Some((i, card)) = waiting_permission(thread) {
            if self.asked.as_deref() != Some(card.key.as_str()) {
                self.asked = Some(card.key.clone());
                self.focus = i;
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
        // `]s` / `[s`: the bracket was a turn's jump, or the first half of
        // a step's.
        if let Some(forward) = self.bracket.take() {
            if key == Key::Char('s') {
                return Action::OtherStep(forward);
            }
        }
        // A question waiting: its keys answer it, wherever the cursor is.
        if let Some(answer) = answer_for(thread, key) {
            return answer;
        }
        // On it, `j` and `k` go from answer to answer -- past the first or
        // the last, on to the cards around it -- and Enter gives the one
        // they're on.
        if let Some((i, card)) = waiting_permission(thread).filter(|(i, _)| *i == self.focus) {
            let on = self.chosen(card);
            match key {
                Key::Char('j') | Key::Down if on + 1 < card.options.len() => {
                    self.choice = Some((card.key.clone(), on + 1));
                    return Action::None;
                }
                Key::Char('k') | Key::Up if on > 0 => {
                    self.choice = Some((card.key.clone(), on - 1));
                    return Action::None;
                }
                Key::Enter => {
                    let _ = i;
                    if let Some(option) = card.options.get(on) {
                        return Action::Permission { key: card.key.clone(), option: option.id.clone() };
                    }
                }
                _ => {}
            }
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
                self.bracket = Some(forward);
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

    /// The answer `j` and `k` are on in `card`: the one chosen, else the
    /// first that allows it, else the first.
    pub fn chosen(&self, card: &fenix_acp::thread::PermissionCard) -> usize {
        match &self.choice {
            Some((key, on)) if *key == card.key && *on < card.options.len() => *on,
            _ => card.options.iter().position(|o| o.kind == PermissionKind::AllowOnce).unwrap_or(0),
        }
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

/// How a card asking for something (`elicitation/create`) was answered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ElicitAnswer {
    /// Answer the form (or open the link): `y`.
    Fill,
    /// Send the form's defaults as they are: `Y`.
    Defaults,
    /// `n`.
    Decline,
    /// `x`.
    Cancel,
}

/// The permission question waiting in `thread`, newest first, and where
/// it is in the thread.
fn waiting_permission(thread: &Thread) -> Option<(usize, &fenix_acp::thread::PermissionCard)> {
    thread.entries.iter().enumerate().rev().find_map(|(i, e)| match e {
        Entry::Permission(c) if c.state == PermissionState::Pending => Some((i, c)),
        _ => None,
    })
}

/// The key a permission's answer takes, as its cap shows it: `y`, `Y`,
/// `n`, `N`, `e`, else its place.
fn option_key(option: &fenix_acp::protocol::PermissionOption, n: usize) -> String {
    match option.kind {
        PermissionKind::AllowOnce => "y".to_string(),
        PermissionKind::AllowAlways => "Y".to_string(),
        PermissionKind::RejectOnce => "n".to_string(),
        PermissionKind::RejectAlways => "N".to_string(),
        PermissionKind::Other if option.id == "edit" => "e".to_string(),
        PermissionKind::Other => (n + 1).to_string(),
    }
}

/// The answer `key` gives the question waiting in `thread`, if one is and
/// it's one of its keys: `y`/`Y` yes (always), `n`/`N` no (always), `1`-`9`
/// by its place. The thread's keys and the prompt's (empty) both answer.
pub fn answer_for(thread: &Thread, key: Key) -> Option<Action> {
    // The newest question waiting, of either kind.
    let waiting = thread.entries.iter().rev().find(|e| matches!(e, Entry::Permission(c) if c.state == PermissionState::Pending) || matches!(e, Entry::Elicitation(c) if c.state == ElicitationState::Pending))?;
    if let Entry::Elicitation(card) = waiting {
        let how = match key {
            Key::Char('y') => ElicitAnswer::Fill,
            Key::Char('Y') if card.request.mode == fenix_acp::protocol::ElicitMode::Form => ElicitAnswer::Defaults,
            Key::Char('n') => ElicitAnswer::Decline,
            Key::Char('x') => ElicitAnswer::Cancel,
            _ => return None,
        };
        return Some(Action::Elicit { key: card.key.clone(), how });
    }
    let Entry::Permission(card) = waiting else { return None };
    let by_kind = |kind: PermissionKind| card.options.iter().find(|o| o.kind == kind).map(|o| o.id.clone());
    let option = match key {
        Key::Char('y') => by_kind(PermissionKind::AllowOnce).or_else(|| by_kind(PermissionKind::AllowAlways)),
        Key::Char('Y') => by_kind(PermissionKind::AllowAlways),
        Key::Char('n') => by_kind(PermissionKind::RejectOnce).or_else(|| by_kind(PermissionKind::RejectAlways)),
        Key::Char('N') => by_kind(PermissionKind::RejectAlways),
        Key::Char(c @ '1'..='9') => card.options.get(c as usize - '1' as usize).map(|o| o.id.clone()),
        // A pipeline's gate: edit what's passed on.
        Key::Char('e') => card.options.iter().find(|o| o.id == "edit").map(|o| o.id.clone()),
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
        // The whole command, then what the agent gave, as it gave it:
        // what's asked can be read to its end before it's answered.
        Entry::Permission(card) => {
            let call = &card.tool_call;
            let mut out = asked_command(call).or_else(|| call.title.clone()).unwrap_or_default();
            if let Some(input) = &call.raw_input {
                let input = serde_json::to_string_pretty(input).unwrap_or_default();
                out.push_str("\n\n");
                out.push_str(&input);
            }
            out
        }
        Entry::Elicitation(card) => card.request.message.clone(),
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

/// A question answered, on a line: `✓ {what} — {answer}`, the question
/// cut to leave the answer whole.
fn answered(mark: &str, what: &str, answer: &str, width: usize) -> String {
    let answer = fit(answer, width.saturating_sub(4).max(1));
    let room = width.saturating_sub(str_cells(mark) + 1 + 3 + str_cells(&answer));
    fit(&format!("{mark} {} — {answer}", fit(what, room)), width)
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
    // The thread's title is the dock's own title bar's; here, the agent
    // and how it's doing, then its mode, model and what it has used --
    // each part whole, going on to a second row rather than off the edge.
    let mut parts: Vec<(String, Role, Option<Chip>)> = vec![(status.agent.clone(), Role::Accent, None)];
    if !status.state.is_empty() {
        let role = status.state_role.unwrap_or(Role::Muted);
        parts.push((status.state.clone(), role, Some(Chip::Status(role))));
    }
    if let Some(modes) = &thread.modes {
        let name = modes.available.iter().find(|m| m.id == modes.current).map(|m| m.name.as_str()).unwrap_or(modes.current.as_str());
        parts.push((name.to_string(), Role::Text, None));
    }
    if let Some(model) = thread.config_options.iter().find(|o| o.category.as_deref() == Some("model")) {
        parts.push((model.current_label(), Role::Text, None));
    }
    if let Some(usage) = &thread.usage {
        let mut text = format!("{}% context", usage.percent());
        if let Some((amount, currency)) = &usage.cost {
            text.push_str(&format!(" · {amount:.2} {currency}"));
        }
        parts.push((text, Role::Muted, None));
    }
    let mut x = left;
    for (i, (text, role, chip)) in parts.iter().enumerate() {
        let text = fit(text, width);
        let gap = if i == 0 { 0 } else if chip.is_some() { 2 } else { 3 };
        if i > 0 && x + gap + text.chars().count() > left + width {
            y += 1;
            x = left;
        } else if i > 0 {
            x = if chip.is_some() { g.put(y, x, "  ", Role::Muted) } else { g.put(y, x, " · ", Role::Muted) };
        }
        x = match chip {
            Some(chip) => g.chip(y, x, &text, *role, *chip),
            None => g.put(y, x, &text, *role),
        };
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
        // Wrapped in a narrow dock, not run past its edge.
        for line in wrap(hint, width.max(12)) {
            g.put(y, left, &line, Role::Muted);
            y += 1;
        }
        y += 1;
    }
    let inner = width.saturating_sub(2);
    // Tool calls one after another are one card: where it started.
    let mut tools_from: Option<usize> = None;
    for (n, &i) in visible.iter().enumerate() {
        let entry = &thread.entries[i];
        let top = y;
        let open = page.is_open(i, entry);
        let mut focused = false;
        match entry {
            Entry::User { blocks, queued } => {
                let mut x = g.put(y, left + 1, "› You", Role::Accent);
                if *queued {
                    g.put(y, x + 1, &fit("(queued — C-c takes it back)", (left + width).saturating_sub(x + 2)), Role::Muted);
                }
                y += 1;
                for block in blocks {
                    match block {
                        ContentBlock::Text(text) => {
                            for line in text.lines().flat_map(|l| wrap(l, inner.saturating_sub(1))) {
                                g.put(y, left + 3, &line, Role::Text);
                                y += 1;
                            }
                        }
                        other => {
                            x = g.put(y, left + 3, &fit(&one_line(&other.as_text()), inner.saturating_sub(1)), Role::Accent);
                            let _ = x;
                            y += 1;
                        }
                    }
                }
                // What you wrote, on an input's surface.
                g.card(top..y, left..left + width, CardLook::Input);
            }
            Entry::Agent { text, extra, .. } => {
                if !text.trim().is_empty() {
                    let laid = markdown(text, width, status.highlight);
                    let lines = laid.text.lines().count();
                    g.embed(&laid, y, left, false);
                    y += lines;
                }
                for block in extra {
                    g.put(y, left, &fit(&one_line(&block.as_text()), width), Role::Muted);
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
                let mut x = g.put(y, left + 1, mark, role) + 1;
                x = g.put(y, x, kind_mark(call.kind), Role::Muted) + 1;
                g.put(y, x, &fit(&one_line(&call.title), (left + width).saturating_sub(x + 1)), Role::Text);
                y += 1;
                if let Some(command) = command_shown(call, status) {
                    g.put(y, left + 3, &fit(&format!("$ {command}"), inner.saturating_sub(1)), Role::Accent);
                    y += 1;
                }
                if let Some(l) = call.locations.first() {
                    let shown = short_path(status, &l.path);
                    let place = match l.line {
                        Some(line) => format!("{shown}:{line}"),
                        None => shown,
                    };
                    g.put(y, left + 3, &fit(&place, inner.saturating_sub(1)), Role::Accent);
                    y += 1;
                }
                if open {
                    for content in &call.content {
                        match content {
                            ToolContent::Diff { path, old_text, new_text } => {
                                g.put(y, left + 3, &fit(&short_path(status, path), inner.saturating_sub(1)), Role::Muted);
                                y += 1;
                                for line in diff_lines(old_text.as_deref().unwrap_or(""), new_text).iter().take(60) {
                                    let role = match line.chars().next() {
                                        Some('+') => Role::Good,
                                        Some('-') => Role::Bad,
                                        _ => Role::Muted,
                                    };
                                    g.put(y, left + 3, &fit(line, inner.saturating_sub(1)), role);
                                    y += 1;
                                }
                            }
                            ToolContent::Content(block) => {
                                let text = block.as_text();
                                let lines: Vec<&str> = text.lines().filter(|l| !l.trim_start().starts_with("```")).collect();
                                let shown = lines.len().min(40);
                                for line in &lines[lines.len() - shown..] {
                                    g.put(y, left + 3, &fit(line, inner.saturating_sub(1)), Role::Muted);
                                    y += 1;
                                }
                                if lines.len() > shown {
                                    g.put(y, left + 3, &format!("({} lines above; v shows all)", lines.len() - shown), Role::Muted);
                                    y += 1;
                                }
                            }
                            ToolContent::Terminal(id) => match status.terminals.get(id) {
                                Some(TermView { tail, exit, .. }) => {
                                    for line in tail.lines() {
                                        g.put(y, left + 3, &fit(line, inner.saturating_sub(1)), Role::Muted);
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
                                    g.put(y, left + 3, &text, role);
                                    y += 1;
                                }
                                None => {
                                    g.put(y, left + 3, &format!("terminal {id}"), Role::Muted);
                                    y += 1;
                                }
                            },
                        }
                    }
                }
            }
            Entry::Permission(card) => {
                let title = one_line(card.tool_call.title.as_deref().unwrap_or(""));
                let title = if title.is_empty() { "a tool".to_string() } else { title };
                let fenix = card.tool_call.id.starts_with("fenix-pipeline-");
                match &card.state {
                    PermissionState::Pending => {
                        let command = (!fenix).then(|| asked_command(&card.tool_call)).flatten();
                        let room = width.saturating_sub(2);
                        match &command {
                            // What's asked to run, its own lines: the start
                            // says what it is, the end is where a `&& rm -rf`
                            // would be.
                            Some(command) => {
                                let lines = command.trim_end().lines().count();
                                let head = if lines > 1 { format!("? The agent asks to run a command ({lines} lines)") } else { "? The agent asks to run a command".to_string() };
                                g.put(y, left + 1, &fit(&head, room), Role::Warn);
                                y += 1;
                                let rows = command_rows(command, inner.saturating_sub(2));
                                let shown: Vec<(String, Role)> = if rows.len() <= COMMAND_ROWS {
                                    rows.into_iter().map(|r| (r, Role::Accent)).collect()
                                } else {
                                    let more = rows.len() - COMMAND_HEAD - 1;
                                    let mut shown: Vec<(String, Role)> = rows[..COMMAND_HEAD].iter().map(|r| (r.clone(), Role::Accent)).collect();
                                    shown.push((format!("  … {more} more lines · v shows it all …"), Role::Muted));
                                    shown.push((rows[rows.len() - 1].clone(), Role::Accent));
                                    shown
                                };
                                for (row, role) in shown {
                                    g.put(y, left + 3, &fit(&row, inner.saturating_sub(2)), role);
                                    y += 1;
                                }
                            }
                            // Anything else: its title, on three rows at most.
                            None => {
                                let said = if fenix { format!("? Fenix asks: {title}") } else { format!("? The agent asks to: {title}") };
                                let rows = wrap(&said, room.saturating_sub(2).max(1));
                                let n = rows.len().min(3);
                                for (i, row) in rows.iter().take(n).enumerate() {
                                    let row = if i + 1 == n && rows.len() > n { fit(&format!("{row} {}", rows[n]), room.saturating_sub(2)) } else { row.clone() };
                                    g.put(y, if i == 0 { left + 1 } else { left + 3 }, &row, Role::Warn);
                                    y += 1;
                                }
                            }
                        }
                        // One answer a line, each on its key's cap; the one
                        // `j` and `k` are on in the accent.
                        let on = page.chosen(card);
                        let mut chosen_row = top;
                        for (n, option) in card.options.iter().enumerate() {
                            let chosen = n == on;
                            if chosen {
                                // Its whole row, not only its cap.
                                g.panels.push((y, left + 2..(left + width).saturating_sub(1)));
                                chosen_row = y;
                            }
                            let x = g.chip(y, left + 3, &option_key(option, n), Role::Accent, if chosen { Chip::Chosen } else { Chip::Key });
                            g.put(y, x + 2, &fit(&one_line(&option.name), (left + width).saturating_sub(x + 3)), if chosen { Role::Accent } else { Role::Text });
                            y += 1;
                        }
                        g.put(y, left + 3, &fit("j k choose · Enter answers · or its key, from anywhere", inner.saturating_sub(2)), Role::Muted);
                        y += 1;
                        // Waiting on you: outlined.
                        g.card(top..y, left..left + width, CardLook::Ask);
                        // The answer chosen is the row kept in view, so the
                        // answers are there in a dock too short for the
                        // whole card.
                        if i == page.focus {
                            g.focus(chosen_row, left..left + width);
                            focused = true;
                        }
                    }
                    PermissionState::Answered(name) => {
                        g.put(y, left, &answered("✓", &title, &one_line(name), width), Role::Muted);
                        y += 1;
                    }
                    PermissionState::Withdrawn => {
                        g.put(y, left, &answered("–", &title, "not answered", width), Role::Muted);
                        y += 1;
                    }
                }
            }
            Entry::Elicitation(card) => {
                let e = &card.request;
                let url = e.mode == fenix_acp::protocol::ElicitMode::Url;
                match &card.state {
                    ElicitationState::Pending => {
                        for (i, line) in wrap(&format!("? The agent asks: {}", e.message), width.saturating_sub(2)).into_iter().enumerate() {
                            g.put(y, if i == 0 { left } else { left + 2 }, &line, Role::Warn);
                            y += 1;
                        }
                        if url {
                            g.put(y, left + 2, &fit(&one_line(e.url.as_deref().unwrap_or("")), inner), Role::Accent);
                            y += 1;
                        }
                        for f in &e.fields {
                            let mut what = format!("{}{}", one_line(&f.title), if f.required { " *" } else { "" });
                            if let Some(d) = &f.default {
                                what.push_str(&format!(" = {}", d.as_str().map(str::to_string).unwrap_or_else(|| d.to_string())));
                            }
                            g.put(y, left + 2, &fit(&what, inner), Role::Text);
                            y += 1;
                            for line in f.description.iter().flat_map(|d| wrap(d, inner.saturating_sub(2))) {
                                g.put(y, left + 4, &line, Role::Muted);
                                y += 1;
                            }
                            if let fenix_acp::protocol::FieldKind::Choice { options, .. } = &f.kind {
                                for (_, title) in options {
                                    for (i, line) in wrap(title, inner.saturating_sub(4)).into_iter().enumerate() {
                                        g.put(y, left + 4, &if i == 0 { format!("· {line}") } else { format!("  {line}") }, Role::Muted);
                                        y += 1;
                                    }
                                }
                            }
                        }
                        // `Y` only when the defaults make an answer.
                        let defaults_do = !e.fields.is_empty() && e.problems(&e.defaults()).is_empty();
                        let keys: &[(&str, &str)] = if url {
                            &[("y", "open the link"), ("n", "decline"), ("x", "cancel")]
                        } else if defaults_do {
                            &[("y", "answer"), ("Y", "the defaults"), ("n", "decline"), ("x", "cancel")]
                        } else {
                            &[("y", "answer"), ("n", "decline"), ("x", "cancel")]
                        };
                        // As many a row as fit, the rest on the next.
                        let mut x = left + 2;
                        for (k, what) in keys {
                            if x > left + 2 && x + k.chars().count() + 1 + what.chars().count() > left + width {
                                y += 1;
                                x = left + 2;
                            }
                            x = g.chip(y, x, k, Role::Accent, Chip::Key) + 1;
                            x = g.put(y, x, &fit(what, (left + width).saturating_sub(x)), Role::Text) + 3;
                        }
                        y += 1;
                        g.card(top..y, left..left + width, CardLook::Ask);
                    }
                    ElicitationState::Answered { action, summary } => {
                        let said = if summary.is_empty() { action.clone() } else { format!("{action}: {summary}") };
                        g.put(y, left, &answered("✓", &one_line(&e.message), &one_line(&said), width), Role::Muted);
                        y += 1;
                    }
                    ElicitationState::Completed => {
                        g.put(y, left, &answered("✓", &one_line(&e.message), "done", width), Role::Muted);
                        y += 1;
                    }
                    ElicitationState::Withdrawn => {
                        g.put(y, left, &answered("–", &one_line(&e.message), "not answered", width), Role::Muted);
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
        if i == page.focus && !focused {
            g.focus(top, left..left + width);
        }
        // A tool call starts or carries on a card; the next entry being
        // one too keeps it going, with no gap between them.
        if matches!(entry, Entry::Tool(_)) {
            let from = *tools_from.get_or_insert(top);
            let next_is_tool = visible.get(n + 1).is_some_and(|&j| matches!(thread.entries[j], Entry::Tool(_)));
            if next_is_tool {
                continue;
            }
            g.card(from..y, left..left + width, CardLook::Plain);
            tools_from = None;
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
            g.put(y, x, &fit(&one_line(&entry.content), width.saturating_sub(2)), if entry.status == PlanStatus::Completed { Role::Muted } else { Role::Text });
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

    /// A tool card names a file in the project from the project, not the
    /// whole path the agent gave.
    #[test]
    fn a_tool_cards_path_is_from_the_project() {
        let t = thread();
        let page = AgentPage::new(ThreadId("x".into()));
        let status = Status { root: Some(std::path::PathBuf::from("/p")), ..status() };
        let text = layout(&page, &t, &Link::Detached, &Turn::Idle, &status, 100).all_text();
        assert!(text.contains("src/parse.rs:2") && !text.contains("/p/src/parse.rs"), "{text}");
    }

    #[test]
    fn the_thread_reads_as_cards_with_the_plan_below() {
        let t = thread();
        let mut page = AgentPage::new(ThreadId("x".into()));
        page.follow_up(&t);
        let text = layout(&page, &t, &Link::Detached, &Turn::Idle, &status(), 100).all_text();
        for want in ["Claude Code  ready", "› You", "▸ thinking  Look at parse.rs first.", "✓ edit Edit src/parse.rs", "/p/src/parse.rs:2", "-b", "+B", "✓ run cargo test", "Fixed it.", "fn x() {}", "PLAN · 1/2", "☑ Fix", "◐ Test"] {
            assert!(text.contains(want), "missing {want:?} in\n{text}");
        }
        assert!(!text.contains("test ok"), "a command's output is folded");
        assert!(!text.contains("Then the tests."), "thinking is folded");
        assert_eq!(page.focus, 4, "following the newest card");
    }

    /// What the agent says is Markdown, and reads as it would rendered:
    /// no `#`, `**` or backticks, a heading and inline code in colours of
    /// their own, list markers, a table's grid, code without its fences --
    /// and yanking it still gives the Markdown it said.
    #[test]
    fn what_the_agent_says_is_rendered_markdown() {
        let mut t = Thread::default();
        let said = "## The fix\n\nThe parser **skipped** `\\r\\n` endings:\n\n- read the line\n- trim *both*\n\n| file | change |\n|---|---|\n| parse.rs | trim |\n\n```rust\nfn trim() {}\n```\n\nSee [the docs](https://example.com).";
        t.apply(SessionUpdate::parse(&json!({ "sessionUpdate": "agent_message_chunk", "content": { "type": "text", "text": said } })), false);
        let page = AgentPage::new(ThreadId("x".into()));
        let laid = layout(&page, &t, &Link::Detached, &Turn::Idle, &status(), 100);
        let text = laid.all_text();
        for want in ["The fix", "The parser skipped \\r\\n endings:", "read the line", "trim both", "parse.rs", "fn trim() {}", "the docs"] {
            assert!(text.contains(want), "missing {want:?} in\n{text}");
        }
        for gone in ["##", "**", "`", "```", "|---|", "](https://"] {
            assert!(!text.contains(gone), "{gone:?} left in\n{text}");
        }
        let line = |needle: &str| text.lines().position(|l| l.contains(needle)).unwrap();
        let roles = |n: usize| laid.spans.iter().filter(|s| s.line == n).map(|s| s.role).collect::<Vec<_>>();
        assert!(roles(line("The fix")).iter().any(|r| *r != Role::Text), "the heading has a style of its own");
        assert!(roles(line("The parser")).len() > 1, "emphasis and inline code are styled apart from the text");
        let mut page = page;
        assert_eq!(page.key(&t, Key::Char('y')), Action::Yank(said.to_string()), "yanked as Markdown");
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
        assert!(text.contains("? The agent asks to: rm -rf build") && text.contains("y  Yes") && text.contains("n  No"), "{text}");
        page.key(&t, Key::Char('g'));
        assert_eq!(page.key(&t, Key::Char('n')), Action::Permission { key: "7".into(), option: "reject".into() });
        assert_eq!(page.key(&t, Key::Char('Y')), Action::Permission { key: "7".into(), option: "allow-always".into() });
        assert_eq!(page.key(&t, Key::Char('2')), Action::Permission { key: "7".into(), option: "allow-always".into() });
    }

    fn asking() -> Thread {
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
        t
    }

    /// The thread's title is the dock's title bar's, not a line of the
    /// page; the status row's parts go on to a second row in a narrow
    /// dock, each whole, rather than off the edge.
    #[test]
    fn the_header_has_no_title_and_wraps_its_parts_whole() {
        let t = thread();
        let page = AgentPage::new(ThreadId("x".into()));
        let wide = layout(&page, &t, &Link::Detached, &Turn::Idle, &status(), 100).all_text();
        let first = wide.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
        assert!(first.trim_start().starts_with("Claude Code"), "the status first; the title is the dock's: {wide}");
        let busy = Status { agent: "Claude Code".into(), state: "waiting on you".into(), ..Status::default() };
        let mut t = t;
        t.modes = Some(fenix_acp::protocol::Modes { current: "default".into(), available: vec![fenix_acp::protocol::Mode { id: "default".into(), name: "Default (recommended)".into(), description: None }] });
        let narrow = layout(&page, &t, &Link::Detached, &Turn::Idle, &busy, 44).text;
        let rows: Vec<&str> = narrow.lines().take(4).collect();
        assert!(rows.iter().any(|r| r.contains("Claude Code") && r.contains("waiting on you")), "{rows:?}");
        assert!(rows.iter().any(|r| r.trim_start().starts_with("Default (recommended)")), "the mode whole, on a row of its own: {rows:?}");
        assert!(narrow.lines().all(|l| l.chars().count() <= 44), "nothing past the edge");
    }

    /// On the question, `j` and `k` go from answer to answer, Enter gives
    /// the one they're on, and past the last `j` goes back to the cards.
    #[test]
    fn j_and_k_choose_an_answer_and_enter_gives_it() {
        let t = asking();
        let mut page = AgentPage::new(ThreadId("x".into()));
        page.follow_up(&t);
        let question = t.entries.len() - 1;
        assert_eq!(page.focus, question);
        let laid = layout(&page, &t, &Link::Detached, &Turn::Idle, &status(), 100);
        let yes = laid.text.lines().position(|l| l.contains("y  Yes")).unwrap();
        assert!(laid.chips.iter().any(|(line, _, chip)| *line == yes && *chip == Chip::Chosen), "the first that allows it is chosen");
        assert_eq!(page.key(&t, Key::Char('j')), Action::None);
        assert_eq!(page.key(&t, Key::Char('j')), Action::None);
        assert_eq!(page.key(&t, Key::Enter), Action::Permission { key: "7".into(), option: "reject".into() });
        assert_eq!(page.key(&t, Key::Char('k')), Action::None);
        assert_eq!(page.key(&t, Key::Enter), Action::Permission { key: "7".into(), option: "allow-always".into() });
        let laid = layout(&page, &t, &Link::Detached, &Turn::Idle, &status(), 100);
        let always = laid.text.lines().position(|l| l.contains("Y  Yes, and")).unwrap();
        assert!(laid.chips.iter().any(|(line, _, chip)| *line == always && *chip == Chip::Chosen));
        page.key(&t, Key::Char('k'));
        page.key(&t, Key::Char('k'));
        assert_ne!(page.focus, question, "past the first answer, on to the card above");
        assert!(laid.panels.iter().any(|(line, _)| *line == always), "the chosen answer's row is lit");
    }

    /// A new question takes the cursor even when you'd moved up the
    /// thread -- once, so you can still move away from it.
    #[test]
    fn a_new_question_takes_the_cursor_once() {
        let t = asking();
        let mut page = AgentPage::new(ThreadId("x".into()));
        page.follow = false;
        page.focus = 0;
        page.follow_up(&t);
        assert_eq!(page.focus, t.entries.len() - 1, "on the question");
        page.focus = 0;
        page.follow_up(&t);
        assert_eq!(page.focus, 0, "not pulled back to it");
    }

    /// Tool calls one after another are one card, what you wrote is on
    /// an input's, and the question waiting is outlined.
    /// An empty thread's hint wraps in a narrow dock.
    #[test]
    fn the_empty_threads_hint_keeps_inside_a_narrow_dock() {
        let t = Thread::default();
        let page = AgentPage::new(ThreadId("x".into()));
        let laid = layout(&page, &t, &Link::Detached, &Turn::Idle, &status(), 44);
        assert!(laid.text.lines().all(|l| l.trim_end().chars().count() <= 44), "{}", laid.text);
        assert!(laid.text.contains("sends it."), "{}", laid.text);
    }

    #[test]
    fn the_thread_is_set_off_in_cards() {
        let t = asking();
        let mut page = AgentPage::new(ThreadId("x".into()));
        page.follow_up(&t);
        let laid = layout(&page, &t, &Link::Detached, &Turn::Idle, &status(), 100);
        let lines: Vec<&str> = laid.text.lines().collect();
        let line_of = |want: &str| lines.iter().position(|l| l.contains(want)).unwrap();
        let card_at = |line: usize| laid.cards.iter().find(|c| c.lines.contains(&line)).map(|c| c.look);
        assert_eq!(card_at(line_of("› You")), Some(CardLook::Input));
        assert_eq!(card_at(line_of("✓ edit")), Some(CardLook::Plain));
        assert_eq!(card_at(line_of("✓ edit")), card_at(line_of("✓ run")));
        let tools = laid.cards.iter().filter(|c| c.look == CardLook::Plain).count();
        assert_eq!(tools, 1, "two tool calls in a row are one card: {:?}", laid.cards);
        assert_eq!(card_at(line_of("rm -rf build")), Some(CardLook::Ask));
        assert_eq!(card_at(line_of("Fixed it.")), None, "what the agent says is on the page");
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

    /// The plan step's command that started this: a heredoc as the title.
    fn heredoc() -> String {
        "cat > /tmp/plan.md <<'EOF'\n# Plan\n1. Change a.rs\n2. Test it\n3. Ship\nEOF".to_string()
    }

    fn asking_with(tool_call: ToolCallFields) -> Thread {
        let mut t = thread();
        t.push_permission(PermissionCard {
            key: "9".into(),
            tool_call,
            options: vec![
                PermissionOption { id: "allow-once".into(), name: "Yes".into(), kind: PermissionKind::AllowOnce },
                PermissionOption { id: "reject".into(), name: "No".into(), kind: PermissionKind::RejectOnce },
            ],
            state: PermissionState::Pending,
        });
        t
    }

    /// A permission card titled with a heredoc: the row of the text *Yes*
    /// is the row of its chosen chip and of its panel, not one off for
    /// each newline in the title.
    #[test]
    fn a_heredoc_title_keeps_the_answers_on_their_rows() {
        let t = asking_with(ToolCallFields { id: "t9".into(), title: Some(heredoc()), kind: Some(ToolKind::Execute), ..Default::default() });
        let mut page = AgentPage::new(ThreadId("x".into()));
        page.follow_up(&t);
        let laid = layout(&page, &t, &Link::Detached, &Turn::Idle, &status(), 100);
        let yes = laid.text.lines().position(|l| l.contains("y  Yes")).expect("the answer is on the page");
        let chosen: Vec<usize> = laid.chips.iter().filter(|(_, _, c)| *c == Chip::Chosen).map(|(l, _, _)| *l).collect();
        assert_eq!(chosen, [yes], "the chip is on the text\n{}", laid.text);
        assert!(laid.panels.iter().any(|(l, _)| *l == yes), "the panel is on the text: {:?}\n{}", laid.panels, laid.text);
        assert_eq!(laid.text.lines().count(), laid.text.split('\n').count());
    }

    /// Text no row should break on: line breaks, tabs, `\r\n`, a control
    /// character, a very long word, wide characters.
    fn hostile() -> Vec<String> {
        vec![
            "one\ntwo\n\nthree\r\nfour\tfive\u{1b}[31m".to_string(),
            "x".repeat(10_000),
            "日本語のテキスト😀 ".repeat(40),
            format!("{}\n{}", "長い".repeat(300), "a\tb\r\nc"),
        ]
    }

    /// Every kind of entry in each of its states, each of its strings `s`.
    fn hostile_thread(s: &str) -> Thread {
        use fenix_acp::protocol::{ElicitMode, Elicitation, Field, FieldKind};
        use fenix_acp::thread::ElicitationCard;
        let mut t = Thread::default();
        t.push_user(vec![ContentBlock::text(s)], false);
        t.push_user(vec![ContentBlock::text(s)], true);
        let mut updates = vec![
            json!({ "sessionUpdate": "agent_thought_chunk", "content": { "type": "text", "text": s } }),
            json!({ "sessionUpdate": "agent_message_chunk", "content": { "type": "text", "text": s } }),
            json!({ "sessionUpdate": "plan", "entries": [{ "content": s, "priority": "high", "status": "completed" }, { "content": s, "priority": "low", "status": "pending" }] }),
        ];
        for (n, status) in ["pending", "in_progress", "completed", "failed"].iter().enumerate() {
            updates.push(json!({ "sessionUpdate": "tool_call", "toolCallId": format!("e{n}"), "title": s, "kind": "execute", "status": status, "rawInput": { "command": s }, "content": [{ "type": "content", "content": { "type": "text", "text": s } }, { "type": "terminal", "terminalId": "term" }] }));
            updates.push(json!({ "sessionUpdate": "tool_call", "toolCallId": format!("d{n}"), "title": s, "kind": "edit", "status": status, "content": [{ "type": "diff", "path": s, "oldText": s, "newText": format!("{s}\nmore") }], "locations": [{ "path": s, "line": 3 }] }));
        }
        for u in updates {
            t.apply(SessionUpdate::parse(&u), false);
        }
        let options = vec![
            PermissionOption { id: "allow-once".into(), name: s.into(), kind: PermissionKind::AllowOnce },
            PermissionOption { id: "reject".into(), name: s.into(), kind: PermissionKind::RejectOnce },
        ];
        for (n, state) in [PermissionState::Answered(s.into()), PermissionState::Withdrawn, PermissionState::Pending].into_iter().enumerate() {
            for (m, kind) in [Some(ToolKind::Execute), Some(ToolKind::Edit), None].into_iter().enumerate() {
                let id = if m == 2 { format!("fenix-pipeline-gate-{n}") } else { format!("p{n}{m}") };
                t.push_permission(PermissionCard { key: format!("{n}{m}"), tool_call: ToolCallFields { id, title: Some(s.into()), kind, raw_input: (m == 0).then(|| json!({ "command": s })), ..Default::default() }, options: options.clone(), state: state.clone() });
            }
        }
        let field = Field { name: "f".into(), title: s.into(), description: Some(s.into()), kind: FieldKind::Choice { options: vec![("a".into(), s.into())], multi: false }, default: Some(json!(s)), required: true };
        for state in [ElicitationState::Answered { action: s.into(), summary: s.into() }, ElicitationState::Completed, ElicitationState::Withdrawn, ElicitationState::Pending] {
            for mode in [ElicitMode::Url, ElicitMode::Form] {
                let request = Elicitation { mode, message: s.into(), tool_call_id: None, fields: vec![field.clone()], url: Some(s.into()), elicitation_id: None };
                t.push_elicitation(ElicitationCard { key: "e".into(), request, state: state.clone() });
            }
        }
        for kind in [NoticeKind::Error, NoticeKind::Warning, NoticeKind::Login, NoticeKind::Interrupted, NoticeKind::Info] {
            t.notice(kind, s);
        }
        t.modes = Some(fenix_acp::protocol::Modes { current: "m".into(), available: vec![fenix_acp::protocol::Mode { id: "m".into(), name: s.into(), description: None }] });
        t
    }

    /// Whatever text it's given, at 30, 60 and 110 columns, open or
    /// folded: as many lines as rows, nothing past the edge in cells,
    /// every chip on its own text, no control character drawn.
    #[test]
    fn hostile_text_keeps_every_card_to_its_rows_and_width() {
        for s in hostile() {
            let t = hostile_thread(&s);
            let mut status = Status { agent: s.clone(), state: s.clone(), logins: vec![("a".into(), s.clone())], ..status() };
            status.terminals.insert("term".into(), TermView { line: s.clone(), tail: s.clone(), exit: Some(fenix_acp::protocol::ExitStatus { code: Some(1), signal: None }) });
            for open in [false, true] {
                let mut page = AgentPage::new(ThreadId("x".into()));
                page.follow_up(&t);
                if open {
                    page.open = (0..t.entries.len()).collect();
                }
                for cols in [30, 60, 110] {
                    let laid = layout(&page, &t, &Link::Detached, &Turn::Idle, &status, cols);
                    let lines: Vec<&str> = laid.text.split('\n').collect();
                    let what = format!("{cols} columns, open {open}, text {:?}", s.chars().take(20).collect::<String>());
                    assert!(laid.text.chars().all(|c| c == '\n' || !c.is_control()), "a control character drawn: {what}");
                    let over = crate::page::overflow(&laid, cols);
                    assert!(over.is_empty(), "{what}: {over:#?}");
                    for (line, range, chip) in &laid.chips {
                        if matches!(chip, Chip::Switch(_)) {
                            continue;
                        }
                        let text: String = lines[*line].chars().skip(range.start).take(range.len()).collect();
                        assert!(!text.trim().is_empty(), "{what}: a chip on line {line} {range:?} over {:?}", lines[*line]);
                    }
                    for card in &laid.cards {
                        assert!(card.lines.end <= lines.len(), "{what}: a card past the page");
                    }
                }
            }
        }
    }

    /// In a dock too short for the whole card, the row kept in view is the
    /// answer chosen, not the card's first: the answers are on screen.
    #[test]
    fn the_answers_stay_in_view_in_a_short_dock() {
        let command = (1..=30).map(|n| format!("echo {n}")).collect::<Vec<_>>().join("\n");
        let t = asking_with(ToolCallFields { id: "t9".into(), title: Some(command.clone()), kind: Some(ToolKind::Execute), raw_input: Some(json!({ "command": command })), ..Default::default() });
        let mut page = AgentPage::new(ThreadId("x".into()));
        page.follow_up(&t);
        let laid = layout(&page, &t, &Link::Detached, &Turn::Idle, &status(), 70);
        let yes = laid.text.lines().position(|l| l.contains("y  Yes")).unwrap();
        assert_eq!(page.focus, t.entries.len() - 1);
        assert_eq!(laid.cursor().0, yes, "the chosen answer is the row kept in view\n{}", laid.text);
        let ask = laid.text.lines().position(|l| l.contains("asks to run a command (30 lines)")).unwrap();
        assert!(yes - ask <= 9, "at most eight rows of the command above the answers\n{}", laid.text);
        let text = laid.text.clone();
        for want in ["$ echo 1", "echo 5", "… 24 more lines · v shows it all …", "echo 30"] {
            assert!(text.contains(want), "missing {want:?}\n{text}");
        }
        assert!(!text.contains("echo 6\n"), "{text}");
        // `v`: the whole command, then its input as the agent gave it.
        let Action::View(shown) = page.key(&t, Key::Char('v')) else { panic!() };
        assert!(shown.starts_with("echo 1\necho 2\n") && shown.contains("echo 30\n\n{"), "{shown}");
    }

    /// A long title cut on the line it's answered on keeps the answer.
    #[test]
    fn an_answered_question_keeps_its_answer() {
        let mut t = thread();
        t.push_permission(PermissionCard { key: "1".into(), tool_call: ToolCallFields { id: "t".into(), title: Some(format!("{}\nmore", "long ".repeat(40))), ..Default::default() }, options: Vec::new(), state: PermissionState::Answered("Yes, and don't ask again".into()) });
        let page = AgentPage::new(ThreadId("x".into()));
        let text = layout(&page, &t, &Link::Detached, &Turn::Idle, &status(), 60).text;
        let line = text.lines().find(|l| l.contains("✓ long")).unwrap();
        assert!(line.ends_with("— Yes, and don't ask again") && line.contains("…"), "{line}");
    }

    #[test]
    fn diffs_show_changes_with_a_line_of_context() {
        assert_eq!(diff_lines("a\nb\nc\nd\ne", "a\nb\nC\nd\ne"), vec![" b", "-c", "+C", " d"]);
        assert_eq!(diff_lines("", "new"), vec!["+new"]);
    }
}
