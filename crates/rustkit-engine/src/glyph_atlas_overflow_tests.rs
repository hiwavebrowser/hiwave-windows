//! The glyph atlas is one texture for every page the app draws, and when
//! it is full it starts over at its first row. A frame in which that
//! happens has quads already batched that point at the old places, and the
//! glyphs rasterized after the reset are written over them: the text drawn
//! first in that frame samples other glyphs (hand test 7 and 8, H23:
//! "static" over a page opened after other pages, clean when opened first).

use super::*;
use rustkit_css::Color;
use rustkit_layout::DisplayCommand;

fn text(text: &str, x: f32, y: f32, font_size: f32) -> DisplayCommand {
    DisplayCommand::Text {
        text: text.to_string(),
        x,
        y,
        color: Color { r: 0, g: 0, b: 0, a: 1.0 },
        font_size,
        font_family: "Helvetica".to_string(),
        font_weight: 400,
        font_style: 0,
        advances: None,
        ascent: None,
        run: None,
        faces: None,
        undilated: false,
    }
}

/// A line the frame shows, then capitals at `sizes` below the frame: they
/// are not seen and they are rasterized all the same.
fn page(sizes: &[f32]) -> Vec<DisplayCommand> {
    let mut commands = vec![text("Hamburg", 10.0, 10.0, 40.0)];
    for size in sizes {
        commands.push(text("ABCDEFGHIJKLMNOPQRSTUVWXYZ", 0.0, 1000.0, *size));
    }
    commands
}

fn frame(engine: &mut Engine, view: EngineViewId, commands: &[DisplayCommand], tag: &str) -> Vec<u8> {
    let viewhost_id = engine.views[&view].viewhost_id;
    let path = std::env::temp_dir().join(format!("rustkit-atlas-{tag}-{}.ppm", std::process::id()));
    engine
        .presenter
        .as_mut()
        .expect("window presenter")
        .capture_commands(viewhost_id, (400, 100), path.to_str().unwrap(), commands)
        .expect("capture");
    let ppm = std::fs::read(&path).expect("frame");
    let _ = std::fs::remove_file(&path);
    ppm
}

/// Two pages share their first line. Each fits in the atlas; the two
/// together do not, so the second page's frame is the one that fills it.
/// Its first line was cached by the first page at the atlas's first row,
/// which is where the glyphs that come after the reset are written.
#[test]
fn a_frame_that_fills_the_atlas_draws_its_earlier_text_right() {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 400, height: 100 })
        .expect("view");

    let resets = |engine: &mut Engine| engine.renderer_mut().expect("renderer").glyph_cache().resets();
    let first = frame(&mut engine, view, &page(&[200.0, 196.0, 192.0, 188.0, 184.0]), "first");
    assert_eq!(resets(&mut engine), 0, "the first page fits");
    let second = frame(&mut engine, view, &page(&[198.0, 194.0, 190.0, 186.0, 182.0]), "second");
    assert_eq!(resets(&mut engine), 1, "the second page filled the atlas once, and fitted when built again");

    assert!(first.iter().any(|b| *b < 128), "the line is drawn");
    let differing = first.iter().zip(&second).filter(|(a, b)| a != b).count();
    assert_eq!(first.len(), second.len());
    assert_eq!(differing, 0, "bytes of the shared line that differ in the frame that filled the atlas");

    // And in the frames after it. The atlas is emptied when it starts
    // over: the last round's pixels, left between the new glyphs, were
    // sampled at the edges of every glyph drawn from then on.
    let third = frame(&mut engine, view, &page(&[200.0, 196.0, 192.0, 188.0, 184.0]), "third");
    let differing = first.iter().zip(&third).filter(|(a, b)| a != b).count();
    assert_eq!(differing, 0, "bytes of the shared line that differ in a later frame");
}

/// The same capitals at 60px in `colors`, one line each (#1030 nit 4).
/// macOS only: elsewhere every colour takes coverage law 0 (one bucket).
#[cfg(target_os = "macos")]
fn colored_lines(colors: &[[u8; 3]]) -> Vec<DisplayCommand> {
    colors
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let mut cmd = text("ABCDEFGHIJKLMNOPQRSTUVWXYZ", 0.0, 1000.0 + 70.0 * i as f32, 60.0);
            if let DisplayCommand::Text { color, .. } = &mut cmd {
                *color = Color { r: c[0], g: c[1], b: c[2], a: 1.0 };
            }
            cmd
        })
        .collect()
}

/// Atlas fill after one frame of `colors` on a fresh engine, and the
/// atlas's size.
#[cfg(target_os = "macos")]
fn fill_for(colors: &[[u8; 3]], tag: &str) -> (f32, u32) {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 400, height: 100 })
        .expect("view");
    frame(&mut engine, view, &colored_lines(colors), tag);
    let cache = engine.renderer_mut().expect("renderer").glyph_cache();
    assert_eq!(cache.resets(), 0, "{tag}: the frame fits the atlas");
    (cache.atlas_fill(), cache.atlas_size())
}

/// #1030: a glyph's dilated mask carries the pre-blend of its colour's
/// luminance bucket, so text in several colours costs atlas space per
/// BUCKET (at most four on a Mac whose smoothing is `some`), not per
/// colour. Four greys in one bucket cost what one colour does; eight
/// colours spread over the buckets cost at most one line per bucket.
/// macOS with smoothing `some` only (the buckets are its law).
#[cfg(target_os = "macos")]
#[test]
fn text_in_many_colours_costs_atlas_per_luminance_bucket_not_per_colour() {
    if rustkit_renderer::smooth_behavior() != rustkit_renderer::SmoothBehavior::Some {
        eprintln!("SKIP: this Mac's CoreGraphics smoothing is {:?}, not `some`", rustkit_renderer::smooth_behavior());
        return;
    }
    let (one, atlas_size) = fill_for(&[[0, 0, 0]], "one");
    let same_bucket = [[0, 0, 0], [0x11, 0x11, 0x11], [0x22, 0x22, 0x22], [0x0a, 0x0a, 0x0a]];
    let buckets = |cs: &[[u8; 3]]| {
        let mut b: Vec<u8> = cs.iter().map(|c| rustkit_renderer::coverage_for(*c, false)).collect();
        b.sort();
        b.dedup();
        b.len()
    };
    assert_eq!(buckets(&same_bucket), 1, "the four greys share a bucket");
    assert_eq!(fill_for(&same_bucket, "same").0, one, "one bucket, one set of masks");

    let spread = [
        [0, 0, 0], [0x44, 0x44, 0x44], [0x88, 0x88, 0x88], [0xcc, 0xcc, 0xcc],
        [0xff, 0xff, 0xff], [0xff, 0, 0], [0, 0x80, 0], [0, 0, 0xff],
    ];
    let n = buckets(&spread);
    assert!((2..=4).contains(&n), "eight colours fall in {n} buckets");
    let many = fill_for(&spread, "spread").0;
    // Rows are handed out whole: allow one ~70px row of slack per bucket.
    let row = 70.0 / atlas_size as f32;
    assert!(
        many <= n as f32 * (one + row),
        "eight colours in {n} buckets fill {many}, one colour {one}"
    );
    assert!(many > one, "more than one bucket costs more than one set");
}
