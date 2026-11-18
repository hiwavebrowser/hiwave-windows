//! GPU canvas A1 (docs/design/GPU_CANVAS_2D.md, "Plan of record: option
//! A"): a canvas's recording replayed into a texture of its own, which the
//! display list draws like any premultiplied image (it sits in the texture
//! cache under the canvas's key).
//!
//! Each replay draws only the ops handed over since the last one, straight
//! onto the texture as it was. A fill's edges are anti-aliased by computed
//! coverage (GPU-AA), not by multisampling the canvas, three ways:
//!
//! - **Exact, no atlas.** A fill with no clip that is one axis-aligned
//!   rectangle, or one convex polygon of at most [`MAX_POLY`] points (a
//!   stroked line, a triangle, a transformed rect), is painted over the
//!   pixels it can touch with each pixel's exact area inside it, computed
//!   in the shader (one edge near the pixel: that half-plane's area in
//!   closed form; more: the pixel square clipped by the polygon).
//! - **Coverage atlas.** Any other fill's pixels (its bounds within the
//!   canvas, cut into tiles that fit the atlas) get a region of a coverage
//!   atlas at SS x SS atlas pixels per canvas pixel ([`SS`] for fills of
//!   up to [`SS4_AREA`] pixels, then 2, then 1), each atlas pixel
//!   [`ATLAS_SAMPLES`]-sample multisampled: 64 sample points per canvas
//!   pixel at SS 4. The fill is drawn there stencil-then-cover: its
//!   triangle fans into the stencil's low 7 bits (non-zero: incremented /
//!   decremented by facing, wrapping; even-odd: bit 0 inverted), then its
//!   region covered where the stencil says inside, writing 1, which also
//!   zeroes the low bits. A clip chain is bit 7, built in the region before
//!   the fill: set everywhere, then cut by each link's path. The atlas
//!   resolves to one byte per atlas pixel; the fill is then painted over
//!   its tiles with its paint times its coverage (the mean of the pixel's
//!   SS x SS atlas pixels, by bilinear taps at 2 x 2 corners).
//! - **Blending.** Paint times coverage under the composite operation's
//!   fixed-function blend where the result is linear in the coverage;
//!   copy, source-in, source-out, destination-in and destination-atop read
//!   the destination from a copy of the texture and lerp by the coverage
//!   in the shader, as tiny-skia does.
//!
//! Fills are painted in order; the atlas is drawn again (a new coverage
//! pass) whenever the next tile does not fit. `putImageData` is a buffer
//! copy into the texture between fills, in order.
//!
//! Budget (per renderer): canvas textures at most
//! [`CANVAS_TEXTURE_BUDGET`] bytes (4 per pixel), and the scratch (the
//! atlas with its stencil and resolve, 9 bytes per atlas pixel, 36 MiB at
//! [`ATLAS_SIZE`], and a destination copy of 4 bytes per canvas pixel,
//! made only for the operations that read it) at most
//! [`CANVAS_SCRATCH_BUDGET`]. A canvas past either,
//! or past the device's largest texture, is denied: it stays on the CPU
//! bitmap.

use super::*;
use rustkit_canvas::gpu::{GpuBatch, GpuBlend, GpuClip, GpuDraw, GpuGradient, GpuPaint, GpuPath, GpuReset, MAX_GRADIENT_STOPS};

/// Canvas textures one renderer keeps, in bytes.
pub const CANVAS_TEXTURE_BUDGET: u64 = 128 << 20;
/// The coverage atlas, its stencil and resolve, and the destination copy,
/// in bytes.
pub const CANVAS_SCRATCH_BUDGET: u64 = 128 << 20;
/// The most vertices one canvas replay draws (fans and quads, 8 bytes
/// each: 16 MB), and never more than the device's largest buffer holds. A
/// replay that would take more (a frame of curves flattens to far more
/// vertices than its recording's bytes) is not drawn: the canvas starts
/// over from its CPU bitmap at its next batch.
pub const CANVAS_VERTEX_CAP: usize = 2 << 20;
/// Over-cap replays of one canvas in a row (its resets between them not
/// counting as a break) after which it is denied: it shows its CPU bitmap
/// for good rather than a stale texture every other frame.
pub const CANVAS_OVER_CAP_LIMIT: u32 = 3;
/// Atlas pixels per canvas pixel, per axis.
pub const SS: u32 = 4;
/// Fills whose pixels (bounds within the canvas) are at most this many
/// get [`SS`]; up to [`SS2_AREA`], 2; past it, 1 (the atlas's
/// multisampling alone, as before GPU-AA), so one large fill does not take
/// the atlas over and over.
pub const SS4_AREA: u64 = 1 << 16;
/// See [`SS4_AREA`].
pub const SS2_AREA: u64 = 1 << 18;

/// Atlas pixels per canvas pixel for a fill of `area` canvas pixels.
fn fill_ss(area: u64) -> u32 {
    if area <= SS4_AREA {
        SS
    } else if area <= SS2_AREA {
        2
    } else {
        1
    }
}
/// Samples per atlas pixel.
pub const ATLAS_SAMPLES: u32 = 4;
/// The coverage atlas's side, in atlas pixels (the device's largest
/// texture if smaller).
pub const ATLAS_SIZE: u32 = 2048;
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const COVERAGE: wgpu::TextureFormat = wgpu::TextureFormat::R8Unorm;
const STENCIL: wgpu::TextureFormat = wgpu::TextureFormat::Stencil8;

/// The atlas and its stencil and resolve, in bytes, at side `n`.
fn atlas_bytes(n: u32) -> u64 {
    n as u64 * n as u64 * (2 * ATLAS_SAMPLES as u64 + 1)
}

/// What a replay did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanvasReplay {
    /// The texture is up to date.
    Drawn,
    /// The texture is missing (evicted, a new renderer, a lost device) and
    /// the batches did not start it over: the next batch must be a reset.
    NeedsReset,
    /// No texture for this canvas (too large, past the budget, no area):
    /// show its CPU bitmap.
    Denied,
}

/// A fan or atlas quad vertex: canvas pixels.
type Pos = [f32; 2];

/// Where a tile's canvas pixels sit in the atlas: atlas = (canvas -
/// `map.xy`) * SS + `map.zw`; `size` = (atlas width, height, SS, 0).
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Inst {
    map: [f32; 4],
    size: [f32; 4],
}

/// A paint quad's vertex: NDC position, the paint (premultiplied colour,
/// or a gradient by negative red), how its coverage is found (`cov`: the
/// atlas offset of canvas pixel 0 = (x, y), kind (0 atlas, 1 rect), blend
/// code) and, for a rect, the rect in canvas pixels.
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct PaintVertex {
    pos: [f32; 2],
    color: [f32; 4],
    cov: [f32; 4],
    rect: [f32; 4],
}

/// A gradient as the shader reads it (GPU-GRADIENT), one per gradient fill
/// of a pass, in a storage buffer. A cover vertex names it by a negative
/// red: `-(index + 1)` (a real colour channel is never negative).
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct GradRecord {
    /// Device to user space: (a, b, c, d).
    inv0: [f32; 4],
    /// (e, f, radial 0/1, stop count).
    inv1: [f32; 4],
    /// (x0, y0, x1, y1).
    geo: [f32; 4],
    /// (r1, 0, 0, 0).
    extra: [f32; 4],
    offs: [[f32; 4]; MAX_GRADIENT_STOPS / 4],
    cols: [[f32; 4]; MAX_GRADIENT_STOPS],
}

impl GradRecord {
    fn of(g: &GpuGradient) -> Self {
        let mut r = Self::zeroed();
        r.inv0 = [g.inv[0], g.inv[1], g.inv[2], g.inv[3]];
        r.inv1 = [g.inv[4], g.inv[5], if g.radial { 1.0 } else { 0.0 }, g.stops.len().min(MAX_GRADIENT_STOPS) as f32];
        r.geo = [g.p0[0], g.p0[1], g.p1[0], g.p1[1]];
        r.extra = [g.r1, 0.0, 0.0, 0.0];
        for (i, (o, c)) in g.stops.iter().take(MAX_GRADIENT_STOPS).enumerate() {
            r.offs[i / 4][i % 4] = *o;
            r.cols[i] = *c;
        }
        r
    }
}

use bytemuck::Zeroable;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Pipe {
    /// Atlas: a fan into the stencil.
    Stencil { eo: bool },
    /// Atlas: 1 where the stencil says inside (and the clip bit), low bits
    /// zeroed.
    Mask { eo: bool, clip: bool },
    ClipSet,
    ClipCut { eo: bool },
    ZeroLow,
    /// Canvas: paint times coverage under a fixed-function blend.
    Paint { blend: GpuBlend },
    /// Canvas: the blend in the shader against a copy of the destination.
    PaintDst,
}

struct Atlas {
    size: u32,
    _color: wgpu::Texture,
    color: wgpu::TextureView,
    _stencil: wgpu::Texture,
    stencil: wgpu::TextureView,
    _resolved: wgpu::Texture,
    resolved: wgpu::TextureView,
}

struct DstCopy {
    size: (u32, u32),
    texture: wgpu::Texture,
    view: wgpu::TextureView,
}

pub(crate) struct CanvasGpu {
    shader: wgpu::ShaderModule,
    atlas_layout: wgpu::PipelineLayout,
    paint_bgl: wgpu::BindGroupLayout,
    paint_layout: wgpu::PipelineLayout,
    pipes: HashMap<Pipe, wgpu::RenderPipeline>,
    atlas: Option<Atlas>,
    dst: Option<DstCopy>,
    /// Bound when no destination copy is needed.
    dst_dummy: wgpu::TextureView,
    /// Bilinear, for the atlas's 2 x 2 means.
    sampler: wgpu::Sampler,
    /// The canvas textures this renderer made, by key, and their bytes.
    canvases: HashMap<String, u64>,
    /// Tests: a lower [`CANVAS_VERTEX_CAP`].
    vertex_cap: Option<usize>,
    /// Over-cap replays in a row, by key.
    over_cap: HashMap<String, u32>,
}


const SHADER: &str = r#"
@vertex
fn vs_atlas(@location(0) p: vec2<f32>, @location(1) map: vec4<f32>, @location(2) size: vec4<f32>) -> @builtin(position) vec4<f32> {
    let a = (p - map.xy) * size.z + map.zw;
    return vec4<f32>(a.x / size.x * 2.0 - 1.0, 1.0 - a.y / size.y * 2.0, 0.0, 1.0);
}

@fragment
fn fs_one() -> @location(0) vec4<f32> {
    return vec4<f32>(1.0);
}

struct VOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) @interpolate(flat) cov: vec4<f32>,
    @location(2) @interpolate(flat) rect: vec4<f32>,
};

@vertex
fn vs_paint(@location(0) p: vec2<f32>, @location(1) c: vec4<f32>, @location(2) cov: vec4<f32>, @location(3) rect: vec4<f32>) -> VOut {
    var o: VOut;
    o.pos = vec4<f32>(p, 0.0, 1.0);
    o.color = c;
    o.cov = cov;
    o.rect = rect;
    return o;
}

struct Grad {
    inv0: vec4<f32>,
    inv1: vec4<f32>,
    geo: vec4<f32>,
    extra: vec4<f32>,
    offs: array<vec4<f32>, 4>,
    cols: array<vec4<f32>, 16>,
};

@group(0) @binding(0) var<storage, read> grads: array<Grad>;

fn off(k: u32, i: u32) -> f32 {
    return grads[k].offs[i / 4u][i % 4u];
}

// A gradient at the pixel centre `p` (device space), as tiny-skia
// evaluates it: user space by the inverse transform, t padded to [0, 1],
// the stops interpolated unpremultiplied, then premultiplied. A radial
// gradient is two-point conical from (x0, y0), radius 0, to (x1, y1), r1:
// the larger t whose circle passes through the point; none: transparent.
fn gradient(k: u32, p: vec2<f32>) -> vec4<f32> {
    let inv0 = grads[k].inv0;
    let inv1 = grads[k].inv1;
    let u = vec2<f32>(inv0.x * p.x + inv0.z * p.y + inv1.x, inv0.y * p.x + inv0.w * p.y + inv1.y);
    let p0 = grads[k].geo.xy;
    let d = grads[k].geo.zw - p0;
    var t: f32;
    if (inv1.z < 0.5) {
        t = dot(u - p0, d) / dot(d, d);
    } else {
        let r1 = grads[k].extra.x;
        let q = u - p0;
        let a = dot(d, d) - r1 * r1;
        let b = dot(q, d);
        let c = dot(q, q);
        if (abs(a) < 1e-6) {
            if (b == 0.0) {
                return vec4<f32>(0.0);
            }
            t = c / (2.0 * b);
        } else {
            let disc = b * b - a * c;
            if (disc < 0.0) {
                return vec4<f32>(0.0);
            }
            let s = sqrt(disc);
            let t1 = (b + s) / a;
            let t2 = (b - s) / a;
            t = max(t1, t2);
            if (t * r1 < 0.0) {
                t = min(t1, t2);
            }
        }
        if (t * r1 < 0.0) {
            return vec4<f32>(0.0);
        }
    }
    t = clamp(t, 0.0, 1.0);
    let n = u32(inv1.w);
    var col = grads[k].cols[0];
    if (t > off(k, 0u)) {
        col = grads[k].cols[n - 1u];
        for (var i = 1u; i < n; i = i + 1u) {
            if (t <= off(k, i)) {
                let o0 = off(k, i - 1u);
                let o1 = off(k, i);
                var f = 1.0;
                if (o1 > o0) {
                    f = (t - o0) / (o1 - o0);
                }
                col = mix(grads[k].cols[i - 1u], grads[k].cols[i], f);
                break;
            }
        }
    }
    return vec4<f32>(col.rgb * col.a, col.a);
}


@group(0) @binding(1) var atlas: texture_2d<f32>;
@group(0) @binding(2) var dst: texture_2d<f32>;
@group(0) @binding(3) var<storage, read> polys: array<vec2<f32>>;
@group(0) @binding(4) var bilinear: sampler;

// The area of a unit square on the inside of a line whose unit normal is
// `nrm`, the square's centre at signed distance `s` (inside positive).
fn half_plane(nrm: vec2<f32>, s: f32) -> f32 {
    let a = max(abs(nrm.x), abs(nrm.y));
    let b = min(abs(nrm.x), abs(nrm.y));
    let lo = 0.5 * (a - b);
    if (abs(s) <= lo) {
        return 0.5 + s / a;
    }
    let t = 0.5 * (a + b) - abs(s);
    if (t <= 0.0) {
        return select(0.0, 1.0, s > 0.0);
    }
    let corner = t * t / (2.0 * a * b);
    return select(corner, 1.0 - corner, s > 0.0);
}

// The area of the pixel square at `px` inside the convex polygon of `n`
// points from `off` (positive orientation: inside is where
// cross(edge, point - start) >= 0): the square clipped by each edge.
fn poly_coverage(off: u32, n: u32, px: vec2<f32>) -> f32 {
    // First by distance from the pixel centre: wholly inside every edge's
    // half-plane, or wholly outside one, or near one edge only (that
    // half-plane's exact area in the square); else clipped.
    let c = px + vec2<f32>(0.5);
    var near = 0u;
    var near_s = 0.0;
    var near_n = vec2<f32>(0.0);
    for (var e = 0u; e < n; e = e + 1u) {
        let p = polys[off + e];
        let d = polys[off + (e + 1u) % n] - p;
        let len = length(d);
        if (len == 0.0) {
            continue;
        }
        // Inside is cross(d, x - p) >= 0: normal (-d.y, d.x) / len.
        let nrm = vec2<f32>(-d.y, d.x) / len;
        let s = dot(nrm, c - p);
        let reach = 0.5 * (abs(nrm.x) + abs(nrm.y));
        if (s <= -reach) {
            return 0.0;
        }
        if (s < reach) {
            near = near + 1u;
            near_s = s;
            near_n = nrm;
        }
    }
    if (near == 0u) {
        return 1.0;
    }
    if (near == 1u) {
        return half_plane(near_n, near_s);
    }
    var a: array<vec2<f32>, 16>;
    var b: array<vec2<f32>, 16>;
    var m = 4u;
    a[0] = px;
    a[1] = px + vec2<f32>(1.0, 0.0);
    a[2] = px + vec2<f32>(1.0, 1.0);
    a[3] = px + vec2<f32>(0.0, 1.0);
    for (var e = 0u; e < n; e = e + 1u) {
        let p = polys[off + e];
        let d = polys[off + (e + 1u) % n] - p;
        var inside = 0u;
        for (var k = 0u; k < m; k = k + 1u) {
            if (d.x * (a[k].y - p.y) - d.y * (a[k].x - p.x) >= 0.0) {
                inside = inside + 1u;
            }
        }
        if (inside == m) {
            continue;
        }
        if (inside == 0u) {
            return 0.0;
        }
        var k2 = 0u;
        for (var k = 0u; k < m; k = k + 1u) {
            let u = a[k];
            let v = a[(k + 1u) % m];
            let su = d.x * (u.y - p.y) - d.y * (u.x - p.x);
            let sv = d.x * (v.y - p.y) - d.y * (v.x - p.x);
            if (su >= 0.0 && k2 < 16u) {
                b[k2] = u;
                k2 = k2 + 1u;
            }
            if ((su >= 0.0) != (sv >= 0.0) && k2 < 16u) {
                b[k2] = u + (v - u) * (su / (su - sv));
                k2 = k2 + 1u;
            }
        }
        m = k2;
        for (var k = 0u; k < m; k = k + 1u) {
            a[k] = b[k];
        }
    }
    var area = 0.0;
    for (var k = 0u; k < m; k = k + 1u) {
        let u = a[k];
        let v = a[(k + 1u) % m];
        area = area + u.x * v.y - v.x * u.y;
    }
    return clamp(abs(area) * 0.5, 0.0, 1.0);
}

// The pixel's coverage: the mean of its SS x SS atlas pixels, or the
// exact area of the pixel inside the rect or convex polygon.
fn coverage(i: VOut) -> f32 {
    let px = floor(i.pos.xy);
    if (i.cov.z > 1.5) {
        return poly_coverage(u32(i.cov.x + 0.5), u32(i.cov.y + 0.5), px);
    }
    if (i.cov.z < 0.5) {
        let ss = i32(i.rect.x + 0.5);
        let base = px * f32(ss) + i.cov.xy;
        if (ss == 1) {
            return textureLoad(atlas, vec2<i32>(base), 0).r;
        }
        // Each bilinear tap at the shared corner of 2 x 2 atlas pixels
        // is their mean.
        let size = vec2<f32>(textureDimensions(atlas));
        var sum = 0.0;
        for (var y = 0; y < ss; y = y + 2) {
            for (var x = 0; x < ss; x = x + 2) {
                sum = sum + textureSampleLevel(atlas, bilinear, (base + vec2<f32>(f32(x) + 1.0, f32(y) + 1.0)) / size, 0.0).r;
            }
        }
        return sum / f32((ss / 2) * (ss / 2));
    }
    let r = i.rect;
    let ox = clamp(min(px.x + 1.0, r.z) - max(px.x, r.x), 0.0, 1.0);
    let oy = clamp(min(px.y + 1.0, r.w) - max(px.y, r.y), 0.0, 1.0);
    return ox * oy;
}

fn paint(i: VOut) -> vec4<f32> {
    if (i.color.r >= 0.0) {
        return i.color;
    }
    return gradient(u32(-i.color.r - 0.5), i.pos.xy);
}

@fragment
fn fs_paint(i: VOut) -> @location(0) vec4<f32> {
    return paint(i) * coverage(i);
}

// Porter-Duff by code (GpuBlend order), lerped by the coverage against
// the destination as it was.
@fragment
fn fs_paint_dst(i: VOut) -> @location(0) vec4<f32> {
    let d = textureLoad(dst, vec2<i32>(floor(i.pos.xy)), 0);
    let s = paint(i);
    let c = coverage(i);
    let op = u32(i.cov.w + 0.5);
    var fa = 0.0;
    var fb = 0.0;
    switch op {
        case 0u: { fa = 0.0; fb = 0.0; }
        case 1u: { fa = 1.0; fb = 1.0 - s.a; }
        case 2u: { fa = d.a; fb = 0.0; }
        case 3u: { fa = 1.0 - d.a; fb = 0.0; }
        case 4u: { fa = d.a; fb = 1.0 - s.a; }
        case 5u: { fa = 1.0 - d.a; fb = 1.0; }
        case 6u: { fa = 0.0; fb = s.a; }
        case 7u: { fa = 0.0; fb = 1.0 - s.a; }
        case 8u: { fa = 1.0 - d.a; fb = s.a; }
        case 9u: { fa = 1.0; fb = 1.0; }
        case 10u: { fa = 1.0; fb = 0.0; }
        default: { fa = 1.0 - d.a; fb = 1.0 - s.a; }
    }
    let r = min(fa * s + fb * d, vec4<f32>(1.0));
    return mix(d, r, c);
}
"#;

/// The shader's code for a blend (`fs_paint_dst`'s switch).
fn blend_code(b: GpuBlend) -> f32 {
    use GpuBlend::*;
    (match b {
        Clear => 0,
        SourceOver => 1,
        SourceIn => 2,
        SourceOut => 3,
        SourceAtop => 4,
        DestinationOver => 5,
        DestinationIn => 6,
        DestinationOut => 7,
        DestinationAtop => 8,
        Lighter => 9,
        Copy => 10,
        Xor => 11,
    }) as f32
}

/// Whether `b`'s result is linear in the coverage with a premultiplied
/// source scaled by it (the destination's factor 1 or 1 - source alpha;
/// clear by a source of (0, 0, 0, coverage)): drawn by fixed-function
/// blending. The rest read a copy of the destination.
fn linear_in_coverage(b: GpuBlend) -> bool {
    use GpuBlend::*;
    matches!(b, Clear | SourceOver | SourceAtop | DestinationOver | DestinationOut | Lighter | Xor)
}

impl CanvasGpu {
    fn new(device: &wgpu::Device) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("canvas_gpu"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let atlas_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("canvas_gpu_atlas"),
            bind_group_layouts: &[],
            push_constant_ranges: &[],
        });
        let texture = |binding, filterable| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                multisampled: false,
                view_dimension: wgpu::TextureViewDimension::D2,
                sample_type: wgpu::TextureSampleType::Float { filterable },
            },
            count: None,
        };
        let paint_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("canvas_gpu_paint"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                texture(1, true),
                texture(2, false),
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let paint_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("canvas_gpu_paint"),
            bind_group_layouts: &[&paint_bgl],
            push_constant_ranges: &[],
        });
        let dst_dummy = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("canvas_gpu_dst_dummy"),
                size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: FORMAT,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
            .create_view(&Default::default());
        Self {
            shader,
            atlas_layout,
            paint_bgl,
            paint_layout,
            pipes: HashMap::new(),
            atlas: None,
            dst: None,
            dst_dummy,
            sampler: device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("canvas_gpu_atlas"),
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            }),
            canvases: HashMap::new(),
            vertex_cap: None,
            over_cap: HashMap::new(),
        }
    }

    /// A replay of `key` past a cap: a reset, or after
    /// [`CANVAS_OVER_CAP_LIMIT`] in a row, denied.
    fn over(&mut self, key: &str) -> CanvasReplay {
        let count = self.over_cap.entry(key.to_string()).or_insert(0);
        *count += 1;
        if *count >= CANVAS_OVER_CAP_LIMIT {
            self.over_cap.remove(key);
            return CanvasReplay::Denied;
        }
        CanvasReplay::NeedsReset
    }

    fn pipe(&mut self, device: &wgpu::Device, p: Pipe) -> &wgpu::RenderPipeline {
        if !self.pipes.contains_key(&p) {
            let built = self.build(device, p);
            self.pipes.insert(p, built);
        }
        &self.pipes[&p]
    }

    fn build(&self, device: &wgpu::Device, p: Pipe) -> wgpu::RenderPipeline {
        use wgpu::{CompareFunction as C, StencilOperation as S};
        if let Pipe::Paint { .. } | Pipe::PaintDst = p {
            let (fs, blend) = match p {
                Pipe::Paint { blend } => ("fs_paint", Some(blend_state(blend))),
                _ => ("fs_paint_dst", None),
            };
            return device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("canvas_gpu_paint"),
                layout: Some(&self.paint_layout),
                vertex: wgpu::VertexState {
                    module: &self.shader,
                    entry_point: Some("vs_paint"),
                    buffers: &[wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<PaintVertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x4, 2 => Float32x4, 3 => Float32x4],
                    }],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &self.shader,
                    entry_point: Some(fs),
                    targets: &[Some(wgpu::ColorTargetState { format: FORMAT, blend, write_mask: wgpu::ColorWrites::ALL })],
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleList, cull_mode: None, ..Default::default() },
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview: None,
                cache: None,
            });
        }
        let face = |compare, fail_op, pass_op| wgpu::StencilFaceState { compare, fail_op, depth_fail_op: S::Keep, pass_op };
        let none = wgpu::ColorWrites::empty();
        // (colour writes, front, back, read mask, write mask)
        let (writes, front, back, read, write) = match p {
            Pipe::Stencil { eo: false } => (none, face(C::Always, S::Keep, S::IncrementWrap), face(C::Always, S::Keep, S::DecrementWrap), 0xff, 0x7f),
            Pipe::Stencil { eo: true } => (none, face(C::Always, S::Keep, S::Invert), face(C::Always, S::Keep, S::Invert), 0xff, 0x01),
            Pipe::Mask { eo, clip } => {
                // Non-zero: inside when the low bits are not 0 (with the
                // clip: when the whole byte is over 0x80, bit 7 set).
                // Even-odd: bit 0 set (and bit 7). Either way the low bits
                // go back to 0 for the next fill.
                let (compare, read) = match (eo, clip) {
                    (false, false) => (C::Less, 0x7f),
                    (false, true) => (C::Less, 0xff),
                    (true, false) => (C::Equal, 0x01),
                    (true, true) => (C::Equal, 0x81),
                };
                let f = face(compare, S::Zero, S::Zero);
                (wgpu::ColorWrites::ALL, f, f, read, 0x7f)
            }
            Pipe::ClipSet => {
                let f = face(C::Always, S::Keep, S::Replace);
                (none, f, f, 0xff, 0x80)
            }
            Pipe::ClipCut { eo } => {
                // Outside the link's path (low bits 0): bit 7 off.
                let f = face(C::Equal, S::Keep, S::Zero);
                (none, f, f, if eo { 0x01 } else { 0x7f }, 0x80)
            }
            Pipe::ZeroLow => {
                let f = face(C::Always, S::Zero, S::Zero);
                (none, f, f, 0xff, 0x7f)
            }
            Pipe::Paint { .. } | Pipe::PaintDst => unreachable!(),
        };
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("canvas_gpu_atlas"),
            layout: Some(&self.atlas_layout),
            vertex: wgpu::VertexState {
                module: &self.shader,
                entry_point: Some("vs_atlas"),
                buffers: &[
                    wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<Pos>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &wgpu::vertex_attr_array![0 => Float32x2],
                    },
                    wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<Inst>() as u64,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &wgpu::vertex_attr_array![1 => Float32x4, 2 => Float32x4],
                    },
                ],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &self.shader,
                entry_point: Some("fs_one"),
                targets: &[Some(wgpu::ColorTargetState { format: COVERAGE, blend: None, write_mask: writes })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleList, cull_mode: None, ..Default::default() },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: STENCIL,
                depth_write_enabled: false,
                depth_compare: wgpu::CompareFunction::Always,
                stencil: wgpu::StencilState { front, back, read_mask: read, write_mask: write },
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState { count: ATLAS_SAMPLES, mask: !0, alpha_to_coverage_enabled: false },
            multiview: None,
            cache: None,
        })
    }

    fn atlas(&mut self, device: &wgpu::Device, n: u32) {
        if self.atlas.as_ref().is_some_and(|a| a.size == n) {
            return;
        }
        let make = |format, samples, usage, label| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d { width: n, height: n, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: samples,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
        };
        let rt = wgpu::TextureUsages::RENDER_ATTACHMENT;
        let color = make(COVERAGE, ATLAS_SAMPLES, rt, "canvas_gpu_atlas");
        let stencil = make(STENCIL, ATLAS_SAMPLES, rt, "canvas_gpu_atlas_stencil");
        let resolved = make(COVERAGE, 1, rt | wgpu::TextureUsages::TEXTURE_BINDING, "canvas_gpu_atlas_resolved");
        self.atlas = Some(Atlas {
            size: n,
            color: color.create_view(&Default::default()),
            _color: color,
            stencil: stencil.create_view(&Default::default()),
            _stencil: stencil,
            resolved: resolved.create_view(&Default::default()),
            _resolved: resolved,
        });
    }

    fn dst(&mut self, device: &wgpu::Device, size: (u32, u32)) {
        if self.dst.as_ref().is_some_and(|d| d.size == size) {
            return;
        }
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("canvas_gpu_dst"),
            size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        self.dst = Some(DstCopy { size, view: texture.create_view(&Default::default()), texture });
    }
}

fn blend_state(b: GpuBlend) -> wgpu::BlendState {
    use wgpu::BlendFactor::*;
    // The source is premultiplied paint times coverage; for clear, (0, 0,
    // 0, coverage). Only the operations `linear_in_coverage` takes.
    let (src_factor, dst_factor) = match b {
        GpuBlend::Clear => (Zero, OneMinusSrcAlpha),
        GpuBlend::SourceOver => (One, OneMinusSrcAlpha),
        GpuBlend::SourceAtop => (DstAlpha, OneMinusSrcAlpha),
        GpuBlend::DestinationOver => (OneMinusDstAlpha, One),
        GpuBlend::DestinationOut => (Zero, OneMinusSrcAlpha),
        GpuBlend::Lighter => (One, One),
        GpuBlend::Xor => (OneMinusDstAlpha, OneMinusSrcAlpha),
        GpuBlend::SourceIn | GpuBlend::SourceOut | GpuBlend::DestinationIn | GpuBlend::DestinationAtop | GpuBlend::Copy => (One, Zero),
    };
    let c = wgpu::BlendComponent { src_factor, dst_factor, operation: wgpu::BlendOperation::Add };
    wgpu::BlendState { color: c, alpha: c }
}

impl Renderer {
    /// Replays `batches` (oldest first) of the canvas whose texture is
    /// kept under `key`. A reset starts the texture over (and supersedes
    /// every batch before it); without one, the texture must exist at the
    /// batches' size.
    pub fn replay_canvas(&mut self, key: &str, batches: &[GpuBatch]) -> CanvasReplay {
        let start = batches.iter().rposition(|b| b.reset.is_some()).unwrap_or(0);
        let batches = &batches[start..];
        let Some(last) = batches.last() else { return CanvasReplay::Drawn };
        let (w, h) = (last.width, last.height);
        let limit = self.device.limits().max_texture_dimension_2d;
        if w == 0 || h == 0 || w > limit || h > limit || w as u64 * h as u64 * 4 + atlas_bytes(ATLAS_SIZE.min(limit)) > CANVAS_SCRATCH_BUDGET {
            return CanvasReplay::Denied;
        }
        if device_has_no_storage_buffers(&self.device) {
            // The paint pipelines read gradients from a storage buffer.
            return CanvasReplay::Denied;
        }
        if batches.iter().any(|b| (b.width, b.height) != (w, h)) {
            return CanvasReplay::NeedsReset;
        }
        let have = self.texture_cache.textures.get(key).map(|t| (t.width, t.height));
        let reset = batches[0].reset.clone();
        if reset.is_none() && have != Some((w, h)) {
            return CanvasReplay::NeedsReset;
        }
        let device = self.device.clone();
        let queue = self.queue.clone();
        let cg = self.canvas_gpu.get_or_insert_with(|| Box::new(CanvasGpu::new(&device)));
        let textures = &self.texture_cache.textures;
        cg.canvases.retain(|k, _| textures.contains_key(k));
        // Lowered first, under the vertex cap, before anything touches the
        // GPU: past the cap nothing is drawn and the texture is as it was.
        let fits_buffer = (device.limits().max_buffer_size / std::mem::size_of::<Pos>() as u64).min(usize::MAX as u64) as usize;
        let mut left = cg.vertex_cap.unwrap_or(CANVAS_VERTEX_CAP).min(fits_buffer);
        let mut draws: Vec<GpuDraw> = Vec::new();
        for b in batches {
            let Some((d, n)) = b.lower_within(left) else {
                return cg.over(key);
            };
            left -= n;
            draws.extend(d);
        }
        // The gradient records a pass binds, counted (every gradient fill,
        // an over-count of the distinct ones) against the device's binding
        // limit, before any GPU work: past it, as past the vertex cap.
        let grads = draws.iter().filter(|d| matches!(d, GpuDraw::Fill { paint: GpuPaint::Gradient(_), .. })).count() as u64;
        if grads * std::mem::size_of::<GradRecord>() as u64 > device.limits().max_storage_buffer_binding_size as u64 {
            return cg.over(key);
        }
        let bytes = w as u64 * h as u64 * 4;
        if reset.is_some() && have != Some((w, h)) {
            let used: u64 = cg.canvases.iter().filter(|(k, _)| k.as_str() != key).map(|(_, b)| *b).sum();
            if used + bytes > CANVAS_TEXTURE_BUDGET {
                return CanvasReplay::Denied;
            }
        }
        // Everything below in error scopes: a validation or out-of-memory
        // error drops the texture and asks for a reset, instead of reaching
        // wgpu's default handler (which panics).
        device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("canvas_gpu") });
        if let Some(reset) = &reset {
            if have != Some((w, h)) {
                let texture = device.create_texture(&wgpu::TextureDescriptor {
                    label: Some(key),
                    size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: FORMAT,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING
                        | wgpu::TextureUsages::COPY_DST
                        | wgpu::TextureUsages::COPY_SRC
                        | wgpu::TextureUsages::RENDER_ATTACHMENT,
                    view_formats: &[],
                });
                let view = texture.create_view(&Default::default());
                let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    layout: &self.texture_cache.bind_group_layout,
                    entries: &[
                        wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
                        wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.texture_cache.sampler) },
                    ],
                    label: Some("canvas_gpu_texture"),
                });
                self.texture_cache.textures.insert(
                    key.to_string(),
                    CachedTexture { texture, view, bind_group, width: w, height: h, premultiplied: true },
                );
                cg.canvases.insert(key.to_string(), bytes);
            }
            let t = &self.texture_cache.textures[key];
            match reset {
                GpuReset::Pixels(px) if px.len() as u64 == w as u64 * h as u64 * 4 => {
                    // Before the encoder's passes: a write is done at the
                    // start of the next submit.
                    queue.write_texture(
                        wgpu::TexelCopyTextureInfo { texture: &t.texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                        px,
                        wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4 * w), rows_per_image: Some(h) },
                        wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                    );
                }
                _ => {
                    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("canvas_gpu_blank"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &t.view,
                            resolve_target: None,
                            ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                        })],
                        depth_stencil_attachment: None,
                        timestamp_writes: None,
                        occlusion_query_set: None,
                    });
                }
            }
        }
        let t = &self.texture_cache.textures[key];
        let mut i = 0;
        while i < draws.len() {
            if let GpuDraw::Put { x, y, width, height, rgba } = &draws[i] {
                encode_put(&device, &mut encoder, &t.texture, (w, h), (*x, *y, *width, *height), rgba);
                i += 1;
                continue;
            }
            let j = draws[i..].iter().position(|d| matches!(d, GpuDraw::Put { .. })).map_or(draws.len(), |k| i + k);
            encode_fills(cg, &device, &mut encoder, t, (w, h), &draws[i..j]);
            i = j;
        }
        queue.submit(std::iter::once(encoder.finish()));
        let validation = ready(device.pop_error_scope()).flatten();
        let memory = ready(device.pop_error_scope()).flatten();
        if let Some(e) = validation.or(memory) {
            tracing::warn!(error = %e, "canvas GPU replay failed; the canvas starts over from its bitmap");
            self.texture_cache.textures.remove(key);
            if let Some(cg) = self.canvas_gpu.as_mut() {
                cg.canvases.remove(key);
            }
            return CanvasReplay::NeedsReset;
        }
        if batches.iter().any(|b| b.op_count() > 0) {
            // A frame of draws fit: the over-cap run is broken.
            if let Some(cg) = self.canvas_gpu.as_mut() {
                cg.over_cap.remove(key);
            }
        }
        CanvasReplay::Drawn
    }

    /// Tests: a lower vertex cap for canvas replays.
    #[doc(hidden)]
    pub fn set_canvas_vertex_cap(&mut self, cap: usize) {
        let device = self.device.clone();
        self.canvas_gpu.get_or_insert_with(|| Box::new(CanvasGpu::new(&device))).vertex_cap = Some(cap);
    }

    /// Tests and tools: the canvas texture under `key` read back,
    /// premultiplied RGBA rows (a GPU round trip; never on a frame's path).
    #[doc(hidden)]
    pub fn read_canvas_texture(&self, key: &str) -> Option<Vec<u8>> {
        let t = self.texture_cache.textures.get(key)?;
        let readback = screenshot::GpuReadbackBuffer::new(&self.device, t.width, t.height);
        let mut encoder = self.device.create_command_encoder(&Default::default());
        readback.copy_from_texture(&mut encoder, &t.texture);
        self.queue.submit(std::iter::once(encoder.finish()));
        readback.read_data_sync(&self.device).ok()
    }

    /// Tests: canvases with an over-cap count.
    #[doc(hidden)]
    pub fn canvas_over_cap_counts(&self) -> usize {
        self.canvas_gpu.as_ref().map_or(0, |cg| cg.over_cap.len())
    }

    /// The canvas textures this renderer keeps, in bytes.
    pub fn canvas_texture_bytes(&self) -> u64 {
        let Some(cg) = &self.canvas_gpu else { return 0 };
        cg.canvases.iter().filter(|(k, _)| self.texture_cache.contains(k)).map(|(_, b)| *b).sum()
    }
}

fn device_has_no_storage_buffers(device: &wgpu::Device) -> bool {
    device.limits().max_storage_buffers_per_shader_stage == 0
}

impl Renderer {
    /// A canvas that went away (its image dropped): its over-cap count
    /// goes with it.
    pub(crate) fn forget_canvases(&mut self, mut keep: impl FnMut(&str) -> bool) {
        if let Some(cg) = self.canvas_gpu.as_mut() {
            cg.over_cap.retain(|k, _| keep(k));
            cg.canvases.retain(|k, _| keep(k));
        }
    }
}

/// A future's output if it is ready at the first poll (wgpu's native
/// error scopes are); `None` if not.
fn ready<F: std::future::Future>(f: F) -> Option<F::Output> {
    let mut f = std::pin::pin!(f);
    let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
    match f.as_mut().poll(&mut cx) {
        std::task::Poll::Ready(v) => Some(v),
        std::task::Poll::Pending => None,
    }
}

fn encode_put(device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder, texture: &wgpu::Texture, size: (u32, u32), rect: (u32, u32, u32, u32), rgba: &[u8]) {
    let (x, y, w, h) = rect;
    if w == 0 || h == 0 || x >= size.0 || y >= size.1 || rgba.len() < (w * h * 4) as usize {
        return;
    }
    let (cw, ch) = (w.min(size.0 - x), h.min(size.1 - y));
    let row = (cw * 4 + 255) & !255;
    let mut padded = vec![0u8; (row * ch) as usize];
    for r in 0..ch as usize {
        let src = r * w as usize * 4;
        padded[r * row as usize..r * row as usize + cw as usize * 4].copy_from_slice(&rgba[src..src + cw as usize * 4]);
    }
    let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("canvas_gpu_put"),
        contents: &padded,
        usage: wgpu::BufferUsages::COPY_SRC,
    });
    encoder.copy_buffer_to_texture(
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: Some(ch) },
        },
        wgpu::TexelCopyTextureInfo { texture, mip_level: 0, origin: wgpu::Origin3d { x, y, z: 0 }, aspect: wgpu::TextureAspect::All },
        wgpu::Extent3d { width: cw, height: ch, depth_or_array_layers: 1 },
    );
}


/// `p` when it is one axis-aligned rectangle (a fan of two triangles
/// round its four corners): `[x0, y0, x1, y1]`.
fn axis_rect(p: &GpuPath) -> Option<[f32; 4]> {
    let t = &p.tris;
    if t.len() != 6 || t[0] != t[3] || t[2] != t[4] {
        return None;
    }
    let q = [t[0], t[1], t[2], t[5]];
    for i in 0..4 {
        let (a, b) = (q[i], q[(i + 1) % 4]);
        if a == b || (a[0] != b[0] && a[1] != b[1]) {
            return None;
        }
    }
    let x0 = q.iter().map(|p| p[0]).fold(f32::INFINITY, f32::min);
    let y0 = q.iter().map(|p| p[1]).fold(f32::INFINITY, f32::min);
    let x1 = q.iter().map(|p| p[0]).fold(f32::NEG_INFINITY, f32::max);
    let y1 = q.iter().map(|p| p[1]).fold(f32::NEG_INFINITY, f32::max);
    let shoelace: f32 = (0..4).map(|i| q[i][0] * q[(i + 1) % 4][1] - q[(i + 1) % 4][0] * q[i][1]).sum::<f32>() / 2.0;
    let area = (x1 - x0) * (y1 - y0);
    if !(area > 0.0) || (shoelace.abs() - area).abs() > area * 1e-4 {
        return None;
    }
    Some([x0, y0, x1, y1])
}

/// Most points of a polygon whose coverage the shader computes exactly.
pub const MAX_POLY: usize = 24;

/// `p` when it is one convex polygon of at most [`MAX_POLY`] points (one
/// fan, every turn the same way, once round): its points, positively
/// oriented (inside where cross(edge, point - start) >= 0).
fn convex_polygon(p: &GpuPath) -> Option<Vec<[f32; 2]>> {
    let t = &p.tris;
    if t.len() < 3 || t.len() % 3 != 0 || t.len() / 3 + 2 > MAX_POLY + 8 {
        return None;
    }
    let mut pts = vec![t[0], t[1], t[2]];
    for k in 1..t.len() / 3 {
        if t[3 * k] != t[0] || t[3 * k + 1] != t[3 * k - 1] {
            return None;
        }
        pts.push(t[3 * k + 2]);
    }
    pts.dedup();
    while pts.len() > 1 && pts.first() == pts.last() {
        pts.pop();
    }
    let n = pts.len();
    if !(3..=MAX_POLY).contains(&n) {
        return None;
    }
    let (mut sign, mut turn) = (0.0f32, 0.0f32);
    for i in 0..n {
        let (a, b, c) = (pts[i], pts[(i + 1) % n], pts[(i + 2) % n]);
        let (e1, e2) = ([b[0] - a[0], b[1] - a[1]], [c[0] - b[0], c[1] - b[1]]);
        let cross = e1[0] * e2[1] - e1[1] * e2[0];
        let dot = e1[0] * e2[0] + e1[1] * e2[1];
        if cross.abs() > 1e-6 * (e1[0].hypot(e1[1]) * e2[0].hypot(e2[1])) {
            if sign == 0.0 {
                sign = cross.signum();
            } else if cross.signum() != sign {
                return None;
            }
        }
        turn += cross.atan2(dot);
    }
    if sign == 0.0 || (turn.abs() - std::f32::consts::TAU).abs() > 1e-2 {
        return None;
    }
    if sign < 0.0 {
        pts.reverse();
    }
    Some(pts)
}

/// The pixel centres whose pixel square meets the convex polygon `p`: `p`
/// grown by half a pixel each way (its sum with the square), as a convex
/// hull.
fn grown_hull(p: &[[f32; 2]]) -> Vec<[f32; 2]> {
    let d = 0.5;
    let mut q: Vec<[f32; 2]> = p.iter().flat_map(|v| [[v[0] - d, v[1] - d], [v[0] + d, v[1] - d], [v[0] + d, v[1] + d], [v[0] - d, v[1] + d]]).collect();
    q.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    q.dedup();
    let cross = |o: [f32; 2], a: [f32; 2], b: [f32; 2]| (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0]);
    let mut hull: Vec<[f32; 2]> = Vec::with_capacity(q.len() + 1);
    for pass in 0..2 {
        let start = hull.len();
        let it: Box<dyn Iterator<Item = &[f32; 2]>> = if pass == 0 { Box::new(q.iter()) } else { Box::new(q.iter().rev()) };
        for &v in it {
            while hull.len() >= start + 2 && cross(hull[hull.len() - 2], hull[hull.len() - 1], v) <= 0.0 {
                hull.pop();
            }
            hull.push(v);
        }
        hull.pop();
    }
    hull
}

/// One tile of a fill: its canvas pixels, painted from its coverage.
struct Piece {
    /// The coverage pass (atlas use) it belongs to.
    pass: usize,
    blend: GpuBlend,
    /// Canvas pixels `[x0, y0, x1, y1]`.
    px: [u32; 4],
    /// Its paint quad's vertices.
    paint: std::ops::Range<u32>,
    /// In the atlas (not an analytic rect): its instance, atlas origin,
    /// quad (canvas pixels, in `geom`), fan, rule and clip.
    atlas: Option<AtlasPiece>,
}

struct AtlasPiece {
    inst: u32,
    at: [u32; 2],
    ss: u32,
    quad: std::ops::Range<u32>,
    fan: std::ops::Range<u32>,
    eo: bool,
    clip: Option<usize>,
}

/// The fills `draws` (no puts) painted onto `target`: each coverage pass
/// draws the coverage of the fills that fit the atlas, then they are
/// painted in order.
fn encode_fills(
    cg: &mut CanvasGpu,
    device: &wgpu::Device,
    encoder: &mut wgpu::CommandEncoder,
    target: &CachedTexture,
    size: (u32, u32),
    draws: &[GpuDraw],
) {
    let (w, h) = size;
    // The atlas: as large as this canvas at SS needs, up to ATLAS_SIZE (a
    // larger one kept from another canvas is used as it is).
    let want = (w.max(h) * SS).next_power_of_two().clamp(256, ATLAS_SIZE).min(device.limits().max_texture_dimension_2d);
    let n = cg.atlas.as_ref().map_or(want, |a| a.size.max(want));
    let ndc = |x: f32, y: f32| [x / w as f32 * 2.0 - 1.0, 1.0 - y / h as f32 * 2.0];
    let corners = |r: [f32; 4]| {
        let [x0, y0, x1, y1] = r;
        [[x0, y0], [x1, y0], [x1, y1], [x0, y0], [x1, y1], [x0, y1]]
    };
    let mut geom: Vec<Pos> = Vec::new();
    let mut insts: Vec<Inst> = Vec::new();
    let mut paints: Vec<PaintVertex> = Vec::new();
    let mut grads: Vec<GradRecord> = Vec::new();
    let mut polys: Vec<[f32; 2]> = Vec::new();
    let mut grad_index: HashMap<usize, usize> = HashMap::new();
    // A clip's link fans (range, even-odd), by clip id.
    let mut clips: HashMap<usize, Vec<(std::ops::Range<u32>, bool)>> = HashMap::new();
    let mut pieces: Vec<Piece> = Vec::new();
    let mut pass = 0usize;
    let mut pass_uses_atlas = vec![false];
    let (mut sx, mut sy, mut shelf) = (0u32, 0u32, 0u32);
    let push_geom = |geom: &mut Vec<Pos>, pts: &[Pos]| {
        let a = geom.len() as u32;
        geom.extend_from_slice(pts);
        a..geom.len() as u32
    };
    let push_paint = |paints: &mut Vec<PaintVertex>, px: [u32; 4], color: [f32; 4], cov: [f32; 4], rect: [f32; 4]| {
        let a = paints.len() as u32;
        let r = [px[0] as f32, px[1] as f32, px[2] as f32, px[3] as f32];
        paints.extend(corners(r).iter().map(|p| PaintVertex { pos: ndc(p[0], p[1]), color, cov, rect }));
        a..paints.len() as u32
    };
    for d in draws {
        let GpuDraw::Fill { path, paint, blend, clip } = d else { continue };
        let color = match paint {
            _ if *blend == GpuBlend::Clear => [0.0, 0.0, 0.0, 1.0],
            GpuPaint::Solid(c) => *c,
            GpuPaint::Gradient(g) => {
                let k = *grad_index.entry(Arc::as_ptr(g) as usize).or_insert_with(|| {
                    grads.push(GradRecord::of(g));
                    grads.len() - 1
                });
                [-(k as f32 + 1.0), 0.0, 0.0, 0.0]
            }
        };
        let code = blend_code(*blend);
        // The canvas pixels the fill can touch.
        let b = path.bounds;
        let px = [
            b[0].max(0.0).floor() as u32,
            b[1].max(0.0).floor() as u32,
            (b[2].ceil().max(0.0) as u32).min(w),
            (b[3].ceil().max(0.0) as u32).min(h),
        ];
        if px[0] >= px[2] || px[1] >= px[3] {
            continue;
        }
        if let (None, Some(r)) = (clip, axis_rect(path)) {
            let paint = push_paint(&mut paints, px, color, [0.0, 0.0, 1.0, code], r);
            pieces.push(Piece { pass, blend: *blend, px, paint, atlas: None });
            continue;
        }
        if let (None, Some(poly)) = (clip, convex_polygon(path)) {
            let cov = [polys.len() as f32, poly.len() as f32, 2.0, code];
            polys.extend_from_slice(&poly);
            let hull = grown_hull(&poly);
            let a = paints.len() as u32;
            for k in 1..hull.len().saturating_sub(1) {
                for v in [hull[0], hull[k], hull[k + 1]] {
                    paints.push(PaintVertex { pos: ndc(v[0], v[1]), color, cov, rect: [0.0; 4] });
                }
            }
            pieces.push(Piece { pass, blend: *blend, px, paint: a..paints.len() as u32, atlas: None });
            continue;
        }
        let fan = push_geom(&mut geom, &path.tris);
        let clip_id = clip.as_ref().map(|c: &Arc<GpuClip>| {
            if !clips.contains_key(&c.id) {
                let links = c.links.iter().map(|l| (push_geom(&mut geom, &l.tris), l.even_odd)).collect();
                clips.insert(c.id, links);
            }
            c.id
        });
        let area = (px[2] - px[0]) as u64 * (px[3] - px[1]) as u64;
        let ss = fill_ss(area);
        let tile = n / ss;
        let mut ty = px[1];
        while ty < px[3] {
            let ty1 = (ty + tile).min(px[3]);
            let mut tx = px[0];
            while tx < px[2] {
                let tx1 = (tx + tile).min(px[2]);
                let (tw, th) = ((tx1 - tx) * ss, (ty1 - ty) * ss);
                if sx + tw > n {
                    sy += shelf;
                    sx = 0;
                    shelf = 0;
                }
                if sy + th > n {
                    pass += 1;
                    pass_uses_atlas.push(false);
                    (sx, sy, shelf) = (0, 0, 0);
                }
                let at = [sx, sy];
                sx += tw;
                shelf = shelf.max(th);
                pass_uses_atlas[pass] = true;
                let inst = insts.len() as u32;
                insts.push(Inst {
                    map: [tx as f32, ty as f32, at[0] as f32, at[1] as f32],
                    size: [n as f32, n as f32, ss as f32, 0.0],
                });
                let tpx = [tx, ty, tx1, ty1];
                let quad = push_geom(&mut geom, &corners([tx as f32, ty as f32, tx1 as f32, ty1 as f32]));
                let cov = [at[0] as f32 - (tx * ss) as f32, at[1] as f32 - (ty * ss) as f32, 0.0, code];
                let paint = push_paint(&mut paints, tpx, color, cov, [ss as f32, 0.0, 0.0, 0.0]);
                pieces.push(Piece {
                    pass,
                    blend: *blend,
                    px: tpx,
                    paint,
                    atlas: Some(AtlasPiece { inst, at, ss, quad, fan: fan.clone(), eo: path.even_odd, clip: clip_id }),
                });
                tx = tx1;
            }
            ty = ty1;
        }
    }
    if pieces.is_empty() {
        return;
    }
    // Pipelines first (they borrow the cache mutably).
    let mut needs_dst = false;
    for p in &pieces {
        if linear_in_coverage(p.blend) {
            cg.pipe(device, Pipe::Paint { blend: p.blend });
        } else {
            needs_dst = true;
            cg.pipe(device, Pipe::PaintDst);
        }
        if let Some(a) = &p.atlas {
            cg.pipe(device, Pipe::Stencil { eo: a.eo });
            cg.pipe(device, Pipe::Mask { eo: a.eo, clip: a.clip.is_some() });
            if let Some(id) = a.clip {
                for (_, eo) in clips[&id].clone() {
                    cg.pipe(device, Pipe::Stencil { eo });
                    cg.pipe(device, Pipe::ClipCut { eo });
                }
                cg.pipe(device, Pipe::ClipSet);
                cg.pipe(device, Pipe::ZeroLow);
            }
        }
    }
    cg.atlas(device, n);
    if needs_dst {
        cg.dst(device, size);
    }
    let buffer = |label, contents: &[u8], usage| device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some(label), contents, usage });
    if geom.is_empty() {
        geom.push([0.0; 2]);
    }
    if insts.is_empty() {
        insts.push(Inst::zeroed());
    }
    if grads.is_empty() {
        grads.push(GradRecord::zeroed());
    }
    if polys.is_empty() {
        polys.push([0.0; 2]);
    }
    let geom_buf = buffer("canvas_gpu_geometry", bytemuck::cast_slice(&geom), wgpu::BufferUsages::VERTEX);
    let inst_buf = buffer("canvas_gpu_tiles", bytemuck::cast_slice(&insts), wgpu::BufferUsages::VERTEX);
    let paint_buf = buffer("canvas_gpu_paint", bytemuck::cast_slice(&paints), wgpu::BufferUsages::VERTEX);
    let grad_buf = buffer("canvas_gpu_gradients", bytemuck::cast_slice(&grads), wgpu::BufferUsages::STORAGE);
    let poly_buf = buffer("canvas_gpu_polygons", bytemuck::cast_slice(&polys), wgpu::BufferUsages::STORAGE);
    let cg = &*cg;
    let atlas = cg.atlas.as_ref().expect("atlas");
    let dst_view = cg.dst.as_ref().map_or(&cg.dst_dummy, |d| &d.view);
    let paint_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        layout: &cg.paint_bgl,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: grad_buf.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&atlas.resolved) },
            wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(dst_view) },
            wgpu::BindGroupEntry { binding: 3, resource: poly_buf.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::Sampler(&cg.sampler) },
        ],
        label: Some("canvas_gpu_paint"),
    });
    for (k, uses_atlas) in pass_uses_atlas.iter().enumerate() {
        let here: Vec<&Piece> = pieces.iter().filter(|p| p.pass == k).collect();
        if *uses_atlas {
            let (view, resolve_target, store) = if ATLAS_SAMPLES > 1 {
                (&atlas.color, Some(&atlas.resolved), wgpu::StoreOp::Discard)
            } else {
                (&atlas.resolved, None, wgpu::StoreOp::Store)
            };
            let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("canvas_gpu_coverage"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    resolve_target,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &atlas.stencil,
                    depth_ops: None,
                    stencil_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(0), store: wgpu::StoreOp::Discard }),
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            rp.set_vertex_buffer(0, geom_buf.slice(..));
            rp.set_vertex_buffer(1, inst_buf.slice(..));
            let step = |rp: &mut wgpu::RenderPass, p: Pipe, reference: u32, range: &std::ops::Range<u32>, inst: u32| {
                if range.is_empty() {
                    return;
                }
                rp.set_pipeline(&cg.pipes[&p]);
                rp.set_stencil_reference(reference);
                rp.draw(range.clone(), inst..inst + 1);
            };
            for p in &here {
                let Some(a) = &p.atlas else { continue };
                let (tw, th) = ((p.px[2] - p.px[0]) * a.ss, (p.px[3] - p.px[1]) * a.ss);
                rp.set_scissor_rect(a.at[0], a.at[1], tw, th);
                if let Some(id) = a.clip {
                    step(&mut rp, Pipe::ClipSet, 0xff, &a.quad, a.inst);
                    for (fan, eo) in &clips[&id] {
                        step(&mut rp, Pipe::Stencil { eo: *eo }, 0, fan, a.inst);
                        step(&mut rp, Pipe::ClipCut { eo: *eo }, 0, &a.quad, a.inst);
                        step(&mut rp, Pipe::ZeroLow, 0, &a.quad, a.inst);
                    }
                }
                step(&mut rp, Pipe::Stencil { eo: a.eo }, 0, &a.fan, a.inst);
                let reference = match (a.eo, a.clip.is_some()) {
                    (false, false) => 0,
                    (false, true) => 0x80,
                    (true, false) => 0x01,
                    (true, true) => 0x81,
                };
                step(&mut rp, Pipe::Mask { eo: a.eo, clip: a.clip.is_some() }, reference, &a.quad, a.inst);
            }
        }
        let begin = |encoder: &mut wgpu::CommandEncoder| {
            let mut rp = encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("canvas_gpu_paint"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &target.view,
                        resolve_target: None,
                        ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                })
                .forget_lifetime();
            rp.set_bind_group(0, &paint_bind, &[]);
            rp.set_vertex_buffer(0, paint_buf.slice(..));
            rp
        };
        let mut i = 0;
        while i < here.len() {
            if linear_in_coverage(here[i].blend) {
                let mut rp = begin(encoder);
                while i < here.len() && linear_in_coverage(here[i].blend) {
                    rp.set_pipeline(&cg.pipes[&Pipe::Paint { blend: here[i].blend }]);
                    rp.draw(here[i].paint.clone(), 0..1);
                    i += 1;
                }
            } else {
                let p = here[i];
                let dst = cg.dst.as_ref().expect("destination copy");
                let (x, y) = (p.px[0], p.px[1]);
                encoder.copy_texture_to_texture(
                    wgpu::TexelCopyTextureInfo { texture: &target.texture, mip_level: 0, origin: wgpu::Origin3d { x, y, z: 0 }, aspect: wgpu::TextureAspect::All },
                    wgpu::TexelCopyTextureInfo { texture: &dst.texture, mip_level: 0, origin: wgpu::Origin3d { x, y, z: 0 }, aspect: wgpu::TextureAspect::All },
                    wgpu::Extent3d { width: p.px[2] - x, height: p.px[3] - y, depth_or_array_layers: 1 },
                );
                let mut rp = begin(encoder);
                rp.set_pipeline(&cg.pipes[&Pipe::PaintDst]);
                rp.draw(p.paint.clone(), 0..1);
                i += 1;
            }
        }
    }
}

#[cfg(test)]
#[path = "canvas_gpu_tests.rs"]
mod canvas_gpu_tests;

#[cfg(test)]
#[path = "canvas_gpu_chrome_tests.rs"]
mod canvas_gpu_chrome_tests;


