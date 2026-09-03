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
}

/// Every language this app has a built-in LSP default or DAP default
/// for -- the only ones worth a row here at all. A language with
/// neither (TOML, JSON, Markdown, ...) has nothing to detect or
/// install, so it's not listed, the same "no support for this, not a
/// gap" posture `dap::default_adapter_command`'s own doc comment
/// already takes for languages beyond Python.
const KNOWN_LANGUAGES: &[LanguageId] =
    &[LanguageId::Python, LanguageId::Rust, LanguageId::C, LanguageId::Cpp, LanguageId::Bash, LanguageId::JavaScript, LanguageId::TypeScript, LanguageId::Tsx];

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
            let found_on_path = is_on_path(&command);
            let install_hint = if found_on_path { None } else { install_hint_for(&command) };
            entries.push(ToolEntry { language, kind: ToolKind::Lsp, command, found_on_path, running: lsp_running(language), install_hint });
        }
        if let Some((command, _)) = resolve_dap_command(language) {
            let found_on_path = is_on_path(&command);
            let install_hint = if found_on_path { None } else { install_hint_for(&command) };
            entries.push(ToolEntry { language, kind: ToolKind::Dap, command, found_on_path, running: dap_running(language), install_hint });
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
fn is_on_path(command: &str) -> bool {
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
        let status = if entry.found_on_path { "found" } else { "missing" };
        let running = if entry.running { ", running" } else { "" };
        out.push_str(&format!("{:<12} {kind}  {:<28} [{status}{running}]\n", format!("{:?}", entry.language), entry.command));
        if let Some(hint) = entry.install_hint {
            out.push_str(&format!("             install: {hint}\n"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

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
            assert_eq!(entry.install_hint.is_some(), !entry.found_on_path);
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
        }];
        let text = render(&entries);
        assert!(text.contains("Rust"));
        assert!(text.contains("rust-analyzer"));
        assert!(text.contains("missing"));
        assert!(text.contains("rustup component add rust-analyzer"));
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
