//! The control protocol end to end: a client and `serve` talking over
//! the real endpoint, and `serve` fed raw bytes for what a well-behaved
//! client never sends.

use std::io::{Cursor, Write};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use fenix_control::protocol::{code, method, OpenProjectParams, CAPABILITIES, MAX_BODY, PROTOCOL};
use fenix_control::{
    serve, AgendaTask, AgendaTimer, Call, CaptureParams, CaptureResult, CaptureTarget, Client,
    ClientError, ClockInParams, CommandInfo, CommandsResult, Endpoint, OpenParams, OpenPath,
    RpcError, RunCommandParams, ServerInfo, StateResult, TaskStatus, TasksParams, TasksResult,
};
use serde_json::{json, Value};

fn info() -> ServerInfo {
    ServerInfo::new("9.8.7", 4242)
}

fn unique() -> Endpoint {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    Endpoint::instance(&format!(
        "proto-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ))
}

/// The stand-in editor's agenda.
fn tasks() -> Vec<AgendaTask> {
    vec![
        AgendaTask {
            id: 1,
            title: "Write the report".into(),
            status: TaskStatus::InProgress,
            jira_key: Some("ABC-12".into()),
            elapsed_minutes: 95,
        },
        AgendaTask {
            id: 2,
            title: "File it".into(),
            status: TaskStatus::Todo,
            jira_key: None,
            elapsed_minutes: 0,
        },
    ]
}

/// A stand-in editor: records every call and answers each the way
/// Fenix would, from fixed data.
fn editor(calls: Arc<Mutex<Vec<Call>>>) -> impl FnMut(Call) -> Result<Value, RpcError> {
    move |call| {
        let reply = match &call {
            Call::State => Ok(serde_json::to_value(StateResult {
                project: Some("C:\\work\\fenix".to_string()),
                file: Some("C:\\work\\fenix\\Cargo.toml".to_string()),
                line: Some(3),
                agenda_timer: Some(AgendaTimer {
                    task_id: 1,
                    title: "Write the report".into(),
                    since: "2026-10-12T09:30:00+02:00".into(),
                }),
            })
            .unwrap()),
            Call::OpenProject(p) if p.root.is_empty() => {
                Err(RpcError::invalid_params("not a folder"))
            }
            Call::Commands => Ok(serde_json::to_value(CommandsResult {
                commands: vec![
                    CommandInfo {
                        id: "agenda.open".into(),
                        description: "Open the agenda".into(),
                        keys: Some("SPC a a".into()),
                        external: true,
                    },
                    CommandInfo {
                        id: "file.save".into(),
                        description: "Save the current file".into(),
                        keys: Some("SPC f s".into()),
                        external: false,
                    },
                    CommandInfo {
                        id: "view.reset_font_size".into(),
                        description: "Reset the body text size".into(),
                        keys: None,
                        external: false,
                    },
                ],
            })
            .unwrap()),
            Call::RunCommand(p) if p.id == "file.save" => Err(RpcError::invalid_params(
                "file.save can't be run from outside Fenix",
            )),
            Call::RunCommand(p) if p.id != "agenda.open" => {
                Err(RpcError::invalid_params(format!("no command {}", p.id)))
            }
            Call::NotebookCapture(p) => Ok(serde_json::to_value(CaptureResult {
                entry: match p.target {
                    CaptureTarget::Inbox => "C:\\nb\\notes\\inbox.md".into(),
                    CaptureTarget::Journal => "C:\\nb\\journal\\2026-10-12.md".into(),
                },
            })
            .unwrap()),
            Call::AgendaTasks(p) => Ok(serde_json::to_value(TasksResult {
                tasks: tasks()
                    .into_iter()
                    .filter(|t| p.status.as_ref().is_none_or(|s| s.contains(&t.status)))
                    .collect(),
            })
            .unwrap()),
            Call::AgendaClockIn(p) if p.task_id > 2 => {
                Err(RpcError::invalid_params(format!("no task {}", p.task_id)))
            }
            _ => Ok(json!({})),
        };
        calls.lock().unwrap().push(call);
        reply
    }
}

/// Serves one connection on a real endpoint, in the background; the
/// calls it handed the editor come back when it ends.
fn serve_one(endpoint: &Endpoint) -> mpsc::Receiver<Vec<Call>> {
    let mut listener = endpoint.bind().unwrap();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let stream = listener.accept().unwrap();
        let writer = stream.try_clone().unwrap();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let _ = serve(stream, writer, &info(), editor(Arc::clone(&calls)));
        tx.send(calls.lock().unwrap().clone()).unwrap();
    });
    rx
}

fn rpc_code(result: Result<impl std::fmt::Debug, ClientError>) -> (i64, Option<Value>) {
    match result {
        Err(ClientError::Rpc(e)) => (e.code, e.data),
        other => panic!("expected an RPC error, got {other:?}"),
    }
}

#[test]
fn hello_reports_the_version_pid_and_capabilities() {
    let endpoint = unique();
    let calls = serve_one(&endpoint);
    let mut client = Client::connect(&endpoint).unwrap().expect("listening");
    let hello = client.hello("test", "0.1").unwrap();
    assert_eq!(hello.protocol, PROTOCOL);
    assert_eq!(hello.version, "9.8.7");
    assert_eq!(hello.pid, 4242);
    assert_eq!(
        hello.capabilities,
        vec![
            "fenix/open",
            "fenix/openProject",
            "fenix/state",
            "fenix/commands",
            "fenix/runCommand",
            "fenix/notebook.capture",
            "fenix/agenda.tasks",
            "fenix/agenda.clockIn",
            "fenix/agenda.clockOut",
            "fenix/diagnostics",
            "fenix/buffers",
            "fenix/selection",
            "fenix/tasks",
            "fenix/tasks.run",
            "fenix/agentCommand",
        ]
    );
    assert_eq!(hello.capabilities, CAPABILITIES);
    drop(client);
    // Hello is the protocol's own: the editor never sees it.
    assert!(calls.recv().unwrap().is_empty());
}

#[test]
fn several_requests_on_one_connection_each_get_an_answer() {
    let endpoint = unique();
    let calls = serve_one(&endpoint);
    let mut client = Client::connect(&endpoint).unwrap().expect("listening");
    client.hello("test", "0.1").unwrap();
    let open = OpenParams {
        paths: vec![
            OpenPath {
                path: "C:\\x.rs".into(),
                line: Some(12),
                column: Some(3),
            },
            OpenPath {
                path: "/y.rs".into(),
                line: Some(4),
                column: None,
            },
            OpenPath {
                path: "z.md".into(),
                line: None,
                column: None,
            },
        ],
        new_window: false,
        focus: true,
    };
    client.open(&open).unwrap();
    client.open_project("C:\\work\\fenix").unwrap();
    let state = client.state().unwrap();
    assert_eq!(state.agenda_timer.as_ref().map(|t| t.task_id), Some(1));
    assert_eq!(state.project.as_deref(), Some("C:\\work\\fenix"));
    assert_eq!(state.file.as_deref(), Some("C:\\work\\fenix\\Cargo.toml"));
    assert_eq!(state.line, Some(3));
    assert_eq!(rpc_code(client.open_project("")).0, code::INVALID_PARAMS);
    drop(client);
    assert_eq!(
        calls.recv().unwrap(),
        vec![
            Call::Open(open),
            Call::OpenProject(OpenProjectParams {
                root: "C:\\work\\fenix".into()
            }),
            Call::State,
            Call::OpenProject(OpenProjectParams {
                root: String::new()
            }),
        ]
    );
}

#[test]
fn an_unsupported_protocol_says_which_are() {
    let endpoint = unique();
    let _calls = serve_one(&endpoint);
    let mut client = Client::connect(&endpoint).unwrap().expect("listening");
    let result = client.request::<Value>(
        method::HELLO,
        json!({ "protocol": 2, "client": { "name": "future", "version": "9", "pid": 1 } }),
    );
    assert_eq!(
        rpc_code(result),
        (
            code::UNSUPPORTED_PROTOCOL,
            Some(json!({ "supported": [1] }))
        )
    );
    // And it didn't count as a hello.
    assert_eq!(rpc_code(client.state()).0, code::HELLO_FIRST);
}

#[test]
fn every_method_waits_for_hello() {
    let endpoint = unique();
    let calls = serve_one(&endpoint);
    let mut client = Client::connect(&endpoint).unwrap().expect("listening");
    assert_eq!(rpc_code(client.state()).0, code::HELLO_FIRST);
    assert_eq!(
        rpc_code(client.open(&OpenParams {
            paths: vec![],
            new_window: false,
            focus: true
        }))
        .0,
        code::HELLO_FIRST
    );
    assert_eq!(
        rpc_code(client.request::<Value>("fenix/nope", json!({}))).0,
        code::HELLO_FIRST
    );
    drop(client);
    assert!(calls.recv().unwrap().is_empty());
}

#[test]
fn an_unknown_method_is_not_found() {
    let endpoint = unique();
    let _calls = serve_one(&endpoint);
    let mut client = Client::connect(&endpoint).unwrap().expect("listening");
    client.hello("test", "0.1").unwrap();
    assert_eq!(
        rpc_code(client.request::<Value>("fenix/stateChanged", json!({}))).0,
        code::METHOD_NOT_FOUND
    );
    // The connection is still good afterwards.
    assert!(client.state().is_ok());
}

#[test]
fn commands_list_keys_and_whether_they_run_from_outside() {
    let endpoint = unique();
    let calls = serve_one(&endpoint);
    let mut client = Client::connect(&endpoint).unwrap().expect("listening");
    client.hello("test", "0.1").unwrap();
    let raw: Value = client.request(method::COMMANDS, json!({})).unwrap();
    assert_eq!(
        raw["commands"][0],
        json!({ "id": "agenda.open", "description": "Open the agenda", "keys": "SPC a a", "external": true })
    );
    // `keys` is left out, not null, when nothing is bound.
    assert_eq!(
        raw["commands"][2],
        json!({ "id": "view.reset_font_size", "description": "Reset the body text size", "external": false })
    );
    assert_eq!(client.commands().unwrap().len(), 3);
    drop(client);
    assert_eq!(calls.recv().unwrap(), vec![Call::Commands, Call::Commands]);
}

#[test]
fn run_command_hands_over_the_id_and_relays_a_refusal() {
    let endpoint = unique();
    let calls = serve_one(&endpoint);
    let mut client = Client::connect(&endpoint).unwrap().expect("listening");
    client.hello("test", "0.1").unwrap();
    client.run_command("agenda.open").unwrap();
    let refused = client.run_command("file.save").unwrap_err();
    let ClientError::Rpc(e) = refused else {
        panic!("{refused:?}")
    };
    assert_eq!(e.code, code::INVALID_PARAMS);
    assert!(e.message.contains("outside"), "{}", e.message);
    assert_eq!(
        rpc_code(client.run_command("no.such")).0,
        code::INVALID_PARAMS
    );
    // No id at all is the server's own refusal; the editor never sees it.
    assert_eq!(
        rpc_code(client.request::<Value>(method::RUN_COMMAND, json!({}))).0,
        code::INVALID_PARAMS
    );
    drop(client);
    let ids: Vec<String> = calls
        .recv()
        .unwrap()
        .into_iter()
        .map(|c| match c {
            Call::RunCommand(RunCommandParams { id }) => id,
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(ids, ["agenda.open", "file.save", "no.such"]);
}

#[test]
fn capture_defaults_to_the_inbox_and_refuses_empty_text() {
    let endpoint = unique();
    let calls = serve_one(&endpoint);
    let mut client = Client::connect(&endpoint).unwrap().expect("listening");
    client.hello("test", "0.1").unwrap();
    let entry: CaptureResult = client
        .request(method::NOTEBOOK_CAPTURE, json!({ "text": "call OBSW" }))
        .unwrap();
    assert!(entry.entry.ends_with("inbox.md"));
    let entry = client
        .notebook_capture("retry is 3", CaptureTarget::Journal)
        .unwrap();
    assert!(entry.ends_with("2026-10-12.md"));
    for params in [
        json!({ "text": "  \n" }),
        json!({}),
        json!({ "text": "x", "target": "desk" }),
    ] {
        assert_eq!(
            rpc_code(client.request::<Value>(method::NOTEBOOK_CAPTURE, params)).0,
            code::INVALID_PARAMS
        );
    }
    drop(client);
    assert_eq!(
        calls.recv().unwrap(),
        vec![
            Call::NotebookCapture(CaptureParams {
                text: "call OBSW".into(),
                target: CaptureTarget::Inbox
            }),
            Call::NotebookCapture(CaptureParams {
                text: "retry is 3".into(),
                target: CaptureTarget::Journal
            }),
        ]
    );
}

#[test]
fn agenda_tasks_filter_by_status_and_clock_in_takes_their_ids() {
    let endpoint = unique();
    let calls = serve_one(&endpoint);
    let mut client = Client::connect(&endpoint).unwrap().expect("listening");
    client.hello("test", "0.1").unwrap();
    let raw: Value = client.request(method::AGENDA_TASKS, json!({})).unwrap();
    assert_eq!(
        raw["tasks"][0],
        json!({ "id": 1, "title": "Write the report", "status": "inProgress", "jiraKey": "ABC-12", "elapsedMinutes": 95 })
    );
    assert!(raw["tasks"][1].get("jiraKey").is_none());
    let todo = client.agenda_tasks(Some(vec![TaskStatus::Todo])).unwrap();
    assert_eq!(todo.tasks.len(), 1);
    assert_eq!(todo.tasks[0].id, 2);
    assert_eq!(
        rpc_code(client.request::<Value>(method::AGENDA_TASKS, json!({ "status": ["someday"] }))).0,
        code::INVALID_PARAMS
    );
    client.agenda_clock_in(1).unwrap();
    // An id kept as text is the same id.
    client
        .request::<Value>(method::AGENDA_CLOCK_IN, json!({ "taskId": "2" }))
        .unwrap();
    assert_eq!(rpc_code(client.agenda_clock_in(9)).0, code::INVALID_PARAMS);
    for params in [
        json!({}),
        json!({ "taskId": "t1" }),
        json!({ "taskId": -1 }),
    ] {
        assert_eq!(
            rpc_code(client.request::<Value>(method::AGENDA_CLOCK_IN, params)).0,
            code::INVALID_PARAMS
        );
    }
    client.agenda_clock_out().unwrap();
    drop(client);
    assert_eq!(
        calls.recv().unwrap(),
        vec![
            Call::AgendaTasks(TasksParams { status: None }),
            Call::AgendaTasks(TasksParams {
                status: Some(vec![TaskStatus::Todo])
            }),
            Call::AgendaClockIn(ClockInParams { task_id: 1 }),
            Call::AgendaClockIn(ClockInParams { task_id: 2 }),
            Call::AgendaClockIn(ClockInParams { task_id: 9 }),
            Call::AgendaClockOut,
        ]
    );
}

#[test]
fn a_server_with_fewer_capabilities_lists_and_answers_only_those() {
    let mut info = ServerInfo::new("1.1.0", 7);
    info.capabilities = vec![method::OPEN.into(), method::STATE.into()];
    let mut input = frame(HELLO);
    for (id, m) in [
        (2, method::AGENDA_TASKS),
        (3, method::COMMANDS),
        (4, method::RUN_COMMAND),
        (5, method::STATE),
    ] {
        input.extend(frame(&format!(
            r#"{{"jsonrpc":"2.0","id":{id},"method":"{m}","params":{{"id":"agenda.open"}}}}"#
        )));
    }
    let calls = Arc::new(Mutex::new(Vec::new()));
    let mut output = Vec::new();
    let _ = serve(
        Cursor::new(input),
        &mut output,
        &info,
        editor(Arc::clone(&calls)),
    );
    let mut cursor = Cursor::new(output);
    let mut replies: Vec<Value> = Vec::new();
    while let Some(body) = fenix_rpc::read_message(&mut cursor).unwrap() {
        replies.push(serde_json::from_slice(&body).unwrap());
    }
    assert_eq!(
        replies[0]["result"]["capabilities"],
        json!(["fenix/open", "fenix/state"])
    );
    for reply in &replies[1..4] {
        assert_eq!(reply["error"]["code"], json!(code::METHOD_NOT_FOUND));
    }
    assert!(replies[4]["result"].is_object());
    assert_eq!(*calls.lock().unwrap(), vec![Call::State]);
}

#[test]
fn open_defaults_to_focus_and_the_focused_window() {
    let params: OpenParams =
        serde_json::from_value(json!({ "paths": [{ "path": "a.rs" }] })).unwrap();
    assert!(params.focus);
    assert!(!params.new_window);
    let params: OpenParams =
        serde_json::from_value(json!({ "paths": [], "newWindow": true, "focus": false })).unwrap();
    assert!(!params.focus);
    assert!(params.new_window);
}

// -- Raw bytes ------------------------------------------------------------

fn frame(body: &str) -> Vec<u8> {
    format!("Content-Length: {}\r\n\r\n{body}", body.len()).into_bytes()
}

/// Every reply `serve` writes for `input`, and the calls it made.
fn exchange(input: &[u8]) -> (Vec<Value>, Vec<Call>) {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let mut output = Vec::new();
    let _ = serve(
        Cursor::new(input.to_vec()),
        &mut output,
        &info(),
        editor(Arc::clone(&calls)),
    );
    let mut replies = Vec::new();
    let mut cursor = Cursor::new(output);
    while let Some(body) = fenix_rpc::read_message(&mut cursor).unwrap() {
        replies.push(serde_json::from_slice(&body).unwrap());
    }
    let calls = calls.lock().unwrap().clone();
    (replies, calls)
}

const HELLO: &str = r#"{"jsonrpc":"2.0","id":1,"method":"fenix/hello","params":{"protocol":1,"client":{"name":"t","version":"1","pid":1}}}"#;

#[test]
fn malformed_messages_get_json_rpc_errors_and_the_connection_goes_on() {
    let mut input = frame(HELLO);
    input.extend(frame("{not json"));
    input.extend(frame(
        r#"[{"jsonrpc":"2.0","id":2,"method":"fenix/state"}]"#,
    ));
    input.extend(frame(r#"{"id":3,"method":"fenix/state"}"#));
    input.extend(frame(
        r#"{"jsonrpc":"2.0","id":4,"method":"fenix/open","params":{"paths":"a.rs"}}"#,
    ));
    input.extend(frame(r#"{"jsonrpc":"2.0","method":"fenix/state"}"#));
    input.extend(frame(
        r#"{"jsonrpc":"2.0","id":"last","method":"fenix/state"}"#,
    ));
    let (replies, calls) = exchange(&input);
    let codes: Vec<(Value, Value)> = replies
        .iter()
        .map(|r| (r["id"].clone(), r["error"]["code"].clone()))
        .collect();
    assert_eq!(
        codes,
        vec![
            (json!(1), Value::Null),
            (Value::Null, json!(code::PARSE_ERROR)),
            (Value::Null, json!(code::INVALID_REQUEST)),
            (json!(3), json!(code::INVALID_REQUEST)),
            (json!(4), json!(code::INVALID_PARAMS)),
            // The notification got no reply, but did run.
            (json!("last"), Value::Null),
        ]
    );
    assert_eq!(calls, vec![Call::State, Call::State]);
}

#[test]
fn an_oversized_message_is_refused_and_ends_the_connection() {
    let mut input = frame(HELLO);
    input.extend(format!("Content-Length: {}\r\n\r\n", MAX_BODY + 1).into_bytes());
    input.extend(frame(r#"{"jsonrpc":"2.0","id":2,"method":"fenix/state"}"#));
    let (replies, calls) = exchange(&input);
    assert_eq!(replies.len(), 2);
    assert_eq!(replies[1]["error"]["code"], json!(code::INVALID_REQUEST));
    assert!(calls.is_empty());
}

#[test]
fn an_endless_header_is_refused() {
    let mut input = b"Content-Length: 2\r\nX-Pad: ".to_vec();
    input.extend(std::iter::repeat_n(b'a', 4096));
    let (replies, _) = exchange(&input);
    assert_eq!(replies.len(), 1);
    assert_eq!(replies[0]["error"]["code"], json!(code::INVALID_REQUEST));
}

#[test]
fn a_legacy_payload_is_one_open_with_no_reply() {
    let (replies, calls) = exchange(b"--new-window\nC:\\file.tcl\r\n--colour\n\nC:\\other.tcl\n");
    assert!(replies.is_empty());
    assert_eq!(
        calls,
        vec![Call::Open(OpenParams {
            paths: vec![
                OpenPath {
                    path: "C:\\file.tcl".into(),
                    line: None,
                    column: None
                },
                OpenPath {
                    path: "C:\\other.tcl".into(),
                    line: None,
                    column: None
                },
            ],
            new_window: true,
            focus: true,
        })]
    );
}

#[test]
fn a_legacy_path_that_starts_like_the_header_is_still_legacy() {
    let (_, calls) = exchange(b"Content-Len.txt\n");
    assert_eq!(
        calls,
        vec![Call::Open(OpenParams {
            paths: vec![OpenPath {
                path: "Content-Len.txt".into(),
                line: None,
                column: None
            }],
            new_window: false,
            focus: true
        })]
    );
}

#[test]
fn an_empty_legacy_payload_just_brings_fenix_forward() {
    let (_, calls) = exchange(b"");
    assert_eq!(
        calls,
        vec![Call::Open(OpenParams {
            paths: vec![],
            new_window: false,
            focus: true
        })]
    );
}

#[test]
fn a_legacy_payload_over_the_real_endpoint() {
    let endpoint = unique();
    let calls = serve_one(&endpoint);
    let mut stream = endpoint.connect().unwrap().expect("listening");
    stream.write_all(b"a.txt\nb.txt\n").unwrap();
    stream.flush().unwrap();
    drop(stream);
    let calls = calls.recv().unwrap();
    let Call::Open(open) = &calls[0] else {
        panic!("{calls:?}")
    };
    assert_eq!(
        open.paths
            .iter()
            .map(|p| p.path.as_str())
            .collect::<Vec<_>>(),
        ["a.txt", "b.txt"]
    );
}

#[test]
fn a_header_split_across_writes_is_still_framed() {
    // A client that writes its header a byte at a time.
    struct Trickle(Vec<u8>, usize);
    impl std::io::Read for Trickle {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if self.1 == self.0.len() || buf.is_empty() {
                return Ok(0);
            }
            buf[0] = self.0[self.1];
            self.1 += 1;
            Ok(1)
        }
    }
    let calls = Arc::new(Mutex::new(Vec::new()));
    let mut output = Vec::new();
    serve(
        Trickle(frame(HELLO), 0),
        &mut output,
        &info(),
        editor(calls),
    )
    .unwrap();
    let reply: Value = serde_json::from_slice(
        &fenix_rpc::read_message(&mut Cursor::new(output))
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(reply["result"]["pid"], json!(4242));
}
