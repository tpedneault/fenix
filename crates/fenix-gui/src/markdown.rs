//! Markdown-specific text logic that isn't Vim-modal or a generic
//! formatter -- list continuation (`SPC c x`'s checkbox toggle too) and
//! bracket-depth reindenting already live in `fenix-vim`/`fenix-format`
//! respectively, since neither needs anything Markdown-specific beyond
//! a line's own text shape. Heading parsing does need to actually know
//! it's looking at Markdown (see `App::picker_outline`'s own doc
//! comment on why that one's gated), so it lives here instead.

/// One ATX heading (`#` through `######`) at the start of `line_text`:
/// its level (1-6, the number of leading `#`s) and title (everything
/// after the required space, trimmed). `None` for a line that isn't a
/// heading -- no leading `#`, more than 6 of them (CommonMark doesn't
/// recognize a 7th-level heading; the line is just text starting with
/// `#######`), or a `#`-run with no space after it (`#!/bin/sh`-style
/// text, not a heading).
///
/// Deliberately doesn't strip CommonMark's optional closing sequence
/// (`## Title ##`) -- doing that correctly means only stripping a
/// trailing `#`-run when it's preceded by whitespace (otherwise `## C#`
/// would lose its own trailing `#`), and closing sequences are rare
/// enough in real-world Markdown that the extra parsing isn't worth the
/// risk of getting that distinction wrong. A closing sequence just
/// ends up as literal trailing text in the title instead.
pub fn parse_heading(line_text: &str) -> Option<(usize, String)> {
    let level = line_text.chars().take_while(|&c| c == '#').count();
    if level == 0 || level > 6 {
        return None;
    }
    let rest = &line_text[level..];
    let title = if rest.is_empty() { rest } else { rest.strip_prefix(' ')? };
    Some((level, title.trim().to_string()))
}

/// A heading's display label for the `SPC c o` outline picker -- its
/// title, indented two spaces per level below the first (matching the
/// PDF viewer's own outline panel convention, `pdf_outline::render`),
/// not the literal `#`s -- so nesting reads at a glance without the
/// visual noise of repeating hash marks down the whole list.
pub fn heading_label(level: usize, title: &str) -> String {
    format!("{}{}", "  ".repeat(level.saturating_sub(1)), title)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_heading_reads_every_level() {
        for level in 1..=6 {
            let line = format!("{} Title", "#".repeat(level));
            assert_eq!(parse_heading(&line), Some((level, "Title".to_string())));
        }
    }

    #[test]
    fn parse_heading_requires_a_space_after_the_hashes() {
        assert!(parse_heading("#!/bin/sh").is_none());
        assert!(parse_heading("#no-space").is_none());
    }

    #[test]
    fn parse_heading_rejects_more_than_six_hashes() {
        assert!(parse_heading("####### too deep").is_none());
    }

    #[test]
    fn parse_heading_rejects_a_line_with_no_hash_at_all() {
        assert!(parse_heading("just text").is_none());
        assert!(parse_heading("").is_none());
    }

    #[test]
    fn parse_heading_accepts_a_bare_hash_run_with_an_empty_title() {
        assert_eq!(parse_heading("###"), Some((3, String::new())));
    }

    #[test]
    fn parse_heading_trims_surrounding_whitespace_from_the_title() {
        assert_eq!(parse_heading("#   spaced out   "), Some((1, "spaced out".to_string())));
    }

    #[test]
    fn parse_heading_does_not_strip_a_closing_sequence() {
        // Disclosed simplification -- see this function's own doc
        // comment on why stripping it correctly isn't worth the risk
        // of also eating a genuine trailing "#" like "C#".
        assert_eq!(parse_heading("## Title ##"), Some((2, "Title ##".to_string())));
    }

    #[test]
    fn heading_label_indents_two_spaces_per_level_below_the_first() {
        assert_eq!(heading_label(1, "Top"), "Top");
        assert_eq!(heading_label(2, "Sub"), "  Sub");
        assert_eq!(heading_label(3, "SubSub"), "    SubSub");
    }
}
