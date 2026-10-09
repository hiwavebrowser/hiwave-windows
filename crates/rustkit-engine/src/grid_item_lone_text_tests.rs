//! A grid item whose only child is text (or one inline box) that wraps.
//!
//! `tools/parity_oracle/grid_item_lone_text_cases.json` holds each page and
//! what the pinned Chromium reports for it: `id:x:y:width:height` of every
//! element with an id (`grid_flexible_row_log.mjs --file
//! grid_item_lone_text_cases.json --write`).
//!
//! The grid pass gave a lone text child the item's width without wrapping it
//! again, so `<div>` of a sentence in a 100px column was one line tall and
//! the row under it started 20px down, over the sentence.

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
/// All three have the right boxes for the grid and its items. What differs is
/// the rectangle script reads for the inline box that wraps: the engine
/// reports its first line (18 tall), Chromium the union of its five lines
/// (98).
const GAPS: &[&str] = &["a-span-that-wraps", "a-link-that-wraps", "a-list-of-links"];

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
fn a_lone_text_child_of_a_grid_item_wraps() {
    let data: serde_json::Value = serde_json::from_str(include_str!(
        "../../../tools/parity_oracle/grid_item_lone_text_cases.json"
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
