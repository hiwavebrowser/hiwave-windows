//! The vendored Boa's interrupt word (third_party/boa_engine/PATCHES.md):
//! script that never calls the host is stopped from another thread, cannot
//! catch the stop, and leaves the context usable.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use boa_engine::error::{EngineError, RuntimeLimitError};
use boa_engine::{Context, JsError, Source};

/// Every turn, as `ViewControl` stores it.
const ALL: u64 = u64::MAX;

fn context_with_word(turn: u64) -> (Context, Arc<AtomicU64>) {
    let word = Arc::new(AtomicU64::new(0));
    let mut context = Context::default();
    context.set_interrupt_word(Some(word.clone()));
    context.set_interrupt_turn(turn);
    (context, word)
}

fn is_interrupt(err: &JsError) -> bool {
    matches!(
        err.as_engine(),
        Some(EngineError::RuntimeLimit(RuntimeLimitError::Interrupted))
    )
}

/// Sets `word` to `value` from another thread after `after`.
fn set_later(
    word: &Arc<AtomicU64>,
    value: u64,
    after: Duration,
) -> std::thread::JoinHandle<Instant> {
    let word = word.clone();
    std::thread::spawn(move || {
        std::thread::sleep(after);
        word.fetch_max(value, Ordering::Relaxed);
        Instant::now()
    })
}

/// Runs `source`, which must be stopped, and returns how long after the
/// word was set it ended.
fn stopped_after_set(
    context: &mut Context,
    word: &Arc<AtomicU64>,
    turn: u64,
    source: &str,
) -> Duration {
    let setter = set_later(word, turn, Duration::from_millis(100));
    let result = context.eval(Source::from_bytes(source));
    let ended = Instant::now();
    let set_at = setter.join().unwrap();
    let err = result.expect_err("the spin is stopped");
    assert!(is_interrupt(&err), "stopped by the interrupt, got {err}");
    ended.saturating_duration_since(set_at)
}

#[test]
fn a_pure_spin_stops_soon_after_another_thread_sets_the_word() {
    for source in [
        "while (true) {}",
        "for (;;) {}",
        "do {} while (true);",
        // The callbacks forEach makes: the eBay shape, with no host call.
        "[1, 2, 3].forEach(function () { while (true) {} });",
        // A reducer with no loop of its own: stopped at its calls.
        "var a = new Array(1000).fill(1); for (;;) { a.reduce(function (x, y) { return x + y; }, 0); }",
    ] {
        let (mut context, word) = context_with_word(7);
        let late = stopped_after_set(&mut context, &word, 7, source);
        assert!(late < Duration::from_millis(50), "{source}: stopped {late:?} after the word was set");
    }
}

#[test]
fn try_catch_and_finally_cannot_swallow_the_stop() {
    let (mut context, word) = context_with_word(3);
    let source = r#"
        var caught = false, finished = false, after = false;
        try {
            try { while (true) {} } catch (e) { caught = true; } finally { finished = true; }
            after = true;
        } catch (e) { caught = true; }
    "#;
    stopped_after_set(&mut context, &word, 3, source);
    // The next turn reads what the stopped one left.
    context.set_interrupt_turn(4);
    let flags = context
        .eval(Source::from_bytes("[caught, finished, after].join()"))
        .unwrap()
        .to_string(&mut context)
        .unwrap()
        .to_std_string_escaped();
    assert_eq!(flags, "false,false,false");
}

#[test]
fn the_stop_unwinds_nested_calls_and_promise_jobs() {
    let (mut context, word) = context_with_word(5);
    // Deep in nested calls, from a promise job.
    let source = r#"
        var rejected = false;
        function inner() { while (true) {} }
        function outer(n) { return n == 0 ? inner() : outer(n - 1); }
        Promise.resolve().then(function () { outer(20); }).catch(function () { rejected = true; });
    "#;
    context.eval(Source::from_bytes(source)).unwrap();
    let setter = set_later(&word, 5, Duration::from_millis(100));
    let result = context.run_jobs();
    setter.join().unwrap();
    let err = result.expect_err("the job is stopped");
    assert!(is_interrupt(&err), "got {err}");
    // An async function cannot turn the stop into a rejection either.
    context.set_interrupt_turn(6);
    context
        .eval(Source::from_bytes(
            "async function spin() { await null; while (true) {} } spin().catch(function () { rejected = true; });",
        ))
        .unwrap();
    let setter = set_later(&word, 6, Duration::from_millis(100));
    let result = context.run_jobs();
    let ended = Instant::now();
    let set_at = setter.join().unwrap();
    // The `Await` reactions return the stop of the body they resume
    // (PATCHES.md), so it reaches `run_jobs`' caller.
    let err = result.expect_err("the async spin is stopped");
    assert!(is_interrupt(&err), "got {err}");
    let late = ended.saturating_duration_since(set_at);
    assert!(
        late < Duration::from_millis(50),
        "the async spin stopped {late:?} after the word was set"
    );
    context.set_interrupt_turn(7);
    context.run_jobs().unwrap();
    let rejected = context.eval(Source::from_bytes("rejected")).unwrap();
    assert_eq!(
        rejected.as_boolean(),
        Some(false),
        "no catch handler saw the stop"
    );
}

#[test]
fn after_a_stop_the_same_context_runs_new_scripts_correctly() {
    let (mut context, word) = context_with_word(1);
    context
        .eval(Source::from_bytes(
            "var total = 0; function add(n) { total += n; return total; }",
        ))
        .unwrap();
    stopped_after_set(
        &mut context,
        &word,
        1,
        "function f() { add(1); while (true) {} } f();",
    );
    context.set_interrupt_turn(2);
    let value = context
        .eval(Source::from_bytes(
            "var s = 0; for (var i = 0; i < 1000; i++) { s += i; } [add(10), s, [1, 2, 3].map(function (x) { return x * 2; }).join()].join(' ')",
        ))
        .unwrap()
        .to_string(&mut context)
        .unwrap()
        .to_std_string_escaped();
    assert_eq!(value, "11 499500 2,4,6");
}

#[test]
fn a_word_naming_an_ended_turn_does_not_stop_the_next() {
    let (mut context, word) = context_with_word(10);
    // Set between turns, naming the one that ended.
    word.fetch_max(10, Ordering::Relaxed);
    context.set_interrupt_turn(11);
    let value = context
        .eval(Source::from_bytes(
            "var n = 0; for (var i = 0; i < 100000; i++) { n++; } n",
        ))
        .unwrap();
    assert_eq!(value.as_number(), Some(100000.0));
    // And 0 stops nothing, whatever the turn.
    word.store(0, Ordering::Relaxed);
    context.set_interrupt_turn(0);
    assert!(context
        .eval(Source::from_bytes("for (var i = 0; i < 1000; i++) {} 1"))
        .is_ok());
}

#[test]
fn all_stops_every_turn_until_cleared_and_a_turn_id_does_not_replace_it() {
    let (mut context, word) = context_with_word(20);
    word.store(ALL, Ordering::Relaxed);
    for turn in [20, 21, 1_000] {
        context.set_interrupt_turn(turn);
        let err = context
            .eval(Source::from_bytes("(function () { return 1; })()"))
            .unwrap_err();
        assert!(is_interrupt(&err));
    }
    // A watchdog's fetch_max after ALL leaves ALL.
    word.fetch_max(1_000, Ordering::Relaxed);
    assert_eq!(word.load(Ordering::Relaxed), ALL);
    // Only a new document clears it, with compare_exchange.
    assert!(word
        .compare_exchange(ALL, 0, Ordering::Relaxed, Ordering::Relaxed)
        .is_ok());
    assert!(context
        .eval(Source::from_bytes("(function () { return 1; })()"))
        .is_ok());
}

#[test]
fn setting_and_clearing_the_word_under_load_never_stops_the_wrong_turn() {
    // A thread hammers the word with the ids of turns that already ended
    // and with 0, while this thread runs short turns that must finish; then
    // it names the running turn, which must stop.
    let (mut context, word) = context_with_word(0);
    let current = Arc::new(AtomicU64::new(0));
    let done = Arc::new(AtomicBool::new(false));
    let hammer = {
        let (word, current, done) = (word.clone(), current.clone(), done.clone());
        std::thread::spawn(move || {
            let mut writes = 0u64;
            while !done.load(Ordering::Relaxed) {
                let running = current.load(Ordering::Acquire);
                if running > 1 {
                    // A stale id, then a clear.
                    word.store(running - 1, Ordering::Relaxed);
                    word.store(0, Ordering::Relaxed);
                    writes += 1;
                }
            }
            writes
        })
    };
    for turn in 2..400u64 {
        context.set_interrupt_turn(turn);
        current.store(turn, Ordering::Release);
        let value = context
            .eval(Source::from_bytes("var k = 0; for (var i = 0; i < 2000; i++) { k = (function (x) { return x + 1; })(k); } k"))
            .unwrap_or_else(|e| panic!("turn {turn} was stopped by a stale word: {e}"));
        assert_eq!(value.as_number(), Some(2000.0));
    }
    done.store(true, Ordering::Relaxed);
    assert!(
        hammer.join().unwrap() > 0,
        "the word was written during the turns"
    );

    let turn = 400;
    context.set_interrupt_turn(turn);
    let late = stopped_after_set(&mut context, &word, turn, "while (true) {}");
    assert!(
        late < Duration::from_millis(50),
        "stopped {late:?} after the word was set"
    );
}
