//! Z plan CH2.B10: chrome.html's downloads panel was 16px short in RustKit
//! (`#rightDownloadsPanel` 605 vs 621, `#rightDownloadsList` 569 vs 585 at
//! 1280x800). `.right-sidebar-content` is a flex column with `gap: 8px` whose
//! three panels are all `display: none` but the active one; the gap was
//! counted for the hidden panels too (two extra gaps = 16px), and the
//! `height: 100%` panel shrank to make room for them. The same empty box was
//! a grid item: a hidden child took an auto-placed cell.
//!
//! `tools/parity_oracle/flex_hidden_gap_cases.json` holds the reduced pages
//! and what Chromium 143.0.7499.4 reports for every element with an id
//! (`id:x:y:width:height`; `grid_flexible_row_log.mjs --file
//! flex_hidden_gap_cases.json --write`). No page has text.

#![cfg(all(target_os = "macos", feature = "headless"))]

use super::*;

/// `BOXES_XYWH` in `grid_flexible_row_log.mjs`.
const BOXES: &str = "Array.prototype.map.call(document.querySelectorAll('[id]'), function (e) {\
    var r = e.getBoundingClientRect();\
    return e.id + ':' + Math.round(r.left) + ':' + Math.round(r.top) + ':' + Math.round(r.width) + ':' + Math.round(r.height);\
    }).join(' ')";

#[test]
fn display_none_children_are_no_flex_or_grid_items_as_in_chrome() {
    let data: serde_json::Value =
        serde_json::from_str(include_str!("../../../tools/parity_oracle/flex_hidden_gap_cases.json"))
            .expect("case file");
    let (w, h) = (data["viewport"][0].as_u64().unwrap(), data["viewport"][1].as_u64().unwrap());
    let mut wrong = Vec::new();
    for case in data["cases"].as_array().expect("cases") {
        let name = case["name"].as_str().unwrap();
        let chrome = case["chrome_boxes"].as_str().expect("run grid_flexible_row_log.mjs --write");
        let mut engine = Engine::new(EngineConfig::default()).expect("engine");
        let id = engine
            .create_headless_view(Bounds::new(0, 0, w as u32, h as u32))
            .expect("headless view");
        engine.load_html(id, case["html"].as_str().unwrap()).expect("load_html");
        let got = engine.execute_script(id, BOXES).expect("script");
        let got = got
            .strip_prefix("String(\"")
            .and_then(|v| v.strip_suffix("\")"))
            .unwrap_or(&got)
            .to_string();
        if got != chrome {
            wrong.push(format!("{name}:\n  rustkit {got}\n  chrome  {chrome}"));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}
