//! The host half of the settings page (`settings_page`): reading the
//! settings into what the page shows, applying a change -- at once, to
//! the running editor, and to `settings.toml` -- and the file itself:
//! opening it at a setting's line, and taking in edits made to it by
//! hand while Fenix runs.

use super::pages::{PageEvent, PageModel};
use super::*;
use crate::settings_page::{Action as SettingsAction, Scope, SecretState, SettingsPage, Snapshot};
use fenix_config::{Kind, Secret};

impl App {
    fn settings_page(&mut self, id: BufferId) -> Option<&mut SettingsPage> {
        match self.pages.get_mut(&id).map(|s| {
            s.stale = true;
            &mut s.model
        }) {
            Some(PageModel::UserSettings(p)) => Some(p),
            _ => None,
        }
    }

    /// The project the focused file is in, to offer its own settings.
    fn settings_project(&self) -> Option<(PathBuf, String)> {
        let root = self.project_root.clone()?;
        let name = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| root.display().to_string());
        Some((root, name))
    }

    fn secret_state(&self, secret: Secret) -> SecretState {
        if secret.from_env().is_some() {
            return SecretState { set: true, source: secret.env().to_string() };
        }
        if self.config.token(secret).is_some() {
            return SecretState { set: true, source: self.secret_store.name().to_string() };
        }
        if secret == Secret::GitHub && fenix_github::gh_token().is_some() {
            return SecretState { set: true, source: "from the gh CLI".to_string() };
        }
        SecretState { set: false, source: String::new() }
    }

    /// What the page shows for `scope`.
    pub(super) fn settings_snapshot(&self, scope: &Scope) -> Snapshot {
        let mine: HashMap<&'static str, fenix_config::Value> = fenix_config::settings()
            .iter()
            .filter(|s| !matches!(s.kind, Kind::Secret(_)))
            .filter_map(|s| s.get(&self.config).map(|v| (s.key, v)))
            .collect();
        let secrets = Secret::ALL.into_iter().map(|s| (s, self.secret_state(s))).collect();
        let mut snap = Snapshot {
            secrets,
            themes: theme::ALL.iter().map(|t| t.name.to_string()).collect(),
            fonts: Vec::new(),
            project: self.settings_project(),
            ..Default::default()
        };
        match scope {
            Scope::You => {
                snap.here = mine;
                snap.problems = self.config.problems.clone();
                snap.file = self.config.path().to_path_buf();
            }
            Scope::Project { root, .. } => {
                let project = fenix_config::ProjectSettings::load(root);
                snap.here = project.values();
                snap.problems = project.problems.clone();
                snap.file = project.path().to_path_buf();
                snap.inherited = mine;
            }
        }
        snap
    }

    /// `SPC p ,`: the focused project's settings -- the settings page in
    /// its scope, or yours when no project is open.
    pub(crate) fn open_project_settings_page(&mut self) {
        let scope = match self.settings_project() {
            Some((root, name)) => Scope::Project { root, name },
            None => {
                self.set_message("no project open -- showing your settings");
                Scope::You
            }
        };
        self.open_settings_page(scope, None);
    }

    /// `SPC ,` (your settings) and `SPC p ,` (the project's): the
    /// settings page, on `key` when there's one to show.
    pub(crate) fn open_settings_page(&mut self, scope: Scope, key: Option<&str>) {
        let snap = self.settings_snapshot(&scope);
        let id = match self.find_page(|m| matches!(m, PageModel::UserSettings(_))) {
            Some(id) => {
                self.show_page(id);
                if let Some(page) = self.settings_page(id) {
                    if page.scope != scope {
                        page.set_scope(scope);
                    }
                    page.refresh(snap);
                }
                id
            }
            None => self.open_page(PageModel::UserSettings(Box::new(SettingsPage::new(scope, snap)))),
        };
        if let (Some(key), Some(page)) = (key, self.settings_page(id)) {
            page.show(key);
        }
    }

    /// Every open settings page, read again.
    pub(super) fn refresh_settings_pages(&mut self) {
        let ids: Vec<BufferId> = self.pages.iter().filter(|(_, s)| matches!(s.model, PageModel::UserSettings(_))).map(|(id, _)| *id).collect();
        for id in ids {
            let Some(scope) = self.settings_page(id).map(|p| p.scope.clone()) else { continue };
            let snap = self.settings_snapshot(&scope);
            if let Some(page) = self.settings_page(id) {
                page.refresh(snap);
            }
        }
    }

    pub(super) fn user_settings_action(&mut self, id: BufferId, action: SettingsAction) {
        match action {
            SettingsAction::None => {}
            SettingsAction::Close => self.close_page(id),
            SettingsAction::SwitchScope(scope) => {
                let snap = self.settings_snapshot(&scope);
                if let Some(page) = self.settings_page(id) {
                    page.set_scope(scope);
                    page.note = None;
                    page.refresh(snap);
                }
            }
            SettingsAction::Set { key, value } => {
                let scope = self.settings_page(id).map(|p| p.scope.clone()).unwrap_or(Scope::You);
                let result = match &scope {
                    Scope::You => self.set_setting(key, value),
                    Scope::Project { root, .. } => {
                        let mut project = fenix_config::ProjectSettings::load(root);
                        let result = project.set(key, value).and_then(|_| project.save().map_err(|e| format!("couldn't save {}: {e}", project.path().display())));
                        if result.is_ok() && self.project_root.as_deref() == Some(root.as_path()) {
                            self.refresh_project_settings(true);
                            self.apply_setting(key);
                        }
                        result
                    }
                };
                let restart = fenix_config::setting(key).is_some_and(|s| s.restart);
                if let Some(page) = self.settings_page(id) {
                    match result {
                        Ok(()) => {
                            page.refused = None;
                            page.note = restart.then(|| ("saved -- it takes effect when Fenix restarts".to_string(), false));
                        }
                        Err(why) => page.refused = Some((key, why)),
                    }
                }
                self.refresh_settings_pages();
            }
            SettingsAction::SetSecret { secret, token } => {
                let result = self.config.set_token(self.secret_store.as_ref(), secret, token.as_deref());
                let note = match (&result, &token) {
                    (Ok(()), Some(_)) => (format!("stored in the {}", self.secret_store.name()), false),
                    (Ok(()), None) => (format!("removed from the {}", self.secret_store.name()), false),
                    (Err(e), _) => (format!("the {} said: {e}", self.secret_store.name()), true),
                };
                if let Some(page) = self.settings_page(id) {
                    page.note = Some(note);
                }
                self.refresh_settings_pages();
            }
            SettingsAction::TestSecret(secret) => self.test_secret(id, secret),
            SettingsAction::OpenProjectPage(root) => self.open_settings(root),
            SettingsAction::OpenFile(key) => {
                let path = match self.settings_page(id).map(|p| p.snap.file.clone()) {
                    Some(path) => path,
                    None => return,
                };
                if !path.exists() {
                    if let Some(parent) = path.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    let _ = std::fs::write(&path, "");
                }
                let line = key.and_then(|k| line_of_key(&std::fs::read_to_string(&path).unwrap_or_default(), k));
                self.open_file_from_picker(&path);
                if let Some(line) = line {
                    self.jump_to_grep_match(&fenix_project::GrepMatch { path, line, col: 1, text: String::new() });
                }
            }
        }
    }

    /// Sets one of your settings: checked, applied to the running editor
    /// and saved.
    pub(crate) fn set_setting(&mut self, key: &str, value: Option<fenix_config::Value>) -> Result<(), String> {
        let setting = fenix_config::setting(key).ok_or_else(|| format!("no setting called {key}"))?;
        setting.set(&mut self.config, value)?;
        self.apply_setting(key);
        self.config.save().map_err(|e| format!("couldn't save settings.toml: {e}"))
    }

    /// Makes the running editor use `key`'s value now. Most settings are
    /// read where they're used, so only these need telling.
    pub(super) fn apply_setting(&mut self, key: &str) {
        match key {
            "editor.theme" => {
                self.theme = self.config.theme.as_deref().and_then(theme::by_name).unwrap_or(&theme::ORBIT_DARK);
            }
            "editor.font_size" | "editor.font_family" => {
                self.apply_font_size();
                self.resync_terminal_cols();
            }
            "editor.indent_width" => {
                let width = self.effective_indent_width();
                self.vim.set_indent_width(width);
            }
            "editor.iskeyword_extra" => {
                let chars = self.effective_iskeyword();
                self.vim.set_iskeyword_extra(chars);
            }
            "mib.roots" => {
                self.mib_roots = self.config.mib_roots.iter().map(|(label, path)| fenix_mib::MibRoot { label: label.clone(), path: path.clone() }).collect();
                self.mib_index = None;
            }
            _ => {}
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    /// Everything applied again, after the file changed by hand.
    fn apply_all_settings(&mut self) {
        for key in ["editor.theme", "editor.font_size", "editor.indent_width", "editor.iskeyword_extra", "mib.roots"] {
            self.apply_setting(key);
        }
    }

    /// Takes in `settings.toml` edited outside the settings page, on the
    /// disk poll's tick: its settings are used at once, and a mistake in
    /// it is named without losing the values that were in use.
    pub(super) fn reload_settings_if_changed(&mut self) {
        let stamp = std::fs::metadata(self.config.path()).and_then(|m| m.modified()).ok();
        if stamp == self.settings_stamp {
            return;
        }
        let first = self.settings_stamp.is_none();
        self.settings_stamp = stamp;
        if first || stamp.is_none() {
            return;
        }
        let before = self.config.problems.len();
        self.config.reload();
        self.apply_all_settings();
        self.refresh_settings_pages();
        match self.config.problems.first() {
            Some(problem) => self.set_error(format!("{problem} -- still using what was there before; SPC , shows it")),
            None if before > 0 => self.set_message("settings.toml read again -- it's fine now"),
            None => {}
        }
    }

    /// Keeps the focused project's own settings at hand: read again when
    /// the project changes, and on the disk poll's tick (`poll`), when
    /// its file may have been edited. What they change is applied.
    pub(super) fn refresh_project_settings(&mut self, poll: bool) {
        let root = self.project_root.clone();
        let same = self.project_settings.as_ref().map(|(r, _)| r) == root.as_ref();
        if same && !poll {
            return;
        }
        let before = self.project_settings.as_ref().map(|(_, p)| p.values());
        self.project_settings = root.map(|r| {
            let settings = fenix_config::ProjectSettings::load(&r);
            (r, settings)
        });
        let after = self.project_settings.as_ref().map(|(_, p)| p.values());
        if before != after || !same {
            for key in ["editor.indent_width", "editor.iskeyword_extra"] {
                self.apply_setting(key);
            }
        }
    }

    /// The focused project's own value for `key`, if it sets one.
    fn project_value(&self, key: &str) -> Option<&fenix_config::Value> {
        self.project_settings.as_ref().and_then(|(_, p)| p.get(key))
    }

    /// Spaces an indent is: the project's, else yours, else 4.
    pub(super) fn effective_indent_width(&self) -> usize {
        match self.project_value("editor.indent_width") {
            Some(fenix_config::Value::Int(n)) => *n as usize,
            _ => self.config.indent_width.unwrap_or(fenix_vim::DEFAULT_INDENT_WIDTH),
        }
    }

    /// Columns a tab takes: the project's, else yours, else 8.
    pub(super) fn effective_tab_width(&self) -> usize {
        match self.project_value("editor.tab_width") {
            Some(fenix_config::Value::Int(n)) => *n as usize,
            _ => self.config.tab_width.unwrap_or(DEFAULT_TAB_WIDTH),
        }
    }

    fn effective_iskeyword(&self) -> Vec<char> {
        let text = match self.project_value("editor.iskeyword_extra") {
            Some(fenix_config::Value::Text(t)) => Some(t.clone()),
            _ => self.config.iskeyword_extra.clone(),
        };
        text.map(|s| s.chars().collect()).unwrap_or_else(|| fenix_vim::DEFAULT_ISKEYWORD_EXTRA.to_vec())
    }

    /// The base branch for the repository at `root`: its project's, else
    /// yours.
    pub(super) fn base_branch_for(&self, root: &Path) -> Option<String> {
        match fenix_config::ProjectSettings::load(root).get("git.base_branch") {
            Some(fenix_config::Value::Text(t)) => Some(t.clone()),
            _ => self.config.git_base_branch.clone(),
        }
    }

    /// Checks a token against its server, off the UI thread.
    fn test_secret(&mut self, id: BufferId, secret: Secret) {
        let token = match secret {
            Secret::GitHub => self.config.token(secret).map(str::to_string).or_else(fenix_github::gh_token),
            _ => self.config.token(secret).map(str::to_string),
        };
        let Some(token) = token else {
            if let Some(page) = self.settings_page(id) {
                page.note = Some(("no token to test -- Enter sets one".into(), true));
            }
            return;
        };
        let server = match secret {
            Secret::GitLab => self.config.gitlab_base_url.clone(),
            Secret::Jira => self.config.jira_base_url.clone(),
            Secret::GitHub => Some("https://api.github.com".into()),
        };
        let Some(server) = server.filter(|s| !s.trim().is_empty()) else {
            if let Some(page) = self.settings_page(id) {
                page.note = Some(("set the server first -- it's the row above".into(), true));
            }
            return;
        };
        if let Some(page) = self.settings_page(id) {
            page.note = Some((format!("asking {server}…"), false));
        }
        self.page_spawn(move |send| {
            let result = match secret {
                Secret::GitLab => fenix_forge::Forge::current_user(&fenix_gitlab::GitLab::new(server.clone(), token, "")),
                Secret::GitHub => fenix_forge::Forge::current_user(&fenix_github::GitHub::new(token, "", "")),
                Secret::Jira => fenix_jira::JiraClient::new(server.clone(), token).myself(),
            };
            let note = match result {
                Ok(user) => (format!("works -- signs in to {server} as {user}"), false),
                Err(e) => (format!("{server} said no: {e}"), true),
            };
            send(PageEvent::SettingsNote { buffer: id, note });
        });
    }

    pub(super) fn apply_settings_event(&mut self, event: PageEvent) {
        if let PageEvent::SettingsNote { buffer, note } = event {
            if let Some(page) = self.settings_page(buffer) {
                page.note = Some(note);
            }
        }
    }
}

/// The line `key` is set on in `text`, 1-based: its own line, or its
/// table's header for a table.
fn line_of_key(text: &str, key: &str) -> Option<usize> {
    let (table, name) = key.rsplit_once('.').unwrap_or(("", key));
    let mut in_table = table.is_empty();
    for (i, line) in text.lines().enumerate() {
        let t = line.trim();
        if t == format!("[{key}]") || t == format!("[[{key}]]") {
            return Some(i + 1);
        }
        if t.starts_with('[') {
            in_table = t.trim_matches(['[', ']']) == table;
            continue;
        }
        if in_table && t.split('=').next().map(str::trim) == Some(name) {
            return Some(i + 1);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::page::Key;

    fn press(app: &mut App, keys: &str) {
        for c in keys.chars() {
            assert!(app.page_key(if c == ' ' { KeyPress::char(' ') } else { KeyPress::char(c) }), "{c}");
        }
    }

    fn page(app: &mut App) -> &mut SettingsPage {
        let id = app.focused_buffer_id();
        app.settings_page(id).expect("the settings page")
    }

    #[test]
    fn a_setting_changed_on_the_page_applies_at_once_and_is_saved() {
        let mut app = App::with_file(None);
        app.open_settings_page(Scope::You, Some("editor.theme"));
        let before = app.theme.name;
        press(&mut app, "l");
        assert_ne!(app.theme.name, before, "the theme changed straight away");
        let saved = std::fs::read_to_string(app.config.path()).unwrap();
        assert!(saved.contains(&format!("theme = \"{}\"", app.theme.name)), "{saved}");

        page(&mut app).show("editor.indent_width");
        assert!(app.page_key(KeyPress::named(FenixNamedKey::Enter)));
        let _ = Key::Enter;
        for c in "3".chars() {
            app.page_key(KeyPress::char(c));
        }
        assert!(app.page_key(KeyPress::named(FenixNamedKey::Enter)));
        assert_eq!(app.config.indent_width, Some(3));
        assert_eq!(app.vim.indent_width(), 3, "vim uses it now");
        press(&mut app, "r");
        assert_eq!(app.config.indent_width, None);
        assert!(!std::fs::read_to_string(app.config.path()).unwrap().contains("indent_width"));
    }

    #[test]
    fn a_token_goes_to_the_store_and_never_to_the_file() {
        let mut app = App::with_file(None);
        app.open_settings_page(Scope::You, Some("gitlab.token"));
        assert!(app.page_key(KeyPress::named(FenixNamedKey::Enter)));
        for c in "glpat-123".chars() {
            app.page_key(KeyPress::char(c));
        }
        assert!(app.page_key(KeyPress::named(FenixNamedKey::Enter)));
        assert_eq!(app.config.gitlab_token.as_deref(), Some("glpat-123"));
        assert_eq!(app.secret_store.get(Secret::GitLab).unwrap().as_deref(), Some("glpat-123"));
        assert!(!std::fs::read_to_string(app.config.path()).unwrap_or_default().contains("glpat"));
        assert_eq!(page(&mut app).snap.secrets[&Secret::GitLab].source, "test");
    }

    #[test]
    fn a_hand_edit_to_the_file_is_taken_in_and_a_mistake_named() {
        let mut app = App::with_file(None);
        app.config.save().unwrap();
        app.reload_settings_if_changed();
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(app.config.path(), "[editor]\nindent_width = 6\n").unwrap();
        app.reload_settings_if_changed();
        assert_eq!(app.config.indent_width, Some(6));
        assert_eq!(app.vim.indent_width(), 6);
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(app.config.path(), "[editor\nindent_width = 9\n").unwrap();
        app.reload_settings_if_changed();
        assert_eq!(app.config.indent_width, Some(6), "the last good value stays");
        assert!(app.status_message.as_ref().is_some_and(|m| m.is_error && m.text.contains("settings.toml:1")), "{:?}", app.status_message.as_ref().map(|m| &m.text));
    }

    #[test]
    fn a_projects_own_settings_apply_to_its_files_and_are_set_from_its_scope() {
        let root = std::env::temp_dir().join(format!("fenix-settings-project-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::create_dir_all(root.join(".fenix")).unwrap();
        std::fs::write(root.join(".fenix").join("settings.toml"), "[editor]
indent_width = 2
[git]
base_branch = \"develop\"
").unwrap();
        std::fs::write(root.join("main.py"), "x = 1
").unwrap();
        let root = fenix_lsp::normalize(std::fs::canonicalize(&root).unwrap());
        let mut app = App::with_file(Some(root.join("main.py").to_string_lossy().into_owned()));
        app.refresh_project_root();
        assert_eq!(app.vim.indent_width(), 2, "the project's, not the default 4");
        assert_eq!(app.base_branch_for(&root).as_deref(), Some("develop"));

        app.open_project_settings_page();
        assert!(matches!(page(&mut app).scope, Scope::Project { .. }));
        page(&mut app).show("editor.tab_width");
        assert!(app.page_key(KeyPress::char('l')), "one more than yours");
        // Back in the project's file, it's in force.
        assert!(app.page_key(KeyPress::char('q')));
        app.refresh_project_root();
        assert_eq!(app.effective_tab_width(), 9);
        let text = std::fs::read_to_string(root.join(".fenix").join("settings.toml")).unwrap();
        assert!(text.contains("tab_width = 9") && text.contains("indent_width = 2"), "{text}");
        assert_eq!(app.config.tab_width, None, "yours is untouched");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn e_opens_the_file_at_the_settings_line() {
        assert_eq!(line_of_key("# hi\n[editor]\ntheme = \"Nord\"\nfont_size = 16\n", "editor.font_size"), Some(4));
        assert_eq!(line_of_key("[git]\nlayout = \"page\"\n[[vnc.hosts]]\nname = \"a\"\n", "vnc.hosts"), Some(3));
        assert_eq!(line_of_key("[editor]\ntheme = \"Nord\"\n", "git.layout"), None);
    }
}
