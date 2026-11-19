//! JS-PERF2 item 2: `a[i]` and `a[i] = v` on an array, a typed array or a string skip the clone and
//! key conversion of the general path. The shortcut must answer what the general path answers, so
//! the cases below (holes, prototype index setters and getters, frozen and non-extensible arrays,
//! accessor and non-writable elements, subclasses, typed-array conversions, detached and resized
//! buffers, differing receivers, proxies, `arguments`) are compared with Chrome for Testing
//! 148.0.7778.216 (`tools/parity_oracle/js_cases_log.mjs index_fast_path_cases.json --write`).
//! They pass before the shortcut too. The deterministic pin that does not is
//! `index_fast_paths_allocs.rs` (whole-double keys); the timing evidence is in the PR body, not a test.

use rustkit_js::{JsRuntime, JsValue};

#[test]
fn the_index_shortcuts_answer_what_chrome_answers() {
    let data: serde_json::Value =
        serde_json::from_str(include_str!("../../../tools/parity_oracle/index_fast_path_cases.json")).expect("cases");
    let mut wrong = Vec::new();
    for c in data["cases"].as_array().expect("cases") {
        let name = c["name"].as_str().unwrap();
        let chrome = c["chrome"].as_str().expect("run js_cases_log.mjs --write");
        // One realm per case: a case may change a prototype.
        let mut rt = JsRuntime::new().unwrap();
        let got = match rt.evaluate_script(c["js"].as_str().unwrap()) {
            Ok(JsValue::String(s)) => s,
            other => format!("{other:?}"),
        };
        if got != chrome {
            wrong.push(format!("{name}:\n  rustkit {got}\n  chrome  {chrome}"));
        }
    }
    assert!(wrong.is_empty(), "{} differ\n{}", wrong.len(), wrong.join("\n"));
}
