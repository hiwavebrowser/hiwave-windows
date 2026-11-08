//! An author `width`/`height` on a text `<input>` or a `<textarea>` sizes its
//! CONTENT box, as Chrome's UA sheet has it (`box-sizing: content-box`), so
//! the border box adds the control's padding and border (CH2.B17; Reduce
//! 11-05: `width: 40px` built 40 where Chrome builds 48 / 46).
//!
//! `tools/parity_oracle/form_control_author_size_cases.json` holds small
//! pages and, for each, what the pinned Chromium reports for every element
//! with an id: `id:x:y:width:height` of its border box
//! (`grid_flexible_row_log.mjs --file form_control_author_size_cases.json
//! --write`). Chrome's UA padding and border: text input `1px 2px` + `2px`,
//! textarea `2px` + `1px`; `type=search` is `border-box` in Chrome's UA.

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
fn author_sizes_on_text_controls_are_content_box_as_in_chrome() {
    let data: serde_json::Value = serde_json::from_str(include_str!(
        "../../../tools/parity_oracle/form_control_author_size_cases.json"
    ))
    .expect("case file");
    let (w, h) = (data["viewport"][0].as_u64().unwrap(), data["viewport"][1].as_u64().unwrap());
    let mut wrong = Vec::new();
    let mut gaps_that_pass = Vec::new();
    for case in data["cases"].as_array().expect("cases") {
        let name = case["name"].as_str().unwrap();
        let chrome = case["chrome_boxes"].as_str().expect("run grid_flexible_row_log.mjs --write");

        let mut engine = Engine::new(EngineConfig::default()).expect("engine");
        let id = engine
            .create_headless_view(Bounds::new(0, 0, w as u32, h as u32))
            .expect("headless view");
        engine.load_html(id, case["html"].as_str().unwrap()).expect("load_html");
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
    assert!(wrong.is_empty(), "{} wrong:\n{}", wrong.len(), wrong.join("\n"));
    assert!(
        gaps_that_pass.is_empty(),
        "listed as a gap but matches Chrome now, take it off the list: {gaps_that_pass:?}"
    );
}
