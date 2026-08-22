use tree_sitter::Language;

/// Which tree-sitter grammar a buffer is highlighted with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LanguageId {
    Rust,
    Toml,
    /// Block-level grammar only (headers, code fences, lists, block
    /// quotes...) -- `tree-sitter-md` actually ships *two* grammars,
    /// block and inline, for full Markdown highlighting (the inline one
    /// covers `**bold**`, `` `code spans` ``, links, etc. within
    /// paragraph text). Running both together is an injection-like setup
    /// this crate deliberately doesn't do for v1 (see the injections cut
    /// in the syntax-highlighting plan) -- block-level highlighting only.
    Markdown,
    Json,
    Yaml,
    Python,
    JavaScript,
    TypeScript,
    /// `.tsx` -- a distinct grammar from plain TypeScript (JSX syntax),
    /// sharing the same highlights query.
    Tsx,
    C,
    Bash,
    Tcl,
}

/// `tree-sitter-tcl`'s Rust bindings only expose `LANGUAGE`/`NODE_TYPES` --
/// its `HIGHLIGHTS_QUERY` constant is commented out upstream (as of the
/// pinned commit; see `Cargo.toml`'s `rev`), unlike every other grammar
/// here. Vendored directly from the same repo (MIT-licensed) instead of
/// left unavailable -- Tcl is a genuinely well-used language, not worth
/// leaving without highlighting over a bindings gap in one specific crate.
const TCL_HIGHLIGHTS_QUERY: &str = include_str!("../queries/tcl/highlights.scm");

impl LanguageId {
    pub(crate) fn language(self) -> Language {
        match self {
            LanguageId::Rust => tree_sitter_rust::LANGUAGE.into(),
            LanguageId::Toml => tree_sitter_toml_ng::LANGUAGE.into(),
            LanguageId::Markdown => tree_sitter_md::LANGUAGE.into(),
            LanguageId::Json => tree_sitter_json::LANGUAGE.into(),
            LanguageId::Yaml => tree_sitter_yaml::LANGUAGE.into(),
            LanguageId::Python => tree_sitter_python::LANGUAGE.into(),
            LanguageId::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
            LanguageId::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            LanguageId::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
            LanguageId::C => tree_sitter_c::LANGUAGE.into(),
            LanguageId::Bash => tree_sitter_bash::LANGUAGE.into(),
            LanguageId::Tcl => tree_sitter_tcl::LANGUAGE.into(),
        }
    }

    pub(crate) fn highlights_query(self) -> &'static str {
        match self {
            LanguageId::Rust => tree_sitter_rust::HIGHLIGHTS_QUERY,
            LanguageId::Toml => tree_sitter_toml_ng::HIGHLIGHTS_QUERY,
            LanguageId::Markdown => tree_sitter_md::HIGHLIGHT_QUERY_BLOCK,
            LanguageId::Json => tree_sitter_json::HIGHLIGHTS_QUERY,
            LanguageId::Yaml => tree_sitter_yaml::HIGHLIGHTS_QUERY,
            LanguageId::Python => tree_sitter_python::HIGHLIGHTS_QUERY,
            LanguageId::JavaScript => tree_sitter_javascript::HIGHLIGHT_QUERY,
            LanguageId::TypeScript | LanguageId::Tsx => tree_sitter_typescript::HIGHLIGHTS_QUERY,
            LanguageId::C => tree_sitter_c::HIGHLIGHT_QUERY,
            LanguageId::Bash => tree_sitter_bash::HIGHLIGHT_QUERY,
            LanguageId::Tcl => TCL_HIGHLIGHTS_QUERY,
        }
    }
}

/// Detects a language from a file extension (no leading dot, e.g. `"rs"`).
/// Extension-only -- no shebang or content sniffing. Returns `None` for
/// anything not in the registry, which callers treat as "no highlighting
/// for this file," not an error.
pub fn detect_language(extension: &str) -> Option<LanguageId> {
    match extension {
        "rs" => Some(LanguageId::Rust),
        "toml" => Some(LanguageId::Toml),
        "md" | "markdown" => Some(LanguageId::Markdown),
        "json" => Some(LanguageId::Json),
        "yaml" | "yml" => Some(LanguageId::Yaml),
        "py" | "pyi" => Some(LanguageId::Python),
        "js" | "mjs" | "cjs" | "jsx" => Some(LanguageId::JavaScript),
        "ts" | "mts" | "cts" => Some(LanguageId::TypeScript),
        "tsx" => Some(LanguageId::Tsx),
        "c" | "h" => Some(LanguageId::C),
        "sh" | "bash" => Some(LanguageId::Bash),
        "tcl" | "tm" => Some(LanguageId::Tcl),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_rust_by_extension() {
        assert_eq!(detect_language("rs"), Some(LanguageId::Rust));
    }

    #[test]
    fn unknown_extension_has_no_language() {
        assert_eq!(detect_language("xyz"), None);
        assert_eq!(detect_language(""), None);
    }

    #[test]
    fn detects_every_registered_language_by_its_common_extension() {
        assert_eq!(detect_language("toml"), Some(LanguageId::Toml));
        assert_eq!(detect_language("md"), Some(LanguageId::Markdown));
        assert_eq!(detect_language("json"), Some(LanguageId::Json));
        assert_eq!(detect_language("yaml"), Some(LanguageId::Yaml));
        assert_eq!(detect_language("yml"), Some(LanguageId::Yaml));
        assert_eq!(detect_language("py"), Some(LanguageId::Python));
        assert_eq!(detect_language("js"), Some(LanguageId::JavaScript));
        assert_eq!(detect_language("ts"), Some(LanguageId::TypeScript));
        assert_eq!(detect_language("tsx"), Some(LanguageId::Tsx));
        assert_eq!(detect_language("c"), Some(LanguageId::C));
        assert_eq!(detect_language("h"), Some(LanguageId::C));
        assert_eq!(detect_language("sh"), Some(LanguageId::Bash));
        assert_eq!(detect_language("tcl"), Some(LanguageId::Tcl));
        assert_eq!(detect_language("tm"), Some(LanguageId::Tcl));
    }
}
