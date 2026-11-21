//! CB1(b): a form control's own box decorations are the author's, and
//! `appearance: none` drops the drop-down's native parts.
//!
//! The chrome board's sidebar rows painted the Mode `<select>` square with a
//! native chevron and the tab strip's new-tab (+) `<button>` with a solid
//! square frame. Chromium paints the select with its 8px radius and no
//! arrow (`appearance: none`, and the `.zen` rule's `background` shorthand
//! resets the author's SVG arrow), and the button's 1px dashed frame with
//! its 6px radius. Two causes:
//! - the form-control painter re-filled the box's background as a square
//!   and drew its own solid frame over the box's (rounded, dashed) border;
//! - `appearance` was not carried, so an `appearance: none` select kept the
//!   arrow, the 4px label inset and the 20px arrow well.
//!
//! Every expected number is Chrome for Testing 148.0.7778.216's (the parity
//! baseline, `PARITY_CHROME_PATH` set to the pinned CfT), headless at
//! 240x60 with the parity launch flags, on the page in the test. Chromium
//! 143.0.7499.4 (Playwright's bundled build) gives the same numbers.

use super::*;

const WIDTH: u32 = 240;
const HEIGHT: u32 = 60;

fn loaded(body: &str, bg: &str) -> (Engine, EngineViewId) {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: WIDTH, height: HEIGHT })
        .expect("view");
    let html = format!("<!doctype html><body style=\"margin:0;background:{bg}\">{body}</body>");
    engine.load_html(view, &html).expect("load_html");
    (engine, view)
}

fn frame(body: &str, bg: &str) -> Vec<u8> {
    let (mut engine, view) = loaded(body, bg);
    engine.render_view(view).expect("render");
    let path = std::env::temp_dir().join(format!(
        "rustkit-form-decoration-{}-{:?}.ppm",
        std::process::id(),
        std::thread::current().id()
    ));
    engine.capture_frame(view, path.to_str().unwrap()).expect("capture");
    let ppm = std::fs::read(&path).expect("frame");
    let _ = std::fs::remove_file(&path);
    ppm[ppm.len() - WIDTH as usize * HEIGHT as usize * 3..].to_vec()
}

fn pixel(px: &[u8], x: usize, y: usize) -> [u8; 3] {
    let at = (y * WIDTH as usize + x) * 3;
    [px[at], px[at + 1], px[at + 2]]
}

/// Near-white pixels (label, arrow), as (x, y).
fn white(px: &[u8]) -> Vec<(usize, usize)> {
    px.chunks_exact(3)
        .enumerate()
        .filter(|(_, p)| p.iter().map(|c| *c as u32).sum::<u32>() > 600)
        .map(|(i, _)| (i % WIDTH as usize, i / WIDTH as usize))
        .collect()
}

fn width_of(engine: &mut Engine, view: EngineViewId) -> f64 {
    let out = engine
        .execute_script(view, "String(document.getElementById('t').getBoundingClientRect().width)")
        .expect("script");
    let digits: String = out.chars().filter(|c| c.is_ascii_digit() || *c == '.').collect();
    digits.parse().unwrap_or_else(|_| panic!("width read {out}"))
}

const CYAN: [u8; 3] = [6, 182, 212];
const DARK: [u8; 3] = [17, 17, 17];

/// Chromium: a 200x33 select at (10,10) with `border-radius: 8px` and a
/// cyan background leaves its corner pixels (10,10) and (11,11) the page's
/// #111 and is cyan at (14,14). RustKit painted the control's background a
/// second time, square, over the rounded one.
#[test]
fn a_rounded_select_keeps_its_rounded_corners() {
    let px = frame(
        r#"<select style="position:absolute;left:10px;top:10px;width:200px;padding:8px 12px;border:none;border-radius:8px;background:#06b6d4;color:white;font:500 13px Arial"><option>Zen</option></select>"#,
        "#111",
    );
    assert_eq!(pixel(&px, 10, 10), DARK, "corner (10,10)");
    assert_eq!(pixel(&px, 11, 11), DARK, "corner (11,11)");
    assert_eq!(pixel(&px, 14, 14), CYAN, "inside the curve (14,14)");
}

/// Chromium: a 28x28 `<button>` with `border: 1px dashed #0f172a` paints
/// its top edge in 3px dashes with 2px gaps of the #334155 page between
/// them (y = 10, x 16..33: dash 16-18, gap 19-20, dash 21-23, gap 24-25,
/// dash 26-28, gap 29-30). RustKit drew the painter's solid frame over it.
#[test]
fn a_dashed_button_frame_has_gaps() {
    let px = frame(
        r#"<button style="position:absolute;left:10px;top:10px;width:28px;height:28px;padding:0;background:transparent;border:1px dashed #0f172a;border-radius:6px;color:#94a3b8;font-size:16px">+</button>"#,
        "#334155",
    );
    let page = [51, 65, 85];
    let dash = [15, 23, 42];
    let top: Vec<[u8; 3]> = (16..33).map(|x| pixel(&px, x, 10)).collect();
    let gaps = top.iter().filter(|p| **p == page).count();
    let dashes = top.iter().filter(|p| **p == dash).count();
    assert!(gaps >= 4, "top edge has {gaps} page pixels between dashes: {top:?}");
    assert!(dashes >= 6, "top edge has {dashes} dash pixels: {top:?}");
}

/// Chromium: `appearance: none` select, 200 wide at x 10, padding 12px:
/// its white label spans x 22..43 (no 4px inset) and nothing is painted
/// near the right edge (no arrow).
#[test]
fn an_appearance_none_select_paints_no_arrow_and_no_label_inset() {
    let px = frame(
        r#"<select style="appearance:none;position:absolute;left:10px;top:10px;width:200px;padding:8px 12px;border:none;border-radius:8px;background:#06b6d4;color:white;font:500 13px Arial"><option>Zen</option></select>"#,
        "#111",
    );
    let w = white(&px);
    let min_x = w.iter().map(|p| p.0).min().expect("a label");
    let max_x = w.iter().map(|p| p.0).max().expect("a label");
    assert!((21..=23).contains(&min_x), "label starts at x {min_x}, Chromium 22");
    assert!(max_x <= 45, "white at x {max_x}: an arrow (Chromium's ink ends at 43)");
}

/// chrome.html's Mode select: an author SVG arrow as `background-image`,
/// then `.zen { background: <color> }`, whose shorthand resets the image.
/// Chromium paints only the label (white x 22..43).
#[test]
fn a_background_shorthand_reset_leaves_an_appearance_none_select_without_an_arrow() {
    let px = frame(
        r#"<style>.s{appearance:none;width:200px;padding:8px 12px;border:none;border-radius:8px;color:white;font:13px Arial;background:#06b6d4;background-image:url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='12' height='12' viewBox='0 0 12 12'%3E%3Cpath fill='white' d='M2 4l4 4 4-4'/%3E%3C/svg%3E");background-repeat:no-repeat;background-position:right 8px center}.s.z{background:#06b6d4}</style><select class="s z" style="position:absolute;left:10px;top:10px"><option>Zen</option></select>"#,
        "#111",
    );
    let max_x = white(&px).iter().map(|p| p.0).max().expect("a label");
    assert!(max_x <= 45, "white at x {max_x}: an arrow");
}

/// Chromium: an `appearance: none` select has no 20px arrow well. Padded
/// 8px 12px with "Zen"/"Balanced" at 13px Arial it is 79 wide (auto: 99);
/// a bare one with "Select" is 40 (`-webkit-appearance: none` the same).
#[test]
fn an_appearance_none_select_has_no_arrow_well() {
    let cases = [
        (r#"<select id=t style="appearance:none;position:absolute;left:10px;top:10px;padding:8px 12px;border:none;font:13px Arial"><option>Zen</option><option>Balanced</option></select>"#, 79.0),
        (r#"<select id=t style="position:absolute;left:10px;top:10px;padding:8px 12px;border:none;font:13px Arial"><option>Zen</option><option>Balanced</option></select>"#, 99.0),
        (r#"<select id=t style="appearance:none;position:absolute;left:10px;top:10px"><option>Select</option></select>"#, 40.0),
        (r#"<select id=t style="-webkit-appearance:none;position:absolute;left:10px;top:10px"><option>Select</option></select>"#, 40.0),
    ];
    for (body, chrome) in cases {
        let (mut engine, view) = loaded(body, "#111");
        let w = width_of(&mut engine, view);
        assert!((w - chrome).abs() <= 0.5, "width {w}, Chromium {chrome}: {body}");
    }
}
