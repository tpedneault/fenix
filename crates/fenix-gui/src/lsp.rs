//! Per-language default server commands and naming for `fenix-gui`'s LSP
//! integration -- mirrors `docker_panel.rs`/`git_panel.rs`'s role (pure
//! logic/rendering helpers used by `app.rs`, no session or threading
//! state of its own; that lives in `App` alongside every other session
//! kind).

use fenix_syntax::LanguageId;

/// The client capabilities this app advertises in every `initialize`
/// request -- deliberately conservative for v1 (just enough to receive
/// diagnostics; hover/completion/rename/etc. are each added alongside
/// the request/rendering code that actually uses them, not speculatively
/// declared ahead of time). `dynamic_registration: Some(false)`
/// throughout: this client's capabilities are fixed at connect time,
/// there's no support (yet) for a server registering/unregistering one
/// mid-session.
pub fn client_capabilities() -> lsp_types::ClientCapabilities {
    lsp_types::ClientCapabilities {
        text_document: Some(lsp_types::TextDocumentClientCapabilities {
            synchronization: Some(lsp_types::TextDocumentSyncClientCapabilities {
                dynamic_registration: Some(false),
                will_save: Some(false),
                will_save_wait_until: Some(false),
                did_save: Some(true),
            }),
            publish_diagnostics: Some(lsp_types::PublishDiagnosticsClientCapabilities { ..Default::default() }),
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// The obvious, common-case server for a language, so the vast majority
/// of setups need zero `[lsp]` configuration -- just the binary already
/// on `PATH`. Checked only when nothing in `Config::lsp_servers` names
/// this language; see `resolve_server_command`.
pub fn default_server_command(language: LanguageId) -> Option<(String, Vec<String>)> {
    match language {
        LanguageId::Python => Some(("pyright-langserver".to_string(), vec!["--stdio".to_string()])),
        _ => None,
    }
}

/// `fenix_syntax::LanguageId`'s own config-file name -- the `LANGUAGE`
/// half of `[lsp]`'s `serverN = LANGUAGE|COMMAND_LINE`. A small
/// standalone match rather than a `Debug`/`Display` impl on `LanguageId`
/// itself (which `fenix-syntax` has no reason to know is used for
/// this) -- covers every language an LSP server plausibly exists for;
/// anything else falls back to its `Debug` name lowercased, fine as a
/// *label* (nothing ever parses it back into a `LanguageId`) even
/// though it isn't hand-picked for every variant.
pub fn language_config_name(language: LanguageId) -> String {
    match language {
        LanguageId::Python => "python".to_string(),
        LanguageId::Rust => "rust".to_string(),
        LanguageId::C => "c".to_string(),
        LanguageId::Bash => "bash".to_string(),
        LanguageId::JavaScript => "javascript".to_string(),
        LanguageId::TypeScript => "typescript".to_string(),
        LanguageId::Tsx => "tsx".to_string(),
        other => format!("{other:?}").to_lowercase(),
    }
}

/// Resolves the command to launch for `language`'s server: a `[lsp]`
/// override if `configured` (`Config::lsp_servers`, verbatim) names this
/// language, else `default_server_command`. Splits the configured
/// command line into program + args on whitespace (no shell-quoting
/// support -- see `Config::lsp_servers`'s own doc comment for why
/// that's an acceptable limitation).
pub fn resolve_server_command(language: LanguageId, configured: &[(String, String)]) -> Option<(String, Vec<String>)> {
    let name = language_config_name(language);
    for (configured_language, command_line) in configured {
        if *configured_language == name {
            let mut parts = command_line.split_whitespace();
            let program = parts.next()?.to_string();
            let args = parts.map(str::to_string).collect();
            return Some((program, args));
        }
    }
    default_server_command(language)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn python_has_a_built_in_default_command() {
        assert_eq!(default_server_command(LanguageId::Python), Some(("pyright-langserver".to_string(), vec!["--stdio".to_string()])));
    }

    #[test]
    fn a_language_with_no_built_in_default_resolves_to_none_absent_config() {
        assert_eq!(resolve_server_command(LanguageId::Rust, &[]), None);
    }

    #[test]
    fn a_configured_override_wins_over_the_built_in_default() {
        let configured = vec![("python".to_string(), "custom-pyright --flag".to_string())];
        assert_eq!(resolve_server_command(LanguageId::Python, &configured), Some(("custom-pyright".to_string(), vec!["--flag".to_string()])));
    }

    #[test]
    fn config_for_a_different_language_does_not_affect_this_ones_resolution() {
        let configured = vec![("rust".to_string(), "rust-analyzer".to_string())];
        assert_eq!(resolve_server_command(LanguageId::Python, &configured), default_server_command(LanguageId::Python));
    }

    #[test]
    fn a_configured_command_with_no_arguments_still_resolves() {
        let configured = vec![("python".to_string(), "my-pyright".to_string())];
        assert_eq!(resolve_server_command(LanguageId::Python, &configured), Some(("my-pyright".to_string(), Vec::new())));
    }
}
