use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use winit::dpi::PhysicalSize;
use winit::window::Window;

/// Everything a wgpu setup owns that *isn't* tied to one particular
/// window: the instance, the adapter, and the logical device/queue pair.
/// Created once, then shared by every frame (OS window) the app opens --
/// `attach` hands each new one its own swapchain on this same device.
///
/// One device rather than one per window on purpose. A `wgpu` resource
/// (texture, buffer, pipeline, bind group) belongs to the device that
/// created it and can't be used with another, so per-window devices
/// would mean a PDF page's texture, a VNC framebuffer's texture, and
/// every pipeline and shader existing once *per window* -- and a pane's
/// content becoming unusable the moment it moved between windows.
/// Several surfaces on one device is the shape wgpu is built for.
pub struct GpuContext {
    instance: wgpu::Instance,
    adapter: wgpu::Adapter,
    pub device: Arc<wgpu::Device>,
    pub queue: Arc<wgpu::Queue>,
    /// The surface format every frame is configured with -- resolved
    /// once, from the first window's capabilities (see `new`), so every
    /// frame's swapchain agrees and one set of render pipelines works
    /// for all of them.
    pub format: wgpu::TextureFormat,
    /// Why the device went, once it has: the driver reset it (a machine
    /// back from a long sleep is the usual reason), and nothing made from
    /// it will ever draw again. Set by wgpu's callback, on whatever thread
    /// noticed; read by `App::gpu_tick`.
    lost: Arc<Mutex<Option<String>>>,
}

/// One frame's own swapchain: the surface, its configuration, and its
/// current size. `device`/`queue` are `Arc` clones of the shared
/// `GpuContext`'s, kept as fields here (rather than reached through a
/// context reference) so every existing `gpu.device`/`gpu.queue` call
/// site keeps working unchanged.
pub struct GpuState {
    /// `None` only while the launch splash is drawing on it from its own
    /// thread (`take_surface`/`restore_surface`).
    pub surface: Option<wgpu::Surface<'static>>,
    /// The surface is out on loan to the launch splash -- the one time
    /// `surface` is meant to be `None`.
    lent: bool,
    /// How getting a frame to draw into has been going lately.
    pub health: SurfaceHealth,
    pub device: Arc<wgpu::Device>,
    pub queue: Arc<wgpu::Queue>,
    pub config: wgpu::SurfaceConfiguration,
    pub size: PhysicalSize<u32>,
    /// A size reported by `resize` that hasn't been applied to the
    /// surface yet -- actually applied by `apply_pending_resize`, called
    /// once at the top of every `redraw`, not by `resize` itself. Windows
    /// sends a `WindowEvent::Resized` for every intermediate size during
    /// a live drag-resize (and often several during a maximize/restore
    /// animation) -- `wgpu::Surface::configure` is a synchronous call
    /// that waits for the GPU to go idle, so reconfiguring on every one
    /// of those events (rather than once per actual frame, using
    /// whatever the latest size happens to be by then) can stall the
    /// whole redraw pipeline, VNC texture uploads included, for the
    /// entire gesture.
    pending_resize: Option<PhysicalSize<u32>>,
}

impl GpuContext {
    /// Builds the shared context together with the *first* frame's
    /// swapchain -- the adapter has to be requested against a real
    /// surface, so the two can't be created independently. Every later
    /// frame goes through `attach` instead, which needs no `await`.
    pub async fn new(window: Arc<Window>) -> (Self, GpuState) {
        let size = window.inner_size();
        let (context, surface, config) = Self::open(window).await.unwrap_or_else(|why| panic!("{why}"));
        surface.configure(&context.device, &config);
        let state = GpuState {
            surface: Some(surface),
            lent: false,
            health: SurfaceHealth::default(),
            device: Arc::clone(&context.device),
            queue: Arc::clone(&context.queue),
            config,
            size,
            pending_resize: None,
        };
        (context, state)
    }

    /// A new instance, adapter and device, for a device that's gone. The
    /// adapter is asked for again rather than kept: after a driver reset
    /// the old one may be dead too, and on a laptop with two GPUs the
    /// answer may differ. `window`'s surface is only needed to ask with,
    /// so nothing is configured here, and the old swapchains can stay
    /// until this has worked.
    pub async fn reopen(window: Arc<Window>) -> Result<Self, String> {
        Self::open(window).await.map(|(context, _, _)| context)
    }

    /// What's common to `new` and `reopen`: everything up to configuring
    /// the first surface.
    async fn open(window: Arc<Window>) -> Result<(Self, wgpu::Surface<'static>, wgpu::SurfaceConfiguration), String> {
        let size = window.inner_size();
        // `WGPU_BACKEND` (vulkan, dx12) chooses the API.
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let surface = instance.create_surface(window).map_err(|e| format!("create surface: {e}"))?;

        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
                apply_limit_buckets: false,
            })
            .await
            .map_err(|e| format!("request adapter: {e}"))?;

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("fenix-device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                experimental_features: wgpu::ExperimentalFeatures::default(),
                memory_hints: wgpu::MemoryHints::default(),
                trace: wgpu::Trace::Off,
            })
            .await
            .map_err(|e| format!("request device: {e}"))?;

        let mut config = surface
            .get_default_config(&adapter, size.width.max(1), size.height.max(1))
            .ok_or("surface unsupported by adapter")?;
        // `get_default_config` just takes the adapter's first reported
        // format, which on this stack (Windows/DX12 in particular) is
        // very often an `*Srgb` variant. Every pipeline here (`rect.rs`,
        // `text`, `surface.rs`, ...) writes already-final display
        // colors straight out of its fragment shader with no linear
        // color management anywhere in this codebase -- an `*Srgb`
        // *render target* asks the GPU to treat that fragment output as
        // linear light and re-encode it to sRGB on the way to the
        // screen, which visibly brightens/washes out everything drawn,
        // most obviously on real photographic content (a VNC pane's
        // desktop capture is *already* sRGB-encoded, so it was getting
        // gamma-encoded a second time on top of that). Forcing a plain
        // (non-sRGB) surface format keeps every pipeline's output byte-
        // for-byte as authored, matching what they already assume.
        if let Some(non_srgb) = surface.get_capabilities(&adapter).formats.into_iter().find(|f| !f.is_srgb()) {
            config.format = non_srgb;
        }
        config.present_mode = wgpu::PresentMode::AutoVsync;
        // Copying a frame out is how the theme cross-fade keeps the old
        // colours to fade from.
        if surface.get_capabilities(&adapter).usages.contains(wgpu::TextureUsages::COPY_SRC) {
            config.usage |= wgpu::TextureUsages::COPY_SRC;
        }

        let lost = Arc::new(Mutex::new(None));
        let told = Arc::clone(&lost);
        device.set_device_lost_callback(move |reason, message| {
            let why = format!("the GPU device was lost ({reason:?}{}{message})", if message.is_empty() { "" } else { ": " });
            crate::watchdog::note(&why);
            *told.lock().unwrap_or_else(|e| e.into_inner()) = Some(why);
        });
        // Left to wgpu, any of these panics: a window that's been asleep
        // isn't worth losing the editor over. In a debug build a validation
        // error still is, as it's a bug to fix.
        device.on_uncaptured_error(Arc::new(|error: wgpu::Error| {
            if cfg!(debug_assertions) && matches!(error, wgpu::Error::Validation { .. }) {
                panic!("wgpu: {error}");
            }
            crate::watchdog::note(&format!("wgpu: {error}"));
        }));

        let context =
            Self { instance, adapter, device: Arc::new(device), queue: Arc::new(queue), format: config.format, lost };
        Ok((context, surface, config))
    }

    /// Why the device went, if it has.
    pub fn lost(&self) -> Option<String> {
        self.lost.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// A second (third, ...) frame's swapchain on this same device.
    /// Synchronous, unlike `new`: the expensive, awaitable part
    /// (adapter and device request) already happened.
    ///
    /// The surface is forced to the context's `format` rather than
    /// asking this surface for its own default -- see `new`'s own note
    /// on why an `*Srgb` render target is wrong for every pipeline in
    /// this codebase, and `GpuContext::format`'s doc comment for why
    /// every frame has to agree on one.
    pub fn attach(&self, window: Arc<Window>) -> GpuState {
        self.try_attach(window).unwrap_or_else(|why| panic!("{why}"))
    }

    /// `attach`, for when it may not work: a machine just back from sleep
    /// can have a window its driver won't draw to yet.
    pub fn try_attach(&self, window: Arc<Window>) -> Result<GpuState, String> {
        let size = window.inner_size();
        let surface = self.instance.create_surface(window).map_err(|e| format!("create surface: {e}"))?;
        let mut config = surface
            .get_default_config(&self.adapter, size.width.max(1), size.height.max(1))
            .ok_or("surface unsupported by adapter")?;
        config.format = self.format;
        config.present_mode = wgpu::PresentMode::AutoVsync;
        if surface.get_capabilities(&self.adapter).usages.contains(wgpu::TextureUsages::COPY_SRC) {
            config.usage |= wgpu::TextureUsages::COPY_SRC;
        }
        surface.configure(&self.device, &config);

        Ok(GpuState {
            surface: Some(surface),
            lent: false,
            health: SurfaceHealth::default(),
            device: Arc::clone(&self.device),
            queue: Arc::clone(&self.queue),
            config,
            size,
            pending_resize: None,
        })
    }

    /// Replaces `gpu`'s surface with a new one for `window`, at the
    /// window's current size -- for a surface that has stopped giving
    /// out frames however it's configured, or one that never came back
    /// from the launch splash. The old one goes first: a window takes
    /// only one swapchain at a time.
    ///
    /// Not on a lost device, though: configuring on one fails, and a
    /// surface that was never configured takes the process down the first
    /// time a frame is asked of it.
    pub fn recreate_surface(&self, gpu: &mut GpuState, window: Arc<Window>) -> Result<(), String> {
        // Doesn't wait; it's what delivers a loss wgpu is still holding.
        let _ = self.device.poll(wgpu::PollType::Poll);
        if let Some(why) = self.lost() {
            return Err(why);
        }
        gpu.surface = None;
        let fresh = self.try_attach(window)?;
        gpu.surface = fresh.surface;
        gpu.config = fresh.config;
        gpu.size = fresh.size;
        gpu.pending_resize = None;
        gpu.lent = false;
        Ok(())
    }

    /// The adapter in use, and the graphics API it's driven through.
    pub fn adapter_info(&self) -> wgpu::AdapterInfo {
        self.adapter.get_info()
    }
}

impl GpuState {
    /// Lends the surface to the splash thread; `restore_surface` takes it
    /// back, with the size the splash last drew at.
    pub fn take_surface(&mut self) -> Option<wgpu::Surface<'static>> {
        self.lent = self.surface.is_some();
        self.surface.take()
    }

    pub fn restore_surface(&mut self, surface: wgpu::Surface<'static>) {
        self.surface = Some(surface);
        self.lent = false;
    }

    /// Whether the launch splash is drawing on the surface.
    pub fn lent(&self) -> bool {
        self.lent
    }

    /// Whether the surface is gone without being on loan to the splash:
    /// nothing would ever be drawn into the window again.
    pub fn surface_missing(&self) -> bool {
        self.surface.is_none() && !self.lent
    }

    /// Configures the surface again, at `size` when it's a real one --
    /// the window's size now rather than the one on record, which is
    /// what an out-of-date surface has usually fallen behind.
    pub fn reconfigure(&mut self, size: PhysicalSize<u32>) {
        if size.width > 0 && size.height > 0 {
            self.size = size;
            self.config.width = size.width;
            self.config.height = size.height;
        }
        self.pending_resize = None;
        if let Some(surface) = &self.surface {
            surface.configure(&self.device, &self.config);
        }
    }

    /// Whether a frame can be copied out (see `GpuContext::new`).
    pub fn can_copy_frames(&self) -> bool {
        self.config.usage.contains(wgpu::TextureUsages::COPY_SRC)
    }

    /// Records the window's latest reported size -- doesn't touch the
    /// surface itself; see `pending_resize`'s own doc comment for why
    /// that's deferred to `apply_pending_resize`.
    pub fn resize(&mut self, new_size: PhysicalSize<u32>) {
        if new_size.width > 0 && new_size.height > 0 {
            self.pending_resize = Some(new_size);
        }
    }

    /// Actually reconfigures the surface to the most recent size passed
    /// to `resize` since the last call, if any -- called once at the top
    /// of every `redraw`, coalescing however many `resize` calls landed
    /// since the previous frame into at most one real `surface.
    /// configure()`.
    pub fn apply_pending_resize(&mut self) {
        if let Some(size) = self.pending_resize.take() {
            self.size = size;
            self.config.width = size.width;
            self.config.height = size.height;
            if let Some(surface) = &self.surface {
                surface.configure(&self.device, &self.config);
            }
        }
    }
}

/// What to do about a surface that didn't give out a frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Recovery {
    /// Configure it again, at the window's current size.
    Reconfigure,
    /// Configuring hasn't helped: make a new surface for the window.
    Recreate,
    /// A new surface hasn't helped either: the device is what's wrong,
    /// whether or not the driver has said so. Everything is made again.
    Rebuild,
}

/// The run of frames a surface has failed to give out, and what to do
/// about each. One failure is routine -- the window changed size since the
/// surface was configured -- and configuring it again is the cure. A run
/// of them is not: left alone, the window goes on showing whatever was
/// presented last (the launch splash, when it starts with the first frame)
/// while the editor works on unseen behind it. So the surface is made
/// afresh, then the device, and the trouble is reported rather than
/// retried in silence.
#[derive(Debug, Default)]
pub struct SurfaceHealth {
    failures: u32,
    reported: bool,
}

/// The surface is made afresh at the third failure in a row.
const RECREATE_AT: u32 = 3;
/// The device at the sixth, and every `REBUILD_EVERY` after that.
const REBUILD_AT: u32 = 6;
const REBUILD_EVERY: u32 = 6;
/// Failures past this many in a row are tried again after `RETRY_WAIT`
/// rather than at once, so a surface that won't recover can't keep the
/// loop spinning.
const RETRY_UP_TO: u32 = 6;
/// How long to wait before asking again for a frame, past `RETRY_UP_TO`.
pub const RETRY_WAIT: Duration = Duration::from_secs(1);

/// What `SurfaceHealth::failed` decided.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FailureResponse {
    pub recovery: Recovery,
    /// Ask for another frame straight away.
    pub retry: bool,
    /// Worth a line in the log: the run just got long enough to matter.
    pub report: bool,
}

impl SurfaceHealth {
    /// Another frame couldn't be had.
    pub fn failed(&mut self) -> FailureResponse {
        self.failures = self.failures.saturating_add(1);
        let n = self.failures;
        let rebuild = n >= REBUILD_AT && (n - REBUILD_AT).is_multiple_of(REBUILD_EVERY);
        let report = n == RECREATE_AT && !self.reported;
        self.reported |= report;
        FailureResponse {
            recovery: if rebuild {
                Recovery::Rebuild
            } else if n == RECREATE_AT {
                Recovery::Recreate
            } else {
                Recovery::Reconfigure
            },
            retry: n <= RETRY_UP_TO,
            report,
        }
    }

    /// A frame was presented. True when that ends a run that was reported.
    pub fn presented(&mut self) -> bool {
        let recovered = self.reported;
        *self = SurfaceHealth::default();
        recovered
    }

    pub fn failures(&self) -> u32 {
        self.failures
    }
}

/// The longest wait between attempts at making the device again.
const MOST_BETWEEN_REBUILDS: Duration = Duration::from_secs(30);

/// How long a window has to draw once a rebuild is done.
pub const FRAME_AFTER_REBUILD: Duration = Duration::from_secs(10);

/// Getting the windows drawing again: what a failing surface, a lost
/// device or the machine waking has asked for, and when next to try.
/// Everything here is what's decided; `App::gpu_tick` does it.
#[derive(Debug, Default)]
pub struct GpuWatch {
    /// The device is to be made again, and why.
    rebuild: Option<String>,
    /// Attempts since a frame last reached the screen: what spaces the
    /// next one out. Reset by a frame, not by an attempt that seemed to
    /// work, since one that didn't would otherwise be retried flat out.
    attempts: u32,
    next_rebuild: Option<Instant>,
    /// Ask for a frame then, whatever else has or hasn't happened: a
    /// window with nothing moving has no other reason to.
    retry_at: Option<Instant>,
    /// A frame is expected by then, and what it means if there's none.
    frame_due: Option<(Instant, String)>,
}

/// What `GpuWatch::tick` says is due.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct GpuDue {
    /// Make the device again.
    pub rebuild: bool,
    /// Ask every window for a frame.
    pub redraw: bool,
    /// When to look again.
    pub next: Option<Instant>,
}

impl GpuWatch {
    /// The device is to be made again.
    pub fn want_rebuild(&mut self, why: String) {
        self.rebuild.get_or_insert(why);
    }

    /// Whether a rebuild is waiting: until it's done nothing is to be
    /// made on the old device.
    pub fn rebuild_pending(&self) -> bool {
        self.rebuild.is_some()
    }

    pub fn why(&self) -> Option<&str> {
        self.rebuild.as_deref()
    }

    /// Ask for a frame after `wait`, unless that's already planned.
    pub fn retry_after(&mut self, now: Instant, wait: Duration) {
        self.retry_at.get_or_insert(now + wait);
    }

    /// A frame has to reach the screen by `at`, or the device is suspect.
    pub fn expect_frame(&mut self, at: Instant, why: String) {
        self.frame_due = Some((at, why));
    }

    /// An attempt at making the device again was made. One that `worked`
    /// still has a frame to prove it with.
    pub fn rebuild_tried(&mut self, now: Instant, worked: bool) {
        self.attempts = self.attempts.saturating_add(1);
        self.next_rebuild = Some(now + self.backoff());
        if worked {
            self.rebuild = None;
            self.expect_frame(now + FRAME_AFTER_REBUILD, format!("no frame {FRAME_AFTER_REBUILD:?} after the GPU was set up again"));
        }
    }

    /// 1 s, 2 s, 4 s... up to `MOST_BETWEEN_REBUILDS`, after `attempts`.
    fn backoff(&self) -> Duration {
        Duration::from_secs(1u64 << self.attempts.saturating_sub(1).min(16)).min(MOST_BETWEEN_REBUILDS)
    }

    /// A frame reached the screen: all is well, and the next trouble
    /// starts from the beginning.
    pub fn presented(&mut self) {
        *self = GpuWatch::default();
    }

    /// What's due at `now`. `hidden` is that no window can be drawn into
    /// (all minimised): a frame can't be expected of one.
    pub fn tick(&mut self, now: Instant, hidden: bool) -> GpuDue {
        let mut due = GpuDue::default();
        if self.frame_due.as_ref().is_some_and(|(at, _)| now >= *at) {
            if let Some((_, why)) = self.frame_due.take().filter(|_| !hidden) {
                self.rebuild.get_or_insert(why);
            }
        }
        if self.retry_at.is_some_and(|at| now >= at) {
            self.retry_at = None;
            due.redraw = true;
        }
        let rebuild_at = self.rebuild.as_ref().map(|_| self.next_rebuild.unwrap_or(now));
        due.rebuild = rebuild_at.is_some_and(|at| now >= at);
        due.next = [self.retry_at, rebuild_at.filter(|at| *at > now), self.frame_due.as_ref().map(|(at, _)| *at)]
            .into_iter()
            .flatten()
            .min();
        due
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_single_failure_is_reconfigured_and_retried_quietly() {
        let mut health = SurfaceHealth::default();
        assert_eq!(health.failed(), FailureResponse { recovery: Recovery::Reconfigure, retry: true, report: false });
        assert!(!health.presented());
        assert_eq!(health.failures(), 0);
    }

    #[test]
    fn a_run_of_failures_climbs_from_the_surface_to_the_device_and_says_so_once() {
        let mut health = SurfaceHealth::default();
        let responses: Vec<FailureResponse> = (0..REBUILD_AT + REBUILD_EVERY).map(|_| health.failed()).collect();
        let at = |recovery| -> Vec<u32> { (1..).zip(&responses).filter(|(_, r)| r.recovery == recovery).map(|(n, _)| n).collect() };
        assert_eq!(at(Recovery::Recreate), vec![RECREATE_AT]);
        assert_eq!(at(Recovery::Rebuild), vec![REBUILD_AT, REBUILD_AT + REBUILD_EVERY]);
        let reported: Vec<u32> = (1..).zip(&responses).filter(|(_, response)| response.report).map(|(n, _)| n).collect();
        assert_eq!(reported, vec![RECREATE_AT]);
    }

    #[test]
    fn retries_stop_after_a_while_so_the_loop_can_go_idle() {
        let mut health = SurfaceHealth::default();
        let retried = (0..50).map(|_| health.failed()).take_while(|response| response.retry).count();
        assert_eq!(retried, RETRY_UP_TO as usize);
        assert!(!health.failed().retry);
    }

    #[test]
    fn drawing_again_ends_the_run_and_says_so_only_if_it_was_reported() {
        let mut health = SurfaceHealth::default();
        for _ in 0..RECREATE_AT {
            health.failed();
        }
        assert!(health.presented());
        assert!(!health.presented());
        assert!(!health.failed().report);
    }

    #[test]
    fn nothing_wrong_leaves_nothing_to_do_or_wait_for() {
        assert_eq!(GpuWatch::default().tick(Instant::now(), false), GpuDue::default());
    }

    #[test]
    fn a_rebuild_is_tried_at_once_then_further_and_further_apart() {
        let start = Instant::now();
        let mut watch = GpuWatch::default();
        watch.want_rebuild("lost".into());
        assert!(watch.tick(start, false).rebuild, "the first attempt doesn't wait");
        let mut now = start;
        let mut waits = Vec::new();
        for _ in 0..7 {
            watch.rebuild_tried(now, false);
            let due = watch.tick(now, false);
            assert!(!due.rebuild);
            let next = due.next.expect("it says when to look again");
            waits.push((next - now).as_secs());
            now = next;
            assert!(watch.tick(now, false).rebuild);
        }
        assert_eq!(waits, [1, 2, 4, 8, 16, 30, 30]);
    }

    /// The driver saying the device is fine proves nothing: only a frame
    /// on the screen ends it, and until one does attempts keep backing off.
    #[test]
    fn a_rebuild_that_seemed_to_work_is_tried_again_if_no_frame_comes() {
        let start = Instant::now();
        let mut watch = GpuWatch::default();
        watch.want_rebuild("lost".into());
        watch.rebuild_tried(start, true);
        assert!(!watch.rebuild_pending());
        let at = watch.tick(start, false).next.expect("waits for the frame");
        assert_eq!(at, start + FRAME_AFTER_REBUILD);
        watch.tick(at, false);
        assert!(watch.rebuild_pending());
        watch.rebuild_tried(at, true);
        assert_eq!(watch.attempts, 2);
    }

    #[test]
    fn a_frame_ends_it_all() {
        let now = Instant::now();
        let mut watch = GpuWatch::default();
        watch.want_rebuild("lost".into());
        watch.rebuild_tried(now, true);
        watch.retry_after(now, RETRY_WAIT);
        watch.presented();
        assert_eq!(watch.attempts, 0);
        assert_eq!(watch.tick(now + Duration::from_secs(60), false), GpuDue::default());
    }

    #[test]
    fn no_frame_from_a_minimised_window_isnt_trouble() {
        let now = Instant::now();
        let mut watch = GpuWatch::default();
        watch.expect_frame(now, "woke".into());
        assert_eq!(watch.tick(now, true), GpuDue::default());
        assert!(!watch.rebuild_pending());
    }

    #[test]
    fn a_retry_asks_for_a_frame_once_when_its_time_comes() {
        let now = Instant::now();
        let mut watch = GpuWatch::default();
        watch.retry_after(now, RETRY_WAIT);
        watch.retry_after(now + Duration::from_millis(500), RETRY_WAIT);
        assert_eq!(watch.tick(now, false).next, Some(now + RETRY_WAIT));
        assert!(watch.tick(now + RETRY_WAIT, false).redraw);
        assert!(!watch.tick(now + RETRY_WAIT, false).redraw);
    }
}
