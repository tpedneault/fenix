//! A Debug Adapter Protocol (DAP) client -- reuses `fenix_rpc`'s
//! `Content-Length` framing (the same wire-level transport LSP uses)
//! and the `debug-adapter-protocol` crate for typed request/response/
//! event shapes, the same "hand this to a crate with the large message
//! surface already modeled" departure from raw `serde_json::Value`
//! `fenix-lsp` already made for `lsp-types`. Deliberately GUI/event-
//! loop-agnostic -- the actual session state machine (launch handshake,
//! breakpoints, stepping, panel wiring) lives in `fenix-gui`.

mod client;
mod launch_config;

pub use client::{DapClient, DapEvent};
pub use debug_adapter_protocol::{events, requests, responses, types};
pub use launch_config::{read_launch_config, LaunchConfig};
