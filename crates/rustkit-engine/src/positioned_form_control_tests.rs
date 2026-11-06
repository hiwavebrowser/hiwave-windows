//! Positioned form controls are placed by their offsets, against the pinned
//! Chromium (#730 follow-up i; Handtest's k7 fixture: a positioned `<input>`
//! was neither painted nor focused where the page put it).
//!
//! `tools/parity_oracle/positioned_form_control_cases.json` holds small pages
//! and, for each, what the pinned Chromium reports for every element with an
//! id: `id:x:y` of its border box (`grid_flexible_row_log.mjs --file
//! positioned_form_control_cases.json --write`). Only the place is compared;
//! control sizes are their own work.
//!
//! Form controls leave box construction early and never had position or
//! offsets copied from their style, so `position: absolute` / `fixed` on an
//! `<input>`, `<textarea>`, `<select>` or a text `<button>` was ignored: the
//! control stayed in the flow, and paint and hit testing used that box.

use super::*;

/// The expression `grid_flexible_row_log.mjs` evaluates in Chromium
/// (`BOXES_XY`).
const BOXES: &str = "Array.prototype.map.call(document.querySelectorAll('[id]'), function (e) {\
    var r = e.getBoundingClientRect();\
    return e.id + ':' + Math.round(r.left) + ':' + Math.round(r.top);\
    }).join(' ')";

/// Shapes that do not match Chrome yet, each with its reason. A shape listed
/// here that starts to match fails the test, so the list cannot go stale.
///
/// `an-absolute-checkbox`: the UA sheet has no margin for checkbox and radio
/// (Chrome: `margin: 3px 3px 3px 4px`), so the box is 4,3 short of Chrome's
/// with or without positioning. The same page with `margin: 0`
/// (`an-absolute-checkbox-with-no-margin`) matches.
const GAPS: &[&str] = &["an-absolute-checkbox"];

#[test]
#[cfg(all(target_os = "macos", feature = "headless"))]
fn positioned_form_controls_are_placed_as_in_chrome() {
    let data: serde_json::Value = serde_json::from_str(include_str!(
        "../../../tools/parity_oracle/positioned_form_control_cases.json"
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

/// A click where the page put a positioned field focuses that field (the k7
/// report: the field was neither drawn nor focused there, while keys still
/// went to the page).
///
/// The body is as tall as the view here. GAP, not this PR's: the hit test
/// stops at any box whose border box does not hold the point
/// (`LayoutBox::hit_test_internal`), so an absolute box that sits outside
/// all of its ancestors' boxes (under a 20px body) is drawn but not hit.
/// Fixed boxes are collected apart and are hit either way.
#[test]
#[cfg(all(target_os = "macos", feature = "headless"))]
fn a_click_on_a_positioned_field_focuses_it() {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine.create_headless_view(Bounds::new(0, 0, 800, 600)).expect("headless view");
    engine
        .load_html(
            view,
            concat!(
                "<!doctype html><body style='margin:0;min-height:600px'><p style=margin:0>flow text</p>",
                "<input id=abs style='position:absolute;top:200px;left:300px;width:160px'>",
                "<input id=fix style='position:fixed;top:400px;left:100px;width:160px'></body>",
            ),
        )
        .expect("load_html");
    assert_eq!(engine.focus_at_point(view, 320.0, 210.0).as_deref(), Some("input"), "the absolute field");
    let active = engine.execute_script(view, "document.activeElement.id").expect("script");
    assert!(active.contains("abs"), "focused {active}");
    assert_eq!(engine.focus_at_point(view, 120.0, 410.0).as_deref(), Some("input"), "the fixed field");
    let active = engine.execute_script(view, "document.activeElement.id").expect("script");
    assert!(active.contains("fix"), "focused {active}");
    // Where the fields sat in the flow before the fix: nothing focusable.
    assert_eq!(engine.focus_at_point(view, 5.0, 30.0), None, "the old in-flow place");
}
