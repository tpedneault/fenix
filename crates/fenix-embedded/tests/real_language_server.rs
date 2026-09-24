//! Autocompletion end to end: the language server command `fenix-
//! embedded` builds, driven by Fenix's own LSP client, with edits sent
//! the way the editor sends them (`fenix_lsp::change_event`). Needs
//! arduino-cli with `arduino:avr`, clangd and arduino-language-server;
//! ignored by default -- run with `cargo test -p fenix-embedded -- --ignored`.

use std::time::{Duration, Instant};

use fenix_lsp::{LspClient, LspEvent};
use lsp_types::notification::{DidChangeTextDocument, DidOpenTextDocument, Initialized};
use lsp_types::request::{Completion, Initialize};

fn wait_for(client: &LspClient, events: &std::sync::mpsc::Receiver<LspEvent>, id: i64) -> serde_json::Value {
    let deadline = Instant::now() + Duration::from_secs(120);
    while let Ok(event) = events.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
        match event {
            LspEvent::Response { id: got, result } if got == id => return result.expect("the request succeeded"),
            // Answer the server's own requests so it never stalls.
            LspEvent::ServerRequest { id, .. } => {
                let _ = client.respond(id, Ok(serde_json::Value::Null));
            }
            LspEvent::Disconnected(reason) => panic!("the language server went away: {reason}"),
            _ => {}
        }
    }
    panic!("no response to request {id}");
}

fn labels(result: &serde_json::Value) -> Vec<String> {
    let items = result.get("items").unwrap_or(result);
    items.as_array().into_iter().flatten().filter_map(|i| i.get("label")?.as_str()).map(|l| l.trim().to_string()).collect()
}

#[test]
#[ignore]
fn library_completion_works_through_incremental_edits() {
    let tools = fenix_embedded::Tools::discover(&fenix_embedded::ToolOverrides::default());
    let parent = std::env::temp_dir().join(format!("fenix-embedded-lsp-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&parent);
    std::fs::create_dir_all(&parent).unwrap();
    let ino = fenix_embedded::arduino::create_sketch(&parent, "LspCheck", "arduino:avr:uno").unwrap();
    let root = ino.parent().unwrap().to_path_buf();
    let original = "#include <EEPROM.h>\n\nvoid setup() {\n  Serial.begin(9600);\n}\n\nvoid loop() {\n}\n";
    std::fs::write(&ino, original).unwrap();

    let platform = fenix_embedded::detect(&root, &tools).unwrap();
    let command = platform.language_server().expect("every tool is installed");
    let mut process = std::process::Command::new(&command.program);
    process.args(&command.args).current_dir(&root);
    let (client, events) = LspClient::spawn_command(process).unwrap();

    let root_uri = fenix_lsp::path_to_uri(&root).unwrap();
    #[allow(deprecated)]
    let id = client
        .request::<Initialize>(lsp_types::InitializeParams {
            process_id: Some(std::process::id()),
            root_uri: Some(root_uri.clone()),
            workspace_folders: Some(vec![lsp_types::WorkspaceFolder { uri: root_uri, name: "root".to_string() }]),
            ..Default::default()
        })
        .unwrap();
    let capabilities: lsp_types::InitializeResult = serde_json::from_value(wait_for(&client, &events, id)).unwrap();
    assert!(fenix_lsp::wants_incremental(&capabilities.capabilities), "this server only takes incremental edits");
    client.notify::<Initialized>(lsp_types::InitializedParams {}).unwrap();

    let uri = fenix_lsp::path_to_uri(&ino).unwrap();
    client
        .notify::<DidOpenTextDocument>(lsp_types::DidOpenTextDocumentParams {
            text_document: lsp_types::TextDocumentItem { uri: uri.clone(), language_id: "cpp".to_string(), version: 0, text: original.to_string() },
        })
        .unwrap();

    // Type `EEPROM.` inside loop(), as the editor would send it.
    let edited = original.replace("void loop() {\n}", "void loop() {\n  EEPROM.\n}");
    let change = fenix_lsp::change_event(Some(original), &edited, true);
    assert!(change.range.is_some());
    client
        .notify::<DidChangeTextDocument>(lsp_types::DidChangeTextDocumentParams {
            text_document: lsp_types::VersionedTextDocumentIdentifier { uri: uri.clone(), version: 1 },
            content_changes: vec![change],
        })
        .unwrap();

    let mut found = Vec::new();
    for _ in 0..30 {
        let id = client
            .request::<Completion>(lsp_types::CompletionParams {
                text_document_position: lsp_types::TextDocumentPositionParams {
                    text_document: lsp_types::TextDocumentIdentifier { uri: uri.clone() },
                    position: lsp_types::Position { line: 7, character: 9 },
                },
                work_done_progress_params: Default::default(),
                partial_result_params: Default::default(),
                context: None,
            })
            .unwrap();
        found = labels(&wait_for(&client, &events, id));
        // Until the server has finished indexing the sketch, clangd
        // answers from a fallback with plain words -- wait that out.
        if found.iter().any(|l| l.starts_with("read(")) {
            break;
        }
        std::thread::sleep(Duration::from_secs(2));
    }
    assert!(found.iter().any(|l| l.starts_with("read(")), "EEPROM's members are offered: {found:?}");
    assert!(found.iter().any(|l| l.starts_with("put(")), "{found:?}");
    let _ = std::fs::remove_dir_all(&parent);
}
