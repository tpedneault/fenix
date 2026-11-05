//! API tokens: the GitLab, Jira and GitHub ones, and an extension's
//! `secret` settings. Where the system has a keychain they're kept
//! in it (`keychain`), and one still in `settings.toml` is moved there the
//! first time Fenix reads it; where it has none, in `settings.toml` like
//! any other setting (the settings page types them masked). An environment
//! variable (`FENIX_GITLAB_TOKEN`, ...) wins over both and is never
//! written anywhere.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Secret {
    GitLab,
    Jira,
    GitHub,
    /// An extension's `secret` setting, by its key.
    Named(&'static str),
}

impl Secret {
    pub const ALL: [Secret; 3] = [Secret::GitLab, Secret::Jira, Secret::GitHub];

    /// Its setting's key: `gitlab.token`.
    pub fn key(self) -> &'static str {
        match self {
            Secret::GitLab => "gitlab.token",
            Secret::Jira => "jira.token",
            Secret::GitHub => "github.token",
            Secret::Named(key) => key,
        }
    }

    /// The environment variable that overrides it: `FENIX_` and its key,
    /// upper-case, `_` for `.` and `-` (`FENIX_GITLAB_TOKEN`).
    pub fn env(self) -> String {
        env_of(self.key())
    }

    /// From the environment, when it's set there.
    pub fn from_env(self) -> Option<String> {
        std::env::var(self.env()).ok().filter(|v| !v.trim().is_empty())
    }
}

/// The environment variable that overrides secret `key`.
pub fn env_of(key: &str) -> String {
    format!("FENIX_{}", key.to_ascii_uppercase().replace(['.', '-'], "_"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_secret_is_named_by_its_key_and_overridden_by_its_variable() {
        assert_eq!(Secret::GitLab.env(), "FENIX_GITLAB_TOKEN");
        assert_eq!(Secret::Named("http.secret.api-key").key(), "http.secret.api-key");
        assert_eq!(Secret::Named("http.secret.api-key").env(), "FENIX_HTTP_SECRET_API_KEY");
    }
}
