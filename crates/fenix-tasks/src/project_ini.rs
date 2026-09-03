//! `.fenix/project.ini`'s `[tasks]` section -- extra tasks a project
//! wants to define for itself beyond `defs::default_tasks`'s built-ins,
//! genuinely new territory (no `.fenix/` directory precedent existed
//! anywhere in this app before this). A small, purpose-built reader
//! rather than a dependency on `fenix-config`'s own (crate-private)
//! `ini` module -- there's only ever this one section to read here, so a
//! general-purpose INI parser would be unused machinery for what's just
//! `taskN = NAME|COMMAND` lines under one `[tasks]` header, the same
//! numbered-key convention `fenix_config::Config`'s own `[lsp]`/`[mib]`/
//! `[vnc]` sections already established. Hand-edited only -- nothing in
//! this app writes `.fenix/project.ini` itself (yet; see the plan's own
//! note that later milestones extend this same file with `[cmake]`/
//! `[launch]` sections), so there's no `save` here, just `load`.

use crate::defs::TaskDef;
use std::path::Path;

/// Reads `root/.fenix/project.ini`'s `[tasks]` section into an ordered
/// list of `TaskDef`s -- `taskN = NAME|COMMAND_LINE`, `COMMAND_LINE`
/// split on whitespace the same no-shell-quoting way `[lsp]`'s server
/// commands already are. A missing file, a missing `[tasks]` section, or
/// any individual malformed line all degrade to being skipped rather
/// than failing the whole read -- same "a bad entry loses only itself"
/// posture `fenix_config::Config::load` already established for every
/// hand-authored list it reads.
pub fn project_tasks(root: &Path) -> Vec<TaskDef> {
    let Ok(contents) = std::fs::read_to_string(root.join(".fenix").join("project.ini")) else {
        return Vec::new();
    };
    let mut entries: Vec<(usize, TaskDef)> = Vec::new();
    let mut in_tasks_section = false;
    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with(['#', ';']) {
            continue;
        }
        if let Some(header) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            in_tasks_section = header.trim() == "tasks";
            continue;
        }
        if !in_tasks_section {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else { continue };
        let key = key.trim();
        let Some(n) = key.strip_prefix("task").and_then(|s| s.parse::<usize>().ok()) else { continue };
        let Some((name, command_line)) = value.trim().split_once('|') else { continue };
        let mut parts = command_line.trim().split_whitespace();
        let Some(command) = parts.next() else { continue };
        let args: Vec<String> = parts.map(str::to_string).collect();
        entries.push((n, TaskDef { name: name.trim().to_string(), command: command.to_string(), args }));
    }
    entries.sort_by_key(|(n, _)| *n);
    entries.into_iter().map(|(_, task)| task).collect()
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
            let dir = std::env::temp_dir().join(format!("fenix-tasks-ini-test-{name}-{}-{n}", std::process::id()));
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
    fn a_missing_fenix_directory_yields_no_tasks_not_an_error() {
        let dir = TempDir::new("missing");
        assert!(project_tasks(dir.path()).is_empty());
    }

    #[test]
    fn reads_tasks_in_ordinal_order_not_key_string_order() {
        let dir = TempDir::new("ordinal");
        dir.write_project_ini("[tasks]\ntask2 = Second|echo two\ntask10 = Tenth|echo ten\ntask1 = First|echo one\n");
        let tasks = project_tasks(dir.path());
        let names: Vec<&str> = tasks.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["First", "Second", "Tenth"]);
    }

    #[test]
    fn parses_the_command_and_its_arguments() {
        let dir = TempDir::new("command_args");
        dir.write_project_ini("[tasks]\ntask1 = Format|cargo fmt --all\n");
        let tasks = project_tasks(dir.path());
        assert_eq!(tasks, vec![TaskDef { name: "Format".to_string(), command: "cargo".to_string(), args: vec!["fmt".to_string(), "--all".to_string()] }]);
    }

    #[test]
    fn ignores_sections_other_than_tasks() {
        let dir = TempDir::new("other_section");
        dir.write_project_ini("[cmake]\nbuild_dir = out\n\n[tasks]\ntask1 = Only|echo hi\n");
        let tasks = project_tasks(dir.path());
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].name, "Only");
    }

    #[test]
    fn a_malformed_entry_is_skipped_without_losing_the_rest() {
        let dir = TempDir::new("malformed");
        dir.write_project_ini("[tasks]\ntask1 = Good|echo good\ntask2 = no-pipe-here\ntask3 = AlsoGood|echo also\n");
        let tasks = project_tasks(dir.path());
        let names: Vec<&str> = tasks.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["Good", "AlsoGood"]);
    }
}
