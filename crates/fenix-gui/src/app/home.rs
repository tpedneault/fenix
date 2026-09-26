//! Home, the start-up dashboard (see `dashboard.rs` for its layout):
//! gathering what it shows, laying it out for each pane that shows it,
//! the keys that move around it, and what the renderer draws on top of
//! its text -- the panels, the focus rail and the logo.

use super::*;
use crate::dashboard::{self, Fill, HomeData, HomeEntry, Role};

/// The focus rail's width: the same 2 px rail the modeline uses.
pub(super) const RAIL_PX: f32 = 2.0;

/// The logo lockup drawn on Home: the image, what it was drawn for (so
/// it's redrawn when the size or text colour changes), and its GPU copy.
pub(super) struct HomeLogo {
    pub(super) key: (u32, [u8; 3]),
    pub(super) image: fenix_brand::Image,
    pub(super) texture: Option<PdfTexture>,
}

/// Where Home's extra drawing goes in one pane, in that pane's rows and
/// cells -- carried on `PaneRender`.
#[derive(Default)]
pub(super) struct HomeOverlay {
    /// (row, first cell, row count) for the selected slot's focus rail.
    pub(super) rail: Option<(usize, usize, usize)>,
    /// (row, cell, rows tall) for the logo lockup.
    pub(super) logo: Option<(usize, usize, usize)>,
    /// (row, first cell, end cell) for each visible rule.
    pub(super) rules: Vec<(usize, usize, usize)>,
    pub(super) rule_color: [f32; 4],
}

/// (row, first cell, end cell, colour) -- the shape `PaneRender`'s
/// `colored_bg_segments` takes.
pub(super) type BgSegments = Vec<(usize, usize, usize, [f32; 4])>;

/// "now", "5 min", "3 h", "2 d", "6 w".
fn age(modified: Option<std::time::SystemTime>) -> String {
    let Some(elapsed) = modified.and_then(|m| m.elapsed().ok()) else { return String::new() };
    let mins = elapsed.as_secs() / 60;
    match mins {
        0 => "now".to_string(),
        1..=59 => format!("{mins} min"),
        60..=1439 => format!("{} h", mins / 60),
        1440..=20159 => format!("{} d", mins / 1440),
        _ => format!("{} w", mins / 10080),
    }
}

use fenix_project::vcs::git_branch;

fn format_elapsed(duration: chrono::Duration) -> String {
    let minutes = duration.num_minutes().max(0);
    format!("{}:{:02}", minutes / 60, minutes % 60)
}

impl App {
    /// Opens a Home buffer (empty until a pane first lays it out at its
    /// real size) -- the one way every path that needs one gets it.
    pub(super) fn new_home_buffer(&mut self) -> BufferId {
        self.refresh_home_data(true);
        self.buffers.open_dashboard("")
    }

    /// Re-gathers everything Home shows. The project TODO scan reads
    /// files, so it only runs when `with_todos` (Home opening, the window
    /// regaining focus); the minute tick keeps the last scan.
    pub(super) fn refresh_home_data(&mut self, with_todos: bool) {
        let now = chrono::Local::now();
        let (resume, recent) = self.home_recent_files(None);
        let probe = self.app_probe();
        for root in self.known_projects.roots().iter().take(5) {
            if !self.project_health.contains_key(root) && root.is_dir() {
                let checks = fenix_project::doctor::diagnose(root, self.project_kind_of(root), &probe, false);
                let health = (fenix_project::doctor::worst(&checks), checks.iter().filter(|c| c.health >= fenix_project::doctor::Health::Warn).count());
                self.project_health.insert(root.clone(), health);
            }
        }
        let mut projects: Vec<dashboard::ProjectItem> = self
            .known_projects
            .roots()
            .iter()
            .take(5)
            .map(|root| dashboard::ProjectItem {
                root: root.clone(),
                name: root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| root.display().to_string()),
                branch: git_branch(root),
                kind: self.project_kind_of(root),
                health: None,
            })
            .collect();
        for item in &mut projects {
            item.health = self.project_health.get(&item.root).map(|(health, _)| *health);
        }

        // Open tasks the way the agenda's Today puts them: the one on the
        // clock, then in progress, then what's due within a week, then
        // what's ready to start. Waiting and blocked ones stay on the page.
        let live = self.agenda_store.active_timer.as_ref().map(|t| (t.task_id, now.signed_duration_since(t.started_at)));
        let week = now.date_naive() + chrono::Duration::days(7);
        let store = &self.agenda_store;
        let mut open: Vec<&fenix_agenda::Task> = store
            .tasks
            .iter()
            .filter(|t| !t.archived && t.status != fenix_agenda::Status::Done)
            .filter(|t| t.status == fenix_agenda::Status::InProgress || (t.status == fenix_agenda::Status::Todo && store.is_ready(t.id)) || live.is_some_and(|(id, _)| id == t.id))
            .collect();
        open.sort_by_key(|t| {
            (
                live.is_none_or(|(id, _)| id != t.id),
                t.status != fenix_agenda::Status::InProgress,
                t.due.filter(|d| *d <= week).is_none(),
                t.due,
                std::cmp::Reverse(t.priority),
                t.order,
            )
        });
        let today: Vec<dashboard::TaskItem> = open
            .into_iter()
            .map(|t| dashboard::TaskItem {
                title: match t.jira_key() {
                    Some(key) => format!("{key} {}", t.title),
                    None => t.title.clone(),
                },
                live: live.filter(|(id, _)| *id == t.id).map(|(_, d)| format_elapsed(d)),
                pressing: matches!(t.priority, fenix_agenda::Priority::High | fenix_agenda::Priority::Urgent),
            })
            .collect();

        let todos = if with_todos {
            let root = self.project_root.clone().or_else(|| self.known_projects.roots().first().cloned());
            root.map(|root| self.home_todos(&root)).unwrap_or_default()
        } else {
            self.home_data.todos.clone()
        };

        let date = now.format("%A %-d %B · %H:%M").to_string();
        let recovery = self.unclaimed_snapshot_names().len();
        // A project workspace's Home: the same page, narrowed to the
        // project's own files and TODOs, and named after it.
        let mut scoped = HashMap::new();
        for root in self.home_projects() {
            let (resume, recent) = self.home_recent_files(Some(&root));
            let todos = match self.home_project_data.get(&root) {
                Some(previous) if !with_todos => previous.todos.clone(),
                _ => self.home_todos(&root),
            };
            let name = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| root.display().to_string());
            let data = HomeData {
                date: format!("{name} · {date}"),
                resume,
                recent,
                projects: projects.clone(),
                today: today.clone(),
                todos,
                recovery,
            };
            scoped.insert(root, data);
        }
        let data = HomeData { date, resume, recent, projects, today, todos, recovery };
        if data != self.home_data || scoped != self.home_project_data {
            self.home_data = data;
            self.home_project_data = scoped;
            // Every Home re-lays itself out on its next frame.
            self.home_views.values_mut().for_each(|view| view.size = (0, 0));
        }
    }

    /// What Home `id` shows: its project's page for a project
    /// workspace's Home, the everything page otherwise.
    pub(super) fn home_data_for(&self, id: BufferId) -> &HomeData {
        self.home_project_of(id).and_then(|root| self.home_project_data.get(&root)).unwrap_or(&self.home_data)
    }

    /// The resume slot and the recent files, from anywhere or only from
    /// under `scope`.
    fn home_recent_files(&self, scope: Option<&Path>) -> (Option<dashboard::FileItem>, Vec<dashboard::FileItem>) {
        // Recent files are stored canonical (with Windows' `\\?\` prefix), project
        // roots normalized, so both are compared normalized.
        let under = |p: &PathBuf| scope.is_none_or(|root| fenix_lsp::normalize(p.clone()).starts_with(root));
        let mut recent_files = self.recent_files.paths().iter().filter(|p| p.is_file() && under(p));
        let project_of = |path: &Path| -> Option<(String, Option<String>)> {
            let root = fenix_project::find_project_root(path)?;
            let name = root.file_name()?.to_string_lossy().into_owned();
            Some((name, git_branch(&root)))
        };
        let resume = recent_files.next().map(|path| {
            let detail = match project_of(path) {
                Some((name, Some(branch))) => format!("{name} · {branch}"),
                Some((name, None)) => name,
                None => path.parent().map(|p| p.display().to_string()).unwrap_or_default(),
            };
            dashboard::FileItem {
                path: path.clone(),
                name: path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                detail,
                age: age(std::fs::metadata(path).and_then(|m| m.modified()).ok()),
            }
        });
        let recent = recent_files
            .take(5)
            .map(|path| dashboard::FileItem {
                path: path.clone(),
                name: path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                detail: String::new(),
                age: age(std::fs::metadata(path).and_then(|m| m.modified()).ok()),
            })
            .collect();
        (resume, recent)
    }

    /// The TODO column for `root`, grouped by kind.
    fn home_todos(&self, root: &Path) -> Vec<dashboard::TodoItem> {
        self.collect_project_todos(root)
            .map(|(found, _)| {
                let mut items: Vec<dashboard::TodoItem> = found
                    .into_iter()
                    .map(|(kind, m)| dashboard::TodoItem {
                        kind,
                        message: m.text.split_once(' ').map_or("", |(_, rest)| rest).to_string(),
                        file: m.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                        path: m.path,
                        line: m.line,
                        col: m.col,
                    })
                    .collect();
                items.sort_by_key(|t| t.kind);
                items
            })
            .unwrap_or_default()
    }

    /// The minute tick: the clock and a running timer move on even when
    /// nothing else happens.
    pub(super) fn tick_home(&mut self) {
        if self.home_views.is_empty() {
            return;
        }
        let minute = chrono::Local::now().format("%H:%M").to_string();
        if !self.home_data.date.ends_with(&minute) {
            self.refresh_home_data(false);
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }
    }

    /// Lays buffer `id`'s Home out for a pane `cols` × `rows` cells in
    /// size, unless it already is. The text is rewritten in place and the
    /// cursor put back on whatever it had selected.
    pub(super) fn ensure_home_layout(&mut self, id: BufferId, pane: fenix_window::WindowId, cols: usize, rows: usize) {
        if self.home_views.get(&id).is_some_and(|view| view.size == (cols, rows)) {
            return;
        }
        let selected = self.home_selected_slot(id, pane).and_then(|i| self.home_views.get(&id).map(|v| v.slots[i].entry.clone()));
        let view = dashboard::layout(self.home_data_for(id), cols, rows);
        let Some(ob) = self.buffers.get_mut(id) else { return };
        if ob.buffer.text() != view.text {
            let end = ob.buffer.len_chars();
            let mut scratch = Cursor::at_start();
            ob.buffer.replace_range(&mut scratch, 0, end, &view.text);
            ob.buffer.mark_saved();
            ob.buffer.drain_edits();
        }
        let target = selected.and_then(|entry| view.find(&entry)).or_else(|| view.find(&HomeEntry::Find)).map(|i| view.slots[i].cursor());
        self.home_views.insert(id, view);
        if let Some((line, col)) = target {
            self.home_place_cursor(id, pane, line, col);
        }
    }

    pub(super) fn home_place_cursor(&mut self, id: BufferId, pane: fenix_window::WindowId, line: usize, col: usize) {
        let Some(ob) = self.buffers.get(id) else { return };
        let line = line.min(ob.buffer.line_count().saturating_sub(1));
        let char_idx = ob.buffer.line_start_char(line) + col.min(ob.buffer.line_len(line));
        if let Some(state) = self.workspaces.active_pane_states_mut().get_mut(&pane) {
            state.cursor = Cursor { char_idx, sticky_col: col };
        }
    }

    /// The slot the cursor in `pane` has selected on Home buffer `id`.
    pub(super) fn home_selected_slot(&self, id: BufferId, pane: fenix_window::WindowId) -> Option<usize> {
        let view = self.home_views.get(&id)?;
        let ob = self.buffers.get(id)?;
        let cursor = self.workspaces.active_pane_states().get(&pane)?.cursor;
        let (line, col) = ob.buffer.line_col(&cursor);
        view.slot_at(line, col)
    }

    /// Keys Home claims when its pane is focused and Vim has nothing
    /// pending: `j`/`k` and `h`/`l` (and the arrows) move between slots,
    /// `Enter` activates one, `1`–`9` activate by number. Everything else
    /// (the leader, `:`, `/`) reaches Vim as usual.
    pub(super) fn home_key(&mut self, key: KeyPress) -> bool {
        let id = self.focused_buffer_id();
        if self.open().kind != BufferKind::Dashboard || !self.vim.is_idle() || key.mods != Mods::default() {
            return false;
        }
        let pane = self.focused_pane_id();
        let Some(view) = self.home_views.get(&id) else { return false };
        let current = self.home_selected_slot(id, pane).or_else(|| view.find(&HomeEntry::Find));
        let target = match key.code {
            KeyCode::Named(FenixNamedKey::Enter) => {
                if let Some(i) = current {
                    let entry = view.slots[i].entry.clone();
                    self.activate_home_entry(entry);
                }
                return true;
            }
            KeyCode::Char(c @ '1'..='9') => {
                if let Some(entry) = view.numbered(c as u8 - b'0').map(|i| view.slots[i].entry.clone()) {
                    self.activate_home_entry(entry);
                }
                return true;
            }
            KeyCode::Char('j') | KeyCode::Named(FenixNamedKey::Down) => current.and_then(|i| view.step(i, true)),
            KeyCode::Char('k') | KeyCode::Named(FenixNamedKey::Up) => current.and_then(|i| view.step(i, false)),
            KeyCode::Char('l') | KeyCode::Named(FenixNamedKey::Right) => current.and_then(|i| view.across(i, true)),
            KeyCode::Char('h') | KeyCode::Named(FenixNamedKey::Left) => current.and_then(|i| view.across(i, false)),
            _ => return false,
        };
        if let Some((line, col)) = target.map(|i| view.slots[i].cursor()) {
            self.home_place_cursor(id, pane, line, col);
        }
        self.wake_caret();
        true
    }

    fn activate_home_entry(&mut self, entry: HomeEntry) {
        match entry {
            HomeEntry::Find => self.picker_find_file(),
            HomeEntry::Resume(path) | HomeEntry::RecentFile(path) => self.open_file_from_picker(&path),
            HomeEntry::Project(root) => self.switch_to_project(root),
            HomeEntry::NewProject => self.cmd_project_new(),
            HomeEntry::Agenda => {
                self.open_agenda_page(Some(crate::agenda_page::Tab::Today));
            }
            HomeEntry::Todo { path, line, col } => {
                self.open_file_from_picker(&path);
                self.jump_to_grep_match(&fenix_project::GrepMatch { path, line, col, text: String::new() });
            }
            HomeEntry::Recover => self.picker_recovery(),
        }
    }

    /// Home's colours for the visible lines, in the same shape syntax
    /// highlighting hands the renderer.
    pub(super) fn home_highlights(&self, id: BufferId, first_line: usize, rows: usize) -> Vec<(std::ops::Range<usize>, glyphon::Color)> {
        let (Some(view), Some(ob)) = (self.home_views.get(&id), self.buffers.get(id)) else { return Vec::new() };
        let theme = self.theme;
        let color = |role: Role| match role {
            Role::Title => theme.fg,
            Role::Text => theme.fg,
            Role::Muted => theme.gutter_fg,
            Role::Focus => rgba_to_glyphon(theme.caret),
            Role::Ember => glyphon::Color::rgb(fenix_brand::EMBER[0], fenix_brand::EMBER[1], fenix_brand::EMBER[2]),
            Role::Warn => theme.git_modified,
            Role::Todo(kind) => theme.todo_color(kind),
            Role::Kind(kind) => super::projects::kind_color(kind, theme),
            Role::Health(health) => match health {
                fenix_project::doctor::Health::Ok => theme.git_staged,
                fenix_project::doctor::Health::Info => theme.gutter_fg,
                fenix_project::doctor::Health::Warn => theme.git_modified,
                fenix_project::doctor::Health::Bad => theme.git_conflicted,
            },
        };
        let last = first_line + rows;
        let mut ranges: Vec<(std::ops::Range<usize>, glyphon::Color)> = view
            .spans
            .iter()
            .filter(|s| s.line >= first_line && s.line <= last && s.line < ob.buffer.line_count())
            .map(|s| {
                let start = ob.buffer.line_start_char(s.line);
                let len = ob.buffer.line_len(s.line);
                let a = ob.buffer.char_to_byte(start + s.cols.start.min(len));
                let b = ob.buffer.char_to_byte(start + s.cols.end.min(len));
                (a..b, color(s.role))
            })
            .filter(|(r, _)| r.start < r.end)
            .collect();
        ranges.sort_by_key(|(r, _)| r.start);
        ranges
    }

    /// Home's backgrounds for a pane whose first visible line is
    /// `first_line`: panels and keycaps, the selected slot's tint, and
    /// where the focus rail and logo go.
    pub(super) fn home_backgrounds(&self, id: BufferId, pane: fenix_window::WindowId, first_line: usize, rows: usize) -> (BgSegments, HomeOverlay) {
        let Some(view) = self.home_views.get(&id) else { return (Vec::new(), HomeOverlay::default()) };
        let theme = self.theme;
        let panel = if theme.sidebar_bg != theme.bg { theme.sidebar_bg } else { theme.bg_modeline };
        let key = if theme.bg_modeline != panel { theme.bg_modeline } else { home_rule(theme) };
        let visible = |line: usize| (line >= first_line && line <= first_line + rows).then(|| line - first_line);
        let mut segments = Vec::new();
        let selected = self.home_selected_slot(id, pane);
        let mut overlay = HomeOverlay::default();
        if let Some(slot) = selected.map(|i| &view.slots[i]) {
            // The find field keeps its panel; everything else takes the
            // editor's current-line tint.
            if slot.entry != HomeEntry::Find {
                for line in slot.line..slot.line + slot.height {
                    if let Some(row) = visible(line) {
                        segments.push((row, slot.cols.start, slot.cols.end, theme.hl_line));
                    }
                }
            }
            overlay.rail = visible(slot.line).map(|row| (row, slot.cols.start, slot.height));
        }
        for p in &view.panels {
            if let Some(row) = visible(p.line) {
                let fill = match p.fill {
                    Fill::Panel => panel,
                    Fill::Key => key,
                };
                segments.push((row, p.cols.start, p.cols.end, fill));
            }
        }
        // Keycaps sit on top of their panel.
        segments.sort_by_key(|s| (s.3 == key) as u8);
        overlay.logo = view.logo.and_then(|logo| visible(logo.line).map(|row| (row, logo.col, logo.lines)));
        overlay.rules = view.rules.iter().filter_map(|r| visible(r.line).map(|row| (row, r.cols.start, r.cols.end))).collect();
        overlay.rule_color = home_rule(theme);
        (segments, overlay)
    }
}

/// The rule colour: the theme's divider, unless it's the same as the
/// background (or the panels) and so wouldn't show -- then a quarter of
/// the text colour mixed into the background. (A lighter mix read as
/// near-black once drawn as thin line glyphs.)
pub(super) fn home_rule(theme: &Theme) -> [f32; 4] {
    if theme.divider != theme.bg && theme.divider != theme.bg_modeline {
        return theme.divider;
    }
    let fg = glyphon_to_rgba(theme.fg);
    let mix = |a: f32, b: f32| a * 0.75 + b * 0.25;
    [mix(theme.bg[0], fg[0]), mix(theme.bg[1], fg[1]), mix(theme.bg[2], fg[2]), 1.0]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ages_read_the_way_the_design_writes_them() {
        let ago = |secs: u64| Some(std::time::SystemTime::now() - std::time::Duration::from_secs(secs));
        assert_eq!(age(ago(5)), "now");
        assert_eq!(age(ago(5 * 60)), "5 min");
        assert_eq!(age(ago(3 * 3600)), "3 h");
        assert_eq!(age(ago(2 * 86400)), "2 d");
        assert_eq!(age(None), "");
    }

    #[test]
    fn elapsed_time_is_hours_and_minutes() {
        assert_eq!(format_elapsed(chrono::Duration::seconds(18 * 60 + 6)), "0:18");
        assert_eq!(format_elapsed(chrono::Duration::minutes(125)), "2:05");
    }
}

