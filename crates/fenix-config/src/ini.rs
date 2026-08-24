use std::collections::BTreeMap;

/// A minimal INI reader: `[section]` headers, `key = value` pairs (split
/// on the *first* `=`, both sides trimmed), blank lines, and `;`/`#`-
/// prefixed comment lines. Anything else -- a malformed line, or a
/// `key = value` before any section header -- is silently dropped, never
/// an error, matching every other persisted-setting reader in this
/// project (missing/corrupt input degrades gracefully rather than
/// failing outright). Returns `section name -> (key -> raw string
/// value)`; `Config::load` does the typed extraction on top of this.
pub(crate) fn parse(text: &str) -> BTreeMap<String, BTreeMap<String, String>> {
    let mut sections: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    let mut current: Option<String> = None;

    for raw_line in text.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
            continue;
        }
        if let Some(inner) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            let name = inner.trim().to_string();
            sections.entry(name.clone()).or_default();
            current = Some(name);
            continue;
        }
        let Some(section) = &current else { continue };
        let Some((key, value)) = line.split_once('=') else { continue };
        sections.entry(section.clone()).or_default().insert(key.trim().to_string(), value.trim().to_string());
    }

    sections
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_key_value_pair_under_its_section() {
        let sections = parse("[editor]\ntheme = TempleOS\n");
        assert_eq!(sections.get("editor").unwrap().get("theme"), Some(&"TempleOS".to_string()));
    }

    #[test]
    fn trims_whitespace_around_the_equals_sign() {
        let sections = parse("[editor]\n  theme   =   TempleOS  \n");
        assert_eq!(sections.get("editor").unwrap().get("theme"), Some(&"TempleOS".to_string()));
    }

    #[test]
    fn skips_blank_lines_and_comment_lines() {
        let sections = parse("[editor]\n; a comment\n\n# another comment\ntheme = TempleOS\n");
        assert_eq!(sections.get("editor").unwrap().len(), 1);
    }

    #[test]
    fn switches_sections_correctly() {
        let sections = parse("[editor]\ntheme = TempleOS\n[completion]\nsymbols_file = /tmp/x.txt\n");
        assert_eq!(sections.get("editor").unwrap().get("theme"), Some(&"TempleOS".to_string()));
        assert_eq!(sections.get("completion").unwrap().get("symbols_file"), Some(&"/tmp/x.txt".to_string()));
    }

    #[test]
    fn a_key_before_any_section_header_is_dropped_not_an_error() {
        let sections = parse("theme = TempleOS\n[editor]\nfont_size = 16\n");
        assert!(!sections.contains_key(""));
        assert_eq!(sections.get("editor").unwrap().len(), 1);
    }

    #[test]
    fn a_malformed_line_with_no_equals_sign_is_dropped() {
        let sections = parse("[editor]\nnot a valid line\ntheme = TempleOS\n");
        assert_eq!(sections.get("editor").unwrap().len(), 1);
    }

    #[test]
    fn a_value_containing_an_equals_sign_keeps_the_rest_after_the_first_split() {
        let sections = parse("[editor]\nfoo = a=b=c\n");
        assert_eq!(sections.get("editor").unwrap().get("foo"), Some(&"a=b=c".to_string()));
    }

    #[test]
    fn empty_input_yields_no_sections() {
        assert!(parse("").is_empty());
    }
}
