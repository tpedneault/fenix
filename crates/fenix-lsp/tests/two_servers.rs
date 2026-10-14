//! Two real language server processes on one document: both are told
//! about it, both answer, and their answers merge as `fanout` says --
//! and one of them crashing leaves the other working.

use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use fenix_lsp::fanout::{self, Attached, Feature, Gather};
use fenix_lsp::{LspClient, LspEvent};
use lsp_types::request::{Completion, GotoDefinition, HoverRequest, Initialize};

const WAIT: Duration = Duration::from_secs(10);

struct Server {
    name: &'static str,
    primary: bool,
    client: LspClient,
    events: Receiver<LspEvent>,
    capabilities: lsp_types::ServerCapabilities,
}

fn start(name: &'static str, primary: bool, extra: &[&str], dir: &std::path::Path) -> Server {
    let mut args = vec![name.to_string()];
    args.extend(extra.iter().map(|a| a.to_string()));
    let (client, events) =
        LspClient::spawn(env!("CARGO_BIN_EXE_fake-language-server"), &args, dir).unwrap();
    let id = client.request::<Initialize>(Default::default()).unwrap();
    let result = response(&events, id).expect("initialize answered");
    let capabilities = serde_json::from_value::<lsp_types::InitializeResult>(result)
        .unwrap()
        .capabilities;
    client
        .notify::<lsp_types::notification::Initialized>(lsp_types::InitializedParams {})
        .unwrap();
    Server {
        name,
        primary,
        client,
        events,
        capabilities,
    }
}

/// The result for request `id`, skipping notifications; `None` if the
/// server went away first.
fn response(events: &Receiver<LspEvent>, id: i64) -> Option<serde_json::Value> {
    let until = Instant::now() + WAIT;
    loop {
        match events
            .recv_timeout(until.saturating_duration_since(Instant::now()))
            .expect("the server answers in time")
        {
            LspEvent::Response { id: got, result } if got == id => return result.ok(),
            LspEvent::Disconnected(_) => return None,
            _ => {}
        }
    }
}

fn diagnostics(events: &Receiver<LspEvent>) -> lsp_types::PublishDiagnosticsParams {
    let until = Instant::now() + WAIT;
    loop {
        if let LspEvent::Notification { method, params } = events
            .recv_timeout(until.saturating_duration_since(Instant::now()))
            .expect("diagnostics arrive")
        {
            if method == "textDocument/publishDiagnostics" {
                return serde_json::from_value(params).unwrap();
            }
        }
    }
}

fn open(servers: &[Server], uri: &lsp_types::Uri) {
    for server in servers {
        let item = lsp_types::TextDocumentItem {
            uri: uri.clone(),
            language_id: "rust".into(),
            version: 0,
            text: "fn main() {}\n".into(),
        };
        server
            .client
            .notify::<lsp_types::notification::DidOpenTextDocument>(
                lsp_types::DidOpenTextDocumentParams {
                    text_document: item,
                },
            )
            .unwrap();
    }
}

fn position(uri: &lsp_types::Uri) -> lsp_types::TextDocumentPositionParams {
    lsp_types::TextDocumentPositionParams {
        text_document: lsp_types::TextDocumentIdentifier { uri: uri.clone() },
        position: lsp_types::Position::new(0, 3),
    }
}

/// Sends a hover to every server routing picks and joins what comes back.
fn hover(servers: &[Server], uri: &lsp_types::Uri) -> Option<String> {
    let attached: Vec<Attached> = servers
        .iter()
        .map(|s| Attached {
            primary: s.primary,
            capabilities: &s.capabilities,
        })
        .collect();
    let targets = fanout::route(Feature::Hover, &attached);
    let mut gather: Gather<&str, Option<String>> = Gather::new(
        targets.iter().map(|&i| servers[i].name).collect(),
        Instant::now() + Duration::from_millis(300),
    );
    let sent: Vec<(usize, Option<i64>)> = targets
        .iter()
        .map(|&i| {
            let params = lsp_types::HoverParams {
                text_document_position_params: position(uri),
                work_done_progress_params: Default::default(),
            };
            (i, servers[i].client.request::<HoverRequest>(params).ok())
        })
        .collect();
    for (i, id) in sent {
        let answer = id
            .and_then(|id| response(&servers[i].events, id))
            .and_then(|v| serde_json::from_value::<Option<lsp_types::Hover>>(v).ok());
        let text = answer.map(|hover| {
            hover.and_then(|h| match h.contents {
                lsp_types::HoverContents::Markup(m) => Some(m.value),
                _ => None,
            })
        });
        gather.answer(&servers[i].name, text);
    }
    assert!(gather.is_complete());
    fanout::join_hovers(gather.into_arrived().into_iter().map(|(_, text)| text))
}

#[test]
fn two_servers_on_one_document_are_both_told_and_both_answer() {
    let dir = std::env::temp_dir();
    // Listed additional-first: routing still puts the primary's answers first.
    let servers = [
        start("kiln", false, &["--triggers", "-"], &dir),
        start("analyzer", true, &["--triggers", "."], &dir),
    ];
    let uri: lsp_types::Uri = "file:///project/src/main.rs".parse().unwrap();
    open(&servers, &uri);
    for server in &servers {
        let published = diagnostics(&server.events);
        assert_eq!(published.uri, uri);
        assert_eq!(
            published.diagnostics[0].message,
            format!("{} saw it", server.name),
            "each server got didOpen"
        );
    }

    assert_eq!(
        hover(&servers, &uri).unwrap(),
        format!("analyzer hover{}kiln hover", fanout::HOVER_SEPARATOR)
    );

    // Completion triggered by `-` goes only to the server that declared it.
    let attached: Vec<Attached> = servers
        .iter()
        .map(|s| Attached {
            primary: s.primary,
            capabilities: &s.capabilities,
        })
        .collect();
    assert_eq!(fanout::completion_targets(&attached, Some('-')), [0]);
    let mut set = fanout::CompletionSet::new(
        fanout::completion_targets(&attached, None)
            .into_iter()
            .map(|i| servers[i].name)
            .collect(),
    );
    for i in fanout::completion_targets(&attached, None) {
        let params = lsp_types::CompletionParams {
            text_document_position: position(&uri),
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
            context: None,
        };
        let id = servers[i].client.request::<Completion>(params).unwrap();
        set.store(
            servers[i].name,
            serde_json::from_value(response(&servers[i].events, id).unwrap()).unwrap(),
        );
    }
    let items: Vec<(&str, String)> = set
        .items()
        .map(|(server, item)| (*server, item.label.clone()))
        .collect();
    assert_eq!(
        items,
        [
            ("analyzer", "analyzer-item".to_string()),
            ("kiln", "kiln-item".to_string())
        ]
    );

    // Both point at the same place: one location, not two.
    let mut answers = Vec::new();
    for server in &servers {
        let params = lsp_types::GotoDefinitionParams {
            text_document_position_params: position(&uri),
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        };
        let id = server.client.request::<GotoDefinition>(params).unwrap();
        let response: lsp_types::GotoDefinitionResponse =
            serde_json::from_value(response(&server.events, id).unwrap()).unwrap();
        answers.push(fanout::locations_of(response));
    }
    assert_eq!(fanout::merge_locations(answers).len(), 1);
}

#[test]
fn a_crashed_additional_server_leaves_the_primary_working() {
    let dir = std::env::temp_dir();
    let servers = [
        start("analyzer", true, &[], &dir),
        start("kiln", false, &["--crash-on", "textDocument/hover"], &dir),
    ];
    let uri: lsp_types::Uri = "file:///project/src/lib.rs".parse().unwrap();
    open(&servers, &uri);
    for server in &servers {
        diagnostics(&server.events);
    }

    assert_eq!(
        hover(&servers, &uri).unwrap(),
        "analyzer hover",
        "the crashed server is skipped, the primary's answer stands"
    );
    let until = Instant::now() + WAIT;
    loop {
        match servers[1]
            .events
            .recv_timeout(until.saturating_duration_since(Instant::now()))
        {
            Ok(LspEvent::Disconnected(_))
            | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            Ok(_) => {}
            Err(error) => panic!("the crash is reported: {error}"),
        }
    }

    // The primary still hears about edits and still answers.
    let change = lsp_types::DidChangeTextDocumentParams {
        text_document: lsp_types::VersionedTextDocumentIdentifier {
            uri: uri.clone(),
            version: 1,
        },
        content_changes: vec![fenix_lsp::change_event(None, "fn main() { }\n", false)],
    };
    servers[0]
        .client
        .notify::<lsp_types::notification::DidChangeTextDocument>(change)
        .unwrap();
    assert_eq!(
        diagnostics(&servers[0].events).diagnostics[0].message,
        "analyzer saw it"
    );
    assert_eq!(hover(&servers[..1], &uri).unwrap(), "analyzer hover");
}
