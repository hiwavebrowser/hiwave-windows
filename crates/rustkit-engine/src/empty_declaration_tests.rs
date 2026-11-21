//! Z plan CH2.E28: an empty declaration (`;;`) or one with no colon in a
//! style rule is skipped and the next declaration applies (CSS Syntax 3
//! §5.4.5). The sheet parser kept the second `;` as the start of the next
//! property name, so `a{color:red;;background:blue}` read the property
//! `;background` and dropped it. Minified sheets hit this.
//!
//! Expected values are Chromium's answers on the same pages, written into
//! `tools/parity_oracle/empty_declaration_cases.json` by
//! `declaration_list_log.mjs --write`; none is typed by hand.

#![cfg(all(target_os = "macos", feature = "headless"))]

use super::*;

const CASES: &str = include_str!("../../../tools/parity_oracle/empty_declaration_cases.json");

fn eval(engine: &mut Engine, id: EngineViewId, script: &str) -> String {
    let value = engine.execute_script(id, script).expect("script");
    let inner = value
        .strip_prefix("String(\"")
        .and_then(|v| v.strip_suffix("\")"))
        .unwrap_or(&value);
    inner.replace("\\\"", "\"").replace("\\\\", "\\")
}

#[test]
fn empty_declarations_in_a_rule_read_as_chromium() {
    let doc: serde_json::Value = serde_json::from_str(CASES).expect("cases");
    assert_eq!(doc["oracle"], "148.0.7778.216");
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let mut wrong = Vec::new();
    for c in doc["cases"].as_array().expect("cases") {
        let s = |k: &str| c[k].as_str().unwrap_or("").to_string();
        let id = engine.create_headless_view(Bounds::new(0, 0, 400, 300)).expect("view");
        engine.load_html(id, &s("html")).expect("load");
        let read = eval(&mut engine, id, &s("expr"));
        if read != s("chrome") {
            wrong.push(format!("{}: read {read:?}, Chromium {:?}", s("name"), s("chrome")));
        }
        let _ = engine.destroy_view(id);
    }
    assert!(wrong.is_empty(), "{} wrong\n{}", wrong.len(), wrong.join("\n"));
}
