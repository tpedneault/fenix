// Every rect Fenix draws. A plain one (no radius, border or blur) is
// its colour, exactly as before; the rest are shaped per pixel from a
// rounded box's signed distance -- negative inside, zero on the edge --
// so corners stay smooth at any scale and a shadow is the same box
// with its edge spread over `blur` pixels.

struct VertexInput {
    @location(0) position: vec2<f32>,
    @location(1) color: vec4<f32>,
    // This vertex's offset from the box's centre, in pixels.
    @location(2) local: vec2<f32>,
    // Half the box's size, in pixels.
    @location(3) half_size: vec2<f32>,
    // radius, border width, blur, unused.
    @location(4) params: vec4<f32>,
    @location(5) border_color: vec4<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) local: vec2<f32>,
    @location(2) half_size: vec2<f32>,
    @location(3) params: vec4<f32>,
    @location(4) border_color: vec4<f32>,
};

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.clip_position = vec4<f32>(in.position, 0.0, 1.0);
    out.color = in.color;
    out.local = in.local;
    out.half_size = in.half_size;
    out.params = in.params;
    out.border_color = in.border_color;
    return out;
}

fn rounded_box(p: vec2<f32>, half_size: vec2<f32>, radius: f32) -> f32 {
    let r = min(radius, min(half_size.x, half_size.y));
    let q = abs(p) - half_size + vec2<f32>(r, r);
    return length(max(q, vec2<f32>(0.0, 0.0))) + min(max(q.x, q.y), 0.0) - r;
}

// A rounded box with its own radius at each corner: top-left,
// top-right, bottom-right, bottom-left.
fn rounded_box4(p: vec2<f32>, half_size: vec2<f32>, radii: vec4<f32>) -> f32 {
    var r = radii.w;
    if (p.x > 0.0 && p.y < 0.0) { r = radii.y; }
    if (p.x > 0.0 && p.y >= 0.0) { r = radii.z; }
    if (p.x <= 0.0 && p.y < 0.0) { r = radii.x; }
    r = min(r, min(half_size.x, half_size.y));
    let q = abs(p) - half_size + vec2<f32>(r, r);
    return length(max(q, vec2<f32>(0.0, 0.0))) + min(max(q.x, q.y), 0.0) - r;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let radius = in.params.x;
    let border = in.params.y;
    let blur = in.params.z;
    let mode = in.params.w;
    if (mode > 1.5) {
        // A wavy underline across the box: period, amplitude and
        // thickness in `border_color`.
        let period = max(in.border_color.x, 1.0);
        let amplitude = in.border_color.y;
        let thickness = max(in.border_color.z, 0.5);
        let wave = amplitude * sin((in.local.x + in.half_size.x) / period * 6.2831853);
        let d = abs(in.local.y - wave) - thickness * 0.5;
        return vec4<f32>(in.color.rgb, in.color.a * clamp(0.5 - d, 0.0, 1.0));
    }
    if (mode > 0.5) {
        let d = rounded_box4(in.local, in.half_size, in.border_color);
        return vec4<f32>(in.color.rgb, in.color.a * clamp(0.5 - d, 0.0, 1.0));
    }
    if (radius <= 0.0 && border <= 0.0 && blur <= 0.0) {
        return in.color;
    }
    let d = rounded_box(in.local, in.half_size, radius);
    if (blur > 0.0) {
        // A shadow: full strength well inside, nothing `blur` outside.
        let a = 1.0 - smoothstep(-blur, blur, d);
        return vec4<f32>(in.color.rgb, in.color.a * a);
    }
    let coverage = clamp(0.5 - d, 0.0, 1.0);
    var color = in.color;
    if (border > 0.0) {
        // Inside the border's inner edge, the fill; across it, blend.
        let inner = clamp(0.5 - (d + border), 0.0, 1.0);
        color = mix(in.border_color, in.color, inner);
    }
    return vec4<f32>(color.rgb, color.a * coverage);
}
