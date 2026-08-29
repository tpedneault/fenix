//! One match from a whole-document text search (see `render.rs`'s
//! `search_document`, which is what actually walks pdfium's per-page
//! search API to produce these). This module only defines the result
//! shape; `render.rs` owns the pdfium-specific search walk itself, same
//! "shape here, pdfium types there" split `outline.rs`/`render.rs` use
//! for the bookmark tree.

/// One occurrence of the search query somewhere in the document.
/// `char_index` is the query's first character's position within that
/// page's own character collection (`PdfPageText::all()`'s indexing) --
/// not currently used for anything beyond identifying *which* occurrence
/// this is on a page with several, since jumping only needs `page_index`
/// (see `App::pdf_jump_session_to_page`), but kept rather than discarded
/// since it's what pdfium's search API hands back for free and a future
/// "highlight the match on the page" feature would need it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PdfSearchMatch {
    pub page_index: u32,
    pub char_index: usize,
    pub context: String,
}
