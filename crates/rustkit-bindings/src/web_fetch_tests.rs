//! fetch / Headers / Request / Response (web_fetch.js), built on the
//! script-network bridge. The engine's side is played by the test.

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

fn respond(b: &DomBindings, id: u64, status: u16, kind: &str, headers: &[(&str, &str)], body: &[u8]) {
    b.deliver_net_response(
        id,
        NetDelivery::Response {
            url: "http://site.test/dir/data".into(),
            status,
            status_text: "OK".into(),
            headers: headers.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect(),
            body_b64: b64(body),
            kind: kind.into(),
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

fn thrown(b: &DomBindings, script: &str) -> String {
    ev(b, &format!("(function () {{ try {{ {script} }} catch (e) {{ return e.name + ':' + e.constructor.name; }} return 'no error'; }})()"))
}

#[test]
fn without_the_bridge_there_is_no_fetch() {
    let b = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    assert_eq!(ev(&b, "[typeof fetch, typeof Headers, typeof Request, typeof Response].join()"), "undefined,undefined,undefined,undefined");
}

#[test]
fn headers_are_case_insensitive_combined_and_sorted() {
    let b = bindings();
    assert_eq!(
        ev(&b, "var h = new Headers({ 'X-B': '2', 'a': '1' }); h.append('A', '1b'); h.append('x-b', '3'); \
                [h.get('a'), h.get('X-B'), h.has('A'), h.has('zz'), String(h.get('zz')), Array.from(h).join('|')].join(' / ')"),
        "1, 1b / 2, 3 / true / false / null / a,1, 1b|x-b,2, 3"
    );
    assert_eq!(
        ev(&b, "var h2 = new Headers([['b', '1'], ['a', '2']]); h2.set('B', 'x'); h2.delete('nope'); \
                var out = []; h2.forEach(function (v, k, o) { out.push(k + '=' + v + (o === h2)); }); \
                out.join() + ' ' + Array.from(h2.keys()) + ' ' + Array.from(h2.values())"),
        "a=2true,b=xtrue a,b 2,x"
    );
    assert_eq!(ev(&b, "var h3 = new Headers(new Headers({ k: 'v' })); h3.get('k') + (h3 instanceof Headers)"), "vtrue");
    assert_eq!(
        ev(&b, "var c = new Headers(); c.append('Set-Cookie', 'a=1'); c.append('Set-Cookie', 'b=2'); \
                c.getSetCookie().join('&') + '|' + Array.from(c).map(function (e) { return e.join(':'); }).join('&')"),
        "a=1&b=2|set-cookie:a=1&set-cookie:b=2"
    );
    assert_eq!(thrown(&b, "new Headers().append('bad name', 'v');"), "TypeError:TypeError");
    assert_eq!(thrown(&b, "new Headers().append('ok', 'a\\nb');"), "TypeError:TypeError");
    assert_eq!(thrown(&b, "Headers();"), "TypeError:TypeError");
}

#[test]
fn a_request_has_the_defaults_and_resolves_its_url() {
    let b = bindings();
    assert_eq!(
        ev(&b, "var r = new Request('../data?x=1#h', { method: 'post', body: 'hi' }); \
                [r.url, r.method, r.mode, r.credentials, r.redirect, r.cache, r.bodyUsed, r.headers.get('content-type'), String(r.signal)].join(' ')"),
        "http://site.test/data?x=1#h POST cors same-origin follow default false text/plain;charset=UTF-8 null"
    );
    assert_eq!(thrown(&b, "new Request('http://u:p@site.test/');"), "TypeError:TypeError");
    assert_eq!(thrown(&b, "new Request('/x', { method: 'TRACE' });"), "TypeError:TypeError");
    assert_eq!(thrown(&b, "new Request('/x', { body: 'b' });"), "TypeError:TypeError");
    assert_eq!(thrown(&b, "new Request('/x', { mode: 'navigate' });"), "TypeError:TypeError");
    assert_eq!(thrown(&b, "new Request('/x', { credentials: 'sometimes' });"), "TypeError:TypeError");
    assert_eq!(thrown(&b, "new Request();"), "TypeError:TypeError");
    // Forbidden request headers are dropped silently.
    assert_eq!(
        ev(&b, "var r2 = new Request('/x', { headers: { Cookie: 'a', 'X-Ok': '1', 'Sec-Fetch-Mode': 'x', Host: 'evil' } }); Array.from(r2.headers).join('|')"),
        "x-ok,1"
    );
    // A Request from a Request copies it; the body moves.
    assert_eq!(
        ev(&b, "var a = new Request('/y', { method: 'PUT', body: 'abc', headers: { 'X-A': '1' } }); var c = new Request(a, { method: 'POST' }); \
                [c.method, c.headers.get('x-a'), a.bodyUsed].join()"),
        "POST,1,true"
    );
    assert_eq!(thrown(&b, "var q = new Request('/z', { method: 'POST', body: 'x' }); q.text(); new Request(q);"), "TypeError:TypeError");
}

#[test]
fn a_response_has_the_defaults_and_reads_its_body_once() {
    let b = bindings();
    assert_eq!(
        ev(&b, "var r = new Response('hello', { status: 201, statusText: 'Made', headers: { 'X-A': '1' } }); \
                [r.status, r.ok, r.statusText, r.type, r.url === '', r.redirected, r.headers.get('x-a'), r.headers.get('content-type'), r.bodyUsed].join('|')"),
        "201|true|Made|default|true|false|1|text/plain;charset=UTF-8|false"
    );
    b.evaluate("var log = []; var rr = new Response('{\"a\":1}'); rr.json().then(function (v) { log.push('json:' + v.a + ':' + rr.bodyUsed); }); rr.text().catch(function (e) { log.push(e.constructor.name); });").unwrap();
    assert_eq!(ev(&b, "log.join()"), "TypeError,json:1:true", "a body is read once");
    assert_eq!(thrown(&b, "new Response('x', { status: 100 });"), "RangeError:RangeError");
    assert_eq!(thrown(&b, "new Response('x', { status: 204 });"), "TypeError:TypeError");
    assert_eq!(ev(&b, "new Response(null, { status: 204 }).body"), "null");
    assert_eq!(ev(&b, "new Response(new Headers().get('x')).bodyUsed"), "false");
}

#[test]
fn a_response_body_reads_as_text_json_bytes_blob_and_form() {
    let b = bindings();
    b.evaluate(
        "var log = []; \
         new Response(new Uint8Array([104, 105])).text().then(function (t) { log.push('text:' + t); }); \
         new Response(new Uint8Array([1, 2, 3])).arrayBuffer().then(function (a) { log.push('ab:' + a.byteLength + new Uint8Array(a).join('')); }); \
         new Response('xyz', { headers: { 'content-type': 'text/x' } }).blob().then(function (bl) { log.push('blob:' + bl.size + ':' + bl.type); }); \
         new Response('a=1&b=2', { headers: { 'content-type': 'application/x-www-form-urlencoded' } }).formData().then(function (f) { log.push('form:' + f.get('a') + f.get('b')); }); \
         new Response('{bad').json().catch(function (e) { log.push('badjson:' + e.name); }); \
         new Response('x', { headers: { 'content-type': 'multipart/form-data; boundary=q' } }).formData().catch(function (e) { log.push('multipart:' + e.constructor.name); }); \
         Response.json({ k: [1] }).text().then(function (t) { log.push('static:' + t); });",
    )
    .unwrap();
    let log = ev(&b, "log.slice().sort().join('|')");
    assert_eq!(
        log,
        "ab:3123|badjson:SyntaxError|blob:3:text/x|form:12|multipart:TypeError|static:{\"k\":[1]}|text:hi"
    );
    assert_eq!(ev(&b, "Response.json(1).headers.get('content-type')"), "application/json");
    assert_eq!(ev(&b, "var e = Response.error(); [e.type, e.status, e.ok].join()"), "error,0,false");
    assert_eq!(ev(&b, "var rd = Response.redirect('/to', 301); [rd.status, rd.headers.get('location')].join()"), "301,http://site.test/to");
    assert_eq!(thrown(&b, "Response.redirect('/to', 200);"), "RangeError:RangeError");
    assert_eq!(ev(&b, "var c = new Response('set-cookie test', { headers: { 'Set-Cookie': 'a=1' } }); c.headers.has('set-cookie')"), "false");
}

#[test]
fn clone_gives_two_independent_bodies_and_refuses_a_used_one() {
    let b = bindings();
    b.evaluate("var log = []; var r = new Response('twice'); var c = r.clone(); r.text().then(function (t) { log.push('r:' + t); }); c.text().then(function (t) { log.push('c:' + t); });").unwrap();
    assert_eq!(ev(&b, "log.join()"), "r:twice,c:twice");
    assert_eq!(thrown(&b, "r.clone();"), "TypeError:TypeError");
    assert_eq!(ev(&b, "var q = new Request('/q', { method: 'POST', body: 'b' }); var qc = q.clone(); [qc.method, qc.url, q.bodyUsed, qc.bodyUsed].join()"), "POST,http://site.test/q,false,false");
}

#[test]
fn the_body_is_a_stream_of_one_chunk() {
    let b = bindings();
    b.evaluate(
        "var log = []; var r = new Response('stream me'); var rd = r.body.getReader(); \
         rd.read().then(function (a) { log.push('chunk:' + new TextDecoder().decode(a.value) + ':' + r.bodyUsed); return rd.read(); }) \
           .then(function (a) { log.push('done:' + a.done); });",
    )
    .unwrap();
    assert_eq!(ev(&b, "log.join()"), "chunk:stream me:true,done:true");
}

#[test]
fn fetch_asks_the_bridge_and_resolves_with_a_response() {
    let b = bindings();
    b.evaluate(
        "var log = []; \
         fetch('data?q=1', { method: 'post', headers: { 'X-A': '1', Cookie: 'x' }, body: JSON.stringify({ a: 1 }) }) \
           .then(function (r) { log.push([r.ok, r.status, r.statusText, r.type, r.url, r.redirected, r.headers.get('X-Thing')].join('|')); return r.text(); }) \
           .then(function (t) { log.push('body:' + t); });",
    )
    .unwrap();
    let req = one_request(&b);
    assert_eq!(req.method, "POST");
    assert_eq!(req.url, "http://site.test/dir/data?q=1");
    assert_eq!((req.mode.as_str(), req.credentials.as_str(), req.redirect.as_str(), req.destination.as_str()), ("cors", "same-origin", "follow", "fetch"));
    assert_eq!(req.headers, vec![("x-a".to_string(), "1".to_string()), ("content-type".to_string(), "text/plain;charset=UTF-8".to_string())]);
    assert!(req.body_b64.is_some());
    respond(&b, req.id, 200, "basic", &[("X-Thing", "a"), ("x-thing", "b")], b"payload");
    assert_eq!(ev(&b, "log.join('#')"), "true|200|OK|basic|http://site.test/dir/data|false|a, b#body:payload");
    assert_eq!(thrown(&b, "r0 = new Response('x'); fetch('/x').then(function (r) { r.headers.append('a', 'b'); });"), "no error");
}

#[test]
fn fetch_options_reach_the_bridge() {
    let b = bindings();
    b.evaluate("fetch('/a', { mode: 'no-cors', credentials: 'include', redirect: 'manual' }); fetch(new Request('/b', { method: 'PUT', body: new Blob(['bb']) }));").unwrap();
    let reqs = b.take_net_requests();
    assert_eq!(reqs.len(), 2);
    assert_eq!((reqs[0].mode.as_str(), reqs[0].credentials.as_str(), reqs[0].redirect.as_str()), ("no-cors", "include", "manual"));
    assert_eq!((reqs[1].method.as_str(), reqs[1].body_b64.as_deref()), ("PUT", Some("YmI=")));
}

#[test]
fn a_network_error_rejects_with_a_type_error() {
    let b = bindings();
    b.evaluate("var log = []; fetch('/x').then(function () { log.push('resolved'); }, function (e) { log.push(e.constructor.name + ':' + e.message); });").unwrap();
    let req = one_request(&b);
    b.deliver_net_response(req.id, NetDelivery::Error("network error".into())).unwrap();
    assert_eq!(ev(&b, "log.join()"), "TypeError:Failed to fetch");
}

#[test]
fn an_opaque_response_has_no_status_headers_or_body() {
    let b = bindings();
    b.evaluate("var log = []; fetch('http://other.test/x', { mode: 'no-cors' }).then(function (r) { log.push([r.type, r.status, r.ok, r.statusText, r.url, Array.from(r.headers).length, String(r.body)].join('|')); return r.text(); }).then(function (t) { log.push('text:[' + t + ']'); });").unwrap();
    let req = one_request(&b);
    respond(&b, req.id, 200, "opaque", &[("x", "y")], b"secret");
    assert_eq!(ev(&b, "log.join('#')"), "opaque|0|false|||0|null#text:[]");
}

#[test]
fn a_null_body_status_has_no_body() {
    let b = bindings();
    b.evaluate("var log = []; fetch('/x', { method: 'DELETE' }).then(function (r) { log.push(r.status + ':' + String(r.body)); });").unwrap();
    let req = one_request(&b);
    respond(&b, req.id, 204, "basic", &[], b"ignored");
    assert_eq!(ev(&b, "log.join()"), "204:null");
}

#[test]
fn abort_rejects_with_the_signals_reason_and_cancels_the_request() {
    let b = bindings();
    // Aborted before fetch: nothing is queued.
    b.evaluate("var log = []; var c0 = new AbortController(); c0.abort(); fetch('/never', { signal: c0.signal }).catch(function (e) { log.push('pre:' + e.name); });").unwrap();
    assert!(b.take_net_requests().is_empty());
    assert_eq!(ev(&b, "log.join()"), "pre:AbortError");

    // Aborted while queued: it never leaves the page.
    b.evaluate("var c1 = new AbortController(); fetch('/q', { signal: c1.signal }).catch(function (e) { log.push('queued:' + e.name); }); c1.abort();").unwrap();
    assert!(b.take_net_requests().is_empty());

    // Aborted in flight: rejects, and a late response is ignored.
    b.evaluate("var c2 = new AbortController(); fetch('/f', { signal: c2.signal }).then(function () { log.push('RESOLVED'); }, function (e) { log.push('flight:' + e.name + ':' + e.message); });").unwrap();
    let req = one_request(&b);
    b.evaluate("c2.abort('custom');").unwrap();
    assert!(!b.deliver_net_response(req.id, NetDelivery::Error("late".into())).unwrap());
    assert_eq!(ev(&b, "log.join()"), "pre:AbortError,queued:AbortError,flight:undefined:undefined");
}

#[test]
fn a_full_queue_rejects_the_overflow() {
    let b = bindings();
    b.evaluate("var rejected = 0; for (var i = 0; i < 70; i++) fetch('/q' + i).catch(function () { rejected++; });").unwrap();
    assert_eq!(ev(&b, "rejected"), "6");
    assert_eq!(b.take_net_requests().len(), 64);
}

/// Prometheus #898 must-fix: a `no-cors` request keeps only the
/// CORS-safelisted headers for its whole life, not just at construction.
#[test]
fn a_no_cors_request_can_never_carry_a_non_safelisted_header() {
    let b = bindings();
    // At construction.
    assert_eq!(
        ev(&b, "var r = new Request('http://other.test/x', { mode: 'no-cors', headers: { Authorization: 'Bearer t', Accept: 'text/x', 'X-Custom': '1', 'Content-Type': 'application/json' } }); Array.from(r.headers).join('|')"),
        "accept,text/x"
    );
    // After construction: append / set cannot widen it; safelisted values still work.
    assert_eq!(
        ev(&b, "r.headers.append('Authorization', 'Bearer t'); r.headers.set('X-Custom', '1'); r.headers.set('Content-Type', 'application/json'); \
                r.headers.set('Content-Type', 'text/plain;charset=UTF-8'); r.headers.append('Accept-Language', 'en'); \
                Array.from(r.headers).join('|')"),
        "accept,text/x|accept-language,en|content-type,text/plain;charset=UTF-8"
    );
    // A cors request is not restricted by this guard.
    assert_eq!(ev(&b, "var c = new Request('/x', { headers: { Authorization: 'a' } }); c.headers.append('X-Custom', '1'); Array.from(c.headers).join('|')"), "authorization,a|x-custom,1");
    // And the wire only ever sees the safelisted set.
    b.evaluate("fetch(r); fetch('http://other.test/y', { mode: 'no-cors', headers: { Authorization: 'x', Accept: '*/*' } });").unwrap();
    let reqs = b.take_net_requests();
    assert_eq!(reqs.len(), 2);
    for req in &reqs {
        assert!(req.headers.iter().all(|(n, _)| ["accept", "accept-language", "content-type", "content-language"].contains(&n.as_str())), "{:?}", req.headers);
    }
    // Only GET, HEAD and POST are allowed in no-cors mode, and it fails at construction.
    assert_eq!(thrown(&b, "new Request('/x', { mode: 'no-cors', method: 'PUT' });"), "TypeError:TypeError");
    assert_eq!(thrown(&b, "new Request('/x', { mode: 'no-cors', method: 'POST' });"), "no error");
    assert_eq!(thrown(&b, "fetch('/x', { mode: 'no-cors', method: 'DELETE' }).catch(function () {}); new Request('/x', { mode: 'no-cors', method: 'DELETE' });"), "TypeError:TypeError");
}
