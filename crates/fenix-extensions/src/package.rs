//! A package to install: a `.fenixext` (a zip with `extension.toml` at its
//! root) or, while you're making one, the folder it would be made from.
//! Read defensively: names that would climb out of the extension's folder
//! are refused, as is a package that would unpack to more than a data
//! extension has any business being.

use std::io::Read;
use std::path::{Component, Path, PathBuf};

use crate::manifest::Manifest;

/// The most a package may unpack to.
pub const MAX_UNPACKED: u64 = 64 * 1024 * 1024;
/// The most files it may hold.
pub const MAX_FILES: usize = 10_000;

/// A package read: its manifest and its files, by path inside it.
pub struct Package {
    pub manifest: Manifest,
    pub files: Vec<(PathBuf, Vec<u8>)>,
}

impl Package {
    /// A `.fenixext` file, or a folder with `extension.toml` in it.
    pub fn open(path: &Path) -> Result<Package, String> {
        let files = if path.is_dir() { read_dir(path)? } else { read_zip(path)? };
        let manifest = files
            .iter()
            .find(|(p, _)| p == Path::new("extension.toml"))
            .ok_or_else(|| format!("{} has no extension.toml at its root", path.display()))?;
        let text = std::str::from_utf8(&manifest.1).map_err(|_| "extension.toml isn't UTF-8".to_string())?;
        let manifest = Manifest::parse(text)?;
        Ok(Package { manifest, files })
    }
}

/// A path inside a package, as written to disk: relative, and never
/// above the folder it's unpacked into.
fn safe_relative(name: &Path) -> Option<PathBuf> {
    let mut out = PathBuf::new();
    for part in name.components() {
        match part {
            Component::Normal(p) => out.push(p),
            Component::CurDir => {}
            _ => return None,
        }
    }
    (!out.as_os_str().is_empty()).then_some(out)
}

fn read_zip(path: &Path) -> Result<Vec<(PathBuf, Vec<u8>)>, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("couldn't open {}: {e}", path.display()))?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| format!("{} isn't a .fenixext (a zip): {e}", path.display()))?;
    if zip.len() > MAX_FILES {
        return Err(format!("{} holds more than {MAX_FILES} files", path.display()));
    }
    let mut files = Vec::new();
    let mut total = 0u64;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).map_err(|e| format!("{}: {e}", path.display()))?;
        if entry.is_dir() {
            continue;
        }
        let name = entry.name().map(|n| n.to_string()).unwrap_or_default();
        let Some(rel) = entry.enclosed_name().and_then(|p| safe_relative(&p)) else {
            return Err(format!("{} has a file named {name:?}, outside the extension's own folder", path.display()));
        };
        let claimed = entry.size();
        total += claimed;
        if total > MAX_UNPACKED {
            return Err(format!("{} unpacks to more than {} MB", path.display(), MAX_UNPACKED / 1024 / 1024));
        }
        let mut bytes = Vec::with_capacity(claimed as usize);
        // The size a zip claims isn't trusted: read no more than one byte
        // past it, which is enough to tell it lied.
        (&mut entry).take(claimed + 1).read_to_end(&mut bytes).map_err(|e| format!("{}: {name}: {e}", path.display()))?;
        if bytes.len() as u64 > claimed {
            return Err(format!("{}: {name} is bigger than it says", path.display()));
        }
        files.push((rel, bytes));
    }
    Ok(files)
}

fn read_dir(root: &Path) -> Result<Vec<(PathBuf, Vec<u8>)>, String> {
    let mut files = Vec::new();
    let mut total = 0u64;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = std::fs::read_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            let kind = entry.file_type().map_err(|e| e.to_string())?;
            if kind.is_symlink() {
                continue;
            }
            if kind.is_dir() {
                stack.push(path);
                continue;
            }
            let rel = path.strip_prefix(root).map(Path::to_path_buf).map_err(|e| e.to_string())?;
            let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            total += bytes.len() as u64;
            if total > MAX_UNPACKED || files.len() >= MAX_FILES {
                return Err(format!("{} is more than an extension can hold", root.display()));
            }
            files.push((rel, bytes));
        }
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(files)
}

#[cfg(test)]
pub(crate) fn zip_of(path: &Path, files: &[(&str, &str)]) {
    use std::io::Write;
    let mut zip = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
    for (name, text) in files {
        zip.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
        zip.write_all(text.as_bytes()).unwrap();
    }
    zip.finish().unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::sample;

    #[test]
    fn a_package_reads_from_a_zip_or_a_folder() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = sample("acme.themes", "1.0.0", "none");
        let zip = dir.path().join("acme.fenixext");
        zip_of(&zip, &[("extension.toml", &manifest), ("themes/dusk.toml", "name = \"Dusk\""), ("README.md", "# Acme")]);
        let p = Package::open(&zip).unwrap();
        assert_eq!(p.manifest.extension.id, "acme.themes");
        assert!(p.files.iter().any(|(f, b)| f == Path::new("themes/dusk.toml") && b == b"name = \"Dusk\""));

        let folder = dir.path().join("acme");
        std::fs::create_dir_all(folder.join("themes")).unwrap();
        std::fs::write(folder.join("extension.toml"), &manifest).unwrap();
        std::fs::write(folder.join("themes").join("dusk.toml"), "x").unwrap();
        let p = Package::open(&folder).unwrap();
        assert_eq!(p.files.len(), 2);
    }

    #[test]
    fn a_package_without_a_manifest_or_with_a_path_out_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let none = dir.path().join("none.fenixext");
        zip_of(&none, &[("themes/x.toml", "")]);
        assert!(Package::open(&none).err().unwrap().contains("no extension.toml"));
        let out = dir.path().join("out.fenixext");
        zip_of(&out, &[("extension.toml", &sample("acme.x", "1.0.0", "none")), ("../../evil.txt", "x")]);
        assert!(Package::open(&out).err().unwrap().contains("outside the extension's own folder"));
        let junk = dir.path().join("junk.fenixext");
        std::fs::write(&junk, "not a zip").unwrap();
        assert!(Package::open(&junk).err().unwrap().contains("isn't a .fenixext"));
    }

    #[test]
    fn paths_that_climb_out_are_not_safe() {
        assert_eq!(safe_relative(Path::new("themes/a.toml")), Some(PathBuf::from("themes/a.toml")));
        assert_eq!(safe_relative(Path::new("./a")), Some(PathBuf::from("a")));
        assert_eq!(safe_relative(Path::new("../a")), None);
        assert_eq!(safe_relative(Path::new("a/../../b")), None);
        assert_eq!(safe_relative(Path::new("/etc/passwd")), None);
    }
}
