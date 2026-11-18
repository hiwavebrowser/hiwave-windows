//! Emptying an array with `q.length = 0` and reusing it (push, shift, index writes, holes, closures,
//! a request-queue pump) gives Chrome's results. Atlas reported engine-side JS on develop e8ed58da
//! losing a pushed element after the second reset. Expected values: Chrome for Testing 148.0.7778.216
//! (`tools/parity_oracle/array_length_reset_cases.json`, `js_cases_log.mjs --write`).

use rustkit_js::{JsRuntime, JsValue};

#[test]
fn reusing_an_array_after_length_zero_matches_chrome() {
    let data: serde_json::Value =
        serde_json::from_str(include_str!("../../../tools/parity_oracle/array_length_reset_cases.json")).expect("cases");
    let cases = data["cases"].as_array().expect("cases");
    let mut wrong = Vec::new();
    for c in cases {
        let name = c["name"].as_str().unwrap();
        let chrome = c["chrome"].as_str().expect("run js_cases_log.mjs --write");
        let mut rt = JsRuntime::new().unwrap();
        let got = match rt.evaluate_script(c["js"].as_str().unwrap()) {
            Ok(JsValue::String(s)) => s,
            other => format!("{other:?}"),
        };
        if got != chrome {
            wrong.push(format!("{name}:\n  rustkit {got}\n  chrome  {chrome}"));
        }
    }
    assert!(wrong.is_empty(), "{} of {} differ\n{}", wrong.len(), cases.len(), wrong.join("\n"));
}
