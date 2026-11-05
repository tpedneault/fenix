//! What a `textDocument/didChange` says. A server declares whether it
//! wants the whole document every time (`Full`) or only what changed
//! (`Incremental`); some only accept the latter -- arduino-language-
//! server panics on a full-text change -- so a client that always sends
//! the whole document can't talk to them at all.
//!
//! The editor doesn't track individual edits for this, so an
//! incremental change is worked out by comparing the text last sent with
//! the text now: everything between their common start and common end is
//! one replaced range. One range is always a correct description of the
//! change, whatever edits produced it.

use lsp_types::{TextDocumentContentChangeEvent, TextDocumentSyncCapability, TextDocumentSyncKind};

/// Whether `capabilities` asks for incremental changes.
pub fn wants_incremental(capabilities: &lsp_types::ServerCapabilities) -> bool {
    let kind = match &capabilities.text_document_sync {
        Some(TextDocumentSyncCapability::Kind(kind)) => Some(*kind),
        Some(TextDocumentSyncCapability::Options(options)) => options.change,
        None => None,
    };
    kind == Some(TextDocumentSyncKind::INCREMENTAL)
}

/// The change event turning `old` into `new`: a single ranged replace
/// when `incremental` and there's an `old` to compare with, the whole of
/// `new` otherwise.
pub fn change_event(old: Option<&str>, new: &str, incremental: bool) -> TextDocumentContentChangeEvent {
    match old {
        Some(old) if incremental => ranged_change(old, new),
        _ => TextDocumentContentChangeEvent { range: None, range_length: None, text: new.to_string() },
    }
}

fn ranged_change(old: &str, new: &str) -> TextDocumentContentChangeEvent {
    let old_chars: Vec<char> = old.chars().collect();
    let new_chars: Vec<char> = new.chars().collect();
    let mut prefix = old_chars.iter().zip(&new_chars).take_while(|(a, b)| a == b).count();
    let max_suffix = old_chars.len().min(new_chars.len()) - prefix;
    let mut suffix = old_chars.iter().rev().zip(new_chars.iter().rev()).take(max_suffix).take_while(|(a, b)| a == b).count();

    // Never split a `\r\n`: a position between its two halves isn't one
    // a server can make sense of.
    if prefix > 0 && old_chars[prefix - 1] == '\r' {
        prefix -= 1;
        suffix = suffix.min(old_chars.len().min(new_chars.len()) - prefix);
    }
    if suffix > 0 && old_chars[old_chars.len() - suffix] == '\n' && old_chars.len() - suffix > prefix && old_chars[old_chars.len() - suffix - 1] == '\r' {
        suffix -= 1;
    }

    let rope = ropey::Rope::from_str(old);
    let start = crate::char_offset_to_position(&rope, prefix);
    let end = crate::char_offset_to_position(&rope, old_chars.len() - suffix);
    let text: String = new_chars[prefix..new_chars.len() - suffix].iter().collect();
    TextDocumentContentChangeEvent { range: Some(lsp_types::Range { start, end }), range_length: None, text }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_types::{Position, Range};

    fn range(sl: u32, sc: u32, el: u32, ec: u32) -> Option<Range> {
        Some(Range { start: Position { line: sl, character: sc }, end: Position { line: el, character: ec } })
    }

    /// Applies a ranged change the way a server would, to check it
    /// really reproduces `new`.
    fn apply(old: &str, change: &TextDocumentContentChangeEvent) -> String {
        let rope = ropey::Rope::from_str(old);
        let r = change.range.unwrap();
        let start = crate::position_to_char_offset(&rope, r.start);
        let end = crate::position_to_char_offset(&rope, r.end);
        let mut out = rope.clone();
        out.remove(start..end);
        out.insert(start, &change.text);
        out.to_string()
    }

    #[test]
    fn an_insertion_is_an_empty_range_at_the_insertion_point() {
        let old = "void loop() {\n}\n";
        let new = "void loop() {\n  EEPROM.\n}\n";
        let change = change_event(Some(old), new, true);
        assert_eq!(change.range, range(1, 0, 1, 0));
        assert_eq!(change.text, "  EEPROM.\n");
        assert_eq!(apply(old, &change), new);
    }

    #[test]
    fn a_deletion_and_a_replacement_cover_only_what_changed() {
        let old = "int x = 1;\nint y = 2;\n";
        let deleted = change_event(Some(old), "int x = 1;\n", true);
        assert_eq!(deleted.range, range(1, 0, 2, 0));
        assert_eq!(deleted.text, "");

        let replaced = change_event(Some(old), "int x = 1;\nint why = 2;\n", true);
        assert_eq!(apply(old, &replaced), "int x = 1;\nint why = 2;\n");
        assert_eq!(replaced.text, "wh", "only the differing middle is sent");
    }

    #[test]
    fn columns_are_utf16_code_units() {
        let old = "// 😀 x\n";
        let new = "// 😀 yx\n";
        let change = change_event(Some(old), new, true);
        assert_eq!(change.range, range(0, 6, 0, 6), "the emoji is two UTF-16 units");
        assert_eq!(apply(old, &change), new);
    }

    #[test]
    fn a_crlf_is_never_split() {
        let old = "a\r\nb\r\n";
        let new = "a\r\nX\r\nb\r\n";
        let change = change_event(Some(old), new, true);
        assert_eq!(apply(old, &change), new);
        let start = change.range.unwrap().start;
        assert_eq!(start.character, 0, "the change starts at a line start, not between \\r and \\n: {start:?}");
    }

    #[test]
    fn repeated_characters_still_round_trip() {
        for (old, new) in [("aaaa", "aa"), ("aa", "aaaa"), ("", "abc"), ("abc", ""), ("same", "same"), ("\n\n", "\n\n\n")] {
            let change = change_event(Some(old), new, true);
            assert_eq!(apply(old, &change), new, "{old:?} -> {new:?}");
        }
    }

    #[test]
    fn full_sync_or_no_previous_text_sends_everything() {
        assert_eq!(change_event(Some("a"), "b", false).range, None);
        assert_eq!(change_event(None, "b", true).range, None);
        assert_eq!(change_event(None, "b", true).text, "b");
    }

    #[test]
    fn incremental_is_read_from_either_capability_shape() {
        let kind = |k| lsp_types::ServerCapabilities { text_document_sync: Some(TextDocumentSyncCapability::Kind(k)), ..Default::default() };
        assert!(wants_incremental(&kind(TextDocumentSyncKind::INCREMENTAL)));
        assert!(!wants_incremental(&kind(TextDocumentSyncKind::FULL)));
        let options = lsp_types::ServerCapabilities {
            text_document_sync: Some(TextDocumentSyncCapability::Options(lsp_types::TextDocumentSyncOptions {
                change: Some(TextDocumentSyncKind::INCREMENTAL),
                ..Default::default()
            })),
            ..Default::default()
        };
        assert!(wants_incremental(&options));
        assert!(!wants_incremental(&Default::default()));
    }
}
