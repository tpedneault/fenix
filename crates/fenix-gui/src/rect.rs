use crate::gpu::GpuState;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Vertex {
    position: [f32; 2],
    color: [f32; 4],
}

const MAX_RECTS: usize = 64;
const VERTICES_PER_RECT: usize = 6;

/// Draws small solid-color rectangles (caret, selection, current-line
/// highlight later) as simple two-triangle quads, independent of the text
/// pipeline.
pub struct RectRenderer {
    pipeline: wgpu::RenderPipeline,
    vertex_buffer: wgpu::Buffer,
    vertex_count: u32,
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

        let vertex_buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("rect-vertex-buffer"),
            size: (MAX_RECTS * VERTICES_PER_RECT * std::mem::size_of::<Vertex>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self { pipeline, vertex_buffer, vertex_count: 0 }
    }

    /// Replace this frame's rects with a single one, in pixel space
    /// (top-left x/y, width/height), color as straight-alpha RGBA in 0..1.
    pub fn set_rect(&mut self, gpu: &GpuState, x: f32, y: f32, w: f32, h: f32, color: [f32; 4]) {
        let sw = gpu.config.width as f32;
        let sh = gpu.config.height as f32;
        let to_ndc = |px: f32, py: f32| [(px / sw) * 2.0 - 1.0, 1.0 - (py / sh) * 2.0];

        let p00 = to_ndc(x, y);
        let p10 = to_ndc(x + w, y);
        let p01 = to_ndc(x, y + h);
        let p11 = to_ndc(x + w, y + h);

        let verts = [
            Vertex { position: p00, color },
            Vertex { position: p10, color },
            Vertex { position: p01, color },
            Vertex { position: p10, color },
            Vertex { position: p11, color },
            Vertex { position: p01, color },
        ];

        gpu.queue.write_buffer(&self.vertex_buffer, 0, bytemuck::cast_slice(&verts));
        self.vertex_count = verts.len() as u32;
    }

    pub fn clear(&mut self) {
        self.vertex_count = 0;
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
