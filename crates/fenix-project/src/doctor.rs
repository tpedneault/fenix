//! The project doctor (`SPC p h`): every check a project's kind needs,
//! each with the fix for it where there is one. "Why is there no
//! autocompletion?" is almost always one of a handful of things -- a
//! missing language server, a venv behind its lock file, a board package
//! not installed -- and the doctor lists them all at once with the same
//! health vocabulary the rest of Fenix uses.
//!
//! What the doctor needs from the outside world -- where a program is,
//! what it prints, whether a MIB root is registered -- comes through a
//! `Probe`, so the checks themselves are testable against a fake one.
//! A quick pass (`deep: false`) only looks at files and PATH; a deep one
//! also runs programs (`--version`, `uv sync --dry-run`, `arduino-cli
//! board list`) and belongs off the UI thread.

use std::path::{Path, PathBuf};

use crate::kind::ProjectKind;
use crate::template::Step;
use crate::tools::ProjectTools;

/// How a check came out, least to most serious.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Health {
    Ok,
    Info,
    Warn,
    Bad,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Toolchain,
    Hardware,
    Database,
    Project,
}

impl Section {
    pub fn title(self) -> &'static str {
        match self {
            Section::Toolchain => "Toolchain",
            Section::Hardware => "Hardware",
            Section::Database => "Database",
            Section::Project => "Project",
        }
    }
}

/// Something only the editor can do to fix a check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditorFix {
    PickBoard,
    PickPort,
    RegisterMibRoot { path: PathBuf, label: String },
}

#[derive(Debug, Clone)]
pub enum FixAction {
    /// A command, run in the project's root.
    Run(Step),
    Editor(EditorFix),
}

#[derive(Debug, Clone)]
pub struct Fix {
    /// What the fix row says: "uv sync", "pick a port".
    pub label: String,
    pub action: FixAction,
    /// Local to the project and undoable (sync a venv, `git init`) --
    /// what "fix all" may run without asking. Installing anything is
    /// never safe in this sense.
    pub safe: bool,
}

#[derive(Debug, Clone)]
pub struct Check {
    pub section: Section,
    pub label: String,
    pub detail: String,
    pub health: Health,
    pub fix: Option<Fix>,
    /// A file (and 1-based line) the check is about, for `Enter`.
    pub location: Option<(PathBuf, usize)>,
}

impl Check {
    fn new(section: Section, label: impl Into<String>, health: Health, detail: impl Into<String>) -> Self {
        Check { section, label: label.into(), detail: detail.into(), health, fix: None, location: None }
    }

    fn fix(mut self, label: &str, action: FixAction, safe: bool) -> Self {
        self.fix = Some(Fix { label: label.to_string(), action, safe });
        self
    }

    fn run(self, label: &str, program: &str, args: &[&str], safe: bool) -> Self {
        self.fix(label, FixAction::Run(Step::new(program, args)), safe)
    }

    fn at(mut self, path: PathBuf, line: usize) -> Self {
        self.location = Some((path, line));
        self
    }
}

/// The worst health among `checks` -- a project's health dot.
pub fn worst(checks: &[Check]) -> Health {
    checks.iter().map(|c| c.health).max().unwrap_or(Health::Ok)
}

/// What the doctor asks of the world.
pub trait Probe {
    /// Where `program` is, if Fenix can find it.
    fn locate(&self, program: &str) -> Option<PathBuf>;
    /// Runs `program` in `dir`: whether it succeeded, and everything it
    /// printed (both streams). Only called on a deep pass.
    fn run(&self, program: &Path, args: &[&str], dir: &Path) -> Option<(bool, String)>;
    /// Whether `dir` is a configured `[mib]` root.
    fn mib_registered(&self, dir: &Path) -> bool;
}

/// Looks for `program` on PATH (with Windows' PATHEXT), for a `Probe`
/// that has nothing better.
pub fn which(program: &str) -> Option<PathBuf> {
    let path = Path::new(program);
    if path.is_absolute() || program.contains(['/', '\\']) {
        return path.is_file().then(|| path.to_path_buf());
    }
    let extensions: Vec<String> = if cfg!(windows) {
        std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_string()).split(';').filter(|e| !e.is_empty()).map(str::to_string).collect()
    } else {
        vec![String::new()]
    };
    std::env::split_paths(&std::env::var_os("PATH")?)
        .flat_map(|dir| extensions.iter().map(move |ext| dir.join(format!("{program}{ext}"))))
        .find(|candidate| candidate.is_file())
}

/// Runs `program` with no console window and returns (success, output).
pub fn run_program(program: &Path, args: &[&str], dir: &Path) -> Option<(bool, String)> {
    let mut command = std::process::Command::new(program);
    command.args(args).current_dir(dir).stdin(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let output = command.output().ok()?;
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    Some((output.status.success(), text))
}

/// The first thing in `text` that looks like a version number.
fn version_in(text: &str) -> Option<String> {
    text.split(|c: char| c.is_whitespace() || c == ',' || c == '(' || c == ')')
        .map(|w| w.trim_start_matches('v'))
        .find(|w| w.contains('.') && w.starts_with(|c: char| c.is_ascii_digit()))
        .map(str::to_string)
}

struct Doctor<'a> {
    root: &'a Path,
    probe: &'a dyn Probe,
    deep: bool,
    checks: Vec<Check>,
}

impl Doctor<'_> {
    /// A program's check: found (with its version on a deep pass) or
    /// `missing` with `hint`.
    fn program(&mut self, section: Section, program: &str, missing: Health, hint: &str) -> Option<PathBuf> {
        match self.probe.locate(program) {
            Some(path) => {
                // Nearly everything answers `--version`; Arduino CLI wants
                // a subcommand.
                let args: &[&str] = if program == "arduino-cli" { &["version"] } else { &["--version"] };
                let version = if self.deep { self.probe.run(&path, args, self.root).and_then(|(_, out)| version_in(&out)) } else { None };
                let detail = version.unwrap_or_else(|| path.display().to_string());
                self.checks.push(Check::new(section, program, Health::Ok, detail));
                Some(path)
            }
            None => {
                self.checks.push(Check::new(section, program, missing, format!("not found -- {hint}")));
                None
            }
        }
    }

    fn push(&mut self, check: Check) {
        self.checks.push(check);
    }
}

/// Every check `root` (a project of `kind`) needs, in display order.
pub fn diagnose(root: &Path, kind: ProjectKind, probe: &dyn Probe, deep: bool) -> Vec<Check> {
    let mut d = Doctor { root, probe, deep, checks: Vec::new() };
    let tools = ProjectTools::read(root);
    match kind {
        ProjectKind::Python => python(&mut d, tools.as_ref().ok()),
        ProjectKind::Rust => {
            d.program(Section::Toolchain, "cargo", Health::Bad, "install Rust from https://rustup.rs");
            if d.probe.locate("rust-analyzer").is_none() {
                d.push(Check::new(Section::Toolchain, "rust-analyzer", Health::Warn, "not found -- no completion or errors as you type").run("rustup component add rust-analyzer", "rustup", &["component", "add", "rust-analyzer"], false));
            } else {
                d.program(Section::Toolchain, "rust-analyzer", Health::Warn, "");
            }
        }
        ProjectKind::Arduino => arduino(&mut d),
        ProjectKind::Mib => mib(&mut d),
        ProjectKind::Tcl => {
            d.program(Section::Toolchain, "tclsh", Health::Warn, "install Tcl to run scripts");
        }
        ProjectKind::Cpp => {
            d.program(Section::Toolchain, "cmake", Health::Warn, "install CMake to configure and build");
            d.program(Section::Toolchain, "clangd", Health::Warn, "no completion or errors as you type");
        }
        ProjectKind::Go => {
            d.program(Section::Toolchain, "go", Health::Bad, "install Go from https://go.dev");
            d.program(Section::Toolchain, "gopls", Health::Warn, "go install golang.org/x/tools/gopls@latest");
        }
        ProjectKind::Node => {
            d.program(Section::Toolchain, "node", Health::Bad, "install Node.js");
            d.program(Section::Toolchain, "typescript-language-server", Health::Warn, "npm install -g typescript-language-server typescript");
        }
        ProjectKind::Other => {}
    }
    project(&mut d, tools);
    d.checks
}

fn python(d: &mut Doctor, tools: Option<&ProjectTools>) {
    let root = d.root;
    let uv_project = root.join("uv.lock").is_file() || std::fs::read_to_string(root.join("pyproject.toml")).is_ok_and(|t| t.contains("[tool.uv"));
    let venv = root.join(".venv");
    let venv_python = if cfg!(windows) { venv.join("Scripts").join("python.exe") } else { venv.join("bin").join("python") };
    if uv_project {
        let uv = d.program(Section::Toolchain, "uv", Health::Bad, "install it from https://docs.astral.sh/uv");
        if !venv_python.is_file() {
            d.push(Check::new(Section::Toolchain, ".venv", Health::Warn, "not created yet").run("uv sync", "uv", &["sync"], true));
        } else if let (true, Some(uv)) = (d.deep, uv) {
            match d.probe.run(&uv, &["sync", "--dry-run"], root) {
                Some((true, out)) if out.contains("Would make no changes") => d.push(Check::new(Section::Toolchain, ".venv", Health::Ok, "in sync with uv.lock")),
                Some((_, out)) => {
                    let changes = out.lines().filter(|l| l.trim_start().starts_with(['+', '-'])).count();
                    d.push(Check::new(Section::Toolchain, ".venv", Health::Warn, format!("behind uv.lock -- {changes} package change{}", if changes == 1 { "" } else { "s" })).run("uv sync", "uv", &["sync"], true));
                }
                None => d.push(Check::new(Section::Toolchain, ".venv", Health::Info, "uv couldn't check it")),
            }
        } else {
            d.push(Check::new(Section::Toolchain, ".venv", Health::Ok, "present"));
        }
    } else {
        d.program(Section::Toolchain, if cfg!(windows) { "python" } else { "python3" }, Health::Bad, "install Python from https://python.org");
    }
    if venv_python.is_file() && d.deep {
        if let Some(version) = d.probe.run(&venv_python, &["--version"], root).and_then(|(_, out)| version_in(&out)) {
            d.push(Check::new(Section::Toolchain, "python", Health::Ok, format!("{version} · .venv")));
        }
    }
    // The language server Fenix starts: the project's own, else pyright.
    let server = tools.and_then(|t| t.lsp.get("python")).map(|c| c.executable.clone()).unwrap_or_else(|| "pyright-langserver".to_string());
    if d.probe.locate(&server).is_some() || Path::new(&server).is_absolute() && Path::new(&server).is_file() {
        d.push(Check::new(Section::Toolchain, server, Health::Ok, "language server"));
    } else {
        let check = Check::new(Section::Toolchain, &server, Health::Bad, "not found -- no completion or errors as you type");
        d.push(if server == "pyright-langserver" { check.run("uv tool install pyright", "uv", &["tool", "install", "pyright"], false) } else { check });
    }
    if venv.is_dir() {
        let has_debugpy = [venv.join("Lib").join("site-packages").join("debugpy")]
            .into_iter()
            .chain(std::fs::read_dir(venv.join("lib")).into_iter().flatten().flatten().map(|e| e.path().join("site-packages").join("debugpy")))
            .any(|p| p.is_dir());
        let check = Check::new(Section::Toolchain, "debugpy", if has_debugpy { Health::Ok } else { Health::Info }, if has_debugpy { "in .venv" } else { "not installed -- needed to debug" });
        d.push(if !has_debugpy && uv_project { check.run("uv add --dev debugpy", "uv", &["add", "--dev", "debugpy"], false) } else { check });
    }
}

fn arduino(d: &mut Doctor) {
    let root = d.root;
    let cli = d.program(Section::Toolchain, "arduino-cli", Health::Bad, "install Arduino CLI from https://arduino.github.io/arduino-cli");
    d.program(Section::Toolchain, "arduino-language-server", Health::Warn, "no completion -- see the Arduino page of the wiki");
    d.program(Section::Toolchain, "clangd", Health::Warn, "no completion -- use the one from an Arduino IDE 2 install");
    let yaml = std::fs::read_to_string(root.join("sketch.yaml")).unwrap_or_default();
    let value = |key: &str| yaml.lines().find_map(|l| l.trim().strip_prefix(key)?.trim().strip_prefix(':').map(|v| v.trim().to_string())).filter(|v| !v.is_empty());
    let fqbn = value("default_fqbn");
    match &fqbn {
        Some(fqbn) => d.push(Check::new(Section::Hardware, "board", Health::Ok, fqbn.clone())),
        None => d.push(Check::new(Section::Hardware, "board", Health::Warn, "none chosen -- builds need one").fix("pick a board", FixAction::Editor(EditorFix::PickBoard), false)),
    }
    let port = value("default_port");
    if let (true, Some(cli)) = (d.deep, &cli) {
        if let Some(fqbn) = &fqbn {
            let core: String = fqbn.split(':').take(2).collect::<Vec<_>>().join(":");
            let installed = d.probe.run(cli, &["core", "list", "--format", "json"], root).and_then(|(_, out)| serde_json::from_str::<serde_json::Value>(&out).ok()).map(|json| {
                json["platforms"].as_array().into_iter().flatten().find(|p| p["id"] == core.as_str()).and_then(|p| p["installed_version"].as_str().map(str::to_string))
            });
            match installed {
                Some(Some(version)) => d.push(Check::new(Section::Toolchain, format!("core {core}"), Health::Ok, version)),
                Some(None) => d.push(Check::new(Section::Toolchain, format!("core {core}"), Health::Bad, "not installed -- builds will fail").run(&format!("arduino-cli core install {core}"), "arduino-cli", &["core", "install", &core], false)),
                None => {}
            }
        }
    }
    match &port {
        None => d.push(Check::new(Section::Hardware, "port", Health::Warn, "none chosen -- uploads need one").fix("pick a port", FixAction::Editor(EditorFix::PickPort), false)),
        Some(port) if d.deep && cli.is_some() => {
            let connected = d
                .probe
                .run(cli.as_ref().unwrap(), &["board", "list", "--format", "json"], root)
                .and_then(|(_, out)| serde_json::from_str::<serde_json::Value>(&out).ok())
                .map(|json| json["detected_ports"].as_array().into_iter().flatten().any(|p| p["port"]["address"] == port.as_str()));
            match connected {
                Some(true) => d.push(Check::new(Section::Hardware, format!("board on {port}"), Health::Ok, "connected")),
                Some(false) => d.push(Check::new(Section::Hardware, format!("board on {port}"), Health::Warn, "not connected").fix("pick a port", FixAction::Editor(EditorFix::PickPort), false)),
                None => d.push(Check::new(Section::Hardware, "port", Health::Ok, port.clone())),
            }
        }
        Some(port) => d.push(Check::new(Section::Hardware, "port", Health::Ok, port.clone())),
    }
}

fn mib(d: &mut Doctor) {
    let root = d.root;
    let dir = if root.join("mib").is_dir() { root.join("mib") } else { root.to_path_buf() };
    let known: Vec<&str> = fenix_mib::schema::all_tables().collect();
    let mut tables = 0;
    let mut rows = 0;
    let mut problems = Vec::new();
    let mut unknown = Vec::new();
    let mut entries: Vec<PathBuf> = std::fs::read_dir(&dir).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("dat"))).collect();
    entries.sort();
    for path in entries {
        let table = path.file_stem().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
        let Some(columns) = fenix_mib::schema::columns(&table) else {
            unknown.push(table);
            continue;
        };
        if !known.contains(&table.as_str()) {
            continue;
        }
        tables += 1;
        let Ok(text) = std::fs::read_to_string(&path) else {
            problems.push(Check::new(Section::Database, format!("{table}.dat"), Health::Bad, "can't be read as text").at(path.clone(), 1));
            continue;
        };
        for (i, line) in text.lines().enumerate().filter(|(_, l)| !l.is_empty()) {
            rows += 1;
            let fields = line.split('\t').count();
            if fields > columns.len() {
                problems.push(Check::new(Section::Database, format!("{table}.dat:{}", i + 1), Health::Warn, format!("{fields} fields, the ICD has {}", columns.len())).at(path.clone(), i + 1));
            }
        }
    }
    let health = if tables == 0 { Health::Warn } else { Health::Ok };
    d.push(Check::new(Section::Database, "tables", health, format!("{tables} table{}, {rows} row{}", if tables == 1 { "" } else { "s" }, if rows == 1 { "" } else { "s" })));
    let shown = problems.len().min(20);
    let more = problems.len() - shown;
    d.checks.extend(problems.into_iter().take(shown));
    if more > 0 {
        d.push(Check::new(Section::Database, "and more", Health::Warn, format!("{more} more rows with too many fields")));
    }
    if !unknown.is_empty() {
        d.push(Check::new(Section::Database, "not ICD 7.2", Health::Info, format!("{}.dat -- ignored", unknown.join(".dat, "))));
    }
    if d.probe.mib_registered(&dir) {
        d.push(Check::new(Section::Database, "registered", Health::Ok, "a [mib] root -- SPC m t finds it"));
    } else {
        let label = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        d.push(Check::new(Section::Database, "registered", Health::Warn, "not a [mib] root -- SPC m t won't see it").fix("register it", FixAction::Editor(EditorFix::RegisterMibRoot { path: dir, label }), true));
    }
}

fn project(d: &mut Doctor, tools: Result<ProjectTools, String>) {
    let root = d.root;
    let tools_path = root.join(".fenix").join("tools.json");
    if tools_path.is_file() {
        match tools {
            Ok(tools) => {
                let n = tools.tasks.len();
                d.push(Check::new(Section::Project, "tools.json", Health::Ok, format!("valid · {n} task{}", if n == 1 { "" } else { "s" })).at(tools_path, 1));
            }
            Err(e) => d.push(Check::new(Section::Project, "tools.json", Health::Bad, e).at(tools_path, 1)),
        }
    }
    if root.join(".git").exists() {
        let branch = crate::vcs::git_branch(root).unwrap_or_else(|| "?".to_string());
        d.push(Check::new(Section::Project, "git", Health::Ok, branch));
    } else {
        d.push(Check::new(Section::Project, "git", Health::Info, "not a repository").run("git init", "git", &["init"], true));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::TempDir;
    use std::collections::HashMap;

    /// Programs it "has", and what running each prints.
    #[derive(Default)]
    struct FakeProbe {
        programs: Vec<&'static str>,
        outputs: HashMap<String, (bool, String)>,
        mib_roots: Vec<PathBuf>,
    }

    impl Probe for FakeProbe {
        fn locate(&self, program: &str) -> Option<PathBuf> {
            self.programs.contains(&program).then(|| PathBuf::from(format!("/bin/{program}")))
        }
        fn run(&self, program: &Path, args: &[&str], _dir: &Path) -> Option<(bool, String)> {
            let name = program.file_name()?.to_string_lossy().into_owned();
            self.outputs.get(&format!("{name} {}", args.join(" "))).cloned().or_else(|| args.first().filter(|a| ["--version", "version"].contains(*a)).map(|_| (true, format!("{name} 1.2.3"))))
        }
        fn mib_registered(&self, dir: &Path) -> bool {
            self.mib_roots.iter().any(|r| r == dir)
        }
    }

    fn find<'a>(checks: &'a [Check], label: &str) -> &'a Check {
        checks.iter().find(|c| c.label == label).unwrap_or_else(|| panic!("no {label} check in {:?}", checks.iter().map(|c| &c.label).collect::<Vec<_>>()))
    }

    #[test]
    fn a_uv_project_without_a_venv_or_pyright_gets_both_fixes() {
        let dir = TempDir::new("doctor_py");
        dir.write("pyproject.toml", "[project]\nname='x'\n");
        dir.write("uv.lock", "");
        let probe = FakeProbe { programs: vec!["uv"], ..Default::default() };
        let checks = diagnose(dir.path(), ProjectKind::Python, &probe, true);
        assert_eq!(find(&checks, "uv").detail, "1.2.3");
        let venv = find(&checks, ".venv");
        assert_eq!(venv.health, Health::Warn);
        assert!(venv.fix.as_ref().is_some_and(|f| f.safe && f.label == "uv sync"));
        let lsp = find(&checks, "pyright-langserver");
        assert_eq!(lsp.health, Health::Bad);
        assert!(lsp.fix.as_ref().is_some_and(|f| !f.safe), "installing is never safe");
        assert_eq!(find(&checks, "git").health, Health::Info);
        assert_eq!(worst(&checks), Health::Bad);
    }

    #[test]
    fn a_venv_behind_its_lock_is_reported_from_uv_itself() {
        let dir = TempDir::new("doctor_venv");
        dir.write("uv.lock", "");
        let python = if cfg!(windows) { ".venv/Scripts/python.exe" } else { ".venv/bin/python" };
        dir.write(python, "");
        let mut probe = FakeProbe { programs: vec!["uv", "pyright-langserver"], ..Default::default() };
        probe.outputs.insert("uv sync --dry-run".into(), (true, "Resolved 3\n + rich==15.0\n - old==1\n".into()));
        let checks = diagnose(dir.path(), ProjectKind::Python, &probe, true);
        assert_eq!(find(&checks, ".venv").detail, "behind uv.lock -- 2 package changes");
        probe.outputs.insert("uv sync --dry-run".into(), (true, "Would make no changes\n".into()));
        let checks = diagnose(dir.path(), ProjectKind::Python, &probe, true);
        assert_eq!(find(&checks, ".venv").health, Health::Ok);
        // A quick pass runs nothing.
        let quick = diagnose(dir.path(), ProjectKind::Python, &probe, false);
        assert_eq!(find(&quick, ".venv").detail, "present");
    }

    #[test]
    fn a_sketch_reports_its_board_core_and_port() {
        let dir = TempDir::new("doctor_ino");
        dir.write("Lab/Lab.ino", "");
        dir.write("Lab/sketch.yaml", "default_fqbn: arduino:avr:uno\ndefault_port: COM4\n");
        let root = dir.path().join("Lab");
        let mut probe = FakeProbe { programs: vec!["arduino-cli"], ..Default::default() };
        probe.outputs.insert("arduino-cli core list --format json".into(), (true, r#"{"platforms":[{"id":"esp32:esp32","installed_version":"3.0"}]}"#.into()));
        probe.outputs.insert("arduino-cli board list --format json".into(), (true, r#"{"detected_ports":[{"port":{"address":"COM1"}}]}"#.into()));
        let checks = diagnose(&root, ProjectKind::Arduino, &probe, true);
        assert_eq!(find(&checks, "board").detail, "arduino:avr:uno");
        assert_eq!(find(&checks, "arduino-cli").detail, "1.2.3", "asked with `version`, not `--version`");
        let core = find(&checks, "core arduino:avr");
        assert_eq!(core.health, Health::Bad);
        assert!(matches!(&core.fix.as_ref().unwrap().action, FixAction::Run(step) if step.display() == "arduino-cli core install arduino:avr"));
        let port = find(&checks, "board on COM4");
        assert_eq!((port.health, port.detail.as_str()), (Health::Warn, "not connected"));
        assert!(matches!(port.fix.as_ref().unwrap().action, FixAction::Editor(EditorFix::PickPort)));
        assert_eq!(find(&checks, "clangd").health, Health::Warn);
    }

    #[test]
    fn a_sketch_with_no_board_offers_to_pick_one() {
        let dir = TempDir::new("doctor_ino_bare");
        dir.write("Lab/Lab.ino", "");
        let checks = diagnose(&dir.path().join("Lab"), ProjectKind::Arduino, &FakeProbe::default(), false);
        assert!(matches!(find(&checks, "board").fix.as_ref().unwrap().action, FixAction::Editor(EditorFix::PickBoard)));
        assert!(matches!(find(&checks, "port").fix.as_ref().unwrap().action, FixAction::Editor(EditorFix::PickPort)));
    }

    #[test]
    fn a_mib_is_counted_its_bad_rows_located_and_its_registration_checked() {
        let dir = TempDir::new("doctor_mib");
        dir.write("mib/ccf.dat", "TC1\tFirst\n\nTC2\tSecond\n");
        let too_many = vec!["x"; 30].join("\t");
        dir.write("mib/pcf.dat", &format!("P1\tok\n{too_many}\n"));
        dir.write("mib/zzz.dat", "");
        let probe = FakeProbe::default();
        let checks = diagnose(dir.path(), ProjectKind::Mib, &probe, false);
        assert_eq!(find(&checks, "tables").detail, "2 tables, 4 rows");
        let bad = find(&checks, "pcf.dat:2");
        assert_eq!(bad.location, Some((dir.path().join("mib/pcf.dat"), 2)));
        assert_eq!(find(&checks, "not ICD 7.2").detail, "zzz.dat -- ignored");
        let registered = find(&checks, "registered");
        assert!(matches!(&registered.fix.as_ref().unwrap().action, FixAction::Editor(EditorFix::RegisterMibRoot { path, .. }) if path == &dir.path().join("mib")));
        let probe = FakeProbe { mib_roots: vec![dir.path().join("mib")], ..Default::default() };
        assert_eq!(find(&diagnose(dir.path(), ProjectKind::Mib, &probe, false), "registered").health, Health::Ok);
    }

    #[test]
    fn a_broken_tools_json_is_bad_and_points_at_the_file() {
        let dir = TempDir::new("doctor_tools");
        dir.write(".fenix/tools.json", "{\"typo\": 1}");
        dir.write(".git/HEAD", "ref: refs/heads/main\n");
        let checks = diagnose(dir.path(), ProjectKind::Other, &FakeProbe::default(), false);
        let tools = find(&checks, "tools.json");
        assert_eq!(tools.health, Health::Bad);
        assert_eq!(tools.location.as_ref().map(|(p, _)| p.ends_with("tools.json")), Some(true));
        assert_eq!(find(&checks, "git").detail, "main");
    }

    #[test]
    fn versions_are_picked_out_of_whatever_a_tool_prints() {
        assert_eq!(version_in("uv 0.12.5 (210d1f678 2026-08-14)").as_deref(), Some("0.12.5"));
        assert_eq!(version_in("arduino-cli  Version: 1.5.1 Commit: x").as_deref(), Some("1.5.1"));
        assert_eq!(version_in("Python 3.12.14").as_deref(), Some("3.12.14"));
        assert_eq!(version_in("rust-analyzer v0.3.2"), Some("0.3.2".into()));
        assert_eq!(version_in("nothing here"), None);
    }
}
