//! STR-4 W1: a classic dedicated worker. Expected values are the pinned
//! Chrome's (CfT 148.0.7778.216) on the same page and worker script
//! (`docs/diagnostics/2026-11-12/str4_worker_w1_chrome.mjs`, its output in
//! `str4_worker_w1_cft148.json`).

use super::*;
use crate::net_bridge::NetDelivery;
use std::time::{Duration, Instant};

const CHROME: &str = include_str!("../../../docs/diagnostics/2026-11-12/str4_worker_w1_cft148.json");

/// The worker the Chrome measurement ran, minus the `setTimeout` probe (W2).
const WORKER: &str = "\
self.onmessage = function (e) {
  if (e.data === 'close') { postMessage('closing'); self.close(); return; }
  if (e.data === 'info') {
    postMessage({ t: typeof self, doc: typeof document, win: typeof window, name: self.name,
      post: typeof postMessage, ael: typeof addEventListener, imp: typeof importScripts,
      loc: typeof location, nav: typeof navigator });
    return;
  }
  postMessage({ echo: e.data, isEvent: e instanceof MessageEvent, tgt: e.target === self });
};
addEventListener('message', function (e) { if (e.data === 'both') postMessage('listener'); });
postMessage('ready');
";

fn b64(bytes: &[u8]) -> String {
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for c in bytes.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= c.len() {
                out.push(T[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

fn bound() -> DomBindings {
    let b = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    b.set_document(Rc::new(
        Document::parse_html("<html><head></head><body></body></html>").unwrap(),
    ))
    .unwrap();
    // The worker scripts these tests serve are on this origin: a page with no origin of its own may start
    // only data: and blob: workers, as in Chrome.
    b.set_location(&url::Url::parse("http://a.test/").unwrap()).unwrap();
    b.enable_net_bridge().unwrap();
    b
}

fn ev(b: &DomBindings, script: &str) -> String {
    match b.evaluate(script).unwrap() {
        JsValue::String(s) => s,
        other => format!("{other:?}"),
    }
}

/// Answer every queued worker-script request with `source`.
fn serve(b: &DomBindings, source: &str) {
    for r in b.take_net_requests() {
        assert_eq!(r.destination, "worker");
        b.deliver_net_response(
            r.id,
            NetDelivery::Response {
                url: r.url,
                status: 200,
                status_text: "OK".into(),
                headers: vec![("content-type".into(), "text/javascript".into())],
                body_b64: b64(source.as_bytes()),
                kind: "basic".into(),
                redirected: false,
            },
        )
        .unwrap();
    }
}

/// Turn the page's timer pump until `done` holds (the page drains its
/// workers' messages there), for at most three seconds.
/// Until every worker the page started has returned (its script ended in `self.close()`), so what it
/// posted is all queued for the page. Bounded by progress, not a fixed wait: it fails only when no
/// worker has ended for 30 s (WFLK1: a fixed 500 ms sleep assumed every worker thread had started and
/// posted by then, which a loaded machine did not do).
fn until_workers_return(b: &DomBindings) {
    let (mut running, mut moved) = (b.worker_threads_running(), Instant::now());
    while running > 0 {
        assert!(moved.elapsed() < Duration::from_secs(30), "{running} workers still running after 30 s");
        std::thread::sleep(Duration::from_millis(5));
        let now = b.worker_threads_running();
        if now < running {
            (running, moved) = (now, Instant::now());
        }
    }
}

/// Turns until `done`, while `progress` (a page expression for a count) still moves: fails only when a
/// turn has delivered nothing new for 5 s. A fixed deadline failed on a loaded machine with the
/// messages still arriving (WFLK1: 8000 in 3 s).
fn pump_while_progressing(b: &DomBindings, done: &str, progress: &str) -> bool {
    let read = |b: &DomBindings| ev(b, &format!("String({progress})"));
    let (mut last, mut moved) = (read(b), Instant::now());
    loop {
        b.run_timers(0, 100).unwrap();
        if ev(b, &format!("String(!!({done}))")) == "true" {
            return true;
        }
        let now = read(b);
        if now != last {
            (last, moved) = (now, Instant::now());
        } else if moved.elapsed() > Duration::from_secs(5) {
            return false;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn pump_until(b: &DomBindings, done: &str) -> bool {
    let end = Instant::now() + Duration::from_secs(3);
    while Instant::now() < end {
        b.run_timers(0, 100).unwrap();
        if ev(b, &format!("String(!!({done}))")) == "true" {
            return true;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    false
}

#[test]
fn a_dedicated_worker_answers_as_in_chrome() {
    let b = bound();
    b.evaluate(
        "var res = [], w = new Worker('http://a.test/worker.js'); \
         w.onmessage = function (e) { res.push(e.data); }; \
         w.addEventListener('message', function (e) { if (e.data === 'ready') res.push('ael:ready'); }); \
         w.postMessage({ a: 1, b: [1, 2, 'x'], n: null }); \
         w.postMessage('info'); w.postMessage('both'); w.postMessage('close'); \
         setTimeout(function () { w.postMessage('after-close'); }, 100);",
    )
    .unwrap();
    serve(&b, WORKER);
    assert!(pump_until(&b, "res.length >= 7"), "got {}", ev(&b, "JSON.stringify(res)"));
    // a message after self.close() must not be answered
    b.run_timers(1000, 100).unwrap();
    std::thread::sleep(Duration::from_millis(100));
    b.run_timers(1000, 100).unwrap();
    let mut want: serde_json::Value = serde_json::from_str(CHROME).unwrap();
    want["log"][3].as_object_mut().unwrap().remove("st");
    let got: serde_json::Value = serde_json::from_str(&ev(&b, "JSON.stringify(res)")).unwrap();
    assert_eq!(got, want["log"]);
}

/// The page side holds the built-ins it uses from install (R1 nit on #1105). A page that replaces
/// `JSON.parse` and `Object.keys` once it has started a worker still gets its messages, and one that
/// replaces `WeakMap.prototype.get`/`set` still reaches its worker's own records (`onmessage`,
/// `postMessage`), as in Chrome, whose worker plumbing is not page script. (Event dispatch itself, which
/// is develop's EventTarget, is not this test's subject.)
#[test]
fn replacing_built_ins_does_not_reach_the_worker_plumbing() {
    let b = bound();
    b.evaluate("var res = [], w = new Worker('http://a.test/worker.js');").unwrap();
    // The script is delivered first: the network bridge (develop's) is not this test's subject.
    serve(&b, WORKER);
    b.evaluate(
        "JSON.parse = function () { throw new Error('page JSON.parse'); };          Object.keys = function () { return []; };          w.onmessage = function (e) { res.push(typeof e.data === 'string' ? e.data : 'obj'); };          w.postMessage('both');",
    )
    .unwrap();
    assert!(pump_until(&b, "res.length >= 2"), "got {}", ev(&b, "res.join()"));
    assert_eq!(ev(&b, "res.slice(0, 2).join()"), "ready,obj");

    let b = bound();
    b.evaluate("var w = new Worker('http://a.test/worker.js');").unwrap();
    serve(&b, WORKER);
    assert_eq!(
        ev(
            &b,
            "WeakMap.prototype.get = function () { return undefined; };              WeakMap.prototype.set = function () { return this; };              var h = function () {}; w.onmessage = h; w.postMessage('x'); w.terminate();              String(w.onmessage === h)",
        ),
        "true"
    );
}

#[test]
fn a_terminated_worker_never_answers() {
    let b = bound();
    b.evaluate(
        "var got = [], w = new Worker('http://a.test/worker.js'); \
         w.onmessage = function (e) { got.push(e.data); }; \
         w.terminate(); w.postMessage('x');",
    )
    .unwrap();
    serve(&b, WORKER);
    std::thread::sleep(Duration::from_millis(200));
    b.run_timers(1000, 100).unwrap();
    assert_eq!(ev(&b, "JSON.stringify(got)"), "[]");
    assert_eq!(CHROME.contains("\"afterTerminate\": []"), true);
}

#[test]
fn terminate_stops_a_worker_that_never_returns() {
    let b = bound();
    b.evaluate(
        "var got = [], w = new Worker('http://a.test/spin.js'); \
         w.onmessage = function (e) { got.push(e.data); }; w.postMessage('go');",
    )
    .unwrap();
    serve(&b, "self.onmessage = function () { postMessage('started'); while (true) {} };");
    assert!(pump_until(&b, "got.length >= 1"));
    assert_eq!(b.worker_threads_running(), 1);
    b.evaluate("w.terminate()").unwrap();
    let end = Instant::now() + Duration::from_secs(3);
    while b.worker_threads_running() != 0 && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(b.worker_threads_running(), 0);
}

#[test]
fn a_worker_cannot_reach_the_page() {
    let b = bound();
    b.evaluate(
        "window.secret = 1; var got = [], w = new Worker('http://a.test/peek.js'); \
         w.onmessage = function (e) { got.push(e.data); };",
    )
    .unwrap();
    serve(&b, "postMessage([typeof secret, typeof document, typeof window, typeof parent, typeof localStorage].join());");
    assert!(pump_until(&b, "got.length >= 1"));
    assert_eq!(ev(&b, "got[0]"), "undefined,undefined,undefined,undefined,undefined");
}

// ---- W2: timers, importScripts, uncaught errors, load failure.
// Expected values: str4_worker_w2_cft148.json (docs/diagnostics/2026-11-12/).

const CHROME_W2: &str = include_str!("../../../docs/diagnostics/2026-11-12/str4_worker_w2_cft148.json");

fn w2_file(path: &str) -> Option<&'static str> {
    Some(match path {
        "/timers.js" => {
            "var order = [];
             setTimeout(function () { order.push('t20'); }, 20);
             setTimeout(function () { order.push('t0'); }, 0);
             var n = 0, iv = setInterval(function () { order.push('i' + (++n)); if (n === 2) clearInterval(iv); }, 5);
             var c = setTimeout(function () { order.push('cancelled'); }, 1); clearTimeout(c);
             Promise.resolve().then(function () { order.push('micro'); });
             setTimeout(function (a, b) { order.push('args:' + a + b); postMessage(order); }, 60, 'x', 'y');"
        }
        "/lib.js" => "var fromLib = 'lib-ran'; function libFn() { return 42; }",
        "/lib2.js" => "var fromLib2 = 'lib2-ran';",
        "/imp.js" => {
            "var r = [];
             importScripts('/lib.js');
             r.push(fromLib, libFn());
             importScripts('/lib.js', '/lib2.js');
             r.push(typeof fromLib2, importScripts());
             try { importScripts('/missing.js'); } catch (e) { r.push('missing:' + e.name); }
             postMessage(r);"
        }
        "/throws.js" => "onmessage = function () { throw new Error('boom'); }; postMessage('up');",
        "/throwsnow.js" => "postMessage('before'); throw new TypeError('top-level');",
        "/syntax.js" => "var = ;",
        "/err.js" => "self.onerror = function () { postMessage('self.onerror'); }; postMessage('ok');",
        _ => return None,
    })
}

/// Run the Chrome measurement's page for `key` and return what it logged.
fn observe(key: &str, file: &str, post: Option<&str>) -> serde_json::Value {
    let b = bound();
    let post = post.map(|p| format!("w.postMessage('{p}');")).unwrap_or_default();
    b.evaluate(&format!(
        "var log = [], w = new Worker('http://a.test{file}'); \
         w.onmessage = function (e) {{ log.push(['m', e.data]); }}; \
         w.onerror = function (e) {{ log.push(['e', e.message === undefined ? null : e.message, typeof e.filename, \
           typeof e.lineno, e instanceof ErrorEvent, e.defaultPrevented]); e.preventDefault(); }}; {post}"
    ))
    .unwrap();
    let end = Instant::now() + Duration::from_millis(500);
    while Instant::now() < end {
        for r in b.take_net_requests() {
            let path = r.url.trim_start_matches("http://a.test").to_string();
            // As the Chrome measurement's server: scripts as text/javascript (importScripts checks it).
            let (status, body, content_type) = match w2_file(&path) {
                Some(s) => (200, s, "text/javascript"),
                None => (404, "no", "text/plain"),
            };
            b.deliver_net_response(
                r.id,
                NetDelivery::Response {
                    url: r.url,
                    status,
                    status_text: "".into(),
                    headers: vec![("content-type".into(), content_type.into())],
                    body_b64: b64(body.as_bytes()),
                    kind: "basic".into(),
                    redirected: false,
                },
            )
            .unwrap();
        }
        b.run_timers(0, 100).unwrap();
        std::thread::sleep(Duration::from_millis(5));
    }
    let got: serde_json::Value = serde_json::from_str(&ev(&b, "JSON.stringify(log)")).unwrap();
    let _ = key;
    got
}

fn chrome(key: &str) -> serde_json::Value {
    let all: serde_json::Value = serde_json::from_str(CHROME_W2).unwrap();
    all[key].clone()
}

#[test]
fn worker_timers_fire_in_chromes_order() {
    assert_eq!(observe("timers", "/timers.js", None), chrome("timers"));
}

#[test]
fn import_scripts_runs_scripts_in_the_worker_scope_and_fails_with_network_error() {
    assert_eq!(observe("imp", "/imp.js", None), chrome("imp"));
}

#[test]
fn an_uncaught_error_in_a_handler_reaches_the_worker_object_and_the_worker_lives_on() {
    assert_eq!(observe("throws", "/throws.js", Some("go")), chrome("throws"));
}

#[test]
fn an_uncaught_error_at_the_top_level_reaches_the_worker_object() {
    assert_eq!(observe("throwsnow", "/throwsnow.js", None), chrome("throwsnow"));
}

#[test]
fn a_syntax_error_in_the_worker_script_is_an_error_event() {
    let got = observe("syntax", "/syntax.js", None);
    let want = chrome("syntax");
    // Chrome's message names its own parser's token; the shape is what is pinned.
    assert_eq!(got.as_array().unwrap().len(), 1);
    let (g, w) = (&got[0], &want[0]);
    assert!(g[1].as_str().unwrap().starts_with("Uncaught SyntaxError"), "{g}");
    assert_eq!(g.as_array().unwrap()[2..], w.as_array().unwrap()[2..]);
}

#[test]
fn a_script_that_fails_to_load_is_a_plain_error_event() {
    assert_eq!(observe("nofile", "/nofile.js", None), chrome("nofile"));
}

#[test]
fn the_worker_scope_error_handler_sees_nothing_when_nothing_throws() {
    assert_eq!(observe("selferr", "/err.js", None), chrome("selferr"));
}

// ---- W3: blob: and data: scripts. Expected: str4_worker_w3_cft148.json.

const CHROME_W3: &str = include_str!("../../../docs/diagnostics/2026-11-12/str4_worker_w3_cft148.json");

#[test]
fn blob_and_data_scripts_run_as_in_chrome() {
    let b = bound();
    let src = "onmessage = function (e) { postMessage({ echo: e.data, proto: self.location.protocol, nm: self.name }); }; postMessage('up');";
    let q = serde_json::to_string(src).unwrap();
    b.evaluate(&format!(
        "var res = {{}}; \
         function go(key, url, opts) {{ \
           var log = res[key] = [], w = new Worker(url, opts); \
           w.onmessage = function (e) {{ log.push(e.data); }}; \
           w.onerror = function (e) {{ log.push('error:' + e.constructor.name); }}; \
           w.postMessage(key); return w; }} \
         var blob = new Blob([{q}], {{ type: 'text/javascript' }}); \
         var burl = URL.createObjectURL(blob); \
         res.urlShape = [burl.indexOf('blob:' + location.origin + '/') === 0, typeof URL.revokeObjectURL]; \
         go('blob', burl, {{ name: 'bn' }}); \
         var u2 = URL.createObjectURL(blob); go('revokedAfterNew', u2); URL.revokeObjectURL(u2); \
         go('data', 'data:text/javascript,' + encodeURIComponent({q})); \
         go('dataB64', 'data:text/javascript;base64,' + btoa({q})); \
         var u3 = URL.createObjectURL(blob); URL.revokeObjectURL(u3); \
         setTimeout(function () {{ go('revokedBefore', u3); }}, 0); \
         go('typedBlobHtml', URL.createObjectURL(new Blob(['postMessage(1)'], {{ type: 'text/html' }})));"
    ))
    .unwrap();
    let end = Instant::now() + Duration::from_millis(600);
    while Instant::now() < end {
        b.run_timers(0, 100).unwrap();
        std::thread::sleep(Duration::from_millis(5));
    }
    let got: serde_json::Value = serde_json::from_str(&ev(&b, "JSON.stringify(res)")).unwrap();
    let want: serde_json::Value = serde_json::from_str(CHROME_W3).unwrap();
    assert_eq!(got, want);
    // none of these went to the network
    assert!(b.take_net_requests().is_empty());
}

// ---- Lifetime: a page's workers end with the page.

#[test]
fn dropping_the_bindings_stops_their_workers() {
    let name = "lifetime-probe-spin";
    let b = bound();
    b.evaluate(&format!("var w = new Worker('http://a.test/spin.js', {{ name: '{name}' }}); w.postMessage('go');"))
        .unwrap();
    serve(&b, "onmessage = function () { postMessage('started'); while (true) {} };");
    let _ = b.evaluate("w.onmessage = function () { window.up = 1; };");
    assert!(pump_until(&b, "window.up === 1"));
    assert_eq!(b.worker_threads_running(), 1);
    drop(b);
    let end = Instant::now() + Duration::from_secs(3);
    let ended = || crate::worker_host::ENDED.lock().unwrap().iter().any(|n| n == name);
    while !ended() && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(ended(), "the worker outlived its page");
}

#[test]
fn an_idle_worker_with_a_timer_ends_with_its_page() {
    let name = "lifetime-probe-timer";
    let b = bound();
    b.evaluate(&format!("var w = new Worker('http://a.test/t.js', {{ name: '{name}' }}); w.onmessage = function () {{ window.up = 1; }};"))
        .unwrap();
    serve(&b, "setInterval(function () {}, 5); postMessage('up');");
    assert!(pump_until(&b, "window.up === 1"));
    drop(b);
    let end = Instant::now() + Duration::from_secs(3);
    let ended = || crate::worker_host::ENDED.lock().unwrap().iter().any(|n| n == name);
    while !ended() && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(ended(), "the worker outlived its page");
}

// ---- W4: structured clone through postMessage. Expected: str4_worker_w4_cft148.json.

const W4_CASES: &str = include_str!("../../../docs/diagnostics/2026-11-12/str4_worker_w4_cases.json");
const CHROME_W4: &str = include_str!("../../../docs/diagnostics/2026-11-12/str4_worker_w4_cft148.json");

/// Run the Chrome measurement's page and worker, and return what the page logged.
fn w4_observe() -> serde_json::Value {
    let fixture: serde_json::Value = serde_json::from_str(W4_CASES).unwrap();
    let worker = fixture["WORKER"].as_str().unwrap().to_string();
    let page_js = fixture["PAGE_JS"].as_str().unwrap().replace("'/w.js'", "'http://a.test/w.js'");
    let b = bound();
    // The page script runs as the Chrome page's inline script does; its final timer (virtual here)
    // is not what the test waits on.
    b.evaluate(&format!("(0, eval)({})", serde_json::to_string(&page_js).unwrap())).unwrap();
    let end = Instant::now() + Duration::from_millis(1500);
    while Instant::now() < end {
        serve(&b, &worker);
        b.run_timers(0, 100).unwrap();
        std::thread::sleep(Duration::from_millis(5));
    }
    serde_json::from_str(&ev(&b, "JSON.stringify(res)")).unwrap()
}

#[test]
fn structured_clone_through_post_message_matches_chrome() {
    let want: serde_json::Value = serde_json::from_str(CHROME_W4).unwrap();
    let got = w4_observe();
    let mut wrong = Vec::new();
    for (k, v) in want.as_object().unwrap() {
        // A Proxy cannot be told from its target in script; Chrome refuses to clone one.
        if k == "throwers" {
            for (t, tv) in v.as_object().unwrap() {
                if t != "proxy" && got["throwers"][t] != *tv {
                    wrong.push(format!("throwers.{t}: chrome {tv}, here {}", got["throwers"][t]));
                }
            }
            continue;
        }
        if &got[k] != v {
            wrong.push(format!("{k}: chrome {}, here {}", short(v), short(&got[k])));
        }
    }
    assert!(wrong.is_empty(), "{} differ:\n{}", wrong.len(), wrong.join("\n"));
}

fn short(v: &serde_json::Value) -> String {
    v.to_string().chars().take(160).collect()
}

/// Which script URLs `new Worker(url)` refuses, and how, as the pinned Chrome (CfT 148.0.7778.216) does
/// (`docs/diagnostics/2026-11-13/str4_worker_origin_chrome.mjs`, output `str4_worker_origin_cft148.json`;
/// the two loopback ports are `MAIN` and `OTHER` there). A script from another origin (another port,
/// host or scheme, with or without CORS headers) and a `javascript:` URL throw a `SecurityError` at the
/// call, before any request: pages catch it to fall back to a `blob:` worker. A URL that does not parse
/// is a `SyntaxError` with its own message. R1 on #1105.
#[test]
fn a_worker_script_from_another_origin_throws_at_construction_as_in_chrome() {
    const ORIGIN: &str = include_str!("../../../docs/diagnostics/2026-11-13/str4_worker_origin_cft148.json");
    let b = bound();
    b.set_location(&url::Url::parse("http://127.0.0.1:7001/").unwrap()).unwrap();
    let want: serde_json::Value = serde_json::from_str(&ORIGIN.replace("MAIN", "7001").replace("OTHER", "7002")).unwrap();
    let cases = [
        ("sameOriginRelative", "/w.js"),
        ("sameOriginAbsolute", "http://127.0.0.1:7001/w.js"),
        ("otherPort", "http://127.0.0.1:7002/w.js"),
        ("otherPortWithCors", "http://127.0.0.1:7002/cors.js"),
        ("otherHost", "http://localhost:7001/w.js"),
        ("otherScheme", "https://127.0.0.1:7001/w.js"),
        ("dataUrl", "data:text/javascript,postMessage(1)"),
        ("javascriptUrl", "javascript:postMessage(1)"),
        ("malformed", "http://"),
        ("emptyString", ""),
    ];
    let mut wrong = Vec::new();
    let mut try_url = |key: &str, url_expr: &str| {
        let script = format!(
            "(function (u) {{ var r = {{ threw: false }}, w; \
               try {{ w = new Worker(u); }} catch (e) {{ r = {{ threw: true, name: e && e.name, code: e && e.code, \
                 isDOMException: e instanceof DOMException, message: String(e && e.message) }}; }} \
               if (w) {{ try {{ w.terminate(); }} catch (_) {{}} }} \
               return JSON.stringify(r); }})({url_expr})"
        );
        let got: serde_json::Value = serde_json::from_str(&ev(&b, &script)).unwrap();
        let asked = b.take_net_requests().len();
        if got != want[key] {
            wrong.push(format!("{key}: chrome {}, here {}", short(&want[key]), short(&got)));
        }
        if got["threw"] == true && asked != 0 {
            wrong.push(format!("{key}: threw, but {asked} request(s) were made first"));
        }
    };
    for (key, url) in cases {
        try_url(key, &serde_json::to_string(url).unwrap());
    }
    try_url("blobSameOrigin", "URL.createObjectURL(new Blob(['postMessage(1)'], { type: 'text/javascript' }))");
    assert!(wrong.is_empty(), "{} differ:\n{}", wrong.len(), wrong.join("\n"));
}

const NITS: &str = include_str!("../../../docs/diagnostics/2026-11-13/str4_worker_nits_cft148.json");

/// A document whose origin is opaque (a data: URL page, a sandboxed frame) starts no worker from an
/// http(s) URL: an opaque origin matches nothing, so Chrome throws the SecurityError naming origin
/// 'null'. data: and blob: workers still start. Measured in CfT 148.0.7778.216
/// (`docs/diagnostics/2026-11-13/str4_worker_nits_chrome.mjs`). R1 LOW on #1105.
#[test]
fn a_page_with_an_opaque_origin_starts_only_data_and_blob_workers() {
    let b = bound();
    b.set_location(&url::Url::parse("data:text/html,x").unwrap()).unwrap();
    let chrome: serde_json::Value = serde_json::from_str(&NITS.replace("PORT", "7001")).unwrap();
    let want = &chrome["dataUrlPage"];
    let script = "(function () { \
        function t(u) { var r = { threw: false }, w; \
          try { w = new Worker(u); } catch (e) { r = { threw: true, name: e && e.name, code: e && e.code, message: String(e && e.message) }; } \
          if (w) { try { w.terminate(); } catch (_) {} } return r; } \
        return JSON.stringify({ origin: String(location.origin), http: t('http://127.0.0.1:7001/w.js'), \
          data: t('data:text/javascript,postMessage(1)') }); })()";
    let got: serde_json::Value = serde_json::from_str(&ev(&b, script)).unwrap();
    let asked = b.take_net_requests().len();
    for key in ["origin", "http", "data"] {
        assert_eq!(got[key], want[key], "{key}");
    }
    assert_eq!(asked, 0, "a refused worker made a request");
}

/// A "__proto__" key in a cloned object arrives as an own property, as in Chrome: the receiver's
/// prototype is not set from it, at the top level or nested. CfT 148.0.7778.216. R1 LOW on #1105.
#[test]
fn a_proto_key_in_a_clone_is_an_own_property() {
    let b = bound();
    b.evaluate(
        "var got = [], proto = null, w = new Worker('http://a.test/proto.js'); \
         w.onmessage = function (e) { got.push(e.data); if (got.length === 2) { \
           var o = got[0], a = got[1][0]; \
           proto = { objKeys: Object.keys(o), objOwnProto: Object.prototype.hasOwnProperty.call(o, '__proto__'), \
             objX: o.x === undefined ? 'undefined' : o.x, objProtoIsObjectProto: Object.getPrototypeOf(o) === Object.prototype, \
             objOwnProtoX: o.hasOwnProperty('__proto__') ? Object.getOwnPropertyDescriptor(o, '__proto__').value.x : null, \
             inArrayKeys: Object.keys(a), inArrayOwnProto: Object.prototype.hasOwnProperty.call(a, '__proto__'), \
             inArrayX: a.x === undefined ? 'undefined' : a.x }; } };",
    )
    .unwrap();
    serve(
        &b,
        "postMessage(JSON.parse('{\"__proto__\":{\"x\":1},\"y\":2}'));\npostMessage([JSON.parse('{\"__proto__\":{\"x\":1}}')]);",
    );
    assert!(pump_until(&b, "proto !== null"), "got {}", ev(&b, "JSON.stringify(got)"));
    let chrome: serde_json::Value = serde_json::from_str(NITS).unwrap();
    let got: serde_json::Value = serde_json::from_str(&ev(&b, "JSON.stringify(proto)")).unwrap();
    assert_eq!(got, chrome["protoKey"]);
}

/// A worker that floods the page cannot stall it: one page turn delivers at most
/// `MAX_ITEMS_PER_POLL` (256) of a worker's messages and leaves the rest, in order, for the next
/// turns (Chrome delivers one message per task; this keeps a turn bounded the same way). Before,
/// a turn parsed and dispatched the whole backlog before the page could paint. R1 LOW on #1105.
#[test]
fn a_flooding_worker_cannot_hold_a_page_turn() {
    let b = bound();
    b.evaluate("var got = [], w = new Worker('http://a.test/flood.js'); w.onmessage = function (e) { got.push(e.data); };")
        .unwrap();
    serve(&b, "for (var i = 0; i < 5000; i++) postMessage(i); self.close();");
    // The worker has sent everything before the page's first turn.
    until_workers_return(&b);
    b.run_timers(0, 100).unwrap();
    let first: usize = ev(&b, "String(got.length)").parse().unwrap();
    assert_eq!(first, 256, "one page turn delivered {first} of the 5000 queued messages");
    assert!(
        pump_while_progressing(&b, "got.length === 5000", "got.length"),
        "only {} arrived",
        ev(&b, "String(got.length)")
    );
    assert_eq!(ev(&b, "String(got.every(function (v, i) { return v === i; }))"), "true", "out of order");
}

/// The worker bridge's host functions are captured once by the page side and are not left on the
/// page's global for page script to call (as the other bridges are). The engine counts running workers
/// through the bindings' own guard, not through a page global. R1 LOW on #1105.
#[test]
fn the_worker_bridge_is_not_left_on_the_page_global() {
    let b = bound();
    let left = ev(
        &b,
        "JSON.stringify(['spawn', 'post', 'poll', 'reply', 'terminate', 'running'].filter(function (n) { \
           return ('__rustkit_worker_' + n) in globalThis; }))",
    );
    assert_eq!(left, "[]");
    assert_eq!(ev(&b, "typeof Worker"), "function");
}

/// The bound is the page turn's, not each worker's: four flooding workers still get at most 256
/// messages delivered in one turn between them (not 4 x 256), and the turns after it take from each
/// worker in turn, so a busy first worker does not starve the others. Security review of #1105.
#[test]
fn flooding_workers_share_one_turns_budget_and_each_is_served() {
    let b = bound();
    b.evaluate(
        "var got = [[], [], [], []]; \
         for (var k = 0; k < 4; k++) (function (k) { var w = new Worker('http://a.test/flood' + k + '.js'); \
           w.onmessage = function (e) { got[k].push(e.data); }; })(k);",
    )
    .unwrap();
    serve(&b, "for (var i = 0; i < 2000; i++) postMessage(i); self.close();");
    // Every worker has sent all 2000 before the page's first turn.
    until_workers_return(&b);
    let total = "String(got[0].length + got[1].length + got[2].length + got[3].length)";
    b.run_timers(0, 100).unwrap();
    let first: usize = ev(&b, total).parse().unwrap();
    assert_eq!(first, 256, "one page turn delivered {first} of the 8000 queued messages from four workers");
    for _ in 0..4 {
        b.run_timers(0, 100).unwrap();
    }
    assert_eq!(
        ev(&b, "String(got.every(function (g) { return g.length > 0; }))"),
        "true",
        "a worker was starved: {}",
        ev(&b, "JSON.stringify(got.map(function (g) { return g.length; }))")
    );
    assert!(
        pump_while_progressing(
            &b,
            "got.every(function (g) { return g.length === 2000; })",
            "got[0].length + got[1].length + got[2].length + got[3].length"
        ),
        "not all arrived: {}",
        ev(&b, "JSON.stringify(got.map(function (g) { return g.length; }))")
    );
    assert_eq!(
        ev(&b, "String(got.every(function (g) { return g.every(function (v, i) { return v === i; }); }))"),
        "true",
        "out of order"
    );
}

/// The structured clone helper is held by the scripts that use it, not left on a global: the page's
/// window (an engine global a page could enumerate or call; the engine's
/// a_web_page_enumerates_no_engine_globals_on_window found `__rk_sc`, batch 75) and the worker's global
/// scope. postMessage still clones both ways (the other worker tests).
#[test]
fn the_structured_clone_helper_is_on_neither_global() {
    let b = bound();
    assert_eq!(
        ev(&b, "Object.getOwnPropertyNames(globalThis).filter(function (n) { return n.indexOf('__rk_sc') === 0; }).join(' ')"),
        ""
    );
    b.evaluate("var seen = null, w = new Worker('http://a.test/sc.js'); w.onmessage = function (e) { seen = e.data; };").unwrap();
    serve(&b, "postMessage({ sc: typeof __rk_sc, own: Object.getOwnPropertyNames(self).indexOf('__rk_sc') });");
    assert!(pump_until(&b, "seen !== null"), "no answer");
    assert_eq!(ev(&b, "JSON.stringify(seen)"), "{\"sc\":\"undefined\",\"own\":-1}");
}

/// Worker scope hardening: a worker's own global scope carries none of the host's or the prelude's
/// plumbing (`__`-prefixed names): the prelude holds the host functions it was given, and the host holds
/// the prelude's entry points.
#[test]
fn a_worker_scope_has_no_host_or_prelude_globals() {
    let b = bound();
    b.evaluate("var seen = null, w = new Worker('http://a.test/g.js'); w.onmessage = function (e) { seen = e.data; };").unwrap();
    serve(&b, "postMessage(Object.getOwnPropertyNames(self).filter(function (n) { return n.indexOf('__') === 0; }).join(' '));");
    assert!(pump_until(&b, "seen !== null"), "no answer");
    assert_eq!(ev(&b, "seen"), "");
}

/// Worker scope hardening: what the host calls each turn is what the prelude set up, held by the host,
/// not whatever the worker's global holds under that name later.
#[test]
fn a_worker_cannot_replace_what_the_host_calls() {
    let b = bound();
    b.evaluate("var got = [], w = new Worker('http://a.test/h.js'); w.onmessage = function (e) { got.push(e.data); };").unwrap();
    serve(
        &b,
        "self.onmessage = function (e) { postMessage('handled:' + e.data); };\n\
         try { self.__rk_worker_dispatch = function () { postMessage('replaced'); }; } catch (_) {}\n\
         postMessage('ready');",
    );
    assert!(pump_until(&b, "got.length >= 1"), "no ready");
    b.evaluate("w.postMessage('x');").unwrap();
    assert!(pump_until(&b, "got.length >= 2"), "no answer: {}", ev(&b, "JSON.stringify(got)"));
    assert_eq!(ev(&b, "JSON.stringify(got)"), "[\"ready\",\"handled:x\"]");
}

/// Worker scope hardening: the clone decoder builds only what the encoder can describe. A payload naming
/// any other constructor, or an id that is not one of its own records, is refused; nothing is built.
#[test]
fn the_clone_decoder_builds_only_what_it_allows() {
    let mut rt = rustkit_js::JsRuntime::new().unwrap();
    rt.evaluate_script(include_str!("structured_clone.js")).unwrap();
    let cases = [
        ("view of another constructor", r#"{"r":{"@":0},"n":[{"t":"TA","c":"Function","b":"return 1","o":0,"l":0}]}"#),
        ("error of another constructor", r#"{"r":{"@":0},"n":[{"t":"E","n":"Function","m":"return 1"}]}"#),
        ("error of an inherited name", r#"{"r":{"@":0},"n":[{"t":"E","n":"constructor","m":"x"}]}"#),
        ("an id that is not a record", r#"{"r":{"@":"constructor"},"n":[]}"#),
        ("an inherited marker", r#"{"r":{"toString":1},"n":[]}"#),
    ];
    let mut wrong = Vec::new();
    for (what, payload) in cases {
        let script = format!(
            "(function () {{ try {{ var v = __rk_sc.deserialize({}); return 'built:' + typeof v; }} catch (e) {{ return 'refused'; }} }})()",
            serde_json::to_string(payload).unwrap()
        );
        match rt.evaluate_script(&script) {
            Ok(JsValue::String(s)) if s == "refused" => {}
            other => wrong.push(format!("{what}: {other:?}")),
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// Worker scope hardening: one malformed item from a worker costs only that item; the rest of the poll,
/// that worker's and other workers', is still delivered.
#[test]
fn one_malformed_item_does_not_drop_the_rest_of_the_poll() {
    let b = bound();
    b.evaluate(
        "var got = [], errors = 0; \
         var a = new Worker('http://a.test/a.js'), c = new Worker('http://a.test/c.js'); \
         a.onmessage = function (e) { got.push('a:' + e.data); }; c.onmessage = function (e) { got.push('c:' + e.data); }; \
         a.onerror = function () { errors++; }; c.onerror = function () { errors++; };",
    )
    .unwrap();
    serve(&b, "self.onmessage = function (e) { postMessage(e.data); };");
    std::thread::sleep(Duration::from_millis(200));
    b.workers.inject_all(|| crate::worker_host::FromWorker::Error("not json {".into()));
    b.evaluate("a.postMessage('1'); c.postMessage('2');").unwrap();
    std::thread::sleep(Duration::from_millis(300));
    assert!(pump_until(&b, "got.length >= 2"), "lost: {}", ev(&b, "JSON.stringify(got)"));
    assert_eq!(ev(&b, "JSON.stringify(got.slice().sort())"), "[\"a:1\",\"c:2\"]");
}

/// A worker's `location` has WorkerLocation's members only (Chrome's has no `ua`); the prelude used a `ua`
/// field on it to build `navigator` and left it there (R1 LOW on #1105). `navigator.userAgent` stays.
#[test]
fn a_worker_location_carries_no_user_agent_field() {
    let b = bound();
    b.evaluate("var seen = null, w = new Worker('http://a.test/l.js'); w.onmessage = function (e) { seen = e.data; };").unwrap();
    serve(&b, "postMessage({ ua: 'ua' in location, nav: typeof navigator.userAgent, href: location.href });");
    assert!(pump_until(&b, "seen !== null"), "no answer");
    assert_eq!(ev(&b, "JSON.stringify(seen)"), "{\"ua\":false,\"nav\":\"string\",\"href\":\"http://a.test/l.js\"}");
}

/// The malformed script-request branch, pinned as the malformed error branch is: a request item the page
/// cannot read costs only itself (R1 LOW on #1105; the guard is in the worker scope hardening fix).
#[test]
fn a_malformed_script_request_does_not_drop_the_rest_of_the_poll() {
    let b = bound();
    b.evaluate(
        "var got = []; \
         var a = new Worker('http://a.test/a2.js'), c = new Worker('http://a.test/c2.js'); \
         a.onmessage = function (e) { got.push('a:' + e.data); }; c.onmessage = function (e) { got.push('c:' + e.data); };",
    )
    .unwrap();
    serve(&b, "self.onmessage = function (e) { postMessage(e.data); };");
    std::thread::sleep(Duration::from_millis(200));
    b.workers.inject_all(|| crate::worker_host::FromWorker::Fetch { rid: 999_999, urls: "not json [".into() });
    b.evaluate("a.postMessage('1'); c.postMessage('2');").unwrap();
    std::thread::sleep(Duration::from_millis(300));
    assert!(pump_until(&b, "got.length >= 2"), "lost: {}", ev(&b, "JSON.stringify(got)"));
    assert_eq!(ev(&b, "JSON.stringify(got.slice().sort())"), "[\"a:1\",\"c:2\"]");
}
