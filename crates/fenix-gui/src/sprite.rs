//! Images drawn as quads, each with its own opacity and tint: the
//! launch splash's blades and wordmark, the modeline's spinner, and the
//! old frame a theme change fades out of. Built from a device and a
//! target size rather than a `GpuState`, so the splash can draw with it
//! from its own thread.

use std::sync::Arc;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Vertex {
    position: [f32; 2],
    uv: [f32; 2],
    /// Multiplies the texel: white keeps the image's own colours, and a
    /// 1x1 white image tinted is a plain rect.
    tint: [f32; 4],
}

/// One image on the GPU, ready to draw.
pub struct Sprite {
    pub width: u32,
    pub height: u32,
    bind_group: wgpu::BindGroup,
}

pub struct SpriteRenderer {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    vertex_buffer: wgpu::Buffer,
    capacity: usize,
    vertices: Vec<Vertex>,
    /// Which sprite each run of six vertices draws.
    draws: Vec<wgpu::BindGroup>,
    size: (f32, f32),
}

impl SpriteRenderer {
    pub fn new(device: Arc<wgpu::Device>, queue: Arc<wgpu::Queue>, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("sprite-shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("sprite-bind-group-layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        multisampled: false,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
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
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("sprite-pipeline-layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("sprite-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Vertex>() as wgpu::BufferAddress,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Float32x4],
                })],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
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
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("sprite-sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let capacity = 64 * 6;
        let vertex_buffer = Self::buffer(&device, capacity);
        SpriteRenderer { device, queue, pipeline, layout, sampler, vertex_buffer, capacity, vertices: Vec::new(), draws: Vec::new(), size: (1.0, 1.0) }
    }

    fn buffer(device: &wgpu::Device, vertices: usize) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sprite-vertex-buffer"),
            size: (vertices * std::mem::size_of::<Vertex>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    /// An image (straight-alpha RGBA) as a sprite.
    pub fn upload(&self, width: u32, height: u32, rgba: &[u8]) -> Sprite {
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("sprite-texture"),
            size: wgpu::Extent3d { width: width.max(1), height: height.max(1), depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        if width > 0 && height > 0 {
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo { texture: &texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                rgba,
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4 * width), rows_per_image: Some(height) },
                wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            );
        }
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Sprite { width, height, bind_group: self.bind(&view) }
    }

    /// A 1x1 white sprite: tinted, a plain rect.
    pub fn white(&self) -> Sprite {
        self.upload(1, 1, &[255, 255, 255, 255])
    }

    /// Any texture view as a sprite -- the copied frame a theme change
    /// fades out of.
    pub fn sprite_of_view(&self, view: &wgpu::TextureView, width: u32, height: u32) -> Sprite {
        Sprite { width, height, bind_group: self.bind(view) }
    }

    fn bind(&self, view: &wgpu::TextureView) -> wgpu::BindGroup {
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sprite-bind-group"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
            ],
        })
    }

    /// Starts a frame drawn onto a target `width` x `height` pixels.
    pub fn begin(&mut self, width: u32, height: u32) {
        self.vertices.clear();
        self.draws.clear();
        self.size = (width.max(1) as f32, height.max(1) as f32);
    }

    /// Queues `sprite` over the pixel rect `(x, y, w, h)`, its colours
    /// multiplied by `tint` (opacity is `tint[3]`).
    pub fn push(&mut self, sprite: &Sprite, x: f32, y: f32, w: f32, h: f32, tint: [f32; 4]) {
        if tint[3] <= 0.0 || w <= 0.0 || h <= 0.0 {
            return;
        }
        let (sw, sh) = self.size;
        let ndc = |px: f32, py: f32| [(px / sw) * 2.0 - 1.0, 1.0 - (py / sh) * 2.0];
        let v = |px: f32, py: f32, u: f32, t: f32| Vertex { position: ndc(px, py), uv: [u, t], tint };
        let (a, b, c, d) = (v(x, y, 0.0, 0.0), v(x + w, y, 1.0, 0.0), v(x, y + h, 0.0, 1.0), v(x + w, y + h, 1.0, 1.0));
        self.vertices.extend_from_slice(&[a, b, c, b, d, c]);
        self.draws.push(sprite.bind_group.clone());
    }

    /// Uploads the frame's quads; call before the render pass opens.
    pub fn flush(&mut self) {
        if self.vertices.len() > self.capacity {
            self.capacity = self.vertices.len().next_power_of_two();
            self.vertex_buffer = Self::buffer(&self.device, self.capacity);
        }
        if !self.vertices.is_empty() {
            self.queue.write_buffer(&self.vertex_buffer, 0, bytemuck::cast_slice(&self.vertices));
        }
    }

    pub fn render<'pass>(&'pass self, pass: &mut wgpu::RenderPass<'pass>) {
        if self.draws.is_empty() {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
        for (i, bind_group) in self.draws.iter().enumerate() {
            pass.set_bind_group(0, bind_group, &[]);
            let first = (i * 6) as u32;
            pass.draw(first..first + 6, 0..1);
        }
    }
}

const SHADER: &str = r#"
struct VertexInput {
    @location(0) position: vec2<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) tint: vec4<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) tint: vec4<f32>,
};

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.clip_position = vec4<f32>(in.position, 0.0, 1.0);
    out.uv = in.uv;
    out.tint = in.tint;
    return out;
}

@group(0) @binding(0) var t_image: texture_2d<f32>;
@group(0) @binding(1) var s_image: sampler;

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    return textureSample(t_image, s_image, in.uv) * in.tint;
}
"#;
