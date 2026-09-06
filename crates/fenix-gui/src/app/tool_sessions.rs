use super::*;

/// One language in one canonical project. Its server's environment is frozen
/// at launch; restarting the server re-reads that project's tool configuration.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LspKey {
    pub language: fenix_syntax::LanguageId,
    pub root: PathBuf,
}

pub(super) fn root_for_path(path: &Path) -> PathBuf {
    let path = refactor::identity(path);
    let root = fenix_project::find_project_root(&path).unwrap_or_else(|| path.parent().unwrap_or(Path::new(".")).to_path_buf());
    fenix_lsp::normalize(refactor::identity(&root))
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

    pub(super) fn lsp_key(&self, language: fenix_syntax::LanguageId) -> LspKey {
        LspKey { language, root: self.integration_root() }
    }

    pub(super) fn restart_project_lsp(&mut self) {
        let root = self.integration_root();
        self.lsp_unavailable.retain(|key| key.root != root);
        let removed: Vec<_> = self.lsp_sessions.keys().filter(|key| key.root == root).cloned().collect();
        for key in removed {
            if let Some(session) = self.lsp_sessions.remove(&key) {
                for path in session.open_documents.keys() {
                    self.diagnostics.remove(path);
                }
            }
        }
        self.sync_lsp_for_focused_buffer();
        if !self.lsp_unavailable.iter().any(|key| key.root == root) {
            self.set_message(format!("restarted language services for {}", root.display()));
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
