//! Renders a PDF's flattened bookmark tree (`fenix_pdf::outline::
//! OutlineEntry`) into real buffer text plus per-line metadata -- same
//! "generated text + a side table describing each line" split
//! `dashboard.rs` uses for the startup dashboard.

/// Per-line metadata for one line of `render`'s returned text, at the
/// matching index in its `lines`. `None` (rather than an entry in
/// `PdfOutlineLine`) would mean "this line is blank/unstyled", but an
/// outline currently has no blank lines -- kept as `Option` anyway to
/// match `App::pdf_outline_lines`' `Vec<Option<PdfOutlineLine>>` shape
/// (mirrors `dashboard_lines`), so a future blank separator line (e.g.
/// between top-level sections) doesn't need a shape change later.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PdfOutlineLine {
    pub page_index: u32,
}

/// One line per entry, indented two spaces per `depth` level so nesting
/// reads visually without needing a tree widget this editor doesn't
/// have. Given an empty `entries` (a PDF with no bookmarks at all), the
/// text is a single explanatory line and `lines` is `[None]`, so `Enter`
/// on it is a harmless no-op rather than the pane just looking blank
/// with no explanation.
pub fn render(entries: &[fenix_pdf::outline::OutlineEntry]) -> (String, Vec<Option<PdfOutlineLine>>) {
    if entries.is_empty() {
        return ("(this PDF has no bookmarks)".to_string(), vec![None]);
    }
    let mut text = String::new();
    let mut lines = Vec::with_capacity(entries.len());
    for entry in entries {
        text.push_str(&"  ".repeat(entry.depth as usize));
        text.push_str(&entry.title);
        text.push('\n');
        lines.push(Some(PdfOutlineLine { page_index: entry.page_index }));
    }
    (text, lines)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fenix_pdf::outline::OutlineEntry;

    #[test]
    fn render_indents_by_depth_and_tags_each_line_with_its_page() {
        let entries = vec![
            OutlineEntry { title: "Chapter 1".to_string(), page_index: 0, depth: 0 },
            OutlineEntry { title: "Section 1.1".to_string(), page_index: 1, depth: 1 },
            OutlineEntry { title: "Chapter 2".to_string(), page_index: 5, depth: 0 },
        ];
        let (text, lines) = render(&entries);
        assert_eq!(text, "Chapter 1\n  Section 1.1\nChapter 2\n");
        assert_eq!(
            lines,
            vec![
                Some(PdfOutlineLine { page_index: 0 }),
                Some(PdfOutlineLine { page_index: 1 }),
                Some(PdfOutlineLine { page_index: 5 }),
            ]
        );
    }

    #[test]
    fn render_an_empty_outline_explains_itself_instead_of_being_blank() {
        let (text, lines) = render(&[]);
        assert_eq!(text, "(this PDF has no bookmarks)");
        assert_eq!(lines, vec![None]);
    }

    #[test]
    fn render_indents_deep_nesting_by_two_spaces_per_level() {
        let entries = vec![OutlineEntry { title: "Deep".to_string(), page_index: 3, depth: 3 }];
        let (text, _) = render(&entries);
        assert_eq!(text, "      Deep\n");
    }
}
