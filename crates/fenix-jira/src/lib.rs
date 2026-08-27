//! A minimal client for a self-hosted Jira Server/Data Center instance's
//! REST API -- read-only for now (search + single-issue fetch), enough
//! to back a browsing dashboard. Deliberately has no thread/event-loop
//! knowledge of its own, same split this workspace already uses for
//! `fenix-docker`/`fenix-git` (pure request/parsing logic) versus
//! `fenix-gui` (owns the background thread + `FenixUserEvent` wiring).
//!
//! TLS trusts the OS's own certificate store (`ureq`'s `native-certs`
//! feature, backed by `rustls-native-certs`), not just the public
//! Mozilla root bundle `ureq`'s `tls` feature uses on its own -- a
//! self-hosted instance's certificate is routinely issued by a
//! corporate-internal CA that only the OS trusts (pushed via group
//! policy, say), never a publicly-trusted one. Without this, every
//! request fails at the TLS handshake with a bare `ureq::Error::
//! Transport`, surfacing to the user as an unhelpfully generic "request
//! failed" -- this was a real bug, not just a theoretical gap (see the
//! `Cargo.toml` history for the exact incident).

mod client;
mod issue;

pub use client::JiraClient;
pub use issue::{build_jql, Comment, IssueDetail, IssueSummary};
