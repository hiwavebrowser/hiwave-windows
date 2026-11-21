//! `justify-content` with negative free space (CH2.B19; from #922's review):
//! `center`, `flex-end` and `end` are unsafe in Chrome, so an overflowing
//! line overflows both sides equally (center) or the start side (end);
//! `space-between` / `space-around` / `space-evenly` fall back to the
//! start. `distribute_main_axis` clamped the free space at 0, so every
//! overflowing line was packed at the start.
//!
//! `tools/parity_oracle/flex_justify_overflow_cases.json` holds small pages
//! and, for each, what the pinned Chromium reports for every element with an
//! id: `id:x:y:width:height` of its border box (`grid_flexible_row_log.mjs
//! --file flex_justify_overflow_cases.json --write`).

use super::*;

/// The expression `grid_flexible_row_log.mjs` evaluates in Chromium
/// (`BOXES_XYWH`).
const BOXES: &str = "Array.prototype.map.call(document.querySelectorAll('[id]'), function (e) {\
    var r = e.getBoundingClientRect();\
    return e.id + ':' + Math.round(r.left) + ':' + Math.round(r.top) + ':' + Math.round(r.width) + ':' + Math.round(r.height);\
    }).join(' ')";

/// Shapes that do not match Chrome yet, each with its reason. A shape listed
/// here that starts to match fails the test, so the list cannot go stale.
///
/// - `fixed-inset0-overlay-in-a-zero-size-fixed-wrapper`: CH2.B15. The
///   `inset: 0` overlay is sized against its zero-size fixed wrapper, not the
///   viewport, and Pete's option 2 (11-06) keeps its overflowing line at the
///   start rather than off screen
///   (`consent_click_tests::a_fixed_overlay_in_a_zero_size_fixed_container`).
///   Take it off, and the guard in `distribute_main_axis` out, when B15 lands.
/// - `row-safe-center-fits`: CH2.B19b. `safe center` parses to `flex-start`,
///   so a FITTING item starts at 0 where Chrome centres it. (The overflowing
///   `safe center` page matches only because safe falls back to the start.)
const GAPS: &[&str] = &[
    "fixed-inset0-overlay-in-a-zero-size-fixed-wrapper",
    "row-safe-center-fits",
];

#[test]
#[cfg(all(target_os = "macos", feature = "headless"))]
fn an_overflowing_flex_line_is_justified_as_in_chrome() {
    let data: serde_json::Value = serde_json::from_str(include_str!(
        "../../../tools/parity_oracle/flex_justify_overflow_cases.json"
    ))
    .expect("case file");
    let (w, h) = (data["viewport"][0].as_u64().unwrap(), data["viewport"][1].as_u64().unwrap());
    let mut wrong = Vec::new();
    let mut gaps_that_pass = Vec::new();
    for case in data["cases"].as_array().expect("cases") {
        let name = case["name"].as_str().unwrap();
        let chrome = case["chrome_boxes"].as_str().expect("run grid_flexible_row_log.mjs --write");

        let mut engine = Engine::new(EngineConfig::default()).expect("engine");
        let id = engine
            .create_headless_view(Bounds::new(0, 0, w as u32, h as u32))
            .expect("headless view");
        engine.load_html(id, case["html"].as_str().unwrap()).expect("load_html");
        let value = engine.execute_script(id, BOXES).expect("script");
        let got = value
            .strip_prefix("String(\"")
            .and_then(|v| v.strip_suffix("\")"))
            .unwrap_or(&value);

        match (got == chrome, GAPS.contains(&name)) {
            (false, false) => wrong.push(format!("{name}\n   engine {got}\n   chrome {chrome}")),
            (true, true) => gaps_that_pass.push(name),
            _ => {}
        }
    }
    assert!(wrong.is_empty(), "{} wrong:\n{}", wrong.len(), wrong.join("\n"));
    assert!(
        gaps_that_pass.is_empty(),
        "listed as a gap but matches Chrome now, take it off the list: {gaps_that_pass:?}"
    );
}
