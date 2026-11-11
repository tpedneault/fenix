//! How each language server Fenix starts by default gets installed: the
//! one table the doctor's fixes (`SPC p h`), Tool Status (`SPC l m`) and
//! the install offer a buffer gets when its server is missing (`SPC l i`)
//! all read, so they never disagree (#43).
//!
//! Only installers that need no administrator rights and put the server
//! where Fenix looks are used: `rustup`, `npm`, `go install`, `uv tool`,
//! and on Windows `winget`. Where none of those is there, or none exists
//! for the server on this system (clangd on Linux comes from the
//! distribution, which needs `sudo`), the answer is what to do by hand and
//! where to read about it. Installing is never one of the doctor's safe
//! fixes: it always waits for you to say so.

use std::path::PathBuf;

use crate::template::Step;

/// The system an install is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Windows,
    Unix,
}

impl Os {
    pub fn current() -> Os {
        if cfg!(windows) {
            Os::Windows
        } else {
            Os::Unix
        }
    }
}

/// How to get a server.
#[derive(Debug, Clone)]
pub enum ServerInstall {
    /// A command that installs it, run where you can watch it.
    Run(Step),
    /// No command Fenix would run for you here: what to do, and where
    /// to read more.
    Manual { how: String, url: &'static str },
}

impl ServerInstall {
    /// One line: the command, or what to do and the link.
    pub fn hint(&self) -> String {
        match self {
            ServerInstall::Run(step) => step.display(),
            ServerInstall::Manual { how, url } => format!("{how} -- {url}"),
        }
    }

    pub fn step(&self) -> Option<&Step> {
        match self {
            ServerInstall::Run(step) => Some(step),
            ServerInstall::Manual { .. } => None,
        }
    }
}

/// One way to install: the installer it needs, and its arguments.
struct Recipe {
    installer: &'static str,
    args: &'static [&'static str],
    /// Only on this system; `None` on both.
    only: Option<Os>,
}

const fn on(installer: &'static str, args: &'static [&'static str]) -> Recipe {
    Recipe { installer, args, only: None }
}

const fn windows(installer: &'static str, args: &'static [&'static str]) -> Recipe {
    Recipe { installer, args, only: Some(Os::Windows) }
}

/// What a server's row says when no installer it needs is there.
struct Server {
    program: &'static str,
    recipes: &'static [Recipe],
    /// What to do by hand, on Windows and elsewhere.
    manual_windows: &'static str,
    manual_unix: &'static str,
    url: &'static str,
}

/// The servers `lsp::default_server_command` starts, in its order.
const SERVERS: &[Server] = &[
    Server {
        program: "pyright-langserver",
        recipes: &[on("uv", &["tool", "install", "pyright"]), on("npm", &["install", "-g", "pyright"])],
        manual_windows: "install uv (or Node.js), then: uv tool install pyright",
        manual_unix: "install uv (or Node.js), then: uv tool install pyright",
        url: "https://microsoft.github.io/pyright/#/installation",
    },
    Server {
        program: "rust-analyzer",
        recipes: &[on("rustup", &["component", "add", "rust-analyzer"])],
        manual_windows: "install Rust with rustup, then: rustup component add rust-analyzer",
        manual_unix: "install Rust with rustup, then: rustup component add rust-analyzer",
        url: "https://rust-analyzer.github.io/book/installation.html",
    },
    Server {
        program: "clangd",
        recipes: &[windows("winget", &["install", "--id", "LLVM.LLVM", "--exact", "--accept-source-agreements", "--accept-package-agreements"])],
        manual_windows: "install LLVM (it has clangd) from its installer",
        manual_unix: "install it with your package manager (sudo apt install clangd, sudo dnf install clang-tools-extra, sudo pacman -S clang)",
        url: "https://clangd.llvm.org/installation",
    },
    Server {
        program: "bash-language-server",
        recipes: &[on("npm", &["install", "-g", "bash-language-server"])],
        manual_windows: "install Node.js, then: npm install -g bash-language-server",
        manual_unix: "install Node.js, then: npm install -g bash-language-server",
        url: "https://github.com/bash-lsp/bash-language-server#installation",
    },
    Server {
        program: "typescript-language-server",
        recipes: &[on("npm", &["install", "-g", "typescript-language-server", "typescript"])],
        manual_windows: "install Node.js, then: npm install -g typescript-language-server typescript",
        manual_unix: "install Node.js, then: npm install -g typescript-language-server typescript",
        url: "https://github.com/typescript-language-server/typescript-language-server#installing",
    },
    Server {
        program: "gopls",
        recipes: &[on("go", &["install", "golang.org/x/tools/gopls@latest"])],
        manual_windows: "install Go, then: go install golang.org/x/tools/gopls@latest",
        manual_unix: "install Go, then: go install golang.org/x/tools/gopls@latest",
        url: "https://go.dev/gopls/#installation",
    },
    Server {
        program: "lua-language-server",
        recipes: &[windows("winget", &["install", "--id", "LuaLS.lua-language-server", "--exact", "--accept-source-agreements", "--accept-package-agreements"])],
        manual_windows: "download a release and put its bin folder on PATH",
        manual_unix: "install it with your package manager, or download a release and put its bin folder on PATH",
        url: "https://luals.github.io/#other-install",
    },
];

/// Every server this table knows, by program name.
pub fn known_servers() -> impl Iterator<Item = &'static str> {
    SERVERS.iter().map(|s| s.program)
}

/// How to install `program` (a language server's program name, as
/// `lsp::default_server_command` gives it) on `os`, where `locate`
/// finds the installers: the first way whose installer is there, else
/// what to do by hand. `None` for a program the table doesn't know.
pub fn server_install_on(program: &str, os: Os, locate: impl Fn(&str) -> Option<PathBuf>) -> Option<ServerInstall> {
    let server = SERVERS.iter().find(|s| s.program == program)?;
    let found = server.recipes.iter().filter(|r| r.only.is_none_or(|only| only == os)).find(|r| locate(r.installer).is_some());
    Some(match found {
        Some(recipe) => ServerInstall::Run(Step::new(recipe.installer, recipe.args)),
        None => ServerInstall::Manual { how: if os == Os::Windows { server.manual_windows } else { server.manual_unix }.to_string(), url: server.url },
    })
}

/// `server_install_on` this system, with PATH searched for installers.
pub fn server_install(program: &str) -> Option<ServerInstall> {
    server_install_on(program, Os::current(), crate::doctor::which)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn having(programs: &'static [&'static str]) -> impl Fn(&str) -> Option<PathBuf> {
        move |p| programs.contains(&p).then(|| PathBuf::from(format!("/bin/{p}")))
    }

    fn command(install: Option<ServerInstall>) -> Option<String> {
        install.and_then(|i| i.step().map(Step::display))
    }

    /// Every server Fenix starts by default has a way to get it on both
    /// systems: a command where its installer is there, else a link.
    #[test]
    fn the_seven_default_servers_each_have_an_install_on_both_systems() {
        let programs: Vec<_> = known_servers().collect();
        for program in ["pyright-langserver", "rust-analyzer", "clangd", "bash-language-server", "typescript-language-server", "gopls", "lua-language-server"] {
            assert!(programs.contains(&program), "{program}");
            for os in [Os::Windows, Os::Unix] {
                match server_install_on(program, os, having(&[])) {
                    Some(ServerInstall::Manual { how, url }) => assert!(!how.is_empty() && url.starts_with("https://"), "{program}"),
                    other => panic!("{program} on {os:?} with nothing installed: {other:?}"),
                }
            }
        }
        assert!(server_install_on("kiln", Os::Unix, having(&["npm"])).is_none());
    }

    #[test]
    fn the_command_is_the_first_whose_installer_is_there() {
        assert_eq!(command(server_install_on("rust-analyzer", Os::Unix, having(&["rustup"]))).as_deref(), Some("rustup component add rust-analyzer"));
        assert_eq!(command(server_install_on("pyright-langserver", Os::Windows, having(&["uv", "npm"]))).as_deref(), Some("uv tool install pyright"));
        assert_eq!(command(server_install_on("pyright-langserver", Os::Unix, having(&["npm"]))).as_deref(), Some("npm install -g pyright"));
        assert_eq!(command(server_install_on("gopls", Os::Unix, having(&["go"]))).as_deref(), Some("go install golang.org/x/tools/gopls@latest"));
        assert_eq!(
            command(server_install_on("typescript-language-server", Os::Windows, having(&["npm"]))).as_deref(),
            Some("npm install -g typescript-language-server typescript")
        );
        assert_eq!(command(server_install_on("bash-language-server", Os::Unix, having(&["npm"]))).as_deref(), Some("npm install -g bash-language-server"));
    }

    /// winget is Windows' only; on Linux clangd and LuaLS come from the
    /// distribution, which Fenix never runs `sudo` for.
    #[test]
    fn what_needs_the_system_package_manager_is_left_to_you() {
        assert!(command(server_install_on("clangd", Os::Windows, having(&["winget"]))).is_some_and(|c| c.starts_with("winget install --id LLVM.LLVM")));
        assert!(command(server_install_on("lua-language-server", Os::Windows, having(&["winget"]))).is_some_and(|c| c.contains("LuaLS.lua-language-server")));
        let clangd = server_install_on("clangd", Os::Unix, having(&["winget", "apt"])).unwrap();
        assert!(clangd.step().is_none());
        assert!(clangd.hint().contains("sudo apt install clangd") && clangd.hint().contains("https://clangd.llvm.org"), "{}", clangd.hint());
        assert!(server_install_on("lua-language-server", Os::Unix, having(&["winget"])).unwrap().step().is_none());
    }
}
