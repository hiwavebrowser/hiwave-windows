//! Web API census: measures which web platform APIs a page script sees in
//! the bindings, and whether one basic call to each works. Measurement only;
//! it never fails on a missing or broken API, only if the probe cannot run.
//!
//! The harness mirrors how rustkit-engine sets up a URL-loaded page's
//! bindings (document, location, viewport size, script-network bridge),
//! except for the engine's cascade selector matcher, which is crate-private
//! there: selector APIs here use the bindings' built-in fallback matcher.
//!
//! Regenerate the ledger's raw data:
//!   WEB_API_CENSUS_OUT=docs/census/web_api_census.json \
//!     cargo test -p rustkit-bindings --test web_api_census -- --nocapture

use rustkit_bindings::DomBindings;
use rustkit_dom::Document;
use rustkit_js::{JsRuntime, JsValue};
use std::rc::Rc;
use url::Url;

const PAGE: &str = r#"<!DOCTYPE html><html><head><title>Census</title>
<style>.box { color: red; } #list li { margin: 0; }</style></head>
<body><div id="main" class="box wide" data-role="hero">Main</div>
<ul id="list"><li id="a1" class="item">One</li><li id="a2" class="item">Two</li><li id="a3" class="item">Three</li></ul>
<a id="lnk" href="/page?x=1">link</a><img src="/i.png" alt="">
<form id="f"><label id="lab" for="q">Q</label><input id="q" name="q" value="v">
<select id="sel" name="sel"><option value="a">A</option><option value="b" selected>B</option></select></form>
</body></html>"#;

const PROBE: &str = include_str!("census/web_api_census_probe.js");

fn eval_string(b: &DomBindings, script: &str) -> String {
    match b.evaluate(script).expect("census script failed") {
        JsValue::String(s) => s,
        other => panic!("expected a string, got {other:?}"),
    }
}

#[test]
fn web_api_census() {
    let b = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    b.set_document(Rc::new(Document::parse_html(PAGE).unwrap()))
        .unwrap();
    b.set_location(&Url::parse("https://census.test/start/index.html?a=1#top").unwrap())
        .unwrap();
    b.set_dimensions(1280.0, 800.0).unwrap();
    b.enable_net_bridge().unwrap();
    b.set_loop_iteration_limit(5_000_000);

    b.evaluate(PROBE).expect("probe failed to run");
    // Let timers, rAF, idle callbacks and the promise jobs they queue settle.
    // Network requests the probe queues are never answered (no network here).
    for _ in 0..5 {
        b.run_timers(60_000, 10_000).unwrap();
        b.evaluate("0").unwrap();
    }

    // A smoke Promise still pending now never settles here: its callback
    // was never called. That is a failed basic use, not a missing API.
    b.evaluate(
        "window.__census.forEach(function (r) { if (r.status === 'pending') { \
            r.status = 'broken'; r.detail = 'never settled: callback not called after draining timers'; } })",
    )
    .unwrap();
    let json = eval_string(&b, "JSON.stringify(window.__census)");
    let rows: Vec<serde_json::Value> = serde_json::from_str(&json).unwrap();
    assert!(
        rows.len() >= 300,
        "probe should cover at least 300 APIs, got {}",
        rows.len()
    );

    let count = |s: &str| rows.iter().filter(|r| r["status"] == s).count();
    println!(
        "census: total={} works={} broken={} missing={}",
        rows.len(),
        count("works"),
        count("broken"),
        count("missing")
    );
    for r in &rows {
        if r["status"] != "works" {
            println!(
                "  [{}] {} / {} {}",
                r["status"].as_str().unwrap(),
                r["area"].as_str().unwrap(),
                r["name"].as_str().unwrap(),
                r["detail"].as_str().unwrap()
            );
        }
    }
    if let Ok(path) = std::env::var("WEB_API_CENSUS_OUT") {
        let pretty = serde_json::to_string_pretty(&rows).unwrap();
        std::fs::write(&path, pretty + "\n").expect("write census output");
        println!("wrote {path}");
    }
}
