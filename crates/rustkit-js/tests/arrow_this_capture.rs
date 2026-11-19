//! `this`, `arguments`, `new.target` and `super` captured through nested arrow functions.
//! In `m() { return () => () => this.name; }` the inner arrow saw the wrong `this` when the
//! outer arrow did not use `this` itself (found in the Speedometer reductions; breaks TipTap).
//!
//! Expected values are Chrome for Testing 148.0.7778.216's answers on the same function bodies:
//! `docs/diagnostics/2026-11-12/arrow_this_cases.mjs` writes both JSON files used here.

use boa_engine::{Context, Source};
use std::collections::BTreeMap;

const CASES: &str = include_str!("../../../docs/diagnostics/2026-11-12/arrow_this_cases.json");
const CHROME: &str = include_str!("../../../docs/diagnostics/2026-11-12/arrow_this_cft148.json");

fn run(body: &str) -> String {
    let mut ctx = Context::default();
    let src = format!(
        "(function () {{ try {{ return String((function () {{ {body} }})()); }} catch (e) {{ return 'ERR:' + e.name; }} }})()"
    );
    match ctx.eval(Source::from_bytes(&src)) {
        Ok(v) => v.to_string(&mut ctx).map(|s| s.to_std_string_escaped()).unwrap_or_else(|_| "?".into()),
        Err(e) => format!("PARSE/RUN:{e}"),
    }
}

#[test]
fn nested_arrows_capture_this_arguments_new_target_and_super_as_chrome_does() {
    let cases: BTreeMap<String, String> = serde_json::from_str(CASES).unwrap();
    let want: BTreeMap<String, String> = serde_json::from_str(CHROME).unwrap();
    assert_eq!(cases.len(), want.len());
    let got: BTreeMap<String, String> = cases.iter().map(|(k, body)| (k.clone(), run(body))).collect();
    let wrong: Vec<String> = want
        .iter()
        .filter(|(k, v)| got.get(*k) != Some(*v))
        .map(|(k, v)| format!("{k}: chrome {v:?}, here {:?}", got[k]))
        .collect();
    assert!(wrong.is_empty(), "{} of {} differ:\n{}", wrong.len(), want.len(), wrong.join("\n"));
}
