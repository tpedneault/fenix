//! API tokens, kept out of every file: in the Windows Credential Manager
//! (or the macOS Keychain) under `fenix/gitlab`, `fenix/jira` and
//! `fenix/github`. Where there's no credential store to use, a token can
//! come from the environment (`FENIX_GITLAB_TOKEN`, ...) instead, and an
//! environment variable wins over the store everywhere.

use std::collections::HashMap;
use std::sync::Mutex;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Secret {
    GitLab,
    Jira,
    GitHub,
}

impl Secret {
    pub const ALL: [Secret; 3] = [Secret::GitLab, Secret::Jira, Secret::GitHub];

    /// Its name in the credential store.
    pub fn name(self) -> &'static str {
        match self {
            Secret::GitLab => "gitlab",
            Secret::Jira => "jira",
            Secret::GitHub => "github",
        }
    }

    /// The environment variable that overrides the store.
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

/// Where tokens are kept.
pub trait SecretStore: Send + Sync {
    fn get(&self, secret: Secret) -> Result<Option<String>, String>;
    fn set(&self, secret: Secret, token: &str) -> Result<(), String>;
    fn delete(&self, secret: Secret) -> Result<(), String>;
    /// What it's called on the settings page: "Credential Manager".
    fn name(&self) -> &'static str;
    /// Whether tokens put in it outlast this run of Fenix.
    fn lasts(&self) -> bool {
        true
    }
}

/// The operating system's credential store.
#[cfg(any(windows, target_os = "macos"))]
pub struct SystemStore;

#[cfg(any(windows, target_os = "macos"))]
impl SystemStore {
    fn entry(secret: Secret) -> Result<keyring::Entry, String> {
        keyring::Entry::new("fenix", secret.name()).map_err(|e| e.to_string())
    }
}

#[cfg(any(windows, target_os = "macos"))]
impl SecretStore for SystemStore {
    fn get(&self, secret: Secret) -> Result<Option<String>, String> {
        match Self::entry(secret)?.get_password() {
            Ok(token) => Ok(Some(token)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }

    fn set(&self, secret: Secret, token: &str) -> Result<(), String> {
        Self::entry(secret)?.set_password(token).map_err(|e| e.to_string())
    }

    fn delete(&self, secret: Secret) -> Result<(), String> {
        match Self::entry(secret)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }

    fn name(&self) -> &'static str {
        if cfg!(windows) {
            "Credential Manager"
        } else {
            "Keychain"
        }
    }
}

/// Tokens held for this run only: the tests' store, and the fallback
/// where the system has none.
#[derive(Default)]
pub struct MemoryStore {
    tokens: Mutex<HashMap<Secret, String>>,
    name: &'static str,
}

impl MemoryStore {
    pub fn new(name: &'static str) -> Self {
        MemoryStore { tokens: Mutex::default(), name }
    }
}

impl SecretStore for MemoryStore {
    fn get(&self, secret: Secret) -> Result<Option<String>, String> {
        Ok(self.tokens.lock().unwrap_or_else(|e| e.into_inner()).get(&secret).cloned())
    }

    fn set(&self, secret: Secret, token: &str) -> Result<(), String> {
        self.tokens.lock().unwrap_or_else(|e| e.into_inner()).insert(secret, token.to_string());
        Ok(())
    }

    fn delete(&self, secret: Secret) -> Result<(), String> {
        self.tokens.lock().unwrap_or_else(|e| e.into_inner()).remove(&secret);
        Ok(())
    }

    fn name(&self) -> &'static str {
        if self.name.is_empty() {
            "memory"
        } else {
            self.name
        }
    }

    fn lasts(&self) -> bool {
        false
    }
}

/// This system's credential store: the real one on Windows and macOS;
/// elsewhere one that only lasts this run, so tokens there come from the
/// environment.
pub fn system() -> Box<dyn SecretStore> {
    #[cfg(any(windows, target_os = "macos"))]
    {
        Box::new(SystemStore)
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        Box::new(MemoryStore::new("this run only -- set FENIX_*_TOKEN instead"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_memory_store_keeps_each_token_apart_and_forgets_on_delete() {
        let store = MemoryStore::new("");
        store.set(Secret::GitLab, "glpat-1").unwrap();
        store.set(Secret::Jira, "jira-1").unwrap();
        assert_eq!(store.get(Secret::GitLab).unwrap().as_deref(), Some("glpat-1"));
        store.delete(Secret::GitLab).unwrap();
        assert_eq!(store.get(Secret::GitLab).unwrap(), None);
        assert_eq!(store.get(Secret::Jira).unwrap().as_deref(), Some("jira-1"));
        assert!(!store.lasts());
    }

    /// Writes to the real credential store, under a name nothing uses.
    #[test]
    #[ignore]
    #[cfg(any(windows, target_os = "macos"))]
    fn the_system_store_round_trips_a_token() {
        let entry = keyring::Entry::new("fenix-test", "roundtrip").unwrap();
        entry.set_password("s3cret").unwrap();
        assert_eq!(entry.get_password().unwrap(), "s3cret");
        entry.delete_credential().unwrap();
        assert!(matches!(entry.get_password(), Err(keyring::Error::NoEntry)));
    }
}
