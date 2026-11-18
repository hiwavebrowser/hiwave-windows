//! Animation frames on a 60 Hz grid (PACE2's next row). The pinned Chrome (CfT 148.0.7778.216,
//! headless, five runs of `docs/diagnostics/2026-11-15/frame_grid_probe.html`,
//! `frame_grid_probe_cft148.json`): a requestAnimationFrame loop gets 181 frames in 3000 ms; the
//! timestamps step by 16.6-16.7 ms, all on one 1000/60 ms grid; and after a frame whose callback
//! ran 24 ms the next frame's timestamp is again 16.7 ms on: a late frame is caught up, it does not
//! move the frames after it and no frame is dropped.
//!
//! RustKit's clock is whole milliseconds, so the grid is 1000/60 ms rounded up: 17, 34, 50, 67,
//! 84, 100, ... from the clock's zero. Until this row a frame was due 16 ms after the turn that ran
//! the last one, so every late turn moved every later frame (the content thread's loop wakes
//! ~1.2 ms late: 173 frames of Chrome's 180-181, PACE2 #1190).
//!
//! The tests turn the clock by hand (no wall clock): each turn comes when the next timer is due,
//! plus a lateness.

use super::*;

/// Grid point `k`: 1000/60 ms times `k`, rounded up to the whole millisecond.
fn grid(k: u64) -> u64 {
    (k * 50).div_ceil(3)
}

fn on_grid(t: u64) -> bool {
    grid(t * 3 / 50) == t || grid(t * 3 / 50 + 1) == t
}

fn frames_page() -> DomBindings {
    let b = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    b.evaluate("var frames = 0; (function f() { requestAnimationFrame(function () { frames++; f(); }); })();")
        .unwrap();
    b
}

fn read(b: &DomBindings, v: &str) -> String {
    match b.evaluate(&format!("String({v})")).unwrap() {
        JsValue::String(s) => s,
        other => panic!("{other:?}"),
    }
}

/// Turns the clock from 0, each turn `late(turn)` ms after the next timer is due, while that is due
/// by `span`. Returns when each turn's frame was due on the engine's reading (the clock plus
/// `next_timer_delay`): a grid point, or the clock itself when the frame is already due.
fn drive(b: &DomBindings, span: u64, late: impl Fn(usize) -> u64) -> Vec<u64> {
    let (mut now, mut dues) = (0u64, Vec::new());
    for turn in 0.. {
        let delay = b.next_timer_delay().expect("a frame is asked for");
        if now + delay > span {
            break;
        }
        dues.push(now + delay);
        let step = delay + late(turn);
        b.advance_timers_for_frame(step, 1000, 0).unwrap();
        now += step;
    }
    dues
}

/// Turns that come 1 ms late every time (as the loop's sleeps do) leave the frames on the grid:
/// 180 due in 3000 ms, as Chrome's 180-181. Due 16 ms after the late turn, they came every 17 ms
/// (176) and drifted off it.
#[test]
fn frames_stay_on_the_60hz_grid_when_every_turn_is_late() {
    let b = frames_page();
    let dues = drive(&b, 3000, |_| 1);
    let off: Vec<u64> = dues
        .iter()
        .copied()
        .filter(|&t| !on_grid(t))
        .take(5)
        .collect();
    assert!(off.is_empty(), "frames due off the grid at {off:?}");
    assert_eq!(read(&b, "frames"), "180");
}

/// A turn that comes 24 ms after its frame was due (a long task) runs that frame, and the next is
/// due at its own grid point, already passed, so it runs on the next turn: no frame is dropped and
/// the frames after are on the grid again (Chrome: 181 frames with seven 24 ms frames, every gap
/// 16.6-16.7 ms). Due 16 ms after the late turn, the late frame moved every later one.
#[test]
fn a_late_frame_is_caught_up_not_carried_forward() {
    let b = frames_page();
    // Turn 30 runs frame 31 (due at grid(31)) 24 ms late. Frame 32 (grid(31) + 17) is due already
    // and runs on the next turn, at once; frame 33 is on the grid.
    let dues = drive(&b, 3000, |turn| if turn == 30 { 24 } else { 0 });
    assert_eq!(dues[30], grid(31));
    assert_eq!(
        dues[31],
        grid(31) + 24,
        "the frame after the late one waited: {:?}",
        &dues[29..34]
    );
    assert!(
        dues[32..].iter().all(|&t| on_grid(t)),
        "{:?}",
        &dues[29..40]
    );
    assert_eq!(read(&b, "frames"), "180");
}

/// The callbacks' timestamp is the frame's time on the grid, not when the late turn ran it: with
/// one turn 24 ms late, every gap between consecutive frames' timestamps stays 16-17 ms, as
/// Chrome's stay 16.6-16.8 ms after its 24 ms frames (the deterministic clock, so the numbers are
/// exact: the frame's grid point plus the clock's 0.1 ms read steps). Stamped with the turn's
/// time, the gaps went 24 ms then 0.
#[test]
fn a_late_frames_timestamp_is_its_grid_time() {
    let b = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    b.set_deterministic_clock(true);
    b.evaluate("var stamps = []; (function f() { requestAnimationFrame(function (t) { stamps.push(t); f(); }); })();")
        .unwrap();
    drive(&b, 1000, |turn| if turn == 30 { 24 } else { 0 });
    let stamps: Vec<f64> = read(&b, "stamps.join()").split(',').map(|s| s.parse().unwrap()).collect();
    let gaps: Vec<f64> = stamps.windows(2).map(|w| w[1] - w[0]).collect();
    assert!(
        gaps.iter().all(|&g| (15.5..=17.5).contains(&g)),
        "gaps {:?}",
        &gaps[25..36]
    );
}

/// Which frame a task's request lands in, and its stamp: `frame_stamp_probe.html`'s sweep on the
/// pinned Chrome (CfT 148.0.7778.216, `frame_stamp_probe_cft148.json`, two runs): a frame at T
/// sets `setTimeout(task, k)` and the task asks for a frame. A task that runs before T + 16.7 gets
/// the frame at T + 16.7; one that runs between T + 16.7 and T + 33.3 (k = 14 to 31) gets the
/// frame at T + 16.7 too, run at once although its time has passed (16.6-16.7 in every run).
/// Here: the frame at 17 (T), a task at 17 + k; the frame it gets, as its stamp less T on the
/// deterministic clock. RustKit takes the latest grid point at or before the task, never before the
/// frame after the last; Chrome's one exception, a task between T + 33.3 and T + 50 that waits for
/// T + 50 (its scheduler had ticked T + 33.3 idle), is not modelled: such a task here gets T + 33.
fn task_frame_gap(k: u64) -> (i64, bool) {
    {
        let b = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
        b.set_deterministic_clock(true);
        b.evaluate(&format!(
            "var T = -1, got = -1; requestAnimationFrame(function (t) {{ T = t; \
               setTimeout(function () {{ requestAnimationFrame(function (t2) {{ got = t2 - T; }}); }}, {k}); }});"
        ))
        .unwrap();
        b.advance_timers_for_frame(17, 1000, 0).unwrap();
        b.advance_timers_for_frame(k, 1000, 0).unwrap();
        // The frame ran in the task's own turn when its time had passed; otherwise when it is due.
        let ran_at_once = read(&b, "got") != "-1";
        if !ran_at_once {
            let delay = b.next_timer_delay().expect("the frame is asked for");
            b.advance_timers_for_frame(delay, 1000, 0).unwrap();
        }
        let got: f64 = read(&b, "got").parse().unwrap();
        (got.round() as i64, ran_at_once)
    }
}

#[test]
fn a_frame_a_task_asks_for_is_the_frame_of_its_time() {
    let gap = task_frame_gap;
    // Chrome: 16.6-16.7, then (from k = 14) 16.6-16.7 run at once.
    assert_eq!(gap(5), (17, false));
    assert_eq!(gap(17), (17, true), "a task at the grid instant itself");
    assert_eq!(gap(20), (17, true));
    assert_eq!(gap(30), (17, true));
    // Two grid steps on: the latest grid point at or before the task (Chrome: 50, see above).
    assert_eq!(gap(60), (50, true));
}

/// A frame asked for after the page was idle runs at once and is the frame of the grid point at or
/// before the request, as Chrome's (`frame_stamp_probe`'s idle part: after 300 ms with no frame, a
/// timer task's frame is stamped 0.6 to 15.3 ms before the task's `performance.now()`, and its
/// callback runs at once). Here a timer at 1005: the frame of 1000.
#[test]
fn a_frame_asked_for_when_idle_is_the_frame_of_its_time() {
    let b = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    b.set_deterministic_clock(true);
    b.evaluate(
        "var asked = -1, t = -1; \
         setTimeout(function () { asked = performance.now(); requestAnimationFrame(function (s) { t = s; }); }, 1005);",
    )
    .unwrap();
    b.advance_timers_for_frame(1005, 1000, 0).unwrap();
    let (asked, t): (f64, f64) = (read(&b, "asked").parse().unwrap(), read(&b, "t").parse().unwrap());
    assert!(t >= 0.0, "the frame did not run in the timer's turn");
    assert!(asked - 17.0 < t && t <= asked, "asked at {asked}, stamped {t}");
    assert_eq!(t.floor(), grid(60) as f64);
}

/// KNOWN DIVERGENCE, not modelled: in Chrome a task between T + 33.3 and T + 50 after a frame at T
/// waits for the frame at T + 50 and runs then (`frame_stamp_probe_cft148.json`, sweep k = 34 to
/// 40: stamped 49.9-50.1, run at 50.1-54.5), as its scheduler had ticked T + 33.3 while idle. Here
/// such a task gets the latest grid point at or before it, T + 33, run at once.
#[test]
fn known_divergence_a_task_after_two_frames_gets_the_passed_frame() {
    assert_eq!(task_frame_gap(40), (33, true), "Chrome: (50, false)");
}

/// A frame a long task held up is stamped with its grid time however late it runs, and a frame a
/// timer in its callback asks for comes after it on the grid: the page never sees a stamp go
/// backwards. The pinned Chrome (CfT 148.0.7778.216, `frame_stamp_probe_cft148.json`,
/// "longTimer", five runs): a frame at T0 asks for the next and posts a task that works 45, 100 or
/// 200 ms; the next frame runs 30-190 ms late and is stamped T0 + 16.6-16.8 every time; its
/// callback's 5 ms timer asks for a frame, stamped at the grid point at or before the request
/// (T0 + 50 after the 45 ms task). Here the frame at 17, the next (due 34) run at 69 by a late
/// turn, its timer at 74: stamped T0 + 17 and T0 + 50. Stamped with the turn's time once two
/// frames late, the late frame got 69 and the timer's frame 67.
#[test]
fn a_frame_held_up_by_a_long_task_keeps_its_grid_time_and_stamps_never_go_back() {
    let b = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    b.set_deterministic_clock(true);
    b.evaluate(
        "var T0 = -1, s = []; requestAnimationFrame(function (t) { T0 = t;            requestAnimationFrame(function (t1) { s.push(t1 - T0);              setTimeout(function () { requestAnimationFrame(function (t2) { s.push(t2 - T0); }); }, 5); }); });",
    )
    .unwrap();
    b.advance_timers_for_frame(17, 1000, 0).unwrap();
    b.advance_timers_for_frame(52, 1000, 0).unwrap();
    b.advance_timers_for_frame(5, 1000, 0).unwrap();
    if read(&b, "s.length") != "2" {
        let delay = b.next_timer_delay().expect("the timer's frame is asked for");
        b.advance_timers_for_frame(delay, 1000, 0).unwrap();
    }
    assert_eq!(read(&b, "s.map(Math.round).join()"), "17,50", "Chrome: 16.7, 50");
}

/// Flipped by TIMELINE1: the timeline is the frame's stamp inside its callbacks
/// (`timeline_tests::the_timeline_is_the_frame_stamp_inside_a_frame`).

/// A turn begins outside any frame's callbacks, whatever the last turn left: a request a task makes
/// is a task's (the frame of its time), not a frame's own (the next).
#[test]
fn a_turn_begins_outside_a_frame() {
    let b = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    b.evaluate("var ran = 0; setTimeout(function () { requestAnimationFrame(function () { ran++; }); }, 40);")
        .unwrap();
    b.runtime
        .borrow_mut()
        .evaluate_host_bookkeeping("__rk.shared.timerQueue.inFrame = true; return 0")
        .unwrap();
    b.advance_timers_for_frame(40, 1000, 0).unwrap();
    assert_eq!(read(&b, "String(ran)"), "1", "the task's frame (34) did not run in its turn");
}
