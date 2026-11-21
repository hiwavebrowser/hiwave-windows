//! CH3.A3c: a transparent view (the RustKit chrome stacked over the page,
//! the "alpha route" of `trench/chrome/A3-composition.md`). Its frame is
//! cleared to (0, 0, 0, 0), not to the canvas, and holds premultiplied
//! colour, so Core Animation's source-over (`src + dst * (1 - src.a)`) puts
//! exactly what the document paints over the page: a popup's shadow
//! outside its box, the page in a rounded popup's corners, a translucent
//! overlay dimming the page.

use super::*;

const WIDTH: u32 = 400;
const HEIGHT: u32 = 300;

fn view_of(html: &str, transparent: bool) -> (Engine, EngineViewId) {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: WIDTH, height: HEIGHT })
        .expect("view");
    engine.load_html(view, html).expect("load_html");
    engine.set_view_transparent(view, transparent).expect("set_view_transparent");
    (engine, view)
}

fn frame(engine: &mut Engine, view: EngineViewId) -> Vec<u8> {
    let (w, h, rgba) = engine.frame_rgba(view).expect("frame");
    assert_eq!((w, h), (WIDTH, HEIGHT));
    rgba
}

fn at(rgba: &[u8], x: u32, y: u32) -> [u8; 4] {
    let i = ((y * WIDTH + x) * 4) as usize;
    [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]]
}

/// Every pixel is a valid premultiplied colour: no channel above alpha.
/// A straight-alpha pixel (colour not scaled by its alpha) breaks this and
/// shows as a light or dark fringe once Core Animation blends it.
fn assert_premultiplied(rgba: &[u8]) {
    for (i, p) in rgba.chunks_exact(4).enumerate() {
        let a = p[3] as i32;
        for c in &p[..3] {
            assert!(
                (*c as i32) <= a + 1,
                "pixel ({}, {}) {:?} is not premultiplied",
                i as u32 % WIDTH,
                i as u32 / WIDTH,
                p
            );
        }
    }
}

/// Core Animation's composite of a premultiplied `src` over an opaque
/// page pixel.
fn over(src: [u8; 4], page: [u8; 3]) -> [u8; 3] {
    let k = 1.0 - src[3] as f32 / 255.0;
    let mut out = [0u8; 3];
    for c in 0..3 {
        out[c] = (src[c] as f32 + page[c] as f32 * k).round().min(255.0) as u8;
    }
    out
}

const POPUP: &str = "<!doctype html><style>html,body{margin:0;background:transparent}\
    #popup{position:absolute;left:100px;top:80px;width:160px;height:100px;background:rgb(0,0,255);\
    border-radius:16px;box-shadow:0 8px 24px rgba(0,0,0,0.5)}\
    p{position:absolute;left:10px;top:250px;margin:0;color:#fff;font:16px sans-serif}</style>\
    <div id=popup></div><p>Hello over the page</p>";

#[test]
fn a_transparent_frame_has_alpha_zero_where_nothing_is_painted() {
    let (mut engine, view) = view_of(POPUP, true);
    let rgba = frame(&mut engine, view);
    assert_eq!(at(&rgba, 380, 20), [0, 0, 0, 0], "nothing painted: the page shows");
    assert_eq!(at(&rgba, 380, 280), [0, 0, 0, 0]);
    assert_eq!(at(&rgba, 180, 130), [0, 0, 255, 255], "inside the popup: its own colour, opaque");
    assert_premultiplied(&rgba);
}

#[test]
fn a_popup_shadow_outside_its_box_is_translucent_black() {
    let (mut engine, view) = view_of(POPUP, true);
    let rgba = frame(&mut engine, view);
    // 15px below the popup's bottom edge, mid-width: inside the shadow.
    let shadow = at(&rgba, 180, 195);
    assert!(shadow[3] > 0 && shadow[3] < 200, "shadow alpha {:?}", shadow);
    assert_eq!(&shadow[..3], &[0, 0, 0], "a black shadow, premultiplied");
    // Over a white page the shadow darkens it, but not to black.
    let seen = over(shadow, [255, 255, 255]);
    assert!(seen[0] < 255 && seen[0] > 60, "page under the shadow {:?}", seen);
}

#[test]
fn a_rounded_popup_corner_shows_the_page() {
    let (mut engine, view) = view_of(POPUP, true);
    let rgba = frame(&mut engine, view);
    // The popup's top-left corner pixel lies outside its 16px curve.
    let corner = at(&rgba, 100, 80);
    assert!(corner[2] < 128 && corner[3] < 160, "corner {:?}: not the popup's blue", corner);
    // Over a red page the corner is mostly the page.
    let seen = over(corner, [255, 0, 0]);
    assert!(seen[0] > 100, "the page shows in the corner: {:?}", seen);
}

#[test]
fn a_translucent_overlay_dims_the_page_premultiplied() {
    let html = "<!doctype html><style>html,body{margin:0;background:transparent}\
        #dim{position:fixed;inset:0;background:rgba(0,0,0,0.3)}\
        #tint{position:absolute;left:20px;top:20px;width:40px;height:40px;background:rgba(255,0,0,0.5)}</style>\
        <div id=dim></div><div id=tint></div>";
    let (mut engine, view) = view_of(html, true);
    let rgba = frame(&mut engine, view);
    let dim = at(&rgba, 300, 200);
    assert_eq!(&dim[..3], &[0, 0, 0]);
    assert!((dim[3] as i32 - 77).abs() <= 1, "0.3 alpha: {:?}", dim);
    let page = [200, 100, 50];
    let seen = over(dim, page);
    for c in 0..3 {
        assert!((seen[c] as i32 - (page[c] as f32 * 0.7).round() as i32).abs() <= 1, "dimmed page {:?}", seen);
    }
    // Half red over the dimming layer: red 0.5 + black 0.3 * 0.5.
    let tint = at(&rgba, 40, 40);
    assert!((tint[0] as i32 - 128).abs() <= 2, "premultiplied red {:?}", tint);
    assert!((tint[3] as i32 - 166).abs() <= 2, "alpha 0.5 + 0.3 * 0.5: {:?}", tint);
    let seen = over(tint, [255, 255, 255]);
    assert!((seen[0] as i32 - 217).abs() <= 3 && (seen[1] as i32 - 89).abs() <= 3, "tint over white {:?}", seen);
    assert_premultiplied(&rgba);
}

/// Gradients and opacity groups take the renderer's other first-pass
/// paths (they used to clear to white whatever the canvas).
#[test]
fn gradients_and_faded_groups_keep_the_frame_transparent() {
    let html = "<!doctype html><style>html,body{margin:0;background:transparent}\
        #g{position:absolute;left:20px;top:20px;width:100px;height:40px;background:linear-gradient(90deg,red,blue)}\
        #f{position:absolute;left:20px;top:100px;width:100px;height:40px;background:rgb(0,128,0);opacity:0.5}</style>\
        <div id=g></div><div id=f></div>";
    let (mut engine, view) = view_of(html, true);
    let rgba = frame(&mut engine, view);
    assert_eq!(at(&rgba, 300, 250), [0, 0, 0, 0], "outside every box");
    assert_eq!(at(&rgba, 300, 30), [0, 0, 0, 0], "beside the gradient");
    let faded = at(&rgba, 70, 120);
    assert!((faded[3] as i32 - 128).abs() <= 2 && (faded[1] as i32 - 64).abs() <= 2, "faded green {:?}", faded);
    assert_premultiplied(&rgba);
}

/// A view that is not transparent is drawn as before: the canvas (white
/// for a transparent document) fills the frame, opaque.
#[test]
fn an_opaque_view_is_unchanged() {
    let (mut engine, view) = view_of(POPUP, false);
    let rgba = frame(&mut engine, view);
    assert_eq!(at(&rgba, 380, 20), [255, 255, 255, 255]);
    assert_eq!(at(&rgba, 180, 130), [0, 0, 255, 255]);
}
