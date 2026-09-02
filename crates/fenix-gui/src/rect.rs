use crate::gpu::GpuState;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Vertex {
    position: [f32; 2],
    color: [f32; 4],
}

/// How many rects one renderer's vertex buffer is *initially* sized for
/// -- not a cap. `flush` grows the buffer whenever a frame queues more
/// than currently fit (see its own doc comment): this used to be a hard
/// limit with `push_rect` silently dropping everything past it, which a
/// long visual-block selection plus a busy `hlsearch` can genuinely
/// reach, and which showed up as highlights simply not being drawn for
/// no visible reason.
const INITIAL_RECTS: usize = 256;
const VERTICES_PER_RECT: usize = 6;

/// Draws small solid-color rectangles (caret, modeline bar, selection and
/// current-line highlight later) as simple two-triangle quads, independent
/// of the text pipeline. Rects are accumulated on the CPU side across a
/// frame via `push_rect`, then uploaded once in `flush`.
pub struct RectRenderer {
    pipeline: wgpu::RenderPipeline,
    vertex_buffer: wgpu::Buffer,
    vertices: Vec<Vertex>,
    vertex_count: u32,
    /// How many vertices `vertex_buffer` currently holds room for --
    /// grown by `flush` as needed, never shrunk (a frame that once
    /// needed many rects tends to again, and the buffer is small).
    capacity: usize,
}

impl RectRenderer {
    pub fn new(gpu: &GpuState) -> Self {
        let shader = gpu.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("rect-shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("rect.wgsl").into()),
        });

        let pipeline_layout =
            gpu.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("rect-pipeline-layout"),
                bind_group_layouts: &[],
                immediate_size: 0,
            });

        let pipeline = gpu.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("rect-pipeline"),
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
                            format: wgpu::VertexFormat::Float32x4,
                            offset: 8,
                            shader_location: 1,
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
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
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

        let vertex_buffer = Self::new_buffer(gpu, INITIAL_RECTS * VERTICES_PER_RECT);

        Self { pipeline, vertex_buffer, vertices: Vec::new(), vertex_count: 0, capacity: INITIAL_RECTS * VERTICES_PER_RECT }
    }

    /// Start a new frame's worth of rects.
    pub fn clear(&mut self) {
        self.vertices.clear();
    }

    /// Queue one rect in pixel space (top-left x/y, width/height), color as
    /// straight-alpha RGBA in 0..1.
    pub fn push_rect(&mut self, gpu: &GpuState, x: f32, y: f32, w: f32, h: f32, color: [f32; 4]) {
        let sw = gpu.config.width as f32;
        let sh = gpu.config.height as f32;
        let to_ndc = |px: f32, py: f32| [(px / sw) * 2.0 - 1.0, 1.0 - (py / sh) * 2.0];

        let p00 = to_ndc(x, y);
        let p10 = to_ndc(x + w, y);
        let p01 = to_ndc(x, y + h);
        let p11 = to_ndc(x + w, y + h);

        self.vertices.extend_from_slice(&[
            Vertex { position: p00, color },
            Vertex { position: p10, color },
            Vertex { position: p01, color },
            Vertex { position: p10, color },
            Vertex { position: p11, color },
            Vertex { position: p01, color },
        ]);
    }

    fn new_buffer(gpu: &GpuState, vertices: usize) -> wgpu::Buffer {
        gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("rect-vertex-buffer"),
            size: (vertices * std::mem::size_of::<Vertex>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    /// Uploads this frame's queued rects, growing the vertex buffer first
    /// if this frame queued more than it currently holds. Call after all
    /// `push_rect` calls, before `render` -- a grow replaces the buffer,
    /// so anything already recorded against the old one would keep
    /// reading the old contents.
    pub fn flush(&mut self, gpu: &GpuState) {
        if self.vertices.len() > self.capacity {
            self.capacity = self.vertices.len().next_power_of_two();
            self.vertex_buffer = Self::new_buffer(gpu, self.capacity);
        }
        if !self.vertices.is_empty() {
            gpu.queue.write_buffer(&self.vertex_buffer, 0, bytemuck::cast_slice(&self.vertices));
        }
        self.vertex_count = self.vertices.len() as u32;
    }

    pub fn render<'pass>(&'pass self, pass: &mut wgpu::RenderPass<'pass>) {
        if self.vertex_count == 0 {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
        pass.draw(0..self.vertex_count, 0..1);
    }
}
