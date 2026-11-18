//! GPU canvas A1: the GPU replay of a recording against the CPU replay of
//! the same recording (the bitmap getImageData reads), per op kind.
//!
//! Gated: interior pixels (in the CPU image the pixel and its 8 neighbours
//! all opaque, or all clear) within 2 per channel, so a wrong colour, a
//! missing or an extra shape fails, and the alpha sum within 3%. Edge
//! pixels differ by design (4x MSAA here, tiny-skia's 4x4 supersampling
//! there): their numbers are printed, not gated, until A3 pins bounds from
//! Chrome. The tests skip, loudly, where no adapter exists.

use super::*;
use rustkit_canvas::{compare_premultiplied, set_gpu_canvas, CanvasRenderingContext2D, CanvasStyle, CompositeOperation, ImageData, PixelDiff};
use rustkit_css::Color;

struct Gpu {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
}

fn gpu() -> Option<Gpu> {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor { backends: wgpu::Backends::all(), ..Default::default() });
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        force_fallback_adapter: false,
    }))?;
    let (device, queue) = pollster::block_on(adapter.request_device(
        &wgpu::DeviceDescriptor {
            label: Some("canvas gpu test device"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default(),
            memory_hints: wgpu::MemoryHints::Performance,
        },
        None,
    ))
    .ok()?;
    Some(Gpu { device: Arc::new(device), queue: Arc::new(queue) })
}

fn renderer(gpu: &Gpu) -> Renderer {
    Renderer::new(gpu.device.clone(), gpu.queue.clone(), wgpu::TextureFormat::Bgra8Unorm).expect("renderer")
}

fn gpu_ctx(w: u32, h: u32) -> CanvasRenderingContext2D {
    set_gpu_canvas(Some(true));
    let c = CanvasRenderingContext2D::new(w, h);
    set_gpu_canvas(None);
    c
}

const KEY: &str = "rustkit-canvas:1@gtest";

/// `draw` on a GPU context, handed to the GPU in `frames` batches (draw
/// is called with the frame number), replayed; (GPU pixels, CPU pixels).
fn both(r: &mut Renderer, w: u32, h: u32, frames: usize, draw: impl Fn(&mut CanvasRenderingContext2D, usize)) -> (Vec<u8>, Vec<u8>) {
    let mut c = gpu_ctx(w, h);
    // A new context's first batch starts the texture over (blank here):
    // taken before drawing, so every draw below is drawn by the GPU.
    let first = c.take_gpu_batch(false);
    assert!(matches!(first.reset, Some(rustkit_canvas::GpuReset::Blank)));
    assert_eq!(r.replay_canvas(KEY, &[first]), CanvasReplay::Drawn);
    for f in 0..frames {
        draw(&mut c, f);
        let b = c.take_gpu_batch(false);
        assert!(b.reset.is_none(), "drawn by the GPU, not uploaded");
        assert_eq!(r.replay_canvas(KEY, &[b]), CanvasReplay::Drawn);
    }
    let gpu = r.read_canvas_texture(KEY).expect("texture");
    r.remove_image(KEY);
    (gpu, c.premultiplied_pixels())
}

fn check(name: &str, w: u32, h: u32, gpu: &[u8], cpu: &[u8]) -> PixelDiff {
    let d = compare_premultiplied(gpu, cpu, w, h, 2);
    println!(
        "canvas_gpu {name}: differing {:.2}% ({} of {}), interior over 2: {}, edge pixels {} (over 2: {}, mean |da| {:.2}), max {}, coverage {:.3}%",
        d.differing_pct(),
        d.differing,
        d.pixels,
        d.interior_over,
        d.edge_pixels,
        d.edge_over,
        d.edge_mean_alpha,
        d.max,
        d.coverage_delta * 100.0
    );
    assert_eq!(d.interior_over, 0, "{name}: an interior pixel differs: wrong colour or shape");
    assert!(d.coverage_delta < 0.03, "{name}: coverage off by {:.2}%", d.coverage_delta * 100.0);
    d
}

fn rgba(r: u8, g: u8, b: u8, a: f32) -> CanvasStyle {
    CanvasStyle::Color(Color::new(r, g, b, a))
}

type Case = (&'static str, fn(&mut CanvasRenderingContext2D, usize));

fn cases() -> Vec<Case> {
    vec![
        ("fill-rect", |c, _| {
            c.set_fill_style(rgba(20, 120, 220, 1.0));
            c.fill_rect(5.25, 7.5, 40.0, 23.75);
        }),
        ("fill-path-nonzero", |c, _| {
            c.set_fill_style(rgba(200, 40, 40, 1.0));
            star(c);
            c.fill();
        }),
        ("fill-path-evenodd", |c, _| {
            c.set_fill_style(rgba(40, 160, 40, 1.0));
            star(c);
            c.fill_with_rule(true);
        }),
        ("stroke-caps-joins-dash", |c, _| {
            c.set_stroke_style(rgba(10, 10, 160, 1.0));
            c.set_line_width(7.0);
            c.set_line_cap(rustkit_canvas::LineCap::Round);
            c.set_line_join(rustkit_canvas::LineJoin::Bevel);
            c.begin_path();
            c.move_to(10.0, 60.0);
            c.line_to(40.0, 10.0);
            c.line_to(70.0, 60.0);
            c.stroke();
            c.set_line_cap(rustkit_canvas::LineCap::Square);
            c.set_line_join(rustkit_canvas::LineJoin::Miter);
            c.set_line_dash(vec![9.0, 5.0]);
            c.begin_path();
            c.move_to(10.0, 70.0);
            c.line_to(90.0, 75.0);
            c.stroke();
        }),
        ("stroke-arcs", |c, _| {
            // MotionMark Canvas Arcs' shape: thick arcs, round-ish.
            for i in 0..12 {
                c.set_stroke_style(rgba((i * 20) as u8, 90, 200 - (i * 10) as u8, 1.0));
                c.set_line_width(2.0 + i as f32);
                c.begin_path();
                c.arc(30.0 + (i % 4) as f32 * 20.0, 25.0 + (i / 4) as f32 * 25.0, 9.0, 0.3 * i as f32, 0.3 * i as f32 + 4.0, i % 2 == 0);
                c.stroke();
            }
        }),
        ("global-alpha-overlap", |c, _| {
            c.set_global_alpha(0.5);
            c.set_fill_style(rgba(255, 0, 0, 1.0));
            c.fill_rect(10.0, 10.0, 40.0, 40.0);
            c.set_fill_style(rgba(0, 0, 255, 0.8));
            c.fill_rect(30.0, 30.0, 40.0, 40.0);
        }),
        ("clear-axis-aligned-fractional", |c, _| {
            c.set_fill_style(rgba(0, 0, 0, 1.0));
            c.fill_rect(0.0, 0.0, 100.0, 80.0);
            c.clear_rect(10.0, 10.0, 20.5, 15.0);
            c.translate(0.25, 40.5);
            c.scale(2.0, 1.0);
            c.clear_rect(10.0, 10.0, 7.3, 9.6);
        }),
        ("clear-rotated", |c, _| {
            c.set_fill_style(rgba(0, 0, 0, 1.0));
            c.fill_rect(0.0, 0.0, 100.0, 80.0);
            c.translate(40.0, 20.0);
            c.rotate(0.3);
            c.clear_rect(0.0, 0.0, 30.0, 30.0);
        }),
        ("nested-clips-save-restore", |c, _| {
            c.save();
            c.begin_path();
            c.rect(10.0, 10.0, 60.0, 50.0);
            c.clip();
            c.save();
            c.begin_path();
            c.arc(40.0, 35.0, 25.0, 0.0, std::f32::consts::TAU, false);
            c.clip();
            c.set_fill_style(rgba(250, 150, 0, 1.0));
            c.fill_rect(0.0, 0.0, 100.0, 80.0);
            c.restore();
            c.set_fill_style(rgba(0, 120, 120, 0.6));
            c.fill_rect(0.0, 40.0, 100.0, 40.0);
            c.restore();
            c.set_fill_style(rgba(90, 0, 90, 1.0));
            c.fill_rect(80.0, 60.0, 15.0, 15.0);
        }),
        ("put-image-data-between-draws", |c, _| {
            c.set_fill_style(rgba(0, 200, 0, 1.0));
            c.fill_rect(0.0, 0.0, 50.0, 50.0);
            let mut img = ImageData::new(20, 10);
            for (i, p) in img.data.chunks_exact_mut(4).enumerate() {
                p.copy_from_slice(&[(i * 7) as u8, 50, 200, if i % 3 == 0 { 255 } else { 128 }]);
            }
            c.put_image_data_dirty(&img, 30, 20, [0, 0, 20, 10]);
            c.set_fill_style(rgba(200, 0, 0, 0.5));
            c.fill_rect(35.0, 15.0, 30.0, 30.0);
        }),
        ("composite-ops", |c, _| {
            let ops = [
                CompositeOperation::SourceIn,
                CompositeOperation::SourceOut,
                CompositeOperation::SourceAtop,
                CompositeOperation::DestinationOver,
                CompositeOperation::DestinationIn,
                CompositeOperation::DestinationOut,
                CompositeOperation::DestinationAtop,
                CompositeOperation::Lighter,
                CompositeOperation::Copy,
                CompositeOperation::Xor,
            ];
            for (i, op) in ops.iter().enumerate() {
                let x = (i % 5) as f32 * 20.0;
                let y = (i / 5) as f32 * 40.0;
                c.set_global_composite_operation(CompositeOperation::SourceOver);
                c.set_fill_style(rgba(0, 0, 255, 0.7));
                c.fill_rect(x + 2.0, y + 2.0, 12.0, 12.0);
                c.set_global_composite_operation(*op);
                c.set_fill_style(rgba(255, 0, 0, 0.6));
                c.fill_rect(x + 6.0, y + 6.0, 12.0, 12.0);
            }
            c.set_global_composite_operation(CompositeOperation::SourceOver);
        }),
        // #1300 R1: a gradient whose geometry is not finite as f32 (a
        // page's 1e300) paints nothing on the CPU (tiny-skia refuses it);
        // the GPU must not clear what it covers.
        ("gradient-non-finite", |c, _| {
            c.set_fill_style(rgba(0, 150, 0, 1.0));
            c.fill_rect(0.0, 0.0, 100.0, 80.0);
            let huge = 1e300_f64 as f32;
            let mut g = rustkit_canvas::LinearGradient::new(0.0, 0.0, huge, 0.0);
            g.add_color_stop(0.0, Color::new(255, 0, 0, 1.0));
            g.add_color_stop(1.0, Color::new(0, 0, 255, 1.0));
            c.set_fill_style(CanvasStyle::LinearGradient(g));
            c.fill_rect(10.0, 10.0, 30.0, 30.0);
            let mut g = rustkit_canvas::RadialGradient::new(50.0, 40.0, 0.0, huge, 40.0, 30.0);
            g.add_color_stop(0.0, Color::new(255, 0, 0, 1.0));
            g.add_color_stop(1.0, Color::new(0, 0, 255, 1.0));
            c.set_fill_style(CanvasStyle::RadialGradient(g));
            c.fill_rect(50.0, 10.0, 30.0, 30.0);
        }),
        // #1300 R1: a radial gradient whose end radius is 0 paints nothing
        // on the CPU (tiny-skia's two-point conical needs a radius).
        ("radial-end-radius-zero", |c, _| {
            c.set_fill_style(rgba(0, 150, 0, 1.0));
            c.fill_rect(0.0, 0.0, 100.0, 80.0);
            let mut g = rustkit_canvas::RadialGradient::new(20.0, 20.0, 0.0, 60.0, 40.0, 0.0);
            g.add_color_stop(0.0, Color::new(255, 0, 0, 1.0));
            g.add_color_stop(1.0, Color::new(0, 0, 255, 1.0));
            c.set_fill_style(CanvasStyle::RadialGradient(g));
            c.fill_rect(5.0, 5.0, 90.0, 70.0);
        }),
    ]
}

fn star(c: &mut CanvasRenderingContext2D) {
    c.begin_path();
    let (cx, cy, r) = (50.0f32, 40.0f32, 35.0f32);
    for k in 0..5 {
        let a = -std::f32::consts::FRAC_PI_2 + k as f32 * 4.0 * std::f32::consts::PI / 5.0;
        let (x, y) = (cx + r * a.cos(), cy + r * a.sin());
        if k == 0 {
            c.move_to(x, y);
        } else {
            c.line_to(x, y);
        }
    }
    c.close_path();
}

/// Every op kind the GPU replay draws, GPU vs CPU, under the metric.
#[test]
fn every_op_kind_the_gpu_draws_matches_the_cpu_bitmap_inside_and_within_coverage() {
    let Some(gpu) = gpu() else {
        eprintln!("SKIPPED canvas_gpu: no GPU adapter");
        return;
    };
    let mut r = renderer(&gpu);
    for (name, draw) in cases() {
        let (g, c) = both(&mut r, 100, 80, 1, draw);
        check(name, 100, 80, &g, &c);
    }
}

/// The same recording handed over across several frames draws what one
/// replay of it does (each frame starts from the resolved texture).
#[test]
fn incremental_replay_over_frames_matches_one_replay() {
    let Some(gpu) = gpu() else {
        eprintln!("SKIPPED canvas_gpu: no GPU adapter");
        return;
    };
    let mut r = renderer(&gpu);
    let draw = |c: &mut CanvasRenderingContext2D, f: usize| {
        for i in 0..4 {
            let k = f * 4 + i;
            c.set_stroke_style(rgba((k * 13) as u8, 100, 150, 0.8));
            c.set_line_width(3.0 + (k % 5) as f32);
            c.begin_path();
            c.arc(15.0 + (k % 8) as f32 * 10.0, 20.0 + (k / 8) as f32 * 30.0, 8.0, 0.0, 4.5, false);
            c.stroke();
        }
    };
    let (split, cpu) = both(&mut r, 100, 80, 4, draw);
    let (one, _) = both(&mut r, 100, 80, 1, |c, _| {
        for f in 0..4 {
            draw(c, f);
        }
    });
    check("incremental-vs-cpu", 100, 80, &split, &cpu);
    let d = compare_premultiplied(&split, &one, 100, 80, 2);
    println!("canvas_gpu incremental-vs-one: differing {:.2}%, max {}", d.differing_pct(), d.max);
    assert_eq!(d.interior_over, 0);
}

/// MotionMark's frame: a full clear and fresh draws each frame; the texture
/// shows only the last frame's draws.
#[test]
fn a_full_clear_each_frame_shows_only_that_frame() {
    let Some(gpu) = gpu() else {
        eprintln!("SKIPPED canvas_gpu: no GPU adapter");
        return;
    };
    let mut r = renderer(&gpu);
    let (g, c) = both(&mut r, 100, 80, 3, |c, f| {
        c.clear_rect(0.0, 0.0, 100.0, 80.0);
        c.set_fill_style(rgba(0, 0, 0, 1.0));
        c.fill_rect(10.0 + f as f32 * 20.0, 10.0, 15.0, 15.0);
    });
    check("full-clear-per-frame", 100, 80, &g, &c);
    // Frames 0 and 1's squares are gone.
    let px = |x: usize, y: usize| g[(y * 100 + x) * 4 + 3];
    assert_eq!(px(15, 15), 0);
    assert_eq!(px(35, 15), 0);
    assert_eq!(px(55, 15), 255);
}

/// A missing texture with no reset asks for one; a reset with the CPU
/// bitmap draws it; a canvas past the device's largest texture is denied.
#[test]
fn a_missing_texture_asks_for_a_reset_and_a_huge_canvas_is_denied() {
    let Some(gpu) = gpu() else {
        eprintln!("SKIPPED canvas_gpu: no GPU adapter");
        return;
    };
    let mut r = renderer(&gpu);
    let mut c = gpu_ctx(40, 30);
    let _ = c.take_gpu_batch(false);
    c.set_fill_style(rgba(10, 20, 30, 1.0));
    c.fill_rect(1.0, 1.0, 20.0, 20.0);
    let b = c.take_gpu_batch(false);
    assert_eq!(r.replay_canvas(KEY, &[b]), CanvasReplay::NeedsReset);
    let b = c.take_gpu_batch(true);
    assert_eq!(r.replay_canvas(KEY, &[b]), CanvasReplay::Drawn);
    assert_eq!(r.read_canvas_texture(KEY).expect("texture"), c.premultiplied_pixels(), "a reset uploads the bitmap as it is");
    assert_eq!(r.canvas_texture_bytes(), 40 * 30 * 4);
    r.remove_image(KEY);
    let limit = gpu.device.limits().max_texture_dimension_2d;
    let huge = gpu_ctx(limit + 1, 4);
    assert_eq!(r.replay_canvas("rustkit-canvas:2@ghuge", &[huge.take_gpu_batch(false)]), CanvasReplay::Denied);
}


/// clearRect under a clip clears only inside the clip (HTML 4.12.5.1.14:
/// "clear the pixels in the specified rectangle that also intersect the
/// current clipping region"). The GPU picture does; the CPU bitmap clears
/// the whole rect today, with or without the recording (row CLEAR-CLIP,
/// found here; not in A1's scope), so this case is checked against the
/// spec, not against the bitmap.
#[test]
fn a_clear_under_a_clip_clears_only_inside_it() {
    let Some(gpu) = gpu() else {
        eprintln!("SKIPPED canvas_gpu: no GPU adapter");
        return;
    };
    let mut r = renderer(&gpu);
    let (g, c) = both(&mut r, 100, 80, 1, |c, _| {
        c.set_fill_style(rgba(0, 0, 0, 1.0));
        c.fill_rect(0.0, 0.0, 100.0, 80.0);
        c.begin_path();
        c.arc(60.0, 40.0, 20.0, 0.0, std::f32::consts::TAU, false);
        c.clip();
        c.clear_rect(30.0, 20.0, 40.0, 30.0);
    });
    let alpha = |px: &[u8], x: usize, y: usize| px[(y * 100 + x) * 4 + 3];
    assert_eq!(alpha(&g, 60, 40), 0, "inside the rect and the clip: cleared");
    assert_eq!(alpha(&g, 35, 35), 255, "inside the rect, outside the clip: kept");
    assert_eq!(alpha(&g, 75, 40), 255, "inside the clip, outside the rect: kept");
    // Today's CPU bitmap (CLEAR-CLIP): the whole rect cleared.
    assert_eq!(alpha(&c, 35, 35), 0, "CLEAR-CLIP fixed? then compare this case with the CPU again");
}

/// A frame of curves past the vertex cap is not drawn (#1299 R1): no
/// buffer is made for it, the texture keeps what it had, and the canvas is
/// asked for a reset, which then draws. Counts, no clock.
#[test]
fn a_frame_past_the_vertex_cap_asks_for_a_reset_instead_of_drawing() {
    let Some(gpu) = gpu() else {
        eprintln!("SKIPPED canvas_gpu: no GPU adapter");
        return;
    };
    let mut r = renderer(&gpu);
    r.set_canvas_vertex_cap(1_000);
    let mut c = gpu_ctx(100, 80);
    assert_eq!(r.replay_canvas(KEY, &[c.take_gpu_batch(false)]), CanvasReplay::Drawn);
    c.set_fill_style(rgba(0, 0, 0, 1.0));
    c.fill_rect(0.0, 0.0, 10.0, 10.0);
    assert_eq!(r.replay_canvas(KEY, &[c.take_gpu_batch(false)]), CanvasReplay::Drawn);
    let before = r.read_canvas_texture(KEY).expect("texture");
    c.set_line_width(2.0);
    c.set_line_dash(vec![1.0, 1.0]);
    for i in 0..30 {
        c.begin_path();
        c.arc(50.0, 40.0, 5.0 + i as f32, 0.0, std::f32::consts::TAU, false);
        c.stroke();
    }
    let heavy = c.take_gpu_batch(false);
    assert!(heavy.lower_within(usize::MAX).expect("lowers").1 > 1_000);
    assert_eq!(r.replay_canvas(KEY, &[heavy]), CanvasReplay::NeedsReset);
    assert_eq!(r.read_canvas_texture(KEY).expect("texture"), before, "nothing drawn");
    let reset = c.take_gpu_batch(true);
    assert_eq!(r.replay_canvas(KEY, &[reset]), CanvasReplay::Drawn);
    assert_eq!(r.read_canvas_texture(KEY).expect("texture"), c.premultiplied_pixels());
    r.remove_image(KEY);
}

/// A canvas over the vertex cap frame after frame does not alternate
/// between a stale frame and a bitmap reset: after
/// `CANVAS_OVER_CAP_LIMIT` over-cap replays in a row it is denied (its page
/// shows its CPU bitmap from then on). The resets in between do not clear
/// the count; a drawn frame of ops does (#1299 delta R1). Counts, no clock.
#[test]
fn a_canvas_over_the_cap_frame_after_frame_is_denied() {
    let Some(gpu) = gpu() else {
        eprintln!("SKIPPED canvas_gpu: no GPU adapter");
        return;
    };
    let mut r = renderer(&gpu);
    r.set_canvas_vertex_cap(1_000);
    let mut c = gpu_ctx(100, 80);
    assert_eq!(r.replay_canvas(KEY, &[c.take_gpu_batch(false)]), CanvasReplay::Drawn);
    let heavy = |c: &mut CanvasRenderingContext2D| {
        c.clear_rect(0.0, 0.0, 100.0, 80.0);
        c.set_line_width(2.0);
        c.set_line_dash(vec![1.0, 1.0]);
        for i in 0..30 {
            c.begin_path();
            c.arc(50.0, 40.0, 5.0 + i as f32, 0.0, std::f32::consts::TAU, false);
            c.stroke();
        }
    };
    let light = |c: &mut CanvasRenderingContext2D| {
        c.clear_rect(0.0, 0.0, 100.0, 80.0);
        c.set_line_dash(vec![]);
        c.fill_rect(1.0, 1.0, 5.0, 5.0);
    };
    // Two over the cap, each followed by its reset; then a light frame:
    // the count starts again.
    for _ in 0..2 {
        heavy(&mut c);
        assert_eq!(r.replay_canvas(KEY, &[c.take_gpu_batch(false)]), CanvasReplay::NeedsReset);
        assert_eq!(r.replay_canvas(KEY, &[c.take_gpu_batch(true)]), CanvasReplay::Drawn);
    }
    light(&mut c);
    assert_eq!(r.replay_canvas(KEY, &[c.take_gpu_batch(false)]), CanvasReplay::Drawn);
    let mut results = Vec::new();
    for _ in 0..CANVAS_OVER_CAP_LIMIT {
        heavy(&mut c);
        results.push(r.replay_canvas(KEY, &[c.take_gpu_batch(false)]));
        if results.last() == Some(&CanvasReplay::NeedsReset) {
            assert_eq!(r.replay_canvas(KEY, &[c.take_gpu_batch(true)]), CanvasReplay::Drawn);
        }
    }
    let mut expected = vec![CanvasReplay::NeedsReset; CANVAS_OVER_CAP_LIMIT as usize - 1];
    expected.push(CanvasReplay::Denied);
    assert_eq!(results, expected);
    r.remove_image(KEY);
}

/// #1300 R1: the gradients of a replay are counted against the device's
/// storage-buffer binding limit before any GPU work, like the vertex cap:
/// past it the canvas asks for a reset and, frame after frame, is denied,
/// instead of failing validation (a reset every frame, never denied).
#[test]
fn gradients_past_the_storage_limit_ask_for_a_reset_then_deny() {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor { backends: wgpu::Backends::all(), ..Default::default() });
    let Some(adapter) = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        force_fallback_adapter: false,
    })) else {
        eprintln!("SKIPPED canvas_gpu: no GPU adapter");
        return;
    };
    // Room for two gradient records.
    let limits = wgpu::Limits { max_storage_buffer_binding_size: 1024, ..wgpu::Limits::default() };
    let (device, queue) = pollster::block_on(adapter.request_device(
        &wgpu::DeviceDescriptor { label: Some("low storage"), required_features: wgpu::Features::empty(), required_limits: limits, memory_hints: wgpu::MemoryHints::Performance },
        None,
    ))
    .expect("device");
    let mut r = Renderer::new(Arc::new(device), Arc::new(queue), wgpu::TextureFormat::Bgra8Unorm).expect("renderer");
    let mut c = gpu_ctx(60, 20);
    assert_eq!(r.replay_canvas(KEY, &[c.take_gpu_batch(false)]), CanvasReplay::Drawn);
    let mut results = Vec::new();
    for _ in 0..CANVAS_OVER_CAP_LIMIT {
        for i in 0..3 {
            let mut g = rustkit_canvas::LinearGradient::new(0.0, 0.0, 10.0 + i as f32, 0.0);
            g.add_color_stop(0.0, Color::new(255, 0, 0, 1.0));
            g.add_color_stop(1.0, Color::new(0, 0, 255, 1.0));
            c.set_fill_style(CanvasStyle::LinearGradient(g));
            c.fill_rect(i as f32 * 20.0, 0.0, 20.0, 20.0);
        }
        results.push(r.replay_canvas(KEY, &[c.take_gpu_batch(false)]));
        if results.last() == Some(&CanvasReplay::NeedsReset) {
            assert_eq!(r.replay_canvas(KEY, &[c.take_gpu_batch(true)]), CanvasReplay::Drawn);
        }
    }
    let mut expected = vec![CanvasReplay::NeedsReset; CANVAS_OVER_CAP_LIMIT as usize - 1];
    expected.push(CanvasReplay::Denied);
    assert_eq!(results, expected);
}

/// #1299 R1 nit: a canvas that goes away (its image removed) takes its
/// over-cap count with it; a canvas later shown under the same key starts
/// from zero.
#[test]
fn a_removed_canvas_forgets_its_over_cap_count() {
    let Some(gpu) = gpu() else {
        eprintln!("SKIPPED canvas_gpu: no GPU adapter");
        return;
    };
    let mut r = renderer(&gpu);
    r.set_canvas_vertex_cap(1_000);
    let heavy = |c: &mut CanvasRenderingContext2D| {
        c.set_line_width(2.0);
        c.set_line_dash(vec![1.0, 1.0]);
        for i in 0..30 {
            c.begin_path();
            c.arc(50.0, 40.0, 5.0 + i as f32, 0.0, std::f32::consts::TAU, false);
            c.stroke();
        }
    };
    let mut c = gpu_ctx(100, 80);
    assert_eq!(r.replay_canvas(KEY, &[c.take_gpu_batch(false)]), CanvasReplay::Drawn);
    for _ in 0..CANVAS_OVER_CAP_LIMIT - 1 {
        heavy(&mut c);
        assert_eq!(r.replay_canvas(KEY, &[c.take_gpu_batch(false)]), CanvasReplay::NeedsReset);
        assert_eq!(r.replay_canvas(KEY, &[c.take_gpu_batch(true)]), CanvasReplay::Drawn);
    }
    r.remove_image(KEY);
    assert_eq!(r.canvas_over_cap_counts(), 0, "the count went with the canvas");
    let mut c = gpu_ctx(100, 80);
    assert_eq!(r.replay_canvas(KEY, &[c.take_gpu_batch(false)]), CanvasReplay::Drawn);
    heavy(&mut c);
    assert_eq!(r.replay_canvas(KEY, &[c.take_gpu_batch(false)]), CanvasReplay::NeedsReset, "counted from zero again");
    r.retain_images(|_| false);
    assert_eq!(r.canvas_over_cap_counts(), 0, "retain drops it too");
}
