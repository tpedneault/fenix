//! `fenix-pdfium`: the PDF extension's companion program. The extension
//! (`fenix.pdf`) runs in WebAssembly, and PDFium doesn't
//! (docs/adr/0001-wasm-runtime.md), so this program renders for it: it
//! reads requests on stdin and writes replies on stdout ([`protocol`]),
//! and does the work with `fenix-pdf`, exactly as Fenix 1.x's built-in
//! reader did, with the same `pdfium.dll` beside it.
//!
//! It's built and shipped with Fenix, beside `fenix.exe`; the extension
//! finds it with `find-program`.

pub mod protocol;

use fenix_pdf::{PdfDocKey, PdfRequest, PdfResponse};
use protocol::{Heading, Link, Match, Reply, Request, Target};

/// A request as `fenix-pdf`'s worker takes it. Closing and opening are
/// the caller's to note for the file watch too.
pub fn to_worker(request: Request) -> PdfRequest {
    let key = |doc: u64| PdfDocKey::from_raw(doc);
    match request {
        Request::Open { doc, path } => PdfRequest::Open { key: key(doc), path: path.into() },
        Request::Close { doc } => PdfRequest::Close { key: key(doc) },
        Request::Render { doc, view, id, page, page_w, page_h, x, y, w, h } => PdfRequest::RenderRegion { key: key(doc), view, request_id: id, page_index: page, page_w, page_h, x, y, w, h },
        Request::CancelBefore { doc, view, before } => PdfRequest::CancelBefore { key: key(doc), view, before },
        Request::Outline { doc } => PdfRequest::FetchOutline { key: key(doc) },
        Request::Text { doc, page } => PdfRequest::Text { key: key(doc), page },
        Request::Links { doc, page } => PdfRequest::Links { key: key(doc), page },
        Request::Search { doc, id, query, match_case } => PdfRequest::Search { key: key(doc), request_id: id, query, match_case, from_page: 0 },
    }
}

/// A worker's reply as the extension hears it; none for the kinds this
/// program never asks for (whole-page renders).
pub fn from_worker(response: PdfResponse) -> Option<Reply> {
    Some(match response {
        PdfResponse::Opened { key, pages } => Reply::Opened { doc: key.raw(), pages },
        PdfResponse::OpenFailed { key, message } => Reply::OpenFailed { doc: key.raw(), message },
        PdfResponse::RegionRendered { key, view, request_id, page_index, x, y, w, h, bgra } => Reply::Rendered { doc: key.raw(), view, id: request_id, page: page_index, x, y, w, h, bgra },
        PdfResponse::RenderFailed { key, request_id, message } => Reply::RenderFailed { doc: key.raw(), id: request_id, message },
        PdfResponse::Outline { key, entries } => Reply::Outline { doc: key.raw(), headings: entries.into_iter().map(|e| Heading { title: e.title, page: e.page_index, depth: e.depth }).collect() },
        PdfResponse::Text { key, page, chars } => Reply::Text { doc: key.raw(), page, chars: chars.into_iter().map(|c| (c.c, c.rect)).collect() },
        PdfResponse::Links { key, page, links } => Reply::Links {
            doc: key.raw(),
            page,
            links: links
                .into_iter()
                .map(|l| Link {
                    rect: l.rect,
                    to: match l.to {
                        fenix_pdf::text::LinkTarget::Page { page, y } => Target::Page { page, y },
                        fenix_pdf::text::LinkTarget::Uri(uri) => Target::Uri(uri),
                    },
                })
                .collect(),
        },
        PdfResponse::SearchResults { key, request_id, matches, done } => {
            Reply::Found { doc: key.raw(), id: request_id, done, matches: matches.into_iter().map(|m| Match { page: m.page_index, context: m.context, rects: m.rects }).collect() }
        }
        PdfResponse::PageRendered { .. } => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_and_replies_go_through_the_worker_unchanged() {
        let r = to_worker(Request::Render { doc: 4, view: 1, id: 9, page: 2, page_w: 600, page_h: 800, x: 0, y: 10, w: 600, h: 20 });
        assert_eq!(r, PdfRequest::RenderRegion { key: PdfDocKey::from_raw(4), view: 1, request_id: 9, page_index: 2, page_w: 600, page_h: 800, x: 0, y: 10, w: 600, h: 20 });
        let back = from_worker(PdfResponse::SearchResults { key: PdfDocKey::from_raw(4), request_id: 3, matches: vec![fenix_pdf::search::PdfSearchMatch { page_index: 1, char_index: 0, context: "x".into(), rects: vec![[0.0; 4]] }], done: true });
        assert_eq!(back, Some(Reply::Found { doc: 4, id: 3, done: true, matches: vec![Match { page: 1, context: "x".into(), rects: vec![[0.0; 4]] }] }));
        assert_eq!(from_worker(PdfResponse::PageRendered { key: PdfDocKey::from_raw(4), request_id: 1, page_index: 0, width: 1, height: 1, bgra: vec![], page_width_pts: 1.0, page_height_pts: 1.0 }), None);
    }
}
