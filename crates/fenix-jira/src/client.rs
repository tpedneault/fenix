/// A thin authenticated-GET client for a self-hosted Jira Server/Data
/// Center instance -- `Authorization: Bearer {token}` (the real PAT
/// convention for Server/DC; Cloud's different `email:token` Basic-auth
/// scheme is out of scope here, matching "self-hosted" in what this was
/// built for). `base_url` has no trailing slash requirement -- `request`
/// normalizes it.
pub struct JiraClient {
    base_url: String,
    token: String,
}

impl JiraClient {
    pub fn new(base_url: impl Into<String>, token: impl Into<String>) -> Self {
        let base_url = base_url.into();
        Self { base_url: base_url.trim_end_matches('/').to_string(), token: token.into() }
    }

    /// Sends an authenticated `GET {base_url}{path}?query...` and parses
    /// the JSON body -- the one shared low-level helper every endpoint
    /// function in this crate funnels through, mirroring `fenix_git::
    /// process::run_action`'s own "one shared shell-out, every public
    /// function built on it" shape. Reads the body as a plain string and
    /// parses it via `serde_json` directly (rather than `ureq`'s own
    /// `into_json`) so this crate doesn't need `ureq`'s `json` feature
    /// on top of `tls`.
    pub(crate) fn request(&self, path: &str, query: &[(&str, &str)]) -> Result<serde_json::Value, String> {
        let url = format!("{}{path}", self.base_url);
        let mut req = ureq::get(&url).set("Authorization", &format!("Bearer {}", self.token)).set("Accept", "application/json");
        for (key, value) in query {
            req = req.query(key, value);
        }
        let response = req.call().map_err(|err| describe_error(&err))?;
        let body = response.into_string().map_err(|err| format!("couldn't read response body: {err}"))?;
        serde_json::from_str(&body).map_err(|err| format!("couldn't parse response as JSON: {err}"))
    }

    /// The write-side counterpart to `request` -- `method` is a plain
    /// verb (`"POST"`/`"PUT"`), `body` is serialized by hand (`serde_
    /// json::to_string`, not `ureq`'s own `send_json`) so this crate
    /// still doesn't need `ureq`'s `json` feature on top of `tls`, same
    /// reasoning `request` already has for GET. Jira's own write
    /// endpoints routinely answer with an empty 204 No Content body
    /// (`PUT /issue/{key}`, `POST .../transitions`) -- treated as `Ok
    /// (None)` rather than a JSON-parse failure; `POST /issue` and
    /// `POST .../comment` do send a real body back, surfaced as `Ok
    /// (Some(value))`.
    pub(crate) fn send(&self, method: &str, path: &str, body: &serde_json::Value) -> Result<Option<serde_json::Value>, String> {
        let url = format!("{}{path}", self.base_url);
        let payload = serde_json::to_string(body).map_err(|err| format!("couldn't encode request body: {err}"))?;
        let req = ureq::request(method, &url)
            .set("Authorization", &format!("Bearer {}", self.token))
            .set("Accept", "application/json")
            .set("Content-Type", "application/json");
        let response = req.send_string(&payload).map_err(|err| describe_error(&err))?;
        let text = response.into_string().map_err(|err| format!("couldn't read response body: {err}"))?;
        if text.trim().is_empty() {
            return Ok(None);
        }
        serde_json::from_str(&text).map(Some).map_err(|err| format!("couldn't parse response as JSON: {err}"))
    }
}

/// `ureq::Error` already carries a real HTTP status + response body for
/// the `Status` case (e.g. a 401 with Jira's own JSON error payload) --
/// surfaced directly rather than just `Display`, since Jira's own error
/// messages (`errorMessages`) are far more useful to a user than "Status
/// code 401." A `Transport` error (DNS, TLS, connection refused) falls
/// back to its own `Display`.
fn describe_error(err: &ureq::Error) -> String {
    match err {
        ureq::Error::Status(code, response) => {
            let body = response.status_text().to_string();
            format!("HTTP {code} ({body})")
        }
        ureq::Error::Transport(transport) => format!("request failed: {transport}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_strips_a_trailing_slash_from_the_base_url() {
        let client = JiraClient::new("https://jira.example.com/", "tok");
        assert_eq!(client.base_url, "https://jira.example.com");
    }

    #[test]
    fn new_leaves_a_base_url_with_no_trailing_slash_unchanged() {
        let client = JiraClient::new("https://jira.example.com", "tok");
        assert_eq!(client.base_url, "https://jira.example.com");
    }
}
