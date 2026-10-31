//! Being a language server: hover, completion, definition and
//! diagnostics for Fenix's languages, merged with every other server's.
//! The manifest says which (`[[contributes.language_servers]]`); Fenix
//! talks LSP to it, a project a server, through [`Extension::lsp_message`],
//! which hands each message to [`Servers::handle`].
//!
//! ```no_run
//! use fenix_extension_api::lsp::{types::*, Documents, LanguageServer, Servers};
//! use fenix_extension_api::Extension;
//!
//! #[derive(Default)]
//! struct Tcl {
//!     servers: Servers,
//! }
//!
//! struct Words;
//!
//! impl LanguageServer for Words {
//!     fn capabilities(&self) -> ServerCapabilities {
//!         ServerCapabilities { hover_provider: Some(HoverProviderCapability::Simple(true)), ..Default::default() }
//!     }
//!
//!     fn hover(&mut self, _server: u64, docs: &Documents, params: HoverParams) -> Option<Hover> {
//!         let word = docs.word_at(&params.text_document_position_params)?;
//!         Some(Hover { contents: HoverContents::Scalar(MarkedString::String(format!("`{word}`"))), range: None })
//!     }
//! }
//!
//! impl Extension for Tcl {
//!     fn lsp_message(&mut self, server: u64, message: &str) {
//!         self.servers.handle(server, message, &mut Words);
//!     }
//! }
//! ```
//!
//! [`Extension::lsp_message`]: crate::Extension::lsp_message

use std::collections::HashMap;

pub use lsp_types as types;
use lsp_types::{
    CompletionParams, CompletionResponse, Diagnostic, GotoDefinitionParams, GotoDefinitionResponse, Hover, HoverParams, Position, PublishDiagnosticsParams, ServerCapabilities,
    TextDocumentPositionParams, TextDocumentSyncCapability, TextDocumentSyncKind, Uri,
};
use serde_json::{json, Value};

/// What a language server answers. Every method has a default: it
/// answers only what its capabilities say it does.
pub trait LanguageServer {
    /// What it does: its answer to `initialize`. Documents are sent whole
    /// on every change, whatever this says about syncing.
    fn capabilities(&self) -> ServerCapabilities;

    /// Server `server` started, for the project at `root`.
    fn initialized(&mut self, _server: u64, _root: Option<&str>) {}

    /// A document was opened or changed: a good time to say its problems
    /// ([`publish_diagnostics`]).
    fn changed(&mut self, _server: u64, _docs: &Documents, _uri: &Uri) {}

    fn hover(&mut self, _server: u64, _docs: &Documents, _params: HoverParams) -> Option<Hover> {
        None
    }

    fn completion(&mut self, _server: u64, _docs: &Documents, _params: CompletionParams) -> Option<CompletionResponse> {
        None
    }

    fn definition(&mut self, _server: u64, _docs: &Documents, _params: GotoDefinitionParams) -> Option<GotoDefinitionResponse> {
        None
    }

    /// Any other request: its result, or `None` for "no such method".
    fn other(&mut self, _server: u64, _docs: &Documents, _method: &str, _params: Value) -> Option<Result<Value, String>> {
        None
    }
}

/// The documents Fenix has open with one server: their text, by URI.
#[derive(Debug, Default)]
pub struct Documents {
    texts: HashMap<String, String>,
    /// The project it serves, from `initialize`.
    pub root: Option<String>,
}

impl Documents {
    /// Document `uri`'s text, as Fenix has it now (saved or not).
    pub fn text(&self, uri: &Uri) -> Option<&str> {
        self.texts.get(uri.as_str()).map(String::as_str)
    }

    /// Every open document's URI.
    pub fn uris(&self) -> impl Iterator<Item = &str> {
        self.texts.keys().map(String::as_str)
    }

    /// Line `line` of `uri` (from 0).
    pub fn line(&self, uri: &Uri, line: u32) -> Option<&str> {
        self.text(uri)?.lines().nth(line as usize)
    }

    /// The word (letters, digits, `_`, `:`, `.`, `-`) at a position.
    pub fn word_at(&self, at: &TextDocumentPositionParams) -> Option<String> {
        let line = self.line(&at.text_document.uri, at.position.line)?;
        let chars: Vec<char> = line.chars().collect();
        let col = utf16_to_char(line, at.position.character).min(chars.len());
        let word = |c: char| c.is_alphanumeric() || matches!(c, '_' | ':' | '.' | '-');
        let start = (0..col).rev().take_while(|&i| word(chars[i])).last().unwrap_or(col);
        let end = (col..chars.len()).take_while(|&i| word(chars[i])).last().map_or(col, |i| i + 1);
        (start < end).then(|| chars[start..end].iter().collect())
    }
}

/// An LSP position's character (UTF-16 units) as a char index in `line`.
pub fn utf16_to_char(line: &str, character: u32) -> usize {
    let mut units = 0;
    for (i, c) in line.chars().enumerate() {
        if units >= character as usize {
            return i;
        }
        units += c.len_utf16();
    }
    line.chars().count()
}

/// A char index in `line` as an LSP position's character (UTF-16 units).
pub fn char_to_utf16(line: &str, index: usize) -> u32 {
    line.chars().take(index).map(char::len_utf16).sum::<usize>() as u32
}

/// A position: line and character, both from 0, the character in UTF-16
/// units as LSP counts them.
pub fn position(line: u32, character: u32) -> Position {
    Position { line, character }
}

/// Every server it runs (one a project), and their documents.
#[derive(Debug, Default)]
pub struct Servers {
    docs: HashMap<u64, Documents>,
}

impl Servers {
    /// Server `server`'s documents.
    pub fn documents(&self, server: u64) -> Option<&Documents> {
        self.docs.get(&server)
    }

    /// One message from Fenix for server `server`, answered by `ls`.
    pub fn handle(&mut self, server: u64, message: &str, ls: &mut impl LanguageServer) {
        let Ok(msg) = serde_json::from_str::<Value>(message) else { return };
        let method = msg.get("method").and_then(Value::as_str).unwrap_or_default().to_string();
        let params = msg.get("params").cloned().unwrap_or(Value::Null);
        let id = msg.get("id").cloned();
        let docs = self.docs.entry(server).or_default();
        let answer: Option<Result<Value, String>> = match method.as_str() {
            "initialize" => {
                docs.root = params.get("rootUri").and_then(Value::as_str).map(str::to_string);
                let mut capabilities = ls.capabilities();
                capabilities.text_document_sync = Some(TextDocumentSyncCapability::Kind(TextDocumentSyncKind::FULL));
                Some(Ok(json!({ "capabilities": capabilities, "serverInfo": { "name": "fenix-extension" } })))
            }
            "initialized" => {
                let root = docs.root.clone();
                ls.initialized(server, root.as_deref());
                None
            }
            "shutdown" => Some(Ok(Value::Null)),
            "exit" => {
                self.docs.remove(&server);
                return;
            }
            "textDocument/didOpen" => {
                let (Some(uri), Some(text)) = (params.pointer("/textDocument/uri").and_then(Value::as_str), params.pointer("/textDocument/text").and_then(Value::as_str)) else { return };
                docs.texts.insert(uri.to_string(), text.to_string());
                if let Ok(uri) = uri.parse::<Uri>() {
                    ls.changed(server, docs, &uri);
                }
                None
            }
            "textDocument/didChange" => {
                let Some(uri) = params.pointer("/textDocument/uri").and_then(Value::as_str) else { return };
                // Whole documents (it said `FULL`): the last change is it.
                if let Some(text) = params.get("contentChanges").and_then(Value::as_array).and_then(|c| c.last()).and_then(|c| c.get("text")).and_then(Value::as_str) {
                    docs.texts.insert(uri.to_string(), text.to_string());
                }
                if let Ok(uri) = uri.parse::<Uri>() {
                    ls.changed(server, docs, &uri);
                }
                None
            }
            "textDocument/didClose" => {
                if let Some(uri) = params.pointer("/textDocument/uri").and_then(Value::as_str) {
                    docs.texts.remove(uri);
                }
                None
            }
            "textDocument/hover" => Some(typed(params, |p| ls.hover(server, docs, p))),
            "textDocument/completion" => Some(typed(params, |p| ls.completion(server, docs, p))),
            "textDocument/definition" => Some(typed(params, |p| ls.definition(server, docs, p))),
            _ if id.is_some() => Some(ls.other(server, docs, &method, params).unwrap_or_else(|| Err(format!("no method {method}")))),
            _ => None,
        };
        let (Some(id), Some(answer)) = (id, answer) else { return };
        let reply = match answer {
            Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            Err(why) => json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32601, "message": why } }),
        };
        crate::host::lsp_send(server, &reply.to_string());
    }
}

/// `params` as `P`, answered by `f`, its answer as JSON.
fn typed<P: serde::de::DeserializeOwned, R: serde::Serialize>(params: Value, f: impl FnOnce(P) -> Option<R>) -> Result<Value, String> {
    let params: P = serde_json::from_value(params).map_err(|e| e.to_string())?;
    Ok(serde_json::to_value(f(params)).unwrap_or(Value::Null))
}

/// Says document `uri`'s problems, from server `server`: they show in
/// the file and its project's problem list, with every other server's.
pub fn publish_diagnostics(server: u64, uri: Uri, diagnostics: Vec<Diagnostic>) {
    let params = PublishDiagnosticsParams { uri, diagnostics, version: None };
    let message = json!({ "jsonrpc": "2.0", "method": "textDocument/publishDiagnostics", "params": params });
    crate::host::lsp_send(server, &message.to_string());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_word_is_found_at_a_position() {
        let mut docs = Documents::default();
        docs.texts.insert("file:///a.tcl".into(), "set x [expr {$y + 1}]\nproc ::ns::go {} {}\n".into());
        let at = |line, character| TextDocumentPositionParams { text_document: lsp_types::TextDocumentIdentifier { uri: "file:///a.tcl".parse().unwrap() }, position: position(line, character) };
        assert_eq!(docs.word_at(&at(0, 1)).as_deref(), Some("set"));
        assert_eq!(docs.word_at(&at(1, 8)).as_deref(), Some("::ns::go"));
        assert_eq!(docs.word_at(&at(0, 3)).as_deref(), Some("set"), "just after it");
        assert_eq!(docs.word_at(&at(0, 6)), None, "on [ after a space");
        assert_eq!(utf16_to_char("a😀b", 3), 2);
        assert_eq!(char_to_utf16("a😀b", 2), 3);
    }
}
