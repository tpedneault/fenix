//! Host-agnostic autocompletion data: your symbols file, Tcl's built-in
//! names (for highlighting its calls), and the named places the symbol
//! pickers list. No knowledge of
//! `Buffer`/`Cursor`/rendering/fuzzy-matching at all -- `fenix-gui` wraps
//! `CompletionItem`s into `fenix_picker::Candidate`s and drives the actual
//! popup, the same way it already builds picker candidate lists for
//! find-file/switch-project from `fenix-project` data.
//!
//! A language with a real language server plugs in as a sibling
//! `CompletionKind` variant (`Lsp`, added once `fenix-gui` actually
//! wired up an LSP client) -- this crate deliberately stays a fixed set
//! of concrete sources rather than a speculative source-plugin trait,
//! since those are the only ones anything here actually implements.

pub mod ctags;
pub mod custom;
pub mod tcl;

/// Which source a `CompletionItem` came from -- drives both display
/// (color-coded per kind in the popup) and, later, ranking tie-breaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionKind {
    /// A predetermined built-in command/keyword for a language with no
    /// language server.
    Keyword,
    /// A user-defined name -- an entry from a user-supplied
    /// `custom::load` symbols file: "a name specific to you or your
    /// codebase," as opposed to `Keyword`'s built-in-language
    /// vocabulary, sharing one bucket with others like it rather than
    /// each getting their own popup color (only two colors are actually calibrated
    /// for legibility against the completion popup's background --
    /// see `fenix-gui`'s `completion_popup`).
    Tag,
    /// A real language server's own `textDocument/completion` result --
    /// shares `Tag`'s color rather than getting a third calibrated one
    /// (same reasoning `Tag`'s own doc comment gives: both represent
    /// "a name specific to this codebase," just from a more capable
    /// source).
    Lsp,
}

#[derive(Debug, Clone)]
pub struct CompletionItem {
    pub label: String,
    pub kind: CompletionKind,
    /// One line of context shown for the selected candidate, empty
    /// when the source has nothing to say (a symbols-file entry).
    pub detail: String,
}
