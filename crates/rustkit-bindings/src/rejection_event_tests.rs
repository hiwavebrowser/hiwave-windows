//! PromiseRejectionEvent and SubmitEvent (web_interfaces.js).

use super::*;

fn ev(script: &str) -> String {
    let b = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    match b.evaluate(script).unwrap() {
        JsValue::String(s) => s,
        JsValue::Boolean(x) => x.to_string(),
        JsValue::Number(n) => format!("{n}"),
        JsValue::Undefined => "undefined".into(),
        JsValue::Null => "null".into(),
        other => format!("{other:?}"),
    }
}

#[test]
fn a_promise_rejection_event_carries_the_promise_and_reason() {
    assert_eq!(
        ev("var p = Promise.resolve(1); var e = new PromiseRejectionEvent('unhandledrejection', { promise: p, reason: 'why', cancelable: true }); [e.type, e.promise === p, e.reason, e.cancelable, e instanceof Event, typeof PromiseRejectionEvent].join()"),
        "unhandledrejection,true,why,true,true,function"
    );
    assert_eq!(ev("var e2 = new PromiseRejectionEvent('rejectionhandled'); [String(e2.promise), String(e2.reason)].join()"), "null,undefined");
}

#[test]
fn a_submit_event_carries_the_submitter() {
    assert_eq!(
        ev("var s = new SubmitEvent('submit', { submitter: null, bubbles: true }); [s.type, String(s.submitter), s.bubbles, s instanceof Event].join()"),
        "submit,null,true,true"
    );
}

#[test]
fn core_js_keeps_the_native_promise_when_the_rejection_event_exists() {
    // core-js decides to replace Promise when PromiseRejectionEvent is missing (a browser
    // without unhandled-rejection events); its polyfill spun lyft's _app until the loop limit.
    assert_eq!(ev("[typeof PromiseRejectionEvent === 'function', typeof SubmitEvent === 'function'].join()"), "true,true");
}

#[test]
fn a_runaway_promise_microtask_chain_throws_and_recovers_in_bindings() {
    let b = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    b.set_max_job_iterations(200);
    let result = b.evaluate("function again() { Promise.resolve().then(again); } again();");
    assert!(result.is_err(), "runaway promise chain must fail: {result:?}");
    // DomBindings remains usable
    let after = b.evaluate("1 + 2").unwrap();
    assert!(matches!(after, JsValue::Number(n) if (n - 3.0).abs() < f64::EPSILON));
}

