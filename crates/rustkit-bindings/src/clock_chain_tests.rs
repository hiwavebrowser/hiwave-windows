//! CLOCK3: a timer set during a live turn counts its delay from the turn's time, not from the
//! virtual due time of the callback that set it. After a long task the live loop's next turn moves
//! the clock by the whole wait; the timers due inside it ran each at its own due time, so a 10 ms
//! `setTimeout` chain set its next link 10 ms after a time already hundreds of ms in the past, and
//! replayed every link it had missed in that one turn (about 200 after a 2 s frame, PACE1's page).
//! Intervals were already re-based on the turn's time; chains were not.
//!
//! The page and the expected relations: the pinned Chrome (CfT 148.0.7778.216),
//! `docs/diagnostics/2026-11-15/chain_after_long_task{.html,_chrome.mjs,_cft148.json}`.

use super::*;

const PAGE: &str = include_str!("../../../docs/diagnostics/2026-11-15/chain_after_long_task.html");
const ZERO_DELAY: &str = include_str!("../../../docs/diagnostics/2026-11-15/zero_delay_chain.html");

/// A fixture's script: its one `<script>`.
fn script(page: &'static str) -> &'static str {
    let start = page.find("<script>").unwrap() + "<script>".len();
    let end = page.rfind("</script>").unwrap();
    &page[start..end]
}

/// Turns a fixture's timers live, as the engine's live turn on the real clock does
/// (`advance_real_timers_for_frame` with the real time since the last turn, a 16 ms frame budget),
/// and waits between turns as the app's content thread does (the next timer, 2 to 50 ms), until the
/// page is done or 5 s pass. Returns `rec`. `real` false turns it on the stepped clock instead
/// (`advance_timers_for_frame`), as every live turn did before CLOCK3.
fn run_live(page: &'static str, real: bool) -> serde_json::Value {
    let b = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    b.set_document(Rc::new(
        Document::parse_html("<html><head></head><body></body></html>").unwrap(),
    ))
    .unwrap();
    b.evaluate(&format!(
        "(0, eval)({})",
        serde_json::to_string(script(page)).unwrap()
    ))
    .unwrap();
    // A fixture starts in `go()` (at the load event in Chrome).
    b.evaluate("go()").unwrap();
    let began = std::time::Instant::now();
    let mut clock = began;
    while began.elapsed() < std::time::Duration::from_secs(5) {
        let elapsed_ms = clock.elapsed().as_millis() as u64;
        clock += std::time::Duration::from_millis(elapsed_ms);
        if real {
            b.advance_real_timers_for_frame(elapsed_ms, 1000, 16)
                .unwrap();
        } else {
            b.advance_timers_for_frame(elapsed_ms, 1000, 16).unwrap();
        }
        if matches!(b.evaluate("rec.done").unwrap(), JsValue::Boolean(true)) {
            break;
        }
        let wait = b.next_timer_delay().unwrap_or(50).clamp(2, 50);
        std::thread::sleep(std::time::Duration::from_millis(wait));
    }
    match b.evaluate("JSON.stringify(rec)").unwrap() {
        JsValue::String(s) => serde_json::from_str(&s).unwrap(),
        other => panic!("{other:?}"),
    }
}

/// Chrome, five runs: chainBurst 1, chainMinGapMs 10 to 10.1, chainAfter 28, intervalBurst 1 to 2,
/// intervalAfter 30 to 31 (longMs 500).
#[test]
fn a_timeout_chain_resumes_at_its_own_pace_after_a_long_task() {
    let rec = run_live(PAGE, true);
    eprintln!("CLOCK3-RESULT {rec}");
    assert_eq!(rec["done"], true, "the page did not finish: {rec}");
    assert!(rec["longMs"].as_f64().unwrap() >= 500.0, "{rec}");
    // The one link that was overdue runs; the next is set from then. At most one in the 5 ms after
    // the long task (the overdue link may land just past them on a loaded machine), and a link
    // after it.
    assert!(
        rec["chainBurst"].as_u64().unwrap() <= 1,
        "chain links run at once after the long task: {rec}"
    );
    assert!(
        rec["chainAfter"].as_u64().unwrap() >= 1,
        "no chain link after the long task: {rec}"
    );
    // Chrome 10. Up to 1 ms less here: the live loop's turn delta is whole milliseconds, so the
    // virtual clock a link is set on trails the real clock by under a millisecond (0 before CLOCK3).
    assert!(
        rec["chainMinGapMs"].as_f64().unwrap() >= 9.0,
        "two chain links less than their 10 ms apart after the long task: {rec}"
    );
    // No replay: at most a link per 10 ms in the 300 ms after it (Chrome 28).
    assert!(
        rec["chainAfter"].as_u64().unwrap() <= 31,
        "chain links in the 300 ms after: {rec}"
    );
    // An interval: one overdue run, then its period on its grid (Chrome 1 to 2, 30 to 31).
    assert!(rec["intervalBurst"].as_u64().unwrap() <= 2, "{rec}");
    assert!(rec["intervalAfter"].as_u64().unwrap() <= 32, "{rec}");
}

/// A stepped turn (a capture's settle, a test) moves virtual time the page has not lived through, so
/// a chain still runs a link per period of it: five 1000 ms links in a 5000 ms step, as before
/// CLOCK3. On the real clock the same turn is 5000 ms the page was held back: the overdue link runs
/// once, and the next is at least a period after the turn's time (plus the real time the turn had
/// taken when it was set). The period is long so the count cannot depend on the machine: only a
/// stall of a whole second inside the turn would let the next link into it.
#[test]
fn a_stepped_turn_still_runs_a_chain_link_per_period_of_virtual_time() {
    let count = |real: bool| {
        let b = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
        b.evaluate("var n = 0; (function link() { n++; setTimeout(link, 1000); })();")
            .unwrap();
        if real {
            b.advance_real_timers_for_frame(5000, 1000, 0).unwrap();
        } else {
            b.advance_timers_for_frame(5000, 1000, 0).unwrap();
        }
        let n = match b.evaluate("String(n)").unwrap() {
            JsValue::String(s) => s,
            other => panic!("{other:?}"),
        };
        (n, b.next_timer_delay())
    };
    // The first link runs at once, then one per 1000 ms: 1000, 2000, 3000, 4000, 5000.
    assert_eq!(count(false), ("6".to_string(), Some(1000)));
    let (n, next) = count(true);
    assert_eq!(n, "2");
    assert!(
        next.is_some_and(|d| d >= 1000),
        "the next link is due in {next:?} ms"
    );
}

/// A frame tree is one timer queue on one clock (#1109), so the real clock's turn is the tree's: a
/// chain a same-origin frame set, run by the page's turn, counts from the turn's time as the page's
/// own does (2 links in a 5000 ms real turn, 6 stepped), and the page's and the frame's chains, set
/// together, run in step. A 1000 ms period, so that only a whole-second stall inside the turn could
/// change a count (#1183 R1 follow-up).
#[test]
fn a_frames_chain_on_the_real_clock_counts_from_the_trees_turn() {
    let count = |real: bool| {
        let parent = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
        parent
            .set_document(Rc::new(Document::parse_html("<html><body></body></html>").unwrap()))
            .unwrap();
        let child = parent.new_child().unwrap();
        child
            .set_document(Rc::new(Document::parse_html("<html><body></body></html>").unwrap()))
            .unwrap();
        child
            .evaluate("var n = 0; (function link() { n++; setTimeout(link, 1000); })();")
            .unwrap();
        parent
            .evaluate("var m = 0; (function link() { m++; setTimeout(link, 1000); })();")
            .unwrap();
        if real {
            parent.advance_real_timers_for_frame(5000, 1000, 0).unwrap();
        } else {
            parent.advance_timers_for_frame(5000, 1000, 0).unwrap();
        }
        let read = |b: &DomBindings, v: &str| match b.evaluate(&format!("String({v})")).unwrap() {
            JsValue::String(s) => s,
            other => panic!("{other:?}"),
        };
        (read(&child, "n"), read(&parent, "m"), parent.next_timer_delay())
    };
    assert_eq!(count(false), ("6".to_string(), "6".to_string(), Some(1000)));
    let (n, m, next) = count(true);
    assert_eq!((n.as_str(), m.as_str()), ("2", "2"));
    assert!(next.is_some_and(|d| d >= 1000), "the next link is due in {next:?} ms");
}

/// An interval held back on the real clock runs once and keeps its grid, as Chrome's does (bursts of
/// 1 to 2 and 30 to 31 runs in the 300 ms after a long task): set at 0 with a 10 ms period, held back
/// for 55 ms, it runs once and next at 60, not at 65 (re-based) nor five times (caught up). Its next
/// is the first grid point past the turn's time plus the real time the turn took, so on a loaded
/// machine it can be 70; either way 5 ms off the turn's time modulo 10.
#[test]
fn an_interval_held_back_on_the_real_clock_runs_once_and_keeps_its_grid() {
    // A 1000 ms period held back 5500 ms (the 10 ms and 55 ms of Chrome's case, scaled so that only
    // a whole-second stall inside a turn could change a count).
    let b = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    b.evaluate("var n = 0; setInterval(function () { n++; }, 1000);")
        .unwrap();
    b.advance_real_timers_for_frame(5500, 1000, 0).unwrap();
    assert!(matches!(b.evaluate("String(n)").unwrap(), JsValue::String(s) if s == "1"));
    let next = b.next_timer_delay().expect("the interval is set");
    assert_eq!(
        next % 1000,
        500,
        "the interval left its grid: next in {next} ms"
    );
    // On time, once a period.
    b.advance_real_timers_for_frame(next, 1000, 0).unwrap();
    assert!(matches!(b.evaluate("String(n)").unwrap(), JsValue::String(s) if s == "2"));
}

/// Animation frames are the same on both clocks: one round a turn, and a frame a timer of the turn
/// asks for (on the 60 Hz grid, the frame of the timer's time, #1191) is in the round when the turn
/// reaches it.
#[test]
fn animation_frames_are_the_same_on_the_real_clock() {
    let run = |real: bool| {
        let b = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
        b.evaluate(
            "var frames = 0, log = [];              (function f() { frames++; requestAnimationFrame(f); })();              setTimeout(function () { requestAnimationFrame(function () { log.push('f'); }); }, 50);",
        )
        .unwrap();
        let mut seen = Vec::new();
        for delta in [300, 16, 16] {
            if real {
                b.advance_real_timers_for_frame(delta, 1000, 0).unwrap();
            } else {
                b.advance_timers_for_frame(delta, 1000, 0).unwrap();
            }
            seen.push(match b.evaluate("frames + ' ' + log.join()").unwrap() {
                JsValue::String(s) => s,
                other => panic!("{other:?}"),
            });
        }
        seen
    };
    // The loop's first call is the script's own; then one frame a turn, and the timer's frame in the
    // first turn's round.
    assert_eq!(run(false), ["2 f", "3 f", "4 f"]);
    assert_eq!(run(true), run(false));
}

/// Measurement only (CLOCK3 item 1): the zero-delay page on both clocks.
#[test]
#[ignore]
fn measure_zero_delay_chain() {
    for real in [false, true] {
        for _ in 0..3 {
            eprintln!("ZERO-DELAY real={real} {}", run_live(ZERO_DELAY, real));
        }
    }
}

/// A real turn's timers count from the turn's instant, not from when its timer slice begins (#1183
/// R1 follow-up): a turn that ran 40 ms of its own work before its timers (a resize's layout, a
/// parked `load`) has a 100 ms timeout set in it come due 140 ms after the turn's time, as in a
/// browser where the 40 ms have passed. Counted from the slice, it came due 40 ms early (100). Real
/// time is a value here (the harness's `__rk.testRealMs`).
#[test]
fn a_real_turns_timers_count_from_the_turns_instant() {
    let b = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    b.runtime
        .borrow_mut()
        .evaluate_host_bookkeeping("var fake = 1000; __rk.testRealMs = function () { return fake; }; return 0")
        .unwrap();
    b.evaluate("setTimeout(function () { setTimeout(function () {}, 100); }, 0);")
        .unwrap();
    b.advance_real_timers_for_frame_since(10, 1000, 0, 40.0).unwrap();
    assert_eq!(b.next_timer_delay(), Some(140));
}

/// LINKS-SLOW: a timer set in a real turn counts from when it is set, a time with a fraction of a
/// millisecond in it; the delay to it is reported rounded up, so a loop that waits that long finds
/// it due. Rounded down (truncated), the loop woke a fraction early, the timer was not due, and the
/// wait to it, under a millisecond, truncated to 0 and became the loop's 2 ms floor: every link of
/// a 10 ms chain came ~2 ms late (the Mac after #1183: 236-245 links in 3000 ms, Chrome 278-281,
/// `links_lateness_probe_cft148.json`). Real time is a value here (`__rk.testRealMs`).
#[test]
fn a_timer_is_never_reported_due_before_it_is_due() {
    let b = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    b.runtime
        .borrow_mut()
        .evaluate_host_bookkeeping(
            "var fake = 1000; __rk.testRealMs = function () { return fake; }; \
             window.__work = function (ms) { fake += ms; }; return 0",
        )
        .unwrap();
    // A timer callback that works 0.4 ms, then sets the next link: it counts from a fraction.
    b.evaluate("var links = 0; function link() { links++; __work(0.4); setTimeout(link, 10); } setTimeout(link, 0);")
        .unwrap();
    b.advance_real_timers_for_frame(0, 1000, 0).unwrap();
    let read = |b: &DomBindings| match b.evaluate("String(links)").unwrap() {
        JsValue::String(s) => s,
        other => panic!("{other:?}"),
    };
    assert_eq!(read(&b), "1");
    for i in 0..5 {
        let delay = b.next_timer_delay().expect("the next link is set");
        // The loop waits that long; real time moves as much, and the turn is given it.
        b.evaluate(&format!("__work({delay})")).unwrap();
        b.advance_real_timers_for_frame(delay, 1000, 0).unwrap();
        assert_eq!(read(&b), (i + 2).to_string(), "link {} was not due after the {delay} ms reported", i + 2);
    }
}

/// LINKS-SLOW: a timeout never runs before the page clock has moved its delay, whatever a real
/// turn's real-time base reads. The loop's clock and the page's are each re-anchored per turn and
/// were measured up to ~2.4 ms apart; a turn whose base read ahead ran timeouts early on
/// `performance.now()` (min lateness -1.7 to -2.9 ms with the early-aimed wake; Chrome never below
/// its 0.1 ms tick, `links_lateness_probe_cft148.json`). Here a turn is told 3 ms more pre-timer time
/// than passed, so its base reads 3 ms ahead: the 10 ms timeout set 7 ms of page clock earlier does
/// not run; it runs once the page clock has moved 10 ms. Real time is a value (`__rk.testRealMs`).
#[test]
fn a_timeout_never_runs_before_the_page_clock_moves_its_delay() {
    let b = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    b.runtime
        .borrow_mut()
        .evaluate_host_bookkeeping(
            "var fake = 1000; __rk.testRealMs = function () { return fake; }; \
             window.__work = function (ms) { fake += ms; }; return 0",
        )
        .unwrap();
    b.evaluate("var setAt = -1, ranAfter = -1; setTimeout(function () { setAt = performance.now(); \
                setTimeout(function () { ranAfter = performance.now() - setAt; }, 10); }, 0);")
        .unwrap();
    b.advance_real_timers_for_frame(0, 1000, 0).unwrap();
    // 7 ms of real time, and a turn whose base reads 3 ms ahead of it.
    b.evaluate("__work(7)").unwrap();
    b.advance_real_timers_for_frame_since(7, 1000, 0, 3.0).unwrap();
    let read = |b: &DomBindings| match b.evaluate("String(ranAfter)").unwrap() {
        JsValue::String(s) => s,
        other => panic!("{other:?}"),
    };
    assert_eq!(read(&b), "-1", "the timeout ran 7 ms of page clock after it was set");
    b.evaluate("__work(3)").unwrap();
    b.advance_real_timers_for_frame(3, 1000, 0).unwrap();
    let after: f64 = read(&b).parse().unwrap();
    assert!(after >= 10.0, "ran {after} ms of page clock after it was set");
}

/// A fresh bindings whose real time is a value (`__rk.testRealMs`), moved by `__work(ms)`.
fn fake_clock_bindings() -> DomBindings {
    let b = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    b.runtime
        .borrow_mut()
        .evaluate_host_bookkeeping(
            "var fake = 1000; __rk.testRealMs = function () { return fake; }; \
             window.__work = function (ms) { fake += ms; }; window.__real = function () { return fake; }; return 0",
        )
        .unwrap();
    b
}

fn num(b: &DomBindings, expr: &str) -> f64 {
    match b.evaluate(expr).unwrap() {
        JsValue::Number(n) => n,
        other => panic!("{expr}: {other:?}"),
    }
}

/// LINKS-SLOW R1: a timeout held for the page clock is skipped, not waited on: a timer due behind
/// it (one with no page-clock hold: set outside a real turn) still runs in the turn. And the wait
/// the turn reports for the held one counts from the turn's real progress: added to the turn's
/// instant (as the loop does) it does not end before the page clock reaches it.
#[test]
fn a_held_timeout_does_not_hold_back_the_timers_due_behind_it() {
    let b = fake_clock_bindings();
    // B: set on the stepped clock (no hold), due at 12. A: set in the first real turn, due at ~10,
    // held until the page clock has moved 10 from its setting.
    b.evaluate("var bRan = false; setTimeout(function () { bRan = true; __work(0.5); }, 12); \
                var aAfter = -1; setTimeout(function () { var s = performance.now(); \
                setTimeout(function () { aAfter = performance.now() - s; }, 10); }, 0);")
        .unwrap();
    b.advance_real_timers_for_frame(0, 1000, 0).unwrap();
    // 9 ms of real time, and a turn whose base reads 3 ms ahead: both are due on the timers'
    // clock; A's page clock has moved 9.
    b.evaluate("__work(9)").unwrap();
    b.advance_real_timers_for_frame_since(9, 1000, 0, 3.0).unwrap();
    assert!(matches!(b.evaluate("bRan").unwrap(), JsValue::Boolean(true)), "B waited behind the held timeout");
    assert_eq!(num(&b, "aAfter"), -1.0, "A ran before the page clock moved its delay");
    // A needs 0.5 ms more page clock; the turn began 3 ms before its timers and B worked 0.5 ms.
    let wait = b.next_timer_delay_exact().expect("A is set");
    assert!(wait >= 3.0 + 0.5 + 0.5 - 1e-6, "the wait {wait} ends before the page clock reaches A");
}

/// LINKS-SLOW R1: on the deterministic clock setting a timeout takes no page clock reading (each
/// reading moves it a step): what the page reads around `setTimeout` is what it reads without it.
#[test]
fn setting_a_timeout_takes_no_reading_on_the_deterministic_clock() {
    let b = fake_clock_bindings();
    b.runtime.borrow_mut().evaluate_host_bookkeeping("__rk.setDetClock(true); return 0").unwrap();
    b.evaluate("var gap = -1; setTimeout(function () { var a = performance.now(); \
                setTimeout(function () {}, 10); gap = performance.now() - a; }, 0);")
        .unwrap();
    b.advance_real_timers_for_frame(0, 1000, 0).unwrap();
    let gap = num(&b, "gap");
    assert!((gap - 0.1).abs() < 1e-9, "two readings around setTimeout were {gap} apart");
}

/// LINKS-SLOW part 2 R1 (3): an animation frame is not an exact wait (frames land on the 60 Hz grid
/// and the loop's whole-millisecond wait serves them; an exact wait ends with a blocking tail, 60
/// a second on every animated page): only a timer's instant is.
#[test]
fn only_a_timer_not_a_frame_sets_the_exact_wait() {
    let b = fake_clock_bindings();
    b.evaluate("requestAnimationFrame(function () {});").unwrap();
    assert!(b.next_timer_delay().is_some(), "the frame is waited for");
    assert_eq!(b.next_timer_delay_exact(), None, "a frame took the exact wait");
    b.evaluate("setTimeout(function () {}, 30);").unwrap();
    assert!(b.next_timer_delay_exact().is_some(), "the timeout has no exact wait");
}

/// LINKS-SLOW R1: a timeout set outside the timer slice of a real turn (here an animation frame
/// callback after 8 ms of work) counts from when it is set, not from the turn's start: it does not
/// run 2 ms of page clock later.
#[test]
fn a_timeout_set_in_a_frame_callback_waits_its_delay() {
    let b = fake_clock_bindings();
    b.evaluate("var after = -1; requestAnimationFrame(function () { __work(8); var s = performance.now(); \
                setTimeout(function () { after = performance.now() - s; }, 10); });")
        .unwrap();
    // Each turn's delta is the real time since the last turn began, as the content loop's clock
    // gives it: the frame callback's 8 ms of work is in the next one.
    b.evaluate("__work(17)").unwrap();
    let mut last = num(&b, "__real()");
    b.advance_real_timers_for_frame(17, 1000, 0).unwrap();
    for _ in 0..20 {
        b.evaluate("__work(1)").unwrap();
        let now = num(&b, "__real()");
        b.advance_real_timers_for_frame((now - last) as u64, 1000, 0).unwrap();
        last = now;
        let after = num(&b, "after");
        if after >= 0.0 {
            assert!(after >= 10.0, "ran {after} ms of page clock after it was set");
            return;
        }
    }
    panic!("the timeout never ran");
}

/// LINKS-SLOW R1: an interval's first run waits its delay on the page clock too (a turn whose base
/// reads 3 ms ahead, as in `a_timeout_never_runs_before_the_page_clock_moves_its_delay`).
#[test]
fn an_intervals_first_run_waits_its_delay_on_the_page_clock() {
    let b = fake_clock_bindings();
    b.evaluate("var first = -1; setTimeout(function () { var s = performance.now(); \
                var id = setInterval(function () { first = performance.now() - s; clearInterval(id); }, 10); }, 0);")
        .unwrap();
    b.advance_real_timers_for_frame(0, 1000, 0).unwrap();
    b.evaluate("__work(7)").unwrap();
    b.advance_real_timers_for_frame_since(7, 1000, 0, 3.0).unwrap();
    assert_eq!(num(&b, "first"), -1.0, "the interval ran 7 ms of page clock after it was set");
    b.evaluate("__work(3)").unwrap();
    b.advance_real_timers_for_frame(3, 1000, 0).unwrap();
    assert!(num(&b, "first") >= 10.0);
}

/// A timeout chain set between real turns (as an event handler sets it) does not replay the links
/// a long gap held back: after 50 ms of real time with a 10 ms chain, the turn runs one link, not
/// five. On the supplied clock, so no stall of the machine running it can let another link through
/// (moved here from the window thread's test, which slept real time).
#[test]
fn a_chain_set_between_real_turns_does_not_replay_a_gap() {
    let b = fake_clock_bindings();
    b.advance_real_timers_for_frame(0, 1000, 0).unwrap();
    b.evaluate("window.n = 0; (function link() { n++; setTimeout(link, 10); })();").unwrap();
    b.evaluate("__work(50)").unwrap();
    b.advance_real_timers_for_frame(50, 1000, 0).unwrap();
    assert_eq!(num(&b, "n"), 2.0, "the chain replayed the links the 50 ms held back");
}
