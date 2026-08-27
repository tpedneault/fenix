//! A minimal client for a self-hosted Jira Server/Data Center instance's
//! REST API -- read-only for now (search + single-issue fetch), enough
//! to back a browsing dashboard. Deliberately has no thread/event-loop
//! knowledge of its own, same split this workspace already uses for
//! `fenix-docker`/`fenix-git` (pure request/parsing logic) versus
//! `fenix-gui` (owns the background thread + `FenixUserEvent` wiring).

mod client;
mod issue;

pub use client::JiraClient;
pub use issue::{build_jql, Comment, IssueDetail, IssueSummary};
