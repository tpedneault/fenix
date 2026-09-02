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
    pub surface: wgpu::Surface<'static>,
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
        // `text`, `vnc_texture.rs`, ...) writes already-final display
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
        surface.configure(&device, &config);

        let device = Arc::new(device);
        let queue = Arc::new(queue);
        let context =
            Self { instance, adapter, device: Arc::clone(&device), queue: Arc::clone(&queue), format: config.format };
        let state = GpuState { surface, device, queue, config, size, pending_resize: None };
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
    // Nothing opens a second frame yet -- the caller arrives with the
    // `frame.new` command. Without this, the whole context reads as dead
    // code, since `attach` is the only thing that consumes its fields.
    #[allow(dead_code)]
    pub fn attach(&self, window: Arc<Window>) -> GpuState {
        let size = window.inner_size();
        let surface = self.instance.create_surface(window).expect("create surface");
        let mut config = surface
            .get_default_config(&self.adapter, size.width.max(1), size.height.max(1))
            .expect("surface unsupported by adapter");
        config.format = self.format;
        config.present_mode = wgpu::PresentMode::AutoVsync;
        surface.configure(&self.device, &config);

        GpuState {
            surface,
            device: Arc::clone(&self.device),
            queue: Arc::clone(&self.queue),
            config,
            size,
            pending_resize: None,
        }
    }
}

impl GpuState {

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
            self.surface.configure(&self.device, &self.config);
        }
    }
}
