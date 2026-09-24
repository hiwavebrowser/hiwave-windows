// GPU Gradient Shader for RustKit Renderer
//
// This shader implements CSS linear gradients following the exact same algorithm
// as the cell-by-cell reference implementation in rustkit-renderer/src/lib.rs.
//
// Includes Chrome/Skia-compatible ordered dithering to reduce gradient banding.
//
// CSS Gradient Angle Convention:
//   0deg   = "to top"    -> gradient direction (0, -1) -> upward
//   90deg  = "to right"  -> gradient direction (1, 0)  -> rightward
//   180deg = "to bottom" -> gradient direction (0, 1)  -> downward
//   270deg = "to left"   -> gradient direction (-1, 0) -> leftward
//
// Direction vector formula: (sin(angle_rad), -cos(angle_rad))

// ==================== Ordered Dithering ====================
//
// Chrome/Skia uses ordered dithering (Bayer matrices) to reduce banding when
// quantizing gradients to 8-bit channels. The dither offset is added to each
// color channel before the GPU's final quantization to 8-bit.
//
// Classic 8x8 Bayer matrix values (0-63), stored as array.
// Indexing: idx = (y & 7) * 8 + (x & 7)
// Dither offset: (matrix[idx] + 0.5) / 64.0
const BAYER_8X8: array<f32, 64> = array<f32, 64>(
    0.0, 48.0, 12.0, 60.0, 3.0, 51.0, 15.0, 63.0,
    32.0, 16.0, 44.0, 28.0, 35.0, 19.0, 47.0, 31.0,
    8.0, 56.0, 4.0, 52.0, 11.0, 59.0, 7.0, 55.0,
    40.0, 24.0, 36.0, 20.0, 43.0, 27.0, 39.0, 23.0,
    2.0, 50.0, 14.0, 62.0, 1.0, 49.0, 13.0, 61.0,
    34.0, 18.0, 46.0, 30.0, 33.0, 17.0, 45.0, 29.0,
    10.0, 58.0, 6.0, 54.0, 9.0, 57.0, 5.0, 53.0,
    42.0, 26.0, 38.0, 22.0, 41.0, 25.0, 37.0, 21.0
);

/// Calculate the dither offset for a pixel position.
/// Returns a value in range [0.0078125, 0.9921875] (approximately 0 to 1).
/// This is added to color values (in 0-255 space) before truncation.
fn get_dither_offset(pixel_pos: vec2<f32>) -> f32 {
    let x = u32(pixel_pos.x);
    let y = u32(pixel_pos.y);
    let idx = (y & 7u) * 8u + (x & 7u);
    let m = BAYER_8X8[idx];
    return (m + 0.5) / 64.0;
}

/// Apply ordered dithering to a color.
/// The color is in 0-1 range, dithering is applied in 0-255 space equivalent.
/// Only RGB channels are dithered to avoid alpha artifacts.
fn apply_dither(color: vec4<f32>, dither: f32) -> vec4<f32> {
    // The dither value (0.008-0.992) is designed to be added to values in 0-255 space.
    // For 0-1 space, we divide by 255 to get the equivalent offset.
    // Example: dither=0.5 in 0-255 means +0.5 to 127 → 127.5 → rounds to 128
    //          in 0-1 space: 0.5/255 = 0.00196 added to 0.498 → 0.49996 * 255 = 127.5 → 128
    let dither_01 = dither / 255.0;

    // Only dither RGB, not alpha, to avoid transparency artifacts.
    // Chrome may dither alpha too, but for GPU blending this can cause issues.
    return vec4<f32>(
        color.r + dither_01,
        color.g + dither_01,
        color.b + dither_01,
        color.a  // Keep alpha unchanged
    );
}

// Viewport uniforms (same as color shader)
struct Uniforms {
    viewport_size: vec2<f32>,
    _padding: vec2<f32>,
};

@group(0) @binding(0)
var<uniform> uniforms: Uniforms;

// Gradient parameters
struct GradientParams {
    // Rectangle bounds in pixel coordinates
    rect_x: f32,
    rect_y: f32,
    rect_width: f32,
    rect_height: f32,
    // Gradient-specific parameters
    // For linear: param0 = angle in radians
    // For radial: param0 = rx, param1 = ry, param2 = cx (0-1), param3 = cy (0-1)
    // For conic:  param0 = start_angle
    param0: f32,
    param1: f32,
    param2: f32,
    param3: f32,
    // Gradient type: 0 = linear, 1 = radial, 2 = conic
    gradient_type: u32,
    // Repeating flag: 0 = clamp, 1 = repeat
    repeating: u32,
    // Repeat length (position of last stop, for repeating gradients)
    repeat_length: f32,
    // Number of color stops
    num_stops: u32,
    // Border radius for rounded rect clipping
    radius_tl: f32,
    radius_tr: f32,
    radius_br: f32,
    radius_bl: f32,
    // Debug mode: 0=normal, 1=t-value, 2=direction, 3=position, 4=coverage, 5=cpu-compare
    debug_mode: u32,
    // Padding to align to 16 bytes (wgpu uniform buffer alignment requirement)
    _padding0: u32,
    _padding1: u32,
    _padding2: u32,
};

@group(1) @binding(0)
var<uniform> gradient_params: GradientParams;

// Color stop structure (5 f32 values each)
struct ColorStop {
    position: f32,
    r: f32,
    g: f32,
    b: f32,
    a: f32,
};

// Storage buffer for color stops (dynamic size)
@group(1) @binding(1)
var<storage, read> color_stops: array<ColorStop>;

// Vertex input (we reuse ColorVertex layout but ignore the color)
struct VertexInput {
    @location(0) position: vec2<f32>,
    @location(1) _color: vec4<f32>,  // unused, but must match ColorVertex layout
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) pixel_pos: vec2<f32>,  // pixel position for gradient calculation
};

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;

    // Convert from pixel coords to clip space (-1 to 1)
    // Note: y is flipped (0 at top in pixel space, -1 at bottom in clip space)
    let x = in.position.x * 2.0 / uniforms.viewport_size.x - 1.0;
    let y = 1.0 - in.position.y * 2.0 / uniforms.viewport_size.y;

    out.clip_position = vec4<f32>(x, y, 0.0, 1.0);
    // Pass the original pixel position for gradient calculation
    out.pixel_pos = in.position;

    return out;
}

/// Calculate t value for linear gradient.
/// Follows the exact algorithm from rustkit-renderer/src/lib.rs
///
/// Coordinate system:
/// - Pixel coordinates: (0,0) at top-left, x increases right, y increases down
/// - rect_x, rect_y: top-left corner of gradient box in pixels
/// - rect_width, rect_height: dimensions in pixels
///
/// Angle convention (CSS Images Level 3):
/// - 0deg = "to top" (colors flow upward)
/// - 90deg = "to right"
/// - 180deg = "to bottom"
/// - 270deg = "to left"
///
/// Direction vector: (sin(angle), -cos(angle))
/// - This produces (0, -1) for 0deg, which points UP in our coordinate system
fn linear_gradient_t(pixel_pos: vec2<f32>) -> f32 {
    let angle_rad = gradient_params.param0;
    let sin_a = sin(angle_rad);
    let cos_a = cos(angle_rad);

    // Half dimensions of the rect
    let half_w = gradient_params.rect_width * 0.5;
    let half_h = gradient_params.rect_height * 0.5;

    // Gradient half-length: CSS spec formula for corner-to-corner diagonal
    // This is the distance from center to edge along the gradient direction
    let gradient_half_length = max(abs(sin_a) * half_w + abs(cos_a) * half_h, 0.001);

    // Rect center position
    let center_x = gradient_params.rect_x + half_w;
    let center_y = gradient_params.rect_y + half_h;

    // Position relative to rect center
    let px = pixel_pos.x - center_x;
    let py = pixel_pos.y - center_y;

    // Project point onto gradient line
    // Gradient direction vector is (sin_a, -cos_a) per CSS convention
    // At 0deg (to top):    direction = (0, -1)
    // At 90deg (to right): direction = (1, 0)
    let projection = px * sin_a + py * (-cos_a);

    // Normalize to 0-1 range
    // projection ranges from -gradient_half_length to +gradient_half_length
    // So (projection / gradient_half_length) ranges from -1 to +1
    // Adding 1 gives 0 to 2, dividing by 2 gives 0 to 1
    let t = (projection / gradient_half_length + 1.0) * 0.5;

    return t;
}

// Calculate t value for radial gradient
fn radial_gradient_t(pixel_pos: vec2<f32>) -> f32 {
    let rx = gradient_params.param0;
    let ry = gradient_params.param1;
    let cx_frac = gradient_params.param2;
    let cy_frac = gradient_params.param3;

    // Calculate center in pixel coordinates
    let center_x = gradient_params.rect_x + gradient_params.rect_width * cx_frac;
    let center_y = gradient_params.rect_y + gradient_params.rect_height * cy_frac;

    // Distance from center, normalized by radii
    let dx = (pixel_pos.x - center_x) / max(rx, 0.001);
    let dy = (pixel_pos.y - center_y) / max(ry, 0.001);

    // Elliptical distance (1.0 at the ellipse edge)
    let t = sqrt(dx * dx + dy * dy);

    return t;
}

// Calculate t value for conic gradient
fn conic_gradient_t(pixel_pos: vec2<f32>) -> f32 {
    let start_angle = gradient_params.param0;
    let cx_frac = gradient_params.param2;
    let cy_frac = gradient_params.param3;

    // Center of the conic gradient (using param2/param3 for center position)
    let center_x = gradient_params.rect_x + gradient_params.rect_width * cx_frac;
    let center_y = gradient_params.rect_y + gradient_params.rect_height * cy_frac;

    // Vector from center to pixel
    let dx = pixel_pos.x - center_x;
    let dy = pixel_pos.y - center_y;

    // Angle from center (atan2 returns -PI to PI)
    // CSS conic gradients start from top (12 o'clock) and go clockwise
    // atan2(dx, -dy) gives angle from top, going clockwise
    var angle = atan2(dx, -dy);

    // Adjust by start angle (convert to same space)
    angle = angle - start_angle;

    // Normalize to 0-2PI range
    if angle < 0.0 {
        angle = angle + 2.0 * 3.14159265359;
    }

    // Convert to 0-1 range
    let t = angle / (2.0 * 3.14159265359);

    return t;
}

// Apply repeating logic to t value
fn apply_repeat(t: f32) -> f32 {
    if gradient_params.repeating == 1u {
        // Repeating gradient: use modulo with the repeat length
        let repeat_len = max(gradient_params.repeat_length, 0.001);
        
        // Handle negative t values properly (rem_euclid equivalent)
        var repeated = t;
        if repeated < 0.0 {
            // For negative values, we need to shift into positive range
            let cycles = ceil(-repeated / repeat_len);
            repeated = repeated + cycles * repeat_len;
        }
        
        // Standard modulo for positive values
        repeated = repeated - floor(repeated / repeat_len) * repeat_len;
        
        return min(repeated, repeat_len);
    } else {
        // Non-repeating: clamp to 0-1
        return clamp(t, 0.0, 1.0);
    }
}

// Interpolate between color stops using premultiplied alpha interpolation in sRGB space.
// This matches Chrome's default gradient rendering.
fn interpolate_color(t: f32) -> vec4<f32> {
    let num = gradient_params.num_stops;

    if num == 0u {
        return vec4<f32>(0.0, 0.0, 0.0, 0.0);
    }

    if num == 1u || t <= color_stops[0].position {
        let s = color_stops[0];
        return vec4<f32>(s.r, s.g, s.b, s.a);
    }

    let last_idx = num - 1u;
    if t >= color_stops[last_idx].position {
        let s = color_stops[last_idx];
        return vec4<f32>(s.r, s.g, s.b, s.a);
    }

    // Find the two stops to interpolate between
    for (var i: u32 = 0u; i < last_idx; i = i + 1u) {
        let pos0 = color_stops[i].position;
        let pos1 = color_stops[i + 1u].position;

        if t >= pos0 && t <= pos1 {
            // Calculate interpolation factor
            var local_t: f32;
            if abs(pos1 - pos0) < 0.0001 {
                local_t = 0.0;
            } else {
                local_t = (t - pos0) / (pos1 - pos0);
            }

            // Premultiplied alpha interpolation (matches ColorF32::lerp)
            let s0 = color_stops[i];
            let s1 = color_stops[i + 1u];

            // Convert to premultiplied alpha
            let c0_pre = vec4<f32>(s0.r * s0.a, s0.g * s0.a, s0.b * s0.a, s0.a);
            let c1_pre = vec4<f32>(s1.r * s1.a, s1.g * s1.a, s1.b * s1.a, s1.a);

            // Interpolate in premultiplied space
            let result_pre = mix(c0_pre, c1_pre, local_t);

            // Convert back to straight alpha
            if result_pre.a > 0.0001 {
                return vec4<f32>(
                    result_pre.x / result_pre.a,
                    result_pre.y / result_pre.a,
                    result_pre.z / result_pre.a,
                    result_pre.a
                );
            } else {
                return vec4<f32>(0.0, 0.0, 0.0, 0.0);
            }
        }
    }

    // Fallback to last color
    let s = color_stops[last_idx];
    return vec4<f32>(s.r, s.g, s.b, s.a);
}

// Calculate alpha coverage for rounded rect clipping using SDF with smoothstep antialiasing
fn point_in_rounded_rect(pixel_pos: vec2<f32>) -> f32 {
    let x = pixel_pos.x;
    let y = pixel_pos.y;
    let rx = gradient_params.rect_x;
    let ry = gradient_params.rect_y;
    let rw = gradient_params.rect_width;
    let rh = gradient_params.rect_height;

    // Check if inside basic rect
    if x < rx || x > rx + rw || y < ry || y > ry + rh {
        return 0.0;
    }

    let r_tl = gradient_params.radius_tl;
    let r_tr = gradient_params.radius_tr;
    let r_br = gradient_params.radius_br;
    let r_bl = gradient_params.radius_bl;

    // Quick check: if all radii are 0, we're inside
    if r_tl == 0.0 && r_tr == 0.0 && r_br == 0.0 && r_bl == 0.0 {
        return 1.0;
    }

    // Check each corner using SDF with smoothstep antialiasing
    // Top-left corner
    if x < rx + r_tl && y < ry + r_tl {
        let dx = x - (rx + r_tl);
        let dy = y - (ry + r_tl);
        let dist = sqrt(dx * dx + dy * dy);
        let sdf = dist - r_tl;
        return 1.0 - smoothstep(-0.5, 0.5, sdf);
    }

    // Top-right corner
    if x > rx + rw - r_tr && y < ry + r_tr {
        let dx = x - (rx + rw - r_tr);
        let dy = y - (ry + r_tr);
        let dist = sqrt(dx * dx + dy * dy);
        let sdf = dist - r_tr;
        return 1.0 - smoothstep(-0.5, 0.5, sdf);
    }

    // Bottom-right corner
    if x > rx + rw - r_br && y > ry + rh - r_br {
        let dx = x - (rx + rw - r_br);
        let dy = y - (ry + rh - r_br);
        let dist = sqrt(dx * dx + dy * dy);
        let sdf = dist - r_br;
        return 1.0 - smoothstep(-0.5, 0.5, sdf);
    }

    // Bottom-left corner
    if x < rx + r_bl && y > ry + rh - r_bl {
        let dx = x - (rx + r_bl);
        let dy = y - (ry + rh - r_bl);
        let dist = sqrt(dx * dx + dy * dy);
        let sdf = dist - r_bl;
        return 1.0 - smoothstep(-0.5, 0.5, sdf);
    }

    return 1.0;
}

// Get debug visualization parameters for the current pixel
fn get_debug_info(pixel_pos: vec2<f32>, t: f32) -> vec4<f32> {
    let mode = gradient_params.debug_mode;
    
    // Mode 1: Visualize t-value as grayscale
    if mode == 1u {
        let t_vis = clamp(t, 0.0, 1.0);
        return vec4<f32>(t_vis, t_vis, t_vis, 1.0);
    }
    
    // Mode 2: Visualize direction vector as RGB (linear gradients only)
    if mode == 2u {
        let angle_rad = gradient_params.param0;
        let dir_x = sin(angle_rad);
        let dir_y = -cos(angle_rad);
        // Map [-1, 1] to [0, 1] for visualization
        return vec4<f32>((dir_x + 1.0) * 0.5, (dir_y + 1.0) * 0.5, 0.5, 1.0);
    }
    
    // Mode 3: Visualize pixel position relative to rect (should be red/green gradient)
    if mode == 3u {
        let rel_x = (pixel_pos.x - gradient_params.rect_x) / gradient_params.rect_width;
        let rel_y = (pixel_pos.y - gradient_params.rect_y) / gradient_params.rect_height;
        return vec4<f32>(clamp(rel_x, 0.0, 1.0), clamp(rel_y, 0.0, 1.0), 0.0, 1.0);
    }
    
    // Mode 4: Visualize border-radius coverage
    if mode == 4u {
        let coverage = point_in_rounded_rect(pixel_pos);
        return vec4<f32>(coverage, coverage, coverage, 1.0);
    }
    
    // Mode 5: Show raw t-value before clamping/repeat (can be negative or >1)
    if mode == 5u {
        // Map t to visible range: -1 to 2 -> 0 to 1
        let t_vis = clamp((t + 1.0) / 3.0, 0.0, 1.0);
        // Red for negative, green for 0-1, blue for >1
        var r = 0.0;
        var g = 0.0;
        var b = 0.0;
        if t < 0.0 {
            r = 1.0;
            g = t_vis;
        } else if t <= 1.0 {
            g = 1.0;
            r = t;
        } else {
            b = 1.0;
            g = clamp(2.0 - t, 0.0, 1.0);
        }
        return vec4<f32>(r, g, b, 1.0);
    }

    // Mode 6: Show first color stop directly (diagnostic: is buffer data readable?)
    if mode == 6u {
        if gradient_params.num_stops > 0u {
            let s = color_stops[0];
            return vec4<f32>(s.r, s.g, s.b, s.a);
        } else {
            // No stops - return cyan to indicate empty
            return vec4<f32>(0.0, 1.0, 1.0, 1.0);
        }
    }

    // Mode 7: Show num_stops as grayscale (0=black, 8=white)
    if mode == 7u {
        let n = f32(gradient_params.num_stops) / 8.0;
        return vec4<f32>(n, n, n, 1.0);
    }

    // Mode 8: Show interpolated color (normal path for verification)
    if mode == 8u {
        let color = interpolate_color(t);
        return color;
    }

    // Default: return magenta to indicate invalid debug mode
    return vec4<f32>(1.0, 0.0, 1.0, 1.0);
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    // Use the interpolated pixel position for gradient calculation
    let pixel_pos = in.pixel_pos;

    // Calculate border-radius clipping
    let alpha_coverage = point_in_rounded_rect(pixel_pos);
    if alpha_coverage <= 0.0 {
        discard;
    }

    // Calculate t value based on gradient type
    var t: f32;
    if gradient_params.gradient_type == 0u {
        // Linear gradient
        t = linear_gradient_t(pixel_pos);
    } else if gradient_params.gradient_type == 1u {
        // Radial gradient
        t = radial_gradient_t(pixel_pos);
    } else {
        // Conic gradient
        t = conic_gradient_t(pixel_pos);
    }

    // Check for debug mode before applying repeat
    if gradient_params.debug_mode != 0u {
        let debug_color = get_debug_info(pixel_pos, t);
        // Still apply border-radius alpha in debug mode
        return vec4<f32>(debug_color.rgb, debug_color.a * alpha_coverage);
    }

    // Apply repeating logic
    t = apply_repeat(t);

    // Interpolate color from stops
    var color = interpolate_color(t);

    // Apply border-radius alpha
    color.a = color.a * alpha_coverage;

    // Ordered dithering is available but disabled for now.
    // Testing shows it doesn't improve parity with Chrome baselines.
    // This may be because Chrome uses different dither patterns per gradient
    // or because wgpu's quantization differs from Skia's.
    // let dither = get_dither_offset(pixel_pos);
    // color = apply_dither(color, dither);

    return color;
}
