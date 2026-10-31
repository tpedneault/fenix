use std::path::Path;

/// One runnable task -- a name for the picker/modeline plus a program
/// and its arguments, exactly the same "no shell, split ahead of time"
/// shape `fenix_lsp`'s own server commands already use (see `fenix-gui`'s
/// `lsp::resolve_server_command`'s own doc comment for why: every
/// command this actually needs to run takes plain flag-only arguments,
/// so real shell quoting was never worth the complexity).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskDef {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
}

impl TaskDef {
    fn new(name: &str, command: &str, args: &[&str]) -> Self {
        Self { name: name.to_string(), command: command.to_string(), args: args.iter().map(|s| s.to_string()).collect() }
    }
}

/// The built-in task list for whichever project-marker files exist
/// directly under `root` -- every ecosystem present contributes its own
/// tasks (a project that's both a Cargo workspace and has a `package.json`
/// frontend gets both sets), so nothing here is mutually exclusive.
/// `cargo build`/`test`/`clippy` ask for `--message-format=json`
/// specifically -- see `output::parse_line`'s own doc comment for why
/// that's the one build tool here whose plain human output can't be
/// parsed line-by-line at all (a diagnostic's location sits on a
/// different line than its message).
pub fn default_tasks(root: &Path) -> Vec<TaskDef> {
    let mut tasks = Vec::new();
    if root.join("Cargo.toml").is_file() {
        tasks.push(TaskDef::new("cargo build", "cargo", &["build", "--message-format=json"]));
        tasks.push(TaskDef::new("cargo test", "cargo", &["test", "--message-format=json"]));
        tasks.push(TaskDef::new("cargo clippy", "cargo", &["clippy", "--message-format=json"]));
    }
    if root.join("pyproject.toml").is_file() || root.join("setup.py").is_file() {
        tasks.push(TaskDef::new("pytest", "pytest", &[]));
        tasks.push(TaskDef::new("ruff check", "ruff", &["check"]));
    }
    if root.join("CMakeLists.txt").is_file() {
        tasks.push(TaskDef::new("cmake configure", "cmake", &["-B", "build"]));
        tasks.push(TaskDef::new("cmake build", "cmake", &["--build", "build"]));
        tasks.push(TaskDef::new("ctest", "ctest", &["--test-dir", "build"]));
    }
    if root.join("package.json").is_file() {
        tasks.push(TaskDef::new("npm run build", "npm", &["run", "build"]));
        tasks.push(TaskDef::new("npm test", "npm", &["test"]));
    }
    tasks
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
            let dir = std::env::temp_dir().join(format!("fenix-tasks-test-{name}-{}-{n}", std::process::id()));
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
    fn a_directory_with_no_markers_at_all_offers_no_tasks() {
        let dir = TempDir::new("no_markers");
        assert!(default_tasks(dir.path()).is_empty());
    }

    #[test]
    fn a_cargo_toml_offers_build_test_and_clippy_with_json_output() {
        let dir = TempDir::new("cargo");
        std::fs::write(dir.path().join("Cargo.toml"), b"[package]\nname=\"x\"").unwrap();
        let tasks = default_tasks(dir.path());
        let names: Vec<&str> = tasks.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["cargo build", "cargo test", "cargo clippy"]);
        assert!(tasks[0].args.contains(&"--message-format=json".to_string()));
    }

    #[test]
    fn a_pyproject_toml_offers_pytest_and_ruff_check() {
        let dir = TempDir::new("python");
        std::fs::write(dir.path().join("pyproject.toml"), b"[project]\nname=\"x\"").unwrap();
        let tasks = default_tasks(dir.path());
        let names: Vec<&str> = tasks.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["pytest", "ruff check"]);
    }

    #[test]
    fn a_cmakelists_txt_offers_configure_build_and_ctest() {
        let dir = TempDir::new("cmake");
        std::fs::write(dir.path().join("CMakeLists.txt"), b"project(x)").unwrap();
        let tasks = default_tasks(dir.path());
        let names: Vec<&str> = tasks.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["cmake configure", "cmake build", "ctest"]);
    }

    #[test]
    fn a_package_json_offers_npm_build_and_test() {
        let dir = TempDir::new("npm");
        std::fs::write(dir.path().join("package.json"), b"{}").unwrap();
        let tasks = default_tasks(dir.path());
        let names: Vec<&str> = tasks.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["npm run build", "npm test"]);
    }

    #[test]
    fn a_project_with_multiple_markers_gets_every_matching_ecosystems_tasks() {
        let dir = TempDir::new("mixed");
        std::fs::write(dir.path().join("Cargo.toml"), b"[package]\nname=\"x\"").unwrap();
        std::fs::write(dir.path().join("package.json"), b"{}").unwrap();
        let tasks = default_tasks(dir.path());
        let names: Vec<&str> = tasks.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["cargo build", "cargo test", "cargo clippy", "npm run build", "npm test"]);
    }
}
