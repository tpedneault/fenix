//! Project templates as data: a folder holding a `template.toml` and a
//! `files/` tree. The TOML declares the questions the new-project wizard
//! asks, the commands it runs and the tasks it writes into
//! `.fenix/tools.json`; files are copied with `{{name}}`-style
//! substitution in their paths and contents. That's the whole language --
//! no scripting, and no conditionals beyond `when` on an entry. The
//! built-in templates ship in this same format (`templates/` beside this
//! crate's `src/`), so they're both the documentation and the tests.
//!
//! ```toml
//! [template]
//! name = "Python · uv"
//! kind = "python"            # a `ProjectKind` id
//! group = "Languages"        # the heading it's listed under
//! description = "..."
//! needs = ["uv"]             # programs looked up on PATH
//! name_rule = "sketch"       # optional: Arduino's stricter naming
//! open = ["src/main.rs", "README.md"]   # first that exists is opened
//!
//! [[ask]]                    # text, unless one of the below
//! key = "python"
//! label = "Python"
//! choices = ["3.13", "3.12"] # a choice, cycled with h/l
//! many = ["pytest", "ruff"]  # or several, toggled
//! default = "3.12"           # a bool default makes a toggle
//! hint = "..."
//!
//! [[file]]                   # files not named here are copied as-is
//! path = "{{name}}.ino"
//! from = "blink.ino"         # or `content = "..."`
//! when = "start == blink"
//!
//! [[run]]
//! exec = "uv"
//! args = ["add", "--dev", "{{dev...}}"]   # `...` splats a list
//! when = "dev"
//!
//! [[task]]                   # merged into .fenix/tools.json's tasks
//! name = "test"
//! exec = "uv"
//! args = ["run", "pytest"]
//!
//! [[hook]]                   # something only the editor can do
//! kind = "mib-root"
//! path = "mib"
//! label = "{{label}}"
//!
//! [tools]                    # any other tools.json content, verbatim
//! ```
//!
//! Variables: every `ask` key, plus `name`, `name_snake`, `name_kebab`,
//! `name_pascal` and `name_upper` (`NAME_SNAKE`, for C macros). `when`
//! is `key`, `!key`, `key == value` or `key != value`, or a list of those
//! that must all hold; for a `many` ask, `==` means "includes". An
//! `[[ask]]` can have a `when` too: it's only asked when that holds (its
//! default still applies otherwise).
//!
//! Files are written before the commands run, so a command sees them --
//! except a `[[file]]` with `after = true`, and the generated
//! `.fenix/tools.json`, which are written after the commands and before
//! git: a scaffolder like `npm create vite` refuses a folder that already
//! has something in it.
//!
//! File contents can keep or drop whole lines with blocks, each
//! directive on a line of its own:
//!
//! ```text
//! {{#if tests == gtest}}
//! FetchContent_Declare(googletest ...)
//! {{else}}
//! enable_testing()
//! {{/if}}
//! ```
//!
//! Blocks nest; their conditions are checked when the template loads.
//! An unknown variable in a path, argument or label is an error when the
//! template loads; in file contents a `{{...}}` that isn't a variable is
//! left alone, so Tcl's and C's own braces survive. Commands never go
//! through a shell: each `args` entry is one literal argument.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::Deserialize;

use crate::kind::ProjectKind;
use crate::tools::{CommandSpec, ProjectTools};

// ------------------------------------------------------------------
// The model
// ------------------------------------------------------------------

/// Where a template came from -- a user's own template shadows a
/// built-in with the same folder name, and a project's own
/// `.fenix/templates/` adds to both.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    BuiltIn,
    User(PathBuf),
    Project(PathBuf),
}

impl Origin {
    pub fn describe(&self) -> String {
        match self {
            Origin::BuiltIn => "built-in".to_string(),
            Origin::User(path) | Origin::Project(path) => path.display().to_string(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameRule {
    /// Anything that's a valid folder name on every platform.
    Any,
    /// Arduino's rule: letters, digits, `_ - .`, starting with a letter
    /// or digit, at most 63 characters -- the folder and its main `.ino`
    /// share the name.
    Sketch,
    /// An npm package name, since scaffolders name the package after the
    /// folder: lowercase letters, digits, `- . _`, starting with a letter
    /// or digit.
    Npm,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AskKind {
    Text { default: String },
    Choice { choices: Vec<String>, default: String },
    Toggle { default: bool },
    Many { options: Vec<String>, default: Vec<String> },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Ask {
    pub key: String,
    pub label: String,
    pub hint: Option<String>,
    pub kind: AskKind,
    /// Only asked when this holds -- see `Template::asks_now`.
    when: Option<Cond>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    Text(String),
    Bool(bool),
    Many(Vec<String>),
}

pub type Answers = BTreeMap<String, Answer>;

#[derive(Debug, Clone, PartialEq)]
enum Cond {
    Truthy(String),
    Falsy(String),
    Eq(String, String),
    Ne(String, String),
    /// A list: every one must hold.
    All(Vec<Cond>),
}

/// A `when` as written: one condition, or a list that must all hold.
#[derive(Deserialize, Clone)]
#[serde(untagged)]
enum RawWhen {
    One(String),
    All(Vec<String>),
}

#[derive(Debug, Clone, PartialEq)]
struct FileEntry {
    path: String,
    body: String,
    when: Option<Cond>,
    /// Written after the commands rather than before.
    after: bool,
}

#[derive(Debug, Clone, PartialEq)]
struct RunEntry {
    exec: String,
    args: Vec<String>,
    cwd: Option<String>,
    when: Option<Cond>,
}

#[derive(Debug, Clone, PartialEq)]
struct TaskEntry {
    name: String,
    exec: String,
    args: Vec<String>,
    when: Option<Cond>,
}

#[derive(Debug, Clone, PartialEq)]
struct HookEntry {
    kind: HookKind,
    path: String,
    label: String,
    when: Option<Cond>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HookKind {
    MibRoot,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Template {
    /// The folder name -- how a user template shadows a built-in, and
    /// what `:project-new <id>` takes.
    pub id: String,
    pub name: String,
    pub kind: ProjectKind,
    pub group: String,
    pub description: String,
    /// Programs the template runs, for the wizard to look up before you
    /// commit to it.
    pub needs: Vec<String>,
    pub name_rule: NameRule,
    /// Files to open once it's created, first existing one wins.
    open: Vec<String>,
    pub origin: Origin,
    pub asks: Vec<Ask>,
    files: Vec<FileEntry>,
    runs: Vec<RunEntry>,
    tasks: Vec<TaskEntry>,
    hooks: Vec<HookEntry>,
    tools: Option<serde_json::Value>,
}

// ------------------------------------------------------------------
// Parsing
// ------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTemplate {
    template: RawMeta,
    #[serde(default)]
    ask: Vec<RawAsk>,
    #[serde(default)]
    file: Vec<RawFile>,
    #[serde(default)]
    run: Vec<RawRun>,
    #[serde(default)]
    task: Vec<RawTask>,
    #[serde(default)]
    hook: Vec<RawHook>,
    tools: Option<toml::Table>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawMeta {
    name: String,
    kind: String,
    group: Option<String>,
    #[serde(default)]
    description: String,
    #[serde(default)]
    needs: Vec<String>,
    name_rule: Option<String>,
    #[serde(default)]
    open: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawAsk {
    key: String,
    label: String,
    hint: Option<String>,
    choices: Option<Vec<String>>,
    many: Option<Vec<String>>,
    default: Option<toml::Value>,
    when: Option<RawWhen>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFile {
    path: String,
    from: Option<String>,
    content: Option<String>,
    when: Option<RawWhen>,
    #[serde(default)]
    after: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRun {
    exec: String,
    #[serde(default)]
    args: Vec<String>,
    cwd: Option<String>,
    when: Option<RawWhen>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTask {
    name: String,
    exec: String,
    #[serde(default)]
    args: Vec<String>,
    when: Option<RawWhen>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawHook {
    kind: String,
    path: String,
    label: Option<String>,
    when: Option<RawWhen>,
}

const NAME_VARS: [&str; 5] = ["name", "name_snake", "name_kebab", "name_pascal", "name_upper"];

impl Template {
    /// Parses and fully validates one template: its `template.toml` text
    /// and its `files/` tree as (forward-slash relative path, contents).
    pub fn parse(id: &str, toml_text: &str, files: &[(String, String)], origin: Origin) -> Result<Self, String> {
        let raw: RawTemplate = toml::from_str(toml_text).map_err(|e| format!("template.toml: {e}"))?;
        let kind = ProjectKind::from_id(&raw.template.kind).ok_or_else(|| format!("unknown kind \"{}\"", raw.template.kind))?;
        let name_rule = match raw.template.name_rule.as_deref() {
            None | Some("any") => NameRule::Any,
            Some("sketch") => NameRule::Sketch,
            Some("npm") => NameRule::Npm,
            Some(other) => return Err(format!("unknown name_rule \"{other}\"")),
        };

        let mut asks = Vec::new();
        let mut ask_whens = Vec::new();
        for raw_ask in raw.ask {
            let key = raw_ask.key.trim().to_string();
            if key.is_empty() || !key.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_') {
                return Err(format!("ask key \"{key}\" must be lowercase letters, digits and _"));
            }
            if NAME_VARS.contains(&key.as_str()) || asks.iter().any(|a: &Ask| a.key == key) {
                return Err(format!("ask key \"{key}\" is used twice"));
            }
            let kind = match (raw_ask.choices, raw_ask.many, raw_ask.default) {
                (Some(_), Some(_), _) => return Err(format!("ask \"{key}\" has both choices and many")),
                (Some(choices), None, default) => {
                    if choices.is_empty() {
                        return Err(format!("ask \"{key}\" has no choices"));
                    }
                    let default = match default {
                        None => choices[0].clone(),
                        Some(toml::Value::String(s)) if choices.contains(&s) => s,
                        Some(_) => return Err(format!("ask \"{key}\": default must be one of its choices")),
                    };
                    AskKind::Choice { choices, default }
                }
                (None, Some(options), default) => {
                    let default = match default {
                        None => Vec::new(),
                        Some(toml::Value::Array(items)) => items
                            .into_iter()
                            .map(|item| match item {
                                toml::Value::String(s) if options.contains(&s) => Ok(s),
                                _ => Err(format!("ask \"{key}\": every default must be one of its options")),
                            })
                            .collect::<Result<_, _>>()?,
                        Some(_) => return Err(format!("ask \"{key}\": default must be a list")),
                    };
                    AskKind::Many { options, default }
                }
                (None, None, Some(toml::Value::Boolean(b))) => AskKind::Toggle { default: b },
                (None, None, Some(toml::Value::String(s))) => AskKind::Text { default: s },
                (None, None, None) => AskKind::Text { default: String::new() },
                (None, None, Some(_)) => return Err(format!("ask \"{key}\": default must be text or true/false")),
            };
            asks.push(Ask { key, label: raw_ask.label, hint: raw_ask.hint, kind, when: None });
            ask_whens.push(raw_ask.when);
        }

        let mut template = Template {
            id: id.to_string(),
            name: raw.template.name,
            kind,
            group: raw.template.group.unwrap_or_else(|| "Yours".to_string()),
            description: raw.template.description,
            needs: raw.template.needs,
            name_rule,
            open: raw.template.open,
            origin,
            asks,
            files: Vec::new(),
            runs: Vec::new(),
            tasks: Vec::new(),
            hooks: Vec::new(),
            tools: None,
        };

        // An ask's `when` can name any ask, so it's read once they all are.
        for (i, when) in ask_whens.into_iter().enumerate() {
            let cond = template.parse_when(when.as_ref())?;
            if cond.as_ref().is_some_and(|c| c.mentions(&template.asks[i].key)) {
                return Err(format!("ask \"{}\" can't depend on itself", template.asks[i].key));
            }
            template.asks[i].when = cond;
        }
        for path in &template.open {
            template.check_relative(path)?;
            template.check_vars(path, false)?;
        }
        // Files named by a `[[file]]` entry's `from` are only written
        // where an entry says; every other file under `files/` is copied.
        let referenced: Vec<&str> = raw.file.iter().filter_map(|f| f.from.as_deref()).collect();
        for (path, body) in files {
            template.check_blocks(body).map_err(|e| format!("files/{path}: {e}"))?;
            if !referenced.contains(&path.as_str()) {
                template.check_relative(path)?;
                template.check_vars(path, false)?;
                template.files.push(FileEntry { path: path.clone(), body: body.clone(), when: None, after: false });
            }
        }
        for raw_file in raw.file {
            template.check_relative(&raw_file.path)?;
            template.check_vars(&raw_file.path, false)?;
            let body = match (raw_file.from, raw_file.content) {
                (Some(from), None) => files
                    .iter()
                    .find(|(path, _)| *path == from)
                    .map(|(_, body)| body.clone())
                    .ok_or_else(|| format!("file \"{}\": files/{from} doesn't exist", raw_file.path))?,
                (None, Some(content)) => {
                    template.check_blocks(&content).map_err(|e| format!("file \"{}\": {e}", raw_file.path))?;
                    content
                }
                _ => return Err(format!("file \"{}\" needs exactly one of from or content", raw_file.path)),
            };
            let when = template.parse_when(raw_file.when.as_ref())?;
            template.files.push(FileEntry { path: raw_file.path, body, when, after: raw_file.after });
        }
        for raw_run in raw.run {
            template.check_vars(&raw_run.exec, false)?;
            for arg in &raw_run.args {
                template.check_vars(arg, true)?;
            }
            if let Some(cwd) = &raw_run.cwd {
                template.check_relative(cwd)?;
                template.check_vars(cwd, false)?;
            }
            let when = template.parse_when(raw_run.when.as_ref())?;
            template.runs.push(RunEntry { exec: raw_run.exec, args: raw_run.args, cwd: raw_run.cwd, when });
        }
        for raw_task in raw.task {
            template.check_vars(&raw_task.name, false)?;
            template.check_vars(&raw_task.exec, false)?;
            for arg in &raw_task.args {
                template.check_vars(arg, true)?;
            }
            let when = template.parse_when(raw_task.when.as_ref())?;
            template.tasks.push(TaskEntry { name: raw_task.name, exec: raw_task.exec, args: raw_task.args, when });
        }
        for raw_hook in raw.hook {
            let kind = match raw_hook.kind.as_str() {
                "mib-root" => HookKind::MibRoot,
                other => return Err(format!("unknown hook kind \"{other}\"")),
            };
            template.check_relative(&raw_hook.path)?;
            template.check_vars(&raw_hook.path, false)?;
            let label = raw_hook.label.unwrap_or_else(|| "{{name}}".to_string());
            template.check_vars(&label, false)?;
            let when = template.parse_when(raw_hook.when.as_ref())?;
            template.hooks.push(HookEntry { kind, path: raw_hook.path, label, when });
        }
        if let Some(tools) = raw.tools {
            let json = serde_json::to_value(tools).map_err(|e| format!("[tools]: {e}"))?;
            let mut strings = Vec::new();
            collect_strings(&json, &mut strings);
            for s in strings {
                template.check_vars(&s, false)?;
            }
            template.tools = Some(json);
        }
        for ask in &template.asks {
            if let AskKind::Text { default } = &ask.kind {
                // A text default may name the project (`{{name}}`), but
                // not another answer -- those aren't known yet.
                for var in variables(default) {
                    if !NAME_VARS.contains(&var.name.as_str()) {
                        return Err(format!("ask \"{}\": a default can only use the name variables", ask.key));
                    }
                }
            }
        }
        Ok(template)
    }

    /// Every `{{#if}}` block in `body` balanced, and its condition about
    /// an ask that exists.
    fn check_blocks(&self, body: &str) -> Result<(), String> {
        let mut depth: Vec<bool> = Vec::new(); // whether each open block has had its else
        for (n, line) in body.lines().enumerate() {
            match directive(line) {
                Some(Directive::If(expr)) => {
                    self.parse_one_when(expr).map_err(|e| format!("line {}: {e}", n + 1))?;
                    depth.push(false);
                }
                Some(Directive::Else) => match depth.last_mut() {
                    Some(seen) if !*seen => *seen = true,
                    Some(_) => return Err(format!("line {}: a second {{{{else}}}} in one block", n + 1)),
                    None => return Err(format!("line {}: {{{{else}}}} outside a block", n + 1)),
                },
                Some(Directive::End) if depth.pop().is_none() => {
                    return Err(format!("line {}: {{{{/if}}}} with no {{{{#if}}}}", n + 1));
                }
                Some(Directive::End) | None => {}
            }
        }
        if depth.is_empty() { Ok(()) } else { Err("a {{#if}} isn't closed".to_string()) }
    }

    /// Whether ask `i` should be asked, given the answers so far.
    pub fn asks_now(&self, i: usize, answers: &Answers) -> bool {
        self.asks.get(i).is_some_and(|ask| cond_holds(ask.when.as_ref(), answers))
    }

    fn ask(&self, key: &str) -> Option<&Ask> {
        self.asks.iter().find(|a| a.key == key)
    }

    fn check_relative(&self, path: &str) -> Result<(), String> {
        let p = Path::new(path);
        if path.is_empty()
            || p.is_absolute()
            || path.starts_with(['/', '\\'])
            || p.components().any(|c| matches!(c, std::path::Component::ParentDir | std::path::Component::Prefix(_)))
        {
            return Err(format!("\"{path}\" must be a relative path inside the project"));
        }
        Ok(())
    }

    /// Every `{{var}}` in `text` must be a known variable; `{{var...}}`
    /// only where `splat_ok` (a whole argument) and only for a list-ish
    /// answer.
    fn check_vars(&self, text: &str, splat_ok: bool) -> Result<(), String> {
        for var in variables(text) {
            let known = NAME_VARS.contains(&var.name.as_str()) || self.ask(&var.name).is_some();
            if !known {
                return Err(format!("\"{text}\" uses {{{{{}}}}}, which isn't a variable", var.name));
            }
            if var.splat {
                if !splat_ok || var.whole != text {
                    return Err(format!("\"{text}\": {{{{{}...}}}} must be a whole argument on its own", var.name));
                }
                if !matches!(self.ask(&var.name).map(|a| &a.kind), Some(AskKind::Text { .. } | AskKind::Many { .. })) {
                    return Err(format!("\"{text}\": only a text or many answer can be splatted"));
                }
            }
        }
        Ok(())
    }

    fn parse_when(&self, when: Option<&RawWhen>) -> Result<Option<Cond>, String> {
        match when {
            None => Ok(None),
            Some(RawWhen::One(one)) => self.parse_one_when(one),
            Some(RawWhen::All(all)) => {
                let conds = all.iter().map(|w| self.parse_one_when(w)).collect::<Result<Vec<_>, _>>()?;
                Ok(Some(Cond::All(conds.into_iter().flatten().collect())))
            }
        }
    }

    fn parse_one_when(&self, when: &str) -> Result<Option<Cond>, String> {
        let when = when.trim();
        let Some(cond) = parse_cond(when) else {
            return Ok(None);
        };
        let (Cond::Truthy(key) | Cond::Falsy(key) | Cond::Eq(key, _) | Cond::Ne(key, _)) = &cond else {
            return Ok(Some(cond));
        };
        let Some(ask) = self.ask(key) else {
            return Err(format!("when \"{when}\": \"{key}\" isn't an ask"));
        };
        if let (Cond::Eq(_, value) | Cond::Ne(_, value), AskKind::Choice { choices: options, .. } | AskKind::Many { options, .. }) = (&cond, &ask.kind) {
            if !options.contains(value) {
                return Err(format!("when \"{when}\": \"{value}\" isn't one of {key}'s options"));
            }
        }
        Ok(Some(cond))
    }

    /// Each ask's default, with the name variables already filled in.
    pub fn default_answers(&self, name: &str) -> Answers {
        let names = name_vars(name);
        self.asks
            .iter()
            .map(|ask| {
                let answer = match &ask.kind {
                    AskKind::Text { default } => Answer::Text(substitute_known(default, &|v| names.get(v).cloned())),
                    AskKind::Choice { default, .. } => Answer::Text(default.clone()),
                    AskKind::Toggle { default } => Answer::Bool(*default),
                    AskKind::Many { default, .. } => Answer::Many(default.clone()),
                };
                (ask.key.clone(), answer)
            })
            .collect()
    }

    /// Everything creating `name` under `parent` with `answers` would do,
    /// computed without touching the disk -- the wizard's review page is
    /// this, printed.
    pub fn plan(&self, name: &str, parent: &Path, answers: &Answers) -> Result<Plan, String> {
        validate_name(name, self.name_rule)?;
        let mut answers = answers.clone();
        for (key, answer) in self.default_answers(name) {
            answers.entry(key).or_insert(answer);
        }
        let ctx = Context { names: name_vars(name), answers: &answers };

        let mut files: Vec<PlannedFile> = Vec::new();
        let mut after_files: Vec<PlannedFile> = Vec::new();
        for entry in &self.files {
            if !ctx.holds(entry.when.as_ref()) {
                continue;
            }
            let path = ctx.text(&entry.path)?;
            if files.iter().chain(&after_files).any(|f| f.path == path) {
                return Err(format!("two files would both be written to {path}"));
            }
            let planned = PlannedFile { path, contents: ctx.contents(&entry.body) };
            if entry.after { after_files.push(planned) } else { files.push(planned) }
        }

        let mut tools = self.tools.clone().map(|t| ctx.json(t)).transpose()?.unwrap_or_else(|| serde_json::json!({}));
        for task in &self.tasks {
            if !ctx.holds(task.when.as_ref()) {
                continue;
            }
            let spec = serde_json::json!({ "executable": ctx.text(&task.exec)?, "args": ctx.args(&task.args)? });
            let tasks = tools.as_object_mut().ok_or("[tools] must be a table")?.entry("tasks").or_insert_with(|| serde_json::json!({}));
            tasks.as_object_mut().ok_or("[tools.tasks] must be a table")?.insert(ctx.text(&task.name)?, spec);
        }
        if tools.as_object().is_some_and(|o| !o.is_empty()) {
            if files.iter().chain(&after_files).any(|f| f.path == ".fenix/tools.json") {
                return Err("both files/ and the template's tasks write .fenix/tools.json".to_string());
            }
            let text = serde_json::to_string_pretty(&tools).map_err(|e| e.to_string())? + "\n";
            ProjectTools::parse(&text).map_err(|e| format!("the generated .fenix/tools.json is invalid: {e}"))?;
            after_files.push(PlannedFile { path: ".fenix/tools.json".to_string(), contents: text });
        }

        let mut steps = Vec::new();
        for run in &self.runs {
            if !ctx.holds(run.when.as_ref()) {
                continue;
            }
            let mut spec = CommandSpec::new(ctx.text(&run.exec)?, ctx.args(&run.args)?);
            spec.cwd = run.cwd.as_deref().map(|c| ctx.text(c)).transpose()?.map(PathBuf::from);
            spec.validate()?;
            steps.push(Step { spec });
        }

        let mut hooks = Vec::new();
        for hook in &self.hooks {
            if !ctx.holds(hook.when.as_ref()) {
                continue;
            }
            match hook.kind {
                HookKind::MibRoot => hooks.push(Hook::MibRoot { path: ctx.text(&hook.path)?, label: ctx.text(&hook.label)? }),
            }
        }

        let open = self.open.iter().map(|p| ctx.text(p)).collect::<Result<_, _>>()?;
        Ok(Plan { dir: parent.join(name), kind: self.kind, files, after_files, steps, hooks, open })
    }
}

fn collect_strings(value: &serde_json::Value, out: &mut Vec<String>) {
    match value {
        serde_json::Value::String(s) => out.push(s.clone()),
        serde_json::Value::Array(items) => items.iter().for_each(|v| collect_strings(v, out)),
        serde_json::Value::Object(map) => map.iter().for_each(|(k, v)| {
            out.push(k.clone());
            collect_strings(v, out)
        }),
        _ => {}
    }
}

// ------------------------------------------------------------------
// Substitution
// ------------------------------------------------------------------

struct Var {
    name: String,
    splat: bool,
    /// The whole `{{...}}` as written.
    whole: String,
}

/// Every well-formed `{{name}}` / `{{name...}}` in `text`. Anything else
/// between double braces isn't a variable and is left alone.
fn variables(text: &str) -> Vec<Var> {
    let mut vars = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("{{") {
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else { break };
        let inner = &after[..end];
        let (name, splat) = match inner.strip_suffix("...") {
            Some(name) => (name, true),
            None => (inner, false),
        };
        if !name.is_empty() && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_') {
            vars.push(Var { name: name.to_string(), splat, whole: format!("{{{{{inner}}}}}") });
            rest = &after[end + 2..];
        } else {
            rest = &rest[start + 2..];
        }
    }
    vars
}

/// Replaces each variable `lookup` knows; leaves every other `{{...}}`.
fn substitute_known(text: &str, lookup: &dyn Fn(&str) -> Option<String>) -> String {
    let mut out = text.to_string();
    for var in variables(text) {
        if var.splat {
            continue;
        }
        if let Some(value) = lookup(&var.name) {
            out = out.replace(&var.whole, &value);
        }
    }
    out
}

struct Context<'a> {
    names: BTreeMap<&'static str, String>,
    answers: &'a Answers,
}

impl Context<'_> {
    fn scalar(&self, name: &str) -> Option<String> {
        if let Some(value) = self.names.get(name) {
            return Some(value.clone());
        }
        Some(match self.answers.get(name)? {
            Answer::Text(s) => s.clone(),
            Answer::Bool(b) => b.to_string(),
            Answer::Many(items) => items.join(" "),
        })
    }

    fn list(&self, name: &str) -> Vec<String> {
        match self.answers.get(name) {
            Some(Answer::Many(items)) => items.clone(),
            Some(Answer::Text(s)) => split_list(s),
            Some(Answer::Bool(b)) => vec![b.to_string()],
            None => Vec::new(),
        }
    }

    fn text(&self, text: &str) -> Result<String, String> {
        let mut out = text.to_string();
        for var in variables(text) {
            let value = self.scalar(&var.name).ok_or_else(|| format!("no value for {{{{{}}}}}", var.name))?;
            out = out.replace(&var.whole, &value);
        }
        Ok(out)
    }

    fn contents(&self, text: &str) -> String {
        substitute_known(&self.blocks(text), &|name| self.scalar(name))
    }

    /// `text` with each `{{#if}}` block's lines kept or dropped.
    fn blocks(&self, text: &str) -> String {
        if !text.contains("{{#if") {
            return text.to_string();
        }
        // For each open block: (whether its parent was being kept,
        // whether its own lines are being kept now).
        let mut stack: Vec<(bool, bool)> = Vec::new();
        let keeping = |stack: &[(bool, bool)]| stack.last().is_none_or(|(_, keep)| *keep);
        let mut out = String::new();
        for line in text.split_inclusive('\n') {
            match directive(line) {
                Some(Directive::If(expr)) => {
                    let parent = keeping(&stack);
                    stack.push((parent, parent && self.holds(parse_cond(expr).as_ref())));
                }
                Some(Directive::Else) => {
                    if let Some((parent, keep)) = stack.last_mut() {
                        *keep = *parent && !*keep;
                    }
                }
                Some(Directive::End) => {
                    stack.pop();
                }
                None if keeping(&stack) => out.push_str(line),
                None => {}
            }
        }
        out
    }

    fn args(&self, args: &[String]) -> Result<Vec<String>, String> {
        let mut out = Vec::new();
        for arg in args {
            match variables(arg).into_iter().find(|v| v.splat) {
                Some(var) => out.extend(self.list(&var.name)),
                None => out.push(self.text(arg)?),
            }
        }
        Ok(out)
    }

    fn json(&self, value: serde_json::Value) -> Result<serde_json::Value, String> {
        Ok(match value {
            serde_json::Value::String(s) => serde_json::Value::String(self.text(&s)?),
            serde_json::Value::Array(items) => serde_json::Value::Array(items.into_iter().map(|v| self.json(v)).collect::<Result<_, _>>()?),
            serde_json::Value::Object(map) => {
                let mut out = serde_json::Map::new();
                for (k, v) in map {
                    out.insert(self.text(&k)?, self.json(v)?);
                }
                serde_json::Value::Object(out)
            }
            other => other,
        })
    }

    fn holds(&self, cond: Option<&Cond>) -> bool {
        cond_holds(cond, self.answers)
    }
}

impl Cond {
    fn mentions(&self, key: &str) -> bool {
        match self {
            Cond::Truthy(k) | Cond::Falsy(k) | Cond::Eq(k, _) | Cond::Ne(k, _) => k == key,
            Cond::All(all) => all.iter().any(|c| c.mentions(key)),
        }
    }
}

/// A `{{#if ...}}`, `{{else}}` or `{{/if}}` line.
enum Directive<'a> {
    If(&'a str),
    Else,
    End,
}

fn directive(line: &str) -> Option<Directive<'_>> {
    let inner = line.trim().strip_prefix("{{")?.strip_suffix("}}")?.trim();
    if let Some(expr) = inner.strip_prefix("#if ") {
        Some(Directive::If(expr.trim()))
    } else if inner == "else" {
        Some(Directive::Else)
    } else if inner == "/if" {
        Some(Directive::End)
    } else {
        None
    }
}

/// A `when` expression, unchecked (checked ones come from `parse_when`).
fn parse_cond(when: &str) -> Option<Cond> {
    let when = when.trim();
    Some(if let Some((key, value)) = when.split_once("!=") {
        Cond::Ne(key.trim().to_string(), value.trim().to_string())
    } else if let Some((key, value)) = when.split_once("==") {
        Cond::Eq(key.trim().to_string(), value.trim().to_string())
    } else if let Some(key) = when.strip_prefix('!') {
        Cond::Falsy(key.trim().to_string())
    } else {
        Cond::Truthy(when.to_string())
    })
}

fn cond_holds(cond: Option<&Cond>, answers: &Answers) -> bool {
    let truthy = |key: &str| match answers.get(key) {
        Some(Answer::Text(s)) => !s.trim().is_empty(),
        Some(Answer::Bool(b)) => *b,
        Some(Answer::Many(items)) => !items.is_empty(),
        None => false,
    };
    let equals = |key: &str, value: &str| match answers.get(key) {
        Some(Answer::Text(s)) => s.trim() == value,
        Some(Answer::Bool(b)) => b.to_string() == value,
        Some(Answer::Many(items)) => items.iter().any(|i| i == value),
        None => false,
    };
    match cond {
        None => true,
        Some(Cond::Truthy(key)) => truthy(key),
        Some(Cond::Falsy(key)) => !truthy(key),
        Some(Cond::Eq(key, value)) => equals(key, value),
        Some(Cond::Ne(key, value)) => !equals(key, value),
        Some(Cond::All(all)) => all.iter().all(|c| cond_holds(Some(c), answers)),
    }
}

/// A text answer as a list: comma-separated when it has a comma (so a
/// name with spaces, "Adafruit NeoPixel", survives), else whitespace.
pub fn split_list(text: &str) -> Vec<String> {
    if text.contains(',') {
        text.split(',').map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).collect()
    } else {
        text.split_whitespace().map(str::to_string).collect()
    }
}

// ------------------------------------------------------------------
// Names
// ------------------------------------------------------------------

/// `name` split into words at separators and lower-to-upper case
/// changes: "orbit-tools" and "OrbitTools" are both [orbit, tools].
fn words(name: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut prev_lower = false;
    for c in name.chars() {
        if !c.is_alphanumeric() {
            if !current.is_empty() {
                words.push(std::mem::take(&mut current));
            }
            prev_lower = false;
            continue;
        }
        if c.is_uppercase() && prev_lower && !current.is_empty() {
            words.push(std::mem::take(&mut current));
        }
        prev_lower = c.is_lowercase() || c.is_ascii_digit();
        current.push(c);
    }
    if !current.is_empty() {
        words.push(current);
    }
    words
}

fn name_vars(name: &str) -> BTreeMap<&'static str, String> {
    let words: Vec<String> = words(name).into_iter().map(|w| w.to_lowercase()).collect();
    let mut snake = words.join("_");
    if snake.starts_with(|c: char| c.is_ascii_digit()) {
        snake.insert(0, '_');
    }
    let pascal: String = words
        .iter()
        .map(|w| {
            let mut chars = w.chars();
            chars.next().map(|first| first.to_uppercase().chain(chars).collect::<String>()).unwrap_or_default()
        })
        .collect();
    let upper = snake.to_uppercase();
    BTreeMap::from([("name", name.to_string()), ("name_snake", snake), ("name_kebab", words.join("-")), ("name_pascal", pascal), ("name_upper", upper)])
}

/// Whether `name` can be a project folder: never a path, never one of
/// Windows' reserved device names, and for a sketch, Arduino's own rule.
pub fn validate_name(name: &str, rule: NameRule) -> Result<(), String> {
    if name.is_empty() {
        return Err("a project needs a name".to_string());
    }
    if name.trim() != name {
        return Err("a name can't start or end with a space".to_string());
    }
    if name == "." || name == ".." || name.ends_with('.') {
        return Err("a name can't end with a dot".to_string());
    }
    if let Some(c) = name.chars().find(|c| matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control()) {
        return Err(format!("a name can't contain '{c}'"));
    }
    let stem = name.split('.').next().unwrap_or(name).to_ascii_uppercase();
    let reserved = ["CON", "PRN", "AUX", "NUL"].contains(&stem.as_str())
        || ((stem.starts_with("COM") || stem.starts_with("LPT")) && stem.len() == 4 && stem.as_bytes()[3].is_ascii_digit());
    if reserved {
        return Err(format!("{name} is a reserved name on Windows"));
    }
    if rule == NameRule::Npm {
        if !name.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit()) {
            return Err("a package name starts with a lowercase letter or digit".to_string());
        }
        if let Some(c) = name.chars().find(|c| !(c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '.' | '_'))) {
            return Err(format!("a package name can't contain '{c}' -- lowercase letters, digits, - . _ only"));
        }
    }
    if rule == NameRule::Sketch {
        if !name.starts_with(|c: char| c.is_ascii_alphanumeric()) {
            return Err("a sketch name starts with a letter or digit".to_string());
        }
        if let Some(c) = name.chars().find(|c| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))) {
            return Err(format!("a sketch name can't contain '{c}' -- letters, digits, _ - . only"));
        }
        if name.len() > 63 {
            return Err("a sketch name is at most 63 characters".to_string());
        }
    }
    Ok(())
}

// ------------------------------------------------------------------
// Plans
// ------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedFile {
    /// Forward-slash path relative to the project folder.
    pub path: String,
    pub contents: String,
}

#[derive(Debug, Clone)]
pub struct Step {
    pub spec: CommandSpec,
}

impl Step {
    pub fn new(executable: &str, args: &[&str]) -> Self {
        Step { spec: CommandSpec::new(executable.to_string(), args.iter().map(|a| a.to_string()).collect()) }
    }

    /// The command line as the review page shows it: each argument that
    /// needs it quoted, so what you read is what runs.
    pub fn display(&self) -> String {
        std::iter::once(&self.spec.executable).chain(&self.spec.args).map(|a| quote_for_display(a)).collect::<Vec<_>>().join(" ")
    }

    /// The process, ready to spawn in `dir`: no shell, no console window,
    /// no stdin, both outputs piped.
    pub fn command(&self, dir: &Path) -> io::Result<Command> {
        let mut command = self.spec.command(dir)?;
        command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        Ok(command)
    }
}

fn quote_for_display(arg: &str) -> String {
    if !arg.is_empty() && !arg.contains([' ', '"', '\t']) {
        arg.to_string()
    } else {
        format!("\"{}\"", arg.replace('"', "\\\""))
    }
}

/// Something only the editor can do once the files exist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Hook {
    /// Register `path` (relative to the project) as a `[mib]` root.
    MibRoot { path: String, label: String },
}

#[derive(Debug, Clone)]
pub struct Plan {
    pub dir: PathBuf,
    pub kind: ProjectKind,
    /// Written before the commands run.
    pub files: Vec<PlannedFile>,
    /// Written after them (and before git): `after = true` files and the
    /// generated `.fenix/tools.json`.
    pub after_files: Vec<PlannedFile>,
    pub steps: Vec<Step>,
    pub hooks: Vec<Hook>,
    /// Candidates to open when it's done, relative to `dir`.
    pub open: Vec<String>,
}

impl Plan {
    /// The first of `open` that exists now, else a README, else `None`.
    pub fn file_to_open(&self) -> Option<PathBuf> {
        self.open.iter().map(String::as_str).chain(["README.md"]).map(|p| self.dir.join(p)).find(|p| p.is_file())
    }
}

impl Plan {
    /// Why the target can't be used, if it can't: it's a file, or a
    /// folder that already has something in it.
    pub fn target_problem(&self) -> Option<String> {
        if self.dir.is_file() {
            return Some(format!("{} is a file", self.dir.display()));
        }
        let count = std::fs::read_dir(&self.dir).map(|entries| entries.count()).unwrap_or(0);
        (count > 0).then(|| format!("{} already exists and has {count} item{}", self.dir.display(), if count == 1 { "" } else { "s" }))
    }

    /// Creates the folder and writes every planned file. Never
    /// overwrites: a file that appears in the meantime stops the write
    /// with an error rather than being replaced.
    pub fn write_files(&self) -> Result<usize, String> {
        if let Some(problem) = self.target_problem() {
            return Err(problem);
        }
        std::fs::create_dir_all(&self.dir).map_err(|e| format!("{}: {e}", self.dir.display()))?;
        self.write(&self.files)
    }

    /// Writes the files that come after the commands -- into the folder
    /// they made, still never over anything.
    pub fn write_after_files(&self) -> Result<usize, String> {
        std::fs::create_dir_all(&self.dir).map_err(|e| format!("{}: {e}", self.dir.display()))?;
        self.write(&self.after_files)
    }

    fn write(&self, files: &[PlannedFile]) -> Result<usize, String> {
        for file in files {
            let path = self.dir.join(&file.path);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
            }
            let mut out = std::fs::OpenOptions::new().write(true).create_new(true).open(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            io::Write::write_all(&mut out, file.contents.as_bytes()).map_err(|e| format!("{}: {e}", path.display()))?;
        }
        Ok(files.len())
    }
}

// ------------------------------------------------------------------
// Where templates come from
// ------------------------------------------------------------------

macro_rules! builtin {
    ($id:literal, [$($file:literal),* $(,)?]) => {
        (
            $id,
            include_str!(concat!("../templates/", $id, "/template.toml")),
            &[$(($file, include_str!(concat!("../templates/", $id, "/files/", $file)))),*] as &[(&str, &str)],
        )
    };
}

/// (folder name, `template.toml`, `files/` as (path, contents)).
type Builtin = (&'static str, &'static str, &'static [(&'static str, &'static str)]);

const BUILTINS: &[Builtin] = &[
    builtin!("python-uv", [".gitignore", "README.md", "tests/test_smoke.py"]),
    builtin!("python-data", [".gitignore", "README.md", "data/processed/.gitkeep", "data/raw/.gitkeep", "notebooks/explore.ipynb", "src/{{name_snake}}/analysis.py", "src/{{name_snake}}/io.py", "tests/test_io.py"]),
    builtin!("python-gui", [".gitignore", "README.md", "src/{{name_snake}}/__init__.py", "src/{{name_snake}}/__main__.py", "src/{{name_snake}}/window.py", "tests/test_window.py"]),
    builtin!("python-cli", [".gitignore", "README.md", "src/{{name_snake}}/__init__.py", "src/{{name_snake}}/__main__.py", "src/{{name_snake}}/cli.py", "tests/test_cli.py"]),
    builtin!("python-api", [".gitignore", "Dockerfile", "README.md", "dockerignore", "src/{{name_snake}}/__init__.py", "src/{{name_snake}}/main.py", "tests/test_api.py"]),
    builtin!("rust-cargo", [".gitignore", "main_clap.rs"]),
    builtin!("rust-workspace", [".gitignore", "Cargo.toml", "README.md"]),
    builtin!("cmake", [".clangd", ".gitignore", "CMakeLists.txt", "CMakePresets.json", "README.md", "c/header.h", "c/library.c", "c/main.c", "c/test.c", "c/tests.cmake", "cpp/header.hpp", "cpp/library.cpp", "cpp/main.cpp", "cpp/test.cpp", "cpp/tests.cmake"]),
    builtin!("web-vite", []),
    builtin!("arduino-sketch", [".gitignore", "blink.ino", "echo.ino", "empty.ino"]),
    builtin!("arduino-library", ["README.md", "examples/Basic/Basic.ino", "keywords.txt", "library.properties", "src/{{name}}.cpp", "src/{{name}}.h"]),
    builtin!("scos-mib", []),
    builtin!("tcl-package", ["README.md", "pkgIndex.tcl", "tests/all.tcl", "tests/{{name_snake}}.test", "{{name_snake}}.tcl"]),
    builtin!("monorepo", [".editorconfig", ".fenix/project.ini", ".gitignore", "README.md"]),
    builtin!("empty", ["README.md"]),
];

/// The templates Fenix ships with.
pub fn builtin_templates() -> Vec<Template> {
    BUILTINS
        .iter()
        .map(|(id, toml_text, files)| {
            let files: Vec<(String, String)> = files.iter().map(|(p, c)| (p.to_string(), c.to_string())).collect();
            Template::parse(id, toml_text, &files, Origin::BuiltIn).unwrap_or_else(|e| panic!("built-in template {id}: {e}"))
        })
        .collect()
}

/// `config_dir/fenix/templates` -- where your own templates live.
pub fn user_templates_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("fenix").join("templates"))
}

/// Reads one template folder.
pub fn load_template_dir(dir: &Path, origin: Origin) -> Result<Template, String> {
    let id = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let toml_text = std::fs::read_to_string(dir.join("template.toml")).map_err(|e| format!("{id}: template.toml: {e}"))?;
    let mut files = Vec::new();
    collect_files(&dir.join("files"), "", &mut files).map_err(|e| format!("{id}: {e}"))?;
    Template::parse(&id, &toml_text, &files, origin).map_err(|e| format!("{id}: {e}"))
}

fn collect_files(dir: &Path, prefix: &str, out: &mut Vec<(String, String)>) -> Result<(), String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Ok(());
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let name = entry.file_name().to_string_lossy().into_owned();
        let rel = if prefix.is_empty() { name } else { format!("{prefix}/{name}") };
        let path = entry.path();
        if path.is_dir() {
            collect_files(&path, &rel, out)?;
        } else {
            let text = std::fs::read_to_string(&path).map_err(|e| format!("files/{rel}: {e} (templates hold text files only)"))?;
            out.push((rel, text));
        }
    }
    Ok(())
}

/// Every template folder directly under `dir`, each parsed on its own so
/// one broken template doesn't hide the rest.
pub fn load_templates_in(dir: &Path, origin: impl Fn(PathBuf) -> Origin) -> Vec<Result<Template, String>> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut dirs: Vec<PathBuf> = entries.flatten().map(|e| e.path()).filter(|p| p.join("template.toml").is_file()).collect();
    dirs.sort();
    dirs.into_iter().map(|d| load_template_dir(&d, origin(d.clone()))).collect()
}

/// Built-ins, then yours (shadowing a built-in of the same id), then the
/// current project's own `.fenix/templates/`. Broken templates come back
/// as messages, not a failure of the whole list.
pub fn available_templates(user_dir: Option<&Path>, project_root: Option<&Path>) -> (Vec<Template>, Vec<String>) {
    let mut templates = builtin_templates();
    let mut errors = Vec::new();
    let mut add = |result: Result<Template, String>, templates: &mut Vec<Template>| match result {
        Ok(template) => {
            templates.retain(|t| t.id != template.id);
            templates.push(template);
        }
        Err(e) => errors.push(e),
    };
    if let Some(dir) = user_dir {
        for result in load_templates_in(dir, Origin::User) {
            add(result, &mut templates);
        }
    }
    if let Some(root) = project_root {
        for result in load_templates_in(&root.join(".fenix").join("templates"), Origin::Project) {
            add(result, &mut templates);
        }
    }
    (templates, errors)
}

/// Every program named in a plan's steps plus a template's `needs`, in
/// order, once each -- what the wizard looks up on PATH.
pub fn programs_needed(template: &Template) -> Vec<String> {
    let mut out: Vec<String> = template.needs.clone();
    for run in &template.runs {
        if !out.contains(&run.exec) && !run.exec.contains("{{") {
            out.push(run.exec.clone());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::TempDir;

    fn parse(toml_text: &str) -> Result<Template, String> {
        Template::parse("t", toml_text, &[], Origin::BuiltIn)
    }

    const HEAD: &str = "[template]\nname = \"T\"\nkind = \"python\"\n";

    #[test]
    fn every_builtin_parses_and_plans_with_its_defaults() {
        let templates = builtin_templates();
        assert_eq!(templates.len(), BUILTINS.len());
        for template in templates {
            let name = if template.name_rule == NameRule::Sketch { "ServoSweep" } else { "orbit-tools" };
            let answers = template.default_answers(name);
            let plan = template.plan(name, Path::new("parent"), &answers).unwrap_or_else(|e| panic!("{}: {e}", template.id));
            assert_eq!(plan.dir, Path::new("parent").join(name));
        }
    }

    /// Every answer each ask can take -- every choice, both states of a
    /// toggle, and for a list none, each one alone, and all of them.
    fn every_answer(ask: &Ask) -> Vec<Answer> {
        match &ask.kind {
            AskKind::Text { default } => vec![Answer::Text(default.clone()), Answer::Text("a, b".into())],
            AskKind::Choice { choices, .. } => choices.iter().map(|c| Answer::Text(c.clone())).collect(),
            AskKind::Toggle { .. } => vec![Answer::Bool(true), Answer::Bool(false)],
            AskKind::Many { options, .. } => {
                let mut all = vec![Answer::Many(Vec::new()), Answer::Many(options.clone())];
                all.extend(options.iter().map(|o| Answer::Many(vec![o.clone()])));
                all
            }
        }
    }

    #[test]
    fn every_builtin_plans_cleanly_for_every_combination_of_answers() {
        for template in builtin_templates() {
            let name = if template.name_rule == NameRule::Sketch { "ServoSweep" } else { "orbit-tools" };
            let mut combinations: Vec<Answers> = vec![template.default_answers(name)];
            for ask in &template.asks {
                combinations = combinations
                    .into_iter()
                    .flat_map(|answers| {
                        every_answer(ask).into_iter().map(move |answer| {
                            let mut answers = answers.clone();
                            answers.insert(ask.key.clone(), answer);
                            answers
                        })
                    })
                    .collect();
            }
            for answers in &combinations {
                let plan = template.plan(name, Path::new("parent"), answers).unwrap_or_else(|e| panic!("{} with {answers:?}: {e}", template.id));
                for file in plan.files.iter().chain(&plan.after_files) {
                    for leftover in ["{{#if", "{{else}}", "{{/if}}", "{{name", "{{#"] {
                        assert!(!file.contents.contains(leftover), "{}: {} keeps {leftover:?} with {answers:?}:
{}", template.id, file.path, file.contents);
                    }
                    for ask in &template.asks {
                        let var = format!("{{{{{}}}}}", ask.key);
                        assert!(!file.contents.contains(&var), "{}: {} keeps {var}", template.id, file.path);
                    }
                }
            }
        }
    }

    #[test]
    fn name_variables_follow_separators_and_case() {
        let vars = name_vars("orbit-tools");
        assert_eq!((vars["name_snake"].as_str(), vars["name_kebab"].as_str(), vars["name_pascal"].as_str()), ("orbit_tools", "orbit-tools", "OrbitTools"));
        let vars = name_vars("ServoSweep");
        assert_eq!((vars["name_snake"].as_str(), vars["name_kebab"].as_str()), ("servo_sweep", "servo-sweep"));
        assert_eq!(name_vars("2024 lab")["name_snake"], "_2024_lab");
    }

    #[test]
    fn the_python_plan_runs_uv_with_the_answers_and_writes_matching_tasks() {
        let template = builtin_templates().into_iter().find(|t| t.id == "python-uv").unwrap();
        let mut answers = template.default_answers("orbit-tools");
        answers.insert("deps".into(), Answer::Text("requests rich".into()));
        answers.insert("dev".into(), Answer::Many(vec!["pytest".into()]));
        let plan = template.plan("orbit-tools", Path::new("p"), &answers).unwrap();
        let lines: Vec<String> = plan.steps.iter().map(Step::display).collect();
        assert_eq!(lines, ["uv init --app --package --python 3.12 --name orbit-tools --vcs none --no-readme", "uv add requests rich", "uv add --dev pytest",]);
        assert!(!plan.files.iter().any(|f| f.path == ".fenix/tools.json"), "written after the commands");
        let tools = plan.after_files.iter().find(|f| f.path == ".fenix/tools.json").unwrap();
        let parsed = ProjectTools::parse(&tools.contents).unwrap();
        assert_eq!(parsed.tasks.keys().collect::<Vec<_>>(), ["run", "test"], "no lint task without ruff");
        assert!(plan.files.iter().any(|f| f.path == "tests/test_smoke.py"));
        assert!(plan.files.iter().any(|f| f.path == "README.md" && f.contents.starts_with("# orbit-tools")));
    }

    #[test]
    fn the_arduino_plan_picks_one_sketch_body_and_names_it_after_the_folder() {
        let template = builtin_templates().into_iter().find(|t| t.id == "arduino-sketch").unwrap();
        let mut answers = template.default_answers("ServoSweep");
        answers.insert("start".into(), Answer::Text("serial echo".into()));
        answers.insert("port".into(), Answer::Text("COM4".into()));
        answers.insert("libraries".into(), Answer::Text("Servo, Adafruit NeoPixel".into()));
        let plan = template.plan("ServoSweep", Path::new("labs"), &answers).unwrap();
        let ino: Vec<_> = plan.files.iter().filter(|f| f.path.ends_with(".ino")).collect();
        assert_eq!(ino.len(), 1);
        assert_eq!(ino[0].path, "ServoSweep.ino");
        assert!(ino[0].contents.contains("Serial.begin(9600)") && ino[0].contents.contains("readStringUntil"));
        let yaml = plan.files.iter().find(|f| f.path == "sketch.yaml").unwrap();
        assert_eq!(yaml.contents, "default_fqbn: arduino:avr:uno\ndefault_port: COM4\n");
        assert_eq!(plan.steps[0].spec.args, ["lib", "install", "Servo", "Adafruit NeoPixel"]);
        assert!(template.plan("Servo Sweep", Path::new("labs"), &answers).is_err(), "sketch names have no spaces");
    }

    #[test]
    fn the_mib_plan_writes_only_the_chosen_tables_and_nothing_else() {
        let template = builtin_templates().into_iter().find(|t| t.id == "scos-mib").unwrap();
        let mut answers = template.default_answers("mission-c");
        answers.insert("tables".into(), Answer::Many(vec!["telecommands".into()]));
        let plan = template.plan("mission-c", Path::new("p"), &answers).unwrap();
        let paths: Vec<&str> = plan.files.iter().map(|f| f.path.as_str()).collect();
        assert!(paths.iter().all(|p| p.ends_with(".dat") && !p.contains('/')), "only tables, at the top: {paths:?}");
        assert!(paths.contains(&"ccf.dat") && !paths.contains(&"pcf.dat"));
        assert_eq!(plan.files.iter().find(|f| f.path == "vdf.dat").unwrap().contents, "mission-c\tmission-c\n");
        assert!(plan.steps.is_empty());
        assert_eq!(plan.hooks, [Hook::MibRoot { path: ".".into(), label: "mission-c".into() }]);
        assert_eq!(plan.kind, ProjectKind::Mib);
    }

    #[test]
    fn braces_in_file_contents_that_are_not_variables_survive() {
        let template = Template::parse("t", HEAD, &[("a.tcl".into(), "dict set d {{x}} {{name}} {{ y }}".into())], Origin::BuiltIn).unwrap();
        let plan = template.plan("demo", Path::new("p"), &Answers::new()).unwrap();
        assert_eq!(plan.files[0].contents, "dict set d {{x}} demo {{ y }}");
    }

    #[test]
    fn invalid_templates_are_rejected_with_a_reason() {
        let cases = [
            (format!("{HEAD}[[run]]\nexec = \"x\"\nargs = [\"{{{{nope}}}}\"]\n"), "isn't a variable"),
            (format!("{HEAD}[[ask]]\nkey = \"a\"\nlabel = \"A\"\nchoices = [\"x\"]\ndefault = \"y\"\n"), "one of its choices"),
            (format!("{HEAD}[[file]]\npath = \"../escape\"\ncontent = \"\"\n"), "inside the project"),
            (format!("{HEAD}[[file]]\npath = \"a\"\nfrom = \"missing\"\n"), "doesn't exist"),
            (format!("{HEAD}[[run]]\nexec = \"x\"\nwhen = \"nope\"\n"), "isn't an ask"),
            (format!("{HEAD}typo = 1\n"), "unknown field"),
            (format!("{HEAD}[[ask]]\nkey = \"a\"\nlabel = \"A\"\n[[run]]\nexec = \"x\"\nargs = [\"-{{{{a...}}}}\"]\n"), "whole argument"),
            ("[template]\nname = \"T\"\nkind = \"cobol\"\n".to_string(), "unknown kind"),
        ];
        for (text, expected) in cases {
            let error = parse(&text).unwrap_err();
            assert!(error.contains(expected), "{error:?} should mention {expected:?}");
        }
    }

    #[test]
    fn conditions_cover_toggles_choices_and_lists() {
        let text = format!(
            "{HEAD}[[ask]]\nkey = \"on\"\nlabel = \"On\"\ndefault = false\n[[ask]]\nkey = \"pick\"\nlabel = \"P\"\nchoices = [\"a\", \"b\"]\n[[ask]]\nkey = \"many\"\nlabel = \"M\"\nmany = [\"x\", \"y\"]\ndefault = [\"y\"]\n\
             [[file]]\npath = \"on\"\ncontent = \"\"\nwhen = \"on\"\n[[file]]\npath = \"off\"\ncontent = \"\"\nwhen = \"!on\"\n\
             [[file]]\npath = \"b\"\ncontent = \"\"\nwhen = \"pick != a\"\n[[file]]\npath = \"y\"\ncontent = \"\"\nwhen = \"many == y\"\n[[file]]\npath = \"x\"\ncontent = \"\"\nwhen = \"many == x\"\n"
        );
        let template = parse(&text).unwrap();
        let plan = template.plan("demo", Path::new("p"), &Answers::new()).unwrap();
        let paths: Vec<&str> = plan.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["off", "y"]);
    }

    #[test]
    fn blocks_keep_or_drop_lines_and_nest() {
        let text = format!(
            "{HEAD}[[ask]]\nkey = \"lang\"\nlabel = \"L\"\nchoices = [\"c\", \"cpp\"]\n[[ask]]\nkey = \"tests\"\nlabel = \"T\"\ndefault = true\n[[ask]]\nkey = \"std\"\nlabel = \"S\"\nchoices = [\"17\", \"20\"]\nwhen = \"lang == cpp\"\n"
        );
        let body = "top\n{{#if lang == cpp}}\nC++{{std}}\n  {{#if tests}}\ngtest\n  {{/if}}\n{{else}}\nC\n{{/if}}\nend {{name_upper}}\n";
        let template = Template::parse("t", &text, &[("f.txt".into(), body.into())], Origin::BuiltIn).unwrap();
        let mut answers = template.default_answers("my-lib");
        let plan = template.plan("my-lib", Path::new("p"), &answers).unwrap();
        assert_eq!(plan.files[0].contents, "top\nC\nend MY_LIB\n");
        answers.insert("lang".into(), Answer::Text("cpp".into()));
        answers.insert("std".into(), Answer::Text("20".into()));
        let plan = template.plan("my-lib", Path::new("p"), &answers).unwrap();
        assert_eq!(plan.files[0].contents, "top\nC++20\ngtest\nend MY_LIB\n");
        answers.insert("tests".into(), Answer::Bool(false));
        let plan = template.plan("my-lib", Path::new("p"), &answers).unwrap();
        assert_eq!(plan.files[0].contents, "top\nC++20\nend MY_LIB\n");
        // The C++ standard is only asked for C++.
        let std = template.asks.iter().position(|a| a.key == "std").unwrap();
        assert!(template.asks_now(std, &answers));
        answers.insert("lang".into(), Answer::Text("c".into()));
        assert!(!template.asks_now(std, &answers));
    }

    #[test]
    fn a_when_list_needs_all_and_after_files_wait_for_the_commands() {
        let text = format!(
            "{HEAD}[[ask]]\nkey = \"a\"\nlabel = \"A\"\ndefault = true\n[[ask]]\nkey = \"b\"\nlabel = \"B\"\nchoices = [\"x\", \"y\"]\n\
             [[file]]\npath = \"both\"\ncontent = \"\"\nwhen = [\"a\", \"b == y\"]\n[[file]]\npath = \"late\"\ncontent = \"\"\nafter = true\n"
        );
        let template = parse(&text).unwrap();
        let mut answers = template.default_answers("demo");
        let plan = template.plan("demo", Path::new("p"), &answers).unwrap();
        assert!(plan.files.is_empty(), "b is x, so `both` isn't written");
        assert_eq!(plan.after_files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(), ["late"]);
        answers.insert("b".into(), Answer::Text("y".into()));
        assert_eq!(template.plan("demo", Path::new("p"), &answers).unwrap().files[0].path, "both");
    }

    #[test]
    fn broken_blocks_are_refused_when_the_template_loads() {
        for (body, why) in [("{{#if nope}}\nx\n{{/if}}\n", "isn't an ask"), ("{{#if on}}\nx\n", "isn't closed"), ("x\n{{/if}}\n", "no {{#if}}"), ("{{#if on}}\n{{else}}\n{{else}}\n{{/if}}\n", "second")] {
            let text = format!("{HEAD}[[ask]]\nkey = \"on\"\nlabel = \"O\"\ndefault = true\n");
            let error = Template::parse("t", &text, &[("f".into(), body.into())], Origin::BuiltIn).unwrap_err();
            assert!(error.contains(why), "{body:?}: {error}");
        }
        let text = format!("{HEAD}[[ask]]\nkey = \"a\"\nlabel = \"A\"\nwhen = \"a\"\n");
        assert!(parse(&text).unwrap_err().contains("itself"));
    }

    #[test]
    fn names_that_cannot_be_folders_are_refused() {
        for bad in ["", " lead", "a/b", "a:b", "CON", "com3.txt", "trail."] {
            assert!(validate_name(bad, NameRule::Any).is_err(), "{bad:?}");
        }
        assert!(validate_name("orbit tools", NameRule::Any).is_ok());
        assert!(validate_name("_lab", NameRule::Sketch).is_err());
        assert!(validate_name("my-site", NameRule::Npm).is_ok());
        assert!(validate_name("MySite", NameRule::Npm).is_err());
    }

    #[test]
    fn writing_a_plan_creates_the_files_and_never_overwrites() {
        let dir = TempDir::new("template_write");
        let template = builtin_templates().into_iter().find(|t| t.id == "empty").unwrap();
        let plan = template.plan("fresh", dir.path(), &Answers::new()).unwrap();
        assert_eq!(plan.write_files().unwrap(), 1);
        assert_eq!(std::fs::read_to_string(dir.path().join("fresh/README.md")).unwrap(), "# fresh\n");
        let problem = plan.write_files().unwrap_err();
        assert!(problem.contains("already exists"), "{problem}");
    }

    #[test]
    fn a_user_template_shadows_the_builtin_and_a_broken_one_is_reported() {
        let dir = TempDir::new("template_user");
        dir.write("empty/template.toml", "[template]\nname = \"My empty\"\nkind = \"other\"\n");
        dir.write("empty/files/NOTES.md", "{{name}}");
        dir.write("broken/template.toml", "[template]\n");
        let (templates, errors) = available_templates(Some(dir.path()), None);
        let empty: Vec<_> = templates.iter().filter(|t| t.id == "empty").collect();
        assert_eq!(empty.len(), 1);
        assert_eq!(empty[0].name, "My empty");
        assert!(matches!(empty[0].origin, Origin::User(_)));
        assert_eq!(errors.len(), 1);
        assert!(errors[0].starts_with("broken:"));
    }
}
