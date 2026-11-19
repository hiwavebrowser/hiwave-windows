//! A `let`, `const` or `class` declared in one `switch` case is in its temporal dead zone in a case
//! that is reached WITHOUT running the declaration (a jump to a later case). Boa kept such bindings in
//! registers, which have no run-time dead zone: compilation follows source order, so a later case's
//! access was compiled as initialised, and reading it returned whatever the reused register last held
//! (in the pinned case, another local's value). Chrome throws ReferenceError. Expected values: Chrome
//! for Testing 148.0.7778.216 (`tools/parity_oracle/switch_tdz_cases.json`, `js_cases_log.mjs --write`),
//! including the paths that must keep working (fallthrough, same-case access, hoisted functions,
//! closures, a fresh binding per loop iteration).

use rustkit_js::{JsRuntime, JsValue};

#[test]
fn switch_case_lexical_bindings_have_a_dead_zone_as_in_chrome() {
    let data: serde_json::Value =
        serde_json::from_str(include_str!("../../../tools/parity_oracle/switch_tdz_cases.json")).expect("cases");
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
