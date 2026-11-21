//! `ArrayBuffer.prototype.transfer`, `transferToFixedLength` and `detached`: Boa has them behind its
//! `experimental` feature, so a page saw none of them, and a structured-clone transfer
//! (`postMessage(buf, [buf])`) had no way to detach the sender's buffer.
//!
//! Expected values: Chrome for Testing 148.0.7778.216
//! (`docs/diagnostics/2026-11-12/arraybuffer_transfer_cases.mjs`).

use rustkit_js::{JsRuntime, JsValue};
use std::collections::BTreeMap;

const CASES: &str = include_str!("../../../docs/diagnostics/2026-11-12/arraybuffer_transfer_cases.json");
const CHROME: &str = include_str!("../../../docs/diagnostics/2026-11-12/arraybuffer_transfer_cft148.json");

fn run(body: &str) -> String {
    let mut rt = JsRuntime::new().unwrap();
    let src = format!(
        "(function () {{ try {{ return String((function () {{ {body} }})()); }} catch (e) {{ return 'THROWN:' + e.name; }} }})()"
    );
    match rt.evaluate_script(&src) {
        Ok(JsValue::String(s)) => s,
        other => format!("?{other:?}"),
    }
}

#[test]
fn array_buffer_transfer_matches_chrome() {
    let cases: BTreeMap<String, String> = serde_json::from_str(CASES).unwrap();
    let want: BTreeMap<String, String> = serde_json::from_str(CHROME).unwrap();
    let wrong: Vec<String> = want
        .iter()
        .filter_map(|(k, v)| {
            let got = run(&cases[k]);
            (&got != v).then(|| format!("{k}: chrome {v:?}, here {got:?}"))
        })
        .collect();
    assert!(wrong.is_empty(), "{} of {} differ:\n{}", wrong.len(), want.len(), wrong.join("\n"));
}
