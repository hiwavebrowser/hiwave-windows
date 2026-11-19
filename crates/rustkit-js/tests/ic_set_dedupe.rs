//! An inline cache site holds at most four shapes before it gives up (megamorphic) and misses on
//! every access. A getter that runs `f(o)` again on the same site (a recursive getter chain) makes
//! each level fill the same shape on the way out, and `InlineCache::set` pushed an entry each
//! time, so five levels of recursion filled the cache with copies of one shape and the site
//! missed on every later read, for every receiver. The count that shows it is the vm-stats miss
//! count of the site, which is switched on for this process (this file holds a single test for
//! that reason: the counters are per process).

use rustkit_js::{JsRuntime, JsValue};

/// Misses recorded for the opcode `op` (`GetPropertyByName`, `SetPropertyByName`) in a vm-stats
/// report: the lines of its "ic misses" section (both the cold and the not-cacheable ones), not
/// the opcode table above it.
fn misses(report: &str, op: &str) -> u64 {
    report
        .lines()
        .skip_while(|l| !l.starts_with("ic misses ("))
        .take_while(|l| !l.starts_with("ic misses by"))
        .filter(|l| l.trim_start().starts_with(op))
        .filter_map(|l| l.split_whitespace().nth(1)?.parse::<u64>().ok())
        .sum()
}

#[test]
fn a_recursive_getter_does_not_fill_a_site_with_copies_of_one_shape() {
    // Read once, when the first counter is bumped; nothing has run in this process yet.
    unsafe_set_env();
    let mut rt = JsRuntime::new().unwrap();
    // The counters are cumulative for the process and runtime start-up fills some; count the
    // misses these scripts add, not the total.
    let before = rustkit_js::vm_stats_report();
    let (get0, set0) = (misses(&before, "GetPropertyByName"), misses(&before, "SetPropertyByName"));
    // One read site (`f`); depth levels of recursion through an accessor, then 2000 reads of
    // each of two shapes that the site should now answer from its cache.
    let script = "(function(){ function f(o){ return o.v }\
        function mk(n){ var o = {}; Object.defineProperty(o, 'v', { get: function(){ return n > 0 ? f(mk(n - 1)) : 0 }, configurable: true }); return o }\
        f(mk(8)); var s = 0; for (var i = 0; i < 2000; i++) s += f(mk(0)) + f({ v: i }); return String(s) })()";
    let got = match rt.evaluate_script(script) {
        Ok(JsValue::String(s)) => s,
        other => format!("{other:?}"),
    };
    assert_eq!(got, "1999000");
    // The same for a setter: levels of recursion through an accessor's setter, then 2000 writes
    // of each of two shapes.
    let script = "(function(){ function g(o, v){ o.w = v }\
        function mk(n){ var o = {}; Object.defineProperty(o, 'w', { set: function(v){ if (n > 0) g(mk(n - 1), v); else hits++ }, configurable: true }); return o }\
        var hits = 0; g(mk(8), 1); var d = { w: 0 }; for (var i = 0; i < 2000; i++) { g(mk(0), i); g(d, i) } return String(hits) + ',' + d.w })()";
    let got = match rt.evaluate_script(script) {
        Ok(JsValue::String(s)) => s,
        other => format!("{other:?}"),
    };
    assert_eq!(got, "2001,1999");
    let report = rustkit_js::vm_stats_report();
    let get = misses(&report, "GetPropertyByName").saturating_sub(get0);
    let set = misses(&report, "SetPropertyByName").saturating_sub(set0);
    assert!(
        get < 200 && set < 200,
        "{get} read misses for 4000 reads and {set} write misses for 4000 writes, each of two shapes\n{report}"
    );
}

#[allow(unused_unsafe)]
fn unsafe_set_env() {
    unsafe { std::env::set_var("RUSTKIT_VM_STATS", "1") };
}
