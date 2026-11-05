//! `SPC l m` -- a detect-and-guide tool status listing (Milestone E of
//! the LSP/DAP/tool-manager plan): for every language with a known LSP
//! server or DAP adapter, whether the command that would actually be
//! launched (a `[lsp]`/`.fenix/project.ini` override if configured,
//! else `lsp::default_server_command`/`dap::default_adapter_command`)
//! is found on `PATH`, whether a session for it is currently running,
//! and -- when it's missing -- the one-line command that installs it.
//! Deliberately never downloads or manages a binary itself: this is
//! "detect and guide," the scope decision locked in before any of this
//! milestone's work started (see the plan's own "Tool manager scope"
//! note) -- Fenix shows what's there and how to get what isn't, full
//! stop.

use fenix_syntax::LanguageId;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolKind {
    Lsp,
    Dap,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolEntry {
    pub language: LanguageId,
    pub kind: ToolKind,
    /// The program name (or path) that would actually be launched --
    /// already resolved through any configured override, exactly what
    /// `ensure_lsp_session`/`debug_start_or_continue` themselves use.
    pub command: String,
    pub found_on_path: bool,
    pub running: bool,
    /// `None` when `found_on_path` is true -- nothing to suggest
    /// installing.
    pub install_hint: Option<&'static str>,
    /// Where it's installed when that isn't on Fenix's `PATH` (`uv tool
    /// install`'s folder), as the doctor finds it (#43).
    pub elsewhere: Option<std::path::PathBuf>,
}

/// A row's state, from what was found: running means found, whatever
/// `PATH` says; one installed off `PATH` says where; one not found at all
/// how to install it.
fn entry(language: LanguageId, kind: ToolKind, command: String, running: bool) -> ToolEntry {
    let found_on_path = running || is_on_path(&command);
    let elsewhere = (!found_on_path).then(|| fenix_project::doctor::in_tool_dirs(&command)).flatten();
    let install_hint = if found_on_path || elsewhere.is_some() { None } else { install_hint_for(&command) };
    ToolEntry { language, kind, command, found_on_path, running, install_hint, elsewhere }
}

/// Every language this app has a built-in LSP default or DAP default
/// for -- the only ones worth a row here at all. A language with
/// neither (TOML, JSON, Markdown, ...) has nothing to detect or
/// install, so it's not listed, the same "no support for this, not a
/// gap" posture `dap::default_adapter_command`'s own doc comment
/// already takes for languages beyond Python.
const KNOWN_LANGUAGES: &[LanguageId] =
    &[
        LanguageId::Python,
        LanguageId::Rust,
        LanguageId::C,
        LanguageId::Cpp,
        LanguageId::Bash,
        LanguageId::JavaScript,
        LanguageId::TypeScript,
        LanguageId::Tsx,
        LanguageId::Go,
        LanguageId::Lua,
    ];

/// The one-line command that installs `command` for `language` -- shown
/// only when `command` isn't found on `PATH`. Keyed by the *command
/// name itself*, not the language, since C and C++ share `clangd` and
/// JavaScript/TypeScript/TSX share `typescript-language-server` -- one
/// hint, not a near-duplicate per language.
fn install_hint_for(command: &str) -> Option<&'static str> {
    match command {
        "pyright-langserver" => Some("uv tool install pyright  (or: pip install pyright)"),
        "python" => Some("install Python from https://python.org, or via your platform's package manager"),
        "rust-analyzer" => Some("rustup component add rust-analyzer"),
        "clangd" => Some("install clangd via your platform's package manager, or the LLVM installer on Windows"),
        "bash-language-server" => Some("npm install -g bash-language-server"),
        "typescript-language-server" => Some("npm install -g typescript-language-server typescript"),
        "gopls" => Some("go install golang.org/x/tools/gopls@latest"),
        "lua-language-server" => Some("install lua-language-server via your platform's package manager, or from https://luals.github.io"),
        "lldb-dap" => Some("install LLVM (lldb-dap comes with it), or rustup's lldb on macOS"),
        _ => None,
    }
}

/// One row per `(language, LSP default or override)` and, for Python
/// only, its DAP adapter too (the one language this app has a built-in
/// debug adapter for -- see `dap::default_adapter_command`'s own doc
/// comment). `configured_lsp`/`debugpy_installed_hint` isn't looked up
/// here -- `command` already reflects whatever `lsp::resolve_server_
/// command`/`dap::default_adapter_command` would actually resolve to,
/// computed by the caller (`App::tool_status_entries`) since only it
/// has `Config::lsp_servers` and the currently-running sessions to
/// check against.
pub fn scan(
    resolve_lsp_command: impl Fn(LanguageId) -> Option<(String, Vec<String>)>,
    resolve_dap_command: impl Fn(LanguageId) -> Option<(String, Vec<String>)>,
    lsp_running: impl Fn(LanguageId) -> bool,
    dap_running: impl Fn(LanguageId) -> bool,
) -> Vec<ToolEntry> {
    let mut entries = Vec::new();
    for &language in KNOWN_LANGUAGES {
        if let Some((command, _)) = resolve_lsp_command(language) {
            entries.push(entry(language, ToolKind::Lsp, command, lsp_running(language)));
        }
        if let Some((command, _)) = resolve_dap_command(language) {
            entries.push(entry(language, ToolKind::Dap, command, dap_running(language)));
        }
    }
    entries
}

/// Whether `command` resolves to a real, existing file -- a pure
/// filesystem check, deliberately *not* spawning the binary the way
/// `fenix_docker::engine::probe` does for `docker --version` (a well-
/// behaved, quick-exiting CLI tool). An LSP server/DAP adapter has no
/// such guarantee: many only speak JSON-RPC on stdin and would just
/// hang waiting for a handshake that never comes if run bare, which
/// would stall this whole scan on the first misbehaving one. Reuses
/// the exact same `PATH`/`PATHEXT` search `fenix_rpc::resolve_command`
/// already does for actually launching a server (see its own doc
/// comment) -- this just checks existence instead of returning the
/// resolved path, and (unlike that Windows-only function) also handles
/// Unix, where a plain `PATH` walk with no extension search is enough.
pub(crate) fn is_on_path(command: &str) -> bool {
    let path = Path::new(command);
    if path.is_absolute() || command.contains(['/', '\\']) {
        return path.is_file();
    }
    let Some(path_var) = std::env::var_os("PATH") else { return false };
    let extensions: Vec<String> = if cfg!(windows) {
        std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_string()).split(';').filter(|e| !e.is_empty()).map(str::to_string).collect()
    } else {
        vec![String::new()]
    };
    for dir in std::env::split_paths(&path_var) {
        for ext in &extensions {
            if dir.join(format!("{command}{ext}")).is_file() {
                return true;
            }
        }
    }
    false
}

/// One `LANGUAGE  KIND  command  [found/missing]  [running]` row per
/// entry, plus an install hint line right under any missing one --
/// plain text, same rendering posture the Task Output/Debug panels
/// already settled on for this app's newer panels (no per-line color
/// metadata module, just a readable listing).
pub fn render(entries: &[ToolEntry]) -> String {
    if entries.is_empty() {
        return "(no known LSP servers or DAP adapters for any recognized language)\n".to_string();
    }
    let mut out = String::new();
    for entry in entries {
        let kind = match entry.kind {
            ToolKind::Lsp => "LSP",
            ToolKind::Dap => "DAP",
        };
        let status = match (entry.found_on_path, &entry.elsewhere) {
            (true, _) => "found",
            (false, Some(_)) => "not on PATH",
            (false, None) => "missing",
        };
        let running = if entry.running { ", running" } else { "" };
        out.push_str(&format!("{:<12} {kind}  {:<28} [{status}{running}]\n", format!("{:?}", entry.language), entry.command));
        if let Some(dir) = entry.elsewhere.as_ref().and_then(|p| p.parent()) {
            out.push_str(&format!("             installed in {}, which isn't on Fenix's PATH -- SPC p h can use it for this project\n", dir.display()));
        }
        if let Some(hint) = entry.install_hint {
            out.push_str(&format!("             install: {hint}\n"));
        }
    }
    out
}

/// An additional language server (`tools.json`'s `lsp.additional`):
/// one process per project, beside the primary servers of every
/// language it lists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdditionalEntry {
    pub name: String,
    pub command: String,
    pub languages: Vec<String>,
    pub found_on_path: bool,
    pub running: bool,
    /// It failed to start or crashed, and waits for `:lsp-restart`.
    pub failed: bool,
}

/// The additional servers' rows, under their own heading -- empty when
/// the project has none.
pub fn render_additional(entries: &[AdditionalEntry]) -> String {
    if entries.is_empty() {
        return String::new();
    }
    let mut out = String::from("\nAdditional language servers (beside each language's own)\n");
    for entry in entries {
        let status = if entry.found_on_path { "found" } else { "missing" };
        let state = if entry.running {
            ", running"
        } else if entry.failed {
            ", failed -- :lsp-restart to retry"
        } else {
            ""
        };
        out.push_str(&format!("{:<12} LSP+ {:<28} [{status}{state}]\n", entry.name, entry.command));
        out.push_str(&format!("             serves: {}\n", entry.languages.join(", ")));
    }
    out
}

/// The language servers that failed in this project, each with why and
/// the last lines it wrote to stderr -- empty when none has.
pub fn render_failures(failures: &[(String, String)]) -> String {
    if failures.is_empty() {
        return String::new();
    }
    let mut out = String::from("\nLanguage servers that failed (:lsp-restart tries again)\n");
    for (name, why) in failures {
        let mut lines = why.lines();
        out.push_str(&format!("{name}: {}\n", lines.next().unwrap_or_default()));
        for line in lines {
            out.push_str(&format!("    {line}\n"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_servers_say_why_and_what_they_wrote() {
        assert_eq!(render_failures(&[]), "");
        let text = render_failures(&[("typescript-language-server".into(), "didn't answer initialize in 30 s\n  | Could not find a valid TypeScript installation".into())]);
        assert!(text.contains("Language servers that failed"), "{text}");
        assert!(text.contains("typescript-language-server: didn't answer initialize in 30 s\n"), "{text}");
        assert!(text.contains("    | Could not find a valid TypeScript installation"), "{text}");
    }

    #[test]
    fn additional_servers_are_listed_with_what_they_serve_and_how_they_are() {
        let entry = |running, failed| AdditionalEntry { name: "kiln".into(), command: "kiln".into(), languages: vec!["rust".into(), "markdown".into()], found_on_path: true, running, failed };
        let text = render_additional(&[entry(true, false)]);
        assert!(text.contains("Additional language servers"));
        assert!(text.contains("kiln") && text.contains("running") && text.contains("serves: rust, markdown"), "{text}");
        assert!(render_additional(&[entry(false, true)]).contains(":lsp-restart"));
        assert_eq!(render_additional(&[]), "");
    }

    #[test]
    fn scan_includes_one_lsp_row_per_known_language_with_a_default() {
        let entries = scan(
            |language| match language {
                LanguageId::Python => Some(("pyright-langserver".to_string(), vec![])),
                LanguageId::Rust => Some(("rust-analyzer".to_string(), vec![])),
                _ => None,
            },
            |_| None,
            |_| false,
            |_| false,
        );
        assert_eq!(entries.len(), 2);
        assert!(entries.iter().any(|e| e.language == LanguageId::Python && e.kind == ToolKind::Lsp));
        assert!(entries.iter().any(|e| e.language == LanguageId::Rust && e.kind == ToolKind::Lsp));
    }

    #[test]
    fn scan_includes_a_dap_row_alongside_an_lsp_row_for_the_same_language() {
        let entries = scan(
            |language| (language == LanguageId::Python).then(|| ("pyright-langserver".to_string(), vec![])),
            |language| (language == LanguageId::Python).then(|| ("python".to_string(), vec!["-m".to_string(), "debugpy.adapter".to_string()])),
            |_| false,
            |_| false,
        );
        assert_eq!(entries.len(), 2);
        assert!(entries.iter().any(|e| e.kind == ToolKind::Lsp && e.command == "pyright-langserver"));
        assert!(entries.iter().any(|e| e.kind == ToolKind::Dap && e.command == "python"));
    }

    #[test]
    fn a_command_found_on_path_has_no_install_hint() {
        // `cmd`/`sh` are always on PATH in any real environment this
        // runs in -- a deliberately boring, always-true "found" case.
        let real_command = if cfg!(windows) { "cmd" } else { "sh" };
        let entries = scan(|_| Some((real_command.to_string(), vec![])), |_| None, |_| false, |_| false);
        assert!(entries[0].found_on_path);
        assert_eq!(entries[0].install_hint, None);
    }

    #[test]
    fn a_command_not_found_on_path_carries_its_install_hint() {
        let entries = scan(|_| Some(("rust-analyzer".to_string(), vec![])), |_| None, |_| false, |_| false);
        // Whether rust-analyzer happens to be installed on the machine
        // running this test varies -- only assert the *pairing* holds:
        // missing implies a hint, found implies none.
        for entry in &entries {
            assert_eq!(entry.install_hint.is_some(), !entry.found_on_path && entry.elsewhere.is_none());
        }
    }

    #[test]
    fn running_is_reported_per_entry_from_the_callbacks_given() {
        let entries = scan(
            |language| (language == LanguageId::Python).then(|| ("pyright-langserver".to_string(), vec![])),
            |_| None,
            |language| language == LanguageId::Python,
            |_| false,
        );
        assert!(entries[0].running);
    }

    #[test]
    fn render_lists_every_entry_with_an_install_hint_line_under_a_missing_one() {
        let entries = vec![ToolEntry {
            language: LanguageId::Rust,
            kind: ToolKind::Lsp,
            command: "rust-analyzer".to_string(),
            found_on_path: false,
            running: false,
            install_hint: Some("rustup component add rust-analyzer"),
            elsewhere: None,
        }];
        let text = render(&entries);
        assert!(text.contains("Rust"));
        assert!(text.contains("rust-analyzer"));
        assert!(text.contains("missing"));
        assert!(text.contains("rustup component add rust-analyzer"));
    }

    /// A server that's running is found, whatever `PATH` says; one
    /// installed off `PATH` says where, not "missing" (#43).
    #[test]
    fn a_running_server_is_found_and_one_off_path_says_where() {
        let entries = scan(|l| (l == LanguageId::Python).then(|| ("definitely-not-on-path-xyz".to_string(), vec![])), |_| None, |l| l == LanguageId::Python, |_| false);
        assert!(entries[0].found_on_path && entries[0].install_hint.is_none(), "not [missing, running]");
        let off = ToolEntry {
            language: LanguageId::Python,
            kind: ToolKind::Lsp,
            command: "pyright-langserver".to_string(),
            found_on_path: false,
            running: false,
            install_hint: None,
            elsewhere: Some(std::path::PathBuf::from("/home/me/.local/bin/pyright-langserver")),
        };
        let text = render(&[off]);
        assert!(text.contains("[not on PATH]") && text.contains("installed in /home/me/.local/bin") && !text.contains("missing"), "{text}");
        assert!(install_hint_for("gopls").is_some() && install_hint_for("lua-language-server").is_some());
    }

    #[test]
    fn render_of_an_empty_list_explains_itself_instead_of_being_blank() {
        assert!(render(&[]).contains("no known"));
    }

    #[test]
    fn is_on_path_never_panics_for_a_nonexistent_command() {
        assert!(!is_on_path("definitely-not-a-real-tool-xyz"));
    }
}
