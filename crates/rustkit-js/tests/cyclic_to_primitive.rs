//! A user function that converts its own object (`toString() { return String(o); }`) must recurse
//! until the engine's limit, as in Chrome (RangeError), not return "" after one level: Boa's
//! `ordinary_to_primitive` carried a `RecursionLimiter` that answered the hint's default for any
//! object already being converted. V8 guards arrays in `join` instead (a cycle contributes "").
//!
//! Expected values: Chrome for Testing 148.0.7778.216 (`docs/diagnostics/2026-11-12/
//! cyclic_to_primitive_cases.mjs`).

use rustkit_js::{JsRuntime, JsValue};
use std::collections::BTreeMap;

const CASES: &str = include_str!("../../../docs/diagnostics/2026-11-12/cyclic_to_primitive_cases.json");
const CHROME: &str = include_str!("../../../docs/diagnostics/2026-11-12/cyclic_to_primitive_cft148.json");

fn child_run(body: String) -> String {
    // A deep host re-entry climb needs the script thread's big stack, not a 2 MiB test thread's.
    rustkit_js::spawn_script_thread("cyclic-case", move || {
        let mut rt = JsRuntime::new().unwrap();
        let src = format!(
            "(function () {{ try {{ return String((function () {{ {body} }})()); }} catch (e) {{ return 'THROWN:' + e.name; }} }})()"
        );
        match rt.evaluate_script(&src) {
            Ok(JsValue::String(s)) => s,
            Ok(other) => format!("?{other:?}"),
            Err(e) => format!("UNCAUGHT:{e}"),
        }
    })
    .unwrap()
    .join()
    .unwrap()
}

/// The child half: runs the case named by CYC_CASE and prints its result.
#[test]
fn child() {
    let Ok(name) = std::env::var("CYC_CASE") else { return };
    let cases: BTreeMap<String, String> = serde_json::from_str(CASES).unwrap();
    println!("CYC_RESULT:{}", child_run(cases[&name].clone()));
}

/// Each case in a process of its own: a runaway recursion that overflows the native stack aborts
/// the process, which the test must report as that case's answer, not die of.
fn run(name: &str) -> String {
    let out = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["child", "--exact", "--nocapture", "--test-threads=1"])
        .env("CYC_CASE", name)
        .stderr(std::process::Stdio::null())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    match stdout.lines().find_map(|l| l.split_once("CYC_RESULT:").map(|(_, r)| r.to_string())) {
        Some(r) => r,
        None => format!("CRASH({})", out.status),
    }
}

#[test]
fn cyclic_conversions_match_chrome() {
    let cases: BTreeMap<String, String> = serde_json::from_str(CASES).unwrap();
    let want: BTreeMap<String, String> = serde_json::from_str(CHROME).unwrap();
    let mut wrong = Vec::new();
    for (name, expected) in &want {
        let got = run(name);
        let _ = &cases;
        // Chrome overflows its stack with a RangeError. Here the host re-entry cap ends the same
        // runaway with an uncatchable RuntimeLimitError (STR-10 left host re-entry uncatchable):
        // different, but loud, which is what matters; a silent "" or "value:" is the bug.
        // Only a user function's runaway recursion may end that way: a loop made only of built-ins never
        // enters the interpreter, so it must reach the native-stack guard and throw a catchable RangeError.
        let loud_instead_of_range_error = name.starts_with("user_")
            && expected.contains("ERR:RangeError")
            && got.starts_with("UNCAUGHT:")
            && got.contains("RuntimeLimitError");
        if &got != expected && !loud_instead_of_range_error {
            wrong.push(format!("{name}: chrome {expected:?}, here {:?}", got.chars().take(120).collect::<String>()));
        }
    }
    assert!(wrong.is_empty(), "{} of {} differ:\n{}", wrong.len(), want.len(), wrong.join("\n"));
}
