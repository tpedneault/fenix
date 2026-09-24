//! Arduino sketches, through `arduino-cli`.
//!
//! A sketch is a folder holding `<folder name>.ino` (or a `sketch.yaml`).
//! Everything runs through the CLI -- compiling, uploading, the serial
//! monitor, board/library/package management -- and completion comes
//! from `arduino-language-server`, which runs clangd against the sketch
//! as the CLI would compile it (`.ino` preprocessing, the board's core
//! and every library it includes).

mod parse;
mod sketch;

use std::path::{Path, PathBuf};

use crate::{process, Board, BoardOption, Command, Debugging, Library, Package, Platform, Port, Tool, Tools};

pub use parse::package_of;
pub use sketch::DEFAULT_BAUD;

/// The board a sketch that hasn't picked one is built for.
pub const DEFAULT_BOARD: &str = "arduino:avr:uno";

/// Speeds the monitor offers, the same list the Arduino IDE has.
pub const BAUD_RATES: [u32; 12] = [300, 600, 750, 1200, 2400, 4800, 9600, 19200, 31250, 38400, 57600, 115200];

/// Whether `dir` is a sketch folder.
pub fn is_sketch(dir: &Path) -> bool {
    if dir.join("sketch.yaml").is_file() {
        return true;
    }
    let Some(name) = dir.file_name().and_then(|n| n.to_str()) else { return false };
    dir.join(format!("{name}.ino")).is_file() || dir.join(format!("{name}.pde")).is_file()
}

pub struct Arduino {
    root: PathBuf,
    cli: Option<PathBuf>,
    clangd: Option<PathBuf>,
    language_server: Option<PathBuf>,
}

impl Arduino {
    pub fn detect(root: &Path, tools: &Tools) -> Option<Self> {
        is_sketch(root).then(|| Self {
            root: root.to_path_buf(),
            cli: tools.arduino_cli.clone(),
            clangd: tools.clangd.clone(),
            language_server: tools.arduino_language_server.clone(),
        })
    }

    fn cli(&self) -> Result<&Path, String> {
        self.cli.as_deref().ok_or_else(|| format!("{} not found -- {}", Tool::ArduinoCli.binary(), Tool::ArduinoCli.install_hint()))
    }

    fn cli_command(&self, args: impl IntoIterator<Item = String>) -> Result<Command, String> {
        Ok(Command::new(self.cli()?.to_string_lossy(), args))
    }

    fn query(&self, args: &[&str]) -> Result<String, String> {
        process::output(self.cli()?, args, Some(&self.root))
    }

    fn sketch_arg(&self) -> String {
        self.root.to_string_lossy().into_owned()
    }
}

/// `fqbn` with `option` set to `value`, every other option kept:
/// `arduino:avr:nano` + cpu/atmega328old -> `arduino:avr:nano:cpu=atmega328old`.
pub fn with_option(fqbn: &str, option: &str, value: &str) -> String {
    let mut parts = fqbn.splitn(4, ':');
    let base: Vec<&str> = parts.by_ref().take(3).collect();
    let mut options: Vec<(String, String)> = parts
        .next()
        .unwrap_or_default()
        .split(',')
        .filter_map(|pair| pair.split_once('=').map(|(k, v)| (k.to_string(), v.to_string())))
        .collect();
    match options.iter_mut().find(|(k, _)| k == option) {
        Some(entry) => entry.1 = value.to_string(),
        None => options.push((option.to_string(), value.to_string())),
    }
    let options: Vec<String> = options.into_iter().map(|(k, v)| format!("{k}={v}")).collect();
    format!("{}:{}", base.join(":"), options.join(","))
}

/// Where `arduino-cli` keeps its configuration by default. The language
/// server insists on being told; the file needn't exist (the CLI falls
/// back to its defaults), so pointing at the default location keeps the
/// server in step with the CLI whether or not anyone has configured it.
fn cli_config_path() -> Option<PathBuf> {
    if cfg!(windows) {
        dirs::data_local_dir().map(|d| d.join("Arduino15").join("arduino-cli.yaml"))
    } else if cfg!(target_os = "macos") {
        dirs::home_dir().map(|d| d.join("Library").join("Arduino15").join("arduino-cli.yaml"))
    } else {
        dirs::home_dir().map(|d| d.join(".arduino15").join("arduino-cli.yaml"))
    }
}

/// Creates `parent/name/name.ino` (a starting sketch) and a `sketch.yaml`
/// naming `board`. Returns the `.ino`'s path.
pub fn create_sketch(parent: &Path, name: &str, board: &str) -> Result<PathBuf, String> {
    let valid = name.len() <= 63
        && name.chars().next().is_some_and(|c| c.is_ascii_alphanumeric())
        && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'));
    if !valid {
        return Err(format!("\"{name}\" isn't a valid sketch name: letters, digits, _ - and . only, starting with a letter or digit"));
    }
    let dir = parent.join(name);
    if dir.exists() {
        return Err(format!("{} already exists", dir.display()));
    }
    std::fs::create_dir_all(&dir).map_err(|err| format!("couldn't create {}: {err}", dir.display()))?;
    let ino = dir.join(format!("{name}.ino"));
    let template = format!(
        "void setup() {{\n  // put your setup code here, to run once:\n  Serial.begin({DEFAULT_BAUD});\n}}\n\nvoid loop() {{\n  // put your main code here, to run repeatedly:\n}}\n"
    );
    std::fs::write(&ino, template).map_err(|err| format!("couldn't write {}: {err}", ino.display()))?;
    sketch::set_yaml_value(&dir, "default_fqbn", board)?;
    Ok(ino)
}

impl Platform for Arduino {
    fn family(&self) -> &'static str {
        "Arduino"
    }

    fn root(&self) -> &Path {
        &self.root
    }

    fn board(&self) -> (String, bool) {
        match sketch::yaml_value(&self.root, "default_fqbn") {
            Some(fqbn) => (fqbn, true),
            None => (DEFAULT_BOARD.to_string(), false),
        }
    }

    fn port(&self) -> Option<String> {
        sketch::yaml_value(&self.root, "default_port")
    }

    fn baud_rate(&self) -> u32 {
        sketch::baud_rate(&self.root)
    }

    fn set_board(&self, board: &str) -> Result<(), String> {
        sketch::set_yaml_value(&self.root, "default_fqbn", board)
    }

    fn set_port(&self, port: &str) -> Result<(), String> {
        sketch::set_yaml_value(&self.root, "default_port", port)
    }

    fn set_baud_rate(&self, baud: u32) -> Result<(), String> {
        sketch::set_baud_rate(&self.root, baud)
    }

    fn build(&self) -> Result<Command, String> {
        let (board, _) = self.board();
        self.cli_command(["compile".to_string(), "--no-color".to_string(), "-b".to_string(), board, self.sketch_arg()])
    }

    fn upload(&self, port: &str) -> Result<Command, String> {
        let (board, _) = self.board();
        self.cli_command(
            ["compile", "--no-color", "--upload", "-p", port, "-b", &board, &self.sketch_arg()].map(str::to_string),
        )
    }

    fn monitor(&self, port: &str) -> Result<Command, String> {
        let (board, _) = self.board();
        let baud = format!("baudrate={}", self.baud_rate());
        self.cli_command(["monitor", "-p", port, "-b", &board, "--config", &baud].map(str::to_string))
    }

    fn language_server(&self) -> Result<Command, String> {
        let missing = |tool: Tool| format!("{} not found -- {}", tool.binary(), tool.install_hint());
        let server = self.language_server.as_deref().ok_or_else(|| missing(Tool::ArduinoLanguageServer))?;
        let clangd = self.clangd.as_deref().ok_or_else(|| missing(Tool::Clangd))?;
        let cli = self.cli()?;
        let config = cli_config_path().ok_or("couldn't work out where arduino-cli keeps its configuration")?;
        let (board, _) = self.board();
        Ok(Command::new(
            server.to_string_lossy(),
            [
                "-cli".to_string(),
                cli.to_string_lossy().into_owned(),
                "-cli-config".to_string(),
                config.to_string_lossy().into_owned(),
                "-clangd".to_string(),
                clangd.to_string_lossy().into_owned(),
                "-fqbn".to_string(),
                board,
            ],
        ))
    }

    fn debugging(&self, port: Option<&str>) -> Result<Debugging, String> {
        let (board, _) = self.board();
        let sketch = self.sketch_arg();
        let mut args = vec!["debug", "--info", "-b", &board, "--format", "json"];
        if let Some(port) = port {
            args.extend(["-p", port]);
        }
        args.push(&sketch);
        match self.query(&args) {
            Ok(_) => {
                let port = port.ok_or("pick the port your board and debug probe are on first")?;
                Ok(Debugging::Supported {
                    prepare: self.cli_command(
                        ["compile", "--no-color", "--optimize-for-debug", "--upload", "-p", port, "-b", &board, &sketch].map(str::to_string),
                    )?,
                    session: self.cli_command(["debug", "-p", port, "-b", &board, &sketch].map(str::to_string))?,
                })
            }
            Err(reason) if reason.contains("not supported") => Ok(Debugging::Unsupported(format!(
                "{board} can't be debugged with breakpoints ({reason}). Print with Serial.println and watch the serial monitor instead."
            ))),
            Err(reason) => Err(reason),
        }
    }

    fn ports(&self) -> Result<Vec<Port>, String> {
        parse::ports(&self.query(&["board", "list", "--format", "json"])?)
    }

    fn boards(&self) -> Result<Vec<Board>, String> {
        parse::boards(&self.query(&["board", "listall", "--format", "json"])?)
    }

    fn board_options(&self, board: &str) -> Result<Vec<BoardOption>, String> {
        parse::board_options(&self.query(&["board", "details", "-b", board, "--format", "json"])?)
    }

    fn board_with_option(&self, board: &str, option: &str, value: &str) -> String {
        with_option(board, option, value)
    }

    fn search_libraries(&self, query: &str) -> Result<Vec<Library>, String> {
        let mut args = vec!["lib", "search", "--omit-releases-details", "--format", "json"];
        if !query.trim().is_empty() {
            args.push(query.trim());
        }
        let found = self.query(&args)?;
        let installed = self.query(&["lib", "list", "--format", "json"])?;
        parse::libraries(&found, &installed)
    }

    fn install_library(&self, name: &str) -> Result<Command, String> {
        self.cli_command(["lib", "install", "--no-color", name].map(str::to_string))
    }

    fn search_packages(&self, query: &str) -> Result<Vec<Package>, String> {
        let mut args = vec!["core", "search", "--format", "json"];
        if !query.trim().is_empty() {
            args.push(query.trim());
        }
        parse::packages(&self.query(&args)?)
    }

    fn install_package(&self, id: &str) -> Result<Command, String> {
        self.cli_command(["core", "install", "--no-color", id].map(str::to_string))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::TempDir;

    fn tools() -> Tools {
        Tools {
            arduino_cli: Some(PathBuf::from("arduino-cli")),
            clangd: Some(PathBuf::from("clangd")),
            arduino_language_server: Some(PathBuf::from("arduino-language-server")),
        }
    }

    fn sketch(dir: &TempDir, name: &str) -> PathBuf {
        let root = dir.path().join(name);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join(format!("{name}.ino")), "void setup() {}\nvoid loop() {}\n").unwrap();
        root
    }

    #[test]
    fn a_folder_is_a_sketch_when_its_ino_matches_its_name_or_it_has_a_sketch_yaml() {
        let dir = TempDir::new("is_sketch");
        assert!(is_sketch(&sketch(&dir, "Blink")));

        let loose = dir.path().join("Loose");
        std::fs::create_dir_all(&loose).unwrap();
        std::fs::write(loose.join("other.ino"), "").unwrap();
        assert!(!is_sketch(&loose), "the main .ino must be named after its folder");
        std::fs::write(loose.join("sketch.yaml"), "default_fqbn: arduino:avr:uno\n").unwrap();
        assert!(is_sketch(&loose));
    }

    #[test]
    fn an_unconfigured_sketch_builds_for_the_uno_and_says_so() {
        let dir = TempDir::new("default_board");
        let arduino = Arduino::detect(&sketch(&dir, "Blink"), &tools()).unwrap();
        assert_eq!(arduino.board(), (DEFAULT_BOARD.to_string(), false));
        assert_eq!(arduino.port(), None);
        assert_eq!(arduino.baud_rate(), 9600);

        arduino.set_board("arduino:avr:nano:cpu=atmega328old").unwrap();
        arduino.set_port("COM4").unwrap();
        assert_eq!(arduino.board(), ("arduino:avr:nano:cpu=atmega328old".to_string(), true));
        assert_eq!(arduino.port().as_deref(), Some("COM4"));
    }

    #[test]
    fn build_upload_and_monitor_commands_use_the_sketchs_board_and_port() {
        let dir = TempDir::new("commands");
        let root = sketch(&dir, "Blink");
        let arduino = Arduino::detect(&root, &tools()).unwrap();
        arduino.set_board("arduino:avr:nano").unwrap();
        arduino.set_baud_rate(115200).unwrap();
        let sketch = root.to_string_lossy().into_owned();

        assert_eq!(arduino.build().unwrap(), Command::new("arduino-cli", ["compile", "--no-color", "-b", "arduino:avr:nano", &sketch]));
        assert_eq!(
            arduino.upload("COM4").unwrap(),
            Command::new("arduino-cli", ["compile", "--no-color", "--upload", "-p", "COM4", "-b", "arduino:avr:nano", &sketch])
        );
        assert_eq!(
            arduino.monitor("COM4").unwrap(),
            Command::new("arduino-cli", ["monitor", "-p", "COM4", "-b", "arduino:avr:nano", "--config", "baudrate=115200"])
        );
    }

    #[test]
    fn the_language_server_is_given_the_cli_its_config_clangd_and_the_board() {
        let dir = TempDir::new("lsp");
        let arduino = Arduino::detect(&sketch(&dir, "Blink"), &tools()).unwrap();
        let command = arduino.language_server().unwrap();
        assert_eq!(command.program, "arduino-language-server");
        assert_eq!(&command.args[..2], ["-cli", "arduino-cli"]);
        assert_eq!(command.args[2], "-cli-config");
        assert!(command.args[3].ends_with("arduino-cli.yaml"));
        assert_eq!(&command.args[4..], ["-clangd", "clangd", "-fqbn", DEFAULT_BOARD]);
    }

    #[test]
    fn missing_tools_are_reported_with_a_hint() {
        let dir = TempDir::new("missing");
        let arduino = Arduino::detect(&sketch(&dir, "Blink"), &Tools::default()).unwrap();
        assert!(arduino.build().unwrap_err().contains("arduino-cli not found"));
        assert!(arduino.language_server().unwrap_err().contains("arduino-language-server not found"));
    }

    #[test]
    fn with_option_adds_or_replaces_one_option() {
        assert_eq!(with_option("arduino:avr:nano", "cpu", "atmega328old"), "arduino:avr:nano:cpu=atmega328old");
        assert_eq!(with_option("arduino:avr:nano:cpu=atmega328", "cpu", "atmega328old"), "arduino:avr:nano:cpu=atmega328old");
        assert_eq!(with_option("esp32:esp32:esp32:PSRAM=disabled", "FlashSize", "4M"), "esp32:esp32:esp32:PSRAM=disabled,FlashSize=4M");
    }

    #[test]
    fn a_new_sketch_is_a_folder_with_a_matching_ino_and_its_board() {
        let dir = TempDir::new("new_sketch");
        let ino = create_sketch(dir.path(), "Lab1", "arduino:avr:uno").unwrap();
        assert_eq!(ino, dir.path().join("Lab1").join("Lab1.ino"));
        assert!(std::fs::read_to_string(&ino).unwrap().contains("Serial.begin(9600);"));
        let arduino = Arduino::detect(&dir.path().join("Lab1"), &tools()).unwrap();
        assert_eq!(arduino.board(), ("arduino:avr:uno".to_string(), true));

        assert!(create_sketch(dir.path(), "Lab1", "arduino:avr:uno").unwrap_err().contains("already exists"));
        assert!(create_sketch(dir.path(), "lab one", "arduino:avr:uno").unwrap_err().contains("isn't a valid sketch name"));
    }
}
