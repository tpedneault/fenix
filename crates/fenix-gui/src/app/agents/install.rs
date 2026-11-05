//! Agents from the ACP registry (`agents.registry`): read, offered, and
//! installed into Fenix's data folder, off the UI thread. The catalog
//! offers what's installed beside the built-ins.

use super::*;
use fenix_acp::registry::{self, RegistryAgent};
use fenix_extensions::registry::Fetch;
use std::sync::mpsc;

/// What the registry's worker sends back.
pub(crate) enum Fetched {
    Listed(Result<Vec<RegistryAgent>, String>),
    Installed(Result<registry::Installed, String>),
}

/// The most the registry's index may be.
const MAX_INDEX: u64 = 8 * 1024 * 1024;

impl App {
    /// Where agents from the registry are installed.
    pub(super) fn agents_install_dir() -> Option<PathBuf> {
        fenix_storage::paths::data_file("agents")
    }

    /// The agents installed from the registry, for the catalog.
    pub(super) fn agents_installed() -> Vec<fenix_acp::hub::AgentSpec> {
        Self::agents_install_dir().map(|dir| registry::load(&dir).iter().map(registry::Installed::spec).collect()).unwrap_or_default()
    }

    /// `SPC a p` → "from the registry": its agents, read off the UI thread.
    pub(super) fn agents_browse_registry(&mut self) {
        let url = self.config.text_value("agents.registry").map(str::trim).unwrap_or(registry::DEFAULT_URL).to_string();
        if url.is_empty() {
            self.set_message("the agent registry is off (agents.registry)");
            return;
        }
        let (tx, rx) = mpsc::channel();
        self.agents.fetched = Some(rx);
        std::thread::spawn(move || {
            let listed = fenix_extensions::registry::Http.get(&url, MAX_INDEX).and_then(|body| registry::parse(&body));
            let _ = tx.send(Fetched::Listed(listed));
        });
        self.set_message("reading the agent registry…");
    }

    /// Installs one, off the UI thread.
    pub(super) fn agents_install(&mut self, agent: RegistryAgent) {
        let Some(dir) = Self::agents_install_dir() else {
            self.set_error("no folder to install agents into");
            return;
        };
        let Some(plan) = registry::plan(&agent, &registry::platform(), &|p| catalog::find_on_path(p).is_some()) else {
            self.set_error(format!("{} has nothing for this machine ({}){}", agent.name, registry::platform(), if agent.npx.is_some() { ": it needs npx (Node.js)" } else { "" }));
            return;
        };
        let (tx, rx) = mpsc::channel();
        self.agents.fetched = Some(rx);
        let name = agent.name.clone();
        std::thread::spawn(move || {
            let fetch = |url: &str, limit: u64| fenix_extensions::registry::Http.get(url, limit);
            let _ = tx.send(Fetched::Installed(registry::install(&agent, &plan, &dir, &fetch)));
        });
        self.set_message(format!("installing {name}…"));
    }

    pub(super) fn agents_uninstall(&mut self, id: &str) {
        let Some(dir) = Self::agents_install_dir() else { return };
        match registry::uninstall(&dir, id) {
            Ok(()) => self.set_message(format!("{id} uninstalled")),
            Err(e) => self.set_error(format!("couldn't uninstall {id}: {e}")),
        }
        self.agents_configure();
    }

    /// What the registry's worker sent, if it has.
    pub(super) fn agents_fetched_tick(&mut self) {
        let Some(rx) = &self.agents.fetched else { return };
        let Ok(fetched) = rx.try_recv() else { return };
        self.agents.fetched = None;
        match fetched {
            Fetched::Listed(Err(e)) => self.set_error(format!("couldn't read the agent registry: {e}")),
            Fetched::Listed(Ok(agents)) => {
                let installed: Vec<String> = Self::agents_installed().into_iter().map(|s| s.id).collect();
                let here = registry::platform();
                let have = |p: &str| catalog::find_on_path(p).is_some();
                let candidates: Vec<_> = agents
                    .into_iter()
                    .map(|a| {
                        let how = match registry::plan(&a, &here, &have) {
                            Some(registry::Plan::Binary(_)) => "download",
                            Some(registry::Plan::Npx(_)) => "npx",
                            Some(registry::Plan::Uvx(_)) => "uvx",
                            None => "not for this machine",
                        };
                        let mark = if installed.contains(&a.id) { " (installed)" } else { "" };
                        let label = format!("Install {} {}{mark}   {how} · {}", a.name, a.version, a.description);
                        fenix_picker::Candidate::new(label, Pick::Install(Box::new(a)))
                    })
                    .collect();
                if candidates.is_empty() {
                    self.set_message("the agent registry lists nothing");
                } else {
                    self.enter_picker(ActivePicker::Agents(fenix_picker::PickerState::new(candidates)));
                }
            }
            Fetched::Installed(Err(e)) => self.set_error(e),
            Fetched::Installed(Ok(i)) => {
                self.agents_configure();
                let note = if i.unchecked { " (the registry gave no SHA-256 to check it against)" } else { "" };
                self.set_message(format!("{} {} installed{note}: SPC a p starts a thread with it", i.name, i.version));
            }
        }
    }
}

/// The prompt `SPC a s` opens for a key, among extensions' prompts.
pub(crate) const SECRET_PROMPT: &str = "fenix:agents";

/// The API keys agents read, offered by `SPC a s`.
const KEYS: &[&str] = &["ANTHROPIC_API_KEY", "OPENAI_API_KEY", "CODEX_API_KEY", "WINDSURF_API_KEY", "GEMINI_API_KEY"];

impl App {
    /// `SPC a s`: an API key for the agents, kept in the keychain and given
    /// to every agent as its variable (`agents.secrets`).
    pub(crate) fn agents_pick_secret(&mut self) {
        let mut names: Vec<String> = KEYS.iter().map(|k| k.to_string()).collect();
        for name in self.config.list_value("agents.secrets") {
            if !names.contains(name) {
                names.push(name.clone());
            }
        }
        let candidates = names
            .into_iter()
            .map(|name| {
                let kept = self.wasm.keychain().get(&format!("agents.{name}")).is_some();
                fenix_picker::Candidate::new(format!("{name}{}", if kept { "   (kept: Enter on an empty line takes it away)" } else { "" }), Pick::Secret(name))
            })
            .collect();
        self.enter_picker(ActivePicker::Agents(fenix_picker::PickerState::new(candidates)));
    }

    pub(super) fn agents_secret_prompt(&mut self, name: &str) {
        self.wasm.prompt = Some(crate::app::wasm_host::CodePrompt { ext: SECRET_PROMPT.into(), id: 0, label: name.to_string(), input: String::new(), masked: true });
        self.wake_caret();
    }

    /// The key typed for variable `name` (empty: taken away): kept in the
    /// keychain, the variable in `agents.secrets`.
    pub(crate) fn agents_secret_entered(&mut self, name: &str, value: &str) {
        let value = Some(value.trim()).filter(|v| !v.is_empty());
        if let Err(e) = self.wasm.keychain().set(&format!("agents.{name}"), value) {
            self.set_error(format!("{name}: {e} -- agents.env can set it instead"));
            return;
        }
        let mut names = self.config.list_value("agents.secrets").to_vec();
        names.retain(|n| n != name);
        if value.is_some() {
            names.push(name.to_string());
        }
        if let Err(e) = self.set_setting("agents.secrets", Some(fenix_config::Value::List(names))) {
            self.set_error(e);
            return;
        }
        let place = self.wasm.keychain().name().unwrap_or("the keychain").to_string();
        self.agents_configure();
        self.set_message(match value {
            Some(_) => format!("{name} kept in {place}: agents get it when they start (SPC a R restarts one)"),
            None => format!("{name} taken away"),
        });
    }
}

