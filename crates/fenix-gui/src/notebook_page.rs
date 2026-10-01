//! `SPC n n`: the notebook -- every note, diagram and journal day, newest
//! first under Pinned / This week / Older, with the selected one shown
//! rendered on the right. `/` filters with the notebook's search syntax
//! (words, `tag:`, `type:`, `project:`, `is:todo`, ...), `Tab` steps
//! through the chips. `N` makes a note, `d` a diagram, `Enter` edits,
//! `o` reads, `t` tags, `r` renames, `P` pins, `D` duplicates, `h` shows
//! the kept versions, `p` moves it into the project, `x x` deletes (to
//! the Recycle Bin), `e` exports, `[`/`]` step through journal days.

use std::path::PathBuf;
use std::time::SystemTime;

use fenix_notebook::query::Query;
use fenix_notebook::Kind;

use crate::page::{edit_line, fit, frame, with_caret, Filter, Grid, Key, Page, Popup, Role};
use crate::reading;

/// One entry, as the page shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub id: String,
    pub kind: Kind,
    pub name: String,
    pub tags: Vec<String>,
    pub about: Option<String>,
    pub pinned: bool,
    pub modified: Option<SystemTime>,
    pub date: Option<chrono::NaiveDate>,
    pub text: String,
    pub open_tasks: usize,
    pub versions: usize,
    pub exports: Vec<String>,
    /// Why a diagram doesn't draw.
    pub problem: Option<String>,
    /// A project's own file, listed below the notebook's.
    pub project_file: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    None,
    Close,
    Edit(String),
    Read(String),
    OpenFile(PathBuf),
    NewNote,
    NewDiagram,
    Rename(String, String),
    SetTags(String, Vec<String>),
    Pin(String, bool),
    Duplicate(String),
    Delete(String),
    /// Load the versions of this entry into the history popup.
    History(String),
    Restore(String, PathBuf),
    ToProject(String),
    Export(String),
    Theme(String),
    /// A diagram on its own.
    View(String),
    /// Open the journal day `n` days from the selected one (or today).
    Day(chrono::NaiveDate),
    PinSearch(String),
    Import,
}

/// Which chip is on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Chip {
    All,
    Notes,
    Diagrams,
    Journal,
    Pinned,
    Todo,
    Tag(String),
    Search(String),
}

impl Chip {
    fn label(&self) -> String {
        match self {
            Chip::All => "all".into(),
            Chip::Notes => "notes".into(),
            Chip::Diagrams => "diagrams".into(),
            Chip::Journal => "journal".into(),
            Chip::Pinned => "pinned".into(),
            Chip::Todo => "to do".into(),
            Chip::Tag(t) => format!("#{t}"),
            Chip::Search(s) => format!("/{s}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Mode {
    List,
    Rename { text: String, caret: usize },
    Tags { text: String, caret: usize },
    History { id: String, versions: Vec<(String, PathBuf, String)>, cursor: usize },
}

pub struct NotebookPage {
    pub rows: Vec<Row>,
    /// Tags, most used first.
    pub tags: Vec<String>,
    pub searches: Vec<String>,
    pub project: Option<String>,
    cursor: usize,
    pub filter: Filter,
    chip: usize,
    mode: Mode,
    pub note: Option<(String, bool)>,
    armed_delete: bool,
    /// Where the rows start on the page, for scrolling.
    pub folder: String,
}

/// "4 min ago", "3 h ago", "yesterday", "Mon", "Sep 12".
pub fn age(when: Option<SystemTime>, now: chrono::DateTime<chrono::Local>) -> String {
    let Some(when) = when else { return String::new() };
    let then: chrono::DateTime<chrono::Local> = when.into();
    let secs = (now - then).num_seconds();
    if secs < 60 {
        return "just now".into();
    }
    if secs < 3600 {
        return format!("{} min ago", secs / 60);
    }
    let days = (now.date_naive() - then.date_naive()).num_days();
    match days {
        0 => format!("{} h ago", secs / 3600),
        1 => "yesterday".into(),
        2..=6 => then.format("%a").to_string(),
        _ if then.format("%Y").to_string() == now.format("%Y").to_string() => then.format("%b %-d").to_string(),
        _ => then.format("%b %-d %Y").to_string(),
    }
}

impl NotebookPage {
    pub fn new(rows: Vec<Row>, tags: Vec<String>, searches: Vec<String>, project: Option<String>, folder: String) -> Self {
        NotebookPage { rows, tags, searches, project, cursor: 0, filter: Filter::default(), chip: 0, mode: Mode::List, note: None, armed_delete: false, folder }
    }

    /// New contents, the cursor staying on the same entry.
    pub fn refresh(&mut self, rows: Vec<Row>, tags: Vec<String>, searches: Vec<String>) {
        let before = self.selected().map(|r| r.id.clone());
        let chip = self.chips().get(self.chip).cloned();
        self.rows = rows;
        self.tags = tags;
        self.searches = searches;
        if let Some(c) = chip {
            self.chip = self.chips().iter().position(|x| *x == c).unwrap_or(0);
        }
        if let Some(id) = before {
            self.select(&id);
        }
        self.cursor = self.cursor.min(self.visible().len().saturating_sub(1));
    }

    /// Puts the cursor on `id`, if it's shown.
    pub fn select(&mut self, id: &str) -> bool {
        match self.visible().iter().position(|&i| self.rows[i].id == id) {
            Some(n) => {
                self.cursor = n;
                true
            }
            None => false,
        }
    }

    pub fn typing(&self) -> bool {
        self.filter.typing || matches!(self.mode, Mode::Rename { .. } | Mode::Tags { .. })
    }

    pub fn paste(&mut self, text: &str) {
        let text: String = text.chars().filter(|c| !c.is_control()).collect();
        match &mut self.mode {
            Mode::Rename { text: t, caret } | Mode::Tags { text: t, caret } => crate::page::insert_at(t, caret, &text),
            _ if self.filter.typing => self.filter.paste(&text),
            _ => {}
        }
    }

    pub fn chips(&self) -> Vec<Chip> {
        let mut out = vec![Chip::All, Chip::Notes, Chip::Diagrams, Chip::Journal, Chip::Pinned, Chip::Todo];
        out.extend(self.tags.iter().take(5).map(|t| Chip::Tag(t.clone())));
        out.extend(self.searches.iter().map(|s| Chip::Search(s.clone())));
        out
    }

    /// Shows the kept versions of `id`: (when, file, first line).
    pub fn show_history(&mut self, id: String, versions: Vec<(String, PathBuf, String)>) {
        if versions.is_empty() {
            self.note = Some(("no earlier versions yet -- one is kept each time you come back to edit it".into(), false));
            return;
        }
        self.mode = Mode::History { id, versions, cursor: 0 };
    }

    fn matches(&self, row: &Row) -> bool {
        let chip = self.chips().get(self.chip).cloned().unwrap_or(Chip::All);
        let ok = match &chip {
            Chip::All => true,
            Chip::Notes => row.kind == Kind::Note,
            Chip::Diagrams => row.kind == Kind::Diagram,
            Chip::Journal => row.kind == Kind::Day,
            Chip::Pinned => row.pinned,
            Chip::Todo => row.open_tasks > 0,
            Chip::Tag(t) => row.tags.iter().any(|x| x.eq_ignore_ascii_case(t)),
            Chip::Search(s) => row_matches(&Query::parse(s), row),
        };
        ok && (self.filter.is_empty() || row_matches(&Query::parse(&self.filter.text), row))
    }

    /// The rows shown, in order: pinned, then by when they changed; with
    /// no chip or filter, only the last few journal days.
    fn visible(&self) -> Vec<usize> {
        let plain = self.chip == 0 && self.filter.is_empty();
        let mut idx: Vec<usize> = (0..self.rows.len()).filter(|&i| self.matches(&self.rows[i])).collect();
        idx.sort_by(|&a, &b| {
            let (ra, rb) = (&self.rows[a], &self.rows[b]);
            ra.project_file.is_some().cmp(&rb.project_file.is_some()).then(rb.pinned.cmp(&ra.pinned)).then(rb.modified.cmp(&ra.modified))
        });
        if plain {
            let mut days = 0;
            idx.retain(|&i| {
                if self.rows[i].kind == Kind::Day && !self.rows[i].pinned {
                    days += 1;
                    days <= 3
                } else {
                    true
                }
            });
        }
        idx
    }

    pub fn selected(&self) -> Option<&Row> {
        self.visible().get(self.cursor).map(|&i| &self.rows[i])
    }

    pub fn key(&mut self, key: Key) -> Action {
        if key != Key::Char('x') {
            self.armed_delete = false;
        }
        if self.filter.typing {
            if self.filter.key(key) {
                self.cursor = 0;
            }
            return Action::None;
        }
        match std::mem::replace(&mut self.mode, Mode::List) {
            Mode::List => {}
            Mode::Rename { mut text, mut caret } => {
                match key {
                    Key::Escape => {}
                    Key::Enter => {
                        if let Some(row) = self.selected() {
                            let name = text.trim().to_string();
                            if !name.is_empty() && name != row.name {
                                return Action::Rename(row.id.clone(), name);
                            }
                        }
                    }
                    Key::Space => {
                        crate::page::insert_at(&mut text, &mut caret, " ");
                        self.mode = Mode::Rename { text, caret };
                    }
                    k => {
                        edit_line(&mut text, &mut caret, k);
                        self.mode = Mode::Rename { text, caret };
                    }
                }
                return Action::None;
            }
            Mode::Tags { mut text, mut caret } => {
                match key {
                    Key::Escape => {}
                    Key::Enter => {
                        if let Some(row) = self.selected() {
                            let tags: Vec<String> = text.split([',', ' ']).map(|t| t.trim().trim_start_matches('#').to_string()).filter(|t| !t.is_empty()).collect();
                            return Action::SetTags(row.id.clone(), tags);
                        }
                    }
                    Key::Space => {
                        crate::page::insert_at(&mut text, &mut caret, " ");
                        self.mode = Mode::Tags { text, caret };
                    }
                    Key::Tab => {
                        // Completes the tag being typed from the known ones.
                        let word = text.rsplit([' ', ',']).next().unwrap_or("").trim_start_matches('#').to_lowercase();
                        if !word.is_empty() {
                            if let Some(t) = self.tags.iter().find(|t| t.to_lowercase().starts_with(&word) && t.to_lowercase() != word) {
                                let keep = text.len() - text.rsplit([' ', ',']).next().unwrap_or("").len();
                                text.truncate(keep);
                                text.push_str(t);
                                text.push(' ');
                                caret = text.chars().count();
                            }
                        }
                        self.mode = Mode::Tags { text, caret };
                    }
                    k => {
                        edit_line(&mut text, &mut caret, k);
                        self.mode = Mode::Tags { text, caret };
                    }
                }
                return Action::None;
            }
            Mode::History { id, versions, mut cursor } => {
                match key {
                    Key::Escape | Key::Char('q') | Key::Char('h') => {}
                    Key::Down | Key::Char('j') => {
                        cursor = (cursor + 1).min(versions.len().saturating_sub(1));
                        self.mode = Mode::History { id, versions, cursor };
                    }
                    Key::Up | Key::Char('k') => {
                        cursor = cursor.saturating_sub(1);
                        self.mode = Mode::History { id, versions, cursor };
                    }
                    Key::Enter => {
                        if let Some((_, path, _)) = versions.get(cursor) {
                            return Action::Restore(id, path.clone());
                        }
                    }
                    _ => self.mode = Mode::History { id, versions, cursor },
                }
                return Action::None;
            }
        }
        let n = self.visible().len();
        let sel = self.selected().cloned();
        let id = sel.as_ref().map(|r| r.id.clone());
        let is_file = sel.as_ref().and_then(|r| r.project_file.clone());
        let notebook_only = |me: &mut Self| {
            me.note = Some(("that's a project file -- it stays in the repo; the explorer renames and deletes it".into(), false));
        };
        match key {
            Key::Down | Key::Char('j') => self.cursor = (self.cursor + 1).min(n.saturating_sub(1)),
            Key::Up | Key::Char('k') => self.cursor = self.cursor.saturating_sub(1),
            Key::Char('g') => self.cursor = 0,
            Key::Char('G') => self.cursor = n.saturating_sub(1),
            Key::Char('/') => self.filter.start(),
            Key::Tab => {
                self.chip = (self.chip + 1) % self.chips().len();
                self.cursor = 0;
            }
            Key::BackTab => {
                let len = self.chips().len();
                self.chip = (self.chip + len - 1) % len;
                self.cursor = 0;
            }
            Key::Escape if !self.filter.is_empty() => self.filter.clear(),
            Key::Escape if self.chip != 0 => {
                self.chip = 0;
                self.cursor = 0;
            }
            Key::Char('q') | Key::Escape => return Action::Close,
            Key::Char('N') => return Action::NewNote,
            Key::Char('d') => return Action::NewDiagram,
            Key::Char('i') => return Action::Import,
            Key::Char('s') if !self.filter.is_empty() => return Action::PinSearch(self.filter.text.trim().to_string()),
            Key::Char('s') => {
                if let Some(Chip::Search(s)) = self.chips().get(self.chip) {
                    return Action::PinSearch(s.clone());
                }
                self.note = Some(("type a search with / first; s pins it as a chip".into(), false));
            }
            Key::Enter => {
                if let Some(path) = is_file {
                    return Action::OpenFile(path);
                }
                if let Some(id) = id {
                    return Action::Edit(id);
                }
            }
            Key::Char('o') => {
                if let Some(path) = is_file {
                    return Action::OpenFile(path);
                }
                if let Some(id) = id {
                    return Action::Read(id);
                }
            }
            Key::Char('[') | Key::Char(']') => {
                let today = chrono::Local::now().date_naive();
                let base = sel.as_ref().and_then(|r| r.date).unwrap_or(today + chrono::Duration::days(if key == Key::Char('[') { 0 } else { -1 }));
                let step = if key == Key::Char('[') { -1 } else { 1 };
                // The nearest day with an entry, else the next calendar day.
                let mut days: Vec<chrono::NaiveDate> = self.rows.iter().filter_map(|r| r.date).collect();
                days.sort();
                let next = if step < 0 { days.iter().rev().find(|d| **d < base).copied() } else { days.iter().find(|d| **d > base).copied() };
                let target = next.unwrap_or(if step < 0 { base - chrono::Duration::days(1) } else { (base + chrono::Duration::days(1)).min(today) });
                return Action::Day(target);
            }
            _ if id.is_none() => {}
            _ if is_file.is_some() && !matches!(key, Key::Char('e')) => notebook_only(self),
            Key::Char('e') => return Action::Export(id.unwrap()),
            Key::Char('r') => {
                let row = sel.unwrap();
                if row.kind == Kind::Day {
                    self.note = Some(("a journal day is named by its date".into(), false));
                } else {
                    let caret = row.name.chars().count();
                    self.mode = Mode::Rename { text: row.name, caret };
                }
            }
            Key::Char('t') => {
                let row = sel.unwrap();
                if row.kind == Kind::Diagram {
                    self.note = Some(("a diagram has no tags of its own -- tag the notes that embed it".into(), false));
                } else {
                    let mut text = row.tags.join(" ");
                    if !text.is_empty() {
                        text.push(' ');
                    }
                    let caret = text.chars().count();
                    self.mode = Mode::Tags { text, caret };
                }
            }
            Key::Char('P') => return Action::Pin(id.unwrap(), !sel.unwrap().pinned),
            Key::Char('D') => return Action::Duplicate(id.unwrap()),
            Key::Char('h') => return Action::History(id.unwrap()),
            Key::Char('p') => return Action::ToProject(id.unwrap()),
            Key::Char('v') => {
                if sel.unwrap().kind == Kind::Diagram {
                    return Action::View(id.unwrap());
                }
                return Action::Read(id.unwrap());
            }
            Key::Char('T') => {
                if sel.unwrap().kind == Kind::Diagram {
                    return Action::Theme(id.unwrap());
                }
                self.note = Some(("T sets a diagram's theme".into(), false));
            }
            Key::Char('x') => {
                if !self.armed_delete {
                    self.armed_delete = true;
                    self.note = Some(("x again moves it to the Recycle Bin".into(), false));
                    return Action::None;
                }
                self.armed_delete = false;
                return Action::Delete(id.unwrap());
            }
            _ => {}
        }
        Action::None
    }
}

/// Whether a row passes a query, on what the page knows of it.
pub fn row_matches(q: &Query, row: &Row) -> bool {
    if let Some(k) = q.kind {
        if row.kind != k {
            return false;
        }
    }
    if q.pinned && !row.pinned {
        return false;
    }
    if q.todo && row.open_tasks == 0 {
        return false;
    }
    for t in &q.tags {
        if !row.tags.iter().any(|x| x.to_lowercase() == *t || x.to_lowercase().starts_with(&format!("{t}/"))) {
            return false;
        }
    }
    if let Some(p) = &q.project {
        if row.about.as_deref().map(|a| a.to_lowercase() != *p).unwrap_or(true) {
            return false;
        }
    }
    let when = row.date.or_else(|| row.modified.map(|m| chrono::DateTime::<chrono::Local>::from(m).date_naive()));
    if let (Some(after), Some(w)) = (q.after, when) {
        if w < after {
            return false;
        }
    }
    if let (Some(before), Some(w)) = (q.before, when) {
        if w >= before {
            return false;
        }
    }
    let name = row.name.to_lowercase();
    let text = row.text.to_lowercase();
    q.terms.iter().all(|t| name.contains(t) || text.contains(t) || row.tags.iter().any(|x| x.to_lowercase().contains(t)))
}

fn kind_tag(row: &Row) -> (&'static str, Role) {
    if row.project_file.is_some() {
        return ("FILE", Role::Muted);
    }
    match row.kind {
        Kind::Note => ("NOTE", Role::Title),
        Kind::Diagram => ("DIAG", Role::Syntax("function")),
        Kind::Day => ("DAY ", Role::Good),
    }
}

pub fn layout(page: &NotebookPage, cols: usize, ctx: &reading::Ctx) -> Page {
    let now = chrono::Local::now();
    let (left, width) = frame(cols, 150);
    let mut g = Grid::new();
    let x = g.put(1, left, "Notebook", Role::Title) + 2;
    let notes = page.rows.iter().filter(|r| r.project_file.is_none()).count();
    g.put(1, x, &fit(&page.folder, width.saturating_sub(x - left + 16)), Role::Muted);
    let count = format!("{notes} entries");
    g.put(1, (left + width).saturating_sub(count.chars().count()), &count, Role::Muted);
    let mut y = 2;
    let (text, role) = page.filter.line("filter: words, tag:, type:, project:, is:todo", width.min(70));
    let end = g.put(y, left, &text, role);
    if page.filter.typing {
        g.panels.push((y, left..end.max(left + 40)));
    }
    let shown = page.visible();
    let total = format!("{} shown", shown.len());
    g.put(y, (left + width).saturating_sub(total.chars().count()), &total, Role::Muted);
    y += 1;
    // The chips.
    let mut x = left;
    for (i, chip) in page.chips().iter().enumerate() {
        let label = chip.label();
        if x + label.chars().count() + 2 > left + width {
            break;
        }
        let on = i == page.chip;
        let end = g.put(y, x, &format!(" {label} "), if on { Role::Title } else { Role::Muted });
        if on {
            g.panels.push((y, x..end));
        }
        x = end + 1;
    }
    y += 1;
    if let Some((text, bad)) = &page.note {
        g.put(y, left, &fit(text, width), if *bad { Role::Bad } else { Role::Good });
        y += 1;
    }
    g.rule(y, left..left + width);
    y += 1;
    let top = y;

    let list_w = 64.min(width * 9 / 20).max(30);
    let selected = shown.get(page.cursor).copied();
    let mut focus_line = top;
    let mut group = "";
    for &i in &shown {
        let row = &page.rows[i];
        let this = if row.project_file.is_some() {
            "Project files"
        } else if row.pinned {
            "Pinned"
        } else {
            let days = row.modified.map(|m| (now.date_naive() - chrono::DateTime::<chrono::Local>::from(m).date_naive()).num_days()).unwrap_or(99);
            if days < 7 {
                "This week"
            } else {
                "Older"
            }
        };
        if this != group {
            if y > top {
                y += 1;
            }
            g.heading(y, left, list_w, this);
            y += 1;
            group = this;
        }
        let (tag, tag_role) = kind_tag(row);
        let mut x = g.put(y, left + 1, tag, tag_role) + 2;
        if row.problem.is_some() {
            x = g.put(y, x - 1, "!", Role::Bad) + 1;
        }
        let when = age(row.modified, now);
        let right = (left + list_w).saturating_sub(when.chars().count());
        let tags: String = row.tags.iter().take(3).map(|t| format!("#{t}")).collect::<Vec<_>>().join(" ");
        let name_room = right.saturating_sub(x + 2 + if tags.is_empty() { 0 } else { tags.chars().count().min(18) + 1 });
        let is_sel = selected == Some(i);
        let name_end = g.put(y, x, &fit(&row.name, name_room.max(8)), if is_sel { Role::Title } else { Role::Text });
        if !tags.is_empty() && name_end + 2 < right {
            g.put(y, name_end + 1, &fit(&tags, right.saturating_sub(name_end + 2)), Role::Syntax("text.reference"));
        }
        g.put(y, right, &when, Role::Muted);
        if is_sel {
            g.focus(y, left..left + list_w);
            focus_line = y;
        }
        y += 1;
    }
    if shown.is_empty() {
        let text = if page.rows.is_empty() { "Nothing here yet -- N makes a note, d a diagram, SPC n j opens today's journal." } else { "Nothing matches -- Esc clears the filter." };
        g.put(top, left, &fit(text, list_w), Role::Muted);
    }

    // The selected one, on the right.
    let dx = left + list_w + 3;
    let dw = width.saturating_sub(list_w + 3);
    if let (Some(row), true) = (page.selected(), dw >= 24) {
        let mut y = top;
        let (tag, _) = kind_tag(row);
        let kind = match (row.project_file.is_some(), row.kind) {
            (true, _) => "project file",
            (_, Kind::Note) => "note",
            (_, Kind::Diagram) => "diagram",
            (_, Kind::Day) => "journal day",
        };
        let _ = tag;
        let mut facts: Vec<(&str, String)> = vec![("", kind.to_string())];
        if !row.tags.is_empty() {
            facts.push(("tags", row.tags.iter().map(|t| format!("#{t}")).collect::<Vec<_>>().join(" ")));
        }
        if let Some(a) = &row.about {
            facts.push(("about", a.clone()));
        }
        let mut changed = format!("changed {}", age(row.modified, now));
        if row.versions > 0 {
            changed.push_str(&format!(" · {} earlier version{}", row.versions, if row.versions == 1 { "" } else { "s" }));
        }
        facts.push(("", changed));
        if row.open_tasks > 0 {
            facts.push(("", format!("{} open checkbox{}", row.open_tasks, if row.open_tasks == 1 { "" } else { "es" })));
        }
        if let Some(e) = row.exports.first() {
            facts.push(("exported", e.clone()));
        }
        if let Some(p) = &row.problem {
            facts.push(("problem", p.clone()));
        }
        for (label, value) in facts {
            let x = if label.is_empty() { dx } else { g.put(y, dx, label, Role::Muted) + 1 };
            g.put(y, x, &fit(&value, dw.saturating_sub(x - dx)), if label == "problem" { Role::Bad } else { Role::Muted });
            y += 1;
        }
        g.rule(y, dx..dx + dw);
        y += 1;
        let preview = match row.kind {
            Kind::Diagram => format!("```mermaid\n{}\n```\n", row.text.trim_end()),
            _ => row.text.clone(),
        };
        let rendered = reading::layout(&preview, dw, ctx);
        // As much as fits beside the list, and a little more.
        let max_lines = (y.max(focus_line + 1) - top + 40).min(60);
        let mut clipped = rendered.page.clone();
        let keep: Vec<&str> = clipped.text.lines().take(max_lines).collect();
        clipped.text = keep.join("\n");
        clipped.spans.retain(|s| s.line < max_lines);
        clipped.rules.retain(|(l, _)| *l < max_lines);
        clipped.panels.retain(|(l, _)| *l < max_lines);
        clipped.images.retain(|i| i.line + i.rows <= max_lines);
        clipped.focus = None;
        clipped.popup = None;
        g.embed(&clipped, y, dx, false);
    }

    match &page.mode {
        Mode::Rename { text, caret } => {
            let rows = vec![
                vec![("Rename".to_string(), Role::Title)],
                Vec::new(),
                vec![(format!("{:<48}", with_caret(text, *caret, 48)), Role::Title)],
                Vec::new(),
                vec![("Enter renames it and every link to it · Esc cancels".into(), Role::Muted)],
            ];
            g.popup = Some(Popup { line: focus_line, col: left + 2, rows });
        }
        Mode::Tags { text, caret } => {
            let known: String = page.tags.iter().take(8).map(|t| format!("#{t}")).collect::<Vec<_>>().join(" ");
            let rows = vec![
                vec![("Tags".to_string(), Role::Title)],
                Vec::new(),
                vec![(format!("{:<48}", with_caret(text, *caret, 48)), Role::Title)],
                Vec::new(),
                vec![(fit(&format!("known: {known}"), 60), Role::Muted)],
                vec![("Space between tags · Tab completes · Enter saves · Esc cancels".into(), Role::Muted)],
            ];
            g.popup = Some(Popup { line: focus_line, col: left + 2, rows });
        }
        Mode::History { versions, cursor, .. } => {
            let mut rows = vec![vec![("Earlier versions".to_string(), Role::Title)], Vec::new()];
            for (i, (when, _, first)) in versions.iter().enumerate().take(20) {
                let on = i == *cursor;
                rows.push(vec![(if on { "› ".into() } else { "  ".into() }, Role::Accent), (format!("{when}  "), if on { Role::Title } else { Role::Text }), (fit(first, 40), Role::Muted)]);
            }
            rows.push(Vec::new());
            rows.push(vec![("Enter puts it back (the current one is kept too) · Esc closes".into(), Role::Muted)]);
            g.popup = Some(Popup { line: focus_line, col: left + 2, rows });
        }
        Mode::List => {}
    }

    let keys: Vec<(&str, &str)> = if page.filter.typing {
        vec![("Enter", "done"), ("Esc", "clear")]
    } else {
        match &page.mode {
            Mode::List => vec![
                ("Enter", "edit"),
                ("o", "read"),
                ("v", "view"),
                ("N", "note"),
                ("d", "diagram"),
                ("e", "export"),
                ("t", "tags"),
                ("r", "rename"),
                ("P", "pin"),
                ("h", "history"),
                ("D", "duplicate"),
                ("p", "to project"),
                ("x", "delete"),
                ("[ ]", "days"),
                ("Tab", "chips"),
                ("/", "filter"),
                ("s", "pin search"),
                ("i", "import"),
                ("q", "close"),
            ],
            _ => vec![("Enter", "ok"), ("Esc", "cancel")],
        }
    };
    g.keys(left, width, &keys);
    g.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: &str, kind: Kind, name: &str, tags: &[&str], mins_ago: u64) -> Row {
        Row {
            id: id.into(),
            kind,
            name: name.into(),
            tags: tags.iter().map(|t| t.to_string()).collect(),
            about: None,
            pinned: false,
            modified: Some(SystemTime::now() - std::time::Duration::from_secs(mins_ago * 60)),
            date: None,
            text: format!("# {name}\n\nbody about {name}\n"),
            open_tasks: 0,
            versions: 0,
            exports: Vec::new(),
            problem: None,
            project_file: None,
        }
    }

    fn page() -> NotebookPage {
        let mut pinned = row("c", Kind::Note, "ADR 007", &["decision"], 60 * 24 * 30);
        pinned.pinned = true;
        let rows = vec![row("a", Kind::Note, "Bench session", &["bench"], 5), row("b", Kind::Diagram, "TC flow", &[], 60), pinned];
        NotebookPage::new(rows, vec!["bench".into(), "decision".into()], Vec::new(), None, "C:/nb".into())
    }

    fn text(p: &NotebookPage) -> String {
        layout(p, 140, &reading::Ctx::plain()).all_text()
    }

    #[test]
    fn pinned_first_then_newest_and_the_selection_rendered() {
        let p = page();
        let t = text(&p);
        let pinned = t.find("ADR 007").unwrap();
        let bench = t.find("Bench session").unwrap();
        let flow = t.find("TC flow").unwrap();
        assert!(pinned < bench && bench < flow, "{t}");
        assert!(t.contains("PINNED") && t.contains("THIS WEEK"));
        // The selected (pinned) note is rendered on the right.
        assert!(t.contains("ADR 007") && t.contains("body about ADR 007"));
        assert_eq!(p.selected().unwrap().id, "c");
    }

    #[test]
    fn filtering_and_chips() {
        let mut p = page();
        p.key(Key::Char('/'));
        for c in "tag:bench".chars() {
            p.key(Key::Char(c));
        }
        p.key(Key::Enter);
        assert_eq!(p.visible().len(), 1);
        assert_eq!(p.selected().unwrap().id, "a");
        p.key(Key::Escape);
        assert_eq!(p.visible().len(), 3);
        p.key(Key::Tab);
        p.key(Key::Tab);
        assert_eq!(p.chips()[p.chip], Chip::Diagrams);
        assert_eq!(p.selected().unwrap().id, "b");
    }

    #[test]
    fn rename_tags_pin_and_delete() {
        let mut p = page();
        p.key(Key::Char('j'));
        assert_eq!(p.selected().unwrap().id, "a");
        p.key(Key::Char('r'));
        assert!(p.typing());
        for _ in 0..7 {
            p.key(Key::Backspace);
        }
        for c in "log".chars() {
            p.key(Key::Char(c));
        }
        assert_eq!(p.key(Key::Enter), Action::Rename("a".into(), "Bench log".into()));
        p.key(Key::Char('t'));
        p.key(Key::Char('d'));
        p.key(Key::Char('e'));
        p.key(Key::Tab);
        assert_eq!(p.key(Key::Enter), Action::SetTags("a".into(), vec!["bench".into(), "decision".into()]));
        assert_eq!(p.key(Key::Char('P')), Action::Pin("a".into(), true));
        assert_eq!(p.key(Key::Char('x')), Action::None);
        assert_eq!(p.key(Key::Char('x')), Action::Delete("a".into()));
        assert_eq!(p.key(Key::Char('N')), Action::NewNote);
    }

    #[test]
    fn history_lists_versions_and_restores_one() {
        let mut p = page();
        p.show_history("c".into(), vec![("2026-09-29 10:41:07".into(), PathBuf::from("/h/1.md"), "# ADR".into()), ("2026-09-28 09:00:00".into(), PathBuf::from("/h/0.md"), "# old".into())]);
        assert!(text(&p).contains("Earlier versions"));
        p.key(Key::Char('j'));
        assert_eq!(p.key(Key::Enter), Action::Restore("c".into(), PathBuf::from("/h/0.md")));
    }

    #[test]
    fn ages_read_naturally() {
        let now = chrono::Local::now();
        assert_eq!(age(Some(SystemTime::now() - std::time::Duration::from_secs(330)), now), "5 min ago");
        assert_eq!(age(None, now), "");
    }
}
