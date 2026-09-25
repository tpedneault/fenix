//! Finding the programs an integration drives. Each is looked for, in
//! order: a path set in `config.ini`, Fenix's own tools folder
//! (`<config dir>/fenix/tools`, where Fenix-managed copies live), `PATH`,
//! then the places its usual installer puts it -- including inside an
//! Arduino IDE 2 install, which bundles its own language server and
//! clangd.

use std::path::{Path, PathBuf};

/// A program an integration can need.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    ArduinoCli,
    Clangd,
    ArduinoLanguageServer,
}

impl Tool {
    /// The executable's base name, without `.exe`.
    pub fn binary(self) -> &'static str {
        match self {
            Tool::ArduinoCli => "arduino-cli",
            Tool::Clangd => "clangd",
            Tool::ArduinoLanguageServer => "arduino-language-server",
        }
    }

    /// What to tell someone who doesn't have it.
    pub fn install_hint(self) -> &'static str {
        match self {
            Tool::ArduinoCli => "install Arduino CLI (https://arduino.github.io/arduino-cli/) or set arduino_cli in the [embedded] section of config.ini",
            Tool::Clangd | Tool::ArduinoLanguageServer => {
                "autocompletion needs clangd and arduino-language-server: put them in Fenix's tools folder, on PATH, or set clangd/arduino_language_server in [embedded]"
            }
        }
    }
}

/// Paths from `config.ini`'s `[embedded]` section, each overriding the
/// search for its tool.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolOverrides {
    pub arduino_cli: Option<PathBuf>,
    pub clangd: Option<PathBuf>,
    pub arduino_language_server: Option<PathBuf>,
}

/// Where each tool was found, if anywhere.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Tools {
    pub arduino_cli: Option<PathBuf>,
    pub clangd: Option<PathBuf>,
    pub arduino_language_server: Option<PathBuf>,
}

impl Tools {
    pub fn discover(overrides: &ToolOverrides) -> Self {
        Self::discover_in(overrides, fenix_tools_dir().as_deref(), &search_path(), &install_locations())
    }

    fn discover_in(overrides: &ToolOverrides, fenix_dir: Option<&Path>, path: &[PathBuf], installs: &[(Tool, PathBuf)]) -> Self {
        let find = |tool: Tool, over: &Option<PathBuf>| -> Option<PathBuf> {
            if let Some(configured) = over {
                return configured.is_file().then(|| configured.clone());
            }
            fenix_dir
                .and_then(|dir| find_below(&dir.join(tool.binary()), &exe_name(tool.binary()), 3))
                .or_else(|| path.iter().map(|dir| dir.join(exe_name(tool.binary()))).find(|p| p.is_file()))
                .or_else(|| installs.iter().filter(|(t, _)| *t == tool).map(|(_, p)| p.clone()).find(|p| p.is_file()))
        };
        Self {
            arduino_cli: find(Tool::ArduinoCli, &overrides.arduino_cli),
            clangd: find(Tool::Clangd, &overrides.clangd),
            arduino_language_server: find(Tool::ArduinoLanguageServer, &overrides.arduino_language_server),
        }
    }

    pub fn get(&self, tool: Tool) -> Option<&Path> {
        match tool {
            Tool::ArduinoCli => self.arduino_cli.as_deref(),
            Tool::Clangd => self.clangd.as_deref(),
            Tool::ArduinoLanguageServer => self.arduino_language_server.as_deref(),
        }
    }

    /// `tool`'s path, or an error saying how to get it.
    pub fn require(&self, tool: Tool) -> Result<&Path, String> {
        self.get(tool).ok_or_else(|| format!("{} not found -- {}", tool.binary(), tool.install_hint()))
    }
}

/// `tools`, with this machine's state: downloads, so they don't roam.
pub fn fenix_tools_dir() -> Option<PathBuf> {
    fenix_storage::paths::tools_dir()
}

fn exe_name(base: &str) -> String {
    if cfg!(windows) {
        format!("{base}.exe")
    } else {
        base.to_string()
    }
}

/// The first file called `name` at most `depth` directories below `dir`
/// -- release archives unpack into a versioned subfolder
/// (`clangd/clang_Windows_64bit/clangd.exe`).
fn find_below(dir: &Path, name: &str, depth: usize) -> Option<PathBuf> {
    let direct = dir.join(name);
    if direct.is_file() {
        return Some(direct);
    }
    if depth == 0 {
        return None;
    }
    let mut children: Vec<PathBuf> = std::fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
    children.sort();
    children.into_iter().find_map(|child| find_below(&child, name, depth - 1))
}

fn search_path() -> Vec<PathBuf> {
    std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default()
}

/// Where official installers put each tool.
fn install_locations() -> Vec<(Tool, PathBuf)> {
    let mut out = Vec::new();
    if cfg!(windows) {
        for var in ["ProgramFiles", "ProgramFiles(x86)"] {
            if let Some(dir) = std::env::var_os(var) {
                out.push((Tool::ArduinoCli, PathBuf::from(dir).join("Arduino CLI").join("arduino-cli.exe")));
            }
        }
        if let Some(local) = dirs::data_local_dir() {
            out.push((Tool::ArduinoCli, local.join("Programs").join("Arduino CLI").join("arduino-cli.exe")));
            let ide = local.join("Programs").join("Arduino IDE").join("resources").join("app").join("lib").join("backend").join("resources");
            out.push((Tool::ArduinoCli, ide.join("arduino-cli.exe")));
            out.push((Tool::Clangd, ide.join("clangd.exe")));
            out.push((Tool::ArduinoLanguageServer, ide.join("arduino-language-server.exe")));
        }
    } else {
        for dir in ["/usr/local/bin", "/opt/homebrew/bin"] {
            out.push((Tool::ArduinoCli, PathBuf::from(dir).join("arduino-cli")));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::TempDir;

    fn touch(path: &Path) -> PathBuf {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "").unwrap();
        path.to_path_buf()
    }

    #[test]
    fn a_configured_path_wins_and_a_missing_one_finds_nothing() {
        let dir = TempDir::new("override");
        let mine = touch(&dir.path().join("my-cli").join(exe_name("arduino-cli")));
        let on_path = touch(&dir.path().join("bin").join(exe_name("arduino-cli")));
        let path = vec![on_path.parent().unwrap().to_path_buf()];

        let overrides = ToolOverrides { arduino_cli: Some(mine.clone()), ..Default::default() };
        assert_eq!(Tools::discover_in(&overrides, None, &path, &[]).arduino_cli, Some(mine));

        let overrides = ToolOverrides { arduino_cli: Some(dir.path().join("nope")), ..Default::default() };
        assert_eq!(Tools::discover_in(&overrides, None, &path, &[]).arduino_cli, None, "a wrong setting is reported, not silently replaced");
    }

    #[test]
    fn fenix_managed_copies_are_found_in_a_versioned_subfolder_before_path() {
        let dir = TempDir::new("managed");
        let tools = dir.path().join("tools");
        let managed = touch(&tools.join("clangd").join("clang_Windows_64bit").join(exe_name("clangd")));
        let on_path = touch(&dir.path().join("bin").join(exe_name("clangd")));

        let found = Tools::discover_in(&ToolOverrides::default(), Some(&tools), &[on_path.parent().unwrap().to_path_buf()], &[]);
        assert_eq!(found.clangd, Some(managed));
    }

    #[test]
    fn install_locations_are_the_last_resort() {
        let dir = TempDir::new("installs");
        let ide = touch(&dir.path().join("ide").join(exe_name("arduino-language-server")));
        let found = Tools::discover_in(&ToolOverrides::default(), None, &[], &[(Tool::ArduinoLanguageServer, ide.clone())]);
        assert_eq!(found.arduino_language_server, Some(ide));
        assert!(found.require(Tool::Clangd).unwrap_err().contains("autocompletion needs clangd"));
    }
}
