//! `position: relative` offsets (Z row C13). `top`/`right`/`bottom`/`left`
//! on a relatively positioned box move the box from its static position
//! (CSS 2.1 section 9.4.3): the box, its descendants and its painted pixels
//! move, its siblings do not, and its geometry reads
//! (`getBoundingClientRect`, `getClientRects`, `offsetTop`/`offsetLeft`,
//! `elementFromPoint`) read the moved position.
//!
//! The expected rows are what Chrome 143.0.7499.4 (the parity oracle's
//! chromium-1200) reports for this hand-made 400x300 page:
//! `id:left:top:width:height` of each border box, `id:offsetLeft:offsetTop:
//! offsetParent`, and the RGB at sampled points of its screenshot. The
//! inline case compares the offset `span` with an unoffset control on the
//! line above, so it does not depend on font metrics.

use super::*;

const WIDTH: u32 = 400;
const HEIGHT: u32 = 300;

const PAGE: &str = r#"<!doctype html>
<html><head><style>
body { margin: 0; background: #fff; font: 16px/20px monospace }
* { box-sizing: border-box }
.c { position: absolute; width: 200px; height: 150px; background: #eee }
#c1 { left: 0; top: 0 } #c2 { left: 200px; top: 0 } #c3 { left: 0; top: 150px } #c4 { left: 200px; top: 150px }
.b { width: 50px; height: 50px }
#a { position: relative; top: 20px; left: 30px; background: #f00 }
#a_abs { position: absolute; top: 0; left: 0; width: 10px; height: 10px; background: #0f0 }
#a_sib { background: #00f }
#b_pre { height: 40px }
#b { position: relative; right: 30px; bottom: 20px; background: #f00 }
#g { position: relative; top: 10px; bottom: 40px; left: 5px; right: 40px; background: #00f }
#p { position: relative; left: 10%; top: 10%; background: #f00 }
#s0, #s { background: #0f0 }
#s { position: relative; left: 15px }
</style></head><body>
<div class="c" id="c1"><div class="b" id="a"><div id="a_abs"></div></div><div class="b" id="a_sib"></div></div>
<div class="c" id="c2"><div id="b_pre"></div><div class="b" id="b"></div><div class="b" id="g"></div></div>
<div class="c" id="c3"><div class="b" id="p"></div></div>
<div class="c" id="c4"><div id="line0">ab<span id="s0">XY</span>cd</div><div id="line1">ab<span id="s">XY</span>cd</div></div>
</body></html>"#;

/// `getBoundingClientRect` of one element, `left:top:width:height`.
fn rect_js(id: &str) -> String {
    format!(
        "(function () {{ var r = document.getElementById('{id}').getBoundingClientRect(); \
         return [r.left, r.top, r.width, r.height].map(Math.round).join(':'); }})()"
    )
}

fn page() -> (Engine, EngineViewId) {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds::new(0, 0, WIDTH, HEIGHT))
        .expect("headless view");
    engine.load_html(view, PAGE).expect("load_html");
    (engine, view)
}

fn eval(engine: &mut Engine, view: EngineViewId, js: &str) -> String {
    let value = engine.execute_script(view, js).expect("script");
    value
        .strip_prefix("String(\"")
        .and_then(|v| v.strip_suffix("\")"))
        .unwrap_or(&value)
        .to_string()
}

fn rect(engine: &mut Engine, view: EngineViewId, id: &str) -> String {
    eval(engine, view, &rect_js(id))
}

/// The rendered frame as a `P6` PPM.
fn frame(engine: &mut Engine, view: EngineViewId, tag: &str) -> Vec<u8> {
    engine.render_view(view).expect("render");
    let path = std::env::temp_dir().join(format!("rustkit-c13-{tag}-{}.ppm", std::process::id()));
    let path_str = path.to_str().unwrap();
    engine.capture_frame(view, path_str).expect("capture");
    let ppm = std::fs::read(&path).expect("frame");
    let _ = std::fs::remove_file(&path);
    ppm
}

/// RGB of a pixel in a `P6` frame of the view's size.
fn pixel(ppm: &[u8], x: usize, y: usize) -> [u8; 3] {
    let pixels = WIDTH as usize * HEIGHT as usize * 3;
    let at = ppm.len() - pixels + (y * WIDTH as usize + x) * 3;
    [ppm[at], ppm[at + 1], ppm[at + 2]]
}

const RED: [u8; 3] = [255, 0, 0];
const GREEN: [u8; 3] = [0, 255, 0];
const BLUE: [u8; 3] = [0, 0, 255];
const CONTAINER: [u8; 3] = [238, 238, 238];

/// (a) `top: 20px; left: 30px` on a 50x50 block at static (0, 0): the rect
/// reads at (30, 20). Chrome: `a:30:20:50:50`.
#[test]
#[cfg(all(target_os = "macos", feature = "headless"))]
fn a_relative_block_reads_its_rect_at_the_offset_position() {
    let (mut engine, view) = page();
    assert_eq!(rect(&mut engine, view, "a"), "30:20:50:50");
    assert_eq!(
        eval(
            &mut engine,
            view,
            "(function () { var r = document.getElementById('a').getClientRects()[0]; \
             return [r.left, r.top, r.width, r.height].map(Math.round).join(':'); })()"
        ),
        "30:20:50:50",
        "getClientRects reads the moved box"
    );
}

/// (b) `right: 30px; bottom: 20px` moves the box left and up: static
/// (200, 40) reads at (170, 20). Chrome: `b:170:20:50:50`.
#[test]
#[cfg(all(target_os = "macos", feature = "headless"))]
fn right_and_bottom_move_the_box_up_and_left() {
    let (mut engine, view) = page();
    assert_eq!(rect(&mut engine, view, "b"), "170:20:50:50");
}

/// (c) `left: 10%; top: 10%` resolve against the containing block's
/// 200x150 content box: static (0, 150) reads at (20, 165). Chrome:
/// `p:20:165:50:50`.
#[test]
#[cfg(all(target_os = "macos", feature = "headless"))]
fn percentage_offsets_resolve_against_the_containing_block() {
    let (mut engine, view) = page();
    assert_eq!(rect(&mut engine, view, "p"), "20:165:50:50");
}

/// (d) An inline `span` with `position: relative; left: 15px` shifts its
/// glyph run 15px right of where the control span on the line above sits,
/// and paints its background there. Chrome: `s0:219.2:151` and
/// `s:234.2:171` (15 right, one 20px line down).
#[test]
#[cfg(all(target_os = "macos", feature = "headless"))]
fn a_relative_inline_shifts_its_glyph_run() {
    let (mut engine, view) = page();
    let delta = eval(
        &mut engine,
        view,
        "(function () { var a = document.getElementById('s0').getBoundingClientRect(); \
         var b = document.getElementById('s').getBoundingClientRect(); \
         return [Math.round(b.left - a.left), Math.round(b.top - a.top)].join(':'); })()",
    );
    assert_eq!(delta, "15:20", "offset span minus control span (dx:dy)");
    let control = eval(
        &mut engine,
        view,
        "(function () { var a = document.getElementById('s0').getBoundingClientRect(); \
         return [Math.round(a.left), Math.round(a.top), Math.round(a.width), Math.round(a.height)].join(':'); })()",
    );
    let c: Vec<usize> = control.split(':').map(|v| v.parse().unwrap()).collect();
    let (x0, y0, h) = (c[0], c[1], c[3]);
    let y1 = y0 + 20;
    let ppm = frame(&mut engine, view, "inline");
    // The offset span renders as the control does, 15px to the right:
    // the same glyph and background pixels, a line down.
    let mut saw_background = false;
    for k in [1, 4, 9, 14] {
        let moved = pixel(&ppm, x0 + 15 + k, y1 + h / 2);
        assert_eq!(
            moved,
            pixel(&ppm, x0 + k, y0 + h / 2),
            "column {k} of the span"
        );
        saw_background |= moved[0] == 0 && moved[1] >= 150 && moved[2] == 0;
    }
    assert!(
        saw_background,
        "the span's green background is at the moved position"
    );
    assert_eq!(
        pixel(&ppm, x0 + 3, y1 + h / 2),
        CONTAINER,
        "the span's static position is left empty"
    );
}

/// (e) The next block sibling keeps its static position: `a_sib` sits at
/// (0, 50), under where `a` would be without offsets. Chrome:
/// `a_sib:0:50:50:50`. (Green on develop; guards the fix.)
#[test]
#[cfg(all(target_os = "macos", feature = "headless"))]
fn siblings_of_a_relative_box_keep_their_static_position() {
    let (mut engine, view) = page();
    assert_eq!(rect(&mut engine, view, "a_sib"), "0:50:50:50");
    assert_eq!(rect(&mut engine, view, "b_pre"), "200:0:200:40");
    assert_eq!(rect(&mut engine, view, "c1"), "0:0:200:150");
}

/// (f) An absolutely positioned child of the relative box takes the moved
/// padding box as its containing block: `top: 0; left: 0` reads at
/// (30, 20). Chrome: `a_abs:30:20:10:10`.
#[test]
#[cfg(all(target_os = "macos", feature = "headless"))]
fn an_absolute_child_moves_with_its_relative_parent() {
    let (mut engine, view) = page();
    assert_eq!(rect(&mut engine, view, "a_abs"), "30:20:10:10");
}

/// (g) `top` and `bottom` both set: `top` wins; `left` and `right` both
/// set: `left` wins (LTR). Static (200, 90) with `top: 10px; bottom: 40px;
/// left: 5px; right: 40px` reads at (205, 100). Chrome: `g:205:100:50:50`.
#[test]
#[cfg(all(target_os = "macos", feature = "headless"))]
fn top_wins_over_bottom_and_left_over_right() {
    let (mut engine, view) = page();
    assert_eq!(rect(&mut engine, view, "g"), "205:100:50:50");
}

/// `offsetLeft`/`offsetTop` include the relative offset (they are read from
/// the box's position, not its static position). Chrome: `a:30:20:c1
/// b:-30:20:c2 g:5:100:c2 p:20:15:c3`.
#[test]
#[cfg(all(target_os = "macos", feature = "headless"))]
fn offset_top_and_left_include_the_relative_offset() {
    let (mut engine, view) = page();
    let offs = eval(
        &mut engine,
        view,
        "['a', 'b', 'g', 'p'].map(function (id) { var e = document.getElementById(id); \
         return id + ':' + e.offsetLeft + ':' + e.offsetTop + ':' + e.offsetParent.id; }).join(' ')",
    );
    assert_eq!(offs, "a:30:20:c1 b:-30:20:c2 g:5:100:c2 p:20:15:c3");
}

/// The frame: each box paints at its offset position and its static
/// position shows the container. Chrome's screenshot at the same points:
/// `55,45=255,0,0 5,5=238,238,238 34,24=0,255,0 25,75=0,0,255
/// 195,45=255,0,0 230,125=0,0,255 45,190=255,0,0`.
#[test]
#[cfg(all(target_os = "macos", feature = "headless"))]
fn the_box_paints_at_its_offset_position() {
    let (mut engine, view) = page();
    let ppm = frame(&mut engine, view, "paint");
    assert_eq!(pixel(&ppm, 55, 45), RED, "a paints at (30,20)");
    assert_eq!(pixel(&ppm, 5, 5), CONTAINER, "a's static position is empty");
    assert_eq!(
        pixel(&ppm, 34, 24),
        GREEN,
        "a's absolute child moved with it"
    );
    assert_eq!(pixel(&ppm, 25, 75), BLUE, "the sibling stayed at (0,50)");
    assert_eq!(pixel(&ppm, 195, 45), RED, "b paints at (170,20), over c1");
    assert_eq!(pixel(&ppm, 230, 125), BLUE, "g paints at (205,100)");
    assert_eq!(pixel(&ppm, 45, 190), RED, "p paints at (20,165)");
}

/// Hit testing reads the moved box: Chrome's `elementFromPoint` at the
/// sampled points: `55,45=a 5,5=c1 34,24=a_abs 25,75=a_sib 210,45=b
/// 230,125=g 45,190=p`.
#[test]
#[cfg(all(target_os = "macos", feature = "headless"))]
fn hit_testing_uses_the_offset_position() {
    let (mut engine, view) = page();
    let hits = eval(
        &mut engine,
        view,
        "[[55, 45], [5, 5], [34, 24], [25, 75], [210, 45], [230, 125], [45, 190]].map(function (p) { \
         return p[0] + ',' + p[1] + '=' + document.elementFromPoint(p[0], p[1]).id; }).join(' ')",
    );
    assert_eq!(
        hits,
        "55,45=a 5,5=c1 34,24=a_abs 25,75=a_sib 210,45=b 230,125=g 45,190=p"
    );
}

/// `b` moved 30px left of its container `c2`, so (195, 45) is inside `b`
/// and outside `c2`. Chrome hits `b` (`195,45=b`); the engine hits `c1`.
/// Not a relative-offset defect: `hit_test_internal` prunes a subtree at
/// its box's border box, so any child that overflows its parent is never
/// hit (`overflow: visible` or not). The frame is right (see
/// `the_box_paints_at_its_offset_position`). Pinned for the hit-test row.
#[test]
#[ignore = "a child overflowing its parent's border box is not hit (hit_test_internal prunes at the parent)"]
#[cfg(all(target_os = "macos", feature = "headless"))]
fn hit_testing_reaches_a_relative_box_that_overflows_its_parent() {
    let (mut engine, view) = page();
    assert_eq!(
        eval(&mut engine, view, "document.elementFromPoint(195, 45).id"),
        "b"
    );
}
