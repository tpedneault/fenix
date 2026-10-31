//! Being a code-review forge (GitHub, GitLab) for Fenix's Git pages:
//! `[[contributes.forges]]` names it and the hosts of the remotes it
//! serves, and Fenix asks it, through its `serve`, about a repository
//! whose `origin` is on one of them.
//!
//! ```toml
//! [[contributes.forges]]
//! name = "GitHub"
//! hosts = ["github.com"]
//! ```
//!
//! | Service | When | Its answer |
//! |---|---|---|
//! | `forge.branch` | the Git status page is read | the branch's request, for the page's header, or `null` |
//! | `forge.open` | `o` there | it opens that request (`number`), or a new one for the branch |
//! | `forge.reviews` | the reviews are asked for (`SPC g M`) | it opens them |
//!
//! Each is given a [`Repository`]; it may read files under its `root` and
//! run programs there (`git`, with `permissions.process`). It can answer
//! later ([`crate::answer`]), and calls [`refresh`] after it changes the
//! repository.
//!
//! ```no_run
//! use fenix_extension_api::{forge, Extension};
//!
//! #[derive(Default)]
//! struct Hub;
//!
//! impl Extension for Hub {
//!     fn serve(&mut self, _call: u64, name: &str, payload: &str) -> Option<Result<String, String>> {
//!         match forge::call(name, payload)? {
//!             Ok(forge::Call::Branch(_repo)) => Some(Ok(forge::branch_answer(None))),
//!             Ok(forge::Call::Open(_repo) | forge::Call::Reviews(_repo)) => Some(Ok(String::new())),
//!             Err(e) => Some(Err(e)),
//!         }
//!     }
//! }
//! ```

use serde::{Deserialize, Serialize};

/// The branch's request, for the Git status page's header.
pub const BRANCH: &str = "forge.branch";
/// `o` on the Git status page: request `number`, or a new one.
pub const OPEN: &str = "forge.open";
/// The reviews waiting.
pub const REVIEWS: &str = "forge.reviews";

/// The repository a forge call is about.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Repository {
    /// Its folder, a full path: it may read and run programs there.
    pub root: String,
    /// `origin`'s URL.
    pub remote: String,
    /// The branch checked out, if one is.
    #[serde(default)]
    pub branch: Option<String>,
    /// The request it's about (`forge.open`), when there's one.
    #[serde(default)]
    pub number: Option<u64>,
}

/// A request as the Git status page's header shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    pub number: u64,
    /// How it's referred to: `#42`, `!42`.
    pub reference: String,
    pub title: String,
    #[serde(default)]
    pub draft: bool,
    /// Its checks, all as one: `passed`, `failed`, `running`, `pending`,
    /// `canceled`, `skipped`, `manual`, or the forge's own word.
    #[serde(default)]
    pub checks: Option<String>,
    #[serde(default)]
    pub approved_by: Vec<String>,
    /// Review threads nobody has resolved.
    #[serde(default)]
    pub unresolved: u32,
}

/// What Fenix asks of a forge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Call {
    Branch(Repository),
    Open(Repository),
    Reviews(Repository),
}

/// Service `name`, if it's a forge call, read.
pub fn call(name: &str, payload: &str) -> Option<Result<Call, String>> {
    let wrap: fn(Repository) -> Call = match name {
        BRANCH => Call::Branch,
        OPEN => Call::Open,
        REVIEWS => Call::Reviews,
        _ => return None,
    };
    Some(serde_json::from_str::<Repository>(payload).map(wrap).map_err(|e| format!("{name}: {e}")))
}

/// `forge.branch`'s answer: the branch's request, or none.
pub fn branch_answer(request: Option<&Request>) -> String {
    serde_json::to_string(&request).unwrap_or_else(|_| "null".into())
}

/// Fenix's Git pages read again, after it changed the repository.
pub fn refresh() {
    crate::host::git_refresh()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forge_calls_are_read_and_answered() {
        let payload = r#"{"root": "C:/src/fenix", "remote": "git@github.com:o/r.git", "branch": "main", "number": null}"#;
        let Some(Ok(Call::Branch(repo))) = call(BRANCH, payload) else { panic!() };
        assert_eq!((repo.branch.as_deref(), repo.number), (Some("main"), None));
        assert!(call("other.thing", payload).is_none());
        assert!(matches!(call(OPEN, "{}"), Some(Err(_))));
        assert_eq!(branch_answer(None), "null");
        let request = Request { number: 7, reference: "#7".into(), title: "x".into(), draft: false, checks: Some("passed".into()), approved_by: vec![], unresolved: 1 };
        assert!(branch_answer(Some(&request)).contains("\"reference\":\"#7\""));
    }
}
