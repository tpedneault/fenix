//! Writing your `keybindings.toml` from inside Fenix (rebinding from the
//! palette): one `[[leader]]` entry set, everything else -- your other
//! entries, your comments -- left as you wrote it. Reading it is the
//! keymap's (fenix-gui's `keymap`).

use std::io;
use std::path::Path;

/// Binds `keys` (`SPC o s`) to `command` in the keybindings file at
/// `path`, made when there isn't one: an entry for the same keys is
/// changed (and no longer unbinds), else one is added at the end.
pub fn write_binding(path: &Path, keys: &str, command: &str) -> io::Result<()> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e),
    };
    let mut doc = text.parse::<toml_edit::DocumentMut>().map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("keybindings.toml can't be read, so it wasn't written over: {}", e.message())))?;
    if !doc.contains_key("leader") {
        doc.insert("leader", toml_edit::Item::ArrayOfTables(toml_edit::ArrayOfTables::new()));
    }
    let Some(entries) = doc["leader"].as_array_of_tables_mut() else {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "keybindings.toml's leader isn't a list of [[leader]] entries"));
    };
    let found = entries.iter().position(|t| t.get("keys").and_then(|k| k.as_str()) == Some(keys));
    match found {
        Some(i) => {
            let entry = entries.get_mut(i).expect("just found");
            entry["command"] = toml_edit::value(command);
            entry.remove("unbind");
        }
        None => {
            let mut entry = toml_edit::Table::new();
            entry["keys"] = toml_edit::value(keys);
            entry["command"] = toml_edit::value(command);
            entries.push(entry);
        }
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    fenix_storage::write(path, doc.to_string().as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_binding_is_added_or_changed_and_the_rest_left_as_written() {
        let dir = std::env::temp_dir().join(format!("fenix-keybindings-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("keybindings.toml");
        write_binding(&path, "SPC o s", "file.save").unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("[[leader]]") && text.contains("keys = \"SPC o s\"") && text.contains("command = \"file.save\""), "{text}");

        std::fs::write(&path, "# mine\n[[leader]]\nkeys = \"SPC a k\"\nunbind = true\n\n[[leader]]\nkeys = \"SPC o s\"\ncommand = \"file.save\"\n").unwrap();
        write_binding(&path, "SPC a k", "agenda.board").unwrap();
        write_binding(&path, "SPC o t", "agenda.open").unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("# mine\n"), "comments stay: {text}");
        assert!(!text.contains("unbind"), "a binding no longer unbinds: {text}");
        assert_eq!(text.matches("[[leader]]").count(), 3, "{text}");
        assert!(text.contains("command = \"agenda.board\"") && text.contains("keys = \"SPC o t\""), "{text}");

        std::fs::write(&path, "leader = 3\n").unwrap();
        assert!(write_binding(&path, "SPC o s", "file.save").is_err(), "not written over");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
