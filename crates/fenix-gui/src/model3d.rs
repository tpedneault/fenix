//! Extensions' 3D models, drawn for their layers: each model item gets a
//! small target of its own, and is drawn into it toon-shaded -- three
//! bands of light, a rim in the theme's accent, shadows tinted from the
//! theme's background -- with an outline (its back faces pushed out a
//! pixel, in a darker shade of its own colour). The target is then a
//! sprite like any sheet's frame, so a layer's opacity, keep-clear and
//! clicks work the same for both.
//!
//! `Style::Pixel` draws into a target a fraction the size, scaled up
//! nearest-pixel, so a model sits with pixel art; `Style::Smooth` draws
//! at the screen's own size, multisampled.
//!
//! A target is drawn again only when what it shows changes: the pose's
//! clips and times, its turn, its size, or the light.

use std::collections::HashMap;
use std::sync::{Arc, Weak};

use fenix_model::Model;
use glam::{Mat4, Quat, Vec3};

use crate::sprite::{Sprite, SpriteRenderer};

const DEPTH: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
const COLOR: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const SAMPLES: u32 = 4;
/// How far above the model the camera looks down from, in degrees.
const ELEVATION: f32 = 16.0;

/// The theme's light, as the models are lit by it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Lighting {
    /// The lit side's colour (near white, warmed by the text colour).
    pub light: [f32; 3],
    /// What the shadowed side is multiplied by.
    pub shadow: [f32; 3],
    /// The rim's colour: the theme's accent.
    pub rim: [f32; 3],
}

impl Lighting {
    /// From a theme's background, text and accent colours.
    pub fn from_theme(bg: [f32; 4], fg: [f32; 3], accent: [f32; 4]) -> Lighting {
        let mix = |a: f32, b: f32, t: f32| a + (b - a) * t;
        let dark = (bg[0] + bg[1] + bg[2]) / 3.0 < 0.5;
        // Shadows lean cool, towards the background on a dark theme.
        let cool = [0.52, 0.55, 0.72];
        let shadow = if dark { [mix(cool[0], bg[0] * 2.0, 0.25), mix(cool[1], bg[1] * 2.0, 0.25), mix(cool[2], bg[2] * 2.0, 0.25)] } else { [mix(cool[0], bg[0], 0.15), mix(cool[1], bg[1], 0.15), mix(cool[2], bg[2], 0.15)] };
        Lighting { light: [mix(1.0, fg[0], 0.12), mix(1.0, fg[1], 0.12), mix(1.0, fg[2], 0.12)], shadow, rim: [accent[0], accent[1], accent[2]] }
    }
}

/// How one model item is drawn this frame.
#[derive(Debug, Clone, PartialEq)]
pub struct Shot {
    /// How tall the model stands, in screen pixels.
    pub height: f32,
    /// Turned about its up axis, in degrees.
    pub yaw: f32,
    pub pixel: bool,
    /// A screen pixel a line's height: what `pixel`'s scale follows.
    pub line: f32,
    pub tint: [f32; 3],
    pub glow: f32,
    pub lighting: Lighting,
}

/// Where a drawn target goes: its sprite, its size on screen, and where
/// in it the model's feet are, all in screen pixels.
pub struct Placed<'a> {
    pub sprite: &'a Sprite,
    pub size: (f32, f32),
    pub feet: (f32, f32),
    /// The model's own box in the target: left, top, width, height.
    pub body: (f32, f32, f32, f32),
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Globals {
    view_proj: [[f32; 4]; 4],
    model: [[f32; 4]; 4],
    light_dir: [f32; 4],
    view_dir: [f32; 4],
    /// rgb, and the outline's width across (clip units).
    light: [f32; 4],
    /// rgb, and the outline's width down.
    shadow: [f32; 4],
    /// rgb, and the glow.
    rim: [f32; 4],
    tint: [f32; 4],
}

/// A model's parts on the GPU: what doesn't move.
struct GpuModel {
    model: Weak<Model>,
    parts: Vec<GpuPart>,
}

struct GpuPart {
    indices: wgpu::Buffer,
    count: u32,
    /// uv and colour, a vertex.
    fixed: wgpu::Buffer,
    texture: wgpu::BindGroup,
}

/// One item's target and what it was last drawn with.
struct Target {
    size: (u32, u32),
    samples: u32,
    /// What `view` shows: kept so it lives, and read back in tests.
    #[cfg_attr(not(test), allow(dead_code))]
    color: wgpu::Texture,
    view: wgpu::TextureView,
    msaa: Option<wgpu::TextureView>,
    depth: wgpu::TextureView,
    globals: wgpu::Buffer,
    globals_bind: wgpu::BindGroup,
    /// Where the posed vertices go, a buffer a part.
    posed: Vec<wgpu::Buffer>,
    sprite: Option<Sprite>,
    drawn: Option<Drawn>,
    /// The feet and the body's box, in target pixels.
    feet: (f32, f32),
    body: (f32, f32, f32, f32),
    /// Screen pixels a target pixel.
    scale: f32,
    used: bool,
}

/// What a target shows: drawn again only when this changes.
#[derive(Debug, Clone, PartialEq)]
struct Drawn {
    model: usize,
    pose: Vec<(usize, f32, f32)>,
    parts: Vec<String>,
    shot: Shot,
}

pub struct ModelRenderer {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    /// Toon and outline, single-sampled and multisampled.
    toon: [wgpu::RenderPipeline; 2],
    outline: [wgpu::RenderPipeline; 2],
    globals_layout: wgpu::BindGroupLayout,
    texture_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    white: wgpu::BindGroup,
    models: HashMap<usize, GpuModel>,
    targets: HashMap<(String, u64, u32), Target>,
    encoder: Option<wgpu::CommandEncoder>,
}

impl ModelRenderer {
    pub fn new(device: Arc<wgpu::Device>, queue: Arc<wgpu::Queue>) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("model-shader"), source: wgpu::ShaderSource::Wgsl(SHADER.into()) });
        let globals_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("model-globals-layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                count: None,
            }],
        });
        let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("model-texture-layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { multisampled: false, view_dimension: wgpu::TextureViewDimension::D2, sample_type: wgpu::TextureSampleType::Float { filterable: true } },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("model-pipeline-layout"), bind_group_layouts: &[Some(&globals_layout), Some(&texture_layout)], immediate_size: 0 });
        let pipeline = |vs: &str, fs: &str, cull: wgpu::Face, samples: u32| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("model-pipeline"),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some(vs),
                    buffers: &[
                        Some(wgpu::VertexBufferLayout { array_stride: 24, step_mode: wgpu::VertexStepMode::Vertex, attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3] }),
                        Some(wgpu::VertexBufferLayout { array_stride: 24, step_mode: wgpu::VertexStepMode::Vertex, attributes: &wgpu::vertex_attr_array![2 => Float32x2, 3 => Float32x4] }),
                    ],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(fs),
                    targets: &[Some(wgpu::ColorTargetState { format: COLOR, blend: None, write_mask: wgpu::ColorWrites::ALL })],
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState { cull_mode: Some(cull), front_face: wgpu::FrontFace::Ccw, ..Default::default() },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH,
                    depth_write_enabled: Some(true),
                    depth_compare: Some(wgpu::CompareFunction::LessEqual),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: wgpu::MultisampleState { count: samples, ..Default::default() },
                multiview_mask: None,
                cache: None,
            })
        };
        let toon = [pipeline("vs_toon", "fs_toon", wgpu::Face::Back, 1), pipeline("vs_toon", "fs_toon", wgpu::Face::Back, SAMPLES)];
        let outline = [pipeline("vs_outline", "fs_outline", wgpu::Face::Front, 1), pipeline("vs_outline", "fs_outline", wgpu::Face::Front, SAMPLES)];
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("model-sampler"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            ..Default::default()
        });
        let white = texture(&device, &queue, &texture_layout, &sampler, 1, 1, &[255, 255, 255, 255]);
        ModelRenderer {
            white,
            device,
            queue,
            toon,
            outline,
            globals_layout,
            texture_layout,
            sampler,
            models: HashMap::new(),
            targets: HashMap::new(),
            encoder: None,
        }
    }

    fn texture(&self, width: u32, height: u32, rgba: &[u8]) -> wgpu::BindGroup {
        texture(&self.device, &self.queue, &self.texture_layout, &self.sampler, width, height, rgba)
    }

    fn buffer(&self, label: &str, bytes: &[u8], usage: wgpu::BufferUsages) -> wgpu::Buffer {
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor { label: Some(label), size: (bytes.len() as u64).max(4).next_multiple_of(4), usage: usage | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let mut padded = bytes.to_vec();
        padded.resize(buffer.size() as usize, 0);
        self.queue.write_buffer(&buffer, 0, &padded);
        buffer
    }

    /// `model`'s fixed parts on the GPU, uploaded the first time.
    fn upload(&mut self, model: &Arc<Model>) -> usize {
        let key = Arc::as_ptr(model) as usize;
        if self.models.get(&key).is_some_and(|m| m.model.upgrade().is_some()) {
            return key;
        }
        let textures: Vec<wgpu::BindGroup> = model.textures.iter().map(|t| self.texture(t.width, t.height, &t.rgba)).collect();
        let parts = model
            .parts
            .iter()
            .map(|part| {
                let fixed: Vec<f32> = part.uvs.iter().zip(&part.colors).flat_map(|(uv, c)| [uv[0], uv[1], c[0], c[1], c[2], c[3]]).collect();
                GpuPart {
                    indices: self.buffer("model-indices", bytemuck::cast_slice(&part.indices), wgpu::BufferUsages::INDEX),
                    count: part.indices.len() as u32,
                    fixed: self.buffer("model-fixed", bytemuck::cast_slice(&fixed), wgpu::BufferUsages::VERTEX),
                    texture: part.texture.and_then(|t| textures.get(t).cloned()).unwrap_or_else(|| self.white.clone()),
                }
            })
            .collect();
        self.models.insert(key, GpuModel { model: Arc::downgrade(model), parts });
        key
    }

    /// Starts a frame: targets not drawn this frame are let go after it.
    pub fn begin(&mut self) {
        for target in self.targets.values_mut() {
            target.used = false;
        }
    }

    /// Draws `model` posed as `pose` into item `key`'s target, unless it
    /// shows that already, and says where it goes.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(&mut self, key: (String, u64, u32), model: &Arc<Model>, playing: &[fenix_model::Playing], parts: &[String], shot: &Shot, sprites: &SpriteRenderer) -> Option<Placed<'_>> {
        let id = self.upload(model);
        let (least, most) = model.bounds;
        let height = (most.y - least.y).max(1e-4);
        // Screen pixels a target pixel, and model units a target pixel.
        let scale = if shot.pixel { (shot.line / 11.0).round().max(2.0) } else { 1.0 };
        let unit = height / (shot.height / scale).max(1.0);
        // Room for a stride, a hop and a turn: wide as the model is round
        // its middle, twice; tall as it is and half again.
        let center = Vec3::new((least.x + most.x) / 2.0, least.y, (least.z + most.z) / 2.0);
        let reach = [least.x - center.x, most.x - center.x].iter().map(|x| x.abs()).fold(0.0, f32::max).hypot([least.z - center.z, most.z - center.z].iter().map(|z| z.abs()).fold(0.0, f32::max));
        let width_u = (reach * 2.0 * 1.25).max(height * 0.8);
        let below_u = height * 0.12;
        let height_u = height * 1.55;
        let size = (((width_u / unit).ceil() as u32).clamp(4, 1024), ((height_u / unit).ceil() as u32).clamp(4, 1024));
        let samples = if shot.pixel { 1 } else { SAMPLES };

        let drawn = Drawn { model: id, pose: playing.iter().map(|p| (p.clip, p.seconds, p.weight)).collect(), parts: parts.to_vec(), shot: shot.clone() };
        let fresh = self.targets.get(&key).is_none_or(|t| t.size != size || t.samples != samples);
        if fresh {
            let target = self.target(size, samples);
            self.targets.insert(key.clone(), target);
        }
        self.prepare(&key, model);
        let needs = self.targets.get(&key).is_some_and(|t| t.drawn.as_ref() != Some(&drawn));
        if needs {
            let wear = |name: &str| parts.iter().any(|p| p == name);
            let pose = model.pose(playing, &wear);
            // Model units across, and from the feet down and up.
            let (w, h) = (size.0 as f32 * unit, size.1 as f32 * unit);
            let elevation = ELEVATION.to_radians();
            let toward = Vec3::new(0.0, elevation.sin(), elevation.cos());
            let distance = (width_u + height_u) * 4.0;
            let view = Mat4::look_at_rh(center + toward * distance, center, Vec3::Y);
            let proj = Mat4::orthographic_rh(-w / 2.0, w / 2.0, -below_u, h - below_u, 0.01, distance * 2.0);
            let turn = Mat4::from_translation(center) * Mat4::from_quat(Quat::from_rotation_y(shot.yaw.to_radians())) * Mat4::from_translation(-center);
            let outline_px = if shot.pixel { 1.0 } else { (shot.line / 14.0).max(1.0) };
            let globals = Globals {
                view_proj: (proj * view).to_cols_array_2d(),
                model: turn.to_cols_array_2d(),
                light_dir: Vec3::new(-0.45, 0.85, 0.55).normalize().extend(0.0).to_array(),
                view_dir: toward.extend(0.0).to_array(),
                light: [shot.lighting.light[0], shot.lighting.light[1], shot.lighting.light[2], 2.0 * outline_px / size.0 as f32],
                shadow: [shot.lighting.shadow[0], shot.lighting.shadow[1], shot.lighting.shadow[2], 2.0 * outline_px / size.1 as f32],
                rim: [shot.lighting.rim[0], shot.lighting.rim[1], shot.lighting.rim[2], shot.glow.clamp(0.0, 1.0)],
                tint: [shot.tint[0], shot.tint[1], shot.tint[2], 1.0],
            };
            let gpu_model = self.models.get(&id)?;
            let target = self.targets.get_mut(&key)?;
            self.queue.write_buffer(&target.globals, 0, bytemuck::bytes_of(&globals));
            for (buffer, part) in target.posed.iter().zip(&pose.parts) {
                let data: Vec<f32> = part.positions.iter().zip(&part.normals).flat_map(|(p, n)| [p[0], p[1], p[2], n[0], n[1], n[2]]).collect();
                if !data.is_empty() {
                    self.queue.write_buffer(buffer, 0, bytemuck::cast_slice(&data));
                }
            }
            let encoder = self.encoder.get_or_insert_with(|| self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("model-encoder") }));
            {
                let (view, resolve) = match &target.msaa {
                    Some(msaa) => (msaa, Some(&target.view)),
                    None => (&target.view, None),
                };
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("model-pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment { view, resolve_target: resolve, ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store }, depth_slice: None })],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment { view: &target.depth, depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Discard }), stencil_ops: None }),
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                let which = usize::from(samples > 1);
                pass.set_bind_group(0, &target.globals_bind, &[]);
                for pipeline in [&self.toon[which], &self.outline[which]] {
                    pass.set_pipeline(pipeline);
                    for ((gpu, posed), shown) in gpu_model.parts.iter().zip(&target.posed).zip(pose.parts.iter().map(|p| p.shown)) {
                        if !shown || gpu.count == 0 {
                            continue;
                        }
                        pass.set_bind_group(1, &gpu.texture, &[]);
                        pass.set_vertex_buffer(0, posed.slice(..));
                        pass.set_vertex_buffer(1, gpu.fixed.slice(..));
                        pass.set_index_buffer(gpu.indices.slice(..), wgpu::IndexFormat::Uint32);
                        pass.draw_indexed(0..gpu.count, 0, 0..1);
                    }
                }
            }
            target.drawn = Some(drawn);
            target.feet = (size.0 as f32 / 2.0, size.1 as f32 - below_u / unit);
            let body_w = ((most.x - least.x).max(most.z - least.z) / unit).min(size.0 as f32);
            let body_h = height / unit;
            target.body = ((size.0 as f32 - body_w) / 2.0, target.feet.1 - body_h, body_w, body_h);
            target.scale = scale;
        }
        let target = self.targets.get_mut(&key)?;
        target.used = true;
        if target.sprite.is_none() {
            target.sprite = Some(sprites.sprite_of_view(&target.view, size.0, size.1));
        }
        let s = target.scale;
        Some(Placed {
            sprite: target.sprite.as_ref()?,
            size: (size.0 as f32 * s, size.1 as f32 * s),
            feet: (target.feet.0 * s, target.feet.1 * s),
            body: (target.body.0 * s, target.body.1 * s, target.body.2 * s, target.body.3 * s),
        })
    }

    fn target(&self, size: (u32, u32), samples: u32) -> Target {
        let texture = |format, samples, usage| {
            self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("model-target"),
                size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: samples,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
        };
        let color = texture(COLOR, 1, wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC);
        let view = color.create_view(&wgpu::TextureViewDescriptor::default());
        let msaa = (samples > 1).then(|| texture(COLOR, samples, wgpu::TextureUsages::RENDER_ATTACHMENT).create_view(&wgpu::TextureViewDescriptor::default()));
        let depth = texture(DEPTH, samples, wgpu::TextureUsages::RENDER_ATTACHMENT).create_view(&wgpu::TextureViewDescriptor::default());
        let globals = self.device.create_buffer(&wgpu::BufferDescriptor { label: Some("model-globals"), size: std::mem::size_of::<Globals>() as u64, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let globals_bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("model-globals-bind"), layout: &self.globals_layout, entries: &[wgpu::BindGroupEntry { binding: 0, resource: globals.as_entire_binding() }] });
        Target { size, samples, color, view, msaa, depth, globals, globals_bind, posed: Vec::new(), sprite: None, drawn: None, feet: (0.0, 0.0), body: (0.0, 0.0, 0.0, 0.0), scale: 1.0, used: false }
    }

    /// The posed-vertex buffers `key`'s target needs for `model`, made
    /// when it hasn't them.
    fn prepare(&mut self, key: &(String, u64, u32), model: &Model) {
        let Some(target) = self.targets.get(key) else { return };
        if target.posed.len() == model.parts.len() && target.posed.iter().zip(&model.parts).all(|(b, p)| b.size() >= (p.vertices() * 24) as u64) {
            return;
        }
        let posed: Vec<wgpu::Buffer> = model
            .parts
            .iter()
            .map(|p| self.device.create_buffer(&wgpu::BufferDescriptor { label: Some("model-posed"), size: ((p.vertices() * 24) as u64).max(24), usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false }))
            .collect();
        if let Some(target) = self.targets.get_mut(key) {
            target.posed = posed;
            target.drawn = None;
        }
    }

    /// Sends what was drawn this frame to the GPU, ahead of the frame's
    /// own passes, and lets go of the targets and models no longer drawn.
    pub fn finish(&mut self) {
        if let Some(encoder) = self.encoder.take() {
            self.queue.submit([encoder.finish()]);
        }
        self.targets.retain(|_, t| t.used);
        self.models.retain(|_, m| m.model.upgrade().is_some());
    }
}

const SHADER: &str = r#"
struct Globals {
    view_proj: mat4x4<f32>,
    model: mat4x4<f32>,
    light_dir: vec4<f32>,
    view_dir: vec4<f32>,
    light: vec4<f32>,
    shadow: vec4<f32>,
    rim: vec4<f32>,
    tint: vec4<f32>,
};

@group(0) @binding(0) var<uniform> g: Globals;
@group(1) @binding(0) var t_base: texture_2d<f32>;
@group(1) @binding(1) var s_base: sampler;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) color: vec4<f32>,
};

struct VertexOutput {
    @builtin(position) clip: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<f32>,
};

fn place(v: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.clip = g.view_proj * g.model * vec4<f32>(v.position, 1.0);
    out.normal = normalize((g.model * vec4<f32>(v.normal, 0.0)).xyz);
    out.uv = v.uv;
    out.color = v.color;
    return out;
}

@vertex
fn vs_toon(v: VertexInput) -> VertexOutput {
    return place(v);
}

// The outline: back faces pushed out along their normals, a fixed
// number of pixels on screen whatever the model's size.
@vertex
fn vs_outline(v: VertexInput) -> VertexOutput {
    var out = place(v);
    let n = (g.view_proj * vec4<f32>(out.normal, 0.0)).xy;
    let len = length(n);
    if (len > 0.00001) {
        let push = n / len * vec2<f32>(g.light.w, g.shadow.w) * out.clip.w;
        out.clip = vec4<f32>(out.clip.xy + push, out.clip.z + 0.0005 * out.clip.w, out.clip.w);
    }
    return out;
}

fn base(in: VertexOutput) -> vec4<f32> {
    return textureSample(t_base, s_base, in.uv) * in.color * vec4<f32>(g.tint.rgb, 1.0);
}

@fragment
fn fs_toon(in: VertexOutput) -> @location(0) vec4<f32> {
    let b = base(in);
    if (b.a < 0.5) {
        discard;
    }
    let n = normalize(in.normal);
    let d = dot(n, normalize(g.light_dir.xyz));
    let lit = b.rgb * g.light.rgb;
    let dark = b.rgb * g.shadow.rgb;
    var color = dark;
    if (d > 0.45) {
        color = lit;
    } else if (d > -0.05) {
        color = mix(dark, lit, 0.55);
    }
    // A hard rim of the accent where the surface turns away at its
    // sides (not its top, which a camera this low sees edge-on too).
    let facing = max(dot(n, normalize(g.view_dir.xyz)), 0.0);
    if (facing < 0.25 && abs(n.y) < 0.55 && d > -0.2) {
        color = mix(color, g.rim.rgb, 0.35);
    }
    let glow = g.rim.a;
    color = mix(color, min(b.rgb * 1.25 + vec3<f32>(0.08), vec3<f32>(1.0)), glow);
    return vec4<f32>(color, 1.0);
}

@fragment
fn fs_outline(in: VertexOutput) -> @location(0) vec4<f32> {
    let b = base(in);
    if (b.a < 0.5) {
        discard;
    }
    return vec4<f32>(b.rgb * 0.28 * (1.0 - g.rim.a) + b.rgb * 0.6 * g.rim.a, 1.0);
}
"#;

/// An image as a texture the models sample.
fn texture(device: &wgpu::Device, queue: &wgpu::Queue, layout: &wgpu::BindGroupLayout, sampler: &wgpu::Sampler, width: u32, height: u32, rgba: &[u8]) -> wgpu::BindGroup {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("model-texture"),
        size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo { texture: &texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
        rgba,
        wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4 * width), rows_per_image: Some(height) },
        wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
    );
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("model-texture-bind"),
        layout,
        entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) }, wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(sampler) }],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use fenix_model::build::{Builder, Mesh, Track};
    use fenix_model::Playing;

    /// Whatever adapter there is, software included; none, and there's
    /// nothing to draw with.
    fn device() -> Option<(Arc<wgpu::Device>, Arc<wgpu::Queue>)> {
        let instance = wgpu::Instance::default();
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions { power_preference: wgpu::PowerPreference::LowPower, compatible_surface: None, force_fallback_adapter: false, apply_limit_buckets: false })).ok()?;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).ok()?;
        Some((Arc::new(device), Arc::new(queue)))
    }

    /// A red post on a root joint, a hat over it, and a clip that sinks
    /// the post into the floor.
    fn post() -> Arc<Model> {
        let mut b = Builder::new();
        let root = b.joint("root", None, [0.0, 0.0, 0.0]);
        let hat = b.joint("opt:hat", Some(root), [0.0, 2.0, 0.0]);
        let mut mesh = Mesh::default();
        mesh.cuboid([0.0, 1.0, 0.0], [0.5, 1.0, 0.5], [0.9, 0.1, 0.1, 1.0], root);
        mesh.ellipsoid([0.0, 2.3, 0.0], [0.4, 0.3, 0.4], Quat::IDENTITY, [0.1, 0.2, 0.9, 1.0], hat, 6, 10);
        b.mesh(mesh);
        b.clip("sink", vec![Track::translation(root, vec![0.0, 1.0], vec![[0.0, 0.0, 0.0], [0.0, -1.0, 0.0]])]);
        Arc::new(Model::from_glb(&b.glb()).unwrap())
    }

    /// The target's pixels, straight-alpha RGBA.
    fn read(device: &wgpu::Device, queue: &wgpu::Queue, target: &Target) -> Vec<[u8; 4]> {
        let (w, h) = target.size;
        let row = (w * 4).next_multiple_of(256);
        let buffer = device.create_buffer(&wgpu::BufferDescriptor { label: None, size: (row * h) as u64, usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo { texture: &target.color, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::TexelCopyBufferInfo { buffer: &buffer, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: Some(h) } },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        queue.submit([encoder.finish()]);
        buffer.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        let data = buffer.slice(..).get_mapped_range().unwrap();
        let mut out = Vec::with_capacity((w * h) as usize);
        for y in 0..h {
            for x in 0..w {
                let i = (y * row + x * 4) as usize;
                out.push([data[i], data[i + 1], data[i + 2], data[i + 3]]);
            }
        }
        out
    }

    /// The rows a colour is drawn on, top to bottom.
    fn rows(pixels: &[[u8; 4]], width: u32, is: impl Fn([u8; 4]) -> bool) -> Option<(u32, u32)> {
        let rows: Vec<u32> = pixels.chunks(width as usize).enumerate().filter(|(_, r)| r.iter().any(|p| is(*p))).map(|(y, _)| y as u32).collect();
        Some((*rows.first()?, *rows.last()?))
    }

    /// With `FENIX_MODEL_SHOT` set to a folder, the picture's kept there
    /// to look at.
    fn shoot(pixels: &[[u8; 4]], size: (u32, u32), name: &str) {
        if let Ok(dir) = std::env::var("FENIX_MODEL_SHOT") {
            let flat: Vec<u8> = pixels.iter().flatten().copied().collect();
            image::RgbaImage::from_raw(size.0, size.1, flat).unwrap().save(std::path::Path::new(&dir).join(name)).unwrap();
        }
    }

    #[test]
    fn a_model_is_drawn_toon_shaded_into_its_target_standing_on_its_feet() {
        let Some((device, queue)) = device() else { return };
        let sprites = SpriteRenderer::with_filter(Arc::clone(&device), Arc::clone(&queue), wgpu::TextureFormat::Rgba8Unorm, wgpu::FilterMode::Nearest);
        let mut renderer = ModelRenderer::new(Arc::clone(&device), Arc::clone(&queue));
        let model = post();
        let lighting = Lighting::from_theme([0.1, 0.1, 0.12, 1.0], [0.9, 0.9, 0.9], [1.0, 0.5, 0.0, 1.0]);
        let shot = Shot { height: 120.0, yaw: 30.0, pixel: false, line: 20.0, tint: [1.0; 3], glow: 0.0, lighting };
        let key = ("x".to_string(), 1, 1);
        let rest = [Playing { clip: Playing::REST, seconds: 0.0, weight: 1.0 }];

        renderer.begin();
        let placed = renderer.draw(key.clone(), &model, &rest, &[], &shot, &sprites).unwrap();
        let (feet, size) = (placed.feet, placed.size);
        renderer.finish();
        let target = &renderer.targets[&key];
        let pixels = read(&device, &queue, target);
        let red = |p: [u8; 4]| p[3] == 255 && p[0] > 100 && p[1] < 60 && p[2] < 60;
        let blue = |p: [u8; 4]| p[3] == 255 && p[2] > 100 && p[0] < 60;
        let (top, bottom) = rows(&pixels, target.size.0, red).expect("the post is drawn");
        // Its feet where it says they are -- the middle of its base; seen
        // from a little above, its front corner a few pixels lower -- and
        // as tall as asked (a touch more, with the outline).
        assert!(bottom as f32 >= feet.1 && bottom as f32 - feet.1 <= 12.0, "feet at {feet:?}, post down to {bottom}");
        shoot(&pixels, target.size, "post-smooth.png");
        // 120 tall without its hat; seen from above, its top shows too.
        let tall = (bottom - top) as f32;
        assert!((118.0..150.0).contains(&tall), "the post is 120 tall: {tall}");
        assert!(rows(&pixels, target.size.0, blue).is_none(), "no hat unless it's worn");
        assert!(pixels.iter().any(|p| p[3] == 255 && p[0] < 80 && p[1] < 20 && p[0] > p[2]), "a dark red outline");
        assert!(pixels[0][3] == 0, "the rest stays clear");
        assert!(size.0 > 0.0 && size.1 >= 120.0);

        // Asked again unchanged, it isn't drawn again.
        renderer.begin();
        renderer.draw(key.clone(), &model, &rest, &[], &shot, &sprites).unwrap();
        assert!(renderer.encoder.is_none(), "nothing to draw");
        // Wearing its hat, and half sunk into the floor.
        let sunk = [Playing { clip: 0, seconds: 0.5, weight: 1.0 }];
        renderer.draw(key.clone(), &model, &sunk, &["hat".into()], &shot, &sprites).unwrap();
        renderer.finish();
        let target = &renderer.targets[&key];
        let pixels = read(&device, &queue, target);
        let (hat_top, hat_bottom) = rows(&pixels, target.size.0, blue).expect("the hat is worn");
        let (sunk_top, _) = rows(&pixels, target.size.0, red).unwrap();
        assert!(sunk_top > top + 20, "sunk: {sunk_top} below {top}");
        // On its top: above the post's top face, down into it.
        assert!(hat_top < sunk_top && hat_bottom > sunk_top && hat_bottom < sunk_top + 40, "hat {hat_top}..{hat_bottom}, post from {sunk_top}");
        shoot(&pixels, target.size, "post-hat.png");

        // Pixel style: a target a fraction the size, scaled up.
        renderer.begin();
        let placed = renderer.draw(key.clone(), &model, &rest, &[], &Shot { pixel: true, ..shot.clone() }, &sprites).unwrap();
        let shown = placed.size;
        renderer.finish();
        let target = &renderer.targets[&key];
        assert_eq!(target.samples, 1);
        assert!(shown.0 >= target.size.0 as f32 * 2.0, "{shown:?} from {:?}", target.size);
        shoot(&read(&device, &queue, target), target.size, "post-pixel.png");
        // Not drawn in a frame, its target is let go.
        renderer.begin();
        renderer.finish();
        assert!(renderer.targets.is_empty());
    }

    /// For whoever makes models: every clip of each `.glb` in the folder
    /// `FENIX_MODEL_PREVIEW` names, drawn as Fenix draws it, four moments a
    /// clip, written beside it as `<name>-preview.png`.
    /// `FENIX_MODEL_PREVIEW=models cargo test -p fenix-gui preview -- --ignored`
    #[test]
    #[ignore]
    fn preview() {
        let Ok(dir) = std::env::var("FENIX_MODEL_PREVIEW") else { return };
        let Some((device, queue)) = device() else { panic!("no adapter to draw with") };
        let sprites = SpriteRenderer::with_filter(Arc::clone(&device), Arc::clone(&queue), wgpu::TextureFormat::Rgba8Unorm, wgpu::FilterMode::Nearest);
        let lighting = Lighting::from_theme([0.12, 0.12, 0.14, 1.0], [0.9, 0.9, 0.88], [1.0, 0.55, 0.1, 1.0]);
        let style = std::env::var("FENIX_MODEL_STYLE").unwrap_or_default();
        let parts: Vec<String> = std::env::var("FENIX_MODEL_PARTS").map(|p| p.split(',').map(str::to_string).collect()).unwrap_or_default();
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|e| e != "glb") {
                continue;
            }
            let model = Arc::new(Model::from_glb(&std::fs::read(&path).unwrap()).unwrap());
            let mut renderer = ModelRenderer::new(Arc::clone(&device), Arc::clone(&queue));
            let shot = Shot { height: 150.0, yaw: 35.0, pixel: style == "pixel", line: 30.0, tint: [1.0; 3], glow: 0.0, lighting };
            let mut frames: Vec<Vec<image::RgbaImage>> = Vec::new();
            for (c, clip) in model.clips.iter().enumerate() {
                let mut row = Vec::new();
                for k in 0..4 {
                    let playing = [Playing { clip: c, seconds: clip.seconds * k as f32 / 4.0, weight: 1.0 }];
                    renderer.begin();
                    let key = ("p".to_string(), c as u64, k);
                    renderer.draw(key.clone(), &model, &playing, &parts, &shot, &sprites).unwrap();
                    renderer.finish();
                    let target = &renderer.targets[&key];
                    let mut img = image::RgbaImage::from_raw(target.size.0, target.size.1, read(&device, &queue, target).into_iter().flatten().collect()).unwrap();
                    if target.scale > 1.0 {
                        img = image::imageops::resize(&img, target.size.0 * target.scale as u32, target.size.1 * target.scale as u32, image::imageops::FilterType::Nearest);
                    }
                    row.push(img);
                }
                frames.push(row);
            }
            let (w, h) = frames.iter().flatten().fold((0, 0), |(w, h), i| (w.max(i.width()), h.max(i.height())));
            let mut sheet = image::RgbaImage::from_pixel(w * 4, h * frames.len() as u32, image::Rgba([34, 34, 40, 255]));
            for (y, row) in frames.iter().enumerate() {
                for (x, img) in row.iter().enumerate() {
                    image::imageops::overlay(&mut sheet, img, (x as u32 * w) as i64, (y as u32 * h) as i64);
                }
            }
            sheet.save(path.with_file_name(format!("{}-preview.png", path.file_stem().unwrap().to_string_lossy()))).unwrap();
            println!("{}: {}", path.display(), model.clips.iter().map(|c| c.name.as_str()).collect::<Vec<_>>().join(" "));
        }
    }
}
