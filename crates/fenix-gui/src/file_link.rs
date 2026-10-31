//! What `gf` follows: the file name under the cursor, and the page a
//! `#page=38` after it asks for (a PDF's, as the PDF reader's `yp` copies
//! them).

/// The file name under column `col` of `line`, and the page a
/// `#page=38` after it asks for. A Markdown link's brackets and quotes
/// around it aren't part of it, nor is a full stop or comma after it.
pub fn link_at(line: &str, col: usize) -> Option<(String, Option<u32>)> {
    let chars: Vec<char> = line.chars().collect();
    let stop = |c: char| c.is_whitespace() || "()[]<>\"'`|".contains(c);
    if col >= chars.len() || stop(chars[col]) {
        return None;
    }
    let start = (0..col).rev().take_while(|&i| !stop(chars[i])).last().unwrap_or(col);
    let end = (col..chars.len()).take_while(|&i| !stop(chars[i])).last()? + 1;
    let mut token: String = chars[start..end].iter().collect();
    while token.ends_with(['.', ',', ';', ':', '!', '?']) {
        token.pop();
    }
    let (path, fragment) = match token.split_once('#') {
        Some((path, fragment)) => (path.to_string(), Some(fragment)),
        None => (token, None),
    };
    let page = fragment.and_then(|f| f.strip_prefix("page=")).and_then(|n| n.parse().ok());
    (!path.is_empty()).then_some((path, page))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_link_under_the_cursor() {
        let line = "see [the spec](specs/133x0b2e2.pdf#page=38), then more";
        let at = line.find("133x").unwrap();
        assert_eq!(link_at(line, at), Some(("specs/133x0b2e2.pdf".to_string(), Some(38))));
        assert_eq!(link_at("open notes.md.", 7), Some(("notes.md".to_string(), None)), "a full stop isn't part of it");
        assert_eq!(link_at("a b", 1), None, "not on a space");
        assert_eq!(link_at("x.pdf#nameddest=intro", 0), Some(("x.pdf".to_string(), None)));
    }
}
