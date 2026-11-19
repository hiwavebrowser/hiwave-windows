//! DOM1: two things the DOM got wrong about what the author wrote.
//!
//! * Attribute order. rustkit-dom kept an element's attributes in a
//!   `HashMap`, so `attributes`, `getAttributeNames()`, `outerHTML` /
//!   `innerHTML` and XMLSerializer wrote them in name order. Chrome keeps
//!   the order they were written or set in: setting an existing name keeps
//!   its place, removing and adding it again puts it last.
//! * Hit testing. A box outside its ancestor's box was not hit: an
//!   absolutely positioned menu under a 24px header, a float in a
//!   zero-height parent, content taller than an `overflow: visible` box.
//!   `elementFromPoint` answered BODY and a click did not focus a field
//!   there. Under `overflow: hidden` the part outside is still not hit.
//!
//! Expected lines: docs/diagnostics/2026-11-12/dom1_attribute_order_cft148.txt
//! and dom1_hit_outside_ancestor_cft148.txt, `window.report()` of the pages
//! beside them in Chrome for Testing 148.0.7778.216 at 800x600.

use super::*;

fn report_of(page: &str) -> (Engine, EngineViewId, Vec<String>) {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 800, height: 600 })
        .expect("view");
    engine.load_html(view, page).expect("load");
    let got = engine
        .execute_script(view, "JSON.stringify(window.report().split('\\n'))")
        .expect("report");
    let got = got.strip_prefix("String(").and_then(|v| v.strip_suffix(')')).unwrap_or(&got).to_string();
    let got: String = serde_json::from_str(&got).expect("a JSON string");
    (engine, view, serde_json::from_str(&got).expect("a JSON array"))
}

/// Lines still off, each for a reason outside this row. They must still
/// DIFFER: a line that starts matching comes off the list.
///
/// * `style set before another attribute`: Chrome writes an element's
///   `style` attribute lazily, when something next reads its attributes,
///   so `el.style.color = 'red'` followed by `setAttribute('x', ..)` reads
///   `x,style`. Here the attribute is written at once (`style,x`).
/// * `rect field`: a bare text input is 19 tall here and 21 in Chrome
///   (SET3, on the settings stack).
const KNOWN: &[&str] = &[
    "style set before another attribute: ",
];

fn differences(chrome: &str, got: &[String], lines: usize) -> Vec<String> {
    let expected: Vec<&str> = chrome.lines().filter(|l| !l.starts_with('#') && !l.is_empty()).collect();
    assert_eq!(expected.len(), lines, "the Chrome report has {lines} lines");
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
    wrong
}

#[test]
fn attributes_keep_the_order_they_were_written_in() {
    let (_engine, _view, got) =
        report_of(include_str!("../../../docs/diagnostics/2026-11-12/dom1_attribute_order.html"));
    let wrong = differences(
        include_str!("../../../docs/diagnostics/2026-11-12/dom1_attribute_order_cft148.txt"),
        &got,
        32,
    );
    assert!(wrong.is_empty(), "{} of 32 report lines differ:\n{}", wrong.len(), wrong.join("\n"));
}

#[test]
fn a_box_outside_its_ancestors_box_is_hit() {
    let (mut engine, view, got) =
        report_of(include_str!("../../../docs/diagnostics/2026-11-12/dom1_hit_outside_ancestor.html"));
    let wrong = differences(
        include_str!("../../../docs/diagnostics/2026-11-12/dom1_hit_outside_ancestor_cft148.txt"),
        &got,
        37,
    );
    assert!(wrong.is_empty(), "{} of 37 report lines differ:\n{}", wrong.len(), wrong.join("\n"));
    // The engine's own click goes where elementFromPoint does.
    assert_eq!(
        engine.focus_at_point(view, 450.0, 470.0).as_deref(),
        Some("input"),
        "a click on the absolutely positioned field below its ancestors focuses it"
    );
}
