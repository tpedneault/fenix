//! Plain word-wrapping for generated panel text -- the Docker/Git/Jira
//! panels' own `label: value` detail rows and Jira's issue description/
//! comment prose, specifically (see each panel module's own call
//! sites), *not* their list rows (Containers/Images/Files/Branches/
//! Commits/...) or Git's diff pane, which stay single-line/unwrapped on
//! purpose: a wrapped list row would misalign against its neighbors the
//! same way `docker_panel::render_containers`' own doc comment already
//! rejected letting a row's content get clipped instead of trimmed, and
//! wrapping a unified diff would break the one-line-per-change
//! convention every diff viewer (this app's own included) relies on to
//! stay readable.
//!
//! This has no notion of the pane it'll actually be rendered in --
//! there's no wrap-aware rendering pipeline in this app (every screen
//! row still maps 1:1 to one buffer line; see `app.rs`'s `content_
//! spans`), so `width` is a fixed, generous approximation of a typical
//! split's width rather than the pane's real current column count.
//! Good enough to keep long prose (a Jira description, a long Docker
//! mount path) from silently running off the pane's right edge and
//! being unreadable; genuinely dynamic wrapping would need `content_
//! spans` itself to know about wrap points, which none of Docker/Git/
//! Jira's panel generation does today.

/// The wrap width panel builders default to, in characters -- a
/// comfortable prose-reading width (`rustfmt`'s own default `max_width`
/// is the same number) that most real split widths in this app
/// comfortably exceed, so most short `label: value` rows never wrap at
/// all; only genuinely long values/paragraphs do.
pub const DEFAULT_WRAP_WIDTH: usize = 100;

/// Wraps `text` to `width` columns: primarily at whitespace runs
/// (`split_whitespace`, so wrapped text has its internal whitespace
/// collapsed to single spaces, an unavoidable side effect of splitting
/// into words at all -- exactly what word-wrapped prose already
/// implies), but a single token that's *itself* longer than `width` --
/// a Docker bind-mount path, a long image digest, anything with no
/// spaces at all -- is hard-broken into `width`-sized chunks rather
/// than left whole and still overflowing the pane, which would defeat
/// the entire point of wrapping for exactly the case it matters most
/// for. Text that's already no wider than `width` needs none of this
/// and is returned completely untouched, spacing included -- never
/// zero lines either, so a blank paragraph-break line still round-trips
/// as its own blank line rather than disappearing.
pub fn wrap_text(text: &str, width: usize) -> Vec<String> {
    if width == 0 || text.chars().count() <= width {
        return vec![text.to_string()];
    }
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut current_len = 0usize;
    for word in text.split_whitespace() {
        let word_len = word.chars().count();
        if word_len <= width {
            place(&mut lines, &mut current, &mut current_len, word, word_len, width);
            continue;
        }
        // `word` alone doesn't fit even on an empty line: flush
        // whatever's building first, then hard-break it into
        // `width`-sized chunks, each pushed directly as its own line --
        // including the final, possibly-shorter remainder, which stays
        // on its own line rather than being handed to `place` for the
        // next real word to pack onto. That keeps a hard break visually
        // unambiguous (the fragment never silently merges with an
        // unrelated following word) at the cost of a little wasted
        // width on that last, short line -- a fine trade for text this
        // is never expecting to need hard-breaking often to begin with.
        if !current.is_empty() {
            lines.push(std::mem::take(&mut current));
            current_len = 0;
        }
        let mut remaining = word;
        while !remaining.is_empty() {
            let split_at = remaining.char_indices().nth(width).map(|(i, _)| i).unwrap_or(remaining.len());
            lines.push(remaining[..split_at].to_string());
            remaining = &remaining[split_at..];
        }
    }
    if !current.is_empty() || lines.is_empty() {
        lines.push(current);
    }
    lines
}

/// Appends `word` (already known to fit within `width` on its own) to
/// `current`, starting a fresh line first if it wouldn't fit alongside
/// what's already there.
fn place(lines: &mut Vec<String>, current: &mut String, current_len: &mut usize, word: &str, word_len: usize, width: usize) {
    if current.is_empty() {
        current.push_str(word);
        *current_len = word_len;
    } else if *current_len + 1 + word_len <= width {
        current.push(' ');
        current.push_str(word);
        *current_len += 1 + word_len;
    } else {
        lines.push(std::mem::take(current));
        current.push_str(word);
        *current_len = word_len;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_no_wider_than_width_is_returned_unchanged_as_one_line() {
        assert_eq!(wrap_text("short", 10), vec!["short".to_string()]);
    }

    #[test]
    fn empty_text_yields_one_empty_line_not_zero_lines() {
        assert_eq!(wrap_text("", 10), vec![String::new()]);
    }

    #[test]
    fn wraps_at_the_last_word_boundary_that_fits() {
        assert_eq!(wrap_text("the quick brown fox jumps", 11), vec!["the quick".to_string(), "brown fox".to_string(), "jumps".to_string()]);
    }

    #[test]
    fn a_single_token_longer_than_width_is_hard_broken_into_width_sized_chunks() {
        let long_token = "a".repeat(25);
        let result = wrap_text(&long_token, 10);
        assert_eq!(result, vec!["a".repeat(10), "a".repeat(10), "a".repeat(5)]);
    }

    #[test]
    fn a_long_token_flushes_the_line_being_built_before_starting_its_own_chunks() {
        let long_token = "b".repeat(25);
        let result = wrap_text(&format!("short {long_token}"), 10);
        assert_eq!(result, vec!["short".to_string(), "b".repeat(10), "b".repeat(10), "b".repeat(5)]);
    }

    #[test]
    fn wrapping_continues_normally_with_ordinary_words_after_a_hard_broken_token() {
        let long_token = "c".repeat(25);
        let result = wrap_text(&format!("{long_token} then normal words"), 10);
        assert_eq!(result, vec!["c".repeat(10), "c".repeat(10), "c".repeat(5), "then".to_string(), "normal".to_string(), "words".to_string()]);
    }

    #[test]
    fn text_already_within_width_is_preserved_byte_for_byte_including_its_own_spacing() {
        // No wrapping needed at all, so nothing about the text is
        // touched -- unlike the case below, where wrapping's own word-
        // splitting can't help but normalize spacing along the way.
        assert_eq!(wrap_text("a    b", 10), vec!["a    b".to_string()]);
    }

    #[test]
    fn wrapping_collapses_runs_of_internal_whitespace_to_a_single_space() {
        // Only text long enough to actually need wrapping goes through
        // `split_whitespace`, which is where the collapsing comes from --
        // an unavoidable side effect of splitting into words at all, not
        // something this function sets out to do to short text (see the
        // test above).
        assert_eq!(wrap_text("first    second    third    fourth", 12), vec!["first second".to_string(), "third fourth".to_string()]);
    }

    #[test]
    fn width_zero_returns_the_whole_text_as_one_line_rather_than_looping_forever() {
        assert_eq!(wrap_text("some text", 0), vec!["some text".to_string()]);
    }

    #[test]
    fn text_that_is_only_whitespace_and_within_width_is_preserved_verbatim() {
        // Caught by the short-circuit before word-splitting even runs --
        // a blank description line must stay a blank line, not vanish.
        assert_eq!(wrap_text("   ", 10), vec!["   ".to_string()]);
    }

    #[test]
    fn exactly_at_width_does_not_wrap() {
        assert_eq!(wrap_text("0123456789", 10), vec!["0123456789".to_string()]);
    }

    #[test]
    fn one_character_over_width_wraps() {
        assert_eq!(wrap_text("01234567 9x", 10), vec!["01234567".to_string(), "9x".to_string()]);
    }

    #[test]
    fn a_long_path_with_no_spaces_still_wraps_instead_of_overflowing() {
        // The actual motivating case: a Docker bind-mount path or a long
        // image digest has no whitespace at all for word-wrap to break
        // on -- it must still wrap, or this feature does nothing for
        // exactly the values it exists to handle.
        let path = format!("/var/lib/docker/volumes/{}/_data", "x".repeat(90));
        let result = wrap_text(&path, 40);
        assert!(result.len() > 1, "expected the long path to wrap onto multiple lines, got {result:?}");
        assert!(result.iter().all(|l| l.chars().count() <= 40));
        assert_eq!(result.concat(), path, "no characters should be lost across the wrap");
    }

    #[test]
    fn a_non_ascii_token_is_hard_broken_at_a_char_boundary_not_a_byte_boundary() {
        // café repeated -- multi-byte UTF-8 characters throughout, so a
        // byte-index split would panic or corrupt the string if this
        // used byte length/indices instead of char ones.
        let long_token = "café".repeat(10); // 40 chars, well over width
        let result = wrap_text(&long_token, 6);
        assert_eq!(result.concat(), long_token);
        assert!(result.iter().all(|l| l.chars().count() <= 6));
    }
}
