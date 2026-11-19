//! A grid item's `height` in viewport or font units (`100vh`, `20rem`,
//! `min(100vh, 500px)`) is its own height, not the row's. The engine's
//! item placement resolved only `px`, `%` and `calc()` and gave every other
//! unit the cell's height, so a sticky `100vh` pane beside a 3000px column
//! (a common product and landing page shape) was 3000 tall where Chrome has
//! the viewport's 800.
//!
//! `tools/parity_oracle/grid_item_viewport_height_cases.json` holds the
//! pages and what the pinned Chromium reports for every element with an id
//! (`grid_flexible_row_log.mjs --file grid_item_viewport_height_cases.json
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
const GAPS: &[&str] = &[];

#[test]
#[cfg(all(target_os = "macos", feature = "headless"))]
fn a_grid_item_with_a_viewport_or_font_height_keeps_it_as_in_chrome() {
    let data: serde_json::Value = serde_json::from_str(include_str!(
        "../../../tools/parity_oracle/grid_item_viewport_height_cases.json"
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
