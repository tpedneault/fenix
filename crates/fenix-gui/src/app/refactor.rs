use super::*;
use fenix_lsp::workspace_edit::{Document, PlannedDocument};
use std::time::SystemTime;

pub(super) fn identity(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

#[derive(Debug, Clone)]
struct Revision {
    buffer: BufferId,
    edits: u64,
    version: Option<i32>,
}
#[derive(Debug, Clone)]
pub(super) struct Context {
    started: SystemTime,
    root: PathBuf,
    revisions: HashMap<PathBuf, Revision>,
}
struct Target {
    buffer: Option<BufferId>,
    revision: Option<u64>,
    disk: String,
}
enum Contents {
    Actions(Vec<lsp_types::CodeAction>, Context),
    Edits(Vec<PlannedDocument>, HashMap<PathBuf, Target>),
}
pub(super) struct Preview {
    pub(super) buffer: BufferId,
    origin: BufferId,
    contents: Contents,
}
struct UndoFile {
    path: PathBuf,
    buffer: BufferId,
    revision: u64,
    before: String,
    after: String,
    was_dirty: bool,
}
pub(super) struct Undo {
    files: Vec<UndoFile>,
}

impl App {
    pub(super) fn refactor_context(&self, language: fenix_syntax::LanguageId) -> Context {
        let mut revisions = HashMap::new();
        for id in self.buffers.ids_sorted_by_path() {
            let ob = self.buffers.get(id).unwrap();
            let Some(path) = ob.buffer.path() else { continue };
            let version = self.lsp_sessions.get(&self.lsp_key(language)).and_then(|session| {
                session.open_documents.iter().find_map(|(known, (edits, next))| (identity(known) == identity(path) && *edits == ob.buffer.edit_count()).then_some(next - 1))
            });
            revisions.insert(identity(path), Revision { buffer: id, edits: ob.buffer.edit_count(), version });
        }
        Context { started: SystemTime::now(), root: self.integration_root(), revisions }
    }

    pub(super) fn preview_lsp_edit(&mut self, edit: lsp_types::WorkspaceEdit, context: Context) {
        if self.refactor_preview.is_some() {
            self.set_error("finish or cancel the current refactor preview first");
            return;
        }
        let mut targets = HashMap::new();
        let plan = fenix_lsp::workspace_edit::prepare(edit, |path| {
            let path = identity(path);
            if tool_sessions::root_for_path(&path) != context.root { return Err(format!("{} belongs to another project", path.display())); }
            let metadata = std::fs::metadata(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            if !metadata.is_file() || metadata.permissions().readonly() {
                return Err(format!("{} is not a writable file", path.display()));
            }
            if metadata.modified().map_err(|e| e.to_string())? > context.started {
                return Err(format!("{} changed on disk after the request", path.display()));
            }
            let disk = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            let buffer = self.buffer_id_for_lsp_path(&path);
            let (text, revision, version) = if let Some(id) = buffer {
                let ob = self.buffers.get(id).unwrap();
                if !ob.kind.tracks_unsaved_changes() || self.externally_changed.contains(&id) {
                    return Err(format!("{} is read-only or has an external conflict", path.display()));
                }
                if self.disk_state.get(&id).is_some_and(|known| Some(*known) != DiskFingerprint::of(&path)) {
                    return Err(format!("{} changed on disk since it was loaded or saved", path.display()));
                }
                let snapshot = context.revisions.get(&path).ok_or_else(|| format!("{} was opened after the request", path.display()))?;
                if snapshot.buffer != id || snapshot.edits != ob.buffer.edit_count() {
                    return Err(format!("{} changed while waiting for the server", path.display()));
                }
                let text = ob.buffer.text();
                if text != disk && (!ob.buffer.is_dirty() || snapshot.version.is_none()) {
                    return Err(format!("{} has changes the server has not seen", path.display()));
                }
                (text, Some(snapshot.edits), snapshot.version)
            } else {
                if context.revisions.contains_key(&path) {
                    return Err(format!("{} was closed after the request", path.display()));
                }
                (disk.clone(), None, None)
            };
            targets.insert(path.clone(), Target { buffer, revision, disk });
            Ok(Document { path, text, version })
        });
        match plan {
            Err(error) => self.set_error(format!("Refactor rejected: {error}")),
            Ok(plan) if plan.is_empty() => self.set_message("no changes proposed"),
            Ok(plan) => {
                let text = render(&plan);
                self.open_refactor_preview(&text, Contents::Edits(plan, targets));
            }
        }
    }

    fn open_refactor_preview(&mut self, text: &str, contents: Contents) {
        let origin = self.focused_buffer_id();
        let buffer = self.buffers.open_text_view(text);
        self.buffers.get_mut(buffer).unwrap().kind = BufferKind::WorkspaceEdit;
        self.open_buffer_in_focused_pane(buffer);
        self.refactor_preview = Some(Preview { buffer, origin, contents });
        self.wake_caret();
    }

    pub(super) fn preview_lsp_actions(&mut self, actions: Vec<lsp_types::CodeActionOrCommand>, context: Context) {
        if self.refactor_preview.is_some() {
            self.set_error("finish or cancel the current refactor preview first");
            return;
        }
        // Never apply the edit half of an edit+command action: both are required.
        let actions: Vec<_> = actions
            .into_iter()
            .filter_map(|action| match action {
                lsp_types::CodeActionOrCommand::CodeAction(action) if action.disabled.is_none() && action.command.is_none() && action.edit.is_some() => Some(action),
                _ => None,
            })
            .collect();
        if actions.is_empty() {
            self.set_error("no supported code action (commands and disabled actions are not applied)");
            return;
        }
        if actions.len() == 1 {
            self.preview_lsp_edit(actions.into_iter().next().unwrap().edit.unwrap(), context);
            return;
        }
        let mut text = "Code actions — Enter previews the action under the cursor; q cancels\n\n".to_string();
        for action in &actions {
            text.push_str(&action.title.replace(['\n', '\r'], " "));
            text.push('\n');
        }
        self.open_refactor_preview(&text, Contents::Actions(actions, context));
        let line = self.open().buffer.line_start_char(2);
        self.pane_state_mut(self.focused_pane_id()).cursor = Cursor { char_idx: line, sticky_col: 0 };
    }

    pub(super) fn refactor_key(&mut self, key: KeyPress) -> bool {
        if self.vim.mode() != Mode::Normal || self.open().kind != BufferKind::WorkspaceEdit {
            return false;
        }
        if key == KeyPress::char('q') || key == KeyPress::named(FenixNamedKey::Escape) {
            self.close_refactor_preview();
            return true;
        }
        if key == KeyPress::char('a') || key == KeyPress::named(FenixNamedKey::Enter) {
            let row = self.open().buffer.line_col(&self.cursor()).0;
            let selection = self.refactor_preview.as_ref().and_then(|p| match &p.contents {
                Contents::Actions(actions, context) => row.checked_sub(2).and_then(|index| actions.get(index)).map(|a| (a.edit.clone().unwrap(), context.clone())),
                _ => None,
            });
            if let Some((edit, context)) = selection {
                self.close_refactor_preview();
                self.preview_lsp_edit(edit, context);
            } else if self.refactor_preview.as_ref().is_some_and(|p| matches!(p.contents, Contents::Edits(..))) {
                self.commit_refactor();
            }
            return true;
        }
        false
    }

    fn close_refactor_preview(&mut self) {
        if let Some(preview) = self.refactor_preview.take() {
            self.open_buffer_in_focused_pane(preview.buffer);
            self.kill_buffer_now();
            if self.buffers.get(preview.origin).is_some() {
                self.open_buffer_in_focused_pane(preview.origin);
            }
        }
        self.wake_caret();
    }

    pub(super) fn commit_refactor(&mut self) {
        let Some(preview) = self.refactor_preview.as_ref() else { return };
        let Contents::Edits(plan, targets) = &preview.contents else { return };
        let mut loaded = HashMap::new();
        let validation = (|| -> Result<(), String> {
            for change in plan {
                let target = &targets[&change.path];
                let metadata = std::fs::metadata(&change.path).map_err(|e| e.to_string())?;
                if !metadata.is_file() || metadata.permissions().readonly() {
                    return Err(format!("{} is no longer writable", change.path.display()));
                }
                if std::fs::read_to_string(&change.path).map_err(|e| e.to_string())? != target.disk {
                    return Err(format!("{} changed on disk during preview", change.path.display()));
                }
                if self.buffer_id_for_lsp_path(&change.path) != target.buffer {
                    return Err("open documents changed during preview; request the refactor again".into());
                }
                if let Some(id) = target.buffer {
                    let ob = self.buffers.get(id).ok_or("a target buffer was closed")?;
                    if !ob.kind.tracks_unsaved_changes()
                        || Some(ob.buffer.edit_count()) != target.revision
                        || ob.buffer.text() != change.before
                        || self.externally_changed.contains(&id)
                    {
                        return Err(format!("{} changed during preview", change.path.display()));
                    }
                } else {
                    let buffer = Buffer::from_path(&change.path).map_err(|e| e.to_string())?;
                    if buffer.text() != change.before {
                        return Err(format!("{} changed while loading", change.path.display()));
                    }
                    loaded.insert(change.path.clone(), buffer);
                }
            }
            Ok(())
        })();
        if let Err(error) = validation {
            self.set_error(format!("Refactor rejected: {error}; no edits applied"));
            return;
        }
        // From here through commit, only infallible in-memory operations occur.
        // No file is saved automatically; normal save/recovery protections apply.
        let plan = plan.clone();
        let existing: HashMap<_, _> = targets.iter().map(|(path, target)| (path.clone(), target.buffer)).collect();
        let mut files = Vec::new();
        for change in plan {
            let id = existing[&change.path].unwrap_or_else(|| self.buffers.insert_loaded_document(loaded.remove(&change.path).unwrap()));
            if existing[&change.path].is_none() {
                self.note_disk_state(id);
            }
            let was_dirty = self.buffers.get(id).unwrap().buffer.is_dirty();
            self.replace_buffer_from_disk(id, &change.after);
            let ob = self.buffers.get_mut(id).unwrap();
            ob.buffer.mark_unsaved();
            files.push(UndoFile { path: change.path, buffer: id, revision: ob.buffer.edit_count(), before: change.before, after: change.after, was_dirty });
        }
        let ids: Vec<_> = files.iter().map(|f| f.buffer).collect();
        self.refactor_undo = Some(Undo { files });
        self.close_refactor_preview();
        self.sync_refactor_buffers(&ids);
        self.snapshot_dirty_buffers();
        self.set_message(format!("refactor applied to {} file(s), left unsaved — :undo-refactor reverses all", ids.len()));
        self.wake_caret();
    }

    pub(super) fn undo_refactor(&mut self) {
        let Some(undo) = &self.refactor_undo else {
            self.set_error("no refactor to undo");
            return;
        };
        for file in &undo.files {
            if !self.buffers.get(file.buffer).is_some_and(|ob| {
                ob.buffer.path().map(identity).as_ref() == Some(&file.path)
                    && ob.kind.tracks_unsaved_changes()
                    && ob.buffer.edit_count() == file.revision
                    && ob.buffer.text() == file.after
            }) {
                self.set_error("cannot undo refactor: a target was edited or closed; no buffers changed");
                return;
            }
        }
        let undo = self.refactor_undo.take().unwrap();
        let ids: Vec<_> = undo.files.iter().map(|f| f.buffer).collect();
        for file in undo.files {
            let ob = self.buffers.get_mut(file.buffer).unwrap();
            ob.buffer.undo(&mut ob.cursor);
            if !file.was_dirty && ob.buffer.path().and_then(|p| std::fs::read_to_string(p).ok()).as_deref() == Some(file.before.as_str()) {
                ob.buffer.mark_saved();
            }
        }
        for pane in self.windows().windows() {
            if let Some(id) = self.windows().content(pane).copied() {
                if ids.contains(&id) {
                    let end = self.buffers.get(id).unwrap().buffer.len_chars();
                    let cursor = &mut self.pane_state_mut(pane).cursor;
                    cursor.char_idx = cursor.char_idx.min(end);
                }
            }
        }
        self.sync_refactor_buffers(&ids);
        self.snapshot_dirty_buffers();
        self.set_message(format!("undid refactor across {} file(s)", ids.len()));
        self.wake_caret();
    }

    fn sync_refactor_buffers(&mut self, ids: &[BufferId]) {
        for id in ids {
            let ob = self.buffers.get(*id).unwrap();
            let Some(path) = ob.buffer.path() else { continue };
            let canonical = fenix_lsp::normalize(identity(path));
            let Some(uri) = fenix_lsp::path_to_uri(&canonical) else { continue };
            let language = fenix_syntax::detect_language_from_path(path);
            let text = ob.buffer.text();
            let revision = ob.buffer.edit_count();
            for (server_language, session) in &mut self.lsp_sessions {
                if session.capabilities.is_none() {
                    continue;
                }
                let result = if let Some((_, version)) = session.open_documents.get(&canonical) {
                    let version = *version;
                    session
                        .client
                        .notify::<lsp_types::notification::DidChangeTextDocument>(lsp_types::DidChangeTextDocumentParams {
                            text_document: lsp_types::VersionedTextDocumentIdentifier { uri: uri.clone(), version },
                            content_changes: vec![lsp_types::TextDocumentContentChangeEvent { range: None, range_length: None, text: text.clone() }],
                        })
                        .map(|_| version + 1)
                } else if language == Some(server_language.language) && tool_sessions::root_for_path(path) == server_language.root {
                    session
                        .client
                        .notify::<lsp_types::notification::DidOpenTextDocument>(lsp_types::DidOpenTextDocumentParams {
                            text_document: lsp_types::TextDocumentItem {
                                uri: uri.clone(),
                                language_id: crate::lsp::language_config_name(server_language.language),
                                version: 0,
                                text: text.clone(),
                            },
                        })
                        .map(|_| 1)
                } else {
                    continue;
                };
                if let Ok(next) = result {
                    session.open_documents.insert(canonical.clone(), (revision, next));
                }
            }
        }
    }
}

fn render(plan: &[PlannedDocument]) -> String {
    let mut out = format!(
        "Refactor preview — {} file(s)\na / Enter: apply all in memory   q: cancel   :undo-refactor: undo last refactor\nFiles stay unsaved until you save them.\n",
        plan.len()
    );
    for change in plan {
        out.push_str(&format!("\n--- {}\n+++ {}\n", change.path.display(), change.path.display()));
        let visible = |text: &str| -> Vec<String> {
            text.split_inclusive('\n')
                .map(|line| {
                    if let Some(line) = line.strip_suffix("\r\n") {
                        format!("{line} [CRLF]")
                    } else if let Some(line) = line.strip_suffix('\n') {
                        line.to_string()
                    } else {
                        format!("{line} [no final newline]")
                    }
                })
                .collect()
        };
        let old = visible(&change.before);
        let new = visible(&change.after);
        let prefix = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
        let suffix = old[prefix..].iter().rev().zip(new[prefix..].iter().rev()).take_while(|(a, b)| a == b).count();
        out.push_str(&format!("@@ line {} @@\n", prefix + 1));
        for line in &old[prefix.saturating_sub(2)..prefix] {
            out.push_str(&format!(" {line}\n"));
        }
        for line in &old[prefix..old.len() - suffix] {
            out.push_str(&format!("-{line}\n"));
        }
        for line in &new[prefix..new.len() - suffix] {
            out.push_str(&format!("+{line}\n"));
        }
        for line in old[old.len() - suffix..].iter().take(2) {
            out.push_str(&format!(" {line}\n"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (tempfile::TempDir, App, PathBuf, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.txt");
        let b = dir.path().join("b.txt");
        std::fs::write(&a, "old\n").unwrap();
        std::fs::write(&b, "old\n").unwrap();
        let app = App::with_file(Some(a.to_string_lossy().into_owned()));
        (dir, app, a, b)
    }
    fn proposal(paths: &[&Path]) -> lsp_types::WorkspaceEdit {
        lsp_types::WorkspaceEdit {
            changes: Some(
                paths
                    .iter()
                    .map(|p| {
                        (
                            fenix_lsp::path_to_uri(p).unwrap(),
                            vec![lsp_types::TextEdit { range: lsp_types::Range::new(lsp_types::Position::new(0, 0), lsp_types::Position::new(0, 3)), new_text: "new".into() }],
                        )
                    })
                    .collect(),
            ),
            ..Default::default()
        }
    }
    fn preview(app: &mut App, paths: &[&Path]) {
        let context = app.refactor_context(fenix_syntax::LanguageId::Rust);
        app.preview_lsp_edit(proposal(paths), context);
        assert!(app.refactor_preview.is_some(), "{:?}", app.status_message.as_ref().map(|m| &m.text));
    }
    #[test]
    fn multi_file_preview_commit_and_logical_undo() {
        let (_dir, mut app, a, b) = fixture();
        preview(&mut app, &[&a, &b]);
        assert!(app.buffer_id_for_lsp_path(&b).is_none());
        assert_eq!(std::fs::read_to_string(&a).unwrap(), "old\n");
        app.commit_refactor();
        for path in [&a, &b] {
            let id = app.buffer_id_for_lsp_path(path).unwrap();
            let ob = app.buffers.get(id).unwrap();
            assert_eq!(ob.buffer.text(), "new\n");
            assert!(ob.buffer.is_dirty());
            assert_eq!(std::fs::read_to_string(path).unwrap(), "old\n");
        }
        app.undo_refactor();
        for path in [&a, &b] {
            let ob = app.buffers.get(app.buffer_id_for_lsp_path(path).unwrap()).unwrap();
            assert_eq!(ob.buffer.text(), "old\n");
            assert!(!ob.buffer.is_dirty());
        }
    }
    #[test]
    fn disk_change_during_preview_rejects_every_file() {
        let (_dir, mut app, a, b) = fixture();
        preview(&mut app, &[&a, &b]);
        std::fs::write(&b, "external\n").unwrap();
        app.commit_refactor();
        assert!(app.refactor_undo.is_none());
        assert_eq!(app.buffers.get(app.buffer_id_for_lsp_path(&a).unwrap()).unwrap().buffer.text(), "old\n");
        assert!(app.buffer_id_for_lsp_path(&b).is_none());
    }
    #[test]
    fn stale_request_and_stale_undo_are_refused() {
        let (_dir, mut app, a, b) = fixture();
        let context = app.refactor_context(fenix_syntax::LanguageId::Rust);
        app.test_insert_str("x");
        app.preview_lsp_edit(proposal(&[&a, &b]), context);
        assert!(app.refactor_preview.is_none());
        let (_dir2, mut app, a, b) = fixture();
        preview(&mut app, &[&a, &b]);
        app.commit_refactor();
        app.test_insert_str("x");
        app.undo_refactor();
        assert_eq!(app.buffers.get(app.buffer_id_for_lsp_path(&b).unwrap()).unwrap().buffer.text(), "new\n");
        assert!(app.status_message.as_ref().unwrap().is_error);
    }
    #[test]
    fn cancel_does_not_open_targets_or_edit_documents() {
        let (_dir, mut app, a, b) = fixture();
        preview(&mut app, &[&a, &b]);
        assert!(app.refactor_key(KeyPress::char('q')));
        assert!(app.refactor_preview.is_none());
        assert_eq!(app.open().buffer.text(), "old\n");
        assert!(app.buffer_id_for_lsp_path(&b).is_none());
    }
    #[test]
    fn multiple_actions_require_selection_then_preview() {
        let (_dir, mut app, a, b) = fixture();
        let context = app.refactor_context(fenix_syntax::LanguageId::Rust);
        let actions = [&a, &b]
            .iter()
            .enumerate()
            .map(|(index, path)| {
                lsp_types::CodeActionOrCommand::CodeAction(lsp_types::CodeAction { title: format!("action {index}"), edit: Some(proposal(&[path])), ..Default::default() })
            })
            .collect();
        app.preview_lsp_actions(actions, context);
        assert!(matches!(app.refactor_preview.as_ref().unwrap().contents, Contents::Actions(..)));
        app.refactor_key(KeyPress::named(FenixNamedKey::Enter));
        assert!(matches!(app.refactor_preview.as_ref().unwrap().contents, Contents::Edits(..)));
        assert_eq!(app.buffers.get(app.buffer_id_for_lsp_path(&a).unwrap()).unwrap().buffer.text(), "old\n");
        app.refactor_key(KeyPress::named(FenixNamedKey::Enter));
        assert_eq!(app.open().buffer.text(), "new\n");
        assert!(app.buffer_id_for_lsp_path(&b).is_none());
    }
    #[test]
    fn formatting_rejects_invalid_batch_and_has_one_undo_step() {
        let (_dir, mut app, a, _) = fixture();
        let id = app.buffer_id_for_lsp_path(&a).unwrap();
        let mut edits = proposal(&[&a]).changes.unwrap().into_values().next().unwrap();
        edits.push(lsp_types::TextEdit { range: lsp_types::Range::new(lsp_types::Position::new(99, 0), lsp_types::Position::new(99, 0)), new_text: "oops".into() });
        app.apply_lsp_text_edits(id, edits);
        assert_eq!(app.open().buffer.text(), "old\n");
        app.apply_lsp_text_edits(id, proposal(&[&a]).changes.unwrap().into_values().next().unwrap());
        let ob = app.buffers.get_mut(id).unwrap();
        assert_eq!(ob.buffer.text(), "new\n");
        assert!(ob.buffer.undo(&mut ob.cursor));
        assert_eq!(ob.buffer.text(), "old\n");
        assert!(!ob.buffer.undo(&mut ob.cursor));
    }
    #[test]
    fn preview_exposes_newline_only_changes() {
        let text = render(&[PlannedDocument { path: "a.txt".into(), before: "a\r\n".into(), after: "a".into() }]);
        assert!(text.contains("-a [CRLF]"));
        assert!(text.contains("+a [no final newline]"));
    }
}
