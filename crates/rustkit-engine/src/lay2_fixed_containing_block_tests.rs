//! LAY2: what a `position: fixed` box is positioned against.
//!
//! The viewport, unless an ancestor is a containing block for fixed
//! descendants (css-position-3 §2.1, css-transforms-1 §3, filter-effects-1
//! §5, css-contain-2 §3): one with a transform (`transform`, `rotate`,
//! `scale`, `translate`), `perspective`, a `filter` or `backdrop-filter`,
//! `will-change` naming one of those, or `contain: layout | paint | strict
//! | content`. An absolutely or relatively positioned ancestor, one that
//! clips or scrolls, another fixed box or a sticky one is not, and neither
//! is `opacity`, `isolation`, `z-index`, `contain: size | style`,
//! `container-type` or a transform on an inline box.
//!
//! The engine placed a fixed box inside an absolutely positioned ancestor
//! against that ancestor (DOM1's fixture: 810,70 where Chrome has 410,80).
//!
//! Expected lines: docs/diagnostics/2026-11-12/
//! lay2_fixed_containing_block_cft148.txt, `window.report()` of the page
//! beside it in Chrome for Testing 148.0.7778.216 at 800x600: every case
//! unscrolled and after `scrollTo(0, 100)`, with a fixed box that has insets
//! (`f`) and one at its static position (`s`).

use super::*;

const PAGE: &str = include_str!("../../../docs/diagnostics/2026-11-12/lay2_fixed_containing_block.html");
const CHROME: &str = include_str!("../../../docs/diagnostics/2026-11-12/lay2_fixed_containing_block_cft148.txt");

#[test]
fn a_fixed_box_is_positioned_against_the_viewport_or_its_fixed_containing_block() {
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
    assert_eq!(expected.len(), 79, "the Chrome report has 79 lines");
    // Still off, for a reason outside this row: the ancestor in
    // `inline_transform` is an inline box with margins, padding and a
    // border. Its own rect is 5px low here (335,440 for Chrome's 335,435)
    // and the static position of the block-level fixed box inside it is
    // the inline's content edge (348,440) where Chrome breaks the line
    // (320,454). The fixed box with insets (`f`) is right. The two lines
    // must still DIFFER; one that matches comes off the list.
    const KNOWN: &[&str] = &["top inline_transform: ", "scrolled inline_transform: "];
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
