//! JS2: `Array.prototype.sort` with a comparator that is not a total order
//! must not panic. Boa's `sort_indexed_properties` handed the page's
//! comparator to `Vec::sort_by`, which since Rust 1.81 panics ("user-provided
//! comparison function does not correctly implement a total order") when it
//! notices an inconsistent answer. Real pages ship such comparators (a
//! boolean `a > b`, a constant, `Math.random() - 0.5`): walmart.com panicked
//! on 17 of 17 loads. ECMA-262 leaves the result implementation-defined for
//! them; it does not allow the engine to die.

#![cfg(feature = "headless")]

use super::*;

fn run(script: &str) -> String {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let id = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 200, height: 100 })
        .expect("view");
    engine.load_html(id, "<html><body></body></html>").expect("load_html");
    engine.execute_script(id, script).expect("script")
}

/// 200 numbers, enough to leave the small-slice insertion sort that never
/// notices an inconsistent comparator.
const ARRAY: &str = "var a = []; for (var i = 0; i < 200; i++) a.push((i * 7919) % 211);";

fn len_after(comparator: &str, method: &str) -> String {
    run(&format!(
        "{ARRAY} var r = a.{method}({comparator}); String(r.length + ':' + a.length)"
    ))
}

#[test]
fn a_boolean_comparator_does_not_panic() {
    assert_eq!(len_after("function (x, y) { return x > y; }", "sort"), "String(\"200:200\")");
}

#[test]
fn a_constant_comparator_does_not_panic() {
    assert_eq!(len_after("function () { return 1; }", "sort"), "String(\"200:200\")");
    assert_eq!(len_after("function () { return -1; }", "sort"), "String(\"200:200\")");
}

#[test]
fn a_random_comparator_does_not_panic() {
    assert_eq!(len_after("function () { return Math.random() - 0.5; }", "sort"), "String(\"200:200\")");
}

#[test]
fn to_sorted_and_typed_arrays_do_not_panic() {
    assert_eq!(len_after("function () { return 1; }", "toSorted"), "String(\"200:200\")");
    let typed = run(
        "var t = new Float64Array(200); for (var i = 0; i < 200; i++) t[i] = (i * 7919) % 211; \
         t.sort(function () { return -1; }); String(t.length)",
    );
    assert_eq!(typed, "String(\"200\")");
}

#[test]
fn a_consistent_comparator_still_sorts_stably() {
    let out = run(&format!(
        "{ARRAY} a.sort(function (x, y) {{ return x - y; }}); \
         var ok = true; for (var i = 1; i < a.length; i++) if (a[i - 1] > a[i]) ok = false; \
         var p = []; for (var j = 0; j < 100; j++) p.push({{ k: j % 5, i: j }}); \
         p.sort(function (x, y) {{ return x.k - y.k; }}); \
         var stable = true; for (var m = 1; m < p.length; m++) if (p[m - 1].k === p[m].k && p[m - 1].i > p[m].i) stable = false; \
         String(ok + ',' + stable)"
    ));
    assert_eq!(out, "String(\"true,true\")");
}

#[test]
fn a_throwing_comparator_stops_the_sort_rethrows_and_leaves_the_array_unchanged() {
    let out = run(&format!(
        "{ARRAY} var before = a.join(); var calls = 0; var msg = ''; \
         try {{ a.sort(function () {{ calls++; if (calls === 5) throw new Error('boom'); return 0; }}); }} catch (e) {{ msg = e.message; }} \
         String(msg + ':' + calls + ':' + (a.join() === before))"
    ));
    // No further comparator calls after the abrupt completion (ECMA-262 SortIndexedProperties),
    // and nothing is written back.
    assert_eq!(out, "String(\"boom:5:true\")");
}

#[test]
fn a_throw_in_the_middle_of_the_merge_leaves_the_array_unchanged() {
    // The pre-pass stops at the first out-of-order pair, so this unsorted array reaches the merge;
    // the throw is at call 400, well past the pre-pass and inside it (R1 on #1060).
    let out = run(&format!(
        "{ARRAY} var before = a.join(); var calls = 0; var msg = ''; \
         try {{ a.sort(function (x, y) {{ calls++; if (calls === 400) throw new Error('mid'); return x - y; }}); }} catch (e) {{ msg = e.message; }} \
         String(msg + ':' + calls + ':' + (a.join() === before))"
    ));
    assert_eq!(out, "String(\"mid:400:true\")");
}

/// Comparator calls, not wall clock: a flaky bound on a loaded machine proves nothing.
fn calls_for(setup: &str) -> u64 {
    let out = run(&format!(
        "{setup} var calls = 0; a.sort(function (x, y) {{ calls++; return x - y; }}); String(calls)"
    ));
    out.trim_start_matches("String(\"").trim_end_matches("\")").parse().unwrap()
}

#[test]
fn sorted_input_costs_n_minus_one_comparator_calls() {
    assert_eq!(calls_for("var a = []; for (var i = 0; i < 10000; i++) a.push(i);"), 9_999);
    // Equal neighbours are still "in order" (stable, nothing to move).
    assert_eq!(calls_for("var a = []; for (var i = 0; i < 1000; i++) a.push(i >> 2);"), 999);
}

#[test]
fn unsorted_input_stays_within_n_log_n_comparator_calls() {
    let n: u64 = 200_000;
    let calls = calls_for("var a = []; for (var i = 0; i < 200000; i++) a.push((i * 7919) % 100003);");
    // Merge sort: at most n * ceil(log2 n) - 2^ceil(log2 n) + 1 calls, plus the sorted pre-pass
    // (stopped at the first out-of-order pair).
    let bound = n * 18 + n;
    assert!(calls <= bound, "{calls} calls for {n} numbers (bound {bound})");
    let out = run("var a = []; for (var i = 0; i < 200000; i++) a.push((i * 7919) % 100003); \
                   a.sort(function (x, y) { return x - y; }); String(a[0] + ',' + a[199999])");
    assert_eq!(out, "String(\"0,100002\")");
}
