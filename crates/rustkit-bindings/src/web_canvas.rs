//! Canvas 2D for page script (HTML §4.12.5): `HTMLCanvasElement.getContext('2d')`
//! hands out a `CanvasRenderingContext2D` whose drawing is recorded by
//! `rustkit-canvas`, one context per canvas element, held here and read by
//! the engine when it builds the display list (`DomBindings::canvas_snapshot`).
//! web_canvas.js is the script surface; every call lands in the one host
//! function `__rustkit_canvas(gen, id, op, ...)`.
//!
//! The engine paints the recording as vector display commands in the
//! canvas's box; rustkit-canvas also rasterizes every draw into a bitmap,
//! which `getImageData` reads (STR-7).

use crate::dom::{DomHost, SharedDomHost};
use rustkit_canvas::{
    parse_canvas_color, serialize_canvas_color, CanvasBitmap, CanvasFont, CanvasPattern, CanvasRenderingContext2D,
    CanvasStyle, DrawCommand, ImageData, LineCap, LineJoin, LinearGradient, PatternRepetition, RadialGradient,
    TextAlign, TextBaseline, Transform2D,
};
use rustkit_js::{JsError, JsRuntime, JsValue};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

/// How wide a run of text is in a canvas font, and the font's ascent and
/// descent, in px: the engine's text shaper, installed by the engine
/// (`DomBindings::set_canvas_text_measurer`).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct CanvasTextExtent {
    pub width: f32,
    pub ascent: f32,
    pub descent: f32,
}

/// Measures `text` in `font` (see [`CanvasTextExtent`]).
pub type CanvasTextMeasurer = Box<dyn Fn(&CanvasFont, &str) -> CanvasTextExtent>;

/// An `<img>`'s decoded pixels by its URL, `None` when not decoded
/// (installed by the engine, `DomBindings::set_canvas_image_loader`).
///
/// The size is the device-pixel size the image is drawn at, `(0, 0)` for
/// its natural size: a vector image (SVG) is rasterized at the size it is
/// drawn, as Chrome does; a raster image ignores it.
///
/// With the bitmap come the URLs of the response it was decoded from: every
/// redirect hop and the final URL (empty when no request was made, as for a
/// `data:` URL). A canvas that draws it is origin-clean only when all of
/// them are of the document's origin (Chrome decides on the response).
pub type CanvasImageLoader = Box<dyn Fn(&str, (u32, u32)) -> Option<(CanvasBitmap, Vec<url::Url>)>>;

/// What the engine reads of one canvas: its bitmap size and the drawing
/// recorded on it since it was last cleared.
#[derive(Debug, Clone)]
pub struct CanvasSnapshot {
    pub width: u32,
    pub height: u32,
    pub commands: Vec<DrawCommand>,
}

/// The size `font` sizes in em/% are relative to: Chromium's default
/// font size, which a canvas in a page with no font-size inherits.
const FONT_BASE_PX: f32 = 16.0;

#[derive(Default)]
pub(crate) struct CanvasStore {
    /// One context per canvas element, by NodeId.
    contexts: HashMap<usize, CanvasRenderingContext2D>,
    /// A context drew, cleared or was resized since the engine last took this.
    painted: bool,
    measurer: Option<CanvasTextMeasurer>,
    pub(crate) images: Option<CanvasImageLoader>,
    /// The document's origin (`DomBindings::set_location`): an image of
    /// any other origin leaves a canvas that draws it not origin-clean.
    pub(crate) origin: Option<url::Origin>,
    /// The image each `<img>` shows, by NodeId, as the engine reported it
    /// (`DomBindings::fire_image_load`): what a canvas draws for that
    /// element. Nothing script can write reaches this.
    pub(crate) loaded: HashMap<usize, LoadedImage>,
    /// The images patterns were made from, by the index script holds.
    pattern_sources: Vec<LoadedImage>,
}

/// An `<img>`'s loaded resource: the response's URL and the image's own
/// size, with the `src` and `srcset` the element had when it loaded (it
/// stands for the element only while they are unchanged).
#[derive(Clone, PartialEq)]
pub(crate) struct LoadedImage {
    pub(crate) src: Option<String>,
    pub(crate) srcset: Option<String>,
    pub(crate) url: String,
    pub(crate) width: u32,
    pub(crate) height: u32,
}

pub(crate) type SharedCanvases = Rc<RefCell<CanvasStore>>;

/// A page's canvases as the engine reads them (`DomBindings::canvas_source`).
#[derive(Clone)]
pub struct CanvasSource(pub(crate) SharedCanvases);

impl CanvasSource {
    /// What the canvas element `node` drew; `None` with no 2D context.
    pub fn snapshot(&self, node: usize) -> Option<CanvasSnapshot> {
        self.0.borrow().snapshot(node)
    }

    /// The canvas element `node`'s recording generation
    /// (`CanvasRenderingContext2D::generation`); `None` with no 2D context.
    /// A reader that cached what it built from a snapshot rebuilds only
    /// when this moves.
    pub fn generation(&self, node: usize) -> Option<u64> {
        self.0.borrow().contexts.get(&node).map(|c| c.generation())
    }

    /// The canvas element `node`'s bitmap as premultiplied RGBA rows:
    /// `(width, height, pixels)`. `None` with no 2D context.
    pub fn pixels(&self, node: usize) -> Option<(u32, u32, Vec<u8>)> {
        let store = self.0.borrow();
        let ctx = store.contexts.get(&node)?;
        Some((ctx.width, ctx.height, ctx.premultiplied_pixels()))
    }

    /// An estimate of the heap the page's canvases hold, in bytes: their
    /// recordings and anything kept for script beside them.
    pub fn retained_bytes(&self) -> usize {
        self.0.borrow().retained_bytes()
    }

    /// GPU canvas A1: whether the canvas element `node`'s context keeps a
    /// GPU picture, and its id (the key its texture is kept under).
    /// `(id, width, height)`.
    pub fn gpu_context(&self, node: usize) -> Option<(u64, u32, u32)> {
        let store = self.0.borrow();
        let ctx = store.contexts.get(&node)?;
        ctx.gpu_canvas().then(|| (ctx.context_id(), ctx.width, ctx.height))
    }

    /// GPU canvas A1: the draws of `node`'s canvas the presenter has not
    /// taken (`CanvasRenderingContext2D::take_gpu_batch`).
    pub fn take_gpu_batch(&self, node: usize, force_reset: bool) -> Option<crate::canvas::GpuBatch> {
        Some(self.0.borrow().contexts.get(&node)?.take_gpu_batch(force_reset))
    }

    /// The canvas elements with a 2D context, by NodeId.
    pub fn nodes(&self) -> Vec<usize> {
        let mut v: Vec<usize> = self.0.borrow().contexts.keys().copied().collect();
        v.sort_unstable();
        v
    }

    /// GPU canvas A1: `node`'s canvas is shown from its CPU bitmap from now on.
    pub fn leave_gpu(&self, node: usize) {
        if let Some(ctx) = self.0.borrow().contexts.get(&node) {
            ctx.leave_gpu();
        }
    }
}

impl CanvasStore {
    /// A new document: its canvases start with no context. Not a paint:
    /// the new document gets a full layout and display list of its own, and
    /// a mark left here would list and present that same frame again at the
    /// next settle (#905 batch 22: an extra frame after every load).
    pub(crate) fn clear(&mut self) {
        self.loaded.clear();
        self.pattern_sources.clear();
        self.contexts.clear();
        self.painted = false;
    }

    pub(crate) fn snapshot(&self, node: usize) -> Option<CanvasSnapshot> {
        self.contexts.get(&node).map(|ctx| CanvasSnapshot {
            width: ctx.width,
            height: ctx.height,
            commands: ctx.get_commands().to_vec(),
        })
    }

    pub(crate) fn retained_bytes(&self) -> usize {
        self.contexts.values().map(|c| c.recorded_bytes() + c.bitmap_bytes()).sum()
    }

    pub(crate) fn take_painted(&mut self) -> bool {
        std::mem::take(&mut self.painted)
    }

    pub(crate) fn set_measurer(&mut self, measurer: Option<CanvasTextMeasurer>) {
        self.measurer = measurer;
    }

    pub(crate) fn measure_font(&self, font: &str, text: &str) -> CanvasTextExtent {
        let font = CanvasFont::parse(font, FONT_BASE_PX).unwrap_or_else(|| {
            CanvasFont::parse("10px sans-serif", FONT_BASE_PX).expect("initial font")
        });
        match &self.measurer {
            Some(measure) => measure(&font, text),
            // No shaper (the bindings on their own, in tests): half an em
            // per character.
            None => CanvasTextExtent {
                width: text.chars().count() as f32 * font.size * 0.5,
                ascent: font.size * 0.8,
                descent: font.size * 0.2,
            },
        }
    }
}

fn num(args: &[JsValue], i: usize) -> f32 {
    match args.get(i) {
        Some(JsValue::Number(n)) => *n as f32,
        _ => f32::NAN,
    }
}

fn nums<const N: usize>(args: &[JsValue], from: usize) -> Option<[f32; N]> {
    let mut out = [0.0; N];
    for (k, slot) in out.iter_mut().enumerate() {
        let v = num(args, from + k);
        if !v.is_finite() {
            return None;
        }
        *slot = v;
    }
    Some(out)
}

fn text(args: &[JsValue], i: usize) -> Option<&str> {
    match args.get(i) {
        Some(JsValue::String(s)) => Some(s.as_str()),
        _ => None,
    }
}

/// "offset|colour|offset|colour..." as colour stops.
fn color_stops(stops: &str) -> Vec<(f32, rustkit_css::Color)> {
    let mut out = Vec::new();
    let mut parts = stops.split('|').filter(|p| !p.is_empty());
    while let (Some(offset), Some(color)) = (parts.next(), parts.next()) {
        if let (Ok(offset), Some(color)) = (offset.parse::<f32>(), parse_canvas_color(color)) {
            out.push((offset, color));
        }
    }
    out
}

/// Comma-separated numbers, `None` unless there are `N` finite ones.
fn number_list<const N: usize>(s: Option<&str>) -> Option<[f32; N]> {
    let mut out = [0.0; N];
    let mut it = s?.split(',');
    for slot in out.iter_mut() {
        let v: f32 = it.next()?.parse().ok()?;
        if !v.is_finite() {
            return None;
        }
        *slot = v;
    }
    Some(out)
}

/// The ImageData a put writes, from what the script left in the pixel
/// slot and the size it gave: only when the size is a whole number of
/// pixels a canvas can hold and the bytes are exactly that many.
fn image_from_slot(data: Option<Vec<u8>>, width: f32, height: f32) -> Option<ImageData> {
    let whole = |v: f32| v >= 1.0 && v <= rustkit_canvas::MAX_CANVAS_SIDE as f32 && v.fract() == 0.0;
    if !whole(width) || !whole(height) {
        return None;
    }
    let (width, height) = (width as u32, height as u32);
    let data = data.filter(|data| rustkit_canvas::rgba_len(width, height) == Some(data.len()))?;
    Some(ImageData { width, height, data })
}

#[cfg(test)]
mod slot_tests {
    use super::image_from_slot;

    #[test]
    fn a_put_takes_the_slots_bytes_only_at_exactly_the_size_given() {
        let made = image_from_slot(Some(vec![7; 24]), 2.0, 3.0).expect("2 x 3 pixels are 24 bytes");
        assert_eq!((made.width, made.height, made.data.len()), (2, 3, 24));
        for (bytes, width, height) in [
            (Some(23), 2.0, 3.0), (Some(25), 2.0, 3.0), (Some(0), 2.0, 3.0), (None, 2.0, 3.0), (Some(24), 3.0, 3.0), (Some(24), 0.0, 3.0),
            (Some(24), 2.5, 3.0), (Some(24), -2.0, -3.0), (Some(24), f32::NAN, 3.0), (Some(24), f32::INFINITY, 1.0), (Some(24), 4294967296.0, 4294967296.0),
            (Some(4), 65536.0, 1.0), (Some(0), 0.0, 0.0),
        ] {
            assert!(image_from_slot(bytes.map(|n| vec![0; n]), width, height).is_none(), "{bytes:?} {width} {height}");
        }
    }
}

fn flag(args: &[JsValue], i: usize) -> bool {
    matches!(args.get(i), Some(JsValue::Boolean(true)))
}

fn dimension(args: &[JsValue], i: usize, default: u32) -> u32 {
    let v = num(args, i);
    if v.is_finite() && v >= 0.0 {
        v as u32
    } else {
        default
    }
}

/// The 2D context's script (web_canvas.js), for a lazy install.
pub(crate) const SCRIPT: &str = include_str!("web_canvas.js");

/// The canvas host functions, and with `script` the 2D context's script
/// (a child realm runs it on first use instead: web_lazy.js).
pub(crate) fn install(
    runtime: &mut JsRuntime,
    store: &SharedCanvases,
    host: &SharedDomHost,
    script: bool,
) -> Result<(), JsError> {
    let (s, h) = (store.clone(), host.clone());
    // Pixels cross in the runtime's slot, not as arguments.
    let pixels = runtime.pixel_slot();
    runtime.register_host_function_by_op(
        "__rustkit_canvas",
        12,
        2,
        Box::new(move |args| {
            let Some(op) = text(args, 2) else {
                return JsValue::Undefined;
            };
            let mut store = s.borrow_mut();
            // A gradient stop belongs to no canvas: checked here, kept by
            // the script's CanvasGradient as the colour serialized.
            if op == "gradientStop" {
                let offset = num(args, 3);
                if !(offset.is_finite() && (0.0..=1.0).contains(&offset)) {
                    return JsValue::String("IndexSizeError".into());
                }
                return match text(args, 4).and_then(parse_canvas_color) {
                    Some(color) => JsValue::String(serialize_canvas_color(color)),
                    None => JsValue::String("SyntaxError".into()),
                };
            }
            let node = match DomHost::node_of(&h, args) {
                Some(node) => node,
                None => return JsValue::Undefined,
            };
            // What an element is as an image source, from the engine's own
            // record of it: "w,h" (its image's size) once its image is in,
            // "" before that, "TypeError" for anything that is not an `<img>`.
            // `imagePattern` also keeps the resource for a pattern and
            // answers "w,h,index".
            if op == "imageSource" || op == "imagePattern" {
                let Some((src, srcset)) = h.borrow().img_sources(node) else {
                    return JsValue::String("TypeError".into());
                };
                let Some(image) = store.loaded.get(&node).filter(|image| image.src == src && image.srcset == srcset).cloned() else {
                    return JsValue::String(String::new());
                };
                if image.width == 0 || image.height == 0 {
                    return JsValue::String(String::new());
                }
                let size = format!("{},{}", image.width, image.height);
                if op == "imageSource" {
                    return JsValue::String(size);
                }
                let index = match store.pattern_sources.iter().position(|kept| *kept == image) {
                    Some(index) => index,
                    None => {
                        store.pattern_sources.push(image);
                        store.pattern_sources.len() - 1
                    }
                };
                return JsValue::String(format!("{size},{index}"));
            }
            if op == "create" {
                let (w, hgt) = (dimension(args, 3, 300), dimension(args, 4, 150));
                store
                    .contexts
                    .entry(node)
                    .or_insert_with(|| CanvasRenderingContext2D::new(w, hgt));
                store.painted = true;
                return JsValue::Undefined;
            }
            if op == "measure" {
                let (Some(t), Some(font)) = (
                    text(args, 3),
                    store.contexts.get(&node).map(|c| c.state.font.clone()),
                ) else {
                    return JsValue::Null;
                };
                let e = store.measure_font(&font, t);
                return JsValue::String(format!("{} {} {}", e.width, e.ascent, e.descent));
            }
            // A source bitmap named by ("canvas", node) or ("image", url),
            // read before the context is borrowed for writing.
            // Whether the source is not origin-clean: a canvas that is not,
            // or an image of another origin than the document's (a data:
            // URL is the document's own). There is no CORS-mode image
            // fetch, so a crossorigin image counts as another origin.
            let mut tainted = false;
            let source = if matches!(op, "drawImage" | "fillPattern" | "strokePattern") {
                // An image source is ("image", the element's NodeId) for a
                // draw and ("pattern", the index `imagePattern` gave) for a
                // pattern: its URL is the engine's record, never an argument.
                let image: Option<LoadedImage> = match (text(args, 3), args.get(4)) {
                    (Some("image"), Some(JsValue::Number(n))) if *n >= 0.0 && n.fract() == 0.0 => {
                        let element = *n as usize;
                        let now = h.borrow().img_sources(element);
                        store.loaded.get(&element).filter(|image| now.as_ref().is_some_and(|(src, srcset)| image.src == *src && image.srcset == *srcset)).cloned()
                    }
                    (Some("pattern"), Some(JsValue::Number(n))) if *n >= 0.0 && n.fract() == 0.0 => store.pattern_sources.get(*n as usize).cloned(),
                    _ => None,
                };
                let is_image = matches!(text(args, 3), Some("image" | "pattern"));
                if is_image && image.is_none() {
                    return JsValue::Undefined;
                }
                // Decided on the URL, before its pixels are looked for: the
                // answer does not depend on what the image store holds.
                tainted = match (text(args, 3), args.get(4), &image) {
                    (Some("canvas"), Some(JsValue::Number(n)), _) => {
                        store.contexts.get(&(*n as usize)).is_some_and(|c| !c.is_origin_clean())
                    }
                    (_, _, Some(image)) => match url::Url::parse(&image.url) {
                        Ok(u) if u.scheme() == "data" => false,
                        Ok(u) => store.origin.as_ref() != Some(&u.origin()) || !u.origin().is_tuple(),
                        Err(_) => true,
                    },
                    _ => false,
                };
                let url = image.as_ref().map(|image| image.url.clone());
                let found = match (text(args, 3), args.get(4)) {
                    (Some("canvas"), Some(JsValue::Number(n))) => {
                        let (w, hh) = store.contexts.get(&(*n as usize)).map(|c| (c.width, c.height)).unwrap_or((0, 0));
                        store.contexts.get(&(*n as usize)).and_then(|c| c.bitmap()).or_else(|| {
                            // A canvas nothing was drawn on: transparent, its size.
                            CanvasBitmap::from_rgba(w, hh, &vec![0; w as usize * hh as usize * 4])
                        })
                    }
                    (Some("image" | "pattern"), _) => {
                        let url = url.as_deref().unwrap_or_default();
                        // drawImage: the size the whole image is drawn at, from
                        // the dest/source ratio and the transform's scale.
                        let size = match (op, number_list::<8>(text(args, 5)), number_list::<2>(text(args, 6))) {
                            ("drawImage", Some([_, _, sw, sh, _, _, dw, dh]), Some([nw, nh])) if sw != 0.0 && sh != 0.0 => {
                                let t = store.contexts.get(&node).map(|c| c.get_transform()).unwrap_or_else(Transform2D::identity);
                                let kx = (dw / sw).abs() * t.a.hypot(t.b);
                                let ky = (dh / sh).abs() * t.c.hypot(t.d);
                                let w = (nw * kx).round();
                                let h = (nh * ky).round();
                                if w >= 1.0 && h >= 1.0 && w <= 8192.0 && h <= 8192.0 { (w as u32, h as u32) } else { (0, 0) }
                            }
                            _ => (0, 0),
                        };
                        let loaded = store.images.as_ref().and_then(|load| load(url, size));
                        // Any response URL of another origin than the
                        // document's (a redirect through one) taints too.
                        if let Some((_, hops)) = &loaded {
                            if hops.iter().any(|u| u.scheme() != "data" && store.origin.as_ref() != Some(&u.origin())) {
                                tainted = true;
                            }
                        }
                        loaded.map(|(bitmap, _)| bitmap)
                    }
                    _ => None,
                };
                if found.is_none() {
                    // Nothing to draw; a canvas asked to draw another
                    // origin's image is not origin-clean all the same.
                    if tainted {
                        if let Some(ctx) = store.contexts.get_mut(&node) {
                            ctx.taint();
                        }
                    }
                    return JsValue::Undefined;
                }
                found
            } else {
                None
            };
            let CanvasStore {
                contexts, painted, ..
            } = &mut *store;
            let Some(ctx) = contexts.get_mut(&node) else {
                return JsValue::Undefined;
            };
            if tainted {
                ctx.taint();
            }
            let mut drew = true;
            let mut result = JsValue::Undefined;
            match op {
                "resize" => ctx.reset_bitmap(dimension(args, 3, 300), dimension(args, 4, 150)),
                "save" => {
                    ctx.save();
                    drew = false;
                }
                "restore" => ctx.restore(),
                "scale" => {
                    if let Some([x, y]) = nums::<2>(args, 3) {
                        ctx.scale(x, y);
                    }
                    drew = false;
                }
                "rotate" => {
                    if let Some([a]) = nums::<1>(args, 3) {
                        ctx.rotate(a);
                    }
                    drew = false;
                }
                "translate" => {
                    if let Some([x, y]) = nums::<2>(args, 3) {
                        ctx.translate(x, y);
                    }
                    drew = false;
                }
                "transform" => {
                    if let Some([a, b, c, d, e, f]) = nums::<6>(args, 3) {
                        ctx.transform(a, b, c, d, e, f);
                    }
                    drew = false;
                }
                "setTransform" => {
                    if let Some([a, b, c, d, e, f]) = nums::<6>(args, 3) {
                        ctx.set_transform(a, b, c, d, e, f);
                    }
                    drew = false;
                }
                "resetTransform" => {
                    ctx.reset_transform();
                    drew = false;
                }
                "fillColor" | "strokeColor" => {
                    drew = false;
                    result = match text(args, 3).and_then(parse_canvas_color) {
                        Some(color) => {
                            if op == "fillColor" {
                                ctx.set_fill_style(CanvasStyle::Color(color));
                            } else {
                                ctx.set_stroke_style(CanvasStyle::Color(color));
                            }
                            JsValue::String(serialize_canvas_color(color))
                        }
                        None => JsValue::Null,
                    };
                }
                "fillGradient" | "strokeGradient" => {
                    drew = false;
                    // x0 y0 x1 y1, then "offset|colour|offset|colour...".
                    if let Some([x0, y0, x1, y1]) = nums::<4>(args, 3) {
                        let mut g = LinearGradient::new(x0, y0, x1, y1);
                        for (offset, color) in color_stops(text(args, 7).unwrap_or("")) {
                            g.add_color_stop(offset, color);
                        }
                        let style = CanvasStyle::LinearGradient(g);
                        if op == "fillGradient" {
                            ctx.set_fill_style(style);
                        } else {
                            ctx.set_stroke_style(style);
                        }
                    }
                }
                "fillRadial" | "strokeRadial" => {
                    drew = false;
                    // x0 y0 r0 x1 y1 r1, then the stops.
                    if let Some([x0, y0, r0, x1, y1, r1]) = nums::<6>(args, 3) {
                        let mut g = RadialGradient::new(x0, y0, r0, x1, y1, r1);
                        for (offset, color) in color_stops(text(args, 9).unwrap_or("")) {
                            g.add_color_stop(offset, color);
                        }
                        let style = CanvasStyle::RadialGradient(g);
                        if op == "fillRadial" {
                            ctx.set_fill_style(style);
                        } else {
                            ctx.set_stroke_style(style);
                        }
                    }
                }
                "fillPattern" | "strokePattern" => {
                    drew = false;
                    let repetition = match text(args, 5) {
                        Some("repeat-x") => PatternRepetition::RepeatX,
                        Some("repeat-y") => PatternRepetition::RepeatY,
                        Some("no-repeat") => PatternRepetition::NoRepeat,
                        _ => PatternRepetition::Repeat,
                    };
                    if let Some(bitmap) = source {
                        let style = CanvasStyle::Pattern(CanvasPattern { bitmap: std::sync::Arc::new(bitmap), repetition });
                        if op == "fillPattern" {
                            ctx.set_fill_style(style);
                        } else {
                            ctx.set_stroke_style(style);
                        }
                    }
                }
                "drawImage" => {
                    if let (Some(bitmap), Some([sx, sy, sw, sh, dx, dy, dw, dh])) = (source, number_list::<8>(text(args, 5))) {
                        // A bitmap rasterized at another size than the
                        // image's natural one (an SVG): the source rect in
                        // its pixels.
                        let (fx, fy) = match number_list::<2>(text(args, 6)) {
                            Some([nw, nh]) if nw > 0.0 && nh > 0.0 => (bitmap.width() as f32 / nw, bitmap.height() as f32 / nh),
                            _ => (1.0, 1.0),
                        };
                        ctx.draw_bitmap(bitmap, [sx * fx, sy * fy, sw * fx, sh * fy], [dx, dy, dw, dh]);
                    }
                }
                "putImageData" => {
                    // width, height, dx, dy, then the dirty rect (cut to the
                    // image); the bytes are what the script's `pixelsIn`
                    // left in the slot, and must be the image's size.
                    let data = pixels.take();
                    match (nums::<4>(args, 3), nums::<4>(args, 7)) {
                        (Some([w, hh, dx, dy]), Some([x, y, dw, dh])) => match image_from_slot(data, w, hh) {
                            // The casts saturate, and the canvas cuts the rect
                            // to the image and the bitmap (`raster::put_span`).
                            Some(data) => ctx.put_image_data_dirty(&data, dx as i32, dy as i32, [x as i32, y as i32, dw as i32, dh as i32]),
                            None => drew = false,
                        },
                        _ => drew = false,
                    }
                }
                "lineWidth" => {
                    ctx.set_line_width(num(args, 3));
                    drew = false;
                }
                "miterLimit" => {
                    let v = num(args, 3);
                    if v.is_finite() && v > 0.0 {
                        ctx.set_miter_limit(v);
                    }
                    drew = false;
                }
                "lineCap" => {
                    drew = false;
                    match text(args, 3) {
                        Some("butt") => ctx.set_line_cap(LineCap::Butt),
                        Some("round") => ctx.set_line_cap(LineCap::Round),
                        Some("square") => ctx.set_line_cap(LineCap::Square),
                        _ => {}
                    }
                }
                "lineJoin" => {
                    drew = false;
                    match text(args, 3) {
                        Some("miter") => ctx.set_line_join(LineJoin::Miter),
                        Some("round") => ctx.set_line_join(LineJoin::Round),
                        Some("bevel") => ctx.set_line_join(LineJoin::Bevel),
                        _ => {}
                    }
                }
                "lineDash" => {
                    drew = false;
                    let list: Option<Vec<f32>> = text(args, 3).map(|t| {
                        t.split(',')
                            .filter(|p| !p.is_empty())
                            .map(|p| p.parse::<f32>().unwrap_or(f32::NAN))
                            .collect()
                    });
                    if let Some(list) = list {
                        ctx.set_line_dash(list);
                    }
                }
                "lineDashOffset" => {
                    drew = false;
                    let v = num(args, 3);
                    if v.is_finite() {
                        ctx.set_line_dash_offset(v);
                    }
                }
                "font" => {
                    drew = false;
                    result = match text(args, 3).and_then(|f| CanvasFont::parse(f, FONT_BASE_PX)) {
                        Some(font) => {
                            let serialized = font.serialize();
                            ctx.set_font(&serialized);
                            JsValue::String(serialized)
                        }
                        None => JsValue::Null,
                    };
                }
                "textAlign" => {
                    drew = false;
                    match text(args, 3) {
                        Some("start") => ctx.set_text_align(TextAlign::Start),
                        Some("end") => ctx.set_text_align(TextAlign::End),
                        Some("left") => ctx.set_text_align(TextAlign::Left),
                        Some("right") => ctx.set_text_align(TextAlign::Right),
                        Some("center") => ctx.set_text_align(TextAlign::Center),
                        _ => {}
                    }
                }
                "textBaseline" => {
                    drew = false;
                    match text(args, 3) {
                        Some("top") => ctx.set_text_baseline(TextBaseline::Top),
                        Some("hanging") => ctx.set_text_baseline(TextBaseline::Hanging),
                        Some("middle") => ctx.set_text_baseline(TextBaseline::Middle),
                        Some("alphabetic") => ctx.set_text_baseline(TextBaseline::Alphabetic),
                        Some("ideographic") => ctx.set_text_baseline(TextBaseline::Ideographic),
                        Some("bottom") => ctx.set_text_baseline(TextBaseline::Bottom),
                        _ => {}
                    }
                }
                "globalAlpha" => {
                    ctx.set_global_alpha(num(args, 3));
                    drew = false;
                }
                "fillRect" | "strokeRect" | "clearRect" => {
                    if let Some([x, y, w, hh]) = nums::<4>(args, 3) {
                        match op {
                            "fillRect" => ctx.fill_rect(x, y, w, hh),
                            "strokeRect" => ctx.stroke_rect(x, y, w, hh),
                            _ => ctx.clear_rect(x, y, w, hh),
                        }
                    }
                }
                "beginPath" => {
                    ctx.begin_path();
                    drew = false;
                }
                "closePath" => {
                    ctx.close_path();
                    drew = false;
                }
                "moveTo" | "lineTo" => {
                    drew = false;
                    if let Some([x, y]) = nums::<2>(args, 3) {
                        if op == "moveTo" {
                            ctx.move_to(x, y);
                        } else {
                            ctx.line_to(x, y);
                        }
                    }
                }
                "quadraticCurveTo" => {
                    drew = false;
                    if let Some([a, b, c, d]) = nums::<4>(args, 3) {
                        ctx.quadratic_curve_to(a, b, c, d);
                    }
                }
                "bezierCurveTo" => {
                    drew = false;
                    if let Some([a, b, c, d, e, f]) = nums::<6>(args, 3) {
                        ctx.bezier_curve_to(a, b, c, d, e, f);
                    }
                }
                "arc" => {
                    drew = false;
                    if let Some([x, y, r, a0, a1]) = nums::<5>(args, 3) {
                        if r < 0.0 {
                            result = JsValue::String("IndexSizeError".into());
                        } else {
                            ctx.arc(x, y, r, a0, a1, flag(args, 8));
                        }
                    }
                }
                "arcTo" => {
                    drew = false;
                    if let Some([x1, y1, x2, y2, r]) = nums::<5>(args, 3) {
                        if r < 0.0 {
                            result = JsValue::String("IndexSizeError".into());
                        } else {
                            ctx.arc_to(x1, y1, x2, y2, r);
                        }
                    }
                }
                "ellipse" => {
                    drew = false;
                    if let Some([x, y, rx, ry, rot, a0, a1]) = nums::<7>(args, 3) {
                        if rx < 0.0 || ry < 0.0 {
                            result = JsValue::String("IndexSizeError".into());
                        } else {
                            ctx.ellipse(x, y, rx, ry, rot, a0, a1, flag(args, 10));
                        }
                    }
                }
                "rect" => {
                    drew = false;
                    if let Some([x, y, w, hh]) = nums::<4>(args, 3) {
                        ctx.rect(x, y, w, hh);
                    }
                }
                "getImageData" => {
                    drew = false;
                    if !ctx.is_origin_clean() {
                        return JsValue::String("SecurityError".into());
                    }
                    if let Some([x, y, w, hh]) = nums::<4>(args, 3) {
                        if rustkit_canvas::rgba_len(w as u32, hh as u32).is_none() || w > u32::MAX as f32 || hh > u32::MAX as f32 {
                            return JsValue::String("RangeError".into());
                        }
                        // The bytes wait in the slot for the script's
                        // `pixelsOut`; the answer is how many.
                        let d = ctx.get_image_data(x as i32, y as i32, w as u32, hh as u32);
                        result = JsValue::Number(d.data.len() as f64);
                        pixels.put(d.data);
                    }
                }
                "fill" => ctx.fill_with_rule(text(args, 3) == Some("evenodd")),
                "stroke" => ctx.stroke(),
                "clip" => ctx.clip_with_rule(text(args, 3) == Some("evenodd")),
                "fillText" | "strokeText" => {
                    if let (Some(t), Some([x, y])) = (text(args, 3), nums::<2>(args, 4)) {
                        let max = num(args, 6);
                        let max = if matches!(args.get(6), Some(JsValue::Number(_))) {
                            Some(max)
                        } else {
                            None
                        };
                        // Whitespace characters become spaces (§4.12.5.1.13 step 1).
                        let t: String = t
                            .chars()
                            .map(|c| {
                                if matches!(c, '\t' | '\n' | '\u{c}' | '\r') {
                                    ' '
                                } else {
                                    c
                                }
                            })
                            .collect();
                        if op == "fillText" {
                            ctx.fill_text_max(&t, x, y, max);
                        } else {
                            ctx.stroke_text_max(&t, x, y, max);
                        }
                    }
                }
                _ => drew = false,
            }
            if drew {
                *painted = true;
            }
            result
        }),
    )?;
    // The host function is the canvas script's alone: moved to the host
    // object now, before any page script, and never on the window again.
    runtime.evaluate_with_host("__rk.canvasCall = globalThis.__rustkit_canvas; delete globalThis.__rustkit_canvas;")?;
    if !script {
        return Ok(());
    }
    runtime.evaluate_with_host(SCRIPT).map(|_| ())
}
