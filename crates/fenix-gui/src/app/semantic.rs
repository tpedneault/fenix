//! Semantic highlighting (`docs/plans/semantic-highlighting.md`): a
//! document's tokens asked of the one of its language servers that
//! offers them, kept as char ranges that move with each edit sent, and
//! laid over the grammar's colours where they're drawn.
//!
//! Only a document shown in a buffer is asked for. Which server and
//! which path that is are worked out when the buffer is synced
//! (`App::semantic_of`), so drawing looks at nothing on disk; a
//! document's tokens belong to that buffer, and go when it closes.

use std::ops::Range;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use fenix_lsp::semantic::{self as tokens, Layer, Token};

use super::{App, BufferId, LspKey, LspSession, PendingLspRequest};

/// How long typing pauses before a document's tokens are asked again.
pub(super) const SEMANTIC_PAUSE: Duration = Duration::from_millis(250);

/// The longest wait before asking again after a refusal.
const SEMANTIC_MAX_RETRY: Duration = Duration::from_secs(4);

/// How long an ask may go unanswered before it's cancelled.
pub(super) const SEMANTIC_TIMEOUT: Duration = Duration::from_secs(10);

/// One document's tokens from one server (`LspSession::semantic`).
#[derive(Debug)]
pub(super) struct SemanticDoc {
    /// The buffer it's shown in: what the tokens are drawn for.
    pub buffer: BufferId,
    /// In order, not overlapping: chars into the text of edit count
    /// `stamp`.
    pub tokens: Vec<Token>,
    /// The edit count the tokens describe; `None` until an answer is
    /// kept, or once an edit couldn't be followed.
    pub stamp: Option<u64>,
    /// An ask is out, for this edit count, by this request, since then.
    pub asking: Option<u64>,
    pub(super) asked: Option<(i64, Instant)>,
    /// One ask timed out already: the next waits for an edit.
    pub(super) timed_out: bool,
    /// When the next ask is due.
    pub due: Option<Instant>,
    /// How long after a refusal to try again (`ContentModified` and the
    /// cancellations): doubled each time, back to the pause after an
    /// edit or an answer.
    retry: Duration,
    /// The server said its tokens changed (a refresh) while this wasn't
    /// drawn: asked once it is.
    pub(super) stale: bool,
}

impl SemanticDoc {
    fn new(buffer: BufferId) -> Self {
        SemanticDoc { buffer, tokens: Vec::new(), stamp: None, asking: None, asked: None, timed_out: false, due: None, retry: SEMANTIC_PAUSE, stale: false }
    }
}

/// The document a buffer's tokens come from: its server and its path as
/// that server knows it.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct SemanticTarget {
    pub key: LspKey,
    pub path: PathBuf,
}

/// What a sync means for a server's semantic tokens of a document.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum SemanticSync {
    /// It's the server asked, for this buffer.
    Ask(BufferId),
    /// Not a buffer's sync (an agent's): any tokens held follow the
    /// edit, and nothing new is asked.
    Follow,
    /// It isn't the server asked (or it's off): its tokens go.
    Drop,
}

/// Whether semantic tokens can be asked of `session`: it's said what it
/// offers and that includes a whole document.
pub(super) fn offers(session: &LspSession) -> bool {
    session.capabilities.as_ref().and_then(tokens::provider).is_some_and(|p| p.full)
}

/// After `sync_lsp_document` sent `canonical`'s text: on opening (or a
/// buffer newly shown), an ask due now; on a change from `old` (edit
/// count `old_count`) to `new` (`new_count`), the tokens moved through
/// it and an ask due after the pause.
pub(super) fn after_sync(session: &mut LspSession, canonical: &Path, change: Option<(&str, u64)>, new: &str, new_count: u64, sync: SemanticSync) {
    let buffer = match sync {
        SemanticSync::Drop => {
            session.semantic.remove(canonical);
            return;
        }
        SemanticSync::Follow => None,
        SemanticSync::Ask(buffer) => Some(buffer),
    };
    let held = session.semantic.contains_key(canonical);
    let doc = match (buffer, held) {
        (None, false) => return,
        (None, true) => session.semantic.get_mut(canonical).expect("held"),
        (Some(b), _) => session.semantic.entry(canonical.to_path_buf()).or_insert_with(|| SemanticDoc::new(b)),
    };
    let now = Instant::now();
    // Shown for the first time, or in another buffer than before (closed
    // and opened again): asked at once.
    let newly_shown = buffer.is_some_and(|b| !held || b != doc.buffer);
    if let Some(buffer) = buffer {
        doc.buffer = buffer;
    }
    match change {
        Some((old, old_count)) => {
            if doc.stamp == Some(old_count) && !doc.tokens.is_empty() {
                let (start, old_end, new_end) = fenix_lsp::changed_span(old, new);
                tokens::shift(&mut doc.tokens, start, old_end, new_end);
                doc.stamp = Some(new_count);
            } else {
                doc.stamp = None;
            }
            doc.timed_out = false;
            doc.retry = SEMANTIC_PAUSE;
            // An agent's sync moved them as well: asked again too.
            doc.due = Some(if newly_shown { now } else { now + SEMANTIC_PAUSE });
        }
        None if buffer.is_some() => doc.due = Some(now),
        None => {}
    }
}

/// Sends `canonical`'s ask to `session` if one is due and none is out.
/// One that can't be sent is no longer due.
fn ask(session: &mut LspSession, canonical: &Path, now: Instant) {
    let edit_count = session.open_documents.get(canonical).map(|&(count, _)| count);
    let Some(doc) = session.semantic.get_mut(canonical) else { return };
    if doc.asking.is_some() || doc.due.is_none_or(|due| due > now) {
        return;
    }
    doc.due = None;
    let (Some(edit_count), Some(uri)) = (edit_count, fenix_lsp::path_to_uri(canonical)) else { return };
    let params = lsp_types::SemanticTokensParams {
        text_document: lsp_types::TextDocumentIdentifier { uri },
        work_done_progress_params: Default::default(),
        partial_result_params: Default::default(),
    };
    if let Ok(id) = session.client.request::<lsp_types::request::SemanticTokensFullRequest>(params) {
        doc.asking = Some(edit_count);
        doc.asked = Some((id, now));
        session.pending.insert(id, PendingLspRequest::SemanticTokens { path: canonical.to_path_buf(), edit_count, request: id });
    }
}

/// An ask out longer than `SEMANTIC_TIMEOUT`: cancelled, and asked once
/// more; a second that times out waits for an edit.
fn time_out(session: &mut LspSession, canonical: &Path, now: Instant) {
    let Some(doc) = session.semantic.get_mut(canonical) else { return };
    let Some((id, at)) = doc.asked.filter(|&(_, at)| doc.asking.is_some() && now.duration_since(at) >= SEMANTIC_TIMEOUT) else { return };
    let _ = at;
    let _ = session.client.notify::<lsp_types::notification::Cancel>(lsp_types::CancelParams { id: lsp_types::NumberOrString::Number(id as i32) });
    session.pending.remove(&id);
    doc.asking = None;
    doc.asked = None;
    doc.due = (!doc.timed_out).then_some(now);
    doc.timed_out = true;
}

/// `tokens` laid over `syntax`'s colours, each as its layer says: the
/// grammar's ranges cut where a token is laid, the token's colour inside
/// it. One that's `Over` isn't laid where it reaches into `protected`
/// (the grammar's comments and strings), one that's `Fill` where it
/// reaches into anything the grammar coloured; one that `Wins` is laid
/// whatever's under it. Both lists, and what comes back, are in order
/// and don't overlap, as `split_line_by_highlights` wants.
pub(super) fn lay_over<C: Copy>(syntax: Vec<(Range<usize>, C)>, laid: &[(Range<usize>, C, Layer)], protected: &[Range<usize>]) -> Vec<(Range<usize>, C)> {
    let reaches = |sorted: &[Range<usize>], r: &Range<usize>| {
        let first = sorted.partition_point(|p| p.end <= r.start);
        sorted.get(first).is_some_and(|p| p.start < r.end)
    };
    let laid: Vec<(Range<usize>, C)> = laid
        .iter()
        .filter(|(r, _, layer)| match layer {
            Layer::Wins => true,
            Layer::Over => !reaches(protected, r),
            Layer::Fill => {
                let first = syntax.partition_point(|(s, _)| s.end <= r.start);
                syntax.get(first).is_none_or(|(s, _)| s.start >= r.end)
            }
        })
        .map(|(r, c, _)| (r.clone(), *c))
        .collect();
    if laid.is_empty() {
        return syntax;
    }
    let mut out = Vec::with_capacity(syntax.len() + laid.len() * 2);
    let mut next = 0; // the next token not yet put out
    for (range, colour) in syntax {
        let mut at = range.start;
        // The tokens before this range's end: each put out once, in order.
        while next < laid.len() && laid[next].0.start < range.end {
            let (token, c) = &laid[next];
            if token.start > at {
                out.push((at..token.start.min(range.end), colour));
            }
            out.push((token.clone(), *c));
            at = at.max(token.end);
            next += 1;
        }
        if at < range.end {
            out.push((at..range.end, colour));
        }
    }
    out.extend(laid[next..].iter().cloned());
    // A token reaching past a range's end, and the next range starting
    // under it: what's under the token goes.
    let mut tidy: Vec<(Range<usize>, C)> = Vec::with_capacity(out.len());
    for (range, c) in out {
        let start = tidy.last().map_or(range.start, |(last, _)| range.start.max(last.end));
        if start < range.end {
            tidy.push((start..range.end, c));
        }
    }
    tidy
}

impl App {
    /// `lsp.semantic_highlighting`.
    pub(super) fn semantic_highlighting_on(&self) -> bool {
        self.config.bool_value("lsp.semantic_highlighting").unwrap_or(true)
    }

    /// Whether buffer `id`'s tokens may be asked for and drawn: the
    /// setting on, and its language (as `editor.wrap_languages` names
    /// it) not in `lsp.semantic_highlighting_off`.
    pub(super) fn semantic_wanted(&self, id: BufferId) -> bool {
        if !self.semantic_highlighting_on() {
            return false;
        }
        let Some(name) = self.buffer_language_name(id) else { return true };
        !self.config.list_value("lsp.semantic_highlighting_off").iter().any(|l| l.trim().eq_ignore_ascii_case(&name))
    }

    /// The one of `path`'s servers asked for its tokens: the first, in
    /// the order its servers are, that offers them.
    pub(super) fn semantic_server(&self, path: &Path) -> Option<LspKey> {
        self.cached_lsp_keys(path).into_iter().find(|key| self.lsp_sessions.get(key).is_some_and(offers))
    }

    /// Each frame, and when the loop wakes: the asks that are due sent,
    /// those out too long cancelled, and what closed buffers held let go.
    pub(super) fn ask_due_semantic_tokens(&mut self) {
        let now = Instant::now();
        // Turned off: nothing held, nothing asked.
        if !self.semantic_highlighting_on() {
            self.semantic_of.clear();
        }
        let buffers = &self.buffers;
        self.semantic_of.retain(|b, _| buffers.get(*b).is_some());
        let of = &self.semantic_of;
        for (key, session) in self.lsp_sessions.iter_mut() {
            if session.semantic.is_empty() {
                continue;
            }
            // Held only for a buffer showing it from this server now: not
            // one closed, renamed away, or asked of another server since.
            session.semantic.retain(|path, d| of.get(&d.buffer).is_some_and(|t| t.key == *key && t.path == *path));
            let paths: Vec<PathBuf> = session.semantic.keys().cloned().collect();
            for path in paths {
                time_out(session, &path, now);
                ask(session, &path, now);
            }
        }
    }

    /// When the loop next has to wake for semantic tokens: an ask due,
    /// or one out that would time out.
    pub(super) fn semantic_wake(&self) -> Option<Instant> {
        self.lsp_sessions
            .values()
            .flat_map(|s| s.semantic.values())
            .filter_map(|d| match (d.asking, d.asked) {
                (Some(_), Some((_, at))) => Some(at + SEMANTIC_TIMEOUT),
                (Some(_), None) => None,
                (None, _) => d.due,
            })
            .min()
    }

    /// `workspace/semanticTokens/refresh`: `key`'s documents asked again
    /// as they're drawn, not all at once.
    pub(super) fn refresh_semantic_tokens(&mut self, key: &LspKey) {
        let Some(session) = self.lsp_sessions.get_mut(key) else { return };
        for doc in session.semantic.values_mut() {
            doc.stale = true;
        }
        self.request_redraw_all();
    }

    /// An answer to `PendingLspRequest::SemanticTokens`: kept when the
    /// document is still as it was asked, else asked again.
    pub(super) fn semantic_tokens_answered(&mut self, key: &LspKey, path: PathBuf, edit_count: u64, request: i64, result: Result<serde_json::Value, fenix_lsp::ResponseError>) {
        let Some(session) = self.lsp_sessions.get_mut(key) else { return };
        let synced = session.open_documents.get(&path).map(|&(count, _)| count);
        let (Some(doc), Some(text)) = (session.semantic.get_mut(&path), session.synced_text.get(&path)) else { return };
        // Not the ask that's out (one before the document was shown again,
        // or cancelled): what it says isn't wanted.
        if doc.asked.map(|(id, _)| id) != Some(request) {
            return;
        }
        doc.asking = None;
        doc.asked = None;
        let now = Instant::now();
        let data = match result {
            Ok(value) => match tokens::read_answer(value) {
                Ok(answer) => answer.and_then(|a| a.data).unwrap_or_default(),
                Err(_) => return,
            },
            // Cancelled or modified while it worked: asked again after
            // a pause. Any other refusal: not asked again until an edit.
            // RequestCancelled, ContentModified, ServerCancelled.
            Err(e) if (-32802..=-32800).contains(&e.code) => {
                doc.due = Some(now + doc.retry);
                doc.retry = (doc.retry * 2).min(SEMANTIC_MAX_RETRY);
                return;
            }
            Err(_) => return,
        };
        // Edited since it was asked: what it says is of other text.
        if synced != Some(edit_count) {
            doc.due = Some(doc.due.unwrap_or(now));
            return;
        }
        let Some(legend) = session.capabilities.as_ref().and_then(tokens::legend) else { return };
        doc.tokens = tokens::decode(&data, text, &tokens::Styles::new(legend));
        doc.stamp = Some(edit_count);
        doc.retry = SEMANTIC_PAUSE;
        self.request_redraw_all();
    }

    /// Buffer `id`'s tokens, when it has some that describe its text as
    /// it is and they're wanted: where they're held. Nothing on disk is
    /// looked at.
    pub(super) fn drawn_tokens(&self, id: BufferId) -> Option<&SemanticTarget> {
        let target = self.semantic_of.get(&id)?;
        let doc = self.lsp_sessions.get(&target.key)?.semantic.get(&target.path)?;
        let edit_count = self.buffers.get(id)?.buffer.edit_count();
        (doc.buffer == id && doc.stamp == Some(edit_count) && self.semantic_wanted(id)).then_some(target)
    }

    /// Drawing buffer `id`: a refresh it missed asked for now.
    fn ask_if_stale(&mut self, id: BufferId) {
        let Some(target) = self.semantic_of.get(&id) else { return };
        let Some(doc) = self.lsp_sessions.get_mut(&target.key).and_then(|s| s.semantic.get_mut(&target.path)) else { return };
        // Kept while an ask is out: asked once that's answered.
        if doc.stale && doc.asking.is_none() {
            doc.stale = false;
            doc.due = Some(Instant::now());
        }
    }

    /// Whether drawing buffer `id` may lay tokens: false (without
    /// allocating anything) when it has none to lay.
    pub(super) fn semantic_to_draw(&mut self, id: BufferId) -> bool {
        if self.semantic_of.is_empty() {
            return false;
        }
        self.ask_if_stale(id);
        self.drawn_tokens(id).is_some()
    }

    /// The grammar's colours for buffer `id` (`syntax`), with its server's
    /// tokens in `windows` (bytes) laid over them; unchanged when it has
    /// none that describe the text as it is.
    pub(super) fn with_semantic_tokens(
        &self,
        id: BufferId,
        syntax: Vec<(Range<usize>, glyphon::Color)>,
        windows: &[Range<usize>],
        protected: &[Range<usize>],
    ) -> Vec<(Range<usize>, glyphon::Color)> {
        let Some(target) = self.drawn_tokens(id) else { return syntax };
        let (Some(doc), Some(ob)) = (self.lsp_sessions.get(&target.key).and_then(|s| s.semantic.get(&target.path)), self.buffers.get(id)) else { return syntax };
        let theme = self.theme;
        let mut laid = Vec::new();
        for window in windows {
            let (from, to) = (ob.buffer.byte_to_char(window.start), ob.buffer.byte_to_char(window.end));
            for t in tokens::within(&doc.tokens, from, to) {
                let Some(slot) = t.style.slot else { continue };
                let bytes = ob.buffer.char_to_byte(t.start.max(from))..ob.buffer.char_to_byte(t.end.min(to));
                if !bytes.is_empty() {
                    laid.push((bytes, theme.semantic_color(slot), t.style.layer));
                }
            }
        }
        lay_over(syntax, &laid, protected)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(start: usize, end: usize, c: char) -> (Range<usize>, char) {
        (start..end, c)
    }

    /// A token laid over the grammar.
    fn o(start: usize, end: usize, c: char) -> (Range<usize>, char, Layer) {
        (start..end, c, Layer::Over)
    }

    #[test]
    fn a_token_inside_a_range_cuts_it() {
        let got = lay_over(vec![r(0, 10, 'v')], &[o(3, 5, 'p')], &[]);
        assert_eq!(got, vec![r(0, 3, 'v'), r(3, 5, 'p'), r(5, 10, 'v')]);
    }

    #[test]
    fn tokens_between_and_across_ranges_are_put_in_order() {
        let got = lay_over(vec![r(0, 2, 'k'), r(4, 8, 'v'), r(12, 14, 'o')], &[o(2, 3, 'a'), o(6, 13, 'b'), o(20, 22, 'c')], &[]);
        assert_eq!(got, vec![r(0, 2, 'k'), r(2, 3, 'a'), r(4, 6, 'v'), r(6, 13, 'b'), r(13, 14, 'o'), r(20, 22, 'c')]);
    }

    #[test]
    fn a_token_over_a_whole_range_replaces_it() {
        let got = lay_over(vec![r(0, 3, 'f'), r(4, 7, 'v')], &[o(4, 7, 'p')], &[]);
        assert_eq!(got, vec![r(0, 3, 'f'), r(4, 7, 'p')]);
    }

    #[test]
    fn a_token_reaching_into_a_comment_or_string_isn_t_laid() {
        let got = lay_over(vec![r(0, 10, 'c'), r(12, 14, 'v')], &[o(2, 4, 'f'), o(12, 14, 'p')], std::slice::from_ref(&(0..10)));
        assert_eq!(got, vec![r(0, 10, 'c'), r(12, 14, 'p')]);
    }

    #[test]
    fn nothing_to_lay_leaves_the_grammar_s_colours() {
        let syntax = vec![r(0, 2, 'k'), r(3, 4, 'v')];
        assert_eq!(lay_over(syntax.clone(), &[], &[]), syntax);
    }

    #[test]
    fn a_lexical_token_fills_only_where_the_grammar_coloured_nothing_and_a_comment_wins() {
        let syntax = vec![r(0, 3, 'k'), r(6, 9, 's')];
        // A keyword token over the grammar's keyword: the grammar's; one
        // where it coloured nothing: laid.
        let got = lay_over(syntax.clone(), &[(0..3, 'K', Layer::Fill), (4..5, 'K', Layer::Fill)], &[]);
        assert_eq!(got, vec![r(0, 3, 'k'), r(4, 5, 'K'), r(6, 9, 's')]);
        // An inactive region's comment token: over all of it, a string too.
        let got = lay_over(syntax, &[(0..9, 'c', Layer::Wins)], std::slice::from_ref(&(6..9)));
        assert_eq!(got, vec![r(0, 9, 'c')]);
    }

    #[test]
    fn a_token_where_the_grammar_coloured_nothing_is_laid_too() {
        let got = lay_over(vec![], &[o(1, 2, 'p')], &[]);
        assert_eq!(got, vec![r(1, 2, 'p')]);
    }
}
