//! Semantic highlighting (`docs/plans/semantic-highlighting.md`): a
//! document's tokens asked of the one of its language servers that
//! offers them, kept as char ranges that move with each edit sent, and
//! laid over the grammar's colours where they're drawn.

use std::ops::Range;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use fenix_lsp::semantic::{self as tokens, Token};

use super::{App, BufferId, LspKey, LspSession, PendingLspRequest};

/// How long typing pauses before a document's tokens are asked again.
pub(super) const SEMANTIC_PAUSE: Duration = Duration::from_millis(250);

/// A document this many lines or bytes long is asked only for the lines
/// in view, when its server answers that way.
const RANGE_LINES: usize = 10_000;
const RANGE_BYTES: usize = 1 << 20;
/// Above this many lines, a server that can't answer for a range isn't
/// asked for the whole document either.
const MAX_FULL_LINES: usize = 50_000;
/// Lines asked for above and below those in view.
const RANGE_MARGIN: usize = 100;

/// One document's tokens from one server (`LspSession::semantic`).
#[derive(Debug, Default)]
pub(super) struct SemanticDoc {
    /// In order, not overlapping: chars into the text of edit count
    /// `stamp`.
    pub tokens: Vec<Token>,
    /// The edit count the tokens describe; `None` until an answer is
    /// kept, or once an edit couldn't be followed.
    pub stamp: Option<u64>,
    /// An ask is out, for this edit count.
    pub asking: Option<u64>,
    /// When the next ask is due.
    pub due: Option<Instant>,
    /// The server's last whole-document answer, as sent, and its id: what
    /// a delta is applied to.
    result_id: Option<String>,
    data: Vec<lsp_types::SemanticToken>,
    /// Asked by range: the lines the tokens cover, and those in view
    /// (with a margin) to ask for next.
    covered: Option<Range<usize>>,
    want: Option<Range<usize>>,
}

/// What was asked for.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum SemanticAsk {
    Full,
    Delta,
    /// These lines.
    Range(Range<usize>),
}

/// Whether semantic tokens can be asked of `session`: it's said what it
/// offers, a whole document or a range.
pub(super) fn offers(session: &LspSession) -> bool {
    session.capabilities.as_ref().and_then(tokens::provider).is_some()
}

/// Whether a document of `lines` lines and `bytes` bytes is asked for by
/// range: large and the server answers that way, or it answers no other.
fn by_range(provider: &tokens::Provider, lines: usize, bytes: usize) -> bool {
    provider.range && (!provider.full || lines > RANGE_LINES || bytes > RANGE_BYTES)
}

/// After `sync_lsp_document` sent `canonical`'s text: on opening, an ask
/// due now; on a change from `old` (edit count `old_count`) to `new`
/// (`new_count`), the tokens moved through it and an ask due after the
/// pause. `ask` is false for a server that isn't the one asked (or with
/// it off): its tokens are let go.
pub(super) fn after_sync(session: &mut LspSession, canonical: &Path, change: Option<(&str, u64)>, new: &str, new_count: u64, ask: bool) {
    if !ask {
        session.semantic.remove(canonical);
        return;
    }
    let doc = session.semantic.entry(canonical.to_path_buf()).or_default();
    let now = Instant::now();
    match change {
        None => doc.due = Some(now),
        Some((old, old_count)) => {
            if doc.stamp == Some(old_count) && !doc.tokens.is_empty() {
                let (start, old_end, new_end) = fenix_lsp::changed_span(old, new);
                tokens::shift(&mut doc.tokens, start, old_end, new_end);
                doc.stamp = Some(new_count);
            } else {
                doc.stamp = None;
            }
            doc.due = Some(now + SEMANTIC_PAUSE);
        }
    }
}

/// Sends `canonical`'s ask to `session` if one is due and none is out:
/// by range for a large document (the lines last in view), as a delta on
/// the last answer when the server keeps them, else the whole of it. One
/// that can't be sent is no longer due.
fn ask(session: &mut LspSession, canonical: &Path, now: Instant) {
    let edit_count = session.open_documents.get(canonical).map(|&(count, _)| count);
    let (lines, bytes) = session.synced_text.get(canonical).map_or((0, 0), |t| (t.bytes().filter(|&b| b == b'\n').count() + 1, t.len()));
    let provider = session.capabilities.as_ref().and_then(tokens::provider);
    let Some(doc) = session.semantic.get_mut(canonical) else { return };
    if doc.asking.is_some() || doc.due.is_none_or(|due| due > now) {
        return;
    }
    doc.due = None;
    let (Some(edit_count), Some(provider), Some(uri)) = (edit_count, provider, fenix_lsp::path_to_uri(canonical)) else { return };
    let text_document = lsp_types::TextDocumentIdentifier { uri };
    let how = if by_range(&provider, lines, bytes) {
        // Nothing in view yet: asked once it's drawn.
        let Some(want) = doc.want.clone() else { return };
        SemanticAsk::Range(want)
    } else if !provider.full || lines > MAX_FULL_LINES {
        return;
    } else if provider.delta && doc.result_id.is_some() {
        SemanticAsk::Delta
    } else {
        SemanticAsk::Full
    };
    let (work_done_progress_params, partial_result_params) = (Default::default(), Default::default());
    let sent = match &how {
        SemanticAsk::Full => session.client.request::<lsp_types::request::SemanticTokensFullRequest>(lsp_types::SemanticTokensParams { text_document, work_done_progress_params, partial_result_params }),
        SemanticAsk::Delta => session.client.request::<lsp_types::request::SemanticTokensFullDeltaRequest>(lsp_types::SemanticTokensDeltaParams {
            text_document,
            previous_result_id: doc.result_id.clone().unwrap_or_default(),
            work_done_progress_params,
            partial_result_params,
        }),
        SemanticAsk::Range(lines) => {
            let at = |line: usize| lsp_types::Position { line: line as u32, character: 0 };
            let range = lsp_types::Range { start: at(lines.start), end: at(lines.end) };
            session.client.request::<lsp_types::request::SemanticTokensRangeRequest>(lsp_types::SemanticTokensRangeParams { text_document, range, work_done_progress_params, partial_result_params })
        }
    };
    if let Ok(id) = sent {
        doc.asking = Some(edit_count);
        session.pending.insert(id, PendingLspRequest::SemanticTokens { path: canonical.to_path_buf(), edit_count, how });
    }
}

/// `tokens` laid over `syntax`'s colours: the grammar's ranges cut where
/// a token is, the token's colour inside it. A token reaching into one
/// of `protected` (the grammar's comments and strings) isn't laid: the
/// grammar already has those right. Both lists, and what comes back,
/// are in order and don't overlap, as `split_line_by_highlights` wants.
pub(super) fn lay_over<C: Copy>(syntax: Vec<(Range<usize>, C)>, laid: &[(Range<usize>, C)], protected: &[Range<usize>]) -> Vec<(Range<usize>, C)> {
    let laid: Vec<&(Range<usize>, C)> = laid
        .iter()
        .filter(|(r, _)| {
            let first = protected.partition_point(|p| p.end <= r.start);
            protected.get(first).is_none_or(|p| p.start >= r.end)
        })
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
            let (token, c) = laid[next];
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
    out.extend(laid[next..].iter().map(|(r, c)| (r.clone(), *c)));
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

    /// Whether `path`'s tokens may be asked for and drawn: the setting on,
    /// and its language not in `lsp.semantic_highlighting_off`.
    pub(super) fn semantic_wanted(&self, path: &Path) -> bool {
        if !self.semantic_highlighting_on() {
            return false;
        }
        let Some(language) = fenix_syntax::detect_language_from_path(path) else { return true };
        let name = language.name();
        !self.config.list_value("lsp.semantic_highlighting_off").iter().any(|l| l.trim().eq_ignore_ascii_case(&name))
    }

    /// The one of `path`'s servers asked for its tokens: the first, in
    /// the order its servers are, that offers them.
    pub(super) fn semantic_server(&self, path: &Path) -> Option<LspKey> {
        self.cached_lsp_keys(path).into_iter().find(|key| self.lsp_sessions.get(key).is_some_and(offers))
    }

    /// Each frame: the asks that are due, sent.
    pub(super) fn ask_due_semantic_tokens(&mut self) {
        let now = Instant::now();
        for session in self.lsp_sessions.values_mut() {
            if session.semantic.is_empty() {
                continue;
            }
            let due: Vec<PathBuf> = session.semantic.iter().filter(|(_, d)| d.asking.is_none() && d.due.is_some_and(|t| t <= now)).map(|(p, _)| p.clone()).collect();
            for path in due {
                ask(session, &path, now);
            }
        }
    }

    /// The next ask due, for the loop to wake for: at once, for one a
    /// frame drawn since made due.
    pub(super) fn semantic_wake(&self, now: Instant) -> Option<Instant> {
        self.lsp_sessions.values().flat_map(|s| s.semantic.values()).filter(|d| d.asking.is_none()).filter_map(|d| d.due).min().map(|t| t.max(now))
    }

    /// `workspace/semanticTokens/refresh`: every open document of `key`'s
    /// asked again.
    pub(super) fn refresh_semantic_tokens(&mut self, key: &LspKey) {
        let Some(session) = self.lsp_sessions.get_mut(key) else { return };
        let now = Instant::now();
        for doc in session.semantic.values_mut() {
            doc.due = Some(now);
        }
    }

    /// An answer to `PendingLspRequest::SemanticTokens` (asked `how`): a
    /// whole document's or a delta's kept as the server's last answer
    /// whatever happened since; the tokens decoded when the document is
    /// still as it was asked, else asked again.
    pub(super) fn semantic_tokens_answered(&mut self, key: &LspKey, path: PathBuf, edit_count: u64, how: SemanticAsk, result: Result<serde_json::Value, fenix_lsp::ResponseError>) {
        let Some(session) = self.lsp_sessions.get_mut(key) else { return };
        let legend = session.capabilities.as_ref().and_then(tokens::provider).map(|p| p.legend);
        let synced = session.open_documents.get(&path).map(|&(count, _)| count);
        let text = session.synced_text.get(&path);
        let Some(doc) = session.semantic.get_mut(&path) else { return };
        doc.asking = None;
        let now = Instant::now();
        let value = match result {
            Ok(value) => value,
            // Modified or cancelled while it worked: asked again after
            // a pause.
            Err(e) if e.code == -32801 || e.code == -32802 => {
                doc.due = Some(now + SEMANTIC_PAUSE);
                return;
            }
            // A delta it can't give (it's let the last answer go): the
            // whole document, after a pause. Any other refusal: not asked
            // again until an edit.
            Err(_) => {
                match &how {
                    SemanticAsk::Delta => {
                        doc.result_id = None;
                        doc.due = Some(now + SEMANTIC_PAUSE);
                    }
                    // Those lines as good as answered: not asked again
                    // each frame they're in view.
                    SemanticAsk::Range(lines) => doc.covered = Some(lines.clone()),
                    SemanticAsk::Full => {}
                }
                return;
            }
        };
        let whole = |doc: &mut SemanticDoc, t: lsp_types::SemanticTokens| {
            doc.result_id = t.result_id;
            doc.data = t.data;
        };
        let range = match how {
            SemanticAsk::Full => {
                match serde_json::from_value::<Option<lsp_types::SemanticTokensResult>>(value) {
                    Ok(Some(lsp_types::SemanticTokensResult::Tokens(t))) => whole(doc, t),
                    Ok(Some(lsp_types::SemanticTokensResult::Partial(p))) => whole(doc, lsp_types::SemanticTokens { result_id: None, data: p.data }),
                    Ok(None) => whole(doc, Default::default()),
                    Err(_) => return,
                }
                None
            }
            SemanticAsk::Delta => {
                let edits = match serde_json::from_value::<Option<lsp_types::SemanticTokensFullDeltaResult>>(value) {
                    Ok(Some(lsp_types::SemanticTokensFullDeltaResult::Tokens(t))) => {
                        whole(doc, t);
                        None
                    }
                    Ok(Some(lsp_types::SemanticTokensFullDeltaResult::TokensDelta(d))) => Some((d.result_id, d.edits)),
                    Ok(Some(lsp_types::SemanticTokensFullDeltaResult::PartialTokensDelta { edits })) => Some((None, edits)),
                    Ok(None) => {
                        whole(doc, Default::default());
                        None
                    }
                    Err(_) => return,
                };
                if let Some((result_id, edits)) = edits {
                    match tokens::apply_edits(&doc.data, &edits) {
                        Some(data) => whole(doc, lsp_types::SemanticTokens { result_id, data }),
                        // Edits that don't fit what's kept: the whole of
                        // it asked again.
                        None => {
                            doc.result_id = None;
                            doc.due = Some(now);
                            return;
                        }
                    }
                }
                None
            }
            SemanticAsk::Range(lines) => match serde_json::from_value::<Option<lsp_types::SemanticTokensRangeResult>>(value) {
                Ok(Some(lsp_types::SemanticTokensRangeResult::Tokens(t))) => Some((lines, t.data)),
                Ok(Some(lsp_types::SemanticTokensRangeResult::Partial(p))) => Some((lines, p.data)),
                Ok(None) => Some((lines, Vec::new())),
                Err(_) => return,
            },
        };
        // Edited since it was asked: what it says is of other text.
        if synced != Some(edit_count) {
            doc.due = Some(doc.due.unwrap_or(now));
            return;
        }
        let (Some(legend), Some(text)) = (legend, text) else { return };
        let styles = tokens::Styles::new(&legend);
        match range {
            Some((lines, data)) => {
                doc.tokens = tokens::decode(&data, text, &styles);
                doc.covered = Some(lines);
            }
            None => {
                doc.tokens = tokens::decode(&doc.data, text, &styles);
                doc.covered = None;
            }
        }
        doc.stamp = Some(edit_count);
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    /// `path` as its language servers know it, worked out once.
    pub(super) fn lsp_path(&mut self, path: &Path) -> PathBuf {
        if let Some(canonical) = self.lsp_canonical.get(path) {
            return canonical.clone();
        }
        let canonical = fenix_lsp::normalize(std::fs::canonicalize(path).unwrap_or(path.to_path_buf()));
        self.lsp_canonical.insert(path.to_path_buf(), canonical.clone());
        canonical
    }

    /// Buffer `id`'s document as its tokens' server knows it: the server
    /// and the path, when its tokens are wanted and it has a server
    /// asked for them.
    fn semantic_doc_of(&mut self, id: BufferId) -> Option<(LspKey, PathBuf)> {
        let path = self.buffers.get(id).and_then(|ob| ob.buffer.path()).map(Path::to_path_buf)?;
        if self.lsp_sessions.values().all(|s| s.semantic.is_empty()) || !self.semantic_wanted(&path) {
            return None;
        }
        let key = self.semantic_server(&path)?;
        let canonical = self.lsp_path(&path);
        self.lsp_sessions.get(&key)?.semantic.contains_key(&canonical).then_some((key, canonical))
    }

    /// Buffer `id`'s tokens, when it has some that describe its text as
    /// it is: the server they're from and the document's path.
    fn drawn_tokens(&mut self, id: BufferId) -> Option<(LspKey, PathBuf)> {
        let (key, canonical) = self.semantic_doc_of(id)?;
        let edit_count = self.buffers.get(id)?.buffer.edit_count();
        let doc = self.lsp_sessions.get(&key)?.semantic.get(&canonical)?;
        (doc.stamp == Some(edit_count)).then_some((key, canonical))
    }

    /// For a document asked for by range: the lines `windows` (bytes)
    /// show, with a margin, to be asked for once they're no longer all
    /// covered.
    fn note_semantic_view(&mut self, id: BufferId, key: &LspKey, canonical: &Path, windows: &[Range<usize>]) {
        let (Some(ob), Some(first), Some(last)) = (self.buffers.get(id), windows.first(), windows.last()) else { return };
        let Some(session) = self.lsp_sessions.get_mut(key) else { return };
        let Some(provider) = session.capabilities.as_ref().and_then(tokens::provider) else { return };
        let lines = ob.buffer.line_count();
        if !by_range(&provider, lines, ob.buffer.rope().len_bytes()) {
            return;
        }
        let rope = ob.buffer.rope();
        let shown = rope.byte_to_line(first.start.min(rope.len_bytes()))..rope.byte_to_line(last.end.min(rope.len_bytes())) + 1;
        let Some(doc) = session.semantic.get_mut(canonical) else { return };
        if doc.covered.as_ref().is_some_and(|c| c.start <= shown.start && shown.end <= c.end) {
            return;
        }
        let want = shown.start.saturating_sub(RANGE_MARGIN)..(shown.end + RANGE_MARGIN).min(lines);
        if doc.want.as_ref() != Some(&want) || doc.due.is_none() {
            doc.want = Some(want);
            if doc.asking.is_none() {
                doc.due = Some(doc.due.map_or_else(Instant::now, |due| due.min(Instant::now())));
            }
        }
    }

    /// The grammar's colours for buffer `id` (`syntax`), with its server's
    /// tokens in `windows` (bytes) laid over them; unchanged when it has
    /// none that describe the text as it is.
    pub(super) fn with_semantic_tokens(
        &mut self,
        id: BufferId,
        syntax: Vec<(Range<usize>, glyphon::Color)>,
        windows: &[Range<usize>],
        protected: &[Range<usize>],
    ) -> Vec<(Range<usize>, glyphon::Color)> {
        let Some((key, canonical)) = self.semantic_doc_of(id) else { return syntax };
        self.note_semantic_view(id, &key, &canonical, windows);
        if self.drawn_tokens(id).is_none() {
            return syntax;
        }
        let (Some(doc), Some(ob)) = (self.lsp_sessions.get(&key).and_then(|s| s.semantic.get(&canonical)), self.buffers.get(id)) else { return syntax };
        let theme = self.theme;
        let mut laid = Vec::new();
        for window in windows {
            let (from, to) = (ob.buffer.byte_to_char(window.start), ob.buffer.byte_to_char(window.end));
            for t in tokens::within(&doc.tokens, from, to) {
                let bytes = ob.buffer.char_to_byte(t.start.max(from))..ob.buffer.char_to_byte(t.end.min(to));
                if !bytes.is_empty() {
                    laid.push((bytes, theme.semantic_color(t.style.slot)));
                }
            }
        }
        lay_over(syntax, &laid, protected)
    }

    /// Buffer `id`'s tokens drawn underlined (`mutable`) on `lines`: each
    /// as its line, its char columns and its colour.
    pub(super) fn underlined_tokens(&mut self, id: BufferId, lines: Range<usize>) -> Vec<(usize, Range<usize>, glyphon::Color)> {
        let Some((key, canonical)) = self.drawn_tokens(id) else { return Vec::new() };
        let (Some(doc), Some(ob)) = (self.lsp_sessions.get(&key).and_then(|s| s.semantic.get(&canonical)), self.buffers.get(id)) else { return Vec::new() };
        let last = ob.buffer.visual_line_count();
        if lines.start >= last {
            return Vec::new();
        }
        let (from, to) = (ob.buffer.line_start_char(lines.start), ob.buffer.line_start_char(lines.end.min(last) - 1) + ob.buffer.line_len(lines.end.min(last) - 1));
        tokens::within(&doc.tokens, from, to)
            .iter()
            .filter(|t| t.style.underline)
            .map(|t| {
                let line = ob.buffer.rope().char_to_line(t.start);
                let first = ob.buffer.line_start_char(line);
                (line, t.start - first..t.end - first, self.theme.semantic_color(t.style.slot))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(start: usize, end: usize, c: char) -> (Range<usize>, char) {
        (start..end, c)
    }

    #[test]
    fn a_token_inside_a_range_cuts_it() {
        let got = lay_over(vec![r(0, 10, 'v')], &[r(3, 5, 'p')], &[]);
        assert_eq!(got, vec![r(0, 3, 'v'), r(3, 5, 'p'), r(5, 10, 'v')]);
    }

    #[test]
    fn tokens_between_and_across_ranges_are_put_in_order() {
        let got = lay_over(vec![r(0, 2, 'k'), r(4, 8, 'v'), r(12, 14, 'o')], &[r(2, 3, 'a'), r(6, 13, 'b'), r(20, 22, 'c')], &[]);
        assert_eq!(got, vec![r(0, 2, 'k'), r(2, 3, 'a'), r(4, 6, 'v'), r(6, 13, 'b'), r(13, 14, 'o'), r(20, 22, 'c')]);
    }

    #[test]
    fn a_token_over_a_whole_range_replaces_it() {
        let got = lay_over(vec![r(0, 3, 'f'), r(4, 7, 'v')], &[r(4, 7, 'p')], &[]);
        assert_eq!(got, vec![r(0, 3, 'f'), r(4, 7, 'p')]);
    }

    #[test]
    fn a_token_reaching_into_a_comment_or_string_isn_t_laid() {
        let got = lay_over(vec![r(0, 10, 'c'), r(12, 14, 'v')], &[r(2, 4, 'f'), r(12, 14, 'p')], std::slice::from_ref(&(0..10)));
        assert_eq!(got, vec![r(0, 10, 'c'), r(12, 14, 'p')]);
    }

    #[test]
    fn nothing_to_lay_leaves_the_grammar_s_colours() {
        let syntax = vec![r(0, 2, 'k'), r(3, 4, 'v')];
        assert_eq!(lay_over(syntax.clone(), &[], &[]), syntax);
    }

    #[test]
    fn a_token_where_the_grammar_coloured_nothing_is_laid_too() {
        let got = lay_over(vec![], &[r(1, 2, 'p')], &[]);
        assert_eq!(got, vec![r(1, 2, 'p')]);
    }
}
