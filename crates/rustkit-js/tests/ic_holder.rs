//! A property inline cache must not outlive the property it cached. A site that read or wrote a
//! property found on the receiver's prototype keeps answering from that slot after the prototype's
//! property is deleted or redefined (accessor <-> data, a getter or setter swapped, made read-only),
//! and an object in dictionary mode (more than 1024 property transitions, and the built-in
//! prototypes) changes some attributes, or gains an own property, without changing its shape at
//! all, so a site keyed on that shape still hits after `Object.freeze`, `writable: false`, or an own
//! property that shadows the prototype's. Expected values: Chrome for Testing 148.0.7778.216
//! (`tools/parity_oracle/ic_holder_cases.json`, `js_cases_log.mjs --write`). Each case warms one
//! site at least twice before the change (the second call is the cache hit). `s_args*`,
//! `s_strict_nosetter`, the swap cases and the `u_global*` cases already matched; they pin paths a
//! fix touches. The `r_*` cases have a getter or setter change the property while the site is
//! being filled, including the memoising idiom where a prototype accessor defines the name on the
//! receiver itself. Each case runs on its own runtime and its own thread, so one that panics is
//! reported by name instead of ending the run.

use rustkit_js::{JsRuntime, JsValue};

#[test]
fn a_cached_property_site_sees_the_property_change_like_chrome() {
    let data: serde_json::Value =
        serde_json::from_str(include_str!("../../../tools/parity_oracle/ic_holder_cases.json")).expect("cases");
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
