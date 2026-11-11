//! `gq` and `gw`: lines wrapped at the text width, Vim's `formatoptions`
//! defaults read for code and Markdown. A paragraph is a run of non-blank
//! lines with the same comment leader; its words are filled greedily, one
//! space between them, each line under the width where a word allows.
//! The first line's indent and leader start every line after it; a list
//! item's (`- `, `1. `) lines after the first line up under its text.

/// The text width `gq` wraps at when none is set (`:set tw=N`): Vim's own
/// fallback, 79.
pub const DEFAULT_TEXT_WIDTH: usize = 79;

/// Comment leaders a line may start with, longest first.
const LEADERS: [&str; 9] = ["///", "//!", "//", "--", ";;", "#", ";", ">", "%"];

/// `lines` (no newlines in them) wrapped at `width`.
pub fn wrap_lines(lines: &[String], width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = &lines[i];
        if line.trim().is_empty() {
            out.push(String::new());
            i += 1;
            continue;
        }
        let (prefix, rest_prefix, leader) = prefixes(line);
        let mut words: Vec<&str> = line[prefix.len()..].split_whitespace().collect();
        i += 1;
        // The paragraph goes on while lines keep the same leader and
        // don't start a list item of their own.
        while i < lines.len() && !lines[i].trim().is_empty() {
            let (p, _, l) = prefixes(&lines[i]);
            if l != leader || list_marker(&lines[i]).is_some() {
                break;
            }
            words.extend(lines[i][p.len()..].split_whitespace());
            i += 1;
        }
        let mut current = prefix.clone();
        let mut has_word = false;
        for word in words {
            let len = current.chars().count() + usize::from(has_word) + word.chars().count();
            if has_word && len > width {
                out.push(current);
                current = rest_prefix.clone();
                has_word = false;
            }
            if has_word {
                current.push(' ');
            }
            current.push_str(word);
            has_word = true;
        }
        out.push(current.trim_end().to_string());
    }
    out
}

/// A line's prefix for its first line (indent, leader and the space
/// after it, or a list marker), the prefix of the lines after it, and
/// the bare leader (empty for none) that keeps a paragraph together.
fn prefixes(line: &str) -> (String, String, String) {
    let indent: String = line.chars().take_while(|c| *c == ' ' || *c == '\t').collect();
    let body = &line[indent.len()..];
    if let Some(marker) = list_marker(line) {
        let first = format!("{indent}{marker}");
        let rest = format!("{indent}{}", " ".repeat(marker.chars().count()));
        return (first, rest, String::new());
    }
    for leader in LEADERS {
        if let Some(after) = body.strip_prefix(leader) {
            let spaces: String = after.chars().take_while(|c| *c == ' ').collect();
            let prefix = format!("{indent}{leader}{}", if spaces.is_empty() { "" } else { " " });
            // The text after the leader starts after all of its spaces.
            let first = format!("{indent}{leader}{spaces}");
            return (first, prefix, leader.to_string());
        }
    }
    (indent.clone(), indent, String::new())
}

/// A Markdown list item's marker and the space after it (`- `, `12. `).
fn list_marker(line: &str) -> Option<String> {
    let body = line.trim_start_matches([' ', '\t']);
    let mut chars = body.char_indices();
    let (_, first) = chars.next()?;
    let end = if matches!(first, '-' | '*' | '+') {
        1
    } else if first.is_ascii_digit() {
        let digits = body.chars().take_while(char::is_ascii_digit).count();
        if !matches!(body[digits..].chars().next(), Some('.' | ')')) {
            return None;
        }
        digits + 1
    } else {
        return None;
    };
    let spaces = body[end..].chars().take_while(|c| *c == ' ').count();
    (spaces > 0).then(|| body[..end + spaces].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wrap(text: &str, width: usize) -> String {
        let lines: Vec<String> = text.lines().map(str::to_string).collect();
        wrap_lines(&lines, width).join("\n")
    }

    #[test]
    fn words_fill_each_line_to_the_width() {
        assert_eq!(wrap("one two three four five", 10), "one two\nthree four\nfive");
        assert_eq!(wrap("one\ntwo\nthree", 79), "one two three");
    }

    #[test]
    fn a_word_longer_than_the_width_has_a_line_of_its_own() {
        assert_eq!(wrap("a verylongword b", 5), "a\nverylongword\nb");
    }

    #[test]
    fn blank_lines_part_paragraphs() {
        assert_eq!(wrap("a\nb\n\nc\nd", 79), "a b\n\nc d");
    }

    #[test]
    fn the_indent_and_comment_leader_go_on_every_line() {
        assert_eq!(wrap("    // one two three four", 17), "    // one two\n    // three four");
        assert_eq!(wrap("# a\n# b", 79), "# a b");
    }

    #[test]
    fn a_list_items_lines_line_up_under_its_text() {
        assert_eq!(wrap("- one two three", 9), "- one two\n  three");
        assert_eq!(wrap("- a\n- b", 79), "- a\n- b");
        assert_eq!(wrap("1. one two three", 10), "1. one two\n   three");
    }

    #[test]
    fn a_different_leader_starts_a_new_paragraph() {
        assert_eq!(wrap("// a\nb", 79), "// a\nb");
    }
}
