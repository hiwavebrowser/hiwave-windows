//! FACE CONTRACT at the glyph cache: every atlas entry is keyed by the face
//! that drew it (a decided `GlyphFace`), never by an undecided `Auto`
//! selector, and the gradient-text mask of a colour face lives apart from
//! that face's colour artwork.
//!
//! Prometheus's third R1 on #997: `GradientText` keyed its masks
//! `FaceKey::Auto(None)` and inserted them in the grayscale atlas; a legacy
//! `Text` (no face record) of the same character, font and size built the
//! same key, and `get_or_rasterize_faced` found that mask before it selected
//! the face, so the plain emoji lost its colour artwork whenever gradient
//! text had drawn it first. Draw-order dependent rendering. These pins draw
//! both orders, warm and reset, against a renderer that drew the plain text
//! alone; `GradientText` must also draw the face `select_face` chooses
//! (`©` + U+FE0F is the emoji face, not Helvetica's `©`).
//!
//! The GPU tests skip, loudly, where no adapter exists.

use super::*;
use crate::glyph::FaceKey;

const VIEW_W: u32 = 260;
const VIEW_H: u32 = 64;
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Bgra8Unorm;
const FONT_SIZE: f32 = 24.0;
const GRADIENT_X: f32 = 12.0;
const TEXT_X: f32 = 140.0;
const TOP: f32 = 12.0;

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
            label: Some("face cache test device"),
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
    })
}

fn renderer(gpu: &Gpu) -> Renderer {
    let mut r = Renderer::new(gpu.device.clone(), gpu.queue.clone(), FORMAT).expect("renderer");
    r.set_viewport_size(VIEW_W, VIEW_H);
    r
}

/// A legacy text command: no advances, no ascent, no run, no face record,
/// so paint decides the face itself (`FaceKey::Auto`).
fn text(s: &str, x: f32) -> DisplayCommand {
    DisplayCommand::Text {
        text: s.to_string(),
        x,
        y: TOP,
        color: Color::new(0, 0, 0, 1.0),
        font_size: FONT_SIZE,
        font_family: "Helvetica".to_string(),
        font_weight: 400,
        font_style: 0,
        advances: None,
        ascent: None,
        run: None,
        faces: None,
    }
}

/// A gradient text command of the same font, as the emitter builds it for
/// `background-clip: text`, in the legacy shape (no face record).
fn gradient(s: &str, x: f32) -> DisplayCommand {
    gradient_with(s, x, None)
}

/// The same with layout's face record, parallel to `advances`.
fn gradient_with(s: &str, x: f32, faces: Option<Vec<rustkit_text::GlyphFace>>) -> DisplayCommand {
    use rustkit_css::{ColorStop, GradientDirection, LinearGradient};
    let advances = faces.as_ref().map(|faces| vec![30.0; faces.len()]);
    DisplayCommand::GradientText {
        text: s.to_string(),
        x,
        y: TOP,
        font_size: FONT_SIZE,
        font_family: "Helvetica".to_string(),
        font_weight: 400,
        font_style: 0,
        gradient: rustkit_css::Gradient::Linear(LinearGradient::new(
            GradientDirection::ToRight,
            vec![
                ColorStop::new(Color::new(220, 20, 20, 1.0), Some(0.0)),
                ColorStop::new(Color::new(20, 20, 220, 1.0), Some(1.0)),
            ],
        )),
        rect: Rect::new(x, TOP, 60.0, 32.0),
        advances,
        ascent: None,
        faces,
    }
}

/// One frame of `list` on `r`, read back (BGRA, `VIEW_W * 4` bytes a row).
fn draw(gpu: &Gpu, r: &mut Renderer, list: &[DisplayCommand]) -> Vec<u8> {
    let (texture, view) = screenshot::create_offscreen_target(&gpu.device, VIEW_W, VIEW_H, FORMAT);
    r.execute(list, &view).expect("execute");
    let readback = screenshot::GpuReadbackBuffer::new(&gpu.device, VIEW_W, VIEW_H);
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    readback.copy_from_texture(&mut encoder, &texture);
    gpu.queue.submit(std::iter::once(encoder.finish()));
    readback.read_data_sync(&gpu.device).expect("readback")
}

/// The pixels of the columns `x0..x1`, every row.
fn columns(frame: &[u8], x0: u32, x1: u32) -> Vec<u8> {
    let row = (VIEW_W * 4) as usize;
    (0..VIEW_H as usize)
        .flat_map(|y| {
            frame[y * row + x0 as usize * 4..y * row + x1 as usize * 4]
                .iter()
                .copied()
        })
        .collect()
}

/// The horizontal extent of the ink in columns `x0..x1`: the pixels that
/// differ from the frame's top-left (background) pixel.
fn ink_width(frame: &[u8], x0: u32, x1: u32) -> Option<u32> {
    let row = (VIEW_W * 4) as usize;
    let bg = &frame[0..4];
    let mut left = None;
    let mut right = None;
    for x in x0..x1 {
        let inked = (0..VIEW_H as usize).any(|y| {
            let i = y * row + x as usize * 4;
            &frame[i..i + 4] != bg
        });
        if inked {
            left.get_or_insert(x);
            right = Some(x);
        }
    }
    Some(right? - left? + 1)
}

/// Whether some pixel in the columns carries a hue (r != g or g != b):
/// the emoji's own colours, which a tinted coverage mask of one colour
/// over a grey background never has.
fn has_hue(frame: &[u8], x0: u32, x1: u32) -> bool {
    columns(frame, x0, x1)
        .chunks_exact(4)
        .any(|px| px[0] != px[1] || px[1] != px[2])
}

const TEXT_COLUMNS: (u32, u32) = (TEXT_X as u32 - 4, VIEW_W);
const GRADIENT_COLUMNS: (u32, u32) = (GRADIENT_X as u32 - 4, TEXT_X as u32 - 8);

#[test]
#[cfg(target_os = "macos")]
fn plain_text_keeps_its_colour_artwork_whatever_gradient_text_drew_before() {
    let Some(gpu) = gpu() else {
        eprintln!("SKIP plain_text_keeps_its_colour_artwork_whatever_gradient_text_drew_before: no GPU adapter");
        return;
    };
    // U+1F600 has Emoji_Presentation: under Helvetica `select_face` is the
    // colour fallback, so the plain text's artwork is colour.
    let emoji = "\u{1F600}";
    let (tx0, tx1) = TEXT_COLUMNS;
    let (gx0, gx1) = GRADIENT_COLUMNS;

    let text_alone = draw(&gpu, &mut renderer(&gpu), &[text(emoji, TEXT_X)]);
    let expected_text = columns(&text_alone, tx0, tx1);
    assert!(
        has_hue(&text_alone, tx0, tx1),
        "control: the plain emoji must paint its colour artwork"
    );
    let gradient_alone = draw(&gpu, &mut renderer(&gpu), &[gradient(emoji, GRADIENT_X)]);
    let expected_gradient = columns(&gradient_alone, gx0, gx1);
    assert!(
        ink_width(&gradient_alone, gx0, gx1).is_some(),
        "control: gradient text must paint something"
    );

    // Gradient first, cold cache.
    let mut r = renderer(&gpu);
    let frame = draw(
        &gpu,
        &mut r,
        &[gradient(emoji, GRADIENT_X), text(emoji, TEXT_X)],
    );
    assert!(
        columns(&frame, tx0, tx1) == expected_text,
        "gradient text drawn first changed the plain text's pixels (colour artwork lost)"
    );
    assert!(
        columns(&frame, gx0, gx1) == expected_gradient,
        "gradient first: the gradient's own pixels moved"
    );

    // Plain text first (its colour entry warm), then the gradient in the
    // next frame: the gradient's mask must not shadow the colour entry.
    let mut r = renderer(&gpu);
    let _ = draw(&gpu, &mut r, &[text(emoji, TEXT_X)]);
    let frame = draw(
        &gpu,
        &mut r,
        &[gradient(emoji, GRADIENT_X), text(emoji, TEXT_X)],
    );
    assert!(
        columns(&frame, tx0, tx1) == expected_text,
        "with the plain text's entry warm, a later gradient glyph took its place"
    );
    assert!(
        columns(&frame, gx0, gx1) == expected_gradient,
        "text first: the gradient's own pixels moved"
    );

    // Reset caches (the atlas starting over) and the same frame again.
    r.glyph_cache().clear();
    let frame = draw(
        &gpu,
        &mut r,
        &[gradient(emoji, GRADIENT_X), text(emoji, TEXT_X)],
    );
    assert!(
        columns(&frame, tx0, tx1) == expected_text,
        "after a cache reset, gradient text drawn first changed the plain text's pixels"
    );
    assert!(
        columns(&frame, gx0, gx1) == expected_gradient,
        "after a cache reset: the gradient's own pixels moved"
    );
}

#[test]
#[cfg(target_os = "macos")]
fn every_atlas_entry_is_keyed_by_a_decided_face() {
    let Some(gpu) = gpu() else {
        eprintln!("SKIP every_atlas_entry_is_keyed_by_a_decided_face: no GPU adapter");
        return;
    };
    let mut r = renderer(&gpu);
    let _ = draw(
        &gpu,
        &mut r,
        &[
            gradient("\u{A9}\u{FE0F}a", GRADIENT_X),
            text("\u{A9}\u{FE0F}a", TEXT_X),
        ],
    );
    let keys: Vec<_> = r.glyph_cache().keys().cloned().collect();
    assert!(!keys.is_empty(), "the frame cached nothing");
    let undecided: Vec<_> = keys
        .iter()
        .filter(|key| matches!(key.face, FaceKey::Auto(_)))
        .map(|key| (key.codepoint, key.face))
        .collect();
    assert!(
        undecided.is_empty(),
        "atlas entries keyed by an undecided face (the selector, not the face that drew them): {undecided:?}"
    );
}

#[test]
#[cfg(target_os = "macos")]
fn gradient_text_draws_the_face_the_selector_chooses() {
    let Some(gpu) = gpu() else {
        eprintln!("SKIP gradient_text_draws_the_face_the_selector_chooses: no GPU adapter");
        return;
    };
    let (gx0, gx1) = GRADIENT_COLUMNS;
    // `©` + U+FE0F: the Emoji property makes the colour face the one
    // decision (Chrome for Testing 148, the #997 matrix), whether the
    // command is plain text or gradient text. Helvetica's `©` is narrower.
    let emoji_gradient = ink_width(
        &draw(
            &gpu,
            &mut renderer(&gpu),
            &[gradient("\u{A9}\u{FE0F}", GRADIENT_X)],
        ),
        gx0,
        gx1,
    )
    .expect("gradient © + VS16 paints ink");
    let emoji_text = ink_width(
        &draw(
            &gpu,
            &mut renderer(&gpu),
            &[text("\u{A9}\u{FE0F}", GRADIENT_X)],
        ),
        gx0,
        gx1,
    )
    .expect("plain © + VS16 paints ink");
    let primary_gradient = ink_width(
        &draw(&gpu, &mut renderer(&gpu), &[gradient("\u{A9}", GRADIENT_X)]),
        gx0,
        gx1,
    )
    .expect("gradient © paints ink");
    assert!(
        primary_gradient + 2 < emoji_text,
        "the pin needs the emoji face's © ({emoji_text}px) wider than Helvetica's ({primary_gradient}px)"
    );
    assert!(
        (emoji_gradient as i64 - emoji_text as i64).abs() <= 2,
        "gradient © + VS16 is {emoji_gradient}px of ink, the plain text's emoji face {emoji_text}px: \
         gradient text drew another face than the selector chooses (Helvetica's © is {primary_gradient}px)"
    );

    // With layout's record on the command, paint draws the recorded face
    // and decides nothing: the emoji face where layout measured it, the
    // primary where layout kept it, whatever the selector would say.
    use rustkit_text::GlyphFace;
    let recorded_emoji = ink_width(
        &draw(
            &gpu,
            &mut renderer(&gpu),
            &[gradient_with(
                "\u{A9}\u{FE0F}",
                GRADIENT_X,
                Some(vec![
                    GlyphFace::Fallback { emoji: true },
                    GlyphFace::Ignorable,
                ]),
            )],
        ),
        gx0,
        gx1,
    )
    .expect("recorded emoji face paints ink");
    assert!(
        (recorded_emoji as i64 - emoji_text as i64).abs() <= 2,
        "recorded emoji face: {recorded_emoji}px of ink against the emoji face's {emoji_text}px"
    );
    let recorded_primary = ink_width(
        &draw(
            &gpu,
            &mut renderer(&gpu),
            &[gradient_with(
                "\u{A9}\u{FE0F}",
                GRADIENT_X,
                Some(vec![GlyphFace::Primary, GlyphFace::Ignorable]),
            )],
        ),
        gx0,
        gx1,
    )
    .expect("recorded primary face paints ink");
    assert_eq!(
        recorded_primary, primary_gradient,
        "recorded primary face: paint re-decided the face instead of drawing the record"
    );
}
