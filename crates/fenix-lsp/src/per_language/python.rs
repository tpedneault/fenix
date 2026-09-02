//! Resolves which Python interpreter a language server should be told
//! to use for a given project root -- neither pyright nor ruff can
//! infer this on their own, and getting it wrong means "go to
//! definition" landing nowhere and every third-party import showing as
//! unresolved. Checked in priority order: uv, Poetry, a bare venv
//! directory, falling back to whatever's on `PATH`. Each check degrades
//! to the next rather than failing outright -- this always returns
//! *some* interpreter, matching this project's established "never block
//! startup, degrade gracefully" posture (see e.g. `fenix_vnc::coords`'s
//! client-side-scaling fallback when a VNC server won't resize).

use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PythonEnvKind {
    /// `pyproject.toml` + `uv.lock` (or an explicit `[tool.uv]` table)
    /// -- uv's own convention puts the virtualenv at `<root>/.venv`.
    Uv,
    /// `[tool.poetry]` in `pyproject.toml` -- the venv's actual path
    /// isn't guessable (Poetry hashes it into a cache directory unless
    /// `virtualenvs.in-project` is configured), so it's resolved by
    /// asking Poetry itself.
    Poetry,
    /// A bare `.venv`/`venv` directory with no Poetry/uv markers.
    Venv,
    /// No project-local virtualenv found at all.
    System,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PythonEnvironment {
    pub interpreter: PathBuf,
    pub kind: PythonEnvKind,
}

pub fn resolve(project_root: &Path) -> PythonEnvironment {
    let pyproject_text = std::fs::read_to_string(project_root.join("pyproject.toml")).unwrap_or_default();

    if pyproject_text.contains("[tool.uv]") || project_root.join("uv.lock").is_file() {
        if let Some(interpreter) = venv_interpreter_path(&project_root.join(".venv")) {
            return PythonEnvironment { interpreter, kind: PythonEnvKind::Uv };
        }
    }

    if pyproject_text.contains("[tool.poetry]") {
        if let Some(interpreter) = poetry_env_interpreter(project_root) {
            return PythonEnvironment { interpreter, kind: PythonEnvKind::Poetry };
        }
    }

    for dir_name in [".venv", "venv"] {
        if let Some(interpreter) = venv_interpreter_path(&project_root.join(dir_name)) {
            return PythonEnvironment { interpreter, kind: PythonEnvKind::Venv };
        }
    }

    PythonEnvironment { interpreter: system_python(), kind: PythonEnvKind::System }
}

/// The interpreter executable inside a venv directory, or `None` if
/// `venv_dir` doesn't actually contain one -- covers both layouts a
/// venv can have (`Scripts\python.exe` on Windows, `bin/python`
/// elsewhere), so a stale or half-created directory (no such file
/// inside it yet) correctly falls through to the next check rather than
/// being treated as a real environment.
fn venv_interpreter_path(venv_dir: &Path) -> Option<PathBuf> {
    let candidate = if cfg!(windows) { venv_dir.join("Scripts").join("python.exe") } else { venv_dir.join("bin").join("python") };
    candidate.is_file().then_some(candidate)
}

/// Asks Poetry itself where a project's virtualenv lives. One short-
/// lived process call, same "run it, capture stdout, done" shape as
/// `fenix_git`/`fenix_docker`'s own one-shot commands -- `None` on any
/// failure (Poetry not installed, no env created yet, ...), which the
/// caller treats as "fall through to the next check," not an error.
fn poetry_env_interpreter(project_root: &Path) -> Option<PathBuf> {
    let output = no_console_command("poetry").args(["env", "info", "--path"]).current_dir(project_root).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let venv_path = String::from_utf8(output.stdout).ok()?;
    venv_interpreter_path(Path::new(venv_path.trim()))
}

/// Whatever `python3`/`python` resolves to on `PATH` -- checked by
/// actually running `--version` rather than just assuming a name is
/// present, since which one (if either) exists varies by platform and
/// installation. Falls back to the bare name `python3` even if neither
/// answered, rather than `Option::None`: every caller needs *some*
/// command to hand the language server, and a nonexistent command is
/// no worse a starting point than no command at all.
fn system_python() -> PathBuf {
    for name in ["python3", "python"] {
        if no_console_command(name).arg("--version").output().is_ok_and(|out| out.status.success()) {
            return PathBuf::from(name);
        }
    }
    PathBuf::from("python3")
}

/// `Command::new`, plus (on Windows) the same "don't flash a console
/// window" flag `fenix-git`'s own process helper uses -- `fenix-gui` has
/// no console of its own to inherit, and these calls are frequent
/// enough (once per project-root resolution) for the flash to be
/// noticeable otherwise.
fn no_console_command(program: &str) -> Command {
    let mut cmd = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct TempDir(PathBuf);
    impl TempDir {
        fn new(name: &str) -> Self {
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!("fenix-lsp-test-{name}-{}-{n}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn fake_venv(root: &Path, dir_name: &str) {
        let venv = root.join(dir_name);
        let interpreter = if cfg!(windows) { venv.join("Scripts") } else { venv.join("bin") };
        std::fs::create_dir_all(&interpreter).unwrap();
        let exe_name = if cfg!(windows) { "python.exe" } else { "python" };
        std::fs::write(interpreter.join(exe_name), b"").unwrap();
    }

    #[test]
    fn no_markers_at_all_falls_back_to_system_python() {
        let dir = TempDir::new("no_markers");
        let env = resolve(dir.path());
        assert_eq!(env.kind, PythonEnvKind::System);
    }

    #[test]
    fn a_bare_venv_directory_is_detected_without_any_pyproject_toml() {
        let dir = TempDir::new("bare_venv");
        fake_venv(dir.path(), ".venv");
        let env = resolve(dir.path());
        assert_eq!(env.kind, PythonEnvKind::Venv);
        assert!(env.interpreter.starts_with(dir.path().join(".venv")));
    }

    #[test]
    fn a_bare_venv_named_venv_without_the_dot_is_also_detected() {
        let dir = TempDir::new("bare_venv_no_dot");
        fake_venv(dir.path(), "venv");
        let env = resolve(dir.path());
        assert_eq!(env.kind, PythonEnvKind::Venv);
        assert!(env.interpreter.starts_with(dir.path().join("venv")));
    }

    #[test]
    fn uv_lock_plus_a_real_dot_venv_resolves_as_uv() {
        let dir = TempDir::new("uv_lock");
        std::fs::write(dir.path().join("uv.lock"), b"").unwrap();
        fake_venv(dir.path(), ".venv");
        let env = resolve(dir.path());
        assert_eq!(env.kind, PythonEnvKind::Uv);
    }

    #[test]
    fn an_explicit_tool_uv_table_resolves_as_uv_even_without_a_lockfile() {
        let dir = TempDir::new("tool_uv_table");
        std::fs::write(dir.path().join("pyproject.toml"), b"[project]\nname = \"x\"\n[tool.uv]\ndev-dependencies = []\n").unwrap();
        fake_venv(dir.path(), ".venv");
        let env = resolve(dir.path());
        assert_eq!(env.kind, PythonEnvKind::Uv);
    }

    #[test]
    fn a_tool_uv_marker_with_no_venv_created_yet_falls_through_to_system() {
        let dir = TempDir::new("uv_no_venv");
        std::fs::write(dir.path().join("uv.lock"), b"").unwrap();
        // No .venv directory created -- `uv sync` just hasn't run yet.
        let env = resolve(dir.path());
        assert_eq!(env.kind, PythonEnvKind::System);
    }

    #[test]
    fn a_tool_poetry_marker_with_poetry_unavailable_falls_through_past_it() {
        // Neither asserts poetry IS installed nor that it isn't --
        // whichever is true on the machine running this test, the
        // function must never panic and must land on *some* real
        // environment (Poetry success, or a graceful fall-through).
        let dir = TempDir::new("tool_poetry");
        std::fs::write(dir.path().join("pyproject.toml"), b"[tool.poetry]\nname = \"x\"\n").unwrap();
        let env = resolve(dir.path());
        assert!(matches!(env.kind, PythonEnvKind::Poetry | PythonEnvKind::System));
    }

    #[test]
    fn uv_is_checked_before_poetry_when_both_markers_are_present() {
        let dir = TempDir::new("both_markers");
        std::fs::write(dir.path().join("pyproject.toml"), b"[tool.poetry]\nname = \"x\"\n[tool.uv]\n").unwrap();
        fake_venv(dir.path(), ".venv");
        let env = resolve(dir.path());
        assert_eq!(env.kind, PythonEnvKind::Uv);
    }
}
