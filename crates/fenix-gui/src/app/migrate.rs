//! Moves an installation from the old layout -- everything loose in
//! `%AppData%\fenix`, settings in `config.ini` -- to the new one (see
//! `fenix_storage::paths`). Runs at every launch and does nothing once
//! there's nothing left to move: each step only runs when its old file
//! is there and its new one isn't. Nothing is deleted; each old file
//! ends up in `backup`, and a step that fails leaves its file where it
//! was, for the next launch to try again.

use std::io;
use std::path::{Path, PathBuf};

use fenix_storage::paths::Roots;

/// What was moved, one line each, and what couldn't be.
#[derive(Debug, Default)]
pub(crate) struct Report {
    pub moved: Vec<String>,
    pub failed: Vec<String>,
}

/// Moves `from` to `to`, across drives if it has to.
fn relocate(from: &Path, to: &Path) -> io::Result<()> {
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if std::fs::rename(from, to).is_ok() {
        return Ok(());
    }
    copy_all(from, to)?;
    if from.is_dir() {
        std::fs::remove_dir_all(from)
    } else {
        std::fs::remove_file(from)
    }
}

fn copy_all(from: &Path, to: &Path) -> io::Result<()> {
    if from.is_dir() {
        std::fs::create_dir_all(to)?;
        for entry in std::fs::read_dir(from)? {
            let entry = entry?;
            copy_all(&entry.path(), &to.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        std::fs::copy(from, to).map(|_| ())
    }
}

/// Puts an old file in `backup`, beside any earlier one of the same name.
fn back_up(file: &Path, roots: &Roots) -> io::Result<PathBuf> {
    let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let mut to = roots.backup().join(&name);
    let mut n = 1;
    while to.exists() {
        n += 1;
        to = roots.backup().join(format!("{name}.{n}"));
    }
    relocate(file, &to)?;
    Ok(to)
}

pub(crate) fn run(legacy: &Path, roots: &Roots) -> Report {
    let mut report = Report::default();
    let mut step = |what: String, result: io::Result<()>| match result {
        Ok(()) => report.moved.push(what),
        Err(e) => report.failed.push(format!("{what}: {e}")),
    };
    let old = |name: &str| Some(legacy.join(name)).filter(|p| p.exists());

    // Settings: config.ini becomes settings.toml, tokens and all; its
    // windows and bookmarks go to state files.
    if let Some(ini) = old("config.ini") {
        if !roots.settings_file().exists() {
            let result = fenix_config::Config::migrate_ini(&ini, roots.settings_file(), roots.local.join("state")).and_then(|_| back_up(&ini, roots).map(|_| ()));
            step(format!("config.ini → {}", roots.settings_file().display()), result);
        }
    }

    // Known projects, and their groups and pins: one state file.
    let projects = roots.state("projects.json");
    if let Some(txt) = old("projects.txt") {
        if !projects.exists() || fenix_project::KnownProjects::load(projects.clone()).is_ok_and(|k| k.roots().is_empty()) {
            let result = fenix_project::read_path_list(&txt).and_then(|list| {
                let mut known = fenix_project::KnownProjects::load_or_default(projects.clone());
                for root in list.into_iter().rev() {
                    known.add(root);
                }
                known.save()?;
                back_up(&txt, roots).map(|_| ())
            });
            step("projects.txt → state/projects.json".into(), result);
        }
    }
    if let Some(json) = old("project_meta.json") {
        let result = match fenix_project::meta::ProjectMeta::read_legacy(&json, projects.clone()) {
            Some(meta) => meta.save().and_then(|_| back_up(&json, roots).map(|_| ())),
            None => back_up(&json, roots).map(|_| ()),
        };
        step("project_meta.json → state/projects.json".into(), result);
    }

    // Recent files and folders: one state file.
    let recent = roots.state("recent.json");
    for (name, dirs) in [("recent_files.txt", false), ("recent_dirs.txt", true)] {
        if let Some(txt) = old(name) {
            let result = fenix_project::read_path_list(&txt).and_then(|list| {
                let mut store = if dirs { fenix_project::RecentFiles::dirs_or_default(recent.clone()) } else { fenix_project::RecentFiles::load_or_default(recent.clone()) };
                if store.paths().is_empty() {
                    for path in list.into_iter().rev() {
                        store.add(path);
                    }
                    store.save()?;
                }
                back_up(&txt, roots).map(|_| ())
            });
            step(format!("{name} → state/recent.json"), result);
        }
    }

    // Whole files and folders that only change place.
    let moves = [
        ("session.json", roots.state("session.json")),
        ("agenda.json", roots.data.join("agenda.json")),
        ("recovery", roots.recovery()),
        ("tools", roots.tools()),
    ];
    for (name, to) in moves {
        if let Some(from) = old(name) {
            if from != to && !to.exists() {
                step(format!("{name} → {}", to.display()), relocate(&from, &to));
            }
        }
    }
    // Snippets: loose files go into their language's folder.
    let snippets = roots.snippets();
    let loose: Vec<PathBuf> = std::fs::read_dir(&snippets)
        .map(|entries| entries.flatten().map(|e| e.path()).filter(|p| p.is_file() && p.extension().is_some_and(|e| e == "snippet")).collect())
        .unwrap_or_default();
    for file in loose {
        let scopes = std::fs::read_to_string(&file).ok().and_then(|t| fenix_snippets::Snippet::parse(&t).ok()).map(|s| s.scopes).unwrap_or_default();
        let to = snippets.join(fenix_snippets::folder_for(&scopes)).join(file.file_name().unwrap_or_default());
        if !to.exists() {
            let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            step(format!("snippets/{name} → snippets/{}/", fenix_snippets::folder_for(&scopes)), relocate(&file, &to));
        }
    }
    if !report.moved.is_empty() || !report.failed.is_empty() {
        write_log(roots, &report);
    }
    report
}

/// Says what moved in `backup/MIGRATION.txt`, for after the message on
/// the status line is gone.
fn write_log(roots: &Roots, report: &Report) {
    let mut text = String::from("Fenix moved its files to where they live now: settings in settings.toml,\nthis machine's state under state/.\nThe files they replaced are in this folder, as they were.\n\n");
    for line in &report.moved {
        text.push_str(&format!("moved   {line}\n"));
    }
    for line in &report.failed {
        text.push_str(&format!("FAILED  {line} (left where it was; tried again next launch)\n"));
    }
    let path = roots.backup().join("MIGRATION.txt");
    let old = std::fs::read_to_string(&path).map(|t| t + "\n").unwrap_or_default();
    let _ = std::fs::create_dir_all(roots.backup());
    let _ = std::fs::write(&path, old + &text);
}

#[cfg(test)]
mod tests {
    use super::*;
    use fenix_config::Secret;

    #[test]
    fn an_old_installation_moves_over_once_and_leaves_its_files_in_backup() {
        let base = std::env::temp_dir().join(format!("fenix-migrate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let legacy = base.join("Roaming").join("fenix");
        std::fs::create_dir_all(legacy.join("recovery")).unwrap();
        std::fs::create_dir_all(legacy.join("tools").join("clangd")).unwrap();
        std::fs::write(legacy.join("config.ini"), "[editor]\ntheme = Nord\n[gitlab]\ntoken = glpat-x\n[windows]\nwindow1 = 0,0,800,600|false\n").unwrap();
        std::fs::write(legacy.join("projects.txt"), "C:\\code\\fenix\nC:\\code\\widget\n").unwrap();
        std::fs::write(legacy.join("project_meta.json"), "{\"pinned\":[\"C:\\\\code\\\\fenix\"],\"groups\":{}}").unwrap();
        std::fs::write(legacy.join("recent_files.txt"), "C:\\code\\a.rs\nC:\\code\\b.rs\n").unwrap();
        std::fs::write(legacy.join("recent_dirs.txt"), "C:\\code\n").unwrap();
        std::fs::write(legacy.join("session.json"), "{}").unwrap();
        std::fs::write(legacy.join("recovery").join("x.fenixsave"), "unsaved").unwrap();
        std::fs::write(legacy.join("tools").join("clangd").join("clangd.exe"), "binary").unwrap();
        std::fs::create_dir_all(legacy.join("snippets")).unwrap();
        std::fs::write(legacy.join("snippets").join("proc.snippet"), "# key: proc\n# scope: tcl\n# --\nproc x {} {}\n").unwrap();
        std::fs::write(legacy.join("snippets").join("todo.snippet"), "# key: todo\n# --\nTODO\n").unwrap();
        let roots = Roots { settings: legacy.clone(), data: legacy.join("data"), local: base.join("Local").join("fenix") };

        let report = run(&legacy, &roots);
        assert!(report.failed.is_empty(), "{:?}", report.failed);
        assert_eq!(report.moved.len(), 10, "{:?}", report.moved);
        assert!(legacy.join("snippets").join("tcl").join("proc.snippet").is_file() && legacy.join("snippets").join("all").join("todo.snippet").is_file());
        assert!(std::fs::read_to_string(roots.backup().join("MIGRATION.txt")).unwrap().contains("moved   config.ini"));

        let config = fenix_config::Config::load_at(roots.settings_file(), roots.local.join("state")).unwrap();
        assert_eq!(config.theme.as_deref(), Some("Nord"));
        assert_eq!(config.windows.len(), 1);
        assert_eq!(config.token(Secret::GitLab).as_deref(), Some("glpat-x"), "the token came along");
        let known = fenix_project::KnownProjects::load(roots.state("projects.json")).unwrap();
        assert_eq!(known.roots(), [PathBuf::from(r"C:\code\fenix"), PathBuf::from(r"C:\code\widget")], "in the same order");
        assert!(fenix_project::meta::ProjectMeta::load_or_default(roots.state("projects.json")).is_pinned(Path::new(r"C:\code\fenix")));
        assert_eq!(fenix_project::RecentFiles::load_or_default(roots.state("recent.json")).paths()[0], PathBuf::from(r"C:\code\a.rs"));
        assert_eq!(fenix_project::RecentFiles::dirs_or_default(roots.state("recent.json")).paths(), [PathBuf::from(r"C:\code")]);
        assert!(roots.state("session.json").is_file());
        assert_eq!(std::fs::read_to_string(roots.recovery().join("x.fenixsave")).unwrap(), "unsaved");
        assert!(roots.tools().join("clangd").join("clangd.exe").is_file());
        for name in ["config.ini", "projects.txt", "project_meta.json", "recent_files.txt", "recent_dirs.txt"] {
            assert!(!legacy.join(name).exists(), "{name} moved out");
            assert!(roots.backup().join(name).is_file(), "{name} kept in backup");
        }

        let again = run(&legacy, &roots);
        assert!(again.moved.is_empty() && again.failed.is_empty(), "a second run has nothing to do: {again:?}");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_step_that_fails_leaves_its_file_for_next_time() {
        let base = std::env::temp_dir().join(format!("fenix-migrate-fail-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let legacy = base.join("fenix");
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(legacy.join("projects.txt"), "C:\\code\\fenix\n").unwrap();
        let roots = Roots::portable(&legacy);
        // The state folder is a file: the new projects.json can't be written.
        std::fs::write(legacy.join("state"), "in the way").unwrap();
        let report = run(&legacy, &roots);
        assert_eq!(report.failed.len(), 1, "{report:?}");
        assert!(legacy.join("projects.txt").is_file(), "still there");
        let _ = std::fs::remove_dir_all(&base);
    }
}
