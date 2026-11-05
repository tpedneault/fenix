//! The ACP registry (`https://cdn.agentclientprotocol.com/registry/v1/
//! latest/registry.json`): agents an editor can install, each with how
//! to get it on each platform -- an archive to download and unpack, or a
//! package `npx` or `uvx` runs.
//!
//! Fenix installs into its data folder (`<data>/agents/<id>/<version>`),
//! keeps what it installed in `<data>/agents/installed.json`, and the
//! catalog offers those beside the built-ins. An archive is refused when
//! its SHA-256 isn't the registry's; one the registry gives no SHA-256
//! for is installed, and said to be unchecked.
//!
//! Fetching is the caller's (`fetch`), so this reads and decides as plain
//! data, and installs into any folder a test gives it.

use crate::hub::AgentSpec;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

pub const DEFAULT_URL: &str = "https://cdn.agentclientprotocol.com/registry/v1/latest/registry.json";

/// The most an archive may be.
pub const MAX_ARCHIVE: u64 = 512 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq)]
pub struct RegistryAgent {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub license: Option<String>,
    pub website: Option<String>,
    pub binary: BTreeMap<String, Binary>,
    pub npx: Option<Package>,
    pub uvx: Option<Package>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Binary {
    pub archive: String,
    pub sha256: Option<String>,
    /// The program, inside the unpacked archive.
    pub cmd: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Package {
    /// `name@version`.
    pub package: String,
    pub args: Vec<String>,
}

fn strings(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array).map(|a| a.iter().filter_map(|s| s.as_str().map(str::to_string)).collect()).unwrap_or_default()
}

fn text(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(str::to_string)
}

/// The registry's agents. Entries it can't read are left out.
pub fn parse(body: &[u8]) -> Result<Vec<RegistryAgent>, String> {
    let v: Value = serde_json::from_slice(body).map_err(|e| format!("the registry isn't JSON: {e}"))?;
    let agents = v.get("agents").and_then(Value::as_array).ok_or("the registry lists no agents")?;
    Ok(agents
        .iter()
        .filter_map(|a| {
            let dist = a.get("distribution")?;
            let package = |key: &str| dist.get(key).and_then(|p| Some(Package { package: text(p, "package")?, args: strings(p.get("args")) }));
            let binary = dist
                .get("binary")
                .and_then(Value::as_object)
                .map(|m| {
                    m.iter()
                        .filter_map(|(platform, b)| {
                            Some((
                                platform.clone(),
                                Binary {
                                    archive: text(b, "archive")?,
                                    sha256: text(b, "sha256").map(|s| s.to_lowercase()),
                                    cmd: text(b, "cmd")?,
                                    args: strings(b.get("args")),
                                    env: b.get("env").and_then(Value::as_object).map(|e| e.iter().filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string()))).collect()).unwrap_or_default(),
                                },
                            ))
                        })
                        .collect()
                })
                .unwrap_or_default();
            Some(RegistryAgent {
                id: text(a, "id")?,
                name: text(a, "name").unwrap_or_default(),
                version: text(a, "version").unwrap_or_default(),
                description: text(a, "description").unwrap_or_default(),
                license: text(a, "license"),
                website: text(a, "website").or_else(|| text(a, "repository")),
                binary,
                npx: package("npx"),
                uvx: package("uvx"),
            })
        })
        .collect())
}

/// The registry's name for this machine: `linux-x86_64`, `windows-aarch64`, …
pub fn platform() -> String {
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        other => other,
    };
    format!("{os}-{}", std::env::consts::ARCH)
}

/// How an agent would be installed here.
#[derive(Clone, Debug, PartialEq)]
pub enum Plan {
    Binary(Binary),
    Npx(Package),
    Uvx(Package),
}

/// The way to install `agent` on `platform`: its archive when there's
/// one, else `npx` or `uvx` when `have` finds them.
pub fn plan(agent: &RegistryAgent, platform: &str, have: &dyn Fn(&str) -> bool) -> Option<Plan> {
    if let Some(b) = agent.binary.get(platform) {
        return Some(Plan::Binary(b.clone()));
    }
    if let Some(p) = agent.npx.as_ref().filter(|_| have("npx")) {
        return Some(Plan::Npx(p.clone()));
    }
    agent.uvx.as_ref().filter(|_| have("uvx")).map(|p| Plan::Uvx(p.clone()))
}

/// An agent Fenix installed from the registry.
#[derive(Clone, Debug, PartialEq)]
pub struct Installed {
    pub id: String,
    pub name: String,
    pub version: String,
    pub program: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    /// Installed without a SHA-256 to check it against.
    pub unchecked: bool,
}

impl Installed {
    pub fn spec(&self) -> AgentSpec {
        AgentSpec { id: self.id.clone(), name: self.name.clone(), program: self.program.clone(), args: self.args.clone(), env: self.env.clone() }
    }
}

/// Gets a URL's bytes, at most the limit given.
pub type Download<'a> = dyn Fn(&str, u64) -> Result<Vec<u8>, String> + 'a;

/// Installs `agent` as `plan` says, under `dir` (`<data>/agents`).
pub fn install(agent: &RegistryAgent, plan: &Plan, dir: &Path, fetch: &Download<'_>) -> Result<Installed, String> {
    let base = |program: String, args: Vec<String>, env: Vec<(String, String)>, unchecked: bool| Installed { id: agent.id.clone(), name: agent.name.clone(), version: agent.version.clone(), program, args, env, unchecked };
    let installed = match plan {
        Plan::Npx(p) => base("npx".into(), [vec!["-y".to_string(), p.package.clone()], p.args.clone()].concat(), vec![], false),
        Plan::Uvx(p) => base("uvx".into(), [vec![p.package.clone()], p.args.clone()].concat(), vec![], false),
        Plan::Binary(b) => {
            let bytes = fetch(&b.archive, MAX_ARCHIVE)?;
            if let Some(want) = &b.sha256 {
                let got = hex(&Sha256::digest(&bytes));
                if &got != want {
                    return Err(format!("{}'s download isn't what the registry says (SHA-256 {got}, not {want}): not installed", agent.name));
                }
            }
            let target = dir.join(safe(&agent.id)).join(safe(&agent.version));
            let _ = std::fs::remove_dir_all(&target);
            std::fs::create_dir_all(&target).map_err(|e| format!("{}: {e}", target.display()))?;
            unpack(&b.archive, &bytes, &target).map_err(|e| format!("couldn't unpack {}: {e}", b.archive))?;
            let program = target.join(b.cmd.trim_start_matches("./"));
            if !program.is_file() {
                return Err(format!("{} has no {} in it", b.archive, b.cmd));
            }
            make_executable(&program);
            base(program.to_string_lossy().into_owned(), b.args.clone(), b.env.clone(), b.sha256.is_none())
        }
    };
    let mut list = load(dir);
    list.retain(|i| i.id != installed.id);
    list.push(installed.clone());
    save(dir, &list).map_err(|e| format!("couldn't keep the list of installed agents: {e}"))?;
    Ok(installed)
}

/// Removes an installed agent: its files, and its line.
pub fn uninstall(dir: &Path, id: &str) -> io::Result<()> {
    let _ = std::fs::remove_dir_all(dir.join(safe(id)));
    let list: Vec<Installed> = load(dir).into_iter().filter(|i| i.id != id).collect();
    save(dir, &list)
}

/// The agents Fenix installed.
pub fn load(dir: &Path) -> Vec<Installed> {
    let Ok(bytes) = std::fs::read(dir.join("installed.json")) else { return Vec::new() };
    let Ok(v) = serde_json::from_slice::<Value>(&bytes) else { return Vec::new() };
    v.get("agents")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|i| {
                    Some(Installed {
                        id: text(i, "id")?,
                        name: text(i, "name").unwrap_or_default(),
                        version: text(i, "version").unwrap_or_default(),
                        program: text(i, "program")?,
                        args: strings(i.get("args")),
                        env: i.get("env").and_then(Value::as_object).map(|e| e.iter().filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string()))).collect()).unwrap_or_default(),
                        unchecked: i.get("unchecked").and_then(Value::as_bool).unwrap_or(false),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn save(dir: &Path, list: &[Installed]) -> io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let agents: Vec<Value> = list
        .iter()
        .map(|i| json!({ "id": i.id, "name": i.name, "version": i.version, "program": i.program, "args": i.args, "env": i.env.iter().cloned().collect::<BTreeMap<_, _>>(), "unchecked": i.unchecked }))
        .collect();
    fenix_storage::write(&dir.join("installed.json"), &serde_json::to_vec_pretty(&json!({ "version": 1, "agents": agents }))?)
}

fn safe(name: &str) -> String {
    name.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') { c } else { '_' }).collect::<String>().trim_matches('.').to_string()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Unpacks a `.zip` here, and a `.tar.gz`/`.tgz`/`.tar.xz`/`.tar` with
/// the system's `tar` (every Linux and macOS, and Windows 10 and later,
/// have one).
fn unpack(name: &str, bytes: &[u8], target: &Path) -> Result<(), String> {
    let lower = name.to_lowercase();
    if lower.ends_with(".zip") {
        let mut archive = zip::ZipArchive::new(io::Cursor::new(bytes)).map_err(|e| e.to_string())?;
        // Every entry stays inside the target (`enclosed_name`).
        return archive.extract(target).map_err(|e| e.to_string());
    }
    let file = target.join(".download");
    std::fs::write(&file, bytes).map_err(|e| e.to_string())?;
    let flags = if lower.ends_with(".tar.xz") { "-xJf" } else if lower.ends_with(".tar") { "-xf" } else { "-xzf" };
    let status = std::process::Command::new("tar").arg(flags).arg(&file).arg("-C").arg(target).status().map_err(|e| format!("tar: {e}"))?;
    let _ = std::fs::remove_file(&file);
    if !status.success() {
        return Err(format!("tar ended with {status}"));
    }
    Ok(())
}

#[cfg(unix)]
fn make_executable(path: &PathBuf) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = std::fs::metadata(path) {
        let mut p = meta.permissions();
        p.set_mode(p.mode() | 0o755);
        let _ = std::fs::set_permissions(path, p);
    }
}

#[cfg(not(unix))]
fn make_executable(_: &PathBuf) {}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry() -> Vec<u8> {
        serde_json::to_vec(&json!({
            "version": "1",
            "agents": [
                { "id": "devin", "name": "Devin", "version": "3000.11.228", "description": "Cognition's agent", "license": "proprietary",
                  "distribution": { "binary": { "linux-x86_64": { "archive": "https://static.devin.ai/cli/3000.11.228/devin-linux.zip", "sha256": "AB", "cmd": "./bin/devin", "args": ["acp"] } } } },
                { "id": "codex-acp", "name": "Codex", "version": "2.5.0", "distribution": { "npx": { "package": "@agentclientprotocol/codex-acp@2.5.0" } } },
                { "id": "broken" },
                { "id": "py", "name": "Py", "version": "1", "distribution": { "uvx": { "package": "py-agent", "args": ["--acp"] } } }
            ]
        }))
        .unwrap()
    }

    fn zip_with(path: &str, body: &[u8]) -> Vec<u8> {
        let mut out = io::Cursor::new(Vec::new());
        let mut z = zip::ZipWriter::new(&mut out);
        z.start_file(path, zip::write::SimpleFileOptions::default()).unwrap();
        io::Write::write_all(&mut z, body).unwrap();
        z.finish().unwrap();
        out.into_inner()
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("fenix-acp-registry-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn the_registry_reads_and_each_agent_gets_a_plan_for_this_machine() {
        let agents = parse(&registry()).unwrap();
        assert_eq!(agents.len(), 3, "the broken entry is left out");
        let have_node = |p: &str| p == "npx";
        assert!(matches!(plan(&agents[0], "linux-x86_64", &have_node), Some(Plan::Binary(b)) if b.cmd == "./bin/devin" && b.sha256.as_deref() == Some("ab")));
        assert_eq!(plan(&agents[0], "windows-x86_64", &have_node), None, "no archive for Windows, no package");
        assert!(matches!(plan(&agents[1], "windows-x86_64", &have_node), Some(Plan::Npx(_))));
        assert_eq!(plan(&agents[1], "linux-x86_64", &|_| false), None, "npx isn't there");
        assert!(matches!(plan(&agents[2], "linux-x86_64", &|p| p == "uvx"), Some(Plan::Uvx(_))));
        assert!(platform().contains('-'));
    }

    #[test]
    fn an_archive_is_checked_unpacked_and_listed() {
        let dir = temp("install");
        let archive = zip_with("bin/devin", b"#!/bin/sh\necho hi\n");
        let mut agent = parse(&registry()).unwrap().remove(0);
        let mut b = agent.binary.remove("linux-x86_64").unwrap();
        b.sha256 = Some(hex(&Sha256::digest(&archive)));
        let fetch = |_: &str, _: u64| Ok(archive.clone());
        let installed = install(&agent, &Plan::Binary(b.clone()), &dir, &fetch).unwrap();
        assert!(Path::new(&installed.program).is_file());
        assert_eq!(installed.args, vec!["acp"]);
        assert!(!installed.unchecked);
        assert_eq!(load(&dir), vec![installed.clone()]);
        assert_eq!(installed.spec().id, "devin");
        // A download that isn't what the registry says is refused.
        b.sha256 = Some("00".repeat(32));
        let refused = install(&agent, &Plan::Binary(b), &dir, &fetch).unwrap_err();
        assert!(refused.contains("SHA-256"), "{refused}");
        uninstall(&dir, "devin").unwrap();
        assert!(load(&dir).is_empty() && !dir.join("devin").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_package_is_a_command_line_and_nothing_to_download() {
        let dir = temp("npx");
        let agents = parse(&registry()).unwrap();
        let installed = install(&agents[1], &Plan::Npx(agents[1].npx.clone().unwrap()), &dir, &|_, _| Err("no download".into())).unwrap();
        assert_eq!((installed.program.as_str(), installed.args.clone()), ("npx", vec!["-y".to_string(), "@agentclientprotocol/codex-acp@2.5.0".to_string()]));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
