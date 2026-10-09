//! The absolutely positioned children of a positioned grid item, against the
//! pinned Chromium (issue #560).
//!
//! `tools/parity_oracle/grid_item_abspos_cases.json` holds small pages and,
//! for each, what the pinned Chromium reports for every element with an id:
//! `id:x:y:width:height` of its border box (`grid_flexible_row_log.mjs --file
//! grid_item_abspos_cases.json --write`). The engine must give the same
//! string.
//!
//! The first page is a card with a whole-card link: a `position: relative`
//! grid item holding text and an `inset: 0` `<a>`. The grid pass settles the
//! item's box after its children are flowed, and the link kept the box it
//! was given against the flow cursor: 0px tall in a 100px card.

use super::*;

/// The expression `grid_flexible_row_log.mjs` evaluates in Chromium
/// (`BOXES_XYWH`).
const BOXES: &str = "Array.prototype.map.call(document.querySelectorAll('[id]'), function (e) {\
    var r = e.getBoundingClientRect();\
    return e.id + ':' + Math.round(r.left) + ':' + Math.round(r.top) + ':' + Math.round(r.width) + ':' + Math.round(r.height);\
    }).join(' ')";

/// Shapes that do not match Chrome yet, each with its reason. A shape listed
/// here that starts to match fails the test, so the list cannot go stale.
///
/// `a-static-item-in-a-relative-grid`, `a-static-centred-item-in-a-relative-grid`:
/// the containing block is the grid, two boxes up. Layout anchors an abspos
/// box to its parent only, so a static item's abspos child keeps the flow
/// position (not a grid fault: the same in a block).
///
/// `issue-560-a-whole-card-link-over-text`,
/// `issue-560-a-centred-card-with-a-whole-card-link`: the overlay is an `<a>`
/// with no content. Box construction does not make an absolutely positioned
/// inline a block (CSS 2.1 section 9.7), and an inline with no content gets
/// no box at all, so script reads 0:0:0:0. The same pages with a `<div>`
/// overlay are in this file and match.
const GAPS: &[&str] = &[
    "a-static-item-in-a-relative-grid",
    "a-static-centred-item-in-a-relative-grid",
    "issue-560-a-whole-card-link-over-text",
    "issue-560-a-centred-card-with-a-whole-card-link",
];

#[test]
#[cfg(all(target_os = "macos", feature = "headless"))]
fn abspos_children_of_a_grid_item_are_as_in_chrome() {
    let data: serde_json::Value =
        serde_json::from_str(include_str!("../../../tools/parity_oracle/grid_item_abspos_cases.json"))
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
