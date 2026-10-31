//! Validate a complete text-only workspace edit before the host mutates anything.
//! File resource operations and annotated edits are deliberately rejected as a
//! whole until their lifecycle/confirmation semantics are supported.

use lsp_types::{DocumentChangeOperation, DocumentChanges, OneOf, Position, TextEdit, WorkspaceEdit};
use ropey::Rope;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub struct Document {
    pub path: PathBuf,
    pub text: String,
    /// The document version known to the server, not the editor's edit counter.
    pub version: Option<i32>,
}

#[derive(Debug, Clone)]
pub struct PlannedDocument {
    pub path: PathBuf,
    pub before: String,
    pub after: String,
}

pub fn prepare(edit: WorkspaceEdit, mut load: impl FnMut(&Path) -> Result<Document, String>) -> Result<Vec<PlannedDocument>, String> {
    if edit.changes.is_some() && edit.document_changes.is_some() {
        return Err("workspace edit contains both changes and documentChanges".into());
    }
    let mut requests = Vec::new();
    let mut add = |document: lsp_types::TextDocumentEdit| -> Result<(), String> {
        let edits = document
            .edits
            .into_iter()
            .map(|edit| match edit {
                OneOf::Left(edit) => Ok(edit),
                OneOf::Right(_) => Err("annotated edits are not supported".to_string()),
            })
            .collect::<Result<Vec<_>, _>>()?;
        requests.push((document.text_document.uri, document.text_document.version, edits));
        Ok(())
    };
    match edit.document_changes {
        Some(DocumentChanges::Edits(documents)) => {
            for document in documents {
                add(document)?;
            }
        }
        Some(DocumentChanges::Operations(operations)) => {
            for operation in operations {
                match operation {
                    DocumentChangeOperation::Edit(document) => add(document)?,
                    DocumentChangeOperation::Op(_) => return Err("file create/rename/delete operations are not supported; no edits applied".into()),
                }
            }
        }
        None => {
            if let Some(changes) = edit.changes {
                requests.extend(changes.into_iter().map(|(uri, edits)| (uri, None, edits)));
            }
        }
    }
    requests.sort_by(|a, b| a.0.as_str().cmp(b.0.as_str()));
    let mut seen = HashSet::new();
    let mut planned = Vec::new();
    for (uri, version, edits) in requests {
        let path = crate::uri_to_path(&uri).ok_or_else(|| format!("unsupported document URI: {}", uri.as_str()))?;
        let document = load(&path)?;
        if !seen.insert(document.path.clone()) {
            return Err(format!("duplicate document target: {}", document.path.display()));
        }
        if let Some(expected) = version {
            if document.version != Some(expected) {
                return Err(format!("stale or unknown document version for {} (expected {expected}, current {:?})", document.path.display(), document.version));
            }
        }
        let after = apply_text_edits(&document.text, &edits).map_err(|e| format!("{}: {e}", document.path.display()))?;
        if after != document.text {
            planned.push(PlannedDocument { path: document.path, before: document.text, after });
        }
    }
    Ok(planned)
}

/// Positions for mutations must be exact. Navigation's clamping helper is not
/// suitable here: an invalid range is an error, not a nearby place to edit.
fn offset(rope: &Rope, position: Position) -> Result<usize, String> {
    let line = position.line as usize;
    if line >= rope.len_lines() {
        return Err("edit line is outside the document".into());
    }
    let text = rope.line(line).to_string();
    let text = text.strip_suffix('\n').unwrap_or(&text);
    let text = text.strip_suffix('\r').unwrap_or(text);
    let mut units = 0;
    let mut chars = 0;
    for ch in text.chars() {
        if units == position.character {
            return Ok(rope.line_to_char(line) + chars);
        }
        units += ch.len_utf16() as u32;
        chars += 1;
        if units > position.character {
            return Err("edit splits a UTF-16 surrogate pair".into());
        }
    }
    if units == position.character { Ok(rope.line_to_char(line) + chars) } else { Err("edit column is outside the line".into()) }
}

pub fn apply_text_edits(text: &str, edits: &[TextEdit]) -> Result<String, String> {
    let mut rope = Rope::from_str(text);
    let mut ranges = Vec::new();
    for edit in edits {
        let start = offset(&rope, edit.range.start)?;
        let end = offset(&rope, edit.range.end)?;
        if end < start {
            return Err("reversed edit range".into());
        }
        ranges.push((start, end, &edit.new_text));
    }
    // Stable ordering preserves server order for multiple inserts at one point.
    ranges.sort_by_key(|(start, _, _)| *start);
    let mut previous_end = 0;
    for (start, end, _) in &ranges {
        if *start < previous_end {
            return Err("overlapping edit ranges".into());
        }
        previous_end = *end;
    }
    for (start, end, replacement) in ranges.into_iter().rev() {
        rope.remove(start..end);
        rope.insert(start, replacement);
    }
    Ok(rope.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_types::Range;
    fn edit(start: u32, end: u32, text: &str) -> TextEdit {
        TextEdit { range: Range::new(Position::new(0, start), Position::new(0, end)), new_text: text.into() }
    }
    #[test]
    fn exact_utf16_and_crlf() {
        assert_eq!(apply_text_edits("a😀z\r\nnext", &[edit(1, 3, "X")]).unwrap(), "aXz\r\nnext");
        assert!(apply_text_edits("a😀z", &[edit(2, 3, "X")]).is_err());
        assert!(apply_text_edits("a\r\n", &[edit(2, 2, "X")]).is_err());
    }
    #[test]
    fn rejects_invalid_and_overlapping_ranges() {
        for edits in [
            vec![edit(3, 2, "")],
            vec![edit(0, 9, "")],
            vec![edit(0, 2, ""), edit(1, 3, "")],
            vec![TextEdit { range: Range::new(Position::new(4, 0), Position::new(4, 0)), new_text: "x".into() }],
        ] {
            assert!(apply_text_edits("abc", &edits).is_err());
        }
    }
    #[test]
    fn same_position_inserts_preserve_server_order() {
        assert_eq!(apply_text_edits("abc", &[edit(1, 1, "X"), edit(1, 1, "Y"), edit(1, 2, "Z")]).unwrap(), "aXYZc");
    }
    #[test]
    fn resource_operations_reject_before_loading_any_file() {
        let edit = serde_json::from_value(serde_json::json!({"documentChanges":[
            {"textDocument":{"uri":"file:///C:/a.txt","version":null},"edits":[]},
            {"kind":"delete","uri":"file:///C:/b.txt"}
        ]}))
        .unwrap();
        assert!(prepare(edit, |_| panic!("must not load unsupported edit")).is_err());
    }
    #[test]
    fn versions_and_duplicate_targets_are_checked() {
        let make = || {
            serde_json::from_value(serde_json::json!({"documentChanges":[
                {"textDocument":{"uri":"file:///C:/a.txt","version":3},"edits":[]}
            ]}))
            .unwrap()
        };
        for version in [None, Some(2)] {
            assert!(prepare(make(), |p| Ok(Document { path: p.into(), text: "abc".into(), version })).is_err());
        }
        assert!(prepare(make(), |p| Ok(Document { path: p.into(), text: "abc".into(), version: Some(3) })).unwrap().is_empty());
        let mut duplicate = make();
        if let Some(DocumentChanges::Edits(edits)) = &mut duplicate.document_changes {
            edits.push(edits[0].clone());
        }
        assert!(prepare(duplicate, |p| Ok(Document { path: p.into(), text: "abc".into(), version: Some(3) })).is_err());
    }
    #[test]
    fn invalid_later_file_returns_no_partial_plan() {
        let edit = serde_json::from_value(serde_json::json!({"changes": {
            "file:///C:/a.txt":[{"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":1}},"newText":"X"}],
            "file:///C:/z.txt":[{"range":{"start":{"line":99,"character":0},"end":{"line":99,"character":0}},"newText":"X"}]
        }}))
        .unwrap();
        assert!(prepare(edit, |p| Ok(Document { path: p.into(), text: "abc".into(), version: None })).is_err());
    }
}
