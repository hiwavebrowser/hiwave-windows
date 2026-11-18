//! The UCS-2 copy of a Latin-1 regexp input that the engine keeps between matches (so a global match
//! over a long string is not quadratic) must not outlive the page's use of it: it holds the source
//! string as well, so one match on a very long string would otherwise pin both for the life of the
//! thread.
//!
//! - It is released when a script turn ends.
//! - An input past the cap is never kept, so the bound holds on paths that arm no turn.

use std::sync::Arc;
use std::time::Duration;

use boa_engine::builtins::regexp::kept_input_units;
use rustkit_js::{JsRuntime, ThreadStamp, ViewControl, Watchdog};

struct Page {
    runtime: JsRuntime,
    stamp: Arc<ThreadStamp>,
    _control: Arc<ViewControl>,
    _watchdog: Watchdog,
}

fn page() -> Page {
    let watchdog = Watchdog::spawn();
    let stamp = watchdog.thread_stamp();
    let control = watchdog.view_control();
    let mut runtime = JsRuntime::new().unwrap();
    runtime.set_view_control(Some(control.clone()));
    Page { runtime, stamp, _control: control, _watchdog: watchdog }
}

#[test]
fn the_kept_regexp_input_is_released_when_the_turn_ends() {
    let mut page = page();
    page.runtime.begin_turn(&page.stamp, Duration::from_secs(60));
    page.runtime
        .evaluate_script("var x = 'xy'.repeat(2 ** 12); x.match(/x/g).length")
        .unwrap();
    // Not vacuous: the copy is kept while the turn runs.
    assert_eq!(kept_input_units(), 2 << 12, "kept during the turn");
    page.runtime.end_turn(&page.stamp);
    assert_eq!(kept_input_units(), 0, "released when the turn ends");
}

#[test]
fn a_regexp_input_past_the_cap_is_never_kept() {
    let mut page = page();
    // Just under the cap (16 Mi units) is kept; just over it is not.
    page.runtime.evaluate_script("var a = 'x'.repeat(2 ** 24); /y/.test(a)").unwrap();
    assert_eq!(kept_input_units(), 1 << 24, "an input at the cap is kept");
    page.runtime
        .evaluate_script("a = null; var b = 'x'.repeat(2 ** 24 + 1); /y/.test(b)")
        .unwrap();
    assert_eq!(kept_input_units(), 0, "an input past the cap is not kept");
}
