//! The sizes a provider's data is held to (plan §6.5), and the one way
//! each is cut. What's over a limit is cut (a string) or left out (a
//! list's rest); the model says so once per provider and limit
//! (`Notice`).

use crate::records::Found;

/// A node's `id` and `parent`, in bytes (§4, §6.5).
pub const ID_BYTES: usize = 1024;
/// A node's `label`, a step's `label`, a provider's `runner`, in characters.
pub const LABEL_CHARS: usize = 256;
/// A `path`, a location's `path`, in bytes.
pub const PATH_BYTES: usize = 4096;
/// A node's `skip` reason, in bytes.
pub const SKIP_BYTES: usize = 1024;
/// A node's `data`, in bytes.
pub const DATA_BYTES: usize = 4096;
/// All the `data` kept for one provider: past it a node's isn't kept.
pub const DATA_PER_PROVIDER: usize = 16 << 20;
/// `found-test`s in one scan's answer.
pub const SCAN_FOUND: usize = 10_000;
/// Records in a `run` ask (§4, "What the provider is asked").
pub const ASK_RECORDS: usize = 10_000;
/// Steps in a plan.
pub const PLAN_STEPS: usize = 256;
/// Ids in all of a plan's steps' `covers`.
pub const PLAN_COVERS: usize = 10_000;
/// Reports in one call's answer.
pub const REPORTS_PER_CALL: usize = 10_000;
/// A report's `message`, a plan's error, a step's `failed` reason, a
/// provider's `problem`, in bytes.
pub const MESSAGE_BYTES: usize = 4096;
/// A failure's `text` and what a test printed, in bytes: their end is kept.
pub const TEXT_BYTES: usize = 64 << 10;
/// Tests kept for one provider in one project.
pub const TESTS_PER_PROVIDER: usize = 100_000;
/// A failure's text as the results file keeps it (§4, "What's kept"):
/// its end.
pub const KEPT_TEXT_BYTES: usize = 4096;
/// The results file, in all.
pub const RESULTS_FILE_BYTES: usize = 8 << 20;
/// How deep the tree goes: a node deeper is put at its provider's top,
/// as one whose parent isn't known is. No real runner nests this far.
pub const MAX_DEPTH: usize = 256;

/// `s` cut to at most `max` bytes, keeping its start, at a character's
/// edge. The one cut of ids: the same long id cuts the same way from a
/// scan, a listing and a report, so it still matches (§4, R9).
pub fn cut_start(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// `s` cut to at most `max` bytes, keeping its end: a failure's text
/// and what a test printed, whose end says the most.
pub fn cut_end(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut start = s.len() - max;
    while !s.is_char_boundary(start) {
        start += 1;
    }
    &s[start..]
}

/// `s` cut to at most `max` characters.
pub fn cut_chars(s: &str, max: usize) -> &str {
    match s.char_indices().nth(max) {
        Some((at, _)) => &s[..at],
        None => s,
    }
}

/// An id as the model keys it.
pub fn cut_id(id: &str) -> &str {
    cut_start(id, ID_BYTES)
}

/// Cuts `s` in place to `max` bytes (its start), saying `what` when it did.
pub(crate) fn bound(s: &mut String, max: usize, what: &'static str, said: &mut dyn FnMut(&'static str)) {
    if s.len() > max {
        let keep = cut_start(s, max).len();
        s.truncate(keep);
        said(what);
    }
}

/// Cuts `s` in place to `max` bytes, keeping its end.
pub(crate) fn bound_end(s: &mut String, max: usize, what: &'static str, said: &mut dyn FnMut(&'static str)) {
    if s.len() > max {
        *s = cut_end(s, max).to_string();
        said(what);
    }
}

pub(crate) fn bound_chars(s: &mut String, max: usize, what: &'static str, said: &mut dyn FnMut(&'static str)) {
    let keep = cut_chars(s, max).len();
    if keep < s.len() {
        s.truncate(keep);
        said(what);
    }
}

/// A provider's `found-test` held to §6.5's sizes. A last line before
/// the first is left out: the test is taken to start where it says.
pub(crate) fn bound_found(found: &mut Found, said: &mut dyn FnMut(&'static str)) {
    bound(&mut found.id, ID_BYTES, "an id over 1 KiB was cut", said);
    if let Some(parent) = &mut found.parent {
        bound(parent, ID_BYTES, "an id over 1 KiB was cut", said);
    }
    bound_chars(&mut found.label, LABEL_CHARS, "a label over 256 characters was cut", said);
    if let Some(path) = &mut found.path {
        bound(path, PATH_BYTES, "a path over 4 KiB was cut", said);
    }
    if let Some(skip) = &mut found.skip {
        bound(skip, SKIP_BYTES, "a skip reason over 1 KiB was cut", said);
    }
    if let Some(data) = &mut found.data {
        bound(data, DATA_BYTES, "a test's data over 4 KiB was cut", said);
    }
    if let (Some(line), Some(end)) = (found.line, found.end_line) {
        if end < line {
            found.end_line = None;
        }
    }
}
