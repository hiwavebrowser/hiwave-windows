//! `Array.prototype.sort` with a comparator that is not a consistent total order (`a > b`, NaN,
//! `-1` always, a mixed bag) leaves the order to the implementation, and pages see V8's.
//! observable-plot keeps a different top-N when the order differs. Each case in
//! `tools/parity_oracle/sort_nan_cases.json` is an expression returning the sorted values and the
//! comparator's call log (`x,y` pairs, in call order); `chrome` is what Chrome for Testing
//! 148.0.7778.216 returns (`node sort_cases_log.mjs --write`). Sizes 2..=63 are V8's binary
//! insertion path; 100 and up take its full TimSort with merges and galloping.

use rustkit_js::{JsRuntime, JsValue};

#[test]
fn sorting_with_an_inconsistent_comparator_matches_chrome_call_for_call() {
    let data: serde_json::Value =
        serde_json::from_str(include_str!("../../../tools/parity_oracle/sort_nan_cases.json")).expect("cases");
    let mut rt = JsRuntime::new().unwrap();
    let mut wrong = Vec::new();
    let cases = data["cases"].as_array().expect("cases");
    for c in cases {
        let name = c["name"].as_str().unwrap();
        let chrome = c["chrome"].as_str().expect("run sort_cases_log.mjs --write");
        let got = match rt.evaluate_script(c["js"].as_str().unwrap()) {
            Ok(JsValue::String(s)) => s,
            other => format!("{other:?}"),
        };
        if got != chrome {
            let cut = |s: &str| s.chars().take(400).collect::<String>();
            wrong.push(format!("{name}:\n  rustkit {}\n  chrome  {}", cut(&got), cut(chrome)));
        }
    }
    assert!(wrong.is_empty(), "{} of {} differ\n{}", wrong.len(), cases.len(), wrong.join("\n"));
}

/// Whatever the comparator, `sort` returns a permutation and never panics; with a consistent one
/// (ties included) the result is the unique stable order. Seeded, so every run is the same.
#[test]
fn sort_is_stable_for_consistent_comparators_and_total_for_any_other() {
    let script = r#"
      (function () {
        var seed = 12345;
        function rnd(n) { seed = (seed * 1103515245 + 12345) & 0x7fffffff; return seed % n; }
        var bad = [];
        for (var round = 0; round < 400; round++) {
          var n = round < 300 ? rnd(40) : 40 + rnd(400);
          var keys = rnd(6) + 1;
          var a = [];
          for (var i = 0; i < n; i++) a.push({ k: rnd(keys), i: i });
          var expected = a.slice().sort(function (x, y) { return x.k - y.k || x.i - y.i; });
          // Stable: ties keep their input order, so sorting by k alone gives the same as with the tiebreak.
          var got = a.slice().sort(function (x, y) { return x.k - y.k; });
          for (var j = 0; j < n; j++) if (got[j] !== expected[j]) { bad.push('stable n=' + n + ' round=' + round); break; }
          var ts = a.toSorted(function (x, y) { return x.k - y.k; });
          for (var j = 0; j < n; j++) if (ts[j] !== expected[j]) { bad.push('toSorted n=' + n); break; }
          // Any comparator: a permutation, no throw.
          var junk = a.slice().sort(function () { return rnd(3) - 1; });
          var seen = {};
          for (var j = 0; j < junk.length; j++) seen[junk[j].i] = 1;
          if (junk.length !== n || Object.keys(seen).length !== n) bad.push('not a permutation n=' + n);
          var typed = new Int32Array(a.map(function (x) { return x.k * 1000 + x.i; }));
          typed.sort(function (x, y) { return rnd(3) - 1; });
          var tseen = {}; for (var j = 0; j < typed.length; j++) tseen[typed[j]] = 1;
          if (Object.keys(tseen).length !== n) bad.push('typed not a permutation n=' + n);
          var tc = new Int32Array(a.map(function (x) { return x.k * 1000 + x.i; }));
          tc.sort(function (x, y) { return (x / 1000 | 0) - (y / 1000 | 0); });
          for (var j = 1; j < tc.length; j++) if ((tc[j - 1] / 1000 | 0) > (tc[j] / 1000 | 0) || ((tc[j - 1] / 1000 | 0) === (tc[j] / 1000 | 0) && tc[j - 1] > tc[j])) { bad.push('typed unstable n=' + n); break; }
        }
        return bad.length ? bad.slice(0, 5).join('; ') : 'ok';
      })()
    "#;
    let mut rt = JsRuntime::new().unwrap();
    match rt.evaluate_script(script) {
        Ok(JsValue::String(s)) => assert_eq!(s, "ok"),
        other => panic!("{other:?}"),
    }
}
