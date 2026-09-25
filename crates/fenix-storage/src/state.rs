//! State files: JSON objects that more than one owner shares, each
//! reading and writing its own key. `state/projects.json` holds the known
//! projects and their groups and pins; `state/recent.json` the recent
//! files and folders. A write reads the file again first and replaces
//! only its own key, so two owners never undo each other, and it goes
//! through `atomic_write`, so an interrupted save never leaves half a
//! file.

use std::io;
use std::path::Path;

use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::{Map, Value};

fn object(path: &Path) -> io::Result<Map<String, Value>> {
    match std::fs::read_to_string(path) {
        Ok(text) => match serde_json::from_str::<Value>(&text) {
            Ok(Value::Object(map)) => Ok(map),
            Ok(_) | Err(_) => Err(io::Error::new(io::ErrorKind::InvalidData, format!("{} isn't a JSON object", path.display()))),
        },
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Map::new()),
        Err(e) => Err(e),
    }
}

/// `key` of the JSON object in `path`: `None` when the file or the key
/// isn't there, an error when it is but can't be read as a `T`.
pub fn read<T: DeserializeOwned>(path: &Path, key: &str) -> io::Result<Option<T>> {
    match object(path)?.remove(key) {
        Some(value) => serde_json::from_value(value).map(Some).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("{}: {key}: {e}", path.display()))),
        None => Ok(None),
    }
}

/// Sets `key` of the JSON object in `path` to `value`, leaving its other
/// keys as they are. A file that isn't a JSON object is replaced.
pub fn write<T: Serialize>(path: &Path, key: &str, value: &T) -> io::Result<()> {
    let mut map = object(path).unwrap_or_default();
    map.insert(key.to_string(), serde_json::to_value(value).map_err(io::Error::other)?);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = serde_json::to_string_pretty(&Value::Object(map)).map_err(io::Error::other)? + "\n";
    crate::write(path, text.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_owners_share_a_file_without_undoing_each_other() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state").join("recent.json");
        assert_eq!(read::<Vec<String>>(&path, "files").unwrap(), None, "nothing yet");
        write(&path, "files", &vec!["a.rs"]).unwrap();
        write(&path, "dirs", &vec!["/src"]).unwrap();
        write(&path, "files", &vec!["b.rs", "a.rs"]).unwrap();
        assert_eq!(read::<Vec<String>>(&path, "files").unwrap().unwrap(), ["b.rs", "a.rs"]);
        assert_eq!(read::<Vec<String>>(&path, "dirs").unwrap().unwrap(), ["/src"]);
    }

    #[test]
    fn a_damaged_file_is_an_error_to_read_and_is_replaced_by_a_write() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.json");
        std::fs::write(&path, "[1, 2").unwrap();
        assert!(read::<Vec<u8>>(&path, "k").is_err());
        write(&path, "k", &vec![1u8]).unwrap();
        assert_eq!(read::<Vec<u8>>(&path, "k").unwrap().unwrap(), [1]);
    }
}
