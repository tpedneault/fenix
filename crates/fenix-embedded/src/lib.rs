//! Microcontroller projects: recognizing one, and knowing how to build,
//! flash, monitor, debug and understand it -- as commands for `fenix-gui`
//! to run, never a thread or a window of its own (the same split
//! `fenix-docker`/`fenix-git` already use).
//!
//! Everything a toolchain-specific integration has to answer is the
//! `Platform` trait. Arduino (`arduino`, driving `arduino-cli`) is the
//! only implementation today; another family -- STM32 through its own
//! CLI, say -- is a second implementation plus one line in `detect`, and
//! the editor side (keys, pickers, the serial monitor, the task runner,
//! the language server) works for it unchanged.

pub mod arduino;
mod process;
mod tools;

use std::path::Path;

pub use tools::{Tool, ToolOverrides, Tools};

/// A program and its arguments, run with the project root as its working
/// directory -- no shell, so nothing here needs quoting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    pub program: String,
    pub args: Vec<String>,
}

impl Command {
    pub fn new(program: impl Into<String>, args: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self { program: program.into(), args: args.into_iter().map(Into::into).collect() }
    }

    /// `program arg arg ...`, for the task panel's header line.
    pub fn display(&self) -> String {
        std::iter::once(self.program.as_str()).chain(self.args.iter().map(String::as_str)).collect::<Vec<_>>().join(" ")
    }
}

/// A board the project can be built for. `id` is whatever the platform
/// calls it (an Arduino FQBN such as `arduino:avr:uno`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Board {
    pub id: String,
    pub name: String,
    /// Which installed package the board comes from, for grouping.
    pub package: String,
}

/// Something a board can be plugged into -- a serial port, usually.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Port {
    /// What to pass back to the platform: `COM3`, `/dev/ttyACM0`.
    pub address: String,
    /// What to show: the platform's own label for the port.
    pub label: String,
    pub protocol: String,
    /// Boards recognized on this port, when the platform can tell.
    pub boards: Vec<Board>,
}

/// One menu of a board's build options (an Arduino Nano's "Processor",
/// say), with every value and which one is in effect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoardOption {
    pub id: String,
    pub label: String,
    pub values: Vec<OptionValue>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OptionValue {
    pub value: String,
    pub label: String,
    pub selected: bool,
}

/// A library the platform can install.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Library {
    pub name: String,
    pub latest: String,
    pub author: String,
    pub summary: String,
    /// The installed version, if any.
    pub installed: Option<String>,
}

/// A package of board support (an Arduino "core", such as `arduino:avr`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Package {
    pub id: String,
    pub name: String,
    pub latest: String,
    pub installed: Option<String>,
}

/// Whether the current board can be debugged with breakpoints, and how.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Debugging {
    /// It can't, and why -- a message worth showing as is.
    Unsupported(String),
    /// Run `prepare` (a debug build, flashed to the board), then
    /// `session` interactively (a GDB console attached to the board).
    Supported { prepare: Command, session: Command },
}

/// Everything the editor needs from one toolchain. Methods that return a
/// `Command` only describe work; methods that return data run the
/// toolchain and block, so the editor calls them off its UI thread.
pub trait Platform: Send + Sync {
    /// The family's name, for messages: "Arduino".
    fn family(&self) -> &'static str;

    fn root(&self) -> &Path;

    /// The board in effect, and whether it was chosen for this project
    /// (`false` means a default is standing in).
    fn board(&self) -> (String, bool);

    /// The port uploads and the monitor use, if one is set.
    fn port(&self) -> Option<String>;

    /// The serial monitor's speed.
    fn baud_rate(&self) -> u32;

    /// Saves the project's board (a full id, options included).
    fn set_board(&self, board: &str) -> Result<(), String>;

    fn set_port(&self, port: &str) -> Result<(), String>;

    fn set_baud_rate(&self, baud: u32) -> Result<(), String>;

    fn build(&self) -> Result<Command, String>;

    /// Build and flash to `port`.
    fn upload(&self, port: &str) -> Result<Command, String>;

    /// A serial monitor on `port`, for a terminal.
    fn monitor(&self, port: &str) -> Result<Command, String>;

    /// The language server for this project's C/C++ sources.
    fn language_server(&self) -> Result<Command, String>;

    /// Blocks.
    fn debugging(&self, port: Option<&str>) -> Result<Debugging, String>;

    /// Blocks.
    fn ports(&self) -> Result<Vec<Port>, String>;

    /// Every board of every installed package. Blocks.
    fn boards(&self) -> Result<Vec<Board>, String>;

    /// `board`'s option menus, with its current values marked. Blocks.
    fn board_options(&self, board: &str) -> Result<Vec<BoardOption>, String>;

    /// `board` with one option menu set to `value`, the rest kept.
    fn board_with_option(&self, board: &str, option: &str, value: &str) -> String;

    /// Blocks.
    fn search_libraries(&self, query: &str) -> Result<Vec<Library>, String>;

    fn install_library(&self, name: &str) -> Result<Command, String>;

    /// Blocks.
    fn search_packages(&self, query: &str) -> Result<Vec<Package>, String>;

    fn install_package(&self, id: &str) -> Result<Command, String>;
}

/// The embedded project rooted exactly at `root`, if it is one.
pub fn detect(root: &Path, tools: &Tools) -> Option<Box<dyn Platform>> {
    arduino::Arduino::detect(root, tools).map(|p| Box::new(p) as Box<dyn Platform>)
}

/// The embedded project `path` (a file or directory) belongs to: the
/// closest enclosing directory that is one.
pub fn project_root_of(path: &Path) -> Option<std::path::PathBuf> {
    let start = if path.is_dir() { path } else { path.parent()? };
    start.ancestors().find(|dir| arduino::is_sketch(dir)).map(Path::to_path_buf)
}

#[cfg(test)]
pub(crate) mod test_util {
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    pub struct TempDir(PathBuf);

    impl TempDir {
        pub fn new(name: &str) -> Self {
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!("fenix-embedded-test-{name}-{}-{n}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        pub fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::TempDir;

    #[test]
    fn a_file_inside_a_sketch_finds_the_sketch_root() {
        let dir = TempDir::new("root_of");
        let sketch = dir.path().join("Blink");
        std::fs::create_dir_all(sketch.join("src")).unwrap();
        std::fs::write(sketch.join("Blink.ino"), "").unwrap();
        std::fs::write(sketch.join("src").join("util.cpp"), "").unwrap();

        assert_eq!(project_root_of(&sketch.join("src").join("util.cpp")), Some(sketch.clone()));
        assert_eq!(project_root_of(&sketch.join("Blink.ino")), Some(sketch));
        assert_eq!(project_root_of(dir.path()), None);
    }

    #[test]
    fn command_display_joins_program_and_arguments() {
        assert_eq!(Command::new("arduino-cli", ["compile", "-b", "arduino:avr:uno"]).display(), "arduino-cli compile -b arduino:avr:uno");
    }
}
