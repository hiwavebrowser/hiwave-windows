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
    let renderer = engine.renderer.as_mut().expect("renderer");
    renderer.set_viewport_size(400, 100);
    engine
        .compositor
        .capture_frame_with_renderer(viewhost_id, path.to_str().unwrap(), renderer, commands)
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

    let resets = |engine: &mut Engine| engine.renderer.as_mut().expect("renderer").glyph_cache().resets();
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
