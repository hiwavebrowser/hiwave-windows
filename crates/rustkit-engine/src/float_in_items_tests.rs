//! A flex item or a grid item establishes an independent formatting context
//! (css-flexbox-1 §3, css-grid-1 §6), so its auto height encloses its floats
//! and its max-content width counts a float beside its text. RustKit kept
//! floats inside items out of both: a float-only item was 0 tall, a float
//! beside text pushed the text below it (the item measured without the
//! float's width), and a grid item with text ignored its float's height.
//!
//! Expected values are getBoundingClientRect in Chrome for Testing
//! 148.0.7778.216 on docs/diagnostics/2026-11-11/float_in_flex_grid_items.html
//! (output beside it, `_cft148.txt`).

use super::*;

fn eval(page: &str, script: &str) -> String {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 400, height: 600 })
        .expect("view");
    engine.load_html(view, page).expect("load");
    let got = engine.execute_script(view, script).expect("script");
    got.trim_start_matches("String(\"").trim_end_matches("\")").to_string()
}

const PAGE: &str = include_str!("../../../docs/diagnostics/2026-11-11/float_in_flex_grid_items.html");

/// Rects to 0.1 px: the float + text widths are Chrome's 35.797 against
/// RustKit's 35.79 (the text's own advance, not this fix's subject).
fn rects(ids: &str) -> String {
    eval(
        PAGE,
        &format!(
            "'{ids}'.split(' ').map(function (i) {{ var r = document.getElementById(i).getBoundingClientRect(); \
             return Math.round(r.width * 10) / 10 + 'x' + Math.round(r.height * 10) / 10; }}).join(' ')"
        ),
    )
}

#[test]
fn a_flex_item_encloses_its_floats() {
    assert_eq!(
        rects("r1i r2i r3i r4i r4j c1i c2i"),
        "10x30 35.8x30 35.8x18 10x30 8x30 400x30 400x30",
        "Chromium 148 (width x height): float only, float + text, short float + text, stretched and the item beside it (stretched to the float's line), column ×2"
    );
}

#[test]
fn a_grid_item_encloses_its_floats() {
    assert_eq!(rects("g1i g2i g3i"), "400x30 400x30 400x30", "Chromium 148: grid items hold their 30px float");
}
