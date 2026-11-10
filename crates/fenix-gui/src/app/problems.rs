//! Lists of places for standard keys' Ctrl+Shift+M and Ctrl+T (and the
//! palette and leader with either keys): every problem the language
//! servers report, in every file (`code.problems`), and the project's
//! symbols as the servers find them for what's typed
//! (`code.project_symbols`). Confirming one goes there, as a TODO in
//! `SPC s T`'s list does.

use std::path::{Path, PathBuf};

use fenix_project::GrepMatch;

use super::{ActivePicker, App, LspKey, PendingLspRequest, QuickfixEntry};

/// The most symbols one server's answer is listed with.
const MAX_SYMBOLS: usize = 500;

/// What `code.project_symbols` has asked, and been answered.
#[derive(Debug, Default)]
pub(crate) struct SymbolSearch {
    /// Bumped with each question: an older answer is dropped.
    generation: u64,
    /// What was last asked.
    query: Option<String>,
    /// Each server's latest answer, as rows.
    answers: Vec<(LspKey, Vec<(String, GrepMatch)>)>,
}

/// A problem's severity, most severe first, and its word.
fn severity(d: &lsp_types::Diagnostic) -> (u8, &'static str) {
    match d.severity {
        Some(lsp_types::DiagnosticSeverity::ERROR) => (0, "error"),
        Some(lsp_types::DiagnosticSeverity::WARNING) => (1, "warning"),
        Some(lsp_types::DiagnosticSeverity::INFORMATION) => (2, "info"),
        _ => (3, "hint"),
    }
}

/// Rows for problems, by path: most severe first, then by file and
/// line. Lines and columns from 1, as a grep match's.
pub(crate) fn problem_rows(root: &Path, problems: Vec<(PathBuf, Vec<lsp_types::Diagnostic>)>) -> Vec<(String, GrepMatch)> {
    let mut rows: Vec<(u8, String, GrepMatch)> = Vec::new();
    for (path, diagnostics) in problems {
        for d in diagnostics {
            let (rank, word) = severity(&d);
            let line = d.range.start.line as usize + 1;
            let col = d.range.start.character as usize + 1;
            let message = d.message.lines().next().unwrap_or("").trim().to_string();
            let source = d.source.as_deref().map(|s| format!(" ({s})")).unwrap_or_default();
            let label = format!("{word}  {}:{line}  {message}{source}", App::relative_label(root, &path));
            rows.push((rank, label, GrepMatch { path: path.clone(), line, col, text: message }));
        }
    }
    rows.sort_by(|a, b| (a.0, &a.2.path, a.2.line, a.2.col).cmp(&(b.0, &b.2.path, b.2.line, b.2.col)));
    rows.dedup_by(|a, b| a.1 == b.1);
    rows.into_iter().map(|(_, label, m)| (label, m)).collect()
}

/// A symbol kind's word.
fn kind_word(kind: lsp_types::SymbolKind) -> &'static str {
    use lsp_types::SymbolKind as K;
    match kind {
        K::FILE => "file",
        K::MODULE | K::NAMESPACE | K::PACKAGE => "module",
        K::CLASS => "class",
        K::METHOD => "method",
        K::PROPERTY | K::FIELD => "field",
        K::CONSTRUCTOR => "constructor",
        K::ENUM => "enum",
        K::INTERFACE => "interface",
        K::FUNCTION => "function",
        K::VARIABLE => "variable",
        K::CONSTANT => "constant",
        K::STRUCT => "struct",
        K::ENUM_MEMBER => "variant",
        K::TYPE_PARAMETER => "type parameter",
        _ => "symbol",
    }
}

/// Rows for a `workspace/symbol` answer, at most `MAX_SYMBOLS`; a symbol
/// whose file isn't a local path is left out.
pub(crate) fn symbol_rows(root: &Path, response: lsp_types::WorkspaceSymbolResponse) -> Vec<(String, GrepMatch)> {
    let place = |uri: &lsp_types::Uri, at: Option<lsp_types::Position>| {
        let path = fenix_lsp::uri_to_path(uri)?;
        let at = at.unwrap_or_default();
        Some((path, at.line as usize + 1, at.character as usize + 1))
    };
    let found: Vec<(String, lsp_types::SymbolKind, Option<String>, Option<(PathBuf, usize, usize)>)> = match response {
        lsp_types::WorkspaceSymbolResponse::Flat(symbols) => symbols.into_iter().map(|s| (s.name, s.kind, s.container_name, place(&s.location.uri, Some(s.location.range.start)))).collect(),
        lsp_types::WorkspaceSymbolResponse::Nested(symbols) => symbols
            .into_iter()
            .map(|s| {
                let at = match &s.location {
                    lsp_types::OneOf::Left(location) => place(&location.uri, Some(location.range.start)),
                    lsp_types::OneOf::Right(location) => place(&location.uri, None),
                };
                (s.name, s.kind, s.container_name, at)
            })
            .collect(),
    };
    found
        .into_iter()
        .filter_map(|(name, kind, container, at)| {
            let (path, line, col) = at?;
            let within = container.filter(|c| !c.is_empty()).map(|c| format!(" in {c}")).unwrap_or_default();
            let label = format!("{name}  {}{within}  {}:{line}", kind_word(kind), App::relative_label(root, &path));
            Some((label, GrepMatch { path, line, col, text: name }))
        })
        .take(MAX_SYMBOLS)
        .collect()
}

impl App {
    /// The project's root, or the working folder, for labels.
    fn label_root(&self) -> PathBuf {
        self.project_root.clone().unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
    }

    /// `code.problems` (Ctrl+Shift+M): every problem the language servers
    /// report, in every file, errors first; they're the quickfix list too,
    /// so F3 and `SPC p n` go through them.
    pub(crate) fn picker_problems(&mut self) {
        let root = self.label_root();
        let problems: Vec<(PathBuf, Vec<lsp_types::Diagnostic>)> = self.diagnostics.paths().into_iter().map(|p| {
            let found = self.all_diagnostics(&p);
            (p, found)
        }).collect();
        let rows = problem_rows(&root, problems);
        if rows.is_empty() {
            self.set_message("no problems reported");
            return;
        }
        let errors = rows.iter().filter(|(l, _)| l.starts_with("error")).count();
        self.quickfix = rows.iter().map(|(_, m)| QuickfixEntry::Grep(m.clone())).collect();
        self.quickfix_index = None;
        self.search.quickfix_page = None;
        let total = rows.len();
        let candidates = rows.into_iter().map(|(label, m)| fenix_picker::Candidate::new(label, m)).collect();
        self.enter_picker(ActivePicker::Problems(fenix_picker::PickerState::new(candidates)));
        self.set_message(format!("{total} problem{}, {errors} error{}", if total == 1 { "" } else { "s" }, if errors == 1 { "" } else { "s" }));
    }

    /// The language servers running that find a project's symbols.
    fn symbol_servers(&self) -> Vec<LspKey> {
        let mut keys: Vec<LspKey> = self
            .lsp_sessions
            .iter()
            .filter(|(_, s)| s.capabilities.as_ref().and_then(|c| c.workspace_symbol_provider.as_ref()).is_some_and(|p| !matches!(p, lsp_types::OneOf::Left(false))))
            .map(|(k, _)| k.clone())
            .collect();
        keys.sort_by_key(|k| format!("{k:?}"));
        keys
    }

    /// `code.project_symbols` (Ctrl+T): the project's symbols, as the
    /// language servers find them for what's typed.
    pub(crate) fn picker_project_symbols(&mut self) {
        if self.symbol_servers().is_empty() {
            self.set_error("no language server running here finds the project's symbols");
            return;
        }
        self.symbol_search = SymbolSearch::default();
        self.enter_picker(ActivePicker::ProjectSymbols(fenix_picker::PickerState::new(Vec::new())));
        self.ask_project_symbols();
    }

    /// The symbol picker's query, asked of the servers when it's changed
    /// since it was last asked.
    pub(crate) fn ask_project_symbols(&mut self) {
        let Some(ActivePicker::ProjectSymbols(state)) = &self.active_picker else { return };
        let query = state.query().to_string();
        if self.symbol_search.query.as_deref() == Some(query.as_str()) {
            return;
        }
        self.symbol_search.generation += 1;
        self.symbol_search.query = Some(query.clone());
        let generation = self.symbol_search.generation;
        for key in self.symbol_servers() {
            let Some(session) = self.lsp_sessions.get_mut(&key) else { continue };
            let params = lsp_types::WorkspaceSymbolParams { query: query.clone(), work_done_progress_params: Default::default(), partial_result_params: Default::default() };
            if let Ok(request) = session.client.request::<lsp_types::request::WorkspaceSymbolRequest>(params) {
                session.pending.insert(request, PendingLspRequest::ProjectSymbols { generation });
            }
        }
    }

    /// A server's answer to the symbol picker's question: its rows in
    /// place of its last ones, if the question is still the latest.
    pub(crate) fn project_symbols_answered(&mut self, key: LspKey, generation: u64, result: Result<serde_json::Value, String>) {
        if generation != self.symbol_search.generation {
            return;
        }
        let root = self.label_root();
        let rows = match result {
            Ok(value) => serde_json::from_value::<Option<lsp_types::WorkspaceSymbolResponse>>(value).ok().flatten().map(|r| symbol_rows(&root, r)).unwrap_or_default(),
            Err(message) => {
                self.set_error(format!("{}: {message}", self.lsp_server_name(&key)));
                Vec::new()
            }
        };
        self.symbol_search.answers.retain(|(k, _)| *k != key);
        self.symbol_search.answers.push((key, rows));
        let candidates: Vec<_> = self.symbol_search.answers.iter().flat_map(|(_, rows)| rows.iter().cloned()).map(|(label, m)| fenix_picker::Candidate::new(label, m)).collect();
        if let Some(ActivePicker::ProjectSymbols(state)) = &mut self.active_picker {
            state.replace_candidates(candidates);
        }
        self.wake_caret();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(line: u32, character: u32) -> lsp_types::Range {
        let p = lsp_types::Position { line, character };
        lsp_types::Range { start: p, end: p }
    }

    #[test]
    fn problems_are_listed_errors_first_then_by_file_and_line() {
        let root = PathBuf::from("/p");
        let d = |sev, line, message: &str| lsp_types::Diagnostic { range: at(line, 2), severity: Some(sev), message: message.to_string(), source: Some("ra".into()), ..Default::default() };
        let rows = problem_rows(&root, vec![
            (PathBuf::from("/p/b.rs"), vec![d(lsp_types::DiagnosticSeverity::WARNING, 0, "unused"), d(lsp_types::DiagnosticSeverity::ERROR, 9, "bad\nmore")]),
            (PathBuf::from("/p/a.rs"), vec![d(lsp_types::DiagnosticSeverity::ERROR, 4, "worse")]),
        ]);
        let labels: Vec<&str> = rows.iter().map(|(l, _)| l.as_str()).collect();
        assert_eq!(labels, ["error  a.rs:5  worse (ra)", "error  b.rs:10  bad (ra)", "warning  b.rs:1  unused (ra)"]);
        assert_eq!((rows[0].1.line, rows[0].1.col), (5, 3), "from 1, as a grep match's");
    }

    #[test]
    fn symbols_are_listed_with_their_kind_and_place() {
        // Absolute on every platform: a URI needs a path that is.
        let root = std::env::temp_dir().join("fenix-symbols-p");
        let uri = fenix_lsp::path_to_uri(&root.join("src").join("app.rs")).unwrap();
        #[allow(deprecated)]
        let flat = lsp_types::SymbolInformation { name: "App".into(), kind: lsp_types::SymbolKind::STRUCT, tags: None, deprecated: None, location: lsp_types::Location { uri: uri.clone(), range: at(41, 4) }, container_name: Some("app".into()) };
        let rows = symbol_rows(&root, lsp_types::WorkspaceSymbolResponse::Flat(vec![flat]));
        assert_eq!(rows.len(), 1);
        assert!(rows[0].0.starts_with("App  struct in app  "), "{}", rows[0].0);
        assert!(rows[0].0.ends_with("app.rs:42"), "{}", rows[0].0);
        assert_eq!((rows[0].1.line, rows[0].1.col), (42, 5));
        let nested = lsp_types::WorkspaceSymbol { name: "run".into(), kind: lsp_types::SymbolKind::FUNCTION, tags: None, container_name: None, location: lsp_types::OneOf::Right(lsp_types::WorkspaceLocation { uri }), data: None };
        let rows = symbol_rows(&root, lsp_types::WorkspaceSymbolResponse::Nested(vec![nested]));
        assert_eq!(rows[0].1.line, 1, "a symbol known only by its file: its start");
    }

    #[test]
    fn ctrl_shift_m_lists_the_problems_and_goes_to_one() {
        let (mut app, f) = super::super::test_keys::app_on_file("a.py", "x = 1\ny = 2\n");
        app.config.set_value("editor.keys", Some(fenix_config::Value::Text("standard".into())));
        app.sync_key_policy();
        let canonical = fenix_lsp::normalize(std::fs::canonicalize(&f.path).unwrap());
        let uri = fenix_lsp::path_to_uri(&canonical).unwrap();
        let key = app.lsp_key(fenix_syntax::LanguageId::Python);
        let error = lsp_types::Diagnostic { range: at(1, 4), severity: Some(lsp_types::DiagnosticSeverity::ERROR), message: "bad".to_string(), ..Default::default() };
        app.apply_lsp_diagnostics(&key, lsp_types::PublishDiagnosticsParams { uri, diagnostics: vec![error], version: None });
        app.press("C-S-m");
        assert!(matches!(app.active_picker, Some(ActivePicker::Problems(_))));
        app.press("Enter");
        assert_eq!(app.line_col(), (1, 4));
    }

    #[test]
    fn with_no_problems_or_servers_they_say_so() {
        let (mut app, _f) = super::super::test_keys::app_on_file("a.txt", "");
        app.run_cmd("code.problems");
        assert!(app.active_picker.is_none());
        assert!(app.modeline_text().contains("no problems"), "{}", app.modeline_text());
        app.run_cmd("code.project_symbols");
        assert!(app.active_picker.is_none());
    }
}
