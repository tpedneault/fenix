//! Code lens (`docs/plans/code-lens.md`): a document's lenses asked of
//! every one of its language servers that offers them, kept as the
//! servers sent them, moved by line with each edit sent, resolved as
//! their lines come into view, and laid out as one row above each line
//! that has some (`App::set_virtual_rows`).
//!
//! As with semantic tokens (`semantic.rs`), only a document shown in a
//! buffer is asked for, and what's held goes when the buffer closes.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use fenix_lsp::code_lens::{self as lens, LensRow};

use super::semantic::SemanticSync as LensSync;
use super::{App, BufferId, LspKey, LspSession, PendingLspRequest};

/// How long typing pauses before a document's lenses are asked again:
/// slower to work out than tokens, and less pressing.
pub(super) const LENS_PAUSE: Duration = Duration::from_millis(500);
/// How long an ask may go unanswered before it's cancelled.
const LENS_TIMEOUT: Duration = Duration::from_secs(10);
/// The longest wait before asking again after a refusal.
const LENS_MAX_RETRY: Duration = Duration::from_secs(4);
/// How many resolves a server may have out at once.
const MAX_RESOLVING: usize = 16;

/// One document's lenses from one server (`LspSession::lenses`).
#[derive(Debug)]
pub(super) struct LensDoc {
    /// The buffer it's shown in.
    pub buffer: BufferId,
    /// As the server sent them, moved by line with each edit since.
    pub lenses: Vec<lsp_types::CodeLens>,
    /// Counts answers, so a resolve asked of an earlier one is let go.
    generation: u64,
    /// An ask is out: its request and since when.
    pub(super) asked: Option<(i64, Instant)>,
    timed_out: bool,
    pub due: Option<Instant>,
    retry: Duration,
    /// The server said its lenses changed while this wasn't drawn.
    stale: bool,
    /// The lenses (by index) a resolve is out for.
    resolving: HashSet<usize>,
}

impl LensDoc {
    fn new(buffer: BufferId) -> Self {
        LensDoc { buffer, lenses: Vec::new(), generation: 0, asked: None, timed_out: false, due: None, retry: LENS_PAUSE, stale: false, resolving: HashSet::new() }
    }
}

/// What a lens on a row runs: its server and which of its lenses.
pub(super) type LensTag = (LspKey, usize);

/// Whether `session` offers lenses.
pub(super) fn offers(session: &LspSession) -> bool {
    session.capabilities.as_ref().and_then(lens::provider).is_some()
}

/// After `sync_lsp_document` sent `canonical`'s text: on opening (or a
/// buffer newly shown), an ask due now; on a change from `old` to `new`,
/// the lenses moved by line and an ask due after the pause.
pub(super) fn after_sync(session: &mut LspSession, canonical: &Path, change: Option<&str>, new: &str, sync: LensSync) {
    let buffer = match sync {
        LensSync::Drop => {
            session.lenses.remove(canonical);
            return;
        }
        LensSync::Follow => None,
        LensSync::Ask(buffer) => Some(buffer),
    };
    let held = session.lenses.contains_key(canonical);
    let doc = match (buffer, held) {
        (None, false) => return,
        (None, true) => session.lenses.get_mut(canonical).expect("held"),
        (Some(b), _) => session.lenses.entry(canonical.to_path_buf()).or_insert_with(|| LensDoc::new(b)),
    };
    let now = Instant::now();
    let newly_shown = buffer.is_some_and(|b| !held || b != doc.buffer);
    if let Some(buffer) = buffer {
        doc.buffer = buffer;
    }
    match change {
        Some(old) => {
            lens::shift_lines(&mut doc.lenses, lens::changed_lines(old, new));
            doc.timed_out = false;
            doc.retry = LENS_PAUSE;
            doc.due = Some(if newly_shown { now } else { now + LENS_PAUSE });
        }
        None if buffer.is_some() => doc.due = Some(now),
        None => {}
    }
}

/// Sends `canonical`'s ask to `session` if one is due and none is out.
fn ask(session: &mut LspSession, canonical: &Path, now: Instant) {
    let Some(doc) = session.lenses.get_mut(canonical) else { return };
    if doc.asked.is_some() || doc.due.is_none_or(|due| due > now) {
        return;
    }
    doc.due = None;
    let (Some(&(edit_count, _)), Some(uri)) = (session.open_documents.get(canonical), fenix_lsp::path_to_uri(canonical)) else { return };
    let params = lsp_types::CodeLensParams {
        text_document: lsp_types::TextDocumentIdentifier { uri },
        work_done_progress_params: Default::default(),
        partial_result_params: Default::default(),
    };
    if let Ok(id) = session.client.request::<lsp_types::request::CodeLensRequest>(params) {
        doc.asked = Some((id, now));
        session.pending.insert(id, PendingLspRequest::CodeLens { path: canonical.to_path_buf(), edit_count, request: id });
    }
}

/// An ask out longer than `LENS_TIMEOUT`: cancelled, and asked once more;
/// a second that times out waits for an edit.
fn time_out(session: &mut LspSession, canonical: &Path, now: Instant) {
    let Some(doc) = session.lenses.get_mut(canonical) else { return };
    let Some((id, _)) = doc.asked.filter(|&(_, at)| now.duration_since(at) >= LENS_TIMEOUT) else { return };
    let _ = session.client.notify::<lsp_types::notification::Cancel>(lsp_types::CancelParams { id: lsp_types::NumberOrString::Number(id as i32) });
    session.pending.remove(&id);
    doc.asked = None;
    doc.due = (!doc.timed_out).then_some(now);
    doc.timed_out = true;
}

impl App {
    /// `lsp.code_lens`.
    pub(super) fn code_lens_on(&self) -> bool {
        self.config.bool_value("lsp.code_lens").unwrap_or(false)
    }

    /// Whether buffer `id`'s lenses may be asked for and drawn: the
    /// setting on, and its language not in `lsp.code_lens_off`.
    pub(super) fn code_lens_wanted(&self, id: BufferId) -> bool {
        if !self.code_lens_on() {
            return false;
        }
        let Some(name) = self.buffer_language_name(id) else { return true };
        !self.config.list_value("lsp.code_lens_off").iter().any(|l| l.trim().eq_ignore_ascii_case(&name))
    }

    /// Those of `path`'s servers asked for its lenses: every one that
    /// offers them, in the order its servers are.
    pub(super) fn lens_servers(&self, path: &Path) -> Vec<LspKey> {
        self.cached_lsp_keys(path).into_iter().filter(|key| self.lsp_sessions.get(key).is_some_and(offers)).collect()
    }

    /// Each frame, and when the loop wakes: asks due sent, those out too
    /// long cancelled, what closed buffers held let go, and the rows of a
    /// buffer whose servers went laid out again.
    pub(super) fn ask_due_lenses(&mut self) {
        if !self.code_lens_on() && !self.lens_of.is_empty() {
            let buffers: Vec<BufferId> = self.lens_of.keys().copied().collect();
            self.lens_of.clear();
            for session in self.lsp_sessions.values_mut() {
                session.lenses.clear();
            }
            for buffer in buffers {
                self.lay_out_lenses(buffer);
            }
            return;
        }
        let now = Instant::now();
        let gone: Vec<BufferId> = self.lens_of.iter().filter(|(b, (keys, _))| self.buffers.get(**b).is_none() || keys.iter().any(|k| !self.lsp_sessions.contains_key(k))).map(|(b, _)| *b).collect();
        for buffer in gone {
            if self.buffers.get(buffer).is_none() {
                self.lens_of.remove(&buffer);
                self.lens_rows.remove(&buffer);
            } else if let Some((keys, _)) = self.lens_of.get_mut(&buffer) {
                // A server stopped: its lenses with it, the view kept.
                let running: Vec<LspKey> = keys.iter().filter(|k| self.lsp_sessions.contains_key(*k)).cloned().collect();
                *keys = running;
                self.lay_out_lenses(buffer);
            }
        }
        let of = &self.lens_of;
        for (key, session) in self.lsp_sessions.iter_mut() {
            if session.lenses.is_empty() {
                continue;
            }
            session.lenses.retain(|path, d| of.get(&d.buffer).is_some_and(|(keys, p)| p == path && keys.contains(key)));
            let paths: Vec<PathBuf> = session.lenses.keys().cloned().collect();
            for path in paths {
                time_out(session, &path, now);
                ask(session, &path, now);
            }
        }
    }

    /// When the loop next has to wake for lenses.
    pub(super) fn lens_wake(&self) -> Option<Instant> {
        self.lsp_sessions
            .values()
            .flat_map(|s| s.lenses.values())
            .filter_map(|d| match d.asked {
                Some((_, at)) => Some(at + LENS_TIMEOUT),
                None => d.due,
            })
            .min()
    }

    /// `workspace/codeLens/refresh`: `key`'s documents asked again as
    /// they're drawn.
    pub(super) fn refresh_lenses(&mut self, key: &LspKey) {
        let Some(session) = self.lsp_sessions.get_mut(key) else { return };
        for doc in session.lenses.values_mut() {
            doc.stale = true;
        }
        self.request_redraw_all();
    }

    /// After a buffer's sync (`sync_lsp_for`): which of its servers are
    /// asked for its lenses, kept for laying them out.
    pub(super) fn lens_targets(&mut self, buffer: Option<BufferId>, path: &Path, canonical: &Path) -> Option<(BufferId, Vec<LspKey>)> {
        let buffer = buffer?;
        if !self.code_lens_wanted(buffer) {
            if self.lens_of.remove(&buffer).is_some() {
                self.lay_out_lenses(buffer);
            }
            return Some((buffer, Vec::new()));
        }
        let keys = self.lens_servers(path);
        self.lens_of.insert(buffer, (keys.clone(), canonical.to_path_buf()));
        Some((buffer, keys))
    }

    /// An answer to `PendingLspRequest::CodeLens`: kept when the document
    /// is still as it was asked, else asked again.
    pub(super) fn lenses_answered(&mut self, key: &LspKey, path: PathBuf, edit_count: u64, request: i64, result: Result<serde_json::Value, fenix_lsp::ResponseError>) {
        let Some(session) = self.lsp_sessions.get_mut(key) else { return };
        let synced = session.open_documents.get(&path).map(|&(count, _)| count);
        let Some(doc) = session.lenses.get_mut(&path) else { return };
        if doc.asked.map(|(id, _)| id) != Some(request) {
            return;
        }
        doc.asked = None;
        let now = Instant::now();
        let lenses = match result {
            Ok(value) => match serde_json::from_value::<Option<Vec<lsp_types::CodeLens>>>(value) {
                Ok(lenses) => lenses.unwrap_or_default(),
                Err(_) => return,
            },
            Err(e) if (-32802..=-32800).contains(&e.code) => {
                doc.due = Some(now + doc.retry);
                doc.retry = (doc.retry * 2).min(LENS_MAX_RETRY);
                return;
            }
            Err(_) => return,
        };
        // Edited since it was asked: of other text.
        if synced != Some(edit_count) {
            doc.due = Some(doc.due.unwrap_or(now));
            return;
        }
        doc.lenses = lenses;
        doc.generation += 1;
        doc.resolving.clear();
        doc.retry = LENS_PAUSE;
        let buffer = doc.buffer;
        self.lay_out_lenses(buffer);
    }

    /// An answer to `PendingLspRequest::CodeLensResolve`: the lens's
    /// command, where it still is (moved, maybe, since it was asked).
    pub(super) fn lens_resolved(&mut self, key: &LspKey, path: PathBuf, generation: u64, index: usize, result: Result<serde_json::Value, fenix_lsp::ResponseError>) {
        let Some(doc) = self.lsp_sessions.get_mut(key).and_then(|s| s.lenses.get_mut(&path)) else { return };
        if doc.generation != generation {
            return;
        }
        doc.resolving.remove(&index);
        let Some(resolved) = result.ok().and_then(|v| serde_json::from_value::<lsp_types::CodeLens>(v).ok()) else { return };
        let Some(lens) = doc.lenses.get_mut(index) else { return };
        lens.command = resolved.command;
        let buffer = doc.buffer;
        self.lay_out_lenses(buffer);
    }

    /// Drawing `rows` lines of buffer `id` from line `base`: a refresh it
    /// missed asked for now, and its lenses in those lines that came
    /// without a command resolved.
    pub(super) fn note_lens_view(&mut self, id: BufferId, base: usize, rows: usize) {
        let Some((keys, path)) = self.lens_of.get(&id).cloned() else { return };
        let lines = base..base + rows;
        for key in keys {
            let Some(session) = self.lsp_sessions.get_mut(&key) else { continue };
            let resolves = session.capabilities.as_ref().and_then(lens::provider).is_some_and(|p| p.resolve);
            let Some(doc) = session.lenses.get_mut(&path) else { continue };
            if doc.stale && doc.asked.is_none() {
                doc.stale = false;
                doc.due = Some(Instant::now());
            }
            if !resolves {
                continue;
            }
            let wanted: Vec<usize> = doc
                .lenses
                .iter()
                .enumerate()
                .filter(|(i, l)| l.command.is_none() && lines.contains(&(l.range.start.line as usize)) && !doc.resolving.contains(i))
                .map(|(i, _)| i)
                .take(MAX_RESOLVING.saturating_sub(doc.resolving.len()))
                .collect();
            for index in wanted {
                let lens = doc.lenses[index].clone();
                if let Ok(id) = session.client.request::<lsp_types::request::CodeLensResolve>(lens) {
                    doc.resolving.insert(index);
                    session.pending.insert(id, PendingLspRequest::CodeLensResolve { path: path.clone(), generation: doc.generation, index });
                }
            }
        }
    }

    /// Buffer `id`'s lenses, from each of its servers in their order,
    /// laid out as rows above their lines (`App::set_virtual_rows`), and
    /// kept by line for what a click on one runs.
    pub(super) fn lay_out_lenses(&mut self, id: BufferId) {
        let mut rows: BTreeMap<usize, LensRow<LensTag>> = BTreeMap::new();
        if let Some((keys, path)) = self.lens_of.get(&id) {
            let all: Vec<(LensTag, &lsp_types::CodeLens)> = keys
                .iter()
                .filter_map(|key| self.lsp_sessions.get(key).and_then(|s| s.lenses.get(path)).filter(|d| d.buffer == id).map(|d| (key, d)))
                .flat_map(|(key, d)| d.lenses.iter().enumerate().map(move |(i, l)| ((key.clone(), i), l)))
                .collect();
            rows = lens::rows(all);
        }
        let texts: BTreeMap<usize, Vec<String>> = rows.iter().map(|(&line, row)| (line, vec![row.text.clone()])).collect();
        if texts.is_empty() {
            self.lens_rows.remove(&id);
        } else {
            self.lens_rows.insert(id, rows);
        }
        if self.virtual_rows.get(&id).map_or(!texts.is_empty(), |had| *had != texts) {
            self.set_virtual_rows(id, texts);
            self.request_redraw_all();
        }
    }
}

/// A lens to run: its server, its document, which of its lenses.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct LensPick {
    pub key: LspKey,
    pub path: PathBuf,
    pub index: usize,
}

impl App {
    /// The lens under `pos` in `pane` (laid out at `rect`), if the pointer
    /// is on one's title: what a click runs, and its row's line and
    /// columns, for the hover.
    pub(super) fn lens_at(&self, pane: fenix_window::WindowId, rect: fenix_window::Rect, pos: (f32, f32)) -> Option<(LensPick, usize, std::ops::Range<usize>)> {
        let buffer = self.mouse_text_buffer(pane)?;
        // Nothing to look for: no layout worked out.
        let rows = self.lens_rows.get(&buffer)?;
        let ob = self.buffers.get(buffer)?;
        let (char_width, line_height) = self.cell_size();
        let content = super::pane_content_rect(rect, line_height, self.pane_has_breadcrumb(pane));
        let row = ((pos.1 - content.y - crate::text::PAD_TOP) / line_height).floor();
        let x = pos.0 - content.x - crate::text::PAD_LEFT - self.gutter_chars(ob) as f32 * char_width;
        if row < 0.0 || x < 0.0 || pos.1 > content.y + content.h {
            return None;
        }
        let state = *self.pane_state(pane);
        let top = state.rendered_scroll.floor().max(0.0) as usize;
        let map = self.pane_rows(pane, buffer, top, state.scroll_row, row as usize + 1);
        let r = map.row(row as usize)?;
        if !matches!(r.kind, super::rows::RowKind::Virtual(_)) {
            return None;
        }
        let lead = super::rows::virtual_row_lead(ob, r.line, &self.tab_stops_for(buffer), map.width().unwrap_or(usize::MAX));
        let col = ((x / char_width).floor() as usize).checked_sub(lead)?;
        let lens_row = rows.get(&r.line)?;
        let (cols, (key, index)) = lens_row.spans.iter().find(|(c, _)| c.contains(&col))?;
        let path = self.lens_of.get(&buffer)?.1.clone();
        Some((LensPick { key: key.clone(), path, index: *index }, r.line, cols.clone()))
    }

    /// The pointer moved to `pos` over `pane`: the lens under it (if any)
    /// drawn as the one a click runs. Whether that changed.
    pub(super) fn hover_lens(&mut self, pane: Option<(fenix_window::WindowId, fenix_window::Rect)>, pos: (f32, f32)) -> bool {
        if self.lens_rows.is_empty() && self.lens_hover.is_none() {
            return false;
        }
        let now = pane.and_then(|(p, rect)| {
            let buffer = *self.windows().content(p)?;
            self.lens_at(p, rect, pos).map(|(_, line, cols)| (buffer, line, cols))
        });
        if now == self.lens_hover {
            return false;
        }
        self.lens_hover = now;
        true
    }

    /// Runs `pick`: the server's command sent back to it, or what Fenix
    /// does for a command it's expected to run itself; why not, when it
    /// can't.
    pub(super) fn run_lens(&mut self, pick: &LensPick) {
        let Some(session) = self.lsp_sessions.get_mut(&pick.key) else { return };
        let Some(code_lens) = session.lenses.get(&pick.path).and_then(|d| d.lenses.get(pick.index)).cloned() else { return };
        let caps = session.capabilities.clone().unwrap_or_default();
        match lens::action(&code_lens, |c| lens::executes(&caps, c)) {
            lens::Action::Server { command, arguments } => {
                let title = code_lens.command.as_ref().map_or(String::new(), |c| c.title.clone());
                let params = lsp_types::ExecuteCommandParams { command, arguments, work_done_progress_params: Default::default() };
                match session.client.request::<lsp_types::request::ExecuteCommand>(params) {
                    Ok(id) => {
                        session.pending.insert(id, PendingLspRequest::LensCommand { title });
                    }
                    Err(e) => self.set_error(format!("couldn't ask the server to run {title}: {e}")),
                }
            }
            lens::Action::ShowReferences { locations } if locations.is_empty() => self.set_message("no references"),
            lens::Action::ShowReferences { locations } => self.apply_lsp_references(locations),
            lens::Action::CargoRun(runnable) => {
                let root = runnable.cwd.as_ref().map(PathBuf::from).unwrap_or_else(|| pick.key.root.clone());
                let mut args = runnable.cargo_args.clone();
                if !runnable.executable_args.is_empty() {
                    args.push("--".into());
                    args.extend(runnable.executable_args.iter().cloned());
                }
                let name = if runnable.label.is_empty() { format!("cargo {}", args.join(" ")) } else { runnable.label.clone() };
                self.run_task_as(fenix_tasks::TaskDef { name, command: "cargo".into(), args }, root, None);
            }
            lens::Action::CargoDebug(runnable) => {
                self.set_error(format!("Fenix can't debug from a lens yet ({}): SPC u u starts the debugger", runnable.label));
            }
            lens::Action::RunTest { path, line } => {
                let _ = self.run_test_at(Path::new(&path), line);
            }
            lens::Action::Unknown { title, command } => self.set_error(format!("Fenix can't run {title} ({command})")),
            lens::Action::Unresolved => self.set_message("that lens is still being worked out"),
        }
    }

    /// `code.lens` (`SPC c l`): the cursor line's lenses -- one runs, more
    /// are listed to choose from.
    pub(crate) fn code_lens_at_cursor(&mut self) {
        let buffer = self.focused_buffer_id();
        let line = self.open().buffer.line_col(&self.cursor()).0;
        let Some(path) = self.lens_of.get(&buffer).map(|(_, p)| p.clone()) else {
            self.set_message(if self.code_lens_on() { "no code lenses here" } else { "code lens is off: lsp.code_lens turns it on" });
            return;
        };
        let Some(row) = self.lens_rows.get(&buffer).and_then(|rows| rows.get(&line)) else {
            self.set_message("no code lenses on this line");
            return;
        };
        let picks: Vec<(String, LensPick)> = row
            .spans
            .iter()
            .map(|(cols, (key, index))| (row.text.chars().skip(cols.start).take(cols.len()).collect(), LensPick { key: key.clone(), path: path.clone(), index: *index }))
            .collect();
        if let [(_, pick)] = picks.as_slice() {
            let pick = pick.clone();
            self.run_lens(&pick);
            return;
        }
        let candidates = picks.into_iter().map(|(label, pick)| fenix_picker::Candidate::new(label, pick)).collect();
        self.enter_picker(super::ActivePicker::Lenses(fenix_picker::PickerState::new(candidates)));
    }
}

/// Buffers' lens servers and path, by buffer (`App::lens_of`).
pub(super) type LensOf = HashMap<BufferId, (Vec<LspKey>, PathBuf)>;
