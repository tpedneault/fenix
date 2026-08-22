use tree_sitter::Language;

/// Which tree-sitter grammar a buffer is highlighted with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LanguageId {
    Rust,
}

impl LanguageId {
    pub(crate) fn language(self) -> Language {
        match self {
            LanguageId::Rust => tree_sitter_rust::LANGUAGE.into(),
        }
    }

    pub(crate) fn highlights_query(self) -> &'static str {
        match self {
            LanguageId::Rust => tree_sitter_rust::HIGHLIGHTS_QUERY,
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
}
