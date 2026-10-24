// No console window on Windows: Fenix is a GUI program. `fenix --console`
// gives it one when you want to read what it prints (see `console`).
#![cfg_attr(not(test), windows_subsystem = "windows")]
// Drawing code passes geometry as plain numbers, the event enum carries
// whole payloads across threads, and state shared with background
// threads is spelled out where it's declared: each of these lints would
// trade that directness for a type alias or a struct used once.
#![allow(clippy::type_complexity, clippy::too_many_arguments, clippy::large_enum_variant)]

#[cfg(feature = "agenda")]
mod agenda_page;
#[cfg(feature = "jira")]
mod jira_page;
#[cfg(feature = "mission")]
mod mib_def;
#[cfg(feature = "mission")]
mod mib_form;
#[cfg(feature = "mission")]
mod mib_page;
#[cfg(feature = "mission")]
mod mib_check;
#[cfg(feature = "mission")]
mod packet_page;
#[cfg(feature = "mission")]
mod hex_page;
#[cfg(feature = "mission")]
mod time_page;
#[cfg(feature = "mission")]
mod stream_page;
#[cfg(feature = "mission")]
mod standards_page;
mod app;
mod cli;
mod clipboard;
mod commands;
mod completion;
mod console;
mod installed;
mod languages;
mod dashboard;
mod extensions_page;
mod page;
mod project_doctor;
mod project_hub;
mod project_settings;
mod project_wizard;
mod diff_view;
#[cfg(feature = "docker")]
mod docker_panel;
#[cfg(feature = "review")]
mod forge_panel;
mod git_panel;
mod git_compare;
mod git_log;
mod git_rebase;
mod git_request;
mod git_status;
#[cfg(feature = "review")]
mod review_inbox;
#[cfg(feature = "review")]
mod review_page;
#[cfg(feature = "review")]
mod review_store;
mod settings_page;
mod snippets_page;
#[cfg(feature = "notebook")]
mod notebook_page;
#[cfg(feature = "notebook")]
mod backlinks_page;
mod reading;
mod reading_page;
#[cfg(feature = "diagrams")]
mod diagram_page;
mod graph_view;
mod gpu;
mod icon;
mod ipc;
mod keymap;
mod launch;
mod dap;
mod lsp;
mod markdown;
mod motion;
mod merge_view;
mod popup;
mod profile;
mod watchdog;
mod reader;
mod reader_sidebar;
mod reader_text;
mod rect;
mod splash;
mod sprite;
mod surface;
mod tabstops;
mod text;
mod theme;
mod tool_status;
mod wrap;

use winit::event_loop::{ControlFlow, EventLoop};

use app::FenixUserEvent;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if console::requested(&args) {
        console::open();
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
    // Every feature's settings are part of the schema before any are read.
    app::feature::register_settings();
    // This thread is the UI thread: from here on, a freeze on it is
    // written to the hang log.
    if let Some(log) = watchdog::log_path() {
        watchdog::start(log);
    }
    let event_loop = EventLoop::<FenixUserEvent>::with_user_event().build()?;
    event_loop.set_control_flow(ControlFlow::Wait);
    let proxy = event_loop.create_proxy();

    let cwd = std::env::current_dir().unwrap_or_default();
    let invocation = match cli::parse(&args, &cwd, |path| path.exists()) {
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

    Ok(())
}
