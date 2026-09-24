//! A sketch's own settings, kept where Arduino's tools keep them so
//! Fenix, `arduino-cli` and the Arduino IDE all agree on them: the board
//! and port in `sketch.yaml` (`default_fqbn`, `default_port`), which
//! `arduino-cli` reads on its own. The monitor's speed has no home there,
//! so it lives in Fenix's per-project `.fenix/project.ini` (`[monitor]
//! baudrate`).

use std::path::Path;

pub const DEFAULT_BAUD: u32 = 9600;

/// A top-level `key: value` from `sketch.yaml`, quotes stripped.
pub fn yaml_value(root: &Path, key: &str) -> Option<String> {
    let text = std::fs::read_to_string(root.join("sketch.yaml")).ok()?;
    text.lines().find_map(|line| {
        let rest = line.strip_prefix(key)?.strip_prefix(':')?;
        let value = rest.trim().trim_matches(|c| c == '"' || c == '\'');
        (!value.is_empty()).then(|| value.to_string())
    })
}

/// Sets a top-level `key: value` in `sketch.yaml`, keeping everything
/// else (profiles, comments) as it was; creates the file if needed.
pub fn set_yaml_value(root: &Path, key: &str, value: &str) -> Result<(), String> {
    let path = root.join("sketch.yaml");
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let line = format!("{key}: {value}");
    let mut replaced = false;
    let mut lines: Vec<String> = text
        .lines()
        .map(|l| {
            if !replaced && l.strip_prefix(key).is_some_and(|rest| rest.starts_with(':')) {
                replaced = true;
                line.clone()
            } else {
                l.to_string()
            }
        })
        .collect();
    if !replaced {
        lines.push(line);
    }
    let mut out = lines.join("\n");
    out.push('\n');
    std::fs::write(&path, out).map_err(|err| format!("couldn't write {}: {err}", path.display()))
}

/// `[monitor] baudrate` from `.fenix/project.ini`.
pub fn baud_rate(root: &Path) -> u32 {
    let Ok(text) = std::fs::read_to_string(root.join(".fenix").join("project.ini")) else { return DEFAULT_BAUD };
    let mut in_monitor = false;
    for line in text.lines().map(str::trim) {
        if let Some(header) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            in_monitor = header.trim() == "monitor";
        } else if in_monitor {
            if let Some((key, value)) = line.split_once('=') {
                if key.trim() == "baudrate" {
                    return value.trim().parse().unwrap_or(DEFAULT_BAUD);
                }
            }
        }
    }
    DEFAULT_BAUD
}

/// Sets `[monitor] baudrate`, keeping every other section of
/// `.fenix/project.ini` (tasks, launch) as it was.
pub fn set_baud_rate(root: &Path, baud: u32) -> Result<(), String> {
    let dir = root.join(".fenix");
    let path = dir.join("project.ini");
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let mut out: Vec<String> = Vec::new();
    let mut in_monitor = false;
    let mut saw_section = false;
    let mut written = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(header) = trimmed.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            if in_monitor && !written {
                out.push(format!("baudrate = {baud}"));
                written = true;
            }
            in_monitor = header.trim() == "monitor";
            saw_section |= in_monitor;
        } else if in_monitor && trimmed.split_once('=').is_some_and(|(k, _)| k.trim() == "baudrate") {
            if !written {
                out.push(format!("baudrate = {baud}"));
                written = true;
            }
            continue;
        }
        out.push(line.to_string());
    }
    if in_monitor && !written {
        out.push(format!("baudrate = {baud}"));
        written = true;
    }
    if !saw_section && !written {
        if out.last().is_some_and(|l| !l.trim().is_empty()) {
            out.push(String::new());
        }
        out.push("[monitor]".to_string());
        out.push(format!("baudrate = {baud}"));
    }
    std::fs::create_dir_all(&dir).map_err(|err| format!("couldn't create {}: {err}", dir.display()))?;
    let mut text = out.join("\n");
    text.push('\n');
    std::fs::write(&path, text).map_err(|err| format!("couldn't write {}: {err}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::TempDir;

    #[test]
    fn yaml_values_are_read_and_updated_in_place() {
        let dir = TempDir::new("yaml");
        std::fs::write(dir.path().join("sketch.yaml"), "profiles:\n  uno:\n    fqbn: arduino:avr:uno\ndefault_fqbn: \"arduino:avr:uno\"\n").unwrap();
        assert_eq!(yaml_value(dir.path(), "default_fqbn").as_deref(), Some("arduino:avr:uno"));
        assert_eq!(yaml_value(dir.path(), "default_port"), None);

        set_yaml_value(dir.path(), "default_fqbn", "arduino:avr:nano:cpu=atmega328old").unwrap();
        set_yaml_value(dir.path(), "default_port", "COM4").unwrap();

        let text = std::fs::read_to_string(dir.path().join("sketch.yaml")).unwrap();
        assert!(text.starts_with("profiles:\n  uno:\n    fqbn: arduino:avr:uno\n"), "nested keys are left alone: {text}");
        assert_eq!(yaml_value(dir.path(), "default_fqbn").as_deref(), Some("arduino:avr:nano:cpu=atmega328old"));
        assert_eq!(yaml_value(dir.path(), "default_port").as_deref(), Some("COM4"));
        assert_eq!(text.matches("default_fqbn").count(), 1);
    }

    #[test]
    fn the_baud_rate_defaults_to_9600_and_is_saved_beside_other_sections() {
        let dir = TempDir::new("baud");
        assert_eq!(baud_rate(dir.path()), DEFAULT_BAUD);

        std::fs::create_dir_all(dir.path().join(".fenix")).unwrap();
        std::fs::write(dir.path().join(".fenix").join("project.ini"), "[tasks]\ntask1 = Fmt|clang-format\n").unwrap();
        set_baud_rate(dir.path(), 115200).unwrap();
        assert_eq!(baud_rate(dir.path()), 115200);

        set_baud_rate(dir.path(), 57600).unwrap();
        let text = std::fs::read_to_string(dir.path().join(".fenix").join("project.ini")).unwrap();
        assert_eq!(baud_rate(dir.path()), 57600);
        assert!(text.contains("[tasks]\ntask1 = Fmt|clang-format"));
        assert_eq!(text.matches("baudrate").count(), 1);
    }

    #[test]
    fn a_baud_rate_inside_an_existing_monitor_section_is_replaced() {
        let dir = TempDir::new("baud_existing");
        std::fs::create_dir_all(dir.path().join(".fenix")).unwrap();
        std::fs::write(dir.path().join(".fenix").join("project.ini"), "[monitor]\nbaudrate = 9600\n[launch]\nprogram = x\n").unwrap();
        set_baud_rate(dir.path(), 115200).unwrap();
        let text = std::fs::read_to_string(dir.path().join(".fenix").join("project.ini")).unwrap();
        assert_eq!(text, "[monitor]\nbaudrate = 115200\n[launch]\nprogram = x\n");
    }
}
