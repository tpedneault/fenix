//! `it`/`at` -- real Vim's tag-block text objects. Purely textual, like
//! Vim's own: any `<name ...>...</name>` pair counts, in any buffer, so
//! they work in XML, HTML, SVG, JSX, or a Markdown file with inline
//! markup without this crate knowing what language it's looking at.

/// One matched `<name ...>` ... `</name>` pair, as byte offsets into the
/// scanned text: the start tag spans `open_start..open_end` and the end
/// tag `close_start..close_end`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TagPair {
    pub open_start: usize,
    pub open_end: usize,
    pub close_start: usize,
    pub close_end: usize,
}

fn is_name_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | ':')
}

/// Byte length of the tag starting at `text[0] == '<'` through its `>`,
/// respecting quoted attribute values, and whether it ends in `/>`.
fn tag_len(text: &str) -> Option<(usize, bool)> {
    let mut quote: Option<char> = None;
    let mut prev = '<';
    for (i, c) in text.char_indices().skip(1) {
        match (quote, c) {
            (None, '"' | '\'') => quote = Some(c),
            (Some(q), c) if c == q => quote = None,
            (None, '>') => return Some((i + 1, prev == '/')),
            // A new tag starting before this one closed: this `<` wasn't
            // a tag at all (`a < b`), so give up on it.
            (None, '<') => return None,
            _ => {}
        }
        prev = c;
    }
    None
}

/// Every properly nested tag pair in `text`. Comments, CDATA, processing
/// instructions and declarations are skipped whole; an end tag closes the
/// nearest open tag of the same name, discarding any unclosed ones in
/// between (`<p><br></p>` still pairs the `p`s, the way HTML reads).
pub(crate) fn pairs(text: &str) -> Vec<TagPair> {
    let mut out = Vec::new();
    let mut stack: Vec<(String, usize, usize)> = Vec::new();
    let mut pos = 0;
    while let Some(offset) = text[pos..].find('<') {
        let start = pos + offset;
        let rest = &text[start..];
        let skip = |end: &str| rest.find(end).map(|i| i + end.len());
        let consumed = if rest.starts_with("<!--") {
            skip("-->")
        } else if rest.starts_with("<![CDATA[") {
            skip("]]>")
        } else if rest.starts_with("<?") {
            skip("?>")
        } else if rest.starts_with("<!") {
            skip(">")
        } else if let Some(after) = rest.strip_prefix("</") {
            let name: String = after.chars().take_while(|&c| is_name_char(c)).collect();
            match tag_len(rest) {
                Some((len, _)) => {
                    if let Some(i) = stack.iter().rposition(|(n, _, _)| *n == name) {
                        let (_, open_start, open_end) = stack[i].clone();
                        stack.truncate(i);
                        out.push(TagPair { open_start, open_end, close_start: start, close_end: start + len });
                    }
                    Some(len)
                }
                None => Some(1),
            }
        } else {
            let name: String = rest[1..].chars().take_while(|&c| is_name_char(c)).collect();
            match tag_len(rest) {
                Some((len, self_closing)) if !name.is_empty() => {
                    if !self_closing {
                        stack.push((name, start, start + len));
                    }
                    Some(len)
                }
                _ => Some(1),
            }
        };
        match consumed {
            Some(n) => pos = start + n,
            None => break,
        }
    }
    out
}

/// The innermost pair whose whole extent (start tag through end tag)
/// contains `byte` -- the element the cursor is in or on.
pub(crate) fn enclosing(text: &str, byte: usize) -> Option<TagPair> {
    pairs(text)
        .into_iter()
        .filter(|p| p.open_start <= byte && byte < p.close_end)
        .min_by_key(|p| p.close_end - p.open_start)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inner<'a>(text: &'a str, at: &str) -> Option<&'a str> {
        let byte = text.find(at).unwrap();
        enclosing(text, byte).map(|p| &text[p.open_end..p.close_start])
    }

    #[test]
    fn finds_the_innermost_enclosing_element() {
        let text = "<a><b id=\"x\">hello</b> tail</a>";
        assert_eq!(inner(text, "hello"), Some("hello"));
        assert_eq!(inner(text, "tail"), Some("<b id=\"x\">hello</b> tail"));
    }

    #[test]
    fn the_cursor_on_a_tag_itself_selects_that_element() {
        let text = "<a><b>x</b></a>";
        assert_eq!(inner(text, "b>x"), Some("x"));
        assert_eq!(inner(text, "/b>"), Some("x"));
    }

    #[test]
    fn comments_self_closing_tags_and_quoted_gt_are_not_tags() {
        let text = "<a title=\"x>y\"><!-- <b> --><br/><c/>body</a>";
        assert_eq!(inner(text, "body"), Some("<!-- <b> --><br/><c/>body"));
    }

    #[test]
    fn a_stray_less_than_is_not_a_tag() {
        let text = "<p>if a < b then</p>";
        assert_eq!(inner(text, "then"), Some("if a < b then"));
    }

    #[test]
    fn nothing_outside_any_element() {
        assert_eq!(enclosing("plain text", 2), None);
    }
}
