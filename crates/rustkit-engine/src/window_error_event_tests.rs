//! The window's `error` event, end to end: the probe page itself, loaded over loopback from two
//! origins, against the pinned Chrome's record of it.
//!
//! Chrome for Testing 148.0.7778.216, headless, recorded 2026-11-16:
//! `docs/diagnostics/2026-11-16/window_error{.html,_page.js,_child.html,_module.js,_other.js}`
//! and `window_error_cft148.json`. The rules one by one are in `rustkit-bindings`
//! `window_error_event_tests`; this is the page's scripts, timers, frames, modules and another
//! origin's script through the engine's own loading and live loop.

use super::*;
use crate::script_net_tests::serve_routes;
use std::time::Duration;

const PIN: &str = include_str!("../../../docs/diagnostics/2026-11-16/window_error_cft148.json");
const PAGE_JS: &str = include_str!("../../../docs/diagnostics/2026-11-16/window_error_page.js");
const CHILD: &str = include_str!("../../../docs/diagnostics/2026-11-16/window_error_child.html");
const MODULE: &str = include_str!("../../../docs/diagnostics/2026-11-16/window_error_module.js");
const OTHER: &str = include_str!("../../../docs/diagnostics/2026-11-16/window_error_other.js");

/// The probe's `out`, once the page has gone through every step.
fn run_probe() -> serde_json::Value {
    let other = serve_routes(vec![("/window_error_other.js", "text/javascript", OTHER.to_string())]);
    // The probe's own page reads the other origin from its query; the test server's routes are
    // exact paths, so the same two scripts are named here.
    let page = format!(
        "<!doctype html><html><head><meta charset=utf-8></head><body>\
         <script>window.OTHER = 'http://127.0.0.1:{}';</script>\
         <script src=\"window_error_page.js\"></script></body></html>",
        other.port
    );
    let server = serve_routes(vec![
        ("/", "text/html", page),
        ("/window_error_page.js", "text/javascript", PAGE_JS.to_string()),
        ("/window_error_child.html", "text/html", CHILD.to_string()),
        ("/window_error_module.js", "text/javascript", MODULE.to_string()),
    ]);
    let config = EngineConfig { nested_contexts: true, post_load_requests_live: true, ..EngineConfig::default() };
    let mut engine = Engine::new(config).expect("engine");
    let view = engine.create_headless_view(Bounds { x: 0, y: 0, width: 800, height: 600 }).expect("view");
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let url = Url::parse(&format!("http://127.0.0.1:{}/", server.port)).unwrap();
    rt.block_on(engine.load_url(view, url)).expect("load_url");
    // A count of turns, not a clock: the probe's steps are 250 ms of the page's time apart and
    // each turn gives it 20 ms (the sleep only lets the loopback fetches land).
    let mut out = String::new();
    for _ in 0..1500 {
        std::thread::sleep(Duration::from_millis(5));
        rt.block_on(engine.pump_live(view, 20));
        out = engine.execute_script(view, "window.__out || ''").unwrap();
        if out.len() > "String(\"\")".len() {
            break;
        }
    }
    let text: String = serde_json::from_str(out.trim_start_matches("String(").trim_end_matches(')')).unwrap_or_else(|e| panic!("{e}: {out}"));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{e}: {text}"))
}

/// One event of the probe's record, as the fields both sides can be held to.
fn row(event: &serde_json::Value) -> String {
    let file = event["file"].as_str().unwrap_or("");
    format!(
        "{} | {} | error {} | positioned {} | {}",
        event["message"].as_str().unwrap_or("?"),
        if file.ends_with(".js") { file } else { "(the document, or none)" },
        event["error"].as_str().unwrap_or("?"),
        event["lineno"] == true && event["colno"] == true,
        [&event["type"], &event["cancelable"], &event["bubbles"], &event["target"]].map(|v| v.to_string()).join(","),
    )
}

/// The page's events by message, and everything else the probe records, against Chrome's.
/// The known differences are edited into Chrome's record one by one, each asserted to be
/// Chrome's first, so a change on either side shows.
#[test]
fn the_probe_page_records_what_the_pinned_chrome_records() {
    let pin: serde_json::Value = serde_json::from_str(PIN).unwrap();
    assert_eq!(pin["browser"], "148.0.7778.216");
    let got = run_probe();
    // The steps that need the other origin's script are left out on both sides (see below).
    let rows = |record: &serde_json::Value| -> std::collections::BTreeSet<String> {
        record["events"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|event| !event["where"].as_str().is_some_and(|step| step.contains("other origin")))
            .map(row)
            .collect()
    };
    let mut want = rows(&pin);
    let mut differ = |chrome: &str, here: Option<&str>| {
        assert!(want.remove(chrome), "not in Chrome's record: {chrome}\n{want:#?}");
        if let Some(here) = here {
            want.insert(here.to_string());
        }
    };
    let own = "\"[object ErrorEvent]\",true,false,true";
    // A thrown value that is not an error, an attribute handler's code and an inserted inline
    // script have no position here (Chrome gives each one).
    for (message, error) in [
        ("Uncaught a string", "string"),
        ("Uncaught [object Object]", "object"),
        ("Uncaught null", "null"),
        ("Uncaught the page's string", "string"),
    ] {
        differ(
            &format!("{message} | window_error_page.js | error {error} | positioned true | {own}"),
            Some(&format!("{message} | (the document, or none) | error {error} | positioned false | {own}")),
        );
    }
    for message in ["Uncaught Error: from an attribute handler", "Uncaught TypeError: from an inserted script"] {
        let error = if message.contains("TypeError") { "TypeError" } else { "Error" };
        differ(
            &format!("{message} | (the document, or none) | error {error} | positioned true | {own}"),
            Some(&format!("{message} | (the document, or none) | error {error} | positioned false | {own}")),
        );
    }
    // The parser's own words for a syntax error, and its script's file.
    differ(
        &format!("Uncaught SyntaxError: Failed to execute 'appendChild' on 'Node': Unexpected token '=' | (the document, or none) | error SyntaxError | positioned true | {own}"),
        Some(&format!("Uncaught SyntaxError: expected token 'identifier', got '=' in identifier parsing at line 1, col 5 | window_error_page.js | error SyntaxError | positioned true | {own}")),
    );
    // The engine's own errors for the page's calls: its wording, and no position (the error is
    // made in the engine's code); an invalid selector of this shape does not throw here.
    differ(
        &format!("Uncaught HierarchyRequestError: Failed to execute 'appendChild' on 'Node': The new child element contains the parent. | window_error_page.js | error HierarchyRequestError | positioned true | {own}"),
        Some(&format!("Uncaught HierarchyRequestError: Failed to execute 'appendChild' on 'Node'. | (the document, or none) | error HierarchyRequestError | positioned false | {own}")),
    );
    differ(
        &format!("Uncaught SyntaxError: Failed to execute 'querySelector' on 'Document': '#page!!' is not a valid selector. | window_error_page.js | error SyntaxError | positioned true | {own}"),
        None,
    );
    // A module script a script inserts is `import()`ed here: its exception rejects that promise
    // and the element hears `error`; no event at the Window. (A module in the markup fires one:
    // the next test.)
    differ(&format!("Uncaught Error: from a module | window_error_module.js | error Error | positioned true | {own}"), None);
    // Another origin's script cannot be loaded in a test: the loader refuses a second loopback
    // origin ("Address not permitted"), so the probe's `other origin` steps are not reached here
    // and are left out of both sides above. Chrome's rows for them are the expectations of the
    // bindings tests, on scripts registered as the engine registers them; the fetch-time rule
    // has `script_error_detail`'s own tests.
    assert_eq!(rows(&got), want);

    // `onerror`: five arguments, `this` the Window, for every event it was called for.
    for call in got["onerror"].as_array().unwrap() {
        assert_eq!((call["n"].as_u64(), call["thisIsWindow"].as_bool()), (Some(5), Some(true)), "{call}");
    }
    let field = |name: &str| (got[name].clone(), pin[name].clone());
    // The page's rejections: `Promise.any`'s error and a failed import have the engine's own
    // wording here.
    let reasons = |record: &serde_json::Value| -> Vec<String> {
        record["rejections"]
            .as_array()
            .unwrap()
            .iter()
            .map(|reason| reason.as_str().unwrap_or("?").to_string())
            .map(|reason| {
                if reason == "All promises were rejected" || reason.starts_with("no promise in Promise.any") {
                    "(any)".to_string()
                } else if reason.starts_with("Failed to fetch dynamically imported module") {
                    "TypeError: a failed import".to_string()
                } else {
                    reason
                }
            })
            .collect::<std::collections::BTreeSet<String>>()
            .into_iter()
            .collect()
    };
    // As sets: the order of announcements across tasks is not held here, and the second
    // failed import of one URL is the same reason.
    assert_eq!(reasons(&got), reasons(&pin));
    for name in ["frame", "inHandler", "syncAfterDispatch", "preventSeen", "reportError", "reportErrorSync",
                 "reportErrorSame", "reportErrorNoArg", "pageDispatchCallsOnerror"] {
        let (here, chrome) = field(name);
        assert_eq!(here, chrome, "{name}");
    }
    // Known difference (dispatch order, not this event's): `onerror` runs after the Window's
    // listeners here, so a listener added after it does not see its cancel.
    assert_eq!(pin["cancelSeen"], serde_json::json!({ "prevented": true, "returnValue": false }));
    assert_eq!(got["cancelSeen"], serde_json::json!({ "prevented": false, "returnValue": true }));
}

/// A page whose markup has `scripts`, loaded; `window.seen` is each `error` event's
/// message, file name, `lineno > 0` and error.
fn seen_after(scripts: &str, routes: Vec<(&'static str, &'static str, String)>) -> String {
    let page = format!(
        "<!doctype html><html><head><script>window.seen = []; window.addEventListener('error', function (ev) {{ \
         seen.push([ev.message, String(ev.filename).replace(/^.*\\//, ''), ev.lineno > 0, ev.error === null ? 'null' : ev.error.name].join('|')); }});</script>\
         </head><body>{scripts}</body></html>"
    );
    let mut all = vec![("/", "text/html", page)];
    all.extend(routes);
    let server = serve_routes(all);
    let mut engine = Engine::new(EngineConfig { post_load_requests_live: true, ..EngineConfig::default() }).expect("engine");
    let view = engine.create_headless_view(Bounds { x: 0, y: 0, width: 800, height: 600 }).expect("view");
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    rt.block_on(engine.load_url(view, Url::parse(&format!("http://127.0.0.1:{}/", server.port)).unwrap())).expect("load_url");
    for _ in 0..400 {
        std::thread::sleep(Duration::from_millis(5));
        rt.block_on(engine.pump_live(view, 20));
        if engine.execute_script(view, "window.seen.length > 0").unwrap() == "Boolean(true)" {
            break;
        }
    }
    engine.execute_script(view, "window.seen.join(' ; ')").unwrap()
}

/// Chrome row `module`: a module's uncaught exception fires the event, with the module's file.
#[test]
fn a_markup_modules_exception_fires_the_event() {
    let seen = seen_after(
        "<script type=module src=/window_error_module.js></script>",
        vec![("/window_error_module.js", "text/javascript", MODULE.to_string())],
    );
    assert_eq!(seen, r#"String("Uncaught Error: from a module|window_error_module.js|true|Error")"#);
}

/// Classic scripts share one global scope, whoever runs them: scripts in the markup, inline
/// scripts a script inserts, and string timers. A top-level `let` or `const` is a global lexical
/// binding (later scripts see it, it is not a property of `window`, declaring it again is a
/// SyntaxError); a top-level `var` is a property of `window`. The probe page against Chrome's
/// record (`docs/diagnostics/2026-11-16/script_scope{.html,_a.js,_b.js,_cft148.json}`).
#[test]
fn classic_scripts_share_one_global_scope_as_in_chrome() {
    const PAGE: &str = include_str!("../../../docs/diagnostics/2026-11-16/script_scope.html");
    const A: &str = include_str!("../../../docs/diagnostics/2026-11-16/script_scope_a.js");
    const B: &str = include_str!("../../../docs/diagnostics/2026-11-16/script_scope_b.js");
    const PIN: &str = include_str!("../../../docs/diagnostics/2026-11-16/script_scope_cft148.json");
    let server = serve_routes(vec![
        ("/", "text/html", PAGE.to_string()),
        ("/script_scope_a.js", "text/javascript", A.to_string()),
        ("/script_scope_b.js", "text/javascript", B.to_string()),
    ]);
    let mut engine = Engine::new(EngineConfig { post_load_requests_live: true, ..EngineConfig::default() }).expect("engine");
    let view = engine.create_headless_view(Bounds { x: 0, y: 0, width: 800, height: 600 }).expect("view");
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    rt.block_on(engine.load_url(view, Url::parse(&format!("http://127.0.0.1:{}/", server.port)).unwrap())).expect("load_url");
    let mut out = String::new();
    for _ in 0..200 {
        std::thread::sleep(Duration::from_millis(5));
        rt.block_on(engine.pump_live(view, 20));
        out = engine.execute_script(view, "window.__out || ''").unwrap();
        if out.len() > "String(\"\")".len() {
            break;
        }
    }
    let text: String = serde_json::from_str(out.trim_start_matches("String(").trim_end_matches(')')).unwrap_or_else(|e| panic!("{e}: {out}"));
    let got: serde_json::Value = serde_json::from_str(&text).unwrap();
    let pin: serde_json::Value = serde_json::from_str(PIN).unwrap();
    assert_eq!(pin["browser"], "148.0.7778.216");
    for field in ["external", "inline", "timer", "errors"] {
        assert_eq!(got[field], pin[field], "{field}: {got}");
    }
}
