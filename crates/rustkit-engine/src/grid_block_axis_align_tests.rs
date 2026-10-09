//! The block axis of grid alignment: `align-items`, `align-self` and
//! `align-content`.
//!
//! `tools/parity_oracle/grid_block_axis_align_cases.json` holds each page
//! and what the pinned Chromium reports for it: `id:x:y:width:height` of
//! every element with an id (`grid_flexible_row_log.mjs --file
//! grid_block_axis_align_cases.json --write`).
//!
//! An item whose `align-self` is not `stretch` is as tall as its content and
//! sits at the start, centre or end of its area. The engine gave every
//! auto-height item the whole area whatever its alignment, so `display:
//! grid; place-items: center` centred nothing on the block axis.
//! `align-content` moves or spreads the rows in the space a `height` or a
//! `min-height` leaves; a grid with a px height was laid out as if it were as
//! tall as its children stacked, so there was never any space.

use super::*;

/// The expression `grid_flexible_row_log.mjs` evaluates in Chromium
/// (`BOXES_XYWH`).
const BOXES: &str = "Array.prototype.map.call(document.querySelectorAll('[id]'), function (e) {\
    var r = e.getBoundingClientRect();\
    return e.id + ':' + Math.round(r.left) + ':' + Math.round(r.top) + ':' + Math.round(r.width) + ':' + Math.round(r.height);\
    }).join(' ')";

/// Pages the engine does not give Chromium's boxes. A page listed here that
/// starts to match Chromium fails the test, so the list cannot go stale.
///
/// - `baseline`: baseline alignment is not implemented; the item sits at the
///   start of its row, at its content height.
/// - `content-center-auto-rows-of-wrapped-paragraphs`: in a grid with a px
///   height the auto rows keep the track-sizing estimate (one line for the
///   paragraph that wraps to five); the repair by real heights only runs
///   for an auto-height grid.
/// - `hero-height-place-content-center`: the block axis is right; on the
///   inline axis `justify-content: center` does not shrink the auto column
///   to its content.
/// - `percent-height-content-center`: a percentage height reaches the grid
///   pass as the height of its children stacked, so there is no free space.
const GAPS: &[&str] = &[
    "baseline",
    "content-center-auto-rows-of-wrapped-paragraphs",
    "hero-height-place-content-center",
    "percent-height-content-center",
];

fn boxes(html: &str, w: u64, h: u64) -> String {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let id = engine
        .create_headless_view(Bounds::new(0, 0, w as u32, h as u32))
        .expect("headless view");
    engine.load_html(id, html).expect("load_html");
    let value = engine.execute_script(id, BOXES).expect("script");
    value
        .strip_prefix("String(\"")
        .and_then(|v| v.strip_suffix("\")"))
        .unwrap_or(&value)
        .to_string()
}

#[test]
#[cfg(all(target_os = "macos", feature = "headless"))]
fn grid_items_and_rows_are_aligned_on_the_block_axis() {
    let data: serde_json::Value = serde_json::from_str(include_str!(
        "../../../tools/parity_oracle/grid_block_axis_align_cases.json"
    ))
    .expect("case file");
    let (w, h) = (data["viewport"][0].as_u64().unwrap(), data["viewport"][1].as_u64().unwrap());
    let cases = data["cases"].as_array().expect("cases");
    let mut not_chrome = Vec::new();
    let mut gaps_that_pass = Vec::new();
    for case in cases {
        let name = case["name"].as_str().unwrap();
        let chrome = case["chrome_boxes"].as_str().expect("run grid_flexible_row_log.mjs --write");
        let engine = boxes(case["html"].as_str().unwrap(), w, h);
        match (engine == chrome, GAPS.contains(&name)) {
            (false, false) => not_chrome.push(format!("{name}\n   engine {engine}\n   chrome {chrome}")),
            (true, true) => gaps_that_pass.push(name),
            _ => {}
        }
    }
    assert!(
        not_chrome.is_empty(),
        "{} of {} differ from Chromium:\n{}",
        not_chrome.len(),
        cases.len(),
        not_chrome.join("\n")
    );
    assert!(
        gaps_that_pass.is_empty(),
        "listed as a gap but matches Chromium now, take it off the list: {gaps_that_pass:?}"
    );
}
