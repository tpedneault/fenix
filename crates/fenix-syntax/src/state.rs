use std::cell::RefCell;
use std::ops::Range;

use tree_sitter::{Language, Parser, Query, QueryCursor, StreamingIterator, Tree};

use crate::edit::{to_input_edit, RawEdit};
use crate::highlight::{resolve_overlaps, RawCapture};
use crate::language::LanguageId;

/// Markdown's own second grammar, for prose text a block-structure
/// parse alone can't see inside of -- see `SyntaxState::inline`'s own
/// doc comment for why this exists at all.
struct InlineGrammar {
    /// Finds every `(inline)` node in the *block* tree -- run against
    /// the block grammar/tree, not this grammar's own one; what tells
    /// `highlights_in_range` where to run the inline parse at all.
    span_query: Query,
    /// Parses each of those spans' own text with the inline grammar.
    /// `RefCell`, not a plain field: `Parser::parse` needs `&mut self`,
    /// but `highlights_in_range` only ever borrows `&self` (matching
    /// every other read-only accessor `SyntaxState` has) -- and a
    /// fresh, from-scratch parse per span, not a persisted tree with
    /// its own incremental-edit tracking, is genuinely all this needs:
    /// a single paragraph's, heading's, or list item's worth of inline
    /// content is cheap to reparse from nothing every call, and
    /// `highlights_in_range` is already windowed to whatever's on
    /// screen, the same reason the per-frame cost of the outer block
    /// parse alone was never a concern either.
    parser: RefCell<Parser>,
    query: Query,
}

impl InlineGrammar {
    fn new(block_language: &Language) -> Self {
        let span_query =
            Query::new(block_language, "(inline) @inline").expect("hand-written query is valid for tree-sitter-md's own block grammar");
        let inline_language: Language = tree_sitter_md::INLINE_LANGUAGE.into();
        let mut parser = Parser::new();
        parser
            .set_language(&inline_language)
            .expect("bundled grammar's ABI is compatible with the bundled tree-sitter core");
        let query = Query::new(&inline_language, tree_sitter_md::HIGHLIGHT_QUERY_INLINE)
            .expect("bundled highlights.scm for this language failed to compile");
        Self { span_query, parser: RefCell::new(parser), query }
    }
}

/// Per-buffer incremental parse + highlight state for one language.
pub struct SyntaxState {
    parser: Parser,
    tree: Option<Tree>,
    query: Query,
    /// Markdown only: tree-sitter-md ships *two* grammars, one for
    /// block structure (headings, lists, code fences -- everything
    /// `query` above already covers) and a separate one for the prose
    /// *inside* a block, which the block grammar itself only ever marks
    /// with a bare `(inline)` node rather than parsing any further --
    /// bold, italic, inline code spans, and links are the inline
    /// grammar's own job, run as a second parse over each such span's
    /// text (a real, if lightweight, language injection, not a
    /// workaround). `None` for every other language, which has no such
    /// split to begin with.
    inline: Option<InlineGrammar>,
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
        let inline = (lang == LanguageId::Markdown).then(|| InlineGrammar::new(&language));
        Self { parser, tree, query, inline }
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
        cursor.set_byte_range(byte_range.clone());
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

        if let Some(inline) = &self.inline {
            self.push_inline_captures(inline, tree, source, byte_range, &mut raw);
        }

        resolve_overlaps(raw)
    }

    /// Finds every `(inline)` span the block tree marks within
    /// `byte_range`, reparses each one's own text with the inline
    /// grammar, and appends its captures (byte-offset back into
    /// `source`'s own coordinate space) onto `raw` -- the actual
    /// injection `highlights_in_range` layers on top of the plain
    /// block-only pass every other language stops at.
    fn push_inline_captures<'a>(
        &'a self,
        inline: &'a InlineGrammar,
        tree: &Tree,
        source: &str,
        byte_range: Range<usize>,
        raw: &mut Vec<RawCapture<'a>>,
    ) {
        let inline_names = inline.query.capture_names();
        let mut span_cursor = QueryCursor::new();
        span_cursor.set_byte_range(byte_range);
        let mut spans = span_cursor.captures(&inline.span_query, tree.root_node(), source.as_bytes());

        let mut parser = inline.parser.borrow_mut();
        while let Some((query_match, capture_ix)) = spans.next() {
            let node = query_match.captures[*capture_ix].node;
            let (start, end) = (node.start_byte(), node.end_byte());
            let Some(span_text) = source.get(start..end) else { continue };
            let Some(span_tree) = parser.parse(span_text, None) else { continue };

            let mut inline_cursor = QueryCursor::new();
            let mut inline_captures = inline_cursor.captures(&inline.query, span_tree.root_node(), span_text.as_bytes());
            while let Some((inline_match, inline_ix)) = inline_captures.next() {
                let capture = &inline_match.captures[*inline_ix];
                raw.push(RawCapture {
                    range: (start + capture.node.start_byte())..(start + capture.node.end_byte()),
                    pattern_index: inline_match.pattern_index,
                    name: inline_names[capture.index as usize],
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tcl gets no LSP -- there is no server for it -- so the
    /// tree-sitter query *is* the whole of its code intelligence, and
    /// anything it misses renders as plain body text. These pin the
    /// four things it used to miss.
    fn tcl_captures(source: &str) -> Vec<(String, String)> {
        let state = SyntaxState::new(LanguageId::Tcl, source);
        state.highlights_in_range(source, 0..source.len()).into_iter().map(|(r, n)| (n.to_string(), source[r].to_string())).collect()
    }

    fn captured_as(source: &str, text: &str) -> Vec<String> {
        tcl_captures(source).into_iter().filter(|(_, t)| t == text).map(|(n, _)| n).collect()
    }

    #[test]
    fn a_procs_own_name_reads_as_a_function_not_a_variable() {
        // It was captured as a variable, which in most themes is just
        // body text -- so every definition in a file looked unhighlighted.
        assert_eq!(captured_as("proc build_all {} {}
", "build_all"), vec!["function"]);
    }

    #[test]
    fn a_ternary_in_an_expr_gets_its_operators_colored() {
        let names = tcl_captures("set x [expr {1 ? 2 : 3}]
");
        let operators: Vec<&str> = names.iter().filter(|(n, _)| n == "operator").map(|(_, t)| t.as_str()).collect();
        assert_eq!(operators, vec!["?", ":"]);
    }

    #[test]
    fn a_fully_qualified_name_reads_as_a_variable() {
        // `::build_flags` is a plain command argument as far as the
        // grammar is concerned, so nothing captured it at all.
        assert_eq!(captured_as("lappend ::build_flags $x
", "::build_flags"), vec!["variable"]);
        assert_eq!(captured_as("namespace eval ::app {}
", "::app"), vec!["variable"]);
    }

    #[test]
    fn a_qualified_name_is_found_inside_a_nested_body_too() {
        let source = "foreach f $flags {
    lappend ::build_flags $f
}
";
        assert_eq!(captured_as(source, "::build_flags"), vec!["variable"]);
    }

    #[test]
    fn an_ordinary_word_is_not_mistaken_for_a_qualified_name() {
        assert!(captured_as("puts hello
", "hello").is_empty());
    }
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
    fn markdown_strong_emphasis_is_captured_by_the_inline_grammar() {
        let source = "Some **bold** text.";
        let state = SyntaxState::new(LanguageId::Markdown, source);
        let highlights = state.highlights_in_range(source, 0..source.len());

        let bold_start = source.find("bold").unwrap();
        let has_strong = highlights.iter().any(|(r, n)| r.start <= bold_start && r.end >= bold_start + 4 && *n == "text.strong");
        assert!(has_strong, "expected \"bold\" captured as text.strong, got {highlights:?}");
    }

    #[test]
    fn markdown_plain_emphasis_is_captured_distinctly_from_strong() {
        let source = "Some *italic* text.";
        let state = SyntaxState::new(LanguageId::Markdown, source);
        let highlights = state.highlights_in_range(source, 0..source.len());

        let italic_start = source.find("italic").unwrap();
        let has_emphasis =
            highlights.iter().any(|(r, n)| r.start <= italic_start && r.end >= italic_start + 6 && *n == "text.emphasis");
        assert!(has_emphasis, "expected \"italic\" captured as text.emphasis, got {highlights:?}");
    }

    #[test]
    fn markdown_inline_code_span_is_captured() {
        let source = "Some `code` here.";
        let state = SyntaxState::new(LanguageId::Markdown, source);
        let highlights = state.highlights_in_range(source, 0..source.len());

        let code_start = source.find("code").unwrap();
        let has_literal = highlights.iter().any(|(r, n)| r.start <= code_start && r.end >= code_start + 4 && *n == "text.literal");
        assert!(has_literal, "expected \"code\" captured as text.literal, got {highlights:?}");
    }

    #[test]
    fn markdown_link_destination_is_captured() {
        let source = "See [here](https://example.com) for more.";
        let state = SyntaxState::new(LanguageId::Markdown, source);
        let highlights = state.highlights_in_range(source, 0..source.len());

        let url_start = source.find("https").unwrap();
        let has_uri = highlights.iter().any(|(r, n)| r.start <= url_start && *n == "text.uri");
        assert!(has_uri, "expected the URL captured as text.uri, got {highlights:?}");
    }

    #[test]
    fn markdown_heading_text_and_its_own_inline_emphasis_are_both_captured() {
        // The nesting case this whole injection exists for: the block
        // grammar captures the heading's full text as `text.title`,
        // the inline grammar separately captures just the `**bold**`
        // portion within it as `text.strong` -- `resolve_overlaps`
        // has to pick the narrower one there, not let the broader
        // heading capture swallow it.
        let source = "# A **bold** heading";
        let state = SyntaxState::new(LanguageId::Markdown, source);
        let highlights = state.highlights_in_range(source, 0..source.len());

        let bold_start = source.find("bold").unwrap();
        let narrow_wins = highlights.iter().any(|(r, n)| r.start <= bold_start && r.end >= bold_start + 4 && *n == "text.strong");
        assert!(narrow_wins, "expected the narrower text.strong capture to win inside the heading, got {highlights:?}");

        let has_title = highlights.iter().any(|(_, n)| *n == "text.title");
        assert!(has_title, "expected the rest of the heading still captured as text.title, got {highlights:?}");
    }

    #[test]
    fn markdown_inline_highlighting_is_windowed_to_the_requested_byte_range() {
        // Same windowing contract the block-only pass already has
        // (`highlights_in_range_is_windowed_to_the_requested_bytes`) --
        // an inline span entirely before the requested range shouldn't
        // contribute anything either.
        let source = "**early bold**\n\nSome *late italic* text.";
        let state = SyntaxState::new(LanguageId::Markdown, source);
        let second_line_start = source.find("Some").unwrap();
        let highlights = state.highlights_in_range(source, second_line_start..source.len());
        assert!(highlights.iter().all(|(r, _)| r.start >= second_line_start));
    }

    #[test]
    fn markdown_inline_grammar_is_reflected_after_apply_edits() {
        let mut source = "Some *italic* text.".to_string();
        let mut state = SyntaxState::new(LanguageId::Markdown, &source);

        // Turn the emphasis into strong emphasis by doubling the
        // asterisks on both sides.
        let star = source.find('*').unwrap();
        source.replace_range(star..star + 1, "**");
        let end_star = source.rfind('*').unwrap();
        source.replace_range(end_star..end_star + 1, "**");
        let edits = [
            RawEdit { start_char: star, new_end_char: star + 2, removed: "*".to_string() },
            RawEdit { start_char: end_star + 1, new_end_char: end_star + 3, removed: "*".to_string() },
        ];
        state.apply_edits(&source, &edits);

        let highlights = state.highlights_in_range(&source, 0..source.len());
        let italic_start = source.find("italic").unwrap();
        let has_strong = highlights.iter().any(|(r, n)| r.start <= italic_start && r.end >= italic_start + 6 && *n == "text.strong");
        assert!(has_strong, "expected the edited text now captured as text.strong, got {highlights:?}");
    }

    #[test]
    fn non_markdown_languages_have_no_inline_grammar_and_are_unaffected() {
        // The injection is Markdown-only -- confirms it doesn't
        // somehow fire (or panic) for a language with no `(inline)`
        // node kind at all.
        let source = "let x = 1;";
        let state = SyntaxState::new(LanguageId::Rust, source);
        let _ = state.highlights_in_range(source, 0..source.len());
        assert!(state.inline.is_none());
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

    #[test]
    fn tcl_highlights_something() {
        smoke_test(LanguageId::Tcl, "proc greet {name} {\n    puts \"hello $name\"\n}\n");
    }

    #[test]
    fn dockerfile_highlights_something() {
        smoke_test(LanguageId::Dockerfile, "FROM rust:1\nRUN cargo build\nCMD [\"./app\"]\n");
    }

    #[test]
    fn batch_highlights_something() {
        smoke_test(LanguageId::Batch, "@echo off\nset FOO=bar\necho %FOO%\n");
    }

    /// Tcl is the primary language Fenix is meant to support well, and its
    /// vendored `highlights.scm` pairs several captures on the same node
    /// (`@repeat @keyword`, `@spell @comment`) -- confirms overlap
    /// resolution doesn't just pick an arbitrary one of the pair that
    /// happens to have no theme mapping.
    #[test]
    fn tcl_control_flow_keywords_and_comments_are_captured() {
        let source = "# a comment\nif {1} {\n    puts hi\n} else {\n    while {0} {\n        break\n    }\n}\n";
        let state = SyntaxState::new(LanguageId::Tcl, source);
        let highlights = state.highlights_in_range(source, 0..source.len());

        let comment_start = source.find('#').unwrap();
        let has_comment = highlights.iter().any(|(r, n)| r.start == comment_start && (*n == "comment" || *n == "spell"));
        assert!(has_comment, "expected the comment to be captured, got {highlights:?}");

        let if_start = source.find("if").unwrap();
        let has_if = highlights
            .iter()
            .any(|(r, n)| r.start == if_start && (*n == "keyword" || *n == "conditional"));
        assert!(has_if, "expected \"if\" to be captured as keyword/conditional, got {highlights:?}");

        let while_start = source.find("while").unwrap();
        let has_while = highlights
            .iter()
            .any(|(r, n)| r.start == while_start && (*n == "keyword" || *n == "repeat"));
        assert!(has_while, "expected \"while\" to be captured as keyword/repeat, got {highlights:?}");
    }
}

