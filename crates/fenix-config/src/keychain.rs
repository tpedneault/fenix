//! The system's keychain, where secrets are kept: Windows'
//! Credential Manager and the macOS Keychain (through the `keyring`
//! crate), and on Linux the Secret Service (GNOME Keyring, KWallet)
//! through its `secret-tool`. Each secret is an entry of service `fenix`,
//! by its key (`gitlab.token`, `http.secret.api`). Where there's none --
//! no `secret-tool`, no service answering -- Fenix keeps secrets in
//! `settings.toml` as before, and says so.

use std::collections::HashMap;
use std::sync::Mutex;

use crate::{schema, Config, Kind, Value};

/// The keychain's service name for Fenix's entries.
pub const SERVICE: &str = "fenix";

/// A keychain: where secrets are kept, by key.
pub trait Store: Send + Sync {
    /// What it's called, as Fenix says where a secret is.
    fn name(&self) -> &str;
    fn get(&self, key: &str) -> Result<Option<String>, String>;
    fn set(&self, key: &str, value: &str) -> Result<(), String>;
    fn delete(&self, key: &str) -> Result<(), String>;
}

/// The keychain Fenix keeps secrets in, if there's one, and what it has
/// read from it so far.
pub struct Keychain {
    store: Option<Box<dyn Store>>,
    read: Mutex<HashMap<String, Option<String>>>,
}

impl Keychain {
    /// No keychain: secrets stay in `settings.toml`.
    pub fn none() -> Keychain {
        Keychain { store: None, read: Mutex::default() }
    }

    pub fn with(store: impl Store + 'static) -> Keychain {
        Keychain { store: Some(Box::new(store)), read: Mutex::default() }
    }

    /// This system's, when it has one that answers.
    pub fn system() -> Keychain {
        match native() {
            Some(store) => Keychain { store: Some(store), read: Mutex::default() },
            None => Keychain::none(),
        }
    }

    /// What it's called; none when there's none.
    pub fn name(&self) -> Option<&str> {
        self.store.as_ref().map(|s| s.name())
    }

    /// Secret `key`, when it's kept there: read once, then remembered.
    pub fn get(&self, key: &str) -> Option<String> {
        let store = self.store.as_ref()?;
        if let Some(known) = self.read.lock().ok().and_then(|r| r.get(key).cloned()) {
            return known;
        }
        let value = store.get(key).ok().flatten().filter(|v| !v.is_empty());
        if let Ok(mut read) = self.read.lock() {
            read.insert(key.to_string(), value.clone());
        }
        value
    }

    /// Secret `key` kept there, or with none, taken away.
    pub fn set(&self, key: &str, value: Option<&str>) -> Result<(), String> {
        let store = self.store.as_ref().ok_or("there's no keychain here")?;
        match value.filter(|v| !v.is_empty()) {
            Some(value) => store.set(key, value)?,
            None => store.delete(key)?,
        }
        if let Ok(mut read) = self.read.lock() {
            read.insert(key.to_string(), value.filter(|v| !v.is_empty()).map(str::to_string));
        }
        Ok(())
    }
}

/// The secrets `config` holds as plain text -- its `secret` settings set
/// in `settings.toml` -- moved to `keychain`: each kept there, then taken
/// out of the file, which is saved. The keys moved, and what went wrong.
pub fn migrate(config: &mut Config, keychain: &Keychain) -> (Vec<&'static str>, Vec<String>) {
    let (mut moved, mut problems) = (Vec::new(), Vec::new());
    if keychain.name().is_none() {
        return (moved, problems);
    }
    for setting in schema::settings().into_iter().filter(|s| matches!(s.kind, Kind::Secret(_))) {
        let Some(Value::Text(text)) = setting.get(config) else { continue };
        if text.trim().is_empty() {
            continue;
        }
        match keychain.set(setting.key, Some(&text)) {
            Ok(()) => match setting.set(config, None) {
                Ok(()) => moved.push(setting.key),
                Err(e) => problems.push(format!("{}: {e}", setting.key)),
            },
            Err(e) => problems.push(format!("{}: kept in settings.toml -- the keychain said {e}", setting.key)),
        }
    }
    if !moved.is_empty() {
        if let Err(e) = config.save() {
            problems.push(format!("couldn't save settings.toml: {e}"));
        }
    }
    (moved, problems)
}

/// A keychain in memory, for tests.
#[derive(Default)]
pub struct Memory(pub Mutex<HashMap<String, String>>);

impl Store for Memory {
    fn name(&self) -> &str {
        "the test keychain"
    }

    fn get(&self, key: &str) -> Result<Option<String>, String> {
        Ok(self.0.lock().map_err(|e| e.to_string())?.get(key).cloned())
    }

    fn set(&self, key: &str, value: &str) -> Result<(), String> {
        self.0.lock().map_err(|e| e.to_string())?.insert(key.to_string(), value.to_string());
        Ok(())
    }

    fn delete(&self, key: &str) -> Result<(), String> {
        self.0.lock().map_err(|e| e.to_string())?.remove(key);
        Ok(())
    }
}

/// Windows' Credential Manager, the macOS Keychain.
#[cfg(any(windows, target_os = "macos"))]
struct Native;

#[cfg(any(windows, target_os = "macos"))]
impl Store for Native {
    fn name(&self) -> &str {
        if cfg!(windows) { "Windows Credential Manager" } else { "the macOS Keychain" }
    }

    fn get(&self, key: &str) -> Result<Option<String>, String> {
        match keyring::Entry::new(SERVICE, key).and_then(|e| e.get_password()) {
            Ok(value) => Ok(Some(value)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }

    fn set(&self, key: &str, value: &str) -> Result<(), String> {
        keyring::Entry::new(SERVICE, key).and_then(|e| e.set_password(value)).map_err(|e| e.to_string())
    }

    fn delete(&self, key: &str) -> Result<(), String> {
        match keyring::Entry::new(SERVICE, key).and_then(|e| e.delete_credential()) {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }
}

#[cfg(any(windows, target_os = "macos"))]
fn native() -> Option<Box<dyn Store>> {
    Some(Box::new(Native))
}

/// The Secret Service, through `secret-tool` (libsecret's): Fenix links no
/// D-Bus library of its own for it.
#[cfg(not(any(windows, target_os = "macos")))]
struct SecretTool;

#[cfg(not(any(windows, target_os = "macos")))]
impl SecretTool {
    /// `secret-tool` with `args`, `input` on its stdin: its stdout, or why
    /// it failed. It may wait for you to unlock the keyring, so it has a
    /// while.
    fn run(args: &[&str], input: Option<&str>) -> Result<(bool, String), String> {
        use std::io::{Read, Write};
        use std::process::{Command, Stdio};
        let mut child = Command::new("secret-tool").args(args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|e| format!("secret-tool: {e}"))?;
        if let Some(mut stdin) = child.stdin.take() {
            if let Some(input) = input {
                let _ = stdin.write_all(input.as_bytes());
            }
        }
        let started = std::time::Instant::now();
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if started.elapsed() < std::time::Duration::from_secs(30) => std::thread::sleep(std::time::Duration::from_millis(10)),
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err("secret-tool didn't answer".into());
                }
            }
        };
        let (mut out, mut err) = (String::new(), String::new());
        if let Some(mut s) = child.stdout.take() {
            let _ = s.read_to_string(&mut out);
        }
        if let Some(mut s) = child.stderr.take() {
            let _ = s.read_to_string(&mut err);
        }
        // Not found is a failure with nothing said; anything said is why.
        match err.lines().map(str::trim).find(|l| !l.is_empty()) {
            Some(why) => Err(why.to_string()),
            None => Ok((status.success(), out)),
        }
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
impl Store for SecretTool {
    fn name(&self) -> &str {
        "the Secret Service"
    }

    fn get(&self, key: &str) -> Result<Option<String>, String> {
        let (found, out) = SecretTool::run(&["lookup", "service", SERVICE, "key", key], None)?;
        Ok(found.then_some(out))
    }

    fn set(&self, key: &str, value: &str) -> Result<(), String> {
        let label = format!("--label=Fenix: {key}");
        match SecretTool::run(&["store", &label, "service", SERVICE, "key", key], Some(value))? {
            (true, _) => Ok(()),
            (false, _) => Err("secret-tool couldn't keep it".into()),
        }
    }

    fn delete(&self, key: &str) -> Result<(), String> {
        SecretTool::run(&["clear", "service", SERVICE, "key", key], None).map(|_| ())
    }
}

/// `secret-tool`, when it's there and a Secret Service answers it.
#[cfg(not(any(windows, target_os = "macos")))]
fn native() -> Option<Box<dyn Store>> {
    SecretTool.get("fenix.probe").ok()?;
    Some(Box::new(SecretTool))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_secret_is_kept_read_once_and_taken_away() {
        let keychain = Keychain::with(Memory::default());
        assert_eq!(keychain.get("http.secret.api"), None);
        keychain.set("http.secret.api", Some("k3y")).unwrap();
        assert_eq!(keychain.get("http.secret.api").as_deref(), Some("k3y"));
        keychain.set("http.secret.api", None).unwrap();
        assert_eq!(keychain.get("http.secret.api"), None);
        assert!(Keychain::none().set("x.y", Some("z")).is_err() && Keychain::none().name().is_none());
    }

    #[test]
    fn plain_text_secrets_move_to_the_keychain_and_out_of_the_file() {
        let dir = std::env::temp_dir().join(format!("fenix-keychain-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.toml");
        std::fs::write(&path, "[gitlab]\ntoken = \"glpat-plain\"\n").unwrap();
        let mut config = Config::load_at(path.clone(), dir.clone()).unwrap();
        // None: they stay.
        assert_eq!(migrate(&mut config, &Keychain::none()), (Vec::new(), Vec::new()));
        assert!(std::fs::read_to_string(&path).unwrap().contains("glpat-plain"));
        let keychain = Keychain::with(Memory::default());
        let (moved, problems) = migrate(&mut config, &keychain);
        assert_eq!((moved, problems), (vec!["gitlab.token"], Vec::<String>::new()));
        assert_eq!(keychain.get("gitlab.token").as_deref(), Some("glpat-plain"));
        assert!(!std::fs::read_to_string(&path).unwrap().contains("glpat"), "gone from the file");
        assert_eq!(config.gitlab_token, None);
        assert_eq!(migrate(&mut config, &keychain).0, Vec::<&str>::new(), "once");
        let _ = std::fs::remove_dir_all(dir);
    }
}
