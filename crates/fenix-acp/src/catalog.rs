//! The agents Fenix knows how to start: its built-in profiles, the ones
//! you set in `agents.servers`, the ones installed from the ACP registry,
//! and the ones extensions bring. An agent is data -- a command line, its environment, how
//! it's named -- never code: nothing elsewhere in Fenix asks which agent
//! it is talking to.

use crate::hub::AgentSpec;
use std::path::{Path, PathBuf};

/// A built-in profile: the programs that may run it, in order of
/// preference -- the agent's own binary first, `npx` as a fallback.
pub struct Builtin {
    pub id: &'static str,
    pub name: &'static str,
    pub candidates: &'static [(&'static str, &'static [&'static str])],
    /// How to log in, for the agents page.
    pub login: &'static str,
}

pub const BUILTINS: &[Builtin] = &[
    Builtin {
        id: "claude",
        name: "Claude Code",
        candidates: &[("claude-agent-acp", &[]), ("npx", &["-y", "@agentclientprotocol/claude-agent-acp"])],
        login: "your Claude login, or ANTHROPIC_API_KEY",
    },
    Builtin {
        id: "codex",
        name: "Codex",
        candidates: &[("codex-acp", &[]), ("npx", &["-y", "@agentclientprotocol/codex-acp"])],
        login: "your ChatGPT login, or OPENAI_API_KEY / CODEX_API_KEY",
    },
    Builtin { id: "devin", name: "Devin", candidates: &[("devin", &["acp"])], login: "`devin auth login`, or WINDSURF_API_KEY" },
    Builtin { id: "gemini", name: "Gemini CLI", candidates: &[("gemini", &["--acp"]), ("npx", &["-y", "@google/gemini-cli", "--acp"])], login: "your Google login" },
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Builtin,
    /// `agents.servers`.
    Configured,
    /// Installed from the ACP registry (`<data>/agents`).
    Registry,
    /// Brought by an extension (`contributes.agents`).
    Extension,
}

/// An agent an extension brings: its spec, with no program until the
/// extension gives one (`agent-provide`), and how to log in.
#[derive(Clone, Debug, PartialEq)]
pub struct Brought {
    pub spec: AgentSpec,
    pub login: Option<String>,
}

/// An agent in the catalog, found or not.
#[derive(Clone, Debug, PartialEq)]
pub struct CatalogEntry {
    pub spec: AgentSpec,
    pub source: Source,
    /// Its program is there (on `PATH`, or a path that exists). An entry
    /// not found is listed, not offered.
    pub found: bool,
    /// For a built-in: how to log in.
    pub login: Option<String>,
}

/// The catalog: built-ins (each with the first program found), then those
/// installed from the registry, then those extensions bring, then your own
/// (`configured`: name -> command line, replacing any of the others by
/// id). `find` looks a program up (on `PATH`; a path is taken as it is).
/// `env` goes to every agent.
pub fn catalog(configured: &[(String, String)], installed: &[AgentSpec], brought: &[Brought], env: &[(String, String)], find: &dyn Fn(&str) -> Option<PathBuf>) -> Vec<CatalogEntry> {
    let mut entries: Vec<CatalogEntry> = Vec::new();
    for b in BUILTINS {
        let found = b.candidates.iter().find(|(program, _)| find(program).is_some());
        let (program, args) = found.copied().unwrap_or(b.candidates[0]);
        entries.push(CatalogEntry {
            spec: AgentSpec { id: b.id.to_string(), name: b.name.to_string(), program: program.to_string(), args: args.iter().map(|a| a.to_string()).collect(), env: env.to_vec() },
            source: Source::Builtin,
            found: found.is_some(),
            login: Some(b.login.to_string()),
        });
    }
    for spec in installed {
        let entry = CatalogEntry { spec: AgentSpec { env: [env, &spec.env].concat(), ..spec.clone() }, source: Source::Registry, found: Path::new(&spec.program).is_file() || find(&spec.program).is_some(), login: None };
        put(&mut entries, entry);
    }
    for b in brought {
        let program = &b.spec.program;
        let found = !program.is_empty() && (Path::new(program).is_file() || find(program).is_some());
        let entry = CatalogEntry { spec: AgentSpec { env: [env, &b.spec.env].concat(), ..b.spec.clone() }, source: Source::Extension, found, login: b.login.clone() };
        put(&mut entries, entry);
    }
    for (name, line) in configured {
        let words = split_command_line(line);
        let Some((program, args)) = words.split_first() else { continue };
        let id = slug(name);
        let builtin_name = BUILTINS.iter().find(|b| b.id == id).map(|b| b.name.to_string());
        let entry = CatalogEntry {
            spec: AgentSpec { id: id.clone(), name: builtin_name.unwrap_or_else(|| name.clone()), program: program.clone(), args: args.to_vec(), env: env.to_vec() },
            source: Source::Configured,
            found: find(program).is_some(),
            login: BUILTINS.iter().find(|b| b.id == id).map(|b| b.login.to_string()),
        };
        put(&mut entries, entry);
    }
    entries
}

fn put(entries: &mut Vec<CatalogEntry>, entry: CatalogEntry) {
    match entries.iter_mut().find(|e| e.spec.id == entry.spec.id) {
        Some(e) => *e = entry,
        None => entries.push(entry),
    }
}

/// A setting's name made an id: lowercase, spaces as dashes.
pub fn slug(name: &str) -> String {
    name.trim().to_lowercase().chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '-' }).collect()
}

/// Splits a command line as a shell would for plain cases: on
/// whitespace, with `"…"` and `'…'` quoting, and `\"` for a quote inside
/// double quotes. Any other backslash stays as it is, so a Windows path
/// (`C:\tools\agent.exe`, `\\server\share`, `\\?\C:\…`) reads as written.
/// No variables, no globs.
pub fn split_command_line(line: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                in_word = true;
                while let Some(c) = chars.next() {
                    match c {
                        '"' => break,
                        '\\' if chars.peek() == Some(&'"') => word.push(chars.next().unwrap()),
                        c => word.push(c),
                    }
                }
            }
            '\'' => {
                in_word = true;
                for c in chars.by_ref() {
                    if c == '\'' {
                        break;
                    }
                    word.push(c);
                }
            }
            c if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut word));
                    in_word = false;
                }
            }
            c => {
                in_word = true;
                word.push(c);
            }
        }
    }
    if in_word {
        words.push(word);
    }
    words
}

/// Looks a program up the way Fenix starts it: a path as it is, else on
/// `PATH` (with `PATHEXT` on Windows).
pub fn find_on_path(program: &str) -> Option<PathBuf> {
    let path = Path::new(program);
    if path.components().count() > 1 {
        return path.is_file().then(|| path.to_path_buf());
    }
    let resolved = fenix_rpc::resolve_command(program);
    if resolved != program {
        return Some(PathBuf::from(resolved));
    }
    let dirs = std::env::var_os("PATH")?;
    std::env::split_paths(&dirs).map(|d| d.join(program)).find(|p| p.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn finder(present: &'static [&'static str]) -> impl Fn(&str) -> Option<PathBuf> {
        move |p: &str| present.contains(&p).then(|| PathBuf::from(format!("/usr/bin/{p}")))
    }

    #[test]
    fn a_builtin_takes_its_own_binary_first_and_npx_after() {
        let entries = catalog(&[], &[], &[], &[], &finder(&["npx", "devin"]));
        let get = |id: &str| entries.iter().find(|e| e.spec.id == id).unwrap();
        assert_eq!(get("claude").spec.program, "npx");
        assert_eq!(get("claude").spec.args, vec!["-y", "@agentclientprotocol/claude-agent-acp"]);
        assert!(get("claude").found);
        assert_eq!((get("devin").spec.program.as_str(), get("devin").spec.args.as_slice()), ("devin", &["acp".to_string()][..]));
        let entries = catalog(&[], &[], &[], &[], &finder(&["claude-agent-acp", "npx"]));
        assert_eq!(entries[0].spec.program, "claude-agent-acp");
        let entries = catalog(&[], &[], &[], &[], &finder(&[]));
        assert!(entries.iter().all(|e| !e.found), "listed, not offered");
    }

    #[test]
    fn your_servers_replace_a_builtin_by_name_or_add_one() {
        let configured = vec![("devin".to_string(), "/opt/devin/bin/devin acp --permission-mode normal".to_string()), ("In House".to_string(), "\"/opt/our agent/run\" --acp".to_string())];
        let entries = catalog(&configured, &[], &[], &[("HTTPS_PROXY".into(), "http://proxy:3128".into())], &finder(&["/opt/devin/bin/devin"]));
        let devin = entries.iter().find(|e| e.spec.id == "devin").unwrap();
        assert_eq!(devin.source, Source::Configured);
        assert_eq!(devin.spec.name, "Devin");
        assert_eq!(devin.spec.args, vec!["acp", "--permission-mode", "normal"]);
        assert!(devin.found);
        let ours = entries.iter().find(|e| e.spec.id == "in-house").unwrap();
        assert_eq!(ours.spec.program, "/opt/our agent/run");
        assert_eq!(ours.spec.env, vec![("HTTPS_PROXY".to_string(), "http://proxy:3128".to_string())]);
    }

    #[test]
    fn an_extensions_agent_is_offered_once_it_says_how_to_start_it() {
        let spec = |program: &str| AgentSpec { id: "cline".into(), name: "Cline".into(), program: program.into(), args: vec!["--acp".into()], env: vec![("A".into(), "1".into())] };
        let given = Brought { spec: spec("cline"), login: Some("cline auth".into()) };
        let entries = catalog(&[], &[], &[given], &[("B".into(), "2".into())], &finder(&["cline"]));
        let cline = entries.iter().find(|e| e.spec.id == "cline").unwrap();
        assert_eq!((cline.source, cline.found, cline.login.as_deref()), (Source::Extension, true, Some("cline auth")));
        assert_eq!(cline.spec.env, vec![("B".to_string(), "2".to_string()), ("A".to_string(), "1".to_string())]);
        let entries = catalog(&[], &[], &[Brought { spec: spec(""), login: None }], &[], &finder(&["cline"]));
        assert!(!entries.iter().find(|e| e.spec.id == "cline").unwrap().found, "listed before the extension runs, not offered");
        let mine = vec![("cline".to_string(), "/opt/cline --acp".to_string())];
        let entries = catalog(&mine, &[], &[Brought { spec: spec("cline"), login: None }], &[], &finder(&[]));
        assert_eq!(entries.iter().find(|e| e.spec.id == "cline").unwrap().source, Source::Configured, "yours wins");
    }

    #[test]
    fn command_lines_split_with_quotes() {
        assert_eq!(split_command_line(r#"a "b c" 'd e' f\g "h\"i""#), vec!["a", "b c", "d e", "f\\g", "h\"i"]);
        assert_eq!(split_command_line("  "), Vec::<String>::new());
        assert_eq!(split_command_line(r#"x """#), vec!["x", ""]);
        assert_eq!(split_command_line(r#""\\?\C:\a b\run.exe" --store "\\server\share""#), vec![r"\\?\C:\a b\run.exe", "--store", r"\\server\share"]);
    }
}
