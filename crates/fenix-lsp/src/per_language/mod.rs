//! Per-language environment/root resolution -- the parts of "correctly
//! starting a language server for this project" that are inherently
//! specific to one language's own ecosystem conventions, not something
//! a single generic rule could cover. See `python`'s own doc comment
//! for why this lives here (in `fenix-lsp`) rather than in
//! `fenix-project`: it's about what a *server* needs to be told, not
//! project/root discovery in general.

pub mod python;
