//! `extension.toml`: who an extension is, which Fenix it runs in, what it
//! needs. Sections a later Fenix reads (`activation`, `permissions`,
//! `contributes`) are let through here, so a package made for that Fenix
//! still says plainly why it can't be installed in this one.

use std::collections::BTreeMap;

use serde::Deserialize;

/// What runs an extension: nothing (data only), or its own code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Runtime {
    None,
    Wasm,
    Lua,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Manifest {
    pub extension: Info,
    pub engine: Engine,
    /// Other extensions it needs, by id, with the versions it takes.
    #[serde(default)]
    pub dependencies: BTreeMap<String, String>,
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
    /// The Fenix releases it runs in, as a version range (`>=2.0, <3`).
    pub fenix: String,
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
        for (dep, range) in &self.dependencies {
            if !valid_id(dep) {
                return Err(format!("extension.toml: dependency {dep:?} isn't an extension id"));
            }
            semver::VersionReq::parse(range).map_err(|e| format!("extension.toml: dependency {dep} {range:?}: {e}"))?;
        }
        Ok(())
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
            Runtime::None => Ok(()),
            runtime => Err(format!(
                "{} runs {} code, which this Fenix can't yet -- only extensions of data (themes, snippets, templates) install for now",
                self.extension.name,
                if runtime == Runtime::Wasm { "WebAssembly" } else { "Lua" }
            )),
        }
    }
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
        "[extension]\nid = \"{id}\"\nname = \"Sample\"\nversion = \"{version}\"\ndescription = \"A sample\"\nruntime = \"{runtime}\"\n\n[engine]\nfenix = \">=2.0, <3\"\n"
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
        assert!(m.runs_in(&v("2.0.0")).is_ok());
        assert!(m.runs_in(&v("1.9.0")).unwrap_err().contains("is for Fenix >=2.0, <3"));
        assert!(m.runs_in(&v("3.0.0")).is_err());
    }

    #[test]
    fn code_this_fenix_cant_run_is_refused_with_a_reason() {
        let m = Manifest::parse(&sample("acme.jira", "1.0.0", "wasm")).unwrap();
        assert!(m.runs_in(&v("2.0.0")).unwrap_err().contains("WebAssembly"));
    }

    #[test]
    fn a_later_fenixs_sections_are_read_past() {
        let text = sample("acme.jira", "1.0.0", "none") + "\n[activation]\ncommands = [\"jira.*\"]\n\n[[contributes.commands]]\nid = \"jira.open\"\n";
        assert!(Manifest::parse(&text).is_ok());
    }

    #[test]
    fn whats_wrong_with_a_manifest_is_said() {
        assert!(Manifest::parse(&sample("Acme.Themes", "1.0.0", "none")).unwrap_err().contains("publisher.name"));
        assert!(Manifest::parse(&sample("themes", "1.0.0", "none")).is_err(), "one part isn't an id");
        assert!(Manifest::parse(&sample("acme.themes", "1.0", "none")).unwrap_err().contains("version"));
        assert!(Manifest::parse(&sample("acme.themes", "1.0.0", "python")).is_err());
        let bad_range = sample("acme.themes", "1.0.0", "none").replace(">=2.0, <3", "two");
        assert!(Manifest::parse(&bad_range).unwrap_err().contains("engine.fenix"));
        let dep = sample("acme.themes", "1.0.0", "none") + "\n[dependencies]\n\"acme.base\" = \"^1\"\n";
        assert_eq!(Manifest::parse(&dep).unwrap().dependencies["acme.base"], "^1");
    }
}
