//! A direct eval inside a class element (method, accessor, field, private field, static field, static
//! block) reads and writes the bindings of the scopes around the class, as any nested function's eval
//! does. Each case runs in its own context and must give Chrome's answer.
//!
//! Expected values are Chrome for Testing 148.0.7778.216's answers:
//! `docs/diagnostics/2026-11-17/class_element_eval_cases.json`, written by
//! `tools/parity_oracle/js_cases_log.mjs --write`.

use boa_engine::{Context, Source};
use std::panic::{AssertUnwindSafe, catch_unwind};

const CASES: &str = include_str!("../../../docs/diagnostics/2026-11-17/class_element_eval_cases.json");

/// The case's answer: its text run as a classic script (as a page's script runs; the oracle runs the same
/// global code through indirect eval), the completion value as a string, or the error's name.
fn run(js: &str) -> String {
    let mut ctx = Context::default();
    match ctx.eval(Source::from_bytes(js)) {
        Ok(v) => v.to_string(&mut ctx).unwrap().to_std_string_escaped(),
        Err(e) => format!("error: {e}"),
    }
}

#[test]
fn a_direct_eval_in_a_class_element_sees_the_enclosing_bindings_as_chrome_does() {
    let doc: serde_json::Value = serde_json::from_str(CASES).unwrap();
    assert_eq!(doc["chrome_version"].as_str(), Some("148.0.7778.216"));
    let cases = doc["cases"].as_array().unwrap();
    let mut wrong = Vec::new();
    for case in cases {
        let (name, js, want) =
            (case["name"].as_str().unwrap(), case["js"].as_str().unwrap(), case["chrome"].as_str().unwrap());
        let got = catch_unwind(AssertUnwindSafe(|| run(js))).unwrap_or_else(|_| "no answer".to_string());
        if got != want {
            wrong.push(format!("{name}: chrome {want:?}, here {got:?}"));
        }
    }
    assert!(wrong.is_empty(), "{} of {} differ:\n{}", wrong.len(), cases.len(), wrong.join("\n"));
}
