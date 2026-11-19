//! CB1(a): the vector primitives (lines, polylines, polygons, circles,
//! rings, ellipses) are clipped by the clip in force, like rects and glyphs.
//!
//! The chrome board's sidebar_collapsed and downloads_open rows painted a
//! white chevron over the toolbar's reload button: the Mode `<select>` sits
//! inside the collapsed sidebar (`width: 0; overflow: hidden`), its box and
//! label were clipped away, but its drop-down arrow is a `Polyline`, and the
//! renderer pushed line, polygon and circle vertices with no clip at all.
//!
//! Every expected number is Chrome for Testing 148.0.7778.216's (the parity
//! baseline, `PARITY_CHROME_PATH` set to the pinned CfT), headless at
//! 240x60 with the parity launch flags, on the page in the test. Chromium
//! 143.0.7499.4 (Playwright's bundled build) gives the same numbers.

use super::*;

const WIDTH: u32 = 240;
const HEIGHT: u32 = 60;
const BG: [u8; 3] = [17, 17, 17];

fn frame(body: &str) -> Vec<u8> {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: WIDTH, height: HEIGHT })
        .expect("view");
    let html = format!("<!doctype html><body style=\"margin:0;background:#111\">{body}</body>");
    engine.load_html(view, &html).expect("load_html");
    engine.render_view(view).expect("render");
    let path = std::env::temp_dir().join(format!(
        "rustkit-vector-clip-{}-{:?}.ppm",
        std::process::id(),
        std::thread::current().id()
    ));
    engine.capture_frame(view, path.to_str().unwrap()).expect("capture");
    let ppm = std::fs::read(&path).expect("frame");
    let _ = std::fs::remove_file(&path);
    ppm[ppm.len() - WIDTH as usize * HEIGHT as usize * 3..].to_vec()
}

/// Every pixel that is not the page background, as (x, y).
fn ink(pixels: &[u8]) -> Vec<(usize, usize)> {
    pixels
        .chunks_exact(3)
        .enumerate()
        .filter(|(_, p)| *p != BG)
        .map(|(i, _)| (i % WIDTH as usize, i / WIDTH as usize))
        .collect()
}

/// Near-white pixels (a white label or arrow), as (x, y).
fn white(pixels: &[u8]) -> Vec<(usize, usize)> {
    pixels
        .chunks_exact(3)
        .enumerate()
        .filter(|(_, p)| p.iter().map(|c| *c as u32).sum::<u32>() > 600)
        .map(|(i, _)| (i % WIDTH as usize, i / WIDTH as usize))
        .collect()
}

/// chrome.html's collapsed sidebar, reduced: a select in a zero-width
/// `overflow: hidden` box. Chromium paints nothing of it (0 white pixels);
/// RustKit painted the arrow, 8 px of white, outside the box.
#[test]
fn a_drop_down_arrow_outside_an_overflow_clip_is_not_painted() {
    let px = frame(
        r#"<div style="position:absolute;left:0;top:0;width:0;height:100px;overflow:hidden"><select style="width:100%;padding:8px 12px;border:none;background:#06b6d4;color:white;font:13px Arial"><option>Zen</option></select></div>"#,
    );
    let w = white(&px);
    assert!(w.is_empty(), "white pixels outside the clip: {} at {:?}", w.len(), &w[..w.len().min(8)]);
}

/// The control case: the same select, unclipped, still paints its arrow
/// (Chromium: white at x 198..205 in a 200 px select at x 10).
#[test]
fn an_unclipped_drop_down_arrow_is_still_painted() {
    let px = frame(
        r#"<select style="position:absolute;left:10px;top:10px;width:200px;padding:8px 12px;border:none;background:#06b6d4;color:white;font:13px Arial"><option>Zen</option></select>"#,
    );
    let arrow: Vec<_> = white(&px).into_iter().filter(|(x, _)| (190..210).contains(x)).collect();
    assert!(!arrow.is_empty(), "no arrow painted near the right edge");
}

/// A radio (an ellipse and a circle ring) in a zero-width clip: Chromium
/// paints 0 pixels.
#[test]
fn a_radio_outside_an_overflow_clip_is_not_painted() {
    let px = frame(
        r#"<div style="position:absolute;left:0;top:0;width:0;height:60px;overflow:hidden"><input type=radio checked style="margin:4px"></div>"#,
    );
    let i = ink(&px);
    assert!(i.is_empty(), "ink outside the clip: {} at {:?}", i.len(), &i[..i.len().min(8)]);
}

/// A radio cut by a 10 px clip: Chromium paints only inside it (66 pixels,
/// x 4..9). Checked as "some ink, none at x >= 10": the radio's own
/// rendering is not Chromium's yet, only the clip is pinned.
#[test]
fn a_radio_cut_by_an_overflow_clip_paints_only_inside_it() {
    let px = frame(
        r#"<div style="position:absolute;left:0;top:0;width:10px;height:60px;overflow:hidden"><input type=radio checked style="margin:4px"></div>"#,
    );
    let i = ink(&px);
    assert!(!i.is_empty(), "the visible half of the radio painted nothing");
    let outside: Vec<_> = i.iter().filter(|(x, _)| *x >= 10).collect();
    assert!(outside.is_empty(), "ink right of the clip: {} at {:?}", outside.len(), &outside[..outside.len().min(8)]);
}
