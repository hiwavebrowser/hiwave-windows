//! SHADOW-BLUR: a blurred box shadow is the shadow shape blurred by a
//! Gaussian of standard deviation half the blur radius (CSS Backgrounds 3
//! §7.2), as Chrome paints it. Until this RustKit approximated the blur with
//! stacked, growing layers: tails twice as dark as Chrome's and a core a
//! third too light, and an inset shadow filled the whole box.
//!
//! Every expected pixel is Chrome for Testing 148.0.7778.216's, from the same
//! pages (`tools/parity_oracle/box_shadow_blur_log.mjs`, output
//! `box_shadow_blur_cases.json`): a 400x300 page with one 120x80 box at
//! (140, 110), sampled along row 150, column 200 and the diagonal out of the
//! box's top-left corner. Chrome's pixels there are an exact Gaussian with
//! sigma = blur / 2, sampled at pixel centres, to within 0.7 of a level.
//!
//! Tolerance: every channel within `TOL` levels of Chrome's, except where a
//! case states its own: Chrome's inset edge sits about a third of a pixel
//! further out than the exact Gaussian (its padding-edge pixel is 3 levels
//! lighter, its corner pixel 6), so the inset blur case allows 6.

use super::*;

const W: u32 = 400;
const H: u32 = 300;
const TOL: u8 = 3;

#[derive(serde::Deserialize)]
struct Case {
    name: String,
    background: String,
    css: String,
    row_y: u32,
    row: Vec<[u8; 3]>,
    column_x: u32,
    column: Vec<[u8; 3]>,
    diagonal: Vec<[u8; 3]>,
}

#[derive(serde::Deserialize)]
struct Oracle {
    chrome: String,
    cases: Vec<Case>,
}

fn oracle() -> Oracle {
    serde_json::from_str(include_str!(
        "../../../tools/parity_oracle/box_shadow_blur_cases.json"
    ))
    .expect("box_shadow_blur_cases.json")
}

/// The same page `box_shadow_blur_log.mjs` builds.
fn page(background: &str, css: &str) -> String {
    format!(
        "<!DOCTYPE html><html><head><style>html,body{{margin:0}}body{{background:{background}}}\
         #b{{position:absolute;left:140px;top:110px;width:120px;height:80px;{css}}}</style></head>\
         <body><div id=\"b\"></div></body></html>"
    )
}

/// The view's frame, row-major RGB.
fn frame(html: &str) -> Vec<[u8; 3]> {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds {
            x: 0,
            y: 0,
            width: W,
            height: H,
        })
        .expect("view");
    engine.load_html(view, html).expect("load_html");
    let viewhost_id = engine.views[&view].viewhost_id;
    let list = engine.views[&view]
        .display_list
        .clone()
        .expect("display list");
    let path = std::env::temp_dir().join(format!(
        "rustkit-shadow-blur-{}-{:?}.ppm",
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

/// Every sampled pixel of `name` against Chrome's: the worst offender and
/// the count past `TOL`, or `Ok`.
fn check(name: &str, tol: u8) {
    let oracle = oracle();
    assert!(
        oracle.chrome.starts_with("148.0.7778.216"),
        "pinned Chrome, got {}",
        oracle.chrome
    );
    let case = oracle.cases.iter().find(|c| c.name == name).expect("case");
    let f = frame(&page(&case.background, &case.css));
    let at = |x: u32, y: u32| f[(y * W + x) as usize];
    let mut samples = Vec::new();
    for (x, want) in case.row.iter().enumerate() {
        samples.push((x as u32, case.row_y, *want));
    }
    for (y, want) in case.column.iter().enumerate() {
        samples.push((case.column_x, y as u32, *want));
    }
    for (k, want) in case.diagonal.iter().enumerate() {
        samples.push((140 - k as u32, 110 - k as u32, *want));
    }
    let mut off = 0;
    let mut worst = (0u8, 0, 0, [0u8; 3], [0u8; 3]);
    for (x, y, want) in samples {
        let got = at(x, y);
        let d = got
            .iter()
            .zip(want)
            .map(|(g, w)| g.abs_diff(w))
            .max()
            .unwrap();
        if d > tol {
            off += 1;
        }
        if d > worst.0 {
            worst = (d, x, y, got, want);
        }
    }
    let (d, x, y, got, want) = worst;
    assert!(
        off == 0,
        "{name} ({}): {off} samples past ±{tol}; worst ({x}, {y}) got {got:?}, Chrome {want:?} (off {d})",
        case.css
    );
}

macro_rules! cases {
    ($($test:ident => $name:literal $(, $tol:literal)?;)*) => {
        $(
            #[test]
            fn $test() {
                check($name, TOL $(.max($tol))?);
            }
        )*
    };
}

cases! {
    blur_4_matches_chrome => "blur4";
    blur_16_matches_chrome => "blur16";
    blur_32_matches_chrome => "blur32";
    blur_64_matches_chrome => "blur64";
    blur_with_spread_matches_chrome => "blur16_spread8";
    blur_with_negative_spread_matches_chrome => "blur32_spread_neg8";
    offset_blur_matches_chrome => "offset";
    rounded_blur_matches_chrome => "round";
    pill_with_a_two_shadow_list_matches_chrome => "pill_list";
    first_shadow_paints_on_top => "list_first_on_top";
    blur_on_a_coloured_page_matches_chrome => "on_colour";
    coloured_shadow_on_a_coloured_page_matches_chrome => "colour_on_colour";
    transparent_box_shows_no_shadow_inside => "transparent_box";
    inset_blur_matches_chrome => "inset16", 6;
    inset_offset_spread_round_matches_chrome => "inset_offset_round";
    inset_ring_matches_chrome => "inset_ring";
    inset_in_a_bordered_box_matches_chrome => "inset_bordered", 6;
}
