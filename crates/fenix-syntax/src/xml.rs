//! XML editing and navigation helpers. Two kinds live here:
//!
//! - *Tree-based* ones (`elements`, `matching_tag`, `element_path`) walk
//!   the persisted tree-sitter parse, so they're only as current as the
//!   last `SyntaxState::apply_edits` -- callers bring the tree up to date
//!   first, the same as they already do for scope ranges.
//! - *Text-based* ones (`close_tag_after_gt`, `innermost_open_element`)
//!   answer questions asked in the middle of typing, when the document
//!   is by definition malformed (`<item` with no `>` yet) and a parse
//!   tree has an `ERROR` node exactly where the answer is needed.

use tree_sitter::{Node, Tree};

/// One element in document order, for an outline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XmlElement {
    /// Nesting depth; the root element is 0.
    pub depth: usize,
    pub name: String,
    /// A short identifying attribute (`id`, `name`, `key`, ...) and its
    /// value, when the element has one -- what tells ten `<dependency>`
    /// siblings apart in a list.
    pub label_attribute: Option<(String, String)>,
    /// Byte offset of the element's start tag `<`.
    pub start_byte: usize,
    pub start_row: usize,
}

/// A well-formedness error, with a 0-indexed line and char column.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XmlError {
    pub line: usize,
    pub col: usize,
    pub message: String,
}

/// The attributes worth showing next to an element's name, in the order
/// they're preferred when an element carries several.
const LABEL_ATTRIBUTES: &[&str] = &["id", "name", "key", "ref", "type", "Include", "Name", "x:Name", "path", "href", "src"];

fn node_text<'a>(node: Node<'_>, source: &'a str) -> &'a str {
    source.get(node.start_byte()..node.end_byte()).unwrap_or("")
}

/// An element node's opening tag: its `STag`, or its `EmptyElemTag` for
/// `<self-closing/>`.
fn open_tag(element: Node<'_>) -> Option<Node<'_>> {
    let mut cursor = element.walk();
    let found = element.children(&mut cursor).find(|c| matches!(c.kind(), "STag" | "EmptyElemTag"));
    found
}

fn close_tag(element: Node<'_>) -> Option<Node<'_>> {
    let mut cursor = element.walk();
    let found = element.children(&mut cursor).find(|c| c.kind() == "ETag");
    found
}

fn tag_name<'a>(tag: Node<'_>, source: &'a str) -> Option<(&'a str, usize)> {
    let mut cursor = tag.walk();
    let name = tag.children(&mut cursor).find(|c| c.kind() == "Name")?;
    Some((node_text(name, source), name.start_byte()))
}

fn label_attribute(tag: Node<'_>, source: &str) -> Option<(String, String)> {
    let mut cursor = tag.walk();
    let mut attributes: Vec<(String, String)> = Vec::new();
    for attr in tag.children(&mut cursor).filter(|c| c.kind() == "Attribute") {
        let mut inner = attr.walk();
        let mut name = None;
        let mut value = None;
        for child in attr.children(&mut inner) {
            match child.kind() {
                "Name" => name = Some(node_text(child, source).to_string()),
                "AttValue" => value = Some(node_text(child, source).trim_matches(|c| c == '"' || c == '\'').to_string()),
                _ => {}
            }
        }
        if let (Some(name), Some(value)) = (name, value) {
            attributes.push((name, value));
        }
    }
    LABEL_ATTRIBUTES.iter().find_map(|want| attributes.iter().find(|(n, _)| n == want).cloned())
}

/// Every element in the document, in document order.
pub(crate) fn elements(tree: &Tree, source: &str) -> Vec<XmlElement> {
    let mut out = Vec::new();
    let mut cursor = tree.walk();
    let mut depth: usize = 0;
    loop {
        let node = cursor.node();
        let is_element = node.kind() == "element";
        if is_element {
            if let Some(tag) = open_tag(node) {
                if let Some((name, _)) = tag_name(tag, source) {
                    out.push(XmlElement {
                        depth,
                        name: name.to_string(),
                        label_attribute: label_attribute(tag, source),
                        start_byte: node.start_byte(),
                        start_row: node.start_position().row,
                    });
                }
            }
        }
        if cursor.goto_first_child() {
            if is_element {
                depth += 1;
            }
            continue;
        }
        loop {
            if cursor.goto_next_sibling() {
                break;
            }
            if !cursor.goto_parent() {
                return out;
            }
            if cursor.node().kind() == "element" {
                depth = depth.saturating_sub(1);
            }
        }
    }
}

/// The innermost element whose own tags (not its content) contain
/// `byte`, plus which of the two it was: `true` for the start tag. An
/// attribute doesn't count as being on the tag -- its value is where
/// `%` should still match brackets.
fn element_by_tag_at(tree: &Tree, byte: usize) -> Option<(Node<'_>, bool)> {
    let mut node = tree.root_node().descendant_for_byte_range(byte, byte)?;
    loop {
        match node.kind() {
            "STag" => return node.parent().map(|p| (p, true)),
            "ETag" => return node.parent().map(|p| (p, false)),
            // A self-closing tag has no partner to jump to.
            "Attribute" | "EmptyElemTag" | "element" | "document" => return None,
            _ => node = node.parent()?,
        }
    }
}

/// With the cursor anywhere on a start or end tag, the byte offset of
/// the *other* tag's name -- `%` between `<item>` and `</item>`. `None`
/// when `byte` isn't on a tag, or the tag is unpaired (self-closing, or
/// a parse too broken to have found its partner).
pub(crate) fn matching_tag(tree: &Tree, source: &str, byte: usize) -> Option<usize> {
    let (element, on_start) = element_by_tag_at(tree, byte)?;
    let other = if on_start { close_tag(element)? } else { open_tag(element)? };
    tag_name(other, source).map(|(_, at)| at)
}

/// An XPath-style location for the element enclosing `byte`, with a
/// 1-based position predicate wherever an element has same-named
/// siblings: `/project/dependencies/dependency[3]/version`.
pub(crate) fn element_path(tree: &Tree, source: &str, byte: usize) -> Option<String> {
    let mut node = tree.root_node().descendant_for_byte_range(byte, byte)?;
    let mut steps: Vec<String> = Vec::new();
    loop {
        if node.kind() == "element" {
            let name = open_tag(node).and_then(|t| tag_name(t, source)).map(|(n, _)| n.to_string())?;
            let (mut index, mut total) = (0usize, 0usize);
            if let Some(parent) = node.parent() {
                let mut cursor = parent.walk();
                for sibling in parent.children(&mut cursor).filter(|c| c.kind() == "element") {
                    let same = open_tag(sibling).and_then(|t| tag_name(t, source)).is_some_and(|(n, _)| n == name);
                    if same {
                        total += 1;
                        if sibling.id() == node.id() {
                            index = total;
                        }
                    }
                }
            }
            steps.push(if total > 1 { format!("{name}[{index}]") } else { name });
        }
        match node.parent() {
            Some(parent) => node = parent,
            None => break,
        }
    }
    if steps.is_empty() {
        return None;
    }
    steps.reverse();
    Some(format!("/{}", steps.join("/")))
}

fn is_name_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | ':')
}

/// Called right after a `>` was typed, with `before` the text up to and
/// including it: if that `>` just finished an ordinary start tag, the
/// closing tag to insert after the cursor (`<item id="1">` -> `</item>`).
/// `None` for anything else -- an end tag, a `/>` self-closing tag, a
/// comment/PI/doctype, a `>` inside an attribute value or plain text.
pub fn close_tag_after_gt(before: &str) -> Option<String> {
    let body = before.strip_suffix('>')?;
    if body.ends_with('/') {
        return None;
    }
    let open = tag_start_before(body)?;
    let tag = &body[open + 1..];
    let name: String = tag.chars().take_while(|&c| is_name_char(c)).collect();
    if name.is_empty() || !tag.starts_with(|c: char| c.is_alphabetic() || c == '_' || c == ':') {
        return None;
    }
    Some(format!("</{name}>"))
}

/// The byte offset of the `<` that opens the tag `body` is still inside
/// of, if it is inside one -- skipping over a `>` that sits within a
/// quoted attribute value. `None` if `body` isn't mid-tag.
fn tag_start_before(body: &str) -> Option<usize> {
    let open = body.rfind('<')?;
    let tag = &body[open..];
    let mut quote: Option<char> = None;
    for c in tag.chars().skip(1) {
        match (quote, c) {
            (None, '"' | '\'') => quote = Some(c),
            (Some(q), c) if c == q => quote = None,
            (None, '>') => return None,
            _ => {}
        }
    }
    (quote.is_none()).then_some(open)
}

/// The name of the innermost element still open at the end of `before`
/// -- what `</` should be completed to. A small tag scanner over the
/// text rather than the parse tree (see the module doc): comments,
/// CDATA sections, processing instructions and `<!DOCTYPE ...>` are
/// skipped whole, self-closing tags never open anything, and an end tag
/// closes the nearest open element of that name (tolerating the
/// mismatches a document being edited is full of).
pub fn innermost_open_element(before: &str) -> Option<String> {
    let mut stack: Vec<String> = Vec::new();
    let mut rest = before;
    while let Some(lt) = rest.find('<') {
        rest = &rest[lt..];
        let skip_to = |rest: &str, end: &str| rest.find(end).map(|i| i + end.len());
        let consumed = if rest.starts_with("<!--") {
            skip_to(rest, "-->")
        } else if rest.starts_with("<![CDATA[") {
            skip_to(rest, "]]>")
        } else if rest.starts_with("<?") {
            skip_to(rest, "?>")
        } else if rest.starts_with("<!") {
            // A DOCTYPE with an internal subset nests `[...]`; the
            // subset's own declarations all end in `>`, so skip to the
            // `]>` when there is one.
            match (rest.find('['), rest.find('>')) {
                (Some(bracket), Some(gt)) if bracket < gt => skip_to(rest, "]>"),
                _ => skip_to(rest, ">"),
            }
        } else if let Some(name_part) = rest.strip_prefix("</") {
            let name: String = name_part.chars().take_while(|&c| is_name_char(c)).collect();
            if let Some(pos) = stack.iter().rposition(|n| *n == name) {
                stack.truncate(pos);
            }
            skip_to(rest, ">")
        } else {
            let name: String = rest[1..].chars().take_while(|&c| is_name_char(c)).collect();
            match tag_end(rest) {
                Some((end, self_closing)) => {
                    if !name.is_empty() && !self_closing {
                        stack.push(name);
                    }
                    Some(end)
                }
                None => None,
            }
        };
        match consumed {
            Some(n) => rest = &rest[n..],
            // An unterminated construct runs to the end of `before`:
            // nothing after it can be a tag.
            None => break,
        }
    }
    stack.pop()
}

/// For `tag` starting at a `<`, the byte length through its closing `>`
/// (respecting quoted attribute values) and whether it ended in `/>`.
fn tag_end(tag: &str) -> Option<(usize, bool)> {
    let mut quote: Option<char> = None;
    let mut prev = '<';
    for (i, c) in tag.char_indices().skip(1) {
        match (quote, c) {
            (None, '"' | '\'') => quote = Some(c),
            (Some(q), c) if c == q => quote = None,
            (None, '>') => return Some((i + 1, prev == '/')),
            _ => {}
        }
        prev = c;
    }
    None
}

/// Checks `source` is well-formed XML (a DTD, if present, is read but
/// not validated against). The first error found, if any.
pub fn check_well_formed(source: &str) -> Result<(), XmlError> {
    let options = roxmltree::ParsingOptions { allow_dtd: true, ..roxmltree::ParsingOptions::default() };
    match roxmltree::Document::parse_with_options(source, options) {
        Ok(_) => Ok(()),
        Err(err) => {
            let pos = err.pos();
            Err(XmlError {
                line: (pos.row as usize).saturating_sub(1),
                col: (pos.col as usize).saturating_sub(1),
                message: err.to_string(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{LanguageId, SyntaxState};

    const DOC: &str = "<?xml version=\"1.0\"?>\n<project>\n  <dependencies>\n    <dependency id=\"a\">\n      <version>1</version>\n    </dependency>\n    <dependency id=\"b\">\n      <version>2</version>\n    </dependency>\n  </dependencies>\n  <empty/>\n</project>\n";

    #[test]
    fn elements_come_back_in_document_order_with_depth_and_label() {
        let state = SyntaxState::new(LanguageId::Xml, DOC);
        let elements = state.xml_elements(DOC);
        let summary: Vec<(usize, &str)> = elements.iter().map(|e| (e.depth, e.name.as_str())).collect();
        assert_eq!(
            summary,
            vec![(0, "project"), (1, "dependencies"), (2, "dependency"), (3, "version"), (2, "dependency"), (3, "version"), (1, "empty")]
        );
        assert_eq!(elements[2].label_attribute, Some(("id".to_string(), "a".to_string())));
        assert_eq!(elements[2].start_row, 3);
    }

    #[test]
    fn matching_tag_jumps_between_start_and_end() {
        let state = SyntaxState::new(LanguageId::Xml, DOC);
        let start = DOC.find("<dependencies>").unwrap() + 3;
        let end_name = DOC.find("</dependencies>").unwrap() + 2;
        assert_eq!(state.xml_matching_tag(DOC, start), Some(end_name));
        assert_eq!(state.xml_matching_tag(DOC, end_name + 1), Some(DOC.find("<dependencies>").unwrap() + 1));
    }

    #[test]
    fn matching_tag_is_none_off_a_tag_or_on_a_self_closing_one() {
        let state = SyntaxState::new(LanguageId::Xml, DOC);
        assert_eq!(state.xml_matching_tag(DOC, DOC.find(">1<").unwrap() + 1), None);
        assert_eq!(state.xml_matching_tag(DOC, DOC.find("<empty/>").unwrap() + 2), None);
        assert_eq!(state.xml_matching_tag(DOC, DOC.find("id=\"a\"").unwrap() + 4), None);
    }

    #[test]
    fn element_path_numbers_same_named_siblings() {
        let state = SyntaxState::new(LanguageId::Xml, DOC);
        let at = DOC.find(">2<").unwrap() + 1;
        assert_eq!(state.xml_element_path(DOC, at).as_deref(), Some("/project/dependencies/dependency[2]/version"));
        let at = DOC.find("<empty").unwrap() + 2;
        assert_eq!(state.xml_element_path(DOC, at).as_deref(), Some("/project/empty"));
    }

    #[test]
    fn close_tag_after_gt_completes_start_tags_only() {
        assert_eq!(close_tag_after_gt("<item>").as_deref(), Some("</item>"));
        assert_eq!(close_tag_after_gt("  <ns:item id=\"1\">").as_deref(), Some("</ns:item>"));
        assert_eq!(close_tag_after_gt("<a href=\"x>y\">").as_deref(), Some("</a>"));
        assert_eq!(close_tag_after_gt("<item/>"), None);
        assert_eq!(close_tag_after_gt("</item>"), None);
        assert_eq!(close_tag_after_gt("<!-- c -->"), None);
        assert_eq!(close_tag_after_gt("<?xml version=\"1.0\"?>"), None);
        assert_eq!(close_tag_after_gt("a > b"), None);
        assert_eq!(close_tag_after_gt("<a>text>"), None);
        assert_eq!(close_tag_after_gt("<a attr=\"x>"), None);
    }

    #[test]
    fn innermost_open_element_tracks_nesting() {
        assert_eq!(innermost_open_element("<a><b>text").as_deref(), Some("b"));
        assert_eq!(innermost_open_element("<a><b>text</b>").as_deref(), Some("a"));
        assert_eq!(innermost_open_element("<a><b/>").as_deref(), Some("a"));
        assert_eq!(innermost_open_element("<a><!-- <c> --><![CDATA[<d>]]><?pi <e>?>").as_deref(), Some("a"));
        assert_eq!(innermost_open_element("<!DOCTYPE r [<!ENTITY x \"y\">]><r attr='>'>").as_deref(), Some("r"));
        assert_eq!(innermost_open_element("<a></a>"), None);
        assert_eq!(innermost_open_element(""), None);
    }

    #[test]
    fn well_formedness_errors_carry_a_position() {
        assert!(check_well_formed(DOC).is_ok());
        let err = check_well_formed("<a>\n  <b></c>\n</a>").unwrap_err();
        assert_eq!(err.line, 1);
        assert!(!err.message.is_empty());
    }
}
