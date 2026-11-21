//! Portable pin for #712: with scripting on, a `<meta http-equiv=refresh>`
//! inside `<noscript>` is text, not a refresh. Challenge pages use that
//! fallback for no-JS clients; honouring it navigates away while the
//! challenge script is still loading.
//!
//! Companion to the headless Cloudflare fixture in `script_loading_tests`
//! and to the open coverage PR's top-level meta-refresh replace pin (#713).

use super::*;
use rustkit_dom::Document;
use std::rc::Rc;

fn page(html: &str, url: &str) -> DomBindings {
    let bindings = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    bindings
        .set_document(Rc::new(Document::parse_html(html).unwrap()))
        .unwrap();
    bindings.set_location(&Url::parse(url).unwrap()).unwrap();
    let _ = bindings.take_navigation_requests_with_replace();
    bindings
}

#[test]
fn a_noscript_meta_refresh_is_not_a_navigation() {
    let html = r#"<!DOCTYPE html><html><head>
<meta http-equiv="refresh" content="360">
<noscript><meta http-equiv="refresh" content="0; url=/nojs"></noscript>
</head><body></body></html>"#;
    let b = page(html, "https://site.test/challenge");
    b.fire_lifecycle_event(LifecycleTarget::Window, "load")
        .unwrap();
    // The 360s top-level refresh is deferred; the noscript 0s fallback must
    // not fire even if timers advance.
    b.run_timers(0, 10).unwrap();
    assert_eq!(
        b.take_navigation_requests_with_replace(),
        Vec::<(String, bool)>::new(),
        "noscript fallback must not navigate when scripting is on"
    );
    b.run_timers(359_000, 10).unwrap();
    assert_eq!(
        b.take_navigation_requests_with_replace(),
        Vec::<(String, bool)>::new()
    );
    b.run_timers(360_000, 10).unwrap();
    assert_eq!(
        b.take_navigation_requests_with_replace(),
        [("https://site.test/challenge".into(), true)],
        "only the non-noscript refresh runs, as replace"
    );
}

#[test]
fn only_a_noscript_refresh_asks_for_nothing() {
    let html = r#"<!DOCTYPE html><html><head>
<noscript><meta http-equiv="refresh" content="0;url=/nojs"></noscript>
</head><body></body></html>"#;
    let b = page(html, "https://site.test/challenge");
    b.fire_lifecycle_event(LifecycleTarget::Window, "load")
        .unwrap();
    b.run_timers(0, 10).unwrap();
    assert_eq!(
        b.take_navigation_requests_with_replace(),
        Vec::<(String, bool)>::new()
    );
    b.run_timers(60_000, 10).unwrap();
    assert_eq!(
        b.take_navigation_requests_with_replace(),
        Vec::<(String, bool)>::new(),
        "no non-noscript refresh means no navigation at all"
    );
}
