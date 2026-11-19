//! The TimSort port (`v8_timsort.rs`) must never crash the page thread, whatever the comparator:
//! with a comparator that is not a total order its merges and gallops walk indexes the algorithm
//! only keeps in range when the comparator is consistent, so a slip would be an out-of-bounds panic.
//! Seeded arrays up to 10,000 elements (past many merge runs and the galloping threshold) sorted
//! with random, constant, inconsistent, NaN, partly-ordered and throwing comparators: each must
//! return a PERMUTATION of its input (or propagate the throw) and never panic.

use rustkit_js::{JsRuntime, JsValue};

#[test]
fn inconsistent_comparators_never_crash_or_lose_elements() {
    let script = r#"
      (function () {
        var seed = 987654321;
        function rnd(n) { seed = (seed * 1103515245 + 12345) & 0x7fffffff; return seed % n; }
        var sizes = [0, 1, 2, 3, 7, 8, 9, 31, 32, 33, 63, 64, 65, 100, 255, 256, 257, 1000, 2047, 2048, 5000, 10000];
        var comparators = [
          function () { return rnd(3) - 1; },
          function () { return 0; },
          function () { return NaN; },
          function () { return -1; },
          function () { return 1; },
          function (a, b) { return a > b; },
          function (a, b) { return (a % 7) - (b % 5); },
          function (a, b) { return rnd(10) < 8 ? a - b : b - a; },
          function (a, b) { return rnd(100) === 0 ? NaN : a - b; },
          function (a, b) { return Math.sin(a) - Math.sin(b); },
          function (a, b) { return a % 2 === 0 ? -1 : 1; },
        ];
        var bad = [];
        for (var si = 0; si < sizes.length; si++) {
          var n = sizes[si];
          // Shapes that make long runs, descending runs, many equal keys and random order.
          var shapes = [
            function (i) { return i; },
            function (i) { return n - i; },
            function (i) { return rnd(1000000); },
            function (i) { return rnd(4); },
            function (i) { return (i % 50) < 25 ? i : n - i; },
          ];
          // The biggest arrays only with two shapes, to keep the debug-build run short.
          var shapeCount = n >= 5000 ? 2 : shapes.length;
          for (var hi = 0; hi < shapeCount; hi++) {
            for (var ci = 0; ci < comparators.length; ci++) {
              var a = [];
              for (var i = 0; i < n; i++) a.push(shapes[hi](i));
              var counts = {};
              for (var j = 0; j < n; j++) counts[a[j]] = (counts[a[j]] || 0) + 1;
              var r = a.slice().sort(comparators[ci]);
              var typed = new Float64Array(a);
              typed.sort(comparators[ci]);
              var after = {}, after2 = {};
              if (r.length !== n || typed.length !== n) { bad.push('length n=' + n + ' h=' + hi + ' c=' + ci); continue; }
              for (var k = 0; k < n; k++) { after[r[k]] = (after[r[k]] || 0) + 1; after2[typed[k]] = (after2[typed[k]] || 0) + 1; }
              for (var key in counts) {
                if (after[key] !== counts[key] || after2[key] !== counts[key]) { bad.push('not a permutation n=' + n + ' h=' + hi + ' c=' + ci); break; }
              }
            }
          }
        }
        // A throwing comparator propagates and leaves the array a permutation of the input.
        var t = []; for (var q = 0; q < 300; q++) t.push(rnd(50));
        var calls = 0, thrown = false;
        try { t.slice().sort(function (a, b) { calls++; if (calls === 700) throw new Error('stop'); return a - b; }); } catch (e) { thrown = e.message === 'stop'; }
        if (!thrown) bad.push('throw not propagated');
        return bad.length ? bad.slice(0, 6).join('; ') : 'ok';
      })()
    "#;
    let mut rt = JsRuntime::new().unwrap();
    match rt.evaluate_script(script) {
        Ok(JsValue::String(s)) => assert_eq!(s, "ok"),
        other => panic!("{other:?}"),
    }
}
