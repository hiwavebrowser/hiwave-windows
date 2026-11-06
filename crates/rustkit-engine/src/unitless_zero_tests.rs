//! Z plan CH2.B14: a unitless `0` must lay out exactly as `0px`.
//!
//! `rustkit_css::parse_length("0")` returns `Length::Zero`, which is also the
//! enum's default, the value an unset property holds. Code that matches only
//! `Length::Px` (or treats `Zero` as "not set") reads an author's bare `0` as
//! `auto`, `none` or `normal`. #913 fixed width/height/min-*/max-*; this file
//! audits the other properties that accept a length.
//!
//! `tools/parity_oracle/unitless_zero_cases.json` holds every page twice,
//! once with `0` and once with `0px` (the `-px` twin). Chromium 143.0.7499.4
//! reports identical boxes for every pair (`id:x:y:width:height`;
//! `grid_flexible_row_log.mjs --file unitless_zero_cases.json --write`).

#![cfg(all(target_os = "macos", feature = "headless"))]

use super::*;

/// `BOXES_XYWH` in `grid_flexible_row_log.mjs`.
const BOXES: &str = "Array.prototype.map.call(document.querySelectorAll('[id]'), function (e) {\
    var r = e.getBoundingClientRect();\
    return e.id + ':' + Math.round(r.left) + ':' + Math.round(r.top) + ':' + Math.round(r.width) + ':' + Math.round(r.height);\
    }).join(' ')";

#[test]
fn unitless_zero_lays_out_as_zero_px_as_in_chrome() {
    let data: serde_json::Value = serde_json::from_str(include_str!(
        "../../../tools/parity_oracle/unitless_zero_cases.json"
    ))
    .expect("case file");
    let (w, h) = (data["viewport"][0].as_u64().unwrap(), data["viewport"][1].as_u64().unwrap());
    let mut wrong = Vec::new();
    let mut got_by_name = std::collections::HashMap::new();
    for case in data["cases"].as_array().expect("cases") {
        let name = case["name"].as_str().unwrap();
        let chrome = case["chrome_boxes"].as_str().expect("run grid_flexible_row_log.mjs --write");
        // A pair whose `0px` twin is also wrong is another defect (named in
        // "known"); it still has to give `0` and `0px` the same boxes.
        let known = case.get("known").is_some();
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
        if got != chrome && !known {
            wrong.push(format!("{name}:\n  rustkit {got}\n  chrome  {chrome}"));
        }
        got_by_name.insert(name.to_string(), got);
    }
    for (name, got) in &got_by_name {
        if let Some(px) = name.strip_suffix("-px") {
            let zero = &got_by_name[px];
            if zero != got {
                wrong.push(format!("{px}: `0` and `0px` differ\n  0   {zero}\n  0px {got}"));
            }
        }
    }
    wrong.sort();
    assert!(wrong.is_empty(), "{} of {} wrong\n{}", wrong.len(), data["cases"].as_array().unwrap().len(), wrong.join("\n"));
}

/// getComputedStyle of the gap properties: an authored `0` is `0px`, and only
/// an unset gap is `normal`. Chromium 143.0.7499.4 gives the same string for
/// `0` and `0px` (in a flex container and in a multi-column one).
#[test]
fn unitless_zero_gap_reads_as_zero_px_not_normal() {
    const READ: &str = "(function () {\
        var t = getComputedStyle(document.getElementById('t'));\
        var m = getComputedStyle(document.getElementById('m'));\
        return [t.getPropertyValue('column-gap'), t.getPropertyValue('row-gap'),\
            m.getPropertyValue('column-gap')].join(' ');\
        })()";
    let mut wrong = Vec::new();
    for (zero, chrome) in [("0", "0px 0px 0px"), ("0px", "0px 0px 0px"), ("", "normal normal normal")] {
        let decl = if zero.is_empty() { String::new() } else { format!("gap:{zero};column-gap:{zero}") };
        let html = format!(
            "<!doctype html><div id=t style=\"display:flex;{decl}\">x</div>\
             <div id=m style=\"column-count:2;{decl}\">y</div>"
        );
        let mut engine = Engine::new(EngineConfig::default()).expect("engine");
        let id = engine.create_headless_view(Bounds::new(0, 0, 400, 300)).expect("headless view");
        engine.load_html(id, &html).expect("load_html");
        let got = engine.execute_script(id, READ).expect("script");
        let got = got.strip_prefix("String(\"").and_then(|v| v.strip_suffix("\")")).unwrap_or(&got).to_string();
        if got != chrome {
            wrong.push(format!("gap {zero:?}: rustkit {got:?}, chrome {chrome:?}"));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}
