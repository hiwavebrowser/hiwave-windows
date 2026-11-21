//! HTML1: with scripting on, `<noscript>` holds text.
//!
//! HTML §13.2.6.4.4 and §13.2.6.4.7: when the scripting flag is on, a
//! `<noscript>` start tag is a raw text element, in the head and in the
//! body, so its contents are one text node and nothing in them is an
//! element. The parser built the elements all the same: a page with
//! `<noscript><strong>…</strong></noscript>` had an element Chrome does not
//! (Speedometer's Vue workload), `querySelector` found it, and an `<img>`
//! in a noscript fallback was fetched.
//!
//! Expected lines: docs/diagnostics/2026-11-12/
//! html1_noscript_raw_text_cft148.txt, `window.report()` of the page beside
//! it in Chrome for Testing 148.0.7778.216. With scripting off (a
//! `DOMParser` document, a template's contents, a page whose scripts do not
//! run) the contents are still elements; the report has those too.

use super::*;
use crate::script_net_tests::serve_routes;

const PAGE: &str = include_str!("../../../docs/diagnostics/2026-11-12/html1_noscript_raw_text.html");
const CHROME: &str = include_str!("../../../docs/diagnostics/2026-11-12/html1_noscript_raw_text_cft148.txt");

/// Still off, for a reason outside this row: Chrome's computed `display`
/// of a `<noscript>` reads `inline` although it has no box (its rect is
/// 0x0 and `offsetParent` null, as here); the UA sheet here says `none`,
/// which is what `getComputedStyle` answers. The line must still DIFFER:
/// when it matches it comes off the list.
const KNOWN: &[&str] = &["does not render: "];

fn load(config: EngineConfig) -> (Engine, EngineViewId, crate::script_net_tests::Server) {
    let server = serve_routes(vec![
        ("/", "text/html", PAGE.to_string()),
        ("/html1_noscript_missing.css", "text/css", "#probe { color: rgb(0, 0, 255); }".to_string()),
        ("/html1_noscript_pixel.png", "image/png", String::new()),
        ("/html1_noscript_frame.html", "text/html", "<p>frame</p>".to_string()),
    ]);
    let mut engine = Engine::new(config).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 800, height: 600 })
        .expect("view");
    let url = Url::parse(&format!("http://127.0.0.1:{}/", server.port)).unwrap();
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    rt.block_on(engine.load_url(view, url)).expect("load_url");
    (engine, view, server)
}

#[test]
fn noscript_holds_text_when_scripting_is_on() {
    let (mut engine, view, server) = load(EngineConfig::default());
    let got = engine
        .execute_script(view, "JSON.stringify(window.report().split('\\n'))")
        .expect("report");
    let got = got.strip_prefix("String(").and_then(|v| v.strip_suffix(')')).unwrap_or(&got).to_string();
    let got: String = serde_json::from_str(&got).expect("a JSON string");
    let got: Vec<String> = serde_json::from_str(&got).expect("a JSON array");
    let expected: Vec<&str> = CHROME.lines().filter(|l| !l.starts_with('#') && !l.is_empty()).collect();
    assert_eq!(expected.len(), 38, "the Chrome report has 38 lines");
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
    // Nothing inside a noscript is fetched: not its image, its frame or
    // its sheet.
    for path in ["/html1_noscript_pixel.png", "/html1_noscript_frame.html", "/html1_noscript_missing.css"] {
        assert!(server.served(path).is_empty(), "{path} was requested {} times", server.served(path).len());
    }
}

/// With scripting off the fallback is the page: its contents are elements
/// (and its image is the page's).
#[test]
fn noscript_holds_elements_when_scripting_is_off() {
    let (engine, view, _server) = load(EngineConfig { javascript_enabled: false, ..EngineConfig::default() });
    let document = engine.views.get(&view).and_then(|v| v.document.clone()).expect("document");
    let mut inside = Vec::new();
    document.traverse(|node| {
        if node.tag_name().is_some_and(|t| t.eq_ignore_ascii_case("strong") || t.eq_ignore_ascii_case("img")) {
            inside.push(node.tag_name().unwrap().to_ascii_lowercase());
        }
    });
    assert_eq!(inside, ["strong", "img"], "the body noscript's markup is elements");
}
