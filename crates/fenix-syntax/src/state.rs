use std::ops::Range;

use tree_sitter::{Parser, Query, QueryCursor, StreamingIterator, Tree};

use crate::edit::{to_input_edit, RawEdit};
use crate::highlight::{resolve_overlaps, RawCapture};
use crate::language::LanguageId;

/// Per-buffer incremental parse + highlight state for one language.
pub struct SyntaxState {
    parser: Parser,
    tree: Option<Tree>,
    query: Query,
}

impl SyntaxState {
    /// Seeds the initial parse tree from `source` (the buffer's full text
    /// at the time highlighting starts).
    pub fn new(lang: LanguageId, source: &str) -> Self {
        let language = lang.language();
        let mut parser = Parser::new();
        parser
            .set_language(&language)
            .expect("bundled grammar's ABI is compatible with the bundled tree-sitter core");
        let tree = parser.parse(source, None);
        let query = Query::new(&language, lang.highlights_query())
            .expect("bundled highlights.scm for this language failed to compile");
        Self { parser, tree, query }
    }

    /// Applies queued low-level edits (in the order they happened) to the
    /// persisted tree, then incrementally reparses against `source` (the
    /// *current*, already-edited text). Passing an empty `edits` slice
    /// still reparses -- cheap, since tree-sitter reuses unchanged
    /// subtrees when nothing actually changed.
    pub fn apply_edits(&mut self, source: &str, edits: &[RawEdit]) {
        if let Some(tree) = self.tree.as_mut() {
            for edit in edits {
                tree.edit(&to_input_edit(source, edit));
            }
        }
        self.tree = self.parser.parse(source, self.tree.as_ref());
    }

    /// Highlight spans within `byte_range`, flattened into a non-
    /// overlapping, ordered list of `(byte range, capture name)` --
    /// restricted to the given range so callers doing windowed rendering
    /// only pay for what's actually on screen, not the whole file.
    pub fn highlights_in_range(&self, source: &str, byte_range: Range<usize>) -> Vec<(Range<usize>, &str)> {
        let Some(tree) = &self.tree else { return Vec::new() };
        let names = self.query.capture_names();

        let mut cursor = QueryCursor::new();
        cursor.set_byte_range(byte_range);
        let mut captures = cursor.captures(&self.query, tree.root_node(), source.as_bytes());

        let mut raw = Vec::new();
        while let Some((query_match, capture_ix)) = captures.next() {
            let capture = &query_match.captures[*capture_ix];
            raw.push(RawCapture {
                range: capture.node.start_byte()..capture.node.end_byte(),
                pattern_index: query_match.pattern_index,
                name: names[capture.index as usize],
            });
        }
        resolve_overlaps(raw)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::language::LanguageId;

    #[test]
    fn parses_rust_and_highlights_a_keyword_and_a_string() {
        let source = r#"fn main() { let s = "hi"; }"#;
        let state = SyntaxState::new(LanguageId::Rust, source);
        let highlights = state.highlights_in_range(source, 0..source.len());
        assert!(!highlights.is_empty(), "expected at least one highlight span, got none");

        // "fn" (byte 0..2) should be captured as some flavor of keyword.
        let fn_capture = highlights.iter().find(|(range, _)| range.start == 0 && range.end == 2);
        assert!(fn_capture.is_some(), "expected a capture covering \"fn\", got {highlights:?}");
        assert!(fn_capture.unwrap().1.starts_with("keyword"), "got {highlights:?}");

        // The string literal `"hi"` should be captured as some flavor of string.
        let string_start = source.find('"').unwrap();
        let has_string_capture =
            highlights.iter().any(|(range, name)| range.start <= string_start && name.starts_with("string"));
        assert!(has_string_capture, "expected a string capture, got {highlights:?}");
    }

    #[test]
    fn highlights_in_range_is_windowed_to_the_requested_bytes() {
        let source = "fn a() {}\nfn b() {}\n";
        let state = SyntaxState::new(LanguageId::Rust, source);
        // Restrict to just the second line -- shouldn't see anything from the first.
        let second_line_start = source.find("fn b").unwrap();
        let highlights = state.highlights_in_range(source, second_line_start..source.len());
        assert!(highlights.iter().all(|(range, _)| range.start >= second_line_start));
    }

    #[test]
    fn incremental_edit_is_reflected_after_apply_edits() {
        let mut source = "fn a() {}".to_string();
        let mut state = SyntaxState::new(LanguageId::Rust, &source);

        // Insert "b" right after "fn a() {}" doesn't change much, so
        // instead rename `a` to `main` by replacing char 3 ("a") -- an
        // edit that should still parse cleanly and keep highlighting the
        // "fn" keyword at the same position.
        let edit = RawEdit { start_char: 3, new_end_char: 7, removed: "a".to_string() };
        source.replace_range(3..4, "main");
        state.apply_edits(&source, std::slice::from_ref(&edit));

        let highlights = state.highlights_in_range(&source, 0..source.len());
        let fn_capture = highlights.iter().find(|(range, _)| range.start == 0 && range.end == 2);
        assert!(fn_capture.is_some(), "expected \"fn\" still highlighted after edit, got {highlights:?}");
    }

    #[test]
    fn empty_source_yields_no_highlights() {
        let state = SyntaxState::new(LanguageId::Rust, "");
        assert_eq!(state.highlights_in_range("", 0..0), Vec::<(Range<usize>, &str)>::new());
    }

    /// One smoke test per newly-registered language: confirms `new` doesn't
    /// panic (grammar ABI compatible, bundled highlights.scm compiles) and
    /// that at least one capture comes back for representative source --
    /// catches per-crate wiring mistakes (wrong query constant, wrong
    /// `LanguageFn`) that pure logic tests wouldn't, the same role
    /// `parses_rust_and_highlights_a_keyword_and_a_string` played for Rust.
    fn smoke_test(lang: LanguageId, source: &str) {
        let state = SyntaxState::new(lang, source);
        let highlights = state.highlights_in_range(source, 0..source.len());
        assert!(!highlights.is_empty(), "expected at least one highlight span for {lang:?}, got none");
    }

    #[test]
    fn toml_highlights_something() {
        smoke_test(LanguageId::Toml, "[package]\nname = \"fenix\"\nversion = \"0.0.1\"\n");
    }

    #[test]
    fn markdown_highlights_something() {
        smoke_test(LanguageId::Markdown, "# Heading\n\nSome *text* and a ```code``` fence.\n");
    }

    #[test]
    fn json_highlights_something() {
        smoke_test(LanguageId::Json, r#"{"key": "value", "n": 1, "ok": true}"#);
    }

    #[test]
    fn yaml_highlights_something() {
        smoke_test(LanguageId::Yaml, "key: value\nlist:\n  - one\n  - two\n");
    }

    #[test]
    fn python_highlights_something() {
        smoke_test(LanguageId::Python, "def main():\n    print(\"hi\")\n");
    }

    #[test]
    fn javascript_highlights_something() {
        smoke_test(LanguageId::JavaScript, "function main() { return \"hi\"; }");
    }

    #[test]
    fn typescript_highlights_something() {
        smoke_test(LanguageId::TypeScript, "function main(): string { return \"hi\"; }");
    }

    #[test]
    fn tsx_highlights_something() {
        // tree-sitter-typescript 0.23.2's bundled HIGHLIGHTS_QUERY only
        // covers a small capture set (type, type.builtin,
        // punctuation.bracket, variable.parameter, keyword) -- no
        // standalone string/comment/keyword-per-token coverage. A type
        // annotation is one of the few things it reliably captures under
        // either TypeScript or Tsx.
        smoke_test(LanguageId::Tsx, "function main(): string { return 1; }");
    }

    #[test]
    fn c_highlights_something() {
        smoke_test(LanguageId::C, "int main() { return 0; }");
    }

    #[test]
    fn bash_highlights_something() {
        smoke_test(LanguageId::Bash, "echo \"hello\"\n");
    }
}
