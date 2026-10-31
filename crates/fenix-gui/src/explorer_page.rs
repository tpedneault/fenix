//! The explorer as a page: one listing, laid out at three widths -- the
//! sidebar pinned at the left (names and status), a pane (`SPC f j`:
//! names, status, size and age) and the directory picker (`SPC p a`,
//! `SPC f e`: a pane's columns, with what `S` picks). The listing's
//! state is `fenix_explorer::ExplorerState`; what's here is how it is
//! drawn: a breadcrumb header, the tree with chevrons and indent guides,
//! an icon for every file, a status column, and a key strip -- or, after
//! `?`, every key.
//!
//! The page shows the rows that fit, scrolled to keep the selection in
//! view, with the header above and the key strip below them always on
//! screen -- so the page is laid out for the pane's height too.

use std::ops::Range;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use fenix_explorer::{Entry, ExplorerState, GitStatus, Row};
use fenix_project::ProjectKind;

use crate::page::{edit_line, fit, fit_tail, insert_at, with_caret, Grid, Key, Page, Popup, Role};

/// Which of the three the page is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Form {
    /// Pinned at the frame's left: names and status only.
    Sidebar,
    /// In a pane, with size and age columns.
    Pane,
    /// Picking a folder: a pane, and the line saying what `S` does.
    Picker { picks: &'static str },
}

/// The project the listing is in, for the header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    pub name: String,
    pub root: PathBuf,
    pub kind: ProjectKind,
}

/// Everything about the page that isn't the listing itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct View {
    pub form: Form,
    /// `?`: every key, in place of the rows.
    pub help: bool,
    pub project: Option<Project>,
    pub branch: Option<String>,
    /// A directory being read that hasn't answered yet.
    pub waiting_on: Option<PathBuf>,
    /// The first listing row shown.
    pub scroll: usize,
    /// The page line the first shown row is on, and how many show.
    pub rows_at: usize,
    pub rows_shown: usize,
    /// The pane height the page was laid out for.
    pub height: Option<usize>,
    /// The file being edited, tinted where it's listed.
    pub current: Option<PathBuf>,
    /// The selected row the last layout scrolled to, so the wheel's
    /// scroll stays put until the selection moves.
    pub scrolled_to: Option<usize>,
    /// The breadcrumb's segments as laid out: their columns on line 0,
    /// and the folder each names -- what a click there goes to.
    pub crumbs: Vec<(Range<usize>, PathBuf)>,
    /// For each row shown, the column of its chevron.
    pub chevrons: Vec<usize>,
    /// A name being typed in place: a new file or folder, or a rename.
    pub edit: Option<RowEdit>,
    /// A small menu beside the selected row (what to do when a copy
    /// would overwrite something), as its rows.
    pub menu: Option<Vec<Vec<(String, Role)>>>,
}

/// What a name typed in a row is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditKind {
    NewFile,
    NewFolder,
    Rename,
}

/// A name being typed in place, in a row of the listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowEdit {
    pub kind: EditKind,
    pub text: String,
    /// Where the caret is, in chars.
    pub caret: usize,
    /// Chars selected, replaced by what's typed: a rename's name without
    /// its extension.
    pub selected: Option<Range<usize>>,
    /// The folder the name goes in.
    pub dir: PathBuf,
    /// The listing row it's on (a rename) or under (a new one).
    pub row: Option<usize>,
    /// How deep the new row sits.
    pub depth: usize,
}

/// What a key did to a name being typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditStep {
    Typing,
    /// Enter: the name, trimmed.
    Done(String),
    Cancel,
}

impl RowEdit {
    /// A new file or folder: inside the selected row when it's an open
    /// folder, else beside it; at the top of an empty listing.
    pub fn new(kind: EditKind, state: &ExplorerState) -> Self {
        let (dir, depth) = match (state.selected_entry(), state.rows.get(state.selected)) {
            (Some(entry), Some(row)) if entry.is_dir() && row.expanded => (entry.path.clone(), entry.depth + 1),
            (Some(entry), _) => (entry.path.parent().map(Path::to_path_buf).unwrap_or_else(|| state.cwd.clone()), entry.depth),
            _ => (state.cwd.clone(), 0),
        };
        let row = (!state.entries.is_empty()).then_some(state.selected);
        RowEdit { kind, text: String::new(), caret: 0, selected: None, dir, row, depth }
    }

    /// A rename of the selected row, its name typed over with the part
    /// before the extension selected.
    pub fn rename(state: &ExplorerState) -> Option<Self> {
        let entry = state.selected_entry()?;
        let name = entry.name.clone();
        let len = name.chars().count();
        let stem = match name.rfind('.') {
            Some(at) if at > 0 && !entry.is_dir() => name[..at].chars().count(),
            _ => len,
        };
        Some(RowEdit {
            kind: EditKind::Rename,
            caret: stem,
            selected: Some(0..stem),
            text: name,
            dir: entry.path.parent().map(Path::to_path_buf).unwrap_or_else(|| state.cwd.clone()),
            row: Some(state.selected),
            depth: entry.depth,
        })
    }

    /// Drops the selected chars, the caret where they were.
    fn take_selection(&mut self) -> bool {
        let Some(sel) = self.selected.take().filter(|s| !s.is_empty()) else { return false };
        let chars: Vec<char> = self.text.chars().collect();
        self.text = chars[..sel.start].iter().chain(&chars[sel.end..]).collect();
        self.caret = sel.start;
        true
    }

    /// One key: typed into the name (over the selection), the caret
    /// moved, or the name finished or given up.
    pub fn key(&mut self, key: Key) -> EditStep {
        match key {
            Key::Enter => return EditStep::Done(self.text.trim().to_string()),
            Key::Escape => return EditStep::Cancel,
            Key::Backspace | Key::Delete | Key::DeleteWordBack if self.take_selection() => {}
            Key::Char(_) | Key::Space => {
                self.take_selection();
                edit_line(&mut self.text, &mut self.caret, key);
            }
            _ => {
                self.selected = None;
                edit_line(&mut self.text, &mut self.caret, key);
            }
        }
        EditStep::Typing
    }

    /// Pasted text, over the selection.
    pub fn paste(&mut self, text: &str) {
        self.take_selection();
        insert_at(&mut self.text, &mut self.caret, text);
    }
}

impl View {
    pub fn new(form: Form) -> Self {
        View {
            form,
            help: false,
            project: None,
            branch: None,
            waiting_on: None,
            scroll: 0,
            rows_at: 0,
            rows_shown: 0,
            height: None,
            current: None,
            scrolled_to: None,
            crumbs: Vec::new(),
            chevrons: Vec::new(),
            edit: None,
            menu: None,
        }
    }

    /// The listing row on page line `line`, if one is shown there.
    pub fn row_at(&self, line: usize) -> Option<usize> {
        (line >= self.rows_at && line < self.rows_at + self.rows_shown).then(|| self.scroll + line - self.rows_at)
    }

    /// Whether column `col` of page line `line` is that row's chevron.
    pub fn on_chevron(&self, line: usize, col: usize) -> bool {
        line.checked_sub(self.rows_at).and_then(|i| self.chevrons.get(i)).is_some_and(|&at| col >= at && col < at + 2)
    }

    /// The folder the breadcrumb segment at column `col` names.
    pub fn crumb_at(&self, col: usize) -> Option<&Path> {
        self.crumbs.iter().find(|(cols, _)| cols.contains(&col)).map(|(_, path)| path.as_path())
    }

    /// Scrolls the rows by `lines` (down when positive), leaving the
    /// selection where it is.
    pub fn scroll_by(&mut self, lines: isize) {
        self.scroll = self.scroll.saturating_add_signed(lines);
    }
}

/// The key strip's keys, by form.
fn strip(form: Form) -> &'static [(&'static str, &'static str)] {
    match form {
        Form::Sidebar => &[("Enter", "open"), ("Tab", "fold"), ("?", "keys")],
        Form::Pane => &[("Enter", "open"), ("Tab", "fold"), ("h", "up"), ("m", "mark"), ("R", "rename"), ("D", "delete"), ("/", "filter"), ("p", "preview"), ("?", "all keys")],
        Form::Picker { .. } => &[("S", "pick this folder"), ("Enter", "go in"), ("h", "up"), ("Tab", "fold"), ("Esc", "cancel"), ("?", "all keys")],
    }
}

/// Every key, by what it's for -- `?`. A test holds this to the key
/// table, so neither can name a key the other hasn't.
pub const HELP: &[(&str, &[(&str, &str)])] = &[
    ("Move", &[("j k ↓ ↑", "down, up"), ("gg G", "first, last"), ("Enter l", "open, or go into a folder"), ("h -", "the folder above"), ("Tab", "open or close a folder")]),
    ("Mark", &[("m u", "mark, unmark"), ("U", "unmark all"), ("t", "flip every mark")]),
    (
        "Files",
        &[
            ("a A", "new file, folder, typed in place (a / makes folders)"),
            ("c +", "the same, as before"),
            ("R", "rename in place"),
            ("D", "delete"),
            ("C M", "copy, move to..."),
            ("z x", "archive, unpack"),
            ("i", "properties"),
            ("w", "flip read-only"),
        ],
    ),
    ("Preview", &[("p", "a preview beside, following the selection"), ("P", "open in the editor's preview tab"), ("J K", "scroll the preview")]),
    ("View", &[("/ f", "filter the whole tree"), ("n N", "next, previous match"), ("F", "find under here"), (".", "hidden files"), ("o O", "sort by, reverse"), ("g r  r", "refresh"), ("?", "these keys")]),
    ("Leave", &[("S", "pick this folder (when picking)"), ("q Esc", "close")]),
    (
        "SPC e",
        &[
            ("SPC e p", "go to a path"),
            ("SPC e b", "places"),
            ("SPC e m", "bookmark here"),
            ("SPC e w", "rename as text"),
            ("SPC e d", "two panes"),
            ("SPC e o", "open with the system"),
            ("SPC e y", "copy the path"),
            ("SPC e T", "terminal here"),
            ("SPC e g", "search here"),
        ],
    ),
];

/// `name` in at most `max` columns, cut in the middle so its start and
/// its extension both show: `explorer_ho…st.rs`.
pub fn fit_middle(name: &str, max: usize) -> String {
    let chars: Vec<char> = name.chars().collect();
    if chars.len() <= max {
        return name.to_string();
    }
    if max < 4 {
        return fit(name, max);
    }
    // The extension, with a few characters before it when there's room.
    let ext = name.rfind('.').filter(|&at| at > 0).map(|at| name[at..].chars().count()).filter(|&n| n < max / 2).unwrap_or(0);
    let tail = (ext + 2).min((max - 1) / 2).max(ext);
    let head = max - 1 - tail;
    let mut out: String = chars[..head].iter().collect();
    out.push('…');
    out.extend(&chars[chars.len() - tail..]);
    out
}

/// "512 B", "1.1 K", "37 M".
pub fn size_text(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "K", "M", "G", "T"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if size < 10.0 {
        format!("{size:.1} {}", UNITS[unit])
    } else {
        format!("{size:.0} {}", UNITS[unit])
    }
}

/// "now", "37 min", "5 h", "3 d", "2 w", "4 mo", "2 y" since `when`;
/// blank when there's no time to tell.
pub fn age_text(when: Option<SystemTime>, now: SystemTime) -> String {
    let Some(when) = when else { return String::new() };
    let secs = now.duration_since(when).map(|d| d.as_secs()).unwrap_or(0);
    let mins = secs / 60;
    match mins {
        0 => "now".to_string(),
        1..=59 => format!("{mins} min"),
        60..=1439 => format!("{} h", mins / 60),
        1440..=10079 => format!("{} d", mins / 1440),
        10080..=43199 => format!("{} w", mins / 10080),
        43200..=525_599 => format!("{} mo", mins / 43200),
        _ => format!("{} y", mins / 525_600),
    }
}

/// A row's status letter and its colour: M, U, A, ! -- and for a folder
/// with anything changed under it, a dot.
pub fn status_mark(entry: &Entry) -> Option<(&'static str, Role)> {
    let status = entry.git_status?;
    let mark = match status {
        GitStatus::Ignored => return None,
        GitStatus::Untracked => "U",
        _ if entry.is_dir() => "•",
        GitStatus::Modified => "M",
        GitStatus::Staged => "A",
        GitStatus::Conflicted => "!",
    };
    Some((mark, Role::Git(status)))
}

/// The colour a row's name is drawn in: what git says of it, dimmed
/// when ignored or unreadable.
fn name_role(entry: &Entry) -> Role {
    if !entry.readable {
        return Role::Muted;
    }
    match entry.git_status {
        Some(GitStatus::Ignored) => Role::Muted,
        Some(status) if !entry.is_dir() || status == GitStatus::Conflicted => Role::Git(status),
        _ if entry.is_dir() => Role::Title,
        _ => Role::Text,
    }
}

/// The breadcrumb: the project's name and the folders down to `cwd`
/// under it -- or, outside any project, the path's own parts -- each
/// with the folder it names.
pub fn breadcrumb(cwd: &Path, project: Option<&Project>) -> Vec<(String, PathBuf)> {
    if let Some(project) = project {
        if let Ok(rel) = cwd.strip_prefix(&project.root) {
            let mut at = project.root.clone();
            let mut parts = vec![(project.name.clone(), at.clone())];
            for part in rel.components() {
                at.push(part);
                parts.push((part.as_os_str().to_string_lossy().into_owned(), at.clone()));
            }
            return parts;
        }
    }
    let mut at = PathBuf::new();
    let mut parts = Vec::new();
    for part in cwd.components() {
        at.push(part);
        let name = part.as_os_str().to_string_lossy().trim_end_matches(['\\', '/']).to_string();
        if !name.is_empty() {
            parts.push((name, at.clone()));
        }
    }
    if parts.is_empty() {
        parts.push((cwd.display().to_string(), cwd.to_path_buf()));
    }
    parts
}

/// Lays the explorer out `cols` wide and, when known, `height` rows
/// tall. `state` is `None` before the listing exists at all.
pub fn layout(state: Option<&ExplorerState>, view: &mut View, cols: usize, height: Option<usize>) -> Page {
    let mut g = Grid::new();
    let sidebar = view.form == Form::Sidebar;
    let left = usize::from(!sidebar);
    let width = cols.saturating_sub(left + usize::from(!sidebar)).max(8);
    view.height = height;
    let Some(state) = state else {
        g.put(0, left, "Reading…", Role::Muted);
        return g.finish();
    };

    // Line 0: where this is.
    let crumbs = breadcrumb(&state.cwd, view.project.as_ref());
    let joined = crumbs.iter().map(|(name, _)| name.as_str()).collect::<Vec<_>>().join(" › ");
    let shown = fit_tail(&joined, width);
    // Where each segment landed: cut from the front, a segment is only
    // clickable if it's whole.
    let total = joined.chars().count();
    let cut = total.saturating_sub(shown.chars().count());
    let skew = usize::from(cut > 0);
    view.crumbs.clear();
    let mut at = 0;
    for (name, path) in &crumbs {
        let len = name.chars().count();
        if at >= cut + skew || cut == 0 {
            let from = left + at - cut + skew;
            view.crumbs.push((from..from + len, path.clone()));
        }
        at += len + 3;
    }
    let last = crumbs.last().map(|(name, _)| name.as_str()).unwrap_or("");
    let split = shown.chars().count().saturating_sub(last.chars().count());
    let head: String = shown.chars().take(split).collect();
    let x = g.put(0, left, &head, Role::Muted);
    g.put(0, x, &shown.chars().skip(split).collect::<String>(), Role::Title);

    // Line 1: what it is, and how it's shown.
    let mut facts: Vec<(String, Role)> = Vec::new();
    if let Some(project) = &view.project {
        facts.push((project.kind.label().to_string(), Role::Kind(project.kind)));
    }
    if let Some(branch) = &view.branch {
        facts.push((format!("⎇ {branch}"), Role::Accent));
    }
    let items = state.item_count();
    facts.push((format!("{items} item{}", if items == 1 { "" } else { "s" }), Role::Muted));
    let changed = state.changed_count();
    if changed > 0 {
        facts.push((format!("{changed} changed"), Role::Warn));
    }
    if !state.marks.is_empty() {
        facts.push((format!("{} marked", state.marks.len()), Role::Accent));
    }
    if !sidebar {
        facts.push((format!("by {}", state.sort.label()), Role::Muted));
    }
    if !state.filter.is_empty() {
        // Said out loud: a listing quietly narrowed looks like one that
        // lost files.
        facts.push((format!("filter: {}", state.filter), Role::Accent));
    }
    if state.show_hidden {
        facts.push(("hidden shown".to_string(), Role::Muted));
    }
    if let Some(path) = &view.waiting_on {
        facts.push((format!("reading {} (Esc stops)", path.display()), Role::Warn));
    }
    let mut x = left;
    for (i, (text, role)) in facts.iter().enumerate() {
        let sep = if i == 0 { "" } else { " · " };
        let room = (left + width).saturating_sub(x + sep.chars().count());
        if room < 4 {
            break;
        }
        x = g.put(1, x, sep, Role::Muted);
        x = g.put(1, x, &fit(text, room), *role);
    }
    let mut y = 2;
    if let Form::Picker { picks } = view.form {
        g.put(y, left, &fit(picks, width), Role::Accent);
        y += 1;
    }
    g.rule(y, left..left + width);
    y += 1;

    // What fits between the header and the key strip (a blank line, its
    // hairline and its keys).
    let foot = 3;
    let room = height.map(|h| h.saturating_sub(y + foot).max(1));

    if view.help {
        layout_help(&mut g, y, left, width);
        view.rows_at = y;
        view.rows_shown = 0;
        g.keys(left, width, &[("?", "back to the files"), ("q", "close")]);
        return g.finish();
    }

    let total = state.entries.len();
    let room = room.unwrap_or(total.max(1));
    // A moved selection is brought into view; otherwise the scroll is
    // the wheel's.
    if view.scrolled_to != Some(state.selected) {
        view.scroll = scroll_to_include(view.scroll, state.selected, room);
        view.scrolled_to = Some(state.selected);
    }
    view.scroll = view.scroll.min(total.saturating_sub(room.min(total)));
    view.rows_at = y;
    view.rows_shown = room.min(total.saturating_sub(view.scroll));
    if total == 0 {
        let note = if !state.filter.is_empty() { "nothing matches the filter" } else { "empty" };
        g.put(y, left + 2, note, Role::Muted);
    }
    let now = SystemTime::now();
    view.chevrons.clear();
    // A new name's row goes under the row it's beside, pushing the rest
    // down a line (and the last one off).
    let new_row = view.edit.as_ref().filter(|e| e.kind != EditKind::Rename);
    if let Some(edit) = new_row.filter(|e| e.row.is_none()) {
        draw_field(&mut g, y, new_row_x(left, view.form, edit.depth), left + width, edit, true);
    }
    let mut extra = usize::from(new_row.is_some_and(|e| e.row.is_none()));
    let shown = view.rows_shown.saturating_sub(usize::from(new_row.is_some()));
    for (i, index) in (view.scroll..view.scroll + shown).enumerate() {
        let (entry, row) = (&state.entries[index], &state.rows[index]);
        let line = y + i + extra;
        let current = view.current.as_deref() == Some(entry.path.as_path());
        let chevron = draw_row(&mut g, line, left, width, view.form, entry, row, state.marks.contains(&entry.path), current, now);
        view.chevrons.push(chevron);
        if let Some(edit) = view.edit.as_ref().filter(|e| e.row == Some(index)) {
            if edit.kind == EditKind::Rename {
                // The name, typed over in place.
                draw_field(&mut g, line, chevron + 4, left + width, edit, false);
            } else {
                draw_field(&mut g, line + 1, new_row_x(left, view.form, edit.depth), left + width, edit, true);
                extra += 1;
            }
        }
        if current && index != state.selected {
            // The file being edited, tinted wherever the selection is.
            g.panel(line, left..left + width);
        }
        if state.marks.contains(&entry.path) {
            g.panel(line, left..left + width);
        }
        if index == state.selected && view.edit.is_none() {
            g.focus(line, left..left + width);
        }
        if index == state.selected {
            if let Some(rows) = &view.menu {
                g.popup = Some(Popup { line, col: chevron + 4, rows: rows.clone() });
            }
        }
    }
    // Rows the listing doesn't fill still count, so the strip sits at
    // the bottom of the pane.
    if let Some(h) = height {
        g.reserve(h.saturating_sub(foot));
    }
    g.keys(left, width, strip(view.form));
    g.finish()
}

/// One row of the tree; the column its chevron is at.
#[allow(clippy::too_many_arguments)]
fn draw_row(g: &mut Grid, line: usize, left: usize, width: usize, form: Form, entry: &Entry, row: &Row, marked: bool, current: bool, now: SystemTime) -> usize {
    let right = left + width;
    // The columns at the right, right to left: the status letter, then
    // (in a pane) size and age.
    let columns = form != Form::Sidebar && width >= 48;
    let (size_w, age_w) = (7, 6);
    let status_x = right.saturating_sub(1);
    let name_end = if columns { status_x.saturating_sub(size_w + age_w + 3) } else { status_x.saturating_sub(1) };

    let mut x = left;
    if form != Form::Sidebar {
        x = g.put(line, x, if marked { "● " } else { "  " }, Role::Accent);
    }
    for (level, &continues) in row.guides.iter().enumerate() {
        // The folder this row is in always reaches it; the ones above
        // only while they have more to come.
        let guide = if continues || level + 1 == row.guides.len() { "│ " } else { "  " };
        x = g.put(line, x, guide, Role::Muted);
    }
    let chevron = match (entry.is_dir(), row.expanded) {
        (true, true) => "▾ ",
        (true, false) => "▸ ",
        (false, _) => "  ",
    };
    let chevron_at = x;
    x = g.put(line, x, chevron, Role::Muted);
    let icon = if entry.is_dir() { crate::icon::icon_for(&entry.name, true, row.expanded) } else { crate::icon::file_icon(&entry.name) };
    let icon_role = if entry.is_dir() { Role::Accent } else { Role::Muted };
    x = g.put(line, x, &format!("{icon} "), icon_role);
    let mut label = row.label.clone();
    if entry.kind.is_link() {
        label.push_str(" →");
    }
    let room = name_end.saturating_sub(x);
    if room > 0 {
        // The file being edited stands out in the accent colour.
        let role = if current { Role::Accent } else { name_role(entry) };
        let shown = fit_middle(&label, room);
        g.put(line, x, &shown, role);
        // What the filter matched, underlined -- where the name is shown
        // whole.
        if let Some(hit) = row.hit.clone().filter(|h| !h.is_empty() && shown == label) {
            g.underline(line, x + hit.start..x + hit.end);
        }
    }
    if columns {
        let size = if entry.is_dir() { String::new() } else { size_text(entry.size) };
        let at = name_end + 1;
        g.put(line, at + size_w - size.chars().count().min(size_w), &size, Role::Muted);
        let age = age_text(entry.modified, now);
        let at = at + size_w + 1;
        g.put(line, at + age_w - age.chars().count().min(age_w), &age, Role::Muted);
    }
    if let Some((mark, role)) = status_mark(entry) {
        g.put(line, status_x, mark, role);
    }
    chevron_at
}

/// Where a new row's name starts at `depth`: past the mark column, the
/// guides, a chevron's room and the icon.
fn new_row_x(left: usize, form: Form, depth: usize) -> usize {
    left + if form == Form::Sidebar { 0 } else { 2 } + depth * 2 + 2
}

/// A name being typed, at (`line`, `x`) up to `end`: the text with its
/// caret, the selection in the accent colour, on a panel -- and, for a
/// new row, the icon of what it will be first.
fn draw_field(g: &mut Grid, line: usize, x: usize, end: usize, edit: &RowEdit, new: bool) {
    let mut x = x;
    if new {
        let icon = if edit.kind == EditKind::NewFolder { crate::icon::icon_for("", true, false) } else { crate::icon::file_icon(&edit.text) };
        x = g.put(line, x, &format!("{icon} "), Role::Accent);
    }
    let room = end.saturating_sub(x).max(2);
    let shown = with_caret(&edit.text, edit.caret, room);
    // Clear what was drawn there (a rename's old name) before the field.
    g.put(line, x, &" ".repeat(room), Role::Text);
    g.put(line, x, &shown, Role::Title);
    g.panel(line, x..end);
    if let Some(sel) = edit.selected.clone().filter(|s| !s.is_empty() && shown.chars().count() == edit.text.chars().count() + 1) {
        // The caret sits at the selection's end, so the selection is
        // where it was typed.
        let piece: String = edit.text.chars().skip(sel.start).take(sel.len()).collect();
        g.put(line, x + sel.start, &piece, Role::Accent);
    }
}

/// `?`: every key, a section at a time.
fn layout_help(g: &mut Grid, mut y: usize, left: usize, width: usize) {
    let key_w = 10.min(width / 2);
    for (section, keys) in HELP {
        g.heading(y, left, width, section);
        y += 1;
        for (key, what) in keys.iter() {
            g.put(y, left, &fit(key, key_w), Role::Accent);
            g.put(y, left + key_w + 1, &fit(what, width.saturating_sub(key_w + 1)), Role::Text);
            y += 1;
        }
        y += 1;
    }
}

/// `scroll` moved as little as keeps row `at` among the `rows` shown.
fn scroll_to_include(scroll: usize, at: usize, rows: usize) -> usize {
    let rows = rows.max(1);
    if at < scroll {
        at
    } else if at >= scroll + rows {
        at + 1 - rows
    } else {
        scroll
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fenix_keymap::{KeyCode, KeyPress, NamedKey};
    use std::time::Duration;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("fenix-explorer-page-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_long_name_keeps_its_start_and_its_extension() {
        assert_eq!(fit_middle("main.rs", 10), "main.rs");
        let cut = fit_middle("explorer_host_implementation.rs", 16);
        assert_eq!(cut.chars().count(), 16);
        assert!(cut.starts_with("explorer_h") && cut.ends_with("on.rs") && cut.contains('…'), "{cut}");
        let plain = fit_middle("averyveryverylongname", 10);
        assert_eq!(plain.chars().count(), 10);
        assert!(plain.starts_with("avery") && plain.contains('…'), "{plain}");
    }

    #[test]
    fn sizes_and_ages_read_like_people_say_them() {
        assert_eq!(size_text(512), "512 B");
        assert_eq!(size_text(1126), "1.1 K");
        assert_eq!(size_text(37 * 1024 * 1024), "37 M");
        let now = SystemTime::now();
        assert_eq!(age_text(Some(now - Duration::from_secs(37 * 60)), now), "37 min");
        assert_eq!(age_text(Some(now - Duration::from_secs(5 * 3600)), now), "5 h");
        assert_eq!(age_text(Some(now - Duration::from_secs(3 * 86400)), now), "3 d");
        assert_eq!(age_text(None, now), "");
    }

    #[test]
    fn the_breadcrumb_starts_at_the_project() {
        let project = Project { name: "fenix".into(), root: PathBuf::from("/src/fenix"), kind: ProjectKind::Rust };
        let names = |crumbs: Vec<(String, PathBuf)>| crumbs.into_iter().map(|(n, _)| n).collect::<Vec<_>>();
        let crumbs = breadcrumb(Path::new("/src/fenix/crates/gui"), Some(&project));
        assert_eq!(crumbs[1].1, PathBuf::from("/src/fenix/crates"), "each segment names its folder");
        assert_eq!(names(crumbs), vec!["fenix", "crates", "gui"]);
        assert_eq!(names(breadcrumb(Path::new("/elsewhere/x"), Some(&project))), vec!["elsewhere", "x"]);
    }

    #[test]
    fn a_pane_shows_the_tree_with_its_columns_and_the_key_strip_at_the_bottom() {
        let dir = temp("pane");
        std::fs::create_dir(dir.join("src")).unwrap();
        std::fs::write(dir.join("src").join("main.rs"), "fn main() {}").unwrap();
        std::fs::write(dir.join("README.md"), "x".repeat(1126)).unwrap();
        let mut state = ExplorerState::opened(&dir).unwrap();
        state.toggle_expand().unwrap();
        let mut view = View::new(Form::Pane);
        let page = layout(Some(&state), &mut view, 80, Some(20));
        let lines: Vec<&str> = page.text.lines().collect();

        assert!(lines[1].contains("2 items"), "{}", page.text);
        let src = lines.iter().position(|l| l.contains("▾") && l.contains("src")).expect("the open folder");
        assert!(lines[src + 1].contains("│") && lines[src + 1].contains("main.rs"), "{}", page.text);
        let readme = lines.iter().find(|l| l.contains("README.md")).unwrap();
        assert!(readme.contains("1.1 K") && readme.contains("now"), "{readme}");
        assert_eq!(lines.len(), 20, "the strip is on the pane's last line:\n{}", page.text);
        assert!(lines[19].contains("Enter open"));
        assert_eq!(view.scroll, 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_sidebar_has_names_and_status_but_no_columns() {
        let dir = temp("sidebar");
        std::fs::write(dir.join("a.txt"), "hello").unwrap();
        let state = ExplorerState::opened(&dir).unwrap();
        let mut view = View::new(Form::Sidebar);
        let page = layout(Some(&state), &mut view, 30, Some(12));
        let row = page.text.lines().find(|l| l.contains("a.txt")).unwrap();
        assert!(!row.contains(" B"), "no size column: {row}");
        assert!(page.text.lines().all(|l| l.chars().count() <= 30), "{}", page.text);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_long_listing_scrolls_to_keep_the_selection_shown() {
        let dir = temp("scroll");
        for i in 0..40 {
            std::fs::write(dir.join(format!("f{i:02}.txt")), "").unwrap();
        }
        let mut state = ExplorerState::opened(&dir).unwrap();
        state.select_index(35);
        let mut view = View::new(Form::Pane);
        let page = layout(Some(&state), &mut view, 80, Some(15));
        assert!(page.text.contains("f35.txt") && !page.text.contains("f00.txt"), "{}", page.text);
        let (line, _) = page.focus.clone().unwrap();
        assert_eq!(view.scroll + line - view.rows_at, 35);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn no_row_is_wider_than_the_pane_and_a_narrow_one_keeps_names_over_columns() {
        let dir = temp("widths");
        std::fs::write(dir.join("a-fairly-long-file-name-here.txt"), "0123456789").unwrap();
        std::fs::create_dir(dir.join("a-directory-with-a-long-name")).unwrap();
        std::fs::write(dir.join("twenty-char-name.txt"), "x").unwrap();
        let mut state = ExplorerState::opened(&dir).unwrap();
        state.toggle_mark();
        let statuses = state.entries.iter().map(|e| (e.path.clone(), GitStatus::Conflicted)).collect();
        state.apply_git_statuses(statuses);
        for (form, width) in [(Form::Sidebar, 24), (Form::Pane, 30), (Form::Pane, 60), (Form::Pane, 120)] {
            let page = layout(Some(&state), &mut View::new(form), width, Some(20));
            for row in page.text.lines() {
                assert!(row.chars().count() <= width, "a {}-char row {width} wide: {row:?}", row.chars().count());
            }
        }
        let page = layout(Some(&state), &mut View::new(Form::Pane), 30, Some(20));
        assert!(page.text.contains("twenty-char-name.txt"), "no size column squeezing it:\n{}", page.text);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_marked_row_says_so_down_the_left_edge() {
        let dir = temp("marked");
        std::fs::write(dir.join("a.txt"), "").unwrap();
        let mut state = ExplorerState::opened(&dir).unwrap();
        state.toggle_mark();
        let page = layout(Some(&state), &mut View::new(Form::Pane), 60, Some(12));
        let row = page.text.lines().find(|l| l.contains("a.txt")).unwrap();
        assert!(row.trim_start().starts_with('●'), "{row:?}");
        assert!(page.text.lines().nth(1).unwrap().contains("1 marked"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_empty_folder_and_a_slow_one_say_so() {
        let dir = temp("empty");
        let state = ExplorerState::opened(&dir).unwrap();
        let mut view = View::new(Form::Pane);
        view.waiting_on = Some(PathBuf::from(r"\\nas\media"));
        let page = layout(Some(&state), &mut view, 80, Some(12));
        assert!(page.text.contains("empty") && page.text.contains("0 items"), "{}", page.text);
        assert!(page.text.contains(r"reading \\nas\media (Esc stops)"), "{}", page.text);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_changed_file_has_its_letter_and_a_folder_with_changes_a_dot() {
        let dir = temp("status");
        std::fs::create_dir(dir.join("sub")).unwrap();
        std::fs::write(dir.join("sub").join("x.rs"), "").unwrap();
        std::fs::write(dir.join("y.rs"), "").unwrap();
        let mut state = ExplorerState::opened(&dir).unwrap();
        let statuses = [(dir.join("sub").join("x.rs"), GitStatus::Modified), (dir.join("y.rs"), GitStatus::Untracked)].into_iter().collect();
        state.apply_git_statuses(statuses);
        let page = layout(Some(&state), &mut View::new(Form::Sidebar), 30, Some(12));
        let sub = page.text.lines().find(|l| l.contains("sub")).unwrap();
        let y = page.text.lines().find(|l| l.contains("y.rs")).unwrap();
        assert!(sub.ends_with('•') && y.ends_with('U'), "the status column lines up at the right:\n{}", page.text);
        assert_eq!(sub.chars().count(), y.chars().count());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_wheel_scrolls_without_moving_the_selection_until_it_moves() {
        let dir = temp("wheel");
        for i in 0..40 {
            std::fs::write(dir.join(format!("f{i:02}.txt")), "").unwrap();
        }
        let mut state = ExplorerState::opened(&dir).unwrap();
        let mut view = View::new(Form::Pane);
        layout(Some(&state), &mut view, 80, Some(15));
        view.scroll_by(10);
        let page = layout(Some(&state), &mut view, 80, Some(15));
        assert_eq!(view.scroll, 10, "the selection didn't move, so the scroll stays");
        assert!(page.focus.is_none() && page.text.contains("f10.txt"));
        view.scroll_by(1000);
        layout(Some(&state), &mut view, 80, Some(15));
        assert!(view.scroll <= 40, "clamped to the listing");
        state.move_selection(1);
        let page = layout(Some(&state), &mut view, 80, Some(15));
        let (line, _) = page.focus.expect("a moved selection is brought back into view");
        assert_eq!(view.row_at(line), Some(1));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_click_finds_its_row_its_chevron_and_its_breadcrumb_folder() {
        let dir = temp("clicks");
        std::fs::create_dir(dir.join("sub")).unwrap();
        std::fs::write(dir.join("a.txt"), "").unwrap();
        let state = ExplorerState::opened(&dir).unwrap();
        let mut view = View::new(Form::Pane);
        let page = layout(Some(&state), &mut view, 80, Some(12));
        let line = page.text.lines().position(|l| l.contains("sub")).unwrap();
        let chevron = page.text.lines().nth(line).unwrap().chars().position(|c| c == '▸').unwrap();
        assert_eq!(view.row_at(line), Some(0));
        assert!(view.on_chevron(line, chevron) && !view.on_chevron(line, chevron + 4));
        let last = view.crumbs.last().unwrap().clone();
        assert_eq!(last.1, dir, "the last segment is the folder shown");
        let parent = &view.crumbs[view.crumbs.len() - 2];
        assert_eq!(view.crumb_at(parent.0.start), dir.parent(), "a segment before it goes up");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_file_being_edited_is_tinted() {
        let dir = temp("current");
        std::fs::write(dir.join("a.txt"), "").unwrap();
        std::fs::write(dir.join("b.txt"), "").unwrap();
        let state = ExplorerState::opened(&dir).unwrap();
        let mut view = View::new(Form::Sidebar);
        view.current = Some(dir.join("b.txt"));
        let page = layout(Some(&state), &mut view, 30, Some(12));
        let line = page.text.lines().position(|l| l.contains("b.txt")).unwrap();
        assert!(page.panels.iter().any(|(l, _)| *l == line), "a tint on b.txt's row");
        let col = page.text.lines().nth(line).unwrap().find("b.txt").map(|b| page.text.lines().nth(line).unwrap()[..b].chars().count()).unwrap();
        assert!(page.spans.iter().any(|s| s.line == line && s.cols.start == col && s.role == Role::Accent));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn help_names_every_key_the_table_binds() {
        let help: Vec<&str> = HELP.iter().flat_map(|(_, keys)| keys.iter().map(|(k, _)| *k)).collect();
        let named = |key: &KeyPress| match key.code {
            KeyCode::Char(c) => c.to_string(),
            KeyCode::Named(NamedKey::Enter) => "Enter".into(),
            KeyCode::Named(NamedKey::Tab) => "Tab".into(),
            KeyCode::Named(NamedKey::Escape) => "Esc".into(),
            KeyCode::Named(NamedKey::Down) => "↓".into(),
            KeyCode::Named(NamedKey::Up) => "↑".into(),
            other => format!("{other:?}"),
        };
        for (keys, _) in fenix_explorer::explorer_trie().bindings() {
            let name = keys.iter().map(named).collect::<Vec<_>>().join(" ");
            let said = if keys.len() == 1 { help.iter().any(|h| h.split_whitespace().any(|w| w == name)) } else { help.iter().any(|h| h.contains(&name) || h.contains(&name.replace(' ', ""))) };
            assert!(said, "{name} is bound but `?` doesn't say so");
        }
    }
}
