struct VertexInput {
    @location(0) position: vec2<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) paper: vec4<f32>,
    @location(3) ink: vec4<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) paper: vec4<f32>,
    @location(2) ink: vec4<f32>,
};

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.clip_position = vec4<f32>(in.position, 0.0, 1.0);
    out.uv = in.uv;
    out.paper = in.paper;
    out.ink = in.ink;
    return out;
}

@group(0) @binding(0) var t_page: texture_2d<f32>;
@group(0) @binding(1) var s_page: sampler;

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let c = textureSample(t_page, s_page, in.uv);
    // `paper.w` on: the page in the theme's colours. Its lightness picks
    // between ink (black) and paper (white); what a colour had besides
    // lightness is added back, so it keeps its hue.
    if (in.paper.w > 0.5) {
        let light = dot(c.rgb, vec3<f32>(0.299, 0.587, 0.114));
        let base = mix(in.ink.rgb, in.paper.rgb, light);
        return vec4<f32>(clamp(base + (c.rgb - vec3<f32>(light)), vec3<f32>(0.0), vec3<f32>(1.0)), c.a);
    }
    return c;
}
