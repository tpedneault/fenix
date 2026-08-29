//! Opens PDF files and rasterizes their pages to plain BGRA pixel
//! buffers, via `pdfium-render` (Google's PDFium, the PDF engine behind
//! Chrome). Deliberately has no `winit`/`wgpu` knowledge of its own, same
//! split this workspace already uses for `fenix-vnc` (pure protocol
//! logic) versus `fenix-gui` (owns the GPU texture upload and
//! `FenixUserEvent` wiring).
//!
//! One deliberate divergence from `fenix-vnc`'s own shape: `fenix-vnc`
//! spawns one dedicated OS thread *per connection*, because two
//! independent TCP sockets genuinely are independent. Pdfium's C API is
//! documented as unsafe to call concurrently, even across two entirely
//! separate open documents -- so unlike `fenix-vnc`, this crate spawns
//! exactly **one** background thread for the whole process's lifetime
//! (`PdfWorker`), serializing every request (across every open document)
//! through one channel. Opening a second PDF while a first is still open
//! is correct and cheap (documents are independent `PdfDocument` values
//! kept in one `HashMap` on that single thread), just never concurrent
//! with each other.
//!
//! `pdfium-render` is synchronous (unlike `vnc-rs`, which forced `fenix-
//! vnc` into owning a small tokio runtime) -- no async runtime is needed
//! or pulled in here at all.

mod render;
pub mod coords;
pub mod crop;

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc as std_mpsc;
use std::thread;

pub use render::PdfError;

/// Opaque identity for one open document, handed out by the caller (not
/// derived from the path) so a document can be closed and reopened, or
/// reopened under a different in-memory identity after a rename, without
/// any ambiguity about which `PdfRequest`/`PdfResponse` pair belongs to
/// which live session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PdfDocKey(u64);

impl PdfDocKey {
    pub fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

impl Default for PdfDocKey {
    fn default() -> Self {
        Self::new()
    }
}

/// One request to the worker thread. `RenderPage` carries its own
/// `request_id` (bumped by the caller on every dispatch) purely so a
/// caller can tell a stale reply apart from the one it actually still
/// wants -- see `PdfResponse::PageRendered`'s own doc comment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PdfRequest {
    Open { key: PdfDocKey, path: PathBuf },
    RenderPage { key: PdfDocKey, request_id: u64, page_index: u32, target_w: u32, target_h: u32 },
    Close { key: PdfDocKey },
}

/// One reply from the worker thread, handed to the caller's own
/// `response_sink` (see `PdfWorker::spawn`).
#[derive(Debug, Clone, PartialEq)]
pub enum PdfResponse {
    /// `page_width_pts`/`page_height_pts` are page 0's native size, in PDF
    /// points (1/72 inch) -- the caller needs these *before* it can even
    /// compute a fit-to-page/fit-width/percent render target (see
    /// `coords::fit_page_size`/`fit_width_size`/`percent_size`), and
    /// there's no render to piggyback them on yet at this point.
    Opened { key: PdfDocKey, page_count: u32, page_width_pts: f32, page_height_pts: f32 },
    OpenFailed { key: PdfDocKey, message: String },
    /// A page finished rendering, as a tightly-packed BGRA buffer of
    /// exactly `width` x `height` pixels (matches the `target_w`/
    /// `target_h` the triggering `RenderPage` asked for -- pdfium never
    /// changes the requested pixel size). `request_id` rides along so the
    /// caller can drop this if it no longer matches the *latest*
    /// `RenderPage` it dispatched for this session (e.g. several resize
    /// events fired before the first render came back) -- the worker
    /// itself already coalesces same-key `RenderPage` requests still
    /// sitting in its queue (see `coalesce_render_requests`), but a
    /// request already being rendered when a newer one arrives can't be
    /// un-dispatched, so this is the caller's own last line of defense
    /// against briefly showing a stale page/size.
    ///
    /// `page_width_pts`/`page_height_pts` are *this* page's own native
    /// size -- refreshes the caller's cached value from `Opened` (or an
    /// earlier `PageRendered`), which matters for the rare PDF whose pages
    /// aren't all the same size: the caller computes a page-turn's render
    /// target from whatever size it last knew about (there's no size to
    /// ask for ahead of the page actually being opened), so a mismatch
    /// here is the caller's cue to immediately dispatch a corrected
    /// re-render at the right target for the page it actually landed on.
    PageRendered { key: PdfDocKey, request_id: u64, page_index: u32, width: u32, height: u32, bgra: Vec<u8>, page_width_pts: f32, page_height_pts: f32 },
    RenderFailed { key: PdfDocKey, request_id: u64, message: String },
}

/// One shared background worker for every open PDF document in the
/// process -- see this module's own doc comment for why there's exactly
/// one, not one per document. Cheap to hold onto indefinitely; spawn it
/// once (lazily, on the first PDF a caller opens) and keep sending it
/// requests for as long as the process runs.
pub struct PdfWorker {
    /// `None` after `Drop` starts -- dropping the sender is what makes
    /// the worker thread's blocking `recv()` fail and the thread exit on
    /// its own, same "drop the thing that unblocks the background
    /// thread, then join" shape `fenix_vnc::VncSession`'s field-order
    /// trick uses, just explicit here since a plain field-declaration-
    /// order drop happens *after* `Drop::drop` runs, not before.
    sender: Option<std_mpsc::Sender<PdfRequest>>,
    thread: Option<thread::JoinHandle<()>>,
}

impl PdfWorker {
    /// Spawns the one dedicated worker thread. `response_sink` is how a
    /// reply gets back to the caller -- `fenix-gui` passes a closure that
    /// posts `FenixUserEvent::PdfResponse` onto its `EventLoopProxy`,
    /// mirroring `fenix_vnc`-backed `VncReader::spawn`'s own `send`
    /// callback shape. Called on every reply from whichever request
    /// triggered it, on this crate's own worker thread -- the sink itself
    /// must be safe to call from a background thread and should return
    /// quickly (an `EventLoopProxy::send_event` call is cheap).
    pub fn spawn(response_sink: impl Fn(PdfResponse) + Send + 'static) -> Self {
        let (sender, receiver) = std_mpsc::channel::<PdfRequest>();
        let thread = thread::spawn(move || render::run(receiver, response_sink));
        Self { sender: Some(sender), thread: Some(thread) }
    }

    /// Queues one request. A no-op if the worker's already been dropped
    /// (shouldn't happen in practice -- `PdfWorker` outlives every caller
    /// that holds a reference to it -- but matches this project's
    /// "degrade, don't panic" posture at every other best-effort send
    /// site).
    pub fn send(&self, request: PdfRequest) {
        if let Some(sender) = &self.sender {
            let _ = sender.send(request);
        }
    }
}

impl Drop for PdfWorker {
    fn drop(&mut self) {
        self.sender = None;
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Collapses a batch of requests drained from the channel in one go so
/// that, per `key`, only the *last* `RenderPage` request in the batch
/// survives -- everything before it is dropped silently (no
/// `RenderFailed` reply; the caller never sees these as having been
/// dispatched at all, since from its perspective a newer request for the
/// same session already supersedes them). `Open`/`Close` requests, and
/// `RenderPage` requests for a *different* key, are never touched or
/// reordered -- only same-key `RenderPage` entries are thinned out,
/// preserving every other request's original position. This is what
/// keeps a live window resize (which can queue many `RenderPage`
/// requests for the same session faster than pdfium can render them)
/// from visibly lagging behind: only the final, current pane size is
/// ever actually rendered.
fn coalesce_render_requests(pending: Vec<PdfRequest>) -> Vec<PdfRequest> {
    use std::collections::HashMap;
    let mut latest_render_index: HashMap<PdfDocKey, usize> = HashMap::new();
    for (i, req) in pending.iter().enumerate() {
        if let PdfRequest::RenderPage { key, .. } = req {
            latest_render_index.insert(*key, i);
        }
    }
    pending
        .into_iter()
        .enumerate()
        .filter(|(i, req)| !matches!(req, PdfRequest::RenderPage { key, .. } if latest_render_index.get(key) != Some(i)))
        .map(|(_, req)| req)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render_req(key: PdfDocKey, request_id: u64) -> PdfRequest {
        PdfRequest::RenderPage { key, request_id, page_index: 0, target_w: 100, target_h: 100 }
    }

    #[test]
    fn pdf_doc_key_hands_out_distinct_values() {
        assert_ne!(PdfDocKey::new(), PdfDocKey::new());
    }

    #[test]
    fn coalesce_keeps_only_the_last_render_request_per_key() {
        let key = PdfDocKey::new();
        let pending = vec![render_req(key, 1), render_req(key, 2), render_req(key, 3)];
        assert_eq!(coalesce_render_requests(pending), vec![render_req(key, 3)]);
    }

    #[test]
    fn coalesce_keeps_render_requests_for_different_keys_independent() {
        let a = PdfDocKey::new();
        let b = PdfDocKey::new();
        let pending = vec![render_req(a, 1), render_req(b, 1), render_req(a, 2)];
        assert_eq!(coalesce_render_requests(pending), vec![render_req(b, 1), render_req(a, 2)]);
    }

    #[test]
    fn coalesce_never_drops_open_or_close_requests() {
        let key = PdfDocKey::new();
        let pending = vec![
            PdfRequest::Open { key, path: PathBuf::from("a.pdf") },
            render_req(key, 1),
            render_req(key, 2),
            PdfRequest::Close { key },
        ];
        assert_eq!(
            coalesce_render_requests(pending),
            vec![PdfRequest::Open { key, path: PathBuf::from("a.pdf") }, render_req(key, 2), PdfRequest::Close { key }]
        );
    }

    #[test]
    fn coalesce_preserves_original_order_of_the_surviving_requests() {
        let a = PdfDocKey::new();
        let b = PdfDocKey::new();
        // Interleaved renders for two different sessions -- the surviving
        // entries stay in their original relative order, just thinned.
        let pending = vec![render_req(a, 1), render_req(b, 1), render_req(a, 2), render_req(b, 2)];
        assert_eq!(coalesce_render_requests(pending), vec![render_req(a, 2), render_req(b, 2)]);
    }
}
