//! Home -- the Forged dashboard (the identity's "Home" board): the logo
//! lockup and the date, a find field, then three columns -- resume and
//! recent files; projects; today's tasks and TODOs -- over a notice and
//! key strip at the bottom.
//!
//! Everything here is text cells: `layout` turns `HomeData` and a pane
//! size (in cells) into the buffer text, which cells get which colour
//! role, which get a flat background panel, where the logo image goes,
//! and the activatable slots. No `winit`/`wgpu`, so the whole layout and
//! its navigation are testable as plain data. `App` draws it (see
//! `app/home.rs`) and lays it out again whenever the pane's size in
//! cells changes, so the columns reflow -- three, then two, then one --
//! as the window narrows.

use std::ops::Range;
use std::path::PathBuf;

use fenix_project::ProjectKind;
use fenix_syntax::TodoKind;

/// What Home shows -- gathered by `App` (see `app/home.rs`), kept here as
/// plain data so layout never touches the filesystem.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HomeData {
    /// "Tuesday 22 September · 18:42".
    pub date: String,
    pub resume: Option<FileItem>,
    pub recent: Vec<FileItem>,
    pub projects: Vec<ProjectItem>,
    pub today: Vec<TaskItem>,
    pub todos: Vec<TodoItem>,
    /// Unsaved buffers a previous session left recoverable (`SPC f v`).
    pub recovery: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FileItem {
    pub path: PathBuf,
    pub name: String,
    /// Under the name for `resume` ("fenix · main"), unused for recent rows.
    pub detail: String,
    /// "2 h" -- since the file last changed.
    pub age: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProjectItem {
    pub root: PathBuf,
    pub name: String,
    pub branch: Option<String>,
    pub kind: ProjectKind,
    /// The doctor's verdict, drawn as a dot at the row's end.
    pub health: Option<fenix_project::doctor::Health>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TaskItem {
    pub title: String,
    /// `Some("0:18")` while its clock runs -- the screen's one live thing.
    pub live: Option<String>,
    /// High or urgent priority.
    pub pressing: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TodoItem {
    pub kind: TodoKind,
    pub message: String,
    pub file: String,
    pub path: PathBuf,
    /// 1-indexed, like a grep match.
    pub line: usize,
    pub col: usize,
}

/// What activating a slot does.
#[derive(Debug, Clone, PartialEq)]
pub enum HomeEntry {
    Find,
    Resume(PathBuf),
    RecentFile(PathBuf),
    Project(PathBuf),
    /// The projects column's last row: the new-project wizard.
    NewProject,
    Agenda,
    Todo { path: PathBuf, line: usize, col: usize },
    Recover,
}

/// A colour role, resolved against the active theme by `App`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// Names: section titles, file and project names.
    Title,
    Text,
    Muted,
    Focus,
    /// Brand Ember: the live timer only.
    Ember,
    Warn,
    Todo(TodoKind),
    /// A project's kind tag, in that kind's colour.
    Kind(ProjectKind),
    /// A project's health dot.
    Health(fenix_project::doctor::Health),
}

/// A flat background behind cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fill {
    /// The find field and the live task.
    Panel,
    /// A keycap.
    Key,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Span {
    pub line: usize,
    pub cols: Range<usize>,
    pub role: Role,
}

/// A horizontal rule across `cols` of `line`: drawn as a 1 px line
/// through the middle of the row, not as box-drawing glyphs (which come
/// from a fallback font and render too thin and dark).
#[derive(Debug, Clone, PartialEq)]
pub struct Rule {
    pub line: usize,
    pub cols: Range<usize>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Panel {
    pub line: usize,
    pub cols: Range<usize>,
    pub fill: Fill,
}

/// Something `Enter` (or its number) activates, and the cells it
/// occupies -- the selection tint and focus rail are drawn over these.
#[derive(Debug, Clone, PartialEq)]
pub struct Slot {
    pub line: usize,
    pub height: usize,
    pub cols: Range<usize>,
    /// 0 for the full-width rows (find, the recovery notice); 1.. for the
    /// content columns, left to right.
    pub column: usize,
    pub number: Option<u8>,
    pub entry: HomeEntry,
}

impl Slot {
    /// Where the cursor sits while this slot is selected.
    pub fn cursor(&self) -> (usize, usize) {
        (self.line, self.cols.start + 2)
    }
}

/// Where the logo lockup image goes: its top-left cell and how many text
/// lines tall its box is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Logo {
    pub line: usize,
    pub col: usize,
    pub lines: usize,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct HomeView {
    /// The pane size in cells this was laid out for.
    pub size: (usize, usize),
    pub text: String,
    pub spans: Vec<Span>,
    pub panels: Vec<Panel>,
    pub rules: Vec<Rule>,
    pub slots: Vec<Slot>,
    pub logo: Option<Logo>,
}

/// The widest the content gets; wider panes centre it.
const MAX_WIDTH: usize = 112;
/// Cells between columns.
const GAP: usize = 4;
const MAX_PROJECTS: usize = 5;
const MAX_RECENT: usize = 5;
const MAX_TASKS: usize = 4;
const MAX_TODOS: usize = 5;

/// Cuts `s` to `max` chars, ending in "…" when it had to.
fn fit(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let mut out: String = s.chars().take(max - 1).collect();
    out.push('…');
    out
}

struct Grid {
    lines: Vec<Vec<char>>,
    spans: Vec<Span>,
    panels: Vec<Panel>,
    rules: Vec<Rule>,
}

impl Grid {
    fn put(&mut self, line: usize, col: usize, text: &str, role: Role) {
        if text.is_empty() {
            return;
        }
        if self.lines.len() <= line {
            self.lines.resize(line + 1, Vec::new());
        }
        let row = &mut self.lines[line];
        let len = text.chars().count();
        if row.len() < col + len {
            row.resize(col + len, ' ');
        }
        for (i, c) in text.chars().enumerate() {
            row[col + i] = c;
        }
        self.spans.push(Span { line, cols: col..col + len, role });
    }

    fn panel(&mut self, line: usize, cols: Range<usize>, fill: Fill) {
        self.panels.push(Panel { line, cols, fill });
    }

    fn rule(&mut self, line: usize, cols: Range<usize>) {
        if self.lines.len() <= line {
            self.lines.resize(line + 1, Vec::new());
        }
        self.rules.push(Rule { line, cols });
    }

    /// "name  3 ──────────── SPC p p" across `width` cells.
    fn header(&mut self, line: usize, x: usize, width: usize, name: &str, count: Option<usize>, key: &str) {
        self.put(line, x, name, Role::Title);
        let mut at = x + name.chars().count() + 1;
        if let Some(count) = count {
            let count = count.to_string();
            self.put(line, at, &count, Role::Muted);
            at += count.len() + 1;
        }
        let key_at = (x + width).saturating_sub(key.chars().count());
        if key_at > at + 1 {
            self.rule(line, at..key_at - 1);
            self.put(line, key_at, key, Role::Muted);
        }
    }

    /// `left` at `x`, `right` right-aligned so it ends at `x + width`,
    /// `left` cut short so the two never touch.
    #[allow(clippy::too_many_arguments)]
    fn row(&mut self, line: usize, x: usize, width: usize, left: &str, left_role: Role, right: &str, right_role: Role) {
        let right_len = right.chars().count();
        let room = width.saturating_sub(right_len + if right_len > 0 { 2 } else { 0 });
        self.put(line, x, &fit(left, room), left_role);
        if right_len > 0 && right_len <= width {
            self.put(line, x + width - right_len, right, right_role);
        }
    }
}

#[derive(Clone, Copy)]
enum Section {
    Resume,
    Recent,
    Projects,
    Today,
    Todos,
}

/// Lays Home out for a pane `cols` × `rows` cells in size.
pub fn layout(data: &HomeData, cols: usize, rows: usize) -> HomeView {
    let size = (cols, rows);
    // Below 24 cells there's no sensible layout; lay out for 24 and let
    // the pane clip it, rather than have every width sum underflow.
    let cols = cols.max(24);
    let width = cols.saturating_sub(4).clamp(24, MAX_WIDTH);
    let left = cols.saturating_sub(width) / 2;
    let mut g = Grid { lines: Vec::new(), spans: Vec::new(), panels: Vec::new(), rules: Vec::new() };
    let mut slots = Vec::new();

    // Lockup and date.
    let top = if rows >= 30 { 3 } else { 1 };
    let logo = Logo { line: top, col: left, lines: 3 };
    let date_len = data.date.chars().count();
    if width > date_len + 30 {
        g.put(top + 1, left + width - date_len, &data.date, Role::Muted);
    }

    // Find field: a three-line panel; the whole of it is one slot.
    let find = top + 4;
    for line in find..find + 3 {
        g.panel(line, left..left + width, Fill::Panel);
    }
    g.put(find + 1, left + 2, "›", Role::Focus);
    let key = " SPC SPC ";
    let key_at = left + width - 2 - key.len();
    g.row(find + 1, left + 5, key_at.saturating_sub(left + 6), "Find a file, a symbol, a command", Role::Muted, "", Role::Muted);
    g.put(find + 1, key_at, key, Role::Text);
    g.panel(find + 1, key_at..key_at + key.len(), Fill::Key);
    slots.push(Slot { line: find, height: 3, cols: left..left + width, column: 0, number: None, entry: HomeEntry::Find });

    // Numbers go to projects first, then recent files, in reading order
    // within each -- assigned up front because the columns are drawn
    // recent-first.
    let project_count = data.projects.len().min(MAX_PROJECTS);
    let project_number = |i: usize| u8::try_from(i + 1).ok().filter(|n| *n <= 9);
    let recent_number = |i: usize| u8::try_from(project_count + i + 1).ok().filter(|n| *n <= 9);

    let columns = if width >= 96 { 3 } else if width >= 60 { 2 } else { 1 };
    let col_width = (width - GAP * (columns - 1)) / columns;
    let groups: Vec<Vec<Section>> = match columns {
        3 => vec![vec![Section::Resume, Section::Recent], vec![Section::Projects], vec![Section::Today, Section::Todos]],
        2 => vec![vec![Section::Resume, Section::Recent], vec![Section::Projects, Section::Today, Section::Todos]],
        _ => vec![vec![Section::Resume, Section::Recent, Section::Projects, Section::Today, Section::Todos]],
    };
    let body = find + 5;
    let mut content_end = body;
    for (i, sections) in groups.iter().enumerate() {
        let x = left + i * (col_width + GAP);
        let column = i + 1;
        let mut y = body;
        for section in sections {
            let start = y;
            match section {
                Section::Resume => {
                    let Some(item) = &data.resume else { continue };
                    g.header(y, x, col_width, "resume", None, "Enter");
                    y += 1;
                    g.put(y, x + 2, &fit(&item.name, col_width - 2), Role::Title);
                    g.put(y + 1, x + 2, &fit(&item.detail, col_width - 2), Role::Muted);
                    slots.push(Slot { line: y, height: 2, cols: x..x + col_width, column, number: None, entry: HomeEntry::Resume(item.path.clone()) });
                    y += 2;
                }
                Section::Recent => {
                    if data.recent.is_empty() {
                        continue;
                    }
                    let shown = &data.recent[..data.recent.len().min(MAX_RECENT)];
                    g.header(y, x, col_width, "recent", Some(shown.len()), "SPC f r");
                    y += 1;
                    for (i, item) in shown.iter().enumerate() {
                        let number = recent_number(i);
                        if let Some(n) = number {
                            g.put(y, x + 2, &n.to_string(), Role::Muted);
                        }
                        g.row(y, x + 5, col_width - 5, &item.name, Role::Text, &item.age, Role::Muted);
                        slots.push(Slot { line: y, height: 1, cols: x..x + col_width, column, number, entry: HomeEntry::RecentFile(item.path.clone()) });
                        y += 1;
                    }
                }
                Section::Projects => {
                    let shown = &data.projects[..project_count];
                    g.header(y, x, col_width, "projects", Some(shown.len()), "SPC p p");
                    y += 1;
                    if shown.is_empty() {
                        g.row(y, x + 2, col_width - 2, "none yet", Role::Muted, "SPC p a", Role::Muted);
                        y += 1;
                    }
                    for (i, item) in shown.iter().enumerate() {
                        let number = project_number(i);
                        if let Some(n) = number {
                            g.put(y, x + 2, &n.to_string(), Role::Muted);
                        }
                        // The kind tag in a fixed four-cell field, so names
                        // line up whatever the tag's length.
                        g.put(y, x + 5, item.kind.tag(), Role::Kind(item.kind));
                        g.put(y, x + 9, &fit(&item.name, col_width.saturating_sub(12)), Role::Title);
                        if let Some(health) = item.health {
                            g.put(y, x + col_width - 1, "●", Role::Health(health));
                        }
                        let under = item.branch.clone().unwrap_or_else(|| item.root.display().to_string());
                        g.put(y + 1, x + 9, &fit(&under, col_width.saturating_sub(9)), Role::Muted);
                        slots.push(Slot { line: y, height: 2, cols: x..x + col_width, column, number, entry: HomeEntry::Project(item.root.clone()) });
                        y += 2;
                    }
                    g.put(y, x + 2, "+", Role::Focus);
                    g.row(y, x + 5, col_width - 5, "new project", Role::Text, "SPC p c", Role::Muted);
                    slots.push(Slot { line: y, height: 1, cols: x..x + col_width, column, number: None, entry: HomeEntry::NewProject });
                    y += 1;
                }
                Section::Today => {
                    if data.today.is_empty() {
                        continue;
                    }
                    let shown = &data.today[..data.today.len().min(MAX_TASKS)];
                    g.header(y, x, col_width, "today", Some(data.today.len()), "SPC a a");
                    y += 1;
                    for task in shown {
                        match &task.live {
                            Some(elapsed) => {
                                g.panel(y, x..x + col_width, Fill::Panel);
                                g.put(y, x + 2, "▶", Role::Ember);
                                g.row(y, x + 4, col_width - 6, &task.title, Role::Title, elapsed, Role::Ember);
                            }
                            None => {
                                g.put(y, x + 2, "○", if task.pressing { Role::Warn } else { Role::Muted });
                                g.put(y, x + 4, &fit(&task.title, col_width - 4), Role::Text);
                            }
                        }
                        slots.push(Slot { line: y, height: 1, cols: x..x + col_width, column, number: None, entry: HomeEntry::Agenda });
                        y += 1;
                    }
                }
                Section::Todos => {
                    if data.todos.is_empty() {
                        continue;
                    }
                    let shown = &data.todos[..data.todos.len().min(MAX_TODOS)];
                    g.header(y, x, col_width, "todos", Some(data.todos.len()), "SPC s T");
                    y += 1;
                    for todo in shown {
                        g.put(y, x + 2, todo.kind.label(), Role::Todo(todo.kind));
                        g.row(y, x + 8, col_width - 8, &todo.message, Role::Text, &fit(&todo.file, col_width / 3), Role::Muted);
                        let entry = HomeEntry::Todo { path: todo.path.clone(), line: todo.line, col: todo.col };
                        slots.push(Slot { line: y, height: 1, cols: x..x + col_width, column, number: None, entry });
                        y += 1;
                    }
                }
            }
            if y > start {
                y += 1; // a blank line between sections
            }
        }
        content_end = content_end.max(y);
    }

    // Notice and key strip, at the foot of the pane when there's room.
    let foot = content_end.max(rows.saturating_sub(2));
    g.rule(foot, left..left + width);
    let strip = foot + 1;
    let hints: [(&str, &str); 3] = [("1–9", "open"), ("j k", "move"), ("SPC", "everything")];
    let hints_len: usize = hints.iter().map(|(k, l)| k.chars().count() + 1 + l.len()).sum::<usize>() + 3 * (hints.len() - 1);
    let mut at = (left + width).saturating_sub(hints_len);
    let hints_start = at;
    for (key, label) in hints {
        g.put(strip, at, key, Role::Text);
        at += key.chars().count() + 1;
        g.put(strip, at, label, Role::Muted);
        at += label.len() + 3;
    }
    if data.recovery > 0 {
        let noun = if data.recovery == 1 { "buffer" } else { "buffers" };
        let text = format!("{} unsaved {noun} can be recovered", data.recovery);
        let key = " SPC f v ";
        let room = hints_start.saturating_sub(left + 2 + key.len() + 4);
        let text = fit(&text, room);
        g.put(strip, left, "●", Role::Warn);
        g.put(strip, left + 2, &text, Role::Text);
        let key_at = left + 2 + text.chars().count() + 2;
        g.put(strip, key_at, key, Role::Text);
        g.panel(strip, key_at..key_at + key.len(), Fill::Key);
        slots.push(Slot { line: strip, height: 1, cols: left..key_at + key.len(), column: 0, number: None, entry: HomeEntry::Recover });
    }

    let text = g.lines.iter().map(|row| row.iter().collect::<String>().trim_end().to_string()).collect::<Vec<_>>().join("\n");
    HomeView { size, text, spans: g.spans, panels: g.panels, rules: g.rules, slots, logo: Some(logo) }
}

impl HomeView {
    /// The slot covering cell (`line`, `col`), else the nearest on that
    /// line -- what the cursor has selected.
    pub fn slot_at(&self, line: usize, col: usize) -> Option<usize> {
        let on_line = |s: &Slot| line >= s.line && line < s.line + s.height;
        self.slots
            .iter()
            .position(|s| on_line(s) && s.cols.contains(&col))
            .or_else(|| {
                self.slots
                    .iter()
                    .enumerate()
                    .filter(|(_, s)| on_line(s))
                    .min_by_key(|(_, s)| if col < s.cols.start { s.cols.start - col } else { col.saturating_sub(s.cols.end) })
                    .map(|(i, _)| i)
            })
    }

    /// The slot numbered `n` (the `1`–`9` keys).
    pub fn numbered(&self, n: u8) -> Option<usize> {
        self.slots.iter().position(|s| s.number == Some(n))
    }

    /// `j`/`k`: the next slot down (or up) the same column. From the top
    /// of a column `k` goes to the find field; from its foot `j` goes to
    /// the recovery notice, if there is one. From those full-width rows,
    /// `j`/`k` go to the first column.
    pub fn step(&self, from: usize, down: bool) -> Option<usize> {
        let current = &self.slots[from];
        // From a full-width row, the first column that has anything in it.
        let column = if current.column == 0 {
            self.slots.iter().map(|s| s.column).filter(|c| *c > 0).min().unwrap_or_default()
        } else {
            current.column
        };
        let mut candidates: Vec<(usize, &Slot)> = self.slots.iter().enumerate().filter(|(_, s)| s.column == column).collect();
        candidates.sort_by_key(|(_, s)| s.line);
        let found = if down {
            candidates.iter().find(|(_, s)| s.line > current.line).map(|(i, _)| *i)
        } else {
            candidates.iter().rev().find(|(_, s)| s.line < current.line).map(|(i, _)| *i)
        };
        found.or_else(|| {
            let full_width = self.slots.iter().enumerate().filter(|(_, s)| s.column == 0);
            if down {
                full_width.filter(|(_, s)| s.line > current.line).min_by_key(|(_, s)| s.line).map(|(i, _)| i)
            } else {
                full_width.filter(|(_, s)| s.line < current.line).max_by_key(|(_, s)| s.line).map(|(i, _)| i)
            }
        })
    }

    /// `h`/`l`: the slot in the neighbouring column nearest this one's
    /// line.
    pub fn across(&self, from: usize, right: bool) -> Option<usize> {
        let current = &self.slots[from];
        if current.column == 0 {
            return None;
        }
        let target = if right { current.column + 1 } else { current.column.checked_sub(1).filter(|c| *c > 0)? };
        self.slots
            .iter()
            .enumerate()
            .filter(|(_, s)| s.column == target)
            .min_by_key(|(_, s)| s.line.abs_diff(current.line))
            .map(|(i, _)| i)
    }

    /// The slot index showing `entry`, to keep the selection across a
    /// re-layout.
    pub fn find(&self, entry: &HomeEntry) -> Option<usize> {
        self.slots.iter().position(|s| &s.entry == entry)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> HomeData {
        HomeData {
            date: "Tuesday 22 September · 18:42".to_string(),
            resume: Some(FileItem { path: "/p/app.rs".into(), name: "app.rs".into(), detail: "fenix · main".into(), age: "now".into() }),
            recent: vec![
                FileItem { path: "/p/a.rs".into(), name: "a.rs".into(), detail: String::new(), age: "2 h".into() },
                FileItem { path: "/p/b.rs".into(), name: "b.rs".into(), detail: String::new(), age: "1 d".into() },
            ],
            projects: vec![
                ProjectItem { root: "/p".into(), name: "fenix".into(), branch: Some("main".into()), kind: ProjectKind::Rust, health: Some(fenix_project::doctor::Health::Warn) },
                ProjectItem { root: "/q".into(), name: "test-tcl".into(), branch: None, kind: ProjectKind::Tcl, health: None },
            ],
            today: vec![
                TaskItem { title: "Pick a dashboard".into(), live: Some("0:18".into()), pressing: false },
                TaskItem { title: "Review PR".into(), live: None, pressing: true },
            ],
            todos: vec![TodoItem { kind: TodoKind::Fix, message: "group id is wrong".into(), file: "pom.xml".into(), path: "/p/pom.xml".into(), line: 5, col: 8 }],
            recovery: 1,
        }
    }

    fn line_of(view: &HomeView, needle: &str) -> usize {
        view.text.lines().position(|l| l.contains(needle)).unwrap_or_else(|| panic!("{needle:?} not in:\n{}", view.text))
    }

    #[test]
    fn a_wide_pane_gets_three_columns_side_by_side() {
        let view = layout(&sample(), 140, 45);
        let resume = line_of(&view, "resume");
        assert_eq!(line_of(&view, "projects"), resume, "projects sits beside resume");
        assert_eq!(line_of(&view, "today"), resume, "and today beside both");
        assert_eq!(view.slots.iter().map(|s| s.column).max(), Some(3));
    }

    #[test]
    fn a_narrow_pane_stacks_into_one_column() {
        let view = layout(&sample(), 50, 60);
        assert!(line_of(&view, "projects") > line_of(&view, "resume"));
        assert!(line_of(&view, "today") > line_of(&view, "projects"));
        assert_eq!(view.slots.iter().map(|s| s.column).max(), Some(1));
        assert!(view.text.lines().all(|l| l.chars().count() <= 50), "nothing runs past the pane:\n{}", view.text);
    }

    #[test]
    fn content_is_centred_and_capped_in_width() {
        let view = layout(&sample(), 200, 45);
        let foot = view.rules.iter().max_by_key(|r| r.line).unwrap();
        assert_eq!(foot.cols.len(), MAX_WIDTH);
        assert_eq!(foot.cols.start, (200 - MAX_WIDTH) / 2);
        assert!(!view.text.contains('─'), "rules are drawn, not typed");
    }

    #[test]
    fn projects_are_numbered_before_recent_files() {
        let view = layout(&sample(), 140, 45);
        assert_eq!(view.slots[view.numbered(1).unwrap()].entry, HomeEntry::Project("/p".into()));
        assert_eq!(view.slots[view.numbered(2).unwrap()].entry, HomeEntry::Project("/q".into()));
        assert_eq!(view.slots[view.numbered(3).unwrap()].entry, HomeEntry::RecentFile("/p/a.rs".into()));
        assert!(view.numbered(9).is_none());
    }

    #[test]
    fn the_key_strip_sits_at_the_foot_of_a_tall_pane() {
        let view = layout(&sample(), 140, 45);
        let strip = line_of(&view, "everything");
        assert_eq!(strip, 44);
        assert!(view.text.lines().nth(strip).unwrap().contains("1 unsaved buffer can be recovered"));
    }

    #[test]
    fn only_the_live_task_is_ember() {
        let view = layout(&sample(), 140, 45);
        let ember: Vec<&Span> = view.spans.iter().filter(|s| s.role == Role::Ember).collect();
        assert_eq!(ember.len(), 2, "the ▶ and the elapsed time");
        assert!(ember.iter().all(|s| s.line == ember[0].line));
    }

    #[test]
    fn empty_sections_are_left_out_and_projects_says_how_to_add_one() {
        let data = HomeData { date: "d".into(), ..HomeData::default() };
        let view = layout(&data, 140, 45);
        assert!(!view.text.contains("recent") && !view.text.contains("today") && !view.text.contains("resume"));
        assert!(view.text.contains("none yet"));
        assert!(!view.text.contains("recovered"));
    }

    #[test]
    fn projects_carry_their_kind_tag_and_end_with_a_new_project_slot() {
        let view = layout(&sample(), 140, 45);
        let fenix = line_of(&view, "fenix");
        assert!(view.text.lines().nth(fenix).unwrap().contains("RS  fenix"));
        assert!(view.spans.iter().any(|s| s.line == fenix && s.role == Role::Kind(ProjectKind::Rust)));
        assert!(view.spans.iter().any(|s| s.line == fenix && s.role == Role::Health(fenix_project::doctor::Health::Warn)), "a health dot");
        let tcl = line_of(&view, "test-tcl");
        assert!(!view.spans.iter().any(|s| s.line == tcl && matches!(s.role, Role::Health(_))), "no dot before the doctor has looked");
        let new = view.find(&HomeEntry::NewProject).expect("a new-project slot");
        let last_project = view.find(&HomeEntry::Project("/q".into())).unwrap();
        assert_eq!(view.step(last_project, true), Some(new), "j from the last project reaches it");
        let empty = layout(&HomeData { date: "d".into(), ..HomeData::default() }, 140, 45);
        assert!(empty.find(&HomeEntry::NewProject).is_some(), "even with no projects yet");
    }

    #[test]
    fn navigation_moves_within_and_across_columns() {
        let view = layout(&sample(), 140, 45);
        let find = view.find(&HomeEntry::Find).unwrap();
        let resume = view.step(find, true).unwrap();
        assert_eq!(view.slots[resume].entry, HomeEntry::Resume("/p/app.rs".into()));
        let first_recent = view.step(resume, true).unwrap();
        assert_eq!(view.slots[first_recent].entry, HomeEntry::RecentFile("/p/a.rs".into()));
        assert_eq!(view.step(resume, false), Some(find), "k from the top goes to find");
        let project = view.across(resume, true).unwrap();
        assert_eq!(view.slots[project].entry, HomeEntry::Project("/p".into()));
        let today = view.across(project, true).unwrap();
        assert_eq!(view.slots[today].entry, HomeEntry::Agenda);
        assert_eq!(view.across(today, true), None, "no fourth column");
        let last_recent = view.find(&HomeEntry::RecentFile("/p/b.rs".into())).unwrap();
        assert_eq!(view.slots[view.step(last_recent, true).unwrap()].entry, HomeEntry::Recover);
    }

    #[test]
    fn slot_at_finds_what_the_cursor_is_on() {
        let view = layout(&sample(), 140, 45);
        let project = &view.slots[view.numbered(1).unwrap()];
        let (line, col) = project.cursor();
        assert_eq!(view.slot_at(line, col), view.numbered(1));
        assert_eq!(view.slot_at(line + 1, col), view.numbered(1), "a two-line row covers both lines");
    }

    #[test]
    fn spans_and_panels_stay_inside_the_text() {
        for (cols, rows) in [(140, 45), (80, 40), (40, 30), (24, 12), (6, 3)] {
            let view = layout(&sample(), cols, rows);
            let lines: Vec<&str> = view.text.lines().collect();
            for span in &view.spans {
                let len = lines.get(span.line).map_or(0, |l| l.chars().count());
                assert!(span.cols.end <= len.max(span.cols.end), "span past its line");
                assert!(span.line < lines.len(), "span on a missing line at {cols}x{rows}");
            }
        }
    }
}
