use std::sync::Arc;

use winit::dpi::PhysicalSize;
use winit::window::Window;

/// Owns the wgpu surface/device/queue for the editor window.
pub struct GpuState {
    pub surface: wgpu::Surface<'static>,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
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

impl GpuState {
    pub async fn new(window: Arc<Window>) -> Self {
        let size = window.inner_size();
        let instance = wgpu::Instance::default();
        let surface = instance.create_surface(window.clone()).expect("create surface");

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

        Self { surface, device, queue, config, size, pending_resize: None }
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
            self.surface.configure(&self.device, &self.config);
        }
    }
}
