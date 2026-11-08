// CSS `filter` colour-matrix composite (Filter Effects 1 §13): lay a
// group's layer, which holds premultiplied colour, over what is under it
// with a 4x5 matrix applied to the unpremultiplied colour, then clamped.

struct Uniforms {
    viewport_size: vec2<f32>,
    _padding: vec2<f32>,
};

@group(0) @binding(0)
var<uniform> uniforms: Uniforms;

@group(1) @binding(0)
var t_layer: texture_2d<f32>;

@group(1) @binding(1)
var s_layer: sampler;

// Rows of the matrix's first four columns, then the fifth (the offsets).
struct ColorMatrix {
    r: vec4<f32>,
    g: vec4<f32>,
    b: vec4<f32>,
    a: vec4<f32>,
    offset: vec4<f32>,
};

@group(2) @binding(0)
var<uniform> matrix: ColorMatrix;

struct VertexInput {
    @location(0) position: vec2<f32>,
    @location(1) tex_coords: vec2<f32>,
    @location(2) color: vec4<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) tex_coords: vec2<f32>,
};

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    let x = in.position.x * 2.0 / uniforms.viewport_size.x - 1.0;
    let y = 1.0 - in.position.y * 2.0 / uniforms.viewport_size.y;
    out.clip_position = vec4<f32>(x, y, 0.0, 1.0);
    out.tex_coords = in.tex_coords;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let p = textureSample(t_layer, s_layer, in.tex_coords);
    var c = vec4<f32>(0.0);
    if (p.a > 0.0) {
        c = vec4<f32>(p.rgb / p.a, p.a);
    }
    let o = clamp(
        vec4<f32>(dot(matrix.r, c), dot(matrix.g, c), dot(matrix.b, c), dot(matrix.a, c)) + matrix.offset,
        vec4<f32>(0.0),
        vec4<f32>(1.0),
    );
    return vec4<f32>(o.rgb * o.a, o.a);
}
