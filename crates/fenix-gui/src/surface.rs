//! Surfaces: images Fenix draws as textured quads -- a VNC framebuffer
//! and its guest cursor, a rendered PDF page, a picture on a page, Home's
//! logo. Whoever owns the pixels owns its `Surface` and uploads to it
//! (whole, or a dirty rectangle at a time); one `SurfacePipeline` draws
//! them all. This is the host side of the surface API extensions will
//! push frames through (`docs/EXTENSIONS_PLAN.md` §5.4).

use crate::gpu::GpuState;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Vertex {
    position: [f32; 2],
    uv: [f32; 2],
    /// Recolouring: the colour white becomes (`w` = 1 turns it on) and
    /// the colour black becomes. See `surface.wgsl`.
    paper: [f32; 4],
    ink: [f32; 4],
}

const VERTICES_PER_QUAD: usize = 6;

/// Quads the vertex buffer starts out able to hold. `flush` grows it if
/// a frame ever needs more.
const INITIAL_QUADS: usize = 32;

/// The whole texture, as `(u0, v0, u1, v1)`.
pub const FULL: (f32, f32, f32, f32) = (0.0, 0.0, 1.0, 1.0);

/// One image as a sampled GPU texture. Recreated (`SurfacePipeline::
/// create`) when its pixel size changes, updated in place otherwise
/// (`SurfacePipeline::upload_rect`). Owned by whatever owns the pixels --
/// a VNC session, a PDF view's page cache, a page's pictures -- since
/// each has its own size and contents.
pub struct Surface {
    texture: wgpu::Texture,
    bind_group: wgpu::BindGroup,
}

impl Surface {
    /// This surface's own pixel size, so a caller can tell whether what
    /// it's about to upload still fits (the common case) or needs a new
    /// one from `SurfacePipeline::create`.
    pub fn size(&self) -> (u32, u32) {
        (self.texture.width(), self.texture.height())
    }
}

/// How a quad lands on what's already drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Blend {
    /// Replaces it: a video frame, a rendered page.
    Opaque,
    /// Blended over it (straight, not premultiplied, alpha): the guest's
    /// cursor, whose alpha comes from RFB's own bitmask, and pictures
    /// with transparency.
    Over,
}

/// Draws surfaces as textured quads. One per window, shared by every
/// surface.
///
/// Every quad a frame draws is recorded by `push` and uploaded once by
/// `flush`, before the render pass, then drawn by slot inside it.
/// Deliberately not "write one quad immediately before each draw":
/// `queue.write_buffer` calls are all applied before *any* of the pass's
/// draws execute, so draws sharing one slot all end up using whichever
/// geometry was written last (two panes showing one VNC session drew
/// nothing in the first and the second twice). Same accumulate-then-
/// flush-then-render shape as `RectRenderer`.
pub struct SurfacePipeline {
    opaque: wgpu::RenderPipeline,
    blended: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    vertices: Vec<Vertex>,
    vertex_buffer: wgpu::Buffer,
    /// Quads `vertex_buffer` can currently hold, grown by `flush`.
    capacity: usize,
}

impl SurfacePipeline {
    pub fn new(gpu: &GpuState) -> Self {
        let shader = gpu
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("surface-shader"),
                source: wgpu::ShaderSource::Wgsl(include_str!("surface.wgsl").into()),
            });

        let bind_group_layout =
            gpu.device
                .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("surface-bind-group-layout"),
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

        let pipeline_layout = gpu
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("surface-pipeline-layout"),
                bind_group_layouts: &[Some(&bind_group_layout)],
                immediate_size: 0,
            });

        // Identical but for the blend mode -- see `Blend`.
        let build_pipeline = |label: &str, blend: Option<wgpu::BlendState>| {
            gpu.device
                .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some(label),
                    layout: Some(&pipeline_layout),
                    vertex: wgpu::VertexState {
                        module: &shader,
                        entry_point: Some("vs_main"),
                        buffers: &[Some(wgpu::VertexBufferLayout {
                            array_stride: std::mem::size_of::<Vertex>() as wgpu::BufferAddress,
                            step_mode: wgpu::VertexStepMode::Vertex,
                            attributes: &[
                                wgpu::VertexAttribute {
                                    format: wgpu::VertexFormat::Float32x2,
                                    offset: 0,
                                    shader_location: 0,
                                },
                                wgpu::VertexAttribute {
                                    format: wgpu::VertexFormat::Float32x2,
                                    offset: 8,
                                    shader_location: 1,
                                },
                                wgpu::VertexAttribute {
                                    format: wgpu::VertexFormat::Float32x4,
                                    offset: 16,
                                    shader_location: 2,
                                },
                                wgpu::VertexAttribute {
                                    format: wgpu::VertexFormat::Float32x4,
                                    offset: 32,
                                    shader_location: 3,
                                },
                            ],
                        })],
                        compilation_options: Default::default(),
                    },
                    fragment: Some(wgpu::FragmentState {
                        module: &shader,
                        entry_point: Some("fs_main"),
                        targets: &[Some(wgpu::ColorTargetState {
                            format: gpu.config.format,
                            blend,
                            write_mask: wgpu::ColorWrites::ALL,
                        })],
                        compilation_options: Default::default(),
                    }),
                    primitive: wgpu::PrimitiveState::default(),
                    depth_stencil: None,
                    multisample: wgpu::MultisampleState::default(),
                    multiview_mask: None,
                    cache: None,
                })
        };
        let opaque = build_pipeline("surface-pipeline", None);
        let blended = build_pipeline(
            "surface-blended-pipeline",
            Some(wgpu::BlendState::ALPHA_BLENDING),
        );

        let sampler = gpu.device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("surface-sampler"),
            // Linear: a VNC pane is scaled client-side to whatever size
            // the pane is, and a PDF page is re-rendered at the pane's
            // size but stretched in the moment between a resize and the
            // new render landing. Linear keeps both from looking blocky.
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let capacity = INITIAL_QUADS;
        let vertex_buffer = Self::new_buffer(gpu, capacity);

        Self {
            opaque,
            blended,
            bind_group_layout,
            sampler,
            vertices: Vec::new(),
            vertex_buffer,
            capacity,
        }
    }

    fn new_buffer(gpu: &GpuState, quads: usize) -> wgpu::Buffer {
        gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("surface-vertex-buffer"),
            size: (quads * VERTICES_PER_QUAD * std::mem::size_of::<Vertex>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    /// Creates (or recreates, on a size change) a surface. `width`/
    /// `height` of `0` are clamped to `1` -- a zero-sized texture is
    /// invalid, and a VNC session briefly has no known resolution right
    /// after connecting.
    pub fn create(&self, gpu: &GpuState, width: u32, height: u32) -> Surface {
        let width = width.max(1);
        let height = height.max(1);
        let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("surface-texture"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Bgra8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("surface-bind-group"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        Surface {
            texture,
            bind_group,
        }
    }

    /// Uploads BGRA pixels into `surface` at `(x, y)`: a whole image, or
    /// one dirty rectangle of a VNC framebuffer (`fenix_vnc::VncFrame::
    /// Rect`, requested as BGRA so it never needs a swizzle first).
    /// `width`/`height` of `0` are a no-op rather than an invalid
    /// zero-sized copy.
    pub fn upload_rect(
        &self,
        gpu: &GpuState,
        surface: &Surface,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
        bgra: &[u8],
    ) {
        if width == 0 || height == 0 {
            return;
        }
        gpu.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &surface.texture,
                mip_level: 0,
                origin: wgpu::Origin3d { x, y, z: 0 },
                aspect: wgpu::TextureAspect::All,
            },
            bgra,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
    }

    /// Drops the previous frame's quads -- call once per frame, before
    /// any `push`.
    pub fn clear(&mut self) {
        self.vertices.clear();
    }

    /// Records a quad showing the part `uv` (`(u0, v0, u1, v1)`, 0..1;
    /// `FULL` for all of it) of a surface in the pixel rect `dest`
    /// (`(x, y, w, h)`), recoloured when `recolor` is `(paper, ink)`:
    /// white becomes `paper`, black `ink`, and colours keep their hue.
    /// Returns the slot to `draw` it with.
    pub fn push(
        &mut self,
        gpu: &GpuState,
        dest: (f32, f32, f32, f32),
        uv: (f32, f32, f32, f32),
        recolor: Option<([f32; 3], [f32; 3])>,
    ) -> usize {
        let slot = self.vertices.len() / VERTICES_PER_QUAD;
        let screen = (gpu.config.width as f32, gpu.config.height as f32);
        self.vertices
            .extend_from_slice(&quad_vertices(screen, dest, uv, recolor));
        slot
    }

    /// Uploads every quad pushed since `clear`, growing the buffer if
    /// this frame needed more than it could hold. Must run before the
    /// render pass that draws them.
    pub fn flush(&mut self, gpu: &GpuState) {
        if self.vertices.is_empty() {
            return;
        }
        let quads = self.vertices.len() / VERTICES_PER_QUAD;
        if quads > self.capacity {
            self.capacity = quads.next_power_of_two();
            self.vertex_buffer = Self::new_buffer(gpu, self.capacity);
        }
        gpu.queue
            .write_buffer(&self.vertex_buffer, 0, bytemuck::cast_slice(&self.vertices));
    }

    /// Draws `surface` over the quad recorded at `slot` by `push`.
    pub fn draw<'pass>(
        &'pass self,
        pass: &mut wgpu::RenderPass<'pass>,
        surface: &'pass Surface,
        slot: usize,
        blend: Blend,
    ) {
        let first = (slot * VERTICES_PER_QUAD) as u32;
        if first as usize >= self.vertices.len() {
            return; // a slot from a frame whose `flush` never ran
        }
        pass.set_pipeline(match blend {
            Blend::Opaque => &self.opaque,
            Blend::Over => &self.blended,
        });
        pass.set_bind_group(0, &surface.bind_group, &[]);
        pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
        pass.draw(first..first + VERTICES_PER_QUAD as u32, 0..1);
    }
}

/// One screen-space rectangle on a `screen`-sized surface as two
/// triangles in NDC -- the same conversion `RectRenderer::push_rect` uses.
fn quad_vertices(
    screen: (f32, f32),
    dest: (f32, f32, f32, f32),
    uv: (f32, f32, f32, f32),
    recolor: Option<([f32; 3], [f32; 3])>,
) -> [Vertex; VERTICES_PER_QUAD] {
    let (dest_x, dest_y, dest_w, dest_h) = dest;
    let (u0, v0, u1, v1) = uv;
    let (paper, ink) = match recolor {
        Some((p, i)) => ([p[0], p[1], p[2], 1.0], [i[0], i[1], i[2], 0.0]),
        None => ([0.0; 4], [0.0; 4]),
    };
    let (sw, sh) = screen;
    let to_ndc = |px: f32, py: f32| [(px / sw) * 2.0 - 1.0, 1.0 - (py / sh) * 2.0];

    let p00 = to_ndc(dest_x, dest_y);
    let p10 = to_ndc(dest_x + dest_w, dest_y);
    let p01 = to_ndc(dest_x, dest_y + dest_h);
    let p11 = to_ndc(dest_x + dest_w, dest_y + dest_h);
    [
        Vertex {
            position: p00,
            uv: [u0, v0],
            paper,
            ink,
        },
        Vertex {
            position: p10,
            uv: [u1, v0],
            paper,
            ink,
        },
        Vertex {
            position: p01,
            uv: [u0, v1],
            paper,
            ink,
        },
        Vertex {
            position: p10,
            uv: [u1, v0],
            paper,
            ink,
        },
        Vertex {
            position: p11,
            uv: [u1, v1],
            paper,
            ink,
        },
        Vertex {
            position: p01,
            uv: [u0, v1],
            paper,
            ink,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_quad_maps_its_pixel_rect_to_ndc_and_its_uv_corners() {
        let v = quad_vertices(
            (200.0, 100.0),
            (0.0, 0.0, 100.0, 50.0),
            (0.25, 0.0, 0.75, 1.0),
            None,
        );
        assert_eq!(v[0].position, [-1.0, 1.0], "top-left");
        assert_eq!(
            v[4].position,
            [0.0, 0.0],
            "bottom-right, the screen's centre"
        );
        assert_eq!((v[0].uv, v[4].uv), ([0.25, 0.0], [0.75, 1.0]));
        assert!(
            v.iter().all(|v| v.paper[3] == 0.0),
            "no recolouring unless asked"
        );
    }

    #[test]
    fn recolouring_turns_on_with_its_paper_and_ink() {
        let v = quad_vertices(
            (10.0, 10.0),
            (0.0, 0.0, 10.0, 10.0),
            FULL,
            Some(([0.1, 0.2, 0.3], [0.9, 0.8, 0.7])),
        );
        assert!(v
            .iter()
            .all(|v| v.paper == [0.1, 0.2, 0.3, 1.0] && v.ink == [0.9, 0.8, 0.7, 0.0]));
    }
}
