//! Probe: what SHOULD RustKit resolve `line-height: normal` to, per font/size?
//!
//! Pair with `scripts/probe_normal_lineheight.py`, which derives the same table
//! from Chrome's committed rects. Run both, diff the columns. This is the A/B
//! instrument for the text-metrics lane -- it needs no live Chrome.
//!
//! STATUS: characterization only. The engine still ships the flat
//! `font_size * 1.2` model (rustkit-css `LineHeight::to_px`). This test proves
//! what Chrome actually does -- `round(ascent) + round(descent) + line_gap`,
//! exact on 19/20 real font/size pairs -- and that it beats the flat model by
//! ~18x on mean error. It does NOT assert what the engine does.
//!
//! Wiring the correct model into layout was measured and REVERTED on
//! 2026-07-13: it regresses the parity board 24/26 -> 23/26, because form
//! control heights (PR #41/#42) are calibrated on the flat model -- Arial
//! 13.3333px happens to give exactly 16.0px under 1.2, and the composed
//! control heights depend on it. Fix the coupling first, then land the model.
//! See trench/forensics/2026-07-13-normal-lineheight-WALL.md.
//!
//! cargo test -p rustkit-layout --test normal_line_height_probe -- --nocapture

use rustkit_css::{FontStyle, FontWeight};
use rustkit_layout::measure_text_advanced;

/// (font-family, font-size, Chrome's resolved normal line-height in px)
/// Chrome column from scripts/probe_normal_lineheight.py on baselines/chrome-148.
const CHROME: &[(&str, f32, f32)] = &[
    ("-apple-system", 14.0, 17.0),
    ("-apple-system", 14.4, 17.0),
    ("-apple-system", 12.8, 15.0),
    ("-apple-system", 13.0, 16.0),
    ("-apple-system", 16.0, 18.0),
    ("-apple-system", 15.2, 18.0),
    ("-apple-system", 12.0, 15.0),
    ("-apple-system", 11.0, 13.0),
    ("system-ui", 16.0, 18.0),
    ("system-ui", 14.4, 17.0),
    ("system-ui", 12.48, 15.0),
    ("system-ui", 12.8, 15.0),
    ("system-ui", 16.32, 19.0),
    ("system-ui", 14.08, 17.0),
    ("system-ui", 20.0, 23.0),
    ("system-ui", 13.0, 16.0),
    ("system-ui", 11.52, 13.0),
    ("system-ui", 40.0, 47.0),
    ("system-ui", 24.0, 28.0),
    ("Arial", 13.33, 16.0),
];

#[test]
fn probe_normal_line_height_vs_chrome() {
    println!(
        "\n{:<15} {:>6} {:>8} {:>8} {:>8} {:>8} {:>8} {:>8}",
        "family", "size", "ascent", "descent", "gap", "rk_norm", "chrome", "err"
    );
    println!("{}", "-".repeat(78));

    let (mut sum_rk, mut sum_flat, mut sum_raw, mut exact) = (0.0f32, 0.0f32, 0.0f32, 0usize);
    for &(family, size, chrome) in CHROME {
        let m = measure_text_advanced("x", family, size, FontWeight::NORMAL, FontStyle::Normal);
        // The TARGET model: Blink rounds ascent/descent independently, so
        // `normal` always lands on a whole pixel.
        let rk = m.ascent.round() + m.descent.round() + m.leading.round();
        let raw = m.height; // ascent + descent + line_gap, unrounded
        let flat = size * 1.2;
        sum_rk += (rk - chrome).abs();
        sum_raw += (raw - chrome).abs();
        sum_flat += (flat - chrome).abs();
        if (rk - chrome).abs() < 0.01 {
            exact += 1;
        }
        println!(
            "{:<15} {:>6.2} {:>8.2} {:>8.2} {:>8.2} {:>8.2} {:>8.2} {:>+8.2}",
            family, size, m.ascent, m.descent, m.leading, rk, chrome, rk - chrome
        );
    }

    let n = CHROME.len() as f32;
    println!("\nmean |error| vs Chrome:");
    println!("  rounded font metrics (TARGET):   {:.3}px", sum_rk / n);
    println!("  raw float metrics (rejected):    {:.3}px", sum_raw / n);
    println!("  flat 1.2 (master):               {:.3}px", sum_flat / n);
    println!("\nexact matches: {}/{}", exact, CHROME.len());

    // Contract: the rounded model must reproduce Chrome on the overwhelming
    // majority of real font/size pairs, and must beat the flat-1.2 model.
    assert!(
        exact >= 18,
        "rounded model matched Chrome exactly on only {exact}/20 pairs"
    );
    assert!(sum_rk < sum_flat, "rounded model is worse than flat 1.2");
}
