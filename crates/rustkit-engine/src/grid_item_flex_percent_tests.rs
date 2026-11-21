//! A flex container that is a grid item has a definite height (its grid
//! area, css-grid-1 §11.6, or its own percentage of it), so a
//! `height: 100%` flex item in it resolves against that height. The
//! engine's block pre-pass had marked the item's percentage `auto`, and the
//! mark outlived the grid's relayout of the container: the item was sized
//! by its content. On a watch maker's product page the image pane
//! (`height: 100%; min-height: 0; overflow: hidden`) of a sticky two-column
//! grid came out 204 tall where Chrome has 800, and showed a slice of the
//! product image.
//!
//! `tools/parity_oracle/grid_item_flex_percent_cases.json` holds the pages
//! and what the pinned Chromium reports for every element with an id
//! (`grid_flexible_row_log.mjs --file grid_item_flex_percent_cases.json
//! --write`; Chrome for Testing 148.0.7778.216 gives the same boxes).

use super::*;

/// The expression `grid_flexible_row_log.mjs` evaluates in Chromium
/// (`BOXES_XYWH`).
const BOXES: &str = "Array.prototype.map.call(document.querySelectorAll('[id]'), function (e) {\
    var r = e.getBoundingClientRect();\
    return e.id + ':' + Math.round(r.left) + ':' + Math.round(r.top) + ':' + Math.round(r.width) + ':' + Math.round(r.height);\
    }).join(' ')";

/// Shapes that do not match Chrome yet, each with its reason. A shape listed
/// here that starts to match fails the test, so the list cannot go stale.
const GAPS: &[&str] = &[
    // Pre-existing on develop a9db07bb (not row sizing): the item's own
    // block flow resolves `min-height: calc(50% + 0px)` against the 600px
    // grid, 300, and Phase 9.5 grows the row to it. Chrome 148 treats the
    // percentage as cyclic while rows are sized: row 18, item 18.
    "a-percent-calc-min-height-item-in-the-auto-row-of-a-definite-grid",
];

#[test]
#[cfg(all(target_os = "macos", feature = "headless"))]
fn a_percent_height_flex_item_of_a_grid_item_is_as_in_chrome() {
    let data: serde_json::Value = serde_json::from_str(include_str!(
        "../../../tools/parity_oracle/grid_item_flex_percent_cases.json"
    ))
    .expect("case file");
    let (w, h) = (
        data["viewport"][0].as_u64().unwrap(),
        data["viewport"][1].as_u64().unwrap(),
    );
    let mut wrong = Vec::new();
    let mut gaps_that_pass = Vec::new();
    for case in data["cases"].as_array().expect("cases") {
        let name = case["name"].as_str().unwrap();
        let chrome = case["chrome_boxes"]
            .as_str()
            .expect("run grid_flexible_row_log.mjs --write");

        let mut engine = Engine::new(EngineConfig::default()).expect("engine");
        let id = engine
            .create_headless_view(Bounds::new(0, 0, w as u32, h as u32))
            .expect("headless view");
        engine
            .load_html(id, case["html"].as_str().unwrap())
            .expect("load_html");
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
    assert!(
        wrong.is_empty(),
        "{} wrong:\n{}",
        wrong.len(),
        wrong.join("\n")
    );
    assert!(
        gaps_that_pass.is_empty(),
        "listed as a gap but matches Chrome now, take it off the list: {gaps_that_pass:?}"
    );
}
