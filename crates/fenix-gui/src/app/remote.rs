//! Projects on other machines, over SSH.
//!
//! `SPC p R` asks which machine, connects (`fenix_ssh::Host`: the
//! system's `ssh`, Fenix's helper at the far end), and shows the
//! machine's folders to pick the project's from. From there a remote
//! project is a project: its files are paths (`fenix_vfs::remote`) that
//! every file call here takes, to the explorer, the file picker, opening
//! and saving alike.
//!
//! Connecting takes as long as the network does and `ssh` may have
//! something to ask, so it happens on a thread of its own. What it has
//! to say comes back as events: a question for the prompt line
//! (`RemoteQuestion`), how it went (`RemoteConnected`), and from then on
//! every change in the connection (`RemoteStatus`). A machine that drops
//! is connected to again by its `Host`; meanwhile its files fail at once
//! with "not connected" and nothing here waits.

use std::collections::VecDeque;
use std::sync::mpsc;

use super::*;

/// The prompt line's owner while `ssh` has a question.
pub(crate) const PROMPT: &str = "fenix:remote";

/// What a connection was wanted for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Then {
    /// `SPC p R`: show the machine's folders, to pick a project's.
    PickFolder,
    /// A project already known (the hub, `SPC p P`): open it.
    OpenProject { root: PathBuf, file: Option<PathBuf> },
    /// A session restored with a project on this machine in it: nothing
    /// to open, only to look again at what's there.
    Resume,
}

/// Something `ssh` asks, and where the answer goes.
pub struct RemoteQuestion {
    pub(crate) host: String,
    pub(crate) prompt: String,
    pub(crate) answer: mpsc::Sender<Option<String>>,
}

impl std::fmt::Debug for RemoteQuestion {
    // Never the answer; and the question is `ssh`'s own words, which
    // name a key or an account but hold no secret.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemoteQuestion").field("host", &self.host).finish()
    }
}

/// How a connection attempt ended.
pub struct RemoteConnected {
    pub(crate) host: String,
    pub(crate) result: Result<Arc<fenix_ssh::Host>, String>,
}

impl std::fmt::Debug for RemoteConnected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemoteConnected").field("host", &self.host).field("ok", &self.result.is_ok()).finish()
    }
}

/// The machines Fenix is connected to, or trying to be.
#[derive(Default)]
pub(crate) struct Remotes {
    hosts: HashMap<String, Arc<fenix_ssh::Host>>,
    /// The connections being made, and what each is for.
    connecting: HashMap<String, Then>,
    /// How each connection is, as its host last said.
    statuses: HashMap<String, fenix_ssh::Status>,
    /// `ssh`'s questions; the first is the one on the prompt line.
    questions: VecDeque<RemoteQuestion>,
    /// Files a restored session had open on a machine that wasn't
    /// connected yet: opened once it is.
    reopen: Vec<PathBuf>,
}

/// A question as one line for the prompt, and whether what's typed is
/// hidden. `ssh` asks about a new host over several lines; the last is
/// the question, and the one with the key's fingerprint is what it's
/// about.
fn prompt_line(host: &str, prompt: &str) -> (String, bool) {
    let lines: Vec<&str> = prompt.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
    let question = lines.last().copied().unwrap_or("").trim_end_matches(':').trim();
    if prompt.contains("(yes/no") {
        let fingerprint = lines.iter().find(|line| line.contains("fingerprint")).copied().unwrap_or("");
        return (format!("{host} is new here. {fingerprint} Type yes to trust it"), false);
    }
    // `ssh` usually names the machine or the key itself.
    if question.contains(host) {
        return (question.to_string(), true);
    }
    (format!("{host}: {question}"), true)
}

impl App {
    /// `SPC p R`: a project on another machine. The machines you're
    /// connected to come first, then the hosts of `~/.ssh/config`; a
    /// name that's none of them (`user@host`) is taken as typed.
    pub(crate) fn cmd_project_open_remote(&mut self) {
        let mut names: Vec<String> = self.remote.hosts.keys().cloned().collect();
        names.sort();
        for name in fenix_ssh::config::hosts() {
            if !names.contains(&name) {
                names.push(name);
            }
        }
        let candidates = names
            .into_iter()
            .map(|name| {
                let label = match self.remote.statuses.get(&name) {
                    Some(fenix_ssh::Status::Connected) => format!("{name}   connected"),
                    Some(_) => format!("{name}   not connected"),
                    None => name.clone(),
                };
                fenix_picker::Candidate::new(label, name)
            })
            .collect();
        self.enter_picker(ActivePicker::RemoteHost(fenix_picker::PickerState::new(candidates)));
        self.set_message("a machine: one listed, or type user@host and Enter");
    }

    /// The picker's choice: connect, then show the machine's folders.
    pub(super) fn remote_host_picked(&mut self, host: &str) {
        self.remote_connect(host, Then::PickFolder);
    }

    /// Whether `host`'s files can be asked for right now.
    pub(crate) fn remote_connected_to(&self, host: &str) -> bool {
        // A machine mounted by something else (a test's) counts: what
        // matters is that its paths resolve.
        match self.remote.hosts.get(host) {
            Some(known) => known.status() == fenix_ssh::Status::Connected,
            None => fenix_vfs::mounted().iter().any(|mounted| mounted == host),
        }
    }

    /// Whether `path` is on a machine Fenix isn't connected to. Then a
    /// connection is started, with `then` to do once it's there, and the
    /// caller has nothing more to do now.
    pub(crate) fn remote_must_connect(&mut self, path: &Path, then: Then) -> bool {
        match fenix_vfs::host_of(path) {
            Some(host) if !self.remote_connected_to(&host) => {
                self.remote_connect(&host, then);
                true
            }
            _ => false,
        }
    }

    /// Connects to `host`, unless it's connected already, and does `then`.
    pub(crate) fn remote_connect(&mut self, host: &str, then: Then) {
        let host = host.trim();
        if !fenix_vfs::path::host_is_valid(host) || host.starts_with('-') || host.contains(char::is_whitespace) {
            return self.set_error(format!("{host:?} isn't a machine's name: user@host, or a Host of ~/.ssh/config"));
        }
        if self.remote_connected_to(host) {
            return self.remote_ready(host, then);
        }
        // Being connected to already: what it's for is now this.
        if self.remote.connecting.insert(host.to_string(), then).is_some() {
            return;
        }
        self.set_message(format!("connecting to {host}..."));
        // A machine known and lost: its own host connects again, and
        // says so (`remote_status`).
        if let Some(known) = self.remote.hosts.get(host) {
            known.reconnect();
            return;
        }
        let Some(proxy) = self.event_proxy.clone() else {
            // No event loop, so nobody to ask and nowhere to answer.
            self.remote.connecting.remove(host);
            return self.set_error(format!("can't connect to {host} from here"));
        };
        let name = host.to_string();
        let spawned = std::thread::Builder::new().name(format!("fenix-connect-{name}")).spawn(move || {
            let (asking, asked) = (proxy.clone(), name.clone());
            let ask = move |prompt: &str| {
                let (answer, answered) = mpsc::channel();
                let question = RemoteQuestion { host: asked.clone(), prompt: prompt.to_string(), answer };
                asking.send_event(FenixUserEvent::RemoteQuestion(question)).ok()?;
                // Until it's answered, or Fenix closes.
                answered.recv().ok().flatten()
            };
            let result = fenix_ssh::Host::connect(&name, fenix_ssh::Target::new(name.clone()), fenix_ssh::Helpers::shipped(), ask);
            if let Ok(host) = &result {
                let (telling, told) = (proxy.clone(), name.clone());
                host.on_status(move |status| {
                    let _ = telling.send_event(FenixUserEvent::RemoteStatus(told.clone(), status.clone()));
                });
            }
            let result = result.map_err(|error| error.to_string());
            let _ = proxy.send_event(FenixUserEvent::RemoteConnected(Box::new(RemoteConnected { host: name, result })));
        });
        if let Err(error) = spawned {
            self.remote.connecting.remove(host);
            self.set_error(format!("couldn't start connecting to {host}: {error}"));
        }
    }

    /// A connection attempt ended.
    pub(super) fn remote_connected(&mut self, connected: RemoteConnected) {
        let RemoteConnected { host, result } = connected;
        let then = self.remote.connecting.remove(&host);
        // Its questions, if any are left, have nobody to answer to.
        self.remote.questions.retain(|question| question.host != host);
        if self.wasm.prompt.as_ref().is_some_and(|prompt| prompt.ext == PROMPT) {
            self.wasm.prompt = None;
            self.remote_ask_next();
        }
        match result {
            Ok(connection) => {
                self.remote.statuses.insert(host.clone(), fenix_ssh::Status::Connected);
                self.remote.hosts.insert(host.clone(), connection);
                self.set_message(format!("connected to {host}"));
                if let Some(then) = then {
                    self.remote_ready(&host, then);
                }
            }
            Err(why) => self.set_error(format!("{host}: {why}")),
        }
        self.wake_caret();
    }

    /// `host` is connected: what the connection was for.
    fn remote_ready(&mut self, host: &str, then: Then) {
        match then {
            Then::PickFolder => match fenix_vfs::home(host) {
                Ok(home) => self.open_folder_picker(&home, ExplorerPurpose::PickRemoteProject),
                Err(error) => self.set_error(format!("{host}: {error}")),
            },
            Then::OpenProject { root, file } => self.open_project(root, file, true),
            Then::Resume => {
                let (here, later): (Vec<PathBuf>, Vec<PathBuf>) = std::mem::take(&mut self.remote.reopen).into_iter().partition(|path| fenix_vfs::host_of(path).as_deref() == Some(host));
                self.remote.reopen = later;
                for path in here {
                    self.open_file_from_picker(&path);
                }
                self.refresh_project_root();
                self.start_disk_probe();
            }
        }
    }

    /// A connection changed: lost, being tried again, back, or down.
    pub(super) fn remote_status(&mut self, host: String, status: fenix_ssh::Status) {
        match &status {
            fenix_ssh::Status::Connected => {
                self.set_message(format!("connected to {host} again"));
                // What changed there while it was away is looked at now,
                // not at the next tick.
                self.start_disk_probe();
                if let Some(then) = self.remote.connecting.remove(&host) {
                    self.remote.statuses.insert(host.clone(), status.clone());
                    self.remote_ready(&host, then);
                }
            }
            fenix_ssh::Status::Reconnecting { attempt: 1, .. } => self.set_error(format!("the connection to {host} was lost -- connecting again; unsaved work is kept here")),
            fenix_ssh::Status::Reconnecting { .. } => {}
            fenix_ssh::Status::Down(why) => {
                self.remote.connecting.remove(&host);
                self.set_error(format!("{host}: {why} -- SPC p R connects again"));
            }
        }
        self.remote.statuses.insert(host, status);
        self.wake_caret();
    }

    /// `ssh` has a question.
    pub(super) fn remote_question(&mut self, question: RemoteQuestion) {
        self.remote.questions.push_back(question);
        self.remote_ask_next();
    }

    /// Puts the first waiting question on the prompt line, once it's free.
    pub(crate) fn remote_ask_next(&mut self) {
        if self.wasm.prompt.is_some() {
            return;
        }
        let Some(question) = self.remote.questions.front() else { return };
        let (label, masked) = prompt_line(&question.host, &question.prompt);
        self.wasm.prompt = Some(wasm_host::CodePrompt { ext: PROMPT.into(), id: 0, label, input: String::new(), masked });
        self.wake_caret();
    }

    /// The prompt line's answer to `ssh`'s question; `None` for Escape,
    /// which `ssh` takes as no answer and gives up on that way in.
    pub(crate) fn remote_answered(&mut self, answer: Option<String>) {
        if let Some(question) = self.remote.questions.pop_front() {
            let _ = question.answer.send(answer);
        }
        self.remote_ask_next();
    }

    /// The modeline's word on the machine the focused file, or else the
    /// project, is on: its name, and how the connection is when it isn't
    /// simply there.
    pub(crate) fn remote_indicator(&self) -> String {
        let path = self.open().buffer.path().or(self.project_root.as_deref());
        let Some(host) = path.and_then(fenix_vfs::host_of) else { return String::new() };
        let state = match self.remote.statuses.get(&host) {
            Some(fenix_ssh::Status::Connected) => "",
            Some(fenix_ssh::Status::Reconnecting { .. }) => ": reconnecting",
            Some(fenix_ssh::Status::Down(_)) => ": not connected",
            None if self.remote.connecting.contains_key(&host) => ": connecting",
            None if self.remote_connected_to(&host) => "",
            None => ": not connected",
        };
        format!("   [{host}{state}]")
    }

    /// A file the session had open on a machine that isn't connected:
    /// to open when it is (`Then::Resume`).
    pub(crate) fn remote_reopen_later(&mut self, path: PathBuf) {
        if !self.remote.reopen.contains(&path) {
            self.remote.reopen.push(path);
        }
    }

    /// After a session is restored: connects again to each machine a
    /// workspace's project, or an open file, is on.
    pub(crate) fn remote_resume_session(&mut self) {
        let projects = self.workspaces.workspaces.iter().filter_map(|workspace| workspace.project.as_deref().and_then(fenix_vfs::host_of));
        let files = self.buffers.ids_sorted_by_path().into_iter().filter_map(|id| self.buffers.get(id)?.buffer.path().and_then(fenix_vfs::host_of));
        let waiting = self.remote.reopen.iter().filter_map(|path| fenix_vfs::host_of(path));
        let mut hosts: Vec<String> = projects.chain(files).chain(waiting).collect();
        hosts.sort();
        hosts.dedup();
        for host in hosts {
            if !self.remote_connected_to(&host) {
                self.remote_connect(&host, Then::Resume);
            }
        }
    }

    /// Ends every connection, as Fenix closes. Programs running on those
    /// machines end with them.
    pub(crate) fn remote_close_all(&mut self) {
        for (_, host) in self.remote.hosts.drain() {
            host.close();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An app, and a "remote machine" that is a folder here, holding a
    /// small project.
    fn app_and_machine(host: &str) -> (App, tempfile::TempDir, Arc<fenix_vfs::testing::Folder>) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("home/tom/proj/src")).unwrap();
        std::fs::write(dir.path().join("home/tom/proj/src/main.rs"), "fn main() {}\n").unwrap();
        std::fs::write(dir.path().join("home/tom/proj/README.md"), "# proj\n").unwrap();
        let machine = fenix_vfs::testing::Folder::mount(host, dir.path());
        (App::with_file(None), dir, machine)
    }

    #[test]
    fn a_question_from_ssh_is_asked_on_the_prompt_line_and_answered() {
        let mut app = App::with_file(None);
        let (answer, answered) = mpsc::channel();
        app.remote_question(RemoteQuestion { host: "devbox".into(), prompt: "tom@devbox's password: ".into(), answer });
        let (second, second_answered) = mpsc::channel();
        let new_host = "The authenticity of host 'devbox' can't be established.\nED25519 key fingerprint is SHA256:abc.\nAre you sure you want to continue connecting (yes/no/[fingerprint])?";
        app.remote_question(RemoteQuestion { host: "devbox".into(), prompt: new_host.into(), answer: second });

        let prompt = app.wasm.prompt.clone().unwrap();
        assert_eq!((prompt.label.as_str(), prompt.masked), ("tom@devbox's password", true));
        assert_eq!(prompt_line("devbox", "Enter passphrase for key 'C:\\Users\\tom\\.ssh\\id_ed25519': ").0, "devbox: Enter passphrase for key 'C:\\Users\\tom\\.ssh\\id_ed25519'");
        for c in "s3cret".chars() {
            assert!(app.code_prompt_key(KeyPress::char(c)));
        }
        assert!(app.code_prompt_text().unwrap().ends_with("••••••▏"), "what's typed is hidden");
        app.code_prompt_key(KeyPress::named(FenixNamedKey::Enter));
        assert_eq!(answered.try_recv().unwrap().as_deref(), Some("s3cret"));

        // The next one takes the line; Escape is no answer.
        let prompt = app.wasm.prompt.clone().unwrap();
        assert!(prompt.label.contains("SHA256:abc") && prompt.label.contains("yes") && !prompt.masked, "{}", prompt.label);
        app.code_prompt_key(KeyPress::named(FenixNamedKey::Escape));
        assert_eq!(second_answered.try_recv().unwrap(), None);
        assert!(app.wasm.prompt.is_none());
    }

    #[test]
    fn a_remote_folder_is_picked_and_opens_as_a_project() {
        let (mut app, _dir, _machine) = app_and_machine("pick-box");
        let known = tempfile::tempdir().unwrap();
        app.known_projects = fenix_project::KnownProjects::load_or_default(known.path().join("projects.txt"));
        let root = fenix_vfs::remote("pick-box", "/home/tom/proj");

        // Connected already (the test's machine): straight to its folders.
        app.remote_connect("pick-box", Then::PickFolder);
        assert!(app.picking_folder());
        assert_eq!(app.explorer_purpose, ExplorerPurpose::PickRemoteProject);
        assert_eq!(app.explorer.as_ref().map(|e| e.cwd.clone()), Some(fenix_vfs::remote("pick-box", "/")));
        let names: Vec<String> = app.explorer.as_ref().unwrap().entries.iter().map(|e| e.name.clone()).collect();
        assert_eq!(names, ["home"]);

        app.explorer_go_to(&root);
        app.explorer_handle_action(ExplorerAction::SelectCwd);
        assert!(app.explorer.is_none());
        assert_eq!(app.workspaces.active_workspace().project.as_deref(), Some(root.as_path()));
        assert_eq!(app.project_root.as_deref(), Some(root.as_path()));
        // Remembered as it is, to open again.
        let reloaded = fenix_project::KnownProjects::load_or_default(known.path().join("projects.txt"));
        assert_eq!(reloaded.roots(), std::slice::from_ref(&root));
        assert_eq!(app.remote_indicator(), "   [pick-box]");
    }

    #[test]
    fn a_remote_file_is_opened_edited_and_saved_there() {
        let (mut app, dir, _machine) = app_and_machine("edit-box");
        let file = fenix_vfs::remote("edit-box", "/home/tom/proj/src/main.rs");
        app.open_project(fenix_vfs::remote("edit-box", "/home/tom/proj"), Some(file.clone()), false);
        assert_eq!(app.open().buffer.path(), Some(file.as_path()));
        assert_eq!(app.open().buffer.text(), "fn main() {}\n");
        assert_eq!(app.project_root, Some(fenix_vfs::remote("edit-box", "/home/tom/proj")));

        app.test_insert_str("// hi\n");
        app.save();
        assert!(!app.open().buffer.is_dirty(), "{}", app.modeline_pieces().1);
        assert_eq!(std::fs::read_to_string(dir.path().join("home/tom/proj/src/main.rs")).unwrap(), "// hi\nfn main() {}\n");
        // Named as the machine names it, not as this one spells it.
        assert!(app.modeline_pieces().1.contains("saved src/main.rs"), "{}", app.modeline_pieces().1);
    }

    #[test]
    fn a_remote_file_changed_there_is_not_written_over() {
        let (mut app, dir, _machine) = app_and_machine("clash-box");
        let file = fenix_vfs::remote("clash-box", "/home/tom/proj/README.md");
        app.open_project(fenix_vfs::remote("clash-box", "/home/tom/proj"), Some(file), false);
        app.test_insert_str("mine ");
        std::fs::write(dir.path().join("home/tom/proj/README.md"), "# theirs, and longer\n").unwrap();

        app.save();
        assert!(app.modeline_pieces().1.contains("changed on disk"), "{}", app.modeline_pieces().1);
        assert!(app.open().buffer.is_dirty());
        assert_eq!(std::fs::read_to_string(dir.path().join("home/tom/proj/README.md")).unwrap(), "# theirs, and longer\n");
        // `:w!` keeps yours.
        app.save_force();
        assert_eq!(std::fs::read_to_string(dir.path().join("home/tom/proj/README.md")).unwrap(), "mine # proj\n");
    }

    #[test]
    fn a_machine_that_is_away_loses_nothing_and_blanks_nothing() {
        let (mut app, dir, machine) = app_and_machine("away-box");
        let file = fenix_vfs::remote("away-box", "/home/tom/proj/README.md");
        app.open_project(fenix_vfs::remote("away-box", "/home/tom/proj"), Some(file.clone()), false);
        let id = app.focused_buffer_id();
        app.test_insert_str("kept ");

        machine.set_away(true);
        // The poll can't look: that isn't the file being gone.
        let mut sweep = DiskSweep::default();
        app.check_one_file(id, &mut sweep, true, None);
        assert!(sweep.is_empty() && !app.externally_changed.contains(&id), "{sweep:?}");
        // A save fails, says why, and keeps the text.
        app.save();
        assert!(app.modeline_pieces().1.contains("save failed"), "{}", app.modeline_pieces().1);
        assert!(app.open().buffer.is_dirty());
        assert_eq!(app.open().buffer.text(), "kept # proj\n");
        // Another of its files doesn't open as an empty one.
        let other = fenix_vfs::remote("away-box", "/home/tom/proj/src/main.rs");
        app.open_file_from_picker(&other);
        assert_eq!(app.open().buffer.path(), Some(file.as_path()), "still on the file it was on");
        assert!(app.modeline_pieces().1.contains("couldn't open"), "{}", app.modeline_pieces().1);

        machine.set_away(false);
        app.save();
        assert_eq!(std::fs::read_to_string(dir.path().join("home/tom/proj/README.md")).unwrap(), "kept # proj\n");
    }

    #[test]
    fn a_new_remote_file_is_made_in_folders_that_are_not_there_yet() {
        let (mut app, dir, _machine) = app_and_machine("new-box");
        let file = fenix_vfs::remote("new-box", "/home/tom/proj/docs/deep/notes.md");
        app.open_project(fenix_vfs::remote("new-box", "/home/tom/proj"), Some(file.clone()), false);
        assert_eq!(app.open().buffer.path(), Some(file.as_path()), "a file that isn't there yet is a new one, named");
        app.test_insert_str("notes\n");
        app.save();
        assert_eq!(std::fs::read_to_string(dir.path().join("home/tom/proj/docs/deep/notes.md")).unwrap(), "notes\n");
    }

    #[test]
    fn a_known_remote_project_asks_for_its_machine_first() {
        let mut app = App::with_file(None);
        let root = fenix_vfs::remote("nobody-there", "/home/tom/proj");
        app.open_project(root.clone(), None, false);
        // No event loop here to connect on: it says so, and opens nothing.
        assert!(app.modeline_pieces().1.contains("nobody-there"), "{}", app.modeline_pieces().1);
        assert_ne!(app.project_root.as_deref(), Some(root.as_path()));
        assert_eq!(app.workspaces.active_workspace().project, None);
    }

    #[test]
    fn a_sessions_remote_files_open_again_once_their_machine_is_back() {
        let (mut app, _dir, machine) = app_and_machine("resume-box");
        let file = fenix_vfs::remote("resume-box", "/home/tom/proj/src/main.rs");
        // As a restore leaves it: the machine not there, the file noted.
        machine.set_away(true);
        app.remote_reopen_later(file.clone());
        app.remote_reopen_later(fenix_vfs::remote("another-box", "/x"));
        machine.set_away(false);
        app.remote_ready("resume-box", Then::Resume);
        assert_eq!(app.open().buffer.path(), Some(file.as_path()));
        assert_eq!(app.open().buffer.text(), "fn main() {}\n");
        assert_eq!(app.remote.reopen, [fenix_vfs::remote("another-box", "/x")], "another machine's wait for it");
    }

    /// A terminal opened in a remote project is on its machine (the real
    /// helper, in this process); one opened in a local project is here.
    #[cfg(unix)]
    #[test]
    fn a_remote_projects_terminal_runs_on_its_machine() {
        let _machine = fenix_ssh::testing::Loopback::mount("term-app-box").unwrap();
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::with_file(None);
        app.project_root = Some(fenix_vfs::remote("term-app-box", &dir.path().to_string_lossy()));
        app.open_terminal_buffer();
        let id = app.focused_buffer_id();
        let state = app.terminal_buffers.get(&id).expect("a terminal opened");
        assert!(state.session.is_remote(), "{}", app.modeline_pieces().1);
        assert_eq!(state.session.start_dir(), app.project_root.as_deref());
        // The panel too.
        app.toggle_terminal();
        assert!(app.terminal.as_ref().expect("the panel's terminal").session.is_remote());

        let mut local = App::with_file(None);
        local.project_root = Some(dir.path().to_path_buf());
        local.open_terminal_buffer();
        let id = local.focused_buffer_id();
        assert!(!local.terminal_buffers.get(&id).expect("a terminal opened").session.is_remote(), "a local project's terminal is local");
    }

    /// Against a real machine, over the system's `ssh`: a project there
    /// gets its terminal, Git and search. Run by hand, as
    /// dev/remote-ssh/README.md says: `FENIX_REMOTE_TEST` is
    /// `user@host /path/to/a/git/repository`, `FENIX_REMOTE_TEST_SSH`
    /// the `ssh` options, `FENIX_REMOTE_TEST_HELPERS` where the helpers
    /// are (`packaging/build-remote-helpers.sh`'s `remote/`).
    #[test]
    #[ignore = "needs a machine to connect to: see dev/remote-ssh/README.md"]
    fn a_real_machines_project_has_its_terminal_git_and_search() {
        let Ok(test) = std::env::var("FENIX_REMOTE_TEST") else { return };
        let (host, dir) = test.split_once(' ').expect("FENIX_REMOTE_TEST is `user@host /path`");
        let options = std::env::var("FENIX_REMOTE_TEST_SSH").unwrap_or_default().split_whitespace().map(str::to_string).collect();
        let helpers = fenix_ssh::Helpers { folder: std::env::var("FENIX_REMOTE_TEST_HELPERS").unwrap_or_else(|_| "target/remote-check/remote".into()).into() };
        let target = fenix_ssh::Target { options, ..fenix_ssh::Target::new(host.to_string()) };
        let machine = fenix_ssh::Host::connect("real-box", target, helpers, |_| None).expect("connected");
        let root = fenix_vfs::remote("real-box", dir);
        let mut app = App::with_file(None);
        app.remote.hosts.insert("real-box".into(), machine.clone());
        app.remote.statuses.insert("real-box".into(), fenix_ssh::Status::Connected);

        // Git: the repository found there, its status read there, and a
        // change made here seen by it.
        let file = fenix_vfs::join(&root, "README.md");
        app.open_project(root.clone(), Some(file.clone()), false);
        let before = app.open().buffer.text();
        app.test_insert_str("an edit\n");
        app.save();
        assert_eq!(fenix_project::vcs::repository_root(&file), Some(root.clone()));
        assert!(fenix_git::list_files(&root).iter().any(|f| f.path == "README.md"), "git there sees the edit");
        app.open_git_status();
        let page = app.focused_buffer_id();
        assert!(matches!(app.pages.get(&page).map(|s| &s.model), Some(PageModel::Git(g)) if g.root == root), "{}", app.modeline_pieces().1);
        assert_eq!(fenix_git::status(&root).map(|s| s.branch).as_deref(), Some("main"));

        // Search, by the helper's engine.
        app.search_open(crate::app::search_host::SearchOpen::Project);
        let id = app.search_page_id().unwrap();
        for c in "an edit".chars() {
            let action = app.search_page_mut(id).unwrap().key(crate::page::Key::Char(c));
            app.search_action(id, action);
        }
        app.search_fire_due(std::time::Instant::now() + std::time::Duration::from_secs(5));
        let found: Vec<PathBuf> = app.search_page(id).unwrap().files.iter().map(|f| f.path.clone()).collect();
        assert_eq!(found, std::slice::from_ref(&file), "{:?}", app.search_page(id).unwrap().status);

        // A terminal, there.
        app.open_terminal_buffer();
        let terminal = app.focused_buffer_id();
        let state = app.terminal_buffers.get_mut(&terminal).expect("a terminal opened");
        assert!(state.session.is_remote() && state.session.is_alive());

        // Put back as it was.
        fenix_vfs::write(&file, before.as_bytes(), fenix_vfs::Over::Anything).unwrap();
        app.remote_close_all();
    }

    #[test]
    fn a_name_that_is_no_machine_is_refused() {
        let mut app = App::with_file(None);
        for bad in ["", "two words", "-oProxyCommand=evil", "a/b"] {
            app.remote_connect(bad, Then::PickFolder);
            assert!(app.modeline_pieces().1.contains("isn't a machine's name"), "{bad:?}: {}", app.modeline_pieces().1);
            assert!(app.remote.connecting.is_empty());
        }
    }
}
