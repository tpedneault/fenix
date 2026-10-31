//! The registry: `index.json`, every published extension and version,
//! signed (`index.json.minisig`) with a key whose public half is built
//! into Fenix, and each package's SHA-256 in it -- so neither the index
//! nor a package can be changed on the way without Fenix refusing it.
//!
//! The index lives on the `registry` branch of
//! `tpedneault/fenix-extensions`; packages are that repository's release
//! assets. `extensions.registry` points elsewhere (a mirror).

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use sha2::{Digest, Sha256};

/// Where the index is, unless `extensions.registry` says otherwise.
pub const DEFAULT_URL: &str = "https://raw.githubusercontent.com/tpedneault/fenix-extensions/registry/index.json";

/// The most an index may be.
const MAX_INDEX: u64 = 8 * 1024 * 1024;

/// The index, as published.
#[derive(Debug, Clone, Deserialize)]
pub struct Index {
    /// The format's version; this Fenix reads 1.
    pub format: u32,
    /// The listings this Fenix can read (see `readable`).
    #[serde(deserialize_with = "readable")]
    pub extensions: Vec<Listing>,
    /// The bundles, apart from the rest: a Fenix before bundles reads past
    /// them.
    #[serde(default)]
    pub bundles: Vec<Listing>,
}

/// The entries of a list that this Fenix can read, the others left out.
/// A newer extension can ask for what this Fenix doesn't know -- a
/// permission a newer API added, say -- and that one entry mustn't make
/// the whole index unreadable: an older Fenix just doesn't offer it, as
/// it doesn't offer a version built for a newer API.
fn readable<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::de::DeserializeOwned,
{
    let raw: Vec<serde_json::Value> = Deserialize::deserialize(deserializer)?;
    Ok(raw.into_iter().filter_map(|entry| serde_json::from_value(entry).ok()).collect())
}

/// One extension in the index, with every version published.
#[derive(Debug, Clone, Deserialize)]
pub struct Listing {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub categories: Vec<String>,
    #[serde(default)]
    pub repository: Option<String>,
    /// The versions this Fenix can read (see `readable`).
    #[serde(deserialize_with = "readable")]
    pub versions: Vec<Release>,
}

/// One published version.
#[derive(Debug, Clone, Deserialize)]
pub struct Release {
    pub version: String,
    /// The Fenix releases it runs in.
    pub fenix: String,
    /// `none` (data), `wasm` or `lua`.
    pub runtime: crate::manifest::Runtime,
    /// The extension API a code extension was built against.
    #[serde(default)]
    pub api: Option<String>,
    /// What a code extension may do, as its manifest asks: shown before
    /// it's installed.
    #[serde(default)]
    pub permissions: crate::manifest::Permissions,
    pub url: String,
    pub sha256: String,
    pub size: u64,
    #[serde(default)]
    pub dependencies: BTreeMap<String, String>,
    /// What it's filed under, at this version (the listing's are its
    /// newest's; an index before these has those alone).
    #[serde(default)]
    pub categories: Vec<String>,
    /// A bundle's members, by id.
    #[serde(default)]
    pub bundle: Vec<String>,
}

/// How the registry is read: a URL in, its bytes out (at most `limit`).
pub trait Fetch {
    fn get(&self, url: &str, limit: u64) -> Result<Vec<u8>, String>;
}

/// Over HTTPS, with the system's certificates.
pub struct Http;

impl Fetch for Http {
    fn get(&self, url: &str, limit: u64) -> Result<Vec<u8>, String> {
        let response = ureq::get(url).timeout(std::time::Duration::from_secs(30)).call().map_err(|e| format!("{url}: {e}"))?;
        let mut bytes = Vec::new();
        response.into_reader().take(limit + 1).read_to_end(&mut bytes).map_err(|e| format!("{url}: {e}"))?;
        if bytes.len() as u64 > limit {
            return Err(format!("{url} is bigger than it should be"));
        }
        Ok(bytes)
    }
}

/// The registry at `url`, trusted when signed by `public_key` (minisign,
/// base64).
pub struct Registry<'a> {
    pub url: String,
    pub public_key: &'a str,
}

impl Registry<'_> {
    /// The index, its signature checked.
    pub fn index(&self, fetch: &dyn Fetch) -> Result<Index, String> {
        if self.public_key.is_empty() {
            return Err("this Fenix has no registry key built in yet -- install from a file instead".into());
        }
        let key = minisign_verify::PublicKey::from_base64(self.public_key).map_err(|e| format!("the registry key: {e}"))?;
        let bytes = fetch.get(&self.url, MAX_INDEX)?;
        let sig = fetch.get(&format!("{}.minisig", self.url), 64 * 1024)?;
        let sig = std::str::from_utf8(&sig).map_err(|_| "the index's signature isn't text".to_string())?;
        let sig = minisign_verify::Signature::decode(sig).map_err(|e| format!("the index's signature: {e}"))?;
        key.verify(&bytes, &sig, false).map_err(|_| format!("{}: the signature doesn't match -- not trusted", self.url))?;
        let index: Index = serde_json::from_slice(&bytes).map_err(|e| format!("the index: {e}"))?;
        if index.format != 1 {
            return Err(format!("the index is format {}, and this Fenix reads 1 -- a newer Fenix reads it", index.format));
        }
        Ok(index)
    }
}

impl Index {
    /// Extension or bundle `id`.
    pub fn listing(&self, id: &str) -> Option<&Listing> {
        self.extensions.iter().chain(&self.bundles).find(|l| l.id == id)
    }

    /// The bundles whose newest version for this Fenix has `id` in it.
    pub fn bundles_of(&self, id: &str, fenix: &semver::Version) -> Vec<&Listing> {
        self.bundles.iter().filter(|b| b.latest_for(fenix).is_some_and(|r| r.members().iter().any(|m| m == id))).collect()
    }
}

impl Listing {
    /// The newest version that runs in this Fenix (`fenix`) and that it
    /// can run: data, a bundle, or WebAssembly built for this Fenix's API.
    pub fn latest_for(&self, fenix: &semver::Version) -> Option<&Release> {
        self.versions
            .iter()
            .filter(|r| match r.runtime {
                crate::manifest::Runtime::None | crate::manifest::Runtime::Bundle => true,
                crate::manifest::Runtime::Wasm => r.api.as_deref().is_some_and(crate::manifest::api_fits),
                crate::manifest::Runtime::Lua => false,
            })
            .filter(|r| semver::VersionReq::parse(&r.fenix).is_ok_and(|req| req.matches(fenix)))
            .filter_map(|r| semver::Version::parse(&r.version).ok().map(|v| (v, r)))
            .max_by(|a, b| a.0.cmp(&b.0))
            .map(|(_, r)| r)
    }
}

impl Release {
    /// A bundle's members: `bundle`, or what it depends on when the index
    /// doesn't say.
    pub fn members(&self) -> Vec<String> {
        match (self.runtime, self.bundle.is_empty()) {
            (crate::manifest::Runtime::Bundle, true) => self.dependencies.keys().cloned().collect(),
            _ => self.bundle.clone(),
        }
    }

    /// What its package says of it besides the index -- its README,
    /// CHANGELOG and screenshots -- unpacked into `<dir>/<id>-<version>/`
    /// from the package, downloaded and checked as `download` does, before
    /// it's installed. Nothing else in it is written. The folder.
    pub fn preview(&self, id: &str, fetch: &dyn Fetch, dir: &Path) -> Result<PathBuf, String> {
        let out = dir.join(format!("{id}-{}", self.version));
        if out.join(".complete").is_file() {
            return Ok(out);
        }
        let package = self.download(id, fetch, dir)?;
        let opened = crate::Package::open(&package);
        let _ = std::fs::remove_file(&package);
        let _ = std::fs::remove_dir_all(&out);
        for (rel, bytes) in opened?.files {
            let shown = rel == Path::new("README.md") || rel == Path::new("CHANGELOG.md") || rel.starts_with("screenshots");
            if !shown {
                continue;
            }
            let to = out.join(&rel);
            if let Some(parent) = to.parent() {
                std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
            }
            std::fs::write(&to, bytes).map_err(|e| format!("{}: {e}", to.display()))?;
        }
        std::fs::create_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
        std::fs::write(out.join(".complete"), "").map_err(|e| e.to_string())?;
        Ok(out)
    }

    /// The package, downloaded into `dir` and checked against the index's
    /// hash and size: its path, ready for `Store::install`.
    pub fn download(&self, id: &str, fetch: &dyn Fetch, dir: &Path) -> Result<PathBuf, String> {
        if self.size > crate::package::MAX_UNPACKED {
            return Err(format!("{id} {} is bigger than an extension may be", self.version));
        }
        let bytes = fetch.get(&self.url, self.size)?;
        let hash = Sha256::digest(&bytes);
        let hex: String = hash.iter().map(|b| format!("{b:02x}")).collect();
        if bytes.len() as u64 != self.size || !hex.eq_ignore_ascii_case(&self.sha256) {
            return Err(format!("{id} {}: the download isn't what the index says it is -- not installed", self.version));
        }
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let path = dir.join(format!("{id}-{}.fenixext", self.version));
        std::fs::write(&path, &bytes).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(path)
    }
}

impl Registry<'_> {
    /// Installs extension `id` from the registry into `store` for this
    /// Fenix (`fenix`), with the extensions it needs that aren't
    /// installed, first -- a bundle's members, for a bundle; those are
    /// removed with the last that needs them. What was installed, in
    /// order.
    pub fn install(&self, id: &str, store: &crate::Store, fetch: &dyn Fetch, fenix: &semver::Version) -> Result<Vec<crate::Installed>, String> {
        let index = self.index(fetch)?;
        let have: Vec<String> = store.installed()?.into_iter().map(|e| e.id).collect();
        let mut plan = Vec::new();
        plan_install(id, &index, fenix, &have, &mut plan, 0)?;
        let downloads = store.root().join(".downloads");
        let mut done = Vec::new();
        for (ext, release) in plan {
            let package = release.download(&ext, fetch, &downloads)?;
            let installed = if ext == id { store.install(&package, fenix) } else { store.install_dependency(&package, fenix) };
            let _ = std::fs::remove_file(&package);
            done.push(installed?);
        }
        let _ = std::fs::remove_dir_all(&downloads);
        Ok(done)
    }
}

/// `id` and what it needs, dependencies first, each once.
fn plan_install<'a>(id: &str, index: &'a Index, fenix: &semver::Version, have: &[String], out: &mut Vec<(String, &'a Release)>, depth: usize) -> Result<(), String> {
    if depth > 16 {
        return Err(format!("{id}'s dependencies go round in a circle"));
    }
    if out.iter().any(|(o, _)| o == id) {
        return Ok(());
    }
    let listing = index.listing(id).ok_or_else(|| format!("{id} isn't in the registry"))?;
    let release = listing.latest_for(fenix).ok_or_else(|| format!("the registry has no version of {} for this Fenix", listing.name))?;
    for dep in release.dependencies.keys() {
        if !have.contains(dep) {
            plan_install(dep, index, fenix, have, out, depth + 1)?;
        }
    }
    out.push((id.to_string(), release));
    Ok(())
}

/// An update: an installed extension with a newer version in the index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Update {
    pub id: String,
    pub installed: String,
    pub latest: String,
}

/// The installed extensions (`(id, version)`) the index has newer
/// versions of, for this Fenix.
pub fn updates(index: &Index, installed: &[(String, String)], fenix: &semver::Version) -> Vec<Update> {
    installed
        .iter()
        .filter_map(|(id, have)| {
            let listing = index.listing(id)?;
            let latest = listing.latest_for(fenix)?;
            let newer = semver::Version::parse(&latest.version).ok()? > semver::Version::parse(have).ok()?;
            newer.then(|| Update { id: id.clone(), installed: have.clone(), latest: latest.version.clone() })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// The registry over a map of URLs.
    struct Fake(HashMap<String, Vec<u8>>);

    impl Fetch for Fake {
        fn get(&self, url: &str, limit: u64) -> Result<Vec<u8>, String> {
            let bytes = self.0.get(url).cloned().ok_or_else(|| format!("{url}: 404"))?;
            if bytes.len() as u64 > limit {
                return Err(format!("{url} is bigger than it should be"));
            }
            Ok(bytes)
        }
    }

    fn keypair() -> minisign::KeyPair {
        minisign::KeyPair::generate_unencrypted_keypair().unwrap()
    }

    fn sign(kp: &minisign::KeyPair, bytes: &[u8]) -> Vec<u8> {
        minisign::sign(Some(&kp.pk), &kp.sk, bytes, None, None).unwrap().to_string().into_bytes()
    }

    fn sha(bytes: &[u8]) -> String {
        Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
    }

    const URL: &str = "https://example.test/index.json";

    fn index_json(package: &[u8]) -> String {
        format!(
            r#"{{"format": 1, "extensions": [{{"id": "acme.dusk", "name": "Dusk", "description": "A theme", "versions": [
                {{"version": "1.0.0", "fenix": ">=1.0, <2", "runtime": "none", "url": "https://example.test/dusk-1.0.0.fenixext", "sha256": "{}", "size": {}}},
                {{"version": "1.1.0", "fenix": ">=1.0, <2", "runtime": "none", "url": "https://example.test/dusk-1.1.0.fenixext", "sha256": "{}", "size": {}}},
                {{"version": "2.0.0", "fenix": ">=2.0", "runtime": "none", "url": "x", "sha256": "", "size": 1}},
                {{"version": "1.5.0", "fenix": ">=1.0, <2", "runtime": "wasm", "url": "x", "sha256": "", "size": 1}}
            ]}}]}}"#,
            sha(package),
            package.len(),
            sha(package),
            package.len()
        )
    }

    fn fenix() -> semver::Version {
        semver::Version::parse("1.0.0").unwrap()
    }

    #[test]
    fn what_this_fenix_cant_read_is_left_out_not_the_whole_index() {
        // A permission no Fenix knows yet (as `sound` was to 2.0.0), and
        // a listing in a shape this Fenix doesn't know.
        let json = r#"{"format": 1, "extensions": [
            {"id": "fenix.pomodoro", "name": "Pomodoro", "versions": [
                {"version": "1.0.0", "fenix": ">=1.0, <2", "runtime": "wasm", "api": "1.0", "url": "x", "sha256": "", "size": 1},
                {"version": "1.1.0", "fenix": ">=1.0, <2", "runtime": "wasm", "api": "1.0", "permissions": {"teleport": true}, "url": "x", "sha256": "", "size": 1}
            ]},
            {"id": "acme.future", "name": "Future", "versions": "not a list"},
            {"id": "acme.dusk", "name": "Dusk", "versions": [
                {"version": "1.0.0", "fenix": ">=1.0, <2", "runtime": "none", "url": "x", "sha256": "", "size": 1}
            ]}
        ]}"#;
        let index: Index = serde_json::from_str(json).unwrap();
        let ids: Vec<&str> = index.extensions.iter().map(|l| l.id.as_str()).collect();
        assert_eq!(ids, vec!["fenix.pomodoro", "acme.dusk"], "only the listing it can't read is left out");
        let pomodoro = &index.extensions[0];
        assert_eq!(pomodoro.versions.len(), 1, "the version asking for `teleport` is left out");
        assert_eq!(pomodoro.versions[0].version, "1.0.0");
    }

    #[test]
    fn a_signed_index_is_read_and_its_newest_runnable_version_chosen() {
        let kp = keypair();
        let package = b"zip bytes".to_vec();
        let index = index_json(&package);
        let fake = Fake(HashMap::from([(URL.to_string(), index.clone().into_bytes()), (format!("{URL}.minisig"), sign(&kp, index.as_bytes())), ("https://example.test/dusk-1.1.0.fenixext".to_string(), package.clone())]));
        let pk = kp.pk.to_base64();
        let registry = Registry { url: URL.into(), public_key: &pk };
        let index = registry.index(&fake).unwrap();
        let dusk = &index.extensions[0];
        assert_eq!(dusk.latest_for(&fenix()).map(|r| r.version.as_str()), Some("1.1.0"), "not 2.0.0 (a later Fenix) nor 1.5.0 (code for no API)");
        let mut code = dusk.clone();
        code.versions[3].api = Some("1.0".into());
        assert_eq!(code.latest_for(&fenix()).map(|r| r.version.as_str()), Some("1.5.0"), "code for this API");
        code.versions[3].api = Some("2.0".into());
        assert_eq!(code.latest_for(&fenix()).map(|r| r.version.as_str()), Some("1.1.0"), "code for another API");
        let dir = tempfile::tempdir().unwrap();
        let path = dusk.latest_for(&fenix()).unwrap().download("acme.dusk", &fake, dir.path()).unwrap();
        assert_eq!(std::fs::read(path).unwrap(), package);
        assert_eq!(updates(&index, &[("acme.dusk".into(), "1.0.0".into()), ("acme.other".into(), "1.0.0".into())], &fenix()), [Update { id: "acme.dusk".into(), installed: "1.0.0".into(), latest: "1.1.0".into() }]);
        assert!(updates(&index, &[("acme.dusk".into(), "1.1.0".into())], &fenix()).is_empty());
    }

    #[test]
    fn an_index_not_signed_by_the_key_is_refused() {
        let (ours, theirs) = (keypair(), keypair());
        let index = index_json(b"x");
        let fake = Fake(HashMap::from([(URL.to_string(), index.clone().into_bytes()), (format!("{URL}.minisig"), sign(&theirs, index.as_bytes()))]));
        let pk = ours.pk.to_base64();
        assert!(Registry { url: URL.into(), public_key: &pk }.index(&fake).is_err());
        // Signed by ours, then changed on the way.
        let tampered = index.replace("A theme", "A virus");
        let fake = Fake(HashMap::from([(URL.to_string(), tampered.into_bytes()), (format!("{URL}.minisig"), sign(&ours, index.as_bytes()))]));
        assert!(Registry { url: URL.into(), public_key: &pk }.index(&fake).unwrap_err().contains("not trusted"));
        assert!(Registry { url: URL.into(), public_key: "" }.index(&fake).unwrap_err().contains("no registry key"));
    }

    /// A real package, zipped, with its index entry.
    fn published(dir: &Path, id: &str, version: &str, deps: &str) -> (Vec<u8>, String) {
        let path = dir.join(format!("{id}.fenixext"));
        let manifest = crate::manifest::sample(id, version, "none") + &if deps.is_empty() { String::new() } else { format!("\n[dependencies]\n{deps}\n") };
        crate::package::zip_of(&path, &[("extension.toml", &manifest)]);
        let bytes = std::fs::read(&path).unwrap();
        let dep_json = if deps.is_empty() { String::new() } else { format!(r#", "dependencies": {{{}}}"#, deps.replace(" = ", ": ")) };
        let entry = format!(
            r#"{{"id": "{id}", "name": "{id}", "versions": [{{"version": "{version}", "fenix": ">=1.0, <2", "runtime": "none", "url": "https://example.test/{id}.fenixext", "sha256": "{}", "size": {}{dep_json}}}]}}"#,
            sha(&bytes),
            bytes.len()
        );
        (bytes, entry)
    }

    #[test]
    fn installing_by_id_brings_what_it_needs_first() {
        let kp = keypair();
        let dir = tempfile::tempdir().unwrap();
        let (base, base_entry) = published(dir.path(), "acme.base", "1.0.0", "");
        let (pack, pack_entry) = published(dir.path(), "acme.pack", "1.0.0", r#""acme.base" = "^1""#);
        let index = format!(r#"{{"format": 1, "extensions": [{pack_entry}, {base_entry}]}}"#);
        let fake = Fake(HashMap::from([
            (URL.to_string(), index.clone().into_bytes()),
            (format!("{URL}.minisig"), sign(&kp, index.as_bytes())),
            ("https://example.test/acme.base.fenixext".to_string(), base),
            ("https://example.test/acme.pack.fenixext".to_string(), pack),
        ]));
        let pk = kp.pk.to_base64();
        let registry = Registry { url: URL.into(), public_key: &pk };
        let store = crate::Store::new(dir.path().join("extensions"));
        let done = registry.install("acme.pack", &store, &fake, &fenix()).unwrap();
        assert_eq!(done.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(), ["acme.base", "acme.pack"]);
        assert!(!store.root().join(".downloads").exists(), "downloads cleared");
        let again = registry.install("acme.pack", &store, &fake, &fenix()).unwrap();
        assert_eq!(again.len(), 1, "what it needs is there already");
        assert!(registry.install("acme.nope", &store, &fake, &fenix()).unwrap_err().contains("isn't in the registry"));
    }

    /// A bundle from the registry brings its members, each marked as
    /// there for it; an index from before bundles reads as it did.
    #[test]
    fn a_bundle_installs_its_members_and_is_read_from_its_own_list() {
        let kp = keypair();
        let dir = tempfile::tempdir().unwrap();
        let (notes, notes_entry) = published(dir.path(), "acme.notes", "1.0.0", "");
        let path = dir.path().join("acme.personal.fenixext");
        let manifest = crate::manifest::sample("acme.personal", "1.0.0", "bundle") + "\n[dependencies]\n\"acme.notes\" = \"^1\"\n";
        crate::package::zip_of(&path, &[("extension.toml", &manifest), ("README.md", "# Personal"), ("screenshots/board.png", "png")]);
        let personal = std::fs::read(&path).unwrap();
        let bundle_entry = format!(
            r#"{{"id": "acme.personal", "name": "Personal", "description": "Notes", "categories": ["tools"], "versions": [{{"version": "1.0.0", "fenix": ">=1.0, <2", "runtime": "bundle", "url": "https://example.test/acme.personal.fenixext", "sha256": "{}", "size": {}, "dependencies": {{"acme.notes": "^1"}}, "categories": ["tools"], "bundle": ["acme.notes"]}}]}}"#,
            sha(&personal),
            personal.len()
        );
        let index = format!(r#"{{"format": 1, "extensions": [{notes_entry}], "bundles": [{bundle_entry}]}}"#);
        let fake = Fake(HashMap::from([
            (URL.to_string(), index.clone().into_bytes()),
            (format!("{URL}.minisig"), sign(&kp, index.as_bytes())),
            ("https://example.test/acme.notes.fenixext".to_string(), notes),
            ("https://example.test/acme.personal.fenixext".to_string(), personal),
        ]));
        let pk = kp.pk.to_base64();
        let registry = Registry { url: URL.into(), public_key: &pk };
        let read = registry.index(&fake).unwrap();
        assert_eq!(read.extensions.len(), 1, "a bundle isn't one of the extensions");
        assert_eq!(read.bundles_of("acme.notes", &fenix()).iter().map(|b| b.name.as_str()).collect::<Vec<_>>(), ["Personal"]);
        assert_eq!(read.listing("acme.personal").unwrap().versions[0].members(), ["acme.notes"]);

        // Its README and screenshots, before it's installed.
        let preview = read.listing("acme.personal").unwrap().versions[0].preview("acme.personal", &fake, &dir.path().join("previews")).unwrap();
        assert!(preview.join("README.md").is_file() && preview.join("screenshots/board.png").is_file() && !preview.join("extension.toml").exists());

        let store = crate::Store::new(dir.path().join("extensions"));
        let done = registry.install("acme.personal", &store, &fake, &fenix()).unwrap();
        assert_eq!(done.iter().map(|e| (e.id.as_str(), e.dependency)).collect::<Vec<_>>(), [("acme.notes", true), ("acme.personal", false)]);
        assert_eq!(store.uninstall_unneeded("acme.personal").unwrap(), ["acme.personal", "acme.notes"]);

        // The index before bundles, and a Fenix before them reading this
        // one: `bundles` and the new fields are read past.
        let old: Index = serde_json::from_str(&index_json(b"x")).unwrap();
        assert!(old.bundles.is_empty() && old.extensions[0].versions[0].categories.is_empty());
        #[derive(Deserialize)]
        #[allow(dead_code)]
        struct Before {
            format: u32,
            extensions: Vec<serde_json::Value>,
        }
        assert!(serde_json::from_str::<Before>(&index).is_ok());
    }

    #[test]
    fn a_download_that_isnt_what_the_index_says_is_refused() {
        let release = Release { version: "1.0.0".into(), fenix: ">=2.0".into(), runtime: crate::manifest::Runtime::None, api: None, permissions: Default::default(), url: "u".into(), sha256: sha(b"good"), size: 4, dependencies: BTreeMap::new(), categories: Vec::new(), bundle: Vec::new() };
        let dir = tempfile::tempdir().unwrap();
        let fake = Fake(HashMap::from([("u".to_string(), b"evil".to_vec())]));
        assert!(release.download("acme.x", &fake, dir.path()).unwrap_err().contains("isn't what the index says"));
        let fake = Fake(HashMap::from([("u".to_string(), b"good".to_vec())]));
        assert!(release.download("acme.x", &fake, dir.path()).is_ok());
    }
}
