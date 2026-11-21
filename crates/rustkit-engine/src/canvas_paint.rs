//! Canvas 2D paint (HTML §4.12.5, CH2.E10 phase 1): a `<canvas>` lays out
//! as a replaced box (`BoxType::Image` under [`canvas_key`], its natural
//! size the bitmap's), and when the display list is built the image
//! command is replaced by what script drew on the canvas's 2D context
//! (`DomBindings::canvas_snapshot`), as vector commands mapped from bitmap
//! pixels onto the box: fills as tessellated polygons (rustkit-svg's fill),
//! strokes as polylines, text as text runs, the clip as a clip rect.
//!
//! Vector, not a raster: the renderer already draws every one of these
//! commands (inline SVG is painted the same way), text goes through the
//! same shaper and glyph cache as the page's own text, and a canvas drawn
//! at devicePixelRatio stays sharp. The cost is what a vector recording
//! cannot express (see the module's "not done" list in the PR): compositing
//! modes, pixel reads. A partial `clearRect` is painted as clips: what was
//! drawn before it paints only outside the rects cleared after it.

use rustkit_bindings::canvas::{
    command_extent, CanvasFont, CanvasStyle, DrawCommand, LineDash, LinearGradient, TextAlign, TextBaseline,
    Transform2D,
};
use rustkit_bindings::{CanvasSnapshot, CanvasSource, CanvasTextExtent, CanvasTextMeasurer};
use rustkit_css::Color;
use rustkit_layout::{DisplayCommand, Rect};

const KEY_PREFIX: &str = "rustkit-canvas:";

/// The image key a canvas element's box paints under.
pub(crate) fn canvas_key(node: usize) -> String {
    format!("{KEY_PREFIX}{node}")
}

/// The canvas element a box's image key names, if it names one.
pub(crate) fn canvas_node(key: &str) -> Option<usize> {
    key.strip_prefix(KEY_PREFIX)?.parse().ok()
}

/// A canvas's bitmap size from its `width`/`height` attributes (HTML
/// §4.12.5: rules for parsing non-negative integers, 300x150 otherwise).
pub(crate) fn bitmap_size(width: Option<&String>, height: Option<&String>) -> (f32, f32) {
    fn parse(v: Option<&String>, default: f32) -> f32 {
        let Some(v) = v else { return default };
        let t = v.trim_start_matches([' ', '\t', '\n', '\r', '\u{c}']);
        let t = t.strip_prefix('+').unwrap_or(t);
        let digits: String = t.chars().take_while(|c| c.is_ascii_digit()).collect();
        match digits.parse::<u64>() {
            Ok(n) if n <= i32::MAX as u64 => n as f32,
            _ => default,
        }
    }
    (parse(width, 300.0), parse(height, 150.0))
}

/// The engine's text shaper for canvas text and `measureText`.
pub(crate) fn text_measurer() -> CanvasTextMeasurer {
    Box::new(|font: &CanvasFont, text: &str| {
        let m = measure(font, text);
        CanvasTextExtent {
            width: m.width,
            ascent: m.ascent,
            descent: m.descent,
        }
    })
}

/// The SVG images a canvas can draw, by URL (`LayoutCtx::canvas_svgs`).
pub(crate) type CanvasSvgs =
    std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, std::sync::Arc<rustkit_svg::SvgDocument>>>>;

/// The image cache as canvas `drawImage`/`createPattern` reads it: an
/// `<img>`'s decoded pixels by URL (its first frame), `None` when the
/// image is not decoded. An SVG image is rasterized at `size` (its natural
/// size for `(0, 0)`), as Chrome rasterizes it at the size it is drawn.
pub(crate) fn image_loader(images: std::sync::Arc<rustkit_image::ImageManager>, svgs: CanvasSvgs) -> rustkit_bindings::CanvasImageLoader {
    Box::new(move |url: &str, size: (u32, u32)| {
        let doc = svgs.lock().ok().and_then(|m| m.get(url).cloned());
        let url = url::Url::parse(url).ok()?;
        let hops = images.response_urls(&url);
        if let Some(doc) = doc {
            return svg_bitmap(&doc, size).map(|b| (b, hops));
        }
        let image = images.get_cached(&url)?;
        let frame = image.current_frame(std::time::Duration::ZERO);
        rustkit_bindings::canvas::CanvasBitmap::from_rgba(frame.width(), frame.height(), frame.data()).map(|b| (b, hops))
    })
}

/// `doc` rasterized at `size` (its natural size for `(0, 0)`, 300x150
/// with none): its path list filled and stroked by a scratch canvas
/// context, so an SVG drawn into a canvas uses the canvas's own raster.
fn svg_bitmap(doc: &rustkit_svg::SvgDocument, size: (u32, u32)) -> Option<rustkit_bindings::canvas::CanvasBitmap> {
    use rustkit_bindings::canvas::{CanvasRenderingContext2D, CanvasStyle, LineCap, LineJoin};
    let (w, h) = if size.0 > 0 && size.1 > 0 {
        size
    } else {
        let (w, h) = doc.get_size(300.0, 150.0);
        (w.round().max(1.0) as u32, h.round().max(1.0) as u32)
    };
    let mut ctx = CanvasRenderingContext2D::new(w, h);
    let color = CanvasStyle::Color;
    for item in doc.path_list(0.0, 0.0, w as f32, h as f32, Color::BLACK) {
        ctx.begin_path();
        for (points, closed) in &item.contours {
            let mut it = points.iter();
            if let Some(&(x, y)) = it.next() {
                ctx.move_to(x, y);
            }
            for &(x, y) in it {
                ctx.line_to(x, y);
            }
            if *closed {
                ctx.close_path();
            }
        }
        if let Some(fill) = item.fill {
            ctx.set_fill_style(color(fill));
            ctx.fill_with_rule(item.even_odd);
        }
        if let Some(stroke) = item.stroke {
            ctx.set_stroke_style(color(stroke.color));
            ctx.set_line_width(stroke.width);
            ctx.set_line_cap(match stroke.cap {
                rustkit_svg::LineCap::Butt => LineCap::Butt,
                rustkit_svg::LineCap::Round => LineCap::Round,
                rustkit_svg::LineCap::Square => LineCap::Square,
            });
            ctx.set_line_join(match stroke.join {
                rustkit_svg::LineJoin::Miter => LineJoin::Miter,
                rustkit_svg::LineJoin::Round => LineJoin::Round,
                rustkit_svg::LineJoin::Bevel => LineJoin::Bevel,
            });
            ctx.set_miter_limit(stroke.miter_limit);
            ctx.stroke();
        }
    }
    ctx.bitmap().or_else(|| rustkit_bindings::canvas::CanvasBitmap::from_rgba(w, h, &vec![0; w as usize * h as usize * 4]))
}

fn measure(font: &CanvasFont, text: &str) -> rustkit_layout::TextMetrics {
    rustkit_layout::measure_text_advanced(
        text,
        &font.family,
        font.size,
        rustkit_css::FontWeight(font.weight),
        if font.italic {
            rustkit_css::FontStyle::Italic
        } else {
            rustkit_css::FontStyle::Normal
        },
    )
}

fn initial_font() -> CanvasFont {
    CanvasFont::parse("10px sans-serif", 16.0).expect("initial canvas font")
}

/// A canvas bitmap waiting for, or kept beside, its texture: premultiplied
/// RGBA rows.
pub(crate) struct CanvasFrame {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) rgba: Vec<u8>,
}

/// The canvas bitmaps the display lists name, by image key
/// (`rustkit-canvas:<node>@<serial>`): the presenter uploads one when a
/// list it draws names it (`upload_display_list_images`). A key is new for
/// every bitmap a canvas paints, so a texture is never stale, and leaves
/// this map when the paint that made it is dropped.
static FRAMES: std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<String, std::sync::Arc<CanvasFrame>>>> =
    std::sync::LazyLock::new(Default::default);
static SERIAL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Whether `url` is a canvas bitmap's image key (as opposed to the layout
/// placeholder `canvas_key`).
pub(crate) fn is_bitmap_key(url: &str) -> bool {
    url.strip_prefix(KEY_PREFIX).is_some_and(|rest| rest.contains('@'))
}

/// The bitmap a display list names under `key`, if its paint is alive.
pub(crate) fn bitmap_frame(key: &str) -> Option<std::sync::Arc<CanvasFrame>> {
    FRAMES.lock().ok()?.get(key).cloned()
}

/// GPU canvas A1: a canvas's batches on their way from the page thread
/// (display-list build) to the presenter (`upload_display_list_images`),
/// which replays them into the canvas's texture. Batches wait here, not in
/// the frame commit: a commit can be replaced before it is drawn, and its
/// batches would be lost with it.
#[derive(Default)]
pub(crate) struct Mailbox {
    batches: Vec<rustkit_bindings::canvas::GpuBatch>,
    bytes: usize,
    /// The presenter could not draw the batches onto a texture as it was
    /// (missing, or batches dropped here): the next batch must be a reset.
    needs_reset: bool,
    /// The presenter keeps no texture for this canvas: CPU bitmap for good.
    denied: bool,
}

/// The most bytes of batches a mailbox holds: past it (a window that is
/// not presenting) they are dropped and the canvas starts over from its
/// bitmap at the next batch.
const MAILBOX_BYTE_CAP: usize = rustkit_bindings::canvas::PENDING_BYTE_BUDGET;

/// Mailboxes by GPU key (`rustkit-canvas:<node>@g<context id>`); one goes
/// when the paint that made it is dropped.
static MAILBOXES: std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<String, Mailbox>>> =
    std::sync::LazyLock::new(Default::default);

/// Whether `url` names a canvas's GPU texture.
pub(crate) fn is_gpu_key(url: &str) -> bool {
    url.strip_prefix(KEY_PREFIX).is_some_and(|rest| rest.contains("@g"))
}

/// Presenter: the batches waiting for `key`, taken.
pub(crate) fn take_gpu_batches(key: &str) -> Vec<rustkit_bindings::canvas::GpuBatch> {
    let Ok(mut boxes) = MAILBOXES.lock() else { return Vec::new() };
    let Some(m) = boxes.get_mut(key) else { return Vec::new() };
    m.bytes = 0;
    std::mem::take(&mut m.batches)
}

/// Presenter: what the replay of `key`'s batches came to.
pub(crate) fn gpu_replayed(key: &str, result: rustkit_renderer::CanvasReplay) {
    let Ok(mut boxes) = MAILBOXES.lock() else { return };
    let Some(m) = boxes.get_mut(key) else { return };
    match result {
        rustkit_renderer::CanvasReplay::Drawn => {}
        rustkit_renderer::CanvasReplay::NeedsReset => m.needs_reset = true,
        rustkit_renderer::CanvasReplay::Denied => m.denied = true,
    }
}

/// Page thread: hands `node`'s new draws to the presenter under `key`;
/// `false` when the presenter denied the canvas a texture (the caller shows
/// the CPU bitmap). A mailbox made here starts with a reset: the texture
/// under a key nobody posted to is unknown.
fn post_gpu_batch(source: &CanvasSource, node: usize, key: &str) -> bool {
    let Ok(mut boxes) = MAILBOXES.lock() else { return false };
    let fresh = !boxes.contains_key(key);
    let m = boxes.entry(key.to_string()).or_default();
    if m.denied {
        return false;
    }
    let force = fresh || std::mem::take(&mut m.needs_reset);
    let Some(batch) = source.take_gpu_batch(node, force) else { return false };
    if batch.reset.is_some() {
        m.batches.clear();
        m.bytes = 0;
    }
    m.bytes += batch.bytes();
    m.batches.push(batch);
    if m.bytes > MAILBOX_BYTE_CAP {
        m.batches.clear();
        m.bytes = 0;
        m.needs_reset = true;
    }
    true
}

/// Whether a `w` x `h` canvas can have a texture: some area, within the
/// largest texture every device takes (wgpu's default limit), and its
/// scratch within the renderer's budget. Checked here so a canvas that
/// cannot is never posted (the presenter's own check stays).
fn gpu_fits(w: u32, h: u32) -> bool {
    const MAX_SIDE: u32 = 8192;
    w > 0 && h > 0 && w <= MAX_SIDE && h <= MAX_SIDE && w as u64 * h as u64 * 20 <= rustkit_renderer::CANVAS_SCRATCH_BUDGET
}

/// Whether the presenter asked `key` for a reset.
fn gpu_needs_reset(key: &str) -> bool {
    MAILBOXES.lock().ok().and_then(|b| b.get(key).map(|m| m.needs_reset)).unwrap_or(false)
}

/// Whether the presenter denied `key` a texture.
fn gpu_denied(key: &str) -> bool {
    MAILBOXES.lock().ok().and_then(|b| b.get(key).map(|m| m.denied)).unwrap_or(false)
}

/// A canvas's paint as the last display list built it: for the bitmap at
/// `generation`, in a box whose fitted image rect was `dest` and content
/// box `content`.
pub(crate) struct CanvasPaint {
    generation: u64,
    dest: Rect,
    content: Rect,
    /// The bitmap's image key in `FRAMES`.
    key: String,
    /// What the text overlay drew (text is not in the bitmap yet), with
    /// the bitmap's image command first.
    pub(crate) commands: Vec<DisplayCommand>,
}

impl Drop for CanvasPaint {
    fn drop(&mut self) {
        if is_gpu_key(&self.key) {
            if let Ok(mut boxes) = MAILBOXES.lock() {
                boxes.remove(&self.key);
            }
            return;
        }
        if let Ok(mut frames) = FRAMES.lock() {
            frames.remove(&self.key);
        }
    }
}

fn same(a: Rect, b: Rect) -> bool {
    (a.x, a.y, a.width, a.height) == (b.x, b.y, b.width, b.height)
}

impl CanvasPaint {
    /// `last` again when the canvas `node` has not drawn since and its box
    /// is where it was; otherwise the canvas painted afresh into `image`,
    /// the box's image command. A canvas paints its bitmap (STR-7b) as an
    /// image under a key of its own, fitted to the box like any replaced
    /// image (`object-fit`, bilinear scaling from the backing store to the
    /// CSS size, as Chrome does). The bitmap is read again only when the
    /// canvas drew. `None` with no 2D context.
    pub(crate) fn reuse_or_paint(
        last: Option<CanvasPaint>,
        source: &CanvasSource,
        node: usize,
        image: &DisplayCommand,
    ) -> Option<CanvasPaint> {
        let DisplayCommand::Image { dest_rect: dest, content_box: content, .. } = image else {
            return None;
        };
        let (dest, content) = (*dest, *content);
        let generation = source.generation(node)?;
        // GPU canvas A1: the canvas's texture under a key that stays while
        // the context lives; each new paint hands the draws since the last
        // one to the presenter instead of copying the bitmap.
        let gpu_key = match source.gpu_context(node) {
            Some((id, w, h)) if gpu_fits(w, h) => Some(format!("{KEY_PREFIX}{node}@g{id}")),
            Some(_) => {
                // No texture this size: from the bitmap for good.
                source.leave_gpu(node);
                None
            }
            None => None,
        };
        let mut last = last;
        if let Some(key) = &gpu_key {
            if gpu_denied(key) {
                if let Ok(mut boxes) = MAILBOXES.lock() {
                    boxes.remove(key);
                }
                source.leave_gpu(node);
            } else {
                let mut kept = None;
                if let Some(l) = last.as_mut().filter(|l| &l.key == key) {
                    if l.generation == generation && same(l.dest, dest) && same(l.content, content) && !gpu_needs_reset(key) {
                        return last;
                    }
                    // Taken, so that dropping `l` keeps the mailbox.
                    kept = Some((l.generation, std::mem::take(&mut l.key)));
                }
                drop(last.take());
                let drew = kept.as_ref().is_none_or(|(g, _)| *g != generation) || gpu_needs_reset(key);
                if !drew || post_gpu_batch(source, node, key) {
                    let paint = Self::paint_with_key(source, node, image, generation, dest, content, key.clone());
                    return Some(paint);
                }
                // Denied just now: from the bitmap from here on.
                if let Ok(mut boxes) = MAILBOXES.lock() {
                    boxes.remove(key);
                }
                source.leave_gpu(node);
            }
        }
        let reused_key = match last {
            Some(last) if last.generation == generation => {
                if same(last.dest, dest) && same(last.content, content) {
                    return Some(last);
                }
                // The same bitmap in a moved box: keep its key and texture.
                let mut last = last;
                Some(std::mem::take(&mut last.key))
            }
            _ => None,
        };
        let key = match reused_key {
            Some(key) => key,
            None => {
                let (width, height, rgba) = source.pixels(node)?;
                if width == 0 || height == 0 || rgba.len() as u64 != width as u64 * height as u64 * 4 {
                    // Nothing to show: a zero-size texture is invalid, and a
                    // canvas past the size limits has no bitmap.
                    return Some(CanvasPaint { generation, dest, content, key: String::new(), commands: Vec::new() });
                }
                let key = format!(
                    "{KEY_PREFIX}{node}@{}",
                    SERIAL.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                );
                if let Ok(mut frames) = FRAMES.lock() {
                    frames.insert(key.clone(), std::sync::Arc::new(CanvasFrame { width, height, rgba }));
                }
                key
            }
        };
        Some(Self::paint_with_key(source, node, image, generation, dest, content, key))
    }

    /// The paint naming the bitmap or texture `key`, with the text overlay.
    fn paint_with_key(
        source: &CanvasSource,
        node: usize,
        image: &DisplayCommand,
        generation: u64,
        dest: Rect,
        content: Rect,
        key: String,
    ) -> CanvasPaint {
        let mut commands = Vec::new();
        let mut bitmap = image.clone();
        if let DisplayCommand::Image { url, .. } = &mut bitmap {
            *url = key.clone();
        }
        commands.push(bitmap);
        if let Some(mut snapshot) = source.snapshot(node) {
            // Text is drawn over the bitmap from the recording, with the
            // clears and clips that cut it (slice 3 rasterizes text).
            snapshot.commands.retain(|c| {
                matches!(
                    c,
                    DrawCommand::FillText { .. }
                        | DrawCommand::StrokeText { .. }
                        | DrawCommand::Clip { .. }
                        | DrawCommand::ClearRect { .. }
                )
            });
            if snapshot
                .commands
                .iter()
                .any(|c| matches!(c, DrawCommand::FillText { .. } | DrawCommand::StrokeText { .. }))
            {
                commands.extend(paint_commands(&snapshot, dest, content));
            }
        }
        PAINTS.with(|p| p.set(p.get() + 1));
        CanvasPaint { generation, dest, content, key, commands }
    }
}

thread_local! {
    static PAINTS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// How many times this thread painted a canvas afresh
/// (`CanvasPaint::reuse_or_paint`): a test's window on the paint cache.
#[cfg(test)]
pub(crate) fn paints() -> usize {
    PAINTS.with(|p| p.get())
}

/// The commands that paint `canvas` into a box whose fitted image rect is
/// `dest` and whose content box is `content`: bitmap pixel (x, y) lands at
/// `dest.x + x * dest.width / bitmap width`, and nothing paints outside the
/// content box.
pub(crate) fn paint_commands(
    canvas: &CanvasSnapshot,
    dest: Rect,
    content: Rect,
) -> Vec<DisplayCommand> {
    let mut out = Vec::new();
    if canvas.commands.is_empty() || canvas.width == 0 || canvas.height == 0 {
        return out;
    }
    let to_box = Transform2D::identity().translate(dest.x, dest.y).scale(
        dest.width / canvas.width as f32,
        dest.height / canvas.height as f32,
    );
    let frame = intersect(content, dest);
    let bitmap = [0.0, 0.0, canvas.width as f32, canvas.height as f32];
    let commands = &canvas.commands[..];
    // What each partial clearRect erases, in bitmap pixels: its rect cut
    // to the bitmap and the clip it was recorded under (rustkit-canvas
    // records only axis-aligned partial clears).
    let mut clip: Option<[f32; 4]> = None;
    let mut clears: Vec<(usize, [f32; 4])> = Vec::new();
    for (i, command) in commands.iter().enumerate() {
        match command {
            DrawCommand::Clip { rect } => clip = *rect,
            DrawCommand::ClearRect { transform, .. } if transform.b == 0.0 && transform.c == 0.0 => {
                let mut r = cut(command_extent(command, bitmap), bitmap);
                if let Some(c) = clip {
                    r = cut(r, c);
                }
                if r[0] < r[2] && r[1] < r[3] {
                    clears.push((i, r));
                }
            }
            _ => {}
        }
    }
    out.push(DisplayCommand::PushClip(frame));
    // The commands between one clear and the next paint where none of the
    // clears after them erased: under each rect of the bitmap less those
    // clears, the commands that reach that rect.
    let mut start = 0;
    let mut clip_in_force = None;
    for k in 0..=clears.len() {
        let end = clears.get(k).map_or(commands.len(), |c| c.0);
        let run = &commands[start..end];
        if k == clears.len() {
            emit_run(&mut out, run, clip_in_force, None, &to_box, frame, bitmap);
        } else {
            let erased: Vec<[f32; 4]> = clears[k..].iter().map(|c| c.1).collect();
            for piece in subtract(bitmap, &erased) {
                let reaches = run.iter().any(|c| {
                    !matches!(c, DrawCommand::Clip { .. } | DrawCommand::ClearRect { .. })
                        && overlaps(command_extent(c, bitmap), piece)
                });
                if !reaches {
                    continue;
                }
                out.push(DisplayCommand::PushClip(intersect(box_rect(&to_box, piece), frame)));
                emit_run(&mut out, run, clip_in_force, Some(piece), &to_box, frame, bitmap);
                out.push(DisplayCommand::PopClip);
            }
        }
        for command in run.iter().chain(commands.get(end)) {
            if let DrawCommand::Clip { rect } = command {
                clip_in_force = *rect;
            }
        }
        start = end + 1;
    }
    out.push(DisplayCommand::PopClip);
    out
}

/// `a` cut to `b` (bitmap `[x0, y0, x1, y1]`).
fn cut(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    [a[0].max(b[0]), a[1].max(b[1]), a[2].min(b[2]), a[3].min(b[3])]
}

fn overlaps(a: [f32; 4], b: [f32; 4]) -> bool {
    a[0] < b[2] && a[2] > b[0] && a[1] < b[3] && a[3] > b[1]
}

/// `from` less every rect of `holes`, as disjoint rects.
fn subtract(from: [f32; 4], holes: &[[f32; 4]]) -> Vec<[f32; 4]> {
    let mut left = vec![from];
    for &h in holes {
        let mut next = Vec::with_capacity(left.len() + 3);
        for r in left {
            if !overlaps(r, h) {
                next.push(r);
                continue;
            }
            // Above and below the hole, then its left and right in its band.
            if r[1] < h[1] {
                next.push([r[0], r[1], r[2], h[1]]);
            }
            if h[3] < r[3] {
                next.push([r[0], h[3], r[2], r[3]]);
            }
            let (y0, y1) = (r[1].max(h[1]), r[3].min(h[3]));
            if r[0] < h[0] {
                next.push([r[0], y0, h[0], y1]);
            }
            if h[2] < r[2] {
                next.push([h[2], y0, r[2], y1]);
            }
        }
        left = next;
    }
    left
}

fn box_rect(to_box: &Transform2D, [x0, y0, x1, y1]: [f32; 4]) -> Rect {
    let (ax, ay) = to_box.apply(x0, y0);
    let (bx, by) = to_box.apply(x1, y1);
    Rect::new(ax.min(bx), ay.min(by), (bx - ax).abs(), (by - ay).abs())
}

/// Paint `run`, which starts under the canvas clip `clip` (bitmap px);
/// with `only`, just the commands that reach that bitmap rect.
fn emit_run(
    out: &mut Vec<DisplayCommand>,
    run: &[DrawCommand],
    clip: Option<[f32; 4]>,
    only: Option<[f32; 4]>,
    to_box: &Transform2D,
    frame: Rect,
    bitmap: [f32; 4],
) {
    let mut clipped = false;
    if let Some(r) = clip {
        out.push(DisplayCommand::PushClip(intersect(box_rect(to_box, r), frame)));
        clipped = true;
    }
    for command in run {
        if let (Some(piece), false) = (only, matches!(command, DrawCommand::Clip { .. })) {
            if !overlaps(command_extent(command, bitmap), piece) {
                continue;
            }
        }
        match command {
            DrawCommand::Clip { rect } => {
                if clipped {
                    out.push(DisplayCommand::PopClip);
                    clipped = false;
                }
                if let Some(r) = *rect {
                    out.push(DisplayCommand::PushClip(intersect(box_rect(to_box, r), frame)));
                    clipped = true;
                }
            }
            other => paint_one(out, other, to_box),
        }
    }
    if clipped {
        out.push(DisplayCommand::PopClip);
    }
}

fn paint_one(out: &mut Vec<DisplayCommand>, command: &DrawCommand, to_box: &Transform2D) {
        match command {
            DrawCommand::Clip { .. } => {}
            DrawCommand::FillRect {
                x,
                y,
                w,
                h,
                style,
                transform,
            } => {
                let t = to_box.multiply(transform);
                let solid = solid(style);
                if let (Some(color), true) = (solid, t.b == 0.0 && t.c == 0.0) {
                    let (ax, ay) = t.apply(*x, *y);
                    let (bx, by) = t.apply(x + w, y + h);
                    let rect = Rect::new(ax.min(bx), ay.min(by), (bx - ax).abs(), (by - ay).abs());
                    if color.a > 0.0 {
                        out.push(DisplayCommand::FillRect { rect, color });
                    }
                } else {
                    let contour = vec![(*x, *y), (x + w, *y), (x + w, y + h), (*x, y + h)];
                    fill(out, &[contour], false, style, &t);
                }
            }
            DrawCommand::FillPath {
                segments,
                style,
                even_odd,
                transform,
            } => {
                fill(
                    out,
                    segments,
                    *even_odd,
                    style,
                    &to_box.multiply(transform),
                );
            }
            DrawCommand::StrokeRect {
                x,
                y,
                w,
                h,
                style,
                line_width,
                dash,
                transform,
            } => {
                let contour = vec![(*x, *y), (x + w, *y), (x + w, y + h), (*x, y + h), (*x, *y)];
                stroke(
                    out,
                    &[contour],
                    style,
                    *line_width,
                    dash,
                    &to_box.multiply(transform),
                );
            }
            DrawCommand::StrokePath {
                segments,
                style,
                line_width,
                dash,
                transform,
            } => {
                stroke(
                    out,
                    segments,
                    style,
                    *line_width,
                    dash,
                    &to_box.multiply(transform),
                );
            }
            DrawCommand::FillText {
                text,
                x,
                y,
                style,
                font,
                align,
                baseline,
                max_width,
                transform,
            }
            | DrawCommand::StrokeText {
                text,
                x,
                y,
                style,
                font,
                align,
                baseline,
                max_width,
                transform,
                ..
            } => {
                let font = CanvasFont::parse(font, 16.0).unwrap_or_else(initial_font);
                draw_text(
                    out,
                    text,
                    *x,
                    *y,
                    style,
                    &font,
                    *align,
                    *baseline,
                    *max_width,
                    &to_box.multiply(transform),
                );
            }
            // Clears are applied as the runs' clips (paint_commands);
            // images are not drawn in phase 1.
            DrawCommand::ClearRect { .. }
            | DrawCommand::DrawImage { .. }
            | DrawCommand::PutImageData { .. } => {}
        }
}

fn intersect(a: Rect, b: Rect) -> Rect {
    let x0 = a.x.max(b.x);
    let y0 = a.y.max(b.y);
    let x1 = (a.x + a.width).min(b.x + b.width);
    let y1 = (a.y + a.height).min(b.y + b.height);
    Rect::new(x0, y0, (x1 - x0).max(0.0), (y1 - y0).max(0.0))
}

fn solid(style: &CanvasStyle) -> Option<Color> {
    match style {
        CanvasStyle::Color(c) => Some(*c),
        _ => None,
    }
}

/// One colour standing for a style a command cannot paint as such: a
/// gradient's middle (strokes and text with a gradient).
fn representative(style: &CanvasStyle) -> Color {
    match style {
        CanvasStyle::Color(c) => *c,
        CanvasStyle::LinearGradient(g) => g.sample(0.5),
        CanvasStyle::RadialGradient(g) => g
            .stops
            .first()
            .map(|s| s.color)
            .unwrap_or(Color::TRANSPARENT),
        CanvasStyle::Pattern(_) => Color::TRANSPARENT,
    }
}

fn device(segments: &[Vec<(f32, f32)>], t: &Transform2D) -> Vec<Vec<(f32, f32)>> {
    segments
        .iter()
        .map(|s| s.iter().map(|&(x, y)| t.apply(x, y)).collect())
        .collect()
}

fn fill(
    out: &mut Vec<DisplayCommand>,
    segments: &[Vec<(f32, f32)>],
    even_odd: bool,
    style: &CanvasStyle,
    t: &Transform2D,
) {
    let contours = device(segments, t);
    match style {
        CanvasStyle::Color(c) => {
            if c.a > 0.0 {
                out.extend(rustkit_svg::fill_device_contours(&contours, even_odd, *c));
            }
        }
        CanvasStyle::LinearGradient(LinearGradient {
            x0,
            y0,
            x1,
            y1,
            stops,
        }) => {
            if stops.is_empty() {
                return;
            }
            let stops = stops.iter().map(|s| (s.offset, s.color)).collect();
            out.extend(rustkit_svg::fill_device_contours_linear(
                &contours,
                even_odd,
                t.apply(*x0, *y0),
                t.apply(*x1, *y1),
                stops,
            ));
        }
        other => {
            let c = representative(other);
            if c.a > 0.0 {
                out.extend(rustkit_svg::fill_device_contours(&contours, even_odd, c));
            }
        }
    }
}

fn scale_of(t: &Transform2D) -> f32 {
    (t.a * t.d - t.b * t.c).abs().sqrt()
}

fn stroke(
    out: &mut Vec<DisplayCommand>,
    segments: &[Vec<(f32, f32)>],
    style: &CanvasStyle,
    line_width: f32,
    dash: &LineDash,
    t: &Transform2D,
) {
    let color = representative(style);
    if color.a <= 0.0 {
        return;
    }
    let scale = scale_of(t);
    let width = line_width * scale;
    for segment in segments {
        let pieces = if dash.segments.iter().any(|d| *d > 0.0) {
            dashed(segment, &dash.segments, dash.offset)
        } else {
            vec![segment.clone()]
        };
        for piece in pieces {
            if piece.len() < 2 {
                continue;
            }
            let points: Vec<(f32, f32)> = piece.iter().map(|&(x, y)| t.apply(x, y)).collect();
            out.push(DisplayCommand::Polyline {
                points,
                color,
                width,
            });
        }
    }
}

/// Split a polyline (user space) into its dashes: `pattern` alternates
/// on and off lengths, starting `offset` into it.
fn dashed(points: &[(f32, f32)], pattern: &[f32], offset: f32) -> Vec<Vec<(f32, f32)>> {
    let total: f32 = pattern.iter().sum();
    if total <= 0.0 {
        return vec![points.to_vec()];
    }
    let mut index = 0;
    let mut left = pattern[0];
    let mut pos = offset.rem_euclid(total);
    while pos > 0.0 {
        if pos >= left {
            pos -= left;
            index = (index + 1) % pattern.len();
            left = pattern[index];
        } else {
            left -= pos;
            pos = 0.0;
        }
    }
    let mut out = Vec::new();
    let mut current: Vec<(f32, f32)> = Vec::new();
    if index % 2 == 0 {
        current.push(points[0]);
    }
    for pair in points.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let len = (b.0 - a.0).hypot(b.1 - a.1);
        let mut at = 0.0;
        while len - at > left {
            at += left;
            let p = (a.0 + (b.0 - a.0) * at / len, a.1 + (b.1 - a.1) * at / len);
            if index % 2 == 0 {
                current.push(p);
                out.push(std::mem::take(&mut current));
            } else {
                current.push(p);
            }
            index = (index + 1) % pattern.len();
            left = pattern[index];
        }
        left -= len - at;
        if index % 2 == 0 {
            current.push(b);
        }
    }
    if index % 2 == 0 && current.len() > 1 {
        out.push(current);
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn draw_text(
    out: &mut Vec<DisplayCommand>,
    text: &str,
    x: f32,
    y: f32,
    style: &CanvasStyle,
    font: &CanvasFont,
    align: TextAlign,
    baseline: TextBaseline,
    max_width: Option<f32>,
    t: &Transform2D,
) {
    let color = representative(style);
    if color.a <= 0.0 || text.is_empty() {
        return;
    }
    let metrics = measure(font, text);
    // maxWidth narrower than the run: the font shrinks to fit (the
    // condensing Chromium does is horizontal only).
    let mut size = font.size;
    let mut width = metrics.width;
    if let Some(max) = max_width {
        if width > max && width > 0.0 {
            size *= max / width;
            width = max;
        }
    }
    let k = if font.size > 0.0 {
        size / font.size
    } else {
        1.0
    };
    let (ascent, descent) = (metrics.ascent * k, metrics.descent * k);
    let dx = match align {
        TextAlign::Left | TextAlign::Start => 0.0,
        TextAlign::Center => -width / 2.0,
        TextAlign::Right | TextAlign::End => -width,
    };
    // The em box split in the font's ascent:descent ratio (HTML canvas
    // §4.12.5.1.13: top/bottom are the em square's edges).
    let em = ascent + descent;
    let (em_up, em_down) = if em > 0.0 {
        (size * ascent / em, size * descent / em)
    } else {
        (size * 0.8, size * 0.2)
    };
    let dy = match baseline {
        TextBaseline::Alphabetic => 0.0,
        TextBaseline::Top => em_up,
        TextBaseline::Hanging => em_up * 0.8,
        TextBaseline::Middle => (em_up - em_down) / 2.0,
        TextBaseline::Bottom | TextBaseline::Ideographic => -em_down,
    };
    // Rotation and skew are not carried by a text run: the run is placed
    // at the transformed baseline origin, scaled by the transform's scale.
    let (px, py) = t.apply(x + dx, y + dy);
    out.push(DisplayCommand::Text {
        text: text.to_string(),
        x: px,
        y: py,
        color,
        font_size: size * scale_of(t),
        font_family: font.family.clone(),
        font_weight: font.weight,
        font_style: if font.italic { 1 } else { 0 },
        advances: None,
        // y is the baseline (see rustkit-svg's text: a zero ascent hands
        // the renderer the baseline directly).
        ascent: Some(0.0),
        run: None,
        faces: None,
        undilated: false,
    });
}

#[cfg(test)]
mod unit_tests {
    use super::*;

    #[test]
    fn dashes_split_a_line_into_on_runs() {
        let runs = dashed(&[(0.0, 0.0), (10.0, 0.0)], &[2.0, 3.0], 0.0);
        assert_eq!(
            runs,
            vec![vec![(0.0, 0.0), (2.0, 0.0)], vec![(5.0, 0.0), (7.0, 0.0)]]
        );
    }

    #[test]
    fn bitmap_size_reads_attributes_as_html_does() {
        let s = |v: &str| Some(v.to_string());
        assert_eq!(bitmap_size(None, None), (300.0, 150.0));
        assert_eq!(
            bitmap_size(s("40").as_ref(), s(" 20px").as_ref()),
            (40.0, 20.0)
        );
        assert_eq!(
            bitmap_size(s("x").as_ref(), s("-5").as_ref()),
            (300.0, 150.0)
        );
    }
}
