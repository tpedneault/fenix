//! A real LSP client: spawns one language server process, performs the
//! `initialize`/`initialized` handshake, keeps document state in sync,
//! and lets a host send requests/receive notifications (diagnostics,
//! completion, hover, etc.). Deliberately has no GUI/rendering knowledge
//! of its own, same split this workspace already uses for `fenix-vnc`/
//! `fenix-terminal` (pure protocol/process logic) versus `fenix-gui`
//! (owns the background reader thread wiring into `FenixUserEvent`,
//! though here the *decoding* itself already happens inside this crate
//! -- see `LspClient`'s own doc comment for why that differs from
//! `fenix-terminal`'s shape).
//!
//! Wire types come from the `lsp-types` crate (used by rust-analyzer
//! itself) rather than hand-rolled structs -- LSP's message surface is
//! large enough (dozens of request/notification shapes) that hand-
//! parsing every one via raw `serde_json::Value` indexing, the way
//! `fenix-docker`/`fenix-jira` do for their much smaller JSON surfaces,
//! would be significantly more error-prone. The JSON-RPC envelope itself
//! (the `id`/`method`/`params`/`result`/`error` wrapper) is *not* part
//! of `lsp-types` -- that's `envelope.rs`, kept deliberately small and
//! separate rather than pulling in a crate like `lsp-server`, whose own
//! opinionated stdio main-loop would compete with this crate's own
//! thread ownership.

mod client;
mod envelope;

pub use client::{LspClient, LspEvent};
pub use envelope::ResponseError;
