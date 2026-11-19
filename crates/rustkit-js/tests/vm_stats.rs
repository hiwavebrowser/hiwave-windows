//! JS-PERF2 item 4: with `RUSTKIT_VM_STATS=1` the interpreter counts the opcodes it runs and the
//! inline-cache misses by site, receiver kind and property name, for the Mac CodeMirror probe to
//! say where property lookups go slow. Off (the default) it costs nothing and reports nothing.
//! The flag is read once per process, so this is the only test in its binary and sets it first.

use rustkit_js::{vm_stats_report, JsRuntime};

#[test]
fn the_counters_report_opcodes_and_inline_cache_misses() {
    assert_eq!(std::env::var_os("RUSTKIT_VM_STATS"), None, "run this test without the flag in the environment");
    // SAFETY: the only test in this binary; nothing else reads the environment concurrently.
    unsafe { std::env::set_var("RUSTKIT_VM_STATS", "1") };
    let mut rt = JsRuntime::new().unwrap();
    rt.evaluate_script(
        "var o = { a: 1, b: 2 }; var s = 0;
         for (var i = 0; i < 100; i++) { s += o.a + o.b; }
         var str = 'abc'; var n = 0;
         for (var j = 0; j < 50; j++) { n += str.charCodeAt(0); }
         var objs = []; for (var k = 0; k < 20; k++) { var x = {}; x['p' + k] = k; x.q = k; objs.push(x); }
         for (var m = 0; m < 20; m++) { s += objs[m].q; }",
    )
    .unwrap();
    // Privacy: the report carries shapes, never data. A string-literal key such as an email, and a
    // computed key built from page data, must not be recorded; an identifier from the source is.
    rt.evaluate_script(
        "var users = {}; var who = 'alice' + '@example.com';
         for (var u = 0; u < 5; u++) { users['bob@example.com'] = u; users[who] = u; var seen = users['bob@example.com'] + users[who]; }
         var tok = { secret_looking_value_1234567890123456789012345678901234567890123456789012345678901234567890: 1 };
         var rd = tok.secret_looking_value_1234567890123456789012345678901234567890123456789012345678901234567890;",
    )
    .unwrap();
    let report = vm_stats_report();
    println!("{report}");
    assert!(!report.contains("example.com"), "a key written as a string literal or built from data was recorded:\n{report}");
    assert!(!report.contains("secret_looking_value"), "an over-long name was recorded:\n{report}");
    assert!(report.contains("(not an identifier"), "non-identifier names should be bucketed, not dropped silently:\n{report}");
    assert!(report.contains("GetPropertyByName"), "no opcode counts:\n{report}");
    assert!(report.contains("ic misses"), "no inline-cache section:\n{report}");
    // The shape of `objs[m]` differs on every iteration, so `q` misses the cache there; `a` and `b`
    // on one object shape miss once each and then hit.
    assert!(report.contains("q"), "the polymorphic property is not named:\n{report}");
    assert!(report.contains("string"), "the primitive receiver kind is not reported:\n{report}");
}
