//! Per-language default debug adapter commands and launch-argument
//! shaping for `fenix-gui`'s DAP integration -- mirrors `lsp.rs`'s own
//! role for language servers (pure logic/rendering helpers used by
//! `app.rs`, no session or threading state of its own).

use fenix_syntax::LanguageId;
use std::path::Path;

/// The obvious, common-case debug adapter for a language -- checked
/// only when nothing else names one (there is no `[dap]` config-file
/// override yet, unlike `lsp::resolve_server_command`'s `[lsp]` --
/// genuinely one adapter is all this needs for now). Python is the only
/// language with a live, verified adapter today: `debugpy`'s own
/// `python -m debugpy.adapter` speaks DAP over stdio with no extra
/// setup once `pip install debugpy` has been run, matching pyright's
/// own "the obvious server, no config needed" bar. Native debugging
/// (Rust/C++ via `lldb-dap`/`codelldb`) is deliberately not defaulted
/// here yet -- the plan's own Milestone D targets those, but neither
/// adapter was available to verify against in this environment, and a
/// wrong guess at the binary name would be worse than no default at
/// all (a clear "no known adapter for language" error, same posture
/// `lsp::default_server_command` already has for anything beyond its
/// own short list).
pub fn default_adapter_command(language: LanguageId) -> Option<(String, Vec<String>)> {
    match language {
        LanguageId::Python => Some(("python".to_string(), vec!["-m".to_string(), "debugpy.adapter".to_string()])),
        _ => None,
    }
}

/// A debug adapter, and how Fenix talks to it: over its stdin and stdout,
/// or a socket (`debug-adapter-connect`).
#[derive(Debug, Clone, PartialEq)]
pub struct Adapter {
    /// What starts it; none for one already running at `port`.
    pub command: Option<fenix_project::tools::CommandSpec>,
    /// Over a socket at 127.0.0.1 this port: 0 a free one Fenix picks,
    /// which `${port}` in the command's args and env becomes.
    pub port: Option<u16>,
}

impl Adapter {
    /// One that speaks on its stdin and stdout.
    pub fn stdio(command: fenix_project::tools::CommandSpec) -> Adapter {
        Adapter { command: Some(command), port: None }
    }

    /// What it is, for a session's header: its command, and where it's
    /// connected to.
    pub fn describe(&self, port: Option<u16>) -> String {
        let command = self.command.as_ref().map(|c| format!("{} {}", c.executable, c.args.join(" ")).trim_end().to_string());
        match (command, port) {
            (Some(command), None) => command,
            (Some(command), Some(port)) => format!("{command} (127.0.0.1:{port})"),
            (None, Some(port)) => format!("the adapter at 127.0.0.1:{port}"),
            (None, None) => String::new(),
        }
    }

    /// The adapter a child session it asked for (`startDebugging`)
    /// connects to: over a socket, another connection to the port it's
    /// listening on, its program not started again (js-debug serves a
    /// session per connection); on stdio, the program started again.
    pub fn for_child(&self, port: Option<u16>) -> Adapter {
        match port.or(self.port.filter(|p| *p != 0)) {
            Some(port) => Adapter { command: None, port: Some(port) },
            None => self.clone(),
        }
    }

    /// Started and connected to, in `root`: the client, its events, and
    /// the port it's on when over a socket.
    pub fn connect(&self, root: &Path) -> std::io::Result<(fenix_dap::DapClient, std::sync::mpsc::Receiver<fenix_dap::DapEvent>, Option<u16>)> {
        match (self.port, &self.command) {
            (None, Some(command)) => fenix_dap::DapClient::spawn_command(command.command(root)?).map(|(c, rx)| (c, rx, None)),
            (None, None) => Err(std::io::Error::other("no debug adapter to start")),
            (Some(port), command) => {
                let port = if port == 0 { fenix_dap::free_port()? } else { port };
                let process = command.as_ref().map(|c| with_port(c, port).command(root)).transpose()?;
                fenix_dap::DapClient::connect_command(process, port).map(|(c, rx)| (c, rx, Some(port)))
            }
        }
    }
}

/// `command` with `${port}` in its args and env as `port`.
pub fn with_port(command: &fenix_project::tools::CommandSpec, port: u16) -> fenix_project::tools::CommandSpec {
    let fill = |s: &String| s.replace("${port}", &port.to_string());
    fenix_project::tools::CommandSpec { args: command.args.iter().map(fill).collect(), env: command.env.iter().map(|(k, v)| (k.clone(), fill(v))).collect(), ..command.clone() }
}

/// The `launch` request's adapter-specific `arguments` object for
/// `language`, debugging `program` -- DAP deliberately leaves this
/// entirely up to each adapter (see `LaunchRequestArguments::
/// additional_attributes`'s own doc comment in the `debug-adapter-
/// protocol` crate), so there's no generic shape to build here, only a
/// per-adapter one. `debugpy`'s: `program` (the script to run) and
/// `console: internalConsole` (debugpy's own stdout/stderr are already
/// piped through this app's `DapClient`/`output` events -- there's no
/// separate terminal to launch it in the way `integratedTerminal`/
/// `externalTerminal` would need). `justMyCode` is deliberately left
/// unset -- debugpy's own default (skip stepping into library code) is
/// the right one for everyday use; the live-verification probe this
/// was built against set it to `false` specifically to *exercise*
/// stepping into library frames, not because that's the right default.
pub fn launch_arguments(language: LanguageId, program: &Path) -> serde_json::Map<String, serde_json::Value> {
    let mut args = serde_json::Map::new();
    match language {
        LanguageId::Python => {
            args.insert("program".to_string(), serde_json::Value::String(program.to_string_lossy().into_owned()));
            args.insert("console".to_string(), serde_json::Value::String("internalConsole".to_string()));
        }
        _ => {
            args.insert("program".to_string(), serde_json::Value::String(program.to_string_lossy().into_owned()));
        }
    }
    args
}

/// A launch configuration an installed extension offers, from its
/// `launch/*.toml` -- what `SPC u u` offers when a project has none of its
/// own (`.fenix/tools.json`'s `launch`):
///
/// ```toml
/// name = "Python: current file"
/// languages = ["python"]
/// request = "launch"          # or "attach"
/// program = "${file}"         # the file you're in when it's left out;
///                             # false sends none
/// args = ["-v"]
/// cwd = "${project}"
/// env = { PYTHONUNBUFFERED = "1" }
/// preLaunchTask = "build"     # run first; the session starts once it
///                             # succeeds
///
/// [attributes]                # the rest of what the adapter takes
/// justMyCode = true
/// ```
///
/// `${file}`, `${fileDir}` and `${project}` in its strings are the file
/// you're in, its folder and the project's; `${module}` is the file as a
/// dotted module of the project's (`pkg/cli/main.py` is `pkg.cli.main`;
/// a package's `__init__.py`, the package). A launch that leaves `program`
/// out sends none when its attributes say what runs another way --
/// debugpy's `module` or `code`, or `program` itself.
///
/// `${pickProcess}` is a process of this machine's you pick, by
/// its id; any other `${name}` is a variable an extension fills in
/// (`launch-variable-set`), picked from its choices when it has several.
/// A value that's only a variable, filled with a whole number, is sent as
/// a number (`processId = "${pickProcess}"`).
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Template {
    pub name: String,
    /// Fenix's names for the languages it's for (`python`).
    pub languages: Vec<String>,
    #[serde(default)]
    pub request: Request,
    pub program: Option<Program>,
    #[serde(default)]
    pub args: Vec<String>,
    pub cwd: Option<String>,
    #[serde(default)]
    pub env: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    pub attributes: toml::Table,
    /// A task run first, by name: the session starts once it succeeds.
    #[serde(default, rename = "preLaunchTask", alias = "pre_launch_task")]
    pub pre_launch_task: Option<String>,
    /// Its variables as they've been filled in so far (picked, or an
    /// extension's one choice): not in the file.
    #[serde(skip)]
    pub filled: std::collections::BTreeMap<String, String>,
}

/// The attribute naming an adapter already listening (`Template::debug_server`).
const DEBUG_SERVER: &str = "debugServer";

/// Fenix's own variables in launch files, which it fills in itself.
pub const OWN_VARIABLES: &[&str] = &["file", "fileDir", "project", "module"];

/// The variable a process picked fills (`${pickProcess}`).
pub const PICK_PROCESS: &str = "pickProcess";

/// The `${name}`s in `text`, in order.
fn variables_in(text: &str, out: &mut Vec<String>) {
    let mut rest = text;
    while let Some(at) = rest.find("${") {
        rest = &rest[at + 2..];
        let Some(end) = rest.find('}') else { return };
        let name = &rest[..end];
        if !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.')) && !out.iter().any(|n| n == name) {
            out.push(name.to_string());
        }
        rest = &rest[end + 1..];
    }
}

fn variables_in_value(value: &toml::Value, out: &mut Vec<String>) {
    match value {
        toml::Value::String(s) => variables_in(s, out),
        toml::Value::Array(items) => items.iter().for_each(|v| variables_in_value(v, out)),
        toml::Value::Table(t) => t.values().for_each(|v| variables_in_value(v, out)),
        _ => {}
    }
}

/// A launch file's `program`: a path, or `false` for none.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(untagged)]
pub enum Program {
    Path(String),
    Sent(bool),
}

/// What runs, besides a program, in a launch's attributes (debugpy's).
const RUNS_OTHERWISE: &[&str] = &["module", "code", "program"];

/// Whether a launch configuration starts the program or attaches to one.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Request {
    #[default]
    Launch,
    Attach,
}

/// A launch file's text, read and checked.
pub fn parse_template(text: &str) -> Result<Template, String> {
    let template: Template = toml::from_str(text).map_err(|e| e.message().to_string())?;
    if template.name.trim().is_empty() {
        return Err("it has no name".into());
    }
    if template.languages.is_empty() {
        return Err("it names no languages".into());
    }
    if template.program == Some(Program::Sent(true)) {
        return Err("program is a path, or false for none".into());
    }
    Ok(template)
}

/// The launch files in `dirs` for language `language` (Fenix's name for
/// it), sorted by name, and what's wrong with any that aren't.
pub fn templates_for(dirs: &[std::path::PathBuf], language: &str) -> (Vec<Template>, Vec<String>) {
    let (mut found, mut problems) = (Vec::new(), Vec::new());
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else { continue };
        let mut files: Vec<_> = entries.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("toml"))).collect();
        files.sort();
        for file in files {
            match std::fs::read_to_string(&file).map_err(|e| e.to_string()).and_then(|text| parse_template(&text)) {
                Ok(t) if t.languages.iter().any(|l| l == language) => found.push(t),
                Ok(_) => {}
                Err(e) => problems.push(format!("{}: {e}", file.display())),
            }
        }
    }
    found.sort_by(|a, b| a.name.cmp(&b.name));
    (found, problems)
}

impl Template {
    /// A launch an extension gave as it runs (`launches-set`), as a
    /// launch file's: its program none sends none.
    pub fn from_config(config: fenix_wasm::host::LaunchConfig) -> Result<Template, String> {
        let attributes: serde_json::Value = serde_json::from_str(if config.attributes.trim().is_empty() { "{}" } else { &config.attributes }).map_err(|e| format!("{}: {e}", config.name))?;
        let attributes = match toml::Value::try_from(without_nulls(attributes)).map_err(|e| format!("{}: {e}", config.name))? {
            toml::Value::Table(t) => t,
            _ => return Err(format!("{}: its attributes are an object", config.name)),
        };
        Ok(Template {
            name: config.name,
            languages: config.languages,
            request: if config.request == fenix_wasm::host::LaunchRequest::Attach { Request::Attach } else { Request::Launch },
            program: Some(config.program.map(Program::Path).unwrap_or(Program::Sent(false))),
            args: config.args,
            cwd: config.cwd,
            env: config.env.into_iter().collect(),
            attributes,
            pre_launch_task: config.pre_launch_task,
            filled: Default::default(),
        })
    }

    /// The project's own launch (`.fenix/tools.json`'s `launch`) as a launch
    /// file's, for `language` (Fenix's name for it): so its variables --
    /// `${pickProcess}`, an extension's -- are filled in, and an attach and
    /// its attributes sent.
    pub fn from_project(launch: &fenix_project::tools::Launch, language: &str) -> Result<Template, String> {
        let attributes = match toml::Value::try_from(without_nulls(serde_json::json!(launch.attributes))).map_err(|e| format!("tools.json launch: {e}"))? {
            toml::Value::Table(t) => t,
            _ => return Err("tools.json launch: its attributes are an object".into()),
        };
        let program = match (&launch.program, launch.no_program) {
            (Some(p), _) => Some(Program::Path(p.to_string_lossy().into_owned())),
            (None, true) => Some(Program::Sent(false)),
            (None, false) => None,
        };
        Ok(Template {
            name: "tools.json".into(),
            languages: vec![language.to_string()],
            request: if launch.attach { Request::Attach } else { Request::Launch },
            program,
            args: launch.args.clone().unwrap_or_default(),
            cwd: launch.cwd.as_ref().map(|c| c.to_string_lossy().into_owned()),
            env: launch.env.clone(),
            attributes,
            pre_launch_task: launch.pre_launch_task.clone(),
            filled: Default::default(),
        })
    }

    /// `debugServer` in its attributes, as VS Code's launch files have it:
    /// the port of an adapter already listening on this machine -- `python
    /// -m debugpy --listen 5678` runs debugpy's -- connected to rather than
    /// the language's adapter started. Not sent to it.
    pub fn debug_server(&self) -> Option<u16> {
        match self.attributes.get(DEBUG_SERVER)? {
            toml::Value::Integer(port) => u16::try_from(*port).ok().filter(|p| *p > 0),
            toml::Value::String(port) => self.fill(port, None, Path::new("")).trim().parse().ok().filter(|p: &u16| *p > 0),
            _ => None,
        }
    }

    /// Whether it has more than Fenix's own variables to fill in.
    pub fn has_variables(&self) -> bool {
        self.variables().iter().any(|v| !OWN_VARIABLES.contains(&v.as_str()))
    }

    /// The `${name}`s in its strings, in order: Fenix's own among them.
    pub fn variables(&self) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(Program::Path(p)) = &self.program {
            variables_in(p, &mut out);
        }
        self.args.iter().chain(&self.cwd).chain(self.env.values()).for_each(|t| variables_in(t, &mut out));
        self.attributes.values().for_each(|v| variables_in_value(v, &mut out));
        out
    }

    /// `${file}`, `${fileDir}`, `${module}` and `${project}` in `text`,
    /// filled in, and the variables picked or given (`filled`).
    fn fill(&self, text: &str, file: Option<&Path>, root: &Path) -> String {
        let shown = |p: &Path| p.to_string_lossy().into_owned();
        let dir = file.and_then(Path::parent).map(shown).unwrap_or_else(|| shown(root));
        let text = if text.contains("${module}") { text.replace("${module}", &file.map(|f| module_of(f, root)).unwrap_or_default()) } else { text.to_string() };
        let mut text = text.replace("${fileDir}", &dir).replace("${file}", &file.map(shown).unwrap_or_default()).replace("${project}", &shown(root));
        for (name, value) in &self.filled {
            text = text.replace(&format!("${{{name}}}"), value);
        }
        text
    }

    fn fill_value(&self, value: toml::Value, file: Option<&Path>, root: &Path) -> serde_json::Value {
        match value {
            // Only a variable, a whole number: a number (`processId`).
            toml::Value::String(s) if s.starts_with("${") && s.ends_with('}') && self.filled.get(&s[2..s.len() - 1]).is_some_and(|v| v.parse::<i64>().is_ok()) => {
                serde_json::json!(self.filled[&s[2..s.len() - 1]].parse::<i64>().unwrap_or_default())
            }
            toml::Value::String(s) => serde_json::Value::String(self.fill(&s, file, root)),
            toml::Value::Integer(i) => serde_json::json!(i),
            toml::Value::Float(f) => serde_json::json!(f),
            toml::Value::Boolean(b) => serde_json::Value::Bool(b),
            toml::Value::Datetime(d) => serde_json::Value::String(d.to_string()),
            toml::Value::Array(items) => serde_json::Value::Array(items.into_iter().map(|v| self.fill_value(v, file, root)).collect()),
            toml::Value::Table(t) => serde_json::Value::Object(t.into_iter().map(|(k, v)| (k, self.fill_value(v, file, root))).collect()),
        }
    }

    /// What `launch` (or `attach`) is sent with, for `language`, from the
    /// file you're in (`file`) in the project at `root`: what Fenix sends
    /// for the language, then its own program, args, cwd and env, then
    /// its attributes over them. An attach names no program unless it
    /// says one.
    pub fn arguments(&self, language: LanguageId, file: Option<&Path>, root: &Path) -> Result<serde_json::Map<String, serde_json::Value>, String> {
        let runs_otherwise = RUNS_OTHERWISE.iter().any(|k| self.attributes.contains_key(*k));
        let program = match (&self.program, self.request) {
            (Some(Program::Path(p)), _) => Some(std::path::PathBuf::from(self.fill(p, file, root))),
            (Some(Program::Sent(_)), _) => None,
            (None, Request::Launch) if runs_otherwise => None,
            (None, Request::Launch) => Some(file.ok_or("no file to debug -- open the one to run")?.to_path_buf()),
            (None, Request::Attach) => None,
        };
        let mut args = match &program {
            Some(program) => launch_arguments(language, &if program.is_absolute() { program.clone() } else { root.join(program) }),
            // What Fenix sends for the language, but the program.
            None if self.request == Request::Launch => {
                let mut args = launch_arguments(language, Path::new(""));
                args.remove("program");
                args
            }
            None => serde_json::Map::new(),
        };
        if self.request == Request::Launch {
            args.insert("args".into(), serde_json::json!(self.args.iter().map(|a| self.fill(a, file, root)).collect::<Vec<_>>()));
            let cwd = self.cwd.as_deref().map(|c| self.fill(c, file, root)).unwrap_or_else(|| root.to_string_lossy().into_owned());
            args.insert("cwd".into(), serde_json::json!(cwd));
            args.insert("env".into(), serde_json::json!(self.env.iter().map(|(k, v)| (k.clone(), self.fill(v, file, root))).collect::<std::collections::BTreeMap<_, _>>()));
        }
        for (key, value) in self.attributes.clone().into_iter().filter(|(k, _)| k != DEBUG_SERVER) {
            args.insert(key, self.fill_value(value, file, root));
        }
        Ok(args)
    }
}

/// `value` without its nulls, which TOML has no room for.
fn without_nulls(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => serde_json::Value::Object(map.into_iter().filter(|(_, v)| !v.is_null()).map(|(k, v)| (k, without_nulls(v))).collect()),
        serde_json::Value::Array(items) => serde_json::Value::Array(items.into_iter().filter(|v| !v.is_null()).map(without_nulls).collect()),
        other => other,
    }
}

/// `file` as a dotted module of the project at `root`: its path there,
/// its extension off, `/` as `.`; a package's `__init__`, the package.
fn module_of(file: &Path, root: &Path) -> String {
    let rel = file.strip_prefix(root).unwrap_or(file).with_extension("");
    let mut parts: Vec<String> = rel.components().filter_map(|c| match c {
        std::path::Component::Normal(p) => Some(p.to_string_lossy().into_owned()),
        _ => None,
    }).collect();
    if parts.len() > 1 && parts.last().is_some_and(|p| p == "__init__") {
        parts.pop();
    }
    parts.join(".")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// a launch whose attributes say what runs (`module`) sends no
    /// program, nor one that says `program = false`; `${module}` is the
    /// file you're in as a module.
    #[test]
    fn a_launch_may_send_no_program_and_name_the_module() {
        let (root, file) = (PathBuf::from("/p"), PathBuf::from("/p/pkg/cli/main.py"));
        let t = parse_template("name = \"m\"\nlanguages = [\"python\"]\n[attributes]\nmodule = \"${module}\"\n").unwrap();
        let args = t.arguments(LanguageId::Python, Some(&file), &root).unwrap();
        assert!(args.get("program").is_none(), "{args:?}");
        assert_eq!(args["module"], "pkg.cli.main");
        assert_eq!(args["console"], "internalConsole", "the rest of what Fenix sends for the language");
        let t = parse_template("name = \"n\"\nlanguages = [\"python\"]\nprogram = false\n").unwrap();
        assert!(t.arguments(LanguageId::Python, None, &root).unwrap().get("program").is_none(), "no file needed");
        assert!(parse_template("name = \"n\"\nlanguages = [\"python\"]\nprogram = true\n").unwrap_err().contains("false for none"));
        assert_eq!(module_of(Path::new("/p/pkg/__init__.py"), &root), "pkg");
        assert_eq!(module_of(Path::new("/p/main.py"), &root), "main");
    }

    /// a launch file's `preLaunchTask`, its variables found and
    /// filled in -- a whole number alone sent as a number -- and a launch
    /// an extension gives read as a launch file.
    #[test]
    fn a_launchs_variables_are_found_and_filled_in() {
        let text = "name = \"attach\"\nlanguages = [\"go\"]\nrequest = \"attach\"\npreLaunchTask = \"build\"\nargs = [\"${target}\"]\n[attributes]\nprocessId = \"${pickProcess}\"\nlabel = \"pid ${pickProcess} of ${file}\"\n";
        let mut t = parse_template(text).unwrap();
        assert_eq!(t.pre_launch_task.as_deref(), Some("build"));
        assert_eq!(t.variables(), ["target", "pickProcess", "file"]);
        t.filled.insert("pickProcess".into(), "4242".into());
        let args = t.arguments(LanguageId::Go, None, Path::new("/p")).unwrap();
        assert_eq!((args["processId"].clone(), args["label"].clone()), (serde_json::json!(4242), serde_json::json!("pid 4242 of ")));
        let given = fenix_wasm::host::LaunchConfig {
            name: "app".into(), languages: vec!["cpp".into()], request: fenix_wasm::host::LaunchRequest::Launch, program: None, args: Vec::new(), cwd: Some("${project}/build".into()),
            env: vec![("A".into(), "1".into())], attributes: r#"{"type": "lldb", "sourceMap": null, "init": ["a", null]}"#.into(), pre_launch_task: None,
        };
        let t = Template::from_config(given).unwrap();
        assert_eq!(t.program, Some(Program::Sent(false)), "none sends none");
        let args = t.arguments(LanguageId::Cpp, None, Path::new("/p")).unwrap();
        assert!(args.get("program").is_none() && args.get("sourceMap").is_none(), "{args:?}");
        assert_eq!((args["type"].clone(), args["init"].clone(), args["env"]["A"].clone()), (serde_json::json!("lldb"), serde_json::json!(["a"]), serde_json::json!("1")));
        assert!(Template::from_config(fenix_wasm::host::LaunchConfig { attributes: "[1]".into(), ..fenix_wasm::host::LaunchConfig { name: "x".into(), languages: vec![], request: fenix_wasm::host::LaunchRequest::Launch, program: None, args: vec![], cwd: None, env: vec![], attributes: String::new(), pre_launch_task: None } }).is_err());
    }

    /// a child session goes to the same port, its program not
    /// started again; on stdio, the program again.
    #[test]
    fn a_child_session_connects_where_its_parent_did() {
        let command = fenix_project::tools::CommandSpec::new("node".into(), vec!["dapDebugServer.js".into(), "${port}".into()]);
        let socket = Adapter { command: Some(command.clone()), port: Some(0) };
        assert_eq!(socket.for_child(Some(4711)), Adapter { command: None, port: Some(4711) });
        assert_eq!(Adapter { command: None, port: Some(4711) }.for_child(None), Adapter { command: None, port: Some(4711) });
        assert_eq!(Adapter::stdio(command.clone()).for_child(None), Adapter::stdio(command));
    }

    #[test]
    fn a_launch_file_is_read_and_filled_in_for_the_file_youre_in() {
        let text = r#"
name = "Python: module"
languages = ["python"]
args = ["${file}", "-v"]
cwd = "${fileDir}"
env = { ROOT = "${project}" }

[attributes]
subProcess = true
justMyCode = false
paths = ["${project}/src"]
"#;
        let t = parse_template(text).unwrap();
        assert_eq!(t.request, Request::Launch);
        let (root, file) = (PathBuf::from("/p"), PathBuf::from("/p/src/a.py"));
        let args = t.arguments(LanguageId::Python, Some(&file), &root).unwrap();
        assert_eq!(args["program"], serde_json::json!(file.to_string_lossy()), "the file you're in, when it names none");
        assert_eq!(args["console"], "internalConsole", "what Fenix sends for the language");
        assert_eq!(args["args"], serde_json::json!([file.to_string_lossy(), "-v"]));
        assert_eq!(args["cwd"], serde_json::json!(PathBuf::from("/p/src").to_string_lossy()));
        assert_eq!(args["env"]["ROOT"], serde_json::json!(root.to_string_lossy()));
        assert_eq!((args["subProcess"].as_bool(), args["justMyCode"].as_bool()), (Some(true), Some(false)));
        assert_eq!(args["paths"], serde_json::json!([format!("{}/src", root.to_string_lossy())]));
    }

    /// `debugServer`: the adapter already listening there is the one
    /// talked to (`python -m debugpy --listen 5678`), and it isn't sent.
    #[test]
    fn a_launch_naming_a_debug_server_is_connected_there() {
        let attach = parse_template("name = \"Python: attach\"\nlanguages = [\"python\"]\nrequest = \"attach\"\n\n[attributes]\ndebugServer = 5678\njustMyCode = true\n").unwrap();
        assert_eq!(attach.debug_server(), Some(5678));
        let args = attach.arguments(LanguageId::Python, None, Path::new("/p")).unwrap();
        assert!(!args.contains_key("debugServer") && args["justMyCode"] == true, "{args:?}");
        let named = parse_template("name = \"x\"\nlanguages = [\"python\"]\nrequest = \"attach\"\n\n[attributes]\ndebugServer = \"5679\"\n").unwrap();
        assert_eq!(named.debug_server(), Some(5679));
        let none = parse_template("name = \"x\"\nlanguages = [\"python\"]\nrequest = \"attach\"\n\n[attributes]\ndebugServer = 0\n").unwrap();
        assert_eq!(none.debug_server(), None);
    }

    #[test]
    fn an_attach_names_no_program_and_a_bad_launch_file_says_why() {
        let t = parse_template("name = \"Attach\"\nlanguages = [\"go\"]\nrequest = \"attach\"\n[attributes]\nmode = \"local\"\nprocessId = 42\n").unwrap();
        let args = t.arguments(LanguageId::Go, None, Path::new("/p")).unwrap();
        assert!(args.get("program").is_none() && args.get("cwd").is_none());
        assert_eq!(args["processId"], 42);
        assert!(parse_template("name = \"x\"\nlanguages = []\n").unwrap_err().contains("no languages"));
        assert!(parse_template("name = \"x\"\nlanguages = [\"go\"]\nrequest = \"run\"\n").is_err());
        assert!(parse_template("name = \"x\"\nlanguages = [\"go\"]\nprogam = \"typo\"\n").is_err(), "an unknown key is a mistake");
        let launch = parse_template("name = \"x\"\nlanguages = [\"go\"]\n").unwrap();
        assert!(launch.arguments(LanguageId::Go, None, Path::new("/p")).is_err(), "a launch with no program needs a file");
    }

    #[test]
    fn launch_files_are_found_for_their_language_sorted_by_name() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("b.toml"), "name = \"Python: b\"\nlanguages = [\"python\"]\n").unwrap();
        std::fs::write(dir.path().join("a.toml"), "name = \"Python: z\"\nlanguages = [\"python\", \"rust\"]\n").unwrap();
        std::fs::write(dir.path().join("c.toml"), "name = \"Go\"\nlanguages = [\"go\"]\n").unwrap();
        std::fs::write(dir.path().join("bad.toml"), "name = 1\n").unwrap();
        let (found, problems) = templates_for(&[dir.path().to_path_buf()], "python");
        assert_eq!(found.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), ["Python: b", "Python: z"]);
        assert_eq!(problems.len(), 1);
    }

    #[test]
    fn python_has_a_built_in_default_adapter_command() {
        assert_eq!(default_adapter_command(LanguageId::Python), Some(("python".to_string(), vec!["-m".to_string(), "debugpy.adapter".to_string()])));
    }

    #[test]
    fn a_language_with_no_verified_adapter_resolves_to_none() {
        assert_eq!(default_adapter_command(LanguageId::Rust), None);
        assert_eq!(default_adapter_command(LanguageId::Cpp), None);
    }

    #[test]
    fn python_launch_arguments_include_the_program_and_internal_console() {
        let args = launch_arguments(LanguageId::Python, &PathBuf::from("C:/proj/main.py"));
        assert_eq!(args.get("program").and_then(|v| v.as_str()), Some("C:/proj/main.py"));
        assert_eq!(args.get("console").and_then(|v| v.as_str()), Some("internalConsole"));
    }
}
