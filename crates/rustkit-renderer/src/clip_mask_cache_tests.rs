//! The outline clip mask cache under scrolling (C8b, #1052 R1 nit 1).
//!
//! A `clip-path` outline is rasterised on the CPU into a coverage mask and
//! cached by shape. The key must not change while a page scrolls, or every
//! scrolled frame pays a rasterisation, a texture and an upload per box: a
//! box that crosses the viewport edge, and a scroll by a fraction of a
//! pixel (a trackpad), must find their masks again. The GPU tests skip,
//! loudly, where no adapter exists.

use super::*;

const VIEW_W: u32 = 400;
const VIEW_H: u32 = 300;
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Bgra8Unorm;

struct Gpu {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
}

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
    let (device, queue) = pollster::block_on(adapter.request_device(
        &wgpu::DeviceDescriptor {
            label: Some("clip mask cache test device"),
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
    let mut r = Renderer::new(gpu.device.clone(), gpu.queue.clone(), FORMAT).expect("renderer");
    r.set_viewport_size(VIEW_W, VIEW_H);
    r
}

/// A green triangle clipped out of a 200x200 box at (100, `top`).
fn triangle(top: f32) -> Vec<DisplayCommand> {
    let rect = Rect::new(100.0, top, 200.0, 200.0);
    vec![
        DisplayCommand::PushClipPath {
            rect,
            contours: vec![vec![(200.0, top), (300.0, top + 200.0), (100.0, top + 200.0)]],
            even_odd: false,
        },
        DisplayCommand::SolidColor(Color::new(0, 170, 0, 1.0), rect),
        DisplayCommand::PopClip,
    ]
}

/// Draw `list` at `scroll`; the frame's pixels and stats.
fn draw(gpu: &Gpu, r: &mut Renderer, list: &[DisplayCommand], scroll: (f32, f32)) -> (Vec<u8>, FrameStats) {
    let (texture, view) = screenshot::create_offscreen_target(&gpu.device, VIEW_W, VIEW_H, FORMAT);
    r.set_scroll_offset(scroll);
    r.execute(list, &view).expect("execute");
    let stats = r.last_frame_stats();
    r.set_scroll_offset((0.0, 0.0));
    let readback = screenshot::GpuReadbackBuffer::new(&gpu.device, VIEW_W, VIEW_H);
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    readback.copy_from_texture(&mut encoder, &texture);
    gpu.queue.submit(std::iter::once(encoder.finish()));
    (readback.read_data_sync(&gpu.device).expect("readback"), stats)
}

/// Masks rasterised over `frames` frames after the first, at scroll
/// offsets `step(i)`.
fn misses_while_scrolling(step: impl Fn(usize) -> f32, frames: usize) -> u64 {
    let Some(gpu) = gpu() else {
        eprintln!("SKIPPED: no GPU adapter");
        return 0;
    };
    let mut r = renderer(&gpu);
    let list = triangle(-80.0);
    let (_, first) = draw(&gpu, &mut r, &list, (0.0, step(0)));
    assert_eq!(first.clip_masks_rasterized, 1, "the first frame rasterises the outline once");
    (1..=frames).map(|i| draw(&gpu, &mut r, &list, (0.0, step(i))).1.clip_masks_rasterized).sum()
}

#[test]
fn a_box_across_the_top_edge_finds_its_mask_again_on_whole_pixel_scrolls() {
    // 20 frames, 3px a frame, the box always crossing the top edge.
    let misses = misses_while_scrolling(|i| 3.0 * i as f32 - 30.0, 20);
    assert_eq!(misses, 0, "a whole-pixel scroll re-rasterised the mask {misses} times in 20 frames");
}

#[test]
fn a_fractional_scroll_does_not_miss_every_frame() {
    // 60 frames of 0.37px: the sub-pixel phase takes many values, but the
    // mask for each phase is made once.
    let misses = misses_while_scrolling(|i| 0.37 * i as f32, 60);
    assert!(misses <= 16, "a fractional scroll re-rasterised the mask {misses} times in 60 frames");
}

#[test]
fn a_mask_found_again_lands_where_the_outline_is() {
    let Some(gpu) = gpu() else {
        eprintln!("SKIPPED: no GPU adapter");
        return;
    };
    // The box at -80, scrolled by 30, after warming the cache at 0: the
    // frame must equal the same box drawn at -110 unscrolled by a fresh
    // renderer.
    let mut r = renderer(&gpu);
    let _ = draw(&gpu, &mut r, &triangle(-80.0), (0.0, 0.0));
    let (scrolled, _) = draw(&gpu, &mut r, &triangle(-80.0), (0.0, 30.0));
    let mut fresh = renderer(&gpu);
    let (moved, _) = draw(&gpu, &mut fresh, &triangle(-110.0), (0.0, 0.0));
    let off = scrolled.iter().zip(&moved).filter(|(a, b)| a.abs_diff(**b) > 2).count();
    assert_eq!(off, 0, "{off} channel values differ between the scrolled and the moved triangle");
}
