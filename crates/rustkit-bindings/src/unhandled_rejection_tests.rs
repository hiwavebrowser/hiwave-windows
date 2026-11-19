//! `unhandledrejection` and `rejectionhandled`: a promise rejected with no handler by the end of
//! its task's microtasks is announced at the Window, and one that gets a handler later is
//! announced again.
//!
//! The expected values are the pinned Chrome's (CfT 148.0.7778.216, headless, 2026-11-15; three
//! runs gave the same file): the probe page `docs/diagnostics/2026-11-15/unhandled_rejection.html`
//! with `unhandled_rejection_page.js` was run there by `unhandled_rejection_chrome.mjs`, and its
//! answers are `unhandled_rejection_cft148.json`. The tests run the same script here, one timer a
//! turn, and compare scenario by scenario. What Chrome does:
//!   - the event comes after the microtasks of the task in which the promise was rejected, in the
//!     order the promises were rejected, with a microtask checkpoint after each event;
//!   - it is a trusted, cancelable `PromiseRejectionEvent` at the Window (it does not bubble; a
//!     listener on the document hears nothing) whose `promise` and `reason` are the page's own;
//!   - a promise that got a handler in the same task, or in one of its microtasks, has no event;
//!   - `preventDefault()`, or `false` returned from `onunhandledrejection`, keeps the rejection
//!     out of the console; everything else is reported after its listeners ran;
//!   - a handler added in a later task brings one `rejectionhandled` (not cancelable), after that
//!     task's microtasks; a handler added inside the `unhandledrejection` listener brings none.
//!
//!   - every rejection of a task is announced, however many (100 and 300 in one task; 100 made
//!     inside a listener; a chain of 40, each made by the listener of the one before).
//!
//! Where this engine still answers otherwise is listed in [`DIFFERENT`], each with its reason.
//! One more difference has no Chrome side to pin: a turn the script watchdog stopped announces
//! nothing, and its rejections are announced (and reported) at the end of the next turn.

use super::*;

const SCRIPT: &str = include_str!("../../../docs/diagnostics/2026-11-15/unhandled_rejection_page.js");
const CHROME: &str = include_str!("../../../docs/diagnostics/2026-11-15/unhandled_rejection_cft148.json");

/// Scenarios that do not answer as Chrome does, and why. Every other scenario of the probe must.
const DIFFERENT: &[(&str, &str)] = &[
    (
        "order.script",
        "Chrome queues the event as a task, behind a 0 ms timer the same task had already set; here the event \
         comes at the end of the turn, before that timer",
    ),
    ("order.raf", "as order.script: the 0 ms timer set in the frame callback runs after the event here, before it in Chrome"),
    (
        "later.handledInAListener",
        "a handler added inside a listener to a promise announced before: its rejectionhandled comes in the same \
         round here, ahead of the events still to come; Chrome queues it as a task, after them",
    ),
    (
        "worker",
        "a worker's runtime announces nothing (Chrome fires unhandledrejection at the WorkerGlobalScope); the \
         scenario is not run in this test (`window.__noWorkers`)",
    ),
    (
        "listeners.order",
        "an on<type> handler runs after the listeners here, whenever it was assigned (every event type); \
         Chrome runs it in the order it was first assigned among them",
    ),
    (
        "listeners.elsewhere",
        "an onunhandledrejection attribute on <body> is the Window's handler here (HTML's body forwarding); \
         Chrome 148 does not forward this one",
    ),
];

fn object(json: &str) -> serde_json::Map<String, serde_json::Value> {
    match serde_json::from_str(json).expect("json") {
        serde_json::Value::Object(map) => map,
        other => panic!("not an object: {other}"),
    }
}

fn page() -> DomBindings {
    let b = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    b.set_document(Rc::new(Document::parse_html("<!doctype html><html><head></head><body></body></html>").unwrap()))
        .unwrap();
    b
}

/// The scripts of these tests stand for a page's own: they are evaluated as page scripts
/// (`evaluate_as_page_script`), not as the host's, whose rejections are not the page's to hear.
fn ev(b: &DomBindings, script: &str) -> String {
    match b.evaluate_as_page_script(script).unwrap() {
        JsValue::String(s) => s,
        other => panic!("{other:?}"),
    }
}

/// Run the page's timers one a turn (each turn ends with its microtasks, as a task does) until
/// `done` is true. The bound is a count of turns, not a time.
fn turns_until(b: &DomBindings, done: &str) {
    for _ in 0..5000 {
        if ev(b, &format!("String(!!({done}))")) == "true" {
            return;
        }
        b.run_timers(3_600_000, 1).unwrap();
    }
    panic!("`{done}` was not true after 5000 turns");
}

/// What the engine reported as an unhandled rejection, as a browser's console names it (an
/// error's message, else the value's text), in two lists: those a console shows, and those whose
/// event the page canceled, which the engine keeps with a mark for its own log only.
fn reported(b: &DomBindings) -> (Vec<String>, Vec<String>) {
    let (canceled, shown): (Vec<_>, Vec<_>) = b.take_error_reports().into_iter().filter(|r| r.rejection).partition(|r| r.canceled);
    let names = |reports: Vec<rustkit_js::ErrorReport>| reports.into_iter().map(|r| r.message.unwrap_or_else(|| r.value.to_string())).collect();
    (names(shown), names(canceled))
}

/// The probe here: each scenario's answer, and the rejections reported, in order (shown, canceled).
fn ours() -> (serde_json::Map<String, serde_json::Value>, (Vec<String>, Vec<String>)) {
    let b = page();
    b.set_error_details(true, true).unwrap();
    b.evaluate_as_page_script("window.__noWorkers = true;").unwrap();
    b.evaluate_as_page_script(SCRIPT).unwrap();
    turns_until(&b, "window.__out");
    (object(&ev(&b, "window.__out")), reported(&b))
}

#[test]
fn rejection_events_come_as_in_chrome() {
    let pin = object(CHROME);
    assert_eq!(pin["browser"], "148.0.7778.216");
    let chrome = pin["out"].as_object().expect("out");
    let (ours, _) = ours();
    let mut wrong = Vec::new();
    let mut fixed = Vec::new();
    for (scenario, want) in chrome {
        let got = ours.get(scenario);
        let listed = DIFFERENT.iter().any(|(name, _)| name == scenario);
        match (got == Some(want), listed) {
            (true, false) | (false, true) => {}
            (true, true) => fixed.push(scenario.as_str()),
            (false, false) => {
                wrong.push(format!("{scenario}: chrome {want}, here {}", got.map_or("nothing".to_string(), |g| g.to_string())))
            }
        }
    }
    assert!(wrong.is_empty(), "{} of {} scenarios differ from Chrome:\n{}", wrong.len(), chrome.len(), wrong.join("\n"));
    assert!(fixed.is_empty(), "these now answer as Chrome does; take them off DIFFERENT: {fixed:?}");
    for (name, _) in DIFFERENT {
        assert!(chrome.contains_key(*name), "{name} is not a scenario of the probe");
    }
}

/// The console's side: what Chrome reported as uncaught (`pageErrors`), less the one exception a
/// listener threw, is the rejections reported here without the mark, in the same order. A
/// rejection whose event was canceled is not in Chrome's; here it is reported with the mark.
#[test]
fn a_rejection_is_reported_unless_its_event_was_canceled() {
    let pin = object(CHROME);
    let chrome: Vec<String> = pin["pageErrors"]
        .as_array()
        .expect("pageErrors")
        .iter()
        .map(|m| m.as_str().expect("text").to_string())
        .filter(|m| m != "listener-threw")
        .collect();
    assert!(chrome.contains(&"reported-N".to_string()) && chrome.contains(&"L4".to_string()), "{chrome:?}");
    for canceled in ["prevented-P", "prevented-F", "L2"] {
        assert!(!chrome.contains(&canceled.to_string()), "{canceled} was canceled in Chrome: {chrome:?}");
    }
    let (_, (shown, marked)) = ours();
    assert_eq!(shown, chrome);
    // The marked ones are the probe's canceled rejections (these three and its
    // flood's), and none of them is one Chrome reported.
    for canceled in ["prevented-P", "prevented-F", "L2"] {
        assert!(marked.contains(&canceled.to_string()), "{canceled} is not marked: {marked:?}");
    }
    assert!(marked.iter().all(|name| !chrome.contains(name)), "{marked:?}");
}

/// The events do not wait for an embedder that records errors: with error details off (the
/// default) a listener still hears the rejection, and nothing is kept for a report.
#[test]
fn the_events_fire_with_error_reports_off() {
    let b = page();
    b.evaluate_as_page_script(
        "var log = []; \
         window.addEventListener('unhandledrejection', function (e) { log.push('unhandled:' + e.reason); }); \
         window.addEventListener('rejectionhandled', function (e) { log.push('handled:' + e.reason); }); \
         var p = Promise.reject('r1'); Promise.resolve().then(function () { log.push('micro'); }); log.push('sync');",
    )
    .unwrap();
    assert_eq!(ev(&b, "log.join()"), "sync,micro,unhandled:r1");
    b.evaluate_as_page_script("p.catch(function () { log.push('caught'); });").unwrap();
    assert_eq!(ev(&b, "log.join()"), "sync,micro,unhandled:r1,caught,handled:r1");
    assert!(b.take_error_reports().is_empty());
}

/// A rejection that is handled before its task ends is not reported and has no event; one whose
/// event nobody cancels is reported once.
#[test]
fn reports_are_as_before_for_a_page_with_no_listener() {
    let b = page();
    b.set_error_details(true, true).unwrap();
    b.evaluate_as_page_script("Promise.reject(new Error('handled')).catch(function () {}); Promise.reject(new RangeError('left'));").unwrap();
    assert_eq!(reported(&b), (vec!["left".to_string()], Vec::new()));
    assert_eq!(reported(&b), (Vec::new(), Vec::new()), "reported once");
    // A handler added later does not report it again, and takes nothing back.
    b.evaluate_as_page_script("Promise.reject(new Error('late')).catch(function () {});").unwrap();
    assert_eq!(reported(&b), (Vec::new(), Vec::new()));
}

/// Announcing a rejection calls none of these when a page has replaced them: the event
/// constructors, `dispatchEvent` (the Window's and EventTarget's), `Reflect.apply` and
/// `Reflect.construct`, `WeakMap.prototype.set`, `WeakSet.prototype.add` and `delete`, and the
/// Promise methods. Nothing else is claimed here.
#[test]
fn announcing_calls_no_replaced_event_constructor_dispatch_reflect_weak_write_or_promise_method() {
    let b = page();
    b.evaluate_as_page_script(
        "var calls = 0, seen = []; function spy() { calls++; throw new Error('replaced'); } \
         window.addEventListener('unhandledrejection', function (e) { \
             seen.push([e.type, e.isTrusted, e.reason, e.promise === p, String(e)].join(':')); }); \
         window.PromiseRejectionEvent = spy; window.Event = spy; window.dispatchEvent = spy; \
         EventTarget.prototype.dispatchEvent = spy; \
         Reflect.apply = spy; Reflect.construct = spy; \
         WeakMap.prototype.set = spy; WeakSet.prototype.add = spy; WeakSet.prototype.delete = spy; \
         Promise.prototype.then = spy; Promise.prototype.catch = spy; Promise.resolve = spy; Array.prototype.push = [].push; \
         var p = Promise.reject('r');",
    )
    .unwrap();
    assert_eq!(ev(&b, "seen.join() + ' calls=' + calls"), "unhandledrejection:true:r:true:[object PromiseRejectionEvent] calls=0");
}

/// A frame's rejection is announced at the frame's Window, and the page's at the page's.
#[test]
fn a_rejection_is_announced_in_the_document_whose_script_rejected() {
    let parent = page();
    let child = parent.new_child().unwrap();
    child
        .set_document(Rc::new(Document::parse_html("<html><head></head><body></body></html>").unwrap()))
        .unwrap();
    parent.expose_window("child", &child).unwrap();
    parent.evaluate_as_page_script("var log = []; window.addEventListener('unhandledrejection', function (e) { log.push('page:' + e.reason); });").unwrap();
    child
        .evaluate_as_page_script("var log = []; window.addEventListener('unhandledrejection', function (e) { log.push('frame:' + e.reason + ':' + (e.target === window)); });")
        .unwrap();
    child.evaluate_as_page_script("Promise.reject('in-frame');").unwrap();
    parent.evaluate_as_page_script("Promise.reject('in-page');").unwrap();
    assert_eq!(ev(&child, "log.join()"), "frame:in-frame:true");
    assert_eq!(ev(&parent, "log.join()"), "page:in-page");
}

/// Every rejection of one task is announced at the end of that task, however many.
#[test]
fn every_rejection_of_a_task_is_announced_at_its_end() {
    for count in [100, 300, 3000] {
        let b = page();
        b.evaluate_as_page_script(&format!(
            "var seen = []; window.addEventListener('unhandledrejection', function (e) {{ seen.push(e.reason); }}); \
             for (var i = 0; i < {count}; i++) Promise.reject(i);"
        ))
        .unwrap();
        assert_eq!(ev(&b, "seen.length + ':' + seen[0] + ':' + seen[seen.length - 1]"), format!("{count}:0:{}", count - 1));
    }
}

/// Rejections made by the listeners are announced too, round after round. A page that rejects
/// again in every listener call is stopped by a count of rounds, not left running: the turn ends,
/// the engine asks for another turn at once (a timer of its own that is due now), and the rest
/// are announced in the turns that follow. None is dropped and none waits for the page to do
/// something else.
#[test]
fn a_listener_that_rejects_again_each_time_does_not_hold_the_turn_and_loses_nothing() {
    let b = page();
    b.evaluate_as_page_script(
        "var n = 0; window.addEventListener('unhandledrejection', function (e) { e.preventDefault(); n++; if (n < 200) Promise.reject(n); }); \
         Promise.reject(0);",
    )
    .unwrap();
    let first: u32 = ev(&b, "String(n)").parse().unwrap();
    assert!((1..200).contains(&first), "{first} events in the first turn");
    // Nothing of the page's is due, yet a turn is asked for now.
    assert!(matches!(b.host_turn("return __rk.nextTimer();", &[]).unwrap(), JsValue::Number(n) if n == 0.0));
    let mut turns = 0;
    while ev(&b, "String(n)") != "200" {
        turns += 1;
        assert!(turns <= 200, "still {} after {turns} more turns", ev(&b, "String(n)"));
        assert!(b.run_timers(0, 1).unwrap() >= 1, "no turn was asked for with rejections still to announce");
    }
    // All announced: no turn is asked for any more.
    b.run_timers(0, 10).unwrap();
    assert!(matches!(b.host_turn("return __rk.nextTimer();", &[]).unwrap(), JsValue::Number(n) if n == -1.0));
}

/// A hundred made inside one listener call are one round: all in the same turn.
#[test]
fn rejections_made_inside_a_listener_are_announced_in_the_same_turn() {
    let b = page();
    b.evaluate_as_page_script(
        "var seen = 0; window.addEventListener('unhandledrejection', function (e) { seen++; \
             if (e.reason === 'seed') for (var i = 0; i < 100; i++) Promise.reject(i); }); \
         Promise.reject('seed');",
    )
    .unwrap();
    assert_eq!(ev(&b, "String(seen)"), "101");
}
