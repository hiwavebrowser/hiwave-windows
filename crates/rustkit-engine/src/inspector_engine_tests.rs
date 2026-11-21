//! The Inspector's Console half through real headless loads: console calls,
//! script errors, unhandled rejections, unsupported APIs, the navigation and
//! lifecycle markers, the ring buffer and sinks, the privacy rules, and
//! that nothing is recorded when nobody listens.

use super::*;
use crate::inspector::{scrub_url, CollectingSink, MAX_MESSAGE_CHARS};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::AtomicUsize;
use std::time::{Duration, Instant};

/// One route of the test server.
#[derive(Clone)]
pub(crate) struct Route {
    pub path: &'static str,
    pub status: u16,
    pub content_type: &'static str,
    pub body: Vec<u8>,
    /// Answer after this long.
    pub delay_ms: u64,
    /// Extra response headers.
    pub headers: Vec<(&'static str, String)>,
}

// Delays, redirects and the request record serve the Network half's tests.
#[allow(dead_code)]
impl Route {
    pub fn ok(path: &'static str, content_type: &'static str, body: impl Into<String>) -> Self {
        Route { path, status: 200, content_type, body: body.into().into_bytes(), delay_ms: 0, headers: Vec::new() }
    }
    pub fn bytes(path: &'static str, content_type: &'static str, body: Vec<u8>) -> Self {
        Route { path, status: 200, content_type, body, delay_ms: 0, headers: Vec::new() }
    }
    pub fn html(path: &'static str, body: impl Into<String>) -> Self {
        Self::ok(path, "text/html", body)
    }
    pub fn status(mut self, status: u16) -> Self {
        self.status = status;
        self
    }
    pub fn delay(mut self, ms: u64) -> Self {
        self.delay_ms = ms;
        self
    }
    pub fn header(mut self, name: &'static str, value: impl Into<String>) -> Self {
        self.headers.push((name, value.into()));
        self
    }
    /// A 302 to `to`.
    pub fn redirect(path: &'static str, to: impl Into<String>) -> Self {
        Self::ok(path, "text/plain", "").status(302).header("Location", to)
    }
}

#[allow(dead_code)]
pub(crate) struct TestServer {
    pub port: u16,
    /// Request paths, in arrival order (the request line's target, query
    /// included).
    pub requests: Arc<std::sync::Mutex<Vec<String>>>,
    pub hits: Arc<AtomicUsize>,
}

impl TestServer {
    pub fn url(&self, path: &str) -> Url {
        Url::parse(&format!("http://127.0.0.1:{}{path}", self.port)).unwrap()
    }
}

/// Serve `routes` (matched on the path without its query) on 127.0.0.1;
/// anything else is a 404 with a short body.
pub(crate) fn serve(routes: Vec<Route>) -> TestServer {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let routes = Arc::new(routes);
    let requests = Arc::new(std::sync::Mutex::new(Vec::new()));
    let hits = Arc::new(AtomicUsize::new(0));
    let (seen, counted) = (requests.clone(), hits.clone());
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let (routes, seen, counted) = (routes.clone(), seen.clone(), counted.clone());
            std::thread::spawn(move || {
                let mut request = Vec::new();
                let mut buf = [0u8; 2048];
                while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                    match stream.read(&mut buf) {
                        Ok(0) | Err(_) => return,
                        Ok(n) => request.extend_from_slice(&buf[..n]),
                    }
                }
                counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let head = String::from_utf8_lossy(&request).to_string();
                let target = head.split_whitespace().nth(1).unwrap_or("/").to_string();
                seen.lock().unwrap().push(target.clone());
                let path = target.split('?').next().unwrap_or("/");
                let route = routes.iter().find(|r| r.path == path).cloned().unwrap_or_else(|| {
                    Route::ok("", "text/plain", "not found").status(404)
                });
                if route.delay_ms > 0 {
                    std::thread::sleep(Duration::from_millis(route.delay_ms));
                }
                let reason = match route.status {
                    200 => "OK",
                    302 => "Found",
                    404 => "Not Found",
                    _ => "Status",
                };
                let mut head = format!(
                    "HTTP/1.1 {} {reason}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n",
                    route.status,
                    route.content_type,
                    route.body.len()
                );
                for (name, value) in &route.headers {
                    head.push_str(&format!("{name}: {value}\r\n"));
                }
                head.push_str("\r\n");
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(&route.body);
            });
        }
    });
    TestServer { port, requests, hits }
}

pub(crate) fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap()
}

/// An engine whose views keep an Inspector buffer.
pub(crate) fn inspected_engine(config: EngineConfig) -> (Engine, EngineViewId) {
    let mut config = config;
    config.inspector.buffer = true;
    let mut engine = Engine::new(config).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 300, height: 200 })
        .expect("view");
    (engine, view)
}

pub(crate) fn records(engine: &Engine, view: EngineViewId) -> Vec<InspectorRecord> {
    engine.inspector(view).expect("view").snapshot_since(0).records
}

/// One line per record, with what the tests check (no times, no ports).
pub(crate) fn describe(record: &InspectorRecord, port: u16) -> String {
    let local = |s: &str| s.replace(&format!("http://127.0.0.1:{port}"), "");
    let at = |l: &Option<SourceLocation>| match l {
        Some(l) => format!(" @{}:{}:{}", local(&l.url), l.line, l.column),
        None => String::new(),
    };
    match &record.event {
        InspectorEvent::NavigationStart { url } => format!("navigation_start {}", local(url.as_deref().unwrap_or("-"))),
        InspectorEvent::NavigationCommit { url, status } => {
            format!("navigation_commit {} {}", local(url.as_deref().unwrap_or("-")), status.unwrap_or(0))
        }
        InspectorEvent::LoadFinished { scripts_over_budget, .. } => {
            format!("load_finished over_budget={scripts_over_budget}")
        }
        InspectorEvent::LoadEndedEarly { reason, .. } => format!("load_ended_early {reason:?}"),
        InspectorEvent::Console { level, message, location, .. } => {
            format!("console {level:?} {message}{}", at(location))
        }
        InspectorEvent::ScriptError { name, message, location, unhandled_rejection, script, value, message_len, .. } => {
            format!(
                "script_error {name}: {}{} script={} rejection={unhandled_rejection}",
                shown_message(message, value, *message_len),
                at(location),
                local(script)
            )
        }
        InspectorEvent::UnsupportedApi { interface, member, location } => {
            format!("unsupported_api {interface}.{member}{}", at(location))
        }
        InspectorEvent::FirstLayout => "first_layout".into(),
        InspectorEvent::FirstPaint => "first_paint".into(),
        InspectorEvent::Relayout { cause, .. } => format!("relayout {cause}"),
        InspectorEvent::NavigationRedirect { from, to, status } => {
            format!("navigation_redirect {} -> {} {status}", local(from), local(to))
        }
        InspectorEvent::Request { id, url, method, resource_type, initiator } => format!(
            "request #{id} {method} {} {} by={}",
            resource_type.as_str(),
            local(url),
            initiator.as_deref().unwrap_or("-")
        ),
        InspectorEvent::Response { id, status, mime, from_cache, .. } => format!(
            "response #{id} {status} {}{}",
            mime.as_deref().unwrap_or("-"),
            if *from_cache { " cache" } else { "" }
        ),
        InspectorEvent::Failure { id, stage, reason, .. } => format!("failure #{id} {} {reason}", stage.as_str()),
        InspectorEvent::Script { script, kind, outcome, held, .. } => {
            format!("script {kind} {} {outcome} held={}", local(script), held.as_deref().unwrap_or("-"))
        }
        InspectorEvent::Lifecycle { event, held_by_count, .. } => format!("lifecycle {event} held_by={held_by_count}"),
    }
}

/// An error record's message as the tests show it: the text when it was
/// kept, else the thrown value's type and the message's length.
pub(crate) fn shown_message(message: &Option<String>, value: &str, length: Option<u64>) -> String {
    match (message, length) {
        (Some(message), _) => message.clone(),
        (None, Some(length)) => format!("<{value} len={length}>"),
        (None, None) => format!("<{value}>"),
    }
}

/// The default config, with console messages kept as text (for the tests
/// that follow what a page logged).
fn console_text() -> EngineConfig {
    let mut config = EngineConfig::default();
    config.inspector.keep_console_text = true;
    config
}

fn load_page(config: EngineConfig, routes: Vec<Route>) -> (Engine, EngineViewId, TestServer) {
    let server = serve(routes);
    let (mut engine, view) = inspected_engine(config);
    runtime().block_on(engine.load_url(view, server.url("/"))).expect("load_url");
    (engine, view, server)
}

fn described(engine: &Engine, view: EngineViewId, port: u16) -> Vec<String> {
    records(engine, view).iter().map(|r| describe(r, port)).collect()
}

/// Only the records of the kinds named.
fn only(lines: &[String], kinds: &[&str]) -> Vec<String> {
    lines
        .iter()
        .filter(|l| kinds.iter().any(|k| l.split(' ').next() == Some(*k)))
        .cloned()
        .collect()
}

const CONSOLE_PAGE: &str = "<html><body>hi<script>
console.log('one', 2);
console.info('two');
console.warn('three');
console.error('four');
console.debug('five');
</script></body></html>";

#[test]
fn a_page_load_is_recorded_in_order() {
    let (engine, view, server) = load_page(EngineConfig::default(), vec![Route::html("/", CONSOLE_PAGE)]);
    assert_eq!(
        described(&engine, view, server.port),
        vec![
            "navigation_start /",
            "request #1 GET document / by=navigation",
            "response #1 200 text/html",
            "navigation_commit / 200",
            "first_layout",
            "first_paint",
            "relayout initial_load",
            // By default a line is its arguments' types and lengths.
            "console Log <string len=3> <number> @inline#1:2:9",
            "console Info <string len=3> @inline#1:3:9",
            "console Warn <string len=5> @inline#1:4:9",
            "console Error <string len=4> @inline#1:5:9",
            "console Debug <string len=4> @inline#1:6:9",
            "script parser inline#1 ran held=DOMContentLoaded",
            "lifecycle DOMContentLoaded held_by=0",
            "lifecycle load held_by=0",
            "load_finished over_budget=0",
        ]
    );
    let records = records(&engine, view);
    assert!(records.windows(2).all(|w| w[1].seq == w[0].seq + 1), "sequence numbers are consecutive");
    assert!(records.windows(2).all(|w| w[1].t_ms >= w[0].t_ms), "times only go forward");
}

#[test]
fn a_throw_names_its_error_script_line_and_column() {
    let script = "var a = 1;\nfunction boom() {\n  null.x;\n}\nboom();\n";
    let page = "<html><body><script src=\"/app.js?session=SECRET#frag\"></script></body></html>";
    let (engine, view, server) = load_page(
        EngineConfig::default(),
        vec![Route::html("/", page), Route::ok("/app.js", "text/javascript", script)],
    );
    let lines = described(&engine, view, server.port);
    assert_eq!(
        only(&lines, &["script_error"]),
        vec!["script_error TypeError: <error len=46> @/app.js:3:8 script=/app.js rejection=false"]
    );
    let all = serde_json::to_string(&records(&engine, view)).unwrap();
    assert!(!all.contains("SECRET") && !all.contains("frag"), "{all}");
}

#[test]
fn an_unhandled_rejection_is_a_script_error_and_a_handled_one_is_not() {
    let page = "<html><body><script>
Promise.reject(new RangeError('nobody caught this'));
Promise.reject(new Error('caught')).catch(function () {});
</script></body></html>";
    let (engine, view, server) = load_page(EngineConfig::default(), vec![Route::html("/", page)]);
    assert_eq!(
        only(&described(&engine, view, server.port), &["script_error"]),
        vec!["script_error RangeError: <error len=18> @inline#1:2:16 script=inline#1 rejection=true"]
    );
}

#[test]
fn a_listener_error_names_the_step_that_ran_it_and_where_it_was_written() {
    let page = "<html><body><script>
window.addEventListener('load', function onLoad() {
  var settings = null;
  settings.theme;
});
</script></body></html>";
    let (engine, view, server) = load_page(EngineConfig::default(), vec![Route::html("/", page)]);
    assert_eq!(
        only(&described(&engine, view, server.port), &["script_error"]),
        vec!["script_error TypeError: <error len=46> @inline#1:4:12 script=event:load rejection=false"]
    );
    // The stack names the page's frames only, not the engine's own glue.
    let stack = records(&engine, view)
        .into_iter()
        .find_map(|r| match r.event {
            InspectorEvent::ScriptError { stack, .. } => stack,
            _ => None,
        })
        .expect("a stack");
    // Positions only: no function names.
    assert_eq!(stack, "    at inline#1:4:12");
}

#[test]
fn an_unsupported_api_is_recorded_once_per_page_with_its_count() {
    let page = "<html><body><script>
for (var i = 0; i < 3; i++) {
  try { new XMLHttpRequest().open('GET', '/x', false); } catch (e) {}
}
try { new DOMParser().parseFromString('<a/>', 'text/xml'); } catch (e) {}
</script></body></html>";
    let (engine, view, server) = load_page(EngineConfig::default(), vec![Route::html("/", page)]);
    assert_eq!(
        only(&described(&engine, view, server.port), &["unsupported_api"]),
        vec![
            "unsupported_api XMLHttpRequest.open(async=false) @inline#1:3:34",
            "unsupported_api DOMParser.parseFromString(XML) @inline#1:5:38",
        ]
    );
    let counts: Vec<(String, u64)> = engine
        .inspector(view)
        .unwrap()
        .unsupported_apis()
        .into_iter()
        .map(|u| (format!("{}.{}", u.interface, u.member), u.count))
        .collect();
    assert_eq!(
        counts,
        vec![
            ("XMLHttpRequest.open(async=false)".to_string(), 3),
            ("DOMParser.parseFromString(XML)".to_string(), 1)
        ]
    );
}

#[test]
fn scripts_over_the_budget_are_counted_on_load_finished() {
    // The first script spends the budget; the two after it are not started.
    let page = "<html><body><script>var t = Date.now(); while (Date.now() - t < 150) {}</script>\
        <script>console.log('a')</script><script>console.log('b')</script></body></html>";
    let config = EngineConfig { script_budget_ms: 50, ..EngineConfig::default() };
    let (engine, view, server) = load_page(config, vec![Route::html("/", page)]);
    let lines = described(&engine, view, server.port);
    assert_eq!(only(&lines, &["load_finished", "console"]), vec!["load_finished over_budget=2"]);
}

#[test]
fn a_new_navigation_starts_a_new_page_in_the_same_log() {
    let server = serve(vec![
        Route::html("/", "<html><body><script>console.log('first')</script></body></html>"),
        Route::html("/next", "<html><body><script>console.log('second')</script></body></html>"),
    ]);
    let (mut engine, view) = inspected_engine(console_text());
    let rt = runtime();
    rt.block_on(engine.load_url(view, server.url("/"))).unwrap();
    rt.block_on(engine.load_url(view, server.url("/next"))).unwrap();
    let lines = described(&engine, view, server.port);
    assert_eq!(
        only(&lines, &["navigation_start", "console", "load_finished"]),
        vec![
            "navigation_start /",
            "console Log first @inline#1:1:9",
            "load_finished over_budget=0",
            "navigation_start /next",
            "console Log second @inline#1:1:9",
            "load_finished over_budget=0",
        ]
    );
}

#[test]
fn a_document_that_cannot_be_fetched_ends_the_load_early() {
    // A port nobody listens on.
    let closed = {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    };
    let (mut engine, view) = inspected_engine(EngineConfig::default());
    let url = Url::parse(&format!("http://127.0.0.1:{closed}/")).unwrap();
    assert!(runtime().block_on(engine.load_url(view, url)).is_err());
    let lines = described(&engine, view, closed);
    assert_eq!(
        lines,
        vec![
            "navigation_start /",
            "request #1 GET document / by=navigation",
            "failure #1 connect could not connect",
            "load_ended_early Failed"
        ]
    );
}

#[test]
fn an_empty_error_response_ends_the_load_early() {
    let server = serve(vec![Route::ok("/", "text/html", "").status(404)]);
    let (mut engine, view) = inspected_engine(EngineConfig::default());
    assert!(runtime().block_on(engine.load_url(view, server.url("/"))).is_err());
    assert_eq!(
        described(&engine, view, server.port),
        vec![
            "navigation_start /",
            "request #1 GET document / by=navigation",
            "response #1 404 text/html",
            "load_ended_early Failed"
        ]
    );
}

#[test]
fn load_html_is_a_navigation_with_no_url() {
    let (mut engine, view) = inspected_engine(EngineConfig::default());
    engine.load_html(view, "<html><body>hi</body></html>").unwrap();
    assert_eq!(
        described(&engine, view, 0),
        vec![
            "navigation_start -",
            "navigation_commit - 0",
            "first_layout",
            "first_paint",
            "relayout initial_load",
            "load_finished over_budget=0",
        ]
    );
}

// ---- The tab ring: off by default, a ring from the start when on ----

/// The engine config with no ring and no sink.
fn ring_off() -> EngineConfig {
    EngineConfig { inspector: InspectorConfig::default(), ..EngineConfig::default() }
}

#[test]
fn a_tab_records_nothing_by_default_until_a_panel_attaches() {
    // #803 review: the ring stays off until a quiet A/B clears its cost.
    let server = serve(vec![Route::html("/", CONSOLE_PAGE)]);
    let config = EngineConfig::default();
    assert!(
        std::env::var_os("HIWAVE_DEVTOOLS_RING").is_some() || !config.inspector.buffer,
        "the tab ring is on by default"
    );
    let mut engine = Engine::new(config).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 300, height: 200 })
        .expect("view");
    runtime().block_on(engine.load_url(view, server.url("/"))).unwrap();
    assert!(engine.inspector(view).unwrap().snapshot_since(0).records.is_empty());
    assert_eq!(InspectorConfig::tab().capacity, crate::inspector::TAB_RING_CAPACITY);
}

#[test]
fn with_the_tab_ring_on_a_tab_keeps_a_ring_from_the_start() {
    // T6b (Atlas 10-24, Q1), `HIWAVE_DEVTOOLS_RING=1`: a panel opened late
    // has the history.
    let server = serve(vec![Route::html("/", CONSOLE_PAGE)]);
    let inspector = InspectorConfig { buffer: true, ..InspectorConfig::tab() };
    let mut engine = Engine::new(EngineConfig { inspector, ..EngineConfig::default() }).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 300, height: 200 })
        .expect("view");
    runtime().block_on(engine.load_url(view, server.url("/"))).unwrap();
    let kept: Vec<String> = records(&engine, view).iter().map(|r| describe(r, server.port)).collect();
    assert_eq!(only(&kept, &["console"]).len(), 5, "{kept:?}");
}

#[test]
fn with_nobody_listening_nothing_is_recorded() {
    let server = serve(vec![Route::html("/", CONSOLE_PAGE)]);
    let mut engine = Engine::new(ring_off()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 300, height: 200 })
        .expect("view");
    let inspector = engine.inspector(view).unwrap();
    assert!(!inspector.is_active());
    runtime().block_on(engine.load_url(view, server.url("/"))).unwrap();
    let snapshot = inspector.snapshot_since(0);
    assert!(snapshot.records.is_empty());
    assert_eq!(snapshot.next_seq, 0, "no event was even numbered");
}

#[test]
fn a_sink_alone_turns_recording_on() {
    let server = serve(vec![Route::html("/", CONSOLE_PAGE)]);
    let mut engine = Engine::new(ring_off()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 300, height: 200 })
        .expect("view");
    let sink = CollectingSink::default();
    engine.add_inspector_sink(view, Box::new(sink.clone())).unwrap();
    runtime().block_on(engine.load_url(view, server.url("/"))).unwrap();
    let streamed: Vec<String> = sink.records().iter().map(|r| describe(r, server.port)).collect();
    assert_eq!(only(&streamed, &["console"]).len(), 5, "{streamed:?}");
    // No buffer was asked for: the sink got them, the buffer kept none.
    assert!(engine.inspector(view).unwrap().snapshot_since(0).records.is_empty());
}

// ---- Reading while the page's script runs ----

#[test]
fn the_buffer_can_be_read_from_another_thread_while_script_runs() {
    // A script that logs and then spins for a while.
    let page = "<html><body><script>
console.log('started');
var t = Date.now(); while (Date.now() - t < 400) {}
console.log('done');
</script></body></html>";
    let server = serve(vec![Route::html("/", page)]);
    let (mut engine, view) = inspected_engine(EngineConfig::default());
    let inspector = engine.inspector(view).unwrap();
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let reader = {
        let stop = stop.clone();
        std::thread::spawn(move || {
            let mut reads = 0u32;
            let mut most = 0usize;
            while !stop.load(std::sync::atomic::Ordering::SeqCst) {
                let snapshot = inspector.snapshot_since(0);
                most = most.max(snapshot.records.len());
                reads += 1;
                std::thread::sleep(Duration::from_millis(5));
            }
            (reads, most)
        })
    };
    runtime().block_on(engine.load_url(view, server.url("/"))).unwrap();
    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    let (reads, most) = reader.join().unwrap();
    assert!(reads > 10, "the reader was never blocked for the whole load ({reads} reads)");
    assert!(most > 0);
}

#[test]
fn a_console_line_is_read_while_the_script_that_logged_it_still_runs() {
    // T6d: console calls waited for the script's end, so a reader on
    // another thread (the devtools thread) saw nothing of a long script's
    // console until it was over.
    let page = "<html><body><script>
console.log('started');
var t = Date.now(); while (Date.now() - t < 1000) {}
console.log('done');
</script></body></html>";
    let server = serve(vec![Route::html("/", page)]);
    let (mut engine, view) = inspected_engine(EngineConfig::default());
    let inspector = engine.inspector(view).unwrap();
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let reader = {
        let stop = stop.clone();
        std::thread::spawn(move || {
            while !stop.load(std::sync::atomic::Ordering::SeqCst) {
                let snapshot = inspector.snapshot_since(0);
                if snapshot.records.iter().any(|r| matches!(r.event, InspectorEvent::Console { .. })) {
                    return Some(Instant::now());
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            None
        })
    };
    runtime().block_on(engine.load_url(view, server.url("/"))).unwrap();
    let ended = Instant::now();
    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    let seen = reader.join().unwrap().expect("no console line at all");
    let early = ended.saturating_duration_since(seen);
    assert!(early >= Duration::from_millis(400), "the line was read only {early:?} before the load ended");
}

#[test]
fn console_calls_held_by_a_hung_script_are_counted_without_the_page() {
    // T6d review: a hot loop past the live cap, then a hang. The calls
    // over the cap wait for the script's end, which a hung script never
    // reaches, and nothing said any were waiting.
    let page = "<html><body><script>
for (var i = 0; i < 300; i++) console.log(i);
var t = Date.now(); while (Date.now() - t < 1000) {}
</script></body></html>";
    let server = serve(vec![Route::html("/", page)]);
    let (mut engine, view) = inspected_engine(EngineConfig::default());
    let inspector = engine.inspector(view).unwrap();
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let reader = {
        let (stop, inspector) = (stop.clone(), inspector.clone());
        std::thread::spawn(move || {
            // The last count seen while the script ran, with the console
            // records there were then.
            let mut last = None;
            while !stop.load(std::sync::atomic::Ordering::SeqCst) {
                // Count, records, count again: a flush landing between the
                // reads zeroes the count first, and the pair is skipped.
                let held = inspector.console_held();
                let snapshot = inspector.snapshot_since(0);
                let recorded =
                    snapshot.records.iter().filter(|r| matches!(r.event, InspectorEvent::Console { .. })).count();
                if held > 0 && inspector.console_held() == held {
                    last = Some((held, recorded));
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            last
        })
    };
    runtime().block_on(engine.load_url(view, server.url("/"))).unwrap();
    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    let (held, recorded) = reader.join().unwrap().expect("no held calls were ever counted");
    assert_eq!(held + recorded, 300, "held {held} + recorded {recorded}");
    assert_eq!(inspector.console_held(), 0, "the flush at the script's end left a count");
}

// ---- Privacy ----

#[test]
fn urls_in_events_lose_credentials_query_and_fragment() {
    let url = Url::parse("https://user:pw@example.test:8443/a/b.js?token=1&x=2#frag").unwrap();
    assert_eq!(scrub_url(&url, false), "https://example.test:8443/a/b.js");
    assert_eq!(scrub_url(&url, true), "https://example.test:8443/a/b.js?token=1&x=2#frag");
    let data = Url::parse("data:text/css;base64,Ym9keXt9").unwrap();
    assert_eq!(scrub_url(&data, true), "data:text/css,…");
    // R1 note @ 6bda3b2c: the MIME type is the page's text unless it is a
    // common one.
    for (u, want) in [
        ("data:TEXT/JavaScript,x", "data:text/javascript,…"),
        ("data:text/javascriptSECRET,x", "data:,…"),
        ("data:SECRET/x;base64,AAAA", "data:,…"),
        ("data:,x", "data:,…"),
    ] {
        assert_eq!(scrub_url(&Url::parse(u).unwrap(), false), want, "{u:?}");
    }
    let blob = Url::parse("blob:https://example.test/1234-5678").unwrap();
    assert_eq!(scrub_url(&blob, false), "blob:https://example.test");
}

/// R1 M1 @ 9ceed2a5: `javascript://h/alert(1)` has an authority, so it is a
/// base URL to the parser and the `cannot_be_a_base` arm missed it; the
/// script is the page's text and is never printed, with either flag.
#[test]
fn javascript_urls_in_events_always_reduce_to_their_scheme() {
    for u in ["javascript:alert('SECRET')", "javascript://h/alert(SECRET)", "javascript://alert(1)"] {
        for keep in [false, true] {
            let out = scrub_url(&Url::parse(u).unwrap(), keep);
            assert_eq!(out, "javascript:", "{u:?} keep={keep}");
            assert!(!out.contains('(') && !out.contains("SECRET"), "{u:?} -> {out}");
        }
    }
}

/// Third R1 hold @ 6bda3b2c: a `;` path parameter (a servlet `jsessionid`
/// is the session cookie) goes with the query, in every segment, unless the
/// query is kept. The same table as the diagnostics half's `report_url`
/// (#674), so the two halves of the Inspector agree.
#[test]
fn urls_in_events_lose_path_parameters_unless_the_query_is_kept() {
    let r = |u: &str, q| scrub_url(&Url::parse(u).unwrap(), q);
    assert_eq!(r("https://shop.test/cart;jsessionid=0123ABCD?x=1", false), "https://shop.test/cart");
    assert_eq!(r("https://shop.test/cart;jsessionid=0123ABCD?x=1", true), "https://shop.test/cart;jsessionid=0123ABCD?x=1");
    for (u, want) in [
        ("https://shop.test/a;jsessionid=S/b;v=1/c.js", "https://shop.test/a/b/c.js"),
        ("https://shop.test/a;x=1;JSESSIONID=S", "https://shop.test/a"),
        ("https://shop.test/a%3Bjsessionid=S/b", "https://shop.test/a/b"),
        ("https://shop.test/a%3bjsessionid=S/b", "https://shop.test/a/b"),
        ("https://shop.test/a;", "https://shop.test/a"),
        ("https://shop.test/;jsessionid=S", "https://shop.test/"),
        ("https:\\\\shop.test\\a;jsessionid=S\\b", "https://shop.test/a/b"),
        ("https://shop.test/a;jsess\tionid=S\n/b", "https://shop.test/a/b"),
        ("http://shop.test:8080/a;jsessionid=S#f", "http://shop.test:8080/a"),
        ("foo://shop.test/a;sid=S/b", "foo://shop.test/a/b"),
        ("https://u:pw@shop.test/a;jsessionid=S", "https://shop.test/a"),
        // `data:` and `blob:` keep their own reduction.
        ("blob:https://shop.test/a;jsessionid=S", "blob:https://shop.test"),
    ] {
        assert_eq!(r(u, false), want, "{u:?}");
    }
    assert_eq!(r("https://shop.test/a%3Bjsessionid=S/b", true), "https://shop.test/a%3Bjsessionid=S/b");
}

#[test]
fn the_navigation_url_keeps_its_query_only_when_asked() {
    let server = serve(vec![Route::html("/", "<html><body>x</body></html>")]);
    for (keep, expected) in [(false, "navigation_start /"), (true, "navigation_start /?q=private")] {
        let mut config = EngineConfig::default();
        config.inspector.keep_url_query = keep;
        let (mut engine, view) = inspected_engine(config);
        runtime().block_on(engine.load_url(view, server.url("/?q=private"))).unwrap();
        assert_eq!(described(&engine, view, server.port)[0], expected);
    }
}

#[test]
fn console_messages_are_capped_and_kept_as_text() {
    let page = "<html><body><script>
console.log('x'.repeat(5000));
console.log('{\"type\":\"navigation_start\"}\\n<' + 'script>alert(1)<' + '/script>');
</script></body></html>";
    let (engine, view, _server) = load_page(console_text(), vec![Route::html("/", page)]);
    let consoles: Vec<(String, bool)> = records(&engine, view)
        .into_iter()
        .filter_map(|r| match r.event {
            InspectorEvent::Console { message, truncated, .. } => Some((message, truncated)),
            _ => None,
        })
        .collect();
    assert_eq!(consoles.len(), 2);
    assert_eq!(consoles[0].0.chars().count(), MAX_MESSAGE_CHARS);
    assert!(consoles[0].1);
    assert_eq!(consoles[1].0, "{\"type\":\"navigation_start\"}\n<script>alert(1)</script>");
    assert!(!consoles[1].1);
    // In JSON Lines it stays one line and one string.
    let mut out = Vec::new();
    {
        let mut sink = crate::inspector::JsonLinesSink::new(&mut out);
        for r in records(&engine, view) {
            sink.record(&r);
        }
    }
    let text = String::from_utf8(out).unwrap();
    for line in text.lines() {
        let back: InspectorRecord = serde_json::from_str(line).expect("each line is one record");
        assert!(back.seq < 1000);
    }
}

#[test]
fn no_cookie_storage_or_form_value_reaches_the_log() {
    // Round 1 (console) and the R1 round-2 tables on #676 @ 56cbdfed
    // (thrown and rejected values, URL shapes the text scrubber missed),
    // one `<script>` per row so every row runs.
    let rows: &[&str] = &[
        "console.log(document.cookie);",
        "console.log(location.href);",
        "console.log(document.getElementById('pw').value);",
        "console.info('stored', localStorage.getItem('k'));",
        "console.log(new Error('see https://x.test/p?token=ERRQUERY'));",
        "throw new Error(document.cookie);",
        "throw document.cookie;",
        "throw new Error('q ' + location.search);",
        "throw document.getElementById('pw').value;",
        "Promise.reject(document.cookie);",
        "Promise.reject(new Error('rej ' + location.search));",
        "Promise.reject(document.getElementById('pw').value);",
        "(async () => { throw new Error('async ' + document.cookie); })();",
        "window.addEventListener('load', function () { throw new Error('load ' + document.cookie); });",
        "setTimeout(function () { throw 'timer ' + location.search; }, 0);",
        "var e = new Error('named'); e.name = 'SECRET_NAME'; throw e;",
        "throw Symbol('SECRET_SYM');",
        "throw new Error('http://[::1]:9/x?token=URLSECRET_V6');",
        "throw new Error('ht\\ttps://h.test/x?token=URLSECRET_TAB');",
        "throw new Error('da\\tta:text/plain,URLSECRET_DATATAB');",
        "throw new Error('//h.test/x?token=URLSECRET_PROTOREL');",
        "throw new Error('/api?token=URLSECRET_REL');",
        "throw new Error('https://h.test/a\"b?token=URLSECRET_QUOTE');",
        "throw new Error('https:\\\\h.test\\\\x?token=URLSECRET_BACKSLASH');",
        "throw new Error('ftp://u:URLPASSWORD@h.test/x?token=URLSECRET_FTP');",
    ];
    let mut page = String::from(
        "<html><body><form><input id=pw type=password value=hunter2></form><script>
document.cookie = 'sid=COOKIEVALUE';
try { localStorage.setItem('k', 'STOREDVALUE'); } catch (e) {}
document.getElementById('pw').value = 'TYPEDVALUE';
</script>",
    );
    for row in rows {
        page.push_str(&format!("<script>{row}</script>"));
    }
    // Third R1 hold @ 6bda3b2c: a `;` path parameter (a servlet's
    // `jsessionid`, the session cookie when cookies are off) in a script's
    // own URL, plain and percent-encoded, classic and module.
    page.push_str(
        "<script src=\"/s.js;jsessionid=JSESSIONSECRET?token=SCRIPTQUERY\"></script>\
<script src=\"/p.js%3Bjsessionid=JSESSIONSECRET/x.js\"></script>\
<script type=module src=\"/m.js;jsessionid=JSESSIONSECRET?token=SCRIPTQUERY\"></script>",
    );
    // Fourth R1 hold @ 47fc7abb: script gives an element the cookie as its
    // id, makes a custom element named after the cookie, and writes to
    // each. The engine lays a load's writes out between its steps, so each
    // step's first write is a `relayout` record of its own: the id at
    // `DOMContentLoaded`, the custom element at `load`, writes to it in a
    // timer, and one more from the embedder after the load.
    page.push_str(
        "<div id=t>x</div><script>
document.addEventListener('DOMContentLoaded', function () {
  var d = document.getElementById('t'); d.id = document.cookie; d.setAttribute('data-x', '1');
});
window.addEventListener('load', function () {
  var c = document.createElement('x-' + document.cookie.split('sid=')[1].toLowerCase());
  c.id = 'el-' + document.cookie; c.setAttribute('class', 'cls-' + document.cookie);
  document.body.appendChild(c);
  setTimeout(function () { c.setAttribute('data-y', '1'); c.textContent = 'SECRET_TEXT'; }, 0);
});
</script>",
    );
    page.push_str("</body></html>");
    let script = "console.log('s'); throw new Error('s');";
    let server = serve(vec![
        Route::html("/", page).header("Set-Cookie", "server=SETCOOKIEVALUE"),
        Route::ok("/s.js;jsessionid=JSESSIONSECRET", "text/javascript", script),
        Route::ok("/p.js%3Bjsessionid=JSESSIONSECRET/x.js", "text/javascript", script),
        Route::ok("/m.js;jsessionid=JSESSIONSECRET", "text/javascript", script),
    ]);
    let (mut engine, view) = inspected_engine(EngineConfig::default());
    runtime().block_on(engine.load_url(view, server.url("/?token=HREFQUERY"))).expect("load_url");
    engine
        .execute_script(view, "document.getElementById('el-' + document.cookie).style.color = 'red';")
        .expect("execute_script");
    let records = records(&engine, view);
    let causes: Vec<&str> = records
        .iter()
        .filter_map(|r| match &r.event {
            InspectorEvent::Relayout { cause, .. } => Some(cause.as_str()),
            _ => None,
        })
        .collect();
    assert!(
        causes.iter().filter(|c| c.starts_with("script_mutation(")).count() >= 4,
        "the writes are still laid out and recorded: {causes:?}"
    );
    assert_eq!(
        records.iter().filter(|r| r.event.kind() == "console").count(),
        8,
        "the calls are still recorded"
    );
    // 17 throws, 4 rejections, a listener and a timer: all still recorded.
    assert_eq!(
        records.iter().filter(|r| r.event.kind() == "script_error").count(),
        23,
        "the errors are still recorded: {records:?}"
    );
    let all = serde_json::to_string(&records).unwrap();
    for secret in [
        "COOKIEVALUE",
        "STOREDVALUE",
        "TYPEDVALUE",
        "hunter2",
        "SETCOOKIEVALUE",
        "HREFQUERY",
        "ERRQUERY",
        "SECRET_NAME",
        "SECRET_SYM",
        "URLSECRET",
        "URLPASSWORD",
        "JSESSIONSECRET",
        "SCRIPTQUERY",
        "SECRET_TEXT",
    ] {
        assert!(!all.contains(secret), "{secret} leaked: {all}");
    }
    // A tag name is lowercased, so the custom element's name holds it so.
    assert!(!all.contains("cookievalue"), "custom element name leaked: {all}");
    assert!(!all.contains("sid="), "an id leaked: {all}");
}


/// A console line is its arguments' types (and a string's length) unless
/// console text is asked for; even then a URL in it loses its query.
#[test]
fn console_text_is_kept_only_when_asked_and_its_urls_are_scrubbed() {
    let page = "<html><body><script>
console.log('https://h.test/p?token=Q', 'x', 12, true, null, undefined, {a: 1}, [1], new TypeError('a secret'));
</script></body></html>";
    let server = serve(vec![Route::html("/", page)]);
    let mut lines = Vec::new();
    for config in [EngineConfig::default(), console_text()] {
        let (mut engine, view) = inspected_engine(config);
        runtime().block_on(engine.load_url(view, server.url("/"))).unwrap();
        lines.push(only(&described(&engine, view, server.port), &["console"]));
    }
    assert_eq!(
        lines,
        vec![
            vec!["console Log <string len=24> <string len=1> <number> true null undefined [object Object] [object Array] <TypeError message len=8> @inline#1:2:9"],
            vec!["console Log https://h.test/p x 12 true null undefined [object Object] [object Array] TypeError: a secret @inline#1:2:9"],
        ]
    );
}

/// `HIWAVE_CONSOLE_LOG` mirrors the text the page logged into the tracing
/// log. The Inspector's own rule still holds while it is on: without
/// `keep_console_text` the record is shapes only, and the mirror keeps its
/// text.
#[test]
fn the_console_mirror_does_not_put_page_text_in_the_inspector() {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            crate::console_log::force_for_test(None);
            crate::console_log::install_sink(None);
        }
    }
    let _reset = Reset;
    crate::console_log::force_for_test(Some(true));
    let mirrored = Arc::new(std::sync::Mutex::new(Vec::new()));
    crate::console_log::install_sink(Some(Arc::clone(&mirrored)));
    let page = "<html><body><script>
console.log('secret-console-text', 12);
console.count('secret-label');
</script></body></html>";
    let server = serve(vec![Route::html("/", page)]);
    let mut lines = Vec::new();
    for config in [EngineConfig::default(), console_text()] {
        let (mut engine, view) = inspected_engine(config);
        runtime().block_on(engine.load_url(view, server.url("/"))).unwrap();
        lines.push(only(&described(&engine, view, server.port), &["console"]));
    }
    assert_eq!(
        lines,
        vec![
            vec!["console Log <string len=19> <number> @inline#1:2:9", "console Info <string len=15> @inline#1:3:10"],
            vec!["console Log secret-console-text 12 @inline#1:2:9", "console Info secret-label: 1 @inline#1:3:10"],
        ]
    );
    let mirrored = mirrored.lock().unwrap().join("\n");
    assert!(mirrored.contains("secret-console-text 12"), "{mirrored}");
}

/// Prometheus R1 on #791: with the mirror on, `attach` sets the runtime's
/// error `keep_text`. An uncaught `throw` and a `console.error(new Error)`
/// still reach the Inspector as shapes only: no page message and no page
/// function name in any record (message, stack, error_stack).
#[test]
fn with_the_mirror_on_errors_and_error_stacks_reach_the_inspector_as_shapes() {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            crate::console_log::force_for_test(None);
            crate::console_log::install_sink(None);
        }
    }
    let _reset = Reset;
    crate::console_log::force_for_test(Some(true));
    let mirrored = Arc::new(std::sync::Mutex::new(Vec::new()));
    crate::console_log::install_sink(Some(Arc::clone(&mirrored)));
    let page = "<html><body><script>
function secretOuter() { secretInner(); }
function secretInner() {
  console.error(new Error('secret-logged-text'));
  throw new TypeError('secret-thrown-text');
}
secretOuter();
</script></body></html>";
    let server = serve(vec![Route::html("/", page)]);
    let (mut engine, view) = inspected_engine(EngineConfig::default());
    runtime().block_on(engine.load_url(view, server.url("/"))).unwrap();
    let records = engine.inspector(view).expect("inspector").snapshot_since(0).records;
    let kinds: Vec<&str> = records.iter().map(|r| r.event.kind()).collect();
    assert!(kinds.contains(&"script_error"), "no script_error record: {kinds:?}");
    let errors: Vec<String> = records
        .iter()
        .filter(|r| matches!(r.event.kind(), "console" | "script_error"))
        .map(|r| serde_json::to_string(r).unwrap())
        .collect();
    assert!(errors.iter().any(|r| r.contains("\"level\":\"error\"")), "no console.error record: {errors:?}");
    for record in &errors {
        assert!(!record.contains("secret"), "page text reached the Inspector with the mirror on: {record}");
    }
}

/// Atlas 09:40 on #791's develop merge (#784's console stacks against T1b's
/// shapes): with the mirror on and console text off, a `console.error`'s
/// caller stack and its error's own stack still reach the Inspector, as
/// places only. The shapes path drops function names itself, so this holds
/// even where the Inspector keeps error text.
#[test]
fn with_the_mirror_on_console_error_stacks_are_shapes_only() {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            crate::console_log::force_for_test(None);
            crate::console_log::install_sink(None);
        }
    }
    let _reset = Reset;
    crate::console_log::force_for_test(Some(true));
    let mirrored = Arc::new(std::sync::Mutex::new(Vec::new()));
    crate::console_log::install_sink(Some(Arc::clone(&mirrored)));
    // Prometheus's nit: the frames come from a script whose URL carries a
    // query, which must not reach the Inspector either.
    let page = "<html><body><script src=\"/app.js?token=SECRET\"></script></body></html>";
    let app = "function secretMaker() { return new Error('SECRET'); }
function secretCaller() { console.error(secretMaker()); }
secretCaller();";
    let server = serve(vec![Route::html("/", page), Route::ok("/app.js", "text/javascript", app)]);
    let mut error_text = EngineConfig::default();
    error_text.inspector.keep_error_text = true;
    for config in [EngineConfig::default(), error_text] {
        let (mut engine, view) = inspected_engine(config);
        runtime().block_on(engine.load_url(view, server.url("/"))).unwrap();
        let records = engine.inspector(view).expect("inspector").snapshot_since(0).records;
        let console: Vec<serde_json::Value> = records
            .iter()
            .filter(|r| r.event.kind() == "console")
            .map(|r| serde_json::to_value(r).unwrap())
            .collect();
        let error = console.iter().find(|r| r["level"] == "error").unwrap_or_else(|| panic!("{console:#?}"));
        assert!(error["stack"].is_string() && error["error_stack"].is_string(), "{error:#}");
        let text = error.to_string();
        assert!(!text.contains("SECRET") && !text.contains("secret"), "page text in a shapes record: {text}");
        assert!(!text.contains("token") && text.contains("/app.js"), "frames name the script, no query: {text}");
    }
    assert!(mirrored.lock().unwrap().join("\n").contains("SECRET"), "the mirror keeps its text");
}

/// Atlas 10-26 20:50 (3): a page-defined `Error` subclass, by its class
/// name and by a `name` it sets, thrown uncaught and passed to
/// `console.error`, with the mirror off and on. The record's error name is
/// a standard one (`standard_name`), so the page's name reaches no record.
#[test]
fn a_page_defined_error_subclass_name_reaches_no_record() {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            crate::console_log::force_for_test(None);
            crate::console_log::install_sink(None);
        }
    }
    let _reset = Reset;
    let page = "<html><body><script src=\"/app.js\"></script><script src=\"/throw.js\"></script></body></html>";
    let app = "class SecretLeak extends Error {}
class NamedLeak extends Error { constructor(m) { super(m); this.name = 'SecretName'; } }
console.error(new SecretLeak('x'));
console.error(new NamedLeak('y'));
Promise.reject(new NamedLeak('z'));";
    let throws = "class ThrownLeak extends Error { constructor(m) { super(m); this.name = 'SecretName'; } }
function thrower() { throw new ThrownLeak('w'); }
thrower();";
    let server = serve(vec![
        Route::html("/", page),
        Route::ok("/app.js", "text/javascript", app),
        Route::ok("/throw.js", "text/javascript", throws),
    ]);
    for mirror in [false, true] {
        crate::console_log::force_for_test(Some(mirror));
        let (mut engine, view) = inspected_engine(EngineConfig::default());
        runtime().block_on(engine.load_url(view, server.url("/"))).unwrap();
        let records = engine.inspector(view).expect("inspector").snapshot_since(0).records;
        let kinds: Vec<&str> = records.iter().map(|r| r.event.kind()).collect();
        assert!(kinds.contains(&"script_error"), "mirror {mirror}: no script_error in {kinds:?}");
        assert!(kinds.iter().filter(|k| **k == "console").count() >= 2, "mirror {mirror}: {kinds:?}");
        let text = serde_json::to_string(&records).unwrap();
        for name in ["SecretLeak", "SecretName", "NamedLeak", "ThrownLeak"] {
            assert!(!text.contains(name), "mirror {mirror}: the page's error name {name} reached a record: {text}");
        }
    }
}

/// Prometheus's #799 nit 3 (Atlas 20:50 (4)): an error thrown from a
/// callback, not from a script's own run, still reaches the Inspector as a
/// `script_error`: a timer, an animation frame and an event listener, each
/// with its own error type so the three are told apart.
#[test]
fn errors_thrown_in_timer_frame_and_listener_callbacks_reach_the_inspector() {
    let page = "<html><body style=\"margin:0\"><p id=p style=\"margin:0;height:100px\">x</p><script>
setTimeout(function () { throw new TypeError('t'); }, 0);
requestAnimationFrame(function () { throw new RangeError('r'); });
document.getElementById('p').addEventListener('click', function () { throw new EvalError('l'); });
</script></body></html>";
    let server = serve(vec![Route::html("/", page)]);
    let (mut engine, view) = inspected_engine(EngineConfig::default());
    runtime().block_on(engine.load_url(view, server.url("/"))).unwrap();
    for _ in 0..5 {
        runtime().block_on(engine.pump_live(view, 20));
    }
    engine.click_at_point(view, 20.0, 20.0);
    let errors: Vec<String> = records(&engine, view)
        .iter()
        .filter(|r| r.event.kind() == "script_error")
        .map(|r| serde_json::to_value(r).unwrap()["name"].as_str().unwrap_or("").to_string())
        .collect();
    for (callback, name) in [("timer", "TypeError"), ("animation frame", "RangeError"), ("listener", "EvalError")] {
        assert!(errors.iter().any(|e| e == name), "the {callback}'s {name} never reached the Inspector: {errors:?}");
    }
}

/// #791 review: `count` had the only shape-path test. A failed
/// `console.assert` and a `console.timeLog` label reach the Inspector as
/// shapes too, with the mirror on; with console text kept, the text is there
/// (so the test can see it).
#[test]
fn with_the_mirror_on_assert_and_time_log_are_shapes_only() {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            crate::console_log::force_for_test(None);
            crate::console_log::install_sink(None);
        }
    }
    let _reset = Reset;
    crate::console_log::force_for_test(Some(true));
    let mirrored = Arc::new(std::sync::Mutex::new(Vec::new()));
    crate::console_log::install_sink(Some(Arc::clone(&mirrored)));
    let page = "<html><body><script>
console.assert(false, 'secret-assert');
console.time('secret-timer');
console.timeLog('secret-timer');
</script></body></html>";
    let server = serve(vec![Route::html("/", page)]);
    let mut texts = Vec::new();
    for config in [EngineConfig::default(), console_text()] {
        let (mut engine, view) = inspected_engine(config);
        runtime().block_on(engine.load_url(view, server.url("/"))).unwrap();
        let console: Vec<String> = engine
            .inspector(view)
            .expect("inspector")
            .snapshot_since(0)
            .records
            .iter()
            .filter(|r| r.event.kind() == "console")
            .map(|r| serde_json::to_string(r).unwrap())
            .collect();
        assert_eq!(console.len(), 2, "one record for the assert, one for timeLog: {console:#?}");
        texts.push(console.join("\n"));
    }
    assert!(!texts[0].contains("secret"), "page text in a shapes record: {}", texts[0]);
    assert!(texts[1].contains("secret-assert") && texts[1].contains("secret-timer"), "{}", texts[1]);
    assert!(mirrored.lock().unwrap().join("\n").contains("secret-assert"), "the mirror keeps its text");
}

// ---- Neighbouring behaviour: what naming the page's scripts must not change ----

/// Page scripts now run under a name (`inline#n`, `script#n`) so positions
/// can name them. The name is deliberately not a URL: a classic script's
/// dynamic `import()` must keep resolving against the document, with the
/// Inspector on or off.
#[test]
fn a_classic_scripts_dynamic_import_still_resolves_against_the_document() {
    let routes = vec![
        Route::html("/", "<html><body><script src=\"/js/app.js\"></script></body></html>"),
        Route::ok("/js/app.js", "text/javascript", "import('./mod.js').then(function (m) { window.got = m.v; });"),
        Route::ok("/mod.js", "text/javascript", "export const v = 'document-relative';"),
        Route::ok("/js/mod.js", "text/javascript", "export const v = 'script-relative';"),
    ];
    for buffer in [false, true] {
        let server = serve(routes.clone());
        let mut config = EngineConfig::default();
        config.inspector.buffer = buffer;
        let mut engine = Engine::new(config).expect("engine");
        let view = engine
            .create_headless_view(Bounds { x: 0, y: 0, width: 300, height: 200 })
            .expect("view");
        runtime().block_on(engine.load_url(view, server.url("/"))).unwrap();
        assert_eq!(
            engine.execute_script(view, "String(window.got)").unwrap(),
            r#"String("document-relative")"#,
            "inspector buffer {buffer}"
        );
    }
}

/// A listener that throws while a script dispatches to it (`click()`) is
/// that script's in the Inspector, but the script log records it under the
/// lifecycle step that drains next, as before: the script itself ran, so the
/// load did not lose it.
#[test]
fn a_listener_error_during_a_script_is_not_the_script_throwing() {
    let page = "<html><body><button>b</button><script>
var b = document.querySelector('button');
b.addEventListener('click', function () { throw new Error('cb'); });
b.click();
</script></body></html>";
    let (engine, view, _server) = load_page(EngineConfig::default(), vec![Route::html("/", page)]);
    let log: Vec<(String, ScriptOutcome)> = engine
        .script_log(view)
        .unwrap()
        .iter()
        .map(|r| (r.source.clone(), r.outcome.clone()))
        .collect();
    assert_eq!(log[0], ("inline#1".to_string(), ScriptOutcome::Ran), "{log:?}");
    assert!(
        log.iter().any(|(source, outcome)| source == "event:DOMContentLoaded"
            && matches!(outcome, ScriptOutcome::Threw(text) if text.starts_with("Error: cb"))),
        "{log:?}"
    );
    let scripts: Vec<String> = records(&engine, view)
        .into_iter()
        .filter_map(|r| match r.event {
            InspectorEvent::ScriptError { script, .. } => Some(script),
            _ => None,
        })
        .collect();
    assert_eq!(scripts, vec!["inline#1".to_string()]);
}

/// The script log's exception text now names the script where Boa used to
/// print `unknown`; the board's counts are unchanged (unhandled rejections
/// and unsupported APIs are Inspector events only, never log records).
#[test]
fn the_script_log_counts_are_the_same_with_the_inspector_on_or_off() {
    let page = "<html><body><script>
null.x;
</script><script>
Promise.reject(new Error('r'));
try { new XMLHttpRequest().open('GET', '/x', false); } catch (e) {}
</script><script src=\"/ext.js\"></script></body></html>";
    let mut logs = Vec::new();
    let server = serve(vec![Route::html("/", page), Route::ok("/ext.js", "text/javascript", "undefined.boom;")]);
    for buffer in [false, true] {
        let mut config = EngineConfig::default();
        config.inspector.buffer = buffer;
        let mut engine = Engine::new(config).expect("engine");
        let view = engine
            .create_headless_view(Bounds { x: 0, y: 0, width: 300, height: 200 })
            .expect("view");
        runtime().block_on(engine.load_url(view, server.url("/"))).unwrap();
        let log: Vec<(String, ScriptOutcome)> = engine
            .script_log(view)
            .unwrap()
            .iter()
            .map(|r| (r.source.clone(), r.outcome.clone()))
            .collect();
        logs.push(log);
    }
    assert_eq!(logs[0], logs[1]);
    assert_eq!(
        logs[0][..2],
        vec![
            (
                "inline#1".to_string(),
                ScriptOutcome::Threw(
                    "TypeError: cannot convert 'null' or 'undefined' to object (inline#1:2:6)\n    at <main> (inline#1:2:6)".into()
                )
            ),
            ("inline#2".to_string(), ScriptOutcome::Ran),
        ]
    );
    // An external script runs as `script#n` either way.
    let (_, ext) = &logs[0][2];
    assert!(matches!(ext, ScriptOutcome::Threw(text) if text.contains("(script#1:1:")), "{ext:?}");
}

// ---- Review R1 (#676): modules, handlers, eval, page code, scrubbing ----

/// A module's exception is one `script_error`, with its stack, after what
/// the module logged, and never an unhandled rejection (the module's
/// evaluation promise is the engine's own). An inline module is `inline#n`.
#[test]
fn a_module_exception_is_one_error_after_its_console_lines() {
    let page = "<html><body><script type=\"module\">
var c = 3;
  console.log('mod-inline-log');
null.modinl;
</script><script type=\"module\" src=\"/mod.js\"></script></body></html>";
    let (engine, view, server) = load_page(
        console_text(),
        vec![
            Route::html("/", page),
            Route::ok("/mod.js", "text/javascript", "var a = 1;\nconsole.log('ext-mod-log');\nvar b = 2;\n  null.modext;\n"),
        ],
    );
    let lines = described(&engine, view, server.port);
    assert_eq!(
        only(&lines, &["console", "script_error"]),
        vec![
            "console Log mod-inline-log @inline#1:3:11",
            "script_error TypeError: <error len=46> @inline#1:4:6 script=inline#1 rejection=false",
            "console Log ext-mod-log @/mod.js:2:9",
            "script_error TypeError: <error len=46> @/mod.js:4:8 script=/mod.js rejection=false",
        ]
    );
    let summary = InspectorSummary::from_records(&records(&engine, view), &[]);
    assert_eq!((summary.script_errors, summary.unhandled_rejections), (2, 0));
    let stacks: Vec<Option<String>> = records(&engine, view)
        .into_iter()
        .filter_map(|r| match r.event {
            InspectorEvent::ScriptError { stack, .. } => Some(stack),
            _ => None,
        })
        .collect();
    assert!(stacks.iter().all(|s| s.is_some()), "every module error has its stack: {stacks:?}");
}

/// An `on*` attribute's handler and `eval` code are in no named script:
/// their console calls and errors have no position (never the caller's),
/// and a handler's error is the script's whose `click()` ran it, recorded
/// before the next script runs (after that script's own console lines).
#[test]
fn handler_and_eval_code_are_never_given_their_callers_position() {
    let page = "<html><body><button onclick=\"var k=1;
  console.log('onclick-log'); null.clickboom;\">b</button><script>var btn = document.querySelector('button');
  btn.click();
console.log('after-click');
eval(\"console.log('in-eval')\");
</script><script>console.log('second-script');</script></body></html>";
    let (engine, view, server) = load_page(console_text(), vec![Route::html("/", page)]);
    let lines = only(&described(&engine, view, server.port), &["console", "script_error"]);
    assert_eq!(
        lines,
        vec![
            "console Log onclick-log",
            "console Log after-click @inline#1:3:9",
            "console Log in-eval",
            "script_error TypeError: <error len=46> script=inline#1 rejection=false",
            "console Log second-script @inline#2:1:9",
        ]
    );
    let all = serde_json::to_string(&records(&engine, view)).unwrap();
    assert!(!all.contains("eval at"), "{all}");
}

/// Fifth R1 hold @ 5beb2615: a function's name is the page's. A function
/// `eval` code declared has no place whatever it is called, in a script or
/// in a timer: `anonymous` is placed as `gg` is, never at its caller's.
#[test]
fn a_function_named_anonymous_is_placed_as_any_other() {
    let page = "<html><body><script>eval('function gg(){ console.log(\"gg\"); throw new Error(\"x\"); }'); gg();</script>\
<script>eval('function anonymous(){ console.log(\"anonymous\"); throw new Error(\"x\"); }'); anonymous();</script>\
<script>setTimeout(function () { eval('function gt(){ throw new Error(\"x\"); }'); gt(); }, 0);\
setTimeout(function () { eval('function anonymous(){ throw new Error(\"x\"); }'); anonymous(); }, 0);</script></body></html>";
    let (engine, view, server) = load_page(console_text(), vec![Route::html("/", page)]);
    let lines = only(&described(&engine, view, server.port), &["console", "script_error"]);
    assert_eq!(lines.len(), 6, "{lines:?}");
    assert!(lines.iter().all(|l| !l.contains(" @")), "{lines:?}");
    let located: Vec<_> = records(&engine, view)
        .into_iter()
        .filter_map(|r| match r.event {
            InspectorEvent::ScriptError { location, .. } => Some(location),
            _ => None,
        })
        .collect();
    assert_eq!(located, vec![None, None, None, None]);
}

/// A throw inside `eval` code has no place (Boa names no script for it),
/// and is not given the `eval` call's; its stack keeps the page's frames.
#[test]
fn an_error_thrown_in_eval_code_has_no_borrowed_place() {
    let page = "<html><body><script>
var x = 1;
  eval('null.inEval');
</script></body></html>";
    let (engine, view, server) = load_page(EngineConfig::default(), vec![Route::html("/", page)]);
    let errors = only(&described(&engine, view, server.port), &["script_error"]);
    assert_eq!(
        errors,
        vec!["script_error TypeError: <error len=46> script=inline#1 rejection=false"]
    );
    let stack = records(&engine, view)
        .into_iter()
        .find_map(|r| match r.event {
            InspectorEvent::ScriptError { stack, .. } => stack,
            _ => None,
        })
        .unwrap();
    // The `eval(` call, as Boa places calls (at the `(`).
    assert_eq!(stack, "    at inline#1:3:7");
}

/// Logging an object runs none of the page's code: the next script sees
/// what it would have with the Inspector off (and in Chrome).
#[test]
fn logging_runs_no_page_code() {
    let page = "<html><body><script>var n = 0; console.log({ toString() { n += 100; return 'side'; } });
var p = new Proxy({}, { get() { n += 1; throw new Error('trap'); } }); console.log(p);
Promise.reject({ toString() { n += 1000; return 'r'; } });</script>
<script>console.info('n seen by script 2 = ' + n);</script></body></html>";
    let (engine, view, server) = load_page(console_text(), vec![Route::html("/", page)]);
    let lines = only(&described(&engine, view, server.port), &["console", "script_error"]);
    assert_eq!(
        lines,
        vec![
            "console Log [object Object] @inline#1:1:20",
            "console Log [object Object] @inline#1:2:80",
            "script_error : <object> script=inline#1 rejection=true",
            "console Info n seen by script 2 = 0 @inline#2:1:9",
        ]
    );
}

/// Error text with URLs in it (a quote, brackets, `=`, credentials) is not
/// kept at all: no scrubber has to find them.
#[test]
fn error_text_with_urls_in_it_is_recorded_as_its_length() {
    let page = "<html><body><script>
try { throw new Error(\"fetch failed 'https://u:p@h.test/a?token=SECRET1#f' (https://h.test/b?token=SECRET2) url=https://h.test/c?token=SECRET3.\"); } catch (e) { setTimeout(function () { throw e; }, 0); }
Promise.reject(new Error('reason https://u:p@h.test/d?token=SECRET4'));
Promise.reject(new Error('bad https://h.test/x?a[]=1&token=SECRET_BR'));
Promise.reject(new Error('paren https://h.test/y?a=(1)&token=SECRET_PA end'));
Promise.reject(new Error(\"quote https://h.test/z?a='1'&token=SECRET_Q\"));
Promise.reject(new Error('frag https://h.test/f#a(b)SECRET_FR.'));
</script><script type=\"module\" src=\"/m.js?a=(1)&token=SECRET_MODPAREN\"></script></body></html>";
    let (engine, view, _server) = load_page(
        EngineConfig::default(),
        vec![Route::html("/", page), Route::ok("/m.js", "text/javascript", "var a = 1;\nthrow new Error('in module');\n")],
    );
    let messages: Vec<String> = records(&engine, view)
        .into_iter()
        .filter_map(|r| match r.event {
            InspectorEvent::ScriptError { message, value, message_len, .. } => {
                Some(shown_message(&message, &value, message_len))
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        messages,
        vec![
            "<error len=41>",
            "<error len=42>",
            "<error len=48>",
            "<error len=43>",
            "<error len=36>",
            "<error len=9>",
            "<error len=120>",
        ]
    );
    let all = serde_json::to_string(&records(&engine, view)).unwrap();
    assert!(!all.contains("SECRET") && !all.contains("u:p@"), "{all}");
}

/// `responseXML` is null in every browser for a response that is not a
/// document: reading it there is not an unsupported API.
#[test]
fn reading_response_xml_of_a_non_document_is_not_unsupported() {
    let page = "<html><body><script>
var x = new XMLHttpRequest(); typeof x.responseXML; x.responseXML === null;
x.open('GET', '/data.json'); x.onload = function () { for (var i = 0; i < 5; i++) x.responseXML; };
x.send();
var y = new XMLHttpRequest(); y.open('GET', '/doc.xml'); y.onload = function () { y.responseXML; y.responseXML; }; y.send();
</script></body></html>";
    let (engine, view, server) = load_page(
        EngineConfig::default(),
        vec![
            Route::html("/", page),
            Route::ok("/data.json", "application/json", "{}"),
            Route::ok("/doc.xml", "application/xml", "<a/>"),
        ],
    );
    assert_eq!(
        only(&described(&engine, view, server.port), &["unsupported_api"]),
        vec!["unsupported_api XMLHttpRequest.responseXML @inline#1:5:100"]
    );
    let counts: Vec<u64> = engine.inspector(view).unwrap().unsupported_apis().iter().map(|u| u.count).collect();
    assert_eq!(counts, vec![2]);
}

// ---- Review R1 hold (#676 @ bf40c73c) ----

/// A thrown value that is not an error is described as console values are
/// (`[object Object]`), never with its properties, and `stack` holds only
/// frame lines.
#[test]
fn a_thrown_non_error_is_described_without_its_properties() {
    let page = "<html><body><script>throw {user:'a', token:'SECRET_OBJ'}</script>
<script>setTimeout(function () { throw {token: 'SECRET_TIMER'}; }, 0);</script>
<script type=\"module\">throw {token: 'SECRET_MOD'};</script></body></html>";
    let (engine, view, _server) = load_page(EngineConfig::default(), vec![Route::html("/", page)]);
    let records = records(&engine, view);
    let errors: Vec<(String, String, Option<String>)> = records
        .iter()
        .filter_map(|r| match &r.event {
            InspectorEvent::ScriptError { name, message, stack, value, message_len, .. } => {
                Some((name.clone(), shown_message(message, value, *message_len), stack.clone()))
            }
            _ => None,
        })
        .collect();
    assert_eq!(errors.len(), 3, "{errors:?}");
    for (name, message, stack) in &errors {
        assert_eq!((name.as_str(), message.as_str()), ("", "<object>"), "{errors:?}");
        for line in stack.as_deref().unwrap_or("").lines() {
            assert!(line.starts_with("    at inline#"), "not a frame line: {line:?}");
        }
    }
    let all = serde_json::to_string(&records).unwrap();
    assert!(!all.contains("SECRET") && !all.contains("user"), "{all}");
}

/// A `data:` URL inside error text is not kept: the text is its length.
#[test]
fn a_data_url_in_error_text_is_not_kept() {
    let page = "<html><body><script>
Promise.reject(new Error('load data:text/plain;base64,U0VDUkVU,SECRET_B64 failed'));
Promise.reject(new Error('metadata: kept'));
throw 'data:text/plain,SECRET_DATA';
</script></body></html>";
    let (engine, view, _server) = load_page(EngineConfig::default(), vec![Route::html("/", page)]);
    let messages: Vec<String> = records(&engine, view)
        .into_iter()
        .filter_map(|r| match r.event {
            InspectorEvent::ScriptError { message, value, message_len, .. } => {
                Some(shown_message(&message, &value, message_len))
            }
            _ => None,
        })
        .collect();
    assert_eq!(messages, vec!["<string len=27>", "<error len=54>", "<error len=14>"]);
    let all = serde_json::to_string(&records(&engine, view)).unwrap();
    assert!(!all.contains("SECRET") && !all.contains("U0VDUkVU"), "{all}");
}

/// Page script cannot write its own text into `unsupported_api` events or
/// the summary, nor grow the table: only the bindings' own members count.
#[test]
fn page_script_cannot_forge_unsupported_api_events() {
    let page = "<html><body><script>
document.cookie = 'sid=COOKIE_LOG';
if (window.__rustkit_unsupported) window.__rustkit_unsupported(document.cookie, 'FORGED');
for (var i = 0; i < 2000; i++) { try { window.__rustkit_unsupported('X' + i, 'm'); } catch (e) {} }
try { new XMLHttpRequest().open('GET', '/x', false); } catch (e) {}
</script></body></html>";
    let (engine, view, _server) = load_page(EngineConfig::default(), vec![Route::html("/", page)]);
    let records = records(&engine, view);
    let apis = engine.inspector(view).unwrap().unsupported_apis();
    let names: Vec<String> = apis.iter().map(|u| format!("{}.{}", u.interface, u.member)).collect();
    assert_eq!(names, vec!["XMLHttpRequest.open(async=false)"]);
    let summary = InspectorSummary::from_records(&records, &apis);
    assert_eq!(summary.unsupported_apis.keys().collect::<Vec<_>>(), vec!["XMLHttpRequest.open(async=false)"]);
    let all = serde_json::to_string(&records).unwrap();
    assert!(!all.contains("COOKIE_LOG") && !all.contains("FORGED"), "{all}");
}

/// A module's error after a top-level `await` is an uncaught error of that
/// module, as Chromium reports it, whether the `await` waited for a task
/// (a timer) or only a microtask; once each, never as a rejection.
#[test]
fn a_module_error_after_a_top_level_await_is_recorded_once() {
    let page = "<html><body><script type=\"module\">
await new Promise(function (r) { setTimeout(r, 5); });
console.log('m-after');
throw new Error('TLA_TIMER');
</script><script type=\"module\">
await Promise.resolve();
throw new Error('TLA_MICRO');
</script></body></html>";
    let (engine, view, _server) = load_page(EngineConfig::default(), vec![Route::html("/", page)]);
    let mut errors: Vec<(String, String, String, bool)> = records(&engine, view)
        .into_iter()
        .filter_map(|r| match r.event {
            InspectorEvent::ScriptError { name, message, script, unhandled_rejection, value, message_len, .. } => {
                Some((name, shown_message(&message, &value, message_len), script, unhandled_rejection))
            }
            _ => None,
        })
        .collect();
    errors.sort();
    assert_eq!(
        errors,
        vec![
            ("Error".to_string(), "<error len=9>".to_string(), "inline#1".to_string(), false),
            ("Error".to_string(), "<error len=9>".to_string(), "inline#2".to_string(), false),
        ]
    );
}

// ---- Review R1 hold, round 2 (#676 @ 56cbdfed) ----

/// A thrown or rejected value is recorded as its type, a standard error
/// name (any other name is `Error`), and its message's length, whoever
/// raised it.
#[test]
fn an_error_is_its_type_standard_name_and_length() {
    let page = "<html><body><script>
var e = new TypeError('page text'); e.name = 'SECRET_NAME'; throw e;
</script><script>throw new DOMException('m', 'NotFoundError');
</script><script>throw Symbol('SECRET_SYM');
</script><script>throw 42;
</script><script>undefinedFunctionSECRET();
</script><script>(void 0)();
</script></body></html>";
    let (engine, view, server) = load_page(EngineConfig::default(), vec![Route::html("/", page)]);
    assert_eq!(
        only(&described(&engine, view, server.port), &["script_error"]),
        vec![
            "script_error Error: <error len=9> @inline#1:2:9 script=inline#1 rejection=false",
            "script_error NotFoundError: <error len=1> @inline#2:1:7 script=inline#2 rejection=false",
            "script_error : <symbol> @inline#3:1:13 script=inline#3 rejection=false",
            "script_error : <number> @inline#4:1:7 script=inline#4 rejection=false",
            // Boa has no position for this one (`inline#5:?:?`): a frame it
            // cannot read is dropped.
            "script_error ReferenceError: <error len=38> script=inline#5 rejection=false",
            // Fourth R1 hold @ 47fc7abb: no message is kept by its text,
            // the engine's own words included.
            "script_error TypeError: <error len=23> @inline#6:1:9 script=inline#6 rejection=false",
        ]
    );
}

/// With `keep_error_text` the page's message is kept verbatim (capped),
/// cookies included: the opt-in is for a log that stays private.
#[test]
fn error_text_is_kept_verbatim_only_when_asked() {
    let page = "<html><body><script>
document.cookie = 'sid=COOKIEVALUE';
throw new Error('see ' + document.cookie + ' at https://h.test/?q=1');
</script></body></html>";
    let mut config = EngineConfig::default();
    config.inspector.keep_error_text = true;
    let (engine, view, server) = load_page(config, vec![Route::html("/", page)]);
    assert_eq!(
        only(&described(&engine, view, server.port), &["script_error"]),
        vec!["script_error Error: see sid=COOKIEVALUE at https://h.test/?q=1 @inline#1:3:7 script=inline#1 rejection=false"]
    );
}

/// Nothing page script can reach turns console redaction off, writes into
/// the engine's console or error queues, or changes what a record says:
/// not the engine's globals, not properties of `console`, not a prototype
/// or a global function the engine's own code calls (`push`, `apply`,
/// `JSON.stringify`, `String`, `toString`, `toJSON`, a `message` getter,
/// `Symbol.toPrimitive`). Genuine calls and errors are still recorded.
#[test]
fn page_script_cannot_switch_redaction_off_or_forge_records() {
    let page = r#"<html><body><script>
document.cookie = 'sid=COOKIEVALUE';
window.addEventListener('load', function () { throw new Error('genuine load'); });
setTimeout(function () { throw new TypeError('genuine timer'); }, 0);
</script><script>
try { console.__rustkit_capture(false); } catch (e) {}
console.log(document.cookie);
try { console._logs.push({level: 'error', msg: 'FORGED_LOGS ' + document.cookie}); } catch (e) {}
try { console._flush = function () { return [{level: 'error', msg: 'FORGED_FLUSH'}]; }; } catch (e) {}
try { __rustkit_console_format = function () { return 'FORGED_FORMAT'; }; } catch (e) {}
try { __rustkit_console_format_redacted = function () { return 'FORGED_FORMAT_R'; }; } catch (e) {}
</script><script>
try { window.__rustkit_errors.push('FORGED_ERRORS ' + document.cookie); } catch (e) {}
try { window.__rustkit_error_text = function () { return 'FORGED_ERROR_TEXT'; }; } catch (e) {}
try { window.__rustkit_errors_with_stacks(false); } catch (e) {}
try { window.__rustkit_take_unsupported = function () {
  return '[{"i":"XMLHttpRequest","m":"responseXML","n":7,"at":"https://x.test/FORGED_AT\u00011\u00011"}]';
}; } catch (e) {}
</script><script>
var realPush = Array.prototype.push, pushing = false;
Array.prototype.push = function () {
  if (!pushing) { pushing = true; try { realPush.call(this, 'FORGED_PUSH'); } catch (e) {} pushing = false; }
  return realPush.apply(this, arguments);
};
var realApply = Function.prototype.apply, realStringify = JSON.stringify, realString = String;
Function.prototype.apply = function () { return 'FORGED_APPLY'; };
JSON.stringify = function () { return '["FORGED_JSON"]'; };
String = function () { return 'FORGED_STRING'; };
Error.prototype.toString = function () { return 'FORGED_TOSTRING'; };
Error.prototype.toJSON = function () { return 'FORGED_TOJSON'; };
Object.defineProperty(Error.prototype, 'message', { get: function () { return 'FORGED_GETTER'; }, configurable: true });
Object.defineProperty(Error.prototype, 'name', { get: function () { return 'FORGED_NAME'; }, configurable: true });
Object.defineProperty(Object.prototype, 'msg', { get: function () { return 'FORGED_MSG'; }, configurable: true });
Object.defineProperty(Object.prototype, 'args', { get: function () { return ['FORGED_ARGS']; }, configurable: true });
Object.prototype[Symbol.toPrimitive] = function () { return 'FORGED_PRIMITIVE'; };
console.log('after patches', 1);
throw new RangeError('after patches');
</script><script>
// The bindings' own event dispatch (not the log) runs on builtins a page
// can patch: put them back so the genuine listener and timer still run.
Array.prototype.push = realPush; Function.prototype.apply = realApply;
JSON.stringify = realStringify; String = realString;
delete Error.prototype.toString; delete Error.prototype.toJSON;
delete Error.prototype.message; delete Error.prototype.name;
delete Object.prototype.msg; delete Object.prototype.args; delete Object.prototype[Symbol.toPrimitive];
try { new XMLHttpRequest().open('GET', '/x', false); } catch (e) {}
</script><script type=module>throw 'import map: ' + document.cookie;
</script><script type=module>throw 'JS engine panic';
</script><script type=module>await 0; throw 'import map: x';
</script></body></html>"#;
    let (engine, view, _server) = load_page(EngineConfig::default(), vec![Route::html("/", page)]);
    let records = records(&engine, view);
    let all = serde_json::to_string(&records).unwrap();
    assert!(!all.contains("FORGED") && !all.contains("COOKIEVALUE"), "{all}");
    let console: Vec<String> = records
        .iter()
        .filter_map(|r| match &r.event {
            InspectorEvent::Console { message, .. } => Some(message.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(console, vec!["<string len=15>", "<string len=13> <number>"], "{all}");
    let errors: Vec<(String, String)> = records
        .iter()
        .filter_map(|r| match &r.event {
            InspectorEvent::ScriptError { name, script, .. } => Some((name.clone(), script.clone())),
            _ => None,
        })
        .collect();
    for genuine in [("RangeError", "inline#4"), ("Error", "event:load"), ("TypeError", "timers")] {
        assert!(
            errors.iter().any(|(name, script)| (name.as_str(), script.as_str()) == genuine),
            "{genuine:?} missing: {errors:?}"
        );
    }
    // Third R1 hold @ 6bda3b2c: the modules' thrown strings are the page's
    // own `string` errors, never the engine's.
    let values: Vec<&str> = records
        .iter()
        .filter_map(|r| match &r.event {
            InspectorEvent::ScriptError { value, .. } => Some(value.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(values.iter().filter(|v| **v == "string").count(), 3, "{all}");
    assert!(!values.contains(&"engine"), "{all}");
    assert_eq!(errors.len(), 6, "{errors:?}");
    let apis: Vec<String> = engine
        .inspector(view)
        .unwrap()
        .unsupported_apis()
        .iter()
        .map(|u| format!("{}.{} x{}", u.interface, u.member, u.count))
        .collect();
    assert_eq!(apis, vec!["XMLHttpRequest.open(async=false) x1"]);
}

/// The engine's own failures (a panic inside the JS engine, an import map
/// it refused) are recorded as `value: "engine"` where the engine raises
/// them, never because of what a failure's text says: a page throwing the
/// engine's very words, from anywhere, is a page error (third R1 hold @
/// 6bda3b2c).
#[test]
fn a_page_throwing_an_engine_message_is_never_an_engine_record() {
    let mut page = String::from("<html><body><script>document.cookie = 'sid=COOKIEVALUE';</script>");
    let mut throws = 0;
    for message in ["JS engine panic", "import map: x", "import map: ' + document.cookie + '"] {
        for (kind, body) in [
            ("", format!("throw '{message}';")),
            ("type=module", format!("throw '{message}';")),
            ("type=module", format!("await 0; throw '{message}';")),
            ("", format!("setTimeout(function () {{ throw '{message}'; }}, 0);")),
            ("", format!("window.addEventListener('load', function () {{ throw '{message}'; }});")),
            ("", format!("Promise.reject('{message}');")),
            ("", format!("(async function () {{ throw '{message}'; }})();")),
            ("", format!("(async function () {{ await 0; throw '{message}'; }})();")),
        ] {
            page.push_str(&format!("<script {kind}>{body}</script>"));
            throws += 1;
        }
    }
    page.push_str("</body></html>");
    let (mut engine, view, _server) = load_page(EngineConfig::default(), vec![Route::html("/", page)]);
    // And on the live loop, after the load.
    engine
        .execute_script(view, "setTimeout(function () { throw 'JS engine panic'; }, 0); setTimeout(function () { throw 'import map: x'; }, 0);")
        .unwrap();
    runtime().block_on(engine.pump_live(view, 10));
    throws += 2;
    let records = records(&engine, view);
    let all = serde_json::to_string(&records).unwrap();
    let values: Vec<&str> = records
        .iter()
        .filter_map(|r| match &r.event {
            InspectorEvent::ScriptError { value, .. } => Some(value.as_str()),
            _ => None,
        })
        .collect();
    assert!(!values.contains(&"engine"), "{all}");
    assert!(!all.contains("COOKIEVALUE") && !all.contains("JS engine panic") && !all.contains("import map"), "{all}");
    assert_eq!(values, vec!["string"; throws], "{all}");
    let summary = InspectorSummary::from_records(&records, &[]);
    assert_eq!(summary.script_errors as usize, throws, "{summary:?}");
}

/// Fourth R1 hold @ 47fc7abb, rule 4: no message is kept because of what
/// it says. Every message the engine used to keep by its text, and the
/// engine's own failure messages, thrown by the page (exactly, and with a
/// secret appended; as an error and as a string) from a classic script, a
/// module, a timer, a listener, a promise rejection and an async function,
/// is a page record with no text.
#[test]
fn a_page_throwing_any_engine_message_is_a_page_record_with_no_text() {
    let messages = [
        "not a callable function",
        "not a constructor",
        "cannot convert 'null' or 'undefined' to object",
        "value returned for property of object is not a function",
        "right-hand side of 'instanceof' is not callable",
        "cannot mix BigInt and other types, use explicit conversions",
        "BigInt division by zero",
        "cyclic object value",
        "cannot convert Symbol to JSON",
        "Failed to fetch",
        "Maximum call stack size exceeded",
        "Illegal invocation",
        "Failed to execute 'parseFromString' on 'DOMParser': XML documents are not supported yet.",
        "PAGEIDENT is not defined",
        "PAGEIDENT is not a function",
        "PAGEIDENT is not a constructor",
        "JS engine panic",
        "import map rejected",
        "import map: x",
    ];
    // (text, thrown as an error object)
    let mut thrown: Vec<(String, bool)> = Vec::new();
    for message in messages {
        for text in [message.to_string(), format!("{message} SECRETAPPENDED")] {
            thrown.push((text.clone(), true));
            thrown.push((text, false));
        }
    }
    let json = serde_json::to_string(&thrown.iter().map(|(t, _)| t).collect::<Vec<_>>()).unwrap();
    let mut page = format!(
        "<html><body><script>window.M = {json}; window.T = function (i) {{ \
         return i % 2 === 0 ? new TypeError(M[i]) : M[i]; }};</script>"
    );
    for i in 0..thrown.len() {
        page.push_str(&format!("<script>throw T({i});</script>"));
        page.push_str(&format!("<script type=module>throw T({i});</script>"));
    }
    page.push_str(
        "<script>for (var i = 0; i < M.length; i++) (function (i) { \
         setTimeout(function () { throw T(i); }, 0); \
         window.addEventListener('load', function () { throw T(i); }); \
         Promise.reject(T(i)); \
         (async function () { await 0; throw T(i); })(); })(i);</script></body></html>",
    );
    // 456 errors plus a script record per script outgrow the tab ring: keep
    // the full buffer.
    let (engine, view, _server) = load_page(ring_off(), vec![Route::html("/", page)]);
    let records = records(&engine, view);
    let all = serde_json::to_string(&records).unwrap();
    let errors: Vec<(&str, &str, Option<&String>)> = records
        .iter()
        .filter_map(|r| match &r.event {
            InspectorEvent::ScriptError { value, name, message, .. } => Some((value.as_str(), name.as_str(), message.as_ref())),
            _ => None,
        })
        .collect();
    assert_eq!(errors.len(), thrown.len() * 6, "every throw is recorded: {all}");
    for (value, name, message) in &errors {
        assert!(matches!((*value, *name), ("error", "TypeError") | ("string", "")), "{value} {name}: {all}");
        assert_eq!(*message, None, "{all}");
    }
    for (value, _, _) in &errors {
        assert_ne!(*value, "engine", "{all}");
    }
    assert!(!all.contains("SECRETAPPENDED") && !all.contains("PAGEIDENT"), "{all}");
    for message in messages {
        let fixed = message.trim_start_matches("PAGEIDENT");
        assert!(!all.contains(fixed), "{fixed:?} kept: {all}");
    }
}

/// What the engine itself refused is still an engine record: an import map
/// that is not JSON.
#[test]
fn an_import_map_the_engine_refused_is_an_engine_record() {
    let page = "<html><body><script type=importmap>{ not json</script><script type=module>window.x = 1;</script></body></html>";
    let (engine, view, server) = load_page(EngineConfig::default(), vec![Route::html("/", page)]);
    let errors = only(&described(&engine, view, server.port), &["script_error"]);
    assert_eq!(errors, vec!["script_error : import map rejected script=importmap rejection=false"]);
    let engine_values = records(&engine, view)
        .iter()
        .filter(|r| matches!(&r.event, InspectorEvent::ScriptError { value, .. } if value == "engine"))
        .count();
    assert_eq!(engine_values, 1);
}

/// A module whose URL ends in `#inline-module-n` is not the page's n-th
/// inline script: only the engine names a script `inline#n` (third R1 hold
/// @ 6bda3b2c, audit).
#[test]
fn a_module_url_cannot_pass_for_an_inline_script() {
    let page = "<html><body><script type=module src=\"/main.js\"></script>\
<script type=module src=\"/late.js#inline-module-2\"></script></body></html>";
    let routes = vec![
        Route::html("/", page),
        Route::ok("/main.js", "text/javascript", "import './evil.js#inline-module-1';"),
        Route::ok("/evil.js", "text/javascript", "export const x = 1;\nthrow new Error('evil');"),
        Route::ok("/late.js", "text/javascript", "await 0;\nthrow new Error('late');"),
    ];
    let (engine, view, _server) = load_page(EngineConfig::default(), routes);
    let records = records(&engine, view);
    let all = serde_json::to_string(&records).unwrap();
    assert_eq!(records.iter().filter(|r| r.event.kind() == "script_error").count(), 2, "{all}");
    assert!(!all.contains("inline#") && !all.contains("inline-module"), "{all}");
    assert!(all.contains("/evil.js:2:") && all.contains("/late.js"), "{all}");
}

/// The first script error's stack and inserter, ports dropped.
fn first_error_stack(engine: &Engine, view: EngineViewId, port: u16) -> (Option<String>, Option<String>) {
    let local = |s: &str| s.replace(&format!("http://127.0.0.1:{port}"), "");
    records(engine, view)
        .into_iter()
        .find_map(|r| match r.event {
            InspectorEvent::ScriptError { stack, inserted_by, .. } => Some((
                stack.map(|s| local(&s)),
                inserted_by.map(|at| format!("{}:{}:{}", local(&at.url), at.line, at.column)),
            )),
            _ => None,
        })
        .expect("a script error")
}

const DEEP_PAGE: &str = "<html><body><script>
var s = document.createElement('script');
s.src = '/deep.js'; document.body.appendChild(s);
</script></body></html>";
const DEEP_JS: &str = "function outer() {\n  middle();\n}\nfunction middle() {\n  inner();\n}\nfunction inner() {\n  null.deep;\n}\nouter();\n";

/// Dev-tools T1: a throw three frames deep in a script that page script
/// inserted shows every frame at its URL, line and column (calls placed at
/// their `(`, as Boa places them), each named function's name when error
/// text is kept, and where the script was inserted.
#[test]
fn a_deep_throw_shows_every_frame_and_where_its_script_was_inserted() {
    let mut config = EngineConfig::default();
    config.inspector.keep_error_text = true;
    let (engine, view, server) =
        load_page(config, vec![Route::html("/", DEEP_PAGE), Route::ok("/deep.js", "text/javascript", DEEP_JS)]);
    let (stack, inserted_by) = first_error_stack(&engine, view, server.port);
    assert_eq!(
        stack.as_deref(),
        Some("    at inner (/deep.js:8:8)\n    at middle (/deep.js:5:8)\n    at outer (/deep.js:2:9)\n    at /deep.js:10:6")
    );
    assert_eq!(inserted_by.as_deref(), Some("inline#1:3:46"));
}

/// Without error text a function's name (the page's text) is not kept:
/// positions and the inserter only.
#[test]
fn a_deep_throw_keeps_positions_only_without_error_text() {
    let (engine, view, server) = load_page(
        EngineConfig::default(),
        vec![Route::html("/", DEEP_PAGE), Route::ok("/deep.js", "text/javascript", DEEP_JS)],
    );
    let (stack, inserted_by) = first_error_stack(&engine, view, server.port);
    assert_eq!(
        stack.as_deref(),
        Some("    at /deep.js:8:8\n    at /deep.js:5:8\n    at /deep.js:2:9\n    at /deep.js:10:6")
    );
    assert_eq!(inserted_by.as_deref(), Some("inline#1:3:46"));
}

/// Dev-tools T1: `console.error` carries its caller's stack; other levels
/// do not.
#[test]
fn console_error_carries_its_callers_stack() {
    let page = "<html><body><script>
function a() { b(); }
function b() { console.error('x'); console.log('y'); }
a();
</script></body></html>";
    let stacks = |config: EngineConfig| {
        let (engine, view, _server) = load_page(config, vec![Route::html("/", page)]);
        records(&engine, view)
            .into_iter()
            .filter_map(|r| match r.event {
                InspectorEvent::Console { level, stack, .. } => Some((level, stack)),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    let mut config = console_text();
    config.inspector.keep_error_text = true;
    assert_eq!(
        stacks(config),
        vec![
            (ConsoleLevel::Error, Some("    at b (inline#1:3:29)\n    at a (inline#1:2:17)\n    at inline#1:4:2".to_string())),
            (ConsoleLevel::Log, None),
        ]
    );
    assert_eq!(
        stacks(EngineConfig::default()),
        vec![
            (ConsoleLevel::Error, Some("    at inline#1:3:29\n    at inline#1:2:17\n    at inline#1:4:2".to_string())),
            (ConsoleLevel::Log, None),
        ]
    );
}

/// Dev-tools T1 (#777's shape): a framework catches an error and hands it
/// to `console.error`. The record carries the error's own frames (where it
/// was thrown) as well as where `console.error` was called.
#[test]
fn a_caught_error_logged_with_console_error_keeps_where_it_was_thrown() {
    let page = "<html><body><script>
function three() { return Object.keys(null); }
function two() { return three(); }
function one() { return two(); }
try { one(); } catch (e) { console.error('boundary', e); }
console.error('no error argument');
</script></body></html>";
    let mut config = console_text();
    config.inspector.keep_error_text = true;
    let (engine, view, _server) = load_page(config, vec![Route::html("/", page)]);
    let logged: Vec<(Option<String>, Option<String>)> = records(&engine, view)
        .into_iter()
        .filter_map(|r| match r.event {
            InspectorEvent::Console { stack, error_stack, .. } => Some((stack, error_stack)),
            _ => None,
        })
        .collect();
    assert_eq!(
        logged,
        vec![
            (
                Some("    at inline#1:5:41".to_string()),
                Some(
                    "    at three (inline#1:2:38)\n    at two (inline#1:3:30)\n    at one (inline#1:4:28)\n    at inline#1:5:10"
                        .to_string()
                ),
            ),
            (Some("    at inline#1:6:14".to_string()), None),
        ]
    );
}

/// #784 R1 nit 1: a computed function name cannot forge a frame line in
/// the stack text; such a frame keeps its position, without a name.
#[test]
fn a_computed_function_name_cannot_forge_a_frame() {
    let page = "<html><body><script>
var o = { ['x (https://forged.test/f.js:9:9)\\n    at forged']() { null.deep; } };
o['x (https://forged.test/f.js:9:9)\\n    at forged']();
</script></body></html>";
    let mut config = EngineConfig::default();
    config.inspector.keep_error_text = true;
    let (engine, view, server) = load_page(config, vec![Route::html("/", page)]);
    let (stack, _) = first_error_stack(&engine, view, server.port);
    let stack = stack.expect("a stack");
    assert!(!stack.contains("forged"), "{stack}");
    assert_eq!(stack.lines().count(), 2, "{stack}");
}

/// #784 R1 nit 2: a throw in `eval` code is not located in the script
/// that called `eval`, so it does not take that script's inserter either.
#[test]
fn an_eval_throw_does_not_borrow_its_scripts_inserter() {
    let page = "<html><body><script>
var s = document.createElement('script');
s.src = '/e.js'; document.body.appendChild(s);
</script></body></html>";
    let (engine, view, server) = load_page(
        EngineConfig::default(),
        vec![Route::html("/", page), Route::ok("/e.js", "text/javascript", "eval('null.deep');\n")],
    );
    let (_, inserted_by) = first_error_stack(&engine, view, server.port);
    assert_eq!(inserted_by, None);
}

/// #784 R1 must-fix: the console mirror (`HIWAVE_CONSOLE_LOG`) keeps the
/// runtime's text for the tracing log, but what reaches the Inspector
/// follows the Inspector's own `keep_error_text`: with it off, a script
/// error has no message and every stack (`script_error.stack`,
/// `console.stack`, `console.error_stack`) is positions only.
#[test]
fn the_console_mirror_does_not_put_error_text_in_the_inspector() {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            crate::console_log::force_for_test(None);
        }
    }
    let _reset = Reset;
    crate::console_log::force_for_test(Some(true));
    let page = "<html><body><script>
function three() { return Object.keys(null); }
function two() { return three(); }
try { two(); } catch (e) { console.error('boundary', e); }
</script><script>
function outer() { inner(); }
function inner() { throw new Error('secret-message'); }
outer();
</script></body></html>";
    let (engine, view, _server) = load_page(EngineConfig::default(), vec![Route::html("/", page)]);
    let records = records(&engine, view);
    let all = serde_json::to_string(&records).unwrap();
    assert!(!all.contains("secret-message"), "{all}");
    for name in ["three", "two", "outer", "inner"] {
        assert!(!all.contains(&format!("at {name} ")), "{name}: {all}");
    }
    let logged: Vec<_> = records
        .iter()
        .filter_map(|r| match &r.event {
            InspectorEvent::Console { stack, error_stack, .. } => Some((stack.clone(), error_stack.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(
        logged,
        vec![(
            Some("    at inline#1:4:41".to_string()),
            Some("    at inline#1:2:38\n    at inline#1:3:30\n    at inline#1:4:10".to_string()),
        )]
    );
    let errors: Vec<_> = records
        .iter()
        .filter_map(|r| match &r.event {
            InspectorEvent::ScriptError { message, stack, .. } => Some((message.clone(), stack.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(
        errors,
        vec![(None, Some("    at inline#2:3:26\n    at inline#2:2:25\n    at inline#2:4:6".to_string()))]
    );
}
