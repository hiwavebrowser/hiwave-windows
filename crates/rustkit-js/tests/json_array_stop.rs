//! JSON.stringify of a long array is a long native loop, and an array's length is set by the page (up to
//! 2^28 elements still pass the string-length pre-check). The turn budget must be able to stop it.
//!
//! No clock: the array's first element is an accessor whose getter sets the interrupt word. The other
//! elements are plain numbers, so nothing after the getter calls into script (where the existing checks
//! run): only a check in the loop itself can stop the serialization.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use boa_engine::error::{EngineError, RuntimeLimitError};
use boa_engine::{js_string, Context, JsValue, NativeFunction, Source};

const ALL: u64 = u64::MAX;

#[test]
fn json_stringify_of_a_long_array_is_stopped_by_the_turn_budget() {
    let word = Arc::new(AtomicU64::new(0));
    let mut context = Context::default();
    context.set_interrupt_word(Some(word.clone()));
    context.set_interrupt_turn(ALL);
    let stop = {
        let word = word.clone();
        // SAFETY: the closure captures an `Arc<AtomicU64>`, which holds nothing the collector traces.
        unsafe {
            NativeFunction::from_closure(move |_, _, _| {
                word.fetch_max(ALL, Ordering::Relaxed);
                Ok(JsValue::undefined())
            })
        }
    };
    context.register_global_callable(js_string!("stopNow"), 0, stop).unwrap();
    context
        .eval(Source::from_bytes(
            "var a = []; for (var i = 1; i < 200; i++) a[i] = i;
             Object.defineProperty(a, 0, { get: function () { stopNow(); return 0; } });",
        ))
        .unwrap();
    let result = context.eval(Source::from_bytes("JSON.stringify(a).length"));
    let err = result.expect_err("the serialization is stopped");
    assert!(
        matches!(err.as_engine(), Some(EngineError::RuntimeLimit(RuntimeLimitError::Interrupted))),
        "stopped by the interrupt, got {err}"
    );
}
