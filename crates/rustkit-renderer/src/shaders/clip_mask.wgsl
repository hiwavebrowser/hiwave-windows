// `clip-path` outline composite (C8b): lay a clipped group's layer, which
// holds premultiplied colour, over what is under it, each pixel scaled by
// the outline's coverage. The quad drawn is the mask's rect in the target,
// its texture coordinates the mask's own pixels, so nothing outside it is
// touched.

struct Uniforms {
    viewport_size: vec2<f32>,
    _padding: vec2<f32>,
};

@group(0) @binding(0)
var<uniform> uniforms: Uniforms;

@group(1) @binding(0)
var t_layer: texture_2d<f32>;

@group(2) @binding(0)
var t_mask: texture_2d<f32>;

struct VertexInput {
    @location(0) position: vec2<f32>,
    @location(1) tex_coords: vec2<f32>,
    @location(2) color: vec4<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) mask_px: vec2<f32>,
};

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    let x = in.position.x * 2.0 / uniforms.viewport_size.x - 1.0;
    let y = 1.0 - in.position.y * 2.0 / uniforms.viewport_size.y;
    out.clip_position = vec4<f32>(x, y, 0.0, 1.0);
    out.mask_px = in.tex_coords;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let p = vec2<i32>(floor(in.clip_position.xy));
    let size = vec2<i32>(textureDimensions(t_mask));
    let m = clamp(vec2<i32>(floor(in.mask_px)), vec2<i32>(0), size - vec2<i32>(1));
    let coverage = textureLoad(t_mask, m, 0).r;
    return textureLoad(t_layer, p, 0) * coverage;
}
