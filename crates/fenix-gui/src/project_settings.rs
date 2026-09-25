//! `SPC p ,`, a project's settings as a page: its kind, group, pin and
//! Jira key, then `.fenix/tools.json` -- tasks, language servers, the
//! debug launch and its environment -- as rows you edit in place instead
//! of hand-written JSON. Every change is checked by the same validation
//! `tools.json` gets when it's read; a bad value stays on its row with
//! the reason, and nothing invalid is ever written. The JSON remains the
//! source of truth: `e` opens it, and the page re-reads it on open.

use std::path::PathBuf;

use fenix_project::tools::{join_command_line, split_command_line, CommandSpec, ProjectTools};
use fenix_project::ProjectKind;

use crate::page::{fit, Grid, Key, Page, Role};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Field {
    Kind,
    Group,
    Pinned,
    Jira,
    Task(String),
    AddTask,
    Lsp(String),
    AddLsp,
    Program,
    Args,
    Cwd,
    Env(String),
    AddEnv,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    None,
    Close,
    /// Write these to `.fenix/tools.json` (already validated).
    SaveTools(ProjectTools),
    /// `None`: go back to detecting it.
    SetKind(Option<ProjectKind>),
    SetGroup(String),
    SetPinned(bool),
    SetJira(String),
    RunTask(String),
    OpenRaw,
}

pub struct Settings {
    pub root: PathBuf,
    pub detected: ProjectKind,
    pub declared: Option<ProjectKind>,
    pub group: String,
    pub pinned: bool,
    pub jira: String,
    pub tools: ProjectTools,
    /// `tools.json` exists but doesn't parse: the page shows why and
    /// only offers the raw file, rather than overwriting it.
    pub tools_error: Option<String>,
    pub focus: usize,
    /// The field being typed into, and its text so far.
    pub editing: Option<(Field, String)>,
    /// Why the last edit of a field was refused.
    pub problem: Option<(Field, String)>,
    /// `d` once asks; `d` again deletes.
    pub confirm_delete: Option<Field>,
    /// Shown under the header once: "saved", "restart the server".
    pub note: Option<String>,
}

impl Settings {
    pub fn new(root: PathBuf, detected: ProjectKind) -> Self {
        let (tools, tools_error) = match ProjectTools::read(&root) {
            Ok(tools) => (tools, None),
            Err(e) => (ProjectTools::default(), Some(e)),
        };
        Settings {
            root,
            detected,
            declared: None,
            group: String::new(),
            pinned: false,
            jira: String::new(),
            tools,
            tools_error,
            focus: 0,
            editing: None,
            problem: None,
            confirm_delete: None,
            note: None,
        }
    }

    pub fn fields(&self) -> Vec<Field> {
        let mut fields = vec![Field::Kind, Field::Group, Field::Pinned, Field::Jira];
        if self.tools_error.is_some() {
            return fields;
        }
        fields.extend(self.tools.tasks.keys().cloned().map(Field::Task));
        fields.push(Field::AddTask);
        fields.extend(self.tools.lsp.keys().cloned().map(Field::Lsp));
        fields.push(Field::AddLsp);
        fields.extend([Field::Program, Field::Args, Field::Cwd]);
        fields.extend(self.tools.launch.env.keys().cloned().map(Field::Env));
        fields.push(Field::AddEnv);
        fields
    }

    pub fn focused(&self) -> Option<Field> {
        self.fields().get(self.focus).cloned()
    }

    /// The text a field edits as.
    fn value(&self, field: &Field) -> String {
        match field {
            Field::Group => self.group.clone(),
            Field::Jira => self.jira.clone(),
            Field::Task(name) => self.tools.tasks.get(name).map(command_line).unwrap_or_default(),
            Field::Lsp(lang) => self.tools.lsp.get(lang).map(command_line).unwrap_or_default(),
            Field::Program => self.tools.launch.program.as_ref().map(|p| p.display().to_string()).unwrap_or_default(),
            Field::Args => self.tools.launch.args.as_deref().map(join_command_line).unwrap_or_default(),
            Field::Cwd => self.tools.launch.cwd.as_ref().map(|p| p.display().to_string()).unwrap_or_default(),
            Field::Env(key) => self.tools.launch.env.get(key).cloned().unwrap_or_default(),
            _ => String::new(),
        }
    }

    fn is_text(field: &Field) -> bool {
        !matches!(field, Field::Kind | Field::Pinned)
    }

    /// Whether Space means something on the focused row.
    pub fn claims_space(&self) -> bool {
        self.editing.is_some() || matches!(self.focused(), Some(Field::Pinned | Field::Kind))
    }

    pub fn key(&mut self, key: Key) -> Action {
        if self.editing.is_some() {
            match key {
                Key::Escape => self.editing = None,
                Key::Enter => return self.commit(),
                Key::Backspace => {
                    if let Some((_, text)) = &mut self.editing {
                        text.pop();
                    }
                }
                Key::Char(c) => self.type_text(&c.to_string()),
                Key::Space => self.type_text(" "),
                _ => {}
            }
            return Action::None;
        }
        let pending_delete = self.confirm_delete.take();
        let count = self.fields().len();
        let field = self.focused();
        match key {
            Key::Char('q') | Key::Escape => return Action::Close,
            Key::Char('j') | Key::Down | Key::Tab => self.focus = (self.focus + 1).min(count.saturating_sub(1)),
            Key::Char('k') | Key::Up | Key::BackTab => self.focus = self.focus.saturating_sub(1),
            Key::Char('g') => self.focus = 0,
            Key::Char('G') => self.focus = count.saturating_sub(1),
            Key::Char('e') => return Action::OpenRaw,
            Key::Char('t') => {
                if let Some(Field::Task(name)) = field {
                    return Action::RunTask(name);
                }
            }
            Key::Char('a') => {
                // Add to whichever section the cursor is in.
                let add = match field {
                    Some(Field::Task(_) | Field::AddTask) => Some(Field::AddTask),
                    Some(Field::Lsp(_) | Field::AddLsp) => Some(Field::AddLsp),
                    Some(Field::Env(_) | Field::AddEnv | Field::Program | Field::Args | Field::Cwd) => Some(Field::AddEnv),
                    _ => None,
                };
                if let Some(add) = add {
                    self.focus_on(&add);
                    self.editing = Some((add, String::new()));
                }
            }
            Key::Char('d') => {
                if let Some(field @ (Field::Task(_) | Field::Lsp(_) | Field::Env(_))) = field {
                    if pending_delete.as_ref() == Some(&field) {
                        return self.delete(&field);
                    }
                    self.confirm_delete = Some(field);
                }
            }
            Key::Char('h') | Key::Left => {
                if field == Some(Field::Kind) {
                    return self.cycle_kind(false);
                }
            }
            Key::Char('l') | Key::Right => {
                if field == Some(Field::Kind) {
                    return self.cycle_kind(true);
                }
            }
            Key::Char('i') | Key::Char('c') | Key::Enter | Key::Space => match field {
                Some(Field::Pinned) => {
                    self.pinned = !self.pinned;
                    return Action::SetPinned(self.pinned);
                }
                Some(Field::Kind) => return self.cycle_kind(true),
                Some(field) if Self::is_text(&field) => {
                    let text = if key == Key::Char('c') || matches!(field, Field::AddTask | Field::AddLsp | Field::AddEnv) { String::new() } else { self.value(&field) };
                    self.editing = Some((field, text));
                }
                _ => {}
            },
            _ => {}
        }
        Action::None
    }

    pub fn type_text(&mut self, text: &str) {
        if let Some((_, editing)) = &mut self.editing {
            editing.extend(text.chars().filter(|c| !c.is_control()));
        }
    }

    fn focus_on(&mut self, field: &Field) {
        if let Some(i) = self.fields().iter().position(|f| f == field) {
            self.focus = i;
        }
    }

    fn cycle_kind(&mut self, forward: bool) -> Action {
        // "detect", then each kind.
        let options: Vec<Option<ProjectKind>> = std::iter::once(None).chain(ProjectKind::ALL.into_iter().map(Some)).collect();
        let at = options.iter().position(|o| *o == self.declared).unwrap_or(0);
        let next = if forward { (at + 1) % options.len() } else { (at + options.len() - 1) % options.len() };
        self.declared = options[next];
        Action::SetKind(self.declared)
    }

    fn refuse(&mut self, field: Field, why: String) -> Action {
        self.problem = Some((field, why));
        Action::None
    }

    /// Tries `tools` as the new settings: saved if they validate,
    /// refused on `field` if not.
    fn save(&mut self, field: Field, tools: ProjectTools) -> Action {
        match tools.to_json() {
            Ok(_) => {
                self.tools = tools.clone();
                self.problem = None;
                if matches!(field, Field::Lsp(_) | Field::AddLsp) {
                    self.note = Some("language servers read their settings once -- :lsp-restart to use this".to_string());
                }
                Action::SaveTools(tools)
            }
            Err(e) => self.refuse(field, e),
        }
    }

    fn commit(&mut self) -> Action {
        let Some((field, text)) = self.editing.take() else { return Action::None };
        let text_trimmed = text.trim().to_string();
        let mut tools = self.tools.clone();
        match &field {
            Field::Group => {
                self.group = text_trimmed.clone();
                Action::SetGroup(text_trimmed)
            }
            Field::Jira => {
                let key = text_trimmed.to_uppercase();
                if !key.is_empty() && !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                    return self.refuse(field, "a Jira key is letters and digits, like FNX".to_string());
                }
                self.jira = key.clone();
                Action::SetJira(key)
            }
            Field::Task(name) | Field::Lsp(name) => {
                let map = if matches!(field, Field::Task(_)) { &mut tools.tasks } else { &mut tools.lsp };
                match parse_command(&text_trimmed, map.get(name)) {
                    Ok(spec) => {
                        map.insert(name.clone(), spec);
                        self.save(field, tools)
                    }
                    Err(e) => self.refuse(field, e),
                }
            }
            Field::AddTask | Field::AddLsp => {
                let Some((name, command)) = text_trimmed.split_once('=') else {
                    return self.refuse(field, "write it as name = command, e.g. test = uv run pytest".to_string());
                };
                let name = name.trim().to_string();
                if name.is_empty() {
                    return self.refuse(field, "it needs a name before the =".to_string());
                }
                let is_task = field == Field::AddTask;
                let map = if is_task { &mut tools.tasks } else { &mut tools.lsp };
                if map.contains_key(&name) {
                    return self.refuse(field, format!("there's already one called {name}"));
                }
                match parse_command(command.trim(), None) {
                    Ok(spec) => {
                        map.insert(name.clone(), spec);
                        let action = self.save(field, tools);
                        if matches!(action, Action::SaveTools(_)) {
                            self.focus_on(&if is_task { Field::Task(name) } else { Field::Lsp(name) });
                        }
                        action
                    }
                    Err(e) => self.refuse(field, e),
                }
            }
            Field::Program => {
                tools.launch.program = (!text_trimmed.is_empty()).then(|| PathBuf::from(&text_trimmed));
                self.save(field, tools)
            }
            Field::Cwd => {
                tools.launch.cwd = (!text_trimmed.is_empty()).then(|| PathBuf::from(&text_trimmed));
                self.save(field, tools)
            }
            Field::Args => match split_command_line(&text_trimmed) {
                Ok(args) => {
                    tools.launch.args = (!args.is_empty()).then_some(args);
                    self.save(field, tools)
                }
                Err(e) => self.refuse(field, e),
            },
            Field::Env(key) => {
                tools.launch.env.insert(key.clone(), text.clone());
                self.save(field, tools)
            }
            Field::AddEnv => {
                let Some((key, value)) = text.split_once('=') else {
                    return self.refuse(field, "write it as NAME = value".to_string());
                };
                let key = key.trim().to_string();
                tools.launch.env.insert(key.clone(), value.trim().to_string());
                let action = self.save(field, tools);
                if matches!(action, Action::SaveTools(_)) {
                    self.focus_on(&Field::Env(key));
                }
                action
            }
            Field::Kind | Field::Pinned => Action::None,
        }
    }

    fn delete(&mut self, field: &Field) -> Action {
        let mut tools = self.tools.clone();
        match field {
            Field::Task(name) => {
                tools.tasks.remove(name);
            }
            Field::Lsp(lang) => {
                tools.lsp.remove(lang);
            }
            Field::Env(key) => {
                tools.launch.env.remove(key);
            }
            _ => return Action::None,
        }
        let action = self.save(field.clone(), tools);
        self.focus = self.focus.min(self.fields().len().saturating_sub(1));
        action
    }
}

fn command_line(spec: &CommandSpec) -> String {
    let mut all = vec![spec.executable.clone()];
    all.extend(spec.args.iter().cloned());
    join_command_line(&all)
}

/// A typed command line as a `CommandSpec`, keeping `existing`'s working
/// directory and environment -- the one-line edit only covers the
/// program and its arguments.
fn parse_command(line: &str, existing: Option<&CommandSpec>) -> Result<CommandSpec, String> {
    let mut words = split_command_line(line)?;
    if words.is_empty() {
        return Err("it needs a program to run".to_string());
    }
    let executable = words.remove(0);
    let mut spec = existing.cloned().unwrap_or_default();
    spec.executable = executable;
    spec.args = words;
    Ok(spec)
}

/// The page as the settings page's "Project & tasks" section shows it:
/// `cols` wide, from its first row, with no header of its own (the
/// settings page names the project already).
pub fn layout(settings: &Settings, cols: usize) -> Page {
    let (left, width) = (0, cols.min(100));
    let mut g = Grid::new();
    let mut y = 0;
    if let Some(note) = &settings.note {
        g.put(y, left + 2, &fit(note, width - 4), Role::Warn);
        y += 1;
    }

    let label_x = left + 2;
    let value_x = left + 18;
    let value_room = width.saturating_sub(20);
    let focused = settings.focused();
    let mut section = "";
    for field in settings.fields() {
        let heading = match &field {
            Field::Kind | Field::Group | Field::Pinned | Field::Jira => "General",
            Field::Task(_) | Field::AddTask => "Tasks",
            Field::Lsp(_) | Field::AddLsp => "Language servers",
            Field::Program | Field::Args | Field::Cwd => "Debug launch",
            Field::Env(_) | Field::AddEnv => "Environment",
        };
        if heading != section {
            if !section.is_empty() {
                y += 1;
            }
            let title = match heading {
                "Tasks" => format!("Tasks · {}", settings.tools.tasks.len()),
                other => other.to_string(),
            };
            g.heading(y, left, width, &title);
            section = heading;
            y += 1;
        }
        if focused.as_ref() == Some(&field) {
            g.focus(y, left..left + width);
        }
        let label = match &field {
            Field::Kind => "Kind".to_string(),
            Field::Group => "Group".to_string(),
            Field::Pinned => "Pinned".to_string(),
            Field::Jira => "Jira".to_string(),
            Field::Task(name) | Field::Lsp(name) | Field::Env(name) => name.clone(),
            Field::AddTask | Field::AddLsp | Field::AddEnv => "+ add".to_string(),
            Field::Program => "program".to_string(),
            Field::Args => "args".to_string(),
            Field::Cwd => "cwd".to_string(),
        };
        let add_row = matches!(field, Field::AddTask | Field::AddLsp | Field::AddEnv);
        g.put(y, label_x, &fit(&label, 15), if add_row { Role::Accent } else { Role::Muted });
        let editing = settings.editing.as_ref().filter(|(f, _)| *f == field);
        if let Some((_, text)) = editing {
            let end = g.put(y, value_x, &fit(&format!("{text}▏"), value_room), Role::Title);
            g.panels.push((y, value_x - 1..end + 1));
            if add_row && text.is_empty() {
                let hint = match field {
                    Field::AddEnv => "NAME = value",
                    Field::AddLsp => "language = command",
                    _ => "name = command",
                };
                g.put(y, end + 2, hint, Role::Muted);
            }
        } else {
            match &field {
                Field::Kind => {
                    let text = match settings.declared {
                        Some(k) => format!("‹ {} ›", k.label()),
                        None => format!("‹ detect: {} ›", settings.detected.label()),
                    };
                    g.put(y, value_x, &text, Role::Title);
                }
                Field::Pinned => {
                    g.put(y, value_x, if settings.pinned { "[x]" } else { "[ ]" }, if settings.pinned { Role::Good } else { Role::Muted });
                }
                _ if add_row => {}
                _ => {
                    let value = settings.value(&field);
                    let shown = if value.is_empty() { "—".to_string() } else { value };
                    let end = g.put(y, value_x, &fit(&shown, value_room), Role::Text);
                    // A cwd or program that isn't there is worth saying.
                    let missing = match &field {
                        Field::Cwd | Field::Program => {
                            let v = settings.value(&field);
                            !v.is_empty() && !settings.root.join(&v).exists()
                        }
                        _ => false,
                    };
                    if missing && end + 12 < left + width {
                        g.put(y, end + 2, "not found", Role::Warn);
                    }
                }
            }
        }
        y += 1;
        if let Some((_, why)) = settings.problem.as_ref().filter(|(f, _)| *f == field) {
            g.put(y, value_x, &fit(why, value_room), Role::Bad);
            y += 1;
        }
        if settings.confirm_delete.as_ref() == Some(&field) {
            g.put(y, value_x, &fit(&format!("d again deletes {label}"), value_room), Role::Warn);
            y += 1;
        }
    }
    if let Some(error) = &settings.tools_error {
        y += 1;
        g.heading(y, left, width, "tools.json");
        y += 1;
        for line in crate::page::wrap(error, width - 4) {
            g.put(y, left + 2, &line, Role::Bad);
            y += 1;
        }
        g.put(y, left + 2, "It isn't changed from here until it's valid -- e opens it.", Role::Muted);
    }
    let keys: &[(&str, &str)] = if settings.editing.is_some() {
        &[("Enter", "save"), ("Esc", "cancel")]
    } else {
        &[("Enter", "edit"), ("a", "add"), ("d", "delete"), ("t", "run task"), ("e", "raw JSON"), ("q", "close")]
    };
    g.keys(left, width, keys);
    g.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(name: &str, tools_json: Option<&str>) -> (Settings, PathBuf) {
        let root = std::env::temp_dir().join(format!("fenix-settings-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join(".fenix")).unwrap();
        if let Some(json) = tools_json {
            std::fs::write(root.join(".fenix/tools.json"), json).unwrap();
        }
        (Settings::new(root.clone(), ProjectKind::Python), root)
    }

    fn type_in(s: &mut Settings, text: &str) {
        for c in text.chars() {
            s.key(if c == ' ' { Key::Space } else { Key::Char(c) });
        }
    }

    #[test]
    fn tasks_servers_and_launch_are_listed_from_tools_json() {
        let (s, root) = settings("list", Some(r#"{"tasks":{"test":{"executable":"uv","args":["run","pytest"]}},"lsp":{"python":{"executable":"basedpyright-langserver","args":["--stdio"]}},"launch":{"program":"src/main.py","cwd":"nowhere","env":{"MODE":"dev"}}}"#));
        let text = layout(&s, 100).text;
        for needle in ["TASKS · 1", "uv run pytest", "LANGUAGE SERVERS", "basedpyright-langserver --stdio", "DEBUG LAUNCH", "src/main.py", "not found", "MODE", "dev", "detect: Python"] {
            assert!(text.contains(needle), "{needle:?} missing:\n{text}");
        }
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn editing_a_task_keeps_its_cwd_and_saves_valid_tools() {
        let (mut s, root) = settings("edit", Some(r#"{"tasks":{"test":{"executable":"uv","args":["run","pytest"],"cwd":"sub"}}}"#));
        s.focus_on(&Field::Task("test".into()));
        s.key(Key::Char('c'));
        type_in(&mut s, r#"uv run pytest -q "tests dir""#);
        let action = s.key(Key::Enter);
        let Action::SaveTools(tools) = action else { panic!("{action:?}") };
        let task = &tools.tasks["test"];
        assert_eq!(task.args, ["run", "pytest", "-q", "tests dir"]);
        assert_eq!(task.cwd.as_deref(), Some(std::path::Path::new("sub")));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn adding_needs_a_name_and_bad_input_is_refused_on_its_row() {
        let (mut s, root) = settings("add", None);
        s.focus_on(&Field::AddTask);
        s.key(Key::Enter);
        type_in(&mut s, "just a command");
        assert_eq!(s.key(Key::Enter), Action::None);
        assert!(layout(&s, 100).text.contains("write it as name = command"));
        s.key(Key::Enter);
        type_in(&mut s, "lint = uv run ruff check");
        assert!(matches!(s.key(Key::Enter), Action::SaveTools(t) if t.tasks["lint"].executable == "uv"));
        assert_eq!(s.focused(), Some(Field::Task("lint".into())), "the new task is selected");
        s.key(Key::Char('c'));
        type_in(&mut s, r#"open "unclosed"#);
        assert_eq!(s.key(Key::Enter), Action::None);
        assert!(s.problem.as_ref().is_some_and(|(_, why)| why.contains("quote")));
        assert_eq!(s.tools.tasks["lint"].args, ["run", "ruff", "check"], "the refused edit changed nothing");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn delete_asks_first_and_general_rows_ask_for_their_own_actions() {
        let (mut s, root) = settings("general", Some(r#"{"tasks":{"a":{"executable":"x"}}}"#));
        s.focus_on(&Field::Task("a".into()));
        assert_eq!(s.key(Key::Char('d')), Action::None);
        assert!(matches!(s.key(Key::Char('d')), Action::SaveTools(t) if t.tasks.is_empty()));
        s.focus_on(&Field::Pinned);
        assert_eq!(s.key(Key::Space), Action::SetPinned(true));
        s.focus_on(&Field::Kind);
        assert_eq!(s.key(Key::Char('l')), Action::SetKind(Some(ProjectKind::Python)));
        assert_eq!(s.key(Key::Char('h')), Action::SetKind(None));
        s.focus_on(&Field::Jira);
        s.key(Key::Enter);
        type_in(&mut s, "fnx");
        assert_eq!(s.key(Key::Enter), Action::SetJira("FNX".into()));
        s.focus_on(&Field::Group);
        s.key(Key::Enter);
        type_in(&mut s, "Mission ops");
        assert_eq!(s.key(Key::Enter), Action::SetGroup("Mission ops".into()));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_broken_tools_json_is_shown_not_overwritten() {
        let (s, root) = settings("broken", Some("{ nope"));
        assert!(s.tools_error.is_some());
        assert_eq!(s.fields().len(), 4, "only the general rows");
        assert!(layout(&s, 90).text.contains("e opens it"));
        let _ = std::fs::remove_dir_all(root);
    }
}
