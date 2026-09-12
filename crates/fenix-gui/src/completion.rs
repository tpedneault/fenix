//! Pure logic for the autocompletion popup: extracting the in-progress
//! identifier prefix at the cursor, and the buffer-word candidate source
//! (the one completion source that works for any language, not just
//! Tcl). No knowledge of rendering, Vim modes, or the Tcl-specific
//! completion *sources* -- `App` (in `app.rs`) owns the actual popup
//! state and drives `fenix-completion`/`fenix-picker` with what this
//! module computes, the same "small self-contained module" role
//! `dashboard.rs`/`icon.rs` already play.

use std::collections::HashSet;

use fenix_core::{Buffer, Cursor};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Keyword,
    Symbol,
    Buffer,
    Lsp,
    Snippet,
}

impl Source {
    pub fn label(self) -> &'static str {
        match self {
            Self::Keyword => "keyword",
            Self::Symbol => "symbol",
            Self::Buffer => "word",
            Self::Lsp => "LSP",
            Self::Snippet => "snippet",
        }
    }
}

#[derive(Clone)]
pub enum Insertion {
    Text(String),
    Snippet(fenix_snippets::Template),
    Lsp(Box<lsp_types::CompletionItem>),
}

#[derive(Clone)]
pub struct Item {
    pub label: String,
    pub source: Source,
    pub detail: String,
    pub documentation: String,
    pub insertion: Insertion,
}

impl From<fenix_completion::CompletionItem> for Item {
    fn from(item: fenix_completion::CompletionItem) -> Self {
        let source = match item.kind {
            fenix_completion::CompletionKind::Keyword => Source::Keyword,
            fenix_completion::CompletionKind::Tag => Source::Symbol,
            fenix_completion::CompletionKind::Lsp => Source::Lsp,
        };
        Self { detail: item.detail, ..Self::text(item.label, source) }
    }
}

impl Item {
    pub fn text(label: String, source: Source) -> Self {
        Self {
            insertion: Insertion::Text(label.clone()),
            label,
            source,
            detail: String::new(),
            documentation: String::new(),
        }
    }

    pub fn snippet(snippet: &fenix_snippets::Snippet) -> Self {
        Self {
            label: snippet.trigger.clone(),
            source: Source::Snippet,
            detail: snippet.name.clone(),
            documentation: snippet
                .template
                .render(&Default::default(), &fenix_snippets::Context::default(), "")
                .text,
            insertion: Insertion::Snippet(snippet.template.clone()),
        }
    }

    pub fn lsp(item: lsp_types::CompletionItem) -> Option<Self> {
        // Native snippets deliberately aren't a full LSP snippet grammar.
        // We advertise plain text only, rather than insert unsupported syntax.
        if item.insert_text_format == Some(lsp_types::InsertTextFormat::SNIPPET) {
            return None;
        }
        let documentation = match &item.documentation {
            Some(lsp_types::Documentation::String(s)) => s.clone(),
            Some(lsp_types::Documentation::MarkupContent(s)) => s.value.clone(),
            None => String::new(),
        };
        Some(Self {
            label: item.label.clone(),
            source: Source::Lsp,
            detail: item.detail.clone().unwrap_or_default(),
            documentation,
            insertion: Insertion::Lsp(Box::new(item)),
        })
    }
}

/// Keep native snippets distinct from code completions with the same label;
/// server results replace redundant local words/keywords. Stable tie order.
pub fn merge(
    local: Vec<fenix_picker::Candidate<Item>>,
    server: Vec<Item>,
) -> Vec<fenix_picker::Candidate<Item>> {
    let mut seen = HashSet::new();
    server
        .into_iter()
        .map(|item| fenix_picker::Candidate::new(item.label.clone(), item))
        .chain(local)
        .filter(|item| {
            seen.insert((
                item.payload.label.clone(),
                item.payload.source == Source::Snippet,
            ))
        })
        .collect()
}

pub fn clipped_line(text: &str, width: usize) -> String {
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() <= width {
        return line;
    }
    if width == 0 {
        return String::new();
    }
    line.chars()
        .take(width - 1)
        .chain(std::iter::once('…'))
        .collect()
}

pub fn scroll_offset(selected: usize, previous: usize, rows: usize) -> usize {
    previous
        .min(selected)
        .max(selected.saturating_sub(rows.max(1) - 1))
}

/// Validate all completion edits before mutating anything, then apply them as
/// one undoable replacement. Additional edits (e.g. imports) share this undo.
pub fn apply_lsp(
    buffer: &mut Buffer,
    cursor: &mut Cursor,
    fallback: std::ops::Range<usize>,
    item: &lsp_types::CompletionItem,
) -> Result<(), String> {
    let convert = |range: lsp_types::Range| -> Result<std::ops::Range<usize>, String> {
        let start = fenix_lsp::position_to_char_offset(buffer.rope(), range.start);
        let end = fenix_lsp::position_to_char_offset(buffer.rope(), range.end);
        if start > end
            || fenix_lsp::char_offset_to_position(buffer.rope(), start) != range.start
            || fenix_lsp::char_offset_to_position(buffer.rope(), end) != range.end
        {
            return Err("completion has an invalid edit range".into());
        }
        Ok(start..end)
    };
    let (main, text) = match &item.text_edit {
        Some(lsp_types::CompletionTextEdit::Edit(edit)) => {
            (convert(edit.range)?, edit.new_text.clone())
        }
        Some(lsp_types::CompletionTextEdit::InsertAndReplace(edit)) => {
            (convert(edit.replace)?, edit.new_text.clone())
        }
        None => (
            fallback,
            item.insert_text
                .clone()
                .unwrap_or_else(|| item.label.clone()),
        ),
    };
    if main.start > cursor.char_idx || main.end < cursor.char_idx {
        return Err("completion edit does not contain the cursor".into());
    }
    let mut edits = vec![(main.clone(), text, true)];
    for edit in item.additional_text_edits.iter().flatten() {
        edits.push((convert(edit.range)?, edit.new_text.clone(), false));
    }
    edits.sort_by_key(|(range, _, _)| (range.start, range.end));
    if edits
        .windows(2)
        .any(|pair| pair[0].0.end > pair[1].0.start || pair[0].0.start == pair[1].0.start)
    {
        return Err("completion contains overlapping edits".into());
    }
    let start = edits.first().unwrap().0.start;
    let end = edits.last().unwrap().0.end;
    let mut result = String::new();
    let mut at = start;
    let mut caret = start;
    for (range, text, primary) in edits {
        result.push_str(&buffer.text_range(at, range.start));
        result.push_str(&text);
        if primary {
            caret = start + result.chars().count();
        }
        at = range.end;
    }
    buffer.replace_range(cursor, start, end, &result);
    cursor.char_idx = caret;
    cursor.sticky_col = buffer.line_col(cursor).1;
    Ok(())
}

/// `:` is included specifically for Tcl's `::` namespace separator --
/// ctags-sourced candidates are labeled with their fully-qualified name
/// (`myns::greet`), so `ns::gr` needs to survive as one prefix, not get
/// truncated to just `gr` after the last `::` (which would both widen
/// the fuzzy match to anything containing "gr" anywhere, and -- worse --
/// make `accept_completion` replace only the `gr` part, leaving the
/// typed `ns::` in place and producing a duplicated `ns::myns::greet`).
/// A lone `:` has no meaning in Tcl outside that pair, so there's no
/// real string this could misinterpret.
fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == ':'
}

/// The identifier prefix ending exactly at the cursor -- e.g. with the
/// cursor right after `se` in `se|t`, returns `(prefix_start_char_idx,
/// "se")`, or after `gr` in `myns::gr|` returns the whole `myns::gr`
/// (see `is_ident_char`'s own doc comment for why `::` stays part of
/// the prefix rather than resetting at each `:`). Scans backward only:
/// unlike `fenix-vim`'s `textobject::span` (built for Normal-mode text
/// objects, where the cursor sits *inside* a word and both directions
/// matter), nothing after the cursor in Insert mode is part of what's
/// being typed. `None` when the cursor isn't immediately preceded by an
/// identifier char (on whitespace, punctuation, buffer start, or an
/// empty buffer) -- the completion popup has nothing to filter by and
/// should stay closed.
pub fn prefix_at_cursor(buffer: &Buffer, cursor: &Cursor) -> Option<(usize, String)> {
    let end = cursor.char_idx.min(buffer.len_chars());
    let mut start = end;
    while start > 0 && buffer.char_at(start - 1).is_some_and(is_ident_char) {
        start -= 1;
    }
    if start == end {
        return None;
    }
    let prefix: String = (start..end).map(|i| buffer.char_at(i).unwrap()).collect();
    Some((start, prefix))
}

/// Best-effort "is the cursor typing inside a string literal that
/// hasn't been closed yet" check, purely textual -- counts unescaped
/// `"` characters from the start of the cursor's line up to the
/// cursor; an odd count means an opening quote has been typed with no
/// closing one yet. This exists because a tree-sitter parse can't help
/// here the way it can for a comment: an *unterminated* string almost
/// always parses as an `ERROR` node with no useful capture at all
/// (verified against the Tcl grammar -- `puts "se` produces `(ERROR
/// (simple_word))`, not a `quoted_word` node), so a capture-based check
/// only ever catches a string that's already been *closed* elsewhere
/// on the line (e.g. the cursor landed back inside an existing one).
/// This is what catches the much more common case: actively typing a
/// brand new string from scratch. A disclosed simplification, not a
/// real lexer: single-line and `"`-only (doesn't know about `'` or a
/// language's own triple-quote/heredoc syntax), so it can go wrong for
/// a multi-line string or a language that doesn't use `"` at all --
/// acceptable for a completion-suppression heuristic, not attempted
/// for anything that needs to be exactly right.
pub fn cursor_in_unterminated_string(buffer: &Buffer, cursor: &Cursor) -> bool {
    let (line, col) = buffer.line_col(cursor);
    let line_start = buffer.line_start_char(line);
    let mut quote_count = 0u32;
    let mut i = line_start;
    while i < line_start + col {
        match buffer.char_at(i) {
            Some('\\') => i += 1, // skip whatever's escaped, quote or not
            Some('"') => quote_count += 1,
            _ => {}
        }
        i += 1;
    }
    quote_count % 2 == 1
}

/// Every distinct identifier-like token already present in `buffer` --
/// the "complete a word from what's already been typed in this file"
/// source real Vim's own `<C-n>`/`<C-p>` draws on, and the only
/// completion source that isn't Tcl-specific (see `App::completion_
/// candidates`, which layers this on top of the Tcl keyword/ctags pool
/// for a Tcl buffer, or uses it alone for anything else). Purely-numeric
/// tokens (`123`) are skipped -- not something worth ever completing to.
/// Scans the whole buffer on every call, same "just clone the text" cost
/// as `Buffer::text()`'s other callers (`format_buffer`, syntax
/// highlighting) -- fine at the size files are actually edited at,
/// expensive on a huge one; not attempting anything cleverer here.
/// Iteration order is whatever `HashSet` happens to produce -- callers
/// already fuzzy-filter/sort the result, so token order was never
/// meaningful.
pub fn buffer_words(buffer: &Buffer) -> HashSet<String> {
    let text = buffer.text();
    text.split(|c: char| !is_ident_char(c))
        .filter(|w| !w.is_empty() && !w.chars().all(|c| c.is_ascii_digit()))
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buf(text: &str) -> Buffer {
        Buffer::from_text(text)
    }

    fn cur(char_idx: usize) -> Cursor {
        Cursor {
            char_idx,
            sticky_col: 0,
        }
    }

    #[test]
    fn cursor_mid_word_returns_the_prefix_up_to_the_cursor() {
        let buffer = buf("set");
        // cursor between "se" and "t"
        let (start, prefix) = prefix_at_cursor(&buffer, &cur(2)).unwrap();
        assert_eq!(start, 0);
        assert_eq!(prefix, "se");
    }

    #[test]
    fn cursor_right_after_a_full_word_returns_the_whole_word() {
        let buffer = buf("puts");
        let (start, prefix) = prefix_at_cursor(&buffer, &cur(4)).unwrap();
        assert_eq!(start, 0);
        assert_eq!(prefix, "puts");
    }

    #[test]
    fn cursor_on_whitespace_returns_none() {
        let buffer = buf("set x");
        // cursor right after "set " (on the space, before "x")
        let (_, none_or_x) = match prefix_at_cursor(&buffer, &cur(4)) {
            Some((s, p)) => (s, p),
            None => return,
        };
        panic!("expected None, got prefix starting elsewhere: {none_or_x}");
    }

    #[test]
    fn cursor_right_after_punctuation_returns_none() {
        let buffer = buf("foo.");
        assert!(prefix_at_cursor(&buffer, &cur(4)).is_none());
    }

    #[test]
    fn cursor_at_buffer_start_returns_none() {
        let buffer = buf("set");
        assert!(prefix_at_cursor(&buffer, &cur(0)).is_none());
    }

    #[test]
    fn empty_buffer_returns_none() {
        let buffer = buf("");
        assert!(prefix_at_cursor(&buffer, &cur(0)).is_none());
    }

    #[test]
    fn namespace_qualified_prefix_keeps_the_double_colon_and_everything_before_it() {
        let buffer = buf("myns::gr");
        let (start, prefix) = prefix_at_cursor(&buffer, &cur(8)).unwrap();
        assert_eq!(start, 0);
        assert_eq!(prefix, "myns::gr");
    }

    #[test]
    fn namespace_qualified_prefix_with_multiple_levels_keeps_the_whole_path() {
        let buffer = buf("outer::inner::pr");
        let (start, prefix) = prefix_at_cursor(&buffer, &cur(17)).unwrap();
        assert_eq!(start, 0);
        assert_eq!(prefix, "outer::inner::pr");
    }

    #[test]
    fn underscore_is_an_identifier_char() {
        let buffer = buf("my_var");
        let (start, prefix) = prefix_at_cursor(&buffer, &cur(6)).unwrap();
        assert_eq!(start, 0);
        assert_eq!(prefix, "my_var");
    }

    #[test]
    fn cursor_in_unterminated_string_is_false_with_no_quotes_on_the_line() {
        let buffer = buf("puts se");
        assert!(!cursor_in_unterminated_string(&buffer, &cur(7)));
    }

    #[test]
    fn cursor_in_unterminated_string_is_true_right_after_an_opening_quote() {
        let buffer = buf("puts \"se");
        assert!(cursor_in_unterminated_string(&buffer, &cur(8)));
    }

    #[test]
    fn cursor_in_unterminated_string_is_false_after_a_closed_string() {
        let buffer = buf("puts \"hi\" se");
        assert!(!cursor_in_unterminated_string(&buffer, &cur(12)));
    }

    #[test]
    fn cursor_in_unterminated_string_ignores_an_escaped_quote() {
        // `\"` doesn't close the string it's inside -- still one open
        // quote (the very first one), not two.
        let buffer = buf("puts \"a\\\"se");
        assert!(cursor_in_unterminated_string(&buffer, &cur(11)));
    }

    #[test]
    fn cursor_in_unterminated_string_only_looks_at_the_current_line() {
        // A quote left open on an earlier line shouldn't leak into the
        // next one -- this heuristic is deliberately single-line only
        // (see its own doc comment).
        let buffer = buf("puts \"unterminated\nse");
        assert!(!cursor_in_unterminated_string(&buffer, &cur(21)));
    }

    #[test]
    fn buffer_words_finds_every_distinct_identifier_token() {
        let buffer = buf("let seek_value = 1;\nlet other = seek_value;\n");
        let words = buffer_words(&buffer);
        assert!(words.contains("let"));
        assert!(words.contains("seek_value"));
        assert!(words.contains("other"));
        // "seek_value" appears twice but is only one distinct word.
        assert_eq!(words.iter().filter(|w| *w == "seek_value").count(), 1);
    }

    #[test]
    fn buffer_words_skips_purely_numeric_tokens() {
        let buffer = buf("let x = 123;\nlet y2 = 4;\n");
        let words = buffer_words(&buffer);
        assert!(!words.contains("123"));
        assert!(!words.contains("4"));
        assert!(words.contains("y2")); // alphanumeric but not *purely* digits
    }

    #[test]
    fn buffer_words_is_empty_for_an_empty_buffer() {
        let buffer = buf("");
        assert!(buffer_words(&buffer).is_empty());
    }

    #[test]
    fn buffer_words_ignores_punctuation_and_whitespace_boundaries() {
        let buffer = buf("baz.qux(quux)");
        let words = buffer_words(&buffer);
        assert_eq!(
            words,
            HashSet::from(["baz", "qux", "quux"].map(str::to_string))
        );
    }

    #[test]
    fn buffer_words_keeps_a_namespace_qualified_name_as_one_token() {
        let buffer = buf("foo::bar, baz");
        let words = buffer_words(&buffer);
        assert_eq!(
            words,
            HashSet::from(["foo::bar", "baz"].map(str::to_string))
        );
    }
    #[test]
    fn selection_scrolls_both_directions() {
        assert_eq!(scroll_offset(17, 0, 10), 8);
        assert_eq!(scroll_offset(3, 8, 10), 3);
        assert_eq!(clipped_line("αβγδε", 3), "αβ…");
    }

    #[test]
    fn lsp_edits_use_utf16_and_keep_import_and_replacement_in_one_undo() {
        let mut buffer = buf("😀\npri");
        let mut cursor = Cursor::default();
        cursor.char_idx = 5;
        let item: lsp_types::CompletionItem = serde_json::from_value(serde_json::json!({
            "label": "print(value)", "textEdit": {
                "range": {"start":{"line":1,"character":0},"end":{"line":1,"character":3}},
                "newText":"print"
            }, "additionalTextEdits": [{
                "range":{"start":{"line":0,"character":0},"end":{"line":0,"character":0}},
                "newText":"import\n"
            }]
        }))
        .unwrap();
        apply_lsp(&mut buffer, &mut cursor, 2..5, &item).unwrap();
        assert_eq!(buffer.text(), "import\n😀\nprint");
        assert_eq!(cursor.char_idx, 14);
        buffer.undo(&mut cursor);
        assert_eq!(buffer.text(), "😀\npri");
    }

    #[test]
    fn lsp_insert_text_and_invalid_edits() {
        let mut buffer = buf("pr");
        let mut cursor = Cursor::default();
        cursor.char_idx = 2;
        let mut item = lsp_types::CompletionItem {
            label: "print(value)".into(),
            insert_text: Some("print".into()),
            ..Default::default()
        };
        apply_lsp(&mut buffer, &mut cursor, 0..2, &item).unwrap();
        assert_eq!(buffer.text(), "print");
        item.additional_text_edits = Some(vec![lsp_types::TextEdit {
            range: lsp_types::Range::new(
                lsp_types::Position::new(0, 0),
                lsp_types::Position::new(0, 2),
            ),
            new_text: "bad".into(),
        }]);
        assert!(apply_lsp(&mut buffer, &mut cursor, 0..5, &item).is_err());
        assert_eq!(buffer.text(), "print");
    }
}
