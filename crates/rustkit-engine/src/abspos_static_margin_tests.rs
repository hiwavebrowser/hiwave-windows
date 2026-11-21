//! Z plan CH2.B24: an absolutely positioned box's static position comes
//! after the previous in-flow sibling's bottom margin. From #970's WPT
//! clip-path/mask reference pages (`<p>text</p><div style="position:
//! absolute">`): the engine dropped the `<p>`'s 16px bottom margin, so the
//! box sat 16px too high and every reference page with that shape read ~1%
//! off.
//!
//! `tools/parity_oracle/abspos_static_margin_cases.json` holds the pages and
//! what Chromium 148.0.7778.216 reports for every element with an id
//! (`id:x:y:width:height`; `grid_flexible_row_log.mjs --file
//! abspos_static_margin_cases.json --write`).

#![cfg(all(target_os = "macos", feature = "headless"))]

use super::*;

/// `BOXES_XYWH` in `grid_flexible_row_log.mjs`.
const BOXES: &str = "Array.prototype.map.call(document.querySelectorAll('[id]'), function (e) {\
    var r = e.getBoundingClientRect();\
    return e.id + ':' + Math.round(r.left) + ':' + Math.round(r.top) + ':' + Math.round(r.width) + ':' + Math.round(r.height);\
    }).join(' ')";

#[test]
fn abspos_static_position_follows_the_previous_bottom_margin_as_in_chrome() {
    let data: serde_json::Value = serde_json::from_str(include_str!(
        "../../../tools/parity_oracle/abspos_static_margin_cases.json"
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
    assert!(wrong.is_empty(), "{} of {} wrong\n{}", wrong.len(), data["cases"].as_array().unwrap().len(), wrong.join("\n"));
}
