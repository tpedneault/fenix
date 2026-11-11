//! Writing your `keybindings.toml` from inside Fenix (rebinding from the
//! palette): one `[[leader]]` or `[[keys]]` entry set, everything else -- your other
//! entries, your comments -- left as you wrote it. Reading it is the
//! keymap's (fenix-gui's `keymap`).

use std::io;
use std::path::Path;

/// Binds `keys` (`SPC o s`) to `command` in the keybindings file at
/// `path`, made when there isn't one: an entry for the same keys is
/// changed (and no longer unbinds), else one is added at the end.
pub fn write_binding(path: &Path, keys: &str, command: &str) -> io::Result<()> {
    write_entry(path, "leader", keys, command, None, None, None)
}

/// Binds the chord `keys` (`C-S-p`, `F12`) to `command` in a `[[keys]]`
/// entry, for the key scheme `scheme` (`vim`, `standard`) -- the same way
/// `write_binding` does a leader key's.
pub fn write_chord(path: &Path, keys: &str, command: &str, scheme: &str) -> io::Result<()> {
    write_entry(path, "keys", keys, command, Some(scheme), None, None)
}

/// `write_chord`, with where it applies (`when`: `text`, `typing`...;
/// `None` anywhere) and, for an entry it adds, a comment above it saying
/// where it came from (`from VS Code`).
pub fn write_chord_noted(path: &Path, keys: &str, command: &str, scheme: &str, when: Option<&str>, note: Option<&str>) -> io::Result<()> {
    write_entry(path, "keys", keys, command, Some(scheme), Some(when), note)
}

fn write_entry(path: &Path, table: &str, keys: &str, command: &str, scheme: Option<&str>, when: Option<Option<&str>>, note: Option<&str>) -> io::Result<()> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e),
    };
    let mut doc = text.parse::<toml_edit::DocumentMut>().map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("keybindings.toml can't be read, so it wasn't written over: {}", e.message())))?;
    if !doc.contains_key(table) {
        doc.insert(table, toml_edit::Item::ArrayOfTables(toml_edit::ArrayOfTables::new()));
    }
    let Some(entries) = doc[table].as_array_of_tables_mut() else {
        return Err(io::Error::new(io::ErrorKind::InvalidData, format!("keybindings.toml's {table} isn't a list of [[{table}]] entries")));
    };
    let same = |t: &toml_edit::Table| t.get("keys").and_then(|k| k.as_str()) == Some(keys) && t.get("scheme").and_then(|k| k.as_str()).unwrap_or("any") == scheme.unwrap_or("any");
    let found = entries.iter().position(same);
    let set_when = |entry: &mut toml_edit::Table| match when {
        Some(Some(when)) => entry["when"] = toml_edit::value(when),
        Some(None) => {
            entry.remove("when");
        }
        None => {}
    };
    match found {
        Some(i) => {
            let entry = entries.get_mut(i).expect("just found");
            entry["command"] = toml_edit::value(command);
            entry.remove("unbind");
            set_when(entry);
        }
        None => {
            let mut entry = toml_edit::Table::new();
            entry["keys"] = toml_edit::value(keys);
            entry["command"] = toml_edit::value(command);
            if let Some(scheme) = scheme {
                entry["scheme"] = toml_edit::value(scheme);
            }
            set_when(&mut entry);
            if let Some(note) = note {
                entry.decor_mut().set_prefix(format!("\n# {note}\n"));
            }
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

        write_chord(&path, "C-S-p", "commands.palette", "standard").unwrap();
        write_chord(&path, "C-S-p", "project.find_file", "standard").unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text.matches("[[keys]]").count(), 1, "the same chord is changed: {text}");
        assert!(text.contains("scheme = \"standard\"") && text.contains("command = \"project.find_file\""), "{text}");

        write_chord_noted(&path, "C-S-d", "edit.copy_lines_down", "standard", Some("text"), Some("from VS Code")).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("# from VS Code\n[[keys]]\nkeys = \"C-S-d\"\ncommand = \"edit.copy_lines_down\"\nscheme = \"standard\"\nwhen = \"text\""), "{text}");
        assert!(text.parse::<toml_edit::DocumentMut>().is_ok(), "{text}");

        std::fs::write(&path, "leader = 3\n").unwrap();
        assert!(write_binding(&path, "SPC o s", "file.save").is_err(), "not written over");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
