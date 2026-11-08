//! Z plan CH2.B13 (A + B): form controls sized by lengths the control path
//! did not resolve. (A) a `position: fixed` control's percentage height and
//! width resolve against the viewport (CSS 2.1 §10.1), not the flow cursor
//! of the box that laid it out; an absolutely positioned one against its
//! containing block once that is known. (B) `vh`, `rem`, `calc()` heights
//! and widths fell back to the intrinsic size in any position. Found by the
//! E12 agent (a fixed `<input>` with `height: 50%`: Chromium 348, RustKit
//! 34.2) and reduced by Reduce.
//!
//! Text inputs and textareas carry `box-sizing: border-box` here: Chrome's
//! UA sheet sizes them content-box and RustKit applies the author size as
//! the border box, which is CH2.B17, not this row.
//!
//! `tools/parity_oracle/control_percent_height_cases.json` holds the pages
//! and what Chromium 143.0.7499.4 reports for every element with an id
//! (`id:x:y:width:height`; `grid_flexible_row_log.mjs --file
//! control_percent_height_cases.json --write`).

#![cfg(all(target_os = "macos", feature = "headless"))]

use super::*;

/// `BOXES_XYWH` in `grid_flexible_row_log.mjs`.
const BOXES: &str = "Array.prototype.map.call(document.querySelectorAll('[id]'), function (e) {\
    var r = e.getBoundingClientRect();\
    return e.id + ':' + Math.round(r.left) + ':' + Math.round(r.top) + ':' + Math.round(r.width) + ':' + Math.round(r.height);\
    }).join(' ')";

#[test]
fn form_control_percent_viewport_and_calc_sizes_are_as_in_chrome() {
    let data: serde_json::Value = serde_json::from_str(include_str!(
        "../../../tools/parity_oracle/control_percent_height_cases.json"
    ))
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
