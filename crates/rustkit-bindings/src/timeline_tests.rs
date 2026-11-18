//! TIMELINE1: `document.timeline` and Web Animations on the page clock, as Chrome's. Pinned on
//! Chrome for Testing 148.0.7778.216, headless, five runs of
//! `docs/diagnostics/2026-11-15/timeline_probe.html` (`timeline_probe_cft148.json`):
//!
//! - In every frame callback `document.timeline.currentTime === t` ("frames", "inFrame").
//! - In a task right after a frame it is that frame's `t`, the same twice 5 ms apart ("task",
//!   "twoTasks"); in a task after 300 ms with no frame, a grid point 0.6-16.1 ms before
//!   `performance.now()`, the same 5 ms later ("idle").
//! - `a.startTime = timeline.currentTime` in a task: `a.currentTime` 0 at once, then exactly
//!   `t - a.startTime` in each frame ("startFrom").
//! - `pause()` at ~100 ms: `currentTime` 100 and "paused" at once, 116.6-116.7 three frames on,
//!   `startTime` null ("paused"): the hold time is taken at the next frame.
//! - A 50 ms animation 200 ms on: 50, "finished", promise settled, `startTime` kept ("finished").
//! - A frame a long task held up: the timeline is its `t` and an animation's `currentTime` is
//!   `t - startTime` ("held").
//!
//! The tests turn the clock by hand on the deterministic page clock (no wall clock).

use super::*;

fn page() -> DomBindings {
    let b = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    b.set_document(Rc::new(
        Document::parse_html("<html><body><div id=box></div></body></html>").unwrap(),
    ))
    .unwrap();
    b.set_deterministic_clock(true);
    b
}

fn read(b: &DomBindings, v: &str) -> String {
    match b.evaluate(&format!("String({v})")).unwrap() {
        JsValue::String(s) => s,
        other => panic!("{other:?}"),
    }
}

fn num(b: &DomBindings, v: &str) -> f64 {
    let s = read(b, v);
    s.parse().unwrap_or_else(|_| panic!("{v} = {s:?}"))
}

fn turn(b: &DomBindings, ms: u64) {
    b.advance_timers_for_frame(ms, 1000, 0).unwrap();
}

/// Inside a frame callback the timeline is the callback's timestamp, also when the turn ran the
/// frame late: due at 17, run at 20, both 17 (Chrome: equal in every frame). It read the turn's
/// time, 20, before (#1191's `known_divergence_timeline_current_time_is_not_the_frame_stamp_yet`).
#[test]
fn the_timeline_is_the_frame_stamp_inside_a_frame() {
    let b = page();
    b.evaluate("var got = ''; requestAnimationFrame(function (t) { got = t + ',' + document.timeline.currentTime; });")
        .unwrap();
    turn(&b, 20);
    assert_eq!(read(&b, "got"), "17,17");
}

/// A task after a frame reads that frame's time, the same however much the page clock moves in the
/// task (Chrome "task" and "twoTasks": the frame's `t`, twice). It read the turn's time.
#[test]
fn a_task_after_a_frame_reads_that_frames_time() {
    let b = page();
    b.evaluate(
        "var T = -1, got = ''; requestAnimationFrame(function (t) { T = t; setTimeout(function () { \
           var a = document.timeline.currentTime; performance.now(); performance.now(); \
           got = [a, document.timeline.currentTime].join(); }, 3); });",
    )
    .unwrap();
    turn(&b, 17);
    turn(&b, 3);
    let t = read(&b, "T");
    assert_eq!(read(&b, "got"), format!("{t},{t}"));
}

/// A task after a long idle reads the grid point at or before its time, not the last frame's
/// (Chrome "idle": 0.6-16.1 ms before `performance.now()` after 300 ms with no frame), the same
/// twice; and the frame it then asks for, the frame of that grid point, is stamped with it (Chrome:
/// in 15 of 25 idle rows; in the other 10 the frame is the next grid point, +16.6, the request
/// landing after Chrome's tick, the shape of the named long-callback divergence). Here a frame at
/// 17, then a timer at 1005: the grid point 1000.
#[test]
fn an_idle_task_reads_the_grid_point_before_it() {
    let b = page();
    b.evaluate(
        "var got = '', inFrame = ''; requestAnimationFrame(function () {}); setTimeout(function () { \
           var p = performance.now(), a = document.timeline.currentTime; performance.now(); \
           got = [p - a < 17 && a <= p, a, document.timeline.currentTime].join(); \
           requestAnimationFrame(function (t) { inFrame = [t, document.timeline.currentTime].join(); }); }, 1005);",
    )
    .unwrap();
    turn(&b, 17);
    turn(&b, 988);
    let got = read(&b, "got");
    let parts: Vec<&str> = got.split(',').collect();
    assert_eq!(parts[0], "true", "{got}");
    assert_eq!(parts[1], parts[2], "{got}");
    assert_eq!(parts[1].parse::<f64>().unwrap().floor(), 1000.0, "{got}");
    assert_eq!(read(&b, "inFrame"), format!("{0},{0}", parts[1]));
}

/// `startTime` set from the timeline in a task: `currentTime` is 0 at once and then exactly
/// `t - startTime` in each frame (Chrome "startFrom": 0, then 16.57-16.67, 33.27-33.37, ...,
/// equal to `t - startTime` to 0.01).
#[test]
fn a_start_time_from_the_timeline_has_no_jump() {
    let b = page();
    b.evaluate(
        "var box = document.getElementById('box'), once = '', rows = []; requestAnimationFrame(function () {}); \
         setTimeout(function () { var a = box.animate({ opacity: [0, 1] }, { duration: 1000 }); \
           a.startTime = document.timeline.currentTime; \
           once = [a.currentTime, a.startTime - document.timeline.currentTime].join(); \
           (function f() { requestAnimationFrame(function (t) { \
             rows.push(Math.abs(a.currentTime - (t - a.startTime)) < 0.01 && a.currentTime > 0); \
             if (rows.length < 4) f(); }); })(); }, 30);",
    )
    .unwrap();
    for ms in [17, 13, 4, 17, 16, 17, 17] {
        turn(&b, ms);
    }
    assert_eq!(read(&b, "once"), "0,0");
    assert_eq!(read(&b, "rows.join()"), "true,true,true,true");
}

/// `pause()` reads the current time at once and is "paused" at once, but the hold time is taken at
/// the next frame (Chrome "paused": 100 at the pause, 116.6-116.7 three frames on, `startTime`
/// null). It held the time read at the pause.
#[test]
fn a_pause_takes_its_hold_time_at_the_next_frame() {
    let b = page();
    b.evaluate(
        "var box = document.getElementById('box'), a = box.animate({ opacity: [0, 1] }, { duration: 1000 }), \
           at = '', later = '', S = -1, next = -1, n = 0; \
         setTimeout(function () { S = a.startTime; a.pause(); at = [a.currentTime, a.playState].join(); \
           (function f() { requestAnimationFrame(function (t) { if (next < 0) next = t; \
             if (++n < 3) f(); else later = [a.currentTime, a.playState, String(a.startTime)].join(); }); })(); }, 105);",
    )
    .unwrap();
    for _ in 0..12 {
        turn(&b, b.next_timer_delay().unwrap_or(17).max(1));
    }
    let at = read(&b, "at");
    let (s, next) = (num(&b, "S"), num(&b, "next"));
    assert!(at.ends_with(",paused"), "{at}");
    assert!(at.split(',').next().unwrap().parse::<f64>().unwrap() < next - s, "{at}");
    assert_eq!(read(&b, "later"), format!("{},paused,null", next - s));
}

/// A finished animation's reads (Chrome "finished": 50, "finished", the promise settled, the start
/// time kept). A guard: this already held.
#[test]
fn a_finished_animation_reads_its_end() {
    let b = page();
    b.evaluate(
        "var box = document.getElementById('box'), a = box.animate({ opacity: [0, 1] }, { duration: 50 }), \
           settled = false, got = ''; a.finished.then(function () { settled = true; }); \
         setTimeout(function () { got = [a.currentTime, a.playState, settled, a.startTime !== null].join(); }, 200);",
    )
    .unwrap();
    for _ in 0..20 {
        match b.next_timer_delay() {
            Some(d) => turn(&b, d.max(1)),
            None => break,
        }
    }
    assert_eq!(read(&b, "got"), "50,finished,true,true");
}

/// A frame a long task held up: the timeline is its timestamp and a running animation's
/// `currentTime` is `t - startTime` (Chrome "held": T0 + 16.6-16.7 for both, difference 0), and
/// the frame its own callback asks for is the frame of the time it runs at, the latest grid point
/// at or before it (Chrome: T0 + 99.9-100.1 when the frame ran ~101 ms after T0). Here the frame
/// at 17 (T0); the next, due 34, run by a turn at 120: stamped T0 + 17; its own request gets
/// grid point 117 (T0 + 100). It got the next grid point after the turn, 134 (T0 + 117).
#[test]
fn a_frame_held_up_by_a_long_task_reads_its_stamp_and_asks_for_the_frame_of_its_time() {
    let b = page();
    b.evaluate(
        "var box = document.getElementById('box'), a = box.animate({ opacity: [0, 1] }, { duration: 5000 }), \
           T0 = -1, row = ''; \
         requestAnimationFrame(function (t0) { T0 = t0; requestAnimationFrame(function (t) { \
           row = [t - T0, document.timeline.currentTime - T0, a.currentTime - (t - a.startTime)].join(); \
           requestAnimationFrame(function (t2) { row += ',' + (t2 - T0); }); }); });",
    )
    .unwrap();
    turn(&b, 17);
    turn(&b, 103);
    if read(&b, "row").split(',').count() < 4 {
        let d = b.next_timer_delay().expect("the next frame is asked for");
        turn(&b, d);
    }
    let row: Vec<i64> = read(&b, "row")
        .split(',')
        .map(|s| s.parse::<f64>().unwrap().round() as i64)
        .collect();
    assert_eq!(row, vec![17, 17, 0, 100], "Chrome: 16.7, 16.7, 0, 100");
}

/// KNOWN DIVERGENCE, not modelled: a frame callback that works 40 or 100 ms and then asks for a
/// frame gets, in Chrome, the latest grid point at or before the request (`timeline_probe`
/// "stalled": t + 33.3, sometimes 50, after 40 ms; t + 100 after 100 ms), run at once. A turn's
/// clock does not move while its callbacks run, so here the request is the frame after this one
/// (t + 17), run late on the next turn, and its own request then catches up.
#[test]
fn known_divergence_a_request_after_a_long_callback_is_the_next_frame() {
    let b = page();
    b.evaluate(
        "var T = -1, got = -1; requestAnimationFrame(function (t) { T = t; \
           var s = performance.now(); while (performance.now() - s < 40) {} \
           requestAnimationFrame(function (t2) { got = t2 - T; }); });",
    )
    .unwrap();
    turn(&b, 17);
    turn(&b, 40);
    assert_eq!(num(&b, "got").round() as i64, 17, "Chrome: 33.3");
}

/// The first frame a page asks for, from a task at 20 ms: the frame of that time, grid point 17,
/// run at once (Chrome: a task's frame is stamped up to 16.7 ms before it, `frame_stamp_probe`
/// "idle" and "sweep"). #1191 R1 follow-up: the case on its own.
#[test]
fn a_first_request_at_20_is_the_frame_at_17_run_at_once() {
    let b = page();
    b.evaluate("var got = -1; setTimeout(function () { requestAnimationFrame(function (t) { got = t; }); }, 20);")
        .unwrap();
    turn(&b, 20);
    assert_eq!(read(&b, "got"), "17");
}

/// Captures stay reproducible on the real-flowing page clock (parity-capture's default): when the
/// timers run ahead of real time (a load's run-ahead, a stepped capture), a frame is stamped with
/// its exact grid time, so the timeline and every script animation's time carry no real time. Here
/// a timer at 5000 reads `performance.now()` and works 3 ms of real time, then asks for a frame:
/// the frame of 5000, stamped 5000 exactly. It was 5000 plus the real time since the clock's
/// reading at 5000 (about 3 ms, a different number every run).
#[test]
fn a_frame_run_ahead_of_real_time_is_stamped_with_its_exact_grid_time() {
    let b = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    b.set_document(Rc::new(
        Document::parse_html("<html><body></body></html>").unwrap(),
    ))
    .unwrap();
    b.evaluate(
        "var got = ''; setTimeout(function () { var s = performance.now(); while (performance.now() - s < 3) {} \
           requestAnimationFrame(function (t) { got = [t, document.timeline.currentTime].join(); }); }, 5000);",
    )
    .unwrap();
    turn(&b, 5000);
    assert_eq!(read(&b, "got"), "5000,5000");
}

/// While a frame is overdue the timeline does not move on in a task: a task that runs before the
/// late frame reads the last frame's time, and the frame then reads its own (Chrome `timeline_probe`
/// "overdue", five runs: a task run before a frame a 100 ms task held up reads T0 + 0, the frame
/// T0 + 16.6-16.8; a task run after it reads T0 + 99.9-100). The page never sees the timeline go
/// back. Here the frame at 17 (T0) asks for the next (due 34) and sets a timer due at 50, after
/// the frame's time; the turn comes at 140 and runs the timer first: it reads T0 + 0, the frame
/// T0 + 17. The timer read the grid point at or before its time, then the frame 17.
#[test]
fn a_task_before_an_overdue_frame_reads_the_last_frames_time() {
    let b = page();
    b.evaluate(
        "var T0 = -1, rows = []; requestAnimationFrame(function (t0) { T0 = t0; \
           requestAnimationFrame(function (t) { rows.push('f:' + (t - T0) + ':' + (document.timeline.currentTime - T0)); }); \
           setTimeout(function () { rows.push('t:' + (document.timeline.currentTime - T0)); }, 33); });",
    )
    .unwrap();
    turn(&b, 17);
    turn(&b, 123);
    assert_eq!(read(&b, "rows.join()"), "t:0,f:17:17");
}

/// `pause()` then a playback rate change in the same task keeps the current time and holds it
/// (Chrome `timeline_probe` "pauseRate", three runs of five: the time at the pause, the same after
/// `playbackRate = 2`, the same three frames on, "paused", rate 2). It went from 100 to 200.
#[test]
fn a_rate_change_after_pause_keeps_the_current_time() {
    let b = page();
    b.evaluate(
        "var box = document.getElementById('box'), a = box.animate({ opacity: [0, 1] }, { duration: 1000 }), \
           got = '', later = '', n = 0; \
         setTimeout(function () { a.pause(); var p = a.currentTime; a.playbackRate = 2; \
           got = [p, a.currentTime].join(); \
           (function f() { requestAnimationFrame(function () { if (++n < 3) f(); \
             else later = [a.currentTime, a.playState, a.playbackRate].join(); }); })(); }, 105);",
    )
    .unwrap();
    for _ in 0..14 {
        turn(&b, b.next_timer_delay().unwrap_or(17).max(1));
    }
    let got = read(&b, "got");
    let p = got.split(',').next().unwrap().to_string();
    assert_eq!(got, format!("{p},{p}"));
    assert_eq!(read(&b, "later"), format!("{p},paused,2"));
}

/// Grid frame stamps (a load-only capture): with real time ahead of the timers' clock (a slow
/// capture turn), the timeline in a frame is still its exact grid time, so a script animation reads
/// the same time every run; the callback's stamp stays on the clock `performance.now()` reads (at
/// or after the grid time), so a page pairing the two keeps the relation it had (R1). Here 40 ms of
/// real time pass before the turn to 17: the timeline reads 17, the stamp the real time. On the
/// flowing clock alone the timeline read the real time too, about 40.
#[test]
fn grid_frame_stamps_carry_no_real_time() {
    let b = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    b.set_document(Rc::new(
        Document::parse_html("<html><body></body></html>").unwrap(),
    ))
    .unwrap();
    b.set_grid_frame_stamps(true);
    b.evaluate("var got = ''; requestAnimationFrame(function (t) { got = [t, document.timeline.currentTime].join(); });")
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(40));
    turn(&b, 17);
    let got = read(&b, "got");
    let (t, tl) = got.split_once(',').unwrap();
    assert_eq!(tl, "17", "{got}");
    assert!(t.parse::<f64>().unwrap() >= 17.0, "{got}");
}

/// After an idle gap, `a = el.animate(...)` in a task does not move the timeline the task reads,
/// and `a.startTime = document.timeline.currentTime` reads 0 at once and then counts up from the
/// frame (Chrome `timeline_probe` "idleStart", 25 rows: the timeline the same before and after
/// `animate()`, 0 at once, then 0-0.07 (one row 16.68) in the first frame and 16.57-16.7 more in
/// the next). The frame `animate()` asks for is no frame held up before the task: the last frame's
/// time is not the task's. It read 17 after `animate()` where it read 10000 before, and the
/// animation started 9983 ms in the past.
#[test]
fn an_animation_started_from_the_timeline_after_idle_has_no_jump() {
    let b = page();
    b.evaluate(
        "var box = document.getElementById('box'), got = '', rows = [], a = null; requestAnimationFrame(function () {}); \
         setTimeout(function () { var before = document.timeline.currentTime; \
           a = box.animate({ opacity: [0, 1] }, { duration: 1000 }); \
           var after = document.timeline.currentTime; a.startTime = document.timeline.currentTime; \
           got = [after - before, a.currentTime].join(); \
           (function f() { requestAnimationFrame(function (t) { rows.push(Math.round(a.currentTime)); \
             if (rows.length < 2) f(); }); })(); }, 10005);",
    )
    .unwrap();
    turn(&b, 17);
    for _ in 0..8 {
        turn(&b, b.next_timer_delay().unwrap_or(17).max(1));
    }
    assert_eq!(read(&b, "got"), "0,0");
    let rows = read(&b, "rows.join()");
    assert!(rows == "0,17" || rows == "17,33", "{rows} (Chrome: 0 then 16.7; once 16.7 then 33.4)");
}

/// Everything the page reads of time never goes back, whatever the order of task reads, frame
/// requests, `animate()` calls, late turns and idle gaps: the timeline, the frame timestamps and
/// `performance.now()` are each non-decreasing; a frame's timestamp is at or before the
/// `performance.now()` its callback reads; and an animation started from the timeline never reads
/// a negative current time in a frame. Random sequences on the hand-driven clock, with the
/// deterministic page clock and the flowing one, grid frame stamps on and off. The flowing-clock
/// runs (and the 0-8 ms real sleeps before some idle gaps) are not bit-reproducible: a seed's
/// values move with real time, while the properties hold for every run.
#[test]
fn the_page_never_reads_time_going_back() {
    fn run(seed: u64, det: bool, grid: bool, real: bool) -> Result<(), String> {
        let b = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
        b.set_document(Rc::new(
            Document::parse_html("<html><body><div id=box></div></body></html>").unwrap(),
        ))
        .unwrap();
        b.set_deterministic_clock(det);
        b.set_grid_frame_stamps(grid);
        b.evaluate(
            "var L = { tl: [], t: [], p: [], over: [], neg: [] }, anims = [], box = document.getElementById('box'), n = 0; \
             function R(w) { L.tl.push(w + ':' + document.timeline.currentTime); L.p.push(performance.now()); } \
             function F() { requestAnimationFrame(function (t) { var p = performance.now(); L.t.push(t); \
               if (t > p) L.over.push(t + '>' + p); R('f'); \
               for (var i = 0; i < anims.length; i++) if (anims[i].currentTime < 0) L.neg.push(anims[i].currentTime); \
               if (++n % 3) F(); }); } \
             function A() { var a = box.animate({ opacity: [0, 1] }, { duration: 100000 }); \
               a.startTime = document.timeline.currentTime; anims.push(a); R('a'); }",
        )
        .unwrap();
        let mut s = seed;
        let mut next = |m: u64| {
            s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (s >> 33) % m
        };
        // A turn on the stepped clock, or a live turn on the real clock (CLOCK3: its delta is real
        // time that has passed, and timers set in it count from when they are set).
        let step = |b: &DomBindings, ms: u64| {
            if real {
                b.advance_real_timers_for_frame(ms, 1000, 0).unwrap();
            } else {
                turn(b, ms);
            }
        };
        for _ in 0..120 {
            match next(8) {
                0 => {
                    b.evaluate("R('h')").unwrap();
                }
                1 => {
                    let d = next(60);
                    let what = [
                        "R('t')",
                        "R('t'); F()",
                        "R('t'); A()",
                        "A(); R('t'); F()",
                        // A timer set in a timer's task, mid-turn (CLOCK3 on the real clock).
                        "R('t'); setTimeout(function () { R('n'); F(); }, 5)",
                    ][next(5) as usize];
                    b.evaluate(&format!("setTimeout(function () {{ {what}; }}, {d})")).unwrap();
                }
                2 => {
                    b.evaluate("F()").unwrap();
                }
                3 => {
                    b.evaluate("A()").unwrap();
                }
                4 | 5 => step(&b, next(40)),
                6 => step(&b, 100 + next(300)),
                _ => {
                    if next(2) == 0 {
                        std::thread::sleep(std::time::Duration::from_millis(next(8)));
                    }
                    step(&b, 1000 + next(2000));
                }
            }
        }
        let list = |v: &str| -> Vec<f64> {
            read(&b, &format!("{v}.join()"))
                .split(',')
                .filter(|x| !x.is_empty())
                .map(|x| x.rsplit(':').next().unwrap().parse().unwrap())
                .collect()
        };
        for (name, v) in [("timeline", "L.tl"), ("frame stamps", "L.t"), ("performance.now", "L.p")] {
            let xs = list(v);
            if let Some(i) = xs.windows(2).position(|w| w[1] < w[0]) {
                let tl = read(&b, "L.tl.join()");
                return Err(format!("{name} went back at {i}: {} then {} (timeline reads {tl})", xs[i], xs[i + 1]));
            }
        }
        for v in ["L.over", "L.neg"] {
            let x = read(&b, &format!("{v}.join()"));
            if !x.is_empty() {
                return Err(format!("{v}: {x}"));
            }
        }
        Ok(())
    }
    let mut failed = Vec::new();
    for seed in 1..=12u64 {
        for (det, grid, real) in [
            (true, false, false),
            (false, false, false),
            (false, true, false),
            (true, true, false),
            (false, false, true),
            (false, true, true),
        ] {
            if let Err(e) = run(seed, det, grid, real) {
                failed.push(format!("seed {seed} det {det} grid {grid} real {real}: {e}"));
            }
        }
    }
    assert!(failed.is_empty(), "{} of 72 runs:\n{}", failed.len(), failed.join("\n"));
}

/// A worker's message delivered at the top of a late turn is a task of its own (#1200 R1
/// follow-up): it reads the last frame's time, never one past the overdue frame, as a timer task
/// does (Chrome "overdue"). A guard, green before the pump began a task too: the pump runs before
/// the turn moves the clock, so it reads at the last turn's time. The worker pump stands in for a
/// worker here.
#[test]
fn a_worker_message_before_an_overdue_frame_reads_the_last_frames_time() {
    let b = page();
    b.evaluate(
        "var T0 = -1, rows = []; requestAnimationFrame(function (t0) { T0 = t0; \
           requestAnimationFrame(function (t) { rows.push('f:' + (t - T0) + ':' + (document.timeline.currentTime - T0)); }); });",
    )
    .unwrap();
    turn(&b, 17);
    b.runtime
        .borrow_mut()
        .evaluate_host_bookkeeping(
            "var once = true; __rk.workersPump = function () { if (!once) return 0; once = false; \
               rows.push('w:' + (document.timeline.currentTime - T0)); return 1; }; return 0",
        )
        .unwrap();
    turn(&b, 123);
    assert_eq!(read(&b, "rows.join()"), "w:0,f:17:17");
}

/// KNOWN DIVERGENCE, captures only (grid frame stamps): there the timeline reads the frame's exact
/// grid time while the callback's `t` stays on the flowing clock `performance.now()` reads, so
/// `a.startTime = t` in a frame puts the start ahead of the timeline by the real time in `t`, and
/// `a.currentTime` reads that much below 0 (Chrome: `t === timeline.currentTime`, so 0). Live, with
/// no grid stamps, the two are one value (`the_timeline_is_the_frame_stamp_inside_a_frame`). Here
/// 40 ms of real time pass before the frame at 17.
#[test]
fn known_divergence_in_a_capture_a_start_time_from_a_frame_stamp_is_off_the_timeline() {
    let b = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    b.set_document(Rc::new(
        Document::parse_html("<html><body><div id=box></div></body></html>").unwrap(),
    ))
    .unwrap();
    b.set_grid_frame_stamps(true);
    b.evaluate(
        "var got = 1; requestAnimationFrame(function (t) { \
           var a = document.getElementById('box').animate({ opacity: [0, 1] }, { duration: 1000 }); \
           a.startTime = t; got = a.currentTime; });",
    )
    .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(40));
    turn(&b, 17);
    assert!(num(&b, "got") < -1.0, "a.currentTime {} (Chrome: 0)", read(&b, "got"));
}
