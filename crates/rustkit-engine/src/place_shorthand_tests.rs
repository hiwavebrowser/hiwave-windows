//! `place-items`, `place-self` and `place-content` do what their two
//! longhands do.
//!
//! `tools/parity_oracle/place_shorthand_cases.json` holds each page twice,
//! once with the shorthand and once with it written out as its longhands,
//! and what the pinned Chromium reports for both: `id:x:y:width:height` of
//! every element with an id (`grid_flexible_row_log.mjs --file
//! place_shorthand_cases.json --write`). Chromium gives the two pages the
//! same boxes; so must the engine.
//!
//! None of the three shorthands was parsed. `display: grid; place-items:
//! center`, the usual way to centre a box, left its item stretched across
//! the cell.

use super::*;

/// The expression `grid_flexible_row_log.mjs` evaluates in Chromium
/// (`BOXES_XYWH`).
const BOXES: &str = "Array.prototype.map.call(document.querySelectorAll('[id]'), function (e) {\
    var r = e.getBoundingClientRect();\
    return e.id + ':' + Math.round(r.left) + ':' + Math.round(r.top) + ':' + Math.round(r.width) + ':' + Math.round(r.height);\
    }).join(' ')";

/// Pages where the engine gives the shorthand and the longhands the same
/// boxes, and those boxes are not Chromium's: the fault is in what the
/// longhand does, not in the shorthand. A page listed here that starts to
/// match Chromium fails the test, so the list cannot go stale.
///
/// Empty since the block axis of grid alignment was implemented
/// (`grid_block_axis_align_tests`); nine pages were listed until then.
const LONGHAND_GAPS: &[&str] = &[];

fn boxes(html: &str, w: u64, h: u64) -> String {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let id = engine
        .create_headless_view(Bounds::new(0, 0, w as u32, h as u32))
        .expect("headless view");
    engine.load_html(id, html).expect("load_html");
    let value = engine.execute_script(id, BOXES).expect("script");
    value
        .strip_prefix("String(\"")
        .and_then(|v| v.strip_suffix("\")"))
        .unwrap_or(&value)
        .to_string()
}

#[test]
#[cfg(all(target_os = "macos", feature = "headless"))]
fn a_place_shorthand_does_what_its_longhands_do() {
    let data: serde_json::Value =
        serde_json::from_str(include_str!("../../../tools/parity_oracle/place_shorthand_cases.json"))
            .expect("case file");
    let (w, h) = (data["viewport"][0].as_u64().unwrap(), data["viewport"][1].as_u64().unwrap());
    let mut differ = Vec::new();
    let mut not_chrome = Vec::new();
    let mut gaps_that_pass = Vec::new();
    for case in data["cases"].as_array().expect("cases") {
        let name = case["name"].as_str().unwrap();
        let chrome = case["chrome_boxes"].as_str().expect("run grid_flexible_row_log.mjs --write");
        assert_eq!(
            Some(chrome),
            case["chrome_boxes_longhand"].as_str(),
            "{name}: Chromium gives the shorthand and the longhands different boxes; the case is wrong"
        );
        let short = boxes(case["html"].as_str().unwrap(), w, h);
        let long = boxes(case["html_longhand"].as_str().unwrap(), w, h);
        if short != long {
            differ.push(format!("{name}\n   shorthand {short}\n   longhands {long}"));
        }
        match (short == chrome, LONGHAND_GAPS.contains(&name)) {
            (false, false) => not_chrome.push(format!("{name}\n   engine {short}\n   chrome {chrome}")),
            (true, true) => gaps_that_pass.push(name),
            _ => {}
        }
    }
    assert!(
        differ.is_empty(),
        "{} shorthands differ from their longhands:\n{}",
        differ.len(),
        differ.join("\n")
    );
    assert!(
        not_chrome.is_empty(),
        "{} differ from Chromium:\n{}",
        not_chrome.len(),
        not_chrome.join("\n")
    );
    assert!(
        gaps_that_pass.is_empty(),
        "listed as a gap but matches Chromium now, take it off the list: {gaps_that_pass:?}"
    );
}
