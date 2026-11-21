//! An emoji that is text by default (no Emoji_Presentation) is drawn from
//! the system's cascade face, as in Chromium; only an Emoji_Presentation
//! character, or one followed by U+FE0F, goes to Apple Color Emoji first.
//! The chrome sidebar's action rows (flex, 8px padding, 14px icon in the
//! system font) are 33 tall for `🛡` and `⚙` and 39 for `🚫` and `📈`;
//! RustKit made all four 39 and `⚙` 18px wide, which put every box below
//! the actions section 12px low on the chrome board.
//!
//! Expected values are getBoundingClientRect in Chrome for Testing
//! 148.0.7778.216 on the same markup (impl-board3 probe, 2026-11-08):
//! `🛡` is `.AppleColorEmojiUI`, `⚙` Menlo, `🚫` / `📈` / `🛡️` Apple Color
//! Emoji (CSS.getPlatformFontsForNode).

use super::*;

fn eval(page: &str, script: &str) -> String {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 800, height: 600 })
        .expect("view");
    engine.load_html(view, page).expect("load");
    let got = engine.execute_script(view, script).expect("script");
    got.trim_start_matches("String(\"").trim_end_matches("\")").to_string()
}

const ROWS: &str = "<!doctype html><body style=\"font-family:-apple-system,BlinkMacSystemFont,sans-serif;margin:0\">\
    <div class=row style=\"display:flex;align-items:center;gap:8px;padding:8px\"><span class=i style=\"font-size:14px\">&#128737;</span><span style=\"font-size:13px\">Shield</span></div>\
    <div class=row style=\"display:flex;align-items:center;gap:8px;padding:8px\"><span class=i style=\"font-size:14px\">&#128683;</span><span style=\"font-size:13px\">Block</span></div>\
    <div class=row style=\"display:flex;align-items:center;gap:8px;padding:8px\"><span class=i style=\"font-size:14px\">&#128200;</span><span style=\"font-size:13px\">Report</span></div>\
    <div class=row style=\"display:flex;align-items:center;gap:8px;padding:8px\"><span class=i style=\"font-size:14px\">&#9881;</span><span style=\"font-size:13px\">Settings</span></div>\
    <div class=row style=\"display:flex;align-items:center;gap:8px;padding:8px\"><span class=i style=\"font-size:14px\">&#128737;&#65039;</span><span style=\"font-size:13px\">ShieldVS16</span></div>\
    <div class=row style=\"display:flex;align-items:center;gap:8px;padding:8px\"><span class=i style=\"font-size:14px\">&#169;&#65039;</span><span style=\"font-size:13px\">CopyrightVS16</span></div>\
    <div class=row style=\"display:flex;align-items:center;gap:8px;padding:8px\"><span class=i style=\"font-size:14px\">&#9662;&#65039;</span><span style=\"font-size:13px\">DownVS16</span></div>\
    <div class=row style=\"display:flex;align-items:center;gap:8px;padding:8px\"><span class=i style=\"font-size:14px\">&#169;</span><span style=\"font-size:13px\">Copyright</span></div>\
    <div class=row style=\"display:flex;align-items:center;gap:8px;padding:8px\"><span class=i style=\"font-size:14px\">&#9881;&#65038;</span><span style=\"font-size:13px\">GearVS15</span></div>\
    <div class=row style=\"display:flex;align-items:center;gap:8px;padding:8px\"><span class=i style=\"font-size:14px\">&#9749;&#65038;</span><span style=\"font-size:13px\">CoffeeVS15</span></div>";

/// Rows 6-10 (CfT 148, the same probe re-run 2026-11-08 with the variation
/// selectors): `©` + U+FE0F is Apple Color Emoji though the system font has
/// `©` (39 / 23); `▾` U+25BE + U+FE0F has no Emoji property and stays the
/// system font (33 / 17, 6.45 wide); `©` alone is the system font (33 / 17);
/// `⚙` + U+FE0E keeps Menlo (33 / 17); `☕` + U+FE0E asks for text and the
/// system font's cascade answers `.AppleColorEmojiUI` (33 / 17, 18 wide).
#[test]
fn text_default_emoji_rows_match_chromium() {
    let rows = eval(
        ROWS,
        "Array.prototype.map.call(document.querySelectorAll('.row'), function (e) { \
         var r = e.getBoundingClientRect(); return r.y + '/' + r.height; }).join(' ')",
    );
    assert_eq!(
        rows,
        "0/33 33/39 72/39 111/33 144/39 183/39 222/33 255/33 288/33 321/33",
        "Chromium 148 row y/height"
    );
    let heights = eval(
        ROWS,
        "Array.prototype.map.call(document.querySelectorAll('.i'), function (e) { \
         return e.getBoundingClientRect().height; }).join(' ')",
    );
    assert_eq!(heights, "17 23 23 17 23 23 17 17 17 17", "Chromium 148 icon heights");
    // A selector is zero-width: `▾` + U+FE0F is as wide as `▾` (Chrome
    // 6.453125), `©` is the system font's (12.375), `⚙` + U+FE0E as wide
    // as `⚙` (Menlo, 8.4375). Core Text's advances differ from Skia's
    // rounding in the third decimal, as the gear above.
    let widths = eval(
        ROWS,
        "Array.prototype.map.call(document.querySelectorAll('.i'), function (e) { \
         return e.getBoundingClientRect().width; }).slice(6, 9).join(' ')",
    );
    let got: Vec<f64> = widths.split(' ').map(|w| w.parse().expect("a width")).collect();
    for (got, want) in got.iter().zip([6.453125, 12.375, 8.4375]) {
        assert!((got - want).abs() < 0.05, "icon widths (rows 7-9) {widths}, Chromium 148 6.453125 12.375 8.4375");
    }
    // The gear is Menlo's advance (Chrome 8.4375), not the emoji face's.
    // (The colour faces' advances are 1-2px wider than Chrome's 18 here:
    // Core Text's sbix advance, a separate, older difference.)
    let gear: f64 = eval(
        ROWS,
        "String(document.querySelectorAll('.i')[3].getBoundingClientRect().width)",
    )
    .parse()
    .expect("a width");
    assert!((gear - 8.4375).abs() < 0.05, "gear width {gear}, Chromium 8.4375");
}
