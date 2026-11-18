//! One clock: requestAnimationFrame's timestamp, Event.timeStamp and document.timeline.currentTime
//! are DOMHighResTimeStamps on the same time origin as performance.now(). A load runs the timers on
//! a virtual clock ahead of real time, so `requestAnimationFrame(t => performance.now() - t)` was
//! negative and Event.timeStamp was epoch milliseconds (MotionMark and Speedometer subtract them).
//!
//! The page script and the expected relations: the pinned Chrome (CfT 148.0.7778.216),
//! `docs/diagnostics/2026-11-12/clock_parity_{page.js,chrome.mjs,cft148.json}`.

use super::*;

const PAGE_JS: &str = include_str!("../../../docs/diagnostics/2026-11-12/clock_parity_page.js");
const CHROME: &str = include_str!("../../../docs/diagnostics/2026-11-12/clock_parity_cft148.json");

fn page() -> DomBindings {
    let b = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    b.set_document(Rc::new(
        Document::parse_html("<html><head></head><body><button id=b>x</button></body></html>").unwrap(),
    ))
    .unwrap();
    b
}

fn ev(b: &DomBindings, script: &str) -> String {
    match b.evaluate(script).unwrap() {
        JsValue::String(s) => s,
        other => panic!("{other:?}"),
    }
}

fn observed(b: &DomBindings) -> serde_json::Value {
    match b.evaluate("window.__out").unwrap() {
        JsValue::String(s) => serde_json::from_str(&s).unwrap(),
        other => panic!("page did not finish: {other:?}"),
    }
}

/// The load: every timer up to the horizon runs at once, so the timers' clock is far ahead of the
/// real one (the 300 ms timeout and two frames run in a few milliseconds of real time).
#[test]
fn the_frame_timestamp_and_event_time_stamps_share_performance_nows_clock_in_a_load() {
    let b = page();
    b.evaluate(&format!("(0, eval)({})", serde_json::to_string(PAGE_JS).unwrap())).unwrap();
    b.run_timers(5000, 1000).unwrap();
    let want: serde_json::Value = serde_json::from_str(CHROME).unwrap();
    same(&observed(&b), &want);
}

/// The live loop: real time advances in turns; the same relations hold.
#[test]
fn the_same_relations_hold_when_the_page_runs_live() {
    live(8);
}

/// FLK6: the same, with the turns as far apart as a loaded runner made them (batch 83's CI failed
/// `.rafLoop.medianNearAFrame` with 8 ms sleeps). Each turn is still one virtual frame of 16 ms.
#[test]
fn the_same_relations_hold_when_the_live_turns_are_slow() {
    live(45);
}

/// Every animation frame callback's timestamp, with the live turn that ran it (`window.__turn`, set
/// by the test before each turn; -1 before the first).
const RECORD_FRAMES: &str = "window.__turn = -1; window.__frames = []; (function () { \
    var raf = window.requestAnimationFrame; \
    window.requestAnimationFrame = function (cb) { \
        return raf(function (t) { window.__frames.push([t, window.__turn]); return cb(t); }); \
    }; })();";

/// 60 live turns of 16 ms of virtual time, `sleep_ms` of real time apart.
///
/// Live, a frame's timestamp is its grid time on `performance.now()`'s clock: the clock read in its
/// turn less how late the turn ran it (`late[k]`: the turn's virtual time less the frame's grid
/// point, the latest of the 60 Hz grid at or before it), held at least the grid step past the frame
/// before it. So how far apart the page's frames are is how far apart this test's turns are in
/// real time: the page's `.rafLoop.medianNearAFrame` (Chrome: the middle gap is 5 to 40 ms) is a
/// wall-clock bound here, and a loaded runner failed it (FLK6). That one relation is checked on the
/// values instead, against the turns this test made, which holds at any load:
/// - each frame runs one turn after the one before (a frame asked for in a turn is due at the next
///   grid point, which the next 16 ms turn reaches), and every callback of a turn gets the same
///   timestamp;
/// - consecutive frames in turns `a < b` are at least their grid points' distance apart (the
///   floor) and at most `max(floor, end[b] - late[b] - (start[a] - late[a]))`, where `start[k]` and
///   `end[k]` are `performance.now()` just before and just after turn `k`: the clock is monotonic,
///   so frame `a`'s stamp is not before `start[a] - late[a]` and frame `b`'s is not past
///   `end[b] - late[b]` unless the floor put it there. The span covers the two turns and the one
///   sleep between them.
/// Every other relation is compared with Chrome as it stands.
fn live(sleep_ms: u64) {
    let b = page();
    b.evaluate(RECORD_FRAMES).unwrap();
    b.evaluate(&format!("(0, eval)({})", serde_json::to_string(PAGE_JS).unwrap())).unwrap();
    let now = |b: &DomBindings| ev(b, "String(performance.now())").parse::<f64>().unwrap();
    let (mut start, mut end, mut at) = (Vec::new(), Vec::new(), Vec::new());
    for turn in 0..60 {
        b.evaluate(&format!("window.__turn = {turn}")).unwrap();
        start.push(now(&b));
        b.advance_timers(16, 100).unwrap();
        end.push(now(&b));
        at.push(b.clock_now_ms());
        std::thread::sleep(std::time::Duration::from_millis(sleep_ms));
    }
    let frames: Vec<(f64, i64)> = serde_json::from_str(&ev(&b, "JSON.stringify(window.__frames)")).unwrap();
    // One timestamp per turn, in turn order.
    let mut stamps: Vec<(i64, f64)> = Vec::new();
    for &(t, turn) in &frames {
        assert!(turn >= 0, "a frame ran outside a live turn: {frames:?}");
        match stamps.last() {
            Some(&(last, at)) if last == turn => assert_eq!(at, t, "two timestamps in turn {turn}: {frames:?}"),
            Some(&(last, _)) => assert!(turn > last, "frames out of turn order: {frames:?}"),
            None => {}
        }
        if stamps.last().map_or(true, |&(last, _)| last != turn) {
            stamps.push((turn, t));
        }
    }
    // The page asks for 2 frames, then a loop of 11: turns 0 to 10.
    assert_eq!(stamps.len(), 11, "{frames:?}");
    // The 60 Hz grid's latest point at or before `t`, and how late a turn at `t` ran it.
    let grid = |k: f64| ((k * 50.0 + 2.0) / 3.0).floor();
    let due = |t: f64| {
        let mut k = (t * 3.0 / 50.0).floor();
        while grid(k + 1.0) <= t {
            k += 1.0;
        }
        while grid(k) > t {
            k -= 1.0;
        }
        grid(k)
    };
    let late = |k: usize| at[k] - due(at[k]);
    for pair in stamps.windows(2) {
        let ((a, ta), (b2, tb)) = (pair[0], pair[1]);
        assert_eq!(b2, a + 1, "a frame skipped a turn: {stamps:?}");
        let floor = due(at[b2 as usize]) - due(at[a as usize]);
        let span = (end[b2 as usize] - late(b2 as usize)) - (start[a as usize] - late(a as usize));
        assert!(tb - ta >= floor - 1e-6, "frames {a} and {b2} less than a frame apart: {stamps:?}");
        assert!(
            tb - ta <= floor.max(span) + 1e-6,
            "frames {a} and {b2} are {} ms apart, more than the {span} ms their turns took: {stamps:?} {end:?}",
            tb - ta
        );
    }
    let want: serde_json::Value = serde_json::from_str(CHROME).unwrap();
    let mut got = observed(&b);
    // Checked above, against the turns.
    got["rafLoop"]["medianNearAFrame"] = want["rafLoop"]["medianNearAFrame"].clone();
    same(&got, &want);
}

/// Equal, naming the first path that differs (the relations are booleans, so say which one).
fn same(got: &serde_json::Value, want: &serde_json::Value) {
    fn walk(path: String, got: &serde_json::Value, want: &serde_json::Value, out: &mut Vec<String>) {
        match (got, want) {
            (serde_json::Value::Object(g), serde_json::Value::Object(w)) => {
                for (k, wv) in w {
                    walk(format!("{path}.{k}"), g.get(k).unwrap_or(&serde_json::Value::Null), wv, out);
                }
            }
            (serde_json::Value::Array(g), serde_json::Value::Array(w)) => {
                for (i, wv) in w.iter().enumerate() {
                    walk(format!("{path}[{i}]"), g.get(i).unwrap_or(&serde_json::Value::Null), wv, out);
                }
            }
            _ if got != want => out.push(format!("{path}: chrome {want}, here {got}")),
            _ => {}
        }
    }
    let mut out = Vec::new();
    walk(String::new(), got, want, &mut out);
    assert!(out.is_empty(), "{}", out.join("
"));
}

/// Inside one script turn the page's clocks run at the rate of the wall clock. CodeMirror's parser
/// reads `Date.now()` for its slice deadline: a clock that ran 2.6x fast expired it early and it
/// yielded after 21 of 86 tokens (Speedometer 3). A guard, not a fix: on the Windows build the
/// clocks already track wall time, so this passes on the code before the page-clock change too.
#[test]
fn date_now_and_performance_now_advance_at_wall_rate_inside_a_busy_turn() {
    for (clock, what) in [("Date.now()", "Date.now"), ("performance.now()", "performance.now")] {
        let b = page();
        let wall = std::time::Instant::now();
        let seen = match b
            .evaluate(&format!(
                "var s = {clock}, n = 0; while ({clock} - s < 200) {{ n++; }} String({clock} - s)"
            ))
            .unwrap()
        {
            JsValue::String(s) => s.parse::<f64>().unwrap(),
            other => panic!("{other:?}"),
        };
        let wall_ms = wall.elapsed().as_secs_f64() * 1000.0;
        let ratio = seen / wall_ms;
        assert!(
            (0.85..=1.2).contains(&ratio),
            "{what} advanced {seen} ms in a turn that took {wall_ms:.1} ms of wall time (ratio {ratio:.2})"
        );
    }
}

/// A rAF loop in a load: every frame is its own time. All ten frames used to get the first frame's
/// stamp (a delta of 0 where Chrome's is about 16.7) because the stamp was fixed per `runTimers` call.
#[test]
fn consecutive_frames_in_a_load_have_distinct_timestamps_one_frame_apart() {
    let b = page();
    b.evaluate(
        "var last, d = []; requestAnimationFrame(function f(t) { if (last !== undefined) d.push(t - last);          last = t; if (d.length < 10) requestAnimationFrame(f); });",
    )
    .unwrap();
    b.run_timers(1000, 1000).unwrap();
    let got = ev(&b, "d.map(function (x) { return Math.round(x); }).join()");
    let deltas: Vec<i64> = got.split(',').map(|x| x.parse().unwrap()).collect();
    assert_eq!(deltas.len(), 10, "{got}");
    assert!(deltas.iter().all(|d| (15..=25).contains(d)), "frame deltas {got} (Chrome: about 16.7)");
}

/// Capture mode: the page clock has no real time in it, so two runs of one page see the same
/// timestamps however long each took, and a busy loop on `performance.now()` still ends.
#[test]
fn in_capture_mode_the_page_clock_reproduces_across_runs() {
    let run = || {
        let b = page();
        b.set_deterministic_clock(true);
        b.evaluate(
            "var log = []; var btn = document.getElementById('b'); btn.addEventListener('click', function (e) { log.push(e.timeStamp); }); btn.click(); log.push(performance.now()); var s = performance.now();              while (performance.now() - s < 5) {} log.push(performance.now());              var n = 0; requestAnimationFrame(function f(t) { log.push(t); log.push(new Event('x').timeStamp);              if (++n < 4) requestAnimationFrame(f); }); setTimeout(function () { log.push(performance.now()); }, 40);",
        )
        .unwrap();
        // however long this run takes in real time
        std::thread::sleep(std::time::Duration::from_millis(30));
        b.run_timers(1000, 1000).unwrap();
        ev(&b, "JSON.stringify(log)")
    };
    let first = run();
    std::thread::sleep(std::time::Duration::from_millis(60));
    let second = run();
    assert_eq!(first, second);
    assert!(first.starts_with('['), "{first}");
}

/// The real-flowing page clock never loses real time. It used to count real time only since the
/// virtual clock last moved, and held its last value until virtual time caught up: after a long
/// real wait with no virtual step, the next timer left `performance.now()` far behind real time.
#[test]
fn the_page_clock_keeps_real_time_across_a_virtual_step() {
    let b = page();
    b.evaluate(
        "var p0 = performance.now(), p1; setTimeout(function () { p1 = performance.now(); }, 16);",
    )
    .unwrap();
    // 150 ms of real time pass with the virtual clock standing still, then it steps 16 ms
    std::thread::sleep(std::time::Duration::from_millis(150));
    b.run_timers(100, 100).unwrap();
    let gap: f64 = ev(&b, "String(p1 - p0)").parse().unwrap();
    assert!(gap >= 140.0, "performance.now() moved {gap} ms across 150 ms of real time and a virtual step");
}

/// A long task, then the timers' clock catching up. The task's real time is given to the timers'
/// clock on the turn after it, so the first frame after the task runs with that clock still behind
/// and is stamped real time; a later frame used to be stamped that stamp plus the whole catch-up:
/// the task counted twice, and `performance.now()` ran ahead of real time by the task's length from
/// then on. In the app's window a frame's 3000 ms task left the frame's clock 3202 ms ahead
/// (`docs/diagnostics/2026-11-15/long_task_clock_app.json`); the pinned Chrome's stays within a
/// millisecond of `Date.now()` (`long_task_clock_cft148.json`). MotionMark reads this clock to
/// decide a test is over. (#1205's red test, moved onto the 60 Hz grid: frames at 17, then the
/// catch-up turn, then two turns on time; on the grid the third frame after the task is the one the
/// floor inflated.)
///
/// The bound is on the values: consecutive frames are no further apart than the real time between
/// the first and the last plus one frame, and `performance.now()` moves no more than that, however
/// long this test's own calls took.
#[test]
fn a_frame_after_a_long_tasks_catch_up_is_not_stamped_the_task_twice() {
    let b = page();
    b.evaluate(
        "var log = []; requestAnimationFrame(function f(t) { log.push(t, performance.now());          if (log.length < 8) requestAnimationFrame(f); });",
    )
    .unwrap();
    // The task: real time the timers' clock has not been given.
    let task_ms = 300;
    std::thread::sleep(std::time::Duration::from_millis(task_ms));
    let began = std::time::Instant::now();
    b.advance_timers(17, 10).unwrap(); // the first frame, the timers' clock a task behind
    b.advance_timers(task_ms, 10).unwrap(); // the catch-up, and the second frame
    b.advance_timers(17, 10).unwrap(); // the third
    b.advance_timers(17, 10).unwrap(); // the fourth
    let real_ms = began.elapsed().as_secs_f64() * 1000.0;
    let log: Vec<f64> = ev(&b, "log.join()").split(',').map(|x| x.parse().unwrap()).collect();
    assert_eq!(log.len(), 8, "{log:?}");
    for i in 1..4 {
        let (stamps, clock) = (log[2 * i] - log[2 * i - 2], log[2 * i + 1] - log[2 * i - 1]);
        assert!(stamps > 0.0, "frame {i} is not after the one before: {log:?}");
        assert!(
            stamps <= real_ms + 17.0 + 1e-6 && clock <= real_ms + 17.0 + 1e-6,
            "{real_ms} ms of real time passed over the frames; frame {i} is stamped {stamps} ms after              the one before and performance.now() moved {clock} ms: {log:?}"
        );
    }
}

/// CLOCK-INFLATE, as a property: in a live loop whose turns give the timers' clock the real time
/// that passed (the app's `LiveClock`), long tasks of random lengths inside turns, with frame loops
/// and timer tasks reading the clock and the timeline, never leave `performance.now()`, a frame's
/// stamp or the timeline ahead of real time by more than one grid step, whatever the task's length
/// (Chrome: 0-1 ms, `long_task_clock_cft148.json`). Before the fix a task was counted twice after
/// its catch-up (311 to 2688 ms ahead here). What is left is one grid step at most: consecutive
/// frames are kept at least the grid distance apart (Chrome's spacing), so a frame run straight
/// after the one before can be stamped up to a step past the clock.
///
/// Real time is a value here, not the wall clock (the harness's `__rk.testRealMs`): a callback's
/// work and the time between turns advance it, each turn is given what passed since the last, and
/// the bound is exact.
#[test]
fn long_tasks_never_leave_the_page_clock_ahead_of_real_time() {
    fn run(seed: u64) -> Result<(), String> {
        let b = page();
        b.runtime
            .borrow_mut()
            .evaluate_host_bookkeeping(
                "var fake = 0; __rk.testRealMs = function () { return fake; }; \
                 window.__work = function (ms) { fake += ms; }; \
                 window.__real = function () { return fake; }; return 0",
            )
            .unwrap();
        b.evaluate(
            "var L = []; function R() { L.push(performance.now(), document.timeline ? document.timeline.currentTime : 0); } \
             function F() { requestAnimationFrame(function (t) { L.push(t); R(); __work(1); F(); }); } F();",
        )
        .unwrap();
        let mut s = seed;
        let mut next = |m: u64| {
            s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (s >> 33) % m
        };
        let num = |b: &DomBindings, v: &str| -> f64 { ev(b, &format!("String({v})")).parse().unwrap() };
        // The page clock and real time agree at the start (a turn to settle the first reading).
        b.advance_timers(0, 10).unwrap();
        let (p0, r0) = (num(&b, "performance.now()"), num(&b, "__real()"));
        let mut turn_began = r0;
        let mut worst = f64::MIN;
        for _ in 0..200 {
            if next(3) == 0 {
                // A long task inside the next turn: a timer callback that works.
                let ms = 100 + next(2900);
                b.evaluate(&format!("setTimeout(function () {{ __work({ms}); }}, 0)")).unwrap();
            }
            if next(4) == 0 {
                b.evaluate(&format!("setTimeout(function () {{ R(); }}, {})", next(40))).unwrap();
            }
            // Time between turns (the loop's wait), then a turn given the real time since the last
            // turn began, whole ms, as the app's loop is.
            b.evaluate(&format!("__work({})", next(20))).unwrap();
            let now = num(&b, "__real()");
            let elapsed = (now - turn_began).floor();
            turn_began += elapsed;
            b.advance_timers(elapsed as u64, 100).unwrap();
            let real = num(&b, "__real()") - r0;
            let reads: Vec<f64> = ev(&b, "L.join()")
                .split(',')
                .filter(|x| !x.is_empty())
                .map(|x| x.parse().unwrap())
                .collect();
            let top = reads.iter().cloned().fold(f64::MIN, f64::max).max(num(&b, "performance.now()"));
            worst = worst.max(top - p0 - real);
            // One grid step (at most 17 ms) and 1 ms for where the starting reading sits.
            if top - p0 > real + 18.0 {
                return Err(format!("{:.1} ms ahead of real time ({:.1} ms real)", top - p0 - real, real));
            }
        }
        eprintln!("seed {seed}: at most {worst:.2} ms ahead of real time");
        Ok(())
    }
    let failed: Vec<String> = (1..=12u64).filter_map(|seed| run(seed).err().map(|e| format!("seed {seed}: {e}"))).collect();
    assert!(failed.is_empty(), "{}", failed.join("\n"));
}
