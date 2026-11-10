//! Whether a project may have Fenix run the programs it names.
//!
//! A project's `.fenix/tools.json` names programs: the language server
//! for a language, debug adapters, tasks. `.fenix/lsp/<server>.json`
//! sets a language server's settings, and for several servers a setting
//! is a command. Fenix starts a language server when a file of its
//! language is opened, so for a project nobody has looked at (a
//! repository just cloned, an archive just unpacked) those files are a
//! way to have a program run by opening a file.
//!
//! So what a project names is only used once its user has said so, for
//! those files as they are: the decision is kept with a SHA-256 of their
//! contents, and files that change (a pull, another archive) are asked
//! about again. A project that names nothing has nothing to decide.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// How a project stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// It names no programs: nothing to trust.
    Nothing,
    /// Its user allowed what it names, as it is now.
    Trusted,
    /// Its user refused what it names, as it is now.
    Refused,
    /// Nobody has decided, or what it names has changed since.
    Unknown,
}

impl Verdict {
    /// Whether what the project names may be used.
    pub fn allows(self) -> bool {
        matches!(self, Verdict::Nothing | Verdict::Trusted)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Decision {
    /// Of the files the decision was made about.
    fingerprint: String,
    trusted: bool,
}

#[derive(Default, Serialize, Deserialize)]
struct Stored {
    #[serde(default)]
    projects: BTreeMap<String, Decision>,
}

/// The decisions made, kept in a file of Fenix's own (never in the
/// project, which could then answer for itself).
#[derive(Debug, Default)]
pub struct Trust {
    file: Option<PathBuf>,
    decided: BTreeMap<String, Decision>,
}

/// The files of a project that name programs or the settings of one,
/// in a fixed order: `.fenix/tools.json`, then `.fenix/lsp/*.json`.
pub fn files(root: &Path) -> Vec<PathBuf> {
    let fenix = root.join(".fenix");
    let mut found = Vec::new();
    let tools = fenix.join("tools.json");
    if tools.is_file() {
        found.push(tools);
    }
    let mut servers: Vec<PathBuf> = std::fs::read_dir(fenix.join("lsp"))
        .map(|entries| entries.flatten().map(|entry| entry.path()).filter(|path| path.extension().is_some_and(|ext| ext == "json") && path.is_file()).collect())
        .unwrap_or_default();
    servers.sort();
    found.extend(servers);
    found
}

/// What stands for those files as they are: `None` when there are none.
/// A file that can't be read counts by its name alone, so a decision
/// made while it was unreadable doesn't cover what it turns out to hold.
pub fn fingerprint(root: &Path) -> Option<String> {
    let files = files(root);
    if files.is_empty() {
        return None;
    }
    let mut hash = Sha256::new();
    for file in files {
        let name = file.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
        let contents = std::fs::read(&file).ok();
        // Lengths before contents: no two sets of files run together
        // into the same bytes.
        hash.update((name.len() as u64).to_le_bytes());
        hash.update(name.as_bytes());
        match contents {
            Some(bytes) => {
                hash.update([1]);
                hash.update((bytes.len() as u64).to_le_bytes());
                hash.update(&bytes);
            }
            None => hash.update([0]),
        }
    }
    Some(hash.finalize().iter().map(|byte| format!("{byte:02x}")).collect())
}

/// What a project is known by: its root, however it was spelt.
fn key(root: &Path) -> String {
    let plain = crate::plain_path(root.to_path_buf()).to_string_lossy().trim_end_matches(['/', '\\']).to_string();
    if cfg!(windows) {
        plain.replace('/', "\\").to_lowercase()
    } else {
        plain
    }
}

impl Trust {
    /// The decisions in `file`, and whether the file was there: a first
    /// run has none, and may want to start from the projects already in
    /// use (`trust`, for each).
    pub fn load(file: Option<PathBuf>) -> (Trust, bool) {
        let stored = file.as_deref().and_then(|file| std::fs::read(file).ok());
        let existed = stored.is_some();
        let decided = stored.and_then(|bytes| serde_json::from_slice::<Stored>(&bytes).ok()).map(|stored| stored.projects).unwrap_or_default();
        (Trust { file, decided }, existed)
    }

    pub fn verdict(&self, root: &Path) -> Verdict {
        let Some(now) = fingerprint(root) else { return Verdict::Nothing };
        match self.decided.get(&key(root)) {
            Some(decision) if decision.fingerprint == now => {
                if decision.trusted {
                    Verdict::Trusted
                } else {
                    Verdict::Refused
                }
            }
            _ => Verdict::Unknown,
        }
    }

    /// Records the user's answer for what the project names now, and
    /// saves it. Nothing to record for a project that names nothing.
    pub fn decide(&mut self, root: &Path, trusted: bool) -> io::Result<()> {
        match fingerprint(root) {
            Some(fingerprint) => {
                self.decided.insert(key(root), Decision { fingerprint, trusted });
            }
            None => {
                self.decided.remove(&key(root));
            }
        }
        self.save()
    }

    /// Writes the decisions, creating the file even when there are none
    /// (which is what says a first run has happened).
    pub fn save(&self) -> io::Result<()> {
        let Some(file) = &self.file else { return Ok(()) };
        if let Some(folder) = file.parent() {
            std::fs::create_dir_all(folder)?;
        }
        let stored = Stored { projects: self.decided.clone() };
        fenix_storage::write(file, &serde_json::to_vec_pretty(&stored).map_err(io::Error::other)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::TempDir;

    #[test]
    fn a_project_that_names_nothing_has_nothing_to_decide() {
        let dir = TempDir::new("trust_nothing");
        dir.write("src/main.rs", "fn main() {}");
        dir.write(".fenix/settings.toml", "[editor]\nindent_width = 2\n");
        let (trust, existed) = Trust::load(None);
        assert!(!existed);
        assert_eq!(trust.verdict(dir.path()), Verdict::Nothing);
        assert!(Verdict::Nothing.allows() && Verdict::Trusted.allows() && !Verdict::Unknown.allows() && !Verdict::Refused.allows());
    }

    #[test]
    fn a_decision_stands_for_the_files_as_they_were_and_is_kept() {
        let dir = TempDir::new("trust_decision");
        let state = TempDir::new("trust_state");
        let file = state.path().join("trust.json");
        dir.write(".fenix/tools.json", r#"{"lsp": {"rust": {"executable": "rust-analyzer"}}}"#);
        let (mut trust, existed) = Trust::load(Some(file.clone()));
        assert!(!existed);
        assert_eq!(trust.verdict(dir.path()), Verdict::Unknown);

        trust.decide(dir.path(), true).unwrap();
        assert_eq!(trust.verdict(dir.path()), Verdict::Trusted);
        // Kept, and found again under another spelling of the folder.
        let (again, existed) = Trust::load(Some(file.clone()));
        assert!(existed);
        assert_eq!(again.verdict(&dir.path().join(".")), Verdict::Unknown, "a different path is a different project");
        assert_eq!(again.verdict(dir.path()), Verdict::Trusted);

        // What it names changes: asked again, whatever was said before.
        dir.write(".fenix/tools.json", r#"{"lsp": {"rust": {"executable": "./payload"}}}"#);
        assert_eq!(again.verdict(dir.path()), Verdict::Unknown);
        let mut again = again;
        again.decide(dir.path(), false).unwrap();
        assert_eq!(again.verdict(dir.path()), Verdict::Refused);
        // Put back as it was, it is not trusted by an old answer.
        dir.write(".fenix/tools.json", r#"{"lsp": {"rust": {"executable": "rust-analyzer"}}}"#);
        assert_eq!(again.verdict(dir.path()), Verdict::Unknown);
    }

    #[test]
    fn a_servers_settings_file_is_part_of_what_is_decided() {
        let dir = TempDir::new("trust_lsp_settings");
        dir.write(".fenix/lsp/rust-analyzer.json", r#"{"check": {"command": "clippy"}}"#);
        let (mut trust, _) = Trust::load(None);
        assert_eq!(trust.verdict(dir.path()), Verdict::Unknown, "settings alone are something to decide");
        trust.decide(dir.path(), true).unwrap();
        assert_eq!(trust.verdict(dir.path()), Verdict::Trusted);
        dir.write(".fenix/lsp/rust-analyzer.json", r#"{"check": {"overrideCommand": ["./payload"]}}"#);
        assert_eq!(trust.verdict(dir.path()), Verdict::Unknown);
        trust.decide(dir.path(), true).unwrap();
        // Another server's settings, added.
        dir.write(".fenix/lsp/pyright.json", "{}");
        assert_eq!(trust.verdict(dir.path()), Verdict::Unknown);
        assert_eq!(files(dir.path()).len(), 2);
    }

    #[test]
    fn two_sets_of_files_never_share_a_fingerprint_by_running_together() {
        let a = TempDir::new("trust_run_a");
        let b = TempDir::new("trust_run_b");
        a.write(".fenix/lsp/a.json", "12");
        a.write(".fenix/lsp/b.json", "3");
        b.write(".fenix/lsp/a.json", "1");
        b.write(".fenix/lsp/b.json", "23");
        assert_ne!(fingerprint(a.path()), fingerprint(b.path()));
    }

    #[test]
    fn a_broken_store_is_no_decisions_not_a_failure() {
        let state = TempDir::new("trust_broken");
        let file = state.write("trust.json", "{ not json");
        let (trust, existed) = Trust::load(Some(file));
        assert!(existed);
        let dir = TempDir::new("trust_broken_project");
        dir.write(".fenix/tools.json", "{}");
        assert_eq!(trust.verdict(dir.path()), Verdict::Unknown);
    }
}
