//! The sizes a provider's data is held to (plan §6.5), and the one way
//! each is cut. What's over a limit is cut (a string), left out (a
//! list's rest) or, for a test's `data`, not kept; the model says so
//! once per provider and limit (`Notice`).

use crate::records::Found;
use std::borrow::Cow;

/// A node's `id` and `parent`, in bytes (§4, §6.5).
pub const ID_BYTES: usize = 1024;
/// A node's `label`, a step's `label`, a provider's `runner`, in characters.
pub const LABEL_CHARS: usize = 256;
/// A `path`, a location's `path`, in bytes.
pub const PATH_BYTES: usize = 4096;
/// A node's `skip` reason, in bytes.
pub const SKIP_BYTES: usize = 1024;
/// A node's `data`, in bytes: one over it isn't kept.
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
/// edge.
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

/// How much of an id over 1 KiB is kept before its hash: 1,024 less
/// the `#` and 16 hex digits.
pub const ID_PREFIX_BYTES: usize = ID_BYTES - 1 - 16;

/// FNV-1a, 64 bits, of `bytes`: the hash a cut id ends with. Chosen
/// because a pack's kit computes it in a dozen lines with nothing to
/// depend on, and its value is fixed for good.
pub fn fnv1a_64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// An id as Fenix keys it (§4, §6.5, "Review of steps 1 and 2"): one of
/// at most 1,024 bytes as it is; a longer one as its first bytes up to
/// 1,007, cut back to a character's start, then `#`, then the FNV-1a
/// 64-bit hash of the whole id's UTF-8 bytes as 16 lowercase hex digits
/// -- 1,021 to 1,024 bytes. The same long id cuts the same way from a
/// scan, a listing, a report and the host's limits, so it still matches
/// (R9), and two that share their first KiB stay two. Cutting a cut id
/// changes nothing. The host (`fenix_wasm::test_limits`) cuts with this
/// same function.
pub fn cut_id(id: &str) -> Cow<'_, str> {
    if id.len() <= ID_BYTES {
        return Cow::Borrowed(id);
    }
    Cow::Owned(format!("{}#{:016x}", cut_start(id, ID_PREFIX_BYTES), fnv1a_64(id.as_bytes())))
}

/// Whether `id` has the form `cut_id` gives a long one: 1,021 bytes or
/// more, ending in `#` and 16 lowercase hex digits. Its provider can't
/// name it from the id alone: only from its `data`.
pub fn is_cut_id(id: &str) -> bool {
    let b = id.as_bytes();
    b.len() > ID_PREFIX_BYTES + 13 && b.len() <= ID_BYTES && b[b.len() - 17] == b'#' && b[b.len() - 16..].iter().all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f'))
}

/// Cuts an id in place as `cut_id` does, saying `what` when it did.
pub(crate) fn bound_id(s: &mut String, said: &mut dyn FnMut(&'static str)) {
    if s.len() > ID_BYTES {
        *s = cut_id(s).into_owned();
        said("an id over 1 KiB was cut");
    }
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
    bound_id(&mut found.id, said);
    if let Some(parent) = &mut found.parent {
        bound_id(parent, said);
    }
    bound_chars(&mut found.label, LABEL_CHARS, "a label over 256 characters was cut", said);
    if let Some(path) = &mut found.path {
        bound(path, PATH_BYTES, "a path over 4 KiB was cut", said);
    }
    if let Some(skip) = &mut found.skip {
        bound(skip, SKIP_BYTES, "a skip reason over 1 KiB was cut", said);
    }
    // Dropped, not cut: cut, it wouldn't be given back "as it was"
    // (Decisions, 26), and the host drops it the same way.
    if found.data.as_ref().is_some_and(|d| d.len() > DATA_BYTES) {
        found.data = None;
        said("a test's data over 4 KiB wasn't kept");
    }
    if let (Some(line), Some(end)) = (found.line, found.end_line) {
        if end < line {
            found.end_line = None;
        }
    }
}
