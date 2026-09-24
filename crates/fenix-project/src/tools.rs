//! Structured, project-local process settings. Strings are literal, never shell split.
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    io,
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CommandSpec {
    pub executable: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
}

impl CommandSpec {
    pub fn new(executable: String, args: Vec<String>) -> Self {
        Self { executable, args, ..Default::default() }
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.executable.trim().is_empty() || self.executable.contains('\0') {
            return Err("executable must be a nonempty string without NUL".into());
        }
        if self.args.iter().any(|a| a.contains('\0')) {
            return Err("argument contains NUL".into());
        }
        for (key, value) in &self.env {
            if key.is_empty() || key.contains(['=', '\0']) || value.contains('\0') {
                return Err("invalid environment entry".into());
            }
        }
        #[cfg(windows)]
        {
            let mut keys = std::collections::HashSet::new();
            if self.env.keys().any(|key| !keys.insert(key.to_ascii_uppercase())) {
                return Err("duplicate case-insensitive Windows environment key".into());
            }
        }
        Ok(())
    }

    pub fn working_directory(&self, root: &Path) -> PathBuf {
        self.cwd.as_ref().map(|cwd| if cwd.is_absolute() { cwd.clone() } else { root.join(cwd) }).unwrap_or_else(|| root.into())
    }

    pub fn command(&self, root: &Path) -> io::Result<Command> {
        self.validate().map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
        let cwd = self.working_directory(root);
        let path = Path::new(&self.executable);
        // Relative executable paths are relative to the project, not the editor's cwd.
        #[allow(unused_mut)] // Windows resolves using the child PATH before CreateProcess.
        let mut executable = if path.is_absolute() {
            path.to_path_buf()
        } else if self.executable.contains(['/', '\\']) {
            root.join(path)
        } else {
            path.to_path_buf()
        };
        #[cfg(windows)]
        if !path.is_absolute() && !self.executable.contains(['/', '\\']) {
            let env_value = |name: &str| self.env.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| std::ffi::OsString::from(v)).or_else(|| std::env::var_os(name));
            let extensions = env_value("PATHEXT").unwrap_or_else(|| ".COM;.EXE;.BAT;.CMD".into());
            let extensions: Vec<String> =
                if path.extension().is_some() { vec![String::new()] } else { extensions.to_string_lossy().split(';').filter(|s| !s.is_empty()).map(str::to_owned).collect() };
            if let Some(paths) = env_value("PATH") {
                'search: for directory in std::env::split_paths(&paths) {
                    let directory = if directory.is_absolute() { directory } else { cwd.join(directory) };
                    for extension in &extensions {
                        let candidate = directory.join(format!("{}{extension}", self.executable));
                        if candidate.is_file() {
                            executable = candidate;
                            break 'search;
                        }
                    }
                }
            }
        }
        #[cfg(windows)]
        if executable == path && !path.is_absolute() && !self.executable.contains(['/', '\\']) && self.env.keys().any(|k| k.eq_ignore_ascii_case("PATH")) {
            return Err(io::Error::new(io::ErrorKind::NotFound, format!("{} was not found in the configured PATH", self.executable)));
        }
        let mut command = Command::new(executable);
        command.args(&self.args).current_dir(cwd).envs(&self.env);
        Ok(command)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Launch {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub program: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
}

impl Launch {
    pub fn is_empty(&self) -> bool {
        *self == Launch::default()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectTools {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub lsp: BTreeMap<String, CommandSpec>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub dap: BTreeMap<String, CommandSpec>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub tasks: BTreeMap<String, CommandSpec>,
    #[serde(default, skip_serializing_if = "Launch::is_empty")]
    pub launch: Launch,
}

impl ProjectTools {
    pub fn read(root: &Path) -> Result<Self, String> {
        let path = root.join(".fenix/tools.json");
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(error) => return Err(format!("{}: {error}", path.display())),
        };
        Self::parse(&text).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// The JSON `write` puts in `.fenix/tools.json`, validated first --
    /// what's written is always something `read` accepts.
    pub fn to_json(&self) -> Result<String, String> {
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())? + "\n";
        Self::parse(&text)?;
        Ok(text)
    }

    /// Writes `root/.fenix/tools.json`. Keys come out sorted and the file
    /// pretty-printed, so hand formatting doesn't survive a write; every
    /// setting does.
    pub fn write(&self, root: &Path) -> Result<(), String> {
        let text = self.to_json()?;
        let dir = root.join(".fenix");
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let path = dir.join("tools.json");
        std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// Parses and validates `tools.json` text -- `read`'s checks, for
    /// text that isn't on disk yet (a template's generated file).
    pub fn parse(text: &str) -> Result<Self, String> {
        let result: Self = serde_json::from_str(text).map_err(|e| e.to_string())?;
        for (name, command) in result.lsp.iter().chain(&result.dap).chain(&result.tasks) {
            if name.trim().is_empty() {
                return Err("empty tool name".to_string());
            }
            command.validate().map_err(|e| format!("({name}): {e}"))?;
        }
        let launch_check =
            CommandSpec { executable: "launch".into(), args: result.launch.args.clone().unwrap_or_default(), cwd: result.launch.cwd.clone(), env: result.launch.env.clone() };
        launch_check.validate().map_err(|e| format!("(launch): {e}"))?;
        Ok(result)
    }
}

/// Splits a command line typed on one line into its literal arguments:
/// whitespace separates, double quotes group (`"My Tool.exe" "a b"`),
/// and `\"` is a quote inside quotes. Backslashes are otherwise literal,
/// so Windows paths need no doubling. No variables, no globs -- the
/// result is exactly the argument vector that will run.
pub fn split_command_line(line: &str) -> Result<Vec<String>, String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut started = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                quoted = !quoted;
                started = true;
            }
            '\\' if quoted && chars.peek() == Some(&'"') => {
                current.push('"');
                chars.next();
            }
            c if c.is_whitespace() && !quoted => {
                if started {
                    args.push(std::mem::take(&mut current));
                    started = false;
                }
            }
            c => {
                current.push(c);
                started = true;
            }
        }
    }
    if quoted {
        return Err("a quote isn't closed".to_string());
    }
    if started {
        args.push(current);
    }
    Ok(args)
}

/// The inverse of `split_command_line`: each argument quoted where it
/// has to be, so splitting the result gives the same arguments back.
pub fn join_command_line(args: &[String]) -> String {
    args.iter()
        .map(|arg| {
            if !arg.is_empty() && !arg.contains(|c: char| c.is_whitespace() || c == '"') {
                arg.clone()
            } else {
                format!("\"{}\"", arg.replace('"', "\\\""))
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_lines_split_on_spaces_and_group_on_quotes() {
        assert_eq!(split_command_line("uv run pytest -q").unwrap(), ["uv", "run", "pytest", "-q"]);
        assert_eq!(split_command_line(r#""C:\My Tools\x.exe" "a b" "" c"#).unwrap(), [r"C:\My Tools\x.exe", "a b", "", "c"]);
        assert_eq!(split_command_line(r#"say "he said \"hi\"""#).unwrap(), ["say", r#"he said "hi""#]);
        assert!(split_command_line(r#"open "never closed"#).is_err());
        let args: Vec<String> = ["a", "b c", "", r#"q"t"#, r"C:\x"].map(String::from).to_vec();
        assert_eq!(split_command_line(&join_command_line(&args)).unwrap(), args);
    }

    #[test]
    fn tools_write_and_read_back_the_same() {
        let dir = tempfile::tempdir().unwrap();
        let mut tools = ProjectTools::default();
        tools.tasks.insert("test".into(), CommandSpec::new("uv".into(), vec!["run".into(), "pytest".into()]));
        tools.launch.program = Some(PathBuf::from("src/main.py"));
        tools.write(dir.path()).unwrap();
        let text = std::fs::read_to_string(dir.path().join(".fenix/tools.json")).unwrap();
        assert!(!text.contains("lsp") && !text.contains("env"), "empty sections aren't written: {text}");
        assert_eq!(ProjectTools::read(dir.path()).unwrap(), tools);
        tools.tasks.insert("bad".into(), CommandSpec::new(String::new(), vec![]));
        assert!(tools.write(dir.path()).is_err(), "an invalid setting is refused, not written");
        assert_eq!(ProjectTools::read(dir.path()).unwrap().tasks.len(), 1);
    }

    #[test]
    fn preserves_literal_windows_paths_arguments_and_environment() {
        let spec: CommandSpec =
            serde_json::from_str(r#"{"executable":"bin/my tool.exe","args":["hello world","","a&b","C:\\My Work\\file"],"cwd":"build dir","env":{"FENIX_TEST":"a b"}}"#).unwrap();
        let root = std::env::current_dir().unwrap();
        let command = spec.command(&root).unwrap();
        assert_eq!(command.get_program(), root.join("bin/my tool.exe"));
        assert_eq!(command.get_args().map(|s| s.to_str().unwrap()).collect::<Vec<_>>(), ["hello world", "", "a&b", r"C:\My Work\file"]);
        assert_eq!(command.get_current_dir(), Some(root.join("build dir").as_path()));
        assert_eq!(command.get_envs().next().unwrap().1.unwrap(), "a b");
    }
    #[test]
    fn malformed_present_configuration_does_not_fall_back() {
        let dir = tempfile::tempdir().unwrap();
        assert!(ProjectTools::read(dir.path()).unwrap().lsp.is_empty());
        std::fs::create_dir(dir.path().join(".fenix")).unwrap();
        for text in ["{", r#"{"typo":{}}"#, r#"{"lsp":{"rust":{"executable":""}}}"#] {
            std::fs::write(dir.path().join(".fenix/tools.json"), text).unwrap();
            assert!(ProjectTools::read(dir.path()).is_err());
        }
    }
    #[cfg(windows)]
    #[test]
    fn resolves_against_the_child_path_without_changing_host_environment() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("fenix-local-tool.CMD");
        std::fs::write(&exe, "@exit /b 0").unwrap();
        let before = std::env::var_os("PATH");
        let mut spec = CommandSpec::new("fenix-local-tool".into(), vec![]);
        spec.env.insert("Path".into(), dir.path().to_string_lossy().into_owned());
        spec.env.insert("PATHEXT".into(), ".CMD".into());
        assert_eq!(spec.command(dir.path()).unwrap().get_program(), exe);
        assert_eq!(std::env::var_os("PATH"), before);
    }
    #[test]
    fn project_launch_rejects_invalid_environment_and_arguments() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(".fenix")).unwrap();
        for text in [r#"{"launch":{"env":{"BAD=NAME":"x"}}}"#, r#"{"launch":{"args":["\u0000"]}}"#] {
            std::fs::write(dir.path().join(".fenix/tools.json"), text).unwrap();
            assert!(ProjectTools::read(dir.path()).is_err());
        }
    }
    #[cfg(windows)]
    #[test]
    fn missing_program_in_explicit_path_cannot_fall_back_to_parent_path() {
        let dir = tempfile::tempdir().unwrap();
        let mut spec = CommandSpec::new("cmd.exe".into(), vec![]);
        spec.env.insert("PATH".into(), dir.path().to_string_lossy().into_owned());
        assert!(spec.command(dir.path()).is_err());
        spec.env.insert("Path".into(), "duplicate".into());
        assert!(spec.validate().is_err());
    }
}
