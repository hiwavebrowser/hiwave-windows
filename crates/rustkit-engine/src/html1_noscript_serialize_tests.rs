//! HTML1 follow-up (R1 on the Speedometer stack): a noscript's text is
//! written raw only where scripting is on for the node.
//!
//! With scripting on, a `<noscript>` holds one text node and `innerHTML` /
//! `outerHTML` write it as it is (HTML §13.3). In a document where
//! scripting is off (one `DOMParser` or `createHTMLDocument` made, a
//! template's contents) its children are elements and text, and text is
//! escaped like any other. The serializer wrote it raw everywhere, so text
//! holding markup came out of an inert document as markup.
//!
//! Expected lines: docs/diagnostics/2026-11-12/html1_noscript_serialize_cft148.txt.

use super::*;

const PAGE: &str = include_str!("../../../docs/diagnostics/2026-11-12/html1_noscript_serialize.html");
const CHROME: &str = include_str!("../../../docs/diagnostics/2026-11-12/html1_noscript_serialize_cft148.txt");

/// Lines that still differ, by their start: each must still differ.
const KNOWN: &[&str] = &[];

#[test]
fn a_noscripts_text_is_written_raw_only_where_scripting_is_on() {
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
    assert_eq!(expected.len(), 33, "the Chrome report has 33 lines");
    assert_eq!(got.len(), expected.len(), "as many lines as Chrome reports: {got:#?}");
    let (mut wrong, mut healed) = (Vec::new(), Vec::new());
    for (g, e) in got.iter().zip(&expected) {
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
