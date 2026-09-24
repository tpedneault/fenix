//! `SPC p p`, the project hub: every known project with what it is
//! (its kind tag) and how it's doing (branch, uncommitted changes, a
//! health dot from the doctor's quick pass), pinned ones first, then by
//! group, most recently used first within each. The right column
//! previews the selected project -- health, git, linked Jira work,
//! tasks. Typing filters, the way the old switcher did, so muscle memory
//! survives: `/` (or just starting to type a path) filters, `Enter`
//! opens.

use std::path::{Path, PathBuf};

use crate::page::{fit, frame, wrap, Grid, Key, Page, Role};
use fenix_project::doctor::Health;
use fenix_project::vcs::GitSummary;
use fenix_project::ProjectKind;

#[derive(Debug, Clone, PartialEq)]
pub struct HubProject {
    pub root: PathBuf,
    pub name: String,
    pub kind: ProjectKind,
    pub pinned: bool,
    pub group: Option<String>,
    pub branch: Option<String>,
    /// Filled in off the UI thread; `None` until then (or not a repo).
    pub git: Option<GitSummary>,
    /// "1 h" since the last commit, worked out when `git` arrives.
    pub last_commit_age: Option<String>,
    /// The doctor's quick pass: worst health and how many problems.
    pub health: Option<(Health, usize)>,
    pub jira: Option<String>,
    /// Open agenda tasks linked to the project's Jira key.
    pub work: Vec<String>,
    pub tasks: Vec<String>,
    /// Whether the folder is still there.
    pub exists: bool,
}

impl HubProject {
    pub fn new(root: PathBuf, kind: ProjectKind) -> Self {
        let name = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| root.display().to_string());
        let exists = root.is_dir();
        HubProject {
            root,
            name,
            kind,
            pinned: false,
            group: None,
            branch: None,
            git: None,
            last_commit_age: None,
            health: None,
            jira: None,
            work: Vec::new(),
            tasks: Vec::new(),
            exists,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    None,
    Close,
    Open(PathBuf),
    /// A folder typed into the filter that isn't a project yet.
    Add(PathBuf),
    /// `a`: browse for one.
    Browse,
    New,
    Doctor(PathBuf),
    Settings(PathBuf),
    TogglePin(PathBuf),
    SetGroup(PathBuf, String),
    Remove(PathBuf),
}

pub struct Hub {
    /// In most-recently-used order.
    pub projects: Vec<HubProject>,
    pub filter: String,
    /// The filter has the keyboard.
    pub filtering: bool,
    pub kind_filter: Option<ProjectKind>,
    /// Index into `rows()`.
    pub focus: usize,
    /// A group name being typed for the focused project.
    pub editing_group: Option<String>,
    /// `d` once asks; `d` again removes.
    pub confirm_remove: Option<PathBuf>,
}

/// A fuzzy subsequence match, case-insensitive: every char of `needle`
/// appears in `hay` in order.
fn matches(hay: &str, needle: &str) -> bool {
    let mut hay = hay.chars().flat_map(char::to_lowercase);
    needle.chars().flat_map(char::to_lowercase).filter(|c| !c.is_whitespace()).all(|n| hay.any(|h| h == n))
}

impl Hub {
    pub fn new(projects: Vec<HubProject>) -> Self {
        Hub { projects, filter: String::new(), filtering: false, kind_filter: None, focus: 0, editing_group: None, confirm_remove: None }
    }

    /// Project indices in display order, with the heading each starts
    /// under: pinned first, then groups alphabetically, then the rest.
    pub fn rows(&self) -> Vec<(String, usize)> {
        let filter = self.filter.trim();
        let visible = |p: &HubProject| {
            self.kind_filter.is_none_or(|k| p.kind == k)
                && (filter.is_empty() || matches(&p.name, filter) || matches(&p.root.display().to_string(), filter) || p.group.as_deref().is_some_and(|g| matches(g, filter)))
        };
        let mut rows: Vec<(String, usize)> = Vec::new();
        rows.extend(self.projects.iter().enumerate().filter(|(_, p)| p.pinned && visible(p)).map(|(i, _)| ("Pinned".to_string(), i)));
        let mut groups: Vec<&str> = self.projects.iter().filter(|p| !p.pinned).filter_map(|p| p.group.as_deref()).collect();
        groups.sort_unstable();
        groups.dedup();
        for group in groups {
            rows.extend(self.projects.iter().enumerate().filter(|(_, p)| !p.pinned && p.group.as_deref() == Some(group) && visible(p)).map(|(i, _)| (group.to_string(), i)));
        }
        let heading = if rows.is_empty() { "Projects" } else { "Ungrouped" };
        rows.extend(self.projects.iter().enumerate().filter(|(_, p)| !p.pinned && p.group.is_none() && visible(p)).map(|(i, _)| (heading.to_string(), i)));
        rows
    }

    pub fn selected(&self) -> Option<&HubProject> {
        self.rows().get(self.focus).map(|(_, i)| &self.projects[*i])
    }

    /// Kinds present, for Tab to cycle through.
    fn kinds(&self) -> Vec<ProjectKind> {
        let mut kinds: Vec<ProjectKind> = Vec::new();
        for p in &self.projects {
            if !kinds.contains(&p.kind) {
                kinds.push(p.kind);
            }
        }
        kinds.sort_by_key(|k| ProjectKind::ALL.iter().position(|x| x == k));
        kinds
    }

    fn clamp_focus(&mut self) {
        self.focus = self.focus.min(self.rows().len().saturating_sub(1));
    }

    /// Keeps the selection on `root` across a change that reorders rows.
    pub fn focus_root(&mut self, root: &Path) {
        if let Some(i) = self.rows().iter().position(|(_, i)| self.projects[*i].root == root) {
            self.focus = i;
        }
    }

    pub fn key(&mut self, key: Key) -> Action {
        if let Some(group) = &mut self.editing_group {
            match key {
                Key::Escape => self.editing_group = None,
                Key::Enter => {
                    let group = std::mem::take(group);
                    self.editing_group = None;
                    if let Some(root) = self.selected().map(|p| p.root.clone()) {
                        return Action::SetGroup(root, group);
                    }
                }
                Key::Backspace => {
                    group.pop();
                }
                Key::Char(c) => group.push(c),
                Key::Space => group.push(' '),
                _ => {}
            }
            return Action::None;
        }
        if self.filtering {
            match key {
                Key::Escape => {
                    self.filtering = false;
                    self.filter.clear();
                }
                Key::Enter => {
                    self.filtering = false;
                    return self.open();
                }
                Key::Backspace => {
                    self.filter.pop();
                }
                Key::Down | Key::Tab => self.focus = (self.focus + 1).min(self.rows().len().saturating_sub(1)),
                Key::Up | Key::BackTab => self.focus = self.focus.saturating_sub(1),
                Key::Char(c) => self.filter.push(c),
                Key::Space => self.filter.push(' '),
                _ => {}
            }
            if !matches!(key, Key::Down | Key::Up | Key::Tab | Key::BackTab) {
                self.focus = 0;
            }
            return Action::None;
        }
        let root = self.selected().map(|p| p.root.clone());
        let pending_remove = self.confirm_remove.take();
        match key {
            Key::Char('q') => return Action::Close,
            Key::Escape if !self.filter.is_empty() || self.kind_filter.is_some() => {
                self.filter.clear();
                self.kind_filter = None;
            }
            Key::Escape => return Action::Close,
            Key::Char('j') | Key::Down => self.focus = (self.focus + 1).min(self.rows().len().saturating_sub(1)),
            Key::Char('k') | Key::Up => self.focus = self.focus.saturating_sub(1),
            Key::Char('g') => {
                if let Some(p) = self.selected() {
                    self.editing_group = Some(p.group.clone().unwrap_or_default());
                }
            }
            Key::Char('G') => self.focus = self.rows().len().saturating_sub(1),
            Key::Char('/') | Key::Char('i') => self.filtering = true,
            // A path typed straight in: start filtering with it.
            Key::Char(c @ ('~' | '.' | '\\')) => {
                self.filtering = true;
                self.filter.push(c);
            }
            Key::Tab | Key::BackTab => {
                let kinds = self.kinds();
                let at = self.kind_filter.and_then(|k| kinds.iter().position(|x| *x == k));
                self.kind_filter = match (at, key == Key::Tab) {
                    (None, true) => kinds.first().copied(),
                    (None, false) => kinds.last().copied(),
                    (Some(i), true) => kinds.get(i + 1).copied(),
                    (Some(i), false) => i.checked_sub(1).and_then(|i| kinds.get(i).copied()),
                };
                self.focus = 0;
            }
            Key::Enter => return self.open(),
            Key::Char('c') => return Action::New,
            Key::Char('a') => return Action::Browse,
            Key::Char('h') => return root.map(Action::Doctor).unwrap_or(Action::None),
            Key::Char(',') => return root.map(Action::Settings).unwrap_or(Action::None),
            Key::Char('P') => return root.map(Action::TogglePin).unwrap_or(Action::None),
            Key::Char('d') => {
                if let Some(root) = root {
                    if pending_remove.as_ref() == Some(&root) {
                        return Action::Remove(root);
                    }
                    self.confirm_remove = Some(root);
                }
            }
            Key::Char(c) if c.is_alphanumeric() => {
                // Typing a name straight away filters, as the old picker did.
                self.filtering = true;
                self.filter.push(c);
                self.focus = 0;
            }
            _ => {}
        }
        self.clamp_focus();
        Action::None
    }

    fn open(&mut self) -> Action {
        if let Some(p) = self.selected() {
            return Action::Open(p.root.clone());
        }
        let typed = self.filter.trim();
        let path = match typed.strip_prefix('~') {
            Some(rest) => dirs::home_dir().map(|h| h.join(rest.trim_start_matches(['/', '\\']))),
            None => Some(PathBuf::from(typed)),
        };
        match path {
            Some(path) if !typed.is_empty() && path.is_dir() => {
                self.filter.clear();
                Action::Add(path)
            }
            _ => Action::None,
        }
    }
}

fn health_role(health: Health) -> Role {
    match health {
        Health::Ok => Role::Good,
        Health::Info => Role::Muted,
        Health::Warn => Role::Warn,
        Health::Bad => Role::Bad,
    }
}

pub fn layout(hub: &Hub, cols: usize) -> Page {
    let (left, width) = frame(cols, 120);
    let mut g = Grid::new();
    let end = g.put(1, left, "Projects", Role::Title);
    g.put(1, end + 2, &hub.projects.len().to_string(), Role::Muted);
    g.rule(2, left..left + width);

    let two_columns = width >= 90;
    let list_width = if two_columns { width * 11 / 20 } else { width };

    // Filter field and kind chips.
    let mut y = 4;
    g.put(y, left, "›", Role::Accent);
    let field = if hub.filtering {
        format!("{}▏", hub.filter)
    } else if hub.filter.is_empty() {
        "type to filter, or paste a folder to add".to_string()
    } else {
        hub.filter.clone()
    };
    let end = g.put(y, left + 2, &fit(&field, list_width.saturating_sub(4)), if hub.filtering || !hub.filter.is_empty() { Role::Title } else { Role::Muted });
    if hub.filtering {
        g.panels.push((y, left + 1..end + 1));
    }
    y += 1;
    let mut x = left + 2;
    let all = if hub.kind_filter.is_none() { Role::Accent } else { Role::Muted };
    x = g.put(y, x, &format!("all {}", hub.projects.len()), all) + 3;
    for kind in hub.kinds() {
        let n = hub.projects.iter().filter(|p| p.kind == kind).count();
        let text = format!("{} {n}", kind.tag());
        if x + text.len() > left + list_width {
            break;
        }
        x = g.put(y, x, &text, if hub.kind_filter == Some(kind) { Role::Accent } else { Role::Kind(kind) }) + 3;
    }
    y += 2;

    let rows = hub.rows();
    let mut heading = None;
    for (row, (group, i)) in rows.iter().enumerate() {
        let p = &hub.projects[*i];
        if heading != Some(group.as_str()) {
            if heading.is_some() {
                y += 1;
            }
            g.heading(y, left, list_width.saturating_sub(2), group);
            heading = Some(group.as_str());
            y += 1;
        }
        let focused = row == hub.focus;
        if focused {
            g.focus(y, left..left + list_width.saturating_sub(2));
        }
        g.put(y, left + 2, p.kind.tag(), Role::Kind(p.kind));
        // Right side: health dot, changes, branch.
        let branch = p.git.as_ref().and_then(|g| g.branch.clone()).or_else(|| p.branch.clone());
        let mut right = String::new();
        if let Some(branch) = &branch {
            right.push_str(&fit(branch, 18));
        } else if !p.exists {
            right.push_str("missing");
        }
        if let Some(git) = &p.git {
            if git.ahead > 0 {
                right.push_str(&format!(" ↑{}", git.ahead));
            }
            if git.changed > 0 {
                right.push_str(&format!(" •{}", git.changed));
            }
        }
        let right_len = right.chars().count() + 2;
        let right_x = (left + list_width).saturating_sub(right_len + 2);
        let name_room = right_x.saturating_sub(left + 8);
        g.put(y, left + 7, &fit(&p.name, name_room), if focused { Role::Title } else { Role::Text });
        g.put(y, right_x, &right, if p.exists { Role::Muted } else { Role::Bad });
        if let Some((health, _)) = p.health {
            g.put(y, (left + list_width).saturating_sub(3), "●", health_role(health));
        }
        y += 1;
    }
    if rows.is_empty() {
        let text = if hub.projects.is_empty() { "no projects yet -- c creates one, a adds a folder" } else { "nothing matches" };
        g.put(y, left + 2, &fit(text, list_width.saturating_sub(4)), Role::Muted);
        y += 1;
    }
    y += 1;
    g.put(y, left + 2, "+", Role::Accent);
    let end = g.put(y, left + 7, "new project", Role::Text);
    g.put(y, end + 2, "c", Role::Accent);
    y += 1;
    if let Some(root) = &hub.confirm_remove {
        y += 1;
        let name = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        g.put(y, left + 2, &fit(&format!("d again to remove {name} from the list -- the folder stays"), list_width), Role::Warn);
        y += 1;
    }

    // The preview.
    if let Some(p) = hub.selected() {
        let (x, mut y, w) = if two_columns { (left + list_width + 2, 4, width - list_width - 2) } else { (left, y + 1, width) };
        let end = g.put(y, x, &fit(&p.name, w.saturating_sub(8)), Role::Title);
        g.put(y, end + 2, p.kind.tag(), Role::Kind(p.kind));
        y += 1;
        g.put(y, x, &fit(&p.root.display().to_string(), w), Role::Muted);
        y += 1;
        if let Some(editing) = &hub.editing_group {
            let end = g.put(y, x, "group  ", Role::Muted);
            let end2 = g.put(y, end, &format!("{editing}▏"), Role::Title);
            g.panels.push((y, end.saturating_sub(1)..end2 + 1));
            y += 1;
        } else if p.pinned || p.group.is_some() {
            let text = match (p.pinned, &p.group) {
                (true, Some(g)) => format!("pinned · {g}"),
                (true, None) => "pinned".to_string(),
                (false, Some(g)) => g.clone(),
                _ => String::new(),
            };
            g.put(y, x, &fit(&text, w), Role::Muted);
            y += 1;
        }

        y += 1;
        g.heading(y, x, w.saturating_sub(9), "Health");
        g.put(y, (x + w).saturating_sub(7), "SPC p h", Role::Muted);
        y += 1;
        match p.health {
            Some((health, problems)) => {
                g.put(y, x, "●", health_role(health));
                let text = match problems {
                    0 => "no problems found".to_string(),
                    n => format!("{n} problem{} -- h for the doctor", if n == 1 { "" } else { "s" }),
                };
                g.put(y, x + 2, &fit(&text, w.saturating_sub(2)), Role::Text);
            }
            None => {
                g.put(y, x, "checking…", Role::Muted);
            }
        }
        y += 2;

        g.heading(y, x, w, "Git");
        y += 1;
        match &p.git {
            Some(git) => {
                let mut line = git.branch.clone().unwrap_or_else(|| "?".to_string());
                if git.ahead > 0 {
                    line.push_str(&format!(" ↑{}", git.ahead));
                }
                if git.behind > 0 {
                    line.push_str(&format!(" ↓{}", git.behind));
                }
                line.push_str(&match git.changed {
                    0 => " · clean".to_string(),
                    n => format!(" · {n} changed"),
                });
                g.put(y, x, &fit(&line, w), Role::Text);
                y += 1;
                if let Some((subject, _)) = &git.last_commit {
                    let age = p.last_commit_age.as_deref().map(|a| format!(" · {a}")).unwrap_or_default();
                    g.put(y, x, &fit(&format!("“{subject}”{age}"), w), Role::Muted);
                    y += 1;
                }
            }
            None if p.root.join(".git").exists() => {
                g.put(y, x, "reading…", Role::Muted);
                y += 1;
            }
            None => {
                g.put(y, x, "not a repository", Role::Muted);
                y += 1;
            }
        }
        if p.jira.is_some() || !p.work.is_empty() {
            y += 1;
            g.heading(y, x, w, "Work");
            y += 1;
            if let Some(key) = &p.jira {
                let end = g.put(y, x, key, Role::Accent);
                g.put(y, end + 2, "Jira project", Role::Muted);
                y += 1;
            }
            for item in p.work.iter().take(3) {
                g.put(y, x, "○", Role::Muted);
                g.put(y, x + 2, &fit(item, w.saturating_sub(2)), Role::Text);
                y += 1;
            }
            if p.work.len() > 3 {
                g.put(y, x + 2, &format!("and {} more", p.work.len() - 3), Role::Muted);
                y += 1;
            }
        }
        if !p.tasks.is_empty() {
            y += 1;
            g.heading(y, x, w, "Tasks");
            y += 1;
            for line in wrap(&p.tasks.join(" · "), w).into_iter().take(3) {
                g.put(y, x, &line, Role::Text);
                y += 1;
            }
        }
    }

    let keys: &[(&str, &str)] = if hub.editing_group.is_some() {
        &[("Enter", "set group"), ("Esc", "cancel")]
    } else if hub.filtering {
        &[("Enter", "open"), ("↑ ↓", "move"), ("Esc", "clear")]
    } else {
        &[("Enter", "open"), ("c", "new"), ("h", "doctor"), (",", "settings"), ("P", "pin"), ("g", "group"), ("d", "remove"), ("Tab", "kind"), ("/", "filter"), ("q", "close")]
    };
    g.keys(left, width, keys);
    g.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(name: &str, kind: ProjectKind, group: Option<&str>, pinned: bool) -> HubProject {
        let mut p = HubProject::new(PathBuf::from(format!("/work/{name}")), kind);
        p.group = group.map(str::to_string);
        p.pinned = pinned;
        p
    }

    fn hub() -> Hub {
        Hub::new(vec![
            project("orbit-tools", ProjectKind::Python, None, true),
            project("blink-lab", ProjectKind::Arduino, Some("Class labs"), false),
            project("mission-c", ProjectKind::Mib, Some("Mission ops"), false),
            project("servo-sweep", ProjectKind::Arduino, Some("Class labs"), false),
            project("report-gen", ProjectKind::Python, None, false),
        ])
    }

    fn names(hub: &Hub) -> Vec<String> {
        hub.rows().iter().map(|(g, i)| format!("{g}/{}", hub.projects[*i].name)).collect()
    }

    #[test]
    fn rows_are_pinned_then_groups_then_the_rest() {
        assert_eq!(
            names(&hub()),
            ["Pinned/orbit-tools", "Class labs/blink-lab", "Class labs/servo-sweep", "Mission ops/mission-c", "Ungrouped/report-gen"]
        );
        let text = layout(&hub(), 130).text;
        assert!(text.contains("PINNED") && text.contains("CLASS LABS") && text.contains("UNGROUPED"), "{text}");
        assert!(text.contains("INO 2") && text.contains("all 5"), "kind chips:\n{text}");
    }

    #[test]
    fn typing_filters_fuzzily_and_enter_opens_the_first_match() {
        let mut hub = hub();
        for c in "srsw".chars() {
            hub.key(Key::Char(c));
        }
        assert!(hub.filtering);
        assert_eq!(names(&hub), ["Class labs/servo-sweep"]);
        assert_eq!(hub.key(Key::Enter), Action::Open(PathBuf::from("/work/servo-sweep")));
        hub.key(Key::Escape);
        assert_eq!(names(&hub).len(), 5, "Esc clears the filter");
    }

    #[test]
    fn tab_cycles_the_kind_filter() {
        let mut hub = hub();
        hub.key(Key::Tab);
        assert_eq!(hub.kind_filter, Some(ProjectKind::Python));
        assert_eq!(names(&hub), ["Pinned/orbit-tools", "Projects/report-gen"].map(|s| s.replace("Projects", "Ungrouped")));
        hub.key(Key::Tab);
        assert_eq!(hub.kind_filter, Some(ProjectKind::Arduino));
        hub.key(Key::Tab);
        hub.key(Key::Tab);
        assert_eq!(hub.kind_filter, None, "past the last kind: all again");
    }

    #[test]
    fn the_row_keys_ask_for_their_actions() {
        let mut hub = hub();
        hub.key(Key::Char('j'));
        let blink = PathBuf::from("/work/blink-lab");
        assert_eq!(hub.key(Key::Char('h')), Action::Doctor(blink.clone()));
        assert_eq!(hub.key(Key::Char(',')), Action::Settings(blink.clone()));
        assert_eq!(hub.key(Key::Char('P')), Action::TogglePin(blink.clone()));
        assert_eq!(hub.key(Key::Char('d')), Action::None, "d once asks");
        assert!(layout(&hub, 130).text.contains("d again to remove blink-lab"));
        assert_eq!(hub.key(Key::Char('d')), Action::Remove(blink.clone()));
        hub.key(Key::Char('g'));
        hub.key(Key::Backspace);
        for c in "Labs".chars() {
            hub.key(Key::Char(c));
        }
        assert!(hub.editing_group.as_deref().unwrap().ends_with("Labs"));
        assert!(matches!(hub.key(Key::Enter), Action::SetGroup(root, _) if root == blink));
        assert_eq!(hub.key(Key::Char('c')), Action::New);
    }

    #[test]
    fn the_preview_shows_git_health_and_work() {
        let mut hub = hub();
        let p = &mut hub.projects[0];
        p.git = Some(GitSummary { branch: Some("master".into()), changed: 3, ahead: 2, behind: 0, last_commit: Some(("Parse PUS-17 replies".into(), 0)) });
        p.last_commit_age = Some("1 h".into());
        p.health = Some((Health::Warn, 1));
        p.jira = Some("FNX".into());
        p.work = vec!["FNX-58 Telemetry decoder".into()];
        p.tasks = vec!["run".into(), "test".into()];
        let text = layout(&hub, 130).text;
        for needle in ["master ↑2 · 3 changed", "Parse PUS-17 replies", "1 problem", "FNX-58 Telemetry decoder", "run · test", "master ↑2 •3"] {
            assert!(text.contains(needle), "{needle:?} missing:\n{text}");
        }
    }

    #[test]
    fn a_folder_typed_into_the_filter_is_offered_for_adding() {
        let dir = std::env::temp_dir();
        let mut hub = Hub::new(Vec::new());
        hub.key(Key::Char('/'));
        for c in dir.display().to_string().chars() {
            hub.key(Key::Char(c));
        }
        assert_eq!(hub.key(Key::Enter), Action::Add(dir));
    }

    #[test]
    fn it_fits_a_narrow_pane() {
        let mut hub = hub();
        hub.projects[0].git = Some(GitSummary { branch: Some("a-very-long-feature-branch-name".into()), ..Default::default() });
        let page = layout(&hub, 50);
        assert!(page.text.lines().all(|l| l.chars().count() <= 50), "{}", page.text);
        assert!(page.focus.is_some());
    }
}
