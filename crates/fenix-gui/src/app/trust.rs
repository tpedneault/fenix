//! Whether a project may have Fenix run the programs it names.
//!
//! A project's `.fenix/tools.json` names its language servers, debug
//! adapters and tasks, and `.fenix/lsp/*.json` a server's settings. A
//! language server starts when a file is opened, so those files would
//! let a project nobody has looked at run a program of its choosing
//! (`fenix_project::trust` has the whole reason). Until its user allows
//! them, a project is treated as naming nothing: your own settings and
//! Fenix's defaults apply, as for a project with no such files.
//!
//! Every reader of what a project names for *running* goes through
//! [`App::project_tools`]. The settings page and the doctor read the
//! file itself: looking is not running.
//!
//! The question is never put in the way of typing. A project that waits
//! says so once (a notification, with its action), and `SPC p y`
//! (`project.trust`) shows the file and asks on the prompt line.

use super::*;
use fenix_project::trust::Verdict;

/// The prompt line's owner while a project's programs are asked about.
pub(crate) const PROMPT: &str = "fenix:trust";

/// The decisions made, and what this session has said about them.
#[derive(Default)]
pub(crate) struct ProjectTrust {
    store: fenix_project::trust::Trust,
    /// Whether decisions are asked for at all. Off where there is nobody
    /// to ask and nothing to protect: the tests' apps, which write a
    /// project's files themselves.
    pub(super) enforced: bool,
    /// The projects already said to be waiting, this session.
    noticed: HashSet<PathBuf>,
    /// The project the prompt line is asking about.
    asking: Option<PathBuf>,
}

impl App {
    /// Starts keeping decisions in Fenix's state folder. The first time
    /// there's no such file: the projects already in use then keep
    /// working as they did, and the question is for what comes after.
    pub(super) fn project_trust_begin(&mut self, file: Option<PathBuf>) {
        let (store, existed) = fenix_project::trust::Trust::load(file);
        self.trust.store = store;
        self.trust.enforced = true;
        if !existed {
            for root in self.known_projects.roots().to_vec() {
                // A remote project's files aren't read for this yet.
                if !fenix_vfs::is_remote(&root) {
                    let _ = self.trust.store.decide(&root, true);
                }
            }
            if let Err(error) = self.trust.store.save() {
                eprintln!("fenix: couldn't keep which projects are trusted: {error}");
            }
        }
    }

    fn project_verdict(&self, root: &Path) -> Verdict {
        if !self.trust.enforced {
            return Verdict::Trusted;
        }
        self.trust.store.verdict(root)
    }

    /// Whether what the project at `root` names may be run.
    pub(crate) fn project_is_trusted(&self, root: &Path) -> bool {
        self.project_verdict(root).allows()
    }

    /// What the project at `root` names to run: its `.fenix/tools.json`,
    /// or nothing at all until its user has allowed it.
    pub(crate) fn project_tools(&self, root: &Path) -> Result<fenix_project::tools::ProjectTools, String> {
        if !self.project_is_trusted(root) {
            return Ok(fenix_project::tools::ProjectTools::default());
        }
        fenix_project::tools::ProjectTools::read(root)
    }

    /// Records that the user allows what the project names now, without
    /// asking: for files Fenix has just written at their word (the
    /// wizard's, the settings page's, the doctor's fix).
    pub(crate) fn trust_project_as_written(&mut self, root: &Path) {
        if let Err(error) = self.trust.store.decide(root, true) {
            self.set_error(format!("couldn't keep that this project is trusted: {error}"));
        }
    }

    /// Runs `write`, something of Fenix's own that changes what the
    /// project names, and keeps the project trusted if it was: the
    /// change is the user's, made here.
    pub(crate) fn keeping_project_trust<T>(&mut self, root: &Path, write: impl FnOnce(&mut App) -> T) -> T {
        let was = self.project_is_trusted(root);
        let result = write(self);
        if was && self.trust.enforced {
            self.trust_project_as_written(root);
        }
        result
    }

    /// Says, once a session, that the project you're in names programs
    /// nobody has allowed yet, and what that means meanwhile.
    pub(super) fn project_trust_notice(&mut self) {
        let Some(root) = self.project_root.clone() else { return };
        if self.project_verdict(&root) != Verdict::Unknown || !self.trust.noticed.insert(root.clone()) {
            return;
        }
        let name = root.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_else(|| root.display().to_string());
        let request = crate::notify::Request::new("fenix", format!("{name} names programs to run"))
            .body("Its .fenix folder sets language servers, tasks or a debugger. They're off until you've looked: yours and Fenix's own are used meanwhile. SPC p y shows what it names and asks.")
            .urgency(crate::notify::Urgency::Normal)
            .action("Look, and decide", "project.trust")
            .tag(format!("project.trust:{}", root.display()));
        self.notify(request);
    }

    /// `SPC p y` (`project.trust`): shows what the project names to run,
    /// and asks on the prompt line whether it may.
    pub(crate) fn cmd_project_trust(&mut self) {
        let Some(root) = self.project_root.clone() else { return self.set_message("no project here") };
        let files = fenix_project::trust::files(&root);
        let verdict = self.project_verdict(&root);
        if !self.trust.enforced || files.is_empty() || verdict == Verdict::Nothing {
            return self.set_message("this project names no programs of its own: nothing to allow");
        }
        // What's being asked about, in front of the question.
        self.open_file_from_picker(&files[0]);
        let name = root.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
        let now = match verdict {
            Verdict::Trusted => " (allowed now)",
            Verdict::Refused => " (refused now)",
            _ => "",
        };
        let what = match files.len() {
            1 => "this file".to_string(),
            n => format!("this file and {} more in .fenix/lsp", n - 1),
        };
        self.trust.asking = Some(root);
        self.wasm.prompt = Some(wasm_host::CodePrompt { ext: PROMPT.into(), id: 0, label: format!("Let {name} run the programs {what} names{now}? yes / no"), input: String::new(), masked: false });
        self.wake_caret();
    }

    /// The prompt line's answer: `yes`, `no`, or nothing (Escape, or
    /// anything else), which decides nothing.
    pub(crate) fn project_trust_answered(&mut self, answer: Option<String>) {
        let Some(root) = self.trust.asking.take() else { return };
        let trusted = match answer.as_deref().map(str::trim).map(str::to_ascii_lowercase).as_deref() {
            Some("yes" | "y") => true,
            Some("no" | "n") => false,
            _ => return self.set_message("nothing decided: its programs stay off"),
        };
        if let Err(error) = self.trust.store.decide(&root, trusted) {
            return self.set_error(format!("couldn't keep the answer: {error}"));
        }
        self.notifications.dismiss_tag("fenix", &format!("project.trust:{}", root.display()));
        // What was read of it while it waited was nothing: read again.
        self.lsp_additional.remove(&root);
        if trusted {
            self.set_message("allowed: its language servers, tasks and debugger are the project's from here (asked again if those files change)");
            // The servers it names, for the files already open.
            self.restart_language_servers_in(&root);
            self.sync_lsp_for_focused_buffer();
        } else {
            self.set_message("refused: yours and Fenix's own are used in this project (SPC p y to change that)");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An app that asks, as a real launch does, with its decisions in a
    /// folder of the test's; and a project that names a language server.
    fn app_and_project(tools: &str) -> (App, tempfile::TempDir, tempfile::TempDir) {
        let state = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(project.path().join(".fenix")).unwrap();
        std::fs::write(project.path().join(".fenix/tools.json"), tools).unwrap();
        std::fs::write(project.path().join("main.rs"), "fn main() {}\n").unwrap();
        let mut app = App::with_file(None);
        // Before the store begins, so this project isn't one already in use.
        app.known_projects = fenix_project::KnownProjects::load_or_default(state.path().join("projects.txt"));
        app.project_trust_begin(Some(state.path().join("trusted-projects.json")));
        (app, state, project)
    }

    const NAMES_A_SERVER: &str = r#"{"lsp": {"rust": {"executable": "./payload"}}, "tasks": {"build": {"executable": "./payload"}}, "launch": {"program": "./payload"}}"#;

    fn answer(app: &mut App, text: &str) {
        for c in text.chars() {
            assert!(app.code_prompt_key(KeyPress::char(c)));
        }
        assert!(app.code_prompt_key(KeyPress::named(FenixNamedKey::Enter)));
    }

    #[test]
    fn a_project_nobody_allowed_names_nothing_to_run() {
        let (mut app, _state, project) = app_and_project(NAMES_A_SERVER);
        let root = project.path().to_path_buf();
        assert!(!app.project_is_trusted(&root));
        let tools = app.project_tools(&root).unwrap();
        assert!(tools.lsp.is_empty() && tools.tasks.is_empty() && tools.launch.is_empty(), "as if the file weren't there");
        assert!(!app.tasks_for(&root).unwrap().iter().any(|task| task.command.contains("payload")));
        assert!(app.additional_lsp_servers(&root, fenix_syntax::LanguageId::Rust).is_empty());

        // Opening one of its files starts nothing the project named.
        app.open_file_from_picker(&root.join("main.rs"));
        assert!(app.lsp_sessions.values().all(|session| !session.name.contains("payload")), "no session with the project's program");
        // And it's said, once.
        let waiting = |app: &App| app.notifications.history().iter().filter(|n| n.summary.contains("names programs to run")).count();
        assert_eq!(waiting(&app), 1);
        app.refresh_project_root();
        assert_eq!(waiting(&app), 1);
    }

    #[test]
    fn yes_allows_what_it_names_until_that_changes() {
        let (mut app, state, project) = app_and_project(r#"{"tasks": {"build": {"executable": "make"}}}"#);
        let root = project.path().to_path_buf();
        app.project_root = Some(root.clone());

        app.cmd_project_trust();
        assert_eq!(app.open().buffer.path().map(|p| p.ends_with("tools.json")), Some(true), "the file is shown");
        assert!(app.wasm.prompt.as_ref().unwrap().label.contains("yes / no"));
        answer(&mut app, "yes");
        assert!(app.project_is_trusted(&root));
        assert_eq!(app.project_tools(&root).unwrap().tasks.len(), 1);
        // Kept for the next launch.
        let (kept, _) = fenix_project::trust::Trust::load(Some(state.path().join("trusted-projects.json")));
        assert_eq!(kept.verdict(&root), Verdict::Trusted);

        // A pull brings another command: asked again.
        std::fs::write(root.join(".fenix/tools.json"), r#"{"tasks": {"build": {"executable": "./payload"}}}"#).unwrap();
        assert!(!app.project_is_trusted(&root));
        assert!(app.project_tools(&root).unwrap().tasks.is_empty());
    }

    #[test]
    fn no_and_no_answer_leave_its_programs_off() {
        let (mut app, _state, project) = app_and_project(NAMES_A_SERVER);
        let root = project.path().to_path_buf();
        app.project_root = Some(root.clone());

        app.cmd_project_trust();
        assert!(app.code_prompt_key(KeyPress::named(FenixNamedKey::Escape)));
        assert!(!app.project_is_trusted(&root) && app.wasm.prompt.is_none());
        app.cmd_project_trust();
        answer(&mut app, "maybe");
        assert!(!app.project_is_trusted(&root));
        app.cmd_project_trust();
        answer(&mut app, "no");
        assert!(!app.project_is_trusted(&root));
        assert!(app.modeline_pieces().1.contains("refused"), "{}", app.modeline_pieces().1);
        // Refused isn't asked about again by itself.
        app.trust.noticed.clear();
        let before = app.notifications.history().len();
        app.project_trust_notice();
        assert_eq!(app.notifications.history().len(), before);
        // But can be changed.
        app.cmd_project_trust();
        assert!(app.wasm.prompt.as_ref().unwrap().label.contains("refused now"));
        answer(&mut app, "y");
        assert!(app.project_is_trusted(&root));
    }

    #[test]
    fn a_servers_settings_wait_for_the_answer_too() {
        let (mut app, _state, project) = app_and_project("{}");
        let root = project.path().to_path_buf();
        std::fs::create_dir_all(root.join(".fenix/lsp")).unwrap();
        std::fs::write(root.join(".fenix/lsp/rust-analyzer.json"), r#"{"check": {"overrideCommand": ["./payload"]}}"#).unwrap();
        let key = tool_sessions::LspKey { server: tool_sessions::LspServer::Additional("rust-analyzer".into()), root: root.clone() };
        assert!(!app.lsp_settings(&key).to_string().contains("payload"), "not sent to the server");
        app.project_root = Some(root.clone());
        app.cmd_project_trust();
        answer(&mut app, "yes");
        assert!(app.lsp_settings(&key).to_string().contains("payload"));
    }

    #[test]
    fn projects_in_use_before_the_question_existed_go_on_working() {
        let state = tempfile::tempdir().unwrap();
        let old = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(old.path().join(".fenix")).unwrap();
        std::fs::write(old.path().join(".fenix/tools.json"), r#"{"tasks": {"build": {"executable": "make"}}}"#).unwrap();
        let mut app = App::with_file(None);
        app.known_projects = fenix_project::KnownProjects::load_or_default(state.path().join("projects.txt"));
        app.known_projects.add(old.path().to_path_buf());
        app.project_trust_begin(Some(state.path().join("trusted-projects.json")));
        assert!(app.project_is_trusted(old.path()));

        // The next launch finds the file, and takes nothing more on faith.
        let later = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(later.path().join(".fenix")).unwrap();
        std::fs::write(later.path().join(".fenix/tools.json"), "{}").unwrap();
        app.known_projects.add(later.path().to_path_buf());
        app.project_trust_begin(Some(state.path().join("trusted-projects.json")));
        assert!(app.project_is_trusted(old.path()) && !app.project_is_trusted(later.path()));
    }

    #[test]
    fn what_fenix_writes_for_you_stays_allowed() {
        let (mut app, _state, project) = app_and_project(r#"{"tasks": {"build": {"executable": "make"}}}"#);
        let root = project.path().to_path_buf();
        // Not allowed: a write of Fenix's doesn't allow it by the back door.
        app.keeping_project_trust(&root, |_| std::fs::write(root.join(".fenix/tools.json"), r#"{"tasks": {"t": {"executable": "x"}}}"#).unwrap());
        assert!(!app.project_is_trusted(&root));
        // Allowed: it stays so across the write.
        app.trust_project_as_written(&root);
        app.keeping_project_trust(&root, |_| std::fs::write(root.join(".fenix/tools.json"), r#"{"tasks": {"u": {"executable": "y"}}}"#).unwrap());
        assert!(app.project_is_trusted(&root));
        // A project with no such files yet, given some by Fenix.
        let fresh = tempfile::tempdir().unwrap();
        app.keeping_project_trust(fresh.path(), |_| {
            std::fs::create_dir_all(fresh.path().join(".fenix")).unwrap();
            std::fs::write(fresh.path().join(".fenix/tools.json"), "{}").unwrap();
        });
        assert!(app.project_is_trusted(fresh.path()));
    }
}
