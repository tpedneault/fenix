//! A PDF's bookmark ("outline") tree, flattened for display. Bookmarks
//! form a real tree (a bookmark can have children, which can have their
//! own children), but `fenix-gui` renders the outline as plain indented
//! lines in an ordinary Vim-navigable text buffer -- there's no tree
//! widget in this editor, so the tree needs to become a flat, order-
//! preserving list before it can be shown at all. `depth` is what lets
//! the renderer recover the nesting visually (via indentation) without
//! needing the tree structure itself.
//!
//! This module only defines the flat shape; the actual pdfium bookmark
//! walk that produces it lives in `render.rs` (it needs pdfium's own
//! `PdfBookmark` type, which has no reason to leak out of that module).

/// One flattened bookmark entry, in the same order a depth-first,
/// prefix-order walk of the bookmark tree would visit it (a parent
/// always comes immediately before its first child). `depth` is 0 for
/// a top-level bookmark, 1 for its direct children, and so on.
#[derive(Debug, Clone, PartialEq)]
pub struct OutlineEntry {
    pub title: String,
    pub page_index: u32,
    pub depth: u32,
}
