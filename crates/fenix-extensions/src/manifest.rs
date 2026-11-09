//! `extension.toml`: who an extension is, which Fenix it runs in, what it
//! needs. A code extension also says when to load it (`activation`), what
//! it may do (`permissions`) and what it adds (`contributes`). Sections a
//! later Fenix reads are let through here, so a package made for that
//! Fenix still says plainly why it can't be installed in this one.

use std::collections::BTreeMap;

use serde::Deserialize;

/// The extension API this Fenix offers code extensions
/// (`fenix-extension-api`'s `wit/world.wit`). An extension built against
/// 1.x runs on any later 1.x: a minor version only adds.
pub const API: &str = "1.5.0";

/// Whether an extension built against `api` runs on this Fenix's `API`.
pub fn api_fits(api: &str) -> bool {
    let (Ok(want), Ok(have)) = (semver::Version::parse(&full_version(api)), semver::Version::parse(API)) else { return false };
    if have.major == 0 { want.major == 0 && want.minor == have.minor && want.patch <= have.patch } else { want.major == have.major && want <= have }
}

/// `0.1` as `0.1.0`.
fn full_version(v: &str) -> String {
    match v.matches('.').count() {
        0 => format!("{v}.0.0"),
        1 => format!("{v}.0"),
        _ => v.to_string(),
    }
}

/// What runs an extension: nothing (data only), or its own code -- or,
/// a bundle, nothing of its own: it's the extensions it depends on,
/// installed with it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Runtime {
    None,
    Wasm,
    Lua,
    Bundle,
}

/// What an extension may be filed under, as the Extensions page lists
/// them: `categories` names these (fenix-extensions' `xtask validate`
/// holds its manifests to them; one from elsewhere that isn't is under
/// Other).
pub const CATEGORIES: &[(&str, &str)] = &[
    ("languages", "Languages"),
    ("themes", "Themes"),
    ("snippets", "Snippets"),
    ("keymaps", "Keymaps"),
    ("templates", "Templates"),
    ("tools", "Tools"),
    ("data", "Data"),
    ("web", "Web"),
    ("mission", "Mission"),
    ("fun", "Fun"),
];

#[derive(Debug, Clone, Deserialize)]
pub struct Manifest {
    pub extension: Info,
    pub engine: Engine,
    /// Other extensions it needs, by id, with the versions it takes.
    #[serde(default)]
    pub dependencies: BTreeMap<String, String>,
    #[serde(default)]
    pub activation: Activation,
    #[serde(default)]
    pub permissions: Permissions,
    #[serde(default)]
    pub contributes: Contributes,
    /// How long its calls may take.
    #[serde(default)]
    pub limits: Limits,
}

/// How long a call into it may take before it's stopped (`[limits]`,
///).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    /// A command's, an event's or a service's, in milliseconds: 5000 when
    /// it doesn't say, at most `MAX_CALL_MS`. Laying out a page still has
    /// a second.
    #[serde(default)]
    pub call_ms: Option<u32>,
}

/// The most `limits.call_ms` may be: a minute.
pub const MAX_CALL_MS: u32 = 60_000;

/// How long a call takes when `limits.call_ms` doesn't say.
pub const CALL_MS: u32 = 5_000;

impl Limits {
    /// How long its calls may take, in milliseconds.
    pub fn call_ms(&self) -> u32 {
        self.call_ms.unwrap_or(CALL_MS)
    }
}

/// When a code extension loads: nothing loads until one of these.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct Activation {
    /// When Fenix starts.
    #[serde(default)]
    pub on_startup: bool,
    /// When one of these commands runs (`jira.*` for any of its own).
    #[serde(default)]
    pub commands: Vec<String>,
    /// When a file of one of these languages (Fenix's names: `rust`,
    /// `ada`) is opened, or a project's top or its `src/` has one. Fenix 1.1.
    #[serde(default)]
    pub languages: Vec<String>,
    /// When the project holds one of these files (`Cargo.toml`,
    /// `{folder}.gpr`, as a context's `markers`). Fenix 1.1.
    #[serde(default)]
    pub markers: Vec<String>,
}

impl Activation {
    /// What it waits for before it loads, when it's for some projects or
    /// languages only: the languages and markers `[activation]` names,
    /// and -- for one that starts with Fenix -- those its contexts name
    /// (a language pack's). Both empty: nothing of the kind; it loads when
    /// Fenix starts, if it asks to, or when it's used.
    pub fn waits_for(&self, contexts: &[ContextContribution]) -> (Vec<String>, Vec<String>) {
        let mut languages = self.languages.clone();
        let mut markers = self.markers.clone();
        if self.on_startup {
            for c in contexts {
                languages.extend(c.languages.iter().cloned());
                markers.extend(c.markers.iter().cloned());
            }
        }
        languages.sort();
        languages.dedup();
        markers.sort();
        markers.dedup();
        (languages, markers)
    }
}

/// What a code extension may do, shown before it's installed and held to
/// by the host. Nothing, unless it says.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Permissions {
    /// Hosts it may reach over HTTP(S); `*` for any.
    #[serde(default)]
    pub network: Vec<String>,
    /// Secrets it may read, and write (`secret-set`), by key, or
    /// by pattern (`http.secret.*`).
    #[serde(default)]
    pub secrets: Vec<String>,
    /// Programs it may run.
    #[serde(default)]
    pub process: Vec<String>,
    /// Sockets it may open, `host:port` or `*:5900-5999`.
    #[serde(default)]
    pub sockets: Vec<String>,
    #[serde(default)]
    pub filesystem: Filesystem,
    /// Whether it may draw over the editor (layers).
    #[serde(default)]
    pub draw: Draw,
    /// Whether it may know where you are in the editor (`here`), and
    /// change what you edit (`buffer-edit`).
    #[serde(default)]
    pub editor: Editor,
    /// Whether it may read the system's fonts, at `/fonts` (to draw
    /// text into a picture).
    #[serde(default)]
    pub fonts: bool,
    /// Folders of yours it may read and change, by name: each at
    /// `/folders/<name>`, its path a setting's (`${setting:key}`).
    #[serde(default)]
    pub folders: BTreeMap<String, String>,
    /// Whether it may read the clipboard's text (`clipboard-text`).
    #[serde(default)]
    pub clipboard: bool,
    /// Other settings it may read (`setting`), by the name before their
    /// dot: `ccsds` for the mission profile another extension keeps.
    #[serde(default)]
    pub settings: Vec<String>,
    /// Whether it may play sounds (`play-sound`): a WAV of its own.
    #[serde(default)]
    pub sound: bool,
    /// Local sockets it may reach (`ipc-connect`): Unix sockets and named
    /// pipes by name, `discord-ipc-*` or `${setting:key}`.
    #[serde(default)]
    pub ipc: Vec<String>,
    /// Ports on this machine's loopback it may listen on (`tcp-listen`,
    ///): `127.0.0.1:8400`, `127.0.0.1:8400-8410`, `127.0.0.1:0`
    /// for any free one -- an OAuth redirect.
    #[serde(default)]
    pub listen: Vec<String>,
    /// Fenix's environment variables it may read (`env-var`), by
    /// name (`JAVA_HOME`), by prefix (`JAVA_*`), or the names a setting
    /// of its holds (`${setting:key}`).
    #[serde(default)]
    pub env: Vec<String>,
    /// Whether it may talk to your coding agents: ask one
    /// (`agents.ask`), and hear what their turns end with (`contributes.events`
    /// `agents`). What it sends goes to the agent's provider.
    #[serde(default)]
    pub agents: bool,
}

/// One entry of `permissions.listen`, read: the ports of this machine's
/// loopback it lets `tcp-listen` take, 0 among them for any free one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListenRule {
    pub ports: (u16, u16),
}

impl ListenRule {
    /// `entry` read, or why it isn't one: `127.0.0.1` or `localhost`, and
    /// a port, a range, `0` (any free one) or `*` (any, 0 too).
    pub fn parse(entry: &str) -> Result<ListenRule, String> {
        let entry = entry.trim();
        let rest = entry.strip_prefix("tcp://").unwrap_or(entry);
        let (host, port) = rest.rsplit_once(':').ok_or_else(|| format!("{entry}: 127.0.0.1:port"))?;
        if !matches!(host.to_ascii_lowercase().as_str(), "127.0.0.1" | "localhost") {
            return Err(format!("{entry}: only this machine's loopback, 127.0.0.1, may be listened on"));
        }
        let number = |p: &str| p.trim().parse::<u16>().map_err(|_| format!("{entry}: a port is a number, a range (8400-8410), 0 or *"));
        let ports = match port.trim() {
            "*" => (0, u16::MAX),
            p => match p.split_once('-') {
                Some((a, b)) => (number(a)?, number(b)?),
                None => (number(p)?, number(p)?),
            },
        };
        if ports.0 > ports.1 {
            return Err(format!("{entry}: an empty range of ports"));
        }
        Ok(ListenRule { ports })
    }

    /// Whether it lets `tcp-listen` take `port` (0: any free one).
    pub fn takes(&self, port: u16) -> bool {
        (self.ports.0..=self.ports.1).contains(&port)
    }
}

/// Whether environment variable `name` is one `allowed` (`permissions.env`)
/// names: by name, or by a prefix ending in `*`; `setting` reads the names
/// a `${setting:key}` entry's setting holds (a list, or a line of names
/// with commas between them). Letters' case is ignored on Windows.
pub fn env_allowed(allowed: &[String], name: &str, setting: &dyn Fn(&str) -> Option<String>) -> bool {
    let same = |a: &str, b: &str| if cfg!(windows) { a.eq_ignore_ascii_case(b) } else { a == b };
    let named = |entry: &str| match entry.strip_suffix('*') {
        Some(prefix) => !prefix.is_empty() && name.len() >= prefix.len() && same(&name[..prefix.len()], prefix),
        None => same(entry, name),
    };
    !name.is_empty()
        && !name.contains(['=', '\0'])
        && allowed.iter().any(|entry| match entry.trim().strip_prefix("${setting:").and_then(|k| k.strip_suffix('}')) {
            Some(key) => setting(key).is_some_and(|value| {
                let names: Vec<String> = serde_json::from_str::<Vec<String>>(value.trim()).unwrap_or_else(|_| value.split([',', '\n']).map(|s| s.trim().to_string()).collect());
                names.iter().any(|n| !n.is_empty() && !n.contains('*') && same(n, name))
            }),
            None => named(entry.trim()),
        })
}

/// Whether secret `key` is one `allowed` (`permissions.secrets`) names: by
/// key, or by a pattern (`http.secret.*`).
pub fn secret_allowed(allowed: &[String], key: &str) -> bool {
    !key.is_empty() && allowed.iter().any(|entry| if entry.contains(['*', '?']) { glob_matches(entry, key) } else { entry == key })
}

/// One entry of `permissions.sockets`, read: the protocols, hosts and
/// ports it lets a socket reach. `tcp://` or `udp://` keeps it to one
/// protocol (any other scheme, `nats://`, is TCP); a host is a name,
/// `*.example.com`, `*` for any, or nothing for this machine's own ports
/// (`:5000`, what `udp-bind` takes); a port is a number, `5900-5999`, or
/// `*`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SocketRule {
    pub tcp: bool,
    pub udp: bool,
    /// Lowercase; `*` any, `*.x` under x, "" this machine's own ports.
    pub host: String,
    pub ports: (u16, u16),
}

impl SocketRule {
    /// `entry` read, or why it isn't one. `${setting:...}` isn't read here:
    /// its addresses are (`SocketRule::from_setting`).
    pub fn parse(entry: &str) -> Result<SocketRule, String> {
        let entry = entry.trim();
        let (tcp, udp, rest) = match entry.split_once("://") {
            Some((scheme, rest)) => match scheme.to_ascii_lowercase().as_str() {
                "udp" => (false, true, rest),
                "tcp" => (true, false, rest),
                _ => (true, false, rest),
            },
            None => (true, true, entry),
        };
        let rest = rest.trim_end_matches('/');
        let (host, port) = if let Some(v6) = rest.strip_prefix('[') {
            let (host, port) = v6.split_once("]:").ok_or_else(|| format!("{entry}: host:port"))?;
            (host.to_string(), port)
        } else {
            let (host, port) = rest.rsplit_once(':').ok_or_else(|| format!("{entry}: host:port"))?;
            (host.to_ascii_lowercase(), port)
        };
        let number = |p: &str| p.trim().parse::<u16>().map_err(|_| format!("{entry}: a port is a number, a range (5900-5999) or *"));
        let ports = match port.trim() {
            "*" => (1, u16::MAX),
            p => match p.split_once('-') {
                Some((a, b)) => (number(a)?, number(b)?),
                None => (number(p)?, number(p)?),
            },
        };
        if ports.0 > ports.1 || ports.1 == 0 {
            return Err(format!("{entry}: an empty range of ports"));
        }
        if host.contains(['/', ' ', '@']) || (host.contains('*') && host != "*" && !(host.starts_with("*.") && !host[2..].contains('*'))) {
            return Err(format!("{entry}: a host is a name, *.example.com, * or nothing (this machine)"));
        }
        Ok(SocketRule { tcp, udp, host, ports })
    }

    /// The addresses a setting's value holds, as rules: a `host:port`
    /// (with a scheme or not); a list's items; a table's (records, a map,
    /// as JSON) fields that are one, or a host field followed by a port
    /// field (`vnc.hosts`' rows: name, host, port).
    pub fn from_setting(value: &str) -> Vec<SocketRule> {
        let value = value.trim();
        let rows: Vec<Vec<String>> = match serde_json::from_str::<serde_json::Value>(value) {
            Ok(serde_json::Value::Array(rows)) => rows
                .into_iter()
                .map(|row| match row {
                    serde_json::Value::Array(fields) => fields.into_iter().map(|f| f.as_str().map(str::to_string).unwrap_or_else(|| f.to_string())).collect(),
                    other => vec![other.as_str().map(str::to_string).unwrap_or_else(|| other.to_string())],
                })
                .collect(),
            _ => vec![value.split(',').map(|s| s.trim().to_string()).collect()],
        };
        let mut rules = Vec::new();
        for row in rows {
            for (i, field) in row.iter().enumerate() {
                let field = field.trim();
                if field.is_empty() || field.contains('*') {
                    continue;
                }
                if let Ok(rule) = SocketRule::parse(field) {
                    rules.push(rule);
                } else if let (Some(prev), Ok(port)) = (i.checked_sub(1).map(|p| row[p].trim()), field.parse::<u16>()) {
                    if !prev.is_empty() && !prev.contains([':', '*', '/']) && prev.parse::<u16>().is_err() {
                        if let Ok(rule) = SocketRule::parse(&format!("{prev}:{port}")) {
                            rules.push(rule);
                        }
                    }
                }
            }
        }
        rules
    }

    /// Whether it lets a connection (`udp`: a datagram) to `host`:`port`.
    pub fn reaches(&self, udp: bool, host: &str, port: u16) -> bool {
        let host = host.trim().trim_start_matches('[').trim_end_matches(']').to_ascii_lowercase();
        let protocol = if udp { self.udp } else { self.tcp };
        let named = match self.host.strip_prefix("*.") {
            _ if self.host == "*" => true,
            Some(domain) => host.ends_with(&format!(".{domain}")),
            None => !self.host.is_empty() && self.host == host,
        };
        protocol && named && !host.is_empty() && (self.ports.0..=self.ports.1).contains(&port)
    }

    /// Whether it lets a UDP socket take this machine's `port`.
    pub fn binds(&self, port: u16) -> bool {
        self.udp && self.host.is_empty() && (self.ports.0..=self.ports.1).contains(&port)
    }
}

/// Whether `name` matches `pattern`: `*` any run of characters (but `/`),
/// `**` any at all, `?` one; letters' case is ignored.
pub fn glob_matches(pattern: &str, name: &str) -> bool {
    fn go(p: &[char], n: &[char]) -> bool {
        match p.first() {
            None => n.is_empty(),
            Some('*') if p.get(1) == Some(&'*') => (0..=n.len()).any(|i| go(&p[2..], &n[i..])),
            Some('*') => (0..=n.len()).take_while(|i| *i == 0 || n[i - 1] != '/').any(|i| go(&p[1..], &n[i..])),
            Some('?') => !n.is_empty() && n[0] != '/' && go(&p[1..], &n[1..]),
            Some(c) => n.first().is_some_and(|m| m.to_lowercase().eq(c.to_lowercase())) && go(&p[1..], &n[1..]),
        }
    }
    let p: Vec<char> = pattern.replace('\\', "/").chars().collect();
    let n: Vec<char> = name.replace('\\', "/").chars().collect();
    go(&p, &n)
}

/// Whether file `path` is one of `patterns`: a pattern with no `/` is
/// matched against its name (`*.dat`), one with a `/` against its path
/// below `root` (`mib/*.dat`, `**/tables/*.dat`).
pub fn file_matches(patterns: &[String], path: &std::path::Path, root: Option<&std::path::Path>) -> bool {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let below = root.and_then(|r| path.strip_prefix(r).ok()).map(|p| p.to_string_lossy().replace('\\', "/"));
    patterns.iter().any(|p| if p.contains('/') { below.as_deref().is_some_and(|b| glob_matches(p.trim_start_matches("./"), b)) } else { glob_matches(p, &name) })
}

/// Whether a code extension may know where you are in the editor.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Editor {
    #[default]
    None,
    /// The file you're in, its project, and what you've selected, when
    /// you run one of its commands; the text of the buffers you edit.
    Read,
    /// That, and changing what's in the buffers you edit.
    Write,
}

impl Editor {
    /// Whether it may know where you are.
    pub fn reads(self) -> bool {
        matches!(self, Editor::Read | Editor::Write)
    }
}

/// Whether a code extension may draw over the editor.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Draw {
    #[default]
    None,
    /// Layers over (and behind) the text: its own images, anywhere.
    Over,
}

impl Permissions {
    /// Whether it asks for nothing at all.
    pub fn none(&self) -> bool {
        *self == Permissions::default()
    }

    /// What it asks for, a line each, as the store shows it.
    pub fn lines(&self) -> Vec<String> {
        let mut out = Vec::new();
        // `${setting:database.connections}` said as what it is, not as written.
        let shown = |item: &String| match item.trim().strip_prefix("${setting:").and_then(|k| k.strip_suffix('}')) {
            Some(key) => format!("those your {key} setting names"),
            None => item.clone(),
        };
        let list = |what: &str, items: &[String]| (!items.is_empty()).then(|| format!("{what}: {}", items.iter().map(shown).collect::<Vec<_>>().join(", ")));
        if self.network.iter().any(|h| h.trim() == "*") {
            out.push("Reach any host on the internet".into());
        } else {
            out.extend(list("Reach over the network", &self.network));
        }
        out.extend(list("Read and keep the secrets", &self.secrets));
        out.extend(list("Run the programs", &self.process));
        out.extend(list("Open sockets to", &self.sockets));
        out.extend(list("Talk to the programs on this machine through", &self.ipc));
        out.extend(list("Listen on this machine's ports", &self.listen));
        out.extend(list("Read Fenix's environment variables", &self.env));
        match self.filesystem {
            Filesystem::None => {}
            Filesystem::ProjectRead => out.push("Read the files of the open project".into()),
            Filesystem::ProjectWrite => out.push("Read and change the files of the open project".into()),
            Filesystem::UserPicked => out.push("Read and change files you pick for it".into()),
        }
        if self.draw == Draw::Over {
            out.push("Draw over the editor".into());
        }
        match self.editor {
            Editor::None => {}
            Editor::Read => out.push("See the file you're in and what you've selected, when you run its commands".into()),
            Editor::Write => out.push("See and change what you're editing".into()),
        }
        if self.fonts {
            out.push("Read your fonts".into());
        }
        if self.clipboard {
            out.push("Read the clipboard".into());
        }
        if self.sound {
            out.push("Play sounds".into());
        }
        if self.agents {
            out.push("Ask your coding agents, and read what they answer".into());
        }
        for (name, path) in &self.folders {
            let setting = path.strip_prefix("${setting:").and_then(|k| k.strip_suffix('}')).unwrap_or(path);
            out.push(format!("Read and change your {name} folder ({setting})"));
        }
        for name in &self.settings {
            out.push(format!("Read your {name}.* settings"));
        }
        out
    }
}

/// Which files a code extension may touch.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Filesystem {
    #[default]
    None,
    ProjectRead,
    ProjectWrite,
    UserPicked,
}

/// What a code extension adds to Fenix.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct Contributes {
    #[serde(default)]
    pub commands: Vec<CommandContribution>,
    /// Settings of its own, on the settings page and in `settings.toml`.
    #[serde(default)]
    pub settings: Vec<SettingContribution>,
    /// The services it answers (`serve`), by name.
    #[serde(default)]
    pub services: Vec<String>,
    /// Those services agents may call, as tools (`fenix --mcp`).
    #[serde(default)]
    pub agent_tools: Vec<AgentToolContribution>,
    /// What agents may read from it, as resources (1.2).
    #[serde(default)]
    pub agent_resources: Vec<AgentResourceContribution>,
    /// The prompts it offers agents' clients (1.2).
    #[serde(default)]
    pub agent_prompts: Vec<AgentPromptContribution>,
    /// Where its `SPC m` menu applies (`[[local.<name>]]` in its keymaps).
    #[serde(default)]
    pub contexts: Vec<ContextContribution>,
    /// Other names its commands, settings and services may start with
    /// besides its own: `diagram` for the Diagrams extension's
    /// `diagram.preview`.
    #[serde(default)]
    pub namespaces: Vec<String>,
    /// The language servers it is, for Fenix's languages: each runs
    /// beside their own servers, its answers merged with theirs.
    #[serde(default)]
    pub language_servers: Vec<LanguageServerContribution>,
    /// What you're doing that it's told of: `saved`, `diagnostics`,
    /// `tasks`, `typing`, `active`; and, as exports of their own,
    /// `changed` (a file's buffer, `buffer-changed`) and `focus`
    /// (`focus-moved`); `view`; `debug` (`debug-changed`),
    /// and with `tasks`, `task-started` too.
    #[serde(default)]
    pub events: Vec<String>,
    /// The files it opens itself, in place of a text buffer (`*.pdf`):
    /// Fenix tells it with `opened`, and it shows them as it likes -- a
    /// surface, a page.
    #[serde(default)]
    pub opens: Vec<String>,
    /// What a file of one of `opens`' patterns starts with, by the pattern
    ///: one that doesn't start so opens as text, not to it -- a
    /// `.db` that isn't SQLite (`"*.db" = "SQLite format 3"`). As text, or
    /// as bytes in hex after `hex:` (`"hex:89504e47"`).
    #[serde(default)]
    pub magic: BTreeMap<String, String>,
    /// The code-review forges it is (`forge.*` services, which Fenix's Git
    /// pages call), by the hosts of the remotes they serve.
    #[serde(default)]
    pub forges: Vec<ForgeContribution>,
    /// The coding agents it brings: each an ACP server it starts
    /// as it says (`agent-provide`), offered beside Fenix's own.
    #[serde(default)]
    pub agents: Vec<AgentContribution>,
}

/// A coding agent an extension brings: one that speaks ACP on stdio, which
/// it installs in its tools folder, or finds, and names with
/// `agent-provide`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentContribution {
    /// Its id, for `agents.default` and the threads: the extension's own
    /// name (`cline` for `acme.cline`), or one starting with it
    /// (`cline.fast`).
    pub id: String,
    /// What it's called on the agents page.
    pub name: String,
    /// How to log in to it, for the agents page.
    #[serde(default)]
    pub login: Option<String>,
}

/// The ids of Fenix's own agents, which no extension's may take.
pub const BUILTIN_AGENTS: &[&str] = &["claude", "codex", "devin", "gemini"];

/// A code-review forge an extension is: GitHub, GitLab. Fenix's Git pages
/// ask it, through its `serve`, about a repository whose `origin` is on
/// one of `hosts`: `forge.branch` (the branch's request, for the status
/// page's header), `forge.open` (`o` there: that request, or a new one)
/// and `forge.reviews` (the reviews waiting).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForgeContribution {
    /// What it's called: `GitHub`.
    pub name: String,
    /// The hosts of the remotes it serves (`github.com`): a name,
    /// `*.example.com`, `${setting:key}` for the host of the URL a
    /// setting of its holds, or `*` for any the others don't claim (a
    /// self-hosted GitLab).
    pub hosts: Vec<String>,
}

impl ForgeContribution {
    /// How well it claims a remote on `host`: 2 by name, 1 as `*`, 0 not
    /// at all. `setting` reads one of its settings.
    pub fn claims(&self, host: &str, setting: &dyn Fn(&str) -> Option<String>) -> u8 {
        let host = host.to_ascii_lowercase();
        let mut best = 0;
        for entry in &self.hosts {
            let entry = match entry.strip_prefix("${setting:").and_then(|k| k.strip_suffix('}')) {
                Some(key) => match setting(key) {
                    Some(value) => url_host(&value).unwrap_or_else(|| value.trim().to_ascii_lowercase()),
                    None => continue,
                },
                None => entry.trim().to_ascii_lowercase(),
            };
            let claimed = match entry.strip_prefix("*.") {
                _ if entry == "*" => 1,
                Some(domain) if host.ends_with(&format!(".{domain}")) => 2,
                None if !entry.is_empty() && entry == host => 2,
                _ => 0,
            };
            best = best.max(claimed);
        }
        best
    }
}

/// The host of a remote's or a server's URL: `https://Host:443/x`,
/// `git@host:owner/repo.git`, `ssh://git@host:22/x`.
pub fn url_host(url: &str) -> Option<String> {
    let url = url.trim();
    let rest = match url.split_once("://") {
        Some((_, rest)) => rest,
        None => url.split_once(':').map(|(head, _)| head)?,
    };
    let authority = rest.split(['/', '?', '#']).next()?;
    let host = authority.rsplit('@').next()?;
    let host = if host.starts_with('[') { host.split(']').next().map(|h| format!("{h}]"))? } else { host.split(':').next()?.to_string() };
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

/// A language server an extension is (`lsp-message`, `lsp-send`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LanguageServerContribution {
    /// What it's called in Fenix's messages and tool status. A file it
    /// serves by pattern that isn't one of Fenix's languages is opened to
    /// it with this as its `languageId`.
    pub name: String,
    /// Fenix's names for the languages it serves (`tcl`, `markdown`,
    /// `mermaid`).
    #[serde(default)]
    pub languages: Vec<String>,
    /// The files it serves besides, by pattern, whatever their language
    /// -- or files Fenix has no language for (`*.dat`, `mib/*.dat`; see
    /// `file_matches`).
    #[serde(default)]
    pub files: Vec<String>,
}

impl LanguageServerContribution {
    /// Whether it serves file `path`, in language `language` (Fenix's name
    /// for it, if it has one), in the project at `root`.
    pub fn serves(&self, path: &std::path::Path, language: Option<&str>, root: Option<&std::path::Path>) -> bool {
        language.is_some_and(|l| self.languages.iter().any(|x| x == l)) || file_matches(&self.files, path, root)
    }
}

/// The bytes a `contributes.magic` entry says a file starts with: its text,
/// or, after `hex:`, the bytes its hex digits spell.
pub fn magic_bytes(magic: &str) -> Result<Vec<u8>, String> {
    let bytes = match magic.strip_prefix("hex:") {
        Some(hex) => {
            let hex: String = hex.chars().filter(|c| !c.is_whitespace()).collect();
            if !hex.len().is_multiple_of(2) || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
                return Err("hex: is followed by pairs of hex digits".into());
            }
            (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap_or_default()).collect()
        }
        None => magic.as_bytes().to_vec(),
    };
    match bytes.len() {
        0 => Err("it says nothing a file starts with".into()),
        1..=256 => Ok(bytes),
        _ => Err("at most 256 bytes".into()),
    }
}

impl Contributes {
    /// Whether it opens file `path` itself: one of `opens`, and, where
    /// `magic` says what such a file starts with, it does (`head` reads
    /// the file's first bytes).
    pub fn opens_file(&self, path: &std::path::Path, head: &dyn Fn(usize) -> Vec<u8>) -> bool {
        self.opens.iter().any(|pattern| {
            if !file_matches(std::slice::from_ref(pattern), path, None) {
                return false;
            }
            match self.magic.get(pattern).and_then(|m| magic_bytes(m).ok()) {
                Some(magic) => head(magic.len()).starts_with(&magic),
                None => true,
            }
        })
    }
}

/// The editor events an extension may ask to be told of.
pub const EDITOR_EVENTS: &[&str] = &["saved", "diagnostics", "tasks", "typing", "active", "changed", "focus", "view", "debug", "agents", "git"];

/// A kind of place its `SPC m` menu is for: a file is in it when a folder
/// it's in holds one of `markers` (`{folder}` in a marker is that
/// folder's own name: an Arduino sketch's `{folder}.ino`), or when it's
/// in one of `languages` (Fenix's names: `mermaid`); and one of its own
/// pages or surfaces is when it's of one of `pages`' kinds (the PDF
/// reader's `pdf`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextContribution {
    pub name: String,
    #[serde(default)]
    pub markers: Vec<String>,
    #[serde(default)]
    pub languages: Vec<String>,
    #[serde(default)]
    pub pages: Vec<String>,
}

impl ContextContribution {
    /// Whether `dir` holds one of its markers.
    pub fn holds(&self, dir: &std::path::Path) -> bool {
        let folder = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        self.markers.iter().any(|m| dir.join(m.replace("{folder}", &folder)).is_file())
    }

    /// The folder `path` (a file, or a folder) is in that holds one of its
    /// markers, nearest first.
    pub fn root_of(&self, path: &std::path::Path) -> Option<std::path::PathBuf> {
        let start = if path.is_dir() { path } else { path.parent()? };
        start.ancestors().find(|dir| self.holds(dir)).map(std::path::Path::to_path_buf)
    }
}

/// Text, or a TOML boolean, integer or float written as text: a setting's
/// `default`, which fenix.java 1.1.0 wrote as `default = true`.
fn text_or_scalar<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Scalar {
        Text(String),
        Bool(bool),
        Int(i64),
        Float(f64),
    }
    Ok(match Scalar::deserialize(deserializer)? {
        Scalar::Text(text) => text,
        Scalar::Bool(b) => b.to_string(),
        Scalar::Int(n) => n.to_string(),
        Scalar::Float(x) => x.to_string(),
    })
}

/// One of its settings.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SettingContribution {
    /// `docker.host`: its own name first.
    pub key: String,
    /// `text`, `bool`, `int`, `path`, `list`, `choice` or `secret`.
    #[serde(default = "text_kind")]
    pub kind: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub description: String,
    /// What applies when it isn't set, as the settings page says it. A
    /// `bool`'s or an `int`'s may be written bare (`default = true`), as
    /// TOML has it; it's kept as the text.
    #[serde(default, deserialize_with = "text_or_scalar")]
    pub default: String,
    /// For a `choice`: the words it takes.
    #[serde(default)]
    pub choices: Vec<String>,
    /// A `map`'s two columns, as the settings page heads them
    /// (`["Key", "Name"]`); a `records` setting's fields, each row a value
    /// for each.
    #[serde(default)]
    pub columns: Vec<String>,
    /// Which part of the settings page; the extension's name when left out.
    #[serde(default)]
    pub section: Option<String>,
    /// A project may set it too, in its `.fenix/settings.toml`: the
    /// extension reads the project's over yours.
    #[serde(default)]
    pub project: bool,
}

fn text_kind() -> String {
    "text".into()
}

/// The kinds a contributed setting may be.
pub const SETTING_KINDS: &[&str] = &["text", "bool", "int", "minutes", "path", "list", "map", "records", "choice", "secret"];

/// One of its services, as a tool coding agents can call (`fenix --mcp`).
/// It's given the tool's arguments as JSON, and what it answers goes back.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AgentToolContribution {
    /// `search`: agents see `<the extension's last name>_search`.
    pub name: String,
    /// What it does, for the agent deciding whether to call it.
    pub description: String,
    /// The service it calls, from `contributes.services`.
    pub service: String,
    /// The JSON Schema of its arguments, as text.
    #[serde(default)]
    pub input: Option<String>,
    /// It changes something: only with `agents.mcp.fenix_write`. Read
    /// as its class when it has none (`effective_class`).
    #[serde(default)]
    pub writes: bool,
    /// What it may do (1.2): `read`, `show`, `edit`, `run` or `external`.
    /// Agents' rules are per class.
    #[serde(default)]
    pub class: Option<ToolClass>,
    /// How long a call may take, an answer given later included (1.2):
    /// `TOOL_TIMEOUT_MS` when it doesn't say, at most `MAX_TOOL_TIMEOUT_MS`.
    #[serde(default)]
    pub timeout_ms: Option<u32>,
    /// Offered to agents by default, when the extension is on (1.2): its
    /// toolset (the extension's short name) is in every session's list.
    #[serde(default)]
    pub default: bool,
}

/// What a tool may do: a rule for each class says whether an agent may
/// call it, must ask, or may not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolClass {
    /// Reads what Fenix or the extension knows.
    Read,
    /// Moves what's on your screen.
    Show,
    /// Changes buffers and files.
    Edit,
    /// Starts processes, runs commands.
    Run,
    /// Acts outside this machine: posts, sends, changes a remote system.
    External,
}

impl ToolClass {
    /// Its name in a manifest, and in `fenix/tools`.
    pub fn as_str(self) -> &'static str {
        match self {
            ToolClass::Read => "read",
            ToolClass::Show => "show",
            ToolClass::Edit => "edit",
            ToolClass::Run => "run",
            ToolClass::External => "external",
        }
    }
}

/// How long a tool's call may take when its manifest doesn't say: 30 s.
pub const TOOL_TIMEOUT_MS: u32 = 30_000;
/// The most a tool's `timeout_ms` may be: 10 minutes.
pub const MAX_TOOL_TIMEOUT_MS: u32 = 600_000;
/// The longest a tool's (or a resource's, a prompt's) description may be,
/// in characters, for an extension built for 1.2.
pub const MAX_TOOL_DESCRIPTION: usize = 300;
/// The largest a tool's input schema may be, in bytes.
pub const MAX_TOOL_SCHEMA: usize = 2048;

impl AgentToolContribution {
    /// Its class: the one it says, else `writes` read the safe way --
    /// `run` when it writes, `read` when it doesn't.
    pub fn effective_class(&self) -> ToolClass {
        self.class.unwrap_or(if self.writes { ToolClass::Run } else { ToolClass::Read })
    }

    /// How long a call may take, in milliseconds.
    pub fn timeout_ms(&self) -> u32 {
        self.timeout_ms.unwrap_or(TOOL_TIMEOUT_MS)
    }
}

/// Something agents can read from an extension, as an MCP resource (1.2):
/// a URI template (`fenix://ext/jira/issue/{key}`) and the service that
/// reads one. The service is given `{"uri": ..., "params": {"key": ...}}`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentResourceContribution {
    pub uri_template: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// The service it calls, from `contributes.services`.
    pub service: String,
    #[serde(default)]
    pub mime_type: Option<String>,
}

impl AgentResourceContribution {
    /// The names in its template's braces, in order: `key` for
    /// `fenix://ext/jira/issue/{key}`.
    pub fn parameters(&self) -> Vec<String> {
        let mut out = Vec::new();
        let mut rest = self.uri_template.as_str();
        while let Some(open) = rest.find('{') {
            let Some(close) = rest[open..].find('}') else { break };
            out.push(rest[open + 1..open + close].to_string());
            rest = &rest[open + close + 1..];
        }
        out
    }
}

/// A prompt agents' clients can offer, as an MCP prompt (1.2): its
/// service is given the arguments as a JSON object and answers the
/// prompt's messages.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentPromptContribution {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub arguments: Vec<PromptArgument>,
    /// The service it calls, from `contributes.services`.
    pub service: String,
}

/// One of a prompt's arguments.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptArgument {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub required: bool,
}

/// A command, run by calling the extension's `run-command`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct CommandContribution {
    /// `jira.open`: its own id first.
    pub id: String,
    pub title: String,
    /// Whether it may run from outside Fenix (`fenix/runCommand`).
    #[serde(default)]
    pub external: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Info {
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub authors: Vec<String>,
    #[serde(default)]
    pub license: Option<String>,
    #[serde(default)]
    pub repository: Option<String>,
    #[serde(default)]
    pub categories: Vec<String>,
    pub runtime: Runtime,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Engine {
    /// The Fenix releases it runs in, as a version range (`>=1.0, <2`).
    pub fenix: String,
    /// The extension API a code extension was built against (`0.1`).
    #[serde(default)]
    pub api: Option<String>,
}

impl Manifest {
    /// `extension.toml`'s text read and checked: a well-formed id, a
    /// version and ranges that parse.
    pub fn parse(text: &str) -> Result<Manifest, String> {
        let m: Manifest = toml::from_str(text).map_err(|e| format!("extension.toml: {}", e.message()))?;
        m.check()?;
        Ok(m)
    }

    fn check(&self) -> Result<(), String> {
        let id = &self.extension.id;
        if !valid_id(id) {
            return Err(format!("extension.toml: id {id:?} -- an id is publisher.name, in lowercase letters, digits, - and _"));
        }
        if self.extension.name.trim().is_empty() {
            return Err("extension.toml: name is empty".into());
        }
        self.version()?;
        semver::VersionReq::parse(&self.engine.fenix).map_err(|e| format!("extension.toml: engine.fenix {:?}: {e}", self.engine.fenix))?;
        if self.extension.runtime == Runtime::Wasm {
            let api = self.engine.api.as_deref().ok_or("extension.toml: a WebAssembly extension says the API it was built against (engine.api)")?;
            semver::Version::parse(&full_version(api)).map_err(|e| format!("extension.toml: engine.api {api:?}: {e}"))?;
        }
        let own = format!("{}.", self.extension.id.rsplit('.').next().unwrap_or_default());
        let ours = |name: &str| name.starts_with(&own) || name.starts_with(&format!("{}.", self.extension.id)) || self.contributes.namespaces.iter().any(|n| name.starts_with(&format!("{n}.")));
        for command in &self.contributes.commands {
            if !ours(&command.id) {
                return Err(format!("extension.toml: command {:?} -- an extension's commands start with its name ({own}...)", command.id));
            }
        }
        for setting in &self.contributes.settings {
            if !ours(&setting.key) {
                return Err(format!("extension.toml: setting {:?} -- an extension's settings start with its name ({own}...)", setting.key));
            }
            if !SETTING_KINDS.contains(&setting.kind.as_str()) {
                return Err(format!("extension.toml: setting {} is a {:?} -- one of {}", setting.key, setting.kind, SETTING_KINDS.join(", ")));
            }
            if setting.kind == "choice" && setting.choices.is_empty() {
                return Err(format!("extension.toml: setting {} is a choice of nothing (choices)", setting.key));
            }
        }
        self.check_agent_contributions()?;
        for service in &self.contributes.services {
            if !ours(service) {
                return Err(format!("extension.toml: service {service:?} -- an extension's services start with its name ({own}...)"));
            }
        }
        for entry in &self.permissions.sockets {
            let by_setting = entry.trim().strip_prefix("${setting:").is_some_and(|k| k.ends_with('}'));
            if !by_setting {
                SocketRule::parse(entry).map_err(|e| format!("extension.toml: permissions.sockets: {e}"))?;
            }
        }
        if let Some(bad) = self.permissions.ipc.iter().find(|n| n.trim().is_empty() || n.contains("..")) {
            return Err(format!("extension.toml: permissions.ipc: {bad:?} -- a local socket's name, like discord-ipc-*"));
        }
        for entry in &self.permissions.listen {
            ListenRule::parse(entry).map_err(|e| format!("extension.toml: permissions.listen: {e}"))?;
        }
        let by_setting = |e: &str| e.trim().strip_prefix("${setting:").is_some_and(|k| k.len() > 1 && k.ends_with('}'));
        if let Some(bad) = self.permissions.env.iter().find(|e| !by_setting(e) && (e.trim().is_empty() || e.trim() == "*" || e.contains(['=', '$', ' ']) || e.trim_end_matches('*').contains('*'))) {
            return Err(format!("extension.toml: permissions.env: {bad:?} -- a variable's name (JAVA_HOME), a prefix (JAVA_*) or ${{setting:key}}"));
        }
        if let Some(bad) = self.permissions.secrets.iter().find(|k| k.trim().is_empty() || k.trim() == "*" || k.starts_with('*')) {
            return Err(format!("extension.toml: permissions.secrets: {bad:?} -- a secret's key (jira.token), or a pattern under a name (http.secret.*)"));
        }
        if let Some(ms) = self.limits.call_ms.filter(|ms| *ms == 0 || *ms > MAX_CALL_MS) {
            return Err(format!("extension.toml: limits.call_ms is {ms} -- from 1 to {MAX_CALL_MS} (a minute)"));
        }
        for (pattern, magic) in &self.contributes.magic {
            if !self.contributes.opens.contains(pattern) {
                return Err(format!("extension.toml: contributes.magic: {pattern:?} isn't one of the files it opens (contributes.opens)"));
            }
            magic_bytes(magic).map_err(|e| format!("extension.toml: contributes.magic {pattern:?}: {e}"))?;
        }
        // A name before a setting's dot: never secrets', which `secrets`
        // asks for one at a time.
        for name in &self.permissions.settings {
            if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
                return Err(format!("extension.toml: permissions.settings: {name:?} -- the name before a setting's dot, like ccsds"));
            }
        }
        for server in &self.contributes.language_servers {
            if server.languages.is_empty() && server.files.is_empty() {
                return Err(format!("extension.toml: language server {} -- it serves languages, files, or both: it needs one or the other", server.name));
            }
        }
        if self.contributes.opens.iter().chain(self.contributes.language_servers.iter().flat_map(|s| &s.files)).any(|p| p.trim().is_empty() || p.contains("..")) {
            return Err("extension.toml: a file pattern is a name (*.pdf) or a path below the project (mib/*.dat)".into());
        }
        if let Some(forge) = self.contributes.forges.iter().find(|f| f.name.trim().is_empty() || f.hosts.is_empty()) {
            return Err(format!("extension.toml: forge {:?} -- a forge has a name and the hosts it serves", forge.name));
        }
        if let Some(event) = self.contributes.events.iter().find(|e| !EDITOR_EVENTS.contains(&e.as_str())) {
            return Err(format!("extension.toml: event {event:?} -- it's one of {}", EDITOR_EVENTS.join(", ")));
        }
        if self.contributes.events.iter().any(|e| e == "changed") && !self.permissions.editor.reads() {
            return Err("extension.toml: event \"changed\" says which file you edit: it needs editor = \"read\" in its permissions".into());
        }
        let short = self.extension.id.rsplit('.').next().unwrap_or_default();
        for agent in &self.contributes.agents {
            let id = agent.id.as_str();
            if id.is_empty() || !id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_' | '.')) {
                return Err(format!("extension.toml: agent {id:?} -- an agent's id is lowercase letters, digits, -, _ and ."));
            }
            if BUILTIN_AGENTS.contains(&id) || !(id == short || ours(id)) {
                return Err(format!("extension.toml: agent {id:?} -- an extension's agents are called by its name ({short}, or {own}...)"));
            }
            if agent.name.trim().is_empty() {
                return Err(format!("extension.toml: agent {id} has no name"));
            }
        }
        for context in &self.contributes.contexts {
            if context.name.is_empty() || !context.name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
                return Err(format!("extension.toml: context {:?} -- a context's name is lowercase letters, digits and -", context.name));
            }
            if context.markers.is_empty() && context.languages.is_empty() && context.pages.is_empty() {
                return Err(format!("extension.toml: context {} -- it's where a folder holds one of its markers, a file is in one of its languages, or one of its pages is of one of its kinds: it needs one of those", context.name));
            }
            if context.markers.iter().any(|m| m.is_empty() || m.contains("..") || m.starts_with(['/', '\\'])) {
                return Err(format!("extension.toml: context {} -- its markers are file names in a folder", context.name));
            }
        }
        for (dep, range) in &self.dependencies {
            if !valid_id(dep) {
                return Err(format!("extension.toml: dependency {dep:?} isn't an extension id"));
            }
            semver::VersionReq::parse(range).map_err(|e| format!("extension.toml: dependency {dep} {range:?}: {e}"))?;
        }
        if self.extension.runtime == Runtime::Bundle {
            if self.dependencies.is_empty() {
                return Err("extension.toml: a bundle is the extensions it depends on, and it has none ([dependencies])".into());
            }
            if !self.permissions.none() || self.contributes != Contributes::default() || self.activation != Activation::default() || self.limits != Limits::default() {
                return Err("extension.toml: a bundle runs nothing of its own -- no permissions, contributions or activation".into());
            }
        }
        Ok(())
    }

    /// `agent_tools`, `agent_resources` and `agent_prompts`: names, the
    /// services they call, and -- for one built for 1.2 -- the sizes
    /// agents are given and a class its permissions fit.
    fn check_agent_contributions(&self) -> Result<(), String> {
        let snake = |name: &str| !name.is_empty() && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
        // Held to the 1.2 limits when it's built for 1.2: one built before
        // keeps loading as it did (`xtask validate` holds them all).
        let strict = self.engine.api.as_deref().and_then(|a| semver::Version::parse(&full_version(a)).ok()).is_some_and(|v| v >= semver::Version::new(1, 2, 0));
        let long = |description: &str| strict && description.chars().count() > MAX_TOOL_DESCRIPTION;
        let p = &self.permissions;
        for tool in &self.contributes.agent_tools {
            if !snake(&tool.name) {
                return Err(format!("extension.toml: agent tool {:?} -- lower case letters, digits and _", tool.name));
            }
            if !self.contributes.services.contains(&tool.service) {
                return Err(format!("extension.toml: agent tool {:?} -- {:?} isn't one of its services", tool.name, tool.service));
            }
            if let Some(input) = &tool.input {
                if !serde_json::from_str::<serde_json::Value>(input).is_ok_and(|v| v.is_object()) {
                    return Err(format!("extension.toml: agent tool {:?} -- input isn't a JSON object", tool.name));
                }
                if strict && input.len() > MAX_TOOL_SCHEMA {
                    return Err(format!("extension.toml: agent tool {:?} -- its input schema is {} bytes, and agents are given at most {MAX_TOOL_SCHEMA}", tool.name, input.len()));
                }
            }
            if long(&tool.description) {
                return Err(format!("extension.toml: agent tool {:?} -- its description is over {MAX_TOOL_DESCRIPTION} characters", tool.name));
            }
            if let Some(ms) = tool.timeout_ms.filter(|ms| *ms == 0 || *ms > MAX_TOOL_TIMEOUT_MS) {
                return Err(format!("extension.toml: agent tool {:?} -- timeout_ms is {ms}: from 1 to {MAX_TOOL_TIMEOUT_MS} (10 minutes)", tool.name));
            }
            match tool.class {
                Some(ToolClass::External) if p.network.is_empty() && p.sockets.is_empty() => {
                    return Err(format!("extension.toml: agent tool {:?} is external -- it needs network or sockets in its permissions", tool.name));
                }
                Some(ToolClass::Edit) if p.editor != Editor::Write && p.filesystem != Filesystem::ProjectWrite => {
                    return Err(format!("extension.toml: agent tool {:?} edits -- it needs editor = \"write\" or filesystem = \"project-write\" in its permissions", tool.name));
                }
                _ => {}
            }
        }
        if let Some(twice) = self.contributes.agent_tools.iter().enumerate().find(|(i, t)| self.contributes.agent_tools[..*i].iter().any(|o| o.name == t.name)) {
            return Err(format!("extension.toml: agent tool {:?} is there twice", twice.1.name));
        }
        for resource in &self.contributes.agent_resources {
            if resource.name.trim().is_empty() || resource.uri_template.trim().is_empty() || !resource.uri_template.contains("://") {
                return Err(format!("extension.toml: agent resource {:?} -- it has a name and a URI template (fenix://ext/jira/issue/{{key}})", resource.name));
            }
            if resource.uri_template.matches('{').count() != resource.uri_template.matches('}').count() || resource.parameters().iter().any(|n| !snake(n)) {
                return Err(format!("extension.toml: agent resource {:?} -- {:?}: each {{name}} is lower case letters, digits and _", resource.name, resource.uri_template));
            }
            if !self.contributes.services.contains(&resource.service) {
                return Err(format!("extension.toml: agent resource {:?} -- {:?} isn't one of its services", resource.name, resource.service));
            }
            if long(&resource.description) {
                return Err(format!("extension.toml: agent resource {:?} -- its description is over {MAX_TOOL_DESCRIPTION} characters", resource.name));
            }
        }
        for prompt in &self.contributes.agent_prompts {
            if !snake(&prompt.name) {
                return Err(format!("extension.toml: agent prompt {:?} -- lower case letters, digits and _", prompt.name));
            }
            if !self.contributes.services.contains(&prompt.service) {
                return Err(format!("extension.toml: agent prompt {:?} -- {:?} isn't one of its services", prompt.name, prompt.service));
            }
            if long(&prompt.description) {
                return Err(format!("extension.toml: agent prompt {:?} -- its description is over {MAX_TOOL_DESCRIPTION} characters", prompt.name));
            }
            if let Some(bad) = prompt.arguments.iter().find(|a| !snake(&a.name)) {
                return Err(format!("extension.toml: agent prompt {:?} -- argument {:?}: lower case letters, digits and _", prompt.name, bad.name));
            }
        }
        Ok(())
    }

    /// The name its agent tools, resources and prompts go by: its id's
    /// last part, with `-` as `_` (`jira` for `acme.jira`). A tool is
    /// `<toolset>_<name>`, and the toolset is the extension's.
    pub fn toolset(&self) -> String {
        toolset_of(&self.extension.id)
    }

    /// A bundle's members: what it depends on.
    pub fn members(&self) -> Vec<String> {
        if self.extension.runtime == Runtime::Bundle { self.dependencies.keys().cloned().collect() } else { Vec::new() }
    }

    pub fn version(&self) -> Result<semver::Version, String> {
        semver::Version::parse(&self.extension.version).map_err(|e| format!("extension.toml: version {:?}: {e}", self.extension.version))
    }

    /// Whether this Fenix (`fenix`, its own version) can install it: one
    /// it runs in, and code it can run. `Err` says why not.
    pub fn runs_in(&self, fenix: &semver::Version) -> Result<(), String> {
        let req = semver::VersionReq::parse(&self.engine.fenix).map_err(|e| e.to_string())?;
        if !req.matches(fenix) {
            return Err(format!("{} {} is for Fenix {}, and this is {fenix}", self.extension.name, self.extension.version, self.engine.fenix));
        }
        match self.extension.runtime {
            Runtime::None | Runtime::Bundle => Ok(()),
            Runtime::Wasm => {
                let api = self.engine.api.as_deref().unwrap_or_default();
                if api_fits(api) {
                    Ok(())
                } else {
                    Err(format!("{} {} was built for extension API {api}, and this Fenix offers {API}", self.extension.name, self.extension.version))
                }
            }
            Runtime::Lua => Err(format!("{} runs Lua code, which this Fenix can't yet", self.extension.name)),
        }
    }
}

/// The toolset of extension `id`'s agent tools: its last part, `-` as `_`.
pub fn toolset_of(id: &str) -> String {
    id.rsplit('.').next().unwrap_or(id).replace('-', "_")
}

/// `publisher.name`: two or more dot-separated parts of lowercase letters,
/// digits, `-` and `_`.
pub fn valid_id(id: &str) -> bool {
    let parts: Vec<&str> = id.split('.').collect();
    parts.len() >= 2 && parts.iter().all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_'))
}

#[cfg(test)]
pub(crate) fn sample(id: &str, version: &str, runtime: &str) -> String {
    format!(
        "[extension]\nid = \"{id}\"\nname = \"Sample\"\nversion = \"{version}\"\ndescription = \"A sample\"\nruntime = \"{runtime}\"\n\n[engine]\nfenix = \">=1.0, <2\"\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> semver::Version {
        semver::Version::parse(s).unwrap()
    }

    #[test]
    fn a_data_extension_for_this_fenix_installs() {
        let m = Manifest::parse(&sample("acme.themes", "1.2.0", "none")).unwrap();
        assert_eq!(m.extension.id, "acme.themes");
        assert_eq!(m.version().unwrap(), v("1.2.0"));
        assert!(m.runs_in(&v("1.0.0")).is_ok());
        assert!(m.runs_in(&v("0.9.0")).unwrap_err().contains("is for Fenix >=1.0, <2"));
        assert!(m.runs_in(&v("2.0.0")).is_err());
    }

    #[test]
    fn code_this_fenix_cant_run_is_refused_with_a_reason() {
        assert!(Manifest::parse(&sample("acme.jira", "1.0.0", "wasm")).unwrap_err().contains("engine.api"), "a wasm one says its API");
        let wasm = |api: &str| Manifest::parse(&sample("acme.jira", "1.0.0", "wasm").replace("fenix = ", &format!("api = \"{api}\"\nfenix = "))).unwrap();
        assert!(wasm("1.0").runs_in(&v("1.0.0")).is_ok());
        assert!(wasm("2.0").runs_in(&v("1.0.0")).unwrap_err().contains("built for extension API 2.0, and this Fenix offers 1.5.0"));
        assert!(Manifest::parse(&sample("acme.jira", "1.0.0", "lua")).unwrap().runs_in(&v("1.0.0")).unwrap_err().contains("Lua"));
    }

    /// A context: a name, and the files that put a folder in it.
    #[test]
    fn a_context_is_named_and_found_by_its_markers() {
        let with = |ctx: &str| Manifest::parse(&(sample("acme.ino", "1.0.0", "none") + &format!("\n[[contributes.contexts]]\n{ctx}\n")));
        let m = with("name = \"sketch\"\nmarkers = [\"sketch.yaml\", \"{folder}.ino\"]").unwrap();
        let ctx = &m.contributes.contexts[0];
        let dir = tempfile::tempdir().unwrap();
        let blink = dir.path().join("Blink");
        std::fs::create_dir_all(blink.join("src")).unwrap();
        std::fs::write(blink.join("src/util.cpp"), "").unwrap();
        assert_eq!(ctx.root_of(&blink.join("src/util.cpp")), None, "not yet a sketch");
        std::fs::write(blink.join("Blink.ino"), "").unwrap();
        assert_eq!(ctx.root_of(&blink.join("src/util.cpp")), Some(blink.clone()), "{{folder}}.ino");
        assert!(with("name = \"Sketch\"\nmarkers = [\"x\"]").unwrap_err().contains("lowercase"));
        assert!(with("name = \"s\"\nmarkers = []").unwrap_err().contains("one of those"));
        assert!(with("name = \"s\"\nlanguages = [\"mermaid\"]").is_ok(), "a language alone");
        assert_eq!(with("name = \"reader\"\npages = [\"pdf\"]").unwrap().contributes.contexts[0].pages, ["pdf"], "a kind of its pages alone");
        assert!(with("name = \"s\"\nmarkers = [\"../x\"]").unwrap_err().contains("file names"));
    }

    #[test]
    fn the_api_fits_by_its_major_and_any_minor_up_to_this_one() {
        assert!(api_fits("1") && api_fits("1.0") && api_fits("1.0.0") && api_fits("1.1") && api_fits("1.2") && api_fits("1.3") && api_fits("1.4") && api_fits("1.5"));
        assert!(!api_fits("1.5.1") && !api_fits("1.6"), "a later 1.x has what this one lacks");
        assert!(!api_fits("0.8") && !api_fits("0.8.8") && !api_fits("2.0") && !api_fits("x"));
    }

    /// `permissions.sockets`: protocols, hosts and ports, and the
    /// addresses a setting holds.
    #[test]
    fn a_socket_rule_names_protocols_hosts_and_ports() {
        let rule = SocketRule::parse("*:5900-5999").unwrap();
        assert!(rule.reaches(false, "10.0.0.5", 5901) && rule.reaches(true, "vm", 5900));
        assert!(!rule.reaches(false, "10.0.0.5", 6000) && !rule.binds(5900), "a remote's ports, not this machine's");
        let udp = SocketRule::parse("udp://:5000").unwrap();
        assert!(udp.binds(5000) && !udp.binds(5001) && !udp.reaches(true, "x", 5000));
        let tcp = SocketRule::parse("tcp://*.example.com:443").unwrap();
        assert!(tcp.reaches(false, "a.Example.com", 443) && !tcp.reaches(true, "a.example.com", 443) && !tcp.reaches(false, "example.com", 443));
        assert!(SocketRule::parse("[::1]:80").unwrap().reaches(false, "::1", 80));
        assert!(SocketRule::parse("host").is_err() && SocketRule::parse("host:x").is_err() && SocketRule::parse("a*b:1").is_err() && SocketRule::parse("h:9-1").is_err());
        // A records setting's rows, a list, a text with a scheme.
        let vnc = SocketRule::from_setting(r#"[["build-vm","10.0.0.5","5900"],["test-vm","vm.local","5901"]]"#);
        assert_eq!(vnc.len(), 2, "{vnc:?}");
        assert!(vnc[0].reaches(false, "10.0.0.5", 5900) && vnc[1].reaches(false, "vm.local", 5901) && !vnc[0].reaches(false, "build-vm", 5900));
        let sources = SocketRule::from_setting(r#"[["ops","tcp://ground:7000","","guess","0"],["sim","udp://:5000","","packets","0"]]"#);
        assert!(sources.iter().any(|r| r.reaches(false, "ground", 7000)) && sources.iter().any(|r| r.binds(5000)), "{sources:?}");
        assert!(SocketRule::from_setting("a:1, b:2").iter().any(|r| r.reaches(false, "b", 2)));
        assert!(SocketRule::from_setting("*:1-65535").is_empty(), "a setting doesn't grant everything");
        let checked = |sockets: &str| Manifest::parse(&(sample("acme.vnc", "1.0.0", "none") + &format!("\n[permissions]\nsockets = [{sockets}]\n")));
        assert!(checked("\"*:5900-5999\", \"${setting:vnc.hosts}\"").is_ok());
        assert!(checked("\"5900\"").unwrap_err().contains("permissions.sockets"));
    }

    #[test]
    fn a_glob_matches_names_and_paths() {
        assert!(glob_matches("*.dat", "pcf.DAT") && !glob_matches("*.dat", "pcf.dat.bak") && glob_matches("p?f.dat", "pcf.dat"));
        assert!(!glob_matches("*.dat", "mib/pcf.dat") && glob_matches("**/*.dat", "mib/x/pcf.dat") && glob_matches("mib/*.dat", "mib/pcf.dat"));
        let root = std::path::Path::new("/p");
        let files = vec!["*.pdf".to_string(), "mib/*.dat".to_string()];
        assert!(file_matches(&files, std::path::Path::new("/p/docs/a.PDF"), None));
        assert!(file_matches(&files, std::path::Path::new("/p/mib/pcf.dat"), Some(root)) && !file_matches(&files, std::path::Path::new("/p/other/pcf.dat"), Some(root)));
    }

    /// A code extension's contributions: servers by file pattern, files it opens, a
    /// forge, settings a project sets too.
    #[test]
    fn servers_by_pattern_openers_and_forges_are_read_and_checked() {
        let with = |extra: &str| Manifest::parse(&(sample("acme.mib", "1.0.0", "none") + "\n" + extra));
        let m = with("[contributes]\nopens = [\"*.pdf\"]\n\n[[contributes.language_servers]]\nname = \"mib\"\nfiles = [\"*.dat\"]\n\n[[contributes.forges]]\nname = \"GitHub\"\nhosts = [\"github.com\"]\n\n[[contributes.settings]]\nkey = \"mib.roots\"\nproject = true\n").unwrap();
        let server = &m.contributes.language_servers[0];
        assert!(server.serves(std::path::Path::new("/p/pcf.dat"), None, None) && !server.serves(std::path::Path::new("/p/a.tcl"), Some("tcl"), None));
        assert_eq!(m.contributes.opens, ["*.pdf"]);
        assert!(m.contributes.settings[0].project);
        let forge = &m.contributes.forges[0];
        assert_eq!(forge.claims("GitHub.com", &|_| None), 2);
        assert_eq!(forge.claims("gitlab.example.com", &|_| None), 0);
        let gitlab = ForgeContribution { name: "GitLab".into(), hosts: vec!["${setting:gitlab.url}".into(), "*".into()] };
        assert_eq!(gitlab.claims("git.acme.org", &|k| (k == "gitlab.url").then(|| "https://git.acme.org/".to_string())), 2);
        assert_eq!(gitlab.claims("elsewhere.org", &|_| None), 1);
        assert_eq!(url_host("git@github.com:o/r.git").as_deref(), Some("github.com"));
        assert_eq!(url_host("ssh://git@Host:22/o/r").as_deref(), Some("host"));
        assert!(with("[[contributes.language_servers]]\nname = \"x\"\n").unwrap_err().contains("one or the other"));
        assert!(with("[contributes]\nopens = [\"../x\"]\n").unwrap_err().contains("file pattern"));
        assert!(with("[[contributes.forges]]\nname = \"X\"\nhosts = []\n").unwrap_err().contains("forge"));
    }

    #[test]
    fn an_extension_brings_agents_by_its_own_name_and_hears_them_when_allowed() {
        let with = |extra: &str| Manifest::parse(&(sample("acme.cline", "1.0.0", "wasm").replace("fenix = ", "api = \"1.0\"\nfenix = ") + extra));
        let m = with("\n[[contributes.agents]]\nid = \"cline\"\nname = \"Cline\"\nlogin = \"cline auth\"\n\n[[contributes.agents]]\nid = \"cline.fast\"\nname = \"Cline (fast)\"\n").unwrap();
        assert_eq!(m.contributes.agents[0], AgentContribution { id: "cline".into(), name: "Cline".into(), login: Some("cline auth".into()) });
        assert_eq!(m.contributes.agents[1].id, "cline.fast");
        assert!(with("\n[[contributes.agents]]\nid = \"claude\"\nname = \"Not Claude\"\n").unwrap_err().contains("by its name"));
        assert!(with("\n[[contributes.agents]]\nid = \"other\"\nname = \"X\"\n").unwrap_err().contains("by its name"));
        assert!(with("\n[[contributes.agents]]\nid = \"Cline\"\nname = \"X\"\n").unwrap_err().contains("lowercase"));
        assert!(with("\n[[contributes.agents]]\nid = \"cline\"\nname = \" \"\n").unwrap_err().contains("no name"));
        // Without the permission it hears that a turn ended, not what was
        // said (Fenix empties the text and the title).
        assert!(with("\n[contributes]\nevents = [\"agents\"]\n").is_ok());
        assert!(with("\n[contributes]\nevents = [\"git\"]\n").is_ok());
        let m = with("\n[permissions]\nagents = true\n\n[contributes]\nevents = [\"agents\"]\n").unwrap();
        assert!(m.permissions.lines().iter().any(|l| l.contains("coding agents")));
    }

    /// What it waits for before loading: the languages and markers its
    /// `[activation]` names, and a pack's contexts' when it starts with
    /// Fenix; nothing for one that names none.
    #[test]
    fn what_it_waits_for_is_its_activations_and_a_packs_contexts() {
        let with = |extra: &str| Manifest::parse(&(sample("acme.ada", "1.0.0", "wasm").replace("fenix = ", "api = \"1.0\"\nfenix = ") + "\n" + extra)).unwrap();
        let pack = with("[activation]\non_startup = true\n\n[[contributes.contexts]]\nname = \"ada\"\nmarkers = [\"alire.toml\", \"{folder}.gpr\"]\nlanguages = [\"ada\", \"gpr\"]\n\n[[contributes.contexts]]\nname = \"pages\"\npages = [\"x\"]\n");
        assert_eq!(pack.activation.waits_for(&pack.contributes.contexts), (vec!["ada".to_string(), "gpr".into()], vec!["alire.toml".to_string(), "{folder}.gpr".into()]));
        let by_command = with("[activation]\ncommands = [\"acme.*\"]\n\n[[contributes.contexts]]\nname = \"ada\"\nlanguages = [\"ada\"]\n");
        assert_eq!(by_command.activation.waits_for(&by_command.contributes.contexts), (vec![], vec![]), "a context of one that doesn't start with Fenix is only its menu");
        let named = with("[activation]\nlanguages = [\"ada\"]\nmarkers = [\"alire.toml\"]\n");
        assert_eq!(named.activation.waits_for(&[]), (vec!["ada".to_string()], vec!["alire.toml".to_string()]));
        let eager = with("[activation]\non_startup = true\n\n[[contributes.contexts]]\nname = \"reader\"\npages = [\"pdf\"]\n");
        assert_eq!(eager.activation.waits_for(&eager.contributes.contexts), (vec![], vec![]), "pages alone: it starts with Fenix");
    }

    #[test]
    fn a_code_extension_says_when_it_loads_what_it_may_do_and_adds() {
        let text = sample("acme.jira", "1.0.0", "wasm").replace("fenix = ", "api = \"1.0\"\nfenix = ")
            + "\n[activation]\ncommands = [\"jira.*\"]\n\n[permissions]\nnetwork = [\"jira.example.com\"]\nsecrets = [\"jira.token\"]\nfilesystem = \"project-read\"\n\n[[contributes.commands]]\nid = \"jira.open\"\ntitle = \"Open Jira\"\nexternal = true\n";
        let m = Manifest::parse(&text).unwrap();
        assert_eq!(m.activation.commands, ["jira.*"]);
        assert_eq!(m.permissions.lines(), ["Reach over the network: jira.example.com", "Read and keep the secrets: jira.token", "Read the files of the open project"]);
        assert_eq!(m.contributes.commands, [CommandContribution { id: "jira.open".into(), title: "Open Jira".into(), external: true }]);
        assert!(Manifest::parse(&text.replace("network = ", "networks = ")).is_err(), "a permission misspelt isn't let through");
        // any host, and local sockets.
        let any = Manifest::parse(&text.replace("network = [\"jira.example.com\"]", "network = [\"*\"]\nipc = [\"discord-ipc-*\"]")).unwrap();
        assert_eq!(any.permissions.lines()[0], "Reach any host on the internet");
        assert!(any.permissions.lines().contains(&"Talk to the programs on this machine through: discord-ipc-*".to_string()));
        assert!(Manifest::parse(&text.replace("network = ", "ipc = [\"../x\"]\nnetwork = ")).unwrap_err().contains("permissions.ipc"));
        assert!(Manifest::parse(&text.replace("id = \"jira.open\"", "id = \"file.save\"")).unwrap_err().contains("start with its name"));
        let renamed = text.replace("id = \"jira.open\"", "id = \"issues.open\"");
        assert!(Manifest::parse(&renamed).is_err());
        assert!(Manifest::parse(&renamed.replace("[[contributes.commands]]", "[contributes]
namespaces = [\"issues\"]

[[contributes.commands]]")).is_ok(), "a name it was given too");
        let more = text.clone() + "\n[contributes]\nservices = [\"jira.issue\"]\n\n[[contributes.settings]]\nkey = \"jira.base_url\"\ndescription = \"Your server\"\n\n[[contributes.settings]]\nkey = \"jira.mode\"\nkind = \"choice\"\nchoices = [\"a\", \"b\"]\n";
        let m = Manifest::parse(&more).unwrap();
        assert_eq!(m.contributes.services, ["jira.issue"]);
        let tool = more.clone() + "\n[[contributes.agent_tools]]\nname = \"issue\"\ndescription = \"An issue\"\nservice = \"jira.issue\"\ninput = '{\"type\":\"object\"}'\n";
        let t = Manifest::parse(&tool).unwrap();
        assert_eq!(t.contributes.agent_tools[0].service, "jira.issue");
        assert!(!t.contributes.agent_tools[0].writes, "reading, unless it says");
        assert!(Manifest::parse(&tool.replace("service = \"jira.issue\"", "service = \"jira.other\"")).unwrap_err().contains("isn't one of its services"));
        assert!(Manifest::parse(&tool.replace("name = \"issue\"", "name = \"Is sue\"")).unwrap_err().contains("lower case"));
        assert!(Manifest::parse(&tool.replace("{\"type\":\"object\"}", "not json")).unwrap_err().contains("JSON object"));
        assert_eq!((m.contributes.settings[0].key.as_str(), m.contributes.settings[0].kind.as_str()), ("jira.base_url", "text"));
        assert!(Manifest::parse(&more.replace("jira.base_url", "editor.font")).unwrap_err().contains("settings start with its name"));
        assert!(Manifest::parse(&more.replace("kind = \"choice\"", "kind = \"colour\"")).unwrap_err().contains("one of text"));
        assert!(Manifest::parse(&more.replace("choices = [\"a\", \"b\"]", "choices = []")).unwrap_err().contains("a choice of nothing"));
        // A bool's or an int's default written bare, as fenix.java 1.1.0
        // did (it wouldn't install): read as its text.
        let bare = more.clone() + "\n[[contributes.settings]]\nkey = \"jira.on\"\nkind = \"bool\"\ndefault = true\n\n[[contributes.settings]]\nkey = \"jira.n\"\nkind = \"int\"\ndefault = 30\n";
        let m = Manifest::parse(&bare).unwrap();
        assert_eq!((m.contributes.settings[2].default.as_str(), m.contributes.settings[3].default.as_str()), ("true", "30"));
        assert!(Manifest::parse(&bare.replace("default = 30", "default = [30]")).is_err(), "a list isn't a default");
    }

    /// 1.2's tools: a class that fits its permissions, read from `writes`
    /// when it has none, a timeout, sizes held for one built for 1.2;
    /// resources and prompts read and checked.
    #[test]
    fn agent_tools_have_classes_timeouts_and_sizes() {
        let text = |api: &str, permissions: &str, tool: &str| {
            sample("acme.jira", "1.0.0", "wasm").replace("fenix = ", &format!("api = \"{api}\"\nfenix = "))
                + &format!("\n[permissions]\n{permissions}\n\n[contributes]\nservices = [\"jira.issue\"]\n\n[[contributes.agent_tools]]\nname = \"issue\"\ndescription = \"An issue\"\nservice = \"jira.issue\"\n{tool}\n")
        };
        let m = Manifest::parse(&text("1.2", "", "")).unwrap();
        let tool = &m.contributes.agent_tools[0];
        assert_eq!((tool.effective_class(), tool.timeout_ms(), tool.default), (ToolClass::Read, TOOL_TIMEOUT_MS, false));
        assert_eq!(m.toolset(), "jira");
        assert_eq!(toolset_of("acme.my-tools"), "my_tools");
        let writes = Manifest::parse(&text("1.0", "", "writes = true")).unwrap();
        assert_eq!(writes.contributes.agent_tools[0].effective_class(), ToolClass::Run, "writes, read the safe way");
        let classed = Manifest::parse(&text("1.2", "network = [\"jira.example.com\"]", "class = \"external\"\ntimeout_ms = 120000\ndefault = true")).unwrap();
        let tool = &classed.contributes.agent_tools[0];
        assert_eq!((tool.effective_class(), tool.timeout_ms(), tool.default, tool.effective_class().as_str()), (ToolClass::External, 120_000, true, "external"));
        assert!(Manifest::parse(&text("1.2", "", "class = \"external\"")).unwrap_err().contains("network or sockets"));
        assert!(Manifest::parse(&text("1.2", "sockets = [\"jira.example.com:443\"]", "class = \"external\"")).is_ok());
        assert!(Manifest::parse(&text("1.2", "editor = \"read\"", "class = \"edit\"")).unwrap_err().contains("editor = \"write\""));
        assert!(Manifest::parse(&text("1.2", "editor = \"write\"", "class = \"edit\"")).is_ok());
        assert!(Manifest::parse(&text("1.2", "filesystem = \"project-write\"", "class = \"edit\"")).is_ok());
        assert!(Manifest::parse(&text("1.2", "", "class = \"show\"")).is_ok());
        assert!(Manifest::parse(&text("1.2", "", "class = \"admin\"")).is_err(), "one of the five");
        assert!(Manifest::parse(&text("1.2", "", "timeout_ms = 600001")).unwrap_err().contains("timeout_ms"));
        assert!(Manifest::parse(&text("1.2", "", "timeout_ms = 0")).is_err());
        let tool_long = text("1.2", "", "").replace("An issue", &"x".repeat(301));
        assert!(Manifest::parse(&tool_long).unwrap_err().contains("over 300 characters"));
        assert!(Manifest::parse(&tool_long.replace("api = \"1.2\"", "api = \"1.1\"")).is_ok(), "one built before 1.2 still loads");
        let big = format!("input = '{{\"type\":\"object\",\"description\":\"{}\"}}'", "y".repeat(2100));
        assert!(Manifest::parse(&text("1.2", "", &big)).unwrap_err().contains("at most 2048"));
        let twice = text("1.2", "", "") + "\n[[contributes.agent_tools]]\nname = \"issue\"\ndescription = \"Again\"\nservice = \"jira.issue\"\n";
        assert!(Manifest::parse(&twice).unwrap_err().contains("twice"));

        let more = text("1.2", "", "") + "\n[[contributes.agent_resources]]\nuri_template = \"fenix://ext/jira/issue/{key}\"\nname = \"Issue\"\ndescription = \"A Jira issue\"\nservice = \"jira.issue\"\nmime_type = \"text/markdown\"\n\n[[contributes.agent_prompts]]\nname = \"triage\"\ndescription = \"Triage an issue\"\nservice = \"jira.issue\"\n\n[[contributes.agent_prompts.arguments]]\nname = \"key\"\ndescription = \"The issue\"\nrequired = true\n";
        let m = Manifest::parse(&more).unwrap();
        let resource = &m.contributes.agent_resources[0];
        assert_eq!((resource.parameters(), resource.mime_type.as_deref()), (vec!["key".to_string()], Some("text/markdown")));
        let prompt = &m.contributes.agent_prompts[0];
        assert_eq!((prompt.name.as_str(), prompt.arguments[0].name.as_str(), prompt.arguments[0].required), ("triage", "key", true));
        assert!(Manifest::parse(&more.replace("service = \"jira.issue\"\nmime_type", "service = \"jira.nope\"\nmime_type")).unwrap_err().contains("agent resource"));
        assert!(Manifest::parse(&more.replace("{key}", "{Key}")).unwrap_err().contains("each {name}"));
        assert!(Manifest::parse(&more.replace("fenix://ext/jira/issue/{key}", "issue")).unwrap_err().contains("URI template"));
        assert!(Manifest::parse(&more.replace("name = \"triage\"", "name = \"Triage\"")).unwrap_err().contains("agent prompt"));
        assert!(Manifest::parse(&more.replace("required = true", "required = true\nkind = 1")).is_err(), "an argument's field misspelt isn't let through");
    }

    /// `changed` and `focus` are events it may ask for -- `changed`
    /// only with the editor to read -- and `sound` a permission.
    #[test]
    fn events_and_the_sound_permission_are_read() {
        let text = |extra: &str| sample("acme.timer", "1.0.0", "wasm").replace("fenix = ", "api = \"1.0\"\nfenix = ") + extra;
        let m = Manifest::parse(&text("\n[permissions]\nsound = true\neditor = \"read\"\n\n[contributes]\nevents = [\"changed\", \"focus\"]\n")).unwrap();
        assert_eq!(m.contributes.events, ["changed", "focus"]);
        assert!(m.permissions.sound && m.permissions.lines().contains(&"Play sounds".to_string()));
        assert!(Manifest::parse(&text("\n[contributes]\nevents = [\"changed\"]\n")).unwrap_err().contains("editor = \"read\""));
        assert!(Manifest::parse(&text("\n[contributes]\nevents = [\"focus\"]\n")).is_ok(), "focus needs nothing");
        // `view`, the lines a pane shows -- its files' only with
        // `editor = "read"`, which the host holds it to.
        assert!(Manifest::parse(&text("\n[contributes]\nevents = [\"view\"]\n")).is_ok());
        // `debug`, a debug session's start, pauses and end.
        assert!(Manifest::parse(&text("\n[contributes]\nevents = [\"debug\", \"tasks\"]\n")).is_ok());
        assert!(m.runs_in(&v("1.0.0")).is_ok());
    }

    /// secrets by pattern, ports to listen on, environment
    /// variables, a longer call, and what a file it opens starts with.
    #[test]
    fn secrets_ports_environment_and_limits_are_read_and_checked() {
        let text = |extra: &str| sample("acme.http", "1.0.0", "wasm").replace("fenix = ", "api = \"1.0\"\nfenix = ") + extra;
        let m = Manifest::parse(&text("\n[permissions]\nsecrets = [\"http.secret.*\"]\nlisten = [\"127.0.0.1:0\", \"localhost:8400-8410\"]\nenv = [\"HOME\", \"JAVA_*\", \"${setting:http.env}\"]\n\n[limits]\ncall_ms = 30000\n")).unwrap();
        assert_eq!(m.limits.call_ms(), 30_000);
        assert_eq!(Manifest::parse(&text("")).unwrap().limits.call_ms(), CALL_MS);
        assert!(m.permissions.lines().contains(&"Listen on this machine's ports: 127.0.0.1:0, localhost:8400-8410".to_string()));
        assert!(m.permissions.lines().contains(&"Read Fenix's environment variables: HOME, JAVA_*, those your http.env setting names".to_string()), "{:?}", m.permissions.lines());
        assert!(secret_allowed(&m.permissions.secrets, "http.secret.api") && !secret_allowed(&m.permissions.secrets, "http.token") && !secret_allowed(&m.permissions.secrets, "jira.token"));
        assert!(secret_allowed(&["jira.token".to_string()], "jira.token") && !secret_allowed(&["jira.token".to_string()], "jira.tokens"));
        let rules: Vec<ListenRule> = m.permissions.listen.iter().map(|e| ListenRule::parse(e).unwrap()).collect();
        assert!(rules[0].takes(0) && !rules[0].takes(8400) && rules[1].takes(8405) && !rules[1].takes(0));
        assert!(ListenRule::parse("127.0.0.1:*").unwrap().takes(0));
        let setting = |key: &str| (key == "http.env").then(|| "[\"API_URL\", \"*\"]".to_string());
        let env = &m.permissions.env;
        assert!(env_allowed(env, "HOME", &setting) && env_allowed(env, "JAVA_HOME", &setting) && env_allowed(env, "API_URL", &setting));
        assert!(!env_allowed(env, "PATH", &setting) && !env_allowed(env, "JAVA", &setting), "* in a setting names nothing");
        assert!(env_allowed(&["${setting:x}".to_string()], "B", &|_| Some("A, B".into())));
        for (bad, why) in [
            ("listen = [\"0.0.0.0:80\"]", "loopback"),
            ("listen = [\"127.0.0.1:x\"]", "a port is"),
            ("env = [\"*\"]", "permissions.env"),
            ("env = [\"A=B\"]", "permissions.env"),
            ("secrets = [\"*\"]", "permissions.secrets"),
        ] {
            assert!(Manifest::parse(&text(&format!("\n[permissions]\n{bad}\n"))).unwrap_err().contains(why), "{bad}");
        }
        assert!(Manifest::parse(&text("\n[limits]\ncall_ms = 600000\n")).unwrap_err().contains("limits.call_ms"));
        assert!(Manifest::parse(&text("\n[limits]\ncall_ms = 0\n")).is_err());
        assert!(Manifest::parse(&text("\n[limits]\ncalls = 1\n")).is_err(), "a limit misspelt isn't let through");
        // What a file it opens starts with.
        let opens = Manifest::parse(&text("\n[contributes]\nopens = [\"*.db\", \"*.png\", \"*.pdf\"]\n\n[contributes.magic]\n\"*.db\" = \"SQLite format 3\"\n\"*.png\" = \"hex:89 50 4e 47\"\n")).unwrap();
        let head = |bytes: &'static [u8]| move |n: usize| bytes[..n.min(bytes.len())].to_vec();
        let c = &opens.contributes;
        assert!(c.opens_file(std::path::Path::new("/x/a.db"), &head(b"SQLite format 3\0...")));
        assert!(!c.opens_file(std::path::Path::new("/x/a.db"), &head(b"DUCK")), "not SQLite: as text");
        assert!(c.opens_file(std::path::Path::new("/x/a.png"), &head(b"\x89PNG\r\n")));
        assert!(c.opens_file(std::path::Path::new("/x/a.pdf"), &head(b"")), "no magic: by name alone");
        assert!(!c.opens_file(std::path::Path::new("/x/a.txt"), &head(b"SQLite format 3")));
        assert!(Manifest::parse(&text("\n[contributes]\nopens = [\"*.db\"]\n\n[contributes.magic]\n\"*.sqlite\" = \"x\"\n")).unwrap_err().contains("isn't one of the files it opens"));
        assert!(Manifest::parse(&text("\n[contributes]\nopens = [\"*.db\"]\n\n[contributes.magic]\n\"*.db\" = \"hex:8\"\n")).unwrap_err().contains("pairs of hex digits"));
    }

    /// A bundle is its members, as dependencies, and runs nothing.
    #[test]
    fn a_bundle_is_its_members() {
        let bundle = sample("acme.lab", "1.0.0", "bundle");
        assert!(Manifest::parse(&bundle).unwrap_err().contains("it has none"));
        let members = bundle + "\n[dependencies]\n\"acme.vnc\" = \"^2\"\n\"acme.arduino\" = \"^2\"\n";
        let m = Manifest::parse(&members).unwrap();
        assert_eq!(m.members(), ["acme.arduino", "acme.vnc"]);
        assert!(m.runs_in(&v("1.0.0")).is_ok());
        assert!(Manifest::parse(&(members.clone() + "\n[permissions]\nnetwork = [\"x\"]\n")).unwrap_err().contains("runs nothing"));
        assert!(Manifest::parse(&(members + "\n[activation]\non_startup = true\n")).unwrap_err().contains("runs nothing"));
        assert!(Manifest::parse(&sample("acme.dusk", "1.0.0", "none")).unwrap().members().is_empty(), "not a bundle");
    }

    #[test]
    fn a_later_fenixs_sections_are_read_past() {
        let text = sample("acme.jira", "1.0.0", "none") + "\n[activation]\ncommands = [\"jira.*\"]\n\n[[contributes.keybindings]]\nkeys = \"SPC j j\"\n";
        assert!(Manifest::parse(&text).is_ok());
    }

    #[test]
    fn whats_wrong_with_a_manifest_is_said() {
        assert!(Manifest::parse(&sample("Acme.Themes", "1.0.0", "none")).unwrap_err().contains("publisher.name"));
        assert!(Manifest::parse(&sample("themes", "1.0.0", "none")).is_err(), "one part isn't an id");
        assert!(Manifest::parse(&sample("acme.themes", "1.0", "none")).unwrap_err().contains("version"));
        assert!(Manifest::parse(&sample("acme.themes", "1.0.0", "python")).is_err());
        let bad_range = sample("acme.themes", "1.0.0", "none").replace(">=1.0, <2", "two");
        assert!(Manifest::parse(&bad_range).unwrap_err().contains("engine.fenix"));
        let dep = sample("acme.themes", "1.0.0", "none") + "\n[dependencies]\n\"acme.base\" = \"^1\"\n";
        assert_eq!(Manifest::parse(&dep).unwrap().dependencies["acme.base"], "^1");
    }
}
