mod app;
mod gpu;
mod rect;
mod text;

use winit::event_loop::{ControlFlow, EventLoop};

use app::App;

fn main() -> anyhow::Result<()> {
    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Wait);

    let mut app = App::new();
    event_loop.run_app(&mut app)?;

    Ok(())
}
