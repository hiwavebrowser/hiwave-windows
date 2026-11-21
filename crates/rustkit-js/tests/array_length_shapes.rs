//! Follow-up to `array_length_reset.rs` (R1 on #1157): a `length` write must take the array's own
//! path whatever else used the same site first — an ordinary object built with an array's shape
//! (`Object.create(Q.prototype)` + a writable `length`, `Object.create([])`), an Array subclass —
//! and through `++`, `+=`, frozen, sealed and non-writable `length`, a non-configurable element, and
//! a `length` given as an object whose conversion touches the array. Expected values: Chrome for
//! Testing 148.0.7778.216 (`tools/parity_oracle/array_length_shapes_cases.json`,
//! `js_cases_log.mjs --write`). Each case runs on its own runtime and on its own thread, so one that
//! panics is reported by name instead of ending the run.

use rustkit_js::{JsRuntime, JsValue};

#[test]
fn length_writes_from_a_shared_site_match_chrome() {
    let data: serde_json::Value =
        serde_json::from_str(include_str!("../../../tools/parity_oracle/array_length_shapes_cases.json")).expect("cases");
    let cases = data["cases"].as_array().expect("cases");
    let mut wrong = Vec::new();
    for c in cases {
        let name = c["name"].as_str().unwrap().to_owned();
        let chrome = c["chrome"].as_str().expect("run js_cases_log.mjs --write").to_owned();
        let js = c["js"].as_str().unwrap().to_owned();
        let got = std::thread::spawn(move || {
            let mut rt = JsRuntime::new().unwrap();
            match rt.evaluate_script(&js) {
                Ok(JsValue::String(s)) => s,
                other => format!("{other:?}"),
            }
        })
        .join()
        .unwrap_or_else(|_| "<panicked>".to_owned());
        if got != chrome {
            wrong.push(format!("{name}:\n  rustkit {got}\n  chrome  {chrome}"));
        }
    }
    assert!(wrong.is_empty(), "{} of {} differ\n{}", wrong.len(), cases.len(), wrong.join("\n"));
}
