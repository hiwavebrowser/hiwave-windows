//! `new EventTarget()` and classes that extend it (DOM §2.7), against what
//! the pinned Chromium answers for the same scripts
//! (tools/parity_oracle/event_target_ctor_cases.json, written by
//! event_target_ctor_log.mjs). Its own test module so it does not collide
//! with the other families' tests in `lib.rs`.

use super::*;

const PAGE: &str = "<!DOCTYPE html><html><head><title>T</title></head><body></body></html>";

/// One case's script in a fresh page, its completion value as a string, or
/// the exception it ended in.
fn answer(js: &str) -> String {
    let bindings = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    bindings.set_document(Rc::new(Document::parse_html(PAGE).unwrap())).unwrap();
    match bindings.evaluate(js) {
        Ok(JsValue::String(s)) => s,
        Ok(JsValue::Boolean(b)) => b.to_string(),
        Ok(JsValue::Number(n)) if n.fract() == 0.0 => format!("{}", n as i64),
        Ok(JsValue::Number(n)) => n.to_string(),
        Ok(JsValue::Null) => "null".to_string(),
        Ok(JsValue::Undefined) => "undefined".to_string(),
        Ok(other) => format!("{other:?}"),
        Err(e) => format!("THREW {e}"),
    }
}

#[test]
fn event_target_is_constructible_as_in_chrome() {
    let data: serde_json::Value =
        serde_json::from_str(include_str!("../../../tools/parity_oracle/event_target_ctor_cases.json")).unwrap();
    let cases = data["cases"].as_array().unwrap();
    assert!(cases.len() >= 17, "the case file lost cases: {}", cases.len());
    let (mut wrong, mut gaps) = (Vec::new(), 0);
    for case in cases {
        let (name, js) = (case["name"].as_str().unwrap(), case["js"].as_str().unwrap());
        let chrome = case["chrome"].as_str().expect("run event_target_ctor_log.mjs --write");
        // A case with a `gap` is a known difference: `engine` is what the
        // engine answers, pinned so the gap closes on purpose.
        let expected = match case.get("gap") {
            Some(_) => {
                gaps += 1;
                case["engine"].as_str().expect("a gap names the engine's answer")
            }
            None => chrome,
        };
        let ours = answer(js);
        if ours != expected {
            wrong.push(format!("{name}\n    chrome: {chrome}\n    ours:   {ours}"));
        }
    }
    assert!(wrong.is_empty(), "{} of {} cases differ from Chrome:\n{}", wrong.len(), cases.len(), wrong.join("\n"));
    assert!(gaps <= 2, "the known differences grew to {gaps}");
}
