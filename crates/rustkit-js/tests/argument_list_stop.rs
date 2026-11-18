//! Building a list from an array-like reads up to 2^27 elements in a native loop (`apply`, `Reflect.apply`,
//! `Reflect.construct`, a Proxy's `ownKeys` result). The turn budget must be able to stop it.
//!
//! No clock: element 0's getter sets the interrupt word; the other elements are missing, so nothing after
//! the getter calls into script (where the existing checks run): only a check in the loop can stop it.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use boa_engine::error::{EngineError, RuntimeLimitError};
use boa_engine::{js_string, Context, JsValue, NativeFunction, Source};

const ALL: u64 = u64::MAX;

fn stopped(src: &str) {
    let word = Arc::new(AtomicU64::new(0));
    let mut context = Context::default();
    context.set_interrupt_word(Some(word.clone()));
    context.set_interrupt_turn(ALL);
    // SAFETY: the closure captures an `Arc<AtomicU64>`, which holds nothing the collector traces.
    let stop = unsafe {
        NativeFunction::from_closure(move |_, _, _| {
            word.fetch_max(ALL, Ordering::Relaxed);
            Ok(JsValue::undefined())
        })
    };
    context.register_global_callable(js_string!("stopNow"), 0, stop).unwrap();
    let err = context.eval(Source::from_bytes(src)).expect_err(src);
    assert!(
        matches!(err.as_engine(), Some(EngineError::RuntimeLimit(RuntimeLimitError::Interrupted))),
        "{src}: stopped by the interrupt, got {err}"
    );
}

#[test]
fn the_turn_budget_stops_a_long_argument_list() {
    let o = "var o = { length: 2 ** 20, get 0() { stopNow(); return 0; } };";
    stopped(&format!("{o} (function () {{}}).apply(null, o);"));
    stopped(&format!("{o} Reflect.apply(function () {{}}, null, o);"));
    stopped(&format!("{o} Reflect.construct(function () {{}}, o);"));
}

#[test]
fn the_turn_budget_stops_a_long_own_keys_list() {
    stopped(
        "var keys = { length: 2 ** 20, get 0() { stopNow(); return 'a'; } };
         Reflect.ownKeys(new Proxy({}, { ownKeys() { return keys; } }));",
    );
}
