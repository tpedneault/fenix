//! `fenix-forge`'s `Forge` trait, over GitLab's `/api/v4`.
//!
//! Shaped like `fenix-jira`'s own client -- `ureq`, one shared
//! authenticated-request helper every endpoint funnels through, raw
//! `serde_json::Value` in and out rather than derived structs -- with
//! GitLab's `PRIVATE-TOKEN` header in place of Jira's bearer scheme.
//! The parsing lives in `parse`, so every response shape is testable
//! against a literal payload with no instance to point at.
//!
//! The only configuration is a base URL and a token: which project a
//! checkout belongs to is worked out from its `origin` remote (see
//! `remote`), because the checkout already knows.
//!
//! Endpoint paths and field names here were checked against GitLab's
//! own API documentation rather than recalled -- including the one that
//! has actually moved: the file list is `/diffs`, with `/changes`
//! deprecated in GitLab 15.7. `changed_files` tries the current path
//! and falls back, so a self-hosted instance older than that still
//! works.

pub mod parse;
pub mod remote;

use fenix_forge::{Approvals, ChangedFile, Forge, MergeRequest, MrFilter};
use serde_json::Value;

/// How many merge requests one listing asks for. Past this the pane
/// isn't being read any more, it's being scrolled -- and the filter
/// (`mine`, `for me`) is the answer to a long list, not pagination.
const PER_PAGE: usize = 100;

pub struct GitLab {
    base_url: String,
    token: String,
    /// `group/sub/project`, as it appears in a URL.
    project: String,
    /// The same, percent-encoded for the `:id` path segment.
    encoded: String,
}

impl GitLab {
    /// A client for `project` on the instance at `base_url`.
    ///
    /// `base_url` is the instance root (`https://gitlab.example.com`),
    /// not the API root -- `/api/v4` is this crate's business, and
    /// making the user get it right in `config.ini` would be one more
    /// way for the setup to fail silently.
    pub fn new(base_url: impl Into<String>, token: impl Into<String>, project: impl Into<String>) -> Self {
        let base_url = base_url.into();
        let project = project.into();
        let encoded = remote::encode_project(&project);
        GitLab { base_url: base_url.trim_end_matches('/').to_string(), token: token.into(), project, encoded }
    }

    /// A client for whichever project `remote_url` points at, or `None`
    /// when that URL isn't a GitLab project URL this can read.
    pub fn from_remote(base_url: impl Into<String>, token: impl Into<String>, remote_url: &str) -> Option<Self> {
        remote::project_path(remote_url).map(|project| GitLab::new(base_url, token, project))
    }

    fn get(&self, path: &str, query: &[(&str, &str)]) -> Result<Value, String> {
        let url = format!("{}/api/v4{path}", self.base_url);
        let mut req = ureq::get(&url).set("PRIVATE-TOKEN", &self.token).set("Accept", "application/json");
        for (key, value) in query {
            req = req.query(key, value);
        }
        let response = req.call().map_err(|err| describe_error(&err))?;
        let body = response.into_string().map_err(|err| format!("couldn't read response body: {err}"))?;
        serde_json::from_str(&body).map_err(|err| format!("couldn't parse response as JSON: {err}"))
    }

    fn mr_path(&self, number: u64, suffix: &str) -> String {
        format!("/projects/{}/merge_requests/{number}{suffix}", self.encoded)
    }
}

impl Forge for GitLab {
    fn project(&self) -> &str {
        &self.project
    }

    fn list_merge_requests(&self, filter: MrFilter) -> Result<Vec<MergeRequest>, String> {
        let per_page = PER_PAGE.to_string();
        // `scope` needs `all` alongside `created_by_me` to search the
        // whole project rather than only what the token's own user can
        // already see listed; the state filter is separate from it.
        let mut query: Vec<(&str, &str)> =
            vec![("state", "opened"), ("per_page", per_page.as_str()), ("order_by", "updated_at"), ("sort", "desc")];
        match filter {
            MrFilter::Mine => query.push(("scope", "created_by_me")),
            MrFilter::ForMe => query.push(("scope", "assigned_to_me")),
            MrFilter::AllOpen => query.push(("scope", "all")),
        }
        let value = self.get(&format!("/projects/{}/merge_requests", self.encoded), &query)?;
        let entries = value.as_array().ok_or_else(|| "expected a list of merge requests".to_string())?;
        Ok(entries.iter().filter_map(parse::merge_request).collect())
    }

    fn merge_request(&self, number: u64) -> Result<MergeRequest, String> {
        let value = self.get(&self.mr_path(number, ""), &[])?;
        parse::merge_request(&value).ok_or_else(|| format!("!{number} came back without an iid"))
    }

    fn approvals(&self, number: u64) -> Result<Approvals, String> {
        self.get(&self.mr_path(number, "/approvals"), &[]).map(|v| parse::approvals(&v))
    }

    fn changed_files(&self, number: u64) -> Result<Vec<ChangedFile>, String> {
        let per_page = PER_PAGE.to_string();
        // `/diffs` is the current path; `/changes` was deprecated in
        // GitLab 15.7 but is all an older self-hosted instance has, and
        // returns the same entry shape under a `changes` key. Trying
        // the current one first means a modern instance never pays for
        // the fallback.
        let current = self.get(&self.mr_path(number, "/diffs"), &[("per_page", per_page.as_str())]);
        let entries = match &current {
            Ok(value) => value.as_array().cloned(),
            Err(_) => None,
        };
        let entries = match entries {
            Some(entries) => entries,
            None => {
                let legacy = self.get(&self.mr_path(number, "/changes"), &[]).map_err(|err| match &current {
                    // Report the modern endpoint's own error: on an
                    // instance that has `/diffs`, a failure there (401,
                    // 404) is the real problem, and the fallback's
                    // error would just describe the same thing twice.
                    Err(first) => first.clone(),
                    Ok(_) => err,
                })?;
                legacy.get("changes").and_then(Value::as_array).cloned().unwrap_or_default()
            }
        };
        Ok(entries.iter().filter_map(parse::changed_file).collect())
    }

    fn checkout_refspec(&self, number: u64) -> String {
        // GitLab publishes every merge request's head under this ref on
        // the project's own remote, so no fork remote has to be added
        // to review a fork-sourced request.
        format!("refs/merge-requests/{number}/head:mr-{number}")
    }
}

/// Mirrors `fenix_jira::client::describe_error` -- a status code plus
/// the server's own status text beats `Display`'s "Status code 401",
/// and a transport failure (DNS, TLS, refused) falls back to its own.
fn describe_error(err: &ureq::Error) -> String {
    match err {
        ureq::Error::Status(code, response) => format!("HTTP {code} ({})", response.status_text()),
        ureq::Error::Transport(transport) => format!("request failed: {transport}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_base_url_loses_a_trailing_slash_and_never_carries_the_api_path() {
        let gl = GitLab::new("https://gitlab.example.com/", "tok", "group/project");
        assert_eq!(gl.base_url, "https://gitlab.example.com");
        // `/api/v4` is this crate's business, not the user's.
        assert!(!gl.base_url.contains("api"));
    }

    #[test]
    fn the_project_is_encoded_once_for_the_url_and_kept_readable_for_display() {
        let gl = GitLab::new("https://gitlab.example.com", "tok", "group/sub/project");
        assert_eq!(gl.project(), "group/sub/project", "what a pane heading shows");
        assert_eq!(gl.mr_path(42, "/approvals"), "/projects/group%2Fsub%2Fproject/merge_requests/42/approvals");
    }

    #[test]
    fn a_client_can_be_built_straight_from_an_origin_url() {
        let gl = GitLab::from_remote("https://gitlab.example.com", "tok", "git@gitlab.example.com:group/project.git").unwrap();
        assert_eq!(gl.project(), "group/project");
        assert!(GitLab::from_remote("https://gitlab.example.com", "tok", "not a url").is_none());
    }

    #[test]
    fn the_checkout_refspec_lands_on_a_branch_named_after_the_request() {
        let gl = GitLab::new("https://gitlab.example.com", "tok", "g/p");
        // GitLab's own published ref, so reviewing a fork-sourced
        // request needs no extra remote.
        assert_eq!(gl.checkout_refspec(42), "refs/merge-requests/42/head:mr-42");
    }
}
