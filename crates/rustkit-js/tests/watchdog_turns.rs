//! The watchdog thread stops a script turn that runs past its budget, with
//! no host call needed (threads plan M1): `ThreadStamp` and `ViewControl`
//! on a `JsRuntime`, the way the engine arms its live turns.

use std::sync::Arc;
use std::time::{Duration, Instant};

use rustkit_js::{JsRuntime, ThreadStamp, ViewControl, Watchdog};

/// Far below T0's 5 s so the tests are quick; the watchdog does not care.
const BUDGET: Duration = Duration::from_millis(300);
/// What M1 promises on top of the budget.
const SLACK: Duration = Duration::from_millis(250);

struct Page {
    runtime: JsRuntime,
    stamp: Arc<ThreadStamp>,
    control: Arc<ViewControl>,
    _watchdog: Watchdog,
}

fn page() -> Page {
    let watchdog = Watchdog::spawn();
    let stamp = watchdog.thread_stamp();
    let control = watchdog.view_control();
    let mut runtime = JsRuntime::new().unwrap();
    runtime.set_view_control(Some(control.clone()));
    Page {
        runtime,
        stamp,
        control,
        _watchdog: watchdog,
    }
}

impl Page {
    /// Runs `source` as one turn with `budget`; the result, whether the
    /// turn was stopped, and how long it took.
    fn turn(&mut self, source: &str, budget: Duration) -> (Result<String, String>, bool, Duration) {
        let started = Instant::now();
        self.runtime.begin_turn(&self.stamp, budget);
        let result = self
            .runtime
            .evaluate_script(source)
            .map(|value| format!("{value:?}"))
            .map_err(|e| e.to_string());
        let stopped = self.runtime.take_deadline_hit();
        self.runtime.end_turn(&self.stamp);
        (result, stopped, started.elapsed())
    }
}

#[test]
fn a_pure_spin_stops_within_the_budget_plus_250_ms() {
    let mut page = page();
    for spin in [
        "while (true) {}",
        "[1, 2, 3].forEach(function () { for (;;) {} });",
        "function r(a) { return a.reduce(function (x, y) { return x + y; }, 0); } var a = [1, 2, 3]; for (;;) { r(a); }",
    ] {
        let (result, stopped, took) = page.turn(spin, BUDGET);
        assert!(result.is_err(), "{spin}: {result:?}");
        assert!(stopped, "{spin}: the stop is reported as the budget's");
        assert!(took >= BUDGET, "{spin}: stopped early, {took:?}");
        assert!(took < BUDGET + SLACK, "{spin}: stopped {took:?} after the turn began");
    }
}

#[test]
fn try_catch_around_the_spin_cannot_swallow_the_stop() {
    let mut page = page();
    page.runtime
        .evaluate_script("var caught = 0, finished = 0;")
        .unwrap();
    let (result, stopped, took) = page.turn(
        "try { while (true) {} } catch (e) { caught++; } finally { finished++; } caught += 10;",
        BUDGET,
    );
    assert!(result.is_err() && stopped, "{result:?}");
    assert!(took < BUDGET + SLACK, "{took:?}");
    // The `finally` runs (the stop's grace; a scheduler re-arms there), and
    // the `catch` and what follows the statement do not.
    let (flags, stopped, _) = page.turn("caught + ',' + finished", BUDGET);
    assert_eq!(flags.unwrap(), r#"String("0,1")"#);
    assert!(!stopped);
}

#[test]
fn after_a_stop_the_same_runtime_runs_new_scripts_correctly() {
    let mut page = page();
    page.runtime
        .evaluate_script("var calls = 0; function f(n) { calls++; return n * 2; }")
        .unwrap();
    let (result, _, _) = page.turn("f(1); while (true) { f(2); }", BUDGET);
    assert!(result.is_err());
    let (value, stopped, _) = page.turn(
        "var s = 0; for (var i = 0; i < 100; i++) { s += f(i); } [s, [3, 4].map(f).join('-'), typeof calls].join(' ')",
        BUDGET,
    );
    assert!(!stopped);
    assert_eq!(value.unwrap(), r#"String("9900 6-8 number")"#);
    // Promise jobs run too.
    let (value, _, _) = page.turn(
        "var done = false; Promise.resolve(1).then(function () { done = true; }); 0",
        BUDGET,
    );
    assert!(value.is_ok());
    assert_eq!(page.turn("done", BUDGET).0.unwrap(), "Boolean(true)");
}

#[test]
fn an_interrupt_set_between_turns_does_not_kill_the_next_turn() {
    // Nothing here is about time: the next turn must run whatever the
    // word says. It gets a budget no loaded runner reaches, so a stop can
    // only come from the stale word. With `BUDGET` (300 ms) the debug
    // build's 10,000 iterations ran past it on a loaded Mac (load 50:
    // 1 run in 10), and the watchdog's own stop read as the stale word's.
    const NO_TIME_LIMIT: Duration = Duration::from_secs(60);
    let mut page = page();
    // The previous turn's id, set after it ended: as a watchdog or Stop
    // that fired just too late would.
    page.runtime.begin_turn(&page.stamp, NO_TIME_LIMIT);
    let ended = page.stamp.current_turn();
    page.runtime.end_turn(&page.stamp);
    page.control.interrupt_turn(ended);
    let (value, stopped, took) = page.turn(
        "var n = 0; for (var i = 0; i < 10000; i++) { n++; } n",
        NO_TIME_LIMIT,
    );
    let watchdog_stops = page._watchdog.turns_stopped();
    assert_eq!(
        value.as_deref(),
        Ok("Number(10000.0)"),
        "took {took:?}, watchdog stops {watchdog_stops}"
    );
    assert!(
        !stopped,
        "a word naming an ended turn is not this turn's stop (took {took:?}, watchdog stops {watchdog_stops})"
    );
    assert_eq!(
        watchdog_stops, 0,
        "the watchdog stopped a turn (took {took:?})"
    );
}

#[test]
fn stop_all_stops_every_turn_until_a_new_document_clears_it() {
    let mut page = page();
    page.control.stop_all();
    for _ in 0..3 {
        let (result, stopped, _) = page.turn("(function () { return 1; })()", BUDGET);
        assert!(result.is_err() && stopped);
    }
    assert!(page.control.clear_all());
    let (result, stopped, _) = page.turn("(function () { return 1; })()", BUDGET);
    assert!(result.is_ok() && !stopped);
}

#[test]
fn turns_racing_the_watchdog_are_stopped_only_past_their_budget() {
    // Turns whose length straddles their budget, while two more threads
    // keep other stamps busy, so the watchdog fires during some turns, at
    // their ends, and between them. Every stop must come at or past the
    // stopped turn's budget, and the short turn after each must never be
    // stopped.
    let watchdog = Watchdog::spawn();
    let stamp = watchdog.thread_stamp();
    let control = watchdog.view_control();
    let mut runtime = JsRuntime::new().unwrap();
    runtime.set_view_control(Some(control.clone()));

    let busy: Vec<_> = (0..2)
        .map(|_| {
            let watchdog = watchdog.clone();
            std::thread::spawn(move || {
                let stamp = watchdog.thread_stamp();
                let control = watchdog.view_control();
                let until = Instant::now() + Duration::from_secs(3);
                while Instant::now() < until {
                    let turn = stamp.begin_turn(&control, Duration::from_millis(1));
                    // A stale id and a stop of the open turn, from this side.
                    control.interrupt_turn(turn.saturating_sub(1));
                    std::thread::sleep(Duration::from_micros(200));
                    stamp.end_turn();
                }
            })
        })
        .collect();

    let mut stops = 0;
    for round in 0..40u64 {
        let budget = Duration::from_millis(5 + round % 10);
        let started = Instant::now();
        runtime.begin_turn(&stamp, budget);
        // Half, all or three times the budget: some rounds end first, some
        // race the watchdog, some are stopped.
        let spin_ms = budget.as_millis() as u64 * [1, 2, 6][round as usize % 3] / 2;
        let spin = format!(
            "var t0 = Date.now(); var k = 0; while (Date.now() - t0 < {spin_ms}) {{ k++; }} k > 0"
        );
        let result = runtime.evaluate_script(&spin);
        let stopped = runtime.take_deadline_hit();
        runtime.end_turn(&stamp);
        let took = started.elapsed();
        // The word can land after the script's last instruction: then the
        // turn finished but is still reported over budget, which it was.
        if stopped {
            stops += 1;
            assert!(
                took >= budget,
                "round {round}: stopped at {took:?}, before its {budget:?} budget"
            );
        } else {
            assert!(
                result.is_ok(),
                "round {round}: {result:?} without a reported stop"
            );
        }
        let (check, after) = {
            runtime.begin_turn(&stamp, Duration::from_secs(10));
            let value =
                runtime.evaluate_script("[1, 2, 3].map(function (x) { return x + 1; }).join()");
            let stopped = runtime.take_deadline_hit();
            runtime.end_turn(&stamp);
            (value, stopped)
        };
        assert!(!after, "round {round}: the next turn was stopped");
        assert_eq!(
            format!("{:?}", check.unwrap()),
            r#"String("2,3,4")"#,
            "round {round}"
        );
    }
    for thread in busy {
        thread.join().unwrap();
    }
    assert!(stops > 0, "the watchdog fired in none of the rounds");
}

#[test]
fn once_the_stopped_script_has_returned_the_rest_of_its_turn_runs() {
    // The engine's own script after a stop, in the same turn (the timer
    // table, the page's queued requests), is not killed by the word; the
    // turn is still reported stopped.
    let mut page = page();
    page.runtime.begin_turn(&page.stamp, BUDGET);
    assert!(page.runtime.evaluate_script("while (true) {}").is_err());
    let value = page
        .runtime
        .evaluate_script("var q = [1, 2]; q.splice(0).length")
        .unwrap();
    assert_eq!(format!("{value:?}"), "Number(2.0)");
    assert!(page.runtime.take_deadline_hit());
    page.runtime.end_turn(&page.stamp);
    // ALL keeps stopping everything.
    page.control.stop_all();
    page.runtime.begin_turn(&page.stamp, BUDGET);
    assert!(page.runtime.evaluate_script("(function () {})()").is_err());
    assert!(page.runtime.evaluate_script("(function () {})()").is_err());
    page.runtime.end_turn(&page.stamp);
}

#[test]
fn a_word_landing_after_a_host_call_refusal_does_not_stop_the_rest_of_the_turn() {
    // T0's deadline and the watchdog keep the same budget: the deadline
    // usually ends the script first and the word lands during what the
    // engine runs next. That must still run (#666's lost-request case).
    let mut page = page();
    page.runtime
        .register_host_function("probe", 0, Box::new(|_| rustkit_js::JsValue::Undefined))
        .unwrap();
    let turn = page
        .runtime
        .begin_turn(&page.stamp, Duration::from_secs(60));
    page.runtime
        .set_execution_deadline(Some(Instant::now() - Duration::from_millis(1)));
    assert!(page.runtime.evaluate_script("probe()").is_err());
    page.control.interrupt_turn(turn);
    let value = page
        .runtime
        .evaluate_script("[1, 2].map(function (x) { return x; }).length")
        .unwrap();
    assert_eq!(format!("{value:?}"), "Number(2.0)");
    assert!(page.runtime.take_deadline_hit());
    page.runtime.end_turn(&page.stamp);
}

#[test]
fn bookkeeping_is_not_stopped_by_the_word() {
    let mut page = page();
    page.runtime
        .evaluate_script("var queue = [1, 2, 3];")
        .unwrap();
    page.runtime
        .begin_turn(&page.stamp, Duration::from_secs(60));
    page.control.interrupt_turn(page.stamp.current_turn());
    let taken = page
        .runtime
        .evaluate_bookkeeping("JSON.stringify(queue.splice(0))")
        .unwrap();
    assert_eq!(format!("{taken:?}"), r#"String("[1,2,3]")"#);
    // Page script in the same turn still is.
    assert!(page.runtime.evaluate_script("(function () {})()").is_err());
    page.runtime.end_turn(&page.stamp);
}

/// The script burns its allowance, then the microtask drain resumes an
/// async function and a promise reaction. Both call the host. The grace
/// turn used to leave the host deadline in the past, so those calls threw
/// an uncatchable error and the await path reported "async function was
/// interrupted" with no `vm.check_interrupt` (the word still named the
/// stopped turn).
#[test]
fn a_reaction_after_the_deadline_still_reaches_the_host() {
    let mut page = page();
    page.runtime
        .register_host_function("probe", 0, Box::new(|_| rustkit_js::JsValue::Undefined))
        .unwrap();
    page.runtime
        .evaluate_script("var canary = 'pending', reaction = 'none';")
        .unwrap();
    page.runtime
        .begin_turn(&page.stamp, Duration::from_millis(40));
    page.runtime
        .set_execution_deadline(Some(Instant::now() + Duration::from_millis(40)));
    let script = r#"
        var gate = new Promise(function (resolve) { resolve(); });
        async function STICKY_CANARY() {
          await gate;
          probe();
          canary = 'ok';
        }
        STICKY_CANARY();
        gate.then(function () { probe(); reaction = 'ran'; });
        var t = Date.now();
        while (Date.now() - t < 80) {}
        probe();
    "#;
    let _ = page.runtime.evaluate_script(script);
    let state = page
        .runtime
        .evaluate_script("canary + '|' + reaction")
        .unwrap();
    assert_eq!(format!("{state:?}"), r#"String("ok|ran")"#);
    page.runtime.end_turn(&page.stamp);
}

#[test]
fn after_a_stop_the_turn_gets_a_short_grace_and_its_stop_is_final() {
    // What the stopped turn's finished callbacks queued still runs (an
    // XHR's send is a promise reaction); a new spin in the grace is
    // stopped at its end, and nothing runs after that in this turn.
    let mut page = page();
    page.runtime.begin_turn(&page.stamp, BUDGET);
    let queued =
        "var sent = false; Promise.resolve().then(function () { sent = true; }); while (true) {}";
    assert!(page.runtime.evaluate_script(queued).is_err());
    let started = Instant::now();
    assert!(page.runtime.evaluate_script("while (true) {}").is_err());
    let took = started.elapsed();
    assert!(
        took < rustkit_js::control::STOP_GRACE + SLACK,
        "the grace spin ran {took:?}"
    );
    assert!(page.runtime.evaluate_script("(function () {})()").is_err());
    assert!(page.runtime.take_deadline_hit());
    page.runtime.end_turn(&page.stamp);
    let (sent, stopped, _) = page.turn("sent", BUDGET);
    assert_eq!(sent.unwrap(), "Boolean(true)", "the queued reaction ran");
    assert!(!stopped);
}
