//! Who called a service, as Fenix routes each call to `guests/callers.wasm`
//! (API 1.6): another extension by its id, through any number of them;
//! an agent's tool, resource or prompt; the control channel; Fenix's own
//! features (`docs/plans/service-callers.md` §2).

use std::cell::RefCell;
use std::rc::Rc;

use fenix_control::AgentCaller;
use serde_json::{json, Value};

use super::tests::{pump_until, said};
use super::*;

/// The test extension for callers as `id`, built for 1.6, its manifest
/// `extra` after its engine.
pub(super) fn callers(id: &str, extra: &str) -> CodeExtension {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../fenix-wasm/tests/guests");
    let staged = tempfile::tempdir().unwrap().keep();
    std::fs::copy(dir.join("callers.wasm"), staged.join("extension.wasm")).unwrap();
    let text = format!("[extension]\nid = \"{id}\"\nname = \"{id}\"\nversion = \"1.0.0\"\nruntime = \"wasm\"\n\n[engine]\napi = \"1.6\"\nfenix = \">=1.0, <2\"\n\n{extra}");
    let manifest = fenix_extensions::Manifest::parse(&text).unwrap_or_else(|e| panic!("{id}: {e}"));
    CodeExtension { id: id.into(), name: id.into(), dir: staged, manifest, refused: Vec::new(), managed: false }
}

/// `services = [...]`, for `callers`.
pub(super) fn serving(services: &[&str]) -> String {
    format!("[contributes]\nservices = [{}]\n", services.iter().map(|s| format!("{s:?}")).collect::<Vec<_>>().join(", "))
}

/// Asks `server` for `name` as `caller` would, and waits for the answer.
pub(super) fn ask(app: &mut App, server: &str, name: &str, payload: &str, caller: ServiceCaller) -> Result<String, String> {
    let got = Rc::new(RefCell::new(None));
    let kept = got.clone();
    app.code_serve(server, name, payload.into(), caller, Answer::Fenix(Box::new(move |_, answer| *kept.borrow_mut() = Some(answer))));
    pump_until(app, |_| got.borrow().is_some());
    got.take().unwrap()
}

/// What `reply` is sent, while the extensions run.
fn waited(app: &mut App, answer: &std::sync::mpsc::Receiver<crate::ipc::Reply>) -> crate::ipc::Reply {
    let got = RefCell::new(None);
    pump_until(app, |_| {
        if got.borrow().is_none() {
            *got.borrow_mut() = answer.try_recv().ok();
        }
        got.borrow().is_some()
    });
    got.into_inner().unwrap()
}

/// A -> B -> C: each sees the one that called it, never the first.
#[test]
fn a_service_sees_its_immediate_caller_through_a_chain() {
    let mut app = App::with_file(None);
    app.wasm.installed = vec![callers("test.a", &serving(&["a.relay"])), callers("test.b", &serving(&["b.relay"])), callers("test.c", &serving(&["c.who"]))];
    assert_eq!(ask(&mut app, "test.b", "b.relay", "c.who", ServiceCaller::Fenix).as_deref(), Ok("fenix | extension:test.b"));
    assert_eq!(ask(&mut app, "test.a", "a.relay", "b.relay c.who", ServiceCaller::Fenix).as_deref(), Ok("fenix | extension:test.a | extension:test.b"));
    let out = app.extension_output_text();
    assert!(out.contains("called-by in handle: none") && out.contains("called-by in activate: none"), "{out}");
}

/// Two extensions asking one service at once: each answer, given later,
/// is for its own caller.
#[test]
fn two_callers_at_once_each_are_seen() {
    let mut app = App::with_file(None);
    app.wasm.installed = vec![callers("test.a", &serving(&["a.relay"])), callers("test.b", &serving(&["b.relay"])), callers("test.c", &serving(&["c.who_later"]))];
    let got: Rc<RefCell<Vec<Result<String, String>>>> = Rc::default();
    for (server, name) in [("test.a", "a.relay"), ("test.b", "b.relay")] {
        let kept = got.clone();
        app.code_serve(server, name, "c.who_later".into(), ServiceCaller::Control, Answer::Fenix(Box::new(move |_, answer| kept.borrow_mut().push(answer))));
    }
    pump_until(&mut app, |_| got.borrow().len() == 2);
    let mut answers: Vec<String> = got.take().into_iter().map(Result::unwrap).collect();
    answers.sort();
    assert_eq!(answers, ["control | extension:test.a", "control | extension:test.b"]);
}

/// An agent's tool (`fenix/tools.call`, and `fenix/agent.call` as `fenix
/// --mcp` and agent threads call it), resource and prompt.
#[test]
fn an_agents_tool_resource_and_prompt_are_each_their_own_caller() {
    let mut app = App::with_file(None);
    let extra = serving(&["c.who"])
        + "\n[[contributes.agent_tools]]\nname = \"who\"\ndescription = \"Who\"\nservice = \"c.who\"\ndefault = true\n\n[[contributes.agent_resources]]\nname = \"who\"\nuri_template = \"fenix://ext/c/{key}\"\nservice = \"c.who\"\n\n[[contributes.agent_prompts]]\nname = \"c_who\"\nservice = \"c.who\"\n";
    app.wasm.installed = vec![callers("test.c", &extra)];
    let (reply, answer) = std::sync::mpsc::channel();
    app.control_tools_call(json!({ "name": "c_who" }), reply);
    assert_eq!(waited(&mut app, &answer).unwrap(), Value::String("agent-tool".into()));
    let (reply, answer) = std::sync::mpsc::channel();
    app.control_agent("call", AgentCaller { client: "fenix-mcp".into(), thread: None }, json!({ "call": "1", "tool": "c_who", "arguments": {} }), reply);
    let through_mcp = waited(&mut app, &answer).unwrap();
    assert_eq!(through_mcp.get("answer").unwrap_or(&through_mcp), &Value::String("agent-tool".into()), "{through_mcp}");
    let (reply, answer) = std::sync::mpsc::channel();
    app.agent_context("read", AgentCaller { client: "c".into(), thread: None }, json!({ "uri": "fenix://ext/c/x" }), reply);
    let read = waited(&mut app, &answer).unwrap();
    assert_eq!(read["contents"][0]["text"], "agent-resource", "{read}");
    let (reply, answer) = std::sync::mpsc::channel();
    app.agent_context("prompt", AgentCaller { client: "c".into(), thread: None }, json!({ "name": "c_who" }), reply);
    let prompt = waited(&mut app, &answer).unwrap();
    assert_eq!(prompt["messages"][0]["content"]["text"], "agent-prompt", "{prompt}");
}

/// The control channel's methods an extension answers
/// (`fenix/notebook.capture`).
#[test]
fn the_control_channel_is_its_own_caller() {
    let mut app = App::with_file(None);
    app.wasm.installed = vec![callers("test.notebook", &serving(&["notebook.capture"]))];
    let (reply, answer) = std::sync::mpsc::channel();
    let call = fenix_control::Call::NotebookCapture(fenix_control::protocol::CaptureParams { text: "x".into(), target: Default::default() });
    assert!(app.control_by_extension(call, reply).is_ok(), "an extension answers it");
    assert_eq!(waited(&mut app, &answer).unwrap(), json!({ "entry": "control" }));
}

/// Fenix's own features: the hex view (`ccsds.hex`) and a `vnc:` link
/// (`vnc.connect`).
#[test]
fn fenixs_own_features_call_as_fenix() {
    let dir = tempfile::tempdir().unwrap();
    let binary = dir.path().join("a.bin");
    std::fs::write(&binary, [0u8, 159, 146, 150, 0, 1, 2]).unwrap();
    let mut app = App::with_file(None);
    app.wasm.installed = vec![callers("test.ccsds", &serving(&["ccsds.hex"])), callers("test.vnc", &serving(&["vnc.connect"]))];
    app.open_binary(&binary);
    pump_until(&mut app, |a| said(a, "ccsds.hex called by fenix"));
    app.open_vnc_host("ground");
    pump_until(&mut app, |a| said(a, "vnc.connect called by fenix"));
}

/// `[contributes.service_callers]`: a caller it doesn't name is refused
/// before the extension runs -- not started, and once running, not
/// called (its `gated` count stays 0) -- and told who may; one it names
/// gets through.
#[test]
fn the_gate_refuses_an_unlisted_caller_before_the_extension_runs() {
    let mut app = App::with_file(None);
    let gate = serving(&["g.gated", "g.count"]) + "\n[contributes.service_callers]\n\"g.gated\" = [\"test.agenda\", \"control\"]\n";
    app.wasm.installed = vec![callers("test.g", &gate), callers("test.evil", &serving(&["evil.relay"])), callers("test.agenda", &serving(&["agenda.relay"]))];
    // Fenix itself isn't named either: refused at once, nothing started.
    let refused = ask(&mut app, "test.g", "g.gated", "", ServiceCaller::Fenix).unwrap_err();
    assert_eq!(refused, "g.gated: test.g takes it only from test.agenda, control, not from Fenix");
    assert!(!app.wasm.workers.contains_key("test.g") && !app.wasm.status.contains_key("test.g"), "not started for it");
    // Another extension, through its own call: refused, told why.
    let through = ask(&mut app, "test.evil", "evil.relay", "g.gated", ServiceCaller::Fenix).unwrap_err();
    assert_eq!(through, "fenix | g.gated: test.g takes it only from test.agenda, control, not from test.evil");
    // An origin a later API names: never let through.
    assert_eq!(ask(&mut app, "test.g", "g.gated", "", ServiceCaller::Other("remote".into())).unwrap_err(), "g.gated: test.g takes it only from test.agenda, control");
    // An agent: the same.
    assert!(ask(&mut app, "test.g", "g.gated", "", ServiceCaller::AgentTool).unwrap_err().ends_with("not from an agent"));
    // Running now (a service it doesn't gate is open to anyone), and its
    // gated service has never run.
    assert_eq!(ask(&mut app, "test.g", "g.count", "", ServiceCaller::AgentTool).as_deref(), Ok("0"));
    assert!(!said(&app, "gated ran"), "{}", app.extension_output_text());
    // Those it names get through.
    assert_eq!(ask(&mut app, "test.agenda", "agenda.relay", "g.gated", ServiceCaller::Fenix).as_deref(), Ok("fenix | gated, for extension:test.agenda"));
    assert_eq!(ask(&mut app, "test.g", "g.gated", "", ServiceCaller::Control).as_deref(), Ok("gated, for control"));
    assert_eq!(ask(&mut app, "test.g", "g.count", "", ServiceCaller::Fenix).as_deref(), Ok("2"));
}
