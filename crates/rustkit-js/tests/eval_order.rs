//! Evaluation order of local variables held in registers. The compiler reads a local's register
//! directly as an operand (no copy into a temporary), which is only right when nothing evaluated
//! before the operand is USED can assign to it: `a + --a` and `a < (a = 5)` must use the OLD `a`
//! (the left side is read first), and the old compiler used the new one.
//!
//! 1,200 generated programs full of exactly those hazards (`tools/parity_oracle/gen_eval_order_cases.py`,
//! seeded) are compared with Chrome for Testing 148.0.7778.216 (`js_cases_log.mjs eval_order_cases.json
//! --write`): locals declared with `var` and `let`, a parameter, objects and arrays, side-effecting
//! keys, compound assignment, `++`/`--`, a closure that mutates a captured variable, calls,
//! literals, chained assignment, `var` declarations with expressions, member stores of locals.
//! Four of them differed on develop before #1124 and #1129. A second file of 56
//! hand-written scoping cases (`compiled_locals_cases.json`: mapped `arguments`, named function
//! expression names, `const`, the temporal dead zone, generators, `eval`, `with`, every compound
//! operator, throwing initializers, frozen and setter targets) guards the same area, including
//! JS-PERF2 item 6's in-place compilation.

use rustkit_js::{JsRuntime, JsValue};

#[test]
fn generated_evaluation_order_programs_match_chrome() {
    let data: serde_json::Value =
        serde_json::from_str(include_str!("../../../tools/parity_oracle/eval_order_cases.json")).expect("cases");
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
            wrong.push(format!("{name}:\n  js      {}\n  rustkit {got}\n  chrome  {chrome}", c["js"].as_str().unwrap().replace('\n', " ")));
        }
    }
    assert!(wrong.is_empty(), "{} of {} differ\n{}", wrong.len(), cases.len(), wrong.iter().take(4).cloned().collect::<Vec<_>>().join("\n"));
}

/// Hand-written scoping hazards for the same change (`tools/parity_oracle/compiled_locals_cases.json`):
/// mapped and unmapped `arguments`, the name of a named function expression, `const`, the temporal
/// dead zone, closures, generators, `eval`, `with`, destructured parameters, every compound
/// operator, mixed types, and members whose object or key is reassigned.
#[test]
fn scoping_hazards_for_register_locals_match_chrome() {
    let data: serde_json::Value =
        serde_json::from_str(include_str!("../../../tools/parity_oracle/compiled_locals_cases.json")).expect("cases");
    let cases = data["cases"].as_array().expect("cases");
    let mut wrong = Vec::new();
    for c in cases {
        let name = c["name"].as_str().unwrap();
        let chrome = c["chrome"].as_str().expect("run js_cases_log.mjs --write");
        let mut rt = JsRuntime::new().unwrap();
        let js = c["js"].as_str().unwrap();
        // An `async-` case is an expression for a promise: Chrome's harness awaits it; here the
        // value is kept in a global and read by a second script, after the jobs have run.
        let got = if name.starts_with("async-") {
            let wrapped = format!("globalThis.__out = 'pending'; ({js}).then(function (v) {{ globalThis.__out = String(v); }}, function (e) {{ globalThis.__out = 'rejected ' + e; }}); 0");
            let _ = rt.evaluate_script(&wrapped);
            match rt.evaluate_script("String(globalThis.__out)") {
                Ok(JsValue::String(s)) => s,
                other => format!("{other:?}"),
            }
        } else {
            match rt.evaluate_script(js) {
                Ok(JsValue::String(s)) => s,
                other => format!("{other:?}"),
            }
        };
        if got != chrome {
            wrong.push(format!("{name}:\n  rustkit {got}\n  chrome  {chrome}"));
        }
    }
    assert!(wrong.is_empty(), "{} of {} differ\n{}", wrong.len(), cases.len(), wrong.join("\n"));
}
