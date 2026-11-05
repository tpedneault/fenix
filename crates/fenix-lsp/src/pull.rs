//! What a client asks a server for rather than waits to be told: its
//! diagnostics for a document (`textDocument/diagnostic`, LSP 3.17's pull
//! diagnostics -- ESLint's and csharp-ls's only way), and what a server asks
//! the client: its settings (`workspace/configuration`).

use lsp_types::{Diagnostic, DocumentDiagnosticReport, DocumentDiagnosticReportResult, ServerCapabilities, Uri};
use serde_json::Value;

/// Whether `caps` says the server answers `textDocument/diagnostic`.
pub fn pulls(caps: &ServerCapabilities) -> bool {
    caps.diagnostic_provider.is_some()
}

/// Whether a change to one document may change what the server says of
/// others (`interFileDependencies`): then the others are asked again too.
pub fn inter_file(caps: &ServerCapabilities) -> bool {
    match &caps.diagnostic_provider {
        Some(lsp_types::DiagnosticServerCapabilities::Options(o)) => o.inter_file_dependencies,
        Some(lsp_types::DiagnosticServerCapabilities::RegistrationOptions(o)) => o.diagnostic_options.inter_file_dependencies,
        None => false,
    }
}

/// What a `textDocument/diagnostic` answer says of the document asked
/// about, and of related ones: each one's result id, for the next ask, and
/// its diagnostics -- none when they're as last reported (`unchanged`).
pub type Report = Vec<(Option<Uri>, Option<String>, Option<Vec<Diagnostic>>)>;

/// The answer to `textDocument/diagnostic`, read: the document asked
/// about first (`None` for its uri), then related documents'.
pub fn report(answer: Value) -> Result<Report, String> {
    let result: DocumentDiagnosticReportResult = serde_json::from_value(answer).map_err(|e| e.to_string())?;
    let (main, related) = match result {
        DocumentDiagnosticReportResult::Report(DocumentDiagnosticReport::Full(full)) => {
            ((None, full.full_document_diagnostic_report.result_id, Some(full.full_document_diagnostic_report.items)), full.related_documents)
        }
        DocumentDiagnosticReportResult::Report(DocumentDiagnosticReport::Unchanged(same)) => ((None, Some(same.unchanged_document_diagnostic_report.result_id), None), same.related_documents),
        DocumentDiagnosticReportResult::Partial(partial) => ((None, None, None), partial.related_documents),
    };
    let mut out = vec![main];
    for (uri, kind) in related.unwrap_or_default() {
        out.push(match kind {
            lsp_types::DocumentDiagnosticReportKind::Full(full) => (Some(uri), full.result_id, Some(full.items)),
            lsp_types::DocumentDiagnosticReportKind::Unchanged(same) => (Some(uri), Some(same.result_id), None),
        });
    }
    Ok(out)
}

/// The value at `section` (`a.b`, as `workspace/configuration` names one)
/// in `settings`; the whole of it for none or `""`; null where there's
/// nothing.
pub fn section(settings: &Value, section: Option<&str>) -> Value {
    let Some(section) = section.filter(|s| !s.is_empty()) else { return settings.clone() };
    let mut at = settings;
    for part in section.split('.') {
        match at.get(part) {
            Some(next) => at = next,
            None => return Value::Null,
        }
    }
    at.clone()
}

/// The answer to `workspace/configuration`'s `params`: for each item, its
/// section of `settings` (or null).
pub fn configuration(params: &Value, settings: &Value) -> Value {
    let items = params.get("items").and_then(Value::as_array).cloned().unwrap_or_default();
    Value::Array(items.iter().map(|item| section(settings, item.get("section").and_then(Value::as_str))).collect())
}

/// `over` laid over `base`: objects merged key by key, anything else
/// replaced.
pub fn merge(base: Value, over: Value) -> Value {
    match (base, over) {
        (Value::Object(mut base), Value::Object(over)) => {
            for (key, value) in over {
                let merged = match base.remove(&key) {
                    Some(old) => merge(old, value),
                    None => value,
                };
                base.insert(key, merged);
            }
            Value::Object(base)
        }
        (base, Value::Null) => base,
        (_, over) => over,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_full_report_and_an_unchanged_one_are_read() {
        let full = json!({"kind": "full", "resultId": "7", "items": [{"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 3}}, "message": "Unexpected var", "source": "eslint"}]});
        let read = report(full).unwrap();
        assert_eq!(read.len(), 1);
        assert_eq!(read[0].1.as_deref(), Some("7"));
        assert_eq!(read[0].2.as_ref().unwrap()[0].message, "Unexpected var");
        let same = report(json!({"kind": "unchanged", "resultId": "7"})).unwrap();
        assert_eq!((same[0].1.as_deref(), same[0].2.is_none()), (Some("7"), true));
        assert!(report(json!(42)).is_err());
    }

    #[test]
    fn related_documents_come_after_the_one_asked_about() {
        let answer = json!({"kind": "full", "items": [], "relatedDocuments": {"file:///p/b.h": {"kind": "full", "items": [{"range": {"start": {"line": 1, "character": 0}, "end": {"line": 1, "character": 1}}, "message": "in a header"}]}}});
        let read = report(answer).unwrap();
        assert_eq!(read.len(), 2);
        assert_eq!(read[1].0.as_ref().map(|u| u.as_str()), Some("file:///p/b.h"));
        assert_eq!(read[1].2.as_ref().unwrap()[0].message, "in a header");
    }

    /// What ESLint asks (`""`, the whole) and Tailwind asks (`editor`,
    /// `tailwindCSS`): a section, or null.
    #[test]
    fn configuration_items_are_answered_by_section_or_null() {
        let settings = json!({"tailwindCSS": {"emmetCompletions": true}, "python": {"analysis": {"typeCheckingMode": "strict"}}});
        let params = json!({"items": [{"section": "tailwindCSS", "scopeUri": "file:///p"}, {"section": "editor"}, {"section": "python.analysis.typeCheckingMode"}, {"section": ""}, {}]});
        let answer = configuration(&params, &settings);
        assert_eq!(answer, json!([{"emmetCompletions": true}, null, "strict", settings, settings]));
        assert_eq!(configuration(&json!({"items": [{"section": "x"}]}), &Value::Null), json!([null]));
    }

    #[test]
    fn settings_are_laid_over_each_other() {
        let base = json!({"python": {"pythonPath": "/v/bin/python", "analysis": {"a": 1}}});
        let over = json!({"python": {"analysis": {"b": 2}}, "x": true});
        assert_eq!(merge(base.clone(), over), json!({"python": {"pythonPath": "/v/bin/python", "analysis": {"a": 1, "b": 2}}, "x": true}));
        assert_eq!(merge(base.clone(), Value::Null), base);
    }
}
