//! A dedicated worker started from a URL, and `importScripts`, through the real engine: `Engine::load_url`,
//! the script-network bridge (`script_net`) and the fetch policy, against a local server. The R1 HOLD on
//! #1105 found both requests refused by the engine (every bindings test answered them itself). Expected
//! values: the pinned Chrome (CfT 148.0.7778.216) on the same routes,
//! `docs/diagnostics/2026-11-14/worker_url_e2e_chrome.mjs`, output `worker_url_e2e_cft148.json`.

use super::*;
use crate::script_net_tests::serve_routes;
use std::collections::BTreeMap;

const CHROME: &str = include_str!("../../../docs/diagnostics/2026-11-14/worker_url_e2e_cft148.json");
const JS: &str = "text/javascript";

fn routes() -> Vec<(&'static str, &'static str, String)> {
    vec![
        ("/", "text/html", "<!doctype html><title>w</title>".to_string()),
        ("/w.js", JS, "importScripts('/lib.js'); postMessage('ready:' + libValue); onmessage = function (e) { postMessage('echo:' + e.data); };".to_string()),
        ("/lib.js", JS, "var libValue = 'lib';".to_string()),
        ("/plain.js", "text/plain", "postMessage('ran');".to_string()),
        ("/imports_plain.js", JS, "try { importScripts('/lib_plain.js'); postMessage('imported'); } catch (e) { postMessage('import:' + e.name); }".to_string()),
        ("/lib_plain.js", "text/plain", "var libValue = 'plain';".to_string()),
        ("/image.js", "image/png", "postMessage('ran');".to_string()),
        ("/imports_redirect.js", JS, "importScripts('/redir_lib.js'); postMessage('ready:' + libValue);".to_string()),
        ("/imports_missing.js", JS, "try { importScripts('/missing.js'); postMessage('imported'); } catch (e) { postMessage('import:' + e.name); }".to_string()),
    ]
}

const CASES: &[(&str, &str)] = &[
    ("url_worker_with_import_scripts", "/w.js"),
    ("worker_script_served_as_text_plain", "/plain.js"),
    ("import_scripts_served_as_text_plain", "/imports_plain.js"),
    ("import_scripts_through_a_same_origin_redirect", "/imports_redirect.js"),
    ("import_scripts_of_a_missing_script", "/imports_missing.js"),
    ("worker_script_through_a_same_origin_redirect", "/redir_w.js"),
    // Chrome refuses a worker script of an image, audio, video or text/csv type (Fetch, "should response
    // to request be blocked due to its MIME type"). The Chrome file's `import_scripts_with_two_content_types`
    // (Chrome runs it) needs two Content-Type headers, which this test's server does not send.
    ("worker_script_served_as_image_png", "/image.js"),
];

/// What the page logs for a worker started from `path`, after the live loop has run 1.5 s of turns.
fn heard(path: &str) -> Vec<String> {
    let server = serve_routes(routes());
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 200, height: 100 })
        .expect("view");
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let url = Url::parse(&format!("http://127.0.0.1:{}/", server.port)).unwrap();
    rt.block_on(engine.load_url(view, url)).expect("load_url");
    engine
        .execute_script(
            view,
            &format!(
                "window.log = []; var w = new Worker({path:?}); \
                 w.onmessage = function (e) {{ log.push('msg:' + e.data); if (String(e.data).indexOf('ready') === 0) w.postMessage('hi'); }}; \
                 w.onerror = function (e) {{ log.push('error:' + e.constructor.name); }};"
            ),
        )
        .expect("start the worker");
    let started = std::time::Instant::now();
    while started.elapsed() < std::time::Duration::from_millis(1500) {
        rt.block_on(engine.pump_live(view, 16));
        std::thread::sleep(std::time::Duration::from_millis(16));
    }
    let out = engine.execute_script(view, "JSON.stringify(log)").expect("log");
    let json = out.strip_prefix("String(").and_then(|s| s.strip_suffix(')')).unwrap_or(&out);
    let text: String = serde_json::from_str(json).unwrap_or_else(|_| json.to_string());
    serde_json::from_str(&text).unwrap_or_else(|_| panic!("log: {out}"))
}

#[test]
fn url_workers_and_import_scripts_load_through_the_engine_as_in_chrome() {
    let chrome: BTreeMap<String, Vec<String>> = serde_json::from_str(CHROME).unwrap();
    let mut wrong = Vec::new();
    for (name, path) in CASES {
        let got = heard(path);
        if got != chrome[*name] {
            wrong.push(format!("{name}: chrome {:?}, here {got:?}", chrome[*name]));
        }
    }
    assert!(wrong.is_empty(), "{} of {} differ:\n{}", wrong.len(), CASES.len(), wrong.join("\n"));
}
