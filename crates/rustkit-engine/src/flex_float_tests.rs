//! A floated flex container is a flex container. `layout_float` re-flowed
//! the children with the block pass after the flex pass, so a
//! `float: left; display: flex` row stacked its items at x 0, one under the
//! other (found under GR1, #1075, whose fix does the same for a floated
//! grid).
//!
//! `tools/parity_oracle/flex_float_cases.json` holds the pages and what the
//! pinned Chromium reports for every element with an id
//! (`grid_flexible_row_log.mjs --file flex_float_cases.json --write`;
//! Chrome for Testing 148.0.7778.216 gives the same boxes): left and right
//! floats, a shrink-to-fit width around an auto item and a gap, `flex-wrap`
//! under a `max-width`, a column (the control), `justify-content` in a
//! float with a width, `align-items`, a floated flex container inside a
//! flex item, and text flowing beside one.

use super::*;

/// The expression `grid_flexible_row_log.mjs` evaluates (`BOXES_XYWH`).
const BOXES: &str = "Array.prototype.map.call(document.querySelectorAll('[id]'), function (e) {\
    var r = e.getBoundingClientRect();\
    return e.id + ':' + Math.round(r.left) + ':' + Math.round(r.top) + ':' + Math.round(r.width) + ':' + Math.round(r.height);\
    }).join(' ')";

/// Shapes that do not match Chrome yet, each with its reason. A shape listed
/// here that starts to match fails the test, so the list cannot go stale.
const GAPS: &[&str] = &[];

#[test]
#[cfg(all(target_os = "macos", feature = "headless"))]
fn a_floated_flex_container_is_laid_out_as_a_flex_container_as_in_chrome() {
    let data: serde_json::Value = serde_json::from_str(include_str!(
        "../../../tools/parity_oracle/flex_float_cases.json"
    ))
    .expect("case file");
    let (w, h) = (
        data["viewport"][0].as_u64().unwrap(),
        data["viewport"][1].as_u64().unwrap(),
    );
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let id = engine
        .create_headless_view(Bounds::new(0, 0, w as u32, h as u32))
        .expect("headless view");
    let mut wrong = Vec::new();
    let mut gaps_that_pass = Vec::new();
    for case in data["cases"].as_array().expect("cases") {
        let name = case["name"].as_str().unwrap();
        let chrome = case["chrome_boxes"]
            .as_str()
            .expect("run grid_flexible_row_log.mjs --write");
        engine
            .load_html(id, case["html"].as_str().unwrap())
            .expect("load_html");
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
    assert!(
        wrong.is_empty(),
        "{} wrong:\n{}",
        wrong.len(),
        wrong.join("\n")
    );
    assert!(
        gaps_that_pass.is_empty(),
        "listed as a gap but matches Chrome now, take it off the list: {gaps_that_pass:?}"
    );
}
