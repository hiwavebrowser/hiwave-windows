//! A realm that was dropped (a removed frame's document) runs nothing:
//! entering it and evaluating is an error, never the first realm's global
//! or host object standing in for it.

use rustkit_js::{JsRuntime, JsValue};

#[test]
fn a_dropped_realm_runs_nothing() {
    let mut rt = JsRuntime::new().unwrap();
    rt.evaluate_script("var where = 'main';").unwrap();
    let child = rt.create_realm().unwrap();
    let main = rt.enter_realm(child);
    rt.evaluate_script("var where = 'child';").unwrap();
    rt.enter_realm(main);
    assert!(rt.drop_realm(child));

    let outer = rt.enter_realm(child);
    let plain = rt.evaluate_script("where");
    let host = rt.evaluate_host_turn("return typeof __rk + ':' + where;", &[]);
    let quiet = rt.evaluate_bookkeeping("where");
    rt.enter_realm(outer);
    assert!(plain.is_err(), "evaluated in a dropped realm: {plain:?}");
    assert!(host.is_err(), "a host turn in a dropped realm ran: {host:?}");
    assert!(quiet.is_err(), "bookkeeping in a dropped realm ran: {quiet:?}");
    // The first realm is as it was.
    assert!(matches!(rt.evaluate_script("where").unwrap(), JsValue::String(s) if s == "main"));
}
