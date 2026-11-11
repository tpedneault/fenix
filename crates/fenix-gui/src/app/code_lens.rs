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
/// How many resolves a server may have out at once, over all its
/// documents.
const MAX_RESOLVING: usize = 16;
/// How many refusals in a row (cancelled, modified) are asked again
/// after: past that, nothing more until an edit or a refresh.
pub(super) const MAX_REFUSALS: u32 = 5;

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
    /// Refusals in a row since the last edit, answer or refresh.
    refusals: u32,
    /// The server said its lenses changed while this wasn't drawn.
    stale: bool,
    /// The lenses (by index) a resolve is out for: by which request,
    /// since when.
    pub(super) resolving: HashMap<usize, (i64, Instant)>,
    /// The lenses (by index) resolved, or tried, since the last answer:
    /// not asked again, whatever came back, until the next.
    pub(super) tried: HashSet<usize>,
    /// What the last answer's lens in the same place said, for a lens of
    /// this one not resolved yet: shown until its own resolve lands.
    carried: HashMap<usize, lsp_types::Command>,
    /// Moved by an edit not this buffer's own (an agent's): laid out
    /// again in the next frame.
    moved: bool,
}

impl LensDoc {
    fn new(buffer: BufferId) -> Self {
        LensDoc {
            buffer,
            lenses: Vec::new(),
            generation: 0,
            asked: None,
            timed_out: false,
            due: None,
            retry: LENS_PAUSE,
            refusals: 0,
            stale: false,
            resolving: HashMap::new(),
            tried: HashSet::new(),
            carried: HashMap::new(),
            moved: false,
        }
    }

    /// The lenses as they're shown: one not resolved yet with what the
    /// last answer's in its place said, while it's carried.
    fn shown(&self) -> impl Iterator<Item = (usize, std::borrow::Cow<'_, lsp_types::CodeLens>)> {
        self.lenses.iter().enumerate().map(|(i, lens)| match (lens.command.is_none(), self.carried.get(&i)) {
            (true, Some(command)) => (i, std::borrow::Cow::Owned(lsp_types::CodeLens { command: Some(command.clone()), ..lens.clone() })),
            _ => (i, std::borrow::Cow::Borrowed(lens)),
        })
    }
}

/// What a lens on a row runs: its server and which of its lenses, or
/// (`None`) which of Fenix's own.
pub(super) type LensTag = (Option<LspKey>, usize);

/// The title of Fenix's own lens over a test.
const RUN_TEST_TITLE: &str = "▶ Run test";

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
            // Several passes over the text: only when there's a lens to move.
            if !doc.lenses.is_empty() {
                lens::shift_lines(&mut doc.lenses, lens::changed_lines(old, new));
                doc.moved |= buffer.is_none();
            }
            doc.timed_out = false;
            doc.retry = LENS_PAUSE;
            doc.refusals = 0;
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
/// a second that times out waits for an edit or a good answer. Resolves
/// out that long are cancelled too, and count as tried.
fn time_out(session: &mut LspSession, canonical: &Path, now: Instant) {
    let Some(doc) = session.lenses.get_mut(canonical) else { return };
    let cancel = |client: &fenix_lsp::LspClient, id: i64| {
        let _ = client.notify::<lsp_types::notification::Cancel>(lsp_types::CancelParams { id: lsp_types::NumberOrString::Number(id as i32) });
    };
    let late: Vec<(usize, i64)> = doc.resolving.iter().filter(|(_, (_, at))| now.duration_since(*at) >= LENS_TIMEOUT).map(|(&i, &(id, _))| (i, id)).collect();
    for (index, id) in late {
        cancel(&session.client, id);
        session.pending.remove(&id);
        doc.resolving.remove(&index);
        doc.tried.insert(index);
    }
    let Some((id, _)) = doc.asked.filter(|&(_, at)| now.duration_since(at) >= LENS_TIMEOUT) else { return };
    cancel(&session.client, id);
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

    /// Buffer `id`'s own "Run test" lenses, one over each test Fenix can
    /// run alone (`fenix_tasks::test_lines`), worked out again only after
    /// an edit. None with `lsp.code_lens_tests` off, lenses off, or no
    /// lens layout for the buffer.
    fn find_own_lenses(&mut self, id: BufferId) {
        let wanted = self.code_lens_on() && self.config.bool_value("lsp.code_lens_tests").unwrap_or(true) && self.lens_of.contains_key(&id);
        let Some(ob) = self.buffers.get(id).filter(|_| wanted) else {
            self.own_lenses.remove(&id);
            return;
        };
        let edit_count = ob.buffer.edit_count();
        if self.own_lenses.get(&id).is_some_and(|(at, _)| *at == edit_count) {
            return;
        }
        let Some(path) = ob.buffer.path().map(Path::to_path_buf) else { return };
        let root = super::tool_sessions::root_for_path(&path);
        let own = fenix_tasks::test_lines(&path, &ob.buffer.text(), &root)
            .into_iter()
            .map(|line| {
                let at = lsp_types::Position { line: line as u32, character: 0 };
                lsp_types::CodeLens {
                    range: lsp_types::Range { start: at, end: at },
                    command: Some(lsp_types::Command {
                        title: RUN_TEST_TITLE.into(),
                        command: lens::RUN_TEST.into(),
                        arguments: Some(vec![serde_json::json!(path.to_string_lossy()), serde_json::json!(line + 1)]),
                    }),
                    data: None,
                }
            })
            .collect();
        self.own_lenses.insert(id, (edit_count, own));
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
        // A buffer whose language `lsp.code_lens_off` now leaves out: its
        // lenses go now, not at its next sync.
        let left_out: Vec<BufferId> = self.lens_of.keys().copied().filter(|&b| self.buffers.get(b).is_some() && !self.code_lens_wanted(b)).collect();
        for buffer in left_out {
            if let Some((keys, path)) = self.lens_of.remove(&buffer) {
                for key in keys {
                    if let Some(session) = self.lsp_sessions.get_mut(&key) {
                        session.lenses.remove(&path);
                    }
                }
            }
            self.lay_out_lenses(buffer);
        }
        let gone: Vec<BufferId> = self.lens_of.iter().filter(|(b, (keys, _))| self.buffers.get(**b).is_none() || keys.iter().any(|k| !self.lsp_sessions.contains_key(k))).map(|(b, _)| *b).collect();
        for buffer in gone {
            if self.buffers.get(buffer).is_none() {
                self.lens_of.remove(&buffer);
                self.lens_rows.remove(&buffer);
                self.own_lenses.remove(&buffer);
                self.virtual_rows.remove(&buffer);
            } else if let Some((keys, _)) = self.lens_of.get_mut(&buffer) {
                // A server stopped: its lenses with it, the view kept.
                let running: Vec<LspKey> = keys.iter().filter(|k| self.lsp_sessions.contains_key(*k)).cloned().collect();
                *keys = running;
                self.lay_out_lenses(buffer);
            }
        }
        let of = &self.lens_of;
        let mut moved = Vec::new();
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
            // Moved by an agent's edit: laid out again.
            for doc in session.lenses.values_mut().filter(|d| d.moved) {
                doc.moved = false;
                moved.push(doc.buffer);
            }
        }
        for buffer in moved {
            self.lay_out_lenses(buffer);
        }
    }

    /// When the loop next has to wake for lenses.
    pub(super) fn lens_wake(&self) -> Option<Instant> {
        self.lsp_sessions
            .values()
            .flat_map(|s| s.lenses.values())
            .filter_map(|d| {
                let ask = match d.asked {
                    Some((_, at)) => Some(at + LENS_TIMEOUT),
                    None => d.due,
                };
                let resolve = d.resolving.values().map(|&(_, at)| at + LENS_TIMEOUT).min();
                ask.into_iter().chain(resolve).min()
            })
            .min()
    }

    /// `workspace/codeLens/refresh`: `key`'s documents asked again as
    /// they're drawn.
    pub(super) fn refresh_lenses(&mut self, key: &LspKey) {
        let Some(session) = self.lsp_sessions.get_mut(key) else { return };
        for doc in session.lenses.values_mut() {
            doc.stale = true;
            doc.refusals = 0;
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
            // Asked again later, each time later still, a few times.
            Err(e) if (-32802..=-32800).contains(&e.code) => {
                doc.refusals += 1;
                if doc.refusals < MAX_REFUSALS {
                    doc.due = Some(now + doc.retry);
                    doc.retry = (doc.retry * 2).min(LENS_MAX_RETRY);
                }
                return;
            }
            Err(_) => return,
        };
        // Edited since it was asked: of other text.
        if synced != Some(edit_count) {
            doc.due = Some(doc.due.unwrap_or(now));
            return;
        }
        // In line order (the server's order kept on a line), so a view's
        // lines are found by search.
        let mut lenses = lenses;
        lenses.sort_by_key(|l| l.range.start.line);
        // What the last answer showed in the same place, kept on show for
        // a lens not resolved yet, so a title doesn't blink to `…`.
        let mut carried = HashMap::new();
        for (i, new) in lenses.iter().enumerate().filter(|(_, l)| l.command.is_none()) {
            let old = doc.lenses.iter().enumerate().find(|(_, old)| old.range == new.range && old.data == new.data);
            if let Some(command) = old.and_then(|(j, old)| old.command.clone().or_else(|| doc.carried.get(&j).cloned())) {
                carried.insert(i, command);
            }
        }
        doc.lenses = lenses;
        doc.carried = carried;
        doc.generation += 1;
        doc.resolving.clear();
        doc.tried.clear();
        doc.retry = LENS_PAUSE;
        doc.refusals = 0;
        doc.timed_out = false;
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
        // Tried, whatever it says: not asked again until the next answer.
        doc.resolving.remove(&index);
        doc.tried.insert(index);
        let resolved = result.ok().and_then(|v| serde_json::from_value::<lsp_types::CodeLens>(v).ok()).and_then(|l| l.command);
        let Some(lens) = doc.lenses.get_mut(index) else { return };
        // Its own title now, or (none came) what was carried stays.
        let Some(command) = resolved else { return };
        lens.command = Some(command);
        doc.carried.remove(&index);
        let buffer = doc.buffer;
        self.lay_out_lenses(buffer);
    }

    /// A pane drawing `runs` of buffer `id`'s lines (each a run shown
    /// together, a closed fold between two): a refresh it missed asked
    /// for now, and its lenses on those lines that came without a
    /// command resolved, each once an answer, no more than
    /// `MAX_RESOLVING` out at once from a server.
    pub(super) fn note_lens_view(&mut self, id: BufferId, runs: &[std::ops::Range<usize>]) {
        let Some((keys, path)) = self.lens_of.get(&id) else { return };
        for key in keys {
            let Some(session) = self.lsp_sessions.get_mut(key) else { continue };
            let Some(doc) = session.lenses.get_mut(path) else { continue };
            if doc.stale && doc.asked.is_none() {
                doc.stale = false;
                doc.due = Some(Instant::now());
            }
            if doc.lenses.is_empty() || !session.capabilities.as_ref().and_then(lens::provider).is_some_and(|p| p.resolve) {
                continue;
            }
            let out: usize = session.lenses.values().map(|d| d.resolving.len()).sum();
            let Some(doc) = session.lenses.get_mut(path) else { continue };
            let mut room = MAX_RESOLVING.saturating_sub(out);
            let mut wanted = Vec::new();
            for run in runs {
                // In line order: the first on the run's first line, by search.
                let first = doc.lenses.partition_point(|l| (l.range.start.line as usize) < run.start);
                for (i, l) in doc.lenses.iter().enumerate().skip(first) {
                    if room == 0 || l.range.start.line as usize >= run.end {
                        break;
                    }
                    if l.command.is_none() && !doc.resolving.contains_key(&i) && !doc.tried.contains(&i) {
                        wanted.push(i);
                        room -= 1;
                    }
                }
            }
            let now = Instant::now();
            for index in wanted {
                let lens = doc.lenses[index].clone();
                if let Ok(request) = session.client.request::<lsp_types::request::CodeLensResolve>(lens) {
                    doc.resolving.insert(index, (request, now));
                    session.pending.insert(request, PendingLspRequest::CodeLensResolve { path: path.clone(), generation: doc.generation, index });
                }
            }
        }
    }

    /// Buffer `id`'s lenses, from each of its servers in their order,
    /// laid out as rows above their lines (`App::set_virtual_rows`), and
    /// kept by line for what a click on one runs.
    pub(super) fn lay_out_lenses(&mut self, id: BufferId) {
        self.find_own_lenses(id);
        let mut rows: BTreeMap<usize, LensRow<LensTag>> = BTreeMap::new();
        if let Some((keys, path)) = self.lens_of.get(&id) {
            let mut shown: Vec<(LensTag, std::borrow::Cow<lsp_types::CodeLens>)> = keys
                .iter()
                .filter_map(|key| self.lsp_sessions.get(key).and_then(|s| s.lenses.get(path)).filter(|d| d.buffer == id).map(|d| (key, d)))
                .flat_map(|(key, d)| d.shown().map(move |(i, l)| ((Some(key.clone()), i), l)))
                .collect();
            // Fenix's own "Run test", on a test's line where no server
            // offers to run it.
            let runs: HashSet<u32> = shown.iter().filter(|(_, l)| l.command.as_ref().is_some_and(|c| c.command.to_lowercase().contains("run") || c.title.contains('▶'))).map(|(_, l)| l.range.start.line).collect();
            if let Some((_, own)) = self.own_lenses.get(&id) {
                shown.extend(own.iter().enumerate().filter(|(_, l)| !runs.contains(&l.range.start.line)).map(|(i, l)| ((None, i), std::borrow::Cow::Borrowed(l))));
            }
            rows = lens::rows(shown.iter().map(|(tag, l)| (tag.clone(), l.as_ref())));
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

/// A lens to run: its buffer, its server (`None`: one of Fenix's own),
/// its document, which of its lenses.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct LensPick {
    pub buffer: BufferId,
    pub key: Option<LspKey>,
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
        // The row as it's drawn: cut to the pane by whole titles
        // (`App::virtual_row_shown`), so only a title on show is clicked.
        let view_cols = map.view_cols();
        let lead = super::rows::virtual_row_lead(ob, r.line, &self.tab_stops_for(buffer), view_cols);
        let col = ((x / char_width).floor() as usize).checked_sub(lead)?;
        let lens_row = rows.get(&r.line)?.fit(view_cols.saturating_sub(1).saturating_sub(lead));
        let (cols, (key, index)) = lens_row.spans.iter().find(|(c, _)| c.contains(&col))?;
        let path = self.lens_of.get(&buffer)?.1.clone();
        Some((LensPick { buffer, key: key.clone(), path, index: *index }, r.line, cols.clone()))
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

    /// Whether a lens may start a program in the project at `root`: only
    /// once the project is trusted (`SPC p y`). Why not is said.
    pub(super) fn lens_may_start(&mut self, root: &Path) -> bool {
        if self.project_is_trusted(root) {
            return true;
        }
        self.set_error("not run: a lens starts programs only in a trusted project (SPC p y)");
        false
    }

    /// Runs `pick`: the server's command sent back to it, or what Fenix
    /// does for a command it's expected to run itself; why not, when it
    /// can't.
    pub(super) fn run_lens(&mut self, pick: &LensPick) {
        let Some(key) = &pick.key else {
            // One of Fenix's own: the test at its line.
            let Some(own) = self.own_lenses.get(&pick.buffer).and_then(|(_, own)| own.get(pick.index)).cloned() else { return };
            if let lens::Action::RunTest { path, line } = lens::action(&own, |_| false) {
                let _ = self.run_test_at(Path::new(&path), line);
            }
            return;
        };
        let Some(session) = self.lsp_sessions.get_mut(key) else { return };
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
            // rust-analyzer's own runs, from rust-analyzer only.
            lens::Action::CargoRun(_) | lens::Action::CargoDebug(_) if !key.is_primary_for(fenix_syntax::LanguageId::Rust) => {
                let command = code_lens.command.as_ref().map_or(String::new(), |c| c.command.clone());
                self.set_error(format!("Fenix runs {command} only from rust-analyzer"));
            }
            lens::Action::CargoRun(runnable) => {
                // Run where the server says, only inside its project: cargo
                // itself by its arguments, no shell between.
                let project = super::refactor::identity(&key.root);
                let root = runnable.cwd.as_ref().map_or_else(|| project.clone(), |cwd| super::refactor::identity(Path::new(cwd)));
                if !root.starts_with(&project) {
                    self.set_error(format!("not run: {} is outside the project", root.display()));
                    return;
                }
                if !self.lens_may_start(&project) {
                    return;
                }
                let mut args = runnable.cargo_args.clone();
                if !runnable.executable_args.is_empty() {
                    args.push("--".into());
                    args.extend(runnable.executable_args.iter().cloned());
                }
                let mut name = if runnable.label.is_empty() { format!("cargo {}", args.join(" ")) } else { runnable.label.clone() };
                // Not a project task of the same name in its place.
                if self.project_tools(&root).is_ok_and(|tools| tools.tasks.contains_key(&name)) {
                    name.push_str(" (code lens)");
                }
                self.run_task_as(fenix_tasks::TaskDef { name, command: "cargo".into(), args }, root, None);
            }
            lens::Action::CargoDebug(runnable) => {
                self.set_error(format!("Fenix can't debug from a lens yet ({}): SPC u u starts the debugger", runnable.label));
            }
            // Fenix's own command, which no server's lens runs.
            lens::Action::RunTest { .. } => self.set_error(format!("Fenix can't run {} ({})", code_lens.command.as_ref().map_or("", |c| c.title.as_str()), lens::RUN_TEST)),
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
            .map(|(cols, (key, index))| (row.text.chars().skip(cols.start).take(cols.len()).collect(), LensPick { buffer, key: key.clone(), path: path.clone(), index: *index }))
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
