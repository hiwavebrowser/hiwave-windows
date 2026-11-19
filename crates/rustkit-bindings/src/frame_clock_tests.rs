//! One page clock for a frame tree. A page and its same-origin frames are one event loop with one
//! timer queue, so they read one clock: `performance.now()`, the animation frame timestamp and
//! `Event.timeStamp` of every document are the same instant, each counted from its own document's
//! time origin.
//!
//! The expected relations are the pinned Chrome's (CfT 148.0.7778.216, headless, a page with a
//! same-origin `<iframe>` that starts 150 ms after it, recorded 2026-11-15;
//! `docs/diagnostics/2026-11-15/frame_clock_{parent.html,child.html,page.js,chrome.mjs,cft148.json}`):
//!   - each document has its own `performance.timeOrigin`; the frame's is later;
//!   - at one instant `parent.performance.now() - frame.performance.now()` is the difference of
//!     the two origins (the frame's reading is the smaller);
//!   - animation frame callbacks asked for in one task by the page and by the frame run in one
//!     frame, the page's first, and each gets its own document's reading of the frame's time (the
//!     two numbers differ by the origin difference; they are not equal);
//!   - an event's `timeStamp` is fixed when it is made, on the clock of the document whose
//!     constructor made it; dispatching it in the other document does not change it; `click()`
//!     in the frame is stamped on the frame's clock.
//!
//! With one document nothing changes: the first document's origin is the clock's zero.

use super::*;

fn page() -> DomBindings {
    let b = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    b.set_document(Rc::new(
        Document::parse_html("<html><head></head><body><button id=b>x</button></body></html>").unwrap(),
    ))
    .unwrap();
    b
}

/// A frame of `parent`'s tree: `child` in the page is its window, `up` in the frame the page's.
fn frame_of(parent: &DomBindings) -> DomBindings {
    let child = parent.new_child().unwrap();
    child
        .set_document(Rc::new(
            Document::parse_html("<html><head></head><body><button id=cb>c</button></body></html>").unwrap(),
        ))
        .unwrap();
    parent.expose_window("child", &child).unwrap();
    child.expose_window("up", &parent).unwrap();
    child
}

/// A page, and a frame made 150 ms of real time and 300 ms of timer time later.
fn tree() -> (DomBindings, DomBindings) {
    let parent = page();
    parent.evaluate("setTimeout(function () {}, 300);").unwrap();
    parent.run_timers(1000, 100).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(150));
    let child = frame_of(&parent);
    (parent, child)
}

fn ev(b: &DomBindings, script: &str) -> String {
    match b.evaluate(script).unwrap() {
        JsValue::String(s) => s,
        other => panic!("{other:?}"),
    }
}

/// The clock reads in a page: a number that does not go back, for `performance.now()`, a new
/// event, an event the engine fires and an animation frame.
#[test]
fn the_page_clock_reads_in_a_page() {
    let b = page();
    let got = ev(
        &b,
        "var a = performance.now(), e = new Event('x'), seen = [], frame; \
         document.getElementById('b').addEventListener('click', function (e) { seen.push(typeof e.timeStamp); }); \
         document.getElementById('b').click(); \
         requestAnimationFrame(function (t) { frame = typeof t; }); \
         [typeof a, typeof e.timeStamp, performance.now() >= a, seen.join()].join()",
    );
    assert_eq!(got, "number,number,true,number");
    b.run_timers(100, 100).unwrap();
    assert_eq!(ev(&b, "String(frame)"), "number");
}

const CAPTURE_LOG: &str = "var log = []; var btn = document.getElementById('b'); \
    btn.addEventListener('click', function (e) { log.push(e.timeStamp); }); btn.click(); log.push(performance.now()); \
    var s = performance.now(); while (performance.now() - s < 5) {} log.push(performance.now()); \
    var n = 0; requestAnimationFrame(function f(t) { log.push(t); log.push(new Event('x').timeStamp); \
    if (++n < 4) requestAnimationFrame(f); }); \
    requestAnimationFrame(function (t) { log.push(t); log.push(performance.now()); }); \
    setTimeout(function () { log.push(performance.now()); log.push(performance.now()); }, 40);";

/// One document, capture mode: every reading is what it was before the clock's state moved onto
/// the frame tree's record (the numbers are the same script's on the tree this was merged with).
#[test]
fn one_documents_capture_clock_reads_what_it_did() {
    let b = page();
    b.set_deterministic_clock(true);
    b.evaluate(CAPTURE_LOG).unwrap();
    b.run_timers(1000, 1000).unwrap();
    b.advance_timers(16, 100).unwrap();
    b.evaluate("log.push(performance.now()); requestAnimationFrame(function (t) { log.push(t); });").unwrap();
    b.advance_timers(16, 100).unwrap();
    let got = ev(&b, "JSON.stringify(log)");
    assert_eq!(got, ONE_DOCUMENT_CAPTURE);
}

/// Recorded with this test's script on the merged-in tree (batch 83, 470f0ae4), one document, then
/// with the animation frames on the 60 Hz grid: the frames at 16, 32, 48, 64 are at 17, 34, 50,
/// 67, and the frame asked for at 83 runs at the end of the 16 ms step (99) and is stamped with
/// its grid time, 84, as Chrome stamps a frame with its frame's time. The timer's readings (5.4,
/// 40) do not move.
const ONE_DOCUMENT_CAPTURE: &str = "[0.1,0.2,5.4,17,17.1,17,17.2,34,34.1,40,40.1,50,50.1,67,67.1,83,84]";

/// The page's clock read before and after one reading of the frame's, all in one script: the
/// frame's reading plus the distance between the two documents' origins lies between the two page
/// readings, so that distance lies in `[before - frame, after - frame]`. No allowance for time:
/// however long the script was held between two reads, the bracket holds.
const BRACKET_IN_PAGE: &str = "(function () { var a = performance.now(), c = child.performance.now(), b = performance.now(); \
    return [a - c, b - c, a, c, b].join(); })()";
const BRACKET_IN_FRAME: &str = "(function () { var a = up.performance.now(), c = performance.now(), b = up.performance.now(); \
    return [a - c, b - c, a, c, b].join(); })()";

fn numbers(s: String) -> Vec<f64> {
    s.split(',').map(|x| x.parse::<f64>().unwrap()).collect()
}

/// Whether two closed ranges share a point (the clock's 0.1 ms steps are sums of doubles).
fn overlap(a: (f64, f64), b: (f64, f64)) -> bool {
    a.0 <= b.1 + 1e-6 && b.0 <= a.1 + 1e-6
}

/// Chrome's `now` rows: the frame's origin is later, the frame reads less than the page at one
/// instant, and the two readings stay the same distance apart, which is where the page's clock
/// stood when the frame was made.
///
/// Chrome's distance is the difference of the two `timeOrigin`s to within 2 ms. Here it is that
/// only while the page clock has not been moved ahead of real time: a load that runs its timers
/// ahead moves the page clock with them (it is never behind the timers' clock), and a frame made
/// after that counts from the moved clock, while its `timeOrigin` is the wall clock's. This page
/// runs 300 ms of timers at once, so the distance is not compared with the `timeOrigin`s as a
/// number of milliseconds (it was, within 3 ms, and failed whenever the frame was made before
/// 300 ms of real time had passed). Every comparison below is a bracket or a floor that
/// scheduling cannot break.
#[test]
fn a_frames_clock_is_the_pages_instant_from_its_own_origin() {
    let parent = page();
    parent.evaluate("setTimeout(function () {}, 300);").unwrap();
    parent.run_timers(1000, 100).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(150));
    // The page's clock before and after the frame is made, and how long making it took.
    let before: f64 = ev(&parent, "String(performance.now())").parse().unwrap();
    let making = std::time::Instant::now();
    let child = frame_of(&parent);
    let took = making.elapsed().as_secs_f64() * 1000.0;
    let after: f64 = ev(&parent, "String(performance.now())").parse().unwrap();

    // Wall time: the sleep is a floor, whatever else took time (whole milliseconds, twice).
    let origins: f64 = ev(&parent, "String(child.performance.timeOrigin - performance.timeOrigin)").parse().unwrap();
    assert!(origins >= 148.0, "the frame's time origin is {origins} ms after the page's");

    let first = numbers(ev(&parent, BRACKET_IN_PAGE));
    let apart = (first[0], first[1]);
    assert!(first[3] >= 0.0 && apart.1 > 0.0, "the frame reads less than the page: {first:?}");
    // The frame's origin is a reading of the page's clock taken while the frame was made, less at
    // most the wall time making it took (in whole milliseconds).
    assert!(
        overlap(apart, (before - took - 1.0, after)),
        "the readings are {apart:?} ms apart; the page's clock read {before} before the frame was made and {after} after, {took:.1} ms of wall time"
    );
    // The frame's own script reads the same distance.
    let own = numbers(ev(&child, BRACKET_IN_FRAME));
    assert!(overlap(apart, (own[0], own[1])), "read in the frame {own:?}, in the page {first:?}");
    // After real time and a jump of the timers' clock, in whichever document set the timer.
    std::thread::sleep(std::time::Duration::from_millis(30));
    child.evaluate("setTimeout(function () {}, 2000);").unwrap();
    parent.run_timers(5000, 100).unwrap();
    let later = numbers(ev(&parent, BRACKET_IN_PAGE));
    // The timers' clock is at 2300 ms now, and the page clock is never behind it.
    assert!(later[2] >= 2300.0 && later[3] >= 2300.0 - after, "both moved with the timers: {later:?} after {first:?}");
    assert!(overlap(apart, (later[0], later[1])), "the readings were {apart:?} ms apart and are {:?}", (later[0], later[1]));
    assert!(overlap((own[0], own[1]), (later[0], later[1])), "read in the frame {own:?}, later in the page {later:?}");
}

/// Reading the two documents' clocks in turn does not run the clock fast, and neither goes back.
/// The wall clock is read before the first reading and after the last, in the same script, so
/// what the page clock moved is at most what the wall clock moved between them (its whole
/// milliseconds, and the page clock's 0.1 ms step), however the script was scheduled.
#[test]
fn reading_two_documents_in_turn_does_not_run_the_clock_fast() {
    let (parent, _child) = tree();
    let got = ev(
        &parent,
        "(function () { var d0 = Date.now(), s = performance.now(), p = s, c = child.performance.now(), back = 0, n = 0; \
         while (p - s < 100) { var c2 = child.performance.now(), p2 = performance.now(); \
           if (c2 < c || p2 < p) back++; c = c2; p = p2; n++; } \
         var d1 = Date.now(); \
         return [p - s, d1 - d0, back, n].join(); })()",
    );
    let parts = numbers(got.clone());
    assert_eq!(parts[2], 0.0, "a clock went back ({got})");
    assert!(parts[3] >= 2.0, "the loop read both clocks more than once ({got})");
    assert!(parts[0] <= parts[1] + 1.2, "{} ms on the page clock in {} ms of wall time ({got})", parts[0], parts[1]);
}

/// Chrome's `raf` rows, in a load and in a live turn: one frame, the page's callback first, each
/// document's own reading of the frame's time.
/// The two timestamps are as far apart as the two documents' clocks are: within the bracket of
/// a page reading before and after a frame reading (no allowance for time).
#[test]
fn one_frames_callbacks_get_each_documents_reading_of_one_time() {
    for live in [false, true] {
        let (parent, _child) = tree();
        parent
            .evaluate(
                "var got = { order: [] }; var apart = (function () { var a = performance.now(), c = child.performance.now(), b = performance.now(); return [a - c, b - c]; })(); \
                 requestAnimationFrame(function (t) { got.p = t; got.pNow = performance.now(); got.order.push('parent'); }); \
                 child.requestAnimationFrame(function (t) { got.c = t; got.cNow = child.performance.now(); got.order.push('frame'); }); \
                 child.requestAnimationFrame(function (t) { got.c2 = t; }); requestAnimationFrame(function (t) { got.p2 = t; });",
            )
            .unwrap();
        if live {
            // To the next point of the 60 Hz frame grid: at most 17 ms on (300 is a grid point;
            // the frame is due at 317).
            std::thread::sleep(std::time::Duration::from_millis(17));
            parent.advance_timers(17, 100).unwrap();
        } else {
            parent.run_timers(1000, 100).unwrap();
        }
        let got = ev(
            &parent,
            "[got.p === got.c, got.p - got.c >= apart[0] - 1e-6 && got.p - got.c <= apart[1] + 1e-6, got.pNow >= got.p && got.cNow >= got.c, \
              got.p2 === got.p && got.c2 === got.c, got.c >= 0, got.order.join('+')].join()",
        );
        assert_eq!(
            got, "false,true,true,true,true,parent+frame",
            "live {live}: equal numbers, apart by the origins, not after each document's now, one stamp a document, the frame's from its origin, order"
        );
    }
}

/// Chrome's `ev` rows. An event's `timeStamp` is a reading of its maker's clock: it lies between
/// a reading of that document's clock taken before the event was made and one taken after (no
/// allowance for time), and stays what it was wherever the event is dispatched.
#[test]
fn an_events_time_stamp_is_on_its_makers_clock_and_stays() {
    let (parent, _child) = tree();
    let got = ev(
        &parent,
        "(function () { var out = [], within = function (lo, x, hi) { return lo <= x && x <= hi; }; \
         var p0 = performance.now(), e1 = new Event('x'), p1 = performance.now(); \
         out.push('made by the page: ' + within(p0, e1.timeStamp, p1)); \
         var c0 = child.performance.now(), e2 = new child.Event('x'), c1 = child.performance.now(); \
         out.push('made with the frame\\'s constructor: ' + within(c0, e2.timeStamp, c1)); \
         var cb = child.document.getElementById('cb'), seen; \
         cb.addEventListener('x', function (e) { seen = e.timeStamp; }); cb.dispatchEvent(e1); \
         out.push('the page\\'s, dispatched in the frame: ' + (seen === e1.timeStamp)); \
         var pb = document.getElementById('b'), pseen; \
         pb.addEventListener('x', function (e) { pseen = e.timeStamp; }); pb.dispatchEvent(e2); \
         out.push('the frame\\'s, dispatched in the page: ' + (pseen === e2.timeStamp)); \
         var click; cb.addEventListener('click', function (e) { click = e.timeStamp; }); \
         var k0 = child.performance.now(); cb.click(); var k1 = child.performance.now(); \
         out.push('click() in the frame: ' + within(k0, click, k1)); \
         return out.join('; '); })()",
    );
    assert_eq!(
        got,
        "made by the page: true; made with the frame's constructor: true; \
         the page's, dispatched in the frame: true; the frame's, dispatched in the page: true; \
         click() in the frame: true"
    );
}

/// An event the engine fires in a document is stamped on that document's clock (Chrome: `load`
/// at the frame's window reads the frame's clock, at the `<iframe>` element the page's): between
/// a reading of that clock before the engine fired it and the listener's reading of it.
#[test]
fn an_event_the_engine_fires_is_on_its_documents_clock() {
    let (parent, child) = tree();
    let listen = "var seen, before; addEventListener('load', function (e) { seen = [e.timeStamp, performance.now()]; });";
    child.evaluate(listen).unwrap();
    parent.evaluate(listen).unwrap();
    child.evaluate("before = performance.now();").unwrap();
    child.fire_lifecycle_event(LifecycleTarget::Window, "load").unwrap();
    parent.evaluate("before = performance.now();").unwrap();
    parent.fire_lifecycle_event(LifecycleTarget::Window, "load").unwrap();
    let within = "String(before <= seen[0] && seen[0] <= seen[1])";
    assert_eq!((ev(&child, within), ev(&parent, within)), ("true".to_string(), "true".to_string()), "between its own document's readings: in the frame, in the page");
}

/// What a page and its frame log in capture mode: both documents' readings, events, frames and
/// timers, and a wait on the frame's clock.
const TREE_LOG: &str = "var log = []; \
    log.push(performance.now()); log.push(child.performance.now()); log.push(new child.Event('x').timeStamp); log.push(new Event('x').timeStamp); \
    var s = child.performance.now(); while (child.performance.now() - s < 5) {} log.push(child.performance.now()); log.push(performance.now()); \
    var n = 0; child.requestAnimationFrame(function f(t) { log.push(t); log.push(child.performance.now()); if (++n < 3) child.requestAnimationFrame(f); }); \
    requestAnimationFrame(function (t) { log.push(t); log.push(performance.now()); }); \
    child.setTimeout(function () { log.push(child.performance.now()); log.push(performance.now()); }, 40);";

/// Capture mode with a frame: no real time in either document's clock, so two runs log the same
/// numbers however long each took, and a wait on the frame's clock ends. The frame is made after
/// the switch, as the engine makes it: the switch is the tree's.
#[test]
fn in_capture_mode_a_frame_trees_clock_reproduces_across_runs() {
    let run = |wait: u64| {
        let parent = page();
        parent.set_deterministic_clock(true);
        parent.evaluate("setTimeout(function () {}, 300);").unwrap();
        parent.run_timers(1000, 100).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(wait));
        let child = frame_of(&parent);
        parent.evaluate(TREE_LOG).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(wait));
        parent.run_timers(1000, 1000).unwrap();
        child.advance_timers(16, 100).unwrap();
        parent.advance_timers(16, 100).unwrap();
        ev(&parent, "log.push(child.performance.now()); JSON.stringify(log)")
    };
    let first = run(10);
    let second = run(70);
    assert_eq!(first, second);
    let log: Vec<f64> = serde_json::from_str(&first).unwrap();
    assert_eq!(log.len(), 17, "{first}");
    // The frame's readings count from its own origin: the page's first is past the 300 ms the
    // timers ran before the frame was made, the frame's is not.
    assert!(log[0] >= 300.0 && log[1] < 10.0, "{first}");
}

/// The capture switch thrown after a frame exists reaches the frame, from either document.
#[test]
fn the_capture_switch_reaches_every_document_of_the_tree() {
    let apart = |b: &DomBindings| -> f64 {
        let a: f64 = ev(b, "String(performance.now())").parse().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(40));
        let z: f64 = ev(b, "String(performance.now())").parse().unwrap();
        z - a
    };
    let (parent, child) = tree();
    assert!(apart(&child) >= 35.0, "the frame's clock flows before the switch");
    parent.set_deterministic_clock(true);
    let (in_frame, in_page) = (apart(&child), apart(&parent));
    assert!(in_frame < 5.0 && in_page < 5.0, "after the switch the frame moved {in_frame} ms and the page {in_page} ms across 40 ms");
    child.set_deterministic_clock(false);
    let (in_frame, in_page) = (apart(&child), apart(&parent));
    assert!(in_frame >= 35.0 && in_page >= 35.0, "switched back from the frame: the frame moved {in_frame} ms and the page {in_page} ms");
}
