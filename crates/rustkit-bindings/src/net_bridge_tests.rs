//! The script-network bridge (net_bridge.rs / web_net_bridge.js). Its own
//! test module so it does not collide with the other families' tests.

use super::*;
use crate::net_bridge::{NetDelivery, NetRequest};

fn bindings() -> DomBindings {
    DomBindings::new(JsRuntime::new().unwrap()).unwrap()
}

/// The bridge, installed, and handed to the page as `window.__net` for
/// these tests alone: the engine keeps it on the host object, where no page
/// script reaches it.
fn bridge(b: &DomBindings) {
    b.enable_net_bridge().unwrap();
    b.host_turn("window.__net = __rk.net; return 0;", &[]).unwrap();
}

fn ev(bindings: &DomBindings, script: &str) -> String {
    match bindings.evaluate(script).unwrap() {
        JsValue::String(s) => s,
        JsValue::Boolean(b) => b.to_string(),
        JsValue::Number(n) => {
            if n.fract() == 0.0 { format!("{}", n as i64) } else { n.to_string() }
        }
        JsValue::Null => "null".to_string(),
        JsValue::Undefined => "undefined".to_string(),
        other => panic!("{script} evaluated to {other:?}"),
    }
}

fn ok(url: &str, status: u16, body_b64: &str) -> NetDelivery {
    NetDelivery::Response {
        url: url.to_string(),
        status,
        status_text: "OK".to_string(),
        headers: vec![("content-type".to_string(), "text/plain".to_string())],
        body_b64: body_b64.to_string(),
        kind: "basic".to_string(),
        redirected: false,
    }
}

/// "never a permissive stand-in": a bindings instance the engine did not
/// enable the bridge on has no network entry point at all.
#[test]
fn without_the_bridge_there_is_no_network_entry_point() {
    let b = bindings();
    assert!(!b.net_bridge_enabled());
    assert_eq!(ev(&b, "typeof window.__net + typeof window.__net"), "undefinedundefined");
    assert!(b.take_net_requests().is_empty());
    assert!(!b.deliver_net_response(1, ok("http://x/", 200, "")).unwrap());
    assert_eq!(b.pending_net_requests(), 0);
}

#[test]
fn a_queued_request_is_taken_once_with_its_defaults() {
    let b = bindings();
    bridge(&b);
    assert!(b.net_bridge_enabled());
    b.evaluate(
        "window.__net.request({ url: 'http://a/one' }, function () {}); \
         window.__net.request({ method: 'POST', url: 'http://a/two', \
           headers: [['X-A', 1]], body_b64: 'aGk=', mode: 'no-cors', credentials: 'include', \
           redirect: 'manual', destination: 'xhr' }, function () {});",
    )
    .unwrap();
    let taken = b.take_net_requests();
    assert_eq!(
        taken,
        vec![
            NetRequest {
                id: 1,
                method: "GET".into(),
                url: "http://a/one".into(),
                headers: vec![],
                body_b64: None,
                mode: "cors".into(),
                credentials: "same-origin".into(),
                redirect: "follow".into(),
                destination: "fetch".into(),
            },
            NetRequest {
                id: 2,
                method: "POST".into(),
                url: "http://a/two".into(),
                headers: vec![("X-A".into(), "1".into())],
                body_b64: Some("aGk=".into()),
                mode: "no-cors".into(),
                credentials: "include".into(),
                redirect: "manual".into(),
                destination: "xhr".into(),
            },
        ]
    );
    assert!(b.take_net_requests().is_empty(), "taken once");
    assert_eq!(b.pending_net_requests(), 2, "both still wait for a delivery");
}

#[test]
fn a_delivery_reaches_its_callback_once_and_only_its_callback() {
    let b = bindings();
    bridge(&b);
    b.evaluate(
        "var log = []; \
         window.__net.request({ url: 'http://a/1' }, function (r) { log.push('1:' + r.status + ':' + r.body_b64 + ':' + r.kind); }); \
         window.__net.request({ url: 'http://a/2' }, function (r) { log.push('2:' + r.ok + ':' + r.error); });",
    )
    .unwrap();
    b.take_net_requests();
    assert!(b.deliver_net_response(2, NetDelivery::Error("Denial::Csp".into())).unwrap());
    assert!(b.deliver_net_response(1, ok("http://a/1", 201, "aGk=")).unwrap());
    assert_eq!(ev(&b, "log.join('|')"), "2:false:Denial::Csp|1:201:aGk=:basic");
    // Settled once: a second delivery finds nothing waiting.
    assert!(!b.deliver_net_response(1, ok("http://a/1", 200, "")).unwrap());
    assert_eq!(b.pending_net_requests(), 0);
}

#[test]
fn a_response_with_hostile_text_survives_the_json_hop() {
    let b = bindings();
    bridge(&b);
    b.evaluate(
        "var got; window.__net.request({ url: 'http://a/' }, function (r) { got = r; });",
    )
    .unwrap();
    b.take_net_requests();
    let nasty = "quote\" backslash\\ newline\n line\u{2028} </script> \u{1F600} 'single'";
    b.deliver_net_response(
        1,
        NetDelivery::Response {
            url: "http://a/?q=\"x\"".into(),
            status: 200,
            status_text: nasty.into(),
            headers: vec![("x-n".into(), nasty.into())],
            body_b64: String::new(),
            kind: "basic".into(),
            redirected: true,
        },
    )
    .unwrap();
    let literal = serde_json::to_string(nasty).unwrap();
    assert_eq!(ev(&b, &format!("got.status_text === {literal}")), "true");
    assert_eq!(ev(&b, &format!("got.headers[0][1] === {literal}")), "true");
    assert_eq!(ev(&b, "got.redirected"), "true");
}

/// Aborting: a request not yet taken never leaves the page; one already taken
/// has its eventual response ignored.
#[test]
fn cancel_removes_a_queued_request_and_drops_a_late_response() {
    let b = bindings();
    bridge(&b);
    b.evaluate(
        "var fired = 0; \
         var a = window.__net.request({ url: 'http://a/queued' }, function () { fired++; }); \
         var c = window.__net.request({ url: 'http://a/flying' }, function () { fired++; }); \
         window.__net.cancel(a);",
    )
    .unwrap();
    let taken = b.take_net_requests();
    assert_eq!(taken.len(), 1, "the cancelled request never left");
    assert_eq!(taken[0].url, "http://a/flying");
    b.evaluate("window.__net.cancel(c);").unwrap();
    assert!(!b.deliver_net_response(taken[0].id, ok("http://a/flying", 200, "")).unwrap());
    assert_eq!(ev(&b, "fired"), "0");
}

#[test]
fn a_throwing_callback_is_reported_and_does_not_block_the_others() {
    let b = bindings();
    bridge(&b);
    b.evaluate(
        "var after = 0; \
         window.__net.request({ url: 'http://a/1' }, function () { throw new Error('boom'); }); \
         window.__net.request({ url: 'http://a/2' }, function () { after++; });",
    )
    .unwrap();
    b.take_net_requests();
    b.deliver_net_response(1, ok("http://a/1", 200, "")).unwrap();
    b.deliver_net_response(2, ok("http://a/2", 200, "")).unwrap();
    assert_eq!(ev(&b, "after"), "1");
    let errors = b.take_reported_errors();
    assert_eq!(errors.len(), 1);
    assert!(errors[0].contains("boom"), "{errors:?}");
}

#[test]
fn failing_the_pending_requests_completes_each_with_an_error() {
    let b = bindings();
    bridge(&b);
    b.evaluate(
        "var log = []; \
         window.__net.request({ url: 'http://a/1' }, function (r) { log.push(r.ok + ':' + r.error); }); \
         window.__net.request({ url: 'http://a/2' }, function (r) { log.push(r.ok + ':' + r.error); });",
    )
    .unwrap();
    // One taken, one still queued: both are failed, and the queue empties.
    b.take_net_requests();
    b.evaluate("window.__net.request({ url: 'http://a/3' }, function (r) { log.push('3:' + r.error); });")
        .unwrap();
    assert_eq!(b.fail_pending_net_requests("network budget exhausted"), 3);
    assert_eq!(
        ev(&b, "log.join('|')"),
        "false:network budget exhausted|false:network budget exhausted|3:network budget exhausted"
    );
    assert!(b.take_net_requests().is_empty());
    assert_eq!(b.pending_net_requests(), 0);
}

#[test]
fn a_shelved_request_survives_fail_all_and_delivers_after_unshelve() {
    let b = bindings();
    bridge(&b);
    b.evaluate(
        "var log = []; \
         window.__net.request({ url: 'http://a/1' }, function (r) { log.push(r.ok + ':' + (r.error || r.status)); }); \
         window.__net.request({ url: 'http://a/2' }, function (r) { log.push('2:' + r.error); });",
    )
    .unwrap();
    let taken = b.take_net_requests();
    assert_eq!(taken.len(), 2);
    assert!(b.shelve_net_request(taken[0].id));
    assert_eq!(b.pending_net_requests(), 1);
    assert_eq!(b.fail_pending_net_requests("network error"), 1);
    assert_eq!(ev(&b, "log.join('|')"), "2:network error");
    assert_eq!(b.unshelve_net_requests(), 1);
    b.deliver_net_response(taken[0].id, ok("http://a/1", 200, ""))
        .unwrap();
    assert_eq!(ev(&b, "log.join('|')"), "2:network error|true:200");
    assert_eq!(b.pending_net_requests(), 0);
}

#[test]
fn the_queue_is_bounded() {
    let b = bindings();
    bridge(&b);
    assert_eq!(
        ev(
            &b,
            "var ids = []; \
             for (var i = 0; i < 1030; i++) ids.push(window.__net.request({ url: 'http://a/' + i }, function () {})); \
             ids.filter(function (n) { return n === 0; }).length"
        ),
        "6",
        "the 1025th request onward is refused with id 0"
    );
    assert_eq!(b.take_net_requests().len(), 1024);
}

#[test]
fn a_request_needs_a_callback() {
    let b = bindings();
    bridge(&b);
    assert!(b.evaluate("window.__net.request({ url: 'http://a/' })").is_err());
}

// ── R1 on #1088 ────────────────────────────────────────────────────────────

/// The engine's blob lookup (`blob_url_text`, for `<script src="blob:">`)
/// runs none of the page's script: it is evaluated as bookkeeping, with no
/// script budget, so anything of the page's on that path could spin for
/// ever. A page that replaces the store's methods, or the built-ins the
/// lookup used to go through, changes nothing.
#[test]
fn the_engines_blob_lookup_runs_no_page_script() {
    let b = bindings();
    ev(&b, "var u = URL.createObjectURL(new Blob(['window.fromBlob = 7;'], { type: 'text/javascript' }));");
    ev(
        &b,
        "window.hijacked = 0; function hit() { window.hijacked++; return 'evil'; } \
         try { __rustkit_blob_urls.text = hit; } catch (e) {} \
         try { __rustkit_blob_urls.get = hit; } catch (e) {} \
         try { Object.defineProperty(window, '__rustkit_blob_urls', { value: { text: hit, get: hit } }); } catch (e) {} \
         String.prototype.split = function () { hit(); return ['x']; }; \
         window.TextDecoder = function () { hit(); this.decode = hit; }; \
         Uint8Array.prototype.slice = hit; 'set'",
    );
    let url = ev(&b, "u");
    assert_eq!(b.blob_url_text(&url).as_deref(), Some("window.fromBlob = 7;"));
    assert_eq!(b.blob_url_text(&format!("{url}#frag")).as_deref(), Some("window.fromBlob = 7;"), "a fragment names the same blob");
    assert_eq!(ev(&b, "window.hijacked"), "0", "the lookup ran page script");
    // A page cannot put a record of its own making behind a name: an object
    // that passes `instanceof Blob` with a trap for every read (R1 delta on
    // #1088). There is no store on the window to hand it to, and what
    // createObjectURL keeps is a copy of the bytes, made in the page's turn.
    ev(
        &b,
        "var trap = new Proxy(Object.create(Blob.prototype), { get: function () { hit(); return new Uint8Array(0); } }); \
         try { __rustkit_blob_urls.add('blob:x', trap); } catch (e) {} \
         var made; try { made = URL.createObjectURL(trap); } catch (e) { made = 'threw'; } \
         window.hijacked = 0; 'set'",
    );
    assert_eq!(b.blob_url_text("blob:x"), None);
    let made = ev(&b, "String(made)");
    if made != "threw" {
        let _ = b.blob_url_text(&made);
    }
    assert_eq!(ev(&b, "window.hijacked"), "0", "a trap ran outside the page's own turn");
    // (This page defined a window property of the store's old name above;
    // a page that has not sees nothing of the store.)
    let fresh = bindings();
    assert_eq!(ev(&fresh, "typeof __rustkit_blob_urls + typeof __rkBlobInternals"), "undefinedundefined", "the store is not on the window");
    // ... and a global of that name the page declares is just the page's.
    let _ = b.evaluate("let __rustkit_blob_urls = { text: hit, get: hit, bytes: hit }; 0");
    assert_eq!(b.blob_url_text(&url).as_deref(), Some("window.fromBlob = 7;"));
    assert_eq!(ev(&b, "window.hijacked"), "0");
}

/// A blob URL names a Blob of the document that made it: revoked, it names
/// nothing (for the engine and for fetch), and another document's bindings
/// never had it. Chrome 148: fetching either is a TypeError.
#[test]
fn a_revoked_blob_url_and_another_documents_name_nothing() {
    let b = bindings();
    b.enable_net_bridge().unwrap();
    ev(&b, "var u = URL.createObjectURL(new Blob(['x'])); var out = 'pending';");
    let url = ev(&b, "u");
    let other = bindings();
    other.enable_net_bridge().unwrap();
    assert_eq!(other.blob_url_text(&url), None);
    ev(&other, &format!("var out = 'pending'; fetch({url:?}).then(function () {{ out = 'answered'; }}, function (e) {{ out = e.name; }}); 0"));
    assert_eq!(ev(&other, "out"), "TypeError");
    assert!(other.take_net_requests().is_empty(), "a blob URL never goes to the network");
    ev(&b, "URL.revokeObjectURL(u); fetch(u).then(function () { out = 'answered'; }, function (e) { out = e.name; }); 0");
    assert_eq!(ev(&b, "out"), "TypeError");
    assert_eq!(b.blob_url_text(&url), None);
}

/// What a page can park before the engine drains is bounded in bytes as
/// well as in count: request bodies travel in the queue, and 1024 requests
/// of any size was no bound on memory. Past 64 MiB of parked body a request
/// is refused, as past the count; taking the queue makes room again.
#[test]
fn the_queue_bounds_parked_body_bytes() {
    let b = bindings();
    bridge(&b);
    assert_eq!(
        ev(
            &b,
            "var body = 'A'.repeat(40 * 1024 * 1024); \
             var ids = [1, 2, 3].map(function (i) { return window.__net.request({ method: 'POST', url: 'http://a/' + i, body_b64: body }, function () {}); }); \
             var small = window.__net.request({ url: 'http://a/small' }, function () {}); \
             [ids[0] > 0, ids[1], ids[2], small > 0].join()"
        ),
        "true,0,0,true",
        "the second 40 MiB body is past the 64 MiB bound; a request without a body still fits"
    );
    assert_eq!(b.take_net_requests().len(), 2);
    assert_eq!(ev(&b, "window.__net.request({ method: 'POST', url: 'http://a/4', body_b64: body }, function () {}) > 0"), "true");
}
