//! `SPC ,`: every setting on one page. Categories on the left, their
//! settings on the right, one row each with its value and, under the
//! selected one, what it does and its key in `settings.toml`. A `•`
//! marks what's been changed from the default. `/` searches every
//! setting's name, key and description. Each kind of value edits the
//! natural way -- `Space` flips a switch, `h`/`l` step a number or cycle
//! a choice, `Enter` types a value in place -- and lists get rows to
//! add to, edit and delete. The page is built from the schema, so a new
//! setting shows up here by being declared there.
//!
//! In a project's scope (`SPC p ,`) it shows the settings a project may
//! set, and says of each whether it's set here, comes from you, or is
//! the default.

use std::collections::HashMap;
use std::path::PathBuf;

use fenix_config::{settings, Category, Kind, Problem, Secret, Setting, Value};

use crate::page::{fit, frame, wrap, Grid, Key, Page, Popup, Role};

/// Whose settings the page is showing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    You,
    Project { root: PathBuf, name: String },
}

/// How a token stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretState {
    pub set: bool,
    /// Where it comes from: "Credential Manager", "FENIX_GITLAB_TOKEN",
    /// "gh CLI".
    pub source: String,
}

/// What the page shows, read from the settings by the host.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    /// Set in the file this scope writes to.
    pub here: HashMap<&'static str, Value>,
    /// In a project's scope: your own value, which applies when the
    /// project doesn't set one.
    pub inherited: HashMap<&'static str, Value>,
    /// In your scope: what the open project sets instead of you.
    pub overridden: HashMap<&'static str, (String, Value)>,
    pub problems: Vec<Problem>,
    pub secrets: HashMap<Secret, SecretState>,
    /// The choices for `Kind::Theme` and `Kind::Font`.
    pub themes: Vec<String>,
    pub fonts: Vec<String>,
    /// The file this scope writes to.
    pub file: PathBuf,
    /// The project `Tab` switches to, when there is one.
    pub project: Option<(PathBuf, String)>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    None,
    Close,
    /// Set (or, with `None`, reset) a setting; already checked.
    Set { key: &'static str, value: Option<Value> },
    /// Store (or, with `None`, forget) a token.
    SetSecret { secret: Secret, token: Option<String> },
    /// Check a token against its server.
    TestSecret(Secret),
    /// Open the file, at a setting's line when there's one.
    OpenFile(Option<&'static str>),
    /// Show `scope` instead.
    SwitchScope(Scope),
    /// The project's own page: kind, group, Jira, tasks, launch.
    OpenProjectPage(PathBuf),
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Focus {
    Categories,
    Settings,
}

#[derive(Debug, Clone, Copy)]
pub enum Row {
    Heading(Category),
    Setting(&'static Setting),
    /// Entry `n` of a list setting.
    Entry(&'static Setting, usize),
    /// "+ add" under a list setting.
    Add(&'static Setting),
}

/// Rows are the same when they're about the same setting (settings are
/// compared by key: a setting is its key).
impl PartialEq for Row {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Row::Heading(a), Row::Heading(b)) => a == b,
            (Row::Setting(a), Row::Setting(b)) | (Row::Add(a), Row::Add(b)) => a.key == b.key,
            (Row::Entry(a, i), Row::Entry(b, j)) => a.key == b.key && i == j,
            _ => false,
        }
    }
}

/// A value being typed.
#[derive(Debug, Clone, PartialEq)]
enum Edit {
    /// In place, on the setting's row.
    Inline { key: &'static str, text: String },
    /// A popup form for one entry of a list: its fields, the one being
    /// typed, and which entry it replaces (`None` adds one).
    Entry { key: &'static str, index: Option<usize>, fields: Vec<String>, at: usize },
}

pub struct SettingsPage {
    pub scope: Scope,
    pub snap: Snapshot,
    category: usize,
    focus: Focus,
    /// Among the selectable rows.
    cursor: usize,
    search: Option<String>,
    searching: bool,
    edit: Option<Edit>,
    /// Why the last value was refused, and for which setting.
    pub refused: Option<(&'static str, String)>,
    /// A line under the header: "saved", "signs in as thomas".
    pub note: Option<(String, bool)>,
    /// `d` once asks; `d` again deletes.
    armed_delete: bool,
}

fn setting_matches(s: &Setting, needle: &str) -> bool {
    let needle = needle.to_lowercase();
    [s.label, s.key, s.help, s.category.label()].iter().any(|t| t.to_lowercase().contains(&needle))
}

/// A list setting's entries, as rows of fields.
fn entries(kind: &Kind, value: Option<&Value>) -> Vec<Vec<String>> {
    match (kind, value) {
        (Kind::Map { .. }, Some(Value::Map(m))) => m.iter().map(|(k, v)| vec![k.clone(), v.clone()]).collect(),
        (Kind::Records(_), Some(Value::Records(rows))) => rows.clone(),
        _ => Vec::new(),
    }
}

/// The labels of a list setting's fields.
fn field_labels(kind: &Kind) -> Vec<&'static str> {
    match kind {
        Kind::Map { key, value, .. } => vec![key, value],
        Kind::Records(fields) => fields.iter().map(|f| f.label).collect(),
        _ => Vec::new(),
    }
}

fn is_list(kind: &Kind) -> bool {
    matches!(kind, Kind::Map { .. } | Kind::Records(_))
}

impl SettingsPage {
    pub fn new(scope: Scope, snap: Snapshot) -> Self {
        let mut page = SettingsPage { scope: Scope::You, snap, category: 0, focus: Focus::Settings, cursor: 0, search: None, searching: false, edit: None, refused: None, note: None, armed_delete: false };
        page.set_scope(scope);
        page
    }

    /// Shows `scope`, on its first category of settings.
    pub fn set_scope(&mut self, scope: Scope) {
        self.scope = scope;
        self.category = self.sections().iter().position(Option::is_some).unwrap_or(0);
        self.cursor = 0;
        self.edit = None;
    }

    /// The left column: in a project's scope, the project's own page
    /// (`None`) first, then the categories.
    fn sections(&self) -> Vec<Option<Category>> {
        let project = matches!(self.scope, Scope::Project { .. }).then_some(None);
        project.into_iter().chain(self.categories().into_iter().map(Some)).collect()
    }

    /// Whether the project's own page is the section showing.
    fn on_project_page(&self) -> bool {
        self.search.is_none() && matches!(self.sections().get(self.category), Some(None))
    }

    /// New values from the host, keeping the cursor on the same setting.
    pub fn refresh(&mut self, snap: Snapshot) {
        let before = self.selected();
        self.snap = snap;
        if let Some(row) = before {
            if let Some(i) = self.selectable().iter().position(|r| *r == row) {
                self.cursor = i;
            }
        }
        self.cursor = self.cursor.min(self.selectable().len().saturating_sub(1));
    }

    /// Opens on `key`'s setting.
    pub fn show(&mut self, key: &str) {
        if let Some(s) = fenix_config::setting(key) {
            if let Some(i) = self.sections().iter().position(|c| *c == Some(s.category)) {
                self.category = i;
            }
            self.focus = Focus::Settings;
            if let Some(i) = self.selectable().iter().position(|r| matches!(r, Row::Setting(x) if x.key == key)) {
                self.cursor = i;
            }
        }
    }

    pub fn typing(&self) -> bool {
        self.searching || self.edit.is_some()
    }

    /// Space flips a switch; anywhere else it's the leader.
    pub fn claims_space(&self) -> bool {
        self.typing() || matches!(self.selected(), Some(Row::Setting(s)) if s.kind == Kind::Bool)
    }

    /// Text pasted while typing.
    pub fn paste(&mut self, text: &str) {
        let text: String = text.chars().filter(|c| !c.is_control()).collect();
        if let Some(target) = self.typed() {
            target.push_str(&text);
        }
    }

    fn typed(&mut self) -> Option<&mut String> {
        if self.searching {
            return self.search.as_mut();
        }
        match &mut self.edit {
            Some(Edit::Inline { text, .. }) => Some(text),
            Some(Edit::Entry { fields, at, .. }) => fields.get_mut(*at),
            None => None,
        }
    }

    fn visible(&self, s: &Setting) -> bool {
        match &self.scope {
            Scope::You => true,
            Scope::Project { .. } => s.project,
        }
    }

    /// The categories this scope has settings in.
    pub fn categories(&self) -> Vec<Category> {
        Category::ALL.into_iter().filter(|c| settings().iter().any(|s| s.category == *c && self.visible(s))).collect()
    }

    fn shown_settings(&self) -> Vec<&'static Setting> {
        match self.search.as_deref().filter(|s| !s.trim().is_empty()) {
            Some(needle) => settings().iter().filter(|s| self.visible(s) && setting_matches(s, needle)).collect(),
            None => {
                let Some(Some(category)) = self.sections().get(self.category).copied() else { return Vec::new() };
                settings().iter().filter(|s| s.category == category && self.visible(s)).collect()
            }
        }
    }

    /// Every row on the right, headings included.
    pub fn rows(&self) -> Vec<Row> {
        let mut rows = Vec::new();
        let mut heading = None;
        for s in self.shown_settings() {
            if heading != Some(s.category) {
                rows.push(Row::Heading(s.category));
                heading = Some(s.category);
            }
            rows.push(Row::Setting(s));
            if is_list(&s.kind) {
                for i in 0..entries(&s.kind, self.snap.here.get(s.key)).len() {
                    rows.push(Row::Entry(s, i));
                }
                rows.push(Row::Add(s));
            }
        }
        rows
    }

    fn selectable(&self) -> Vec<Row> {
        self.rows().into_iter().filter(|r| !matches!(r, Row::Heading(_))).collect()
    }

    pub fn selected(&self) -> Option<Row> {
        match self.focus {
            Focus::Settings => self.selectable().get(self.cursor).copied(),
            Focus::Categories => None,
        }
    }

    fn setting_of(row: Row) -> Option<&'static Setting> {
        match row {
            Row::Setting(s) | Row::Entry(s, _) | Row::Add(s) => Some(s),
            Row::Heading(_) => None,
        }
    }

    /// The value in force for `s` in this scope: set here, else
    /// inherited, else none (the default).
    fn value(&self, s: &Setting) -> Option<&Value> {
        self.snap.here.get(s.key).or_else(|| self.snap.inherited.get(s.key))
    }

    fn set(&mut self, key: &'static str, value: Option<Value>) -> Action {
        self.refused = None;
        Action::Set { key, value }
    }

    /// `h`/`l`: the value one step along.
    fn step(&mut self, s: &'static Setting, forward: bool) -> Action {
        let current = self.value(s).cloned();
        let cycle = |choices: &[String], current: Option<&str>| -> Option<Value> {
            if choices.is_empty() {
                return None;
            }
            let at = current.and_then(|c| choices.iter().position(|x| x == c));
            let next = match (at, forward) {
                (Some(i), true) => (i + 1) % choices.len(),
                (Some(i), false) => (i + choices.len() - 1) % choices.len(),
                (None, _) => 0,
            };
            Some(Value::Text(choices[next].clone()))
        };
        let text = match &current {
            Some(Value::Text(t)) => Some(t.as_str()),
            _ => None,
        };
        let next = match s.kind {
            Kind::Bool => {
                // Flips what's in force: the default when nothing's set.
                let on = match current {
                    Some(Value::Bool(b)) => b,
                    _ => s.default == "on",
                };
                Some(Value::Bool(!on))
            }
            Kind::Int { min, max } => {
                let n = match current {
                    Some(Value::Int(n)) => n,
                    _ => s.default.parse().unwrap_or(min),
                };
                Some(Value::Int((n + if forward { 1 } else { -1 }).clamp(min, max)))
            }
            Kind::Float { min, max } => {
                let n = match current {
                    Some(Value::Float(n)) => n,
                    Some(Value::Int(n)) => n as f64,
                    _ => s.default.parse().unwrap_or(min),
                };
                Some(Value::Float((n + if forward { 1.0 } else { -1.0 }).clamp(min, max)))
            }
            Kind::Minutes => {
                let n = match current {
                    Some(Value::Int(n)) => n,
                    _ => s.default.parse().unwrap_or(1),
                };
                Some(Value::Int((n + if forward { 1 } else { -1 }).max(1)))
            }
            Kind::Choice(choices) => cycle(&choices.iter().map(|c| c.to_string()).collect::<Vec<_>>(), text.or(Some(s.default))),
            Kind::Theme => cycle(&self.snap.themes.clone(), text.or(Some(s.default))),
            Kind::Font => cycle(&self.snap.fonts.clone(), text),
            _ => None,
        };
        match next {
            Some(v) => self.set(s.key, Some(v)),
            None => Action::None,
        }
    }

    fn start_inline(&mut self, s: &'static Setting) {
        let text = match (&s.kind, self.snap.here.get(s.key)) {
            (Kind::Secret(_), _) => String::new(),
            (Kind::List, Some(Value::List(l))) => l.join(", "),
            (_, Some(v)) => v.show(),
            (_, None) => String::new(),
        };
        self.edit = Some(Edit::Inline { key: s.key, text });
    }

    fn move_cursor(&mut self, by: isize) {
        match self.focus {
            Focus::Categories => {
                let n = self.sections().len();
                self.category = (self.category as isize + by).clamp(0, n.saturating_sub(1) as isize) as usize;
                self.cursor = 0;
            }
            Focus::Settings => {
                let n = self.selectable().len();
                self.cursor = (self.cursor as isize + by).clamp(0, n.saturating_sub(1) as isize) as usize;
            }
        }
    }

    pub fn key(&mut self, key: Key) -> Action {
        if key != Key::Char('d') {
            self.armed_delete = false;
        }
        if self.searching {
            match key {
                Key::Escape => {
                    self.searching = false;
                    self.search = None;
                }
                Key::Enter | Key::Down => {
                    self.searching = false;
                    self.focus = Focus::Settings;
                    self.cursor = 0;
                }
                Key::Backspace => {
                    if let Some(s) = &mut self.search {
                        s.pop();
                    }
                }
                Key::Char(c) => self.search.get_or_insert_with(String::new).push(c),
                Key::Space => self.search.get_or_insert_with(String::new).push(' '),
                _ => {}
            }
            self.cursor = 0;
            return Action::None;
        }
        if let Some(edit) = self.edit.clone() {
            return self.edit_key(edit, key);
        }
        let row = self.selected();
        let s = row.and_then(Self::setting_of);
        match key {
            Key::Down | Key::Char('j') => self.move_cursor(1),
            Key::Up | Key::Char('k') => self.move_cursor(-1),
            Key::Tab | Key::BackTab if self.search.is_none() => {
                self.focus = if self.focus == Focus::Settings { Focus::Categories } else { Focus::Settings };
            }
            Key::Char('/') => {
                self.searching = true;
                self.search = Some(String::new());
            }
            Key::Escape if self.search.is_some() => self.search = None,
            Key::Char('q') | Key::Escape => return Action::Close,
            Key::Char('e') => return Action::OpenFile(s.map(|s| s.key)),
            Key::Char('p') | Key::Char('P') => {
                return match (&self.scope, &self.snap.project) {
                    (Scope::You, Some((root, name))) => Action::SwitchScope(Scope::Project { root: root.clone(), name: name.clone() }),
                    (Scope::Project { .. }, _) => Action::SwitchScope(Scope::You),
                    _ => {
                        self.note = Some(("no project open -- open a file in one first".into(), true));
                        Action::None
                    }
                };
            }
            Key::Enter if self.on_project_page() => {
                if let Scope::Project { root, .. } = &self.scope {
                    return Action::OpenProjectPage(root.clone());
                }
            }
            _ if self.focus == Focus::Categories => {
                if matches!(key, Key::Enter | Key::Char('l') | Key::Right) {
                    self.focus = Focus::Settings;
                    self.cursor = 0;
                }
            }
            Key::Char('h') | Key::Left => {
                if let Some(Row::Setting(s)) = row {
                    return self.step(s, false);
                }
            }
            Key::Char('l') | Key::Right => {
                if let Some(Row::Setting(s)) = row {
                    return self.step(s, true);
                }
            }
            Key::Space => {
                if let Some(Row::Setting(s)) = row {
                    if s.kind == Kind::Bool {
                        return self.step(s, true);
                    }
                }
            }
            Key::Char('r') => {
                if let Some(s) = s {
                    if let Kind::Secret(secret) = s.kind {
                        return Action::SetSecret { secret, token: None };
                    }
                    if self.snap.here.contains_key(s.key) {
                        self.note = Some((format!("{} back to {}", s.label, if matches!(self.scope, Scope::Project { .. }) { "yours" } else { "the default" }), false));
                        return self.set(s.key, None);
                    }
                }
            }
            Key::Char('x') => {
                if let Some(Row::Setting(Setting { kind: Kind::Secret(secret), .. })) = row {
                    return Action::SetSecret { secret: *secret, token: None };
                }
            }
            Key::Char('t') => {
                if let Some(Row::Setting(Setting { kind: Kind::Secret(secret), .. })) = row {
                    return Action::TestSecret(*secret);
                }
            }
            Key::Char('a') => {
                if let Some(s) = s.filter(|s| is_list(&s.kind)) {
                    self.edit = Some(Edit::Entry { key: s.key, index: None, fields: vec![String::new(); field_labels(&s.kind).len()], at: 0 });
                }
            }
            Key::Char('d') => {
                if let Some(Row::Entry(s, i)) = row {
                    if !self.armed_delete {
                        self.armed_delete = true;
                        self.note = Some(("d again deletes it".into(), false));
                        return Action::None;
                    }
                    self.armed_delete = false;
                    let mut list = entries(&s.kind, self.snap.here.get(s.key));
                    list.remove(i);
                    return self.set_entries(s, list);
                }
            }
            Key::Char('J') | Key::Char('K') => {
                if let Some(Row::Entry(s, i)) = row {
                    let mut list = entries(&s.kind, self.snap.here.get(s.key));
                    let j = if key == Key::Char('J') { i + 1 } else { i.wrapping_sub(1) };
                    if j < list.len() {
                        list.swap(i, j);
                        self.cursor = (self.cursor as isize + if key == Key::Char('J') { 1 } else { -1 }) as usize;
                        return self.set_entries(s, list);
                    }
                }
            }
            Key::Enter => match row {
                Some(Row::Setting(s)) => match s.kind {
                    Kind::Bool | Kind::Choice(_) | Kind::Theme => return self.step(s, true),
                    Kind::Map { .. } | Kind::Records(_) => {
                        self.edit = Some(Edit::Entry { key: s.key, index: None, fields: vec![String::new(); field_labels(&s.kind).len()], at: 0 });
                    }
                    _ => self.start_inline(s),
                },
                Some(Row::Entry(s, i)) => {
                    let fields = entries(&s.kind, self.snap.here.get(s.key)).get(i).cloned().unwrap_or_default();
                    self.edit = Some(Edit::Entry { key: s.key, index: Some(i), fields, at: 0 });
                }
                Some(Row::Add(s)) => {
                    self.edit = Some(Edit::Entry { key: s.key, index: None, fields: vec![String::new(); field_labels(&s.kind).len()], at: 0 });
                }
                _ => {}
            },
            _ => {}
        }
        Action::None
    }

    fn set_entries(&mut self, s: &'static Setting, list: Vec<Vec<String>>) -> Action {
        let value = match s.kind {
            _ if list.is_empty() => None,
            Kind::Map { .. } => Some(Value::Map(list.into_iter().map(|r| (r[0].clone(), r[1].clone())).collect())),
            _ => Some(Value::Records(list)),
        };
        if let Some(v) = &value {
            if let Err(why) = s.kind.check(v) {
                self.refused = Some((s.key, why));
                return Action::None;
            }
        }
        self.set(s.key, value)
    }

    fn edit_key(&mut self, edit: Edit, key: Key) -> Action {
        match key {
            Key::Escape => {
                self.edit = None;
                self.refused = None;
                return Action::None;
            }
            Key::Backspace => {
                if let Some(t) = self.typed() {
                    t.pop();
                }
                return Action::None;
            }
            Key::Char(c) => {
                if let Some(t) = self.typed() {
                    t.push(c);
                }
                return Action::None;
            }
            Key::Space => {
                if let Some(t) = self.typed() {
                    t.push(' ');
                }
                return Action::None;
            }
            _ => {}
        }
        match edit {
            Edit::Inline { key: k, text } => {
                if key != Key::Enter {
                    return Action::None;
                }
                let Some(s) = fenix_config::setting(k) else { return Action::None };
                self.edit = None;
                if let Kind::Secret(secret) = s.kind {
                    return Action::SetSecret { secret, token: Some(text).filter(|t| !t.trim().is_empty()) };
                }
                if text.trim().is_empty() {
                    return self.set(k, None);
                }
                match s.kind.parse(&text) {
                    Ok(v) => self.set(k, Some(v)),
                    Err(why) => {
                        // Stays open with the reason, to be put right.
                        self.edit = Some(Edit::Inline { key: k, text });
                        self.refused = Some((k, why));
                        Action::None
                    }
                }
            }
            Edit::Entry { key: k, index, mut fields, at } => {
                let Some(s) = fenix_config::setting(k) else { return Action::None };
                match key {
                    Key::Tab | Key::Down => {
                        self.edit = Some(Edit::Entry { key: k, index, fields, at: (at + 1) % field_labels(&s.kind).len().max(1) });
                        Action::None
                    }
                    Key::BackTab | Key::Up => {
                        let n = field_labels(&s.kind).len().max(1);
                        self.edit = Some(Edit::Entry { key: k, index, fields, at: (at + n - 1) % n });
                        Action::None
                    }
                    Key::Enter if at + 1 < fields.len() => {
                        self.edit = Some(Edit::Entry { key: k, index, fields, at: at + 1 });
                        Action::None
                    }
                    Key::Enter => {
                        for f in &mut fields {
                            *f = f.trim().to_string();
                        }
                        if let Kind::Records(defs) = s.kind {
                            for (f, def) in fields.iter_mut().zip(defs.iter()) {
                                if f.is_empty() {
                                    if let Some(d) = def.default {
                                        *f = d.to_string();
                                    }
                                }
                            }
                        }
                        let mut list = entries(&s.kind, self.snap.here.get(s.key));
                        match index {
                            Some(i) if i < list.len() => list[i] = fields.clone(),
                            _ => list.push(fields.clone()),
                        }
                        let action = self.set_entries(s, list);
                        if action == Action::None {
                            // Refused: the form stays, with the reason.
                            self.edit = Some(Edit::Entry { key: k, index, fields, at });
                        } else {
                            self.edit = None;
                        }
                        action
                    }
                    _ => {
                        self.edit = Some(Edit::Entry { key: k, index, fields, at });
                        Action::None
                    }
                }
            }
        }
    }
}

/// How a setting's value reads on its row.
fn shown(page: &SettingsPage, s: &Setting) -> (String, Role) {
    if let Kind::Secret(secret) = s.kind {
        return match page.snap.secrets.get(&secret) {
            Some(state) if state.set => ("set".into(), Role::Good),
            _ => ("not set".into(), Role::Muted),
        };
    }
    match (page.snap.here.get(s.key), page.snap.inherited.get(s.key)) {
        (Some(v), _) => (v.show(), Role::Text),
        (None, Some(v)) => (v.show(), Role::Muted),
        (None, None) if s.default.is_empty() => ("not set".into(), Role::Muted),
        (None, None) => (s.default.to_string(), Role::Muted),
    }
}

/// What the right edge of a setting's row says.
fn hint(page: &SettingsPage, s: &Setting) -> String {
    if let Kind::Secret(secret) = s.kind {
        return page.snap.secrets.get(&secret).map(|st| st.source.clone()).unwrap_or_default();
    }
    let here = page.snap.here.contains_key(s.key);
    match &page.scope {
        Scope::Project { .. } => match (here, page.snap.inherited.get(s.key)) {
            (true, Some(yours)) => format!("set here · yours: {}", yours.show()),
            (true, None) if !s.default.is_empty() => format!("set here · default {}", s.default),
            (true, None) => "set here".into(),
            (false, Some(_)) => "from you".into(),
            (false, None) => "default".into(),
        },
        Scope::You => {
            if let Some((project, value)) = page.snap.overridden.get(s.key) {
                return format!("{project} uses {}", value.show());
            }
            match (here, s.default.is_empty()) {
                (true, false) => format!("default {}", s.default),
                (true, true) => String::new(),
                (false, _) => "default".into(),
            }
        }
    }
}

pub fn layout(page: &SettingsPage, cols: usize) -> Page {
    let (left, width) = frame(cols, 132);
    let mut g = Grid::new();
    let title = match &page.scope {
        Scope::You => "Settings".to_string(),
        Scope::Project { name, .. } => format!("Settings · {name}"),
    };
    let x = g.put(1, left, &title, Role::Title) + 2;
    let changed = page.snap.here.len();
    let file = page.snap.file.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
    let scope_hint = match (&page.scope, &page.snap.project) {
        (Scope::You, Some((_, name))) => format!("p: {name}'s own"),
        (Scope::Project { .. }, _) => "p: yours".to_string(),
        _ => String::new(),
    };
    g.put(1, x, &scope_hint, Role::Muted);
    let right = format!("{file} · {changed} set");
    g.put(1, (left + width).saturating_sub(right.chars().count()), &right, Role::Muted);
    let mut y = 2;
    // The search field.
    let search_label = match (&page.search, page.searching) {
        (Some(s), true) => format!("/ {s}▏"),
        (Some(s), false) if !s.is_empty() => format!("/ {s}  (Esc clears)"),
        _ => format!("/ search {} settings", settings().iter().filter(|s| page.visible(s)).count()),
    };
    let end = g.put(y, left, &fit(&search_label, width), if page.searching { Role::Title } else { Role::Muted });
    if page.searching {
        g.panels.push((y, left..end.max(left + 30).min(left + width)));
    }
    y += 1;
    let problems = &page.snap.problems;
    if let Some((text, bad)) = &page.note {
        g.put(y, left, &fit(text, width), if *bad { Role::Bad } else { Role::Good });
        y += 1;
    } else if !problems.is_empty() {
        let first = &problems[0];
        let more = if problems.len() > 1 { format!(" (+{} more)", problems.len() - 1) } else { String::new() };
        g.put(y, left, &fit(&format!("{first}{more}"), width), Role::Warn);
        y += 1;
    }
    g.rule(y, left..left + width);
    y += 1;
    let top = y;

    // Categories.
    let cat_w = 28;
    let sections = page.sections();
    let searching = page.search.as_deref().is_some_and(|s| !s.trim().is_empty());
    for (i, c) in sections.iter().enumerate() {
        let set = settings().iter().filter(|s| Some(s.category) == *c && page.visible(s) && page.snap.here.contains_key(s.key)).count();
        let role = if i == page.category && !searching { Role::Title } else { Role::Text };
        let label = c.map(|c| c.label()).unwrap_or("Project & tasks");
        g.put(top + i, left, &fit(label, cat_w - 5), role);
        if set > 0 {
            g.put(top + i, left + cat_w - 4, &format!("• {set}"), Role::Accent);
        }
        if i == page.category && !searching {
            if page.focus == Focus::Categories {
                g.focus(top + i, left..left + cat_w - 1);
            } else {
                g.panels.push((top + i, left..left + cat_w - 1));
            }
        }
    }

    // Settings.
    let sx = left + cat_w + 2;
    let sw = width.saturating_sub(cat_w + 2);
    let label_w = 26.min(sw / 3);
    let vx = sx + label_w + 2;
    let selected = page.selected();
    let mut y = top;
    let rows = page.rows();
    if page.on_project_page() {
        for line in wrap("The project's kind, group, pin and Jira key, its tasks, its language servers and its debug launch (.fenix/tools.json).", sw) {
            g.put(y, sx, &line, Role::Text);
            y += 1;
        }
        y += 1;
        g.put(y, sx, "Enter opens them.", Role::Accent);
        if page.focus == Focus::Settings {
            g.focus(y, sx..sx + sw);
        }
    } else if rows.is_empty() {
        g.put(y, sx, "Nothing matches.", Role::Muted);
    }
    for row in rows {
        match row {
            Row::Heading(c) => {
                if y > top {
                    y += 1;
                }
                g.heading(y, sx, sw, c.label());
                y += 1;
                continue;
            }
            Row::Setting(s) => {
                let is_sel = selected == Some(row);
                let set_here = page.snap.here.contains_key(s.key);
                // Apart, not over each other: spans mustn't overlap.
                if set_here {
                    g.put(y, sx, "•", Role::Accent);
                }
                g.put(y, sx + 2, &fit(s.label, label_w.saturating_sub(2)), if is_sel { Role::Title } else { Role::Text });
                let editing = matches!(&page.edit, Some(Edit::Inline { key, .. }) if *key == s.key);
                let hint_text = hint(page, s);
                let room = (sx + sw).saturating_sub(vx + hint_text.chars().count() + 2);
                if editing {
                    let Some(Edit::Inline { text, .. }) = &page.edit else { unreachable!() };
                    let shown = if matches!(s.kind, Kind::Secret(_)) { "•".repeat(text.chars().count()) } else { text.clone() };
                    let end = g.put(y, vx, &fit(&format!("{shown}▏"), room.max(10)), Role::Title);
                    g.panels.push((y, vx.saturating_sub(1)..(end + 1).max(vx + 24).min(sx + sw)));
                } else {
                    let (text, role) = shown(page, s);
                    let text = match s.kind {
                        Kind::Bool => format!("{} {text}", if matches!(page.value(s), Some(Value::Bool(false))) || (page.value(s).is_none() && s.default == "off") { "[ ]" } else { "[x]" }),
                        Kind::Choice(_) | Kind::Theme | Kind::Int { .. } | Kind::Float { .. } | Kind::Minutes if is_sel => format!("‹ {text} ›"),
                        _ => text,
                    };
                    g.put(y, vx, &fit(&text, room), role);
                }
                g.put(y, (sx + sw).saturating_sub(hint_text.chars().count()), &hint_text, Role::Muted);
                if is_sel {
                    g.focus(y, sx..sx + sw);
                }
                y += 1;
                if let Some((key, why)) = &page.refused {
                    if *key == s.key {
                        g.put(y, vx, &fit(why, sw.saturating_sub(label_w + 2)), Role::Bad);
                        y += 1;
                    }
                }
                if let Some(p) = problems.iter().find(|p| p.key.as_deref() == Some(s.key)) {
                    g.put(y, vx, &fit(&format!("in the file: {}", p.message), sw.saturating_sub(label_w + 2)), Role::Warn);
                    y += 1;
                }
                if is_sel {
                    let mut help = s.help.to_string();
                    if s.restart {
                        help.push_str(" Takes effect when Fenix restarts.");
                    }
                    for line in wrap(&help, sw.saturating_sub(label_w + 2)) {
                        g.put(y, vx, &line, Role::Muted);
                        y += 1;
                    }
                    let key_line = if matches!(s.kind, Kind::Secret(_)) { format!("{} · never written to a file", page.snap.secrets.get(&secret_of(s)).map(|x| x.source.as_str()).unwrap_or("")) } else { s.key.to_string() };
                    g.put(y, vx, &fit(&key_line, sw.saturating_sub(label_w + 2)), Role::Accent);
                    y += 1;
                }
            }
            Row::Entry(s, i) => {
                let is_sel = selected == Some(row);
                let list = entries(&s.kind, page.snap.here.get(s.key));
                let fields = &list[i];
                let x = g.put(y, sx + 4, &fit(&fields[0], label_w.saturating_sub(4)), if is_sel { Role::Title } else { Role::Text });
                let rest = fields[1..].join(if matches!(s.kind, Kind::Records(_)) { "  " } else { "" });
                let rest = match s.kind {
                    Kind::Records(defs) if defs.len() == 3 && defs[2].name == "port" => format!("{}:{}", fields[1], fields[2]),
                    _ => rest,
                };
                g.put(y, vx.max(x + 1), &fit(&rest, (sx + sw).saturating_sub(vx)), Role::Muted);
                if is_sel {
                    g.focus(y, sx..sx + sw);
                }
                y += 1;
            }
            Row::Add(s) => {
                let is_sel = selected == Some(row);
                let what = field_labels(&s.kind).first().map(|l| l.to_lowercase()).unwrap_or_default();
                g.put(y, sx + 4, &format!("+ add {}", if what == "name" { "one".to_string() } else { format!("a {what}") }), Role::Accent);
                if is_sel {
                    g.focus(y, sx..sx + sw);
                }
                y += 1;
            }
        }
    }

    // The entry form floats beside the row it's about.
    if let Some(Edit::Entry { key, index, fields, at }) = &page.edit {
        if let Some(s) = fenix_config::setting(key) {
            let line = g.focus.as_ref().map(|(l, _)| *l).unwrap_or(top);
            let labels = field_labels(&s.kind);
            let label_w = labels.iter().map(|l| l.chars().count()).max().unwrap_or(4) + 2;
            let mut rows = vec![vec![(format!("{} · {}", s.label, if index.is_some() { "edit" } else { "new" }), Role::Title)], Vec::new()];
            for (i, (label, value)) in labels.iter().zip(fields).enumerate() {
                let on = i == *at;
                let shown = if on { format!("{value}▏") } else { value.clone() };
                rows.push(vec![(format!("{label:<label_w$}"), if on { Role::Accent } else { Role::Muted }), (format!("{shown:<40}"), if on { Role::Title } else { Role::Text })]);
            }
            if let Some((k, why)) = &page.refused {
                if k == key {
                    rows.push(Vec::new());
                    rows.push(vec![(why.clone(), Role::Bad)]);
                }
            }
            rows.push(Vec::new());
            rows.push(vec![("Tab next field · Enter saves · Esc cancels".into(), Role::Muted)]);
            g.popup = Some(Popup { line, col: vx, rows });
        }
    }

    let keys: Vec<(&str, &str)> = if page.searching {
        vec![("Enter", "go to the results"), ("Esc", "clear")]
    } else if matches!(page.edit, Some(Edit::Inline { .. })) {
        vec![("Enter", "keep"), ("Esc", "put it back")]
    } else if page.edit.is_some() {
        vec![("Tab", "next field"), ("Enter", "save"), ("Esc", "cancel")]
    } else {
        match selected {
            Some(Row::Setting(Setting { kind: Kind::Secret(_), .. })) => vec![("Enter", "set"), ("t", "test"), ("x", "clear"), ("/", "search"), ("e", "open the file"), ("q", "close")],
            Some(Row::Setting(s)) if is_list(&s.kind) => vec![("a", "add"), ("Enter", "add"), ("r", "clear all"), ("/", "search"), ("e", "open the file"), ("q", "close")],
            Some(Row::Entry(..)) => vec![("Enter", "edit"), ("d", "delete"), ("J/K", "move"), ("a", "add"), ("q", "close")],
            _ => vec![("j/k", "setting"), ("Tab", "categories"), ("h/l", "change"), ("Space", "toggle"), ("Enter", "edit"), ("r", "reset"), ("/", "search"), ("e", "open the file"), ("p", "project"), ("q", "close")],
        }
    };
    g.keys(left, width, &keys);
    g.finish()
}

fn secret_of(s: &Setting) -> Secret {
    match s.kind {
        Kind::Secret(secret) => secret,
        _ => Secret::GitLab,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap() -> Snapshot {
        let mut here = HashMap::new();
        here.insert("editor.font_size", Value::Float(18.0));
        here.insert("vnc.hosts", Value::Records(vec![vec!["test-vm".into(), "127.0.0.1".into(), "5900".into()], vec!["build".into(), "10.0.0.5".into(), "5901".into()]]));
        let mut secrets = HashMap::new();
        secrets.insert(Secret::GitLab, SecretState { set: true, source: "Credential Manager".into() });
        Snapshot { here, secrets, themes: vec!["Orbit Dark".into(), "Nord".into()], file: PathBuf::from("settings.toml"), ..Default::default() }
    }

    fn page() -> SettingsPage {
        SettingsPage::new(Scope::You, snap())
    }

    fn goto(p: &mut SettingsPage, key: &str) {
        p.show(key);
        assert!(matches!(p.selected(), Some(Row::Setting(s)) if s.key == key), "{key}");
    }

    #[test]
    fn a_category_lists_its_settings_with_what_changed_marked() {
        let mut p = page();
        goto(&mut p, "editor.font_size");
        let text = layout(&p, 130).text;
        assert!(text.contains("• Font size") && text.contains("‹ 18 ›") && text.contains("default 16"), "{text}");
        assert!(text.contains("Text size, in points.") && text.contains("editor.font_size"), "the selected one explains itself:\n{text}");
        assert!(text.contains("Appearance") && text.contains("• 1"), "{text}");
    }

    #[test]
    fn h_and_l_step_a_number_and_cycle_a_choice_and_space_flips_a_switch() {
        let mut p = page();
        goto(&mut p, "editor.font_size");
        assert_eq!(p.key(Key::Char('l')), Action::Set { key: "editor.font_size", value: Some(Value::Float(19.0)) });
        goto(&mut p, "editor.theme");
        assert_eq!(p.key(Key::Char('l')), Action::Set { key: "editor.theme", value: Some(Value::Text("Nord".into())) });
        goto(&mut p, "editor.animations");
        assert_eq!(p.key(Key::Space), Action::Set { key: "editor.animations", value: Some(Value::Bool(false)) }, "on by default, so off");
        goto(&mut p, "git.layout");
        assert_eq!(p.key(Key::Enter), Action::Set { key: "git.layout", value: Some(Value::Text("panes".into())) });
    }

    #[test]
    fn a_typed_value_is_checked_and_a_bad_one_stays_with_the_reason() {
        let mut p = page();
        goto(&mut p, "git.auto_fetch");
        p.key(Key::Enter);
        assert!(p.typing());
        for c in "five".chars() {
            p.key(Key::Char(c));
        }
        assert_eq!(p.key(Key::Enter), Action::None);
        assert!(p.typing(), "still open");
        assert!(layout(&p, 130).text.contains("a number of minutes"));
        for _ in 0..4 {
            p.key(Key::Backspace);
        }
        p.key(Key::Char('5'));
        assert_eq!(p.key(Key::Enter), Action::Set { key: "git.auto_fetch", value: Some(Value::Int(5)) });
        assert!(!p.typing());
    }

    #[test]
    fn r_resets_and_a_cleared_field_is_back_to_the_default() {
        let mut p = page();
        goto(&mut p, "editor.font_size");
        assert_eq!(p.key(Key::Char('r')), Action::Set { key: "editor.font_size", value: None });
        p.key(Key::Enter);
        for _ in 0..4 {
            p.key(Key::Backspace);
        }
        assert_eq!(p.key(Key::Enter), Action::Set { key: "editor.font_size", value: None });
    }

    #[test]
    fn list_entries_are_rows_that_are_added_edited_moved_and_deleted() {
        let mut p = page();
        goto(&mut p, "vnc.hosts");
        let text = layout(&p, 130).text;
        assert!(text.contains("test-vm") && text.contains("10.0.0.5:5901") && text.contains("+ add one"), "{text}");
        // Edit the second host's port.
        p.key(Key::Char('j'));
        p.key(Key::Char('j'));
        assert!(matches!(p.selected(), Some(Row::Entry(_, 1))));
        p.key(Key::Enter);
        assert!(layout(&p, 130).popup.is_some(), "the form floats");
        p.key(Key::Tab);
        p.key(Key::Tab);
        for _ in 0..4 {
            p.key(Key::Backspace);
        }
        for c in "99999".chars() {
            p.key(Key::Char(c));
        }
        assert_eq!(p.key(Key::Enter), Action::None, "refused");
        assert!(layout(&p, 130).popup.unwrap().text().contains("Port: a whole number from 1 to 65535"));
        for _ in 0..5 {
            p.key(Key::Backspace);
        }
        p.key(Key::Char('7'));
        let Action::Set { value: Some(Value::Records(rows)), .. } = p.key(Key::Enter) else { panic!() };
        assert_eq!(rows[1], ["build", "10.0.0.5", "7"]);
        // Delete the first, twice to be sure.
        p.key(Key::Char('k'));
        assert_eq!(p.key(Key::Char('d')), Action::None);
        let Action::Set { value: Some(Value::Records(rows)), .. } = p.key(Key::Char('d')) else { panic!() };
        assert_eq!(rows.len(), 1);
        // Add one: a port left empty takes its default.
        p.key(Key::Char('a'));
        for (i, text) in ["lab", "10.1.1.1"].iter().enumerate() {
            for c in text.chars() {
                p.key(Key::Char(c));
            }
            if i == 0 {
                p.key(Key::Enter);
            }
        }
        p.key(Key::Enter);
        let Action::Set { value: Some(Value::Records(rows)), .. } = p.key(Key::Enter) else { panic!() };
        assert_eq!(rows.last().unwrap(), &["lab", "10.1.1.1", "5900"]);
    }

    #[test]
    fn search_finds_settings_by_name_key_or_description_across_categories() {
        let mut p = page();
        p.key(Key::Char('/'));
        for c in "token".chars() {
            p.key(Key::Char(c));
        }
        let text = layout(&p, 130).text;
        assert!(text.contains("GitLab token") && text.contains("GitHub token") && text.contains("Jira token"), "{text}");
        assert!(!text.contains("Font size"));
        p.key(Key::Enter);
        assert!(matches!(p.selected(), Some(Row::Setting(s)) if s.key == "gitlab.token"));
        assert!(layout(&p, 130).text.contains("Credential Manager · never written to a file"));
    }

    #[test]
    fn a_token_is_typed_masked_and_tested_or_cleared() {
        let mut p = page();
        goto(&mut p, "gitlab.token");
        assert_eq!(p.key(Key::Char('t')), Action::TestSecret(Secret::GitLab));
        p.key(Key::Enter);
        for c in "glpat-abc".chars() {
            p.key(Key::Char(c));
        }
        let text = layout(&p, 130).text;
        assert!(text.contains("•••••••••") && !text.contains("glpat-abc"), "{text}");
        assert_eq!(p.key(Key::Enter), Action::SetSecret { secret: Secret::GitLab, token: Some("glpat-abc".into()) });
        assert_eq!(p.key(Key::Char('x')), Action::SetSecret { secret: Secret::GitLab, token: None });
    }

    #[test]
    fn a_projects_scope_shows_only_what_a_project_may_set_and_where_it_comes_from() {
        let mut snap = snap();
        snap.here = HashMap::from([("editor.indent_width", Value::Int(2))]);
        snap.inherited = HashMap::from([("editor.indent_width", Value::Int(4)), ("editor.tab_width", Value::Int(8))]);
        let mut p = SettingsPage::new(Scope::Project { root: PathBuf::from("/p"), name: "fenix".into() }, snap);
        goto(&mut p, "editor.indent_width");
        let text = layout(&p, 130).text;
        assert!(text.starts_with("\nSettings · fenix") || text.contains("Settings · fenix"), "{text}");
        assert!(text.contains("set here · yours: 4") && text.contains("from you"), "{text}");
        assert!(!text.contains("Font size") && !p.categories().contains(&Category::Appearance), "the theme isn't per-project");
        assert_eq!(p.key(Key::Char('r')), Action::Set { key: "editor.indent_width", value: None });
        assert!(text.contains("Project & tasks"), "{text}");
        p.key(Key::Tab);
        p.key(Key::Char('k'));
        assert!(layout(&p, 130).text.contains("Enter opens them."));
        assert_eq!(p.key(Key::Enter), Action::OpenProjectPage(PathBuf::from("/p")));
        assert_eq!(p.key(Key::Char('p')), Action::SwitchScope(Scope::You));
    }
}
