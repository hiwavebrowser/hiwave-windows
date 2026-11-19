//! The canvas bitmap (STR-7): every draw on a context is rasterized into a
//! premultiplied RGBA pixmap by tiny-skia (a port of Skia's CPU pipeline,
//! BSD-3-Clause), the pixels `getImageData` reads back. The recording
//! beside it (`DrawCommand`) stays what the engine paints.
//!
//! Paths keep their curves here (the recording flattens them): arcs and
//! ellipses become cubic Béziers of at most a quarter turn each.
//!
//! GPU canvas A0 (docs/design/GPU_CANVAS_2D.md): a draw is not rasterized
//! at the call. It is recorded as an [`Op`] holding everything the raster
//! takes, and the pending ops are replayed into the bitmap, oldest first and
//! through the same calls, when something reads it ([`Recorded::flush`]), or
//! when they pass [`crate::PENDING_BYTE_BUDGET`]. A clip is recorded as its
//! path ([`Clip`]); its mask is built when a replay first needs it.

use crate::{CanvasStyle, CompositeOperation, LineCap, LineJoin, PathCommand, PatternRepetition, Transform2D};
use rustkit_css::Color;
use std::f32::consts::{PI, TAU};
use std::sync::{Arc, Mutex, OnceLock};
use tiny_skia::{
    BlendMode, FillRule, FilterQuality, GradientStop, Mask, Paint, PathBuilder, PathStroker, Pixmap, Point, Rect,
    Shader, SpreadMode, Stroke, StrokeDash, Transform,
};

/// The bitmap: `None` until something is drawn (a canvas never drawn on
/// holds no pixels), or when the bitmap has no area.
#[derive(Debug, Default)]
pub(crate) struct Raster {
    pixmap: Option<Pixmap>,
}

/// A bitmap a canvas can draw (`drawImage`) or repeat (`createPattern`):
/// another canvas's pixels or a decoded image, premultiplied.
#[derive(Debug, Clone)]
pub struct CanvasBitmap(pub(crate) Pixmap);

impl CanvasBitmap {
    /// From unpremultiplied RGBA rows; `None` when empty or the wrong length.
    pub fn from_rgba(width: u32, height: u32, rgba: &[u8]) -> Option<Self> {
        if rgba.len() as u64 != width as u64 * height as u64 * 4 {
            return None;
        }
        let mut pm = Pixmap::new(width, height)?;
        for (dst, src) in pm.pixels_mut().iter_mut().zip(rgba.chunks_exact(4)) {
            *dst = tiny_skia::ColorU8::from_rgba(src[0], src[1], src[2], src[3]).premultiply();
        }
        Some(CanvasBitmap(pm))
    }

    pub fn width(&self) -> u32 {
        self.0.width()
    }

    pub fn height(&self) -> u32 {
        self.0.height()
    }
}

/// The clip a draw is cut to: the context's clip, if any (shared by the
/// saved states and the recorded ops that hold it).
pub(crate) type ClipMask = Option<Arc<Clip>>;

/// The most clips a chain holds with no mask built: past it the newest
/// clip's mask is built at the `clip()` call, which lets go of the chain
/// before it (a page clipping forever without a restore stays bounded).
const CLIP_CHAIN_MAX: usize = 32;

/// One `clip()`: the path, its rule and transform, cut from the clip in
/// force before it (`parent`) on a `size` bitmap (HTML §4.12.5.1.10: the new
/// clip is the old one intersected with the path). Its coverage mask is
/// built on first use, by the same tiny-skia calls the clip used to make at
/// the call, so the pixels are the same.
#[derive(Debug)]
pub(crate) struct Clip {
    /// The clip before this one, until this one's mask is built.
    parent: Mutex<ClipMask>,
    /// Clips in the chain up to this one with no mask built (this one included).
    unbuilt: usize,
    size: (u32, u32),
    path: Vec<PathCommand>,
    even_odd: bool,
    t: Transform2D,
    /// `None` inside: no mask (the bitmap has no area), so nothing clips.
    mask: OnceLock<Option<Mask>>,
    /// GPU canvas A1 (switch on only): every `clip()` in force, oldest
    /// first, as paths, for the GPU replay's stencil. The mask's build lets
    /// go of `parent`, so the chain is kept here as paths only (no masks).
    /// `None` with the switch off, and past [`CLIP_CHAIN_MAX`] links (the
    /// GPU replay does not draw under such a clip: the canvas shows its CPU
    /// bitmap for that frame).
    pub(crate) chain: Option<Arc<Vec<Arc<ClipLink>>>>,
}

/// One `clip()` of a chain, as the GPU replay draws it into its stencil.
#[derive(Debug)]
pub(crate) struct ClipLink {
    pub(crate) path: Vec<PathCommand>,
    pub(crate) even_odd: bool,
    pub(crate) t: Transform2D,
}

impl Clip {
    /// The coverage mask, built (with every unbuilt clip before it, oldest
    /// first, without recursion) on first use.
    pub(crate) fn mask(&self) -> Option<&Mask> {
        if self.mask.get().is_none() {
            // The unbuilt ancestors, newest first; built oldest first, each
            // let go of as soon as its child has its mask (a child's build
            // drops its parent), so at most two masks of the chain are alive.
            let mut chain: Vec<Arc<Clip>> = Vec::new();
            let mut next = self.parent.lock().ok().and_then(|p| p.clone());
            while let Some(c) = next {
                if c.mask.get().is_some() {
                    break;
                }
                next = c.parent.lock().ok().and_then(|p| p.clone());
                chain.push(c);
            }
            while let Some(c) = chain.pop() {
                c.build();
            }
            self.build();
        }
        self.mask.get().and_then(|m| m.as_ref())
    }

    /// Builds the mask from the parent's (already built, or none) and drops
    /// the parent.
    fn build(&self) {
        let _timed = timed();
        let parent = self.parent.lock().ok().and_then(|mut p| p.take());
        self.mask.get_or_init(|| {
            let rule = if self.even_odd { FillRule::EvenOdd } else { FillRule::Winding };
            let ts = transform(&self.t);
            let path = build_path(&self.path);
            let old = match &parent {
                // A parent with no mask (no area): as if there were no clip.
                Some(p) => p.mask.get().and_then(|m| m.as_ref()),
                None => None,
            };
            let mut m = match old {
                Some(m) => m.clone(),
                None => {
                    let mut m = Mask::new(self.size.0, self.size.1)?;
                    if let Some(p) = &path {
                        m.fill_path(p, rule, true, ts);
                    }
                    return Some(m);
                }
            };
            match &path {
                Some(p) => m.intersect_path(p, rule, true, ts),
                None => m.clear(),
            }
            Some(m)
        });
    }
}

/// The mask of `clip`, built if it was not.
pub(crate) fn clip_mask(clip: &ClipMask) -> Option<&Mask> {
    clip.as_deref().and_then(Clip::mask)
}

/// What every draw takes from the context besides its geometry.
pub(crate) struct Ctx<'a> {
    pub size: (u32, u32),
    pub op: CompositeOperation,
    pub clip: Option<&'a Mask>,
    /// globalAlpha, for what `CanvasStyle::with_alpha` does not fade
    /// (patterns, images).
    pub alpha: f32,
}

/// How a stroke is drawn: the context's line styles at the time.
pub(crate) struct StrokeStyle<'a> {
    pub width: f32,
    pub cap: LineCap,
    pub join: LineJoin,
    pub miter_limit: f32,
    pub dash: &'a [f32],
    pub dash_offset: f32,
}

pub(crate) fn transform(t: &Transform2D) -> Transform {
    Transform::from_row(t.a, t.b, t.c, t.d, t.e, t.f)
}

fn blend(op: CompositeOperation) -> BlendMode {
    match op {
        CompositeOperation::SourceOver => BlendMode::SourceOver,
        CompositeOperation::SourceIn => BlendMode::SourceIn,
        CompositeOperation::SourceOut => BlendMode::SourceOut,
        CompositeOperation::SourceAtop => BlendMode::SourceAtop,
        CompositeOperation::DestinationOver => BlendMode::DestinationOver,
        CompositeOperation::DestinationIn => BlendMode::DestinationIn,
        CompositeOperation::DestinationOut => BlendMode::DestinationOut,
        CompositeOperation::DestinationAtop => BlendMode::DestinationAtop,
        CompositeOperation::Lighter => BlendMode::Plus,
        CompositeOperation::Copy => BlendMode::Source,
        CompositeOperation::Xor => BlendMode::Xor,
        CompositeOperation::Multiply => BlendMode::Multiply,
        CompositeOperation::Screen => BlendMode::Screen,
        CompositeOperation::Overlay => BlendMode::Overlay,
        CompositeOperation::Darken => BlendMode::Darken,
        CompositeOperation::Lighten => BlendMode::Lighten,
    }
}

fn color(c: Color) -> tiny_skia::Color {
    tiny_skia::Color::from_rgba8(c.r, c.g, c.b, (c.a.clamp(0.0, 1.0) * 255.0).round() as u8)
}

fn stops(list: &[crate::ColorStop]) -> Vec<GradientStop> {
    list.iter().map(|s| GradientStop::new(s.offset, color(s.color))).collect()
}

/// A style's shader (globalAlpha already folded into colours and stops;
/// a pattern draws `faded` when given), and for a pattern that does not repeat both
/// ways the band it is cut to, in user space. `None` paints nothing (a
/// gradient with no stops, or a degenerate one).
fn shader<'a>(style: &'a CanvasStyle, faded: Option<&'a Pixmap>) -> Option<(Shader<'a>, Option<Rect>)> {
    match style {
        CanvasStyle::Color(c) => Some((Shader::SolidColor(color(*c)), None)),
        CanvasStyle::LinearGradient(g) => {
            if g.stops.is_empty() || (g.x0 == g.x1 && g.y0 == g.y1) {
                return None;
            }
            if g.stops.len() == 1 {
                return Some((Shader::SolidColor(color(g.stops[0].color)), None));
            }
            let s = tiny_skia::LinearGradient::new(
                Point::from_xy(g.x0, g.y0),
                Point::from_xy(g.x1, g.y1),
                stops(&g.stops),
                SpreadMode::Pad,
                Transform::identity(),
            )?;
            Some((s, None))
        }
        CanvasStyle::RadialGradient(g) => {
            if g.stops.is_empty() || (g.x0 == g.x1 && g.y0 == g.y1 && g.r0 == g.r1) {
                return None;
            }
            if g.stops.len() == 1 {
                return Some((Shader::SolidColor(color(g.stops[0].color)), None));
            }
            // tiny-skia's two-point conical gradient starts at radius 0.
            // A start radius r0 maps offset t to (r0 + t (r1 - r0)) / r1 of
            // the end circle, exact when the circles share a centre, and
            // the inside of the start circle takes the first colour.
            let mut list = stops(&g.stops);
            if g.r0 > 0.0 && g.r1 > 0.0 {
                let k = g.r0 / g.r1;
                list = g
                    .stops
                    .iter()
                    .map(|s| GradientStop::new(k + s.offset * (1.0 - k), color(s.color)))
                    .collect();
                list.insert(0, GradientStop::new(0.0, color(g.stops[0].color)));
            }
            let s = tiny_skia::RadialGradient::new(
                Point::from_xy(g.x0, g.y0),
                Point::from_xy(g.x1, g.y1),
                g.r1,
                list,
                SpreadMode::Pad,
                Transform::identity(),
            )?;
            Some((s, None))
        }
        CanvasStyle::Pattern(p) => {
            let (w, h) = (p.bitmap.width() as f32, p.bitmap.height() as f32);
            const FAR: f32 = 1.0e7;
            let band = match p.repetition {
                PatternRepetition::Repeat => None,
                PatternRepetition::RepeatX => Rect::from_xywh(-FAR, 0.0, 2.0 * FAR, h),
                PatternRepetition::RepeatY => Rect::from_xywh(0.0, -FAR, w, 2.0 * FAR),
                PatternRepetition::NoRepeat => Rect::from_xywh(0.0, 0.0, w, h),
            };
            let s = tiny_skia::Pattern::new(
                faded.unwrap_or(&p.bitmap.0).as_ref(),
                SpreadMode::Repeat,
                FilterQuality::Bilinear,
                1.0,
                Transform::identity(),
            );
            Some((s, band))
        }
    }
}

/// Multiplies every premultiplied pixel by `alpha` (globalAlpha).
fn fade(pm: &mut Pixmap, alpha: f32) {
    if alpha >= 1.0 {
        return;
    }
    let k = (alpha.clamp(0.0, 1.0) * 255.0).round() as u32;
    let m = |c: u8| ((c as u32 * k + 127) / 255) as u8;
    for p in pm.pixels_mut() {
        if let Some(q) = tiny_skia::PremultipliedColorU8::from_rgba(m(p.red()), m(p.green()), m(p.blue()), m(p.alpha())) {
            *p = q;
        }
    }
}

/// `clip` cut further to `band` (user space under `t`).
fn band_clip(size: (u32, u32), clip: Option<&Mask>, band: Rect, t: Transform) -> Option<Mask> {
    let path = PathBuilder::from_rect(band);
    let mut m = match clip {
        Some(c) => c.clone(),
        None => {
            let mut m = Mask::new(size.0, size.1)?;
            m.data_mut().fill(255);
            m
        }
    };
    m.intersect_path(&path, FillRule::Winding, true, t);
    Some(m)
}

/// What this thread has rasterized so far (FRAME-STAGES): the draws that
/// painted into a bitmap and the milliseconds the raster calls took (a
/// draw that returned early, or a clip, adds its time but is not a draw),
/// never reset. Kept only while [`set_raster_timing`] is on for the
/// thread; off (the default) a draw reads one thread-local flag and no
/// clock. The per-frame stage line reads the difference across a turn;
/// draws run inside the page's script, so this is a breakdown of its
/// script time.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct RasterTotals {
    pub draws: u64,
    pub ms: f64,
}

thread_local! {
    static TOTALS: std::cell::Cell<RasterTotals> = const { std::cell::Cell::new(RasterTotals { draws: 0, ms: 0.0 }) };
    static TIMING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// This thread's [`RasterTotals`].
pub fn raster_totals() -> RasterTotals {
    TOTALS.with(|t| t.get())
}

/// Keeps this thread's [`RasterTotals`] from now on (`on`), or stops.
/// Per thread, as the totals are: the thread that reads them asks.
pub fn set_raster_timing(on: bool) {
    TIMING.with(|t| t.set(on));
}

fn timing() -> bool {
    TIMING.with(|t| t.get())
}

/// One raster call, timed until dropped when timing is on: two `Instant`
/// reads and a thread-local add.
struct Timed(Option<std::time::Instant>);

fn timed() -> Timed {
    Timed(timing().then(std::time::Instant::now))
}

/// One draw painted into a bitmap (counted only while timing is on).
fn painted() {
    if timing() {
        TOTALS.with(|t| {
            let mut v = t.get();
            v.draws += 1;
            t.set(v);
        });
    }
}

impl Drop for Timed {
    fn drop(&mut self) {
        let Some(start) = self.0 else { return };
        let ms = start.elapsed().as_secs_f64() * 1000.0;
        TOTALS.with(|t| {
            let mut v = t.get();
            v.ms += ms;
            t.set(v);
        });
    }
}

impl Raster {
    /// Replays one recorded op.
    pub(crate) fn apply(&mut self, op: &Op) {
        match op {
            Op::Fill { d, path, even_odd, style } => self.fill(&d.ctx(), path, *even_odd, style, &d.t),
            Op::Stroke { d, path, line, style } => self.stroke(&d.ctx(), path, &line.style(), style, &d.t),
            Op::Clear { clip, rect, t } => self.clear(clip_mask(clip), rect[0], rect[1], rect[2], rect[3], t),
            Op::Image { d, image, src, dst } => self.draw_image(&d.ctx(), image, *src, *dst, &d.t),
            Op::Put { size, rgba, width, dx, dy, dirty } => self.put(*size, rgba, *width, *dx, *dy, *dirty),
        }
    }

    /// Drops the pixels (a resize, a context reset).
    pub(crate) fn reset(&mut self) {
        self.pixmap = None;
    }

    /// The pixels cleared to transparent black, as a full clear would
    /// leave them after the draws it hides; `make`: one of those draws
    /// would have made the bitmap.
    pub(crate) fn wipe(&mut self, size: (u32, u32), make: bool) {
        match self.pixmap.as_mut() {
            Some(pm) => pm.fill(tiny_skia::Color::TRANSPARENT),
            None if make => {
                self.pixmap(size.0, size.1);
            }
            None => {}
        }
    }

    fn pixmap(&mut self, width: u32, height: u32) -> Option<&mut Pixmap> {
        if self.pixmap.is_none() && crate::bitmap_fits(width, height) {
            self.pixmap = Pixmap::new(width, height);
        }
        self.pixmap.as_mut()
    }

    /// A copy of the pixels, for another canvas to draw; `None` when
    /// nothing was drawn.
    pub(crate) fn bitmap(&self) -> Option<CanvasBitmap> {
        self.pixmap.clone().map(CanvasBitmap)
    }

    /// Fills the tiny-skia path `path` (user space under `t`) with `style`.
    fn fill_skia(&mut self, ctx: &Ctx, path: &tiny_skia::Path, rule: FillRule, style: &CanvasStyle, t: &Transform2D) {
        // A pattern under globalAlpha draws a faded copy (see `fade`).
        let faded = match style {
            CanvasStyle::Pattern(p) if ctx.alpha < 1.0 => {
                let mut pm = p.bitmap.0.clone();
                fade(&mut pm, ctx.alpha);
                Some(pm)
            }
            _ => None,
        };
        let Some((shader, band)) = shader(style, faded.as_ref()) else {
            return;
        };
        let ts = transform(t);
        let banded = band.and_then(|b| band_clip(ctx.size, ctx.clip, b, ts));
        let clip = if band.is_some() { banded.as_ref() } else { ctx.clip };
        if band.is_some() && clip.is_none() {
            return;
        }
        let paint = Paint { shader, blend_mode: blend(ctx.op), anti_alias: true, ..Paint::default() };
        if let Some(pm) = self.pixmap(ctx.size.0, ctx.size.1) {
            pm.fill_path(path, &paint, rule, ts, clip);
            painted();
        }
    }

    /// Fills `path` (canvas path commands, in user space under `t`).
    pub(crate) fn fill(&mut self, ctx: &Ctx, path: &[PathCommand], even_odd: bool, style: &CanvasStyle, t: &Transform2D) {
        let _timed = timed();
        let Some(path) = build_path(path) else {
            return;
        };
        let rule = if even_odd { FillRule::EvenOdd } else { FillRule::Winding };
        self.fill_skia(ctx, &path, rule, style, t);
    }

    /// Strokes `path` with `line`.
    pub(crate) fn stroke(&mut self, ctx: &Ctx, path: &[PathCommand], line: &StrokeStyle, style: &CanvasStyle, t: &Transform2D) {
        let _timed = timed();
        let Some(path) = build_path(path) else {
            return;
        };
        let stroke = skia_stroke(line);
        // The outline is filled rather than handed to `stroke_path`, which
        // draws a stroke no wider than a device pixel as a hairline, without
        // joins or caps; Chrome draws its true outline ("stroke-rect-thin").
        let res_scale = PathStroker::compute_resolution_scale(&transform(t));
        let Some(outline) = path.stroke(&stroke, res_scale) else {
            return;
        };
        self.fill_skia(ctx, &outline, FillRule::Winding, style, t);
    }

    /// Clears the rect `x, y, w, h` under `t` to transparent black.
    pub(crate) fn clear(&mut self, clip: Option<&Mask>, x: f32, y: f32, w: f32, h: f32, t: &Transform2D) {
        let _timed = timed();
        let Some(pm) = self.pixmap.as_mut() else {
            return; // nothing drawn yet: already clear
        };
        let Some(path) = build_path(&[PathCommand::Rect(x, y, w, h)]) else {
            return;
        };
        // Chrome clears whole pixels (no anti-aliasing), as Skia does for a
        // clearRect: canvas_raster_cases.json "clear-rect".
        let paint = Paint { blend_mode: BlendMode::Clear, anti_alias: false, ..Paint::default() };
        pm.fill_path(&path, &paint, FillRule::Winding, transform(t), clip);
        painted();
    }

    /// drawImage: the source rect `src` of `image` drawn into the
    /// destination rect `dst` (user space under `t`), smoothed (bilinear,
    /// Chrome's default "low" quality).
    pub(crate) fn draw_image(&mut self, ctx: &Ctx, image: &CanvasBitmap, src: [f32; 4], dst: [f32; 4], t: &Transform2D) {
        let _timed = timed();
        let [sx, sy, sw, sh] = src;
        let [dx, dy, dw, dh] = dst;
        if sw == 0.0 || sh == 0.0 || dw == 0.0 || dh == 0.0 {
            return;
        }
        // The pixels the source rect touches and one more each side: Chrome
        // samples past the source rect into the rest of the image (bilinear
        // at its edges reads the neighbouring pixels, "draw-image-img-data-png"
        // in canvas_img_cases.json), and clamps only at the image's edges.
        let (iw, ih) = (image.width() as f32, image.height() as f32);
        let x0 = (sx.min(sx + sw).floor() - 1.0).max(0.0);
        let y0 = (sy.min(sy + sh).floor() - 1.0).max(0.0);
        let x1 = (sx.max(sx + sw).ceil() + 1.0).min(iw);
        let y1 = (sy.max(sy + sh).ceil() + 1.0).min(ih);
        if x1 <= x0 || y1 <= y0 {
            return;
        }
        let Some(rect) = tiny_skia::IntRect::from_xywh(x0 as i32, y0 as i32, (x1 - x0) as u32, (y1 - y0) as u32) else {
            return;
        };
        let Some(mut crop) = image.0.clone_rect(rect) else {
            return;
        };
        // globalAlpha is folded into the pixels: a Pattern's own opacity
        // gives anti-aliased edge pixels more alpha than the inside
        // ("draw-image-rotated-alpha" read 195 at edges for 153 inside).
        fade(&mut crop, ctx.alpha);
        // Source space to destination space, then cropped space to source.
        let (kx, ky) = (dw / sw, dh / sh);
        let to_dst = Transform::from_row(kx, 0.0, 0.0, ky, dx - sx * kx, dy - sy * ky).pre_translate(x0, y0);
        // The source rect cut to the image, in destination space.
        let (cx0, cy0) = (sx.min(sx + sw).max(0.0), sy.min(sy + sh).max(0.0));
        let (cx1, cy1) = (sx.max(sx + sw).min(iw), sy.max(sy + sh).min(ih));
        let a = (dx + (cx0 - sx) * kx, dy + (cy0 - sy) * ky);
        let b = (dx + (cx1 - sx) * kx, dy + (cy1 - sy) * ky);
        let Some(area) = Rect::from_ltrb(a.0.min(b.0), a.1.min(b.1), a.0.max(b.0), a.1.max(b.1)) else {
            return;
        };
        let identity = kx == 1.0 && ky == 1.0 && t.a == 1.0 && t.b == 0.0 && t.c == 0.0 && t.d == 1.0;
        let integral = |v: f32| v.fract() == 0.0;
        let quality = if identity && integral(dx + t.e) && integral(dy + t.f) && integral(sx) && integral(sy) {
            FilterQuality::Nearest
        } else {
            FilterQuality::Bilinear
        };
        let shader = tiny_skia::Pattern::new(crop.as_ref(), SpreadMode::Pad, quality, 1.0, to_dst);
        let paint = Paint { shader, blend_mode: blend(ctx.op), anti_alias: true, ..Paint::default() };
        if let Some(pm) = self.pixmap(ctx.size.0, ctx.size.1) {
            pm.fill_path(&PathBuilder::from_rect(area), &paint, FillRule::Winding, transform(t), ctx.clip);
            painted();
        }
    }

    /// putImageData: unpremultiplied `rgba` rows of `width` written as they
    /// are (no transform, clip, alpha or compositing) at `dx, dy`, the
    /// part inside `dirty` (`x, y, w, h` in the image, already cut to it).
    pub(crate) fn put(&mut self, size: (u32, u32), rgba: &[u8], width: u32, dx: i32, dy: i32, dirty: [i32; 4]) {
        let _timed = timed();
        let Some(pm) = self.pixmap(size.0, size.1) else {
            return;
        };
        let pw = pm.width() as i64;
        // Only the rows and columns that are in the image, in `dirty` and
        // on the bitmap are visited, whatever numbers came in.
        let height = if width == 0 { 0 } else { (rgba.len() / 4 / width as usize).min(u32::MAX as usize) as u32 };
        let Some((rows, cols)) = put_span((width, height), (pm.width(), pm.height()), dx, dy, dirty) else {
            return;
        };
        painted();
        let pixels = pm.pixels_mut();
        for row in rows {
            let ty = dy as i64 + row;
            for col in cols.clone() {
                let tx = dx as i64 + col;
                let i = ((row * width as i64 + col) * 4) as usize;
                let Some(p) = rgba.get(i..i + 4) else { continue };
                pixels[(ty * pw + tx) as usize] = tiny_skia::ColorU8::from_rgba(p[0], p[1], p[2], p[3]).premultiply();
            }
        }
    }

    /// The pixels of the rect `x, y, w, h` (bitmap pixels; outside the
    /// bitmap reads as transparent black), unpremultiplied RGBA.
    /// The whole bitmap as premultiplied RGBA rows (`size` when nothing
    /// has drawn yet: transparent black).
    pub(crate) fn premultiplied(&self, size: (u32, u32)) -> Vec<u8> {
        match &self.pixmap {
            Some(pm) => pm.data().to_vec(),
            None => match crate::rgba_len(size.0, size.1) {
                Some(len) => vec![0u8; len],
                None => Vec::new(),
            },
        }
    }

    /// The bytes the bitmap holds (none before the first draw).
    pub(crate) fn bytes(&self) -> usize {
        self.pixmap.as_ref().map_or(0, |pm| pm.data().len())
    }

    pub(crate) fn read(&self, x: i32, y: i32, w: u32, h: u32) -> Vec<u8> {
        // Past the read limit: nothing (the bindings throw RangeError first).
        let Some(len) = crate::rgba_len(w, h) else {
            return Vec::new();
        };
        let mut out = vec![0u8; len];
        let Some(pm) = &self.pixmap else {
            return out;
        };
        let (pw, ph) = (pm.width() as i64, pm.height() as i64);
        let pixels = pm.pixels();
        for row in 0..h as i64 {
            let sy = y as i64 + row;
            if sy < 0 || sy >= ph {
                continue;
            }
            for col in 0..w as i64 {
                let sx = x as i64 + col;
                if sx < 0 || sx >= pw {
                    continue;
                }
                let p = pixels[(sy * pw + sx) as usize];
                let o = ((row * w as i64 + col) * 4) as usize;
                out[o..o + 4].copy_from_slice(&unpremultiply(p.red(), p.green(), p.blue(), p.alpha()));
            }
        }
        out
    }
}

/// tiny-skia's stroke for `line` (both pictures outline a stroke with it).
pub(crate) fn skia_stroke(line: &StrokeStyle) -> Stroke {
    Stroke {
        width: line.width,
        miter_limit: line.miter_limit,
        line_cap: match line.cap {
            LineCap::Butt => tiny_skia::LineCap::Butt,
            LineCap::Round => tiny_skia::LineCap::Round,
            LineCap::Square => tiny_skia::LineCap::Square,
        },
        line_join: match line.join {
            LineJoin::Miter => tiny_skia::LineJoin::Miter,
            LineJoin::Round => tiny_skia::LineJoin::Round,
            LineJoin::Bevel => tiny_skia::LineJoin::Bevel,
        },
        dash: if line.dash.is_empty() || line.dash.iter().all(|d| *d == 0.0) {
            None
        } else {
            StrokeDash::new(line.dash.to_vec(), line.dash_offset)
        },
    }
}

/// The clip after clipping to `path` (HTML §4.12.5.1.10: the new clip is
/// the old one intersected with the path), on a `size` bitmap. Recorded:
/// the mask is built when first used (see [`Clip`]).
/// `gpu`: the context keeps a GPU picture (A1), so the clip keeps its chain
/// of paths for the stencil ([`Clip::chain`]).
pub(crate) fn clip_to(size: (u32, u32), old: &ClipMask, path: &[PathCommand], even_odd: bool, t: &Transform2D, gpu: bool) -> ClipMask {
    let unbuilt = 1 + old.as_ref().map_or(0, |c| if c.mask.get().is_some() { 0 } else { c.unbuilt });
    let chain = if gpu {
        let before = match old {
            None => Some(Vec::new()),
            Some(c) => c.chain.as_ref().map(|v| v.as_ref().clone()),
        };
        before.filter(|v| v.len() < CLIP_CHAIN_MAX).map(|mut v| {
            v.push(Arc::new(ClipLink { path: path.to_vec(), even_odd, t: *t }));
            Arc::new(v)
        })
    } else {
        None
    };
    let clip = Arc::new(Clip {
        parent: Mutex::new(old.clone()),
        unbuilt,
        size,
        path: path.to_vec(),
        even_odd,
        t: *t,
        mask: OnceLock::new(),
        chain,
    });
    if unbuilt > CLIP_CHAIN_MAX {
        clip.mask();
    }
    Some(clip)
}

/// A draw's context at the call: what [`Ctx`] borrows, owned.
#[derive(Debug)]
pub(crate) struct DrawCtx {
    pub size: (u32, u32),
    pub op: CompositeOperation,
    pub clip: ClipMask,
    pub alpha: f32,
    pub t: Transform2D,
}

impl DrawCtx {
    fn ctx(&self) -> Ctx<'_> {
        Ctx { size: self.size, op: self.op, clip: clip_mask(&self.clip), alpha: self.alpha }
    }
}

/// [`StrokeStyle`], owned.
#[derive(Debug)]
pub(crate) struct Line {
    pub width: f32,
    pub cap: LineCap,
    pub join: LineJoin,
    pub miter_limit: f32,
    pub dash: Vec<f32>,
    pub dash_offset: f32,
}

impl Line {
    pub(crate) fn style(&self) -> StrokeStyle<'_> {
        StrokeStyle {
            width: self.width,
            cap: self.cap,
            join: self.join,
            miter_limit: self.miter_limit,
            dash: &self.dash,
            dash_offset: self.dash_offset,
        }
    }
}

/// One recorded raster operation: everything the raster takes for it,
/// resolved at the call.
#[derive(Debug)]
pub(crate) enum Op {
    Fill { d: DrawCtx, path: Vec<PathCommand>, even_odd: bool, style: CanvasStyle },
    Stroke { d: DrawCtx, path: Vec<PathCommand>, line: Line, style: CanvasStyle },
    Clear { clip: ClipMask, rect: [f32; 4], t: Transform2D },
    /// drawImage: the source bitmap as it was at the call.
    Image { d: DrawCtx, image: CanvasBitmap, src: [f32; 4], dst: [f32; 4] },
    /// putImageData: the unpremultiplied rows of `width` it writes at
    /// `dx, dy` (the part of the image it writes, already cut), `dirty`
    /// in them.
    Put { size: (u32, u32), rgba: Vec<u8>, width: u32, dx: i32, dy: i32, dirty: [i32; 4] },
}

impl Op {
    /// The clip the op draws under.
    pub(crate) fn clip(&self) -> Option<&Arc<Clip>> {
        match self {
            Op::Fill { d, .. } | Op::Stroke { d, .. } | Op::Image { d, .. } => d.clip.as_ref(),
            Op::Clear { clip, .. } => clip.as_ref(),
            Op::Put { .. } => None,
        }
    }

    /// The pattern bitmap the op fills or strokes with, if any.
    fn pattern(&self) -> Option<&Arc<CanvasBitmap>> {
        match self {
            Op::Fill { style: CanvasStyle::Pattern(p), .. } | Op::Stroke { style: CanvasStyle::Pattern(p), .. } => Some(&p.bitmap),
            _ => None,
        }
    }

    /// The heap the op holds, estimated: the op itself and what it owns
    /// alone. What it shares (a pattern's bitmap, a clip's path and mask)
    /// is counted by [`Recorded::push`], once for every pending op that
    /// holds it.
    pub(crate) fn bytes(&self) -> usize {
        fn style(s: &CanvasStyle) -> usize {
            match s {
                CanvasStyle::LinearGradient(g) => g.stops.len() * std::mem::size_of::<crate::ColorStop>(),
                CanvasStyle::RadialGradient(g) => g.stops.len() * std::mem::size_of::<crate::ColorStop>(),
                _ => 0,
            }
        }
        let path = |p: &[PathCommand]| p.len() * std::mem::size_of::<PathCommand>();
        std::mem::size_of::<Op>()
            + match self {
                Op::Fill { path: p, style: s, .. } => path(p) + style(s),
                Op::Stroke { path: p, line, style: s, .. } => path(p) + style(s) + line.dash.len() * 4,
                Op::Clear { .. } => 0,
                Op::Image { image, .. } => image.0.data().len(),
                Op::Put { rgba, .. } => rgba.len(),
            }
    }
}

/// A put's bytes cut to what it writes (`put_span`), as the rows of a
/// smaller image put at the same place: the same pixels written.
pub(crate) fn put_op(size: (u32, u32), rgba: &[u8], width: u32, dx: i32, dy: i32, dirty: [i32; 4]) -> Op {
    let height = if width == 0 { 0 } else { (rgba.len() / 4 / width as usize).min(u32::MAX as usize) as u32 };
    let none = Op::Put { size, rgba: Vec::new(), width: 0, dx: 0, dy: 0, dirty: [0, 0, 0, 0] };
    if !crate::bitmap_fits(size.0, size.1) {
        return none;
    }
    let Some((rows, cols)) = put_span((width, height), size, dx, dy, dirty) else {
        return none;
    };
    let cw = (cols.end - cols.start) as usize;
    let mut out = Vec::with_capacity(cw * (rows.end - rows.start) as usize * 4);
    for row in rows.clone() {
        let i = ((row * width as i64 + cols.start) * 4) as usize;
        out.extend_from_slice(&rgba[i..i + cw * 4]);
    }
    let (ch, cw) = ((rows.end - rows.start) as i32, cw as i32);
    Op::Put {
        size,
        rgba: out,
        width: cw as u32,
        dx: (dx as i64 + cols.start) as i32,
        dy: (dy as i64 + rows.start) as i32,
        dirty: [0, 0, cw, ch],
    }
}

/// The bitmap and the draws recorded since it was last brought up to date.
///
/// GPU canvas A1: with the switch on, the recording keeps two positions,
/// the CPU replay's (`cpu`, how far the bitmap is up to date) and the GPU
/// replay's (`gpu`, how far ops have been handed to the presenter), and an
/// op is dropped only when both have passed it. Positions are absolute op
/// numbers; `base` is the number of `ops[0]`. With the switch off `gpu` is
/// never read and every flush drops everything, as at A0.
#[derive(Debug, Default)]
pub(crate) struct Recorded {
    raster: Raster,
    ops: Vec<Arc<Op>>,
    base: u64,
    cpu: u64,
    gpu: u64,
    bytes: usize,
    /// What the pending ops share, by address, each counted once in
    /// `bytes` (and kept alive here, so an address is not reused while
    /// counted): clips (their path, and the mask the replay builds, one byte
    /// per pixel) and pattern bitmaps.
    clips: std::collections::HashMap<usize, Arc<Clip>>,
    patterns: std::collections::HashMap<usize, Arc<CanvasBitmap>>,
    /// Tests: rasterize at the call, as before the recording.
    immediate: bool,
    /// The context keeps a GPU picture (A1 switch on at its creation).
    gpu_on: bool,
    /// How the GPU picture starts over at its next batch, if it must.
    gpu_reset: GpuStart,
}

/// How the next GPU batch starts the texture.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum GpuStart {
    /// From where the last batch left it.
    #[default]
    Continue,
    /// Blank, then every op held from the GPU position (a new context, a
    /// reset or resize: nothing was drawn before those ops).
    Blank,
    /// From the CPU bitmap, brought up to date (ops were dropped before the
    /// GPU took them).
    Bitmap,
}

impl Recorded {
    /// A recording; `gpu`: it keeps a GPU position (A1).
    pub(crate) fn new(gpu: bool) -> Self {
        Self { gpu_on: gpu, gpu_reset: if gpu { GpuStart::Blank } else { GpuStart::Continue }, ..Self::default() }
    }

    fn end(&self) -> u64 {
        self.base + self.ops.len() as u64
    }

    /// Counts what `op` holds into `bytes` (shared things once).
    fn count(&mut self, op: &Op) {
        self.bytes += op.bytes();
        if let Some(c) = op.clip() {
            if let std::collections::hash_map::Entry::Vacant(e) = self.clips.entry(Arc::as_ptr(c) as usize) {
                self.bytes += c.size.0 as usize * c.size.1 as usize + c.path.len() * std::mem::size_of::<PathCommand>();
                if let Some(chain) = &c.chain {
                    self.bytes += chain.iter().map(|l| l.path.len() * std::mem::size_of::<PathCommand>()).sum::<usize>();
                }
                e.insert(c.clone());
            }
        }
        if let Some(p) = op.pattern() {
            if let std::collections::hash_map::Entry::Vacant(e) = self.patterns.entry(Arc::as_ptr(p) as usize) {
                self.bytes += p.0.data().len();
                e.insert(p.clone());
            }
        }
    }

    pub(crate) fn push(&mut self, op: Op) {
        if self.immediate {
            self.raster.apply(&op);
            return;
        }
        self.count(&op);
        self.ops.push(Arc::new(op));
        if self.bytes > crate::PENDING_BYTE_BUDGET {
            self.flush();
            if self.gpu_on && self.gpu < self.end() {
                // The GPU fell behind past the cap (no frame presented):
                // its picture starts over from the bitmap.
                self.gpu = self.end();
                self.gpu_reset = GpuStart::Bitmap;
                self.trim();
            }
        }
    }

    /// The bitmap with every pending op replayed into it.
    pub(crate) fn flush(&mut self) -> &mut Raster {
        let from = (self.cpu - self.base) as usize;
        for op in &self.ops[from..] {
            self.raster.apply(op);
        }
        self.cpu = self.end();
        self.trim();
        &mut self.raster
    }

    /// Drops the ops both replays have passed (the CPU's alone with the
    /// switch off), recounting what the rest hold.
    fn trim(&mut self) {
        let keep = if self.gpu_on { self.cpu.min(self.gpu) } else { self.cpu };
        let n = (keep - self.base) as usize;
        if n == 0 {
            return;
        }
        self.base = keep;
        self.bytes = 0;
        self.clips.clear();
        self.patterns.clear();
        if n >= self.ops.len() {
            self.ops.clear();
            return;
        }
        self.ops.drain(..n);
        for op in std::mem::take(&mut self.ops) {
            self.count(&op);
            self.ops.push(op);
        }
    }

    /// A clear of the whole `size` bitmap with no clip, about to be pushed
    /// (switch on only): what was drawn before it shows in neither picture,
    /// so neither replays it. The bitmap is wiped instead of replayed (it
    /// is made when a dropped op would have made it).
    pub(crate) fn full_clear(&mut self, size: (u32, u32)) {
        if !self.gpu_on || self.immediate {
            return;
        }
        let from = (self.cpu - self.base) as usize;
        let makes = self.ops[from..].iter().any(|op| !matches!(**op, Op::Clear { .. }));
        self.raster.wipe(size, makes);
        self.cpu = self.end();
        self.gpu = self.end();
        self.trim();
    }

    /// The pixels dropped (a resize, a context reset): nothing pending is
    /// left to replay.
    pub(crate) fn reset(&mut self) {
        self.ops.clear();
        self.base = self.end();
        self.cpu = self.base;
        self.gpu = self.base;
        self.bytes = 0;
        self.clips.clear();
        self.patterns.clear();
        self.raster.reset();
        if self.gpu_on {
            self.gpu_reset = GpuStart::Blank;
        }
    }

    pub(crate) fn pending_ops(&self) -> usize {
        self.ops.len() - (self.cpu - self.base) as usize
    }

    pub(crate) fn pending_bytes(&self) -> usize {
        self.bytes
    }

    pub(crate) fn set_immediate(&mut self, on: bool) {
        if on {
            self.flush();
        }
        self.immediate = on;
    }

    pub(crate) fn gpu_on(&self) -> bool {
        self.gpu_on
    }

    /// The ops the GPU has not taken yet, handed over as a batch, and the
    /// GPU position moved to the end. A batch starts the texture blank when
    /// nothing was drawn before its ops (a new context, a reset or resize),
    /// and starts it over from the CPU bitmap (brought up to date here)
    /// when ops were dropped before the GPU took them, when the presenter
    /// asks (`force_reset`: it lost the texture), or when an op is one the
    /// GPU replay does not draw at A1 ([`crate::gpu::supported`]).
    pub(crate) fn take_gpu(&mut self, size: (u32, u32), force_reset: bool) -> crate::gpu::GpuBatch {
        let from = (self.gpu - self.base) as usize;
        let unsupported = self.ops[from..].iter().any(|op| !crate::gpu::supported(op));
        let start = std::mem::take(&mut self.gpu_reset);
        if unsupported || start == GpuStart::Bitmap || (force_reset && start != GpuStart::Blank) {
            self.flush();
            let pixels = self.raster.pixmap.as_ref().map(|pm| pm.data().to_vec());
            self.gpu = self.end();
            self.trim();
            return crate::gpu::GpuBatch::reset(size, pixels);
        }
        let ops = self.ops[from..].to_vec();
        self.gpu = self.end();
        self.trim();
        let mut batch = crate::gpu::GpuBatch::ops(size, ops);
        if start == GpuStart::Blank {
            batch.reset = Some(crate::gpu::GpuReset::Blank);
        }
        batch
    }

    /// The GPU picture is given up for good (the presenter denied it): the
    /// recording goes back to one position.
    pub(crate) fn leave_gpu(&mut self) {
        self.gpu_on = false;
        self.gpu_reset = GpuStart::Continue;
        self.trim();
    }

    /// Ops the GPU has not taken yet.
    pub(crate) fn gpu_pending(&self) -> usize {
        if self.gpu_on {
            (self.end() - self.gpu) as usize
        } else {
            0
        }
    }
}

/// [`unpremultiply`], for the GPU compare.
pub(crate) fn unpremultiply_px(r: u8, g: u8, b: u8, a: u8) -> [u8; 4] {
    unpremultiply(r, g, b, a)
}

/// Skia's unpremultiply (SkUnPreMultiply): `c * 255 / a`, rounded.
fn unpremultiply(r: u8, g: u8, b: u8, a: u8) -> [u8; 4] {
    match a {
        0 => [0, 0, 0, 0],
        255 => [r, g, b, 255],
        _ => {
            let scale = ((255u32 << 24) + a as u32 / 2) / a as u32;
            let f = |c: u8| ((c as u32 * scale + (1 << 23)) >> 24).min(255) as u8;
            [f(r), f(g), f(b), a]
        }
    }
}

/// A canvas path as a tiny-skia path, curves kept (HTML §4.12.5.1.6).
/// `None` when it has no segments.
pub(crate) fn build_path(commands: &[PathCommand]) -> Option<tiny_skia::Path> {
    let mut pb = PathBuilder::new();
    // The current point and the start of the sub-path; `None` before the
    // first point ("ensure there is a subpath").
    let mut cur: Option<(f32, f32)> = None;
    let mut start = (0.0, 0.0);
    let ensure = |pb: &mut PathBuilder, cur: &mut Option<(f32, f32)>, start: &mut (f32, f32), p: (f32, f32)| {
        if cur.is_none() {
            pb.move_to(p.0, p.1);
            *cur = Some(p);
            *start = p;
        }
    };
    for cmd in commands {
        match *cmd {
            PathCommand::MoveTo(x, y) => {
                pb.move_to(x, y);
                cur = Some((x, y));
                start = (x, y);
            }
            PathCommand::LineTo(x, y) => {
                ensure(&mut pb, &mut cur, &mut start, (x, y));
                pb.line_to(x, y);
                cur = Some((x, y));
            }
            PathCommand::QuadraticCurveTo(cx, cy, x, y) => {
                ensure(&mut pb, &mut cur, &mut start, (cx, cy));
                pb.quad_to(cx, cy, x, y);
                cur = Some((x, y));
            }
            PathCommand::BezierCurveTo(c1x, c1y, c2x, c2y, x, y) => {
                ensure(&mut pb, &mut cur, &mut start, (c1x, c1y));
                pb.cubic_to(c1x, c1y, c2x, c2y, x, y);
                cur = Some((x, y));
            }
            PathCommand::Arc(cx, cy, r, a0, a1, ccw) => {
                ellipse(&mut pb, &mut cur, &mut start, (cx, cy), (r, r), 0.0, a0, a1, ccw);
            }
            PathCommand::Ellipse(cx, cy, rx, ry, rot, a0, a1, ccw) => {
                ellipse(&mut pb, &mut cur, &mut start, (cx, cy), (rx, ry), rot, a0, a1, ccw);
            }
            PathCommand::ArcTo(x1, y1, x2, y2, r) => {
                ensure(&mut pb, &mut cur, &mut start, (x1, y1));
                let p0 = cur.unwrap_or((x1, y1));
                arc_to(&mut pb, p0, (x1, y1), (x2, y2), r, &mut cur);
            }
            PathCommand::Rect(x, y, w, h) => {
                pb.move_to(x, y);
                pb.line_to(x + w, y);
                pb.line_to(x + w, y + h);
                pb.line_to(x, y + h);
                pb.close();
                pb.move_to(x, y);
                cur = Some((x, y));
                start = (x, y);
            }
            PathCommand::ClosePath => {
                if cur.is_some() {
                    pb.close();
                    // A new sub-path starts at the closed one's start.
                    pb.move_to(start.0, start.1);
                    cur = Some(start);
                }
            }
        }
    }
    pb.finish()
}

/// An arc of the ellipse centred on `c` with radii `r`, rotated `rot`,
/// from angle `a0` to `a1` (HTML §4.12.5.1.6 ellipse()): a line from the
/// current point to its start, then the arc.
#[allow(clippy::too_many_arguments)]
fn ellipse(
    pb: &mut PathBuilder,
    cur: &mut Option<(f32, f32)>,
    start: &mut (f32, f32),
    c: (f32, f32),
    r: (f32, f32),
    rot: f32,
    a0: f32,
    a1: f32,
    ccw: bool,
) {
    let sweep = if !ccw && a1 - a0 >= TAU {
        TAU
    } else if ccw && a0 - a1 >= TAU {
        -TAU
    } else {
        let mut s = (a1 - a0) % TAU;
        if !ccw && s < 0.0 {
            s += TAU;
        } else if ccw && s > 0.0 {
            s -= TAU;
        }
        s
    };
    let (sr, cr) = rot.sin_cos();
    let point = |a: f32| {
        let (s, co) = a.sin_cos();
        let (x, y) = (r.0 * co, r.1 * s);
        (c.0 + x * cr - y * sr, c.1 + x * sr + y * cr)
    };
    // The derivative of `point`, per radian.
    let tangent = |a: f32| {
        let (s, co) = a.sin_cos();
        let (x, y) = (-r.0 * s, r.1 * co);
        (x * cr - y * sr, x * sr + y * cr)
    };
    let p0 = point(a0);
    match cur {
        Some(_) => pb.line_to(p0.0, p0.1),
        None => {
            pb.move_to(p0.0, p0.1);
            *start = p0;
        }
    }
    let n = ((sweep.abs() / (PI / 2.0)) - 1e-4).ceil().max(1.0) as usize;
    let step = sweep / n as f32;
    let k = 4.0 / 3.0 * (step / 4.0).tan();
    let mut a = a0;
    let mut p = p0;
    for i in 0..n {
        let b = if i + 1 == n { a0 + sweep } else { a + step };
        let q = point(b);
        let (t0, t1) = (tangent(a), tangent(b));
        if sweep != 0.0 {
            pb.cubic_to(p.0 + k * t0.0, p.1 + k * t0.1, q.0 - k * t1.0, q.1 - k * t1.1, q.0, q.1);
        }
        a = b;
        p = q;
    }
    *cur = Some(p);
}

/// arcTo (HTML §4.12.5.1.6): the arc of radius `r` tangent to the lines
/// p0-p1 and p1-p2, joined to the current point by a line.
fn arc_to(
    pb: &mut PathBuilder,
    p0: (f32, f32),
    p1: (f32, f32),
    p2: (f32, f32),
    r: f32,
    cur: &mut Option<(f32, f32)>,
) {
    let (v1, v2) = ((p0.0 - p1.0, p0.1 - p1.1), (p2.0 - p1.0, p2.1 - p1.1));
    let cross = v1.0 * v2.1 - v1.1 * v2.0;
    let (l1, l2) = ((v1.0 * v1.0 + v1.1 * v1.1).sqrt(), (v2.0 * v2.0 + v2.1 * v2.1).sqrt());
    if p0 == p1 || p1 == p2 || r == 0.0 || cross.abs() <= 1e-6 * l1 * l2 {
        pb.line_to(p1.0, p1.1);
        *cur = Some(p1);
        return;
    }
    let (u1, u2) = ((v1.0 / l1, v1.1 / l1), (v2.0 / l2, v2.1 / l2));
    let cos = (u1.0 * u2.0 + u1.1 * u2.1).clamp(-1.0, 1.0);
    let half = cos.acos() / 2.0;
    let d = r / half.tan();
    let t1 = (p1.0 + u1.0 * d, p1.1 + u1.1 * d);
    let t2 = (p1.0 + u2.0 * d, p1.1 + u2.1 * d);
    let (bx, by) = (u1.0 + u2.0, u1.1 + u2.1);
    let bl = (bx * bx + by * by).sqrt();
    let h = r / half.sin();
    let c = (p1.0 + bx / bl * h, p1.1 + by / bl * h);
    let a0 = (t1.1 - c.1).atan2(t1.0 - c.0);
    let a1 = (t2.1 - c.1).atan2(t2.0 - c.0);
    // The short way round: anticlockwise when the corner turns right.
    let ccw = cross > 0.0;
    let mut start = p0;
    ellipse(pb, cur, &mut start, c, (r, r), 0.0, a0, a1, ccw);
}

/// The rows and the columns of an image that a put writes: `dirty`
/// (`x, y, w, h` in the image) cut to the image (`image`: width, height)
/// and to the bitmap (`bitmap`) the image is put on at `dx, dy`. `None`
/// when nothing is left. Every sum is made in 64 bits, so no `i32` given
/// here overflows, and the ranges are never longer than the bitmap's side.
pub(crate) fn put_span(
    image: (u32, u32),
    bitmap: (u32, u32),
    dx: i32,
    dy: i32,
    dirty: [i32; 4],
) -> Option<(std::ops::Range<i64>, std::ops::Range<i64>)> {
    let [x, y, w, h] = dirty.map(i64::from);
    let (dx, dy) = (i64::from(dx), i64::from(dy));
    if w <= 0 || h <= 0 {
        return None;
    }
    let cols = x.max(0).max(-dx)..(x + w).min(i64::from(image.0)).min(i64::from(bitmap.0) - dx);
    let rows = y.max(0).max(-dy)..(y + h).min(i64::from(image.1)).min(i64::from(bitmap.1) - dy);
    (cols.start < cols.end && rows.start < rows.end).then_some((rows, cols))
}

#[cfg(test)]
mod raster_totals_tests {
    use crate::{raster_totals, set_raster_timing, CanvasRenderingContext2D};

    /// Every draw that paints into the bitmap is one more on this thread's
    /// totals (the per-frame stage line reads the difference across a
    /// turn); a draw that returns before painting, and a clip, are not.
    /// Counts, not milliseconds: no wall-clock bound.
    #[test]
    fn each_draw_painted_into_the_bitmap_counts_once_on_the_thread_totals() {
        set_raster_timing(true);
        let before = raster_totals();
        // Nothing drawn yet: the clear returns before painting.
        // Draws are recorded and rasterized at the first read (GPU canvas
        // A0, #1282): every count below is taken after a read.
        let mut fresh = CanvasRenderingContext2D::new(20, 20);
        fresh.clear_rect(0.0, 0.0, 3.0, 3.0);
        fresh.premultiplied_pixels();
        assert_eq!(raster_totals().draws, before.draws, "a clear of a bitmap never drawn paints nothing");
        let mut ctx = CanvasRenderingContext2D::new(20, 20);
        ctx.fill_rect(1.0, 1.0, 5.0, 5.0);
        ctx.stroke_rect(2.0, 2.0, 6.0, 6.0);
        ctx.clear_rect(0.0, 0.0, 3.0, 3.0);
        ctx.begin_path();
        ctx.rect(0.0, 0.0, 10.0, 10.0);
        ctx.clip();
        ctx.premultiplied_pixels();
        let read = raster_totals();
        assert_eq!(read.draws - before.draws, 3, "fill, stroke and clear: one each; the clip is none");
        assert!(read.ms >= before.ms, "the time only grows");
        // The clip's mask is built when a draw under it is first replayed:
        // that adds time, not a draw.
        ctx.fill_rect(0.0, 0.0, 20.0, 20.0);
        assert_eq!(raster_totals().draws, read.draws, "a draw not yet read is not rasterized");
        ctx.premultiplied_pixels();
        let after = raster_totals();
        assert_eq!(after.draws - read.draws, 1, "the fill under the clip is one draw; building the clip's mask is none");
        assert!(after.ms >= read.ms);
        // Another thread's draws are not this thread's.
        let other = std::thread::spawn(|| {
            set_raster_timing(true);
            let mut ctx = CanvasRenderingContext2D::new(4, 4);
            ctx.fill_rect(0.0, 0.0, 1.0, 1.0);
            ctx.premultiplied_pixels();
            raster_totals().draws
        })
        .join()
        .unwrap();
        assert_eq!(other, 1);
        assert_eq!(raster_totals().draws, after.draws);
        set_raster_timing(false);
    }

    /// Off (the default for every thread), a draw keeps no totals: no
    /// count and no clock read reaches them.
    #[test]
    fn with_raster_timing_off_draws_keep_no_totals() {
        let draws = std::thread::spawn(|| {
            let mut ctx = CanvasRenderingContext2D::new(8, 8);
            ctx.fill_rect(0.0, 0.0, 4.0, 4.0);
            ctx.stroke_rect(1.0, 1.0, 4.0, 4.0);
            ctx.clear_rect(0.0, 0.0, 2.0, 2.0);
            ctx.premultiplied_pixels();
            raster_totals()
        })
        .join()
        .unwrap();
        assert_eq!(draws, crate::RasterTotals::default());
        // Turned off again after being on: the totals stand still.
        set_raster_timing(true);
        let mut ctx = CanvasRenderingContext2D::new(8, 8);
        ctx.fill_rect(0.0, 0.0, 4.0, 4.0);
        ctx.premultiplied_pixels();
        set_raster_timing(false);
        let held = raster_totals();
        ctx.fill_rect(0.0, 0.0, 4.0, 4.0);
        ctx.premultiplied_pixels();
        assert_eq!(raster_totals(), held);
    }
}

#[cfg(test)]
mod put_span_tests {
    use super::put_span;

    #[test]
    fn a_put_is_cut_to_the_image_the_dirty_rect_and_the_bitmap() {
        // Whole image inside the bitmap.
        assert_eq!(put_span((2, 2), (4, 4), 1, 1, [0, 0, 2, 2]), Some((0..2, 0..2)));
        // Past the bitmap's right and bottom edge; before its left and top.
        assert_eq!(put_span((2, 2), (4, 4), 3, 3, [0, 0, 2, 2]), Some((0..1, 0..1)));
        assert_eq!(put_span((2, 2), (4, 4), -1, -1, [0, 0, 2, 2]), Some((1..2, 1..2)));
        // A dirty rect inside, one larger than the image, one that starts before it.
        assert_eq!(put_span((2, 2), (4, 4), 1, 1, [1, 0, 1, 2]), Some((0..2, 1..2)));
        assert_eq!(put_span((2, 2), (4, 4), 0, 0, [1, 1, 50, 50]), Some((1..2, 1..2)));
        assert_eq!(put_span((2, 2), (4, 4), 0, 0, [-5, -5, 6, 6]), Some((0..1, 0..1)));
        // Nothing left.
        assert_eq!(put_span((2, 2), (4, 4), 4, 0, [0, 0, 2, 2]), None);
        assert_eq!(put_span((2, 2), (4, 4), 0, -2, [0, 0, 2, 2]), None);
        assert_eq!(put_span((2, 2), (4, 4), 0, 0, [2, 0, 1, 1]), None);
        assert_eq!(put_span((0, 0), (4, 4), 0, 0, [0, 0, 1, 1]), None);
        assert_eq!(put_span((2, 2), (0, 0), 0, 0, [0, 0, 2, 2]), None);
    }

    #[test]
    fn no_number_overflows_or_makes_a_long_walk() {
        let (lo, hi) = (i32::MIN, i32::MAX);
        for dirty in [[hi, hi, hi, hi], [lo, lo, hi, hi], [hi, hi, lo, lo], [lo, lo, lo, lo], [0, 0, hi, hi], [-1, -1, hi, hi], [hi - 1, hi - 1, 2, 2], [0, 0, -1, 5], [0, 0, 5, 0]] {
            for (dx, dy) in [(0, 0), (hi, hi), (lo, lo), (hi, lo), (lo, hi), (-1, 1)] {
                for image in [(2, 2), (u32::MAX, u32::MAX), (0, 7)] {
                    for bitmap in [(4, 4), (16384, 16384), (u32::MAX, 1)] {
                        if let Some((rows, cols)) = put_span(image, bitmap, dx, dy, dirty) {
                            assert!(rows.start >= 0 && cols.start >= 0, "{dirty:?} {dx} {dy} {image:?} {bitmap:?}");
                            assert!(rows.end <= i64::from(image.1) && cols.end <= i64::from(image.0));
                            assert!(rows.end - rows.start <= i64::from(bitmap.1) && cols.end - cols.start <= i64::from(bitmap.0));
                            assert!(dy as i64 + rows.start >= 0 && dy as i64 + rows.end <= i64::from(bitmap.1));
                            assert!(dx as i64 + cols.start >= 0 && dx as i64 + cols.end <= i64::from(bitmap.0));
                        }
                    }
                }
            }
        }
        // The whole of a large image on a large bitmap is still exactly its overlap.
        assert_eq!(put_span((16384, 16384), (16384, 16384), 16380, 0, [0, 0, hi, hi]), Some((0..16384, 0..4)));
    }
}
