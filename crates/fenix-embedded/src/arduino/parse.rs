//! `arduino-cli ... --format json` output, turned into the platform-
//! neutral types. Every parser tolerates missing fields: a row it can't
//! use is skipped rather than failing the whole list.

use serde_json::Value;

use crate::{Board, BoardOption, Library, OptionValue, Package, Port};

fn parse(json: &str) -> Result<Value, String> {
    serde_json::from_str(json.trim_start_matches('\u{feff}')).map_err(|err| format!("unexpected arduino-cli output: {err}"))
}

fn str_of<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str)
}

fn array<'a>(v: &'a Value, key: &str) -> impl Iterator<Item = &'a Value> {
    v.get(key).and_then(Value::as_array).into_iter().flatten()
}

/// The package part of an FQBN: `arduino:avr:uno` -> `arduino:avr`.
pub fn package_of(fqbn: &str) -> String {
    fqbn.splitn(3, ':').take(2).collect::<Vec<_>>().join(":")
}

fn board(v: &Value) -> Option<Board> {
    let id = str_of(v, "fqbn")?.to_string();
    Some(Board { name: str_of(v, "name").unwrap_or(&id).to_string(), package: package_of(&id), id })
}

/// `board list`.
pub fn ports(json: &str) -> Result<Vec<Port>, String> {
    let v = parse(json)?;
    Ok(array(&v, "detected_ports")
        .filter_map(|detected| {
            let port = detected.get("port")?;
            let address = str_of(port, "address")?.to_string();
            Some(Port {
                label: str_of(port, "label").unwrap_or(&address).to_string(),
                protocol: str_of(port, "protocol_label").or_else(|| str_of(port, "protocol")).unwrap_or_default().to_string(),
                boards: array(detected, "matching_boards").filter_map(board).collect(),
                address,
            })
        })
        .collect())
}

/// `board listall`.
pub fn boards(json: &str) -> Result<Vec<Board>, String> {
    let v = parse(json)?;
    let mut boards: Vec<Board> = array(&v, "boards").filter_map(board).collect();
    boards.sort_by(|a, b| a.package.cmp(&b.package).then_with(|| a.name.cmp(&b.name)));
    Ok(boards)
}

/// `board details`'s `config_options`.
pub fn board_options(json: &str) -> Result<Vec<BoardOption>, String> {
    let v = parse(json)?;
    Ok(array(&v, "config_options")
        .filter_map(|option| {
            Some(BoardOption {
                id: str_of(option, "option")?.to_string(),
                label: str_of(option, "option_label").or_else(|| str_of(option, "option"))?.to_string(),
                values: array(option, "values")
                    .filter_map(|value| {
                        let id = str_of(value, "value")?.to_string();
                        Some(OptionValue {
                            label: str_of(value, "value_label").unwrap_or(&id).to_string(),
                            selected: value.get("selected").and_then(Value::as_bool).unwrap_or(false),
                            value: id,
                        })
                    })
                    .collect(),
            })
        })
        .collect())
}

/// `lib search` joined with `lib list` (which knows what's installed).
pub fn libraries(search_json: &str, installed_json: &str) -> Result<Vec<Library>, String> {
    let search = parse(search_json)?;
    let installed = parse(installed_json)?;
    let installed: Vec<(String, String)> = array(&installed, "installed_libraries")
        .filter_map(|entry| {
            let lib = entry.get("library")?;
            Some((str_of(lib, "name")?.to_string(), str_of(lib, "version").unwrap_or_default().to_string()))
        })
        .collect();
    Ok(array(&search, "libraries")
        .filter_map(|lib| {
            let name = str_of(lib, "name")?.to_string();
            let latest = lib.get("latest");
            let field = |key: &str| latest.and_then(|l| str_of(l, key)).unwrap_or_default().to_string();
            Some(Library {
                installed: installed.iter().find(|(n, _)| *n == name).map(|(_, v)| v.clone()),
                latest: field("version"),
                author: field("author"),
                summary: field("sentence"),
                name,
            })
        })
        .collect())
}

/// `core search`.
pub fn packages(json: &str) -> Result<Vec<Package>, String> {
    let v = parse(json)?;
    Ok(array(&v, "platforms")
        .filter_map(|platform| {
            let id = str_of(platform, "id")?.to_string();
            let latest = str_of(platform, "latest_version").unwrap_or_default().to_string();
            let installed = str_of(platform, "installed_version").filter(|s| !s.is_empty()).map(str::to_string);
            let release = platform.get("releases").and_then(|r| r.get(&latest));
            Some(Package { name: release.and_then(|r| str_of(r, "name")).unwrap_or(&id).to_string(), id, latest, installed })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ports_carry_the_boards_recognized_on_them() {
        let json = r#"{"detected_ports": [
            {"port": {"address": "COM1", "label": "COM1", "protocol": "serial", "protocol_label": "Serial Port", "properties": {}}},
            {"matching_boards": [{"name": "Arduino UNO", "fqbn": "arduino:avr:uno"}],
             "port": {"address": "COM4", "label": "COM4", "protocol": "serial", "protocol_label": "Serial Port (USB)",
                      "properties": {"pid": "0x0043", "vid": "0x2341"}}}
        ]}"#;
        let ports = ports(json).unwrap();
        assert_eq!(ports.len(), 2);
        assert!(ports[0].boards.is_empty());
        assert_eq!(ports[1].address, "COM4");
        assert_eq!(ports[1].protocol, "Serial Port (USB)");
        assert_eq!(ports[1].boards[0], Board { id: "arduino:avr:uno".to_string(), name: "Arduino UNO".to_string(), package: "arduino:avr".to_string() });
    }

    #[test]
    fn no_detected_ports_is_an_empty_list() {
        assert!(ports("{}").unwrap().is_empty());
    }

    #[test]
    fn boards_are_listed_by_package_then_name() {
        let json = r#"{"boards": [
            {"name": "Arduino Nano", "fqbn": "arduino:avr:nano"},
            {"name": "Arduino Mega or Mega 2560", "fqbn": "arduino:avr:mega"},
            {"name": "no fqbn, skipped"}
        ]}"#;
        let names: Vec<String> = boards(json).unwrap().into_iter().map(|b| b.name).collect();
        assert_eq!(names, ["Arduino Mega or Mega 2560", "Arduino Nano"]);
    }

    #[test]
    fn board_options_mark_the_selected_value() {
        let json = r#"{"fqbn": "arduino:avr:nano", "config_options": [{"option": "cpu", "option_label": "Processor", "values": [
            {"value": "atmega328", "value_label": "ATmega328P", "selected": true},
            {"value": "atmega328old", "value_label": "ATmega328P (Old Bootloader)"}
        ]}]}"#;
        let options = board_options(json).unwrap();
        assert_eq!(options[0].id, "cpu");
        assert_eq!(options[0].label, "Processor");
        assert!(options[0].values[0].selected);
        assert!(!options[0].values[1].selected);
        assert_eq!(options[0].values[1].label, "ATmega328P (Old Bootloader)");
    }

    #[test]
    fn libraries_know_which_are_installed() {
        let search = r#"{"libraries": [
            {"name": "Servo", "latest": {"version": "1.3.0", "author": "Michael Margolis, Arduino", "sentence": "Allows Arduino boards to control a variety of servo motors."}},
            {"name": "DHT sensor library", "latest": {"version": "1.4.6", "author": "Adafruit", "sentence": "Arduino library for DHT11, DHT22, etc Temp & Humidity Sensors"}}
        ], "status": "success"}"#;
        let installed = r#"{"installed_libraries": [{"library": {"name": "Servo", "version": "1.2.2"}}]}"#;
        let libs = libraries(search, installed).unwrap();
        assert_eq!(libs[0].installed.as_deref(), Some("1.2.2"));
        assert_eq!(libs[0].latest, "1.3.0");
        assert_eq!(libs[1].installed, None);
        assert_eq!(libs[1].author, "Adafruit");
        assert!(libraries(search, r#"{"installed_libraries": []}"#).unwrap().iter().all(|l| l.installed.is_none()));
    }

    #[test]
    fn packages_take_their_name_from_the_latest_release() {
        let json = r#"{"platforms": [
            {"id": "arduino:avr", "installed_version": "1.8.8", "latest_version": "1.8.8",
             "releases": {"1.8.6": {"name": "Old name"}, "1.8.8": {"name": "Arduino AVR Boards"}}},
            {"id": "arduino:megaavr", "latest_version": "1.8.9", "releases": {"1.8.9": {"name": "Arduino megaAVR Boards"}}}
        ]}"#;
        let packages = packages(json).unwrap();
        assert_eq!(packages[0].name, "Arduino AVR Boards");
        assert_eq!(packages[0].installed.as_deref(), Some("1.8.8"));
        assert_eq!(packages[1].installed, None);
    }

    #[test]
    fn a_byte_order_mark_is_ignored() {
        assert!(ports("\u{feff}{\"detected_ports\": []}").unwrap().is_empty());
    }
}
