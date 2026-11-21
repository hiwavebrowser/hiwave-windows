//! A property read that hits its inline cache must still be the read that [[Get]] would do. The
//! cache is keyed on shape, and an exotic object can share an ordinary object's shape: a Proxy made
//! from `{}` has the empty shape with `Object.prototype`, and a typed array has the empty shape with
//! its prototype. A site warmed on ordinary objects then answered from the prototype's slot for a
//! Proxy (skipping its `get` trap) and for a typed array reading a canonical numeric name such as
//! `Infinity` or `NaN` (which [[Get]] on a typed array never looks up on the prototype). Expected
//! values: Chrome for Testing 148.0.7778.216 (`tools/parity_oracle/ic_exotic_get_cases.json`,
//! `js_cases_log.mjs --write`). Twelve of the 17 cases already matched on develop; they pin what must
//! keep working: ordinary hits, a typed array and a mapped `arguments` object reading a
//! non-numeric name, a Proxy's `length` through an object with its own `length`, and the fill side (a Proxy with no `get` trap, or only a `set` trap, reads
//! through to its target, and the entry made for it must not be taken by an ordinary object that
//! shares its shape: a Proxy's [[Get]] marks its slot not cacheable). Each case runs on its
//! own runtime and its own thread, so one that panics is reported by name.

use rustkit_js::{JsRuntime, JsValue};

#[test]
fn a_cached_read_does_what_get_would_on_an_exotic_receiver_like_chrome() {
    let data: serde_json::Value =
        serde_json::from_str(include_str!("../../../tools/parity_oracle/ic_exotic_get_cases.json")).expect("cases");
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
