use std::sync::Arc;

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
        let instance = wgpu::Instance::default();
        let surface = instance.create_surface(window).expect("create surface");

        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
                apply_limit_buckets: false,
            })
            .await
            .expect("request adapter");

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
            .expect("request device");

        let mut config = surface
            .get_default_config(&adapter, size.width.max(1), size.height.max(1))
            .expect("surface unsupported by adapter");
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
        surface.configure(&device, &config);

        let device = Arc::new(device);
        let queue = Arc::new(queue);
        let context =
            Self { instance, adapter, device: Arc::clone(&device), queue: Arc::clone(&queue), format: config.format };
        let state = GpuState {
            surface: Some(surface),
            lent: false,
            health: SurfaceHealth::default(),
            device,
            queue,
            config,
            size,
            pending_resize: None,
        };
        (context, state)
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
        let size = window.inner_size();
        let surface = self.instance.create_surface(window).expect("create surface");
        let mut config = surface
            .get_default_config(&self.adapter, size.width.max(1), size.height.max(1))
            .expect("surface unsupported by adapter");
        config.format = self.format;
        config.present_mode = wgpu::PresentMode::AutoVsync;
        if surface.get_capabilities(&self.adapter).usages.contains(wgpu::TextureUsages::COPY_SRC) {
            config.usage |= wgpu::TextureUsages::COPY_SRC;
        }
        surface.configure(&self.device, &config);

        GpuState {
            surface: Some(surface),
            lent: false,
            health: SurfaceHealth::default(),
            device: Arc::clone(&self.device),
            queue: Arc::clone(&self.queue),
            config,
            size,
            pending_resize: None,
        }
    }

    /// Replaces `gpu`'s surface with a new one for `window`, at the
    /// window's current size -- for a surface that has stopped giving
    /// out frames however it's configured, or one that never came back
    /// from the launch splash. The old one goes first: a window takes
    /// only one swapchain at a time.
    pub fn recreate_surface(&self, gpu: &mut GpuState, window: Arc<Window>) {
        gpu.surface = None;
        let fresh = self.attach(window);
        gpu.surface = fresh.surface;
        gpu.config = fresh.config;
        gpu.size = fresh.size;
        gpu.pending_resize = None;
        gpu.lent = false;
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
}

/// The run of frames a surface has failed to give out (`Outdated` or
/// `Lost`), and what to do about each. One failure is routine -- the
/// window changed size since the surface was configured -- and
/// configuring it again is the cure. A run of them is not: left alone,
/// the window goes on showing whatever was presented last (the launch
/// splash, when it starts with the first frame) while the editor works
/// on unseen behind it. So the surface is made afresh, and the trouble
/// is reported rather than retried in silence.
#[derive(Debug, Default)]
pub struct SurfaceHealth {
    failures: u32,
    reported: bool,
}

/// The surface is made afresh at the third failure in a row, and every
/// `RECREATE_EVERY` after that.
const RECREATE_AT: u32 = 3;
const RECREATE_EVERY: u32 = 30;
/// Failures past this many in a row wait for the next event to be tried
/// again, so a surface that won't recover can't keep the loop spinning.
const RETRY_UP_TO: u32 = 6;

/// What `SurfaceHealth::failed` decided.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FailureResponse {
    pub recovery: Recovery,
    /// Ask for another frame straight away.
    pub retry: bool,
    /// Worth a line on stderr: the run just got long enough to matter.
    pub report: bool,
}

impl SurfaceHealth {
    /// Another frame couldn't be had.
    pub fn failed(&mut self) -> FailureResponse {
        self.failures = self.failures.saturating_add(1);
        let n = self.failures;
        let recreate = n >= RECREATE_AT && (n - RECREATE_AT).is_multiple_of(RECREATE_EVERY);
        let report = n == RECREATE_AT && !self.reported;
        self.reported |= report;
        FailureResponse {
            recovery: if recreate { Recovery::Recreate } else { Recovery::Reconfigure },
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
    fn a_run_of_failures_recreates_the_surface_and_says_so_once() {
        let mut health = SurfaceHealth::default();
        let responses: Vec<FailureResponse> = (0..RECREATE_AT + RECREATE_EVERY).map(|_| health.failed()).collect();
        let recreated: Vec<u32> = (1..)
            .zip(&responses)
            .filter(|(_, response)| response.recovery == Recovery::Recreate)
            .map(|(n, _)| n)
            .collect();
        assert_eq!(recreated, vec![RECREATE_AT, RECREATE_AT + RECREATE_EVERY]);
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
}
