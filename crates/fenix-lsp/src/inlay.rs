//! Inlay hints (`textDocument/inlayHint`): what a language server would
//! write into the text without it being there -- a variable's type after
//! its name (`: Vec<String>`), a parameter's name before an argument
//! (`count:`). Pure: what a server offers, what Fenix advertises, and an
//! answer turned into places in the text with the label drawn there.
//! `App` asks for the lines in view and draws them dimmed, in the line,
//! as the debugger's values are.

/// Whether `capabilities` offer inlay hints.
pub fn offers(capabilities: &lsp_types::ServerCapabilities) -> bool {
    match &capabilities.inlay_hint_provider {
        Some(lsp_types::OneOf::Left(on)) => *on,
        Some(lsp_types::OneOf::Right(_)) => true,
        None => false,
    }
}

/// What Fenix advertises for documents: hints, never resolved (a hint's
/// tooltip and its edits aren't shown).
pub fn client_capabilities() -> lsp_types::InlayHintClientCapabilities {
    lsp_types::InlayHintClientCapabilities { dynamic_registration: Some(false), resolve_support: None }
}

/// What Fenix advertises for the workspace: it asks again when told to
/// (`workspace/inlayHint/refresh`).
pub fn workspace_capabilities() -> lsp_types::InlayHintWorkspaceClientCapabilities {
    lsp_types::InlayHintWorkspaceClientCapabilities { refresh_support: Some(true) }
}

/// The longest label drawn, in chars; a longer one is cut with `…`.
pub const MAX_LABEL: usize = 48;

/// One hint, placed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placed {
    /// The char offset into the document it's drawn at (before the char
    /// there).
    pub at: usize,
    /// What's drawn, its padding included.
    pub text: String,
    /// A parameter's name, before what follows it: it stays put while you
    /// type there, so what you type goes after it, into the argument. A
    /// type's goes along with what's typed before it.
    pub before_text: bool,
}

/// A hint's label as drawn: its parts joined, control characters and
/// line breaks made spaces, cut at `MAX_LABEL`, with the padding the
/// server asked for.
pub fn label(hint: &lsp_types::InlayHint) -> String {
    let raw = match &hint.label {
        lsp_types::InlayHintLabel::String(s) => s.clone(),
        lsp_types::InlayHintLabel::LabelParts(parts) => parts.iter().map(|p| p.value.as_str()).collect(),
    };
    let mut text: String = raw.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    if text.chars().count() > MAX_LABEL {
        text = text.chars().take(MAX_LABEL - 1).collect::<String>() + "…";
    }
    let left = if hint.padding_left == Some(true) { " " } else { "" };
    let right = if hint.padding_right == Some(true) { " " } else { "" };
    format!("{left}{text}{right}")
}

/// An answer's hints placed in `rope` (the text they were asked of), in
/// order; a hint with nothing to draw is left out.
pub fn place(hints: &[lsp_types::InlayHint], rope: &ropey::Rope) -> Vec<Placed> {
    let mut placed: Vec<Placed> = hints
        .iter()
        .filter_map(|hint| {
            let text = label(hint);
            (!text.trim().is_empty()).then(|| Placed {
                at: crate::position_to_char_offset(rope, hint.position),
                text,
                before_text: hint.kind == Some(lsp_types::InlayHintKind::PARAMETER),
            })
        })
        .collect();
    placed.sort_by_key(|p| p.at);
    placed
}

/// The request for lines `lines` (0-based, end exclusive) of `uri`, in a
/// document of `line_count` lines: to the end of the last line asked.
pub fn params(uri: lsp_types::Uri, lines: std::ops::Range<usize>, line_count: usize) -> lsp_types::InlayHintParams {
    let end = lines.end.min(line_count.max(1));
    lsp_types::InlayHintParams {
        text_document: lsp_types::TextDocumentIdentifier { uri },
        range: lsp_types::Range { start: lsp_types::Position { line: lines.start as u32, character: 0 }, end: lsp_types::Position { line: end as u32, character: 0 } },
        work_done_progress_params: Default::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hint(line: u32, character: u32, label: lsp_types::InlayHintLabel, kind: Option<lsp_types::InlayHintKind>) -> lsp_types::InlayHint {
        lsp_types::InlayHint {
            position: lsp_types::Position { line, character },
            label,
            kind,
            text_edits: None,
            tooltip: None,
            padding_left: None,
            padding_right: None,
            data: None,
        }
    }

    #[test]
    fn a_server_offers_hints_by_a_flag_or_options() {
        let mut caps = lsp_types::ServerCapabilities::default();
        assert!(!offers(&caps));
        caps.inlay_hint_provider = Some(lsp_types::OneOf::Left(false));
        assert!(!offers(&caps));
        caps.inlay_hint_provider = Some(lsp_types::OneOf::Left(true));
        assert!(offers(&caps));
        caps.inlay_hint_provider = Some(lsp_types::OneOf::Right(lsp_types::InlayHintServerCapabilities::Options(Default::default())));
        assert!(offers(&caps));
    }

    #[test]
    fn labels_are_joined_padded_cleaned_and_cut() {
        let parts = lsp_types::InlayHintLabel::LabelParts(vec![
            lsp_types::InlayHintLabelPart { value: ": Vec<".into(), ..Default::default() },
            lsp_types::InlayHintLabelPart { value: "String>".into(), ..Default::default() },
        ]);
        assert_eq!(label(&hint(0, 0, parts, None)), ": Vec<String>");
        let mut padded = hint(0, 0, lsp_types::InlayHintLabel::String("count:".into()), None);
        padded.padding_right = Some(true);
        padded.padding_left = Some(true);
        assert_eq!(label(&padded), " count: ");
        assert_eq!(label(&hint(0, 0, lsp_types::InlayHintLabel::String("a\nb\tc".into()), None)), "a b c");
        let long = label(&hint(0, 0, lsp_types::InlayHintLabel::String("x".repeat(100)), None));
        assert_eq!(long.chars().count(), MAX_LABEL);
        assert!(long.ends_with('…'));
    }

    /// Positions are UTF-16, as the protocol says; parameters stay before
    /// what follows them.
    #[test]
    fn hints_are_placed_in_order_by_utf16_position() {
        let rope = ropey::Rope::from_str("let 😀x = f(1);\nlet y = 2;\n");
        let hints = [
            hint(1, 5, lsp_types::InlayHintLabel::String(": i32".into()), Some(lsp_types::InlayHintKind::TYPE)),
            hint(0, 7, lsp_types::InlayHintLabel::String(": u8".into()), Some(lsp_types::InlayHintKind::TYPE)),
            hint(0, 12, lsp_types::InlayHintLabel::String("n:".into()), Some(lsp_types::InlayHintKind::PARAMETER)),
            hint(0, 0, lsp_types::InlayHintLabel::String("  ".into()), None),
        ];
        let placed = place(&hints, &rope);
        assert_eq!(placed.len(), 3, "an empty label is left out");
        assert_eq!((placed[0].at, placed[0].text.as_str(), placed[0].before_text), (6, ": u8", false), "after 😀x, two UTF-16 units for 😀");
        assert_eq!((placed[1].at, placed[1].before_text), (11, true));
        assert_eq!(placed[2].at, rope.line_to_char(1) + 5);
    }

    #[test]
    fn a_range_is_asked_to_the_end_of_its_last_line() {
        let uri: lsp_types::Uri = "file:///a.rs".parse().unwrap();
        let p = params(uri, 10..60, 40);
        assert_eq!((p.range.start.line, p.range.end.line, p.range.end.character), (10, 40, 0));
    }
}
