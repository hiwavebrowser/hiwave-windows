//! A function whose body runs a sloppy direct eval: a `var` that one call's eval declares belongs to that
//! call's environment only. Another call of the same function, made before or after, reads and writes
//! the name where it is declared for that call (an outer scope, or nowhere). Each case runs in its own
//! context and must give Chrome's answer.
//!
//! Expected values are Chrome for Testing 148.0.7778.216's answers:
//! `docs/diagnostics/2026-11-17/eval_var_per_call_cases.json`, written by
//! `tools/parity_oracle/js_cases_log.mjs --write`.

use boa_engine::{Context, Source, js_string};
use std::panic::{AssertUnwindSafe, catch_unwind};

/// Not held to Chrome here, a separate row (EVAL-VAR-SHARED-SCOPE): eval code compiled against the
/// shared scope reads, by its compiled slot, a name only another call's eval declared, and gets a
/// ReferenceError where Chrome reads the name further out. Develop answers the same. Each must still
/// give an answer.
const ROWS: &[&str] = &[
    "later_call_strict_eval_code_reads_a_name_an_earlier_call_declared",
    "later_call_eval_code_reads_a_name_an_earlier_call_declared",
    "closure_made_before_another_call_declared_reads_by_strict_eval",
    "closure_made_before_another_call_declared_declares_it_too",
    "suspended_generator_resumed_after_another_call_declared",
    "arrow_made_before_another_call_declared",
    "eval_in_eval_made_before_another_call_declared",
    "default_parameters_closure_made_before_another_call_declared",
    "old_activation_sloppy_eval_writes_the_outer_name",
];

const CASES: &str = include_str!("../../../docs/diagnostics/2026-11-17/eval_var_per_call_cases.json");

/// The case's answer as the oracle reads it: the completion value as a string.
fn run(js: &str) -> String {
    let mut ctx = Context::default();
    // As the oracle runs it: the case's text through indirect eval (global code), its completion value.
    let lit = serde_json::to_string(js).unwrap();
    let src = format!("globalThis.__r = 'pending'; Promise.resolve((0, eval)({lit})).then(String).then(function (v) {{ globalThis.__r = v; }});");
    if let Err(e) = ctx.eval(Source::from_bytes(&src)) {
        return format!("error: {e}");
    }
    if let Err(e) = ctx.run_jobs() {
        return format!("jobs error: {e}");
    }
    let r = ctx.global_object().get(js_string!("__r"), &mut ctx).unwrap();
    r.to_string(&mut ctx).unwrap().to_std_string_escaped()
}

#[test]
fn an_eval_var_belongs_to_the_call_that_declared_it() {
    let doc: serde_json::Value = serde_json::from_str(CASES).unwrap();
    assert_eq!(doc["chrome_version"].as_str(), Some("148.0.7778.216"));
    let cases = doc["cases"].as_array().unwrap();
    let mut wrong = Vec::new();
    for case in cases {
        let (name, js, want) =
            (case["name"].as_str().unwrap(), case["js"].as_str().unwrap(), case["chrome"].as_str().unwrap());
        let got = catch_unwind(AssertUnwindSafe(|| run(js))).unwrap_or_else(|_| "no answer".to_string());
        // A row's answer is not held to Chrome, but it must be an answer.
        if ROWS.contains(&name) {
            if got == "no answer" {
                wrong.push(format!("{name}: no answer"));
            }
            continue;
        }
        if got != want {
            wrong.push(format!("{name}: chrome {want:?}, here {got:?}"));
        }
    }
    assert!(wrong.is_empty(), "{} of {} differ:\n{}", wrong.len(), cases.len(), wrong.join("\n"));
}

/// A store into a declarative environment at an index its scope declares but the environment was not
/// sized for gives an answer (the lexical / global / module guard). A body-level `using` declaration
/// is such a store while the scope analysis does not bind it there (see the `using-bindings` patch).
#[test]
fn a_store_past_an_environments_end_gives_an_answer() {
    for src in [
        "(function () { using x = null; return 'ok'; })()",
        "(function () { await using x = undefined; return 'ok'; })",
    ] {
        let got = catch_unwind(AssertUnwindSafe(|| {
            let mut ctx = Context::default();
            match ctx.eval(Source::from_bytes(src)) {
                Ok(_) => "answer".to_string(),
                Err(e) => format!("error: {e}"),
            }
        }))
        .unwrap_or_else(|_| "no answer".to_string());
        assert_ne!(got, "no answer", "{src}");
    }
}
