//! Renders a PDF text search's flat match list (`fenix_pdf::search::
//! PdfSearchMatch`) into real buffer text plus per-line metadata -- same
//! "generated text + a side table describing each line" split `pdf_
//! outline.rs` uses for the bookmark panel.

/// Per-line metadata for one line of `render`'s returned text, at the
/// matching index in its `lines`. Only `page_index` is kept (not `char_
/// index`/`context`) -- jumping to a result only ever needs the page, and
/// nothing here re-derives the row's own text back out of the buffer, so
/// there's no reason to duplicate what the rendered line already shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PdfSearchResultLine {
    pub page_index: u32,
}

/// One line per match, formatted `p.NNN  <context>` (1-indexed page
/// number, matching the 1-indexed page the goto-page prompt and title
/// bar already show the user elsewhere in this feature). Given no
/// matches at all, the text is a single explanatory line and `lines` is
/// `[None]` -- same "explain the empty state instead of just looking
/// blank" posture as `pdf_outline::render`'s own empty case.
pub fn render(query: &str, matches: &[fenix_pdf::search::PdfSearchMatch]) -> (String, Vec<Option<PdfSearchResultLine>>) {
    if matches.is_empty() {
        return (format!("(no matches for \"{query}\")"), vec![None]);
    }
    let mut text = String::new();
    let mut lines = Vec::with_capacity(matches.len());
    for m in matches {
        text.push_str(&format!("p.{:>3}  {}\n", m.page_index + 1, m.context));
        lines.push(Some(PdfSearchResultLine { page_index: m.page_index }));
    }
    (text, lines)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fenix_pdf::search::PdfSearchMatch;

    #[test]
    fn render_formats_one_line_per_match_with_a_1_indexed_page_number() {
        let matches = vec![
            PdfSearchMatch { page_index: 0, char_index: 5, context: "the quick brown fox".to_string() },
            PdfSearchMatch { page_index: 6, char_index: 0, context: "jumps over the lazy dog".to_string() },
        ];
        let (text, lines) = render("fox", &matches);
        assert_eq!(text, "p.  1  the quick brown fox\np.  7  jumps over the lazy dog\n");
        assert_eq!(lines, vec![Some(PdfSearchResultLine { page_index: 0 }), Some(PdfSearchResultLine { page_index: 6 })]);
    }

    #[test]
    fn render_with_no_matches_explains_itself_instead_of_being_blank() {
        let (text, lines) = render("xylophone", &[]);
        assert_eq!(text, "(no matches for \"xylophone\")");
        assert_eq!(lines, vec![None]);
    }
}
