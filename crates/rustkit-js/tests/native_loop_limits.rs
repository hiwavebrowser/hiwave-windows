//! Native loops over a page's string must not cost more than the string is long, and the turn budget must
//! be able to stop them.
//!
//! - A global regular-expression match over a Latin-1 string copied the whole string for every match, so
//!   matching a long string was quadratic. Counted here as bytes allocated (deterministic; no clock).
//! - The loops in `String.prototype.replaceAll` and `split` call nothing per step once they have
//!   started, so the interrupt checks that run on a call never ran: only a check in the loop can stop
//!   them. (The `RegExp.prototype` loops call `exec` on every step, and that call is checked.) No clock:
//!   the search value's `toString` sets the interrupt word just before the loop starts, and the rest of
//!   the loop calls nothing.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use boa_engine::error::{EngineError, RuntimeLimitError};
use boa_engine::{js_string, Context, JsValue, NativeFunction, Source};

thread_local! {
    // Const-initialised: reading it never allocates, so the allocator can use it.
    static ALLOCATED: Cell<u64> = const { Cell::new(0) };
}

struct Counting;
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let _ = ALLOCATED.try_with(|n| n.set(n.get() + l.size() as u64));
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        let _ = ALLOCATED.try_with(|c| c.set(c.get() + n as u64));
        unsafe { System.realloc(p, l, n) }
    }
}
#[global_allocator]
static A: Counting = Counting;

const ALL: u64 = u64::MAX;

fn context_with_stop() -> Context {
    let word = Arc::new(AtomicU64::new(0));
    let mut context = Context::default();
    context.set_interrupt_word(Some(word.clone()));
    context.set_interrupt_turn(ALL);
    // SAFETY: the closure captures an `Arc<AtomicU64>`, which holds nothing the collector traces.
    let stop = unsafe {
        NativeFunction::from_closure(move |_, _, _| {
            word.fetch_max(ALL, Ordering::Relaxed);
            Ok(JsValue::undefined())
        })
    };
    context.register_global_callable(js_string!("stopNow"), 0, stop).unwrap();
    context
}

/// Bytes allocated on this thread while `body` runs, after `setup`.
fn bytes_for(setup: &str, body: &str) -> u64 {
    let mut context = Context::default();
    context.eval(Source::from_bytes(setup)).unwrap();
    let before = ALLOCATED.with(Cell::get);
    context.eval(Source::from_bytes(body)).unwrap();
    ALLOCATED.with(Cell::get) - before
}

#[test]
fn a_global_match_over_a_long_string_allocates_about_what_it_returns() {
    // 8,192 matches in a 16,384-character Latin-1 string. Copying the string for each match is
    // 8,192 x 32 KiB = 256 MiB; what the matches themselves need is a few MiB.
    let setup = "var x = 'xy'.repeat(2 ** 13);";
    let limit = 32 << 20;
    for body in [
        "x.match(/x/g).length",
        "x.replace(/x/g, 'z').length",
        "x.split(/y/).length",
        "var re = /x/g, n = 0; while (re.exec(x)) n++; n",
        "[...x.matchAll(/x/g)].length",
    ] {
        let bytes = bytes_for(setup, body);
        assert!(bytes < limit, "{body}: {bytes} bytes allocated");
    }
}

fn assert_stopped(context: &mut Context, src: &str) {
    let err = context.eval(Source::from_bytes(src)).expect_err(src);
    assert!(
        matches!(err.as_engine(), Some(EngineError::RuntimeLimit(RuntimeLimitError::Interrupted))),
        "{src}: stopped by the interrupt, got {err}"
    );
}

#[test]
fn the_turn_budget_stops_a_string_search_loop() {
    for call in ["x.replaceAll(search, 'z')", "x.split(search)"] {
        let mut context = context_with_stop();
        let src = format!(
            "var x = 'xy'.repeat(2 ** 10), search = {{ toString() {{ stopNow(); return 'x'; }} }};
             {call}"
        );
        assert_stopped(&mut context, &src);
    }
}
