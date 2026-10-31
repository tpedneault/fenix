use super::*;

/// Which server: a language's primary one, or an additional server
/// (`tools.json`'s `lsp.additional`) by name, which may serve several
/// languages from one process.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum LspServer {
    Primary(fenix_syntax::LanguageId),
    Additional(String),
}

/// One server in one canonical project: one process, shared by every
/// buffer it serves there. Its environment is frozen at launch;
/// restarting the server re-reads that project's tool configuration.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LspKey {
    pub server: LspServer,
    pub root: PathBuf,
}

impl LspKey {
    pub fn primary(language: fenix_syntax::LanguageId, root: PathBuf) -> Self {
        LspKey { server: LspServer::Primary(language), root }
    }

    pub fn is_primary(&self) -> bool {
        matches!(self.server, LspServer::Primary(_))
    }

    pub fn is_primary_for(&self, language: fenix_syntax::LanguageId) -> bool {
        self.server == LspServer::Primary(language)
    }
}

pub(super) fn root_for_path(path: &Path) -> PathBuf {
    let path = refactor::identity(path);
    let root = fenix_project::find_project_root(&path).unwrap_or_else(|| path.parent().unwrap_or(Path::new(".")).to_path_buf());
    fenix_lsp::normalize(refactor::identity(&root))
}

/// The root a language server for `path` runs in: the language workspace
/// its project belongs to (a Cargo or uv workspace, an npm/pnpm
/// workspace, a `go.work`), else its project. One rust-analyzer per Cargo
/// workspace instead of one per member crate, and pyright started where a
/// uv workspace's shared `.venv` is.
pub(super) fn lsp_root_for_path(path: &Path) -> PathBuf {
    let root = root_for_path(path);
    fenix_lsp::normalize(fenix_project::workspace::language_root(&root))
}

pub(super) fn generation() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

impl App {
    pub(super) fn integration_root(&self) -> PathBuf {
        if self.open().kind == BufferKind::TaskOutput {
            if let Some(session) = &self.task_session {
                return session.root.clone();
            }
        }
        if self.open().kind == BufferKind::Debug {
            if let Some(session) = &self.debug_session {
                return session.root.clone();
            }
        }
        if let Some(path) = self.open().buffer.path() {
            return root_for_path(path);
        }
        fenix_lsp::normalize(refactor::identity(&self.project_root.clone().unwrap_or_else(|| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))))
    }

    #[cfg(test)]
    pub(super) fn lsp_key(&self, language: fenix_syntax::LanguageId) -> LspKey {
        LspKey::primary(language, self.integration_root())
    }

    /// `:lsp-restart [NAME]`: restarts this project's language servers --
    /// all of them, or the one called `NAME` (an additional server's name,
    /// a primary server's program, or a language). Each re-reads the
    /// project's tool configuration as it starts again.
    pub(super) fn restart_project_lsp(&mut self, name: &str) {
        let root = self.integration_root();
        let name = name.trim();
        let matches = |app: &App, key: &LspKey| {
            key.root == root
                && (name.is_empty()
                    || app.lsp_server_name(key) == name
                    || match &key.server {
                        LspServer::Primary(language) => crate::lsp::language_config_name(*language) == name,
                        LspServer::Additional(server) => server == name,
                    })
        };
        let removed: Vec<_> = self.lsp_sessions.keys().filter(|key| matches(self, key)).cloned().collect();
        let unavailable: Vec<_> = self.lsp_unavailable.iter().filter(|key| matches(self, key)).cloned().collect();
        if !name.is_empty() && removed.is_empty() && unavailable.is_empty() {
            self.set_error(format!("no language server called {name} in {}", root.display()));
            return;
        }
        for key in &unavailable {
            self.lsp_unavailable.remove(key);
        }
        if name.is_empty() {
            // The list of additional servers is re-read too.
            self.lsp_additional.remove(&root);
        }
        let names: Vec<String> = removed.iter().map(|key| self.lsp_server_name(key)).collect();
        for key in removed {
            self.stop_lsp_session(&key);
        }
        self.sync_lsp_for_focused_buffer();
        let failed = |app: &App| app.lsp_unavailable.iter().any(|key| key.root == root && (name.is_empty() || matches(app, key)));
        if !failed(self) {
            let what = if name.is_empty() || names.is_empty() { "language services".to_string() } else { names.join(", ") };
            self.set_message(format!("restarted {what} for {}", root.display()));
        }
    }

    /// Stops one server: its process, its diagnostics, and anything still
    /// waiting on its answers.
    pub(super) fn stop_lsp_session(&mut self, key: &LspKey) {
        if let Some(session) = self.lsp_sessions.remove(key) {
            self.diagnostics.forget_server(key);
            for pending in session.pending.into_values() {
                self.lsp_request_failed(key, pending);
            }
        }
    }

    /// What `key`'s settings go by: a primary server's language
    /// (`typescript`), an additional server's name -- one an extension
    /// added, as it named it.
    pub(super) fn lsp_settings_name(key: &LspKey) -> String {
        match &key.server {
            LspServer::Primary(language) => crate::lsp::language_config_name(*language),
            LspServer::Additional(name) => name.split_once(':').map(|(_, n)| n.to_string()).unwrap_or_else(|| name.clone()),
        }
    }

    /// The settings `key`'s server is given when it asks
    /// (`workspace/configuration`), as one object: Fenix's own for it (a
    /// Python server's interpreter), the project's
    /// `.fenix/lsp/<name>.json` over that, and what extensions gave
    /// (`language-server-settings`) over that. Null when nothing says
    /// anything.
    pub(super) fn lsp_settings(&self, key: &LspKey) -> serde_json::Value {
        let name = Self::lsp_settings_name(key);
        let mut settings = serde_json::Value::Null;
        if key.is_primary_for(fenix_syntax::LanguageId::Python) {
            let env = fenix_lsp::per_language::python::resolve(&key.root);
            settings = serde_json::json!({ "python": { "pythonPath": env.interpreter.to_string_lossy() } });
        }
        let file = key.root.join(".fenix/lsp").join(format!("{name}.json"));
        if let Some(own) = std::fs::read_to_string(&file).ok().and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok()).filter(|v| v.is_object()) {
            settings = fenix_lsp::pull::merge(settings, own);
        }
        if let Some(given) = self.wasm.server_settings(&key.root, &name) {
            settings = fenix_lsp::pull::merge(settings, given);
        }
        settings
    }

    /// The servers running in `root` whose settings may have changed --
    /// `name`'s, or all of them -- told (`workspace/didChangeConfiguration`):
    /// most ask again (`workspace/configuration`); the rest take what's
    /// sent.
    pub(super) fn lsp_settings_changed(&mut self, root: &Path, name: Option<&str>) {
        let at = fenix_lsp::normalize(refactor::identity(root));
        let keys: Vec<LspKey> = self
            .lsp_sessions
            .iter()
            .filter(|(key, session)| session.capabilities.is_some() && (key.root == at || at.starts_with(&key.root)) && name.is_none_or(|n| Self::lsp_settings_name(key) == n))
            .map(|(key, _)| key.clone())
            .collect();
        for key in keys {
            let settings = self.lsp_settings(&key);
            if let Some(session) = self.lsp_sessions.get(&key) {
                let _ = session.client.notify::<lsp_types::notification::DidChangeConfiguration>(lsp_types::DidChangeConfigurationParams { settings });
            }
        }
    }

    /// What a server is called in messages: an additional server's own
    /// name, or the program a primary server runs.
    pub(super) fn lsp_server_name(&self, key: &LspKey) -> String {
        if let Some(session) = self.lsp_sessions.get(key) {
            return session.name.clone();
        }
        match &key.server {
            LspServer::Primary(language) => format!("{} language server", crate::lsp::language_config_name(*language)),
            LspServer::Additional(name) => name.clone(),
        }
    }

    pub(super) fn debug_scope_matches(&mut self) -> bool {
        if self.debug_session.as_ref().is_some_and(|s| s.root != self.integration_root()) {
            self.set_error("debug session belongs to another project; switch to its debug panel to control or stop it");
            return false;
        }
        true
    }
}
