//! Durable editor state, separate from live process/panel ownership.
use super::*;
use fenix_window::Layout;
use serde::{Deserialize, Serialize};

const VERSION: u32 = 1;
const MAX_BYTES: usize = 64 * 1024 * 1024;

#[derive(Default)]
pub(super) struct State {
    pub(super) path: Option<PathBuf>,
    last: Vec<u8>,
    blocked: bool,
    discard: bool,
    pub(super) error: Option<String>,
    pub(super) pending: Option<(Vec<WorkspaceList>, usize)>,
}

#[derive(Serialize, Deserialize)]
struct Document {
    path: Option<PathBuf>,
    text: Option<String>,
    dirty: bool,
    disk: Option<DiskFingerprint>,
    #[serde(default)]
    conflict: bool,
    #[serde(default)]
    recovery: Option<String>,
    cursor: usize,
}

#[derive(Clone, Serialize, Deserialize)]
struct Pane {
    document: Option<usize>,
    cursor: usize,
    sticky_col: usize,
    scroll_line: usize,
    scroll_col: usize,
}

#[derive(Serialize, Deserialize)]
struct SavedWorkspace {
    name: String,
    layout: Layout<Pane>,
    focused: usize,
}

#[derive(Serialize, Deserialize)]
struct SavedFrame {
    workspaces: Vec<SavedWorkspace>,
    active: usize,
}

#[derive(Serialize, Deserialize)]
struct Session {
    version: u32,
    documents: Vec<Document>,
    frames: Vec<SavedFrame>,
    focused_frame: usize,
}

impl Session {
    fn validate(&self) -> Result<(), String> {
        if self.version != VERSION { return Err("unsupported session version".into()); }
        if self.documents.len() > 4096 || self.frames.is_empty() || self.frames.len() > 16 || self.focused_frame >= self.frames.len() { return Err("invalid session dimensions".into()); }
        for doc in &self.documents {
            if doc.path.as_ref().is_some_and(|p| !p.is_absolute()) { return Err("session document path must be absolute".into()); }
            if doc.dirty && doc.text.is_none() { return Err("unsaved document has no contents".into()); }
        }
        for frame in &self.frames {
            if frame.workspaces.is_empty() || frame.workspaces.len() > 128 || frame.active >= frame.workspaces.len() { return Err("invalid workspace selection".into()); }
            for workspace in &frame.workspaces {
                if workspace.name.len() > 4096 { return Err("workspace name is too long".into()); }
                let tree = WindowTree::from_layout(workspace.layout.clone(), workspace.focused).map_err(str::to_string)?;
                for pane in tree.windows() {
                    if tree.content(pane).unwrap().document.is_some_and(|id| id >= self.documents.len()) { return Err("invalid session document reference".into()); }
                }
            }
        }
        Ok(())
    }
}

/// Drops the panes of a saved layout that can't be restored, promoting
/// the survivors the way closing a window does. `None` when nothing in
/// it can be restored at all.
///
/// A pane whose buffer had no document (every generated panel buffer --
/// see `BufferKind::is_session_owned`) comes back pointed at one shared
/// placeholder, so a whole panel restores as N copies of the start
/// screen instead of the dashboard it was.
///
/// Pruning rather than discarding the whole workspace is what handles
/// the case that actually shows up in a real session file: using the
/// Git dashboard opens a real file into one of its panes, which leaves
/// a workspace of six unrestorable panes and one genuine document. That
/// mix is neither wholly a panel (so the save side's own check lets it
/// through) nor worth restoring intact. Keeping only the document turns
/// it into an ordinary one-pane workspace and loses nothing the user
/// could have got back anyway.
///
/// Judged from the saved data alone, with no need to know which buffer
/// kinds produced it -- which is also what lets it clean up sessions
/// written before any of this was prevented.
fn prune_unrestorable(layout: Layout<Pane>) -> Option<Layout<Pane>> {
    match layout {
        Layout::Leaf(pane) => pane.document.is_some().then_some(Layout::Leaf(pane)),
        Layout::Split { kind, ratio, first, second } => {
            match (prune_unrestorable(*first), prune_unrestorable(*second)) {
                (Some(first), Some(second)) => {
                    Some(Layout::Split { kind, ratio, first: Box::new(first), second: Box::new(second) })
                }
                // The sibling takes the whole space, exactly as it would
                // if the other pane had been closed by hand.
                (Some(only), None) | (None, Some(only)) => Some(only),
                (None, None) => None,
            }
        }
    }
}

/// Where `focused` (an index into a layout's leaves, left to right) ends
/// up once `prune_unrestorable` has removed the undocumented ones --
/// how many survivors precede it, clamped into the pruned list. Without
/// this a restored workspace can open focused on a different pane than
/// the one that was focused when it was saved.
fn remap_focus(layout: &Layout<Pane>, focused: usize) -> usize {
    fn leaves(layout: &Layout<Pane>, out: &mut Vec<bool>) {
        match layout {
            Layout::Leaf(pane) => out.push(pane.document.is_some()),
            Layout::Split { first, second, .. } => {
                leaves(first, out);
                leaves(second, out);
            }
        }
    }
    let mut documented = Vec::new();
    leaves(layout, &mut documented);
    let kept = documented.iter().filter(|&&d| d).count();
    documented[..focused.min(documented.len())].iter().filter(|&&d| d).count().min(kept.saturating_sub(1))
}

impl App {
    fn capture_session(&self) -> Session {
        let mut documents = Vec::new();
        let mut ids = HashMap::new();
        for id in self.buffers.ids_sorted_by_path() {
            let ob = self.buffers.get(id).unwrap();
            if !ob.kind.tracks_unsaved_changes() { continue; }
            if self.session.discard && ob.buffer.path().is_none() && ob.buffer.is_dirty() { continue; }
            let path = ob.buffer.path().map(|p| std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf()));
            let dirty = ob.buffer.is_dirty() && !self.session.discard;
            ids.insert(id, documents.len());
            documents.push(Document {
                conflict: dirty && self.externally_changed.contains(&id),
                recovery: self.unnamed_snapshots.get(&id).and_then(|p| p.file_name()).and_then(|n| n.to_str()).map(str::to_string),
                disk: self.disk_state.get(&id).copied().or_else(|| path.as_ref().and_then(|p| DiskFingerprint::of(p))),
                path: path.clone(), text: (dirty || path.is_none()).then(|| ob.buffer.text()), dirty, cursor: ob.cursor.char_idx,
            });
        }
        let frames = (0..self.frames.len()).map(|frame| {
            let list = if frame == self.active_frame { &self.workspaces } else { &self.frames[frame].as_ref().unwrap().workspaces };
            // Docker/Git/Jira/VNC/PDF/Task/Debug/Terminal sessions each
            // own a live, in-memory struct (`DockerSession`, `GitSession`,
            // ...) that this whole `capture_session`/restore mechanism
            // has no way to reconstruct -- none of it is captured here,
            // and nothing on the restore side ever rebuilds one. Every
            // one of those panels opens in its own dedicated workspace
            // (`self.workspaces.new_workspace(...)`, never mixed into an
            // ordinary file workspace), so restoring *that* workspace's
            // layout unconditionally produced exactly the bug this skips:
            // the right pane shapes, each one silently pointed at a blank
            // placeholder buffer instead of the session it used to show,
            // since `document: None` (nothing in `ids` for a buffer kind
            // that isn't `tracks_unsaved_changes`) falls back to that
            // placeholder on restore. Skipped here instead of only on
            // restore so a broken layout is never written to disk in the
            // first place. Kept (not skipped) in the one edge case where
            // *every* workspace in this frame is one of these -- an empty
            // `workspaces` list would fail `Session::validate`, and a
            // frame that's nothing but ephemeral sessions restoring with
            // its old (already-established-to-be-imperfect) behavior
            // beats inventing a synthetic fallback workspace here.
            let ephemeral: Vec<bool> = list.workspaces.iter().map(|w| self.workspace_is_ephemeral_session(w)).collect();
            let keep_all = ephemeral.iter().all(|&e| e);
            let workspaces = list
                .workspaces
                .iter()
                .enumerate()
                .filter(|&(i, _)| keep_all || !ephemeral[i])
                .map(|(_, workspace)| SavedWorkspace {
                    name: workspace.name.clone(),
                    focused: workspace.windows.windows().iter().position(|id| *id == workspace.windows.focused_id()).unwrap_or(0),
                    layout: workspace.windows.snapshot(|pane, buffer| {
                        let state = workspace.pane_states.get(&pane).copied().unwrap_or_else(|| PaneState::seeded_at(self.buffers.get(*buffer).map(|ob| ob.cursor).unwrap_or(Cursor::at_start())));
                        Pane { document: ids.get(buffer).copied(), cursor: state.cursor.char_idx, sticky_col: state.cursor.sticky_col,
                            scroll_line: state.scroll_line, scroll_col: state.scroll_col }
                    }),
                })
                .collect();
            // Remapped to the filtered list's own indices -- falls back
            // to 0 (not `list.active` unchanged) when the active
            // workspace itself was the one skipped, same as an out-of-
            // range index anywhere else in a freshly built list.
            let active = if keep_all {
                list.active
            } else {
                list.workspaces.iter().enumerate().filter(|&(i, _)| !ephemeral[i]).position(|(i, _)| i == list.active).unwrap_or(0)
            };
            SavedFrame { workspaces, active }
        }).collect();
        Session { version: VERSION, documents, frames, focused_frame: self.focused_frame.min(self.frames.len() - 1) }
    }

    /// Whether every pane in `workspace` shows a buffer kind backed by a
    /// live, in-memory session object this module has no way to save or
    /// rebuild (`Docker`/`Git`/`Jira`/`Vnc`/`Pdf`/`PdfOutline`/
    /// `PdfSearchResults`/`TaskOutput`/`Debug`/`Terminal`) -- see
    /// `capture_session`'s own doc comment for why `capture_session`
    /// skips persisting such a workspace's layout at all. A pane whose
    /// buffer has already been closed out from under it (`self.buffers.
    /// get` returning `None`, which shouldn't normally happen but isn't
    /// this function's job to assume away) counts as *not* ephemeral --
    /// safer to keep an unrecognized pane's layout than to silently drop
    /// it.
    fn workspace_is_ephemeral_session(&self, workspace: &Workspace) -> bool {
        workspace.windows.windows().into_iter().all(|pane| {
            workspace
                .windows
                .content(pane)
                .and_then(|id| self.buffers.get(*id))
                .is_some_and(|ob| ob.kind.is_session_owned())
        })
    }

    pub(super) fn checkpoint_session(&mut self) -> bool {
        let Some(path) = self.session.path.clone() else { return true };
        if self.config.restore_session == Some(false) || self.session.blocked || self.session.pending.is_some() { return false; }
        let result = (|| -> Result<Vec<u8>, String> {
            let saved = self.capture_session();
            saved.validate()?;
            let bytes = serde_json::to_vec(&saved).map_err(|e| e.to_string())?;
            if bytes.len() > MAX_BYTES { return Err("session exceeds 64 MiB; save large unsaved documents first".into()); }
            if bytes != self.session.last {
                if let Some(parent) = path.parent() { std::fs::create_dir_all(parent).map_err(|e| e.to_string())?; }
                fenix_storage::write(&path, &bytes).map_err(|e| e.to_string())?;
            }
            Ok(bytes)
        })();
        match result {
            Ok(bytes) => { self.session.last = bytes; self.session.error = None; true }
            Err(error) => { self.session.error = Some(error.clone()); self.set_error(format!("Session save failed: {error}")); false }
        }
    }

    pub(super) fn restore_session(&mut self, prefer_first_frame: bool) {
        let Some(path) = self.session.path.clone() else { return };
        if self.config.restore_session == Some(false) { return; }
        let result = (|| -> Result<Option<(Session, Option<std::time::SystemTime>)>, String> {
            let meta = match std::fs::metadata(&path) {
                Ok(meta) => meta,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(e) => return Err(e.to_string()),
            };
            if meta.len() > MAX_BYTES as u64 { return Err("session file exceeds 64 MiB".into()); }
            let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
            let saved: Session = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
            saved.validate()?;
            Ok(Some((saved, meta.modified().ok())))
        })();
        match result {
            Ok(Some((saved, modified))) => self.apply_saved_session(saved, prefer_first_frame, modified),
            Ok(None) => {},
            Err(error) => {
                self.session.blocked = true;
                self.session.error = Some(error.clone());
                self.set_error(format!("Session restore failed: {error}; original file retained. :session-save replaces it"));
            }
        }
    }

    fn apply_saved_session(&mut self, saved: Session, prefer_first_frame: bool, modified: Option<std::time::SystemTime>) {
        let mut documents = Vec::new();
        let mut warnings = Vec::new();
        let recovery = self.recovery_dir.as_ref().map(|dir| fenix_recovery::list(dir)).unwrap_or_default();
        for mut doc in saved.documents {
            // Recovery is written before the session checkpoint. A newer snapshot
            // can contain edits from a crash or failed session write in between.
            if let Some(snapshot) = recovery.iter().find(|snapshot| {
                let matches = match (&doc.path, &snapshot.original) {
                    (Some(path), Some(original)) => std::path::absolute(original).ok().as_ref() == Some(path),
                    (None, None) => doc.recovery.is_some() && snapshot.snapshot.file_name().and_then(|n| n.to_str()) == doc.recovery.as_deref(),
                    _ => false,
                };
                matches && modified.is_some_and(|time| std::fs::metadata(&snapshot.snapshot).and_then(|meta| meta.modified()).unwrap_or(snapshot.saved_at) > time)
            }) {
                doc.text = Some(snapshot.contents.clone());
                doc.dirty = true;
            }
            let existing = doc.path.as_ref().and_then(|p| self.buffer_id_for_lsp_path(p));
            let buffer = match doc.path.as_ref() {
                Some(path) => match Buffer::from_path(path) {
                    Ok(buffer) => Some(buffer),
                    Err(error) => {
                        warnings.push(format!("{}: {error}", path.display()));
                        doc.text.as_ref().map(|text| Buffer::from_text(text))
                    }
                },
                None => Some(Buffer::from_text(doc.text.as_deref().unwrap_or(""))),
            };
            let Some(buffer) = buffer else { documents.push(None); continue };
            let id = existing.filter(|_| buffer.path().is_some()).unwrap_or_else(|| self.buffers.insert_loaded_document(buffer));
            self.note_disk_state(id);
            let ob = self.buffers.get_mut(id).unwrap();
            if let Some(text) = &doc.text {
                if ob.buffer.text() != *text {
                    let len = ob.buffer.len_chars();
                    ob.buffer.replace_range(&mut ob.cursor, 0, len, text);
                }
            }
            if doc.dirty { ob.buffer.mark_unsaved(); } else { ob.buffer.mark_saved(); }
            ob.cursor.char_idx = doc.cursor.min(ob.buffer.len_chars());
            if doc.path.is_none() {
                if let Some(snapshot) = recovery.iter().find(|snapshot| snapshot.original.is_none() && snapshot.snapshot.file_name().and_then(|n| n.to_str()) == doc.recovery.as_deref() && Some(&snapshot.contents) == doc.text.as_ref()) {
                    self.unnamed_snapshots.insert(id, snapshot.snapshot.clone());
                }
            }
            if doc.dirty && (doc.conflict || doc.path.as_ref().is_some_and(|p| DiskFingerprint::of(p) != doc.disk)) {
                self.externally_changed.insert(id);
                warnings.push("a restored document changed on disk; review the conflict before saving".into());
            }
            documents.push(Some(id));
        }
        // Version 1 records no document for dashboards and transient panels.
        // Restore those leaves to a usable dashboard, including its actions.
        let dashboard = dashboard::render(self.known_projects.roots(), self.recent_files.paths());
        let placeholder = self.buffers.open_dashboard(&dashboard.text);
        self.dashboard_lines.insert(placeholder, dashboard.lines);
        let mut frames = Vec::new();
        for frame in saved.frames {
            // `capture_session` no longer writes a panel workspace at all
            // (see `workspace_is_ephemeral_session`), but sessions saved
            // before that are already on disk, and one of those restores
            // the Git dashboard's seven-pane shape with every pane
            // showing the same placeholder. Drop those here too, so an
            // existing broken session heals on the next launch instead
            // of needing the user to close six dead panes by hand.
            //
            // Kept when it would empty the frame, mirroring the save
            // side: a frame has to have at least one workspace.
            let pruned: Vec<(SavedWorkspace, Option<SavedWorkspace>)> = frame
                .workspaces
                .into_iter()
                .map(|w| {
                    let focused = remap_focus(&w.layout, w.focused);
                    let kept = prune_unrestorable(w.layout.clone())
                        .map(|layout| SavedWorkspace { name: w.name.clone(), layout, focused });
                    (w, kept)
                })
                .collect();
            // Nothing restorable anywhere in this frame: keep it exactly
            // as saved rather than hand on an empty workspace list,
            // which is not a state the rest of the app is built for.
            // That's also what preserves the deliberate behaviour for a
            // *single* transient pane -- it comes back as the start
            // screen.
            let keep_all = pruned.iter().all(|(_, kept)| kept.is_none());
            let active = pruned[..frame.active.min(pruned.len())].iter().filter(|(_, k)| k.is_some()).count();
            let (kept, active) = if keep_all {
                (pruned.into_iter().map(|(original, _)| original).collect(), frame.active)
            } else {
                let kept: Vec<SavedWorkspace> = pruned.into_iter().filter_map(|(_, kept)| kept).collect();
                let active = active.min(kept.len().saturating_sub(1));
                (kept, active)
            };
            let frame = SavedFrame { workspaces: kept, active };
            let mut workspaces = Vec::new();
            for workspace in frame.workspaces {
                let mut states = Vec::new();
                fn map(layout: Layout<Pane>, docs: &[Option<BufferId>], fallback: BufferId, states: &mut Vec<Pane>) -> Layout<BufferId> {
                    match layout {
                        Layout::Leaf(pane) => { let id = pane.document.and_then(|i| docs[i]).unwrap_or(fallback); states.push(pane); Layout::Leaf(id) }
                        Layout::Split { kind, ratio, first, second } => Layout::Split { kind, ratio, first: Box::new(map(*first, docs, fallback, states)), second: Box::new(map(*second, docs, fallback, states)) },
                    }
                }
                let layout = map(workspace.layout, &documents, placeholder, &mut states);
                let tree = WindowTree::from_layout(layout, workspace.focused).expect("validated layout");
                let mut pane_states = HashMap::new();
                let mut pane_tabs = HashMap::new();
                for (id, pane) in tree.windows().into_iter().zip(states) {
                    pane_tabs.insert(id, vec![*tree.content(id).unwrap_or(&placeholder)]);
                    if tree.content(id) == Some(&placeholder) {
                        pane_states.insert(id, PaneState::seeded_at(Cursor::at_start()));
                        continue;
                    }
                    let buffer = &self.buffers.get(*tree.content(id).unwrap()).unwrap().buffer;
                    let scroll_line = pane.scroll_line.min(buffer.visual_line_count().saturating_sub(1));
                    pane_states.insert(id, PaneState {
                        cursor: Cursor { char_idx: pane.cursor.min(buffer.len_chars()), sticky_col: pane.sticky_col.min(1_000_000) },
                        scroll_line, rendered_scroll: scroll_line as f32, scroll_col: pane.scroll_col.min(1_000_000),
                    });
                }
                workspaces.push(Workspace { name: workspace.name, windows: tree, pane_states, scroll_anims: HashMap::new(), pane_tabs });
            }
            frames.push(WorkspaceList { workspaces, active: frame.active });
        }
        let placeholder_used = frames.iter().any(|frame| frame.workspaces.iter().any(|workspace| workspace.windows.windows().iter().any(|pane| workspace.windows.content(*pane) == Some(&placeholder))));
        if !placeholder_used {
            self.buffers.close(placeholder);
            self.dashboard_lines.remove(&placeholder);
        }
        self.workspaces = frames.remove(0);
        self.session.pending = Some((frames, if prefer_first_frame { 0 } else { saved.focused_frame }));
        self.refresh_project_root();
        self.refresh_all_gutter_hunks();
        self.main_view = MainView::Editor;
        if warnings.is_empty() { self.set_message("restored editor session"); }
        else { self.set_error(format!("Session restored with warnings: {}", warnings.join("; "))); }
    }

    pub(super) fn restore_session_frames(&mut self, event_loop: &ActiveEventLoop) -> bool {
        let Some((frames, focused)) = self.session.pending.take() else { return false };
        if self.config.restore_windows == Some(false) {
            for (index, frame) in frames.into_iter().enumerate() {
                if index + 1 == focused { self.workspaces.active = self.workspaces.workspaces.len() + frame.active; }
                self.workspaces.workspaces.extend(frame.workspaces);
            }
            self.refresh_project_root();
            self.refresh_all_gutter_hunks();
            return true;
        }
        let layouts = self.config.windows.clone();
        if let (Some(window), Some(layout)) = (&self.window, layouts.first()) {
            window.set_outer_position(PhysicalPosition::new(layout.x, layout.y));
            let _ = window.request_inner_size(PhysicalSize::new(layout.width, layout.height));
            window.set_maximized(layout.maximized);
        }
        for (index, frame) in frames.into_iter().enumerate() {
            let placement = layouts.get(index + 1).map(|layout| FramePlacement::from(*layout)).unwrap_or_else(|| self.next_frame_placement(event_loop));
            let before = self.frames.len();
            self.open_frame(event_loop, placement);
            if self.frames.len() > before { self.workspaces = frame; }
            else { self.workspaces.workspaces.extend(frame.workspaces); }
        }
        self.focus_frame(focused.min(self.frames.len() - 1));
        self.focus_active_frame_window();
        self.refresh_project_root();
        self.refresh_all_gutter_hunks();
        true
    }

    pub(crate) fn discard_session_edits(&mut self) -> bool {
        self.session.discard = true;
        let saved = self.config.restore_session == Some(false) || self.session.blocked || self.checkpoint_session();
        if saved {
            for id in self.dirty_tracked_buffer_ids() { self.discard_recovery_for(id); }
        } else { self.session.discard = false; }
        saved
    }

    pub(super) fn save_session_explicit(&mut self) -> bool {
        self.session.blocked = false;
        if self.session.path.is_none() || self.config.restore_session == Some(false) { self.set_error("session persistence is disabled"); return false; }
        self.snapshot_dirty_buffers();
        if self.checkpoint_session() { self.set_message("session saved"); true } else { false }
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!("fenix-session-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
        fn app(&self) -> App {
            let mut app = App::with_file(None);
            app.session.path = Some(self.0.join("session.json"));
            app
        }
        fn restore(&self) -> App {
            let mut app = self.app();
            app.restore_session(false);
            if let Some((frames, focused)) = app.session.pending.take() {
                for frame in frames {
                    let index = app.test_add_frame(app.focused_buffer_id());
                    app.frames[index].as_mut().unwrap().workspaces = frame;
                }
                app.focus_frame(focused);
            }
            app
        }
    }
    impl Drop for Temp { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); } }

    fn dirty(app: &mut App, id: BufferId, text: &str) {
        let ob = app.buffers.get_mut(id).unwrap();
        let len = ob.buffer.len_chars();
        ob.buffer.replace_range(&mut ob.cursor, 0, len, text);
    }

    #[test]
    fn session_restores_dashboard_content_actions_and_resets_transient_scroll() {
        let temp = Temp::new();
        let mut app = temp.app();
        let pane = app.focused_pane_id();
        app.workspaces.active_pane_states_mut().get_mut(&pane).unwrap().scroll_col = 500;
        assert!(app.checkpoint_session());

        let restored = temp.restore();
        let expected = dashboard::render(restored.known_projects.roots(), restored.recent_files.paths());
        assert_eq!(restored.open().kind, BufferKind::Dashboard);
        assert_eq!(restored.open().buffer.text(), expected.text);
        assert!(restored.dashboard_lines.contains_key(&restored.focused_buffer_id()));
        assert_eq!(restored.cursor().char_idx, 0);
        assert_eq!(restored.pane_state(restored.focused_pane_id()).scroll_col, 0);
    }

    #[test]
    fn session_restores_transient_panel_to_dashboard_without_restarting_it() {
        let temp = Temp::new();
        let mut app = temp.app();
        let id = app.focused_buffer_id();
        app.buffers.get_mut(id).unwrap().kind = BufferKind::TaskOutput;
        assert!(app.checkpoint_session());
        let restored = temp.restore();
        assert_eq!(restored.open().kind, BufferKind::Dashboard);
        assert_eq!(restored.open().buffer.text(), dashboard::render(restored.known_projects.roots(), restored.recent_files.paths()).text);
        assert!(restored.dashboard_lines.contains_key(&restored.focused_buffer_id()));
    }

    /// A multi-pane panel workspace must not be written to the session
    /// at all. Its panes hold session-owned buffers that restore can't
    /// rebuild, so saving the layout brings back the right *shape* with
    /// every pane pointed at the same placeholder -- the Git dashboard
    /// coming back as seven identical dashboards.
    ///
    /// The Git panel is the case that regressed: six `Git` panes plus
    /// one `Diff` pane for its Main view, and `Diff` was missing from
    /// the list that decided this, so the whole workspace looked
    /// restorable. Built here with that exact mix rather than a single
    /// kind, so classifying only the obvious ones wouldn't pass.
    #[test]
    fn a_panel_workspace_is_left_out_of_the_session_even_when_its_panes_differ_in_kind() {
        let temp = Temp::new();
        let mut app = temp.app();

        // A real file workspace, so the panel isn't the only one here --
        // the all-ephemeral frame keeps its workspaces deliberately.
        let file = temp.0.join("real.txt");
        std::fs::write(&file, "keep me\n").unwrap();
        app.open_startup_file(&file);
        let file_workspaces = app.workspaces.workspaces.len();

        // A panel workspace shaped like the Git dashboard's.
        let status = app.buffers.open_git("status");
        app.workspaces.new_workspace(status, Cursor::at_start());
        let main = app.buffers.open_diff("diff");
        let main_pane = app.windows_mut().split(SplitKind::Vertical, main);
        app.workspaces.active_pane_states_mut().insert(main_pane, PaneState::seeded_at(Cursor::at_start()));
        assert_eq!(app.workspaces.workspaces.len(), file_workspaces + 1, "panel opened in its own workspace");
        assert_eq!(app.open().kind, BufferKind::Diff, "the Main pane is a Diff buffer, not a Git one");

        assert!(app.checkpoint_session());
        let restored = temp.restore();

        assert_eq!(
            restored.workspaces.workspaces.len(),
            file_workspaces,
            "the panel workspace should have been skipped, not restored as placeholder panes"
        );
        for workspace in &restored.workspaces.workspaces {
            let panes = workspace.windows.windows();
            let kinds: Vec<_> =
                panes.iter().filter_map(|p| workspace.windows.content(*p)).filter_map(|id| restored.buffers.get(*id)).map(|ob| ob.kind).collect();
            assert!(
                !kinds.contains(&BufferKind::Diff) && !kinds.contains(&BufferKind::Git),
                "no session-owned pane should survive a restore, got {kinds:?}"
            );
        }
    }

    /// A session written *before* panel workspaces were excluded is
    /// already on disk for anyone upgrading, and restoring one brings
    /// back a multi-pane panel as that many copies of the start screen.
    /// Restore drops those, so the bad session heals on next launch
    /// rather than leaving six dead panes to close by hand.
    #[test]
    fn a_session_saved_with_placeholder_only_panels_drops_them_on_restore() {
        let temp = Temp::new();
        let file = temp.0.join("real.txt");
        std::fs::write(&file, "keep me\n").unwrap();

        // Hand-built the way the old code would have written it: a real
        // file workspace, plus a panel workspace whose panes all record
        // no document at all.
        let pane = |document| Pane { document, cursor: 0, sticky_col: 0, scroll_line: 0, scroll_col: 0 };
        let split = |first, second| Layout::Split { kind: SplitKind::Vertical, ratio: 0.5, first: Box::new(first), second: Box::new(second) };
        let saved = Session {
            version: VERSION,
            documents: vec![Document {
                path: Some(file.clone()), text: None, dirty: false, disk: DiskFingerprint::of(&file),
                conflict: false, recovery: None, cursor: 0,
            }],
            frames: vec![SavedFrame {
                active: 1,
                workspaces: vec![
                    SavedWorkspace { name: "workspace-1".into(), focused: 0, layout: Layout::Leaf(pane(Some(0))) },
                    SavedWorkspace {
                        name: "workspace-2".into(),
                        focused: 0,
                        layout: split(Layout::Leaf(pane(None)), split(Layout::Leaf(pane(None)), Layout::Leaf(pane(None)))),
                    },
                ],
            }],
            focused_frame: 0,
        };
        saved.validate().expect("the broken shape is still a structurally valid session");
        std::fs::write(temp.0.join("session.json"), serde_json::to_vec(&saved).unwrap()).unwrap();

        let restored = temp.restore();
        assert_eq!(restored.workspaces.workspaces.len(), 1, "the placeholder-only panel workspace should be gone");
        let kept = &restored.workspaces.workspaces[0];
        assert_eq!(kept.windows.windows().len(), 1, "the real file workspace is untouched");
        assert_eq!(restored.workspaces.active, 0, "active index follows the workspace that survived");
    }

    /// The shape a real session file actually ends up in: using the Git
    /// dashboard opens a file into one of its panes, so the workspace is
    /// six unrestorable panes *and* one genuine document. Neither wholly
    /// a panel nor worth restoring intact -- the document survives on its
    /// own, the six placeholders don't.
    #[test]
    fn a_panel_workspace_that_had_a_file_opened_into_it_keeps_only_the_file() {
        let temp = Temp::new();
        let file = temp.0.join("opened.txt");
        std::fs::write(&file, "the one real pane
").unwrap();

        let pane = |document| Pane { document, cursor: 0, sticky_col: 0, scroll_line: 0, scroll_col: 0 };
        let split = |first, second| Layout::Split { kind: SplitKind::Vertical, ratio: 0.5, first: Box::new(first), second: Box::new(second) };
        // Six placeholder panes with the document third from the left,
        // so a passing result can't come from just taking the first or
        // last leaf.
        let panel = split(
            Layout::Leaf(pane(None)),
            split(Layout::Leaf(pane(None)), split(Layout::Leaf(pane(Some(0))), split(Layout::Leaf(pane(None)), Layout::Leaf(pane(None))))),
        );
        let saved = Session {
            version: VERSION,
            documents: vec![Document {
                path: Some(file.clone()), text: None, dirty: false, disk: DiskFingerprint::of(&file),
                conflict: false, recovery: None, cursor: 0,
            }],
            frames: vec![SavedFrame {
                active: 0,
                workspaces: vec![SavedWorkspace { name: "panel".into(), focused: 2, layout: panel }],
            }],
            focused_frame: 0,
        };
        saved.validate().expect("structurally valid");
        std::fs::write(temp.0.join("session.json"), serde_json::to_vec(&saved).unwrap()).unwrap();

        let restored = temp.restore();
        assert_eq!(restored.workspaces.workspaces.len(), 1);
        let kept = &restored.workspaces.workspaces[0];
        assert_eq!(kept.windows.windows().len(), 1, "only the documented pane should survive");
        let id = *kept.windows.content(kept.windows.windows()[0]).unwrap();
        assert_eq!(restored.buffers.get(id).unwrap().buffer.path(), Some(std::path::absolute(&file).unwrap().as_path()));
        assert_eq!(restored.buffers.get(id).unwrap().kind, BufferKind::Text, "the survivor is the real file, not a placeholder");
    }

    #[test]
    fn session_restored_dashboard_recent_file_action_opens_the_document() {
        let temp = Temp::new();
        let mut app = temp.app();
        assert!(app.checkpoint_session());
        let file = temp.0.join("recent.txt");
        std::fs::write(&file, "recent document").unwrap();
        let mut restored = temp.app();
        restored.recent_files.add(file.clone());
        restored.restore_session(false);
        let id = restored.focused_buffer_id();
        let line = restored.dashboard_lines[&id].iter().position(|line| {
            matches!(line.as_ref().and_then(|line| line.entry.as_ref()),
                Some(dashboard::DashboardEntry::RecentFile(path)) if path == &file)
        }).expect("restored dashboard should expose recent-file actions");
        let (buffer, cursor) = restored.focused_buffer_and_cursor_mut();
        cursor.char_idx = buffer.line_start_char(line);
        restored.dashboard_activate_selected();
        assert_eq!(restored.open().buffer.text(), "recent document");
    }

    #[test]
    fn session_restores_unsaved_unicode_shared_panes_workspaces_and_frames() {
        let temp = Temp::new();
        let mut app = temp.app();
        let id = app.buffers.open_scratch();
        dirty(&mut app, id, "hello\n世界\nthird\n");
        let layout = Layout::Split { kind: SplitKind::Vertical, ratio: 0.35,
            first: Box::new(Layout::Leaf(id)), second: Box::new(Layout::Leaf(id)) };
        let tree = WindowTree::from_layout(layout.clone(), 1).unwrap();
        let original_panes = tree.windows();
        let mut workspace = Workspace::new("Writing".into(), tree, Cursor::at_start());
        for pane in &original_panes {
            workspace.pane_states.insert(*pane, PaneState { cursor: Cursor { char_idx: 8, sticky_col: 2 }, scroll_line: 1, rendered_scroll: 1.0, scroll_col: 3 });
        }
        app.workspaces.workspaces = vec![workspace];
        let frame = app.test_add_frame(id);
        app.frames[frame].as_mut().unwrap().workspaces.workspaces[0].name = "Review".into();
        app.focus_frame(frame);
        assert!(app.checkpoint_session());
        let mut restored = temp.restore();
        assert_eq!(restored.focused_frame, 1);
        let restored_id = restored.focused_buffer_id();
        assert_eq!(restored.open().buffer.text(), "hello\n世界\nthird\n");
        assert!(restored.open().buffer.is_dirty());
        restored.focus_frame(0);
        assert_eq!(restored.workspaces.active_name(), "Writing");
        assert_eq!(restored.windows().snapshot(|_, _| id), layout);
        assert_eq!(restored.focused_buffer_id(), restored_id);
        let panes = restored.windows().windows();
        assert_eq!(restored.windows().focused_id(), panes[1]);
        assert!(panes.iter().all(|p| !original_panes.contains(p)));
        assert_eq!(restored.pane_state(panes[1]).cursor.char_idx, 8);
        assert_eq!(restored.pane_state(panes[1]).scroll_line, 1);
        assert_eq!(restored.pane_state(panes[1]).scroll_col, 3);
        assert!(restored.checkpoint_session());
    }

    #[test]
    fn session_drops_an_ephemeral_sessions_own_workspace_but_keeps_a_real_one_beside_it() {
        let temp = Temp::new();
        let mut app = temp.app();
        // A real, ordinary file workspace -- this is the one that should
        // survive a restore untouched.
        let real_id = app.buffers.open_scratch();
        dirty(&mut app, real_id, "real content\n");
        app.workspaces.workspaces[0].name = "Writing".into();
        app.set_pane_content(app.focused_pane_id(), real_id);
        // A second workspace standing in for a Docker/Git-style session:
        // its own dedicated workspace, its one pane's buffer tagged with
        // a kind `capture_session` now treats as ephemeral.
        let docker_id = app.buffers.open_scratch();
        app.buffers.get_mut(docker_id).unwrap().kind = BufferKind::Docker;
        let tree = WindowTree::new(docker_id);
        let workspace = Workspace::new("docker".into(), tree, Cursor::at_start());
        app.workspaces.workspaces.push(workspace);
        app.workspaces.active = 1;

        assert!(app.checkpoint_session());
        let mut restored = temp.restore();

        // The ephemeral workspace is gone entirely -- not restored with
        // a placeholder in its old shape, just not there.
        assert_eq!(restored.workspaces.workspaces.len(), 1);
        assert_eq!(restored.workspaces.active, 0);
        assert_eq!(restored.workspaces.active_name(), "Writing");
        assert_eq!(restored.open().buffer.text(), "real content\n");
        assert!(restored.open().buffer.is_dirty());
        // Restoring again still round-trips cleanly -- the fallback
        // logic didn't leave `active`/`workspaces` in some state that
        // can't itself be saved and restored a second time.
        restored.focus_frame(0);
        assert!(restored.checkpoint_session());
    }

    #[test]
    fn session_conflict_survives_repeated_restarts_without_writing_disk() {
        let temp = Temp::new();
        let path = temp.0.join("document.txt");
        std::fs::write(&path, "original").unwrap();
        let mut app = temp.app();
        app.open_startup_file(&path);
        let id = app.focused_buffer_id();
        dirty(&mut app, id, "my unsaved changes");
        assert!(app.checkpoint_session());
        std::fs::write(&path, "external changes with different length").unwrap();
        let mut restored = temp.restore();
        assert_eq!(restored.open().buffer.text(), "my unsaved changes");
        assert!(restored.externally_changed.contains(&restored.focused_buffer_id()));
        assert!(restored.checkpoint_session());
        let restored_again = temp.restore();
        assert!(restored_again.externally_changed.contains(&restored_again.focused_buffer_id()));
        assert_eq!(std::fs::read_to_string(path).unwrap(), "external changes with different length");
    }

    #[test]
    fn session_missing_dirty_file_becomes_unnamed_and_clean_file_is_skipped() {
        let temp = Temp::new();
        let dirty_path = temp.0.join("dirty.txt");
        let clean_path = temp.0.join("clean.txt");
        std::fs::write(&dirty_path, "old").unwrap();
        std::fs::write(&clean_path, "clean").unwrap();
        let mut app = temp.app();
        app.open_startup_file(&clean_path);
        app.open_startup_file(&dirty_path);
        let id = app.focused_buffer_id();
        dirty(&mut app, id, "rescue this");
        assert!(app.checkpoint_session());
        std::fs::remove_file(&dirty_path).unwrap();
        std::fs::remove_file(&clean_path).unwrap();
        let restored = temp.restore();
        assert_eq!(restored.open().buffer.text(), "rescue this");
        assert!(restored.open().buffer.path().is_none());
        assert!(restored.open().buffer.is_dirty());
        assert!(restored.buffer_id_for_lsp_path(&clean_path).is_none());
        assert!(!dirty_path.exists());
    }

    #[test]
    fn session_discard_removes_unsaved_payload_and_recovery() {
        let temp = Temp::new();
        let mut app = temp.app();
        app.recovery_dir = Some(temp.0.join("recovery"));
        let id = app.buffers.open_scratch();
        dirty(&mut app, id, "discard me");
        app.snapshot_dirty_buffers();
        assert!(app.checkpoint_session());
        let mut restored = temp.app();
        restored.recovery_dir = app.recovery_dir.clone();
        restored.restore_session(false);
        restored.session.pending.take();
        assert_eq!(restored.unnamed_snapshots.len(), 1);
        assert!(restored.discard_session_edits());
        assert!(fenix_recovery::list(restored.recovery_dir.as_ref().unwrap()).is_empty());
        let again = temp.restore();
        assert!(again.dirty_tracked_buffer_ids().is_empty());
        assert!(!std::fs::read_to_string(temp.0.join("session.json")).unwrap().contains("discard me"));
    }

    #[test]
    fn session_invalid_data_is_preserved_until_explicit_save() {
        let temp = Temp::new();
        let path = temp.0.join("session.json");
        for invalid in ["not json", "{\"version\":999,\"documents\":[],\"frames\":[],\"focused_frame\":0}"] {
            std::fs::write(&path, invalid).unwrap();
            let mut app = temp.restore();
            assert!(app.session.blocked);
            assert!(!app.checkpoint_session());
            assert_eq!(std::fs::read_to_string(&path).unwrap(), invalid);
            assert!(app.save_session_explicit());
            assert!(serde_json::from_slice::<Session>(&std::fs::read(&path).unwrap()).unwrap().validate().is_ok());
        }
    }

    #[test]
    fn session_disabled_does_not_read_or_replace_existing_file() {
        let temp = Temp::new();
        let path = temp.0.join("session.json");
        std::fs::write(&path, "untouched").unwrap();
        let mut app = temp.app();
        app.config.restore_session = Some(false);
        app.restore_session(false);
        assert!(app.session.error.is_none());
        assert!(!app.checkpoint_session());
        assert!(!app.save_session_explicit());
        assert_eq!(std::fs::read_to_string(path).unwrap(), "untouched");
    }

    #[test]
    fn session_failed_checkpoint_keeps_edits_and_can_retry() {
        let temp = Temp::new();
        let mut app = temp.app();
        let id = app.buffers.open_scratch();
        dirty(&mut app, id, "keep this");
        std::fs::create_dir(temp.0.join("session.json")).unwrap();
        assert!(!app.discard_session_edits());
        assert!(!app.session.discard);
        assert!(app.session.error.is_some());
        assert!(app.buffers.get(id).unwrap().buffer.is_dirty());
        std::fs::remove_dir(temp.0.join("session.json")).unwrap();
        assert!(app.save_session_explicit());
        assert!(app.session.error.is_none());
        let restored = temp.restore();
        assert_eq!(restored.dirty_tracked_buffer_ids().len(), 1);
    }

    #[test]
    fn session_rejects_bad_document_references() {
        let temp = Temp::new();
        let app = temp.app();
        let mut saved = app.capture_session();
        saved.frames[0].workspaces[0].layout = Layout::Leaf(Pane { document: Some(999), cursor: 0, sticky_col: 0, scroll_line: 0, scroll_col: 0 });
        assert!(saved.validate().is_err());
    }

    #[test]
    fn session_prefers_newer_recovery_after_interrupted_checkpoint() {
        let temp = Temp::new();
        let path = temp.0.join("document.txt");
        std::fs::write(&path, "disk").unwrap();
        let mut app = temp.app();
        app.open_startup_file(&path);
        let id = app.focused_buffer_id();
        dirty(&mut app, id, "older edits");
        assert!(app.checkpoint_session());
        let file = std::fs::File::options().write(true).open(temp.0.join("session.json")).unwrap();
        file.set_times(std::fs::FileTimes::new().set_modified(std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(1))).unwrap();
        drop(file);
        let recovery = temp.0.join("recovery");
        fenix_recovery::write(&recovery, &path, "newest edits").unwrap();
        let mut restored = temp.app();
        restored.recovery_dir = Some(recovery.clone());
        restored.restore_session(false);
        restored.session.pending.take();
        assert_eq!(restored.open().buffer.text(), "newest edits");
        assert!(restored.open().buffer.is_dirty());
        restored.snapshot_dirty_buffers();
        assert_eq!(fenix_recovery::list(&recovery)[0].contents, "newest edits");
        assert_eq!(std::fs::read_to_string(path).unwrap(), "disk");
    }

    #[test]
    fn session_clean_files_reload_disk_and_clamp_saved_positions() {
        let temp = Temp::new();
        let path = temp.0.join("document.txt");
        std::fs::write(&path, "a longer document\nsecond line\n").unwrap();
        let mut app = temp.app();
        app.open_startup_file(&path);
        let pane = app.focused_pane_id();
        app.pane_state_mut(pane).cursor.char_idx = 20;
        app.pane_state_mut(pane).scroll_line = 1;
        assert!(app.checkpoint_session());
        std::fs::write(&path, "hi").unwrap();
        let restored = temp.restore();
        assert_eq!(restored.open().buffer.text(), "hi");
        assert!(!restored.open().buffer.is_dirty());
        assert_eq!(restored.cursor().char_idx, 2);
        assert_eq!(restored.pane_state(restored.focused_pane_id()).scroll_line, 0);
    }

    #[test]
    fn session_cli_file_takes_focus_without_losing_restored_edits() {
        let temp = Temp::new();
        let path = temp.0.join("requested.txt");
        std::fs::write(&path, "requested").unwrap();
        let mut app = temp.app();
        let id = app.buffers.open_scratch();
        dirty(&mut app, id, "keep hidden work");
        let frame = app.test_add_frame(id);
        app.focus_frame(frame);
        assert!(app.checkpoint_session());
        let mut restored = temp.app();
        restored.restore_session(true);
        assert_eq!(restored.session.pending.as_ref().unwrap().1, 0);
        restored.open_startup_file(&path);
        assert_eq!(restored.open().buffer.text(), "requested");
        assert_eq!(restored.dirty_tracked_buffer_ids().len(), 1);
    }


    #[test]
    fn session_cli_path_alias_reuses_unsaved_restored_document() {
        let temp = Temp::new();
        let path = temp.0.join("document.txt");
        std::fs::write(&path, "disk").unwrap();
        let mut app = temp.app();
        app.open_startup_file(&path);
        let id = app.focused_buffer_id();
        dirty(&mut app, id, "unsaved");
        assert!(app.checkpoint_session());
        let mut restored = temp.restore();
        let restored_id = restored.focused_buffer_id();
        restored.open_startup_file(&temp.0.join(".").join("document.txt"));
        assert_eq!(restored.focused_buffer_id(), restored_id);
        assert_eq!(restored.open().buffer.text(), "unsaved");
        assert!(restored.open().buffer.is_dirty());
    }

}
