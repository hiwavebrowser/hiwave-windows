//! STR-8 / STR-11 (stress suites, first run): four things page script found
//! missing.
//!
//! * `getBBox()`, `getTotalLength()` and `getPointAtLength()` on SVG shapes
//!   threw "not a callable function".
//! * `document.readyState` read `loading` in deferred and module scripts;
//!   it is `interactive` from the end of parsing (HTML §13.2.7 "the end").
//! * `XMLSerializer` was undefined.
//! * `FileReader` was undefined.
//!
//! Expected lines: docs/diagnostics/2026-11-12/
//! str8_svg_geometry_readystate_serializer_cft148.txt, `window.report()` of
//! the page beside it in Chrome for Testing 148.0.7778.216.

use super::*;
use crate::script_net_tests::serve_routes;

const PAGE: &str = include_str!("../../../docs/diagnostics/2026-11-12/str8_svg_geometry_readystate_serializer.html");
const DEFER: &str = include_str!("../../../docs/diagnostics/2026-11-12/str8_defer.js");
const CHROME: &str = include_str!("../../../docs/diagnostics/2026-11-12/str8_svg_geometry_readystate_serializer_cft148.txt");

#[test]
fn svg_geometry_ready_state_serializer_and_file_reader_answer_as_chrome() {
    let server = serve_routes(vec![
        ("/", "text/html", PAGE.to_string()),
        ("/str8_defer.js", "text/javascript", DEFER.to_string()),
    ]);
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 800, height: 600 })
        .expect("view");
    let url = Url::parse(&format!("http://127.0.0.1:{}/", server.port)).unwrap();
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    rt.block_on(engine.load_url(view, url)).expect("load_url");
    let got = engine
        .execute_script(view, "JSON.stringify(window.report().split('\\n'))")
        .expect("report");
    let got = got.strip_prefix("String(").and_then(|v| v.strip_suffix(')')).unwrap_or(&got);
    let got: String = serde_json::from_str(got).expect("a JSON string");
    let got: Vec<String> = serde_json::from_str(&got).expect("a JSON array");
    let expected: Vec<&str> = CHROME.lines().filter(|l| !l.starts_with('#') && !l.is_empty()).collect();
    assert_eq!(expected.len(), 84, "the Chrome report has 84 lines");
    let mut wrong = Vec::new();
    for (i, want) in expected.iter().enumerate() {
        match got.get(i) {
            Some(have) if have == want => {}
            Some(have) => wrong.push(format!("  {have}\n    Chromium: {want}")),
            None => wrong.push(format!("  (missing)\n    Chromium: {want}")),
        }
    }
    if got.len() > expected.len() {
        wrong.push(format!("  {} extra lines, first: {}", got.len() - expected.len(), got[expected.len()]));
    }
    assert!(wrong.is_empty(), "{} of {} report lines differ:\n{}", wrong.len(), expected.len(), wrong.join("\n"));
}
