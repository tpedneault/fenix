//! Inlay hints (`textDocument/inlayHint`): a type after a name, a
//! parameter's name before an argument, asked of the first of a buffer's
//! language servers that offers them, for the lines in view (and a
//! margin), and drawn dimmed in the line as decorations
//! (`crate::decorations`, the debugger's values' way): they take room,
//! so the caret, extra carets, selections and soft wrap all land where
//! the text went, and they move with your typing until the next answer.
//!
//! Asked when a buffer is shown, after typing pauses, when the view
//! moves past what was asked, and when the server says they changed
//! (`workspace/inlayHint/refresh`). `lsp.inlay_hints` (on) and
//! `lsp.inlay_hints_off` (per language) turn them off; `SPC t i` flips
//! the setting.

use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::{App, BufferId, LspKey, PendingLspRequest};
use crate::decorations::{Decoration, Look};

/// Whose decorations they are, among an extension's and the debugger's.
pub(super) const INLAY_OWNER: &str = "fenix.inlay_hints";
/// How long typing pauses before hints are asked again.
const INLAY_PAUSE: Duration = Duration::from_millis(300);
/// How long an ask may go unanswered before it's cancelled.
const INLAY_TIMEOUT: Duration = Duration::from_secs(10);
/// The longest wait before asking again after a refusal.
const INLAY_MAX_RETRY: Duration = Duration::from_secs(4);
/// Lines asked for above and below those in view.
const MARGIN: usize = 60;

/// An ask out.
#[derive(Debug, Clone)]
pub(super) struct Asked {
    pub id: i64,
    at: Instant,
    edit_count: u64,
    lines: Range<usize>,
}

/// One buffer's hints, from one server.
#[derive(Debug)]
pub(super) struct InlayDoc {
    pub key: LspKey,
    /// The document's canonical path, as its server knows it.
    pub path: PathBuf,
    /// The lines each pane shows of it, as last drawn.
    views: HashMap<fenix_window::WindowId, Range<usize>>,
    pub asked: Option<Asked>,
    /// The edit count and lines the hints drawn were answered for.
    answered: Option<(u64, Range<usize>)>,
    /// The edit count last seen: a change sets the pause going.
    seen_edit: Option<u64>,
    due: Option<Instant>,
    /// The server said its hints changed.
    stale: bool,
    retry: Duration,
    refusals: u32,
    timed_out: bool,
}

impl InlayDoc {
    fn new(key: LspKey, path: PathBuf) -> Self {
        InlayDoc { key, path, views: HashMap::new(), asked: None, answered: None, seen_edit: None, due: None, stale: false, retry: INLAY_PAUSE, refusals: 0, timed_out: false }
    }

    /// The lines wanted: every view's, with the margin, in `line_count`.
    fn wanted(&self, line_count: usize) -> Option<Range<usize>> {
        let start = self.views.values().map(|r| r.start).min()?;
        let end = self.views.values().map(|r| r.end).max()?;
        Some(start.saturating_sub(MARGIN)..(end + MARGIN).min(line_count.max(1)))
    }
}

impl App {
    /// `lsp.inlay_hints`.
    pub(super) fn inlay_hints_on(&self) -> bool {
        self.config.bool_value("lsp.inlay_hints").unwrap_or(true)
    }

    /// Whether buffer `id`'s hints may be asked for and drawn: the setting
    /// on, and its language not in `lsp.inlay_hints_off`.
    pub(super) fn inlay_hints_wanted(&self, id: BufferId) -> bool {
        if !self.inlay_hints_on() {
            return false;
        }
        let Some(name) = self.buffer_language_name(id) else { return true };
        !self.config.list_value("lsp.inlay_hints_off").iter().any(|l| l.trim().eq_ignore_ascii_case(&name))
    }

    /// After a buffer's sync (`sync_lsp_for`): the server its hints are
    /// asked of, kept; none wanted or offered, what it had goes.
    pub(super) fn inlay_targets(&mut self, buffer: Option<BufferId>, path: &Path, canonical: &Path) {
        let Some(buffer) = buffer else { return };
        let key = self
            .inlay_hints_wanted(buffer)
            .then(|| self.cached_lsp_keys(path).into_iter().find(|key| self.lsp_sessions.get(key).and_then(|s| s.capabilities.as_ref()).is_some_and(fenix_lsp::inlay::offers)))
            .flatten();
        match key {
            Some(key) => {
                let same = self.inlay.get(&buffer).is_some_and(|d| d.key == key && d.path == canonical);
                if !same {
                    self.clear_inlay_hints(buffer);
                    self.inlay.insert(buffer, InlayDoc::new(key, canonical.to_path_buf()));
                }
            }
            None => self.drop_inlay_doc(buffer),
        }
    }

    /// Buffer `id`'s hints let go, and any ask out cancelled.
    fn drop_inlay_doc(&mut self, id: BufferId) {
        if let Some(doc) = self.inlay.remove(&id) {
            if let (Some(asked), Some(session)) = (doc.asked, self.lsp_sessions.get_mut(&doc.key)) {
                let _ = session.client.notify::<lsp_types::notification::Cancel>(lsp_types::CancelParams { id: lsp_types::NumberOrString::Number(asked.id as i32) });
                session.pending.remove(&asked.id);
            }
        }
        self.clear_inlay_hints(id);
    }

    /// Buffer `id`'s hints off the screen.
    fn clear_inlay_hints(&mut self, id: BufferId) {
        if let Some(sets) = self.wasm.decorations.get_mut(&id) {
            let before = sets.len();
            sets.retain(|(who, _)| who != INLAY_OWNER);
            if sets.len() != before {
                self.wasm.decorations_rev += 1;
            }
            if sets.is_empty() {
                self.wasm.decorations.remove(&id);
            }
        }
    }

    /// What pane `pane` shows of buffer `id` (`runs`, its lines), as it's
    /// drawn.
    pub(super) fn note_inlay_view(&mut self, id: BufferId, pane: fenix_window::WindowId, runs: &[Range<usize>]) {
        let Some(doc) = self.inlay.get_mut(&id) else { return };
        let (Some(first), Some(last)) = (runs.iter().map(|r| r.start).min(), runs.iter().map(|r| r.end).max()) else { return };
        doc.views.insert(pane, first..last);
    }

    /// Each frame, and when the loop wakes: what's due asked, what's out
    /// too long cancelled, and what closed buffers or stopped servers had
    /// let go.
    pub(super) fn ask_due_inlay_hints(&mut self) {
        if self.inlay.is_empty() {
            return;
        }
        let gone: Vec<BufferId> = self
            .inlay
            .iter()
            .filter(|(b, d)| !self.inlay_hints_wanted(**b) || self.buffers.get(**b).is_none() || !self.lsp_sessions.contains_key(&d.key))
            .map(|(b, _)| *b)
            .collect();
        for buffer in gone {
            self.drop_inlay_doc(buffer);
        }
        let now = Instant::now();
        let buffers: Vec<BufferId> = self.inlay.keys().copied().collect();
        for buffer in buffers {
            let Some(ob) = self.buffers.get(buffer) else { continue };
            let (edit_count, line_count) = (ob.buffer.edit_count(), ob.buffer.line_count());
            let Some(doc) = self.inlay.get_mut(&buffer) else { continue };
            let Some(session) = self.lsp_sessions.get_mut(&doc.key) else { continue };
            // Out too long: cancelled, and asked once more.
            if let Some(asked) = doc.asked.as_ref().filter(|a| now.duration_since(a.at) >= INLAY_TIMEOUT) {
                let _ = session.client.notify::<lsp_types::notification::Cancel>(lsp_types::CancelParams { id: lsp_types::NumberOrString::Number(asked.id as i32) });
                session.pending.remove(&asked.id);
                doc.asked = None;
                doc.due = (!doc.timed_out).then_some(now);
                doc.timed_out = true;
            }
            if doc.seen_edit != Some(edit_count) {
                // The first look asks now; an edit after the pause.
                doc.due = Some(if doc.seen_edit.is_none() { now } else { now + INLAY_PAUSE });
                doc.seen_edit = Some(edit_count);
                doc.timed_out = false;
                doc.refusals = 0;
                doc.retry = INLAY_PAUSE;
            }
            // Only of the text the server has.
            if session.open_documents.get(&doc.path).map(|&(count, _)| count) != Some(edit_count) || doc.asked.is_some() {
                continue;
            }
            let Some(lines) = doc.wanted(line_count) else { continue };
            let covers = |(count, had): &(u64, Range<usize>)| *count == edit_count && had.start <= lines.start && had.end >= lines.end;
            if !doc.stale && doc.answered.as_ref().is_some_and(covers) {
                continue;
            }
            // The view moved past what was asked of this text: now, not
            // after a pause; else when due, or told to.
            let moved = doc.answered.as_ref().is_some_and(|(count, _)| *count == edit_count);
            if !(doc.stale || moved || doc.due.is_some_and(|due| due <= now)) {
                continue;
            }
            let Some(uri) = fenix_lsp::path_to_uri(&doc.path) else { continue };
            let params = fenix_lsp::inlay::params(uri, lines.clone(), line_count);
            if let Ok(id) = session.client.request::<lsp_types::request::InlayHintRequest>(params) {
                doc.asked = Some(Asked { id, at: now, edit_count, lines });
                doc.due = None;
                doc.stale = false;
                session.pending.insert(id, PendingLspRequest::InlayHint { buffer, request: id });
            }
        }
    }

    /// When the loop next has to wake for hints.
    pub(super) fn inlay_wake(&self) -> Option<Instant> {
        self.inlay
            .values()
            .filter_map(|d| match &d.asked {
                Some(asked) => Some(asked.at + INLAY_TIMEOUT),
                None => d.due,
            })
            .min()
    }

    /// `workspace/inlayHint/refresh`: `key`'s documents asked again.
    pub(super) fn refresh_inlay_hints(&mut self, key: &LspKey) {
        for doc in self.inlay.values_mut().filter(|d| d.key == *key) {
            doc.stale = true;
            doc.refusals = 0;
        }
        self.request_redraw_all();
    }

    /// An answer to `PendingLspRequest::InlayHint`: drawn when the text is
    /// still what was asked of, else asked again.
    pub(super) fn inlay_hints_answered(&mut self, key: &LspKey, buffer: BufferId, request: i64, result: Result<serde_json::Value, fenix_lsp::ResponseError>) {
        let Some(doc) = self.inlay.get_mut(&buffer).filter(|d| d.key == *key) else { return };
        let Some(asked) = doc.asked.take_if(|a| a.id == request) else { return };
        let now = Instant::now();
        let hints = match result {
            Ok(value) => serde_json::from_value::<Option<Vec<lsp_types::InlayHint>>>(value).ok().flatten().unwrap_or_default(),
            // Cancelled or modified: asked again later, a few times.
            Err(e) if (-32802..=-32800).contains(&e.code) => {
                doc.refusals += 1;
                if doc.refusals < super::code_lens::MAX_REFUSALS {
                    doc.due = Some(now + doc.retry);
                    doc.retry = (doc.retry * 2).min(INLAY_MAX_RETRY);
                }
                return;
            }
            Err(_) => return,
        };
        let Some(ob) = self.buffers.get_mut(buffer) else { return };
        // Edited since: of other text.
        if ob.buffer.edit_count() != asked.edit_count {
            doc.due = Some(doc.due.unwrap_or(now));
            return;
        }
        doc.answered = Some((asked.edit_count, asked.lines));
        let decorations: Vec<Decoration> = fenix_lsp::inlay::place(&hints, ob.buffer.rope())
            .into_iter()
            .map(|p| Decoration { start: p.at, end: p.at, look: Look::Text { text: p.text, role: crate::page::Role::Muted, after: p.before_text }, id: None })
            .collect();
        // They move with the edits from here on, as an extension's do; the
        // others are first moved by the edits not yet applied to them.
        ob.buffer.track_anchors();
        let edits = ob.buffer.take_anchor_edits();
        self.wasm.decorations_rev += 1;
        let sets = self.wasm.decorations.entry(buffer).or_default();
        for (_, set) in sets.iter_mut() {
            crate::decorations::shift(set, &edits);
        }
        sets.retain(|(who, _)| who != INLAY_OWNER);
        if !decorations.is_empty() {
            sets.push((INLAY_OWNER.to_string(), decorations));
        }
        if sets.is_empty() {
            self.wasm.decorations.remove(&buffer);
        }
        self.request_redraw_all();
    }

    /// `SPC t i`: inlay hints on or off, everywhere, and saved.
    pub(crate) fn toggle_inlay_hints(&mut self) {
        let on = !self.inlay_hints_on();
        self.config.set_value("lsp.inlay_hints", Some(fenix_config::Value::Bool(on)));
        if let Err(err) = self.config.save() {
            self.set_error(format!("couldn't save settings.toml: {err}"));
            return;
        }
        if on {
            // Asked at each file's next sync.
            self.lsp_frame_synced = None;
            self.set_message("Inlay hints on");
        } else {
            let buffers: Vec<BufferId> = self.inlay.keys().copied().collect();
            for buffer in buffers {
                self.drop_inlay_doc(buffer);
            }
            self.set_message("Inlay hints off");
        }
        self.request_redraw_all();
    }

    /// The hints drawn in buffer `id`: (char offset, text), in order.
    #[cfg(test)]
    pub(super) fn inlay_hints_drawn(&self, id: BufferId) -> Vec<(usize, String)> {
        self.wasm
            .decorations
            .get(&id)
            .into_iter()
            .flatten()
            .filter(|(who, _)| who == INLAY_OWNER)
            .flat_map(|(_, set)| set.iter())
            .filter_map(|d| match &d.look {
                Look::Text { text, .. } => Some((d.start, text.clone())),
                _ => None,
            })
            .collect()
    }
}
