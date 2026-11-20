//! Static source phase imports (`import source x from "m"`, the source-phase-imports proposal, test262
//! feature `source-phase-imports`). The declaration parses; linking it to a Source Text Module throws a
//! SyntaxError, because such a module has no source phase representation (GetModuleSource is always
//! abrupt). A module that cannot be loaded fails at load, before linking, with the host's error.
//! `import source from "m"` and `import source, { a } from "m"` stay default imports named `source`.
//!
//! Before this patch the parser read `import source x` as a default import named `source` and then
//! wanted `from`, so the whole importing module was a SyntaxError at parse time
//! (test262 `test/language/module-code/source-phase-import/import-source.js`).
//!
//! Cases: `docs/diagnostics/2026-11-20/import_source_phase_cases.json`; Chrome's answers from
//! `import_source_phase_chrome.mjs` beside it.

use boa_engine::builtins::promise::PromiseState;
use boa_engine::module::MapModuleLoader;
use boa_engine::{Context, JsError, JsValue, Module, Source, js_string};
use std::collections::BTreeMap;
use std::rc::Rc;

const CASES: &str = include_str!("../../../docs/diagnostics/2026-11-20/import_source_phase_cases.json");
/// Chrome for Testing 148.0.7778.216 with `--js-flags=--js-source-phase-imports`. Without the flag
/// (`import_source_phase_cft148.json`) Chrome 148 does not parse the proposal's form: every
/// `import source <binding>` case is `parse:SyntaxError` there, and the other cases agree.
const CHROME_FLAG: &str = include_str!("../../../docs/diagnostics/2026-11-20/import_source_phase_cft148_flag.json");

const DEP: &str = "export default 42; export const a = 7;";

/// Parses `src` as a module, links and evaluates it against a loader that knows only `dep`, and
/// answers `parse:<ErrorName>`, `<ErrorName>` for a rejection, or `ok:<value of export r>`.
fn run(src: &str) -> String {
    let loader = Rc::new(MapModuleLoader::new());
    let mut ctx = Context::builder().module_loader(loader.clone()).build().unwrap();
    let dep = Module::parse(Source::from_bytes(DEP), None, &mut ctx).unwrap();
    loader.insert("dep", dep);
    let module = match Module::parse(Source::from_bytes(src), None, &mut ctx) {
        Ok(module) => module,
        Err(e) => return format!("parse:{}", name(&e, &mut ctx)),
    };
    let promise = module.load_link_evaluate(&mut ctx);
    ctx.run_jobs().unwrap();
    match promise.state() {
        PromiseState::Fulfilled(_) => {
            let r = module.namespace(&mut ctx).get(js_string!("r"), &mut ctx).unwrap();
            format!("ok:{}", r.display())
        }
        PromiseState::Rejected(reason) => name(&JsError::from_opaque(reason), &mut ctx),
        PromiseState::Pending => "pending".to_string(),
    }
}

fn name(e: &JsError, ctx: &mut Context) -> String {
    let value = e.clone().into_opaque(ctx).unwrap_or(JsValue::undefined());
    value
        .as_object()
        .and_then(|o| o.get(js_string!("name"), ctx).ok())
        .and_then(|n| n.as_string().map(|s| s.to_std_string_escaped()))
        .unwrap_or_else(|| "?".to_string())
}

#[test]
fn source_phase_import_parses_and_fails_at_link_as_chrome_with_the_flag_does() {
    let cases: BTreeMap<String, String> = serde_json::from_str(CASES).unwrap();
    let mut want: BTreeMap<String, String> = serde_json::from_str(CHROME_FLAG).unwrap();
    want.remove("_version");
    assert_eq!(cases.len(), want.len());
    let mut wrong = Vec::new();
    for (name, src) in &cases {
        let got = run(src);
        if got != want[name] {
            wrong.push(format!("{name} {src:?}: Chrome {}, got {got}", want[name]));
        }
    }
    assert!(wrong.is_empty(), "{} of {} differ:\n{}", wrong.len(), cases.len(), wrong.join("\n"));
}
