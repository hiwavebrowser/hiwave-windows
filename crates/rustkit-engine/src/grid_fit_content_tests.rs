//! A `fit-content(L)` grid track's growth limit is the larger of its base
//! size and min(L, its items' max-content contribution) (css-grid-1 §12.5,
//! §7.2.4), and it takes part in maximize tracks (§12.6) like any track
//! with a finite limit. The engine took L itself as the limit, so GR1 (#1075)
//! kept these tracks out of the pass before the `fr` tracks: beside an
//! `fr` track they stayed at their min-content, and beside a fixed track
//! they grew to L.
//!
//! `tools/parity_oracle/grid_fit_content_cases.json` holds the pages and what
//! the pinned Chromium reports for every element with an id
//! (`grid_flexible_row_log.mjs --file grid_fit_content_cases.json --write`;
//! Chrome for Testing 148.0.7778.216 gives the same boxes). The items are
//! 60px inline-blocks in `font-size: 0` boxes, so min-content is 60 and
//! max-content is their sum, with no font in the answer.

use super::*;

/// The expression `grid_flexible_row_log.mjs` evaluates (`BOXES_XYWH`).
const BOXES: &str = "Array.prototype.map.call(document.querySelectorAll('[id]'), function (e) {\
    var r = e.getBoundingClientRect();\
    return e.id + ':' + Math.round(r.left) + ':' + Math.round(r.top) + ':' + Math.round(r.width) + ':' + Math.round(r.height);\
    }).join(' ')";

/// Shapes that do not match Chrome yet, each with its reason. A shape listed
/// here that starts to match fails the test, so the list cannot go stale.
const GAPS: &[&str] = &[
    // A header across `fit-content(300px) auto`: Chromium gives the auto
    // column the header's 360 and the fit-content one 0. The engine grows
    // neither: a spanning item's max-content does not reach an `auto`
    // column's size, with or without a fit-content column beside it, so
    // the header wraps at 60. Not a fit-content bug; the page is here for
    // the day spanning items size intrinsic columns.
    "a-header-spanning-a-fit-content-and-an-auto-column",
];

#[test]
#[cfg(all(target_os = "macos", feature = "headless"))]
fn a_fit_content_track_grows_to_its_max_content_as_in_chrome() {
    let data: serde_json::Value = serde_json::from_str(include_str!(
        "../../../tools/parity_oracle/grid_fit_content_cases.json"
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
