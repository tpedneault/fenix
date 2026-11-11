// No console window on Windows: Fenix is a GUI program. `fenix --console`
// gives it one when you want to read what it prints (see `console`).
#![cfg_attr(not(test), windows_subsystem = "windows")]
// Drawing code passes geometry as plain numbers, the event enum carries
// whole payloads across threads, and state shared with background
// threads is spelled out where it's declared: each of these lints would
// trade that directness for a type alias or a struct used once.
#![allow(clippy::type_complexity, clippy::too_many_arguments, clippy::large_enum_variant)]

mod a11y;
mod agent_page;
mod bidi;
mod agent_review;
mod agent_usage_page;
mod agent_tools;
mod agent_work;
mod mcp_http;
mod app;
mod cli;
mod clipboard;
mod commands;
mod completion;
mod console;
mod installed;
mod languages;
mod dashboard;
mod decorations;
mod code_page;
mod extension_output;
mod extensions_page;
mod explorer_page;
mod preview_page;
mod search_page;
mod page;
mod project_doctor;
mod project_hub;
mod project_settings;
mod project_wizard;
mod diff_view;
mod git_panel;
mod git_compare;
mod git_log;
mod git_rebase;
mod git_status;
mod settings_page;
mod snippets_page;
mod sound;
mod md;
mod reading;
mod reading_page;
mod graph_view;
mod gpu;
mod icon;
mod ipc;
mod keymap;
mod launch;
mod dap;
mod processes;
mod formatter;
mod lsp;
mod markdown;
mod mcp;
mod motion;
mod mouse;
mod folds;
mod notify;
mod notifications_page;
mod welcome_page;
mod merge_view;
mod popup;
mod profile;
mod watchdog;
mod file_link;
mod rect;
mod splash;
mod model3d;
mod sprite;
mod surface;
mod screenshot;
mod tabstops;
mod text;
mod theme;
mod tool_status;
mod tutorial;
mod wrap;

use winit::event_loop::{ControlFlow, EventLoop};

use app::FenixUserEvent;

fn main() -> anyhow::Result<()> {
    // Two runs that aren't Fenix the editor, and whose stdio is spoken
    // for, so they come before anything that prints or opens a window.
    // `ssh`, started by a Fenix connecting to a remote machine, runs this
    // program to put a question (a password, a new host's key) to that
    // Fenix; and a remote project's programs (`git`, a language server)
    // are run as `fenix --remote-exec`, a stand-in that passes them
    // through the connection that Fenix has open.
    if let Some(code) = fenix_ssh::askpass::answer_if_asked() {
        std::process::exit(code);
    }
    if let Some(code) = fenix_ssh::bridge::stand_in_if_asked() {
        std::process::exit(code);
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    // `--mcp`: an MCP server on stdio for a coding agent, talking to the
    // Fenix that's running. Before anything that opens a console or a
    // window: its stdio is the protocol.
    // `fenix mcp token`: what an MCP client over HTTP presents.
    if args.first().map(String::as_str) == Some("mcp") && args.get(1).map(String::as_str) == Some("token") {
        match mcp_http::token() {
            Ok(token) => {
                println!("{token}");
                std::process::exit(0);
            }
            Err(why) => {
                eprintln!("fenix: {why}");
                std::process::exit(1);
            }
        }
    }
    if mcp::requested(&args) {
        if let Some(address) = args.iter().position(|a| a == "--http").and_then(|i| args.get(i + 1)) {
            if let Err(why) = mcp_http::serve(address) {
                eprintln!("fenix: {why}");
                std::process::exit(1);
            }
            std::process::exit(0);
        }
        mcp::run(&args);
    }
    // `--help` and `--version` print and exit, on Windows too (a console
    // is attached to say it in), never reaching a running Fenix.
    if let Some(said) = cli::info_request(&args) {
        console::open();
        println!("{said}");
        std::process::exit(0);
    }
    if console::requested(&args) {
        console::open();
    }
    for flag in cli::unknown_flags(&args) {
        eprintln!("fenix: no option {flag}, ignored -- fenix --help lists them");
    }

    // `--notify`: a notification for the running Fenix, and exit.
    if let Some(request) = cli::notify_request(&args) {
        console::open();
        let params = match request {
            Ok(params) => params,
            Err(why) => {
                eprintln!("fenix: {why}");
                std::process::exit(2);
            }
        };
        let sent = fenix_control::Client::connect(&fenix_control::Endpoint::current())
            .map_err(|e| e.to_string())
            .and_then(|c| c.ok_or_else(|| "Fenix isn't running".to_string()))
            .and_then(|mut c| {
                c.hello("fenix-cli", env!("CARGO_PKG_VERSION")).map_err(|e| e.to_string())?;
                c.request::<serde_json::Value>(fenix_control::protocol::method::NOTIFY, serde_json::to_value(&params).expect("plain data")).map_err(|e| e.to_string())
            });
        match sent {
            Ok(_) => std::process::exit(0),
            Err(why) => {
                eprintln!("fenix: {why}");
                std::process::exit(1);
            }
        }
    }

    // `--install-extension` and friends change what's installed and
    // exit: no window, no hand-off to a running Fenix.
    match installed::request(&args) {
        Ok(Some(request)) => {
            console::open();
            // `--machine`: the machine's store, for everyone here.
            let machine = installed::wants_machine(&args);
            let store = if machine { fenix_extensions::Store::managed() } else { fenix_extensions::Store::current() };
            let Some(store) = store else {
                eprintln!("fenix: no folder to keep extensions in");
                std::process::exit(1);
            };
            let managed = if machine { None } else { fenix_extensions::Store::managed() };
            match installed::run(&request, &store, managed.as_ref()) {
                Ok(said) => {
                    println!("{said}");
                    std::process::exit(0);
                }
                Err(e) => {
                    eprintln!("fenix: {e}");
                    std::process::exit(1);
                }
            }
        }
        Ok(None) => {}
        Err(e) => {
            console::open();
            eprintln!("fenix: {e}");
            std::process::exit(2);
        }
    }

    // Safe mode is settled before anything reads what's switched on.
    installed::set_safe_mode(installed::wants_safe_mode(&args));
    // The language files' file types, before a file is opened.
    languages::init();

    profile::launch_mark("main");
    // The code extensions' settings are part of the schema before any are
    // read.
    installed::register_code_settings();
    // This thread is the UI thread: from here on, a freeze on it is
    // written to the hang log.
    if let Some(log) = watchdog::log_path() {
        watchdog::start(log);
    }
    let event_loop = EventLoop::<FenixUserEvent>::with_user_event().build()?;
    event_loop.set_control_flow(ControlFlow::Wait);
    let proxy = event_loop.create_proxy();
    // Winit doesn't say when the machine wakes; the watchdog notices.
    let waking = proxy.clone();
    watchdog::on_wake(move |slept| {
        let _ = waking.send_event(FenixUserEvent::Woke(slept));
    });

    let cwd = std::env::current_dir().unwrap_or_default();
    let invocation = match cli::parse(&args, &cwd, |path| path.exists()).map(|i| cli::folders_as_project(i, std::path::Path::is_dir)) {
        Ok(invocation) => invocation,
        Err(message) => {
            eprintln!("fenix: {message}");
            std::process::exit(2);
        }
    };

    // Single-instance: a second `fenix` launch (Explorer's "Open With"
    // on a double-clicked file, `fenix x.rs:12`, or just relaunching
    // `fenix.exe`) hands what it was asked for to the Fenix already
    // running instead of opening a second window -- see `ipc`.
    // `FENIX_STANDALONE=1` opts out, for a test instance (usually with
    // its own `FENIX_HOME`, which has its own endpoint anyway) beside a
    // Fenix someone is using.
    // Safe mode opens a window of its own: a Fenix already running has
    // its extensions, and handing it the files would keep them.
    // Started by `SPC q r`: the Fenix it replaces lets go of the
    // endpoint as it finishes quitting.
    if std::env::var_os(ipc::RESTARTED_ENV).is_some() {
        std::env::remove_var(ipc::RESTARTED_ENV);
        ipc::wait_until_free(std::time::Duration::from_secs(15));
    }
    let standalone = std::env::var_os("FENIX_STANDALONE").is_some_and(|v| !v.is_empty() && v != "0") || installed::safe_mode();
    let role = if standalone { ipc::Role::Standalone } else { ipc::negotiate(&invocation) };
    match role {
        ipc::Role::HandedOff => return Ok(()),
        ipc::Role::Server(listener) => ipc::spawn_accept_loop(listener, proxy.clone()),
        ipc::Role::Standalone => {}
    }

    // This launch turned out to be the editor, so there's nobody to
    // hand `--new-window` to and no window to add one alongside -- the
    // window this launch is about to open *is* the new one.
    // The window comes up first, with the launch splash in it, and the
    // editor loads behind it -- see `launch`.
    let mut launcher = launch::Launcher::new(proxy, invocation);
    event_loop.run_app(&mut launcher)?;

    // `SPC q r`: the session is saved; the next Fenix restores it. It's
    // started without this launch's files (the session has them) but
    // with its safe mode, console and extensions' options left out too:
    // a restart is to use what's installed.
    if launcher.restart_requested() {
        let keep: Vec<&String> = args.iter().filter(|a| a.as_str() == "--console").collect();
        let exe = std::env::current_exe()?;
        if let Err(e) = std::process::Command::new(exe).args(keep).env(ipc::RESTARTED_ENV, "1").spawn() {
            eprintln!("fenix: couldn't start again ({e}); start Fenix yourself");
        }
    }

    Ok(())
}
