//! A plain assignment to a `let` or `const` before its declaration has run (the temporal dead
//! zone) is a ReferenceError, raised after the right side has been evaluated. Boa threw for a read,
//! a compound assignment and `++` (they read first), but a plain `z = 1` compiled before `let z`
//! silently gave `z` a fresh register of its own and succeeded. Expected values: Chrome for Testing
//! 148.0.7778.216 (`tools/parity_oracle/tdz_assign_cases.json`, `js_cases_log.mjs --write`).

use rustkit_js::{JsRuntime, JsValue};

#[test]
fn assignments_in_the_temporal_dead_zone_match_chrome() {
    let data: serde_json::Value =
        serde_json::from_str(include_str!("../../../tools/parity_oracle/tdz_assign_cases.json")).expect("cases");
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
