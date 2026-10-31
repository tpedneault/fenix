//! A conversation with an ACP agent in the terminal, through the same
//! hub Fenix uses: to check an agent works before trying it in Fenix.
//!
//! ```text
//! cargo run -p fenix-acp --example chat -- claude-agent-acp
//! cargo run -p fenix-acp --example chat -- devin acp
//! cargo run -p fenix-acp --example chat -- npx -y @agentclientprotocol/codex-acp
//! cargo run -p fenix-acp --example chat -- target/debug/fake-agent --profile claude
//! ```
//!
//! Type a prompt and Enter. `/cancel` cancels the turn, `/mode <id>`
//! switches mode, `/quit` leaves. Permission requests are asked as a
//! numbered list. File reads and writes the agent asks of the client are
//! done on disk, in the current directory.

use fenix_acp::client::{AcpClient, AcpEvent, Launch, RpcError};
use fenix_acp::hub::{AgentSpec, ConnId, Effect, Hub, ThreadId};
use fenix_acp::protocol::{code, ClientRequest, ContentBlock};
use fenix_acp::thread::{Entry, PermissionState};
use serde_json::json;
use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::sync::mpsc;
use std::time::{Duration, Instant};

enum Input {
    Agent(ConnId, u32, AcpEvent),
    Line(String),
}

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(program) = args.next() else {
        eprintln!("usage: chat <agent program> [args...]");
        std::process::exit(2);
    };
    let cwd = std::env::current_dir().expect("a current directory");
    let mut hub = Hub::new("chat", env!("CARGO_PKG_VERSION"));
    hub.set_agents(vec![AgentSpec { id: "agent".into(), name: program.clone(), program, args: args.collect(), env: vec![] }]);
    let (tx, rx) = mpsc::channel::<Input>();
    let lines = tx.clone();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines().map_while(Result::ok) {
            if lines.send(Input::Line(line)).is_err() {
                break;
            }
        }
    });

    let thread = hub.new_thread("agent", &cwd);
    let mut clients: HashMap<ConnId, AcpClient> = HashMap::new();
    let mut shown = 0usize;
    let mut asked: Option<String> = None;
    let fx = hub.focus(&thread);
    run(&mut hub, fx, &mut clients, &tx, &thread);
    println!("(type a prompt; /cancel, /mode <id>, /quit)");
    loop {
        let input = match rx.recv_timeout(Duration::from_millis(250)) {
            Ok(input) => Some(input),
            Err(mpsc::RecvTimeoutError::Timeout) => None,
            Err(_) => break,
        };
        let fx = match input {
            Some(Input::Agent(conn, generation, event)) => hub.on_event(conn, generation, event, Instant::now()),
            Some(Input::Line(line)) => {
                let line = line.trim().to_string();
                if let Some(key) = asked.take() {
                    let view = hub.view(&thread).unwrap();
                    let card = view.thread.entries.iter().find_map(|e| match e {
                        Entry::Permission(c) if c.key == key => Some(c.clone()),
                        _ => None,
                    });
                    let option = card.and_then(|c| line.parse::<usize>().ok().and_then(|n| c.options.get(n.wrapping_sub(1)).map(|o| o.id.clone())));
                    hub.answer_permission(&thread, &key, option.as_deref())
                } else if line == "/quit" {
                    break;
                } else if line == "/cancel" {
                    hub.cancel(&thread, Instant::now())
                } else if let Some(mode) = line.strip_prefix("/mode ") {
                    hub.set_mode(&thread, mode.trim())
                } else if line.is_empty() {
                    Vec::new()
                } else {
                    hub.prompt(&thread, vec![ContentBlock::text(line)])
                }
            }
            None => hub.tick(Instant::now()),
        };
        run(&mut hub, fx, &mut clients, &tx, &thread);
        let view = hub.view(&thread).unwrap();
        for entry in &view.thread.entries[shown.min(view.thread.entries.len())..] {
            print_entry(entry);
        }
        shown = view.thread.entries.len();
        if asked.is_none() {
            if let Some(card) = view.thread.entries.iter().find_map(|e| match e {
                Entry::Permission(c) if c.state == PermissionState::Pending => Some(c),
                _ => None,
            }) {
                println!("? {} wants: {}", view.thread.display_title(), card.tool_call.title.clone().unwrap_or_default());
                for (i, o) in card.options.iter().enumerate() {
                    println!("  {}. {}", i + 1, o.name);
                }
                asked = Some(card.key.clone());
            }
        }
        std::io::stdout().flush().ok();
    }
    let fx = hub.shutdown();
    run(&mut hub, fx, &mut clients, &tx, &thread);
}

fn run(hub: &mut Hub, fx: Vec<Effect>, clients: &mut HashMap<ConnId, AcpClient>, tx: &mpsc::Sender<Input>, _thread: &ThreadId) {
    let mut queue: std::collections::VecDeque<Effect> = fx.into();
    while let Some(effect) = queue.pop_front() {
        let more = match effect {
            Effect::Spawn { conn, generation, agent, root } => {
                let sender = tx.clone();
                let launch = Launch { program: agent.program.clone(), args: agent.args.clone(), env: agent.env.clone(), cwd: root };
                let result = AcpClient::start(&launch, move |event| sender.send(Input::Agent(conn, generation, event)).is_ok()).map(|client| {
                    clients.insert(conn, client);
                });
                hub.on_spawned(conn, generation, result.map_err(|e| e.to_string()), Instant::now())
            }
            Effect::Stop { conn } => {
                clients.remove(&conn);
                Vec::new()
            }
            Effect::Request { conn, id, method, params } => {
                if let Some(c) = clients.get(&conn) {
                    let _ = c.request(id, method, params);
                }
                Vec::new()
            }
            Effect::Notify { conn, method, params } => {
                if let Some(c) = clients.get(&conn) {
                    let _ = c.notify(method, params);
                }
                Vec::new()
            }
            Effect::Respond { conn, id, result } => {
                if let Some(c) = clients.get(&conn) {
                    let _ = c.respond(id, result);
                }
                Vec::new()
            }
            Effect::Serve { conn, id, thread: t, request } => {
                let result = match request {
                    ClientRequest::ReadTextFile { path, .. } => std::fs::read_to_string(&path).map(|content| json!({ "content": content })).map_err(|e| RpcError::new(code::INTERNAL_ERROR, e.to_string())),
                    ClientRequest::WriteTextFile { path, content, .. } => std::fs::write(&path, content).map(|_| json!(null)).map_err(|e| RpcError::new(code::INTERNAL_ERROR, e.to_string())),
                    _ => Err(RpcError::new(code::METHOD_NOT_FOUND, "this example runs no terminals")),
                };
                hub.on_served(conn, &t, id, result)
            }
            Effect::TerminalLogin { args, .. } => {
                println!("! log in by running the agent's program with: {}", args.join(" "));
                Vec::new()
            }
            Effect::Announced { paths, .. } => {
                println!("  (about to change: {})", paths.join(", "));
                Vec::new()
            }
            _ => Vec::new(),
        };
        queue.extend(more);
    }
}

fn print_entry(entry: &Entry) {
    match entry {
        Entry::User { blocks, queued } => println!("> {}{}", blocks.iter().map(ContentBlock::as_text).collect::<Vec<_>>().join(" "), if *queued { " (queued)" } else { "" }),
        Entry::Agent { text, .. } => println!("{text}"),
        Entry::Thought { text, .. } => println!("  (thinking) {}", text.lines().next().unwrap_or("")),
        Entry::Tool(call) => println!("  [{:?}] {} — {:?}", call.kind, call.title, call.status),
        Entry::Permission(_) => {}
        Entry::Notice(n) => println!("  ({:?}) {}", n.kind, n.text),
    }
}
