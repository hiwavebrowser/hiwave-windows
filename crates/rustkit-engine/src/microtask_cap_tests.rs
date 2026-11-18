//! H19c: the per-drain job cap (`script_loop_iteration_limit.min(100_000)`) killed a promise chain
//! of more than 100 000 jobs, where Chrome runs 200 000 `.then`s in about 17 ms. The cap exists
//! so a runaway promise recursion cannot hang the page; the script budget's interrupt (the view's
//! control word, which Boa reads at every call, microtask jobs included) is the guard that
//! actually bounds time, so the cap can be the loop limit as long as the budget still stops an
//! endless chain. These pin both halves.

use super::*;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::{Duration, Instant};

fn serve_page(body: &str) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let body = body.to_string();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let body = body.clone();
            std::thread::spawn(move || {
                let mut buf = [0u8; 2048];
                let mut request = Vec::new();
                while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                    match stream.read(&mut buf) {
                        Ok(0) | Err(_) => return,
                        Ok(n) => request.extend_from_slice(&buf[..n]),
                    }
                }
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            });
        }
    });
    port
}

fn load(config: EngineConfig, script: &str) -> (Engine, EngineViewId, Duration) {
    let port = serve_page(&format!("<p>x</p><script>{script}</script>"));
    let mut engine = Engine::new(config).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 300, height: 200 })
        .expect("view");
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let started = Instant::now();
    rt.block_on(engine.load_url(view, Url::parse(&format!("http://127.0.0.1:{port}/")).unwrap()))
        .expect("load");
    (engine, view, started.elapsed())
}

/// A chain of `n` `.then`s, each queuing the next, counted in `window.n`.
fn chain(n: u32) -> String {
    format!("window.n = 0; function step() {{ if (++window.n < {n}) Promise.resolve().then(step); }} step();")
}

/// The same chain with work in each step (an inner loop), so a job costs real time however fast
/// the interpreter is: the timeout test must not depend on a cheap job taking ~50 us (#1038's
/// optimised rustkit-js makes it several times cheaper and would thin the margin out).
fn costly_chain(n: u32) -> String {
    format!(
        "window.n = 0; window.acc = 0; function step() {{ for (var i = 0; i < 5000; i++) window.acc += i;          if (++window.n < {n}) Promise.resolve().then(step); }} step();"
    )
}

/// The live app's engine: the script budget interrupts a running script, microtask drains
/// included, so a long budget lets a long (finite) chain finish.
fn live_config(budget_ms: u64) -> EngineConfig {
    EngineConfig {
        script_budget_ms: budget_ms,
        interrupt_scripts_at_budget: true,
        ..EngineConfig::default()
    }
}

/// 120 000 jobs: past the old 100 000 cap, short enough for a debug build (about 50 us a job).
const LONG_CHAIN: u32 = 120_000;

#[test]
fn with_the_budget_interrupt_a_chain_past_100k_jobs_runs_to_the_end() {
    let (mut engine, view, _) = load(live_config(120_000), &chain(LONG_CHAIN));
    let n = engine.execute_script(view, "String(window.n)").unwrap();
    assert_eq!(n, format!("String(\"{LONG_CHAIN}\")"), "the chain ran to its end; log: {:#?}", engine.script_log(view));
    let log = engine.script_log(view).unwrap();
    assert!(
        log.iter().all(|r| matches!(r.outcome, ScriptOutcome::Ran)),
        "no record may report the job cap: {log:#?}"
    );
}

#[test]
fn without_the_budget_interrupt_the_cap_still_bounds_a_runaway_chain() {
    // The default engine config has no watchdog, so the cap is the only guard there. The executor
    // ALSO has a wall-clock job timeout (`script_budget_ms`, 5 s by default): on a machine slower
    // than ~40 us a job the timeout trips first and the test would read the wrong message. So the
    // timeout is put far out of reach here, and the assertion is the cap's own message.
    let config = EngineConfig { script_budget_ms: 3_600_000, ..EngineConfig::default() };
    let (mut engine, view, _) = load(config, &chain(LONG_CHAIN));
    let text = format!("{:#?}", engine.script_log(view));
    assert!(text.contains("Job queue iteration limit (100000)"), "the cap must still fire: {text}");
    assert!(!text.contains("execution timeout"), "and the wall-clock timeout must not be what ended it: {text}");
}

#[test]
fn the_wall_clock_job_timeout_still_fires_when_the_cap_is_out_of_reach() {
    // The sibling: neither limit is left unpinned. A 300 ms timeout against a chain of 120 000 jobs
    // that each loop 5 000 times (tens of seconds at any speed the interpreter reaches, so the
    // margin does not thin out as rustkit-js gets faster) ends it with the timeout's message, not
    // the cap's, because it is checked each loop turn and the cap would take far longer.
    let config = EngineConfig { script_budget_ms: 300, ..EngineConfig::default() };
    let (mut engine, view, took) = load(config, &costly_chain(LONG_CHAIN));
    let text = format!("{:#?}", engine.script_log(view));
    assert!(text.contains("Job queue execution timeout"), "the timeout must be what ended it: {text}");
    assert!(!text.contains("Job queue iteration limit"), "not the cap: {text}");
    assert!(took < Duration::from_secs(8), "and the timeout bounded the time, took {took:?}");
}

#[test]
fn an_endless_promise_chain_is_stopped_by_the_script_budget_not_the_job_cap() {
    let (mut engine, view, took) =
        load(live_config(400), "window.n = 0; function f() { window.n++; Promise.resolve().then(f); } f();");
    // The page comes back, in bounded time, with the budget (not the cap) as the reason.
    assert!(took < Duration::from_secs(8), "the load took {took:?}: nothing stopped the chain");
    let log = engine.script_log(view).unwrap();
    let text = format!("{log:#?}");
    assert!(!text.contains("Job queue iteration limit"), "the job cap ended it, not the budget: {text}");
    assert!(
        log.iter().any(|r| !matches!(r.outcome, ScriptOutcome::Ran)),
        "the stopped chain is recorded as stopped: {text}"
    );
}

/// The chain the next two tests run: endless, each job counted in `window.n`.
const ENDLESS: &str = "window.n = 0; function f() { window.n++; Promise.resolve().then(f); } f(); 'returned'";

fn jobs_run(engine: &mut Engine, view: EngineViewId) -> u64 {
    engine
        .execute_script(view, "String(window.n)")
        .unwrap()
        .trim_start_matches("String(\"")
        .trim_end_matches("\")")
        .parse()
        .unwrap_or(u64::MAX)
}

#[test]
fn an_endless_promise_chain_through_execute_script_is_stopped_by_the_live_turn_time_budget() {
    // JS1 (#1048): `Engine::execute_script` (chrome IPC, the devtools console, `PageCmd::Eval`)
    // arms a live turn now, so under #1031's design its microtask cap is the loop limit (1G by
    // default) and only the TIME budget bounds an endless chain. The budget here is 400 ms; the
    // chain must be recorded as stopped by it, in bounded wall-clock, not by a job count.
    let config = EngineConfig { live_turn_budget_ms: 400, ..live_config(60_000) };
    let (mut engine, view, _) = load(config, "window.n = 0;");
    let started = Instant::now();
    let _ = engine.execute_script(view, ENDLESS);
    let took = started.elapsed();
    assert!(took < Duration::from_secs(4), "nothing stopped the chain: {took:?}");
    let log = engine.script_log(view).unwrap();
    let text = format!("{log:#?}");
    assert!(
        log.iter().any(|r| r.source == "eval" && r.outcome == ScriptOutcome::OverBudget),
        "the time budget stopped it: {text}"
    );
    assert!(!text.contains("Job queue iteration limit"), "not a job count: {text}");
    assert_eq!(engine.execute_script(view, "1 + 1").unwrap(), "Number(2.0)", "the page is usable after");
}

#[test]
fn an_evaluation_with_no_turn_armed_keeps_the_100k_cap_under_the_live_config() {
    // Prometheus R1 on #1031, kept for what can still run unarmed (the bookkeeping evaluations
    // that go straight to the bindings, as turn 0 does): no deadline bounds them, so the raised
    // cap must not apply. The loop limit is 300 000 so that "the cap followed the config" and
    // "the cap held at 100 000" differ by seconds, not minutes.
    let config = EngineConfig {
        script_loop_iteration_limit: 300_000,
        ..live_config(60_000)
    };
    let (mut engine, view, _) = load(config, "window.n = 0;");
    let started = Instant::now();
    let result = engine
        .views
        .get(&view)
        .and_then(|v| v.bindings.as_ref())
        .expect("bindings")
        .evaluate(ENDLESS);
    let took = started.elapsed();
    let n = jobs_run(&mut engine, view);
    assert!(n <= 100_001, "the unarmed evaluation ran {n} jobs ({took:?}); the cap followed the config: {result:?}");
}
