//! Z plan CH2.B18 item (1): a border side with a style and no width is
//! `medium` (3px), as in Chromium; `thin`/`medium`/`thick` are widths.
//!
//! `border-width`'s initial value is `medium` (CSS Backgrounds 3 §3.3); it
//! computes to 0 only where the style is `none`/`hidden`. RustKit's initial
//! `BorderStyle` is `Solid` (so a bare `border: 2px` paints), so an unset
//! width cannot start at 3px; an explicit style on a side whose width is
//! still unset (`Length::Zero`; an authored `0` is `Px(0.0)`) takes medium.
//! Not on input/textarea/button/select, whose UA border lives in layout
//! (R1 hold): there `border-style` keeps the UA width, as in Chromium.
//!
//! `tools/parity_oracle/border_medium_cases.json`, Chromium 143.0.7499.4
//! (`grid_flexible_row_log.mjs --file border_medium_cases.json --write`).

#![cfg(all(target_os = "macos", feature = "headless"))]

use super::*;

/// `BOXES_XYWH` in `grid_flexible_row_log.mjs`.
const BOXES: &str = "Array.prototype.map.call(document.querySelectorAll('[id]'), function (e) {\
    var r = e.getBoundingClientRect();\
    return e.id + ':' + Math.round(r.left) + ':' + Math.round(r.top) + ':' + Math.round(r.width) + ':' + Math.round(r.height);\
    }).join(' ')";

#[test]
fn a_border_style_without_a_width_is_medium_as_in_chrome() {
    let data: serde_json::Value = serde_json::from_str(include_str!(
        "../../../tools/parity_oracle/border_medium_cases.json"
    ))
    .expect("case file");
    let (w, h) = (data["viewport"][0].as_u64().unwrap(), data["viewport"][1].as_u64().unwrap());
    let mut wrong = Vec::new();
    let cases = data["cases"].as_array().expect("cases");
    for case in cases {
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
    assert!(wrong.is_empty(), "{} of {} cases differ from Chromium 143:\n{}", wrong.len(), cases.len(), wrong.join("\n"));
}
