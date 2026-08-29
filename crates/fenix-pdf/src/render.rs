//! The actual pdfium-render calls -- kept out of `lib.rs` so the public
//! request/response protocol and the worker-thread loop shape are
//! readable without pdfium's own API in the way. Nothing here runs
//! outside `PdfWorker`'s one dedicated thread (see `lib.rs`'s own doc
//! comment for why there's exactly one).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;

use pdfium_render::prelude::*;

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
    loop {
        let Ok(first) = receiver.recv() else { return };
        let mut pending = vec![first];
        while let Ok(req) = receiver.try_recv() {
            pending.push(req);
        }
        for req in crate::coalesce_render_requests(pending) {
            handle_request(&pdfium, &mut docs, req, &sink);
        }
    }
}

fn handle_request<'a>(pdfium: &'a Pdfium, docs: &mut HashMap<PdfDocKey, PdfDocument<'a>>, req: PdfRequest, sink: &impl Fn(PdfResponse)) {
    match req {
        PdfRequest::Open { key, path } => match pdfium.load_pdf_from_file(&path, None) {
            Ok(doc) => {
                let page_count = doc.pages().len().max(0) as u32;
                docs.insert(key, doc);
                sink(PdfResponse::Opened { key, page_count });
            }
            Err(err) => sink(PdfResponse::OpenFailed { key, message: format!("{err:?}") }),
        },
        PdfRequest::RenderPage { key, request_id, page_index, target_w, target_h } => {
            let Some(doc) = docs.get(&key) else {
                sink(PdfResponse::RenderFailed { key, request_id, message: "document not open".to_string() });
                return;
            };
            match render_page(doc, page_index, target_w, target_h) {
                Ok((width, height, bgra)) => sink(PdfResponse::PageRendered { key, request_id, page_index, width, height, bgra }),
                Err(message) => sink(PdfResponse::RenderFailed { key, request_id, message }),
            }
        }
        PdfRequest::Close { key } => {
            docs.remove(&key);
        }
    }
}

/// Renders one page to a BGRA pixel buffer at exactly `target_w` x
/// `target_h` -- `PdfBitmapFormat::default()` (what `PdfPage::render`
/// uses when not told otherwise) is already `BGRA`, matching
/// `wgpu::TextureFormat::Bgra8Unorm` with no channel-swizzle needed,
/// same reasoning `fenix_vnc::do_handshake`'s `PixelFormat::bgra()`
/// already established for the VNC pipeline this mirrors.
fn render_page(doc: &PdfDocument, page_index: u32, target_w: u32, target_h: u32) -> Result<(u32, u32, Vec<u8>), String> {
    let page = doc.pages().get(page_index as i32).map_err(|err| format!("{err:?}"))?;
    let bitmap = page.render(target_w.max(1) as i32, target_h.max(1) as i32, None).map_err(|err| format!("{err:?}"))?;
    let width = bitmap.width().max(0) as u32;
    let height = bitmap.height().max(0) as u32;
    Ok((width, height, bitmap.as_raw_bytes()))
}
