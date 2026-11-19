//! A regular expression literal is compiled once per code block, not once per evaluation. Each
//! evaluation still makes a new RegExp object (its own identity and its own `lastIndex`), which is
//! the language rule; only the compiled matcher is shared. `regexp_compile_count` counts pattern
//! compiles in the process, so this file holds one test: the counter is process-wide.
//! Expected values for the semantics: Chrome for Testing 148.0.7778.216
//! (`tools/parity_oracle/regex_literal_cases.json`, `js_cases_log.mjs --write`).

use rustkit_js::{JsRuntime, JsValue, regexp_compile_count};

fn run(rt: &mut JsRuntime, js: &str) -> String {
    match rt.evaluate_script(js) {
        Ok(JsValue::String(s)) => s,
        other => format!("{other:?}"),
    }
}

#[test]
fn a_regex_literal_compiles_once_and_every_evaluation_is_a_new_object() {
    let mut rt = JsRuntime::new().unwrap();

    // The count: 1000 evaluations of one literal, in a loop and in a called function.
    let before = regexp_compile_count();
    let looped = run(
        &mut rt,
        "(function(){var n=0;for(var i=0;i<1000;i++){if(/(\\d+)-(\\w+)/.test(i+'-x'))n++}return String(n)})()",
    );
    let looped_compiles = regexp_compile_count() - before;
    let before = regexp_compile_count();
    let called = run(
        &mut rt,
        "(function(){function f(s){return /a+b/g.test(s)}var n=0;for(var i=0;i<1000;i++){if(f('xaab'))n++}return String(n)})()",
    );
    let called_compiles = regexp_compile_count() - before;
    let mut wrong = Vec::new();
    if (looped.as_str(), called.as_str()) != ("1000", "1000") {
        wrong.push(format!("results {looped} / {called}, expected 1000 / 1000"));
    }
    if looped_compiles > 1 || called_compiles > 1 {
        wrong.push(format!(
            "pattern compiles for 1000 evaluations: {looped_compiles} in a loop, {called_compiles} in a called function"
        ));
    }

    // The semantics, against Chrome.
    let data: serde_json::Value =
        serde_json::from_str(include_str!("../../../tools/parity_oracle/regex_literal_cases.json")).expect("cases");
    for c in data["cases"].as_array().expect("cases") {
        let chrome = c["chrome"].as_str().expect("run js_cases_log.mjs --write");
        let got = run(&mut rt, c["js"].as_str().unwrap());
        if got != chrome {
            wrong.push(format!("{}:\n  rustkit {got}\n  chrome  {chrome}", c["name"]));
        }
    }
    assert!(wrong.is_empty(), "{} differ\n{}", wrong.len(), wrong.join("\n"));
}
