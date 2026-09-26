//! Startup, in the order that keeps the window alive: the window and
//! its GPU first, the launch splash drawing into it from its own thread,
//! and only then everything slow -- the session, the buffers, the fonts
//! -- with the splash's hairline following along. Once the editor is
//! ready the splash hands the surface back and fades out over the
//! editor's first frames.
//!
//! Until then `Launcher` stands in for `App` with winit, holding on to
//! whatever events arrive early (a second launch handing over files) to
//! pass on once `App` exists.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use winit::application::ApplicationHandler;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoopProxy};
use winit::window::{Window, WindowId};

use crate::app::{App, FenixUserEvent};
use crate::gpu::GpuContext;
use crate::motion::{self, Feature};
use crate::splash::SplashHandle;
use crate::theme;

pub struct Launcher {
    state: State,
}

struct Showing {
    proxy: EventLoopProxy<FenixUserEvent>,
    files: Vec<String>,
    queued: Vec<FenixUserEvent>,
    window: Arc<Window>,
    gpu_context: GpuContext,
    gpu: crate::gpu::GpuState,
    splash: Option<SplashHandle>,
}

enum State {
    Pending { proxy: EventLoopProxy<FenixUserEvent>, files: Vec<String>, queued: Vec<FenixUserEvent> },
    /// The window is up with the splash in it; the slow part waits for
    /// the event loop to have gone round once, so Windows has actually
    /// shown the window before the loop is kept busy.
    Showing(Box<Showing>),
    Running(Box<App>),
    /// Only while `launch` is moving from one to the other.
    Moving,
}

impl Launcher {
    /// `files`: what to open, the first one the way `App::new` opens it.
    pub fn new(proxy: EventLoopProxy<FenixUserEvent>, files: Vec<String>) -> Self {
        Launcher { state: State::Pending { proxy, files, queued: Vec::new() } }
    }

    fn launch(&mut self, event_loop: &ActiveEventLoop) {
        let State::Pending { proxy, files, queued } = std::mem::replace(&mut self.state, State::Moving) else { return };

        // Only what the window needs: where it goes, its colours, and
        // whether there's a splash. `App::new` reads the file properly.
        let config = match fenix_storage::paths::Roots::current() {
            Some(roots) => fenix_config::Config::load_at_or_default(roots.settings_file(), roots.local.join("state")),
            None => fenix_config::Config::empty("settings.toml".into(), "state".into()),
        };
        let look = config.theme.as_deref().and_then(theme::by_name).unwrap_or(&theme::ORBIT_DARK);
        // The splash is how a slow start shows it's alive, so only its
        // own setting hides it; with animations off it just holds still.
        let splash_on = config.motion.splash != Some(false);
        let still = !motion::on(&config, Feature::Splash);

        let mut attrs = Window::default_attributes().with_title("Fenix").with_window_icon(crate::app::fenix_icon()).with_visible(false);
        if config.restore_windows != Some(false) {
            if let Some(first) = config.windows.first() {
                attrs = attrs
                    .with_position(PhysicalPosition::new(first.x, first.y))
                    .with_inner_size(PhysicalSize::new(first.width, first.height))
                    .with_maximized(first.maximized);
            }
        }
        let window = Arc::new(event_loop.create_window(attrs).expect("failed to create window"));
        crate::profile::launch_mark("window");
        let (gpu_context, mut gpu) = pollster::block_on(GpuContext::new(Arc::clone(&window)));
        crate::profile::launch_mark("gpu");

        let splash = if splash_on {
            let fg = [look.fg.r(), look.fg.g(), look.fg.b(), look.fg.a()].map(|c| c as f32 / 255.0);
            gpu.take_surface().and_then(|surface| {
                SplashHandle::start(Arc::clone(&window), Arc::clone(&gpu.device), Arc::clone(&gpu.queue), surface, gpu.config.clone(), look.bg, fg, still).ok()
            })
        } else {
            None
        };
        crate::profile::launch_mark(if splash.is_some() { "splash started" } else { "no splash" });
        if let Some(splash) = &splash {
            splash.wait_first_frame(Duration::from_millis(250));
            crate::profile::launch_mark("splash first frame");
            window.set_visible(true);
            splash.set_progress(0.1);
        }
        self.state = State::Showing(Box::new(Showing { proxy, files, queued, window, gpu_context, gpu, splash }));
        // Round the loop once more before the slow part: see `Showing`.
        event_loop.set_control_flow(winit::event_loop::ControlFlow::Poll);
    }

    /// The slow part, behind the splash: the editor itself.
    fn load(&mut self, event_loop: &ActiveEventLoop) {
        let State::Showing(showing) = std::mem::replace(&mut self.state, State::Moving) else { return };
        let Showing { proxy, files, queued, window, gpu_context, gpu, splash } = *showing;
        let progress = |p: f32| {
            if let Some(splash) = &splash {
                splash.set_progress(p);
            }
        };
        progress(0.15);
        let mut app = Box::new(App::new(proxy, files.first().cloned()));
        for extra in files.iter().skip(1) {
            app.open_startup_file(Path::new(extra));
        }
        crate::profile::launch_mark("App::new");
        progress(0.6);
        let window_id = window.id();
        app.start_in(event_loop, Arc::clone(&window), gpu_context, gpu, &progress);
        for event in queued {
            app.user_event(event_loop, event);
        }
        match splash {
            Some(splash) => app.end_splash(window_id, splash.finish()),
            None => app.end_splash(window_id, None),
        }
        window.set_visible(true);
        window.request_redraw();
        event_loop.set_control_flow(winit::event_loop::ControlFlow::Wait);
        self.state = State::Running(app);
    }
}

impl ApplicationHandler<FenixUserEvent> for Launcher {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        match &mut self.state {
            State::Running(app) => app.resumed(event_loop),
            State::Pending { .. } => self.launch(event_loop),
            State::Showing(_) | State::Moving => {}
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: FenixUserEvent) {
        match &mut self.state {
            State::Running(app) => app.user_event(event_loop, event),
            State::Pending { queued, .. } => queued.push(event),
            State::Showing(showing) => showing.queued.push(event),
            State::Moving => {}
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        if let State::Running(app) = &mut self.state {
            app.window_event(event_loop, id, event);
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        match &mut self.state {
            State::Running(app) => app.about_to_wait(event_loop),
            State::Showing(_) => self.load(event_loop),
            _ => {}
        }
    }

    fn exiting(&mut self, event_loop: &ActiveEventLoop) {
        if let State::Running(app) = &mut self.state {
            app.exiting(event_loop);
        }
    }
}
