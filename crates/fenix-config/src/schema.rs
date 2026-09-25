//! Every setting, declared once: its key in `settings.toml`, where it
//! goes on the settings page, what kind of value it takes and what it's
//! for. Loading, checking, saving, the settings page and the README's
//! settings table all read this list, so they can't disagree.

use std::path::PathBuf;
use std::sync::LazyLock;

use toml_edit::{value, Array, ArrayOfTables, InlineTable, Item, Table};

use crate::{Config, JiraBlocked, Secret};

/// A setting's value, independent of how it's stored.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Bool(bool),
    Int(i64),
    Float(f64),
    Text(String),
    List(Vec<String>),
    /// Named entries, in order: `[lsp.servers] python = "..."`.
    Map(Vec<(String, String)>),
    /// Rows of fields, as the `Kind::Records` names them.
    Records(Vec<Vec<String>>),
}

/// One field of a `Kind::Records` row.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Field {
    pub name: &'static str,
    pub label: &'static str,
    /// Whole numbers in this range, for a field that's a number.
    pub range: Option<(i64, i64)>,
    /// Used when the field is left out.
    pub default: Option<&'static str>,
}

/// What a setting takes, and so how it's edited and checked.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind {
    Bool,
    Int { min: i64, max: i64 },
    Float { min: f64, max: f64 },
    /// A number of minutes; `5`, `5m` and `1h` are all accepted.
    Minutes,
    Text,
    Path,
    /// One of these words.
    Choice(&'static [&'static str]),
    /// A theme's name; the choices are the themes the editor has.
    Theme,
    /// A font's name; the choices are the fonts installed.
    Font,
    /// A list of words (`["alex", "sam"]`).
    List,
    /// Named entries; `key` and `value` label the two halves.
    Map { key: &'static str, value: &'static str, paths: bool },
    Records(&'static [Field]),
    /// A token: typed masked on the page, and an environment variable
    /// overrides it.
    Secret(Secret),
}

/// Where a setting sits on the page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Category {
    Editor,
    Appearance,
    Files,
    Completion,
    Git,
    Forges,
    Jira,
    Embedded,
    Vnc,
    Documents,
    Session,
}

impl Category {
    pub const ALL: [Category; 11] = [
        Category::Editor,
        Category::Appearance,
        Category::Files,
        Category::Completion,
        Category::Git,
        Category::Forges,
        Category::Jira,
        Category::Embedded,
        Category::Vnc,
        Category::Documents,
        Category::Session,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Category::Editor => "Editor",
            Category::Appearance => "Appearance",
            Category::Files => "Files & explorer",
            Category::Completion => "Completion & LSP",
            Category::Git => "Git",
            Category::Forges => "Forges",
            Category::Jira => "Jira & agenda",
            Category::Embedded => "Embedded & MIB",
            Category::Vnc => "VNC",
            Category::Documents => "Documents & workspaces",
            Category::Session => "Windows & session",
        }
    }
}

type Get = fn(&Config) -> Option<Value>;
type Set = fn(&mut Config, Option<Value>) -> Result<(), String>;

/// One setting.
pub struct Setting {
    /// Its key in `settings.toml`: `editor.font_size`.
    pub key: &'static str,
    pub category: Category,
    pub label: &'static str,
    pub help: &'static str,
    pub kind: Kind,
    /// What applies when it isn't set, as the page shows it.
    pub default: &'static str,
    /// Whether a project's own settings may set it.
    pub project: bool,
    /// Whether it only takes effect after a restart.
    pub restart: bool,
    get: Get,
    set: Set,
}

impl std::fmt::Debug for Setting {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Setting").field("key", &self.key).finish()
    }
}

impl Setting {
    /// Its value in `config`; `None` when it isn't set.
    pub fn get(&self, config: &Config) -> Option<Value> {
        (self.get)(config)
    }

    /// Sets it in `config`, after checking it; `None` unsets it.
    pub fn set(&self, config: &mut Config, value: Option<Value>) -> Result<(), String> {
        if let Some(v) = &value {
            self.kind.check(v)?;
        }
        (self.set)(config, value)
    }

    fn project(mut self) -> Self {
        self.project = true;
        self
    }

    fn restart(mut self) -> Self {
        self.restart = true;
        self
    }

    fn default(mut self, default: &'static str) -> Self {
        self.default = default;
        self
    }
}

fn s(key: &'static str, category: Category, label: &'static str, kind: Kind, help: &'static str, (get, set): (Get, Set)) -> Setting {
    Setting { key, category, label, help, kind, default: "", project: false, restart: false, get, set }
}

// -- Conversions between a field's type and a `Value` ---------------------

fn text_get(v: &Option<String>) -> Option<Value> {
    v.clone().map(Value::Text)
}
fn text_set(v: Option<Value>) -> Result<Option<String>, String> {
    match v {
        None => Ok(None),
        Some(Value::Text(t)) => Ok(Some(t)),
        Some(other) => Err(format!("expected text, got {}", other.describe())),
    }
}
fn path_get(v: &Option<PathBuf>) -> Option<Value> {
    v.as_ref().map(|p| Value::Text(p.display().to_string()))
}
fn path_set(v: Option<Value>) -> Result<Option<PathBuf>, String> {
    text_set(v).map(|t| t.map(PathBuf::from))
}
fn bool_get(v: &Option<bool>) -> Option<Value> {
    v.map(Value::Bool)
}
fn bool_set(v: Option<Value>) -> Result<Option<bool>, String> {
    match v {
        None => Ok(None),
        Some(Value::Bool(b)) => Ok(Some(b)),
        Some(other) => Err(format!("expected true or false, got {}", other.describe())),
    }
}
fn int<T: TryFrom<i64> + Into<i64> + Copy>(v: &Option<T>) -> Option<Value> {
    v.map(|n| Value::Int(n.into()))
}
fn int_set<T: TryFrom<i64>>(v: Option<Value>) -> Result<Option<T>, String> {
    match v {
        None => Ok(None),
        Some(Value::Int(n)) => T::try_from(n).map(Some).map_err(|_| format!("{n} is out of range")),
        Some(other) => Err(format!("expected a whole number, got {}", other.describe())),
    }
}
fn usize_get(v: &Option<usize>) -> Option<Value> {
    v.map(|n| Value::Int(n as i64))
}
fn usize_set(v: Option<Value>) -> Result<Option<usize>, String> {
    int_set::<i64>(v)?.map(|n| usize::try_from(n).map_err(|_| format!("{n} is out of range"))).transpose()
}
fn u64_get(v: &Option<u64>) -> Option<Value> {
    v.map(|n| Value::Int(n as i64))
}
fn u64_set(v: Option<Value>) -> Result<Option<u64>, String> {
    int_set::<i64>(v)?.map(|n| u64::try_from(n).map_err(|_| format!("{n} is out of range"))).transpose()
}
fn f32_get(v: &Option<f32>) -> Option<Value> {
    v.map(|n| Value::Float(n as f64))
}
fn f32_set(v: Option<Value>) -> Result<Option<f32>, String> {
    match v {
        None => Ok(None),
        Some(Value::Float(n)) => Ok(Some(n as f32)),
        Some(Value::Int(n)) => Ok(Some(n as f32)),
        Some(other) => Err(format!("expected a number, got {}", other.describe())),
    }
}
fn list_get(v: &[String]) -> Option<Value> {
    (!v.is_empty()).then(|| Value::List(v.to_vec()))
}
fn list_set(v: Option<Value>) -> Result<Vec<String>, String> {
    match v {
        None => Ok(Vec::new()),
        Some(Value::List(l)) => Ok(l),
        Some(Value::Text(t)) => Ok(crate::names(&t)),
        Some(other) => Err(format!("expected a list, got {}", other.describe())),
    }
}
fn map_get(v: &[(String, String)]) -> Option<Value> {
    (!v.is_empty()).then(|| Value::Map(v.to_vec()))
}
fn map_set(v: Option<Value>) -> Result<Vec<(String, String)>, String> {
    match v {
        None => Ok(Vec::new()),
        Some(Value::Map(m)) => Ok(m),
        Some(other) => Err(format!("expected named entries, got {}", other.describe())),
    }
}
fn path_map_get(v: &[(String, PathBuf)]) -> Option<Value> {
    (!v.is_empty()).then(|| Value::Map(v.iter().map(|(k, p)| (k.clone(), p.display().to_string())).collect()))
}
fn path_map_set(v: Option<Value>) -> Result<Vec<(String, PathBuf)>, String> {
    map_set(v).map(|m| m.into_iter().map(|(k, p)| (k, PathBuf::from(p))).collect())
}

/// How a Blocked meaning is written: `flag`, `local`, or `ID: Name`.
pub fn blocked_text(b: &JiraBlocked) -> String {
    match b {
        JiraBlocked::Flag => "flag".into(),
        JiraBlocked::Local => "local".into(),
        JiraBlocked::Status { id, name } => format!("{id}: {name}"),
    }
}

pub fn blocked_parse(text: &str) -> Result<JiraBlocked, String> {
    match text.trim() {
        "flag" => Ok(JiraBlocked::Flag),
        "local" => Ok(JiraBlocked::Local),
        other => match other.split_once(':') {
            Some((id, name)) if !id.trim().is_empty() => Ok(JiraBlocked::Status { id: id.trim().into(), name: name.trim().into() }),
            _ => Err(format!("\"{other}\": expected flag, local, or a status as ID: Name")),
        },
    }
}

macro_rules! field {
    ($f:ident, $get:expr, $set:expr) => {
        (|c: &Config| $get(&c.$f), |c: &mut Config, v: Option<Value>| {
            c.$f = $set(v)?;
            Ok(())
        })
    };
}

const HOST: &[Field] = &[
    Field { name: "name", label: "Name", range: None, default: None },
    Field { name: "host", label: "Host", range: None, default: None },
    Field { name: "port", label: "Port", range: Some((1, 65535)), default: Some("5900") },
];

static SETTINGS: LazyLock<Vec<Setting>> = LazyLock::new(|| {
    use Category::*;
    vec![
        // Editor
        s("editor.indent_width", Editor, "Indent width", Kind::Int { min: 1, max: 16 }, "Spaces a Tab or >> inserts; Fenix always indents with spaces.", field!(indent_width, usize_get, usize_set)).default("4").project(),
        s("editor.tab_width", Editor, "Tab width", Kind::Int { min: 1, max: 16 }, "Columns a tab character already in a file takes up.", field!(tab_width, usize_get, usize_set)).default("8").project(),
        s("editor.iskeyword_extra", Editor, "Word characters", Kind::Text, "Characters besides letters, digits and _ that count as part of a word, for w, * and completion.", field!(iskeyword_extra, text_get, text_set)).default("none extra").project(),
        // Appearance
        s("editor.theme", Appearance, "Theme", Kind::Theme, "The colour theme; h and l preview each one.", field!(theme, text_get, text_set)).default("Orbit Dark"),
        s("editor.font_family", Appearance, "Font", Kind::Font, "A monospace font installed on this machine; h and l go through them.", field!(font_family, text_get, text_set)).default("the system's monospace font"),
        s("editor.font_size", Appearance, "Font size", Kind::Float { min: 6.0, max: 48.0 }, "Text size, in points.", field!(font_size, f32_get, f32_set)).default("16"),
        s("editor.animations", Appearance, "Animations", Kind::Bool, "Smooth scrolling and the caret's fade.", field!(animations, bool_get, bool_set)).default("on"),
        // Files
        s("editor.watch_files", Files, "Watch files on disk", Kind::Bool, "Notice when an open file changes on disk, and reload it when you haven't edited it.", field!(watch_files, bool_get, bool_set)).default("on"),
        // Completion & LSP
        s("completion.symbols_file", Completion, "Extra words file", Kind::Path, "A text file of words, one per line, offered by completion everywhere.", field!(completion_symbols_file, path_get, path_set)),
        s("snippets.builtin", Completion, "Built-in snippets", Kind::Bool, "Offer the snippets that come with Fenix; yours and a project's always are. SPC i S manages them.", field!(snippets_builtin, bool_get, bool_set)).default("on"),
        s("lsp.servers", Completion, "Language servers", Kind::Map { key: "Language", value: "Command", paths: false }, "A language server to run for a language, as the command line that starts it.", field!(lsp_servers, map_get, map_set)),
        // Git
        s("git.base_branch", Git, "Base branch", Kind::Text, "The branch pull requests and comparisons start from.", field!(git_base_branch, text_get, text_set)).default("main or master").project(),
        s("git.reviewers", Git, "Reviewers", Kind::List, "Usernames asked to review a new pull request.", (|c: &Config| list_get(&c.git_reviewers), |c: &mut Config, v| {
            c.git_reviewers = list_set(v)?;
            Ok(())
        })).project(),
        s("git.auto_fetch", Git, "Fetch every", Kind::Minutes, "Fetch the focused repository in the background this often, in minutes.", field!(git_auto_fetch_minutes, u64_get, u64_set)).default("never"),
        s("git.layout", Git, "SPC g g opens", Kind::Choice(&["page", "panes"]), "The Git status page, or the older seven-pane panel.", field!(git_layout, text_get, text_set)).default("page"),
        s("git.graph_style", Git, "Graph lines", Kind::Choice(&["ascii", "unicode"]), "Characters the commit graph is drawn with; unicode needs a font with box-drawing glyphs.", field!(git_graph_style, text_get, text_set)).default("ascii"),
        s("git.graph_limit", Git, "Graph commits", Kind::Int { min: 10, max: 100_000 }, "How many commits the graph view reads.", field!(git_graph_limit, usize_get, usize_set)).default("200"),
        // Forges
        s("gitlab.base_url", Forges, "GitLab server", Kind::Text, "The GitLab instance's address, like https://gitlab.example.com.", field!(gitlab_base_url, text_get, text_set)),
        s("gitlab.token", Forges, "GitLab token", Kind::Secret(Secret::GitLab), "A personal access token with the api scope.", field!(gitlab_token, text_get, text_set)),
        s("github.token", Forges, "GitHub token", Kind::Secret(Secret::GitHub), "Used when the GitHub CLI isn't signed in (gh auth login).", field!(github_token, text_get, text_set)),
        // Jira & agenda
        s("jira.base_url", Jira, "Jira server", Kind::Text, "Your Jira Server or Data Center's address.", field!(jira_base_url, text_get, text_set)),
        s("jira.token", Jira, "Jira token", Kind::Secret(Secret::Jira), "A personal access token for the Jira server.", field!(jira_token, text_get, text_set)),
        s("jira.sync_minutes", Jira, "Sync every", Kind::Minutes, "How often linked agenda tasks are brought up to date, in minutes.", (|c: &Config| int(&c.jira_sync_minutes), |c: &mut Config, v| {
            c.jira_sync_minutes = int_set(v)?;
            Ok(())
        })).default("10"),
        s("jira.projects", Jira, "Projects", Kind::Map { key: "Key", value: "Name", paths: false }, "Jira projects the dashboard tracks.", field!(jira_projects, map_get, map_set)),
        s("jira.users", Jira, "Users", Kind::Map { key: "Id", value: "Name", paths: false }, "People the dashboard tracks.", field!(jira_users, map_get, map_set)),
        s("jira.blocked", Jira, "What Blocked means", Kind::Map { key: "Project", value: "Meaning", paths: false }, "Per project: flag, local, or the status to move to, as ID: Name. Learned the first time you block a task.", (|c: &Config| (!c.jira_blocked.is_empty()).then(|| Value::Map(c.jira_blocked.iter().map(|(p, b)| (p.clone(), blocked_text(b))).collect())), |c: &mut Config, v| {
            c.jira_blocked = map_set(v)?.into_iter().map(|(p, t)| blocked_parse(&t).map(|b| (p, b))).collect::<Result<_, _>>()?;
            Ok(())
        })),
        s("jira.priorities", Jira, "Priority names", Kind::Map { key: "Jira priority", value: "Agenda priority", paths: false }, "How a Jira priority maps onto the agenda's.", field!(jira_priority_map, map_get, map_set)),
        s("agenda.categories", Jira, "Agenda categories", Kind::List, "Categories offered when you file a task.", (|c: &Config| list_get(&c.agenda_categories), |c: &mut Config, v| {
            c.agenda_categories = list_set(v)?;
            Ok(())
        })),
        s("agenda.worklog_round", Jira, "Round worklogs to", Kind::Minutes, "Time logged to Jira is rounded to this many minutes.", (|c: &Config| int(&c.agenda_worklog_round), |c: &mut Config, v| {
            c.agenda_worklog_round = int_set(v)?;
            Ok(())
        })).default("15"),
        // Embedded & MIB
        s("embedded.arduino_cli", Embedded, "arduino-cli", Kind::Path, "Where arduino-cli is, when it isn't found by itself.", field!(embedded_arduino_cli, path_get, path_set)).default("found on PATH"),
        s("embedded.clangd", Embedded, "clangd", Kind::Path, "Where clangd is, when it isn't found by itself.", field!(embedded_clangd, path_get, path_set)).default("found on PATH"),
        s("embedded.arduino_language_server", Embedded, "Arduino language server", Kind::Path, "Where arduino-language-server is, when it isn't found by itself.", field!(embedded_arduino_language_server, path_get, path_set)).default("downloaded when needed"),
        s("mib.roots", Embedded, "MIB roots", Kind::Map { key: "Name", value: "Folder", paths: true }, "Folders holding a MIB database.", field!(mib_roots, path_map_get, path_map_set)),
        s("mib.telecommand_template", Embedded, "Telecommand template", Kind::Text, "How a telecommand is written; {name} and {args} are filled in.", field!(mib_telecommand_template, text_get, text_set)),
        s("mib.telecommand_argument_template", Embedded, "Argument template", Kind::Text, "How each argument is written; {name} and {value} are filled in.", field!(mib_telecommand_argument_template, text_get, text_set)),
        s("mib.telecommand_argument_separator", Embedded, "Argument separator", Kind::Text, "What goes between arguments.", field!(mib_telecommand_argument_separator, text_get, text_set)),
        // VNC
        s("vnc.hosts", Vnc, "Hosts", Kind::Records(HOST), "Machines SPC v connects to. No passwords: every host is taken to be on a trusted network.", (|c: &Config| (!c.vnc_hosts.is_empty()).then(|| Value::Records(c.vnc_hosts.iter().map(|(n, h, p)| vec![n.clone(), h.clone(), p.to_string()]).collect())), |c: &mut Config, v| {
            c.vnc_hosts = match v {
                None => Vec::new(),
                Some(Value::Records(rows)) => rows.into_iter().map(|r| (r[0].clone(), r[1].clone(), r[2].parse().unwrap_or(5900))).collect(),
                Some(other) => return Err(format!("expected hosts, got {}", other.describe())),
            };
            Ok(())
        })),
        // Documents & workspaces
        s("documents", Documents, "Documents", Kind::Map { key: "Name", value: "File", paths: true }, "The SPC r f document index: a name and the file it opens.", field!(documents, path_map_get, path_map_set)),
        s("workspaces", Documents, "Workspace shelf", Kind::Map { key: "Name", value: "Opens", paths: false }, "SPC TAB f: git, jira, docker, vnc:HOST, project:PATH, or nothing.", field!(workspaces, map_get, map_set)),
        // Windows & session
        s("session.restore_windows", Session, "Reopen windows", Kind::Bool, "Put Fenix's windows back where they were, on the monitors they were on.", field!(restore_windows, bool_get, bool_set)).default("on").restart(),
        s("session.restore_session", Session, "Reopen workspaces", Kind::Bool, "Reopen the workspaces and files you had open.", field!(restore_session, bool_get, bool_set)).default("on").restart(),
        s("session.workspace_per_project", Session, "A workspace per project", Kind::Bool, "Opening a project gives it a workspace of its own.", field!(workspace_per_project, bool_get, bool_set)).default("on"),
    ]
});

/// Every setting, in page order.
pub fn settings() -> &'static [Setting] {
    &SETTINGS
}

/// The setting at `key`.
pub fn setting(key: &str) -> Option<&'static Setting> {
    settings().iter().find(|s| s.key == key)
}

/// Every setting as a Markdown table, by category -- the README's copy,
/// which a test keeps in step with this list.
pub fn markdown_table() -> String {
    let mut out = String::from("| Setting | Takes | Default | What it does |
|---|---|---|---|
");
    for category in Category::ALL {
        out.push_str(&format!("| **{}** | | | |
", category.label()));
        for s in settings().iter().filter(|s| s.category == category) {
            let takes = match s.kind {
                Kind::Bool => "true / false".to_string(),
                Kind::Int { min, max } => format!("{min}–{max}"),
                Kind::Float { min, max } => format!("{min}–{max}"),
                Kind::Minutes => "minutes".to_string(),
                Kind::Text | Kind::Font | Kind::Theme => "text".to_string(),
                Kind::Path => "a path".to_string(),
                Kind::Choice(c) => c.join(" / "),
                Kind::List => "a list".to_string(),
                Kind::Map { key, value, .. } => format!("{} = {}", key.to_lowercase(), value.to_lowercase()),
                Kind::Records(fields) => format!("[[tables]] of {}", fields.iter().map(|f| f.name).collect::<Vec<_>>().join(", ")),
                Kind::Secret(_) => "text (a token)".to_string(),
            };
            let mut help = s.help.replace('|', r"\|");
            if s.project {
                help.push_str(" *A project can set it.*");
            }
            if s.restart {
                help.push_str(" *Needs a restart.*");
            }
            let key = format!("`{}`", s.key);
            out.push_str(&format!("| {key} | {takes} | {} | {help} |
", if s.default.is_empty() { "–" } else { s.default }));
        }
    }
    out
}

// -- Values ----------------------------------------------------------------

impl Value {
    fn describe(&self) -> String {
        match self {
            Value::Bool(b) => b.to_string(),
            Value::Int(n) => n.to_string(),
            Value::Float(n) => n.to_string(),
            Value::Text(t) => format!("\"{t}\""),
            Value::List(_) => "a list".into(),
            Value::Map(_) => "named entries".into(),
            Value::Records(_) => "a table".into(),
        }
    }

    /// As the page shows it, on one line.
    pub fn show(&self) -> String {
        match self {
            Value::Bool(true) => "on".into(),
            Value::Bool(false) => "off".into(),
            Value::Int(n) => n.to_string(),
            Value::Float(n) if n.fract() == 0.0 => format!("{n:.0}"),
            Value::Float(n) => n.to_string(),
            Value::Text(t) if t.is_empty() => "(empty)".into(),
            Value::Text(t) => t.clone(),
            Value::List(l) => l.join(", "),
            Value::Map(m) => match m.len() {
                1 => "1 entry".into(),
                n => format!("{n} entries"),
            },
            Value::Records(r) => match r.len() {
                1 => "1 entry".into(),
                n => format!("{n} entries"),
            },
        }
    }
}

fn minutes(text: &str) -> Option<i64> {
    let text = text.trim();
    if let Some(h) = text.strip_suffix('h') {
        return h.trim().parse::<i64>().ok().map(|h| h * 60);
    }
    text.strip_suffix('m').unwrap_or(text).trim().parse().ok()
}

impl Kind {
    /// Whether `value` is one this setting accepts, and why not.
    pub fn check(&self, value: &Value) -> Result<(), String> {
        match (self, value) {
            (Kind::Int { min, max }, Value::Int(n)) if n < min || n > max => Err(format!("a whole number from {min} to {max}")),
            (Kind::Float { min, max }, Value::Float(n)) if n < min || n > max => Err(format!("a number from {min} to {max}")),
            (Kind::Float { min, max }, Value::Int(n)) if (*n as f64) < *min || (*n as f64) > *max => Err(format!("a number from {min} to {max}")),
            (Kind::Minutes, Value::Int(n)) if *n < 1 => Err("at least 1 minute".into()),
            (Kind::Choice(choices), Value::Text(t)) if !choices.contains(&t.as_str()) => Err(format!("one of {}", choices.join(", "))),
            (Kind::Records(fields), Value::Records(rows)) => {
                for row in rows {
                    if row.len() != fields.len() {
                        return Err(format!("each entry needs {}", fields.iter().map(|f| f.label).collect::<Vec<_>>().join(", ")));
                    }
                    for (f, v) in fields.iter().zip(row) {
                        if v.trim().is_empty() && f.default.is_none() {
                            return Err(format!("{} can't be empty", f.label));
                        }
                        if let Some((min, max)) = f.range {
                            match v.trim().parse::<i64>() {
                                Ok(n) if n >= min && n <= max => {}
                                _ => return Err(format!("{}: a whole number from {min} to {max}", f.label)),
                            }
                        }
                    }
                }
                Ok(())
            }
            (Kind::Map { .. }, Value::Map(entries)) => {
                let mut seen = std::collections::HashSet::new();
                for (k, _) in entries {
                    if k.trim().is_empty() {
                        return Err("an entry needs a name".into());
                    }
                    if !seen.insert(k) {
                        return Err(format!("\"{k}\" is there twice"));
                    }
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    /// A value typed on the page, for a setting that takes one line.
    pub fn parse(&self, text: &str) -> Result<Value, String> {
        let t = text.trim();
        let value = match self {
            Kind::Bool => match t {
                "on" | "true" | "yes" => Value::Bool(true),
                "off" | "false" | "no" => Value::Bool(false),
                _ => return Err("on or off".into()),
            },
            Kind::Int { min, max } => Value::Int(t.parse().map_err(|_| format!("a whole number from {min} to {max}"))?),
            Kind::Float { min, max } => Value::Float(t.parse().map_err(|_| format!("a number from {min} to {max}"))?),
            Kind::Minutes => Value::Int(minutes(t).ok_or("a number of minutes, like 5, 5m or 1h")?),
            Kind::List => Value::List(crate::names(t)),
            _ => Value::Text(text.to_string()),
        };
        self.check(&value)?;
        Ok(value)
    }

    /// Reads it from the file.
    pub(crate) fn read(&self, item: &Item) -> Result<Value, String> {
        let got = |what: &str| format!("expected {what}, got {}", item.type_name());
        match self {
            Kind::Bool => item.as_bool().map(Value::Bool).ok_or_else(|| got("true or false")),
            Kind::Int { .. } => item.as_integer().map(Value::Int).ok_or_else(|| got("a whole number")),
            Kind::Float { .. } => item.as_float().or_else(|| item.as_integer().map(|n| n as f64)).map(Value::Float).ok_or_else(|| got("a number")),
            Kind::Minutes => item.as_integer().or_else(|| item.as_str().and_then(minutes)).map(Value::Int).ok_or_else(|| got("a number of minutes")),
            Kind::Text | Kind::Path | Kind::Choice(_) | Kind::Theme | Kind::Font | Kind::Secret(_) => item.as_str().map(|s| Value::Text(s.to_string())).ok_or_else(|| got("text in quotes")),
            Kind::List => match item.as_array() {
                Some(a) => a.iter().map(|v| v.as_str().map(str::to_string).ok_or_else(|| "every entry should be text in quotes".to_string())).collect::<Result<_, _>>().map(Value::List),
                None => item.as_str().map(|s| Value::List(crate::names(s))).ok_or_else(|| got("a list, like [\"a\", \"b\"]")),
            },
            Kind::Map { .. } => {
                let table = item.as_table_like().ok_or_else(|| got("a table of names"))?;
                table
                    .iter()
                    .map(|(k, v)| match v.as_str() {
                        Some(s) => Ok((k.to_string(), s.to_string())),
                        None => Err(format!("{k}: expected text in quotes")),
                    })
                    .collect::<Result<_, _>>()
                    .map(Value::Map)
            }
            Kind::Records(fields) => {
                let rows = item.as_array_of_tables().ok_or_else(|| got("a list of [[tables]]"))?;
                rows.iter()
                    .map(|row| {
                        fields
                            .iter()
                            .map(|f| match row.get(f.name) {
                                Some(v) => v.as_str().map(str::to_string).or_else(|| v.as_integer().map(|n| n.to_string())).ok_or_else(|| format!("{}: expected text or a number", f.name)),
                                None => f.default.map(str::to_string).ok_or_else(|| format!("an entry is missing {}", f.name)),
                            })
                            .collect::<Result<Vec<_>, _>>()
                    })
                    .collect::<Result<_, _>>()
                    .map(Value::Records)
            }
        }
    }

    /// Writes it for the file.
    pub(crate) fn write(self, v: &Value) -> Item {
        match (self, v) {
            (_, Value::Bool(b)) => value(*b),
            (_, Value::Int(n)) => value(*n),
            (_, Value::Float(n)) if n.fract() == 0.0 && n.abs() < 1e15 => value(*n as i64),
            (_, Value::Float(n)) => value(*n),
            (_, Value::Text(t)) => value(t.as_str()),
            (_, Value::List(l)) => value(l.iter().map(String::as_str).collect::<Array>()),
            (_, Value::Map(m)) => {
                let mut table = Table::new();
                for (k, v) in m {
                    table.insert(k, value(v.as_str()));
                }
                Item::Table(table)
            }
            (Kind::Records(fields), Value::Records(rows)) => {
                let mut array = ArrayOfTables::new();
                for row in rows {
                    let mut table = Table::new();
                    for (f, v) in fields.iter().zip(row) {
                        if f.default == Some(v.as_str()) {
                            continue;
                        }
                        match (f.range, v.parse::<i64>()) {
                            (Some(_), Ok(n)) => table.insert(f.name, value(n)),
                            _ => table.insert(f.name, value(v.as_str())),
                        };
                    }
                    array.push(table);
                }
                Item::ArrayOfTables(array)
            }
            (_, Value::Records(rows)) => value(rows.iter().map(|r| InlineTable::from_iter(r.iter().enumerate().map(|(i, v)| (i.to_string(), v.as_str())))).collect::<Array>()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_key_is_unique_and_every_category_has_a_setting() {
        let mut keys: Vec<&str> = settings().iter().map(|s| s.key).collect();
        keys.sort();
        let before = keys.len();
        keys.dedup();
        assert_eq!(keys.len(), before, "a key declared twice");
        for c in Category::ALL {
            assert!(settings().iter().any(|s| s.category == c), "{} is empty", c.label());
        }
    }

    #[test]
    fn typed_values_are_checked_against_the_setting() {
        let size = setting("editor.font_size").unwrap();
        assert_eq!(size.kind.parse("18"), Ok(Value::Float(18.0)));
        assert!(size.kind.parse("90").unwrap_err().contains("6 to 48"));
        assert!(size.kind.parse("16px").is_err());
        let fetch = setting("git.auto_fetch").unwrap();
        assert_eq!(fetch.kind.parse("1h"), Ok(Value::Int(60)));
        assert!(fetch.kind.parse("five minutes").unwrap_err().contains("minutes"));
        let layout = setting("git.layout").unwrap();
        assert!(layout.kind.parse("tabs").unwrap_err().contains("page, panes"));
        let hosts = setting("vnc.hosts").unwrap();
        assert!(hosts.kind.check(&Value::Records(vec![vec!["a".into(), "h".into(), "70000".into()]])).unwrap_err().contains("Port"));
    }

    /// The README lists every setting; this is what keeps it true. When
    /// it fails, paste `markdown_table()` over the README's table (run
    /// `cargo test -p fenix-config print_the_settings_table -- --ignored
    /// --nocapture`).
    #[test]
    fn the_readme_lists_every_setting_as_the_schema_declares_it() {
        let readme = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../README.md")).unwrap().replace('\r', "");
        assert!(readme.contains(&markdown_table()), "README.md's settings table is out of date");
    }

    #[test]
    #[ignore]
    fn print_the_settings_table() {
        print!("{}", markdown_table());
    }

    #[test]
    fn a_blocked_meaning_reads_back_as_it_was_written() {
        for b in [JiraBlocked::Flag, JiraBlocked::Local, JiraBlocked::Status { id: "10103".into(), name: "On Hold".into() }] {
            assert_eq!(blocked_parse(&blocked_text(&b)), Ok(b));
        }
        assert!(blocked_parse("maybe").is_err());
    }
}
