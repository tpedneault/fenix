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
    pub extensions: Vec<Listing>,
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
    pub url: String,
    pub sha256: String,
    pub size: u64,
    #[serde(default)]
    pub dependencies: BTreeMap<String, String>,
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

impl Listing {
    /// The newest version that runs in this Fenix (`fenix`) and that it
    /// can install (data only, for now).
    pub fn latest_for(&self, fenix: &semver::Version) -> Option<&Release> {
        self.versions
            .iter()
            .filter(|r| r.runtime == crate::manifest::Runtime::None)
            .filter(|r| semver::VersionReq::parse(&r.fenix).is_ok_and(|req| req.matches(fenix)))
            .filter_map(|r| semver::Version::parse(&r.version).ok().map(|v| (v, r)))
            .max_by(|a, b| a.0.cmp(&b.0))
            .map(|(_, r)| r)
    }
}

impl Release {
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
    /// installed, first. What was installed, in order.
    pub fn install(&self, id: &str, store: &crate::Store, fetch: &dyn Fetch, fenix: &semver::Version) -> Result<Vec<crate::Installed>, String> {
        let index = self.index(fetch)?;
        let have: Vec<String> = store.installed()?.into_iter().map(|e| e.id).collect();
        let mut plan = Vec::new();
        plan_install(id, &index, fenix, &have, &mut plan, 0)?;
        let downloads = store.root().join(".downloads");
        let mut done = Vec::new();
        for (ext, release) in plan {
            let package = release.download(&ext, fetch, &downloads)?;
            let installed = store.install(&package, fenix);
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
    let listing = index.extensions.iter().find(|l| l.id == id).ok_or_else(|| format!("{id} isn't in the registry"))?;
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
            let listing = index.extensions.iter().find(|l| &l.id == id)?;
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
                {{"version": "1.0.0", "fenix": ">=2.0, <3", "runtime": "none", "url": "https://example.test/dusk-1.0.0.fenixext", "sha256": "{}", "size": {}}},
                {{"version": "1.1.0", "fenix": ">=2.0, <3", "runtime": "none", "url": "https://example.test/dusk-1.1.0.fenixext", "sha256": "{}", "size": {}}},
                {{"version": "2.0.0", "fenix": ">=3.0", "runtime": "none", "url": "x", "sha256": "", "size": 1}},
                {{"version": "1.5.0", "fenix": ">=2.0, <3", "runtime": "wasm", "url": "x", "sha256": "", "size": 1}}
            ]}}]}}"#,
            sha(package),
            package.len(),
            sha(package),
            package.len()
        )
    }

    fn fenix() -> semver::Version {
        semver::Version::parse("2.0.0").unwrap()
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
        assert_eq!(dusk.latest_for(&fenix()).map(|r| r.version.as_str()), Some("1.1.0"), "not 2.0.0 (a later Fenix) nor 1.5.0 (code)");
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
            r#"{{"id": "{id}", "name": "{id}", "versions": [{{"version": "{version}", "fenix": ">=2.0, <3", "runtime": "none", "url": "https://example.test/{id}.fenixext", "sha256": "{}", "size": {}{dep_json}}}]}}"#,
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

    #[test]
    fn a_download_that_isnt_what_the_index_says_is_refused() {
        let release = Release { version: "1.0.0".into(), fenix: ">=2.0".into(), runtime: crate::manifest::Runtime::None, url: "u".into(), sha256: sha(b"good"), size: 4, dependencies: BTreeMap::new() };
        let dir = tempfile::tempdir().unwrap();
        let fake = Fake(HashMap::from([("u".to_string(), b"evil".to_vec())]));
        assert!(release.download("acme.x", &fake, dir.path()).unwrap_err().contains("isn't what the index says"));
        let fake = Fake(HashMap::from([("u".to_string(), b"good".to_vec())]));
        assert!(release.download("acme.x", &fake, dir.path()).is_ok());
    }
}
