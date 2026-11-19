//! GR1: css-grid-1 §12.6 "maximize tracks" runs BEFORE §12.7 "expand
//! flexible tracks". Free space first grows every track with a finite
//! growth limit toward it (`minmax(0, 300px)` to 300); only what is left
//! is shared by the `fr` tracks. The engine shared the space among the
//! `fr` tracks first, so an empty `minmax(0, 300px)` sidebar beside a
//! `minmax(0, 1fr)` column came out 0 wide (a real-site page's
//! `minmax(0, 340px)` sidebar; found under CQ1, #1070).
//!
//! `tools/parity_oracle/grid_maximize_tracks_cases.json` holds the pages
//! and what the pinned Chromium reports for every element with an id
//! (`grid_flexible_row_log.mjs --file grid_maximize_tracks_cases.json
//! --write`; Chrome for Testing 148.0.7778.216 gives the same boxes):
//! empty and filled items, a gap, a `2fr` track, two `minmax` tracks
//! sharing a short grid, a float and an `inline-grid` (indefinite width),
//! and `auto-fill`/`auto-fit` repeats.

use super::*;

/// The expression `grid_flexible_row_log.mjs` evaluates (`BOXES_XYWH`).
const BOXES: &str = "Array.prototype.map.call(document.querySelectorAll('[id]'), function (e) {\
    var r = e.getBoundingClientRect();\
    return e.id + ':' + Math.round(r.left) + ':' + Math.round(r.top) + ':' + Math.round(r.width) + ':' + Math.round(r.height);\
    }).join(' ')";

/// Shapes that do not match Chrome yet, each with its reason. A shape listed
/// here that starts to match fails the test, so the list cannot go stale.
const GAPS: &[&str] = &[];

#[test]
#[cfg(all(target_os = "macos", feature = "headless"))]
fn maximize_tracks_grows_minmax_tracks_before_the_fr_tracks_as_in_chrome() {
    let data: serde_json::Value =
        serde_json::from_str(include_str!("../../../tools/parity_oracle/grid_maximize_tracks_cases.json"))
            .expect("case file");
    let (w, h) = (data["viewport"][0].as_u64().unwrap(), data["viewport"][1].as_u64().unwrap());
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let id = engine
        .create_headless_view(Bounds::new(0, 0, w as u32, h as u32))
        .expect("headless view");
    let mut wrong = Vec::new();
    let mut gaps_that_pass = Vec::new();
    for case in data["cases"].as_array().expect("cases") {
        let name = case["name"].as_str().unwrap();
        let chrome = case["chrome_boxes"].as_str().expect("run grid_flexible_row_log.mjs --write");
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
