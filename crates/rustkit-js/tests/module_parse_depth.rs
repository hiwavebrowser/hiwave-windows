//! A module source nested too deep to parse must fail with a catchable `RangeError`, as Chrome's import
//! does, not overflow the native stack (#1099's MED, Prometheus R1): `Module::parse` ran with no engine
//! entry, and once any engine entry had installed the parser's process-wide stack probe, a parse outside
//! an entry was never measured at all, so a deep module source recursed until the thread overflowed.
//!
//! Each case runs in a child process (an overflow aborts it). The child runs a script first, so the probe
//! is installed as it is in a page that has run anything. Expected values: Chrome for Testing
//! 148.0.7778.216, `docs/diagnostics/2026-11-13/module_parse_depth_cases.mjs` (dynamic `import()` of a
//! blob: module; "ok:object" there is a module that parsed and evaluated, here a module that parsed).

use rustkit_js::{JsRuntime, JsValue};
use std::collections::BTreeMap;
use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const CASES: &str = include_str!("../../../docs/diagnostics/2026-11-13/module_parse_depth_cases.json");
const CHROME: &str = include_str!("../../../docs/diagnostics/2026-11-13/module_parse_depth_cft148.json");

fn child_run(expr: String) -> String {
    rustkit_js::spawn_script_thread("module-depth-case", move || {
        let mut rt = JsRuntime::new().unwrap();
        // Any script first: the engine entry installs the parser's stack probe, as on a real page.
        let _ = rt.evaluate_script("1 + 1");
        let source = match rt.evaluate_script(&expr) {
            Ok(JsValue::String(s)) => s,
            other => return format!("?source {other:?}"),
        };
        let parsed = match rt.begin_module("https://module-depth.test/case.mjs", &source) {
            Ok(_) => "ok".to_string(),
            Err(message) => format!("ERR:{}", message.split(':').next().unwrap_or("").trim()),
        };
        // The runtime runs on after a failed parse, as the page does in Chrome.
        let after = match rt.evaluate_script("'after:' + String([1, 2])") {
            Ok(JsValue::String(s)) => s,
            other => format!("?after {other:?}"),
        };
        format!("{parsed}|{after}")
    })
    .unwrap()
    .join()
    .unwrap()
}

/// The child half: runs the case named by MPD_CASE and prints its result.
#[test]
fn child() {
    let Ok(name) = std::env::var("MPD_CASE") else { return };
    let cases: BTreeMap<String, String> = serde_json::from_str(CASES).unwrap();
    println!("MPD_RESULT:{}", child_run(cases[&name].clone()));
}

fn isolated(name: &str) -> String {
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["child", "--exact", "--nocapture", "--test-threads=1"])
        .env("MPD_CASE", name)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let start = Instant::now();
    let status = loop {
        if let Some(s) = child.try_wait().unwrap() {
            break Some(s);
        }
        if start.elapsed() > Duration::from_secs(60) {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let mut out = String::new();
    let _ = child.stdout.take().unwrap().read_to_string(&mut out);
    match out.lines().find_map(|l| l.split_once("MPD_RESULT:").map(|(_, r)| r.to_string())) {
        Some(r) => r,
        None => match status {
            Some(s) => format!("CRASH({s})"),
            None => "HANG".into(),
        },
    }
}

#[test]
fn a_module_nested_too_deep_fails_with_a_range_error_as_in_chrome() {
    let cases: BTreeMap<String, String> = serde_json::from_str(CASES).unwrap();
    let chrome: BTreeMap<String, String> = serde_json::from_str(CHROME).unwrap();
    let mut wrong = Vec::new();
    for name in cases.keys() {
        // Chrome: "ERR:RangeError" for a deep source, "ok:object" for the shallow one (it parsed).
        let want_parse = if chrome[name].starts_with("ok:") { "ok".to_string() } else { chrome[name].clone() };
        let want = format!("{want_parse}|{}", chrome["page_runs_on"]);
        let got = isolated(name);
        if got != want {
            wrong.push(format!("{name}: chrome {want:?}, here {got:?}"));
        }
    }
    assert!(wrong.is_empty(), "{} of {} differ:\n{}", wrong.len(), cases.len(), wrong.join("\n"));
}
