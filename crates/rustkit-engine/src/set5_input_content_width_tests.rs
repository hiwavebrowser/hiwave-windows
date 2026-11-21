//! SET5: a text input's content width comes from its font.
//!
//! It was the UA-font width (145px) scaled by the font size, whatever the
//! font and whatever `size` said. Chrome builds it from the font: a
//! character width times `size` (20 when absent), plus what the font's
//! widest character adds, rounded up. The character width is the advance of
//! "x" (rounded up from a half), or the advance of "0" for a short list of
//! families whose average width is not trusted (Times, Helvetica, Courier,
//! Lucida Grande, ...), which also get nothing added.
//!
//! Expected lines: docs/diagnostics/2026-11-12/set5_input_content_width_cft148.txt,
//! `window.report()` of set5_input_content_width.html in Chrome for Testing
//! 148.0.7778.216: 15 families at 13 font sizes, the `size` attribute, and
//! the text-like types.

use super::*;

const PAGE: &str = include_str!("../../../docs/diagnostics/2026-11-12/set5_input_content_width.html");
const CHROME: &str = include_str!("../../../docs/diagnostics/2026-11-12/set5_input_content_width_cft148.txt");

/// Lines that still differ, by their start: each must still differ (the
/// test fails when one starts matching, so the entry goes with its fix).
const KNOWN: &[&str] = &[
    // `system-ui` resolves to a face whose "x" advance and ascent differ
    // from Chrome's San Francisco at most sizes (its optical sizes and
    // tracking): a font-resolution row, not this rule.
    "system-ui:",
];

#[test]
fn a_text_inputs_content_width_is_built_from_its_font_and_size() {
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
    assert_eq!(expected.len(), 51, "the Chrome report has 51 lines");
    let (mut wrong, mut healed) = (Vec::new(), Vec::new());
    for i in 0..expected.len().max(got.len()) {
        let (g, e) = (got.get(i).map_or("(missing)", String::as_str), expected.get(i).copied().unwrap_or("(none)"));
        let held = KNOWN.iter().any(|k| e.starts_with(k));
        if g != e && !held {
            wrong.push(format!("  {g}\n    Chromium: {e}"));
        } else if g == e && held {
            healed.push(e.to_string());
        }
    }
    assert!(healed.is_empty(), "these lines match now; take them off KNOWN: {healed:#?}");
    assert!(wrong.is_empty(), "{} of {} report lines differ:\n{}", wrong.len(), expected.len(), wrong.join("\n"));
}
