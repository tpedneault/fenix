//! Renders an `fenix_agenda::AgendaStore` into real buffer text, the same
//! "generated text + parallel per-line metadata" shape `jira_panel`/
//! `docker_panel`/`git_panel` already use. Unlike those, the agenda has no
//! live/async data behind it (it's a local, synchronous JSON file), so it
//! doesn't need their multi-pane session machinery: one `BufferKind::Agenda`
//! buffer, re-rendered in place by whichever `AgendaView` is current --
//! exactly how `BufferKind::Dashboard` and `BufferKind::Explorer` already
//! work.
//!
//! A task linked to a Jira issue renders like any other task, plus its
//! key, Jira's own status name where it differs from the column, and a
//! sync marker: `↑` while a change is on its way to Jira, `⚠` when one
//! failed or both sides changed the same field.

use fenix_agenda::{AgendaStore, OpKind, Priority, Status, Subtask, SyncField, Task, TaskId, TimeSource, WorklogRow};

/// Which view is currently rendered into the one agenda buffer -- `App`
/// keeps this alongside the buffer id and re-renders on every mutation and
/// on `SPC a k`/`l`/`r`/`w`/`Enter`/`Esc`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgendaView {
    List,
    Board,
    Report,
    /// The worklog review: unsent time on linked tasks, one row per issue
    /// per day, about to be sent to Jira.
    Worklogs,
    Detail(TaskId),
}

/// What a cursor landing on one part of a line means -- `Task` everywhere
/// a row represents a whole task (list rows, board cards, report rows,
/// the "Blocks" section of the detail view); `Dependency` only in the
/// detail view's own "Blocked by" section, where it additionally supports
/// removal (`B`); `Subtask`/`Note`/`TimeEntry`/`Comment` only in the
/// detail view's own checklist/activity/time-entry sections, each carrying
/// that list's index rather than a `TaskId` (they aren't tasks -- `App::
/// agenda_task_id_at_cursor` explicitly excludes them). `Conflict` is a
/// field Jira and you both changed; `Worklog` indexes the worklog review's
/// rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgendaEntry {
    Task(TaskId),
    Dependency(TaskId),
    Subtask(usize),
    Note(usize),
    TimeEntry(usize),
    Comment(usize),
    Conflict(SyncField),
    Worklog(usize),
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
    /// Char column where a `TaskRow`'s trailing priority word begins,
    /// dimmed from there to the end of the line -- mirrors `git_panel::
    /// GitLine::dim_from`. `None` for every other style, and for a
    /// Kanban `TaskRow` (`render_board` packs several cards per physical
    /// line, so a single "dim from here to the end" column can't
    /// describe all of them at once the way one task per line can).
    pub dim_from: Option<usize>,
}

impl AgendaLine {
    fn plain(style: AgendaLineStyle) -> Self {
        Self { style, entries: Vec::new(), badges: Vec::new(), dim_from: None }
    }

    fn row(style: AgendaLineStyle, entry: AgendaEntry) -> Self {
        Self { style, entries: vec![(0, entry)], badges: Vec::new(), dim_from: None }
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

    /// A plain line with one colored span.
    fn push_badged(&mut self, text: &str, style: AgendaLineStyle, badge: (usize, usize, AgendaBadgeColor)) {
        self.push(text, Some(AgendaLine { style, entries: Vec::new(), badges: vec![badge], dim_from: None }));
    }

    fn push_blank(&mut self) {
        self.push("", None);
    }

    fn push_section(&mut self, title: &str) {
        self.push_plain(title, AgendaLineStyle::SectionHeader);
        self.push_plain(&"-".repeat(title.chars().count()), AgendaLineStyle::Detail);
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
const ROW_ACTION_HINTS: &str =
    "Enter detail  ·  s status  ·  p priority  ·  c category  ·  t clock  ·  T log time  ·  N note  ·  e title  ·  E description  ·  I Jira  ·  x archive  ·  D delete";

/// `render_detail`'s own footer -- `b`/`a` (add dependency/subtask) are
/// deliberately left out, since their own sections already hint them
/// inline right where they apply ("(none) -- press b to add one"); this
/// covers only the actions detail has no other hint for.
const DETAIL_ACTION_HINTS: &str =
    "Esc back  ·  s status  ·  p priority  ·  c category  ·  t clock  ·  T log time  ·  N note  ·  e title  ·  E description  ·  I Jira  ·  D delete";

/// The extra keys a linked task's detail page has.
const LINKED_ACTION_HINTS: &str = "C comment  ·  A assign  ·  y copy link  ·  o open in browser  ·  r retry";

const WORKLOG_HINTS: &str = "e edit minutes  ·  D drop from this batch  ·  x dismiss (never send)  ·  W send all  ·  Esc back";

fn push_footer(b: &mut Builder, hints: &str, extra: Option<&str>) {
    b.push_blank();
    let line = match extra {
        Some(extra) => format!("  {hints}  ·  {extra}"),
        None => format!("  {hints}"),
    };
    b.push_plain(&line, AgendaLineStyle::Footer);
}

/// A status LED -- a plain body-font glyph, not a Nerd Font icon, same
/// "must stay legible even without a Nerd Font installed" reasoning
/// `editor_ui.rs`'s own `ƒ` scope marker documents.
const STATUS_LED: char = '■';

/// A change on its way to Jira.
const PENDING_MARK: &str = "↑";
/// A push that failed, or a conflict waiting on you.
const PROBLEM_MARK: &str = "⚠";

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
    format_minutes(d.num_minutes())
}

fn format_minutes(total_minutes: i64) -> String {
    let total_minutes = total_minutes.max(0);
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

/// Jira's own status name, when it says more than the column does --
/// "In Review" under In Progress, "On Hold" under Blocked.
fn jira_status_name(task: &Task) -> Option<&str> {
    let name = task.jira.as_ref()?.base.status_name.as_str();
    (!name.is_empty() && !name.eq_ignore_ascii_case(task.status.label())).then_some(name)
}

/// The sync marker for a linked task, if it has one: a conflict or a
/// failed push first (they need you), then pending changes.
fn sync_mark(store: &AgendaStore, task: &Task) -> Option<(String, AgendaBadgeColor)> {
    let link = task.jira.as_ref()?;
    if !link.conflicts.is_empty() {
        return Some((format!("{PROBLEM_MARK} conflict"), AgendaBadgeColor::Bad));
    }
    if link.error.is_some() {
        return Some((format!("{PROBLEM_MARK} not synced"), AgendaBadgeColor::Bad));
    }
    if link.resolving || store.pending_for(task.id).next().is_some() {
        return Some((PENDING_MARK.to_string(), AgendaBadgeColor::Warn));
    }
    None
}

/// "title" or "KEY title".
fn keyed_title(task: &Task) -> String {
    match task.jira_key() {
        Some(key) => format!("{key} {}", task.title),
        None => task.title.clone(),
    }
}

/// One line per task: `■ KEY Title (Category)  · Priority · time · ...`.
/// Sections group by status (`Status::ALL` order), each with its own
/// header, mirroring `jira_panel`'s Projects/Users grouping shape.
pub fn render_list(store: &AgendaStore) -> AgendaPanel {
    let mut b = Builder::new();
    if !store.tasks.iter().any(|t| !t.archived) {
        let (text, meta) = empty_line("No tasks yet -- SPC a n to add one, SPC a i to import from Jira");
        b.push(&text, meta);
        return b.finish();
    }
    for status in Status::ALL {
        let tasks = sorted_in_status(store, status, false);
        if tasks.is_empty() {
            continue;
        }
        b.push_section(&format!("{} ({})", status.label(), tasks.len()));
        for task in tasks {
            push_task_row(&mut b, store, task);
        }
        b.push_blank();
    }
    push_footer(&mut b, ROW_ACTION_HINTS, None);
    b.finish()
}

fn push_task_row(b: &mut Builder, store: &AgendaStore, task: &Task) {
    let prefix = format!("  {STATUS_LED} ");
    let badge_len = prefix.chars().count();
    let mut badges = vec![(0, badge_len, priority_color(task.priority))];
    if let Some(key) = task.jira_key() {
        badges.push((badge_len, key.chars().count(), AgendaBadgeColor::Neutral));
    }
    let middle = format!("{}{}", keyed_title(task), category_tag(task));
    let dim_from = prefix.chars().count() + middle.chars().count();
    let mut line = format!("{prefix}{middle}  · {}", task.priority.label());
    let elapsed = store.elapsed_on(task.id);
    if elapsed > chrono::Duration::zero() {
        line.push_str(&format!("  · {}", format_duration(elapsed)));
    }
    if !store.is_ready(task.id) && task.status != Status::Done {
        line.push_str("  · waiting on a dependency");
    }
    if let Some(name) = jira_status_name(task) {
        line.push_str(&format!("  · {name}"));
    }
    if task.jira.as_ref().is_some_and(|l| l.not_mine) {
        line.push_str("  · reassigned");
    }
    if let Some((mark, color)) = sync_mark(store, task) {
        line.push_str("  ");
        badges.push((line.chars().count(), mark.chars().count(), color));
        line.push_str(&mark);
    }
    b.push(&line, Some(AgendaLine { style: AgendaLineStyle::TaskRow, entries: vec![(0, AgendaEntry::Task(task.id))], badges, dim_from: Some(dim_from) }));
}

const CARD_WIDTH: usize = 26;
const COLUMN_GAP: &str = "  |  ";

/// Four side-by-side status columns, plain-ASCII-separated (no
/// box-drawing glyphs -- same reasoning `git_graph_style`'s own doc
/// comment gives for defaulting its own graph to ascii: a font missing
/// those glyphs knocks every column out of alignment). Each card is two
/// lines (title + priority badge, then category/waiting marker -- or, for
/// a linked task, its key and Jira status), with a blank separator line
/// between cards in the same column. Every physical line therefore
/// carries up to four entries, one per column -- see
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
        let (text, meta) = empty_line("No tasks yet -- SPC a n to add one, SPC a i to import from Jira");
        b.push(&text, meta);
        return b.finish();
    }

    let column_stride = CARD_WIDTH + COLUMN_GAP.chars().count();
    for row in 0..max_cards {
        let mut line1 = String::new();
        let mut line2 = String::new();
        let mut entries1 = Vec::new();
        let mut badges1 = Vec::new();
        let mut badges2 = Vec::new();
        for (col_idx, column) in columns.iter().enumerate() {
            let col_offset = col_idx * column_stride;
            match column.get(row) {
                Some(task) => {
                    let title_cell = pad_or_truncate(&format!("[{}] {}", task.priority.label(), task.title), CARD_WIDTH);
                    let badge_len = format!("[{}]", task.priority.label()).chars().count();
                    let mut parts: Vec<String> = Vec::new();
                    if let Some((mark, color)) = sync_mark(store, task) {
                        let mark = mark.split(' ').next().unwrap_or_default().to_string();
                        badges2.push((col_offset, mark.chars().count(), color));
                        parts.push(mark);
                    }
                    match task.jira_key() {
                        Some(key) => {
                            parts.push(key.to_string());
                            if let Some(name) = jira_status_name(task) {
                                parts.push(format!("· {name}"));
                            }
                        }
                        None => {
                            let category = category_tag(task).trim_start().to_string();
                            if !category.is_empty() {
                                parts.push(category);
                            }
                        }
                    }
                    if !store.is_ready(task.id) && task.status != Status::Done {
                        parts.push("(waiting)".to_string());
                    }
                    line1.push_str(&title_cell);
                    line2.push_str(&pad_or_truncate(&parts.join(" "), CARD_WIDTH));
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
        b.push(&line1, Some(AgendaLine { style: AgendaLineStyle::TaskRow, entries: entries1, badges: badges1, dim_from: None }));
        b.push(&line2, Some(AgendaLine { style: AgendaLineStyle::TaskRow, entries: entries2, badges: badges2, dim_from: None }));
        if row + 1 < max_cards {
            b.push_blank();
        }
    }
    push_footer(&mut b, ROW_ACTION_HINTS, Some("H/L move column  ·  J/K reorder"));
    b.finish()
}

/// Time entries grouped by calendar day, then by task within the day, with
/// per-day and per-task-overall totals -- a personal timesheet, not
/// billing-grade (minute precision, local time, no rounding rules). Time
/// on linked tasks that hasn't gone to Jira yet is totalled at the top,
/// with the key that reviews and sends it.
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

    let (unsent, issues) = store.unsent_time();
    if issues > 0 {
        let plural = if issues == 1 { "issue" } else { "issues" };
        let line = format!("{PROBLEM_MARK} Unsent to Jira: {} across {issues} {plural} -- W to review and send", format_duration(unsent));
        b.push_badged(&line, AgendaLineStyle::SectionHeader, (0, 1, AgendaBadgeColor::Warn));
        b.push_blank();
    }

    for (day, entries) in by_day.iter().rev() {
        b.push_section(&day.format("%Y-%m-%d (%A)").to_string());

        let mut per_task: std::collections::BTreeMap<TaskId, (String, chrono::Duration)> = std::collections::BTreeMap::new();
        for (task, duration) in entries {
            let e = per_task.entry(task.id).or_insert((keyed_title(task), chrono::Duration::zero()));
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

    b.push_section("Total by task");
    let mut totals: std::collections::BTreeMap<TaskId, (String, chrono::Duration)> = std::collections::BTreeMap::new();
    for task in &store.tasks {
        let total = task.total_time();
        if total > chrono::Duration::zero() {
            totals.insert(task.id, (keyed_title(task), total));
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
    push_footer(&mut b, ROW_ACTION_HINTS, Some("W worklogs"));

    b.finish()
}

/// The worklog review: `rows` as they'll be sent (already rounded, with
/// any edits and drops applied by the caller), one per issue per day.
pub fn render_worklogs(rows: &[WorklogRow], round: u32) -> AgendaPanel {
    let mut b = Builder::new();
    let title = if round > 1 { format!("Worklogs to send  (rounded to {})", format_minutes(i64::from(round))) } else { "Worklogs to send".to_string() };
    b.push_section(&title);
    if rows.is_empty() {
        let (text, meta) = empty_line("Nothing to send -- every bit of time on a linked task is already in Jira");
        b.push(&text, meta);
        push_footer(&mut b, "Esc back", None);
        return b.finish();
    }
    let mut total = 0;
    for (i, row) in rows.iter().enumerate() {
        total += row.minutes;
        let change = if row.minutes == row.actual_minutes {
            format_minutes(row.minutes)
        } else {
            format!("{} -> {}", format_minutes(row.actual_minutes), format_minutes(row.minutes))
        };
        let line = format!("  {}  {}  {}  {change}", row.date.format("%Y-%m-%d"), pad_or_truncate(&row.key, 10), pad_or_truncate(&row.title, 30));
        let key_start = 2 + 10 + 2;
        b.push(
            &line,
            Some(AgendaLine {
                style: AgendaLineStyle::TaskRow,
                entries: vec![(0, AgendaEntry::Worklog(i))],
                badges: vec![(key_start, row.key.chars().count(), AgendaBadgeColor::Neutral)],
                dim_from: None,
            }),
        );
    }
    b.push_blank();
    b.push_plain(&format!("Total: {}", format_minutes(total)), AgendaLineStyle::SectionHeader);
    push_footer(&mut b, WORKLOG_HINTS, None);
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

/// Jira's `2024-01-15T10:30:00.000+0000` in local time.
fn parse_jira_time(raw: &str) -> Option<chrono::DateTime<chrono::Local>> {
    chrono::DateTime::parse_from_str(raw, "%Y-%m-%dT%H:%M:%S%.3f%z").ok().map(|t| t.with_timezone(&chrono::Local))
}

/// "just now", "12m ago", "3h ago", or a date.
fn ago(at: chrono::DateTime<chrono::Local>) -> String {
    let minutes = (chrono::Local::now() - at).num_minutes();
    if minutes < 1 {
        "just now".to_string()
    } else if minutes < 60 {
        format!("{minutes}m ago")
    } else if minutes < 24 * 60 {
        format!("{}h ago", minutes / 60)
    } else {
        at.format("%Y-%m-%d %H:%M").to_string()
    }
}

/// The single task detail pane: title + status badge, priority/category/
/// timestamps, the "Blocked by"/"Blocks" dependency sections, the subtask
/// checklist, the notes log (newest last), and the time-entry list --
/// everything one task can carry, laid out to read top-to-bottom like
/// `jira_panel::render_detail`'s own issue page. A linked task adds its
/// key, Jira's status/priority/assignee, sync state, any conflicts, and
/// mixes Jira's comments into its notes as one "Activity" timeline.
pub fn render_detail(store: &AgendaStore, id: TaskId) -> AgendaPanel {
    let mut b = Builder::new();
    let Some(task) = store.task(id) else {
        let (text, meta) = empty_line("This task no longer exists");
        b.push(&text, meta);
        return b.finish();
    };
    let link = task.jira.as_ref();

    b.push_plain(&keyed_title(task), AgendaLineStyle::Title);
    let mut status_line = format!("[{}]", task.status.label());
    let badge_len = status_line.chars().count();
    if let Some(name) = jira_status_name(task) {
        status_line.push_str(&format!("  Jira: {name}"));
    }
    status_line.push_str(&format!("  ·  Priority: {}", task.priority.label()));
    if let Some(remote) = link.and_then(|l| l.base.priority.as_deref()).filter(|p| !p.eq_ignore_ascii_case(task.priority.label())) {
        status_line.push_str(&format!(" (Jira: {remote})"));
    }
    status_line.push_str(&category_tag(task));
    b.push_badged(&status_line, AgendaLineStyle::Detail, (0, badge_len, status_color(task.status)));

    if let Some(link) = link {
        let assignee = link.base.assignee.as_deref().unwrap_or("Unassigned");
        let synced = link.last_synced.map(ago).unwrap_or_else(|| "never".to_string());
        b.push_plain(&format!("Assignee: {assignee}  ·  Synced {synced}"), AgendaLineStyle::Detail);
        if let Some(error) = &link.error {
            let line = format!("{PROBLEM_MARK} Couldn't update Jira: {error} -- r retries");
            b.push_badged(&line, AgendaLineStyle::Detail, (0, 1, AgendaBadgeColor::Bad));
        }
        let pending = store.pending_for(id).count();
        if pending > 0 || link.resolving {
            let what = if pending == 1 { "1 change".to_string() } else { format!("{pending} changes") };
            let line = if link.resolving { format!("{PENDING_MARK} Checking Jira's workflow...") } else { format!("{PENDING_MARK} {what} waiting to be sent") };
            b.push_badged(&line, AgendaLineStyle::Detail, (0, 1, AgendaBadgeColor::Warn));
        }
        if link.not_mine {
            b.push_badged(&format!("{PROBLEM_MARK} Reassigned in Jira -- no longer assigned to you"), AgendaLineStyle::Detail, (0, 1, AgendaBadgeColor::Warn));
        }
        if !link.conflicts.is_empty() {
            b.push_blank();
            b.push_section("Conflicts -- Enter to choose");
            for conflict in &link.conflicts {
                let mine = match conflict.field {
                    SyncField::Title => task.title.clone(),
                    SyncField::Description => task.description.clone(),
                    SyncField::Status => task.status.label().to_string(),
                    SyncField::Priority => task.priority.label().to_string(),
                };
                let line = format!(
                    "  {PROBLEM_MARK} {}: Jira has \"{}\", you have \"{}\"",
                    conflict.field.label(),
                    first_line(&conflict.their_value()),
                    first_line(&mine)
                );
                b.push(
                    &line,
                    Some(AgendaLine {
                        style: AgendaLineStyle::TaskRow,
                        entries: vec![(0, AgendaEntry::Conflict(conflict.field))],
                        badges: vec![(2, 1, AgendaBadgeColor::Bad)],
                        dim_from: None,
                    }),
                );
            }
        }
    }
    b.push_blank();

    b.push_plain(&format!("Created: {}", task.created_at.format("%Y-%m-%d %H:%M")), AgendaLineStyle::Detail);
    b.push_plain(&format!("Updated: {}", task.updated_at.format("%Y-%m-%d %H:%M")), AgendaLineStyle::Detail);
    let elapsed = store.elapsed_on(id);
    b.push_plain(&format!("Time spent: {}", format_duration(elapsed)), AgendaLineStyle::Detail);
    b.push_blank();

    b.push_section("Description");
    push_wrapped(&mut b, "  ", &task.description, AgendaLineStyle::Body, None);
    b.push_blank();

    let blocked_by = store.blocked_by(id);
    b.push_section("Blocked by");
    if blocked_by.is_empty() {
        b.push_plain("  (none) -- press b to add one", AgendaLineStyle::Empty);
    } else {
        for dep in blocked_by {
            let done = dep.status == Status::Done;
            let marker = if done { STATUS_LED } else { CHECKBOX_PENDING };
            let color = if done { AgendaBadgeColor::Good } else { AgendaBadgeColor::Neutral };
            let prefix = format!("  {marker} ");
            let badge_len = prefix.chars().count();
            let line = format!("{prefix}{}", dep.title);
            b.push(
                &line,
                Some(AgendaLine {
                    style: AgendaLineStyle::TaskRow,
                    entries: vec![(0, AgendaEntry::Dependency(dep.id))],
                    badges: vec![(0, badge_len, color)],
                    dim_from: None,
                }),
            );
        }
    }
    b.push_blank();

    let blocks = store.blocks(id);
    if !blocks.is_empty() {
        b.push_section("Blocks");
        for dependent in blocks {
            let line = format!("  {}", dependent.title);
            b.push_row(&line, AgendaLineStyle::TaskRow, AgendaEntry::Task(dependent.id));
        }
        b.push_blank();
    }

    b.push_section("Subtasks");
    if task.subtasks.is_empty() {
        b.push_plain("  (none) -- press a to add one", AgendaLineStyle::Empty);
    } else {
        for (i, subtask) in task.subtasks.iter().enumerate() {
            push_subtask_row(&mut b, i, subtask);
        }
    }
    b.push_blank();

    match link {
        Some(link) => push_activity(&mut b, store, task, &link.base.comments),
        None => {
            b.push_section("Notes");
            if task.notes.is_empty() {
                b.push_plain("  (none) -- press N to add one", AgendaLineStyle::Empty);
            } else {
                for (i, note) in task.notes.iter().enumerate() {
                    b.push_row(&format!("  {}", note.at.format("%Y-%m-%d %H:%M")), AgendaLineStyle::Note, AgendaEntry::Note(i));
                    push_wrapped(&mut b, "    ", &note.text, AgendaLineStyle::Body, Some(AgendaEntry::Note(i)));
                }
            }
        }
    }
    b.push_blank();

    b.push_section("Time entries");
    if task.time_entries.is_empty() {
        b.push_plain("  (none)", AgendaLineStyle::Empty);
    } else {
        for (i, entry) in task.time_entries.iter().enumerate() {
            let source = match entry.source {
                TimeSource::Timer => "timer",
                TimeSource::Manual => "manual",
            };
            let mut line = format!(
                "  {} - {}  {}  ({source})",
                entry.start.format("%Y-%m-%d %H:%M"),
                entry.end.format("%H:%M"),
                format_duration(entry.duration())
            );
            if link.is_some() {
                line.push_str(if entry.sent { "  sent" } else { "  unsent" });
            }
            b.push_row(&line, AgendaLineStyle::Detail, AgendaEntry::TimeEntry(i));
        }
    }
    push_footer(&mut b, DETAIL_ACTION_HINTS, link.map(|_| LINKED_ACTION_HINTS));
    let on_note = if link.is_some() { "on a note: e edits it, D removes it, C posts it as a comment" } else { "on a note: e edits it, D removes it" };
    b.push_plain(&format!("  {on_note}   ·   on a time entry: D removes it"), AgendaLineStyle::Footer);

    b.finish()
}

fn first_line(text: &str) -> String {
    let line = text.lines().next().unwrap_or_default();
    let clipped: String = line.chars().take(60).collect();
    if clipped.chars().count() < line.chars().count() || text.lines().count() > 1 {
        format!("{clipped}…")
    } else {
        clipped
    }
}

/// Notes and Jira comments in one timeline, oldest first, each labelled so
/// a private note never reads as a public comment -- plus any comment
/// still on its way to Jira, at the end.
fn push_activity(b: &mut Builder, store: &AgendaStore, task: &Task, comments: &[fenix_agenda::RemoteComment]) {
    b.push_section("Activity");
    enum Item<'a> {
        Note(usize),
        Comment(usize, &'a fenix_agenda::RemoteComment),
    }
    let mut items: Vec<(Option<chrono::DateTime<chrono::Local>>, Item)> = Vec::new();
    for (i, note) in task.notes.iter().enumerate() {
        items.push((Some(note.at), Item::Note(i)));
    }
    for (i, comment) in comments.iter().enumerate() {
        items.push((parse_jira_time(&comment.created), Item::Comment(i, comment)));
    }
    // Unparseable timestamps sort last rather than first.
    items.sort_by_key(|(at, _)| (at.is_none(), *at));
    let sending: Vec<&str> = store
        .pending_for(task.id)
        .filter_map(|op| match &op.kind {
            OpKind::AddComment(body) => Some(body.as_str()),
            _ => None,
        })
        .collect();

    if items.is_empty() && sending.is_empty() {
        b.push_plain("  (none) -- N adds a private note, C a Jira comment", AgendaLineStyle::Empty);
        return;
    }
    for (at, item) in items {
        let when = at.map(|t| t.format("%Y-%m-%d %H:%M").to_string()).unwrap_or_default();
        match item {
            Item::Note(i) => {
                b.push_row(&format!("  {when}  note (private)"), AgendaLineStyle::Note, AgendaEntry::Note(i));
                push_wrapped(b, "    ", &task.notes[i].text, AgendaLineStyle::Body, Some(AgendaEntry::Note(i)));
            }
            Item::Comment(i, comment) => {
                b.push_row(&format!("  {when}  {}", comment.author), AgendaLineStyle::Note, AgendaEntry::Comment(i));
                push_wrapped(b, "    ", &comment.body, AgendaLineStyle::Body, Some(AgendaEntry::Comment(i)));
            }
        }
    }
    for body in sending {
        b.push_badged(&format!("  {PENDING_MARK} sending comment"), AgendaLineStyle::Note, (2, 1, AgendaBadgeColor::Warn));
        push_wrapped(b, "    ", body, AgendaLineStyle::Body, None);
    }
}

/// A pending subtask's hollow-square companion to `STATUS_LED` -- same
/// plain body-font pairing real checkbox UIs use for "not done yet",
/// legible without a Nerd Font the same way `STATUS_LED` already is.
const CHECKBOX_PENDING: char = '□';

fn push_subtask_row(b: &mut Builder, index: usize, subtask: &Subtask) {
    let (mark, style) =
        if subtask.done { (STATUS_LED, AgendaLineStyle::SubtaskDone) } else { (CHECKBOX_PENDING, AgendaLineStyle::SubtaskPending) };
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
        assert!(panel.text.contains(&format!("{STATUS_LED} Write the plan (Fenix)  · High")));
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
        assert!(panel.text.contains(&format!("{CHECKBOX_PENDING} Dep")));

        let subtask_row = entries_of(&panel).into_iter().find(|l| l.entries.iter().any(|(_, e)| matches!(e, AgendaEntry::Subtask(_)))).unwrap();
        assert_eq!(subtask_row.entry_at(0), Some(AgendaEntry::Subtask(0)));
        assert!(panel.text.contains(&format!("{CHECKBOX_PENDING} Draft outline")));

        let dep_row = entries_of(&panel).into_iter().find(|l| l.entries.iter().any(|(_, e)| *e == AgendaEntry::Dependency(dep))).unwrap();
        assert_eq!(dep_row.entry_at(0), Some(AgendaEntry::Dependency(dep)));
        assert_eq!(dep_row.badges[0].2, AgendaBadgeColor::Neutral);
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

    fn linked_store(status: &str) -> (AgendaStore, TaskId) {
        let mut store = AgendaStore::default();
        let snapshot = fenix_agenda::RemoteSnapshot {
            summary: "Fix login timeout".to_string(),
            status_id: "5".to_string(),
            status_name: status.to_string(),
            status_category: "indeterminate".to_string(),
            priority: Some("Major".to_string()),
            assignee: Some("Jo".to_string()),
            comments: vec![fenix_agenda::RemoteComment {
                id: "1".to_string(),
                author: "Jane Smith".to_string(),
                body: "Can we also cover SSO?".to_string(),
                created: "2020-01-02T10:00:00.000+0000".to_string(),
            }],
            ..Default::default()
        };
        let update = fenix_agenda::RemoteUpdate { snapshot, status: Status::InProgress, priority: Priority::High, mine: Some(true) };
        let id = store.create_linked("PROJ-12".to_string(), update, Some("Backend".to_string()));
        (store, id)
    }

    #[test]
    fn a_linked_row_shows_its_key_and_jiras_status_when_it_differs_from_the_column() {
        let (store, _) = linked_store("In Review");
        let panel = render_list(&store);
        assert!(panel.text.contains(&format!("{STATUS_LED} PROJ-12 Fix login timeout (Backend)  · High  · In Review")));

        let (store, _) = linked_store("In Progress");
        assert!(!render_list(&store).text.contains("· In Progress"), "no point repeating the column name");
    }

    #[test]
    fn a_pending_change_shows_the_pending_mark_and_a_failure_the_problem_mark() {
        let (mut store, id) = linked_store("In Review");
        store.enqueue(id, fenix_agenda::OpKind::SetSummary("x".to_string()));
        let panel = render_list(&store);
        let row = panel.text.lines().find(|l| l.contains("PROJ-12")).unwrap();
        assert!(row.ends_with(PENDING_MARK));
        let meta = entries_of(&panel).into_iter().find(|l| l.entries.iter().any(|(_, e)| *e == AgendaEntry::Task(id))).unwrap();
        assert!(meta.badges.iter().any(|&(_, _, c)| c == AgendaBadgeColor::Warn));

        let op = store.next_op().unwrap().id;
        store.op_failed(op, "HTTP 403".to_string());
        let panel = render_list(&store);
        assert!(panel.text.contains(&format!("{PROBLEM_MARK} not synced")));
    }

    #[test]
    fn a_linked_board_card_shows_the_key_and_jira_status_on_its_second_line() {
        let (store, _) = linked_store("In Review");
        let panel = render_board(&store);
        assert!(panel.text.contains("PROJ-12 · In Review"));
    }

    #[test]
    fn a_linked_detail_page_has_jira_metadata_and_one_activity_timeline() {
        let (mut store, id) = linked_store("In Review");
        store.add_note(id, "private thought".to_string());
        store.enqueue(id, fenix_agenda::OpKind::AddComment("on its way".to_string()));

        let panel = render_detail(&store, id);
        assert_eq!(panel.text.lines().next(), Some("PROJ-12 Fix login timeout"));
        assert!(panel.text.contains("[In Progress]  Jira: In Review  ·  Priority: High (Jira: Major) (Backend)"));
        assert!(panel.text.contains("Assignee: Jo"));
        assert!(panel.text.contains("Activity"));
        assert!(!panel.text.contains("\nNotes\n"));
        let jane = panel.text.find("Jane Smith").unwrap();
        let note = panel.text.find("note (private)").unwrap();
        assert!(jane < note, "the 2020 comment sorts before today's note");
        assert!(panel.text.contains("sending comment"));
        assert!(panel.text.contains("C comment"));
        let comment_row = entries_of(&panel).into_iter().find(|l| l.entries.iter().any(|(_, e)| *e == AgendaEntry::Comment(0)));
        assert!(comment_row.is_some());
    }

    #[test]
    fn a_conflict_gets_its_own_selectable_row() {
        let (mut store, id) = linked_store("In Review");
        store.set_title(id, "Mine".to_string());
        store.enqueue(id, fenix_agenda::OpKind::SetSummary("Mine".to_string()));
        let mut theirs = store.task(id).unwrap().jira.as_ref().unwrap().base.clone();
        theirs.summary = "Theirs".to_string();
        store.apply_remote(id, fenix_agenda::RemoteUpdate { snapshot: theirs, status: Status::InProgress, priority: Priority::High, mine: None });

        let panel = render_detail(&store, id);
        assert!(panel.text.contains("title: Jira has \"Theirs\", you have \"Mine\""));
        let row = entries_of(&panel).into_iter().find(|l| l.entries.iter().any(|(_, e)| *e == AgendaEntry::Conflict(SyncField::Title)));
        assert!(row.is_some());
    }

    #[test]
    fn the_report_totals_unsent_linked_time_and_points_at_the_review() {
        let (mut store, id) = linked_store("In Review");
        store.log_manual_time(id, chrono::Duration::minutes(90));
        let panel = render_report(&store);
        assert!(panel.text.contains("Unsent to Jira: 1h 30m across 1 issue -- W to review and send"));
        assert!(panel.text.contains("PROJ-12 Fix login timeout"));
    }

    #[test]
    fn the_worklog_review_shows_rounding_and_a_total() {
        let (mut store, id) = linked_store("In Review");
        store.log_manual_time(id, chrono::Duration::minutes(50));
        let rows = store.worklog_batch(15);
        let panel = render_worklogs(&rows, 15);
        assert!(panel.text.contains("Worklogs to send  (rounded to 15m)"));
        assert!(panel.text.contains("50m -> 45m"));
        assert!(panel.text.contains("Total: 45m"));
        assert!(panel.text.contains("W send all"));
        let row = entries_of(&panel).into_iter().find(|l| l.entries.iter().any(|(_, e)| *e == AgendaEntry::Worklog(0)));
        assert!(row.is_some());

        assert!(render_worklogs(&[], 15).text.contains("Nothing to send"));
    }
}
