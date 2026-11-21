//! H19c, second half: the error records for timers and lifecycle events hard-coded
//! `elapsed_ms: 0`, so an Inspector line for a slow callback that then threw said it took no time.
//! The record must carry the callback step's measured wall time, as a script's own record does.

use super::*;
use std::io::{Read, Write};
use std::net::TcpListener;

fn load(script: &str) -> (Engine, EngineViewId) {
    load_with(EngineConfig::default(), script)
}

fn load_with(config: EngineConfig, script: &str) -> (Engine, EngineViewId) {
    load_as(config, script, false)
}

/// `yielding`: the load returns after `DOMContentLoaded` when a script the
/// page inserted is still out, and `load` fires in a later turn
/// (`Engine::load_url_yielding`).
fn load_as(config: EngineConfig, script: &str, yielding: bool) -> (Engine, EngineViewId) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let body = format!("<p>x</p><script>{script}</script>");
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
                // `/held.js` answers late, with a script: a page that inserted
                // it has its `load` wait for it (the load's open tail).
                let held = String::from_utf8_lossy(&request).starts_with("GET /held.js");
                let (kind, body) = if held {
                    std::thread::sleep(std::time::Duration::from_millis(400));
                    ("text/javascript", "window.heldRan = true;".to_string())
                } else {
                    ("text/html", body)
                };
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            });
        }
    });
    let mut engine = Engine::new(config).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 300, height: 200 })
        .expect("view");
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let url = Url::parse(&format!("http://127.0.0.1:{port}/")).unwrap();
    if yielding {
        rt.block_on(engine.load_url_yielding(view, url)).expect("load");
    } else {
        rt.block_on(engine.load_url(view, url)).expect("load");
    }
    (engine, view)
}

const SPIN_THEN_THROW: &str = "var t = Date.now(); while (Date.now() - t < 80) {} throw new Error('after the spin');";

#[test]
fn a_timer_that_spins_then_throws_records_its_time() {
    let (engine, view) = load(&format!("setTimeout(function () {{ {SPIN_THEN_THROW} }}, 0);"));
    let log = engine.script_log(view).unwrap();
    let record = log
        .iter()
        .find(|r| r.source == "timers" && matches!(r.outcome, ScriptOutcome::Threw(_)))
        .unwrap_or_else(|| panic!("no timers Threw record: {log:#?}"));
    assert!(record.elapsed_ms >= 60, "the 80 ms callback is recorded as {} ms: {log:#?}", record.elapsed_ms);
}

#[test]
fn a_load_listener_that_spins_then_throws_records_its_time() {
    let (engine, view) = load(&format!("window.addEventListener('load', function () {{ {SPIN_THEN_THROW} }});"));
    let log = engine.script_log(view).unwrap();
    let record = log
        .iter()
        .find(|r| r.source == "event:load" && matches!(r.outcome, ScriptOutcome::Threw(_)))
        .unwrap_or_else(|| panic!("no event:load Threw record: {log:#?}"));
    assert!(record.elapsed_ms >= 60, "the 80 ms listener is recorded as {} ms: {log:#?}", record.elapsed_ms);
}

/// SCRIPT-TIME-DUP: a step that reported several errors (one timer calling
/// `reportError` five times, a `load` listener doing the same) is one
/// stretch of script time. The log has a record for each error, and the
/// step's time on one of them only: a total over the log (a capture's
/// script time) counts the step once.
#[test]
fn a_step_that_reports_several_errors_counts_its_time_once() {
    const SPIN_THEN_REPORT: &str =
        "var t = Date.now(); while (Date.now() - t < 40) {} for (var i = 0; i < 5; i++) reportError(new Error('reported ' + i));";
    for (source, script) in [
        ("timers", format!("setTimeout(function () {{ {SPIN_THEN_REPORT} }}, 0);")),
        ("event:load", format!("window.addEventListener('load', function () {{ {SPIN_THEN_REPORT} }});")),
    ] {
        let (engine, view) = load(&script);
        let log = engine.script_log(view).unwrap();
        let times: Vec<u64> = log
            .iter()
            .filter(|r| r.source == source && matches!(&r.outcome, ScriptOutcome::Threw(m) if m.contains("reported ")))
            .map(|r| r.elapsed_ms)
            .collect();
        assert_eq!(times.len(), 5, "{source}: {log:#?}");
        let (total, longest) = (times.iter().sum::<u64>(), *times.iter().max().unwrap());
        assert!(longest >= 20, "{source}: the 40 ms step is recorded as {longest} ms");
        assert_eq!(total, longest, "{source}: the step's time is counted {} times: {times:?}", times.iter().filter(|&&t| t > 0).count());
        assert_eq!(times[0], longest, "{source}: the first record carries it: {times:?}");
    }
}

/// A step that ends in a record of its own (it was stopped at the script
/// budget) and also reported errors on the way has its time on its own
/// record only: the errors' records carry 0, so a total over the log
/// counts the step once. For the `load` step, a `DOMContentLoaded`
/// listener, the timers of the load, and the timers of a live turn.
#[test]
fn a_step_with_its_own_record_that_also_reported_errors_counts_its_time_once() {
    const REPORT_THEN_SPIN: &str = "reportError(new Error('reported first')); reportError(new Error('reported second')); for (;;) { document.getElementById('x'); }";
    let budget = || EngineConfig { script_budget_ms: 300, live_turn_budget_ms: 300, interrupt_scripts_at_budget: true, ..EngineConfig::default() };
    let check = |what: &str, source: &str, log: &[ScriptRecord], least: u64| {
        let step: Vec<&ScriptRecord> = log.iter().filter(|r| r.source == source).collect();
        let is_report = |r: &&&ScriptRecord| matches!(&r.outcome, ScriptOutcome::Threw(m) if m.contains("reported "));
        let reported: Vec<u64> = step.iter().filter(is_report).map(|r| r.elapsed_ms).collect();
        let own: Vec<u64> = step.iter().filter(|r| !is_report(r)).map(|r| r.elapsed_ms).collect();
        assert_eq!(reported.len(), 2, "{what}: {log:#?}");
        assert_eq!(own.len(), 1, "{what}: the step's own record: {log:#?}");
        assert!(own[0] >= least, "{what}: the stopped step's own record has its time: {log:#?}");
        assert_eq!(reported, [0, 0], "{what}: the step's time is on its own record only: {log:#?}");
    };
    // The `load` step and a listener step before it.
    for (source, event, target) in [("event:load", "load", "window"), ("event:DOMContentLoaded", "DOMContentLoaded", "document")] {
        let (engine, view) = load_with(budget(), &format!("{target}.addEventListener('{event}', function () {{ {REPORT_THEN_SPIN} }});"));
        check(source, source, &engine.script_log(view).unwrap(), 100);
    }
    // The `load` step of a yielding load, fired in a later turn once the
    // script the page inserted has arrived (`complete_load_tail`). With
    // interruption off the listener's endless loop ends at the loop limit,
    // which reaches that step as its own error.
    let (mut engine, view) = load_as(
        EngineConfig { script_loop_iteration_limit: 200_000, post_load_requests_live: true, ..EngineConfig::default() },
        &format!("var s = document.createElement('script'); s.src = '/held.js'; document.head.appendChild(s); \
                  window.addEventListener('load', function () {{ \
                  reportError(new Error('reported first')); reportError(new Error('reported second')); for (;;) {{}} }});"),
        true,
    );
    assert!(!engine.script_log(view).unwrap().iter().any(|r| r.source == "event:load"), "`load` has not fired when the load yields");
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    // Bounded by a count of turns (each waits a little for the held script).
    for _ in 0..300 {
        if engine.script_log(view).unwrap().iter().any(|r| r.source == "event:load") {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
        rt.block_on(engine.pump_live(view, 20));
    }
    check("a load step fired in a later turn", "event:load", &engine.script_log(view).unwrap(), 1);
    // The timers of the load.
    let (engine, view) = load_with(budget(), &format!("setTimeout(function () {{ {REPORT_THEN_SPIN} }}, 0);"));
    check("the load's timers", "timers", &engine.script_log(view).unwrap(), 100);
    // The timers of a live turn, after the load.
    let (mut engine, view) =
        load_with(EngineConfig { timer_horizon_ms: 0, ..budget() }, &format!("setTimeout(function () {{ {REPORT_THEN_SPIN} }}, 50);"));
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    for _ in 0..20 {
        rt.block_on(engine.pump_live(view, 50));
        if engine.script_log(view).unwrap().iter().any(|r| r.source == "timers") {
            break;
        }
    }
    check("a live turn's timers", "timers", &engine.script_log(view).unwrap(), 100);
}
