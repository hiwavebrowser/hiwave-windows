//! What a scrolled frame costs, on synthetic display lists (the threads-P3
//! follow-up: a 20,000-command page took 426 ms per scrolled frame on the
//! Mac), and proof that the fast path keeps paint order.
//!
//! The timing table is an ignored test:
//! `cargo test --release -p rustkit-renderer --lib -- --ignored scroll_cost_table --nocapture`
//! (`SCROLL_COST_SIZES=1000,5000` picks the sizes). The GPU tests skip,
//! loudly, where no adapter exists.

use super::*;
use std::time::Instant;

const VIEW_W: u32 = 1024;
const VIEW_H: u32 = 768;
const ROW_H: f32 = 20.0;

struct Gpu {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    adapter: String,
    device_type: wgpu::DeviceType,
}

/// One device per test (the renderer's tests share no GPU state).
fn gpu() -> Option<Gpu> {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
        backends: wgpu::Backends::all(),
        ..Default::default()
    });
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        force_fallback_adapter: false,
    }))?;
    let info = adapter.get_info();
    let (name, device_type) = (info.name, info.device_type);
    let (device, queue) = pollster::block_on(adapter.request_device(
        &wgpu::DeviceDescriptor {
            label: Some("scroll cost test device"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default(),
            memory_hints: wgpu::MemoryHints::Performance,
        },
        None,
    ))
    .ok()?;
    Some(Gpu {
        device: Arc::new(device),
        queue: Arc::new(queue),
        adapter: name,
        device_type,
    })
}

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Bgra8Unorm;

fn renderer(gpu: &Gpu) -> Renderer {
    let mut r = Renderer::new(gpu.device.clone(), gpu.queue.clone(), FORMAT).expect("renderer");
    r.set_viewport_size(VIEW_W, VIEW_H);
    r
}

fn text(s: &str, x: f32, y: f32, color: Color) -> DisplayCommand {
    DisplayCommand::Text {
        text: s.to_string(),
        x,
        y,
        color,
        font_size: 14.0,
        font_family: "sans-serif".to_string(),
        font_weight: 400,
        font_style: 0,
        advances: Some(vec![7.0; s.chars().count()]),
        ascent: Some(13.0),
        run: None,
        faces: None,
    }
}

fn fill(x: f32, y: f32, w: f32, h: f32, color: Color) -> DisplayCommand {
    DisplayCommand::SolidColor(color, Rect::new(x, y, w, h))
}

fn image(url: &str, x: f32, y: f32, w: f32, h: f32) -> DisplayCommand {
    let r = Rect::new(x, y, w, h);
    DisplayCommand::Image {
        url: url.to_string(),
        src_rect: None,
        dest_rect: r,
        object_fit: ObjectFit::Fill,
        content_box: r,
        object_position: (0.5, 0.5),
        object_position_offset: (0.0, 0.0),
        opacity: 1.0,
        current_color: Color::new(0, 0, 0, 1.0),
    }
}

#[derive(Clone, Copy, Debug)]
#[repr(usize)]
enum Mix {
    /// Paragraph rows of text, an underline now and then.
    Text,
    /// Every row a small background (badge, table cell) and its text; one
    /// row in fifty has a box that lands on the row above's text.
    Backgrounds,
    /// `Backgrounds` with no box over earlier text: nothing ever flushes,
    /// so every fill is checked against every glyph batched so far.
    Table,
    /// Thumbnail, caption and a separator line per row.
    Images,
}

const LINE: &str = "The quick brown fox jumps over the lazy dog 0123";

/// A page of about `n` commands, rows `ROW_H` apart, a page background first.
fn page(mix: Mix, n: usize) -> Vec<DisplayCommand> {
    let ink = Color::new(20, 20, 20, 1.0);
    let mut list = vec![fill(
        0.0,
        0.0,
        VIEW_W as f32,
        0.0,
        Color::new(255, 255, 255, 1.0),
    )];
    let mut row = 0usize;
    while list.len() < n {
        let y = row as f32 * ROW_H;
        match mix {
            Mix::Text => {
                list.push(text(LINE, 16.0, y, ink));
                if row % 7 == 3 {
                    list.push(DisplayCommand::TextDecoration {
                        x: 16.0,
                        y: y + 16.0,
                        width: 120.0,
                        thickness: 1.0,
                        color: ink,
                        style: rustkit_layout::TextDecorationStyleValue::Solid,
                    });
                }
            }
            Mix::Backgrounds | Mix::Table => {
                let tint = Color::new(200 + (row % 5) as u8 * 10, 230, 240, 1.0);
                list.push(fill(10.0, y + 1.0, 80.0, ROW_H - 2.0, tint));
                list.push(text("cell", 14.0, y, ink));
                list.push(fill(
                    100.0,
                    y + 1.0,
                    500.0,
                    ROW_H - 2.0,
                    Color::new(245, 245, 250, 1.0),
                ));
                list.push(text(LINE, 104.0, y, ink));
                if matches!(mix, Mix::Backgrounds) && row % 50 == 49 {
                    // A positioned box over the row above's text.
                    list.push(fill(
                        300.0,
                        y - 12.0,
                        60.0,
                        16.0,
                        Color::new(250, 80, 80, 1.0),
                    ));
                }
            }
            Mix::Images => {
                list.push(image(
                    if row.is_multiple_of(2) {
                        "a.png"
                    } else {
                        "b.png"
                    },
                    10.0,
                    y + 2.0,
                    16.0,
                    16.0,
                ));
                list.push(text(LINE, 32.0, y, ink));
                list.push(fill(
                    0.0,
                    y + ROW_H - 1.0,
                    VIEW_W as f32,
                    1.0,
                    Color::new(220, 220, 220, 1.0),
                ));
            }
        }
        row += 1;
    }
    let height = row as f32 * ROW_H;
    list[0] = fill(
        0.0,
        0.0,
        VIEW_W as f32,
        height,
        Color::new(255, 255, 255, 1.0),
    );
    list
}

trait PushGet<T> {
    fn push_get(&mut self, item: T) -> &mut T;
}

impl<T> PushGet<T> for Vec<T> {
    fn push_get(&mut self, item: T) -> &mut T {
        self.push(item);
        self.last_mut().unwrap()
    }
}

fn page_height(list: &[DisplayCommand]) -> f32 {
    match list[0] {
        DisplayCommand::SolidColor(_, r) => r.height,
        _ => unreachable!(),
    }
}

fn upload_images(r: &mut Renderer) {
    let px: Vec<u8> = [30u8, 120, 200, 255].repeat(16 * 16);
    r.upload_image("a.png", 16, 16, &px).unwrap();
    r.upload_image("b.png", 16, 16, &px).unwrap();
}

/// The frame's pixels (BGRA).
fn draw(gpu: &Gpu, r: &mut Renderer, list: &[DisplayCommand], scroll: (f32, f32)) -> Vec<u8> {
    let (texture, view) = screenshot::create_offscreen_target(&gpu.device, VIEW_W, VIEW_H, FORMAT);
    r.set_scroll_offset(scroll);
    r.execute(list, &view).expect("execute");
    r.set_scroll_offset((0.0, 0.0));
    let readback = screenshot::GpuReadbackBuffer::new(&gpu.device, VIEW_W, VIEW_H);
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    readback.copy_from_texture(&mut encoder, &texture);
    gpu.queue.submit(std::iter::once(encoder.finish()));
    readback.read_data_sync(&gpu.device).expect("readback")
}

/// Milliseconds for one frame, CPU work plus waiting for the GPU; `NaN`
/// when the frame fails (a batch past wgpu's buffer limit).
fn time_frame(
    gpu: &Gpu,
    r: &mut Renderer,
    view: &wgpu::TextureView,
    list: &[DisplayCommand],
    scroll: (f32, f32),
) -> f64 {
    r.set_scroll_offset(scroll);
    gpu.device.push_error_scope(wgpu::ErrorFilter::Validation);
    let t = Instant::now();
    let drawn = r.execute(list, view);
    gpu.device.poll(wgpu::Maintain::Wait);
    let ms = t.elapsed().as_secs_f64() * 1e3;
    let invalid = pollster::block_on(gpu.device.pop_error_scope());
    r.set_scroll_offset((0.0, 0.0));
    if drawn.is_err() || invalid.is_some() {
        f64::NAN
    } else {
        ms
    }
}

fn median(mut v: Vec<f64>) -> f64 {
    if v.iter().any(|ms| ms.is_nan()) {
        return f64::NAN;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[v.len() / 2]
}

/// The commands of `list` with a row inside the viewport at `scroll_y`:
/// the work a frame cannot avoid.
fn visible_only(list: &[DisplayCommand], scroll_y: f32) -> Vec<DisplayCommand> {
    let top = |c: &DisplayCommand| match c {
        DisplayCommand::SolidColor(_, r) => (r.y, r.y + r.height),
        DisplayCommand::Text { y, .. } => (*y, *y + ROW_H),
        DisplayCommand::TextDecoration { y, thickness, .. } => (*y, *y + thickness),
        DisplayCommand::Image { dest_rect, .. } => (dest_rect.y, dest_rect.y + dest_rect.height),
        _ => (f32::MIN, f32::MAX),
    };
    list.iter()
        .filter(|c| {
            let (y0, y1) = top(c);
            y1 > scroll_y && y0 < scroll_y + VIEW_H as f32
        })
        .cloned()
        .collect()
}

#[test]
#[ignore = "timing table; run with --release --ignored --nocapture"]
fn scroll_cost_table() {
    let Some(gpu) = gpu() else {
        eprintln!("SKIP scroll_cost_table: no GPU adapter");
        return;
    };
    let sizes: Vec<usize> = std::env::var("SCROLL_COST_SIZES")
        .ok()
        .map(|s| s.split(',').filter_map(|n| n.trim().parse().ok()).collect())
        .unwrap_or_else(|| vec![1_000, 5_000, 20_000, 80_000]);
    let reps: usize = std::env::var("SCROLL_COST_REPS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(5);
    println!("adapter: {}", gpu.adapter);
    println!("| mix | path | commands | frame ms (scroll 0) | scrolled frame ms | visible-only ms | overlap checks | overlap comparisons | overlap ms | overlap flushes | batch flushes | submits | quads | quads off-screen | culled |");
    println!("|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|");
    let mixes = std::env::var("SCROLL_COST_MIXES").unwrap_or_default();
    // `SCROLL_COST_PATHS=after` skips the slow reference path.
    let paths = std::env::var("SCROLL_COST_PATHS").unwrap_or_else(|_| "before,after".into());
    for mix in [Mix::Text, Mix::Backgrounds, Mix::Table, Mix::Images] {
        if !mixes.is_empty() && !mixes.contains(&format!("{mix:?}")) {
            continue;
        }
        for &n in &sizes {
            let list = page(mix, n);
            for (path, reference) in [("before", true), ("after", false)] {
                if !paths.contains(path) {
                    continue;
                }
                let mut r = renderer(&gpu);
                r.reference = reference;
                upload_images(&mut r);
                let (_t, view) =
                    screenshot::create_offscreen_target(&gpu.device, VIEW_W, VIEW_H, FORMAT);
                let mid = ((page_height(&list) - VIEW_H as f32) / 2.0)
                    .max(0.0)
                    .round();
                // Warm the glyph atlas.
                time_frame(&gpu, &mut r, &view, &list, (0.0, 0.0));
                let at0 = median(
                    (0..reps)
                        .map(|_| time_frame(&gpu, &mut r, &view, &list, (0.0, 0.0)))
                        .collect(),
                );
                // A wheel scroll re-presents the same list at a new offset.
                let scrolled = median(
                    (0..reps)
                        .map(|i| {
                            time_frame(&gpu, &mut r, &view, &list, (0.0, mid + (i as f32) * 40.0))
                        })
                        .collect(),
                );
                let visible = visible_only(&list, mid);
                time_frame(&gpu, &mut r, &view, &visible, (0.0, mid));
                let vis = median(
                    (0..reps)
                        .map(|_| time_frame(&gpu, &mut r, &view, &visible, (0.0, mid)))
                        .collect(),
                );
                r.set_measure(true);
                time_frame(&gpu, &mut r, &view, &list, (0.0, mid));
                let s = r.last_frame_stats();
                println!(
                    "| {mix:?} | {path} | {} | {at0:.1} | {scrolled:.1} | {vis:.1} | {} | {} | {:.1} | {} | {} | {} | {} | {} | {} |",
                    list.len(),
                    s.overlap_checks,
                    s.overlap_comparisons,
                    s.overlap_ns as f64 / 1e6,
                    s.overlap_flushes,
                    s.batch_flushes,
                    s.submits,
                    s.quads,
                    s.quads_offscreen,
                    s.culled,
                );
            }
        }
    }
}

/// A page that exercises what culling must get right: a fixed header, an
/// `overflow: hidden` box whose content is mostly clipped, a transform that
/// pulls far-down content into view, an opacity group, a rotated box, a
/// positioned box over earlier text, and `n` rows of mixed content.
fn kitchen_sink(n: usize) -> Vec<DisplayCommand> {
    let ink = Color::new(20, 20, 20, 1.0);
    let mut list = page(Mix::Backgrounds, n);
    let height = page_height(&list);
    list.extend([
        // Content 3000px down, translated up into the first screen.
        DisplayCommand::PushTransform {
            matrix: [1.0, 0.0, 0.0, 1.0, 0.0, -3000.0],
            origin: (0.0, 0.0),
        },
        fill(620.0, 3100.0, 200.0, 40.0, Color::new(40, 160, 90, 1.0)),
        text(LINE, 624.0, 3110.0, Color::new(255, 255, 255, 1.0)),
        DisplayCommand::PopTransform,
        // A rotated card: its bounds are tested on the mapped corners.
        DisplayCommand::PushTransform {
            matrix: [0.0, 1.0, -1.0, 0.0, 0.0, 0.0],
            origin: (700.0, 400.0),
        },
        fill(650.0, 380.0, 120.0, 40.0, Color::new(90, 60, 200, 1.0)),
        text("rotated", 660.0, 390.0, Color::new(255, 255, 255, 1.0)),
        DisplayCommand::PopTransform,
        // A clipped scroller: 200 rows inside a 300px box.
        DisplayCommand::PushClip(Rect::new(650.0, 500.0, 300.0, 120.0)),
    ]);
    for i in 0..200 {
        let y = 500.0 + i as f32 * ROW_H;
        list.push(fill(
            650.0,
            y,
            300.0,
            ROW_H - 1.0,
            Color::new(255, 240 - (i % 4) as u8 * 20, 200, 1.0),
        ));
        list.push(text(LINE, 654.0, y, ink));
    }
    list.push(DisplayCommand::PopClip);
    // An opacity group across the page.
    list.push(DisplayCommand::PushOpacity(0.5));
    for i in 0..60 {
        let y = i as f32 * (height / 60.0);
        list.push(fill(800.0, y, 150.0, 30.0, Color::new(0, 120, 255, 1.0)));
        list.push(text("faded", 804.0, y + 4.0, ink));
    }
    list.push(DisplayCommand::PopOpacity);
    // Every 400px: an image, a caption on it, a badge over both, then an
    // image over earlier text; and text right of the viewport whose
    // negative letter-spacing walks it back on screen.
    let mut y = 60.0;
    while y < height {
        list.push(image("a.png", 300.0, y, 120.0, 80.0));
        list.push(text(
            "caption",
            304.0,
            y + 50.0,
            Color::new(255, 255, 255, 1.0),
        ));
        list.push(fill(
            390.0,
            y - 6.0,
            40.0,
            24.0,
            Color::new(220, 0, 60, 1.0),
        ));
        list.push(text("seen through", 440.0, y + 20.0, ink));
        list.push(image("b.png", 470.0, y + 18.0, 30.0, 30.0));
        if let DisplayCommand::Text {
            advances: Some(advances),
            ..
        } = list.push_get(text(LINE, 1400.0, y + 100.0, ink))
        {
            advances.iter_mut().for_each(|a| *a = -14.0);
        }
        y += 400.0;
    }
    // A fixed header, drawn last so it is on top.
    list.extend([
        DisplayCommand::PushViewportFixed,
        fill(0.0, 0.0, VIEW_W as f32, 40.0, Color::new(30, 30, 40, 1.0)),
        text("Fixed header", 16.0, 12.0, Color::new(255, 255, 255, 1.0)),
        fill(
            0.0,
            VIEW_H as f32 - 24.0,
            VIEW_W as f32,
            24.0,
            Color::new(240, 200, 0, 1.0),
        ),
        DisplayCommand::PopViewportFixed,
    ]);
    list
}

fn reference_draw(gpu: &Gpu, list: &[DisplayCommand], scroll: (f32, f32)) -> (Vec<u8>, FrameStats) {
    let mut r = renderer(gpu);
    upload_images(&mut r);
    r.reference = true;
    let px = draw(gpu, &mut r, list, scroll);
    (px, r.last_frame_stats())
}

fn fast_draw(gpu: &Gpu, list: &[DisplayCommand], scroll: (f32, f32)) -> (Vec<u8>, FrameStats) {
    let mut r = renderer(gpu);
    upload_images(&mut r);
    let px = draw(gpu, &mut r, list, scroll);
    (px, r.last_frame_stats())
}

/// The list without its `Text` commands: drawn on the reference path, it
/// tells which pixels of the reference frame are glyph pixels.
fn without_text(list: &[DisplayCommand]) -> Vec<DisplayCommand> {
    list.iter()
        .filter(|c| !matches!(c, DisplayCommand::Text { .. }))
        .cloned()
        .collect()
}

/// Checks a culled frame against the unculled reference.
///
/// Culling must never change which pixels are covered or the order things
/// are painted in: every pixel outside text must match byte for byte. Text
/// may differ by at most 1 LSB per channel, on glyph pixels only. That is
/// the lead's ruling on #710: culled text skips the glyph cache, so the
/// visible glyphs are packed into different atlas slots, and linear sampling
/// at fractional positions can round 1 LSB differently. Atlas packing
/// already varies with page history on develop. (On lavapipe, clipped-away
/// glyph quads sharing a draw can also move visible glyph pixels by 1 LSB.)
///
/// `no_text` is the reference frame drawn from `without_text`. A pixel where
/// it differs from `want` is a glyph pixel. A missing, extra or misordered
/// paint shows as a difference outside those pixels, or as more than 1 LSB
/// inside them, and fails.
fn assert_culled_matches(want: &[u8], got: &[u8], no_text: &[u8], what: &str) {
    if let Err(e) = check_culled(want, got, no_text) {
        panic!("{what}: {e}");
    }
}

/// `assert_culled_matches` without the panic: the reason a frame fails.
fn check_culled(want: &[u8], got: &[u8], no_text: &[u8]) -> Result<(), String> {
    let mut wrong = 0usize;
    let mut first = None;
    let mut tolerated = 0usize;
    for (i, ((w, g), n)) in want
        .chunks_exact(4)
        .zip(got.chunks_exact(4))
        .zip(no_text.chunks_exact(4))
        .enumerate()
    {
        if w == g {
            continue;
        }
        let glyph = w != n;
        let lsb = w.iter().zip(g).all(|(a, b)| a.abs_diff(*b) <= 1);
        if glyph && lsb {
            tolerated += 1;
        } else {
            wrong += 1;
            first.get_or_insert((
                i % VIEW_W as usize,
                i / VIEW_W as usize,
                w.to_vec(),
                g.to_vec(),
                glyph,
            ));
        }
    }
    if wrong == 0 {
        return Ok(());
    }
    Err(format!(
        "{wrong} pixels differ beyond the glyph tolerance ({tolerated} glyph pixels within 1 LSB); \
         first (x, y, reference, culled, glyph pixel): {first:?}"
    ))
}

#[test]
fn culled_frames_match_unculled_ones_at_every_scroll() {
    let Some(gpu) = gpu() else {
        eprintln!("SKIP: no GPU adapter");
        return;
    };
    let mut lists: Vec<(&str, Vec<DisplayCommand>)> = vec![("kitchen sink", kitchen_sink(3_000))];
    for mix in [Mix::Text, Mix::Backgrounds, Mix::Table, Mix::Images] {
        lists.push((
            ["text", "backgrounds", "table", "images"][mix as usize],
            page(mix, 2_000),
        ));
    }
    for (name, list) in &lists {
        let end = page_height(list) - VIEW_H as f32;
        for scroll in [
            (0.0, 0.0),
            (0.0, 777.0),
            (0.0, (end / 2.0).round()),
            (0.0, end),
            (13.0, 500.5),
            (0.0, -50.0),
        ] {
            let (want, want_stats) = reference_draw(&gpu, list, scroll);
            let (got, got_stats) = fast_draw(&gpu, list, scroll);
            let (no_text, _) = reference_draw(&gpu, &without_text(list), scroll);
            assert_culled_matches(&want, &got, &no_text, &format!("{name} at {scroll:?}"));
            assert_eq!(want_stats.culled, 0);
            assert!(
                got_stats.culled > 0 || list.len() < 100,
                "{name} at {scroll:?}: nothing culled"
            );
            // The grid finds the same overlaps the scan found wherever
            // nothing was culled away; culling only drops flushes caused
            // by off-screen geometry.
            assert!(got_stats.overlap_flushes <= want_stats.overlap_flushes);
        }
    }
}

#[test]
fn paint_order_is_kept_for_interleaved_fills_and_text() {
    let Some(gpu) = gpu() else {
        eprintln!("SKIP: no GPU adapter");
        return;
    };
    // Rows of: a background, text on it, then a box over that text (a
    // positioned box painting later), then text on the box. Every box
    // over text must flush, and the pixels must be the scan's.
    let ink = Color::new(0, 0, 0, 1.0);
    let red = Color::new(255, 0, 0, 1.0);
    let mut list = vec![fill(
        0.0,
        0.0,
        VIEW_W as f32,
        VIEW_H as f32,
        Color::new(255, 255, 255, 1.0),
    )];
    let rows = 30;
    for i in 0..rows {
        let y = 10.0 + i as f32 * 24.0;
        list.push(fill(10.0, y, 600.0, 22.0, Color::new(230, 230, 255, 1.0)));
        list.push(text(
            "MMMMMMMMMMMMMMMMMMMMMMMMMMMMMMMMMMMMMMMMMMMM",
            14.0,
            y + 2.0,
            ink,
        ));
        list.push(fill(100.0, y, 60.0, 22.0, red));
        list.push(text("ok", 104.0, y + 2.0, Color::new(255, 255, 255, 1.0)));
    }
    let (want, want_stats) = reference_draw(&gpu, &list, (0.0, 0.0));
    let (got, got_stats) = fast_draw(&gpu, &list, (0.0, 0.0));
    assert_eq!(want, got, "the grid changed the pixels");
    assert_eq!(got_stats.overlap_flushes, want_stats.overlap_flushes);
    assert!(
        got_stats.overlap_flushes >= rows as u64,
        "every red box lands on text: {got_stats:?}"
    );
    assert!(
        got_stats.overlap_comparisons < want_stats.overlap_comparisons
            || want_stats.overlap_comparisons < 100
    );
    // The red box is on top of the black text under it: inside the box,
    // away from its own white text, no pixel is black. BGRA.
    for i in 0..rows {
        let y = (10.0 + i as f32 * 24.0) as usize + 10;
        for x in 140..158 {
            let p = &got[(y * VIEW_W as usize + x) * 4..][..4];
            assert_eq!(
                p,
                [0, 0, 255, 255],
                "row {i} pixel ({x}, {y}) is {p:?}, not the red box"
            );
        }
    }
}

#[test]
fn a_badge_stays_over_its_image_and_an_image_over_its_text_whatever_is_off_screen() {
    let Some(gpu) = gpu() else {
        eprintln!("SKIP: no GPU adapter");
        return;
    };
    let ink = Color::new(0, 0, 0, 1.0);
    let badge = Color::new(255, 0, 0, 1.0);
    let list = vec![
        fill(
            0.0,
            0.0,
            VIEW_W as f32,
            4000.0,
            Color::new(255, 255, 255, 1.0),
        ),
        image("a.png", 100.0, 100.0, 200.0, 100.0),
        // Off screen at scroll 0: a fill over text, which flushed the batch
        // before, image and all, and so put the badge below on top.
        text(LINE, 100.0, 3000.0, ink),
        fill(100.0, 3000.0, 300.0, 20.0, Color::new(0, 200, 0, 1.0)),
        fill(150.0, 120.0, 60.0, 40.0, badge),
        // An image over earlier text.
        text("MMMMMMMMMMMMMMMMMMMM", 100.0, 300.0, ink),
        image("b.png", 120.0, 296.0, 60.0, 24.0),
    ];
    for scroll in [(0.0, 0.0), (0.0, 2600.0)] {
        let (want, _) = reference_draw(&gpu, &list, scroll);
        let (got, stats) = fast_draw(&gpu, &list, scroll);
        let (no_text, _) = reference_draw(&gpu, &without_text(&list), scroll);
        assert_culled_matches(&want, &got, &no_text, &format!("at {scroll:?}"));
        if scroll.1 == 0.0 {
            assert!(stats.culled >= 2, "{stats:?}");
            let px = |x: usize, y: usize| got[(y * VIEW_W as usize + x) * 4..][..4].to_vec();
            // BGRA: the badge over the image, the image over the text.
            assert_eq!(px(180, 140), vec![0, 0, 255, 255], "badge under its image");
            assert_eq!(
                px(150, 308),
                vec![200, 120, 30, 255],
                "image under earlier text"
            );
        }
    }
}

/// The buffer-limit frame for an adapter: how many commands of text page,
/// and the buffer and batch limits to set (`None` keeps the renderer's own:
/// the device's buffer limit, at most 256 MB, and 64 MB batches).
#[derive(Debug, PartialEq)]
struct BufferLimitCase {
    commands: usize,
    max_buffer_bytes: Option<u64>,
    max_batch_bytes: Option<u64>,
}

/// Real GPUs draw the 80,000-command frame past the device's 256 MB limit. A
/// CPU adapter (WARP, llvmpipe, SwiftShader) cannot: its queue times out on
/// 3.4M glyph quads and panics in `Queue::drop`, which aborts the whole test
/// binary. It draws 2,000 commands past a 64 KiB buffer limit and a 16 KiB
/// batch limit instead (the same 4:1 ratio), so the flush and split paths
/// still run there.
fn buffer_limit_case(device_type: wgpu::DeviceType) -> BufferLimitCase {
    match device_type {
        wgpu::DeviceType::Cpu => BufferLimitCase {
            commands: 2_000,
            max_buffer_bytes: Some(64 * 1024),
            max_batch_bytes: Some(16 * 1024),
        },
        _ => BufferLimitCase {
            commands: 80_000,
            max_buffer_bytes: None,
            max_batch_bytes: None,
        },
    }
}

#[test]
fn a_software_adapter_draws_a_small_frame_past_a_small_limit() {
    let full = BufferLimitCase {
        commands: 80_000,
        max_buffer_bytes: None,
        max_batch_bytes: None,
    };
    for hw in [
        wgpu::DeviceType::DiscreteGpu,
        wgpu::DeviceType::IntegratedGpu,
        wgpu::DeviceType::VirtualGpu,
        wgpu::DeviceType::Other,
    ] {
        assert_eq!(buffer_limit_case(hw), full, "{hw:?}");
    }
    // On a CPU adapter (WARP, llvmpipe, SwiftShader) 3.4M glyph quads time
    // out the queue and panic in `Queue::drop`, aborting the test binary.
    let soft = buffer_limit_case(wgpu::DeviceType::Cpu);
    let cap = soft
        .max_buffer_bytes
        .expect("a small limit on a CPU adapter");
    let batch = soft
        .max_batch_bytes
        .expect("a small batch on a CPU adapter");
    assert!(batch < cap, "{soft:?}");
    assert!(soft.commands <= 2_000, "{soft:?}");
    // Still past the limit: the frame's glyph vertices need several buffers.
    let glyphs: usize = page(Mix::Text, soft.commands)
        .iter()
        .map(|c| match c {
            DisplayCommand::Text { text, .. } => text.chars().count(),
            _ => 0,
        })
        .sum();
    let glyph_bytes = glyphs * 4 * std::mem::size_of::<TextureVertex>();
    assert!(glyph_bytes as u64 > 4 * cap, "{glyph_bytes} vs {cap}");
    assert!(glyph_bytes as u64 > 2 * batch, "{glyph_bytes} vs {batch}");
}

#[test]
fn a_batch_past_the_buffer_limit_is_flushed_instead_of_panicking() {
    let Some(gpu) = gpu() else {
        eprintln!("SKIP: no GPU adapter");
        return;
    };
    // 80,000 rows of text drawn without culling: about 3.4M glyph quads,
    // past wgpu's 256 MB buffer limit in one batch. This panicked in
    // `flush_to` (Texture Vertex Buffer). A CPU adapter draws a small frame
    // past a small limit (`buffer_limit_case`).
    let case = buffer_limit_case(gpu.device_type);
    if case.max_buffer_bytes.is_some() {
        eprintln!(
            "software adapter {} ({:?}): {case:?}",
            gpu.adapter, gpu.device_type
        );
    }
    let list = page(Mix::Text, case.commands);
    let mut r = renderer(&gpu);
    r.reference = true;
    if let Some(cap) = case.max_buffer_bytes {
        r.max_buffer_bytes = cap;
    }
    if let Some(batch) = case.max_batch_bytes {
        r.max_batch_bytes = batch;
    }
    let (_t, view) = screenshot::create_offscreen_target(&gpu.device, VIEW_W, VIEW_H, FORMAT);
    r.execute(&list, &view).expect("the frame draws");
    let s = r.last_frame_stats();
    assert!(s.batch_flushes >= 2, "{s:?}");
}

fn count_ink(px: &[u8]) -> usize {
    px.chunks_exact(4)
        .filter(|p| p[..3] != [255, 255, 255])
        .count()
}

#[test]
fn a_batch_past_the_buffer_limit_is_drawn_in_pieces() {
    let Some(gpu) = gpu() else {
        eprintln!("SKIP: no GPU adapter");
        return;
    };
    // Fills, then images (two textures, alternating, so the runs split
    // too), then a screenful of text: no command lands under one batched
    // before it, so the frame is one batch. With a 64 KiB buffer limit
    // each part of it is many times over.
    let mut list = vec![fill(
        0.0,
        0.0,
        1024.0,
        768.0,
        Color::new(255, 255, 255, 1.0),
    )];
    for i in 0..36 {
        let y = i as f32 * 21.0;
        list.push(fill(0.0, y, 1024.0, 1.0, Color::new(200, 200, 200, 1.0)));
        for j in 0..40 {
            list.push(fill(
                j as f32 * 25.0,
                y + 1.0,
                3.0,
                3.0,
                Color::new(0, 0, 255, 1.0),
            ));
        }
    }
    for i in 0..36 {
        let y = i as f32 * 21.0 + 4.0;
        list.push(image(["a.png", "b.png"][i % 2], 0.0, y, 16.0, 16.0));
        list.push(image(["b.png", "a.png"][i % 2], 1000.0, y, 16.0, 16.0));
    }
    for i in 0..36 {
        list.push(text(
            &LINE.repeat(2),
            20.0,
            i as f32 * 21.0 + 4.0,
            Color::new(0, 0, 0, 1.0),
        ));
    }
    let (want, _) = fast_draw(&gpu, &list, (0.0, 0.0));
    let mut r = renderer(&gpu);
    upload_images(&mut r);
    r.max_buffer_bytes = 64 * 1024;
    let got = draw(&gpu, &mut r, &list, (0.0, 0.0));
    let s = r.last_frame_stats();
    assert_eq!(s.batch_flushes, 1, "one batch, split only at draw: {s:?}");
    let tex = std::mem::size_of::<TextureVertex>();
    assert!(
        r.texture_vertices.len() * tex > 4 * 64 * 1024,
        "glyph batch over the limit"
    );
    assert!(r.color_vertices.len() * std::mem::size_of::<ColorVertex>() > 2 * 64 * 1024);
    assert!(count_ink(&want) > 10_000);
    let differ = want
        .chunks_exact(4)
        .zip(got.chunks_exact(4))
        .filter(|(a, b)| a != b)
        .count();
    assert_eq!(differ, 0, "{differ} pixels differ from the one-draw frame");

    // Images alone, more quads than one piece holds, to split inside runs.
    let mut r = renderer(&gpu);
    upload_images(&mut r);
    r.max_buffer_bytes = 4 * 1024;
    let tiles: Vec<_> = (0..1200)
        .map(|i| {
            let (x, y) = ((i % 40) as f32 * 25.0, (i / 40) as f32 * 25.0);
            image(["a.png", "b.png"][(i / 300) % 2], x, y, 20.0, 20.0)
        })
        .collect();
    let got = draw(&gpu, &mut r, &tiles, (0.0, 0.0));
    let (want, _) = fast_draw(&gpu, &tiles, (0.0, 0.0));
    assert!(r.image_vertices.len() * tex > 8 * 4 * 1024);
    assert_eq!(want, got);
}

#[test]
fn a_line_of_millions_of_characters_is_drawn() {
    let Some(gpu) = gpu() else {
        eprintln!("SKIP: no GPU adapter");
        return;
    };
    // One on-screen `Text` of 2.3 M characters (a minified bundle shown as
    // text/plain): one command, 2.3 M glyph quads, 294 MB of vertices,
    // past the 256 MB limit before any flush between commands can help.
    // It panicked in `flush_to`, then failed the frame with
    // `BufferTooLarge`; the draw now splits it.
    let line = "x".repeat(2_300_000);
    let list = vec![
        fill(0.0, 0.0, 1024.0, 768.0, Color::new(255, 255, 255, 1.0)),
        text(&line, 16.0, 100.0, Color::new(0, 0, 0, 1.0)),
    ];
    let mut r = renderer(&gpu);
    let px = draw(&gpu, &mut r, &list, (0.0, 0.0));
    assert!(
        (r.texture_vertices.len() * std::mem::size_of::<TextureVertex>()) as u64 > MAX_BUFFER_SIZE,
        "the batch is over the limit"
    );
    assert!(count_ink(&px) > 1_000, "the start of the line is drawn");
}

fn deco(x: f32, y: f32, width: f32, thickness: f32, color: Color) -> DisplayCommand {
    DisplayCommand::TextDecoration {
        x,
        y,
        width,
        thickness,
        color,
        style: rustkit_layout::TextDecorationStyleValue::Solid,
    }
}

fn spaced(s: &str, x: f32, y: f32, advance: f32, color: Color) -> DisplayCommand {
    let mut t = text(s, x, y, color);
    if let DisplayCommand::Text {
        advances: Some(a), ..
    } = &mut t
    {
        a.iter_mut().for_each(|a| *a = advance);
    }
    t
}

/// Lists whose culled frame used to differ from the unculled one: a
/// decoration's order against an earlier image hung on an unrelated
/// flush below the fold (R1 HOLD @ 9891715b, A); negative widths and a
/// border side wider than its box were culled while drawn (B and the
/// border note). Each is repeated down the page, so every scroll offset
/// culls a different part of it.
fn cull_hazards() -> Vec<DisplayCommand> {
    let ink = Color::new(0, 0, 0, 1.0);
    let red = Color::new(255, 0, 0, 1.0);
    let blue = Color::new(0, 0, 255, 1.0);
    let mut list = vec![fill(
        0.0,
        0.0,
        VIEW_W as f32,
        6000.0,
        Color::new(255, 255, 255, 1.0),
    )];
    let mut y = 0.0;
    while y < 5400.0 {
        list.extend([
            // A: an underlined link over an image, with a fill over text
            // further down in between: that fill's flush used to be what
            // put the underline over the image.
            image("a.png", 100.0, y + 100.0, 300.0, 600.0),
            text(LINE, 100.0, y + 900.0, ink),
            fill(100.0, y + 900.0, 300.0, 20.0, Color::new(0, 200, 0, 1.0)),
            text("Read more", 120.0, y + 420.0, ink),
            deco(120.0, y + 436.0, 63.0, 3.0, red),
            deco(120.0, y + 426.0, 63.0, 1.0, blue),
            // A decoration over an earlier text run, not its own.
            text("MMMMMMMMMMMMMMMMMMMMMMMM", 450.0, y + 200.0, ink),
            text("x", 450.0, y + 230.0, ink),
            deco(440.0, y + 205.0, 200.0, 4.0, red),
            // An orphan decoration (no text before it) over an image.
            image("b.png", 650.0, y + 300.0, 100.0, 100.0),
            deco(640.0, y + 340.0, 140.0, 6.0, blue),
            // B: text right of the viewport walking back on screen, its
            // underline drawn leftwards; a fill with a negative width.
            spaced("ABCDEFGHIJKLMNOPQRST", 1100.0, y + 160.0, -15.0, ink),
            deco(1100.0, y + 174.0, -300.0, 2.0, red),
            fill(1300.0, y + 500.0, -500.0, 40.0, Color::new(0, 120, 0, 1.0)),
            fill(500.0, y + 640.0, 120.0, -60.0, Color::new(120, 0, 120, 1.0)),
            // A border whose top side is wider than its box and reaches
            // 380px below it; a rounded one the same.
            DisplayCommand::Border {
                color: Color::new(200, 100, 0, 1.0),
                rect: Rect::new(100.0, y - 200.0, 300.0, 20.0),
                top: 400.0,
                right: 0.0,
                bottom: 0.0,
                left: 0.0,
            },
            DisplayCommand::RoundedBorder {
                rect: Rect::new(700.0, y - 300.0, 200.0, 30.0),
                widths: [420.0, 2.0, 2.0, 2.0],
                colors: [Color::new(0, 150, 150, 1.0); 4],
                radius: rustkit_layout::BorderRadius::default(),
            },
            // Glyphs and an image after all of it.
            text("on top", 300.0, y + 430.0, Color::new(255, 255, 255, 1.0)),
            image("b.png", 140.0, y + 560.0, 40.0, 40.0),
        ]);
        y += 1200.0;
    }
    list
}

fn draw_with(
    gpu: &Gpu,
    list: &[DisplayCommand],
    scroll: (f32, f32),
    reference: bool,
    batch_bytes: u64,
) -> (Vec<u8>, FrameStats) {
    let mut r = renderer(gpu);
    let px = |rgb: [u8; 3]| [rgb[2], rgb[1], rgb[0], 255].repeat(16 * 16);
    r.upload_image("a.png", 16, 16, &px([200, 120, 30]))
        .unwrap();
    r.upload_image("b.png", 16, 16, &px([30, 160, 220]))
        .unwrap();
    r.reference = reference;
    r.max_batch_bytes = batch_bytes;
    let frame = draw(gpu, &mut r, list, scroll);
    (frame, r.last_frame_stats())
}

#[test]
fn culling_never_changes_coverage_or_order_of_decorations_negative_sizes_or_wide_borders() {
    let Some(gpu) = gpu() else {
        eprintln!("SKIP: no GPU adapter");
        return;
    };
    let hazards = cull_hazards();
    // The same list with a backdrop blur, drawn into a frame texture that
    // the blur reads.
    let mut blurred = hazards.clone();
    blurred.push(DisplayCommand::BackdropFilter {
        rect: Rect::new(900.0, 700.0, 100.0, 60.0),
        border_radius: rustkit_layout::BorderRadius::default(),
        steps: vec![rustkit_layout::FilterStep::Blur(4.0)],
        own_groups: 0,
    });
    let cases: [(&str, &[DisplayCommand], u64); 4] = [
        ("fast", &hazards, MAX_BATCH_BYTES),
        // A cap of 2 KiB flushes every few commands, at places that move
        // with culling.
        ("fast, small cap", &hazards, 2 * 1024),
        ("blur", &blurred, MAX_BATCH_BYTES),
        ("blur, small cap", &blurred, 2 * 1024),
    ];
    for (name, list, cap) in cases {
        for scroll in [
            (0.0, 0.0),
            (0.0, 150.0),
            (0.0, 300.0),
            (0.0, 1200.0),
            (7.0, 2111.5),
            (0.0, -40.0),
            (0.0, 4500.0),
        ] {
            let (want, _) = draw_with(&gpu, list, scroll, true, cap);
            let (got, stats) = draw_with(&gpu, list, scroll, false, cap);
            let (no_text, _) = draw_with(&gpu, &without_text(list), scroll, true, cap);
            assert_culled_matches(&want, &got, &no_text, &format!("{name} at {scroll:?}"));
            assert!(stats.culled > 0, "{name} at {scroll:?}: nothing culled");
        }
    }
    // What the reference draws is what CSS order says, at every scroll: the
    // underline over the image under it (BGRA red), the decoration over the
    // earlier run, the leftward underline and fill, and the wide border.
    let px = |f: &[u8], x: usize, y: usize| f[(y * VIEW_W as usize + x) * 4..][..4].to_vec();
    let red = vec![0, 0, 255, 255];
    for scroll_y in [0.0f32, 150.0] {
        let (got, _) = draw_with(&gpu, &hazards, (0.0, scroll_y), false, MAX_BATCH_BYTES);
        let at = |x: usize, y: f32| px(&got, x, (y - scroll_y) as usize);
        assert_eq!(at(170, 437.0), red, "underline at scroll {scroll_y}");
        assert_eq!(at(600, 206.0), red, "decoration over earlier text");
        if scroll_y > 0.0 {
            // At scroll 0 no transform is set and the negative sizes draw
            // nothing (`push_color_quad`), on both paths.
            assert_eq!(at(900, 175.0), red, "leftward underline");
            assert_eq!(at(900, 520.0), vec![0, 120, 0, 255], "leftward fill");
        }
    }
    let (got, _) = draw_with(&gpu, &hazards, (0.0, 0.0), false, MAX_BATCH_BYTES);
    assert_eq!(
        px(&got, 200, 150),
        vec![0, 100, 200, 255],
        "wide top border"
    );
}

fn underline(x: f32, y: f32, width: f32, color: Color) -> DisplayCommand {
    DisplayCommand::TextDecoration {
        x,
        y,
        width,
        thickness: 3.0,
        color,
        style: rustkit_layout::TextDecorationStyleValue::Solid,
    }
}

#[test]
fn an_underline_over_an_earlier_image_and_negative_widths_draw_the_same_at_any_scroll() {
    let Some(gpu) = gpu() else {
        eprintln!("SKIP: no GPU adapter");
        return;
    };
    let ink = Color::new(0, 0, 0, 1.0);
    let list = vec![
        fill(
            0.0,
            0.0,
            VIEW_W as f32,
            4000.0,
            Color::new(255, 255, 255, 1.0),
        ),
        image("a.png", 100.0, 100.0, 200.0, 100.0),
        // Off screen at scroll 0: a fill over text, which flushed the batch
        // (image included) before the underlined text below.
        text(LINE, 100.0, 3000.0, ink),
        fill(100.0, 3000.0, 300.0, 20.0, Color::new(0, 200, 0, 1.0)),
        // Underlined text over the image: the underline lands on the image.
        text("underlined", 110.0, 150.0, ink),
        underline(110.0, 170.0, 150.0, Color::new(255, 0, 255, 1.0)),
        // Negative widths reach back left of `x` and up from `y`.
        fill(1100.0, 400.0, -300.0, 20.0, Color::new(0, 160, 0, 1.0)),
        underline(1100.0, 440.0, -300.0, Color::new(0, 0, 200, 1.0)),
    ];
    for scroll in [(0.0, 0.0), (0.0, 2600.0), (40.0, 0.0)] {
        let (want, _) = reference_draw(&gpu, &list, scroll);
        let (got, stats) = fast_draw(&gpu, &list, scroll);
        let (no_text, _) = reference_draw(&gpu, &without_text(&list), scroll);
        assert_culled_matches(&want, &got, &no_text, &format!("at {scroll:?}"));
        let px = |x: usize, y: usize| got[(y * VIEW_W as usize + x) * 4..][..4].to_vec();
        if scroll == (0.0, 0.0) {
            assert!(stats.culled >= 2, "{stats:?}");
            // BGRA: the underline over the image.
            assert_eq!(
                px(250, 171),
                vec![255, 0, 255, 255],
                "underline under the image"
            );
        }
        if scroll == (40.0, 0.0) {
            // Under the scroll translate the negative-width bars are drawn
            // (unscrolled, the renderer drops a negative-size quad), 40px
            // left of where they are laid out.
            assert_eq!(
                px(860, 410),
                vec![0, 160, 0, 255],
                "negative-width fill missing"
            );
            assert_eq!(
                px(860, 441),
                vec![200, 0, 0, 255],
                "negative-width underline missing"
            );
        }
    }
}

#[test]
fn the_culled_frame_check_allows_one_lsb_on_glyph_pixels_only() {
    let n = (VIEW_W * VIEW_H) as usize;
    let white = [255u8, 255, 255, 255].repeat(n);
    // Pixel 10 is a glyph pixel (text drew there); pixel 20 is not.
    let mut want = white.clone();
    want[40..44].copy_from_slice(&[20, 20, 20, 255]);
    let no_text = white.clone();
    let ok = |got: &[u8]| {
        std::panic::catch_unwind(|| assert_culled_matches(&want, got, &no_text, "t")).is_ok()
    };
    let mut got = want.clone();
    got[40..44].copy_from_slice(&[21, 19, 20, 255]);
    assert!(ok(&got), "1 LSB on a glyph pixel is allowed");
    got[40..44].copy_from_slice(&[22, 20, 20, 255]);
    assert!(!ok(&got), "2 LSB on a glyph pixel is not");
    let mut got = want.clone();
    got[80..84].copy_from_slice(&[254, 255, 255, 255]);
    assert!(!ok(&got), "any difference outside text is not");
    let mut got = want.clone();
    got[40..44].copy_from_slice(&[255, 255, 255, 255]);
    assert!(!ok(&got), "a glyph pixel left uncovered is not");
}

/// Xorshift64: the randomized differential's lists are reproducible from
/// their seed alone.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        // Spread small seeds so seed 1 and seed 2 start far apart.
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    /// A float in `lo..hi`.
    fn f(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (self.next() >> 40) as f32 / (1u64 << 24) as f32 * (hi - lo)
    }
    /// An integer in `0..n`.
    fn u(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    fn chance(&mut self, p: f32) -> bool {
        self.f(0.0, 1.0) < p
    }
    /// Whole pixels three times in four, fractional ones otherwise.
    fn pos(&mut self, lo: f32, hi: f32) -> f32 {
        let v = self.f(lo, hi);
        if self.chance(0.75) {
            v.round()
        } else {
            v
        }
    }
    fn color(&mut self, translucent: bool) -> Color {
        let a = if translucent && self.chance(0.3) {
            self.f(0.2, 0.9)
        } else {
            1.0
        };
        Color::new(self.u(256) as u8, self.u(256) as u8, self.u(256) as u8, a)
    }
}

/// What a randomized seed draws: the list, its scroll offsets, and the
/// renderer settings it needs.
struct FuzzCase {
    list: Vec<DisplayCommand>,
    scrolls: Vec<(f32, f32)>,
    batch_bytes: u64,
    gpu_gradients: bool,
}

/// The randomized differential from R1 HOLD @ 5c529151: 20-240 commands
/// mixing fills (translucent, negative-size), glyph runs (sizes 10-40, bold,
/// italic, negative advances) with 0-3 decorations of every style after
/// them, orphan decorations, three image textures (one a checkerboard),
/// normal, wide and rounded borders, clips, translate / scale / rotate
/// transforms, viewport-fixed groups, opacity groups, stacking contexts and
/// box shadows. A quarter of the seeds add a blur backdrop filter, a
/// quarter GPU linear gradients, and 30% a small batch cap. Each is drawn
/// at scroll 0 and at 4 random offsets (some fractional, negative, or with
/// x scroll).
fn fuzz_case(seed: u64) -> FuzzCase {
    let mut rng = Rng::new(seed);
    let height = rng.f(1500.0, 8000.0).round();
    let blur = rng.chance(0.25);
    let gpu_gradients = rng.chance(0.25);
    let batch_bytes = if rng.chance(0.3) {
        [512, 2048, 16 * 1024][rng.u(3)]
    } else {
        MAX_BATCH_BYTES
    };
    let mut list = vec![fill(
        0.0,
        0.0,
        VIEW_W as f32,
        height,
        Color::new(255, 255, 255, 1.0),
    )];
    // The pops owed for the groups open now, innermost last.
    let mut open: Vec<DisplayCommand> = Vec::new();
    let mut fixed = false;
    let n = 20 + rng.u(221);
    const CHARS: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789 .,gjpqyW";
    let styles = [
        rustkit_layout::TextDecorationStyleValue::Solid,
        rustkit_layout::TextDecorationStyleValue::Double,
        rustkit_layout::TextDecorationStyleValue::Dotted,
        rustkit_layout::TextDecorationStyleValue::Dashed,
        rustkit_layout::TextDecorationStyleValue::Wavy,
    ];
    while list.len() < n {
        let x = rng.pos(-200.0, VIEW_W as f32 + 200.0);
        let y = rng.pos(-200.0, height);
        match rng.u(20) {
            0..=3 => {
                let w = rng.pos(-300.0, 500.0);
                let h = rng.pos(-100.0, 300.0);
                list.push(fill(x, y, w, h, rng.color(true)));
            }
            4 => list.push(DisplayCommand::RoundedRect {
                color: rng.color(true),
                rect: Rect::new(x, y, rng.pos(1.0, 400.0), rng.pos(1.0, 200.0)),
                radius: rustkit_layout::BorderRadius::uniform(rng.f(0.0, 40.0)),
            }),
            5..=8 => {
                let len = 1 + rng.u(40);
                let s: String = (0..len)
                    .map(|_| CHARS[rng.u(CHARS.len())] as char)
                    .collect();
                let size = rng.f(10.0, 40.0).round();
                let advance = if rng.chance(0.1) {
                    -rng.f(1.0, 20.0)
                } else {
                    size * rng.f(0.4, 0.7)
                };
                list.push(DisplayCommand::Text {
                    text: s,
                    x,
                    y,
                    color: rng.color(true),
                    font_size: size,
                    font_family: "sans-serif".to_string(),
                    font_weight: if rng.chance(0.3) { 700 } else { 400 },
                    font_style: rng.chance(0.2) as u8,
                    advances: Some(vec![advance; len]),
                    ascent: Some(size * 0.9),
                    run: None,
                    faces: None,
                });
                for _ in 0..rng.u(4) {
                    let width = advance * len as f32 * if rng.chance(0.8) { 1.0 } else { -1.0 };
                    list.push(DisplayCommand::TextDecoration {
                        x,
                        y: y + size * rng.f(0.0, 1.2),
                        width,
                        thickness: rng.f(1.0, 4.0),
                        color: rng.color(true),
                        style: styles[rng.u(styles.len())],
                    });
                }
            }
            9 => list.push(DisplayCommand::TextDecoration {
                x,
                y,
                width: rng.pos(-300.0, 300.0),
                thickness: rng.f(1.0, 6.0),
                color: rng.color(true),
                style: styles[rng.u(styles.len())],
            }),
            10..=11 => {
                let url = ["a.png", "b.png", "checker.png"][rng.u(3)];
                list.push(image(url, x, y, rng.pos(4.0, 400.0), rng.pos(4.0, 400.0)));
            }
            12 => {
                let wide = rng.chance(0.3);
                let mut side = || {
                    if wide {
                        rng.f(0.0, 400.0)
                    } else {
                        rng.f(0.0, 6.0)
                    }
                };
                let (top, right, bottom, left) = (side(), side(), side(), side());
                list.push(DisplayCommand::Border {
                    color: rng.color(true),
                    rect: Rect::new(x, y, rng.pos(1.0, 400.0), rng.pos(1.0, 200.0)),
                    top,
                    right,
                    bottom,
                    left,
                });
            }
            13 => {
                let wide = rng.chance(0.3);
                let mut side = || {
                    if wide {
                        rng.f(0.0, 400.0)
                    } else {
                        rng.f(0.0, 6.0)
                    }
                };
                let widths = [side(), side(), side(), side()];
                list.push(DisplayCommand::RoundedBorder {
                    rect: Rect::new(x, y, rng.pos(1.0, 400.0), rng.pos(1.0, 200.0)),
                    widths,
                    colors: [
                        rng.color(false),
                        rng.color(false),
                        rng.color(false),
                        rng.color(false),
                    ],
                    radius: rustkit_layout::BorderRadius::uniform(rng.f(0.0, 30.0)),
                });
            }
            14 => list.push(DisplayCommand::BoxShadow {
                offset_x: rng.f(-10.0, 10.0),
                offset_y: rng.f(-10.0, 10.0),
                blur_radius: rng.f(0.0, 20.0),
                spread_radius: rng.f(-5.0, 10.0),
                color: rng.color(true),
                rect: Rect::new(x, y, rng.pos(10.0, 300.0), rng.pos(10.0, 200.0)),
                border_radius: rustkit_layout::BorderRadius::uniform(rng.f(0.0, 20.0)),
                inset: rng.chance(0.2),
            }),
            15 if blur => list.push(DisplayCommand::BackdropFilter {
                rect: Rect::new(x, y, rng.pos(50.0, 500.0), rng.pos(50.0, 300.0)),
                border_radius: rustkit_layout::BorderRadius::uniform(rng.f(0.0, 20.0)),
                steps: vec![rustkit_layout::FilterStep::Blur(rng.f(1.0, 12.0))],
                own_groups: 0,
            }),
            15 if gpu_gradients => {
                let stops = (0..2 + rng.u(3))
                    .map(|_| rustkit_css::ColorStop::new(rng.color(true), None))
                    .collect();
                list.push(DisplayCommand::LinearGradient {
                    rect: Rect::new(x, y, rng.pos(10.0, 500.0), rng.pos(10.0, 300.0)),
                    direction: rustkit_css::GradientDirection::Angle(rng.f(0.0, 360.0)),
                    stops,
                    repeating: rng.chance(0.2),
                    border_radius: rustkit_layout::BorderRadius::default(),
                });
            }
            16 => {
                let w = rng.pos(10.0, 600.0);
                let h = rng.pos(10.0, 600.0);
                if rng.chance(0.5) {
                    list.push(DisplayCommand::PushClip(Rect::new(x, y, w, h)));
                } else {
                    list.push(DisplayCommand::PushClipRounded {
                        rect: Rect::new(x, y, w, h),
                        radius: rustkit_layout::BorderRadius::uniform(rng.f(0.0, 40.0)),
                    });
                }
                open.push(DisplayCommand::PopClip);
            }
            17 => {
                let matrix = match rng.u(3) {
                    // Sticky-like: content pulled up or down a long way.
                    0 => [
                        1.0,
                        0.0,
                        0.0,
                        1.0,
                        rng.f(-200.0, 200.0),
                        rng.f(-3000.0, 3000.0),
                    ],
                    1 => {
                        let s = rng.f(0.1, 2.0);
                        [s, 0.0, 0.0, s, rng.f(-100.0, 100.0), rng.f(-100.0, 100.0)]
                    }
                    _ => {
                        let (sin, cos) = rng.f(0.0, std::f32::consts::TAU).sin_cos();
                        [cos, sin, -sin, cos, 0.0, 0.0]
                    }
                };
                list.push(DisplayCommand::PushTransform {
                    matrix,
                    origin: (rng.pos(0.0, VIEW_W as f32), rng.pos(0.0, height)),
                });
                open.push(DisplayCommand::PopTransform);
            }
            18 => {
                if rng.chance(0.5) {
                    list.push(DisplayCommand::PushOpacity(rng.f(0.1, 0.95)));
                    open.push(DisplayCommand::PopOpacity);
                } else {
                    list.push(DisplayCommand::PushStackingContext {
                        z_index: rng.u(5) as i32 - 2,
                        rect: Rect::new(x, y, rng.pos(10.0, 600.0), rng.pos(10.0, 600.0)),
                    });
                    open.push(DisplayCommand::PopStackingContext);
                }
            }
            19 if !fixed && open.is_empty() => {
                list.push(DisplayCommand::PushViewportFixed);
                open.push(DisplayCommand::PopViewportFixed);
                fixed = true;
            }
            _ => {
                if let Some(pop) = open.pop() {
                    fixed &= !matches!(pop, DisplayCommand::PopViewportFixed);
                    list.push(pop);
                }
            }
        }
    }
    list.extend(open.into_iter().rev());
    let mut scrolls = vec![(0.0, 0.0)];
    for _ in 0..4 {
        let mut y = rng.f(-100.0, height);
        if rng.chance(0.6) {
            y = y.round();
        }
        let x = if rng.chance(0.2) {
            rng.pos(-50.0, 300.0)
        } else {
            0.0
        };
        scrolls.push((x, y));
    }
    FuzzCase {
        list,
        scrolls,
        batch_bytes,
        gpu_gradients,
    }
}

/// One frame of a randomized case, in a fresh renderer (so the glyph atlas
/// starts empty, as on a page's first frame).
fn fuzz_draw(
    gpu: &Gpu,
    case: &FuzzCase,
    list: &[DisplayCommand],
    scroll: (f32, f32),
    reference: bool,
) -> (Vec<u8>, FrameStats) {
    let mut r = renderer(gpu);
    let solid = |rgb: [u8; 3]| [rgb[2], rgb[1], rgb[0], 255].repeat(16 * 16);
    r.upload_image("a.png", 16, 16, &solid([200, 120, 30]))
        .unwrap();
    r.upload_image("b.png", 16, 16, &solid([30, 160, 220]))
        .unwrap();
    let checker: Vec<u8> = (0..16 * 16)
        .flat_map(|i| {
            if (i % 16 / 2 + i / 16 / 2) % 2 == 0 {
                [0, 0, 0, 255]
            } else {
                [255, 255, 255, 255]
            }
        })
        .collect();
    r.upload_image("checker.png", 16, 16, &checker).unwrap();
    r.max_batch_bytes = case.batch_bytes;
    r.gpu_gradients_enabled = case.gpu_gradients;
    // Warm the glyph atlas the same way for both paths: draw the full list
    // on the reference path first. Atlas packing then no longer depends on
    // which glyphs culling skipped, so culled and unculled must match byte
    // for byte (Atlas's amended ruling on #710, R1 HOLD @ 6a5c89eb).
    r.reference = true;
    let _ = draw(gpu, &mut r, list, scroll);
    r.reference = reference;
    let frame = draw(gpu, &mut r, list, scroll);
    (frame, r.last_frame_stats())
}

/// The reviewer's randomized differential (R1 HOLD @ 5c529151), on demand:
/// `cargo test --release -p rustkit-renderer --lib -- --ignored culling_matches_unculled_on_random_lists --nocapture`
/// (`CULL_FUZZ_SEEDS=300` is the count, `CULL_FUZZ_FIRST=1` the first seed).
/// Every frame is checked with `assert_culled_matches`: no coverage or
/// order difference anywhere, 1 LSB at most on glyph pixels. It is ignored
/// because a few hundred seeds take minutes on a software adapter.
#[test]
#[ignore = "randomized differential; run with --release --ignored --nocapture"]
fn culling_matches_unculled_on_random_lists() {
    let Some(gpu) = gpu() else {
        eprintln!("SKIP: no GPU adapter");
        return;
    };
    let env = |k: &str, d: u64| {
        std::env::var(k)
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(d)
    };
    let first = env("CULL_FUZZ_FIRST", 1);
    let seeds = env("CULL_FUZZ_SEEDS", 300);
    eprintln!("adapter: {}; seeds {first}..{}", gpu.adapter, first + seeds);
    let (mut frames, mut exact, mut culled, mut failures) = (0usize, 0usize, 0u64, Vec::new());
    for seed in first..first + seeds {
        let case = fuzz_case(seed);
        let no_text_list = without_text(&case.list);
        for &scroll in &case.scrolls {
            let (want, _) = fuzz_draw(&gpu, &case, &case.list, scroll, true);
            let (got, stats) = fuzz_draw(&gpu, &case, &case.list, scroll, false);
            frames += 1;
            culled += stats.culled;
            if want == got {
                exact += 1;
                continue;
            }
            // With both atlases warmed identically there is no tolerance:
            // any difference is a culling bug. The glyph check only says
            // where it is.
            let (no_text, _) = fuzz_draw(&gpu, &case, &no_text_list, scroll, true);
            let e = check_culled(&want, &got, &no_text)
                .err()
                .unwrap_or_else(|| "glyph pixels differ within 1 LSB".into());
            eprintln!(
                "seed {seed} at {scroll:?} ({} commands): {e}",
                case.list.len()
            );
            failures.push((seed, scroll));
        }
    }
    eprintln!(
        "{frames} frames ({culled} commands culled): {exact} byte-identical, \
         {} failing",
        failures.len()
    );
    assert!(failures.is_empty(), "failing (seed, scroll): {failures:?}");
}
