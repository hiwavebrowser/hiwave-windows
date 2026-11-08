//! The Inspector's Network half through real headless loads: one `request`
//! per fetch the engine makes and exactly one `response` or `failure` for
//! it, for every kind of resource and every way a request can end.

use super::*;
use crate::inspector_engine_tests::{describe, records, runtime, serve, Route, TestServer};

/// A 1x1 PNG.
const PNG: &[u8] = &[
    137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 2, 0, 0, 0, 144, 119,
    83, 222, 0, 0, 0, 12, 73, 68, 65, 84, 120, 156, 99, 248, 207, 192, 0, 0, 3, 1, 1, 0, 201, 254, 146, 239, 0, 0, 0,
    0, 73, 69, 78, 68, 174, 66, 96, 130,
];

fn png(path: &'static str) -> Route {
    Route::bytes(path, "image/png", PNG.to_vec())
}

fn engine(config: EngineConfig, shield: Option<rustkit_net::RequestInterceptor>) -> (Engine, EngineViewId) {
    let mut config = config;
    config.inspector.buffer = true;
    let mut engine = Engine::with_interceptor(config, shield).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 300, height: 200 })
        .expect("view");
    (engine, view)
}

fn load(routes: Vec<Route>) -> (Engine, EngineViewId, TestServer) {
    let server = serve(routes);
    let (mut engine, view) = engine(EngineConfig::default(), None);
    runtime().block_on(engine.load_url(view, server.url("/"))).expect("load_url");
    (engine, view, server)
}

/// Every `request` has exactly one `response` or `failure`, after it, with
/// its id; nothing answers a request that was not made.
fn assert_each_request_answered_once(records: &[InspectorRecord]) {
    let mut open = std::collections::BTreeMap::new();
    let mut answered = std::collections::BTreeSet::new();
    for record in records {
        match &record.event {
            InspectorEvent::Request { id, url, .. } => {
                assert!(open.insert(*id, url.clone()).is_none(), "request #{id} recorded twice");
            }
            InspectorEvent::Response { id, .. } | InspectorEvent::Failure { id, .. } => {
                assert!(open.remove(id).is_some(), "#{id} answered without being open");
                assert!(answered.insert(*id), "#{id} answered twice");
            }
            _ => {}
        }
    }
    assert!(open.is_empty(), "requests never answered: {open:?}");
}

/// The network lines only, each request with its outcome, in request
/// order: `type path -> outcome`.
fn outcomes(records: &[InspectorRecord], port: u16) -> Vec<String> {
    let local = |s: &str| s.replace(&format!("http://127.0.0.1:{port}"), "");
    let mut lines: Vec<(u64, String)> = Vec::new();
    for record in records {
        match &record.event {
            InspectorEvent::Request { id, url, resource_type, initiator, .. } => lines.push((
                *id,
                format!("{} {} by={}", resource_type.as_str(), local(url), initiator.as_deref().unwrap_or("-")),
            )),
            InspectorEvent::Response { id, status, mime, .. } => {
                if let Some((_, line)) = lines.iter_mut().find(|(i, _)| i == id) {
                    line.push_str(&format!(" -> {status} {}", mime.as_deref().unwrap_or("-")));
                }
            }
            InspectorEvent::Failure { id, stage, .. } => {
                if let Some((_, line)) = lines.iter_mut().find(|(i, _)| i == id) {
                    line.push_str(&format!(" -> failed {}", stage.as_str()));
                }
            }
            _ => {}
        }
    }
    lines.into_iter().map(|(_, line)| line).collect()
}

fn sorted(mut lines: Vec<String>) -> Vec<String> {
    lines.sort();
    lines
}

#[test]
fn every_kind_of_resource_is_a_request_with_one_outcome() {
    let page = r#"<html><head><link rel="stylesheet" href="/a.css"></head><body>
<img src="/pic.png">
<script src="/app.js"></script>
<script type="module" src="/m.js"></script>
</body></html>"#;
    let (engine, view, server) = load(vec![
        Route::html("/", page),
        Route::ok("/a.css", "text/css", "@font-face { font-family: F; src: url(/f.woff2); } body { font-family: F; }"),
        Route::ok("/f.woff2", "font/woff2", "not really a font"),
        png("/pic.png"),
        Route::ok(
            "/app.js",
            "text/javascript",
            "fetch('/api/data'); var x = new XMLHttpRequest(); x.open('GET', '/api/xhr'); x.send();",
        ),
        Route::ok("/m.js", "text/javascript", "import './dep.js';"),
        Route::ok("/dep.js", "text/javascript", "export const d = 1;"),
        Route::ok("/api/data", "application/json", "{}"),
        Route::ok("/api/xhr", "text/plain", "ok"),
    ]);
    let records = records(&engine, view);
    assert_each_request_answered_once(&records);
    assert_eq!(
        sorted(outcomes(&records, server.port)),
        sorted(vec![
            "document / by=navigation -> 200 text/html".to_string(),
            "stylesheet /a.css by=parser -> 200 text/css".to_string(),
            "font /f.woff2 by=css -> 200 font/woff2".to_string(),
            "image /pic.png by=parser -> 200 image/png".to_string(),
            "script /app.js by=parser -> 200 text/javascript".to_string(),
            "module /m.js by=parser -> 200 text/javascript".to_string(),
            "module /dep.js by=script -> 200 text/javascript".to_string(),
            "fetch /api/data by=fetch -> 200 application/json".to_string(),
            "xhr /api/xhr by=xhr -> 200 text/plain".to_string(),
        ])
    );
}

#[test]
fn a_small_load_is_recorded_in_this_exact_order() {
    let page = r#"<html><head><link rel="stylesheet" href="/a.css"></head><body><img src="/pic.png"><script>console.log('hi')</script></body></html>"#;
    let (engine, view, server) = load(vec![
        Route::html("/", page),
        Route::ok("/a.css", "text/css", "body { margin: 0 }"),
        png("/pic.png"),
    ]);
    let lines: Vec<String> = records(&engine, view).iter().map(|r| describe(r, server.port)).collect();
    assert_eq!(
        lines,
        vec![
            "navigation_start /",
            "request #1 GET document / by=navigation",
            "response #1 200 text/html",
            "navigation_commit / 200",
            "request #2 GET stylesheet /a.css by=parser",
            "response #2 200 text/css",
            "first_layout",
            "first_paint",
            // The sheet arrived before the first layout (render-blocking),
            // so that layout is the initial load's.
            "relayout initial_load",
            "request #3 GET image /pic.png by=parser",
            "response #3 200 image/png",
            "relayout image_loaded(count=1)",
            "console Log <string len=2> @inline#1:1:9",
            "script parser inline#1 ran held=DOMContentLoaded",
            "lifecycle DOMContentLoaded held_by=0",
            "lifecycle load held_by=0",
            "load_finished over_budget=0",
        ]
    );
}

#[test]
fn a_404_image_is_a_response_with_its_status() {
    let (engine, view, server) = load(vec![Route::html("/", "<html><body><img src=\"/missing.png\"></body></html>")]);
    let records = records(&engine, view);
    assert_each_request_answered_once(&records);
    assert!(outcomes(&records, server.port).contains(&"image /missing.png by=parser -> 404 text/plain".to_string()));
}

#[test]
fn a_request_the_shield_refuses_is_a_policy_failure() {
    let server = serve(vec![Route::html("/", "<html><body><img src=\"/tracker.png\"></body></html>"), png("/tracker.png")]);
    let mut shield = rustkit_net::RequestInterceptor::new();
    shield.block(rustkit_net::intercept::UrlPattern::contains("tracker"));
    let (mut engine, view) = engine(EngineConfig::default(), Some(shield));
    runtime().block_on(engine.load_url(view, server.url("/"))).unwrap();
    let records = records(&engine, view);
    assert_each_request_answered_once(&records);
    assert!(outcomes(&records, server.port).contains(&"image /tracker.png by=parser -> failed policy".to_string()));
    assert!(!server.requests.lock().unwrap().iter().any(|p| p.contains("tracker")), "refused before any byte");
    let blocked_by: Vec<_> = records
        .iter()
        .filter_map(|r| match &r.event {
            InspectorEvent::Failure { blocked_by, .. } => Some(blocked_by.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(blocked_by, vec![None], "a rule that names no list records none");
}

/// Dev-tools T2: a shield that says which list and rule refused a request
/// has them on the failure, `blocked_by`, for every kind of request the
/// page makes: an image, a parser script, a script that script inserted,
/// and a `fetch`.
#[test]
fn a_shield_block_records_its_list_and_rule() {
    struct EasyListAds;
    impl rustkit_net::InterceptHandler for EasyListAds {
        fn intercept(&self, request: &rustkit_net::Request) -> rustkit_net::InterceptAction {
            if request.url.path().starts_with("/ads/") {
                rustkit_net::InterceptAction::BlockedBy(rustkit_net::BlockReason {
                    list: "EasyList".into(),
                    rule: Some("/ads/*".into()),
                })
            } else {
                rustkit_net::InterceptAction::Allow
            }
        }
    }
    let page = r#"<html><body><img src="/ads/i.png"><script src="/ads/p.js"></script><script>
var s = document.createElement('script'); s.src = '/ads/ins.js'; document.body.appendChild(s);
fetch('/ads/f').catch(function () {});
</script></body></html>"#;
    let server = serve(vec![
        Route::html("/", page),
        png("/ads/i.png"),
        Route::ok("/ads/p.js", "text/javascript", ""),
        Route::ok("/ads/ins.js", "text/javascript", ""),
        Route::ok("/ads/f", "text/plain", ""),
    ]);
    let mut shield = rustkit_net::RequestInterceptor::new();
    shield.add_handler(Arc::new(EasyListAds));
    let (mut engine, view) = engine(EngineConfig::default(), Some(shield));
    runtime().block_on(engine.load_url(view, server.url("/"))).unwrap();
    let records = records(&engine, view);
    assert_each_request_answered_once(&records);
    let local = |s: &str| s.replace(&format!("http://127.0.0.1:{}", server.port), "");
    let mut urls = std::collections::BTreeMap::new();
    let mut blocked = Vec::new();
    for record in &records {
        match &record.event {
            InspectorEvent::Request { id, url, .. } => {
                urls.insert(*id, local(url));
            }
            InspectorEvent::Failure { id, stage, blocked_by, .. } => {
                let by = blocked_by.as_ref().map(|b| format!("{} {}", b.list, b.rule.as_deref().unwrap_or("-")));
                blocked.push(format!("{} {} {}", urls[id], stage.as_str(), by.as_deref().unwrap_or("null")));
            }
            _ => {}
        }
    }
    assert_eq!(
        sorted(blocked),
        sorted(vec![
            "/ads/i.png policy EasyList /ads/*".to_string(),
            "/ads/p.js policy EasyList /ads/*".to_string(),
            "/ads/ins.js policy EasyList /ads/*".to_string(),
            "/ads/f policy EasyList /ads/*".to_string(),
        ])
    );
    assert!(server.requests.lock().unwrap().iter().all(|p| !p.starts_with("/ads/")), "refused before any byte");
}

/// Two script requests to another origin on the loopback: the policy's
/// private-network rule refuses one before it connects (a public page may
/// not reach private addresses, and another port is another site), and a
/// `same-origin` request is refused as cross-origin. (A response missing
/// `Access-Control-Allow-Origin` cannot be served from a loopback test
/// server for the same reason; the policy's own tests cover it.)
#[test]
fn script_requests_the_policy_refuses_are_policy_and_cors_failures() {
    let other = serve(vec![Route::ok("/data", "application/json", "{\"private\":true}")]);
    let page = format!(
        "<html><body><script>
fetch('http://127.0.0.1:{port}/data').catch(function () {{}});
fetch('http://127.0.0.1:{port}/data?again', {{ mode: 'same-origin' }}).catch(function () {{}});
</script></body></html>",
        port = other.port
    );
    let page: &'static str = Box::leak(page.into_boxed_str());
    let (engine, view, server) = load(vec![Route::html("/", page)]);
    let records = records(&engine, view);
    assert_each_request_answered_once(&records);
    let refused: Vec<String> = outcomes(&records, server.port)
        .into_iter()
        .filter(|l| l.starts_with("fetch "))
        .collect();
    assert_eq!(
        refused,
        vec![
            format!("fetch http://127.0.0.1:{}/data by=fetch -> failed policy", other.port),
            format!("fetch http://127.0.0.1:{}/data by=fetch -> failed cors", other.port),
        ]
    );
    assert_eq!(other.hits.load(std::sync::atomic::Ordering::SeqCst), 0, "neither reached the other server");
}

#[test]
fn a_slow_request_is_cancelled_by_navigating_away() {
    let server = serve(vec![
        Route::html("/", "<html><body>first</body></html>"),
        Route::ok("/slow", "text/plain", "late").delay(3000),
        Route::html("/next", "<html><body>next</body></html>"),
    ]);
    let (mut engine, view) = engine(EngineConfig::default(), None);
    let rt = runtime();
    rt.block_on(engine.load_url(view, server.url("/"))).unwrap();
    engine.execute_script(view, "fetch('/slow').catch(function () {}); 0").unwrap();
    // One live turn starts it; nothing waits for it.
    rt.block_on(engine.pump_live(view, 0));
    rt.block_on(engine.load_url(view, server.url("/next"))).unwrap();
    let records = records(&engine, view);
    assert_each_request_answered_once(&records);
    let slow: Vec<String> = records
        .iter()
        .map(|r| describe(r, server.port))
        .filter(|l| l.contains("/slow") || l.starts_with("failure"))
        .collect();
    assert_eq!(slow, vec!["request #2 GET fetch /slow by=fetch", "failure #2 cancelled abandoned (navigation or view closed)"]);
}

#[test]
fn a_stylesheet_with_the_wrong_mime_type_is_recorded_with_it_and_a_module_is_refused() {
    let page = r#"<html><head><link rel="stylesheet" href="/plain.css"></head><body>
<script type="module" src="/plain.js"></script></body></html>"#;
    let (mut engine, view, server) = load(vec![
        Route::html("/", page),
        Route::ok("/plain.css", "text/plain", "body { background: rgb(0, 128, 0) }"),
        Route::ok("/plain.js", "text/plain", "window.ran = true;"),
    ]);
    let records = records(&engine, view);
    assert_each_request_answered_once(&records);
    let lines = outcomes(&records, server.port);
    // Chrome refuses a stylesheet served as text/plain in standards mode;
    // RustKit applies it today. The event says what came, the engine's
    // behaviour is unchanged here.
    assert!(lines.contains(&"stylesheet /plain.css by=parser -> 200 text/plain".to_string()), "{lines:?}");
    assert!(lines.contains(&"module /plain.js by=parser -> failed mime".to_string()), "{lines:?}");
    assert_eq!(engine.execute_script(view, "String(window.ran)").unwrap(), r#"String("undefined")"#);
}

#[test]
fn a_redirect_chain_is_recorded_between_the_request_and_its_response() {
    let server = serve(vec![
        Route::redirect("/", "/hop"),
        Route::redirect("/hop", "/final?token=SECRET"),
        Route::html("/final", "<html><body>here</body></html>"),
    ]);
    let (mut engine, view) = engine(EngineConfig::default(), None);
    runtime().block_on(engine.load_url(view, server.url("/"))).unwrap();
    let records = records(&engine, view);
    assert_each_request_answered_once(&records);
    let lines: Vec<String> = records.iter().map(|r| describe(r, server.port)).take(6).collect();
    assert_eq!(
        lines,
        vec![
            "navigation_start /",
            "request #1 GET document / by=navigation",
            "navigation_redirect / -> /hop 302",
            "navigation_redirect /hop -> /final 302",
            "response #1 200 text/html",
            // The committed document is the last hop's (Z row S1); the
            // query stays off the log as everywhere else.
            "navigation_commit /final 200",
        ]
    );
    assert!(!serde_json::to_string(&records).unwrap().contains("SECRET"));
}

#[test]
fn a_document_that_cannot_be_reached_is_a_connect_failure() {
    let closed = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    };
    let (mut engine, view) = engine(EngineConfig::default(), None);
    let url = Url::parse(&format!("http://127.0.0.1:{closed}/")).unwrap();
    assert!(runtime().block_on(engine.load_url(view, url)).is_err());
    let lines: Vec<String> = records(&engine, view).iter().map(|r| describe(r, closed)).collect();
    assert_eq!(
        lines,
        vec![
            "navigation_start /",
            "request #1 GET document / by=navigation",
            "failure #1 connect could not connect",
            "load_ended_early Failed",
        ]
    );
}

#[test]
fn an_image_past_the_subresource_budget_is_over_budget() {
    let server = serve(vec![
        Route::html("/", "<html><body><img src=\"/slow.png\"></body></html>"),
        png("/slow.png").delay(1500),
    ]);
    let config = EngineConfig { subresource_budget_ms: 200, ..EngineConfig::default() };
    let (mut engine, view) = engine(config, None);
    runtime().block_on(engine.load_url(view, server.url("/"))).unwrap();
    let records = records(&engine, view);
    assert_each_request_answered_once(&records);
    assert!(outcomes(&records, server.port).contains(&"image /slow.png by=parser -> failed over_budget".to_string()));
}

#[test]
fn an_image_that_does_not_decode_is_a_decode_failure() {
    let (engine, view, server) = load(vec![
        Route::html("/", "<html><body><img src=\"/broken.png\"></body></html>"),
        Route::ok("/broken.png", "image/png", "these are not image bytes"),
    ]);
    let records = records(&engine, view);
    assert_each_request_answered_once(&records);
    assert!(outcomes(&records, server.port).contains(&"image /broken.png by=parser -> failed decode".to_string()));
}

/// The loader's memory cache answers a repeated document GET. (Loopback
/// subresources are never cached: the shared cache would answer a private
/// URL without connecting, so it skips them.)
#[test]
fn a_repeated_get_is_answered_from_the_cache() {
    let server = serve(vec![Route::html("/", "<html><body>x</body></html>").header("Cache-Control", "max-age=600")]);
    let (mut engine, view) = engine(EngineConfig::default(), None);
    let rt = runtime();
    rt.block_on(engine.load_url(view, server.url("/"))).unwrap();
    rt.block_on(engine.load_url(view, server.url("/"))).unwrap();
    let records = records(&engine, view);
    assert_each_request_answered_once(&records);
    let documents: Vec<String> = records
        .iter()
        .map(|r| describe(r, server.port))
        .filter(|l| l.starts_with("response"))
        .collect();
    assert_eq!(documents, vec!["response #1 200 text/html", "response #2 200 text/html cache"]);
    assert_eq!(server.hits.load(std::sync::atomic::Ordering::SeqCst), 1);
    let encoded: Vec<Option<u64>> = records
        .iter()
        .filter_map(|r| match r.event {
            InspectorEvent::Response { encoded_size, .. } => Some(encoded_size),
            _ => None,
        })
        .collect();
    assert_eq!(encoded, vec![Some(27), None], "the cache transferred nothing");
}

// ---- Privacy ----

#[test]
fn no_header_or_body_reaches_the_log_and_urls_lose_their_query() {
    let page = r#"<html><body><img src="/pic.png?user=ALICE#frag"><script>
fetch('/api?key=APIKEY', { method: 'POST', headers: { 'Authorization': 'Bearer AUTHTOKEN', 'X-Custom': 'CUSTOMHEADER' }, body: 'REQUESTBODY' });
fetch('/odd');
fetch('/alice');
var x = new XMLHttpRequest(); x.open('GET', '/odd'); x.send();
</script></body></html>"#;
    let (engine, view, _server) = load(vec![
        Route::html("/", page).header("Set-Cookie", "sid=SETCOOKIE"),
        png("/pic.png"),
        Route::ok("/api", "application/json", "{\"secret\":\"RESPONSEBODY\"}").header("Set-Cookie", "t=APICOOKIE"),
        // A malformed Content-Type is header text, not a MIME essence.
        Route::ok("/odd", "application/json, token=SECRET_CT", "{}"),
        // A well-formed type the server made up (third R1 hold @ 43dd085c).
        Route::ok("/alice", "vnd.user-ALICE/json", "{}"),
    ]);
    let records = records(&engine, view);
    let odd: Vec<Option<String>> = records
        .iter()
        .filter_map(|r| match &r.event {
            InspectorEvent::Response { mime, .. } if mime.as_deref() != Some("text/html") && mime.as_deref() != Some("image/png") && mime.as_deref() != Some("application/json") => Some(mime.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(odd, vec![None, None, None], "{records:?}");
    let all = serde_json::to_string(&records).unwrap();
    assert!(!all.to_ascii_lowercase().contains("secret_ct"), "the header leaked: {all}");
    for secret in [
        "ALICE", "frag", "APIKEY", "AUTHTOKEN", "Authorization", "CUSTOMHEADER", "REQUESTBODY", "RESPONSEBODY", "SETCOOKIE",
        "APICOOKIE", "Set-Cookie",
    ] {
        assert!(!all.contains(secret), "{secret} leaked: {all}");
    }
    assert!(all.contains("\"method\":\"POST\""));
}

#[test]
fn the_summary_counts_requests_by_type_and_outcome() {
    let page = r#"<html><body><img src="/pic.png"><img src="/missing.png"><script src="/gone.js"></script></body></html>"#;
    let (engine, view, _server) = load(vec![Route::html("/", page), png("/pic.png")]);
    let records = records(&engine, view);
    let summary = InspectorSummary::from_records(&records, &[]);
    let requests = serde_json::to_string(&summary.requests).unwrap();
    assert_eq!(requests, r#"{"document":{"2xx":1},"image":{"2xx":1,"4xx":1},"script":{"4xx":1}}"#);
    assert!(summary.failures_by_stage.is_empty());
}

/// Third R1 hold @ 43dd085c, item 1: a `;` path parameter (a servlet's
/// `jsessionid`) reaches no request URL and no `navigation_redirect`.
#[test]
fn request_and_redirect_urls_lose_their_path_parameters() {
    let page = r#"<html><body><img src="/a.png;jsessionid=JSESSIONSECRET?sig=Q">
<link rel=stylesheet href="/s.css;jsessionid=JSESSIONSECRET">
<script src="/s.js;jsessionid=JSESSIONSECRET"></script><script>
fetch('/f;jsessionid=JSESSIONSECRET?x=1');
var x = new XMLHttpRequest(); x.open('GET', '/ct1;p=JSESSIONSECRET'); x.send();
</script></body></html>"#;
    let server = serve(vec![
        Route::redirect("/start", "/;jsessionid=JSESSIONSECRET?token=Q"),
        Route::html("/;jsessionid=JSESSIONSECRET", page),
        png("/a.png;jsessionid=JSESSIONSECRET"),
        Route::ok("/s.css;jsessionid=JSESSIONSECRET", "text/css", "body{}"),
        Route::ok("/s.js;jsessionid=JSESSIONSECRET", "text/javascript", "1;"),
        Route::ok("/f;jsessionid=JSESSIONSECRET", "application/json", "{}"),
        Route::ok("/ct1;p=JSESSIONSECRET", "text/plain", "x"),
    ]);
    let (mut engine, view) = engine(EngineConfig::default(), None);
    runtime().block_on(engine.load_url(view, server.url("/start"))).expect("load_url");
    runtime().block_on(engine.pump_live(view, 50));
    let records = records(&engine, view);
    let all = serde_json::to_string(&records).unwrap();
    assert!(!all.contains("JSESSIONSECRET") && !all.contains("jsessionid"), "{all}");
    let port = server.port;
    for url in ["/a.png", "/s.css", "/s.js", "/f", "/ct1"] {
        assert!(all.contains(&format!("\"url\":\"http://127.0.0.1:{port}{url}\"")), "{url} not requested: {all}");
    }
    assert!(all.contains(&format!("\"to\":\"http://127.0.0.1:{port}/\"")), "{all}");
}

/// Dev-tools T2 acceptance, one page: a request the shield refuses (with
/// its list and rule), six that never answer (each `timeout`), and a
/// seventh to the same origin queued behind the per-origin cap of six
/// until the deadline: `timeout` with the reason `in-flight cap` and
/// `queued_ms` about the whole wait. Takes the 30 s live request timeout.
#[test]
fn a_shield_block_a_timeout_and_a_cap_overflow_each_record_their_result() {
    struct EasyListAds;
    impl rustkit_net::InterceptHandler for EasyListAds {
        fn intercept(&self, request: &rustkit_net::Request) -> rustkit_net::InterceptAction {
            if request.url.path().starts_with("/ads/") {
                rustkit_net::InterceptAction::BlockedBy(rustkit_net::BlockReason {
                    list: "EasyList".into(),
                    rule: Some("/ads/*".into()),
                })
            } else {
                rustkit_net::InterceptAction::Allow
            }
        }
    }
    const HANG_MS: u64 = 60_000;
    let server = serve(vec![
        Route::html("/", "<html><body>fixture</body></html>"),
        Route::ok("/ads/x.js", "text/javascript", ""),
        Route::ok("/hang/0", "text/plain", "late").delay(HANG_MS),
        Route::ok("/hang/1", "text/plain", "late").delay(HANG_MS),
        Route::ok("/hang/2", "text/plain", "late").delay(HANG_MS),
        Route::ok("/hang/3", "text/plain", "late").delay(HANG_MS),
        Route::ok("/hang/4", "text/plain", "late").delay(HANG_MS),
        Route::ok("/hang/5", "text/plain", "late").delay(HANG_MS),
        Route::ok("/hang/6", "text/plain", "late").delay(HANG_MS),
    ]);
    let mut shield = rustkit_net::RequestInterceptor::new();
    shield.add_handler(Arc::new(EasyListAds));
    let (mut engine, view) = engine(EngineConfig::default(), Some(shield));
    let rt = runtime();
    rt.block_on(engine.load_url(view, server.url("/"))).unwrap();
    engine
        .execute_script(
            view,
            "fetch('/ads/x.js').catch(function () {});
for (var i = 0; i < 7; i++) fetch('/hang/' + i).catch(function () {});
0",
        )
        .unwrap();
    let started = std::time::Instant::now();
    while started.elapsed() < std::time::Duration::from_secs(45) {
        rt.block_on(engine.pump_live(view, 200));
        let answered = records(&engine, view)
            .iter()
            .filter(|r| matches!(r.event, InspectorEvent::Failure { .. }))
            .count();
        if answered >= 8 {
            break;
        }
    }
    let records = records(&engine, view);
    assert_each_request_answered_once(&records);
    let local = |s: &str| s.replace(&format!("http://127.0.0.1:{}", server.port), "");
    let mut urls = std::collections::BTreeMap::new();
    let mut ended = Vec::new();
    let mut queued = std::collections::BTreeMap::new();
    for record in &records {
        match &record.event {
            InspectorEvent::Request { id, url, .. } => {
                urls.insert(*id, local(url));
            }
            InspectorEvent::Failure { id, stage, reason, blocked_by, queued_ms, .. } => {
                let by = blocked_by.as_ref().map(|b| format!(" by {} {}", b.list, b.rule.as_deref().unwrap_or("-")));
                ended.push(format!("{} {} {}{}", urls[id], stage.as_str(), reason, by.unwrap_or_default()));
                queued.insert(urls[id].clone(), *queued_ms);
            }
            _ => {}
        }
    }
    assert_eq!(
        sorted(ended),
        vec![
            "/ads/x.js policy blocked by the shield by EasyList /ads/*",
            "/hang/0 timeout timed out",
            "/hang/1 timeout timed out",
            "/hang/2 timeout timed out",
            "/hang/3 timeout timed out",
            "/hang/4 timeout timed out",
            "/hang/5 timeout timed out",
            "/hang/6 timeout in-flight cap",
        ]
    );
    let capped = queued["/hang/6"].expect("the capped request has queued_ms");
    assert!(capped >= 25_000.0, "queued for the whole wait: {capped}");
    for path in ["/hang/0", "/hang/1", "/hang/2", "/hang/3", "/hang/4"] {
        let q = queued[path].expect("a script request has queued_ms");
        assert!(q < 1_000.0, "{path} queued {q} ms");
    }
}

/// What asked for a request: `fetch()` and XHR name themselves, and a
/// `<script src>` page script inserted is `inserted_script`, apart from
/// the markup's (`parser`).
#[test]
fn script_made_requests_name_the_api_that_made_them() {
    let page = "<html><body><script src=\"/parsed.js\"></script><script>
fetch('/f').catch(function () {});
var x = new XMLHttpRequest(); x.open('GET', '/x'); x.send();
var s = document.createElement('script'); s.src = '/ins.js'; document.body.appendChild(s);
</script></body></html>";
    let server = serve(vec![
        Route::html("/", page),
        Route::ok("/parsed.js", "text/javascript", "0"),
        Route::ok("/ins.js", "text/javascript", "0"),
        Route::ok("/f", "text/plain", "f"),
        Route::ok("/x", "text/plain", "x"),
    ]);
    let (mut engine, view) = engine(EngineConfig::default(), None);
    let rt = runtime();
    rt.block_on(engine.load_url(view, server.url("/"))).unwrap();
    // Live turns until the four have been answered (the script-made ones
    // finish on later turns), at most about 3 s.
    for _ in 0..60 {
        rt.block_on(engine.pump_live(view, 50));
        let answered = records(&engine, view)
            .iter()
            .filter(|r| matches!(r.event, InspectorEvent::Response { .. } | InspectorEvent::Failure { .. }))
            .count();
        if answered >= 5 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let records = records(&engine, view);
    assert_each_request_answered_once(&records);
    let mut requests: Vec<String> = records
        .iter()
        .map(|r| describe(r, server.port))
        .filter(|l| l.starts_with("request") && !l.contains(" document "))
        .map(|l| l.splitn(3, ' ').nth(2).unwrap_or_default().to_string())
        .collect();
    requests.sort();
    assert_eq!(
        requests,
        vec![
            "GET fetch /f by=fetch",
            "GET script /ins.js by=inserted_script",
            "GET script /parsed.js by=parser",
            "GET xhr /x by=xhr",
        ]
    );
}
