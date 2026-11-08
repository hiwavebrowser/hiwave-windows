//! An HTTP error response's page runs like any other document: the body of a
//! 403, a 404 or a 503 gets the same script, network, cookie and navigation
//! behaviour as a 200's. Only the status it was served with differs.
//!
//! Until 2026-10-16 a script such a page inserted (`<script src>` appended
//! by an inline script) was never fetched, `location.reload()` did nothing,
//! `<meta http-equiv=refresh>` was ignored and the engine kept no cookies,
//! so a page that checks the browser and then reloads itself stayed on its
//! first answer. None of that was specific to error pages; the fixtures
//! here are error pages because that is where it showed.

use super::*;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Mutex;

/// What the server answers for a path, given the request's Cookie header.
pub(crate) type Handler = fn(path: &str, cookie: &str) -> (u16, &'static str, Vec<(&'static str, &'static str)>, String);

pub(crate) struct Server {
    port: u16,
    /// (request target, Cookie header) in arrival order.
    seen: Arc<Mutex<Vec<(String, String)>>>,
}

impl Server {
    pub(crate) fn url(&self, path: &str) -> Url {
        Url::parse(&format!("http://127.0.0.1:{}{path}", self.port)).unwrap()
    }
    pub(crate) fn requests(&self) -> Vec<(String, String)> {
        self.seen.lock().unwrap().clone()
    }
}

pub(crate) fn serve(handler: Handler) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let log = log.clone();
            std::thread::spawn(move || {
                let mut request = Vec::new();
                let mut buf = [0u8; 2048];
                while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                    match stream.read(&mut buf) {
                        Ok(0) | Err(_) => return,
                        Ok(n) => request.extend_from_slice(&buf[..n]),
                    }
                }
                let head = String::from_utf8_lossy(&request).to_string();
                let target = head.split_whitespace().nth(1).unwrap_or("/").to_string();
                let cookie = head
                    .lines()
                    .find_map(|l| l.split_once(':').filter(|(n, _)| n.eq_ignore_ascii_case("cookie")))
                    .map(|(_, v)| v.trim().to_string())
                    .unwrap_or_default();
                log.lock().unwrap().push((target.clone(), cookie.clone()));
                let path = target.split('?').next().unwrap_or("/");
                let (status, content_type, headers, body) = handler(path, &cookie);
                let mut out = format!(
                    "HTTP/1.1 {status} Status\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n",
                    body.len()
                );
                for (name, value) in headers {
                    out.push_str(&format!("{name}: {value}\r\n"));
                }
                out.push_str("\r\n");
                let _ = stream.write_all(out.as_bytes());
                let _ = stream.write_all(body.as_bytes());
            });
        }
    });
    Server { port, seen }
}

pub(crate) fn engine() -> (Engine, EngineViewId, tokio::runtime::Runtime) {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 300, height: 200 })
        .expect("view");
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    (engine, view, rt)
}

pub(crate) fn text(engine: &mut Engine, view: EngineViewId, js: &str) -> String {
    engine.execute_script(view, js).unwrap()
}

/// A 403 whose inline script inserts `<script src>`. That script sets a
/// cookie and reloads; the server answers the reload, which carries the
/// cookie (and the one the 403 set), with the real page.
fn checking_site(path: &str, cookie: &str) -> (u16, &'static str, Vec<(&'static str, &'static str)>, String) {
    match path {
        "/" if cookie.contains("passed=1") => (
            200,
            "text/html",
            vec![],
            "<html><head><title>Welcome</title></head><body><p>the site</p></body></html>".into(),
        ),
        "/" => (
            403,
            "text/html",
            vec![("Set-Cookie", "visit=abc; Path=/; HttpOnly")],
            r#"<html><head><title>Just a moment</title></head><body><p>checking</p>
<script>
  window.ranInline = true;
  var s = document.createElement('script');
  s.src = '/check.js';
  s.onload = function () { window.loaded = true; };
  document.getElementsByTagName('head')[0].appendChild(s);
</script></body></html>"#
                .into(),
        ),
        "/check.js" => (
            200,
            "text/javascript",
            vec![],
            "window.checkRan = document.currentScript && document.currentScript.src;\n\
             document.cookie = 'passed=1; path=/';\n\
             location.reload();"
                .into(),
        ),
        _ => (404, "text/plain", vec![], "not found".into()),
    }
}

#[test]
fn a_403_page_runs_the_script_it_inserts_and_its_reload_carries_the_cookies() {
    let server = serve(checking_site);
    let (mut engine, view, rt) = engine();
    rt.block_on(engine.load_url(view, server.url("/"))).expect("a 403 with a page is a page");
    assert_eq!(engine.http_status(view), Some(403));
    assert_eq!(text(&mut engine, view, "String(window.ranInline)"), r#"String("true")"#);
    assert_eq!(
        text(&mut engine, view, "String(window.checkRan)"),
        format!(r#"String("{}")"#, server.url("/check.js")),
        "the inserted script was fetched and run, as the current script"
    );
    assert_eq!(text(&mut engine, view, "String(window.loaded)"), r#"String("true")"#, "its element heard load");
    assert_eq!(
        text(&mut engine, view, "document.cookie"),
        r#"String("passed=1")"#,
        "document.cookie reads the jar; the HttpOnly cookie stays out of script's reach"
    );
    let (reload, replace) = engine.take_script_navigation_with_replace(view).expect("location.reload() asks to navigate");
    assert!(replace, "a reload replaces the history entry: Back does not return to the 403");
    assert_eq!(reload, server.url("/").to_string());

    rt.block_on(engine.load_url(view, Url::parse(&reload).unwrap())).expect("the reload");
    assert_eq!(engine.http_status(view), Some(200));
    assert_eq!(engine.views[&view].title.as_deref(), Some("Welcome"));
    assert!(engine.views[&view].layout.is_some(), "the real page is laid out");
    let requests = server.requests();
    let paths: Vec<&str> = requests.iter().map(|(p, _)| p.as_str()).collect();
    assert_eq!(paths, vec!["/", "/check.js", "/"]);
    assert_eq!(requests[1].1, "visit=abc", "the inserted script's fetch carries the response's cookie");
    assert_eq!(requests[2].1, "visit=abc; passed=1", "the reload carries both cookies");
}

/// The same page served with 200 behaves the same way: none of this is
/// about the status.
#[test]
fn a_200_page_runs_inserted_scripts_the_same_way() {
    fn site(path: &str, cookie: &str) -> (u16, &'static str, Vec<(&'static str, &'static str)>, String) {
        let (status, ty, headers, body) = checking_site(path, cookie);
        (if status == 403 { 200 } else { status }, ty, headers, body)
    }
    let server = serve(site);
    let (mut engine, view, rt) = engine();
    rt.block_on(engine.load_url(view, server.url("/"))).expect("load");
    assert_eq!(engine.http_status(view), Some(200));
    assert_eq!(text(&mut engine, view, "String(window.loaded)"), r#"String("true")"#);
    assert_eq!(engine.take_script_navigation(view), Some(server.url("/").to_string()));
}

/// A 503 with `<meta http-equiv=refresh>` asks to go where the meta says
/// once the page has loaded.
#[test]
fn a_503_page_with_a_meta_refresh_navigates() {
    fn site(path: &str, _: &str) -> (u16, &'static str, Vec<(&'static str, &'static str)>, String) {
        match path {
            "/" => (
                503,
                "text/html",
                vec![],
                r#"<html><head><meta http-equiv="refresh" content="0; url=/later"><title>Busy</title></head><body>try again</body></html>"#.into(),
            ),
            _ => (200, "text/html", vec![], "<html><head><title>Later</title></head><body>ok</body></html>".into()),
        }
    }
    let server = serve(site);
    let (mut engine, view, rt) = engine();
    rt.block_on(engine.load_url(view, server.url("/"))).expect("a 503 with a page is a page");
    assert_eq!(engine.http_status(view), Some(503));
    assert_eq!(engine.views[&view].title.as_deref(), Some("Busy"));
    let (next, replace) = engine.take_script_navigation_with_replace(view).expect("the refresh asks to navigate");
    assert!(replace, "a declarative refresh replaces the history entry");
    assert_eq!(next, server.url("/later").to_string());
    rt.block_on(engine.load_url(view, Url::parse(&next).unwrap())).expect("follow the refresh");
    assert_eq!(engine.views[&view].title.as_deref(), Some("Later"));
}

/// A refresh with no URL reloads the same document, after its delay.
#[test]
fn a_meta_refresh_without_a_url_reloads_after_its_delay() {
    fn site(_: &str, _: &str) -> (u16, &'static str, Vec<(&'static str, &'static str)>, String) {
        (503, "text/html", vec![], r#"<meta http-equiv="Refresh" content="10"><p>busy</p>"#.into())
    }
    let server = serve(site);
    let (mut engine, view, rt) = engine();
    rt.block_on(engine.load_url(view, server.url("/wait"))).expect("load");
    assert_eq!(engine.take_script_navigation(view), None, "not before its ten seconds");
    rt.block_on(engine.pump_live(view, 10_000));
    assert_eq!(engine.take_script_navigation(view), Some(server.url("/wait").to_string()));
}

/// A 404 page's script fetches JSON from its own origin like any page.
#[test]
fn a_404_page_fetches_json() {
    fn site(path: &str, _: &str) -> (u16, &'static str, Vec<(&'static str, &'static str)>, String) {
        match path {
            "/data.json" => (200, "application/json", vec![], r#"{"name":"found it"}"#.into()),
            _ => (
                404,
                "text/html",
                vec![],
                r#"<html><body><p id="out">missing</p><script>
fetch('/data.json').then(function (r) { return r.json(); }).then(function (d) {
  document.getElementById('out').textContent = d.name;
});
</script></body></html>"#
                    .into(),
            ),
        }
    }
    let server = serve(site);
    let (mut engine, view, rt) = engine();
    rt.block_on(engine.load_url(view, server.url("/nope"))).expect("a 404 with a page is a page");
    assert_eq!(engine.http_status(view), Some(404));
    assert_eq!(
        text(&mut engine, view, "document.getElementById('out').textContent"),
        r#"String("found it")"#
    );
}

/// What else script navigates with, on an error page: `location.replace`
/// and `assign`, and assigning the document's own URL (a reload, not a
/// fragment navigation).
#[test]
fn location_replace_assign_and_href_to_itself_navigate() {
    fn site(_: &str, _: &str) -> (u16, &'static str, Vec<(&'static str, &'static str)>, String) {
        (404, "text/html", vec![], "<p>not here</p>".into())
    }
    let server = serve(site);
    let (mut engine, view, rt) = engine();
    rt.block_on(engine.load_url(view, server.url("/page"))).expect("load");
    // The URL, and whether it replaces the history entry (Chrome: replace,
    // reload and the document's own URL do; an assign elsewhere adds one).
    for (js, want, replace) in [
        ("location.replace('/a')", "/a", true),
        ("location.assign('/b?x=1')", "/b?x=1", false),
        ("location.href = '/c'", "/c", false),
        ("location.href = location.href", "/page", true),
        ("location.assign(location.href)", "/page", true),
        ("location.reload()", "/page", true),
        ("location.href = '#top'", "", false),
    ] {
        text(&mut engine, view, js);
        let want = (!want.is_empty()).then(|| (server.url(want).to_string(), replace));
        assert_eq!(engine.take_script_navigation_with_replace(view), want, "{js}");
    }
}

/// A script element is run once: moving an inserted script, or inserting
/// one the parser already ran, runs nothing again; a script set to a type
/// that is not JavaScript is not run; one whose fetch fails hears `error`.
#[test]
fn inserted_scripts_run_once_and_only_when_they_are_javascript() {
    fn site(path: &str, _: &str) -> (u16, &'static str, Vec<(&'static str, &'static str)>, String) {
        match path {
            "/" => (
                200,
                "text/html",
                vec![],
                r#"<html><head><script id="p">window.runs = (window.runs || 0) + 1;</script></head><body><script>
var log = [];
function add(src, type) {
  var s = document.createElement('script');
  if (type) s.type = type;
  s.src = src;
  s.onload = function () { log.push('load ' + src); };
  s.onerror = function () { log.push('error ' + src); };
  document.body.appendChild(s);
  return s;
}
var a = add('/a.js');
document.head.appendChild(a);
add('/data.txt', 'text/plain');
add('/gone.js');
document.body.appendChild(document.getElementById('p'));
var inline = document.createElement('script');
inline.textContent = 'log.push("inline " + (document.currentScript === inline));';
document.body.appendChild(inline);
</script></body></html>"#
                    .into(),
            ),
            "/a.js" => (200, "text/javascript", vec![], "log.push('ran a');".into()),
            "/data.txt" => (200, "text/plain", vec![], "log.push('ran data');".into()),
            _ => (404, "text/plain", vec![], "".into()),
        }
    }
    let server = serve(site);
    let (mut engine, view, rt) = engine();
    rt.block_on(engine.load_url(view, server.url("/"))).expect("load");
    let got = text(&mut engine, view, "log.join(',') + ' runs=' + runs");
    let body = got.trim_start_matches("String(\"").trim_end_matches("\")");
    assert!(body.starts_with("inline true,"), "{got}");
    assert!(body.contains("ran a") && body.contains("load /a.js") && body.contains("error /gone.js"), "{got}");
    assert!(body.find("ran a").unwrap() < body.find("load /a.js").unwrap(), "{got}");
    assert!(!body.contains("ran data"), "{got}");
    assert!(body.ends_with("runs=1"), "{got}");
    // The two external scripts are fetched together, so either may finish,
    // and run, first: a script the page inserted is async unless `async`
    // was set false. `/a.js` still runs before its own `load`.
    let mut paths: Vec<String> = server.requests().into_iter().map(|(p, _)| p).collect();
    paths.sort();
    assert_eq!(paths, vec!["/", "/a.js", "/gone.js"]);
}

/// A script inserted after the load, from a timer, is fetched and run by
/// the live loop.
#[test]
fn a_script_inserted_after_the_load_runs_in_a_live_turn() {
    fn site(path: &str, _: &str) -> (u16, &'static str, Vec<(&'static str, &'static str)>, String) {
        match path {
            "/" => (
                403,
                "text/html",
                vec![],
                r#"<p>wait</p><script>
setTimeout(function () {
  var s = document.createElement('script');
  s.src = '/late.js';
  document.head.appendChild(s);
}, 60000);
</script>"#
                    .into(),
            ),
            "/late.js" => (200, "text/javascript", vec![], "window.late = 'ran';".into()),
            _ => (404, "text/plain", vec![], "".into()),
        }
    }
    let server = serve(site);
    let (mut engine, view, rt) = engine();
    rt.block_on(engine.load_url(view, server.url("/"))).expect("load");
    assert_eq!(text(&mut engine, view, "String(window.late)"), r#"String("undefined")"#);
    // The timer inserts the script on the first turn. A turn does not wait
    // the fetch out, so a later turn may be the one that runs it.
    let started = std::time::Instant::now();
    let mut elapsed = 60_000;
    loop {
        rt.block_on(engine.pump_live(view, elapsed));
        elapsed = 10;
        if text(&mut engine, view, "String(window.late)") == r#"String("ran")"# {
            break;
        }
        assert!(
            started.elapsed() < std::time::Duration::from_secs(3),
            "the live loop never ran the script inserted after the load"
        );
    }
}

/// A load cancelled while an inserted script is being fetched runs none of
/// it: the M3b cancel reaches the inserted-script fetch like every other.
#[test]
fn a_cancelled_load_does_not_run_the_script_it_inserted() {
    static ASKED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    fn site(path: &str, _: &str) -> (u16, &'static str, Vec<(&'static str, &'static str)>, String) {
        match path {
            "/slow.js" => {
                ASKED.store(true, std::sync::atomic::Ordering::SeqCst);
                std::thread::sleep(std::time::Duration::from_millis(400));
                (200, "text/javascript", vec![], "window.slowRan = true;".into())
            }
            _ => (
                403,
                "text/html",
                vec![],
                r#"<p>x</p><script>var s = document.createElement('script'); s.src = '/slow.js'; document.head.appendChild(s);</script>"#.into(),
            ),
        }
    }
    let server = serve(site);
    let (mut engine, view, rt) = engine();
    let canceller = engine.nav_canceller(view).expect("canceller");
    // Cancelled once the inserted script's request is out.
    let cancel = std::thread::spawn(move || {
        let started = std::time::Instant::now();
        while !ASKED.load(std::sync::atomic::Ordering::SeqCst) && started.elapsed().as_secs() < 30 {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        canceller.cancel_current();
    });
    let _ = rt.block_on(engine.load_url(view, server.url("/")));
    cancel.join().unwrap();
    // A cancelled page's script may refuse to run at all; it must not have
    // run the inserted one.
    let ran = engine.execute_script(view, "String(window.slowRan)").unwrap_or_default();
    assert_ne!(ran, r#"String("true")"#);
    assert!(
        server.requests().iter().any(|(p, _)| p == "/slow.js"),
        "the cancel came while the inserted script was being fetched"
    );
}

/// An inserted script whose fetch outlasts the script budget still runs
/// during the load. The wait is the network cap, so the element hears
/// `load` before `readyState` becomes `complete`.
#[test]
fn an_inserted_script_slower_than_the_script_budget_still_runs() {
    fn site(path: &str, _: &str) -> (u16, &'static str, Vec<(&'static str, &'static str)>, String) {
        match path {
            "/slow.js" => {
                std::thread::sleep(std::time::Duration::from_millis(600));
                (200, "text/javascript", vec![], "L('/slow.js ran'); window.slowRan = true;".into())
            }
            _ => (
                403,
                "text/html",
                vec![],
                r#"<p>x</p><pre id="log"></pre><script>
function L(m) { document.getElementById('log').textContent += m + '\n'; }
document.addEventListener('readystatechange', function () { L('readystatechange readyState=' + document.readyState); });
document.addEventListener('DOMContentLoaded', function () { L('DOMContentLoaded readyState=' + document.readyState); });
window.addEventListener('load', function () { L('window load readyState=' + document.readyState); });
L('readyState=' + document.readyState);
var s = document.createElement('script'); s.src = '/slow.js';
s.onload = function () { L('/slow.js load'); }; s.onerror = function () { L('/slow.js error'); };
document.head.appendChild(s);
L('/slow.js inserted');
</script>"#
                    .into(),
            ),
        }
    }
    let server = serve(site);
    let mut engine = Engine::new(EngineConfig { script_budget_ms: 200, ..EngineConfig::default() }).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 300, height: 200 })
        .expect("view");
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    rt.block_on(engine.load_url(view, server.url("/"))).expect("load");
    let log = page_log(&mut engine, view);
    assert_eq!(
        event_trace(&log),
        [
            "readyState=loading",
            "/slow.js inserted",
            "readystatechange readyState=interactive",
            "DOMContentLoaded readyState=interactive",
            "/slow.js ran",
            "/slow.js load",
            "readystatechange readyState=complete",
            "window load readyState=complete",
        ],
        "{log}"
    );
    assert_eq!(text(&mut engine, view, "String(window.slowRan)"), r#"String("true")"#);
    let record = engine.views[&view]
        .script_log
        .iter()
        .find(|r| r.source.ends_with("/slow.js"))
        .expect("the inserted script is in the script log");
    assert_eq!(record.outcome, ScriptOutcome::Ran);
}

/// The page is the reduced loader: an inline script inserts chunk A, whose
/// `onload` inserts chunk B, whose `IntersectionObserver` replaces the feed.
fn chunk_chain_page() -> String {
    r#"<!doctype html><html><head>
<script>
function L(m) { var p = document.getElementById('log'); if (p) p.textContent += m + '\n'; }
document.addEventListener('readystatechange', function () { L('readystatechange readyState=' + document.readyState); });
function insert(src, next) {
  var s = document.createElement('script');
  s.src = src;
  s.async = true;
  s.onload = function () { L(src + ' load'); if (next) next(); };
  s.onerror = function () { L(src + ' error'); };
  document.head.appendChild(s);
  L(src + ' inserted');
}
document.addEventListener('DOMContentLoaded', function () { L('DOMContentLoaded readyState=' + document.readyState); });
window.addEventListener('load', function () { L('window load readyState=' + document.readyState); });
L('readyState=' + document.readyState);
insert('/chunk-a.js', function () { insert('/chunk-b.js'); });
</script></head><body>
<div id="feed"><div class="post">Server-rendered post 1</div><div class="post">Server-rendered post 2</div></div>
<div id="sentinel" style="height:1px"></div>
<pre id="log"></pre>
</body></html>"#
        .into()
}

const CHUNK_A: &str = "\
L('chunk a ran');\n\
document.getElementById('feed').innerHTML = '<div class=\"skeleton\">loading</div>';\n";

const CHUNK_B: &str = "\
L('chunk b ran');\n\
new IntersectionObserver(function (entries) {\n\
  if (!entries[0].isIntersecting) return;\n\
  var feed = document.getElementById('feed');\n\
  feed.innerHTML = '';\n\
  for (var i = 1; i <= 3; i++) {\n\
    var d = document.createElement('div');\n\
    d.className = 'post';\n\
    d.textContent = 'Feed post ' + i;\n\
    feed.appendChild(d);\n\
  }\n\
  L('IO rendered ' + feed.children.length);\n\
}).observe(document.getElementById('sentinel'));\n";

fn chain_engine(budget_ms: u64) -> (Engine, EngineViewId, tokio::runtime::Runtime) {
    let mut engine = Engine::new(EngineConfig { script_budget_ms: budget_ms, ..EngineConfig::default() }).expect("engine");
    // Tall enough that the sentinel is inside the viewport when the feed is short.
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 400, height: 800 })
        .expect("view");
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    (engine, view, rt)
}

fn page_log(engine: &mut Engine, view: EngineViewId) -> String {
    let raw = text(
        engine,
        view,
        r"document.getElementById('log').textContent.replace(/\n/g, '|')",
    );
    raw.trim_start_matches("String(\"").trim_end_matches("\")").to_string()
}

/// The `#log` lines, in order. A trailing newline is not an event.
fn event_trace(log: &str) -> Vec<&str> {
    log.split('|').filter(|line| !line.is_empty()).collect()
}

/// Chrome's order for the two-chunk loader: `DOMContentLoaded` before the
/// chunks, each chunk's `load` before the next chunk, `window` `load` and
/// `readyState` `complete` after the chain (and after the observer, which
/// reports from the layout between the scripts and `load`).
const CHAIN_TRACE: &[&str] = &[
    "readyState=loading",
    "/chunk-a.js inserted",
    "readystatechange readyState=interactive",
    "DOMContentLoaded readyState=interactive",
    "chunk a ran",
    "/chunk-a.js load",
    "/chunk-b.js inserted",
    "chunk b ran",
    "/chunk-b.js load",
    "IO rendered 3",
    "readystatechange readyState=complete",
    "window load readyState=complete",
];

/// A chunk chain runs after `DOMContentLoaded` and before `window` `load`.
/// The whole trace is the order, not only that the posts showed up.
#[test]
fn an_inserted_chunk_chain_runs_after_dom_content_loaded() {
    fn site(path: &str, _: &str) -> (u16, &'static str, Vec<(&'static str, &'static str)>, String) {
        match path {
            "/chunk-a.js" => (200, "text/javascript", vec![], CHUNK_A.into()),
            "/chunk-b.js" => (200, "text/javascript", vec![], CHUNK_B.into()),
            _ => (200, "text/html", vec![], chunk_chain_page()),
        }
    }
    let server = serve(site);
    let (mut engine, view, rt) = chain_engine(5_000);
    rt.block_on(engine.load_url(view, server.url("/"))).expect("load");
    let log = page_log(&mut engine, view);
    assert_eq!(event_trace(&log), CHAIN_TRACE, "{log}");
    let feed = text(&mut engine, view, "document.getElementById('feed').textContent");
    assert!(feed.contains("Feed post 1") && feed.contains("Feed post 2") && feed.contains("Feed post 3"), "{feed}");
    assert_eq!(text(&mut engine, view, "document.readyState"), r#"String("complete")"#);
}

/// The same chain, each chunk held longer than the load's script budget.
/// The wait is the network cap, so both chunks still run during the load,
/// and `window` `load` is still after them. No later turn is required.
#[test]
fn an_inserted_chunk_chain_past_the_budget_renders_on_the_live_loop() {
    fn site(path: &str, _: &str) -> (u16, &'static str, Vec<(&'static str, &'static str)>, String) {
        match path {
            "/chunk-a.js" | "/chunk-b.js" => {
                std::thread::sleep(std::time::Duration::from_millis(500));
                let body = if path == "/chunk-a.js" { CHUNK_A } else { CHUNK_B };
                (200, "text/javascript", vec![], body.into())
            }
            _ => (200, "text/html", vec![], chunk_chain_page()),
        }
    }
    let server = serve(site);
    let (mut engine, view, rt) = chain_engine(200);
    rt.block_on(engine.load_url(view, server.url("/"))).expect("load");
    let log = page_log(&mut engine, view);
    assert_eq!(event_trace(&log), CHAIN_TRACE, "{log}");
    assert!(
        engine.script_log(view).unwrap().iter().all(|record| record.outcome != ScriptOutcome::OverBudget),
        "a slow chunk was charged to the script budget: {:?}",
        engine.script_log(view).unwrap().iter().map(|r| (&r.source, &r.outcome)).collect::<Vec<_>>()
    );
    let feed = text(&mut engine, view, "document.getElementById('feed').textContent");
    assert!(feed.contains("Feed post 1") && feed.contains("Feed post 2") && feed.contains("Feed post 3"), "{feed}");
}

/// Three delayed chunks, each inserted from the previous one's `onload`.
/// Each wait is longer than the script budget and still part of the load:
/// they run in order, and `window` `load` is after the last one.
#[test]
fn an_inserted_chain_of_three_delayed_chunks_runs_in_order() {
    const A: &str = "L('chunk a ran');\n";
    const B: &str = "L('chunk b ran');\n";
    const C: &str = "\
L('chunk c ran');\n\
new IntersectionObserver(function (entries) {\n\
  if (!entries[0].isIntersecting) return;\n\
  var feed = document.getElementById('feed');\n\
  feed.innerHTML = '';\n\
  for (var i = 1; i <= 3; i++) {\n\
    var d = document.createElement('div');\n\
    d.className = 'post';\n\
    d.textContent = 'Feed post ' + i;\n\
    feed.appendChild(d);\n\
  }\n\
  L('IO rendered ' + feed.children.length);\n\
}).observe(document.getElementById('sentinel'));\n";
    fn page() -> String {
        r#"<!doctype html><html><head>
<script>
function L(m) { var p = document.getElementById('log'); if (p) p.textContent += m + '\n'; }
document.addEventListener('readystatechange', function () { L('readystatechange readyState=' + document.readyState); });
function insert(src, next) {
  var s = document.createElement('script');
  s.src = src;
  s.async = true;
  s.onload = function () { L(src + ' load'); if (next) next(); };
  s.onerror = function () { L(src + ' error'); };
  document.head.appendChild(s);
  L(src + ' inserted');
}
document.addEventListener('DOMContentLoaded', function () { L('DOMContentLoaded readyState=' + document.readyState); });
window.addEventListener('load', function () { L('window load readyState=' + document.readyState); });
L('readyState=' + document.readyState);
insert('/chunk-a.js', function () { insert('/chunk-b.js', function () { insert('/chunk-c.js'); }); });
</script></head><body>
<div id="feed"><div class="post">Server-rendered post 1</div></div>
<div id="sentinel" style="height:1px"></div>
<pre id="log"></pre>
</body></html>"#
            .into()
    }
    fn site(path: &str, _: &str) -> (u16, &'static str, Vec<(&'static str, &'static str)>, String) {
        match path {
            "/chunk-a.js" | "/chunk-b.js" | "/chunk-c.js" => {
                std::thread::sleep(std::time::Duration::from_millis(400));
                let body = match path {
                    "/chunk-a.js" => A,
                    "/chunk-b.js" => B,
                    _ => C,
                };
                (200, "text/javascript", vec![], body.into())
            }
            _ => (200, "text/html", vec![], page()),
        }
    }
    let server = serve(site);
    let (mut engine, view, rt) = chain_engine(200);
    rt.block_on(engine.load_url(view, server.url("/"))).expect("load");
    let log = page_log(&mut engine, view);
    assert_eq!(
        event_trace(&log),
        [
            "readyState=loading",
            "/chunk-a.js inserted",
            "readystatechange readyState=interactive",
            "DOMContentLoaded readyState=interactive",
            "chunk a ran",
            "/chunk-a.js load",
            "/chunk-b.js inserted",
            "chunk b ran",
            "/chunk-b.js load",
            "/chunk-c.js inserted",
            "chunk c ran",
            "/chunk-c.js load",
            "IO rendered 3",
            "readystatechange readyState=complete",
            "window load readyState=complete",
        ],
        "{log}"
    );
    let feed = text(&mut engine, view, "document.getElementById('feed').textContent");
    assert!(feed.contains("Feed post 1") && feed.contains("Feed post 2") && feed.contains("Feed post 3"), "{feed}");
}

/// A script inserted by a timer, after `load`, is still fetching when the
/// next navigation starts. The fetch is cancelled; its body does not run
/// on the next page. The first load does not wait the fetch out.
#[test]
fn navigating_away_drops_an_inserted_script_still_fetching() {
    // Set once /slow.js is answered. The load must end before that: a
    // wall-clock limit also counted the debug build's first layout, which
    // on macOS alone took longer than the limit.
    static SLOW_ANSWERED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    fn site(path: &str, _: &str) -> (u16, &'static str, Vec<(&'static str, &'static str)>, String) {
        match path {
            "/slow.js" => {
                std::thread::sleep(std::time::Duration::from_millis(2000));
                SLOW_ANSWERED.store(true, std::sync::atomic::Ordering::SeqCst);
                (200, "text/javascript", vec![], "window.fromFirst = 'ran';".into())
            }
            "/next" => (
                200,
                "text/html",
                vec![],
                "<p id=\"p\">second</p><script>window.page = 'b';</script>".into(),
            ),
            _ => (
                200,
                "text/html",
                vec![],
                r#"<p>first</p><script>
setTimeout(function () {
  var s = document.createElement('script');
  s.src = '/slow.js';
  document.head.appendChild(s);
}, 0);
</script>"#
                    .into(),
            ),
        }
    }
    let server = serve(site);
    let (mut engine, view, rt) = chain_engine(200);
    let started = std::time::Instant::now();
    rt.block_on(engine.load_url(view, server.url("/"))).expect("first load");
    assert!(
        !SLOW_ANSWERED.load(std::sync::atomic::Ordering::SeqCst),
        "the load waited out a script inserted after load: {:?}",
        started.elapsed()
    );
    assert_eq!(text(&mut engine, view, "String(window.fromFirst)"), r#"String("undefined")"#);
    // The fetch was started by the load but runs on its own thread: on the
    // Mac it can still be connecting when the load returns. It must reach
    // the server well before /slow.js answers (2 s), i.e. while in flight.
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(1500);
    while !server.requests().iter().any(|(p, _)| p == "/slow.js") && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(
        server.requests().iter().any(|(p, _)| p == "/slow.js")
            && !SLOW_ANSWERED.load(std::sync::atomic::Ordering::SeqCst),
        "the fetch should be in flight when the load returns"
    );
    rt.block_on(engine.load_url(view, server.url("/next"))).expect("second load");
    let started = std::time::Instant::now();
    while started.elapsed() < std::time::Duration::from_millis(1500) {
        rt.block_on(engine.pump_live(view, 50));
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert_eq!(text(&mut engine, view, "String(window.page)"), r#"String("b")"#);
    assert_eq!(
        text(&mut engine, view, "String(window.fromFirst)"),
        r#"String("undefined")"#,
        "the previous page's script ran after navigation"
    );
    assert!(
        engine.script_log(view).unwrap().iter().all(|record| record.outcome != ScriptOutcome::Ran || !record.source.contains("slow.js")),
        "the dropped script was recorded as run: {:?}",
        engine.script_log(view).unwrap().iter().map(|r| (&r.source, &r.outcome)).collect::<Vec<_>>()
    );
}

/// A fetch that fails after the script budget still fires `error` during
/// the load, before `readyState` is `complete`. It is not over budget.
#[test]
fn a_failed_inserted_fetch_past_the_budget_fires_error() {
    fn site(path: &str, _: &str) -> (u16, &'static str, Vec<(&'static str, &'static str)>, String) {
        match path {
            "/missing.js" => {
                std::thread::sleep(std::time::Duration::from_millis(600));
                (404, "text/plain", vec![], String::new())
            }
            _ => (
                200,
                "text/html",
                vec![],
                r#"<pre id="log"></pre><script>
function L(m) { document.getElementById('log').textContent += m + '\n'; }
document.addEventListener('readystatechange', function () { L('readystatechange readyState=' + document.readyState); });
document.addEventListener('DOMContentLoaded', function () { L('DOMContentLoaded readyState=' + document.readyState); });
window.addEventListener('load', function () { L('window load readyState=' + document.readyState); });
L('readyState=' + document.readyState);
var s = document.createElement('script');
s.src = '/missing.js';
s.onload = function () { L('/missing.js load'); };
s.onerror = function () { L('/missing.js error'); };
document.head.appendChild(s);
L('/missing.js inserted');
</script>"#
                    .into(),
            ),
        }
    }
    let server = serve(site);
    let (mut engine, view, rt) = chain_engine(200);
    rt.block_on(engine.load_url(view, server.url("/"))).expect("load");
    let log = page_log(&mut engine, view);
    assert_eq!(
        event_trace(&log),
        [
            "readyState=loading",
            "/missing.js inserted",
            "readystatechange readyState=interactive",
            "DOMContentLoaded readyState=interactive",
            "/missing.js error",
            "readystatechange readyState=complete",
            "window load readyState=complete",
        ],
        "{log}"
    );
    assert!(
        engine.script_log(view).unwrap().iter().any(|record| {
            record.source.contains("missing.js") && matches!(record.outcome, ScriptOutcome::FetchFailed(_))
        }),
        "the failure was not recorded: {:?}",
        engine.script_log(view).unwrap().iter().map(|r| (&r.source, &r.outcome)).collect::<Vec<_>>()
    );
}

/// Two external scripts the page inserted, the first held on the server.
/// `ordered` sets `async = false` on both, which is the in-order list.
fn race_page(ordered: bool) -> String {
    let set = if ordered {
        "s.async = false; log.push('set-async ' + s.async + ' ' + s.hasAttribute('async'));"
    } else {
        ""
    };
    format!(
        r#"<!doctype html><head></head><body><script>
var log = [];
log.push('default-async ' + document.createElement('script').async);
function add(src) {{
  var s = document.createElement('script');
  {set}
  s.src = src;
  s.onload = function () {{ log.push('load ' + src); }};
  s.onerror = function () {{ log.push('error ' + src); }};
  document.head.appendChild(s);
}}
add('/slow.js');
add('/fast.js');
</script></body></html>"#
    )
}

fn race_handler(ordered: bool, path: &str) -> (u16, &'static str, Vec<(&'static str, &'static str)>, String) {
    match path {
        "/slow.js" => {
            std::thread::sleep(std::time::Duration::from_millis(400));
            (200, "text/javascript", vec![], "log.push('ran /slow.js');".into())
        }
        "/fast.js" => (200, "text/javascript", vec![], "log.push('ran /fast.js');".into()),
        _ => (200, "text/html", vec![], race_page(ordered)),
    }
}

fn async_race(path: &str, _: &str) -> (u16, &'static str, Vec<(&'static str, &'static str)>, String) {
    race_handler(false, path)
}

fn ordered_race(path: &str, _: &str) -> (u16, &'static str, Vec<(&'static str, &'static str)>, String) {
    race_handler(true, path)
}

fn script_log_text(engine: &mut Engine, view: EngineViewId) -> String {
    let raw = text(engine, view, "log.join(',')");
    raw.trim_start_matches("String(\"").trim_end_matches("\")").to_string()
}

/// A script the page inserted is async unless `async` was set false. The
/// one whose fetch finishes first runs first, even if it was inserted second.
#[test]
fn a_fast_inserted_script_runs_before_a_slower_one_inserted_earlier() {
    let server = serve(async_race);
    let (mut engine, view, rt) = engine();
    rt.block_on(engine.load_url(view, server.url("/"))).expect("load");
    let log = script_log_text(&mut engine, view);
    assert!(log.contains("default-async true"), "{log}");
    let at = |needle: &str| log.find(needle).unwrap_or_else(|| panic!("{needle} missing from {log}"));
    assert!(at("ran /fast.js") < at("load /fast.js"), "{log}");
    assert!(at("load /fast.js") < at("ran /slow.js"), "{log}");
    assert!(at("ran /slow.js") < at("load /slow.js"), "{log}");
}

/// `async = false` is the in-order list: the script inserted first runs
/// first, even when its fetch is the one that finishes second.
#[test]
fn scripts_with_async_false_run_in_insertion_order() {
    let server = serve(ordered_race);
    let (mut engine, view, rt) = engine();
    rt.block_on(engine.load_url(view, server.url("/"))).expect("load");
    let log = script_log_text(&mut engine, view);
    assert!(log.contains("default-async true"), "{log}");
    assert!(log.contains("set-async false false"), "{log}");
    let at = |needle: &str| log.find(needle).unwrap_or_else(|| panic!("{needle} missing from {log}"));
    assert!(at("ran /slow.js") < at("load /slow.js"), "{log}");
    assert!(at("load /slow.js") < at("ran /fast.js"), "{log}");
    assert!(at("ran /fast.js") < at("load /fast.js"), "{log}");
}

/// One external script, no chunk chain. The trace is the whole order:
/// `readyState`, `DOMContentLoaded`, the script, its `load`, then
/// `window` `load`.
#[test]
fn an_inserted_script_has_the_chrome_event_order() {
    fn site(path: &str, _: &str) -> (u16, &'static str, Vec<(&'static str, &'static str)>, String) {
        match path {
            "/a.js" => {
                std::thread::sleep(std::time::Duration::from_millis(50));
                (200, "text/javascript", vec![], "L('/a.js ran');".into())
            }
            _ => (
                200,
                "text/html",
                vec![],
                r#"<pre id="log"></pre><script>
function L(m) { document.getElementById('log').textContent += m + '\n'; }
document.addEventListener('readystatechange', function () { L('readystatechange readyState=' + document.readyState); });
document.addEventListener('DOMContentLoaded', function () { L('DOMContentLoaded readyState=' + document.readyState); });
window.addEventListener('load', function () { L('window load readyState=' + document.readyState); });
L('readyState=' + document.readyState);
var s = document.createElement('script');
s.src = '/a.js';
s.onload = function () { L('/a.js load'); };
s.onerror = function () { L('/a.js error'); };
document.head.appendChild(s);
L('/a.js inserted');
</script>"#
                    .into(),
            ),
        }
    }
    let server = serve(site);
    let (mut engine, view, rt) = engine();
    rt.block_on(engine.load_url(view, server.url("/"))).expect("load");
    let log = page_log(&mut engine, view);
    assert_eq!(
        event_trace(&log),
        [
            "readyState=loading",
            "/a.js inserted",
            "readystatechange readyState=interactive",
            "DOMContentLoaded readyState=interactive",
            "/a.js ran",
            "/a.js load",
            "readystatechange readyState=complete",
            "window load readyState=complete",
        ],
        "{log}"
    );
}

/// The #715 reduced fixture (`docs/diagnostics/2026-10-16`). Listeners for
/// `readystatechange` and `window` `load` are added here so the trace can
/// name them; the page and both chunks are the fixture files.
#[test]
fn the_715_fixture_event_trace_matches_chrome() {
    fn html() -> String {
        let raw = include_str!("../../../docs/diagnostics/2026-10-16/reddit_707_script_insert_feed.html");
        let mark = "insert('reddit_707_chunk_a.js'";
        let extra = "\
L('readyState=' + document.readyState);\n\
document.addEventListener('readystatechange', function () { L('readystatechange readyState=' + document.readyState); });\n\
window.addEventListener('load', function () { L('window load readyState=' + document.readyState); });\n";
        raw.replacen(mark, &format!("{extra}{mark}"), 1)
    }
    fn site(path: &str, _: &str) -> (u16, &'static str, Vec<(&'static str, &'static str)>, String) {
        match path {
            "/reddit_707_chunk_a.js" => {
                std::thread::sleep(std::time::Duration::from_millis(300));
                (
                    200,
                    "text/javascript",
                    vec![],
                    include_str!("../../../docs/diagnostics/2026-10-16/reddit_707_chunk_a.js").into(),
                )
            }
            "/reddit_707_chunk_b.js" => {
                std::thread::sleep(std::time::Duration::from_millis(300));
                (
                    200,
                    "text/javascript",
                    vec![],
                    include_str!("../../../docs/diagnostics/2026-10-16/reddit_707_chunk_b.js").into(),
                )
            }
            _ => (200, "text/html", vec![], html()),
        }
    }
    let server = serve(site);
    let (mut engine, view, rt) = chain_engine(200);
    rt.block_on(engine.load_url(view, server.url("/"))).expect("load");
    let log = page_log(&mut engine, view);
    assert_eq!(
        event_trace(&log),
        [
            "readyState=loading",
            "reddit_707_chunk_a.js inserted",
            "readystatechange readyState=interactive",
            "DOMContentLoaded, readyState=interactive",
            "chunk a ran",
            "reddit_707_chunk_a.js load",
            "reddit_707_chunk_b.js inserted",
            "chunk b ran",
            "reddit_707_chunk_b.js load",
            "IO rendered 3 posts",
            "readystatechange readyState=complete",
            "window load readyState=complete",
        ],
        "{log}"
    );
    let feed = text(&mut engine, view, "document.getElementById('feed').textContent");
    assert!(feed.contains("Feed post 1") && feed.contains("Feed post 3"), "{feed}");
}

/// A script the `load` handler inserts is after the event. The load does
/// not wait for it.
#[test]
fn a_script_inserted_by_the_load_handler_does_not_delay_load() {
    // Set once /after.js is answered; the load ends before that.
    static AFTER_ANSWERED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    fn site(path: &str, _: &str) -> (u16, &'static str, Vec<(&'static str, &'static str)>, String) {
        match path {
            "/after.js" => {
                std::thread::sleep(std::time::Duration::from_millis(400));
                AFTER_ANSWERED.store(true, std::sync::atomic::Ordering::SeqCst);
                (200, "text/javascript", vec![], "L('/after.js ran');".into())
            }
            _ => (
                200,
                "text/html",
                vec![],
                r#"<pre id="log"></pre><script>
function L(m) { document.getElementById('log').textContent += m + '\n'; }
document.addEventListener('readystatechange', function () { L('readystatechange readyState=' + document.readyState); });
document.addEventListener('DOMContentLoaded', function () { L('DOMContentLoaded readyState=' + document.readyState); });
window.addEventListener('load', function () {
  L('window load readyState=' + document.readyState);
  var s = document.createElement('script');
  s.src = '/after.js';
  s.onload = function () { L('/after.js load'); };
  document.head.appendChild(s);
  L('/after.js inserted');
});
L('readyState=' + document.readyState);
</script>"#
                    .into(),
            ),
        }
    }
    let server = serve(site);
    let (mut engine, view, rt) = engine();
    let started = std::time::Instant::now();
    rt.block_on(engine.load_url(view, server.url("/"))).expect("load");
    assert!(
        !AFTER_ANSWERED.load(std::sync::atomic::Ordering::SeqCst),
        "load waited for a script inserted by the load handler: {:?}",
        started.elapsed()
    );
    let log = page_log(&mut engine, view);
    assert_eq!(
        event_trace(&log),
        [
            "readyState=loading",
            "readystatechange readyState=interactive",
            "DOMContentLoaded readyState=interactive",
            "readystatechange readyState=complete",
            "window load readyState=complete",
            "/after.js inserted",
        ],
        "{log}"
    );
    let started = std::time::Instant::now();
    loop {
        rt.block_on(engine.pump_live(view, 20));
        let log = page_log(&mut engine, view);
        if log.contains("/after.js load") {
            let lines = event_trace(&log);
            let load_at = lines.iter().position(|l| *l == "window load readyState=complete").unwrap();
            let ran_at = lines.iter().position(|l| *l == "/after.js ran").expect(&log);
            let heard_at = lines.iter().position(|l| *l == "/after.js load").unwrap();
            assert!(load_at < ran_at && ran_at < heard_at, "{log}");
            break;
        }
        assert!(started.elapsed() < std::time::Duration::from_secs(3), "the load handler's script never ran: {log}");
    }
}

/// At most eight inserted-script fetches are on the wire at once, and all
/// twenty still run and fire `load` before `window` `load`.
#[test]
fn inserted_script_fetches_are_capped_at_eight_in_flight() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static IN_FLIGHT: AtomicUsize = AtomicUsize::new(0);
    static MAX_SEEN: AtomicUsize = AtomicUsize::new(0);
    IN_FLIGHT.store(0, Ordering::SeqCst);
    MAX_SEEN.store(0, Ordering::SeqCst);
    fn site(path: &str, _: &str) -> (u16, &'static str, Vec<(&'static str, &'static str)>, String) {
        if path.starts_with("/c") {
            let now = IN_FLIGHT.fetch_add(1, Ordering::SeqCst) + 1;
            MAX_SEEN.fetch_max(now, Ordering::SeqCst);
            std::thread::sleep(std::time::Duration::from_millis(80));
            IN_FLIGHT.fetch_sub(1, Ordering::SeqCst);
            return (200, "text/javascript", vec![], "ran++;".into());
        }
        (
            200,
            "text/html",
            vec![],
            r#"<pre id="log"></pre><script>
function L(m) { document.getElementById('log').textContent += m + '\n'; }
var ran = 0, heard = 0;
document.addEventListener('readystatechange', function () { L('readystatechange readyState=' + document.readyState); });
document.addEventListener('DOMContentLoaded', function () { L('DOMContentLoaded readyState=' + document.readyState); });
window.addEventListener('load', function () {
  L('window load readyState=' + document.readyState + ' ran=' + ran + ' heard=' + heard);
});
L('readyState=' + document.readyState);
for (var i = 0; i < 20; i++) {
  var s = document.createElement('script');
  s.src = '/c' + i + '.js';
  s.onload = function () { heard++; };
  document.head.appendChild(s);
}
</script>"#
                .into(),
        )
    }
    let server = serve(site);
    let (mut engine, view, rt) = chain_engine(5_000);
    rt.block_on(engine.load_url(view, server.url("/"))).expect("load");
    let log = page_log(&mut engine, view);
    assert_eq!(
        event_trace(&log),
        [
            "readyState=loading",
            "readystatechange readyState=interactive",
            "DOMContentLoaded readyState=interactive",
            "readystatechange readyState=complete",
            "window load readyState=complete ran=20 heard=20",
        ],
        "{log}"
    );
    let max = MAX_SEEN.load(Ordering::SeqCst);
    assert!(max <= 8, "more than 8 inserted fetches were in flight: {max}");
    assert!(max >= 2, "the fetches never overlapped: {max}");
}

/// Cancelling a load drops the inserted script's socket. The load returns
/// without waiting out the server's sleep.
#[test]
fn cancelling_a_load_closes_the_inserted_fetch() {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let asked = Arc::new(AtomicBool::new(false));
    let closed = Arc::new(AtomicBool::new(false));
    let asked_bg = asked.clone();
    let closed_bg = closed.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let asked_bg = asked_bg.clone();
            let closed_bg = closed_bg.clone();
            std::thread::spawn(move || {
                let mut request = Vec::new();
                let mut buf = [0u8; 2048];
                while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                    match stream.read(&mut buf) {
                        Ok(0) | Err(_) => return,
                        Ok(n) => request.extend_from_slice(&buf[..n]),
                    }
                }
                let head = String::from_utf8_lossy(&request);
                let target = head.split_whitespace().nth(1).unwrap_or("/");
                let path = target.split('?').next().unwrap_or("/");
                if path == "/slow.js" {
                    asked_bg.store(true, Ordering::SeqCst);
                    let _ = stream.set_read_timeout(Some(std::time::Duration::from_millis(50)));
                    let started = std::time::Instant::now();
                    let mut probe = [0u8; 8];
                    while started.elapsed() < std::time::Duration::from_secs(3) {
                        match stream.read(&mut probe) {
                            Ok(0) => {
                                closed_bg.store(true, Ordering::SeqCst);
                                return;
                            }
                            Err(e)
                                if e.kind() == std::io::ErrorKind::WouldBlock
                                    || e.kind() == std::io::ErrorKind::TimedOut =>
                            {
                                continue;
                            }
                            Err(_) => {
                                closed_bg.store(true, Ordering::SeqCst);
                                return;
                            }
                            Ok(_) => continue,
                        }
                    }
                    return;
                }
                let body = r#"<p>x</p><script>
var s = document.createElement('script');
s.src = '/slow.js';
document.head.appendChild(s);
</script>"#;
                let out = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(out.as_bytes());
            });
        }
    });
    let (mut engine, view, rt) = chain_engine(5_000);
    let canceller = engine.nav_canceller(view).expect("canceller");
    let asked_wait = asked.clone();
    let cancel = std::thread::spawn(move || {
        let started = std::time::Instant::now();
        while !asked_wait.load(Ordering::SeqCst) && started.elapsed().as_secs() < 30 {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        canceller.cancel_current();
        std::time::Instant::now()
    });
    let _ = rt.block_on(engine.load_url(view, Url::parse(&format!("http://127.0.0.1:{port}/")).unwrap()));
    // From the cancel, not from the load's start: the debug build's first
    // layout comes before the request and took over a second on macOS. The
    // server holds the fetch for 3 s.
    let cancelled_at = cancel.join().unwrap();
    assert!(
        cancelled_at.elapsed() < std::time::Duration::from_millis(1500),
        "the cancelled load waited out the fetch: {:?} after the cancel",
        cancelled_at.elapsed()
    );
    assert!(asked.load(Ordering::SeqCst), "the inserted script was never requested");
    let ran = engine.execute_script(view, "String(window.slowRan)").unwrap_or_default();
    assert_ne!(ran, r#"String("true")"#);
    let started = std::time::Instant::now();
    while !closed.load(Ordering::SeqCst) && started.elapsed() < std::time::Duration::from_secs(2) {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(closed.load(Ordering::SeqCst), "the in-flight fetch was not cancelled");
}


/// Pump a yielded load until `window` `load` has fired and `PageLoaded`
/// was sent. Each turn runs at most one arrived inserted script.
fn finish_yielded_load(engine: &mut Engine, view: EngineViewId, rt: &tokio::runtime::Runtime) {
    let started = std::time::Instant::now();
    loop {
        let done = engine
            .views
            .get(&view)
            .is_some_and(|state| state.load_tail.is_none())
            && !engine.views[&view].navigation.is_loading()
            && text(engine, view, "document.readyState").contains("complete");
        if done {
            return;
        }
        if started.elapsed() > std::time::Duration::from_secs(20) {
            let ready = text(engine, view, "document.readyState");
            let tail = engine.views[&view].load_tail;
            let loading = engine.views[&view].navigation.is_loading();
            let log = page_log(engine, view);
            panic!("tailed load did not finish: readyState={ready} tail={tail:?} loading={loading} log={log}");
        }
        rt.block_on(engine.pump_live(view, 16));
        std::thread::sleep(std::time::Duration::from_millis(30));
    }
}

fn chrome_order_page() -> String {
    r#"<pre id="log"></pre><script>
function L(m) { document.getElementById('log').textContent += m + '\n'; }
document.addEventListener('readystatechange', function () { L('readystatechange readyState=' + document.readyState); });
document.addEventListener('DOMContentLoaded', function () { L('DOMContentLoaded readyState=' + document.readyState); });
window.addEventListener('load', function () { L('window load readyState=' + document.readyState); });
L('readyState=' + document.readyState);
var s = document.createElement('script');
s.src = '/a.js';
s.onload = function () { L('/a.js load'); };
s.onerror = function () { L('/a.js error'); };
document.head.appendChild(s);
L('/a.js inserted');
</script>"#
        .into()
}

const CHROME_ORDER_TRACE: &[&str] = &[
    "readyState=loading",
    "/a.js inserted",
    "readystatechange readyState=interactive",
    "DOMContentLoaded readyState=interactive",
    "/a.js ran",
    "/a.js load",
    "readystatechange readyState=complete",
    "window load readyState=complete",
];

fn chrome_order_site(
    path: &str,
    _: &str,
) -> (u16, &'static str, Vec<(&'static str, &'static str)>, String) {
    match path {
        "/a.js" => {
            std::thread::sleep(std::time::Duration::from_millis(40));
            (200, "text/javascript", vec![], "L('/a.js ran');".into())
        }
        _ => (200, "text/html", vec![], chrome_order_page()),
    }
}

/// The sliced load and `load_url` produce the same event trace.
#[test]
fn a_yielding_load_matches_the_chrome_order_trace() {
    let server = serve(chrome_order_site);
    let (mut whole, whole_view, whole_rt) = engine();
    whole_rt
        .block_on(whole.load_url(whole_view, server.url("/")))
        .expect("whole load");
    let whole_log = page_log(&mut whole, whole_view);
    let whole_trace = event_trace(&whole_log);

    let (mut sliced, sliced_view, sliced_rt) = engine();
    let yielded = sliced_rt
        .block_on(sliced.load_url_yielding(sliced_view, server.url("/")))
        .expect("yielding load");
    assert_eq!(yielded, LoadYield::Yielded);
    assert!(
        sliced.views[&sliced_view].navigation.is_loading(),
        "PageLoaded waits for the chain"
    );
    finish_yielded_load(&mut sliced, sliced_view, &sliced_rt);
    let sliced_log = page_log(&mut sliced, sliced_view);
    let sliced_trace = event_trace(&sliced_log);
    assert_eq!(whole_trace, CHROME_ORDER_TRACE, "whole-chain trace");
    assert_eq!(sliced_trace, whole_trace, "sliced trace");
    assert!(!sliced.views[&sliced_view].navigation.is_loading());
}

fn fixture_715_html() -> String {
    let raw =
        include_str!("../../../docs/diagnostics/2026-10-16/reddit_707_script_insert_feed.html");
    let mark = "insert('reddit_707_chunk_a.js'";
    let extra = "\
L('readyState=' + document.readyState);\n\
document.addEventListener('readystatechange', function () { L('readystatechange readyState=' + document.readyState); });\n\
window.addEventListener('load', function () { L('window load readyState=' + document.readyState); });\n";
    raw.replacen(mark, &format!("{extra}{mark}"), 1)
}

fn fixture_715_site(
    path: &str,
    _: &str,
) -> (u16, &'static str, Vec<(&'static str, &'static str)>, String) {
    match path {
        "/reddit_707_chunk_a.js" => {
            std::thread::sleep(std::time::Duration::from_millis(80));
            (
                200,
                "text/javascript",
                vec![],
                include_str!("../../../docs/diagnostics/2026-10-16/reddit_707_chunk_a.js").into(),
            )
        }
        "/reddit_707_chunk_b.js" => {
            std::thread::sleep(std::time::Duration::from_millis(80));
            (
                200,
                "text/javascript",
                vec![],
                include_str!("../../../docs/diagnostics/2026-10-16/reddit_707_chunk_b.js").into(),
            )
        }
        _ => (200, "text/html", vec![], fixture_715_html()),
    }
}

const FIXTURE_715_TRACE: &[&str] = &[
    "readyState=loading",
    "reddit_707_chunk_a.js inserted",
    "readystatechange readyState=interactive",
    "DOMContentLoaded, readyState=interactive",
    "chunk a ran",
    "reddit_707_chunk_a.js load",
    "reddit_707_chunk_b.js inserted",
    "chunk b ran",
    "reddit_707_chunk_b.js load",
    "IO rendered 3 posts",
    "readystatechange readyState=complete",
    "window load readyState=complete",
];

/// The #715 fixture's sliced load matches the whole-chain trace.
#[test]
fn a_yielding_load_matches_the_715_fixture_trace() {
    let server = serve(fixture_715_site);
    let (mut whole, whole_view, whole_rt) = chain_engine(5_000);
    whole_rt
        .block_on(whole.load_url(whole_view, server.url("/")))
        .expect("whole load");
    let whole_log = page_log(&mut whole, whole_view);
    let whole_trace = event_trace(&whole_log);

    let (mut sliced, sliced_view, sliced_rt) = chain_engine(5_000);
    let yielded = sliced_rt
        .block_on(sliced.load_url_yielding(sliced_view, server.url("/")))
        .expect("yielding load");
    assert_eq!(yielded, LoadYield::Yielded);
    assert_eq!(
        text(&mut sliced, sliced_view, "document.readyState"),
        r#"String("interactive")"#,
    );
    finish_yielded_load(&mut sliced, sliced_view, &sliced_rt);
    let sliced_log = page_log(&mut sliced, sliced_view);
    let sliced_trace = event_trace(&sliced_log);
    assert_eq!(whole_trace, FIXTURE_715_TRACE, "whole-chain trace");
    assert_eq!(sliced_trace, whole_trace, "sliced trace");
    let feed = text(
        &mut sliced,
        sliced_view,
        "document.getElementById('feed').textContent",
    );
    assert!(
        feed.contains("Feed post 1") && feed.contains("Feed post 3"),
        "{feed}"
    );
}

/// A page that inserts nothing returns from the yielding load the same way
/// `load_url` does: `load` has fired, and a later live turn is not required.
#[test]
fn a_page_with_no_inserted_script_does_not_yield() {
    fn site(path: &str, _: &str) -> (u16, &'static str, Vec<(&'static str, &'static str)>, String) {
        match path {
            "/" => (
                200,
                "text/html",
                vec![],
                r#"<pre id="log"></pre><script>
function L(m) { document.getElementById('log').textContent += m + '\n'; }
document.addEventListener('readystatechange', function () { L('readystatechange readyState=' + document.readyState); });
document.addEventListener('DOMContentLoaded', function () { L('DOMContentLoaded readyState=' + document.readyState); });
window.addEventListener('load', function () { L('window load readyState=' + document.readyState); });
setTimeout(function () { L('timer'); }, 0);
L('readyState=' + document.readyState);
</script>"#
                    .into(),
            ),
            _ => (404, "text/plain", vec![], "no".into()),
        }
    }
    let expected = [
        "readyState=loading",
        "readystatechange readyState=interactive",
        "DOMContentLoaded readyState=interactive",
        "readystatechange readyState=complete",
        "window load readyState=complete",
        "timer",
    ];
    let server = serve(site);
    let (mut whole, whole_view, whole_rt) = engine();
    whole_rt
        .block_on(whole.load_url(whole_view, server.url("/")))
        .expect("whole");
    let (mut sliced, sliced_view, sliced_rt) = engine();
    let yielded = sliced_rt
        .block_on(sliced.load_url_yielding(sliced_view, server.url("/")))
        .expect("yielding");
    assert_eq!(yielded, LoadYield::Complete);
    assert!(sliced.views[&sliced_view].load_tail.is_none());
    assert!(!sliced.views[&sliced_view].navigation.is_loading());
    assert_eq!(event_trace(&page_log(&mut whole, whole_view)), expected);
    assert_eq!(event_trace(&page_log(&mut sliced, sliced_view)), expected);
}

/// The yielding load returns while an inserted script is still on the
/// network. A live turn during that wait runs the timer that is due, as
/// Chrome does, and does not fire `load`. The event loop has the engine
/// back, and a frame was presented for the interactive document.
#[test]
fn a_yielding_load_returns_during_the_inserted_script_fetch() {
    // Set once /a.js is answered; the load returns before that.
    static A_ANSWERED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    fn site(path: &str, _: &str) -> (u16, &'static str, Vec<(&'static str, &'static str)>, String) {
        match path {
            "/a.js" => {
                std::thread::sleep(std::time::Duration::from_millis(1500));
                A_ANSWERED.store(true, std::sync::atomic::Ordering::SeqCst);
                (200, "text/javascript", vec![], "L('/a.js ran');".into())
            }
            _ => (
                200,
                "text/html",
                vec![],
                r#"<pre id="log"></pre><script>
function L(m) { document.getElementById('log').textContent += m + '\n'; }
document.addEventListener('readystatechange', function () { L('readystatechange readyState=' + document.readyState); });
document.addEventListener('DOMContentLoaded', function () { L('DOMContentLoaded readyState=' + document.readyState); });
window.addEventListener('load', function () { L('window load readyState=' + document.readyState); });
setTimeout(function () { L('timer'); }, 0);
L('readyState=' + document.readyState);
var s = document.createElement('script');
s.src = '/a.js';
s.onload = function () { L('/a.js load'); };
document.head.appendChild(s);
L('/a.js inserted');
</script>"#
                    .into(),
            ),
        }
    }
    let server = serve(site);
    let (mut engine, view, rt) = engine();
    let frames_before = engine.views[&view].frames_presented;
    let started = std::time::Instant::now();
    let yielded = rt
        .block_on(engine.load_url_yielding(view, server.url("/")))
        .expect("yield");
    let returned_in = started.elapsed();
    assert_eq!(yielded, LoadYield::Yielded);
    assert!(
        !A_ANSWERED.load(std::sync::atomic::Ordering::SeqCst),
        "the load held the event loop for the fetch: {returned_in:?}"
    );
    assert!(
        engine.views[&view].frames_presented > frames_before,
        "no frame was presented for the interactive document"
    );
    assert_eq!(
        text(&mut engine, view, "document.readyState"),
        r#"String("interactive")"#
    );
    assert!(engine.views[&view].navigation.is_loading());
    let during = page_log(&mut engine, view);
    assert_eq!(
        event_trace(&during),
        [
            "readyState=loading",
            "/a.js inserted",
            "readystatechange readyState=interactive",
            "DOMContentLoaded readyState=interactive",
        ],
        "{during}"
    );
    let turn_started = std::time::Instant::now();
    rt.block_on(engine.pump_live(view, 5_000));
    assert!(
        turn_started.elapsed() < std::time::Duration::from_millis(500),
        "the live turn waited for the fetch: {:?}",
        turn_started.elapsed()
    );
    let mid = page_log(&mut engine, view);
    // The 0 ms timer does not wait for `load` (#730: a scroll listener's
    // debounce timer left the rows it fills white while the tail was open).
    // `load` still waits for the script.
    assert_eq!(
        event_trace(&mid),
        [
            "readyState=loading",
            "/a.js inserted",
            "readystatechange readyState=interactive",
            "DOMContentLoaded readyState=interactive",
            "timer",
        ],
        "load ran before the script, or the due timer did not run: {mid}"
    );
    assert_eq!(
        text(&mut engine, view, "document.readyState"),
        r#"String("interactive")"#
    );
    finish_yielded_load(&mut engine, view, &rt);
    assert_eq!(
        event_trace(&page_log(&mut engine, view)),
        [
            "readyState=loading",
            "/a.js inserted",
            "readystatechange readyState=interactive",
            "DOMContentLoaded readyState=interactive",
            "timer",
            "/a.js ran",
            "/a.js load",
            "readystatechange readyState=complete",
            "window load readyState=complete",
        ],
    );
}

/// The turn that finishes a yielded load fires `load` and returns. A fetch
/// the `load` handler starts, and one a timer of the load's horizon
/// starts, are answered on later turns: no turn waits for them (#727 R1
/// H1: the tail pumped them with the 30 s network cap, a 4 s turn here).
#[test]
fn a_yielded_load_does_not_wait_for_a_load_handler_fetch() {
    // Set once a held request is answered.
    static BEACON_ANSWERED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    fn site(path: &str, _: &str) -> (u16, &'static str, Vec<(&'static str, &'static str)>, String) {
        match path {
            "/a.js" => {
                std::thread::sleep(std::time::Duration::from_millis(300));
                (200, "text/javascript", vec![], "L('/a.js ran');".into())
            }
            "/beacon" | "/late" => {
                std::thread::sleep(std::time::Duration::from_millis(4000));
                BEACON_ANSWERED.store(true, std::sync::atomic::Ordering::SeqCst);
                (200, "text/plain", vec![], "ok".into())
            }
            _ => (
                200,
                "text/html",
                vec![],
                r#"<pre id="log"></pre><script>
function L(m) { document.getElementById('log').textContent += m + '\n'; }
window.addEventListener('load', function () {
  L('window load');
  fetch('/beacon').then(function (r) { return r.text(); }).then(function (t) { L('beacon ' + t); });
  setTimeout(function () {
    fetch('/late').then(function (r) { return r.text(); }).then(function (t) { L('late ' + t); });
  }, 0);
});
var s = document.createElement('script');
s.src = '/a.js';
document.head.appendChild(s);
</script>"#
                    .into(),
            ),
        }
    }
    let server = serve(site);
    let (mut engine, view, rt) = engine();
    let yielded = rt
        .block_on(engine.load_url_yielding(view, server.url("/")))
        .expect("yield");
    assert_eq!(yielded, LoadYield::Yielded);
    let started = std::time::Instant::now();
    let mut longest = std::time::Duration::ZERO;
    let mut load_fired = false;
    loop {
        let turn = std::time::Instant::now();
        rt.block_on(engine.pump_live(view, 16));
        longest = longest.max(turn.elapsed());
        let log = page_log(&mut engine, view);
        if !load_fired && log.contains("window load") {
            load_fired = true;
            assert!(
                !BEACON_ANSWERED.load(std::sync::atomic::Ordering::SeqCst),
                "the turn that fired load waited for its fetch: {:?}",
                turn.elapsed()
            );
            assert!(engine.views[&view].load_tail.is_none());
        }
        if log.contains("beacon ok") && log.contains("late ok") {
            break;
        }
        assert!(
            started.elapsed() < std::time::Duration::from_secs(12),
            "the held requests were never answered: {log}"
        );
        std::thread::sleep(std::time::Duration::from_millis(30));
    }
    assert!(load_fired);
    assert!(
        longest < std::time::Duration::from_millis(500),
        "a live turn held the window for {longest:?}"
    );
}

/// After the load, a timer starts a `fetch()` and a dynamic `import()` the
/// server holds for 3 s each. No live turn waits for either: each starts
/// on one turn and is answered on a later one (#727 Atlas hold, item 1:
/// a turn fetched the modules an `import()` asked for inside itself, for
/// up to `LIVE_NETWORK_BUDGET`, and failed the import as timed out).
#[test]
fn a_slow_post_load_fetch_or_import_holds_no_live_turn() {
    fn site(path: &str, _: &str) -> (u16, &'static str, Vec<(&'static str, &'static str)>, String) {
        match path {
            "/slow" => {
                std::thread::sleep(std::time::Duration::from_millis(3000));
                (200, "text/plain", vec![], "ok".into())
            }
            "/m.js" => {
                std::thread::sleep(std::time::Duration::from_millis(3000));
                (200, "text/javascript", vec![], "export const v = 'ok';".into())
            }
            _ => (
                200,
                "text/html",
                vec![],
                r#"<pre id="log"></pre><script>
function L(m) { document.getElementById('log').textContent += m + '\n'; }
window.addEventListener('load', function () {
  setTimeout(function () {
    fetch('/slow').then(function (r) { return r.text(); }).then(function (t) { L('fetch ' + t); });
    import('/m.js').then(function (m) { L('import ' + m.v); }, function (e) { L('import failed ' + e); });
  }, 6000);
});
</script>"#
                    .into(),
            ),
        }
    }
    let server = serve(site);
    let (mut engine, view, rt) = engine();
    rt.block_on(engine.load_url(view, server.url("/"))).expect("load");
    // Past the load's timer horizon (5 s), so the timer runs on a live
    // turn, not inside the load.
    assert!(!page_log(&mut engine, view).contains("fetch"));
    let started = std::time::Instant::now();
    let mut longest = std::time::Duration::ZERO;
    let mut turns = 0;
    loop {
        let turn = std::time::Instant::now();
        rt.block_on(engine.pump_live(view, if turns == 0 { 6_000 } else { 16 }));
        longest = longest.max(turn.elapsed());
        turns += 1;
        let log = page_log(&mut engine, view);
        if log.contains("fetch ok") && log.contains("import ") {
            assert!(log.contains("import ok"), "{log}");
            break;
        }
        assert!(
            started.elapsed() < std::time::Duration::from_secs(15),
            "the held requests were never answered: {log}"
        );
        std::thread::sleep(std::time::Duration::from_millis(16));
    }
    assert!(turns > 2, "both answers came on the turn that asked for them");
    assert!(
        longest < std::time::Duration::from_millis(100),
        "a live turn held the window for {longest:?}"
    );
}

/// Navigating away while the chain is still fetching drops it. `load` does
/// not fire for the old document, and it does not fire an extra time on
/// the next one. The late script does not run there.
#[test]
fn navigating_away_mid_tail_does_not_fire_load() {
    fn site(path: &str, _: &str) -> (u16, &'static str, Vec<(&'static str, &'static str)>, String) {
        match path {
            "/slow.js" => {
                std::thread::sleep(std::time::Duration::from_millis(1500));
                (
                    200,
                    "text/javascript",
                    vec![],
                    "window.slowRan = true; L('slow ran');".into(),
                )
            }
            "/next" => (
                200,
                "text/html",
                vec![],
                r#"<pre id="log"></pre><script>
function L(m) { document.getElementById('log').textContent += m + '\n'; }
window.addEventListener('load', function () { L('window load'); });
</script>"#
                    .into(),
            ),
            _ => (
                200,
                "text/html",
                vec![],
                r#"<pre id="log"></pre><script>
function L(m) { document.getElementById('log').textContent += m + '\n'; }
window.addEventListener('load', function () { L('old load'); });
var s = document.createElement('script');
s.src = '/slow.js';
document.head.appendChild(s);
</script>"#
                    .into(),
            ),
        }
    }
    let server = serve(site);
    let (mut engine, view, rt) = engine();
    let yielded = rt
        .block_on(engine.load_url_yielding(view, server.url("/")))
        .expect("yield");
    assert_eq!(yielded, LoadYield::Yielded);
    assert_eq!(
        text(&mut engine, view, "document.readyState"),
        r#"String("interactive")"#
    );
    assert!(
        !page_log(&mut engine, view).contains("old load"),
        "load fired before the chain was idle"
    );
    rt.block_on(engine.load_url(view, server.url("/next")))
        .expect("next page");
    rt.block_on(engine.pump_live(view, 16));
    std::thread::sleep(std::time::Duration::from_millis(200));
    rt.block_on(engine.pump_live(view, 16));
    assert!(engine.views[&view].load_tail.is_none());
    let log = page_log(&mut engine, view);
    assert_eq!(event_trace(&log), ["window load"], "{log}");
    assert_ne!(
        text(&mut engine, view, "String(window.slowRan)"),
        r#"String("true")"#
    );
}

/// h2's page: an inline script and a `fetch`, no inserted script. The
/// yielding load finishes inside the call. `window` `load` is not parked.
#[test]
fn a_fetch_without_an_inserted_script_does_not_yield() {
    fn site(path: &str, _: &str) -> (u16, &'static str, Vec<(&'static str, &'static str)>, String) {
        match path {
            "/beacon" => (204, "text/plain", vec![], String::new()),
            _ => (
                200,
                "text/html",
                vec![],
                r#"<pre id="log"></pre><script>
function L(m) { document.getElementById('log').textContent += m + '\n'; }
window.addEventListener('load', function () { L('window load'); });
fetch('/beacon').catch(function () {});
L('script');
</script>"#
                    .into(),
            ),
        }
    }
    let server = serve(site);
    let (mut engine, view, rt) = engine();
    let yielded = rt
        .block_on(engine.load_url_yielding(view, server.url("/")))
        .expect("load");
    assert_eq!(yielded, LoadYield::Complete);
    assert!(engine.views[&view].load_tail.is_none());
    assert!(!engine.views[&view].navigation.is_loading());
    assert_eq!(
        text(&mut engine, view, "document.readyState"),
        r#"String("complete")"#
    );
    assert_eq!(event_trace(&page_log(&mut engine, view)), ["script", "window load"]);
}

/// On the yielded path, `window` `load` fires once, the lifecycle log shows
/// that step and the timer horizon, and `readyState` is `complete`.
#[test]
fn a_yielding_load_logs_load_and_timers_once() {
    let server = serve(chrome_order_site);
    let (mut engine, view, rt) = engine();
    let captured = LifecycleLog::default();
    let writer = captured.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_max_level(tracing::Level::INFO)
        .with_writer(move || writer.clone())
        .finish();
    tracing::subscriber::with_default(subscriber, || {
        let yielded = rt
            .block_on(engine.load_url_yielding(view, server.url("/")))
            .expect("yield");
        assert_eq!(yielded, LoadYield::Yielded);
        assert_eq!(
            text(&mut engine, view, "document.readyState"),
            r#"String("interactive")"#
        );
        finish_yielded_load(&mut engine, view, &rt);
    });
    assert_eq!(
        text(&mut engine, view, "document.readyState"),
        r#"String("complete")"#
    );
    let log = page_log(&mut engine, view);
    let loads = event_trace(&log)
        .into_iter()
        .filter(|line| *line == "window load readyState=complete")
        .count();
    assert_eq!(loads, 1, "window load fired once: {log}");
    let lines = captured.lines();
    let steps = lines
        .iter()
        .filter(|line| line.contains("Running page lifecycle step"))
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(
        steps.iter().filter(|line| line.contains("event:load")).count(),
        1,
        "lifecycle log: {steps:?}"
    );
    assert_eq!(
        steps.iter().filter(|line| line.contains("timers")).count(),
        1,
        "lifecycle log: {steps:?}"
    );
    assert!(
        lines.iter().any(|line| line.contains("Page lifecycle step done") && line.contains("event:load")),
        "{lines:?}"
    );
    assert!(
        lines.iter().any(|line| line.contains("Page lifecycle step done") && line.contains("timers")),
        "{lines:?}"
    );
}

#[derive(Clone, Default)]
struct LifecycleLog(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl LifecycleLog {
    fn lines(&self) -> Vec<String> {
        let bytes = self.0.lock().unwrap().clone();
        String::from_utf8(bytes)
            .unwrap()
            .lines()
            .map(str::to_string)
            .collect()
    }
}

impl std::io::Write for LifecycleLog {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
