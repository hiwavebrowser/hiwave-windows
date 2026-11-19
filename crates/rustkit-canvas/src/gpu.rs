//! GPU canvas A1 (docs/design/GPU_CANVAS_2D.md, "Plan of record: option
//! A"): the recording handed to the presenter, which replays it into a
//! texture per canvas, and the lowering of each op into what the GPU draws
//! (device-space triangles for a stencil-then-cover fill, a colour, a
//! blend, the clip chain), done on the presenter's thread.
//!
//! Behind `HIWAVE_GPU_CANVAS=1` (or [`set_gpu_canvas`] on a thread), read
//! when a context is created; off (the default), nothing here runs.
//!
//! What the GPU replay draws: fills and strokes with a solid colour or a
//! linear or radial gradient (GPU-GRADIENT: at most [`MAX_GRADIENT_STOPS`]
//! stops, evaluated per pixel in the shader as tiny-skia does), clears,
//! `putImageData`, under the composite operations fixed-function blending
//! can do (source-over, the Porter-Duff set, lighter, copy, xor) and under
//! a clip chain of at most 32 `clip()`s. Anything else (a pattern style,
//! `drawImage`, the blend modes that read the destination) makes the batch
//! a reset: the CPU bitmap is brought up to date and uploaded, today's
//! path, for that frame.

use crate::raster::{build_path, skia_stroke, transform, ClipLink, Op};
use crate::{CanvasStyle, CompositeOperation, PathCommand};
use std::sync::{Arc, OnceLock};
use tiny_skia::PathSegment;

thread_local! {
    static OVERRIDE: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}

/// Whether a context created now on this thread keeps a GPU picture: the
/// thread's [`set_gpu_canvas`] if set, else `HIWAVE_GPU_CANVAS=1`.
pub fn gpu_canvas_enabled() -> bool {
    static ENV: OnceLock<bool> = OnceLock::new();
    OVERRIDE.with(|o| o.get()).unwrap_or_else(|| *ENV.get_or_init(|| std::env::var("HIWAVE_GPU_CANVAS").is_ok_and(|v| v == "1")))
}

/// Tests: the switch for contexts created on this thread from now on
/// (`None`: back to the environment).
pub fn set_gpu_canvas(on: Option<bool>) {
    OVERRIDE.with(|o| o.set(on));
}

/// What the GPU picture of a canvas starts over from.
#[derive(Debug, Clone)]
pub enum GpuReset {
    /// Transparent black (nothing drawn yet).
    Blank,
    /// The CPU bitmap, premultiplied RGBA rows.
    Pixels(Vec<u8>),
}

/// The ops of one canvas handed from the page thread to the presenter at
/// one display-list build: the ops since the last batch, or a reset.
#[derive(Debug, Clone)]
pub struct GpuBatch {
    pub width: u32,
    pub height: u32,
    /// Set: the texture starts over from this before the batch's ops (a
    /// reset from the bitmap holds no ops).
    pub reset: Option<GpuReset>,
    ops: Vec<Arc<Op>>,
}

/// The composite operations the GPU replay draws, as fixed-function
/// blends of premultiplied colour (`Clear` is a clearRect).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GpuBlend {
    Clear,
    SourceOver,
    SourceIn,
    SourceOut,
    SourceAtop,
    DestinationOver,
    DestinationIn,
    DestinationOut,
    DestinationAtop,
    Lighter,
    Copy,
    Xor,
}

impl GpuBlend {
    fn of(op: CompositeOperation) -> Option<Self> {
        Some(match op {
            CompositeOperation::SourceOver => Self::SourceOver,
            CompositeOperation::SourceIn => Self::SourceIn,
            CompositeOperation::SourceOut => Self::SourceOut,
            CompositeOperation::SourceAtop => Self::SourceAtop,
            CompositeOperation::DestinationOver => Self::DestinationOver,
            CompositeOperation::DestinationIn => Self::DestinationIn,
            CompositeOperation::DestinationOut => Self::DestinationOut,
            CompositeOperation::DestinationAtop => Self::DestinationAtop,
            CompositeOperation::Lighter => Self::Lighter,
            CompositeOperation::Copy => Self::Copy,
            CompositeOperation::Xor => Self::Xor,
            CompositeOperation::Multiply
            | CompositeOperation::Screen
            | CompositeOperation::Overlay
            | CompositeOperation::Darken
            | CompositeOperation::Lighten => return None,
        })
    }
}

/// A path as the GPU fills it: a triangle fan per contour (device pixels),
/// drawn into the stencil, then its bounds covered where the stencil says
/// inside.
#[derive(Debug, Clone, Default)]
pub struct GpuPath {
    pub tris: Vec<[f32; 2]>,
    /// `[x0, y0, x1, y1]`, device pixels.
    pub bounds: [f32; 4],
    pub even_odd: bool,
}

/// A clip chain: the stencil's clip bit is the intersection of its links.
#[derive(Debug)]
pub struct GpuClip {
    /// Unique while the batch that lowered it is alive.
    pub id: usize,
    pub links: Vec<GpuPath>,
}

/// The most colour stops a gradient the GPU draws has (after the CPU's own
/// remapping of a radial gradient's start radius, which adds one); a
/// gradient with more is drawn from the CPU bitmap.
pub const MAX_GRADIENT_STOPS: usize = 16;

/// A gradient as the shader evaluates it, per pixel at the pixel's centre,
/// as tiny-skia does for the CPU bitmap: the device point taken to the
/// gradient's user space by `inv`, `t` from the geometry, padded to
/// [0, 1], the stops interpolated unpremultiplied, then premultiplied.
#[derive(Debug, Clone, PartialEq)]
pub struct GpuGradient {
    /// Two-point conical (tiny-skia's radial: from the point `p0`, radius
    /// 0, to the circle at `p1` of radius `r1`) rather than linear.
    pub radial: bool,
    /// Device to user space: `[a, b, c, d, e, f]`, x' = a x + c y + e,
    /// y' = b x + d y + f.
    pub inv: [f32; 6],
    pub p0: [f32; 2],
    pub p1: [f32; 2],
    pub r1: f32,
    /// `(offset, [r, g, b, a])`, unpremultiplied, 0..1, offsets ascending.
    pub stops: Vec<(f32, [f32; 4])>,
}

/// What a fill is covered with.
#[derive(Debug, Clone)]
pub enum GpuPaint {
    /// A premultiplied colour.
    Solid([f32; 4]),
    Gradient(Arc<GpuGradient>),
}

/// One thing the GPU replay draws.
#[derive(Debug, Clone)]
pub enum GpuDraw {
    /// `path` covered with `paint` under `blend`.
    Fill { path: GpuPath, paint: GpuPaint, blend: GpuBlend, clip: Option<Arc<GpuClip>> },
    /// putImageData: premultiplied rows written as they are.
    Put { x: u32, y: u32, width: u32, height: u32, rgba: Vec<u8> },
}

/// Whether the GPU replay draws `op` at A1.
pub(crate) fn supported(op: &Op) -> bool {
    let clip_ok = |c: Option<&Arc<crate::raster::Clip>>| c.is_none_or(|c| c.chain.is_some());
    match op {
        Op::Fill { d, style, .. } | Op::Stroke { d, style, .. } => {
            paint_of(style, &d.t).is_some() && GpuBlend::of(d.op).is_some() && clip_ok(d.clip.as_ref())
        }
        Op::Clear { clip, .. } => clip_ok(clip.as_ref()),
        Op::Put { .. } => true,
        Op::Image { .. } => false,
    }
}

/// Curves are cut into lines no further than this from the curve, in
/// device pixels. (GPU-AA: at 0.2 the chords alone put thin curved
/// strokes past the CPU's pin bounds against Chrome, whatever the
/// anti-aliasing; 0.1 brings them within, measured on every raster pin.)
const TOLERANCE: f32 = 0.1;

impl GpuBatch {
    pub(crate) fn reset(size: (u32, u32), pixels: Option<Vec<u8>>) -> Self {
        let reset = match pixels {
            Some(p) => GpuReset::Pixels(p),
            None => GpuReset::Blank,
        };
        Self { width: size.0, height: size.1, reset: Some(reset), ops: Vec::new() }
    }

    pub(crate) fn ops(size: (u32, u32), ops: Vec<Arc<Op>>) -> Self {
        Self { width: size.0, height: size.1, reset: None, ops }
    }

    /// Ops in the batch.
    pub fn op_count(&self) -> usize {
        self.ops.len()
    }

    /// The bytes the batch holds alone, estimated (ops and reset pixels;
    /// what ops share with the recording is not counted).
    pub fn bytes(&self) -> usize {
        let reset = match &self.reset {
            Some(GpuReset::Pixels(p)) => p.len(),
            _ => 0,
        };
        reset + self.ops.iter().map(|op| op.bytes()).sum::<usize>()
    }

    /// The batch's ops as the GPU draws them, in order. Run on the
    /// presenter's thread: this is the path building, stroking and curve
    /// flattening the page thread no longer does.
    pub fn lower(&self) -> Vec<GpuDraw> {
        self.lower_within(usize::MAX).map(|(d, _)| d).unwrap_or_default()
    }

    /// [`Self::lower`], unless the GPU replay of the batch would take more
    /// than `max_vertices` vertices (the renderer's per-replay cap: a few
    /// MB of curves can flatten to far more): `None` then, and lowering
    /// stops as soon as it knows, so no more than about the cap is ever
    /// built. With the draws, the vertices their replay takes: every fan,
    /// each fill's coverage and paint quads, and for a clipped fill the
    /// clip's rebuild in its coverage region (each clip's fans are kept
    /// once and counted once).
    pub fn lower_within(&self, max_vertices: usize) -> Option<(Vec<GpuDraw>, usize)> {
        const QUAD: usize = 6;
        let left = std::cell::Cell::new(max_vertices);
        let take = |n: usize| -> Option<()> {
            let l = left.get().checked_sub(n)?;
            left.set(l);
            Some(())
        };
        let mut clips: std::collections::HashMap<usize, Arc<GpuClip>> = Default::default();
        let mut out = Vec::with_capacity(self.ops.len());
        for op in &self.ops {
            let (clip, draw) = match &**op {
                Op::Fill { d, path, even_odd, style } => {
                    let (Some(blend), Some(Some(paint))) = (GpuBlend::of(d.op), paint_of(style, &d.t)) else { continue };
                    let Some(p) = build_path(path).and_then(|p| p.transform(transform(&d.t))) else { continue };
                    let path = flatten(&p, *even_odd, &left)?;
                    if path.tris.is_empty() {
                        continue;
                    }
                    (d.clip.as_ref(), GpuDraw::Fill { path, paint, blend, clip: None })
                }
                Op::Stroke { d, path, line, style } => {
                    let (Some(blend), Some(Some(paint))) = (GpuBlend::of(d.op), paint_of(style, &d.t)) else { continue };
                    let Some(p) = build_path(path) else { continue };
                    let ts = transform(&d.t);
                    let res_scale = tiny_skia::PathStroker::compute_resolution_scale(&ts);
                    let Some(outline) = p.stroke(&skia_stroke(&line.style()), res_scale).and_then(|o| o.transform(ts)) else {
                        continue;
                    };
                    let path = flatten(&outline, false, &left)?;
                    if path.tris.is_empty() {
                        continue;
                    }
                    (d.clip.as_ref(), GpuDraw::Fill { path, paint, blend, clip: None })
                }
                Op::Clear { clip, rect, t } => {
                    let [x, y, w, h] = *rect;
                    let path = if t.b == 0.0 && t.c == 0.0 {
                        // Whole pixels, as the CPU's non-anti-aliased clear:
                        // tiny-skia rounds a rect's edges to the nearest
                        // pixel (halves up).
                        let (ax, ay) = t.apply(x, y);
                        let (bx, by) = t.apply(x + w, y + h);
                        let snap = |v: f32| v.round();
                        let r = [snap(ax.min(bx)), snap(ay.min(by)), snap(ax.max(bx)), snap(ay.max(by))];
                        if r[0] >= r[2] || r[1] >= r[3] {
                            continue;
                        }
                        take(QUAD)?;
                        rect_path(r)
                    } else {
                        let Some(p) = build_path(&[PathCommand::Rect(x, y, w, h)]).and_then(|p| p.transform(transform(t))) else {
                            continue;
                        };
                        flatten(&p, false, &left)?
                    };
                    (clip.as_ref(), GpuDraw::Fill { path, paint: GpuPaint::Solid([0.0; 4]), blend: GpuBlend::Clear, clip: None })
                }
                Op::Put { rgba, width, dx, dy, .. } => {
                    if *width == 0 || *dx < 0 || *dy < 0 {
                        continue;
                    }
                    let height = (rgba.len() / 4 / *width as usize) as u32;
                    let mut px = Vec::with_capacity(rgba.len());
                    for p in rgba.chunks_exact(4) {
                        let c = tiny_skia::ColorU8::from_rgba(p[0], p[1], p[2], p[3]).premultiply();
                        px.extend_from_slice(&[c.red(), c.green(), c.blue(), c.alpha()]);
                    }
                    out.push(GpuDraw::Put { x: *dx as u32, y: *dy as u32, width: *width, height, rgba: px });
                    continue;
                }
                Op::Image { .. } => continue,
            };
            let GpuDraw::Fill { path, paint, blend, .. } = draw else { continue };
            take(2 * QUAD)?; // the coverage and paint quads
            let clip = match clip.and_then(|c| c.chain.as_ref().map(|chain| (c, chain))) {
                None => None,
                Some((c, chain)) => {
                    let id = Arc::as_ptr(c) as usize;
                    let gc = match clips.get(&id) {
                        Some(gc) => gc.clone(),
                        None => {
                            let mut links = Vec::with_capacity(chain.len());
                            for l in chain.iter() {
                                links.push(lower_link(l, &left)?);
                            }
                            let gc = Arc::new(GpuClip { id, links });
                            clips.insert(id, gc.clone());
                            gc
                        }
                    };
                    // The rebuild in this fill's region: the set quad,
                    // and per link its fan (drawn again; kept once), a cut
                    // quad and a zero quad.
                    take(QUAD)?;
                    for l in &gc.links {
                        take(2 * QUAD + l.tris.len())?;
                    }
                    Some(gc)
                }
            };
            out.push(GpuDraw::Fill { path, paint, blend, clip });
        }
        Some((out, max_vertices - left.get()))
    }
}

fn lower_link(l: &ClipLink, left: &std::cell::Cell<usize>) -> Option<GpuPath> {
    match build_path(&l.path).and_then(|p| p.transform(transform(&l.t))) {
        Some(p) => flatten(&p, l.even_odd, left),
        // No segments: the clip keeps nothing.
        None => Some(GpuPath { tris: Vec::new(), bounds: [0.0; 4], even_odd: l.even_odd }),
    }
}

/// A colour as the CPU raster takes it (alpha to 8 bits, as
/// `raster::color`), unpremultiplied, 0..1.
fn rgba(c: rustkit_css::Color) -> [f32; 4] {
    let a = (c.a.clamp(0.0, 1.0) * 255.0).round() / 255.0;
    [c.r as f32 / 255.0, c.g as f32 / 255.0, c.b as f32 / 255.0, a]
}

fn premultiplied(c: [f32; 4]) -> [f32; 4] {
    [c[0] * c[3], c[1] * c[3], c[2] * c[3], c[3]]
}

/// What `style` covers a fill with under the transform `t`, as the CPU
/// raster's `shader` decides it (globalAlpha is already in the colours):
/// `None` when the GPU does not draw it (a pattern, a gradient with more
/// than [`MAX_GRADIENT_STOPS`] stops, a transform with no finite inverse,
/// a stop offset or a radial end radius that is not finite);
/// `Some(None)` when it paints nothing (no stops, a degenerate gradient,
/// and as tiny-skia refuses them: points whose distance is not finite as
/// f32, a radial end radius that is not over 1/4096).
fn paint_of(style: &CanvasStyle, t: &crate::Transform2D) -> Option<Option<GpuPaint>> {
    let gradient = |radial: bool, p0: [f32; 2], p1: [f32; 2], r1: f32, stops: Vec<(f32, [f32; 4])>| {
        if stops.len() > MAX_GRADIENT_STOPS || stops.iter().any(|(o, _)| !o.is_finite()) {
            return None;
        }
        // tiny-skia's gradient constructors (the CPU picture): a length
        // that is not finite, or a two-point conical radius that is
        // negative or nearly zero, make no shader, so nothing is painted.
        // An end radius that is not finite does make one (it paints a
        // degenerate colour): the CPU bitmap draws that.
        if !r1.is_finite() {
            return None;
        }
        let length = ((p1[0] - p0[0]).powi(2) + (p1[1] - p0[1]).powi(2)).sqrt();
        if !length.is_finite() || (radial && r1 <= 1.0 / 4096.0) {
            return Some(None);
        }
        if !radial && length <= 1.0 / 32768.0 {
            // tiny-skia's degenerate linear gradient (padded): its last colour.
            return Some(Some(GpuPaint::Solid(premultiplied(stops.last()?.1))));
        }
        let inv = t.inverse()?;
        if ![inv.a, inv.b, inv.c, inv.d, inv.e, inv.f].iter().all(|v| v.is_finite()) {
            return None;
        }
        Some(Some(GpuPaint::Gradient(Arc::new(GpuGradient {
            radial,
            inv: [inv.a, inv.b, inv.c, inv.d, inv.e, inv.f],
            p0,
            p1,
            r1,
            stops,
        }))))
    };
    match style {
        CanvasStyle::Color(c) => Some(Some(GpuPaint::Solid(premultiplied(rgba(*c))))),
        CanvasStyle::LinearGradient(g) => {
            if g.stops.is_empty() || (g.x0 == g.x1 && g.y0 == g.y1) {
                return Some(None);
            }
            if g.stops.len() == 1 {
                return Some(Some(GpuPaint::Solid(premultiplied(rgba(g.stops[0].color)))));
            }
            let stops = g.stops.iter().map(|s| (s.offset, rgba(s.color))).collect();
            gradient(false, [g.x0, g.y0], [g.x1, g.y1], 0.0, stops)
        }
        CanvasStyle::RadialGradient(g) => {
            if g.stops.is_empty() || (g.x0 == g.x1 && g.y0 == g.y1 && g.r0 == g.r1) {
                return Some(None);
            }
            if g.stops.len() == 1 {
                return Some(Some(GpuPaint::Solid(premultiplied(rgba(g.stops[0].color)))));
            }
            // The CPU raster's remapping of a start radius (raster.rs
            // `shader`): offset t at (r0 + t (r1 - r0)) / r1 of the end
            // circle, and the first colour inside the start circle.
            let mut stops: Vec<(f32, [f32; 4])> = g.stops.iter().map(|s| (s.offset, rgba(s.color))).collect();
            if g.r0 > 0.0 && g.r1 > 0.0 {
                let k = g.r0 / g.r1;
                stops = g.stops.iter().map(|s| (k + s.offset * (1.0 - k), rgba(s.color))).collect();
                stops.insert(0, (0.0, rgba(g.stops[0].color)));
            }
            gradient(true, [g.x0, g.y0], [g.x1, g.y1], g.r1, stops)
        }
        CanvasStyle::Pattern(_) => None,
    }
}

fn rect_path(r: [f32; 4]) -> GpuPath {
    let [x0, y0, x1, y1] = r;
    GpuPath { tris: vec![[x0, y0], [x1, y0], [x1, y1], [x0, y0], [x1, y1], [x0, y1]], bounds: r, even_odd: false }
}

/// `path` (device space) as triangle fans, one per contour, curves cut to
/// lines within [`TOLERANCE`]; its vertices are taken from `left`, and
/// `None` (stopping early) when they would not fit.
fn flatten(path: &tiny_skia::Path, even_odd: bool, left: &std::cell::Cell<usize>) -> Option<GpuPath> {
    let cap = left.get();
    let mut tris = Vec::new();
    let mut contour: Vec<[f32; 2]> = Vec::new();
    let mut bounds = [f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY];
    let fan = |contour: &mut Vec<[f32; 2]>, tris: &mut Vec<[f32; 2]>| {
        if contour.len() >= 3 {
            let p0 = contour[0];
            for w in contour[1..].windows(2) {
                tris.extend_from_slice(&[p0, w[0], w[1]]);
            }
        }
        contour.clear();
    };
    let mut last = [0.0f32; 2];
    for seg in path.segments() {
        match seg {
            PathSegment::MoveTo(p) => {
                fan(&mut contour, &mut tris);
                last = [p.x, p.y];
                contour.push(last);
            }
            PathSegment::LineTo(p) => {
                last = [p.x, p.y];
                contour.push(last);
            }
            PathSegment::QuadTo(c, p) => {
                let (p0, c, p1) = (last, [c.x, c.y], [p.x, p.y]);
                let dd = len([p0[0] - 2.0 * c[0] + p1[0], p0[1] - 2.0 * c[1] + p1[1]]);
                let n = steps((dd / (8.0 * TOLERANCE)).sqrt());
                for i in 1..=n {
                    let t = i as f32 / n as f32;
                    let u = 1.0 - t;
                    contour.push([
                        u * u * p0[0] + 2.0 * u * t * c[0] + t * t * p1[0],
                        u * u * p0[1] + 2.0 * u * t * c[1] + t * t * p1[1],
                    ]);
                }
                last = p1;
            }
            PathSegment::CubicTo(c1, c2, p) => {
                let (p0, c1, c2, p1) = (last, [c1.x, c1.y], [c2.x, c2.y], [p.x, p.y]);
                let d1 = len([p0[0] - 2.0 * c1[0] + c2[0], p0[1] - 2.0 * c1[1] + c2[1]]);
                let d2 = len([c1[0] - 2.0 * c2[0] + p1[0], c1[1] - 2.0 * c2[1] + p1[1]]);
                let n = steps((3.0 * d1.max(d2) / (4.0 * TOLERANCE)).sqrt());
                for i in 1..=n {
                    let t = i as f32 / n as f32;
                    let u = 1.0 - t;
                    let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
                    contour.push([
                        a * p0[0] + b * c1[0] + c * c2[0] + d * p1[0],
                        a * p0[1] + b * c1[1] + c * c2[1] + d * p1[1],
                    ]);
                }
                last = p1;
            }
            PathSegment::Close => {}
        }
        // A contour of n points fans into 3 (n - 2) vertices.
        if tris.len().saturating_add(contour.len().saturating_mul(3)) > cap.saturating_add(6) {
            return None;
        }
    }
    fan(&mut contour, &mut tris);
    if tris.len() > cap {
        return None;
    }
    left.set(cap - tris.len());
    for p in &tris {
        bounds = [bounds[0].min(p[0]), bounds[1].min(p[1]), bounds[2].max(p[0]), bounds[3].max(p[1])];
    }
    if tris.is_empty() {
        bounds = [0.0; 4];
    }
    Some(GpuPath { tris, bounds, even_odd })
}

fn len(v: [f32; 2]) -> f32 {
    (v[0] * v[0] + v[1] * v[1]).sqrt()
}

fn steps(n: f32) -> usize {
    if n.is_finite() {
        (n.ceil() as usize).clamp(1, 256)
    } else {
        1
    }
}

/// How far one picture is from another (A3's hook; not a gate at A1): two
/// `width` x `height` premultiplied RGBA images, unpremultiplied and
/// compared per pixel. A pixel is *interior* when in `reference` it and its
/// 8 neighbours are all the same colour (A3's rule says all alpha 255 or
/// all alpha 0; that counts the edge of one opaque shape drawn over another
/// as interior, so the colour is compared too); every other pixel is an
/// *edge* pixel.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PixelDiff {
    pub pixels: usize,
    /// Pixels whose largest channel difference is over the tolerance.
    pub differing: usize,
    pub interior_over: usize,
    pub edge_pixels: usize,
    pub edge_over: usize,
    /// Mean |Δα| over edge pixels.
    pub edge_mean_alpha: f64,
    /// The largest channel difference anywhere.
    pub max: u8,
    /// |Σα − Σα_ref| / Σα_ref (0 when both are empty).
    pub coverage_delta: f64,
}

impl PixelDiff {
    pub fn differing_pct(&self) -> f64 {
        if self.pixels == 0 {
            0.0
        } else {
            100.0 * self.differing as f64 / self.pixels as f64
        }
    }
}

/// [`PixelDiff`] of `image` against `reference`, `tolerance` per channel.
pub fn compare_premultiplied(image: &[u8], reference: &[u8], width: u32, height: u32, tolerance: u8) -> PixelDiff {
    let (w, h) = (width as usize, height as usize);
    let mut d = PixelDiff { pixels: w * h, ..Default::default() };
    if image.len() != w * h * 4 || reference.len() != w * h * 4 {
        d.differing = d.pixels;
        d.max = 255;
        return d;
    }
    let un = |p: &[u8]| crate::raster::unpremultiply_px(p[0], p[1], p[2], p[3]);
    let px = |x: usize, y: usize| &reference[(y * w + x) * 4..(y * w + x) * 4 + 4];
    let (mut sum_a, mut sum_r, mut edge_sum) = (0u64, 0u64, 0u64);
    for y in 0..h {
        for x in 0..w {
            let i = (y * w + x) * 4;
            let (a, r) = (un(&image[i..i + 4]), un(&reference[i..i + 4]));
            let m = (0..4).map(|c| a[c].abs_diff(r[c])).max().unwrap_or(0);
            sum_a += a[3] as u64;
            sum_r += r[3] as u64;
            d.max = d.max.max(m);
            let over = m > tolerance;
            d.differing += over as usize;
            let mut flat = true;
            for yy in y.saturating_sub(1)..=(y + 1).min(h - 1) {
                for xx in x.saturating_sub(1)..=(x + 1).min(w - 1) {
                    flat &= px(xx, yy) == px(x, y);
                }
            }
            if flat {
                d.interior_over += over as usize;
            } else {
                d.edge_pixels += 1;
                d.edge_over += over as usize;
                edge_sum += a[3].abs_diff(r[3]) as u64;
            }
        }
    }
    if d.edge_pixels > 0 {
        d.edge_mean_alpha = edge_sum as f64 / d.edge_pixels as f64;
    }
    if sum_r > 0 {
        d.coverage_delta = (sum_a as f64 - sum_r as f64).abs() / sum_r as f64;
    } else if sum_a > 0 {
        d.coverage_delta = 1.0;
    }
    d
}

#[cfg(test)]
mod gpu_recording_tests {
    use super::*;
    use crate::CanvasRenderingContext2D;

    fn gpu_ctx(w: u32, h: u32) -> CanvasRenderingContext2D {
        set_gpu_canvas(Some(true));
        let c = CanvasRenderingContext2D::new(w, h);
        set_gpu_canvas(None);
        c
    }

    /// A read brings the bitmap up to date but keeps the ops the GPU has
    /// not taken; once both passed them they go.
    #[test]
    fn an_op_is_dropped_only_when_both_replays_passed_it() {
        let mut c = gpu_ctx(20, 20);
        assert!(c.gpu_canvas());
        let first = c.take_gpu_batch(false);
        assert!(matches!(first.reset, Some(GpuReset::Blank)), "a new context's first batch starts the texture blank");
        c.fill_rect(1.0, 1.0, 5.0, 5.0);
        c.stroke_rect(2.0, 2.0, 6.0, 6.0);
        assert_eq!(c.gpu_pending_ops(), 2);
        let _ = c.premultiplied_pixels();
        assert_eq!(c.pending_ops(), 0, "the CPU replayed them");
        assert_eq!(c.gpu_pending_ops(), 2, "the GPU has not taken them: kept");
        let b = c.take_gpu_batch(false);
        assert!(b.reset.is_none());
        assert_eq!(b.op_count(), 2);
        assert_eq!(c.gpu_pending_ops(), 0);
        assert_eq!(c.pending_bytes(), 0, "both passed: dropped");
        // The other order: the GPU first, then a read.
        c.fill_rect(0.0, 0.0, 3.0, 3.0);
        assert_eq!(c.take_gpu_batch(false).op_count(), 1);
        assert_eq!(c.pending_ops(), 1, "the CPU has not replayed it: kept");
        let _ = c.premultiplied_pixels();
        assert_eq!(c.pending_bytes(), 0);
    }

    /// The pixels a read sees are the same whatever the GPU took, and the
    /// same as a context without the switch.
    #[test]
    fn reads_are_byte_identical_with_the_gpu_position() {
        let draw = |c: &mut CanvasRenderingContext2D, take: bool| {
            c.set_fill_style(CanvasStyle::Color(rustkit_css::Color::new(200, 30, 40, 0.7)));
            c.fill_rect(1.5, 2.5, 9.0, 7.0);
            if take {
                c.take_gpu_batch(false);
            }
            c.begin_path();
            c.arc(10.0, 10.0, 6.0, 0.0, 5.0, false);
            c.clip();
            c.set_line_width(3.0);
            c.stroke_rect(3.0, 3.0, 12.0, 12.0);
            c.clear_rect(4.0, 4.0, 2.0, 2.0);
            c.premultiplied_pixels()
        };
        let plain = draw(&mut CanvasRenderingContext2D::new(20, 20), false);
        assert_eq!(draw(&mut gpu_ctx(20, 20), true), plain);
        assert_eq!(draw(&mut gpu_ctx(20, 20), false), plain);
    }

    /// A clear of the whole bitmap drops what came before it in both
    /// pictures (MotionMark clears every frame: the CPU never replays a
    /// frame nobody reads), and a read after it still sees what it should.
    #[test]
    fn a_full_clear_drops_everything_before_it_for_both() {
        let mut c = gpu_ctx(16, 16);
        c.take_gpu_batch(false);
        for i in 0..50 {
            c.fill_rect(i as f32 % 10.0, 0.0, 4.0, 4.0);
        }
        c.clear_rect(0.0, 0.0, 16.0, 16.0);
        assert_eq!(c.gpu_pending_ops(), 1, "only the clear is left for the GPU");
        assert_eq!(c.pending_ops(), 1);
        c.fill_rect(2.0, 2.0, 3.0, 3.0);
        let mut plain = CanvasRenderingContext2D::new(16, 16);
        for i in 0..50 {
            plain.fill_rect(i as f32 % 10.0, 0.0, 4.0, 4.0);
        }
        plain.clear_rect(0.0, 0.0, 16.0, 16.0);
        plain.fill_rect(2.0, 2.0, 3.0, 3.0);
        assert_eq!(c.premultiplied_pixels(), plain.premultiplied_pixels());
        // A clear under a clip is not a full clear.
        let mut c = gpu_ctx(16, 16);
        c.take_gpu_batch(false);
        c.fill_rect(0.0, 0.0, 4.0, 4.0);
        c.begin_path();
        c.rect(0.0, 0.0, 8.0, 8.0);
        c.clip();
        c.clear_rect(0.0, 0.0, 16.0, 16.0);
        assert_eq!(c.gpu_pending_ops(), 2);
    }

    /// Past the cap with the GPU behind, the CPU replays and drops as at
    /// A0, and the GPU's next batch is a reset from the bitmap.
    #[test]
    fn the_gpu_behind_past_the_cap_starts_over_from_the_bitmap() {
        let mut c = gpu_ctx(64, 64);
        c.take_gpu_batch(false);
        let mut n = 0;
        while c.gpu_pending_ops() == n {
            c.fill_rect((n % 50) as f32, 1.0, 3.0, 3.0);
            n += 1;
            assert!(n < 1_000_000, "the cap never tripped");
        }
        assert!(c.pending_bytes() <= crate::PENDING_BYTE_BUDGET);
        let b = c.take_gpu_batch(false);
        let Some(GpuReset::Pixels(px)) = &b.reset else { panic!("a reset with the bitmap") };
        assert_eq!(px, &c.premultiplied_pixels());
    }

    /// A new context's first batch starts the texture blank and carries
    /// its draws: nothing is rasterized on the CPU for a first paint, even
    /// when the presenter asks for a reset then. A resize starts blank too.
    #[test]
    fn a_new_or_resized_context_starts_blank_with_its_draws() {
        let mut c = gpu_ctx(8, 8);
        c.fill_rect(0.0, 0.0, 2.0, 2.0);
        let b = c.take_gpu_batch(true);
        assert!(matches!(b.reset, Some(GpuReset::Blank)));
        assert_eq!(b.op_count(), 1);
        assert_eq!(c.pending_ops(), 1, "not rasterized on the CPU");
        c.fill_rect(1.0, 1.0, 2.0, 2.0);
        let b = c.take_gpu_batch(false);
        assert!(b.reset.is_none() && b.op_count() == 1);
        c.reset_bitmap(16, 16);
        c.fill_rect(1.0, 1.0, 2.0, 2.0);
        let b = c.take_gpu_batch(false);
        assert!(matches!(b.reset, Some(GpuReset::Blank)));
        assert_eq!((b.width, b.height, b.op_count()), (16, 16, 1));
    }

    /// An op the GPU does not draw at A1 makes the batch a reset; a forced
    /// reset (the presenter lost the texture) too.
    #[test]
    fn an_op_the_gpu_does_not_draw_makes_a_reset() {
        let mut c = gpu_ctx(8, 8);
        c.take_gpu_batch(false);
        c.fill_rect(0.0, 0.0, 2.0, 2.0);
        c.set_global_composite_operation(CompositeOperation::Multiply);
        c.fill_rect(1.0, 1.0, 2.0, 2.0);
        assert!(matches!(c.take_gpu_batch(false).reset, Some(GpuReset::Pixels(_))));
        c.set_global_composite_operation(CompositeOperation::SourceOver);
        c.fill_rect(0.0, 0.0, 2.0, 2.0);
        let b = c.take_gpu_batch(true);
        assert!(b.reset.is_some() && b.op_count() == 0);
        // A pattern too.
        let bitmap = crate::CanvasBitmap::from_rgba(2, 2, &[255; 16]).unwrap();
        c.set_fill_style(CanvasStyle::Pattern(crate::CanvasPattern {
            bitmap: Arc::new(bitmap),
            repetition: crate::PatternRepetition::Repeat,
        }));
        c.fill_rect(0.0, 0.0, 2.0, 2.0);
        assert!(c.take_gpu_batch(false).reset.is_some());
    }

    /// GPU-GRADIENT: linear and radial gradients are drawn by the GPU
    /// (no reset), lowered as the CPU raster resolves them: a radial
    /// gradient's start radius remapped into its stops, globalAlpha in the
    /// colours, the device-to-user inverse of the transform; one stop is a
    /// solid colour, no stops paint nothing; past MAX_GRADIENT_STOPS, a
    /// reset.
    #[test]
    fn gradients_lower_as_the_cpu_resolves_them() {
        let mut c = gpu_ctx(40, 40);
        c.take_gpu_batch(false);
        c.translate(10.0, 0.0);
        c.set_global_alpha(0.5);
        let mut g = crate::RadialGradient::new(20.0, 20.0, 6.0, 20.0, 20.0, 18.0);
        g.add_color_stop(0.0, rustkit_css::Color::new(255, 0, 0, 1.0));
        g.add_color_stop(1.0, rustkit_css::Color::new(0, 0, 255, 1.0));
        c.set_fill_style(CanvasStyle::RadialGradient(g));
        c.fill_rect(0.0, 0.0, 40.0, 40.0);
        let b = c.take_gpu_batch(false);
        assert!(b.reset.is_none(), "drawn by the GPU");
        let draws = b.lower();
        let GpuDraw::Fill { paint: GpuPaint::Gradient(g), .. } = &draws[0] else { panic!("a gradient: {draws:?}") };
        assert!(g.radial);
        assert_eq!(g.inv, [1.0, 0.0, 0.0, 1.0, -10.0, 0.0]);
        let k = 6.0 / 18.0;
        assert_eq!(g.stops.len(), 3);
        assert_eq!(g.stops[0].0, 0.0);
        assert!((g.stops[1].0 - k).abs() < 1e-6 && (g.stops[2].0 - 1.0).abs() < 1e-6);
        assert!((g.stops[1].1[3] - 128.0 / 255.0).abs() < 1e-6, "globalAlpha in the stops, to 8 bits");
        // One stop: solid. None: nothing.
        let mut one = crate::LinearGradient::new(0.0, 0.0, 8.0, 0.0);
        one.add_color_stop(0.5, rustkit_css::Color::new(0, 255, 0, 1.0));
        c.set_fill_style(CanvasStyle::LinearGradient(one));
        c.fill_rect(0.0, 0.0, 2.0, 2.0);
        c.set_fill_style(CanvasStyle::LinearGradient(crate::LinearGradient::new(0.0, 0.0, 8.0, 0.0)));
        c.fill_rect(0.0, 0.0, 2.0, 2.0);
        let draws = c.take_gpu_batch(false).lower();
        assert_eq!(draws.len(), 1);
        assert!(matches!(&draws[0], GpuDraw::Fill { paint: GpuPaint::Solid(_), .. }));
        let mut many = crate::LinearGradient::new(0.0, 0.0, 8.0, 0.0);
        for i in 0..=MAX_GRADIENT_STOPS {
            many.add_color_stop(i as f32 / MAX_GRADIENT_STOPS as f32, rustkit_css::Color::new(0, 0, 0, 1.0));
        }
        c.set_fill_style(CanvasStyle::LinearGradient(many));
        c.fill_rect(0.0, 0.0, 2.0, 2.0);
        assert!(c.take_gpu_batch(false).reset.is_some());
        // #1300 R1, as tiny-skia builds them: points too far apart for f32
        // or an end radius of 0 paint nothing; an end radius that is not
        // finite paints a degenerate colour there, so the CPU draws it.
        let huge = 1e300_f64 as f32;
        let stops = |g: &mut crate::RadialGradient| {
            g.add_color_stop(0.0, rustkit_css::Color::new(255, 0, 0, 1.0));
            g.add_color_stop(1.0, rustkit_css::Color::new(0, 0, 255, 1.0));
        };
        let mut far = crate::LinearGradient::new(0.0, 0.0, huge, 0.0);
        far.add_color_stop(0.0, rustkit_css::Color::new(255, 0, 0, 1.0));
        far.add_color_stop(1.0, rustkit_css::Color::new(0, 0, 255, 1.0));
        c.set_fill_style(CanvasStyle::LinearGradient(far));
        c.fill_rect(0.0, 0.0, 2.0, 2.0);
        let mut zero = crate::RadialGradient::new(5.0, 5.0, 0.0, 20.0, 20.0, 0.0);
        stops(&mut zero);
        c.set_fill_style(CanvasStyle::RadialGradient(zero));
        c.fill_rect(0.0, 0.0, 2.0, 2.0);
        let b = c.take_gpu_batch(false);
        assert!(b.reset.is_none());
        assert!(b.lower().is_empty(), "nothing painted");
        let mut inf = crate::RadialGradient::new(5.0, 5.0, 0.0, 20.0, 20.0, huge);
        stops(&mut inf);
        c.set_fill_style(CanvasStyle::RadialGradient(inf));
        c.fill_rect(0.0, 0.0, 2.0, 2.0);
        assert!(c.take_gpu_batch(false).reset.is_some(), "drawn from the CPU bitmap");
    }

    /// Off (the default), a context keeps no GPU position: nothing is
    /// kept for it and a read drops everything, as at A0.
    #[test]
    fn with_the_switch_off_nothing_is_kept_for_the_gpu() {
        set_gpu_canvas(Some(false));
        let mut c = CanvasRenderingContext2D::new(8, 8);
        set_gpu_canvas(None);
        assert!(!c.gpu_canvas());
        c.fill_rect(0.0, 0.0, 2.0, 2.0);
        assert_eq!(c.gpu_pending_ops(), 0);
        let _ = c.premultiplied_pixels();
        assert_eq!(c.pending_bytes(), 0);
    }

    /// The lowering: a filled rect is two triangles covering it; a stroke
    /// becomes its outline; a clip chain is carried; curves are flattened.
    #[test]
    fn ops_lower_to_device_space_fans() {
        let mut c = gpu_ctx(40, 40);
        c.take_gpu_batch(false);
        c.translate(10.0, 5.0);
        c.fill_rect(0.0, 0.0, 4.0, 3.0);
        c.begin_path();
        c.rect(0.0, 0.0, 20.0, 20.0);
        c.clip();
        c.begin_path();
        c.arc(10.0, 10.0, 8.0, 0.0, std::f32::consts::TAU, false);
        c.stroke();
        let draws = c.take_gpu_batch(false).lower();
        assert_eq!(draws.len(), 2);
        let GpuDraw::Fill { path, clip, blend, .. } = &draws[0] else { panic!() };
        assert_eq!(path.bounds, [10.0, 5.0, 14.0, 8.0]);
        assert!(clip.is_none());
        assert_eq!(*blend, GpuBlend::SourceOver);
        let GpuDraw::Fill { path, clip, .. } = &draws[1] else { panic!() };
        let clip = clip.as_ref().expect("the clip chain");
        assert_eq!(clip.links.len(), 1);
        assert!(path.tris.len() > 60, "the circle's outline is flattened: {}", path.tris.len());
        assert!(path.bounds[0] >= 10.0 + 2.0 - 1.0 && path.bounds[2] <= 10.0 + 18.0 + 1.0);
    }

    /// A frame of curves can lower to far more vertices than its bytes
    /// suggest (#1299 R1): lowering under a cap stops with `None` once the
    /// replay would pass it, and under a cap it fits, it says how many it
    /// takes. Counts, no clock.
    #[test]
    fn lowering_stops_at_the_vertex_cap() {
        let mut c = gpu_ctx(400, 400);
        c.take_gpu_batch(false);
        c.set_line_width(3.0);
        c.set_line_dash(vec![1.0, 1.0]);
        for i in 0..40 {
            c.begin_path();
            c.arc(200.0, 200.0, 20.0 + i as f32 * 4.0, 0.0, std::f32::consts::TAU, false);
            c.stroke();
        }
        let b = c.take_gpu_batch(false);
        let (draws, used) = b.lower_within(usize::MAX).expect("no cap");
        assert_eq!(draws.len(), 40);
        let fans: usize = draws.iter().map(|d| if let GpuDraw::Fill { path, .. } = d { path.tris.len() + 12 } else { 0 }).sum();
        assert_eq!(used, fans, "fans, and each fill's coverage and paint quads");
        assert!(used > 5_000, "40 dashed arcs: {used} vertices");
        assert!(b.lower_within(used - 1).is_none(), "one short of what it takes: refused");
        assert!(b.lower_within(used / 2).is_none());
        assert_eq!(b.lower_within(used).map(|(_, n)| n), Some(used));
    }

    #[test]
    fn the_compare_metric_tells_a_shift_from_a_match() {
        let (w, h) = (8u32, 8u32);
        let mut a = vec![0u8; 256];
        for y in 2..6 {
            for x in 2..6 {
                a[(y * 8 + x) * 4..(y * 8 + x) * 4 + 4].copy_from_slice(&[255, 0, 0, 255]);
            }
        }
        let same = compare_premultiplied(&a, &a, w, h, 2);
        assert_eq!((same.differing, same.max), (0, 0));
        let mut shifted = vec![0u8; 256];
        for y in 2..6 {
            for x in 3..7 {
                shifted[(y * 8 + x) * 4..(y * 8 + x) * 4 + 4].copy_from_slice(&[255, 0, 0, 255]);
            }
        }
        let d = compare_premultiplied(&shifted, &a, w, h, 2);
        assert_eq!(d.differing, 8);
        assert_eq!(d.max, 255);
    }
}
