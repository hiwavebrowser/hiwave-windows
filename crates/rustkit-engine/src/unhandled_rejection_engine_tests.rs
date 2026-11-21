//! `unhandledrejection` in a loaded page, through the engine's own load and live turns: a
//! rejection made by the page's script, by one of its timers and by an animation frame callback
//! each reach the Window's listener after that task's microtasks, and one nobody cancels is
//! still counted for the console.
//!
//! The values are the pinned Chrome's (CfT 148.0.7778.216, headless;
//! `docs/diagnostics/2026-11-15/unhandled_rejection_cft148.json`: `order.timer`,
//! `order.rejectedInMicrotask`, `event`, `handled.sameTask`, `prevent`, `later.catch`). The
//! bindings' own tests (`unhandled_rejection_tests`) run the whole probe.

use super::inspector::InspectorSummary;
use super::inspector_engine_tests::{inspected_engine, records};
use super::*;

const PAGE: &str = r#"<!doctype html><html><head><title>rejections</title></head><body>
<script>
window.__log = [];
window.addEventListener("unhandledrejection", function (e) {
  __log.push("unhandled(" + e.reason.message + ")" + (e.isTrusted && e.cancelable && e.promise instanceof Promise ? "" : "!"));
  if (e.reason.message === "quiet") e.preventDefault();
});
window.onrejectionhandled = function (e) { __log.push("handled(" + e.reason.message + ")"); };
// the page's own script
__log.push("script");
window.__late = Promise.reject(new Error("in-script"));
Promise.reject(new Error("caught")).catch(function () { __log.push("caught"); });
Promise.resolve().then(function () { __log.push("micro"); });
// a timer
setTimeout(function () {
  __log.push("timer");
  Promise.reject(new Error("quiet"));
  Promise.resolve().then(function () { __log.push("timer-micro"); });
}, 20);
// an animation frame
requestAnimationFrame(function () {
  __log.push("frame");
  (async function () { throw new Error("in-frame"); })();
  Promise.resolve().then(function () { __log.push("frame-micro"); });
});
</script>
</body></html>"#;

fn read(engine: &mut Engine, view: EngineViewId, expr: &str) -> String {
    let value = engine.execute_script(view, &format!("String({expr})")).unwrap();
    value.strip_prefix("String(\"").and_then(|v| v.strip_suffix("\")")).unwrap_or(&value).to_string()
}

/// Live turns of 16 ms until `done` holds; the bound is a count of turns.
fn pump_until(engine: &mut Engine, view: EngineViewId, done: &str) {
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    for _ in 0..400 {
        if read(engine, view, done) == "true" {
            return;
        }
        rt.block_on(engine.pump_live(view, 16));
    }
    panic!("`{done}` did not hold after 400 live turns: {}", read(engine, view, "window.__log.join(' ')"));
}

#[test]
fn rejections_of_a_loaded_page_reach_its_window_listener_after_each_tasks_microtasks() {
    let (mut engine, view) = inspected_engine(EngineConfig::default());
    engine.load_html(view, PAGE).expect("load_html");
    pump_until(&mut engine, view, "['timer-micro', 'frame-micro'].every(function (k) { return window.__log.indexOf(k) >= 0; }) \
                                   && window.__log.filter(function (k) { return k.indexOf('unhandled') === 0; }).length >= 3");
    let log = read(&mut engine, view, "window.__log.join(' ')");
    let at = |what: &str| log.split(' ').position(|k| k == what).unwrap_or_else(|| panic!("no {what} in: {log}"));
    // The script's own: after its microtasks, and nothing for the promise it caught.
    assert!(log.starts_with("script caught micro unhandled(in-script)") || log.starts_with("script micro caught unhandled(in-script)"), "{log}");
    assert!(!log.contains("unhandled(caught)"), "{log}");
    // A timer's and a frame callback's: each after that callback's microtask.
    assert!(at("timer") < at("timer-micro") && at("timer-micro") < at("unhandled(quiet)"), "{log}");
    assert!(at("frame") < at("frame-micro") && at("frame-micro") < at("unhandled(in-frame)"), "{log}");
    assert_eq!(log.matches("unhandled(").count(), 3, "one event a rejection: {log}");
    assert!(!log.contains('!'), "each event is trusted, cancelable and carries its promise: {log}");
    // The console's count: the two nobody canceled, not the one a listener canceled.
    let summary = InspectorSummary::from_records(&records(&engine, view), &[]);
    assert_eq!(summary.unhandled_rejections, 2, "{summary:?}");
    // A handler added in a later task: rejectionhandled, once.
    engine.execute_script(view, "window.__late.catch(function () { __log.push('late-catch'); })").unwrap();
    pump_until(&mut engine, view, "window.__log.indexOf('handled(in-script)') >= 0");
    let log = read(&mut engine, view, "window.__log.join(' ')");
    assert!(log.ends_with("late-catch handled(in-script)"), "{log}");
}

/// A page whose listener rejects again each time, with no timer of its own: what one turn's end
/// leaves unannounced keeps the live loop turning (its next timer is due now) until every one
/// has been announced, and then the loop may sleep.
#[test]
fn rejections_a_turn_left_unannounced_keep_the_live_loop_turning_until_all_are_announced() {
    let (mut engine, view) = inspected_engine(EngineConfig::default());
    engine
        .load_html(
            view,
            "<!doctype html><html><body><script>window.__n = 0; \
             window.addEventListener('unhandledrejection', function (e) { e.preventDefault(); if (++__n < 200) Promise.reject(__n); });\
             </script></body></html>",
        )
        .expect("load_html");
    // As a script of the page's own: a host's script rejects for nobody.
    engine.execute_as_page_script(view, "Promise.reject(0)").unwrap();
    let first: u32 = read(&mut engine, view, "window.__n").parse().unwrap();
    assert!((1..200).contains(&first), "{first} announced in the first turn");
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let mut turns = 0;
    loop {
        let out = rt.block_on(engine.pump_live(view, 0));
        let n: u32 = read(&mut engine, view, "window.__n").parse().unwrap();
        if n == 200 {
            break;
        }
        turns += 1;
        assert!(turns <= 200, "{n} of 200 after {turns} live turns");
        assert_eq!(out.next_timer_ms, Some(0), "{n} of 200 announced and the loop was not asked to turn again");
    }
    let rest = rt.block_on(engine.pump_live(view, 0));
    assert_eq!(rest.next_timer_ms, None, "all announced: nothing is due");
    assert_eq!(read(&mut engine, view, "window.__n"), "200");
}
