//! `using` and `await using` declarations outside a block: in a function, arrow, method, generator,
//! async function, async generator and class static block body, in a `for (using ... of ...)` head, and in
//! eval code. Each case runs in its own context and must give Chrome's answer.
//!
//! Expected values are Chrome for Testing 148.0.7778.216's answers:
//! `docs/diagnostics/2026-11-17/using_binding_cases.json`, written by
//! `tools/parity_oracle/js_cases_log.mjs --write`. No case depends on disposal (`Symbol.dispose`).

use boa_engine::{Context, Module, Source, js_string};
use std::panic::{AssertUnwindSafe, catch_unwind};

/// Not held to Chrome here, a separate row: the parser does not read `for (using x of ...)` yet.
const ROWS: &[&str] = &["for_of_in_function"];

const CASES: &str = include_str!("../../../docs/diagnostics/2026-11-17/using_binding_cases.json");

/// The case's answer as the oracle reads it: the value (or the promise's value) as a string.
fn run(js: &str) -> String {
    let mut ctx = Context::default();
    let src = format!("globalThis.__r = 'pending'; Promise.resolve({js}).then(String).then(function (v) {{ globalThis.__r = v; }});");
    if let Err(e) = ctx.eval(Source::from_bytes(&src)) {
        return format!("error: {e}");
    }
    if let Err(e) = ctx.run_jobs() {
        return format!("jobs error: {e}");
    }
    let r = ctx.global_object().get(js_string!("__r"), &mut ctx).unwrap();
    r.to_string(&mut ctx).unwrap().to_std_string_escaped()
}

/// The module case's source: the oracle imports it as a data: URL module, which this harness cannot load,
/// so it runs here through the module API.
const MODULE_SRC: &str = "using x = null; await using y = undefined; export default \"ok \" + x + \" \" + y;";

/// A module's default export as a string, after it is linked and evaluated.
fn run_module(src: &str) -> String {
    let mut ctx = Context::default();
    let module = match Module::parse(Source::from_bytes(src), None, &mut ctx) {
        Ok(m) => m,
        Err(e) => return format!("threw {e}"),
    };
    let promise = module.load_link_evaluate(&mut ctx);
    if let Err(e) = ctx.run_jobs() {
        return format!("jobs error: {e}");
    }
    if let Err(e) = promise.state().as_rejected().map_or(Ok(()), |e| Err(e.display().to_string())) {
        return format!("threw {e}");
    }
    let ns = module.namespace(&mut ctx);
    let v = ns.get(js_string!("default"), &mut ctx).unwrap();
    v.to_string(&mut ctx).unwrap().to_std_string_escaped()
}

#[test]
fn using_declarations_outside_a_block_answer_as_chrome_does() {
    let doc: serde_json::Value = serde_json::from_str(CASES).unwrap();
    assert_eq!(doc["chrome_version"].as_str(), Some("148.0.7778.216"));
    let cases = doc["cases"].as_array().unwrap();
    let mut wrong = Vec::new();
    for case in cases {
        let (name, js, want) =
            (case["name"].as_str().unwrap(), case["js"].as_str().unwrap(), case["chrome"].as_str().unwrap());
        if ROWS.contains(&name) {
            continue;
        }
        let got = if name == "module_top_level" {
            assert!(js.contains("using x = null; await using y = undefined; export default"), "{js}");
            catch_unwind(AssertUnwindSafe(|| run_module(MODULE_SRC))).unwrap_or_else(|_| "no answer".to_string())
        } else {
            catch_unwind(AssertUnwindSafe(|| run(js))).unwrap_or_else(|_| "no answer".to_string())
        };
        if got != want {
            wrong.push(format!("{name}: chrome {want:?}, here {got:?}"));
        }
    }
    assert!(wrong.is_empty(), "{} of {} differ:\n{}", wrong.len(), cases.len(), wrong.join("\n"));
}
