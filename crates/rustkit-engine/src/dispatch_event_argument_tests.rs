//! `EventTarget.dispatchEvent` takes an Event: an object made by one of
//! the Event constructors (DOM §2.9, Web IDL). Anything else is a
//! TypeError, on every event target. And an event's `isTrusted` is its own
//! accessor with no setter, which cannot be redefined (DOM §2.2).
//!
//! Expected lines: docs/diagnostics/2026-11-12/dispatch_event_argument_cft148.txt,
//! `window.report()` of the page beside it in Chrome for Testing
//! 148.0.7778.216.

use super::*;

const PAGE: &str = include_str!("../../../docs/diagnostics/2026-11-12/dispatch_event_argument.html");
const CHROME: &str = include_str!("../../../docs/diagnostics/2026-11-12/dispatch_event_argument_cft148.txt");

#[test]
fn dispatch_event_takes_only_events_and_is_trusted_is_the_engines() {
    let mut engine = Engine::new_page(EngineConfig::default(), None, Arc::new(AtomicU64::new(0))).expect("engine");
    let id = engine.create_page_view(Bounds::new(0, 0, 800, 600)).expect("page view");
    engine.load_html(id, PAGE).expect("load_html");
    let out = engine.execute_script(id, "String(window.report())").expect("script");
    let got = out
        .strip_prefix("String(\"")
        .and_then(|s| s.strip_suffix("\")"))
        .map(|s| s.replace("\\n", "\n").replace("\\\"", "\"").replace("\\\\", "\\"))
        .unwrap_or(out);
    let got: Vec<&str> = got.lines().collect();
    let want: Vec<&str> = CHROME.lines().filter(|l| !l.starts_with('#') && !l.trim().is_empty()).collect();
    assert_eq!(got.len(), want.len(), "lines: {got:#?}");
    let wrong: Vec<String> = got.iter().zip(&want).filter(|(g, w)| g != w).map(|(g, w)| format!("got  {g}\n     want {w}")).collect();
    assert!(wrong.is_empty(), "the lines that differ from Chrome 148:\n{}", wrong.join("\n"));
}
