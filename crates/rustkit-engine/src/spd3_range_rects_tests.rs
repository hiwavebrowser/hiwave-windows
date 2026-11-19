//! SPD3 (Speedometer editors): the geometry of a Range, per character.
//!
//! CodeMirror and TipTap measure text with `Range.getClientRects()` and
//! `getBoundingClientRect()`. The engine answered the union of the boxes
//! of the elements the range's ends sit in: a three-character range in a
//! line read as the whole paragraph. The rects are now those of CSSOM View
//! §10.1: each element the range selects whose parent it does not, and the
//! selected part of each text node, per line, from layout's own character
//! positions.
//!
//! Also `InputEvent.getTargetRanges()`, `StaticRange` and
//! `AbstractRange`, which the editors' `beforeinput` handling reads.
//!
//! Expected lines: docs/diagnostics/2026-11-12/spd3_range_rects_cft148.txt,
//! `window.report()` of the page beside it in Chrome for Testing
//! 148.0.7778.216 at 800x600 (Menlo 16px/20px, so every advance is 9.63px).

use super::*;

const PAGE: &str = include_str!("../../../docs/diagnostics/2026-11-12/spd3_range_rects.html");
const CHROME: &str = include_str!("../../../docs/diagnostics/2026-11-12/spd3_range_rects_cft148.txt");

/// Lines still off, for a reason outside this row: the image in `#mixed`
/// is `vertical-align: top` and sits 5px low here (y 115 for Chrome's
/// 110), which is the element's own rect, not the range's. They must
/// still DIFFER: one that matches comes off the list.
///
/// `tall across sizes` is off by 1/64 px in its last two rects: Chrome
/// lays inline boxes out on its 1/64 px grid, so the box after the 24px
/// span starts at the sum of two rounded lengths (111.53), where the
/// layout here keeps the unrounded sum (111.51).
const KNOWN: &[&str] = &[
    "mixed all rects: ",
    "mixed image bounding: ",
    "mixed image rects: ",
    "tall across sizes bounding: ",
    "tall across sizes rects: ",
];

#[test]
fn a_range_reports_the_rects_of_its_characters() {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 800, height: 600 })
        .expect("view");
    engine.load_html(view, PAGE).expect("load");
    let got = engine
        .execute_script(view, "JSON.stringify(window.report().split('\\n'))")
        .expect("report");
    let got = got.strip_prefix("String(").and_then(|v| v.strip_suffix(')')).unwrap_or(&got).to_string();
    let got: String = serde_json::from_str(&got).expect("a JSON string");
    let got: Vec<String> = serde_json::from_str(&got).expect("a JSON array");
    let expected: Vec<&str> = CHROME.lines().filter(|l| !l.starts_with('#') && !l.is_empty()).collect();
    assert_eq!(expected.len(), 67, "the Chrome report has 67 lines");
    let mut wrong = Vec::new();
    for (i, want) in expected.iter().enumerate() {
        let listed = KNOWN.iter().any(|k| want.starts_with(k));
        match got.get(i) {
            Some(have) if (have == want) != listed => {}
            Some(have) if listed => wrong.push(format!("  {have}\n    matches Chromium now: take it off KNOWN")),
            Some(have) => wrong.push(format!("  {have}\n    Chromium: {want}")),
            None => wrong.push(format!("  (missing)\n    Chromium: {want}")),
        }
    }
    assert!(wrong.is_empty(), "{} of {} report lines differ:\n{}", wrong.len(), expected.len(), wrong.join("\n"));
}
