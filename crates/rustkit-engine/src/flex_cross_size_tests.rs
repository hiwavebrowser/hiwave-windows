//! Z plan CH2.B9 items (2) and (3): the chrome tab's close button sat 4 px
//! low in RustKit. Reduce found two flex bugs on the tab bar:
//! (2) a flex container that is itself a flex item was 2 px too tall (30 vs
//! Chromium 28: its cross size came out one text line, not its items);
//! (3) `align-items: center` on an item taller than its line kept the item
//! at the start (safe alignment); flexbox `center` is unsafe by default and
//! overflows both sides (CSS Box Alignment 3 §4.4, CSS Flexbox 1 §8.3).
//!
//! `tools/parity_oracle/flex_cross_size_cases.json` holds the reduced pages
//! and what Chromium 143.0.7499.4 reports for every element with an id
//! (`id:x:y:width:height`; `grid_flexible_row_log.mjs --file
//! flex_cross_size_cases.json --write`).

#![cfg(all(target_os = "macos", feature = "headless"))]

use super::*;

/// `BOXES_XYWH` in `grid_flexible_row_log.mjs`.
const BOXES: &str = "Array.prototype.map.call(document.querySelectorAll('[id]'), function (e) {\
    var r = e.getBoundingClientRect();\
    return e.id + ':' + Math.round(r.left) + ':' + Math.round(r.top) + ':' + Math.round(r.width) + ':' + Math.round(r.height);\
    }).join(' ')";

#[test]
fn flex_item_cross_size_and_unsafe_center_are_as_in_chrome() {
    let data: serde_json::Value =
        serde_json::from_str(include_str!("../../../tools/parity_oracle/flex_cross_size_cases.json"))
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
