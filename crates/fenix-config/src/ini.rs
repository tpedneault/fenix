use std::collections::BTreeMap;

/// The write-side counterpart to `parse`'s quote handling: `raw` as-is
/// if trimming it wouldn't change anything (the common case -- keeps
/// the file looking like a normal INI for every ordinary value), or
/// wrapped in double quotes if `raw` has any leading/trailing
/// whitespace `parse` would otherwise strip back out on the next load
/// (including `raw` being *entirely* whitespace). An empty `raw` is
/// also quoted -- not required for correctness (an unquoted empty
/// value already round-trips fine, `parse` just inserts an empty
/// string), but `key = ""` reads as a deliberate choice where `key = `
/// looks like a mistake. Doesn't escape an embedded `"` -- not needed
/// for any value this crate currently writes, and adding blanket
/// escaping for a case that can't occur would just be unused
/// complexity.
pub(crate) fn quote_if_needed(raw: &str) -> String {
    if raw.trim() == raw && !raw.is_empty() {
        raw.to_string()
    } else {
        format!("\"{raw}\"")
    }
}

/// A minimal INI reader: `[section]` headers, `key = value` pairs (split
/// on the *first* `=`, both sides trimmed), blank lines, and `;`/`#`-
/// prefixed comment lines. Anything else -- a malformed line, or a
/// `key = value` before any section header -- is silently dropped, never
/// an error, matching every other persisted-setting reader in this
/// project (missing/corrupt input degrades gracefully rather than
/// failing outright). Returns `section name -> (key -> raw string
/// value)`; `Config::load` does the typed extraction on top of this.
///
/// A value wrapped in a matching pair of double quotes (`key = " "`)
/// has its quotes stripped and its *inner* content taken verbatim, not
/// trimmed -- the only way to represent a value that's meaningful
/// whitespace (a single-space argument separator, say) or has
/// significant leading/trailing padding, since the plain unquoted form
/// always gets trimmed down to nothing. Every existing unquoted value
/// keeps behaving exactly as before (still trimmed, still typo-
/// tolerant of stray surrounding whitespace) -- quoting is opt-in.
pub(crate) fn parse(text: &str) -> BTreeMap<String, BTreeMap<String, String>> {
    let mut sections: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    let mut current: Option<String> = None;

    // Strips a leading UTF-8 BOM (`\u{FEFF}`) if present -- Windows
    // Notepad's "UTF-8" save option writes one, and so does PowerShell's
    // `Out-File`/`Set-Content` without `-Encoding utf8NoBOM`, both very
    // plausible ways for someone to hand-author this file. `str::trim`
    // does *not* strip it (`\u{FEFF}` isn't Unicode whitespace), so
    // without this, a BOM'd file has its very first line read as
    // `"\u{FEFF}[section]"` instead of `"[section]"` -- `strip_prefix('[')`
    // then fails, the line is silently treated as "not a section header,"
    // and every key under what was meant to be the file's first section
    // is dropped as if it came before any `[section]` at all.
    let text = text.strip_prefix('\u{FEFF}').unwrap_or(text);

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
        let value = value.trim();
        let value = match value.len() {
            n if n >= 2 && value.starts_with('"') && value.ends_with('"') => &value[1..n - 1],
            _ => value,
        };
        sections.entry(section.clone()).or_default().insert(key.trim().to_string(), value.to_string());
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
    fn a_quoted_value_of_pure_whitespace_survives_intact() {
        let sections = parse("[mib]\nseparator = \" \"\n");
        assert_eq!(sections.get("mib").unwrap().get("separator"), Some(&" ".to_string()));
    }

    #[test]
    fn an_unquoted_value_of_pure_whitespace_still_trims_to_empty() {
        // The documented reason quoting exists at all -- this is the
        // exact case that motivated it.
        let sections = parse("[mib]\nseparator =  \n");
        assert_eq!(sections.get("mib").unwrap().get("separator"), Some(&String::new()));
    }

    #[test]
    fn a_quoted_value_preserves_leading_and_trailing_padding() {
        let sections = parse("[mib]\nseparator = \" | \"\n");
        assert_eq!(sections.get("mib").unwrap().get("separator"), Some(&" | ".to_string()));
    }

    #[test]
    fn a_quoted_empty_string_parses_as_empty_not_two_quote_characters() {
        let sections = parse("[mib]\nseparator = \"\"\n");
        assert_eq!(sections.get("mib").unwrap().get("separator"), Some(&String::new()));
    }

    #[test]
    fn a_normal_unquoted_value_is_unaffected_by_quote_handling() {
        let sections = parse("[editor]\ntheme = TempleOS\n");
        assert_eq!(sections.get("editor").unwrap().get("theme"), Some(&"TempleOS".to_string()));
    }

    #[test]
    fn empty_input_yields_no_sections() {
        assert!(parse("").is_empty());
    }

    #[test]
    fn a_leading_utf8_bom_does_not_swallow_the_files_first_section() {
        // The actual reported bug: a config.ini hand-saved by an editor
        // that writes a UTF-8 BOM (Notepad's "UTF-8" option, PowerShell's
        // `Out-File`/`Set-Content` without `-Encoding utf8NoBOM`) had its
        // entire first section silently vanish -- every key in it read
        // as "before any section header" and got dropped.
        let sections = parse("\u{FEFF}[vnc]\nhost1 = build-vm|10.0.0.5|5900\nhost2 = test-vm|10.0.0.6|5901\n[workspaces]\nws1 = VNC Build|vnc:build-vm\n");
        assert_eq!(sections.get("vnc").unwrap().len(), 2);
        assert_eq!(sections.get("vnc").unwrap().get("host1"), Some(&"build-vm|10.0.0.5|5900".to_string()));
        assert_eq!(sections.get("workspaces").unwrap().get("ws1"), Some(&"VNC Build|vnc:build-vm".to_string()));
    }

    #[test]
    fn a_bom_on_a_file_with_only_one_section_still_parses_it() {
        let sections = parse("\u{FEFF}[editor]\ntheme = TempleOS\n");
        assert_eq!(sections.get("editor").unwrap().get("theme"), Some(&"TempleOS".to_string()));
    }
}
