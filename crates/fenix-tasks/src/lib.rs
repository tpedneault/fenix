//! The build/task runner: per-project task discovery (`defs`/
//! `project_ini`), one-shot process execution, and output parsing
//! (`output`) that feeds `fenix-gui`'s generalized quickfix list the
//! same way a project grep or an LSP references response already does.
//! This crate stays GUI/event-loop-agnostic (see `spawn`'s own doc
//! comment) -- the actual streaming/quickfix/panel wiring lives in
//! `fenix-gui`, mirroring the split `fenix-docker`'s own `spawn_log_
//! follower` already established between "here's a piped child" and
//! "here's what reads it on a background thread and feeds the UI."

mod defs;
mod output;
mod project_ini;

pub use defs::TaskDef;
pub use output::{parse_line, ParsedLine, TaskLocation};

use std::path::Path;
use std::process::{Child, Command, Stdio};

/// Every runnable task for `root` -- `defs::default_tasks`' built-ins
/// (whichever project markers are actually present) followed by
/// `project_ini::project_tasks`' hand-authored `.fenix/project.ini`
/// `[tasks]` entries. Appended, not merged/deduplicated by name: a
/// project-local task sharing a built-in's name (redefining `cargo
/// build` with extra flags, say) simply shows up as a second, separate
/// entry in the picker -- simpler than silent-override semantics, and
/// the two are still visually distinguishable by whichever one the
/// picker's fuzzy match actually favors.
pub fn discover_tasks(root: &Path) -> Vec<TaskDef> {
    let mut tasks = defs::default_tasks(root);
    tasks.extend(project_ini::project_tasks(root));
    tasks
}

/// Spawns `task` as a genuinely long-lived child (stdout/stderr both
/// piped, kept separate rather than combined -- the caller reads each on
/// its own thread), working directory `root`. Returns `None` (never
/// panics) if the binary itself can't be launched -- a missing `cargo`/
/// `pytest`/`npm`/whatever, the same "report it, don't crash" posture
/// `fenix_docker::logs::spawn_log_follower` already established. The
/// caller owns reading both pipes and eventually killing/waiting on the
/// child (this crate has no event loop of its own to drive that from --
/// see `fenix-gui`'s own task-runner session for the thread/kill
/// lifecycle).
pub fn spawn(task: &TaskDef, root: &Path) -> Option<Child> {
    Command::new(&task.command).args(&task.args).current_dir(root).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct TempDir(std::path::PathBuf);
    impl TempDir {
        fn new(name: &str) -> Self {
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!("fenix-tasks-lib-test-{name}-{}-{n}", std::process::id()));
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

    #[test]
    fn discover_tasks_combines_built_ins_with_project_ini_entries() {
        let dir = TempDir::new("combined");
        std::fs::write(dir.path().join("Cargo.toml"), b"[package]\nname=\"x\"").unwrap();
        std::fs::create_dir_all(dir.path().join(".fenix")).unwrap();
        std::fs::write(dir.path().join(".fenix").join("project.ini"), b"[tasks]\ntask1 = Format|cargo fmt --all\n").unwrap();

        let tasks = discover_tasks(dir.path());
        let names: Vec<&str> = tasks.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["cargo build", "cargo test", "cargo clippy", "Format"]);
    }

    #[test]
    fn discover_tasks_is_empty_for_a_directory_with_no_markers_or_overrides() {
        let dir = TempDir::new("empty");
        assert!(discover_tasks(dir.path()).is_empty());
    }

    #[test]
    fn spawn_never_panics_for_a_nonexistent_binary() {
        let dir = TempDir::new("spawn_missing");
        let task = TaskDef { name: "nope".to_string(), command: "definitely-not-a-real-binary-xyz".to_string(), args: Vec::new() };
        assert!(spawn(&task, dir.path()).is_none());
    }

    #[test]
    fn spawn_launches_a_real_command_with_its_working_directory_and_args() {
        let dir = TempDir::new("spawn_real");
        let program = if cfg!(windows) { "cmd" } else { "echo" };
        let args = if cfg!(windows) { vec!["/c".to_string(), "echo".to_string(), "hi".to_string()] } else { vec!["hi".to_string()] };
        let task = TaskDef { name: "echo".to_string(), command: program.to_string(), args };
        let mut child = spawn(&task, dir.path()).expect("echo/cmd should be launchable in any test environment");
        let status = child.wait().expect("child should exit");
        assert!(status.success());
    }
}
