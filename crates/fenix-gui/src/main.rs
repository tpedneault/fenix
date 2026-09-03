mod app;
mod commands;
mod completion;
mod dashboard;
mod diff_view;
mod docker_panel;
mod git_panel;
mod gpu;
mod icon;
mod ipc;
mod jira_panel;
mod keymap;
mod dap;
mod lsp;
mod markdown;
mod pdf_outline;
mod pdf_search;
mod pdf_texture;
mod popup;
mod rect;
mod tabstops;
mod text;
mod theme;
mod tool_status;
mod vnc_texture;
mod wrap;

use std::path::Path;

use winit::event_loop::{ControlFlow, EventLoop};

use app::{App, FenixUserEvent};

fn main() -> anyhow::Result<()> {
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

    // `App::new` seeds its initial buffer from the first file itself;
    // anything beyond that (e.g. Explorer's "Open With" on multiple
    // selected files) opens as extra buffers right after startup, same
    // landing spot an IPC hand-off's files use
    // (`App::apply_open_files`).
    let mut app = App::new(proxy, files.first().map(|arg| arg.to_string()));
    for extra in files.iter().skip(1) {
        app.open_startup_file(Path::new(extra));
    }
    event_loop.run_app(&mut app)?;

    Ok(())
}
