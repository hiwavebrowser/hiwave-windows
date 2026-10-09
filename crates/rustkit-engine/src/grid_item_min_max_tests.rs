//! `min-width`, `max-width`, `min-height` and `max-height` on a grid item,
//! against the pinned Chromium.
//!
//! `tools/parity_oracle/grid_item_min_max_cases.json` holds small pages and,
//! for each, what the pinned Chromium reports for every element with an id:
//! `id:x:y:width:height` of its border box (`grid_flexible_row_log.mjs --file
//! grid_item_min_max_cases.json --write`). The engine must give the same
//! string.
//!
//! The first page is bing's search icon: a `<label>` holding an `<svg>`,
//! start-aligned in a 48px column, pinned to 24px by `min-width`,
//! `max-width` and `max-height`. It was 1.6px wide and as tall as its row.

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
/// `a-lone-svg-in-a-start-aligned-item`: Chromium puts the svg on a line (29
/// tall: the image above the baseline, the strut's descent below); a grid
/// item with a single image child is still on the block arm of Phase 9 and
/// is as tall as the image (24). The same gap as `a-lone-image` in
/// `grid_flexible_row_tests`.
const GAPS: &[&str] = &["a-lone-svg-in-a-start-aligned-item"];

#[test]
#[cfg(all(target_os = "macos", feature = "headless"))]
fn min_and_max_sizes_of_a_grid_item_are_as_in_chrome() {
    let data: serde_json::Value =
        serde_json::from_str(include_str!("../../../tools/parity_oracle/grid_item_min_max_cases.json"))
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
