//! A stop runs the `finally` blocks it unwinds through, for a grace, and
//! never a `catch` (third_party/boa_engine/PATCHES.md, "stop finally").
//!
//! A scheduler that posts its next task from a `finally` (React's does)
//! was left believing a task was still running once the watchdog stopped
//! one, and the page never ran another (2026-10-22).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use boa_engine::error::{EngineError, RuntimeLimitError};
use boa_engine::{Context, JsError, Source};

const GRACE: Duration = Duration::from_millis(50);

fn context_with_word(turn: u64) -> (Context, Arc<AtomicU64>) {
    let word = Arc::new(AtomicU64::new(0));
    let mut context = Context::default();
    context.set_interrupt_word(Some(word.clone()));
    context.set_interrupt_turn(turn);
    context.set_stop_finally_grace(Some(GRACE));
    (context, word)
}

fn is_interrupt(err: &JsError) -> bool {
    matches!(
        err.as_engine(),
        Some(EngineError::RuntimeLimit(RuntimeLimitError::Interrupted))
    )
}

/// Runs `source` with the word set to `value` after 50 ms, from another
/// thread. It must be stopped; returns how long it ran past the set.
fn run_stopped(context: &mut Context, word: &Arc<AtomicU64>, value: u64, source: &str) -> Duration {
    let setter = {
        let word = word.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            word.fetch_max(value, Ordering::Relaxed);
            Instant::now()
        })
    };
    let result = context.eval(Source::from_bytes(source));
    let ended = Instant::now();
    let set_at = setter.join().unwrap();
    let err = result.expect_err("the spin is stopped");
    assert!(is_interrupt(&err), "stopped by the interrupt, got {err}");
    ended.saturating_duration_since(set_at)
}

/// `js` read in a new turn, as a string.
fn read(context: &mut Context, turn: u64, js: &str) -> String {
    context.set_interrupt_turn(turn);
    context
        .eval(Source::from_bytes(js))
        .unwrap()
        .to_string(context)
        .unwrap()
        .to_std_string_escaped()
}

#[test]
fn a_stop_runs_finally_blocks_innermost_first_and_no_catch() {
    let (mut context, word) = context_with_word(3);
    let source = r#"
        var log = [];
        function inner() {
            try { while (true) {} }
            catch (e) { log.push('inner catch'); }
            finally { log.push('inner finally'); }
            log.push('after inner');
        }
        try {
            try { inner(); }
            finally { log.push('outer finally'); }
            log.push('after outer');
        } catch (e) { log.push('outer catch'); }
    "#;
    run_stopped(&mut context, &word, 3, source);
    assert_eq!(read(&mut context, 4, "log.join()"), "inner finally,outer finally");
}

#[test]
fn a_finally_can_schedule_the_next_task() {
    let (mut context, word) = context_with_word(3);
    let source = r#"
        var queue = [], posted = 0;
        function post(cb) { queue.push(cb); posted++; }
        function work() {
            try { while (true) {} }
            finally { post(function next() {}); }
        }
        [1].forEach(function () { work(); });
    "#;
    run_stopped(&mut context, &word, 3, source);
    assert_eq!(read(&mut context, 4, "String(posted)"), "1");
    // And the context runs the next task normally.
    assert_eq!(read(&mut context, 5, "queue[0](); 'ran'"), "ran");
}

#[test]
fn stop_runs_no_finally() {
    let (mut context, word) = context_with_word(3);
    let source = "var ran = false; try { while (true) {} } finally { ran = true; }";
    run_stopped(&mut context, &word, u64::MAX, source);
    word.store(0, Ordering::Relaxed);
    assert_eq!(read(&mut context, 4, "String(ran)"), "false");
}

#[test]
fn without_a_grace_no_finally_runs() {
    let (mut context, word) = context_with_word(3);
    context.set_stop_finally_grace(None);
    let source = "var ran = false; try { while (true) {} } finally { ran = true; }";
    run_stopped(&mut context, &word, 3, source);
    assert_eq!(read(&mut context, 4, "String(ran)"), "false");
}

#[test]
fn a_finally_cannot_swallow_the_stop() {
    for (finally, expect) in [
        // `return` in a `finally` would end the throw it was entered with.
        ("function f() { try { while (true) {} } finally { log.push('f'); return 1; } } f(); log.push('returned');", "f"),
        // A throw out of the block, caught around it.
        ("try { try { while (true) {} } finally { log.push('f'); throw 1; } } catch (e) { log.push('caught'); }", "f"),
        // `break` out of it.
        ("for (;;) { try { while (true) {} } finally { log.push('f'); break; } } log.push('broke');", "f"),
    ] {
        let (mut context, word) = context_with_word(3);
        let source = format!("var log = []; {finally}");
        let late = run_stopped(&mut context, &word, 3, &source);
        assert_eq!(read(&mut context, 4, "log.join()"), expect, "{finally}");
        assert!(late < GRACE + Duration::from_millis(100), "{finally}: {late:?}");
    }
}

#[test]
fn a_catch_inside_the_finally_is_the_pages() {
    let (mut context, word) = context_with_word(3);
    let source = r#"
        var log = [];
        try { while (true) {} }
        finally {
            try { JSON.parse('{'); } catch (e) { log.push('parse failed'); }
            try { (function () { throw 2; })(); } catch (e) { log.push('caught ' + e); }
            log.push('end');
        }
        log.push('after');
    "#;
    run_stopped(&mut context, &word, 3, source);
    assert_eq!(read(&mut context, 4, "log.join()"), "parse failed,caught 2,end");
}

#[test]
fn a_finally_that_does_not_end_is_stopped_at_the_grace() {
    let (mut context, word) = context_with_word(3);
    let source = r#"
        var log = [];
        try {
            try { while (true) {} } finally { log.push('spin'); while (true) {} }
        } finally { log.push('outer'); }
    "#;
    let late = run_stopped(&mut context, &word, 3, source);
    assert!(late >= GRACE, "the finally had its grace: {late:?}");
    assert!(late < GRACE + Duration::from_millis(100), "{late:?}");
    // The grace is over: the outer block does not run.
    assert_eq!(read(&mut context, 4, "log.join()"), "spin");
    // The next turn is not stopped.
    assert_eq!(read(&mut context, 5, "var n = 0; for (var i = 0; i < 1000; i++) n++; String(n)"), "1000");
}

#[test]
fn the_loop_limit_runs_finally_too() {
    let mut context = Context::default();
    context.set_stop_finally_grace(Some(GRACE));
    context.runtime_limits_mut().set_loop_iteration_limit(10_000);
    let err = context
        .eval(Source::from_bytes("var ran = false; try { while (true) {} } finally { ran = true; }"))
        .expect_err("the loop limit stops it");
    assert!(matches!(
        err.as_engine(),
        Some(EngineError::RuntimeLimit(RuntimeLimitError::LoopIteration))
    ), "{err}");
    let ran = context.eval(Source::from_bytes("String(ran)")).unwrap();
    assert_eq!(ran.to_string(&mut context).unwrap().to_std_string_escaped(), "true");
}

#[test]
fn a_generator_and_an_async_function_run_their_finally() {
    let (mut context, word) = context_with_word(3);
    let source = r#"
        var log = [];
        function* g() { try { yield 1; while (true) {} } finally { log.push('gen'); } }
        async function a() { try { await null; while (true) {} } finally { log.push('async'); } }
        a();
        var it = g(); it.next(); it.next();
    "#;
    run_stopped(&mut context, &word, 3, source);
    assert_eq!(read(&mut context, 4, "log.join()"), "gen");
    // The async function's body resumes in a job of a later turn, and is
    // stopped there.
    let setter = {
        let word = word.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            word.fetch_max(4, Ordering::Relaxed);
        })
    };
    let result = context.run_jobs();
    setter.join().unwrap();
    let err = result.expect_err("the job is stopped");
    assert!(is_interrupt(&err), "{err}");
    assert_eq!(read(&mut context, 5, "log.join()"), "gen,async");
}

/// An `await` in a `finally` the stop runs suspends the function, and the
/// script around it ends normally (R1 on #776). The stop is over then: the
/// next script, in a new turn, runs even when nobody calls
/// `end_stop_unwind`.
#[test]
fn a_script_that_ends_normally_after_a_stop_leaves_no_stop_behind() {
    let (mut context, word) = context_with_word(3);
    let setter = {
        let word = word.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            word.fetch_max(3, Ordering::Relaxed);
        })
    };
    let ended = context.eval(Source::from_bytes(
        "async function g() { try { while (true) {} } finally { await 0; } } g(); 'done'",
    ));
    setter.join().unwrap();
    let ended = ended.expect("the await suspends g, and the script ends");
    assert_eq!(ended.to_string(&mut context).unwrap().to_std_string_escaped(), "done");
    // Past the grace, in a new turn: not stopped.
    std::thread::sleep(GRACE + Duration::from_millis(20));
    assert_eq!(read(&mut context, 4, "var n = 0; for (var i = 0; i < 1000; i++) n++; String(n)"), "1000");
}
