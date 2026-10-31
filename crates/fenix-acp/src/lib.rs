//! The Agent Client Protocol, client side: Fenix talking to a coding
//! agent (Claude Code's adapter, Codex's, `devin acp`, `gemini --acp`)
//! over the agent's stdio. Host-agnostic like `fenix-lsp`: no window,
//! no event loop; `fenix-gui` wires it in. See
//! `docs/plans/agents-acp.md` and `docs/adr/0002-agents-acp.md`.
//!
//! - `protocol`: ACP v1's messages, read leniently.
//! - `client`: one agent process and its stdio.
//! - `thread`: the reducer a conversation is built by.
//! - `hub`: sessions -- which thread is bound to which agent session,
//!   attaching, resuming, crashes -- as a pure state machine.
//! - `store`: threads kept on disk.
//! - `catalog`: the agents Fenix knows how to start.
//! - `mock`: an agent for tests, playing Claude Code's adapter.

pub mod catalog;
pub mod client;
pub mod driver;
pub mod hub;
pub mod mock;
pub mod protocol;
pub mod registry;
pub mod store;
pub mod thread;
