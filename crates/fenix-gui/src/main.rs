mod app;
mod commands;
mod completion;
mod dashboard;
mod docker_panel;
mod git_panel;
mod gpu;
mod icon;
mod jira_panel;
mod keymap;
mod popup;
mod rect;
mod tabstops;
mod text;
mod theme;

use winit::event_loop::{ControlFlow, EventLoop};

use app::{App, FenixUserEvent};

fn main() -> anyhow::Result<()> {
    let event_loop = EventLoop::<FenixUserEvent>::with_user_event().build()?;
    event_loop.set_control_flow(ControlFlow::Wait);

    let proxy = event_loop.create_proxy();
    let mut app = App::new(proxy);
    event_loop.run_app(&mut app)?;

    Ok(())
}
