// Backdrop filter compute shader
// Supports Gaussian blur (separable 2-pass) and color matrix filters

struct FilterParams {
    blur_radius: f32,
    filter_type: u32,  // 0=none, 1=grayscale, 2=sepia, 3=brightness
    filter_amount: f32,
    texture_width: f32,
    texture_height: f32,
    _padding0: f32,
    _padding1: f32,
    _padding2: f32,
};

@group(0) @binding(0)
var<uniform> params: FilterParams;

@group(0) @binding(1)
var input_texture: texture_2d<f32>;

@group(0) @binding(2)
var output_texture: texture_storage_2d<rgba8unorm, write>;

// Gaussian weight function
fn gaussian(x: f32, sigma: f32) -> f32 {
    let sigma2 = sigma * sigma;
    return exp(-(x * x) / (2.0 * sigma2)) / (sqrt(2.0 * 3.14159265) * sigma);
}

// Horizontal blur pass
@compute @workgroup_size(16, 16, 1)
fn blur_horizontal(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let coords = vec2<i32>(global_id.xy);
    let dims = vec2<i32>(i32(params.texture_width), i32(params.texture_height));

    if (coords.x >= dims.x || coords.y >= dims.y) {
        return;
    }

    let radius = i32(params.blur_radius);
    let sigma = params.blur_radius / 3.0;

    if (radius <= 0) {
        // No blur, just copy
        let color = textureLoad(input_texture, coords, 0);
        textureStore(output_texture, coords, color);
        return;
    }

    var color_sum = vec4<f32>(0.0);
    var weight_sum = 0.0;

    // Sample along X axis
    for (var i = -radius; i <= radius; i++) {
        let sample_x = clamp(coords.x + i, 0, dims.x - 1);
        let sample_coords = vec2<i32>(sample_x, coords.y);
        let sample_color = textureLoad(input_texture, sample_coords, 0);

        let weight = gaussian(f32(i), sigma);
        color_sum += sample_color * weight;
        weight_sum += weight;
    }

    let result = color_sum / weight_sum;
    textureStore(output_texture, coords, result);
}

// Vertical blur pass
@compute @workgroup_size(16, 16, 1)
fn blur_vertical(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let coords = vec2<i32>(global_id.xy);
    let dims = vec2<i32>(i32(params.texture_width), i32(params.texture_height));

    if (coords.x >= dims.x || coords.y >= dims.y) {
        return;
    }

    let radius = i32(params.blur_radius);
    let sigma = params.blur_radius / 3.0;

    if (radius <= 0) {
        // No blur, just copy
        let color = textureLoad(input_texture, coords, 0);
        textureStore(output_texture, coords, color);
        return;
    }

    var color_sum = vec4<f32>(0.0);
    var weight_sum = 0.0;

    // Sample along Y axis
    for (var i = -radius; i <= radius; i++) {
        let sample_y = clamp(coords.y + i, 0, dims.y - 1);
        let sample_coords = vec2<i32>(coords.x, sample_y);
        let sample_color = textureLoad(input_texture, sample_coords, 0);

        let weight = gaussian(f32(i), sigma);
        color_sum += sample_color * weight;
        weight_sum += weight;
    }

    let result = color_sum / weight_sum;
    textureStore(output_texture, coords, result);
}

// Color matrix filter pass
@compute @workgroup_size(16, 16, 1)
fn apply_color_filter(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let coords = vec2<i32>(global_id.xy);
    let dims = vec2<i32>(i32(params.texture_width), i32(params.texture_height));

    if (coords.x >= dims.x || coords.y >= dims.y) {
        return;
    }

    var color = textureLoad(input_texture, coords, 0);
    let amount = params.filter_amount;

    // Apply color matrix based on filter type
    switch (params.filter_type) {
        case 0u: {
            // None - pass through
        }
        case 1u: {
            // Grayscale
            let luma = color.r * 0.2126 + color.g * 0.7152 + color.b * 0.0722;
            let gray = vec3<f32>(luma, luma, luma);
            color = vec4<f32>(mix(color.rgb, gray, amount), color.a);
        }
        case 2u: {
            // Sepia
            let sepia_r = color.r * 0.393 + color.g * 0.769 + color.b * 0.189;
            let sepia_g = color.r * 0.349 + color.g * 0.686 + color.b * 0.168;
            let sepia_b = color.r * 0.272 + color.g * 0.534 + color.b * 0.131;
            let sepia = vec3<f32>(
                min(sepia_r, 1.0),
                min(sepia_g, 1.0),
                min(sepia_b, 1.0)
            );
            color = vec4<f32>(mix(color.rgb, sepia, amount), color.a);
        }
        case 3u: {
            // Brightness
            color = vec4<f32>(color.rgb * amount, color.a);
        }
        default: {
            // Unknown - pass through
        }
    }

    textureStore(output_texture, coords, color);
}
