//! The script-network bridge (net_bridge.rs / web_net_bridge.js). Its own
//! test module so it does not collide with the other families' tests.

use super::*;
use crate::net_bridge::{NetDelivery, NetRequest};

fn bindings() -> DomBindings {
    DomBindings::new(JsRuntime::new().unwrap()).unwrap()
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
    assert_eq!(ev(&b, "typeof window.__rustkit_net"), "undefined");
    assert!(b.take_net_requests().is_empty());
    assert!(!b.deliver_net_response(1, ok("http://x/", 200, "")).unwrap());
    assert_eq!(b.pending_net_requests(), 0);
}

#[test]
fn a_queued_request_is_taken_once_with_its_defaults() {
    let b = bindings();
    b.enable_net_bridge().unwrap();
    assert!(b.net_bridge_enabled());
    b.evaluate(
        "window.__rustkit_net.request({ url: 'http://a/one' }, function () {}); \
         window.__rustkit_net.request({ method: 'POST', url: 'http://a/two', \
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
    b.enable_net_bridge().unwrap();
    b.evaluate(
        "var log = []; \
         window.__rustkit_net.request({ url: 'http://a/1' }, function (r) { log.push('1:' + r.status + ':' + r.body_b64 + ':' + r.kind); }); \
         window.__rustkit_net.request({ url: 'http://a/2' }, function (r) { log.push('2:' + r.ok + ':' + r.error); });",
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
    b.enable_net_bridge().unwrap();
    b.evaluate(
        "var got; window.__rustkit_net.request({ url: 'http://a/' }, function (r) { got = r; });",
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
    b.enable_net_bridge().unwrap();
    b.evaluate(
        "var fired = 0; \
         var a = window.__rustkit_net.request({ url: 'http://a/queued' }, function () { fired++; }); \
         var c = window.__rustkit_net.request({ url: 'http://a/flying' }, function () { fired++; }); \
         window.__rustkit_net.cancel(a);",
    )
    .unwrap();
    let taken = b.take_net_requests();
    assert_eq!(taken.len(), 1, "the cancelled request never left");
    assert_eq!(taken[0].url, "http://a/flying");
    b.evaluate("window.__rustkit_net.cancel(c);").unwrap();
    assert!(!b.deliver_net_response(taken[0].id, ok("http://a/flying", 200, "")).unwrap());
    assert_eq!(ev(&b, "fired"), "0");
}

#[test]
fn a_throwing_callback_is_reported_and_does_not_block_the_others() {
    let b = bindings();
    b.enable_net_bridge().unwrap();
    b.evaluate(
        "var after = 0; \
         window.__rustkit_net.request({ url: 'http://a/1' }, function () { throw new Error('boom'); }); \
         window.__rustkit_net.request({ url: 'http://a/2' }, function () { after++; });",
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
    b.enable_net_bridge().unwrap();
    b.evaluate(
        "var log = []; \
         window.__rustkit_net.request({ url: 'http://a/1' }, function (r) { log.push(r.ok + ':' + r.error); }); \
         window.__rustkit_net.request({ url: 'http://a/2' }, function (r) { log.push(r.ok + ':' + r.error); });",
    )
    .unwrap();
    // One taken, one still queued: both are failed, and the queue empties.
    b.take_net_requests();
    b.evaluate("window.__rustkit_net.request({ url: 'http://a/3' }, function (r) { log.push('3:' + r.error); });")
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
fn the_queue_is_bounded() {
    let b = bindings();
    b.enable_net_bridge().unwrap();
    assert_eq!(
        ev(
            &b,
            "var ids = []; \
             for (var i = 0; i < 70; i++) ids.push(window.__rustkit_net.request({ url: 'http://a/' + i }, function () {})); \
             ids.filter(function (n) { return n === 0; }).length"
        ),
        "6",
        "the 65th request onward is refused with id 0"
    );
    assert_eq!(b.take_net_requests().len(), 64);
}

#[test]
fn a_request_needs_a_callback() {
    let b = bindings();
    b.enable_net_bridge().unwrap();
    assert!(b.evaluate("window.__rustkit_net.request({ url: 'http://a/' })").is_err());
}
