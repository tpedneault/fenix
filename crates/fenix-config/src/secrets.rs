//! API tokens: the GitLab, Jira and GitHub ones. They're kept in
//! `settings.toml` like any other setting (the settings page types them
//! masked); an environment variable (`FENIX_GITLAB_TOKEN`, ...) wins over
//! the file and is never written to it.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Secret {
    GitLab,
    Jira,
    GitHub,
}

impl Secret {
    pub const ALL: [Secret; 3] = [Secret::GitLab, Secret::Jira, Secret::GitHub];

    /// The environment variable that overrides the file.
    pub fn env(self) -> &'static str {
        match self {
            Secret::GitLab => "FENIX_GITLAB_TOKEN",
            Secret::Jira => "FENIX_JIRA_TOKEN",
            Secret::GitHub => "FENIX_GITHUB_TOKEN",
        }
    }

    /// From the environment, when it's set there.
    pub fn from_env(self) -> Option<String> {
        std::env::var(self.env()).ok().filter(|v| !v.trim().is_empty())
    }
}
