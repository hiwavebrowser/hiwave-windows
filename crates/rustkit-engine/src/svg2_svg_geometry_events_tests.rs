//! SVG2 (Speedometer React-Stockcharts): where an SVG element is, for
//! script, and the fields of mouse and wheel events.
//!
//! * `getBoundingClientRect()` on a shape or a group was 0,0,0,0;
//!   `getCTM`, `getScreenCTM`, `createSVGPoint` / `matrixTransform`,
//!   `ownerSVGElement` and `viewportElement` were missing; and
//!   `elementFromPoint` (and the engine's own mouse events) answered the
//!   `<svg>`, not the shape under the point.
//! * `offsetX` / `offsetY` read the client point, `layerX` / `layerY` and a
//!   constructed event's `movementX` / `movementY` were missing, and a
//!   WheelEvent had no `wheelDelta`, `wheelDeltaX`, `wheelDeltaY` or mode
//!   constants.
//!
//! Expected lines: docs/diagnostics/2026-11-12/
//! svg2_svg_geometry_mouse_fields_cft148.txt, `window.report()` of the page
//! beside it in Chrome for Testing 148.0.7778.216 after the same pointer
//! input (Playwright).

use super::*;

const PAGE: &str = include_str!("../../../docs/diagnostics/2026-11-12/svg2_svg_geometry_mouse_fields.html");
const CHROME: &str = include_str!("../../../docs/diagnostics/2026-11-12/svg2_svg_geometry_mouse_fields_cft148.txt");

/// Lines still off, each for a reason outside this row; filled in from the
/// first run with the fix. They must still DIFFER: one that matches comes
/// off the list.
const KNOWN: &[&str] = &[];

#[test]
fn svg_elements_have_geometry_and_pointer_events_their_fields() {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 800, height: 600 })
        .expect("view");
    engine.load_html(view, PAGE).expect("load");
    // The probe's pointer: two moves over the big rect, a press and release
    // on the padded box's child, a move back.
    engine.mouse_move_at_point(view, 100.0, 80.0);
    engine.mouse_move_at_point(view, 130.0, 95.0);
    engine.mouse_move_at_point(view, 540.0, 70.0);
    engine.mouse_down_at_point(view, 540.0, 70.0);
    engine.click_at_point(view, 540.0, 70.0);
    engine.mouse_move_at_point(view, 150.0, 100.0);
    let got = engine
        .execute_script(view, "JSON.stringify(window.report().split('\\n'))")
        .expect("report");
    let got = got.strip_prefix("String(").and_then(|v| v.strip_suffix(')')).unwrap_or(&got).to_string();
    let got: String = serde_json::from_str(&got).expect("a JSON string");
    let got: Vec<String> = serde_json::from_str(&got).expect("a JSON array");
    let expected: Vec<&str> = CHROME.lines().filter(|l| !l.starts_with('#') && !l.is_empty()).collect();
    assert_eq!(expected.len(), 88, "the Chrome report has 88 lines");
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
    if got.len() > expected.len() {
        wrong.push(format!("  {} extra lines, first: {}", got.len() - expected.len(), got[expected.len()]));
    }
    assert!(wrong.is_empty(), "{} of {} report lines differ:\n{}", wrong.len(), expected.len(), wrong.join("\n"));
}
