//! `.fenix/project.ini`'s `[launch]` section -- what program a debug
//! session should run, extending the same per-project config file
//! `fenix-tasks`' own `[tasks]` section already introduced (see its
//! `project_ini` module's doc comment for why a small purpose-built
//! reader, not a dependency on `fenix-config`'s crate-private `ini`
//! module). Optional: absent, or with no `program` key, `fenix-gui`
//! falls back to debugging whatever buffer is focused -- the common
//! "debug the script I have open" case for an interpreted language,
//! which needs no configuration at all. A compiled target (a native
//! binary to `launch`/`attach` to) has no such fallback and genuinely
//! needs this section.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LaunchConfig {
    /// Resolved against `root` already if the configured value was
    /// relative -- a caller never needs to join it again.
    pub program: Option<PathBuf>,
    pub args: Vec<String>,
}

/// Reads `root/.fenix/project.ini`'s `[launch]` section. A missing
/// file, a missing `[launch]` section, or an unrecognized key are all
/// silently ignored -- same "absent means use the fallback" posture
/// `LaunchConfig::default()` already gives a caller with nothing
/// configured.
pub fn read_launch_config(root: &Path) -> LaunchConfig {
    let Ok(contents) = std::fs::read_to_string(root.join(".fenix").join("project.ini")) else {
        return LaunchConfig::default();
    };
    let mut config = LaunchConfig::default();
    let mut in_launch_section = false;
    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with(['#', ';']) {
            continue;
        }
        if let Some(header) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            in_launch_section = header.trim() == "launch";
            continue;
        }
        if !in_launch_section {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else { continue };
        let value = value.trim();
        match key.trim() {
            "program" => {
                let path = PathBuf::from(value);
                config.program = Some(if path.is_absolute() { path } else { root.join(path) });
            }
            "args" => config.args = value.split_whitespace().map(str::to_string).collect(),
            _ => {}
        }
    }
    config
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
            let dir = std::env::temp_dir().join(format!("fenix-dap-launch-test-{name}-{}-{n}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
        fn path(&self) -> &Path {
            &self.0
        }
        fn write_project_ini(&self, contents: &str) {
            let dot_fenix = self.path().join(".fenix");
            std::fs::create_dir_all(&dot_fenix).unwrap();
            std::fs::write(dot_fenix.join("project.ini"), contents).unwrap();
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_missing_fenix_directory_yields_the_default_config() {
        let dir = TempDir::new("missing");
        assert_eq!(read_launch_config(dir.path()), LaunchConfig::default());
    }

    #[test]
    fn a_relative_program_is_resolved_against_root() {
        let dir = TempDir::new("relative");
        dir.write_project_ini("[launch]\nprogram = target/debug/my-binary\n");
        let config = read_launch_config(dir.path());
        assert_eq!(config.program, Some(dir.path().join("target/debug/my-binary")));
    }

    #[test]
    fn an_absolute_program_is_kept_as_is() {
        let dir = TempDir::new("absolute");
        let abs = if cfg!(windows) { "C:\\tools\\my-binary.exe" } else { "/usr/local/bin/my-binary" };
        dir.write_project_ini(&format!("[launch]\nprogram = {abs}\n"));
        let config = read_launch_config(dir.path());
        assert_eq!(config.program, Some(PathBuf::from(abs)));
    }

    #[test]
    fn args_are_split_on_whitespace() {
        let dir = TempDir::new("args");
        dir.write_project_ini("[launch]\nprogram = a.out\nargs = --verbose --count 3\n");
        let config = read_launch_config(dir.path());
        assert_eq!(config.args, vec!["--verbose".to_string(), "--count".to_string(), "3".to_string()]);
    }

    #[test]
    fn ignores_sections_other_than_launch() {
        let dir = TempDir::new("other_section");
        dir.write_project_ini("[tasks]\ntask1 = Build|cargo build\n\n[launch]\nprogram = a.out\n");
        let config = read_launch_config(dir.path());
        assert_eq!(config.program, Some(dir.path().join("a.out")));
    }
}
