//! The script-network pump against a real loopback server and the real
//! policy: no engine, no GPU. The page is `http://127.0.0.1:PORT/`, a
//! private origin, so its own origin is reachable and nothing else private is.

use crate::script_net::{pump, Pump};
use rustkit_bindings::DomBindings;
use rustkit_js::{JsRuntime, JsValue};
use rustkit_net::policy::FetchPolicy;
use rustkit_net::{LoaderConfig, ResourceLoader};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use url::Url;

pub(crate) struct Server {
    pub(crate) port: u16,
    pub(crate) hits: Arc<AtomicUsize>,
}

/// `/echo` answers with the request body, `/slow*` after 300 ms, anything
/// else with its own path as the body.
fn serve() -> Server {
    serve_routes(Vec::new())
}

/// `serve`, plus fixed `routes` (path, content type, body) that take
/// precedence.
pub(crate) fn serve_routes(routes: Vec<(&'static str, &'static str, String)>) -> Server {
    let routes = Arc::new(routes);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let hits = Arc::new(AtomicUsize::new(0));
    let counted = hits.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let counted = counted.clone();
            let routes = routes.clone();
            std::thread::spawn(move || {
                let mut request = Vec::new();
                let mut buf = [0u8; 2048];
                let header_end = loop {
                    if let Some(i) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                        break i + 4;
                    }
                    match stream.read(&mut buf) {
                        Ok(0) | Err(_) => return,
                        Ok(n) => request.extend_from_slice(&buf[..n]),
                    }
                };
                counted.fetch_add(1, Ordering::SeqCst);
                let head = String::from_utf8_lossy(&request[..header_end]).to_string();
                let path = head.split_whitespace().nth(1).unwrap_or("/").to_string();
                let length = head
                    .lines()
                    .find_map(|l| {
                        l.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .map(|v| v.trim().parse::<usize>().unwrap_or(0))
                    })
                    .unwrap_or(0);
                while request.len() < header_end + length {
                    match stream.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => request.extend_from_slice(&buf[..n]),
                    }
                }
                let fixed = routes.iter().find(|(p, _, _)| *p == path);
                let content_type = fixed.map(|(_, ct, _)| *ct).unwrap_or("text/plain");
                let body = if let Some((_, _, body)) = fixed {
                    body.clone().into_bytes()
                } else if path == "/echo" {
                    request[header_end..].to_vec()
                } else {
                    if path.starts_with("/slow") {
                        std::thread::sleep(Duration::from_millis(300));
                    }
                    path.clone().into_bytes()
                };
                // `/missing...` is a 404, for the failure cases.
                let status = if fixed.is_none() && path.starts_with("/missing") { "404 Not Found" } else { "200 OK" };
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(&body);
            });
        }
    });
    Server { port, hits }
}

struct Rig {
    bindings: DomBindings,
    policy: FetchPolicy,
    loader: ResourceLoader,
    rt: tokio::runtime::Runtime,
    port: u16,
    hits: Arc<AtomicUsize>,
}

fn rig() -> Rig {
    let server = serve();
    let bindings = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    bindings.enable_net_bridge().unwrap();
    Rig {
        policy: FetchPolicy::for_page(
            Url::parse(&format!("http://127.0.0.1:{}/", server.port)).unwrap(),
            None,
        ),
        loader: ResourceLoader::new(LoaderConfig::default()).unwrap(),
        rt: tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap(),
        bindings,
        port: server.port,
        hits: server.hits,
    }
}

impl Rig {
    fn js(&self, script: &str) -> String {
        match self.bindings.evaluate(script).unwrap() {
            JsValue::String(s) => s,
            JsValue::Number(n) => format!("{n}"),
            JsValue::Boolean(b) => b.to_string(),
            other => format!("{other:?}"),
        }
    }

    fn pump(&self, rounds: u32, within: Duration) -> Pump {
        self.rt.block_on(pump(
            &self.bindings,
            &self.policy,
            &self.loader,
            tokio::time::Instant::now() + within,
            rounds,
            None,
        ))
    }

    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.port)
    }
}

#[test]
fn a_same_origin_request_is_fetched_under_the_policy_and_delivered() {
    let rig = rig();
    rig.js(&format!(
        "var got; window.__rustkit_net.request({{ url: '{}', destination: 'xhr' }}, function (r) {{ got = r; }});",
        rig.url("/hello")
    ));
    let found = rig.pump(8, Duration::from_secs(10));
    assert_eq!((found.requests, found.rounds, found.poisoned), (1, 1, false));
    assert_eq!(
        rig.js("got.ok + ':' + got.status + ':' + got.kind + ':' + got.body_b64"),
        "true:200:basic:L2hlbGxv"
    );
    assert_eq!(rig.bindings.pending_net_requests(), 0);
}

#[test]
fn a_request_body_survives_both_directions_as_bytes() {
    let rig = rig();
    // "hi", a NUL and a non-UTF-8 byte.
    rig.js(&format!(
        "var got; window.__rustkit_net.request({{ method: 'POST', url: '{}', body_b64: 'aGkA/w==' }}, function (r) {{ got = r; }});",
        rig.url("/echo")
    ));
    rig.pump(8, Duration::from_secs(10));
    assert_eq!(rig.js("got.ok + ':' + got.body_b64"), "true:aGkA/w==");
}

/// The page never learns why: every refusal is the same error, and a
/// refused request never reaches the server.
#[test]
fn a_denied_request_is_a_generic_network_error_and_never_connects() {
    let rig = rig();
    // Another private address than the page's own: refused before connect.
    rig.js(&format!(
        "var log = []; \
         window.__rustkit_net.request({{ url: 'http://127.0.0.2:{}/secret' }}, function (r) {{ log.push(r.ok + ':' + r.error); }}); \
         window.__rustkit_net.request({{ url: 'http://10.0.0.1:9/' }}, function (r) {{ log.push(r.ok + ':' + r.error); }}); \
         window.__rustkit_net.request({{ url: 'ftp://127.0.0.1/' }}, function (r) {{ log.push(r.ok + ':' + r.error); }}); \
         window.__rustkit_net.request({{ method: 'NOT A METHOD', url: '{}' }}, function (r) {{ log.push(r.ok + ':' + r.error); }});",
        rig.port,
        rig.url("/x")
    ));
    let found = rig.pump(8, Duration::from_secs(10));
    assert_eq!(found.requests, 4);
    assert_eq!(
        rig.js("log.join('|')"),
        "false:network error|false:network error|false:network error|false:network error"
    );
    assert_eq!(rig.hits.load(Ordering::SeqCst), 0, "nothing reached the server");
}

/// Fetched concurrently, delivered in the order the page asked.
#[test]
fn deliveries_follow_request_order_not_completion_order() {
    let rig = rig();
    rig.js(&format!(
        "var order = []; \
         window.__rustkit_net.request({{ url: '{}' }}, function () {{ order.push('slow'); }}); \
         window.__rustkit_net.request({{ url: '{}' }}, function () {{ order.push('fast1'); }}); \
         window.__rustkit_net.request({{ url: '{}' }}, function () {{ order.push('fast2'); }});",
        rig.url("/slow"),
        rig.url("/a"),
        rig.url("/b")
    ));
    rig.pump(8, Duration::from_secs(10));
    assert_eq!(rig.js("order.join()"), "slow,fast1,fast2");
}

const CHAIN: &str = "var path = []; \
     function next(n) {{ \
         window.__rustkit_net.request({{ url: '{base}/step' + n }}, function (r) {{ \
             path.push(r.ok ? 'ok' + n : 'err' + n); \
             if (n < {last}) next(n + 1); \
         }}); \
     }} \
     next(1);";

fn chain(rig: &Rig, last: u32) {
    rig.js(
        &CHAIN
            .replace("{{", "{")
            .replace("}}", "}")
            .replace("{base}", &rig.url(""))
            .replace("{last}", &last.to_string()),
    );
}

/// A response handler that asks for more: a chain of three finishes in
/// three rounds.
#[test]
fn a_response_handler_can_start_the_next_request() {
    let rig = rig();
    chain(&rig, 3);
    let found = rig.pump(8, Duration::from_secs(10));
    assert_eq!((found.requests, found.rounds), (3, 3));
    assert_eq!(rig.js("path.join()"), "ok1,ok2,ok3");
}

/// Out of rounds: what is still waiting completes with an error, and
/// nothing is left waiting forever.
#[test]
fn what_outlasts_the_rounds_completes_with_an_error() {
    let rig = rig();
    chain(&rig, 5);
    let found = rig.pump(2, Duration::from_secs(10));
    assert_eq!((found.requests, found.rounds), (2, 2));
    // Rounds 1 and 2 delivered; the request round 2's handler made was
    // failed instead of fetched, and so were the retries its failure made.
    assert_eq!(rig.js("path.join()"), "ok1,ok2,err3,err4,err5");
    assert_eq!(rig.bindings.pending_net_requests(), 0);
    assert_eq!(rig.hits.load(Ordering::SeqCst), 2, "only the two rounds' requests were sent");
}

/// A page that retries on every failure cannot keep the engine here: the
/// failure passes are bounded and the rest is dropped.
#[test]
fn a_retry_loop_on_failure_cannot_hold_the_pump() {
    let rig = rig();
    rig.js(&format!(
        "var failures = 0;          function again() {{              window.__rustkit_net.request({{ url: '{}' }}, function (r) {{ if (!r.ok) {{ failures++; again(); }} }});          }}          again();",
        rig.url("/never")
    ));
    let found = rig.pump(0, Duration::from_secs(10));
    assert_eq!(found.requests, 0);
    assert_eq!(rig.js("failures"), "3", "three passes, then dropped");
    assert_eq!(rig.bindings.pending_net_requests(), 0);
    assert_eq!(rig.bindings.take_net_requests().len(), 0);
    assert_eq!(rig.hits.load(Ordering::SeqCst), 0);
}

/// No budget left: nothing is fetched, everything fails.
#[test]
fn with_no_budget_nothing_is_fetched_and_everything_fails() {
    let rig = rig();
    rig.js(&format!(
        "var log = []; window.__rustkit_net.request({{ url: '{}' }}, function (r) {{ log.push(r.ok + ':' + r.error); }});",
        rig.url("/late")
    ));
    let found = rig.pump(8, Duration::ZERO);
    assert_eq!(found.requests, 0);
    assert_eq!(rig.js("log.join()"), "false:network error");
    assert_eq!(rig.hits.load(Ordering::SeqCst), 0);
}

/// Without the bridge there is nothing to pump.
#[test]
fn a_page_without_the_bridge_has_no_requests() {
    let rig = rig();
    let bare = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    let found = rig.rt.block_on(pump(
        &bare,
        &rig.policy,
        &rig.loader,
        tokio::time::Instant::now() + Duration::from_secs(1),
        8,
        None,
    ));
    assert_eq!((found.requests, found.rounds, found.poisoned), (0, 0, false));
}
