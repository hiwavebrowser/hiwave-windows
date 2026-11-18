//! `JsRuntime::keep_function` / `call_kept`: the embedder keeps a function and calls it by handle (the
//! worker host keeps the prelude's entry points this way).

use rustkit_js::{JsRuntime, JsValue};

/// A handle belongs to the runtime that issued it: another runtime refuses it instead of calling its own
/// function at the same index (R1 LOW on #1105).
#[test]
fn a_handle_from_another_runtime_is_refused() {
    let mut a = JsRuntime::new().unwrap();
    let mut b = JsRuntime::new().unwrap();
    let from_a = a.keep_function("(function () { return 'a'; })").unwrap();
    let from_b = b.keep_function("(function () { return 'b'; })").unwrap();
    assert!(matches!(a.call_kept(from_a, &[]), Ok(JsValue::String(ref s)) if s == "a"));
    assert!(matches!(b.call_kept(from_b, &[]), Ok(JsValue::String(ref s)) if s == "b"));
    assert!(b.call_kept(from_a, &[]).is_err(), "runtime b called a's handle");
}

/// A kept function outlives the global it was read from and a collection: the runtime holds it (R1 LOW on
/// #1105). A guard; the handles are held outside the collected heap by construction.
#[test]
fn a_kept_function_survives_its_global_and_a_collection() {
    let mut rt = JsRuntime::new().unwrap();
    rt.evaluate_script("globalThis.__k = (function () { var o = { n: 41 }; return function (x) { return ++o.n + ':' + x; }; })();")
        .unwrap();
    let kept = rt.keep_function("__k").unwrap();
    rt.evaluate_script("delete globalThis.__k; var junk = []; for (var i = 0; i < 20000; i++) junk.push({ i: i }); junk = null;")
        .unwrap();
    boa_gc::force_collect();
    boa_gc::force_collect();
    let got = rt.call_kept(kept, &[JsValue::String("x".into())]);
    assert!(matches!(got, Ok(JsValue::String(ref s)) if s == "42:x"), "{got:?}");
}
