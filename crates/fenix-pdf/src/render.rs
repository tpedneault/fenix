//! The actual pdfium-render calls -- kept out of `lib.rs` so the public
//! request/response protocol and the worker-thread loop shape are
//! readable without pdfium's own API in the way. Nothing here runs
//! outside `PdfWorker`'s one dedicated thread (see `lib.rs`'s own doc
//! comment for why there's exactly one).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;

use pdfium_render::prelude::*;

use crate::outline::OutlineEntry;
use crate::search::PdfSearchMatch;
use crate::{PdfDocKey, PdfRequest, PdfResponse};

/// Everything that can go wrong opening/using pdfium itself, as opposed
/// to a plain per-request failure already folded into `PdfResponse::
/// OpenFailed`/`RenderFailed` -- this is the "the library itself never
/// came up" case, checked once at worker startup.
#[derive(Debug)]
pub enum PdfError {
    /// No `pdfium` dynamic library could be found or loaded, by any of
    /// `locate_pdfium_library`'s candidate locations or the OS's own
    /// system library search. Every subsequent `Open` request fails with
    /// this same message rather than the worker thread ever panicking or
    /// exiting -- same "degrade gracefully, log/show, never crash"
    /// posture as every other optional external dependency in this
    /// project (see `README.md`'s "Optional external tools").
    LibraryNotFound,
}

impl std::fmt::Display for PdfError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PdfError::LibraryNotFound => write!(
                f,
                "pdfium library not found -- download it from https://github.com/bblanchon/pdfium-binaries \
                 and place it next to fenix.exe (or point FENIX_PDFIUM_PATH at its directory)"
            ),
        }
    }
}

/// Candidate locations for the `pdfium` dynamic library, checked in
/// order, before falling back to the OS's own system library search
/// (`Pdfium::bind_to_system_library`, tried by `bind_pdfium` itself once
/// neither of these finds anything): (1) the directory `fenix.exe` itself
/// runs from -- the documented "download it once, drop it next to the
/// exe" setup story (see `README.md`) -- then (2) `FENIX_PDFIUM_PATH`, an
/// env var override purely for dev convenience (switching between
/// `target/debug`/`target/release` without copying the library twice).
fn locate_pdfium_library() -> Option<PathBuf> {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let candidate = Pdfium::pdfium_platform_library_name_at_path(dir);
            if candidate.exists() {
                return Some(candidate);
            }
        }
    }
    if let Ok(dir) = std::env::var("FENIX_PDFIUM_PATH") {
        let candidate = Pdfium::pdfium_platform_library_name_at_path(Path::new(&dir));
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

/// Binds the pdfium library and wraps it in a `Pdfium` instance, trying
/// `locate_pdfium_library`'s explicit candidates first and the OS's own
/// system library search last. Dynamic linking only (this crate's
/// default `pdfium-render` feature set, no `static` feature) -- see this
/// crate's own top-level doc comment for why.
fn bind_pdfium() -> Result<Pdfium, PdfError> {
    if let Some(path) = locate_pdfium_library() {
        if let Ok(bindings) = Pdfium::bind_to_library(&path) {
            return Ok(Pdfium::new(bindings));
        }
    }
    Pdfium::bind_to_system_library().map(Pdfium::new).map_err(|_| PdfError::LibraryNotFound)
}

/// The worker thread's entire lifetime: binds pdfium once, then drains
/// `receiver` until the last `PdfWorker`/its sender is dropped. Every
/// currently-open document lives in `docs`, keyed by `PdfDocKey` -- kept
/// in this one function's stack frame (not returned or stored anywhere
/// else) because `PdfDocument<'a>` borrows from the `Pdfium` instance
/// that opened it, and that instance must outlive every document it
/// opened.
pub fn run(receiver: Receiver<PdfRequest>, sink: impl Fn(PdfResponse) + Send + 'static) {
    match bind_pdfium() {
        Ok(pdfium) => run_with_pdfium(receiver, sink, pdfium),
        Err(err) => run_without_pdfium(receiver, sink, err),
    }
}

/// Pdfium never came up (`PdfError::LibraryNotFound`) -- every `Open`
/// request fails immediately with that same reason, forever; there is
/// nothing to open/render/close since no document can ever be opened.
/// Still drains the channel (rather than returning immediately) so the
/// worker thread's lifetime matches the `run_with_pdfium` case -- it
/// exits on its own once the last `PdfWorker` (and its `Sender`) drops.
fn run_without_pdfium(receiver: Receiver<PdfRequest>, sink: impl Fn(PdfResponse), err: PdfError) {
    let message = err.to_string();
    while let Ok(req) = receiver.recv() {
        if let PdfRequest::Open { key, .. } = req {
            sink(PdfResponse::OpenFailed { key, message: message.clone() });
        }
    }
}

fn run_with_pdfium(receiver: Receiver<PdfRequest>, sink: impl Fn(PdfResponse) + Send + 'static, pdfium: Pdfium) {
    let mut docs: HashMap<PdfDocKey, PdfDocument> = HashMap::new();
    // The worker's own queue: everything waiting is read in before each
    // request is handled, so a scroll's `Cancel` or a newer render gets
    // a say before pdfium spends time on work nobody can see any more.
    let mut pending: Vec<PdfRequest> = Vec::new();
    loop {
        if pending.is_empty() {
            let Ok(first) = receiver.recv() else { return };
            pending.push(first);
        }
        while let Ok(req) = receiver.try_recv() {
            pending.push(req);
        }
        pending = crate::coalesce_render_requests(pending);
        if pending.is_empty() {
            continue;
        }
        let req = pending.remove(0);
        if let Some(rest) = handle_request(&pdfium, &mut docs, req, &sink) {
            pending.push(rest);
        }
    }
}

/// Pages a search looks through before letting other work go first.
const SEARCH_BATCH: u32 = 8;

/// Handles one request; what's left of a search comes back to be queued.
fn handle_request<'a>(pdfium: &'a Pdfium, docs: &mut HashMap<PdfDocKey, PdfDocument<'a>>, req: PdfRequest, sink: &impl Fn(PdfResponse)) -> Option<PdfRequest> {
    match req {
        PdfRequest::Open { key, path } => match pdfium.load_pdf_from_file(&path, None) {
            Ok(doc) => {
                let pages = doc.pages();
                // A size pdfium can't read (a malformed page) stands in as
                // US Letter, so the layout still has a place for it.
                let sizes = pages
                    .as_range()
                    .map(|i| pages.page_size(i).map(|rect| (rect.width().value, rect.height().value)).unwrap_or((612.0, 792.0)))
                    .collect();
                docs.insert(key, doc);
                sink(PdfResponse::Opened { key, pages: sizes });
            }
            Err(err) => sink(PdfResponse::OpenFailed { key, message: format!("{err:?}") }),
        },
        PdfRequest::RenderPage { key, request_id, page_index, target_w, target_h, .. } => {
            let Some(doc) = docs.get(&key) else {
                sink(PdfResponse::RenderFailed { key, request_id, message: "document not open".to_string() });
                return None;
            };
            match render_page(doc, page_index, target_w, target_h) {
                Ok((width, height, bgra, page_width_pts, page_height_pts)) => {
                    sink(PdfResponse::PageRendered { key, request_id, page_index, width, height, bgra, page_width_pts, page_height_pts })
                }
                Err(message) => sink(PdfResponse::RenderFailed { key, request_id, message }),
            }
        }
        PdfRequest::FetchOutline { key } => {
            // No document open under this key is a no-op rather than an
            // error reply -- there's no `PdfResponse` variant for "outline
            // fetch failed" since this can only happen from a caller bug
            // (asking before `Open`/after `Close`), never from anything a
            // real PDF file's content could trigger.
            if let Some(doc) = docs.get(&key) {
                let entries = flatten_bookmarks(doc.bookmarks());
                sink(PdfResponse::Outline { key, entries });
            }
        }
        PdfRequest::Search { key, request_id, query, match_case, from_page } => {
            // No document under this key is a caller bug, not a search
            // that found nothing, so there's no reply.
            let doc = docs.get(&key)?;
            let count = doc.pages().len() as u32;
            let to = from_page.saturating_add(SEARCH_BATCH).min(count);
            let matches = search_pages(doc, &query, match_case, from_page..to);
            let done = to >= count;
            sink(PdfResponse::SearchResults { key, request_id, matches, done });
            return (!done).then_some(PdfRequest::Search { key, request_id, query, match_case, from_page: to });
        }
        PdfRequest::Close { key } => {
            docs.remove(&key);
        }
        // Only ever acted on in the queue, by `coalesce_render_requests`.
        PdfRequest::Cancel { .. } => {}
        PdfRequest::Text { key, page } => {
            let doc = docs.get(&key)?;
            sink(PdfResponse::Text { key, page, chars: page_chars(doc, page) });
        }
        PdfRequest::Links { key, page } => {
            let doc = docs.get(&key)?;
            sink(PdfResponse::Links { key, page, links: page_links(doc, page) });
        }
    }
    None
}

/// Walks a document's bookmark tree depth-first, prefix-order (a parent
/// is always emitted immediately before its first child), producing the
/// flat `Vec<OutlineEntry>` `fenix-gui` renders as indented lines. Written
/// as an explicit recursive walk over `first_child`/`next_sibling` rather
/// than `PdfBookmarks::iter()` because the built-in iterator doesn't
/// expose each bookmark's tree depth, which is exactly what indentation
/// needs -- this walk tracks it directly as it descends/moves sideways.
///
/// A bookmark whose destination can't be resolved to a page (e.g. an
/// external URL action instead of an in-document jump) is omitted from
/// the result -- there's nothing for `Enter` to jump to, so showing it
/// would be a dead entry. Its children, if any, are still walked and can
/// still appear (just one level deeper than a visible parent, in the
/// rare case the parent itself had to be dropped).
///
/// Guards against a cyclic bookmark graph (malformed PDFs can have one)
/// the same way `pdfium-render`'s own `PdfBookmarksIterator` does: a
/// `HashSet` of already-visited bookmarks (`PdfBookmark` is `Hash`/`Eq`
/// by its underlying handle) stops the walk from recursing forever.
fn flatten_bookmarks(bookmarks: &PdfBookmarks) -> Vec<OutlineEntry> {
    let mut out = Vec::new();
    if let Some(root) = bookmarks.root() {
        let mut visited = HashSet::new();
        flatten_bookmark(root, 0, &mut out, &mut visited);
    }
    out
}

fn flatten_bookmark<'a>(bookmark: PdfBookmark<'a>, depth: u32, out: &mut Vec<OutlineEntry>, visited: &mut HashSet<PdfBookmark<'a>>) {
    if !visited.insert(bookmark.clone()) {
        return;
    }
    if let Some(page_index) = bookmark.destination().and_then(|dest| dest.page_index().ok()) {
        out.push(OutlineEntry { title: bookmark.title().unwrap_or_default(), page_index: page_index.max(0) as u32, depth });
    }
    if let Some(child) = bookmark.first_child() {
        flatten_bookmark(child, depth + 1, out, visited);
    }
    if let Some(sibling) = bookmark.next_sibling() {
        flatten_bookmark(sibling, depth, out, visited);
    }
}

/// Searches every page of `doc`, in page order, for `query`, using
/// pdfium's own per-page text search (`PdfPageText::search`) rather than
/// hand-rolled substring matching over extracted text -- pdfium's search
/// already accounts for how its own text extraction represents ligatures
/// and inter-character spacing, which a naive `str::find` over `PdfPage
/// Text::all()` could get subtly wrong for the exact same query a human
/// typed. Case-insensitive substring matching (`PdfSearchOptions`'s
/// defaults) -- the same forgiving posture as most simple search UIs.
///
/// Deliberately extracts and holds each page's full text (`PdfPageText::
/// all()`) only for as long as that one page's search takes, not across
/// the whole document or between separate `Search` requests -- see this
/// crate's plan/README reasoning: a large PDF's full extracted text is a
/// real memory cost for a feature used occasionally, so nothing here is
/// cached beyond a single page during a single search call.
///
/// A page whose text collection or search can't be created (`Result::
/// Err`, e.g. a malformed page) is skipped rather than aborting the
/// whole-document search -- one bad page shouldn't hide matches on every
/// other page.
fn search_pages(doc: &PdfDocument, query: &str, match_case: bool, pages: std::ops::Range<u32>) -> Vec<PdfSearchMatch> {
    if query.trim().is_empty() {
        return Vec::new();
    }
    let options = PdfSearchOptions::new().match_case(match_case);
    let mut matches = Vec::new();
    for page_index in pages {
        let Ok(page) = doc.pages().get(page_index as i32) else { continue };
        let page_h = page.height().value;
        let Ok(text_page) = page.text() else { continue };
        let Ok(search) = text_page.search(query, &options) else { continue };
        let page_chars: Vec<char> = text_page.all().chars().collect();
        while let Some(segments) = search.find_next() {
            let char_index = segments.first().ok().and_then(|segment| segment.chars().ok()).and_then(|chars| chars.first_char_index());
            let Some(char_index) = char_index else { continue };
            // pdfium's rects have their origin at the page's bottom-left.
            let rects = segments
                .iter()
                .map(|segment| {
                    let b = segment.bounds();
                    [b.left().value, page_h - b.top().value, b.right().value, page_h - b.bottom().value]
                })
                .collect();
            let context = build_context(&page_chars, char_index, query.chars().count());
            matches.push(PdfSearchMatch { page_index, char_index, context, rects });
        }
    }
    matches
}

/// A page's characters in reading order, each with its box in points from
/// the page's top-left. Empty when pdfium can't read the page's text.
fn page_chars(doc: &PdfDocument, page_index: u32) -> Vec<crate::text::PageChar> {
    let Ok(page) = doc.pages().get(page_index as i32) else { return Vec::new() };
    let page_h = page.height().value;
    let Ok(text) = page.text() else { return Vec::new() };
    text.chars()
        .iter()
        .filter_map(|ch| {
            let c = ch.unicode_char()?;
            let c = if c == '\r' { '\n' } else { c };
            let rect = match ch.loose_bounds() {
                Ok(b) if c != '\n' => [b.left().value, page_h - b.top().value, b.right().value, page_h - b.bottom().value],
                _ => [0.0; 4],
            };
            Some(crate::text::PageChar { c, rect })
        })
        // pdfium ends a line with "\r\n": one break is enough.
        .fold(Vec::new(), |mut out: Vec<crate::text::PageChar>, ch| {
            if !(ch.c == '\n' && out.last().is_some_and(|last| last.c == '\n')) {
                out.push(ch);
            }
            out
        })
}

/// A page's links, with where each goes, in points from its top-left.
fn page_links(doc: &PdfDocument, page_index: u32) -> Vec<crate::text::PageLink> {
    use crate::text::{LinkTarget, PageLink};
    let Ok(page) = doc.pages().get(page_index as i32) else { return Vec::new() };
    let page_h = page.height().value;
    let links = page.links();
    links
        .iter()
        .filter_map(|link| {
            let b = link.rect().ok()?;
            let rect = [b.left().value, page_h - b.top().value, b.right().value, page_h - b.bottom().value];
            let to_page = |dest: PdfDestination| {
                let page = dest.page_index().ok()? as u32;
                let y = match dest.view_settings() {
                    Ok(PdfDestinationViewSettings::SpecificCoordinatesAndZoom(_, y, _) | PdfDestinationViewSettings::FitPageHorizontallyToWindow(y)) => y.map(|y| {
                        let target_h = doc.pages().page_size(page as i32).map(|r| r.height().value).unwrap_or(page_h);
                        target_h - y.value
                    }),
                    _ => None,
                };
                Some(LinkTarget::Page { page, y })
            };
            let to = match link.action() {
                Some(PdfAction::Uri(uri)) => LinkTarget::Uri(uri.uri().ok()?),
                Some(PdfAction::LocalDestination(local)) => to_page(local.destination().ok()?)?,
                _ => to_page(link.destination()?)?,
            };
            Some(PageLink { rect, to })
        })
        .collect()
}

/// Builds a short, single-line context snippet around one match, from a
/// page's full character sequence (already collected by the caller, so
/// this stays a pure, panic-free, unit-testable function with no pdfium
/// dependency of its own -- same "pull the pure math out" split `coords.
/// rs`/`crop.rs` already use). Collapses every run of whitespace
/// (including the newlines `PdfPageText::all()` inserts between visual
/// lines) down to a single space, since a raw multi-line snippet would
/// otherwise be unreadable as one row in the results-list buffer.
fn build_context(chars: &[char], char_index: usize, match_len: usize) -> String {
    const PAD: usize = 30;
    let start = char_index.saturating_sub(PAD);
    let end = (char_index.saturating_add(match_len).saturating_add(PAD)).min(chars.len());
    if start >= end {
        return String::new();
    }
    chars[start..end].iter().collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Renders one page to a BGRA pixel buffer at exactly `target_w` x
/// `target_h`, matching `wgpu::TextureFormat::Bgra8Unorm` with no
/// channel-swizzle needed, same reasoning `fenix_vnc::do_handshake`'s
/// `PixelFormat::bgra()` already established for the VNC pipeline this
/// mirrors.
///
/// The `set_reverse_byte_order(false)` is what actually makes that true,
/// and is not optional: `PdfRenderConfig::new()` (which the convenience
/// `PdfPage::render` uses internally, with no way to opt out) defaults
/// that flag to *true*, i.e. it asks pdfium for `FPDF_REVERSE_BYTE_
/// ORDER` so `pdfium-render`'s own `image` interop gets RGBA. The
/// bitmap still *reports* `PdfBitmapFormat::BGRA` in that case --
/// `FPDFBitmap_GetFormat` describes how the buffer was allocated, not
/// the order pdfium ended up writing into it -- so the mismatch is
/// completely silent, and `as_raw_bytes()` hands back RGBA bytes that
/// look like a valid BGRA buffer. Uploaded to a `Bgra8Unorm` texture
/// that swaps red and blue on every pixel: a blue cover page renders
/// orange. Rendering through an explicit config with the flag off (into
/// a bitmap sized exactly like `PdfPage::render` would have sized it) is
/// what keeps this crate's documented "always BGRA" contract honest, at
/// no cost -- pdfium writes the bytes we want directly, rather than this
/// crate swizzling a multi-megabyte buffer after the fact.
///
/// `target_w`/`target_h` is *not* necessarily aspect-correct for the
/// page on its own -- pdfium stretches the page content to fill exactly
/// the pixel box it's asked for, so getting an undistorted fit/zoom
/// depends on the *caller* computing an aspect-correct `target_w`/
/// `target_h` in the first place (see `coords::fit_page_size`/`fit_width_
/// size`/`percent_size`), not on anything this function does.
fn render_page(doc: &PdfDocument, page_index: u32, target_w: u32, target_h: u32) -> Result<(u32, u32, Vec<u8>, f32, f32), String> {
    let page = doc.pages().get(page_index as i32).map_err(|err| format!("{err:?}"))?;
    let (page_width_pts, page_height_pts) = (page.width().value, page.height().value);
    let (target_w, target_h) = (target_w.max(1) as i32, target_h.max(1) as i32);
    let mut bitmap = PdfBitmap::empty(target_w, target_h, PdfBitmapFormat::BGRA).map_err(|err| format!("{err:?}"))?;
    let config = PdfRenderConfig::new()
        .set_target_width(target_w)
        .set_target_height(target_h)
        .set_reverse_byte_order(false);
    page.render_into_bitmap_with_config(&mut bitmap, &config).map_err(|err| format!("{err:?}"))?;
    let width = bitmap.width().max(0) as u32;
    let height = bitmap.height().max(0) as u32;
    Ok((width, height, bitmap.as_raw_bytes(), page_width_pts, page_height_pts))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_context_pads_around_the_match_and_collapses_a_line_break_to_one_space() {
        let text = "the quick brown fox\njumps over the lazy dog";
        let chars: Vec<char> = text.chars().collect();
        let char_index = text.find("fox").unwrap();
        assert_eq!(build_context(&chars, char_index, 3), "the quick brown fox jumps over the lazy dog");
    }

    #[test]
    fn build_context_clamps_to_the_start_and_end_of_a_short_text_without_panicking() {
        let chars: Vec<char> = "hi".chars().collect();
        assert_eq!(build_context(&chars, 0, 2), "hi");
    }

    #[test]
    fn build_context_on_an_out_of_range_index_returns_empty_rather_than_panicking() {
        let chars: Vec<char> = "short".chars().collect();
        assert_eq!(build_context(&chars, 100, 5), "");
    }

}
