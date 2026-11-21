//! SORT1: a numeric `a - b` sort makes no call between comparisons, so the interrupt checks that run at a
//! call's entry never run. The turn budget must still stop one (third_party/boa_engine/PATCHES.md).
//!
//! No clock: the array's last element is an accessor whose getter sets the interrupt word, so the word
//! is set while the sort reads its elements, before any pair is compared. (The last one: a sort that
//! was not stopped writes the other elements back before it reaches that element's setter.)

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use boa_engine::error::{EngineError, RuntimeLimitError};
use boa_engine::{js_string, Context, JsValue, NativeFunction, Source};

const ALL: u64 = u64::MAX;
const N: usize = 200;

/// Runs `sort` on a 200-element array `a` whose read sets the interrupt word. Answers the context and
/// the word, with the sort's result.
fn sort_with_the_word_set_while_reading(sort: &str) -> (Context, Arc<AtomicU64>, boa_engine::JsResult<JsValue>) {
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
    context
        .register_global_callable(js_string!("stopNow"), 0, stop)
        .unwrap();
    // Descending numbers, so an unsorted array is told from a sorted one by its first two elements.
    context
        .eval(Source::from_bytes(&format!(
            "var a = []; for (var i = 0; i < {N} - 1; i++) a[i] = {N} - i; var armed = true;
             Object.defineProperty(a, {N} - 1, {{ configurable: true, enumerable: true,
                 get: function () {{ if (armed) {{ armed = false; stopNow(); }} return 0; }},
                 set: function () {{}} }});"
        )))
        .unwrap();
    let result = context.eval(Source::from_bytes(sort));
    (context, word, result)
}

fn assert_interrupted(result: &boa_engine::JsResult<JsValue>) {
    let err = result.as_ref().expect_err("the sort is stopped");
    assert!(
        matches!(err.as_engine(), Some(EngineError::RuntimeLimit(RuntimeLimitError::Interrupted))),
        "stopped by the interrupt, got {err}"
    );
}

#[test]
fn a_numeric_difference_sort_is_stopped_by_the_turn_budget() {
    let (mut context, word, result) =
        sort_with_the_word_set_while_reading("a.sort(function (x, y) { return x - y; });");
    assert_interrupted(&result);
    // A later turn sees the array as it was: the stopped sort wrote nothing back.
    word.store(0, Ordering::Relaxed);
    let seen = context
        .eval(Source::from_bytes("a[0] + ',' + a[1] + ',' + a.length"))
        .unwrap();
    let seen = seen.to_string(&mut context).unwrap().to_std_string_escaped();
    assert_eq!(seen, format!("{N},{},{N}", N - 1));
}

#[test]
fn a_numeric_difference_to_sorted_is_stopped_by_the_turn_budget() {
    let (_context, _word, result) = sort_with_the_word_set_while_reading("a.toSorted((x, y) => x - y);");
    assert_interrupted(&result);
}
