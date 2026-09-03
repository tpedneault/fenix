//! Per-language default debug adapter commands and launch-argument
//! shaping for `fenix-gui`'s DAP integration -- mirrors `lsp.rs`'s own
//! role for language servers (pure logic/rendering helpers used by
//! `app.rs`, no session or threading state of its own).

use fenix_syntax::LanguageId;
use std::path::Path;

/// The obvious, common-case debug adapter for a language -- checked
/// only when nothing else names one (there is no `[dap]` config-file
/// override yet, unlike `lsp::resolve_server_command`'s `[lsp]` --
/// genuinely one adapter is all this needs for now). Python is the only
/// language with a live, verified adapter today: `debugpy`'s own
/// `python -m debugpy.adapter` speaks DAP over stdio with no extra
/// setup once `pip install debugpy` has been run, matching pyright's
/// own "the obvious server, no config needed" bar. Native debugging
/// (Rust/C++ via `lldb-dap`/`codelldb`) is deliberately not defaulted
/// here yet -- the plan's own Milestone D targets those, but neither
/// adapter was available to verify against in this environment, and a
/// wrong guess at the binary name would be worse than no default at
/// all (a clear "no known adapter for language" error, same posture
/// `lsp::default_server_command` already has for anything beyond its
/// own short list).
pub fn default_adapter_command(language: LanguageId) -> Option<(String, Vec<String>)> {
    match language {
        LanguageId::Python => Some(("python".to_string(), vec!["-m".to_string(), "debugpy.adapter".to_string()])),
        _ => None,
    }
}

/// The `launch` request's adapter-specific `arguments` object for
/// `language`, debugging `program` -- DAP deliberately leaves this
/// entirely up to each adapter (see `LaunchRequestArguments::
/// additional_attributes`'s own doc comment in the `debug-adapter-
/// protocol` crate), so there's no generic shape to build here, only a
/// per-adapter one. `debugpy`'s: `program` (the script to run) and
/// `console: internalConsole` (debugpy's own stdout/stderr are already
/// piped through this app's `DapClient`/`output` events -- there's no
/// separate terminal to launch it in the way `integratedTerminal`/
/// `externalTerminal` would need). `justMyCode` is deliberately left
/// unset -- debugpy's own default (skip stepping into library code) is
/// the right one for everyday use; the live-verification probe this
/// was built against set it to `false` specifically to *exercise*
/// stepping into library frames, not because that's the right default.
pub fn launch_arguments(language: LanguageId, program: &Path) -> serde_json::Map<String, serde_json::Value> {
    let mut args = serde_json::Map::new();
    match language {
        LanguageId::Python => {
            args.insert("program".to_string(), serde_json::Value::String(program.to_string_lossy().into_owned()));
            args.insert("console".to_string(), serde_json::Value::String("internalConsole".to_string()));
        }
        _ => {
            args.insert("program".to_string(), serde_json::Value::String(program.to_string_lossy().into_owned()));
        }
    }
    args
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn python_has_a_built_in_default_adapter_command() {
        assert_eq!(default_adapter_command(LanguageId::Python), Some(("python".to_string(), vec!["-m".to_string(), "debugpy.adapter".to_string()])));
    }

    #[test]
    fn a_language_with_no_verified_adapter_resolves_to_none() {
        assert_eq!(default_adapter_command(LanguageId::Rust), None);
        assert_eq!(default_adapter_command(LanguageId::Cpp), None);
    }

    #[test]
    fn python_launch_arguments_include_the_program_and_internal_console() {
        let args = launch_arguments(LanguageId::Python, &PathBuf::from("C:/proj/main.py"));
        assert_eq!(args.get("program").and_then(|v| v.as_str()), Some("C:/proj/main.py"));
        assert_eq!(args.get("console").and_then(|v| v.as_str()), Some("internalConsole"));
    }
}
