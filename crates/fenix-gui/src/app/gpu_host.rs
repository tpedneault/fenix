//! Keeping the windows drawing: the machine waking, and the GPU device
//! going away under Fenix.
//!
//! A device is lost when the driver resets it, and a laptop that has been
//! asleep long enough to power its GPU down is the usual way that comes
//! about. Nothing made from a lost device ever works again: not a surface
//! configured anew, and not the frame after this one. Every wgpu object
//! Fenix holds has to be made again on a new device; what they were made
//! from (the buffers' text, the fonts, the pictures, an extension's
//! framebuffer) is all still here, so nothing the user sees is lost, and
//! terminals, language servers and agents don't notice.
//!
//! `GpuWatch` decides when; this does it. See `watchdog` for how the wake
//! is noticed and where all of this is logged.

use std::sync::Arc;
use std::time::{Duration, Instant};

use super::{motion_host, pages, App};
use crate::gpu::GpuContext;
use crate::rect::RectRenderer;
use crate::surface::SurfacePipeline;

/// How long after waking a window has to have drawn. Generous: a GPU
/// coming back from sleep is slow, and a false alarm costs a moment's
/// rebuild.
const FRAME_AFTER_WAKE: Duration = Duration::from_secs(15);

impl App {
    /// The machine was asleep. Winit doesn't say so; `watchdog` does.
    ///
    /// Every window is asked for a frame, since that's what finds out
    /// whether the GPU came back: an idle window has no other reason to
    /// draw, and a dead device is only noticed by using it. And a frame
    /// is expected, as a device that doesn't work can also fail by never
    /// saying so.
    pub(super) fn machine_woke(&mut self, slept: Duration, now: Instant) {
        crate::watchdog::note(&format!("the machine woke after {} s; checking the windows still draw", slept.as_secs()));
        self.gpu_watch.expect_frame(now + FRAME_AFTER_WAKE, format!("no frame {FRAME_AFTER_WAKE:?} after the machine woke"));
        // Files may have changed under it, through a share or a sync.
        self.next_disk_check = now;
        self.redraw_every_window();
    }

    /// `GpuWatch`'s side of `about_to_wait`: does what's due, and says
    /// when it next wants a look.
    pub(super) fn gpu_tick(&mut self, now: Instant) -> Option<Instant> {
        if let Some(why) = self.gpu_context.as_ref().and_then(GpuContext::lost) {
            self.gpu_watch.want_rebuild(why);
        }
        let hidden = (0..self.frames.len()).filter_map(|f| self.frame_window(f)).all(|window| window.is_minimized() == Some(true));
        let due = self.gpu_watch.tick(now, hidden);
        if due.rebuild {
            let why = self.gpu_watch.why().unwrap_or("").to_string();
            let started = Instant::now();
            match self.rebuild_gpu() {
                Ok(adapter) => {
                    crate::watchdog::note(&format!("set the GPU up again in {} ms, on {adapter} ({why})", started.elapsed().as_millis()));
                    self.gpu_watch.rebuild_tried(now, true);
                }
                Err(failed) => {
                    crate::watchdog::note(&format!("couldn't set the GPU up again ({why}): {failed}"));
                    self.gpu_watch.rebuild_tried(now, false);
                }
            }
            // What's due next depends on how that went.
            return self.gpu_watch.tick(now, hidden).next;
        }
        if due.redraw {
            self.redraw_every_window();
        }
        due.next
    }

    fn redraw_every_window(&self) {
        for frame in 0..self.frames.len() {
            if let Some(window) = self.frame_window(frame) {
                window.request_redraw();
            }
        }
    }

    /// Makes everything on the GPU again, for every frame, on a new
    /// device. On an error nothing that's been kept is any worse: the
    /// caller tries again later.
    fn rebuild_gpu(&mut self) -> Result<String, String> {
        let _profile = crate::profile::Scope::new("gpu rebuild");
        let frames = self.frames.len();
        let windows: Vec<_> = (0..frames).filter_map(|f| Some((f, Arc::clone(self.frame_window(f)?)))).collect();
        let (_, first) = windows.first().ok_or("no window")?;
        // The splash draws on the device from its own thread, and has the
        // surface. It's only there for a moment, at launch.
        if (0..frames).any(|f| self.frame_gpu_mut(f).is_some_and(|gpu| gpu.lent())) {
            return Err("the launch splash is still drawing".into());
        }

        // All of this can fail while the driver is still coming back, and
        // needs the old swapchains no more than it needs anything else.
        let context = pollster::block_on(GpuContext::reopen(Arc::clone(first)))?;

        // A window takes one swapchain at a time, so the old ones go
        // before the new ones are made. `GpuWatch::rebuild_pending` stops
        // `redraw` from noticing they're gone.
        for frame in 0..frames {
            if let Some(gpu) = self.frame_gpu_mut(frame) {
                gpu.surface = None;
            }
        }
        let mut states = Vec::with_capacity(windows.len());
        for (frame, window) in &windows {
            states.push((*frame, Arc::clone(window), context.try_attach(Arc::clone(window))?));
        }

        // Nothing past here fails.
        let (_, _, first_state) = states.first().ok_or("no window")?;
        if let Some(fonts) = &self.fonts {
            fonts.borrow_mut().rebuild_gpu(first_state);
        }
        self.surfaces = Some(SurfacePipeline::new(first_state));

        let focused = self.active_frame;
        for (frame, window, state) in states {
            self.activate_frame(frame);
            if let Some(text) = &mut self.text {
                text.rebuild_gpu(&state);
                text.resize(state.size.width as f32, state.size.height as f32);
            }
            self.bg_rect = Some(RectRenderer::new(&state));
            self.popup_rect = Some(RectRenderer::new(&state));
            self.caret_rect = Some(RectRenderer::new(&state));
            self.motion_gpu = Some(motion_host::MotionGpu::new(&state));
            self.gpu = Some(state);
            window.request_redraw();
        }
        self.activate_frame(focused);

        // Everything uploaded to the old device, to go again. The pixels
        // themselves are all still held by whoever owns them.
        self.splash_fade = None;
        if let Some(logo) = &mut self.home_logo {
            logo.texture = None;
        }
        for picture in self.page_images.textures.values_mut() {
            picture.texture = None;
        }
        for state in self.pages.values_mut() {
            if let pages::PageModel::Code(page) = &mut state.model {
                if let Some(surface) = &mut page.surface {
                    surface.texture = None;
                    if let Some((_, picture)) = &mut surface.cursor {
                        *picture = None;
                    }
                }
            }
        }

        let info = context.adapter_info();
        let old = self.gpu_context.replace(context);
        // The last of the old device, which is made to wait for a GPU that
        // may be in no state to answer: not on this thread, then. If no
        // thread can be started it goes here after all.
        let _ = std::thread::Builder::new().name("fenix-gpu-close".into()).spawn(move || drop(old));
        Ok(format!("{} via {:?}", info.name, info.backend))
    }

    /// `frame`'s swapchain, parked or live.
    fn frame_gpu_mut(&mut self, frame: usize) -> Option<&mut crate::gpu::GpuState> {
        match self.frames.get_mut(frame)? {
            Some(parked) => parked.gpu.as_mut(),
            None => self.gpu.as_mut(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rebuild_with_no_window_says_so() {
        let mut app = App::with_file(None);
        assert!(app.rebuild_gpu().is_err());
        assert!(app.gpu_tick(Instant::now()).is_none(), "nothing wrong, nothing to do");
    }

    #[test]
    fn a_wake_asks_for_a_frame_and_a_missing_one_means_a_rebuild() {
        let mut app = App::with_file(None);
        let now = Instant::now();
        app.machine_woke(Duration::from_secs(3600), now);
        assert_eq!(app.next_disk_check, now);
        assert_eq!(app.gpu_watch.tick(now, false).next, Some(now + FRAME_AFTER_WAKE));
        app.gpu_watch.tick(now + FRAME_AFTER_WAKE, false);
        assert!(app.gpu_watch.rebuild_pending());
        app.gpu_watch.presented();
        assert!(!app.gpu_watch.rebuild_pending());
    }
}

/// A real `App` in a hidden window on the real GPU, through a real event
/// loop. Run by hand, each in a process of its own (one event loop per
/// process), as they need a GPU and a window, though it's hidden and never
/// takes focus:
///
/// ```text
/// $env:FENIX_LIVE_GPU=1; cargo test -p fenix-gui live_ -- --nocapture --test-threads=1
/// ```
#[cfg(all(test, windows))]
mod live {
    use super::*;
    use crate::app::FenixUserEvent;
    use winit::application::ApplicationHandler;
    use winit::event::WindowEvent;
    use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
    use winit::platform::windows::EventLoopBuilderExtWindows;
    use winit::window::{Window, WindowId};

    /// Drives a real `App` in a hidden window on the real GPU, through a
    /// real event loop. The steps are `step`'s; the test ends when it
    /// returns true.
    struct Live {
        app: App,
        window: Option<Arc<Window>>,
        step: Box<dyn FnMut(&mut App, &ActiveEventLoop) -> bool>,
        give_up: Instant,
        finished: bool,
    }

    impl ApplicationHandler<FenixUserEvent> for Live {
        fn resumed(&mut self, event_loop: &ActiveEventLoop) {
            if self.window.is_some() {
                return;
            }
            let attrs = Window::default_attributes().with_visible(false).with_inner_size(winit::dpi::PhysicalSize::new(640, 480));
            let window = Arc::new(event_loop.create_window(attrs).expect("a hidden window"));
            let (context, gpu) = pollster::block_on(GpuContext::new(Arc::clone(&window)));
            self.window = Some(Arc::clone(&window));
            self.app.start_in(event_loop, window, context, gpu, &|_| {});
        }

        fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
            self.app.window_event(event_loop, id, event);
        }

        fn user_event(&mut self, event_loop: &ActiveEventLoop, event: FenixUserEvent) {
            self.app.user_event(event_loop, event);
        }

        fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
            self.app.about_to_wait(event_loop);
            // A hidden window gets no `WM_PAINT`, so `request_redraw` never
            // comes back: this stands in for the OS.
            self.app.redraw();
            // Whatever `App` planned, keep this moving.
            event_loop.set_control_flow(ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(10)));
            if (self.step)(&mut self.app, event_loop) {
                self.finished = true;
                event_loop.exit();
            } else if Instant::now() > self.give_up {
                event_loop.exit();
            }
        }
    }

    fn live(mut step: impl FnMut(&mut App, &ActiveEventLoop) -> bool + 'static) -> bool {
        // `cargo test` runs this off the main thread, which winit allows
        // on Windows only when asked.
        let event_loop = EventLoop::<FenixUserEvent>::with_user_event().with_any_thread(true).build().expect("an event loop");
        let mut live = Live {
            app: App::with_file(None),
            window: None,
            step: Box::new(move |app, event_loop| step(app, event_loop)),
            give_up: Instant::now() + Duration::from_secs(60),
            finished: false,
        };
        event_loop.run_app(&mut live).expect("the event loop ran");
        live.finished
    }

    /// What the window shows now, as its pixels.
    fn picture(app: &mut App) -> Vec<u32> {
        app.motion.capture_frame = true;
        app.redraw();
        app.motion.capture_frame = false;
        let (gpu, motion) = (app.gpu.as_ref().expect("a window"), app.motion_gpu.as_ref().expect("a window"));
        let (texture, _) = motion.snapshot.as_ref().expect("the frame was copied");
        let (width, height) = (texture.width(), texture.height());
        let row = (width * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("live-test-readback"),
            size: u64::from(row * height),
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut encoder = gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        encoder.copy_texture_to_buffer(
            texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo { buffer: &buffer, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: None } },
            wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        );
        gpu.queue.submit([encoder.finish()]);
        buffer.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        gpu.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        let bytes = buffer.slice(..).get_mapped_range().expect("mapped").to_vec();
        bytes.chunks(row as usize).flat_map(|line| line[..width as usize * 4].chunks(4).map(|p| u32::from_le_bytes([p[0], p[1], p[2], p[3]])).collect::<Vec<_>>()).collect()
    }

    fn colours(picture: &[u32]) -> usize {
        picture.iter().collect::<std::collections::HashSet<_>>().len()
    }

    fn device_of(app: &App) -> Arc<wgpu::Device> {
        Arc::clone(&app.gpu.as_ref().expect("a window").device)
    }

    fn wanted() -> bool {
        std::env::var_os("FENIX_LIVE_GPU").is_some()
    }

    /// Why the window froze, on the real GPU: after the loss, a surface
    /// answers `Validation` to every request, and configuring it again
    /// changes nothing. Fenix took that status for a passing problem,
    /// printed it to a console nobody has, and never asked for a frame
    /// again; the window stayed as it was until Fenix was killed.
    #[test]
    fn live_a_lost_device_never_gives_a_surface_a_frame_again() {
        if !wanted() {
            return;
        }
        let finished = live(|app, _| {
            if app.frames_presented == 0 {
                return false;
            }
            device_of(app).destroy();
            let gpu = app.gpu.as_mut().expect("a window");
            for attempt in 0..5 {
                let size = gpu.size;
                gpu.reconfigure(size);
                let status = gpu.surface.as_ref().expect("a surface").get_current_texture();
                println!("attempt {attempt} after the loss: {}", std::any::type_name_of_val(&status));
                assert!(matches!(status, wgpu::CurrentSurfaceTexture::Validation), "attempt {attempt}");
            }
            true
        });
        assert!(finished, "never drew a first frame");
    }

    /// The freeze, and getting out of it: a device lost while the machine
    /// slept, then the window drawing the same picture on a new one, with
    /// nothing done but the wake. Reading the pixels is the point: a rebuild
    /// that presented blank frames would look fine by any other measure.
    #[test]
    fn live_gpu_loss_recovers_and_draws_again() {
        if !wanted() {
            return;
        }
        enum Stage {
            FirstFrames,
            Lost { before: Vec<u32>, at: Instant, old: Arc<wgpu::Device> },
            Done,
        }
        let mut stage = Stage::FirstFrames;
        let finished = live(move |app, event_loop| match &stage {
            Stage::FirstFrames => {
                // Two windows: the frames parked behind the live one have
                // to come along, and that's the part easily got wrong.
                if app.frames.len() < 2 {
                    app.new_frame(event_loop);
                    app.focus_frame(0);
                }
                if app.frames_presented > 2 {
                    let before = picture(app);
                    println!("before: {} pixels, {} colours", before.len(), colours(&before));
                    assert!(colours(&before) > 16, "text on the screen to compare with");
                    let old = device_of(app);
                    old.destroy();
                    // Nothing else moves an idle window: this is all the
                    // app is told.
                    app.machine_woke(Duration::from_secs(8 * 3600), Instant::now());
                    stage = Stage::Lost { before, at: Instant::now(), old };
                }
                false
            }
            Stage::Lost { before, at, old } => {
                if Arc::ptr_eq(old, &device_of(app)) || app.gpu_watch.rebuild_pending() {
                    return false;
                }
                let frames = app.frames_presented;
                let after = picture(app);
                if app.frames_presented == frames {
                    return false;
                }
                let differing = before.iter().zip(&after).filter(|(a, b)| a != b).count();
                println!("after: drawing {} ms on, {} colours, {differing} pixels of {} differ", at.elapsed().as_millis(), colours(&after), after.len());
                assert_eq!(before.len(), after.len());
                assert!(differing * 100 < after.len(), "not the picture it had");
                // Every window on the new device, and each with a surface.
                let device = device_of(app);
                app.focus_frame(1);
                assert_eq!(app.active_frame, 1);
                assert!(Arc::ptr_eq(&device, &device_of(app)), "the second window was left on the old device");
                let second = picture(app);
                assert!(colours(&second) > 16, "the second window draws too");
                app.focus_frame(0);
                println!("second window: {} colours", colours(&second));
                stage = Stage::Done;
                false
            }
            Stage::Done => {
                app.frames_presented > 8 && !app.gpu_watch.rebuild_pending()
            }
        });
        assert!(finished, "the window never drew again");
    }

    /// What the loop costs with nothing to do. Idle by the app's own
    /// timing, so this one lets `App` choose its wait. Prints what it saw;
    /// the assert is only that it isn't spinning.
    #[test]
    fn live_idle_wakes() {
        if !wanted() {
            return;
        }
        struct Idle(Live, u64, Option<Instant>, Instant);
        impl ApplicationHandler<FenixUserEvent> for Idle {
            fn resumed(&mut self, event_loop: &ActiveEventLoop) {
                self.0.resumed(event_loop);
            }

            fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
                self.0.window_event(event_loop, id, event);
            }

            fn user_event(&mut self, event_loop: &ActiveEventLoop, event: FenixUserEvent) {
                self.0.user_event(event_loop, event);
            }

            fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
                self.0.app.about_to_wait(event_loop);
                self.1 += 1;
                // Give the first frames and the caret's blinking time to
                // settle, then count.
                if self.2.is_none() && self.3.elapsed() > Duration::from_secs(40) {
                    self.2 = Some(Instant::now());
                    self.1 = 0;
                }
                if self.2.is_some_and(|since| since.elapsed() > Duration::from_secs(30)) {
                    event_loop.exit();
                }
                if self.3.elapsed() > Duration::from_secs(90) {
                    event_loop.exit();
                }
            }
        }
        let event_loop = EventLoop::<FenixUserEvent>::with_user_event().with_any_thread(true).build().expect("an event loop");
        let inner = Live { app: App::with_file(None), window: None, step: Box::new(|_, _| false), give_up: Instant::now(), finished: false };
        let mut idle = Idle(inner, 0, None, Instant::now());
        event_loop.run_app(&mut idle).expect("the event loop ran");
        let since = idle.2.expect("it got that far").elapsed().as_secs_f64();
        println!("{} wakes in {since:.0} s idle: {:.1} a second, {} frames all told", idle.1, idle.1 as f64 / since, idle.0.app.frames_presented);
        assert!((idle.1 as f64 / since) < 30.0, "an idle window is spinning");
    }
}
