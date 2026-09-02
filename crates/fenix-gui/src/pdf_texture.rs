use crate::gpu::GpuState;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Vertex {
    position: [f32; 2],
    uv: [f32; 2],
}

const VERTICES_PER_QUAD: usize = 6;

/// One rendered PDF page as a sampled GPU texture. Recreated (`PdfPipeline
/// ::create_texture`) whenever its pixel size changes (a page turn, a
/// resize, a zoom change -- every `RenderPage` reply can carry a
/// different `width`/`height`, see `fenix_pdf::PdfResponse::
/// PageRendered`); otherwise just re-uploaded wholesale via `PdfPipeline
/// ::upload_rect` (a freshly rendered page always arrives complete, no
/// dirty-rects the way VNC's incremental framebuffer updates have).
/// Owned by the session (`app.rs`'s `PdfSession`), not by `PdfPipeline`
/// itself, since several PDFs can be open at once, each at its own
/// current page size.
pub struct PdfTexture {
    texture: wgpu::Texture,
    bind_group: wgpu::BindGroup,
}

impl PdfTexture {
    /// This texture's own pixel size, so a caller can tell whether the
    /// crop it's about to upload still fits the texture it already has
    /// (the common case -- a pan moves the crop's *origin*, never its
    /// size) or genuinely needs a new one from `PdfPipeline::create_
    /// texture`. Without this a pan recreated a multi-megabyte texture
    /// and its bind group on every keypress just to upload the same
    /// number of bytes into it.
    pub fn size(&self) -> (u32, u32) {
        (self.texture.width(), self.texture.height())
    }
}

/// Draws a rendered PDF page as a textured quad -- a structural duplicate
/// of `vnc_texture::VncPipeline` (same shader-embedding convention, same
/// NDC-space quad math, same `wgpu::Queue`-driven upload-then-draw
/// shape), kept as its own separate pipeline rather than generalizing the
/// two into one shared type: they'd end up byte-for-byte identical today,
/// but duplicating keeps this feature branch from touching VNC's already-
/// working code (see `fenix-pdf`'s own workspace-plan doc comment). One
/// `PdfPipeline` is shared by every open PDF session; each session owns
/// its own `PdfTexture`.
pub struct PdfPipeline {
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    /// Exactly one quad's worth of vertices, rewritten via
    /// `queue.write_buffer` immediately before every `draw` call -- a PDF
    /// pane is always exactly one rectangle.
    vertex_buffer: wgpu::Buffer,
}

impl PdfPipeline {
    pub fn new(gpu: &GpuState) -> Self {
        let shader = gpu.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("pdf-shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("pdf_texture.wgsl").into()),
        });

        let bind_group_layout = gpu.device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("pdf-bind-group-layout"),
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
            label: Some("pdf-pipeline-layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        let pipeline = gpu.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("pdf-pipeline"),
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
                targets: &[Some(wgpu::ColorTargetState {
                    format: gpu.config.format,
                    // An opaque rendered page -- no alpha blending needed.
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let sampler = gpu.device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("pdf-sampler"),
            // Linear filtering: the pane's pixel size and the page's
            // rendered pixel size are kept in lockstep by `fenix_pdf::
            // coords::fit_page_size` (re-requested on every resize), so
            // this mostly avoids ever needing to stretch -- linear still
            // covers the brief window between a resize and the next
            // render reply landing.
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let vertex_buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("pdf-vertex-buffer"),
            size: (VERTICES_PER_QUAD * std::mem::size_of::<Vertex>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self { pipeline, bind_group_layout, sampler, vertex_buffer }
    }

    /// Creates (or recreates, on a page-size change) the GPU texture
    /// backing one session's current page render. `width`/`height` of `0`
    /// are clamped to `1` -- a zero-sized texture is invalid, and a
    /// session briefly has no rendered page yet right after opening.
    pub fn create_texture(&self, gpu: &GpuState, width: u32, height: u32) -> PdfTexture {
        let width = width.max(1);
        let height = height.max(1);
        let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("pdf-texture"),
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
            label: Some("pdf-bind-group"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
            ],
        });
        PdfTexture { texture, bind_group }
    }

    /// Uploads pixel data into `tex` starting at `(x, y)` -- always the
    /// whole rendered page at `(0, 0)` for now (see `PdfTexture`'s own
    /// doc comment on why there are no dirty-rects here), but takes an
    /// offset/size rather than assuming the whole texture so it can be
    /// reused for a sub-rect upload later (e.g. panning a zoomed-in page)
    /// without a second near-identical method. `width`/`height` of `0`
    /// are a no-op rather than an invalid zero-sized copy.
    pub fn upload_rect(&self, gpu: &GpuState, tex: &PdfTexture, x: u32, y: u32, width: u32, height: u32, bgra: &[u8]) {
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
    /// `(dest_x, dest_y, dest_w, dest_h)`. Same NDC-space quad conversion
    /// `vnc_texture::VncPipeline::draw`/`RectRenderer::push_rect` use.
    pub fn draw<'pass>(&'pass self, gpu: &GpuState, pass: &mut wgpu::RenderPass<'pass>, tex: &'pass PdfTexture, dest_x: f32, dest_y: f32, dest_w: f32, dest_h: f32) {
        let sw = gpu.config.width as f32;
        let sh = gpu.config.height as f32;
        let to_ndc = |px: f32, py: f32| [(px / sw) * 2.0 - 1.0, 1.0 - (py / sh) * 2.0];

        let p00 = to_ndc(dest_x, dest_y);
        let p10 = to_ndc(dest_x + dest_w, dest_y);
        let p01 = to_ndc(dest_x, dest_y + dest_h);
        let p11 = to_ndc(dest_x + dest_w, dest_y + dest_h);

        let vertices = [
            Vertex { position: p00, uv: [0.0, 0.0] },
            Vertex { position: p10, uv: [1.0, 0.0] },
            Vertex { position: p01, uv: [0.0, 1.0] },
            Vertex { position: p10, uv: [1.0, 0.0] },
            Vertex { position: p11, uv: [1.0, 1.0] },
            Vertex { position: p01, uv: [0.0, 1.0] },
        ];
        gpu.queue.write_buffer(&self.vertex_buffer, 0, bytemuck::cast_slice(&vertices));

        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &tex.bind_group, &[]);
        pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
        pass.draw(0..VERTICES_PER_QUAD as u32, 0..1);
    }
}
