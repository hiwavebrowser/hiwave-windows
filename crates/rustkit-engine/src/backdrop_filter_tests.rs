//! CH2.E8: `backdrop-filter` paints the content behind the box filtered
//! (Filter Effects 2 §3). Until this it reached the cascade only as a
//! stacking-context bit: nothing behind a box was ever blurred.
//!
//! Every expected pixel is Chromium 143.0.7499.4's, from the same page
//! (`ch2_e8_backdrop_probe.mjs` in the PR's receipts): 400x300, a body of
//! 10px vertical stripes, and a 300x80 bar at (50, 100) carrying the
//! declaration under test. Tolerances are stated per assertion; the blur
//! is a Gaussian of standard deviation = the `blur()` length, sampled
//! within the bar with mirrored edges, which is what Chromium paints to
//! within 1.1 levels on this page (Skia's GPU kernel differs a little).

use super::*;

const W: u32 = 400;
const H: u32 = 300;

/// (even stripe, odd stripe) colours.
const BLACK_WHITE: (&str, &str) = ("#000", "#fff");
const RED_BLUE: (&str, &str) = ("#f00", "#00f");

/// The stripes are 40 boxes of a flex row (not a gradient, so the page
/// tests the backdrop and nothing else).
fn page((even, odd): (&str, &str), bar: &str) -> String {
    format!(
        "<!DOCTYPE html><html><head><style>html,body{{margin:0}}\
         #s{{display:flex;width:400px;height:300px}}#s i{{flex:none;width:10px;height:300px;background:{odd}}}\
         #s i:nth-child(odd){{background:{even}}}\
         #bar{{position:absolute;left:50px;top:100px;width:300px;height:80px;{bar}}}\
         </style></head><body><div id=\"s\">{}</div><div id=\"bar\"></div></body></html>",
        "<i></i>".repeat(40)
    )
}

/// The view's frame, row-major RGB.
fn frame(html: &str) -> Vec<[u8; 3]> {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: W, height: H })
        .expect("view");
    engine.load_html(view, html).expect("load_html");
    let viewhost_id = engine.views[&view].viewhost_id;
    let list = engine.views[&view].display_list.clone().expect("display list");
    let path = std::env::temp_dir().join(format!(
        "rustkit-backdrop-{}-{:?}.ppm",
        std::process::id(),
        std::thread::current().id()
    ));
    engine
        .presenter
        .as_mut()
        .expect("presenter")
        .draw_scrolled(viewhost_id, &list, (0.0, 0.0), Some(path.to_str().unwrap()))
        .expect("capture");
    let ppm = std::fs::read(&path).expect("frame");
    let _ = std::fs::remove_file(&path);
    let pixels = (W * H) as usize;
    assert!(ppm.len() >= pixels * 3, "a {W}x{H} frame");
    ppm[ppm.len() - pixels * 3..]
        .chunks(3)
        .map(|p| [p[0], p[1], p[2]])
        .collect()
}

fn at(f: &[[u8; 3]], x: u32, y: u32) -> [u8; 3] {
    f[(y * W + x) as usize]
}

fn near(got: [u8; 3], want: [u8; 3], tol: u8, what: &str) {
    let off = got.iter().zip(want).map(|(g, w)| g.abs_diff(w)).max().unwrap();
    assert!(off <= tol, "{what}: got {got:?}, Chromium {want:?} (±{tol})");
}

/// Variance of the red channel along row `y`, columns `x0..x1`.
fn variance(f: &[[u8; 3]], y: u32, x0: u32, x1: u32) -> f32 {
    let v: Vec<f32> = (x0..x1).map(|x| at(f, x, y)[0] as f32).collect();
    let mean = v.iter().sum::<f32>() / v.len() as f32;
    v.iter().map(|a| (a - mean) * (a - mean)).sum::<f32>() / v.len() as f32
}

#[test]
fn a_blurred_bar_smooths_the_stripes_behind_it() {
    let f = frame(&page(BLACK_WHITE, "backdrop-filter:blur(8px)"));
    // Outside the bar the stripes are untouched: black and white.
    near(at(&f, 55, 50), [255; 3], 2, "stripe above the bar");
    near(at(&f, 65, 50), [0; 3], 2, "stripe above the bar");
    let outside = variance(&f, 50, 60, 340);
    let inside = variance(&f, 140, 60, 340);
    assert!(
        inside < outside * 0.05,
        "the stripes behind the bar are blurred: variance inside {inside:.0} vs outside {outside:.0}"
    );
    // Chromium's pixels across the bar (row 140). The edges are mirrored
    // within the bar: x 50 is a white stripe's first column and reads 204,
    // not the ~128 a blur reading the black stripe left of the bar gives.
    for (x, want) in [(50, 204), (55, 184), (65, 128), (75, 135), (200, 126), (349, 51)] {
        near(at(&f, x, 140), [want; 3], 4, &format!("blurred x {x}"));
    }
    // The blur covers the whole box, to its top row.
    near(at(&f, 52, 102), [200; 3], 4, "blurred top row");
}

#[test]
fn saturate_zero_greys_the_backdrop() {
    let f = frame(&page(RED_BLUE, "backdrop-filter:saturate(0)"));
    near(at(&f, 55, 140), [18; 3], 2, "blue through saturate(0)");
    near(at(&f, 65, 140), [54; 3], 2, "red through saturate(0)");
    near(at(&f, 55, 50), [0, 0, 255], 2, "blue above the bar");
}

#[test]
fn a_chained_list_applies_in_order() {
    // brightness(2) first clamps blue at 1, then saturate(0) greys it: 18.
    let f = frame(&page(RED_BLUE, "backdrop-filter:brightness(2) saturate(0)"));
    near(at(&f, 55, 140), [18; 3], 2, "brightness(2) saturate(0) on blue");
    near(at(&f, 65, 140), [54; 3], 2, "brightness(2) saturate(0) on red");
    // The other order greys first and then doubles: 37 and 109.
    let f = frame(&page(RED_BLUE, "backdrop-filter:saturate(0) brightness(2)"));
    near(at(&f, 55, 140), [37; 3], 2, "saturate(0) brightness(2) on blue");
    near(at(&f, 65, 140), [109; 3], 2, "saturate(0) brightness(2) on red");
    // A blur and a colour function together.
    let f = frame(&page(RED_BLUE, "backdrop-filter:blur(8px) saturate(0)"));
    for (x, want) in [(50, 26), (65, 36), (200, 37), (349, 47)] {
        near(at(&f, x, 140), [want; 3], 3, &format!("blur(8px) saturate(0) x {x}"));
    }
}

#[test]
fn contrast_and_opacity_functions() {
    let f = frame(&page(RED_BLUE, "backdrop-filter:contrast(0.5)"));
    near(at(&f, 55, 140), [64, 64, 191], 2, "contrast(0.5) on blue");
    // opacity() fades the filtered backdrop, which is drawn over the
    // backdrop: Chromium's frame is unchanged.
    let f = frame(&page(BLACK_WHITE, "backdrop-filter:opacity(0.5)"));
    near(at(&f, 65, 140), [0; 3], 2, "opacity(0.5) over black");
    near(at(&f, 75, 140), [255; 3], 2, "opacity(0.5) over white");
}

#[test]
fn the_box_background_paints_over_the_filtered_backdrop() {
    let f = frame(&page(BLACK_WHITE, "background:rgba(255,255,255,0.5);backdrop-filter:blur(8px)"));
    for (x, want) in [(50, 230), (65, 192), (200, 191), (349, 153)] {
        near(at(&f, x, 140), [want; 3], 4, &format!("half-white over blur x {x}"));
    }
}

/// chrome.html's peek-mode toolbar has `opacity: 0.95` and a backdrop
/// blur. The box's opacity fades the filtered backdrop with it; the
/// backdrop is still what is behind the box, not the box's own empty group.
#[test]
fn the_box_opacity_fades_the_filtered_backdrop() {
    let f = frame(&page(BLACK_WHITE, "opacity:0.5;backdrop-filter:blur(8px)"));
    for (x, want) in [(50, 230), (65, 64), (200, 63), (349, 26)] {
        near(at(&f, x, 140), [want; 3], 4, &format!("opacity 0.5 blur x {x}"));
    }
}

#[test]
fn an_unknown_function_voids_the_whole_value() {
    let f = frame(&page(BLACK_WHITE, "backdrop-filter:blur(8px) foo(1)"));
    near(at(&f, 65, 140), [0; 3], 2, "no blur: black stripe");
    near(at(&f, 75, 140), [255; 3], 2, "no blur: white stripe");
    // The earlier declaration stays.
    let f = frame(&page(RED_BLUE, "backdrop-filter:saturate(0);backdrop-filter:blur(2px) foo(1)"));
    near(at(&f, 55, 140), [18; 3], 2, "saturate(0) kept");
}

#[test]
fn a_rounded_box_filters_inside_its_corners_only() {
    let f = frame(&page(BLACK_WHITE, "border-radius:20px;backdrop-filter:blur(8px)"));
    // (52, 102) is outside the 20px corner: the white stripe as it is.
    near(at(&f, 52, 102), [255; 3], 2, "outside the corner");
    near(at(&f, 65, 140), [128; 3], 4, "inside");
}
