mod agenda_page;
mod jira_page;
mod app;
mod commands;
mod completion;
mod dashboard;
mod page;
mod project_doctor;
mod project_hub;
mod project_settings;
mod project_wizard;
mod diff_view;
mod docker_panel;
mod forge_panel;
mod git_panel;
mod git_log;
mod git_rebase;
mod git_request;
mod git_status;
mod review_inbox;
mod review_page;
mod review_store;
mod settings_page;
mod snippets_page;
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
mod pdf_outline;
mod pdf_search;
mod pdf_texture;
mod popup;
mod profile;
mod rect;
mod splash;
mod sprite;
mod tabstops;
mod text;
mod theme;
mod tool_status;
mod vnc_texture;
mod wrap;

use winit::event_loop::{ControlFlow, EventLoop};

use app::FenixUserEvent;

fn main() -> anyhow::Result<()> {
    profile::launch_mark("main");
    let event_loop = EventLoop::<FenixUserEvent>::with_user_event().build()?;
    event_loop.set_control_flow(ControlFlow::Wait);
    let proxy = event_loop.create_proxy();

    // Single-instance: a second `fenix` launch (Explorer's "Open With"
    // on a double-clicked file, or just relaunching `fenix.exe`) hands
    // its file arguments to whichever instance is already running
    // instead of opening a second window -- see `ipc`'s own doc
    // comment for the mechanism and its one disclosed tradeoff.
    let args: Vec<String> = std::env::args().skip(1).collect();
    match ipc::negotiate(&args) {
        ipc::Role::HandedOff => return Ok(()),
        ipc::Role::Server(listener) => ipc::spawn_accept_loop(listener, proxy.clone()),
        ipc::Role::Standalone => {}
    }

    // This launch turned out to be the only one running, so there's
    // nobody to hand `--new-window` to and no window to add one
    // alongside -- the window this launch is about to open *is* the
    // new one. The flag is dropped here along with any other flag, so
    // it can't be mistaken for a file to open.
    let files: Vec<&String> = args.iter().filter(|arg| !arg.starts_with('-')).collect();

    // The window comes up first, with the launch splash in it, and the
    // editor loads behind it -- see `launch`.
    let mut launcher = launch::Launcher::new(proxy, files.iter().map(|f| f.to_string()).collect());
    event_loop.run_app(&mut launcher)?;

    Ok(())
}
