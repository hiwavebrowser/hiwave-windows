// `backdrop-filter` steps (Filter Effects 2 §3), run on the region of the
// backdrop under one box. Every pass reads `size` pixels of `input` starting
// at `origin` and writes them to `output` at (0, 0). Colours are
// premultiplied, as the layers and the frame hold them.
//
// The blur is a Gaussian of standard deviation `sigma` (the `blur()` length,
// Filter Effects 1 §13), separable, out to 3 sigma. Samples past the region
// are mirrored back into it: Chromium blurs a backdrop with mirrored edges at
// the box, so nothing outside the box bleeds in.

struct Params {
    origin: vec2<i32>,
    size: vec2<i32>,
    sigma: f32,
    radius: i32,
    _pad0: f32,
    _pad1: f32,
    // The 4x5 matrix as rows of its first four columns, then the offsets.
    row_r: vec4<f32>,
    row_g: vec4<f32>,
    row_b: vec4<f32>,
    row_a: vec4<f32>,
    offsets: vec4<f32>,
};

@group(0) @binding(0)
var<uniform> params: Params;

@group(0) @binding(1)
var input_texture: texture_2d<f32>;

@group(0) @binding(2)
var output_texture: texture_storage_2d<rgba8unorm, write>;

fn mirror(i: i32, n: i32) -> i32 {
    var j = i;
    if (j < 0) {
        j = -j - 1;
    }
    if (j >= n) {
        j = 2 * n - j - 1;
    }
    return clamp(j, 0, n - 1);
}

fn blur(coords: vec2<i32>, step: vec2<i32>) -> vec4<f32> {
    let n = select(params.size.y, params.size.x, step.x != 0);
    let along = select(coords.y, coords.x, step.x != 0);
    let two_s2 = 2.0 * params.sigma * params.sigma;
    var sum = vec4<f32>(0.0);
    var weights = 0.0;
    for (var i = -params.radius; i <= params.radius; i++) {
        let k = mirror(along + i, n);
        var at = coords;
        if (step.x != 0) {
            at.x = k;
        } else {
            at.y = k;
        }
        let w = exp(-f32(i * i) / two_s2);
        sum += textureLoad(input_texture, params.origin + at, 0) * w;
        weights += w;
    }
    return sum / weights;
}

@compute @workgroup_size(16, 16, 1)
fn blur_horizontal(@builtin(global_invocation_id) id: vec3<u32>) {
    let coords = vec2<i32>(id.xy);
    if (coords.x >= params.size.x || coords.y >= params.size.y) {
        return;
    }
    textureStore(output_texture, coords, blur(coords, vec2<i32>(1, 0)));
}

@compute @workgroup_size(16, 16, 1)
fn blur_vertical(@builtin(global_invocation_id) id: vec3<u32>) {
    let coords = vec2<i32>(id.xy);
    if (coords.x >= params.size.x || coords.y >= params.size.y) {
        return;
    }
    textureStore(output_texture, coords, blur(coords, vec2<i32>(0, 1)));
}

// One colour function's matrix, on the unpremultiplied colour, clamped to
// [0, 1] after (each function of a list clamps, as in Chromium).
@compute @workgroup_size(16, 16, 1)
fn color_matrix(@builtin(global_invocation_id) id: vec3<u32>) {
    let coords = vec2<i32>(id.xy);
    if (coords.x >= params.size.x || coords.y >= params.size.y) {
        return;
    }
    let p = textureLoad(input_texture, params.origin + coords, 0);
    var c = vec4<f32>(0.0);
    if (p.a > 0.0) {
        c = vec4<f32>(p.rgb / p.a, p.a);
    }
    let out = clamp(
        vec4<f32>(dot(params.row_r, c), dot(params.row_g, c), dot(params.row_b, c), dot(params.row_a, c))
            + params.offsets,
        vec4<f32>(0.0),
        vec4<f32>(1.0),
    );
    textureStore(output_texture, coords, vec4<f32>(out.rgb * out.a, out.a));
}
