//! `x++` / `x--` / `++x` / `--x` convert their operand with ToNumeric: a postfix used as a VALUE yields
//! the converted old value (`'5'++` is 5, `undefined++` is NaN), and a conversion that throws leaves
//! the variable as it was. Two Boa bugs, from an independent R1 on #1149: the in-place local fast
//! path emitted `Move(dst, local); Inc(local, local)`, so the value was the UNconverted old one; and
//! `Inc`/`Dec` took the source register before converting and did not put it back when the
//! conversion threw, so the variable became `undefined`. Expected values: Chrome for Testing
//! 148.0.7778.216 (`tools/parity_oracle/update_conversion_cases.json`, `js_cases_log.mjs --write`),
//! for `let`, `var`, a parameter, a global and a member.

use rustkit_js::{JsRuntime, JsValue};

#[test]
fn update_operators_convert_and_survive_a_throwing_conversion_as_in_chrome() {
    let data: serde_json::Value =
        serde_json::from_str(include_str!("../../../tools/parity_oracle/update_conversion_cases.json")).expect("cases");
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
