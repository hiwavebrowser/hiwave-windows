//! A live turn runs timer callbacks for one time slice, not for 1000 of
//! them (`LIVE_FRAME_MS`, the frame budget #730 slices a turn by), and a page whose task the watchdog stopped
//! can still schedule its next one.
//!
//! Real window, 2026-10-22: a page that chains `setTimeout(0)` work held
//! each turn ~4.7 s (1000 callbacks of a few ms) and the window with it.
//! Sooner or later a turn ran past the 5 s budget, the watchdog stopped a
//! callback part-way, and the page's scheduler, whose bookkeeping was in
//! that callback's `finally`, never posted its next task: no more timers,
//! no more requests.

use super::*;
use crate::script_net_tests::serve_routes;

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap()
}

fn load(config: EngineConfig, page: String) -> (Engine, EngineViewId) {
    let server = serve_routes(vec![("/", "text/html", page)]);
    let mut engine = Engine::new(config).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 200, height: 100 })
        .expect("view");
    let url = Url::parse(&format!("http://127.0.0.1:{}/", server.port)).unwrap();
    runtime().block_on(engine.load_url(view, url)).expect("load_url");
    (engine, view)
}

fn over_budget(engine: &Engine, view: EngineViewId) -> Vec<String> {
    engine
        .script_log(view)
        .unwrap()
        .iter()
        .filter(|record| record.outcome == ScriptOutcome::OverBudget)
        .map(|record| record.source.clone())
        .collect()
}

fn number(engine: &mut Engine, view: EngineViewId, js: &str) -> f64 {
    let out = engine.execute_script(view, js).unwrap();
    out.strip_prefix("Number(")
        .and_then(|rest| rest.strip_suffix(')'))
        .and_then(|n| n.parse().ok())
        .unwrap_or_else(|| panic!("{js} = {out}"))
}

const CHAIN: u32 = 5_000;

/// 5000 `setTimeout(0)` callbacks, each ~2 ms of work, started after the
/// load's timer horizon so the live loop runs them all.
fn chain_page() -> String {
    format!(
        "<html><body><p id=p>alive</p><script>
window.ran = 0;
function step() {{
  var t = Date.now(); while (Date.now() - t < 2) {{}}
  window.ran++;
  if (window.ran < {CHAIN}) setTimeout(step, 0);
}}
setTimeout(step, 60000);
</script></body></html>"
    )
}

#[test]
fn a_page_that_chains_zero_delay_timers_gets_turns_of_one_slice() {
    // A budget the 1000-callback cap overran by about twice (1000 x 2 ms).
    let config = EngineConfig {
        interrupt_scripts_at_budget: true,
        live_turn_budget_ms: 1_000,
        ..EngineConfig::default()
    };
    let (mut engine, view) = load(config, chain_page());
    let rt = runtime();
    let mut elapsed = 60_000;
    let mut longest = std::time::Duration::ZERO;
    let mut turns = 0;
    while turns < 2_000 {
        let started = std::time::Instant::now();
        let pump = rt.block_on(engine.pump_live(view, elapsed));
        longest = longest.max(started.elapsed());
        turns += 1;
        elapsed = started.elapsed().as_millis() as u64;
        if pump.timers_ran > 0 && number(&mut engine, view, "window.ran") < f64::from(CHAIN) {
            // (3) What the slice held back is due now: the loop must not
            // sleep on it.
            assert_eq!(pump.next_timer_ms, Some(0), "turn {turns}");
        }
        if pump.next_timer_ms.is_none() {
            break;
        }
    }
    assert_eq!(over_budget(&engine, view), Vec::<String>::new(), "the watchdog never fires");
    assert_eq!(number(&mut engine, view, "window.ran"), f64::from(CHAIN), "after {turns} turns");
    // The slice and one callback past it, with room for a debug build's
    // layout and bookkeeping; the cap alone made this ~1000 ms (stopped).
    assert!(
        longest < std::time::Duration::from_millis(LIVE_FRAME_MS) + std::time::Duration::from_millis(150),
        "longest turn {longest:?} over {turns} turns"
    );
    assert!(turns > 50, "{turns} turns: the slice cut none of them");
    eprintln!("{CHAIN} callbacks in {turns} turns, longest {longest:?}");
}

/// The shape of a React-style scheduler: one task per message, and the
/// next message posted from the task's `finally`. `{task}` is the first
/// task's body.
fn scheduler_page(task: &str) -> String {
    format!(
        "<html><body><p id=p>alive</p><script>
window.done = [];
var queue = [], running = false, channel = new MessageChannel();
channel.port1.onmessage = function () {{
  var task = queue.shift();
  try {{ if (task) task(); }}
  finally {{ if (queue.length) channel.port2.postMessage(null); else running = false; }}
}};
function schedule(task) {{
  queue.push(task);
  if (!running) {{ running = true; channel.port2.postMessage(null); }}
}}
setTimeout(function () {{
  schedule(function () {{ window.done.push('long'); {task} }});
  schedule(function () {{ window.done.push('next'); }});
}}, 60000);
setTimeout(function () {{ schedule(function () {{ window.done.push('later'); }}); }}, 62000);
</script></body></html>"
    )
}

#[test]
fn a_scheduler_that_rearms_in_finally_runs_its_next_task_after_a_stop() {
    let config = EngineConfig {
        interrupt_scripts_at_budget: true,
        live_turn_budget_ms: 200,
        script_loop_iteration_limit: u64::MAX / 2,
        ..EngineConfig::default()
    };
    let (mut engine, view) = load(config, scheduler_page("while (true) {}"));
    let rt = runtime();
    let started = std::time::Instant::now();
    rt.block_on(engine.pump_live(view, 60_000));
    assert!(started.elapsed() < std::time::Duration::from_millis(1_500), "{:?}", started.elapsed());
    assert_eq!(over_budget(&engine, view), ["timers"], "{:#?}", engine.script_log(view));
    for _ in 0..5 {
        rt.block_on(engine.pump_live(view, 10));
    }
    assert_eq!(
        engine.execute_script(view, "window.done.join(',')").unwrap(),
        r#"String("long,next")"#,
        "the task after the stopped one runs on a later turn"
    );
    // And the scheduler is not stuck "running": a task scheduled later runs.
    rt.block_on(engine.pump_live(view, 2_000));
    for _ in 0..3 {
        rt.block_on(engine.pump_live(view, 10));
    }
    assert_eq!(
        engine.execute_script(view, "window.done.join(',')").unwrap(),
        r#"String("long,next,later")"#
    );
    assert_eq!(over_budget(&engine, view), ["timers"], "one stop");
}

/// A `finally` that does not end is stopped too: the stop gives the page's
/// cleanup a moment, not a second budget.
#[test]
fn a_finally_that_spins_after_a_stop_is_stopped_as_well() {
    let page = "<html><body><p id=p>alive</p><script>
setTimeout(function () {
  try { while (true) {} } finally { window.inFinally = true; while (true) {} }
}, 60000);
setTimeout(function () { window.after = true; }, 61000);
</script></body></html>";
    let config = EngineConfig {
        interrupt_scripts_at_budget: true,
        live_turn_budget_ms: 200,
        script_loop_iteration_limit: u64::MAX / 2,
        ..EngineConfig::default()
    };
    let (mut engine, view) = load(config, page.to_string());
    let rt = runtime();
    let started = std::time::Instant::now();
    rt.block_on(engine.pump_live(view, 60_000));
    let took = started.elapsed();
    assert!(took < std::time::Duration::from_millis(1_000), "{took:?}");
    assert_eq!(over_budget(&engine, view), ["timers"], "{:#?}", engine.script_log(view));
    assert_eq!(engine.execute_script(view, "window.inFinally === true").unwrap(), "Boolean(true)");
    rt.block_on(engine.pump_live(view, 1_000));
    assert_eq!(engine.execute_script(view, "window.after === true").unwrap(), "Boolean(true)");
}
