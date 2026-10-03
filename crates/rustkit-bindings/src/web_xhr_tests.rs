//! XMLHttpRequest (web_xhr.js), built on the script-network bridge. The
//! engine's side is played by the test: it takes the queued request, checks
//! what the page asked for, and delivers an outcome.

use super::*;
use crate::net_bridge::{NetDelivery, NetRequest};

fn bindings() -> DomBindings {
    let b = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    b.set_location(&url::Url::parse("http://site.test/dir/page.html").unwrap()).unwrap();
    b.enable_net_bridge().unwrap();
    b
}

fn ev(b: &DomBindings, script: &str) -> String {
    match b.evaluate(script).unwrap() {
        JsValue::String(s) => s,
        JsValue::Boolean(x) => x.to_string(),
        JsValue::Number(n) => {
            if n.fract() == 0.0 { format!("{}", n as i64) } else { n.to_string() }
        }
        JsValue::Undefined => "undefined".into(),
        JsValue::Null => "null".into(),
        other => format!("{other:?}"),
    }
}

fn b64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = (chunk[0] as u32) << 16 | (*chunk.get(1).unwrap_or(&0) as u32) << 8 | *chunk.get(2).unwrap_or(&0) as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

fn unb64(s: &str) -> Vec<u8> {
    let mut out = Vec::new();
    let mut acc = 0u32;
    let mut bits = 0;
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => continue,
        } as u32;
        acc = acc << 6 | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    out
}

fn ok(b: &DomBindings, id: u64, status: u16, headers: &[(&str, &str)], body: &[u8]) {
    b.deliver_net_response(
        id,
        NetDelivery::Response {
            url: "http://site.test/dir/data".into(),
            status,
            status_text: "OK".into(),
            headers: headers.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect(),
            body_b64: b64(body),
            kind: "basic".into(),
            redirected: false,
        },
    )
    .unwrap();
}

fn one_request(b: &DomBindings) -> NetRequest {
    let mut taken = b.take_net_requests();
    assert_eq!(taken.len(), 1, "{taken:?}");
    taken.remove(0)
}

#[test]
fn without_the_bridge_there_is_no_xmlhttprequest() {
    let b = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    assert_eq!(ev(&b, "typeof XMLHttpRequest"), "undefined");
    assert_eq!(ev(&b, "typeof XMLHttpRequestUpload"), "undefined");
}

#[test]
fn a_get_goes_through_the_states_and_events_in_order() {
    let b = bindings();
    b.evaluate(
        "var log = []; var x = new XMLHttpRequest(); \
         ['readystatechange','loadstart','progress','load','error','abort','timeout','loadend'].forEach(function (t) { \
             x.addEventListener(t, function (e) { log.push(t === 'readystatechange' ? 'rsc' + x.readyState : t); }); }); \
         log.push('state' + x.readyState); \
         x.open('get', 'data?q=1#frag'); \
         log.push('opened' + x.readyState); \
         x.send();",
    )
    .unwrap();
    let req = one_request(&b);
    assert_eq!(req.method, "GET", "the method is normalised");
    assert_eq!(req.url, "http://site.test/dir/data?q=1", "resolved against the page, fragment dropped");
    assert_eq!((req.mode.as_str(), req.credentials.as_str(), req.redirect.as_str(), req.destination.as_str()), ("cors", "same-origin", "follow", "xhr"));
    assert_eq!(req.body_b64, None);
    ok(&b, req.id, 200, &[("Content-Type", "text/plain"), ("X-Thing", "a"), ("x-thing", "b")], b"hello");
    assert_eq!(
        ev(&b, "log.join()"),
        "state0,rsc1,opened1,loadstart,rsc2,rsc3,progress,rsc4,load,loadend"
    );
    assert_eq!(ev(&b, "[x.readyState, x.status, x.statusText, x.responseText, x.response, x.responseURL].join('|')"), "4|200|OK|hello|hello|http://site.test/dir/data");
    assert_eq!(ev(&b, "x.getResponseHeader('CONTENT-type')"), "text/plain");
    assert_eq!(ev(&b, "x.getResponseHeader('x-thing')"), "a, b");
    assert_eq!(ev(&b, "x.getResponseHeader('nope')"), "null");
    assert_eq!(ev(&b, "x.getAllResponseHeaders()"), "content-type: text/plain\r\nx-thing: a, b\r\n");
}

#[test]
fn bodies_are_sent_as_bytes_with_the_right_default_content_type() {
    let b = bindings();
    b.evaluate(
        "var x1 = new XMLHttpRequest(); x1.open('POST', '/a'); x1.send('h\u{e9}llo'); \
         var x2 = new XMLHttpRequest(); x2.open('POST', '/b'); x2.setRequestHeader('content-type', 'application/json'); x2.send('{}'); \
         var x3 = new XMLHttpRequest(); x3.open('POST', '/c'); x3.send(new URLSearchParams({ a: '1 2', b: '&' })); \
         var x4 = new XMLHttpRequest(); x4.open('PUT', '/d'); x4.send(new Uint8Array([0, 255, 7]).buffer); \
         var x5 = new XMLHttpRequest(); x5.open('POST', '/e'); x5.send(new Blob(['blob!'], { type: 'text/x-blob' })); \
         var x6 = new XMLHttpRequest(); x6.open('GET', '/f'); x6.send('ignored on GET');",
    )
    .unwrap();
    let mut by_path = std::collections::HashMap::new();
    for r in b.take_net_requests() {
        by_path.insert(r.url.rsplit('/').next().unwrap().to_string(), r);
    }
    let header = |r: &NetRequest| r.headers.iter().find(|(n, _)| n.eq_ignore_ascii_case("content-type")).map(|(_, v)| v.clone());
    let body = |r: &NetRequest| unb64(r.body_b64.as_deref().unwrap_or(""));
    assert_eq!(body(&by_path["a"]), "h\u{e9}llo".as_bytes());
    assert_eq!(header(&by_path["a"]).as_deref(), Some("text/plain;charset=UTF-8"));
    assert_eq!(header(&by_path["b"]).as_deref(), Some("application/json"), "the author's type wins");
    assert_eq!(body(&by_path["c"]), b"a=1+2&b=%26");
    assert_eq!(header(&by_path["c"]).as_deref(), Some("application/x-www-form-urlencoded;charset=UTF-8"));
    assert_eq!(body(&by_path["d"]), vec![0, 255, 7]);
    assert_eq!(header(&by_path["d"]), None, "a typed array has no default type");
    assert_eq!(body(&by_path["e"]), b"blob!");
    assert_eq!(header(&by_path["e"]).as_deref(), Some("text/x-blob"));
    assert_eq!(by_path["f"].body_b64, None, "a GET carries no body");
}

#[test]
fn form_data_becomes_multipart() {
    let b = bindings();
    b.evaluate(
        "var fd = new FormData(); fd.append('name', 'Ada'); fd.append('file', new Blob(['xyz'], { type: 'text/plain' }), 'a.txt'); \
         var x = new XMLHttpRequest(); x.open('POST', '/up'); x.send(fd);",
    )
    .unwrap();
    let req = one_request(&b);
    let ct = req.headers.iter().find(|(n, _)| n == "Content-Type").map(|(_, v)| v.clone()).unwrap();
    assert!(ct.starts_with("multipart/form-data; boundary="), "{ct}");
    let boundary = ct.split("boundary=").nth(1).unwrap();
    let body = String::from_utf8(unb64(req.body_b64.as_deref().unwrap())).unwrap();
    assert!(body.contains(&format!("--{boundary}\r\nContent-Disposition: form-data; name=\"name\"\r\n\r\nAda\r\n")), "{body}");
    assert!(body.contains("name=\"file\"; filename=\"a.txt\"\r\nContent-Type: text/plain\r\n\r\nxyz\r\n"), "{body}");
    assert!(body.ends_with(&format!("--{boundary}--\r\n")), "{body}");
}

#[test]
fn request_headers_are_combined_and_the_forbidden_ones_dropped() {
    let b = bindings();
    b.evaluate(
        "var x = new XMLHttpRequest(); x.open('GET', '/h'); \
         x.setRequestHeader('X-A', '1'); x.setRequestHeader('x-a', '2'); \
         ['Cookie', 'Host', 'Origin', 'Referer', 'Sec-Fetch-Mode', 'Proxy-Authorization', 'Content-Length', 'Accept-Encoding'] \
             .forEach(function (n) { x.setRequestHeader(n, 'evil'); }); \
         x.setRequestHeader('Accept', 'text/x'); x.send();",
    )
    .unwrap();
    let req = one_request(&b);
    assert_eq!(req.headers, vec![("X-A".to_string(), "1, 2".to_string()), ("Accept".to_string(), "text/x".to_string())]);
}

#[test]
fn the_state_rules_throw_the_right_errors() {
    let b = bindings();
    let name = |script: &str| ev(&b, &format!("(function () {{ try {{ {script} }} catch (e) {{ return e.name; }} return 'no error'; }})()"));
    assert_eq!(name("var x = new XMLHttpRequest(); x.setRequestHeader('A', 'b');"), "InvalidStateError");
    assert_eq!(name("var x = new XMLHttpRequest(); x.send();"), "InvalidStateError");
    assert_eq!(name("var x = new XMLHttpRequest(); x.open('GET', '/'); x.send(); x.send();"), "InvalidStateError");
    assert_eq!(name("var x = new XMLHttpRequest(); x.open('GET', '/'); x.send(); x.setRequestHeader('A', 'b');"), "InvalidStateError");
    assert_eq!(name("var x = new XMLHttpRequest(); x.open('GET', '/', false);"), "NotSupportedError");
    assert_eq!(name("var x = new XMLHttpRequest(); x.open('TRACE', '/');"), "SecurityError");
    assert_eq!(name("var x = new XMLHttpRequest(); x.open('B A D', '/');"), "SyntaxError");
    assert_eq!(name("var x = new XMLHttpRequest(); x.open('GET', 'http://');"), "SyntaxError");
    assert_eq!(name("var x = new XMLHttpRequest(); x.open('GET', '/'); x.setRequestHeader('bad name', 'v');"), "SyntaxError");
    assert_eq!(name("var x = new XMLHttpRequest(); x.responseType = 'json'; x.responseText;"), "InvalidStateError");
    assert_eq!(name("XMLHttpRequest();"), "TypeError");
    assert_eq!(name("new XMLHttpRequestUpload();"), "TypeError");
    b.take_net_requests();
}

#[test]
fn response_types() {
    let b = bindings();
    b.evaluate(
        "var types = ['json', 'arraybuffer', 'blob', 'text', 'json']; var xs = types.map(function (t, i) { \
             var x = new XMLHttpRequest(); x.open('GET', '/t' + i); x.responseType = t; x.send(); return x; });",
    )
    .unwrap();
    let reqs = b.take_net_requests();
    assert_eq!(reqs.len(), 5);
    ok(&b, reqs[0].id, 200, &[("Content-Type", "application/json")], br#"{"a":[1,2]}"#);
    ok(&b, reqs[1].id, 200, &[], &[1, 2, 3]);
    ok(&b, reqs[2].id, 200, &[("Content-Type", "image/png")], b"PNG");
    ok(&b, reqs[3].id, 200, &[("Content-Type", "text/plain; charset=utf-8")], "caf\u{e9}".as_bytes());
    ok(&b, reqs[4].id, 200, &[], b"{not json");
    assert_eq!(ev(&b, "JSON.stringify(xs[0].response)"), r#"{"a":[1,2]}"#);
    assert_eq!(ev(&b, "(xs[1].response instanceof ArrayBuffer) + ':' + xs[1].response.byteLength + ':' + new Uint8Array(xs[1].response).join('')"), "true:3:123");
    assert_eq!(ev(&b, "(xs[2].response instanceof Blob) + ':' + xs[2].response.size + ':' + xs[2].response.type"), "true:3:image/png");
    assert_eq!(ev(&b, "xs[3].response"), "caf\u{e9}");
    assert_eq!(ev(&b, "xs[4].response"), "null", "a body that is not JSON is null");
    assert_eq!(ev(&b, "xs[0].response === xs[0].response"), "true", "the parsed value is cached");
}

#[test]
fn a_network_error_fires_error_with_a_zero_status() {
    let b = bindings();
    b.evaluate(
        "var log = []; var x = new XMLHttpRequest(); \
         x.onreadystatechange = function () { log.push('rsc' + x.readyState); }; \
         x.onerror = function () { log.push('error:' + x.status); }; \
         x.onload = function () { log.push('LOAD'); }; \
         x.onloadend = function () { log.push('loadend'); }; \
         x.open('GET', '/x'); x.send();",
    )
    .unwrap();
    let req = one_request(&b);
    b.deliver_net_response(req.id, NetDelivery::Error("network error".into())).unwrap();
    assert_eq!(ev(&b, "log.join()"), "rsc1,rsc4,error:0,loadend");
    assert_eq!(ev(&b, "x.responseText + '|' + x.statusText + '|' + x.getAllResponseHeaders()"), "||");
}

#[test]
fn abort_fires_abort_and_loadend_and_ignores_a_late_response() {
    let b = bindings();
    b.evaluate(
        "var log = []; var x = new XMLHttpRequest(); \
         ['readystatechange', 'abort', 'error', 'load', 'loadend'].forEach(function (t) { \
             x.addEventListener(t, function () { log.push(t === 'readystatechange' ? 'rsc' + x.readyState : t); }); }); \
         x.open('GET', '/x'); x.send();",
    )
    .unwrap();
    let req = one_request(&b);
    b.evaluate("log.push('abort()'); x.abort(); log.push('after:' + x.readyState);").unwrap();
    assert!(!b.deliver_net_response(req.id, NetDelivery::Error("late".into())).unwrap(), "nothing waits for it");
    assert_eq!(ev(&b, "log.join()"), "rsc1,abort(),rsc4,abort,loadend,after:0");

    // Aborting before send() is silent; the request stays OPENED.
    b.evaluate("log = []; x.open('GET', '/y'); x.abort();").unwrap();
    assert_eq!(ev(&b, "log.join() + '|' + x.readyState"), "rsc1|1");
    assert!(b.take_net_requests().is_empty());
}

#[test]
fn aborting_before_the_request_leaves_the_page_means_it_never_does() {
    let b = bindings();
    b.evaluate("var x = new XMLHttpRequest(); x.open('GET', '/x'); x.send(); x.abort();").unwrap();
    assert!(b.take_net_requests().is_empty(), "cancelled while still queued");
}

#[test]
fn a_timeout_fires_timeout_and_the_late_response_is_ignored() {
    let b = bindings();
    b.evaluate(
        "var log = []; var x = new XMLHttpRequest(); x.timeout = 100; \
         x.ontimeout = function () { log.push('timeout:' + x.readyState + ':' + x.status); }; \
         x.onloadend = function () { log.push('loadend'); }; \
         x.onload = function () { log.push('LOAD'); }; \
         x.open('GET', '/slow'); x.send();",
    )
    .unwrap();
    let req = one_request(&b);
    b.run_timers(500, 100).unwrap();
    assert!(!b.deliver_net_response(req.id, NetDelivery::Error("late".into())).unwrap());
    assert_eq!(ev(&b, "log.join()"), "timeout:4:0,loadend");
}

#[test]
fn a_handler_that_aborts_midway_stops_the_rest_and_a_throwing_one_is_reported() {
    let b = bindings();
    b.evaluate(
        "var log = []; var x = new XMLHttpRequest(); \
         x.onreadystatechange = function () { if (x.readyState === 3) x.abort(); }; \
         x.onload = function () { log.push('LOAD'); }; \
         x.onabort = function () { log.push('abort'); }; \
         x.onloadend = function () { log.push('loadend'); }; \
         x.open('GET', '/x'); x.send();",
    )
    .unwrap();
    let req = one_request(&b);
    ok(&b, req.id, 200, &[], b"body");
    assert_eq!(ev(&b, "log.join()"), "abort,loadend", "abort() at LOADING ends the request: no load");

    b.evaluate(
        "var log2 = []; var y = new XMLHttpRequest(); \
         y.onload = function () { throw new Error('handler boom'); }; \
         y.addEventListener('load', function () { log2.push('second listener'); }); \
         y.open('GET', '/y'); y.send();",
    )
    .unwrap();
    let req = one_request(&b);
    ok(&b, req.id, 200, &[], b"");
    assert_eq!(ev(&b, "log2.join()"), "second listener");
    assert!(b.take_reported_errors().iter().any(|e| e.contains("handler boom")));
}

#[test]
fn an_opaque_response_has_no_status_or_body() {
    let b = bindings();
    b.evaluate("var x = new XMLHttpRequest(); x.open('GET', 'http://other.test/x'); x.send();").unwrap();
    let req = one_request(&b);
    b.deliver_net_response(
        req.id,
        NetDelivery::Response {
            url: "http://other.test/x".into(),
            status: 200,
            status_text: "OK".into(),
            headers: vec![("x".into(), "y".into())],
            body_b64: b64(b"secret"),
            kind: "opaque".into(),
            redirected: false,
        },
    )
    .unwrap();
    assert_eq!(ev(&b, "[x.readyState, x.status, x.responseText.length, x.getAllResponseHeaders().length].join()"), "4,0,0,0");
}

#[test]
fn a_full_queue_is_a_network_error_for_the_page() {
    let b = bindings();
    b.evaluate(
        "var errors = 0, xs = []; for (var i = 0; i < 70; i++) { \
             var x = new XMLHttpRequest(); x.onerror = function () { errors++; }; \
             x.open('GET', '/q' + i); x.send(); xs.push(x); }",
    )
    .unwrap();
    assert_eq!(ev(&b, "errors"), "6", "requests past the queue bound fail without leaving the page");
    assert_eq!(b.take_net_requests().len(), 64);
}

#[test]
fn open_after_a_finished_request_starts_over() {
    let b = bindings();
    b.evaluate("var x = new XMLHttpRequest(); x.open('GET', '/one'); x.send();").unwrap();
    let r1 = one_request(&b);
    ok(&b, r1.id, 200, &[], b"one");
    b.evaluate("x.open('POST', '/two'); x.send('b');").unwrap();
    let r2 = one_request(&b);
    assert_eq!((r2.method.as_str(), r2.url.as_str()), ("POST", "http://site.test/two"));
    ok(&b, r2.id, 201, &[], b"two");
    assert_eq!(ev(&b, "x.status + ':' + x.responseText"), "201:two");
}
