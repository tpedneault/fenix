use crate::gpu::GpuState;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Vertex {
    position: [f32; 2],
    uv: [f32; 2],
}

const VERTICES_PER_QUAD: usize = 6;

/// One VNC session's live pixel framebuffer as a sampled GPU texture.
/// Recreated (`VncPipeline::create_texture`) whenever the session's
/// resolution changes; updated in place otherwise via `VncPipeline::
/// upload_rect`. Owned by the session (`app.rs`'s `VncSession`), not by
/// `VncPipeline` itself, since up to three sessions can be open at once,
/// each at its own resolution.
pub struct VncTexture {
    texture: wgpu::Texture,
    bind_group: wgpu::BindGroup,
}

/// Draws a live VNC framebuffer as a textured quad -- the first real
/// texture/sampler/bind-group pipeline in this codebase (every other
/// visual, including the file explorer's own "icons," is text glyphs or
/// flat rects -- see `RectRenderer`, which this mirrors structurally:
/// same shader-embedding convention, same NDC-space quad math, same
/// `wgpu::Queue`-driven upload-then-draw shape). One `VncPipeline` is
/// shared by every open VNC session; each session owns its own
/// `VncTexture` (different resolutions, different pixel data).
pub struct VncPipeline {
    pipeline: wgpu::RenderPipeline,
    /// Same shader and bind group layout as `pipeline`, but alpha
    /// blended -- for `draw_cursor`. The guest's cursor (the
    /// `CursorPseudo` encoding) is a small BGRA image whose alpha comes
    /// from RFB's own bitmask, so drawing it through the opaque pane
    /// pipeline would paint its fully-transparent pixels as solid black
    /// instead of letting the desktop show through.
    cursor_pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    /// Exactly one quad's worth of vertices, rewritten via
    /// `queue.write_buffer` immediately before every `draw` call -- a
    /// VNC pane is always exactly one rectangle, so unlike
    /// `RectRenderer` (many accumulated rects per frame) there's no
    /// separate per-frame accumulation step.
    vertex_buffer: wgpu::Buffer,
    /// The cursor's own quad, kept separate from `vertex_buffer` rather
    /// than reusing it: `queue.write_buffer` calls are all applied
    /// before any of the pass's draws execute, so a pane and its cursor
    /// sharing one buffer would both end up drawn with whichever
    /// geometry was written last.
    cursor_vertex_buffer: wgpu::Buffer,
}

impl VncPipeline {
    pub fn new(gpu: &GpuState) -> Self {
        let shader = gpu.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("vnc-shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("vnc_texture.wgsl").into()),
        });

        let bind_group_layout = gpu.device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("vnc-bind-group-layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let pipeline_layout = gpu.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("vnc-pipeline-layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        // Identical but for the blend mode -- see `cursor_pipeline`.
        let build_pipeline = |label: &str, blend: Option<wgpu::BlendState>| {
            gpu.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<Vertex>() as wgpu::BufferAddress,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &[
                            wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x2, offset: 0, shader_location: 0 },
                            wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x2, offset: 8, shader_location: 1 },
                        ],
                    })],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_main"),
                    targets: &[Some(wgpu::ColorTargetState { format: gpu.config.format, blend, write_mask: wgpu::ColorWrites::ALL })],
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        // An opaque video frame -- no alpha blending, unlike
        // `RectRenderer`'s `ALPHA_BLENDING` (used for translucent
        // selection/highlight rects).
        let pipeline = build_pipeline("vnc-pipeline", None);
        let cursor_pipeline = build_pipeline("vnc-cursor-pipeline", Some(wgpu::BlendState::ALPHA_BLENDING));

        let sampler = gpu.device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("vnc-sampler"),
            // Linear filtering is what makes this project's client-side
            // scaling (no server-side `SetDesktopSize` request in v1)
            // look acceptable rather than blocky when a pane's pixel
            // size doesn't match the VM's native resolution.
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let quad_buffer = |label: &str| {
            gpu.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: (VERTICES_PER_QUAD * std::mem::size_of::<Vertex>()) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let vertex_buffer = quad_buffer("vnc-vertex-buffer");
        let cursor_vertex_buffer = quad_buffer("vnc-cursor-vertex-buffer");

        Self { pipeline, cursor_pipeline, bind_group_layout, sampler, vertex_buffer, cursor_vertex_buffer }
    }

    /// Creates (or recreates, on a resolution change) the GPU texture
    /// backing one session's framebuffer. `width`/`height` of `0` are
    /// clamped to `1` -- a zero-sized texture is invalid, and a session
    /// briefly has no known resolution yet right after connecting.
    pub fn create_texture(&self, gpu: &GpuState, width: u32, height: u32) -> VncTexture {
        let width = width.max(1);
        let height = height.max(1);
        let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("vnc-texture"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Bgra8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("vnc-bind-group"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
            ],
        });
        VncTexture { texture, bind_group }
    }

    /// Uploads one dirty sub-rectangle of already-BGRA pixel data (see
    /// `fenix_vnc::VncFrame::Rect` -- requested via `PixelFormat::bgra()`
    /// so this never needs a channel-swizzle first). `width`/`height` of
    /// `0` are a no-op rather than an invalid zero-sized copy.
    pub fn upload_rect(&self, gpu: &GpuState, tex: &VncTexture, x: u32, y: u32, width: u32, height: u32, bgra: &[u8]) {
        if width == 0 || height == 0 {
            return;
        }
        gpu.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &tex.texture,
                mip_level: 0,
                origin: wgpu::Origin3d { x, y, z: 0 },
                aspect: wgpu::TextureAspect::All,
            },
            bgra,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(width * 4), rows_per_image: Some(height) },
            wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        );
    }

    /// Draws `tex` scaled to exactly fill the pixel rect
    /// `(dest_x, dest_y, dest_w, dest_h)` -- this project's v1 resize
    /// strategy (client-side scaling, no server-side `SetDesktopSize`
    /// request) means the whole framebuffer always maps onto the whole
    /// pane, whatever its current on-screen size is. Same NDC-space
    /// quad conversion `RectRenderer::push_rect` uses.
    pub fn draw<'pass>(&'pass self, gpu: &GpuState, pass: &mut wgpu::RenderPass<'pass>, tex: &'pass VncTexture, dest_x: f32, dest_y: f32, dest_w: f32, dest_h: f32) {
        let vertices = quad_vertices(gpu, dest_x, dest_y, dest_w, dest_h);
        gpu.queue.write_buffer(&self.vertex_buffer, 0, bytemuck::cast_slice(&vertices));

        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &tex.bind_group, &[]);
        pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
        pass.draw(0..VERTICES_PER_QUAD as u32, 0..1);
    }

    /// Draws the guest's mouse cursor on top of an already-drawn pane,
    /// alpha blended (see `cursor_pipeline`) so its transparent pixels
    /// let the desktop underneath show through.
    ///
    /// Separate from `draw` rather than a flag on it because the two
    /// must be able to coexist within one render pass, which needs both
    /// a different pipeline and its own vertex buffer.
    pub fn draw_cursor<'pass>(
        &'pass self,
        gpu: &GpuState,
        pass: &mut wgpu::RenderPass<'pass>,
        tex: &'pass VncTexture,
        dest_x: f32,
        dest_y: f32,
        dest_w: f32,
        dest_h: f32,
    ) {
        let vertices = quad_vertices(gpu, dest_x, dest_y, dest_w, dest_h);
        gpu.queue.write_buffer(&self.cursor_vertex_buffer, 0, bytemuck::cast_slice(&vertices));

        pass.set_pipeline(&self.cursor_pipeline);
        pass.set_bind_group(0, &tex.bind_group, &[]);
        pass.set_vertex_buffer(0, self.cursor_vertex_buffer.slice(..));
        pass.draw(0..VERTICES_PER_QUAD as u32, 0..1);
    }
}

/// One screen-space rectangle as two triangles in NDC, UV-mapped to the
/// whole texture -- the same conversion `RectRenderer::push_rect` uses.
fn quad_vertices(gpu: &GpuState, dest_x: f32, dest_y: f32, dest_w: f32, dest_h: f32) -> [Vertex; VERTICES_PER_QUAD] {
    let sw = gpu.config.width as f32;
    let sh = gpu.config.height as f32;
    let to_ndc = |px: f32, py: f32| [(px / sw) * 2.0 - 1.0, 1.0 - (py / sh) * 2.0];

    let p00 = to_ndc(dest_x, dest_y);
    let p10 = to_ndc(dest_x + dest_w, dest_y);
    let p01 = to_ndc(dest_x, dest_y + dest_h);
    let p11 = to_ndc(dest_x + dest_w, dest_y + dest_h);
    [
        Vertex { position: p00, uv: [0.0, 0.0] },
        Vertex { position: p10, uv: [1.0, 0.0] },
        Vertex { position: p01, uv: [0.0, 1.0] },
        Vertex { position: p10, uv: [1.0, 0.0] },
        Vertex { position: p11, uv: [1.0, 1.0] },
        Vertex { position: p01, uv: [0.0, 1.0] },
    ]
}
