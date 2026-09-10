//! Renders an `fenix_agenda::AgendaStore` into real buffer text, the same
//! "generated text + parallel per-line metadata" shape `jira_panel`/
//! `docker_panel`/`git_panel` already use. Unlike those, the agenda has no
//! live/async data behind it (it's a local, synchronous JSON file), so it
//! doesn't need their multi-pane session machinery: one `BufferKind::Agenda`
//! buffer, re-rendered in place by whichever `AgendaView` is current --
//! exactly how `BufferKind::Dashboard` and `BufferKind::Explorer` already
//! work.

use fenix_agenda::{AgendaStore, Priority, Status, Subtask, Task, TaskId, TimeSource};

/// Which of the four views is currently rendered into the one agenda
/// buffer -- `App` keeps this alongside the buffer id and re-renders on
/// every mutation and on `SPC a k`/`l`/`r`/`Enter`/`Esc`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgendaView {
    List,
    Board,
    Report,
    Detail(TaskId),
}

/// What a cursor landing on one part of a line means -- `Task` everywhere
/// a row represents a whole task (list rows, board cards, report rows,
/// the "Blocks" section of the detail view); `Dependency` only in the
/// detail view's own "Blocked by" section, where it additionally supports
/// removal (`B`); `Subtask`/`Note`/`TimeEntry` only in the detail view's
/// own checklist/notes-log/time-entry-list sections, each carrying that
/// list's index rather than a `TaskId` (they aren't tasks -- `App::agenda_
/// task_id_at_cursor` explicitly excludes all three).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgendaEntry {
    Task(TaskId),
    Dependency(TaskId),
    Subtask(usize),
    Note(usize),
    TimeEntry(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgendaLineStyle {
    Title,
    SectionHeader,
    ColumnHeader,
    TaskRow,
    Detail,
    Body,
    SubtaskDone,
    SubtaskPending,
    Note,
    Empty,
    Footer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgendaBadgeColor {
    Good,
    Warn,
    Bad,
    Neutral,
}

/// Per-line metadata for one line of `AgendaPanel::text`, at the matching
/// index in `AgendaPanel::lines`. `entries`/`badges` are lists rather than
/// a single `Option` (mirrors `JiraLine`'s single `entry`/`badge`) because
/// the Kanban board packs one card-row from each of its four side-by-side
/// status columns into a single physical text line -- every other view
/// just ever has zero or one entry, starting at column 0.
#[derive(Debug, Clone)]
pub struct AgendaLine {
    pub style: AgendaLineStyle,
    pub entries: Vec<(usize, AgendaEntry)>,
    pub badges: Vec<(usize, usize, AgendaBadgeColor)>,
}

impl AgendaLine {
    fn plain(style: AgendaLineStyle) -> Self {
        Self { style, entries: Vec::new(), badges: Vec::new() }
    }

    fn row(style: AgendaLineStyle, entry: AgendaEntry) -> Self {
        Self { style, entries: vec![(0, entry)], badges: Vec::new() }
    }

    /// What a cursor at `column` (0-based char offset into the line)
    /// resolves to -- the last entry whose start column is `<= column`.
    pub fn entry_at(&self, column: usize) -> Option<AgendaEntry> {
        self.entries.iter().rev().find(|(start, _)| *start <= column).map(|(_, e)| *e)
    }
}

pub struct AgendaPanel {
    pub text: String,
    pub lines: Vec<Option<AgendaLine>>,
}

struct Builder {
    text: String,
    lines: Vec<Option<AgendaLine>>,
}

impl Builder {
    fn new() -> Self {
        Self { text: String::new(), lines: Vec::new() }
    }

    fn push(&mut self, text: &str, meta: Option<AgendaLine>) {
        self.text.push_str(text);
        self.text.push('\n');
        self.lines.push(meta);
    }

    fn push_plain(&mut self, text: &str, style: AgendaLineStyle) {
        self.push(text, Some(AgendaLine::plain(style)));
    }

    fn push_row(&mut self, text: &str, style: AgendaLineStyle, entry: AgendaEntry) {
        self.push(text, Some(AgendaLine::row(style, entry)));
    }

    fn push_blank(&mut self) {
        self.push("", None);
    }

    fn finish(self) -> AgendaPanel {
        AgendaPanel { text: self.text, lines: self.lines }
    }
}

fn empty_line(message: &str) -> (String, Option<AgendaLine>) {
    (format!("    {message}"), Some(AgendaLine::plain(AgendaLineStyle::Empty)))
}

/// A dim one-line key reminder at the bottom of every agenda view --
/// same role `dashboard::push_footer` already plays, since a fully
/// generated buffer has no other place to teach its own keymap. The row
/// actions (`agenda_route_key`) all live outside the leader trie, so
/// `which-key` never surfaces them the way it does `SPC a`'s own
/// children -- this is the only place a first-time user sees them at
/// all.
const ROW_ACTION_HINTS: &str = "Enter detail  ·  s status  ·  p priority  ·  c category  ·  t clock  ·  T log time  ·  N note  ·  e edit  ·  x archive  ·  D delete";

/// `render_detail`'s own footer -- `b`/`a` (add dependency/subtask) are
/// deliberately left out, since their own sections already hint them
/// inline right where they apply ("(none) -- press b to add one"); this
/// covers only the actions detail has no other hint for.
const DETAIL_ACTION_HINTS: &str =
    "Esc back  ·  s status  ·  p priority  ·  c category  ·  t clock  ·  T log time  ·  N note  ·  e edit  ·  D delete";

fn push_footer(b: &mut Builder, hints: &str, extra: Option<&str>) {
    b.push_blank();
    let line = match extra {
        Some(extra) => format!("  {hints}  ·  {extra}"),
        None => format!("  {hints}"),
    };
    b.push_plain(&line, AgendaLineStyle::Footer);
}

fn priority_color(p: Priority) -> AgendaBadgeColor {
    match p {
        Priority::Urgent => AgendaBadgeColor::Bad,
        Priority::High => AgendaBadgeColor::Warn,
        Priority::Medium | Priority::Low => AgendaBadgeColor::Neutral,
    }
}

fn status_color(s: Status) -> AgendaBadgeColor {
    match s {
        Status::Done => AgendaBadgeColor::Good,
        Status::InProgress => AgendaBadgeColor::Warn,
        Status::Blocked => AgendaBadgeColor::Bad,
        Status::Todo => AgendaBadgeColor::Neutral,
    }
}

/// `"1h 45m"`/`"45m"`/`"0m"` -- minute precision is enough for a personal
/// timesheet; seconds would just be noise once a task has been worked on
/// across more than one sitting.
pub fn format_duration(d: chrono::Duration) -> String {
    let total_minutes = d.num_minutes().max(0);
    let hours = total_minutes / 60;
    let minutes = total_minutes % 60;
    if hours > 0 {
        format!("{hours}h {minutes}m")
    } else {
        format!("{minutes}m")
    }
}

fn pad_or_truncate(s: &str, width: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() > width {
        let truncated: String = chars[..width.saturating_sub(1)].iter().collect();
        format!("{truncated}…")
    } else {
        let mut out: String = chars.into_iter().collect();
        out.push_str(&" ".repeat(width - out.chars().count()));
        out
    }
}

/// Tasks in display order within one status: ready tasks (no unresolved
/// dependency) before ones waiting on something, then higher priority
/// first, then manual `order` -- shared by both the list (within each
/// status section) and the board (within each column).
fn sorted_in_status(store: &AgendaStore, status: Status, include_archived: bool) -> Vec<&Task> {
    let mut tasks: Vec<&Task> =
        store.tasks.iter().filter(|t| t.status == status && (include_archived || !t.archived)).collect();
    tasks.sort_by_key(|t| (!store.is_ready(t.id), std::cmp::Reverse(t.priority), t.order));
    tasks
}

fn category_tag(task: &Task) -> String {
    match &task.category {
        Some(c) if !c.is_empty() => format!(" ({c})"),
        _ => String::new(),
    }
}

/// One line per task: `[Priority] Title (Category)  · time  · waiting`.
/// Sections group by status (`Status::ALL` order), each with its own
/// header, mirroring `jira_panel`'s Projects/Users grouping shape.
pub fn render_list(store: &AgendaStore) -> AgendaPanel {
    let mut b = Builder::new();
    if !store.tasks.iter().any(|t| !t.archived) {
        let (text, meta) = empty_line("No tasks yet -- SPC a n to add one");
        b.push(&text, meta);
        return b.finish();
    }
    for status in Status::ALL {
        let tasks = sorted_in_status(store, status, false);
        if tasks.is_empty() {
            continue;
        }
        let header = format!("{} ({})", status.label(), tasks.len());
        b.push_plain(&header, AgendaLineStyle::SectionHeader);
        b.push_plain(&"-".repeat(header.chars().count()), AgendaLineStyle::Detail);
        for task in tasks {
            push_task_row(&mut b, store, task);
        }
        b.push_blank();
    }
    push_footer(&mut b, ROW_ACTION_HINTS, None);
    b.finish()
}

fn push_task_row(b: &mut Builder, store: &AgendaStore, task: &Task) {
    let prefix = format!("  [{}] ", task.priority.label());
    let badge_len = prefix.chars().count();
    let mut line = format!("{prefix}{}{}", task.title, category_tag(task));
    let elapsed = store.elapsed_on(task.id);
    if elapsed > chrono::Duration::zero() {
        line.push_str(&format!("  · {}", format_duration(elapsed)));
    }
    if !store.is_ready(task.id) && task.status != Status::Done {
        line.push_str("  · waiting on a dependency");
    }
    b.push(
        &line,
        Some(AgendaLine {
            style: AgendaLineStyle::TaskRow,
            entries: vec![(0, AgendaEntry::Task(task.id))],
            badges: vec![(0, badge_len, priority_color(task.priority))],
        }),
    );
}

const CARD_WIDTH: usize = 26;
const COLUMN_GAP: &str = "  |  ";

/// Four side-by-side status columns, plain-ASCII-separated (no
/// box-drawing glyphs -- same reasoning `git_graph_style`'s own doc
/// comment gives for defaulting its own graph to ascii: a font missing
/// those glyphs knocks every column out of alignment). Each card is two
/// lines (title + priority badge, then category/waiting marker), with a
/// blank separator line between cards in the same column. Every physical
/// line therefore carries up to four entries, one per column -- see
/// `AgendaLine::entry_at`.
pub fn render_board(store: &AgendaStore) -> AgendaPanel {
    let mut b = Builder::new();
    let columns: Vec<Vec<&Task>> = Status::ALL.iter().map(|&s| sorted_in_status(store, s, false)).collect();

    let header: Vec<String> = Status::ALL
        .iter()
        .zip(&columns)
        .map(|(s, tasks)| pad_or_truncate(&format!("{} ({})", s.label(), tasks.len()), CARD_WIDTH))
        .collect();
    b.push_plain(&header.join(COLUMN_GAP), AgendaLineStyle::ColumnHeader);
    let underline: Vec<String> = (0..columns.len()).map(|_| "-".repeat(CARD_WIDTH)).collect();
    b.push_plain(&underline.join(COLUMN_GAP), AgendaLineStyle::Detail);

    let max_cards = columns.iter().map(Vec::len).max().unwrap_or(0);
    if max_cards == 0 {
        let (text, meta) = empty_line("No tasks yet -- SPC a n to add one");
        b.push(&text, meta);
        return b.finish();
    }

    let column_stride = CARD_WIDTH + COLUMN_GAP.chars().count();
    for row in 0..max_cards {
        let mut line1 = String::new();
        let mut line2 = String::new();
        let mut entries1 = Vec::new();
        let mut badges1 = Vec::new();
        for (col_idx, column) in columns.iter().enumerate() {
            let col_offset = col_idx * column_stride;
            match column.get(row) {
                Some(task) => {
                    let title_cell = pad_or_truncate(&format!("[{}] {}", task.priority.label(), task.title), CARD_WIDTH);
                    let badge_len = format!("[{}]", task.priority.label()).chars().count();
                    let mut sub = category_tag(task).trim_start().to_string();
                    if !store.is_ready(task.id) && task.status != Status::Done {
                        if !sub.is_empty() {
                            sub.push(' ');
                        }
                        sub.push_str("(waiting)");
                    }
                    line1.push_str(&title_cell);
                    line2.push_str(&pad_or_truncate(&sub, CARD_WIDTH));
                    entries1.push((col_offset, AgendaEntry::Task(task.id)));
                    badges1.push((col_offset, badge_len, priority_color(task.priority)));
                }
                None => {
                    line1.push_str(&" ".repeat(CARD_WIDTH));
                    line2.push_str(&" ".repeat(CARD_WIDTH));
                }
            }
            if col_idx + 1 < columns.len() {
                line1.push_str(COLUMN_GAP);
                line2.push_str(COLUMN_GAP);
            }
        }
        // Line 2 shares line 1's entries (same column offsets, same
        // tasks) so clicking/cursoring onto either row of a card resolves
        // to that card's task.
        let entries2 = entries1.clone();
        b.push(&line1, Some(AgendaLine { style: AgendaLineStyle::TaskRow, entries: entries1, badges: badges1 }));
        b.push(&line2, Some(AgendaLine { style: AgendaLineStyle::TaskRow, entries: entries2, badges: Vec::new() }));
        if row + 1 < max_cards {
            b.push_blank();
        }
    }
    push_footer(&mut b, ROW_ACTION_HINTS, Some("H/L move column  ·  J/K reorder"));
    b.finish()
}

/// Time entries grouped by calendar day, then by task within the day, with
/// per-day and per-task-overall totals -- a personal timesheet, not
/// billing-grade (minute precision, local time, no rounding rules).
pub fn render_report(store: &AgendaStore) -> AgendaPanel {
    let mut b = Builder::new();

    let mut by_day: std::collections::BTreeMap<chrono::NaiveDate, Vec<(&Task, chrono::Duration)>> = std::collections::BTreeMap::new();
    for task in &store.tasks {
        for entry in &task.time_entries {
            by_day.entry(entry.start.date_naive()).or_default().push((task, entry.duration()));
        }
    }

    if by_day.is_empty() {
        let (text, meta) = empty_line("No time logged yet");
        b.push(&text, meta);
        return b.finish();
    }

    for (day, entries) in by_day.iter().rev() {
        let header = day.format("%Y-%m-%d (%A)").to_string();
        b.push_plain(&header, AgendaLineStyle::SectionHeader);
        b.push_plain(&"-".repeat(header.chars().count()), AgendaLineStyle::Detail);

        let mut per_task: std::collections::BTreeMap<TaskId, (String, chrono::Duration)> = std::collections::BTreeMap::new();
        for (task, duration) in entries {
            let e = per_task.entry(task.id).or_insert((task.title.clone(), chrono::Duration::zero()));
            e.1 += *duration;
        }
        let mut day_total = chrono::Duration::zero();
        for (id, (title, duration)) in &per_task {
            day_total += *duration;
            let line = format!("  {}  {}", pad_or_truncate(title, 30), format_duration(*duration));
            b.push_row(&line, AgendaLineStyle::TaskRow, AgendaEntry::Task(*id));
        }
        b.push_plain(&format!("  {}  {}", pad_or_truncate("Day total", 30), format_duration(day_total)), AgendaLineStyle::Detail);
        b.push_blank();
    }

    b.push_plain("Total by task", AgendaLineStyle::SectionHeader);
    b.push_plain("-------------", AgendaLineStyle::Detail);
    let mut totals: std::collections::BTreeMap<TaskId, (String, chrono::Duration)> = std::collections::BTreeMap::new();
    for task in &store.tasks {
        let total = task.total_time();
        if total > chrono::Duration::zero() {
            totals.insert(task.id, (task.title.clone(), total));
        }
    }
    let mut grand_total = chrono::Duration::zero();
    for (id, (title, duration)) in &totals {
        grand_total += *duration;
        let line = format!("  {}  {}", pad_or_truncate(title, 30), format_duration(*duration));
        b.push_row(&line, AgendaLineStyle::TaskRow, AgendaEntry::Task(*id));
    }
    b.push_blank();
    b.push_plain(&format!("Grand total: {}", format_duration(grand_total)), AgendaLineStyle::SectionHeader);
    push_footer(&mut b, ROW_ACTION_HINTS, None);

    b.finish()
}

/// `entry`, when given, is attached to every wrapped line (not just the
/// first) so a cursor landing anywhere in a multi-line note's body still
/// resolves to that same note -- e.g. `AgendaEntry::Note(i)` for a note's
/// own body text. `None` for prose nothing else acts on directly (the
/// task description, whose own edit already applies from anywhere on the
/// page -- see `App::agenda_task_id_at_cursor`'s Detail-page fallback).
fn push_wrapped(b: &mut Builder, indent: &str, text: &str, style: AgendaLineStyle, entry: Option<AgendaEntry>) {
    let wrap_width = crate::wrap::DEFAULT_WRAP_WIDTH.saturating_sub(indent.chars().count()).max(20);
    if text.is_empty() {
        b.push_plain(&format!("{indent}(none)"), AgendaLineStyle::Empty);
        return;
    }
    for line in text.lines() {
        for wrapped in crate::wrap::wrap_text(line, wrap_width) {
            let text = format!("{indent}{wrapped}");
            match entry {
                Some(entry) => b.push_row(&text, style, entry),
                None => b.push_plain(&text, style),
            }
        }
    }
}

/// The single task detail pane: title + status badge, priority/category/
/// timestamps, the "Blocked by"/"Blocks" dependency sections, the subtask
/// checklist, the notes log (newest last), and the time-entry list --
/// everything one task can carry, laid out to read top-to-bottom like
/// `jira_panel::render_detail`'s own issue page.
pub fn render_detail(store: &AgendaStore, id: TaskId) -> AgendaPanel {
    let mut b = Builder::new();
    let Some(task) = store.task(id) else {
        let (text, meta) = empty_line("This task no longer exists");
        b.push(&text, meta);
        return b.finish();
    };

    b.push_plain(&task.title, AgendaLineStyle::Title);
    let status_line = format!("[{}]  Priority: {}{}", task.status.label(), task.priority.label(), category_tag(task));
    let badge_len = format!("[{}]", task.status.label()).chars().count();
    b.push(
        &status_line,
        Some(AgendaLine { style: AgendaLineStyle::Detail, entries: Vec::new(), badges: vec![(0, badge_len, status_color(task.status))] }),
    );
    b.push_blank();

    b.push_plain(&format!("Created: {}", task.created_at.format("%Y-%m-%d %H:%M")), AgendaLineStyle::Detail);
    b.push_plain(&format!("Updated: {}", task.updated_at.format("%Y-%m-%d %H:%M")), AgendaLineStyle::Detail);
    let elapsed = store.elapsed_on(id);
    b.push_plain(&format!("Time spent: {}", format_duration(elapsed)), AgendaLineStyle::Detail);
    b.push_blank();

    b.push_plain("Description", AgendaLineStyle::SectionHeader);
    b.push_plain("-----------", AgendaLineStyle::Detail);
    push_wrapped(&mut b, "  ", &task.description, AgendaLineStyle::Body, None);
    b.push_blank();

    let blocked_by = store.blocked_by(id);
    b.push_plain("Blocked by", AgendaLineStyle::SectionHeader);
    b.push_plain("----------", AgendaLineStyle::Detail);
    if blocked_by.is_empty() {
        b.push_plain("  (none) -- press b to add one", AgendaLineStyle::Empty);
    } else {
        for dep in blocked_by {
            let marker = if dep.status == Status::Done { "[done]" } else { "[open]" };
            let line = format!("  {marker} {}", dep.title);
            b.push_row(&line, AgendaLineStyle::TaskRow, AgendaEntry::Dependency(dep.id));
        }
    }
    b.push_blank();

    let blocks = store.blocks(id);
    if !blocks.is_empty() {
        b.push_plain("Blocks", AgendaLineStyle::SectionHeader);
        b.push_plain("------", AgendaLineStyle::Detail);
        for dependent in blocks {
            let line = format!("  {}", dependent.title);
            b.push_row(&line, AgendaLineStyle::TaskRow, AgendaEntry::Task(dependent.id));
        }
        b.push_blank();
    }

    b.push_plain("Subtasks", AgendaLineStyle::SectionHeader);
    b.push_plain("--------", AgendaLineStyle::Detail);
    if task.subtasks.is_empty() {
        b.push_plain("  (none) -- press a to add one", AgendaLineStyle::Empty);
    } else {
        for (i, subtask) in task.subtasks.iter().enumerate() {
            push_subtask_row(&mut b, i, subtask);
        }
    }
    b.push_blank();

    b.push_plain("Notes", AgendaLineStyle::SectionHeader);
    b.push_plain("-----", AgendaLineStyle::Detail);
    if task.notes.is_empty() {
        b.push_plain("  (none) -- press N to add one", AgendaLineStyle::Empty);
    } else {
        for (i, note) in task.notes.iter().enumerate() {
            b.push_row(&format!("  {}", note.at.format("%Y-%m-%d %H:%M")), AgendaLineStyle::Note, AgendaEntry::Note(i));
            push_wrapped(&mut b, "    ", &note.text, AgendaLineStyle::Body, Some(AgendaEntry::Note(i)));
        }
    }
    b.push_blank();

    b.push_plain("Time entries", AgendaLineStyle::SectionHeader);
    b.push_plain("------------", AgendaLineStyle::Detail);
    if task.time_entries.is_empty() {
        b.push_plain("  (none)", AgendaLineStyle::Empty);
    } else {
        for (i, entry) in task.time_entries.iter().enumerate() {
            let source = match entry.source {
                TimeSource::Timer => "timer",
                TimeSource::Manual => "manual",
            };
            let line = format!(
                "  {} - {}  {}  ({source})",
                entry.start.format("%Y-%m-%d %H:%M"),
                entry.end.format("%H:%M"),
                format_duration(entry.duration())
            );
            b.push_row(&line, AgendaLineStyle::Detail, AgendaEntry::TimeEntry(i));
        }
    }
    push_footer(&mut b, DETAIL_ACTION_HINTS, None);
    b.push_plain("  on a note: e edits it, D removes it   ·   on a time entry: D removes it", AgendaLineStyle::Footer);

    b.finish()
}

fn push_subtask_row(b: &mut Builder, index: usize, subtask: &Subtask) {
    let (mark, style) =
        if subtask.done { ("[x]", AgendaLineStyle::SubtaskDone) } else { ("[ ]", AgendaLineStyle::SubtaskPending) };
    let line = format!("  {mark} {}", subtask.text);
    b.push_row(&line, style, AgendaEntry::Subtask(index));
}

#[cfg(test)]
mod tests {
    use super::*;
    use fenix_agenda::AgendaStore;

    fn entries_of(panel: &AgendaPanel) -> Vec<&AgendaLine> {
        panel.lines.iter().flatten().collect()
    }

    #[test]
    fn render_list_empty_store_shows_a_placeholder() {
        let store = AgendaStore::default();
        assert!(render_list(&store).text.contains("No tasks yet"));
    }

    #[test]
    fn render_list_groups_by_status_and_carries_the_task_entry_and_priority_badge() {
        let mut store = AgendaStore::default();
        let id = store.create_task("Write the plan".to_string(), "".to_string(), Priority::High, Some("Fenix".to_string()));
        let panel = render_list(&store);

        assert!(panel.text.contains("Todo (1)"));
        assert!(panel.text.contains("[High] Write the plan (Fenix)"));
        let row = entries_of(&panel).into_iter().find(|l| l.entries.iter().any(|(_, e)| *e == AgendaEntry::Task(id))).unwrap();
        assert_eq!(row.entry_at(0), Some(AgendaEntry::Task(id)));
        assert_eq!(row.badges[0].2, AgendaBadgeColor::Warn);
    }

    #[test]
    fn render_list_marks_a_task_waiting_on_an_unresolved_dependency() {
        let mut store = AgendaStore::default();
        let dep = store.create_task("Dep".to_string(), "".to_string(), Priority::Low, None);
        let task = store.create_task("Task".to_string(), "".to_string(), Priority::Low, None);
        store.add_dependency(task, dep);

        assert!(render_list(&store).text.contains("waiting on a dependency"));
    }

    #[test]
    fn render_list_shows_a_key_hint_footer() {
        let mut store = AgendaStore::default();
        store.create_task("Write the plan".to_string(), "".to_string(), Priority::Low, None);
        let panel = render_list(&store);
        assert!(panel.text.contains("s status"));
        assert!(panel.text.contains("D delete"));
        let footer = entries_of(&panel).into_iter().find(|l| l.style == AgendaLineStyle::Footer);
        assert!(footer.is_some());
    }

    #[test]
    fn render_list_hides_archived_tasks() {
        let mut store = AgendaStore::default();
        let id = store.create_task("Gone".to_string(), "".to_string(), Priority::Low, None);
        store.archive(id);
        assert!(render_list(&store).text.contains("No tasks yet"));
    }

    #[test]
    fn render_board_empty_store_shows_a_placeholder() {
        let store = AgendaStore::default();
        assert!(render_board(&store).text.contains("No tasks yet"));
    }

    #[test]
    fn render_board_places_each_status_in_its_own_column_and_resolves_the_right_task_per_column() {
        let mut store = AgendaStore::default();
        let todo = store.create_task("Todo task".to_string(), "".to_string(), Priority::Low, None);
        let doing = store.create_task("Doing task".to_string(), "".to_string(), Priority::Low, None);
        store.set_status(doing, Status::InProgress);

        let panel = render_board(&store);
        assert!(panel.text.contains("Todo task"));
        assert!(panel.text.contains("Doing task"));

        let card_row = entries_of(&panel).into_iter().find(|l| l.style == AgendaLineStyle::TaskRow).unwrap();
        assert_eq!(card_row.entry_at(0), Some(AgendaEntry::Task(todo)));
        let in_progress_column_start = CARD_WIDTH + COLUMN_GAP.chars().count();
        assert_eq!(card_row.entry_at(in_progress_column_start), Some(AgendaEntry::Task(doing)));
    }

    #[test]
    fn render_board_footer_mentions_the_board_only_move_and_reorder_keys() {
        let mut store = AgendaStore::default();
        store.create_task("Todo task".to_string(), "".to_string(), Priority::Low, None);
        let panel = render_board(&store);
        assert!(panel.text.contains("H/L move column"));
        assert!(panel.text.contains("J/K reorder"));
    }

    #[test]
    fn render_report_shows_no_time_logged_when_the_store_has_no_entries() {
        let store = AgendaStore::default();
        assert!(render_report(&store).text.contains("No time logged yet"));
    }

    #[test]
    fn render_report_totals_manual_time_by_task() {
        let mut store = AgendaStore::default();
        let id = store.create_task("Write the plan".to_string(), "".to_string(), Priority::Low, None);
        store.log_manual_time(id, chrono::Duration::minutes(90));

        let panel = render_report(&store);
        assert!(panel.text.contains("1h 30m"));
        let row = entries_of(&panel).into_iter().find(|l| l.entries.iter().any(|(_, e)| *e == AgendaEntry::Task(id))).unwrap();
        assert_eq!(row.entry_at(0), Some(AgendaEntry::Task(id)));
    }

    #[test]
    fn render_detail_shows_description_notes_subtasks_and_dependencies() {
        let mut store = AgendaStore::default();
        let dep = store.create_task("Dep".to_string(), "".to_string(), Priority::Low, None);
        let id = store.create_task("Task".to_string(), "Full description".to_string(), Priority::Urgent, None);
        store.add_dependency(id, dep);
        store.add_subtask(id, "Draft outline".to_string());
        store.add_note(id, "started working on it".to_string());

        let panel = render_detail(&store, id);
        assert!(panel.text.contains("Full description"));
        assert!(panel.text.contains("Draft outline"));
        assert!(panel.text.contains("started working on it"));
        assert!(panel.text.contains("[open] Dep"));

        let subtask_row = entries_of(&panel).into_iter().find(|l| l.entries.iter().any(|(_, e)| matches!(e, AgendaEntry::Subtask(_)))).unwrap();
        assert_eq!(subtask_row.entry_at(0), Some(AgendaEntry::Subtask(0)));

        let dep_row = entries_of(&panel).into_iter().find(|l| l.entries.iter().any(|(_, e)| *e == AgendaEntry::Dependency(dep))).unwrap();
        assert_eq!(dep_row.entry_at(0), Some(AgendaEntry::Dependency(dep)));
    }

    #[test]
    fn render_detail_note_and_time_entry_rows_carry_their_own_index() {
        let mut store = AgendaStore::default();
        let id = store.create_task("Task".to_string(), "".to_string(), Priority::Low, None);
        store.add_note(id, "first note".to_string());
        store.add_note(
            id,
            "second note, deliberately long enough that it definitely wraps onto more than a single \
             rendered line once the wrap width and the notes section's own indent are both accounted for"
                .to_string(),
        );
        store.log_manual_time(id, chrono::Duration::minutes(30));

        let panel = render_detail(&store, id);

        let note_0_rows: Vec<_> =
            entries_of(&panel).into_iter().filter(|l| l.entries.iter().any(|(_, e)| *e == AgendaEntry::Note(0))).collect();
        assert_eq!(note_0_rows.len(), 2, "a short one-line note carries its entry on its timestamp line and its one body line");

        let note_1_rows: Vec<_> =
            entries_of(&panel).into_iter().filter(|l| l.entries.iter().any(|(_, e)| *e == AgendaEntry::Note(1))).collect();
        assert!(note_1_rows.len() > note_0_rows.len(), "a longer, wrapped note should carry its entry on more rows than a short one");

        let time_row = entries_of(&panel).into_iter().find(|l| l.entries.iter().any(|(_, e)| *e == AgendaEntry::TimeEntry(0))).unwrap();
        assert_eq!(time_row.entry_at(0), Some(AgendaEntry::TimeEntry(0)));
    }

    #[test]
    fn render_detail_footer_explains_note_and_time_entry_actions() {
        let mut store = AgendaStore::default();
        let id = store.create_task("Task".to_string(), "".to_string(), Priority::Low, None);
        let panel = render_detail(&store, id);
        assert!(panel.text.contains("on a note: e edits it, D removes it"));
        assert!(panel.text.contains("on a time entry: D removes it"));
    }

    #[test]
    fn render_detail_footer_mentions_esc_back_but_not_the_already_inline_hinted_dependency_and_subtask_keys() {
        let mut store = AgendaStore::default();
        let id = store.create_task("Task".to_string(), "".to_string(), Priority::Low, None);
        let panel = render_detail(&store, id);
        let footer = entries_of(&panel).into_iter().find(|l| l.style == AgendaLineStyle::Footer).unwrap();
        assert!(footer.entries.is_empty(), "the footer is a hint, not something to land the cursor on");
        assert!(panel.text.contains("Esc back"));
        assert!(panel.text.contains("press b to add one"));
        assert!(panel.text.contains("press a to add one"));
    }

    #[test]
    fn render_detail_on_a_missing_task_shows_a_placeholder_instead_of_panicking() {
        let store = AgendaStore::default();
        let panel = render_detail(&store, TaskId(999));
        assert!(panel.text.contains("no longer exists"));
    }

    #[test]
    fn format_duration_formats_hours_and_minutes() {
        assert_eq!(format_duration(chrono::Duration::minutes(45)), "45m");
        assert_eq!(format_duration(chrono::Duration::minutes(90)), "1h 30m");
        assert_eq!(format_duration(chrono::Duration::minutes(120)), "2h 0m");
    }
}
