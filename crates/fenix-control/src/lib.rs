//! Fenix's control channel: how a later `fenix` launch, or any tool the
//! user runs, asks the running Fenix to do something -- open files at a
//! line, open a project, say what it's showing.
//!
//! - [`endpoint`]: where it listens, reachable by this user in this
//!   login session only (a named pipe on Windows, a Unix socket in a
//!   private directory on Linux).
//! - [`protocol`]: JSON-RPC 2.0 over `Content-Length` framing, the same
//!   envelope LSP and DAP use; the reference is `docs/CONTROL.md`.
//! - [`server`]: one connection's protocol, up to the requests only the
//!   editor can answer, which it hands back as a [`protocol::Call`].
//! - [`client`]: the other end, which `fenix file:12` uses.
//!
//! Nothing here knows about windows or the event loop: `fenix-gui`
//! runs the accept loop and answers each call on its UI thread.

pub mod client;
pub mod endpoint;
pub mod protocol;
pub mod server;

pub use client::{Client, ClientError};
pub use endpoint::{BindError, Endpoint, Listener, Stream};
pub use protocol::{Call, OpenParams, OpenPath, RpcError, StateResult};
pub use server::{serve, ServerInfo};
