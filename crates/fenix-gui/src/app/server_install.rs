//! A language server that isn't installed, offered to you: when a file's
//! built-in server (`lsp::default_server_command`) can't be found, a
//! notice says so and how it's installed (`fenix_project::server_install`,
//! the table the doctor and Tool Status read). `SPC l i` (`lsp.install`),
//! or the notice's action, shows the exact command and asks; `y` runs it
//! in Task Output, where you watch it, and once it succeeds the server is
//! started for the files waiting on it. Nothing is installed without that
//! `y`. Where there's no command Fenix would run (clangd on Linux), the
//! notice says what to do and links to it. `lsp.offer_install` (on) turns
//! the notice off; `SPC l i` still works.

use super::*;
use crate::notify::{Request, Urgency};
use fenix_project::server_install::{server_install, ServerInstall};

/// An install asked about, or running.
#[derive(Debug, Clone)]
pub(super) struct PendingInstall {
    /// The server's program: `rust-analyzer`.
    pub program: String,
    /// Where it runs: the project of the file that wanted it.
    pub root: PathBuf,
    pub step: fenix_project::template::Step,
}

#[derive(Debug, Default)]
pub(super) struct InstallState {
    /// Programs whose notice has shown this session: once is enough.
    pub offered: HashSet<String>,
    /// The y/n question, while it's up.
    pub confirm: Option<PendingInstall>,
    /// The install running in Task Output.
    pub running: Option<PendingInstall>,
    /// Where each offered program was wanted, for its notice's action.
    pub roots: HashMap<String, PathBuf>,
}

/// Starts language server `spec` in `root`; one Fenix can't find on PATH
/// is looked for where its installer puts it (`doctor::in_tool_dirs`:
/// rustup's, `go install`'s, uv's, npm's and winget's folders), which an
/// install has just added to a PATH this Fenix started without.
pub(super) fn spawn_lsp_server(
    spec: &fenix_project::tools::CommandSpec,
    root: &Path,
) -> std::io::Result<(fenix_lsp::LspClient, std::sync::mpsc::Receiver<fenix_lsp::LspEvent>)> {
    let spawned = spec.command(root).and_then(fenix_lsp::LspClient::spawn_command);
    match spawned {
        Err(err) if err.kind() == std::io::ErrorKind::NotFound && !spec.executable.contains(['/', '\\']) => match fenix_project::doctor::in_tool_dirs(&spec.executable) {
            Some(found) => {
                let mut spec = spec.clone();
                spec.executable = found.display().to_string();
                spec.command(root).and_then(fenix_lsp::LspClient::spawn_command)
            }
            None => Err(err),
        },
        other => other,
    }
}

impl App {
    /// Server `key` (`executable`) couldn't be started because it isn't
    /// there: for a built-in server, a notice with how to install it,
    /// once a session, unless `lsp.offer_install` is off. True when it
    /// said so, so the caller needn't.
    pub(super) fn offer_server_install(&mut self, key: &LspKey, executable: &str, err: &std::io::Error) -> bool {
        let LspServer::Primary(language) = key.server else { return false };
        if err.kind() != std::io::ErrorKind::NotFound || self.config.bool_value("lsp.offer_install") == Some(false) {
            return false;
        }
        // Only Fenix's own choice of server: one you named is yours to install.
        if crate::lsp::default_server_command(language).is_none_or(|(program, _)| program != executable) {
            return false;
        }
        let Some(install) = server_install(executable) else { return false };
        if !self.server_install.offered.insert(executable.to_string()) {
            return true;
        }
        let what = language.name();
        let request = match &install {
            ServerInstall::Run(step) => Request::new("lsp", format!("{executable} isn't installed"))
                .body(format!("No completion or errors in {what} files. SPC l i installs it: {}", step.display()))
                .action("Install…", format!("lsp.install:{executable}")),
            ServerInstall::Manual { how, url } => Request::new("lsp", format!("{executable} isn't installed"))
                .body(format!("No completion or errors in {what} files. To get it, {how}."))
                .action("How to install it", format!("url:{url}")),
        };
        self.server_install.roots.insert(executable.to_string(), key.root.clone());
        self.notify(request.urgency(Urgency::Normal).tag(format!("install:{executable}")));
        true
    }

    /// `SPC l i`: installs the focused file's language server, once you've
    /// seen the command and said yes.
    pub(crate) fn cmd_lsp_install(&mut self) {
        let Some(language) = self.focused_language() else {
            self.set_error("this buffer has no language -- open a source file first");
            return;
        };
        let Some((program, _)) = crate::lsp::default_server_command(language) else {
            self.set_error(format!("Fenix has no built-in language server for {} -- an extension's (SPC x x) or .fenix/tools.json's is yours to install", language.name()));
            return;
        };
        let root = self.open().buffer.path().map(tool_sessions::lsp_root_for_path).unwrap_or_else(|| self.integration_root());
        self.ask_server_install(&program, root);
    }

    /// A notice's `lsp.install:<program>`, or `SPC l i`: the question,
    /// with the command, for `program` in `root`.
    pub(super) fn ask_server_install(&mut self, program: &str, root: PathBuf) {
        let found = fenix_project::doctor::which(program).or_else(|| fenix_project::doctor::in_tool_dirs(program));
        if let Some(found) = found {
            self.set_message(format!("{program} is installed ({}) -- :lsp-restart starts it", found.display()));
            return;
        }
        if self.server_install.running.is_some() {
            self.set_error("an install is already running -- see Task Output");
            return;
        }
        match server_install(program) {
            Some(ServerInstall::Run(step)) => {
                self.server_install.confirm = Some(PendingInstall { program: program.to_string(), root, step });
                self.wake_caret();
            }
            Some(ServerInstall::Manual { how, url }) => self.set_message(format!("Fenix can't install {program} here -- {how}: {url}")),
            None => self.set_error(format!("Fenix doesn't know how {program} is installed")),
        }
    }

    /// A notice's `lsp.install:<program>`.
    pub(super) fn notice_server_install(&mut self, program: &str) {
        let root = self.server_install.roots.get(program).cloned().unwrap_or_else(|| self.integration_root());
        self.ask_server_install(program, root);
    }

    /// The question in place of the modeline while it's up.
    pub(super) fn server_install_confirm_text(&self) -> Option<String> {
        let pending = self.server_install.confirm.as_ref()?;
        Some(format!("Install {}? y runs `{}` in Task Output, n doesn't", pending.program, pending.step.display()))
    }

    /// The key that answers it: `y` runs the install, anything else
    /// doesn't.
    pub(super) fn server_install_confirm_key(&mut self, key: KeyPress) {
        let Some(pending) = self.server_install.confirm.take() else { return };
        if key.code == KeyCode::Char('y') && key.mods == Default::default() {
            self.run_server_install(pending);
        } else {
            self.set_message(format!("{} not installed", pending.program));
        }
        self.wake_caret();
    }

    fn run_server_install(&mut self, pending: PendingInstall) {
        let spec = &pending.step.spec;
        let task = fenix_tasks::TaskDef { name: format!("install {}", pending.program), command: spec.executable.clone(), args: spec.args.clone() };
        let given = fenix_project::tools::CommandSpec::new(spec.executable.clone(), spec.args.clone());
        let root = pending.root.clone();
        self.server_install.running = Some(pending);
        if !self.run_task_as(task, root, Some(given)) {
            self.server_install.running = None;
        }
    }

    /// Task Output's run ended: an install of ours that worked starts its
    /// server for every file waiting on it; one that didn't says so.
    pub(super) fn server_install_finished(&mut self, success: bool) {
        let Some(pending) = self.server_install.running.take() else { return };
        if !success {
            self.set_error(format!("installing {} failed -- Task Output says why", pending.program));
            return;
        }
        // Every root's server of a language this program serves, given up
        // on because it wasn't there, is tried again.
        let waiting: Vec<LspKey> = self
            .lsp_unavailable
            .iter()
            .filter(|key| matches!(key.server, LspServer::Primary(language) if crate::lsp::default_server_command(language).is_some_and(|(p, _)| p == pending.program)))
            .cloned()
            .collect();
        for key in &waiting {
            self.lsp_unavailable.remove(key);
            self.lsp_failures.remove(key);
        }
        self.server_install.offered.remove(&pending.program);
        let files: Vec<(PathBuf, String, u64, BufferId)> = self
            .buffers
            .ids()
            .filter_map(|id| {
                let ob = self.buffers.get(id)?;
                let path = ob.buffer.path()?.to_path_buf();
                let language = fenix_syntax::detect_language_from_path(&path)?;
                waiting.iter().any(|k| k.is_primary_for(language)).then(|| (path, ob.buffer.text(), ob.buffer.edit_count(), id))
            })
            .collect();
        for (path, text, edit_count, id) in files {
            self.sync_lsp_for(&path, &text, edit_count, Some(id));
        }
        self.set_message(format!("installed {} -- starting it", pending.program));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn not_found() -> std::io::Error {
        std::io::Error::new(std::io::ErrorKind::NotFound, "program not found")
    }

    /// A missing built-in server is offered once, as a notice with the
    /// command and an action; one you named, or another failure, isn't.
    #[test]
    fn a_missing_built_in_server_is_offered_once() {
        let mut app = App::with_file(None);
        let key = LspKey::primary(fenix_syntax::LanguageId::Rust, std::env::temp_dir());
        assert!(!app.offer_server_install(&key, "rust-analyzer", &std::io::Error::other("crashed")), "only a server that isn't there");
        assert!(!app.offer_server_install(&key, "my-analyzer", &not_found()), "a server you named is yours");
        assert!(app.offer_server_install(&key, "rust-analyzer", &not_found()));
        let shown = app.notifications.newest().and_then(|id| app.notifications.get(id)).cloned().expect("a notice");
        let count = app.notifications.history().len();
        assert!(shown.summary.contains("rust-analyzer isn't installed"), "{}", shown.summary);
        assert!(shown.actions.first().is_some_and(|a| a.command == "lsp.install:rust-analyzer" || a.command.starts_with("url:")), "{:?}", shown.actions);
        assert!(app.offer_server_install(&key, "rust-analyzer", &not_found()));
        assert_eq!(app.notifications.history().len(), count, "once a session");
    }

    #[test]
    fn the_offer_can_be_turned_off() {
        let mut app = App::with_file(None);
        app.config.set_value("lsp.offer_install", Some(fenix_config::Value::Bool(false)));
        let key = LspKey::primary(fenix_syntax::LanguageId::Go, std::env::temp_dir());
        assert!(!app.offer_server_install(&key, "gopls", &not_found()));
    }

    /// The question shows the exact command; only `y` runs it, in Task
    /// Output, and its success starts the server that was given up on.
    #[test]
    fn nothing_is_installed_without_a_yes_and_success_starts_the_server() {
        let mut app = App::with_file(None);
        let dir = tempfile::tempdir().unwrap();
        let (program, args): (&str, &[&str]) = if cfg!(windows) { ("cmd", &["/c", "echo", "installed"]) } else { ("echo", &["installed"]) };
        let root = fenix_lsp::normalize(refactor::identity(dir.path()));
        let pending = PendingInstall { program: "gopls".into(), root: root.clone(), step: fenix_project::template::Step::new(program, args) };
        app.server_install.confirm = Some(pending.clone());
        assert!(app.active_prompt_text().is_some_and(|t| t.contains("Install gopls?") && t.contains(&pending.step.display())), "{:?}", app.active_prompt_text());
        app.server_install_confirm_key(KeyPress::char('n'));
        assert!(app.server_install.confirm.is_none() && app.server_install.running.is_none() && app.task_session.is_none(), "n runs nothing");

        let key = LspKey::primary(fenix_syntax::LanguageId::Go, root);
        app.lsp_unavailable.insert(key.clone());
        app.lsp_failures.insert(key.clone(), "couldn't start gopls".into());
        app.server_install.confirm = Some(pending);
        app.server_install_confirm_key(KeyPress::char('y'));
        // Headless, the task runs to its end here.
        assert_eq!(app.open().kind, BufferKind::TaskOutput);
        assert!(app.open().buffer.text().contains("installed"), "{}", app.open().buffer.text());
        assert!(app.server_install.running.is_none(), "finished");
        assert!(!app.lsp_unavailable.contains(&key) && !app.lsp_failures.contains_key(&key), "tried again");
    }
}
