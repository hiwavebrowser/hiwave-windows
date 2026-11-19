//! JS1: the per-loop iteration cap is a guard for configs without the
//! watchdog, not the thing that decides how long a page's script may run.
//! At 10M iterations an optimized Boa threw `RuntimeLimitError` after
//! 50 to 70 ms, so a legitimate heavy loop (a 50M-iteration hash or a
//! table build) was killed far inside the 5 s turn budget. The time budget
//! (`interrupt_scripts_at_budget`) is what stops a runaway now.

#![cfg(feature = "headless")]

use super::*;

fn view(config: EngineConfig) -> (Engine, EngineViewId) {
    let mut engine = Engine::new(config).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 200, height: 100 })
        .expect("view");
    (engine, view)
}

fn threw(engine: &Engine, view: EngineViewId) -> Vec<String> {
    engine
        .script_log(view)
        .unwrap()
        .iter()
        .filter_map(|r| match &r.outcome {
            ScriptOutcome::Threw(m) => Some(m.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn a_twelve_million_iteration_loop_completes_in_one_turn() {
    // The app's shape: the watchdog is on, so completion here is inside
    // the app's 5 s turn budget, not merely under the cap. 12M is past the
    // old 10M cap and takes ~1.8 s here (`[profile.dev.package."*"]`, #1038:
    // about 7M iterations/s for this body, so 50M would not fit in 5 s).
    let config = EngineConfig {
        interrupt_scripts_at_budget: true,
        script_budget_ms: 5_000,
        ..EngineConfig::default()
    };
    let (mut engine, id) = view(config);
    engine
        .load_html(id, "<html><body><script>(function () { var n = 0; for (var i = 0; i < 12000000; i++) { n++; } window.done = n; })();</script></body></html>")
        .expect("load_html");
    assert_eq!(engine.execute_script(id, "window.done").unwrap(), "Number(12000000.0)");
    assert!(threw(&engine, id).is_empty(), "{:?}", threw(&engine, id));
}

#[test]
fn while_true_is_still_stopped_by_the_time_budget() {
    let config = EngineConfig {
        interrupt_scripts_at_budget: true,
        script_budget_ms: 300,
        ..EngineConfig::default()
    };
    let (mut engine, id) = view(config);
    let started = std::time::Instant::now();
    engine
        .load_html(id, "<html><body><script>while (true) {}</script></body></html>")
        .expect("load_html");
    let took = started.elapsed();
    assert!(took < std::time::Duration::from_secs(5), "{took:?}");
    let log = engine.script_log(id).unwrap();
    assert!(
        log.iter().any(|r| r.outcome == ScriptOutcome::OverBudget),
        "stopped by the budget, not the iteration cap: {log:#?}"
    );
}

#[test]
fn the_microtask_cap_does_not_follow_the_loop_cap() {
    let (mut engine, id) = view(EngineConfig::default());
    let started = std::time::Instant::now();
    engine
        .load_html(id, "<html><body><p>alive</p><script>window.n = 0; function again() { window.n++; Promise.resolve().then(again); } again();</script></body></html>")
        .expect("load_html");
    assert!(started.elapsed() < std::time::Duration::from_secs(10), "{:?}", started.elapsed());
    // The 100k job cap ended it (100001 runs with optimized dependencies),
    // not the 1G loop cap, which would be 10000x further.
    let n = engine.execute_script(id, "window.n").unwrap();
    let n: f64 = n.trim_start_matches("Number(").trim_end_matches(')').parse().unwrap();
    assert!((99_900.0..=100_100.0).contains(&n), "microtask chain ran {n} times");
}

/// Prometheus's R1 on #1048: `execute_script` (the chrome's eval, the
/// devtools console, `PageCmd::Eval`) begins no turn of its own, so with
/// the iteration cap out of the way a `while (true) {}` there would hold
/// the page's thread for good.
#[test]
fn while_true_through_execute_script_is_stopped_by_the_live_turn_budget() {
    let config = EngineConfig {
        interrupt_scripts_at_budget: true,
        live_turn_budget_ms: 300,
        ..EngineConfig::default()
    };
    let (mut engine, id) = view(config);
    engine.load_html(id, "<html><body><p>alive</p></body></html>").expect("load_html");
    let started = std::time::Instant::now();
    let _ = engine.execute_script(id, "while (true) {}");
    let took = started.elapsed();
    assert!(took < std::time::Duration::from_secs(3), "{took:?}");
    let log = engine.script_log(id).unwrap();
    assert!(
        log.iter().any(|r| r.outcome == ScriptOutcome::OverBudget && r.source == "eval"),
        "{log:#?}"
    );
    // The page is usable afterwards.
    assert_eq!(engine.execute_script(id, "1 + 1").unwrap(), "Number(2.0)");
}
