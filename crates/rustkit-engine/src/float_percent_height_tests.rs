//! Z plan CH2.B16: a float's percentage height resolves against its
//! containing block's definite height, not the viewport. Reduce (11-05): a
//! float with `height: 50%` whose parent is `height: 200px` and has only float
//! children came out 300 (half the 600px viewport), Chromium 100.
//!
//! `tools/parity_oracle/float_percent_height_cases.json` holds the pages and
//! what Chromium 143.0.7499.4 reports for every element with an id
//! (`id:x:y:width:height`; `grid_flexible_row_log.mjs --file
//! float_percent_height_cases.json --write`).

#![cfg(all(target_os = "macos", feature = "headless"))]

use super::*;

/// `BOXES_XYWH` in `grid_flexible_row_log.mjs`.
const BOXES: &str = "Array.prototype.map.call(document.querySelectorAll('[id]'), function (e) {\
    var r = e.getBoundingClientRect();\
    return e.id + ':' + Math.round(r.left) + ':' + Math.round(r.top) + ':' + Math.round(r.width) + ':' + Math.round(r.height);\
    }).join(' ')";

#[test]
fn float_percent_height_resolves_against_the_parent_as_in_chrome() {
    let data: serde_json::Value = serde_json::from_str(include_str!(
        "../../../tools/parity_oracle/float_percent_height_cases.json"
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
        // "known": wrong for every box, not floats (named in the case file);
        // it must flip the day that is fixed, so a pass is reported too.
        let known = case.get("known").is_some();
        if (got != chrome) != known {
            let why = if known { "known case now right, drop the mark" } else { "wrong" };
            wrong.push(format!("{name} ({why}):\n  rustkit {got}\n  chrome  {chrome}"));
        }
    }
    assert!(wrong.is_empty(), "{} of {} wrong\n{}", wrong.len(), data["cases"].as_array().unwrap().len(), wrong.join("\n"));
}
